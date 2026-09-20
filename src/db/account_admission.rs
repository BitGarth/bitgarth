use super::error::DbError;
use crate::payments::types::EntitlementTier;
use crate::wallets::DigitalAssetAccountId;
use chrono::{DateTime, SecondsFormat, TimeDelta, Timelike, Utc};
use rusqlite::params;

pub(crate) fn format_admission_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub(crate) fn next_admission_timestamp(
    now: DateTime<Utc>,
    previous: Option<DateTime<Utc>>,
) -> Result<DateTime<Utc>, DbError> {
    let truncate = |value: DateTime<Utc>| {
        value
            .with_nanosecond(value.nanosecond() / 1_000 * 1_000)
            .ok_or_else(|| DbError::new("Invalid admission timestamp"))
    };
    let now = truncate(now)?;
    match previous {
        Some(previous) => {
            let next = truncate(previous)?
                .checked_add_signed(TimeDelta::microseconds(1))
                .ok_or_else(|| DbError::new("Admission timestamp overflow"))?;
            Ok(now.max(next))
        }
        None => Ok(now),
    }
}

fn parse_admission_timestamp(raw: &str) -> Result<DateTime<Utc>, DbError> {
    raw.parse()
        .map_err(|error| DbError::new(format!("Invalid account admission timestamp: {error}")))
}

fn last_admission_timestamp(
    tx: &rusqlite::Transaction<'_>,
    sql: &'static str,
) -> Result<Option<DateTime<Utc>>, DbError> {
    let raw: Option<String> = tx
        .query_row(sql, [], |row| row.get(0))
        .map_err(|error| DbError::new(format!("Failed to load last admission: {error}")))?;
    raw.as_deref().map(parse_admission_timestamp).transpose()
}

pub(crate) fn enroll_native_account_in_tx(
    tx: &rusqlite::Transaction<'_>,
    account_id: DigitalAssetAccountId,
    now: DateTime<Utc>,
    tier: &EntitlementTier,
) -> Result<(), DbError> {
    let previous = last_admission_timestamp(tx, "SELECT MAX(selected_at) FROM account_sync_slots")?;
    let admitted_at = format_admission_timestamp(next_admission_timestamp(now, previous)?);
    tx.execute(
        "INSERT INTO account_sync_slots (account_id, selected_at, selected_under_tier)
         SELECT id, ?2, ?3 FROM digital_asset_accounts WHERE id = ?1
         ON CONFLICT(account_id) DO NOTHING",
        params![account_id.to_string(), admitted_at, tier.as_str()],
    )
    .map_err(|error| DbError::new(format!("Failed to enroll native account: {error}")))?;
    Ok(())
}

pub(crate) fn next_manual_admission_timestamp_in_tx(
    tx: &rusqlite::Transaction<'_>,
    now: DateTime<Utc>,
) -> Result<String, DbError> {
    let previous =
        last_admission_timestamp(tx, "SELECT MAX(admitted_at) FROM manual_asset_accounts")?;
    Ok(format_admission_timestamp(next_admission_timestamp(
        now, previous,
    )?))
}

fn ordered_rows(
    conn: &rusqlite::Connection,
    sql: &str,
) -> Result<Vec<(String, DateTime<Utc>)>, DbError> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|error| DbError::new(format!("Failed to prepare admission rows: {error}")))?;
    let raw = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| DbError::new(format!("Failed to load admission rows: {error}")))?;
    let mut rows = Vec::new();
    for row in raw {
        let (id, timestamp) =
            row.map_err(|error| DbError::new(format!("Invalid admission row: {error}")))?;
        rows.push((id, parse_admission_timestamp(&timestamp)?));
    }
    rows.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    Ok(rows)
}

pub(crate) fn backfill_account_admission_conn(
    conn: &mut rusqlite::Connection,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let tx = conn
        .transaction()
        .map_err(|error| DbError::new(format!("Failed to start admission repair: {error}")))?;
    let native_existing = ordered_rows(
        &tx,
        "SELECT s.account_id, s.selected_at FROM account_sync_slots s
         JOIN digital_asset_accounts a ON a.id = s.account_id",
    )?;
    let native_missing = ordered_rows(
        &tx,
        "SELECT a.id, a.created_at FROM digital_asset_accounts a
         LEFT JOIN account_sync_slots s ON s.account_id = a.id
         WHERE s.account_id IS NULL",
    )?;
    let manual_existing = ordered_rows(
        &tx,
        "SELECT id, admitted_at FROM manual_asset_accounts WHERE admitted_at IS NOT NULL",
    )?;
    let manual_missing = ordered_rows(
        &tx,
        "SELECT id, created_at FROM manual_asset_accounts WHERE admitted_at IS NULL",
    )?;

    let mut previous = None;
    for (id, selected_at) in native_existing {
        let admitted_at = next_admission_timestamp(selected_at, previous)?;
        tx.execute(
            "UPDATE account_sync_slots SET selected_at = ?2 WHERE account_id = ?1",
            params![id, format_admission_timestamp(admitted_at)],
        )
        .map_err(|error| DbError::new(format!("Failed to normalize native admission: {error}")))?;
        previous = Some(admitted_at);
    }
    for (id, _) in native_missing {
        let admitted_at = next_admission_timestamp(now, previous)?;
        tx.execute(
            "INSERT INTO account_sync_slots (account_id, selected_at, selected_under_tier)
             VALUES (?1, ?2, ?3)",
            params![
                id,
                format_admission_timestamp(admitted_at),
                EntitlementTier::Free.as_str(),
            ],
        )
        .map_err(|error| DbError::new(format!("Failed to backfill native admission: {error}")))?;
        previous = Some(admitted_at);
    }

    previous = None;
    for (id, selected_at) in manual_existing {
        let admitted_at = next_admission_timestamp(selected_at, previous)?;
        tx.execute(
            "UPDATE manual_asset_accounts SET admitted_at = ?2 WHERE id = ?1",
            params![id, format_admission_timestamp(admitted_at)],
        )
        .map_err(|error| DbError::new(format!("Failed to normalize manual admission: {error}")))?;
        previous = Some(admitted_at);
    }
    for (id, _) in manual_missing {
        let admitted_at = next_admission_timestamp(now, previous)?;
        tx.execute(
            "UPDATE manual_asset_accounts SET admitted_at = ?2 WHERE id = ?1",
            params![id, format_admission_timestamp(admitted_at)],
        )
        .map_err(|error| DbError::new(format!("Failed to backfill manual admission: {error}")))?;
        previous = Some(admitted_at);
    }
    tx.commit()
        .map_err(|error| DbError::new(format!("Failed to commit admission repair: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, OptionalExtension};

    fn at(raw: &str) -> DateTime<Utc> {
        raw.parse().expect("fixed timestamp should parse")
    }

    #[test]
    fn admission_timestamp_is_fixed_width_utc_microseconds() {
        let first = at("2026-04-01T00:00:00.000000Z");
        let later = at("2026-04-01T00:00:00.000001Z");
        let first_raw = format_admission_timestamp(first);
        let later_raw = format_admission_timestamp(later);
        assert_eq!(first_raw, "2026-04-01T00:00:00.000000Z");
        assert_eq!(later_raw, "2026-04-01T00:00:00.000001Z");
        assert!(first_raw < later_raw);
        assert!(first < later);
    }

    #[test]
    fn admission_timestamp_advances_through_equal_and_backward_clocks() {
        let previous = at("2026-04-01T00:00:00.000000Z");
        assert_eq!(
            next_admission_timestamp(previous, None).expect("first admission"),
            previous,
        );
        assert_eq!(
            next_admission_timestamp(previous, Some(previous)).expect("equal clock"),
            at("2026-04-01T00:00:00.000001Z"),
        );
        assert_eq!(
            next_admission_timestamp(at("2026-03-31T23:00:00Z"), Some(previous))
                .expect("backward clock"),
            at("2026-04-01T00:00:00.000001Z"),
        );
    }

    fn raw_v51_database() -> Connection {
        let conn = Connection::open_in_memory().expect("raw database should open");
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE digital_asset_accounts (id TEXT PRIMARY KEY, created_at TEXT NOT NULL);
             CREATE TABLE account_sync_slots (
                 account_id TEXT PRIMARY KEY REFERENCES digital_asset_accounts(id) ON DELETE CASCADE,
                 selected_at TEXT NOT NULL,
                 selected_under_tier TEXT NOT NULL
             );
             CREATE TABLE manual_asset_accounts (id TEXT PRIMARY KEY, created_at TEXT NOT NULL);",
        )
        .expect("pre-V52 tables should exist");
        conn
    }

    fn apply_v52(conn: &Connection) {
        conn.execute_batch(include_str!(
            "../../migrations/user/V52__account_admission_order.sql"
        ))
        .expect("V52 should apply to raw pre-V52 tables");
    }

    fn ordered_ids(conn: &Connection, sql: &str) -> Vec<String> {
        let mut statement = conn.prepare(sql).expect("order query should prepare");
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("order query should run")
            .map(|row| row.expect("ordered id should load"))
            .collect()
    }

    #[test]
    fn v52_backfill_appends_slotless_native_and_orders_manual_by_creation() {
        let mut conn = raw_v51_database();
        conn.execute_batch(
            "INSERT INTO digital_asset_accounts VALUES
               ('a', '2026-04-01T00:00:00Z'),
               ('b', '2026-04-03T00:00:00Z'),
               ('c', '2026-04-04T00:00:00Z');
             INSERT INTO account_sync_slots VALUES
               ('c', '2026-04-02T00:00:00Z', 'basic'),
               ('b', '2026-04-02T00:00:00Z', 'free');
             INSERT INTO manual_asset_accounts VALUES
               ('z', '2026-04-01T00:00:00Z'),
               ('m', '2026-04-03T00:00:00Z');",
        )
        .expect("historical rows should seed");
        apply_v52(&conn);
        let now = at("2026-05-01T00:00:00Z");
        backfill_account_admission_conn(&mut conn, now).expect("repair should complete");

        assert_eq!(
            ordered_ids(
                &conn,
                "SELECT account_id FROM account_sync_slots ORDER BY selected_at, account_id"
            ),
            ["b", "c", "a"],
        );
        assert_eq!(
            ordered_ids(
                &conn,
                "SELECT id FROM manual_asset_accounts ORDER BY admitted_at, id"
            ),
            ["z", "m"],
        );
        let original_tier: String = conn
            .query_row(
                "SELECT selected_under_tier FROM account_sync_slots WHERE account_id = 'c'",
                [],
                |row| row.get(0),
            )
            .expect("tier should load");
        assert_eq!(original_tier, "basic");
        let original_created: String = conn
            .query_row(
                "SELECT created_at FROM digital_asset_accounts WHERE id = 'a'",
                [],
                |row| row.get(0),
            )
            .expect("source creation time should load");
        assert_eq!(original_created, "2026-04-01T00:00:00Z");
        let before = ordered_ids(
            &conn,
            "SELECT selected_at FROM account_sync_slots ORDER BY selected_at, account_id",
        );
        assert!(
            before
                .iter()
                .all(|value| value.ends_with(".000000Z") || value.ends_with(".000001Z"))
        );
        backfill_account_admission_conn(&mut conn, now)
            .expect("interrupted marker replay should be idempotent");
        assert_eq!(
            before,
            ordered_ids(
                &conn,
                "SELECT selected_at FROM account_sync_slots ORDER BY selected_at, account_id"
            ),
        );
    }

    #[test]
    fn v52_backfill_without_slots_uses_native_creation_order() {
        let mut conn = raw_v51_database();
        conn.execute_batch(
            "INSERT INTO digital_asset_accounts VALUES
               ('z', '2026-04-01T00:00:00Z'),
               ('a', '2026-04-02T00:00:00Z');",
        )
        .expect("slotless rows should seed");
        apply_v52(&conn);
        backfill_account_admission_conn(&mut conn, at("2026-05-01T00:00:00Z"))
            .expect("repair should complete");
        assert_eq!(
            ordered_ids(
                &conn,
                "SELECT account_id FROM account_sync_slots ORDER BY selected_at, account_id"
            ),
            ["z", "a"],
        );
    }

    #[test]
    fn native_enrollment_is_atomic_monotone_and_idempotent() {
        let mut conn = raw_v51_database();
        apply_v52(&conn);
        let first = DigitalAssetAccountId::new();
        let second = DigitalAssetAccountId::new();
        let now = at("2026-04-01T00:00:00Z");
        let first_id = first.to_string();
        let second_id = second.to_string();

        {
            let tx = conn.transaction().expect("first transaction should open");
            tx.execute(
                "INSERT INTO digital_asset_accounts VALUES (?1, ?2)",
                params![first_id, format_admission_timestamp(now)],
            )
            .expect("first account should insert");
            enroll_native_account_in_tx(&tx, first, now, &EntitlementTier::Free)
                .expect("first admission should insert");
        }
        let rolled_back: Option<String> = conn
            .query_row(
                "SELECT account_id FROM account_sync_slots WHERE account_id = ?1",
                [&first_id],
                |row| row.get(0),
            )
            .optional()
            .expect("rollback should be queryable");
        assert!(rolled_back.is_none());

        for (account, id) in [(first, &first_id), (second, &second_id)] {
            let tx = conn
                .transaction()
                .expect("creation transaction should open");
            tx.execute(
                "INSERT INTO digital_asset_accounts VALUES (?1, ?2)",
                params![id, format_admission_timestamp(now)],
            )
            .expect("account should insert");
            enroll_native_account_in_tx(&tx, account, now, &EntitlementTier::Free)
                .expect("admission should insert");
            tx.commit().expect("creation should commit");
        }
        let before = ordered_ids(
            &conn,
            "SELECT selected_at FROM account_sync_slots ORDER BY selected_at, account_id",
        );
        assert_eq!(
            before,
            ["2026-04-01T00:00:00.000000Z", "2026-04-01T00:00:00.000001Z",]
        );
        let tx = conn
            .transaction()
            .expect("duplicate transaction should open");
        enroll_native_account_in_tx(&tx, first, now, &EntitlementTier::Free)
            .expect("duplicate admission should be harmless");
        tx.commit().expect("duplicate should commit");
        assert_eq!(
            before,
            ordered_ids(
                &conn,
                "SELECT selected_at FROM account_sync_slots ORDER BY selected_at, account_id"
            ),
        );
        conn.execute(
            "DELETE FROM digital_asset_accounts WHERE id = ?1",
            [&first_id],
        )
        .expect("account should delete");
        assert_eq!(
            ordered_ids(&conn, "SELECT account_id FROM account_sync_slots"),
            [second_id],
        );
    }
}
