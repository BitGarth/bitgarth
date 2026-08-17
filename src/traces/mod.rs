//! HTTP request/response tracing module.
//!
//! When the `BGTRACES` environment variable is set to `"fs"` (requires
//! `dev-config` feature), all outgoing HTTP requests are traced to per-user
//! directories in HAR 1.2 format.
//!
//! The env var is read once at startup and cached. Toggling requires an app restart.

pub(crate) mod client;
#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod enforcement;
pub(crate) mod har;
pub(crate) mod writer;

use std::sync::OnceLock;

/// Controls whether HTTP tracing is active and where traces are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TracingMode {
    Disabled,
    Filesystem,
}

static TRACING_MODE: OnceLock<TracingMode> = OnceLock::new();

/// Pure function: parse the BGTRACES env var value into a TracingMode.
fn parse_tracing_mode(value: Option<&str>) -> TracingMode {
    match value {
        Some("fs") => TracingMode::Filesystem,
        _ => TracingMode::Disabled,
    }
}

/// Read BGTRACES env var once, cache the result. Requires `dev-config` feature.
pub(crate) fn tracing_mode() -> &'static TracingMode {
    TRACING_MODE.get_or_init(|| {
        #[cfg(feature = "dev-config")]
        let value = std::env::var("BGTRACES").ok();
        #[cfg(not(feature = "dev-config"))]
        let value: Option<String> = None;
        parse_tracing_mode(value.as_deref())
    })
}

/// Check if tracing is enabled.
pub(crate) fn is_tracing_enabled() -> bool {
    matches!(tracing_mode(), TracingMode::Filesystem)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tracing_mode_fs() {
        assert_eq!(parse_tracing_mode(Some("fs")), TracingMode::Filesystem);
    }

    #[test]
    fn test_parse_tracing_mode_other() {
        assert_eq!(parse_tracing_mode(Some("other")), TracingMode::Disabled);
    }

    #[test]
    fn test_parse_tracing_mode_none() {
        assert_eq!(parse_tracing_mode(None), TracingMode::Disabled);
    }

    #[test]
    fn test_parse_tracing_mode_empty_string() {
        assert_eq!(parse_tracing_mode(Some("")), TracingMode::Disabled);
    }
}
