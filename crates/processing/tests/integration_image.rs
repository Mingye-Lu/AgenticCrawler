use std::path::PathBuf;

use base64::Engine;
use image::{ImageBuffer, Rgb};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn ensure_sample_png(size: u32) -> PathBuf {
    let path = fixtures_dir().join(format!("sample_{size}x{size}.png"));
    if !path.exists() {
        let img: ImageBuffer<Rgb<u8>, _> = ImageBuffer::from_fn(size, size, |x, _y| {
            #[allow(clippy::cast_possible_truncation)]
            Rgb([((x * 255) / size) as u8, 128u8, 64u8])
        });
        img.save(&path).unwrap();
    }
    path
}

#[test]
fn view_small_image_no_resize() {
    use acrawl_processing::image_proc::view_image;

    let path = ensure_sample_png(50);
    let result = view_image(&path, None).unwrap();

    assert_eq!(result.original_dimensions, (50, 50));
    assert_eq!(result.resized_dimensions, (50, 50));
    assert!(!result.image_base64.is_empty());
    assert_eq!(result.media_type, "image/jpeg");
}

#[test]
fn view_large_image_gets_resized() {
    use acrawl_processing::image_proc::view_image;

    let path = ensure_sample_png(2000);
    let result = view_image(&path, None).unwrap();

    assert_eq!(result.original_dimensions, (2000, 2000));
    assert!(result.resized_dimensions.0 <= 1568);
    assert!(result.resized_dimensions.1 <= 1568);
    assert!(!result.image_base64.is_empty());

    base64::engine::general_purpose::STANDARD
        .decode(&result.image_base64)
        .expect("output should be valid base64");
}

#[test]
fn view_image_custom_max_dimension() {
    use acrawl_processing::image_proc::view_image;

    let path = ensure_sample_png(800);
    let result = view_image(&path, Some(400)).unwrap();

    assert_eq!(result.original_dimensions, (800, 800));
    assert!(result.resized_dimensions.0 <= 400);
    assert!(result.resized_dimensions.1 <= 400);
}

#[test]
fn view_image_nonexistent_returns_error() {
    use acrawl_processing::error::ProcessingError;
    use acrawl_processing::image_proc::view_image;

    let result = view_image(std::path::Path::new("/nonexistent/image.png"), None);
    assert!(matches!(result, Err(ProcessingError::IoError(_))));
}
