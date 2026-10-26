//! Typed Tauri commands exposed to the React frontend.
//!
//! Commands stay thin: they translate between the frontend and the domain/db layers and hold no
//! business logic. Phase 1 exposes a single status command, enough to prove that the database
//! was initialised in the app data directory and that the typed bridge works.

use serde::Serialize;
use tauri::State;

use crate::{db::Database, error::Result};

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

// Tauri's `State` extractor is handed to commands by value.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub fn get_app_status(db: State<'_, Database>) -> Result<AppStatus> {
    app_status(&db)
}

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
}
