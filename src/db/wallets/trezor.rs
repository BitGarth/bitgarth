use super::derivation::{InitialHdAddressBootstrapRequest, bootstrap_initial_hd_account_addresses};
use super::errors::{LinkTrezorDbError, db_error_from_sqlite};
use crate::account_limits::AccountActivationState;
use crate::db::account_limits::{
    account_state_for, classify_supported_accounts_in_tx,
    ensure_supported_account_hard_cap_before_insert_in_tx,
};
use crate::db::error::DbError;
use crate::db::user_db::with_user_db_mut;
use crate::db::wallet_accounts::query_wallet_account_label_keys_in_tx;
use crate::wallets::{
    AccessorKind, AccountKind, BIP44_GAP_LIMIT, DigitalAssetAccountId, HdKeyId, IdentitySource,
    KeyRole, KeySource, Label, LinkTrezorOutcome, Network, SyncedAssetId,
    ValidatedLinkTrezorRequest, ValidatedMasterFingerprint, WALLET_LABEL_MAX_LENGTH,
    WalletAccessorId, WalletId, generate_unique_account_label, hash_device_id,
};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use std::str::FromStr;

#[derive(Debug)]
pub(crate) struct LinkTrezorDbResult {
    pub(crate) wallet_id: WalletId,
    pub(crate) created_account_ids: Vec<DigitalAssetAccountId>,
    pub(crate) skipped_account_indexes: Vec<crate::wallets::AccountIndex>,
    pub(crate) outcome: LinkTrezorOutcome,
}

fn attach_master_fingerprint_to_wallet_in_tx(
    tx: &rusqlite::Transaction<'_>,
    wallet_id: WalletId,
    master_fingerprint: &ValidatedMasterFingerprint,
    now: DateTime<Utc>,
) -> Result<(), LinkTrezorDbError> {
    let timestamp = now.to_rfc3339();
    let rows_updated = tx
        .execute(
            "UPDATE wallets
             SET master_fingerprint = ?1,
                 identity_source = ?2,
                 verified_at = COALESCE(verified_at, ?3),
                 updated_at = ?3
             WHERE id = ?4
               AND (master_fingerprint IS NULL OR master_fingerprint = ?1)",
            params![
                master_fingerprint.as_str(),
                IdentitySource::DeviceVerified.as_str(),
                timestamp,
                wallet_id.to_string(),
            ],
        )
        .map_err(|e| {
            LinkTrezorDbError::from(db_error_from_sqlite(
                "Failed to attach wallet master fingerprint",
                e,
            ))
        })?;

    if rows_updated == 0 {
        return Err(LinkTrezorDbError::MasterFingerprintConflict);
    }

    Ok(())
}

pub(crate) fn link_trezor_wallet(
    user_id: crate::models::UserId,
    request: ValidatedLinkTrezorRequest,
    active_limit: usize,
    now: DateTime<Utc>,
) -> Result<LinkTrezorDbResult, LinkTrezorDbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|e| {
            LinkTrezorDbError::internal(format!("Failed to start transaction: {e}"))
        })?;

        let fingerprint_wallet_id = tx
            .query_row(
                "SELECT id FROM wallets WHERE master_fingerprint = ?1",
                [request.master_fingerprint.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| {
                LinkTrezorDbError::internal(format!("Failed to find wallet by fingerprint: {e}"))
            })?;

        // Normalized-key wallet affinity takes precedence over fingerprint affinity.
        let mut affinity_wallet_ids = std::collections::BTreeSet::<String>::new();
        for account in &request.accounts {
            let wallet_id = tx
                .query_row(
                    "SELECT a.wallet_id \
                     FROM digital_asset_accounts a \
                     JOIN digital_asset_account_hd_keys k ON k.account_id = a.id \
                     WHERE k.normalized_extended_pubkey = ?1 \
                     LIMIT 1",
                    [account.extended_pubkey.normalized_as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| {
                    LinkTrezorDbError::internal(format!(
                        "Failed to resolve normalized key wallet affinity: {e}"
                    ))
                })?;

            if let Some(wallet_id) = wallet_id {
                affinity_wallet_ids.insert(wallet_id);
            }
        }

        if affinity_wallet_ids.len() > 1 {
            return Err(LinkTrezorDbError::MultiWalletAffinityConflict);
        }

        let affinity_wallet_id = affinity_wallet_ids.into_iter().next();

        let (wallet_id, outcome) = if let Some(affinity_wallet_id) = affinity_wallet_id {
            if let Some(existing_fingerprint_wallet_id) = &fingerprint_wallet_id
                && existing_fingerprint_wallet_id != &affinity_wallet_id
            {
                return Err(LinkTrezorDbError::MasterFingerprintConflict);
            }

            let parsed_id = WalletId::from_str(&affinity_wallet_id).map_err(|e| {
                LinkTrezorDbError::internal(format!("Invalid affinity wallet id in db: {e}"))
            })?;
            (parsed_id, LinkTrezorOutcome::ExistingWallet)
        } else {
            match fingerprint_wallet_id {
                Some(id) => {
                    let parsed_id = WalletId::from_str(&id).map_err(|e| {
                        LinkTrezorDbError::internal(format!("Invalid wallet id in db: {e}"))
                    })?;
                    (parsed_id, LinkTrezorOutcome::ExistingWallet)
                }
                None => {
                    let new_id = WalletId::new();
                    let timestamp = now.to_rfc3339();

                    let wallet_label_key = request.wallet_label.key();
                    tx.execute(
                        "INSERT INTO wallets \
                         (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            new_id.to_string(),
                            request.wallet_label.as_str(),
                            wallet_label_key.as_str(),
                            request.master_fingerprint.as_str(),
                            IdentitySource::DeviceVerified.as_str(),
                            timestamp,
                            timestamp,
                            timestamp,
                        ],
                    )
                    .map_err(|e| {
                        LinkTrezorDbError::from(db_error_from_sqlite("Failed to insert wallet", e))
                    })?;

                    (new_id, LinkTrezorOutcome::NewWallet)
                }
            }
        };

        if outcome == LinkTrezorOutcome::ExistingWallet {
            attach_master_fingerprint_to_wallet_in_tx(
                &tx,
                wallet_id,
                &request.master_fingerprint,
                now,
            )?;
        }

        let device_id_hash = request
            .device_id
            .as_ref()
            .map(|device_id| hash_device_id(device_id.as_str()));

        let accessor_label = match request.device_label.as_ref() {
            Some(raw) => Some(
                Label::parse_with_limit(raw.as_str(), WALLET_LABEL_MAX_LENGTH).map_err(|e| {
                    LinkTrezorDbError::internal(format!("Invalid accessor label: {e}"))
                })?,
            ),
            None => None,
        };

        let accessor_id = find_or_create_trezor_accessor(
            &tx,
            wallet_id,
            device_id_hash.as_deref(),
            accessor_label,
            now,
        )
        .map_err(LinkTrezorDbError::from)?;

        let mut creating_account_keys = std::collections::BTreeSet::new();
        for account in &request.accounts {
            let existing_hd_key = tx
                .query_row(
                    "SELECT id FROM digital_asset_account_hd_keys \
                     WHERE normalized_extended_pubkey = ?1 AND address_scheme = ?2 \
                     LIMIT 1",
                    params![
                        account.extended_pubkey.normalized_as_str(),
                        account.extended_pubkey.address_scheme().as_str()
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| {
                    LinkTrezorDbError::internal(format!("Failed to lookup hd key: {e}"))
                })?;

            if existing_hd_key.is_none() {
                creating_account_keys.insert((
                    account.extended_pubkey.normalized_as_str().to_string(),
                    account
                        .extended_pubkey
                        .address_scheme()
                        .as_str()
                        .to_string(),
                ));
            }
        }
        ensure_supported_account_hard_cap_before_insert_in_tx(&tx, creating_account_keys.len())
            .map_err(LinkTrezorDbError::from)?;

        let mut created_account_ids = Vec::new();
        let mut created_hd_accounts = Vec::new();
        let mut skipped_account_indexes = Vec::new();

        for account in request.accounts {
            let timestamp = now.to_rfc3339();
            let existing_hd_key = tx
                .query_row(
                    "SELECT id FROM digital_asset_account_hd_keys \
                     WHERE normalized_extended_pubkey = ?1 AND address_scheme = ?2 \
                     LIMIT 1",
                    params![
                        account.extended_pubkey.normalized_as_str(),
                        account.extended_pubkey.address_scheme().as_str()
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| {
                    LinkTrezorDbError::internal(format!("Failed to lookup hd key: {e}"))
                })?;

            if existing_hd_key.is_some() {
                if !skipped_account_indexes.contains(&account.account_index) {
                    skipped_account_indexes.push(account.account_index);
                }
                continue;
            }

            let account_id = DigitalAssetAccountId::new();
            let existing_keys = query_wallet_account_label_keys_in_tx(&tx, wallet_id)
                .map_err(LinkTrezorDbError::from)?
                .into_iter()
                .map(|row| {
                    let _ = row.account_id;
                    row.label_key
                })
                .collect::<Vec<_>>();
            let account_label = generate_unique_account_label(
                SyncedAssetId::Bitcoin,
                &existing_keys,
            )
            .map_err(|e| {
                LinkTrezorDbError::internal(format!("Failed to generate account label: {e}"))
            })?;
            let account_label_key = account_label.key();
            tx.execute(
                "INSERT INTO digital_asset_accounts \
                 (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    account_id.to_string(),
                    wallet_id.to_string(),
                    account_label.as_str(),
                    account_label_key.as_str(),
                    SyncedAssetId::Bitcoin.as_str(),
                    Network::Mainnet.as_str(),
                    AccountKind::HdPubkey.as_str(),
                    timestamp,
                    timestamp,
                ],
            )
            .map_err(|e| {
                LinkTrezorDbError::from(db_error_from_sqlite("Failed to insert account", e))
            })?;
            created_account_ids.push(account_id);

            tx.execute(
                "INSERT INTO digital_asset_account_hd_keys \
                 (id, account_id, key_role, extended_pubkey, normalized_extended_pubkey, derivation_purpose, derivation_coin_type, derivation_account, address_scheme, key_source, verified_by_accessor_id, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    HdKeyId::new().to_string(),
                    account_id.to_string(),
                    KeyRole::Primary.as_str(),
                    account.extended_pubkey.as_str(),
                    account.extended_pubkey.normalized_as_str(),
                    account.derivation_path.purpose.value() as i64,
                    account.derivation_path.coin_type.value() as i64,
                    account.derivation_path.account.as_u32() as i64,
                    account.extended_pubkey.address_scheme().as_str(),
                    KeySource::DeviceVerified.as_str(),
                    Some(accessor_id.to_string()),
                    timestamp,
                    timestamp,
                ],
            )
            .map_err(|e| LinkTrezorDbError::from(db_error_from_sqlite("Failed to insert hd key", e)))?;

            created_hd_accounts.push((
                account_id,
                account.extended_pubkey.address_scheme(),
                account.extended_pubkey.as_str().to_string(),
            ));
        }

        let classified = classify_supported_accounts_in_tx(&tx, active_limit)
            .map_err(LinkTrezorDbError::from)?;
        for (account_id, address_scheme, extended_pubkey) in created_hd_accounts {
            if account_state_for(&classified, &account_id.into()) != AccountActivationState::Active
            {
                continue;
            }
            bootstrap_initial_hd_account_addresses(
                &tx,
                InitialHdAddressBootstrapRequest {
                    account_id,
                    asset_id: SyncedAssetId::Bitcoin,
                    network: Network::Mainnet,
                    address_scheme,
                    extended_pubkey: extended_pubkey.as_str(),
                    gap_limit: BIP44_GAP_LIMIT,
                    now,
                },
            )
            .map_err(LinkTrezorDbError::from)?;
        }

        tx.commit().map_err(|e| {
            LinkTrezorDbError::internal(format!("Failed to commit transaction: {e}"))
        })?;

        Ok(LinkTrezorDbResult {
            wallet_id,
            created_account_ids,
            skipped_account_indexes,
            outcome,
        })
    })
}

pub(super) fn find_or_create_trezor_accessor(
    tx: &rusqlite::Transaction<'_>,
    wallet_id: WalletId,
    device_id_hash: Option<&str>,
    accessor_label: Option<Label>,
    now: DateTime<Utc>,
) -> Result<WalletAccessorId, DbError> {
    let existing_id = tx
        .query_row(
            "SELECT id FROM wallet_accessors \
             WHERE wallet_id = ?1 AND accessor_kind = ?2 \
               AND ((?3 IS NOT NULL AND device_id_hash = ?3) OR (?3 IS NULL AND device_id_hash IS NULL)) \
             LIMIT 1",
            params![wallet_id.to_string(), AccessorKind::Trezor.as_str(), device_id_hash],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| DbError::new(format!("Failed to lookup wallet accessor: {e}")))?;

    match existing_id {
        Some(id) => {
            let accessor_id = WalletAccessorId::from_str(&id)
                .map_err(|e| DbError::new(format!("Invalid wallet accessor id in db: {e}")))?;

            if let Some(label) = accessor_label {
                tx.execute(
                    "UPDATE wallet_accessors SET accessor_label = ?1, updated_at = ?2 WHERE id = ?3",
                    params![
                        label.as_str(),
                        now.to_rfc3339(),
                        accessor_id.to_string(),
                    ],
                )
                .map_err(|e| DbError::new(format!("Failed to update wallet accessor label: {e}")))?;
            }

            Ok(accessor_id)
        }
        None => {
            let accessor_id = WalletAccessorId::new();
            let timestamp = now.to_rfc3339();
            let accessor_label_value = accessor_label
                .as_ref()
                .map(|label| label.as_str().to_string());

            tx.execute(
                "INSERT INTO wallet_accessors \
                 (id, wallet_id, accessor_kind, accessor_label, device_id_hash, device_model, accessor_version, firmware_version, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    accessor_id.to_string(),
                    wallet_id.to_string(),
                    AccessorKind::Trezor.as_str(),
                    accessor_label_value,
                    device_id_hash,
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    timestamp,
                    timestamp,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert wallet accessor: {e}")))?;

            Ok(accessor_id)
        }
    }
}
