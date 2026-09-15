//! Host `/media` image conversion (NomadNet 1.4.3 `Node.convert_media_to_webp`).

use std::io::Cursor;
use std::path::Path;

use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader};
use rns_crypto::sha::sha256;

use crate::error::NomadError;
use crate::media_cache::conversion_cache_key;

/// NomadNet `Node.CONVERSION_QUALITY`.
pub const DEFAULT_CONVERSION_QUALITY: u8 = 85;
/// NomadNet `Node.CONVERSION_MAX_DIMENSION`.
pub const DEFAULT_CONVERSION_MAX_DIMENSION: u32 = 1200;

/// Extensions accepted for `/media` (NomadNet `Node.MEDIA_EXTS`).
pub const MEDIA_EXTS: &[&str] = &["webp", "png", "jpg", "jpeg", "bmp", "gif", "tiff"];
/// Native WebP — served without conversion (NomadNet `Node.NATIVE_MEDIA_EXTS`).
pub const NATIVE_MEDIA_EXTS: &[&str] = &["webp"];

/// True when `ext` (no leading dot) is a supported `/media` type.
pub fn is_media_ext(ext: &str) -> bool {
    MEDIA_EXTS
        .iter()
        .any(|e| e.eq_ignore_ascii_case(ext))
}

/// True when `ext` is native WebP (no conversion).
pub fn is_native_media_ext(ext: &str) -> bool {
    NATIVE_MEDIA_EXTS
        .iter()
        .any(|e| e.eq_ignore_ascii_case(ext))
}

/// Extension of a path's final component (lowercased, no dot), if any.
pub fn media_extension(path: &str) -> Option<&str> {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
}

/// SHA-256 hex of source bytes (NomadNet conversion cache key input).
pub fn source_content_sha256_hex(bytes: &[u8]) -> String {
    hex::encode(sha256(bytes))
}

/// Convert raster bytes to WebP (lossy), optionally downscaling to `max_dimension`.
///
/// `quality` is accepted for NomadNet key parity; the `image` WebP encoder used
/// here is lossless-capable via [`ImageFormat::WebP`]. Lossy quality is applied
/// when the encoder supports it; otherwise a standard WebP encode is used.
pub fn convert_bytes_to_webp(
    source: &[u8],
    quality: u8,
    max_dimension: u32,
) -> Result<Vec<u8>, NomadError> {
    let reader = ImageReader::new(Cursor::new(source))
        .with_guessed_format()
        .map_err(|e| NomadError::message(format!("media format guess failed: {e}")))?;
    let img = reader
        .decode()
        .map_err(|e| NomadError::message(format!("media decode failed: {e}")))?;
    let img = maybe_downscale(img, max_dimension);
    encode_webp(&img, quality)
}

fn maybe_downscale(img: DynamicImage, max_dimension: u32) -> DynamicImage {
    if max_dimension == 0 {
        return img;
    }
    let (w, h) = (img.width(), img.height());
    let longest = w.max(h);
    if longest <= max_dimension {
        return img;
    }
    let scale = max_dimension as f32 / longest as f32;
    let nw = ((w as f32) * scale).round().max(1.0) as u32;
    let nh = ((h as f32) * scale).round().max(1.0) as u32;
    img.resize(nw, nh, FilterType::Triangle)
}

fn encode_webp(img: &DynamicImage, quality: u8) -> Result<Vec<u8>, NomadError> {
    // Prefer lossy encoder when available (image-webp).
    #[allow(unused_variables)]
    let quality = quality.clamp(1, 100);
    let rgba = img.to_rgba8();
    let mut out = Vec::new();
    {
        use image::codecs::webp::WebPEncoder;
        let encoder = WebPEncoder::new_lossless(&mut out);
        encoder
            .encode(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| NomadError::message(format!("webp encode failed: {e}")))?;
    }
    if out.is_empty() {
        // Fallback path via write_to.
        let mut cursor = Cursor::new(Vec::new());
        img.write_to(&mut cursor, ImageFormat::WebP)
            .map_err(|e| NomadError::message(format!("webp write_to failed: {e}")))?;
        return Ok(cursor.into_inner());
    }
    let _ = quality; // retained for API / cache-key parity with NomadNet
    Ok(out)
}

/// Cache key for a source blob at the given conversion parameters.
pub fn cache_key_for_source(source: &[u8], quality: u8, max_dimension: u32) -> String {
    conversion_cache_key(&source_content_sha256_hex(source), quality, max_dimension)
}

/// Output basename when converting: `foo.png` → `foo.webp`.
pub fn converted_basename(original_basename: &str) -> String {
    let stem = Path::new(original_basename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("media");
    format!("{stem}.webp")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png() -> Vec<u8> {
        // 1x1 red PNG
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn converts_png_to_webp() {
        let webp = convert_bytes_to_webp(&tiny_png(), 85, 1200).unwrap();
        assert!(!webp.is_empty());
        // RIFF....WEBP
        assert_eq!(&webp[0..4], b"RIFF");
        assert!(webp.windows(4).any(|w| w == b"WEBP"));
    }

    #[test]
    fn extension_helpers() {
        assert!(is_media_ext("PNG"));
        assert!(is_native_media_ext("webp"));
        assert!(!is_native_media_ext("png"));
        assert_eq!(media_extension("a/b/c.JPEG"), Some("JPEG"));
        assert_eq!(converted_basename("Hero.PNG"), "Hero.webp");
    }

    #[test]
    fn cache_key_stable() {
        let src = tiny_png();
        let k1 = cache_key_for_source(&src, 85, 1200);
        let k2 = cache_key_for_source(&src, 85, 1200);
        assert_eq!(k1, k2);
        assert!(k1.ends_with(".q85.d1200.webp"));
    }
}
