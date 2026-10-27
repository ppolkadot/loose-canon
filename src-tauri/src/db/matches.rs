//! Match decisions and merges.
//!
//! `property_matches` holds the *current* decision about a pair of listings; every merge and
//! undo is also appended to `property_merge_events`, so repeated cycles stay reconstructable.
//!
//! A merge is **soft**: the absorbed property gets `merged_into_property_id` and keeps every
//! listing, price observation and user status it had, so undoing it is clearing that pointer. A
//! property can only be merged into a *root* (a property that is not itself merged), which makes
//! cycles impossible while still allowing chains; the database enforces the rule with triggers.
//!
//! Every operation here runs in one `IMMEDIATE` transaction and is idempotent: repeating an
//! accept, reject or undo changes nothing and writes no second audit event. Only users decide:
//! there is deliberately no automatic merge (see `crate::matching`).

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use serde_json::json;

use super::{
    format_timestamp,
    listings::StoredPrice,
    parse_timestamp,
    properties::{self, Page, parse_enum, stored_price},
};
use crate::{
    error::{Error, Result},
    models::matching::{DecisionSource, DecisionStatus, MatchCategory, MatchEvidence},
};

/// A comparison result ready to be stored. `listing_a_id < listing_b_id`, and the evidence is
/// relative to that orientation.
pub struct NewMatch<'a> {
    pub listing_a_id: i64,
    pub listing_b_id: i64,
    pub category: MatchCategory,
    pub min_image_distance: Option<u32>,
    pub evidence: &'a MatchEvidence,
    /// What was compared on each side (no free text, no personal data).
    pub inputs: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOutcome {
    Created,
    Updated,
    Unchanged,
    /// The user (or an earlier merge) already decided this pair; the proposal was not applied.
    DecisionKept,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stored {
    pub match_id: i64,
    pub outcome: StoreOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AcceptOutcome {
    Merged {
        surviving: i64,
        absorbed: i64,
    },
    AlreadyMerged,
    /// Both listings already belong to one merge group; nothing was changed.
    AlreadySameGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RejectOutcome {
    Rejected,
    AlreadyRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum UndoOutcome {
    Reverted { surviving: i64, absorbed: i64 },
    AlreadyReverted,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeEventRecord {
    pub id: i64,
    pub match_id: i64,
    pub event: String,
    pub surviving_property_id: i64,
    pub absorbed_property_id: i64,
    pub actor: String,
    pub rule_version: Option<String>,
    pub note: Option<String>,
    pub occurred_at: DateTime<Utc>,
    pub snapshot: serde_json::Value,
}

#[derive(Debug, Clone, Default)]
pub struct MatchQuery {
    pub status: Option<DecisionStatus>,
    /// Only pairs the user can still act on: the two listings are in different merge groups.
    pub actionable_only: bool,
    /// `0` means the default.
    pub limit: u32,
    pub offset: u32,
}

impl MatchQuery {
    pub const DEFAULT_LIMIT: u32 = 50;
    pub const MAX_LIMIT: u32 = 200;

    #[must_use]
    pub fn effective_limit(&self) -> u32 {
        match self.limit {
            0 => Self::DEFAULT_LIMIT,
            n => n.min(Self::MAX_LIMIT),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingBrief {
    pub listing_id: i64,
    pub property_id: i64,
    /// Root of the merge group the listing is in right now.
    pub group_root: i64,
    pub adapter_id: String,
    pub url: String,
    pub title: Option<String>,
    pub price: Option<StoredPriceView>,
}

/// A listing's headline price with its classification, exactly as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredPriceView {
    pub amount: crate::models::money::Money,
    pub classification: crate::models::price::PriceClassification,
    pub confidence: crate::models::confidence::Confidence,
}

impl From<StoredPrice> for StoredPriceView {
    fn from(p: StoredPrice) -> Self {
        Self {
            amount: p.amount,
            classification: p.classification,
            confidence: p.confidence,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeInfo {
    pub surviving_property_id: i64,
    pub absorbed_property_id: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchView {
    pub id: i64,
    pub category: MatchCategory,
    pub decision_status: DecisionStatus,
    pub decision_source: Option<DecisionSource>,
    pub decision_note: Option<String>,
    pub a: ListingBrief,
    pub b: ListingBrief,
    pub evidence: MatchEvidence,
    pub min_image_distance: Option<u32>,
    pub created_at: DateTime<Utc>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub reverted_at: Option<DateTime<Utc>>,
    /// The merge this decision currently holds (or held, when `reverted_at` is set).
    pub merge: Option<MergeInfo>,
}

fn refuse<T>(reason: impl Into<String>) -> Result<T> {
    Err(Error::MatchRefused(reason.into()))
}

/// The stored fields of one match that the operations below decide on.
struct MatchRow {
    a: i64,
    b: i64,
    status: DecisionStatus,
    surviving: Option<i64>,
    absorbed: Option<i64>,
    reverted: bool,
    rule_version: Option<String>,
}

impl MatchRow {
    /// A merge this decision currently holds.
    const fn live_merge(&self) -> Option<(i64, i64)> {
        match (self.status, self.surviving, self.absorbed, self.reverted) {
            (DecisionStatus::Accepted, Some(surviving), Some(absorbed), false) => {
                Some((surviving, absorbed))
            }
            _ => None,
        }
    }
}

/// `(listing a, listing b, decision status, surviving, absorbed, reverted at, evidence json)`.
type RawRow = (
    i64,
    i64,
    String,
    Option<i64>,
    Option<i64>,
    Option<String>,
    String,
);

fn load_row(conn: &Connection, match_id: i64) -> Result<MatchRow> {
    let raw: Option<RawRow> = conn
        .query_row(
            "SELECT listing_a_id, listing_b_id, decision_status, merge_surviving_property_id,
                    merge_absorbed_property_id, reverted_at, evidence_json
               FROM property_matches WHERE id = ?1",
            [match_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .optional()?;
    let Some((a, b, status, surviving, absorbed, reverted_at, evidence_json)) = raw else {
        return refuse(format!("match {match_id} does not exist"));
    };
    let evidence: serde_json::Value = serde_json::from_str(&evidence_json)?;
    Ok(MatchRow {
        a,
        b,
        status: parse_enum("property_matches.decision_status", &status)?,
        surviving,
        absorbed,
        reverted: reverted_at.is_some(),
        rule_version: evidence["rule_version"].as_str().map(str::to_owned),
    })
}

fn property_of(conn: &Connection, listing_id: i64) -> Result<i64> {
    let property: Option<Option<i64>> = conn
        .query_row(
            "SELECT property_id FROM listing_links WHERE id = ?1",
            [listing_id],
            |r| r.get(0),
        )
        .optional()?;
    match property {
        Some(Some(property)) => Ok(property),
        _ => refuse(format!(
            "listing {listing_id} does not belong to a property"
        )),
    }
}

fn root_of_listing(conn: &Connection, listing_id: i64) -> Result<i64> {
    let property = property_of(conn, listing_id)?;
    properties::root_of(conn, property)?.map_or_else(
        || refuse(format!("property {property} has no merge-group root")),
        Ok,
    )
}

// ================================================================================================
// Review status
// ================================================================================================

/// Sets `match_review_status` of a group root: `review_required` while a pair touching exactly
/// one side of the group awaits the user, `reviewed` once any decision touched it, else `none`.
fn refresh_review_status(conn: &Connection, root_id: i64) -> Result<()> {
    let (awaiting, decided): (bool, bool) = conn.query_row(
        "WITH ours(id) AS (
             SELECT l.id FROM listing_links l
              WHERE l.property_id IN (SELECT member_id FROM property_groups WHERE root_id = ?1)
         )
         SELECT
           EXISTS (SELECT 1 FROM property_matches m
                    WHERE m.decision_status = 'review_required'
                      AND ((m.listing_a_id IN (SELECT id FROM ours)) <> (m.listing_b_id IN (SELECT id FROM ours)))),
           EXISTS (SELECT 1 FROM property_matches m
                    WHERE m.decision_status IN ('accepted', 'rejected')
                      AND (m.listing_a_id IN (SELECT id FROM ours) OR m.listing_b_id IN (SELECT id FROM ours)))",
        [root_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let status = if awaiting {
        "review_required"
    } else if decided {
        "reviewed"
    } else {
        "none"
    };
    conn.execute(
        "UPDATE properties SET match_review_status = ?1 WHERE id = ?2 AND match_review_status <> ?1",
        params![status, root_id],
    )?;
    Ok(())
}

fn refresh_review_status_of_pair(conn: &Connection, a: i64, b: i64) -> Result<()> {
    let (root_a, root_b) = (root_of_listing(conn, a)?, root_of_listing(conn, b)?);
    refresh_review_status(conn, root_a)?;
    if root_b != root_a {
        refresh_review_status(conn, root_b)?;
    }
    Ok(())
}

// ================================================================================================
// Storing proposals
// ================================================================================================

/// `(id, decision status, category, image distance, inputs json, evidence json)` of a stored pair.
type ExistingMatch = (i64, String, String, Option<i64>, Option<String>, String);

pub fn store_candidate(
    conn: &mut Connection,
    new: &NewMatch<'_>,
    at: DateTime<Utc>,
) -> Result<Stored> {
    if new.listing_a_id >= new.listing_b_id {
        return refuse("the two listings of a match must be given in ascending id order");
    }
    let status = if new.category == MatchCategory::Low {
        DecisionStatus::Candidate
    } else {
        DecisionStatus::ReviewRequired
    };
    let evidence_json = serde_json::to_string(new.evidence)?;
    let inputs_json = serde_json::to_string(&new.inputs)?;
    let distance = new.min_image_distance.map(i64::from);

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing: Option<ExistingMatch> = tx
        .query_row(
            "SELECT id, decision_status, confidence_category, phash_distance, metadata_signals_json, evidence_json
               FROM property_matches WHERE listing_a_id = ?1 AND listing_b_id = ?2",
            params![new.listing_a_id, new.listing_b_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;

    let stored = match existing {
        None => {
            tx.execute(
                "INSERT INTO property_matches
                    (listing_a_id, listing_b_id, phash_distance, metadata_signals_json, evidence_json,
                     confidence_category, decision_status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    new.listing_a_id,
                    new.listing_b_id,
                    distance,
                    inputs_json,
                    evidence_json,
                    new.category.as_str(),
                    status.as_str(),
                    format_timestamp(at)
                ],
            )?;
            Stored {
                match_id: tx.last_insert_rowid(),
                outcome: StoreOutcome::Created,
            }
        }
        Some((id, decision, category, stored_distance, stored_inputs, stored_evidence)) => {
            let decision: DecisionStatus =
                parse_enum("property_matches.decision_status", &decision)?;
            if matches!(
                decision,
                DecisionStatus::Accepted | DecisionStatus::Rejected
            ) {
                Stored {
                    match_id: id,
                    outcome: StoreOutcome::DecisionKept,
                }
            } else if decision == status
                && category == new.category.as_str()
                && stored_distance == distance
                && stored_inputs.as_deref() == Some(inputs_json.as_str())
                && stored_evidence == evidence_json
            {
                Stored {
                    match_id: id,
                    outcome: StoreOutcome::Unchanged,
                }
            } else {
                tx.execute(
                    "UPDATE property_matches
                        SET phash_distance = ?1, metadata_signals_json = ?2, evidence_json = ?3,
                            confidence_category = ?4, decision_status = ?5
                      WHERE id = ?6",
                    params![
                        distance,
                        inputs_json,
                        evidence_json,
                        new.category.as_str(),
                        status.as_str(),
                        id
                    ],
                )?;
                Stored {
                    match_id: id,
                    outcome: StoreOutcome::Updated,
                }
            }
        }
    };
    if matches!(
        stored.outcome,
        StoreOutcome::Created | StoreOutcome::Updated
    ) {
        refresh_review_status_of_pair(&tx, new.listing_a_id, new.listing_b_id)?;
    }
    tx.commit()?;
    Ok(stored)
}

// ================================================================================================
// Decisions
// ================================================================================================

/// One property as it stands, for the audit snapshot.
fn property_snapshot(conn: &Connection, property_id: i64) -> Result<serde_json::Value> {
    let (user_status, hidden_at, merged_into): (String, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT user_status, hidden_at, merged_into_property_id FROM properties WHERE id = ?1",
            [property_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
    let members: i64 = conn.query_row(
        "SELECT COUNT(*) FROM property_groups WHERE root_id = ?1",
        [property_id],
        |r| r.get(0),
    )?;
    Ok(json!({
        "id": property_id,
        "user_status": user_status,
        "hidden_at": hidden_at,
        "merged_into": merged_into,
        "group_members": members,
    }))
}

/// The effective status of a group root, for the audit snapshot.
fn group_status_snapshot(conn: &Connection, root_id: i64) -> Result<serde_json::Value> {
    Ok(properties::effective_status(conn, root_id)?.map_or(
        serde_json::Value::Null,
        |s| json!({ "effective_status": s.status.as_str(), "status_conflict": s.conflict }),
    ))
}

struct NewEvent<'a> {
    match_id: i64,
    event: &'static str,
    surviving: i64,
    absorbed: i64,
    rule_version: Option<&'a str>,
    note: Option<&'a str>,
    at: &'a str,
    snapshot: &'a serde_json::Value,
}

fn insert_event(conn: &Connection, e: &NewEvent<'_>) -> Result<()> {
    conn.execute(
        "INSERT INTO property_merge_events
            (match_id, event, surviving_property_id, absorbed_property_id, actor, rule_version, note,
             occurred_at, snapshot_json)
         VALUES (?1, ?2, ?3, ?4, 'user', ?5, ?6, ?7, ?8)",
        params![
            e.match_id,
            e.event,
            e.surviving,
            e.absorbed,
            e.rule_version,
            e.note,
            e.at,
            serde_json::to_string(e.snapshot)?
        ],
    )?;
    Ok(())
}

/// Merges the two groups of a match: the property with the smaller id survives, so the outcome
/// does not depend on which listing happens to be `a`.
pub fn accept_match(
    conn: &mut Connection,
    match_id: i64,
    note: Option<&str>,
    at: DateTime<Utc>,
) -> Result<AcceptOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let row = load_row(&tx, match_id)?;
    if row.live_merge().is_some() {
        return Ok(AcceptOutcome::AlreadyMerged);
    }
    let (root_a, root_b) = (root_of_listing(&tx, row.a)?, root_of_listing(&tx, row.b)?);
    if root_a == root_b {
        return Ok(AcceptOutcome::AlreadySameGroup);
    }
    let (surviving, absorbed) = (root_a.min(root_b), root_a.max(root_b));
    let mut snapshot = json!({
        "surviving": property_snapshot(&tx, surviving)?,
        "absorbed": property_snapshot(&tx, absorbed)?,
    });

    let when = format_timestamp(at);
    tx.execute(
        "UPDATE properties SET merged_into_property_id = ?1, merged_at = ?2 WHERE id = ?3",
        params![surviving, when, absorbed],
    )?;
    tx.execute(
        "UPDATE property_matches
            SET decision_status = 'accepted', decision_source = 'user', decision_note = ?1,
                merge_surviving_property_id = ?2, merge_absorbed_property_id = ?3,
                reviewed_at = ?4, reverted_at = NULL
          WHERE id = ?5",
        params![note, surviving, absorbed, when, match_id],
    )?;
    snapshot["after"] = group_status_snapshot(&tx, surviving)?;
    insert_event(
        &tx,
        &NewEvent {
            match_id,
            event: "merged",
            surviving,
            absorbed,
            rule_version: row.rule_version.as_deref(),
            note,
            at: &when,
            snapshot: &snapshot,
        },
    )?;
    properties::refresh_canonical_chain(&tx, surviving)?;
    refresh_review_status_of_pair(&tx, row.a, row.b)?;
    tx.commit()?;
    Ok(AcceptOutcome::Merged {
        surviving,
        absorbed,
    })
}

/// Keeps the pair apart. A live merge must be undone first.
pub fn reject_match(
    conn: &mut Connection,
    match_id: i64,
    note: Option<&str>,
    at: DateTime<Utc>,
) -> Result<RejectOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let row = load_row(&tx, match_id)?;
    if row.status == DecisionStatus::Rejected {
        return Ok(RejectOutcome::AlreadyRejected);
    }
    if row.live_merge().is_some() {
        return refuse(format!(
            "match {match_id} is merged; undo the merge before rejecting it"
        ));
    }
    tx.execute(
        "UPDATE property_matches
            SET decision_status = 'rejected', decision_source = 'user', decision_note = ?1, reviewed_at = ?2
          WHERE id = ?3",
        params![note, format_timestamp(at), match_id],
    )?;
    refresh_review_status_of_pair(&tx, row.a, row.b)?;
    tx.commit()?;
    Ok(RejectOutcome::Rejected)
}

/// Reverses a live merge: clears the absorbed property's pointer, records the match as rejected
/// (so it is not proposed again) and keeps the merge ids as a record of what was undone.
pub fn undo_merge(
    conn: &mut Connection,
    match_id: i64,
    note: Option<&str>,
    at: DateTime<Utc>,
) -> Result<UndoOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let row = load_row(&tx, match_id)?;
    if row.status == DecisionStatus::Rejected && row.reverted {
        return Ok(UndoOutcome::AlreadyReverted);
    }
    let Some((surviving, absorbed)) = row.live_merge() else {
        return refuse(format!("match {match_id} has no live merge to undo"));
    };
    let pointer: Option<i64> = tx.query_row(
        "SELECT merged_into_property_id FROM properties WHERE id = ?1",
        [absorbed],
        |r| r.get(0),
    )?;
    if pointer != Some(surviving) {
        return refuse(format!(
            "match {match_id} records property {absorbed} merged into {surviving}, but the property points elsewhere; \
             nothing was changed"
        ));
    }
    let mut snapshot = json!({
        "surviving": property_snapshot(&tx, surviving)?,
        "absorbed": property_snapshot(&tx, absorbed)?,
    });

    let when = format_timestamp(at);
    tx.execute(
        "UPDATE properties SET merged_into_property_id = NULL, merged_at = NULL WHERE id = ?1",
        [absorbed],
    )?;
    tx.execute(
        "UPDATE property_matches
            SET decision_status = 'rejected', decision_source = 'user', decision_note = ?1,
                reviewed_at = ?2, reverted_at = ?2
          WHERE id = ?3",
        params![note, when, match_id],
    )?;
    snapshot["after"] = json!({
        "surviving": group_status_snapshot(&tx, surviving)?,
        "absorbed": group_status_snapshot(&tx, absorbed)?,
    });
    insert_event(
        &tx,
        &NewEvent {
            match_id,
            event: "reverted",
            surviving,
            absorbed,
            rule_version: row.rule_version.as_deref(),
            note,
            at: &when,
            snapshot: &snapshot,
        },
    )?;
    // The absorbed property's own values never depended on the merge (they come from its own
    // subtree); only the group it left needs recomputing.
    properties::refresh_canonical_chain(&tx, surviving)?;
    refresh_review_status_of_pair(&tx, row.a, row.b)?;
    tx.commit()?;
    Ok(UndoOutcome::Reverted {
        surviving,
        absorbed,
    })
}

// ================================================================================================
// Reading
// ================================================================================================

pub fn merge_events(conn: &Connection, match_id: i64) -> Result<Vec<MergeEventRecord>> {
    let mut statement = conn.prepare(
        "SELECT id, match_id, event, surviving_property_id, absorbed_property_id, actor, rule_version, note,
                occurred_at, snapshot_json
           FROM property_merge_events WHERE match_id = ?1 ORDER BY id",
    )?;
    let raw = statement
        .query_map([match_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, String>(9)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    raw.into_iter()
        .map(
            |(
                id,
                match_id,
                event,
                surviving,
                absorbed,
                actor,
                rule_version,
                note,
                occurred_at,
                snapshot,
            )| {
                Ok(MergeEventRecord {
                    id,
                    match_id,
                    event,
                    surviving_property_id: surviving,
                    absorbed_property_id: absorbed,
                    actor,
                    rule_version,
                    note,
                    occurred_at: parse_timestamp(&occurred_at)?,
                    snapshot: serde_json::from_str(&snapshot)?,
                })
            },
        )
        .collect()
}

const MATCH_COLUMNS: &str = "m.id, m.listing_a_id, m.listing_b_id, m.phash_distance, m.evidence_json,
    m.confidence_category, m.decision_status, m.decision_source, m.decision_note,
    m.merge_surviving_property_id, m.merge_absorbed_property_id, m.created_at, m.reviewed_at, m.reverted_at";

type RawMatch = (
    i64,
    i64,
    i64,
    Option<i64>,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    String,
    Option<String>,
    Option<String>,
);

fn raw_match(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawMatch> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
    ))
}

fn brief(conn: &Connection, listing_id: i64) -> Result<ListingBrief> {
    type Raw = (
        i64,
        i64,
        i64,
        String,
        String,
        Option<String>,
        Option<i64>,
        String,
        Option<String>,
    );
    let raw: Option<Raw> = conn
        .query_row(
            "SELECT l.id, l.property_id, g.root_id, l.adapter_id, l.url, l.title,
                    l.price_centavos, l.price_classification, l.price_confidence
               FROM listing_links l
               JOIN property_groups g ON g.member_id = l.property_id
              WHERE l.id = ?1",
            [listing_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            },
        )
        .optional()?;
    let Some((
        id,
        property_id,
        group_root,
        adapter_id,
        url,
        title,
        centavos,
        classification,
        confidence,
    )) = raw
    else {
        return Err(Error::InvalidStoredValue {
            column: "property_matches.listing_id",
            value: format!("listing {listing_id} has no property"),
        });
    };
    Ok(ListingBrief {
        listing_id: id,
        property_id,
        group_root,
        adapter_id,
        url,
        title,
        price: stored_price(centavos, &classification, confidence.as_deref())?
            .map(StoredPriceView::from),
    })
}

fn view_from_raw(conn: &Connection, raw: RawMatch) -> Result<MatchView> {
    let (
        id,
        a,
        b,
        distance,
        evidence,
        category,
        status,
        source,
        note,
        surviving,
        absorbed,
        created,
        reviewed,
        reverted,
    ) = raw;
    let optional_time = |value: Option<String>| value.as_deref().map(parse_timestamp).transpose();
    Ok(MatchView {
        id,
        category: parse_enum("property_matches.confidence_category", &category)?,
        decision_status: parse_enum("property_matches.decision_status", &status)?,
        decision_source: source
            .as_deref()
            .map(|s| parse_enum("property_matches.decision_source", s))
            .transpose()?,
        decision_note: note,
        a: brief(conn, a)?,
        b: brief(conn, b)?,
        evidence: serde_json::from_str(&evidence)?,
        min_image_distance: distance.and_then(|d| u32::try_from(d).ok()),
        created_at: parse_timestamp(&created)?,
        reviewed_at: optional_time(reviewed)?,
        reverted_at: optional_time(reverted)?,
        merge: surviving
            .zip(absorbed)
            .map(|(surviving_property_id, absorbed_property_id)| MergeInfo {
                surviving_property_id,
                absorbed_property_id,
            }),
    })
}

pub fn get_match(conn: &Connection, match_id: i64) -> Result<Option<MatchView>> {
    let raw = conn
        .query_row(
            &format!("SELECT {MATCH_COLUMNS} FROM property_matches m WHERE m.id = ?1"),
            [match_id],
            raw_match,
        )
        .optional()?;
    raw.map(|raw| view_from_raw(conn, raw)).transpose()
}

pub fn list_matches(conn: &Connection, query: &MatchQuery) -> Result<Page<MatchView>> {
    let limit = query.effective_limit();
    let status = query.status.map(DecisionStatus::as_str);
    let actionable = i64::from(query.actionable_only);
    let from = "FROM property_matches m
         JOIN listing_links la ON la.id = m.listing_a_id
         JOIN listing_links lb ON lb.id = m.listing_b_id
         JOIN property_groups ga ON ga.member_id = la.property_id
         JOIN property_groups gb ON gb.member_id = lb.property_id
        WHERE (?1 IS NULL OR m.decision_status = ?1) AND (?2 = 0 OR ga.root_id <> gb.root_id)";
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) {from}"),
        params![status, actionable],
        |r| r.get(0),
    )?;
    let mut statement = conn.prepare(&format!(
        "SELECT {MATCH_COLUMNS} {from}
          ORDER BY CASE m.confidence_category WHEN 'high' THEN 0 WHEN 'possible' THEN 1 ELSE 2 END, m.id
          LIMIT ?3 OFFSET ?4"
    ))?;
    let raw = statement
        .query_map(params![status, actionable, limit, query.offset], raw_match)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let items = raw
        .into_iter()
        .map(|raw| view_from_raw(conn, raw))
        .collect::<Result<Vec<_>>>()?;
    Ok(Page {
        items,
        total: u64::try_from(total).unwrap_or(0),
        offset: query.offset,
        limit,
    })
}

pub fn decided_pairs(conn: &Connection) -> Result<HashSet<(i64, i64)>> {
    let mut statement = conn.prepare(
        "SELECT listing_a_id, listing_b_id FROM property_matches WHERE decision_status IN ('accepted', 'rejected')",
    )?;
    let rows = statement
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<std::result::Result<HashSet<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests;
