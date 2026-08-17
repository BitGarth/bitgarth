#[cfg(feature = "server")]
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
#[cfg(feature = "server")]
use chrono_tz::Tz;
#[cfg(feature = "server")]
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
#[cfg(feature = "server")]
use std::fmt;

use crate::asset_views::CatalogAssetKey;
#[cfg(feature = "server")]
use crate::models::{CurrencyCode, UserTimezone};
#[cfg(feature = "server")]
use crate::wallets::ReportDateParam;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub(crate) enum PriceSubject {
    CatalogAsset(CatalogAssetKey),
}

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum BoundaryKind {
    Opening,
    Closing,
}

#[cfg(feature = "server")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PriceOverride {
    pub(crate) subject: PriceSubject,
    pub(crate) quote_currency: CurrencyCode,
    pub(crate) price_time_utc: DateTime<Utc>,
    pub(crate) price: Decimal,
    pub(crate) source_note: Option<String>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[cfg(feature = "server")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NewPriceOverride {
    pub(crate) subject: PriceSubject,
    pub(crate) quote_currency: CurrencyCode,
    pub(crate) price_time_utc: DateTime<Utc>,
    pub(crate) price: Decimal,
    pub(crate) source_note: Option<String>,
}

#[cfg(feature = "server")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum OverrideLookup {
    SameDayLatestAtOrBefore {
        at: DateTime<Utc>,
        local_day_start_utc: DateTime<Utc>,
        next_local_day_start_utc: DateTime<Utc>,
    },
}

#[cfg(feature = "server")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedPrice {
    pub(crate) price: Decimal,
    pub(crate) source: PriceSource,
}

#[cfg(feature = "server")]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PriceSource {
    UserOverride {
        source_note: Option<String>,
        updated_at: DateTime<Utc>,
    },
    ProviderPrice {
        provider: String,
        provider_asset_id: Option<String>,
        provider_quote_id: Option<String>,
        retrieved_at: DateTime<Utc>,
        license_scope: String,
    },
}

#[cfg(feature = "server")]
pub(crate) const SOURCE_NOTE_MAX_CHARS: usize = 120;

#[cfg(feature = "server")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PriceOverrideValidationError {
    InvalidDecimal(String),
    NonPositivePrice,
    SourceNoteTooLong { max: usize, actual: usize },
    InvalidLocalTimestamp(String),
    AmbiguousLocalTimestamp(String),
    NonexistentLocalTimestamp(String),
}

#[cfg(feature = "server")]
impl fmt::Display for PriceOverrideValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDecimal(_) => write!(f, "Enter a valid decimal price"),
            Self::NonPositivePrice => write!(f, "Price must be greater than zero"),
            Self::SourceNoteTooLong { max, actual } => {
                write!(f, "Source note is too long: max {max}, got {actual}")
            }
            Self::InvalidLocalTimestamp(_) => write!(f, "Enter a valid local timestamp"),
            Self::AmbiguousLocalTimestamp(_) => {
                write!(f, "Local timestamp is ambiguous in the selected timezone")
            }
            Self::NonexistentLocalTimestamp(_) => {
                write!(f, "Local timestamp does not exist in the selected timezone")
            }
        }
    }
}

#[cfg(feature = "server")]
impl std::error::Error for PriceOverrideValidationError {}

#[cfg(feature = "server")]
pub(crate) fn validate_price_decimal(raw: &str) -> Result<Decimal, PriceOverrideValidationError> {
    let trimmed = raw.trim();
    let value = trimmed
        .parse::<Decimal>()
        .map_err(|_| PriceOverrideValidationError::InvalidDecimal(raw.to_string()))?;
    if value <= Decimal::ZERO {
        return Err(PriceOverrideValidationError::NonPositivePrice);
    }
    Ok(value)
}

#[cfg(feature = "server")]
pub(crate) fn validate_source_note(
    raw: Option<String>,
) -> Result<Option<String>, PriceOverrideValidationError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let actual = trimmed.chars().count();
    if actual > SOURCE_NOTE_MAX_CHARS {
        return Err(PriceOverrideValidationError::SourceNoteTooLong {
            max: SOURCE_NOTE_MAX_CHARS,
            actual,
        });
    }
    Ok(Some(trimmed))
}

#[cfg(feature = "server")]
pub(crate) fn local_timestamp_to_utc(
    local_timestamp: &str,
    timezone: UserTimezone,
) -> Result<DateTime<Utc>, PriceOverrideValidationError> {
    let naive =
        NaiveDateTime::parse_from_str(local_timestamp, "%Y-%m-%dT%H:%M:%S").map_err(|_| {
            PriceOverrideValidationError::InvalidLocalTimestamp(local_timestamp.to_string())
        })?;
    let tz: Tz = timezone.into();
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
        chrono::LocalResult::Ambiguous(_, _) => Err(
            PriceOverrideValidationError::AmbiguousLocalTimestamp(local_timestamp.to_string()),
        ),
        chrono::LocalResult::None => Err(PriceOverrideValidationError::NonexistentLocalTimestamp(
            local_timestamp.to_string(),
        )),
    }
}

#[cfg(feature = "server")]
pub(crate) fn report_boundary_utc(
    date: ReportDateParam,
    boundary: BoundaryKind,
    timezone: UserTimezone,
) -> Result<DateTime<Utc>, PriceOverrideValidationError> {
    let local_text = match boundary {
        BoundaryKind::Opening => format!("{date}T00:00:00"),
        BoundaryKind::Closing => format!("{date}T23:59:59"),
    };
    local_timestamp_to_utc(&local_text, timezone)
}

pub(crate) fn price_subject_sort_key(subject: &PriceSubject) -> String {
    match subject {
        PriceSubject::CatalogAsset(id) => format!("asset:{}", id.as_str()),
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    fn tz(name: &str) -> UserTimezone {
        UserTimezone(name.parse().expect("valid timezone"))
    }

    #[test]
    fn validate_price_decimal_rejects_non_positive_values() {
        assert!(matches!(
            validate_price_decimal("0"),
            Err(PriceOverrideValidationError::NonPositivePrice)
        ));
        assert!(matches!(
            validate_price_decimal("-1"),
            Err(PriceOverrideValidationError::NonPositivePrice)
        ));
    }

    #[test]
    fn validate_price_decimal_rejects_non_decimal() {
        assert!(matches!(
            validate_price_decimal("abc"),
            Err(PriceOverrideValidationError::InvalidDecimal(_))
        ));
        assert!(matches!(
            validate_price_decimal(""),
            Err(PriceOverrideValidationError::InvalidDecimal(_))
        ));
    }

    #[test]
    fn validate_price_decimal_accepts_valid() {
        let d = validate_price_decimal("42.50").expect("valid decimal");
        assert_eq!(d, Decimal::from_str_exact("42.50").unwrap());
    }

    #[test]
    fn validate_source_note_trims_and_rejects_long_notes() {
        assert_eq!(
            validate_source_note(Some("  exchange screenshot  ".to_string())).expect("valid"),
            Some("exchange screenshot".to_string())
        );
        let long = "x".repeat(SOURCE_NOTE_MAX_CHARS + 1);
        assert!(matches!(
            validate_source_note(Some(long)),
            Err(PriceOverrideValidationError::SourceNoteTooLong {
                max: SOURCE_NOTE_MAX_CHARS,
                actual
            }) if actual == SOURCE_NOTE_MAX_CHARS + 1
        ));
    }

    #[test]
    fn validate_source_note_empty_becomes_none() {
        assert_eq!(
            validate_source_note(Some("  ".to_string())).expect("valid"),
            None
        );
        assert_eq!(validate_source_note(None).expect("valid"), None);
    }

    #[test]
    fn local_timestamp_to_utc_uses_user_timezone() {
        let utc = local_timestamp_to_utc("2025-01-01T00:00:00", tz("America/New_York"))
            .expect("valid local time");
        assert_eq!(utc.year(), 2025);
        assert_eq!(utc.month(), 1);
        assert_eq!(utc.day(), 1);
        assert_eq!(utc.hour(), 5);
    }

    #[test]
    fn report_boundary_utc_uses_opening_and_closing_local_times() {
        let date = ReportDateParam::from_naive_date(
            chrono::NaiveDate::from_ymd_opt(2025, 1, 1).expect("valid date"),
        );
        let opening = report_boundary_utc(date, BoundaryKind::Opening, tz("Europe/Amsterdam"))
            .expect("opening");
        let closing = report_boundary_utc(date, BoundaryKind::Closing, tz("Europe/Amsterdam"))
            .expect("closing");
        assert_eq!(
            opening.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "2024-12-31T23:00:00Z"
        );
        assert_eq!(
            closing.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "2025-01-01T22:59:59Z"
        );
    }

    #[test]
    fn price_subject_sort_key_uses_catalog_asset() {
        assert_eq!(
            price_subject_sort_key(&PriceSubject::CatalogAsset(
                CatalogAssetKey::try_new("bitcoin").expect("valid key")
            )),
            "asset:bitcoin"
        );
    }

    #[test]
    fn catalog_asset_key_subject_wire_format_is_stable_string_id() {
        let subject = PriceSubject::CatalogAsset(
            crate::asset_views::CatalogAssetKey::try_new("bitcoin").expect("valid key"),
        );
        let json = serde_json::to_string(&subject).expect("serialize");
        // MUST match the pre-refactor wire format produced by AssetId.
        assert_eq!(json, r#"{"kind":"catalog_asset","id":"bitcoin"}"#);
        let back: PriceSubject = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, subject);
    }

    #[test]
    fn catalog_sort_key_uses_string_id() {
        let key = crate::asset_views::CatalogAssetKey::try_new("bitcoin").expect("valid key");
        assert_eq!(
            price_subject_sort_key(&PriceSubject::CatalogAsset(key)),
            "asset:bitcoin"
        );
    }

    #[test]
    fn override_lookup_same_day_constructs() {
        let now = Utc::now();
        let lookup = OverrideLookup::SameDayLatestAtOrBefore {
            at: now,
            local_day_start_utc: now,
            next_local_day_start_utc: now,
        };
        assert!(matches!(
            lookup,
            OverrideLookup::SameDayLatestAtOrBefore { .. }
        ));
    }
}
