#[cfg(feature = "server")]
use dioxus::logger::tracing;
#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
use std::sync::{LazyLock, Mutex, MutexGuard, RwLock};

#[cfg(feature = "server")]
const BITGARTH_SYNC_CONTROL_ENV: &str = "BITGARTH_SYNC_CONTROL";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SyncControlMode {
    Enabled,
    #[default]
    Disabled,
}

pub(crate) fn parse_sync_control_mode(value: Option<&str>) -> SyncControlMode {
    match value.map(str::trim).map(str::to_ascii_lowercase) {
        Some(value) if matches!(value.as_str(), "1" | "true" | "yes" | "on") => {
            SyncControlMode::Enabled
        }
        _ => SyncControlMode::Disabled,
    }
}

#[cfg(feature = "server")]
fn is_recognized_sync_control_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "0" | "false" | "no" | "off" | ""
    )
}

#[cfg(feature = "server")]
pub(crate) fn sync_control_mode() -> SyncControlMode {
    #[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
    if let Some(mode) = test_sync_control_mode_override() {
        return mode;
    }

    let raw_value: Option<String> = {
        #[cfg(feature = "dev-config")]
        {
            std::env::var(BITGARTH_SYNC_CONTROL_ENV).ok()
        }
        #[cfg(not(feature = "dev-config"))]
        {
            None
        }
    };
    if let Some(raw_value) = raw_value.as_deref()
        && !is_recognized_sync_control_value(raw_value)
    {
        tracing::warn!(
            env_var = BITGARTH_SYNC_CONTROL_ENV,
            value = %raw_value,
            "sync control: invalid mode value, using disabled"
        );
    }
    parse_sync_control_mode(raw_value.as_deref())
}

#[cfg(feature = "server")]
pub(crate) fn is_sync_control_enabled() -> bool {
    matches!(sync_control_mode(), SyncControlMode::Enabled)
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
static TEST_SYNC_CONTROL_MODE_OVERRIDE: LazyLock<RwLock<Option<SyncControlMode>>> =
    LazyLock::new(|| RwLock::new(None));

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
static TEST_SYNC_CONTROL_MODE_OVERRIDE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
fn test_sync_control_mode_override() -> Option<SyncControlMode> {
    TEST_SYNC_CONTROL_MODE_OVERRIDE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .copied()
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
pub(crate) fn reset_sync_control_mode_override_for_tests() {
    let _lock = TEST_SYNC_CONTROL_MODE_OVERRIDE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *TEST_SYNC_CONTROL_MODE_OVERRIDE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SyncControlMode::Disabled);
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
pub(crate) struct SyncControlModeOverrideGuard {
    previous: Option<SyncControlMode>,
    _lock: MutexGuard<'static, ()>,
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
impl Drop for SyncControlModeOverrideGuard {
    fn drop(&mut self) {
        *TEST_SYNC_CONTROL_MODE_OVERRIDE
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.previous;
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
pub(crate) fn set_sync_control_mode_override_for_tests(
    mode: Option<SyncControlMode>,
) -> SyncControlModeOverrideGuard {
    let lock = TEST_SYNC_CONTROL_MODE_OVERRIDE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut override_mode = TEST_SYNC_CONTROL_MODE_OVERRIDE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = *override_mode;
    *override_mode = mode;
    drop(override_mode);

    SyncControlModeOverrideGuard {
        previous,
        _lock: lock,
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn parse_sync_control_mode_accepts_enabled_values() {
        for enabled_value in ["1", "true", "TRUE", " yes ", "on"] {
            assert_eq!(
                parse_sync_control_mode(Some(enabled_value)),
                SyncControlMode::Enabled,
                "expected {enabled_value:?} to enable sync control"
            );
        }
    }

    #[test]
    fn parse_sync_control_mode_treats_other_values_as_disabled() {
        for disabled_value in [None, Some(""), Some("0"), Some("false"), Some("off")] {
            assert_eq!(
                parse_sync_control_mode(disabled_value),
                SyncControlMode::Disabled
            );
        }
    }

    #[test]
    fn parse_sync_control_mode_treats_unrecognized_values_as_disabled() {
        assert_eq!(
            parse_sync_control_mode(Some("unexpected")),
            SyncControlMode::Disabled
        );
    }

    #[test]
    fn guarded_override_restores_mode_repeatedly() {
        reset_sync_control_mode_override_for_tests();

        for _ in 0..32 {
            let enabled_guard =
                set_sync_control_mode_override_for_tests(Some(SyncControlMode::Enabled));
            assert_eq!(enabled_guard.previous, Some(SyncControlMode::Disabled));
            assert_eq!(sync_control_mode(), SyncControlMode::Enabled);
            drop(enabled_guard);

            let restored_guard =
                set_sync_control_mode_override_for_tests(Some(SyncControlMode::Disabled));
            assert_eq!(restored_guard.previous, Some(SyncControlMode::Disabled));
            assert_eq!(sync_control_mode(), SyncControlMode::Disabled);
            drop(restored_guard);
        }
    }
}
