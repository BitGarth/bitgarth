use crate::models::{DateTimeFormat, UserId, UserTimezone};
use crate::{AuthState, AuthStatus};
use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use dioxus::logger::tracing;
use dioxus::prelude::*;

#[cfg(feature = "web")]
async fn detect_timezone_web() -> Option<Tz> {
    use dioxus::document::eval;

    let mut eval_result = eval(r#"dioxus.send(Intl.DateTimeFormat().resolvedOptions().timeZone)"#);
    match eval_result.recv().await {
        Ok(serde_json::Value::String(tz_str)) => tz_str.parse::<Tz>().ok(),
        _ => None,
    }
}

#[cfg(feature = "desktop")]
fn detect_timezone_desktop() -> Option<Tz> {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|s| s.parse::<Tz>().ok())
}

/// The browser/OS zone, `None` until detected (always during SSR) or when detection fails.
pub(crate) fn use_timezone() -> Signal<Option<Tz>> {
    let tz = use_signal(|| None);

    #[cfg(feature = "web")]
    {
        let mut tz = tz;
        use_effect(move || {
            spawn(async move {
                let detected = detect_timezone_web().await;
                tz.set(detected);
            });
        });
    }

    #[cfg(feature = "desktop")]
    {
        let mut tz = tz;
        use_effect(move || {
            let detected = detect_timezone_desktop();
            tz.set(detected);
        });
    }

    tz
}

/// Saves the detected zone for a signed-in user who never saved one, so every
/// device and every SSR render agree instead of each following its own browser.
pub(crate) fn use_persist_detected_timezone(
    detected: Signal<Option<Tz>>,
    auth_state: AuthState,
    mut timezone: Signal<UserTimezone>,
) {
    let mut persisted_for = use_signal(|| None::<UserId>);
    use_effect(move || {
        let Some(detected) = detected() else {
            return;
        };
        let user_id = match &*auth_state.read() {
            AuthStatus::Authenticated(auth) if auth.settings.timezone.is_none() => {
                auth.user.user_id
            }
            _ => return,
        };
        if *persisted_for.peek() == Some(user_id) {
            return;
        }
        persisted_for.set(Some(user_id));
        let detected = UserTimezone::from(detected);
        timezone.set(detected);
        spawn(async move {
            if let Err(err) = crate::backend::save_timezone(detected).await {
                tracing::warn!(error = %err, "settings: failed to save detected timezone");
            }
        });
    });
}

pub(crate) fn format_timestamp(
    created_at: &DateTime<Utc>,
    tz: Tz,
    format: DateTimeFormat,
) -> String {
    let local = tz.from_utc_datetime(&created_at.naive_utc());
    format!(
        "{} {}",
        local.format(format.chrono_format()),
        local.format("%Z")
    )
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn test_format_utc_to_new_york_timezone() {
        let utc_time = Utc.with_ymd_and_hms(2026, 1, 21, 19, 48, 35).unwrap();
        let tz: Tz = "America/New_York".parse().unwrap();

        let formatted = format_timestamp(&utc_time, tz, DateTimeFormat::MonthDayYear12);

        // New York is UTC-5 in winter (EST), so 19:48 UTC = 14:48 EST
        assert_eq!(formatted, "Jan 21, 2026 02:48 PM EST");
    }

    #[test]
    fn test_format_utc_to_london_timezone() {
        let utc_time = Utc.with_ymd_and_hms(2026, 7, 15, 12, 30, 0).unwrap();
        let tz: Tz = "Europe/London".parse().unwrap();

        let formatted = format_timestamp(&utc_time, tz, DateTimeFormat::MonthDayYear12);

        // London is UTC+1 in summer (BST), so 12:30 UTC = 13:30 BST
        assert_eq!(formatted, "Jul 15, 2026 01:30 PM BST");
    }

    #[test]
    fn test_format_utc_stays_utc() {
        let utc_time = Utc.with_ymd_and_hms(2026, 1, 21, 19, 48, 35).unwrap();

        let formatted = format_timestamp(&utc_time, Tz::UTC, DateTimeFormat::MonthDayYear12);

        assert_eq!(formatted, "Jan 21, 2026 07:48 PM UTC");
    }

    #[test]
    fn test_parse_valid_timezone() {
        let tz: Result<Tz, _> = "Europe/London".parse();
        assert!(tz.is_ok());

        let tz: Result<Tz, _> = "America/New_York".parse();
        assert!(tz.is_ok());

        let tz: Result<Tz, _> = "Asia/Tokyo".parse();
        assert!(tz.is_ok());
    }

    #[test]
    fn test_parse_invalid_timezone_falls_back() {
        let tz: Tz = "Invalid/Timezone".parse().unwrap_or(Tz::UTC);
        assert_eq!(tz, Tz::UTC);

        let tz: Tz = "Not_A_Timezone".parse().unwrap_or(Tz::UTC);
        assert_eq!(tz, Tz::UTC);
    }
}
