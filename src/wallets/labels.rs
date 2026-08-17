use super::primitives::WALLET_LABEL_MAX_LENGTH;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawLabel(String);

impl RawLabel {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn validate(self, max_len: usize) -> Result<Label, LabelError> {
        Label::parse_with_limit(&self.0, max_len)
    }
}

/// Canonical uniqueness key for labels.
///
/// Produced by trimming, collapsing internal whitespace runs to a single space,
/// and lowercasing. Two labels with the same `LabelKey` are considered
/// duplicates for uniqueness purposes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct LabelKey(String);

impl LabelKey {
    /// Wrap an already-canonical key string (e.g. read from the database).
    pub(crate) fn new(canonical: String) -> Self {
        LabelKey(canonical)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LabelKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub(crate) struct ValidatedManualAssetUnitCode(String);

impl ValidatedManualAssetUnitCode {
    pub(crate) const MAX_LEN: usize = 20;

    pub(crate) fn parse(value: &str) -> Result<Self, ManualAssetUnitCodeError> {
        let canonical = value.trim().to_ascii_uppercase();
        if canonical.is_empty() {
            return Err(ManualAssetUnitCodeError::Empty);
        }
        if canonical.len() > Self::MAX_LEN {
            return Err(ManualAssetUnitCodeError::TooLong {
                max: Self::MAX_LEN,
                actual: canonical.len(),
            });
        }
        let mut characters = canonical.chars();
        let Some(first_character) = characters.next() else {
            return Err(ManualAssetUnitCodeError::Empty);
        };
        if !first_character.is_ascii_alphabetic() {
            return Err(ManualAssetUnitCodeError::MustStartWithAsciiLetter);
        }
        if !characters.all(|character| character.is_ascii_alphanumeric()) {
            return Err(ManualAssetUnitCodeError::AsciiAlphanumericOnly);
        }
        Ok(Self(canonical))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ValidatedManualAssetUnitCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for ValidatedManualAssetUnitCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManualAssetUnitCodeError {
    Empty,
    TooLong { max: usize, actual: usize },
    MustStartWithAsciiLetter,
    AsciiAlphanumericOnly,
}

impl fmt::Display for ManualAssetUnitCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManualAssetUnitCodeError::Empty => write!(f, "unit code cannot be empty"),
            ManualAssetUnitCodeError::TooLong { max, actual } => {
                write!(f, "unit code exceeds max length {max}: got {actual}")
            }
            ManualAssetUnitCodeError::MustStartWithAsciiLetter => {
                write!(f, "unit code must start with an ASCII letter")
            }
            ManualAssetUnitCodeError::AsciiAlphanumericOnly => {
                write!(
                    f,
                    "unit code must use only ASCII letters and digits after the first letter"
                )
            }
        }
    }
}

impl std::error::Error for ManualAssetUnitCodeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct ManualAssetDisplayScale(u8);

impl ManualAssetDisplayScale {
    pub(crate) const MAX: u8 = 18;

    pub(crate) const fn from_u8(value: u8) -> Self {
        Self(value)
    }

    pub(crate) fn manual_decimal_precision(
        value: i64,
    ) -> Result<Self, ManualAssetDisplayScaleError> {
        let scale = Self::try_from(value)?;
        if scale.as_u8() > Self::MAX {
            return Err(ManualAssetDisplayScaleError::ManualPrecisionOutOfRange { value });
        }
        Ok(scale)
    }

    #[cfg(test)]
    pub(crate) const fn fixed() -> Self {
        Self(8)
    }

    pub(crate) const fn as_u8(self) -> u8 {
        self.0
    }
}

impl TryFrom<i64> for ManualAssetDisplayScale {
    type Error = ManualAssetDisplayScaleError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        let scale =
            u8::try_from(value).map_err(|_| ManualAssetDisplayScaleError::OutOfRange { value })?;
        Ok(Self(scale))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManualAssetDisplayScaleError {
    OutOfRange { value: i64 },
    ManualPrecisionOutOfRange { value: i64 },
}

impl fmt::Display for ManualAssetDisplayScaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManualAssetDisplayScaleError::OutOfRange { value } => {
                write!(f, "manual asset display scale out of range for u8: {value}")
            }
            ManualAssetDisplayScaleError::ManualPrecisionOutOfRange { value } => {
                write!(
                    f,
                    "manual asset decimal precision must be between 0 and {}: {value}",
                    ManualAssetDisplayScale::MAX
                )
            }
        }
    }
}

impl std::error::Error for ManualAssetDisplayScaleError {}

/// Compute the canonical uniqueness key for a label string.
///
/// 1. Trim leading/trailing whitespace.
/// 2. Collapse internal whitespace runs to a single space.
/// 3. Lowercase.
///
/// Invariant contract:
/// - Equivalent labels that differ only by case/trim/whitespace map to the same key.
/// - Canonicalization is deterministic and idempotent.
pub(crate) fn canonicalize_label(input: &str) -> LabelKey {
    let collapsed: String = input.split_whitespace().collect::<Vec<_>>().join(" ");
    LabelKey(collapsed.to_lowercase())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct Label(String);

impl Label {
    pub(crate) fn parse_with_limit(input: &str, max_len: usize) -> Result<Self, LabelError> {
        let normalized = normalize_label_display(input);
        if normalized.is_empty() {
            return Err(LabelError::Empty);
        }
        if normalized.len() > max_len {
            return Err(LabelError::TooLong {
                max: max_len,
                actual: normalized.len(),
            });
        }
        Ok(Label(normalized))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Compute the canonical uniqueness key for this label.
    pub(crate) fn key(&self) -> LabelKey {
        canonicalize_label(&self.0)
    }
}

/// Normalize a label string for display: trim and collapse internal whitespace
/// runs to a single space, but preserve original casing.
fn normalize_label_display(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn truncate_to_max_bytes(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }

    let mut end = 0usize;
    for (idx, ch) in input.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    input[..end].to_string()
}

impl<'de> Deserialize<'de> for Label {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Label::parse_with_limit(&value, WALLET_LABEL_MAX_LENGTH).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LabelError {
    Empty,
    TooLong { max: usize, actual: usize },
}

impl fmt::Display for LabelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LabelError::Empty => write!(f, "Label cannot be empty"),
            LabelError::TooLong { max, actual } => {
                write!(f, "Label too long: max {max}, got {actual}")
            }
        }
    }
}

impl std::error::Error for LabelError {}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
pub(super) mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::string::string_regex;
    use proptest::test_runner::{Config as ProptestConfig, RngSeed, TestRunner};

    const PROPTEST_CASES: u32 = 128;
    const PROPTEST_SEED: u64 = 20_260_226;

    pub(in super::super) fn deterministic_runner() -> TestRunner {
        TestRunner::new(ProptestConfig {
            cases: PROPTEST_CASES,
            failure_persistence: None,
            rng_seed: RngSeed::Fixed(PROPTEST_SEED),
            ..ProptestConfig::default()
        })
    }

    pub(in super::super) fn label_words_strategy(
        min_words: usize,
        max_words: usize,
    ) -> impl Strategy<Value = Vec<String>> {
        let word = string_regex("[A-Za-z0-9]{1,12}").expect("word regex should be valid");
        prop::collection::vec(word, min_words..=max_words)
    }

    pub(in super::super) fn joined_words(words: &[String]) -> String {
        words.join(" ")
    }

    #[test]
    fn test_label_validation() {
        assert!(Label::parse_with_limit("Main Wallet", WALLET_LABEL_MAX_LENGTH).is_ok());
        assert!(Label::parse_with_limit("", WALLET_LABEL_MAX_LENGTH).is_err());
    }

    #[test]
    fn test_canonicalize_label() {
        // case-insensitive
        assert_eq!(canonicalize_label("Main").as_str(), "main");
        assert_eq!(canonicalize_label("MAIN").as_str(), "main");
        // trim-insensitive
        assert_eq!(canonicalize_label("  Main  ").as_str(), "main");
        // internal whitespace collapse
        assert_eq!(canonicalize_label("Main  Wallet").as_str(), "main wallet");
        assert_eq!(
            canonicalize_label("Main\t\n Wallet").as_str(),
            "main wallet"
        );
        // deterministic
        assert_eq!(
            canonicalize_label("Main Wallet"),
            canonicalize_label("  main   wallet  ")
        );
        // non-equivalent labels stay distinct
        assert_ne!(
            canonicalize_label("Main Wallet"),
            canonicalize_label("MainWallet")
        );
    }

    #[test]
    fn prop_canonicalize_label_is_idempotent_and_deterministic() {
        let mut runner = deterministic_runner();
        let strategy = any::<String>();
        let result = runner.run(&strategy, |input| {
            let first = canonicalize_label(&input);
            let second = canonicalize_label(&input);
            let third = canonicalize_label(first.as_str());
            prop_assert_eq!(first.clone(), second);
            prop_assert_eq!(first, third);
            Ok(())
        });

        assert!(
            result.is_ok(),
            "canonicalize_label property run failed: {result:?}"
        );
    }

    #[test]
    fn prop_canonicalize_label_equivalence_for_case_trim_and_whitespace() {
        let mut runner = deterministic_runner();
        let strategy = label_words_strategy(1, 5);
        let result = runner.run(&strategy, |words| {
            let expected = words
                .iter()
                .map(|word| word.to_lowercase())
                .collect::<Vec<_>>()
                .join(" ");
            let mixed_case_words = words
                .iter()
                .enumerate()
                .map(|(idx, word)| {
                    if idx % 2 == 0 {
                        word.to_uppercase()
                    } else {
                        word.to_lowercase()
                    }
                })
                .collect::<Vec<_>>();
            let variant = format!(" \t{}\n ", mixed_case_words.join("   \n\t  "));

            let canonical = canonicalize_label(&variant);
            prop_assert_eq!(canonical.as_str(), expected);
            Ok(())
        });

        assert!(
            result.is_ok(),
            "canonicalize_label equivalence property run failed: {result:?}"
        );
    }

    #[test]
    fn test_label_key_from_label() {
        let label = Label::parse_with_limit("  Main  Wallet  ", WALLET_LABEL_MAX_LENGTH)
            .expect("valid label");
        assert_eq!(label.as_str(), "Main Wallet"); // display: trimmed + collapsed
        assert_eq!(label.key().as_str(), "main wallet"); // key: lowercased
    }

    #[test]
    fn test_label_normalization_collapses_whitespace() {
        let label =
            Label::parse_with_limit("  a   b  c  ", WALLET_LABEL_MAX_LENGTH).expect("valid label");
        assert_eq!(label.as_str(), "a b c");
    }

    #[test]
    fn validated_manual_asset_unit_code_canonicalizes_and_rejects_invalid_values() {
        let ada = ValidatedManualAssetUnitCode::parse(" ada ").expect("ADA should validate");
        let abc2 = ValidatedManualAssetUnitCode::parse("abc2").expect("ABC2 should validate");
        assert_eq!(ada.to_string(), "ADA");
        assert_eq!(abc2.to_string(), "ABC2");

        assert!(matches!(
            ValidatedManualAssetUnitCode::parse(""),
            Err(ManualAssetUnitCodeError::Empty)
        ));
        assert!(matches!(
            ValidatedManualAssetUnitCode::parse("2abc"),
            Err(ManualAssetUnitCodeError::MustStartWithAsciiLetter)
        ));
        assert!(matches!(
            ValidatedManualAssetUnitCode::parse("ada-1"),
            Err(ManualAssetUnitCodeError::AsciiAlphanumericOnly)
        ));
    }

    #[test]
    fn manual_asset_unit_code_accepts_synced_codes() {
        let btc = ValidatedManualAssetUnitCode::parse("btc").expect("BTC should validate");
        let eth = ValidatedManualAssetUnitCode::parse(" ETH ").expect("ETH should validate");

        assert_eq!(btc.as_str(), "BTC");
        assert_eq!(eth.as_str(), "ETH");
    }

    #[test]
    fn validated_manual_asset_unit_code_rejects_values_longer_than_twenty_chars() {
        let input = "ABCDEFGHIJKLMNOPQRSTU";
        assert!(matches!(
            ValidatedManualAssetUnitCode::parse(input),
            Err(ManualAssetUnitCodeError::TooLong {
                max: 20,
                actual: 21
            })
        ));
    }

    #[test]
    fn manual_asset_decimal_precision_fixed_helper_uses_legacy_baseline() {
        assert_eq!(ManualAssetDisplayScale::fixed().as_u8(), 8);
    }
}
