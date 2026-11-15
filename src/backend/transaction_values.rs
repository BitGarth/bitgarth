//! Fiat values for an account's transactions, priced the same way as the
//! reports: the user's own price saved earlier that local day wins, then the
//! CoinGecko daily price for that local date (only while CoinGecko prices are on).

use std::collections::HashMap;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use rust_decimal::{Decimal, RoundingStrategy};

use crate::amounts::{UnsignedAmount, format_unsigned_amount};
use crate::db::{self, AccountTransactionFlowRow, DbError, PriceOverrideRecord};
use crate::models::{CurrencyCode, UserId, UserTimezone};
use crate::report_dates::{DateBoundaryKind, local_report_date_to_utc_boundary};
use crate::services::price_overrides::PriceSubject;
use crate::transactions::{AccountTransactionDirection, ChainTransactionStatus};
use crate::wallets::{
    AccountFlowFiatView, AccountFlowSummaryView, Network, SyncedAssetId, TransactionFiatView,
};

/// Prices keyed by the exact timestamp they were resolved for.
pub(crate) struct TransactionPrices {
    pub(crate) currency: CurrencyCode,
    prices: HashMap<DateTime<Utc>, Decimal>,
}

impl TransactionPrices {
    fn price_at(&self, at: DateTime<Utc>) -> Option<Decimal> {
        self.prices.get(&at).copied()
    }
}

/// Resolves a price for each timestamp. `None` when the account's network has
/// no market price (test networks).
pub(crate) fn resolve_transaction_prices(
    user_id: UserId,
    asset: SyncedAssetId,
    network: Network,
    timestamps: &[DateTime<Utc>],
) -> Result<Option<TransactionPrices>, DbError> {
    if network != Network::Mainnet {
        return Ok(None);
    }
    let settings = db::load_settings(user_id)?;
    let currency = settings.currency.unwrap_or_else(|| {
        crate::settings::default_currency(settings.language.unwrap_or_default())
    });
    let timezone = settings
        .timezone
        .unwrap_or_else(|| UserTimezone::from(chrono_tz::Tz::UTC));
    let mut prices = HashMap::new();
    let (Some(earliest), Some(latest)) = (timestamps.iter().min(), timestamps.iter().max()) else {
        return Ok(Some(TransactionPrices { currency, prices }));
    };

    let subject = super::prices::native_asset_price_subject(asset);
    let PriceSubject::CatalogAsset(asset_key) = &subject;
    // A local day starts less than two days before any moment in it.
    let overrides = db::list_price_overrides_in_range(
        user_id,
        subject.clone(),
        currency,
        *earliest - Duration::days(2),
        *latest,
    )?;
    let prices_db = settings
        .price_fetching_enabled
        .then(db::open_migrated_prices_db)
        .transpose()?;

    let tz: chrono_tz::Tz = timezone.into();
    let mut daily: HashMap<NaiveDate, Option<Decimal>> = HashMap::new();
    for &at in timestamps {
        let local_date = at.with_timezone(&tz).date_naive();
        let day_start =
            local_report_date_to_utc_boundary(local_date, timezone, DateBoundaryKind::StartOfDay);
        let price = match same_day_override_price(&overrides, day_start, at) {
            Some(price) => Some(price),
            None => match &prices_db {
                Some(conn) => match daily.get(&local_date) {
                    Some(cached) => *cached,
                    None => {
                        let point = db::lookup_daily_price_point(
                            conn,
                            &db::DailyPricePointQuery {
                                asset_id: asset_key.as_str().to_string(),
                                quote_currency: currency,
                                date_utc: local_date,
                            },
                        )?
                        .map(|record| record.price);
                        daily.insert(local_date, point);
                        point
                    }
                },
                None => None,
            },
        };
        if let Some(price) = price {
            prices.insert(at, price);
        }
    }
    Ok(Some(TransactionPrices { currency, prices }))
}

/// The latest of the user's own prices saved between the start of the local
/// day and `at`, matching the reports' same-day lookup.
fn same_day_override_price(
    overrides: &[PriceOverrideRecord],
    day_start: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Option<Decimal> {
    overrides
        .iter()
        .filter(|record| record.price_time_utc >= day_start && record.price_time_utc <= at)
        .max_by_key(|record| record.price_time_utc)
        .map(|record| record.price)
}

fn amount_decimal(amount: UnsignedAmount, decimal_precision: u8) -> Option<Decimal> {
    format_unsigned_amount(amount, decimal_precision)
        .parse()
        .ok()
}

/// Amount times price, rounded to the currency's minor unit.
fn fiat_amount(
    amount: UnsignedAmount,
    price: Decimal,
    decimal_precision: u8,
    currency: CurrencyCode,
) -> Option<Decimal> {
    let value = amount_decimal(amount, decimal_precision)?.checked_mul(price)?;
    let decimal_places = u32::try_from(currency.decimal_places()).ok()?;
    Some(value.round_dp_with_strategy(decimal_places, RoundingStrategy::MidpointAwayFromZero))
}

pub(crate) fn transaction_fiat_view(
    prices: &TransactionPrices,
    occurred_at: DateTime<Utc>,
    value: UnsignedAmount,
    fee: Option<UnsignedAmount>,
    decimal_precision: u8,
) -> Option<TransactionFiatView> {
    let price = prices.price_at(occurred_at)?;
    Some(TransactionFiatView {
        price,
        value: fiat_amount(value, price, decimal_precision, prices.currency)?,
        fee: fee.and_then(|fee| fiat_amount(fee, price, decimal_precision, prices.currency)),
    })
}

/// Money in and out of the account for one settled row, fees excluded. A failed
/// transaction moves no value. A self-transfer can still receive or send (an
/// Ethereum call that returns or forwards ether), so its net is read from its
/// closing balance against the previous row's; when either is unknown it counts
/// as moving nothing but its fee.
fn row_flows(row: &AccountTransactionFlowRow) -> (UnsignedAmount, UnsignedAmount) {
    let zero = UnsignedAmount::zero();
    if row.status != ChainTransactionStatus::Confirmed {
        return (zero, zero);
    }
    match row.direction {
        AccountTransactionDirection::Incoming => (row.value, zero),
        AccountTransactionDirection::Outgoing => (zero, row.value),
        AccountTransactionDirection::SelfTransfer => {
            let (Some(closing), Some(previous)) =
                (row.closing_balance, row.previous_closing_balance)
            else {
                return (zero, zero);
            };
            let fee = row.fee.map_or(0, UnsignedAmount::value);
            let after_fee = closing.value().saturating_add(fee);
            if after_fee >= previous.value() {
                (
                    UnsignedAmount::from_u128(after_fee - previous.value()),
                    zero,
                )
            } else {
                (
                    zero,
                    UnsignedAmount::from_u128(previous.value() - after_fee),
                )
            }
        }
    }
}

/// Received and sent are money in and out (see `row_flows`); fees count every
/// fee the account paid, failed transactions included, so opening + received -
/// sent - fees = closing. Fiat totals are the sums of the rounded
/// per-transaction values, so they add up to what the rows show.
pub(crate) fn summarize_flows(
    rows: &[AccountTransactionFlowRow],
    prices: Option<&TransactionPrices>,
    decimal_precision: u8,
) -> Result<Option<Box<AccountFlowSummaryView>>, crate::amounts::AmountError> {
    if rows.is_empty() {
        return Ok(None);
    }
    let mut received = UnsignedAmount::zero();
    let mut sent = UnsignedAmount::zero();
    let mut fees = UnsignedAmount::zero();
    let mut fiat = AccountFlowFiatView {
        received: Decimal::ZERO,
        sent: Decimal::ZERO,
        fees: Decimal::ZERO,
    };
    let mut unpriced_transactions = 0_u32;

    for row in rows {
        let (inflow, outflow) = row_flows(row);
        let fee = row.fee.unwrap_or(UnsignedAmount::zero());
        let parts = [
            (inflow, &mut received, &mut fiat.received),
            (outflow, &mut sent, &mut fiat.sent),
            (fee, &mut fees, &mut fiat.fees),
        ];
        if parts
            .iter()
            .all(|(amount, _, _)| *amount == UnsignedAmount::zero())
        {
            continue;
        }
        let price = prices.and_then(|prices| prices.price_at(row.occurred_at));
        let mut priced = price.is_some();
        for (amount, total, fiat_total) in parts {
            if amount == UnsignedAmount::zero() {
                continue;
            }
            *total = total.checked_add(amount)?;
            add_fiat(
                fiat_total,
                amount,
                price,
                decimal_precision,
                prices,
                &mut priced,
            );
        }
        if !priced {
            unpriced_transactions = unpriced_transactions.saturating_add(1);
        }
    }

    let amount_view = |amount: UnsignedAmount| crate::backend::BalanceAmountView {
        raw_value: amount.raw_string(),
        formatted_value: format_unsigned_amount(amount, decimal_precision),
    };
    Ok(Some(Box::new(AccountFlowSummaryView {
        received: amount_view(received),
        sent: amount_view(sent),
        fees: amount_view(fees),
        fiat: (prices.is_some() && unpriced_transactions == 0).then_some(fiat),
        unpriced_transactions,
    })))
}

fn add_fiat(
    total: &mut Decimal,
    amount: UnsignedAmount,
    price: Option<Decimal>,
    decimal_precision: u8,
    prices: Option<&TransactionPrices>,
    priced: &mut bool,
) {
    let value = price
        .zip(prices)
        .and_then(|(price, prices)| fiat_amount(amount, price, decimal_precision, prices.currency));
    match value.and_then(|value| total.checked_add(value)) {
        Some(sum) => *total = sum,
        None => *priced = false,
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    fn dec(value: &str) -> Decimal {
        Decimal::from_str(value).expect("valid decimal")
    }

    fn eur() -> CurrencyCode {
        CurrencyCode::from_code("EUR").expect("EUR")
    }

    fn sats(value: u128) -> UnsignedAmount {
        UnsignedAmount::from_u128(value)
    }

    fn override_at(time: &str, price: &str) -> PriceOverrideRecord {
        PriceOverrideRecord {
            id: time.to_string(),
            subject: PriceSubject::CatalogAsset(crate::asset_views::CatalogAssetKey::from_trusted(
                "bitcoin".to_string(),
            )),
            quote_currency: eur(),
            price_time_utc: at(time),
            price: dec(price),
            source_note: None,
            created_at: at(time),
            updated_at: at(time),
        }
    }

    fn flow(
        status: ChainTransactionStatus,
        direction: AccountTransactionDirection,
        time: &str,
        value: u128,
        fee: Option<u128>,
    ) -> AccountTransactionFlowRow {
        AccountTransactionFlowRow {
            status,
            direction,
            occurred_at: at(time),
            value: sats(value),
            fee: fee.map(sats),
            closing_balance: None,
            previous_closing_balance: None,
        }
    }

    fn self_transfer(previous: u128, closing: u128, fee: u128) -> AccountTransactionFlowRow {
        AccountTransactionFlowRow {
            closing_balance: Some(sats(closing)),
            previous_closing_balance: Some(sats(previous)),
            ..flow(
                ChainTransactionStatus::Confirmed,
                AccountTransactionDirection::SelfTransfer,
                "2026-01-06T10:00:00Z",
                1_000,
                Some(fee),
            )
        }
    }

    #[test]
    fn self_transfer_flows_come_from_the_balance_change_before_its_fee() {
        let zero = UnsignedAmount::zero();
        // Plain move to an own address: only the fee leaves.
        assert_eq!(row_flows(&self_transfer(10_000, 9_900, 100)), (zero, zero));
        // A call that also returned 2,000.
        assert_eq!(
            row_flows(&self_transfer(10_000, 11_900, 100)),
            (sats(2_000), zero)
        );
        // A call that also forwarded 3,000 elsewhere.
        assert_eq!(
            row_flows(&self_transfer(10_000, 6_900, 100)),
            (zero, sats(3_000))
        );
        let mut unknown = self_transfer(10_000, 6_900, 100);
        unknown.previous_closing_balance = None;
        assert_eq!(row_flows(&unknown), (zero, zero));
    }

    #[test]
    fn same_day_override_takes_the_latest_saved_at_or_before_the_moment() {
        let overrides = [
            override_at("2026-01-09T23:00:00Z", "1"),
            override_at("2026-01-10T08:00:00Z", "2"),
            override_at("2026-01-10T12:00:00Z", "3"),
            override_at("2026-01-10T18:00:00Z", "4"),
        ];
        let day_start = at("2026-01-10T00:00:00Z");
        assert_eq!(
            same_day_override_price(&overrides, day_start, at("2026-01-10T13:00:00Z")),
            Some(dec("3"))
        );
        // The previous day's price never carries over.
        assert_eq!(
            same_day_override_price(&overrides, day_start, at("2026-01-10T07:00:00Z")),
            None
        );
    }

    #[test]
    fn fiat_amount_rounds_half_away_from_zero_to_the_currency_minor_unit() {
        // 0.12345678 BTC at 91,850.00 = 11,339.505263 -> 11,339.51
        assert_eq!(
            fiat_amount(sats(12_345_678), dec("91850.00"), 8, eur()),
            Some(dec("11339.51"))
        );
        assert_eq!(
            fiat_amount(
                sats(50_000_000),
                dec("1.01"),
                8,
                CurrencyCode::from_code("JPY").expect("JPY")
            ),
            Some(dec("1"))
        );
    }

    #[test]
    fn summary_counts_value_for_confirmed_and_fees_for_failed_too() {
        let rows = [
            flow(
                ChainTransactionStatus::Confirmed,
                AccountTransactionDirection::Incoming,
                "2026-01-01T10:00:00Z",
                100_000_000,
                None,
            ),
            flow(
                ChainTransactionStatus::Confirmed,
                AccountTransactionDirection::Outgoing,
                "2026-01-02T10:00:00Z",
                30_000_000,
                Some(1_000),
            ),
            flow(
                ChainTransactionStatus::Failed,
                AccountTransactionDirection::Outgoing,
                "2026-01-03T10:00:00Z",
                5_000_000,
                Some(2_000),
            ),
            flow(
                ChainTransactionStatus::Confirmed,
                AccountTransactionDirection::SelfTransfer,
                "2026-01-04T10:00:00Z",
                7_000_000,
                Some(500),
            ),
            flow(
                ChainTransactionStatus::Confirmed,
                AccountTransactionDirection::SelfTransfer,
                "2026-01-05T10:00:00Z",
                7_000_000,
                None,
            ),
        ];
        let prices = TransactionPrices {
            currency: eur(),
            prices: rows
                .iter()
                .map(|row| (row.occurred_at, dec("50000")))
                .collect(),
        };

        let summary = summarize_flows(&rows, Some(&prices), 8)
            .expect("no overflow")
            .expect("rows present");

        assert_eq!(summary.received.formatted_value, "1");
        assert_eq!(summary.sent.formatted_value, "0.3");
        assert_eq!(summary.fees.formatted_value, "0.000035");
        assert_eq!(summary.unpriced_transactions, 0);
        assert_eq!(
            summary.fiat,
            Some(AccountFlowFiatView {
                received: dec("50000.00"),
                sent: dec("15000.00"),
                // 0.5 + 1.0 + 0.25
                fees: dec("1.75"),
            })
        );
    }

    #[test]
    fn summary_drops_fiat_totals_when_any_counted_transaction_is_unpriced() {
        let rows = [
            flow(
                ChainTransactionStatus::Confirmed,
                AccountTransactionDirection::Incoming,
                "2026-01-01T10:00:00Z",
                100,
                None,
            ),
            flow(
                ChainTransactionStatus::Confirmed,
                AccountTransactionDirection::Incoming,
                "2026-01-02T10:00:00Z",
                100,
                None,
            ),
            // Moves nothing and pays nothing, so a missing price is irrelevant.
            flow(
                ChainTransactionStatus::Confirmed,
                AccountTransactionDirection::SelfTransfer,
                "2026-01-03T10:00:00Z",
                100,
                None,
            ),
        ];
        let prices = TransactionPrices {
            currency: eur(),
            prices: HashMap::from([(at("2026-01-01T10:00:00Z"), dec("1"))]),
        };

        let summary = summarize_flows(&rows, Some(&prices), 8)
            .expect("no overflow")
            .expect("rows present");

        assert_eq!(summary.unpriced_transactions, 1);
        assert_eq!(summary.fiat, None);
        assert_eq!(
            summarize_flows(&[], Some(&prices), 8).expect("no overflow"),
            None
        );
    }
}
