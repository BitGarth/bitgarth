use crate::db::error::DbError;
use crate::db::sqlite_config::{
    SqliteAutoVacuumMode, load_auto_vacuum_mode, load_freelist_count, load_page_count,
    run_incremental_vacuum,
};
use crate::db::user_db::with_user_db_mut;
use crate::models::{SyncHistoryRetentionDays, UserId};
use crate::wallets::{DigitalAssetAddressId, Network, SyncedAssetId};
use chrono::{DateTime, Duration, Utc};
use rusqlite::params;

use super::ids::{SourceConnectionId, SyncRunId};
use super::shared::{OpaqueJsonText, SyncRunScopeKind, SyncRunStatus, SyncRunTriggerKind};
use super::source_connections::{IntegrationKind, load_active_source_connection_id};

pub(crate) struct StartSyncRunRequest {
    pub(crate) integration: IntegrationKind,
    pub(crate) scope_kind: SyncRunScopeKind,
    pub(crate) scope_address_id: DigitalAssetAddressId,
    pub(crate) asset_id: SyncedAssetId,
    pub(crate) network: Network,
    pub(crate) trigger_kind: SyncRunTriggerKind,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) summary_json: Option<OpaqueJsonText>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartedSyncRun {
    pub(crate) sync_run_id: SyncRunId,
    pub(crate) source_connection_id: SourceConnectionId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompleteSyncRunRequest {
    pub(crate) sync_run_id: SyncRunId,
    pub(crate) status: SyncRunStatus,
    pub(crate) completed_at: DateTime<Utc>,
    pub(crate) summary_json: Option<OpaqueJsonText>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RawSyncHistoryCleanupStats {
    pub(crate) deleted_sync_runs: u32,
    pub(crate) deleted_request_attempts: u32,
    pub(crate) deleted_raw_observation_sets: u32,
    pub(crate) deleted_raw_parse_attempts: u32,
    pub(crate) deleted_raw_mempool_transaction_observations: u32,
    pub(crate) deleted_raw_etherscan_normal_transaction_observations: u32,
    pub(crate) deleted_raw_etherscan_internal_transaction_observations: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RawSyncHistoryCompactionStats {
    pub(crate) auto_vacuum_mode: SqliteAutoVacuumMode,
    pub(crate) freelist_pages_before_cleanup: u32,
    pub(crate) freelist_pages_after_cleanup: u32,
    pub(crate) freelist_pages_after_compaction: u32,
    pub(crate) pages_freed_by_cleanup: u32,
    pub(crate) incremental_vacuum_pages_requested: u32,
    pub(crate) page_count_before_compaction: u32,
    pub(crate) page_count_after_compaction: u32,
    pub(crate) pages_reclaimed_by_compaction: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RawSyncHistoryCleanupReport {
    pub(crate) deletion: RawSyncHistoryCleanupStats,
    pub(crate) compaction: RawSyncHistoryCompactionStats,
}

pub(crate) fn start_sync_run(
    user_id: UserId,
    request: StartSyncRunRequest,
) -> Result<StartedSyncRun, DbError> {
    let sync_run_id = SyncRunId::new();
    with_user_db_mut(user_id, |conn| {
        let source_connection_id = load_active_source_connection_id(
            conn,
            request.integration,
            request.network,
            request.scope_address_id,
        )?;
        let started_at = request.started_at.to_rfc3339();
        conn.execute(
            "INSERT INTO sync_runs
             (id, integration, scope_kind, scope_address_id, source_connection_id, asset_id, network, trigger_kind, status, started_at, completed_at, summary_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                sync_run_id.to_string(),
                request.integration.as_db_value(),
                request.scope_kind.as_db_value(),
                request.scope_address_id.to_string(),
                source_connection_id.to_string(),
                request.asset_id.as_str(),
                request.network.as_str(),
                request.trigger_kind.as_db_value(),
                SyncRunStatus::Started.as_db_value(),
                started_at,
                Option::<String>::None,
                request.summary_json.as_ref().map(OpaqueJsonText::as_str),
                started_at,
                started_at,
            ],
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to insert sync run", err))?;
        Ok(StartedSyncRun {
            sync_run_id,
            source_connection_id,
        })
    })
}

pub(crate) fn complete_sync_run(
    user_id: UserId,
    request: CompleteSyncRunRequest,
) -> Result<(), DbError> {
    if request.status == SyncRunStatus::Started {
        return Err(DbError::new(
            "complete_sync_run cannot set sync run status back to started",
        ));
    }

    with_user_db_mut(user_id, |conn| {
        let changed = conn
            .execute(
                "UPDATE sync_runs
                 SET status = ?1,
                     completed_at = ?2,
                     summary_json = ?3,
                     updated_at = ?4
                 WHERE id = ?5",
                params![
                    request.status.as_db_value(),
                    request.completed_at.to_rfc3339(),
                    request.summary_json.as_ref().map(OpaqueJsonText::as_str),
                    request.completed_at.to_rfc3339(),
                    request.sync_run_id.to_string(),
                ],
            )
            .map_err(|err| DbError::from_rusqlite_error("Failed to complete sync run", err))?;

        if changed != 1 {
            return Err(DbError::new(format!(
                "Failed to complete sync run {}: row missing",
                request.sync_run_id
            )));
        }
        Ok(())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SyncRunRetentionCandidate {
    pub(super) sync_run_id: String,
    pub(super) source_connection_id: String,
    pub(super) status: SyncRunStatus,
    pub(super) age_anchor: DateTime<Utc>,
}

fn parse_sync_run_status(value: &str) -> Result<SyncRunStatus, DbError> {
    match value {
        "started" => Ok(SyncRunStatus::Started),
        "completed_success" => Ok(SyncRunStatus::CompletedSuccess),
        "completed_failure" => Ok(SyncRunStatus::CompletedFailure),
        _ => Err(DbError::new(format!(
            "Invalid sync run status in DB: {value}"
        ))),
    }
}

fn load_sync_run_retention_candidates_tx(
    tx: &rusqlite::Transaction<'_>,
) -> Result<Vec<SyncRunRetentionCandidate>, DbError> {
    let mut stmt = tx
        .prepare(
            "SELECT id, source_connection_id, status, started_at, completed_at
             FROM sync_runs",
        )
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to prepare raw sync history cleanup candidate query",
                err,
            )
        })?;

    stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })
    .map_err(|err| {
        DbError::from_rusqlite_error("Failed to query raw sync history cleanup candidates", err)
    })?
    .map(|row| {
        let (sync_run_id, source_connection_id, status_raw, started_at_raw, completed_at_raw) = row
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to read raw sync history cleanup candidate",
                    err,
                )
            })?;
        let status = parse_sync_run_status(&status_raw)?;
        let started_at = crate::models::parse_datetime(&started_at_raw).map_err(|err| {
            DbError::new(format!(
                "Failed to parse started_at for raw sync history cleanup candidate: {err}"
            ))
        })?;
        let age_anchor = match status {
            SyncRunStatus::Started => started_at,
            SyncRunStatus::CompletedSuccess | SyncRunStatus::CompletedFailure => {
                let completed_at_raw = completed_at_raw.ok_or_else(|| {
                    DbError::new(
                        "Completed raw sync history cleanup candidate missing completed_at",
                    )
                })?;
                crate::models::parse_datetime(&completed_at_raw).map_err(|err| {
                    DbError::new(format!(
                        "Failed to parse completed_at for raw sync history cleanup candidate: {err}"
                    ))
                })?
            }
        };
        Ok(SyncRunRetentionCandidate {
            sync_run_id,
            source_connection_id,
            status,
            age_anchor,
        })
    })
    .collect()
}

pub(super) fn select_prunable_sync_run_ids(
    candidates: &[SyncRunRetentionCandidate],
    successful_cutoff: DateTime<Utc>,
    failure_cutoff: DateTime<Utc>,
) -> Vec<String> {
    candidates
        .iter()
        .filter(|candidate| match candidate.status {
            SyncRunStatus::CompletedSuccess => candidate.age_anchor < successful_cutoff,
            SyncRunStatus::CompletedFailure | SyncRunStatus::Started => {
                candidate.age_anchor < failure_cutoff
            }
        })
        .map(|candidate| candidate.sync_run_id.clone())
        .collect()
}

fn create_cleanup_temp_table_tx(tx: &rusqlite::Transaction<'_>) -> Result<(), DbError> {
    tx.execute(
        "CREATE TEMP TABLE IF NOT EXISTS raw_sync_history_prunable_sync_runs (
            sync_run_id TEXT PRIMARY KEY
        )",
        [],
    )
    .map_err(|err| {
        DbError::from_rusqlite_error("Failed to create raw sync history cleanup temp table", err)
    })?;
    tx.execute("DELETE FROM raw_sync_history_prunable_sync_runs", [])
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to clear raw sync history cleanup temp table", err)
        })?;
    Ok(())
}

fn stage_prunable_sync_run_ids_tx(
    tx: &rusqlite::Transaction<'_>,
    sync_run_ids: &[String],
) -> Result<(), DbError> {
    create_cleanup_temp_table_tx(tx)?;

    {
        let mut insert = tx
            .prepare("INSERT INTO raw_sync_history_prunable_sync_runs (sync_run_id) VALUES (?1)")
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to prepare raw sync history cleanup staging insert",
                    err,
                )
            })?;

        for sync_run_id in sync_run_ids {
            insert.execute(params![sync_run_id]).map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to stage raw sync history cleanup sync run",
                    err,
                )
            })?;
        }
    }

    Ok(())
}

fn count_prunable_rows_tx(
    tx: &rusqlite::Transaction<'_>,
    sql: &str,
    context: &'static str,
) -> Result<u32, DbError> {
    let count: i64 = tx
        .query_row(sql, [], |row| row.get(0))
        .map_err(|err| DbError::from_rusqlite_error(context, err))?;
    u32::try_from(count).map_err(|_| DbError::new(format!("{context}: count out of range")))
}

fn clear_cleanup_temp_table_tx(tx: &rusqlite::Transaction<'_>) -> Result<(), DbError> {
    tx.execute("DELETE FROM raw_sync_history_prunable_sync_runs", [])
        .map_err(|err| {
            DbError::from_rusqlite_error(
                "Failed to clear raw sync history cleanup temp table after delete",
                err,
            )
        })?;
    Ok(())
}

pub(super) fn incremental_vacuum_pages_to_request(
    auto_vacuum_mode: SqliteAutoVacuumMode,
    pages_freed_by_cleanup: u32,
) -> u32 {
    if auto_vacuum_mode.supports_incremental_vacuum() {
        pages_freed_by_cleanup
    } else {
        0
    }
}

fn load_compaction_snapshot(
    conn: &rusqlite::Connection,
    freelist_context: &'static str,
    page_count_context: &'static str,
) -> Result<(u32, u32), DbError> {
    let freelist_count = load_freelist_count(conn, freelist_context)?;
    let page_count = load_page_count(conn, page_count_context)?;
    Ok((freelist_count, page_count))
}

fn finalize_cleanup_report(
    conn: &rusqlite::Connection,
    deletion: RawSyncHistoryCleanupStats,
    auto_vacuum_mode: SqliteAutoVacuumMode,
    freelist_pages_before_cleanup: u32,
) -> Result<RawSyncHistoryCleanupReport, DbError> {
    let (freelist_pages_after_cleanup, page_count_before_compaction) = load_compaction_snapshot(
        conn,
        "Failed to load freelist count after raw sync history cleanup",
        "Failed to load page count before raw sync history compaction",
    )?;
    let pages_freed_by_cleanup =
        freelist_pages_after_cleanup.saturating_sub(freelist_pages_before_cleanup);
    let incremental_vacuum_pages_requested =
        incremental_vacuum_pages_to_request(auto_vacuum_mode, pages_freed_by_cleanup);

    if incremental_vacuum_pages_requested == 0 {
        return Ok(RawSyncHistoryCleanupReport {
            deletion,
            compaction: RawSyncHistoryCompactionStats {
                auto_vacuum_mode,
                freelist_pages_before_cleanup,
                freelist_pages_after_cleanup,
                freelist_pages_after_compaction: freelist_pages_after_cleanup,
                pages_freed_by_cleanup,
                incremental_vacuum_pages_requested,
                page_count_before_compaction,
                page_count_after_compaction: page_count_before_compaction,
                pages_reclaimed_by_compaction: 0,
            },
        });
    }

    run_incremental_vacuum(
        conn,
        incremental_vacuum_pages_requested,
        "Failed to run incremental vacuum after raw sync history cleanup",
    )?;

    let (freelist_pages_after_compaction, page_count_after_compaction) = load_compaction_snapshot(
        conn,
        "Failed to load freelist count after raw sync history compaction",
        "Failed to load page count after raw sync history compaction",
    )?;

    Ok(RawSyncHistoryCleanupReport {
        deletion,
        compaction: RawSyncHistoryCompactionStats {
            auto_vacuum_mode,
            freelist_pages_before_cleanup,
            freelist_pages_after_cleanup,
            freelist_pages_after_compaction,
            pages_freed_by_cleanup,
            incremental_vacuum_pages_requested,
            page_count_before_compaction,
            page_count_after_compaction,
            pages_reclaimed_by_compaction: page_count_before_compaction
                .saturating_sub(page_count_after_compaction),
        },
    })
}

pub(crate) fn cleanup_raw_sync_history_with_compaction(
    user_id: UserId,
    completed_at: DateTime<Utc>,
    retention_days: SyncHistoryRetentionDays,
) -> Result<RawSyncHistoryCleanupReport, DbError> {
    let successful_cutoff = completed_at - Duration::days(i64::from(retention_days.value()));
    let failure_cutoff = completed_at - Duration::hours(24);

    with_user_db_mut(user_id, |conn| {
        let auto_vacuum_mode = load_auto_vacuum_mode(
            conn,
            "Failed to detect SQLite auto_vacuum mode for raw sync history cleanup",
        )?;
        let freelist_pages_before_cleanup = load_freelist_count(
            conn,
            "Failed to load freelist count before raw sync history cleanup",
        )?;

        let deletion = {
            let tx = conn.transaction().map_err(|err| {
                DbError::new(format!("Failed to start raw sync history cleanup: {err}"))
            })?;

            let candidates = load_sync_run_retention_candidates_tx(&tx)?;
            let prunable_sync_run_ids =
                select_prunable_sync_run_ids(&candidates, successful_cutoff, failure_cutoff);
            if prunable_sync_run_ids.is_empty() {
                tx.commit().map_err(|err| {
                    DbError::new(format!(
                        "Failed to commit empty raw sync history cleanup: {err}"
                    ))
                })?;
                RawSyncHistoryCleanupStats::default()
            } else {
                stage_prunable_sync_run_ids_tx(&tx, &prunable_sync_run_ids)?;

                let stats = RawSyncHistoryCleanupStats {
                    deleted_sync_runs: u32::try_from(prunable_sync_run_ids.len())
                        .unwrap_or(u32::MAX),
                    deleted_request_attempts: count_prunable_rows_tx(
                        &tx,
                        "SELECT COUNT(*) FROM request_attempts
                         WHERE sync_run_id IN (
                             SELECT sync_run_id FROM raw_sync_history_prunable_sync_runs
                         )",
                        "Failed to count prunable request attempts",
                    )?,
                    deleted_raw_observation_sets: count_prunable_rows_tx(
                        &tx,
                        "SELECT COUNT(*) FROM raw_observation_sets
                         WHERE sync_run_id IN (
                             SELECT sync_run_id FROM raw_sync_history_prunable_sync_runs
                         )",
                        "Failed to count prunable raw observation sets",
                    )?,
                    deleted_raw_parse_attempts: count_prunable_rows_tx(
                        &tx,
                        "SELECT COUNT(*) FROM raw_parse_attempts
                         WHERE sync_run_id IN (
                             SELECT sync_run_id FROM raw_sync_history_prunable_sync_runs
                         )",
                        "Failed to count prunable raw parse attempts",
                    )?,
                    deleted_raw_mempool_transaction_observations: count_prunable_rows_tx(
                        &tx,
                        "SELECT COUNT(*) FROM raw_mempool_transaction_observations
                         WHERE sync_run_id IN (
                             SELECT sync_run_id FROM raw_sync_history_prunable_sync_runs
                         )",
                        "Failed to count prunable raw mempool transaction observations",
                    )?,
                    deleted_raw_etherscan_normal_transaction_observations: count_prunable_rows_tx(
                        &tx,
                        "SELECT COUNT(*) FROM raw_etherscan_normal_transaction_observations
                         WHERE sync_run_id IN (
                             SELECT sync_run_id FROM raw_sync_history_prunable_sync_runs
                         )",
                        "Failed to count prunable raw etherscan normal observations",
                    )?,
                    deleted_raw_etherscan_internal_transaction_observations:
                        count_prunable_rows_tx(
                            &tx,
                            "SELECT COUNT(*) FROM raw_etherscan_internal_transaction_observations
                         WHERE sync_run_id IN (
                             SELECT sync_run_id FROM raw_sync_history_prunable_sync_runs
                         )",
                            "Failed to count prunable raw etherscan internal observations",
                        )?,
                };

                tx.execute(
                    "DELETE FROM sync_runs
                     WHERE id IN (
                         SELECT sync_run_id FROM raw_sync_history_prunable_sync_runs
                     )",
                    [],
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error("Failed to delete prunable sync runs", err)
                })?;

                clear_cleanup_temp_table_tx(&tx)?;

                tx.commit().map_err(|err| {
                    DbError::new(format!("Failed to commit raw sync history cleanup: {err}"))
                })?;
                stats
            }
        };

        finalize_cleanup_report(
            conn,
            deletion,
            auto_vacuum_mode,
            freelist_pages_before_cleanup,
        )
    })
}

#[cfg(all(test, feature = "db-tests"))]
pub(super) fn cleanup_raw_sync_history(
    user_id: UserId,
    completed_at: DateTime<Utc>,
    retention_days: SyncHistoryRetentionDays,
) -> Result<RawSyncHistoryCleanupStats, DbError> {
    cleanup_raw_sync_history_with_compaction(user_id, completed_at, retention_days)
        .map(|report| report.deletion)
}
