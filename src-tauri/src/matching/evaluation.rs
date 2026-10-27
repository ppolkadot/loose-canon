//! Measuring the matching rule against labelled pairs.
//!
//! The two ways the rule can be wrong are reported **separately**, each with its count and
//! denominator, because they are not equally costly: a *false merge* hides a different property
//! behind another one, a *missed duplicate* only leaves two ads side by side. The harness runs
//! the same candidate generation and comparison the application uses, so a duplicate the
//! generator never proposes counts as missed.
//!
//! What a report means depends entirely on where the pairs came from. A set written by the
//! developers (`Provenance::Synthetic`) exercises the rule on constructed cases and says nothing
//! about advertisements in the field; only a labelled sample of real, permitted advertisements
//! (`Provenance::Real`) can support a claim about real performance, and even then only within the
//! limits of its size and how it was drawn. Every report says so.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{
    RULE_VERSION,
    candidates::generate,
    compare::{MatchConfig, MatchInput, compare},
};
use crate::{
    error::{Error, Result},
    models::matching::MatchCategory,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Written by the developers to exercise the rule.
    Synthetic,
    /// Real advertisements, labelled by hand, collected where permitted.
    Real,
}

/// The kinds of case a labelled set should cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    SameCrossSource,
    SameDifferentAngle,
    SimilarProperties,
    Condominium,
    ImageTransformation,
    ConflictingMetadata,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabeledPair {
    pub id: String,
    pub scenario: Scenario,
    /// The hand-made label: are these two advertisements the same property?
    pub same_property: bool,
    pub listing_a: i64,
    pub listing_b: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabeledSet {
    pub description: String,
    pub provenance: Provenance,
    /// Every listing involved, including ones that only appear in the library (their photos can
    /// make a hash common). Each listing is its own merge group.
    pub listings: Vec<MatchInput>,
    pub pairs: Vec<LabeledPair>,
}

impl LabeledSet {
    pub fn from_json(json: &str) -> Result<Self> {
        Ok(serde_json::from_str(json)?)
    }
}

/// `count` out of `of`, never a bare percentage: small samples must look small.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Ratio {
    pub count: usize,
    pub of: usize,
}

/// Where pairs of one kind ended up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CategoryCounts {
    pub high: usize,
    pub possible: usize,
    pub low: usize,
    /// The candidate generator never proposed the pair, so it was never compared.
    pub not_proposed: usize,
}

impl CategoryCounts {
    const fn total(&self) -> usize {
        self.high + self.possible + self.low + self.not_proposed
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ScenarioStats {
    pub same_property: CategoryCounts,
    pub different_property: CategoryCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairOutcome {
    pub id: String,
    pub scenario: Scenario,
    pub same_property: bool,
    pub proposed: bool,
    /// `None` when the pair was never proposed.
    pub category: Option<MatchCategory>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvaluationReport {
    pub rule_version: String,
    pub provenance: Provenance,
    pub description: String,
    pub pairs: usize,
    pub same_property: CategoryCounts,
    pub different_property: CategoryCounts,
    /// Different properties rated `high`: what an automatic merge would have got wrong.
    pub false_merges: Ratio,
    pub false_merge_ids: Vec<String>,
    /// Different properties put in front of the user (`possible` or `high`): review burden.
    pub false_review_items: Ratio,
    /// Same-property pairs that never reached the user (rated `low` or never proposed).
    pub missed_duplicates: Ratio,
    pub missed_duplicate_ids: Vec<String>,
    /// Same-property pairs not rated `high`.
    pub duplicates_not_high: Ratio,
    pub by_scenario: BTreeMap<Scenario, ScenarioStats>,
    pub outcomes: Vec<PairOutcome>,
    pub limitations: Vec<String>,
}

/// Checks that the set is internally consistent and returns its listings (each its own merge
/// group, whatever the file says) with an index by listing id.
fn validated_listings(set: &LabeledSet) -> Result<(Vec<MatchInput>, HashMap<i64, usize>)> {
    let invalid = |detail: String| Error::InvalidConfig {
        what: "evaluation set",
        detail,
    };
    let listings: Vec<MatchInput> = set
        .listings
        .iter()
        .cloned()
        .map(|mut l| {
            l.group_root = l.listing_id;
            l
        })
        .collect();
    let mut index: HashMap<i64, usize> = HashMap::new();
    for (position, listing) in listings.iter().enumerate() {
        if index.insert(listing.listing_id, position).is_some() {
            return Err(invalid(format!(
                "listing {} appears twice",
                listing.listing_id
            )));
        }
    }
    let mut seen_ids = HashSet::new();
    let mut seen_pairs = HashSet::new();
    for pair in &set.pairs {
        if !seen_ids.insert(pair.id.as_str()) {
            return Err(invalid(format!("pair id {:?} appears twice", pair.id)));
        }
        if pair.listing_a == pair.listing_b {
            return Err(invalid(format!(
                "pair {:?} pairs a listing with itself",
                pair.id
            )));
        }
        for id in [pair.listing_a, pair.listing_b] {
            if !index.contains_key(&id) {
                return Err(invalid(format!(
                    "pair {:?} names unknown listing {id}",
                    pair.id
                )));
            }
        }
        if !seen_pairs.insert(ordered(pair)) {
            return Err(invalid(format!(
                "pair {:?} repeats another pair of listings",
                pair.id
            )));
        }
    }
    Ok((listings, index))
}

/// Evaluates the rule on `set`. Fails on an inconsistent set rather than reporting on it.
pub fn evaluate(set: &LabeledSet, config: &MatchConfig) -> Result<EvaluationReport> {
    config.validate()?;
    let (listings, index) = validated_listings(set)?;
    // The same candidate generation and comparison the application runs.
    let candidates = generate(&listings, config);
    let proposed: HashSet<(i64, i64)> = candidates
        .pairs
        .iter()
        .map(|c| (listings[c.a].listing_id, listings[c.b].listing_id))
        .collect();

    let mut same = CategoryCounts::default();
    let mut different = CategoryCounts::default();
    let mut by_scenario: BTreeMap<Scenario, ScenarioStats> = BTreeMap::new();
    let mut outcomes = Vec::with_capacity(set.pairs.len());
    let (mut false_merge_ids, mut missed_duplicate_ids) = (Vec::new(), Vec::new());
    for pair in &set.pairs {
        let key = ordered(pair);
        let was_proposed = proposed.contains(&key);
        let category = was_proposed.then(|| {
            let (a, b) = (&listings[index[&key.0]], &listings[index[&key.1]]);
            compare(a, b, config, &candidates.common_hashes).category
        });
        let scenario = by_scenario.entry(pair.scenario).or_default();
        let (overall, in_scenario) = if pair.same_property {
            (&mut same, &mut scenario.same_property)
        } else {
            (&mut different, &mut scenario.different_property)
        };
        bump(overall, category);
        bump(in_scenario, category);
        match (pair.same_property, category) {
            (false, Some(MatchCategory::High)) => false_merge_ids.push(pair.id.clone()),
            (true, None | Some(MatchCategory::Low)) => missed_duplicate_ids.push(pair.id.clone()),
            _ => {}
        }
        outcomes.push(PairOutcome {
            id: pair.id.clone(),
            scenario: pair.scenario,
            same_property: pair.same_property,
            proposed: was_proposed,
            category,
        });
    }
    false_merge_ids.sort();
    missed_duplicate_ids.sort();

    let false_merges = Ratio {
        count: different.high,
        of: different.total(),
    };
    let report = EvaluationReport {
        rule_version: RULE_VERSION.to_owned(),
        provenance: set.provenance,
        description: set.description.clone(),
        pairs: set.pairs.len(),
        same_property: same,
        different_property: different,
        false_merges,
        false_merge_ids,
        false_review_items: Ratio {
            count: different.high + different.possible,
            of: different.total(),
        },
        missed_duplicates: Ratio {
            count: same.low + same.not_proposed,
            of: same.total(),
        },
        missed_duplicate_ids,
        duplicates_not_high: Ratio {
            count: same.possible + same.low + same.not_proposed,
            of: same.total(),
        },
        by_scenario,
        outcomes,
        limitations: limitations(set.provenance, &same, &different, false_merges),
    };
    Ok(report)
}

/// The pair as `(smaller listing id, larger listing id)`.
fn ordered(pair: &LabeledPair) -> (i64, i64) {
    (
        pair.listing_a.min(pair.listing_b),
        pair.listing_a.max(pair.listing_b),
    )
}

fn bump(counts: &mut CategoryCounts, category: Option<MatchCategory>) {
    match category {
        Some(MatchCategory::High) => counts.high += 1,
        Some(MatchCategory::Possible) => counts.possible += 1,
        Some(MatchCategory::Low) => counts.low += 1,
        None => counts.not_proposed += 1,
    }
}

/// What a reader must keep in mind about these numbers. Always includes the provenance caveat.
fn limitations(
    provenance: Provenance,
    same: &CategoryCounts,
    different: &CategoryCounts,
    false_merges: Ratio,
) -> Vec<String> {
    let mut notes = Vec::new();
    notes.push(match provenance {
        Provenance::Synthetic => "Synthetic set: the pairs were written by the developers to exercise the rule. \
            These counts describe how the rule behaves on constructed cases and say nothing about its \
            performance on real advertisements."
            .to_owned(),
        Provenance::Real => "The counts come from the labelled sample only; its size and how it was drawn bound \
            what can be concluded about advertisements in general."
            .to_owned(),
    });
    notes.push(format!(
        "The rule and its thresholds ({RULE_VERSION}) are experimental and uncalibrated; automatic merging \
         stays disabled and every merge is a user decision."
    ));
    if same.total() == 0 {
        notes.push(
            "There are no same-property pairs, so the share of duplicates missed is undefined."
                .to_owned(),
        );
    }
    if different.total() == 0 {
        notes.push(
            "There are no different-property pairs, so the false-merge rate is undefined."
                .to_owned(),
        );
    }
    if same.total() < 30 || different.total() < 30 {
        notes.push(format!(
            "With fewer than 30 pairs of a kind (same-property: {}, different-property: {}), counts are shown \
             instead of percentages and should not be read as rates.",
            same.total(),
            different.total()
        ));
    }
    if false_merges.of > 0 && false_merges.count == 0 {
        let n = false_merges.of;
        notes.push(format!(
            "No false merge was observed in {n} different-property pairs; with zero events the true \
             false-merge rate could still be about {} per thousand or higher (rule of three, 95% confidence).",
            3000_usize.div_ceil(n)
        ));
    }
    notes
}

#[cfg(test)]
mod tests;
