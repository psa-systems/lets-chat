//! LC-78-AVATAR-PROXY pre-execution spike (sharpening #4).
//!
//! The avatar-proxy fetch routes foreign-controlled image bytes (from
//! arbitrary Matrix homeservers) through the same `image` crate the upload
//! pipeline uses. User uploads are semi-trusted (they go through a UI
//! gesture from a registered account); foreign avatars are not. That makes
//! the decoder the trust boundary, same posture as `pulldown-latex` and
//! `mail-parser` were for their respective foreign-input ingress paths.
//!
//! This spike confirms two properties against a hostile corpus:
//! 1. **No panics.** Decoder rejects hostile bytes via `Result::Err`, never
//!    via `panic!` / unreachable / arithmetic overflow / OOM-from-overcommit.
//! 2. **Bounded byte intake.** Inputs at 1MiB or below complete without
//!    exhausting memory. (Production fetch caps at 1MiB; this spike confirms
//!    that cap is sufficient for the decoder's own resource use.)
//!
//! Failure here is a blocker: if any input panics, the fetch module needs
//! `catch_unwind` defense in depth on the production path, or a smaller cap,
//! or a sandboxing layer. The spike is the gate, not the fix.

use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Try to decode `bytes` through the same path the production pipeline
/// uses (sniff format, then decode). Wraps in `catch_unwind` so a decoder
/// panic returns `Err(())` rather than aborting the test. Returns
/// `Ok(true)` if decode succeeded, `Ok(false)` if it returned `Err`,
/// `Err(())` if it panicked.
fn try_decode(bytes: &[u8]) -> Result<bool, ()> {
    catch_unwind(AssertUnwindSafe(|| {
        match image::ImageReader::new(Cursor::new(bytes)).with_guessed_format() {
            Ok(r) => r.decode().is_ok(),
            Err(_) => false,
        }
    }))
    .map_err(|_| ())
}

#[test]
fn empty_bytes_does_not_panic() {
    assert_eq!(try_decode(&[]), Ok(false), "empty input should be Err, not panic");
}

#[test]
fn random_bytes_does_not_panic() {
    // Fixed-seed pseudo-random bytes claiming no particular format.
    let mut buf = Vec::with_capacity(4096);
    let mut x: u32 = 0xDEADBEEF;
    for _ in 0..4096 {
        x = x.wrapping_mul(1664525).wrapping_add(1013904223);
        buf.push((x >> 16) as u8);
    }
    assert_eq!(try_decode(&buf), Ok(false), "random bytes should be Err, not panic");
}

#[test]
fn png_signature_alone_does_not_panic() {
    // Just the 8-byte PNG signature, no IHDR. Decoder should reject cleanly.
    let bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    assert_eq!(try_decode(&bytes), Ok(false), "PNG sig only should be Err, not panic");
}

#[test]
fn png_signature_plus_garbage_does_not_panic() {
    let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend(std::iter::repeat(0xAAu8).take(1024));
    assert_eq!(try_decode(&bytes), Ok(false), "PNG sig + garbage should be Err, not panic");
}

#[test]
fn jpeg_soi_alone_does_not_panic() {
    // Start-of-image marker with no segments after.
    let bytes = [0xFF, 0xD8];
    assert_eq!(try_decode(&bytes), Ok(false));
}

#[test]
fn jpeg_soi_plus_garbage_does_not_panic() {
    let mut bytes = vec![0xFF, 0xD8];
    bytes.extend(std::iter::repeat(0xFFu8).take(4096));
    assert_eq!(try_decode(&bytes), Ok(false));
}

#[test]
fn gif_header_alone_does_not_panic() {
    let bytes = b"GIF89a".to_vec();
    assert_eq!(try_decode(&bytes), Ok(false));
}

#[test]
fn webp_header_alone_does_not_panic() {
    // RIFF + size + WEBP magic with no chunks.
    let bytes = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
    assert_eq!(try_decode(&bytes), Ok(false));
}

#[test]
fn truncated_after_dimensions_does_not_panic() {
    // Hand-crafted PNG: signature + IHDR claiming 1x1, but no IDAT/IEND.
    // image-rs should error on missing required chunks, not panic.
    let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    // IHDR: 13 bytes, type "IHDR", width=1, height=1, bit_depth=8, color=2 (RGB),
    // compression=0, filter=0, interlace=0, CRC32.
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]); // length 13
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]); // width
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]); // height
    bytes.extend_from_slice(&[0x08, 0x02, 0x00, 0x00, 0x00]);
    bytes.extend_from_slice(&[0x90, 0x77, 0x53, 0xDE]); // CRC (may be wrong; decoder should handle)
    // No IDAT, no IEND.
    assert_eq!(try_decode(&bytes), Ok(false));
}

#[test]
fn pixel_bomb_dimension_overflow_does_not_panic() {
    // Hand-crafted PNG IHDR claiming 65535x65535 (~4 GB raw RGBA). image-rs
    // 0.25 has no default Limits, so this CAN return Err on memory failure
    // but MUST NOT panic. If the decoder OOMs into a panic (e.g., via
    // `try_reserve` returning Err but a downstream `vec![0; n]` failing),
    // the production fetch needs additional defenses (Limits::max_image_size
    // or pre-decode dimension sniff + reject).
    let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]); // width 65535
    bytes.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF]); // height 65535
    bytes.extend_from_slice(&[0x08, 0x02, 0x00, 0x00, 0x00]); // 8-bit RGB
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // CRC placeholder
    // Minimal IDAT to make image-rs attempt decode.
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x02]); // length 2
    bytes.extend_from_slice(b"IDAT");
    bytes.extend_from_slice(&[0x78, 0x01]); // zlib header, no data
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // CRC placeholder
    // IEND.
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    bytes.extend_from_slice(b"IEND");
    bytes.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]);
    assert_eq!(try_decode(&bytes), Ok(false), "pixel-bomb must Err, not panic");
}

#[test]
fn one_mebibyte_random_bytes_does_not_panic() {
    // 1 MiB of pseudo-random bytes labeled as nothing in particular. This is
    // the upper bound on what the production fetch will hand to the decoder
    // (the 1MiB byte cap). Confirms the decoder handles the cap-sized input
    // without panicking on any of the format-sniff branches.
    let mut buf = Vec::with_capacity(1 << 20);
    let mut x: u32 = 0xC0FFEE;
    for _ in 0..(1 << 20) {
        x = x.wrapping_mul(1664525).wrapping_add(1013904223);
        buf.push((x >> 16) as u8);
    }
    assert_eq!(try_decode(&buf), Ok(false));
}

#[test]
fn known_good_png_round_trips() {
    // Sanity: the spike framework correctly returns Ok(true) for a valid
    // image, not just Ok(false) for everything.
    use image::ImageEncoder;
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(&img, 2, 2, image::ExtendedColorType::Rgba8)
        .unwrap();
    assert_eq!(try_decode(&buf), Ok(true), "a valid PNG must decode");
}
