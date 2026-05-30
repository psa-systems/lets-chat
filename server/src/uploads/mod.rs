pub mod pipeline;
pub mod sweep;

/// Process-wide cap on concurrent image-pipeline tasks. Decode + re-encode
/// is memory-hungry; without a cap a burst of large uploads could OOM the
/// server. Tune here if operator-side load tells a different story.
pub const THUMBNAIL_CONCURRENCY: usize = 4;

pub fn thumbnail_semaphore() -> &'static tokio::sync::Semaphore {
    use std::sync::OnceLock;
    static SEM: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    SEM.get_or_init(|| tokio::sync::Semaphore::new(THUMBNAIL_CONCURRENCY))
}

/// Derive the preview filename from a content-addressed storage name. The
/// pipeline write side and the serve-route read side share this convention,
/// so the on-disk file the upload writes is the file the serve route looks
/// for. `"abc.jpg"` becomes `"abc_preview.jpg"`.
pub fn preview_storage_name(storage_name: &str) -> String {
    match storage_name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}_preview.{ext}"),
        None => format!("{storage_name}_preview"),
    }
}

/// Map a storage-name extension to its canonical MIME. Serve-time uses this
/// to derive the Content-Type for `?size=preview` independently of the DB
/// row's `mime_type`; today they match for every supported format, but a
/// future format where preview-mime ≠ original-mime (e.g. HEIC → JPEG
/// transcode) would otherwise quietly serve the wrong Content-Type.
pub fn mime_for_storage_path(storage_name: &str) -> Option<&'static str> {
    let (_, ext) = storage_name.rsplit_once('.')?;
    match ext {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Write `bytes` to `final_path` via a `.partial` sibling + rename, so a crash
/// mid-write never leaves a half-written content-addressed file in place. On
/// any error, the partial file is cleaned up best-effort. Used by both the
/// upload handler and the admin regenerate-thumbnails action.
pub async fn write_atomic(final_path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    // LC-208: the staging name MUST be unique per write, not a fixed
    // `{final}.partial`. Content-addressed storage means two concurrent
    // uploads of identical bytes derive the same final path, and the dedup
    // check in `post_upload` is TOCTOU (both see "final absent", both write).
    // With a shared staging path both writers race on one `{final}.partial`:
    // whichever renames second hits ENOENT after the first consumes it, and
    // the upload 500s. A per-write nonce gives each writer its own staging
    // file; both renames target the same (identical-content) final path,
    // which is safe (rename is atomic, last writer wins with the same bytes).
    let mut partial_os = final_path.as_os_str().to_owned();
    partial_os.push(format!(".{}.partial", uuid::Uuid::new_v4()));
    let partial: std::path::PathBuf = partial_os.into();
    if let Err(e) = tokio::fs::write(&partial, bytes).await {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e);
    }
    if let Err(e) = tokio::fs::rename(&partial, final_path).await {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e);
    }
    Ok(())
}

/// Sha256 an in-memory byte slice. Used for image uploads where the canonical
/// filename derives from the STRIPPED re-encoded bytes, not the temp file.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Sha256 a file by streaming through it 64 KiB at a time. Returns the hex
/// digest. Used to derive the canonical content-addressed filename.
pub async fn sha256_file(path: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Ok(hex)
}
