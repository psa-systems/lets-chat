//! LC-206 killer test pair: the property the safe helpers establish is
//! "a panic in the blocking decode does NOT propagate out of the helper -
//! it surfaces as `SafeImageError::Panic`, distinct from a normal decode
//! failure." The 10 MiB spike was clean (no input panics image 0.25), so
//! the panic case uses a SYNTHETIC panic through the same `run_blocking`
//! boundary - this tests the helper's catch shape regardless of source, and
//! stays valid across a future image-crate bump that might regress.
//!
//! Both assertions match the SPECIFIC error variant (Panic vs
//! Pipeline(Decode)), never bare `is_err()` - same discipline as the
//! `views::math` catch_unwind tests: a test that accepts "any error" would
//! pass even if the boundary collapsed the two failure modes together, which
//! is exactly the distinction bridge_avatar's decode-vs-panic logging relies
//! on.

use lets_chat::uploads::pipeline::{self, PipelineError, SafeImageError};

#[tokio::test]
async fn synthetic_panic_maps_to_panic_variant() {
    // Drive the panic-boundary seam directly with a closure that panics.
    let result: Result<(), SafeImageError> =
        pipeline::run_blocking(|| -> Result<(), PipelineError> {
            panic!("synthetic decoder panic");
        })
        .await;

    assert!(
        matches!(result, Err(SafeImageError::Panic(_))),
        "a panicking blocking closure must map to SafeImageError::Panic, got {result:?}"
    );
    // Reaching here proves the async caller survived: the panic was caught
    // at the JoinError boundary, not unwound through this tokio task.
}

#[tokio::test]
async fn malformed_image_maps_to_pipeline_decode_not_panic() {
    // PNG signature with no IHDR/IDAT: a clean decode failure, not a panic.
    // It must surface as Pipeline(Decode), keeping the two failure modes
    // distinguishable.
    let bytes: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    let path = std::env::temp_dir().join(format!(
        "lc206-malformed-{}-{}.png",
        std::process::id(),
        "boundary"
    ));
    std::fs::write(&path, bytes).expect("write malformed png");

    let result = pipeline::process_image_safely(path.clone(), "image/png".to_string()).await;
    let _ = std::fs::remove_file(&path);

    // ProcessedImage has no Debug (it would dump MB of bytes), so tag the Ok
    // case rather than `{result:?}` it.
    let tag = match &result {
        Ok(_) => "Ok(ProcessedImage)".to_string(),
        Err(e) => format!("Err({e:?})"),
    };
    assert!(
        matches!(
            result,
            Err(SafeImageError::Pipeline(PipelineError::Decode(_)))
        ),
        "a malformed image must map to Pipeline(Decode), NOT Panic, got {tag}"
    );
}
