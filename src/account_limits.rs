use crate::wallets::WalletAccountId;
use chrono::{DateTime, Utc};

pub(crate) const SUPPORTED_ACCOUNT_HARD_CAP: usize = 100;

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
}
