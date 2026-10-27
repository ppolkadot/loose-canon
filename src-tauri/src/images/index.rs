//! Finding pairs of near-identical perceptual hashes without comparing every pair.
//!
//! Multi-index hashing: split the 64 bits into `max_distance + 1` chunks. Two hashes that differ
//! in at most `max_distance` bits must be identical on at least one chunk (pigeonhole), so only
//! hashes sharing a chunk value need to be compared. Each pair is reported once, in the first
//! chunk where the two agree.
//!
//! A hash shared by a huge number of images (a "no photo" placeholder, a logo, a watermark card)
//! says nothing about identity and would make buckets quadratic; buckets above
//! [`MAX_BUCKET_SIZE`] are skipped and counted instead.

use std::collections::HashMap;

use super::phash::hamming_distance;

/// Largest distance the index supports. Candidate generation uses 5; beyond this the chunks
/// become so narrow that the index stops helping.
pub const MAX_SUPPORTED_DISTANCE: u32 = 16;

/// Buckets with more images than this are not expanded into pairs.
pub const MAX_BUCKET_SIZE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NearPair {
    /// The smaller of the two ids.
    pub a: i64,
    pub b: i64,
    pub distance: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NearPairs {
    /// Sorted by `(a, b)`.
    pub pairs: Vec<NearPair>,
    /// Buckets skipped for being over-common. Non-zero means some pairs may be missing on
    /// purpose; the hashes involved are not usable as evidence.
    pub oversized_buckets: usize,
}

/// All pairs `(id, hash)` entries whose hashes differ in at most `max_distance` bits. Entries
/// with equal ids are never paired with each other. `max_distance` is clamped to
/// [`MAX_SUPPORTED_DISTANCE`].
#[must_use]
pub fn near_pairs(entries: &[(i64, u64)], max_distance: u32) -> NearPairs {
    let max_distance = max_distance.min(MAX_SUPPORTED_DISTANCE);
    let chunk_count = max_distance + 1;
    // Contiguous bit ranges of near-equal width covering all 64 bits.
    let ranges: Vec<(u32, u64)> = (0..chunk_count)
        .map(|c| {
            let start = c * 64 / chunk_count;
            let end = (c + 1) * 64 / chunk_count;
            let width = end - start;
            let mask = if width >= 64 {
                u64::MAX
            } else {
                (1_u64 << width) - 1
            };
            (start, mask)
        })
        .collect();
    let chunks = ranges.len();
    let key = |hash: u64, chunk: usize| (hash >> ranges[chunk].0) & ranges[chunk].1;

    let mut result = NearPairs::default();
    for chunk in 0..chunks {
        let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
        for (index, &(_, hash)) in entries.iter().enumerate() {
            buckets.entry(key(hash, chunk)).or_default().push(index);
        }
        for members in buckets.values().filter(|m| m.len() >= 2) {
            if members.len() > MAX_BUCKET_SIZE {
                result.oversized_buckets += 1;
                continue;
            }
            for (position, &i) in members.iter().enumerate() {
                for &j in &members[position + 1..] {
                    let ((id_i, hash_i), (id_j, hash_j)) = (entries[i], entries[j]);
                    // Report the pair only in the first chunk where the two agree.
                    if id_i == id_j
                        || (0..chunk).any(|earlier| key(hash_i, earlier) == key(hash_j, earlier))
                    {
                        continue;
                    }
                    let distance = hamming_distance(hash_i, hash_j);
                    if distance <= max_distance {
                        result.pairs.push(NearPair {
                            a: id_i.min(id_j),
                            b: id_i.max(id_j),
                            distance,
                        });
                    }
                }
            }
        }
    }
    result.pairs.sort_by_key(|p| (p.a, p.b, p.distance));
    result
}

#[cfg(test)]
mod tests;
