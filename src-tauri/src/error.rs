//! Error types.
//!
//! Three layers, from innermost to outermost:
//! * [`ModelError`]: a domain invariant was violated (negative money, zero area, unknown enum text).
//! * [`ParseError`]: free text could not be turned into an exact value.
//! * [`Error`]: application-level error (database, I/O, migrations) that wraps the other two and
//!   is what Tauri commands return.

use serde::{Serialize, Serializer, ser::SerializeStruct};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    #[error("monetary amount cannot be negative (got {0} centavos)")]
    NegativeMoney(i64),
    #[error("monetary amount exceeds the supported range")]
    MoneyOverflow,
    #[error("area of {centi_m2} hundredths of m² is outside 0 < area <= {max_m2} m²")]
    InvalidArea { centi_m2: u64, max_m2: u32 },
    #[error("unknown {kind} value {value:?}")]
    UnknownVariant { kind: &'static str, value: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("empty input")]
    Empty,
    #[error("unrecognised monetary format: {0:?}")]
    UnrecognizedFormat(String),
    #[error("invalid number: {0:?}")]
    InvalidNumber(String),
    #[error("{0:?} cannot be represented exactly in centavos")]
    TooPrecise(String),
    #[error("amount must be greater than zero")]
    ZeroAmount,
    #[error("amount is outside the plausible range")]
    OutOfRange,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(
        "database schema version {found} is newer than this build supports ({supported}); refusing to open it"
    )]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("migration {version} ({name}) failed: {source}")]
    Migration {
        version: u32,
        name: &'static str,
        source: rusqlite::Error,
    },
    #[error("schema_migrations table disagrees with PRAGMA user_version ({details})")]
    MigrationStateInconsistent { details: String },
    #[error("SQLite foreign-key enforcement could not be enabled on this connection")]
    ForeignKeysUnavailable,
    #[error("unexpected value {value:?} stored in column {column}")]
    InvalidStoredValue { column: &'static str, value: String },
    #[error(
        "price observation at {observed_at} is older than the latest recorded observation ({latest})"
    )]
    OutOfOrderObservation { observed_at: String, latest: String },
    #[error("the database lock was poisoned by a panic in another thread")]
    LockPoisoned,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Short machine-readable discriminator for the frontend.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Database(_) => "database",
            Self::Io(_) => "io",
            Self::Json(_) => "json",
            Self::Model(_) => "model",
            Self::Parse(_) => "parse",
            Self::SchemaTooNew { .. } => "schema_too_new",
            Self::Migration { .. } => "migration",
            Self::MigrationStateInconsistent { .. } => "migration_state_inconsistent",
            Self::ForeignKeysUnavailable => "foreign_keys_unavailable",
            Self::InvalidStoredValue { .. } => "invalid_stored_value",
            Self::OutOfOrderObservation { .. } => "out_of_order_observation",
            Self::LockPoisoned => "lock_poisoned",
        }
    }
}

/// Tauri commands must return a serialisable error; the frontend sees `{ kind, message }`.
impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Error", 2)?;
        state.serialize_field("kind", self.kind())?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}
