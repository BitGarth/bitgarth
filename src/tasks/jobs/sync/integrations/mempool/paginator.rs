use crate::integrations::mempool::MempoolAddressTransaction;

pub(crate) fn confirmed_mempool_tx_count_in_page(page: &[MempoolAddressTransaction]) -> usize {
    page.iter().filter(|tx| tx.status.confirmed).count()
}

pub(crate) fn last_confirmed_txid_in_page(page: &[MempoolAddressTransaction]) -> Option<String> {
    page.iter()
        .rev()
        .find(|tx| tx.status.confirmed)
        .map(|tx| tx.txid.clone())
}
