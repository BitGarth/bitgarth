#![cfg(any(feature = "server", test))]

use crate::amounts::UnsignedAmount;
use crate::asset_capabilities::unsynced::UnsyncedAssetInstance;
use crate::wallets::{
    ManualAssetBalanceAssertionId, ManualAssetDisplayScale, rescale_manual_asset_amount,
};
use chrono::NaiveDate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualMigrationAssertion {
    pub(crate) assertion_id: ManualAssetBalanceAssertionId,
    pub(crate) asserted_on: NaiveDate,
    pub(crate) amount: UnsignedAmount,
    pub(crate) source_scale: ManualAssetDisplayScale,
    pub(crate) entered_balance_text: Option<String>,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualMigrationPlannedAssertion {
    pub(crate) assertion_id: ManualAssetBalanceAssertionId,
    pub(crate) asserted_on: NaiveDate,
    pub(crate) amount: UnsignedAmount,
    pub(crate) entered_balance_text: Option<String>,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoinGeckoManualMigrationAccount {
    pub(crate) coingecko_id: String,
    pub(crate) network_id: String,
    pub(crate) decimal_precision: ManualAssetDisplayScale,
    pub(crate) coingecko_platform_id: Option<String>,
    pub(crate) provider_platform_asset_ref: Option<String>,
    pub(crate) assertions: Vec<ManualMigrationAssertion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoinGeckoManualMigrationCandidate {
    pub(crate) target: UnsyncedAssetInstance,
    pub(crate) coingecko_platform_id: Option<String>,
    pub(crate) provider_platform_asset_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoinGeckoManualMigrationPlan {
    Noop {
        target: UnsyncedAssetInstance,
        assertion_count: usize,
    },
    AutoRescale {
        target: UnsyncedAssetInstance,
        assertions: Vec<ManualMigrationPlannedAssertion>,
    },
    ReviewRequired {
        target: UnsyncedAssetInstance,
        assertion_count: usize,
    },
}

pub(crate) fn rescale_lossless(
    amount: UnsignedAmount,
    from: ManualAssetDisplayScale,
    to: ManualAssetDisplayScale,
) -> Option<UnsignedAmount> {
    if amount.value() == 0 {
        return Some(UnsignedAmount::from_u128(0));
    }

    let from = from.as_u8();
    let to = to.as_u8();
    if to >= from {
        return rescale_manual_asset_amount(
            amount,
            ManualAssetDisplayScale::from_u8(from),
            ManualAssetDisplayScale::from_u8(to),
        )
        .ok();
    }

    let divisor = 10_u128.checked_pow(u32::from(from - to))?;
    let value = amount.value();
    if !value.is_multiple_of(divisor) {
        return None;
    }
    Some(UnsignedAmount::from_u128(value / divisor))
}

pub(crate) fn plan_coingecko_manual_migration(
    account: &CoinGeckoManualMigrationAccount,
    candidate: &CoinGeckoManualMigrationCandidate,
) -> Option<CoinGeckoManualMigrationPlan> {
    if !coingecko_candidate_matches(account, candidate) {
        return None;
    }

    if account.decimal_precision == candidate.target.decimal_precision {
        return Some(CoinGeckoManualMigrationPlan::Noop {
            target: candidate.target.clone(),
            assertion_count: account.assertions.len(),
        });
    }

    let mut planned = Vec::with_capacity(account.assertions.len());
    for assertion in &account.assertions {
        let Some(amount) = rescale_lossless(
            assertion.amount,
            account.decimal_precision,
            candidate.target.decimal_precision,
        ) else {
            return Some(CoinGeckoManualMigrationPlan::ReviewRequired {
                target: candidate.target.clone(),
                assertion_count: account.assertions.len(),
            });
        };
        planned.push(ManualMigrationPlannedAssertion {
            assertion_id: assertion.assertion_id,
            asserted_on: assertion.asserted_on,
            amount,
            entered_balance_text: assertion.entered_balance_text.clone(),
            note: assertion.note.clone(),
        });
    }

    Some(CoinGeckoManualMigrationPlan::AutoRescale {
        target: candidate.target.clone(),
        assertions: planned,
    })
}

fn coingecko_candidate_matches(
    account: &CoinGeckoManualMigrationAccount,
    candidate: &CoinGeckoManualMigrationCandidate,
) -> bool {
    if account.coingecko_id != candidate.target.coingecko_id.as_str() {
        return false;
    }
    if account.network_id != candidate.target.id.network_id.as_str() {
        return false;
    }
    if account.coingecko_platform_id.as_deref() != candidate.coingecko_platform_id.as_deref() {
        return false;
    }

    match (
        account.provider_platform_asset_ref.as_deref(),
        candidate.provider_platform_asset_ref.as_deref(),
    ) {
        (Some(account_ref), Some(candidate_ref))
            if account_ref
                .trim()
                .eq_ignore_ascii_case(candidate_ref.trim()) => {}
        (None, None) => {}
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amounts::UnsignedAmount;
    use crate::wallets::{
        ManualAssetBalanceAssertionId, ManualAssetDisplayScale, ValidatedManualAssetUnitCode,
    };
    use chrono::NaiveDate;

    fn amount(value: u128) -> UnsignedAmount {
        UnsignedAmount::from_u128(value)
    }

    fn scale(value: u8) -> ManualAssetDisplayScale {
        ManualAssetDisplayScale::from_u8(value)
    }

    fn coingecko_account(
        coingecko_id: &str,
        network_id: &str,
        amount: UnsignedAmount,
        decimal_precision: ManualAssetDisplayScale,
    ) -> CoinGeckoManualMigrationAccount {
        CoinGeckoManualMigrationAccount {
            coingecko_id: coingecko_id.to_string(),
            network_id: network_id.to_string(),
            decimal_precision,
            coingecko_platform_id: None,
            provider_platform_asset_ref: None,
            assertions: vec![ManualMigrationAssertion {
                assertion_id: ManualAssetBalanceAssertionId::new(),
                asserted_on: NaiveDate::from_ymd_opt(2026, 6, 1).expect("valid date"),
                amount,
                source_scale: decimal_precision,
                entered_balance_text: Some("516.42050900".to_string()),
                note: Some("opening".to_string()),
            }],
        }
    }

    fn target_for_unit(unit: &str) -> UnsyncedAssetInstance {
        let code = ValidatedManualAssetUnitCode::parse(unit).expect("valid code");
        crate::asset_capabilities::manual_migration_targets_for_unit_code(&code)
            .expect("catalog loads")
            .into_iter()
            .next()
            .expect("target")
            .clone()
    }

    fn candidate(target: UnsyncedAssetInstance) -> CoinGeckoManualMigrationCandidate {
        CoinGeckoManualMigrationCandidate {
            target,
            coingecko_platform_id: None,
            provider_platform_asset_ref: None,
        }
    }

    #[test]
    fn rescale_lossless_upscales_exactly() {
        assert_eq!(
            rescale_lossless(amount(123), scale(2), scale(5)),
            Some(amount(123_000))
        );
    }

    #[test]
    fn rescale_lossless_downscales_when_remainder_is_zero() {
        assert_eq!(
            rescale_lossless(amount(51_642_050_900), scale(8), scale(6)),
            Some(amount(516_420_509))
        );
    }

    #[test]
    fn rescale_lossless_rejects_lossy_downscale() {
        assert_eq!(
            rescale_lossless(amount(51_642_050_901), scale(8), scale(6)),
            None
        );
    }

    #[test]
    fn rescale_lossless_rejects_nonzero_overflow() {
        assert_eq!(
            rescale_lossless(amount(u128::MAX), scale(0), scale(1)),
            None
        );
    }

    #[test]
    fn rescale_lossless_accepts_zero_for_large_deltas() {
        assert_eq!(
            rescale_lossless(amount(0), scale(0), scale(255)),
            Some(amount(0))
        );
        assert_eq!(
            rescale_lossless(amount(0), scale(255), scale(0)),
            Some(amount(0))
        );
    }

    #[test]
    fn coingecko_migration_requires_platform_metadata_for_platform_candidates() {
        let target = target_for_unit("USDC");
        let mut platform_candidate = candidate(target.clone());
        platform_candidate.coingecko_platform_id = Some("ethereum".to_string());
        platform_candidate.provider_platform_asset_ref =
            Some("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".to_string());
        let account = coingecko_account("usd-coin", "ethereum-mainnet", amount(1), scale(6));
        assert!(plan_coingecko_manual_migration(&account, &platform_candidate).is_none());

        let mut account = coingecko_account("usd-coin", "ethereum-mainnet", amount(1), scale(6));
        account.coingecko_platform_id = Some("polygon-pos".to_string());
        account.provider_platform_asset_ref =
            Some("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48".to_string());
        assert!(plan_coingecko_manual_migration(&account, &platform_candidate).is_none());

        let candidate_without_platform = candidate(target);
        let mut account_with_platform_only =
            coingecko_account("usd-coin", "ethereum-mainnet", amount(1), scale(6));
        account_with_platform_only.coingecko_platform_id = Some("ethereum".to_string());
        assert!(
            plan_coingecko_manual_migration(
                &account_with_platform_only,
                &candidate_without_platform
            )
            .is_none()
        );
    }

    #[test]
    fn coingecko_migration_noops_when_precision_already_matches() {
        let target = target_for_unit("ADA");
        let account =
            coingecko_account("cardano", "cardano-mainnet", amount(516_420_509), scale(6));

        let plan = plan_coingecko_manual_migration(&account, &candidate(target))
            .expect("matching account should plan");

        assert!(matches!(
            plan,
            CoinGeckoManualMigrationPlan::Noop {
                assertion_count: 1,
                ..
            }
        ));
    }

    #[test]
    fn coingecko_migration_upscales_assertions_losslessly() {
        let target = target_for_unit("SOL");
        let account = coingecko_account("solana", "solana-mainnet", amount(123), scale(6));

        let plan = plan_coingecko_manual_migration(&account, &candidate(target))
            .expect("matching account should plan");

        let CoinGeckoManualMigrationPlan::AutoRescale { assertions, .. } = plan else {
            panic!("expected auto-rescale plan");
        };
        assert_eq!(assertions[0].amount, amount(123_000));
    }

    #[test]
    fn coingecko_migration_downscales_clean_assertions_losslessly() {
        let target = target_for_unit("ADA");
        let account = coingecko_account(
            "cardano",
            "cardano-mainnet",
            amount(51_642_050_900),
            scale(8),
        );

        let plan = plan_coingecko_manual_migration(&account, &candidate(target))
            .expect("matching account should plan");

        let CoinGeckoManualMigrationPlan::AutoRescale { assertions, .. } = plan else {
            panic!("expected auto-rescale plan");
        };
        assert_eq!(assertions[0].amount, amount(516_420_509));
    }

    #[test]
    fn coingecko_migration_marks_lossy_downscale_for_review() {
        let target = target_for_unit("ADA");
        let account = coingecko_account(
            "cardano",
            "cardano-mainnet",
            amount(51_642_050_901),
            scale(8),
        );

        let plan = plan_coingecko_manual_migration(&account, &candidate(target))
            .expect("matching account should plan");

        assert!(matches!(
            plan,
            CoinGeckoManualMigrationPlan::ReviewRequired {
                assertion_count: 1,
                ..
            }
        ));
    }

    #[test]
    fn coingecko_migration_rejects_same_coingecko_id_wrong_network() {
        let target = target_for_unit("USDC");
        let account = coingecko_account("usd-coin", "polygon-mainnet", amount(1), scale(6));

        assert!(plan_coingecko_manual_migration(&account, &candidate(target)).is_none());
    }
}
