//! Image-processing pipeline. Decodes once, re-encodes both a stripped
//! original and a 360px-max-dimension preview. Re-encoding through the
//! `image` crate IS the metadata strip: decode discards EXIF / XMP / IPTC /
//! PNG text chunks, and the encoder writes none of them back.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{
    AnimationDecoder, DynamicImage, ExtendedColorType, Frame, ImageEncoder, ImageError, ImageReader,
};

const PREVIEW_MAX_DIM: u32 = 360;
/// q=92 is below the visual-threshold for typical chat content; the small
/// perceptual loss buys guaranteed-stripped output for every JPEG.
const JPEG_QUALITY: u8 = 92;

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("decode failed: {0}")]
    Decode(ImageError),
    #[error("encode failed: {0}")]
    Encode(ImageError),
    #[error("unsupported mime: {0}")]
    UnsupportedMime(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub struct ProcessedImage {
    pub original_bytes: Vec<u8>,
    pub preview_bytes: Vec<u8>,
    pub mime: String,
}

#[derive(Clone, Copy, Debug)]
enum Format {
    Jpeg,
    Png,
    Gif,
    Webp,
}

impl Format {
    fn from_mime(m: &str) -> Option<Self> {
        match m {
            "image/jpeg" => Some(Self::Jpeg),
            "image/png" => Some(Self::Png),
            "image/gif" => Some(Self::Gif),
            "image/webp" => Some(Self::Webp),
            _ => None,
        }
    }

    fn mime(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

/// LC-206: error from the async safe-decode helpers. `Pipeline` carries a
/// normal `process_image` / `preview_from_path` failure (decode / encode /
/// unsupported / io); `Panic` carries a `spawn_blocking` `JoinError` (a
/// decoder panic under `panic=unwind`; a task cancellation would also land
/// here, matching the pre-LC-206 bridge-avatar code which bucketed every
/// `JoinError` under "decoder panic"). Keeping the two variants distinct
/// lets a caller log decode-vs-panic separately (bridge_avatar) or fold
/// them (uploads / attachments) as it sees fit.
#[derive(Debug, thiserror::Error)]
pub enum SafeImageError {
    #[error("{0}")]
    Pipeline(#[from] PipelineError),
    #[error("image decoder panicked: {0}")]
    Panic(String),
}

/// Shared panic boundary for the safe-decode helpers. Runs a sync pipeline
/// fn on the blocking pool so the CPU-bound decode never parks a tokio
/// runtime thread, and folds a panicking closure (tokio surfaces it as a
/// `JoinError` under `panic=unwind`) into `SafeImageError::Panic` instead of
/// letting it unwind. `#[doc(hidden)] pub` only so the killer test
/// (`image_decode_safe_boundary`) can drive it with a synthetic panic; it is
/// NOT a routine entry point. The LC-206 grep-ban forbids calling it from
/// `src/` outside this file, exactly as LC-152 bans `outbound_unchecked` -
/// a `src/` caller would replicate the spawn_blocking + JoinError boilerplate
/// inline and defeat the dedup these helpers exist to provide.
#[doc(hidden)]
pub async fn run_blocking<T, F>(f: F) -> Result<T, SafeImageError>
where
    F: FnOnce() -> Result<T, PipelineError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r.map_err(SafeImageError::Pipeline),
        Err(join) => Err(SafeImageError::Panic(format!("{join}"))),
    }
}

/// Safe async entry point for the full decode + re-encode pipeline. Wraps
/// the whole `process_image` (not decode-only: re-encode is also CPU work
/// that belongs off the async runtime). Every `src/` caller routes through
/// here; raw `process_image` stays `pub` as the sync unit-test seam only.
pub async fn process_image_safely(
    path: PathBuf,
    mime: String,
) -> Result<ProcessedImage, SafeImageError> {
    run_blocking(move || process_image(&path, &mime)).await
}

/// Safe async entry point for the preview-only path (admin thumbnail
/// regen). Same panic boundary as `process_image_safely`, different return.
pub async fn process_preview_safely(
    path: PathBuf,
    mime: String,
) -> Result<Vec<u8>, SafeImageError> {
    run_blocking(move || preview_from_path(&path, &mime)).await
}

pub fn process_image(tmp_path: &Path, sniffed_mime: &str) -> Result<ProcessedImage, PipelineError> {
    let format = Format::from_mime(sniffed_mime)
        .ok_or_else(|| PipelineError::UnsupportedMime(sniffed_mime.to_string()))?;
    match format {
        Format::Gif => process_gif(tmp_path),
        _ => process_still(tmp_path, format),
    }
}

fn process_still(tmp_path: &Path, format: Format) -> Result<ProcessedImage, PipelineError> {
    let decoded = ImageReader::open(tmp_path)?
        .with_guessed_format()?
        .decode()
        .map_err(PipelineError::Decode)?;

    let preview = thumbnail(&decoded);
    let original_bytes = encode(&decoded, format)?;
    let preview_bytes = encode(&preview, format)?;

    Ok(ProcessedImage {
        original_bytes,
        preview_bytes,
        mime: format.mime().to_string(),
    })
}

/// Original keeps its animation (re-encoded without metadata). Preview is the
/// first frame as a static GIF: animated previews in the message list are a
/// distraction trap. Loop count is forced to `Repeat::Infinite` during
/// re-encode; image 0.25's `into_frames` does not surface the source loop
/// count, and the privacy gain from re-encoding outweighs that one-bit
/// deviation for the rare finite-loop GIF.
fn process_gif(tmp_path: &Path) -> Result<ProcessedImage, PipelineError> {
    let file = File::open(tmp_path)?;
    let decoder = GifDecoder::new(BufReader::new(file)).map_err(PipelineError::Decode)?;
    let frames: Vec<Frame> = decoder
        .into_frames()
        .collect::<Result<Vec<_>, _>>()
        .map_err(PipelineError::Decode)?;
    if frames.is_empty() {
        return Err(PipelineError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "GIF decoded to zero frames",
        )));
    }

    let first_frame_buffer = frames[0].buffer().clone();

    let mut original_bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut original_bytes);
        encoder
            .set_repeat(Repeat::Infinite)
            .map_err(PipelineError::Encode)?;
        encoder
            .encode_frames(frames)
            .map_err(PipelineError::Encode)?;
    }

    let preview_source = DynamicImage::ImageRgba8(first_frame_buffer);
    let preview = thumbnail(&preview_source);
    let preview_bytes = encode(&preview, Format::Gif)?;

    Ok(ProcessedImage {
        original_bytes,
        preview_bytes,
        mime: Format::Gif.mime().to_string(),
    })
}

/// Read an image from disk and produce just the 360px preview bytes. Used by
/// the admin "regenerate thumbnails" action to backfill missing `_preview`
/// files without re-encoding the (already-stripped) original. For animated
/// GIFs this returns a single-frame static preview, matching the upload-time
/// behaviour for new uploads.
pub fn preview_from_path(path: &std::path::Path, mime: &str) -> Result<Vec<u8>, PipelineError> {
    let format =
        Format::from_mime(mime).ok_or_else(|| PipelineError::UnsupportedMime(mime.to_string()))?;
    let decoded = ImageReader::open(path)?
        .with_guessed_format()?
        .decode()
        .map_err(PipelineError::Decode)?;
    let preview = thumbnail(&decoded);
    encode(&preview, format)
}

fn thumbnail(img: &DynamicImage) -> DynamicImage {
    if img.width().max(img.height()) <= PREVIEW_MAX_DIM {
        img.clone()
    } else {
        img.thumbnail(PREVIEW_MAX_DIM, PREVIEW_MAX_DIM)
    }
}

fn encode(img: &DynamicImage, format: Format) -> Result<Vec<u8>, PipelineError> {
    let mut buf = Vec::new();
    match format {
        Format::Jpeg => {
            let rgb = img.to_rgb8();
            JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY)
                .write_image(&rgb, rgb.width(), rgb.height(), ExtendedColorType::Rgb8)
                .map_err(PipelineError::Encode)?;
        }
        Format::Png => {
            let rgba = img.to_rgba8();
            PngEncoder::new(&mut buf)
                .write_image(&rgba, rgba.width(), rgba.height(), ExtendedColorType::Rgba8)
                .map_err(PipelineError::Encode)?;
        }
        Format::Webp => {
            let rgba = img.to_rgba8();
            WebPEncoder::new_lossless(&mut buf)
                .write_image(&rgba, rgba.width(), rgba.height(), ExtendedColorType::Rgba8)
                .map_err(PipelineError::Encode)?;
        }
        Format::Gif => {
            let mut enc = GifEncoder::new(&mut buf);
            let frame = Frame::new(img.to_rgba8());
            enc.encode_frame(frame).map_err(PipelineError::Encode)?;
        }
    }
    Ok(buf)
}
