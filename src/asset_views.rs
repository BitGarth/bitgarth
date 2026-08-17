use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatalogAssetKeyError(String);

impl std::fmt::Display for CatalogAssetKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CatalogAssetKeyError {}

fn validate_catalog_asset_key(value: &str) -> Result<(), CatalogAssetKeyError> {
    const MAX_LEN: usize = 96;
    if value.is_empty() {
        return Err(CatalogAssetKeyError("empty catalog asset key".to_string()));
    }
    if value.len() > MAX_LEN {
        return Err(CatalogAssetKeyError(format!(
            "catalog asset key is longer than {MAX_LEN} bytes"
        )));
    }
    if value
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        Ok(())
    } else {
        Err(CatalogAssetKeyError(
            "catalog asset key must contain only lowercase ASCII letters, digits, or '-'"
                .to_string(),
        ))
    }
}

/// Client-safe stable string identity for a catalog asset, e.g. "bitcoin".
///
/// Boundary key, not the server catalog model. The server must still
/// validate catalog membership before accepting user-submitted keys.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct CatalogAssetKey(String);

impl CatalogAssetKey {
    #[cfg(feature = "server")]
    pub(crate) fn from_trusted(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn try_new(value: impl Into<String>) -> Result<Self, CatalogAssetKeyError> {
        let value = value.into();
        validate_catalog_asset_key(&value)?;
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for CatalogAssetKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::try_new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ManualAssetInstanceIdView {
    pub(crate) asset_id: String,
    pub(crate) network_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_asset_key_try_new_accepts_slug() {
        let key = CatalogAssetKey::try_new("usd-coin").expect("valid key");
        assert_eq!(key.as_str(), "usd-coin");
    }

    #[test]
    fn catalog_asset_key_try_new_rejects_invalid_values() {
        assert!(CatalogAssetKey::try_new("").is_err());
        assert!(CatalogAssetKey::try_new("USD").is_err());
        assert!(CatalogAssetKey::try_new("usd coin").is_err());
        assert!(CatalogAssetKey::try_new("usd_coin").is_err());
    }

    #[test]
    fn catalog_asset_key_wire_format_is_stable_string_id() {
        let key = CatalogAssetKey::try_new("bitcoin").expect("valid key");
        let json = serde_json::to_string(&key).expect("serialize");
        assert_eq!(json, r#""bitcoin""#);
        let back: CatalogAssetKey = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, key);
    }

    #[test]
    fn catalog_asset_key_deserialize_rejects_invalid_values() {
        let err =
            serde_json::from_str::<CatalogAssetKey>(r#""USD Coin""#).expect_err("invalid key");
        assert!(err.to_string().contains("catalog asset key"));
    }

    #[test]
    fn manual_asset_instance_id_view_serializes_asset_and_network_only() {
        let view = ManualAssetInstanceIdView {
            asset_id: "usd-coin".to_string(),
            network_id: "algorand-mainnet".to_string(),
        };
        let json = serde_json::to_string(&view).expect("serialize");
        assert_eq!(
            json,
            r#"{"asset_id":"usd-coin","network_id":"algorand-mainnet"}"#
        );
        let back: ManualAssetInstanceIdView = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, view);
    }
}
