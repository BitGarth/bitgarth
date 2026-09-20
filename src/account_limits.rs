pub(crate) use crate::account_mode::NativeAccountMode;
use crate::wallets::{DigitalAssetAccountId, WalletAccountId};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

use crate::payments::account_allowances::AccountAllowances;

pub(crate) use crate::payments::account_allowances::SUPPORTED_ACCOUNT_HARD_CAP;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SupportedAccountKind {
    Native,
    ManualAsset,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SupportedAccountLimitRecord {
    pub(crate) account_id: WalletAccountId,
    pub(crate) kind: SupportedAccountKind,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountActivationState {
    Active,
    Inactive,
}

#[derive(Clone, Debug)]
pub(crate) struct NativeAccountModeRecord {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) admitted_at: DateTime<Utc>,
    pub(crate) supports_balance_sync: bool,
    pub(crate) supports_transaction_sync: bool,
}

pub(crate) fn classify_native_account_modes(
    mut records: Vec<NativeAccountModeRecord>,
    allowances: AccountAllowances,
) -> HashMap<DigitalAssetAccountId, NativeAccountMode> {
    records.sort_by(|left, right| {
        left.admitted_at.cmp(&right.admitted_at).then_with(|| {
            left.account_id
                .to_string()
                .cmp(&right.account_id.to_string())
        })
    });
    let mut balance_count = 0;
    let mut transaction_count = 0;
    records
        .into_iter()
        .map(|record| {
            let mode =
                if !record.supports_balance_sync || balance_count >= allowances.balance_sync() {
                    NativeAccountMode::Inactive
                } else {
                    balance_count += 1;
                    if record.supports_transaction_sync
                        && transaction_count < allowances.transaction_history_sync()
                    {
                        transaction_count += 1;
                        NativeAccountMode::Transactions
                    } else {
                        NativeAccountMode::BalanceOnly
                    }
                };
            (record.account_id, mode)
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClassifiedAccount {
    pub(crate) account_id: WalletAccountId,
    pub(crate) kind: SupportedAccountKind,
    pub(crate) state: AccountActivationState,
}

pub(crate) fn classify_supported_accounts(
    mut accounts: Vec<SupportedAccountLimitRecord>,
    active_limit: usize,
) -> Vec<ClassifiedAccount> {
    accounts.sort_by(|left, right| {
        left.created_at.cmp(&right.created_at).then_with(|| {
            left.account_id
                .to_string()
                .cmp(&right.account_id.to_string())
        })
    });

    accounts
        .into_iter()
        .enumerate()
        .map(|(index, account)| ClassifiedAccount {
            account_id: account.account_id,
            kind: account.kind,
            state: if index < active_limit {
                AccountActivationState::Active
            } else {
                AccountActivationState::Inactive
            },
        })
        .collect()
}

pub(crate) fn would_exceed_supported_account_hard_cap(
    current_supported_count: usize,
    creating_supported_count: usize,
) -> bool {
    current_supported_count.saturating_add(creating_supported_count) > SUPPORTED_ACCOUNT_HARD_CAP
}

#[cfg(test)]
pub(crate) fn native_account_sync_eligible(
    account_state: AccountActivationState,
    account_supports_requested_sync: bool,
    provider_or_plan_supports_requested_sync: bool,
) -> bool {
    account_state == AccountActivationState::Active
        && account_supports_requested_sync
        && provider_or_plan_supports_requested_sync
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::str::FromStr;

    fn account_id(value: &str) -> WalletAccountId {
        WalletAccountId::from_str(value).expect("valid account id")
    }

    fn created_at(hour: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 18, hour, 0, 0).unwrap()
    }

    fn record(
        account_id: WalletAccountId,
        kind: SupportedAccountKind,
        created_at: chrono::DateTime<Utc>,
    ) -> SupportedAccountLimitRecord {
        SupportedAccountLimitRecord {
            account_id,
            kind,
            created_at,
        }
    }

    #[test]
    fn classifies_oldest_accounts_as_active() {
        let older_account_id = account_id("01J00000000000000000000001");
        let newer_account_id = account_id("01J00000000000000000000002");

        let classified = classify_supported_accounts(
            vec![
                record(
                    newer_account_id,
                    SupportedAccountKind::ManualAsset,
                    created_at(12),
                ),
                record(
                    older_account_id,
                    SupportedAccountKind::Native,
                    created_at(11),
                ),
            ],
            1,
        );

        assert_eq!(
            classified,
            vec![
                ClassifiedAccount {
                    account_id: older_account_id,
                    kind: SupportedAccountKind::Native,
                    state: AccountActivationState::Active,
                },
                ClassifiedAccount {
                    account_id: newer_account_id,
                    kind: SupportedAccountKind::ManualAsset,
                    state: AccountActivationState::Inactive,
                },
            ]
        );
    }

    #[test]
    fn uses_account_id_as_created_at_tie_break() {
        let lower_account_id = account_id("01J00000000000000000000001");
        let higher_account_id = account_id("01J00000000000000000000002");
        let created_at = created_at(12);

        let classified = classify_supported_accounts(
            vec![
                record(
                    higher_account_id,
                    SupportedAccountKind::ManualAsset,
                    created_at,
                ),
                record(lower_account_id, SupportedAccountKind::Native, created_at),
            ],
            1,
        );

        assert_eq!(classified[0].account_id, lower_account_id);
        assert_eq!(classified[0].state, AccountActivationState::Active);
        assert_eq!(classified[1].account_id, higher_account_id);
        assert_eq!(classified[1].state, AccountActivationState::Inactive);
    }

    #[test]
    fn marks_accounts_over_limit_inactive() {
        let first_account_id = account_id("01J00000000000000000000001");
        let second_account_id = account_id("01J00000000000000000000002");
        let third_account_id = account_id("01J00000000000000000000003");

        let classified = classify_supported_accounts(
            vec![
                record(
                    first_account_id,
                    SupportedAccountKind::Native,
                    created_at(10),
                ),
                record(
                    second_account_id,
                    SupportedAccountKind::Native,
                    created_at(11),
                ),
                record(
                    third_account_id,
                    SupportedAccountKind::Native,
                    created_at(12),
                ),
            ],
            2,
        );

        assert_eq!(
            classified
                .iter()
                .map(|account| account.state)
                .collect::<Vec<_>>(),
            vec![
                AccountActivationState::Active,
                AccountActivationState::Active,
                AccountActivationState::Inactive,
            ]
        );
    }

    #[test]
    fn hard_cap_rejects_only_when_sum_exceeds_cap() {
        assert!(!would_exceed_supported_account_hard_cap(
            SUPPORTED_ACCOUNT_HARD_CAP - 1,
            1
        ));
        assert!(!would_exceed_supported_account_hard_cap(
            SUPPORTED_ACCOUNT_HARD_CAP,
            0
        ));
        assert!(would_exceed_supported_account_hard_cap(
            SUPPORTED_ACCOUNT_HARD_CAP,
            1
        ));
        assert!(would_exceed_supported_account_hard_cap(usize::MAX, 1));
    }

    #[test]
    fn sync_eligibility_inactive_account_with_sync_slot_row_is_not_eligible() {
        assert!(!native_account_sync_eligible(
            AccountActivationState::Inactive,
            true,
            true,
        ));
    }

    #[test]
    fn sync_eligibility_active_supported_account_without_sync_slot_row_is_eligible() {
        assert!(native_account_sync_eligible(
            AccountActivationState::Active,
            true,
            true,
        ));
    }

    #[test]
    fn sync_eligibility_unsupported_provider_remains_ineligible() {
        assert!(!native_account_sync_eligible(
            AccountActivationState::Active,
            false,
            true,
        ));
        assert!(!native_account_sync_eligible(
            AccountActivationState::Active,
            true,
            false,
        ));
    }

    #[test]
    fn independent_native_modes_keep_supported_admission_prefixes() {
        use crate::payments::account_allowances::AccountAllowances;
        use crate::wallets::DigitalAssetAccountId;

        let ids = (0..202)
            .map(|_| DigitalAssetAccountId::new())
            .collect::<Vec<_>>();
        let records = ids
            .iter()
            .enumerate()
            .map(|(index, id)| NativeAccountModeRecord {
                account_id: *id,
                admitted_at: created_at(0) + chrono::Duration::microseconds(index as i64),
                supports_balance_sync: true,
                supports_transaction_sync: true,
            })
            .collect::<Vec<_>>();
        let free = classify_native_account_modes(
            records.clone(),
            AccountAllowances::try_new(50, 3, 1000).unwrap(),
        );
        assert_eq!(free[&ids[0]], NativeAccountMode::Transactions);
        assert_eq!(free[&ids[2]], NativeAccountMode::Transactions);
        assert_eq!(free[&ids[3]], NativeAccountMode::BalanceOnly);
        assert_eq!(free[&ids[49]], NativeAccountMode::BalanceOnly);
        assert_eq!(free[&ids[50]], NativeAccountMode::Inactive);

        let paid = classify_native_account_modes(
            records,
            AccountAllowances::try_new(200, 200, 1000).unwrap(),
        );
        assert_eq!(paid[&ids[199]], NativeAccountMode::Transactions);
        assert_eq!(paid[&ids[200]], NativeAccountMode::Inactive);
    }

    #[test]
    fn unsupported_native_does_not_use_allowance_and_deletion_fills_vacancy() {
        use crate::payments::account_allowances::AccountAllowances;
        use crate::wallets::DigitalAssetAccountId;

        let ids = (0..4)
            .map(|_| DigitalAssetAccountId::new())
            .collect::<Vec<_>>();
        let records = ids
            .iter()
            .enumerate()
            .map(|(index, id)| NativeAccountModeRecord {
                account_id: *id,
                admitted_at: created_at(0) + chrono::Duration::microseconds(index as i64),
                supports_balance_sync: index != 0,
                supports_transaction_sync: index != 0,
            })
            .collect::<Vec<_>>();
        let allowances = AccountAllowances::try_new(2, 1, 0).unwrap();
        let modes = classify_native_account_modes(records.clone(), allowances);
        assert_eq!(modes[&ids[0]], NativeAccountMode::Inactive);
        assert_eq!(modes[&ids[1]], NativeAccountMode::Transactions);
        assert_eq!(modes[&ids[2]], NativeAccountMode::BalanceOnly);
        assert_eq!(modes[&ids[3]], NativeAccountMode::Inactive);

        let remaining = classify_native_account_modes(
            records.into_iter().skip(1).skip(1).collect(),
            allowances,
        );
        assert_eq!(remaining[&ids[2]], NativeAccountMode::Transactions);
        assert_eq!(remaining[&ids[3]], NativeAccountMode::BalanceOnly);
    }
}
