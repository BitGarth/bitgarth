use super::super::*;
use super::support::*;
use crate::db::acquire_test_runtime;
use crate::db::error::DbError;
use crate::db::user_db::with_user_db_mut;
use crate::models::UserId;
use crate::wallets::{DigitalAssetAddressId, Network};
use chrono::Utc;
use rusqlite::params;

#[test]
fn source_connection_reactivation_reuses_existing_identity() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let first_address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("aa");
    insert_test_eth_address(user_id, first_address_id, &watched_address);
    let original_source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Etherscan,
        Network::Mainnet,
        first_address_id,
    );

    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::new(format!("Failed to start source deactivation tx: {err}"))
        })?;
        deactivate_source_connection_for_address_tx(&tx, first_address_id, Utc::now())?;
        tx.execute(
            "DELETE FROM digital_asset_addresses WHERE id = ?1",
            params![first_address_id.to_string()],
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to delete old test address", err))?;
        tx.commit().map_err(|err| {
            DbError::new(format!("Failed to commit source deactivation tx: {err}"))
        })?;
        Ok::<(), DbError>(())
    })
    .expect("source deactivation should succeed");

    let second_address_id = DigitalAssetAddressId::new();
    insert_test_eth_address(user_id, second_address_id, &watched_address);
    let reactivated_source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Etherscan,
        Network::Mainnet,
        second_address_id,
    );

    assert_eq!(
        original_source_connection_id,
        reactivated_source_connection_id
    );
}
