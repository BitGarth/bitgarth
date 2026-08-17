use crate::db::error::DbError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MoveAccountDbError {
    AccountNotFound,
    TargetWalletNotFound,
    AlreadyInTargetWallet,
    Conflict(WalletDbConflict),
    Internal(String),
}

impl MoveAccountDbError {
    pub(super) fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

impl std::fmt::Display for MoveAccountDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MoveAccountDbError::AccountNotFound => write!(f, "Account not found"),
            MoveAccountDbError::TargetWalletNotFound => write!(f, "Target wallet not found"),
            MoveAccountDbError::AlreadyInTargetWallet => {
                write!(f, "Account is already in target wallet")
            }
            MoveAccountDbError::Conflict(conflict) => {
                write!(f, "Move-account conflict: {conflict:?}")
            }
            MoveAccountDbError::Internal(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for MoveAccountDbError {}

impl From<DbError> for MoveAccountDbError {
    fn from(value: DbError) -> Self {
        if let Some(conflict) = classify_wallet_db_conflict(&value) {
            return Self::Conflict(conflict);
        }
        Self::Internal(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkTrezorDbError {
    MultiWalletAffinityConflict,
    MasterFingerprintConflict,
    Conflict(WalletDbConflict),
    Internal(String),
}

impl LinkTrezorDbError {
    pub(super) fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

impl std::fmt::Display for LinkTrezorDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkTrezorDbError::MultiWalletAffinityConflict => {
                write!(
                    f,
                    "Selected accounts are already linked across multiple wallets"
                )
            }
            LinkTrezorDbError::MasterFingerprintConflict => {
                write!(
                    f,
                    "Selected wallet is linked to a different master fingerprint"
                )
            }
            LinkTrezorDbError::Conflict(conflict) => {
                write!(f, "Link-trezor conflict: {conflict:?}")
            }
            LinkTrezorDbError::Internal(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for LinkTrezorDbError {}

impl From<DbError> for LinkTrezorDbError {
    fn from(value: DbError) -> Self {
        if let Some(conflict) = classify_wallet_db_conflict(&value) {
            return Self::Conflict(conflict);
        }
        Self::Internal(value.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalletDbConflict {
    /// Wallet label key uniqueness (`wallets.label_key`).
    WalletLabel,
    /// Per-wallet account label-key uniqueness (`digital_asset_accounts.wallet_id + label_key`).
    AccountLabelInWallet,
    /// Extended public key uniqueness
    /// (`digital_asset_account_hd_keys.normalized_extended_pubkey + address_scheme`).
    ExtendedPubkey,
    /// Network-scoped normalized address uniqueness (`digital_asset_addresses` unique index).
    AddressAlreadyLinked,
}

pub(super) fn db_error_from_sqlite(context: &str, error: rusqlite::Error) -> DbError {
    DbError::from_rusqlite_error(context, error)
}

fn classify_wallet_db_conflict_sqlite_failure(
    failure: &crate::db::error::SqliteFailureInfo,
) -> Option<WalletDbConflict> {
    if failure.code != rusqlite::ErrorCode::ConstraintViolation {
        return None;
    }

    let message = failure.message.as_ref()?;
    let normalized = message.to_ascii_lowercase();

    if normalized.contains("wallets.label_key") || normalized.contains("idx_wallets_label_key") {
        return Some(WalletDbConflict::WalletLabel);
    }

    if normalized.contains("digital_asset_accounts.wallet_id, digital_asset_accounts.label_key")
        || normalized.contains("idx_daa_label_key")
        || normalized.contains("manual_asset_accounts.wallet_id, manual_asset_accounts.label_key")
        || normalized.contains("idx_maa_label_key")
    {
        return Some(WalletDbConflict::AccountLabelInWallet);
    }

    if normalized.contains(
        "digital_asset_account_hd_keys.normalized_extended_pubkey, digital_asset_account_hd_keys.address_scheme",
    ) || normalized.contains("idx_daa_hd_normalized_scheme")
    {
        return Some(WalletDbConflict::ExtendedPubkey);
    }

    if normalized.contains(
        "digital_asset_addresses.asset_id, digital_asset_addresses.network, digital_asset_addresses.address_normalized",
    ) || normalized.contains("idx_addresses_unique")
    {
        return Some(WalletDbConflict::AddressAlreadyLinked);
    }

    None
}

pub(crate) fn classify_wallet_db_conflict(error: &DbError) -> Option<WalletDbConflict> {
    // Contract: this classifier is the only DB-level source of wallet conflict kinds.
    // Backend mapping keeps HTTP semantics explicit:
    // - `WalletDbConflict::*` => `409 Conflict` with endpoint-local field keys
    // - unknown DB failures => safe `500 Internal` envelope
    error
        .sqlite_failure()
        .and_then(classify_wallet_db_conflict_sqlite_failure)
        .or_else(|| {
            let normalized = error.to_string().to_ascii_lowercase();
            if normalized.contains("idx_daa_label_key")
                || normalized.contains("idx_maa_label_key")
                || normalized.contains("account label already exists in this wallet")
            {
                Some(WalletDbConflict::AccountLabelInWallet)
            } else {
                None
            }
        })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::{
        add_eth_account_to_existing_wallet_fixture, create_eth_wallet_account_fixture,
        setup_test_user, unique_user_id, wallet_label,
    };
    use crate::wallets::{ACCOUNT_LABEL_MAX_LENGTH, AddressScheme, Label, ValidatedExtendedPubkey};
    use chrono::Utc;

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

    #[test]
    fn wallet_label_conflict_is_classified() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        create_eth_wallet_account_fixture(user_id, &test_eth_address(), "Main Wallet", Utc::now());

        let err = crate::db::wallets::single_address::add_ethereum_address(
            user_id,
            &second_eth_address(),
            crate::wallets::Network::Mainnet,
            None,
            Some(&wallet_label("  main   wallet ")),
            Utc::now(),
        )
        .expect_err("duplicate wallet label should fail");

        assert_eq!(
            classify_wallet_db_conflict(&err),
            Some(WalletDbConflict::WalletLabel)
        );
    }

    #[test]
    fn account_label_conflict_in_wallet_is_classified() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let first = create_eth_wallet_account_fixture(
            user_id,
            &test_eth_address(),
            "Label Conflict Wallet",
            Utc::now(),
        );
        let second = add_eth_account_to_existing_wallet_fixture(
            user_id,
            &second_eth_address(),
            first.wallet_id,
            Utc::now(),
        );

        let err = crate::db::wallets::labels::update_account_label(
            user_id,
            second.account_id,
            Label::parse_with_limit("Ethereum Account 1", ACCOUNT_LABEL_MAX_LENGTH)
                .expect("label should parse"),
            Utc::now(),
        )
        .expect_err("duplicate account label in same wallet should fail");

        assert_eq!(
            classify_wallet_db_conflict(&err),
            Some(WalletDbConflict::AccountLabelInWallet)
        );
    }

    #[test]
    fn extended_pubkey_conflict_is_classified() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let xpub = test_account_xpub(0);
        let validated = ValidatedExtendedPubkey::parse(AddressScheme::NativeSegwit, &xpub)
            .expect("xpub should parse");

        crate::db::wallets::xpub::add_xpub_wallet(
            user_id,
            &validated,
            None,
            Some(&wallet_label("Xpub Wallet A")),
            100,
            Utc::now(),
        )
        .expect("first xpub insert should succeed");

        let err = crate::db::wallets::xpub::add_xpub_wallet(
            user_id,
            &validated,
            None,
            Some(&wallet_label("Xpub Wallet B")),
            100,
            Utc::now(),
        )
        .expect_err("duplicate xpub should fail");

        assert_eq!(
            classify_wallet_db_conflict(&err),
            Some(WalletDbConflict::ExtendedPubkey)
        );
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
