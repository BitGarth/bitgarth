//! Database error types

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqliteFailureInfo {
    pub(crate) code: rusqlite::ErrorCode,
    pub(crate) extended_code: i32,
    pub(crate) message: Option<String>,
}

/// Database initialization or operation error
#[derive(Debug, Clone)]
pub struct DbError {
    message: String,
    sqlite_failure: Option<SqliteFailureInfo>,
}

impl DbError {
    pub fn new(msg: impl Into<String>) -> Self {
        DbError {
            message: msg.into(),
            sqlite_failure: None,
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
            message: format!("{context}: {error}"),
            sqlite_failure,
        }
    }

    pub(crate) fn sqlite_failure(&self) -> Option<&SqliteFailureInfo> {
        self.sqlite_failure.as_ref()
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for DbError {}

/// Alias for backwards compatibility
pub(crate) type DbInitError = DbError;
