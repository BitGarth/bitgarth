use super::super::error::{AppDbFailure, AppDbFailureKind, AppDbStage, MigrationIdentity};
use super::*;
use std::cell::Cell;

fn app_failure(error: &DbError) -> &AppDbFailure {
    error
        .app_initialization()
        .expect("app initialization errors retain typed metadata")
}

fn history_rows(
    conn: &rusqlite::Connection,
) -> Result<Vec<(i64, String, String)>, rusqlite::Error> {
    let mut statement = conn
        .prepare("SELECT version, name, checksum FROM refinery_schema_history ORDER BY version")?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect()
}

#[test]
fn changed_name_is_classified_without_rewriting_history() -> Result<(), Box<dyn std::error::Error>>
{
    let mut conn = rusqlite::Connection::open_in_memory()?;
    configure_and_migrate_connection(&mut conn)?;
    conn.execute(
        "UPDATE refinery_schema_history SET name = ?1 WHERE version = 8",
        ["payment_product_options_cache"],
    )?;
    let history_before_rejection = history_rows(&conn)?;
    let outcome = configure_and_migrate_connection(&mut conn);
    let Err(error) = outcome else {
        return Err("divergent database unexpectedly initialized".into());
    };
    assert!(matches!(
        error.app_initialization().map(|value| &value.kind),
        Some(AppDbFailureKind::Divergent { applied, expected })
            if applied.version == 8
                && applied.name == "payment_product_options_cache"
                && expected.name == "user_last_login"
    ));
    let AppDbFailureKind::Divergent { expected, .. } = &app_failure(&error).kind else {
        return Err("expected divergent migration metadata".into());
    };
    let expected: &MigrationIdentity = expected;
    assert_eq!(expected.version, 8);
    let name: String = conn.query_row(
        "SELECT name FROM refinery_schema_history WHERE version = 8",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(name, "payment_product_options_cache");
    assert_eq!(history_rows(&conn)?, history_before_rejection);
    Ok(())
}

#[test]
fn changed_checksum_is_classified_without_rewriting_history()
-> Result<(), Box<dyn std::error::Error>> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    configure_and_migrate_connection(&mut conn)?;
    conn.execute(
        "UPDATE refinery_schema_history SET checksum = '0' WHERE version = 8",
        [],
    )?;
    let history_before_rejection = history_rows(&conn)?;

    let error = configure_and_migrate_connection(&mut conn).expect_err("divergent checksum");
    assert!(matches!(
        &app_failure(&error).kind,
        AppDbFailureKind::Divergent { applied, expected }
            if applied.version == 8
                && applied.checksum == 0
                && expected.version == 8
                && expected.name == "user_last_login"
    ));
    assert_eq!(history_rows(&conn)?, history_before_rejection);
    Ok(())
}

#[test]
fn malformed_checksum_is_rejected_without_rewriting_history()
-> Result<(), Box<dyn std::error::Error>> {
    rejects_malformed_history(
        "checksum",
        &["invalid-checksum", "18446744073709551616", "-1", ""],
    )
}

#[test]
fn malformed_timestamp_is_rejected_without_rewriting_history()
-> Result<(), Box<dyn std::error::Error>> {
    rejects_malformed_history(
        "applied_on",
        &[
            "invalid-timestamp",
            "2026-02-30T00:00:00Z",
            "2026-09-21T00:00:00+24:00",
            "2026-09-21T00:00:00+00:60",
        ],
    )
}

fn rejects_malformed_history(
    field: &str,
    values: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    for value in values {
        let mut conn = rusqlite::Connection::open_in_memory()?;
        migrations_runner()?
            .set_target(refinery::Target::Version(7))
            .run(&mut conn)?;
        conn.execute(
            &format!("UPDATE refinery_schema_history SET {field} = ?1 WHERE version = 7"),
            [value],
        )?;
        let before = history_rows(&conn)?;
        let error =
            configure_and_migrate_connection(&mut conn).expect_err("reject invalid metadata");
        assert_eq!(app_failure(&error).stage, AppDbStage::Migration);
        assert_eq!(
            history_rows(&conn)?,
            before,
            "no pending migrations or history changes"
        );
        let stored: String = conn.query_row(
            &format!("SELECT {field} FROM refinery_schema_history WHERE version = 7"),
            [],
            |row| row.get(0),
        )?;
        assert_eq!(stored, *value);
    }
    Ok(())
}

#[test]
fn unknown_applied_migration_is_classified_without_rewriting_history()
-> Result<(), Box<dyn std::error::Error>> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    configure_and_migrate_connection(&mut conn)?;
    conn.execute(
        "UPDATE refinery_schema_history SET version = 2147483647
        WHERE version = (SELECT MAX(version) FROM refinery_schema_history)",
        [],
    )?;
    let history_before_rejection = history_rows(&conn)?;

    let error = configure_and_migrate_connection(&mut conn).expect_err("missing migration");
    assert!(matches!(
        &app_failure(&error).kind,
        AppDbFailureKind::Missing { migration } if migration.version == 2_147_483_647
    ));
    assert_eq!(history_rows(&conn)?, history_before_rejection);
    Ok(())
}

#[test]
fn deleted_applied_migration_is_classified_without_rewriting_history()
-> Result<(), Box<dyn std::error::Error>> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    configure_and_migrate_connection(&mut conn)?;
    conn.execute("DELETE FROM refinery_schema_history WHERE version = 8", [])?;
    let history_before_rejection = history_rows(&conn)?;

    let error = configure_and_migrate_connection(&mut conn).expect_err("missing migration");
    assert!(matches!(
        &app_failure(&error).kind,
        AppDbFailureKind::Missing { migration } if migration.version == 8
    ));
    assert_eq!(history_rows(&conn)?, history_before_rejection);
    Ok(())
}

#[test]
fn pending_migration_upgrade_preserves_existing_rows() -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    migrations_runner()?
        .set_target(refinery::Target::Version(7))
        .run(&mut conn)?;
    conn.execute_batch("CREATE TABLE startup_test_sentinel(value TEXT)")?;
    conn.execute(
        "INSERT INTO startup_test_sentinel(value) VALUES (?1)",
        ["kept"],
    )?;

    configure_and_migrate_connection(&mut conn)?;

    let value: String = conn.query_row("SELECT value FROM startup_test_sentinel", [], |row| {
        row.get(0)
    })?;
    assert_eq!(value, "kept");
    Ok(())
}

#[test]
fn malformed_app_sqlite_error_is_not_a_schema_incompatibility()
-> Result<(), Box<dyn std::error::Error>> {
    let conn = rusqlite::Connection::open_in_memory()?;
    let sqlite_error = conn
        .execute_batch("SELECT FROM")
        .expect_err("malformed SQLite statement");
    let error = DbError::from_app_sqlite_error(AppDbStage::Open, sqlite_error);

    assert_eq!(app_failure(&error).stage, AppDbStage::Open);
    assert!(matches!(app_failure(&error).kind, AppDbFailureKind::Sqlite));
    Ok(())
}

#[test]
fn app_sqlite_failure_retains_sqlite_information() {
    let details = rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR);
    let error = DbError::from_app_sqlite_error(
        AppDbStage::Open,
        rusqlite::Error::SqliteFailure(details, Some("syntax error".to_owned())),
    );

    let sqlite_failure = error
        .sqlite_failure()
        .expect("SQLite failure information should be retained");
    assert_eq!(sqlite_failure.code, details.code);
    assert_eq!(sqlite_failure.extended_code, details.extended_code);
    assert_eq!(sqlite_failure.message.as_deref(), Some("syntax error"));
}

#[test]
fn failed_cached_connection_is_not_retained() -> Result<(), Box<dyn std::error::Error>> {
    let mut state = ThreadConnectionState::new();
    let key = AppDbConnectionKey::TestInMemory;
    let attempts = Cell::new(0);

    let first = cached_connection(&mut state, key.clone(), |_| {
        attempts.set(attempts.get() + 1);
        Err(DbError::new("first initialization fails"))
    });
    assert!(first.is_err());

    let second = cached_connection(&mut state, key.clone(), |_| {
        attempts.set(attempts.get() + 1);
        rusqlite::Connection::open_in_memory()
            .map_err(|error| DbError::from_app_sqlite_error(AppDbStage::Open, error))
    })?;
    let _immutable_borrow = second.borrow();
    drop(_immutable_borrow);

    let third = cached_connection(&mut state, key, |_| {
        attempts.set(attempts.get() + 1);
        Err(DbError::new("cached connection should skip initializer"))
    })?;
    let _mutable_borrow = third.borrow_mut();
    assert_eq!(attempts.get(), 2);
    Ok(())
}

#[test]
fn file_connection_retries_after_parent_block_is_removed() -> Result<(), Box<dyn std::error::Error>>
{
    let root = std::env::temp_dir().join(format!("bitgarth_app_db_retry_{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&root)?;
    let blocking_file = root.join("app");
    std::fs::write(&blocking_file, "block parent")?;
    let db_path = crate::project_paths::app_database_path_from_project_dir(&root);
    let mut state = ThreadConnectionState::new();
    let key = AppDbConnectionKey::Production;

    let first = cached_connection(&mut state, key.clone(), |_| {
        initialize_file_connection(&db_path)
    });
    assert!(matches!(
        first.as_ref().err().and_then(DbError::app_initialization),
        Some(AppDbFailure {
            stage: AppDbStage::Path,
            kind: AppDbFailureKind::Io { .. },
        })
    ));

    std::fs::remove_file(&blocking_file)?;
    let second = cached_connection(&mut state, key, |_| initialize_file_connection(&db_path));
    assert!(second.is_ok());
    drop(second);
    drop(state);
    std::fs::remove_dir_all(&root)?;
    Ok(())
}
