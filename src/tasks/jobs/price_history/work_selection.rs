use chrono::NaiveDate;

#[cfg(feature = "server")]
use crate::models::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PriceHistoryAssetWork {
    pub(crate) asset_id: String,
    pub(crate) provider_asset_id: String,
    pub(crate) first_owned_date: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PriceHistoryWorkCandidate {
    asset_id: String,
    provider_asset_id: Option<String>,
    created_at_date: NaiveDate,
    first_evidence_date: Option<NaiveDate>,
}

#[cfg(feature = "server")]
pub(crate) fn load_user_price_history_work(
    user_id: UserId,
) -> Result<Vec<PriceHistoryAssetWork>, crate::db::DbError> {
    let bundle = crate::db::load_wallet_summary_bundle(user_id)?;
    let first_owned_dates = load_price_history_first_owned_dates(user_id)?;
    let mut candidates = Vec::new();

    for wallet in bundle.wallets {
        for account in wallet.accounts {
            let asset_id = crate::asset_capabilities::asset_id_for_synced_asset(account.asset_id);
            let account_id = account.id.to_string();
            candidates.push(PriceHistoryWorkCandidate {
                asset_id: asset_id.as_str().to_string(),
                provider_asset_id: crate::asset_capabilities::asset(&asset_id)
                    .and_then(|asset| asset.price_refs.coingecko.as_deref())
                    .map(str::to_string),
                created_at_date: account.created_at.date_naive(),
                first_evidence_date: first_owned_dates
                    .native_by_account_id
                    .get(&account_id)
                    .copied(),
            });
        }
    }

    for row in bundle.manual_asset_accounts {
        let account_id = row.account_id.to_string();
        candidates.push(PriceHistoryWorkCandidate {
            asset_id: row.asset_id.as_str().to_string(),
            provider_asset_id: crate::asset_capabilities::resolve_manual_coingecko_id(
                row.asset_id.as_str(),
                Some(row.coingecko_id.as_str()),
            ),
            created_at_date: row.created_at.date_naive(),
            first_evidence_date: first_owned_dates
                .manual_by_account_id
                .get(&account_id)
                .copied(),
        });
    }

    Ok(select_price_history_asset_work(candidates))
}

fn select_price_history_asset_work(
    candidates: Vec<PriceHistoryWorkCandidate>,
) -> Vec<PriceHistoryAssetWork> {
    let rows = candidates
        .into_iter()
        .filter_map(|candidate| {
            candidate
                .provider_asset_id
                .map(|provider_asset_id| PriceHistoryAssetWork {
                    asset_id: candidate.asset_id,
                    provider_asset_id,
                    first_owned_date: candidate
                        .first_evidence_date
                        .unwrap_or(candidate.created_at_date),
                })
        })
        .collect();

    merge_asset_work(rows)
}

#[cfg(feature = "server")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PriceHistoryFirstOwnedDates {
    native_by_account_id: std::collections::HashMap<String, NaiveDate>,
    manual_by_account_id: std::collections::HashMap<String, NaiveDate>,
}

#[cfg(feature = "server")]
fn load_price_history_first_owned_dates(
    user_id: UserId,
) -> Result<PriceHistoryFirstOwnedDates, crate::db::DbError> {
    crate::db::with_user_db(user_id, |conn| {
        Ok(PriceHistoryFirstOwnedDates {
            native_by_account_id: load_native_first_owned_dates(conn)?,
            manual_by_account_id: load_manual_first_owned_dates(conn)?,
        })
    })
}

#[cfg(feature = "server")]
fn load_native_first_owned_dates(
    conn: &rusqlite::Connection,
) -> Result<std::collections::HashMap<String, NaiveDate>, crate::db::DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT account_id, MIN(occurred_at)
             FROM account_transaction_ledger
             WHERE status = 'confirmed'
             GROUP BY account_id",
        )
        .map_err(|err| {
            crate::db::DbError::new(format!(
                "Failed to prepare native first-owned date query: {err}"
            ))
        })?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(|err| {
            crate::db::DbError::new(format!("Failed to query native first-owned dates: {err}"))
        })?;

    let mut dates = std::collections::HashMap::new();
    for row in rows {
        let (account_id, occurred_at) = row.map_err(|err| {
            crate::db::DbError::new(format!("Failed to read native first-owned date row: {err}"))
        })?;
        if let Some(occurred_at) = occurred_at {
            let occurred_at = crate::models::parse_datetime(&occurred_at).map_err(|err| {
                crate::db::DbError::new(format!("Invalid native first-owned date in DB: {err}"))
            })?;
            dates.insert(account_id, occurred_at.date_naive());
        }
    }

    Ok(dates)
}

#[cfg(feature = "server")]
fn load_manual_first_owned_dates(
    conn: &rusqlite::Connection,
) -> Result<std::collections::HashMap<String, NaiveDate>, crate::db::DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT account_id, MIN(asserted_on)
             FROM manual_asset_balance_assertions
             GROUP BY account_id",
        )
        .map_err(|err| {
            crate::db::DbError::new(format!(
                "Failed to prepare manual first-owned date query: {err}"
            ))
        })?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(|err| {
            crate::db::DbError::new(format!("Failed to query manual first-owned dates: {err}"))
        })?;

    let mut dates = std::collections::HashMap::new();
    for row in rows {
        let (account_id, asserted_on) = row.map_err(|err| {
            crate::db::DbError::new(format!("Failed to read manual first-owned date row: {err}"))
        })?;
        if let Some(asserted_on) = asserted_on {
            let asserted_on =
                NaiveDate::parse_from_str(&asserted_on, "%Y-%m-%d").map_err(|err| {
                    crate::db::DbError::new(format!("Invalid manual first-owned date in DB: {err}"))
                })?;
            dates.insert(account_id, asserted_on);
        }
    }

    Ok(dates)
}

pub(crate) fn merge_asset_work(rows: Vec<PriceHistoryAssetWork>) -> Vec<PriceHistoryAssetWork> {
    let mut merged = std::collections::BTreeMap::<(String, String), NaiveDate>::new();
    for row in rows {
        let key = (row.asset_id, row.provider_asset_id);
        merged
            .entry(key)
            .and_modify(|date| *date = (*date).min(row.first_owned_date))
            .or_insert(row.first_owned_date);
    }

    merged
        .into_iter()
        .map(
            |((asset_id, provider_asset_id), first_owned_date)| PriceHistoryAssetWork {
                asset_id,
                provider_asset_id,
                first_owned_date,
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("test date")
    }

    #[test]
    fn merge_asset_work_keeps_earliest_owned_date() {
        let merged = merge_asset_work(vec![
            PriceHistoryAssetWork {
                asset_id: "bitcoin".to_string(),
                provider_asset_id: "bitcoin".to_string(),
                first_owned_date: d("2026-01-10"),
            },
            PriceHistoryAssetWork {
                asset_id: "bitcoin".to_string(),
                provider_asset_id: "bitcoin".to_string(),
                first_owned_date: d("2026-01-01"),
            },
        ]);

        assert_eq!(
            merged,
            vec![PriceHistoryAssetWork {
                asset_id: "bitcoin".to_string(),
                provider_asset_id: "bitcoin".to_string(),
                first_owned_date: d("2026-01-01"),
            }]
        );
    }

    #[test]
    fn select_asset_work_uses_first_evidence_date_and_skips_unpriceable_assets() {
        let selected = select_price_history_asset_work(vec![
            PriceHistoryWorkCandidate {
                asset_id: "bitcoin".to_string(),
                provider_asset_id: Some("bitcoin".to_string()),
                created_at_date: d("2026-02-01"),
                first_evidence_date: Some(d("2026-01-10")),
            },
            PriceHistoryWorkCandidate {
                asset_id: "unpriced".to_string(),
                provider_asset_id: None,
                created_at_date: d("2026-01-01"),
                first_evidence_date: Some(d("2025-12-01")),
            },
        ]);

        assert_eq!(
            selected,
            vec![PriceHistoryAssetWork {
                asset_id: "bitcoin".to_string(),
                provider_asset_id: "bitcoin".to_string(),
                first_owned_date: d("2026-01-10"),
            }]
        );
    }

    #[test]
    fn select_asset_work_falls_back_to_created_at_and_merges_duplicate_assets() {
        let selected = select_price_history_asset_work(vec![
            PriceHistoryWorkCandidate {
                asset_id: "bitcoin".to_string(),
                provider_asset_id: Some("bitcoin".to_string()),
                created_at_date: d("2026-02-01"),
                first_evidence_date: None,
            },
            PriceHistoryWorkCandidate {
                asset_id: "bitcoin".to_string(),
                provider_asset_id: Some("bitcoin".to_string()),
                created_at_date: d("2026-03-01"),
                first_evidence_date: Some(d("2026-01-15")),
            },
        ]);

        assert_eq!(
            selected,
            vec![PriceHistoryAssetWork {
                asset_id: "bitcoin".to_string(),
                provider_asset_id: "bitcoin".to_string(),
                first_owned_date: d("2026-01-15"),
            }]
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn first_owned_date_sql_uses_confirmed_native_and_earliest_manual_evidence() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "
            CREATE TABLE account_transaction_ledger (
                account_id TEXT NOT NULL,
                status TEXT NOT NULL,
                occurred_at TEXT NOT NULL
            );
            CREATE TABLE manual_asset_balance_assertions (
                account_id TEXT NOT NULL,
                asserted_on TEXT NOT NULL
            );
            INSERT INTO account_transaction_ledger (account_id, status, occurred_at) VALUES
                ('native-1', 'pending', '2026-01-01T00:00:00Z'),
                ('native-1', 'confirmed', '2026-01-10T00:00:00Z'),
                ('native-1', 'confirmed', '2026-01-20T00:00:00Z');
            INSERT INTO manual_asset_balance_assertions (account_id, asserted_on) VALUES
                ('manual-1', '2026-03-10'),
                ('manual-1', '2026-02-05');
            ",
        )
        .expect("schema and rows");

        let native = load_native_first_owned_dates(&conn).expect("native first-owned dates");
        let manual = load_manual_first_owned_dates(&conn).expect("manual first-owned dates");

        assert_eq!(native.get("native-1"), Some(&d("2026-01-10")));
        assert_eq!(manual.get("manual-1"), Some(&d("2026-02-05")));
    }
}
