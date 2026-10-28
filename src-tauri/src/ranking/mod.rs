//! The deterministic local ranking: a 0-100 "Pontuação local" with a separate data-confidence
//! indicator and the evidence that explains both. It works with no network, no key and no AI.
//!
//! What it is not: a verdict that a property is a good purchase. A component that cannot be
//! computed is *unavailable* (never zero), the weights are the specification's initial hypotheses,
//! and missing, conflicting or thin data lowers the confidence instead of manufacturing precision.
//! Design: `docs/phase-3-design.md`.

pub mod config;
pub mod load;
pub mod score;
pub mod store;

/// Version of the ranking rules. Stored with every score; bump it when the rules or their default
/// thresholds change so old scores stay interpretable.
pub const RANKING_VERSION: &str = "ranking/1";

use chrono::{DateTime, Utc};
use rusqlite::Connection;

use crate::{db::settings::get_setting_or_default, error::Result};
use config::{RANKING_CONFIG_KEY, RankingConfig, SEARCH_PREFERENCES_KEY, SearchPreferences};
use store::SaveReport;

/// Recomputes and stores every score from what is in the library and the saved settings, at `now`.
/// An invalid saved configuration or preference set is an error and nothing is changed.
pub fn recompute(conn: &mut Connection, now: DateTime<Utc>) -> Result<SaveReport> {
    let config: RankingConfig = get_setting_or_default(conn, RANKING_CONFIG_KEY)?;
    config.validate()?;
    let preferences: SearchPreferences = get_setting_or_default(conn, SEARCH_PREFERENCES_KEY)?;
    preferences.validate()?;
    let inputs = load::load_inputs(conn)?;
    let ranked = score::rank(&inputs, &preferences, &config, now);
    store::save_ranking(conn, &ranked, now)
}

#[cfg(test)]
mod tests;
