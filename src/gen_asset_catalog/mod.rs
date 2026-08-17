//! Developer-run generator for assets/catalog/unsynced_asset_catalog.json.
//! Run: cargo run --features server -- gen-asset-catalog [IDS | --ids-file PATH] [--out PATH]
//! Uses the public CoinGecko API (https://api.coingecko.com/api/v3/) unless
//! COINGECKO_API_KEY is set, in which case the key is sent as the
//! `x-cg-demo-api-key` header (redacted from traces).

#[cfg(feature = "server")]
use crate::integrations::coingecko::client::{CoingeckoClient, CoingeckoError};
#[cfg(feature = "server")]
use crate::integrations::coingecko::{CoingeckoCoinDetail, CoingeckoPlatformDetail};
#[cfg(feature = "server")]
use crate::models::UserId;
#[cfg(feature = "server")]
use crate::traces::client::{IntegrationLabel, TracedBlockingClient};

#[cfg(feature = "server")]
const DEFAULT_OUT: &str = "assets/catalog/unsynced_asset_catalog.json";

#[cfg(feature = "server")]
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(feature = "server")]
fn usage() -> String {
    "usage: gen-asset-catalog [IDS | --ids-file PATH] [--out PATH]\n  IDS: comma-separated CoinGecko ids (for example: dash,polkadot)\n  PATH: text file with one CoinGecko id per line".to_string()
}

#[cfg(feature = "server")]
fn save_catalog(path: &str, catalog: &UnsyncedCatalogFileOut) -> Result<(), String> {
    let mut json =
        serde_json::to_string_pretty(catalog).map_err(|err| format!("serialize: {err}"))?;
    json.push('\n');
    std::fs::write(path, json).map_err(|err| format!("write {path}: {err}"))
}

#[cfg(feature = "server")]
fn format_coingecko_error(err: &CoingeckoError) -> String {
    format!("[{}] {err}", now_iso())
}

#[cfg(feature = "server")]
pub(crate) fn maybe_run_from_args() -> Result<bool, String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("gen-asset-catalog") => {}
        _ => return Ok(false),
    }
    let parsed = parse_args(args)?;

    let api_key = std::env::var("COINGECKO_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let client = TracedBlockingClient::builder(
        IntegrationLabel::new("coingecko-catalog-gen"),
        UserId::new(),
    )
    .configure(|builder| builder.timeout(std::time::Duration::from_secs(30)))
    .redact_headers(&["x-cg-pro-api-key", "x-cg-demo-api-key"])
    .build()
    .map_err(|err| format!("client build: {err}"))?;

    let base_url = url::Url::parse("https://api.coingecko.com/api/v3/")
        .map_err(|err| format!("base url: {err}"))?;
    let coingecko = CoingeckoClient::new(client, base_url, api_key);
    let ids = match parsed.ids {
        CatalogIds::Inline(ids) => ids,
        CatalogIds::File(path) => read_ids_file(&path)?,
    };
    let result = build_catalog_for_ids(&coingecko, &parsed.out, &ids)?;
    save_catalog(&parsed.out, &result)?;
    eprintln!("wrote {}", parsed.out);
    Ok(true)
}

#[cfg(feature = "server")]
#[derive(Debug)]
struct CatalogArgs {
    out: String,
    ids: CatalogIds,
}

#[cfg(feature = "server")]
#[derive(Debug)]
enum CatalogIds {
    Inline(Vec<String>),
    File(String),
}

#[cfg(feature = "server")]
fn parse_args(args: impl Iterator<Item = String>) -> Result<CatalogArgs, String> {
    let mut out = None;
    let mut ids_file = None;
    let mut inline_ids = None;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ids-file" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--ids-file requires a path".to_string())?;
                if ids_file.replace(path).is_some() {
                    return Err("--ids-file may only be supplied once".to_string());
                }
            }
            "--out" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--out requires a path".to_string())?;
                if out.replace(path).is_some() {
                    return Err("--out may only be supplied once".to_string());
                }
            }
            "--help" | "-h" => return Err(usage()),
            other => {
                if other.starts_with("--") {
                    return Err(format!("unknown argument: {other}"));
                }
                if inline_ids.replace(parse_inline_ids(other)?).is_some() {
                    return Err("inline ids may only be supplied once".to_string());
                }
            }
        }
    }

    let ids = match (inline_ids, ids_file) {
        (Some(ids), None) => CatalogIds::Inline(ids),
        (None, Some(path)) => CatalogIds::File(path),
        (None, None) => return Err(usage()),
        (Some(_), Some(_)) => return Err("supply only one id source".to_string()),
    };
    Ok(CatalogArgs {
        out: out.unwrap_or_else(|| DEFAULT_OUT.to_string()),
        ids,
    })
}

#[cfg(feature = "server")]
fn parse_inline_ids(raw: &str) -> Result<Vec<String>, String> {
    let ids: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        return Err("inline ids must contain at least one CoinGecko id".to_string());
    }
    Ok(ids)
}

#[cfg(feature = "server")]
fn read_ids_file(path: &str) -> Result<Vec<String>, String> {
    let raw = std::fs::read_to_string(path).map_err(|err| format!("read {path}: {err}"))?;
    let ids: Vec<String> = raw
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        return Err(format!(
            "ids file {path} must contain at least one CoinGecko id"
        ));
    }
    Ok(ids)
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct UnsyncedCatalogFileOut {
    schema_version: u32,
    assets: Vec<UnsyncedCatalogRowOut>,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct UnsyncedCatalogRowOut {
    asset_id: String,
    market_cap_rank: u32,
    canonical_name: String,
    unit_code: String,
    symbol: Option<String>,
    network_id: String,
    network_name: String,
    decimal_precision: CatalogPrecisionOut,
    updated_at: String,
    coingecko_id: String,
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
enum CatalogPrecisionOut {
    Known(u8),
    Placeholder,
}

#[cfg(feature = "server")]
impl serde::Serialize for CatalogPrecisionOut {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Known(value) => serializer.serialize_u8(*value),
            Self::Placeholder => serializer.serialize_str("REPLACEME"),
        }
    }
}

#[cfg(feature = "server")]
impl<'de> serde::Deserialize<'de> for CatalogPrecisionOut {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::Number(number) => {
                let raw = number
                    .as_u64()
                    .ok_or_else(|| serde::de::Error::custom("decimal_precision must be u8"))?;
                let parsed = u8::try_from(raw)
                    .map_err(|_| serde::de::Error::custom("decimal_precision must be u8"))?;
                Ok(Self::Known(parsed))
            }
            serde_json::Value::String(value) if value == "REPLACEME" => Ok(Self::Placeholder),
            _ => Err(serde::de::Error::custom(
                "decimal_precision must be a number or REPLACEME",
            )),
        }
    }
}

#[cfg(feature = "server")]
fn load_existing_catalog(path: &str) -> Result<UnsyncedCatalogFileOut, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UnsyncedCatalogFileOut {
                schema_version: 1,
                assets: Vec::new(),
            });
        }
        Err(err) => return Err(format!("read {path}: {err}")),
    };
    let catalog: UnsyncedCatalogFileOut =
        serde_json::from_str(&raw).map_err(|err| format!("parse {path}: {err}"))?;
    if catalog.schema_version != 1 {
        return Err(format!(
            "unsupported unsynced catalog schema_version: {}",
            catalog.schema_version
        ));
    }
    Ok(catalog)
}

#[cfg(feature = "server")]
fn merge_rows(
    existing: Vec<UnsyncedCatalogRowOut>,
    new: Vec<UnsyncedCatalogRowOut>,
) -> Vec<UnsyncedCatalogRowOut> {
    let mut map: std::collections::HashMap<(String, String), UnsyncedCatalogRowOut> = existing
        .into_iter()
        .map(|row| ((row.asset_id.clone(), row.network_id.clone()), row))
        .collect();
    for row in new {
        map.insert((row.asset_id.clone(), row.network_id.clone()), row);
    }
    let mut merged: Vec<UnsyncedCatalogRowOut> = map.into_values().collect();
    merged.sort_by(|left, right| {
        left.market_cap_rank
            .cmp(&right.market_cap_rank)
            .then_with(|| left.asset_id.cmp(&right.asset_id))
            .then_with(|| left.network_id.cmp(&right.network_id))
    });
    merged
}

#[cfg(feature = "server")]
fn build_catalog_for_ids(
    coingecko: &CoingeckoClient,
    out_path: &str,
    ids: &[String],
) -> Result<UnsyncedCatalogFileOut, String> {
    let existing = load_existing_catalog(out_path)?;
    let mut new_rows = Vec::new();
    let mut first_error = None;

    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        match fetch_rows_by_id(coingecko, id) {
            Ok(rows) => new_rows.extend(rows),
            Err(err) => {
                eprintln!("{}", format_coingecko_error(&err));
                first_error = Some(err.to_string());
                break;
            }
        }
    }

    let catalog = UnsyncedCatalogFileOut {
        schema_version: existing.schema_version,
        assets: merge_rows(existing.assets, new_rows),
    };

    if let Some(err) = first_error {
        save_catalog(out_path, &catalog)?;
        return Err(err);
    }

    Ok(catalog)
}

#[cfg(feature = "server")]
fn fetch_rows_by_id(
    coingecko: &CoingeckoClient,
    asset_id: &str,
) -> Result<Vec<UnsyncedCatalogRowOut>, CoingeckoError> {
    let detail = coingecko.coin_detail(asset_id)?;
    Ok(rows_from_detail(asset_id, &detail))
}

#[cfg(feature = "server")]
fn rows_from_detail(asset_id: &str, detail: &CoingeckoCoinDetail) -> Vec<UnsyncedCatalogRowOut> {
    let row_asset_id = if detail.id.trim().is_empty() {
        asset_id
    } else {
        &detail.id
    };
    let canonical_name = placeholder_if_empty(&detail.name);
    let unit_code = coingecko_symbol_to_unit_code(&detail.symbol);
    let market_cap_rank = detail.market_cap_rank.unwrap_or(u32::MAX);
    let coingecko_id = row_asset_id.to_string();
    let updated_at = now_iso();
    let mut rows = token_rows_from_detail_platforms(
        row_asset_id,
        &canonical_name,
        market_cap_rank,
        &unit_code,
        &updated_at,
        &coingecko_id,
        &detail.detail_platforms,
    );

    if rows.is_empty() {
        rows.push(UnsyncedCatalogRowOut {
            asset_id: row_asset_id.to_string(),
            market_cap_rank,
            canonical_name,
            unit_code,
            symbol: None,
            network_id: coingecko_web_slug_to_mainnet_id(&detail.web_slug),
            network_name: placeholder_if_empty(&detail.name),
            decimal_precision: CatalogPrecisionOut::Placeholder,
            updated_at,
            coingecko_id,
        });
    }

    rows
}

#[cfg(feature = "server")]
fn token_rows_from_detail_platforms(
    asset_id: &str,
    canonical_name: &str,
    market_cap_rank: u32,
    unit_code: &str,
    updated_at: &str,
    coingecko_id: &str,
    platforms: &std::collections::HashMap<String, CoingeckoPlatformDetail>,
) -> Vec<UnsyncedCatalogRowOut> {
    let mut platform_rows: Vec<(&String, &CoingeckoPlatformDetail)> = platforms
        .iter()
        .filter(|(_, detail)| !detail.contract_address.trim().is_empty())
        .collect();
    platform_rows.sort_by(|left, right| left.0.cmp(right.0));

    platform_rows
        .into_iter()
        .filter_map(|(platform_slug, detail)| {
            let network = coingecko_platform_to_unsynced_network(platform_slug)?;
            Some(UnsyncedCatalogRowOut {
                asset_id: asset_id.to_string(),
                market_cap_rank,
                canonical_name: format!("{canonical_name} on {}", network.name),
                unit_code: unit_code.to_string(),
                symbol: None,
                network_id: network.id,
                network_name: network.name,
                decimal_precision: catalog_precision_out(detail.decimal_place),
                updated_at: updated_at.to_string(),
                coingecko_id: coingecko_id.to_string(),
            })
        })
        .collect()
}

#[cfg(feature = "server")]
fn catalog_precision_out(value: Option<u8>) -> CatalogPrecisionOut {
    value
        .filter(|precision| *precision <= crate::wallets::ManualAssetDisplayScale::MAX)
        .map(CatalogPrecisionOut::Known)
        .unwrap_or(CatalogPrecisionOut::Placeholder)
}

#[cfg(feature = "server")]
fn placeholder_if_empty(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "REPLACEME".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(feature = "server")]
fn coingecko_symbol_to_unit_code(symbol: &str) -> String {
    let trimmed = symbol.trim();
    if trimmed.is_empty() {
        "REPLACEME".to_string()
    } else {
        trimmed.to_ascii_uppercase()
    }
}

#[cfg(feature = "server")]
fn coingecko_web_slug_to_mainnet_id(web_slug: &str) -> String {
    match sanitize_slug(web_slug) {
        Some(slug) => format!("{slug}-mainnet"),
        None => "REPLACEME".to_string(),
    }
}

#[cfg(feature = "server")]
struct UnsyncedNetworkOut {
    id: String,
    name: String,
}

#[cfg(feature = "server")]
fn coingecko_platform_to_unsynced_network(platform_slug: &str) -> Option<UnsyncedNetworkOut> {
    let known = match platform_slug {
        "ethereum" => Some(("ethereum-mainnet", "Ethereum")),
        "polygon-pos" => Some(("polygon-mainnet", "Polygon")),
        "binance-smart-chain" => Some(("bnb-smart-chain-mainnet", "BNB Smart Chain")),
        "avalanche" => Some(("avalanche-c-chain", "Avalanche C-Chain")),
        "arbitrum-one" => Some(("arbitrum-one", "Arbitrum One")),
        "optimistic-ethereum" => Some(("optimism-mainnet", "Optimism")),
        "base" => Some(("base-mainnet", "Base")),
        "solana" => Some(("solana-mainnet", "Solana")),
        "tron" => Some(("tron-mainnet", "Tron")),
        "algorand" => Some(("algorand-mainnet", "Algorand")),
        "stellar" => Some(("stellar-mainnet", "Stellar")),
        "cardano" => Some(("cardano-mainnet", "Cardano")),
        _ => None,
    };
    if let Some((id, name)) = known {
        return Some(UnsyncedNetworkOut {
            id: id.to_string(),
            name: name.to_string(),
        });
    }

    let id = sanitize_slug(platform_slug)?;
    Some(UnsyncedNetworkOut {
        id,
        name: title_case_slug(platform_slug),
    })
}

#[cfg(feature = "server")]
fn sanitize_slug(value: &str) -> Option<String> {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for character in value.trim().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }
    if last_was_dash {
        slug.pop();
    }
    if slug.is_empty() { None } else { Some(slug) }
}

#[cfg(feature = "server")]
fn title_case_slug(value: &str) -> String {
    let words: Vec<String> = value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            let Some(first) = characters.next() else {
                return String::new();
            };
            format!(
                "{}{}",
                first.to_ascii_uppercase(),
                characters.as_str().to_ascii_lowercase()
            )
        })
        .collect();
    if words.is_empty() {
        "REPLACEME".to_string()
    } else {
        words.join(" ")
    }
}

#[cfg(all(test, feature = "server"))]
pub(crate) fn sample_catalog_json_for_test() -> String {
    serde_json::to_string(&UnsyncedCatalogFileOut {
        schema_version: 1,
        assets: vec![
            UnsyncedCatalogRowOut {
                asset_id: "usd-coin".to_string(),
                market_cap_rank: 7,
                canonical_name: "USD Coin on Ethereum".to_string(),
                unit_code: "USDC".to_string(),
                symbol: None,
                network_id: "ethereum-mainnet".to_string(),
                network_name: "Ethereum".to_string(),
                decimal_precision: CatalogPrecisionOut::Known(6),
                updated_at: "2026-06-01T00:00:00Z".to_string(),
                coingecko_id: "usd-coin".to_string(),
            },
            UnsyncedCatalogRowOut {
                asset_id: "cardano".to_string(),
                market_cap_rank: 9,
                canonical_name: "Cardano".to_string(),
                unit_code: "ADA".to_string(),
                symbol: Some("A".to_string()),
                network_id: "cardano-mainnet".to_string(),
                network_name: "Cardano".to_string(),
                decimal_precision: CatalogPrecisionOut::Known(6),
                updated_at: "2026-06-01T00:00:00Z".to_string(),
                coingecko_id: "cardano".to_string(),
            },
        ],
    })
    .expect("sample catalog serializes")
}

#[cfg(all(test, feature = "server"))]
mod gen_tests {
    fn assert_ids_file(ids: super::CatalogIds, expected: &str) {
        match ids {
            super::CatalogIds::File(path) => assert_eq!(path, expected),
            super::CatalogIds::Inline(ids) => panic!("expected ids file, got inline ids: {ids:?}"),
        }
    }

    fn assert_inline_ids(ids: super::CatalogIds, expected: &[&str]) {
        let expected: Vec<String> = expected.iter().map(|id| id.to_string()).collect();
        match ids {
            super::CatalogIds::Inline(ids) => assert_eq!(ids, expected),
            super::CatalogIds::File(path) => panic!("expected inline ids, got ids file: {path}"),
        }
    }

    #[test]
    fn emitted_catalog_round_trips_through_loader() {
        let json = super::sample_catalog_json_for_test();
        assert!(
            crate::asset_capabilities::unsynced::build_unsynced_catalog_from_json(&json).is_ok()
        );
    }

    #[test]
    fn generated_native_row_uses_coingecko_defaults_and_placeholder_precision() {
        let detail = crate::integrations::coingecko::CoingeckoCoinDetail {
            id: "zcash".to_string(),
            symbol: "zec".to_string(),
            name: "Zcash".to_string(),
            web_slug: "zcash".to_string(),
            market_cap_rank: Some(14),
            detail_platforms: std::collections::HashMap::new(),
        };
        let rows = super::rows_from_detail("zcash", &detail);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].unit_code, "ZEC");
        assert_eq!(rows[0].network_id, "zcash-mainnet");
        assert_eq!(rows[0].network_name, "Zcash");
        assert_eq!(
            rows[0].decimal_precision,
            super::CatalogPrecisionOut::Placeholder
        );
    }

    #[test]
    fn fallback_row_uses_coingecko_symbol_web_slug_and_name() {
        let detail = crate::integrations::coingecko::CoingeckoCoinDetail {
            id: "dash".to_string(),
            symbol: "dash".to_string(),
            name: "Dash".to_string(),
            web_slug: "dash".to_string(),
            market_cap_rank: Some(103),
            detail_platforms: std::collections::HashMap::new(),
        };

        let rows = super::rows_from_detail("dash", &detail);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].unit_code, "DASH");
        assert_eq!(rows[0].network_id, "dash-mainnet");
        assert_eq!(rows[0].network_name, "Dash");
        assert_eq!(
            rows[0].decimal_precision,
            super::CatalogPrecisionOut::Placeholder
        );
    }

    #[test]
    fn save_catalog_writes_trailing_newline() {
        let path = std::env::temp_dir().join(format!(
            "bitgarth-gen-asset-catalog-out-{}.json",
            std::process::id()
        ));
        let catalog = super::UnsyncedCatalogFileOut {
            schema_version: 1,
            assets: Vec::new(),
        };

        super::save_catalog(path.to_str().expect("utf8 path"), &catalog).expect("save catalog");
        let raw = std::fs::read_to_string(&path).expect("read catalog");
        std::fs::remove_file(path).expect("remove catalog");

        assert!(raw.ends_with('\n'));
    }

    #[test]
    fn over_bound_coingecko_precision_serializes_as_placeholder() {
        let detail = crate::integrations::coingecko::CoingeckoCoinDetail {
            id: "future-token".to_string(),
            symbol: "future".to_string(),
            name: "Future Token".to_string(),
            web_slug: "future-token".to_string(),
            market_cap_rank: Some(999),
            detail_platforms: std::collections::HashMap::from([(
                "ethereum".to_string(),
                crate::integrations::coingecko::CoingeckoPlatformDetail {
                    contract_address: "0x123".to_string(),
                    decimal_place: Some(19),
                },
            )]),
        };
        let rows = super::rows_from_detail("future-token", &detail);
        let json = serde_json::to_string(&super::UnsyncedCatalogFileOut {
            schema_version: 1,
            assets: rows,
        })
        .expect("catalog serializes");

        assert!(json.contains(r#""decimal_precision":"REPLACEME""#));
        assert!(
            crate::asset_capabilities::unsynced::build_unsynced_catalog_from_json(&json).is_err()
        );
    }

    #[test]
    fn parse_args_out_flag() {
        let args = vec![
            "--ids-file".to_string(),
            "ids.txt".to_string(),
            "--out".to_string(),
            "custom.json".to_string(),
        ];
        let result = super::parse_args(args.into_iter()).expect("parse");
        assert_eq!(result.out, "custom.json");
        assert_ids_file(result.ids, "ids.txt");
    }

    #[test]
    fn parse_args_ids_file() {
        let args = vec!["--ids-file".to_string(), "ids.txt".to_string()];
        let result = super::parse_args(args.into_iter()).expect("parse");
        assert_ids_file(result.ids, "ids.txt");
    }

    #[test]
    fn parse_args_inline_ids() {
        let args = vec!["tether,solana".to_string()];
        let result = super::parse_args(args.into_iter()).expect("parse");
        assert_inline_ids(result.ids, &["tether", "solana"]);
    }

    #[test]
    fn parse_args_rejects_inline_ids_with_ids_file() {
        let args = vec![
            "--ids-file".to_string(),
            "ids.txt".to_string(),
            "tether,solana".to_string(),
        ];
        let result = super::parse_args(args.into_iter());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("only one id source"));
    }

    #[test]
    fn parse_args_rejects_unknown_flag() {
        let args = vec!["--bogus".to_string()];
        assert!(super::parse_args(args.into_iter()).is_err());
    }

    #[test]
    fn parse_args_help_shows_usage() {
        let args = vec!["--help".to_string()];
        let result = super::parse_args(args.into_iter());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("usage:"));
    }

    #[test]
    fn parse_args_requires_explicit_ids() {
        let result = super::parse_args(Vec::<String>::new().into_iter());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("[IDS | --ids-file PATH]"));
    }

    #[test]
    fn read_ids_file_ignores_blanks_and_comments() {
        let path = std::env::temp_dir().join(format!(
            "bitgarth-gen-asset-catalog-ids-{}.txt",
            std::process::id()
        ));
        std::fs::write(&path, "tether\n\n# comment\n solana \n").expect("write ids file");

        let ids = super::read_ids_file(path.to_str().expect("utf8 path")).expect("ids parse");
        std::fs::remove_file(path).expect("remove ids file");

        assert_eq!(ids, vec!["tether".to_string(), "solana".to_string()]);
    }

    #[test]
    fn placeholder_precision_row_serializes_replaceme() {
        let row = super::UnsyncedCatalogRowOut {
            asset_id: "future-coin".to_string(),
            market_cap_rank: 42,
            canonical_name: "Future Coin".to_string(),
            unit_code: "FUT".to_string(),
            symbol: None,
            network_id: "future-mainnet".to_string(),
            network_name: "Future".to_string(),
            decimal_precision: super::CatalogPrecisionOut::Placeholder,
            updated_at: "2026-06-01T00:00:00Z".to_string(),
            coingecko_id: "future-coin".to_string(),
        };

        let json = serde_json::to_string(&row).expect("row serializes");

        assert!(json.contains("\"decimal_precision\":\"REPLACEME\""));
    }
}
