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
    NativeUpdater,
    AppStore,
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

pub(crate) fn channel() -> Channel {
    static CHANNEL: OnceLock<Channel> = OnceLock::new();
    cached_channel_from(&CHANNEL, || std::env::var("BITGARTH_CHANNEL").ok())
}

fn cached_channel_from<F>(cache: &'static OnceLock<Channel>, get_raw: F) -> Channel
where
    F: FnOnce() -> Option<String>,
{
    *cache.get_or_init(|| parse_channel(get_raw().as_deref()))
}

impl Channel {
    pub(crate) fn as_header_value(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Umbrel => "umbrel",
            Self::Desktop => "desktop",
            Self::Ios => "ios",
            Self::Android => "android",
            Self::Hosted => "hosted",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn upgrade_kind(self) -> UpgradeUi {
        match self {
            Self::Docker => UpgradeUi::Script,
            Self::Umbrel => UpgradeUi::Native("Umbrel"),
            Self::Desktop => UpgradeUi::NativeUpdater,
            Self::Ios | Self::Android => UpgradeUi::AppStore,
            Self::Hosted | Self::Unknown => UpgradeUi::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Channel, UpgradeUi, channel, parse_channel};
    use std::sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    };

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
        assert_eq!(Channel::Desktop.upgrade_kind(), UpgradeUi::NativeUpdater);
        assert_eq!(Channel::Ios.upgrade_kind(), UpgradeUi::AppStore);
        assert_eq!(Channel::Android.upgrade_kind(), UpgradeUi::AppStore);
        assert_eq!(Channel::Hosted.upgrade_kind(), UpgradeUi::None);
        assert_eq!(Channel::Unknown.upgrade_kind(), UpgradeUi::None);
    }

    #[test]
    fn header_values_are_stable() {
        assert_eq!(Channel::Docker.as_header_value(), "docker");
        assert_eq!(Channel::Umbrel.as_header_value(), "umbrel");
        assert_eq!(Channel::Desktop.as_header_value(), "desktop");
        assert_eq!(Channel::Ios.as_header_value(), "ios");
        assert_eq!(Channel::Android.as_header_value(), "android");
        assert_eq!(Channel::Hosted.as_header_value(), "hosted");
        assert_eq!(Channel::Unknown.as_header_value(), "unknown");
    }

    #[test]
    fn channel_wrapper_uses_cache() {
        static TEST_CHANNEL: OnceLock<Channel> = OnceLock::new();
        static CALLS: AtomicUsize = AtomicUsize::new(0);

        let first = super::cached_channel_from(&TEST_CHANNEL, || {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Some("docker".to_string())
        });
        let second = super::cached_channel_from(&TEST_CHANNEL, || {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Some("hosted".to_string())
        });

        assert_eq!(first, Channel::Docker);
        assert_eq!(second, Channel::Docker);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn channel_wrapper_returns_a_channel_from_environment() {
        let _ = channel();
    }
}
