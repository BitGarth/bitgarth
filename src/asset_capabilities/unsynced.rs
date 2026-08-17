use super::UnitCode;
use crate::wallets::ValidatedManualAssetUnitCode;
use crate::wallets::labels::ManualAssetDisplayScale;
use chrono::{DateTime, Utc};
use once_cell::sync::OnceCell;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

const DEPLOYED_CATALOG_PATH: &str = "/srv/assets/catalog/unsynced_asset_catalog.json";
const LOCAL_CATALOG_PATH: &str = "assets/catalog/unsynced_asset_catalog.json";
const CATALOG_PATH_ENV: &str = "BITGARTH_UNSYNCED_ASSET_CATALOG_PATH";
const PLACEHOLDERS: &[&str] = &["REPLACEME", "TODO"];

static UNSYNCED_CATALOG: OnceCell<UnsyncedAssetCatalog> = OnceCell::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnsyncedCatalogError(String);

impl UnsyncedCatalogError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for UnsyncedCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsynced asset catalog error: {}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct UnsyncedAssetId(Cow<'static, str>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct UnsyncedNetworkId(Cow<'static, str>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AssetDisplaySymbol(char);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoingeckoAssetId(Cow<'static, str>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct UnsyncedAssetInstanceId {
    pub(crate) asset_id: UnsyncedAssetId,
    pub(crate) network_id: UnsyncedNetworkId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnsyncedAssetInstance {
    pub(crate) id: UnsyncedAssetInstanceId,
    pub(crate) market_cap_rank: u32,
    pub(crate) canonical_name: String,
    pub(crate) unit_code: UnitCode,
    pub(crate) symbol: Option<AssetDisplaySymbol>,
    pub(crate) network_name: String,
    pub(crate) decimal_precision: ManualAssetDisplayScale,
    pub(crate) updated_at: DateTime<Utc>,
    pub(crate) coingecko_id: CoingeckoAssetId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnsyncedAssetCatalog {
    instances: HashMap<UnsyncedAssetInstanceId, UnsyncedAssetInstance>,
    pub(crate) manual_order: Vec<UnsyncedAssetInstanceId>,
}

impl UnsyncedAssetCatalog {
    pub(crate) fn instance(&self, id: &UnsyncedAssetInstanceId) -> Option<&UnsyncedAssetInstance> {
        self.instances.get(id)
    }

    pub(crate) fn coingecko_id_for_asset_id(&self, asset_id: &str) -> Option<&str> {
        self.manual_order
            .iter()
            .filter(|id| id.asset_id.as_str() == asset_id)
            .filter_map(|id| self.instance(id))
            .map(|instance| instance.coingecko_id.as_str())
            .next()
    }

    pub(crate) fn search(&self, query: &str) -> Vec<&UnsyncedAssetInstance> {
        let query = query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return Vec::new();
        }

        let mut matches = self
            .manual_order
            .iter()
            .filter_map(|id| self.instance(id))
            .filter_map(|instance| {
                let unit_code = instance.unit_code.as_str().to_ascii_lowercase();
                let canonical_name = instance.canonical_name.to_ascii_lowercase();
                let asset_id = instance.id.asset_id.as_str();
                let network_name = instance.network_name.to_ascii_lowercase();
                let network_id = instance.id.network_id.as_str();

                let quality = if unit_code == query || asset_id == query {
                    Some(0_u8)
                } else if unit_code.starts_with(&query)
                    || canonical_name.starts_with(&query)
                    || asset_id.starts_with(&query)
                {
                    Some(1_u8)
                } else if canonical_name.contains(&query)
                    || network_name.contains(&query)
                    || asset_id.contains(&query)
                    || network_id.contains(&query)
                {
                    Some(2_u8)
                } else {
                    None
                };

                quality.map(|quality| (quality, instance))
            })
            .collect::<Vec<_>>();

        matches.sort_by_key(|(quality, instance)| (*quality, instance.market_cap_rank));
        matches.truncate(25);
        matches
            .into_iter()
            .map(|(_quality, instance)| instance)
            .collect()
    }
}

pub(crate) fn resolve_catalog_path() -> PathBuf {
    let override_path = std::env::var(CATALOG_PATH_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty());
    resolve_catalog_path_for(override_path, Path::new(DEPLOYED_CATALOG_PATH).exists())
}

fn resolve_catalog_path_for(override_path: Option<String>, deployed_exists: bool) -> PathBuf {
    if let Some(path) = override_path {
        return PathBuf::from(path);
    }
    if deployed_exists {
        return PathBuf::from(DEPLOYED_CATALOG_PATH);
    }
    PathBuf::from(LOCAL_CATALOG_PATH)
}

pub(crate) fn load_unsynced_catalog_from_path(
    path: &Path,
) -> Result<UnsyncedAssetCatalog, UnsyncedCatalogError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| UnsyncedCatalogError::new(format!("read {}: {error}", path.display())))?;
    let catalog = build_unsynced_catalog_from_json(&raw)?;
    tracing::info!(
        "Loaded {} un-synced assets from {}",
        catalog.manual_order.len(),
        path.display()
    );
    Ok(catalog)
}

pub(crate) fn load_catalog() -> Result<&'static UnsyncedAssetCatalog, UnsyncedCatalogError> {
    let catalog = UNSYNCED_CATALOG.get_or_try_init(|| {
        let path = resolve_catalog_path();
        load_unsynced_catalog_from_path(&path)
    })?;
    let _ = catalog.search("");
    Ok(catalog)
}

#[derive(serde::Deserialize)]
struct CatalogFile {
    schema_version: u32,
    assets: Vec<CatalogAsset>,
}

#[derive(serde::Deserialize)]
struct CatalogAsset {
    asset_id: String,
    market_cap_rank: u32,
    canonical_name: String,
    unit_code: String,
    #[serde(default)]
    symbol: Option<String>,
    network_id: String,
    network_name: String,
    decimal_precision: serde_json::Value,
    updated_at: String,
    coingecko_id: String,
}

pub(crate) fn build_unsynced_catalog_from_json(
    json: &str,
) -> Result<UnsyncedAssetCatalog, UnsyncedCatalogError> {
    let file: CatalogFile = serde_json::from_str(json)
        .map_err(|error| UnsyncedCatalogError::new(format!("parse error: {error}")))?;
    if file.schema_version != 1 {
        return Err(UnsyncedCatalogError::new(format!(
            "unsupported schema_version: {}",
            file.schema_version
        )));
    }
    if file.assets.is_empty() {
        return Err(UnsyncedCatalogError::new("catalog cannot be empty"));
    }

    let mut instances = HashMap::new();
    let mut manual_order = Vec::new();

    for asset in file.assets {
        let instance = convert_asset(asset)?;
        let id = instance.id.clone();
        if instances.insert(id.clone(), instance).is_some() {
            return Err(UnsyncedCatalogError::new(format!(
                "duplicate asset/network identity: {}/{}",
                id.asset_id.as_str(),
                id.network_id.as_str()
            )));
        }
        manual_order.push(id);
    }

    manual_order.sort_by(|left, right| {
        let left_rank = instances
            .get(left)
            .map(|instance| instance.market_cap_rank)
            .unwrap_or(u32::MAX);
        let right_rank = instances
            .get(right)
            .map(|instance| instance.market_cap_rank)
            .unwrap_or(u32::MAX);
        left_rank
            .cmp(&right_rank)
            .then_with(|| left.asset_id.as_str().cmp(right.asset_id.as_str()))
            .then_with(|| left.network_id.as_str().cmp(right.network_id.as_str()))
    });

    Ok(UnsyncedAssetCatalog {
        instances,
        manual_order,
    })
}

fn convert_asset(asset: CatalogAsset) -> Result<UnsyncedAssetInstance, UnsyncedCatalogError> {
    let asset_id = UnsyncedAssetId::parse(&asset.asset_id)?;
    validate_required_string("canonical_name", &asset.canonical_name)?;
    validate_required_string("unit_code", &asset.unit_code)?;
    let unit_code = ValidatedManualAssetUnitCode::parse(&asset.unit_code)
        .map_err(|error| UnsyncedCatalogError::new(format!("{error}")))?;
    validate_required_string("network_name", &asset.network_name)?;
    validate_required_string("updated_at", &asset.updated_at)?;
    let coingecko_id = CoingeckoAssetId::parse(&asset.coingecko_id)?;
    let symbol = match asset.symbol {
        Some(value) => Some(AssetDisplaySymbol::parse(&value)?),
        None => None,
    };
    let network_id = UnsyncedNetworkId::parse(&asset.network_id)?;
    let decimal_precision = parse_decimal_precision(asset.decimal_precision)?;
    let updated_at = DateTime::parse_from_rfc3339(&asset.updated_at)
        .map_err(|error| UnsyncedCatalogError::new(format!("invalid updated_at: {error}")))?
        .with_timezone(&Utc);

    Ok(UnsyncedAssetInstance {
        id: UnsyncedAssetInstanceId {
            asset_id,
            network_id,
        },
        market_cap_rank: asset.market_cap_rank,
        canonical_name: asset.canonical_name,
        unit_code: UnitCode::owned(unit_code.as_str().to_string()),
        symbol,
        network_name: asset.network_name,
        decimal_precision,
        updated_at,
        coingecko_id,
    })
}

impl UnsyncedAssetId {
    pub(crate) fn parse(value: &str) -> Result<Self, UnsyncedCatalogError> {
        validate_slug("asset_id", value)?;
        Ok(Self(Cow::Owned(value.to_string())))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl UnsyncedNetworkId {
    pub(crate) fn parse(value: &str) -> Result<Self, UnsyncedCatalogError> {
        validate_slug("network_id", value)?;
        Ok(Self(Cow::Owned(value.to_string())))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl AssetDisplaySymbol {
    fn parse(value: &str) -> Result<Self, UnsyncedCatalogError> {
        validate_required_string("symbol", value)?;
        let mut characters = value.chars();
        let Some(symbol) = characters.next() else {
            return Err(UnsyncedCatalogError::new("symbol cannot be empty"));
        };
        if characters.next().is_some() {
            return Err(UnsyncedCatalogError::new(
                "symbol must contain exactly one Unicode scalar",
            ));
        }
        Ok(Self(symbol))
    }

    pub(crate) const fn as_char(self) -> char {
        self.0
    }
}

impl fmt::Display for AssetDisplaySymbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_char())
    }
}

impl CoingeckoAssetId {
    pub(crate) fn parse(value: &str) -> Result<Self, UnsyncedCatalogError> {
        validate_slug("coingecko_id", value)?;
        Ok(Self(Cow::Owned(value.to_string())))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CoingeckoAssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn parse_decimal_precision(
    value: serde_json::Value,
) -> Result<ManualAssetDisplayScale, UnsyncedCatalogError> {
    match value {
        serde_json::Value::Number(number) => {
            let value = number
                .as_i64()
                .ok_or_else(|| UnsyncedCatalogError::new("decimal_precision must be an integer"))?;
            ManualAssetDisplayScale::manual_decimal_precision(value)
                .map_err(|error| UnsyncedCatalogError::new(format!("{error}")))
        }
        serde_json::Value::String(value) if is_placeholder(&value) => Err(
            UnsyncedCatalogError::new("decimal_precision cannot be a placeholder"),
        ),
        serde_json::Value::String(_) => Err(UnsyncedCatalogError::new(
            "decimal_precision must be an integer",
        )),
        _ => Err(UnsyncedCatalogError::new(
            "decimal_precision must be an integer",
        )),
    }
}

fn validate_slug(field: &str, value: &str) -> Result<(), UnsyncedCatalogError> {
    validate_required_string(field, value)?;
    if value.len() > 64 {
        return Err(UnsyncedCatalogError::new(format!(
            "{field} longer than 64 chars"
        )));
    }
    let valid = value.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    });
    if !valid {
        return Err(UnsyncedCatalogError::new(format!(
            "invalid {field} slug: {value}"
        )));
    }
    Ok(())
}

fn validate_required_string(field: &str, value: &str) -> Result<(), UnsyncedCatalogError> {
    if value.is_empty() {
        return Err(UnsyncedCatalogError::new(format!(
            "{field} cannot be empty"
        )));
    }
    if is_placeholder(value) {
        return Err(UnsyncedCatalogError::new(format!(
            "{field} cannot be a placeholder"
        )));
    }
    Ok(())
}

fn is_placeholder(value: &str) -> bool {
    PLACEHOLDERS.contains(&value)
}

#[cfg(test)]
fn resolve_catalog_path_for_test(override_path: Option<String>, deployed_exists: bool) -> PathBuf {
    resolve_catalog_path_for(override_path, deployed_exists)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_CATALOG: &str = r#"{
        "schema_version": 1,
        "assets": [{
            "asset_id": "solana",
            "market_cap_rank": 5,
            "canonical_name": "Solana",
            "unit_code": "SOL",
            "coingecko_id": "solana",
            "network_id": "solana-mainnet",
            "network_name": "Solana",
            "decimal_precision": 9,
            "updated_at": "2026-05-31T10:30:00Z"
        }]
    }"#;

    fn catalog_with_symbol(symbol_json: &str) -> String {
        MINIMAL_CATALOG.replace(
            "            \"coingecko_id\": \"solana\",",
            &format!(
                "            \"symbol\": {symbol_json},\n            \"coingecko_id\": \"solana\","
            ),
        )
    }

    fn solana_id() -> UnsyncedAssetInstanceId {
        UnsyncedAssetInstanceId {
            asset_id: UnsyncedAssetId::parse("solana").expect("valid asset id"),
            network_id: UnsyncedNetworkId::parse("solana-mainnet").expect("valid network id"),
        }
    }

    #[test]
    fn parses_valid_minimal_catalog() {
        let catalog =
            build_unsynced_catalog_from_json(MINIMAL_CATALOG).expect("catalog should parse");

        let instance = catalog.instance(&solana_id()).expect("instance exists");
        assert_eq!(instance.id, solana_id());
        assert_eq!(instance.market_cap_rank, 5);
        assert_eq!(instance.canonical_name, "Solana");
        assert_eq!(instance.unit_code.as_str(), "SOL");
        assert_eq!(instance.symbol, None);
        assert_eq!(instance.network_name, "Solana");
        assert_eq!(instance.decimal_precision.as_u8(), 9);
        assert_eq!(
            instance.updated_at.to_rfc3339(),
            "2026-05-31T10:30:00+00:00"
        );
        assert_eq!(instance.coingecko_id.as_str(), "solana");
        assert_eq!(catalog.manual_order, vec![solana_id()]);
    }

    #[test]
    fn rejects_duplicate_asset_network_identity() {
        let duplicate = MINIMAL_CATALOG.replace(
            r#"        }]"#,
            r#"        }, {
            "asset_id": "solana",
            "market_cap_rank": 5,
            "canonical_name": "Solana",
            "unit_code": "SOL",
            "coingecko_id": "solana",
            "network_id": "solana-mainnet",
            "network_name": "Solana",
            "decimal_precision": 9,
            "updated_at": "2026-05-31T10:30:00Z"
        }]"#,
        );

        assert!(build_unsynced_catalog_from_json(&duplicate).is_err());
    }

    #[test]
    fn uses_exact_flat_json_shape() {
        let flat = r#"{
            "schema_version": 1,
            "assets": [{
                "asset_id": "cardano",
                "market_cap_rank": 11,
                "canonical_name": "Cardano",
                "unit_code": "ADA",
                "symbol": "₳",
                "coingecko_id": "cardano",
                "network_id": "future-mainnet",
                "network_name": "Future Mainnet",
                "decimal_precision": 6,
                "updated_at": "2026-05-31T10:30:00Z"
            }]
        }"#;
        let id = UnsyncedAssetInstanceId {
            asset_id: UnsyncedAssetId::parse("cardano").expect("valid asset id"),
            network_id: UnsyncedNetworkId::parse("future-mainnet").expect("valid network id"),
        };

        let catalog = build_unsynced_catalog_from_json(flat).expect("flat catalog parses");
        let instance = catalog.instance(&id).expect("instance exists");
        assert_eq!(instance.market_cap_rank, 11);
        assert_eq!(instance.symbol.map(AssetDisplaySymbol::as_char), Some('₳'));
        assert_eq!(instance.coingecko_id.as_str(), "cardano");
    }

    #[test]
    fn rejects_unsupported_schema_version() {
        let bad = MINIMAL_CATALOG.replace("\"schema_version\": 1", "\"schema_version\": 2");
        assert!(build_unsynced_catalog_from_json(&bad).is_err());
    }

    #[test]
    fn rejects_missing_required_market_cap_rank() {
        let missing = MINIMAL_CATALOG.replace("            \"market_cap_rank\": 5,\n", "");
        assert!(build_unsynced_catalog_from_json(&missing).is_err());
    }

    #[test]
    fn rejects_nested_network_shape() {
        let nested = r#"{
            "schema_version": 1,
            "assets": [{
                "asset_id": "solana",
                "market_cap_rank": 5,
                "canonical_name": "Solana",
                "unit_code": "SOL",
                "coingecko_id": "solana",
                "networks": [{
                "network_id": "solana-mainnet",
                "network_name": "Solana",
                "decimal_precision": 9,
                "updated_at": "2026-05-31T10:30:00Z"
                }]
            }]
        }"#;
        assert!(build_unsynced_catalog_from_json(nested).is_err());
    }

    #[test]
    fn rejects_placeholder_values() {
        for placeholder in ["REPLACEME", "TODO"] {
            for bad in [
                MINIMAL_CATALOG.replace(
                    "\"asset_id\": \"solana\"",
                    &format!("\"asset_id\": \"{placeholder}\""),
                ),
                MINIMAL_CATALOG.replace(
                    "\"canonical_name\": \"Solana\"",
                    &format!("\"canonical_name\": \"{placeholder}\""),
                ),
                MINIMAL_CATALOG.replace(
                    "\"unit_code\": \"SOL\"",
                    &format!("\"unit_code\": \"{placeholder}\""),
                ),
                MINIMAL_CATALOG.replace(
                    "\"coingecko_id\": \"solana\"",
                    &format!("\"coingecko_id\": \"{placeholder}\""),
                ),
                catalog_with_symbol(&format!("\"{placeholder}\"")),
                MINIMAL_CATALOG.replace(
                    "\"network_id\": \"solana-mainnet\"",
                    &format!("\"network_id\": \"{placeholder}\""),
                ),
                MINIMAL_CATALOG.replace(
                    "\"network_name\": \"Solana\"",
                    &format!("\"network_name\": \"{placeholder}\""),
                ),
                MINIMAL_CATALOG.replace(
                    "\"decimal_precision\": 9",
                    &format!("\"decimal_precision\": \"{placeholder}\""),
                ),
            ] {
                assert!(
                    build_unsynced_catalog_from_json(&bad).is_err(),
                    "placeholder should be rejected in {bad}"
                );
            }
        }
    }

    #[test]
    fn validates_symbol_as_single_unicode_scalar() {
        for bad_symbol in ["", "SOL"] {
            let bad = catalog_with_symbol(&format!("\"{bad_symbol}\""));
            assert!(
                build_unsynced_catalog_from_json(&bad).is_err(),
                "bad symbol should fail: {bad_symbol:?}"
            );
        }

        let valid_symbol = catalog_with_symbol("\"◎\"");
        let catalog =
            build_unsynced_catalog_from_json(&valid_symbol).expect("single scalar accepted");
        assert_eq!(
            catalog
                .instance(&solana_id())
                .expect("instance")
                .symbol
                .map(AssetDisplaySymbol::as_char),
            Some('◎')
        );
    }

    #[test]
    fn accepts_missing_or_null_symbol_as_none() {
        let catalog =
            build_unsynced_catalog_from_json(MINIMAL_CATALOG).expect("missing symbol accepted");
        assert_eq!(
            catalog.instance(&solana_id()).expect("instance").symbol,
            None
        );

        let null = catalog_with_symbol("null");
        let catalog = build_unsynced_catalog_from_json(&null).expect("null symbol accepted");
        assert_eq!(
            catalog.instance(&solana_id()).expect("instance").symbol,
            None
        );
    }

    #[test]
    fn rejects_missing_required_coingecko_id() {
        let missing = MINIMAL_CATALOG.replace("            \"coingecko_id\": \"solana\",\n", "");
        assert!(build_unsynced_catalog_from_json(&missing).is_err());
    }

    #[test]
    fn rejects_decimal_bounds() {
        for bad_precision in [-1, 19, 256] {
            let bad = MINIMAL_CATALOG.replace(
                "\"decimal_precision\": 9",
                &format!("\"decimal_precision\": {bad_precision}"),
            );
            assert!(
                build_unsynced_catalog_from_json(&bad).is_err(),
                "bad precision should fail: {bad_precision}"
            );
        }
    }

    #[test]
    fn rejects_unit_codes_that_manual_accounts_cannot_use() {
        let bad = MINIMAL_CATALOG.replace("\"unit_code\": \"SOL\"", "\"unit_code\": \"bad-unit\"");
        assert!(build_unsynced_catalog_from_json(&bad).is_err());
    }

    #[test]
    fn rejects_invalid_timestamp_parsing() {
        let bad = MINIMAL_CATALOG.replace("2026-05-31T10:30:00Z", "not-a-timestamp");
        assert!(build_unsynced_catalog_from_json(&bad).is_err());
    }

    #[test]
    fn rejects_empty_catalog() {
        assert!(build_unsynced_catalog_from_json(r#"{ "assets": [] }"#).is_err());
    }

    #[test]
    fn accepts_network_id_not_defined_in_rust() {
        let json = MINIMAL_CATALOG.replace("solana-mainnet", "future-mainnet");
        let catalog = build_unsynced_catalog_from_json(&json).expect("unknown network accepted");
        let id = UnsyncedAssetInstanceId {
            asset_id: UnsyncedAssetId::parse("solana").expect("valid asset id"),
            network_id: UnsyncedNetworkId::parse("future-mainnet").expect("valid network id"),
        };

        assert!(catalog.instance(&id).is_some());
    }

    #[test]
    fn repository_unsynced_catalog_loads() {
        let raw = std::fs::read_to_string("assets/catalog/unsynced_asset_catalog.json")
            .expect("catalog file should exist");
        let catalog = build_unsynced_catalog_from_json(&raw).expect("catalog should parse");
        assert!(
            catalog
                .search("ADA")
                .iter()
                .any(|row| row.unit_code.as_str() == "ADA")
        );
        assert!(
            catalog
                .search("USDC")
                .iter()
                .any(|row| row.id.network_id.as_str() == "ethereum-mainnet")
        );
        assert!(
            catalog
                .search("USDC")
                .iter()
                .any(|row| row.id.network_id.as_str() == "polygon-mainnet")
        );
        assert!(
            catalog
                .search("USDC")
                .iter()
                .all(|row| row.id.network_id.as_str() != "algorand-mainnet")
        );
    }

    #[test]
    fn catalog_path_prefers_override_then_srv_then_local() {
        assert_eq!(
            resolve_catalog_path_for_test(Some("/tmp/custom-catalog.json".to_string()), true),
            std::path::PathBuf::from("/tmp/custom-catalog.json")
        );
        assert_eq!(
            resolve_catalog_path_for_test(None, true),
            std::path::PathBuf::from("/srv/assets/catalog/unsynced_asset_catalog.json")
        );
        assert_eq!(
            resolve_catalog_path_for_test(None, false),
            std::path::PathBuf::from("assets/catalog/unsynced_asset_catalog.json")
        );
    }

    #[test]
    fn search_matches_unit_name_asset_id_and_network() {
        let catalog =
            build_unsynced_catalog_from_json(MINIMAL_CATALOG).expect("catalog should parse");

        assert_eq!(catalog.search("SOL").len(), 1);
        assert_eq!(catalog.search("Solana").len(), 1);
        assert_eq!(catalog.search("solana").len(), 1);
        assert_eq!(catalog.search("mainnet").len(), 1);
    }

    #[test]
    fn search_ranks_exact_unit_before_network_substring() {
        let json = r#"{
            "schema_version": 1,
            "assets": [
                {
                    "asset_id": "network-match",
                    "market_cap_rank": 1,
                    "canonical_name": "Network Match",
                    "unit_code": "NET",
                    "symbol": null,
                    "network_id": "ada-network-mainnet",
                    "network_name": "ADA Network",
                    "decimal_precision": 6,
                    "updated_at": "2026-05-30T00:00:00Z",
                    "coingecko_id": "network-match"
                },
                {
                    "asset_id": "cardano",
                    "market_cap_rank": 9,
                    "canonical_name": "Cardano",
                    "unit_code": "ADA",
                    "symbol": "₳",
                    "network_id": "cardano-mainnet",
                    "network_name": "Cardano",
                    "decimal_precision": 6,
                    "updated_at": "2026-05-30T00:00:00Z",
                    "coingecko_id": "cardano"
                }
            ]
        }"#;
        let catalog = build_unsynced_catalog_from_json(json).expect("catalog should parse");
        let rows = catalog.search("ada");

        assert_eq!(rows[0].unit_code.as_str(), "ADA");
    }

    #[test]
    fn search_preserves_market_cap_order_within_equal_quality_and_truncates() {
        let assets = (1..=30)
            .map(|rank| {
                format!(
                    r#"{{
                        "asset_id": "asset-{rank}",
                        "market_cap_rank": {rank},
                        "canonical_name": "Ranked Asset {rank}",
                        "unit_code": "RANK{rank}",
                        "symbol": null,
                        "network_id": "ranked-mainnet",
                        "network_name": "Ranked",
                        "decimal_precision": 6,
                        "updated_at": "2026-05-30T00:00:00Z",
                        "coingecko_id": "asset-{rank}"
                    }}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(r#"{{ "schema_version": 1, "assets": [{assets}] }}"#);
        let catalog = build_unsynced_catalog_from_json(&json).expect("catalog should parse");
        let rows = catalog.search("ranked");

        assert_eq!(rows.len(), 25);
        assert_eq!(rows[0].market_cap_rank, 1);
        assert_eq!(rows[24].market_cap_rank, 25);
    }
}
