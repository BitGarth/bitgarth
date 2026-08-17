use super::derivation::{InitialHdAddressBootstrapRequest, bootstrap_initial_hd_account_addresses};
use super::errors::db_error_from_sqlite;
use crate::account_limits::AccountActivationState;
use crate::db::account_limits::{
    account_state_for, classify_supported_accounts_in_tx,
    ensure_supported_account_hard_cap_before_insert_in_tx,
};
use crate::db::error::DbError;
use crate::db::user_db::{with_user_db, with_user_db_mut};
use crate::models::UserId;
use crate::wallets::{
    ACCOUNT_LABEL_MAX_LENGTH, AccountIndex, AccountKind, AddressScheme, BIP44_GAP_LIMIT,
    DerivationPath, DigitalAssetAccountId, HdKeyId, IdentitySource, KeyRole, KeySource, Label,
    Network, NormalizedExtendedPubkey, SyncedAssetId, ValidatedExtendedPubkey,
    WALLET_LABEL_MAX_LENGTH, WalletId, generate_unique_account_label,
};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use std::str::FromStr;

pub(crate) fn find_wallet_for_extended_pubkey(
    user_id: UserId,
    extended_pubkey: &str,
) -> Result<Option<(WalletId, String)>, DbError> {
    let normalized = NormalizedExtendedPubkey::parse(extended_pubkey)
        .map_err(|e| DbError::new(format!("Invalid extended pubkey: {e}")))?;

    with_user_db(user_id, |conn| {
        let row = conn
            .query_row(
                "SELECT w.id, w.label \
                 FROM wallets w \
                 JOIN digital_asset_accounts a ON a.wallet_id = w.id \
                 JOIN digital_asset_account_hd_keys k ON k.account_id = a.id \
                 WHERE k.normalized_extended_pubkey = ?1 \
                 LIMIT 1",
                [normalized.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| DbError::new(format!("Failed to find wallet for extended pubkey: {e}")))?;

        match row {
            Some((id_str, label_str)) => {
                let wallet_id = WalletId::from_str(&id_str)
                    .map_err(|e| DbError::new(format!("Invalid wallet id in db: {e}")))?;
                let label_parsed = Label::parse_with_limit(&label_str, WALLET_LABEL_MAX_LENGTH)
                    .map_err(|e| DbError::new(format!("Invalid wallet label in db: {e}")))?;
                let display_label = crate::wallets::display_wallet_label(&label_parsed);
                Ok(Some((wallet_id, display_label)))
            }
            None => Ok(None),
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtendedPubkeySchemeLink {
    pub wallet_id: WalletId,
    pub wallet_label: String,
    pub account_label: String,
}

pub(crate) fn find_extended_pubkey_scheme_link(
    user_id: UserId,
    extended_pubkey: &str,
    address_scheme: AddressScheme,
) -> Result<Option<ExtendedPubkeySchemeLink>, DbError> {
    let normalized = NormalizedExtendedPubkey::parse(extended_pubkey)
        .map_err(|e| DbError::new(format!("Invalid extended pubkey: {e}")))?;

    with_user_db(user_id, |conn| {
        let row = conn
            .query_row(
                "SELECT w.id, w.label, a.label \
                 FROM wallets w \
                 JOIN digital_asset_accounts a ON a.wallet_id = w.id \
                 JOIN digital_asset_account_hd_keys k ON k.account_id = a.id \
                 WHERE k.normalized_extended_pubkey = ?1 AND k.address_scheme = ?2 \
                 LIMIT 1",
                params![normalized.as_str(), address_scheme.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| {
                DbError::new(format!(
                    "Failed to find scheme link for extended pubkey: {e}"
                ))
            })?;

        match row {
            Some((wallet_id_raw, wallet_label_raw, account_label_raw)) => {
                let wallet_id = WalletId::from_str(&wallet_id_raw)
                    .map_err(|e| DbError::new(format!("Invalid wallet id in db: {e}")))?;
                let wallet_label =
                    Label::parse_with_limit(&wallet_label_raw, WALLET_LABEL_MAX_LENGTH)
                        .map_err(|e| DbError::new(format!("Invalid wallet label in db: {e}")))?;
                let account_label =
                    Label::parse_with_limit(&account_label_raw, ACCOUNT_LABEL_MAX_LENGTH)
                        .map_err(|e| DbError::new(format!("Invalid account label in db: {e}")))?;

                Ok(Some(ExtendedPubkeySchemeLink {
                    wallet_id,
                    wallet_label: crate::wallets::display_wallet_label(&wallet_label),
                    account_label: crate::wallets::display_account_label(&account_label),
                }))
            }
            None => Ok(None),
        }
    })
}

#[derive(Debug)]
pub(crate) struct AddXpubDbResult {
    pub wallet_id: WalletId,
    pub account_id: DigitalAssetAccountId,
}

/// Add a user-provided extended public key, creating or linking to a wallet.
///
/// Convenience wrapper used by tests; production callers use
/// [`add_xpub_wallet_with_account_label`] to pass the user-provided name.
#[cfg(all(test, feature = "db-tests"))]
pub(crate) fn add_xpub_wallet(
    user_id: UserId,
    extended_pubkey: &ValidatedExtendedPubkey,
    wallet_id: Option<WalletId>,
    wallet_label: Option<&Label>,
    active_limit: usize,
    now: DateTime<Utc>,
) -> Result<AddXpubDbResult, DbError> {
    add_xpub_wallet_with_account_label(
        user_id,
        extended_pubkey,
        wallet_id,
        wallet_label,
        None,
        active_limit,
        now,
    )
}

/// Like [`add_xpub_wallet`], but with an optional user-provided account name.
/// When `account_label` is `None`, the account is auto-named.
pub(crate) fn add_xpub_wallet_with_account_label(
    user_id: UserId,
    extended_pubkey: &ValidatedExtendedPubkey,
    wallet_id: Option<WalletId>,
    wallet_label: Option<&Label>,
    account_label: Option<&Label>,
    active_limit: usize,
    now: DateTime<Utc>,
) -> Result<AddXpubDbResult, DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn
            .transaction()
            .map_err(|e| db_error_from_sqlite("Failed to start xpub add transaction", e))?;

        let timestamp = now.to_rfc3339();

        // Resolve wallet affinity for this normalized key. If a wallet already
        // contains any scheme variant for the same normalized key, new scheme
        // variants are routed to that wallet.
        let affinity_wallet_id = tx
            .query_row(
                "SELECT w.id \
                 FROM wallets w \
                 JOIN digital_asset_accounts a ON a.wallet_id = w.id \
                 JOIN digital_asset_account_hd_keys k ON k.account_id = a.id \
                 WHERE k.normalized_extended_pubkey = ?1 \
                 LIMIT 1",
                [extended_pubkey.normalized_as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| {
                DbError::new(format!(
                    "Failed to resolve normalized key wallet affinity: {e}"
                ))
            })?;

        // Resolve or create wallet
        let resolved_wallet_id = if let Some(wallet_id_raw) = affinity_wallet_id {
            WalletId::from_str(&wallet_id_raw)
                .map_err(|e| DbError::new(format!("Invalid affinity wallet id in db: {e}")))?
        } else {
            match wallet_id {
                Some(existing_id) => {
                    // Verify the wallet exists for this user
                    let exists = tx
                        .query_row(
                            "SELECT 1 FROM wallets WHERE id = ?1",
                            [existing_id.to_string()],
                            |_| Ok(()),
                        )
                        .optional()
                        .map_err(|e| DbError::new(format!("Failed to verify wallet: {e}")))?;

                    if exists.is_none() {
                        return Err(DbError::new("Wallet not found"));
                    }
                    existing_id
                }
                None => {
                    let new_id = WalletId::new();
                    let effective_label = wallet_label.cloned().ok_or_else(|| {
                        DbError::new("wallet_label is required when creating a wallet")
                    })?;
                    let wl_key = effective_label.key();
                    tx.execute(
                        "INSERT INTO wallets \
                         (id, label, label_key, master_fingerprint, identity_source, verified_at, created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![
                            new_id.to_string(),
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
                    new_id
                }
            }
        };

        // Create account
        let account_id = DigitalAssetAccountId::new();
        let acct_label = super::labels::resolve_new_account_label(
            &tx,
            resolved_wallet_id,
            account_label,
            |keys| {
                generate_unique_account_label(SyncedAssetId::Bitcoin, keys)
                    .map_err(|e| DbError::new(format!("Failed to generate account label: {e}")))
            },
        )?;
        let acct_label_key = acct_label.key();
        ensure_supported_account_hard_cap_before_insert_in_tx(&tx, 1)?;
        tx.execute(
            "INSERT INTO digital_asset_accounts \
             (id, wallet_id, label, label_key, asset_id, network, account_kind, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                account_id.to_string(),
                resolved_wallet_id.to_string(),
                acct_label.as_str(),
                acct_label_key.as_str(),
                SyncedAssetId::Bitcoin.as_str(),
                Network::Mainnet.as_str(),
                AccountKind::HdPubkey.as_str(),
                timestamp,
                timestamp,
            ],
        )
        .map_err(|e| db_error_from_sqlite("Failed to insert account", e))?;

        // Create HD key with default account index 0
        let address_scheme = extended_pubkey.address_scheme();
        let default_account_index = AccountIndex::new(0)
            .map_err(|e| DbError::new(format!("Invalid default account index: {e}")))?;
        let derivation_path =
            DerivationPath::bitcoin_for_address_scheme(default_account_index, address_scheme);

        tx.execute(
            "INSERT INTO digital_asset_account_hd_keys \
             (id, account_id, key_role, extended_pubkey, normalized_extended_pubkey, derivation_purpose, derivation_coin_type, derivation_account, address_scheme, key_source, verified_by_accessor_id, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                HdKeyId::new().to_string(),
                account_id.to_string(),
                KeyRole::Primary.as_str(),
                extended_pubkey.as_str(),
                extended_pubkey.normalized_as_str(),
                derivation_path.purpose.value() as i64,
                derivation_path.coin_type.value() as i64,
                derivation_path.account.as_u32() as i64,
                address_scheme.as_str(),
                KeySource::UserProvided.as_str(),
                Option::<String>::None,
                timestamp,
                timestamp,
            ],
        )
        .map_err(|e| db_error_from_sqlite("Failed to insert hd key", e))?;

        let classified = classify_supported_accounts_in_tx(&tx, active_limit)?;
        if account_state_for(&classified, &account_id.into()) == AccountActivationState::Active {
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
            )?;
        }

        tx.commit()
            .map_err(|e| db_error_from_sqlite("Failed to commit xpub add transaction", e))?;

        Ok(AddXpubDbResult {
            wallet_id: resolved_wallet_id,
            account_id,
        })
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::test_fixtures::{setup_test_user, unique_user_id, wallet_label};
    use crate::db::user_db::with_user_db;
    use crate::wallets::AddressScheme;

    #[test]
    fn inactive_xpub_creation_does_not_bootstrap_derived_addresses() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let xpub = test_account_xpub(0);
        let validated = ValidatedExtendedPubkey::parse(AddressScheme::NativeSegwit, &xpub)
            .expect("xpub should parse");

        let result = add_xpub_wallet(
            user_id,
            &validated,
            None,
            Some(&wallet_label("Inactive Xpub Wallet")),
            0,
            Utc::now(),
        )
        .expect("inactive xpub account should still be created");

        let address_count = with_user_db(user_id, |conn| -> Result<i64, crate::db::DbError> {
            conn.query_row(
                "SELECT COUNT(*) FROM digital_asset_addresses WHERE account_id = ?1",
                [result.account_id.to_string()],
                |row| row.get(0),
            )
            .map_err(|err| crate::db::DbError::new(format!("derived address count failed: {err}")))
        })
        .expect("derived address count should load");

        assert_eq!(address_count, 0);
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
}
