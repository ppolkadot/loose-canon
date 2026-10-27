//! One matching pass over the stored listings: load, propose candidates, compare, store.
//!
//! Nothing is ever merged here. A pass only records proposals; merging is a user decision made
//! through `db::matches`. Database work is short and runs off the async workers; the comparison
//! itself holds no database lock.

use std::collections::HashSet;

use serde::Serialize;
use serde_json::json;

use super::{
    RULE_VERSION,
    candidates::generate,
    compare::{MatchConfig, MatchInput, compare},
    load::load_inputs,
};
use crate::{
    db::matches::{NewMatch, StoreOutcome, decided_pairs, store_candidate},
    error::{Error, Result},
    models::matching::{MatchCategory, MatchEvidence},
    sources::{
        CancelToken,
        pipeline::{CollectionDeps, with_db},
    },
};

/// Proposals written per transaction, so one pass never holds the database for long.
const STORE_BATCH: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchRunReport {
    pub rule_version: String,
    pub listings: usize,
    /// Pairs proposed by photos and/or metadata.
    pub candidate_pairs: usize,
    /// Proposed pairs skipped because the user already decided them.
    pub skipped_decided: usize,
    pub compared: usize,
    pub high: usize,
    pub possible: usize,
    /// `Low` results stored as "considered and kept apart" (they came from photos).
    pub low_stored: usize,
    /// `Low` results from metadata alone, not stored (there would be far too many).
    pub low_discarded: usize,
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    /// Proposals not applied because a decision exists.
    pub decisions_kept: usize,
    pub oversized_blocks: usize,
    pub oversized_image_buckets: usize,
    /// Image hashes ignored because many listings share them.
    pub common_hashes: usize,
    pub cancelled: bool,
}

/// A comparison result ready to be stored.
struct Proposal {
    listing_a: i64,
    listing_b: i64,
    category: MatchCategory,
    min_image_distance: Option<u32>,
    evidence: MatchEvidence,
    inputs: serde_json::Value,
}

/// Runs one pass. Fails before doing any work on an invalid configuration.
pub async fn run_matching(
    deps: &CollectionDeps,
    config: &MatchConfig,
    cancel: &CancelToken,
) -> Result<MatchRunReport> {
    run_batched(deps, config, cancel, STORE_BATCH).await
}

/// [`run_matching`] with the transaction size passed in, so batching and cancellation between
/// batches can be tested with small numbers.
async fn run_batched(
    deps: &CollectionDeps,
    config: &MatchConfig,
    cancel: &CancelToken,
    batch_size: usize,
) -> Result<MatchRunReport> {
    config.validate()?;
    let mut report = MatchRunReport {
        rule_version: RULE_VERSION.to_owned(),
        ..MatchRunReport::default()
    };
    if cancel.is_cancelled() {
        report.cancelled = true;
        return Ok(report);
    }

    let (inputs, decided) = with_db(&deps.db, |conn| {
        Ok((load_inputs(conn)?, decided_pairs(conn)?))
    })
    .await?;
    report.listings = inputs.len();
    if cancel.is_cancelled() {
        report.cancelled = true;
        return Ok(report);
    }

    let config = config.clone();
    let (proposals, counted) =
        tokio::task::spawn_blocking(move || propose(&inputs, &decided, &config))
            .await
            .map_err(|e| Error::Io(std::io::Error::other(format!("matching task failed: {e}"))))?;
    report = MatchRunReport {
        listings: report.listings,
        ..counted
    };

    let mut proposals = proposals.into_iter().peekable();
    while proposals.peek().is_some() {
        if cancel.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let batch: Vec<Proposal> = proposals.by_ref().take(batch_size).collect();
        let at = (deps.clock)();
        let outcomes = with_db(&deps.db, move |conn| {
            batch
                .iter()
                .map(|p| {
                    let new = NewMatch {
                        listing_a_id: p.listing_a,
                        listing_b_id: p.listing_b,
                        category: p.category,
                        min_image_distance: p.min_image_distance,
                        evidence: &p.evidence,
                        inputs: p.inputs.clone(),
                    };
                    store_candidate(conn, &new, at).map(|stored| stored.outcome)
                })
                .collect::<Result<Vec<_>>>()
        })
        .await?;
        for outcome in outcomes {
            match outcome {
                StoreOutcome::Created => report.created += 1,
                StoreOutcome::Updated => report.updated += 1,
                StoreOutcome::Unchanged => report.unchanged += 1,
                StoreOutcome::DecisionKept => report.decisions_kept += 1,
            }
        }
    }
    Ok(report)
}

/// Generates candidates, compares them and decides which results are worth storing. Pure.
fn propose(
    inputs: &[MatchInput],
    decided: &HashSet<(i64, i64)>,
    config: &MatchConfig,
) -> (Vec<Proposal>, MatchRunReport) {
    let candidates = generate(inputs, config);
    let mut report = MatchRunReport {
        rule_version: RULE_VERSION.to_owned(),
        candidate_pairs: candidates.pairs.len(),
        oversized_blocks: candidates.oversized_blocks,
        oversized_image_buckets: candidates.oversized_image_buckets,
        common_hashes: candidates.common_hashes.len(),
        ..MatchRunReport::default()
    };
    let mut proposals = Vec::new();
    for candidate in &candidates.pairs {
        let (a, b) = (&inputs[candidate.a], &inputs[candidate.b]);
        if decided.contains(&(a.listing_id, b.listing_id)) {
            report.skipped_decided += 1;
            continue;
        }
        let result = compare(a, b, config, &candidates.common_hashes);
        report.compared += 1;
        match result.category {
            MatchCategory::High => report.high += 1,
            MatchCategory::Possible => report.possible += 1,
            MatchCategory::Low if candidate.from_images => report.low_stored += 1,
            MatchCategory::Low => {
                report.low_discarded += 1;
                continue;
            }
        }
        proposals.push(Proposal {
            listing_a: a.listing_id,
            listing_b: b.listing_id,
            category: result.category,
            min_image_distance: result.min_image_distance,
            evidence: result.evidence,
            inputs: json!({ "a": a.summary(), "b": b.summary() }),
        });
    }
    (proposals, report)
}

#[cfg(test)]
mod tests;
