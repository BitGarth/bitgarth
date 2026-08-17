use super::super::raw_ingestion::ensure_source_connection_for_address_tx;
use super::parse::{
    ParsedImportedAddress, ParsedImportedBalanceAssertion, ParsedImportedHdKey,
    ParsedImportedManualAccount, ParsedImportedManualAssetSnapshot, ParsedImportedNativeAccount,
    ParsedImportedWallet,
};
use crate::amounts::AmountSplitParts;
use crate::amounts::UnsignedAmount;

fn split_unsigned_amount(
    amount: UnsignedAmount,
    field_name: &'static str,
) -> Result<AmountSplitParts, WalletDataImportDbError> {
    super::super::amount_storage::split_unsigned_amount(amount).map_err(|err| {
        WalletDataImportDbError::Internal(format!("Failed to split {field_name}: {err}"))
    })
}
use super::resolve::{
    AddressLookupKey, ExistingNativeAccountMeta, ExistingWalletMeta, HdKeyLookupKey, ImportState,
    ManualAccountLookupKey, resolve_native_account_candidates,
};
use super::{
    ImportDuplicateSkipView, ImportGlobalDuplicateSkipView, WalletDataImportDbError,
    WalletDataImportResult,
};
use crate::wallets::{
    AddressSourceType, DigitalAssetAddressId, HdKeyId, IdentitySource, KeySource, Label,
    ManualAssetBalanceAssertionId, ManualAssetDisplayScale, ValidatedManualAssetAssertionNote,
    WalletAccountId, WalletId,
};
use chrono::{DateTime, Utc};
use rusqlite::params;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ImportCreationPlan {
    pub(super) supported_accounts_to_create: usize,
}

pub(super) fn plan_import_creations(
    initial_state: &ImportState,
    imported_wallets: &[ParsedImportedWallet],
) -> Result<ImportCreationPlan, WalletDataImportDbError> {
    let mut state = initial_state.clone();
    let mut supported_accounts_to_create = 0usize;

    for imported_wallet in imported_wallets {
        let wallet_id = plan_wallet_id(&mut state, imported_wallet)?;

        for native_account in &imported_wallet.native_accounts {
            if resolve_native_account_candidates(native_account, &state)?.is_empty() {
                supported_accounts_to_create = supported_accounts_to_create.saturating_add(1);
                plan_created_native_account(&mut state, wallet_id, native_account)?;
            }
        }

        for manual_account in &imported_wallet.manual_accounts {
            if plan_created_manual_account(&mut state, wallet_id, manual_account)? {
                supported_accounts_to_create = supported_accounts_to_create.saturating_add(1);
            }
        }
    }

    Ok(ImportCreationPlan {
        supported_accounts_to_create,
    })
}

fn plan_wallet_id(
    state: &mut ImportState,
    imported_wallet: &ParsedImportedWallet,
) -> Result<WalletId, WalletDataImportDbError> {
    if let Some(fingerprint) = imported_wallet.master_fingerprint.as_ref()
        && let Some(wallet_id) = state
            .wallet_by_fingerprint
            .get(fingerprint.as_str())
            .copied()
    {
        return Ok(wallet_id);
    }

    let mut affinity_wallet_ids = HashSet::<WalletId>::new();
    for native_account in &imported_wallet.native_accounts {
        for account_id in resolve_native_account_candidates(native_account, state)? {
            let wallet_id = state
                .native_account_meta
                .get(&account_id)
                .ok_or_else(|| {
                    WalletDataImportDbError::Internal(format!(
                        "Native account candidate {} missing metadata while planning import",
                        account_id
                    ))
                })?
                .wallet_id;
            affinity_wallet_ids.insert(wallet_id);
        }
    }

    if affinity_wallet_ids.len() > 1 {
        return Err(WalletDataImportDbError::Validation(format!(
            "Import wallet '{}' has identifiers linked to multiple existing wallets",
            imported_wallet.label.as_str()
        )));
    }

    if let Some(wallet_id) = affinity_wallet_ids.iter().next().copied() {
        return Ok(wallet_id);
    }

    let wallet_id = WalletId::new();
    state.wallet_meta.insert(
        wallet_id,
        ExistingWalletMeta {
            label: imported_wallet.label.clone(),
        },
    );
    state.wallet_label_keys.insert(imported_wallet.label.key());
    state
        .wallet_account_label_keys
        .entry(wallet_id)
        .or_default();
    if let Some(fingerprint) = imported_wallet.master_fingerprint.as_ref() {
        state
            .wallet_by_fingerprint
            .insert(fingerprint.as_str().to_string(), wallet_id);
    }
    Ok(wallet_id)
}

fn plan_created_native_account(
    state: &mut ImportState,
    wallet_id: WalletId,
    account: &ParsedImportedNativeAccount,
) -> Result<(), WalletDataImportDbError> {
    let account_id = WalletAccountId::new();
    let wallet_label = state
        .wallet_meta
        .get(&wallet_id)
        .ok_or_else(|| {
            WalletDataImportDbError::Internal(format!(
                "Missing metadata for wallet {} while planning account creation",
                wallet_id
            ))
        })?
        .label
        .clone();
    state.native_account_meta.insert(
        account_id,
        ExistingNativeAccountMeta {
            wallet_id,
            wallet_label,
            account_label: account.label.clone(),
        },
    );
    state
        .wallet_account_label_keys
        .entry(wallet_id)
        .or_default()
        .insert(account.label.key());

    for hd_key in &account.hd_keys {
        state.hd_key_lookup.insert(
            HdKeyLookupKey {
                normalized_extended_pubkey: hd_key.value.normalized_as_str().to_string(),
                address_scheme: hd_key.address_scheme,
            },
            account_id,
        );
    }
    for address in &account.addresses {
        state.address_lookup.insert(
            AddressLookupKey {
                asset_id: address.asset_id.as_str().to_string(),
                network: address.network.as_str().to_string(),
                address_normalized: address.normalized_address.clone(),
            },
            account_id,
        );
    }

    Ok(())
}

fn plan_created_manual_account(
    state: &mut ImportState,
    wallet_id: WalletId,
    account: &ParsedImportedManualAccount,
) -> Result<bool, WalletDataImportDbError> {
    let lookup_key = ManualAccountLookupKey {
        wallet_id,
        asset_id: account.snapshot.asset_id.clone(),
        network_id: account.snapshot.network_id.clone(),
    };
    if state.manual_account_lookup.contains_key(&lookup_key) {
        return Ok(false);
    }

    let account_id = WalletAccountId::new();
    state.manual_account_lookup.insert(lookup_key, account_id);
    state
        .wallet_account_label_keys
        .entry(wallet_id)
        .or_default()
        .insert(account.label.key());
    Ok(true)
}

pub(super) fn fallback_import_created_at(
    import_started_at: DateTime<Utc>,
    sequence: usize,
) -> DateTime<Utc> {
    let micros = i64::try_from(sequence).unwrap_or(i64::MAX);
    import_started_at
        .checked_add_signed(chrono::Duration::microseconds(micros))
        .unwrap_or(import_started_at)
}

pub(super) fn insert_wallet_in_tx(
    tx: &rusqlite::Transaction<'_>,
    label: &Label,
    fingerprint: Option<&crate::wallets::ValidatedMasterFingerprint>,
    now: DateTime<Utc>,
) -> Result<WalletId, WalletDataImportDbError> {
    let wallet_id = WalletId::new();
    tx.execute(
        "INSERT INTO wallets
         (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            wallet_id.to_string(),
            label.as_str(),
            label.key().as_str(),
            fingerprint.map(crate::wallets::ValidatedMasterFingerprint::as_str),
            IdentitySource::UserProvided.as_str(),
            Option::<String>::None,
            now.to_rfc3339(),
            now.to_rfc3339(),
        ],
    )
    .map_err(|err| {
        WalletDataImportDbError::Internal(format!(
            "Failed to insert wallet '{}' during import: {err}",
            label.as_str()
        ))
    })?;

    Ok(wallet_id)
}

pub(super) fn insert_native_account_in_tx(
    tx: &rusqlite::Transaction<'_>,
    wallet_id: WalletId,
    account_label: &Label,
    account: &ParsedImportedNativeAccount,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> Result<WalletAccountId, WalletDataImportDbError> {
    let account_id = WalletAccountId::new();

    tx.execute(
        "INSERT INTO digital_asset_accounts
         (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            account_id.to_string(),
            wallet_id.to_string(),
            account_label.as_str(),
            account_label.key().as_str(),
            account.asset_id.as_str(),
            account.network.as_str(),
            account.account_kind.as_str(),
            created_at.to_rfc3339(),
            updated_at.to_rfc3339(),
        ],
    )
    .map_err(|err| {
        WalletDataImportDbError::Internal(format!(
            "Failed to insert native account '{}' during import: {err}",
            account_label.as_str()
        ))
    })?;

    Ok(account_id)
}

pub(super) fn insert_hd_key_in_tx(
    tx: &rusqlite::Transaction<'_>,
    account_id: WalletAccountId,
    hd_key: &ParsedImportedHdKey,
    now: DateTime<Utc>,
) -> Result<(), WalletDataImportDbError> {
    tx.execute(
        "INSERT INTO digital_asset_account_hd_keys
         (id, account_id, key_role, extended_pubkey, normalized_extended_pubkey, derivation_purpose, derivation_coin_type, derivation_account, address_scheme, key_source, verified_by_accessor_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            HdKeyId::new().to_string(),
            account_id.to_string(),
            hd_key.key_role.as_str(),
            hd_key.value.as_str(),
            hd_key.value.normalized_as_str(),
            i64::from(hd_key.derivation_purpose.value()),
            i64::from(hd_key.derivation_coin_type.value()),
            i64::from(hd_key.derivation_account.as_u32()),
            hd_key.address_scheme.as_str(),
            KeySource::UserProvided.as_str(),
            Option::<String>::None,
            now.to_rfc3339(),
            now.to_rfc3339(),
        ],
    )
    .map_err(|err| {
        WalletDataImportDbError::Internal(format!(
            "Failed to insert HD key '{}' during import: {err}",
            hd_key.display_identifier
        ))
    })?;

    Ok(())
}

pub(super) fn insert_address_in_tx(
    tx: &rusqlite::Transaction<'_>,
    account_id: WalletAccountId,
    address: &ParsedImportedAddress,
    now: DateTime<Utc>,
) -> Result<(), WalletDataImportDbError> {
    let address_id = DigitalAssetAddressId::new();

    tx.execute(
        "INSERT INTO digital_asset_addresses
         (id, account_id, asset_id, network, address, address_normalized, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            address_id.to_string(),
            account_id.to_string(),
            address.asset_id.as_str(),
            address.network.as_str(),
            address.canonical_address,
            address.normalized_address,
            address.address_scheme.as_str(),
            Option::<i64>::None,
            Option::<i64>::None,
            AddressSourceType::Imported.as_str(),
            now.to_rfc3339(),
            now.to_rfc3339(),
        ],
    )
    .map_err(|err| {
        WalletDataImportDbError::Internal(format!(
            "Failed to insert address '{}' during import: {err}",
            address.display_identifier
        ))
    })?;

    ensure_source_connection_for_address_tx(
        tx,
        address_id,
        address.asset_id,
        address.network,
        &address.normalized_address,
        now,
    )
    .map_err(WalletDataImportDbError::from)?;

    Ok(())
}

pub(super) fn insert_manual_asset_account_in_tx(
    tx: &rusqlite::Transaction<'_>,
    wallet_id: WalletId,
    label: &Label,
    snapshot: &ParsedImportedManualAssetSnapshot,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> Result<WalletAccountId, WalletDataImportDbError> {
    let account_id = WalletAccountId::new();
    tx.execute(
        "INSERT INTO manual_asset_accounts
         (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
          unit_code, symbol, asset_name, network_name, coingecko_id, asset_source,
          precision_source, coingecko_platform_id, provider_platform_asset_ref,
          created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            account_id.to_string(),
            wallet_id.to_string(),
            label.as_str(),
            label.key().as_str(),
            &snapshot.asset_id,
            &snapshot.network_id,
            i64::from(snapshot.decimal_precision.as_u8()),
            snapshot.unit_code.as_str(),
            snapshot.symbol.as_deref(),
            &snapshot.asset_name,
            &snapshot.network_name,
            &snapshot.coingecko_id,
            &snapshot.asset_source,
            &snapshot.precision_source,
            snapshot.coingecko_platform_id.as_deref(),
            snapshot.provider_platform_asset_ref.as_deref(),
            created_at.to_rfc3339(),
            updated_at.to_rfc3339(),
        ],
    )
    .map_err(|err| {
        WalletDataImportDbError::Internal(format!(
            "Failed to insert manual asset account '{}' during import: {err}",
            label.as_str()
        ))
    })?;
    Ok(account_id)
}

pub(super) fn insert_manual_asset_assertion_in_tx(
    tx: &rusqlite::Transaction<'_>,
    account_id: WalletAccountId,
    assertion: &ParsedImportedBalanceAssertion,
    target_scale: ManualAssetDisplayScale,
    now: DateTime<Utc>,
) -> Result<(), WalletDataImportDbError> {
    let parsed_balance = assertion
        .balance
        .parse_at_scale(target_scale)
        .map_err(|err| {
            WalletDataImportDbError::Validation(format!(
                "Failed to normalize manual assertion '{}' at scale {}: {err}",
                assertion.balance.trimmed(),
                target_scale.as_u8(),
            ))
        })?;
    let parts = split_unsigned_amount(parsed_balance.amount(), "manual assertion")?;

    tx.execute(
        "INSERT INTO manual_asset_balance_assertions
         (id, account_id, asserted_on, balance_amount_hi, balance_amount_lo, note, entered_balance_text, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            ManualAssetBalanceAssertionId::new().to_string(),
            account_id.to_string(),
            assertion.asserted_on.format("%Y-%m-%d").to_string(),
            parts.hi,
            parts.lo,
            assertion.note.as_ref().map(ValidatedManualAssetAssertionNote::as_str),
            assertion.balance.trimmed(),
            now.to_rfc3339(),
            now.to_rfc3339(),
        ],
    )
    .map_err(|err| {
        WalletDataImportDbError::Internal(format!(
            "Failed to insert manual assertion for account {}: {err}",
            account_id
        ))
    })?;

    Ok(())
}

pub(super) fn push_duplicate_skip(
    result: &mut WalletDataImportResult,
    identifier_kind: &str,
    identifier: &str,
    wallet_label: &Label,
    account_label: &Label,
) {
    result.duplicate_skips.push(ImportDuplicateSkipView {
        identifier_kind: identifier_kind.to_string(),
        identifier: identifier.to_string(),
        wallet_label: wallet_label.as_str().to_string(),
        account_label: account_label.as_str().to_string(),
    });
}

pub(super) fn push_global_duplicate_skip(
    result: &mut WalletDataImportResult,
    identifier_kind: &str,
    identifier: &str,
    wallet_label: &Label,
    account_label: &Label,
) {
    result
        .global_duplicate_skips
        .push(ImportGlobalDuplicateSkipView {
            identifier_kind: identifier_kind.to_string(),
            identifier: identifier.to_string(),
            existing_wallet_label: wallet_label.as_str().to_string(),
            existing_account_label: account_label.as_str().to_string(),
        });
}

pub(super) fn merge_native_account_identifiers(
    tx: &rusqlite::Transaction<'_>,
    state: &mut ImportState,
    target_account_id: WalletAccountId,
    account: &ParsedImportedNativeAccount,
    now: DateTime<Utc>,
    result: &mut WalletDataImportResult,
) -> Result<(), WalletDataImportDbError> {
    for hd_key in &account.hd_keys {
        let key = HdKeyLookupKey {
            normalized_extended_pubkey: hd_key.value.normalized_as_str().to_string(),
            address_scheme: hd_key.address_scheme,
        };

        match state.hd_key_lookup.get(&key).copied() {
            Some(existing_account_id) if existing_account_id == target_account_id => {
                let (wallet_label, account_label) =
                    state.account_meta_for_reporting(target_account_id)?;
                push_duplicate_skip(
                    result,
                    "hd_key",
                    &hd_key.display_identifier,
                    &wallet_label,
                    &account_label,
                );
            }
            Some(existing_account_id) => {
                let (wallet_label, account_label) =
                    state.account_meta_for_reporting(existing_account_id)?;
                push_global_duplicate_skip(
                    result,
                    "hd_key",
                    &hd_key.display_identifier,
                    &wallet_label,
                    &account_label,
                );
            }
            None => {
                insert_hd_key_in_tx(tx, target_account_id, hd_key, now)?;
                state.hd_key_lookup.insert(key, target_account_id);
            }
        }
    }

    for address in &account.addresses {
        let key = AddressLookupKey {
            asset_id: address.asset_id.as_str().to_string(),
            network: address.network.as_str().to_string(),
            address_normalized: address.normalized_address.clone(),
        };

        match state.address_lookup.get(&key).copied() {
            Some(existing_account_id) if existing_account_id == target_account_id => {
                let (wallet_label, account_label) =
                    state.account_meta_for_reporting(target_account_id)?;
                push_duplicate_skip(
                    result,
                    "address",
                    &address.display_identifier,
                    &wallet_label,
                    &account_label,
                );
            }
            Some(existing_account_id) => {
                let (wallet_label, account_label) =
                    state.account_meta_for_reporting(existing_account_id)?;
                push_global_duplicate_skip(
                    result,
                    "address",
                    &address.display_identifier,
                    &wallet_label,
                    &account_label,
                );
            }
            None => {
                insert_address_in_tx(tx, target_account_id, address, now)?;
                state.address_lookup.insert(key, target_account_id);
            }
        }
    }

    Ok(())
}
