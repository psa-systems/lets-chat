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
//!
//! ## LC-206 extension: 10 MiB ceiling
//!
//! The avatar fetch caps at 1 MiB, but the UPLOAD path (which runs the same
//! decoder) allows 10 MiB. Size-dependent decoder bugs (dimension /
//! allocation overflow that only triggers at large inputs) are exactly what
//! a 1 MiB-only spike misses. LC-206 re-ran the corpus scaled to 10 MiB and
//! it stayed clean (0 panics); those rows are promoted below so the
//! validation is permanent, not a one-off pre-execution run.

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
    assert_eq!(
        try_decode(&[]),
        Ok(false),
        "empty input should be Err, not panic"
    );
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
    assert_eq!(
        try_decode(&buf),
        Ok(false),
        "random bytes should be Err, not panic"
    );
}

#[test]
fn png_signature_alone_does_not_panic() {
    // Just the 8-byte PNG signature, no IHDR. Decoder should reject cleanly.
    let bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    assert_eq!(
        try_decode(&bytes),
        Ok(false),
        "PNG sig only should be Err, not panic"
    );
}

#[test]
fn png_signature_plus_garbage_does_not_panic() {
    let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend(std::iter::repeat_n(0xAAu8, 1024));
    assert_eq!(
        try_decode(&bytes),
        Ok(false),
        "PNG sig + garbage should be Err, not panic"
    );
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
    bytes.extend(std::iter::repeat_n(0xFFu8, 4096));
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
    assert_eq!(
        try_decode(&bytes),
        Ok(false),
        "pixel-bomb must Err, not panic"
    );
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

// ---------------------------------------------------------------------------
// LC-206: the same corpus scaled to the upload path's 10 MiB ceiling. All
// rows stayed clean (Err, never panic) when first run; these guard against a
// future image-crate bump regressing at the larger input size.
// ---------------------------------------------------------------------------

const MIB: usize = 1 << 20;
const TEN_MIB: usize = 10 * MIB;

/// Fixed-seed pseudo-random bytes (same LCG the 1 MiB row uses, factored).
fn lcg_fill(n: usize, seed: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(n);
    let mut x = seed;
    for _ in 0..n {
        x = x.wrapping_mul(1664525).wrapping_add(1013904223);
        buf.push((x >> 16) as u8);
    }
    buf
}

#[test]
fn ten_mib_random_bytes_does_not_panic() {
    assert!(try_decode(&lcg_fill(TEN_MIB, 0xC0FFEE)).is_ok());
}

#[test]
fn ten_mib_png_sig_plus_garbage_does_not_panic() {
    let mut b = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    b.extend(lcg_fill(TEN_MIB, 0x1111));
    assert!(try_decode(&b).is_ok());
}

#[test]
fn ten_mib_jpeg_soi_plus_garbage_does_not_panic() {
    let mut b = vec![0xFF, 0xD8];
    b.extend(lcg_fill(TEN_MIB, 0x2222));
    assert!(try_decode(&b).is_ok());
}

#[test]
fn ten_mib_jpeg_soi_plus_ff_run_does_not_panic() {
    // Long run of marker-prefix bytes: a JPEG scanner stress case at scale.
    let mut b = vec![0xFF, 0xD8];
    b.extend(std::iter::repeat_n(0xFFu8, TEN_MIB));
    assert!(try_decode(&b).is_ok());
}

#[test]
fn ten_mib_gif_header_plus_garbage_does_not_panic() {
    let mut b = b"GIF89a".to_vec();
    b.extend(lcg_fill(TEN_MIB, 0x3333));
    assert!(try_decode(&b).is_ok());
}

#[test]
fn ten_mib_webp_lying_riff_size_does_not_panic() {
    // RIFF with a declared size far larger than the actual buffer; WebP
    // decoders have historically panicked on lying chunk sizes.
    let mut b = Vec::with_capacity(TEN_MIB + 12);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    b.extend_from_slice(b"WEBP");
    b.extend(lcg_fill(TEN_MIB, 0x4444));
    assert!(try_decode(&b).is_ok());
}

#[test]
fn jpeg_sof0_huge_dims_truncated_does_not_panic() {
    // SOF0 claims 65500x65500 then the stream is truncated: forces the
    // decoder to read declared dimensions before the data runs out.
    let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    b.extend_from_slice(b"JFIF\x00");
    b.extend_from_slice(&[0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
    b.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0xFF, 0xDC, 0xFF, 0xDC, 0x03]);
    b.extend_from_slice(&[0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01]);
    assert!(try_decode(&b).is_ok());
}

#[test]
fn large_valid_png_decodes_at_scale_without_panic() {
    // CAVEAT: this row tests ALLOCATION-at-scale, not compression ratio.
    // The pixel data is LCG noise, so the encoded PNG is large (~36 MB) and
    // decodes to a 3000*3000*4 = 36 MB buffer - it exercises the real
    // large-allocation decode path. It is NOT a decompression bomb (small
    // compressed -> huge decoded); that high-ratio vector is covered by
    // `pixel_bomb_dimension_overflow_does_not_panic`, which image 0.25
    // rejects at header-read before allocating. Do not read this passing
    // row as "decompression bombs are fine" - read it as "a genuinely large
    // valid decode does not panic." Both properties matter; they are
    // different rows.
    use image::ImageEncoder;
    let (w, h) = (3000u32, 3000u32);
    let pixels = lcg_fill((w * h * 4) as usize, 0x5555);
    let img = image::RgbaImage::from_raw(w, h, pixels).expect("raw buffer");
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(&img, w, h, image::ExtendedColorType::Rgba8)
        .expect("encode large png");
    assert_eq!(
        try_decode(&buf),
        Ok(true),
        "a large valid PNG must DECODE, not Err or panic"
    );
}
