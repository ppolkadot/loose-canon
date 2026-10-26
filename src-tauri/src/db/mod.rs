//! SQLite persistence.
//!
//! # Schema versioning and migrations
//!
//! The schema version is SQLite's `PRAGMA user_version`. Every migration is a numbered SQL
//! script in [`MIGRATIONS`]; version 1 is `schema.sql`. On open, every migration newer than the
//! database's version is applied **in order, each inside its own transaction** that also bumps
//! `user_version` and appends a row to `schema_migrations`, so a failure leaves the database at
//! the previous good version. Properties of this scheme:
//!
//! * Safe to repeat: an up-to-date database is not touched.
//! * Never destructive: migrations use plain `CREATE` (no `IF NOT EXISTS`, no `DROP`), so
//!   running `schema.sql` against a database that already has the tables fails loudly instead
//!   of silently merging with or replacing existing data.
//! * Forward-only: a database whose version is newer than this build knows is refused
//!   ([`Error::SchemaTooNew`]) instead of being opened and possibly corrupted.
//! * Auditable: `schema_migrations` must agree with `user_version`, otherwise open fails.
//!
//! To change the schema, **append** a migration (`0002_*.sql`, version 2...) and never edit one
//! that has shipped. Migrations that rebuild a table must account for foreign keys being
//! enforced (SQLite cannot toggle them inside a transaction).

pub mod price_history;

use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, params};

use crate::error::{Error, Result};

/// File name of the application database inside Tauri's app data directory.
pub const DATABASE_FILE_NAME: &str = "loose_canon.sqlite3";

pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Ordered, contiguous, starting at 1. Append only.
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial_schema",
    sql: include_str!("schema.sql"),
}];

/// Schema version this build creates and understands.
pub const LATEST_SCHEMA_VERSION: u32 = MIGRATIONS[MIGRATIONS.len() - 1].version;

/// `dir` is expected to come from Tauri's path API (`app_data_dir`); nothing here hardcodes a
/// machine-specific location.
#[must_use]
pub fn database_file(dir: &Path) -> PathBuf {
    dir.join(DATABASE_FILE_NAME)
}

/// Timestamps are stored as UTC RFC 3339 with millisecond precision (`2026-01-31T12:00:00.000Z`),
/// which sorts lexicographically in chronological order.
#[must_use]
pub fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn parse_timestamp(text: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| Error::InvalidStoredValue {
            column: "timestamp",
            value: text.to_owned(),
        })
}

/// Applies the per-connection settings the schema relies on and **verifies** that SQLite
/// really enforces foreign keys. `file_backed` additionally enables WAL.
pub fn configure(conn: &Connection, file_backed: bool) -> Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    if !foreign_keys_enabled(conn)? {
        return Err(Error::ForeignKeysUnavailable);
    }
    if file_backed {
        // WAL lets readers proceed while a collection run writes. If the filesystem cannot do
        // WAL SQLite keeps its current mode, which is slower but still correct, so the
        // returned mode is deliberately not treated as an error.
        let _mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    }
    Ok(())
}

pub fn foreign_keys_enabled(conn: &Connection) -> Result<bool> {
    let flag: i64 = conn.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    Ok(flag == 1)
}

pub fn schema_version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Brings `conn` up to [`LATEST_SCHEMA_VERSION`].
pub fn migrate(conn: &mut Connection) -> Result<()> {
    migrate_with(conn, MIGRATIONS)
}

fn migrate_with(conn: &mut Connection, migrations: &[Migration]) -> Result<()> {
    let latest = migrations.last().map_or(0, |m| m.version);
    let current = schema_version(conn)?;
    if current > latest {
        return Err(Error::SchemaTooNew {
            found: current,
            supported: latest,
        });
    }
    if current > 0 {
        check_bookkeeping(conn, current)?;
    }
    for migration in migrations.iter().filter(|m| m.version > current) {
        let failed = |source| Error::Migration {
            version: migration.version,
            name: migration.name,
            source,
        };
        // Dropping `tx` on any error path rolls the whole migration back.
        let tx = conn.transaction()?;
        tx.execute_batch(migration.sql).map_err(failed)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            params![
                migration.version,
                migration.name,
                format_timestamp(Utc::now())
            ],
        )
        .map_err(failed)?;
        tx.pragma_update(None, "user_version", migration.version)
            .map_err(failed)?;
        tx.commit().map_err(failed)?;
    }
    Ok(())
}

/// `schema_migrations` must say the same thing as `PRAGMA user_version`.
fn check_bookkeeping(conn: &Connection, current: u32) -> Result<()> {
    let recorded = conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
        row.get::<_, Option<u32>>(0)
    });
    match recorded {
        Ok(Some(version)) if version == current => Ok(()),
        Ok(other) => Err(Error::MigrationStateInconsistent {
            details: format!("user_version is {current} but schema_migrations records {other:?}"),
        }),
        Err(e) => Err(Error::MigrationStateInconsistent {
            details: format!("user_version is {current} but schema_migrations is unreadable: {e}"),
        }),
    }
}

/// The application database: one SQLite connection behind a mutex.
///
/// A single connection keeps Phase 1 simple and correct (SQLite serialises writers anyway).
/// Long-running collection work in Phase 2 should hold the lock only for short transactions;
/// if read contention ever shows up in measurements, add a read-only connection pool then.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Opens (creating the file and its parent directories if needed) and migrates the database.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(path)?;
        configure(&conn, true)?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// In-memory database, migrated. Used by tests.
    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn, false)?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| Error::LockPoisoned)
    }

    /// Runs `f` with exclusive access to the connection.
    pub fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.lock()?;
        f(&mut guard)
    }

    pub fn schema_version(&self) -> Result<u32> {
        self.with_conn(|c| schema_version(c))
    }

    pub fn foreign_keys_enabled(&self) -> Result<bool> {
        self.with_conn(|c| foreign_keys_enabled(c))
    }
}

#[cfg(test)]
mod tests;
