use super::labels::Label;
use super::primitives::{AccountIndex, DerivationPath, WALLET_LABEL_MAX_LENGTH};
use super::xpub::{
    TrezorDeviceId, TrezorDeviceLabel, ValidatedExtendedPubkey, ValidatedMasterFingerprint,
};
use crate::models::FieldErrors;
use std::collections::HashSet;

#[cfg(feature = "server")]
use super::labels::RawLabel;
#[cfg(feature = "server")]
use super::manual_assets::{
    RawManualAssetAssertionNote, RawManualAssetBalance, ValidatedManualAssetAssertionNote,
    ValidatedManualAssetBalanceLiteral,
};
#[cfg(feature = "server")]
use super::primitives::{
    ACCOUNT_LABEL_MAX_LENGTH, AddressScheme, DEFAULT_ACCOUNT_ADDRESSES_PAGE_SIZE,
    DigitalAssetAccountId, MAX_ACCOUNT_ADDRESSES_PAGE_SIZE, Network, ReportDateParam,
    TransactionSortDirection, WalletAccountId, WalletId,
};
#[cfg(feature = "server")]
use super::requests::{
    AddBtcAddressRequest, AddEthAddressRequest, AddManualAssetAccountAssetRequest,
    AddManualAssetAccountRequest, AddXpubRequest, CoinGeckoManualAssetPrecisionSourceRequest,
    CoinGeckoManualAssetSnapshotRequest, GetAccountAddressesRequest, GetAccountTransactionsRequest,
    GetWalletByFingerprintRequest, MoveAccountRequest, MoveDestination, RawTransactionFilters,
    ValidateXpubRequest,
};
#[cfg(feature = "server")]
use chrono::{NaiveDate, Utc};

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedGetWalletByFingerprintRequest {
    pub master_fingerprint: ValidatedMasterFingerprint,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedMoveAccountRequest {
    pub account_id: WalletAccountId,
    pub destination: ValidatedMoveDestination,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedAddManualAssetAccountRequest {
    pub wallet_id: Option<WalletId>,
    pub wallet_label: Option<Label>,
    pub account_label: Option<Label>,
    pub asset: ValidatedAddManualAssetAccountAsset,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ValidatedAddManualAssetAccountAsset {
    BitGarthCatalog {
        candidate_id: crate::asset_capabilities::ManualAssetCatalogCandidateId,
    },
    CoinGeckoDiscovery {
        snapshot: ValidatedCoinGeckoManualAssetSnapshot,
    },
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedCoinGeckoManualAssetSnapshot {
    pub asset_id: crate::asset_capabilities::AssetId,
    pub network_id: crate::asset_capabilities::unsynced::UnsyncedNetworkId,
    pub decimal_precision: super::labels::ManualAssetDisplayScale,
    pub unit_code: super::labels::ValidatedManualAssetUnitCode,
    pub symbol: Option<String>,
    pub asset_name: String,
    pub network_name: String,
    pub coingecko_id: crate::asset_capabilities::unsynced::CoingeckoAssetId,
    pub precision_source: ValidatedCoinGeckoManualAssetPrecisionSource,
    pub coingecko_platform_id: Option<String>,
    pub provider_platform_asset_ref: Option<String>,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValidatedCoinGeckoManualAssetPrecisionSource {
    CoingeckoPlatform,
    UserOverride,
    UserDefault,
}

#[cfg(feature = "server")]
impl ValidatedCoinGeckoManualAssetPrecisionSource {
    pub(crate) const fn as_db_str(self) -> &'static str {
        match self {
            Self::CoingeckoPlatform => "coingecko_platform",
            Self::UserOverride => "user_override",
            Self::UserDefault => "user_default",
        }
    }
}

#[cfg(feature = "server")]
impl From<CoinGeckoManualAssetPrecisionSourceRequest>
    for ValidatedCoinGeckoManualAssetPrecisionSource
{
    fn from(value: CoinGeckoManualAssetPrecisionSourceRequest) -> Self {
        match value {
            CoinGeckoManualAssetPrecisionSourceRequest::CoingeckoPlatform => {
                Self::CoingeckoPlatform
            }
            CoinGeckoManualAssetPrecisionSourceRequest::UserOverride => Self::UserOverride,
            CoinGeckoManualAssetPrecisionSourceRequest::UserDefault => Self::UserDefault,
        }
    }
}

#[cfg(feature = "server")]
impl ValidatedAddManualAssetAccountAsset {
    pub(crate) fn source_label(&self) -> &'static str {
        match self {
            Self::BitGarthCatalog { .. } => "bitgarth_catalog",
            Self::CoinGeckoDiscovery { .. } => "coingecko_discovery",
        }
    }
}

#[cfg(feature = "server")]
impl ValidatedAddManualAssetAccountRequest {
    pub(crate) fn asset_source_label(&self) -> &'static str {
        self.asset.source_label()
    }
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddManualAssetAccountValidationError {
    Fields(FieldErrors),
    Catalog(crate::asset_capabilities::unsynced::UnsyncedCatalogError),
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedGetAccountAddressesRequest {
    pub account_id: DigitalAssetAccountId,
    pub address_scheme: AddressScheme,
    pub page: u32,
    pub page_size: u32,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedGetAccountTransactionsRequest {
    pub account_id: WalletAccountId,
    pub pending_page: u32,
    pub confirmed_page: u32,
    pub sort: TransactionSortDirection,
    pub filters: TransactionFilters,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedAddManualAssetBalanceAssertionRequest {
    pub account_id: WalletAccountId,
    pub asserted_on: NaiveDate,
    pub balance: ValidatedManualAssetBalanceLiteral,
    pub note: Option<ValidatedManualAssetAssertionNote>,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedUpdateManualAssetBalanceAssertionRequest {
    pub assertion_id: super::manual_assets::ManualAssetBalanceAssertionId,
    pub account_id: super::primitives::WalletAccountId,
    pub asserted_on: NaiveDate,
    pub balance: ValidatedManualAssetBalanceLiteral,
    pub note: Option<ValidatedManualAssetAssertionNote>,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransactionFilters {
    pub status: Vec<crate::transactions::ChainTransactionStatus>,
    pub from_date: Option<chrono::DateTime<chrono::Utc>>,
    pub to_date: Option<chrono::DateTime<chrono::Utc>>,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ValidatedMoveDestination {
    ExistingWallet { wallet_id: WalletId },
    NewWallet { label: Label },
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedAddEthAddressRequest {
    pub address: crate::ethereum::EthAddress,
    pub network: Network,
    pub wallet_id: Option<WalletId>,
    pub wallet_label: Option<Label>,
    pub account_label: Option<Label>,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedAddBtcAddressRequest {
    pub address: super::bitcoin::BtcAddress,
    pub network: Network,
    pub wallet_id: Option<WalletId>,
    pub wallet_label: Option<Label>,
    pub account_label: Option<Label>,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedValidateXpubRequest {
    pub extended_pubkey: String,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedAddXpubRequest {
    pub extended_pubkey: ValidatedExtendedPubkey,
    pub wallet_id: Option<WalletId>,
    pub wallet_label: Option<Label>,
    pub account_label: Option<Label>,
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedTrezorAccountLink {
    pub account_index: AccountIndex,
    pub extended_pubkey: ValidatedExtendedPubkey,
    pub derivation_path: DerivationPath,
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedLinkTrezorRequest {
    pub master_fingerprint: ValidatedMasterFingerprint,
    pub wallet_label: Label,
    pub device_id: Option<TrezorDeviceId>,
    pub device_label: Option<TrezorDeviceLabel>,
    pub accounts: Vec<ValidatedTrezorAccountLink>,
}

pub(crate) fn validate_link_trezor_request(
    request: super::requests::LinkTrezorRequest,
) -> Result<ValidatedLinkTrezorRequest, FieldErrors> {
    let mut errors = FieldErrors::new();

    let fingerprint = match request.master_fingerprint.validate() {
        Ok(fingerprint) => Some(fingerprint),
        Err(err) => {
            errors.add("master_fingerprint", err.to_string());
            None
        }
    };

    let wallet_label = match request.wallet_label.validate(WALLET_LABEL_MAX_LENGTH) {
        Ok(label) => Some(label),
        Err(err) => {
            errors.add("wallet_label", err.to_string());
            None
        }
    };

    if request.accounts.is_empty() {
        errors.add("accounts", "At least one account is required".to_string());
    }

    let mut seen_account_and_scheme = HashSet::new();
    let mut validated_accounts = Vec::new();
    for (index, account) in request.accounts.into_iter().enumerate() {
        let field_prefix = format!("accounts[{index}]");

        let account_index = match account.account_index.validate() {
            Ok(value) => value,
            Err(err) => {
                errors.add(&format!("{field_prefix}.account_index"), err.to_string());
                continue;
            }
        };

        if !seen_account_and_scheme.insert((account_index.as_u32(), account.address_scheme)) {
            errors.add(
                &format!("{field_prefix}.account_index"),
                "Duplicate account index for address scheme".to_string(),
            );
            continue;
        }

        let extended_pubkey = match ValidatedExtendedPubkey::parse(
            account.address_scheme,
            account.extended_pubkey.as_str(),
        ) {
            Ok(value) => value,
            Err(err) => {
                errors.add(&format!("{field_prefix}.extended_pubkey"), err.to_string());
                continue;
            }
        };

        let derivation_path =
            DerivationPath::bitcoin_for_address_scheme(account_index, account.address_scheme);
        validated_accounts.push(ValidatedTrezorAccountLink {
            account_index,
            extended_pubkey,
            derivation_path,
        });
    }

    if errors.is_empty() {
        let missing_fingerprint = fingerprint.is_none();
        let missing_wallet_label = wallet_label.is_none();
        match (fingerprint, wallet_label) {
            (Some(fingerprint), Some(wallet_label)) => Ok(ValidatedLinkTrezorRequest {
                master_fingerprint: fingerprint,
                wallet_label,
                device_id: request.device_id,
                device_label: request.device_label,
                accounts: validated_accounts,
            }),
            _ => {
                let mut invariant_errors = FieldErrors::new();
                if missing_fingerprint {
                    invariant_errors.add(
                        "master_fingerprint",
                        "Missing validated master fingerprint".to_string(),
                    );
                }
                if missing_wallet_label {
                    invariant_errors
                        .add("wallet_label", "Missing validated wallet label".to_string());
                }
                Err(invariant_errors)
            }
        }
    } else {
        Err(errors)
    }
}

#[cfg(feature = "server")]
pub(super) fn merge_field_errors(target: &mut FieldErrors, source: FieldErrors) {
    for (field, messages) in source.0 {
        for message in messages {
            target.add(&field, message);
        }
    }
}

/// Validate an optional user-provided account name. `None` (or a value that
/// fails validation) yields `None`; validation failures are recorded on the
/// `account_label` field so the caller's `errors.is_empty()` guard rejects the
/// request. An omitted account label means "auto-name the account".
#[cfg(feature = "server")]
fn validate_optional_account_label(
    label: Option<RawLabel>,
    errors: &mut FieldErrors,
) -> Option<Label> {
    match label {
        Some(raw) => match raw.validate(ACCOUNT_LABEL_MAX_LENGTH) {
            Ok(label) => Some(label),
            Err(err) => {
                errors.add("account_label", err.to_string());
                None
            }
        },
        None => None,
    }
}

#[cfg(feature = "server")]
fn validate_required_label(
    label: Option<RawLabel>,
    max_len: usize,
    field: &str,
) -> Result<Label, FieldErrors> {
    match label {
        Some(raw) => raw.validate(max_len).map_err(|err| {
            let mut errors = FieldErrors::new();
            errors.add(field, err.to_string());
            errors
        }),
        None => {
            let mut errors = FieldErrors::new();
            errors.add(field, "Wallet label is required".to_string());
            Err(errors)
        }
    }
}

#[cfg(feature = "server")]
impl GetWalletByFingerprintRequest {
    pub(crate) fn try_into_validated(
        self,
    ) -> Result<ValidatedGetWalletByFingerprintRequest, FieldErrors> {
        let mut errors = FieldErrors::new();

        let master_fingerprint = match self.master_fingerprint.validate() {
            Ok(value) => Some(value),
            Err(err) => {
                errors.add("master_fingerprint", err.to_string());
                None
            }
        };

        if !errors.is_empty() {
            return Err(errors);
        }

        match master_fingerprint {
            Some(master_fingerprint) => {
                Ok(ValidatedGetWalletByFingerprintRequest { master_fingerprint })
            }
            None => {
                let mut invariant_errors = FieldErrors::new();
                invariant_errors.add(
                    "master_fingerprint",
                    "Missing validated master fingerprint".to_string(),
                );
                Err(invariant_errors)
            }
        }
    }
}

#[cfg(feature = "server")]
impl AddManualAssetAccountRequest {
    pub(crate) fn try_into_validated(
        self,
    ) -> Result<ValidatedAddManualAssetAccountRequest, AddManualAssetAccountValidationError> {
        let mut errors = FieldErrors::new();

        let asset_request = match (self.asset, self.asset_instance_id) {
            (Some(asset), None) => Some(asset),
            (None, Some(asset_instance_id)) => {
                Some(AddManualAssetAccountAssetRequest::BitGarthCatalog { asset_instance_id })
            }
            (Some(_), Some(_)) => {
                errors.add(
                    "asset",
                    "Provide either asset or asset_instance_id, not both.".to_string(),
                );
                None
            }
            (None, None) => {
                errors.add("asset", "Select a manual asset.".to_string());
                None
            }
        };

        let wallet_label = if self.wallet_id.is_none() {
            match validate_required_label(
                self.wallet_label,
                WALLET_LABEL_MAX_LENGTH,
                "wallet_label",
            ) {
                Ok(label) => Some(label),
                Err(label_errors) => {
                    merge_field_errors(&mut errors, label_errors);
                    None
                }
            }
        } else if self.wallet_label.is_some() {
            errors.add(
                "wallet_label",
                "Wallet label must be omitted when adding to an existing wallet".to_string(),
            );
            None
        } else {
            None
        };

        let account_label = validate_optional_account_label(self.account_label, &mut errors);

        if !errors.is_empty() {
            return Err(AddManualAssetAccountValidationError::Fields(errors));
        }

        match asset_request {
            Some(asset_request) => Ok(ValidatedAddManualAssetAccountRequest {
                wallet_id: self.wallet_id,
                wallet_label,
                account_label,
                asset: validate_add_manual_asset_account_asset(asset_request)?,
            }),
            None => {
                let mut invariant_errors = FieldErrors::new();
                invariant_errors.add(
                    "asset_instance_id",
                    "Missing validated asset instance".to_string(),
                );
                Err(AddManualAssetAccountValidationError::Fields(
                    invariant_errors,
                ))
            }
        }
    }
}

#[cfg(feature = "server")]
fn validate_add_manual_asset_account_asset(
    asset: AddManualAssetAccountAssetRequest,
) -> Result<ValidatedAddManualAssetAccountAsset, AddManualAssetAccountValidationError> {
    match asset {
        AddManualAssetAccountAssetRequest::BitGarthCatalog { asset_instance_id } => {
            validate_bitgarth_catalog_manual_asset(asset_instance_id)
        }
        AddManualAssetAccountAssetRequest::CoinGeckoDiscovery { snapshot } => {
            validate_coingecko_manual_asset_snapshot(snapshot).map(|snapshot| {
                ValidatedAddManualAssetAccountAsset::CoinGeckoDiscovery { snapshot }
            })
        }
    }
}

#[cfg(feature = "server")]
fn validate_bitgarth_catalog_manual_asset(
    asset_instance_id: crate::asset_views::ManualAssetInstanceIdView,
) -> Result<ValidatedAddManualAssetAccountAsset, AddManualAssetAccountValidationError> {
    match crate::asset_capabilities::manual_catalog_candidate_id_from_view(&asset_instance_id) {
        Ok(Some(candidate_id)) => {
            Ok(ValidatedAddManualAssetAccountAsset::BitGarthCatalog { candidate_id })
        }
        Ok(None) => {
            let mut errors = FieldErrors::new();
            errors.add(
                "asset_instance_id",
                "Select a supported manual asset.".to_string(),
            );
            Err(AddManualAssetAccountValidationError::Fields(errors))
        }
        Err(err) => Err(AddManualAssetAccountValidationError::Catalog(err)),
    }
}

#[cfg(feature = "server")]
fn validate_coingecko_manual_asset_snapshot(
    snapshot: CoinGeckoManualAssetSnapshotRequest,
) -> Result<ValidatedCoinGeckoManualAssetSnapshot, AddManualAssetAccountValidationError> {
    const DISPLAY_NAME_MAX_LEN: usize = 120;
    const PROVIDER_METADATA_MAX_LEN: usize = 256;

    let mut errors = FieldErrors::new();

    let asset_id = match crate::asset_capabilities::AssetId::owned(snapshot.asset_id) {
        Ok(value) => Some(value),
        Err(err) => {
            errors.add("asset_id", err.to_string());
            None
        }
    };
    let network_id =
        match crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(&snapshot.network_id) {
            Ok(value) => Some(value),
            Err(err) => {
                errors.add("network_id", err.to_string());
                None
            }
        };
    let decimal_precision = match super::labels::ManualAssetDisplayScale::manual_decimal_precision(
        snapshot.decimal_precision,
    ) {
        Ok(value) => Some(value),
        Err(err) => {
            errors.add("decimal_precision", err.to_string());
            None
        }
    };
    let unit_code = match super::labels::ValidatedManualAssetUnitCode::parse(&snapshot.unit_code) {
        Ok(value) => Some(value),
        Err(err) => {
            errors.add("unit_code", err.to_string());
            None
        }
    };
    let asset_name = validate_snapshot_display_text(
        "asset_name",
        snapshot.asset_name,
        DISPLAY_NAME_MAX_LEN,
        &mut errors,
    );
    let network_name = validate_snapshot_display_text(
        "network_name",
        snapshot.network_name,
        DISPLAY_NAME_MAX_LEN,
        &mut errors,
    );
    let coingecko_id = match crate::asset_capabilities::unsynced::CoingeckoAssetId::parse(
        &snapshot.coingecko_id,
    ) {
        Ok(value) => Some(value),
        Err(err) => {
            errors.add("coingecko_id", err.to_string());
            None
        }
    };
    if let Some(coingecko_id) = coingecko_id.as_ref() {
        match crate::asset_capabilities::coingecko_id_is_manual_discovery_excluded(
            coingecko_id.as_str(),
        ) {
            Ok(true) => errors.add(
                "coingecko_id",
                "Select this asset from the BitGarth catalog or synced asset flow.".to_string(),
            ),
            Ok(false) => {}
            Err(err) => return Err(AddManualAssetAccountValidationError::Catalog(err)),
        }
    }
    let coingecko_platform_id = validate_optional_snapshot_metadata(
        "coingecko_platform_id",
        snapshot.coingecko_platform_id,
        PROVIDER_METADATA_MAX_LEN,
        &mut errors,
    );
    let provider_platform_asset_ref = validate_optional_snapshot_metadata(
        "provider_platform_asset_ref",
        snapshot.provider_platform_asset_ref,
        PROVIDER_METADATA_MAX_LEN,
        &mut errors,
    );
    // CoinGecko tickers (e.g. "vvv") are asset codes, not display glyphs. The
    // unit code already carries the code; persisting the ticker in `symbol`
    // makes the amount formatter prefix it like a currency glyph ("vvv1"
    // instead of "1 VVV"). Discovery assets have no glyph, so drop it.
    let _ = &snapshot.symbol;
    let symbol: Option<String> = None;

    if !errors.is_empty() {
        return Err(AddManualAssetAccountValidationError::Fields(errors));
    }

    match (
        asset_id,
        network_id,
        decimal_precision,
        unit_code,
        asset_name,
        network_name,
        coingecko_id,
    ) {
        (
            Some(asset_id),
            Some(network_id),
            Some(decimal_precision),
            Some(unit_code),
            Some(asset_name),
            Some(network_name),
            Some(coingecko_id),
        ) => Ok(ValidatedCoinGeckoManualAssetSnapshot {
            asset_id,
            network_id,
            decimal_precision,
            unit_code,
            symbol,
            asset_name,
            network_name,
            coingecko_id,
            precision_source: snapshot.precision_source.into(),
            coingecko_platform_id,
            provider_platform_asset_ref,
        }),
        _ => {
            let mut invariant_errors = FieldErrors::new();
            invariant_errors.add(
                "asset",
                "Missing validated CoinGecko manual asset fields".to_string(),
            );
            Err(AddManualAssetAccountValidationError::Fields(
                invariant_errors,
            ))
        }
    }
}

#[cfg(feature = "server")]
fn validate_snapshot_display_text(
    field: &'static str,
    value: String,
    max_len: usize,
    errors: &mut FieldErrors,
) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.add(field, "Required.".to_string());
        return None;
    }
    if trimmed.len() > max_len {
        errors.add(field, format!("Must be at most {max_len} characters."));
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(feature = "server")]
fn validate_optional_snapshot_metadata(
    field: &'static str,
    value: Option<String>,
    max_len: usize,
    errors: &mut FieldErrors,
) -> Option<String> {
    let trimmed = value?.trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() > max_len {
        errors.add(field, format!("Must be at most {max_len} characters."));
        return None;
    }
    Some(trimmed)
}

#[cfg(feature = "server")]
fn validate_manual_asset_assertion_fields(
    asserted_on: ReportDateParam,
    balance: RawManualAssetBalance,
    note: Option<RawManualAssetAssertionNote>,
    today: NaiveDate,
    decimal_precision: u8,
) -> Result<
    (
        NaiveDate,
        ValidatedManualAssetBalanceLiteral,
        Option<ValidatedManualAssetAssertionNote>,
    ),
    FieldErrors,
> {
    let mut errors = FieldErrors::new();
    let asserted_on = asserted_on.into_naive_date();
    if asserted_on > today {
        errors.add(
            "asserted_on",
            "assertion date cannot be in the future".to_string(),
        );
    }

    let balance = match ValidatedManualAssetBalanceLiteral::parse(balance.as_str()) {
        Ok(value) => {
            if value.entered_fractional_digits() > decimal_precision {
                errors.add(
                    "balance",
                    format!(
                        "balance has too many decimal places ({} entered, {} max for this asset)",
                        value.entered_fractional_digits(),
                        decimal_precision,
                    ),
                );
                None
            } else {
                Some(value)
            }
        }
        Err(err) => {
            errors.add("balance", err.to_string());
            None
        }
    };

    let note = match ValidatedManualAssetAssertionNote::parse_optional(note) {
        Ok(value) => Some(value),
        Err(err) => {
            errors.add("note", err.to_string());
            None
        }
    };

    if !errors.is_empty() {
        return Err(errors);
    }

    match (balance, note) {
        (Some(balance), Some(note)) => Ok((asserted_on, balance, note)),
        _ => {
            let mut invariant_errors = FieldErrors::new();
            invariant_errors.add(
                "balance",
                "Missing validated manual asset assertion fields".to_string(),
            );
            Err(invariant_errors)
        }
    }
}

#[cfg(feature = "server")]
impl super::manual_assets::AddManualAssetBalanceAssertionRequest {
    pub(crate) fn try_into_validated(
        self,
        decimal_precision: u8,
    ) -> Result<ValidatedAddManualAssetBalanceAssertionRequest, FieldErrors> {
        self.try_into_validated_at(Utc::now().date_naive(), decimal_precision)
    }

    pub(crate) fn try_into_validated_at(
        self,
        today: NaiveDate,
        decimal_precision: u8,
    ) -> Result<ValidatedAddManualAssetBalanceAssertionRequest, FieldErrors> {
        let (asserted_on, balance, note) = validate_manual_asset_assertion_fields(
            self.asserted_on,
            self.balance,
            self.note,
            today,
            decimal_precision,
        )?;
        Ok(ValidatedAddManualAssetBalanceAssertionRequest {
            account_id: self.account_id,
            asserted_on,
            balance,
            note,
        })
    }
}

#[cfg(feature = "server")]
impl super::manual_assets::UpdateManualAssetBalanceAssertionRequest {
    pub(crate) fn try_into_validated(
        self,
        decimal_precision: u8,
    ) -> Result<ValidatedUpdateManualAssetBalanceAssertionRequest, FieldErrors> {
        self.try_into_validated_at(Utc::now().date_naive(), decimal_precision)
    }

    pub(crate) fn try_into_validated_at(
        self,
        today: NaiveDate,
        decimal_precision: u8,
    ) -> Result<ValidatedUpdateManualAssetBalanceAssertionRequest, FieldErrors> {
        let (asserted_on, balance, note) = validate_manual_asset_assertion_fields(
            self.asserted_on,
            self.balance,
            self.note,
            today,
            decimal_precision,
        )?;
        Ok(ValidatedUpdateManualAssetBalanceAssertionRequest {
            assertion_id: self.assertion_id,
            account_id: self.account_id,
            asserted_on,
            balance,
            note,
        })
    }
}

#[cfg(feature = "server")]
impl MoveAccountRequest {
    pub(crate) fn try_into_validated(self) -> Result<ValidatedMoveAccountRequest, FieldErrors> {
        let mut errors = FieldErrors::new();

        let destination = match self.destination {
            MoveDestination::ExistingWallet { wallet_id } => {
                Some(ValidatedMoveDestination::ExistingWallet { wallet_id })
            }
            MoveDestination::NewWallet { label } => match label.validate(WALLET_LABEL_MAX_LENGTH) {
                Ok(validated_label) => Some(ValidatedMoveDestination::NewWallet {
                    label: validated_label,
                }),
                Err(err) => {
                    errors.add("destination.label", err.to_string());
                    None
                }
            },
        };

        if !errors.is_empty() {
            return Err(errors);
        }

        match destination {
            Some(destination) => Ok(ValidatedMoveAccountRequest {
                account_id: self.account_id,
                destination,
            }),
            None => {
                let mut invariant_errors = FieldErrors::new();
                invariant_errors.add("destination", "Missing move destination".to_string());
                Err(invariant_errors)
            }
        }
    }
}

#[cfg(feature = "server")]
impl GetAccountAddressesRequest {
    pub(crate) fn try_into_validated(
        self,
    ) -> Result<ValidatedGetAccountAddressesRequest, FieldErrors> {
        let mut errors = FieldErrors::new();

        let page = self.page.unwrap_or(1);
        if page == 0 {
            errors.add("page", "Page must be greater than 0".to_string());
        }

        let requested_page_size = self
            .page_size
            .unwrap_or(DEFAULT_ACCOUNT_ADDRESSES_PAGE_SIZE);
        if requested_page_size == 0 {
            errors.add("page_size", "Page size must be greater than 0".to_string());
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(ValidatedGetAccountAddressesRequest {
            account_id: self.account_id,
            address_scheme: self.address_scheme,
            page,
            page_size: requested_page_size.min(MAX_ACCOUNT_ADDRESSES_PAGE_SIZE),
        })
    }
}

#[cfg(feature = "server")]
impl GetAccountTransactionsRequest {
    pub(crate) fn try_into_validated(
        self,
    ) -> Result<ValidatedGetAccountTransactionsRequest, FieldErrors> {
        let mut errors = FieldErrors::new();
        let pending_page = self.pending_page.unwrap_or(1);
        let confirmed_page = self.confirmed_page.unwrap_or(1);

        if pending_page == 0 {
            errors.add(
                "pending_page",
                "Pending page must be greater than 0".to_string(),
            );
        }
        if confirmed_page == 0 {
            errors.add(
                "confirmed_page",
                "Confirmed page must be greater than 0".to_string(),
            );
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        let sort = self
            .sort
            .as_deref()
            .map(TransactionSortDirection::from_query_value)
            .unwrap_or(TransactionSortDirection::Descending);

        let parsed_filters = parse_raw_filters(self.filters.as_deref())?;

        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(ValidatedGetAccountTransactionsRequest {
            account_id: self.account_id,
            pending_page,
            confirmed_page,
            sort,
            filters: parsed_filters,
        })
    }
}

#[cfg(feature = "server")]
fn parse_raw_filters(raw: Option<&str>) -> Result<TransactionFilters, FieldErrors> {
    let raw_json = match raw {
        Some(s) if !s.trim().is_empty() => s,
        _ => {
            return Ok(TransactionFilters {
                status: Vec::new(),
                from_date: None,
                to_date: None,
            });
        }
    };

    let raw_filters: RawTransactionFilters = serde_json::from_str(raw_json).map_err(|err| {
        let mut errors = FieldErrors::new();
        errors.add("filters", format!("Invalid filters JSON: {err}"));
        errors
    })?;

    let mut errors = FieldErrors::new();

    let status: Vec<crate::transactions::ChainTransactionStatus> = raw_filters
        .status
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            match crate::transactions::ChainTransactionStatus::from_db_value(s) {
                Some(status) => Some(status),
                None => {
                    errors.add(
                        "filters.status",
                        format!("Unknown status: {s}. Valid: pending, confirmed, dropped, failed"),
                    );
                    None
                }
            }
        })
        .collect();

    let from_date = raw_filters
        .from_date
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&chrono::Utc))
                .map_err(|_| {
                    errors.add(
                        "filters.from_date",
                        "Invalid from_date format (expected RFC 3339)".to_string(),
                    );
                })
                .ok()
        });

    let to_date = raw_filters
        .to_date
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&chrono::Utc))
                .map_err(|_| {
                    errors.add(
                        "filters.to_date",
                        "Invalid to_date format (expected RFC 3339)".to_string(),
                    );
                })
                .ok()
        });

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(TransactionFilters {
        status,
        from_date,
        to_date,
    })
}

#[cfg(feature = "server")]
impl AddEthAddressRequest {
    pub(crate) fn try_into_validated(self) -> Result<ValidatedAddEthAddressRequest, FieldErrors> {
        let mut errors = FieldErrors::new();

        let address = match crate::ethereum::EthAddress::parse(&self.address) {
            Ok(value) => Some(value),
            Err(err) => {
                errors.add("address", err.to_string());
                None
            }
        };

        if !matches!(self.network, Network::Mainnet | Network::Testnet) {
            errors.add(
                "network",
                format!(
                    "Ethereum does not support network: {}",
                    self.network.as_str()
                ),
            );
        }

        let wallet_label = if self.wallet_id.is_none() {
            match validate_required_label(
                self.wallet_label,
                WALLET_LABEL_MAX_LENGTH,
                "wallet_label",
            ) {
                Ok(label) => Some(label),
                Err(label_errors) => {
                    merge_field_errors(&mut errors, label_errors);
                    None
                }
            }
        } else {
            None
        };

        let account_label = validate_optional_account_label(self.account_label, &mut errors);

        if !errors.is_empty() {
            return Err(errors);
        }

        match address {
            Some(address) => Ok(ValidatedAddEthAddressRequest {
                address,
                network: self.network,
                wallet_id: self.wallet_id,
                wallet_label,
                account_label,
            }),
            None => {
                let mut invariant_errors = FieldErrors::new();
                invariant_errors.add("address", "Missing validated address".to_string());
                Err(invariant_errors)
            }
        }
    }
}

#[cfg(feature = "server")]
impl AddBtcAddressRequest {
    pub(crate) fn try_into_validated(self) -> Result<ValidatedAddBtcAddressRequest, FieldErrors> {
        let mut errors = FieldErrors::new();

        let address = match super::bitcoin::BtcAddress::parse(&self.address, self.network) {
            Ok(value) => Some(value),
            Err(err) => {
                errors.add("address", err.to_string());
                None
            }
        };

        if !matches!(self.network, Network::Mainnet | Network::Testnet) {
            errors.add(
                "network",
                format!(
                    "Bitcoin single address does not support network: {}",
                    self.network.as_str()
                ),
            );
        }

        let wallet_label = if self.wallet_id.is_none() {
            match validate_required_label(
                self.wallet_label,
                WALLET_LABEL_MAX_LENGTH,
                "wallet_label",
            ) {
                Ok(label) => Some(label),
                Err(label_errors) => {
                    merge_field_errors(&mut errors, label_errors);
                    None
                }
            }
        } else {
            None
        };

        let account_label = validate_optional_account_label(self.account_label, &mut errors);

        if !errors.is_empty() {
            return Err(errors);
        }

        match address {
            Some(address) => Ok(ValidatedAddBtcAddressRequest {
                address,
                network: self.network,
                wallet_id: self.wallet_id,
                wallet_label,
                account_label,
            }),
            None => {
                let mut invariant_errors = FieldErrors::new();
                invariant_errors.add("address", "Missing validated address".to_string());
                Err(invariant_errors)
            }
        }
    }
}

#[cfg(feature = "server")]
impl ValidateXpubRequest {
    pub(crate) fn try_into_validated(self) -> Result<ValidatedValidateXpubRequest, FieldErrors> {
        match super::xpub::validate_extended_pubkey_format(&self.extended_pubkey) {
            Ok(extended_pubkey) => Ok(ValidatedValidateXpubRequest { extended_pubkey }),
            Err(err) => {
                let mut errors = FieldErrors::new();
                errors.add("extended_pubkey", err.to_string());
                Err(errors)
            }
        }
    }
}

#[cfg(feature = "server")]
impl AddXpubRequest {
    pub(crate) fn try_into_validated(self) -> Result<ValidatedAddXpubRequest, FieldErrors> {
        let mut errors = FieldErrors::new();

        if !matches!(
            self.address_scheme,
            AddressScheme::Legacy | AddressScheme::NestedSegwit | AddressScheme::NativeSegwit
        ) {
            errors.add(
                "address_scheme",
                format!(
                    "Unsupported address scheme: {}",
                    self.address_scheme.as_str()
                ),
            );
        }

        let extended_pubkey =
            match ValidatedExtendedPubkey::parse(self.address_scheme, &self.extended_pubkey) {
                Ok(value) => Some(value),
                Err(err) => {
                    errors.add("extended_pubkey", err.to_string());
                    None
                }
            };

        let wallet_label = if self.wallet_id.is_none() {
            match validate_required_label(
                self.wallet_label,
                WALLET_LABEL_MAX_LENGTH,
                "wallet_label",
            ) {
                Ok(label) => Some(label),
                Err(label_errors) => {
                    merge_field_errors(&mut errors, label_errors);
                    None
                }
            }
        } else {
            None
        };

        let account_label = validate_optional_account_label(self.account_label, &mut errors);

        if !errors.is_empty() {
            return Err(errors);
        }

        match extended_pubkey {
            Some(extended_pubkey) => Ok(ValidatedAddXpubRequest {
                extended_pubkey,
                wallet_id: self.wallet_id,
                wallet_label,
                account_label,
            }),
            None => {
                let mut invariant_errors = FieldErrors::new();
                invariant_errors.add("extended_pubkey", "Missing validated key".to_string());
                Err(invariant_errors)
            }
        }
    }
}

#[cfg(feature = "server")]
pub(crate) fn hash_device_id(raw: &str) -> String {
    use sha2::{Digest, Sha256};

    let result = Sha256::digest(raw.as_bytes());
    format!("sha256:{}", hex::encode(result))
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::super::labels::RawLabel;
    use super::super::primitives::{AddressScheme, RawAccountIndex};
    use super::super::requests::TrezorAccountLinkRequest;
    use super::super::xpub::RawMasterFingerprint;
    use super::*;

    #[cfg(feature = "server")]
    use super::super::bitcoin::RawBtcAddress;
    #[cfg(feature = "server")]
    use super::super::labels::Label;
    #[cfg(feature = "server")]
    use super::super::manual_assets::{
        AddManualAssetBalanceAssertionRequest, RawManualAssetAssertionNote, RawManualAssetBalance,
    };
    #[cfg(feature = "server")]
    use super::super::primitives::{
        DigitalAssetAccountId, ReportDateParam, WalletAccountId, WalletId,
    };
    #[cfg(feature = "server")]
    use super::super::requests::{
        AddBtcAddressRequest, AddEthAddressRequest, AddManualAssetAccountRequest, AddXpubRequest,
        GetAccountAddressesRequest, GetAccountTransactionsRequest, GetWalletByFingerprintRequest,
        MoveAccountRequest, MoveDestination, ValidateXpubRequest,
    };

    // Deterministic xpub fixtures — same seeds as xpub.rs tests
    fn test_account_xpub(account: u32) -> String {
        match account {
            0 => "xpub6C7dm6fpZENX4meEzE4DLTSb4nvYMPiZvJKMnbhGoDTfBMTMsY7eBxmaQq9RpSSKTdFyb5MoE1encwjP99mSHwjJf8JVoo572k9ireBAxyq".to_string(),
            2 => "xpub6CPCqKiAkerFSqn3dJSsfzeBX5ZTGS4dufSDVEPTFnhiHg2HgcaSY5T3uLR3Z2QCxzgaawVB3N2HH2cKoLccAi2rVuTEwNxt7LJfaiApAo6".to_string(),
            _ => panic!("no static fixture for account {account}; add one or use account 0 or 2"),
        }
    }

    #[test]
    fn test_validate_link_trezor_request_allows_same_account_index_across_schemes() {
        use super::super::requests::LinkTrezorRequest;
        let request = LinkTrezorRequest {
            master_fingerprint: RawMasterFingerprint::new("a1b2c3d4".to_string()),
            wallet_label: RawLabel::new("My Trezor".to_string()),
            device_id: None,
            device_label: None,
            accounts: vec![
                TrezorAccountLinkRequest {
                    account_index: RawAccountIndex::new(0),
                    address_scheme: AddressScheme::Legacy,
                    extended_pubkey: super::super::xpub::RawExtendedPubkey::new(test_account_xpub(
                        0,
                    )),
                },
                TrezorAccountLinkRequest {
                    account_index: RawAccountIndex::new(0),
                    address_scheme: AddressScheme::NativeSegwit,
                    extended_pubkey: super::super::xpub::RawExtendedPubkey::new(test_account_xpub(
                        0,
                    )),
                },
            ],
        };

        let validated = validate_link_trezor_request(request);
        assert!(validated.is_ok());
    }

    #[test]
    fn test_validate_link_trezor_request_rejects_duplicate_account_and_scheme() {
        use super::super::requests::LinkTrezorRequest;
        let request = LinkTrezorRequest {
            master_fingerprint: RawMasterFingerprint::new("a1b2c3d4".to_string()),
            wallet_label: RawLabel::new("My Trezor".to_string()),
            device_id: None,
            device_label: None,
            accounts: vec![
                TrezorAccountLinkRequest {
                    account_index: RawAccountIndex::new(0),
                    address_scheme: AddressScheme::NativeSegwit,
                    extended_pubkey: super::super::xpub::RawExtendedPubkey::new(test_account_xpub(
                        0,
                    )),
                },
                TrezorAccountLinkRequest {
                    account_index: RawAccountIndex::new(0),
                    address_scheme: AddressScheme::NativeSegwit,
                    extended_pubkey: super::super::xpub::RawExtendedPubkey::new(test_account_xpub(
                        0,
                    )),
                },
            ],
        };

        let errors = match validate_link_trezor_request(request) {
            Ok(_) => panic!("duplicate account+scheme should fail"),
            Err(value) => value,
        };

        let message = errors
            .first("accounts[1].account_index")
            .cloned()
            .unwrap_or_default();
        assert_eq!(message, "Duplicate account index for address scheme");
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_hash_device_id() {
        let first = hash_device_id("device-123");
        let second = hash_device_id("device-123");
        let different = hash_device_id("device-456");

        assert_eq!(first, second);
        assert_ne!(first, different);
        assert!(first.starts_with("sha256:"));
        assert_eq!(first.len(), "sha256:".len() + 64);
        assert!(
            first
                .chars()
                .skip("sha256:".len())
                .all(|c| c.is_ascii_hexdigit())
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_wallet_by_fingerprint_request_try_into_validated_accepts_valid_fingerprint() {
        let request = GetWalletByFingerprintRequest {
            master_fingerprint: RawMasterFingerprint::new("A1B2C3D4".to_string()),
        };

        let validated = request
            .try_into_validated()
            .expect("fingerprint should validate");
        assert_eq!(validated.master_fingerprint.as_str(), "a1b2c3d4");
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_wallet_by_fingerprint_request_try_into_validated_rejects_invalid_fingerprint() {
        let request = GetWalletByFingerprintRequest {
            master_fingerprint: RawMasterFingerprint::new("xyz".to_string()),
        };

        let errors = request
            .try_into_validated()
            .expect_err("invalid fingerprint should fail");
        assert!(errors.first("master_fingerprint").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_eth_address_request_try_into_validated_accepts_valid_input() {
        let request = AddEthAddressRequest {
            address: crate::ethereum::RawEthAddress::new(
                "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
            ),
            network: Network::Mainnet,
            wallet_id: None,
            wallet_label: Some(RawLabel::new("ETH watch".to_string())),
            account_label: None,
        };

        let validated = request
            .try_into_validated()
            .expect("valid ethereum request should validate");
        assert_eq!(validated.network, Network::Mainnet);
        assert!(validated.wallet_id.is_none());
        assert_eq!(
            validated.wallet_label.as_ref().map(Label::as_str),
            Some("ETH watch")
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_eth_address_request_try_into_validated_rejects_unsupported_network() {
        let request = AddEthAddressRequest {
            address: crate::ethereum::RawEthAddress::new(
                "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
            ),
            network: Network::Signet,
            wallet_id: None,
            wallet_label: None,
            account_label: None,
        };

        let errors = request
            .try_into_validated()
            .expect_err("unsupported network should fail");
        assert!(errors.first("network").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_eth_address_request_try_into_validated_requires_wallet_label_for_new_wallet() {
        let request = AddEthAddressRequest {
            address: crate::ethereum::RawEthAddress::new(
                "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
            ),
            network: Network::Mainnet,
            wallet_id: None,
            wallet_label: None,
            account_label: None,
        };

        let errors = request
            .try_into_validated()
            .expect_err("missing wallet label should fail");
        assert!(errors.first("wallet_label").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_eth_address_request_try_into_validated_allows_missing_wallet_label_for_existing_wallet()
     {
        let request = AddEthAddressRequest {
            address: crate::ethereum::RawEthAddress::new(
                "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
            ),
            network: Network::Mainnet,
            wallet_id: Some(WalletId::new()),
            wallet_label: None,
            account_label: None,
        };

        let validated = request
            .try_into_validated()
            .expect("existing wallet path should not require wallet label");
        assert!(validated.wallet_label.is_none());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_btc_address_request_try_into_validated_accepts_valid_input() {
        let request = AddBtcAddressRequest {
            address: RawBtcAddress::new("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string()),
            network: Network::Mainnet,
            wallet_id: None,
            wallet_label: Some(RawLabel::new("BTC watch".to_string())),
            account_label: None,
        };

        let validated = request
            .try_into_validated()
            .expect("valid bitcoin request should validate");
        assert_eq!(validated.network, Network::Mainnet);
        assert_eq!(
            validated.wallet_label.as_ref().map(Label::as_str),
            Some("BTC watch")
        );
        assert_eq!(validated.address.address_scheme(), AddressScheme::Legacy);
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_btc_address_request_try_into_validated_rejects_unsupported_network() {
        let request = AddBtcAddressRequest {
            address: RawBtcAddress::new("tb1qfm9w4x5ndec9zkta2h3l99u8j27fsl8s9mnn8h".to_string()),
            network: Network::Signet,
            wallet_id: None,
            wallet_label: None,
            account_label: None,
        };

        let errors = request
            .try_into_validated()
            .expect_err("unsupported network should fail");
        assert!(errors.first("network").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_btc_address_request_try_into_validated_requires_wallet_label_for_new_wallet() {
        let request = AddBtcAddressRequest {
            address: RawBtcAddress::new("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string()),
            network: Network::Mainnet,
            wallet_id: None,
            wallet_label: None,
            account_label: None,
        };

        let errors = request
            .try_into_validated()
            .expect_err("missing wallet label should fail");
        assert!(errors.first("wallet_label").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_validate_xpub_request_try_into_validated_rejects_invalid_prefix() {
        let request = ValidateXpubRequest {
            extended_pubkey: "tpub6ABC".to_string(),
        };

        let errors = request
            .try_into_validated()
            .expect_err("invalid xpub prefix should fail");
        assert!(errors.first("extended_pubkey").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_xpub_request_try_into_validated_rejects_unsupported_scheme() {
        let request = AddXpubRequest {
            extended_pubkey: test_account_xpub(0),
            address_scheme: AddressScheme::Taproot,
            wallet_id: None,
            wallet_label: None,
            account_label: None,
        };

        let errors = request
            .try_into_validated()
            .expect_err("unsupported scheme should fail");
        assert!(errors.first("address_scheme").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_xpub_request_try_into_validated_requires_wallet_label_for_new_wallet() {
        let request = AddXpubRequest {
            extended_pubkey: test_account_xpub(0),
            address_scheme: AddressScheme::NativeSegwit,
            wallet_id: None,
            wallet_label: None,
            account_label: None,
        };

        let errors = request
            .try_into_validated()
            .expect_err("missing wallet label should fail");
        assert!(errors.first("wallet_label").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_account_request_validates_manual_asset_instance() {
        use crate::asset_views::ManualAssetInstanceIdView;

        let request = AddManualAssetAccountRequest {
            wallet_id: None,
            wallet_label: Some(RawLabel::new(" Manual Wallet ".to_string())),
            asset: Some(AddManualAssetAccountAssetRequest::BitGarthCatalog {
                asset_instance_id: ManualAssetInstanceIdView {
                    asset_id: "cardano".to_string(),
                    network_id: "cardano-mainnet".to_string(),
                },
            }),
            asset_instance_id: None,
            account_label: None,
        };

        let validated = request
            .try_into_validated()
            .expect("request should validate");
        assert!(matches!(
            validated.asset,
            ValidatedAddManualAssetAccountAsset::BitGarthCatalog {
                candidate_id: crate::asset_capabilities::ManualAssetCatalogCandidateId::Unsynced(
                    ref asset_instance_id
                )
            } if asset_instance_id.asset_id.as_str() == "cardano"
                && asset_instance_id.network_id.as_str() == "cardano-mainnet"
        ));
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_account_request_validates_search_style_erc20_instance() {
        use crate::asset_views::ManualAssetInstanceIdView;

        let request = AddManualAssetAccountRequest {
            wallet_id: None,
            wallet_label: Some(RawLabel::new(" Manual Wallet ".to_string())),
            asset: Some(AddManualAssetAccountAssetRequest::BitGarthCatalog {
                asset_instance_id: ManualAssetInstanceIdView {
                    asset_id: "usd-coin".to_string(),
                    network_id: "ethereum-mainnet".to_string(),
                },
            }),
            asset_instance_id: None,
            account_label: None,
        };

        let validated = request
            .try_into_validated()
            .expect("search result id should validate");
        assert!(matches!(
            validated.asset,
            ValidatedAddManualAssetAccountAsset::BitGarthCatalog {
                candidate_id: crate::asset_capabilities::ManualAssetCatalogCandidateId::Unsynced(
                    ref asset_instance_id
                )
            } if asset_instance_id.asset_id.as_str() == "usd-coin"
                && asset_instance_id.network_id.as_str() == "ethereum-mainnet"
        ));
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_account_request_accepts_synced_catalog_instance() {
        use crate::asset_views::ManualAssetInstanceIdView;

        let request = AddManualAssetAccountRequest {
            wallet_id: Some(WalletId::new()),
            wallet_label: None,
            asset: Some(AddManualAssetAccountAssetRequest::BitGarthCatalog {
                asset_instance_id: ManualAssetInstanceIdView {
                    asset_id: "bitcoin".to_string(),
                    network_id: "bitcoin-mainnet".to_string(),
                },
            }),
            asset_instance_id: None,
            account_label: None,
        };

        let validated = request
            .try_into_validated()
            .expect("synced catalog asset should validate for manual account");
        assert!(matches!(
            validated.asset,
            ValidatedAddManualAssetAccountAsset::BitGarthCatalog {
                candidate_id: crate::asset_capabilities::ManualAssetCatalogCandidateId::Synced(
                    crate::asset_capabilities::SyncedAssetInstanceId::BtcBitcoinMainnet
                )
            }
        ));
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_account_request_rejects_unknown_instance() {
        use crate::asset_views::ManualAssetInstanceIdView;

        let request = AddManualAssetAccountRequest {
            wallet_id: Some(WalletId::new()),
            wallet_label: None,
            asset: Some(AddManualAssetAccountAssetRequest::BitGarthCatalog {
                asset_instance_id: ManualAssetInstanceIdView {
                    asset_id: "cardano".to_string(),
                    network_id: "ethereum-mainnet".to_string(),
                },
            }),
            asset_instance_id: None,
            account_label: None,
        };

        let AddManualAssetAccountValidationError::Fields(errors) =
            request.try_into_validated().expect_err("unknown pair")
        else {
            panic!("expected field validation error");
        };
        assert!(errors.first("asset_instance_id").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_account_request_validates_coingecko_snapshot() {
        let request = AddManualAssetAccountRequest {
            wallet_id: Some(WalletId::new()),
            wallet_label: None,
            asset: Some(AddManualAssetAccountAssetRequest::CoinGeckoDiscovery {
                snapshot: CoinGeckoManualAssetSnapshotRequest {
                    asset_id: "adappter-token".to_string(),
                    network_id: "ethereum-mainnet".to_string(),
                    decimal_precision: 6,
                    unit_code: "ADP".to_string(),
                    symbol: Some("adp".to_string()),
                    asset_name: "Adappter Token".to_string(),
                    network_name: "Ethereum".to_string(),
                    coingecko_id: "adappter-token".to_string(),
                    coingecko_platform_id: Some("ethereum".to_string()),
                    provider_platform_asset_ref: Some("0xabc".to_string()),
                    precision_source: CoinGeckoManualAssetPrecisionSourceRequest::CoingeckoPlatform,
                },
            }),
            asset_instance_id: None,
            account_label: None,
        };

        let validated = request
            .try_into_validated()
            .expect("snapshot should validate");
        let ValidatedAddManualAssetAccountAsset::CoinGeckoDiscovery { snapshot } = validated.asset
        else {
            panic!("expected coingecko branch");
        };

        assert_eq!(snapshot.asset_id.as_str(), "adappter-token");
        assert_eq!(snapshot.network_id.as_str(), "ethereum-mainnet");
        assert_eq!(snapshot.decimal_precision.as_u8(), 6);
        assert_eq!(snapshot.unit_code.as_str(), "ADP");
        assert_eq!(snapshot.coingecko_id.as_str(), "adappter-token");
        assert_eq!(snapshot.precision_source.as_db_str(), "coingecko_platform");
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_account_request_rejects_invalid_coingecko_snapshot() {
        let request = AddManualAssetAccountRequest {
            wallet_id: Some(WalletId::new()),
            wallet_label: None,
            asset: Some(AddManualAssetAccountAssetRequest::CoinGeckoDiscovery {
                snapshot: CoinGeckoManualAssetSnapshotRequest {
                    asset_id: "BAD ID".to_string(),
                    network_id: "ethereum-mainnet".to_string(),
                    decimal_precision: 19,
                    unit_code: "BAD-UNIT".to_string(),
                    symbol: None,
                    asset_name: " ".to_string(),
                    network_name: "Ethereum".to_string(),
                    coingecko_id: "bad id".to_string(),
                    coingecko_platform_id: None,
                    provider_platform_asset_ref: None,
                    precision_source: CoinGeckoManualAssetPrecisionSourceRequest::UserDefault,
                },
            }),
            asset_instance_id: None,
            account_label: None,
        };

        let AddManualAssetAccountValidationError::Fields(errors) = request
            .try_into_validated()
            .expect_err("invalid snapshot should fail")
        else {
            panic!("expected field validation error");
        };

        assert!(errors.first("asset_id").is_some());
        assert!(errors.first("decimal_precision").is_some());
        assert!(errors.first("unit_code").is_some());
        assert!(errors.first("asset_name").is_some());
        assert!(errors.first("coingecko_id").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_account_request_rejects_synced_coingecko_snapshot() {
        let request = AddManualAssetAccountRequest {
            wallet_id: Some(WalletId::new()),
            wallet_label: None,
            asset: Some(AddManualAssetAccountAssetRequest::CoinGeckoDiscovery {
                snapshot: CoinGeckoManualAssetSnapshotRequest {
                    asset_id: "bitcoin".to_string(),
                    network_id: "bitcoin-mainnet".to_string(),
                    decimal_precision: 8,
                    unit_code: "XBTC".to_string(),
                    symbol: Some("btc".to_string()),
                    asset_name: "Bitcoin".to_string(),
                    network_name: "Bitcoin".to_string(),
                    coingecko_id: "bitcoin".to_string(),
                    coingecko_platform_id: None,
                    provider_platform_asset_ref: None,
                    precision_source: CoinGeckoManualAssetPrecisionSourceRequest::UserDefault,
                },
            }),
            asset_instance_id: None,
            account_label: None,
        };

        let AddManualAssetAccountValidationError::Fields(errors) = request
            .try_into_validated()
            .expect_err("synced CoinGecko asset should fail")
        else {
            panic!("expected field validation error");
        };
        assert!(errors.first("coingecko_id").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_balance_assertion_request_accepts_zero_balance() {
        use super::super::labels::ManualAssetDisplayScale;
        #[cfg(feature = "server")]
        use crate::amounts::UnsignedAmount;
        use chrono::NaiveDate;

        let today = NaiveDate::from_ymd_opt(2026, 4, 2).expect("valid date");
        let request = AddManualAssetBalanceAssertionRequest {
            account_id: WalletAccountId::new(),
            asserted_on: ReportDateParam::from_naive_date(today),
            balance: RawManualAssetBalance::new("0".to_string()),
            note: Some(RawManualAssetAssertionNote::new(" sold ".to_string())),
        };

        let validated = request
            .try_into_validated_at(today, 18)
            .expect("zero-balance assertion should validate");

        assert_eq!(validated.asserted_on, today);
        assert_eq!(validated.balance.trimmed(), "0");
        assert_eq!(validated.balance.entered_fractional_digits(), 0);
        assert_eq!(
            validated
                .balance
                .parse_at_scale(ManualAssetDisplayScale::fixed())
                .expect("fixed-scale parse should succeed")
                .amount(),
            UnsignedAmount::zero()
        );
        assert_eq!(
            validated.note.as_ref().map(|note| note.as_str()),
            Some("sold")
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_balance_assertion_request_rejects_future_date_and_scale_overflow() {
        use chrono::NaiveDate;

        let today = NaiveDate::from_ymd_opt(2026, 4, 2).expect("valid date");
        let request = AddManualAssetBalanceAssertionRequest {
            account_id: WalletAccountId::new(),
            asserted_on: ReportDateParam::from_naive_date(
                NaiveDate::from_ymd_opt(2026, 4, 3).expect("valid future date"),
            ),
            balance: RawManualAssetBalance::new("1.1234567890123456789".to_string()),
            note: None,
        };

        let errors = request
            .try_into_validated_at(today, 18)
            .expect_err("future date and excessive scale should fail");
        assert!(errors.first("asserted_on").is_some());
        assert!(errors.first("balance").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_balance_assertion_request_accepts_more_than_eight_fractional_digits() {
        use chrono::NaiveDate;

        let today = NaiveDate::from_ymd_opt(2026, 4, 2).expect("valid date");
        let request = AddManualAssetBalanceAssertionRequest {
            account_id: WalletAccountId::new(),
            asserted_on: ReportDateParam::from_naive_date(today),
            balance: RawManualAssetBalance::new("1.123456789".to_string()),
            note: None,
        };

        let validated = request
            .try_into_validated_at(today, 18)
            .expect("nine-decimal assertion should validate at the literal boundary");

        assert_eq!(validated.balance.trimmed(), "1.123456789");
        assert_eq!(validated.balance.entered_fractional_digits(), 9);
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_assertion_rejects_precision_exceeding_decimal_precision() {
        use chrono::NaiveDate;

        let today = NaiveDate::from_ymd_opt(2026, 4, 2).unwrap();
        let request = AddManualAssetBalanceAssertionRequest {
            account_id: WalletAccountId::new(),
            asserted_on: ReportDateParam::from_naive_date(today),
            balance: RawManualAssetBalance::new("1.123456789".to_string()), // 9 fractional digits
            note: None,
        };

        let result = request.try_into_validated_at(today, 8);
        assert!(
            result.is_err(),
            "should reject 9 digits when decimal_precision is 8"
        );
        let errors = result.unwrap_err();
        assert!(errors.first("balance").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_add_manual_asset_assertion_accepts_precision_at_decimal_precision() {
        use chrono::NaiveDate;

        let today = NaiveDate::from_ymd_opt(2026, 4, 2).unwrap();
        let request = AddManualAssetBalanceAssertionRequest {
            account_id: WalletAccountId::new(),
            asserted_on: ReportDateParam::from_naive_date(today),
            balance: RawManualAssetBalance::new("1.12345678".to_string()), // 8 fractional digits
            note: None,
        };

        let validated = request
            .try_into_validated_at(today, 8)
            .expect("8 digits should be accepted when decimal_precision is 8");
        assert_eq!(validated.balance.entered_fractional_digits(), 8);
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_move_account_request_try_into_validated_accepts_existing_wallet_destination() {
        let request = MoveAccountRequest {
            account_id: WalletAccountId::new(),
            destination: MoveDestination::ExistingWallet {
                wallet_id: WalletId::new(),
            },
        };

        let validated = request
            .try_into_validated()
            .expect("existing wallet destination should validate");

        assert!(matches!(
            validated.destination,
            ValidatedMoveDestination::ExistingWallet { .. }
        ));
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_move_account_request_try_into_validated_rejects_empty_new_wallet_label() {
        let request = MoveAccountRequest {
            account_id: WalletAccountId::new(),
            destination: MoveDestination::NewWallet {
                label: RawLabel::new("   ".to_string()),
            },
        };

        let errors = request
            .try_into_validated()
            .expect_err("empty new wallet label should fail");
        assert!(errors.first("destination.label").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_account_addresses_request_try_into_validated_defaults_and_clamps_page_size() {
        let request = GetAccountAddressesRequest {
            account_id: DigitalAssetAccountId::new(),
            address_scheme: AddressScheme::NativeSegwit,
            page: None,
            page_size: Some(MAX_ACCOUNT_ADDRESSES_PAGE_SIZE + 1),
        };

        let validated = request
            .try_into_validated()
            .expect("request should validate");

        assert_eq!(validated.page, 1);
        assert_eq!(validated.page_size, MAX_ACCOUNT_ADDRESSES_PAGE_SIZE);
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_account_addresses_request_try_into_validated_rejects_zero_values() {
        let request = GetAccountAddressesRequest {
            account_id: DigitalAssetAccountId::new(),
            address_scheme: AddressScheme::NativeSegwit,
            page: Some(0),
            page_size: Some(0),
        };

        let errors = request
            .try_into_validated()
            .expect_err("zero page and page_size should fail");

        assert!(errors.first("page").is_some());
        assert!(errors.first("page_size").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_account_transactions_request_try_into_validated_defaults_pages() {
        let request = GetAccountTransactionsRequest {
            account_id: WalletAccountId::new(),
            pending_page: None,
            confirmed_page: None,
            sort: None,
            filters: None,
        };

        let validated = request
            .try_into_validated()
            .expect("missing pages should default");

        assert_eq!(validated.pending_page, 1);
        assert_eq!(validated.confirmed_page, 1);
        assert_eq!(validated.sort, TransactionSortDirection::Descending);
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_account_transactions_request_try_into_validated_rejects_zero_pages() {
        let request = GetAccountTransactionsRequest {
            account_id: WalletAccountId::new(),
            pending_page: Some(0),
            confirmed_page: Some(0),
            sort: None,
            filters: None,
        };

        let errors = request
            .try_into_validated()
            .expect_err("zero pages should fail");

        assert!(errors.first("pending_page").is_some());
        assert!(errors.first("confirmed_page").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_account_transactions_request_sort_direction_parsing() {
        let asc = GetAccountTransactionsRequest {
            account_id: WalletAccountId::new(),
            pending_page: None,
            confirmed_page: None,
            sort: Some("asc".to_string()),
            filters: None,
        }
        .try_into_validated()
        .expect("valid request");
        assert_eq!(asc.sort, TransactionSortDirection::Ascending);

        let desc = GetAccountTransactionsRequest {
            account_id: WalletAccountId::new(),
            pending_page: None,
            confirmed_page: None,
            sort: Some("desc".to_string()),
            filters: None,
        }
        .try_into_validated()
        .expect("valid request");
        assert_eq!(desc.sort, TransactionSortDirection::Descending);
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_account_transactions_request_empty_filters_produces_no_filter() {
        let request = GetAccountTransactionsRequest {
            account_id: WalletAccountId::new(),
            pending_page: None,
            confirmed_page: None,
            sort: None,
            filters: None,
        }
        .try_into_validated()
        .expect("valid request");

        assert!(request.filters.status.is_empty());
        assert!(request.filters.from_date.is_none());
        assert!(request.filters.to_date.is_none());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_account_transactions_request_parses_status_filter() {
        let request = GetAccountTransactionsRequest {
            account_id: WalletAccountId::new(),
            pending_page: None,
            confirmed_page: None,
            sort: None,
            filters: Some(r#"{"status":["confirmed","failed"]}"#.to_string()),
        }
        .try_into_validated()
        .expect("valid request");

        assert_eq!(request.filters.status.len(), 2);
        assert_eq!(
            request.filters.status[0],
            crate::transactions::ChainTransactionStatus::Confirmed
        );
        assert_eq!(
            request.filters.status[1],
            crate::transactions::ChainTransactionStatus::Failed
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_account_transactions_request_parses_date_filters() {
        let request = GetAccountTransactionsRequest {
            account_id: WalletAccountId::new(),
            pending_page: None,
            confirmed_page: None,
            sort: None,
            filters: Some(
                r#"{"from_date":"2026-01-01T00:00:00Z","to_date":"2026-12-31T23:59:59Z"}"#
                    .to_string(),
            ),
        }
        .try_into_validated()
        .expect("valid request");

        assert!(request.filters.from_date.is_some());
        assert!(request.filters.to_date.is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_account_transactions_request_rejects_invalid_status() {
        let result = GetAccountTransactionsRequest {
            account_id: WalletAccountId::new(),
            pending_page: None,
            confirmed_page: None,
            sort: None,
            filters: Some(r#"{"status":["bogus"]}"#.to_string()),
        }
        .try_into_validated();

        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.first("filters.status").is_some());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_get_account_transactions_request_rejects_invalid_date() {
        let result = GetAccountTransactionsRequest {
            account_id: WalletAccountId::new(),
            pending_page: None,
            confirmed_page: None,
            sort: None,
            filters: Some(r#"{"from_date":"not-a-date"}"#.to_string()),
        }
        .try_into_validated();

        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.first("filters.from_date").is_some());
    }
}
