use crate::db::{SyncTransactionInputRecord, SyncTransactionOutputRecord, SyncTransactionRecord};
use crate::integrations::mempool::{MempoolAddressTransaction, MempoolTransactionStatus};
use crate::tasks::jobs::sync::UserTransactionMonitorError;
use crate::transactions::{ChainTransactionStatus, TrackedAddress, TxHash};
use chrono::{DateTime, TimeZone, Utc};

fn map_status(raw: &MempoolTransactionStatus) -> ChainTransactionStatus {
    if raw.confirmed {
        ChainTransactionStatus::Confirmed
    } else {
        ChainTransactionStatus::Pending
    }
}

fn parse_optional_address(
    raw_address: Option<&str>,
) -> Result<Option<TrackedAddress>, UserTransactionMonitorError> {
    raw_address
        .filter(|value| !value.trim().is_empty())
        .map(TrackedAddress::parse)
        .transpose()
        .map_err(|err| UserTransactionMonitorError::Parse(format!("address parse error: {err}")))
}

fn parse_block_time(
    raw_block_time: Option<i64>,
) -> Result<Option<DateTime<Utc>>, UserTransactionMonitorError> {
    raw_block_time
        .map(|seconds| {
            Utc.timestamp_opt(seconds, 0).single().ok_or_else(|| {
                UserTransactionMonitorError::Parse(format!("invalid block timestamp: {seconds}"))
            })
        })
        .transpose()
}

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

fn map_mempool_transaction(
    raw: &MempoolAddressTransaction,
) -> Result<SyncTransactionRecord, UserTransactionMonitorError> {
    let tx_hash = TxHash::parse(&raw.txid)
        .map_err(|err| UserTransactionMonitorError::Parse(format!("tx hash parse error: {err}")))?;
    let status = map_status(&raw.status);
    let block_time = parse_block_time(raw.status.block_time)?;
    let fee_amount = raw
        .fee
        .map(|value| parse_non_negative_i64(value, "fee"))
        .transpose()?;

    let mut inputs = Vec::new();
    for (position, vin) in raw.vin.iter().enumerate() {
        let Some(prev_txid_raw) = vin.txid.as_deref() else {
            continue;
        };
        let Some(prev_output_index) = vin.vout else {
            continue;
        };
        let input_index = i64::try_from(position).map_err(|err| {
            UserTransactionMonitorError::Parse(format!("input index out of range: {err}"))
        })?;
        let prev_tx_hash = TxHash::parse(prev_txid_raw).map_err(|err| {
            UserTransactionMonitorError::Parse(format!("prev tx hash parse error: {err}"))
        })?;
        let prev_output_index = parse_non_negative_i64(prev_output_index, "prev_output_index")?;
        let value_amount = vin
            .prevout
            .as_ref()
            .map(|prevout| parse_non_negative_i64(prevout.value, "input value"))
            .transpose()?;
        let prev_address = parse_optional_address(
            vin.prevout
                .as_ref()
                .and_then(|prevout| prevout.scriptpubkey_address.as_deref()),
        )?;

        inputs.push(SyncTransactionInputRecord {
            input_index,
            prev_tx_hash,
            prev_output_index,
            prev_address,
            value_amount,
        });
    }

    let mut outputs = Vec::new();
    for (position, vout) in raw.vout.iter().enumerate() {
        if vout.scriptpubkey.trim().is_empty() {
            return Err(UserTransactionMonitorError::Parse(
                "output script_pubkey cannot be empty".to_string(),
            ));
        }
        let output_index = i64::try_from(position).map_err(|err| {
            UserTransactionMonitorError::Parse(format!("output index out of range: {err}"))
        })?;
        let value_amount = parse_non_negative_i64(vout.value, "output value")?;
        let raw_address = parse_optional_address(vout.scriptpubkey_address.as_deref())?;

        outputs.push(SyncTransactionOutputRecord {
            output_index,
            raw_address,
            script_pubkey_hex: vout.scriptpubkey.clone(),
            value_amount,
        });
    }

    Ok(SyncTransactionRecord {
        tx_hash,
        status,
        block_height: raw.status.block_height,
        block_hash: raw.status.block_hash.clone(),
        block_time,
        fee_amount,
        inputs,
        outputs,
    })
}

pub(crate) fn map_mempool_transactions(
    raw_transactions: &[MempoolAddressTransaction],
) -> Result<Vec<SyncTransactionRecord>, UserTransactionMonitorError> {
    let mut mapped = Vec::with_capacity(raw_transactions.len());
    for raw in raw_transactions {
        mapped.push(map_mempool_transaction(raw)?);
    }
    Ok(mapped)
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn map_status_converts_confirmed_and_pending() {
        let confirmed = MempoolTransactionStatus {
            confirmed: true,
            block_height: Some(1),
            block_hash: Some("abc".to_string()),
            block_time: Some(1),
        };
        let pending = MempoolTransactionStatus {
            confirmed: false,
            block_height: None,
            block_hash: None,
            block_time: None,
        };

        assert_eq!(map_status(&confirmed), ChainTransactionStatus::Confirmed);
        assert_eq!(map_status(&pending), ChainTransactionStatus::Pending);
    }

    #[test]
    fn parse_non_negative_i64_rejects_negative_values() {
        assert!(parse_non_negative_i64(-1, "value").is_err());
        assert!(parse_non_negative_i64(0, "value").is_ok());
    }
}
