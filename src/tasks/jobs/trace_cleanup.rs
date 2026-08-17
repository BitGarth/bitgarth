use crate::db::DbError;
use crate::models::UserId;
use crate::project_paths::get_project_dir;
use chrono::{DateTime, NaiveDate, Utc};
use dioxus::logger::tracing;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TraceCleanupInterval(Duration);

impl TraceCleanupInterval {
    const fn from_hours(hours: u64) -> Self {
        Self(Duration::from_secs(hours * 60 * 60))
    }

    pub(crate) const fn as_duration(self) -> Duration {
        self.0
    }
}

pub(crate) const TRACE_CLEANUP_INTERVAL: TraceCleanupInterval = TraceCleanupInterval::from_hours(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TraceRetentionHours(u16);

impl TraceRetentionHours {
    pub(crate) const fn new(hours: u16) -> Self {
        Self(hours)
    }

    fn as_duration(self) -> chrono::Duration {
        chrono::Duration::hours(i64::from(self.0))
    }
}

pub(crate) const TRACE_RETENTION_HOURS: TraceRetentionHours = TraceRetentionHours::new(24);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TraceCleanupParams {
    pub(crate) retention: TraceRetentionHours,
}

impl Default for TraceCleanupParams {
    fn default() -> Self {
        Self {
            retention: TRACE_RETENTION_HOURS,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TraceCleanupSummary {
    pub(crate) scanned_hour_dirs: u64,
    pub(crate) deleted_hour_dirs: u64,
    pub(crate) skipped_symlink_entries: u64,
    pub(crate) io_errors: u64,
}

#[derive(Debug)]
pub(crate) enum TraceCleanupError {
    ProjectPath(DbError),
    ReadUsersRoot {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for TraceCleanupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceCleanupError::ProjectPath(err) => write!(f, "{err}"),
            TraceCleanupError::ReadUsersRoot { path, source } => {
                write!(f, "Failed to read users root at {path:?}: {source}")
            }
        }
    }
}

impl std::error::Error for TraceCleanupError {}

fn parse_directory_component(name: &OsStr, width: usize, min: u32, max: u32) -> Option<u32> {
    let value = name.to_str()?;
    if value.len() != width || !value.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let parsed = value.parse::<u32>().ok()?;
    if parsed < min || parsed > max {
        return None;
    }

    Some(parsed)
}

fn hour_start_utc(year: i32, month: u32, day: u32, hour: u32) -> Option<DateTime<Utc>> {
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let datetime = date.and_hms_opt(hour, 0, 0)?;
    Some(DateTime::from_naive_utc_and_offset(datetime, Utc))
}

fn is_path_symlink(path: &Path) -> Result<bool, std::io::Error> {
    Ok(std::fs::symlink_metadata(path)?.file_type().is_symlink())
}

fn is_real_directory_entry(entry: &std::fs::DirEntry, summary: &mut TraceCleanupSummary) -> bool {
    match entry.file_type() {
        Ok(file_type) => {
            if file_type.is_symlink() {
                summary.skipped_symlink_entries += 1;
                tracing::warn!(
                    path = ?entry.path(),
                    "tasks: trace cleanup skipping symlink entry"
                );
                return false;
            }
            file_type.is_dir()
        }
        Err(err) => {
            summary.io_errors += 1;
            tracing::warn!(
                path = ?entry.path(),
                error = %err,
                "tasks: trace cleanup could not read file type"
            );
            false
        }
    }
}

fn cleanup_user_trace_tree(
    traces_dir: &Path,
    cutoff: DateTime<Utc>,
    summary: &mut TraceCleanupSummary,
) {
    let year_dirs = match std::fs::read_dir(traces_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            summary.io_errors += 1;
            tracing::warn!(
                path = ?traces_dir,
                error = %err,
                "tasks: trace cleanup failed to read traces directory"
            );
            return;
        }
    };

    for year_entry in year_dirs.flatten() {
        if !is_real_directory_entry(&year_entry, summary) {
            continue;
        }
        let Some(year_u32) = parse_directory_component(&year_entry.file_name(), 4, 0, 9999) else {
            continue;
        };
        let Some(year) = i32::try_from(year_u32).ok() else {
            continue;
        };

        let month_dirs = match std::fs::read_dir(year_entry.path()) {
            Ok(entries) => entries,
            Err(err) => {
                summary.io_errors += 1;
                tracing::warn!(
                    path = ?year_entry.path(),
                    error = %err,
                    "tasks: trace cleanup failed to read year directory"
                );
                continue;
            }
        };

        for month_entry in month_dirs.flatten() {
            if !is_real_directory_entry(&month_entry, summary) {
                continue;
            }
            let Some(month) = parse_directory_component(&month_entry.file_name(), 2, 1, 12) else {
                continue;
            };

            let day_dirs = match std::fs::read_dir(month_entry.path()) {
                Ok(entries) => entries,
                Err(err) => {
                    summary.io_errors += 1;
                    tracing::warn!(
                        path = ?month_entry.path(),
                        error = %err,
                        "tasks: trace cleanup failed to read month directory"
                    );
                    continue;
                }
            };

            for day_entry in day_dirs.flatten() {
                if !is_real_directory_entry(&day_entry, summary) {
                    continue;
                }
                let Some(day) = parse_directory_component(&day_entry.file_name(), 2, 1, 31) else {
                    continue;
                };

                let hour_dirs = match std::fs::read_dir(day_entry.path()) {
                    Ok(entries) => entries,
                    Err(err) => {
                        summary.io_errors += 1;
                        tracing::warn!(
                            path = ?day_entry.path(),
                            error = %err,
                            "tasks: trace cleanup failed to read day directory"
                        );
                        continue;
                    }
                };

                for hour_entry in hour_dirs.flatten() {
                    if !is_real_directory_entry(&hour_entry, summary) {
                        continue;
                    }
                    let Some(hour) = parse_directory_component(&hour_entry.file_name(), 2, 0, 23)
                    else {
                        continue;
                    };
                    let Some(hour_start) = hour_start_utc(year, month, day, hour) else {
                        continue;
                    };

                    summary.scanned_hour_dirs += 1;
                    if hour_start >= cutoff {
                        continue;
                    }

                    let hour_path = hour_entry.path();
                    match std::fs::remove_dir_all(&hour_path) {
                        Ok(()) => {
                            summary.deleted_hour_dirs += 1;
                        }
                        Err(err) => {
                            summary.io_errors += 1;
                            tracing::warn!(
                                path = ?hour_path,
                                error = %err,
                                "tasks: trace cleanup failed to remove old hour directory"
                            );
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn run_for_project_dir_with_now(
    project_dir: &Path,
    now: DateTime<Utc>,
    params: TraceCleanupParams,
) -> Result<TraceCleanupSummary, TraceCleanupError> {
    let users_dir = project_dir.join("users");
    let mut summary = TraceCleanupSummary::default();
    let cutoff = now - params.retention.as_duration();

    let user_entries = match std::fs::read_dir(&users_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(summary),
        Err(err) => {
            return Err(TraceCleanupError::ReadUsersRoot {
                path: users_dir,
                source: err,
            });
        }
    };

    for user_entry in user_entries.flatten() {
        if !is_real_directory_entry(&user_entry, &mut summary) {
            continue;
        }

        let user_name = user_entry.file_name();
        let Some(user_id_str) = user_name.to_str() else {
            continue;
        };
        if UserId::from_str(user_id_str).is_err() {
            continue;
        }

        let traces_dir = user_entry.path().join("traces");
        match is_path_symlink(&traces_dir) {
            Ok(true) => {
                summary.skipped_symlink_entries += 1;
                tracing::warn!(
                    path = ?traces_dir,
                    "tasks: trace cleanup skipping symlink traces directory"
                );
                continue;
            }
            Ok(false) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                summary.io_errors += 1;
                tracing::warn!(
                    path = ?traces_dir,
                    error = %err,
                    "tasks: trace cleanup failed to read traces directory metadata"
                );
                continue;
            }
        }

        cleanup_user_trace_tree(&traces_dir, cutoff, &mut summary);
    }

    Ok(summary)
}

pub(crate) fn run(params: TraceCleanupParams) -> Result<TraceCleanupSummary, TraceCleanupError> {
    let project_dir = get_project_dir().map_err(TraceCleanupError::ProjectPath)?;
    run_for_project_dir_with_now(&project_dir, Utc::now(), params)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-02-13T12:30:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    fn fixed_user_id() -> UserId {
        UserId::from_str("01K2EZ4N4G0W0EPF9YXRJZD9WQ").expect("valid user id")
    }

    fn temp_project_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("bitgarth_{name}_{}", ulid::Ulid::new()))
    }

    #[test]
    fn parse_directory_component_rejects_invalid_values() {
        assert_eq!(
            parse_directory_component(OsStr::new("08"), 2, 0, 23),
            Some(8)
        );
        assert_eq!(parse_directory_component(OsStr::new("8"), 2, 0, 23), None);
        assert_eq!(parse_directory_component(OsStr::new("aa"), 2, 0, 23), None);
        assert_eq!(parse_directory_component(OsStr::new("99"), 2, 0, 23), None);
    }

    #[test]
    fn trace_cleanup_deletes_old_hour_directories_only() {
        let project_dir = temp_project_dir("trace_cleanup_old");
        let user_id = fixed_user_id();
        let traces_root = project_dir
            .join("users")
            .join(user_id.to_string())
            .join("traces");
        let old_hour_dir = traces_root.join("2026/02/12/10");
        let recent_hour_dir = traces_root.join("2026/02/13/12");

        std::fs::create_dir_all(&old_hour_dir).expect("create old hour dir");
        std::fs::create_dir_all(&recent_hour_dir).expect("create recent hour dir");
        std::fs::write(old_hour_dir.join("request.har"), "old").expect("write old file");
        std::fs::write(recent_hour_dir.join("request.har"), "recent").expect("write recent file");

        let result =
            run_for_project_dir_with_now(&project_dir, fixed_now(), TraceCleanupParams::default())
                .expect("cleanup should succeed");

        assert!(
            !old_hour_dir.exists(),
            "old hour directory should be deleted"
        );
        assert!(
            recent_hour_dir.exists(),
            "recent hour directory should be retained"
        );
        assert_eq!(result.deleted_hour_dirs, 1);
        assert_eq!(result.scanned_hour_dirs, 2);
        assert_eq!(result.io_errors, 0);

        std::fs::remove_dir_all(&project_dir).expect("cleanup temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn trace_cleanup_skips_symlinked_trace_dirs() {
        use std::os::unix::fs::symlink;

        let project_dir = temp_project_dir("trace_cleanup_symlink");
        let user_id = fixed_user_id();
        let user_dir = project_dir.join("users").join(user_id.to_string());
        let traces_target = project_dir.join("outside_traces");
        let old_hour_dir = traces_target.join("2026/02/10/10");

        std::fs::create_dir_all(&user_dir).expect("create user dir");
        std::fs::create_dir_all(&old_hour_dir).expect("create old hour dir");
        symlink(&traces_target, user_dir.join("traces")).expect("create traces symlink");

        let result =
            run_for_project_dir_with_now(&project_dir, fixed_now(), TraceCleanupParams::default())
                .expect("cleanup should succeed");

        assert!(
            old_hour_dir.exists(),
            "symlink target should not be touched"
        );
        assert_eq!(result.deleted_hour_dirs, 0);
        assert_eq!(result.skipped_symlink_entries, 1);

        std::fs::remove_dir_all(&project_dir).expect("cleanup temp dir");
    }
}
