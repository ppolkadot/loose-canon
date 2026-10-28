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
    #[error("invalid source outcome: {0}")]
    InvalidOutcome(String),
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
    /// A configuration file (region seed, source configuration...) failed validation.
    #[error("invalid {what}: {detail}")]
    InvalidConfig { what: &'static str, detail: String },
    #[error("unexpected value {value:?} stored in column {column}")]
    InvalidStoredValue { column: &'static str, value: String },
    #[error(
        "price observation at {observed_at} is older than the latest recorded observation ({latest})"
    )]
    OutOfOrderObservation { observed_at: String, latest: String },
    #[error("the database lock was poisoned by a panic in another thread")]
    LockPoisoned,
    /// A collection was requested but no source adapter is registered. Reported as such, never as
    /// "searched and found nothing".
    #[error("no sources are configured, so there is nothing to collect from")]
    NoSourcesConfigured,
    /// A collection is already running; only one runs at a time.
    #[error("a collection is already running")]
    CollectionAlreadyRunning,
    /// A match decision was refused because the stored state does not allow it (the match does
    /// not exist, a merge is still live, the property pointers no longer agree with the record).
    #[error("match operation refused: {0}")]
    MatchRefused(String),
    /// A change was saved but the local scores could not be refreshed afterwards. The change
    /// stands; the scores keep their previous values until the next successful refresh.
    #[error("the change was saved, but the local scores could not be refreshed: {0}")]
    RankingRefreshFailed(String),
    /// The operating system's credential store refused or failed. The message never contains
    /// the secret.
    #[error("credential store error: {0}")]
    SecretStore(String),
    /// An AI prioritisation did not produce a result. The local ranking is unaffected.
    #[error(transparent)]
    Ai(#[from] crate::ai::failure::AiFailure),
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
            Self::InvalidConfig { .. } => "invalid_config",
            Self::InvalidStoredValue { .. } => "invalid_stored_value",
            Self::OutOfOrderObservation { .. } => "out_of_order_observation",
            Self::LockPoisoned => "lock_poisoned",
            Self::NoSourcesConfigured => "no_sources_configured",
            Self::CollectionAlreadyRunning => "collection_already_running",
            Self::MatchRefused(_) => "match_refused",
            Self::RankingRefreshFailed(_) => "ranking_refresh_failed",
            Self::SecretStore(_) => "secret_store",
            Self::Ai(failure) => failure.kind(),
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
