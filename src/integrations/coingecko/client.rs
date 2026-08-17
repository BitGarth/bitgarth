//! CoinGecko HTTP client.
//!
//! Thin wrapper around `TracedBlockingClient` that builds URLs and decodes
//! responses for the CoinGecko REST API. Server-only.

use super::{
    CoinGeckoCredentialMode, CoingeckoCoinDetail, CoingeckoListCoin,
    CoingeckoMarketChartRangeResponse, SimplePriceResponse,
};
use crate::traces::client::{
    TracedBlockingClient, TransportErrorKind, TransportFailure, TransportFailureStage,
};
use url::Url;

const PUBLIC_BASE_URL: &str = "https://api.coingecko.com/api/v3/";
const PRO_BASE_URL: &str = "https://pro-api.coingecko.com/api/v3/";
const PRO_HEADER_NAME: &str = "x-cg-pro-api-key";

#[cfg(feature = "dev-config")]
const COINGECKO_BASE_URL_ENV: &str = "BITGARTH_COINGECKO_BASE_URL";

/// Dev/test-only base URL override (e.g. the screenshot/e2e harness pointing at
/// a local mock CoinGecko). Gated behind `dev-config` so production builds never
/// read it. Mirrors `BITGARTH_CENTRAL_BASE_URL`.
#[cfg(feature = "dev-config")]
fn dev_base_url_override() -> Result<Option<Url>, CoingeckoError> {
    match std::env::var(COINGECKO_BASE_URL_ENV) {
        Ok(value) if !value.trim().is_empty() => Url::parse(value.trim())
            .map(Some)
            .map_err(|err| CoingeckoError::Url(err.to_string())),
        _ => Ok(None),
    }
}

#[derive(Debug)]
pub(crate) enum CoingeckoError {
    Url(String),
    Transport(TransportFailure),
    Decode(String),
    Api {
        status_code: u16,
        error_code: u32,
        error_message: String,
        retry_after: Option<String>,
    },
    UnexpectedResponse {
        status_code: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
}

impl std::fmt::Display for CoingeckoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(message) => write!(f, "coingecko url error: {message}"),
            Self::Transport(failure) => {
                write!(
                    f,
                    "coingecko transport error: {}",
                    failure.persistence_message()
                )
            }
            Self::Decode(message) => write!(f, "coingecko decode error: {message}"),
            Self::Api {
                status_code,
                error_code,
                error_message,
                retry_after,
            } => {
                write!(
                    f,
                    "coingecko api error {status_code} (error_code={error_code}): {error_message}"
                )?;
                if let Some(ra) = retry_after {
                    write!(f, " (Retry-After: {ra})")?;
                }
                Ok(())
            }
            Self::UnexpectedResponse {
                status_code,
                headers,
                body,
            } => {
                write!(f, "coingecko unexpected response {status_code}\nheaders:")?;
                for (k, v) in headers {
                    write!(f, "\n  {k}: {v}")?;
                }
                write!(f, "\nbody: {body}")
            }
        }
    }
}

impl std::error::Error for CoingeckoError {}

impl CoingeckoError {
    pub(crate) fn is_history_limit(&self) -> bool {
        matches!(
            self,
            CoingeckoError::Api {
                error_code: 10012,
                ..
            }
        )
    }

    pub(crate) fn is_rate_limited(&self) -> bool {
        matches!(
            self,
            CoingeckoError::Api {
                status_code: 429,
                ..
            } | CoingeckoError::UnexpectedResponse {
                status_code: 429,
                ..
            }
        )
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CoinGeckoRequestConfig {
    pub(crate) base_url: Url,
    pub(crate) header: Option<(String, String)>,
    pub(crate) license_scope: &'static str,
}

impl std::fmt::Debug for CoinGeckoRequestConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let header = self
            .header
            .as_ref()
            .map(|(name, _value)| (name.as_str(), "***REDACTED***"));
        f.debug_struct("CoinGeckoRequestConfig")
            .field("base_url", &self.base_url)
            .field("header", &header)
            .field("license_scope", &self.license_scope)
            .finish()
    }
}

impl CoinGeckoCredentialMode {
    pub(crate) fn request_config(&self) -> Result<CoinGeckoRequestConfig, CoingeckoError> {
        let config = match self {
            Self::PublicKeyless => CoinGeckoRequestConfig {
                base_url: Url::parse(PUBLIC_BASE_URL)
                    .map_err(|err| CoingeckoError::Url(err.to_string()))?,
                header: None,
                license_scope: "public_keyless",
            },
            Self::Pro { api_key } => CoinGeckoRequestConfig {
                base_url: Url::parse(PRO_BASE_URL)
                    .map_err(|err| CoingeckoError::Url(err.to_string()))?,
                header: Some((PRO_HEADER_NAME.to_string(), api_key.as_str().to_string())),
                license_scope: "coingecko_pro_key",
            },
        };
        #[cfg(feature = "dev-config")]
        let config = match dev_base_url_override()? {
            Some(base_url) => CoinGeckoRequestConfig { base_url, ..config },
            None => config,
        };
        Ok(config)
    }
}

pub(crate) struct CoingeckoClient {
    client: TracedBlockingClient,
    base_url: Url,
    header: Option<(String, String)>,
    license_scope: &'static str,
}

pub(super) fn simple_price_path(ids: &[&str], vs_currency: &str) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("ids", &ids.join(","));
    query.append_pair("vs_currencies", vs_currency);
    query.append_pair("include_last_updated_at", "false");
    format!("simple/price?{}", query.finish())
}

pub(super) fn coins_list_path(include_platform: bool) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair(
        "include_platform",
        if include_platform { "true" } else { "false" },
    );
    format!("coins/list?{}", query.finish())
}

pub(super) fn coin_detail_path(id: &str) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("localization", "false");
    query.append_pair("tickers", "false");
    query.append_pair("market_data", "false");
    query.append_pair("community_data", "false");
    query.append_pair("developer_data", "false");
    query.append_pair("sparkline", "false");
    query.append_pair("include_categories_details", "false");
    format!("coins/{id}?{}", query.finish())
}

pub(super) fn market_chart_range_path(
    id: &str,
    vs_currency: &str,
    from_unix_seconds: i64,
    to_unix_seconds: i64,
) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("vs_currency", vs_currency);
    query.append_pair("from", &from_unix_seconds.to_string());
    query.append_pair("to", &to_unix_seconds.to_string());
    query.append_pair("interval", "daily");
    format!("coins/{id}/market_chart/range?{}", query.finish())
}

impl CoingeckoClient {
    pub(crate) fn new(
        client: TracedBlockingClient,
        base_url: Url,
        api_key: Option<String>,
    ) -> Self {
        let header = api_key.map(|api_key| ("x-cg-demo-api-key".to_string(), api_key));
        let license_scope = if header.is_some() {
            "public_demo_key"
        } else {
            "public_keyless"
        };
        Self {
            client,
            base_url,
            header,
            license_scope,
        }
    }

    pub(crate) fn from_credential_mode(
        client: TracedBlockingClient,
        mode: CoinGeckoCredentialMode,
    ) -> Result<Self, CoingeckoError> {
        let config = mode.request_config()?;
        Ok(Self {
            client,
            base_url: config.base_url,
            header: config.header,
            license_scope: config.license_scope,
        })
    }

    pub(crate) fn license_scope(&self) -> &'static str {
        self.license_scope
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, CoingeckoError> {
        use super::CoingeckoApiErrorBody;

        let url = self
            .base_url
            .join(path)
            .map_err(|err| CoingeckoError::Url(err.to_string()))?;
        let request = self.client.get(url.as_str());
        let request = match &self.header {
            Some((name, value)) => request.header(name.as_str(), value.as_str()),
            None => request,
        };
        let response = request.send().map_err(|err| {
            CoingeckoError::Transport(TransportFailure::from_reqwest_error(
                TransportFailureStage::SendFailed,
                &err,
            ))
        })?;
        let status_code = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    v.to_str().unwrap_or("<non-utf8>").to_string(),
                )
            })
            .collect();
        let body = response.text().map_err(|err| {
            CoingeckoError::Transport(TransportFailure::new(
                TransportFailureStage::ResponseBodyReadFailed,
                err.to_string(),
                TransportErrorKind::Decode,
            ))
        })?;

        if !(200..300).contains(&(status_code as usize)) {
            if let Ok(api_error) = serde_json::from_str::<CoingeckoApiErrorBody>(&body) {
                return Err(CoingeckoError::Api {
                    status_code,
                    error_code: api_error.status.error_code,
                    error_message: api_error.status.error_message,
                    retry_after,
                });
            }
            return Err(CoingeckoError::UnexpectedResponse {
                status_code,
                headers,
                body,
            });
        }

        serde_json::from_str(&body).map_err(|err| CoingeckoError::Decode(err.to_string()))
    }

    /// GET /coins/{id} with lean detail flags.
    pub(crate) fn coin_detail(&self, id: &str) -> Result<CoingeckoCoinDetail, CoingeckoError> {
        self.get_json(&coin_detail_path(id))
    }

    /// GET /coins/list for the public CoinGecko discovery catalog.
    pub(crate) fn coins_list(
        &self,
        include_platform: bool,
    ) -> Result<Vec<CoingeckoListCoin>, CoingeckoError> {
        self.get_json(&coins_list_path(include_platform))
    }

    /// GET /simple/price for the given ids in `vs_currency` (already lowercased).
    pub(crate) fn simple_price(
        &self,
        ids: &[&str],
        vs_currency: &str,
    ) -> Result<SimplePriceResponse, CoingeckoError> {
        self.get_json(&simple_price_path(ids, vs_currency))
    }

    /// GET /coins/{id}/market_chart/range for historical market prices.
    pub(crate) fn market_chart_range(
        &self,
        id: &str,
        vs_currency: &str,
        from_unix_seconds: i64,
        to_unix_seconds: i64,
    ) -> Result<CoingeckoMarketChartRangeResponse, CoingeckoError> {
        self.get_json(&market_chart_range_path(
            id,
            vs_currency,
            from_unix_seconds,
            to_unix_seconds,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::coingecko::CoinGeckoCredentialMode;
    use crate::models::SimpleApiKey;
    use crate::models::UserId;
    use crate::traces::client::IntegrationLabel;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn credential_mode_selects_public_request_config() {
        let mode = CoinGeckoCredentialMode::PublicKeyless;
        let config = mode.request_config().expect("public config");

        assert_eq!(
            config.base_url.as_str(),
            "https://api.coingecko.com/api/v3/"
        );
        assert!(config.header.is_none());
        assert_eq!(config.license_scope, "public_keyless");
    }

    #[test]
    fn credential_mode_selects_pro_request_config() {
        let mode = CoinGeckoCredentialMode::Pro {
            api_key: SimpleApiKey::new("PRO_KEY".to_string()).expect("key"),
        };
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
    fn request_config_debug_redacts_header_value() {
        let mode = CoinGeckoCredentialMode::Pro {
            api_key: SimpleApiKey::new("PRO_KEY".to_string()).expect("key"),
        };
        let config = mode.request_config().expect("pro config");
        let debug = format!("{config:?}");

        assert!(debug.contains("x-cg-pro-api-key"));
        assert!(debug.contains("***REDACTED***"));
        assert!(!debug.contains("PRO_KEY"));
    }

    #[test]
    fn error_helper_treats_api_and_unexpected_429_as_rate_limited() {
        let api_error = CoingeckoError::Api {
            status_code: 429,
            error_code: 0,
            error_message: "rate limited".to_string(),
            retry_after: None,
        };
        let unexpected_error = CoingeckoError::UnexpectedResponse {
            status_code: 429,
            headers: Vec::new(),
            body: "too many requests".to_string(),
        };

        assert!(api_error.is_rate_limited());
        assert!(unexpected_error.is_rate_limited());
    }

    #[test]
    fn simple_price_parses_decimal_without_f64() {
        let body = r#"{"bitcoin":{"usd":0.123456789123456789},"ethereum":{"usd":2701.5}}"#;
        let parsed: super::super::SimplePriceResponse = serde_json::from_str(body).unwrap();
        let btc = parsed.get("bitcoin").unwrap().get("usd").unwrap();
        assert_eq!(
            Decimal::from_str(btc.get()).unwrap(),
            Decimal::from_str("0.123456789123456789").unwrap()
        );
    }

    #[test]
    fn simple_price_path_joins_ids_and_currency() {
        let path = super::simple_price_path(&["bitcoin", "ethereum"], "eur");
        assert_eq!(
            path,
            "simple/price?ids=bitcoin%2Cethereum&vs_currencies=eur&include_last_updated_at=false"
        );
    }

    #[test]
    fn coins_list_path_encodes_platform_flag() {
        assert_eq!(
            super::coins_list_path(true),
            "coins/list?include_platform=true"
        );
        assert_eq!(
            super::coins_list_path(false),
            "coins/list?include_platform=false"
        );
    }

    #[test]
    fn coin_detail_path_uses_lean_flags() {
        assert_eq!(
            super::coin_detail_path("usd-coin"),
            "coins/usd-coin?localization=false&tickers=false&market_data=false&community_data=false&developer_data=false&sparkline=false&include_categories_details=false"
        );
    }

    #[test]
    fn coins_list_response_decodes() {
        let body = r#"[{
            "id": "cardano",
            "symbol": "ada",
            "name": "Cardano",
            "platforms": {}
        }]"#;
        let parsed: Vec<CoingeckoListCoin> = serde_json::from_str(body).expect("decode");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "cardano");
        assert_eq!(parsed[0].symbol, "ada");
        assert!(parsed[0].platforms.is_empty());
    }

    #[test]
    fn client_surface_is_constructible_without_network() {
        let traced_client = TracedBlockingClient::builder(
            IntegrationLabel::new("coingecko-catalog-gen"),
            UserId::new(),
        )
        .build()
        .expect("traced client");
        let base_url = Url::parse("https://api.coingecko.com/api/v3/").expect("base url");
        let client = CoingeckoClient::new(traced_client, base_url, None);
        assert_eq!(client.license_scope(), "public_keyless");

        let traced_client = TracedBlockingClient::builder(
            IntegrationLabel::new("coingecko-simple-price"),
            UserId::new(),
        )
        .build()
        .expect("traced client");
        let credential_client = CoingeckoClient::from_credential_mode(
            traced_client,
            CoinGeckoCredentialMode::PublicKeyless,
        )
        .expect("credential client");
        assert_eq!(credential_client.license_scope(), "public_keyless");

        let coin_detail: fn(&CoingeckoClient, &str) -> Result<CoingeckoCoinDetail, CoingeckoError> =
            CoingeckoClient::coin_detail;
        let simple_price: fn(
            &CoingeckoClient,
            &[&str],
            &str,
        ) -> Result<SimplePriceResponse, CoingeckoError> = CoingeckoClient::simple_price;
        let coins_list: fn(
            &CoingeckoClient,
            bool,
        ) -> Result<Vec<CoingeckoListCoin>, CoingeckoError> = CoingeckoClient::coins_list;

        let _ = (client, coin_detail, simple_price, coins_list);
    }
}
