//! Simple API key CRUD operations.

use super::error::DbError;
use super::user_db::{with_user_db, with_user_db_mut};
use crate::models::{ApiKeyProvider, SimpleApiKey, UserId};
use chrono::Utc;

pub(crate) fn load_api_key(
    user_id: UserId,
    provider: ApiKeyProvider,
) -> Result<Option<SimpleApiKey>, DbError> {
    let storage_key = provider.as_storage_key();
    debug_assert_eq!(
        ApiKeyProvider::from_storage_key(storage_key),
        Some(provider)
    );

    with_user_db(user_id, |conn| {
        let result = conn.query_row(
            "SELECT api_key FROM api_keys WHERE provider = ?1",
            [storage_key],
            |row| {
                let value: String = row.get(0)?;
                Ok(value)
            },
        );

        match result {
            Ok(value) => Ok(SimpleApiKey::from_non_empty_storage(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(DbError::new(format!("Failed to load API key: {error}"))),
        }
    })
}

pub(crate) fn has_api_key(user_id: UserId, provider: ApiKeyProvider) -> Result<bool, DbError> {
    Ok(load_api_key(user_id, provider)?.is_some())
}

pub(crate) fn list_all_api_keys(
    user_id: UserId,
) -> Result<Vec<(ApiKeyProvider, SimpleApiKey)>, DbError> {
    with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare("SELECT provider, api_key FROM api_keys ORDER BY provider ASC")
            .map_err(|error| DbError::new(format!("Failed to prepare API key list: {error}")))?;
        let rows = stmt
            .query_map([], |row| {
                let provider_raw: String = row.get(0)?;
                let api_key_raw: String = row.get(1)?;
                Ok((provider_raw, api_key_raw))
            })
            .map_err(|error| DbError::new(format!("Failed to list API keys: {error}")))?;

        let mut result = Vec::new();
        for row in rows {
            let (provider_raw, api_key_raw) =
                row.map_err(|error| DbError::new(format!("Failed to map API key row: {error}")))?;
            let Some(provider) = ApiKeyProvider::from_storage_key(&provider_raw) else {
                tracing::debug!(provider = %provider_raw, "api keys: skipping unknown provider row");
                continue;
            };
            let Some(api_key) = SimpleApiKey::from_non_empty_storage(api_key_raw) else {
                return Err(DbError::new(format!(
                    "Stored API key for provider {provider_raw} is blank or invalid"
                )));
            };
            result.push((provider, api_key));
        }

        Ok(result)
    })
}

pub(crate) fn save_api_key(
    user_id: UserId,
    provider: ApiKeyProvider,
    api_key: &SimpleApiKey,
) -> Result<(), DbError> {
    let now = Utc::now().to_rfc3339();
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "INSERT INTO api_keys (provider, api_key, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(provider) DO UPDATE SET
                api_key = excluded.api_key,
                updated_at = excluded.updated_at",
            rusqlite::params![provider.as_storage_key(), api_key.as_str(), now],
        )
        .map_err(|error| DbError::new(format!("Failed to save API key: {error}")))?;
        Ok(())
    })
}

pub(crate) fn clear_api_key(user_id: UserId, provider: ApiKeyProvider) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "DELETE FROM api_keys WHERE provider = ?1",
            [provider.as_storage_key()],
        )
        .map_err(|error| DbError::new(format!("Failed to clear API key: {error}")))?;
        Ok(())
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::{setup_test_user, unique_user_id};

    #[test]
    fn provider_identity_upsert_and_clear_are_scoped() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let etherscan_first =
            SimpleApiKey::new("etherscan-first".to_string()).expect("key should be valid");
        let etherscan_second =
            SimpleApiKey::new("etherscan-second".to_string()).expect("key should be valid");
        let coingecko =
            SimpleApiKey::new("coingecko-key".to_string()).expect("key should be valid");

        save_api_key(user_id, ApiKeyProvider::Etherscan, &etherscan_first)
            .expect("save etherscan key");
        save_api_key(user_id, ApiKeyProvider::CoinGecko, &coingecko).expect("save coingecko key");
        save_api_key(user_id, ApiKeyProvider::Etherscan, &etherscan_second)
            .expect("update etherscan key");

        let etherscan_loaded = load_api_key(user_id, ApiKeyProvider::Etherscan)
            .expect("load updated key")
            .expect("key should exist");
        assert_eq!(etherscan_loaded.as_str(), "etherscan-second");

        let coingecko_loaded = load_api_key(user_id, ApiKeyProvider::CoinGecko)
            .expect("load coingecko key")
            .expect("coingecko key should exist");
        assert_eq!(coingecko_loaded.as_str(), "coingecko-key");

        clear_api_key(user_id, ApiKeyProvider::Etherscan).expect("clear key");
        assert!(
            load_api_key(user_id, ApiKeyProvider::Etherscan)
                .expect("load after clear")
                .is_none()
        );
        assert!(has_api_key(user_id, ApiKeyProvider::CoinGecko).expect("has coingecko key"));
    }

    #[test]
    fn list_all_api_keys_returns_rows_ordered_by_provider() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let etherscan =
            SimpleApiKey::new("etherscan-key".to_string()).expect("etherscan key should be valid");
        let coingecko =
            SimpleApiKey::new("coingecko-key".to_string()).expect("coingecko key should be valid");

        save_api_key(user_id, ApiKeyProvider::Etherscan, &etherscan).expect("save etherscan key");
        save_api_key(user_id, ApiKeyProvider::CoinGecko, &coingecko).expect("save coingecko key");

        let rows = list_all_api_keys(user_id).expect("list api keys");

        assert_eq!(
            rows,
            vec![
                (ApiKeyProvider::CoinGecko, coingecko),
                (ApiKeyProvider::Etherscan, etherscan),
            ]
        );
    }

    #[test]
    fn list_all_api_keys_returns_empty_for_user_with_no_keys() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let rows = list_all_api_keys(user_id).expect("list api keys");

        assert!(rows.is_empty());
    }

    #[test]
    fn list_all_api_keys_errors_for_known_provider_with_blank_key() {
        let user_id = unique_user_id();
        setup_test_user(user_id);

        let now = Utc::now().to_rfc3339();
        with_user_db_mut(user_id, |conn| {
            conn.execute(
                "INSERT INTO api_keys (provider, api_key, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?3)",
                rusqlite::params![ApiKeyProvider::Etherscan.as_storage_key(), "", now],
            )
            .expect("insert blank api key");
            Ok::<(), DbError>(())
        })
        .expect("write test row");

        let error = list_all_api_keys(user_id).expect_err("blank known key should error");

        assert!(error.to_string().contains("etherscan"));
    }
}
