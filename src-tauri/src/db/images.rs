//! Image metadata: which images a listing has, what has been processed, and the hashes.
//!
//! Only metadata lives here; image files live in the application cache and are referenced by a
//! relative path (the schema refuses absolute or drive-qualified paths).

use chrono::{DateTime, Utc};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::Serialize;

use super::format_timestamp;
use crate::{
    error::{Error, ModelError, Result},
    images::phash::{ALGORITHM, from_db, to_db},
    models::image::ImageProcessingStatus,
    sources::url_norm::parse_http_url,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingImage {
    pub id: i64,
    pub listing_id: i64,
    pub position: u32,
    pub adapter_id: String,
    pub source_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredImage {
    pub id: i64,
    pub listing_id: i64,
    pub position: u32,
    pub source_url: Option<String>,
    pub local_path: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub phash: Option<u64>,
    pub phash_algorithm: Option<String>,
    pub status: ImageProcessingStatus,
    pub error: Option<String>,
}

/// Registers up to `max_per_listing` images for a listing (counting those it already has),
/// ignoring URLs that are not plain http(s) and URLs already registered. Returns how many rows
/// were added. Nothing is downloaded here.
pub fn register_images(
    conn: &mut Connection,
    listing_id: i64,
    urls: &[String],
    max_per_listing: usize,
    at: DateTime<Utc>,
) -> Result<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing: i64 = tx.query_row(
        "SELECT COUNT(*) FROM property_images WHERE listing_id = ?1",
        [listing_id],
        |r| r.get(0),
    )?;
    let mut position: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM property_images WHERE listing_id = ?1",
        [listing_id],
        |r| r.get(0),
    )?;
    let room = max_per_listing.saturating_sub(usize::try_from(existing).unwrap_or(usize::MAX));
    let created_at = format_timestamp(at);
    let mut added = 0;
    for url in urls {
        if added >= room {
            break;
        }
        if parse_http_url(url).is_err() {
            continue;
        }
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO property_images (listing_id, position, source_url, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![listing_id, position, url, created_at],
        )?;
        if inserted == 1 {
            added += 1;
            position += 1;
        }
    }
    tx.commit()?;
    Ok(added)
}

/// `?first, ?first+1, ...` for `count` bound values. Only placeholders are generated into the
/// query text; every value is bound.
fn placeholders(first: usize, count: usize) -> String {
    (first..first + count)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Images of the given adapters still to process (never processed, or failed transiently),
/// oldest first. Images of other adapters are not returned, so they cannot use up `limit`.
pub fn pending_images(
    conn: &Connection,
    adapter_ids: &[&str],
    limit: u32,
) -> Result<Vec<PendingImage>> {
    if adapter_ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT i.id, i.listing_id, i.position, l.adapter_id, i.source_url
           FROM property_images i
           JOIN listing_links l ON l.id = i.listing_id
          WHERE i.processing_status = 'pending' AND i.source_url IS NOT NULL
            AND l.adapter_id IN ({})
          ORDER BY i.id
          LIMIT ?1",
        placeholders(2, adapter_ids.len())
    );
    let mut statement = conn.prepare(&sql)?;
    let values = std::iter::once(rusqlite::types::Value::from(i64::from(limit))).chain(
        adapter_ids
            .iter()
            .map(|id| rusqlite::types::Value::from((*id).to_owned())),
    );
    let rows = statement
        .query_map(rusqlite::params_from_iter(values), |r| {
            Ok(PendingImage {
                id: r.get(0)?,
                listing_id: r.get(1)?,
                position: r.get(2)?,
                adapter_id: r.get(3)?,
                source_url: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// How many pending images belong to adapters **not** in `adapter_ids` (nothing can process
/// them right now; they stay pending).
pub fn count_pending_outside(conn: &Connection, adapter_ids: &[&str]) -> Result<u32> {
    let filter = if adapter_ids.is_empty() {
        String::new()
    } else {
        format!(
            "AND l.adapter_id NOT IN ({})",
            placeholders(1, adapter_ids.len())
        )
    };
    let sql = format!(
        "SELECT COUNT(*)
           FROM property_images i
           JOIN listing_links l ON l.id = i.listing_id
          WHERE i.processing_status = 'pending' AND i.source_url IS NOT NULL {filter}"
    );
    let count: i64 = conn.query_row(&sql, rusqlite::params_from_iter(adapter_ids.iter()), |r| {
        r.get(0)
    })?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// Stores the result of a successful download + hash.
pub fn record_hashed(
    conn: &Connection,
    image_id: i64,
    local_path: &str,
    width: u32,
    height: u32,
    phash: u64,
    at: DateTime<Utc>,
) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE property_images
            SET local_path = ?1, width = ?2, height = ?3, phash = ?4, phash_algorithm = ?5,
                processing_status = 'hashed', processing_error = NULL, processed_at = ?6
          WHERE id = ?7",
        params![
            local_path,
            width,
            height,
            to_db(phash),
            ALGORITHM,
            format_timestamp(at),
            image_id
        ],
    )?;
    Ok(changed == 1)
}

/// Records a **permanent** outcome (`Failed` or `Skipped`) with the reason; not retried.
pub fn record_final(
    conn: &Connection,
    image_id: i64,
    status: ImageProcessingStatus,
    error_kind: &str,
    at: DateTime<Utc>,
) -> Result<bool> {
    if !matches!(
        status,
        ImageProcessingStatus::Failed | ImageProcessingStatus::Skipped
    ) {
        return Err(
            ModelError::InvalidOutcome(format!("{status} is not a final failure status")).into(),
        );
    }
    let changed = conn.execute(
        "UPDATE property_images
            SET processing_status = ?1, processing_error = ?2, processed_at = ?3
          WHERE id = ?4",
        params![status.as_str(), error_kind, format_timestamp(at), image_id],
    )?;
    Ok(changed == 1)
}

/// Records a **transient** failure: the image stays `pending` (so a later run retries) with the
/// reason visible.
pub fn record_transient(conn: &Connection, image_id: i64, error_kind: &str) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE property_images SET processing_error = ?1
          WHERE id = ?2 AND processing_status = 'pending'",
        params![error_kind, image_id],
    )?;
    Ok(changed == 1)
}

pub fn images_of_listing(conn: &Connection, listing_id: i64) -> Result<Vec<StoredImage>> {
    let mut statement = conn.prepare(
        "SELECT id, listing_id, position, source_url, local_path, width, height, phash, phash_algorithm,
                processing_status, processing_error
           FROM property_images WHERE listing_id = ?1 ORDER BY position, id",
    )?;
    let raw = statement
        .query_map([listing_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, u32>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<u32>>(5)?,
                r.get::<_, Option<u32>>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, Option<String>>(8)?,
                r.get::<_, String>(9)?,
                r.get::<_, Option<String>>(10)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    raw.into_iter()
        .map(
            |(
                id,
                listing_id,
                position,
                source_url,
                local_path,
                width,
                height,
                phash,
                algorithm,
                status,
                error,
            )| {
                Ok(StoredImage {
                    id,
                    listing_id,
                    position,
                    source_url,
                    local_path,
                    width,
                    height,
                    phash: phash.map(from_db),
                    phash_algorithm: algorithm,
                    status: status.parse().map_err(|_| Error::InvalidStoredValue {
                        column: "property_images.processing_status",
                        value: status.clone(),
                    })?,
                    error,
                })
            },
        )
        .collect()
}

/// `(image_id, listing_id, hash)` of every image hashed with the **current** algorithm.
pub fn hashes(conn: &Connection) -> Result<Vec<(i64, i64, u64)>> {
    let mut statement = conn.prepare(
        "SELECT id, listing_id, phash FROM property_images
          WHERE phash IS NOT NULL AND phash_algorithm = ?1 ORDER BY id",
    )?;
    let rows = statement
        .query_map([ALGORITHM], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .map(|(id, listing, hash)| (id, listing, from_db(hash)))
        .collect())
}

#[cfg(test)]
mod tests;
