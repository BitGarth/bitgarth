//! Project paths and filesystem helpers.
//!
//! Pure path builders live alongside small side-effecting helpers that
//! ensure directories exist when needed.

use crate::db::DbError;
use crate::models::UserId;
use directories::ProjectDirs;
use once_cell::sync::Lazy;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(crate) const PROJECT_DIR_OVERRIDE_ENV: &str = "BITGARTH_PROJECT_DIR";
static PROCESS_PROJECT_DIR_OVERRIDE: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));

pub(crate) struct ProjectDirOverrideGuard {
    previous: Option<PathBuf>,
}

impl Drop for ProjectDirOverrideGuard {
    fn drop(&mut self) {
        let mut guard = match PROCESS_PROJECT_DIR_OVERRIDE.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = self.previous.take();
    }
}

fn parse_project_dir_override(value: &OsStr) -> Result<PathBuf, DbError> {
    let path = PathBuf::from(value);

    if value.is_empty() || value.to_str().is_some_and(|v| v.trim().is_empty()) {
        return Err(DbError::new(format!(
            "Environment variable {PROJECT_DIR_OVERRIDE_ENV} is set but empty"
        )));
    }

    if !path.is_absolute() {
        return Err(DbError::new(format!(
            "Environment variable {PROJECT_DIR_OVERRIDE_ENV} must be an absolute path: {path:?}"
        )));
    }

    std::fs::create_dir_all(&path).map_err(|e| {
        DbError::new(format!(
            "Environment variable {PROJECT_DIR_OVERRIDE_ENV} could not create directory {path:?}: {e}"
        ))
    })?;

    if !path.is_dir() {
        return Err(DbError::new(format!(
            "Environment variable {PROJECT_DIR_OVERRIDE_ENV} must point to a directory: {path:?}"
        )));
    }

    Ok(path)
}

/// Resolve the base project directory for the application.
///
/// If `BITGARTH_PROJECT_DIR` is set, it must point to an existing absolute
/// directory and that path is used as the project root.
pub(crate) fn get_project_dir() -> Result<PathBuf, DbError> {
    if let Some(runtime_context) = crate::runtime_context::current_runtime_context() {
        return Ok(runtime_context.project_dir().to_path_buf());
    }

    let process_override = {
        let guard = match PROCESS_PROJECT_DIR_OVERRIDE.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.clone()
    };
    if let Some(path) = process_override {
        return Ok(path);
    }

    if let Some(override_value) = std::env::var_os(PROJECT_DIR_OVERRIDE_ENV) {
        return parse_project_dir_override(&override_value);
    }

    default_project_dir()
}

fn default_project_dir() -> Result<PathBuf, DbError> {
    let proj_dirs = ProjectDirs::from("app.bitgarth", "", "bitgarth")
        .ok_or_else(|| DbError::new("Failed to determine application data directory"))?;

    Ok(proj_dirs.data_dir().to_path_buf())
}

pub(crate) fn push_project_dir_override(path: PathBuf) -> Result<ProjectDirOverrideGuard, DbError> {
    let metadata = std::fs::metadata(&path).map_err(|e| {
        DbError::new(format!(
            "Project dir override points to an invalid path {path:?}: {e}"
        ))
    })?;
    if !metadata.is_dir() {
        return Err(DbError::new(format!(
            "Project dir override must point to a directory: {path:?}"
        )));
    }

    let mut guard = match PROCESS_PROJECT_DIR_OVERRIDE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let previous = guard.replace(path);
    Ok(ProjectDirOverrideGuard { previous })
}

/// Side-effecting function: ensure a directory exists.
pub(crate) fn ensure_dir_exists(path: &Path) -> Result<(), DbError> {
    std::fs::create_dir_all(path)
        .map_err(|e| DbError::new(format!("Failed to create directory at {:?}: {}", path, e)))
}

/// Pure function: build `{project_dir}/app`.
fn app_dir_from_project_dir(project_dir: &Path) -> PathBuf {
    project_dir.join("app")
}

/// Pure function: build `{app_dir}/data`.
fn app_data_dir_from_app_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("data")
}

/// Pure function: build `{app_data_dir}/app.db`.
fn app_database_path_from_app_data_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("app.db")
}

/// Pure function: build `{project_dir}/app/data/app.db`.
pub(crate) fn app_database_path_from_project_dir(project_dir: &Path) -> PathBuf {
    let app_dir = app_dir_from_project_dir(project_dir);
    let app_data_dir = app_data_dir_from_app_dir(&app_dir);
    app_database_path_from_app_data_dir(&app_data_dir)
}

/// Side-effecting function: get `{project_dir}/app/data/app.db`.
pub(crate) fn get_app_database_path() -> Result<PathBuf, DbError> {
    let project_dir = get_project_dir()?;
    let app_data_dir = app_data_dir_from_app_dir(&app_dir_from_project_dir(&project_dir));
    ensure_dir_exists(&app_data_dir)?;
    Ok(app_database_path_from_project_dir(&project_dir))
}

/// Pure function: build `{app_data_dir}/prices`.
fn price_data_dir_from_app_data_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("prices")
}

/// Pure function: build `{price_data_dir}/prices.db`.
fn price_database_path_from_price_data_dir(price_data_dir: &Path) -> PathBuf {
    price_data_dir.join("prices.db")
}

/// Pure function: build `{project_dir}/app/data/prices/prices.db`.
pub(crate) fn price_database_path_from_project_dir(project_dir: &Path) -> PathBuf {
    let app_dir = app_dir_from_project_dir(project_dir);
    let app_data_dir = app_data_dir_from_app_dir(&app_dir);
    let price_data_dir = price_data_dir_from_app_data_dir(&app_data_dir);
    price_database_path_from_price_data_dir(&price_data_dir)
}

/// Side-effecting function: get `{project_dir}/app/data/prices/prices.db`.
pub(crate) fn get_price_database_path() -> Result<PathBuf, DbError> {
    let project_dir = get_project_dir()?;
    let app_dir = app_dir_from_project_dir(&project_dir);
    let app_data_dir = app_data_dir_from_app_dir(&app_dir);
    let price_data_dir = price_data_dir_from_app_data_dir(&app_data_dir);
    ensure_dir_exists(&price_data_dir)?;
    Ok(price_database_path_from_project_dir(&project_dir))
}

/// Pure function: build `{project_dir}/users/{user_id}`.
pub(crate) fn user_dir_from_project_dir(project_dir: &Path, user_id: UserId) -> PathBuf {
    project_dir.join("users").join(user_id.to_string())
}

/// Pure function: build `{user_dir}/data`.
fn user_data_dir_from_user_dir(user_dir: &Path) -> PathBuf {
    user_dir.join("data")
}

/// Pure function: build `{user_data_dir}/u{user_id}.db`.
fn user_database_path_from_user_data_dir(user_data_dir: &Path, user_id: UserId) -> PathBuf {
    user_data_dir.join(format!("u{user_id}.db"))
}

/// Pure function: build `{project_dir}/users/{user_id}/data/u{user_id}.db`.
pub(crate) fn user_database_path_from_project_dir(project_dir: &Path, user_id: UserId) -> PathBuf {
    let user_dir = user_dir_from_project_dir(project_dir, user_id);
    let user_data_dir = user_data_dir_from_user_dir(&user_dir);
    user_database_path_from_user_data_dir(&user_data_dir, user_id)
}

/// Pure function: build `{user_data_dir}/u{user_id}.json`.
#[cfg(feature = "server")]
fn user_envelope_path_from_user_data_dir(user_data_dir: &Path, user_id: UserId) -> PathBuf {
    user_data_dir.join(format!("u{user_id}.json"))
}

/// Pure function: build `{project_dir}/users/{user_id}/data/u{user_id}.json`.
#[cfg(feature = "server")]
pub(crate) fn user_envelope_path_from_project_dir(project_dir: &Path, user_id: UserId) -> PathBuf {
    let user_dir = user_dir_from_project_dir(project_dir, user_id);
    let user_data_dir = user_data_dir_from_user_dir(&user_dir);
    user_envelope_path_from_user_data_dir(&user_data_dir, user_id)
}

/// Pure function: build `{user_dir}/traces`.
fn user_traces_dir_from_user_dir(user_dir: &Path) -> PathBuf {
    user_dir.join("traces")
}

/// Pure function: get `{project_dir}/users/{user_id}`.
pub(crate) fn get_user_dir(user_id: UserId) -> Result<PathBuf, DbError> {
    let project_dir = get_project_dir()?;
    Ok(user_dir_from_project_dir(&project_dir, user_id))
}

/// Side-effecting function: get `{project_dir}/users/{user_id}/data`, ensuring it exists.
pub(crate) fn get_user_data_dir(user_id: UserId) -> Result<PathBuf, DbError> {
    let user_dir = get_user_dir(user_id)?;
    let user_data_dir = user_data_dir_from_user_dir(&user_dir);
    ensure_dir_exists(&user_data_dir)?;
    Ok(user_data_dir)
}

/// Side-effecting function: get `{project_dir}/users/{user_id}/data/u{user_id}.db`.
pub(crate) fn get_user_database_path(user_id: UserId) -> Result<PathBuf, DbError> {
    let user_data_dir = get_user_data_dir(user_id)?;
    let project_dir = get_project_dir()?;
    debug_assert_eq!(
        user_database_path_from_project_dir(&project_dir, user_id),
        user_database_path_from_user_data_dir(&user_data_dir, user_id)
    );
    Ok(user_database_path_from_project_dir(&project_dir, user_id))
}

/// Side-effecting function: get `{project_dir}/users/{user_id}/data/u{user_id}.json`.
#[cfg(feature = "server")]
pub(crate) fn get_user_envelope_path(user_id: UserId) -> Result<PathBuf, DbError> {
    let user_data_dir = get_user_data_dir(user_id)?;
    let project_dir = get_project_dir()?;
    debug_assert_eq!(
        user_envelope_path_from_project_dir(&project_dir, user_id),
        user_envelope_path_from_user_data_dir(&user_data_dir, user_id)
    );
    Ok(user_envelope_path_from_project_dir(&project_dir, user_id))
}

/// Pure function: get `{project_dir}/users/{user_id}/traces`.
pub(crate) fn get_user_traces_dir(user_id: UserId) -> Result<PathBuf, DbError> {
    let user_dir = get_user_dir(user_id)?;
    Ok(user_traces_dir_from_user_dir(&user_dir))
}

/// Pure function: build `{hledger_dir}/{owner_segment}`.
#[cfg(test)]
pub(crate) fn hledger_owner_dir(hledger_dir: &Path, owner_segment: &str) -> PathBuf {
    hledger_dir.join(owner_segment)
}

/// Pure function: build `{hledger_dir}/{owner_segment}/{wallet_segment}`.
#[cfg(test)]
pub(crate) fn hledger_owner_wallet_dir(
    hledger_dir: &Path,
    owner_segment: &str,
    wallet_segment: &str,
) -> PathBuf {
    hledger_owner_dir(hledger_dir, owner_segment).join(wallet_segment)
}

/// Pure function: build `{hledger_dir}/{owner_segment}/{wallet_segment}/{account_segment}`.
#[cfg(test)]
pub(crate) fn hledger_owner_account_dir(
    hledger_dir: &Path,
    owner_segment: &str,
    wallet_segment: &str,
    account_segment: &str,
) -> PathBuf {
    hledger_owner_wallet_dir(hledger_dir, owner_segment, wallet_segment).join(account_segment)
}

/// Pure function: build `{hledger_dir}/{owner_segment}/{wallet_segment}/{account_segment}/journal/{year}`.
#[cfg(test)]
pub(crate) fn hledger_owner_account_year_journal_dir(
    hledger_dir: &Path,
    owner_segment: &str,
    wallet_segment: &str,
    account_segment: &str,
    year: &str,
) -> PathBuf {
    hledger_owner_account_dir(hledger_dir, owner_segment, wallet_segment, account_segment)
        .join("journal")
        .join(year)
}

/// Pure function: build `{hledger_dir}/{owner_segment}/{wallet_segment}/{account_segment}/journal/{year}/{year}.j.txt`.
#[cfg(test)]
pub(crate) fn hledger_owner_account_year_journal_path(
    hledger_dir: &Path,
    owner_segment: &str,
    wallet_segment: &str,
    account_segment: &str,
    year: &str,
) -> PathBuf {
    hledger_owner_account_year_journal_dir(
        hledger_dir,
        owner_segment,
        wallet_segment,
        account_segment,
        year,
    )
    .join(format!("{year}.j.txt"))
}

/// Pure function: build `{hledger_dir}/{owner_segment}/{wallet_segment}/{account_segment}/{year}-opening.j.txt`.
#[cfg(test)]
pub(crate) fn hledger_owner_account_year_opening_journal_path(
    hledger_dir: &Path,
    owner_segment: &str,
    wallet_segment: &str,
    account_segment: &str,
    year: &str,
) -> PathBuf {
    hledger_owner_account_dir(hledger_dir, owner_segment, wallet_segment, account_segment)
        .join(format!("{year}-opening.j.txt"))
}

/// Pure function: build `{hledger_dir}/{owner_segment}/{wallet_segment}/{account_segment}/{year}-closing.j.txt`.
#[cfg(test)]
pub(crate) fn hledger_owner_account_year_closing_journal_path(
    hledger_dir: &Path,
    owner_segment: &str,
    wallet_segment: &str,
    account_segment: &str,
    year: &str,
) -> PathBuf {
    hledger_owner_account_dir(hledger_dir, owner_segment, wallet_segment, account_segment)
        .join(format!("{year}-closing.j.txt"))
}

/// Pure function: build `{hledger_dir}/{owner_segment}/{wallet_segment}/{account_segment}/{year}-include.j.txt`.
#[cfg(test)]
pub(crate) fn hledger_owner_account_year_include_journal_path(
    hledger_dir: &Path,
    owner_segment: &str,
    wallet_segment: &str,
    account_segment: &str,
    year: &str,
) -> PathBuf {
    hledger_owner_account_dir(hledger_dir, owner_segment, wallet_segment, account_segment)
        .join(format!("{year}-include.j.txt"))
}

/// Pure function: build `{hledger_dir}/{owner_segment}/{wallet_segment}/{account_segment}/all-years.j.txt`.
#[cfg(test)]
pub(crate) fn hledger_owner_account_all_years_journal_path(
    hledger_dir: &Path,
    owner_segment: &str,
    wallet_segment: &str,
    account_segment: &str,
) -> PathBuf {
    hledger_owner_account_dir(hledger_dir, owner_segment, wallet_segment, account_segment)
        .join("all-years.j.txt")
}

/// Pure function: build `{hledger_dir}/directives.j.txt`.
#[cfg(test)]
pub(crate) fn hledger_directives_path(hledger_dir: &Path) -> PathBuf {
    hledger_dir.join("directives.j.txt")
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn fixed_user_id() -> UserId {
        UserId::from_str("01KGQYDBAH5B0JD0BSF2VX95FR").expect("valid ULID")
    }

    #[test]
    fn test_app_dir_from_project_dir() {
        let project_dir = Path::new("/project");
        assert_eq!(
            app_dir_from_project_dir(project_dir),
            PathBuf::from("/project").join("app")
        );
    }

    #[test]
    fn test_app_data_dir_from_app_dir() {
        let app_dir = Path::new("/project/app");
        assert_eq!(
            app_data_dir_from_app_dir(app_dir),
            PathBuf::from("/project").join("app").join("data")
        );
    }

    #[test]
    fn test_app_database_path_from_app_data_dir() {
        let app_data_dir = Path::new("/project/app/data");
        assert_eq!(
            app_database_path_from_app_data_dir(app_data_dir),
            PathBuf::from("/project")
                .join("app")
                .join("data")
                .join("app.db")
        );
    }

    #[test]
    fn test_price_data_dir_from_app_data_dir() {
        let app_data_dir = Path::new("/project/app/data");
        assert_eq!(
            price_data_dir_from_app_data_dir(app_data_dir),
            PathBuf::from("/project")
                .join("app")
                .join("data")
                .join("prices")
        );
    }

    #[test]
    fn test_price_database_path_from_price_data_dir() {
        let price_data_dir = Path::new("/project/app/data/prices");
        assert_eq!(
            price_database_path_from_price_data_dir(price_data_dir),
            PathBuf::from("/project")
                .join("app")
                .join("data")
                .join("prices")
                .join("prices.db")
        );
    }

    #[test]
    fn test_price_database_path_from_project_dir() {
        let project_dir = Path::new("/project");
        assert_eq!(
            price_database_path_from_project_dir(project_dir),
            PathBuf::from("/project")
                .join("app")
                .join("data")
                .join("prices")
                .join("prices.db")
        );
    }

    #[test]
    fn test_get_price_database_path_creates_price_data_dir() {
        let project_dir =
            std::env::temp_dir().join(format!("bitgarth_price_project_dir_{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&project_dir).expect("temp project dir should be created");

        {
            let runtime_context =
                crate::runtime_context::RuntimeContext::new_test(project_dir.clone());
            let _guard = crate::runtime_context::push_default_runtime_context(runtime_context);

            let result = get_price_database_path().expect("price database path");

            assert_eq!(
                result,
                project_dir
                    .join("app")
                    .join("data")
                    .join("prices")
                    .join("prices.db")
            );
            assert!(
                project_dir.join("app").join("data").join("prices").is_dir(),
                "price data dir should be created"
            );
        }

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn test_user_dir_from_project_dir() {
        let project_dir = Path::new("/project");
        let user_id = fixed_user_id();
        assert_eq!(
            user_dir_from_project_dir(project_dir, user_id),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
        );
    }

    #[test]
    fn test_user_data_dir_from_user_dir() {
        let user_dir = Path::new("/project/users/01KGQYDBAH5B0JD0BSF2VX95FR");
        assert_eq!(
            user_data_dir_from_user_dir(user_dir),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("data")
        );
    }

    #[test]
    fn test_user_database_path_from_user_data_dir() {
        let user_data_dir = Path::new("/project/users/01KGQYDBAH5B0JD0BSF2VX95FR/data");
        let user_id = fixed_user_id();
        assert_eq!(
            user_database_path_from_user_data_dir(user_data_dir, user_id),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("data")
                .join("u01KGQYDBAH5B0JD0BSF2VX95FR.db")
        );
    }

    #[test]
    fn test_user_envelope_path_from_user_data_dir() {
        let user_data_dir = Path::new("/project/users/01KGQYDBAH5B0JD0BSF2VX95FR/data");
        let user_id = fixed_user_id();
        assert_eq!(
            user_envelope_path_from_user_data_dir(user_data_dir, user_id),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("data")
                .join("u01KGQYDBAH5B0JD0BSF2VX95FR.json")
        );
    }

    #[test]
    fn test_user_traces_dir_from_user_dir() {
        let user_dir = Path::new("/project/users/01KGQYDBAH5B0JD0BSF2VX95FR");
        assert_eq!(
            user_traces_dir_from_user_dir(user_dir),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("traces")
        );
    }

    #[test]
    fn test_hledger_wallet_account_and_journal_paths() {
        let hledger_dir = Path::new("/project/users/01KGQYDBAH5B0JD0BSF2VX95FR/hledger");
        assert_eq!(
            hledger_owner_dir(hledger_dir, "me"),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("hledger")
                .join("me")
        );
        assert_eq!(
            hledger_owner_wallet_dir(hledger_dir, "me", "MainWallet"),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("hledger")
                .join("me")
                .join("MainWallet")
        );
        assert_eq!(
            hledger_owner_account_dir(hledger_dir, "me", "MainWallet", "BitcoinAccount1"),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("hledger")
                .join("me")
                .join("MainWallet")
                .join("BitcoinAccount1")
        );
        assert_eq!(
            hledger_owner_account_year_journal_path(
                hledger_dir,
                "me",
                "MainWallet",
                "BitcoinAccount1",
                "2026"
            ),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("hledger")
                .join("me")
                .join("MainWallet")
                .join("BitcoinAccount1")
                .join("journal")
                .join("2026")
                .join("2026.j.txt")
        );
        assert_eq!(
            hledger_owner_account_year_opening_journal_path(
                hledger_dir,
                "me",
                "MainWallet",
                "BitcoinAccount1",
                "2026"
            ),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("hledger")
                .join("me")
                .join("MainWallet")
                .join("BitcoinAccount1")
                .join("2026-opening.j.txt")
        );
        assert_eq!(
            hledger_owner_account_year_closing_journal_path(
                hledger_dir,
                "me",
                "MainWallet",
                "BitcoinAccount1",
                "2026"
            ),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("hledger")
                .join("me")
                .join("MainWallet")
                .join("BitcoinAccount1")
                .join("2026-closing.j.txt")
        );
        assert_eq!(
            hledger_owner_account_year_include_journal_path(
                hledger_dir,
                "me",
                "MainWallet",
                "BitcoinAccount1",
                "2026"
            ),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("hledger")
                .join("me")
                .join("MainWallet")
                .join("BitcoinAccount1")
                .join("2026-include.j.txt")
        );
        assert_eq!(
            hledger_owner_account_all_years_journal_path(
                hledger_dir,
                "me",
                "MainWallet",
                "BitcoinAccount1"
            ),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("hledger")
                .join("me")
                .join("MainWallet")
                .join("BitcoinAccount1")
                .join("all-years.j.txt")
        );
        assert_eq!(
            hledger_directives_path(hledger_dir),
            PathBuf::from("/project")
                .join("users")
                .join("01KGQYDBAH5B0JD0BSF2VX95FR")
                .join("hledger")
                .join("directives.j.txt")
        );
    }

    #[test]
    fn test_parse_project_dir_override_rejects_empty() {
        let result = parse_project_dir_override(OsStr::new(""));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_project_dir_override_rejects_relative_path() {
        let result = parse_project_dir_override(OsStr::new("relative/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_project_dir_override_creates_nonexistent_path() {
        let path = std::env::temp_dir().join(format!(
            "bitgarth_missing_project_dir_{}",
            ulid::Ulid::new()
        ));
        assert!(!path.exists(), "path should not exist before the call");

        let result = parse_project_dir_override(path.as_os_str());
        assert!(result.is_ok(), "should create the directory");
        assert!(path.is_dir(), "directory should now exist");

        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn test_parse_project_dir_override_rejects_file_path() {
        let base =
            std::env::temp_dir().join(format!("bitgarth_project_dir_file_{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&base).expect("temp dir should be created");
        let file_path = base.join("project-root-file");
        std::fs::write(&file_path, "not a directory").expect("temp file should be created");

        let result = parse_project_dir_override(file_path.as_os_str());
        assert!(result.is_err());

        let _ = std::fs::remove_file(&file_path);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_parse_project_dir_override_accepts_existing_absolute_directory() {
        let path =
            std::env::temp_dir().join(format!("bitgarth_project_dir_valid_{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&path).expect("temp dir should be created");

        let parsed = parse_project_dir_override(path.as_os_str()).expect("path should be valid");
        assert_eq!(parsed, path);

        let _ = std::fs::remove_dir_all(&parsed);
    }

    #[test]
    fn default_project_dir_uses_stable_bitgarth_identity() {
        let expected = directories::ProjectDirs::from("app.bitgarth", "", "bitgarth")
            .expect("supported platform")
            .data_dir()
            .to_path_buf();

        assert_eq!(
            default_project_dir().expect("default project dir"),
            expected
        );
    }
}
