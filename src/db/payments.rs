use super::error::DbError;
use super::user_db::{with_user_db, with_user_db_mut};
use crate::models::UserId;
use crate::payments::keys::VerifiedEntitlementToken;
use crate::payments::types::{
    EntitlementHolderId, EntitlementTier, PaymentAmount, PaymentOrderId, PaymentOrderStatus,
    PaymentSecret, ProductTier, SubscriptionSubjectId, TokenId,
    capability_schema_version_from_storage_json, entitlement_capabilities_storage_json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{OptionalExtension, params};
use std::collections::HashSet;
use std::str::FromStr;

const PAYMENT_SUBJECT_ROW_ID: &str = "premium";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaymentSubjectRecord {
    pub(crate) entitlement_holder_id: EntitlementHolderId,
    pub(crate) management_secret: Option<PaymentSecret>,
    pub(crate) active_token_history_id: Option<TokenId>,
    pub(crate) last_refresh_at: Option<DateTime<Utc>>,
    pub(crate) last_refresh_status: Option<String>,
    pub(crate) last_capability_refresh_at: Option<DateTime<Utc>>,
    pub(crate) last_successful_capability_refresh_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NewPaymentOrder {
    pub(crate) order_id: PaymentOrderId,
    pub(crate) order_secret: PaymentSecret,
    pub(crate) product_tier: ProductTier,
    pub(crate) amount: PaymentAmount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaymentOrderRecord {
    pub(crate) order_id: PaymentOrderId,
    pub(crate) order_secret: PaymentSecret,
    pub(crate) product_tier: ProductTier,
    pub(crate) amount: PaymentAmount,
    pub(crate) status: PaymentOrderStatus,
    pub(crate) paid_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaymentOrderHistoryRecord {
    pub(crate) order_id: PaymentOrderId,
    pub(crate) product_tier: ProductTier,
    pub(crate) amount: PaymentAmount,
    pub(crate) status: PaymentOrderStatus,
    pub(crate) paid_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NewPaymentOrderHistoryRecord {
    pub(crate) order_id: PaymentOrderId,
    pub(crate) product_tier: ProductTier,
    pub(crate) amount: PaymentAmount,
    pub(crate) status: PaymentOrderStatus,
    pub(crate) paid_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NewPendingPremiumTransfer {
    pub(crate) source_file_name: String,
    pub(crate) imported_management_secret: PaymentSecret,
    pub(crate) imported_active_token: Option<String>,
    pub(crate) imported_token_id: Option<TokenId>,
    pub(crate) imported_subscription_subject_id: Option<SubscriptionSubjectId>,
    pub(crate) imported_subscription_valid_until: Option<DateTime<Utc>>,
    pub(crate) imported_token_expires_at: Option<DateTime<Utc>>,
    pub(crate) imported_token_issued_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingPremiumTransferRecord {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) imported_management_secret: PaymentSecret,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TokenHistoryStatus {
    Active,
    Inactive,
    Revoked,
    Superseded,
    Expired,
    Invalidated,
}

impl TokenHistoryStatus {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Revoked => "revoked",
            Self::Superseded => "superseded",
            Self::Expired => "expired",
            Self::Invalidated => "invalidated",
        }
    }
}

impl std::str::FromStr for TokenHistoryStatus {
    type Err = DbError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "inactive" => Ok(Self::Inactive),
            "revoked" => Ok(Self::Revoked),
            "superseded" => Ok(Self::Superseded),
            "expired" => Ok(Self::Expired),
            "invalidated" => Ok(Self::Invalidated),
            other => Err(DbError::new(format!(
                "Invalid token history status: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaymentTokenHistoryRecord {
    pub(crate) token_id: TokenId,
    pub(crate) subscription_subject_id: SubscriptionSubjectId,
    pub(crate) active_token: String,
    pub(crate) entitlement_tier: EntitlementTier,
    pub(crate) subscription_valid_until: DateTime<Utc>,
    pub(crate) token_expires_at: DateTime<Utc>,
    pub(crate) token_issued_at: DateTime<Utc>,
    pub(crate) capability_set_id: Option<String>,
    pub(crate) capability_schema_version: u16,
    pub(crate) capabilities_json: Option<String>,
    pub(crate) status: TokenHistoryStatus,
    pub(crate) status_reason: Option<String>,
    pub(crate) first_seen_at: DateTime<Utc>,
    pub(crate) last_seen_at: DateTime<Utc>,
    pub(crate) deactivated_at: Option<DateTime<Utc>>,
}

pub(crate) fn load_payment_subject(
    user_id: UserId,
) -> Result<Option<PaymentSubjectRecord>, DbError> {
    with_user_db(user_id, query_payment_subject)
}

pub(crate) fn load_or_create_payment_subject(
    user_id: UserId,
    now: DateTime<Utc>,
) -> Result<PaymentSubjectRecord, DbError> {
    with_user_db_mut(user_id, |conn| {
        if let Some(record) = query_payment_subject(conn)? {
            return Ok(record);
        }

        let entitlement_holder_id = EntitlementHolderId::new();
        conn.execute(
            "INSERT INTO payment_subject (id, entitlement_holder_id, updated_at) VALUES (?1, ?2, ?3)",
            params![
                PAYMENT_SUBJECT_ROW_ID,
                entitlement_holder_id.to_storage_value(),
                format_timestamp(now),
            ],
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to insert payment subject", err))?;

        query_payment_subject(conn)?
            .ok_or_else(|| DbError::new("Payment subject insert did not persist"))
    })
}

pub(crate) fn load_active_token_history(
    user_id: UserId,
) -> Result<Option<PaymentTokenHistoryRecord>, DbError> {
    with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT h.token_id, h.subscription_subject_id, h.active_token, h.entitlement_tier, \
                    h.subscription_valid_until, h.token_expires_at, h.token_issued_at, \
                    h.capability_set_id, h.capabilities_json, h.status, h.status_reason, \
                    h.first_seen_at, h.last_seen_at, h.deactivated_at \
             FROM payment_token_history h \
             JOIN payment_subject s ON s.active_token_history_id = h.token_id \
             WHERE s.id = ?1 AND h.status = 'active'",
            [PAYMENT_SUBJECT_ROW_ID],
            parse_token_history_row,
        )
        .optional()
        .map_err(|err| DbError::from_rusqlite_error("Failed to load active token history", err))
    })
}

pub(crate) fn update_payment_management_secret(
    user_id: UserId,
    management_secret: &PaymentSecret,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "UPDATE payment_subject SET management_secret = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                management_secret.as_str(),
                format_timestamp(now),
                PAYMENT_SUBJECT_ROW_ID,
            ],
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to update management secret", err))?;
        Ok(())
    })
}

pub(crate) fn insert_payment_order(
    user_id: UserId,
    order: &NewPaymentOrder,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "INSERT INTO payment_orders \
             (order_id, order_secret, product_tier, order_amount_minor_units, order_currency, order_display_scale, status, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7)",
            params![
                order.order_id.to_storage_value(),
                order.order_secret.as_str(),
                order.product_tier.as_str(),
                i64::try_from(order.amount.minor_units)
                    .map_err(|_| DbError::new("Payment order amount exceeds SQLite range"))?,
                &order.amount.currency,
                i64::from(order.amount.decimal_precision),
                format_timestamp(now),
            ],
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to insert payment order", err))?;
        Ok(())
    })
}

pub(crate) fn load_payment_order(
    user_id: UserId,
    order_id: PaymentOrderId,
) -> Result<Option<PaymentOrderRecord>, DbError> {
    with_user_db(user_id, |conn| query_payment_order(conn, order_id))
}

pub(crate) fn load_latest_payment_order(
    user_id: UserId,
) -> Result<Option<PaymentOrderRecord>, DbError> {
    with_user_db(user_id, query_latest_payment_order)
}

pub(crate) fn load_all_payment_order_history(
    user_id: UserId,
) -> Result<Vec<PaymentOrderHistoryRecord>, DbError> {
    with_user_db(user_id, |conn| {
        let mut records = Vec::new();
        let mut local_order_ids = HashSet::new();

        let mut local_stmt = conn
            .prepare(
                "SELECT order_id, order_secret, product_tier, order_amount_minor_units, order_currency, \
                        order_display_scale, status, paid_at \
                 FROM payment_orders ORDER BY order_id ASC",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to prepare local payment orders query", err)
            })?;
        let local_rows = local_stmt
            .query_map([], |row| {
                let order_id_raw: String = row.get(0)?;
                let order_secret_raw: String = row.get(1)?;
                let product_tier_raw: String = row.get(2)?;
                let minor_units_raw: i64 = row.get(3)?;
                let currency: String = row.get(4)?;
                let decimal_precision_raw: i64 = row.get(5)?;
                let status_raw: String = row.get(6)?;
                let paid_at_raw: Option<String> = row.get(7)?;

                parse_payment_order_row(PaymentOrderRow {
                    order_id_raw,
                    order_secret_raw,
                    product_tier_raw,
                    minor_units_raw,
                    currency,
                    decimal_precision_raw,
                    status_raw,
                    paid_at_raw,
                })
                .map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })
            })
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to query local payment orders", err)
            })?;
        for row in local_rows {
            let order = row.map_err(|err| {
                DbError::from_rusqlite_error("Failed to collect local payment orders", err)
            })?;
            local_order_ids.insert(order.order_id);
            records.push(payment_order_to_history_record(order));
        }

        let mut imported_stmt = conn
            .prepare(
                "SELECT order_id, product_tier, order_amount_minor_units, order_currency, \
                        order_display_scale, status, paid_at \
                 FROM payment_order_history ORDER BY order_id ASC",
            )
            .map_err(|err| {
                DbError::from_rusqlite_error(
                    "Failed to prepare imported payment history query",
                    err,
                )
            })?;
        let imported_rows = imported_stmt
            .query_map([], |row| {
                let order_id_raw: String = row.get(0)?;
                let product_tier_raw: String = row.get(1)?;
                let minor_units_raw: i64 = row.get(2)?;
                let currency: String = row.get(3)?;
                let decimal_precision_raw: i64 = row.get(4)?;
                let status_raw: String = row.get(5)?;
                let paid_at_raw: Option<String> = row.get(6)?;

                parse_payment_order_history_row(PaymentOrderHistoryRow {
                    order_id_raw,
                    product_tier_raw,
                    minor_units_raw,
                    currency,
                    decimal_precision_raw,
                    status_raw,
                    paid_at_raw,
                })
                .map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })
            })
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to query imported payment history", err)
            })?;
        for row in imported_rows {
            let record = row.map_err(|err| {
                DbError::from_rusqlite_error("Failed to collect imported payment history", err)
            })?;
            if !local_order_ids.contains(&record.order_id) {
                records.push(record);
            }
        }

        records.sort_by_key(|record| record.order_id.to_storage_value());
        Ok(records)
    })
}

pub(crate) fn upsert_imported_payment_order_history(
    user_id: UserId,
    orders: &[NewPaymentOrderHistoryRecord],
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn.transaction().map_err(|err| {
            DbError::from_rusqlite_error("Failed to begin imported payment history tx", err)
        })?;
        let now = format_timestamp(now);
        for order in orders {
            tx.execute(
                "INSERT INTO payment_order_history \
                 (order_id, product_tier, order_amount_minor_units, order_currency, \
                  order_display_scale, status, paid_at, imported_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8) \
                 ON CONFLICT(order_id) DO UPDATE SET \
                  product_tier = excluded.product_tier, \
                  order_amount_minor_units = excluded.order_amount_minor_units, \
                  order_currency = excluded.order_currency, \
                  order_display_scale = excluded.order_display_scale, \
                  status = excluded.status, \
                  paid_at = excluded.paid_at, \
                  updated_at = excluded.updated_at",
                params![
                    order.order_id.to_storage_value(),
                    order.product_tier.as_str(),
                    i64::try_from(order.amount.minor_units)
                        .map_err(|_| DbError::new("Payment history amount exceeds SQLite range"))?,
                    &order.amount.currency,
                    i64::from(order.amount.decimal_precision),
                    order.status.as_str(),
                    order.paid_at.map(format_timestamp),
                    &now,
                ],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to upsert imported payment history", err)
            })?;
        }
        tx.commit().map_err(|err| {
            DbError::from_rusqlite_error("Failed to commit imported payment history tx", err)
        })
    })
}

pub(crate) fn cancel_payment_order(
    user_id: UserId,
    order_id: PaymentOrderId,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    with_user_db_mut(user_id, |conn| {
        let rows = conn
            .execute(
                "UPDATE payment_orders SET status = 'canceled', updated_at = ?1 \
                 WHERE order_id = ?2 AND status = 'pending'",
                params![format_timestamp(now), order_id.to_storage_value()],
            )
            .map_err(|err| DbError::from_rusqlite_error("Failed to cancel payment order", err))?;
        Ok(rows > 0)
    })
}

pub(crate) fn mark_payment_order_status(
    user_id: UserId,
    order_id: PaymentOrderId,
    status: PaymentOrderStatus,
    paid_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "UPDATE payment_orders SET status = ?1, paid_at = ?2, updated_at = ?3 WHERE order_id = ?4",
            params![
                status.as_str(),
                paid_at.map(format_timestamp),
                format_timestamp(now),
                order_id.to_storage_value(),
            ],
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to update payment order", err))?;
        Ok(())
    })
}

pub(crate) fn reconcile_payment_order_status(
    user_id: UserId,
    order_id: PaymentOrderId,
    status: PaymentOrderStatus,
    paid_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    with_user_db_mut(user_id, |conn| {
        let rows = conn
            .execute(
                "UPDATE payment_orders SET status = ?1, paid_at = ?2, updated_at = ?3 \
                 WHERE order_id = ?4 AND status IN ('pending', 'canceled')",
                params![
                    status.as_str(),
                    paid_at.map(format_timestamp),
                    format_timestamp(now),
                    order_id.to_storage_value(),
                ],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to reconcile payment order", err)
            })?;
        Ok(rows > 0)
    })
}

pub(crate) fn reconcile_imported_payment_order_history_status(
    user_id: UserId,
    order_id: PaymentOrderId,
    status: PaymentOrderStatus,
    paid_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    with_user_db_mut(user_id, |conn| {
        let rows = conn
            .execute(
                "UPDATE payment_order_history SET status = ?1, paid_at = ?2, updated_at = ?3 \
                 WHERE order_id = ?4 AND status IN ('pending', 'canceled')",
                params![
                    status.as_str(),
                    paid_at.map(format_timestamp),
                    format_timestamp(now),
                    order_id.to_storage_value(),
                ],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to reconcile imported payment history", err)
            })?;
        Ok(rows > 0)
    })
}

pub(crate) fn store_verified_premium_token(
    user_id: UserId,
    order_id: Option<PaymentOrderId>,
    verified: &VerifiedEntitlementToken,
    paid_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    store_verified_premium_token_with_activation_transition(
        user_id, order_id, verified, paid_at, now,
    )
    .map(|_| ())
}

pub(crate) fn store_verified_premium_token_with_activation_transition(
    user_id: UserId,
    order_id: Option<PaymentOrderId>,
    verified: &VerifiedEntitlementToken,
    paid_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn
            .transaction()
            .map_err(|err| DbError::from_rusqlite_error("Failed to begin payment token tx", err))?;

        let had_active_token = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM payment_token_history WHERE status = 'active')",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to inspect active payment token", err)
            })?;

        let capabilities_json = entitlement_capabilities_storage_json(
            verified.claims.capability_schema_version,
            &verified.claims.capabilities,
        )
        .map_err(|err| DbError::new(format!("Failed to serialize capabilities: {err}")))?;

        let now_ts = format_timestamp(now);

        // Deactivate previous active row (if any).
        tx.execute(
            "UPDATE payment_token_history SET status = 'superseded', deactivated_at = ?1 \
             WHERE status = 'active'",
            params![&now_ts],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to deactivate previous active token", err)
        })?;

        // Upsert the new token history row.
        tx.execute(
            "INSERT INTO payment_token_history \
             (token_id, subscription_subject_id, active_token, entitlement_tier, \
              subscription_valid_until, token_expires_at, token_issued_at, \
              capability_set_id, capabilities_json, status, status_reason, \
              first_seen_at, last_seen_at, deactivated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active', NULL, ?10, ?10, NULL) \
             ON CONFLICT(token_id) DO UPDATE SET \
              active_token = excluded.active_token, \
              entitlement_tier = excluded.entitlement_tier, \
              subscription_valid_until = excluded.subscription_valid_until, \
              token_expires_at = excluded.token_expires_at, \
              token_issued_at = excluded.token_issued_at, \
              capability_set_id = excluded.capability_set_id, \
              capabilities_json = excluded.capabilities_json, \
              status = 'active', status_reason = NULL, \
              last_seen_at = excluded.last_seen_at, deactivated_at = NULL",
            params![
                verified.claims.token_id.to_storage_value(),
                verified.claims.subscription_subject_id.to_storage_value(),
                verified.compact_token.as_str(),
                verified.claims.tier.as_str(),
                format_timestamp(verified.claims.subscription_valid_until),
                format_timestamp(verified.claims.token_expires_at),
                format_timestamp(verified.claims.issued_at),
                verified.claims.capability_set_id.as_deref(),
                capabilities_json,
                &now_ts,
            ],
        )
        .map_err(|err| DbError::from_rusqlite_error("Failed to upsert token history row", err))?;

        // Set pointer and refresh timestamps.
        tx.execute(
            "UPDATE payment_subject \
             SET active_token_history_id = ?1, \
                 last_capability_refresh_at = ?2, last_successful_capability_refresh_at = ?2, \
                 updated_at = ?2 \
             WHERE id = ?3",
            params![
                verified.claims.token_id.to_storage_value(),
                &now_ts,
                PAYMENT_SUBJECT_ROW_ID,
            ],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to update payment subject pointer", err)
        })?;

        if let Some(order_id) = order_id {
            tx.execute(
                "UPDATE payment_orders SET status = 'paid', paid_at = ?1, updated_at = ?2 WHERE order_id = ?3",
                params![
                    paid_at.map(format_timestamp),
                    &now_ts,
                    order_id.to_storage_value(),
                ],
            )
            .map_err(|err| DbError::from_rusqlite_error("Failed to mark payment order paid", err))?;
        }

        // Prune inactive rows beyond 50.
        tx.execute(
            "DELETE FROM payment_token_history \
             WHERE status != 'active' \
               AND token_id NOT IN (\
                 SELECT token_id FROM payment_token_history \
                 WHERE status != 'active' \
                 ORDER BY last_seen_at DESC, first_seen_at DESC \
                 LIMIT 50\
               )",
            [],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to prune inactive token history", err)
        })?;

        tx.commit().map_err(|err| {
            DbError::from_rusqlite_error("Failed to commit payment token tx", err)
        })?;
        Ok(!had_active_token)
    })
}

pub(crate) fn record_payment_refresh_status(
    user_id: UserId,
    status: &str,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "UPDATE payment_subject \
             SET last_refresh_at = ?1, last_refresh_status = ?2, \
                 last_capability_refresh_at = ?1, updated_at = ?1 \
             WHERE id = ?3",
            params![format_timestamp(now), status, PAYMENT_SUBJECT_ROW_ID],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to record payment refresh status", err)
        })?;
        Ok(())
    })
}

pub(crate) fn clear_verified_premium_token(
    user_id: UserId,
    history_status: TokenHistoryStatus,
    status_reason: Option<&str>,
    refresh_status: &str,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        let tx = conn
            .transaction()
            .map_err(|err| DbError::from_rusqlite_error("Failed to begin clear token tx", err))?;
        let now_ts = format_timestamp(now);

        // Mark the active history row as inactive (if any).
        tx.execute(
            "UPDATE payment_token_history \
             SET status = ?1, status_reason = ?2, deactivated_at = ?3 \
             WHERE token_id = (SELECT active_token_history_id FROM payment_subject WHERE id = ?4) \
               AND status = 'active'",
            params![
                history_status.as_str(),
                status_reason,
                &now_ts,
                PAYMENT_SUBJECT_ROW_ID
            ],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to deactivate token history row", err)
        })?;

        // Clear the pointer.
        tx.execute(
            "UPDATE payment_subject \
             SET active_token_history_id = NULL, \
                 last_capability_refresh_at = ?1, \
                 last_refresh_at = ?1, last_refresh_status = ?2, updated_at = ?1 \
             WHERE id = ?3",
            params![&now_ts, refresh_status, PAYMENT_SUBJECT_ROW_ID],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to clear premium token pointer", err)
        })?;
        tx.commit()
            .map_err(|err| DbError::from_rusqlite_error("Failed to commit clear token tx", err))
    })
}

#[cfg(test)]
pub(crate) fn set_token_history_active_token_for_test(
    user_id: UserId,
    token_id: &str,
    active_token: &str,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "UPDATE payment_token_history SET active_token = ?1 WHERE token_id = ?2",
            params![active_token, token_id],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to update test token history active token", err)
        })?;
        Ok(())
    })
}

#[cfg(test)]
fn set_active_token_test_now() -> DateTime<Utc> {
    "2026-04-16T12:00:00Z"
        .parse()
        .unwrap_or_else(|_| unreachable!("hardcoded timestamp is valid"))
}

#[cfg(test)]
pub(crate) fn set_active_token_for_test(
    user_id: crate::models::UserId,
    active_token: Option<&str>,
    token_expires_at: Option<&str>,
    token_issued_at: Option<&str>,
) -> Result<(), crate::db::DbError> {
    if let Some(token) = active_token {
        let subject = load_payment_subject(user_id)?
            .ok_or_else(|| crate::db::DbError::new("Payment subject not found"))?;
        let claims = crate::payments::types::TokenClaims {
            token_id: TokenId::from_str("01JQABCDEF000000000000000F").unwrap(),
            subscription_subject_id: SubscriptionSubjectId::from_str("01JQABCDEF000000000000000G")
                .unwrap(),
            entitlement_holder_id: subject.entitlement_holder_id,
            tier: crate::payments::types::EntitlementTier::Premium,
            capability_set_id: None,
            capability_schema_version: crate::payments::types::CAPABILITY_SCHEMA_VERSION_V3,
            capabilities: crate::payments::types::EntitlementCapabilities::v3_from_parts(
                50, 50000, true,
            ),
            subscription_valid_until: "2027-04-16T12:00:00Z".parse().unwrap(),
            token_expires_at: token_expires_at
                .unwrap_or("2026-04-23T12:00:00Z")
                .parse()
                .unwrap(),
            issued_at: token_issued_at
                .unwrap_or("2026-04-16T12:00:00Z")
                .parse()
                .unwrap(),
        };
        let entitlements = crate::payments::types::FeatureEntitlements::from_capabilities(
            claims.tier.clone(),
            claims.capability_schema_version,
            claims.capabilities.clone(),
            Some(claims.subscription_valid_until),
            Some(claims.token_expires_at),
            crate::payments::types::EntitlementSource::SignedCentralToken,
        );
        let verified = crate::payments::keys::VerifiedEntitlementToken {
            compact_token: token.to_string(),
            claims,
            entitlements,
        };
        store_verified_premium_token(user_id, None, &verified, None, set_active_token_test_now())?;
    } else {
        clear_verified_premium_token(
            user_id,
            TokenHistoryStatus::Inactive,
            None,
            "unavailable",
            set_active_token_test_now(),
        )?;
    }
    Ok(())
}

pub(crate) fn insert_pending_premium_transfer(
    user_id: UserId,
    transfer: &NewPendingPremiumTransfer,
    now: DateTime<Utc>,
) -> Result<String, DbError> {
    let id = ulid::Ulid::new().to_string();
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "INSERT INTO pending_premium_transfers \
             (id, source_file_name, imported_at, status, imported_management_secret, \
              imported_active_token, imported_token_id, imported_subscription_subject_id, \
              imported_subscription_valid_until, imported_token_expires_at, imported_token_issued_at) \
             VALUES (?1, ?2, ?3, 'pending_confirmation', ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &id,
                &transfer.source_file_name,
                format_timestamp(now),
                transfer.imported_management_secret.as_str(),
                transfer.imported_active_token.as_deref(),
                transfer.imported_token_id.map(TokenId::to_storage_value),
                transfer
                    .imported_subscription_subject_id
                    .map(SubscriptionSubjectId::to_storage_value),
                transfer
                    .imported_subscription_valid_until
                    .map(format_timestamp),
                transfer.imported_token_expires_at.map(format_timestamp),
                transfer.imported_token_issued_at.map(format_timestamp),
            ],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to insert pending premium transfer", err)
        })?;
        Ok::<(), DbError>(())
    })?;
    Ok(id)
}

pub(crate) fn load_pending_premium_transfer(
    user_id: UserId,
    pending_transfer_id: &str,
) -> Result<Option<PendingPremiumTransferRecord>, DbError> {
    with_user_db(user_id, |conn| {
        conn.query_row(
            "SELECT id, status, imported_management_secret \
             FROM pending_premium_transfers WHERE id = ?1",
            params![pending_transfer_id],
            |row| {
                let id: String = row.get(0)?;
                let status: String = row.get(1)?;
                let imported_management_secret_raw: String = row.get(2)?;
                let imported_management_secret =
                    PaymentSecret::from_raw(imported_management_secret_raw).map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?;
                Ok(PendingPremiumTransferRecord {
                    id,
                    status,
                    imported_management_secret,
                })
            },
        )
        .optional()
        .map_err(|err| DbError::from_rusqlite_error("Failed to load pending premium transfer", err))
    })
}

pub(crate) fn mark_pending_premium_transfer_completed(
    user_id: UserId,
    pending_transfer_id: &str,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "UPDATE pending_premium_transfers \
             SET status = 'completed', last_attempt_at = ?1, completed_at = ?1, \
                 last_error_code = NULL, last_error_message = NULL \
             WHERE id = ?2",
            params![format_timestamp(now), pending_transfer_id],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to mark pending premium transfer completed", err)
        })?;
        Ok::<(), DbError>(())
    })
}

pub(crate) fn mark_pending_premium_transfer_failure(
    user_id: UserId,
    pending_transfer_id: &str,
    retryable: bool,
    error_code: &str,
    error_message: &str,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let status = if retryable {
        "retryable_failure"
    } else {
        "non_retryable_failure"
    };
    with_user_db_mut(user_id, |conn| {
        conn.execute(
            "UPDATE pending_premium_transfers \
             SET status = ?1, last_attempt_at = ?2, last_error_code = ?3, last_error_message = ?4 \
             WHERE id = ?5",
            params![
                status,
                format_timestamp(now),
                error_code,
                error_message,
                pending_transfer_id,
            ],
        )
        .map_err(|err| {
            DbError::from_rusqlite_error("Failed to mark pending premium transfer failed", err)
        })?;
        Ok::<(), DbError>(())
    })
}

fn query_payment_subject(
    conn: &rusqlite::Connection,
) -> Result<Option<PaymentSubjectRecord>, DbError> {
    conn.query_row(
        "SELECT entitlement_holder_id, management_secret, active_token_history_id, \
                last_refresh_at, last_refresh_status, \
                last_capability_refresh_at, last_successful_capability_refresh_at \
         FROM payment_subject WHERE id = ?1",
        [PAYMENT_SUBJECT_ROW_ID],
        |row| {
            let entitlement_holder_id_raw: String = row.get(0)?;
            let management_secret_raw: Option<String> = row.get(1)?;
            let active_token_history_id_raw: Option<String> = row.get(2)?;
            let last_refresh_at_raw: Option<String> = row.get(3)?;
            let last_refresh_status: Option<String> = row.get(4)?;
            let last_capability_refresh_at_raw: Option<String> = row.get(5)?;
            let last_successful_capability_refresh_at_raw: Option<String> = row.get(6)?;

            parse_payment_subject_row(PaymentSubjectRow {
                entitlement_holder_id_raw,
                management_secret_raw,
                active_token_history_id_raw,
                last_refresh_at_raw,
                last_refresh_status,
                last_capability_refresh_at_raw,
                last_successful_capability_refresh_at_raw,
            })
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })
        },
    )
    .optional()
    .map_err(|err| DbError::from_rusqlite_error("Failed to load payment subject", err))
}

struct PaymentSubjectRow {
    entitlement_holder_id_raw: String,
    management_secret_raw: Option<String>,
    active_token_history_id_raw: Option<String>,
    last_refresh_at_raw: Option<String>,
    last_refresh_status: Option<String>,
    last_capability_refresh_at_raw: Option<String>,
    last_successful_capability_refresh_at_raw: Option<String>,
}

fn parse_payment_subject_row(row: PaymentSubjectRow) -> Result<PaymentSubjectRecord, DbError> {
    Ok(PaymentSubjectRecord {
        entitlement_holder_id: EntitlementHolderId::from_str(&row.entitlement_holder_id_raw)
            .map_err(|err| DbError::new(format!("Invalid payment entitlement holder: {err}")))?,
        management_secret: row
            .management_secret_raw
            .map(PaymentSecret::from_raw)
            .transpose()
            .map_err(|err| DbError::new(format!("Invalid payment management secret: {err}")))?,
        active_token_history_id: row
            .active_token_history_id_raw
            .as_deref()
            .map(TokenId::from_str)
            .transpose()
            .map_err(|err| DbError::new(format!("Invalid active token history id: {err}")))?,
        last_refresh_at: parse_optional_timestamp(
            row.last_refresh_at_raw.as_deref(),
            "last_refresh_at",
        )?,
        last_refresh_status: row.last_refresh_status,
        last_capability_refresh_at: parse_optional_timestamp(
            row.last_capability_refresh_at_raw.as_deref(),
            "last_capability_refresh_at",
        )?,
        last_successful_capability_refresh_at: parse_optional_timestamp(
            row.last_successful_capability_refresh_at_raw.as_deref(),
            "last_successful_capability_refresh_at",
        )?,
    })
}

fn query_payment_order(
    conn: &rusqlite::Connection,
    order_id: PaymentOrderId,
) -> Result<Option<PaymentOrderRecord>, DbError> {
    conn.query_row(
        "SELECT order_id, order_secret, product_tier, order_amount_minor_units, order_currency, \
                order_display_scale, status, paid_at \
         FROM payment_orders WHERE order_id = ?1",
        [order_id.to_storage_value()],
        |row| {
            let order_id_raw: String = row.get(0)?;
            let order_secret_raw: String = row.get(1)?;
            let product_tier_raw: String = row.get(2)?;
            let minor_units_raw: i64 = row.get(3)?;
            let currency: String = row.get(4)?;
            let decimal_precision_raw: i64 = row.get(5)?;
            let status_raw: String = row.get(6)?;
            let paid_at_raw: Option<String> = row.get(7)?;

            parse_payment_order_row(PaymentOrderRow {
                order_id_raw,
                order_secret_raw,
                product_tier_raw,
                minor_units_raw,
                currency,
                decimal_precision_raw,
                status_raw,
                paid_at_raw,
            })
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })
        },
    )
    .optional()
    .map_err(|err| DbError::from_rusqlite_error("Failed to load payment order", err))
}

fn query_latest_payment_order(
    conn: &rusqlite::Connection,
) -> Result<Option<PaymentOrderRecord>, DbError> {
    conn.query_row(
        "SELECT order_id, order_secret, product_tier, order_amount_minor_units, order_currency, \
                order_display_scale, status, paid_at \
         FROM payment_orders ORDER BY updated_at DESC, created_at DESC LIMIT 1",
        [],
        |row| {
            let order_id_raw: String = row.get(0)?;
            let order_secret_raw: String = row.get(1)?;
            let product_tier_raw: String = row.get(2)?;
            let minor_units_raw: i64 = row.get(3)?;
            let currency: String = row.get(4)?;
            let decimal_precision_raw: i64 = row.get(5)?;
            let status_raw: String = row.get(6)?;
            let paid_at_raw: Option<String> = row.get(7)?;

            parse_payment_order_row(PaymentOrderRow {
                order_id_raw,
                order_secret_raw,
                product_tier_raw,
                minor_units_raw,
                currency,
                decimal_precision_raw,
                status_raw,
                paid_at_raw,
            })
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })
        },
    )
    .optional()
    .map_err(|err| DbError::from_rusqlite_error("Failed to load latest payment order", err))
}

struct PaymentOrderRow {
    order_id_raw: String,
    order_secret_raw: String,
    product_tier_raw: String,
    minor_units_raw: i64,
    currency: String,
    decimal_precision_raw: i64,
    status_raw: String,
    paid_at_raw: Option<String>,
}

struct PaymentOrderHistoryRow {
    order_id_raw: String,
    product_tier_raw: String,
    minor_units_raw: i64,
    currency: String,
    decimal_precision_raw: i64,
    status_raw: String,
    paid_at_raw: Option<String>,
}

fn payment_order_to_history_record(order: PaymentOrderRecord) -> PaymentOrderHistoryRecord {
    PaymentOrderHistoryRecord {
        order_id: order.order_id,
        product_tier: order.product_tier,
        amount: order.amount,
        status: order.status,
        paid_at: order.paid_at,
    }
}

fn parse_payment_order_row(row: PaymentOrderRow) -> Result<PaymentOrderRecord, DbError> {
    let minor_units = u64::try_from(row.minor_units_raw)
        .map_err(|_| DbError::new("Invalid payment order amount in DB"))?;
    let decimal_precision = u8::try_from(row.decimal_precision_raw)
        .map_err(|_| DbError::new("Invalid payment order display scale in DB"))?;
    Ok(PaymentOrderRecord {
        order_id: PaymentOrderId::from_str(&row.order_id_raw)
            .map_err(|err| DbError::new(format!("Invalid payment order id: {err}")))?,
        order_secret: PaymentSecret::from_raw(row.order_secret_raw)
            .map_err(|err| DbError::new(format!("Invalid payment order secret: {err}")))?,
        product_tier: ProductTier::from_str(&row.product_tier_raw)
            .map_err(|err| DbError::new(format!("Invalid payment product tier: {err}")))?,
        amount: PaymentAmount {
            minor_units,
            currency: row.currency,
            currency_symbol: None,
            decimal_precision,
        },
        status: PaymentOrderStatus::from_str(&row.status_raw)
            .map_err(|err| DbError::new(format!("Invalid payment order status: {err}")))?,
        paid_at: parse_optional_timestamp(row.paid_at_raw.as_deref(), "paid_at")?,
    })
}

fn parse_payment_order_history_row(
    row: PaymentOrderHistoryRow,
) -> Result<PaymentOrderHistoryRecord, DbError> {
    let minor_units = u64::try_from(row.minor_units_raw)
        .map_err(|_| DbError::new("Invalid payment history amount in DB"))?;
    let decimal_precision = u8::try_from(row.decimal_precision_raw)
        .map_err(|_| DbError::new("Invalid payment history display scale in DB"))?;
    Ok(PaymentOrderHistoryRecord {
        order_id: PaymentOrderId::from_str(&row.order_id_raw)
            .map_err(|err| DbError::new(format!("Invalid payment history order id: {err}")))?,
        product_tier: ProductTier::from_str(&row.product_tier_raw)
            .map_err(|err| DbError::new(format!("Invalid payment history product tier: {err}")))?,
        amount: PaymentAmount {
            minor_units,
            currency: row.currency,
            currency_symbol: None,
            decimal_precision,
        },
        status: PaymentOrderStatus::from_str(&row.status_raw)
            .map_err(|err| DbError::new(format!("Invalid payment history status: {err}")))?,
        paid_at: parse_optional_timestamp(row.paid_at_raw.as_deref(), "paid_at")?,
    })
}

fn parse_optional_timestamp(
    value: Option<&str>,
    field: &'static str,
) -> Result<Option<DateTime<Utc>>, DbError> {
    value
        .map(|raw| {
            raw.parse::<DateTime<Utc>>()
                .map_err(|err| DbError::new(format!("Invalid payment {field}: {err}")))
        })
        .transpose()
}

fn parse_token_history_row(
    row: &rusqlite::Row<'_>,
) -> Result<PaymentTokenHistoryRecord, rusqlite::Error> {
    let token_id_raw: String = row.get(0)?;
    let subscription_subject_id_raw: String = row.get(1)?;
    let active_token: String = row.get(2)?;
    let entitlement_tier_raw: String = row.get(3)?;
    let subscription_valid_until_raw: String = row.get(4)?;
    let token_expires_at_raw: String = row.get(5)?;
    let token_issued_at_raw: String = row.get(6)?;
    let capability_set_id: Option<String> = row.get(7)?;
    let capabilities_json: Option<String> = row.get(8)?;
    let capability_schema_version =
        capability_schema_version_from_storage_json(capabilities_json.as_deref());
    let status_raw: String = row.get(9)?;
    let status_reason: Option<String> = row.get(10)?;
    let first_seen_at_raw: String = row.get(11)?;
    let last_seen_at_raw: String = row.get(12)?;
    let deactivated_at_raw: Option<String> = row.get(13)?;

    Ok(PaymentTokenHistoryRecord {
        token_id: TokenId::from_str(&token_id_raw).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })?,
        subscription_subject_id: SubscriptionSubjectId::from_str(&subscription_subject_id_raw)
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?,
        active_token,
        entitlement_tier: EntitlementTier::from_str(&entitlement_tier_raw).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(err))
        })?,
        subscription_valid_until: subscription_valid_until_raw
            .parse::<DateTime<Utc>>()
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?,
        token_expires_at: token_expires_at_raw
            .parse::<DateTime<Utc>>()
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?,
        token_issued_at: token_issued_at_raw
            .parse::<DateTime<Utc>>()
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?,
        capability_set_id,
        capability_schema_version,
        capabilities_json,
        status: TokenHistoryStatus::from_str(&status_raw).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(err))
        })?,
        status_reason,
        first_seen_at: first_seen_at_raw.parse::<DateTime<Utc>>().map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                11,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?,
        last_seen_at: last_seen_at_raw.parse::<DateTime<Utc>>().map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                12,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?,
        deactivated_at: deactivated_at_raw
            .map(|raw| raw.parse::<DateTime<Utc>>())
            .transpose()
            .map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    13,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                )
            })?,
    })
}

fn format_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db;
    use crate::payments::types::{
        CAPABILITY_SCHEMA_VERSION_V3, EntitlementCapabilities, EntitlementSource, EntitlementTier,
        FeatureEntitlements, SubscriptionSubjectId, TokenClaims, TokenId,
    };
    use std::error::Error;

    fn test_now() -> DateTime<Utc> {
        "2026-04-16T12:00:00Z"
            .parse()
            .unwrap_or_else(|_| unreachable!("hardcoded timestamp is valid"))
    }

    fn test_secret() -> PaymentSecret {
        PaymentSecret::from_raw("frPMkDek45GSAMEAFTXV5ORxF8p3c5_MqPg7Zq-bNuI")
            .unwrap_or_else(|_| unreachable!("hardcoded secret is valid"))
    }

    fn test_token_claims(subject: &PaymentSubjectRecord) -> TokenClaims {
        TokenClaims {
            token_id: TokenId::from_str("01JQABCDEF000000000000000F").unwrap(),
            subscription_subject_id: SubscriptionSubjectId::from_str("01JQABCDEF000000000000000G")
                .unwrap(),
            entitlement_holder_id: subject.entitlement_holder_id,
            tier: EntitlementTier::Premium,
            capability_set_id: Some("capset_premium_v1".to_string()),
            capability_schema_version: CAPABILITY_SCHEMA_VERSION_V3,
            capabilities: EntitlementCapabilities::v3_from_parts(50, 50000, true),
            subscription_valid_until: "2027-04-16T12:00:00Z".parse().unwrap(),
            token_expires_at: "2026-04-23T12:00:00Z".parse().unwrap(),
            issued_at: test_now(),
        }
    }

    fn test_verified_token(compact: &str, claims: &TokenClaims) -> VerifiedEntitlementToken {
        let entitlements = FeatureEntitlements::from_capabilities(
            claims.tier.clone(),
            claims.capability_schema_version,
            claims.capabilities.clone(),
            Some(claims.subscription_valid_until),
            Some(claims.token_expires_at),
            EntitlementSource::SignedCentralToken,
        );
        VerifiedEntitlementToken {
            compact_token: compact.to_string(),
            claims: claims.clone(),
            entitlements,
        }
    }

    #[test]
    fn load_active_token_history_returns_none_when_no_history() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        load_or_create_payment_subject(user_id, test_now())?;

        assert!(load_active_token_history(user_id)?.is_none());
        Ok(())
    }

    #[test]
    fn store_creates_history_row_and_sets_pointer() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;

        let claims = test_token_claims(&subject);
        let verified = test_verified_token("sig_v1", &claims);
        store_verified_premium_token(user_id, None, &verified, None, test_now())?;

        let reloaded = load_payment_subject(user_id)?.expect("subject should exist");
        assert_eq!(reloaded.active_token_history_id, Some(claims.token_id));

        let history = load_active_token_history(user_id)?.expect("should have active history");
        assert_eq!(history.active_token, "sig_v1");
        assert_eq!(history.token_id, claims.token_id);
        assert_eq!(history.status, TokenHistoryStatus::Active);
        Ok(())
    }

    #[test]
    fn load_or_create_payment_subject_persists_one_holder() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;

        let first = load_or_create_payment_subject(user_id, test_now())?;
        let second = load_or_create_payment_subject(user_id, test_now())?;

        assert_eq!(first.entitlement_holder_id, second.entitlement_holder_id);
        assert!(first.management_secret.is_none());
        assert!(load_payment_subject(user_id)?.is_some());
        Ok(())
    }

    #[test]
    fn payment_order_round_trips_sensitive_server_state() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        load_or_create_payment_subject(user_id, test_now())?;

        let order_id = PaymentOrderId::from_str("01JQABCDEF000000000000000E")?;
        let order = NewPaymentOrder {
            order_id,
            order_secret: test_secret(),
            product_tier: ProductTier::Premium,
            amount: PaymentAmount {
                minor_units: 999,
                currency: "USD".to_string(),
                currency_symbol: Some("$".to_string()),
                decimal_precision: 2,
            },
        };
        insert_payment_order(user_id, &order, test_now())?;

        let stored = load_payment_order(user_id, order_id)?.expect("order should exist");
        assert_eq!(stored.order_secret, order.order_secret);
        assert_eq!(stored.amount.atlos_decimal_amount(), "9.99");
        assert_eq!(stored.status, PaymentOrderStatus::Pending);
        Ok(())
    }

    #[test]
    fn pending_premium_transfer_persists_imported_secret_separately() -> Result<(), Box<dyn Error>>
    {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;

        let pending_id = insert_pending_premium_transfer(
            user_id,
            &NewPendingPremiumTransfer {
                source_file_name: "wallet-data.json".to_string(),
                imported_management_secret: test_secret(),
                imported_active_token: Some("old-holder-token".to_string()),
                imported_token_id: Some(TokenId::from_str("01JQABCDEF000000000000000F")?),
                imported_subscription_subject_id: Some(SubscriptionSubjectId::from_str(
                    "01JQABCDEF000000000000000G",
                )?),
                imported_subscription_valid_until: Some(test_now()),
                imported_token_expires_at: Some(test_now()),
                imported_token_issued_at: Some(test_now()),
            },
            test_now(),
        )?;

        let row = with_user_db(user_id, |conn| {
            conn.query_row(
                "SELECT status, imported_management_secret, imported_active_token \
                 FROM pending_premium_transfers WHERE id = ?1",
                [&pending_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to query pending premium transfer", err)
            })
        })?;

        assert_eq!(row.0, "pending_confirmation");
        assert_eq!(row.1, test_secret().as_str());
        assert_eq!(row.2, "old-holder-token");
        Ok(())
    }

    #[test]
    fn verified_token_updates_subject_and_marks_order_paid() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;

        let order_id = PaymentOrderId::from_str("01JQABCDEF000000000000000E")?;
        insert_payment_order(
            user_id,
            &NewPaymentOrder {
                order_id,
                order_secret: test_secret(),
                product_tier: ProductTier::Premium,
                amount: PaymentAmount {
                    minor_units: 999,
                    currency: "USD".to_string(),
                    currency_symbol: Some("$".to_string()),
                    decimal_precision: 2,
                },
            },
            test_now(),
        )?;

        let claims = TokenClaims {
            token_id: TokenId::from_str("01JQABCDEF000000000000000F")?,
            subscription_subject_id: SubscriptionSubjectId::from_str("01JQABCDEF000000000000000G")?,
            entitlement_holder_id: subject.entitlement_holder_id,
            tier: EntitlementTier::Premium,
            capability_set_id: Some("capset_premium_v1".to_string()),
            capability_schema_version: CAPABILITY_SCHEMA_VERSION_V3,
            capabilities: EntitlementCapabilities::v3_from_parts(50, 50000, true),
            subscription_valid_until: "2027-04-16T12:00:00Z".parse()?,
            token_expires_at: "2026-04-23T12:00:00Z".parse()?,
            issued_at: test_now(),
        };
        let entitlements = FeatureEntitlements::from_capabilities(
            claims.tier.clone(),
            claims.capability_schema_version,
            claims.capabilities.clone(),
            Some(claims.subscription_valid_until),
            Some(claims.token_expires_at),
            EntitlementSource::SignedCentralToken,
        );
        let verified = VerifiedEntitlementToken {
            compact_token: "claims.signature".to_string(),
            claims,
            entitlements,
        };

        store_verified_premium_token(
            user_id,
            Some(order_id),
            &verified,
            Some(test_now()),
            test_now(),
        )?;

        let updated_subject = load_payment_subject(user_id)?.expect("subject should exist");
        assert_eq!(
            updated_subject.active_token_history_id,
            Some(verified.claims.token_id)
        );

        let history = load_active_token_history(user_id)?.expect("should have active history");
        assert_eq!(history.active_token, "claims.signature");
        assert_eq!(history.token_id, verified.claims.token_id);
        assert_eq!(history.entitlement_tier, EntitlementTier::Premium);
        assert_eq!(
            history.capability_set_id.as_deref(),
            Some("capset_premium_v1")
        );
        assert_eq!(
            history.capability_schema_version,
            CAPABILITY_SCHEMA_VERSION_V3
        );
        assert!(history.capabilities_json.is_some());
        assert!(
            history
                .capabilities_json
                .as_deref()
                .is_some_and(|json| json.contains("\"capability_schema_version\":3"))
        );

        let updated_order = load_payment_order(user_id, order_id)?.expect("order should exist");
        assert_eq!(updated_order.status, PaymentOrderStatus::Paid);
        assert_eq!(updated_order.paid_at, Some(test_now()));
        Ok(())
    }

    #[test]
    fn stored_capability_json_omits_background_sync_and_reads_legacy_field() {
        let capabilities = EntitlementCapabilities::v3_from_parts(10, 5000, true);
        let json = capabilities
            .to_storage_json()
            .expect("capabilities serialize");
        assert!(!json.contains("background_sync"));

        let legacy: EntitlementCapabilities = serde_json::from_str(
            r#"{"limits":{"synced_accounts":10,"history":{"max_transactions_per_account":10000}},"features":{"historical_sync":true,"background_sync":true}}"#,
        )
        .expect("legacy capabilities should parse");
        assert!(legacy.features.historical_sync);
    }

    #[test]
    fn management_secret_refresh_status_and_order_status_update_round_trip()
    -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        load_or_create_payment_subject(user_id, test_now())?;

        update_payment_management_secret(user_id, &test_secret(), test_now())?;
        record_payment_refresh_status(user_id, "active", test_now())?;

        let order_id = PaymentOrderId::from_str("01JQABCDEF000000000000000E")?;
        insert_payment_order(
            user_id,
            &NewPaymentOrder {
                order_id,
                order_secret: test_secret(),
                product_tier: ProductTier::Premium,
                amount: PaymentAmount {
                    minor_units: 999,
                    currency: "USD".to_string(),
                    currency_symbol: Some("$".to_string()),
                    decimal_precision: 2,
                },
            },
            test_now(),
        )?;
        mark_payment_order_status(
            user_id,
            order_id,
            PaymentOrderStatus::Failed,
            None,
            test_now(),
        )?;

        let subject = load_payment_subject(user_id)?.expect("subject should exist");
        assert_eq!(subject.management_secret, Some(test_secret()));
        assert_eq!(subject.last_refresh_status.as_deref(), Some("active"));
        let order = load_payment_order(user_id, order_id)?.expect("order should exist");
        assert_eq!(order.status, PaymentOrderStatus::Failed);
        assert_eq!(
            load_latest_payment_order(user_id)?
                .expect("latest order should exist")
                .order_id,
            order_id
        );

        clear_verified_premium_token(
            user_id,
            TokenHistoryStatus::Revoked,
            None,
            "revoked",
            test_now(),
        )?;
        let subject = load_payment_subject(user_id)?.expect("subject should exist");
        assert!(subject.active_token_history_id.is_none());
        assert_eq!(subject.last_refresh_status.as_deref(), Some("revoked"));
        Ok(())
    }

    #[test]
    fn cancel_transitions_pending_to_canceled() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        load_or_create_payment_subject(user_id, test_now())?;

        let order_id = PaymentOrderId::from_str("01JQABCDEF000000000000000E")?;
        insert_payment_order(
            user_id,
            &NewPaymentOrder {
                order_id,
                order_secret: test_secret(),
                product_tier: ProductTier::Premium,
                amount: PaymentAmount {
                    minor_units: 999,
                    currency: "USD".to_string(),
                    currency_symbol: Some("$".to_string()),
                    decimal_precision: 2,
                },
            },
            test_now(),
        )?;

        assert!(cancel_payment_order(user_id, order_id, test_now())?);
        let order = load_payment_order(user_id, order_id)?.expect("order should exist");
        assert_eq!(order.status, PaymentOrderStatus::Canceled);
        Ok(())
    }

    #[test]
    fn cancel_does_not_regress_terminal_status() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        load_or_create_payment_subject(user_id, test_now())?;

        let order_id = PaymentOrderId::from_str("01JQABCDEF000000000000000E")?;
        insert_payment_order(
            user_id,
            &NewPaymentOrder {
                order_id,
                order_secret: test_secret(),
                product_tier: ProductTier::Premium,
                amount: PaymentAmount {
                    minor_units: 999,
                    currency: "USD".to_string(),
                    currency_symbol: Some("$".to_string()),
                    decimal_precision: 2,
                },
            },
            test_now(),
        )?;
        mark_payment_order_status(
            user_id,
            order_id,
            PaymentOrderStatus::Failed,
            None,
            test_now(),
        )?;

        assert!(!cancel_payment_order(user_id, order_id, test_now())?);
        let order = load_payment_order(user_id, order_id)?.expect("order should exist");
        assert_eq!(order.status, PaymentOrderStatus::Failed);
        Ok(())
    }

    #[test]
    fn reconcile_allows_canceled_to_paid() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        load_or_create_payment_subject(user_id, test_now())?;

        let order_id = PaymentOrderId::from_str("01JQABCDEF000000000000000E")?;
        insert_payment_order(
            user_id,
            &NewPaymentOrder {
                order_id,
                order_secret: test_secret(),
                product_tier: ProductTier::Premium,
                amount: PaymentAmount {
                    minor_units: 999,
                    currency: "USD".to_string(),
                    currency_symbol: Some("$".to_string()),
                    decimal_precision: 2,
                },
            },
            test_now(),
        )?;
        cancel_payment_order(user_id, order_id, test_now())?;

        assert!(reconcile_payment_order_status(
            user_id,
            order_id,
            PaymentOrderStatus::Paid,
            Some(test_now()),
            test_now()
        )?);
        let order = load_payment_order(user_id, order_id)?.expect("order should exist");
        assert_eq!(order.status, PaymentOrderStatus::Paid);
        Ok(())
    }

    #[test]
    fn reconcile_allows_canceled_to_failed() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        load_or_create_payment_subject(user_id, test_now())?;

        let order_id = PaymentOrderId::from_str("01JQABCDEF000000000000000E")?;
        insert_payment_order(
            user_id,
            &NewPaymentOrder {
                order_id,
                order_secret: test_secret(),
                product_tier: ProductTier::Premium,
                amount: PaymentAmount {
                    minor_units: 999,
                    currency: "USD".to_string(),
                    currency_symbol: Some("$".to_string()),
                    decimal_precision: 2,
                },
            },
            test_now(),
        )?;
        cancel_payment_order(user_id, order_id, test_now())?;

        assert!(reconcile_payment_order_status(
            user_id,
            order_id,
            PaymentOrderStatus::Failed,
            None,
            test_now()
        )?);
        let order = load_payment_order(user_id, order_id)?.expect("order should exist");
        assert_eq!(order.status, PaymentOrderStatus::Failed);
        Ok(())
    }

    #[test]
    fn reconcile_does_not_regress_paid_to_canceled() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        load_or_create_payment_subject(user_id, test_now())?;

        let order_id = PaymentOrderId::from_str("01JQABCDEF000000000000000E")?;
        insert_payment_order(
            user_id,
            &NewPaymentOrder {
                order_id,
                order_secret: test_secret(),
                product_tier: ProductTier::Premium,
                amount: PaymentAmount {
                    minor_units: 999,
                    currency: "USD".to_string(),
                    currency_symbol: Some("$".to_string()),
                    decimal_precision: 2,
                },
            },
            test_now(),
        )?;
        mark_payment_order_status(
            user_id,
            order_id,
            PaymentOrderStatus::Paid,
            Some(test_now()),
            test_now(),
        )?;

        assert!(!reconcile_payment_order_status(
            user_id,
            order_id,
            PaymentOrderStatus::Failed,
            None,
            test_now()
        )?);
        let order = load_payment_order(user_id, order_id)?.expect("order should exist");
        assert_eq!(order.status, PaymentOrderStatus::Paid);
        Ok(())
    }

    #[test]
    fn load_all_payment_order_history_returns_inserted_local_orders() -> Result<(), Box<dyn Error>>
    {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        load_or_create_payment_subject(user_id, test_now())?;

        let order_a = PaymentOrderId::from_str("01JQABCDEF000000000000000A")?;
        let order_b = PaymentOrderId::from_str("01JQABCDEF000000000000000B")?;
        for order_id in [order_a, order_b] {
            insert_payment_order(
                user_id,
                &NewPaymentOrder {
                    order_id,
                    order_secret: test_secret(),
                    product_tier: ProductTier::Premium,
                    amount: PaymentAmount {
                        minor_units: 999,
                        currency: "USD".to_string(),
                        currency_symbol: Some("$".to_string()),
                        decimal_precision: 2,
                    },
                },
                test_now(),
            )?;
        }

        let all = load_all_payment_order_history(user_id)?;
        assert_eq!(all.len(), 2);
        Ok(())
    }

    #[test]
    fn imported_payment_history_round_trips_without_order_secret() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;

        let order_id = PaymentOrderId::from_str("01JQABCDEF000000000000000E")?;
        upsert_imported_payment_order_history(
            user_id,
            &[NewPaymentOrderHistoryRecord {
                order_id,
                product_tier: ProductTier::Premium,
                amount: PaymentAmount {
                    minor_units: 999,
                    currency: "USD".to_string(),
                    currency_symbol: None,
                    decimal_precision: 2,
                },
                status: PaymentOrderStatus::Canceled,
                paid_at: None,
            }],
            test_now(),
        )?;

        let history = load_all_payment_order_history(user_id)?;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].order_id, order_id);
        assert_eq!(history[0].amount.atlos_decimal_amount(), "9.99");
        assert_eq!(history[0].status, PaymentOrderStatus::Canceled);
        Ok(())
    }

    #[test]
    fn local_payment_order_takes_precedence_over_imported_history() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        load_or_create_payment_subject(user_id, test_now())?;

        let order_id = PaymentOrderId::from_str("01JQABCDEF000000000000000E")?;
        upsert_imported_payment_order_history(
            user_id,
            &[NewPaymentOrderHistoryRecord {
                order_id,
                product_tier: ProductTier::Premium,
                amount: PaymentAmount {
                    minor_units: 999,
                    currency: "USD".to_string(),
                    currency_symbol: None,
                    decimal_precision: 2,
                },
                status: PaymentOrderStatus::Canceled,
                paid_at: None,
            }],
            test_now(),
        )?;
        insert_payment_order(
            user_id,
            &NewPaymentOrder {
                order_id,
                order_secret: test_secret(),
                product_tier: ProductTier::Premium,
                amount: PaymentAmount {
                    minor_units: 123,
                    currency: "USD".to_string(),
                    currency_symbol: Some("$".to_string()),
                    decimal_precision: 2,
                },
            },
            test_now(),
        )?;

        let history = load_all_payment_order_history(user_id)?;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].amount.atlos_decimal_amount(), "1.23");
        assert_eq!(history[0].status, PaymentOrderStatus::Pending);
        Ok(())
    }

    #[test]
    fn imported_payment_history_reconciles_canceled_to_paid() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;

        let order_id = PaymentOrderId::from_str("01JQABCDEF000000000000000E")?;
        upsert_imported_payment_order_history(
            user_id,
            &[NewPaymentOrderHistoryRecord {
                order_id,
                product_tier: ProductTier::Premium,
                amount: PaymentAmount {
                    minor_units: 999,
                    currency: "USD".to_string(),
                    currency_symbol: None,
                    decimal_precision: 2,
                },
                status: PaymentOrderStatus::Canceled,
                paid_at: None,
            }],
            test_now(),
        )?;

        assert!(reconcile_imported_payment_order_history_status(
            user_id,
            order_id,
            PaymentOrderStatus::Paid,
            Some(test_now()),
            test_now(),
        )?);
        let history = load_all_payment_order_history(user_id)?;
        assert_eq!(history[0].status, PaymentOrderStatus::Paid);
        assert_eq!(history[0].paid_at, Some(test_now()));
        Ok(())
    }

    fn load_all_token_history_for_test(
        user_id: crate::models::UserId,
    ) -> Result<Vec<PaymentTokenHistoryRecord>, DbError> {
        with_user_db(user_id, |conn| {
            let mut rows = Vec::new();
            let mut stmt = conn
                .prepare(
                    "SELECT token_id, subscription_subject_id, active_token, entitlement_tier, \
                            subscription_valid_until, token_expires_at, token_issued_at, \
                            capability_set_id, capabilities_json, status, status_reason, \
                            first_seen_at, last_seen_at, deactivated_at \
                     FROM payment_token_history ORDER BY first_seen_at ASC",
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error("Failed to query token history", err)
                })?;
            let result = stmt.query_map([], parse_token_history_row).map_err(|err| {
                DbError::from_rusqlite_error("Failed to query token history rows", err)
            })?;
            for row in result {
                rows.push(row.map_err(|err| {
                    DbError::from_rusqlite_error("Failed to parse token history row", err)
                })?);
            }
            Ok(rows)
        })
    }

    #[test]
    fn storing_new_token_supersedes_previous_active() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;

        let claims_a = test_token_claims(&subject);
        let verified_a = test_verified_token("sig_a", &claims_a);
        store_verified_premium_token(user_id, None, &verified_a, None, test_now())?;

        // Second token with different token_id
        let mut claims_b = test_token_claims(&subject);
        claims_b.token_id = TokenId::from_str("01JQABCDEF00000000000000AB")?;
        let verified_b = test_verified_token("sig_b", &claims_b);
        store_verified_premium_token(user_id, None, &verified_b, None, test_now())?;

        // First row should be superseded
        let all = load_all_token_history_for_test(user_id)?;
        assert_eq!(all.len(), 2);
        let old_row = all
            .iter()
            .find(|r| r.token_id == claims_a.token_id)
            .unwrap();
        assert_eq!(old_row.status, TokenHistoryStatus::Superseded);
        assert!(old_row.deactivated_at.is_some());

        // New row is active
        let active = load_active_token_history(user_id)?.unwrap();
        assert_eq!(active.token_id, claims_b.token_id);
        assert_eq!(active.active_token, "sig_b");
        Ok(())
    }

    #[test]
    fn token_store_reports_only_first_activation_transition() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;
        let claims = test_token_claims(&subject);
        let verified = test_verified_token("sig_v1", &claims);

        assert!(store_verified_premium_token_with_activation_transition(
            user_id,
            None,
            &verified,
            None,
            test_now(),
        )?);
        assert!(!store_verified_premium_token_with_activation_transition(
            user_id,
            None,
            &verified,
            None,
            test_now(),
        )?);
        Ok(())
    }

    #[test]
    fn storing_same_token_updates_last_seen_at() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;

        let claims = test_token_claims(&subject);
        let verified = test_verified_token("sig_v1", &claims);
        store_verified_premium_token(user_id, None, &verified, None, test_now())?;
        let first = load_active_token_history(user_id)?;

        let later: DateTime<Utc> = "2026-04-17T12:00:00Z".parse()?;
        store_verified_premium_token(user_id, None, &verified, None, later)?;
        let second = load_active_token_history(user_id)?;

        assert_eq!(
            first.as_ref().unwrap().token_id,
            second.as_ref().unwrap().token_id
        );
        assert!(second.as_ref().unwrap().last_seen_at > first.as_ref().unwrap().last_seen_at);

        // Still only one row
        let all = load_all_token_history_for_test(user_id)?;
        assert_eq!(all.len(), 1);
        Ok(())
    }

    #[test]
    fn clear_preserves_history_row_and_nulls_pointer() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;

        let claims = test_token_claims(&subject);
        let verified = test_verified_token("sig_v1", &claims);
        store_verified_premium_token(user_id, None, &verified, None, test_now())?;

        clear_verified_premium_token(
            user_id,
            TokenHistoryStatus::Revoked,
            Some("central_revoked"),
            "revoked",
            test_now(),
        )?;

        let reloaded = load_payment_subject(user_id)?.unwrap();
        assert!(reloaded.active_token_history_id.is_none());
        assert_eq!(reloaded.last_refresh_status.as_deref(), Some("revoked"));

        let all = load_all_token_history_for_test(user_id)?;
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, TokenHistoryStatus::Revoked);
        assert_eq!(all[0].status_reason.as_deref(), Some("central_revoked"));
        assert_eq!(all[0].active_token, "sig_v1");
        assert!(all[0].deactivated_at.is_some());
        Ok(())
    }

    #[test]
    fn pruning_keeps_active_plus_50_newest_inactive() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;

        // Store one active token
        let claims = test_token_claims(&subject);
        let verified = test_verified_token("sig_active", &claims);
        store_verified_premium_token(user_id, None, &verified, None, test_now())?;

        // Insert 55 inactive rows directly
        with_user_db_mut(user_id, |conn| {
            for i in 0..55u32 {
                let token_id = format!("01JQABCDEF0000000000{i:06}");
                let ts = format_timestamp(test_now());
                conn.execute(
                    "INSERT INTO payment_token_history \
                     (token_id, subscription_subject_id, active_token, entitlement_tier, \
                      subscription_valid_until, token_expires_at, token_issued_at, \
                      capability_set_id, capabilities_json, status, status_reason, \
                      first_seen_at, last_seen_at, deactivated_at) \
                     VALUES (?1, ?2, 'old_sig', 'premium', ?3, ?4, ?5, NULL, NULL, \
                             'inactive', 'test', ?6, ?6, ?6)",
                    rusqlite::params![
                        &token_id,
                        claims.subscription_subject_id.to_storage_value(),
                        format_timestamp(claims.subscription_valid_until),
                        format_timestamp(claims.token_expires_at),
                        format_timestamp(claims.issued_at),
                        &ts,
                    ],
                )
                .map_err(|err| {
                    DbError::from_rusqlite_error("Failed to insert test inactive row", err)
                })?;
            }
            Ok::<(), DbError>(())
        })?;

        // Store another token, which triggers pruning
        let mut claims_2 = test_token_claims(&subject);
        claims_2.token_id = TokenId::from_str("01JQABCDEF00000000000000AA")?;
        let verified_2 = test_verified_token("sig_new_active", &claims_2);
        store_verified_premium_token(user_id, None, &verified_2, None, test_now())?;

        let all = load_all_token_history_for_test(user_id)?;
        let active_count = all
            .iter()
            .filter(|r| r.status == TokenHistoryStatus::Active)
            .count();
        let inactive_count = all
            .iter()
            .filter(|r| r.status != TokenHistoryStatus::Active)
            .count();
        assert_eq!(active_count, 1);
        assert!(
            inactive_count <= 50,
            "should keep at most 50 inactive/superseded rows, got {inactive_count}"
        );
        assert!(
            all.len() <= 51,
            "should have at most 1 active + 50 inactive/superseded rows, got {}",
            all.len()
        );
        Ok(())
    }

    #[test]
    fn inactive_history_row_does_not_grant_access() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;

        let claims = test_token_claims(&subject);
        let verified = test_verified_token("sig_v1", &claims);
        store_verified_premium_token(user_id, None, &verified, None, test_now())?;
        clear_verified_premium_token(
            user_id,
            TokenHistoryStatus::Revoked,
            None,
            "revoked",
            test_now(),
        )?;

        let entitlements =
            crate::payments::entitlements::load_feature_entitlements(user_id, test_now())?;
        assert_eq!(entitlements.tier, EntitlementTier::Free);
        Ok(())
    }

    #[test]
    fn pointer_to_non_active_history_row_does_not_load_or_grant_access()
    -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;

        let claims = test_token_claims(&subject);
        let verified = test_verified_token("sig_v1", &claims);
        store_verified_premium_token(user_id, None, &verified, None, test_now())?;

        with_user_db_mut(user_id, |conn| {
            conn.execute(
                "UPDATE payment_token_history \
                 SET status = 'revoked', status_reason = 'test_non_active_pointer', \
                     deactivated_at = ?1 \
                 WHERE token_id = ?2",
                rusqlite::params![
                    format_timestamp(test_now()),
                    claims.token_id.to_storage_value()
                ],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to mark pointed history row revoked", err)
            })?;
            Ok::<(), DbError>(())
        })?;

        assert!(load_active_token_history(user_id)?.is_none());

        let entitlements =
            crate::payments::entitlements::load_feature_entitlements(user_id, test_now())?;
        assert_eq!(entitlements.tier, EntitlementTier::Free);
        Ok(())
    }

    #[test]
    fn clear_verified_token_rolls_back_history_status_when_subject_update_fails()
    -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;

        let claims = test_token_claims(&subject);
        let verified = test_verified_token("sig_v1", &claims);
        store_verified_premium_token(user_id, None, &verified, None, test_now())?;

        let result = clear_verified_premium_token(
            user_id,
            TokenHistoryStatus::Revoked,
            Some("central_revoked"),
            "not_a_valid_refresh_status",
            test_now(),
        );
        assert!(result.is_err());

        let subject = load_payment_subject(user_id)?.expect("subject should still exist");
        assert_eq!(subject.active_token_history_id, Some(claims.token_id));

        let history = load_active_token_history(user_id)?.expect("active history should remain");
        assert_eq!(history.status, TokenHistoryStatus::Active);
        assert!(history.status_reason.is_none());
        assert!(history.deactivated_at.is_none());
        Ok(())
    }

    #[test]
    fn dangling_pointer_does_not_grant_access() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;

        let claims = test_token_claims(&subject);
        let verified = test_verified_token("sig_v1", &claims);
        store_verified_premium_token(user_id, None, &verified, None, test_now())?;

        // Manually delete the history row, leaving a dangling pointer
        with_user_db_mut(user_id, |conn| {
            conn.execute(
                "DELETE FROM payment_token_history WHERE token_id = ?1",
                [claims.token_id.to_storage_value()],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to delete token history row", err)
            })?;
            Ok::<(), DbError>(())
        })?;

        assert!(load_active_token_history(user_id)?.is_none());

        let entitlements =
            crate::payments::entitlements::load_feature_entitlements(user_id, test_now())?;
        assert_eq!(entitlements.tier, EntitlementTier::Free);
        Ok(())
    }

    #[test]
    fn history_row_without_pointer_does_not_grant_access() -> Result<(), Box<dyn Error>> {
        let _guard = db::acquire_test_runtime()?;
        let user_id = db::unique_user_id();
        db::initialize_user_db_for_test(user_id)?;
        let subject = load_or_create_payment_subject(user_id, test_now())?;

        // Insert an inactive history row directly (no pointer set)
        with_user_db_mut(user_id, |conn| {
            conn.execute(
                "INSERT INTO payment_token_history \
                 (token_id, subscription_subject_id, active_token, entitlement_tier, \
                  subscription_valid_until, token_expires_at, token_issued_at, \
                  capability_set_id, capabilities_json, status, status_reason, \
                  first_seen_at, last_seen_at, deactivated_at) \
                 VALUES (?1, ?2, 'rogue_sig', 'premium', '2027-04-16T12:00:00Z', \
                         '2026-04-23T12:00:00Z', '2026-04-16T12:00:00Z', NULL, NULL, \
                         'inactive', 'test', '2026-04-16T12:00:00Z', '2026-04-16T12:00:00Z', '2026-04-16T12:00:00Z')",
                rusqlite::params![
                    "01JQABCDEF00000000000000BB",
                    subject.entitlement_holder_id.to_storage_value(),
                ],
            )
            .map_err(|err| {
                DbError::from_rusqlite_error("Failed to insert rogue history row", err)
            })?;
            Ok::<(), DbError>(())
        })?;

        let entitlements =
            crate::payments::entitlements::load_feature_entitlements(user_id, test_now())?;
        assert_eq!(entitlements.tier, EntitlementTier::Free);
        Ok(())
    }
}
