use std::io::Cursor;

use image::{DynamicImage, ImageFormat, RgbImage};

use super::*;
use crate::images::phash::{perceptual_hash, tests::scene};

fn encode(img: &RgbImage, format: ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut out, format)
        .expect("encode");
    out.into_inner()
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A PNG whose header claims `width` x `height` but which carries no pixel data: what a
/// decompression bomb looks like before it is decoded.
pub(crate) fn png_claiming(width: u32, height: u32) -> Vec<u8> {
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        png.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        png.extend_from_slice(&body);
        png.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    // A zlib stream holding nothing: enough for a decoder to get past the header, which is how
    // a real bomb gets in before its dimensions are looked at.
    chunk(b"IDAT", &[0x78, 0x9C, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
    chunk(b"IEND", &[]);
    png
}

#[test]
fn decodes_and_hashes_every_supported_format() {
    let original = scene(9, 160, 120);
    let expected = perceptual_hash(&DynamicImage::ImageRgb8(original.clone()));
    for (format, stored) in [
        (ImageFormat::Png, StoredFormat::Png),
        (ImageFormat::Jpeg, StoredFormat::Jpeg),
        (ImageFormat::WebP, StoredFormat::WebP),
    ] {
        let bytes = encode(&original, format);
        let hashed = hash_image(&bytes, &ImageLimits::default())
            .unwrap_or_else(|e| panic!("{format:?}: {e}"));
        assert_eq!(
            (hashed.width, hashed.height, hashed.format),
            (160, 120, stored),
            "{format:?}"
        );
        let distance = (hashed.phash ^ expected).count_ones();
        // Lossless formats give the identical hash; JPEG only a very close one.
        assert!(
            distance <= if format == ImageFormat::Jpeg { 4 } else { 0 },
            "{format:?}: {distance} bits"
        );
    }
}

#[test]
fn the_format_comes_from_the_content_not_from_a_name_or_a_header() {
    // Valid PNG bytes are a PNG whatever anything else claims.
    let png = encode(&scene(1, 64, 48), ImageFormat::Png);
    assert_eq!(
        hash_image(&png, &ImageLimits::default()).unwrap().format,
        StoredFormat::Png
    );
}

#[test]
fn empty_and_oversized_payloads_are_refused_before_decoding() {
    assert_eq!(
        hash_image(&[], &ImageLimits::default()),
        Err(ImageError::Empty)
    );
    let png = encode(&scene(1, 64, 48), ImageFormat::Png);
    let limits = ImageLimits {
        max_bytes: png.len() - 1,
        ..ImageLimits::default()
    };
    assert_eq!(
        hash_image(&png, &limits),
        Err(ImageError::TooLarge {
            limit: png.len() - 1
        })
    );
    let exact = ImageLimits {
        max_bytes: png.len(),
        ..ImageLimits::default()
    };
    assert!(hash_image(&png, &exact).is_ok());
}

#[test]
fn text_html_and_unsupported_formats_are_unsupported_not_corrupt() {
    for bytes in [
        b"<html><body>Not an image</body></html>".to_vec(),
        b"GIF89a\x01\x00\x01\x00\x00\x00\x00;".to_vec(),
        b"%PDF-1.7 fake".to_vec(),
        vec![0_u8; 64],
        (0..=255).collect::<Vec<u8>>(),
    ] {
        assert_eq!(
            hash_image(&bytes, &ImageLimits::default()),
            Err(ImageError::UnsupportedFormat),
            "{:?}",
            &bytes[..bytes.len().min(12)]
        );
    }
}

#[test]
fn truncated_or_damaged_images_are_corrupt() {
    let png = encode(&scene(2, 128, 96), ImageFormat::Png);
    let truncated = &png[..png.len() / 2];
    assert!(matches!(
        hash_image(truncated, &ImageLimits::default()),
        Err(ImageError::Corrupt(_))
    ));

    let jpeg = encode(&scene(2, 128, 96), ImageFormat::Jpeg);
    let mut damaged = jpeg.clone();
    damaged.truncate(jpeg.len() / 3);
    assert!(matches!(
        hash_image(&damaged, &ImageLimits::default()),
        Err(ImageError::Corrupt(_))
    ));
}

#[test]
fn a_complete_jpeg_with_a_short_trailer_is_still_accepted() {
    let mut jpeg = encode(&scene(2, 128, 96), ImageFormat::Jpeg);
    assert!(hash_image(&jpeg, &ImageLimits::default()).is_ok());
    jpeg.extend_from_slice(&[0_u8; 200]); // padding some servers append
    assert!(hash_image(&jpeg, &ImageLimits::default()).is_ok());
}

#[test]
fn a_decompression_bomb_is_rejected_from_its_header_before_anything_is_allocated() {
    let bomb = png_claiming(50_000, 50_000);
    assert!(bomb.len() < 100);
    assert_eq!(
        hash_image(&bomb, &ImageLimits::default()),
        Err(ImageError::TooManyPixels {
            width: 50_000,
            height: 50_000
        })
    );
    // Within the per-side limit but over the total pixel budget.
    let wide = png_claiming(9_000, 9_000);
    assert!(matches!(
        hash_image(&wide, &ImageLimits::default()),
        Err(ImageError::TooManyPixels { .. })
    ));
}

#[test]
fn custom_dimension_limits_are_enforced() {
    let png = encode(&scene(3, 200, 100), ImageFormat::Png);
    let narrow = ImageLimits {
        max_width: 100,
        ..ImageLimits::default()
    };
    assert_eq!(
        hash_image(&png, &narrow),
        Err(ImageError::TooManyPixels {
            width: 200,
            height: 100
        })
    );
    let short = ImageLimits {
        max_height: 50,
        ..ImageLimits::default()
    };
    assert!(matches!(
        hash_image(&png, &short),
        Err(ImageError::TooManyPixels { .. })
    ));
    let few = ImageLimits {
        max_pixels: 10_000,
        ..ImageLimits::default()
    };
    assert!(matches!(
        hash_image(&png, &few),
        Err(ImageError::TooManyPixels { .. })
    ));
}

#[test]
fn error_kinds_are_stable_distinct_names() {
    let kinds = [
        ImageError::Empty.kind(),
        ImageError::TooLarge { limit: 1 }.kind(),
        ImageError::UnsupportedFormat.kind(),
        ImageError::TooManyPixels {
            width: 1,
            height: 1,
        }
        .kind(),
        ImageError::Corrupt(String::new()).kind(),
    ];
    let unique: std::collections::HashSet<_> = kinds.iter().collect();
    assert_eq!(unique.len(), kinds.len());
}

#[test]
fn extensions_are_the_safe_cache_suffixes() {
    assert_eq!(StoredFormat::Jpeg.extension(), "jpg");
    assert_eq!(StoredFormat::Png.extension(), "png");
    assert_eq!(StoredFormat::WebP.extension(), "webp");
}
