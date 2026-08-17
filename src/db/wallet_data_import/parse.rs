use super::WalletDataImportDbError;
use crate::asset_views::ManualAssetInstanceIdView;
use crate::ethereum::{EthAddress, RawEthAddress};
use crate::payments::types::EntitlementTier;
use crate::wallets::{
    ACCOUNT_LABEL_MAX_LENGTH, AccountIndex, AccountKind, AddressScheme, BtcAddress,
    DerivationCoinType, DerivationPurpose, KeyRole, Label, ManualAssetDisplayScale, Network,
    RawBtcAddress, RawMasterFingerprint, SyncedAssetId, ValidatedExtendedPubkey,
    ValidatedMasterFingerprint, WALLET_LABEL_MAX_LENGTH,
};
use crate::wallets::{
    RawManualAssetAssertionNote, ValidatedManualAssetAssertionNote,
    ValidatedManualAssetBalanceLiteral, ValidatedManualAssetUnitCode,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use std::collections::HashSet;

const WALLET_DATA_VERSION_V1: u16 = 1;
const WALLET_DATA_VERSION_V2: u16 = 2;
const WALLET_DATA_VERSION_V3: u16 = 3;
const WALLET_DATA_VERSION_V4: u16 = 4;
const WALLET_DATA_VERSION_V5: u16 = 5;

fn supported_wallet_data_version(version: u16) -> bool {
    matches!(
        version,
        WALLET_DATA_VERSION_V1
            | WALLET_DATA_VERSION_V2
            | WALLET_DATA_VERSION_V3
            | WALLET_DATA_VERSION_V4
            | WALLET_DATA_VERSION_V5
    )
}

#[derive(Debug, Deserialize)]
struct ImportHeader {
    version: u16,
}

#[derive(Debug, Deserialize)]
pub(super) struct WalletDataImportPayload {
    version: u16,
    exported_at: DateTime<Utc>,
    bitgarth_version: String,
    wallets: Vec<WalletDataImportWallet>,
    pub(super) settings: Option<WalletDataImportSettings>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WalletDataImportSettings {
    pub(crate) language: Option<String>,
    pub(crate) date_time_format: Option<String>,
    pub(crate) number_format: Option<String>,
    pub(crate) currency: Option<String>,
    pub(crate) timezone: Option<String>,
    pub(crate) session_duration: Option<String>,
    pub(crate) mempool_base_url: Option<String>,
    pub(crate) etherscan_base_url: Option<String>,
    pub(crate) etherscan_api_key: Option<String>,
    pub(crate) hledger_account_prefix: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletDataImportWallet {
    label: String,
    master_fingerprint: Option<String>,
    identity_source: crate::wallets::IdentitySource,
    verified_at: Option<DateTime<Utc>>,
    accessors: Vec<WalletDataImportAccessor>,
    digital_asset_accounts: Vec<WalletDataImportDigitalAssetAccount>,
    #[serde(default)]
    manual_asset_accounts: Vec<WalletDataImportManualAssetAccountAny>,
    #[serde(default, rename = "legacy_custom_asset_accounts")]
    _ignored_legacy_custom_asset_accounts: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletDataImportAccessor {
    accessor_kind: crate::wallets::AccessorKind,
    accessor_label: Option<String>,
    device_model: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletDataImportDigitalAssetAccount {
    label: String,
    asset_id: SyncedAssetId,
    network: Network,
    account_kind: AccountKind,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    sync_slot: Option<WalletDataImportSyncSlot>,
    hd_keys: Vec<WalletDataImportHdKey>,
    addresses: Vec<WalletDataImportAddress>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WalletDataImportSyncSlot {
    pub(super) selected_at: DateTime<Utc>,
    pub(super) selected_under_tier: EntitlementTier,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletDataImportHdKey {
    key_role: KeyRole,
    extended_pubkey: String,
    derivation_purpose: u32,
    derivation_coin_type: u32,
    derivation_account: u32,
    address_scheme: AddressScheme,
    key_source: crate::wallets::KeySource,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletDataImportAddress {
    address: String,
    address_scheme: AddressScheme,
    source_type: crate::wallets::AddressSourceType,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletDataImportLegacyCustomAssetAccount {
    #[serde(rename = "label")]
    _label: String,
    #[serde(rename = "unit_code")]
    _unit_code: String,
    #[serde(rename = "display_scale")]
    _decimal_precision: u8,
    #[serde(rename = "balance_assertions")]
    _balance_assertions: Vec<WalletDataImportBalanceAssertion>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletDataImportManualAssetAccount {
    label: String,
    asset_instance_id: ManualAssetInstanceIdView,
    #[serde(default)]
    created_at: Option<DateTime<Utc>>,
    unit_code: Option<String>,
    decimal_precision: Option<u8>,
    symbol: Option<String>,
    asset_name: Option<String>,
    network_name: Option<String>,
    coingecko_id: Option<String>,
    #[serde(default)]
    asset_source: Option<String>,
    #[serde(default)]
    precision_source: Option<String>,
    #[serde(default)]
    coingecko_platform_id: Option<String>,
    #[serde(default)]
    provider_platform_asset_ref: Option<String>,
    balance_assertions: Vec<WalletDataImportBalanceAssertion>,
}

/// V4 exports may carry either the new structured manual rows (with
/// `asset_instance_id`) or older legacy-shaped rows (with `unit_code` /
/// `decimal_precision`) inside `manual_asset_accounts`. Accept both via an
/// untagged enum so backward-compat is preserved.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WalletDataImportManualAssetAccountAny {
    Structured(Box<WalletDataImportManualAssetAccount>),
    Legacy(WalletDataImportLegacyCustomAssetAccount),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletDataImportBalanceAssertion {
    asserted_on: NaiveDate,
    balance_amount: String,
    note: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedImportedHdKey {
    pub(super) key_role: KeyRole,
    pub(super) value: ValidatedExtendedPubkey,
    pub(super) derivation_purpose: DerivationPurpose,
    pub(super) derivation_coin_type: DerivationCoinType,
    pub(super) derivation_account: AccountIndex,
    pub(super) address_scheme: AddressScheme,
    pub(super) display_identifier: String,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedImportedAddress {
    pub(super) asset_id: SyncedAssetId,
    pub(super) network: Network,
    pub(super) canonical_address: String,
    pub(super) normalized_address: String,
    pub(super) address_scheme: AddressScheme,
    pub(super) display_identifier: String,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedImportedNativeAccount {
    pub(super) label: Label,
    pub(super) asset_id: SyncedAssetId,
    pub(super) network: Network,
    pub(super) account_kind: AccountKind,
    pub(super) created_at: Option<DateTime<Utc>>,
    pub(super) sync_slot: Option<WalletDataImportSyncSlot>,
    pub(super) hd_keys: Vec<ParsedImportedHdKey>,
    pub(super) addresses: Vec<ParsedImportedAddress>,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedImportedBalanceAssertion {
    pub(super) asserted_on: NaiveDate,
    pub(super) balance: ValidatedManualAssetBalanceLiteral,
    pub(super) note: Option<ValidatedManualAssetAssertionNote>,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedImportedManualAccount {
    pub(super) label: Label,
    pub(super) created_at: Option<DateTime<Utc>>,
    pub(super) snapshot: ParsedImportedManualAssetSnapshot,
    pub(super) assertions: Vec<ParsedImportedBalanceAssertion>,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedImportedManualAssetSnapshot {
    pub(super) asset_id: String,
    pub(super) network_id: String,
    pub(super) unit_code: ValidatedManualAssetUnitCode,
    pub(super) decimal_precision: ManualAssetDisplayScale,
    pub(super) symbol: Option<String>,
    pub(super) asset_name: String,
    pub(super) network_name: String,
    pub(super) coingecko_id: String,
    pub(super) asset_source: String,
    pub(super) precision_source: String,
    pub(super) coingecko_platform_id: Option<String>,
    pub(super) provider_platform_asset_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedImportedWallet {
    pub(super) label: Label,
    pub(super) master_fingerprint: Option<ValidatedMasterFingerprint>,
    pub(super) native_accounts: Vec<ParsedImportedNativeAccount>,
    pub(super) manual_accounts: Vec<ParsedImportedManualAccount>,
    pub(super) ignored_accessors_count: usize,
}

pub(super) fn parse_payload(
    payload_json: &str,
) -> Result<WalletDataImportPayload, WalletDataImportDbError> {
    let value: serde_json::Value = serde_json::from_str(payload_json)
        .map_err(|_| WalletDataImportDbError::BadRequest(super::BAD_JSON_MESSAGE.to_string()))?;

    let header: ImportHeader = serde_json::from_value(value.clone()).map_err(|_| {
        WalletDataImportDbError::Validation(
            "Wallet data export is missing a valid numeric version field.".to_string(),
        )
    })?;

    if header.version > WALLET_DATA_VERSION_V5 {
        return Err(WalletDataImportDbError::Validation(
            super::NEWER_VERSION_MESSAGE.to_string(),
        ));
    }
    if !supported_wallet_data_version(header.version) {
        return Err(WalletDataImportDbError::Validation(format!(
            "Unsupported wallet data export version: {}",
            header.version
        )));
    }

    let payload: WalletDataImportPayload = serde_json::from_value(value).map_err(|err| {
        WalletDataImportDbError::Validation(format!(
            "Wallet data export schema validation failed: {err}"
        ))
    })?;

    if payload.version > WALLET_DATA_VERSION_V5 {
        return Err(WalletDataImportDbError::Validation(
            super::NEWER_VERSION_MESSAGE.to_string(),
        ));
    }
    if !supported_wallet_data_version(payload.version) {
        return Err(WalletDataImportDbError::Validation(format!(
            "Unsupported wallet data export version: {}",
            payload.version
        )));
    }

    let _ = payload.exported_at;
    let _ = payload.bitgarth_version.as_str();

    Ok(payload)
}

fn trim_to_max_bytes(input: &str, max_bytes: usize) -> String {
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

pub(super) fn unique_label_with_numeric_suffix(
    preferred: &Label,
    existing_keys: &HashSet<crate::wallets::LabelKey>,
    max_len: usize,
) -> Result<Label, WalletDataImportDbError> {
    if !existing_keys.contains(&preferred.key()) {
        return Ok(preferred.clone());
    }

    for suffix in 2..=1000_u32 {
        let suffix_text = format!(" ({suffix})");
        let base_max_len = max_len.saturating_sub(suffix_text.len());
        let candidate = format!(
            "{}{}",
            trim_to_max_bytes(preferred.as_str(), base_max_len),
            suffix_text
        );
        let parsed = Label::parse_with_limit(&candidate, max_len).map_err(|err| {
            WalletDataImportDbError::Validation(format!(
                "Failed to build deterministic label collision suffix: {err}"
            ))
        })?;
        if !existing_keys.contains(&parsed.key()) {
            return Ok(parsed);
        }
    }

    let fallback_suffix = " (1001)";
    let fallback_base = trim_to_max_bytes(
        preferred.as_str(),
        max_len.saturating_sub(fallback_suffix.len()),
    );
    Label::parse_with_limit(&format!("{fallback_base}{fallback_suffix}"), max_len).map_err(|err| {
        WalletDataImportDbError::Validation(format!(
            "Failed to generate deterministic fallback label: {err}"
        ))
    })
}

fn parse_wallet_label(raw: &str) -> Result<Label, WalletDataImportDbError> {
    Label::parse_with_limit(raw, WALLET_LABEL_MAX_LENGTH).map_err(|err| {
        WalletDataImportDbError::Validation(format!("Invalid wallet label '{raw}': {err}"))
    })
}

fn parse_account_label(raw: &str) -> Result<Label, WalletDataImportDbError> {
    Label::parse_with_limit(raw, ACCOUNT_LABEL_MAX_LENGTH).map_err(|err| {
        WalletDataImportDbError::Validation(format!("Invalid account label '{raw}': {err}"))
    })
}

fn parse_master_fingerprint(
    raw: Option<&str>,
) -> Result<Option<ValidatedMasterFingerprint>, WalletDataImportDbError> {
    match raw {
        Some(value) => {
            let raw = RawMasterFingerprint::new(value.to_string());
            raw.validate().map(Some).map_err(|err| {
                WalletDataImportDbError::Validation(format!(
                    "Invalid wallet master_fingerprint '{value}': {err}"
                ))
            })
        }
        None => Ok(None),
    }
}

fn parse_imported_native_account(
    account: &WalletDataImportDigitalAssetAccount,
    preserve_account_created_at: bool,
) -> Result<ParsedImportedNativeAccount, WalletDataImportDbError> {
    if account.account_kind == AccountKind::SingleAddress && !account.hd_keys.is_empty() {
        return Err(WalletDataImportDbError::Validation(format!(
            "Account '{}' is single_address but includes hd_keys",
            account.label
        )));
    }

    let label = parse_account_label(&account.label)?;

    let mut hd_keys = Vec::with_capacity(account.hd_keys.len());
    for hd_key in &account.hd_keys {
        let parsed_xpub =
            ValidatedExtendedPubkey::parse(hd_key.address_scheme, &hd_key.extended_pubkey)
                .map_err(|err| {
                    WalletDataImportDbError::Validation(format!(
                        "Invalid extended_pubkey for account '{}': {err}",
                        account.label
                    ))
                })?;

        let derivation_purpose = DerivationPurpose::from_value(hd_key.derivation_purpose)
            .ok_or_else(|| {
                WalletDataImportDbError::Validation(format!(
                    "Invalid derivation_purpose {} for account '{}'",
                    hd_key.derivation_purpose, account.label
                ))
            })?;

        let derivation_account = AccountIndex::new(hd_key.derivation_account).map_err(|err| {
            WalletDataImportDbError::Validation(format!(
                "Invalid derivation_account {} for account '{}': {err}",
                hd_key.derivation_account, account.label
            ))
        })?;

        let _ = hd_key.key_source;

        hd_keys.push(ParsedImportedHdKey {
            key_role: hd_key.key_role,
            value: parsed_xpub,
            derivation_purpose,
            derivation_coin_type: DerivationCoinType::new(hd_key.derivation_coin_type),
            derivation_account,
            address_scheme: hd_key.address_scheme,
            display_identifier: hd_key.extended_pubkey.clone(),
        });
    }

    let mut addresses = Vec::with_capacity(account.addresses.len());
    for imported_address in &account.addresses {
        let parsed = match account.asset_id {
            SyncedAssetId::Bitcoin => {
                let btc = BtcAddress::parse(
                    &RawBtcAddress::new(imported_address.address.clone()),
                    account.network,
                )
                .map_err(|err| {
                    WalletDataImportDbError::Validation(format!(
                        "Invalid Bitcoin address '{}' for account '{}': {err}",
                        imported_address.address, account.label
                    ))
                })?;

                if btc.address_scheme() != imported_address.address_scheme {
                    return Err(WalletDataImportDbError::Validation(format!(
                        "Bitcoin address '{}' has scheme '{}' but export declared '{}'",
                        imported_address.address,
                        btc.address_scheme().as_str(),
                        imported_address.address_scheme.as_str()
                    )));
                }

                ParsedImportedAddress {
                    asset_id: account.asset_id,
                    network: account.network,
                    canonical_address: btc.canonical().to_string(),
                    normalized_address: btc.normalized().to_string(),
                    address_scheme: btc.address_scheme(),
                    display_identifier: imported_address.address.clone(),
                }
            }
            SyncedAssetId::Ethereum => {
                let eth = EthAddress::parse(&RawEthAddress::new(imported_address.address.clone()))
                    .map_err(|err| {
                        WalletDataImportDbError::Validation(format!(
                            "Invalid Ethereum address '{}' for account '{}': {err}",
                            imported_address.address, account.label
                        ))
                    })?;

                if imported_address.address_scheme != AddressScheme::Standard {
                    return Err(WalletDataImportDbError::Validation(format!(
                        "Ethereum address '{}' must use address_scheme=standard",
                        imported_address.address
                    )));
                }

                ParsedImportedAddress {
                    asset_id: account.asset_id,
                    network: account.network,
                    canonical_address: eth.checksummed(),
                    normalized_address: eth.normalized(),
                    address_scheme: AddressScheme::Standard,
                    display_identifier: imported_address.address.clone(),
                }
            }
        };

        let _ = imported_address.source_type;
        addresses.push(parsed);
    }

    if hd_keys.is_empty() && addresses.is_empty() {
        return Err(WalletDataImportDbError::Validation(format!(
            "Account '{}' does not include any identifiers to import",
            account.label
        )));
    }

    if account.account_kind == AccountKind::HdPubkey && hd_keys.is_empty() {
        return Err(WalletDataImportDbError::Validation(format!(
            "HD account '{}' must include at least one hd_key identifier",
            account.label
        )));
    }

    if account.account_kind == AccountKind::SingleAddress && addresses.is_empty() {
        return Err(WalletDataImportDbError::Validation(format!(
            "Single-address account '{}' must include at least one address identifier",
            account.label
        )));
    }

    Ok(ParsedImportedNativeAccount {
        label,
        asset_id: account.asset_id,
        network: account.network,
        account_kind: account.account_kind,
        created_at: if preserve_account_created_at {
            account.created_at
        } else {
            None
        },
        sync_slot: account.sync_slot.clone(),
        hd_keys,
        addresses,
    })
}

fn parse_imported_balance_assertions(
    raw: &[WalletDataImportBalanceAssertion],
    account_label: &str,
    today: NaiveDate,
) -> Result<Vec<ParsedImportedBalanceAssertion>, WalletDataImportDbError> {
    let mut assertions = Vec::with_capacity(raw.len());
    for assertion in raw {
        if assertion.asserted_on > today {
            return Err(WalletDataImportDbError::Validation(format!(
                "Assertion date '{}' for account '{}' cannot be in the future",
                assertion.asserted_on, account_label
            )));
        }

        let balance = ValidatedManualAssetBalanceLiteral::parse(&assertion.balance_amount)
            .map_err(|err| {
                WalletDataImportDbError::Validation(format!(
                    "Invalid balance_amount '{}' for account '{}': {err}",
                    assertion.balance_amount, account_label
                ))
            })?;

        let note = ValidatedManualAssetAssertionNote::parse_optional(
            assertion
                .note
                .as_ref()
                .map(|value| RawManualAssetAssertionNote::new(value.clone())),
        )
        .map_err(|err| {
            WalletDataImportDbError::Validation(format!(
                "Invalid custom assertion note for account '{}': {err}",
                account_label
            ))
        })?;

        assertions.push(ParsedImportedBalanceAssertion {
            asserted_on: assertion.asserted_on,
            balance,
            note,
        });
    }
    Ok(assertions)
}

fn validate_non_empty_snapshot_string(
    field_name: &'static str,
    value: String,
    account_label: &str,
) -> Result<String, WalletDataImportDbError> {
    if value.trim().is_empty() {
        return Err(WalletDataImportDbError::Validation(format!(
            "Invalid manual {field_name} for account '{account_label}': cannot be empty"
        )));
    }
    Ok(value)
}

fn validate_manual_symbol(
    value: Option<String>,
    account_label: &str,
) -> Result<Option<String>, WalletDataImportDbError> {
    value
        .map(|symbol| {
            let mut chars = symbol.chars();
            let Some(_) = chars.next() else {
                return Err(WalletDataImportDbError::Validation(format!(
                    "Invalid manual symbol for account '{account_label}': cannot be empty"
                )));
            };
            if chars.next().is_some() {
                return Err(WalletDataImportDbError::Validation(format!(
                    "Invalid manual symbol for account '{account_label}': must be one character"
                )));
            }
            Ok(symbol)
        })
        .transpose()
}

fn catalog_snapshot_from_view(
    view: &ManualAssetInstanceIdView,
    account_label: &str,
) -> Result<ParsedImportedManualAssetSnapshot, WalletDataImportDbError> {
    let manual_id = crate::asset_capabilities::manual_catalog_candidate_id_from_view(view)
        .map_err(|err| {
            WalletDataImportDbError::Internal(format!(
                "Failed to load manual asset catalog during import: {err}"
            ))
        })?
        .and_then(|id| match id {
            crate::asset_capabilities::ManualAssetCatalogCandidateId::Unsynced(id) => Some(id),
            crate::asset_capabilities::ManualAssetCatalogCandidateId::Synced(_) => None,
        })
        .ok_or_else(|| {
            WalletDataImportDbError::Validation(format!(
                "Unknown manual asset_instance_id for account '{account_label}'"
            ))
        })?;
    let catalog = crate::asset_capabilities::load_unsynced_catalog().map_err(|err| {
        WalletDataImportDbError::Internal(format!(
            "Failed to load manual asset catalog during import: {err}"
        ))
    })?;
    let instance = catalog.instance(&manual_id).ok_or_else(|| {
        WalletDataImportDbError::Internal(format!(
            "Manual asset_instance_id missing from loaded catalog for account '{account_label}'"
        ))
    })?;

    Ok(ParsedImportedManualAssetSnapshot {
        asset_id: instance.id.asset_id.as_str().to_string(),
        network_id: instance.id.network_id.as_str().to_string(),
        unit_code: ValidatedManualAssetUnitCode::parse(instance.unit_code.as_str()).map_err(
            |err| {
                WalletDataImportDbError::Internal(format!(
                    "Invalid catalog unit_code for account '{account_label}': {err}"
                ))
            },
        )?,
        decimal_precision: instance.decimal_precision,
        symbol: instance.symbol.map(|symbol| symbol.as_char().to_string()),
        asset_name: instance.canonical_name.clone(),
        network_name: instance.network_name.clone(),
        coingecko_id: instance.coingecko_id.as_str().to_string(),
        asset_source: "bitgarth_catalog".to_string(),
        precision_source: "bitgarth_catalog".to_string(),
        coingecko_platform_id: None,
        provider_platform_asset_ref: None,
    })
}

fn parse_manual_snapshot(
    account: &WalletDataImportManualAssetAccount,
) -> Result<ParsedImportedManualAssetSnapshot, WalletDataImportDbError> {
    crate::asset_capabilities::unsynced::UnsyncedAssetId::parse(
        &account.asset_instance_id.asset_id,
    )
    .map_err(|err| {
        WalletDataImportDbError::Validation(format!(
            "Invalid manual asset_id for account '{}': {err}",
            account.label
        ))
    })?;
    crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(
        &account.asset_instance_id.network_id,
    )
    .map_err(|err| {
        WalletDataImportDbError::Validation(format!(
            "Invalid manual network_id for account '{}': {err}",
            account.label
        ))
    })?;

    let required_fields_present = [
        account.unit_code.is_some(),
        account.decimal_precision.is_some(),
        account.asset_name.is_some(),
        account.network_name.is_some(),
        account.coingecko_id.is_some(),
    ];
    let present_count = required_fields_present
        .iter()
        .filter(|field_present| **field_present)
        .count();
    if present_count == 0 {
        return catalog_snapshot_from_view(&account.asset_instance_id, &account.label);
    }
    if present_count != required_fields_present.len() {
        return Err(WalletDataImportDbError::Validation(format!(
            "Manual account '{}' has a partial manual asset snapshot; provide all required snapshot fields or none",
            account.label
        )));
    }

    let unit_code_raw = account.unit_code.clone().ok_or_else(|| {
        WalletDataImportDbError::Validation(format!(
            "Manual account '{}' is missing unit_code",
            account.label
        ))
    })?;
    let unit_code = ValidatedManualAssetUnitCode::parse(&unit_code_raw).map_err(|err| {
        WalletDataImportDbError::Validation(format!(
            "Invalid manual unit_code '{}' for account '{}': {err}",
            unit_code_raw, account.label
        ))
    })?;

    let decimal_precision = parse_manual_asset_display_scale(
        account.decimal_precision.ok_or_else(|| {
            WalletDataImportDbError::Validation(format!(
                "Manual account '{}' is missing decimal_precision",
                account.label
            ))
        })?,
        &account.label,
    )?;
    let symbol = validate_manual_symbol(account.symbol.clone(), &account.label)?;
    let asset_name = validate_non_empty_snapshot_string(
        "asset_name",
        account.asset_name.clone().ok_or_else(|| {
            WalletDataImportDbError::Validation(format!(
                "Manual account '{}' is missing asset_name",
                account.label
            ))
        })?,
        &account.label,
    )?;
    let network_name = validate_non_empty_snapshot_string(
        "network_name",
        account.network_name.clone().ok_or_else(|| {
            WalletDataImportDbError::Validation(format!(
                "Manual account '{}' is missing network_name",
                account.label
            ))
        })?,
        &account.label,
    )?;
    let coingecko_id = account.coingecko_id.clone().ok_or_else(|| {
        WalletDataImportDbError::Validation(format!(
            "Manual account '{}' is missing coingecko_id",
            account.label
        ))
    })?;
    crate::asset_capabilities::unsynced::CoingeckoAssetId::parse(&coingecko_id).map_err(|err| {
        WalletDataImportDbError::Validation(format!(
            "Invalid manual coingecko_id for account '{}': {err}",
            account.label
        ))
    })?;
    let asset_source = validate_manual_asset_source(account.asset_source.clone(), &account.label)?;
    let precision_source =
        validate_manual_precision_source(account.precision_source.clone(), &account.label)?;
    let coingecko_platform_id = validate_optional_provider_metadata(
        "coingecko_platform_id",
        account.coingecko_platform_id.clone(),
        &account.label,
    )?;
    let provider_platform_asset_ref = validate_optional_provider_metadata(
        "provider_platform_asset_ref",
        account.provider_platform_asset_ref.clone(),
        &account.label,
    )?;

    Ok(ParsedImportedManualAssetSnapshot {
        asset_id: account.asset_instance_id.asset_id.clone(),
        network_id: account.asset_instance_id.network_id.clone(),
        unit_code,
        decimal_precision,
        symbol,
        asset_name,
        network_name,
        coingecko_id,
        asset_source,
        precision_source,
        coingecko_platform_id,
        provider_platform_asset_ref,
    })
}

fn validate_manual_asset_source(
    value: Option<String>,
    account_label: &str,
) -> Result<String, WalletDataImportDbError> {
    let value = value.unwrap_or_else(|| "bitgarth_catalog".to_string());
    match value.as_str() {
        "bitgarth_catalog" | "coingecko_discovery" => Ok(value),
        _ => Err(WalletDataImportDbError::Validation(format!(
            "Invalid manual asset_source for account '{account_label}'"
        ))),
    }
}

fn validate_manual_precision_source(
    value: Option<String>,
    account_label: &str,
) -> Result<String, WalletDataImportDbError> {
    let value = value.unwrap_or_else(|| "bitgarth_catalog".to_string());
    match value.as_str() {
        "bitgarth_catalog" | "coingecko_platform" | "user_override" | "user_default" => Ok(value),
        _ => Err(WalletDataImportDbError::Validation(format!(
            "Invalid manual precision_source for account '{account_label}'"
        ))),
    }
}

fn validate_optional_provider_metadata(
    field: &'static str,
    value: Option<String>,
    account_label: &str,
) -> Result<Option<String>, WalletDataImportDbError> {
    const MAX_LEN: usize = 256;
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > MAX_LEN {
        return Err(WalletDataImportDbError::Validation(format!(
            "Manual account '{account_label}' {field} must be at most {MAX_LEN} characters"
        )));
    }
    Ok(Some(trimmed.to_string()))
}

fn parse_manual_asset_display_scale(
    value: u8,
    account_label: &str,
) -> Result<ManualAssetDisplayScale, WalletDataImportDbError> {
    ManualAssetDisplayScale::manual_decimal_precision(i64::from(value)).map_err(|err| {
        WalletDataImportDbError::Validation(format!(
            "Invalid decimal_precision '{}' for manual account '{}': {err}",
            value, account_label
        ))
    })
}

fn parse_imported_structured_manual_account(
    account: &WalletDataImportManualAssetAccount,
    today: NaiveDate,
    preserve_account_created_at: bool,
) -> Result<ParsedImportedManualAccount, WalletDataImportDbError> {
    let label = parse_account_label(&account.label)?;
    let snapshot = parse_manual_snapshot(account)?;
    let assertions =
        parse_imported_balance_assertions(&account.balance_assertions, &account.label, today)?;
    Ok(ParsedImportedManualAccount {
        label,
        created_at: if preserve_account_created_at {
            account.created_at
        } else {
            None
        },
        snapshot,
        assertions,
    })
}

fn parse_imported_wallet(
    wallet: &WalletDataImportWallet,
    today: NaiveDate,
    preserve_account_created_at: bool,
) -> Result<ParsedImportedWallet, WalletDataImportDbError> {
    let label = parse_wallet_label(&wallet.label)?;
    let master_fingerprint = parse_master_fingerprint(wallet.master_fingerprint.as_deref())?;
    let _ = wallet.identity_source;
    let _ = wallet.verified_at;

    let native_accounts = wallet
        .digital_asset_accounts
        .iter()
        .map(|account| parse_imported_native_account(account, preserve_account_created_at))
        .collect::<Result<Vec<_>, _>>()?;

    let mut manual_accounts = Vec::<ParsedImportedManualAccount>::new();

    for account in &wallet.manual_asset_accounts {
        match account {
            WalletDataImportManualAssetAccountAny::Structured(structured) => {
                manual_accounts.push(parse_imported_structured_manual_account(
                    structured,
                    today,
                    preserve_account_created_at,
                )?);
            }
            WalletDataImportManualAssetAccountAny::Legacy(legacy) => {
                let _ = legacy;
            }
        }
    }

    let ignored_accessors_count = wallet.accessors.len();
    for accessor in &wallet.accessors {
        let _ = accessor.accessor_kind;
        let _ = accessor.accessor_label.as_ref();
        let _ = accessor.device_model.as_ref();
    }

    Ok(ParsedImportedWallet {
        label,
        master_fingerprint,
        native_accounts,
        manual_accounts,
        ignored_accessors_count,
    })
}

pub(super) fn parse_imported_wallets(
    payload: &WalletDataImportPayload,
    today: NaiveDate,
) -> Result<Vec<ParsedImportedWallet>, WalletDataImportDbError> {
    let preserve_account_created_at = payload.version >= WALLET_DATA_VERSION_V5;
    payload
        .wallets
        .iter()
        .map(|wallet| parse_imported_wallet(wallet, today, preserve_account_created_at))
        .collect()
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::wallets::canonicalize_label;

    #[test]
    fn unique_label_with_numeric_suffix_keeps_non_conflicting_label() {
        let label = Label::parse_with_limit("Main Wallet", WALLET_LABEL_MAX_LENGTH)
            .expect("label should parse");
        let existing = HashSet::<crate::wallets::LabelKey>::new();

        let resolved = unique_label_with_numeric_suffix(&label, &existing, WALLET_LABEL_MAX_LENGTH)
            .expect("label should resolve");

        assert_eq!(resolved.as_str(), "Main Wallet");
    }

    #[test]
    fn unique_label_with_numeric_suffix_adds_incrementing_suffix() {
        let label = Label::parse_with_limit("Main Wallet", WALLET_LABEL_MAX_LENGTH)
            .expect("label should parse");
        let mut existing = HashSet::<crate::wallets::LabelKey>::new();
        existing.insert(canonicalize_label("Main Wallet"));
        existing.insert(canonicalize_label("Main Wallet (2)"));

        let resolved = unique_label_with_numeric_suffix(&label, &existing, WALLET_LABEL_MAX_LENGTH)
            .expect("label should resolve");

        assert_eq!(resolved.as_str(), "Main Wallet (3)");
    }

    #[test]
    fn parse_payload_rejects_newer_versions() {
        let payload = r#"{"version":99,"exported_at":"2026-04-04T12:00:00Z","bitgarth_version":"0.1.0","wallets":[]}"#;
        let parsed = parse_payload(payload);
        assert!(matches!(
            parsed,
            Err(WalletDataImportDbError::Validation(message)) if message == super::super::NEWER_VERSION_MESSAGE
        ));
    }

    #[test]
    fn parse_payload_rejects_invalid_json() {
        let parsed = parse_payload("not-json");
        assert!(matches!(
            parsed,
            Err(WalletDataImportDbError::BadRequest(message)) if message == super::super::BAD_JSON_MESSAGE
        ));
    }

    #[test]
    fn parse_payload_accepts_v1_without_settings() {
        let payload = r#"{"version":1,"exported_at":"2026-04-04T12:00:00Z","bitgarth_version":"0.1.0","wallets":[]}"#;
        let parsed = parse_payload(payload).expect("V1 should parse");
        assert_eq!(parsed.version, 1);
        assert!(parsed.settings.is_none());
    }

    #[test]
    fn parse_payload_accepts_v2_without_settings() {
        let payload = r#"{"version":2,"exported_at":"2026-04-04T12:00:00Z","bitgarth_version":"0.1.0","wallets":[]}"#;
        let parsed = parse_payload(payload).expect("V2 without settings should parse");
        assert_eq!(parsed.version, 2);
        assert!(parsed.settings.is_none());
    }

    #[test]
    fn parse_payload_accepts_v3_with_ignored_premium_transfer() {
        let payload = r#"{"version":3,"exported_at":"2026-04-04T12:00:00Z","bitgarth_version":"0.1.0","wallets":[],"premium_transfer":{"management_secret":"ignored-in-phase-1"}}"#;
        let parsed = parse_payload(payload).expect("V3 should parse");
        assert_eq!(parsed.version, 3);
        assert!(parsed.settings.is_none());
    }

    #[test]
    fn parse_payload_accepts_v4_with_api_keys_and_subscription_transfer() {
        let payload = r#"{"version":4,"exported_at":"2026-04-04T12:00:00Z","bitgarth_version":"0.1.0","wallets":[],"settings":{"language":"nl"},"api_keys":[{"provider":"etherscan","api_key":"KEY123"}],"subscription_transfer":{"management_secret":"ignored-in-task-3"}}"#;
        let parsed = parse_payload(payload).expect("V4 should parse");
        assert_eq!(parsed.version, 4);
        let settings = parsed.settings.expect("settings should parse");
        assert_eq!(settings.language.as_deref(), Some("nl"));
    }

    #[test]
    fn parse_payload_accepts_v2_with_settings() {
        let payload = r#"{"version":2,"exported_at":"2026-04-04T12:00:00Z","bitgarth_version":"0.1.0","wallets":[],"settings":{"theme":"dark","language":"nl","date_time_format":"24h","number_format":"dot","currency":"EUR","timezone":"Europe/Amsterdam","session_duration":"480","raw_sync_history_retention_days":90,"mempool_base_url":"https://mempool.example.com/","etherscan_base_url":"https://api.etherscan.io/","etherscan_api_key":"TESTKEY123"}}"#;
        let parsed = parse_payload(payload).expect("V2 with settings should parse");
        assert_eq!(parsed.version, 2);
        let settings = parsed.settings.expect("settings should be present");
        // legacy `theme` field in the JSON is silently ignored — verifies forward-compat.
        assert_eq!(settings.language.as_deref(), Some("nl"));
        assert_eq!(settings.date_time_format.as_deref(), Some("24h"));
        assert_eq!(settings.number_format.as_deref(), Some("dot"));
        assert_eq!(settings.currency.as_deref(), Some("EUR"));
        assert_eq!(settings.timezone.as_deref(), Some("Europe/Amsterdam"));
        assert_eq!(settings.session_duration.as_deref(), Some("480"));
        assert_eq!(
            settings.mempool_base_url.as_deref(),
            Some("https://mempool.example.com/")
        );
        assert_eq!(
            settings.etherscan_base_url.as_deref(),
            Some("https://api.etherscan.io/")
        );
        assert_eq!(settings.etherscan_api_key.as_deref(), Some("TESTKEY123"));
    }
}
