use super::super::wallet_accounts::query_wallet_account_label_keys_in_tx;
use super::merge::{
    insert_manual_asset_account_in_tx, insert_native_account_in_tx, insert_wallet_in_tx,
};
use super::parse::{
    ParsedImportedManualAccount, ParsedImportedNativeAccount, ParsedImportedWallet,
    unique_label_with_numeric_suffix,
};
use super::{ImportNativeAccountView, WalletDataImportDbError, WalletDataImportResult};
use crate::wallets::{
    ACCOUNT_LABEL_MAX_LENGTH, AccountKind, AddressScheme, Label, LabelKey, Network, SyncedAssetId,
    ValidatedMasterFingerprint, WALLET_LABEL_MAX_LENGTH, WalletAccountId, WalletId,
};
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExistingWalletMeta {
    pub(super) label: Label,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExistingNativeAccountMeta {
    pub(super) wallet_id: WalletId,
    pub(super) wallet_label: Label,
    pub(super) account_label: Label,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct HdKeyLookupKey {
    pub(super) normalized_extended_pubkey: String,
    pub(super) address_scheme: AddressScheme,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct AddressLookupKey {
    pub(super) asset_id: String,
    pub(super) network: String,
    pub(super) address_normalized: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ManualAccountLookupKey {
    pub(super) wallet_id: WalletId,
    pub(super) asset_id: String,
    pub(super) network_id: String,
}

#[derive(Debug, Clone)]
pub(super) struct ImportState {
    pub(super) wallet_by_fingerprint: HashMap<String, WalletId>,
    pub(super) wallet_meta: HashMap<WalletId, ExistingWalletMeta>,
    pub(super) wallet_label_keys: HashSet<LabelKey>,
    pub(super) wallet_account_label_keys: HashMap<WalletId, HashSet<LabelKey>>,
    pub(super) native_account_meta: HashMap<WalletAccountId, ExistingNativeAccountMeta>,
    pub(super) hd_key_lookup: HashMap<HdKeyLookupKey, WalletAccountId>,
    pub(super) address_lookup: HashMap<AddressLookupKey, WalletAccountId>,
    pub(super) manual_account_lookup: HashMap<ManualAccountLookupKey, WalletAccountId>,
    pub(super) manual_asset_assertion_dates: HashMap<WalletAccountId, HashSet<NaiveDate>>,
}

impl ImportState {
    pub(super) fn account_meta_for_reporting(
        &self,
        account_id: WalletAccountId,
    ) -> Result<(Label, Label), WalletDataImportDbError> {
        if let Some(meta) = self.native_account_meta.get(&account_id) {
            return Ok((meta.wallet_label.clone(), meta.account_label.clone()));
        }
        Err(WalletDataImportDbError::Internal(format!(
            "Missing account metadata for {} during import",
            account_id
        )))
    }
}

fn parse_wallet_id(value: &str, field_name: &str) -> Result<WalletId, WalletDataImportDbError> {
    WalletId::from_str(value).map_err(|err| {
        WalletDataImportDbError::Internal(format!("Invalid {field_name} in DB: {err}"))
    })
}

fn parse_wallet_account_id(
    value: &str,
    field_name: &str,
) -> Result<WalletAccountId, WalletDataImportDbError> {
    WalletAccountId::from_str(value).map_err(|err| {
        WalletDataImportDbError::Internal(format!("Invalid {field_name} in DB: {err}"))
    })
}

fn parse_date(value: &str, field_name: &str) -> Result<NaiveDate, WalletDataImportDbError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|err| {
        WalletDataImportDbError::Internal(format!("Invalid {field_name} in DB: {err}"))
    })
}

fn parse_db_label(
    raw: &str,
    max_len: usize,
    field_name: &str,
) -> Result<Label, WalletDataImportDbError> {
    Label::parse_with_limit(raw, max_len).map_err(|err| {
        WalletDataImportDbError::Internal(format!("Invalid {field_name} in DB: {err}"))
    })
}

fn parse_db_asset_id(raw: &str) -> Result<SyncedAssetId, WalletDataImportDbError> {
    SyncedAssetId::from_str(raw)
        .ok_or_else(|| WalletDataImportDbError::Internal(format!("Invalid asset_id in DB: {raw}")))
}

fn parse_db_network(raw: &str) -> Result<Network, WalletDataImportDbError> {
    Network::from_str(raw)
        .ok_or_else(|| WalletDataImportDbError::Internal(format!("Invalid network in DB: {raw}")))
}

fn parse_db_address_scheme(raw: &str) -> Result<AddressScheme, WalletDataImportDbError> {
    AddressScheme::from_str(raw).ok_or_else(|| {
        WalletDataImportDbError::Internal(format!("Invalid address_scheme in DB: {raw}"))
    })
}

fn parse_db_master_fingerprint(
    raw: &str,
) -> Result<ValidatedMasterFingerprint, WalletDataImportDbError> {
    ValidatedMasterFingerprint::parse(raw).map_err(|err| {
        WalletDataImportDbError::Internal(format!("Invalid master_fingerprint in DB: {err}"))
    })
}

pub(super) fn load_import_state(
    tx: &rusqlite::Transaction<'_>,
) -> Result<ImportState, WalletDataImportDbError> {
    let mut wallet_meta = HashMap::<WalletId, ExistingWalletMeta>::new();
    let mut wallet_label_keys = HashSet::<LabelKey>::new();
    let mut wallet_by_fingerprint = HashMap::<String, WalletId>::new();

    {
        let mut stmt = tx
            .prepare(
                "SELECT id, label, label_key, master_fingerprint
                 FROM wallets
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to prepare wallet import metadata query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to execute wallet import metadata query: {err}"
                ))
            })?;

        for row in rows {
            let (wallet_id_raw, label_raw, label_key_raw, fingerprint_raw) =
                row.map_err(|err| {
                    WalletDataImportDbError::Internal(format!(
                        "Failed to map wallet import metadata row: {err}"
                    ))
                })?;

            let wallet_id = parse_wallet_id(&wallet_id_raw, "wallet id")?;
            let label = parse_db_label(&label_raw, WALLET_LABEL_MAX_LENGTH, "wallet label")?;
            let label_key = LabelKey::new(label_key_raw);

            if let Some(fingerprint_raw) = fingerprint_raw {
                let fingerprint = parse_db_master_fingerprint(&fingerprint_raw)?;
                wallet_by_fingerprint.insert(fingerprint.as_str().to_string(), wallet_id);
            }

            wallet_label_keys.insert(label_key);
            wallet_meta.insert(wallet_id, ExistingWalletMeta { label });
        }
    }

    let mut wallet_account_label_keys = HashMap::<WalletId, HashSet<LabelKey>>::new();
    for wallet_id in wallet_meta.keys().copied() {
        let rows = query_wallet_account_label_keys_in_tx(tx, wallet_id)?;
        let keys = rows
            .into_iter()
            .map(|row| row.label_key)
            .collect::<HashSet<_>>();
        wallet_account_label_keys.insert(wallet_id, keys);
    }

    let mut native_account_meta = HashMap::<WalletAccountId, ExistingNativeAccountMeta>::new();
    {
        let mut stmt = tx
            .prepare(
                "SELECT a.id, a.wallet_id, a.label
                 FROM digital_asset_accounts a
                 ORDER BY a.created_at ASC, a.id ASC",
            )
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to prepare native account metadata query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to execute native account metadata query: {err}"
                ))
            })?;

        for row in rows {
            let (account_id_raw, wallet_id_raw, account_label_raw) = row.map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to map native account metadata row: {err}"
                ))
            })?;

            let account_id = parse_wallet_account_id(&account_id_raw, "native account id")?;
            let wallet_id = parse_wallet_id(&wallet_id_raw, "native account wallet_id")?;
            let account_label = parse_db_label(
                &account_label_raw,
                ACCOUNT_LABEL_MAX_LENGTH,
                "account label",
            )?;
            let wallet_label = wallet_meta
                .get(&wallet_id)
                .ok_or_else(|| {
                    WalletDataImportDbError::Internal(format!(
                        "Native account references missing wallet {}",
                        wallet_id
                    ))
                })?
                .label
                .clone();

            native_account_meta.insert(
                account_id,
                ExistingNativeAccountMeta {
                    wallet_id,
                    wallet_label,
                    account_label,
                },
            );
        }
    }

    let mut hd_key_lookup = HashMap::<HdKeyLookupKey, WalletAccountId>::new();
    {
        let mut stmt = tx
            .prepare(
                "SELECT account_id, normalized_extended_pubkey, address_scheme
                 FROM digital_asset_account_hd_keys",
            )
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to prepare HD key lookup query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to execute HD key lookup query: {err}"
                ))
            })?;

        for row in rows {
            let (account_id_raw, normalized_xpub, address_scheme_raw) = row.map_err(|err| {
                WalletDataImportDbError::Internal(format!("Failed to map HD key lookup row: {err}"))
            })?;
            let account_id = parse_wallet_account_id(&account_id_raw, "hd account id")?;
            let address_scheme = parse_db_address_scheme(&address_scheme_raw)?;
            hd_key_lookup.insert(
                HdKeyLookupKey {
                    normalized_extended_pubkey: normalized_xpub,
                    address_scheme,
                },
                account_id,
            );
        }
    }

    let mut address_lookup = HashMap::<AddressLookupKey, WalletAccountId>::new();
    {
        let mut stmt = tx
            .prepare(
                "SELECT account_id, asset_id, network, address_normalized
                 FROM digital_asset_addresses",
            )
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to prepare address lookup query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to execute address lookup query: {err}"
                ))
            })?;

        for row in rows {
            let (account_id_raw, asset_id_raw, network_raw, address_normalized) =
                row.map_err(|err| {
                    WalletDataImportDbError::Internal(format!(
                        "Failed to map address lookup row: {err}"
                    ))
                })?;
            let account_id = parse_wallet_account_id(&account_id_raw, "address account id")?;
            let asset_id = parse_db_asset_id(&asset_id_raw)?;
            let network = parse_db_network(&network_raw)?;

            address_lookup.insert(
                AddressLookupKey {
                    asset_id: asset_id.as_str().to_string(),
                    network: network.as_str().to_string(),
                    address_normalized,
                },
                account_id,
            );
        }
    }

    let mut manual_account_lookup = HashMap::<ManualAccountLookupKey, WalletAccountId>::new();
    {
        let mut stmt = tx
            .prepare(
                "SELECT id, wallet_id, label, asset_id, network_id, decimal_precision, unit_code,
                        symbol, asset_name, network_name, coingecko_id
                 FROM manual_asset_accounts
                 ORDER BY created_at ASC, id ASC",
            )
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to prepare manual account metadata query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                ))
            })
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to execute manual account metadata query: {err}"
                ))
            })?;

        for row in rows {
            let (
                account_id_raw,
                wallet_id_raw,
                account_label_raw,
                asset_id_raw,
                network_id_raw,
                _decimal_precision_raw,
                _unit_code_raw,
                _symbol_raw,
                _asset_name_raw,
                _network_name_raw,
                _coingecko_id_raw,
            ) = row.map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to map manual account metadata row: {err}"
                ))
            })?;

            let account_id = parse_wallet_account_id(&account_id_raw, "manual account id")?;
            let wallet_id = parse_wallet_id(&wallet_id_raw, "manual account wallet_id")?;
            let _account_label = parse_db_label(
                &account_label_raw,
                ACCOUNT_LABEL_MAX_LENGTH,
                "manual account label",
            )?;
            if !wallet_meta.contains_key(&wallet_id) {
                return Err(WalletDataImportDbError::Internal(format!(
                    "Manual account references missing wallet {}",
                    wallet_id
                )));
            }
            manual_account_lookup.insert(
                ManualAccountLookupKey {
                    wallet_id,
                    asset_id: asset_id_raw,
                    network_id: network_id_raw,
                },
                account_id,
            );
        }
    }

    let mut manual_asset_assertion_dates = HashMap::<WalletAccountId, HashSet<NaiveDate>>::new();
    {
        let mut stmt = tx
            .prepare(
                "SELECT account_id, asserted_on
                 FROM manual_asset_balance_assertions",
            )
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to prepare manual assertion date query: {err}"
                ))
            })?;

        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to execute manual assertion date query: {err}"
                ))
            })?;

        for row in rows {
            let (account_id_raw, asserted_on_raw) = row.map_err(|err| {
                WalletDataImportDbError::Internal(format!(
                    "Failed to map manual assertion date row: {err}"
                ))
            })?;
            let account_id =
                parse_wallet_account_id(&account_id_raw, "manual assertion account_id")?;
            let asserted_on = parse_date(&asserted_on_raw, "manual assertion asserted_on")?;
            manual_asset_assertion_dates
                .entry(account_id)
                .or_default()
                .insert(asserted_on);
        }
    }

    Ok(ImportState {
        wallet_by_fingerprint,
        wallet_meta,
        wallet_label_keys,
        wallet_account_label_keys,
        native_account_meta,
        hd_key_lookup,
        address_lookup,
        manual_account_lookup,
        manual_asset_assertion_dates,
    })
}

fn load_account_kind_in_tx(
    tx: &rusqlite::Transaction<'_>,
    account_id: WalletAccountId,
) -> Result<AccountKind, WalletDataImportDbError> {
    let raw = tx
        .query_row(
            "SELECT account_kind FROM digital_asset_accounts WHERE id = ?1 LIMIT 1",
            params![account_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| {
            WalletDataImportDbError::Internal(format!(
                "Failed to load account_kind for {}: {err}",
                account_id
            ))
        })?
        .ok_or_else(|| {
            WalletDataImportDbError::Internal(format!(
                "Account {} disappeared while importing",
                account_id
            ))
        })?;

    AccountKind::from_str(&raw).ok_or_else(|| {
        WalletDataImportDbError::Internal(format!("Invalid account_kind in DB: {raw}"))
    })
}

pub(super) fn resolve_native_account_candidates(
    account: &ParsedImportedNativeAccount,
    state: &ImportState,
) -> Result<HashSet<WalletAccountId>, WalletDataImportDbError> {
    let mut candidates = HashSet::<WalletAccountId>::new();

    match account.account_kind {
        AccountKind::HdPubkey => {
            for hd_key in &account.hd_keys {
                let key = HdKeyLookupKey {
                    normalized_extended_pubkey: hd_key.value.normalized_as_str().to_string(),
                    address_scheme: hd_key.address_scheme,
                };
                if let Some(account_id) = state.hd_key_lookup.get(&key).copied() {
                    candidates.insert(account_id);
                }
            }
        }
        AccountKind::SingleAddress => {
            for address in &account.addresses {
                let key = AddressLookupKey {
                    asset_id: address.asset_id.as_str().to_string(),
                    network: address.network.as_str().to_string(),
                    address_normalized: address.normalized_address.clone(),
                };
                if let Some(account_id) = state.address_lookup.get(&key).copied() {
                    candidates.insert(account_id);
                }
            }
        }
    }

    if candidates.len() > 1 {
        return Err(WalletDataImportDbError::Validation(format!(
            "Ambiguous account match for '{}' (matched {} candidate accounts)",
            account.label.as_str(),
            candidates.len()
        )));
    }

    Ok(candidates)
}

pub(super) fn resolve_or_create_wallet_id(
    tx: &rusqlite::Transaction<'_>,
    state: &mut ImportState,
    imported_wallet: &ParsedImportedWallet,
    now: DateTime<Utc>,
    result: &mut WalletDataImportResult,
) -> Result<WalletId, WalletDataImportDbError> {
    if let Some(fingerprint) = imported_wallet.master_fingerprint.as_ref()
        && let Some(wallet_id) = state
            .wallet_by_fingerprint
            .get(fingerprint.as_str())
            .copied()
    {
        let wallet_label = state
            .wallet_meta
            .get(&wallet_id)
            .ok_or_else(|| {
                WalletDataImportDbError::Internal(format!(
                    "Fingerprint lookup resolved missing wallet {}",
                    wallet_id
                ))
            })?
            .label
            .clone();
        result
            .wallets_matched
            .push(wallet_label.as_str().to_string());
        return Ok(wallet_id);
    }

    let mut affinity_wallet_ids = HashSet::<WalletId>::new();
    for native_account in &imported_wallet.native_accounts {
        let candidates = resolve_native_account_candidates(native_account, state)?;
        if let Some(account_id) = candidates.iter().next().copied() {
            let wallet_id = state
                .native_account_meta
                .get(&account_id)
                .ok_or_else(|| {
                    WalletDataImportDbError::Internal(format!(
                        "Native account candidate {} missing metadata",
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
        let wallet_label = state
            .wallet_meta
            .get(&wallet_id)
            .ok_or_else(|| {
                WalletDataImportDbError::Internal(format!(
                    "Affinity wallet {} missing metadata",
                    wallet_id
                ))
            })?
            .label
            .clone();
        result
            .wallets_matched
            .push(wallet_label.as_str().to_string());
        return Ok(wallet_id);
    }

    let unique_wallet_label = unique_label_with_numeric_suffix(
        &imported_wallet.label,
        &state.wallet_label_keys,
        WALLET_LABEL_MAX_LENGTH,
    )?;
    let wallet_id = insert_wallet_in_tx(
        tx,
        &unique_wallet_label,
        imported_wallet.master_fingerprint.as_ref(),
        now,
    )?;

    state.wallet_label_keys.insert(unique_wallet_label.key());
    state.wallet_meta.insert(
        wallet_id,
        ExistingWalletMeta {
            label: unique_wallet_label.clone(),
        },
    );
    state
        .wallet_account_label_keys
        .insert(wallet_id, HashSet::new());

    if let Some(fingerprint) = imported_wallet.master_fingerprint.as_ref() {
        state
            .wallet_by_fingerprint
            .insert(fingerprint.as_str().to_string(), wallet_id);
    }

    result
        .wallets_created
        .push(unique_wallet_label.as_str().to_string());

    Ok(wallet_id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResolvedNativeAccount {
    pub(super) account_id: WalletAccountId,
    pub(super) was_created: bool,
}

pub(super) fn resolve_or_create_native_account(
    tx: &rusqlite::Transaction<'_>,
    state: &mut ImportState,
    wallet_id: WalletId,
    account: &ParsedImportedNativeAccount,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    result: &mut WalletDataImportResult,
) -> Result<ResolvedNativeAccount, WalletDataImportDbError> {
    let candidates = resolve_native_account_candidates(account, state)?;

    if let Some(account_id) = candidates.iter().next().copied() {
        let db_account_kind = load_account_kind_in_tx(tx, account_id)?;
        if db_account_kind != account.account_kind {
            return Err(WalletDataImportDbError::Validation(format!(
                "Matched existing account '{}' has kind '{}' but import declared '{}'",
                account.label.as_str(),
                db_account_kind.as_str(),
                account.account_kind.as_str()
            )));
        }

        let meta = state.native_account_meta.get(&account_id).ok_or_else(|| {
            WalletDataImportDbError::Internal(format!(
                "Matched account {} is missing metadata",
                account_id
            ))
        })?;
        result
            .native_accounts_matched
            .push(ImportNativeAccountView {
                wallet_label: meta.wallet_label.as_str().to_string(),
                account_label: meta.account_label.as_str().to_string(),
            });
        return Ok(ResolvedNativeAccount {
            account_id,
            was_created: false,
        });
    }

    let wallet_label = state
        .wallet_meta
        .get(&wallet_id)
        .ok_or_else(|| {
            WalletDataImportDbError::Internal(format!(
                "Missing metadata for wallet {} while creating account",
                wallet_id
            ))
        })?
        .label
        .clone();

    let existing_label_keys = state
        .wallet_account_label_keys
        .entry(wallet_id)
        .or_default()
        .clone();

    let unique_label = unique_label_with_numeric_suffix(
        &account.label,
        &existing_label_keys,
        ACCOUNT_LABEL_MAX_LENGTH,
    )?;

    let account_id = insert_native_account_in_tx(
        tx,
        wallet_id,
        &unique_label,
        account,
        created_at,
        updated_at,
    )?;

    state
        .wallet_account_label_keys
        .entry(wallet_id)
        .or_default()
        .insert(unique_label.key());
    state.native_account_meta.insert(
        account_id,
        ExistingNativeAccountMeta {
            wallet_id,
            wallet_label: wallet_label.clone(),
            account_label: unique_label.clone(),
        },
    );

    result
        .native_accounts_created
        .push(ImportNativeAccountView {
            wallet_label: wallet_label.as_str().to_string(),
            account_label: unique_label.as_str().to_string(),
        });

    Ok(ResolvedNativeAccount {
        account_id,
        was_created: true,
    })
}

pub(super) fn resolve_or_create_manual_account(
    tx: &rusqlite::Transaction<'_>,
    state: &mut ImportState,
    wallet_id: WalletId,
    account: &ParsedImportedManualAccount,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> Result<WalletAccountId, WalletDataImportDbError> {
    let lookup_key = ManualAccountLookupKey {
        wallet_id,
        asset_id: account.snapshot.asset_id.clone(),
        network_id: account.snapshot.network_id.clone(),
    };

    if let Some(account_id) = state.manual_account_lookup.get(&lookup_key).copied() {
        return Ok(account_id);
    }

    let existing_label_keys = state
        .wallet_account_label_keys
        .entry(wallet_id)
        .or_default()
        .clone();

    let unique_label = unique_label_with_numeric_suffix(
        &account.label,
        &existing_label_keys,
        ACCOUNT_LABEL_MAX_LENGTH,
    )?;

    let account_id = insert_manual_asset_account_in_tx(
        tx,
        wallet_id,
        &unique_label,
        &account.snapshot,
        created_at,
        updated_at,
    )?;

    state
        .wallet_account_label_keys
        .entry(wallet_id)
        .or_default()
        .insert(unique_label.key());
    state.manual_account_lookup.insert(lookup_key, account_id);

    Ok(account_id)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::wallets::{
        AccountIndex, AddressScheme, DerivationCoinType, DerivationPurpose, KeyRole, Network,
        SyncedAssetId, ValidatedExtendedPubkey,
    };

    #[test]
    fn resolve_native_account_candidates_returns_single_match() {
        let account_id = WalletAccountId::new();

        let mut state = ImportState {
            wallet_by_fingerprint: HashMap::new(),
            wallet_meta: HashMap::new(),
            wallet_label_keys: HashSet::new(),
            wallet_account_label_keys: HashMap::new(),
            native_account_meta: HashMap::new(),
            hd_key_lookup: HashMap::new(),
            address_lookup: HashMap::new(),
            manual_account_lookup: HashMap::new(),
            manual_asset_assertion_dates: HashMap::new(),
        };

        let xpub = ValidatedExtendedPubkey::parse(
            AddressScheme::NativeSegwit,
            "zpub6qU5MALAB8Bscej9sTEkgSocaxvLzAYYeytsL9fXfv8W4BTykA99FNDNpftwXMGomwc2KatVrbXo4qXsdBC1DiNHCHGapas9enpPBo8y8Y4",
        )
        .expect("xpub should parse");

        state.hd_key_lookup.insert(
            HdKeyLookupKey {
                normalized_extended_pubkey: xpub.normalized_as_str().to_string(),
                address_scheme: AddressScheme::NativeSegwit,
            },
            account_id,
        );

        let account = ParsedImportedNativeAccount {
            label: Label::parse_with_limit("BTC", ACCOUNT_LABEL_MAX_LENGTH)
                .expect("label should parse"),
            asset_id: SyncedAssetId::Bitcoin,
            network: Network::Mainnet,
            account_kind: AccountKind::HdPubkey,
            created_at: None,
            sync_slot: None,
            hd_keys: vec![super::super::parse::ParsedImportedHdKey {
                key_role: KeyRole::Primary,
                value: xpub,
                derivation_purpose: DerivationPurpose::Bip84,
                derivation_coin_type: DerivationCoinType::new(0),
                derivation_account: AccountIndex::new(0).expect("index should parse"),
                address_scheme: AddressScheme::NativeSegwit,
                display_identifier: "zpub...".to_string(),
            }],
            addresses: Vec::new(),
        };

        let candidates = resolve_native_account_candidates(&account, &state)
            .expect("candidate resolution should succeed");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates.iter().next().copied(), Some(account_id));
    }
}
