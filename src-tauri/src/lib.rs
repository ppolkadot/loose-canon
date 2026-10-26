pub mod commands;
pub mod db;
pub mod error;
pub mod models;
pub mod parser;

use tauri::Manager;

/// Starts the desktop application.
///
/// The database lives in Tauri's per-user app data directory (resolved through the path API,
/// so nothing is hardcoded) and is migrated before the window can issue any command.
///
/// # Panics
/// Panics if the Tauri runtime itself fails to start. A database that cannot be opened makes
/// `setup` fail, which aborts startup with the error; presenting that in a dialog is Phase 4.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let database = db::Database::open(&db::database_file(&data_dir))?;
            app.manage(database);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::get_app_status])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
