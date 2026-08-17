//! Current spot prices via CoinGecko, cached in prices.db.
//! Server-only.

use crate::integrations::coingecko::CoinGeckoCredentialMode;
use crate::models::{ApiKeyProvider, CurrencyCode, SimpleApiKey, UserId};
use crate::{
    backend::{
        AccountBalanceStateView, AccountStateView, AccountView, BalanceAmountView,
        CurrentAssetValueView, WalletValueSummaryView, WalletView, WalletsValueSummaryView,
    },
    db::ManualAssetAccountRow,
};
use chrono::Utc;
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const CURRENT_PRICE_PROVIDER_COINGECKO: &str = "coingecko";

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct PriceRequest {
    asset_id: String,
    provider_asset_id: String,
}

#[cfg(test)]
pub(crate) fn reset_cache_for_test() {
    if let Ok(conn) = crate::db::initialize_prices_db() {
        let _ = conn.execute(
            "DELETE FROM current_price_cache WHERE provider = ?1",
            [CURRENT_PRICE_PROVIDER_COINGECKO],
        );
    }
}

#[cfg(test)]
pub(crate) fn seed_price_for_test(id: &str, currency: CurrencyCode, price: &str) {
    let parsed_price = Decimal::from_str(price).expect("test price should parse");
    if let Ok(conn) = crate::db::initialize_prices_db() {
        let _ = crate::db::upsert_current_price_cache(
            &conn,
            crate::db::CurrentPriceCacheUpsert {
                asset_id: id.to_string(),
                quote_currency: currency,
                provider: CURRENT_PRICE_PROVIDER_COINGECKO.to_string(),
                provider_asset_id: id.to_string(),
                provider_quote_id: Some(currency.code().to_lowercase()),
                price: parsed_price,
                observed_at: None,
                retrieved_at: Utc::now(),
                license_scope: "public_keyless".to_string(),
            },
        );
    }
}

/// Returns coingecko_id -> current price (Decimal) in `currency` for the given
/// requests. Dedupes, serves fresh prices.db hits, and fetches misses in one
/// batched `/simple/price` call behind a short timeout. Missing/failed ids are
/// simply absent from the returned map (caller shows "no price").
async fn current_prices(
    user_id: UserId,
    requests: &[PriceRequest],
    currency: CurrencyCode,
) -> HashMap<String, Decimal> {
    current_prices_with_dependencies(
        user_id,
        requests,
        currency,
        || {
            credential_mode_from_api_key_load(crate::db::load_api_key(
                user_id,
                ApiKeyProvider::CoinGecko,
            ))
        },
        |user_id, credential_mode, provider_ids, vs_currency| {
            fetch_simple_price_blocking(user_id, credential_mode, &provider_ids, &vs_currency)
        },
    )
    .await
}

async fn current_prices_with_dependencies<C, F>(
    user_id: UserId,
    requests: &[PriceRequest],
    currency: CurrencyCode,
    credential_loader: C,
    price_fetcher: F,
) -> HashMap<String, Decimal>
where
    C: FnOnce() -> Result<CoinGeckoCredentialMode, crate::db::DbError>,
    F: FnOnce(
            UserId,
            CoinGeckoCredentialMode,
            Vec<String>,
            String,
        ) -> Option<HashMap<String, Decimal>>
        + Send
        + 'static,
{
    let db_requests: Vec<crate::db::CurrentPriceCacheRequest> = requests
        .iter()
        .map(|request| crate::db::CurrentPriceCacheRequest {
            asset_id: request.asset_id.clone(),
            provider_asset_id: request.provider_asset_id.clone(),
        })
        .collect();

    let now = Utc::now();
    let mut prices_by_provider_id = HashMap::new();
    let mut hit_requests = HashSet::new();
    let conn = match crate::db::initialize_prices_db() {
        Ok(conn) => match crate::db::load_fresh_current_price_cache(
            &conn,
            &db_requests,
            currency,
            CURRENT_PRICE_PROVIDER_COINGECKO,
            now,
        ) {
            Ok(hits) => {
                for hit in hits {
                    prices_by_provider_id.insert(hit.provider_asset_id.clone(), hit.price);
                    hit_requests.insert((hit.asset_id, hit.provider_asset_id));
                }
                Some(conn)
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "current prices: failed to read prices db cache"
                );
                None
            }
        },
        Err(err) => {
            tracing::warn!(
                error = %err,
                "current prices: failed to open prices db cache"
            );
            None
        }
    };

    let misses: Vec<PriceRequest> = requests
        .iter()
        .filter(|request| {
            !hit_requests.contains(&(request.asset_id.clone(), request.provider_asset_id.clone()))
        })
        .cloned()
        .collect();
    if misses.is_empty() {
        return prices_by_provider_id;
    }

    let credential_mode = match credential_loader() {
        Ok(mode) => mode,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "current prices: failed to load CoinGecko API key; skipping provider request"
            );
            return prices_by_provider_id;
        }
    };

    let license_scope = match credential_mode.request_config() {
        Ok(config) => config.license_scope.to_string(),
        Err(err) => {
            tracing::warn!(
                error = %err,
                "current prices: invalid CoinGecko credential mode"
            );
            return prices_by_provider_id;
        }
    };

    let mut provider_ids: Vec<String> = misses
        .iter()
        .map(|request| request.provider_asset_id.clone())
        .collect();
    provider_ids.sort();
    provider_ids.dedup();

    let vs_currency = currency.code().to_lowercase();
    let fetch_vs_currency = vs_currency.clone();
    let fetched = tokio::task::spawn_blocking(move || {
        price_fetcher(user_id, credential_mode, provider_ids, fetch_vs_currency)
    })
    .await
    .ok()
    .flatten();

    if let Some(fresh) = fetched {
        if let Some(conn) = conn.as_ref() {
            let retrieved_at = Utc::now();
            for request in &misses {
                if let Some(price) = fresh.get(&request.provider_asset_id)
                    && let Err(err) = crate::db::upsert_current_price_cache(
                        conn,
                        crate::db::CurrentPriceCacheUpsert {
                            asset_id: request.asset_id.clone(),
                            quote_currency: currency,
                            provider: CURRENT_PRICE_PROVIDER_COINGECKO.to_string(),
                            provider_asset_id: request.provider_asset_id.clone(),
                            provider_quote_id: Some(vs_currency.clone()),
                            price: *price,
                            observed_at: None,
                            retrieved_at,
                            license_scope: license_scope.clone(),
                        },
                    )
                {
                    tracing::warn!(
                        asset_id = %request.asset_id,
                        provider_asset_id = %request.provider_asset_id,
                        error = %err,
                        "current prices: failed to persist current price"
                    );
                }
            }
        } else {
            tracing::warn!(
                "current prices: fetched prices but skipped persistence because prices db was unavailable"
            );
        }

        for (provider_asset_id, price) in fresh {
            prices_by_provider_id.insert(provider_asset_id, price);
        }
    }
    prices_by_provider_id
}

pub(crate) async fn selected_manual_asset_current_price(
    user_id: UserId,
    asset_id: String,
    provider_asset_id: String,
    currency: CurrencyCode,
    allow_remote_lookup: bool,
) -> Option<Decimal> {
    let request = PriceRequest {
        asset_id,
        provider_asset_id: provider_asset_id.clone(),
    };
    let db_request = crate::db::CurrentPriceCacheRequest {
        asset_id: request.asset_id.clone(),
        provider_asset_id: request.provider_asset_id.clone(),
    };
    let now = Utc::now();

    match crate::db::initialize_prices_db() {
        Ok(conn) => match crate::db::load_fresh_current_price_cache(
            &conn,
            &[db_request],
            currency,
            CURRENT_PRICE_PROVIDER_COINGECKO,
            now,
        ) {
            Ok(mut hits) => {
                if let Some(hit) = hits.pop() {
                    return Some(hit.price);
                }
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "current prices: failed to read selected manual asset price cache"
                );
            }
        },
        Err(err) => {
            tracing::warn!(
                error = %err,
                "current prices: failed to open selected manual asset price cache"
            );
        }
    }

    let price_fetching_enabled = match crate::db::get_price_fetching_enabled(user_id) {
        Ok(enabled) => enabled,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "current prices: failed to load price-fetching setting for selected manual asset"
            );
            false
        }
    };
    if !price_fetching_enabled && !allow_remote_lookup {
        return None;
    }

    current_prices(user_id, &[request], currency)
        .await
        .get(&provider_asset_id)
        .copied()
}

pub(crate) async fn apply_wallet_valuations(
    user_id: UserId,
    wallets: &mut [WalletView],
    manual_rows: &[ManualAssetAccountRow],
    currency: CurrencyCode,
) -> WalletsValueSummaryView {
    let requests = collect_price_requests(wallets, manual_rows);
    let prices = current_prices(user_id, &requests, currency).await;

    apply_wallet_valuations_from_prices(wallets, manual_rows, &prices, currency)
}

#[cfg(test)]
pub(crate) fn apply_wallet_valuations_from_prices_for_test(
    wallets: &mut [WalletView],
    manual_rows: &[ManualAssetAccountRow],
    prices: &HashMap<String, Decimal>,
    currency: CurrencyCode,
) -> WalletsValueSummaryView {
    apply_wallet_valuations_from_prices(wallets, manual_rows, prices, currency)
}

fn apply_wallet_valuations_from_prices(
    wallets: &mut [WalletView],
    manual_rows: &[ManualAssetAccountRow],
    prices: &HashMap<String, Decimal>,
    currency: CurrencyCode,
) -> WalletsValueSummaryView {
    let mut page_priced = 0u32;
    let mut page_total = 0u32;
    let mut page_priced_wallets = 0u32;
    let total_wallets = wallets.len() as u32;
    let mut page_total_value = Decimal::ZERO;

    for wallet in wallets.iter_mut() {
        let (wallet_priced, wallet_total, wallet_value) =
            value_one_wallet(wallet, manual_rows, prices, currency);
        page_priced += wallet_priced;
        page_total += wallet_total;
        if let Some(wallet_value) = wallet_value {
            page_total_value += wallet_value;
            if wallet_total > 0 && wallet_priced == wallet_total {
                page_priced_wallets += 1;
            }
            wallet.value_summary = Some(WalletValueSummaryView {
                priced_total: wallet_value.to_string(),
                currency,
                priced_asset_count: wallet_priced,
                total_asset_count: wallet_total,
            });
        } else {
            wallet.value_summary = None;
        }
    }

    WalletsValueSummaryView {
        priced_total: page_total_value.to_string(),
        currency,
        priced_asset_count: page_priced,
        total_asset_count: page_total,
        priced_wallet_count: page_priced_wallets,
        total_wallet_count: total_wallets,
    }
}

fn collect_price_requests(
    wallets: &[WalletView],
    manual_rows: &[ManualAssetAccountRow],
) -> Vec<PriceRequest> {
    let mut requests = Vec::new();
    for wallet in wallets {
        for account in &wallet.accounts {
            match account {
                AccountView::Native(native) if account_is_active(native.account_state) => {
                    if let Some(request) = price_request_for_synced_asset(native.balance.asset_id) {
                        requests.push(request);
                    }
                }
                AccountView::Manual(manual) if account_is_active(manual.account_state) => {
                    if let Some(row) = manual_rows
                        .iter()
                        .find(|row| row.account_id == manual.account_id)
                        && let Some(id) = crate::asset_capabilities::resolve_manual_coingecko_id(
                            row.asset_id.as_str(),
                            Some(row.coingecko_id.as_str()),
                        )
                    {
                        requests.push(PriceRequest {
                            asset_id: row.asset_id.as_str().to_string(),
                            provider_asset_id: id,
                        });
                    }
                }
                AccountView::Native(_) | AccountView::Manual(_) | AccountView::Custom(_) => {}
            }
        }
    }
    requests.sort_by(|lhs, rhs| {
        lhs.asset_id
            .cmp(&rhs.asset_id)
            .then(lhs.provider_asset_id.cmp(&rhs.provider_asset_id))
    });
    requests.dedup();
    requests
}

fn account_is_active(state: AccountStateView) -> bool {
    matches!(state, AccountStateView::Active)
}

fn price_request_for_synced_asset(asset_id: crate::wallets::SyncedAssetId) -> Option<PriceRequest> {
    let asset_id = crate::asset_capabilities::asset_id_for_synced_asset(asset_id);
    let provider_asset_id = crate::asset_capabilities::asset(&asset_id)
        .and_then(|asset| asset.price_refs.coingecko.as_deref())?;
    Some(PriceRequest {
        asset_id: asset_id.as_str().to_string(),
        provider_asset_id: provider_asset_id.to_string(),
    })
}

fn aggregate_provider_asset_id(
    wallet: &WalletView,
    balance: &crate::backend::WalletAggregateBalanceView,
    manual_rows: &[ManualAssetAccountRow],
) -> Option<String> {
    for account in &wallet.accounts {
        match account {
            AccountView::Native(native) if account_is_active(native.account_state) => {
                let Some(instance) = crate::asset_capabilities::asset_instance(
                    &crate::asset_capabilities::synced_asset_instance(
                        crate::asset_capabilities::synced_asset_instance_id(native.asset),
                    )
                    .asset_instance_id,
                ) else {
                    continue;
                };
                if instance.id.asset_id.as_str() == balance.asset_id
                    && crate::asset_capabilities::network_slug(instance.id.network_id)
                        == balance.network_id
                {
                    return price_request_for_synced_asset(native.asset)
                        .map(|request| request.provider_asset_id);
                }
            }
            AccountView::Manual(manual) if account_is_active(manual.account_state) => {
                let Some(row) = manual_rows
                    .iter()
                    .find(|row| row.account_id == manual.account_id)
                else {
                    continue;
                };
                if row.asset_id.as_str() == balance.asset_id
                    && row.network_id.as_str() == balance.network_id
                {
                    return crate::asset_capabilities::resolve_manual_coingecko_id(
                        row.asset_id.as_str(),
                        Some(row.coingecko_id.as_str()),
                    );
                }
            }
            AccountView::Native(_) | AccountView::Manual(_) | AccountView::Custom(_) => {}
        }
    }
    None
}

fn aggregate_has_inactive_component(
    wallet: &WalletView,
    balance: &crate::backend::WalletAggregateBalanceView,
    manual_rows: &[ManualAssetAccountRow],
) -> bool {
    wallet.accounts.iter().any(|account| match account {
        AccountView::Native(native) if !account_is_active(native.account_state) => {
            let instance = crate::asset_capabilities::asset_instance(
                &crate::asset_capabilities::synced_asset_instance(
                    crate::asset_capabilities::synced_asset_instance_id(native.asset),
                )
                .asset_instance_id,
            );
            instance.is_some_and(|instance| {
                instance.id.asset_id.as_str() == balance.asset_id
                    && crate::asset_capabilities::network_slug(instance.id.network_id)
                        == balance.network_id
            })
        }
        AccountView::Manual(manual) if !account_is_active(manual.account_state) => manual_rows
            .iter()
            .find(|row| row.account_id == manual.account_id)
            .is_some_and(|row| {
                row.asset_id.as_str() == balance.asset_id
                    && row.network_id.as_str() == balance.network_id
            }),
        AccountView::Native(_) | AccountView::Manual(_) | AccountView::Custom(_) => false,
    })
}

fn value_one_wallet(
    wallet: &mut WalletView,
    manual_rows: &[ManualAssetAccountRow],
    prices: &HashMap<String, Decimal>,
    currency: CurrencyCode,
) -> (u32, u32, Option<Decimal>) {
    let mut priced = 0u32;
    let mut total = 0u32;
    let mut sum = Decimal::ZERO;
    let mut active_native_totals = HashMap::<crate::wallets::SyncedAssetId, Option<Decimal>>::new();

    for account in &wallet.accounts {
        if let AccountView::Native(native) = account
            && account_is_active(native.account_state)
        {
            let total = active_native_totals
                .entry(native.balance.asset_id)
                .or_insert(Some(Decimal::ZERO));
            match known_amount_decimal(&native.balance.balance_state) {
                Some(amount) => {
                    if let Some(total) = total {
                        *total += amount;
                    }
                }
                None => *total = None,
            }
        }
    }
    let native_totals_are_known = active_native_totals.values().all(Option::is_some);

    let aggregate_values =
        wallet
            .balances
            .iter()
            .map(|balance| {
                let has_inactive_component = balance.balance_reliability.reasons().contains(
                &crate::balance_reliability::BalanceProvisionalReason::InactiveAccountNotSyncing,
            ) || aggregate_has_inactive_component(wallet, balance, manual_rows);
                let provider_id = (!has_inactive_component)
                    .then(|| aggregate_provider_asset_id(wallet, balance, manual_rows))
                    .flatten();
                let amount = known_amount_decimal(&balance.balance_state);
                match (provider_id, amount) {
                    (Some(provider_id), Some(amount)) => {
                        prices.get(&provider_id).map(|price| CurrentAssetValueView {
                            price: price.to_string(),
                            converted_value: (*price * amount).to_string(),
                            currency,
                        })
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>();
    for (balance, current_value) in wallet.balances.iter_mut().zip(aggregate_values) {
        balance.current_value = current_value;
    }

    for (asset_id, amount) in &active_native_totals {
        if price_request_for_synced_asset(*asset_id).is_some() {
            total += 1;
        }
        if let Some(amount) = amount
            && let Some((value, _)) =
                current_value_for_synced_amount(*asset_id, *amount, prices, currency)
        {
            sum += value;
            priced += 1;
        }
    }

    for account in wallet.accounts.iter_mut() {
        match account {
            AccountView::Native(native) => {
                if !account_is_active(native.account_state) {
                    native.balance.current_value = None;
                    continue;
                }
                if let Some((_, current_value)) = current_value_for_synced_balance(
                    native.balance.asset_id,
                    &native.balance.balance_state,
                    prices,
                    currency,
                ) {
                    native.balance.current_value = Some(current_value);
                }
            }
            AccountView::Manual(manual) => {
                if !account_is_active(manual.account_state) {
                    manual.current_value = None;
                    continue;
                }
                let id = manual_rows
                    .iter()
                    .find(|row| row.account_id == manual.account_id)
                    .and_then(|row| {
                        crate::asset_capabilities::resolve_manual_coingecko_id(
                            row.asset_id.as_str(),
                            Some(row.coingecko_id.as_str()),
                        )
                    });
                let bal = known_amount_decimal(&manual.balance_state);
                if bal.is_some() {
                    total += 1;
                }
                if let (Some(id), Some(bal)) = (id, bal)
                    && let Some(price) = prices.get(&id)
                {
                    let value = price * bal;
                    sum += value;
                    priced += 1;
                    manual.current_value = Some(CurrentAssetValueView {
                        price: price.to_string(),
                        converted_value: value.to_string(),
                        currency,
                    });
                }
            }
            AccountView::Custom(custom) => {
                if known_nonzero(&custom.balance_state) {
                    total += 1;
                }
            }
        }
    }

    if native_totals_are_known {
        (priced, total, Some(sum))
    } else {
        (0, total, None)
    }
}

fn current_value_for_synced_balance(
    asset_id: crate::wallets::SyncedAssetId,
    state: &AccountBalanceStateView,
    prices: &HashMap<String, Decimal>,
    currency: CurrencyCode,
) -> Option<(Decimal, CurrentAssetValueView)> {
    let bal = known_amount_decimal(state)?;
    current_value_for_synced_amount(asset_id, bal, prices, currency)
}

fn current_value_for_synced_amount(
    asset_id: crate::wallets::SyncedAssetId,
    amount: Decimal,
    prices: &HashMap<String, Decimal>,
    currency: CurrencyCode,
) -> Option<(Decimal, CurrentAssetValueView)> {
    let asset_id = crate::asset_capabilities::asset_id_for_synced_asset(asset_id);
    let id = crate::asset_capabilities::asset(&asset_id)
        .and_then(|asset| asset.price_refs.coingecko.as_deref())?;
    let price = prices.get(id)?;
    let value = *price * amount;
    Some((
        value,
        CurrentAssetValueView {
            price: price.to_string(),
            converted_value: value.to_string(),
            currency,
        },
    ))
}

fn balance_amount_decimal(amount: &BalanceAmountView) -> Option<Decimal> {
    if amount.formatted_value.trim().is_empty() && amount.raw_value == "0" {
        return Some(Decimal::ZERO);
    }
    Decimal::from_str(&amount.formatted_value).ok()
}

fn known_amount_decimal(state: &AccountBalanceStateView) -> Option<Decimal> {
    match state {
        AccountBalanceStateView::Known { amount } => balance_amount_decimal(amount),
        AccountBalanceStateView::Unknown => None,
    }
}

fn known_nonzero(state: &AccountBalanceStateView) -> bool {
    known_amount_decimal(state)
        .map(|amount| amount != Decimal::ZERO)
        .unwrap_or(false)
}

fn credential_mode_from_api_key(api_key: Option<SimpleApiKey>) -> CoinGeckoCredentialMode {
    match api_key {
        Some(api_key) => CoinGeckoCredentialMode::Pro { api_key },
        None => CoinGeckoCredentialMode::PublicKeyless,
    }
}

fn credential_mode_from_api_key_load(
    api_key: Result<Option<SimpleApiKey>, crate::db::DbError>,
) -> Result<CoinGeckoCredentialMode, crate::db::DbError> {
    api_key.map(credential_mode_from_api_key)
}

pub(crate) fn credential_mode_for_user(
    user_id: UserId,
) -> Result<CoinGeckoCredentialMode, crate::db::DbError> {
    credential_mode_for_user_with_loader(|| {
        crate::db::load_api_key(user_id, ApiKeyProvider::CoinGecko)
    })
}

fn credential_mode_for_user_with_loader<L>(
    loader: L,
) -> Result<CoinGeckoCredentialMode, crate::db::DbError>
where
    L: FnOnce() -> Result<Option<SimpleApiKey>, crate::db::DbError>,
{
    credential_mode_from_api_key_load(loader())
}

fn fetch_simple_price_blocking(
    user_id: UserId,
    credential_mode: CoinGeckoCredentialMode,
    ids: &[String],
    vs_currency: &str,
) -> Option<HashMap<String, Decimal>> {
    use crate::integrations::coingecko::client::CoingeckoClient;
    use crate::traces::client::{IntegrationLabel, TracedBlockingClient};

    let traced =
        TracedBlockingClient::builder(IntegrationLabel::new("coingecko-simple-price"), user_id)
            .configure(|b| b.timeout(REQUEST_TIMEOUT))
            .redact_headers(&["x-cg-pro-api-key"])
            .build()
            .ok()?;
    let client = CoingeckoClient::from_credential_mode(traced, credential_mode).ok()?;

    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let response = client.simple_price(&id_refs, vs_currency).ok()?;

    let mut out = HashMap::new();
    for (id, by_currency) in response {
        if let Some(value) = by_currency.get(vs_currency)
            && let Ok(price) = Decimal::from_str(value.get())
        {
            out.insert(id, price);
        }
    }
    Some(out)
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn test_wallet(
        wallet_id: crate::wallets::WalletId,
        balances: Vec<crate::backend::WalletAggregateBalanceView>,
        accounts: Vec<AccountView>,
    ) -> WalletView {
        WalletView {
            id: wallet_id,
            label: "Test Wallet".to_string(),
            master_fingerprint: None,
            logical_account_count: accounts.len() as u32,
            has_accessors: false,
            balances,
            accounts,
            value_summary: None,
        }
    }

    fn test_native_account(
        asset: crate::wallets::SyncedAssetId,
        state: crate::backend::AccountStateView,
        formatted_value: &str,
        account_reference: &str,
    ) -> AccountView {
        let native_account_id = crate::wallets::DigitalAssetAccountId::new();
        let account_id = crate::wallets::WalletAccountId::from(native_account_id);
        let account: crate::backend::NativeAccountView =
            serde_json::from_value(serde_json::json!({
                "account_id": account_id.to_string(),
                "native_account_id": native_account_id.to_string(),
                "account_number": 0,
                "account_state": match state {
                    crate::backend::AccountStateView::Active => "active",
                    crate::backend::AccountStateView::Inactive => "inactive",
                },
                "asset": asset.as_str(),
                "scheme": "native_segwit",
                "label": "Native Account",
                "derivation_path": null,
                "account_reference_kind": "single_address",
                "account_reference": account_reference,
                "balance": test_native_wallet_balance(asset, formatted_value),
                "transaction_counts": {
                    "pending": 0,
                    "confirmed": 0,
                    "dropped": 0,
                    "failed": 0,
                    "total": 0,
                },
                "has_derived_addresses": false,
                "sync_slot": {
                    "selected": false,
                    "active": false,
                    "can_select": true,
                    "limit": 1,
                    "selected_at": null,
                    "selected_under_tier": null,
                },
                "manual_sync": {
                    "mode": "balance_refresh",
                    "slot_effect": "will_select_available_slot",
                    "disabled_reason": null,
                    "used_slots": 0,
                    "slot_limit": 1,
                    "next_tier_display_name": null,
                },
            }))
            .unwrap();
        AccountView::Native(Box::new(account))
    }

    fn test_native_wallet_balance(
        asset: crate::wallets::SyncedAssetId,
        formatted_value: &str,
    ) -> crate::backend::WalletBalanceView {
        serde_json::from_value(serde_json::json!({
            "asset_id": asset.as_str(),
            "context": {
                "network": "mainnet",
            },
            "unit_code": match asset {
                crate::wallets::SyncedAssetId::Bitcoin => "BTC",
                crate::wallets::SyncedAssetId::Ethereum => "ETH",
            },
            "symbol": null,
            "balance_reliability": crate::balance_reliability::BalanceReliability::finalized(),
            "balance_state": {
                "kind": "known",
                "amount": {
                    "raw_value": "0",
                    "formatted_value": formatted_value,
                },
            },
            "current_value": null,
        }))
        .unwrap()
    }

    fn test_wallet_balance(
        asset: crate::wallets::SyncedAssetId,
        formatted_value: &str,
    ) -> crate::backend::WalletAggregateBalanceView {
        let instance = crate::asset_capabilities::asset_instance(
            &crate::asset_capabilities::synced_asset_instance(
                crate::asset_capabilities::synced_asset_instance_id(asset),
            )
            .asset_instance_id,
        )
        .unwrap();
        serde_json::from_value(serde_json::json!({
            "asset_id": instance.id.asset_id.as_str(),
            "network_id": crate::asset_capabilities::network_slug(instance.id.network_id),
            "unit_code": match asset {
                crate::wallets::SyncedAssetId::Bitcoin => "BTC",
                crate::wallets::SyncedAssetId::Ethereum => "ETH",
            },
            "symbol": null,
            "balance_reliability": crate::balance_reliability::BalanceReliability::finalized(),
            "balance_state": {
                "kind": "known",
                "amount": {
                    "raw_value": "0",
                    "formatted_value": formatted_value,
                },
            },
            "current_value": null,
        }))
        .unwrap()
    }

    fn test_manual_row(
        wallet_id: crate::wallets::WalletId,
        account_id: crate::wallets::WalletAccountId,
    ) -> ManualAssetAccountRow {
        let now = "2026-06-06T07:27:19Z".parse().unwrap();
        ManualAssetAccountRow {
            account_id,
            wallet_id,
            label: crate::wallets::Label::parse_with_limit(
                "ADA Account",
                crate::wallets::ACCOUNT_LABEL_MAX_LENGTH,
            )
            .unwrap(),
            asset_id: crate::asset_capabilities::AssetId::owned("cardano".to_string()).unwrap(),
            network_id: crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(
                "cardano-mainnet",
            )
            .unwrap(),
            unit_code: crate::wallets::ValidatedManualAssetUnitCode::parse("ADA").unwrap(),
            decimal_precision: crate::wallets::ManualAssetDisplayScale::from_u8(6),
            symbol: None,
            asset_name: "Cardano".to_string(),
            network_name: "Cardano".to_string(),
            coingecko_id: crate::asset_capabilities::unsynced::CoingeckoAssetId::parse(
                "stale-snapshot",
            )
            .unwrap(),
            asset_source: "bitgarth_catalog".to_string(),
            precision_source: "bitgarth_catalog".to_string(),
            coingecko_platform_id: None,
            provider_platform_asset_ref: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn test_manual_account(
        account_id: crate::wallets::WalletAccountId,
        state: crate::backend::AccountStateView,
        formatted_value: &str,
    ) -> AccountView {
        AccountView::Manual(crate::backend::ManualAssetAccountView {
            account_id,
            account_state: state,
            label: "ADA Account".to_string(),
            asset_instance_id: crate::asset_views::ManualAssetInstanceIdView {
                asset_id: "cardano".to_string(),
                network_id: "cardano-mainnet".to_string(),
            },
            unit_code: "ADA".to_string(),
            asset_name: "Cardano".to_string(),
            network_name: "Cardano".to_string(),
            decimal_precision: 6,
            symbol: None,
            balance_state: AccountBalanceStateView::Known {
                amount: BalanceAmountView {
                    raw_value: "0".to_string(),
                    formatted_value: formatted_value.to_string(),
                },
            },
            current_value: None,
        })
    }

    #[test]
    fn synced_asset_price_request_uses_catalog_provider_id() {
        assert_eq!(
            price_request_for_synced_asset(crate::wallets::SyncedAssetId::Ethereum),
            Some(PriceRequest {
                asset_id: "ethereum".to_string(),
                provider_asset_id: "ethereum".to_string(),
            })
        );
    }

    #[test]
    fn collected_price_requests_include_asset_id_and_provider_asset_id() {
        let wallet_id = crate::wallets::WalletId::new();
        let account_id = crate::wallets::WalletAccountId::new();
        let now = "2026-06-06T07:27:19Z".parse().unwrap();
        let manual_row = ManualAssetAccountRow {
            account_id,
            wallet_id,
            label: crate::wallets::Label::parse_with_limit(
                "ADA Account 1",
                crate::wallets::ACCOUNT_LABEL_MAX_LENGTH,
            )
            .unwrap(),
            asset_id: crate::asset_capabilities::AssetId::owned("cardano".to_string()).unwrap(),
            network_id: crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(
                "cardano-mainnet",
            )
            .unwrap(),
            unit_code: crate::wallets::ValidatedManualAssetUnitCode::parse("ADA").unwrap(),
            decimal_precision: crate::wallets::ManualAssetDisplayScale::from_u8(6),
            symbol: None,
            asset_name: "Cardano".to_string(),
            network_name: "Cardano".to_string(),
            coingecko_id: crate::asset_capabilities::unsynced::CoingeckoAssetId::parse(
                "stale-snapshot",
            )
            .unwrap(),
            asset_source: "bitgarth_catalog".to_string(),
            precision_source: "bitgarth_catalog".to_string(),
            coingecko_platform_id: None,
            provider_platform_asset_ref: None,
            created_at: now,
            updated_at: now,
        };
        let wallet = WalletView {
            id: wallet_id,
            label: "Manual".to_string(),
            master_fingerprint: None,
            logical_account_count: 1,
            has_accessors: false,
            balances: vec![],
            accounts: vec![AccountView::Manual(
                crate::backend::ManualAssetAccountView {
                    account_id,
                    account_state: crate::backend::AccountStateView::Active,
                    label: "ADA Account 1".to_string(),
                    asset_instance_id: crate::asset_views::ManualAssetInstanceIdView {
                        asset_id: "cardano".to_string(),
                        network_id: "cardano-mainnet".to_string(),
                    },
                    unit_code: "ADA".to_string(),
                    asset_name: "Cardano".to_string(),
                    network_name: "Cardano".to_string(),
                    decimal_precision: 6,
                    symbol: None,
                    balance_state: AccountBalanceStateView::Known {
                        amount: BalanceAmountView {
                            raw_value: "1000000".to_string(),
                            formatted_value: "1".to_string(),
                        },
                    },
                    current_value: None,
                },
            )],
            value_summary: None,
        };

        let requests = collect_price_requests(&[wallet], &[manual_row]);

        assert_eq!(
            requests,
            vec![PriceRequest {
                asset_id: "cardano".to_string(),
                provider_asset_id: "cardano".to_string(),
            }]
        );
    }

    #[test]
    fn inactive_native_balance_does_not_produce_price_request() {
        let wallet_id = crate::wallets::WalletId::new();
        let account = test_native_account(
            crate::wallets::SyncedAssetId::Bitcoin,
            crate::backend::AccountStateView::Inactive,
            "1",
            "bc1q-private-address",
        );
        let aggregate_balance = test_wallet_balance(crate::wallets::SyncedAssetId::Bitcoin, "1");
        let wallet = test_wallet(wallet_id, vec![aggregate_balance], vec![account]);

        let requests = collect_price_requests(&[wallet], &[]);

        assert!(requests.is_empty());
    }

    #[test]
    fn inactive_manual_asset_does_not_produce_price_request() {
        let wallet_id = crate::wallets::WalletId::new();
        let account_id = crate::wallets::WalletAccountId::new();
        let manual_row = test_manual_row(wallet_id, account_id);
        let wallet = test_wallet(
            wallet_id,
            vec![],
            vec![test_manual_account(
                account_id,
                crate::backend::AccountStateView::Inactive,
                "1",
            )],
        );

        let requests = collect_price_requests(&[wallet], &[manual_row]);

        assert!(requests.is_empty());
    }

    #[test]
    fn active_native_and_manual_accounts_produce_price_requests() {
        let wallet_id = crate::wallets::WalletId::new();
        let manual_account_id = crate::wallets::WalletAccountId::new();
        let manual_row = test_manual_row(wallet_id, manual_account_id);
        let wallet = test_wallet(
            wallet_id,
            vec![],
            vec![
                test_native_account(
                    crate::wallets::SyncedAssetId::Ethereum,
                    crate::backend::AccountStateView::Active,
                    "1",
                    "0xprivateaddress",
                ),
                test_manual_account(
                    manual_account_id,
                    crate::backend::AccountStateView::Active,
                    "2",
                ),
            ],
        );

        let requests = collect_price_requests(&[wallet], &[manual_row]);

        assert_eq!(
            requests,
            vec![
                PriceRequest {
                    asset_id: "cardano".to_string(),
                    provider_asset_id: "cardano".to_string(),
                },
                PriceRequest {
                    asset_id: "ethereum".to_string(),
                    provider_asset_id: "ethereum".to_string(),
                },
            ]
        );
    }

    #[test]
    fn inactive_accounts_do_not_contribute_to_current_value_totals() {
        let currency = CurrencyCode::from_code("USD").unwrap();
        let wallet_id = crate::wallets::WalletId::new();
        let active_manual_id = crate::wallets::WalletAccountId::new();
        let inactive_manual_id = crate::wallets::WalletAccountId::new();
        let active_manual_row = test_manual_row(wallet_id, active_manual_id);
        let inactive_manual_row = test_manual_row(wallet_id, inactive_manual_id);
        let prices = HashMap::from([
            ("bitcoin".to_string(), Decimal::from_str("10").unwrap()),
            ("cardano".to_string(), Decimal::from_str("3").unwrap()),
        ]);
        let mut wallet = test_wallet(
            wallet_id,
            vec![test_wallet_balance(
                crate::wallets::SyncedAssetId::Bitcoin,
                "3",
            )],
            vec![
                test_native_account(
                    crate::wallets::SyncedAssetId::Bitcoin,
                    crate::backend::AccountStateView::Active,
                    "1",
                    "bc1q-active-private-address",
                ),
                test_native_account(
                    crate::wallets::SyncedAssetId::Bitcoin,
                    crate::backend::AccountStateView::Inactive,
                    "2",
                    "bc1q-inactive-private-address",
                ),
                test_manual_account(
                    active_manual_id,
                    crate::backend::AccountStateView::Active,
                    "2",
                ),
                test_manual_account(
                    inactive_manual_id,
                    crate::backend::AccountStateView::Inactive,
                    "5",
                ),
            ],
        );

        let (priced, total, sum) = value_one_wallet(
            &mut wallet,
            &[active_manual_row, inactive_manual_row],
            &prices,
            currency,
        );

        assert_eq!(priced, 2);
        assert_eq!(total, 2);
        assert_eq!(sum, Some(Decimal::from_str("16").unwrap()));
        assert!(wallet.balances[0].current_value.is_none());
        let AccountView::Native(active_native) = &wallet.accounts[0] else {
            panic!("expected active native");
        };
        assert_eq!(
            active_native
                .balance
                .current_value
                .as_ref()
                .map(|value| value.converted_value.as_str()),
            Some("10")
        );
        let AccountView::Native(inactive_native) = &wallet.accounts[1] else {
            panic!("expected inactive native");
        };
        assert!(inactive_native.balance.current_value.is_none());
        let AccountView::Manual(inactive_manual) = &wallet.accounts[3] else {
            panic!("expected inactive manual");
        };
        assert!(inactive_manual.current_value.is_none());
        assert_eq!(
            known_amount_decimal(&inactive_manual.balance_state),
            Some(Decimal::from_str("5").unwrap())
        );
    }

    #[test]
    fn all_active_native_asset_preserves_aggregate_current_value() {
        let currency = CurrencyCode::from_code("USD").unwrap();
        let wallet_id = crate::wallets::WalletId::new();
        let prices = HashMap::from([("bitcoin".to_string(), Decimal::from_str("10").unwrap())]);
        let mut wallet = test_wallet(
            wallet_id,
            vec![test_wallet_balance(
                crate::wallets::SyncedAssetId::Bitcoin,
                "3",
            )],
            vec![
                test_native_account(
                    crate::wallets::SyncedAssetId::Bitcoin,
                    crate::backend::AccountStateView::Active,
                    "1",
                    "bc1q-active-private-address-1",
                ),
                test_native_account(
                    crate::wallets::SyncedAssetId::Bitcoin,
                    crate::backend::AccountStateView::Active,
                    "2",
                    "bc1q-active-private-address-2",
                ),
            ],
        );

        let (_priced, _total, _sum) = value_one_wallet(&mut wallet, &[], &prices, currency);

        assert_eq!(
            wallet.balances[0]
                .current_value
                .as_ref()
                .map(|value| value.converted_value.as_str()),
            Some("30")
        );
    }

    #[test]
    fn multiple_active_native_rows_for_same_asset_count_as_one_priced_asset() {
        let currency = CurrencyCode::from_code("USD").unwrap();
        let wallet_id = crate::wallets::WalletId::new();
        let prices = HashMap::from([("bitcoin".to_string(), Decimal::from_str("10").unwrap())]);
        let mut wallet = test_wallet(
            wallet_id,
            vec![test_wallet_balance(
                crate::wallets::SyncedAssetId::Bitcoin,
                "3",
            )],
            vec![
                test_native_account(
                    crate::wallets::SyncedAssetId::Bitcoin,
                    crate::backend::AccountStateView::Active,
                    "1",
                    "bc1q-active-private-address-1",
                ),
                test_native_account(
                    crate::wallets::SyncedAssetId::Bitcoin,
                    crate::backend::AccountStateView::Active,
                    "2",
                    "bc1q-active-private-address-2",
                ),
            ],
        );

        let (priced, total, sum) = value_one_wallet(&mut wallet, &[], &prices, currency);

        assert_eq!(priced, 1);
        assert_eq!(total, 1);
        assert_eq!(sum, Some(Decimal::from_str("30").unwrap()));
    }

    #[tokio::test]
    async fn coingecko_fetch_inputs_do_not_include_account_private_data() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let wallet_id = crate::wallets::WalletId::new();
        let native = test_native_account(
            crate::wallets::SyncedAssetId::Bitcoin,
            crate::backend::AccountStateView::Active,
            "1",
            "bc1q-private-address",
        );
        let AccountView::Native(native_view) = &native else {
            panic!("expected native");
        };
        let private_account_id = native_view.account_id.to_string();
        let private_address = native_view.account_reference.clone();
        let wallet = test_wallet(wallet_id, vec![], vec![native]);
        let requests = collect_price_requests(&[wallet], &[]);
        let currency = CurrencyCode::from_code("USD").unwrap();
        let conn = crate::db::initialize_prices_db().unwrap();
        conn.execute(
            "DELETE FROM current_price_cache WHERE asset_id = ?1 AND quote_currency = ?2",
            ("bitcoin", currency.code()),
        )
        .unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_fetch = Arc::clone(&captured);

        current_prices_with_dependencies(
            UserId::new(),
            &requests,
            currency,
            || Ok(CoinGeckoCredentialMode::PublicKeyless),
            move |_user_id, _credential_mode, provider_ids, vs_currency| {
                captured_fetch
                    .lock()
                    .unwrap()
                    .push((provider_ids, vs_currency));
                Some(HashMap::new())
            },
        )
        .await;

        let calls = captured.lock().unwrap();
        assert_eq!(
            calls.as_slice(),
            &[(vec!["bitcoin".to_string()], "usd".to_string())]
        );
        let request_debug = format!("{requests:?}");
        assert!(!request_debug.contains(&private_account_id));
        assert!(!request_debug.contains(&private_address));
    }

    #[test]
    fn missing_coingecko_key_uses_public_mode() {
        let mode = credential_mode_from_api_key(None);
        assert_eq!(mode, CoinGeckoCredentialMode::PublicKeyless);

        let config = mode.request_config().expect("public config");
        assert_eq!(
            config.base_url.as_str(),
            "https://api.coingecko.com/api/v3/"
        );
        assert!(config.header.is_none());
        assert_eq!(config.license_scope, "public_keyless");
    }

    #[test]
    fn configured_coingecko_key_uses_pro_mode() {
        let api_key = SimpleApiKey::new("PRO_KEY".to_string()).expect("valid key");

        let mode = credential_mode_from_api_key(Some(api_key.clone()));
        assert_eq!(mode, CoinGeckoCredentialMode::Pro { api_key });

        let config = mode.request_config().expect("pro config");
        assert_eq!(
            config.base_url.as_str(),
            "https://pro-api.coingecko.com/api/v3/"
        );
        assert_eq!(
            config
                .header
                .as_ref()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            Some(("x-cg-pro-api-key", "PRO_KEY"))
        );
        assert_eq!(config.license_scope, "coingecko_pro_key");
    }

    #[test]
    fn failed_coingecko_key_load_fails_closed() {
        assert!(
            credential_mode_from_api_key_load(Err(crate::db::DbError::new("load failed"))).is_err()
        );
    }

    #[test]
    fn credential_mode_for_user_loader_preserves_keyless_and_pro_modes() {
        assert_eq!(
            credential_mode_for_user_with_loader(|| Ok(None)).expect("keyless mode"),
            CoinGeckoCredentialMode::PublicKeyless
        );

        let api_key = SimpleApiKey::new("PRO_KEY".to_string()).expect("valid key");
        assert_eq!(
            credential_mode_for_user_with_loader(|| Ok(Some(api_key.clone()))).expect("pro mode"),
            CoinGeckoCredentialMode::Pro { api_key }
        );
    }

    #[tokio::test]
    async fn stale_or_missing_rows_trigger_one_batched_fetch_and_are_persisted() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let usd = CurrencyCode::from_code("USD").unwrap();
        let stale_asset_id = "current-price-service-stale";
        let missing_asset_id = "current-price-service-missing";
        let conn = crate::db::initialize_prices_db().unwrap();
        conn.execute(
            "DELETE FROM current_price_cache WHERE asset_id IN (?1, ?2)",
            [stale_asset_id, missing_asset_id],
        )
        .unwrap();
        crate::db::upsert_current_price_cache(
            &conn,
            crate::db::CurrentPriceCacheUpsert {
                asset_id: stale_asset_id.to_string(),
                quote_currency: usd,
                provider: CURRENT_PRICE_PROVIDER_COINGECKO.to_string(),
                provider_asset_id: stale_asset_id.to_string(),
                provider_quote_id: Some("usd".to_string()),
                price: Decimal::from_str("0.11").unwrap(),
                observed_at: None,
                retrieved_at: Utc::now()
                    - crate::db::CURRENT_PRICE_CACHE_TTL
                    - chrono::Duration::seconds(1),
                license_scope: "public_keyless".to_string(),
            },
        )
        .unwrap();

        let calls = Arc::new(Mutex::new(Vec::new()));
        let fetch_calls = Arc::clone(&calls);
        let prices = current_prices_with_dependencies(
            UserId::new(),
            &[
                PriceRequest {
                    asset_id: stale_asset_id.to_string(),
                    provider_asset_id: stale_asset_id.to_string(),
                },
                PriceRequest {
                    asset_id: missing_asset_id.to_string(),
                    provider_asset_id: missing_asset_id.to_string(),
                },
            ],
            usd,
            || Ok(CoinGeckoCredentialMode::PublicKeyless),
            move |_user_id, credential_mode, provider_ids, vs_currency| {
                let license_scope = credential_mode
                    .request_config()
                    .expect("valid public config")
                    .license_scope
                    .to_string();
                fetch_calls.lock().unwrap().push((
                    provider_ids.clone(),
                    vs_currency.clone(),
                    license_scope,
                ));
                Some(HashMap::from([
                    (
                        stale_asset_id.to_string(),
                        Decimal::from_str("1.23").unwrap(),
                    ),
                    (
                        missing_asset_id.to_string(),
                        Decimal::from_str("4.56").unwrap(),
                    ),
                ]))
            },
        )
        .await;

        assert_eq!(
            prices.get(stale_asset_id),
            Some(&Decimal::from_str("1.23").unwrap())
        );
        assert_eq!(
            prices.get(missing_asset_id),
            Some(&Decimal::from_str("4.56").unwrap())
        );
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[(
                vec![missing_asset_id.to_string(), stale_asset_id.to_string()],
                "usd".to_string(),
                "public_keyless".to_string(),
            )]
        );

        let rows = crate::db::load_fresh_current_price_cache(
            &conn,
            &[
                crate::db::CurrentPriceCacheRequest {
                    asset_id: stale_asset_id.to_string(),
                    provider_asset_id: stale_asset_id.to_string(),
                },
                crate::db::CurrentPriceCacheRequest {
                    asset_id: missing_asset_id.to_string(),
                    provider_asset_id: missing_asset_id.to_string(),
                },
            ],
            usd,
            CURRENT_PRICE_PROVIDER_COINGECKO,
            Utc::now(),
        )
        .unwrap();
        let cached: HashMap<_, _> = rows
            .into_iter()
            .map(|row| (row.asset_id, (row.price, row.license_scope)))
            .collect();
        assert_eq!(
            cached.get(stale_asset_id),
            Some(&(
                Decimal::from_str("1.23").unwrap(),
                "public_keyless".to_string()
            ))
        );
        assert_eq!(
            cached.get(missing_asset_id),
            Some(&(
                Decimal::from_str("4.56").unwrap(),
                "public_keyless".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn pro_mode_fetch_persists_pro_license_scope() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let usd = CurrencyCode::from_code("USD").unwrap();
        let asset_id = "current-price-service-pro-license";
        let conn = crate::db::initialize_prices_db().unwrap();
        conn.execute(
            "DELETE FROM current_price_cache WHERE asset_id = ?1",
            [asset_id],
        )
        .unwrap();

        let api_key = SimpleApiKey::new("PRO_KEY".to_string()).unwrap();
        let prices = current_prices_with_dependencies(
            UserId::new(),
            &[PriceRequest {
                asset_id: asset_id.to_string(),
                provider_asset_id: asset_id.to_string(),
            }],
            usd,
            || Ok(CoinGeckoCredentialMode::Pro { api_key }),
            move |_user_id, credential_mode, provider_ids, vs_currency| {
                let config = credential_mode.request_config().expect("valid pro config");
                assert_eq!(config.license_scope, "coingecko_pro_key");
                assert_eq!(provider_ids, vec![asset_id.to_string()]);
                assert_eq!(vs_currency, "usd");
                Some(HashMap::from([(
                    asset_id.to_string(),
                    Decimal::from_str("7.89").unwrap(),
                )]))
            },
        )
        .await;

        assert_eq!(
            prices.get(asset_id),
            Some(&Decimal::from_str("7.89").unwrap())
        );
        let rows = crate::db::load_fresh_current_price_cache(
            &conn,
            &[crate::db::CurrentPriceCacheRequest {
                asset_id: asset_id.to_string(),
                provider_asset_id: asset_id.to_string(),
            }],
            usd,
            CURRENT_PRICE_PROVIDER_COINGECKO,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].price, Decimal::from_str("7.89").unwrap());
        assert_eq!(rows[0].license_scope, "coingecko_pro_key");
    }

    #[tokio::test]
    async fn selected_manual_asset_price_returns_fresh_cache_hit_without_remote_consent() {
        let _runtime = crate::db::acquire_test_runtime().expect("test runtime should initialize");
        let usd = CurrencyCode::from_code("USD").unwrap();
        seed_price_for_test("selected-price-cache-hit", usd, "1.23");

        let price = selected_manual_asset_current_price(
            UserId::new(),
            "selected-price-cache-hit".to_string(),
            "selected-price-cache-hit".to_string(),
            usd,
            false,
        )
        .await;

        assert_eq!(price, Some(Decimal::from_str("1.23").unwrap()));
    }

    #[tokio::test]
    async fn selected_manual_asset_price_miss_without_consent_returns_none() {
        let usd = CurrencyCode::from_code("USD").unwrap();

        let price = selected_manual_asset_current_price(
            UserId::new(),
            "selected-price-miss-no-consent".to_string(),
            "selected-price-miss-no-consent".to_string(),
            usd,
            false,
        )
        .await;

        assert_eq!(price, None);
    }

    #[test]
    fn balance_amount_decimal_uses_formatted_units_not_raw_base_units() {
        let amount = BalanceAmountView {
            raw_value: "1000000".to_string(),
            formatted_value: "1".to_string(),
        };

        assert_eq!(
            balance_amount_decimal(&amount),
            Some(Decimal::from_str("1").unwrap())
        );
    }

    #[test]
    fn wallet_valuation_uses_manual_display_units_not_raw_base_units() {
        let eur = CurrencyCode::from_code("EUR").unwrap();
        let wallet_id = crate::wallets::WalletId::new();
        let account_id = crate::wallets::WalletAccountId::new();
        let now = "2026-06-06T07:27:19Z".parse().unwrap();
        let mut prices = HashMap::new();
        prices.insert(
            "cardano".to_string(),
            Decimal::from_str("0.135896").unwrap(),
        );
        let manual_row = ManualAssetAccountRow {
            account_id,
            wallet_id,
            label: crate::wallets::Label::parse_with_limit(
                "ADA Account 1",
                crate::wallets::ACCOUNT_LABEL_MAX_LENGTH,
            )
            .unwrap(),
            asset_id: crate::asset_capabilities::AssetId::owned("cardano".to_string()).unwrap(),
            network_id: crate::asset_capabilities::unsynced::UnsyncedNetworkId::parse(
                "cardano-mainnet",
            )
            .unwrap(),
            unit_code: crate::wallets::ValidatedManualAssetUnitCode::parse("ADA").unwrap(),
            decimal_precision: crate::wallets::ManualAssetDisplayScale::from_u8(6),
            symbol: None,
            asset_name: "Cardano".to_string(),
            network_name: "Cardano".to_string(),
            coingecko_id: crate::asset_capabilities::unsynced::CoingeckoAssetId::parse("cardano")
                .unwrap(),
            asset_source: "bitgarth_catalog".to_string(),
            precision_source: "bitgarth_catalog".to_string(),
            coingecko_platform_id: None,
            provider_platform_asset_ref: None,
            created_at: now,
            updated_at: now,
        };
        let mut wallet = WalletView {
            id: wallet_id,
            label: "Manual".to_string(),
            master_fingerprint: None,
            logical_account_count: 1,
            has_accessors: false,
            balances: vec![],
            accounts: vec![AccountView::Manual(
                crate::backend::ManualAssetAccountView {
                    account_id,
                    account_state: crate::backend::AccountStateView::Active,
                    label: "ADA Account 1".to_string(),
                    asset_instance_id: crate::asset_views::ManualAssetInstanceIdView {
                        asset_id: "cardano".to_string(),
                        network_id: "cardano-mainnet".to_string(),
                    },
                    unit_code: "ADA".to_string(),
                    asset_name: "Cardano".to_string(),
                    network_name: "Cardano".to_string(),
                    decimal_precision: 6,
                    symbol: None,
                    balance_state: AccountBalanceStateView::Known {
                        amount: BalanceAmountView {
                            raw_value: "1000000".to_string(),
                            formatted_value: "1".to_string(),
                        },
                    },
                    current_value: None,
                },
            )],
            value_summary: None,
        };

        let (priced, total, sum) = value_one_wallet(&mut wallet, &[manual_row], &prices, eur);

        assert_eq!(priced, 1);
        assert_eq!(total, 1);
        assert_eq!(sum, Some(Decimal::from_str("0.135896").unwrap()));
        let AccountView::Manual(manual) = &wallet.accounts[0] else {
            panic!("expected manual account");
        };
        assert_eq!(
            manual
                .current_value
                .as_ref()
                .map(|value| value.converted_value.as_str()),
            Some("0.135896")
        );
    }
}
