use crate::db::error::DbError;
use crate::models::parse_datetime;
use crate::wallets::{
    ACCOUNT_LABEL_MAX_LENGTH, AccountIndex, AccountKind, AccountWithHdKeys, AddressScheme,
    AddressSourceType, DerivationCoinType, DerivationPath, DerivationPurpose,
    DigitalAssetAccountId, DigitalAssetAddressId, DigitalAssetAddressRecord, HdKeyId, HdKeyRecord,
    IdentitySource, KeyRole, KeySource, Label, Network, SyncedAssetId, ValidatedExtendedPubkey,
    ValidatedMasterFingerprint, WALLET_LABEL_MAX_LENGTH, WalletAccessorId, WalletAccessorSummary,
    WalletId, WalletSummary,
};
use std::str::FromStr;

pub(super) fn parse_wallet_row(row: &rusqlite::Row<'_>) -> Result<WalletSummary, rusqlite::Error> {
    let id: String = row.get(0)?;
    let master_fingerprint: Option<String> = row.get(1)?;
    let identity_source: String = row.get(2)?;
    let verified_at: Option<String> = row.get(3)?;
    let label: String = row.get(4)?;
    let created_at: String = row.get(5)?;
    let updated_at: String = row.get(6)?;

    let id = WalletId::from_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let master_fingerprint = match master_fingerprint {
        Some(value) => Some(ValidatedMasterFingerprint::parse(&value).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?),
        None => None,
    };

    let identity_source = IdentitySource::from_str(&identity_source).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown identity source")),
        )
    })?;

    let verified_at = match verified_at {
        Some(value) => Some(parse_datetime(&value).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?),
        None => None,
    };

    let label = Label::parse_with_limit(&label, WALLET_LABEL_MAX_LENGTH).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let created_at = parse_datetime(&created_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let updated_at = parse_datetime(&updated_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(WalletSummary {
        id,
        master_fingerprint,
        identity_source,
        verified_at,
        label,
        created_at,
        updated_at,
    })
}

pub(super) fn parse_accessor_row(
    row: &rusqlite::Row<'_>,
) -> Result<WalletAccessorSummary, rusqlite::Error> {
    let id: String = row.get(0)?;
    let accessor_kind: String = row.get(1)?;
    let accessor_label: Option<String> = row.get(2)?;
    let device_id_hash: Option<String> = row.get(3)?;
    let device_model: Option<String> = row.get(4)?;
    let accessor_version: Option<String> = row.get(5)?;
    let firmware_version: Option<String> = row.get(6)?;
    let created_at: String = row.get(7)?;
    let updated_at: String = row.get(8)?;

    let id = WalletAccessorId::from_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let accessor_kind =
        crate::wallets::AccessorKind::from_str(&accessor_kind).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(DbError::new("Unknown accessor kind")),
            )
        })?;

    let accessor_label = match accessor_label {
        Some(value) => Some(
            Label::parse_with_limit(&value, WALLET_LABEL_MAX_LENGTH).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
        ),
        None => None,
    };

    let created_at = parse_datetime(&created_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let updated_at = parse_datetime(&updated_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(WalletAccessorSummary {
        id,
        accessor_kind,
        accessor_label,
        device_id_hash,
        device_model,
        accessor_version,
        firmware_version,
        created_at,
        updated_at,
    })
}

pub(super) fn parse_summary_accessor_row(
    row: &rusqlite::Row<'_>,
) -> Result<WalletAccessorSummary, rusqlite::Error> {
    let id: String = row.get(1)?;
    let accessor_kind: String = row.get(2)?;
    let accessor_label: Option<String> = row.get(3)?;
    let device_id_hash: Option<String> = row.get(4)?;
    let device_model: Option<String> = row.get(5)?;
    let accessor_version: Option<String> = row.get(6)?;
    let firmware_version: Option<String> = row.get(7)?;
    let created_at: String = row.get(8)?;
    let updated_at: String = row.get(9)?;

    let id = WalletAccessorId::from_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let accessor_kind =
        crate::wallets::AccessorKind::from_str(&accessor_kind).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(DbError::new("Unknown accessor kind")),
            )
        })?;

    let accessor_label = match accessor_label {
        Some(value) => Some(
            Label::parse_with_limit(&value, WALLET_LABEL_MAX_LENGTH).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
        ),
        None => None,
    };

    let created_at = parse_datetime(&created_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let updated_at = parse_datetime(&updated_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(WalletAccessorSummary {
        id,
        accessor_kind,
        accessor_label,
        device_id_hash,
        device_model,
        accessor_version,
        firmware_version,
        created_at,
        updated_at,
    })
}

pub(super) fn parse_account_row(
    row: &rusqlite::Row<'_>,
) -> Result<AccountWithHdKeys, rusqlite::Error> {
    use crate::asset_capabilities::account_model_for;

    let id: String = row.get(0)?;
    let asset_id: String = row.get(1)?;
    let network: String = row.get(2)?;
    let account_kind: String = row.get(3)?;
    let label: String = row.get(4)?;
    let created_at: String = row.get(5)?;
    let updated_at: String = row.get(6)?;

    let id = DigitalAssetAccountId::from_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let asset_id = SyncedAssetId::from_str(&asset_id).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown asset id")),
        )
    })?;

    let network = Network::from_str(&network).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown network")),
        )
    })?;

    let account_model = account_model_for(asset_id);

    let account_kind = AccountKind::from_str(&account_kind).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown account kind")),
        )
    })?;

    let label = Label::parse_with_limit(&label, ACCOUNT_LABEL_MAX_LENGTH).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let created_at = parse_datetime(&created_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let updated_at = parse_datetime(&updated_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(AccountWithHdKeys {
        id,
        asset_id,
        network,
        account_model,
        account_kind,
        label,
        hd_keys: Vec::new(),
        addresses: Vec::new(),
        created_at,
        updated_at,
    })
}

pub(super) fn parse_summary_account_row(
    row: &rusqlite::Row<'_>,
) -> Result<AccountWithHdKeys, rusqlite::Error> {
    use crate::asset_capabilities::account_model_for;

    let id: String = row.get(1)?;
    let asset_id: String = row.get(2)?;
    let network: String = row.get(3)?;
    let account_kind: String = row.get(4)?;
    let label: String = row.get(5)?;
    let created_at: String = row.get(6)?;
    let updated_at: String = row.get(7)?;

    let id = DigitalAssetAccountId::from_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let asset_id = SyncedAssetId::from_str(&asset_id).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown asset id")),
        )
    })?;

    let network = Network::from_str(&network).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown network")),
        )
    })?;

    let account_model = account_model_for(asset_id);

    let account_kind = AccountKind::from_str(&account_kind).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown account kind")),
        )
    })?;

    let label = Label::parse_with_limit(&label, ACCOUNT_LABEL_MAX_LENGTH).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let created_at = parse_datetime(&created_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let updated_at = parse_datetime(&updated_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(AccountWithHdKeys {
        id,
        asset_id,
        network,
        account_model,
        account_kind,
        label,
        hd_keys: Vec::new(),
        addresses: Vec::new(),
        created_at,
        updated_at,
    })
}

pub(super) fn parse_hd_key_row(row: &rusqlite::Row<'_>) -> Result<HdKeyRecord, rusqlite::Error> {
    let id: String = row.get(0)?;
    let key_role: String = row.get(1)?;
    let key_source: String = row.get(2)?;
    let verified_by_accessor_id: Option<String> = row.get(3)?;
    let address_scheme: String = row.get(4)?;
    let extended_pubkey: String = row.get(5)?;
    let derivation_purpose: i64 = row.get(6)?;
    let derivation_coin_type: i64 = row.get(7)?;
    let derivation_account: i64 = row.get(8)?;
    let created_at: String = row.get(9)?;
    let updated_at: String = row.get(10)?;

    let id = HdKeyId::from_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let key_role = KeyRole::from_str(&key_role).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown key role")),
        )
    })?;

    let key_source = KeySource::from_str(&key_source).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown key source")),
        )
    })?;

    let verified_by_accessor_id = match verified_by_accessor_id {
        Some(value) => Some(WalletAccessorId::from_str(&value).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?),
        None => None,
    };

    let address_scheme = AddressScheme::from_str(&address_scheme).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown address scheme")),
        )
    })?;

    let extended_pubkey = ValidatedExtendedPubkey::parse(address_scheme, &extended_pubkey)
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
        })?;

    if derivation_purpose < 0 {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Integer,
            Box::new(DbError::new("Derivation purpose must be non-negative")),
        ));
    }

    let purpose = DerivationPurpose::from_value(derivation_purpose as u32).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Integer,
            Box::new(DbError::new("Unknown derivation purpose")),
        )
    })?;

    if derivation_coin_type < 0 {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            7,
            rusqlite::types::Type::Integer,
            Box::new(DbError::new("Derivation coin type must be non-negative")),
        ));
    }

    let coin_type = DerivationCoinType::new(derivation_coin_type as u32);
    let account_index = account_index_from_i64(derivation_account).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Integer, Box::new(e))
    })?;

    let derivation_path = DerivationPath {
        purpose,
        coin_type,
        account: account_index,
    };

    let created_at = parse_datetime(&created_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let updated_at = parse_datetime(&updated_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(HdKeyRecord {
        id,
        key_role,
        key_source,
        verified_by_accessor_id,
        address_scheme,
        extended_pubkey,
        derivation_path,
        created_at,
        updated_at,
    })
}

pub(super) fn parse_summary_hd_key_row(
    row: &rusqlite::Row<'_>,
) -> Result<HdKeyRecord, rusqlite::Error> {
    let id: String = row.get(1)?;
    let key_role: String = row.get(2)?;
    let key_source: String = row.get(3)?;
    let verified_by_accessor_id: Option<String> = row.get(4)?;
    let address_scheme: String = row.get(5)?;
    let extended_pubkey: String = row.get(6)?;
    let derivation_purpose: i64 = row.get(7)?;
    let derivation_coin_type: i64 = row.get(8)?;
    let derivation_account: i64 = row.get(9)?;
    let created_at: String = row.get(10)?;
    let updated_at: String = row.get(11)?;

    let id = HdKeyId::from_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let key_role = KeyRole::from_str(&key_role).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown key role")),
        )
    })?;

    let key_source = KeySource::from_str(&key_source).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown key source")),
        )
    })?;

    let verified_by_accessor_id = match verified_by_accessor_id {
        Some(value) => Some(WalletAccessorId::from_str(&value).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
        })?),
        None => None,
    };

    let address_scheme = AddressScheme::from_str(&address_scheme).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown address scheme")),
        )
    })?;

    let extended_pubkey = ValidatedExtendedPubkey::parse(address_scheme, &extended_pubkey)
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
        })?;

    if derivation_purpose < 0 {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            7,
            rusqlite::types::Type::Integer,
            Box::new(DbError::new("Derivation purpose must be non-negative")),
        ));
    }

    let purpose = DerivationPurpose::from_value(derivation_purpose as u32).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            7,
            rusqlite::types::Type::Integer,
            Box::new(DbError::new("Unknown derivation purpose")),
        )
    })?;

    if derivation_coin_type < 0 {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            8,
            rusqlite::types::Type::Integer,
            Box::new(DbError::new("Derivation coin type must be non-negative")),
        ));
    }

    let coin_type = DerivationCoinType::new(derivation_coin_type as u32);
    let account_index = account_index_from_i64(derivation_account).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Integer, Box::new(e))
    })?;

    let derivation_path = DerivationPath {
        purpose,
        coin_type,
        account: account_index,
    };

    let created_at = parse_datetime(&created_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let updated_at = parse_datetime(&updated_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(11, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(HdKeyRecord {
        id,
        key_role,
        key_source,
        verified_by_accessor_id,
        address_scheme,
        extended_pubkey,
        derivation_path,
        created_at,
        updated_at,
    })
}

pub(super) fn parse_address_row(
    row: &rusqlite::Row<'_>,
) -> Result<DigitalAssetAddressRecord, rusqlite::Error> {
    let id: String = row.get(0)?;
    let asset_id: String = row.get(1)?;
    let network: String = row.get(2)?;
    let address: String = row.get(3)?;
    let address_scheme: String = row.get(4)?;
    let derivation_change: Option<i64> = row.get(5)?;
    let derivation_index: Option<i64> = row.get(6)?;
    let source_type: String = row.get(7)?;
    let created_at: String = row.get(8)?;
    let updated_at: String = row.get(9)?;

    let id = DigitalAssetAddressId::from_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let asset_id = SyncedAssetId::from_str(&asset_id).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown asset id")),
        )
    })?;

    let network = Network::from_str(&network).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown network")),
        )
    })?;

    let address_scheme = AddressScheme::from_str(&address_scheme).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown address scheme")),
        )
    })?;

    let derivation_change = parse_optional_u32(derivation_change).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Integer, Box::new(e))
    })?;
    let derivation_index = parse_optional_u32(derivation_index).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Integer, Box::new(e))
    })?;

    let source_type = AddressSourceType::from_str(&source_type).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            7,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown address source type")),
        )
    })?;

    let created_at = parse_datetime(&created_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let updated_at = parse_datetime(&updated_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(DigitalAssetAddressRecord {
        id,
        asset_id,
        network,
        address,
        address_scheme,
        derivation_change,
        derivation_index,
        source_type,
        created_at,
        updated_at,
    })
}

pub(super) fn parse_summary_address_row(
    row: &rusqlite::Row<'_>,
) -> Result<DigitalAssetAddressRecord, rusqlite::Error> {
    let id: String = row.get(1)?;
    let asset_id: String = row.get(2)?;
    let network: String = row.get(3)?;
    let address: String = row.get(4)?;
    let address_scheme: String = row.get(5)?;
    let derivation_change: Option<i64> = row.get(6)?;
    let derivation_index: Option<i64> = row.get(7)?;
    let source_type: String = row.get(8)?;
    let created_at: String = row.get(9)?;
    let updated_at: String = row.get(10)?;

    let id = DigitalAssetAddressId::from_str(&id).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let asset_id = SyncedAssetId::from_str(&asset_id).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown asset id")),
        )
    })?;

    let network = Network::from_str(&network).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown network")),
        )
    })?;

    let address_scheme = AddressScheme::from_str(&address_scheme).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown address scheme")),
        )
    })?;

    let derivation_change = parse_optional_u32(derivation_change).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Integer, Box::new(e))
    })?;
    let derivation_index = parse_optional_u32(derivation_index).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Integer, Box::new(e))
    })?;

    let source_type = AddressSourceType::from_str(&source_type).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            8,
            rusqlite::types::Type::Text,
            Box::new(DbError::new("Unknown address source type")),
        )
    })?;

    let created_at = parse_datetime(&created_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let updated_at = parse_datetime(&updated_at).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(DigitalAssetAddressRecord {
        id,
        asset_id,
        network,
        address,
        address_scheme,
        derivation_change,
        derivation_index,
        source_type,
        created_at,
        updated_at,
    })
}

fn parse_optional_u32(value: Option<i64>) -> Result<Option<u32>, DbError> {
    match value {
        Some(value) if value < 0 => Err(DbError::new("Value must be non-negative")),
        Some(value) => {
            let parsed =
                u32::try_from(value).map_err(|_| DbError::new("Value out of u32 range"))?;
            Ok(Some(parsed))
        }
        None => Ok(None),
    }
}

pub(super) fn account_index_from_i64(value: i64) -> Result<AccountIndex, DbError> {
    if value < 0 {
        return Err(DbError::new("Account index must be non-negative"));
    }

    let as_u32 = u32::try_from(value).map_err(|_| DbError::new("Account index out of range"))?;
    AccountIndex::new(as_u32).map_err(|e| DbError::new(e.to_string()))
}
