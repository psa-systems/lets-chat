//! Admin backup / restore (LC-95).
//!
//! Two halves to this module:
//!
//! - **Backup**: [`build_archive`] takes a consistent snapshot of the
//!   three SQLite databases via `VACUUM INTO`, walks the on-disk
//!   `uploads/` and `avatars/` trees, and writes the lot into a
//!   single zip with a `manifest.json` describing every entry's
//!   size + sha256. The route layer streams the resulting file to
//!   the admin.
//!
//! - **Restore**: [`verify_archive`] re-hashes every file against the
//!   manifest and refuses on any drift; [`stage_extract`] unpacks the
//!   archive into a sibling staging directory; the admin route drops
//!   a [`MARKER_FILENAME`] marker inside the live data dir. On the
//!   next startup, [`apply_pending_restore`] atomically swaps the
//!   staging dir into place, preserving the previous data dir under a
//!   timestamped suffix.
//!
//! Encryption is intentionally out of scope for this PR per the
//! locked design choice; the archive is a plain zip. Operators who
//! need at-rest encryption pipe the download through their own tool.
//!
//! ## Threading model
//!
//! `build_archive` is async because `VACUUM INTO` runs through sqlx;
//! the subsequent zip build (which reads + hashes every file in the
//! uploads tree) is pure synchronous filesystem work and runs inside
//! a `spawn_blocking` so it can't park a tokio worker on a multi-GB
//! deployment. [`verify_archive`] and [`stage_extract`] stay sync;
//! the route layer wraps both in `spawn_blocking` for the same
//! reason.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AppError;

pub const MARKER_FILENAME: &str = ".restore-pending";
pub const STAGED_SUFFIX: &str = ".staged-restore";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Manifest {
    pub version: String,
    pub git_hash: String,
    pub created_at: String,
    pub files: Vec<ManifestEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ManifestEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

pub fn staged_dir_for(data_dir: &Path) -> PathBuf {
    let s = data_dir.as_os_str().to_string_lossy();
    PathBuf::from(format!("{s}{STAGED_SUFFIX}"))
}

pub fn marker_path_for(data_dir: &Path) -> PathBuf {
    data_dir.join(MARKER_FILENAME)
}

/// Synchronous startup hook: if a `.restore-pending` marker exists
/// inside `data_dir`, atomically swap the sibling staging directory
/// into place. The previous data dir is preserved under a
/// timestamped sibling so the operator can roll back by renaming.
///
/// Called BEFORE any SQLite pool opens; failures bubble up so main
/// can fail-fast (a half-renamed state should not start serving).
pub fn apply_pending_restore(data_dir: &Path) -> std::io::Result<()> {
    let marker = marker_path_for(data_dir);
    if !marker.exists() {
        return Ok(());
    }
    let staged = staged_dir_for(data_dir);
    if !staged.exists() {
        // Marker survived a prior failed restore. Drop it and
        // continue; the operator already knows something went wrong
        // (the previous startup would have logged the cause).
        tracing::warn!(
            marker = %marker.display(),
            staged = %staged.display(),
            "restore marker present but staging directory missing; clearing marker"
        );
        std::fs::remove_file(&marker)?;
        return Ok(());
    }
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let parent = data_dir.parent().unwrap_or(Path::new("."));
    let basename = data_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "data".to_string());
    let replaced = parent.join(format!("{basename}.replaced-{ts}"));
    tracing::info!(
        src = %staged.display(),
        replacing = %data_dir.display(),
        backup = %replaced.display(),
        "applying pending restore"
    );
    cross_device_aware_rename(data_dir, &replaced)?;
    cross_device_aware_rename(&staged, data_dir)?;
    Ok(())
}

/// `std::fs::rename` but with a clearer error when the operator's
/// data dir is on a different filesystem from its parent. EXDEV is
/// surfaced verbatim today and looks like an opaque "Invalid cross-
/// device link" string in the journal; this wrapper translates it
/// into an actionable hint.
fn cross_device_aware_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        // EXDEV = 18 on Linux. The portable `ErrorKind::CrossesDevices`
        // landed in a recent std version; match by raw OS error to
        // avoid the MSRV bump.
        Err(e) if e.raw_os_error() == Some(18) => Err(std::io::Error::other(format!(
            "cannot rename {} to {}: the source and destination are on \
             different filesystems (EXDEV). The data directory and its \
             parent must live on the same mount for the atomic-rename \
             swap to work. Mount them on the same volume or move the \
             data dir under a path whose parent you control.",
            from.display(),
            to.display()
        ))),
        Err(e) => Err(e),
    }
}

/// Build a backup archive at `output`. Snapshots the three pools via
/// SQLite's `VACUUM INTO` (async, sqlx), then defers the zip build
/// (sync, CPU + filesystem) to a blocking pool task so the tokio
/// worker stays free.
pub async fn build_archive(
    auth: &sqlx::SqlitePool,
    chat: &sqlx::SqlitePool,
    settings: &sqlx::SqlitePool,
    data_dir: &Path,
    output: &Path,
) -> Result<Manifest, AppError> {
    let parent = output
        .parent()
        .ok_or_else(|| AppError::Internal("archive output has no parent".into()))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| AppError::Internal(format!("create archive parent: {e}")))?;
    let work_dir = parent.join(format!(".lc-backup-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&work_dir)
        .await
        .map_err(|e| AppError::Internal(format!("create work dir: {e}")))?;

    // VACUUM INTO does not accept bound parameters; interpolate as a
    // SQLite string literal. The path is fully server-controlled
    // (uuid + fixed filename) so the escape is paranoia, not a
    // security gate. `raw_sql` is the idiomatic call for a non-
    // prepareable DDL statement.
    let pairs: [(&str, &sqlx::SqlitePool); 3] = [
        ("auth.db", auth),
        ("chat.db", chat),
        ("settings.db", settings),
    ];
    let mut snapshot_paths: Vec<(String, PathBuf)> = Vec::new();
    for (name, pool) in pairs {
        let dst = work_dir.join(name);
        let sql = format!(
            "VACUUM INTO {}",
            sqlite_string_literal(&dst.to_string_lossy())
        );
        sqlx::raw_sql(&sql)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("VACUUM INTO {name}: {e}")))?;
        if !dst.exists() {
            // VACUUM INTO can silently no-op against an `sqlite::memory:`
            // pool whose connections each have their own DB. Surface it
            // as an explicit error rather than failing later with a
            // confusing read-file diagnostic.
            return Err(AppError::Internal(format!(
                "VACUUM INTO {name} produced no file at {}",
                dst.display(),
            )));
        }
        snapshot_paths.push((name.to_string(), dst));
    }

    // Heavy work: walking uploads/, reading each file, hashing,
    // writing into the zip. Deferred to the blocking pool so a
    // multi-GB tree does not stall a tokio worker for many seconds.
    let data_dir_owned = data_dir.to_path_buf();
    let output_owned = output.to_path_buf();
    let work_dir_owned = work_dir.clone();
    tokio::task::spawn_blocking(move || {
        build_zip_blocking(work_dir_owned, snapshot_paths, data_dir_owned, output_owned)
    })
    .await
    .map_err(|e| AppError::Internal(format!("zip task join: {e}")))?
}

/// Sync half of `build_archive`. Owns its inputs so it can run on
/// the blocking pool without lifetime ties to the caller.
fn build_zip_blocking(
    work_dir: PathBuf,
    snapshot_paths: Vec<(String, PathBuf)>,
    data_dir: PathBuf,
    output: PathBuf,
) -> Result<Manifest, AppError> {
    let mut entries: Vec<ManifestEntry> = Vec::new();
    let zf = std::fs::File::create(&output)
        .map_err(|e| AppError::Internal(format!("create archive: {e}")))?;
    let mut zw = zip::ZipWriter::new(zf);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for (name, snap) in &snapshot_paths {
        stream_file_into_zip(&mut zw, name, snap, opts, &mut entries)?;
    }
    let uploads_root = data_dir.join("uploads");
    if uploads_root.exists() {
        walk_into_zip(&mut zw, &uploads_root, "uploads", opts, &mut entries)?;
    }
    let avatars_root = data_dir.join("avatars");
    if avatars_root.exists() {
        walk_into_zip(&mut zw, &avatars_root, "avatars", opts, &mut entries)?;
    }
    let manifest = Manifest {
        version: crate::version::VERSION.to_string(),
        git_hash: crate::version::GIT_HASH.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        files: entries,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| AppError::Internal(format!("manifest serialize: {e}")))?;
    zw.start_file("manifest.json", opts)
        .map_err(|e| AppError::Internal(format!("zip manifest header: {e}")))?;
    zw.write_all(&manifest_bytes)
        .map_err(|e| AppError::Internal(format!("zip manifest body: {e}")))?;
    zw.finish()
        .map_err(|e| AppError::Internal(format!("zip finish: {e}")))?;
    let _ = std::fs::remove_dir_all(&work_dir);
    Ok(manifest)
}

/// Chunked copy from `src` into the zip writer, hashing on the fly.
/// Peak memory is the buffer size (64 KiB) regardless of the source
/// file's size, so a multi-GB SQLite snapshot or upload streams
/// through without pulling the whole thing into a `Vec<u8>`.
fn stream_file_into_zip(
    zw: &mut zip::ZipWriter<std::fs::File>,
    name: &str,
    src: &Path,
    opts: zip::write::SimpleFileOptions,
    entries: &mut Vec<ManifestEntry>,
) -> Result<(), AppError> {
    zw.start_file(name, opts)
        .map_err(|e| AppError::Internal(format!("zip start {name}: {e}")))?;
    let mut f = std::fs::File::open(src)
        .map_err(|e| AppError::Internal(format!("open {}: {e}", src.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| AppError::Internal(format!("read {}: {e}", src.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        zw.write_all(&buf[..n])
            .map_err(|e| AppError::Internal(format!("zip write {name}: {e}")))?;
        total += n as u64;
    }
    let sha: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    entries.push(ManifestEntry {
        path: name.to_string(),
        size: total,
        sha256: sha,
    });
    Ok(())
}

fn walk_into_zip(
    zw: &mut zip::ZipWriter<std::fs::File>,
    src: &Path,
    arc_prefix: &str,
    opts: zip::write::SimpleFileOptions,
    entries: &mut Vec<ManifestEntry>,
) -> Result<(), AppError> {
    let reader = std::fs::read_dir(src)
        .map_err(|e| AppError::Internal(format!("readdir {}: {e}", src.display())))?;
    for entry in reader {
        let entry = entry.map_err(|e| AppError::Internal(format!("readdir entry: {e}")))?;
        let ty = entry
            .file_type()
            .map_err(|e| AppError::Internal(format!("file_type: {e}")))?;
        let name_os = entry.file_name();
        let name = name_os.to_string_lossy();
        // The upload pipeline streams new uploads through `uploads/.tmp/`
        // and `*.tmp` siblings; skip both so we never archive a half-
        // written file (and so its absence after restore is harmless).
        if name == ".tmp" || name.ends_with(".tmp") {
            continue;
        }
        let path = entry.path();
        let arc_name = format!("{arc_prefix}/{name}");
        if ty.is_dir() {
            walk_into_zip(zw, &path, &arc_name, opts, entries)?;
        } else if ty.is_file() {
            stream_file_into_zip(zw, &arc_name, &path, opts, entries)?;
        }
    }
    Ok(())
}

fn sqlite_string_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Parse `"major.minor.patch"` into `(major, minor)`. Returns `None`
/// when either segment is missing or non-numeric. Used by the version
/// gate so a patch-level upgrade does not refuse a backup made on
/// the previous patch.
fn semver_major_minor(v: &str) -> Option<(u32, u32)> {
    let mut parts = v.split('.');
    let maj = parts.next()?.parse().ok()?;
    let min = parts.next()?.parse().ok()?;
    Some((maj, min))
}

/// Parse the manifest and re-hash every entry against it. Refuses
/// archives whose major.minor version differs from the running
/// binary; patch-level differences (a re-deploy or a bugfix release)
/// are accepted. Returns the validated manifest on success.
pub fn verify_archive(zip_path: &Path) -> Result<Manifest, AppError> {
    let f = std::fs::File::open(zip_path)
        .map_err(|e| AppError::Internal(format!("open archive: {e}")))?;
    let mut archive = zip::ZipArchive::new(f)
        .map_err(|e| AppError::BadRequest(format!("not a valid zip: {e}")))?;
    let manifest: Manifest = {
        let mut file = archive
            .by_name("manifest.json")
            .map_err(|_| AppError::BadRequest("archive missing manifest.json".into()))?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)
            .map_err(|e| AppError::BadRequest(format!("read manifest: {e}")))?;
        serde_json::from_str(&buf)
            .map_err(|e| AppError::BadRequest(format!("manifest parse: {e}")))?
    };
    let archive_mm = semver_major_minor(&manifest.version);
    let running_mm = semver_major_minor(crate::version::VERSION);
    if archive_mm.is_none() || running_mm.is_none() || archive_mm != running_mm {
        return Err(AppError::BadRequest(format!(
            "archive was built on lets-chat {} (major.minor incompatible with this server {}); \
             upgrade or downgrade the server to the same major.minor before restoring",
            manifest.version,
            crate::version::VERSION
        )));
    }
    for entry in &manifest.files {
        let mut file = archive
            .by_name(&entry.path)
            .map_err(|_| AppError::BadRequest(format!("archive missing {}", entry.path)))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut total: u64 = 0;
        loop {
            let n = file
                .read(&mut buf)
                .map_err(|e| AppError::BadRequest(format!("read {}: {e}", entry.path)))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            total += n as u64;
        }
        if total != entry.size {
            return Err(AppError::BadRequest(format!(
                "size mismatch for {}: archive has {}, manifest says {}",
                entry.path, total, entry.size
            )));
        }
        let got: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if got != entry.sha256 {
            return Err(AppError::BadRequest(format!(
                "sha256 mismatch for {}; archive may be corrupt or tampered",
                entry.path
            )));
        }
    }
    Ok(manifest)
}

/// Extract the validated archive into `staged_dir`, replacing any
/// previous staged contents. `manifest.json` is dropped from the
/// extracted tree since it has no role inside the live data dir.
pub fn stage_extract(zip_path: &Path, staged_dir: &Path) -> Result<(), AppError> {
    if staged_dir.exists() {
        std::fs::remove_dir_all(staged_dir)
            .map_err(|e| AppError::Internal(format!("clean stage: {e}")))?;
    }
    std::fs::create_dir_all(staged_dir)
        .map_err(|e| AppError::Internal(format!("create stage: {e}")))?;
    let f = std::fs::File::open(zip_path)
        .map_err(|e| AppError::Internal(format!("open archive: {e}")))?;
    let mut archive =
        zip::ZipArchive::new(f).map_err(|e| AppError::BadRequest(format!("zip parse: {e}")))?;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| AppError::Internal(format!("zip read: {e}")))?;
        let name = file.name().to_string();
        // Defend against absolute paths and `..` traversal so a
        // malicious archive cannot write outside `staged_dir`.
        if name.starts_with('/')
            || name.starts_with('\\')
            || name.split(['/', '\\']).any(|seg| seg == "..")
        {
            return Err(AppError::BadRequest(format!(
                "archive contains an unsafe path: {name}"
            )));
        }
        let dest = staged_dir.join(&name);
        if name.ends_with('/') {
            std::fs::create_dir_all(&dest)
                .map_err(|e| AppError::Internal(format!("mkdir {}: {e}", dest.display())))?;
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| AppError::Internal(format!("mkdir {}: {e}", parent.display())))?;
            }
            let mut out = std::fs::File::create(&dest)
                .map_err(|e| AppError::Internal(format!("create {}: {e}", dest.display())))?;
            std::io::copy(&mut file, &mut out)
                .map_err(|e| AppError::Internal(format!("write {}: {e}", dest.display())))?;
        }
    }
    let _ = std::fs::remove_file(staged_dir.join("manifest.json"));
    Ok(())
}
