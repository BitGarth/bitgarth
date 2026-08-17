pub(crate) mod executor;
pub(crate) mod planner;
pub(crate) mod work_selection;

pub(crate) use executor::run_price_history_reconciliation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PriceHistoryReconciliationReason {
    Login,
    PriceFetchingEnabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PriceHistoryReconciliationParams {
    pub(crate) reason: PriceHistoryReconciliationReason,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconciliation_params_carry_reason() {
        assert_eq!(
            PriceHistoryReconciliationParams {
                reason: PriceHistoryReconciliationReason::Login,
            }
            .reason,
            PriceHistoryReconciliationReason::Login
        );
        assert_eq!(
            PriceHistoryReconciliationParams {
                reason: PriceHistoryReconciliationReason::PriceFetchingEnabled,
            }
            .reason,
            PriceHistoryReconciliationReason::PriceFetchingEnabled
        );
    }
}
