use super::super::*;
use super::support::*;
use crate::db::acquire_test_runtime;
use crate::db::error::DbError;
use crate::db::user_db::{with_user_db, with_user_db_mut};
use crate::models::UserId;
use crate::wallets::{DigitalAssetAddressId, Network};
use chrono::Utc;
use rusqlite::params;
use ulid::Ulid;

#[test]
fn insert_raw_etherscan_normal_version_reuses_existing_row_for_exact_duplicate() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("ab");
    insert_test_eth_address(user_id, address_id, &watched_address);
    let source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Etherscan,
        Network::Mainnet,
        address_id,
    );
    let tx_hash = sample_txid("11");
    let payload_bytes = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa11","blockNumber":"10","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","gasPrice":"7","gasUsed":"9","isError":"0","txreceipt_status":"1","nonce":"3"}"#,
    );
    let payload_hash_sha256_hex = PayloadSha256Hex::from_payload(&payload_bytes);
    let observed_at = Utc::now();

    let first = insert_raw_etherscan_normal_transaction_version(
        user_id,
        InsertRawEtherscanNormalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            payload_hash_sha256_hex: payload_hash_sha256_hex.clone(),
            payload_bytes: payload_bytes.clone(),
            first_observed_at: observed_at,
        },
    )
    .expect("first raw etherscan normal insert should succeed");

    let second = insert_raw_etherscan_normal_transaction_version(
        user_id,
        InsertRawEtherscanNormalTransactionVersionRequest {
            source_connection_id,
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            payload_hash_sha256_hex,
            payload_bytes,
            first_observed_at: observed_at + chrono::Duration::seconds(5),
        },
    )
    .expect("second raw etherscan normal insert should reuse");

    assert_eq!(first.raw_version_id, second.raw_version_id);
}

#[test]
fn insert_raw_etherscan_internal_version_reuses_existing_row_for_exact_duplicate() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("cd");
    insert_test_eth_address(user_id, address_id, &watched_address);
    let source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Etherscan,
        Network::Mainnet,
        address_id,
    );
    let tx_hash = sample_txid("12");
    let trace_id = EtherscanTraceId::parse("4").expect("trace id");
    let payload_bytes = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa12","blockNumber":"10","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","isError":"0","type":"call","traceId":"4"}"#,
    );
    let payload_hash_sha256_hex = PayloadSha256Hex::from_payload(&payload_bytes);
    let observed_at = Utc::now();

    let first = insert_raw_etherscan_internal_transaction_version(
        user_id,
        InsertRawEtherscanInternalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            trace_id: trace_id.clone(),
            payload_hash_sha256_hex: payload_hash_sha256_hex.clone(),
            payload_bytes: payload_bytes.clone(),
            first_observed_at: observed_at,
        },
    )
    .expect("first raw etherscan internal insert should succeed");

    let second = insert_raw_etherscan_internal_transaction_version(
        user_id,
        InsertRawEtherscanInternalTransactionVersionRequest {
            source_connection_id,
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            trace_id,
            payload_hash_sha256_hex,
            payload_bytes,
            first_observed_at: observed_at + chrono::Duration::seconds(5),
        },
    )
    .expect("second raw etherscan internal insert should reuse");

    assert_eq!(first.raw_version_id, second.raw_version_id);
}

#[test]
fn raw_etherscan_normal_version_reuses_unchanged_current_head() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("61");
    insert_test_eth_address(user_id, address_id, &watched_address);
    let source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Etherscan,
        Network::Mainnet,
        address_id,
    );
    let tx_hash = sample_txid("61");
    let payload = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa61","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","gasPrice":"7","gasUsed":"9","isError":"0","txreceipt_status":"1","nonce":"1"}"#,
    );

    let first = insert_raw_etherscan_normal_transaction_version(
        user_id,
        InsertRawEtherscanNormalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload),
            payload_bytes: payload.clone(),
            first_observed_at: utc_dt(2026, 4, 4, 10, 0, 0),
        },
    )
    .expect("first normal version should insert");
    let second = insert_raw_etherscan_normal_transaction_version(
        user_id,
        InsertRawEtherscanNormalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload),
            payload_bytes: payload,
            first_observed_at: utc_dt(2026, 4, 4, 10, 1, 0),
        },
    )
    .expect("unchanged normal version should reuse head");

    assert_eq!(first.raw_version_id, second.raw_version_id);
    assert_eq!(
        second.write_outcome,
        RawVersionWriteOutcome::ReusedCurrentHead
    );

    let heads = load_current_raw_etherscan_normal_transaction_heads(user_id, &source_connection_id)
        .expect("current normal heads should load");
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0].raw_version_id, first.raw_version_id);
    assert_eq!(heads[0].tx_hash, tx_hash);
}

#[test]
fn raw_etherscan_normal_version_inserts_fresh_head_for_reverted_payload() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("62");
    insert_test_eth_address(user_id, address_id, &watched_address);
    let source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Etherscan,
        Network::Mainnet,
        address_id,
    );
    let tx_hash = sample_txid("62");
    let payload_a = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa62","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","gasPrice":"7","gasUsed":"9","isError":"0","txreceipt_status":"1","nonce":"1"}"#,
    );
    let payload_b = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa62","blockNumber":"101","timeStamp":"2","from":"0x1111111111111111111111111111111111111111","to":"0x3333333333333333333333333333333333333333","value":"6","gasPrice":"8","gasUsed":"10","isError":"0","txreceipt_status":"1","nonce":"2"}"#,
    );

    let first = insert_raw_etherscan_normal_transaction_version(
        user_id,
        InsertRawEtherscanNormalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_a),
            payload_bytes: payload_a.clone(),
            first_observed_at: utc_dt(2026, 4, 4, 10, 0, 0),
        },
    )
    .expect("first normal version should insert");
    let second = insert_raw_etherscan_normal_transaction_version(
        user_id,
        InsertRawEtherscanNormalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_b),
            payload_bytes: payload_b,
            first_observed_at: utc_dt(2026, 4, 4, 10, 1, 0),
        },
    )
    .expect("second normal version should insert");
    let third = insert_raw_etherscan_normal_transaction_version(
        user_id,
        InsertRawEtherscanNormalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_a),
            payload_bytes: payload_a,
            first_observed_at: utc_dt(2026, 4, 4, 10, 2, 0),
        },
    )
    .expect("reverted normal payload should insert fresh head");

    assert_ne!(first.raw_version_id, third.raw_version_id);
    assert_eq!(third.write_outcome, RawVersionWriteOutcome::InsertedNewHead);

    let lineage: Vec<(String, Option<String>)> = with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, supersedes_raw_version_id
                     FROM raw_etherscan_normal_transaction_versions
                     WHERE source_connection_id = ?1 AND tx_hash = ?2
                     ORDER BY created_at ASC, id ASC",
            )
            .map_err(|err| DbError::from_rusqlite_error("prepare normal lineage query", err))?;
        stmt.query_map(
            params![source_connection_id.to_string(), tx_hash.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map_err(|err| DbError::from_rusqlite_error("query normal lineage rows", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| DbError::from_rusqlite_error("read normal lineage rows", err))
    })
    .expect("normal lineage rows should load");

    assert_eq!(lineage.len(), 3);
    assert_eq!(lineage[0], (first.raw_version_id.to_string(), None));
    assert_eq!(
        lineage[1],
        (
            second.raw_version_id.to_string(),
            Some(first.raw_version_id.to_string()),
        )
    );
    assert_eq!(
        lineage[2],
        (
            third.raw_version_id.to_string(),
            Some(second.raw_version_id.to_string()),
        )
    );
}

#[test]
fn raw_etherscan_internal_version_reuses_unchanged_current_head() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("63");
    insert_test_eth_address(user_id, address_id, &watched_address);
    let source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Etherscan,
        Network::Mainnet,
        address_id,
    );
    let tx_hash = sample_txid("63");
    let trace_id = EtherscanTraceId::parse("0").expect("trace id");
    let payload = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa63","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","isError":"0","type":"call","traceId":"0"}"#,
    );

    let first = insert_raw_etherscan_internal_transaction_version(
        user_id,
        InsertRawEtherscanInternalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            trace_id: trace_id.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload),
            payload_bytes: payload.clone(),
            first_observed_at: utc_dt(2026, 4, 4, 10, 0, 0),
        },
    )
    .expect("first internal version should insert");
    let second = insert_raw_etherscan_internal_transaction_version(
        user_id,
        InsertRawEtherscanInternalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            trace_id: trace_id.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload),
            payload_bytes: payload,
            first_observed_at: utc_dt(2026, 4, 4, 10, 1, 0),
        },
    )
    .expect("unchanged internal version should reuse head");

    assert_eq!(first.raw_version_id, second.raw_version_id);
    assert_eq!(
        second.write_outcome,
        RawVersionWriteOutcome::ReusedCurrentHead
    );

    let heads =
        load_current_raw_etherscan_internal_transaction_heads(user_id, &source_connection_id)
            .expect("current internal heads should load");
    assert_eq!(heads.len(), 1);
    assert_eq!(heads[0].raw_version_id, first.raw_version_id);
    assert_eq!(heads[0].tx_hash, tx_hash);
    assert_eq!(heads[0].trace_id, trace_id);
}

#[test]
fn raw_etherscan_internal_version_inserts_fresh_head_for_reverted_payload() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("64");
    insert_test_eth_address(user_id, address_id, &watched_address);
    let source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Etherscan,
        Network::Mainnet,
        address_id,
    );
    let tx_hash = sample_txid("64");
    let trace_id = EtherscanTraceId::parse("0").expect("trace id");
    let payload_a = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa64","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","isError":"0","type":"call","traceId":"0"}"#,
    );
    let payload_b = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa64","blockNumber":"100","timeStamp":"2","from":"0x1111111111111111111111111111111111111111","to":"0x3333333333333333333333333333333333333333","value":"6","isError":"0","type":"delegatecall","traceId":"0"}"#,
    );

    let first = insert_raw_etherscan_internal_transaction_version(
        user_id,
        InsertRawEtherscanInternalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            trace_id: trace_id.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_a),
            payload_bytes: payload_a.clone(),
            first_observed_at: utc_dt(2026, 4, 4, 10, 0, 0),
        },
    )
    .expect("first internal version should insert");
    let second = insert_raw_etherscan_internal_transaction_version(
        user_id,
        InsertRawEtherscanInternalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            trace_id: trace_id.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_b),
            payload_bytes: payload_b,
            first_observed_at: utc_dt(2026, 4, 4, 10, 1, 0),
        },
    )
    .expect("second internal version should insert");
    let third = insert_raw_etherscan_internal_transaction_version(
        user_id,
        InsertRawEtherscanInternalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: tx_hash.clone(),
            trace_id: trace_id.clone(),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&payload_a),
            payload_bytes: payload_a,
            first_observed_at: utc_dt(2026, 4, 4, 10, 2, 0),
        },
    )
    .expect("reverted internal payload should insert fresh head");

    assert_ne!(first.raw_version_id, third.raw_version_id);
    assert_eq!(third.write_outcome, RawVersionWriteOutcome::InsertedNewHead);

    let lineage: Vec<(String, Option<String>)> = with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, supersedes_raw_version_id
                     FROM raw_etherscan_internal_transaction_versions
                     WHERE source_connection_id = ?1 AND tx_hash = ?2 AND trace_id = ?3
                     ORDER BY created_at ASC, id ASC",
            )
            .map_err(|err| DbError::from_rusqlite_error("prepare internal lineage query", err))?;
        stmt.query_map(
            params![
                source_connection_id.to_string(),
                tx_hash.as_str(),
                trace_id.as_str()
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .map_err(|err| DbError::from_rusqlite_error("query internal lineage rows", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| DbError::from_rusqlite_error("read internal lineage rows", err))
    })
    .expect("internal lineage rows should load");

    assert_eq!(lineage.len(), 3);
    assert_eq!(lineage[0], (first.raw_version_id.to_string(), None));
    assert_eq!(
        lineage[1],
        (
            second.raw_version_id.to_string(),
            Some(first.raw_version_id.to_string()),
        )
    );
    assert_eq!(
        lineage[2],
        (
            third.raw_version_id.to_string(),
            Some(second.raw_version_id.to_string()),
        )
    );
}

#[test]
fn raw_etherscan_normal_observations_reassemble_page_in_item_order() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("ef");
    insert_test_eth_address(user_id, address_id, &watched_address);
    let sync_run = start_etherscan_sync_run(user_id, address_id);
    let observation_set = record_test_observation_set(
        user_id,
        sync_run.sync_run_id,
        sync_run.source_connection_id.clone(),
        RawObservationSetGroupingKind::EtherscanNormal,
        r#"{"endpoint_family":"txlist","page_number":1,"page_size":1000,"start_block":"0","end_block":"99999999","window_index":0}"#,
        Utc::now(),
    );
    let source_connection_id = sync_run.source_connection_id.clone();
    let first_payload = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa21","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","gasPrice":"7","gasUsed":"9","isError":"0","txreceipt_status":"1","nonce":"1"}"#,
    );
    let second_payload = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa22","blockNumber":"101","timeStamp":"2","from":"0x1111111111111111111111111111111111111111","to":"0x3333333333333333333333333333333333333333","value":"6","gasPrice":"8","gasUsed":"10","isError":"0","txreceipt_status":"1","nonce":"2"}"#,
    );
    let first_version = insert_raw_etherscan_normal_transaction_version(
        user_id,
        InsertRawEtherscanNormalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: sample_txid("21"),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&first_payload),
            payload_bytes: first_payload,
            first_observed_at: Utc::now(),
        },
    )
    .expect("first version should insert");
    let second_version = insert_raw_etherscan_normal_transaction_version(
        user_id,
        InsertRawEtherscanNormalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: sample_txid("22"),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&second_payload),
            payload_bytes: second_payload,
            first_observed_at: Utc::now(),
        },
    )
    .expect("second version should insert");

    record_raw_etherscan_normal_observation(
        user_id,
        RecordRawEtherscanNormalTransactionObservationRequest {
            sync_run_id: sync_run.sync_run_id,
            source_connection_id: source_connection_id.clone(),
            raw_observation_set_id: observation_set.raw_observation_set_id,
            raw_etherscan_normal_transaction_version_id: second_version.raw_version_id,
            page_item_index: 1,
            observed_at: Utc::now(),
        },
    )
    .expect("second observation should insert");
    record_raw_etherscan_normal_observation(
        user_id,
        RecordRawEtherscanNormalTransactionObservationRequest {
            sync_run_id: sync_run.sync_run_id,
            source_connection_id,
            raw_observation_set_id: observation_set.raw_observation_set_id,
            raw_etherscan_normal_transaction_version_id: first_version.raw_version_id,
            page_item_index: 0,
            observed_at: Utc::now(),
        },
    )
    .expect("first observation should insert");

    let payloads: Vec<String> = with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT CAST(v.payload_bytes AS TEXT)
                 FROM raw_etherscan_normal_transaction_observations o
                 INNER JOIN raw_etherscan_normal_transaction_versions v
                   ON v.id = o.raw_etherscan_normal_transaction_version_id
                 WHERE o.raw_observation_set_id = ?1
                 ORDER BY o.page_item_index ASC",
            )
            .map_err(|err| DbError::from_rusqlite_error("prepare normal reassembly", err))?;
        stmt.query_map(
            [observation_set.raw_observation_set_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|err| DbError::from_rusqlite_error("query normal reassembly", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| DbError::from_rusqlite_error("read normal reassembly", err))
    })
    .expect("normal reassembly query should succeed");

    assert_eq!(payloads.len(), 2);
    assert!(payloads[0].contains(
        "\"hash\":\"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa21\""
    ));
    assert!(payloads[1].contains(
        "\"hash\":\"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa22\""
    ));
}

#[test]
fn raw_etherscan_internal_observations_reassemble_page_in_item_order() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("99");
    insert_test_eth_address(user_id, address_id, &watched_address);
    let sync_run = start_etherscan_sync_run(user_id, address_id);
    let observation_set = record_test_observation_set(
        user_id,
        sync_run.sync_run_id,
        sync_run.source_connection_id.clone(),
        RawObservationSetGroupingKind::EtherscanInternal,
        r#"{"endpoint_family":"txlistinternal","page_number":1,"page_size":1000,"start_block":"0","end_block":"99999999","window_index":0}"#,
        Utc::now(),
    );
    let source_connection_id = sync_run.source_connection_id.clone();
    let first_payload = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa31","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","value":"5","isError":"0","type":"call","traceId":"0"}"#,
    );
    let second_payload = sample_payload(
        r#"{"hash":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa31","blockNumber":"100","timeStamp":"1","from":"0x1111111111111111111111111111111111111111","to":"0x3333333333333333333333333333333333333333","value":"6","isError":"0","type":"call","traceId":"1"}"#,
    );
    let first_version = insert_raw_etherscan_internal_transaction_version(
        user_id,
        InsertRawEtherscanInternalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: sample_txid("31"),
            trace_id: EtherscanTraceId::parse("0").expect("trace id"),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&first_payload),
            payload_bytes: first_payload,
            first_observed_at: Utc::now(),
        },
    )
    .expect("first internal version should insert");
    let second_version = insert_raw_etherscan_internal_transaction_version(
        user_id,
        InsertRawEtherscanInternalTransactionVersionRequest {
            source_connection_id: source_connection_id.clone(),
            chain_id: EtherscanChainId::try_new(1).expect("chain id"),
            network: Network::Mainnet,
            tx_hash: sample_txid("31"),
            trace_id: EtherscanTraceId::parse("1").expect("trace id"),
            payload_hash_sha256_hex: PayloadSha256Hex::from_payload(&second_payload),
            payload_bytes: second_payload,
            first_observed_at: Utc::now(),
        },
    )
    .expect("second internal version should insert");

    record_raw_etherscan_internal_observation(
        user_id,
        RecordRawEtherscanInternalTransactionObservationRequest {
            sync_run_id: sync_run.sync_run_id,
            source_connection_id: source_connection_id.clone(),
            raw_observation_set_id: observation_set.raw_observation_set_id,
            raw_etherscan_internal_transaction_version_id: second_version.raw_version_id,
            page_item_index: 1,
            observed_at: Utc::now(),
        },
    )
    .expect("second internal observation should insert");
    record_raw_etherscan_internal_observation(
        user_id,
        RecordRawEtherscanInternalTransactionObservationRequest {
            sync_run_id: sync_run.sync_run_id,
            source_connection_id,
            raw_observation_set_id: observation_set.raw_observation_set_id,
            raw_etherscan_internal_transaction_version_id: first_version.raw_version_id,
            page_item_index: 0,
            observed_at: Utc::now(),
        },
    )
    .expect("first internal observation should insert");

    let payloads: Vec<String> = with_user_db(user_id, |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT CAST(v.payload_bytes AS TEXT)
                 FROM raw_etherscan_internal_transaction_observations o
                 INNER JOIN raw_etherscan_internal_transaction_versions v
                   ON v.id = o.raw_etherscan_internal_transaction_version_id
                 WHERE o.raw_observation_set_id = ?1
                 ORDER BY o.page_item_index ASC",
            )
            .map_err(|err| DbError::from_rusqlite_error("prepare internal reassembly", err))?;
        stmt.query_map(
            [observation_set.raw_observation_set_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|err| DbError::from_rusqlite_error("query internal reassembly", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| DbError::from_rusqlite_error("read internal reassembly", err))
    })
    .expect("internal reassembly query should succeed");

    assert_eq!(payloads.len(), 2);
    assert!(payloads[0].contains("\"traceId\":\"0\""));
    assert!(payloads[1].contains("\"traceId\":\"1\""));
}

#[test]
fn raw_etherscan_normal_versions_reject_invalid_tx_hash_shape() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("77");
    insert_test_eth_address(user_id, address_id, &watched_address);
    let source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Etherscan,
        Network::Mainnet,
        address_id,
    );

    let error = with_user_db_mut(user_id, |conn| {
            conn.execute(
                "INSERT INTO raw_etherscan_normal_transaction_versions
                 (id, source_connection_id, chain_id, network, tx_hash, payload_hash_sha256_hex, payload_bytes, first_observed_at, supersedes_raw_version_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    Ulid::new().to_string(),
                    source_connection_id.to_string(),
                    1_i64,
                    Network::Mainnet.as_str(),
                    "not-a-hash",
                    "a".repeat(64),
                    vec![1_u8],
                    Utc::now().to_rfc3339(),
                    Option::<String>::None,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map(|_| ())
            .map_err(|err| DbError::from_rusqlite_error("insert invalid normal tx hash", err))
        })
        .expect_err("invalid tx hash should fail");

    assert!(error.to_string().contains("insert invalid normal tx hash"));
}

#[test]
fn raw_etherscan_internal_versions_reject_empty_trace_id() {
    let _guard = acquire_test_runtime();
    let user_id = UserId::new();
    crate::db::initialize_user_db_for_test(user_id).expect("user db should initialize");
    let address_id = DigitalAssetAddressId::new();
    let watched_address = sample_eth_address("88");
    insert_test_eth_address(user_id, address_id, &watched_address);
    let source_connection_id = source_connection_id_for_address(
        user_id,
        IntegrationKind::Etherscan,
        Network::Mainnet,
        address_id,
    );

    let error = with_user_db_mut(user_id, |conn| {
            conn.execute(
                "INSERT INTO raw_etherscan_internal_transaction_versions
                 (id, source_connection_id, chain_id, network, tx_hash, trace_id, payload_hash_sha256_hex, payload_bytes, first_observed_at, supersedes_raw_version_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    Ulid::new().to_string(),
                    source_connection_id.to_string(),
                    1_i64,
                    Network::Mainnet.as_str(),
                    sample_txid("41").as_str(),
                    "",
                    "b".repeat(64),
                    vec![1_u8],
                    Utc::now().to_rfc3339(),
                    Option::<String>::None,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map(|_| ())
            .map_err(|err| DbError::from_rusqlite_error("insert empty trace id", err))
        })
        .expect_err("empty trace id should fail");

    assert!(error.to_string().contains("insert empty trace id"));
}
