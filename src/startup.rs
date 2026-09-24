use crate::db::{AppDbFailureKind, AppDbStage, DbError, MigrationIdentity};
use crate::project_paths::{app_database_path_from_project_dir, get_project_dir_with_context};
use std::io;
use std::path::{Path, PathBuf};

pub(crate) struct StartupFailure {
    project_root: Option<PathBuf>,
    database_path: Option<PathBuf>,
    error: DbError,
}

pub(crate) fn check_app_database() -> Result<(), StartupFailure> {
    let project_root = get_project_dir_with_context().map_err(|failure| StartupFailure {
        database_path: failure
            .project_root
            .as_deref()
            .map(app_database_path_from_project_dir),
        project_root: failure.project_root,
        error: failure.error,
    })?;
    let database_path = app_database_path_from_project_dir(&project_root);
    crate::db::initialize_app_db().map_err(|error| StartupFailure {
        project_root: Some(project_root),
        database_path: Some(database_path),
        error,
    })
}

pub(crate) fn report_app_database_failure(failure: &StartupFailure) {
    let report = render_failure(failure, chrono::Utc::now(), crate::version::version());
    let _ = emit_failure(failure, &report, &mut io::stderr().lock());
}

fn bounded_field(value: &str, budget: usize) -> String {
    const TRUNCATED: &str = "[truncated]";
    let mut result = String::new();
    for character in value.chars() {
        let escaped = if character.is_control() {
            character.escape_default().to_string()
        } else {
            character.to_string()
        };
        if result.len() + escaped.len() > budget - TRUNCATED.len() {
            result.push_str(TRUNCATED);
            break;
        }
        result.push_str(&escaped);
    }
    result
}

fn report_path(path: Option<&Path>) -> String {
    path.map(|path| bounded_field(&path.display().to_string(), 4096))
        .unwrap_or_else(|| "unavailable".to_owned())
}

fn migration_line(label: &str, migration: &MigrationIdentity) -> String {
    format!(
        "{label}: V{}__{}; checksum: {}\n",
        migration.version,
        bounded_field(&migration.name, 1024),
        migration.checksum
    )
}

fn render_failure(
    failure: &StartupFailure,
    timestamp: chrono::DateTime<chrono::Utc>,
    build: &str,
) -> String {
    let metadata = failure.error.app_initialization();
    let stage = metadata.map(|metadata| metadata.stage);
    let kind = metadata.map(|metadata| &metadata.kind);
    let sqlite = failure.error.sqlite_failure();
    let busy = sqlite.is_some_and(|details| {
        matches!(
            details.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
        )
    });
    let category = if busy {
        "database-busy"
    } else {
        match kind {
            Some(AppDbFailureKind::Divergent { .. }) => "schema-divergent",
            Some(AppDbFailureKind::Missing { .. }) => "schema-missing",
            _ => match stage {
                Some(AppDbStage::Path) => "database-path",
                Some(AppDbStage::Open) => "database-open",
                Some(AppDbStage::Migration | AppDbStage::MigrationDefinition) => "migration-failed",
                _ if failure.project_root.is_none() => "database-path",
                _ => "initialization-failed",
            },
        }
    };
    let stage = match stage {
        Some(AppDbStage::Path) => "path",
        Some(AppDbStage::Open) => "open",
        Some(AppDbStage::MigrationDefinition) => "migration-definition",
        Some(AppDbStage::Migration) => "migration",
        Some(AppDbStage::SchemaQuery) => "schema-query",
        None if failure.project_root.is_none() => "path",
        None => "initialization",
    };
    let mut report = format!(
        "BitGarth could not initialize its app database.\n\
         This report records the most recent failed start; it does not describe current health.\n\
         Time (UTC): {}\nBuild: {}\nStage: {stage}\nCategory: {category}\nProject directory: {}\nDatabase: {}\n",
        timestamp.to_rfc3339(),
        bounded_field(build, 256),
        report_path(failure.project_root.as_deref()),
        report_path(failure.database_path.as_deref())
    );
    match kind {
        Some(AppDbFailureKind::Divergent { applied, expected }) => {
            report.push_str(&migration_line("Applied migration", applied));
            report.push_str(&migration_line("Expected migration", expected));
        }
        Some(AppDbFailureKind::Missing { migration }) => {
            report.push_str(&migration_line("Missing migration", migration))
        }
        Some(AppDbFailureKind::Io { kind, raw_os_error }) => {
            report.push_str(&format!("IO kind: {kind:?}; OS code: {raw_os_error:?}\n"))
        }
        _ => {}
    }
    if let Some(details) = sqlite {
        report.push_str(&format!(
            "SQLite code: {:?}; extended code: {}\n",
            details.code, details.extended_code
        ));
    }
    report.push_str("Recovery:\n");
    report.push_str(match category {
        "schema-divergent" | "schema-missing" => "Stop every BitGarth process using this directory and back up the entire project\n\
directory, including user databases, envelopes, session-wrap-secret, and SQLite\n\
sidecar files. Use a build compatible with the full database history. To start\n\
an empty installation deliberately, select a new empty absolute directory with\n\
BITGARTH_PROJECT_DIR and keep the original directory intact. Do not delete only\n\
app.db or change migration history. Waiting will not repair incompatible history.\n",
        "database-busy" => "Close the process holding the database before restarting BitGarth.\n",
        "database-path" | "database-open" => "Check the reported path, permissions, disk space, and locks.\nSet BITGARTH_PROJECT_DIR to the intended absolute project directory.\n",
        "migration-failed" => "Preserve a backup of the entire project directory and inspect local logs before retrying.\n",
        _ => "Inspect local logs for the initialization failure before retrying.\n",
    });
    report
}

fn persist_failure(project_root: &Path, report: &str) -> io::Result<PathBuf> {
    persist_failure_with(project_root, report, |_| Ok(()))
}

fn persist_failure_with(
    project_root: &Path,
    report: &str,
    before_create: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let path = project_root.join("startup-error.txt");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() || is_reparse_point(&metadata) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "diagnostic destination is not a regular file",
                ));
            }
            std::fs::remove_file(&path)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    before_create(&path)?;
    let mut file = options.open(&path)?;
    io::Write::write_all(&mut file, report.as_bytes())?;
    file.sync_all()?;
    Ok(path)
}

fn emit_failure(
    failure: &StartupFailure,
    report: &str,
    stderr: &mut impl io::Write,
) -> io::Result<()> {
    let written = stderr.write_all(report.as_bytes());
    let saved = match &failure.project_root {
        Some(root) => persist_failure(root, report),
        None => Err(io::Error::from(io::ErrorKind::NotFound)),
    };
    let status_written = match saved {
        Ok(path) => stderr.write_all(saved_path_confirmation(&path).as_bytes()),
        Err(error) => writeln!(
            stderr,
            "Could not save startup diagnostic (IO kind: {:?}; OS code: {:?}). The report above remains the startup failure diagnostic.",
            error.kind(),
            error.raw_os_error()
        ),
    };
    // Persistence and its status write are attempted even if the report write
    // failed. Return the first stderr error, including a failed confirmation.
    written.and(status_written)
}

fn saved_path_confirmation(path: &Path) -> String {
    format!("Startup diagnostic saved to: {}\n", report_path(Some(path)))
}

#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
#[path = "startup_tests.rs"]
mod tests;
