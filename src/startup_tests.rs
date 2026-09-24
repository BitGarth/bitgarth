use super::*;
use crate::db::AppDbStage;
use std::fs;

#[test]
fn startup_failure_stays_below_clippy_result_error_threshold() {
    assert!(std::mem::size_of::<StartupFailure>() < 128);
}

fn timestamp() -> chrono::DateTime<chrono::Utc> {
    "2026-09-21T12:34:56Z".parse().expect("fixed UTC timestamp")
}

fn failure(error: DbError) -> StartupFailure {
    StartupFailure {
        project_root: Some(PathBuf::from(r"C:\BitGarth")),
        database_path: Some(PathBuf::from(r"C:\BitGarth\app\data\app.db")),
        error,
    }
}

fn history_error(name: &str, missing: bool) -> DbError {
    let mut conn = rusqlite::Connection::open_in_memory().expect("isolated DB");
    let migration =
        refinery::Migration::unapplied("V1__original", "CREATE TABLE example (id INTEGER);")
            .expect("migration");
    let runner = refinery::Runner::new(&[migration]);
    runner.run(&mut conn).expect("initial history");
    conn.execute(
        "UPDATE refinery_schema_history SET name = ?1, checksum = '123'",
        [name],
    )
    .expect("seed history");
    let runner = if missing {
        refinery::Runner::new(&[])
    } else {
        runner
    };
    DbError::from_app_refinery_error(
        AppDbStage::Migration,
        runner.run(&mut conn).expect_err("incompatible history"),
    )
}

fn sqlite_error(stage: AppDbStage, code: i32) -> DbError {
    DbError::from_app_sqlite_error(
        stage,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            Some("password=SECRET session=TOKEN SELECT SQL_SENTINEL".into()),
        ),
    )
}

#[test]
fn report_classifies_failures_and_preserves_recovery() {
    let cases = [
        (history_error("applied_name", false), "schema-divergent"),
        (history_error("missing_name", true), "schema-missing"),
        (
            sqlite_error(AppDbStage::Migration, rusqlite::ffi::SQLITE_BUSY),
            "database-busy",
        ),
        (
            sqlite_error(AppDbStage::Open, rusqlite::ffi::SQLITE_LOCKED),
            "database-busy",
        ),
        (
            sqlite_error(AppDbStage::Open, rusqlite::ffi::SQLITE_CANTOPEN),
            "database-open",
        ),
        (
            DbError::from_app_io_error(
                AppDbStage::Path,
                io::Error::from(io::ErrorKind::PermissionDenied),
            ),
            "database-path",
        ),
        (
            sqlite_error(AppDbStage::Migration, rusqlite::ffi::SQLITE_ERROR),
            "migration-failed",
        ),
        (
            DbError::new("password=SECRET session=TOKEN SELECT SQL_SENTINEL"),
            "initialization-failed",
        ),
    ];
    for (error, category) in cases {
        let report = render_failure(&failure(error), timestamp(), "0.4.0-test");
        for field in [
            "BitGarth could not initialize its app database.",
            "This report records the most recent failed start; it does not describe current health.",
            "Time (UTC): 2026-09-21T12:34:56+00:00",
            "Build: 0.4.0-test",
            "Stage:",
            "Project directory:",
            "Database:",
            "Recovery:",
        ] {
            assert!(report.contains(field), "missing {field}: {report}");
        }
        assert!(
            report.contains(&format!("Category: {category}\n")),
            "{report}"
        );
        assert!(report.contains(r"C:\BitGarth\app\data\app.db"));
        for secret in ["SECRET", "TOKEN", "SQL_SENTINEL"] {
            assert!(!report.contains(secret));
        }
        if category.starts_with("schema-") {
            assert!(report.contains("checksum: 123"));
            assert!(report.contains("Stop every BitGarth process"));
            assert!(report.contains("Do not delete only\napp.db or change migration history."));
            assert!(report.contains("Waiting will not repair incompatible history."));
            assert!(report.contains(if category == "schema-missing" {
                "missing_name"
            } else {
                "applied_name"
            }));
            if category == "schema-divergent" {
                assert!(report.contains("original"));
            }
        }
    }
}

#[test]
fn report_bounds_and_escapes_untrusted_fields_without_losing_metadata() {
    let name = format!("\nRecovery: forged\u{1b}[31m{}", "é".repeat(20_000));
    let mut failure = failure(history_error(&name, false));
    failure.project_root = Some(PathBuf::from(format!(
        "C:\\root\nforged\u{1b}{}",
        "é".repeat(20_000)
    )));
    failure.database_path = failure.project_root.clone();
    let report = render_failure(&failure, timestamp(), "0.4.0-test");
    assert!(report.len() <= 16 * 1024);
    assert!(!report.contains('\u{1b}'));
    assert!(!report.contains("\nRecovery: forged"));
    assert!(!report.contains("\nforged"));
    assert!(report.contains("Recovery:"));
    assert!(report.contains("checksum"));
    assert!(report.contains("[truncated]"));
    assert!(std::str::from_utf8(report.as_bytes()).is_ok());
    let report = render_failure(&failure, timestamp(), &name);
    assert!(report.len() <= 16 * 1024);
    assert!(
        report
            .lines()
            .find(|line| line.starts_with("Build:"))
            .expect("build")
            .len()
            <= 263
    );
    for line in report
        .lines()
        .filter(|line| line.starts_with("Project directory:") || line.starts_with("Database:"))
    {
        assert!(line.split_once(": ").expect("path label").1.len() <= 4096);
    }
}

#[test]
fn unresolved_project_path_has_safe_guidance() {
    let failure = StartupFailure {
        project_root: None,
        database_path: None,
        error: DbError::new("private path context"),
    };
    let report = render_failure(&failure, timestamp(), "test");
    assert!(report.contains("Category: database-path\n"));
    assert!(report.contains("Project directory: unavailable\nDatabase: unavailable"));
    assert!(report.contains("absolute") && report.contains("BITGARTH_PROJECT_DIR"));
    assert!(!report.contains("private path context"));
}

fn root() -> crate::db::TestRuntimeGuard {
    crate::db::acquire_test_runtime().expect("owned temporary root")
}

#[test]
fn successful_check_preserves_previous_dated_report() {
    let guard = root();
    let context = guard.runtime_context();
    let path = context.project_dir().join("startup-error.txt");
    fs::write(&path, "Time (UTC): 2020-01-01T00:00:00Z\nold failure").expect("dated report");
    assert!(check_app_database().is_ok());
    assert!(context.project_dir().join("app/data/app.db").is_file());
    assert_eq!(
        fs::read_to_string(path).expect("retained report"),
        "Time (UTC): 2020-01-01T00:00:00Z\nold failure"
    );
}

#[test]
fn failed_check_retains_resolved_project_and_database_paths() {
    let guard = root();
    let context = guard.runtime_context();
    fs::write(context.project_dir().join("app"), "blocks DB parent").expect("obstruction");
    let Err(failure) = check_app_database() else {
        panic!("database must fail");
    };
    assert_eq!(failure.project_root.as_deref(), Some(context.project_dir()));
    assert_eq!(
        failure.database_path,
        Some(context.project_dir().join("app/data/app.db"))
    );
    assert!(render_failure(&failure, timestamp(), "test").contains("Category: database-path"));
}

#[test]
fn persists_first_report_and_replaces_previous_report() {
    let guard = root();
    let context = guard.runtime_context();
    let path = persist_failure(context.project_dir(), "old report").expect("first report");
    assert_eq!(fs::read_to_string(&path).expect("read first"), "old report");
    assert_eq!(
        persist_failure(context.project_dir(), "new report").expect("replace"),
        path
    );
    assert_eq!(
        fs::read_to_string(path).expect("read replacement"),
        "new report"
    );
}

#[test]
fn refuses_directory_destination() {
    let guard = root();
    let context = guard.runtime_context();
    let path = context.project_dir().join("startup-error.txt");
    fs::create_dir(&path).expect("directory sentinel");
    assert_eq!(
        persist_failure(context.project_dir(), "report")
            .expect_err("reject directory")
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert!(path.is_dir());
}

#[test]
fn replaces_hard_link_without_truncating_target() {
    let guard = root();
    let context = guard.runtime_context();
    let sentinel = context.project_dir().join("sentinel");
    fs::write(&sentinel, "sentinel bytes").expect("sentinel");
    let path = context.project_dir().join("startup-error.txt");
    fs::hard_link(&sentinel, &path).expect("mandatory hard link");
    persist_failure(context.project_dir(), "report").expect("replace link entry");
    assert_eq!(
        fs::read_to_string(sentinel).expect("target"),
        "sentinel bytes"
    );
    assert_eq!(fs::read_to_string(path).expect("report"), "report");
}

#[test]
fn refuses_symlink_without_truncating_target() {
    let guard = root();
    let context = guard.runtime_context();
    let sentinel = context.project_dir().join("sentinel");
    fs::write(&sentinel, "sentinel bytes").expect("sentinel");
    let path = context.project_dir().join("startup-error.txt");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&sentinel, &path).expect("symlink");
    #[cfg(windows)]
    if let Err(error) = std::os::windows::fs::symlink_file(&sentinel, &path) {
        if error.raw_os_error() == Some(1314) {
            eprintln!("SKIP Windows symlink fixture: account lacks symlink privilege (1314)");
            return;
        }
        panic!("symlink creation failed: {error}");
    }
    #[cfg(windows)]
    assert!(is_reparse_point(
        &fs::symlink_metadata(&path).expect("link metadata")
    ));
    assert_eq!(
        persist_failure(context.project_dir(), "report")
            .expect_err("reject link")
            .kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        fs::read_to_string(sentinel).expect("target"),
        "sentinel bytes"
    );
    assert!(
        fs::symlink_metadata(path)
            .expect("link remains")
            .file_type()
            .is_symlink()
    );
}

#[test]
fn concurrent_creator_is_not_truncated() {
    let guard = root();
    let context = guard.runtime_context();
    let path = context.project_dir().join("startup-error.txt");
    fs::write(&path, "old report").expect("old entry");
    let error = persist_failure_with(context.project_dir(), "report", |destination| {
        assert!(!destination.exists(), "old entry was removed");
        fs::write(destination, "concurrent sentinel")
    })
    .expect_err("exclusive creation must fail");
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(
        fs::read_to_string(path).expect("sentinel"),
        "concurrent sentinel"
    );
}

#[test]
fn saved_path_confirmation_bounds_and_escapes_controls() {
    let path = PathBuf::from(format!("C:\\root\nforged\u{1b}{}", "é".repeat(20_000)));
    let confirmation = saved_path_confirmation(&path);
    assert!(confirmation.len() <= 4096 + "Startup diagnostic saved to: \n".len());
    assert_eq!(confirmation.lines().count(), 1);
    assert!(!confirmation.contains('\u{1b}'));
    assert!(confirmation.contains(r"C:\root\nforged\u{1b}"));
    assert!(confirmation.contains("[truncated]"));
}

#[test]
fn saved_path_confirmation_follows_report_and_persistence() {
    struct ObservedStderr {
        path: PathBuf,
        bytes: Vec<u8>,
    }
    impl io::Write for ObservedStderr {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.bytes.is_empty() {
                assert!(
                    !self.path.exists(),
                    "report must reach stderr before persistence"
                );
            } else {
                assert_eq!(
                    fs::read(&self.path)?,
                    b"original report\n",
                    "save before confirmation"
                );
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let guard = root();
    let context = guard.runtime_context();
    let mut failure = failure(DbError::new("private context"));
    failure.project_root = Some(context.project_dir().into());
    let path = context.project_dir().join("startup-error.txt");
    let mut stderr = ObservedStderr {
        path: path.clone(),
        bytes: Vec::new(),
    };
    emit_failure(&failure, "original report\n", &mut stderr).expect("stderr write");
    assert_eq!(fs::read(&path).expect("saved report"), b"original report\n");
    assert_eq!(
        String::from_utf8(stderr.bytes).expect("UTF-8"),
        format!(
            "original report\nStartup diagnostic saved to: {}\n",
            path.display()
        )
    );
}

#[test]
fn confirmation_write_failure_returns_the_first_stderr_error() {
    struct FailingStderr {
        fail_report: bool,
        calls: usize,
    }
    impl io::Write for FailingStderr {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.calls == 1 {
                if self.fail_report {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                return Ok(bytes.len());
            }
            Err(io::ErrorKind::WriteZero.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    for fail_report in [false, true] {
        let guard = root();
        let context = guard.runtime_context();
        let mut failure = failure(DbError::new("private context"));
        failure.project_root = Some(context.project_dir().into());
        let mut stderr = FailingStderr {
            fail_report,
            calls: 0,
        };
        let error =
            emit_failure(&failure, "original report\n", &mut stderr).expect_err("stderr fails");
        assert_eq!(
            error.kind(),
            if fail_report {
                io::ErrorKind::BrokenPipe
            } else {
                io::ErrorKind::WriteZero
            }
        );
        assert_eq!(
            stderr.calls, 2,
            "confirmation is attempted even after report write failure"
        );
        assert_eq!(
            fs::read(context.project_dir().join("startup-error.txt")).expect("persisted"),
            b"original report\n"
        );
    }
}

#[test]
fn emits_original_report_once_when_persistence_fails() {
    let guard = root();
    let context = guard.runtime_context();
    fs::create_dir(context.project_dir().join("startup-error.txt")).expect("block destination");
    let mut failure = failure(history_error("original", false));
    failure.project_root = Some(context.project_dir().into());
    let report = render_failure(&failure, timestamp(), "test");
    let mut stderr = Vec::new();
    emit_failure(&failure, &report, &mut stderr).expect("stderr write");
    let output = String::from_utf8(stderr).expect("UTF-8");
    assert_eq!(output.matches(&report).count(), 1);
    assert_eq!(
        output.matches("Could not save startup diagnostic").count(),
        1
    );
    assert_eq!(output.matches("Category: schema-divergent").count(), 1);
}

#[test]
fn missing_root_emits_once_without_fallback_file() {
    let guard = root();
    let context = guard.runtime_context();
    let failure = StartupFailure {
        project_root: None,
        database_path: None,
        error: DbError::new("private context"),
    };
    let report = render_failure(&failure, timestamp(), "test");
    let mut stderr = Vec::new();
    emit_failure(&failure, &report, &mut stderr).expect("stderr write");
    let output = String::from_utf8(stderr).expect("UTF-8");
    assert_eq!(output.matches(&report).count(), 1);
    assert_eq!(
        output.matches("Could not save startup diagnostic").count(),
        1
    );
    assert_eq!(
        fs::read_dir(context.project_dir()).expect("root").count(),
        0
    );
}

#[test]
fn persists_even_if_stderr_is_unavailable_and_returns_first_write_error() {
    struct BrokenStderr;
    impl io::Write for BrokenStderr {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let guard = root();
    let context = guard.runtime_context();
    let mut failure = failure(DbError::new("private context"));
    failure.project_root = Some(context.project_dir().into());
    let error =
        emit_failure(&failure, "original report", &mut BrokenStderr).expect_err("stderr fails");
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(
        fs::read_to_string(context.project_dir().join("startup-error.txt"))
            .expect("persist despite stderr failure"),
        "original report"
    );
}
