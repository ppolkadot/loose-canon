//! The local ranking: the user's search preferences, the weights and thresholds, and a manual
//! refresh. The score itself reaches the frontend inside each `PropertyView`.

// Tauri hands `State` to commands by value.
#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use tauri::State;

use crate::{
    error::Result,
    ranking::{
        config::{RankingConfig, SearchPreferences},
        store::SaveReport,
    },
    service::AppService,
};

/// What the user is looking for; an empty set means no fit criterion is scored.
#[tauri::command]
pub async fn get_search_preferences(
    service: State<'_, Arc<AppService>>,
) -> Result<SearchPreferences> {
    let service = Arc::clone(&service);
    service.offload(AppService::search_preferences).await
}

/// Saves the preferences (refused when they contradict themselves) and refreshes the scores.
#[tauri::command]
pub async fn save_search_preferences(
    service: State<'_, Arc<AppService>>,
    preferences: SearchPreferences,
) -> Result<SaveReport> {
    service.save_search_preferences(preferences).await
}

/// The weights and thresholds in use (the documented defaults until the user changes them).
#[tauri::command]
pub async fn get_ranking_config(service: State<'_, Arc<AppService>>) -> Result<RankingConfig> {
    let service = Arc::clone(&service);
    service.offload(AppService::ranking_config).await
}

/// Saves the weights and thresholds (refused when they make the ranking meaningless) and
/// refreshes every score.
#[tauri::command]
pub async fn save_ranking_config(
    service: State<'_, Arc<AppService>>,
    config: RankingConfig,
) -> Result<SaveReport> {
    service.save_ranking_config(config).await
}

/// Recomputes every score now. Collections and merge decisions already do this on their own.
#[tauri::command]
pub async fn recompute_ranking(service: State<'_, Arc<AppService>>) -> Result<SaveReport> {
    let service = Arc::clone(&service);
    service.offload(AppService::recompute_ranking).await
}
