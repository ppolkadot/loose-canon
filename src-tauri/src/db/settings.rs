//! Small JSON settings documents (`app_settings`): search preferences, ranking weights, AI options.
//!
//! **Secrets never go through here.** A document that no longer matches the shape the code
//! expects is an *error*, never silently replaced by defaults: that would discard what the user
//! chose without telling them.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

use super::{format_timestamp, parse_timestamp};
use crate::error::Result;

/// The stored document for `key`, or `None` when it was never saved.
pub fn get_setting<T: DeserializeOwned>(conn: &Connection, key: &str) -> Result<Option<T>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    json.map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(Into::into)
}

/// The stored document, or `T::default()` when none was saved. A stored document of the wrong
/// shape is an error.
pub fn get_setting_or_default<T: DeserializeOwned + Default>(
    conn: &Connection,
    key: &str,
) -> Result<T> {
    Ok(get_setting(conn, key)?.unwrap_or_default())
}

/// Saves (inserts or replaces) the document for `key`.
pub fn set_setting<T: Serialize>(
    conn: &Connection,
    key: &str,
    value: &T,
    at: DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO app_settings (key, value_json, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json,
                                        updated_at = excluded.updated_at",
        params![key, serde_json::to_string(value)?, format_timestamp(at)],
    )?;
    Ok(())
}

/// When the document for `key` was last saved.
pub fn setting_updated_at(conn: &Connection, key: &str) -> Result<Option<DateTime<Utc>>> {
    let text: Option<String> = conn
        .query_row(
            "SELECT updated_at FROM app_settings WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    text.as_deref().map(parse_timestamp).transpose()
}

#[cfg(test)]
mod tests;
