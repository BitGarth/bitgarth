//! CoinGecko integration.
//!
//! Owns the CoinGecko REST surface used by the catalog generator: per-coin
//! detail (contract addresses + decimal precision). Accepts a
//! `TracedBlockingClient` constructed by the caller. Server-only.

#[cfg(feature = "server")]
pub(crate) mod client;

#[cfg(feature = "server")]
use crate::models::SimpleApiKey;
#[cfg(any(feature = "server", test))]
use chrono::{DateTime, NaiveDate, Utc};
#[cfg(any(feature = "server", test))]
use rust_decimal::Decimal;

#[cfg(feature = "server")]
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum CoinGeckoCredentialMode {
    PublicKeyless,
    Pro { api_key: SimpleApiKey },
}

#[cfg(feature = "server")]
impl std::fmt::Debug for CoinGeckoCredentialMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PublicKeyless => f.write_str("PublicKeyless"),
            Self::Pro { .. } => f
                .debug_struct("Pro")
                .field("api_key", &"***REDACTED***")
                .finish(),
        }
    }
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MarketPrice(pub(crate) Decimal);

#[cfg(any(feature = "server", test))]
impl MarketPrice {
    pub(crate) fn parse_json_raw(raw: &str) -> Result<Self, String> {
        let decimal = if raw.contains('e') || raw.contains('E') {
            Decimal::from_scientific(raw)
        } else {
            Decimal::from_str_exact(raw)
        }
        .map_err(|err| format!("invalid market price decimal {raw}: {err}"))?;
        Ok(Self(decimal))
    }

    #[cfg(any(feature = "server", test))]
    pub(crate) fn as_decimal(&self) -> Decimal {
        self.0
    }
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CoinGeckoDailyPrice {
    pub(crate) price_time: DateTime<Utc>,
    pub(crate) date_utc: NaiveDate,
    pub(crate) price: MarketPrice,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct CoingeckoMarketChartRangeResponse {
    pub(crate) prices: Vec<(i64, Box<serde_json::value::RawValue>)>,
}

#[cfg(any(feature = "server", test))]
impl CoingeckoMarketChartRangeResponse {
    pub(crate) fn into_daily_prices(self) -> Result<Vec<CoinGeckoDailyPrice>, String> {
        let mut prices = self.prices;
        prices.sort_by_key(|(millis, _)| *millis);

        let mut daily_prices = std::collections::BTreeMap::new();
        for (millis, raw_price) in prices {
            let price_time = DateTime::<Utc>::from_timestamp_millis(millis)
                .ok_or_else(|| format!("invalid CoinGecko price timestamp: {millis}"))?;
            let price = MarketPrice::parse_json_raw(raw_price.get())?;
            daily_prices.insert(
                price_time.date_naive(),
                CoinGeckoDailyPrice {
                    price_time,
                    date_utc: price_time.date_naive(),
                    price,
                },
            );
        }

        Ok(daily_prices.into_values().collect())
    }
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct CoingeckoListCoin {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) platforms: std::collections::HashMap<String, String>,
}

/// One platform entry from /coins/{id} `detail_platforms`.
#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub(crate) struct CoingeckoPlatformDetail {
    #[serde(default)]
    pub(crate) contract_address: String,
    #[serde(default)]
    pub(crate) decimal_place: Option<u8>,
}

/// Subset of /coins/{id} we consume.
#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub(crate) struct CoingeckoCoinDetail {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) symbol: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) web_slug: String,
    #[serde(default)]
    pub(crate) market_cap_rank: Option<u32>,
    #[serde(default)]
    pub(crate) detail_platforms: std::collections::HashMap<String, CoingeckoPlatformDetail>,
}

/// Error envelope returned by CoinGecko on non-2xx responses.
#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub(crate) struct CoingeckoApiErrorBody {
    pub(crate) status: CoingeckoApiErrorStatus,
}

#[cfg(any(feature = "server", test))]
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub(crate) struct CoingeckoApiErrorStatus {
    pub(crate) error_code: u32,
    pub(crate) error_message: String,
}

/// Response of GET /simple/price: { "<coingecko_id>": { "<currency>": <number> } }.
///
/// Prices are decoded as raw JSON numbers and converted to Decimal text by the
/// caller; never via f64 on the value path.
#[cfg(any(feature = "server", test))]
pub(crate) type SimplePriceResponse = std::collections::HashMap<
    String,
    std::collections::HashMap<String, Box<serde_json::value::RawValue>>,
>;

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "server")]
    use crate::models::SimpleApiKey;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn decodes_detail_platforms_with_decimal_place() {
        let json = r#"{
            "id": "usd-coin",
            "symbol": "usdc",
            "name": "USD Coin",
            "web_slug": "usd-coin",
            "market_cap_rank": 7,
            "detail_platforms": {
                "ethereum": { "contract_address": "0xA0b8...", "decimal_place": 6 },
                "": { "contract_address": "", "decimal_place": null }
            }
        }"#;
        let detail: CoingeckoCoinDetail = serde_json::from_str(json).expect("decode");
        assert_eq!(detail.id, "usd-coin");
        assert_eq!(detail.symbol, "usdc");
        assert_eq!(detail.name, "USD Coin");
        assert_eq!(detail.web_slug, "usd-coin");
        assert_eq!(detail.market_cap_rank, Some(7));
        assert_eq!(
            detail
                .detail_platforms
                .get("ethereum")
                .and_then(|p| p.decimal_place),
            Some(6)
        );
    }

    #[test]
    fn decodes_list_coin_with_platforms() {
        let json = r#"{
            "id": "usd-coin",
            "symbol": "usdc",
            "name": "USD Coin",
            "platforms": {
                "ethereum": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
                "polygon-pos": "0x2791bca1f2de4661ed88a30c99a7a9449aa84174"
            }
        }"#;
        let coin: CoingeckoListCoin = serde_json::from_str(json).expect("decode");

        assert_eq!(coin.id, "usd-coin");
        assert_eq!(coin.symbol, "usdc");
        assert_eq!(coin.name, "USD Coin");
        assert_eq!(
            coin.platforms.get("ethereum").map(String::as_str),
            Some("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48")
        );
        assert_eq!(
            coin.platforms.get("polygon-pos").map(String::as_str),
            Some("0x2791bca1f2de4661ed88a30c99a7a9449aa84174")
        );
    }

    #[test]
    fn daily_price_boundary_type_is_constructible() {
        let price = MarketPrice(Decimal::from_str("12.34").expect("decimal"));
        let price_time = DateTime::parse_from_rfc3339("2026-06-06T12:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc);
        let date_utc = NaiveDate::from_ymd_opt(2026, 6, 6).expect("date");

        let daily = CoinGeckoDailyPrice {
            price_time,
            date_utc,
            price,
        };

        assert_eq!(daily.price.0, Decimal::from_str("12.34").expect("decimal"));
        assert_eq!(daily.date_utc, date_utc);
    }

    #[test]
    fn market_price_parses_scientific_notation_without_f64() {
        let price = MarketPrice::parse_json_raw("9.7e-7").expect("scientific decimal");

        assert_eq!(price.as_decimal().to_string(), "0.00000097");
    }

    #[test]
    fn market_price_rejects_unsupported_json_value_forms() {
        for raw in ["null", r#""12.34""#, r#"{"value":12.34}"#] {
            assert!(
                MarketPrice::parse_json_raw(raw).is_err(),
                "{raw} should not parse as market price"
            );
        }
    }

    #[cfg(feature = "server")]
    #[test]
    fn market_chart_range_path_encodes_unix_range_and_currency() {
        assert_eq!(
            client::market_chart_range_path("bitcoin", "usd", 1_717_891_200, 1_717_977_600),
            "coins/bitcoin/market_chart/range?vs_currency=usd&from=1717891200&to=1717977600&interval=daily"
        );
    }

    #[test]
    fn market_chart_response_buckets_daily_prices_by_utc_date() {
        let raw = r#"{"prices":[[1717977600000,30.0],[1717891200000,10.0],[1717934400000,20.0]]}"#;
        let response: CoingeckoMarketChartRangeResponse =
            serde_json::from_str(raw).expect("response should decode");
        let prices = response
            .into_daily_prices()
            .expect("daily prices should parse");

        assert_eq!(prices.len(), 2);
        assert_eq!(prices[0].date_utc.to_string(), "2024-06-09");
        assert_eq!(prices[0].price_time.timestamp_millis(), 1_717_934_400_000);
        assert_eq!(prices[0].price.as_decimal().to_string(), "20.0");
        assert_eq!(prices[1].date_utc.to_string(), "2024-06-10");
        assert_eq!(prices[1].price.as_decimal().to_string(), "30.0");
    }

    #[test]
    fn market_chart_response_parses_daily_prices_without_f64() {
        let raw = r#"{"prices":[[1717891200000,12345.67890123456789],[1717977600000,12346.00000000000001]]}"#;
        let response: CoingeckoMarketChartRangeResponse =
            serde_json::from_str(raw).expect("response should decode");
        let prices = response
            .into_daily_prices()
            .expect("daily prices should parse");

        assert_eq!(prices[0].date_utc.to_string(), "2024-06-09");
        assert_eq!(
            prices[0].price.as_decimal().to_string(),
            "12345.67890123456789"
        );
        assert_eq!(
            prices[1].price.as_decimal().to_string(),
            "12346.00000000000001"
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn credential_mode_debug_redacts_api_key() {
        let mode = CoinGeckoCredentialMode::Pro {
            api_key: SimpleApiKey::new("PRO_KEY".to_string()).expect("key"),
        };
        let debug = format!("{mode:?}");

        assert!(debug.contains("***REDACTED***"));
        assert!(!debug.contains("PRO_KEY"));
    }
}
