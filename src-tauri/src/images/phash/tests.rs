use std::io::Cursor;

use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage, imageops};

use super::*;

/// A deterministic, photo-like test scene: smooth gradients plus a few coloured blocks, all
/// derived from `seed`. Different seeds give structurally different scenes.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "fixture arithmetic on small values bounded by construction: pixel coordinates, percentages and channel values clamped to 0..=255"
)]
pub(crate) fn scene(seed: u32, w: u32, h: u32) -> RgbImage {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(12_345);
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let (gx, gy) = ((next() % 200) as f32 / 100.0, (next() % 200) as f32 / 100.0);
    let base = [
        (next() % 200) as f32,
        (next() % 200) as f32,
        (next() % 200) as f32,
    ];
    let blocks: Vec<(u32, u32, u32, u32, [u8; 3])> = (0..7)
        .map(|_| {
            let bw = w / 6 + next() % (w / 3);
            let bh = h / 6 + next() % (h / 3);
            (
                next() % (w - bw),
                next() % (h - bh),
                bw,
                bh,
                [
                    (next() % 256) as u8,
                    (next() % 256) as u8,
                    (next() % 256) as u8,
                ],
            )
        })
        .collect();
    RgbImage::from_fn(w, h, |x, y| {
        let fx = x as f32 / w as f32;
        let fy = y as f32 / h as f32;
        let mut px = [
            (base[0] + 55.0 * (gx * fx + (1.0 - gx) * fy)).clamp(0.0, 255.0) as u8,
            (base[1] + 55.0 * (gy * fy + (1.0 - gy) * fx)).clamp(0.0, 255.0) as u8,
            (base[2] + 55.0 * (fx * fy)).clamp(0.0, 255.0) as u8,
        ];
        for &(bx, by, bw, bh, colour) in &blocks {
            if x >= bx && x < bx + bw && y >= by && y < by + bh {
                px = colour;
            }
        }
        Rgb(px)
    })
}

fn hash_of(img: &RgbImage) -> u64 {
    perceptual_hash(&DynamicImage::ImageRgb8(img.clone()))
}

fn jpeg_roundtrip(img: &RgbImage, quality: u8) -> RgbImage {
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode_image(&DynamicImage::ImageRgb8(img.clone()))
        .unwrap();
    image::load_from_memory_with_format(&bytes, ImageFormat::Jpeg)
        .unwrap()
        .to_rgb8()
}

#[test]
fn the_same_pixels_always_give_the_same_hash() {
    let a = scene(3, 160, 120);
    assert_eq!(hash_of(&a), hash_of(&a.clone()));
    assert_eq!(hamming_distance(hash_of(&a), hash_of(&a)), 0);
}

#[test]
fn the_hash_is_pinned_by_a_golden_value_so_a_behaviour_change_cannot_pass_unnoticed() {
    // If this fails the algorithm changed: bump `ALGORITHM` and re-pin deliberately.
    assert_eq!(ALGORITHM, "dct64-v1");
    let h = hash_of(&scene(7, 128, 96));
    assert_eq!(h, GOLDEN_SCENE_7, "dct64-v1 output changed: {h:#018x}");
    assert!(
        (20..=44).contains(&h.count_ones()),
        "a healthy hash is roughly balanced"
    );
}

const GOLDEN_SCENE_7: u64 = 0xee4c_b15b_0f64_c4b1;

#[test]
fn near_duplicates_stay_within_the_candidate_threshold() {
    let original = scene(11, 320, 240);
    let h = hash_of(&original);

    let half = imageops::resize(&original, 160, 120, imageops::FilterType::Triangle);
    assert!(hamming_distance(h, hash_of(&half)) <= 5, "downscaled copy");

    let upscaled = imageops::resize(&original, 640, 480, imageops::FilterType::Triangle);
    assert!(
        hamming_distance(h, hash_of(&upscaled)) <= 5,
        "upscaled copy"
    );

    assert!(
        hamming_distance(h, hash_of(&jpeg_roundtrip(&original, 30))) <= 6,
        "heavy JPEG compression"
    );

    let mut brighter = original.clone();
    for p in brighter.pixels_mut() {
        for c in &mut p.0 {
            *c = c.saturating_add(20);
        }
    }
    assert!(
        hamming_distance(h, hash_of(&brighter)) <= 5,
        "brightness shift"
    );

    let mut watermarked = original.clone();
    for y in 215..235 {
        for x in 250..310 {
            watermarked.put_pixel(x, y, Rgb([255, 255, 255]));
        }
    }
    assert!(
        hamming_distance(h, hash_of(&watermarked)) <= 8,
        "small corner watermark"
    );
}

#[test]
fn different_scenes_are_far_apart_and_a_mirror_image_is_not_the_same() {
    let hashes: Vec<u64> = (1..=8).map(|s| hash_of(&scene(s, 160, 120))).collect();
    for i in 0..hashes.len() {
        for j in (i + 1)..hashes.len() {
            assert!(
                hamming_distance(hashes[i], hashes[j]) >= 10,
                "scenes {} and {} are only {} bits apart",
                i + 1,
                j + 1,
                hamming_distance(hashes[i], hashes[j])
            );
        }
    }
    let original = scene(5, 160, 120);
    let mirrored = imageops::flip_horizontal(&original);
    assert!(hamming_distance(hash_of(&original), hash_of(&mirrored)) >= 8);
}

#[test]
fn transparency_is_composited_onto_white() {
    let white = RgbImage::from_pixel(64, 48, Rgb([255, 255, 255]));
    let transparent = RgbaImage::from_pixel(64, 48, Rgba([10, 200, 30, 0]));
    assert_eq!(
        perceptual_hash(&DynamicImage::ImageRgba8(transparent)),
        hash_of(&white),
        "a fully transparent image is white, whatever colour is stored under the alpha"
    );
}

#[test]
fn flat_images_and_tiny_images_do_not_panic() {
    let flat = RgbImage::from_pixel(100, 100, Rgb([128, 128, 128]));
    assert_eq!(hash_of(&flat), hash_of(&flat.clone()));
    let tiny = RgbImage::from_pixel(1, 1, Rgb([9, 9, 9]));
    let _ = hash_of(&tiny);
    let thin = RgbImage::from_pixel(500, 2, Rgb([200, 10, 10]));
    let _ = hash_of(&thin);
    let _ = hash_of(&RgbImage::from_pixel(2, 500, Rgb([1, 2, 3])));
}

#[test]
fn the_hash_does_not_depend_on_how_the_image_was_encoded() {
    let original = scene(21, 200, 150);
    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(original.clone())
        .write_to(&mut png, ImageFormat::Png)
        .unwrap();
    let decoded = image::load_from_memory(png.get_ref()).unwrap();
    assert_eq!(
        perceptual_hash(&decoded),
        hash_of(&original),
        "lossless round trip"
    );
}

#[test]
fn hamming_distance_and_database_encoding() {
    assert_eq!(hamming_distance(0, 0), 0);
    assert_eq!(hamming_distance(0, u64::MAX), 64);
    assert_eq!(hamming_distance(0b1011, 0b0001), 2);
    assert_eq!(
        hamming_distance(0xF0F0, 0x0F0F),
        hamming_distance(0x0F0F, 0xF0F0)
    );
    for h in [
        0_u64,
        1,
        u64::MAX,
        0x8000_0000_0000_0000,
        0xF0F0_F0F0_0F0F_0F0F,
        0x7FFF_FFFF_FFFF_FFFF,
    ] {
        assert_eq!(from_db(to_db(h)), h);
    }
    assert!(
        to_db(0x8000_0000_0000_0000) < 0,
        "top bit set is negative in SQLite, bit pattern preserved"
    );
}
