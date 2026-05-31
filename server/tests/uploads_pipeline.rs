use flate2::write::ZlibEncoder;
use flate2::Compression;
use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{AnimationDecoder, ExtendedColorType, ImageEncoder};
use lets_chat::uploads::pipeline::{preview_from_path, process_image, PipelineError};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

fn unique_path(stem: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "lets-chat-pipeline-{}-{}-{}",
        std::process::id(),
        n,
        stem,
    ))
}

fn write_temp(stem: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = unique_path(stem);
    std::fs::write(&p, bytes).expect("write temp");
    p
}

/// A minimal JPEG with an APP1 EXIF segment that holds one ImageDescription
/// tag. Built by encoding a clean 4x4 JPEG via the `image` crate and then
/// injecting the APP1 segment immediately after the SOI marker.
fn jpeg_with_exif() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 100, 50]));
    let mut clean = Vec::new();
    JpegEncoder::new(&mut clean)
        .write_image(&img, 4, 4, ExtendedColorType::Rgb8)
        .unwrap();

    // TIFF (little-endian) + IFD0 with one tag: ImageDescription = "FOO\0"
    let tiff: [u8; 26] = [
        b'I', b'I', 0x2A, 0x00, // header + magic
        0x08, 0x00, 0x00, 0x00, // IFD0 offset = 8
        0x01, 0x00, // 1 entry
        0x0E, 0x01, // tag = 0x010E (ImageDescription)
        0x02, 0x00, // type = ASCII
        0x04, 0x00, 0x00, 0x00, // count = 4
        b'F', b'O', b'O', 0x00, // inline value "FOO\0"
        0x00, 0x00, 0x00, 0x00, // next IFD = 0
    ];
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&tiff);
    let seg_len = (payload.len() + 2) as u16;
    let mut app1 = vec![0xFF, 0xE1, (seg_len >> 8) as u8, (seg_len & 0xFF) as u8];
    app1.extend_from_slice(&payload);

    let mut tainted = Vec::with_capacity(clean.len() + app1.len());
    tainted.extend_from_slice(&clean[..2]);
    tainted.extend_from_slice(&app1);
    tainted.extend_from_slice(&clean[2..]);
    tainted
}

fn count_exif_fields(bytes: &[u8]) -> usize {
    let mut cursor = std::io::Cursor::new(bytes);
    match exif::Reader::new().read_from_container(&mut cursor) {
        Ok(exif) => exif.fields().count(),
        Err(_) => 0,
    }
}

#[test]
fn jpeg_re_encode_strips_exif() {
    let tainted = jpeg_with_exif();
    assert!(
        count_exif_fields(&tainted) > 0,
        "fixture has no EXIF; test is meaningless"
    );

    let p = write_temp("exif.jpg", &tainted);
    let processed = process_image(&p, "image/jpeg").expect("pipeline");
    assert_eq!(
        count_exif_fields(&processed.original_bytes),
        0,
        "EXIF survived the strip on the stored original",
    );
    assert_eq!(
        count_exif_fields(&processed.preview_bytes),
        0,
        "EXIF survived the strip on the preview",
    );

    let _ = std::fs::remove_file(&p);
}

#[test]
fn preview_caps_longer_side_at_360_preserving_aspect() {
    let img = image::RgbaImage::from_pixel(1200, 800, image::Rgba([50, 200, 100, 255]));
    let mut buf = Vec::new();
    PngEncoder::new(&mut buf)
        .write_image(&img, 1200, 800, ExtendedColorType::Rgba8)
        .unwrap();
    let p = write_temp("big.png", &buf);

    let processed = process_image(&p, "image/png").expect("pipeline");
    let preview = image::load_from_memory(&processed.preview_bytes).expect("decode preview");
    assert_eq!(preview.width(), 360);
    assert_eq!(preview.height(), 240); // 800 * (360 / 1200) = 240

    let _ = std::fs::remove_file(&p);
}

#[test]
fn animated_gif_preserves_frames_in_original_and_static_in_preview() {
    let mut input = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut input);
        enc.set_repeat(Repeat::Infinite).unwrap();
        for i in 0..3u8 {
            let mut img = image::RgbaImage::new(20, 20);
            for px in img.pixels_mut() {
                *px = image::Rgba([i * 80, 0, 0, 255]);
            }
            enc.encode_frame(image::Frame::new(img)).unwrap();
        }
    }

    let p = write_temp("anim.gif", &input);
    let processed = process_image(&p, "image/gif").expect("pipeline");

    let orig_frames: Vec<_> = GifDecoder::new(std::io::Cursor::new(&processed.original_bytes))
        .unwrap()
        .into_frames()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        orig_frames.len(),
        3,
        "original GIF should preserve all 3 frames",
    );

    let preview_frames: Vec<_> = GifDecoder::new(std::io::Cursor::new(&processed.preview_bytes))
        .unwrap()
        .into_frames()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        preview_frames.len(),
        1,
        "preview GIF should be a single static frame",
    );

    let _ = std::fs::remove_file(&p);
}

#[test]
fn corrupt_bytes_after_png_magic_return_decode_error() {
    // PNG signature followed by garbage. `infer` would call this a PNG; the
    // `image` decoder rejects it. Pipeline must surface PipelineError::Decode
    // (which Task 3 maps to a 400).
    let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend_from_slice(&[0u8; 32]);
    let p = write_temp("corrupt.png", &bytes);
    match process_image(&p, "image/png") {
        Err(PipelineError::Decode(_)) => {}
        Err(other) => panic!("expected Decode, got {other:?}"),
        Ok(_) => panic!("expected Decode error, got Ok"),
    }
    let _ = std::fs::remove_file(&p);
}

#[test]
fn non_image_mime_returns_unsupported() {
    let p = write_temp("doc.pdf", b"%PDF-1.4");
    match process_image(&p, "application/pdf") {
        Err(PipelineError::UnsupportedMime(m)) => assert_eq!(m, "application/pdf"),
        Err(other) => panic!("expected UnsupportedMime, got {other:?}"),
        Ok(_) => panic!("expected UnsupportedMime, got Ok"),
    }
    let _ = std::fs::remove_file(&p);
}

#[test]
fn preview_from_path_emits_only_preview_bytes() {
    let img = image::RgbaImage::from_pixel(500, 300, image::Rgba([10, 20, 30, 255]));
    let mut buf = Vec::new();
    PngEncoder::new(&mut buf)
        .write_image(&img, 500, 300, ExtendedColorType::Rgba8)
        .unwrap();
    let p = write_temp("regen.png", &buf);

    let preview = preview_from_path(&p, "image/png").expect("preview_from_path");
    let decoded = image::load_from_memory(&preview).expect("decode preview");
    assert_eq!(decoded.width().max(decoded.height()), 360);

    let _ = std::fs::remove_file(&p);
}

// ---------------------------------------------------------------------------
// LC-206-IMAGE-LIMITS (#284): a VALID PNG decompression bomb (tiny file, huge
// decoded dimensions) must be rejected by the decode `Limits`, not decoded
// into a multi-hundred-MB buffer. This is distinct from the malformed pixel-
// bomb in the spike corpus (which image rejects at header-read for bad CRCs /
// truncation): here the PNG is well-formed, so rejection PROVES the limit
// fires, not that the input is broken. The small control below decodes fine,
// pinning that the builder produces valid PNGs.
// ---------------------------------------------------------------------------

/// IEEE CRC-32 (reflected), the variant PNG chunks use.
fn png_crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn png_chunk(out: &mut Vec<u8>, ctype: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(ctype);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(ctype);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&png_crc32(&crc_input).to_be_bytes());
}

/// A well-formed 8-bit RGB PNG of `w`x`h` all-zero pixels. The scanlines are
/// streamed through a real zlib encoder (never materialised in full), so a
/// 12000x12000 image whose decoded buffer is ~432 MB is produced from a row
/// at a time and a few-KB output - exactly the decompression-bomb shape.
fn constant_rgb_png(w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit depth, color type 2 (RGB)
    png_chunk(&mut out, b"IHDR", &ihdr);

    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    let row = vec![0u8; 1 + (w as usize) * 3]; // filter byte + RGB pixels, all zero
    for _ in 0..h {
        enc.write_all(&row).unwrap();
    }
    let idat = enc.finish().unwrap();
    png_chunk(&mut out, b"IDAT", &idat);

    png_chunk(&mut out, b"IEND", &[]);
    out
}

#[test]
fn constant_png_builder_produces_a_decodable_image() {
    // Control: the SAME builder at tiny dimensions yields a valid PNG that the
    // pipeline decodes. So the bomb test below fails on the LIMIT, not on a
    // malformed fixture.
    let p = write_temp("ctrl.png", &constant_rgb_png(8, 8));
    let out = process_image(&p, "image/png");
    let _ = std::fs::remove_file(&p);
    // ProcessedImage has no Debug (it would dump raw bytes), so use is_ok()
    // with a static message rather than Debug-printing the Ok value.
    assert!(
        out.is_ok(),
        "small constant PNG must decode through the pipeline",
    );
}

#[test]
fn png_decompression_bomb_rejected_by_limits() {
    // 12000x12000 RGB -> ~432 MB decoded, over the 256 MiB max_alloc but under
    // the 16384 dimension cap, so it is the max_alloc path under test. Tiny on
    // disk (all-zero pixels compress to a few KB).
    let bomb = constant_rgb_png(12_000, 12_000);
    // The whole point: it is small on disk (~410 KB, well under the 10 MiB
    // upload byte cap) yet decodes to ~432 MB. The byte cap does NOT catch it;
    // the decode Limits must.
    assert!(
        bomb.len() < 10 * 1024 * 1024,
        "bomb must pass the 10 MiB byte cap to reach the decoder, was {} bytes",
        bomb.len()
    );
    let p = write_temp("bomb.png", &bomb);
    let out = process_image(&p, "image/png");
    let _ = std::fs::remove_file(&p);
    // Match directly (ProcessedImage has no Debug, so no expect_err).
    match out {
        Err(PipelineError::Decode(image::ImageError::Limits(_))) => {}
        Err(other) => panic!("bomb must be rejected by the decode Limits, got {other:?}"),
        Ok(_) => panic!("decompression bomb decoded instead of being rejected"),
    }
}
