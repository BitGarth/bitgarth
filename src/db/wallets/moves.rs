use super::errors::{MoveAccountDbError, db_error_from_sqlite};
use crate::db::user_db::with_user_db_mut;
use crate::db::wallet_accounts::query_wallet_account_label_keys_in_tx;
use crate::wallets::{
    ACCOUNT_LABEL_MAX_LENGTH, IdentitySource, Label, WalletAccountId, WalletId,
    generate_move_account_label,
};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WalletAccountStorageKind {
    Native,
    Manual,
}

#[derive(Debug, Clone)]
pub(super) struct WalletAccountContext {
    pub(super) kind: WalletAccountStorageKind,
    pub(super) current_wallet_id: WalletId,
    pub(super) account_label: Label,
    pub(super) source_wallet_label: Label,
}

pub(super) fn load_wallet_account_context_in_tx(
    tx: &rusqlite::Transaction<'_>,
    account_id: WalletAccountId,
) -> Result<WalletAccountContext, MoveAccountDbError> {
    let row = tx
        .query_row(
            "SELECT account_kind, wallet_id, account_label, wallet_label
             FROM (
                 SELECT 'native' AS account_kind,
                        a.wallet_id AS wallet_id,
                        a.label AS account_label,
                        w.label AS wallet_label
                 FROM digital_asset_accounts a
                 JOIN wallets w ON w.id = a.wallet_id
                 WHERE a.id = ?1
                 UNION ALL
                 SELECT 'manual' AS account_kind,
                        a.wallet_id AS wallet_id,
                        a.label AS account_label,
                        w.label AS wallet_label
                 FROM manual_asset_accounts a
                 JOIN wallets w ON w.id = a.wallet_id
                 WHERE a.id = ?1
             )
             LIMIT 1",
            params![account_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|e| MoveAccountDbError::internal(format!("Failed to load account: {e}")))?;

    let (account_kind_raw, wallet_id_raw, account_label_raw, source_wallet_label_raw) =
        row.ok_or(MoveAccountDbError::AccountNotFound)?;

    let kind = match account_kind_raw.as_str() {
        "native" => WalletAccountStorageKind::Native,
        "manual" => WalletAccountStorageKind::Manual,
        other => {
            return Err(MoveAccountDbError::internal(format!(
                "Invalid wallet account kind in db: {other}"
            )));
        }
    };
    let current_wallet_id = WalletId::from_str(&wallet_id_raw)
        .map_err(|e| MoveAccountDbError::internal(format!("Invalid wallet id in db: {e}")))?;

    let account_label = Label::parse_with_limit(&account_label_raw, ACCOUNT_LABEL_MAX_LENGTH)
        .map_err(|e| MoveAccountDbError::internal(format!("Invalid account label in db: {e}")))?;
    let source_wallet_label = Label::parse_with_limit(
        &source_wallet_label_raw,
        crate::wallets::WALLET_LABEL_MAX_LENGTH,
    )
    .map_err(|e| MoveAccountDbError::internal(format!("Invalid source wallet label in db: {e}")))?;

    Ok(WalletAccountContext {
        kind,
        current_wallet_id,
        source_wallet_label,
        account_label,
    })
}

fn ensure_wallet_exists_in_tx(
    tx: &rusqlite::Transaction<'_>,
    wallet_id: WalletId,
) -> Result<(), MoveAccountDbError> {
    let exists = tx
        .query_row(
            "SELECT 1 FROM wallets WHERE id = ?1 LIMIT 1",
            params![wallet_id.to_string()],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| MoveAccountDbError::internal(format!("Failed to verify wallet: {e}")))?;

    if exists.is_none() {
        return Err(MoveAccountDbError::TargetWalletNotFound);
    }

    Ok(())
}

fn load_wallet_account_move_family_in_tx(
    tx: &rusqlite::Transaction<'_>,
    account_id: WalletAccountId,
) -> Result<Vec<WalletAccountId>, MoveAccountDbError> {
    let account_id_raw = account_id.to_string();
    let mut statement = tx
        .prepare(
            "SELECT DISTINCT sibling_key.account_id
             FROM digital_asset_account_hd_keys self_key
             JOIN digital_asset_account_hd_keys sibling_key
               ON sibling_key.normalized_extended_pubkey = self_key.normalized_extended_pubkey
             JOIN digital_asset_accounts sibling_account
               ON sibling_account.id = sibling_key.account_id
             WHERE self_key.account_id = ?1
             ORDER BY sibling_account.created_at, sibling_key.account_id",
        )
        .map_err(|e| {
            MoveAccountDbError::internal(format!(
                "Failed to prepare shared normalized key family query: {e}"
            ))
        })?;
    let rows = statement
        .query_map(params![account_id_raw], |row| row.get::<_, String>(0))
        .map_err(|e| {
            MoveAccountDbError::internal(format!(
                "Failed to load shared normalized key family: {e}"
            ))
        })?;

    let mut family_account_ids = Vec::new();
    for row in rows {
        let account_id_raw = row.map_err(|e| {
            MoveAccountDbError::internal(format!(
                "Failed to read shared normalized key family row: {e}"
            ))
        })?;
        let account_id = WalletAccountId::from_str(&account_id_raw).map_err(|e| {
            MoveAccountDbError::internal(format!(
                "Invalid account id in shared normalized key family: {e}"
            ))
        })?;
        family_account_ids.push(account_id);
    }

    if family_account_ids.is_empty() {
        return Ok(vec![account_id]);
    }

    Ok(family_account_ids)
}

fn move_wallet_account_family_to_wallet_in_tx(
    tx: &rusqlite::Transaction<'_>,
    account_ids: &[WalletAccountId],
    target_wallet_id: WalletId,
    now: DateTime<Utc>,
) -> Result<(), MoveAccountDbError> {
    let target_wallet_id_raw = target_wallet_id.to_string();
    let timestamp = now.to_rfc3339();
    let mut existing_target_keys = query_wallet_account_label_keys_in_tx(tx, target_wallet_id)
        .map_err(MoveAccountDbError::from)?
        .into_iter()
        .map(|row| {
            let _ = row.account_id;
            row.label_key
        })
        .collect::<Vec<_>>();

    for account_id in account_ids {
        let context = load_wallet_account_context_in_tx(tx, account_id.to_owned())?;
        if context.current_wallet_id == target_wallet_id {
            continue;
        }

        let target_label = generate_move_account_label(
            &context.account_label,
            &context.source_wallet_label,
            &existing_target_keys,
        )
        .map_err(|e| {
            MoveAccountDbError::internal(format!("Failed to generate moved account label: {e}"))
        })?;
        let target_label_key = target_label.key();

        let sql = match context.kind {
            WalletAccountStorageKind::Native => {
                "UPDATE digital_asset_accounts \
                 SET wallet_id = ?1, label = ?2, label_key = ?3, updated_at = ?4 \
                 WHERE id = ?5"
            }
            WalletAccountStorageKind::Manual => {
                "UPDATE manual_asset_accounts \
                 SET wallet_id = ?1, label = ?2, label_key = ?3, updated_at = ?4 \
                 WHERE id = ?5"
            }
        };

        let updated = tx
            .execute(
                sql,
                params![
                    target_wallet_id_raw.as_str(),
                    target_label.as_str(),
                    target_label_key.as_str(),
                    timestamp.as_str(),
                    account_id.to_string()
                ],
            )
            .map_err(|e| {
                MoveAccountDbError::from(db_error_from_sqlite("Failed to move account", e))
            })?;

        if updated == 0 {
            return Err(MoveAccountDbError::AccountNotFound);
        }

        existing_target_keys.push(target_label_key);
    }

    Ok(())
}

pub(crate) fn move_account_to_wallet(
    user_id: crate::models::UserId,
    account_id: impl Into<WalletAccountId>,
    target_wallet_id: WalletId,
    now: DateTime<Utc>,
) -> Result<(), MoveAccountDbError> {
    let account_id = account_id.into();
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|e| {
            MoveAccountDbError::internal(format!("Failed to start account move transaction: {e}"))
        })?;

        let move_context = load_wallet_account_context_in_tx(&tx, account_id)?;
        ensure_wallet_exists_in_tx(&tx, target_wallet_id)?;

        if move_context.current_wallet_id == target_wallet_id {
            return Err(MoveAccountDbError::AlreadyInTargetWallet);
        }

        let family_account_ids = load_wallet_account_move_family_in_tx(&tx, account_id)?;
        move_wallet_account_family_to_wallet_in_tx(
            &tx,
            &family_account_ids,
            target_wallet_id,
            now,
        )?;

        tx.commit().map_err(|e| {
            MoveAccountDbError::internal(format!("Failed to commit account move: {e}"))
        })?;

        Ok(())
    })
}

pub(crate) fn create_wallet_and_move_account(
    user_id: crate::models::UserId,
    account_id: impl Into<WalletAccountId>,
    label: Label,
    now: DateTime<Utc>,
) -> Result<WalletId, MoveAccountDbError> {
    let account_id = account_id.into();
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|e| {
            MoveAccountDbError::internal(format!(
                "Failed to start create wallet and move transaction: {e}"
            ))
        })?;

        let family_account_ids = load_wallet_account_move_family_in_tx(&tx, account_id)?;

        let created_wallet_id = WalletId::new();
        let label_key = label.key();
        let timestamp = now.to_rfc3339();
        tx.execute(
            "INSERT INTO wallets \
             (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                created_wallet_id.to_string(),
                label.as_str(),
                label_key.as_str(),
                Option::<String>::None,
                IdentitySource::UserProvided.as_str(),
                Option::<String>::None,
                timestamp,
                timestamp,
            ],
        )
        .map_err(|e| MoveAccountDbError::from(db_error_from_sqlite("Failed to insert wallet", e)))?;

        move_wallet_account_family_to_wallet_in_tx(
            &tx,
            &family_account_ids,
            created_wallet_id,
            now,
        )?;

        tx.commit().map_err(|e| {
            MoveAccountDbError::internal(format!(
                "Failed to commit create wallet and move transaction: {e}"
            ))
        })?;

        Ok(created_wallet_id)
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::DbError;
    use crate::db::user_db::{with_user_db, with_user_db_mut};
    use crate::db::{
        add_eth_account_to_existing_wallet_fixture, create_eth_wallet_account_fixture,
        setup_test_user, unique_user_id, wallet_label,
    };
    use crate::wallets::{
        AccessorKind, AccountKind, AddressScheme, DigitalAssetAccountId, HdKeyId, IdentitySource,
        KeyRole, KeySource, Label, Network, SyncedAssetId, ValidatedExtendedPubkey,
        WALLET_LABEL_MAX_LENGTH, WalletAccessorId, WalletId,
    };
    use chrono::{DateTime, Utc};
    use rusqlite::params;
    use std::str::FromStr;

    fn test_eth_address() -> crate::ethereum::EthAddress {
        let raw = crate::ethereum::RawEthAddress::new(
            "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed".to_string(),
        );
        crate::ethereum::EthAddress::parse(&raw).expect("test address should be valid")
    }

    fn second_eth_address() -> crate::ethereum::EthAddress {
        let raw = crate::ethereum::RawEthAddress::new(
            "0xde0b295669a9fd93d5f28d9ec85e40f4cb697bae".to_string(),
        );
        crate::ethereum::EthAddress::parse(&raw).expect("second test address should be valid")
    }

    fn third_eth_address() -> crate::ethereum::EthAddress {
        let raw = crate::ethereum::RawEthAddress::new(
            "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359".to_string(),
        );
        crate::ethereum::EthAddress::parse(&raw).expect("third test address should be valid")
    }

    fn test_account_xpub(account: u32) -> String {
        use bitcoin::Network as BitcoinNetwork;
        use bitcoin::bip32::{ChildNumber, DerivationPath as BitcoinDerivationPath, Xpriv, Xpub};
        use bitcoin::secp256k1::Secp256k1;

        let secp = Secp256k1::new();
        let mut seed = [0_u8; 32];
        seed[0..4].copy_from_slice(&account.to_be_bytes());

        let master = match Xpriv::new_master(BitcoinNetwork::Bitcoin, &seed) {
            Ok(value) => value,
            Err(err) => panic!("deterministic master key should succeed: {err}"),
        };

        let path = BitcoinDerivationPath::from(vec![
            ChildNumber::Hardened { index: 84 },
            ChildNumber::Hardened { index: 0 },
            ChildNumber::Hardened { index: account },
        ]);

        let account_xpriv = match master.derive_priv(&secp, &path) {
            Ok(value) => value,
            Err(err) => panic!("deterministic account derivation should succeed: {err}"),
        };

        Xpub::from_priv(&secp, &account_xpriv).to_string()
    }

    fn find_account_wallet_and_updated_at(
        wallets: &[crate::wallets::WalletWithDetails],
        account_id: DigitalAssetAccountId,
    ) -> Option<(WalletId, chrono::DateTime<chrono::Utc>)> {
        for wallet in wallets {
            for account in &wallet.accounts {
                if account.id == account_id {
                    return Some((wallet.wallet.id, account.updated_at));
                }
            }
        }
        None
    }

    fn find_account_label(
        wallets: &[crate::wallets::WalletWithDetails],
        account_id: DigitalAssetAccountId,
    ) -> Option<String> {
        for wallet in wallets {
            for account in &wallet.accounts {
                if account.id == account_id {
                    return Some(account.label.as_str().to_string());
                }
            }
        }
        None
    }

    fn load_native_account_wallet_id(
        user_id: crate::models::UserId,
        account_id: DigitalAssetAccountId,
    ) -> WalletId {
        with_user_db(user_id, |conn| -> Result<WalletId, DbError> {
            let wallet_id_raw: String = conn
                .query_row(
                    "SELECT wallet_id FROM digital_asset_accounts WHERE id = ?1",
                    params![account_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to load account wallet id: {e}")))?;
            WalletId::from_str(&wallet_id_raw)
                .map_err(|e| DbError::new(format!("Invalid wallet id in db: {e}")))
        })
        .expect("account wallet id should load")
    }

    fn insert_wallet_and_reassign_account_with_raw_label(
        user_id: crate::models::UserId,
        account_id: DigitalAssetAccountId,
        wallet_id: WalletId,
        wallet_label: &str,
        now: DateTime<Utc>,
    ) {
        let timestamp = now.to_rfc3339();
        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO wallets \
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    wallet_id.to_string(),
                    wallet_label,
                    wallet_label,
                    Option::<String>::None,
                    IdentitySource::UserProvided.as_str(),
                    Option::<String>::None,
                    timestamp,
                    timestamp,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert raw-label wallet: {e}")))?;
            conn.execute(
                "UPDATE digital_asset_accounts SET wallet_id = ?1 WHERE id = ?2",
                params![wallet_id.to_string(), account_id.to_string()],
            )
            .map_err(|e| DbError::new(format!("Failed to reassign account wallet id: {e}")))?;
            Ok(())
        })
        .expect("raw-label wallet fixture should insert");
    }

    #[test]
    fn same_account_label_in_different_wallets_succeeds() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let first = super::super::single_address::add_ethereum_address(
            user_id,
            &test_eth_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Wallet One")),
            Utc::now(),
        )
        .expect("first wallet should insert");
        let second = super::super::single_address::add_ethereum_address(
            user_id,
            &second_eth_address(),
            Network::Mainnet,
            None,
            Some(&wallet_label("Wallet Two")),
            Utc::now(),
        )
        .expect("second wallet should insert");

        let shared = Label::parse_with_limit("Shared Account", ACCOUNT_LABEL_MAX_LENGTH)
            .expect("shared label should parse");
        super::super::labels::update_account_label(
            user_id,
            first.account_id,
            shared.clone(),
            Utc::now(),
        )
        .expect("first account label update should succeed");
        super::super::labels::update_account_label(user_id, second.account_id, shared, Utc::now())
            .expect("second account label update should succeed");
    }

    #[test]
    fn move_account_to_wallet_happy_path_updates_wallet_id_and_updated_at() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let source = super::super::single_address::add_ethereum_address(
            user_id,
            &test_eth_address(),
            Network::Mainnet,
            None,
            Some(&Label::parse_with_limit("Move Source", WALLET_LABEL_MAX_LENGTH).expect("valid")),
            Utc::now(),
        )
        .expect("source account should be created");

        let target = super::super::single_address::add_ethereum_address(
            user_id,
            &second_eth_address(),
            Network::Mainnet,
            None,
            Some(&Label::parse_with_limit("Move Target", WALLET_LABEL_MAX_LENGTH).expect("valid")),
            Utc::now(),
        )
        .expect("target wallet should be created");

        let before_wallets =
            super::super::loaders::list_wallets(user_id).expect("wallets should load before move");
        let (_before_wallet_id, before_updated_at) =
            find_account_wallet_and_updated_at(&before_wallets, source.account_id)
                .expect("source account should exist before move");

        let move_time = before_updated_at + chrono::Duration::minutes(5);
        move_account_to_wallet(user_id, source.account_id, target.wallet_id, move_time)
            .expect("move should succeed");

        let after_wallets =
            super::super::loaders::list_wallets(user_id).expect("wallets should load after move");
        let (after_wallet_id, after_updated_at) =
            find_account_wallet_and_updated_at(&after_wallets, source.account_id)
                .expect("source account should exist after move");

        assert_eq!(after_wallet_id, target.wallet_id);
        assert_eq!(after_updated_at, move_time);
    }

    #[test]
    fn move_account_to_wallet_conflicting_label_auto_renames() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let source = create_eth_wallet_account_fixture(
            user_id,
            &test_eth_address(),
            "Move Source",
            Utc::now(),
        );
        let target = create_eth_wallet_account_fixture(
            user_id,
            &second_eth_address(),
            "Move Target",
            Utc::now(),
        );

        move_account_to_wallet(user_id, source.account_id, target.wallet_id, Utc::now())
            .expect("move should succeed");

        let wallets = super::super::loaders::list_wallets(user_id).expect("wallets should load");
        let moved_label = find_account_label(&wallets, source.account_id)
            .expect("moved account label should exist");
        assert_eq!(
            moved_label,
            "Ethereum Account 1 moved from wallet Move Source"
        );
    }

    #[test]
    fn move_account_to_wallet_conflicting_label_uses_numeric_suffix() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let source = create_eth_wallet_account_fixture(
            user_id,
            &test_eth_address(),
            "Move Source",
            Utc::now(),
        );
        let target = create_eth_wallet_account_fixture(
            user_id,
            &second_eth_address(),
            "Move Target",
            Utc::now(),
        );
        let existing_renamed = add_eth_account_to_existing_wallet_fixture(
            user_id,
            &third_eth_address(),
            target.wallet_id,
            Utc::now(),
        );
        super::super::labels::update_account_label(
            user_id,
            existing_renamed.account_id,
            Label::parse_with_limit(
                "Ethereum Account 1 moved from wallet Move Source",
                ACCOUNT_LABEL_MAX_LENGTH,
            )
            .expect("label should parse"),
            Utc::now(),
        )
        .expect("existing renamed label should be set");

        move_account_to_wallet(user_id, source.account_id, target.wallet_id, Utc::now())
            .expect("move should succeed");

        let wallets = super::super::loaders::list_wallets(user_id).expect("wallets should load");
        let moved_label = find_account_label(&wallets, source.account_id)
            .expect("moved account label should exist");
        assert_eq!(
            moved_label,
            "Ethereum Account 1 moved from wallet Move Source (2)"
        );
    }

    #[test]
    fn move_account_to_wallet_account_not_found_returns_error() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let target = create_eth_wallet_account_fixture(
            user_id,
            &test_eth_address(),
            "Move Target",
            Utc::now(),
        );

        let result = move_account_to_wallet(
            user_id,
            DigitalAssetAccountId::new(),
            target.wallet_id,
            Utc::now(),
        );

        assert!(matches!(result, Err(MoveAccountDbError::AccountNotFound)));
    }

    #[test]
    fn move_account_to_wallet_target_wallet_not_found_returns_error() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let source = create_eth_wallet_account_fixture(
            user_id,
            &test_eth_address(),
            "Move Source",
            Utc::now(),
        );

        let result =
            move_account_to_wallet(user_id, source.account_id, WalletId::new(), Utc::now());

        assert!(matches!(
            result,
            Err(MoveAccountDbError::TargetWalletNotFound)
        ));
    }

    #[test]
    fn move_account_to_wallet_same_wallet_returns_error() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let source = create_eth_wallet_account_fixture(
            user_id,
            &test_eth_address(),
            "Same Wallet Source",
            Utc::now(),
        );

        let result =
            move_account_to_wallet(user_id, source.account_id, source.wallet_id, Utc::now());

        assert!(matches!(
            result,
            Err(MoveAccountDbError::AlreadyInTargetWallet)
        ));
    }

    #[test]
    fn move_account_to_wallet_shared_normalized_key_family_moves_all_siblings_together() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let xpub = test_account_xpub(0);
        let legacy = ValidatedExtendedPubkey::parse(AddressScheme::Legacy, &xpub)
            .expect("legacy xpub should parse");
        let native = ValidatedExtendedPubkey::parse(AddressScheme::NativeSegwit, &xpub)
            .expect("native xpub should parse");

        let first = super::super::xpub::add_xpub_wallet(
            user_id,
            &legacy,
            None,
            Some(&wallet_label("Shared-Key Source Wallet")),
            100,
            Utc::now(),
        )
        .expect("first shared-key account should insert");
        let second = super::super::xpub::add_xpub_wallet(
            user_id,
            &native,
            Some(first.wallet_id),
            None,
            100,
            Utc::now(),
        )
        .expect("second shared-key account should insert");

        let target = create_eth_wallet_account_fixture(
            user_id,
            &test_eth_address(),
            "Shared-Key Target Wallet",
            Utc::now(),
        );

        let move_time = Utc::now() + chrono::Duration::minutes(5);
        move_account_to_wallet(user_id, first.account_id, target.wallet_id, move_time)
            .expect("moving one sibling should move the full family");

        let wallets = super::super::loaders::list_wallets(user_id)
            .expect("wallets should load after family move");
        let (first_wallet_id, first_updated_at) =
            find_account_wallet_and_updated_at(&wallets, first.account_id)
                .expect("first sibling should still exist");
        let (second_wallet_id, second_updated_at) =
            find_account_wallet_and_updated_at(&wallets, second.account_id)
                .expect("second sibling should still exist");

        assert_eq!(first_wallet_id, target.wallet_id);
        assert_eq!(second_wallet_id, target.wallet_id);
        assert_eq!(first_updated_at, move_time);
        assert_eq!(second_updated_at, move_time);
    }

    #[test]
    fn move_account_to_wallet_single_hd_account_without_siblings_is_allowed() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let xpub = test_account_xpub(4);
        let legacy = ValidatedExtendedPubkey::parse(AddressScheme::Legacy, &xpub)
            .expect("legacy xpub should parse");
        let source = super::super::xpub::add_xpub_wallet(
            user_id,
            &legacy,
            None,
            Some(&wallet_label("Single-Key Source Wallet")),
            100,
            Utc::now(),
        )
        .expect("single shared-key account should insert");

        let target = create_eth_wallet_account_fixture(
            user_id,
            &test_eth_address(),
            "Single-Key Target Wallet",
            Utc::now(),
        );

        move_account_to_wallet(user_id, source.account_id, target.wallet_id, Utc::now())
            .expect("move should succeed when no sibling normalized key accounts exist");
    }

    #[test]
    fn move_account_to_wallet_shared_normalized_key_family_resolves_conflicts_per_sibling() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let xpub = test_account_xpub(8);
        let legacy = ValidatedExtendedPubkey::parse(AddressScheme::Legacy, &xpub)
            .expect("legacy xpub should parse");
        let nested = ValidatedExtendedPubkey::parse(AddressScheme::NestedSegwit, &xpub)
            .expect("nested xpub should parse");

        let first = super::super::xpub::add_xpub_wallet(
            user_id,
            &legacy,
            None,
            Some(&wallet_label("Shared-Key Source For Create+Move")),
            100,
            Utc::now(),
        )
        .expect("first shared-key account should insert");
        let second = super::super::xpub::add_xpub_wallet(
            user_id,
            &nested,
            Some(first.wallet_id),
            None,
            100,
            Utc::now(),
        )
        .expect("second shared-key account should insert");

        let target = create_eth_wallet_account_fixture(
            user_id,
            &test_eth_address(),
            "Shared-Key Target Wallet",
            Utc::now(),
        );
        let target_second = add_eth_account_to_existing_wallet_fixture(
            user_id,
            &second_eth_address(),
            target.wallet_id,
            Utc::now(),
        );
        let target_third = add_eth_account_to_existing_wallet_fixture(
            user_id,
            &third_eth_address(),
            target.wallet_id,
            Utc::now(),
        );

        super::super::labels::update_account_label(
            user_id,
            target.account_id,
            Label::parse_with_limit("Bitcoin Account 1", ACCOUNT_LABEL_MAX_LENGTH)
                .expect("conflicting label should parse"),
            Utc::now(),
        )
        .expect("target label should update");
        super::super::labels::update_account_label(
            user_id,
            target_second.account_id,
            Label::parse_with_limit("Bitcoin Account 2", ACCOUNT_LABEL_MAX_LENGTH)
                .expect("conflicting label should parse"),
            Utc::now(),
        )
        .expect("target label should update");
        super::super::labels::update_account_label(
            user_id,
            target_third.account_id,
            Label::parse_with_limit(
                "Bitcoin Account 1 moved from wallet Shared-Key Source For Create+Move",
                ACCOUNT_LABEL_MAX_LENGTH,
            )
            .expect("conflicting renamed label should parse"),
            Utc::now(),
        )
        .expect("target renamed label should update");

        move_account_to_wallet(user_id, first.account_id, target.wallet_id, Utc::now())
            .expect("family move should succeed with per-sibling relabeling");

        let wallets = super::super::loaders::list_wallets(user_id)
            .expect("wallets should load after family move");
        assert_eq!(
            find_account_label(&wallets, first.account_id).as_deref(),
            Some("Bitcoin Account 1 moved from wallet Shared-Key Source For Create+Move (2)")
        );
        assert_eq!(
            find_account_label(&wallets, second.account_id).as_deref(),
            Some("Bitcoin Account 2 moved from wallet Shared-Key Source For Create+Move")
        );
    }

    #[test]
    fn create_wallet_and_move_account_shared_normalized_key_family_moves_all_siblings_together() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let xpub = test_account_xpub(10);
        let legacy = ValidatedExtendedPubkey::parse(AddressScheme::Legacy, &xpub)
            .expect("legacy xpub should parse");
        let nested = ValidatedExtendedPubkey::parse(AddressScheme::NestedSegwit, &xpub)
            .expect("nested xpub should parse");

        let first = super::super::xpub::add_xpub_wallet(
            user_id,
            &legacy,
            None,
            Some(&wallet_label("Shared-Key Source For Create+Move")),
            100,
            Utc::now(),
        )
        .expect("first shared-key account should insert");
        let second = super::super::xpub::add_xpub_wallet(
            user_id,
            &nested,
            Some(first.wallet_id),
            None,
            100,
            Utc::now(),
        )
        .expect("second shared-key account should insert");

        let move_time = Utc::now() + chrono::Duration::minutes(5);
        let destination_label =
            Label::parse_with_limit("Family Destination Wallet", WALLET_LABEL_MAX_LENGTH)
                .expect("destination label should parse");
        let created_wallet_id =
            create_wallet_and_move_account(user_id, first.account_id, destination_label, move_time)
                .expect("family create+move should succeed");

        let wallets = super::super::loaders::list_wallets(user_id)
            .expect("wallets should load after family create+move");
        let (first_wallet_id, first_updated_at) =
            find_account_wallet_and_updated_at(&wallets, first.account_id)
                .expect("first sibling should still exist");
        let (second_wallet_id, second_updated_at) =
            find_account_wallet_and_updated_at(&wallets, second.account_id)
                .expect("second sibling should still exist");

        assert_eq!(first_wallet_id, created_wallet_id);
        assert_eq!(second_wallet_id, created_wallet_id);
        assert_eq!(first_updated_at, move_time);
        assert_eq!(second_updated_at, move_time);
    }

    #[test]
    fn create_wallet_and_move_account_creates_wallet_and_moves_account_atomically() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let source = create_eth_wallet_account_fixture(
            user_id,
            &test_eth_address(),
            "Original Wallet",
            Utc::now(),
        );

        let move_time = Utc::now() + chrono::Duration::minutes(5);
        let new_wallet_label =
            Label::parse_with_limit("Moved Account Wallet", WALLET_LABEL_MAX_LENGTH)
                .expect("valid");
        let created_wallet_id = create_wallet_and_move_account(
            user_id,
            source.account_id,
            new_wallet_label.clone(),
            move_time,
        )
        .expect("wallet create+move should succeed");

        let wallets = super::super::loaders::list_wallets(user_id).expect("wallets should load");
        let (account_wallet_id, account_updated_at) =
            find_account_wallet_and_updated_at(&wallets, source.account_id)
                .expect("source account should still exist");
        assert_eq!(account_wallet_id, created_wallet_id);
        assert_eq!(account_updated_at, move_time);

        let created_wallet = wallets
            .iter()
            .find(|wallet| wallet.wallet.id == created_wallet_id)
            .expect("created wallet should exist");
        assert_eq!(
            created_wallet.wallet.label.as_str(),
            new_wallet_label.as_str()
        );
    }

    #[test]
    fn move_account_to_wallet_shared_normalized_key_family_rolls_back_on_mid_family_failure() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let xpub = test_account_xpub(12);
        let legacy = ValidatedExtendedPubkey::parse(AddressScheme::Legacy, &xpub)
            .expect("legacy xpub should parse");
        let native = ValidatedExtendedPubkey::parse(AddressScheme::NativeSegwit, &xpub)
            .expect("native xpub should parse");

        let first = super::super::xpub::add_xpub_wallet(
            user_id,
            &legacy,
            None,
            Some(&wallet_label("Rollback Source Wallet")),
            100,
            Utc::now(),
        )
        .expect("first shared-key account should insert");
        let second = super::super::xpub::add_xpub_wallet(
            user_id,
            &native,
            Some(first.wallet_id),
            None,
            100,
            Utc::now(),
        )
        .expect("second shared-key account should insert");

        let corrupted_wallet_id = WalletId::new();
        insert_wallet_and_reassign_account_with_raw_label(
            user_id,
            second.account_id,
            corrupted_wallet_id,
            "",
            Utc::now(),
        );

        let target = create_eth_wallet_account_fixture(
            user_id,
            &test_eth_address(),
            "Rollback Target Wallet",
            Utc::now(),
        );

        let result =
            move_account_to_wallet(user_id, first.account_id, target.wallet_id, Utc::now());
        assert!(matches!(result, Err(MoveAccountDbError::Internal(_))));

        assert_eq!(
            load_native_account_wallet_id(user_id, first.account_id),
            first.wallet_id
        );
        assert_eq!(
            load_native_account_wallet_id(user_id, second.account_id),
            corrupted_wallet_id
        );
    }

    #[test]
    fn create_wallet_and_move_account_shared_normalized_key_family_rolls_back_on_failure() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let xpub = test_account_xpub(14);
        let legacy = ValidatedExtendedPubkey::parse(AddressScheme::Legacy, &xpub)
            .expect("legacy xpub should parse");
        let native = ValidatedExtendedPubkey::parse(AddressScheme::NativeSegwit, &xpub)
            .expect("native xpub should parse");

        let first = super::super::xpub::add_xpub_wallet(
            user_id,
            &legacy,
            None,
            Some(&wallet_label("Rollback Create+Move Source")),
            100,
            Utc::now(),
        )
        .expect("first shared-key account should insert");
        let second = super::super::xpub::add_xpub_wallet(
            user_id,
            &native,
            Some(first.wallet_id),
            None,
            100,
            Utc::now(),
        )
        .expect("second shared-key account should insert");

        let corrupted_wallet_id = WalletId::new();
        insert_wallet_and_reassign_account_with_raw_label(
            user_id,
            second.account_id,
            corrupted_wallet_id,
            "",
            Utc::now(),
        );

        let created_label =
            Label::parse_with_limit("Rolled Back Destination", WALLET_LABEL_MAX_LENGTH)
                .expect("destination label should parse");
        let result = create_wallet_and_move_account(
            user_id,
            first.account_id,
            created_label.clone(),
            Utc::now(),
        );
        assert!(matches!(result, Err(MoveAccountDbError::Internal(_))));

        assert_eq!(
            load_native_account_wallet_id(user_id, first.account_id),
            first.wallet_id
        );
        assert_eq!(
            load_native_account_wallet_id(user_id, second.account_id),
            corrupted_wallet_id
        );

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let wallet_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM wallets WHERE label = ?1",
                    params![created_label.as_str()],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    DbError::new(format!("Failed to count rolled-back wallet rows: {e}"))
                })?;
            assert_eq!(wallet_count, 0);
            Ok(())
        })
        .expect("created wallet should be rolled back");
    }

    #[test]
    fn move_account_to_wallet_preserves_verified_by_accessor_id() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let source_wallet_id = WalletId::new();
        let target_wallet_id = WalletId::new();
        let account_id = DigitalAssetAccountId::new();
        let accessor_id = WalletAccessorId::new();
        let hd_key_id = HdKeyId::new();
        let now = Utc::now();
        let timestamp = now.to_rfc3339();
        let xpub = test_account_xpub(0);

        with_user_db_mut(user_id, |conn| -> Result<(), DbError> {
            conn.execute(
                "INSERT INTO wallets \
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    source_wallet_id.to_string(),
                    "Source Hardware Wallet",
                    "source hardware wallet",
                    Option::<String>::None,
                    IdentitySource::DeviceVerified.as_str(),
                    Option::<String>::None,
                    timestamp,
                    timestamp,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert source wallet: {e}")))?;

            conn.execute(
                "INSERT INTO wallets \
                 (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    target_wallet_id.to_string(),
                    "Target Wallet",
                    "target wallet",
                    Option::<String>::None,
                    IdentitySource::UserProvided.as_str(),
                    Option::<String>::None,
                    timestamp,
                    timestamp,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert target wallet: {e}")))?;

            conn.execute(
                "INSERT INTO wallet_accessors \
                 (id, wallet_id, accessor_kind, accessor_label, device_id_hash, device_model, accessor_version, firmware_version, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    accessor_id.to_string(),
                    source_wallet_id.to_string(),
                    AccessorKind::Trezor.as_str(),
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    Option::<String>::None,
                    timestamp,
                    timestamp,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert accessor: {e}")))?;

            conn.execute(
                "INSERT INTO digital_asset_accounts \
                 (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    account_id.to_string(),
                    source_wallet_id.to_string(),
                    "Bitcoin Account 1",
                    "bitcoin account 1",
                    SyncedAssetId::Bitcoin.as_str(),
                    Network::Mainnet.as_str(),
                    AccountKind::HdPubkey.as_str(),
                    timestamp,
                    timestamp,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert account: {e}")))?;

            let normalized_xpub =
                ValidatedExtendedPubkey::parse(AddressScheme::NativeSegwit, &xpub)
                    .map_err(|e| DbError::new(format!("Failed to normalize fixture xpub: {e}")))?
                    .normalized_as_str()
                    .to_string();

            conn.execute(
                "INSERT INTO digital_asset_account_hd_keys \
                 (id, account_id, key_role, extended_pubkey, normalized_extended_pubkey, derivation_purpose, derivation_coin_type, derivation_account, address_scheme, key_source, verified_by_accessor_id, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    hd_key_id.to_string(),
                    account_id.to_string(),
                    KeyRole::Primary.as_str(),
                    xpub,
                    normalized_xpub,
                    84_i64,
                    0_i64,
                    0_i64,
                    AddressScheme::NativeSegwit.as_str(),
                    KeySource::DeviceVerified.as_str(),
                    accessor_id.to_string(),
                    timestamp,
                    timestamp,
                ],
            )
            .map_err(|e| DbError::new(format!("Failed to insert hd key: {e}")))?;

            Ok(())
        })
        .expect("fixture rows should be inserted");

        move_account_to_wallet(
            user_id,
            account_id,
            target_wallet_id,
            now + chrono::Duration::minutes(1),
        )
        .expect("move should succeed");

        with_user_db(user_id, |conn| -> Result<(), DbError> {
            let stored: Option<String> = conn
                .query_row(
                    "SELECT verified_by_accessor_id FROM digital_asset_account_hd_keys WHERE id = ?1",
                    params![hd_key_id.to_string()],
                    |row| row.get(0),
                )
                .map_err(|e| DbError::new(format!("Failed to load hd key verification id: {e}")))?;

            let expected_accessor_id = accessor_id.to_string();
            assert_eq!(stored.as_deref(), Some(expected_accessor_id.as_str()));
            Ok(())
        })
        .expect("verified_by_accessor_id should remain unchanged");
    }
}
