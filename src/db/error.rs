//! Database error types

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqliteFailureInfo {
    pub(crate) code: rusqlite::ErrorCode,
    pub(crate) extended_code: i32,
    pub(crate) message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppDbStage {
    Path,
    Open,
    MigrationDefinition,
    Migration,
    SchemaQuery,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MigrationIdentity {
    pub(crate) version: u32,
    pub(crate) name: String,
    pub(crate) checksum: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AppDbFailureKind {
    Io {
        kind: std::io::ErrorKind,
        raw_os_error: Option<i32>,
    },
    Sqlite,
    Divergent {
        applied: MigrationIdentity,
        expected: MigrationIdentity,
    },
    Missing {
        migration: MigrationIdentity,
    },
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AppDbFailure {
    pub(crate) stage: AppDbStage,
    pub(crate) kind: AppDbFailureKind,
}

#[derive(Debug, Clone)]
struct DbErrorDetail {
    message: String,
    sqlite_failure: Option<SqliteFailureInfo>,
}

/// Database initialization or operation error
#[derive(Debug, Clone)]
pub struct DbError {
    detail: Box<DbErrorDetail>,
    app_initialization: Option<Box<AppDbFailure>>,
}

impl DbError {
    pub fn new(msg: impl Into<String>) -> Self {
        DbError {
            detail: Box::new(DbErrorDetail {
                message: msg.into(),
                sqlite_failure: None,
            }),
            app_initialization: None,
        }
    }

    pub(crate) fn from_rusqlite_error(context: impl Into<String>, error: rusqlite::Error) -> Self {
        let context = context.into();
        let sqlite_failure = match &error {
            rusqlite::Error::SqliteFailure(details, message) => Some(SqliteFailureInfo {
                code: details.code,
                extended_code: details.extended_code,
                message: message.clone(),
            }),
            _ => None,
        };

        DbError {
            detail: Box::new(DbErrorDetail {
                message: format!("{context}: {error}"),
                sqlite_failure,
            }),
            app_initialization: None,
        }
    }

    pub(crate) fn from_app_io_error(stage: AppDbStage, error: std::io::Error) -> Self {
        let kind = error.kind();
        let raw_os_error = error.raw_os_error();
        DbError {
            detail: Box::new(DbErrorDetail {
                message: format!("App database initialization failed during {stage:?}: {error}"),
                sqlite_failure: None,
            }),
            app_initialization: Some(Box::new(AppDbFailure {
                stage,
                kind: AppDbFailureKind::Io { kind, raw_os_error },
            })),
        }
    }

    pub(crate) fn from_app_sqlite_error(stage: AppDbStage, error: rusqlite::Error) -> Self {
        let mut db_error = Self::from_rusqlite_error(
            format!("App database initialization failed during {stage:?}"),
            error,
        );
        db_error.app_initialization = Some(Box::new(AppDbFailure {
            stage,
            kind: AppDbFailureKind::Sqlite,
        }));
        db_error
    }

    pub(crate) fn from_app_refinery_error(stage: AppDbStage, error: refinery::Error) -> Self {
        let sqlite_failure = match error.kind() {
            refinery::error::Kind::Connection(_, source) => source
                .downcast_ref::<rusqlite::Error>()
                .and_then(sqlite_failure_info),
            _ => None,
        };
        let kind = match error.kind() {
            refinery::error::Kind::DivergentVersion(applied, expected) => {
                AppDbFailureKind::Divergent {
                    applied: migration_identity(applied),
                    expected: migration_identity(expected),
                }
            }
            refinery::error::Kind::MissingVersion(migration) => AppDbFailureKind::Missing {
                migration: migration_identity(migration),
            },
            refinery::error::Kind::Connection(_, source)
                if source.downcast_ref::<rusqlite::Error>().is_some() =>
            {
                AppDbFailureKind::Sqlite
            }
            _ => AppDbFailureKind::Other,
        };

        DbError {
            detail: Box::new(DbErrorDetail {
                message: format!("App database initialization failed during {stage:?}: {error}"),
                sqlite_failure,
            }),
            app_initialization: Some(Box::new(AppDbFailure { stage, kind })),
        }
    }

    pub(crate) fn sqlite_failure(&self) -> Option<&SqliteFailureInfo> {
        self.detail.sqlite_failure.as_ref()
    }

    #[cfg(any(test, not(bitgarth_db_unit_only)))]
    pub(crate) fn app_initialization(&self) -> Option<&AppDbFailure> {
        self.app_initialization.as_deref()
    }
}

fn sqlite_failure_info(error: &rusqlite::Error) -> Option<SqliteFailureInfo> {
    match error {
        rusqlite::Error::SqliteFailure(details, message) => Some(SqliteFailureInfo {
            code: details.code,
            extended_code: details.extended_code,
            message: message.clone(),
        }),
        _ => None,
    }
}

fn migration_identity(migration: &refinery::Migration) -> MigrationIdentity {
    MigrationIdentity {
        version: migration.version(),
        name: migration.name().to_owned(),
        checksum: migration.checksum(),
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail.message)
    }
}

impl std::error::Error for DbError {}

/// Alias for backwards compatibility
pub(crate) type DbInitError = DbError;
