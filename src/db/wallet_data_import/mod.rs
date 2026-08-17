use super::error::DbError;
use super::user_db::with_user_db_mut;
use crate::models::UserId;
use crate::wallets::{AccountKind, BIP44_GAP_LIMIT, DigitalAssetAccountId, KeyRole};
use chrono::{DateTime, Utc};
use std::str::FromStr;

use crate::db::wallets::{
    InitialHdAddressBootstrapRequest, bootstrap_initial_hd_account_addresses,
};

mod merge;
mod parse;
mod resolve;

pub(crate) use parse::WalletDataImportSettings;

const BAD_JSON_MESSAGE: &str = "The selected file is not a valid BitGarth wallet data export.";
const NEWER_VERSION_MESSAGE: &str =
    "This export was created with a newer version of BitGarth. Please update before importing.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WalletDataImportDbError {
    BadRequest(String),
    Validation(String),
    Internal(String),
}

impl std::fmt::Display for WalletDataImportDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(message) => write!(f, "{message}"),
            Self::Validation(message) => write!(f, "{message}"),
            Self::Internal(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for WalletDataImportDbError {}

impl From<DbError> for WalletDataImportDbError {
    fn from(value: DbError) -> Self {
        let message = value.to_string();
        if message.contains("Supported account hard cap exceeded") {
            Self::Validation(message)
        } else {
            Self::Internal(message)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportNativeAccountView {
    pub(crate) wallet_label: String,
    pub(crate) account_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportDuplicateSkipView {
    pub(crate) identifier_kind: String,
    pub(crate) identifier: String,
    pub(crate) wallet_label: String,
    pub(crate) account_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportGlobalDuplicateSkipView {
    pub(crate) identifier_kind: String,
    pub(crate) identifier: String,
    pub(crate) existing_wallet_label: String,
    pub(crate) existing_account_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalletDataImportResult {
    pub(crate) wallets_created: Vec<String>,
    pub(crate) wallets_matched: Vec<String>,
    pub(crate) native_accounts_created: Vec<ImportNativeAccountView>,
    pub(crate) native_accounts_matched: Vec<ImportNativeAccountView>,
    pub(crate) duplicate_skips: Vec<ImportDuplicateSkipView>,
    pub(crate) global_duplicate_skips: Vec<ImportGlobalDuplicateSkipView>,
    pub(crate) assertions_created: u32,
    pub(crate) assertions_skipped: u32,
    pub(crate) validation_warnings: Vec<String>,
}

fn bootstrap_created_hd_account_if_needed(
    tx: &rusqlite::Transaction<'_>,
    account_id: crate::wallets::WalletAccountId,
    account: &parse::ParsedImportedNativeAccount,
    now: DateTime<Utc>,
) -> Result<(), WalletDataImportDbError> {
    if account.account_kind != AccountKind::HdPubkey {
        return Ok(());
    }

    let primary_hd_key = account
        .hd_keys
        .iter()
        .find(|hd_key| hd_key.key_role == KeyRole::Primary)
        .ok_or_else(|| {
            WalletDataImportDbError::Validation(format!(
                "HD account '{}' must include a primary hd_key identifier",
                account.label.as_str()
            ))
        })?;

    let digital_account_id =
        DigitalAssetAccountId::from_str(&account_id.to_string()).map_err(|err| {
            WalletDataImportDbError::Internal(format!(
                "Failed to convert imported HD account id for bootstrap: {err}"
            ))
        })?;

    bootstrap_initial_hd_account_addresses(
        tx,
        InitialHdAddressBootstrapRequest {
            account_id: digital_account_id,
            asset_id: account.asset_id,
            network: account.network,
            address_scheme: primary_hd_key.address_scheme,
            extended_pubkey: primary_hd_key.value.as_str(),
            gap_limit: BIP44_GAP_LIMIT,
            now,
        },
    )
    .map_err(WalletDataImportDbError::from)
}

fn import_into_transaction(
    tx: &rusqlite::Transaction<'_>,
    user_id: UserId,
    imported_wallets: &[parse::ParsedImportedWallet],
    active_limit: usize,
    now: DateTime<Utc>,
) -> Result<WalletDataImportResult, WalletDataImportDbError> {
    let mut state = resolve::load_import_state(tx)?;
    let creation_plan = merge::plan_import_creations(&state, imported_wallets)?;
    crate::db::account_limits::ensure_supported_account_hard_cap_before_insert_in_tx(
        tx,
        creation_plan.supported_accounts_to_create,
    )
    .map_err(WalletDataImportDbError::from)?;

    let mut result = WalletDataImportResult {
        wallets_created: Vec::new(),
        wallets_matched: Vec::new(),
        native_accounts_created: Vec::new(),
        native_accounts_matched: Vec::new(),
        duplicate_skips: Vec::new(),
        global_duplicate_skips: Vec::new(),
        assertions_created: 0,
        assertions_skipped: 0,
        validation_warnings: Vec::new(),
    };
    let mut supported_account_sequence = 0usize;
    let mut created_hd_accounts = Vec::new();

    for imported_wallet in imported_wallets {
        if imported_wallet.ignored_accessors_count > 0 {
            result.validation_warnings.push(format!(
                "Wallet '{}' contained {} accessor metadata rows that were ignored during import.",
                imported_wallet.label.as_str(),
                imported_wallet.ignored_accessors_count
            ));
        }

        let wallet_id = resolve::resolve_or_create_wallet_id(
            tx,
            &mut state,
            imported_wallet,
            now,
            &mut result,
        )?;

        for native_account in &imported_wallet.native_accounts {
            let created_at = native_account.created_at.unwrap_or_else(|| {
                merge::fallback_import_created_at(now, supported_account_sequence)
            });
            supported_account_sequence = supported_account_sequence.saturating_add(1);
            let resolved_account = resolve::resolve_or_create_native_account(
                tx,
                &mut state,
                wallet_id,
                native_account,
                created_at,
                now,
                &mut result,
            )?;
            let target_account_id = resolved_account.account_id;

            merge::merge_native_account_identifiers(
                tx,
                &mut state,
                target_account_id,
                native_account,
                now,
                &mut result,
            )?;

            if resolved_account.was_created && native_account.account_kind == AccountKind::HdPubkey
            {
                created_hd_accounts.push((target_account_id, native_account.clone()));
            }

            if let Some(sync_slot) = native_account.sync_slot.as_ref() {
                let native_account_id = DigitalAssetAccountId::from_str(
                    &target_account_id.to_string(),
                )
                .map_err(|err| {
                    WalletDataImportDbError::Internal(format!(
                        "Failed to convert imported account id for sync slot: {err}"
                    ))
                })?;
                super::sync_slots::upsert_imported_account_sync_slot(
                    tx,
                    native_account_id,
                    sync_slot.selected_at,
                    &sync_slot.selected_under_tier,
                )
                .map_err(WalletDataImportDbError::from)?;
            }
        }

        for manual_account in &imported_wallet.manual_accounts {
            let created_at = manual_account.created_at.unwrap_or_else(|| {
                merge::fallback_import_created_at(now, supported_account_sequence)
            });
            supported_account_sequence = supported_account_sequence.saturating_add(1);
            let manual_account_id = resolve::resolve_or_create_manual_account(
                tx,
                &mut state,
                wallet_id,
                manual_account,
                created_at,
                now,
            )?;

            let target_scale = manual_account.snapshot.decimal_precision;

            let assertion_dates = state
                .manual_asset_assertion_dates
                .entry(manual_account_id)
                .or_default();

            for assertion in &manual_account.assertions {
                if assertion_dates.contains(&assertion.asserted_on) {
                    result.assertions_skipped =
                        result.assertions_skipped.checked_add(1).ok_or_else(|| {
                            WalletDataImportDbError::Internal(
                                "manual assertion skipped count overflow".to_string(),
                            )
                        })?;
                    continue;
                }

                merge::insert_manual_asset_assertion_in_tx(
                    tx,
                    manual_account_id,
                    assertion,
                    target_scale,
                    now,
                )?;

                assertion_dates.insert(assertion.asserted_on);
                result.assertions_created =
                    result.assertions_created.checked_add(1).ok_or_else(|| {
                        WalletDataImportDbError::Internal(
                            "manual assertion created count overflow".to_string(),
                        )
                    })?;
            }
        }
    }

    let classified = crate::db::account_limits::classify_supported_accounts_in_tx(tx, active_limit)
        .map_err(WalletDataImportDbError::from)?;
    for (target_account_id, native_account) in created_hd_accounts {
        if crate::db::account_limits::account_state_for(&classified, &target_account_id)
            == crate::account_limits::AccountActivationState::Active
        {
            bootstrap_created_hd_account_if_needed(tx, target_account_id, &native_account, now)?;
        }
    }

    let _ = user_id;

    Ok(result)
}

pub(crate) fn import_wallet_data(
    user_id: UserId,
    payload_json: &str,
    active_limit: usize,
    now: DateTime<Utc>,
) -> Result<WalletDataImportResult, WalletDataImportDbError> {
    let payload = parse::parse_payload(payload_json)?;
    let imported_wallets = parse::parse_imported_wallets(&payload, now.date_naive())?;

    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            WalletDataImportDbError::Internal(format!(
                "Failed to begin wallet-data import transaction: {err}"
            ))
        })?;

        let result = import_into_transaction(&tx, user_id, &imported_wallets, active_limit, now)?;

        tx.commit().map_err(|err| {
            WalletDataImportDbError::Internal(format!(
                "Failed to commit wallet-data import transaction: {err}"
            ))
        })?;

        Ok(result)
    })
}

/// Extract settings from a wallet data import payload.
/// Returns `None` if the payload is V1 (no settings) or if settings are absent.
/// This is intended to be called before `import_wallet_data` so the caller can
/// apply settings after the DB transaction commits.
pub(crate) fn extract_import_settings(
    payload_json: &str,
) -> Result<Option<WalletDataImportSettings>, WalletDataImportDbError> {
    let payload = parse::parse_payload(payload_json)?;
    Ok(payload.settings)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn extract_import_settings_returns_none_for_v1() {
        let payload = r#"{"version":1,"exported_at":"2026-04-04T12:00:00Z","bitgarth_version":"0.1.0","wallets":[]}"#;
        let settings = extract_import_settings(payload).expect("should parse");
        assert!(settings.is_none());
    }

    #[test]
    fn extract_import_settings_returns_settings_for_v2() {
        let payload = r#"{"version":2,"exported_at":"2026-04-04T12:00:00Z","bitgarth_version":"0.1.0","wallets":[],"settings":{"language":"en","hledger_account_prefix":"assets:My Wallet"}}"#;
        let settings = extract_import_settings(payload).expect("should parse");
        let s = settings.expect("settings should be present for V2");
        assert_eq!(s.language.as_deref(), Some("en"));
        assert_eq!(
            s.hledger_account_prefix.as_deref(),
            Some("assets:My Wallet")
        );
    }
}

#[cfg(any(
    all(test, feature = "db-tests"),
    all(test, feature = "server", not(bitgarth_db_unit_only))
))]
mod legacy_promotion_tests {
    use super::*;
    use crate::account_limits::AccountActivationState;
    use crate::db::account_limits::{account_state_for, classify_supported_accounts_for_user};
    use crate::db::user_db::with_user_db;
    use crate::models::UserId;
    use chrono::{Duration, TimeZone};

    const TEST_ACTIVE_LIMIT: usize = 10;
    const TEST_NATIVE_SEGWIT_ZPUB: &str = "zpub6qU5MALAB8Bscej9sTEkgSocaxvLzAYYeytsL9fXfv8W4BTykA99FNDNpftwXMGomwc2KatVrbXo4qXsdBC1DiNHCHGapas9enpPBo8y8Y4";

    fn fixed_import_started_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 18, 12, 0, 0)
            .single()
            .expect("fixed import timestamp should be valid")
    }

    fn unique_user_id() -> UserId {
        UserId::new()
    }

    fn setup_test_user(user_id: UserId) {
        super::super::user_db::enable_test_mode();
        let sqlcipher_compatibility = super::super::encryption::current_sqlcipher_compatibility()
            .expect("SQLCipher compatibility should probe");
        super::super::user_db::initialize_user_db(
            user_id,
            super::super::encryption::UserDbOpenMode::Encrypted {
                dek: super::super::encryption::Dek::generate(),
                authority: super::super::encryption::UnlockAuthority::PasswordLogin,
                sqlcipher_compatibility,
            },
        )
        .expect("test user db should initialize");
    }

    fn manual_account_json(index: usize, created_at: Option<&str>) -> String {
        let created_at_json = created_at
            .map(|value| format!(r#","created_at":"{value}""#))
            .unwrap_or_default();
        format!(
            r#"{{
                "label":"Manual {index:03}",
                "asset_instance_id":{{"asset_id":"manual-asset-{index:03}","network_id":"manual-network-{index:03}"}},
                "unit_code":"TOK{index:03}",
                "decimal_precision":6,
                "symbol":null,
                "asset_name":"Manual Asset {index:03}",
                "network_name":"Manual Network {index:03}",
                "coingecko_id":"manual-asset-{index:03}",
                "asset_source":"coingecko_discovery",
                "precision_source":"coingecko_platform",
                "coingecko_platform_id":null,
                "provider_platform_asset_ref":null,
                "balance_assertions":[]
                {created_at_json}
              }}"#
        )
    }

    fn native_eth_account_json(index: usize, created_at: Option<&str>) -> String {
        let created_at_json = created_at
            .map(|value| format!(r#","created_at":"{value}""#))
            .unwrap_or_default();
        format!(
            r#"{{
                "label":"ETH {index:03}",
                "asset_id":"ethereum",
                "network":"mainnet",
                "account_kind":"single_address",
                "sync_slot":null,
                "hd_keys":[],
                "addresses":[{{
                  "address":"0x0000000000000000000000000000000000000{index:03}",
                  "address_scheme":"standard",
                  "source_type":"imported"
                }}]
                {created_at_json}
              }}"#
        )
    }

    fn native_hd_account_json(index: usize, created_at: Option<&str>) -> String {
        let created_at_json = created_at
            .map(|value| format!(r#","created_at":"{value}""#))
            .unwrap_or_default();
        format!(
            r#"{{
                "label":"BTC HD {index:03}",
                "asset_id":"bitcoin",
                "network":"mainnet",
                "account_kind":"hd_pubkey",
                "sync_slot":null,
                "hd_keys":[{{
                  "key_role":"primary",
                  "extended_pubkey":"{TEST_NATIVE_SEGWIT_ZPUB}",
                  "derivation_purpose":84,
                  "derivation_coin_type":0,
                  "derivation_account":0,
                  "address_scheme":"native_segwit",
                  "key_source":"user_provided"
                }}],
                "addresses":[]
                {created_at_json}
              }}"#
        )
    }

    fn manual_import_payload(version: u16, manual_accounts: Vec<String>) -> String {
        format!(
            r#"{{
              "version":{version},
              "exported_at":"2026-04-04T12:00:00Z",
              "bitgarth_version":"0.1.0",
              "wallets":[
                {{
                  "label":"Manual Wallet",
                  "master_fingerprint":null,
                  "identity_source":"user_provided",
                  "verified_at":null,
                  "accessors":[],
                  "digital_asset_accounts":[],
                  "manual_asset_accounts":[{}]
                }}
              ]
            }}"#,
            manual_accounts.join(",")
        )
    }

    fn mixed_import_payload(
        version: u16,
        native_accounts: Vec<String>,
        manual_accounts: Vec<String>,
    ) -> String {
        format!(
            r#"{{
              "version":{version},
              "exported_at":"2026-04-04T12:00:00Z",
              "bitgarth_version":"0.1.0",
              "wallets":[
                {{
                  "label":"Mixed Wallet",
                  "master_fingerprint":null,
                  "identity_source":"user_provided",
                  "verified_at":null,
                  "accessors":[],
                  "digital_asset_accounts":[{}],
                  "manual_asset_accounts":[{}]
                }}
              ]
            }}"#,
            native_accounts.join(","),
            manual_accounts.join(",")
        )
    }

    fn account_created_at_values(user_id: crate::models::UserId) -> Vec<(String, DateTime<Utc>)> {
        with_user_db(
            user_id,
            |conn| -> Result<Vec<(String, DateTime<Utc>)>, super::super::error::DbError> {
                let mut stmt = conn
                    .prepare(
                        "SELECT label, created_at
                         FROM manual_asset_accounts
                         ORDER BY label ASC",
                    )
                    .map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "manual created_at query prepare failed: {err}"
                        ))
                    })?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "manual created_at query failed: {err}"
                        ))
                    })?;
                let mut values = Vec::new();
                for row in rows {
                    let (label, raw_created_at) = row.map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "manual created_at row failed: {err}"
                        ))
                    })?;
                    let created_at = DateTime::parse_from_rfc3339(&raw_created_at)
                        .map(|value| value.with_timezone(&Utc))
                        .map_err(|err| {
                            super::super::error::DbError::new(format!(
                                "manual created_at parse failed: {err}"
                            ))
                        })?;
                    values.push((label, created_at));
                }
                Ok(values)
            },
        )
        .expect("manual account created_at values should load")
    }

    fn native_account_created_at_values(
        user_id: crate::models::UserId,
    ) -> Vec<(String, DateTime<Utc>)> {
        with_user_db(
            user_id,
            |conn| -> Result<Vec<(String, DateTime<Utc>)>, super::super::error::DbError> {
                let mut stmt = conn
                    .prepare(
                        "SELECT label, created_at
                         FROM digital_asset_accounts
                         ORDER BY label ASC",
                    )
                    .map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "native created_at query prepare failed: {err}"
                        ))
                    })?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "native created_at query failed: {err}"
                        ))
                    })?;
                let mut values = Vec::new();
                for row in rows {
                    let (label, raw_created_at) = row.map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "native created_at row failed: {err}"
                        ))
                    })?;
                    let created_at = DateTime::parse_from_rfc3339(&raw_created_at)
                        .map(|value| value.with_timezone(&Utc))
                        .map_err(|err| {
                            super::super::error::DbError::new(format!(
                                "native created_at parse failed: {err}"
                            ))
                        })?;
                    values.push((label, created_at));
                }
                Ok(values)
            },
        )
        .expect("native account created_at values should load")
    }

    fn manual_labels_for_state(
        user_id: crate::models::UserId,
        active_limit: usize,
        state: AccountActivationState,
    ) -> Vec<String> {
        let classified =
            classify_supported_accounts_for_user(user_id, active_limit).expect("classify accounts");
        with_user_db(
            user_id,
            |conn| -> Result<Vec<String>, super::super::error::DbError> {
                let mut labels = Vec::new();
                for account in &classified {
                    if account_state_for(&classified, &account.account_id) != state {
                        continue;
                    }
                    match conn.query_row(
                        "SELECT label FROM manual_asset_accounts WHERE id = ?1",
                        [account.account_id.to_string()],
                        |row| row.get::<_, String>(0),
                    ) {
                        Ok(label) => labels.push(label),
                        Err(rusqlite::Error::QueryReturnedNoRows) => {}
                        Err(err) => {
                            return Err(super::super::error::DbError::new(format!(
                                "manual label query failed: {err}"
                            )));
                        }
                    }
                }
                labels.sort();
                Ok(labels)
            },
        )
        .expect("manual labels should load")
    }

    fn active_manual_labels(user_id: crate::models::UserId, active_limit: usize) -> Vec<String> {
        manual_labels_for_state(user_id, active_limit, AccountActivationState::Active)
    }

    fn inactive_manual_labels(user_id: crate::models::UserId, active_limit: usize) -> Vec<String> {
        manual_labels_for_state(user_id, active_limit, AccountActivationState::Inactive)
    }

    fn manual_account_count(user_id: crate::models::UserId) -> i64 {
        with_user_db(
            user_id,
            |conn| -> Result<i64, super::super::error::DbError> {
                conn.query_row("SELECT COUNT(*) FROM manual_asset_accounts", [], |row| {
                    row.get(0)
                })
                .map_err(|err| {
                    super::super::error::DbError::new(format!(
                        "manual account count query failed: {err}"
                    ))
                })
            },
        )
        .expect("manual account count should load")
    }

    fn derived_address_count_for_label(user_id: crate::models::UserId, label: &str) -> i64 {
        with_user_db(
            user_id,
            |conn| -> Result<i64, super::super::error::DbError> {
                conn.query_row(
                    "SELECT COUNT(*)
                     FROM digital_asset_addresses address
                     JOIN digital_asset_accounts account ON account.id = address.account_id
                     WHERE account.label = ?1",
                    [label],
                    |row| row.get(0),
                )
                .map_err(|err| {
                    super::super::error::DbError::new(format!(
                        "derived address count query failed: {err}"
                    ))
                })
            },
        )
        .expect("derived address count should load")
    }

    #[test]
    fn malformed_supported_manual_account_still_fails() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let malformed = manual_account_json(0, None)
            .replace(r#""asset_id":"manual-asset-000""#, r#""asset_id":"""#);
        let payload = manual_import_payload(4, vec![malformed]);

        assert!(matches!(
            import_wallet_data(
                user_id,
                &payload,
                TEST_ACTIVE_LIMIT,
                fixed_import_started_at(),
            ),
            Err(WalletDataImportDbError::Validation(_))
        ));
    }

    #[test]
    fn malformed_archive_json_still_fails() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        assert!(matches!(
            import_wallet_data(user_id, "{", TEST_ACTIVE_LIMIT, fixed_import_started_at(),),
            Err(WalletDataImportDbError::BadRequest(_))
        ));
    }

    #[test]
    fn wallet_data_import_created_at_preserves_v5_manual_account_created_at() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let payload = manual_import_payload(
            5,
            vec![manual_account_json(0, Some("2026-01-02T03:04:05Z"))],
        );

        import_wallet_data(user_id, &payload, TEST_ACTIVE_LIMIT, now)
            .expect("import should succeed");

        let values = account_created_at_values(user_id);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].0, "Manual 000");
        assert_eq!(
            values[0].1,
            Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
                .single()
                .expect("expected created_at should be valid")
        );
    }

    #[test]
    fn wallet_data_import_created_at_preserves_v5_native_account_created_at() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let payload = mixed_import_payload(
            5,
            vec![native_eth_account_json(1, Some("2026-01-02T03:04:05Z"))],
            Vec::new(),
        );

        import_wallet_data(user_id, &payload, TEST_ACTIVE_LIMIT, now)
            .expect("import should succeed");

        let values = native_account_created_at_values(user_id);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].0, "ETH 001");
        assert_eq!(
            values[0].1,
            Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
                .single()
                .expect("expected created_at should be valid")
        );
    }

    #[test]
    fn wallet_data_import_created_at_assigns_v4_fallbacks_in_file_order() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let payload = manual_import_payload(
            4,
            vec![manual_account_json(0, None), manual_account_json(1, None)],
        );

        import_wallet_data(user_id, &payload, TEST_ACTIVE_LIMIT, now)
            .expect("import should succeed");

        assert_eq!(
            account_created_at_values(user_id),
            vec![
                ("Manual 000".to_string(), now),
                ("Manual 001".to_string(), now + Duration::microseconds(1)),
            ]
        );
    }

    #[test]
    fn wallet_data_import_created_at_ignores_v4_created_at_fields() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let payload = manual_import_payload(
            4,
            vec![
                manual_account_json(0, Some("2026-01-02T03:04:05Z")),
                manual_account_json(1, Some("2026-01-03T03:04:05Z")),
            ],
        );

        import_wallet_data(user_id, &payload, TEST_ACTIVE_LIMIT, now)
            .expect("import should succeed");

        assert_eq!(
            account_created_at_values(user_id),
            vec![
                ("Manual 000".to_string(), now),
                ("Manual 001".to_string(), now + Duration::microseconds(1)),
            ]
        );
    }

    #[test]
    fn wallet_data_import_created_at_fallbacks_keep_same_active_set_on_repeated_imports() {
        let now = fixed_import_started_at();
        let payload = manual_import_payload(
            4,
            vec![
                manual_account_json(0, None),
                manual_account_json(1, None),
                manual_account_json(2, None),
            ],
        );

        let first_user_id = unique_user_id();
        setup_test_user(first_user_id);
        import_wallet_data(first_user_id, &payload, TEST_ACTIVE_LIMIT, now)
            .expect("first import should succeed");

        let second_user_id = unique_user_id();
        setup_test_user(second_user_id);
        import_wallet_data(second_user_id, &payload, TEST_ACTIVE_LIMIT, now)
            .expect("second import should succeed");

        assert_eq!(
            active_manual_labels(first_user_id, 2),
            vec!["Manual 000".to_string(), "Manual 001".to_string()]
        );
        assert_eq!(
            active_manual_labels(second_user_id, 2),
            active_manual_labels(first_user_id, 2)
        );
    }

    #[test]
    fn wallet_data_import_account_limit_allows_supported_accounts_over_active_limit() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let payload = manual_import_payload(
            4,
            (0..15)
                .map(|index| manual_account_json(index, None))
                .collect(),
        );

        import_wallet_data(user_id, &payload, TEST_ACTIVE_LIMIT, now)
            .expect("import should succeed");

        assert_eq!(manual_account_count(user_id), 15);
        assert_eq!(
            active_manual_labels(user_id, 10),
            (0..10)
                .map(|index| format!("Manual {index:03}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            inactive_manual_labels(user_id, 10),
            (10..15)
                .map(|index| format!("Manual {index:03}"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn wallet_data_import_does_not_bootstrap_inactive_hd_accounts() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let payload = mixed_import_payload(
            5,
            vec![native_hd_account_json(0, Some("2026-01-02T03:04:05Z"))],
            vec![manual_account_json(0, Some("2026-01-01T03:04:05Z"))],
        );

        import_wallet_data(user_id, &payload, 1, now).expect("import should succeed");

        assert_eq!(derived_address_count_for_label(user_id, "BTC HD 000"), 0);
    }

    #[test]
    fn wallet_data_import_account_limit_rejects_hard_cap_without_committed_rows() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let payload = manual_import_payload(
            4,
            (0..101)
                .map(|index| manual_account_json(index, None))
                .collect(),
        );

        let result = import_wallet_data(user_id, &payload, TEST_ACTIVE_LIMIT, now);

        assert!(matches!(
            result,
            Err(WalletDataImportDbError::Validation(message))
                if message.contains("Supported account hard cap exceeded")
        ));
        with_user_db(
            user_id,
            |conn| -> Result<(), super::super::error::DbError> {
                let wallet_count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM wallets", [], |row| row.get(0))
                    .map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "wallet count query failed: {err}"
                        ))
                    })?;
                let native_account_count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM digital_asset_accounts", [], |row| {
                        row.get(0)
                    })
                    .map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "native account count query failed: {err}"
                        ))
                    })?;
                let manual_account_count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM manual_asset_accounts", [], |row| {
                        row.get(0)
                    })
                    .map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "manual account count query failed: {err}"
                        ))
                    })?;
                let manual_assertion_count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM manual_asset_balance_assertions",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "manual assertion count query failed: {err}"
                        ))
                    })?;
                assert_eq!(wallet_count, 0);
                assert_eq!(native_account_count, 0);
                assert_eq!(manual_account_count, 0);
                assert_eq!(manual_assertion_count, 0);
                Ok(())
            },
        )
        .expect("rollback verification should succeed");
    }

    #[test]
    fn import_restores_manual_asset_snapshot_fields() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = chrono::Utc::now();
        let payload = r#"{
          "version":4,
          "exported_at":"2026-04-04T12:00:00Z",
          "bitgarth_version":"0.1.0",
          "wallets":[
            {
              "label":"Manual Wallet",
              "master_fingerprint":null,
              "identity_source":"user_provided",
              "verified_at":null,
              "accessors":[],
              "digital_asset_accounts":[],
              "manual_asset_accounts":[{
                "label":"USDC on Algorand",
                "asset_instance_id":{"asset_id":"usd-coin","network_id":"algorand-mainnet"},
                "unit_code":"USDC",
                "decimal_precision":6,
                "symbol":null,
                "asset_name":"USDC on Algorand",
                "network_name":"Algorand",
                "coingecko_id":"usd-coin",
                "asset_source":"coingecko_discovery",
                "precision_source":"coingecko_platform",
                "coingecko_platform_id":"algorand",
                "provider_platform_asset_ref":"31566704",
                "balance_assertions":[]
              }]
            }
          ]
        }"#;

        import_wallet_data(user_id, payload, TEST_ACTIVE_LIMIT, now)
            .expect("import should succeed");

        with_user_db(
            user_id,
            |conn| -> Result<(), super::super::error::DbError> {
                let manual_count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM manual_asset_accounts
                         WHERE asset_id = 'usd-coin'
                           AND network_id = 'algorand-mainnet'
                           AND decimal_precision = 6
                           AND unit_code = 'USDC'
                           AND symbol IS NULL
                           AND asset_name = 'USDC on Algorand'
                           AND network_name = 'Algorand'
                           AND coingecko_id = 'usd-coin'
                           AND asset_source = 'coingecko_discovery'
                           AND precision_source = 'coingecko_platform'
                           AND coingecko_platform_id = 'algorand'
                           AND provider_platform_asset_ref = '31566704'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "manual snapshot query failed: {err}"
                        ))
                    })?;
                assert_eq!(
                    manual_count, 1,
                    "manual_asset_accounts should have one USDC Algorand row"
                );
                Ok(())
            },
        )
        .expect("verification reads should succeed");
    }

    #[test]
    fn import_rejects_partial_manual_asset_snapshot_fields() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = chrono::Utc::now();
        let payload = r#"{
          "version":4,
          "exported_at":"2026-04-04T12:00:00Z",
          "bitgarth_version":"0.1.0",
          "wallets":[
            {
              "label":"Manual Wallet",
              "master_fingerprint":null,
              "identity_source":"user_provided",
              "verified_at":null,
              "accessors":[],
              "digital_asset_accounts":[],
              "manual_asset_accounts":[{
                "label":"USDC on Algorand",
                "asset_instance_id":{"asset_id":"usd-coin","network_id":"algorand-mainnet"},
                "unit_code":"USDC",
                "decimal_precision":6,
                "asset_name":"USDC on Algorand",
                "network_name":"Algorand",
                "balance_assertions":[]
              }]
            }
          ]
        }"#;

        let result = import_wallet_data(user_id, payload, TEST_ACTIVE_LIMIT, now);

        assert!(matches!(
            result,
            Err(WalletDataImportDbError::Validation(message))
                if message.contains("partial manual asset snapshot")
        ));
    }

    #[test]
    fn import_rejects_manual_asset_snapshot_precision_above_db_bound() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = chrono::Utc::now();
        let payload = r#"{
          "version":4,
          "exported_at":"2026-04-04T12:00:00Z",
          "bitgarth_version":"0.1.0",
          "wallets":[
            {
              "label":"Manual Wallet",
              "master_fingerprint":null,
              "identity_source":"user_provided",
              "verified_at":null,
              "accessors":[],
              "digital_asset_accounts":[],
              "manual_asset_accounts":[{
                "label":"USDC on Algorand",
                "asset_instance_id":{"asset_id":"usd-coin","network_id":"algorand-mainnet"},
                "unit_code":"USDC",
                "decimal_precision":19,
                "symbol":null,
                "asset_name":"USDC on Algorand",
                "network_name":"Algorand",
                "coingecko_id":"usd-coin",
                "balance_assertions":[]
              }]
            }
          ]
        }"#;

        let result = import_wallet_data(user_id, payload, TEST_ACTIVE_LIMIT, now);

        assert!(matches!(
            result,
            Err(WalletDataImportDbError::Validation(message))
                if message.contains("decimal_precision")
                    && message.contains("between 0 and 18")
        ));
    }

    #[test]
    fn import_hydrates_old_structured_tezos_manual_asset_from_catalog() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = chrono::Utc::now();
        let payload = r#"{
          "version":4,
          "exported_at":"2026-04-04T12:00:00Z",
          "bitgarth_version":"0.1.0",
          "wallets":[
            {
              "label":"Manual Wallet",
              "master_fingerprint":null,
              "identity_source":"user_provided",
              "verified_at":null,
              "accessors":[],
              "digital_asset_accounts":[],
              "manual_asset_accounts":[{
                "label":"Tezos Mainnet",
                "asset_instance_id":{"asset_id":"tezos","network_id":"tezos-mainnet"},
                "balance_assertions":[]
              }]
            }
          ]
        }"#;

        import_wallet_data(user_id, payload, TEST_ACTIVE_LIMIT, now)
            .expect("import should succeed");

        with_user_db(
            user_id,
            |conn| -> Result<(), super::super::error::DbError> {
                let manual_count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM manual_asset_accounts
                         WHERE asset_id = 'tezos'
                           AND network_id = 'tezos-mainnet'
                           AND decimal_precision = 6
                           AND unit_code = 'XTZ'
                           AND symbol IS NULL
                           AND asset_name = 'Tezos'
                           AND network_name = 'Tezos'
                           AND coingecko_id = 'tezos'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|err| {
                        super::super::error::DbError::new(format!(
                            "manual catalog snapshot query failed: {err}"
                        ))
                    })?;
                assert_eq!(
                    manual_count, 1,
                    "old structured manual import should hydrate catalog snapshot"
                );
                Ok(())
            },
        )
        .expect("verification reads should succeed");
    }
}
