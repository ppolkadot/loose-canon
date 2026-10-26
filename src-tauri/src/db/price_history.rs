//! Append-only record of every distinct price a listing has shown.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{format_timestamp, parse_timestamp};
use crate::{
    error::{Error, Result},
    models::{
        confidence::Confidence,
        money::Money,
        price::{PriceClassification, PriceProvenance},
    },
};

/// One price seen on a listing at a point in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceObservation {
    pub amount: Money,
    pub classification: PriceClassification,
    /// Confidence in the classification, when known.
    pub confidence: Option<Confidence>,
    pub observed_at: DateTime<Utc>,
    pub provenance: PriceProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// A new history row was written.
    Recorded { id: i64 },
    /// The latest recorded observation already has this amount and classification.
    Unchanged { latest_id: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObservation {
    pub id: i64,
    pub listing_id: i64,
    pub observation: PriceObservation,
}

/// Records `observation` unless it repeats the latest recorded price of the listing.
///
/// "Same price" means same amount **and** same classification: an *ágio* of R$ 80.000 turning
/// into a full price of R$ 80.000 is a change worth keeping. Differences in confidence,
/// provenance or timestamp alone are not price changes. Rows are never updated or deleted
/// (the schema enforces it), so a refresh can only add history.
///
/// An observation older than the latest recorded one is rejected with
/// [`Error::OutOfOrderObservation`] rather than being slotted into the past.
pub fn record_observation(
    conn: &mut Connection,
    listing_id: i64,
    observation: &PriceObservation,
) -> Result<RecordOutcome> {
    // IMMEDIATE takes the write lock up front so the "latest" we compare against cannot change
    // between the check and the insert.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let observed_at = format_timestamp(observation.observed_at);

    let latest: Option<(i64, i64, String, String)> = tx
        .query_row(
            "SELECT id, amount_centavos, price_classification, observed_at
               FROM listing_price_history
              WHERE listing_id = ?1
              ORDER BY observed_at DESC, id DESC
              LIMIT 1",
            [listing_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;

    if let Some((latest_id, amount, classification, latest_at)) = latest {
        if observed_at < latest_at {
            return Err(Error::OutOfOrderObservation {
                observed_at,
                latest: latest_at,
            });
        }
        if amount == observation.amount.centavos()
            && classification == observation.classification.as_str()
        {
            return Ok(RecordOutcome::Unchanged { latest_id });
        }
    }

    tx.execute(
        "INSERT INTO listing_price_history
            (listing_id, amount_centavos, price_classification, confidence, observed_at, provenance_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            listing_id,
            observation.amount.centavos(),
            observation.classification.as_str(),
            observation.confidence.map(Confidence::as_str),
            observed_at,
            serde_json::to_string(&observation.provenance)?,
        ],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(RecordOutcome::Recorded { id })
}

/// Observations of a listing, oldest first.
pub fn list_observations(conn: &Connection, listing_id: i64) -> Result<Vec<StoredObservation>> {
    let mut statement = conn.prepare(
        "SELECT id, amount_centavos, price_classification, confidence, observed_at, provenance_json
           FROM listing_price_history
          WHERE listing_id = ?1
          ORDER BY observed_at ASC, id ASC",
    )?;
    let raw_rows = statement
        .query_map([listing_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    raw_rows
        .into_iter()
        .map(
            |(id, amount, classification, confidence, observed_at, provenance)| {
                let invalid = |column: &'static str, value: &str| Error::InvalidStoredValue {
                    column,
                    value: value.to_owned(),
                };
                Ok(StoredObservation {
                    id,
                    listing_id,
                    observation: PriceObservation {
                        amount: Money::from_centavos(amount)?,
                        classification: classification
                            .parse()
                            .map_err(|_| invalid("price_classification", &classification))?,
                        confidence: confidence
                            .as_deref()
                            .map(|c| c.parse().map_err(|_| invalid("confidence", c)))
                            .transpose()?,
                        observed_at: parse_timestamp(&observed_at)?,
                        provenance: serde_json::from_str(&provenance)?,
                    },
                })
            },
        )
        .collect()
}
