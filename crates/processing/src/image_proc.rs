use std::io::Cursor;
use std::path::Path;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use image::{imageops::FilterType, DynamicImage, ImageFormat};
use serde::{Deserialize, Serialize};

use crate::error::ProcessingError;

/// Maximum file size allowed for image processing (100 MB).
const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Default maximum dimension (longest edge) for resized output.
const DEFAULT_MAX_DIMENSION: u32 = 1568;

/// JPEG encoding quality (0-100).
const JPEG_QUALITY: u8 = 85;

/// Result of processing an image file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageOutput {
    /// Output format name (e.g. "jpeg", "png").
    pub format: String,
    /// Original image dimensions (width, height).
    pub original_dimensions: (u32, u32),
    /// Dimensions after resizing (width, height). Equal to original if no resize needed.
    pub resized_dimensions: (u32, u32),
    /// Size of the encoded output in bytes.
    pub size_bytes: u64,
    /// Base64-encoded image data.
    pub image_base64: String,
    /// MIME type of the output (e.g. "image/jpeg", "image/png").
    pub media_type: String,
}

/// Load, optionally resize, and base64-encode an image file.
///
/// Returns an error for SVG files (use `read_document` instead) and files
/// exceeding 100 MB.
///
/// If the longest edge exceeds `max_dimension` (default 1568), the image is
/// scaled down proportionally using Lanczos3 resampling. Images with an alpha
/// channel are output as PNG; opaque images are output as JPEG at 85% quality.
pub fn view_image(path: &Path, max_dimension: Option<u32>) -> Result<ImageOutput, ProcessingError> {
    if path.extension().and_then(|e| e.to_str()) == Some("svg") {
        return Err(ProcessingError::UnsupportedFormat(
            "SVG is text/XML — use read_document to view as text".to_string(),
        ));
    }

    let metadata = std::fs::metadata(path)?;
    let file_size = metadata.len();
    if file_size > MAX_FILE_SIZE {
        return Err(ProcessingError::FileTooLarge {
            actual_bytes: file_size,
            limit_bytes: MAX_FILE_SIZE,
        });
    }

    let img = image::open(path).map_err(|e| ProcessingError::FormatError(e.to_string()))?;

    let original_dimensions = (img.width(), img.height());

    let max_dim = max_dimension.unwrap_or(DEFAULT_MAX_DIMENSION);
    let (w, h) = original_dimensions;
    let longest = w.max(h);
    let (new_w, new_h) = if longest > max_dim {
        let ratio = f64::from(max_dim) / f64::from(longest);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let dims = ((f64::from(w) * ratio) as u32, (f64::from(h) * ratio) as u32);
        dims
    } else {
        (w, h)
    };

    let resized = if (new_w, new_h) == (w, h) {
        img
    } else {
        img.resize(new_w, new_h, FilterType::Lanczos3)
    };

    let resized_dimensions = (resized.width(), resized.height());

    let has_alpha = matches!(
        resized.color(),
        image::ColorType::Rgba8
            | image::ColorType::La8
            | image::ColorType::Rgba16
            | image::ColorType::La16
            | image::ColorType::Rgba32F
    );

    let (encoded_bytes, format_name, media_type) = if has_alpha {
        let mut buf = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        resized
            .write_to(&mut cursor, ImageFormat::Png)
            .map_err(|e| ProcessingError::FormatError(e.to_string()))?;
        (buf, "png".to_string(), "image/png".to_string())
    } else {
        let mut buf = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        let rgb = DynamicImage::ImageRgb8(resized.to_rgb8());
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, JPEG_QUALITY);
        rgb.write_with_encoder(encoder)
            .map_err(|e| ProcessingError::FormatError(e.to_string()))?;
        (buf, "jpeg".to_string(), "image/jpeg".to_string())
    };

    let size_bytes = encoded_bytes.len() as u64;
    let image_base64 = BASE64.encode(&encoded_bytes);

    Ok(ImageOutput {
        format: format_name,
        original_dimensions,
        resized_dimensions,
        size_bytes,
        image_base64,
        media_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb, Rgba};
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_test_image(width: u32, height: u32) -> NamedTempFile {
        let img = ImageBuffer::<Rgb<u8>, _>::from_fn(width, height, |x, _y| {
            #[allow(clippy::cast_possible_truncation)]
            Rgb([255u8, (x % 256) as u8, 0u8])
        });
        let mut f = NamedTempFile::with_suffix(".png").unwrap();
        image::DynamicImage::ImageRgb8(img)
            .write_to(
                &mut std::io::BufWriter::new(&mut f),
                image::ImageFormat::Png,
            )
            .unwrap();
        f
    }

    fn create_rgba_test_image(width: u32, height: u32) -> NamedTempFile {
        let img = ImageBuffer::<Rgba<u8>, _>::from_fn(width, height, |x, y| {
            #[allow(clippy::cast_possible_truncation)]
            Rgba([
                (x % 256) as u8,
                (y % 256) as u8,
                128u8,
                if x % 2 == 0 { 128u8 } else { 255u8 },
            ])
        });
        let mut f = NamedTempFile::with_suffix(".png").unwrap();
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::BufWriter::new(&mut f),
                image::ImageFormat::Png,
            )
            .unwrap();
        f
    }

    #[test]
    fn resize_large_image() {
        let tmp = create_test_image(3000, 2000);
        let out = view_image(tmp.path(), None).unwrap();

        assert_eq!(out.original_dimensions, (3000, 2000));
        assert!(out.resized_dimensions.0 <= 1568);
        assert!(out.resized_dimensions.1 <= 1568);
        assert_ne!(out.original_dimensions, out.resized_dimensions);
        assert!(!out.image_base64.is_empty());
        assert_eq!(out.media_type, "image/jpeg");
        assert_eq!(out.format, "jpeg");
        assert!(out.size_bytes > 0);
    }

    #[test]
    fn small_image_no_resize() {
        let tmp = create_test_image(50, 50);
        let out = view_image(tmp.path(), None).unwrap();

        assert_eq!(out.original_dimensions, (50, 50));
        assert_eq!(out.original_dimensions, out.resized_dimensions);
        assert!(!out.image_base64.is_empty());
        assert_eq!(out.media_type, "image/jpeg");
    }

    #[test]
    fn svg_returns_unsupported_error() {
        let mut f = NamedTempFile::with_suffix(".svg").unwrap();
        write!(f, "<svg></svg>").unwrap();
        let result = view_image(f.path(), None);
        assert!(matches!(result, Err(ProcessingError::UnsupportedFormat(_))));
    }

    #[test]
    fn rgba_image_outputs_png() {
        let tmp = create_rgba_test_image(100, 100);
        let out = view_image(tmp.path(), None).unwrap();

        assert_eq!(out.media_type, "image/png");
        assert_eq!(out.format, "png");
        assert!(!out.image_base64.is_empty());
    }

    #[test]
    fn custom_max_dimension() {
        let tmp = create_test_image(2000, 1000);
        let out = view_image(tmp.path(), Some(500)).unwrap();

        assert!(out.resized_dimensions.0 <= 500);
        assert!(out.resized_dimensions.1 <= 500);
    }
}
