#[cfg(any(feature = "server", test))]
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Channel {
    Docker,
    Umbrel,
    Desktop,
    Ios,
    Android,
    Hosted,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpgradeUi {
    Script,
    Native(&'static str),
    Generic,
    None,
}

pub(crate) fn parse_channel(raw: Option<&str>) -> Channel {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) if value.eq_ignore_ascii_case("docker") => Channel::Docker,
        Some(value) if value.eq_ignore_ascii_case("umbrel") => Channel::Umbrel,
        Some(value) if value.eq_ignore_ascii_case("desktop") => Channel::Desktop,
        Some(value) if value.eq_ignore_ascii_case("ios") => Channel::Ios,
        Some(value) if value.eq_ignore_ascii_case("android") => Channel::Android,
        Some(value) if value.eq_ignore_ascii_case("hosted") => Channel::Hosted,
        _ => Channel::Unknown,
    }
}

#[cfg(any(feature = "server", test))]
pub(crate) fn channel() -> Channel {
    parse_channel(Some(channel_id()))
}

#[cfg(any(feature = "server", test))]
fn build_default_channel() -> &'static str {
    if cfg!(feature = "desktop") {
        "desktop"
    } else {
        "web"
    }
}

#[cfg(any(feature = "server", test))]
fn resolve_channel_id(raw: Option<&str>, default: &str) -> String {
    raw.map(str::trim)
        .filter(|value| {
            (1..=32).contains(&value.len())
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| default.to_string())
}

#[cfg(any(feature = "server", test))]
fn cached_channel_id_from<'a, F>(cache: &'a OnceLock<String>, default: &str, get_raw: F) -> &'a str
where
    F: FnOnce() -> Option<String>,
{
    cache
        .get_or_init(|| resolve_channel_id(get_raw().as_deref(), default))
        .as_str()
}

#[cfg(any(feature = "server", test))]
pub(crate) fn channel_id() -> &'static str {
    static CHANNEL_ID: OnceLock<String> = OnceLock::new();
    cached_channel_id_from(&CHANNEL_ID, build_default_channel(), || {
        std::env::var("BITGARTH_CHANNEL").ok()
    })
}

impl Channel {
    pub(crate) fn upgrade_kind(self) -> UpgradeUi {
        match self {
            Self::Docker => UpgradeUi::Script,
            Self::Umbrel => UpgradeUi::Native("Umbrel"),
            Self::Hosted => UpgradeUi::None,
            Self::Desktop | Self::Ios | Self::Android | Self::Unknown => UpgradeUi::Generic,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Channel, UpgradeUi, build_default_channel, cached_channel_id_from, channel, parse_channel,
        resolve_channel_id,
    };
    use std::sync::OnceLock;

    #[test]
    fn resolve_reporting_channel() {
        for default in ["web", "desktop"] {
            for raw in [
                None,
                Some(""),
                Some(" \t"),
                Some("a_b"),
                Some("a/b"),
                Some("é"),
                Some("web\r\nx"),
                Some("a b"),
            ] {
                assert_eq!(resolve_channel_id(raw, default), default);
            }
            assert_eq!(resolve_channel_id(Some(&"a".repeat(33)), default), default);
            assert_eq!(
                resolve_channel_id(Some(&"a".repeat(32)), default),
                "a".repeat(32)
            );
            assert_eq!(resolve_channel_id(Some("x"), default), "x");
            assert_eq!(resolve_channel_id(Some(" HomeBrew "), default), "homebrew");
            assert_eq!(resolve_channel_id(Some(" HOSTED "), default), "hosted");
        }
        let cache = OnceLock::new();
        let calls = std::cell::Cell::new(0);
        for raw in ["HomeBrew", "docker"] {
            assert_eq!(
                cached_channel_id_from(&cache, "web", || {
                    calls.set(calls.get() + 1);
                    Some(raw.to_string())
                }),
                "homebrew"
            );
        }
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn default_channel_matches_features() {
        assert_eq!(
            build_default_channel(),
            if cfg!(feature = "desktop") {
                "desktop"
            } else {
                "web"
            }
        );
    }

    #[test]
    fn parse_channel_maps_known_values() {
        assert_eq!(parse_channel(Some("docker")), Channel::Docker);
        assert_eq!(parse_channel(Some("umbrel")), Channel::Umbrel);
        assert_eq!(parse_channel(Some("desktop")), Channel::Desktop);
        assert_eq!(parse_channel(Some("ios")), Channel::Ios);
        assert_eq!(parse_channel(Some("android")), Channel::Android);
        assert_eq!(parse_channel(Some("hosted")), Channel::Hosted);
    }

    #[test]
    fn parse_channel_normalizes_empty_case_and_invalid_values() {
        assert_eq!(parse_channel(None), Channel::Unknown);
        assert_eq!(parse_channel(Some("")), Channel::Unknown);
        assert_eq!(parse_channel(Some("plain-docker")), Channel::Unknown);
        assert_eq!(parse_channel(Some("DOCKER")), Channel::Docker);
        assert_eq!(parse_channel(Some(" docker ")), Channel::Docker);
    }

    #[test]
    fn upgrade_kind_maps_channels_to_ui_modes() {
        assert_eq!(Channel::Docker.upgrade_kind(), UpgradeUi::Script);
        assert_eq!(Channel::Umbrel.upgrade_kind(), UpgradeUi::Native("Umbrel"));
        assert_eq!(Channel::Desktop.upgrade_kind(), UpgradeUi::Generic);
        assert_eq!(Channel::Ios.upgrade_kind(), UpgradeUi::Generic);
        assert_eq!(Channel::Android.upgrade_kind(), UpgradeUi::Generic);
        assert_eq!(Channel::Hosted.upgrade_kind(), UpgradeUi::None);
        assert_eq!(Channel::Unknown.upgrade_kind(), UpgradeUi::Generic);
    }

    #[test]
    fn channel_wrapper_returns_a_channel_from_environment() {
        let _ = channel();
    }
}
