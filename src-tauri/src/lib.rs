pub mod commands;
pub mod db;
pub mod error;
pub mod images;
pub mod matching;
pub mod models;
pub mod parser;
pub mod service;
pub mod sources;

use std::sync::Arc;

use tauri::Manager;

/// Starts the desktop application.
///
/// The database lives in Tauri's per-user app data directory and the image cache in its cache
/// directory (both resolved through the path API, so nothing is hardcoded). Startup migrates the
/// database, seeds the region catalogue and marks runs left `running` by a previous process as
/// failed, all before the window can issue any command.
///
/// # Panics
/// Panics if the Tauri runtime itself fails to start. A database that cannot be opened makes
/// `setup` fail, which aborts startup with the error; presenting that in a dialog is Phase 4.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let cache_dir = app.path().app_cache_dir()?;
            let database = Arc::new(db::Database::open(&db::database_file(&data_dir))?);
            let service = service::AppService::new(
                database,
                sources::registered_adapters(),
                cache_dir,
                Arc::new(chrono::Utc::now),
            );
            service.startup()?;
            app.manage(service);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_status,
            commands::collection::list_sources,
            commands::collection::start_collection,
            commands::collection::cancel_collection,
            commands::collection::is_collection_running,
            commands::collection::list_collection_runs,
            commands::collection::get_collection_run,
            commands::collection::list_source_problems,
            commands::collection::list_regions,
            commands::properties::list_properties,
            commands::properties::set_property_status,
            commands::matches::list_matches,
            commands::matches::get_match,
            commands::matches::list_match_events,
            commands::matches::accept_match,
            commands::matches::reject_match,
            commands::matches::undo_merge,
            commands::matches::run_matching_pass,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
