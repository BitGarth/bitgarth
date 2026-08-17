use super::helpers::{ActiveFilters, status_label};
use crate::transactions::ChainTransactionStatus;
use dioxus::prelude::*;

pub(super) const ALL_STATUSES: [ChainTransactionStatus; 4] = [
    ChainTransactionStatus::Pending,
    ChainTransactionStatus::Confirmed,
    ChainTransactionStatus::Dropped,
    ChainTransactionStatus::Failed,
];

#[component]
pub(super) fn TransactionStatusFilterRow(
    filters: ActiveFilters,
    loading: bool,
    on_filters_change: EventHandler<ActiveFilters>,
) -> Element {
    let all_active = filters.is_all();

    rsx! {
        div { class: "tx-toolbar-secondary-row",
            div { class: "tx-toolbar-secondary-group",
                span { class: "tx-filter-label", "Status" }
                div { class: "tx-filter-pills",
                    button {
                        class: if all_active { "tx-filter-pill all active" } else { "tx-filter-pill all" },
                        r#type: "button",
                        disabled: loading,
                        "aria-pressed": "{all_active}",
                        onclick: move |_| on_filters_change.call(ActiveFilters::default()),
                        "All"
                    }
                    span { class: "tx-filter-divider", aria_hidden: "true" }
                    for status in ALL_STATUSES {
                        {
                            let active = filters.is_status_selected(status);
                            let label = status_label(status);
                            let filters_clone = filters.clone();
                            rsx! {
                                button {
                                    key: "{label}",
                                    class: if active { "tx-filter-pill active" } else { "tx-filter-pill" },
                                    r#type: "button",
                                    disabled: loading,
                                    "aria-pressed": "{active}",
                                    onclick: move |_| {
                                        on_filters_change.call(filters_clone.with_status_toggled(status));
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
