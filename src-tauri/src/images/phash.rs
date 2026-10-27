//! Perceptual hash `dct64-v1`: 64 bits describing the coarse structure of an image.
//!
//! Pipeline (every stage is implemented here, none delegated to a library whose behaviour could
//! change between versions, so the same pixels always give the same hash):
//!
//! 1. composite onto white (transparency), then luma `Y = (299 R + 587 G + 114 B) / 1000`;
//! 2. area-average down to 32 x 32;
//! 3. 2-D DCT-II, keep the 8 x 8 lowest frequencies (row-major, MSB first);
//! 4. bit = coefficient > median of the 63 AC coefficients (the DC term is compared too).
//!
//! A small Hamming distance means "visually similar", **never** "same property": similar photos
//! of different houses exist and the same house can be photographed very differently. The hash
//! only proposes candidates. If the algorithm ever changes, bump [`ALGORITHM`]: hashes of
//! different algorithms must not be compared.

use std::f64::consts::PI;

use image::DynamicImage;

/// Stored next to every hash (`property_images.phash_algorithm`).
pub const ALGORITHM: &str = "dct64-v1";

const SIDE: usize = 32;
const LOW: usize = 8;

/// Luma plane (0..=255) of `image`, transparency composited onto white.
fn luma_plane(image: &DynamicImage) -> (Vec<f64>, usize, usize) {
    let rgba = image.to_rgba8();
    let (width, height) = (rgba.width() as usize, rgba.height() as usize);
    let plane = rgba
        .pixels()
        .map(|pixel| {
            let [r, g, b, a] = pixel.0.map(u32::from);
            let over_white = |c: u32| (c * a + 255 * (255 - a)) / 255;
            let luma = (299 * over_white(r) + 587 * over_white(g) + 114 * over_white(b)) / 1000;
            f64::from(luma)
        })
        .collect();
    (plane, width, height)
}

/// `n` as `f64`, for values that are small by construction (indices below `2 * SIDE`, area
/// weights up to `SIDE * SIDE`).
fn small(n: usize) -> f64 {
    f64::from(u16::try_from(n).expect("a small index or weight"))
}

/// Source pixels of an axis of `extent` pixels covered by destination cell `cell`, each with the
/// covered length. Positions are kept in exact integer units of 1/`SIDE` source pixel (the cell
/// spans `cell * extent .. (cell + 1) * extent`, pixel `p` spans `p * SIDE .. (p + 1) * SIDE`),
/// so the weights are exact and no platform's floating-point rounding can change them.
fn coverage(cell: usize, extent: usize) -> impl Iterator<Item = (usize, usize)> {
    let (start, end) = (cell * extent, (cell + 1) * extent);
    (start / SIDE..end.div_ceil(SIDE)).map(move |pixel| {
        let overlap = end.min((pixel + 1) * SIDE) - start.max(pixel * SIDE);
        (pixel, overlap)
    })
}

/// Area-average resize to `SIDE` x `SIDE`: each destination cell is the mean of the source
/// pixels it covers, weighted by the fraction of each pixel covered.
fn area_resize(plane: &[f64], width: usize, height: usize) -> Vec<f64> {
    let mut out = vec![0.0; SIDE * SIDE];
    for dy in 0..SIDE {
        for dx in 0..SIDE {
            let (mut sum, mut weight_sum) = (0.0, 0.0);
            for (sy, wy) in coverage(dy, height) {
                for (sx, wx) in coverage(dx, width) {
                    let weight = small(wy * wx);
                    sum += plane[sy * width + sx] * weight;
                    weight_sum += weight;
                }
            }
            out[dy * SIDE + dx] = if weight_sum > 0.0 {
                sum / weight_sum
            } else {
                0.0
            };
        }
    }
    out
}

/// The `LOW` x `LOW` lowest-frequency coefficients of the 2-D DCT-II of a `SIDE` x `SIDE`
/// block (unnormalised: only the ordering of the coefficients matters).
fn low_frequency_dct(block: &[f64]) -> [f64; LOW * LOW] {
    let mut cos = [[0.0_f64; SIDE]; LOW];
    for (u, row) in cos.iter_mut().enumerate() {
        for (x, value) in row.iter_mut().enumerate() {
            *value = (PI * small(2 * x + 1) * small(u) / small(2 * SIDE)).cos();
        }
    }
    // Transform the rows first (only the LOW frequencies we keep), then the columns.
    let mut rows = [[0.0_f64; LOW]; SIDE];
    for y in 0..SIDE {
        for v in 0..LOW {
            rows[y][v] = (0..SIDE).map(|x| block[y * SIDE + x] * cos[v][x]).sum();
        }
    }
    let mut out = [0.0_f64; LOW * LOW];
    for u in 0..LOW {
        for v in 0..LOW {
            out[u * LOW + v] = (0..SIDE).map(|y| rows[y][v] * cos[u][y]).sum();
        }
    }
    out
}

/// 64-bit perceptual hash of `image`.
#[must_use]
pub fn perceptual_hash(image: &DynamicImage) -> u64 {
    let (plane, width, height) = luma_plane(image);
    let coefficients = low_frequency_dct(&area_resize(&plane, width, height));
    // Median of the 63 AC coefficients (the DC term would dominate any average).
    let mut ac: Vec<f64> = coefficients[1..].to_vec();
    ac.sort_by(f64::total_cmp);
    let median = ac[ac.len() / 2];
    coefficients
        .iter()
        .enumerate()
        .fold(0_u64, |hash, (i, &c)| {
            if c > median {
                hash | (1 << (63 - i))
            } else {
                hash
            }
        })
}

/// Number of differing bits.
#[must_use]
pub const fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// SQLite stores integers as signed 64-bit; the bit pattern is kept unchanged.
#[must_use]
pub const fn to_db(hash: u64) -> i64 {
    i64::from_ne_bytes(hash.to_ne_bytes())
}

#[must_use]
pub const fn from_db(stored: i64) -> u64 {
    u64::from_ne_bytes(stored.to_ne_bytes())
}

#[cfg(test)]
pub(crate) mod tests;
