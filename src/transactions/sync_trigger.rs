#![cfg_attr(
    not(feature = "server"),
    expect(
        dead_code,
        reason = "Transaction sync domain types are primarily exercised on server paths"
    )
)]

use super::types::*;
#[cfg(feature = "server")]
use crate::models::FieldErrors;
use crate::wallets::{
    DigitalAssetAccountId, DigitalAssetAddressId, Network, SyncedAssetId, WalletAccountId,
};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransactionSyncTriggerSource {
    Manual,
    AutoAdd,
    AutoSessionRestore,
    AutoFreshness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawTransactionSyncTriggerSource(String);

impl RawTransactionSyncTriggerSource {
    pub(crate) fn manual() -> Self {
        Self("manual".to_string())
    }

    pub(crate) fn validate(
        self,
    ) -> Result<TransactionSyncTriggerSource, TransactionSyncSourceError> {
        match self.0.trim().to_ascii_lowercase().as_str() {
            "manual" => Ok(TransactionSyncTriggerSource::Manual),
            other => Err(TransactionSyncSourceError::Unsupported(other.to_string())),
        }
    }

    #[cfg(feature = "server")]
    pub(crate) fn try_into_validated(self) -> Result<TransactionSyncTriggerSource, FieldErrors> {
        self.validate().map_err(|err| {
            let mut errors = FieldErrors::new();
            errors.add("source", err.to_string());
            errors
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransactionSyncSourceError {
    Unsupported(String),
}

impl fmt::Display for TransactionSyncSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransactionSyncSourceError::Unsupported(value) => {
                write!(f, "unsupported transaction sync source: {value}")
            }
        }
    }
}

impl std::error::Error for TransactionSyncSourceError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransactionSyncScope {
    User,
    Account { account_id: DigitalAssetAccountId },
    Address { address_id: DigitalAssetAddressId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RawTransactionSyncScope {
    #[default]
    User,
    Account {
        account_id: DigitalAssetAccountId,
    },
    Address {
        address_id: DigitalAssetAddressId,
    },
}

impl RawTransactionSyncScope {
    fn validate(self) -> TransactionSyncScope {
        match self {
            Self::User => TransactionSyncScope::User,
            Self::Account { account_id } => TransactionSyncScope::Account { account_id },
            Self::Address { address_id } => TransactionSyncScope::Address { address_id },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RawTransactionSyncTriggerRequest {
    pub source: RawTransactionSyncTriggerSource,
    #[serde(default)]
    pub scope: RawTransactionSyncScope,
}

impl RawTransactionSyncTriggerRequest {
    #[cfg(feature = "server")]
    pub(crate) fn try_into_validated(
        self,
    ) -> Result<ValidatedTransactionSyncTriggerRequest, FieldErrors> {
        Ok(ValidatedTransactionSyncTriggerRequest {
            source: self.source.try_into_validated()?,
            scope: self.scope.validate(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ValidatedTransactionSyncTriggerRequest {
    pub source: TransactionSyncTriggerSource,
    pub scope: TransactionSyncScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TransactionSyncQueueOutcome {
    Started,
    Queued,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TriggerSyncResponse {
    pub outcome: TransactionSyncQueueOutcome,
    pub sync_run_id: TransactionSyncRunId,
}

// ============ Sync Control Types ============

pub(crate) const SYNC_CONTROL_MIN_ITERATIONS: u32 = 1;
pub(crate) const SYNC_CONTROL_MAX_ITERATIONS: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RawSyncIterationCount(pub(crate) u32);

impl RawSyncIterationCount {
    pub(crate) fn validate(self) -> Result<ValidatedSyncIterationCount, SyncIterationCountError> {
        if self.0 < SYNC_CONTROL_MIN_ITERATIONS {
            return Err(SyncIterationCountError::BelowMinimum);
        }
        if self.0 > SYNC_CONTROL_MAX_ITERATIONS {
            return Err(SyncIterationCountError::AboveMaximum);
        }
        Ok(ValidatedSyncIterationCount(self.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ValidatedSyncIterationCount(u32);

impl ValidatedSyncIterationCount {
    pub(crate) fn value(&self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncIterationCountError {
    BelowMinimum,
    AboveMaximum,
}

impl std::fmt::Display for SyncIterationCountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncIterationCountError::BelowMinimum => write!(
                f,
                "iteration count must be at least {SYNC_CONTROL_MIN_ITERATIONS}"
            ),
            SyncIterationCountError::AboveMaximum => write!(
                f,
                "iteration count must be at most {SYNC_CONTROL_MAX_ITERATIONS}"
            ),
        }
    }
}

impl std::error::Error for SyncIterationCountError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunAccountSyncControlRequest {
    pub iterations: RawSyncIterationCount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SyncControlInvocationResponse {
    pub iterations_requested: u32,
    pub iterations_completed: u32,
    pub addresses_touched: u32,
    pub total_new_transactions: u32,
    pub total_updated_transactions: u32,
    pub backfill_continuing: bool,
    pub stopped_early: bool,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SyncControlAddressState {
    pub address_id: DigitalAssetAddressId,
    pub asset_id: SyncedAssetId,
    pub network: Network,
    pub full_address: String,
    pub truncated_address: String,
    pub last_sync_at: Option<String>,
    pub last_result: Option<String>,
    pub backfill_active: bool,
    pub backfill_cursor_display: Option<String>,
    pub estimated_remaining_pages: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccountSyncControlStateResponse {
    pub account_id: WalletAccountId,
    pub addresses_total: u32,
    pub integration: Option<String>,
    pub addresses: Vec<SyncControlAddressState>,
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn raw_sync_trigger_source_validates_manual() {
        let source = RawTransactionSyncTriggerSource("manual".to_string());
        let validated = source.validate().expect("manual source should validate");
        assert_eq!(validated, TransactionSyncTriggerSource::Manual);
    }

    #[test]
    fn raw_sync_trigger_source_rejects_internal_automatic_sources() {
        let auto_add = RawTransactionSyncTriggerSource("auto_add".to_string());
        let auto_session_restore =
            RawTransactionSyncTriggerSource("auto_session_restore".to_string());
        let auto_freshness = RawTransactionSyncTriggerSource("auto_freshness".to_string());

        assert!(auto_add.validate().is_err());
        assert!(auto_session_restore.validate().is_err());
        assert!(auto_freshness.validate().is_err());
    }

    #[cfg(feature = "server")]
    #[test]
    fn raw_sync_trigger_source_try_into_validated_returns_field_error() {
        let source = RawTransactionSyncTriggerSource("unknown".to_string());
        let errors = source
            .try_into_validated()
            .expect_err("unknown source should fail");
        assert!(errors.first("source").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn raw_sync_trigger_request_defaults_to_user_scope() {
        let request = RawTransactionSyncTriggerRequest {
            source: RawTransactionSyncTriggerSource::manual(),
            scope: RawTransactionSyncScope::default(),
        };

        let validated = request
            .try_into_validated()
            .expect("user-scoped request should validate");

        assert_eq!(validated.source, TransactionSyncTriggerSource::Manual);
        assert_eq!(validated.scope, TransactionSyncScope::User);
    }

    #[cfg(feature = "server")]
    #[test]
    fn raw_sync_trigger_request_validates_account_scope() {
        let account_id = crate::wallets::DigitalAssetAccountId::new();
        let request = RawTransactionSyncTriggerRequest {
            source: RawTransactionSyncTriggerSource::manual(),
            scope: RawTransactionSyncScope::Account { account_id },
        };

        let validated = request
            .try_into_validated()
            .expect("account-scoped request should validate");

        assert_eq!(
            validated.scope,
            TransactionSyncScope::Account { account_id }
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn raw_sync_trigger_request_validates_address_scope() {
        let address_id = crate::wallets::DigitalAssetAddressId::new();
        let request = RawTransactionSyncTriggerRequest {
            source: RawTransactionSyncTriggerSource::manual(),
            scope: RawTransactionSyncScope::Address { address_id },
        };

        let validated = request
            .try_into_validated()
            .expect("address-scoped request should validate");

        assert_eq!(
            validated.scope,
            TransactionSyncScope::Address { address_id }
        );
    }

    #[test]
    fn raw_sync_iteration_count_rejects_zero() {
        let raw = RawSyncIterationCount(0);
        assert!(matches!(
            raw.validate(),
            Err(SyncIterationCountError::BelowMinimum)
        ));
    }

    #[test]
    fn raw_sync_iteration_count_rejects_above_max() {
        let raw = RawSyncIterationCount(101);
        assert!(matches!(
            raw.validate(),
            Err(SyncIterationCountError::AboveMaximum)
        ));
    }

    #[test]
    fn raw_sync_iteration_count_accepts_valid_range() {
        let min = RawSyncIterationCount(1)
            .validate()
            .expect("min should be valid");
        assert_eq!(min.value(), 1);

        let max = RawSyncIterationCount(100)
            .validate()
            .expect("max should be valid");
        assert_eq!(max.value(), 100);
    }
}
