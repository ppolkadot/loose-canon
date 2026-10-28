//! Ranking weights, thresholds and the user's preferences.
//!
//! The weights are the specification's *initial hypotheses* (price per m² 45 %, cross-listing
//! price difference 25 %, bedroom/suite fit 15 %, freshness 15 %), configurable and stored with
//! the ranking version. Nothing here has been validated against market data.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Settings keys (`app_settings`).
pub const RANKING_CONFIG_KEY: &str = "ranking_config";
pub const SEARCH_PREFERENCES_KEY: &str = "search_preferences";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct RankingConfig {
    pub weight_price_per_m2: f64,
    pub weight_cross_listing: f64,
    pub weight_fit: f64,
    pub weight_freshness: f64,
    /// Fewest comparables a price-per-m² comparison may rest on.
    pub min_comparables: usize,
    /// Comparables needed before the comparison counts as well supported.
    pub solid_comparables: usize,
    /// A price spread of this percent between sources scores 100 (linear below it).
    pub cross_listing_cap_percent: f64,
    /// A listing first seen this many days ago (or more) scores 0 for freshness.
    pub freshness_max_age_days: u32,
    /// A listing last scraped longer ago than this lowers the data confidence.
    pub stale_scrape_days: u32,
}

impl Default for RankingConfig {
    fn default() -> Self {
        Self {
            weight_price_per_m2: 0.45,
            weight_cross_listing: 0.25,
            weight_fit: 0.15,
            weight_freshness: 0.15,
            min_comparables: 5,
            solid_comparables: 10,
            cross_listing_cap_percent: 10.0,
            freshness_max_age_days: 60,
            stale_scrape_days: 14,
        }
    }
}

impl RankingConfig {
    /// Rejects values that would make the ranking meaningless or non-finite.
    pub fn validate(&self) -> Result<()> {
        let weights = [
            self.weight_price_per_m2,
            self.weight_cross_listing,
            self.weight_fit,
            self.weight_freshness,
        ];
        let problem = if weights.iter().any(|w| !w.is_finite() || *w < 0.0) {
            Some("weights must be finite and not negative")
        } else if self.total_weight() <= 0.0 {
            Some("at least one weight must be greater than zero")
        } else if self.min_comparables < 2 {
            Some("min_comparables must be at least 2")
        } else if self.solid_comparables < self.min_comparables {
            Some("solid_comparables must not be below min_comparables")
        } else if !(self.cross_listing_cap_percent > 0.0 && self.cross_listing_cap_percent <= 100.0)
        {
            Some("cross_listing_cap_percent must be within (0, 100]")
        } else if self.freshness_max_age_days == 0 {
            Some("freshness_max_age_days must be at least 1")
        } else if self.stale_scrape_days == 0 {
            Some("stale_scrape_days must be at least 1")
        } else {
            None
        };
        problem.map_or(Ok(()), |detail| {
            Err(Error::InvalidConfig {
                what: "ranking configuration",
                detail: detail.to_owned(),
            })
        })
    }

    #[must_use]
    pub fn total_weight(&self) -> f64 {
        self.weight_price_per_m2
            + self.weight_cross_listing
            + self.weight_fit
            + self.weight_freshness
    }
}

/// What the user is looking for. Every field is optional: a criterion that is not stated is not
/// scored (and a higher bedroom count is never assumed to be better).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct SearchPreferences {
    pub bedrooms_min: Option<u8>,
    pub bedrooms_max: Option<u8>,
    pub suites_min: Option<u8>,
    pub parking_min: Option<u8>,
}

impl SearchPreferences {
    pub fn validate(&self) -> Result<()> {
        // The database allows at most 100 of each (see `properties`), so anything above is noise.
        let counts = [
            self.bedrooms_min,
            self.bedrooms_max,
            self.suites_min,
            self.parking_min,
        ];
        let problem = if counts.iter().flatten().any(|n| *n > 100) {
            Some("a room or parking count cannot exceed 100")
        } else if matches!((self.bedrooms_min, self.bedrooms_max), (Some(min), Some(max)) if min > max)
        {
            Some("bedrooms_min is above bedrooms_max")
        } else {
            None
        };
        problem.map_or(Ok(()), |detail| {
            Err(Error::InvalidConfig {
                what: "search preferences",
                detail: detail.to_owned(),
            })
        })
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.bedrooms_min.is_none()
            && self.bedrooms_max.is_none()
            && self.suites_min.is_none()
            && self.parking_min.is_none()
    }
}

#[cfg(test)]
mod tests;
