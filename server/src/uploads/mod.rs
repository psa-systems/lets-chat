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
