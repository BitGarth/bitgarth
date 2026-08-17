use crate::amounts::UnsignedAmount;
use crate::db::{ProviderTransferKey, SyncAccountTransactionRecord, SyncAccountTransferRecord};
use crate::ethereum::{EthAddress, RawEthAddress, TransferKind};
use crate::integrations::etherscan::{EtherscanInternalTx, EtherscanNormalTx};
use crate::tasks::jobs::sync::UserTransactionMonitorError;
use crate::transactions::{ChainTransactionStatus, TrackedAddress, TxHash};
use chrono::{DateTime, TimeZone, Utc};
use std::collections::BTreeMap;

fn parse_non_negative_i64(
    value: i64,
    field_name: &'static str,
) -> Result<i64, UserTransactionMonitorError> {
    if value < 0 {
        return Err(UserTransactionMonitorError::Parse(format!(
            "{field_name} cannot be negative: {value}"
        )));
    }
    Ok(value)
}

fn parse_etherscan_tx_hash(raw_hash: &str) -> Result<TxHash, UserTransactionMonitorError> {
    let trimmed = raw_hash.trim();
    let normalized = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    TxHash::parse(normalized)
        .map_err(|err| UserTransactionMonitorError::Parse(format!("tx hash parse error: {err}")))
}

fn parse_etherscan_decimal_u128(
    raw: &str,
    field_name: &'static str,
) -> Result<u128, UserTransactionMonitorError> {
    raw.trim().parse::<u128>().map_err(|err| {
        UserTransactionMonitorError::Parse(format!("{field_name} parse error: {err}"))
    })
}

fn parse_etherscan_block_height(raw: &str) -> Result<Option<i64>, UserTransactionMonitorError> {
    let parsed = raw.trim().parse::<i64>().map_err(|err| {
        UserTransactionMonitorError::Parse(format!("block_number parse error: {err}"))
    })?;
    let normalized = parse_non_negative_i64(parsed, "block_number")?;
    if normalized == 0_i64 {
        Ok(None)
    } else {
        Ok(Some(normalized))
    }
}

fn parse_etherscan_nonce(raw: &str) -> Result<Option<i64>, UserTransactionMonitorError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed = trimmed
        .parse::<i64>()
        .map_err(|err| UserTransactionMonitorError::Parse(format!("nonce parse error: {err}")))?;
    let normalized = parse_non_negative_i64(parsed, "nonce")?;
    Ok(Some(normalized))
}

fn parse_etherscan_block_time(
    raw: &str,
) -> Result<Option<DateTime<Utc>>, UserTransactionMonitorError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let seconds = trimmed.parse::<i64>().map_err(|err| {
        UserTransactionMonitorError::Parse(format!("timestamp parse error: {err}"))
    })?;
    Utc.timestamp_opt(seconds, 0)
        .single()
        .map(Some)
        .ok_or_else(|| {
            UserTransactionMonitorError::Parse(format!("invalid block timestamp: {seconds}"))
        })
}

fn parse_etherscan_address(
    raw: &str,
) -> Result<Option<TrackedAddress>, UserTransactionMonitorError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let parsed_eth =
        EthAddress::parse(&RawEthAddress::new(trimmed.to_string())).map_err(|err| {
            UserTransactionMonitorError::Parse(format!("etherscan address parse error: {err}"))
        })?;
    let canonical = parsed_eth.checksummed();
    TrackedAddress::parse(&canonical)
        .map(Some)
        .map_err(|err| UserTransactionMonitorError::Parse(format!("address parse error: {err}")))
}

fn parse_etherscan_value_amount(raw: &str) -> Result<UnsignedAmount, UserTransactionMonitorError> {
    let parsed = parse_etherscan_decimal_u128(raw, "value")?;
    Ok(UnsignedAmount::from_u128(parsed))
}

fn parse_etherscan_fee_amount(
    gas_used_raw: &str,
    gas_price_raw: &str,
) -> Result<UnsignedAmount, UserTransactionMonitorError> {
    let gas_used = parse_etherscan_decimal_u128(gas_used_raw, "gas_used")?;
    let gas_price = parse_etherscan_decimal_u128(gas_price_raw, "gas_price")?;
    let total = gas_used.checked_mul(gas_price).ok_or_else(|| {
        UserTransactionMonitorError::Parse(
            "fee overflow while multiplying gas_used*gas_price".to_string(),
        )
    })?;
    Ok(UnsignedAmount::from_u128(total))
}

fn flag_is_true(raw: &str) -> bool {
    let trimmed = raw.trim();
    trimmed == "1" || trimmed.eq_ignore_ascii_case("true")
}

fn map_etherscan_normal_status(
    raw: &EtherscanNormalTx,
) -> Result<ChainTransactionStatus, UserTransactionMonitorError> {
    let block_height = parse_etherscan_block_height(&raw.block_number)?;
    if block_height.is_none() {
        return Ok(ChainTransactionStatus::Pending);
    }

    if flag_is_true(&raw.is_error) || raw.txreceipt_status.trim() == "0" {
        Ok(ChainTransactionStatus::Failed)
    } else {
        Ok(ChainTransactionStatus::Confirmed)
    }
}

fn map_etherscan_internal_status(
    raw: &EtherscanInternalTx,
) -> Result<ChainTransactionStatus, UserTransactionMonitorError> {
    let block_height = parse_etherscan_block_height(&raw.block_number)?;
    if block_height.is_none() {
        return Ok(ChainTransactionStatus::Pending);
    }

    if flag_is_true(&raw.is_error) {
        Ok(ChainTransactionStatus::Failed)
    } else {
        Ok(ChainTransactionStatus::Confirmed)
    }
}

fn merge_chain_status(
    current: ChainTransactionStatus,
    incoming: ChainTransactionStatus,
) -> ChainTransactionStatus {
    use ChainTransactionStatus::{Confirmed, Dropped, Failed, Pending};
    match (current, incoming) {
        (Failed, _) | (_, Failed) => Failed,
        (Pending, _) | (_, Pending) => Pending,
        (Confirmed, _) | (_, Confirmed) => Confirmed,
        _ => Dropped,
    }
}

fn max_optional_i64(lhs: Option<i64>, rhs: Option<i64>) -> Option<i64> {
    match (lhs, rhs) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn latest_optional_time(
    lhs: Option<DateTime<Utc>>,
    rhs: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (lhs, rhs) {
        (Some(left), Some(right)) => Some(if left >= right { left } else { right }),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn map_internal_transfer_kind(raw_call_type: &str) -> TransferKind {
    let normalized = raw_call_type.trim().to_ascii_lowercase();
    if normalized == "selfdestruct" || normalized == "suicide" {
        TransferKind::SelfDestruct
    } else {
        TransferKind::Internal
    }
}

#[derive(Debug, Clone)]
struct EtherscanAggregatedTx {
    tx_hash: TxHash,
    status: ChainTransactionStatus,
    block_height: Option<i64>,
    block_time: Option<DateTime<Utc>>,
    fee_amount: Option<UnsignedAmount>,
    nonce: Option<i64>,
    transfers: Vec<SyncAccountTransferRecord>,
}

fn upsert_aggregated_transfer(
    aggregated: &mut EtherscanAggregatedTx,
    transfer: SyncAccountTransferRecord,
) {
    if let Some(existing) = aggregated
        .transfers
        .iter_mut()
        .find(|value| value.provider_transfer_key == transfer.provider_transfer_key)
    {
        *existing = transfer;
    } else {
        aggregated.transfers.push(transfer);
    }
}

fn assign_transfer_display_indices(transfers: &mut [SyncAccountTransferRecord]) {
    transfers.sort_by(|left, right| {
        match (
            left.provider_transfer_key.is_normal(),
            right.provider_transfer_key.is_normal(),
        ) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => left
                .provider_transfer_key
                .as_str()
                .cmp(right.provider_transfer_key.as_str()),
        }
    });

    let mut next_internal_index = 1_i64;
    for transfer in transfers {
        if transfer.provider_transfer_key.is_normal() {
            transfer.transfer_index = 0;
        } else {
            transfer.transfer_index = next_internal_index;
            next_internal_index = next_internal_index.saturating_add(1);
        }
    }
}

pub(crate) fn map_etherscan_transactions(
    normal_transactions: Vec<EtherscanNormalTx>,
    internal_transactions: Vec<EtherscanInternalTx>,
) -> Result<Vec<SyncAccountTransactionRecord>, UserTransactionMonitorError> {
    let mut by_hash = BTreeMap::<String, EtherscanAggregatedTx>::new();

    for raw in normal_transactions {
        let tx_hash = parse_etherscan_tx_hash(&raw.hash)?;
        let status = map_etherscan_normal_status(&raw)?;
        let block_height = parse_etherscan_block_height(&raw.block_number)?;
        let block_time = parse_etherscan_block_time(&raw.time_stamp)?;
        let fee_amount = parse_etherscan_fee_amount(&raw.gas_used, &raw.gas_price)?;
        let nonce = parse_etherscan_nonce(&raw.nonce)?;
        let from_address = parse_etherscan_address(&raw.from)?;
        let to_address = parse_etherscan_address(&raw.to)?;
        let value_amount = parse_etherscan_value_amount(&raw.value)?;
        let tx_key = tx_hash.as_str().to_string();

        let aggregated = by_hash
            .entry(tx_key)
            .or_insert_with(|| EtherscanAggregatedTx {
                tx_hash: tx_hash.clone(),
                status,
                block_height,
                block_time,
                fee_amount: Some(fee_amount),
                nonce,
                transfers: Vec::new(),
            });

        aggregated.status = merge_chain_status(aggregated.status, status);
        aggregated.block_height = max_optional_i64(aggregated.block_height, block_height);
        aggregated.block_time = latest_optional_time(aggregated.block_time, block_time);
        if aggregated.fee_amount.is_none() {
            aggregated.fee_amount = Some(fee_amount);
        }
        if aggregated.nonce.is_none() {
            aggregated.nonce = nonce;
        }

        upsert_aggregated_transfer(
            aggregated,
            SyncAccountTransferRecord {
                provider_transfer_key: ProviderTransferKey::normal(),
                transfer_index: 0_i64,
                transfer_kind: TransferKind::Normal,
                from_address,
                to_address,
                value_amount,
            },
        );
    }

    for raw in internal_transactions {
        let tx_hash = parse_etherscan_tx_hash(&raw.hash)?;
        let status = map_etherscan_internal_status(&raw)?;
        let block_height = parse_etherscan_block_height(&raw.block_number)?;
        let block_time = parse_etherscan_block_time(&raw.time_stamp)?;
        let from_address = parse_etherscan_address(&raw.from)?;
        let to_address = parse_etherscan_address(&raw.to)?;
        let value_amount = parse_etherscan_value_amount(&raw.value)?;
        let provider_transfer_key = ProviderTransferKey::from_internal_trace_id(&raw.trace_id)
            .ok_or_else(|| {
                UserTransactionMonitorError::Parse(
                    "etherscan internal trace_id cannot be empty".to_string(),
                )
            })?;
        let transfer_kind = map_internal_transfer_kind(&raw.call_type);
        let tx_key = tx_hash.as_str().to_string();

        let aggregated = by_hash
            .entry(tx_key)
            .or_insert_with(|| EtherscanAggregatedTx {
                tx_hash: tx_hash.clone(),
                status,
                block_height,
                block_time,
                fee_amount: None,
                nonce: None,
                transfers: Vec::new(),
            });

        aggregated.status = merge_chain_status(aggregated.status, status);
        aggregated.block_height = max_optional_i64(aggregated.block_height, block_height);
        aggregated.block_time = latest_optional_time(aggregated.block_time, block_time);

        upsert_aggregated_transfer(
            aggregated,
            SyncAccountTransferRecord {
                provider_transfer_key,
                transfer_index: 0,
                transfer_kind,
                from_address,
                to_address,
                value_amount,
            },
        );
    }

    let mut mapped = Vec::with_capacity(by_hash.len());
    for aggregated in by_hash.into_values() {
        let mut transfers = aggregated.transfers;
        assign_transfer_display_indices(&mut transfers);
        mapped.push(SyncAccountTransactionRecord {
            tx_hash: aggregated.tx_hash,
            status: aggregated.status,
            block_height: aggregated.block_height,
            block_hash: None,
            block_time: aggregated.block_time,
            fee_amount: aggregated.fee_amount,
            nonce: aggregated.nonce,
            transfers,
        });
    }
    Ok(mapped)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn make_normal_tx(hash: &str) -> EtherscanNormalTx {
        EtherscanNormalTx {
            hash: hash.to_string(),
            block_number: "123".to_string(),
            time_stamp: "1700000000".to_string(),
            from: "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed".to_string(),
            to: "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
            value: "100".to_string(),
            gas_price: "10".to_string(),
            gas_used: "21000".to_string(),
            is_error: "0".to_string(),
            txreceipt_status: "1".to_string(),
            nonce: "7".to_string(),
        }
    }

    fn make_internal_tx(hash: &str) -> EtherscanInternalTx {
        EtherscanInternalTx {
            hash: hash.to_string(),
            block_number: "123".to_string(),
            time_stamp: "1700000001".to_string(),
            from: "0x52908400098527886E0F7030069857D2E4169EE7".to_string(),
            to: "0x8617E340B3D01FA5F11F306F4090FD50E238070D".to_string(),
            value: "50".to_string(),
            is_error: "0".to_string(),
            call_type: "selfdestruct".to_string(),
            trace_id: "2".to_string(),
        }
    }

    #[test]
    fn parse_etherscan_tx_hash_strips_prefix() {
        let parsed = parse_etherscan_tx_hash(
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("valid hash");
        assert_eq!(
            parsed.as_str(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn map_etherscan_normal_status_maps_failed() {
        let mut tx =
            make_normal_tx("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        tx.is_error = "1".to_string();
        tx.txreceipt_status = "0".to_string();

        let status = map_etherscan_normal_status(&tx).expect("status");
        assert_eq!(status, ChainTransactionStatus::Failed);
    }

    #[test]
    fn map_etherscan_transactions_computes_normal_transaction_fee() {
        let mapped = map_etherscan_transactions(
            vec![make_normal_tx(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )],
            Vec::new(),
        )
        .expect("mapped");

        assert_eq!(mapped.len(), 1);
        assert_eq!(
            mapped[0].fee_amount,
            Some(UnsignedAmount::from_u128(210_000))
        );
    }

    #[test]
    fn map_etherscan_transactions_maps_normal_and_internal_transfer_indices() {
        let mapped = map_etherscan_transactions(
            vec![make_normal_tx(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )],
            vec![make_internal_tx(
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )],
        )
        .expect("mapped");
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].transfers.len(), 2);
        assert_eq!(mapped[0].transfers[0].transfer_index, 0);
        assert_eq!(mapped[0].transfers[0].transfer_kind, TransferKind::Normal);
        assert_eq!(mapped[0].transfers[1].transfer_index, 1);
        assert_eq!(
            mapped[0].transfers[1].transfer_kind,
            TransferKind::SelfDestruct
        );
    }

    #[test]
    fn colliding_legacy_trace_indices_keep_both_internal_transfers() {
        let hash = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut first = make_internal_tx(hash);
        first.trace_id = "1".to_string();
        first.value = "11".to_string();
        let mut nested = make_internal_tx(hash);
        nested.trace_id = "0_1".to_string();
        nested.value = "22".to_string();

        let mapped = map_etherscan_transactions(Vec::new(), vec![first, nested])
            .expect("both internal transfers should map");
        let transfers = &mapped[0].transfers;
        assert_eq!(transfers.len(), 2);
        assert_eq!(
            transfers
                .iter()
                .map(|transfer| (
                    transfer.provider_transfer_key.as_str(),
                    transfer.transfer_index,
                ))
                .collect::<Vec<_>>(),
            vec![("internal:0_1", 1), ("internal:1", 2)]
        );
    }

    #[test]
    fn internal_provider_identity_is_stable_across_response_subsets() {
        let hash = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let mut target = make_internal_tx(hash);
        target.trace_id = "4_2".to_string();
        target.value = "77".to_string();
        let mut sibling = make_internal_tx(hash);
        sibling.trace_id = "1".to_string();

        let alone = map_etherscan_transactions(Vec::new(), vec![target.clone()])
            .expect("single transfer should map");
        let with_sibling = map_etherscan_transactions(Vec::new(), vec![sibling, target])
            .expect("subset should map");

        let key = |records: &[SyncAccountTransactionRecord]| {
            records[0]
                .transfers
                .iter()
                .find(|transfer| transfer.value_amount == UnsignedAmount::from_u128(77))
                .expect("target transfer should exist")
                .provider_transfer_key
                .as_str()
                .to_string()
        };
        assert_eq!(key(&alone), "internal:4_2");
        assert_eq!(key(&with_sibling), "internal:4_2");
    }
}
