use super::labels::truncate_to_max_bytes;
use super::labels::{Label, LabelError, LabelKey, ValidatedManualAssetUnitCode};
use super::primitives::{ACCOUNT_LABEL_MAX_LENGTH, AccountIndex, SyncedAssetId};
use std::collections::HashSet;

pub(crate) fn display_wallet_label(label: &Label) -> String {
    label.as_str().to_string()
}

pub(crate) fn display_account_label(label: &Label) -> String {
    label.as_str().to_string()
}

/// Generate a default account label: "{Asset} Account {n}".
///
/// `n` is 1-indexed. The caller is responsible for ensuring uniqueness by
/// incrementing `n` until no conflict is found in the target wallet.
fn generate_account_label(
    asset_id: SyncedAssetId,
    account_number: u32,
) -> Result<Label, LabelError> {
    Label::parse_with_limit(
        &format!("{} Account {account_number}", asset_id.display_name()),
        ACCOUNT_LABEL_MAX_LENGTH,
    )
}

/// Generate a unique account label for the given asset within a wallet.
///
/// Tries `"{Asset} Account 1"`, then `"… 2"`, etc. until a label key is
/// found that does not appear in `existing_label_keys`.
pub(crate) fn generate_unique_account_label(
    asset_id: SyncedAssetId,
    existing_label_keys: &[LabelKey],
) -> Result<Label, LabelError> {
    for n in 1..=1000 {
        let label = generate_account_label(asset_id, n)?;
        if !existing_label_keys.contains(&label.key()) {
            return Ok(label);
        }
    }
    // Fallback: should never be reached in practice
    generate_account_label(asset_id, 1001)
}

fn generate_custom_account_label(
    unit_code: &ValidatedManualAssetUnitCode,
    account_number: u32,
) -> Result<Label, LabelError> {
    Label::parse_with_limit(
        &format!("{unit_code} Account {account_number}"),
        ACCOUNT_LABEL_MAX_LENGTH,
    )
}

pub(crate) fn generate_unique_custom_account_label(
    unit_code: &ValidatedManualAssetUnitCode,
    existing_label_keys: &[LabelKey],
) -> Result<Label, LabelError> {
    for n in 1..=1000 {
        let label = generate_custom_account_label(unit_code, n)?;
        if !existing_label_keys.contains(&label.key()) {
            return Ok(label);
        }
    }

    generate_custom_account_label(unit_code, 1001)
}

/// Generate a unique label for an account being moved into a target wallet.
///
/// If the current label does not conflict, returns it as-is.
/// Otherwise appends `" moved from wallet {source_wallet_label}"`, then
/// numeric suffixes `(2)`, `(3)`, ... until unique.
///
/// Invariant contract:
/// - Returned label is always valid for `ACCOUNT_LABEL_MAX_LENGTH`.
/// - Returned label key never collides with `existing_label_keys`.
/// - For the same inputs, output is deterministic.
pub(crate) fn generate_move_account_label(
    current_label: &Label,
    source_wallet_label: &Label,
    existing_label_keys: &[LabelKey],
) -> Result<Label, LabelError> {
    // If current label doesn't conflict, keep it
    if !existing_label_keys.contains(&current_label.key()) {
        return Ok(current_label.clone());
    }

    // Try base rename: "{current} moved from wallet {source}"
    let base = format!(
        "{} moved from wallet {}",
        current_label.as_str(),
        source_wallet_label.as_str()
    );

    // Truncate base if too long
    let base_truncated = truncate_to_max_bytes(&base, ACCOUNT_LABEL_MAX_LENGTH);

    let base_label = Label::parse_with_limit(&base_truncated, ACCOUNT_LABEL_MAX_LENGTH)?;
    if !existing_label_keys.contains(&base_label.key()) {
        return Ok(base_label);
    }

    // Numeric suffix escalation
    for n in 2..=1000 {
        let suffix = format!(" ({n})");
        let max_base = ACCOUNT_LABEL_MAX_LENGTH.saturating_sub(suffix.len());
        let truncated_base = truncate_to_max_bytes(&base_truncated, max_base);
        let candidate = format!("{truncated_base}{suffix}");
        let label = Label::parse_with_limit(&candidate, ACCOUNT_LABEL_MAX_LENGTH)?;
        if !existing_label_keys.contains(&label.key()) {
            return Ok(label);
        }
    }

    // Fallback
    let fallback_base = truncate_to_max_bytes(
        &base_truncated,
        ACCOUNT_LABEL_MAX_LENGTH.saturating_sub(" (1001)".len()),
    );
    Label::parse_with_limit(&format!("{fallback_base} (1001)"), ACCOUNT_LABEL_MAX_LENGTH)
}

#[cfg(any(target_arch = "wasm32", feature = "desktop", test))]
pub(crate) fn suggest_next_accounts(existing: &[AccountIndex], count: usize) -> Vec<AccountIndex> {
    let mut results = Vec::new();
    let mut used = HashSet::new();
    for account in existing {
        used.insert(account.as_u32());
    }

    let mut candidate = 0u32;
    while results.len() < count {
        if !used.contains(&candidate)
            && let Ok(account) = AccountIndex::new(candidate)
        {
            results.push(account);
        }
        candidate += 1;
    }
    results
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::super::labels::tests::{deterministic_runner, joined_words, label_words_strategy};
    use super::super::labels::{Label, LabelKey, ValidatedManualAssetUnitCode, canonicalize_label};
    use super::super::primitives::{
        ACCOUNT_LABEL_MAX_LENGTH, AccountIndex, SyncedAssetId, WALLET_LABEL_MAX_LENGTH,
    };
    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::TestCaseError;

    #[test]
    fn test_display_helpers() {
        let label = match Label::parse_with_limit("Cold Storage", WALLET_LABEL_MAX_LENGTH) {
            Ok(value) => value,
            Err(err) => panic!("label should be valid: {err}"),
        };
        assert_eq!(display_wallet_label(&label), "Cold Storage");

        let account_label = match Label::parse_with_limit("Savings", ACCOUNT_LABEL_MAX_LENGTH) {
            Ok(value) => value,
            Err(err) => panic!("account label should be valid: {err}"),
        };
        assert_eq!(display_account_label(&account_label), "Savings");
    }

    #[test]
    fn test_generate_account_label() {
        let label = generate_account_label(SyncedAssetId::Bitcoin, 1).expect("valid label");
        assert_eq!(label.as_str(), "Bitcoin Account 1");

        let label = generate_account_label(SyncedAssetId::Ethereum, 3).expect("valid label");
        assert_eq!(label.as_str(), "Ethereum Account 3");
    }

    #[test]
    fn test_generate_unique_account_label_skips_conflicts() {
        let existing = vec![
            canonicalize_label("Bitcoin Account 1"),
            canonicalize_label("Bitcoin Account 2"),
        ];
        let label = generate_unique_account_label(SyncedAssetId::Bitcoin, &existing)
            .expect("should generate unique label");
        assert_eq!(label.as_str(), "Bitcoin Account 3");
    }

    #[test]
    fn test_generate_unique_account_label_first_available() {
        let existing: Vec<LabelKey> = vec![];
        let label = generate_unique_account_label(SyncedAssetId::Ethereum, &existing)
            .expect("should generate unique label");
        assert_eq!(label.as_str(), "Ethereum Account 1");
    }

    #[test]
    fn test_generate_move_account_label_no_conflict() {
        let current =
            Label::parse_with_limit("Savings", ACCOUNT_LABEL_MAX_LENGTH).expect("valid label");
        let source_wallet =
            Label::parse_with_limit("Old Wallet", WALLET_LABEL_MAX_LENGTH).expect("valid label");
        let existing: Vec<LabelKey> = vec![];
        let result = generate_move_account_label(&current, &source_wallet, &existing)
            .expect("should succeed");
        assert_eq!(result.as_str(), "Savings");
    }

    #[test]
    fn test_generate_move_account_label_with_conflict() {
        let current =
            Label::parse_with_limit("Savings", ACCOUNT_LABEL_MAX_LENGTH).expect("valid label");
        let source_wallet =
            Label::parse_with_limit("Old Wallet", WALLET_LABEL_MAX_LENGTH).expect("valid label");
        let existing = vec![canonicalize_label("Savings")];
        let result = generate_move_account_label(&current, &source_wallet, &existing)
            .expect("should succeed");
        assert_eq!(result.as_str(), "Savings moved from wallet Old Wallet");
    }

    #[test]
    fn test_generate_move_account_label_with_numeric_suffix() {
        let current =
            Label::parse_with_limit("Savings", ACCOUNT_LABEL_MAX_LENGTH).expect("valid label");
        let source_wallet =
            Label::parse_with_limit("Old Wallet", WALLET_LABEL_MAX_LENGTH).expect("valid label");
        let existing = vec![
            canonicalize_label("Savings"),
            canonicalize_label("Savings moved from wallet Old Wallet"),
        ];
        let result = generate_move_account_label(&current, &source_wallet, &existing)
            .expect("should succeed");
        assert_eq!(result.as_str(), "Savings moved from wallet Old Wallet (2)");
    }

    #[test]
    fn wallet_account_id_round_trips_through_string() {
        use super::super::primitives::WalletAccountId;
        use std::str::FromStr;
        let wallet_account_id = WalletAccountId::new();
        let parsed = WalletAccountId::from_str(&wallet_account_id.to_string())
            .expect("wallet account id should parse");
        assert_eq!(parsed, wallet_account_id);
    }

    #[test]
    fn generate_unique_custom_account_label_matches_wallet_account_labeling_language() {
        let unit_code = ValidatedManualAssetUnitCode::parse("ADA").expect("ADA should validate");
        let existing = vec![
            canonicalize_label("ADA Account 1"),
            canonicalize_label("ADA Account 2"),
        ];

        let label =
            generate_unique_custom_account_label(&unit_code, &existing).expect("label should work");

        assert_eq!(label.as_str(), "ADA Account 3");
        assert_eq!(label.key(), canonicalize_label("ADA Account 3"));
    }

    #[test]
    fn prop_generate_move_account_label_is_deterministic_and_unique() {
        let mut runner = deterministic_runner();
        let strategy = (
            label_words_strategy(1, 4),
            label_words_strategy(1, 4),
            prop::collection::vec(label_words_strategy(1, 4), 0..8),
        );
        let result = runner.run(
            &strategy,
            |(current_words, source_words, existing_word_sets)| {
                let current = Label::parse_with_limit(
                    &joined_words(&current_words),
                    ACCOUNT_LABEL_MAX_LENGTH,
                )
                .map_err(|err| TestCaseError::fail(format!("current label parse failed: {err}")))?;
                let source =
                    Label::parse_with_limit(&joined_words(&source_words), WALLET_LABEL_MAX_LENGTH)
                        .map_err(|err| {
                            TestCaseError::fail(format!("source label parse failed: {err}"))
                        })?;

                let mut existing: Vec<LabelKey> = existing_word_sets
                    .into_iter()
                    .map(|words| canonicalize_label(&joined_words(&words)))
                    .collect();
                existing.push(current.key());

                let first =
                    generate_move_account_label(&current, &source, &existing).map_err(|err| {
                        TestCaseError::fail(format!("first generation failed: {err}"))
                    })?;
                let second =
                    generate_move_account_label(&current, &source, &existing).map_err(|err| {
                        TestCaseError::fail(format!("second generation failed: {err}"))
                    })?;

                prop_assert_eq!(first.clone(), second);
                prop_assert!(!existing.contains(&first.key()));
                prop_assert!(first.as_str().len() <= ACCOUNT_LABEL_MAX_LENGTH);
                Ok(())
            },
        );

        assert!(
            result.is_ok(),
            "generate_move_account_label deterministic/unique property run failed: {result:?}"
        );
    }

    #[test]
    fn prop_generate_move_account_label_escalates_numeric_suffix_and_respects_max_length() {
        let mut runner = deterministic_runner();
        let strategy = (label_words_strategy(1, 4), label_words_strategy(1, 4));
        let result = runner.run(&strategy, |(current_words, source_words)| {
            let current =
                Label::parse_with_limit(&joined_words(&current_words), ACCOUNT_LABEL_MAX_LENGTH)
                    .map_err(|err| {
                        TestCaseError::fail(format!("current label parse failed: {err}"))
                    })?;
            let source =
                Label::parse_with_limit(&joined_words(&source_words), WALLET_LABEL_MAX_LENGTH)
                    .map_err(|err| {
                        TestCaseError::fail(format!("source label parse failed: {err}"))
                    })?;

            let base = format!("{} moved from wallet {}", current.as_str(), source.as_str());
            let base_truncated = truncate_to_max_bytes(&base, ACCOUNT_LABEL_MAX_LENGTH);
            let base_label = Label::parse_with_limit(&base_truncated, ACCOUNT_LABEL_MAX_LENGTH)
                .map_err(|err| TestCaseError::fail(format!("base label parse failed: {err}")))?;

            let suffix = " (2)";
            let max_base = ACCOUNT_LABEL_MAX_LENGTH.saturating_sub(suffix.len());
            let candidate_base = truncate_to_max_bytes(&base_truncated, max_base);
            let second_candidate = Label::parse_with_limit(
                &format!("{candidate_base}{suffix}"),
                ACCOUNT_LABEL_MAX_LENGTH,
            )
            .map_err(|err| TestCaseError::fail(format!("second candidate parse failed: {err}")))?;

            let existing = vec![current.key(), base_label.key(), second_candidate.key()];
            let result = generate_move_account_label(&current, &source, &existing)
                .map_err(|err| TestCaseError::fail(format!("generation failed: {err}")))?;

            prop_assert!(result.as_str().ends_with(" (3)"));
            prop_assert!(result.as_str().len() <= ACCOUNT_LABEL_MAX_LENGTH);
            Ok(())
        });

        assert!(
            result.is_ok(),
            "generate_move_account_label suffix property run failed: {result:?}"
        );
    }

    #[test]
    fn test_suggest_next_accounts() {
        let mut existing = Vec::new();
        if let Ok(account) = AccountIndex::new(0) {
            existing.push(account);
        }
        if let Ok(account) = AccountIndex::new(1) {
            existing.push(account);
        }
        if let Ok(account) = AccountIndex::new(3) {
            existing.push(account);
        }
        let suggested = suggest_next_accounts(&existing, 2);
        let values: Vec<u32> = suggested.iter().map(|a| a.as_u32()).collect();
        assert_eq!(values, vec![2, 4]);
    }
}
