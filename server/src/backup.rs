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

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AppError;

/// Marker file dropped inside the live data dir when a restore is
/// staged. Contains no payload (presence is the signal); written by
/// the restore route, removed implicitly when the dir it lives in
/// is renamed aside on the next startup.
pub const MARKER_FILENAME: &str = ".restore-pending";

/// Sibling directory suffix the restore route extracts into. So a
/// data dir at `/data` stages to `/data.staged-restore` and, on
/// startup, replaces `/data` (the old `/data` is renamed to
/// `/data.replaced-{ts}` for recovery).
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
    std::fs::rename(data_dir, &replaced)?;
    std::fs::rename(&staged, data_dir)?;
    Ok(())
}

/// Build a backup archive at `output`. Snapshots the three pools via
/// SQLite's `VACUUM INTO`, walks the on-disk trees, and writes one
/// zip with a `manifest.json` describing every entry.
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
    std::fs::create_dir_all(parent)
        .map_err(|e| AppError::Internal(format!("create archive parent: {e}")))?;
    let work_dir = parent.join(format!(".lc-backup-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&work_dir)
        .map_err(|e| AppError::Internal(format!("create work dir: {e}")))?;

    // VACUUM INTO does not accept bound parameters; interpolate as a
    // SQLite string literal. Path is fully server-controlled (uuid +
    // fixed filename) so the escape is paranoia, not a security gate.
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
        sqlx::query(&sql)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("VACUUM INTO {name}: {e}")))?;
        if !dst.exists() {
            // VACUUM INTO can silently no-op against an `sqlite::memory:`
            // pool that has more than one connection (each connection
            // is its own DB), and our integration tests rely on the
            // file actually existing. Surface it as an explicit error
            // rather than failing later with a confusing read-file
            // diagnostic.
            return Err(AppError::Internal(format!(
                "VACUUM INTO {name} produced no file at {}",
                dst.display(),
            )));
        }
        snapshot_paths.push((name.to_string(), dst));
    }

    let mut entries: Vec<ManifestEntry> = Vec::new();
    let zf = std::fs::File::create(output)
        .map_err(|e| AppError::Internal(format!("create archive: {e}")))?;
    let mut zw = zip::ZipWriter::new(zf);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for (name, snap) in &snapshot_paths {
        write_file_into_zip(&mut zw, name, snap, opts, &mut entries)?;
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

fn write_file_into_zip(
    zw: &mut zip::ZipWriter<std::fs::File>,
    name: &str,
    src: &Path,
    opts: zip::write::SimpleFileOptions,
    entries: &mut Vec<ManifestEntry>,
) -> Result<(), AppError> {
    let bytes = std::fs::read(src)
        .map_err(|e| AppError::Internal(format!("read {}: {e}", src.display())))?;
    let size = bytes.len() as u64;
    let sha = sha256_hex(&bytes);
    zw.start_file(name, opts)
        .map_err(|e| AppError::Internal(format!("zip start {name}: {e}")))?;
    zw.write_all(&bytes)
        .map_err(|e| AppError::Internal(format!("zip write {name}: {e}")))?;
    entries.push(ManifestEntry {
        path: name.to_string(),
        size,
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
            write_file_into_zip(zw, &arc_name, &path, opts, entries)?;
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn sqlite_string_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Parse the manifest and re-hash every entry against it. Refuses
/// archives built on a different lets-chat version (exact-match
/// today; a semver-aware loosening is a follow-up). Returns the
/// validated manifest on success.
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
    if manifest.version != crate::version::VERSION {
        return Err(AppError::BadRequest(format!(
            "archive was built on lets-chat {}, this server is {}; \
             match the server version before restoring",
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
