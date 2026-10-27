use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::ModelError;

string_enum! {
    /// Lifecycle of one collection run. Matches `collection_runs.status`.
    pub enum RunStatus {
        Running => "running",
        Completed => "completed",
        Cancelled => "cancelled",
        Failed => "failed",
    }
}

string_enum! {
    /// What happened for one source in one run. These are deliberately four different states: a
    /// source that failed is never reported as "no properties exist".
    pub enum SourceOutcomeKind {
        /// The source returned listings.
        Results => "results",
        /// The source answered completely and had nothing for the request.
        SuccessfulEmpty => "successful_empty",
        /// The source could not be reached, refused, or errored.
        Failed => "failed",
        /// The source answered but its content could not be parsed reliably.
        Unreliable => "unreliable",
    }
}

/// Result of one source in one run, with counts and enough detail to explain itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceOutcome {
    pub adapter_id: String,
    pub kind: SourceOutcomeKind,
    /// `false` for a partial search (cancelled, rate limited, truncated...). A partial result keeps
    /// its warnings and must not be presented as a complete search.
    pub complete: bool,
    pub discovered: u32,
    pub stored: u32,
    pub unparseable: u32,
    pub failed: u32,
    pub skipped: u32,
    pub warnings: Vec<String>,
    pub error_kind: Option<String>,
    pub error_detail: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}

impl SourceOutcome {
    /// The same rules the database enforces, checked early so a bad outcome is a clear error
    /// instead of a constraint failure.
    pub fn validate(&self) -> Result<(), ModelError> {
        let invalid = |why: &str| {
            Err(ModelError::InvalidOutcome(format!(
                "{}: {why}",
                self.adapter_id
            )))
        };
        if self.adapter_id.trim().is_empty() {
            return Err(ModelError::InvalidOutcome("adapter id is empty".to_owned()));
        }
        if self.finished_at < self.started_at {
            return invalid("finished before it started");
        }
        match self.kind {
            SourceOutcomeKind::SuccessfulEmpty if !self.complete || self.discovered != 0 => {
                invalid("\"nothing found\" requires a complete search that discovered nothing")
            }
            SourceOutcomeKind::Failed if self.error_kind.is_none() => {
                invalid("a failure must say what failed")
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn outcome(kind: SourceOutcomeKind) -> SourceOutcome {
        let t = Utc.timestamp_opt(1_780_000_000, 0).unwrap();
        SourceOutcome {
            adapter_id: "fake".into(),
            kind,
            complete: true,
            discovered: 0,
            stored: 0,
            unparseable: 0,
            failed: 0,
            skipped: 0,
            warnings: vec![],
            error_kind: None,
            error_detail: None,
            started_at: t,
            finished_at: t,
        }
    }

    #[test]
    fn enums_round_trip_their_text_form() {
        for s in RunStatus::ALL {
            assert_eq!(s.as_str().parse::<RunStatus>().unwrap(), *s);
        }
        for k in SourceOutcomeKind::ALL {
            assert_eq!(k.as_str().parse::<SourceOutcomeKind>().unwrap(), *k);
        }
        assert!("paused".parse::<RunStatus>().is_err());
    }

    #[test]
    fn a_complete_search_that_found_nothing_is_a_valid_empty_outcome() {
        assert!(
            outcome(SourceOutcomeKind::SuccessfulEmpty)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn nothing_found_needs_a_complete_search_that_discovered_nothing() {
        let mut partial = outcome(SourceOutcomeKind::SuccessfulEmpty);
        partial.complete = false;
        assert!(partial.validate().is_err());
        let mut found = outcome(SourceOutcomeKind::SuccessfulEmpty);
        found.discovered = 3;
        assert!(found.validate().is_err());
    }

    #[test]
    fn a_failure_must_say_what_failed() {
        let mut failed = outcome(SourceOutcomeKind::Failed);
        assert!(failed.validate().is_err());
        failed.error_kind = Some("timeout".into());
        assert!(failed.validate().is_ok());
    }

    #[test]
    fn time_must_not_run_backwards() {
        let mut o = outcome(SourceOutcomeKind::Results);
        o.finished_at = o.started_at - chrono::Duration::seconds(1);
        assert!(o.validate().is_err());
        o.finished_at = o.started_at;
        o.adapter_id = " ".into();
        assert!(o.validate().is_err());
    }
}
