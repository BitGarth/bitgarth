#![cfg(feature = "server")]

use super::account_allowances::AccountAllowances;
use super::client::{CentralProductOptions, CentralTierCapabilities};
use super::types::{
    AccountAllowancePolicy, AccountLimits, CAPABILITY_SCHEMA_VERSION_V4, EntitlementCapabilities,
    EntitlementCapabilityLimits, EntitlementFeatureFlags, EntitlementSource, EntitlementTier,
    FeatureEntitlements, HistoryLimits,
};
use chrono::{DateTime, Utc};
use dioxus::logger::tracing;
use serde::{Deserialize, Serialize};

const BAKED_FREE_TIER_DEFAULTS: &str =
    include_str!("../../assets/entitlements/free_tier_defaults.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FreeTierAccounts {
    pub(crate) balance_sync: u16,
    pub(crate) transaction_history_sync: u16,
    pub(crate) manual: u16,
}

impl FreeTierAccounts {
    pub(crate) fn validated(self) -> Option<AccountAllowances> {
        AccountAllowances::try_new(
            self.balance_sync,
            self.transaction_history_sync,
            self.manual,
        )
        .ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FreeTierHistory {
    pub(crate) max_transactions_per_account: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FreeTierLimits {
    pub(crate) accounts: FreeTierAccounts,
    pub(crate) history: FreeTierHistory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FreeTierFeatures {
    pub(crate) transaction_history_sync: bool,
    pub(crate) balance_sync: bool,
    pub(crate) exchange_rates_current: bool,
    pub(crate) exchange_rates_history: bool,
    pub(crate) price_overrides: bool,
    pub(crate) balance_assertions: bool,
    pub(crate) hledger_export: bool,
    pub(crate) tax_reports: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FreeTierCapabilities {
    pub(crate) limits: FreeTierLimits,
    pub(crate) features: FreeTierFeatures,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FreeTierSnapshot {
    pub(crate) captured_at: DateTime<Utc>,
    pub(crate) capability_schema_version: u16,
    pub(crate) capabilities: FreeTierCapabilities,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FreeTierObservation {
    pub(crate) observed_at: DateTime<Utc>,
    pub(crate) capability_schema_version: u16,
    pub(crate) capabilities: FreeTierCapabilities,
}

pub(crate) fn baked_free_tier_snapshot() -> FreeTierSnapshot {
    let snapshot: FreeTierSnapshot = match serde_json::from_str(BAKED_FREE_TIER_DEFAULTS) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            eprintln!("baked free tier defaults must be valid canonical v4 JSON: {error}");
            std::process::exit(1);
        }
    };
    if snapshot.capability_schema_version != CAPABILITY_SCHEMA_VERSION_V4
        || snapshot.capabilities.limits.accounts.validated().is_none()
    {
        eprintln!("baked free tier defaults must be valid capability schema v4");
        std::process::exit(1);
    }
    snapshot
}

fn baked_observation() -> FreeTierObservation {
    let snapshot = baked_free_tier_snapshot();
    FreeTierObservation {
        observed_at: snapshot.captured_at,
        capability_schema_version: snapshot.capability_schema_version,
        capabilities: snapshot.capabilities,
    }
}

pub(crate) fn free_entitlements_from_observation(
    observation: FreeTierObservation,
) -> FeatureEntitlements {
    let capabilities = EntitlementCapabilities {
        limits: EntitlementCapabilityLimits {
            accounts: Some(AccountLimits::Independent {
                balance_sync: observation.capabilities.limits.accounts.balance_sync,
                transaction_history_sync: observation
                    .capabilities
                    .limits
                    .accounts
                    .transaction_history_sync,
                manual: observation.capabilities.limits.accounts.manual,
            }),
            synced_accounts: observation.capabilities.limits.accounts.balance_sync,
            history: HistoryLimits {
                max_transactions_per_account: observation
                    .capabilities
                    .limits
                    .history
                    .max_transactions_per_account,
            },
        },
        features: EntitlementFeatureFlags {
            historical_sync: false,
            transaction_history_sync: observation.capabilities.features.transaction_history_sync,
            balance_sync: observation.capabilities.features.balance_sync,
            exchange_rates_current: observation.capabilities.features.exchange_rates_current,
            exchange_rates_history: observation.capabilities.features.exchange_rates_history,
            price_overrides: observation.capabilities.features.price_overrides,
            balance_assertions: observation.capabilities.features.balance_assertions,
            hledger_export: observation.capabilities.features.hledger_export,
            tax_reports: observation.capabilities.features.tax_reports,
        },
    };

    FeatureEntitlements::from_capabilities(
        EntitlementTier::Free,
        observation.capability_schema_version,
        capabilities,
        None,
        None,
        EntitlementSource::LocalFree,
    )
}

pub(crate) fn free_capabilities_from_central(
    capabilities: &CentralTierCapabilities,
) -> Option<FreeTierCapabilities> {
    if capabilities.capability_schema_version != CAPABILITY_SCHEMA_VERSION_V4 {
        tracing::warn!(
            capability_schema_version = capabilities.capability_schema_version,
            "payments: ignoring non-v4 free tier capabilities"
        );
        return None;
    }

    let AccountAllowancePolicy::Independent(allowances) = capabilities.account_allowance_policy
    else {
        return None;
    };
    Some(FreeTierCapabilities {
        limits: FreeTierLimits {
            accounts: FreeTierAccounts {
                balance_sync: allowances.balance_sync(),
                transaction_history_sync: allowances.transaction_history_sync(),
                manual: allowances.manual(),
            },
            history: FreeTierHistory {
                max_transactions_per_account: capabilities
                    .historical_backfill_transactions_per_account,
            },
        },
        features: FreeTierFeatures {
            transaction_history_sync: capabilities.transaction_history_sync,
            balance_sync: capabilities.balance_sync,
            exchange_rates_current: capabilities.exchange_rates_current,
            exchange_rates_history: capabilities.exchange_rates_history,
            price_overrides: capabilities.price_overrides,
            balance_assertions: capabilities.balance_assertions,
            hledger_export: capabilities.hledger_export,
            tax_reports: capabilities.tax_reports,
        },
    })
}

pub(crate) fn free_observation_from_central_capabilities(
    capabilities: &CentralTierCapabilities,
    observed_at: DateTime<Utc>,
) -> Option<FreeTierObservation> {
    Some(FreeTierObservation {
        observed_at,
        capability_schema_version: CAPABILITY_SCHEMA_VERSION_V4,
        capabilities: free_capabilities_from_central(capabilities)?,
    })
}

pub(crate) fn free_observation_from_product_options(
    options: &CentralProductOptions,
    observed_at: DateTime<Utc>,
) -> Option<FreeTierObservation> {
    let tier = options.tiers.iter().find(|tier| tier.tier == "free")?;
    free_observation_from_central_capabilities(&tier.capabilities, observed_at)
}

pub(crate) fn newer_free_observation(
    baked: FreeTierObservation,
    cached: FreeTierObservation,
) -> FreeTierObservation {
    if cached.capability_schema_version == CAPABILITY_SCHEMA_VERSION_V4
        && cached.capabilities.limits.accounts.validated().is_some()
        && cached.observed_at >= baked.observed_at
    {
        cached
    } else {
        baked
    }
}

pub(crate) fn resolve_free_entitlements(_now: DateTime<Utc>) -> FeatureEntitlements {
    let baked = baked_observation();
    let cached = match crate::db::load_free_tier_entitlement_cache() {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(error = %err, "payments: free tier cache unavailable; using baked defaults");
            None
        }
    };
    let observation = match cached {
        Some(cached) => newer_free_observation(baked, cached),
        None => baked,
    };
    free_entitlements_from_observation(observation)
}

pub(crate) fn record_free_tier_from_product_options(
    options: &CentralProductOptions,
    fetched_at: DateTime<Utc>,
) {
    let Some(observation) = free_observation_from_product_options(options, fetched_at) else {
        tracing::warn!("payments: product options response did not include a valid free tier");
        return;
    };

    if let Err(err) = crate::db::upsert_free_tier_entitlement_cache(&observation) {
        tracing::warn!(error = %err, "payments: failed to update free tier cache");
    }
}

#[cfg(test)]
pub(crate) fn free_tier_capabilities_for_test(accounts_total: u16) -> FreeTierCapabilities {
    let mut capabilities = baked_free_tier_snapshot().capabilities;
    capabilities.limits.accounts.balance_sync = accounts_total;
    capabilities.limits.accounts.transaction_history_sync = capabilities
        .limits
        .accounts
        .transaction_history_sync
        .min(accounts_total);
    capabilities
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use crate::payments::client::{
        CentralAppCompatibility, CentralAppCompatibilityStatus, CentralProductOptions,
        CentralProductTier, CentralTierCapabilities, CentralTierPresentation,
    };
    use crate::payments::types::{
        CAPABILITY_SCHEMA_VERSION_LEGACY, CAPABILITY_SCHEMA_VERSION_V4, EntitlementSource,
        EntitlementTier,
    };

    fn observed_at(raw: &str) -> chrono::DateTime<chrono::Utc> {
        raw.parse().expect("test timestamp parses")
    }

    fn dt(hour: u32) -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 6, 30, hour, 0, 0).unwrap()
    }

    fn central_capabilities(account_total: u16) -> CentralTierCapabilities {
        CentralTierCapabilities {
            capability_set_id: Some("free.v4".to_string()),
            capability_schema_version: CAPABILITY_SCHEMA_VERSION_V4,
            sync_account_slots: account_total,
            account_allowance_policy: AccountAllowancePolicy::Independent(
                AccountAllowances::try_new(account_total, 3, 1000).expect("test allowances"),
            ),
            historical_backfill_transactions_per_account: 1234,
            historical_sync: true,
            transaction_history_sync: true,
            balance_sync: true,
            exchange_rates_current: true,
            exchange_rates_history: false,
            price_overrides: true,
            balance_assertions: true,
            hledger_export: true,
            tax_reports: false,
        }
    }

    fn central_free_capabilities(accounts: u16) -> CentralTierCapabilities {
        central_capabilities(accounts)
    }

    fn product_options_with_free(accounts: u16) -> CentralProductOptions {
        CentralProductOptions {
            tiers: vec![CentralProductTier {
                tier: "free".to_string(),
                display_name: "Free".to_string(),
                capabilities: central_free_capabilities(accounts),
                presentation: CentralTierPresentation {
                    summary: "Free tier".to_string(),
                    bullets: vec![],
                    is_featured: false,
                    ribbon_label: None,
                },
            }],
            options: vec![],
            app_compatibility: Some(CentralAppCompatibility {
                status: CentralAppCompatibilityStatus::UpgradeRequired,
                detail: "Upgrade required".to_string(),
                minimum_app_version: Some("9.9.9".to_string()),
            }),
            pricing_summary: None,
        }
    }

    #[test]
    fn baked_asset_parses_as_v4_with_independent_allowances() {
        let snapshot = baked_free_tier_snapshot();

        assert_eq!(
            snapshot.capability_schema_version,
            CAPABILITY_SCHEMA_VERSION_V4
        );
        assert_eq!(snapshot.capabilities.limits.accounts.balance_sync, 50);
        assert_eq!(
            snapshot
                .capabilities
                .limits
                .accounts
                .transaction_history_sync,
            3
        );
        assert_eq!(snapshot.capabilities.limits.accounts.manual, 1000);
        assert_eq!(
            snapshot
                .capabilities
                .limits
                .history
                .max_transactions_per_account,
            1000
        );
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/payments/v4-product-options.json"
        ))
        .expect("Central fixture");
        assert_eq!(
            serde_json::to_value(snapshot.capabilities).expect("snapshot capabilities"),
            fixture["tiers"][0]["capabilities"]
        );
    }

    #[test]
    fn test_capabilities_only_override_baked_account_total() {
        let mut expected = baked_free_tier_snapshot().capabilities;
        expected.limits.accounts.balance_sync = 17;

        assert_eq!(free_tier_capabilities_for_test(17), expected);
    }

    #[test]
    fn strict_snapshot_rejects_missing_account_allowance() {
        let invalid = r#"{
          "captured_at": "2026-06-30T00:00:00Z",
          "capability_schema_version": 4,
          "capabilities": {
            "limits": {
              "accounts": { "balance_sync": 50, "manual": 1000 },
              "history": { "max_transactions_per_account": 0 }
            },
            "features": {
              "transaction_history_sync": false,
              "balance_sync": true,
              "exchange_rates_current": true,
              "exchange_rates_history": false,
              "price_overrides": false,
              "balance_assertions": false,
              "hledger_export": false,
              "tax_reports": false
            }
          }
        }"#;

        assert!(serde_json::from_str::<FreeTierSnapshot>(invalid).is_err());
    }

    #[test]
    fn stored_snapshot_omits_legacy_aliases() {
        let snapshot = baked_free_tier_snapshot();
        let json = serde_json::to_string(&snapshot).expect("snapshot serializes");

        assert!(!json.contains("synced_accounts"));
        assert!(!json.contains("historical_sync"));
        assert!(!json.contains("stored_total"));
        assert!(!json.contains("\"total\""));
        assert!(json.contains("transaction_history_sync"));
    }

    #[test]
    fn central_capabilities_convert_to_canonical_v4_entitlements() {
        let observation = free_observation_from_central_capabilities(
            &central_capabilities(42),
            observed_at("2026-06-30T12:00:00Z"),
        )
        .expect("v4 free capabilities convert");
        let entitlements = free_entitlements_from_observation(observation);

        assert_eq!(entitlements.tier, EntitlementTier::Free);
        assert_eq!(entitlements.sync_account_slots_limit, 42);
        assert!(entitlements.historical_backfill_enabled);
        assert!(entitlements.balance_sync_enabled);
        assert!(entitlements.transaction_history_sync_enabled);
        assert!(entitlements.balance_assertions);
        assert!(entitlements.hledger_export);
        assert!(entitlements.exchange_rates_current);
        assert_eq!(
            entitlements.account_allowance_policy,
            AccountAllowancePolicy::Independent(
                AccountAllowances::try_new(42, 3, 1000).expect("test allowances")
            )
        );
        assert_eq!(
            entitlements.historical_backfill_transactions_per_account,
            1234
        );
        assert_eq!(entitlements.source, EntitlementSource::LocalFree);

        let mut unsupported = central_capabilities(42);
        unsupported.capability_schema_version = CAPABILITY_SCHEMA_VERSION_LEGACY;
        assert!(free_capabilities_from_central(&unsupported).is_none());

        let options = CentralProductOptions {
            tiers: vec![CentralProductTier {
                tier: "free".to_string(),
                display_name: "Free".to_string(),
                capabilities: central_capabilities(7),
                presentation: CentralTierPresentation {
                    summary: "Free tier".to_string(),
                    bullets: vec![],
                    is_featured: false,
                    ribbon_label: None,
                },
            }],
            options: vec![],
            app_compatibility: None,
            pricing_summary: None,
        };
        let from_options =
            free_observation_from_product_options(&options, observed_at("2026-06-30T12:00:00Z"))
                .expect("free tier found");
        assert_eq!(from_options.capabilities.limits.accounts.balance_sync, 7);
    }

    #[test]
    fn recency_rule_prefers_newer_cache_and_tie_prefers_cache() {
        let baked = FreeTierObservation {
            observed_at: observed_at("2026-06-30T00:00:00Z"),
            capability_schema_version: CAPABILITY_SCHEMA_VERSION_V4,
            capabilities: free_tier_capabilities_for_test(20),
        };
        let older_cached = FreeTierObservation {
            observed_at: observed_at("2026-06-29T23:59:59Z"),
            capability_schema_version: CAPABILITY_SCHEMA_VERSION_V4,
            capabilities: free_tier_capabilities_for_test(19),
        };
        let tie_cached = FreeTierObservation {
            observed_at: baked.observed_at,
            capability_schema_version: CAPABILITY_SCHEMA_VERSION_V4,
            capabilities: free_tier_capabilities_for_test(21),
        };
        let newer_cached = FreeTierObservation {
            observed_at: observed_at("2026-06-30T00:00:01Z"),
            capability_schema_version: CAPABILITY_SCHEMA_VERSION_V4,
            capabilities: free_tier_capabilities_for_test(22),
        };

        assert_eq!(
            newer_free_observation(baked.clone(), older_cached)
                .capabilities
                .limits
                .accounts
                .balance_sync,
            20
        );
        assert_eq!(
            newer_free_observation(baked.clone(), tie_cached)
                .capabilities
                .limits
                .accounts
                .balance_sync,
            21
        );
        assert_eq!(
            newer_free_observation(baked, newer_cached)
                .capabilities
                .limits
                .accounts
                .balance_sync,
            22
        );
    }

    #[test]
    fn newer_legacy_or_unknown_cache_cannot_override_v4_baked_free() {
        let baked = FreeTierObservation {
            observed_at: observed_at("2026-06-30T00:00:00Z"),
            capability_schema_version: CAPABILITY_SCHEMA_VERSION_V4,
            capabilities: free_tier_capabilities_for_test(50),
        };
        for schema in [3, 5] {
            let cached = FreeTierObservation {
                observed_at: observed_at("2026-07-01T00:00:00Z"),
                capability_schema_version: schema,
                capabilities: free_tier_capabilities_for_test(500),
            };
            assert_eq!(newer_free_observation(baked.clone(), cached), baked);
        }
    }

    #[test]
    fn product_options_free_tier_extracts_even_when_upgrade_required() {
        let observation =
            free_observation_from_product_options(&product_options_with_free(25), dt(2)).unwrap();

        assert_eq!(
            free_entitlements_from_observation(observation).sync_account_slots_limit,
            25
        );
    }

    #[test]
    fn record_and_resolve_use_newer_cached_free_tier() {
        crate::db::enable_test_mode();
        crate::db::reset_test_db();

        let newer_than_baked =
            baked_free_tier_snapshot().captured_at + chrono::TimeDelta::seconds(1);
        record_free_tier_from_product_options(&product_options_with_free(31), newer_than_baked);

        assert_eq!(
            resolve_free_entitlements(dt(4)).sync_account_slots_limit,
            31
        );
    }
}
