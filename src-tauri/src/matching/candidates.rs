//! Proposing pairs worth comparing. A candidate is **not** a match: it only says "look at these
//! two". Two sources of candidates, both bounded so a large library never costs a full pairwise
//! scan:
//!
//! * near-identical image hashes (multi-index lookup, ignoring hashes shared by many listings);
//! * listings in the same municipality and neighbourhood (blocks above a size limit are skipped
//!   and counted, never expanded).
//!
//! Listings already in the same merge group are never proposed.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::compare::{MatchConfig, MatchInput};
use crate::images::index::near_pairs;

/// A pair to compare, as indices into the input slice. `a` always has the smaller listing id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub a: usize,
    pub b: usize,
    pub from_images: bool,
    pub from_metadata: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Candidates {
    /// Sorted by `(listing id of a, listing id of b)`.
    pub pairs: Vec<Candidate>,
    /// Image hashes ignored because many listings share them.
    pub common_hashes: HashSet<u64>,
    /// Metadata blocks skipped for being larger than `max_block_size`.
    pub oversized_blocks: usize,
    /// Image-hash buckets the index skipped for being over-common.
    pub oversized_image_buckets: usize,
}

/// Hashes that appear on at least `config.common_hash_listings` different listings.
#[must_use]
pub fn common_hashes(inputs: &[MatchInput], config: &MatchConfig) -> HashSet<u64> {
    let mut listings_per_hash: HashMap<u64, HashSet<i64>> = HashMap::new();
    for input in inputs {
        for image in &input.images {
            listings_per_hash
                .entry(image.hash)
                .or_default()
                .insert(input.listing_id);
        }
    }
    listings_per_hash
        .into_iter()
        .filter(|(_, listings)| listings.len() >= config.common_hash_listings)
        .map(|(hash, _)| hash)
        .collect()
}

#[must_use]
pub fn generate(inputs: &[MatchInput], config: &MatchConfig) -> Candidates {
    let common = common_hashes(inputs, config);
    let mut found: BTreeMap<(i64, i64), Candidate> = BTreeMap::new();
    let mut add = |i: usize, j: usize, from_images: bool| {
        let (x, y) = (&inputs[i], &inputs[j]);
        if x.listing_id == y.listing_id || x.group_root == y.group_root {
            return;
        }
        let (a, b) = if x.listing_id < y.listing_id {
            (i, j)
        } else {
            (j, i)
        };
        let key = (inputs[a].listing_id, inputs[b].listing_id);
        let entry = found.entry(key).or_insert(Candidate {
            a,
            b,
            from_images: false,
            from_metadata: false,
        });
        entry.from_images |= from_images;
        entry.from_metadata |= !from_images;
    };

    // Image candidates.
    let mut owner: HashMap<i64, usize> = HashMap::new();
    let mut entries: Vec<(i64, u64)> = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        for image in input.images.iter().filter(|i| !common.contains(&i.hash)) {
            owner.insert(image.image_id, index);
            entries.push((image.image_id, image.hash));
        }
    }
    let near = near_pairs(&entries, config.image_distance);
    for pair in &near.pairs {
        if let (Some(&i), Some(&j)) = (owner.get(&pair.a), owner.get(&pair.b)) {
            add(i, j, true);
        }
    }

    // Metadata candidates: same municipality and neighbourhood, both known.
    let mut blocks: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for (index, input) in inputs.iter().enumerate() {
        if let (Some(municipality), Some(neighborhood)) = (&input.municipality, &input.neighborhood)
        {
            blocks
                .entry((municipality, neighborhood))
                .or_default()
                .push(index);
        }
    }
    let mut oversized_blocks = 0;
    for members in blocks.values() {
        if members.len() > config.max_block_size {
            oversized_blocks += 1;
            continue;
        }
        for (position, &i) in members.iter().enumerate() {
            for &j in &members[position + 1..] {
                add(i, j, false);
            }
        }
    }

    Candidates {
        pairs: found.into_values().collect(),
        common_hashes: common,
        oversized_blocks,
        oversized_image_buckets: near.oversized_buckets,
    }
}

#[cfg(test)]
mod tests;
