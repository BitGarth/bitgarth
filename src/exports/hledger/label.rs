use crate::wallets::WalletAccountId;
use std::collections::HashMap;

pub(crate) use crate::models::HLEDGER_ACCOUNT_SEGMENT_MAX_LENGTH as HLEDGER_SEGMENT_MAX_LENGTH;
const HLEDGER_SEGMENT_EMPTY_FALLBACK: &str = "unnamed";
const HLEDGER_COLLISION_DELIMITER: &str = "__";
const ACCOUNT_ID_SUFFIX_LENGTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HledgerAccountSegments {
    pub(crate) account_id: WalletAccountId,
    pub(crate) wallet_segment: String,
    pub(crate) account_segment: String,
}

impl HledgerAccountSegments {
    fn composite_key(&self) -> String {
        format!("{}/{}", self.wallet_segment, self.account_segment)
    }
}

pub(crate) fn normalize_label_for_hledger(input: &str) -> String {
    let filtered: String = input
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        .collect();

    let mut normalized = filtered
        .trim_matches(|ch| matches!(ch, '_' | '-'))
        .to_string();

    if normalized.is_empty() {
        return HLEDGER_SEGMENT_EMPTY_FALLBACK.to_string();
    }

    if normalized.len() > HLEDGER_SEGMENT_MAX_LENGTH {
        normalized.truncate(HLEDGER_SEGMENT_MAX_LENGTH);
        normalized = normalized
            .trim_matches(|ch| matches!(ch, '_' | '-'))
            .to_string();
        if normalized.is_empty() {
            return HLEDGER_SEGMENT_EMPTY_FALLBACK.to_string();
        }
    }

    normalized
}

pub(crate) fn resolve_segment_collisions(
    mut segments: Vec<HledgerAccountSegments>,
) -> Vec<HledgerAccountSegments> {
    let mut original_key_counts: HashMap<String, usize> = HashMap::new();
    for segment in &segments {
        *original_key_counts
            .entry(segment.composite_key())
            .or_default() += 1;
    }

    for segment in &mut segments {
        if original_key_counts
            .get(&segment.composite_key())
            .copied()
            .unwrap_or(0)
            > 1
        {
            let suffix = account_id_suffix(segment.account_id);
            segment.account_segment = append_collision_suffix(&segment.account_segment, &suffix);
        }
    }

    segments
}

fn account_id_suffix(account_id: WalletAccountId) -> String {
    let account_id_text = account_id.to_string();
    let start = account_id_text
        .len()
        .saturating_sub(ACCOUNT_ID_SUFFIX_LENGTH);
    account_id_text[start..].to_string()
}

fn append_collision_suffix(base: &str, suffix: &str) -> String {
    let decorated_suffix = format!("{HLEDGER_COLLISION_DELIMITER}{suffix}");
    if decorated_suffix.len() >= HLEDGER_SEGMENT_MAX_LENGTH {
        return decorated_suffix
            .chars()
            .take(HLEDGER_SEGMENT_MAX_LENGTH)
            .collect();
    }

    let available_base_len = HLEDGER_SEGMENT_MAX_LENGTH - decorated_suffix.len();
    let mut base_prefix = base.to_string();
    if base_prefix.len() > available_base_len {
        base_prefix.truncate(available_base_len);
    }
    format!("{base_prefix}{decorated_suffix}")
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn account_id(value: &str) -> WalletAccountId {
        WalletAccountId::from_str(value).expect("valid ULID")
    }

    #[test]
    fn normalize_label_for_hledger_applies_allowlist() {
        assert_eq!(
            normalize_label_for_hledger(" Main Wallet #1 / BTC "),
            "MainWallet1BTC"
        );
        assert_eq!(
            normalize_label_for_hledger("cold-storage_01"),
            "cold-storage_01"
        );
    }

    #[test]
    fn normalize_label_for_hledger_trims_and_falls_back_to_unnamed() {
        assert_eq!(
            normalize_label_for_hledger("__Main-Wallet__"),
            "Main-Wallet"
        );
        assert_eq!(normalize_label_for_hledger("___---___"), "unnamed");
        assert_eq!(normalize_label_for_hledger("猫🚀"), "unnamed");
    }

    #[test]
    fn normalize_label_for_hledger_limits_length() {
        let input = "a".repeat(300);
        let normalized = normalize_label_for_hledger(&input);
        assert_eq!(normalized.len(), HLEDGER_SEGMENT_MAX_LENGTH);
        assert!(normalized.chars().all(|ch| ch == 'a'));
    }

    #[test]
    fn resolve_segment_collisions_appends_deterministic_suffix() {
        let first_id = account_id("01KGQYDBAH5B0JD0BSF2VX95FR");
        let second_id = account_id("01KGQYDBAH5B0JD0BSF2VX95FS");
        let third_id = account_id("01KGQYDBAH5B0JD0BSF2VX95FT");
        let segments = vec![
            HledgerAccountSegments {
                account_id: first_id,
                wallet_segment: "MainWallet".to_string(),
                account_segment: "BitcoinAccount1".to_string(),
            },
            HledgerAccountSegments {
                account_id: second_id,
                wallet_segment: "MainWallet".to_string(),
                account_segment: "BitcoinAccount1".to_string(),
            },
            HledgerAccountSegments {
                account_id: third_id,
                wallet_segment: "MainWallet".to_string(),
                account_segment: "BitcoinAccount2".to_string(),
            },
        ];

        let resolved = resolve_segment_collisions(segments);
        assert_eq!(resolved[0].wallet_segment, "MainWallet");
        assert_eq!(resolved[1].wallet_segment, "MainWallet");
        assert!(
            resolved[0]
                .account_segment
                .ends_with(&format!("__{}", account_id_suffix(first_id)))
        );
        assert!(
            resolved[1]
                .account_segment
                .ends_with(&format!("__{}", account_id_suffix(second_id)))
        );
        assert_eq!(resolved[2].account_segment, "BitcoinAccount2");
    }

    #[test]
    fn resolve_segment_collisions_keeps_segment_length_bounded() {
        let first_id = account_id("01KGQYDBAH5B0JD0BSF2VX95FR");
        let second_id = account_id("01KGQYDBAH5B0JD0BSF2VX95FS");
        let long_segment = "a".repeat(HLEDGER_SEGMENT_MAX_LENGTH);
        let segments = vec![
            HledgerAccountSegments {
                account_id: first_id,
                wallet_segment: "MainWallet".to_string(),
                account_segment: long_segment.clone(),
            },
            HledgerAccountSegments {
                account_id: second_id,
                wallet_segment: "MainWallet".to_string(),
                account_segment: long_segment,
            },
        ];

        let resolved = resolve_segment_collisions(segments);
        assert_eq!(
            resolved[0].account_segment.len(),
            HLEDGER_SEGMENT_MAX_LENGTH
        );
        assert_eq!(
            resolved[1].account_segment.len(),
            HLEDGER_SEGMENT_MAX_LENGTH
        );
    }
}
