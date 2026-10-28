//! Typed Tauri commands exposed to the React frontend.
//!
//! Commands stay thin: they validate and convert arguments, call the service or a repository, and
//! hold no business logic. Everything returned is a DTO serialised in `camelCase` (stored
//! documents such as match evidence and the saved search request keep their stored shape).
//! Mirrored by the typed functions in `src/lib/api.ts`.

pub mod collection;
pub mod matches;
pub mod properties;
pub mod ranking;

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::{db::Database, error::Result, service::AppService};

/// Mirrored by `AppStatus` in `src/lib/api.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    pub app_version: String,
    pub schema_version: u32,
    pub foreign_keys_enabled: bool,
}

pub fn app_status(db: &Database) -> Result<AppStatus> {
    Ok(AppStatus {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        schema_version: db.schema_version()?,
        foreign_keys_enabled: db.foreign_keys_enabled()?,
    })
}

/// A region slug as stored: lower-case letters, digits and hyphens.
pub(crate) fn validate_slug(slug: &str) -> Result<()> {
    let valid = !slug.is_empty()
        && slug.len() <= 100
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(crate::error::Error::InvalidConfig {
            what: "region slug",
            detail: format!("{slug:?} is not a valid region identifier"),
        })
    }
}

// Tauri hands `State` to commands by value.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn get_app_status(service: State<'_, Arc<AppService>>) -> Result<AppStatus> {
    app_status(service.db())
}

#[cfg(test)]
mod contract_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::LATEST_SCHEMA_VERSION;

    #[test]
    fn status_reports_the_real_database_state() {
        let db = Database::open_in_memory().unwrap();
        let status = app_status(&db).unwrap();
        assert_eq!(status.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(status.schema_version, LATEST_SCHEMA_VERSION);
        assert!(status.foreign_keys_enabled);
    }

    #[test]
    fn status_serialises_with_the_camel_case_keys_the_frontend_expects() {
        let json = serde_json::to_value(AppStatus {
            app_version: "1.2.3".into(),
            schema_version: 4,
            foreign_keys_enabled: true,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "appVersion": "1.2.3", "schemaVersion": 4, "foreignKeysEnabled": true })
        );
    }

    #[test]
    fn errors_reach_the_frontend_as_kind_and_message() {
        let err = crate::error::Error::SchemaTooNew {
            found: 9,
            supported: 1,
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "schema_too_new");
        assert!(json["message"].as_str().unwrap().contains("newer"));
    }

    #[test]
    fn every_error_the_commands_can_raise_has_a_distinct_kind() {
        use crate::error::Error;
        let kinds = [
            Error::NoSourcesConfigured.kind(),
            Error::CollectionAlreadyRunning.kind(),
            Error::MatchRefused(String::new()).kind(),
            Error::InvalidConfig {
                what: "x",
                detail: String::new(),
            }
            .kind(),
        ];
        let unique: std::collections::HashSet<_> = kinds.iter().collect();
        assert_eq!(unique.len(), kinds.len());
    }

    #[test]
    fn region_slugs_are_validated_before_they_reach_a_query() {
        for ok in ["goiania", "br-go-setor-bueno", "a1"] {
            validate_slug(ok).unwrap();
        }
        let too_long = "a".repeat(101);
        for bad in [
            "",
            "Goiania",
            "setor bueno",
            "a;drop",
            "../x",
            "ação",
            too_long.as_str(),
        ] {
            assert!(validate_slug(bad).is_err(), "{bad:?} must be refused");
        }
    }
}
