use crate::db::DbError;
use crate::db::encryption::{
    DbEnvelope, Dek, SqlcipherCompatibility, read_envelope, read_envelope_path,
};
use crate::db::with_db;
use crate::models::{RawPlaintextPassword, RawUsername, UserId, ValidatedUsername};
use crate::project_paths::{get_user_database_path, push_project_dir_override};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use rusqlite::OpenFlags;
use rusqlite::types::ValueRef;
use serde_json::{Map, Value};
use std::ffi::OsString;
use std::fmt;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

const USER_DB_COMMAND: &str = "user-db";
const USER_DB_QUERY_COMMAND: &str = "query";
const USER_DB_SHELL_COMMAND: &str = "shell";
const USER_DB_SQLCIPHER_COMMAND: &str = "sqlcipher";
const DEFAULT_PASSWORD_ENV: &str = "BITGARTH_DB_PASSWORD";
const USER_DB_KEY_COMMAND: &str = "key";
const DEFAULT_SQLCIPHER_BIN: &str = "sqlcipher";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PasswordEnvVar(String);

impl Default for PasswordEnvVar {
    fn default() -> Self {
        Self(DEFAULT_PASSWORD_ENV.to_string())
    }
}

impl PasswordEnvVar {
    fn new(value: String) -> Result<Self, UserDbCliError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(UserDbCliError::usage(
                "--password-env value must not be empty",
            ));
        }
        Ok(Self(trimmed.to_string()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    fn resolve_password_or_prompt(&self) -> Result<RawPlaintextPassword, UserDbCliError> {
        match std::env::var(self.as_str()) {
            Ok(value) if !value.is_empty() => Ok(RawPlaintextPassword::new(value)),
            _ => read_password_from_tty(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QuerySql(String);

impl QuerySql {
    fn new(value: String) -> Result<Self, UserDbCliError> {
        if value.trim().is_empty() {
            return Err(UserDbCliError::usage("--sql value must not be empty"));
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectDirPath(PathBuf);

impl ProjectDirPath {
    fn new(value: String) -> Result<Self, UserDbCliError> {
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            return Err(UserDbCliError::usage(
                "--project-dir must be an absolute path",
            ));
        }
        Ok(Self(path))
    }

    fn as_path_buf(&self) -> PathBuf {
        self.0.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DbFilePath(PathBuf);

impl DbFilePath {
    fn new(value: String) -> Result<Self, UserDbCliError> {
        let path = PathBuf::from(value);
        if path.as_os_str().is_empty() {
            return Err(UserDbCliError::usage("--db-path must not be empty"));
        }
        Ok(Self(path))
    }

    fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SqlcipherBinary(PathBuf);

impl Default for SqlcipherBinary {
    fn default() -> Self {
        Self(PathBuf::from(DEFAULT_SQLCIPHER_BIN))
    }
}

impl SqlcipherBinary {
    fn new(value: String) -> Result<Self, UserDbCliError> {
        let path = PathBuf::from(value);
        if path.as_os_str().is_empty() {
            return Err(UserDbCliError::usage("--sqlcipher-bin must not be empty"));
        }
        Ok(Self(path))
    }

    fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
enum UserLookup {
    UserId(UserId),
    Username(ValidatedUsername),
}

#[derive(Debug, Clone, PartialEq)]
struct CommonArgs {
    user_lookup: UserLookup,
    password_env: PasswordEnvVar,
    project_dir: Option<ProjectDirPath>,
}

#[derive(Debug, Clone, PartialEq)]
struct QueryRequest {
    common: CommonArgs,
    sql: QuerySql,
}

#[derive(Debug, Clone, PartialEq)]
struct ShellRequest {
    common: CommonArgs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SqlcipherRequest {
    db_path: DbFilePath,
    password_env: PasswordEnvVar,
    sqlcipher_bin: SqlcipherBinary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KeyRequest {
    db_path: DbFilePath,
    password_env: PasswordEnvVar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedUserTarget {
    user_id: UserId,
    username: String,
}

#[derive(Debug)]
enum UserDbCommand {
    Help,
    Query(QueryRequest),
    Shell(ShellRequest),
    Sqlcipher(SqlcipherRequest),
    Key(KeyRequest),
}

#[derive(Debug)]
pub(crate) enum UserDbCliError {
    Usage(String),
    Lookup(String),
    Unlock(String),
    Db(String),
    Io(String),
    Json(String),
}

impl UserDbCliError {
    fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }
}

impl fmt::Display for UserDbCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::Lookup(message) => write!(f, "{message}"),
            Self::Unlock(message) => write!(f, "{message}"),
            Self::Db(message) => write!(f, "{message}"),
            Self::Io(message) => write!(f, "{message}"),
            Self::Json(message) => write!(f, "{message}"),
        }
    }
}

impl From<DbError> for UserDbCliError {
    fn from(value: DbError) -> Self {
        Self::Db(value.to_string())
    }
}

pub(crate) fn maybe_run_from_args() -> Result<bool, UserDbCliError> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().and_then(|arg| arg.to_str()) != Some(USER_DB_COMMAND) {
        return Ok(false);
    }

    match parse_command(&args[1..])? {
        UserDbCommand::Help => print_usage(),
        UserDbCommand::Query(request) => run_query_command(request)?,
        UserDbCommand::Shell(request) => run_shell_command(request)?,
        UserDbCommand::Sqlcipher(request) => run_sqlcipher_command(request)?,
        UserDbCommand::Key(request) => run_key_command(request)?,
    }

    Ok(true)
}

fn parse_command(args: &[OsString]) -> Result<UserDbCommand, UserDbCliError> {
    let Some(subcommand) = args.first().and_then(|value| value.to_str()) else {
        return Ok(UserDbCommand::Help);
    };

    match subcommand {
        "help" | "--help" | "-h" => Ok(UserDbCommand::Help),
        USER_DB_QUERY_COMMAND => parse_query_command(&args[1..]).map(UserDbCommand::Query),
        USER_DB_SHELL_COMMAND => parse_shell_command(&args[1..]).map(UserDbCommand::Shell),
        USER_DB_SQLCIPHER_COMMAND => {
            parse_sqlcipher_command(&args[1..]).map(UserDbCommand::Sqlcipher)
        }
        USER_DB_KEY_COMMAND => parse_key_command(&args[1..]).map(UserDbCommand::Key),
        other => Err(UserDbCliError::usage(format!(
            "unknown user-db subcommand '{other}'"
        ))),
    }
}

fn parse_query_command(args: &[OsString]) -> Result<QueryRequest, UserDbCliError> {
    let mut common = parse_common_args(args)?;
    let sql = common
        .sql
        .take()
        .ok_or_else(|| UserDbCliError::usage("missing required --sql flag"))?;
    let common = build_common_args(common)?;
    Ok(QueryRequest { common, sql })
}

fn parse_shell_command(args: &[OsString]) -> Result<ShellRequest, UserDbCliError> {
    let parsed = parse_common_args(args)?;
    if parsed.sql.is_some() {
        return Err(UserDbCliError::usage(
            "--sql is only supported with user-db query",
        ));
    }
    let common = build_common_args(parsed)?;
    Ok(ShellRequest { common })
}

#[derive(Debug, Default)]
struct ParsedSqlcipherArgs {
    db_path: Option<DbFilePath>,
    password_env: Option<PasswordEnvVar>,
    sqlcipher_bin: Option<SqlcipherBinary>,
}

fn parse_sqlcipher_command(args: &[OsString]) -> Result<SqlcipherRequest, UserDbCliError> {
    let mut parsed = ParsedSqlcipherArgs::default();

    let mut index = 0;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or_else(|| UserDbCliError::usage("user-db arguments must be valid UTF-8"))?;

        match flag {
            "--db-path" => {
                let value = next_flag_value(args, &mut index, "--db-path")?;
                parsed.db_path = Some(DbFilePath::new(value.to_string())?);
            }
            "--password-env" => {
                let value = next_flag_value(args, &mut index, "--password-env")?;
                parsed.password_env = Some(PasswordEnvVar::new(value.to_string())?);
            }
            "--sqlcipher-bin" => {
                let value = next_flag_value(args, &mut index, "--sqlcipher-bin")?;
                parsed.sqlcipher_bin = Some(SqlcipherBinary::new(value.to_string())?);
            }
            "--help" | "-h" => {
                return Err(UserDbCliError::usage(
                    "user-db sqlcipher requires --db-path <DB_PATH>",
                ));
            }
            other => {
                return Err(UserDbCliError::usage(format!(
                    "unknown user-db sqlcipher flag '{other}'"
                )));
            }
        }

        index += 1;
    }

    let db_path = parsed
        .db_path
        .ok_or_else(|| UserDbCliError::usage("missing required --db-path flag"))?;

    Ok(SqlcipherRequest {
        db_path,
        password_env: parsed.password_env.unwrap_or_default(),
        sqlcipher_bin: parsed.sqlcipher_bin.unwrap_or_default(),
    })
}

#[derive(Debug, Default)]
struct ParsedCommonArgs {
    user_lookup: Option<UserLookup>,
    password_env: Option<PasswordEnvVar>,
    project_dir: Option<ProjectDirPath>,
    sql: Option<QuerySql>,
}

fn parse_common_args(args: &[OsString]) -> Result<ParsedCommonArgs, UserDbCliError> {
    let mut parsed = ParsedCommonArgs::default();

    let mut index = 0;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or_else(|| UserDbCliError::usage("user-db arguments must be valid UTF-8"))?;

        match flag {
            "--user-id" => {
                if parsed.user_lookup.is_some() {
                    return Err(UserDbCliError::usage(
                        "use either --user-id or --username, not both",
                    ));
                }
                let value = next_flag_value(args, &mut index, "--user-id")?;
                let user_id = UserId::from_str(value).map_err(|err| {
                    UserDbCliError::usage(format!("invalid --user-id value: {err}"))
                })?;
                parsed.user_lookup = Some(UserLookup::UserId(user_id));
            }
            "--username" => {
                if parsed.user_lookup.is_some() {
                    return Err(UserDbCliError::usage(
                        "use either --user-id or --username, not both",
                    ));
                }
                let value = next_flag_value(args, &mut index, "--username")?;
                let username = RawUsername::new(value.to_string())
                    .validate()
                    .map_err(|err| {
                        UserDbCliError::usage(format!("invalid --username value: {err}"))
                    })?;
                parsed.user_lookup = Some(UserLookup::Username(username));
            }
            "--password-env" => {
                let value = next_flag_value(args, &mut index, "--password-env")?;
                parsed.password_env = Some(PasswordEnvVar::new(value.to_string())?);
            }
            "--project-dir" => {
                let value = next_flag_value(args, &mut index, "--project-dir")?;
                parsed.project_dir = Some(ProjectDirPath::new(value.to_string())?);
            }
            "--sql" => {
                let value = next_flag_value(args, &mut index, "--sql")?;
                parsed.sql = Some(QuerySql::new(value.to_string())?);
            }
            "--help" | "-h" => return Ok(ParsedCommonArgs::default()),
            other => {
                return Err(UserDbCliError::usage(format!(
                    "unknown user-db flag '{other}'"
                )));
            }
        }

        index += 1;
    }

    Ok(parsed)
}

fn build_common_args(parsed: ParsedCommonArgs) -> Result<CommonArgs, UserDbCliError> {
    let user_lookup = parsed
        .user_lookup
        .ok_or_else(|| UserDbCliError::usage("one of --user-id or --username is required"))?;

    Ok(CommonArgs {
        user_lookup,
        password_env: parsed.password_env.unwrap_or_default(),
        project_dir: parsed.project_dir,
    })
}

fn next_flag_value<'a>(
    args: &'a [OsString],
    index: &mut usize,
    flag: &str,
) -> Result<&'a str, UserDbCliError> {
    let value = args
        .get(*index + 1)
        .and_then(|value| value.to_str())
        .ok_or_else(|| UserDbCliError::usage(format!("missing value for {flag}")))?;
    *index += 1;
    Ok(value)
}

fn print_usage() {
    println!(
        "Usage:
  user-db query (--user-id <USER_ID> | --username <USERNAME>) [--project-dir <ABS_PATH>] [--password-env <ENV_VAR>] --sql <SQL>
  user-db shell (--user-id <USER_ID> | --username <USERNAME>) [--project-dir <ABS_PATH>] [--password-env <ENV_VAR>]
  user-db sqlcipher --db-path <DB_PATH> [--password-env <ENV_VAR>] [--sqlcipher-bin <PATH>]
  user-db key --db-path <DB_PATH> [--password-env <ENV_VAR>]

The password is read from the environment variable named by --password-env when present.
Default password env var: {DEFAULT_PASSWORD_ENV}

If the env var is unset, the helper will prompt for the password interactively.

Examples:
  export {DEFAULT_PASSWORD_ENV}='your-password'
  ./scripts/user-db query --username alice --sql \"SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name\"
  ./scripts/user-db shell --user-id 01ARZ3NDEKTSV4RRFFQ69G5FAV
  ./scripts/user-db sqlcipher --db-path /path/to/u01ARZ3NDEKTSV4RRFFQ69G5FAV.db --sqlcipher-bin /usr/local/Cellar/sqlcipher/4.14.0/bin/sqlcipher
  ./scripts/user-db key --db-path /path/to/u01ARZ3NDEKTSV4RRFFQ69G5FAV.db

Notes:
  - The helper opens the user DB read-only.
  - If you run BitGarth with a custom project dir, either export BITGARTH_PROJECT_DIR or pass --project-dir.
  - The interactive shell supports .tables, .schema [pattern], .help, .quit, and one-line SQL statements."
    );
}

fn run_query_command(request: QueryRequest) -> Result<(), UserDbCliError> {
    let _project_dir_guard = apply_project_dir_override(request.common.project_dir.as_ref())?;
    let target = resolve_user_target(&request.common.user_lookup)?;
    let password = request.common.password_env.resolve_password_or_prompt()?;
    let conn = open_user_db_connection(target.user_id, &password)?;
    let result = run_sql(&conn, &request.sql)?;
    print_query_result(&result)?;
    Ok(())
}

fn run_shell_command(request: ShellRequest) -> Result<(), UserDbCliError> {
    let _project_dir_guard = apply_project_dir_override(request.common.project_dir.as_ref())?;
    let target = resolve_user_target(&request.common.user_lookup)?;
    let password = request.common.password_env.resolve_password_or_prompt()?;
    let conn = open_user_db_connection(target.user_id, &password)?;

    println!(
        "Opened read-only user DB for {} ({})",
        target.username, target.user_id
    );
    println!("Type .help for commands.");

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut lines = stdin.lock().lines();

    loop {
        write!(stdout, "bitgarth:{}> ", target.user_id)
            .map_err(|err| UserDbCliError::Io(format!("failed to write prompt: {err}")))?;
        stdout
            .flush()
            .map_err(|err| UserDbCliError::Io(format!("failed to flush prompt: {err}")))?;

        let Some(line) = lines.next() else {
            break;
        };
        let line =
            line.map_err(|err| UserDbCliError::Io(format!("failed to read shell input: {err}")))?;

        match parse_shell_input(&line)? {
            ShellInput::Empty => {}
            ShellInput::Help => print_shell_help(),
            ShellInput::Quit => break,
            ShellInput::Tables => print_tables(&conn)?,
            ShellInput::Schema(pattern) => print_schema(&conn, pattern.as_deref())?,
            ShellInput::Sql(sql) => {
                let result = run_sql(&conn, &sql)?;
                print_query_result(&result)?;
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct ResolvedDbAccess {
    db_path: PathBuf,
    envelope_path: PathBuf,
    encrypted_open: Option<(Dek, SqlcipherCompatibility)>,
}

fn run_sqlcipher_command(request: SqlcipherRequest) -> Result<(), UserDbCliError> {
    let password = request.password_env.resolve_password_or_prompt()?;
    let access = resolve_db_access_from_path(request.db_path.as_path(), &password)?;

    let mut command = Command::new(request.sqlcipher_bin.as_path());
    command.arg("-readonly");

    if let Some((dek, sqlcipher_compatibility)) = &access.encrypted_open {
        command
            .arg("-cmd")
            .arg(format!("PRAGMA key = \"x'{}'\";", dek.as_hex()))
            .arg("-cmd")
            .arg(format!(
                "PRAGMA cipher_compatibility = {};",
                sqlcipher_compatibility.as_u32()
            ));
    }

    command
        .arg("-cmd")
        .arg("PRAGMA query_only = ON;")
        .arg(access.db_path.as_os_str());

    let status = command.status().map_err(|err| {
        UserDbCliError::Io(format!(
            "failed to start sqlcipher at {} for {}: {err}",
            request.sqlcipher_bin.as_path().display(),
            access.db_path.display()
        ))
    })?;

    if !status.success() {
        return Err(UserDbCliError::Db(format!(
            "sqlcipher exited with status {} while opening {} using envelope {}",
            status,
            access.db_path.display(),
            access.envelope_path.display()
        )));
    }

    Ok(())
}

fn parse_key_command(args: &[OsString]) -> Result<KeyRequest, UserDbCliError> {
    let mut db_path: Option<DbFilePath> = None;
    let mut password_env: Option<PasswordEnvVar> = None;

    let mut index = 0;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .ok_or_else(|| UserDbCliError::usage("user-db arguments must be valid UTF-8"))?;

        match flag {
            "--db-path" => {
                let value = next_flag_value(args, &mut index, "--db-path")?;
                db_path = Some(DbFilePath::new(value.to_string())?);
            }
            "--password-env" => {
                let value = next_flag_value(args, &mut index, "--password-env")?;
                password_env = Some(PasswordEnvVar::new(value.to_string())?);
            }
            "--help" | "-h" => {
                return Err(UserDbCliError::usage(
                    "user-db key requires --db-path <DB_PATH>",
                ));
            }
            other => {
                return Err(UserDbCliError::usage(format!(
                    "unknown user-db key flag '{other}'"
                )));
            }
        }

        index += 1;
    }

    let db_path =
        db_path.ok_or_else(|| UserDbCliError::usage("missing required --db-path flag"))?;

    Ok(KeyRequest {
        db_path,
        password_env: password_env.unwrap_or_default(),
    })
}

fn run_key_command(request: KeyRequest) -> Result<(), UserDbCliError> {
    let password = request.password_env.resolve_password_or_prompt()?;
    let access = resolve_db_access_from_path(request.db_path.as_path(), &password)?;

    if let Some((dek, sqlcipher_compatibility)) = &access.encrypted_open {
        println!("PRAGMA key = \"x'{}'\";", dek.as_hex());
        println!(
            "PRAGMA cipher_compatibility = {};",
            sqlcipher_compatibility.as_u32()
        );
    } else {
        eprintln!("Database is not encrypted (dev-config mode).");
    }

    Ok(())
}

fn read_password_from_tty() -> Result<RawPlaintextPassword, UserDbCliError> {
    eprint!("Password: ");
    io::stderr()
        .flush()
        .map_err(|err| UserDbCliError::Io(format!("failed to flush stderr: {err}")))?;

    let stty_echo_off = Command::new("stty")
        .arg("-echo")
        .status()
        .map_err(|err| UserDbCliError::Io(format!("failed to disable terminal echo: {err}")))?;

    if !stty_echo_off.success() {
        return Err(UserDbCliError::Io(
            "failed to disable terminal echo".to_string(),
        ));
    }

    let mut password = String::new();
    let read_result = io::stdin().read_line(&mut password);

    // Always re-enable echo, even if reading failed.
    let _ = Command::new("stty").arg("echo").status();
    eprintln!();

    read_result.map_err(|err| UserDbCliError::Io(format!("failed to read password: {err}")))?;

    let trimmed = password.trim_end_matches('\n').trim_end_matches('\r');
    if trimmed.is_empty() {
        return Err(UserDbCliError::usage("password must not be empty"));
    }

    Ok(RawPlaintextPassword::new(trimmed.to_string()))
}

fn resolve_db_access_from_path(
    db_path: &Path,
    password: &RawPlaintextPassword,
) -> Result<ResolvedDbAccess, UserDbCliError> {
    let db_path = db_path.to_path_buf();
    let envelope_path = envelope_path_for_db_path(&db_path);

    if !db_path.exists() {
        return Err(UserDbCliError::Lookup(format!(
            "database file not found at {}",
            db_path.display()
        )));
    }

    let envelope = read_envelope_path(&envelope_path).map_err(|err| {
        UserDbCliError::Unlock(format!(
            "failed to read user DB envelope at {}: {err}",
            envelope_path.display()
        ))
    })?;

    let encrypted_open = encrypted_open_from_envelope(&envelope, password, &envelope_path)?;

    Ok(ResolvedDbAccess {
        db_path,
        envelope_path,
        encrypted_open,
    })
}

fn envelope_path_for_db_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("json")
}

fn encrypted_open_from_envelope(
    envelope: &DbEnvelope,
    password: &RawPlaintextPassword,
    envelope_path: &Path,
) -> Result<Option<(Dek, SqlcipherCompatibility)>, UserDbCliError> {
    match envelope {
        DbEnvelope::Encrypted {
            sqlcipher_version, ..
        } => {
            let dek = envelope
                .unwrap_with_password(password.as_str())
                .map_err(|err| {
                    UserDbCliError::Unlock(format!(
                        "failed to unwrap the encrypted user DB key from {}: {err}",
                        envelope_path.display()
                    ))
                })?;
            Ok(Some((dek, sqlcipher_version.clone())))
        }
        #[cfg(feature = "dev-config")]
        DbEnvelope::UnencryptedDev => Ok(None),
    }
}

fn apply_project_dir_override(
    project_dir: Option<&ProjectDirPath>,
) -> Result<Option<crate::project_paths::ProjectDirOverrideGuard>, UserDbCliError> {
    match project_dir {
        Some(project_dir) => push_project_dir_override(project_dir.as_path_buf())
            .map(Some)
            .map_err(|err| {
                UserDbCliError::usage(format!("failed to apply --project-dir override: {err}"))
            }),
        None => Ok(None),
    }
}

fn resolve_user_target(user_lookup: &UserLookup) -> Result<ResolvedUserTarget, UserDbCliError> {
    match user_lookup {
        UserLookup::UserId(user_id) => resolve_user_target_by_id(*user_id),
        UserLookup::Username(username) => resolve_user_target_by_username(username),
    }
}

fn resolve_user_target_by_id(user_id: UserId) -> Result<ResolvedUserTarget, UserDbCliError> {
    let result: Result<Option<String>, DbError> = with_db(|conn| {
        let mut stmt = conn
            .prepare("SELECT username FROM users WHERE user_id = ?1")
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    format!("failed to prepare username lookup for user {user_id}"),
                    err,
                )
            })?;
        let query = stmt.query_row([user_id.to_string()], |row| row.get::<_, String>(0));
        match query {
            Ok(username) => Ok(Some(username)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(DbError::from_rusqlite_error(
                format!("failed to resolve username for user {user_id}"),
                err,
            )),
        }
    });

    let Some(username) = result? else {
        return Err(UserDbCliError::Lookup(format!(
            "no user found for user ID {user_id}"
        )));
    };

    Ok(ResolvedUserTarget { user_id, username })
}

fn resolve_user_target_by_username(
    username: &ValidatedUsername,
) -> Result<ResolvedUserTarget, UserDbCliError> {
    let result: Result<Option<UserId>, DbError> = with_db(|conn| {
        let mut stmt = conn
            .prepare("SELECT user_id FROM users WHERE username = ?1")
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    format!("failed to prepare user ID lookup for {}", username.as_str()),
                    err,
                )
            })?;
        let query = stmt.query_row([username.as_str()], |row| row.get::<_, String>(0));
        match query {
            Ok(user_id_text) => {
                let user_id = UserId::from_str(&user_id_text).map_err(|err| {
                    DbError::new(format!(
                        "invalid user_id ULID in users table for {}: {err}",
                        username.as_str()
                    ))
                })?;
                Ok(Some(user_id))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(DbError::from_rusqlite_error(
                format!("failed to resolve user ID for {}", username.as_str()),
                err,
            )),
        }
    });

    let Some(user_id) = result? else {
        return Err(UserDbCliError::Lookup(format!(
            "no user found for username {}",
            username.as_str()
        )));
    };

    Ok(ResolvedUserTarget {
        user_id,
        username: username.as_str().to_string(),
    })
}

fn open_user_db_connection(
    user_id: UserId,
    password: &RawPlaintextPassword,
) -> Result<rusqlite::Connection, UserDbCliError> {
    let db_path = get_user_database_path(user_id)
        .map_err(|err| UserDbCliError::Db(format!("failed to resolve user DB path: {err}")))?;
    let envelope = read_envelope(user_id).map_err(|err| {
        UserDbCliError::Unlock(format!(
            "failed to read user DB envelope for {user_id}: {err}"
        ))
    })?;
    let envelope_path = envelope_path_for_db_path(&db_path);
    let encrypted_open = encrypted_open_from_envelope(&envelope, password, &envelope_path)?;

    let conn = rusqlite::Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| {
            UserDbCliError::Db(format!(
                "failed to open user DB at {}: {err}",
                db_path.display()
            ))
        })?;

    if let Some((dek, sqlcipher_version)) = encrypted_open {
        apply_encrypted_db_pragmas(&conn, &dek, &sqlcipher_version)?;
    }

    conn.pragma_update(None, "query_only", "ON")
        .map_err(|err| UserDbCliError::Db(format!("failed to enable query_only mode: {err}")))?;

    conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|err| {
        UserDbCliError::Unlock(format!(
            "user DB opened but could not be queried; the password may be wrong or the DB/envelope may be incompatible: {err}"
        ))
    })?;

    Ok(conn)
}

fn apply_encrypted_db_pragmas(
    conn: &rusqlite::Connection,
    dek: &Dek,
    compatibility: &SqlcipherCompatibility,
) -> Result<(), UserDbCliError> {
    let key = format!("x'{}'", dek.as_hex());
    conn.execute_batch(&format!("PRAGMA key = \"{}\"", key))
        .map_err(|err| UserDbCliError::Db(format!("failed to set SQLCipher key: {err}")))?;
    conn.pragma_update(
        None,
        "cipher_compatibility",
        compatibility.as_u32().to_string(),
    )
    .map_err(|err| {
        UserDbCliError::Db(format!(
            "failed to set SQLCipher compatibility to {}: {err}",
            compatibility.as_u32()
        ))
    })?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
enum SqlRunResult {
    Rows(Vec<Value>),
    StatementComplete,
}

fn run_sql(conn: &rusqlite::Connection, sql: &QuerySql) -> Result<SqlRunResult, UserDbCliError> {
    let mut stmt = conn
        .prepare(sql.as_str())
        .map_err(|err| UserDbCliError::Db(format!("failed to prepare SQL statement: {err}")))?;

    let column_count = stmt.column_count();
    if column_count == 0 {
        conn.execute_batch(sql.as_str())
            .map_err(|err| UserDbCliError::Db(format!("failed to execute SQL statement: {err}")))?;
        return Ok(SqlRunResult::StatementComplete);
    }

    let column_names = stmt
        .column_names()
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>();

    let mut rows = stmt
        .query([])
        .map_err(|err| UserDbCliError::Db(format!("failed to execute SQL query: {err}")))?;

    let mut result_rows = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|err| UserDbCliError::Db(format!("failed to read SQL row: {err}")))?
    {
        result_rows.push(row_to_json(row, &column_names)?);
    }

    Ok(SqlRunResult::Rows(result_rows))
}

fn row_to_json(row: &rusqlite::Row<'_>, column_names: &[String]) -> Result<Value, UserDbCliError> {
    let mut object = Map::new();
    for (index, column_name) in column_names.iter().enumerate() {
        let value = row.get_ref(index).map_err(|err| {
            UserDbCliError::Db(format!("failed to read column {column_name}: {err}"))
        })?;
        object.insert(column_name.clone(), value_ref_to_json(value));
    }
    Ok(Value::Object(object))
}

fn value_ref_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::from(value),
        ValueRef::Real(value) => Value::from(value),
        ValueRef::Text(value) => Value::String(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(value) => Value::String(format!("base64:{}", BASE64.encode(value))),
    }
}

fn print_query_result(result: &SqlRunResult) -> Result<(), UserDbCliError> {
    match result {
        SqlRunResult::Rows(rows) => {
            let json = serde_json::to_string_pretty(rows)
                .map_err(|err| UserDbCliError::Json(format!("failed to render JSON: {err}")))?;
            println!("{json}");
        }
        SqlRunResult::StatementComplete => {
            println!("statement executed successfully (no rows returned)");
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ShellInput {
    Empty,
    Help,
    Quit,
    Tables,
    Schema(Option<String>),
    Sql(QuerySql),
}

fn parse_shell_input(input: &str) -> Result<ShellInput, UserDbCliError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(ShellInput::Empty);
    }

    match trimmed {
        ".help" => Ok(ShellInput::Help),
        ".quit" | ".exit" => Ok(ShellInput::Quit),
        ".tables" => Ok(ShellInput::Tables),
        _ if trimmed.starts_with(".schema") => {
            let pattern = trimmed
                .strip_prefix(".schema")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            Ok(ShellInput::Schema(pattern))
        }
        _ => Ok(ShellInput::Sql(QuerySql::new(trimmed.to_string())?)),
    }
}

fn print_shell_help() {
    println!(
        "Shell commands:
  .tables             List non-internal tables
  .schema             Print schema for all objects
  .schema <pattern>   Print schema for objects whose name matches the SQLite LIKE pattern
  .help               Show this help
  .quit / .exit       Exit the shell

SQL statements must fit on one line."
    );
}

fn print_tables(conn: &rusqlite::Connection) -> Result<(), UserDbCliError> {
    let sql = QuerySql::new(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
            .to_string(),
    )?;
    let SqlRunResult::Rows(rows) = run_sql(conn, &sql)? else {
        return Ok(());
    };

    for row in rows {
        if let Some(name) = row.get("name").and_then(Value::as_str) {
            println!("{name}");
        }
    }

    Ok(())
}

fn print_schema(conn: &rusqlite::Connection, pattern: Option<&str>) -> Result<(), UserDbCliError> {
    let (sql_text, pattern_value) = match pattern {
        Some(pattern) => (
            "SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL AND name LIKE ?1 ORDER BY type, name",
            Some(pattern.to_string()),
        ),
        None => (
            "SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY type, name",
            None,
        ),
    };

    let mut stmt = conn
        .prepare(sql_text)
        .map_err(|err| UserDbCliError::Db(format!("failed to prepare schema query: {err}")))?;

    let mut rows = match pattern_value {
        Some(pattern) => stmt
            .query([pattern])
            .map_err(|err| UserDbCliError::Db(format!("failed to query schema: {err}")))?,
        None => stmt
            .query([])
            .map_err(|err| UserDbCliError::Db(format!("failed to query schema: {err}")))?,
    };

    let mut first = true;
    while let Some(row) = rows
        .next()
        .map_err(|err| UserDbCliError::Db(format!("failed to read schema row: {err}")))?
    {
        let name: String = row
            .get(0)
            .map_err(|err| UserDbCliError::Db(format!("failed to read schema name: {err}")))?;
        let sql: String = row
            .get(1)
            .map_err(|err| UserDbCliError::Db(format!("failed to read schema SQL: {err}")))?;

        if !first {
            println!();
        }
        first = false;
        println!("-- {name}");
        println!("{sql};");
    }

    Ok(())
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    #[cfg(feature = "db-tests")]
    use crate::backend::register;
    #[cfg(feature = "db-tests")]
    use crate::db;
    #[cfg(feature = "db-tests")]
    use crate::models::RawPlaintextPassword;

    #[cfg(not(bitgarth_db_unit_only))]
    fn parse(args: &[&str]) -> Result<UserDbCommand, UserDbCliError> {
        let values = args.iter().map(OsString::from).collect::<Vec<_>>();
        parse_command(&values)
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn parse_query_command_with_username_and_defaults() {
        let command = parse(&["query", "--username", "alice", "--sql", "SELECT 1 AS one"])
            .expect("query should parse");

        let UserDbCommand::Query(request) = command else {
            panic!("expected query command");
        };

        assert_eq!(
            request.common.password_env,
            PasswordEnvVar::new(DEFAULT_PASSWORD_ENV.to_string())
                .expect("default env should parse")
        );
        assert_eq!(
            request.sql,
            QuerySql::new("SELECT 1 AS one".to_string()).unwrap()
        );
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn parse_shell_command_rejects_sql_flag() {
        let error = parse(&[
            "shell",
            "--user-id",
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "--sql",
            "SELECT 1",
        ])
        .expect_err("shell should reject sql");

        assert!(matches!(error, UserDbCliError::Usage(_)));
        assert!(error.to_string().contains("--sql"));
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn parse_sqlcipher_command_with_db_path_and_default_binary() {
        let command = parse(&["sqlcipher", "--db-path", "/tmp/example.db"])
            .expect("sqlcipher command should parse");

        let UserDbCommand::Sqlcipher(request) = command else {
            panic!("expected sqlcipher command");
        };

        assert_eq!(
            request.db_path,
            DbFilePath::new("/tmp/example.db".to_string()).unwrap()
        );
        assert_eq!(request.sqlcipher_bin, SqlcipherBinary::default());
    }

    #[cfg(not(bitgarth_db_unit_only))]
    #[test]
    fn parse_shell_input_supports_meta_commands() {
        assert_eq!(parse_shell_input("").unwrap(), ShellInput::Empty);
        assert_eq!(parse_shell_input(".help").unwrap(), ShellInput::Help);
        assert_eq!(parse_shell_input(".tables").unwrap(), ShellInput::Tables);
        assert_eq!(
            parse_shell_input(".schema account_%").unwrap(),
            ShellInput::Schema(Some("account_%".to_string()))
        );
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn open_user_db_connection_unlocks_encrypted_user_db() {
        let _guard = db::acquire_test_runtime().expect("test runtime should initialize");

        let username = "query_user".to_string();
        let password = RawPlaintextPassword::new("SecurePass123".to_string());
        let response = register(
            RawUsername::new(username.clone()),
            password.clone(),
            Some(crate::legal::current_registration_acknowledgement()),
        )
        .await
        .expect("register should succeed");

        let conn = open_user_db_connection(response.user.user_id, &password)
            .expect("password should unlock encrypted user db");

        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .expect("query should succeed");

        assert!(table_count > 0);

        let resolved = resolve_user_target(&UserLookup::Username(
            RawUsername::new(username)
                .validate()
                .expect("username should validate"),
        ))
        .expect("username should resolve");
        assert_eq!(resolved.user_id, response.user.user_id);

        let db_path =
            get_user_database_path(response.user.user_id).expect("user db path should resolve");
        let access = resolve_db_access_from_path(&db_path, &password)
            .expect("db path should resolve to encrypted access");
        assert_eq!(access.db_path, db_path);
        assert!(access.encrypted_open.is_some());
    }

    #[cfg(feature = "db-tests")]
    #[tokio::test(flavor = "current_thread")]
    async fn open_user_db_connection_rejects_wrong_password() {
        let _guard = db::acquire_test_runtime().expect("test runtime should initialize");

        let password = RawPlaintextPassword::new("SecurePass123".to_string());
        let response = register(
            RawUsername::new("wrong_password_user".to_string()),
            password,
            Some(crate::legal::current_registration_acknowledgement()),
        )
        .await
        .expect("register should succeed");

        let error = open_user_db_connection(
            response.user.user_id,
            &RawPlaintextPassword::new("WrongPass123".to_string()),
        )
        .expect_err("wrong password should fail");

        assert!(error.to_string().contains("unwrap"));
    }
}
