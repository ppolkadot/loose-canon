use super::*;
use crate::matching::compare::ImageHash;

const H1: u64 = 0x0123_4567_89ab_cdef;
const H2: u64 = 0xf0e1_d2c3_b4a5_9687;

fn config() -> MatchConfig {
    MatchConfig::default()
}

fn listing(id: i64, hashes: &[u64]) -> MatchInput {
    let mut m = MatchInput::new(id, "alpha");
    m.images = hashes
        .iter()
        .enumerate()
        .map(|(i, &hash)| ImageHash {
            image_id: id * 100 + i64::try_from(i).unwrap(),
            hash,
        })
        .collect();
    m
}

fn placed(id: i64, municipality: &str, neighborhood: Option<&str>) -> MatchInput {
    let mut m = listing(id, &[]);
    m.municipality = Some(municipality.into());
    m.neighborhood = neighborhood.map(Into::into);
    m
}

fn ids(c: &Candidates, inputs: &[MatchInput]) -> Vec<(i64, i64)> {
    c.pairs
        .iter()
        .map(|p| (inputs[p.a].listing_id, inputs[p.b].listing_id))
        .collect()
}

#[test]
fn near_identical_images_propose_a_candidate_marked_as_from_images() {
    let inputs = [
        listing(1, &[H1]),
        listing(2, &[H1 ^ 0b101]),
        listing(3, &[!H1]),
    ];
    let found = generate(&inputs, &config());
    assert_eq!(ids(&found, &inputs), vec![(1, 2)]);
    assert!(found.pairs[0].from_images && !found.pairs[0].from_metadata);
}

#[test]
fn images_beyond_the_threshold_propose_nothing() {
    let inputs = [listing(1, &[H1]), listing(2, &[H1 ^ 0b11_1111])];
    let proposed = generate(&inputs, &config());
    assert_eq!(proposed.pairs.len(), 0, "{:?}", proposed.pairs);
}

#[test]
fn a_listing_is_never_paired_with_itself_through_its_own_similar_photos() {
    let inputs = [listing(1, &[H1, H1 ^ 1, H1 ^ 2])];
    let proposed = generate(&inputs, &config());
    assert_eq!(proposed.pairs.len(), 0, "{:?}", proposed.pairs);
}

#[test]
fn a_hash_shared_by_many_listings_proposes_nothing_and_is_reported() {
    let inputs: Vec<MatchInput> = (1..=4).map(|id| listing(id, &[H2])).collect();
    let found = generate(&inputs, &config());
    assert!(
        found.pairs.is_empty(),
        "a placeholder image links everything to everything"
    );
    assert!(found.common_hashes.contains(&H2));
    let three: Vec<MatchInput> = (1..=3).map(|id| listing(id, &[H2])).collect();
    let found = generate(&three, &config());
    assert!(
        found.common_hashes.is_empty(),
        "three listings is below the limit of four"
    );
    assert_eq!(found.pairs.len(), 3);
}

#[test]
fn listings_in_the_same_neighbourhood_are_proposed_from_metadata() {
    let inputs = [
        placed(1, "goiania", Some("setor bueno")),
        placed(2, "goiania", Some("setor bueno")),
        placed(3, "goiania", Some("setor marista")),
        placed(4, "aparecida de goiania", Some("setor bueno")),
        placed(5, "goiania", None),
        placed(6, "goiania", None),
    ];
    let found = generate(&inputs, &config());
    assert_eq!(ids(&found, &inputs), vec![(1, 2)]);
    assert!(found.pairs[0].from_metadata && !found.pairs[0].from_images);
}

#[test]
fn a_pair_found_both_ways_is_one_candidate_with_both_flags() {
    let mut a = placed(1, "goiania", Some("setor bueno"));
    let mut b = placed(2, "goiania", Some("setor bueno"));
    a.images = listing(1, &[H1]).images;
    b.images = listing(2, &[H1]).images;
    let found = generate(&[a, b], &config());
    assert_eq!(found.pairs.len(), 1);
    assert!(found.pairs[0].from_images && found.pairs[0].from_metadata);
}

#[test]
fn pairs_are_oriented_by_listing_id_and_sorted() {
    // Supply the inputs out of order.
    let inputs = [listing(30, &[H1]), listing(10, &[H1]), listing(20, &[H1])];
    let found = generate(&inputs, &config());
    assert_eq!(ids(&found, &inputs), vec![(10, 20), (10, 30), (20, 30)]);
}

#[test]
fn listings_already_in_one_merge_group_are_never_proposed() {
    let mut a = listing(1, &[H1]);
    let mut b = listing(2, &[H1]);
    a.group_root = 7;
    b.group_root = 7;
    let proposed = generate(&[a, b], &config());
    assert_eq!(proposed.pairs.len(), 0, "{:?}", proposed.pairs);
}

#[test]
fn oversized_metadata_blocks_are_skipped_and_counted_not_expanded() {
    let small = MatchConfig {
        max_block_size: 3,
        ..config()
    };
    let inputs: Vec<MatchInput> = (1..=4)
        .map(|id| placed(id, "goiania", Some("centro")))
        .collect();
    let found = generate(&inputs, &small);
    assert_eq!(found.pairs.len(), 0, "{:?}", found.pairs);
    assert_eq!(found.oversized_blocks, 1);
    let at_limit: Vec<MatchInput> = (1..=3)
        .map(|id| placed(id, "goiania", Some("centro")))
        .collect();
    let found = generate(&at_limit, &small);
    assert_eq!((found.pairs.len(), found.oversized_blocks), (3, 0));
}

#[test]
fn a_large_unrelated_library_is_not_scanned_pairwise() {
    // 4000 listings with unrelated hashes and no location: nothing to propose, and fast.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let inputs: Vec<MatchInput> = (1..=4000)
        .map(|id| listing(id, &[next(), next()]))
        .collect();
    let started = std::time::Instant::now();
    let found = generate(&inputs, &config());
    assert!(found.pairs.is_empty(), "{} pairs", found.pairs.len());
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn generation_is_deterministic() {
    let inputs = [
        listing(1, &[H1]),
        listing(2, &[H1 ^ 1]),
        placed(3, "goiania", Some("centro")),
        placed(4, "goiania", Some("centro")),
    ];
    assert_eq!(generate(&inputs, &config()), generate(&inputs, &config()));
}
