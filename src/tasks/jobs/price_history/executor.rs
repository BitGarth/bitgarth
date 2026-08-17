use super::PriceHistoryReconciliationParams;
use crate::models::{CurrencyCode, UserId};
use chrono::{DateTime, Utc};
use std::time::Duration;

const HISTORICAL_BACKFILL_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const RATE_LIMIT_RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(5)];
const FRESHNESS_WINDOW_DAYS: i64 = 7;
const SUCCESS_EMPTY_COOLDOWN_SECS: i64 = 24 * 60 * 60;
const RATE_LIMIT_EVIDENCE_COOLDOWN_SECS: i64 = 15 * 60;
const PUBLIC_KEYLESS_LICENSE_SCOPE: &str = "public_keyless";
static PUBLIC_KEYLESS_HISTORY_REQUEST_LANE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PriceHistorySkipReason {
    PriceFetchingDisabled,
    HistoricalBackfillDisabled,
}

fn evaluate_gates(
    price_fetching_enabled: bool,
    historical_backfill_enabled: bool,
) -> Result<(), PriceHistorySkipReason> {
    if !price_fetching_enabled {
        return Err(PriceHistorySkipReason::PriceFetchingDisabled);
    }
    if !historical_backfill_enabled {
        return Err(PriceHistorySkipReason::HistoricalBackfillDisabled);
    }
    Ok(())
}

pub(crate) fn run_price_history_reconciliation(
    user_id: UserId,
    params: PriceHistoryReconciliationParams,
) -> Result<String, String> {
    let mut runtime = RealPriceHistoryReconciliationRuntime;
    run_price_history_reconciliation_with_runtime(user_id, params, &mut runtime)
}

trait PriceHistoryReconciliationRuntime {
    fn load_price_fetching_enabled(&mut self, user_id: UserId) -> Result<bool, String>;
    fn load_historical_backfill_enabled(&mut self, user_id: UserId) -> Result<bool, String>;
    fn load_currency(&mut self, user_id: UserId) -> Result<CurrencyCode, String>;
    fn load_work(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<super::work_selection::PriceHistoryAssetWork>, String>;
    fn run_backfill_for_work(
        &mut self,
        user_id: UserId,
        currency: CurrencyCode,
        work: &[super::work_selection::PriceHistoryAssetWork],
    ) -> Result<(), String>;
}

struct RealPriceHistoryReconciliationRuntime;

impl PriceHistoryReconciliationRuntime for RealPriceHistoryReconciliationRuntime {
    fn load_price_fetching_enabled(&mut self, user_id: UserId) -> Result<bool, String> {
        crate::db::get_price_fetching_enabled(user_id)
            .map_err(|err| format!("load price-fetching preference failed: {err}"))
    }

    fn load_historical_backfill_enabled(&mut self, user_id: UserId) -> Result<bool, String> {
        crate::payments::entitlements::load_feature_entitlements(user_id, chrono::Utc::now())
            .map(|entitlements| entitlements.historical_backfill_enabled)
            .map_err(|err| format!("load feature entitlements failed: {err}"))
    }

    fn load_currency(&mut self, user_id: UserId) -> Result<CurrencyCode, String> {
        let settings = crate::db::load_settings(user_id)
            .map_err(|err| format!("load settings failed: {err}"))?;
        Ok(settings.currency.unwrap_or_else(|| {
            crate::settings::default_currency(settings.language.unwrap_or_default())
        }))
    }

    fn load_work(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<super::work_selection::PriceHistoryAssetWork>, String> {
        super::work_selection::load_user_price_history_work(user_id)
            .map_err(|err| format!("load price history work failed: {err}"))
    }

    fn run_backfill_for_work(
        &mut self,
        user_id: UserId,
        currency: CurrencyCode,
        work: &[super::work_selection::PriceHistoryAssetWork],
    ) -> Result<(), String> {
        run_backfill_for_work(user_id, currency, work)
    }
}

fn run_price_history_reconciliation_with_runtime<R: PriceHistoryReconciliationRuntime>(
    user_id: UserId,
    params: PriceHistoryReconciliationParams,
    runtime: &mut R,
) -> Result<String, String> {
    let price_fetching_enabled = runtime.load_price_fetching_enabled(user_id)?;
    let historical_backfill_enabled = runtime.load_historical_backfill_enabled(user_id)?;
    match evaluate_gates(price_fetching_enabled, historical_backfill_enabled) {
        Ok(()) => {}
        Err(reason) => {
            tracing::debug!(
                user_id = %user_id,
                reason = ?reason,
                trigger = ?params.reason,
                "price history: reconciliation skipped"
            );
            return Ok(format!("skipped: {reason:?}"));
        }
    }

    let currency = runtime.load_currency(user_id)?;
    let work = runtime.load_work(user_id)?;
    runtime.run_backfill_for_work(user_id, currency, &work)?;
    Ok(format!("processed {} asset(s)", work.len()))
}

fn history_horizon_for_credential_mode(
    credential_mode: &crate::integrations::coingecko::CoinGeckoCredentialMode,
) -> super::planner::PriceHistoryHorizon {
    match credential_mode {
        crate::integrations::coingecko::CoinGeckoCredentialMode::PublicKeyless => {
            super::planner::PriceHistoryHorizon::PublicKeyless
        }
        crate::integrations::coingecko::CoinGeckoCredentialMode::Pro { .. } => {
            super::planner::PriceHistoryHorizon::Pro
        }
    }
}

fn run_backfill_for_work(
    user_id: UserId,
    currency: CurrencyCode,
    work: &[super::work_selection::PriceHistoryAssetWork],
) -> Result<(), String> {
    if work.is_empty() {
        return Ok(());
    }

    let prices_conn =
        crate::db::initialize_prices_db().map_err(|err| format!("open prices db failed: {err}"))?;
    let credential_mode = crate::services::current_prices::credential_mode_for_user(user_id)
        .map_err(|err| format!("load CoinGecko credential mode failed: {err}"))?;
    let horizon = history_horizon_for_credential_mode(&credential_mode);
    let client = crate::traces::client::TracedBlockingClient::builder(
        crate::traces::client::IntegrationLabel::new("coingecko-price-history"),
        user_id,
    )
    .redact_headers(&["x-cg-pro-api-key", "x-cg-demo-api-key"])
    .configure(|builder| builder.timeout(HISTORICAL_BACKFILL_REQUEST_TIMEOUT))
    .build()
    .map_err(|err| format!("build CoinGecko traced client failed: {err}"))?;
    let coingecko = crate::integrations::coingecko::client::CoingeckoClient::from_credential_mode(
        client,
        credential_mode,
    )
    .map_err(|err| format!("build CoinGecko client failed: {err}"))?;
    let now = chrono::Utc::now();
    let today = now.date_naive();
    let mut deps = RealPriceHistoryBackfillDeps {
        conn: &prices_conn,
        coingecko: &coingecko,
    };

    run_backfill_for_work_with_deps(&mut deps, currency, work, today, horizon, now)
}

fn run_backfill_for_work_with_deps<D: PriceHistoryBackfillDeps>(
    deps: &mut D,
    currency: CurrencyCode,
    work: &[super::work_selection::PriceHistoryAssetWork],
    today: chrono::NaiveDate,
    horizon: super::planner::PriceHistoryHorizon,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let freshness_plan =
        load_freshness_span_work_plan_with_deps(deps, currency, work, today, horizon, now)?;
    if !freshness_plan.work.is_empty() {
        return execute_fair_span_work_with_deps(deps, currency, freshness_plan.work);
    }
    if freshness_plan.has_rate_limit_cooldown_blocker {
        return Ok(());
    }

    let fair_work = load_fair_span_work_with_deps(deps, currency, work, today, horizon)?;
    execute_fair_span_work_with_deps(deps, currency, fair_work)
}

fn asset_key(item: &super::work_selection::PriceHistoryAssetWork) -> (String, String) {
    (item.asset_id.clone(), item.provider_asset_id.clone())
}

fn execute_fair_span_work_with_deps<D: PriceHistoryBackfillDeps>(
    deps: &mut D,
    currency: CurrencyCode,
    fair_work: Vec<PriceHistoryAssetSpanWork>,
) -> Result<(), String> {
    let mut stopped_assets = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::from(fair_work);

    while let Some(item) = queue.pop_front() {
        let key = asset_key(&item.item);
        if stopped_assets.contains(&key) {
            continue;
        }

        let outcome = fetch_and_store_missing_span(deps, currency, &item.item, item.span)?;
        match outcome.control {
            BackfillControl::Continue => {
                let insert_at = queue
                    .iter()
                    .position(|queued| asset_key(&queued.item) == key)
                    .unwrap_or(queue.len());
                for (offset, deferred) in outcome.deferred.into_iter().enumerate() {
                    queue.insert(insert_at + offset, deferred);
                }
            }
            BackfillControl::StopAsset => {
                stopped_assets.insert(key);
            }
            BackfillControl::StopRun => break,
        }
    }
    Ok(())
}

trait PriceHistoryBackfillDeps {
    fn license_scope(&self) -> &str;
    fn retrieved_at(&self) -> DateTime<Utc>;
    fn load_daily_price_dates(
        &mut self,
        query: &crate::db::DailyPriceDateQuery,
    ) -> Result<Vec<chrono::NaiveDate>, String>;
    fn latest_historical_price_attempt(
        &mut self,
        query: &crate::db::HistoricalPriceAttemptQuery,
    ) -> Result<Option<crate::db::HistoricalPriceAttemptRecord>, String>;
    fn latest_historical_price_cooldown_attempt(
        &mut self,
        query: &crate::db::HistoricalPriceAttemptCooldownQuery,
    ) -> Result<Option<crate::db::HistoricalPriceAttemptRecord>, String>;
    fn fetch_daily_prices(
        &mut self,
        provider_asset_id: &str,
        quote_currency: &str,
        from_unix_seconds: i64,
        to_unix_seconds: i64,
    ) -> Result<Vec<crate::integrations::coingecko::CoinGeckoDailyPrice>, PriceHistoryFetchError>;
    fn upsert_daily_price_points(
        &mut self,
        rows: &[crate::db::DailyPricePointUpsert],
    ) -> Result<(), String>;
    fn upsert_historical_price_attempt(
        &mut self,
        row: crate::db::HistoricalPriceAttemptUpsert,
    ) -> Result<(), String>;
    fn sleep_after_rate_limit(&mut self, delay: Duration);
}

struct RealPriceHistoryBackfillDeps<'a> {
    conn: &'a rusqlite::Connection,
    coingecko: &'a crate::integrations::coingecko::client::CoingeckoClient,
}

impl PriceHistoryBackfillDeps for RealPriceHistoryBackfillDeps<'_> {
    fn license_scope(&self) -> &str {
        self.coingecko.license_scope()
    }

    fn retrieved_at(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn load_daily_price_dates(
        &mut self,
        query: &crate::db::DailyPriceDateQuery,
    ) -> Result<Vec<chrono::NaiveDate>, String> {
        crate::db::load_daily_price_dates(self.conn, query)
            .map_err(|err| format!("load daily price dates failed: {err}"))
    }

    fn latest_historical_price_attempt(
        &mut self,
        query: &crate::db::HistoricalPriceAttemptQuery,
    ) -> Result<Option<crate::db::HistoricalPriceAttemptRecord>, String> {
        crate::db::latest_historical_price_attempt(self.conn, query)
            .map_err(|err| format!("load historical price attempt failed: {err}"))
    }

    fn latest_historical_price_cooldown_attempt(
        &mut self,
        query: &crate::db::HistoricalPriceAttemptCooldownQuery,
    ) -> Result<Option<crate::db::HistoricalPriceAttemptRecord>, String> {
        crate::db::latest_historical_price_cooldown_attempt(self.conn, query)
            .map_err(|err| format!("load historical price cooldown attempt failed: {err}"))
    }

    fn fetch_daily_prices(
        &mut self,
        provider_asset_id: &str,
        quote_currency: &str,
        from_unix_seconds: i64,
        to_unix_seconds: i64,
    ) -> Result<Vec<crate::integrations::coingecko::CoinGeckoDailyPrice>, PriceHistoryFetchError>
    {
        self.coingecko
            .market_chart_range(
                provider_asset_id,
                quote_currency,
                from_unix_seconds,
                to_unix_seconds,
            )
            .map_err(classify_coingecko_error)?
            .into_daily_prices()
            .map_err(PriceHistoryFetchError::Failed)
    }

    fn upsert_daily_price_points(
        &mut self,
        rows: &[crate::db::DailyPricePointUpsert],
    ) -> Result<(), String> {
        crate::db::upsert_daily_price_points(self.conn, rows)
            .map_err(|err| format!("daily price upsert failed: {err}"))
    }

    fn upsert_historical_price_attempt(
        &mut self,
        row: crate::db::HistoricalPriceAttemptUpsert,
    ) -> Result<(), String> {
        crate::db::upsert_historical_price_attempt(self.conn, row)
            .map_err(|err| format!("historical price attempt upsert failed: {err}"))
    }

    fn sleep_after_rate_limit(&mut self, delay: Duration) {
        std::thread::sleep(delay);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackfillControl {
    Continue,
    StopAsset,
    StopRun,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PriceHistoryAssetSpanWork {
    item: super::work_selection::PriceHistoryAssetWork,
    span: super::planner::DateSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FreshnessPriority {
    NoRecentAttempt,
    RateLimitedRecentAttempt,
    SuccessEmptyCooldown,
    Covered,
    StoppedAsset,
}

impl FreshnessPriority {
    pub(crate) fn sort_rank(self) -> u8 {
        match self {
            FreshnessPriority::NoRecentAttempt => 0,
            FreshnessPriority::RateLimitedRecentAttempt => 1,
            FreshnessPriority::SuccessEmptyCooldown => 2,
            FreshnessPriority::Covered => 3,
            FreshnessPriority::StoppedAsset => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PriceHistoryAttemptEvidence {
    pub(crate) status: crate::db::HistoricalPriceAttemptStatus,
    pub(crate) attempted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FreshnessAssetCandidate {
    pub(crate) item: super::work_selection::PriceHistoryAssetWork,
    pub(crate) missing_span: super::planner::DateSpan,
    pub(crate) latest_attempt: Option<PriceHistoryAttemptEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FreshnessWorkClassification {
    pub(crate) priority: FreshnessPriority,
    pub(crate) item: super::work_selection::PriceHistoryAssetWork,
    pub(crate) missing_span: Option<super::planner::DateSpan>,
}

impl FreshnessWorkClassification {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Task 3 wires freshness scheduling")
    )]
    pub(crate) fn item(&self) -> &super::work_selection::PriceHistoryAssetWork {
        &self.item
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "Task 3 wires freshness scheduling")
    )]
    pub(crate) fn priority(&self) -> FreshnessPriority {
        self.priority
    }

    pub(crate) fn sort_key(
        &self,
    ) -> (
        u8,
        String,
        String,
        chrono::NaiveDate,
        Option<chrono::NaiveDate>,
        Option<chrono::NaiveDate>,
    ) {
        (
            self.priority.sort_rank(),
            self.item.asset_id.clone(),
            self.item.provider_asset_id.clone(),
            self.item.first_owned_date,
            self.missing_span.map(|span| span.start),
            self.missing_span.map(|span| span.end),
        )
    }
}

pub(crate) fn freshness_window(
    today: chrono::NaiveDate,
    lower_bound: chrono::NaiveDate,
) -> super::planner::DateSpan {
    super::planner::DateSpan {
        start: lower_bound.max(today - chrono::Duration::days(FRESHNESS_WINDOW_DAYS - 1)),
        end: today,
    }
}

pub(crate) fn freshness_missing_span(
    window: super::planner::DateSpan,
    existing: &[chrono::NaiveDate],
) -> Option<super::planner::DateSpan> {
    missing_daily_spans_newest_first(window, existing).next()
}

pub(crate) fn classify_freshness_candidate(
    candidate: FreshnessAssetCandidate,
    now: DateTime<Utc>,
) -> FreshnessWorkClassification {
    let (priority, include_missing_span) = match candidate.latest_attempt {
        None => (FreshnessPriority::NoRecentAttempt, true),
        Some(latest_attempt) => classify_freshness_attempt(latest_attempt, now),
    };

    FreshnessWorkClassification {
        priority,
        item: candidate.item,
        missing_span: include_missing_span.then_some(candidate.missing_span),
    }
}

fn classify_freshness_attempt(
    latest_attempt: PriceHistoryAttemptEvidence,
    now: DateTime<Utc>,
) -> (FreshnessPriority, bool) {
    match latest_attempt.status {
        crate::db::HistoricalPriceAttemptStatus::SuccessWithPrices => {
            (FreshnessPriority::NoRecentAttempt, true)
        }
        crate::db::HistoricalPriceAttemptStatus::SuccessEmpty => {
            if attempt_age_secs(now, latest_attempt.attempted_at) < SUCCESS_EMPTY_COOLDOWN_SECS {
                (FreshnessPriority::SuccessEmptyCooldown, false)
            } else {
                (FreshnessPriority::NoRecentAttempt, true)
            }
        }
        crate::db::HistoricalPriceAttemptStatus::RateLimited => {
            if attempt_age_secs(now, latest_attempt.attempted_at)
                < RATE_LIMIT_EVIDENCE_COOLDOWN_SECS
            {
                (FreshnessPriority::RateLimitedRecentAttempt, false)
            } else {
                (FreshnessPriority::NoRecentAttempt, true)
            }
        }
        crate::db::HistoricalPriceAttemptStatus::TransientFailure => {
            (FreshnessPriority::NoRecentAttempt, true)
        }
        crate::db::HistoricalPriceAttemptStatus::PermanentFailure => {
            (FreshnessPriority::StoppedAsset, false)
        }
    }
}

fn attempt_age_secs(now: DateTime<Utc>, attempted_at: DateTime<Utc>) -> i64 {
    now.signed_duration_since(attempted_at).num_seconds()
}

fn cooldown_min_attempted_at(now: DateTime<Utc>) -> DateTime<Utc> {
    let cooldown_secs = SUCCESS_EMPTY_COOLDOWN_SECS.max(RATE_LIMIT_EVIDENCE_COOLDOWN_SECS);
    now - chrono::Duration::seconds(cooldown_secs)
}

fn attempt_evidence(
    attempt: crate::db::HistoricalPriceAttemptRecord,
) -> PriceHistoryAttemptEvidence {
    PriceHistoryAttemptEvidence {
        status: attempt.status,
        attempted_at: attempt.attempted_at,
    }
}

fn select_freshness_attempt_evidence(
    covering_attempt: Option<crate::db::HistoricalPriceAttemptRecord>,
    cooldown_attempt: Option<crate::db::HistoricalPriceAttemptRecord>,
    now: DateTime<Utc>,
) -> Option<PriceHistoryAttemptEvidence> {
    if let Some(attempt) = covering_attempt.as_ref() {
        let evidence = PriceHistoryAttemptEvidence {
            status: attempt.status,
            attempted_at: attempt.attempted_at,
        };
        let (_, include_missing_span) = classify_freshness_attempt(evidence, now);
        if attempt.status == crate::db::HistoricalPriceAttemptStatus::PermanentFailure
            || include_missing_span
        {
            return Some(evidence);
        }
    }

    cooldown_attempt
        .map(attempt_evidence)
        .or_else(|| covering_attempt.map(attempt_evidence))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FreshnessSpanWorkPlan {
    work: Vec<PriceHistoryAssetSpanWork>,
    has_rate_limit_cooldown_blocker: bool,
}

#[cfg(test)]
fn load_freshness_span_work_with_deps<D: PriceHistoryBackfillDeps>(
    deps: &mut D,
    currency: CurrencyCode,
    work: &[super::work_selection::PriceHistoryAssetWork],
    today: chrono::NaiveDate,
    horizon: super::planner::PriceHistoryHorizon,
    now: DateTime<Utc>,
) -> Result<Vec<PriceHistoryAssetSpanWork>, String> {
    load_freshness_span_work_plan_with_deps(deps, currency, work, today, horizon, now)
        .map(|plan| plan.work)
}

fn load_freshness_span_work_plan_with_deps<D: PriceHistoryBackfillDeps>(
    deps: &mut D,
    currency: CurrencyCode,
    work: &[super::work_selection::PriceHistoryAssetWork],
    today: chrono::NaiveDate,
    horizon: super::planner::PriceHistoryHorizon,
    now: DateTime<Utc>,
) -> Result<FreshnessSpanWorkPlan, String> {
    let mut classifications = Vec::new();

    for item in work {
        let requested_start =
            super::planner::requested_start_date_for_horizon(item.first_owned_date, today, horizon);
        let window = freshness_window(today, requested_start);
        let existing = deps.load_daily_price_dates(&daily_price_date_query(
            deps.license_scope(),
            currency,
            item,
            window.start,
            window.end,
        ))?;

        let Some(missing_span) = freshness_missing_span(window, &existing) else {
            classifications.push(FreshnessWorkClassification {
                priority: FreshnessPriority::Covered,
                item: item.clone(),
                missing_span: None,
            });
            continue;
        };

        let covering_attempt =
            deps.latest_historical_price_attempt(&crate::db::HistoricalPriceAttemptQuery {
                asset_id: item.asset_id.clone(),
                provider: "coingecko".to_string(),
                from_date: missing_span.start,
                to_date: missing_span.end,
            })?;
        let cooldown_attempt = deps.latest_historical_price_cooldown_attempt(
            &crate::db::HistoricalPriceAttemptCooldownQuery {
                asset_id: item.asset_id.clone(),
                provider: "coingecko".to_string(),
                min_attempted_at: cooldown_min_attempted_at(now),
            },
        )?;
        let latest_attempt =
            select_freshness_attempt_evidence(covering_attempt, cooldown_attempt, now);

        classifications.push(classify_freshness_candidate(
            FreshnessAssetCandidate {
                item: item.clone(),
                missing_span,
                latest_attempt,
            },
            now,
        ));
    }

    classifications.sort_by_key(FreshnessWorkClassification::sort_key);
    let has_rate_limit_cooldown_blocker = classifications.iter().any(|classification| {
        classification.priority == FreshnessPriority::RateLimitedRecentAttempt
            && classification.missing_span.is_none()
    });
    let work = classifications
        .into_iter()
        .filter_map(|classification| {
            classification
                .missing_span
                .map(|span| PriceHistoryAssetSpanWork {
                    item: classification.item,
                    span,
                })
        })
        .collect();

    Ok(FreshnessSpanWorkPlan {
        work,
        has_rate_limit_cooldown_blocker,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FairSpanExecutionOutcome {
    control: BackfillControl,
    deferred: Vec<PriceHistoryAssetSpanWork>,
}

fn load_fair_span_work_with_deps<D: PriceHistoryBackfillDeps>(
    deps: &mut D,
    currency: CurrencyCode,
    work: &[super::work_selection::PriceHistoryAssetWork],
    today: chrono::NaiveDate,
    horizon: super::planner::PriceHistoryHorizon,
) -> Result<Vec<PriceHistoryAssetSpanWork>, String> {
    let mut per_asset_spans = Vec::new();

    for (asset_index, item) in work.iter().enumerate() {
        let requested_start =
            super::planner::requested_start_date_for_horizon(item.first_owned_date, today, horizon);
        let mut spans = Vec::new();

        for range in super::planner::backward_provider_ranges(requested_start, today) {
            let existing = deps.load_daily_price_dates(&daily_price_date_query(
                deps.license_scope(),
                currency,
                item,
                range.start,
                range.end,
            ))?;

            spans.extend(super::planner::missing_daily_spans_newest_first(
                super::planner::DateSpan {
                    start: range.start,
                    end: range.end,
                },
                &existing,
            ));
        }

        per_asset_spans.push((asset_index, spans));
    }

    Ok(super::planner::fair_missing_span_work(per_asset_spans)
        .into_iter()
        .map(|work_item| PriceHistoryAssetSpanWork {
            item: work[work_item.asset_index].clone(),
            span: work_item.span,
        })
        .collect())
}

fn fetch_and_store_missing_span<D: PriceHistoryBackfillDeps>(
    deps: &mut D,
    currency: CurrencyCode,
    item: &super::work_selection::PriceHistoryAssetWork,
    span: super::planner::DateSpan,
) -> Result<FairSpanExecutionOutcome, String> {
    fetch_and_store_span_with_retries(deps, currency, item, span)
}

fn daily_price_date_query(
    license_scope: &str,
    currency: CurrencyCode,
    item: &super::work_selection::PriceHistoryAssetWork,
    start: chrono::NaiveDate,
    end: chrono::NaiveDate,
) -> crate::db::DailyPriceDateQuery {
    crate::db::DailyPriceDateQuery {
        asset_id: item.asset_id.clone(),
        quote_currency: currency,
        provider_asset_id: item.provider_asset_id.clone(),
        license_scope: license_scope.to_string(),
        start,
        end,
    }
}

fn fetch_and_store_span_with_retries<D: PriceHistoryBackfillDeps>(
    deps: &mut D,
    currency: CurrencyCode,
    item: &super::work_selection::PriceHistoryAssetWork,
    span: super::planner::DateSpan,
) -> Result<FairSpanExecutionOutcome, String> {
    let mut retry_delays = RATE_LIMIT_RETRY_DELAYS.iter();
    let mut current_span = span;
    let mut deferred = Vec::new();
    loop {
        let missing_spans = missing_spans_after_exact_recheck(deps, currency, item, current_span)?;
        if missing_spans.is_empty() {
            return Ok(FairSpanExecutionOutcome {
                control: BackfillControl::Continue,
                deferred,
            });
        }

        if missing_spans.len() != 1 || missing_spans[0] != current_span {
            current_span = missing_spans[0];
            retry_delays = RATE_LIMIT_RETRY_DELAYS.iter();
            let mut newly_deferred = missing_spans
                .into_iter()
                .skip(1)
                .map(|span| PriceHistoryAssetSpanWork {
                    item: item.clone(),
                    span,
                })
                .collect::<Vec<_>>();
            if !newly_deferred.is_empty() {
                newly_deferred.extend(deferred);
                deferred = newly_deferred;
            }
            continue;
        }

        match fetch_and_store_span(deps, currency, item, current_span) {
            Ok(()) => {
                return Ok(FairSpanExecutionOutcome {
                    control: BackfillControl::Continue,
                    deferred,
                });
            }
            Err(PriceHistoryFetchError::HistoryLimit) => {
                record_terminal_historical_attempt(
                    deps,
                    item,
                    current_span,
                    crate::db::HistoricalPriceAttemptStatus::PermanentFailure,
                    "history_limit",
                );
                return Ok(FairSpanExecutionOutcome {
                    control: BackfillControl::StopAsset,
                    deferred: Vec::new(),
                });
            }
            Err(PriceHistoryFetchError::NotFound(message)) => {
                record_terminal_historical_attempt(
                    deps,
                    item,
                    current_span,
                    crate::db::HistoricalPriceAttemptStatus::PermanentFailure,
                    "not_found",
                );
                tracing::warn!(
                    asset_id = %item.asset_id,
                    provider_asset_id = %item.provider_asset_id,
                    error = %message,
                    "price history: asset not found by provider"
                );
                return Ok(FairSpanExecutionOutcome {
                    control: BackfillControl::StopAsset,
                    deferred: Vec::new(),
                });
            }
            Err(PriceHistoryFetchError::RateLimited(message)) => {
                if let Some(delay) = retry_delays.next() {
                    tracing::debug!(
                        asset_id = %item.asset_id,
                        provider_asset_id = %item.provider_asset_id,
                        error = %message,
                        delay_ms = delay.as_millis(),
                        "price history: rate limited; backing off before retry"
                    );
                    deps.sleep_after_rate_limit(*delay);
                } else {
                    tracing::warn!(
                        asset_id = %item.asset_id,
                        provider_asset_id = %item.provider_asset_id,
                        error = %message,
                        "price history: rate limit persisted after retries"
                    );
                    record_terminal_historical_attempt(
                        deps,
                        item,
                        current_span,
                        crate::db::HistoricalPriceAttemptStatus::RateLimited,
                        "rate_limited",
                    );
                    return Ok(FairSpanExecutionOutcome {
                        control: BackfillControl::StopRun,
                        deferred: Vec::new(),
                    });
                }
            }
            Err(PriceHistoryFetchError::Failed(message)) => {
                record_terminal_historical_attempt(
                    deps,
                    item,
                    current_span,
                    crate::db::HistoricalPriceAttemptStatus::TransientFailure,
                    "transient_failure",
                );
                tracing::warn!(
                    asset_id = %item.asset_id,
                    provider_asset_id = %item.provider_asset_id,
                    error = %message,
                    "price history: asset span failed"
                );
                return Ok(FairSpanExecutionOutcome {
                    control: BackfillControl::Continue,
                    deferred,
                });
            }
        }
    }
}

fn record_terminal_historical_attempt<D: PriceHistoryBackfillDeps>(
    deps: &mut D,
    item: &super::work_selection::PriceHistoryAssetWork,
    span: super::planner::DateSpan,
    status: crate::db::HistoricalPriceAttemptStatus,
    error_code: &str,
) {
    let row = historical_attempt_upsert(
        item,
        span,
        status,
        0,
        deps.retrieved_at(),
        None,
        Some(error_code),
    );
    if let Err(err) = deps.upsert_historical_price_attempt(row) {
        tracing::warn!(
            asset_id = %item.asset_id,
            provider_asset_id = %item.provider_asset_id,
            error = %err,
            "price history: historical attempt upsert failed"
        );
    }
}

fn missing_spans_after_exact_recheck<D: PriceHistoryBackfillDeps>(
    deps: &mut D,
    currency: CurrencyCode,
    item: &super::work_selection::PriceHistoryAssetWork,
    span: super::planner::DateSpan,
) -> Result<Vec<super::planner::DateSpan>, String> {
    let existing = deps.load_daily_price_dates(&daily_price_date_query(
        deps.license_scope(),
        currency,
        item,
        span.start,
        span.end,
    ))?;
    Ok(missing_daily_spans_newest_first(span, &existing).collect())
}

fn classify_coingecko_error(
    err: crate::integrations::coingecko::client::CoingeckoError,
) -> PriceHistoryFetchError {
    if err.is_history_limit() {
        PriceHistoryFetchError::HistoryLimit
    } else if err.is_rate_limited() {
        PriceHistoryFetchError::RateLimited(format!("CoinGecko rate limited: {err}"))
    } else if is_coingecko_not_found(&err) {
        PriceHistoryFetchError::NotFound(format!("CoinGecko asset not found: {err}"))
    } else {
        PriceHistoryFetchError::Failed(format!("CoinGecko request failed: {err}"))
    }
}

fn is_coingecko_not_found(err: &crate::integrations::coingecko::client::CoingeckoError) -> bool {
    matches!(
        err,
        crate::integrations::coingecko::client::CoingeckoError::Api {
            status_code: 404,
            ..
        } | crate::integrations::coingecko::client::CoingeckoError::UnexpectedResponse {
            status_code: 404,
            ..
        }
    )
}

fn missing_daily_spans_newest_first(
    requested: super::planner::DateSpan,
    existing: &[chrono::NaiveDate],
) -> impl Iterator<Item = super::planner::DateSpan> {
    let missing = super::planner::missing_daily_spans(requested, existing);
    missing.into_iter().rev()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PriceHistoryFetchError {
    HistoryLimit,
    RateLimited(String),
    NotFound(String),
    Failed(String),
}

fn historical_attempt_upsert(
    item: &super::work_selection::PriceHistoryAssetWork,
    span: super::planner::DateSpan,
    status: crate::db::HistoricalPriceAttemptStatus,
    rows_returned: usize,
    attempted_at: DateTime<Utc>,
    next_retry_after: Option<DateTime<Utc>>,
    error_code: Option<&str>,
) -> crate::db::HistoricalPriceAttemptUpsert {
    crate::db::HistoricalPriceAttemptUpsert {
        asset_id: item.asset_id.clone(),
        provider: "coingecko".to_string(),
        from_date: span.start,
        to_date: span.end,
        status,
        attempted_at,
        rows_returned: rows_returned.min(u32::MAX as usize) as u32,
        next_retry_after,
        error_code: error_code.map(str::to_string),
    }
}

fn unix_day_start(date: chrono::NaiveDate) -> Result<i64, String> {
    date.and_hms_opt(0, 0, 0)
        .map(|dt| dt.and_utc().timestamp())
        .ok_or_else(|| format!("invalid UTC date: {date}"))
}

fn fetch_and_store_span<D: PriceHistoryBackfillDeps>(
    deps: &mut D,
    currency: CurrencyCode,
    item: &super::work_selection::PriceHistoryAssetWork,
    span: super::planner::DateSpan,
) -> Result<(), PriceHistoryFetchError> {
    let from = unix_day_start(span.start).map_err(PriceHistoryFetchError::Failed)?;
    let to = unix_day_start(span.end + chrono::Duration::days(1))
        .map_err(PriceHistoryFetchError::Failed)?;
    let quote_currency = currency.code().to_ascii_lowercase();
    let retrieved_at = deps.retrieved_at();
    let rows = fetch_daily_prices_with_request_lane(
        deps,
        &item.provider_asset_id,
        quote_currency.as_str(),
        from,
        to,
    )?
    .into_iter()
    .filter(|price| price.date_utc >= span.start && price.date_utc <= span.end)
    .map(|price| crate::db::DailyPricePointUpsert {
        asset_id: item.asset_id.clone(),
        quote_currency: currency,
        price_time_utc: price.price_time,
        date_utc: price.date_utc,
        price: price.price.as_decimal(),
        provider_asset_id: item.provider_asset_id.clone(),
        provider_quote_id: Some(quote_currency.clone()),
        license_scope: deps.license_scope().to_string(),
        retrieved_at,
    })
    .collect::<Vec<_>>();

    deps.upsert_daily_price_points(&rows)
        .map_err(PriceHistoryFetchError::Failed)?;

    let status = if rows.is_empty() {
        crate::db::HistoricalPriceAttemptStatus::SuccessEmpty
    } else {
        crate::db::HistoricalPriceAttemptStatus::SuccessWithPrices
    };
    let attempt =
        historical_attempt_upsert(item, span, status, rows.len(), retrieved_at, None, None);
    deps.upsert_historical_price_attempt(attempt)
        .map_err(PriceHistoryFetchError::Failed)
}

fn fetch_daily_prices_with_request_lane<D: PriceHistoryBackfillDeps>(
    deps: &mut D,
    provider_asset_id: &str,
    quote_currency: &str,
    from_unix_seconds: i64,
    to_unix_seconds: i64,
) -> Result<Vec<crate::integrations::coingecko::CoinGeckoDailyPrice>, PriceHistoryFetchError> {
    if deps.license_scope() != PUBLIC_KEYLESS_LICENSE_SCOPE {
        return deps.fetch_daily_prices(
            provider_asset_id,
            quote_currency,
            from_unix_seconds,
            to_unix_seconds,
        );
    }

    let _guard = PUBLIC_KEYLESS_HISTORY_REQUEST_LANE.lock().map_err(|err| {
        PriceHistoryFetchError::Failed(format!("price history lane poisoned: {err}"))
    })?;
    deps.fetch_daily_prices(
        provider_asset_id,
        quote_currency,
        from_unix_seconds,
        to_unix_seconds,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DailyPricePointUpsert;
    use crate::integrations::coingecko::{CoinGeckoDailyPrice, MarketPrice};
    use crate::tasks::jobs::price_history::PriceHistoryReconciliationReason;
    use crate::tasks::jobs::price_history::planner::DateSpan;
    use crate::tasks::jobs::price_history::work_selection::PriceHistoryAssetWork;
    use chrono::{DateTime, NaiveDateTime, Utc};
    use rust_decimal::Decimal;
    use std::collections::{HashMap, VecDeque};
    use std::time::Duration;

    fn d(value: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("test date")
    }

    fn dt(value: &str) -> DateTime<Utc> {
        NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%SZ")
            .map(|dt| dt.and_utc())
            .expect("test datetime")
    }

    fn usd() -> CurrencyCode {
        CurrencyCode::from_code("USD").expect("USD should parse")
    }

    fn work(first_owned_date: chrono::NaiveDate) -> PriceHistoryAssetWork {
        PriceHistoryAssetWork {
            asset_id: "bitcoin".to_string(),
            provider_asset_id: "bitcoin".to_string(),
            first_owned_date,
        }
    }

    fn asset_work(asset_id: &str, first_owned_date: chrono::NaiveDate) -> PriceHistoryAssetWork {
        PriceHistoryAssetWork {
            asset_id: asset_id.to_string(),
            provider_asset_id: asset_id.to_string(),
            first_owned_date,
        }
    }

    fn asset_work_with_provider(
        asset_id: &str,
        provider_asset_id: &str,
        first_owned_date: chrono::NaiveDate,
    ) -> PriceHistoryAssetWork {
        PriceHistoryAssetWork {
            asset_id: asset_id.to_string(),
            provider_asset_id: provider_asset_id.to_string(),
            first_owned_date,
        }
    }

    fn run_fair_test_backfill(
        deps: &mut FakeBackfillDeps,
        work_items: &[PriceHistoryAssetWork],
        today: chrono::NaiveDate,
    ) -> Result<(), String> {
        let fair_work = load_fair_span_work_with_deps(
            deps,
            usd(),
            work_items,
            today,
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::Pro,
        )?;

        execute_fair_span_work_with_deps(deps, usd(), fair_work)
    }

    fn daily_price(date: &str, price: &str) -> CoinGeckoDailyPrice {
        let date_utc = d(date);
        CoinGeckoDailyPrice {
            price_time: date_utc.and_hms_opt(12, 0, 0).expect("test time").and_utc(),
            date_utc,
            price: MarketPrice(Decimal::from_str_exact(price).expect("test price")),
        }
    }

    fn attempt_record(
        asset_id: &str,
        from_date: chrono::NaiveDate,
        to_date: chrono::NaiveDate,
        status: crate::db::HistoricalPriceAttemptStatus,
        attempted_at: DateTime<Utc>,
    ) -> crate::db::HistoricalPriceAttemptRecord {
        crate::db::HistoricalPriceAttemptRecord {
            id: format!("{asset_id}-{from_date}-{to_date}"),
            asset_id: asset_id.to_string(),
            provider: "coingecko".to_string(),
            from_date,
            to_date,
            status,
            attempted_at,
            rows_returned: 0,
            next_retry_after: None,
            error_code: None,
        }
    }

    type AttemptLookupKey = (String, String, chrono::NaiveDate, chrono::NaiveDate);

    fn attempt_lookup_key(
        asset_id: &str,
        provider: &str,
        from_date: chrono::NaiveDate,
        to_date: chrono::NaiveDate,
    ) -> AttemptLookupKey {
        (
            asset_id.to_string(),
            provider.to_string(),
            from_date,
            to_date,
        )
    }

    fn expected_attempt_upsert(
        asset_id: &str,
        from_date: chrono::NaiveDate,
        to_date: chrono::NaiveDate,
        status: crate::db::HistoricalPriceAttemptStatus,
        rows_returned: u32,
        error_code: Option<&str>,
    ) -> crate::db::HistoricalPriceAttemptUpsert {
        crate::db::HistoricalPriceAttemptUpsert {
            asset_id: asset_id.to_string(),
            provider: "coingecko".to_string(),
            from_date,
            to_date,
            status,
            attempted_at: dt("2026-01-10T00:00:00Z"),
            rows_returned,
            next_retry_after: None,
            error_code: error_code.map(str::to_string),
        }
    }

    struct FakeBackfillDeps {
        license_scope: &'static str,
        retrieved_at: DateTime<Utc>,
        load_results: VecDeque<Vec<chrono::NaiveDate>>,
        attempt_results: HashMap<AttemptLookupKey, crate::db::HistoricalPriceAttemptRecord>,
        fetch_results: VecDeque<Result<Vec<CoinGeckoDailyPrice>, PriceHistoryFetchError>>,
        loads: Vec<(chrono::NaiveDate, chrono::NaiveDate)>,
        attempt_lookups: Vec<AttemptLookupKey>,
        cooldown_attempt_lookups: Vec<(String, String, DateTime<Utc>)>,
        fetches: Vec<(String, String, i64, i64)>,
        upserts: Vec<Vec<DailyPricePointUpsert>>,
        attempt_upserts: Vec<crate::db::HistoricalPriceAttemptUpsert>,
        sleeps: Vec<Duration>,
    }

    impl FakeBackfillDeps {
        fn seed_attempt(&mut self, record: crate::db::HistoricalPriceAttemptRecord) {
            self.attempt_results.insert(
                attempt_lookup_key(
                    &record.asset_id,
                    &record.provider,
                    record.from_date,
                    record.to_date,
                ),
                record,
            );
        }
    }

    impl Default for FakeBackfillDeps {
        fn default() -> Self {
            Self {
                license_scope: "test_license",
                retrieved_at: dt("2026-01-10T00:00:00Z"),
                load_results: VecDeque::new(),
                attempt_results: HashMap::new(),
                fetch_results: VecDeque::new(),
                loads: Vec::new(),
                attempt_lookups: Vec::new(),
                cooldown_attempt_lookups: Vec::new(),
                fetches: Vec::new(),
                upserts: Vec::new(),
                attempt_upserts: Vec::new(),
                sleeps: Vec::new(),
            }
        }
    }

    impl PriceHistoryBackfillDeps for FakeBackfillDeps {
        fn license_scope(&self) -> &str {
            self.license_scope
        }

        fn retrieved_at(&self) -> DateTime<Utc> {
            self.retrieved_at
        }

        fn load_daily_price_dates(
            &mut self,
            query: &crate::db::DailyPriceDateQuery,
        ) -> Result<Vec<chrono::NaiveDate>, String> {
            self.loads.push((query.start, query.end));
            Ok(self.load_results.pop_front().unwrap_or_default())
        }

        fn latest_historical_price_attempt(
            &mut self,
            query: &crate::db::HistoricalPriceAttemptQuery,
        ) -> Result<Option<crate::db::HistoricalPriceAttemptRecord>, String> {
            let key = attempt_lookup_key(
                &query.asset_id,
                &query.provider,
                query.from_date,
                query.to_date,
            );
            self.attempt_lookups.push(key.clone());
            Ok(self.attempt_results.get(&key).cloned())
        }

        fn latest_historical_price_cooldown_attempt(
            &mut self,
            query: &crate::db::HistoricalPriceAttemptCooldownQuery,
        ) -> Result<Option<crate::db::HistoricalPriceAttemptRecord>, String> {
            self.cooldown_attempt_lookups.push((
                query.asset_id.clone(),
                query.provider.clone(),
                query.min_attempted_at,
            ));
            Ok(self
                .attempt_results
                .values()
                .filter(|attempt| {
                    attempt.asset_id == query.asset_id
                        && attempt.provider == query.provider
                        && matches!(
                            attempt.status,
                            crate::db::HistoricalPriceAttemptStatus::SuccessEmpty
                                | crate::db::HistoricalPriceAttemptStatus::RateLimited
                        )
                        && attempt.attempted_at >= query.min_attempted_at
                })
                .max_by(|left, right| {
                    left.attempted_at
                        .cmp(&right.attempted_at)
                        .then_with(|| left.id.cmp(&right.id))
                })
                .cloned())
        }

        fn fetch_daily_prices(
            &mut self,
            provider_asset_id: &str,
            quote_currency: &str,
            from_unix_seconds: i64,
            to_unix_seconds: i64,
        ) -> Result<Vec<CoinGeckoDailyPrice>, PriceHistoryFetchError> {
            self.fetches.push((
                provider_asset_id.to_string(),
                quote_currency.to_string(),
                from_unix_seconds,
                to_unix_seconds,
            ));
            self.fetch_results
                .pop_front()
                .unwrap_or_else(|| Ok(Vec::new()))
        }

        fn upsert_daily_price_points(
            &mut self,
            rows: &[DailyPricePointUpsert],
        ) -> Result<(), String> {
            self.upserts.push(rows.to_vec());
            Ok(())
        }

        fn upsert_historical_price_attempt(
            &mut self,
            row: crate::db::HistoricalPriceAttemptUpsert,
        ) -> Result<(), String> {
            self.attempt_upserts.push(row);
            Ok(())
        }

        fn sleep_after_rate_limit(&mut self, delay: Duration) {
            self.sleeps.push(delay);
        }
    }

    struct LaneProbeDeps {
        license_scope: &'static str,
        active_fetches: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        max_active_fetches: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl PriceHistoryBackfillDeps for LaneProbeDeps {
        fn license_scope(&self) -> &str {
            self.license_scope
        }

        fn retrieved_at(&self) -> DateTime<Utc> {
            dt("2026-01-10T00:00:00Z")
        }

        fn load_daily_price_dates(
            &mut self,
            _query: &crate::db::DailyPriceDateQuery,
        ) -> Result<Vec<chrono::NaiveDate>, String> {
            Ok(Vec::new())
        }

        fn latest_historical_price_attempt(
            &mut self,
            _query: &crate::db::HistoricalPriceAttemptQuery,
        ) -> Result<Option<crate::db::HistoricalPriceAttemptRecord>, String> {
            Ok(None)
        }

        fn latest_historical_price_cooldown_attempt(
            &mut self,
            _query: &crate::db::HistoricalPriceAttemptCooldownQuery,
        ) -> Result<Option<crate::db::HistoricalPriceAttemptRecord>, String> {
            Ok(None)
        }

        fn fetch_daily_prices(
            &mut self,
            _provider_asset_id: &str,
            _quote_currency: &str,
            _from_unix_seconds: i64,
            _to_unix_seconds: i64,
        ) -> Result<Vec<CoinGeckoDailyPrice>, PriceHistoryFetchError> {
            use std::sync::atomic::Ordering;

            let active = self.active_fetches.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active_fetches.fetch_max(active, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            self.active_fetches.fetch_sub(1, Ordering::SeqCst);
            Ok(Vec::new())
        }

        fn upsert_daily_price_points(
            &mut self,
            _rows: &[DailyPricePointUpsert],
        ) -> Result<(), String> {
            Ok(())
        }

        fn upsert_historical_price_attempt(
            &mut self,
            _row: crate::db::HistoricalPriceAttemptUpsert,
        ) -> Result<(), String> {
            Ok(())
        }

        fn sleep_after_rate_limit(&mut self, _delay: Duration) {}
    }

    #[test]
    fn gates_skip_when_price_fetching_disabled() {
        assert_eq!(
            evaluate_gates(false, true),
            Err(PriceHistorySkipReason::PriceFetchingDisabled)
        );
    }

    #[test]
    fn gates_skip_when_historical_backfill_disabled() {
        assert_eq!(
            evaluate_gates(true, false),
            Err(PriceHistorySkipReason::HistoricalBackfillDisabled)
        );
    }

    #[test]
    fn gates_pass_when_both_capabilities_are_enabled() {
        assert_eq!(evaluate_gates(true, true), Ok(()));
    }

    #[test]
    fn public_keyless_fetches_are_serialized_across_threads() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};

        let active_fetches = Arc::new(AtomicUsize::new(0));
        let max_active_fetches = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let active_fetches = Arc::clone(&active_fetches);
                let max_active_fetches = Arc::clone(&max_active_fetches);
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    let mut deps = LaneProbeDeps {
                        license_scope: PUBLIC_KEYLESS_LICENSE_SCOPE,
                        active_fetches,
                        max_active_fetches,
                    };
                    start.wait();
                    fetch_daily_prices_with_request_lane(&mut deps, "bitcoin", "usd", 1, 2)
                        .expect("lane fetch should succeed");
                })
            })
            .collect::<Vec<_>>();

        start.wait();
        for handle in handles {
            handle.join().expect("thread should finish");
        }

        assert_eq!(max_active_fetches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn empty_work_returns_before_opening_backfill_dependencies() {
        assert_eq!(run_backfill_for_work(UserId::new(), usd(), &[]), Ok(()));
    }

    #[test]
    fn coingecko_404_classifies_as_not_found() {
        let err = crate::integrations::coingecko::client::CoingeckoError::Api {
            status_code: 404,
            error_code: 0,
            error_message: "not found".to_string(),
            retry_after: None,
        };

        assert!(matches!(
            classify_coingecko_error(err),
            PriceHistoryFetchError::NotFound(_)
        ));
    }

    #[test]
    fn coingecko_10012_classifies_as_history_limit() {
        let err = crate::integrations::coingecko::client::CoingeckoError::Api {
            status_code: 401,
            error_code: 10012,
            error_message: "historical data not allowed".to_string(),
            retry_after: None,
        };

        assert_eq!(
            classify_coingecko_error(err),
            PriceHistoryFetchError::HistoryLimit
        );
    }

    #[test]
    fn credential_mode_selects_history_horizon() {
        assert_eq!(
            history_horizon_for_credential_mode(
                &crate::integrations::coingecko::CoinGeckoCredentialMode::PublicKeyless
            ),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::PublicKeyless
        );

        let api_key =
            crate::models::SimpleApiKey::new("PRO_KEY".to_string()).expect("valid test key");
        assert_eq!(
            history_horizon_for_credential_mode(
                &crate::integrations::coingecko::CoinGeckoCredentialMode::Pro { api_key }
            ),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::Pro
        );
    }

    #[test]
    fn missing_spans_are_processed_newest_first() {
        let requested = DateSpan {
            start: d("2026-01-01"),
            end: d("2026-01-10"),
        };
        let existing = vec![d("2026-01-03"), d("2026-01-04"), d("2026-01-08")];

        assert_eq!(
            missing_daily_spans_newest_first(requested, &existing).collect::<Vec<_>>(),
            vec![
                DateSpan {
                    start: d("2026-01-09"),
                    end: d("2026-01-10"),
                },
                DateSpan {
                    start: d("2026-01-05"),
                    end: d("2026-01-07"),
                },
                DateSpan {
                    start: d("2026-01-01"),
                    end: d("2026-01-02"),
                },
            ]
        );
    }

    #[test]
    fn freshness_window_requests_only_newest_missing_subspan() {
        let window = freshness_window(d("2026-07-10"), d("2026-07-04"));
        let existing = vec![
            d("2026-07-01"),
            d("2026-07-02"),
            d("2026-07-03"),
            d("2026-07-04"),
            d("2026-07-08"),
            d("2026-07-09"),
            d("2026-07-10"),
        ];

        assert_eq!(
            window,
            DateSpan {
                start: d("2026-07-04"),
                end: d("2026-07-10"),
            }
        );
        assert_eq!(
            freshness_missing_span(window, &existing),
            Some(DateSpan {
                start: d("2026-07-05"),
                end: d("2026-07-07"),
            })
        );
    }

    #[test]
    fn freshness_window_does_not_request_before_lower_bound() {
        assert_eq!(
            freshness_window(d("2026-07-10"), d("2026-07-08")),
            DateSpan {
                start: d("2026-07-08"),
                end: d("2026-07-10"),
            }
        );
    }

    #[test]
    fn freshness_priority_no_recent_attempt_outranks_success_empty() {
        let missing_span = DateSpan {
            start: d("2026-07-05"),
            end: d("2026-07-07"),
        };
        let now = dt("2026-07-10T12:00:00Z");
        let mut classifications = [
            classify_freshness_candidate(
                FreshnessAssetCandidate {
                    item: asset_work("BBB", d("2026-01-01")),
                    missing_span,
                    latest_attempt: Some(PriceHistoryAttemptEvidence {
                        status: crate::db::HistoricalPriceAttemptStatus::SuccessEmpty,
                        attempted_at: dt("2026-07-10T11:30:00Z"),
                    }),
                },
                now,
            ),
            classify_freshness_candidate(
                FreshnessAssetCandidate {
                    item: asset_work("SOL", d("2026-01-01")),
                    missing_span,
                    latest_attempt: None,
                },
                now,
            ),
        ];

        classifications.sort_by_key(|classification| classification.sort_key());

        assert_eq!(classifications[0].item().asset_id, "SOL");
        assert_eq!(
            classifications[0].priority(),
            FreshnessPriority::NoRecentAttempt
        );
        assert_eq!(classifications[0].missing_span, Some(missing_span));
        assert_eq!(classifications[1].item().asset_id, "BBB");
        assert_eq!(
            classifications[1].priority(),
            FreshnessPriority::SuccessEmptyCooldown
        );
        assert_eq!(classifications[1].missing_span, None);
    }

    #[test]
    fn freshness_success_with_prices_keeps_missing_span_eligible() {
        let missing_span = DateSpan {
            start: d("2026-07-05"),
            end: d("2026-07-07"),
        };

        let classification = classify_freshness_candidate(
            FreshnessAssetCandidate {
                item: asset_work("BTC", d("2026-01-01")),
                missing_span,
                latest_attempt: Some(PriceHistoryAttemptEvidence {
                    status: crate::db::HistoricalPriceAttemptStatus::SuccessWithPrices,
                    attempted_at: dt("2026-07-10T11:30:00Z"),
                }),
            },
            dt("2026-07-10T12:00:00Z"),
        );

        assert_eq!(
            classification.priority(),
            FreshnessPriority::NoRecentAttempt
        );
        assert_eq!(classification.missing_span, Some(missing_span));
    }

    #[test]
    fn freshness_transient_failure_is_eligible() {
        let missing_span = DateSpan {
            start: d("2026-07-05"),
            end: d("2026-07-07"),
        };

        let classification = classify_freshness_candidate(
            FreshnessAssetCandidate {
                item: asset_work("BTC", d("2026-01-01")),
                missing_span,
                latest_attempt: Some(PriceHistoryAttemptEvidence {
                    status: crate::db::HistoricalPriceAttemptStatus::TransientFailure,
                    attempted_at: dt("2026-07-10T11:30:00Z"),
                }),
            },
            dt("2026-07-10T12:00:00Z"),
        );

        assert_eq!(
            classification.priority(),
            FreshnessPriority::NoRecentAttempt
        );
        assert_eq!(classification.missing_span, Some(missing_span));
    }

    #[test]
    fn freshness_permanent_failure_stops_asset() {
        let missing_span = DateSpan {
            start: d("2026-07-05"),
            end: d("2026-07-07"),
        };

        let classification = classify_freshness_candidate(
            FreshnessAssetCandidate {
                item: asset_work("BTC", d("2026-01-01")),
                missing_span,
                latest_attempt: Some(PriceHistoryAttemptEvidence {
                    status: crate::db::HistoricalPriceAttemptStatus::PermanentFailure,
                    attempted_at: dt("2026-07-10T11:30:00Z"),
                }),
            },
            dt("2026-07-10T12:00:00Z"),
        );

        assert_eq!(classification.priority(), FreshnessPriority::StoppedAsset);
        assert_eq!(classification.missing_span, None);
    }

    #[test]
    fn freshness_recent_rate_limit_blocks_immediate_retry() {
        let missing_span = DateSpan {
            start: d("2026-07-05"),
            end: d("2026-07-07"),
        };

        let classification = classify_freshness_candidate(
            FreshnessAssetCandidate {
                item: asset_work("BTC", d("2026-01-01")),
                missing_span,
                latest_attempt: Some(PriceHistoryAttemptEvidence {
                    status: crate::db::HistoricalPriceAttemptStatus::RateLimited,
                    attempted_at: dt("2026-07-10T11:46:00Z"),
                }),
            },
            dt("2026-07-10T12:00:00Z"),
        );

        assert_eq!(
            classification.priority(),
            FreshnessPriority::RateLimitedRecentAttempt
        );
        assert_eq!(classification.missing_span, None);
    }

    #[test]
    fn freshness_rate_limit_at_cooldown_boundary_is_eligible() {
        let missing_span = DateSpan {
            start: d("2026-07-05"),
            end: d("2026-07-07"),
        };

        let classification = classify_freshness_candidate(
            FreshnessAssetCandidate {
                item: asset_work("BTC", d("2026-01-01")),
                missing_span,
                latest_attempt: Some(PriceHistoryAttemptEvidence {
                    status: crate::db::HistoricalPriceAttemptStatus::RateLimited,
                    attempted_at: dt("2026-07-10T11:45:00Z"),
                }),
            },
            dt("2026-07-10T12:00:00Z"),
        );

        assert_eq!(
            classification.priority(),
            FreshnessPriority::NoRecentAttempt
        );
        assert_eq!(classification.missing_span, Some(missing_span));
    }

    #[test]
    fn freshness_success_empty_exact_cooldown_is_eligible() {
        let missing_span = DateSpan {
            start: d("2026-07-05"),
            end: d("2026-07-07"),
        };

        let classification = classify_freshness_candidate(
            FreshnessAssetCandidate {
                item: asset_work("BBB", d("2026-01-01")),
                missing_span,
                latest_attempt: Some(PriceHistoryAttemptEvidence {
                    status: crate::db::HistoricalPriceAttemptStatus::SuccessEmpty,
                    attempted_at: dt("2026-07-09T12:00:00Z"),
                }),
            },
            dt("2026-07-10T12:00:00Z"),
        );

        assert_eq!(
            classification.priority(),
            FreshnessPriority::NoRecentAttempt
        );
        assert_eq!(classification.missing_span, Some(missing_span));
    }

    #[test]
    fn freshness_sort_key_is_deterministic_for_equal_priority_assets() {
        let missing_span = DateSpan {
            start: d("2026-07-05"),
            end: d("2026-07-07"),
        };
        let mut classifications = [
            classify_freshness_candidate(
                FreshnessAssetCandidate {
                    item: asset_work("SOL", d("2026-01-01")),
                    missing_span,
                    latest_attempt: None,
                },
                dt("2026-07-10T12:00:00Z"),
            ),
            classify_freshness_candidate(
                FreshnessAssetCandidate {
                    item: asset_work("BTC", d("2026-01-01")),
                    missing_span,
                    latest_attempt: None,
                },
                dt("2026-07-10T12:00:00Z"),
            ),
            classify_freshness_candidate(
                FreshnessAssetCandidate {
                    item: asset_work("ETH", d("2026-01-01")),
                    missing_span,
                    latest_attempt: None,
                },
                dt("2026-07-10T12:00:00Z"),
            ),
        ];

        classifications.sort_by_key(|classification| classification.sort_key());

        assert_eq!(
            classifications
                .iter()
                .map(|classification| classification.item().asset_id.as_str())
                .collect::<Vec<_>>(),
            vec!["BTC", "ETH", "SOL"]
        );
    }

    #[test]
    fn success_empty_becomes_eligible_after_cooldown() {
        let missing_span = DateSpan {
            start: d("2026-07-05"),
            end: d("2026-07-07"),
        };

        let classification = classify_freshness_candidate(
            FreshnessAssetCandidate {
                item: asset_work("BBB", d("2026-01-01")),
                missing_span,
                latest_attempt: Some(PriceHistoryAttemptEvidence {
                    status: crate::db::HistoricalPriceAttemptStatus::SuccessEmpty,
                    attempted_at: dt("2026-07-09T12:00:00Z"),
                }),
            },
            dt("2026-07-10T12:00:00Z"),
        );

        assert_eq!(
            classification.priority(),
            FreshnessPriority::NoRecentAttempt
        );
        assert_eq!(classification.item().asset_id, "BBB");
        assert_eq!(classification.missing_span, Some(missing_span));
    }

    #[test]
    fn freshness_lane_reaches_sol_before_retrying_empty_bbb_or_deep_history() {
        let mut deps = FakeBackfillDeps::default();
        let covered_recent = (28..=30)
            .map(|day| d(&format!("2026-06-{day:02}")))
            .chain((1..=2).map(|day| d(&format!("2026-07-{day:02}"))))
            .collect::<Vec<_>>();
        deps.load_results.push_back(covered_recent.clone());
        deps.load_results.push_back(covered_recent.clone());
        deps.load_results.push_back(covered_recent.clone());
        deps.load_results.push_back(covered_recent.clone());
        deps.load_results.push_back(
            (28..=30)
                .map(|day| d(&format!("2026-06-{day:02}")))
                .chain((1..=4).map(|day| d(&format!("2026-07-{day:02}"))))
                .collect(),
        );
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results.push_back(Ok(Vec::new()));
        deps.fetch_results.push_back(Ok(Vec::new()));
        deps.seed_attempt(attempt_record(
            "BBB",
            d("2026-07-03"),
            d("2026-07-04"),
            crate::db::HistoricalPriceAttemptStatus::SuccessEmpty,
            dt("2026-07-04T11:30:00Z"),
        ));
        deps.seed_attempt(attempt_record(
            "GGG",
            d("2026-07-03"),
            d("2026-07-04"),
            crate::db::HistoricalPriceAttemptStatus::RateLimited,
            dt("2026-07-04T11:50:00Z"),
        ));

        let work_items = vec![
            asset_work_with_provider("BBB", "based-baby", d("2026-01-01")),
            asset_work_with_provider("DDD", "dot-dot-finance", d("2026-01-01")),
            asset_work_with_provider("GGG", "good-games-guild", d("2026-01-01")),
            asset_work_with_provider("SOL", "solana", d("2026-01-01")),
            asset_work_with_provider("BTC", "bitcoin", d("2026-01-01")),
        ];

        run_backfill_for_work_with_deps(
            &mut deps,
            usd(),
            &work_items,
            d("2026-07-04"),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::Pro,
            dt("2026-07-04T12:00:00Z"),
        )
        .expect("backfill should succeed");

        assert_eq!(
            deps.fetches
                .iter()
                .map(|(provider_asset_id, _, _, _)| provider_asset_id.as_str())
                .collect::<Vec<_>>(),
            vec!["dot-dot-finance", "solana"]
        );
        assert_eq!(
            deps.loads,
            vec![
                (d("2026-06-28"), d("2026-07-04")),
                (d("2026-06-28"), d("2026-07-04")),
                (d("2026-06-28"), d("2026-07-04")),
                (d("2026-06-28"), d("2026-07-04")),
                (d("2026-06-28"), d("2026-07-04")),
                (d("2026-07-03"), d("2026-07-04")),
                (d("2026-07-03"), d("2026-07-04")),
            ]
        );
        assert_eq!(
            deps.attempt_lookups,
            vec![
                attempt_lookup_key("BBB", "coingecko", d("2026-07-03"), d("2026-07-04")),
                attempt_lookup_key("DDD", "coingecko", d("2026-07-03"), d("2026-07-04")),
                attempt_lookup_key("GGG", "coingecko", d("2026-07-03"), d("2026-07-04")),
                attempt_lookup_key("SOL", "coingecko", d("2026-07-03"), d("2026-07-04")),
            ]
        );
    }

    #[test]
    fn freshness_work_clamps_lookup_and_missing_span_to_horizon_lower_bound() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(vec![d("2026-07-04")]);
        let work_items = vec![asset_work("bitcoin", d("2026-07-02"))];

        let freshness_work = load_freshness_span_work_with_deps(
            &mut deps,
            usd(),
            &work_items,
            d("2026-07-04"),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::Pro,
            dt("2026-07-04T12:00:00Z"),
        )
        .expect("freshness work should load");

        assert_eq!(deps.loads, vec![(d("2026-07-02"), d("2026-07-04"))]);
        assert_eq!(
            freshness_work,
            vec![PriceHistoryAssetSpanWork {
                item: asset_work("bitcoin", d("2026-07-02")),
                span: DateSpan {
                    start: d("2026-07-02"),
                    end: d("2026-07-03"),
                },
            }]
        );
        assert_eq!(
            deps.attempt_lookups,
            vec![attempt_lookup_key(
                "bitcoin",
                "coingecko",
                d("2026-07-02"),
                d("2026-07-03")
            )]
        );
    }

    #[test]
    fn freshness_work_retries_success_with_prices_when_db_still_has_gap() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(vec![d("2026-07-08")]);
        deps.seed_attempt(attempt_record(
            "bitcoin",
            d("2026-07-09"),
            d("2026-07-10"),
            crate::db::HistoricalPriceAttemptStatus::SuccessWithPrices,
            dt("2026-07-10T11:30:00Z"),
        ));
        let work_items = vec![asset_work("bitcoin", d("2026-01-01"))];

        let freshness_work = load_freshness_span_work_with_deps(
            &mut deps,
            usd(),
            &work_items,
            d("2026-07-10"),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::Pro,
            dt("2026-07-10T12:00:00Z"),
        )
        .expect("freshness work should load");

        assert_eq!(
            freshness_work,
            vec![PriceHistoryAssetSpanWork {
                item: asset_work("bitcoin", d("2026-01-01")),
                span: DateSpan {
                    start: d("2026-07-09"),
                    end: d("2026-07-10"),
                },
            }]
        );
    }

    #[test]
    fn freshness_covering_success_with_prices_overrides_older_cooldown_evidence() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(vec![d("2026-07-08")]);
        deps.seed_attempt(attempt_record(
            "bitcoin",
            d("2026-07-09"),
            d("2026-07-10"),
            crate::db::HistoricalPriceAttemptStatus::SuccessWithPrices,
            dt("2026-07-10T11:55:00Z"),
        ));
        deps.seed_attempt(attempt_record(
            "bitcoin",
            d("2026-07-04"),
            d("2026-07-08"),
            crate::db::HistoricalPriceAttemptStatus::RateLimited,
            dt("2026-07-10T11:50:00Z"),
        ));
        let work_items = vec![asset_work("bitcoin", d("2026-01-01"))];

        let freshness_work = load_freshness_span_work_with_deps(
            &mut deps,
            usd(),
            &work_items,
            d("2026-07-10"),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::Pro,
            dt("2026-07-10T12:00:00Z"),
        )
        .expect("freshness work should load");

        assert_eq!(
            freshness_work,
            vec![PriceHistoryAssetSpanWork {
                item: asset_work("bitcoin", d("2026-01-01")),
                span: DateSpan {
                    start: d("2026-07-09"),
                    end: d("2026-07-10"),
                },
            }]
        );
    }

    #[test]
    fn run_backfill_does_not_deep_fill_when_freshness_is_rate_limited() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(vec![d("2026-07-08")]);
        deps.seed_attempt(attempt_record(
            "bitcoin",
            d("2026-07-09"),
            d("2026-07-10"),
            crate::db::HistoricalPriceAttemptStatus::RateLimited,
            dt("2026-07-10T11:50:00Z"),
        ));
        let work_items = vec![asset_work("bitcoin", d("2026-01-01"))];

        run_backfill_for_work_with_deps(
            &mut deps,
            usd(),
            &work_items,
            d("2026-07-10"),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::Pro,
            dt("2026-07-10T12:00:00Z"),
        )
        .expect("backfill should succeed");

        assert_eq!(deps.loads, vec![(d("2026-07-04"), d("2026-07-10"))]);
        assert_eq!(
            deps.attempt_lookups,
            vec![attempt_lookup_key(
                "bitcoin",
                "coingecko",
                d("2026-07-09"),
                d("2026-07-10")
            )]
        );
        assert!(deps.fetches.is_empty());
        assert!(deps.upserts.is_empty());
    }

    #[test]
    fn freshness_success_empty_cooldown_survives_date_rollover() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(
            (5..=10)
                .map(|day| d(&format!("2026-07-{day:02}")))
                .collect(),
        );
        deps.seed_attempt(attempt_record(
            "BBB",
            d("2026-07-04"),
            d("2026-07-10"),
            crate::db::HistoricalPriceAttemptStatus::SuccessEmpty,
            dt("2026-07-11T00:00:00Z"),
        ));
        let work_items = vec![asset_work("BBB", d("2026-01-01"))];

        let freshness_work = load_freshness_span_work_with_deps(
            &mut deps,
            usd(),
            &work_items,
            d("2026-07-11"),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::Pro,
            dt("2026-07-11T12:00:00Z"),
        )
        .expect("freshness work should load");

        assert!(freshness_work.is_empty());
    }

    #[test]
    fn freshness_rate_limit_cooldown_survives_date_rollover_and_blocks_deep_fill() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(
            (5..=10)
                .map(|day| d(&format!("2026-07-{day:02}")))
                .collect(),
        );
        deps.seed_attempt(attempt_record(
            "GGG",
            d("2026-07-04"),
            d("2026-07-10"),
            crate::db::HistoricalPriceAttemptStatus::RateLimited,
            dt("2026-07-11T11:50:00Z"),
        ));
        let work_items = vec![asset_work("GGG", d("2026-01-01"))];

        run_backfill_for_work_with_deps(
            &mut deps,
            usd(),
            &work_items,
            d("2026-07-11"),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::Pro,
            dt("2026-07-11T12:00:00Z"),
        )
        .expect("backfill should succeed");

        assert_eq!(deps.loads, vec![(d("2026-07-05"), d("2026-07-11"))]);
        assert!(deps.fetches.is_empty());
        assert!(deps.upserts.is_empty());
    }

    #[test]
    fn run_backfill_executes_deep_history_when_freshness_has_no_immediate_work() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results
            .push_back((2..=8).map(|day| d(&format!("2026-07-{day:02}"))).collect());
        deps.load_results.push_back(vec![d("2026-07-01")]);
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-07-07", "7")]));
        let work_items = vec![asset_work("bitcoin", d("2026-07-01"))];

        run_backfill_for_work_with_deps(
            &mut deps,
            usd(),
            &work_items,
            d("2026-07-08"),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::Pro,
            dt("2026-07-08T12:00:00Z"),
        )
        .expect("backfill should succeed");

        assert_eq!(
            deps.loads,
            vec![
                (d("2026-07-02"), d("2026-07-08")),
                (d("2026-07-01"), d("2026-07-08")),
                (d("2026-07-02"), d("2026-07-08")),
            ]
        );
        assert!(deps.attempt_lookups.is_empty());
        assert_eq!(deps.fetches.len(), 1);
    }

    #[test]
    fn existing_span_suppresses_provider_call() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results
            .push_back(vec![d("2026-01-01"), d("2026-01-02"), d("2026-01-03")]);

        run_fair_test_backfill(&mut deps, &[work(d("2026-01-01"))], d("2026-01-03"))
            .expect("backfill should succeed");

        assert_eq!(deps.loads, vec![(d("2026-01-01"), d("2026-01-03"))]);
        assert!(deps.fetches.is_empty());
        assert!(deps.upserts.is_empty());
    }

    #[test]
    fn fair_span_work_interleaves_assets_before_deeper_history() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(vec![
            d("2026-01-04"),
            d("2026-01-05"),
            d("2026-01-06"),
            d("2026-01-07"),
        ]);
        deps.load_results.push_back(vec![
            d("2026-01-01"),
            d("2026-01-02"),
            d("2026-01-03"),
            d("2026-01-04"),
            d("2026-01-05"),
            d("2026-01-06"),
            d("2026-01-07"),
        ]);

        let work_items = vec![
            asset_work("bitcoin", d("2026-01-01")),
            asset_work("ethereum", d("2026-01-01")),
        ];

        let fair_work = load_fair_span_work_with_deps(
            &mut deps,
            usd(),
            &work_items,
            d("2026-01-10"),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::Pro,
        )
        .expect("fair work should load");

        assert_eq!(
            fair_work,
            vec![
                PriceHistoryAssetSpanWork {
                    item: asset_work("bitcoin", d("2026-01-01")),
                    span: DateSpan {
                        start: d("2026-01-08"),
                        end: d("2026-01-10"),
                    },
                },
                PriceHistoryAssetSpanWork {
                    item: asset_work("ethereum", d("2026-01-01")),
                    span: DateSpan {
                        start: d("2026-01-08"),
                        end: d("2026-01-10"),
                    },
                },
                PriceHistoryAssetSpanWork {
                    item: asset_work("bitcoin", d("2026-01-01")),
                    span: DateSpan {
                        start: d("2026-01-01"),
                        end: d("2026-01-03"),
                    },
                },
            ]
        );
        assert_eq!(
            deps.loads,
            vec![
                (d("2026-01-01"), d("2026-01-10")),
                (d("2026-01-01"), d("2026-01-10")),
            ]
        );
    }

    #[test]
    fn public_keyless_fair_work_never_loads_older_than_365_days() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(Vec::new());
        let work_items = vec![asset_work("bitcoin", d("2010-01-01"))];

        let fair_work = load_fair_span_work_with_deps(
            &mut deps,
            usd(),
            &work_items,
            d("2026-06-12"),
            crate::tasks::jobs::price_history::planner::PriceHistoryHorizon::PublicKeyless,
        )
        .expect("fair work should load");

        assert_eq!(deps.loads, vec![(d("2025-06-13"), d("2026-06-12"))]);
        assert_eq!(
            fair_work,
            vec![PriceHistoryAssetSpanWork {
                item: asset_work("bitcoin", d("2010-01-01")),
                span: DateSpan {
                    start: d("2025-06-13"),
                    end: d("2026-06-12"),
                },
            }]
        );
    }

    #[test]
    fn fair_queue_stop_asset_skips_later_spans_for_same_asset_only() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::HistoryLimit));
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-09", "9")]));
        let fair_work = vec![
            PriceHistoryAssetSpanWork {
                item: asset_work("bitcoin", d("2026-01-01")),
                span: DateSpan {
                    start: d("2026-01-09"),
                    end: d("2026-01-10"),
                },
            },
            PriceHistoryAssetSpanWork {
                item: asset_work("ethereum", d("2026-01-01")),
                span: DateSpan {
                    start: d("2026-01-09"),
                    end: d("2026-01-10"),
                },
            },
            PriceHistoryAssetSpanWork {
                item: asset_work("bitcoin", d("2026-01-01")),
                span: DateSpan {
                    start: d("2026-01-01"),
                    end: d("2026-01-03"),
                },
            },
        ];

        execute_fair_span_work_with_deps(&mut deps, usd(), fair_work)
            .expect("fair queue should succeed");

        assert_eq!(
            deps.loads,
            vec![
                (d("2026-01-09"), d("2026-01-10")),
                (d("2026-01-09"), d("2026-01-10")),
            ]
        );
        assert_eq!(
            deps.fetches
                .iter()
                .map(|(provider_asset_id, _, _, _)| provider_asset_id.as_str())
                .collect::<Vec<_>>(),
            vec!["bitcoin", "ethereum"]
        );
    }

    #[test]
    fn exact_recheck_split_defers_older_subspan_behind_existing_fair_work() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(vec![
            d("2026-01-04"),
            d("2026-01-05"),
            d("2026-01-06"),
            d("2026-01-07"),
        ]);
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-10", "10")]));
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-10", "20")]));
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-03", "3")]));
        let fair_work = vec![
            PriceHistoryAssetSpanWork {
                item: asset_work("bitcoin", d("2026-01-01")),
                span: DateSpan {
                    start: d("2026-01-01"),
                    end: d("2026-01-10"),
                },
            },
            PriceHistoryAssetSpanWork {
                item: asset_work("ethereum", d("2026-01-08")),
                span: DateSpan {
                    start: d("2026-01-08"),
                    end: d("2026-01-10"),
                },
            },
        ];

        execute_fair_span_work_with_deps(&mut deps, usd(), fair_work)
            .expect("fair queue should succeed");

        assert_eq!(
            deps.fetches
                .iter()
                .map(|(provider_asset_id, _, _, _)| provider_asset_id.as_str())
                .collect::<Vec<_>>(),
            vec!["bitcoin", "ethereum", "bitcoin"]
        );
        assert_eq!(
            deps.loads,
            vec![
                (d("2026-01-01"), d("2026-01-10")),
                (d("2026-01-08"), d("2026-01-10")),
                (d("2026-01-08"), d("2026-01-10")),
                (d("2026-01-01"), d("2026-01-03")),
            ]
        );
        assert_eq!(deps.upserts[0][0].asset_id, "bitcoin");
        assert_eq!(deps.upserts[1][0].asset_id, "ethereum");
        assert_eq!(deps.upserts[2][0].asset_id, "bitcoin");
        assert_eq!(deps.upserts[2][0].date_utc, d("2026-01-03"));
    }

    #[test]
    fn exact_recheck_split_defers_before_later_same_asset_planned_work() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(vec![
            d("2026-01-04"),
            d("2026-01-05"),
            d("2026-01-06"),
            d("2026-01-07"),
        ]);
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-10", "10")]));
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-10", "20")]));
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-03", "3")]));
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2025-12-31", "31")]));
        let fair_work = vec![
            PriceHistoryAssetSpanWork {
                item: asset_work("bitcoin", d("2026-01-01")),
                span: DateSpan {
                    start: d("2026-01-01"),
                    end: d("2026-01-10"),
                },
            },
            PriceHistoryAssetSpanWork {
                item: asset_work("ethereum", d("2026-01-08")),
                span: DateSpan {
                    start: d("2026-01-08"),
                    end: d("2026-01-10"),
                },
            },
            PriceHistoryAssetSpanWork {
                item: asset_work("bitcoin", d("2025-12-01")),
                span: DateSpan {
                    start: d("2025-12-01"),
                    end: d("2025-12-31"),
                },
            },
        ];

        execute_fair_span_work_with_deps(&mut deps, usd(), fair_work)
            .expect("fair queue should succeed");

        assert_eq!(
            deps.fetches
                .iter()
                .map(|(provider_asset_id, _quote, from, to)| {
                    (provider_asset_id.as_str(), *from, *to)
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    "bitcoin",
                    unix_day_start(d("2026-01-08")).expect("from timestamp"),
                    unix_day_start(d("2026-01-11")).expect("to timestamp"),
                ),
                (
                    "ethereum",
                    unix_day_start(d("2026-01-08")).expect("from timestamp"),
                    unix_day_start(d("2026-01-11")).expect("to timestamp"),
                ),
                (
                    "bitcoin",
                    unix_day_start(d("2026-01-01")).expect("from timestamp"),
                    unix_day_start(d("2026-01-04")).expect("to timestamp"),
                ),
                (
                    "bitcoin",
                    unix_day_start(d("2025-12-01")).expect("from timestamp"),
                    unix_day_start(d("2026-01-01")).expect("to timestamp"),
                ),
            ]
        );
        assert_eq!(deps.upserts[0][0].date_utc, d("2026-01-10"));
        assert_eq!(deps.upserts[1][0].asset_id, "ethereum");
        assert_eq!(deps.upserts[2][0].date_utc, d("2026-01-03"));
        assert_eq!(deps.upserts[3][0].date_utc, d("2025-12-31"));
    }

    #[test]
    fn exact_span_recheck_suppresses_provider_call_when_gap_was_filled() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(Vec::new());
        deps.load_results
            .push_back(vec![d("2026-01-01"), d("2026-01-02"), d("2026-01-03")]);

        run_fair_test_backfill(&mut deps, &[work(d("2026-01-01"))], d("2026-01-03"))
            .expect("backfill should succeed");

        assert_eq!(
            deps.loads,
            vec![
                (d("2026-01-01"), d("2026-01-03")),
                (d("2026-01-01"), d("2026-01-03")),
            ]
        );
        assert!(deps.fetches.is_empty());
        assert!(deps.upserts.is_empty());
    }

    #[test]
    fn fetched_rows_include_daily_price_metadata() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-02", "42000.50")]));
        let item = work(d("2026-01-02"));

        run_fair_test_backfill(&mut deps, &[item], d("2026-01-02"))
            .expect("backfill should succeed");

        assert_eq!(
            deps.fetches,
            vec![(
                "bitcoin".to_string(),
                "usd".to_string(),
                unix_day_start(d("2026-01-02")).expect("from timestamp"),
                unix_day_start(d("2026-01-03")).expect("to timestamp"),
            )]
        );
        assert_eq!(deps.upserts.len(), 1);
        assert_eq!(
            deps.upserts[0],
            vec![DailyPricePointUpsert {
                asset_id: "bitcoin".to_string(),
                quote_currency: usd(),
                price_time_utc: dt("2026-01-02T12:00:00Z"),
                date_utc: d("2026-01-02"),
                price: Decimal::from_str_exact("42000.50").expect("test price"),
                provider_asset_id: "bitcoin".to_string(),
                provider_quote_id: Some("usd".to_string()),
                license_scope: "test_license".to_string(),
                retrieved_at: dt("2026-01-10T00:00:00Z"),
            }]
        );
    }

    #[test]
    fn empty_success_records_success_empty_attempt() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results.push_back(Ok(Vec::new()));

        run_fair_test_backfill(&mut deps, &[work(d("2026-01-02"))], d("2026-01-02"))
            .expect("backfill should succeed");

        assert_eq!(deps.upserts, vec![Vec::new()]);
        assert_eq!(
            deps.attempt_upserts,
            vec![expected_attempt_upsert(
                "bitcoin",
                d("2026-01-02"),
                d("2026-01-02"),
                crate::db::HistoricalPriceAttemptStatus::SuccessEmpty,
                0,
                None,
            )]
        );
    }

    #[test]
    fn successful_rows_record_success_with_prices_attempt() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results.push_back(Ok(vec![
            daily_price("2026-01-02", "42000.50"),
            daily_price("2026-01-03", "43000.25"),
        ]));

        run_fair_test_backfill(&mut deps, &[work(d("2026-01-02"))], d("2026-01-03"))
            .expect("backfill should succeed");

        assert_eq!(deps.upserts.len(), 1);
        assert_eq!(deps.upserts[0].len(), 2);
        assert_eq!(
            deps.attempt_upserts,
            vec![expected_attempt_upsert(
                "bitcoin",
                d("2026-01-02"),
                d("2026-01-03"),
                crate::db::HistoricalPriceAttemptStatus::SuccessWithPrices,
                2,
                None,
            )]
        );
    }

    #[test]
    fn history_limit_stops_before_older_missing_spans_only() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results
            .push_back(vec![d("2026-01-03"), d("2026-01-04"), d("2026-01-08")]);
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-09", "1")]));
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::HistoryLimit));

        run_fair_test_backfill(&mut deps, &[work(d("2026-01-01"))], d("2026-01-10"))
            .expect("backfill should succeed");

        assert_eq!(deps.fetches.len(), 2);
        assert_eq!(deps.loads[1], (d("2026-01-09"), d("2026-01-10")));
        assert_eq!(deps.loads[2], (d("2026-01-05"), d("2026-01-07")));
        assert_eq!(
            deps.attempt_upserts,
            vec![
                expected_attempt_upsert(
                    "bitcoin",
                    d("2026-01-09"),
                    d("2026-01-10"),
                    crate::db::HistoricalPriceAttemptStatus::SuccessWithPrices,
                    1,
                    None,
                ),
                expected_attempt_upsert(
                    "bitcoin",
                    d("2026-01-05"),
                    d("2026-01-07"),
                    crate::db::HistoricalPriceAttemptStatus::PermanentFailure,
                    0,
                    Some("history_limit"),
                ),
            ]
        );
    }

    #[test]
    fn persistent_rate_limit_records_rate_limited_attempt() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::RateLimited(
                "rate limited 1".to_string(),
            )));
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::RateLimited(
                "rate limited 2".to_string(),
            )));
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::RateLimited(
                "rate limited 3".to_string(),
            )));

        let work_items = vec![
            asset_work("bitcoin", d("2026-01-01")),
            asset_work("ethereum", d("2026-01-01")),
        ];

        run_fair_test_backfill(&mut deps, &work_items, d("2026-01-01"))
            .expect("backfill should stop cleanly");

        let fetched_provider_ids = deps
            .fetches
            .iter()
            .map(|(provider_asset_id, _quote, _from, _to)| provider_asset_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(fetched_provider_ids, vec!["bitcoin", "bitcoin", "bitcoin"]);
        assert_eq!(
            deps.loads,
            vec![
                (d("2026-01-01"), d("2026-01-01")),
                (d("2026-01-01"), d("2026-01-01")),
                (d("2026-01-01"), d("2026-01-01")),
                (d("2026-01-01"), d("2026-01-01")),
                (d("2026-01-01"), d("2026-01-01")),
            ]
        );
        assert_eq!(
            deps.sleeps,
            vec![Duration::from_secs(1), Duration::from_secs(5)]
        );
        assert!(deps.upserts.is_empty());
        assert_eq!(
            deps.attempt_upserts,
            vec![expected_attempt_upsert(
                "bitcoin",
                d("2026-01-01"),
                d("2026-01-01"),
                crate::db::HistoricalPriceAttemptStatus::RateLimited,
                0,
                Some("rate_limited"),
            )]
        );
        assert_eq!(
            deps.attempt_upserts[0].error_code.as_deref(),
            Some("rate_limited")
        );
    }

    #[test]
    fn persistent_rate_limit_on_second_btc_span_happens_after_eth_newest_span() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(vec![
            d("2026-01-04"),
            d("2026-01-05"),
            d("2026-01-06"),
            d("2026-01-07"),
        ]);
        deps.load_results.push_back(vec![
            d("2026-01-01"),
            d("2026-01-02"),
            d("2026-01-03"),
            d("2026-01-04"),
            d("2026-01-05"),
            d("2026-01-06"),
            d("2026-01-07"),
        ]);
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-10", "10")]));
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-10", "20")]));
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::RateLimited(
                "rate limited 1".to_string(),
            )));
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::RateLimited(
                "rate limited 2".to_string(),
            )));
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::RateLimited(
                "rate limited 3".to_string(),
            )));

        let work_items = vec![
            asset_work("bitcoin", d("2026-01-01")),
            asset_work("ethereum", d("2026-01-01")),
        ];

        run_fair_test_backfill(&mut deps, &work_items, d("2026-01-10"))
            .expect("fair backfill should stop cleanly");

        let fetched_provider_ids = deps
            .fetches
            .iter()
            .map(|(provider_asset_id, _quote, _from, _to)| provider_asset_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            fetched_provider_ids,
            vec!["bitcoin", "ethereum", "bitcoin", "bitcoin", "bitcoin"]
        );
        assert_eq!(deps.upserts.len(), 2);
        assert_eq!(deps.upserts[0][0].asset_id, "bitcoin");
        assert_eq!(deps.upserts[1][0].asset_id, "ethereum");
        assert_eq!(
            deps.sleeps,
            vec![Duration::from_secs(1), Duration::from_secs(5)]
        );
    }

    #[test]
    fn retry_recheck_processes_all_split_missing_subspans() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.load_results
            .push_back(vec![d("2026-01-04"), d("2026-01-05"), d("2026-01-06")]);
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::RateLimited(
                "rate limited".to_string(),
            )));
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-10", "10")]));
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-03", "3")]));

        run_fair_test_backfill(&mut deps, &[work(d("2026-01-01"))], d("2026-01-10"))
            .expect("backfill should succeed");

        assert_eq!(deps.fetches.len(), 3);
        assert_eq!(
            deps.loads,
            vec![
                (d("2026-01-01"), d("2026-01-10")),
                (d("2026-01-01"), d("2026-01-10")),
                (d("2026-01-01"), d("2026-01-10")),
                (d("2026-01-07"), d("2026-01-10")),
                (d("2026-01-01"), d("2026-01-03")),
            ]
        );
        assert_eq!(deps.sleeps, vec![Duration::from_secs(1)]);
        assert_eq!(deps.upserts.len(), 2);
        assert_eq!(deps.upserts[0][0].date_utc, d("2026-01-10"));
        assert_eq!(deps.upserts[1][0].date_utc, d("2026-01-03"));
    }

    #[test]
    fn retry_budget_resets_when_exact_recheck_selects_new_subspan() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(Vec::new());
        deps.load_results
            .push_back(vec![d("2026-01-04"), d("2026-01-05"), d("2026-01-06")]);
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::RateLimited(
                "parent rate limited".to_string(),
            )));
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::RateLimited(
                "selected subspan rate limited 1".to_string(),
            )));
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::RateLimited(
                "selected subspan rate limited 2".to_string(),
            )));
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-10", "10")]));
        deps.fetch_results
            .push_back(Ok(vec![daily_price("2026-01-03", "3")]));
        let fair_work = vec![PriceHistoryAssetSpanWork {
            item: work(d("2026-01-01")),
            span: DateSpan {
                start: d("2026-01-01"),
                end: d("2026-01-10"),
            },
        }];

        execute_fair_span_work_with_deps(&mut deps, usd(), fair_work)
            .expect("backfill should succeed");

        assert_eq!(
            deps.sleeps,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(5),
            ]
        );
        assert_eq!(
            deps.fetches
                .iter()
                .map(|(_provider_asset_id, _quote, from, to)| (*from, *to))
                .collect::<Vec<_>>(),
            vec![
                (
                    unix_day_start(d("2026-01-01")).expect("from timestamp"),
                    unix_day_start(d("2026-01-11")).expect("to timestamp"),
                ),
                (
                    unix_day_start(d("2026-01-07")).expect("from timestamp"),
                    unix_day_start(d("2026-01-11")).expect("to timestamp"),
                ),
                (
                    unix_day_start(d("2026-01-07")).expect("from timestamp"),
                    unix_day_start(d("2026-01-11")).expect("to timestamp"),
                ),
                (
                    unix_day_start(d("2026-01-07")).expect("from timestamp"),
                    unix_day_start(d("2026-01-11")).expect("to timestamp"),
                ),
                (
                    unix_day_start(d("2026-01-01")).expect("from timestamp"),
                    unix_day_start(d("2026-01-04")).expect("to timestamp"),
                ),
            ]
        );
        assert_eq!(deps.upserts.len(), 2);
        assert_eq!(deps.upserts[0][0].date_utc, d("2026-01-10"));
        assert_eq!(deps.upserts[1][0].date_utc, d("2026-01-03"));
    }

    #[test]
    fn retry_recheck_full_coverage_skips_second_provider_call() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(
            (1..=10)
                .map(|day| d(&format!("2026-01-{day:02}")))
                .collect(),
        );
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::RateLimited(
                "rate limited".to_string(),
            )));

        run_fair_test_backfill(&mut deps, &[work(d("2026-01-01"))], d("2026-01-10"))
            .expect("backfill should succeed");

        assert_eq!(deps.fetches.len(), 1);
        assert_eq!(
            deps.loads,
            vec![
                (d("2026-01-01"), d("2026-01-10")),
                (d("2026-01-01"), d("2026-01-10")),
                (d("2026-01-01"), d("2026-01-10")),
            ]
        );
        assert_eq!(deps.sleeps, vec![Duration::from_secs(1)]);
        assert!(deps.upserts.is_empty());
    }

    #[test]
    fn not_found_stops_asset_for_run() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results
            .push_back(vec![d("2026-01-04"), d("2026-01-05"), d("2026-01-06")]);
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::NotFound(
                "not found".to_string(),
            )));

        run_fair_test_backfill(&mut deps, &[work(d("2026-01-01"))], d("2026-01-10"))
            .expect("backfill should succeed");

        assert_eq!(deps.fetches.len(), 1);
        assert_eq!(
            deps.loads,
            vec![
                (d("2026-01-01"), d("2026-01-10")),
                (d("2026-01-07"), d("2026-01-10")),
            ]
        );
        assert!(deps.upserts.is_empty());
        assert_eq!(
            deps.attempt_upserts,
            vec![expected_attempt_upsert(
                "bitcoin",
                d("2026-01-07"),
                d("2026-01-10"),
                crate::db::HistoricalPriceAttemptStatus::PermanentFailure,
                0,
                Some("not_found"),
            )]
        );
    }

    #[test]
    fn failed_terminal_path_records_transient_failure_attempt() {
        let mut deps = FakeBackfillDeps::default();
        deps.load_results.push_back(Vec::new());
        deps.load_results.push_back(Vec::new());
        deps.fetch_results
            .push_back(Err(PriceHistoryFetchError::Failed(
                "provider response included raw details".to_string(),
            )));

        run_fair_test_backfill(&mut deps, &[work(d("2026-01-01"))], d("2026-01-10"))
            .expect("backfill should succeed");

        assert!(deps.upserts.is_empty());
        assert_eq!(
            deps.attempt_upserts,
            vec![expected_attempt_upsert(
                "bitcoin",
                d("2026-01-01"),
                d("2026-01-10"),
                crate::db::HistoricalPriceAttemptStatus::TransientFailure,
                0,
                Some("transient_failure"),
            )]
        );
    }

    struct FakeReconciliationRuntime {
        price_fetching_enabled: bool,
        historical_backfill_enabled: bool,
        load_price_fetching_calls: u32,
        load_entitlement_calls: u32,
        load_currency_calls: u32,
        load_work_calls: u32,
        backfill_calls: u32,
    }

    impl Default for FakeReconciliationRuntime {
        fn default() -> Self {
            Self {
                price_fetching_enabled: true,
                historical_backfill_enabled: true,
                load_price_fetching_calls: 0,
                load_entitlement_calls: 0,
                load_currency_calls: 0,
                load_work_calls: 0,
                backfill_calls: 0,
            }
        }
    }

    impl PriceHistoryReconciliationRuntime for FakeReconciliationRuntime {
        fn load_price_fetching_enabled(&mut self, _user_id: UserId) -> Result<bool, String> {
            self.load_price_fetching_calls += 1;
            Ok(self.price_fetching_enabled)
        }

        fn load_historical_backfill_enabled(&mut self, _user_id: UserId) -> Result<bool, String> {
            self.load_entitlement_calls += 1;
            Ok(self.historical_backfill_enabled)
        }

        fn load_currency(&mut self, _user_id: UserId) -> Result<CurrencyCode, String> {
            self.load_currency_calls += 1;
            Ok(usd())
        }

        fn load_work(&mut self, _user_id: UserId) -> Result<Vec<PriceHistoryAssetWork>, String> {
            self.load_work_calls += 1;
            Ok(vec![work(d("2026-01-01"))])
        }

        fn run_backfill_for_work(
            &mut self,
            _user_id: UserId,
            _currency: CurrencyCode,
            _work: &[PriceHistoryAssetWork],
        ) -> Result<(), String> {
            self.backfill_calls += 1;
            Ok(())
        }
    }

    #[test]
    fn reconciliation_gate_skips_before_backfill_setup_when_price_fetching_disabled() {
        let mut runtime = FakeReconciliationRuntime {
            price_fetching_enabled: false,
            ..FakeReconciliationRuntime::default()
        };

        let result = run_price_history_reconciliation_with_runtime(
            UserId::new(),
            PriceHistoryReconciliationParams {
                reason: PriceHistoryReconciliationReason::Login,
            },
            &mut runtime,
        )
        .expect("skip should be clean");

        assert_eq!(result, "skipped: PriceFetchingDisabled");
        assert_eq!(runtime.load_price_fetching_calls, 1);
        assert_eq!(runtime.load_entitlement_calls, 1);
        assert_eq!(runtime.load_currency_calls, 0);
        assert_eq!(runtime.load_work_calls, 0);
        assert_eq!(runtime.backfill_calls, 0);
    }

    #[test]
    fn reconciliation_gate_skips_before_backfill_setup_when_historical_backfill_disabled() {
        let mut runtime = FakeReconciliationRuntime {
            historical_backfill_enabled: false,
            ..FakeReconciliationRuntime::default()
        };

        let result = run_price_history_reconciliation_with_runtime(
            UserId::new(),
            PriceHistoryReconciliationParams {
                reason: PriceHistoryReconciliationReason::PriceFetchingEnabled,
            },
            &mut runtime,
        )
        .expect("skip should be clean");

        assert_eq!(result, "skipped: HistoricalBackfillDisabled");
        assert_eq!(runtime.load_price_fetching_calls, 1);
        assert_eq!(runtime.load_entitlement_calls, 1);
        assert_eq!(runtime.load_currency_calls, 0);
        assert_eq!(runtime.load_work_calls, 0);
        assert_eq!(runtime.backfill_calls, 0);
    }
}
