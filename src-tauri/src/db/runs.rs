//! Collection runs and the per-source outcomes that make source health visible.
//!
//! Nothing here is ever deleted: the history of what each source did is the evidence behind
//! "this source failed" versus "this source had nothing", and the two must never be confused.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use super::{format_timestamp, parse_timestamp};
use crate::{
    error::{Error, ModelError, Result},
    models::collection::{RunStatus, SourceOutcome},
};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: i64,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: RunStatus,
    pub request: serde_json::Value,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredOutcome {
    pub id: i64,
    pub run_id: i64,
    pub outcome: SourceOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunSummary {
    pub run: RunRecord,
    pub outcomes: Vec<StoredOutcome>,
}

pub fn start_run(conn: &Connection, request: &serde_json::Value, at: DateTime<Utc>) -> Result<i64> {
    conn.execute(
        "INSERT INTO collection_runs (started_at, request_json) VALUES (?1, ?2)",
        params![format_timestamp(at), serde_json::to_string(request)?],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Ends a run. Returns `false` when it was not running (already finished, or unknown), leaving it
/// untouched. `status` must not be `Running`.
pub fn finish_run(
    conn: &Connection,
    run_id: i64,
    status: RunStatus,
    at: DateTime<Utc>,
    note: Option<&str>,
) -> Result<bool> {
    if status == RunStatus::Running {
        return Err(
            ModelError::InvalidOutcome("a run cannot be finished as running".to_owned()).into(),
        );
    }
    let changed = conn.execute(
        "UPDATE collection_runs SET status = ?1, finished_at = MAX(started_at, ?2), note = ?3
          WHERE id = ?4 AND status = 'running'",
        params![status.as_str(), format_timestamp(at), note, run_id],
    )?;
    Ok(changed == 1)
}

/// Records one source's outcome for a run that is still running.
pub fn record_outcome(conn: &Connection, run_id: i64, outcome: &SourceOutcome) -> Result<i64> {
    outcome.validate()?;
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM collection_runs WHERE id = ?1",
            [run_id],
            |r| r.get(0),
        )
        .optional()?;
    match status.as_deref() {
        Some("running") => {}
        Some(other) => {
            return Err(
                ModelError::InvalidOutcome(format!("run {run_id} is already {other}")).into(),
            );
        }
        None => {
            return Err(ModelError::InvalidOutcome(format!("run {run_id} does not exist")).into());
        }
    }
    conn.execute(
        "INSERT INTO source_run_outcomes
            (run_id, adapter_id, outcome, complete, discovered_count, stored_count, unparseable_count,
             failed_count, skipped_count, warnings_json, error_kind, error_detail, started_at, finished_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            run_id,
            outcome.adapter_id,
            outcome.kind.as_str(),
            i64::from(outcome.complete),
            outcome.discovered,
            outcome.stored,
            outcome.unparseable,
            outcome.failed,
            outcome.skipped,
            serde_json::to_string(&outcome.warnings)?,
            outcome.error_kind,
            outcome.error_detail,
            format_timestamp(outcome.started_at),
            format_timestamp(outcome.finished_at),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

const RUN_COLUMNS: &str = "id, started_at, finished_at, status, request_json, note";

type RawRun = (i64, String, Option<String>, String, String, Option<String>);

fn run_from_row(r: &Row<'_>) -> rusqlite::Result<RawRun> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
    ))
}

fn run_from_raw(raw: RawRun) -> Result<RunRecord> {
    let (id, started_at, finished_at, status, request, note) = raw;
    Ok(RunRecord {
        id,
        started_at: parse_timestamp(&started_at)?,
        finished_at: finished_at.as_deref().map(parse_timestamp).transpose()?,
        status: status.parse().map_err(|_| Error::InvalidStoredValue {
            column: "collection_runs.status",
            value: status.clone(),
        })?,
        request: serde_json::from_str(&request)?,
        note,
    })
}

const OUTCOME_COLUMNS: &str =
    "id, run_id, adapter_id, outcome, complete, discovered_count, stored_count,
    unparseable_count, failed_count, skipped_count, warnings_json, error_kind, error_detail,
    started_at, finished_at";

type RawOutcome = (
    i64,
    i64,
    String,
    String,
    i64,
    u32,
    u32,
    u32,
    u32,
    u32,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
);

fn outcome_from_row(r: &Row<'_>) -> rusqlite::Result<RawOutcome> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
        r.get(14)?,
    ))
}

fn outcome_from_raw(raw: RawOutcome) -> Result<StoredOutcome> {
    let (
        id,
        run_id,
        adapter_id,
        kind,
        complete,
        discovered,
        stored,
        unparseable,
        failed,
        skipped,
        warnings,
        error_kind,
        error_detail,
        started,
        finished,
    ) = raw;
    Ok(StoredOutcome {
        id,
        run_id,
        outcome: SourceOutcome {
            adapter_id,
            kind: kind.parse().map_err(|_| Error::InvalidStoredValue {
                column: "source_run_outcomes.outcome",
                value: kind.clone(),
            })?,
            complete: complete == 1,
            discovered,
            stored,
            unparseable,
            failed,
            skipped,
            warnings: serde_json::from_str(&warnings)?,
            error_kind,
            error_detail,
            started_at: parse_timestamp(&started)?,
            finished_at: parse_timestamp(&finished)?,
        },
    })
}

fn query_outcomes(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<StoredOutcome>> {
    let mut statement = conn.prepare(sql)?;
    let raws = statement
        .query_map(params, outcome_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    raws.into_iter().map(outcome_from_raw).collect()
}

pub fn get_run(conn: &Connection, run_id: i64) -> Result<Option<RunSummary>> {
    let raw = conn
        .query_row(
            &format!("SELECT {RUN_COLUMNS} FROM collection_runs WHERE id = ?1"),
            [run_id],
            run_from_row,
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let outcomes = query_outcomes(
        conn,
        &format!(
            "SELECT {OUTCOME_COLUMNS} FROM source_run_outcomes WHERE run_id = ?1 ORDER BY adapter_id"
        ),
        [run_id],
    )?;
    Ok(Some(RunSummary {
        run: run_from_raw(raw)?,
        outcomes,
    }))
}

/// Newest first.
pub fn list_runs(conn: &Connection, limit: u32) -> Result<Vec<RunRecord>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {RUN_COLUMNS} FROM collection_runs ORDER BY started_at DESC, id DESC LIMIT ?1"
    ))?;
    let raws = statement
        .query_map([limit], run_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    raws.into_iter().map(run_from_raw).collect()
}

/// The most recent outcome of every source that has ever run, ordered by source.
pub fn latest_outcomes_by_source(conn: &Connection) -> Result<Vec<StoredOutcome>> {
    query_outcomes(
        conn,
        &format!(
            "SELECT {OUTCOME_COLUMNS} FROM source_run_outcomes
              WHERE id IN (SELECT MAX(id) FROM source_run_outcomes GROUP BY adapter_id)
              ORDER BY adapter_id"
        ),
        [],
    )
}

/// Failed, unreliable or incomplete outcomes, newest first.
pub fn recent_problems(conn: &Connection, limit: u32) -> Result<Vec<StoredOutcome>> {
    query_outcomes(
        conn,
        &format!(
            "SELECT {OUTCOME_COLUMNS} FROM source_run_outcomes
              WHERE outcome IN ('failed', 'unreliable') OR complete = 0
              ORDER BY finished_at DESC, id DESC LIMIT ?1"
        ),
        [limit],
    )
}

/// Marks runs still `running` as failed. Call at startup: a run cannot be running when no process
/// is executing it, and a stale "running" row would otherwise live forever.
pub fn fail_interrupted_runs(conn: &Connection, at: DateTime<Utc>) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE collection_runs
            SET status = 'failed', finished_at = MAX(started_at, ?1),
                note = 'interrupted: the application stopped before this run finished'
          WHERE status = 'running'",
        [format_timestamp(at)],
    )?)
}

#[cfg(test)]
mod tests;
