use super::error::DbError;
use super::user_db::with_user_db_mut;
use crate::models::UserId;
use crate::payments::types::FeatureEntitlements;
use crate::wallets::{
    AccountKind, BIP44_GAP_LIMIT, DigitalAssetAccountId, KeyRole, WalletAccountId,
};
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
    imported_wallets: &[parse::ParsedImportedWallet],
    entitlements: &FeatureEntitlements,
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
    let mut created_hd_accounts = Vec::new();
    let mut native_admissions = Vec::new();
    let mut manual_admissions = Vec::new();
    let mut supported_account_sequence = 0usize;

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
            let index = supported_account_sequence;
            let created_at = native_account
                .created_at
                .unwrap_or_else(|| merge::fallback_import_created_at(now, index));
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
            if resolved_account.was_created {
                native_admissions.push((
                    target_account_id,
                    native_account
                        .sync_slot
                        .as_ref()
                        .map(|slot| slot.selected_at),
                    index,
                ));
                if native_account.account_kind == AccountKind::HdPubkey {
                    created_hd_accounts.push((target_account_id, native_account.clone()));
                }
            }
        }

        for manual_account in &imported_wallet.manual_accounts {
            let index = supported_account_sequence;
            let created_at = manual_account
                .created_at
                .unwrap_or_else(|| merge::fallback_import_created_at(now, index));
            supported_account_sequence = supported_account_sequence.saturating_add(1);
            let key = resolve::ManualAccountLookupKey {
                wallet_id,
                asset_id: manual_account.snapshot.asset_id.clone(),
                network_id: manual_account.snapshot.network_id.clone(),
            };
            let is_new = !state.manual_account_lookup.contains_key(&key);
            let manual_account_id = resolve::resolve_or_create_manual_account(
                tx,
                &mut state,
                wallet_id,
                manual_account,
                created_at,
                now,
            )?;
            if is_new {
                manual_admissions.push((manual_account_id, manual_account.admitted_at, index));
            }
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

    crate::db::account_limits::ensure_supported_account_hard_cap_before_insert_in_tx(tx, 0)
        .map_err(WalletDataImportDbError::from)?;

    reorder_new_admissions(
        tx,
        native_admissions,
        "SELECT selected_at FROM account_sync_slots ORDER BY selected_at DESC LIMIT ?1",
        "UPDATE account_sync_slots SET selected_at = ?2 WHERE account_id = ?1",
    )?;
    reorder_new_admissions(
        tx,
        manual_admissions,
        "SELECT admitted_at FROM manual_asset_accounts ORDER BY admitted_at DESC LIMIT ?1",
        "UPDATE manual_asset_accounts SET admitted_at = ?2 WHERE id = ?1",
    )?;

    let classified = crate::db::account_limits::classify_supported_accounts_for_entitlements_in_tx(
        tx,
        entitlements,
    )
    .map_err(WalletDataImportDbError::from)?;
    for (target_account_id, native_account) in created_hd_accounts {
        if crate::db::account_limits::account_state_for(&classified, &target_account_id)
            == crate::account_limits::AccountActivationState::Active
        {
            bootstrap_created_hd_account_if_needed(tx, target_account_id, &native_account, now)?;
        }
    }

    Ok(result)
}

fn reorder_new_admissions(
    tx: &rusqlite::Transaction<'_>,
    mut accounts: Vec<(WalletAccountId, Option<DateTime<Utc>>, usize)>,
    newest_timestamps_sql: &'static str,
    update_timestamp_sql: &'static str,
) -> Result<(), WalletDataImportDbError> {
    if accounts.len() < 2 {
        return Ok(());
    }
    let original_ids = accounts.iter().map(|entry| entry.0).collect::<Vec<_>>();
    accounts.sort_by(|left, right| {
        left.1
            .is_none()
            .cmp(&right.1.is_none())
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    if accounts.iter().map(|entry| entry.0).eq(original_ids) {
        return Ok(());
    }

    // New rows receive canonical values after the destination maximum. Reassign
    // only those values so source clocks cannot move an import ahead of existing rows.
    let limit = i64::try_from(accounts.len())
        .map_err(|_| WalletDataImportDbError::Internal("Too many imported accounts".to_string()))?;
    let mut statement = tx.prepare(newest_timestamps_sql).map_err(|err| {
        WalletDataImportDbError::Internal(format!("Failed to prepare admission reorder: {err}"))
    })?;
    let mut local_timestamps = statement
        .query_map([limit], |row| row.get::<_, String>(0))
        .map_err(|err| {
            WalletDataImportDbError::Internal(format!("Failed to read new admissions: {err}"))
        })?
        .map(|row| {
            row.map_err(|err| {
                WalletDataImportDbError::Internal(format!("Invalid new admission: {err}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if local_timestamps.len() != accounts.len() {
        return Err(WalletDataImportDbError::Internal(
            "Missing new account admission".to_string(),
        ));
    }
    local_timestamps.reverse();
    for ((account_id, _, _), timestamp) in accounts.into_iter().zip(local_timestamps) {
        tx.execute(
            update_timestamp_sql,
            rusqlite::params![account_id.to_string(), timestamp],
        )
        .map_err(|err| {
            WalletDataImportDbError::Internal(format!("Failed to reorder admission: {err}"))
        })?;
    }
    Ok(())
}

pub(crate) fn import_wallet_data(
    user_id: UserId,
    payload_json: &str,
    entitlements: &FeatureEntitlements,
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

        let result = import_into_transaction(&tx, &imported_wallets, entitlements, now)?;

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

    fn import_wallet_data(
        user_id: UserId,
        payload_json: &str,
        active_limit: usize,
        now: DateTime<Utc>,
    ) -> Result<WalletDataImportResult, WalletDataImportDbError> {
        let mut entitlements = crate::payments::types::FeatureEntitlements::free();
        entitlements.account_allowance_policy =
            crate::payments::types::AccountAllowancePolicy::LegacyCombined {
                total: u16::try_from(active_limit).expect("test limit fits u16"),
            };
        entitlements.sync_account_slots_limit =
            u16::try_from(active_limit).expect("test limit fits u16");
        super::import_wallet_data(user_id, payload_json, &entitlements, now)
    }

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

    #[test]
    fn manual_only_import_preserves_distinct_fingerprints_and_assertions() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let mut payload: serde_json::Value = serde_json::from_str(&manual_import_payload(
            6,
            vec![manual_account_json(1, None)],
        ))
        .expect("fixture should parse");
        for (fingerprint, amount) in [("a1b2c3d4", "10"), ("b1c2d3e4", "20")] {
            payload["wallets"][0]["master_fingerprint"] = fingerprint.into();
            payload["wallets"][0]["manual_asset_accounts"][0]["balance_assertions"] = serde_json::json!([{"asserted_on":"2026-04-01", "balance_amount":amount, "note":null}]);
            let imported =
                import_wallet_data(user_id, &payload.to_string(), TEST_ACTIVE_LIMIT, now)
                    .expect("distinct wallet should import");
            assert_eq!(imported.wallets_created.len(), 1);
            assert_eq!(imported.assertions_created, 1);
            let repeated =
                import_wallet_data(user_id, &payload.to_string(), TEST_ACTIVE_LIMIT, now)
                    .expect("same fingerprint should match on repeat import");
            assert!(repeated.wallets_created.is_empty());
            assert_eq!(repeated.assertions_created, 0);
        }
        assert_eq!(manual_account_count(user_id), 2);
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

    fn admission_labels(user_id: UserId, table: &str, timestamp: &str) -> Vec<String> {
        with_user_db(user_id, |conn| -> Result<Vec<String>, super::super::error::DbError> {
            let sql = match (table, timestamp) {
                ("native", "selected_at") => "SELECT a.label FROM digital_asset_accounts a JOIN account_sync_slots s ON s.account_id = a.id ORDER BY s.selected_at, a.id",
                ("manual", "admitted_at") => "SELECT label FROM manual_asset_accounts ORDER BY admitted_at, id",
                _ => unreachable!(),
            };
            let mut stmt = conn.prepare(sql).map_err(|err| super::super::error::DbError::new(err.to_string()))?;
            stmt.query_map([], |row| row.get::<_, String>(0))
                .map_err(|err| super::super::error::DbError::new(err.to_string()))?
                .map(|row| row.map_err(|err| super::super::error::DbError::new(err.to_string())))
                .collect()
        })
        .expect("admission labels should load")
    }

    #[test]
    fn fresh_v6_restore_preserves_saved_native_and_manual_priority() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let first_native = native_eth_account_json(1, Some("2026-04-01T00:00:00Z"))
            .replace("\"sync_slot\":null", "\"sync_slot\":{\"selected_at\":\"2026-04-02T00:00:00Z\",\"selected_under_tier\":\"free\"}");
        let second_native = native_eth_account_json(2, Some("2026-04-01T00:00:00Z"))
            .replace("\"sync_slot\":null", "\"sync_slot\":{\"selected_at\":\"2026-04-01T00:00:00Z\",\"selected_under_tier\":\"free\"}");
        let first_manual = manual_account_json(1, Some("2026-04-01T00:00:00Z")).replace(
            "\"balance_assertions\":[]",
            "\"admitted_at\":\"2026-04-02T00:00:00Z\",\"balance_assertions\":[]",
        );
        let second_manual = manual_account_json(2, Some("2026-04-01T00:00:00Z")).replace(
            "\"balance_assertions\":[]",
            "\"admitted_at\":\"2026-04-01T00:00:00Z\",\"balance_assertions\":[]",
        );
        let payload = mixed_import_payload(
            6,
            vec![first_native, second_native],
            vec![first_manual, second_manual],
        );

        import_wallet_data(
            user_id,
            &payload,
            TEST_ACTIVE_LIMIT,
            fixed_import_started_at(),
        )
        .expect("v6 restore should import");
        assert_eq!(
            admission_labels(user_id, "native", "selected_at"),
            vec!["ETH 002", "ETH 001"]
        );
        assert_eq!(
            admission_labels(user_id, "manual", "admitted_at"),
            vec!["Manual 002", "Manual 001"]
        );
    }

    #[test]
    fn import_appends_new_native_after_inactive_destination_and_keeps_duplicates_stable() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let destination = mixed_import_payload(
            5,
            (1..=3)
                .map(|index| native_eth_account_json(index, None))
                .collect(),
            Vec::new(),
        );
        import_wallet_data(user_id, &destination, 2, now).expect("destination should seed");

        let source_slot = "\"sync_slot\":{\"selected_at\":\"2020-01-01T00:00:00Z\",\"selected_under_tier\":\"free\"}";
        let new_account = |index| {
            native_eth_account_json(index, Some("2019-01-01T00:00:00Z"))
                .replace("\"sync_slot\":null", source_slot)
        };
        let incoming = mixed_import_payload(
            6,
            vec![new_account(4), new_account(1), new_account(5)],
            Vec::new(),
        );
        import_wallet_data(user_id, &incoming, 2, now).expect("new accounts should append");
        let expected = vec!["ETH 001", "ETH 002", "ETH 003", "ETH 004", "ETH 005"];
        assert_eq!(admission_labels(user_id, "native", "selected_at"), expected);
        import_wallet_data(user_id, &incoming, 2, now).expect("repeat import should succeed");
        assert_eq!(admission_labels(user_id, "native", "selected_at"), expected);
    }

    #[test]
    fn import_appends_manual_after_destination_even_with_older_source_priority() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let anchor = native_eth_account_json(1, None);
        let destination = mixed_import_payload(
            5,
            vec![anchor.clone()],
            vec![manual_account_json(1, None), manual_account_json(2, None)],
        );
        import_wallet_data(user_id, &destination, 1, now).expect("destination should seed");

        let older = manual_account_json(3, Some("2019-01-01T00:00:00Z")).replace(
            "\"balance_assertions\":[]",
            "\"admitted_at\":\"2020-01-01T00:00:00Z\",\"balance_assertions\":[]",
        );
        let future = manual_account_json(4, Some("2030-01-01T00:00:00Z")).replace(
            "\"balance_assertions\":[]",
            "\"admitted_at\":\"2030-01-01T00:00:00Z\",\"balance_assertions\":[]",
        );
        let incoming = mixed_import_payload(6, vec![anchor], vec![future, older]);
        import_wallet_data(user_id, &incoming, 1, now).expect("manual accounts should append");
        let expected = vec!["Manual 001", "Manual 002", "Manual 003", "Manual 004"];
        assert_eq!(admission_labels(user_id, "manual", "admitted_at"), expected);
        import_wallet_data(user_id, &incoming, 1, now).expect("repeat import should succeed");
        assert_eq!(admission_labels(user_id, "manual", "admitted_at"), expected);
    }

    #[test]
    fn wallet_data_import_storage_boundary_is_exact_and_atomic() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let wallet_id = crate::wallets::WalletId::new();
        super::super::user_db::with_user_db_mut(user_id, |conn| -> Result<(), super::super::error::DbError> {
            let tx = conn.transaction().map_err(|err| super::super::error::DbError::new(err.to_string()))?;
            tx.execute(
                "INSERT INTO wallets (id, label, label_key, identity_source, created_at, updated_at)
                 VALUES (?1, 'Seed Wallet', 'seed wallet', 'user_provided', ?2, ?2)",
                rusqlite::params![wallet_id.to_string(), now.to_rfc3339()],
            ).map_err(|err| super::super::error::DbError::new(err.to_string()))?;
            let mut insert = tx.prepare(
                "INSERT INTO manual_asset_accounts
                 (id, wallet_id, label, label_key, asset_id, network_id, decimal_precision,
                  unit_code, asset_name, network_name, coingecko_id, asset_source,
                  precision_source, created_at, updated_at, admitted_at)
                 VALUES (?1, ?2, ?3, ?3, ?4, ?5, 6, 'TOK', 'Token',
                         'Manual Network', 'token', 'bitgarth_catalog', 'bitgarth_catalog', ?6, ?6, ?7)"
            ).map_err(|err| super::super::error::DbError::new(err.to_string()))?;
            for index in 0..4_999 {
                let label = format!("Seed {index:04}");
                let admitted_at = crate::db::account_admission::format_admission_timestamp(
                    now + chrono::Duration::microseconds(index),
                );
                insert.execute(rusqlite::params![
                    crate::wallets::WalletAccountId::new().to_string(),
                    wallet_id.to_string(), label, format!("manual-asset-{index:03}"),
                    format!("manual-network-{index:03}"),
                    now.to_rfc3339(), admitted_at,
                ]).map_err(|err| super::super::error::DbError::new(err.to_string()))?;
            }
            drop(insert);
            tx.commit().map_err(|err| super::super::error::DbError::new(err.to_string()))
        }).expect("4,999 accounts should seed");

        let two_new = mixed_import_payload(
            6,
            vec![
                native_eth_account_json(1, None),
                native_eth_account_json(2, None),
            ],
            Vec::new(),
        );
        assert!(matches!(
            import_wallet_data(user_id, &two_new, TEST_ACTIVE_LIMIT, now),
            Err(WalletDataImportDbError::Validation(message)) if message.contains("Supported account hard cap exceeded")
        ));
        assert_eq!(manual_account_count(user_id), 4_999);
        assert!(admission_labels(user_id, "native", "selected_at").is_empty());

        let one_new = mixed_import_payload(6, vec![native_eth_account_json(1, None)], Vec::new());
        import_wallet_data(user_id, &one_new, TEST_ACTIVE_LIMIT, now)
            .expect("exact cap should succeed");
        assert_eq!(
            admission_labels(user_id, "native", "selected_at"),
            ["ETH 001"]
        );
        import_wallet_data(user_id, &one_new, TEST_ACTIVE_LIMIT, now)
            .expect("duplicate-only import at cap should succeed");
        assert_eq!(
            admission_labels(user_id, "native", "selected_at"),
            ["ETH 001"]
        );
        assert_eq!(manual_account_count(user_id), 4_999);
        let manual_duplicate = manual_import_payload(6, vec![manual_account_json(1, None)])
            .replace("\"label\":\"Manual Wallet\"", "\"label\":\"Seed Wallet\"");
        import_wallet_data(user_id, &manual_duplicate, TEST_ACTIVE_LIMIT, now)
            .expect("manual duplicate-only import at cap should succeed");
        assert_eq!(manual_account_count(user_id), 4_999);

        let mut augmented =
            serde_json::from_str::<serde_json::Value>(&native_eth_account_json(1, None))
                .expect("native fixture should parse");
        let second = serde_json::from_str::<serde_json::Value>(&native_eth_account_json(2, None))
            .expect("second native fixture should parse");
        augmented["addresses"]
            .as_array_mut()
            .expect("addresses array")
            .push(second["addresses"][0].clone());
        let overlapping = mixed_import_payload(
            6,
            vec![augmented.to_string(), second.to_string()],
            Vec::new(),
        );
        import_wallet_data(user_id, &overlapping, TEST_ACTIVE_LIMIT, now)
            .expect("new identifier on matched account must not count as a new account");
        assert_eq!(
            admission_labels(user_id, "native", "selected_at"),
            ["ETH 001"]
        );
    }

    #[test]
    fn import_plan_keeps_existing_identifier_owner_when_new_hd_account_overlaps() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let manual = manual_account_json(1, None);
        let destination_x = manual_import_payload(6, vec![manual.clone()]).replace(
            "\"master_fingerprint\":null",
            "\"master_fingerprint\":\"a1b2c3d4\"",
        );
        import_wallet_data(user_id, &destination_x, TEST_ACTIVE_LIMIT, now)
            .expect("manual wallet should seed");

        let address = serde_json::json!({
            "address": "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
            "address_scheme": "native_segwit",
            "source_type": "imported"
        });
        let single = serde_json::json!({
            "label": "BTC B", "asset_id": "bitcoin", "network": "mainnet",
            "account_kind": "single_address", "sync_slot": null,
            "hd_keys": [], "addresses": [address.clone()]
        });
        let destination_y = mixed_import_payload(6, vec![single.to_string()], Vec::new());
        import_wallet_data(user_id, &destination_y, TEST_ACTIVE_LIMIT, now)
            .expect("native wallet should seed");

        let mut hd = serde_json::from_str::<serde_json::Value>(&native_hd_account_json(1, None))
            .expect("HD fixture should parse");
        hd["addresses"] = serde_json::json!([address]);
        let mut source_x = serde_json::from_str::<serde_json::Value>(&mixed_import_payload(
            6,
            vec![hd.to_string()],
            vec![manual.clone()],
        ))
        .expect("source X should parse");
        source_x["wallets"][0]["label"] = "Manual Wallet".into();
        source_x["wallets"][0]["master_fingerprint"] = "a1b2c3d4".into();
        let source_y = serde_json::from_str::<serde_json::Value>(&mixed_import_payload(
            6,
            vec![single.to_string()],
            vec![manual],
        ))
        .expect("source Y should parse");
        let incoming = serde_json::json!({
            "version": 6, "exported_at": "2026-04-04T12:00:00Z",
            "bitgarth_version": "0.1.0",
            "wallets": [source_x["wallets"][0].clone(), source_y["wallets"][0].clone()]
        })
        .to_string();
        let payload = parse::parse_payload(&incoming).expect("incoming payload should parse");
        let parsed = parse::parse_imported_wallets(&payload, now.date_naive())
            .expect("incoming wallets should parse");
        let plan = super::super::user_db::with_user_db_mut(user_id, |conn| {
            let tx = conn
                .transaction()
                .map_err(|err| WalletDataImportDbError::Internal(err.to_string()))?;
            let state = resolve::load_import_state(&tx)?;
            merge::plan_import_creations(&state, &parsed)
        })
        .expect("creation plan should succeed");
        assert_eq!(plan.supported_accounts_to_create, 2);
    }

    #[test]
    fn manual_admission_prefix_of_one_thousand_survives_older_import() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let anchor = native_eth_account_json(1, None);
        let destination = mixed_import_payload(
            6,
            vec![anchor.clone()],
            (0..1_000)
                .map(|index| manual_account_json(index, None))
                .collect(),
        );
        import_wallet_data(user_id, &destination, TEST_ACTIVE_LIMIT, now)
            .expect("first thousand manual accounts should import");
        let before = admission_labels(user_id, "manual", "admitted_at");
        let older = manual_account_json(1_000, Some("2010-01-01T00:00:00Z")).replace(
            "\"balance_assertions\":[]",
            "\"admitted_at\":\"2010-01-01T00:00:00Z\",\"balance_assertions\":[]",
        );
        let incoming = mixed_import_payload(6, vec![anchor], vec![older]);
        import_wallet_data(user_id, &incoming, TEST_ACTIVE_LIMIT, now)
            .expect("older source account should append");
        let after = admission_labels(user_id, "manual", "admitted_at");
        assert_eq!(before, after[..1_000]);
        assert_eq!(after[1_000], "Manual 1000");
    }

    #[test]
    fn restore_orders_new_accounts_across_wallets_with_stable_ties_and_missing_metadata() {
        let now = fixed_import_started_at();
        for (first_slot, second_slot, expected) in [
            (
                Some("2026-04-02T00:00:00Z"),
                Some("2026-04-01T00:00:00Z"),
                ["ETH 002", "ETH 001"],
            ),
            (
                Some("2026-04-01T00:00:00Z"),
                Some("2026-04-01T00:00:00Z"),
                ["ETH 001", "ETH 002"],
            ),
            (None, Some("2030-04-01T00:00:00Z"), ["ETH 002", "ETH 001"]),
            (None, None, ["ETH 001", "ETH 002"]),
        ] {
            let user_id = unique_user_id();
            setup_test_user(user_id);
            let account = |index, slot: Option<&str>| {
                let json = native_eth_account_json(index, None);
                match slot {
                    Some(timestamp) => json.replace(
                        "\"sync_slot\":null",
                        &format!("\"sync_slot\":{{\"selected_at\":\"{timestamp}\",\"selected_under_tier\":\"free\"}}"),
                    ),
                    None => json,
                }
            };
            let payload = format!(
                "{{\"version\":6,\"exported_at\":\"2026-04-04T12:00:00Z\",\"bitgarth_version\":\"0.1.0\",\"wallets\":[{{\"label\":\"Wallet A\",\"master_fingerprint\":null,\"identity_source\":\"user_provided\",\"verified_at\":null,\"accessors\":[],\"digital_asset_accounts\":[{}],\"manual_asset_accounts\":[]}},{{\"label\":\"Wallet B\",\"master_fingerprint\":null,\"identity_source\":\"user_provided\",\"verified_at\":null,\"accessors\":[],\"digital_asset_accounts\":[{}],\"manual_asset_accounts\":[]}}]}}",
                account(1, first_slot),
                account(2, second_slot),
            );
            import_wallet_data(user_id, &payload, TEST_ACTIVE_LIMIT, now)
                .expect("multi-wallet restore should succeed");
            assert_eq!(admission_labels(user_id, "native", "selected_at"), expected);
        }
    }

    #[test]
    fn import_uses_newly_created_account_to_match_later_wallet() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let account = native_eth_account_json(1, None);
        let payload = format!(
            "{{\"version\":6,\"exported_at\":\"2026-04-04T12:00:00Z\",\"bitgarth_version\":\"0.1.0\",\"wallets\":[{{\"label\":\"Wallet A\",\"master_fingerprint\":null,\"identity_source\":\"user_provided\",\"verified_at\":null,\"accessors\":[],\"digital_asset_accounts\":[{account}],\"manual_asset_accounts\":[]}},{{\"label\":\"Wallet B\",\"master_fingerprint\":null,\"identity_source\":\"user_provided\",\"verified_at\":null,\"accessors\":[],\"digital_asset_accounts\":[{account}],\"manual_asset_accounts\":[]}}]}}"
        );
        import_wallet_data(
            user_id,
            &payload,
            TEST_ACTIVE_LIMIT,
            fixed_import_started_at(),
        )
        .expect("duplicate wallet import should succeed");
        let counts = with_user_db(
            user_id,
            |conn| -> Result<(i64, i64), super::super::error::DbError> {
                let wallets = conn
                    .query_row("SELECT COUNT(*) FROM wallets", [], |row| row.get(0))
                    .map_err(|err| super::super::error::DbError::new(err.to_string()))?;
                let accounts = conn
                    .query_row("SELECT COUNT(*) FROM digital_asset_accounts", [], |row| {
                        row.get(0)
                    })
                    .map_err(|err| super::super::error::DbError::new(err.to_string()))?;
                Ok((wallets, accounts))
            },
        )
        .expect("counts should load");
        assert_eq!(counts, (1, 1));
    }

    #[test]
    fn repeating_manual_only_import_keeps_one_wallet_and_admission() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let payload = manual_import_payload(6, vec![manual_account_json(1, None)]);
        let now = fixed_import_started_at();
        import_wallet_data(user_id, &payload, TEST_ACTIVE_LIMIT, now).expect("first import");
        import_wallet_data(user_id, &payload, TEST_ACTIVE_LIMIT, now).expect("repeat import");
        assert_eq!(manual_account_count(user_id), 1);
        assert_eq!(
            admission_labels(user_id, "manual", "admitted_at"),
            ["Manual 001"]
        );
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
    fn wallet_data_import_bootstraps_native_hd_despite_earlier_manual_admission_under_v4() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let payload = mixed_import_payload(
            6,
            vec![native_hd_account_json(0, Some("2026-01-02T03:04:05Z"))],
            vec![manual_account_json(0, Some("2026-01-01T03:04:05Z"))],
        );
        let mut entitlements = crate::payments::types::FeatureEntitlements::free();
        entitlements.account_allowance_policy =
            crate::payments::types::AccountAllowancePolicy::Independent(
                crate::payments::account_allowances::AccountAllowances::try_new(1, 1, 1)
                    .expect("valid test allowances"),
            );
        entitlements.balance_sync_enabled = true;
        entitlements.transaction_history_sync_enabled = true;

        super::import_wallet_data(user_id, &payload, &entitlements, now)
            .expect("import should succeed");

        assert!(derived_address_count_for_label(user_id, "BTC HD 000") > 0);
    }

    #[test]
    fn wallet_data_import_account_limit_rejects_hard_cap_without_committed_rows() {
        let user_id = unique_user_id();
        setup_test_user(user_id);
        let now = fixed_import_started_at();
        let payload = manual_import_payload(
            4,
            (0..crate::account_limits::SUPPORTED_ACCOUNT_HARD_CAP + 1)
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
