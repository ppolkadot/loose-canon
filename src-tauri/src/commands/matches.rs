//! Match candidates: reviewing them, merging, rejecting and undoing. Every merge is a user
//! decision; nothing in the application merges on its own.

// Tauri hands `State` to commands by value.
#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use serde::Deserialize;
use tauri::State;

use crate::{
    db::{
        matches::{
            self as store, AcceptOutcome, MatchQuery, MatchView, MergeEventRecord, RejectOutcome,
            UndoOutcome,
        },
        properties::Page,
    },
    error::{Error, Result},
    matching::run::MatchRunReport,
    models::matching::DecisionStatus,
    service::AppService,
};

/// Longest note a user may attach to a decision.
const MAX_NOTE_CHARS: usize = 500;

/// Which candidates to list. Mirrored by `ListMatchesArgs` in `src/lib/api.ts`. The default is
/// every stored pair; the review queue is `status: "review_required"` with `actionableOnly`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ListMatchesArgs {
    pub status: Option<DecisionStatus>,
    /// Only pairs the user can still act on (the two listings are not yet one property).
    pub actionable_only: bool,
    pub limit: u32,
    pub offset: u32,
}

impl From<ListMatchesArgs> for MatchQuery {
    fn from(args: ListMatchesArgs) -> Self {
        Self {
            status: args.status,
            actionable_only: args.actionable_only,
            limit: args.limit,
            offset: args.offset,
        }
    }
}

/// Trims a note; an empty one is no note; an overlong one is refused rather than cut.
pub fn clean_note(note: Option<String>) -> Result<Option<String>> {
    let Some(note) = note else {
        return Ok(None);
    };
    let note = note.trim();
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(Error::MatchRefused(format!(
            "a note is limited to {MAX_NOTE_CHARS} characters"
        )));
    }
    Ok((!note.is_empty()).then(|| note.to_owned()))
}

#[tauri::command]
pub async fn list_matches(
    service: State<'_, Arc<AppService>>,
    args: ListMatchesArgs,
) -> Result<Page<MatchView>> {
    let query = MatchQuery::from(args);
    service
        .blocking(move |conn| store::list_matches(conn, &query))
        .await
}

#[tauri::command]
pub async fn get_match(
    service: State<'_, Arc<AppService>>,
    match_id: i64,
) -> Result<Option<MatchView>> {
    service
        .blocking(move |conn| store::get_match(conn, match_id))
        .await
}

/// The audit trail of one pair: every merge and undo, oldest first.
#[tauri::command]
pub async fn list_match_events(
    service: State<'_, Arc<AppService>>,
    match_id: i64,
) -> Result<Vec<MergeEventRecord>> {
    service
        .blocking(move |conn| store::merge_events(conn, match_id))
        .await
}

/// Merges the two properties and refreshes the scores. Repeating it changes nothing.
#[tauri::command]
pub async fn accept_match(
    service: State<'_, Arc<AppService>>,
    match_id: i64,
    note: Option<String>,
) -> Result<AcceptOutcome> {
    let note = clean_note(note)?;
    let at = chrono::Utc::now();
    service
        .blocking_then_rerank(move |conn| store::accept_match(conn, match_id, note.as_deref(), at))
        .await
}

/// Keeps the two apart. A merged pair must be undone first.
#[tauri::command]
pub async fn reject_match(
    service: State<'_, Arc<AppService>>,
    match_id: i64,
    note: Option<String>,
) -> Result<RejectOutcome> {
    let note = clean_note(note)?;
    let at = chrono::Utc::now();
    service
        .blocking(move |conn| store::reject_match(conn, match_id, note.as_deref(), at))
        .await
}

/// Reverses a merge (each side keeps the status it had) and refreshes the scores.
#[tauri::command]
pub async fn undo_merge(
    service: State<'_, Arc<AppService>>,
    match_id: i64,
    note: Option<String>,
) -> Result<UndoOutcome> {
    let note = clean_note(note)?;
    let at = chrono::Utc::now();
    service
        .blocking_then_rerank(move |conn| store::undo_merge(conn, match_id, note.as_deref(), at))
        .await
}

/// Looks for duplicate candidates now. It only proposes; it never merges.
#[tauri::command]
pub async fn run_matching_pass(service: State<'_, Arc<AppService>>) -> Result<MatchRunReport> {
    service.run_matching_pass().await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_default_lists_every_stored_pair() {
        let query = MatchQuery::from(serde_json::from_str::<ListMatchesArgs>("{}").unwrap());
        assert_eq!(
            (
                query.status,
                query.actionable_only,
                query.limit,
                query.offset
            ),
            (None, false, 0, 0)
        );
        assert_eq!(query.effective_limit(), MatchQuery::DEFAULT_LIMIT);
    }

    #[test]
    fn the_review_queue_is_read_from_its_stored_text_form() {
        let args: ListMatchesArgs = serde_json::from_str(
            r#"{ "status": "review_required", "actionableOnly": true, "limit": 10, "offset": 20 }"#,
        )
        .unwrap();
        let query = MatchQuery::from(args);
        assert_eq!(query.status, Some(DecisionStatus::ReviewRequired));
        assert!(query.actionable_only);
        assert_eq!((query.limit, query.offset), (10, 20));
        assert!(serde_json::from_str::<ListMatchesArgs>(r#"{ "status": "merged" }"#).is_err());
        assert!(
            serde_json::from_str::<ListMatchesArgs>(r#"{ "state": "accepted" }"#).is_err(),
            "unknown field"
        );
    }

    #[test]
    fn notes_are_trimmed_and_an_empty_note_is_no_note() {
        assert_eq!(clean_note(None).unwrap(), None);
        assert_eq!(clean_note(Some("   ".into())).unwrap(), None);
        assert_eq!(
            clean_note(Some("  mesma casa  ".into()))
                .unwrap()
                .as_deref(),
            Some("mesma casa")
        );
    }

    #[test]
    fn an_overlong_note_is_refused_not_cut() {
        let at_limit = "a".repeat(MAX_NOTE_CHARS);
        assert_eq!(
            clean_note(Some(at_limit.clone())).unwrap().as_deref(),
            Some(at_limit.as_str())
        );
        let error = clean_note(Some("a".repeat(MAX_NOTE_CHARS + 1))).unwrap_err();
        assert!(matches!(error, Error::MatchRefused(_)), "{error}");
        // Characters, not bytes: accented text is not penalised.
        assert!(clean_note(Some("ã".repeat(MAX_NOTE_CHARS))).is_ok());
    }

    #[test]
    fn decision_outcomes_reach_the_frontend_as_tagged_camel_case() {
        assert_eq!(
            serde_json::to_value(AcceptOutcome::Merged {
                surviving: 3,
                absorbed: 9
            })
            .unwrap(),
            json!({ "kind": "merged", "surviving": 3, "absorbed": 9 })
        );
        assert_eq!(
            serde_json::to_value(AcceptOutcome::AlreadyMerged).unwrap(),
            json!({ "kind": "already_merged" })
        );
        assert_eq!(
            serde_json::to_value(AcceptOutcome::AlreadySameGroup).unwrap(),
            json!({ "kind": "already_same_group" })
        );
        assert_eq!(
            serde_json::to_value(RejectOutcome::Rejected).unwrap(),
            json!({ "kind": "rejected" })
        );
        assert_eq!(
            serde_json::to_value(RejectOutcome::AlreadyRejected).unwrap(),
            json!({ "kind": "already_rejected" })
        );
        assert_eq!(
            serde_json::to_value(UndoOutcome::Reverted {
                surviving: 1,
                absorbed: 2
            })
            .unwrap(),
            json!({ "kind": "reverted", "surviving": 1, "absorbed": 2 })
        );
        assert_eq!(
            serde_json::to_value(UndoOutcome::AlreadyReverted).unwrap(),
            json!({ "kind": "already_reverted" })
        );
    }
}
