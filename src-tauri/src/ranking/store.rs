//! Saving and reading scores (`properties.local_score*`, `data_confidence`, `ranking_version`).

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;

use super::score::{Ranked, ScoreDetails};
use crate::{
    db::{format_timestamp, parse_timestamp},
    error::{Error, Result},
    models::confidence::Confidence,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReport {
    /// Properties saved with a score.
    pub scored: usize,
    /// Properties saved without one (nothing could be computed), with their explanation.
    pub unscored: usize,
    /// Properties that had a score but are no longer ranked (no active listing, or merged away).
    pub cleared: usize,
}

/// What is stored for one property.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoredScore {
    pub score: Option<f64>,
    pub confidence: Option<Confidence>,
    pub details: Option<ScoreDetails>,
    pub ranking_version: Option<String>,
    pub scored_at: Option<DateTime<Utc>>,
}

/// Replaces every stored score with `ranked` in one transaction: ranked properties are written,
/// every other property loses any score it had (a stale score must not outlive its basis).
/// A property id that does not exist is ignored.
pub fn save_ranking(
    conn: &mut Connection,
    ranked: &[Ranked],
    at: DateTime<Utc>,
) -> Result<SaveReport> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let when = format_timestamp(at);
    let mut report = SaveReport::default();
    let mut written = Vec::with_capacity(ranked.len());
    for r in ranked {
        let changed = tx.execute(
            "UPDATE properties
                SET local_score = ?1, local_score_json = ?2, data_confidence = ?3,
                    ranking_version = ?4, scored_at = ?5
              WHERE id = ?6",
            params![
                r.score,
                serde_json::to_string(&r.details)?,
                r.confidence.as_str(),
                r.details.ranking_version,
                when,
                r.property_id
            ],
        )?;
        if changed == 1 {
            written.push(r.property_id);
            if r.score.is_some() {
                report.scored += 1;
            } else {
                report.unscored += 1;
            }
        }
    }
    report.cleared = tx.execute(
        "UPDATE properties
            SET local_score = NULL, local_score_json = NULL, data_confidence = NULL,
                ranking_version = NULL, scored_at = NULL
          WHERE (local_score IS NOT NULL OR local_score_json IS NOT NULL OR data_confidence IS NOT NULL
                 OR ranking_version IS NOT NULL OR scored_at IS NOT NULL)
            AND id NOT IN (SELECT value FROM json_each(?1))",
        [serde_json::to_string(&written)?],
    )?;
    tx.commit()?;
    Ok(report)
}

/// The stored score of a property, or `None` when the property does not exist.
pub fn load_score(conn: &Connection, property_id: i64) -> Result<Option<StoredScore>> {
    type Row = (
        Option<f64>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<Row> = conn
        .query_row(
            "SELECT local_score, local_score_json, data_confidence, ranking_version, scored_at
               FROM properties WHERE id = ?1",
            [property_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((score, details, confidence, ranking_version, scored_at)) = row else {
        return Ok(None);
    };
    Ok(Some(StoredScore {
        score,
        confidence: confidence
            .map(|c| {
                c.parse().map_err(|_| Error::InvalidStoredValue {
                    column: "properties.data_confidence",
                    value: c.clone(),
                })
            })
            .transpose()?,
        details: details.map(|d| serde_json::from_str(&d)).transpose()?,
        ranking_version,
        scored_at: scored_at.as_deref().map(parse_timestamp).transpose()?,
    }))
}

#[cfg(test)]
mod tests;
