use crate::db::error::DbError;
use crate::db::raw_ingestion::ensure_source_connection_for_address_tx;
use crate::db::user_db::with_user_db_mut;
use crate::wallets::{
    AddressScheme, AddressSourceType, DigitalAssetAccountId, DigitalAssetAddressId, KeyRole,
    Network, SyncedAssetId, XPUB_MAINNET_VERSION, YPUB_MAINNET_VERSION, ZPUB_MAINNET_VERSION,
};
use bitcoin::bip32::{ChildNumber, Xpub};
use bitcoin::secp256k1::Secp256k1;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeneratedDerivedAddress {
    pub(crate) address_id: DigitalAssetAddressId,
    pub(crate) address: String,
    pub(crate) derivation_change: u32,
    pub(crate) derivation_index: u32,
}

pub(crate) struct InitialHdAddressBootstrapRequest<'a> {
    pub(crate) account_id: DigitalAssetAccountId,
    pub(crate) asset_id: SyncedAssetId,
    pub(crate) network: Network,
    pub(crate) address_scheme: AddressScheme,
    pub(crate) extended_pubkey: &'a str,
    pub(crate) gap_limit: u32,
    pub(crate) now: DateTime<Utc>,
}

pub(crate) fn bootstrap_initial_hd_account_addresses(
    tx: &rusqlite::Transaction<'_>,
    request: InitialHdAddressBootstrapRequest<'_>,
) -> Result<(), DbError> {
    let last_index = request
        .gap_limit
        .checked_sub(1)
        .ok_or_else(|| DbError::new("HD bootstrap gap limit must be greater than zero"))?;

    generate_derived_addresses_for_hd_key(
        tx,
        request.account_id,
        request.asset_id,
        request.network,
        request.address_scheme,
        request.extended_pubkey,
        0,
        request.gap_limit,
        0,
        request.now,
    )?;
    generate_derived_addresses_for_hd_key(
        tx,
        request.account_id,
        request.asset_id,
        request.network,
        request.address_scheme,
        request.extended_pubkey,
        1,
        request.gap_limit,
        0,
        request.now,
    )?;
    ensure_account_sync_state_initialized(
        tx,
        request.account_id,
        request.gap_limit,
        Some(last_index),
        Some(last_index),
        request.now,
    )
}

pub(crate) fn derive_next_derived_addresses_for_account(
    user_id: crate::models::UserId,
    account_id: DigitalAssetAccountId,
    address_scheme: AddressScheme,
    derivation_change: u32,
    count: u32,
    now: DateTime<Utc>,
) -> Result<Vec<GeneratedDerivedAddress>, DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|e| {
            DbError::new(format!(
                "Failed to start address derivation transaction: {e}"
            ))
        })?;

        let (asset_id, network, extended_pubkey) =
            load_primary_hd_key_for_scheme(&tx, account_id, address_scheme)?;

        let generated = generate_next_derived_addresses(
            &tx,
            NextDerivedAddressesRequest {
                account_id,
                asset_id,
                network,
                address_scheme,
                extended_pubkey: &extended_pubkey,
                derivation_change,
                count,
                now,
            },
        )?;

        tx.commit().map_err(|e| {
            DbError::new(format!(
                "Failed to commit address derivation transaction: {e}"
            ))
        })?;
        Ok(generated)
    })
}

pub(super) fn load_primary_hd_key_for_scheme(
    tx: &rusqlite::Transaction<'_>,
    account_id: DigitalAssetAccountId,
    address_scheme: AddressScheme,
) -> Result<(SyncedAssetId, Network, String), DbError> {
    let (asset_id, network, extended_pubkey) = tx
        .query_row(
            "SELECT a.asset_id, a.network, k.extended_pubkey \
             FROM digital_asset_accounts a \
             JOIN digital_asset_account_hd_keys k ON k.account_id = a.id \
             WHERE a.id = ?1 AND k.key_role = ?2 AND k.address_scheme = ?3 \
             ORDER BY k.created_at ASC \
             LIMIT 1",
            params![
                account_id.to_string(),
                KeyRole::Primary.as_str(),
                address_scheme.as_str()
            ],
            |row| {
                let asset_id: String = row.get(0)?;
                let network: String = row.get(1)?;
                let extended_pubkey: String = row.get(2)?;
                Ok((asset_id, network, extended_pubkey))
            },
        )
        .optional()
        .map_err(|e| DbError::new(format!("Failed to load account hd key for generation: {e}")))?
        .ok_or_else(|| DbError::new("No primary HD key found for account and address scheme"))?;

    let asset_id = SyncedAssetId::from_str(&asset_id)
        .ok_or_else(|| DbError::new("Unsupported asset id for address generation"))?;
    let network = Network::from_str(&network)
        .ok_or_else(|| DbError::new("Unsupported network for address generation"))?;
    Ok((asset_id, network, extended_pubkey))
}

pub(super) fn ensure_account_sync_state_initialized(
    tx: &rusqlite::Transaction<'_>,
    account_id: DigitalAssetAccountId,
    gap_limit: u32,
    last_derived_external_index: Option<u32>,
    last_derived_internal_index: Option<u32>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let timestamp = now.to_rfc3339();
    tx.execute(
        "INSERT INTO account_sync_state
         (id, account_id, last_scanned_height, last_scanned_time, gap_limit, last_derived_external_index, last_derived_internal_index, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(account_id) DO NOTHING",
        params![
            ulid::Ulid::new().to_string(),
            account_id.to_string(),
            Option::<i64>::None,
            Option::<String>::None,
            i64::from(gap_limit),
            last_derived_external_index.map(i64::from),
            last_derived_internal_index.map(i64::from),
            timestamp,
            timestamp,
        ],
    )
    .map_err(|e| DbError::new(format!("Failed to initialize account sync state: {e}")))?;
    Ok(())
}

struct NextDerivedAddressesRequest<'a> {
    account_id: DigitalAssetAccountId,
    asset_id: SyncedAssetId,
    network: Network,
    address_scheme: AddressScheme,
    extended_pubkey: &'a str,
    derivation_change: u32,
    count: u32,
    now: DateTime<Utc>,
}

fn generate_next_derived_addresses(
    tx: &rusqlite::Transaction<'_>,
    request: NextDerivedAddressesRequest<'_>,
) -> Result<Vec<GeneratedDerivedAddress>, DbError> {
    if request.count == 0 {
        return Ok(Vec::new());
    }

    let start_index = next_derivation_index(tx, request.account_id, request.derivation_change)?;
    generate_derived_addresses_for_hd_key(
        tx,
        request.account_id,
        request.asset_id,
        request.network,
        request.address_scheme,
        request.extended_pubkey,
        request.derivation_change,
        request.count,
        start_index,
        request.now,
    )
}

fn next_derivation_index(
    tx: &rusqlite::Transaction<'_>,
    account_id: DigitalAssetAccountId,
    derivation_change: u32,
) -> Result<u32, DbError> {
    let max_index: Option<i64> = tx
        .query_row(
            "SELECT MAX(derivation_index) \
             FROM digital_asset_addresses \
             WHERE account_id = ?1 AND source_type = ?2 AND derivation_change = ?3",
            params![
                account_id.to_string(),
                AddressSourceType::Derived.as_str(),
                i64::from(derivation_change)
            ],
            |row| row.get(0),
        )
        .map_err(|e| DbError::new(format!("Failed to compute next derivation index: {e}")))?;

    match max_index {
        Some(value) if value < 0 => Err(DbError::new(
            "Negative derivation index found in digital_asset_addresses",
        )),
        Some(value) => {
            let max_u32 = u32::try_from(value)
                .map_err(|_| DbError::new("Derivation index out of u32 range"))?;
            max_u32
                .checked_add(1)
                .ok_or_else(|| DbError::new("Derivation index overflow"))
        }
        None => Ok(0),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn generate_derived_addresses_for_hd_key(
    tx: &rusqlite::Transaction<'_>,
    account_id: DigitalAssetAccountId,
    asset_id: SyncedAssetId,
    network: Network,
    address_scheme: AddressScheme,
    extended_pubkey: &str,
    derivation_change: u32,
    count: u32,
    start_index: u32,
    now: DateTime<Utc>,
) -> Result<Vec<GeneratedDerivedAddress>, DbError> {
    if count == 0 {
        return Ok(Vec::new());
    }

    let timestamp = now.to_rfc3339();
    let mut generated = Vec::new();
    for offset in 0..count {
        let derivation_index = start_index
            .checked_add(offset)
            .ok_or_else(|| DbError::new("Derivation index overflow"))?;
        let address = derive_address_from_extended_pubkey(
            asset_id,
            network,
            address_scheme,
            extended_pubkey,
            derivation_change,
            derivation_index,
        )?;
        let address_normalized = address.to_lowercase();

        let address_id = DigitalAssetAddressId::new();
        tx.execute(
            "INSERT INTO digital_asset_addresses \
             (id, account_id, asset_id, network, address, address_normalized, address_scheme, derivation_change, derivation_index, source_type, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                address_id.to_string(),
                account_id.to_string(),
                asset_id.as_str(),
                network.as_str(),
                address.as_str(),
                address_normalized,
                address_scheme.as_str(),
                i64::from(derivation_change),
                i64::from(derivation_index),
                AddressSourceType::Derived.as_str(),
                timestamp,
                timestamp,
            ],
        )
        .map_err(|e| DbError::new(format!("Failed to insert derived address: {e}")))?;
        ensure_source_connection_for_address_tx(
            tx,
            address_id,
            asset_id,
            network,
            &address_normalized,
            now,
        )?;

        generated.push(GeneratedDerivedAddress {
            address_id,
            address,
            derivation_change,
            derivation_index,
        });
    }

    Ok(generated)
}

pub(crate) fn derive_address_from_extended_pubkey(
    asset_id: SyncedAssetId,
    network: Network,
    address_scheme: AddressScheme,
    extended_pubkey: &str,
    derivation_change: u32,
    derivation_index: u32,
) -> Result<String, DbError> {
    if asset_id != SyncedAssetId::Bitcoin {
        return Err(DbError::new(
            "Address derivation only supports bitcoin accounts",
        ));
    }

    let xpub = parse_slip132_extended_pubkey(extended_pubkey)?;
    let secp = Secp256k1::verification_only();
    let change_key = xpub
        .derive_pub(
            &secp,
            &[ChildNumber::Normal {
                index: derivation_change,
            }],
        )
        .map_err(|e| DbError::new(format!("Failed to derive change key: {e}")))?;
    let derived_key = change_key
        .derive_pub(
            &secp,
            &[ChildNumber::Normal {
                index: derivation_index,
            }],
        )
        .map_err(|e| DbError::new(format!("Failed to derive child key: {e}")))?;

    let network = to_bitcoin_network(network);
    let compressed_public_key = bitcoin::CompressedPublicKey(derived_key.public_key);
    let address = match address_scheme {
        AddressScheme::Legacy => {
            let public_key = bitcoin::PublicKey::new(derived_key.public_key);
            bitcoin::Address::p2pkh(public_key, network)
        }
        AddressScheme::NestedSegwit => bitcoin::Address::p2shwpkh(&compressed_public_key, network),
        AddressScheme::NativeSegwit => bitcoin::Address::p2wpkh(&compressed_public_key, network),
        AddressScheme::Taproot => {
            return Err(DbError::new(
                "Address derivation for taproot accounts is not supported",
            ));
        }
        AddressScheme::Standard => {
            return Err(DbError::new(
                "Address derivation is not applicable to standard (account-based) address scheme",
            ));
        }
    };

    Ok(address.to_string())
}

fn parse_slip132_extended_pubkey(extended_pubkey: &str) -> Result<Xpub, DbError> {
    let normalized = if extended_pubkey.starts_with("zpub") || extended_pubkey.starts_with("ypub") {
        convert_extended_pubkey_version(extended_pubkey, XPUB_MAINNET_VERSION)?
    } else if extended_pubkey.starts_with("xpub") {
        extended_pubkey.to_string()
    } else {
        return Err(DbError::new(
            "Only xpub/ypub/zpub extended public keys are currently supported",
        ));
    };

    Xpub::from_str(&normalized).map_err(|e| DbError::new(format!("Invalid extended pubkey: {e}")))
}

fn convert_extended_pubkey_version(
    input: &str,
    target_version: [u8; 4],
) -> Result<String, DbError> {
    let mut data = bs58::decode(input)
        .with_check(None)
        .into_vec()
        .map_err(|e| DbError::new(format!("Invalid base58check extended pubkey: {e}")))?;

    if data.len() < 4 {
        return Err(DbError::new("Extended pubkey payload is too short"));
    }

    if input.starts_with("xpub") && data[0..4] != XPUB_MAINNET_VERSION {
        return Err(DbError::new("Invalid xpub version bytes"));
    }

    if input.starts_with("ypub") && data[0..4] != YPUB_MAINNET_VERSION {
        return Err(DbError::new("Invalid ypub version bytes"));
    }

    if input.starts_with("zpub") && data[0..4] != ZPUB_MAINNET_VERSION {
        return Err(DbError::new("Invalid zpub version bytes"));
    }

    data[0..4].copy_from_slice(&target_version);
    Ok(bs58::encode(data).with_check().into_string())
}

fn to_bitcoin_network(network: Network) -> bitcoin::Network {
    match network {
        Network::Mainnet => bitcoin::Network::Bitcoin,
        Network::Testnet => bitcoin::Network::Testnet,
        Network::Signet => bitcoin::Network::Signet,
        Network::Regtest => bitcoin::Network::Regtest,
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use bitcoin::Network as BitcoinNetwork;
    use bitcoin::bip32::{DerivationPath as BitcoinDerivationPath, Xpriv, Xpub};
    use bitcoin::secp256k1::Secp256k1;

    fn test_account_xpub(account: u32) -> String {
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

    #[test]
    fn test_same_xpub_derives_different_addresses_per_scheme() {
        let xpub = test_account_xpub(0);

        let legacy = derive_address_from_extended_pubkey(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            AddressScheme::Legacy,
            &xpub,
            0,
            0,
        )
        .expect("legacy derivation should succeed");

        let nested = derive_address_from_extended_pubkey(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            AddressScheme::NestedSegwit,
            &xpub,
            0,
            0,
        )
        .expect("nested segwit derivation should succeed");

        let native = derive_address_from_extended_pubkey(
            SyncedAssetId::Bitcoin,
            Network::Mainnet,
            AddressScheme::NativeSegwit,
            &xpub,
            0,
            0,
        )
        .expect("native segwit derivation should succeed");

        assert!(legacy.starts_with('1'), "legacy should start with 1");
        assert!(nested.starts_with('3'), "nested segwit should start with 3");
        assert!(
            native.starts_with("bc1q"),
            "native segwit should start with bc1q"
        );
        assert_ne!(legacy, nested);
        assert_ne!(legacy, native);
        assert_ne!(nested, native);
    }
}
