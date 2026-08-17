/// Returns the application version string.
///
/// Format: `{package_version}-{git_short_sha}`
/// Example: `0.1.0-abcdefg`
///
/// If the git SHA was not available at build time, returns just the package version.
pub(crate) fn version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

    VERSION.get_or_init(|| {
        let pkg_version = env!("CARGO_PKG_VERSION");
        match option_env!("GIT_SHORT_SHA") {
            Some(sha) if !sha.is_empty() => format!("{pkg_version}-{sha}"),
            _ => pkg_version.to_string(),
        }
    })
}

/// True when the loaded client build differs from the server build.
/// Plain string inequality — robust regardless of the identifier format.
pub(crate) fn is_drifted(loaded: &str, server: &str) -> bool {
    loaded != server
}

/// Format a `version()` string for display.
/// `"0.1.5-abc1234"` -> `"0.1.5 (abc1234)"`; `"0.1.5"` -> `"0.1.5"`.
/// Splits on the last hyphen so future pre-release package versions keep their package version intact.
pub(crate) fn format_build(raw: &str) -> String {
    match raw.rsplit_once('-') {
        Some((version, sha))
            if !version.is_empty()
                && !sha.is_empty()
                && sha.chars().all(|ch| ch.is_ascii_hexdigit()) =>
        {
            format!("{version} ({sha})")
        }
        _ => raw.to_string(),
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn version_contains_package_version() {
        let v = version();
        assert!(v.starts_with(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn drift_is_false_when_equal() {
        assert!(!is_drifted("0.1.5-abc1234", "0.1.5-abc1234"));
        assert!(!is_drifted("0.1.5", "0.1.5"));
    }

    #[test]
    fn drift_is_true_when_version_or_sha_differs() {
        assert!(is_drifted("0.1.5-abc1234", "0.1.6-def5678"));
        assert!(is_drifted("0.1.5-abc1234", "0.1.5-def5678"));
        assert!(is_drifted("0.1.5", "0.1.6"));
    }

    #[test]
    fn format_build_splits_version_and_sha_on_last_hyphen() {
        assert_eq!(format_build("0.1.5-abc1234"), "0.1.5 (abc1234)");
        assert_eq!(format_build("0.1.5"), "0.1.5");
        assert_eq!(format_build("0.1.5-alpha.1"), "0.1.5-alpha.1");
        assert_eq!(
            format_build("0.1.5-alpha.1-abc1234"),
            "0.1.5-alpha.1 (abc1234)"
        );
    }
}
