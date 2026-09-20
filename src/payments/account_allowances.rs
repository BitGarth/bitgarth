pub(crate) const SUPPORTED_ACCOUNT_HARD_CAP: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AccountAllowances {
    balance_sync: u16,
    transaction_history_sync: u16,
    manual: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InvalidAccountAllowances {
    TransactionsExceedBalances,
    ExceedsStorageSafeguard,
}

impl AccountAllowances {
    pub(crate) fn try_new(
        balance_sync: u16,
        transaction_history_sync: u16,
        manual: u16,
    ) -> Result<Self, InvalidAccountAllowances> {
        if transaction_history_sync > balance_sync {
            return Err(InvalidAccountAllowances::TransactionsExceedBalances);
        }
        if u32::from(balance_sync) + u32::from(manual) > SUPPORTED_ACCOUNT_HARD_CAP as u32 {
            return Err(InvalidAccountAllowances::ExceedsStorageSafeguard);
        }
        Ok(Self {
            balance_sync,
            transaction_history_sync,
            manual,
        })
    }

    pub(crate) const fn balance_sync(self) -> u16 {
        self.balance_sync
    }

    pub(crate) const fn transaction_history_sync(self) -> u16 {
        self.transaction_history_sync
    }

    #[cfg(feature = "server")]
    pub(crate) const fn manual(self) -> u16 {
        self.manual
    }
}

#[cfg(test)]
mod tests {
    use super::AccountAllowances;

    #[test]
    fn validates_account_allowances() {
        assert!(AccountAllowances::try_new(50, 3, 1000).is_ok());
        assert!(AccountAllowances::try_new(0, 0, 0).is_ok());
        assert!(AccountAllowances::try_new(3, 4, 0).is_err());
        assert!(AccountAllowances::try_new(4000, 3, 1000).is_ok());
        assert!(AccountAllowances::try_new(4001, 3, 1000).is_err());
        assert!(AccountAllowances::try_new(u16::MAX, 0, u16::MAX).is_err());
    }
}
