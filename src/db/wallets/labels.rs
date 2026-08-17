use super::errors::db_error_from_sqlite;
use super::moves::{WalletAccountStorageKind, load_wallet_account_context_in_tx};
use crate::db::error::DbError;
use crate::db::user_db::with_user_db_mut;
use crate::db::wallet_accounts::query_wallet_account_label_keys_in_tx;
use crate::models::UserId;
use crate::wallets::{Label, LabelKey, WalletAccountId, WalletId};
use chrono::{DateTime, Utc};
use rusqlite::params;

fn account_label_conflict_error() -> DbError {
    DbError::new("Account label already exists in this wallet (idx_maa_label_key)")
}

pub(super) fn ensure_wallet_account_label_available_in_tx(
    tx: &rusqlite::Transaction<'_>,
    wallet_id: WalletId,
    candidate_label: &Label,
    account_id_to_ignore: Option<WalletAccountId>,
) -> Result<(), DbError> {
    let candidate_key = candidate_label.key();
    let existing_keys = query_wallet_account_label_keys_in_tx(tx, wallet_id)?;
    let has_conflict = existing_keys.into_iter().any(|row| {
        if let Some(account_id_to_ignore) = account_id_to_ignore
            && row.account_id == account_id_to_ignore
        {
            return false;
        }
        row.label_key == candidate_key
    });

    if has_conflict {
        return Err(account_label_conflict_error());
    }

    Ok(())
}

/// Resolve the label for a newly created account.
///
/// When the user supplied `provided_label`, verify it does not collide with an
/// existing account in the wallet and use it verbatim. Otherwise fall back to
/// `generate_default`, which auto-generates a unique label from the wallet's
/// existing account label keys.
pub(super) fn resolve_new_account_label<F>(
    tx: &rusqlite::Transaction<'_>,
    wallet_id: WalletId,
    provided_label: Option<&Label>,
    generate_default: F,
) -> Result<Label, DbError>
where
    F: FnOnce(&[LabelKey]) -> Result<Label, DbError>,
{
    match provided_label {
        Some(label) => {
            ensure_wallet_account_label_available_in_tx(tx, wallet_id, label, None)?;
            Ok(label.clone())
        }
        None => {
            let existing_keys = query_wallet_account_label_keys_in_tx(tx, wallet_id)?
                .into_iter()
                .map(|row| row.label_key)
                .collect::<Vec<_>>();
            generate_default(&existing_keys)
        }
    }
}

pub(crate) fn update_wallet_label(
    user_id: UserId,
    wallet_id: WalletId,
    label: Label,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let label_key = label.key();
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "UPDATE wallets SET label = ?1, label_key = ?2, updated_at = ?3 WHERE id = ?4",
            params![
                label.as_str(),
                label_key.as_str(),
                now.to_rfc3339(),
                wallet_id.to_string()
            ],
        )
        .map_err(|e| db_error_from_sqlite("Failed to update wallet label", e))?;
        Ok(())
    })
}

pub(crate) fn update_account_label(
    user_id: UserId,
    account_id: impl Into<WalletAccountId>,
    label: Label,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let account_id = account_id.into();
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|e| {
            DbError::new(format!(
                "Failed to start account label update transaction: {e}"
            ))
        })?;
        let context = load_wallet_account_context_in_tx(&tx, account_id)
            .map_err(|e| DbError::new(e.to_string()))?;
        ensure_wallet_account_label_available_in_tx(
            &tx,
            context.current_wallet_id,
            &label,
            Some(account_id),
        )?;
        let label_key = label.key();
        let sql = match context.kind {
            WalletAccountStorageKind::Native => {
                "UPDATE digital_asset_accounts
                 SET label = ?1, label_key = ?2, updated_at = ?3
                 WHERE id = ?4"
            }
            WalletAccountStorageKind::Manual => {
                "UPDATE manual_asset_accounts
                 SET label = ?1, label_key = ?2, updated_at = ?3
                 WHERE id = ?4"
            }
        };
        let updated = tx
            .execute(
                sql,
                params![
                    label.as_str(),
                    label_key.as_str(),
                    now.to_rfc3339(),
                    account_id.to_string()
                ],
            )
            .map_err(|e| db_error_from_sqlite("Failed to update account label", e))?;
        if updated == 0 {
            return Err(DbError::new("Account not found"));
        }
        tx.commit()
            .map_err(|e| DbError::new(format!("Failed to commit account label update: {e}")))?;
        Ok(())
    })
}
