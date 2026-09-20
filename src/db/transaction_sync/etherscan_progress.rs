use crate::db::error::DbError;
use crate::db::user_db::{with_user_db, with_user_db_mut};
use crate::models::UserId;
use crate::transactions::EthereumBlockNumber;
use crate::wallets::DigitalAssetAddressId;
use rusqlite::{OptionalExtension, params};

/// A reconciled page boundary. No completed coverage is inferred from this cursor.
pub(crate) struct EtherscanPendingRange {
    pub(crate) start_block: EthereumBlockNumber,
    pub(crate) end_block: EthereumBlockNumber,
    pub(crate) transaction_tip: Option<EthereumBlockNumber>,
}

pub(crate) fn load_etherscan_pending_range(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
) -> Result<Option<EtherscanPendingRange>, DbError> {
    with_user_db(user_id, |conn| {
        let raw = conn
            .query_row(
                "SELECT start_block, end_block, transaction_tip FROM etherscan_pending_ranges
             WHERE address_id = ?1",
                [address_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|err| DbError::from_rusqlite_error("load etherscan pending range", err))?;
        raw.map(|(start, end, tip)| {
            let parse = |value| {
                EthereumBlockNumber::try_new(value)
                    .map_err(|err| DbError::new(format!("Invalid etherscan pending range: {err}")))
            };
            Ok(EtherscanPendingRange {
                start_block: parse(start)?,
                end_block: parse(end)?,
                transaction_tip: tip.map(parse).transpose()?,
            })
        })
        .transpose()
    })
}

pub(crate) fn save_etherscan_pending_range(
    user_id: UserId,
    address_id: DigitalAssetAddressId,
    range: Option<EtherscanPendingRange>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let result = match range {
            Some(range) => conn.execute(
                "INSERT INTO etherscan_pending_ranges (address_id, start_block, end_block, transaction_tip)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(address_id) DO UPDATE SET start_block = excluded.start_block,
                    end_block = excluded.end_block, transaction_tip = excluded.transaction_tip",
                params![address_id.to_string(), range.start_block.value(), range.end_block.value(),
                    range.transaction_tip.map(EthereumBlockNumber::value)],
            ),
            None => conn.execute("DELETE FROM etherscan_pending_ranges WHERE address_id = ?1",
                [address_id.to_string()]),
        };
        result
            .map(|_| ())
            .map_err(|err| DbError::from_rusqlite_error("save etherscan pending range", err))
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    #[test]
    fn pending_range_migration_enforces_bounds_and_cascades_deleted_addresses() {
        let conn = rusqlite::Connection::open_in_memory().expect("test database should open");
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
            CREATE TABLE digital_asset_addresses (id TEXT PRIMARY KEY);
            INSERT INTO digital_asset_addresses VALUES ('address');",
        )
        .expect("parent fixture should load");
        conn.execute_batch(include_str!(
            "../../../migrations/user/V54__etherscan_pending_range.sql"
        ))
        .expect("pending range migration should apply");
        conn.execute(
            "INSERT INTO etherscan_pending_ranges VALUES ('address', 99, 249, 250)",
            [],
        )
        .expect("valid recent range should persist");
        assert!(
            conn.execute("UPDATE etherscan_pending_ranges SET end_block = 98", [])
                .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE etherscan_pending_ranges SET transaction_tip = 248",
                []
            )
            .is_err()
        );
        conn.execute("DELETE FROM digital_asset_addresses", [])
            .expect("address should delete");
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM etherscan_pending_ranges", [], |row| {
                row.get(0)
            })
            .expect("pending range count should load");
        assert_eq!(remaining, 0);
    }
}
