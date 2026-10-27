use super::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn brute_force(entries: &[(i64, u64)], max_distance: u32) -> Vec<NearPair> {
    let mut out = Vec::new();
    for i in 0..entries.len() {
        for j in (i + 1)..entries.len() {
            let ((ia, ha), (ib, hb)) = (entries[i], entries[j]);
            if ia == ib {
                continue;
            }
            let distance = hamming_distance(ha, hb);
            if distance <= max_distance {
                out.push(NearPair {
                    a: ia.min(ib),
                    b: ia.max(ib),
                    distance,
                });
            }
        }
    }
    out.sort_by_key(|p| (p.a, p.b));
    out
}

/// Random hashes plus, for each, a few near copies (1..=8 flipped bits).
fn corpus(seed: u64, base: usize) -> Vec<(i64, u64)> {
    let mut rng = Rng(seed);
    let mut entries = Vec::new();
    let mut id = 0;
    for _ in 0..base {
        let hash = rng.next();
        id += 1;
        entries.push((id, hash));
        for _ in 0..(rng.next() % 3) {
            let mut near = hash;
            for _ in 0..=(rng.next() % 8) {
                near ^= 1 << (rng.next() % 64);
            }
            id += 1;
            entries.push((id, near));
        }
    }
    entries
}

#[test]
fn matches_a_brute_force_comparison_for_every_supported_threshold() {
    let entries = corpus(0x9E37_79B9_7F4A_7C15, 400);
    assert!(entries.len() > 600);
    for max_distance in [0, 1, 2, 3, 5, 8, 12, 16] {
        let found = near_pairs(&entries, max_distance);
        assert_eq!(found.oversized_buckets, 0);
        assert_eq!(
            found.pairs,
            brute_force(&entries, max_distance),
            "max_distance = {max_distance}"
        );
    }
}

#[test]
fn each_pair_is_reported_once_with_ordered_ids_and_its_true_distance() {
    let entries = vec![(7, 0b1111), (3, 0b1110), (9, 0b1100), (5, u64::MAX)];
    let found = near_pairs(&entries, 2);
    assert_eq!(
        found.pairs,
        vec![
            NearPair {
                a: 3,
                b: 7,
                distance: 1
            },
            NearPair {
                a: 3,
                b: 9,
                distance: 1
            },
            NearPair {
                a: 7,
                b: 9,
                distance: 2
            },
        ]
    );
}

#[test]
fn identical_hashes_are_distance_zero_pairs() {
    let found = near_pairs(&[(1, 0xABCD), (2, 0xABCD), (3, 0xABCD)], 0);
    assert_eq!(found.pairs.len(), 3);
    assert!(found.pairs.iter().all(|p| p.distance == 0));
}

#[test]
fn entries_with_the_same_id_are_never_paired() {
    // Two images of the same listing.
    let found = near_pairs(&[(1, 0xFF), (1, 0xFE), (2, 0xFF)], 5);
    assert!(found.pairs.iter().all(|p| p.a != p.b));
    assert_eq!(found.pairs.len(), 2);
}

#[test]
fn empty_and_single_inputs_have_no_pairs() {
    assert_eq!(near_pairs(&[], 5), NearPairs::default());
    assert_eq!(near_pairs(&[(1, 5)], 5), NearPairs::default());
}

#[test]
fn an_over_common_hash_is_skipped_and_reported_instead_of_exploding() {
    // 600 listings sharing one placeholder hash, plus two genuinely near hashes.
    let mut entries: Vec<(i64, u64)> = (1..=600).map(|i| (i, 0xDEAD_BEEF_DEAD_BEEF)).collect();
    entries.push((10_001, 0x0123_4567_89AB_CDEF));
    entries.push((10_002, 0x0123_4567_89AB_CDEF ^ 0b11));
    let found = near_pairs(&entries, 5);
    assert!(found.oversized_buckets > 0);
    assert_eq!(
        found.pairs,
        vec![NearPair {
            a: 10_001,
            b: 10_002,
            distance: 2
        }],
        "the placeholder pairs are dropped; the real near pair survives"
    );
}

#[test]
fn the_threshold_is_clamped_to_the_supported_maximum() {
    let entries = corpus(42, 60);
    assert_eq!(
        near_pairs(&entries, 1_000),
        near_pairs(&entries, MAX_SUPPORTED_DISTANCE)
    );
}

#[test]
fn it_stays_fast_on_a_large_corpus() {
    let entries = corpus(7, 20_000);
    let started = std::time::Instant::now();
    let found = near_pairs(&entries, 5);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
    assert_ne!(found.pairs.len(), 0, "expected near pairs to be found");
}
