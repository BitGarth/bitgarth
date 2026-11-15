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

/// Old and alternate names from tzdata's `backward` file (2025b): the pre-1993
/// conventions, the 1995 renames and the alternate spellings. They duplicate a
/// current zone, so the picker hides them. `UTC` stays: it is the default.
const DEPRECATED_TIMEZONE_ALIASES: &[&str] = &[
    "Africa/Asmera",
    "America/Buenos_Aires",
    "America/Catamarca",
    "America/Cordoba",
    "America/Godthab",
    "America/Indianapolis",
    "America/Jujuy",
    "America/Knox_IN",
    "America/Louisville",
    "America/Mendoza",
    "America/Virgin",
    "Asia/Ashkhabad",
    "Asia/Calcutta",
    "Asia/Chungking",
    "Asia/Dacca",
    "Asia/Istanbul",
    "Asia/Katmandu",
    "Asia/Macao",
    "Asia/Rangoon",
    "Asia/Saigon",
    "Asia/Thimbu",
    "Asia/Ujung_Pandang",
    "Asia/Ulan_Bator",
    "Atlantic/Faeroe",
    "Australia/ACT",
    "Australia/LHI",
    "Australia/NSW",
    "Australia/North",
    "Australia/Queensland",
    "Australia/South",
    "Australia/Tasmania",
    "Australia/Victoria",
    "Australia/West",
    "Australia/Yancowinna",
    "Brazil/Acre",
    "Brazil/DeNoronha",
    "Brazil/East",
    "Brazil/West",
    "CET",
    "CST6CDT",
    "Canada/Atlantic",
    "Canada/Central",
    "Canada/Eastern",
    "Canada/Mountain",
    "Canada/Newfoundland",
    "Canada/Pacific",
    "Canada/Saskatchewan",
    "Canada/Yukon",
    "Chile/Continental",
    "Chile/EasterIsland",
    "Cuba",
    "EET",
    "EST",
    "EST5EDT",
    "Egypt",
    "Eire",
    "Etc/GMT+0",
    "Etc/GMT-0",
    "Etc/GMT0",
    "Etc/Greenwich",
    "Etc/UCT",
    "Etc/Universal",
    "Etc/Zulu",
    "Europe/Kiev",
    "Europe/Nicosia",
    "GB",
    "GB-Eire",
    "GMT+0",
    "GMT-0",
    "GMT0",
    "Greenwich",
    "HST",
    "Hongkong",
    "Iceland",
    "Iran",
    "Israel",
    "Jamaica",
    "Japan",
    "Kwajalein",
    "Libya",
    "MET",
    "MST",
    "MST7MDT",
    "Mexico/BajaNorte",
    "Mexico/BajaSur",
    "Mexico/General",
    "NZ",
    "NZ-CHAT",
    "Navajo",
    "PRC",
    "PST8PDT",
    "Pacific/Ponape",
    "Pacific/Samoa",
    "Pacific/Truk",
    "Poland",
    "Portugal",
    "ROC",
    "ROK",
    "Singapore",
    "Turkey",
    "UCT",
    "US/Alaska",
    "US/Aleutian",
    "US/Arizona",
    "US/Central",
    "US/East-Indiana",
    "US/Eastern",
    "US/Hawaii",
    "US/Indiana-Starke",
    "US/Michigan",
    "US/Mountain",
    "US/Pacific",
    "US/Samoa",
    "Universal",
    "W-SU",
    "Zulu",
];

/// Zones offered by the timezone picker: every current IANA name, plus the
/// user's saved zone even when it is a deprecated alias.
pub(crate) fn timezone_choices(current: Tz) -> impl Iterator<Item = Tz> {
    chrono_tz::TZ_VARIANTS
        .iter()
        .copied()
        .filter(move |tz| *tz == current || !DEPRECATED_TIMEZONE_ALIASES.contains(&tz.name()))
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
    fn timezone_choices_hide_aliases_but_keep_the_saved_zone() {
        let names: Vec<_> = timezone_choices(Tz::UTC).map(|tz| tz.name()).collect();
        assert!(names.contains(&"UTC"));
        assert!(names.contains(&"Africa/Asmara"));
        assert!(names.contains(&"America/Argentina/Buenos_Aires"));
        assert!(!names.contains(&"Africa/Asmera"));
        assert!(!names.contains(&"America/Buenos_Aires"));

        let saved = Tz::Europe__Kiev;
        assert!(timezone_choices(saved).any(|tz| tz == saved));
    }

    #[test]
    fn test_format_utc_to_new_york_timezone() {
        let utc_time = Utc.with_ymd_and_hms(2026, 1, 21, 19, 48, 35).unwrap();
        let tz: Tz = "America/New_York".parse().unwrap();

        let formatted = format_timestamp(&utc_time, tz, DateTimeFormat::MonthDayYear12);

        // New York is UTC-5 in winter (EST), so 19:48 UTC = 14:48 EST
        assert_eq!(formatted, "Jan 21, 2026 2:48 PM EST");
    }

    #[test]
    fn test_format_utc_to_london_timezone() {
        let utc_time = Utc.with_ymd_and_hms(2026, 7, 15, 12, 30, 0).unwrap();
        let tz: Tz = "Europe/London".parse().unwrap();

        let formatted = format_timestamp(&utc_time, tz, DateTimeFormat::MonthDayYear12);

        // London is UTC+1 in summer (BST), so 12:30 UTC = 13:30 BST
        assert_eq!(formatted, "Jul 15, 2026 1:30 PM BST");
    }

    #[test]
    fn test_format_utc_stays_utc() {
        let utc_time = Utc.with_ymd_and_hms(2026, 1, 21, 19, 48, 35).unwrap();

        let formatted = format_timestamp(&utc_time, Tz::UTC, DateTimeFormat::MonthDayYear12);

        assert_eq!(formatted, "Jan 21, 2026 7:48 PM UTC");
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
