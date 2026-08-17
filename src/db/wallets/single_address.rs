use super::errors::db_error_from_sqlite;
use crate::db::account_limits::ensure_supported_account_hard_cap_before_insert_in_tx;
use crate::db::error::DbError;
use crate::db::raw_ingestion::ensure_source_connection_for_address_tx;
use crate::db::user_db::with_user_db_mut;
use crate::ethereum::EthAddress;
use crate::models::UserId;
use crate::wallets::{
    AccountKind, AddressScheme, AddressSourceType, BtcAddress, DigitalAssetAccountId,
    DigitalAssetAddressId, IdentitySource, Label, Network, SyncedAssetId, WalletId,
    generate_unique_account_label,
};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

#[derive(Debug)]
pub(crate) struct AddEthAddressDbResult {
    pub(crate) wallet_id: WalletId,
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) address_id: DigitalAssetAddressId,
}

#[derive(Debug)]
pub(crate) struct AddBtcAddressDbResult {
    pub(crate) wallet_id: WalletId,
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) address_id: DigitalAssetAddressId,
}

/// Add a user-provided Ethereum address.
///
/// When `existing_wallet_id` is `None`, a new single-key wallet is created.
/// When `Some`, the account and address are added to the existing wallet.
///
/// Returns an error if the normalized address already exists for the same
/// asset and network, or if the specified wallet does not exist.
#[cfg(any(feature = "dev-config", feature = "db-tests"))]
pub(crate) fn add_ethereum_address(
    user_id: UserId,
    address: &EthAddress,
    network: Network,
    existing_wallet_id: Option<&WalletId>,
    wallet_label: Option<&Label>,
    now: DateTime<Utc>,
) -> Result<AddEthAddressDbResult, DbError> {
    add_ethereum_address_with_account_label(
        user_id,
        address,
        network,
        existing_wallet_id,
        wallet_label,
        None,
        now,
    )
}

/// Like [`add_ethereum_address`], but with an optional user-provided account
/// name. When `account_label` is `None`, the account is auto-named.
pub(crate) fn add_ethereum_address_with_account_label(
    user_id: UserId,
    address: &EthAddress,
    network: Network,
    existing_wallet_id: Option<&WalletId>,
    wallet_label: Option<&Label>,
    account_label: Option<&Label>,
    now: DateTime<Utc>,
) -> Result<AddEthAddressDbResult, DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn
            .transaction()
            .map_err(|e| db_error_from_sqlite("Failed to start ethereum address transaction", e))?;

        let timestamp = now.to_rfc3339();
        let normalized = address.normalized();

        let wallet_id = match existing_wallet_id {
            Some(id) => {
                // Verify the wallet exists in the user's database
                let wallet_exists = tx
                    .query_row(
                        "SELECT 1 FROM wallets WHERE id = ?1 LIMIT 1",
                        params![id.to_string()],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(|e| DbError::new(format!("Failed to verify wallet existence: {e}")))?;
                if wallet_exists.is_none() {
                    return Err(DbError::new(format!("Wallet not found: {}", id)));
                }
                *id
            }
            None => {
                // Create wallet (single_key, no fingerprint)
                let new_wallet_id = WalletId::new();
                let effective_label = wallet_label.cloned().ok_or_else(|| {
                    DbError::new("wallet_label is required when creating a wallet")
                })?;
                let wl_key = effective_label.key();
                tx.execute(
                    "INSERT INTO wallets \
                     (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        new_wallet_id.to_string(),
                        effective_label.as_str(),
                        wl_key.as_str(),
                        Option::<String>::None,
                        IdentitySource::UserProvided.as_str(),
                        Option::<String>::None,
                        timestamp,
                        timestamp,
                    ],
                )
                .map_err(|e| db_error_from_sqlite("Failed to insert wallet", e))?;
                new_wallet_id
            }
        };

        // Create account (account model, single_address kind)
        let account_id = DigitalAssetAccountId::new();
        let acct_label =
            super::labels::resolve_new_account_label(&tx, wallet_id, account_label, |keys| {
                generate_unique_account_label(SyncedAssetId::Ethereum, keys)
                    .map_err(|e| DbError::new(format!("Failed to generate account label: {e}")))
            })?;
        let acct_label_key = acct_label.key();
        ensure_supported_account_hard_cap_before_insert_in_tx(&tx, 1)?;
        tx.execute(
            "INSERT INTO digital_asset_accounts \
             (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                account_id.to_string(),
                wallet_id.to_string(),
                acct_label.as_str(),
                acct_label_key.as_str(),
                SyncedAssetId::Ethereum.as_str(),
                network.as_str(),
                AccountKind::SingleAddress.as_str(),
                timestamp,
                timestamp,
            ],
        )
        .map_err(|e| db_error_from_sqlite("Failed to insert ethereum account", e))?;

        // Create address (standard scheme, user_provided source)
        let address_id = DigitalAssetAddressId::new();
        tx.execute(
            "INSERT INTO digital_asset_addresses \
             (id, account_id, asset_id, network, address, address_normalized, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                address_id.to_string(),
                account_id.to_string(),
                SyncedAssetId::Ethereum.as_str(),
                network.as_str(),
                address.checksummed(),
                normalized,
                AddressScheme::Standard.as_str(),
                Option::<i64>::None,
                Option::<i64>::None,
                AddressSourceType::UserProvided.as_str(),
                timestamp,
                timestamp,
            ],
        )
        .map_err(|e| db_error_from_sqlite("Failed to insert ethereum address", e))?;
        ensure_source_connection_for_address_tx(
            &tx,
            address_id,
            SyncedAssetId::Ethereum,
            network,
            &address.normalized(),
            now,
        )?;

        tx.commit().map_err(|e| {
            db_error_from_sqlite("Failed to commit ethereum address transaction", e)
        })?;

        Ok(AddEthAddressDbResult {
            wallet_id,
            account_id,
            address_id,
        })
    })
}

/// Add a user-provided Bitcoin address.
///
/// When `existing_wallet_id` is `None`, a new single-key wallet is created.
/// When `Some`, the account and address are added to the existing wallet.
///
/// The address scheme is auto-detected from the validated address type.
///
/// Returns an error if the normalized address already exists for the same
/// asset and network, or if the specified wallet does not exist.
#[cfg(any(feature = "dev-config", feature = "db-tests"))]
pub(crate) fn add_bitcoin_address(
    user_id: UserId,
    address: &BtcAddress,
    network: Network,
    existing_wallet_id: Option<&WalletId>,
    wallet_label: Option<&Label>,
    now: DateTime<Utc>,
) -> Result<AddBtcAddressDbResult, DbError> {
    add_bitcoin_address_with_account_label(
        user_id,
        address,
        network,
        existing_wallet_id,
        wallet_label,
        None,
        now,
    )
}

/// Like [`add_bitcoin_address`], but with an optional user-provided account
/// name. When `account_label` is `None`, the account is auto-named.
pub(crate) fn add_bitcoin_address_with_account_label(
    user_id: UserId,
    address: &BtcAddress,
    network: Network,
    existing_wallet_id: Option<&WalletId>,
    wallet_label: Option<&Label>,
    account_label: Option<&Label>,
    now: DateTime<Utc>,
) -> Result<AddBtcAddressDbResult, DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn
            .transaction()
            .map_err(|e| db_error_from_sqlite("Failed to start bitcoin address transaction", e))?;

        let timestamp = now.to_rfc3339();

        let wallet_id = match existing_wallet_id {
            Some(id) => {
                let wallet_exists = tx
                    .query_row(
                        "SELECT 1 FROM wallets WHERE id = ?1 LIMIT 1",
                        params![id.to_string()],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(|e| DbError::new(format!("Failed to verify wallet existence: {e}")))?;
                if wallet_exists.is_none() {
                    return Err(DbError::new(format!("Wallet not found: {}", id)));
                }
                *id
            }
            None => {
                let new_wallet_id = WalletId::new();
                let effective_label = wallet_label.cloned().ok_or_else(|| {
                    DbError::new("wallet_label is required when creating a wallet")
                })?;
                let wl_key = effective_label.key();
                tx.execute(
                    "INSERT INTO wallets \
                     (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        new_wallet_id.to_string(),
                        effective_label.as_str(),
                        wl_key.as_str(),
                        Option::<String>::None,
                        IdentitySource::UserProvided.as_str(),
                        Option::<String>::None,
                        timestamp,
                        timestamp,
                    ],
                )
                .map_err(|e| db_error_from_sqlite("Failed to insert wallet", e))?;
                new_wallet_id
            }
        };

        // Create account (SingleAddress kind)
        let account_id = DigitalAssetAccountId::new();
        let acct_label =
            super::labels::resolve_new_account_label(&tx, wallet_id, account_label, |keys| {
                generate_unique_account_label(SyncedAssetId::Bitcoin, keys)
                    .map_err(|e| DbError::new(format!("Failed to generate account label: {e}")))
            })?;
        let acct_label_key = acct_label.key();
        ensure_supported_account_hard_cap_before_insert_in_tx(&tx, 1)?;
        tx.execute(
            "INSERT INTO digital_asset_accounts \
             (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                account_id.to_string(),
                wallet_id.to_string(),
                acct_label.as_str(),
                acct_label_key.as_str(),
                SyncedAssetId::Bitcoin.as_str(),
                network.as_str(),
                AccountKind::SingleAddress.as_str(),
                timestamp,
                timestamp,
            ],
        )
        .map_err(|e| db_error_from_sqlite("Failed to insert bitcoin account", e))?;

        // Create address (auto-detected scheme, user_provided source)
        let address_id = DigitalAssetAddressId::new();
        tx.execute(
            "INSERT INTO digital_asset_addresses \
             (id, account_id, asset_id, network, address, address_normalized, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                address_id.to_string(),
                account_id.to_string(),
                SyncedAssetId::Bitcoin.as_str(),
                network.as_str(),
                address.canonical(),
                address.normalized(),
                address.address_scheme().as_str(),
                Option::<i64>::None,
                Option::<i64>::None,
                AddressSourceType::UserProvided.as_str(),
                timestamp,
                timestamp,
            ],
        )
        .map_err(|e| db_error_from_sqlite("Failed to insert bitcoin address", e))?;
        ensure_source_connection_for_address_tx(
            &tx,
            address_id,
            SyncedAssetId::Bitcoin,
            network,
            address.normalized(),
            now,
        )?;

        tx.commit()
            .map_err(|e| db_error_from_sqlite("Failed to commit bitcoin address transaction", e))?;

        Ok(AddBtcAddressDbResult {
            wallet_id,
            account_id,
            address_id,
        })
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::WalletDbConflict;
    use crate::db::wallets::errors::classify_wallet_db_conflict;
    use crate::db::wallets::loaders::list_wallets;
    use crate::db::{
        add_eth_account_to_existing_wallet_fixture, create_eth_wallet_account_fixture,
        setup_test_user, unique_user_id, wallet_label,
    };
    use crate::ethereum::RawEthAddress;
    use crate::wallets::{
        AccountKind, AddressScheme, AddressSourceType, BtcAddress, Label, Network, RawBtcAddress,
        WALLET_LABEL_MAX_LENGTH,
    };
    use chrono::Utc;

    fn test_eth_address() -> EthAddress {
        let raw = RawEthAddress::new("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".to_string());
        EthAddress::parse(&raw).expect("test address should be valid")
    }

    fn second_eth_address() -> EthAddress {
        let raw = RawEthAddress::new("0xde0b295669a9fd93d5f28d9ec85e40f4cb697bae".to_string());
        EthAddress::parse(&raw).expect("second test address should be valid")
    }

    fn test_btc_address() -> BtcAddress {
        let raw = RawBtcAddress::new("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".to_string());
        BtcAddress::parse(&raw, Network::Mainnet).expect("test address should be valid")
    }

    fn second_btc_address() -> BtcAddress {
        let raw = RawBtcAddress::new("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string());
        BtcAddress::parse(&raw, Network::Mainnet).expect("second test address should be valid")
    }

    #[test]
    fn add_eth_address_creates_new_wallet() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let address = test_eth_address();
        let result =
            create_eth_wallet_account_fixture(user_id, &address, "My ETH Wallet", Utc::now());

        // Verify wallet, account, and address were created
        let wallets = list_wallets(user_id).expect("should list wallets");
        assert_eq!(wallets.len(), 1);
        assert_eq!(wallets[0].wallet.id, result.wallet_id);
    }

    #[test]
    fn add_eth_address_to_existing_wallet() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        // First, create a wallet with one ETH address
        let addr1 = test_eth_address();
        let first = create_eth_wallet_account_fixture(user_id, &addr1, "Multi-Asset", Utc::now());

        // Now add a second ETH address to the same wallet
        let addr2 = second_eth_address();
        let second = add_eth_account_to_existing_wallet_fixture(
            user_id,
            &addr2,
            first.wallet_id,
            Utc::now(),
        );

        assert_eq!(
            second.wallet_id, first.wallet_id,
            "should use the same wallet"
        );
        assert_ne!(
            second.account_id, first.account_id,
            "should create separate accounts"
        );

        // Verify only one wallet exists
        let wallets = list_wallets(user_id).expect("should list wallets");
        assert_eq!(wallets.len(), 1);
    }

    #[test]
    fn add_eth_address_to_nonexistent_wallet_fails() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let address = test_eth_address();
        let fake_wallet_id = WalletId::new();
        let result = add_ethereum_address(
            user_id,
            &address,
            Network::Mainnet,
            Some(&fake_wallet_id),
            None,
            Utc::now(),
        );

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Wallet not found"),
            "error should mention wallet not found, got: {err}"
        );
    }

    #[test]
    fn add_duplicate_eth_address_fails() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let address = test_eth_address();
        create_eth_wallet_account_fixture(user_id, &address, "Duplicate ETH A", Utc::now());

        // Try to add the same address again (even to a different wallet)
        let result = add_ethereum_address(
            user_id,
            &address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Duplicate ETH B")),
            Utc::now(),
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            classify_wallet_db_conflict(&err),
            Some(WalletDbConflict::AddressAlreadyLinked)
        );
    }

    #[test]
    fn add_btc_address_creates_new_wallet() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let address = test_btc_address();
        let label =
            Label::parse_with_limit("My BTC Wallet", WALLET_LABEL_MAX_LENGTH).expect("valid");
        let result = add_bitcoin_address(
            user_id,
            &address,
            Network::Mainnet,
            None,
            Some(&label),
            Utc::now(),
        )
        .expect("should create wallet");

        let wallets = list_wallets(user_id).expect("should list wallets");
        assert_eq!(wallets.len(), 1);
        assert_eq!(wallets[0].wallet.id, result.wallet_id);
    }

    #[test]
    fn add_btc_address_to_existing_wallet() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let addr1 = test_btc_address();
        let label =
            Label::parse_with_limit("Multi-Address", WALLET_LABEL_MAX_LENGTH).expect("valid");
        let first = add_bitcoin_address(
            user_id,
            &addr1,
            Network::Mainnet,
            None,
            Some(&label),
            Utc::now(),
        )
        .expect("should create first address");

        let addr2 = second_btc_address();
        let second = add_bitcoin_address(
            user_id,
            &addr2,
            Network::Mainnet,
            Some(&first.wallet_id),
            None,
            Utc::now(),
        )
        .expect("should add to existing wallet");

        assert_eq!(
            second.wallet_id, first.wallet_id,
            "should use the same wallet"
        );
        assert_ne!(
            second.account_id, first.account_id,
            "should create separate accounts"
        );

        let wallets = list_wallets(user_id).expect("should list wallets");
        assert_eq!(wallets.len(), 1);
        assert_eq!(wallets[0].accounts.len(), 2);
    }

    #[test]
    fn add_btc_address_to_nonexistent_wallet_fails() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let address = test_btc_address();
        let fake_wallet_id = WalletId::new();
        let result = add_bitcoin_address(
            user_id,
            &address,
            Network::Mainnet,
            Some(&fake_wallet_id),
            None,
            Utc::now(),
        );

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Wallet not found"),
            "error should mention wallet not found, got: {err}"
        );
    }

    #[test]
    fn add_duplicate_btc_address_fails() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let address = test_btc_address();
        add_bitcoin_address(
            user_id,
            &address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Duplicate BTC A")),
            Utc::now(),
        )
        .expect("first add should succeed");

        let result = add_bitcoin_address(
            user_id,
            &address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Duplicate BTC B")),
            Utc::now(),
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            classify_wallet_db_conflict(&err),
            Some(WalletDbConflict::AddressAlreadyLinked)
        );
    }

    #[test]
    fn add_btc_address_stores_correct_scheme() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        // Add a native segwit address
        let raw = RawBtcAddress::new("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".to_string());
        let address = BtcAddress::parse(&raw, Network::Mainnet).expect("should parse");
        let result = add_bitcoin_address(
            user_id,
            &address,
            Network::Mainnet,
            None,
            Some(&wallet_label("Scheme Wallet")),
            Utc::now(),
        )
        .expect("should create wallet");

        let wallets = list_wallets(user_id).expect("should list wallets");
        assert_eq!(wallets.len(), 1);
        assert_eq!(wallets[0].accounts.len(), 1);
        assert_eq!(
            wallets[0].accounts[0].account_kind,
            AccountKind::SingleAddress
        );
        assert_eq!(wallets[0].accounts[0].addresses.len(), 1);
        assert_eq!(
            wallets[0].accounts[0].addresses[0].address_scheme,
            AddressScheme::NativeSegwit
        );
        assert_eq!(
            wallets[0].accounts[0].addresses[0].source_type,
            AddressSourceType::UserProvided
        );
        assert_eq!(wallets[0].accounts[0].addresses[0].id, result.address_id);
    }
}
