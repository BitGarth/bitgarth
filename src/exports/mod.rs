#[cfg(any(
    test,
    all(
        feature = "server",
        any(
            all(not(test), not(feature = "desktop")),
            all(test, not(bitgarth_db_unit_only))
        )
    )
))]
pub(crate) mod hledger;
