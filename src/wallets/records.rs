use super::labels::Label;
use super::primitives::{
    AccessorKind, AccountIndex, AccountKind, AddressScheme, AddressSourceType, DerivationPath,
    DigitalAssetAccountId, DigitalAssetAddressId, HdKeyId, IdentitySource, KeyRole, KeySource,
    Network, SyncedAssetId, WalletAccessorId, WalletId,
};
use super::xpub::{ValidatedExtendedPubkey, ValidatedMasterFingerprint};
use crate::account_model::AccountModel;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WalletSummary {
    pub id: WalletId,
    pub master_fingerprint: Option<ValidatedMasterFingerprint>,
    pub identity_source: IdentitySource,
    pub verified_at: Option<DateTime<Utc>>,
    pub label: Label,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WalletAccessorSummary {
    pub id: WalletAccessorId,
    pub accessor_kind: AccessorKind,
    pub accessor_label: Option<Label>,
    pub device_id_hash: Option<String>,
    pub device_model: Option<String>,
    pub accessor_version: Option<String>,
    pub firmware_version: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AccountWithHdKeys {
    pub id: DigitalAssetAccountId,
    pub asset_id: SyncedAssetId,
    pub network: Network,
    pub account_model: AccountModel,
    pub account_kind: AccountKind,
    pub label: Label,
    pub hd_keys: Vec<HdKeyRecord>,
    pub addresses: Vec<DigitalAssetAddressRecord>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AccountWithHdKeys {
    pub(crate) fn primary_account_index(&self) -> Option<AccountIndex> {
        self.hd_keys
            .iter()
            .find(|key| key.key_role == KeyRole::Primary)
            .map(|key| key.derivation_path.account)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct HdKeyRecord {
    pub id: HdKeyId,
    pub key_role: KeyRole,
    pub key_source: KeySource,
    pub verified_by_accessor_id: Option<WalletAccessorId>,
    pub address_scheme: AddressScheme,
    pub extended_pubkey: ValidatedExtendedPubkey,
    pub derivation_path: DerivationPath,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DigitalAssetAddressRecord {
    pub id: DigitalAssetAddressId,
    pub asset_id: SyncedAssetId,
    pub network: Network,
    pub address: String,
    pub address_scheme: AddressScheme,
    pub derivation_change: Option<u32>,
    pub derivation_index: Option<u32>,
    pub source_type: AddressSourceType,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DigitalAssetAddressRecord {
    pub(crate) fn is_receive(&self) -> bool {
        self.derivation_change == Some(0)
    }

    pub(crate) fn is_change(&self) -> bool {
        self.derivation_change == Some(1)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WalletWithDetails {
    pub wallet: WalletSummary,
    pub accessors: Vec<WalletAccessorSummary>,
    pub accounts: Vec<AccountWithHdKeys>,
}
