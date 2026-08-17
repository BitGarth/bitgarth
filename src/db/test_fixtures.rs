use crate::db::transaction_sync::SyncAddress;
use crate::ethereum::EthAddress;
use crate::models::UserId;
use crate::wallets::{
    AccountKind, AddressScheme, AddressSourceType, IdentitySource, Label, Network, SyncedAssetId,
    WALLET_LABEL_MAX_LENGTH, WalletId,
};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

use super::app_db::{enable_test_mode as enable_app_test_mode, with_app_db_mut};
use super::error::DbError;
use super::raw_ingestion::ensure_source_connection_for_address_tx;
use super::user_db::{enable_test_mode, initialize_user_db_for_test, with_user_db_mut};
use super::wallets::{AddEthAddressDbResult, add_ethereum_address};

pub(crate) fn ensure_test_app_user(user_id: UserId) {
    enable_app_test_mode();

    let now = Utc::now().to_rfc3339();
    let username = format!("test_user_{user_id}");
    let result: Result<(), DbError> = with_app_db_mut(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO users (user_id, username, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![user_id.to_string(), username, now, now],
        )
        .map_err(|e| DbError::new(e.to_string()))?;
        Ok(())
    });

    result.expect("Should ensure test app user");
}

pub(crate) fn setup_test_user(user_id: UserId) {
    ensure_test_app_user(user_id);
    enable_test_mode();
    initialize_user_db_for_test(user_id).expect("Should initialize user db");
}

#[cfg(feature = "dev-config")]
pub(crate) fn setup_unencrypted_dev_test_user(user_id: UserId) {
    use crate::db::encryption::{DbEnvelope, UserDbOpenMode, write_envelope};

    ensure_test_app_user(user_id);
    enable_test_mode();
    write_envelope(user_id, &DbEnvelope::unencrypted_dev())
        .expect("unencrypted test envelope should be written");
    super::user_db::initialize_user_db(user_id, UserDbOpenMode::UnencryptedDev)
        .expect("unencrypted test user database should initialize");
}

pub(crate) fn unique_user_id() -> UserId {
    UserId::new()
}

pub(crate) fn wallet_label(value: &str) -> Label {
    Label::parse_with_limit(value, WALLET_LABEL_MAX_LENGTH).expect("valid wallet label")
}

pub(crate) fn create_eth_wallet_account_fixture(
    user_id: UserId,
    address: &EthAddress,
    wallet_label_value: &str,
    now: DateTime<Utc>,
) -> AddEthAddressDbResult {
    let label = wallet_label(wallet_label_value);
    add_ethereum_address(user_id, address, Network::Mainnet, None, Some(&label), now)
        .expect("should create wallet account fixture")
}

pub(crate) fn add_eth_account_to_existing_wallet_fixture(
    user_id: UserId,
    address: &EthAddress,
    wallet_id: WalletId,
    now: DateTime<Utc>,
) -> AddEthAddressDbResult {
    add_ethereum_address(
        user_id,
        address,
        Network::Mainnet,
        Some(&wallet_id),
        None,
        now,
    )
    .expect("should create account fixture in existing wallet")
}

pub(crate) fn persist_sync_address_fixture(
    user_id: UserId,
    sync_address: &SyncAddress,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::new(format!("Failed to start sync address fixture tx: {err}"))
        })?;
        let timestamp = now.to_rfc3339();
        let account_id = ensure_test_account_for_sync_address_tx(&tx, sync_address, now)?;
        let address_exists = tx
            .query_row(
                "SELECT 1 FROM digital_asset_addresses WHERE id = ?1 LIMIT 1",
                params![sync_address.address_id.to_string()],
                |_| Ok(()),
            )
            .optional()
            .map_err(|err| DbError::new(format!("Failed to query sync address fixture: {err}")))?;

        if address_exists.is_none() {
            let address_scheme = sync_address
                .address_scheme
                .unwrap_or(default_address_scheme_for_asset(sync_address.asset_id));
            let source_type = match (
                sync_address.derivation_change,
                sync_address.derivation_index,
            ) {
                (Some(_), Some(_)) => AddressSourceType::Derived,
                _ => AddressSourceType::Imported,
            };
            let normalized_address = sync_address.address.as_str().to_ascii_lowercase();

            tx.execute(
                "INSERT INTO digital_asset_addresses
                 (id, account_id, asset_id, network, address, address_normalized, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    sync_address.address_id.to_string(),
                    account_id.to_string(),
                    sync_address.asset_id.as_str(),
                    sync_address.network.as_str(),
                    sync_address.address.as_str(),
                    normalized_address,
                    address_scheme.as_str(),
                    sync_address.derivation_change.map(i64::from),
                    sync_address.derivation_index.map(i64::from),
                    source_type.as_str(),
                    timestamp,
                    timestamp,
                ],
            )
            .map_err(|err| DbError::new(format!("Failed to insert sync address fixture: {err}")))?;
        }

        ensure_source_connection_for_address_tx(
            &tx,
            sync_address.address_id,
            sync_address.asset_id,
            sync_address.network,
            &sync_address.address.as_str().to_ascii_lowercase(),
            now,
        )?;

        tx.commit()
            .map_err(|err| DbError::new(format!("Failed to commit sync address fixture: {err}")))?;
        Ok(())
    })
}

fn ensure_test_account_for_sync_address_tx(
    tx: &rusqlite::Transaction<'_>,
    sync_address: &SyncAddress,
    now: DateTime<Utc>,
) -> Result<crate::wallets::DigitalAssetAccountId, DbError> {
    let account_id = sync_address
        .account_id
        .unwrap_or_else(crate::wallets::DigitalAssetAccountId::new);
    let account_exists = tx
        .query_row(
            "SELECT 1 FROM digital_asset_accounts WHERE id = ?1 LIMIT 1",
            params![account_id.to_string()],
            |_| Ok(()),
        )
        .optional()
        .map_err(|err| DbError::new(format!("Failed to query sync account fixture: {err}")))?;
    if account_exists.is_some() {
        return Ok(account_id);
    }

    let timestamp = now.to_rfc3339();
    let wallet_id = WalletId::new();
    let wallet_label = format!("test-wallet-{wallet_id}");
    let account_label = format!("test-account-{account_id}");
    let account_kind = match (
        sync_address.derivation_change,
        sync_address.derivation_index,
    ) {
        (Some(_), Some(_)) => AccountKind::HdPubkey,
        _ => AccountKind::SingleAddress,
    };

    tx.execute(
        "INSERT INTO wallets
         (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            wallet_id.to_string(),
            wallet_label,
            account_label,
            Option::<String>::None,
            IdentitySource::UserProvided.as_str(),
            Option::<String>::None,
            timestamp,
            timestamp,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert sync wallet fixture: {err}")))?;
    tx.execute(
        "INSERT INTO digital_asset_accounts
         (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            account_id.to_string(),
            wallet_id.to_string(),
            account_label,
            format!("test-account-key-{account_id}"),
            sync_address.asset_id.as_str(),
            sync_address.network.as_str(),
            account_kind.as_str(),
            timestamp,
            timestamp,
        ],
    )
    .map_err(|err| DbError::new(format!("Failed to insert sync account fixture: {err}")))?;
    Ok(account_id)
}

fn default_address_scheme_for_asset(asset_id: SyncedAssetId) -> AddressScheme {
    match asset_id {
        SyncedAssetId::Bitcoin => AddressScheme::NativeSegwit,
        SyncedAssetId::Ethereum => AddressScheme::Standard,
    }
}
