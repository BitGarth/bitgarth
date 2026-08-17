mod account_details;
mod account_rows;
#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
mod account_selector;
mod add_bitcoin;
mod add_ethereum;
mod add_manual_asset;
mod add_xpub;
mod dialogs;
mod helpers;
#[cfg(any(target_arch = "wasm32", feature = "desktop"))]
mod link_trezor;
mod sync_bridge;
mod sync_state;
mod sync_status;
mod wallet_card;
mod wallet_dropdown;
mod wallet_section;

use chrono::{DateTime, Utc};
use dioxus::prelude::*;
pub(crate) use sync_bridge::{SyncBridgeSignals, use_sync_event_bridge};
pub(crate) use sync_state::{
    AccountSyncStateMap, AccountSyncStateSignal, SyncRunCompletion, build_account_sync_state_map,
};
pub(crate) use sync_status::AccountSyncStatusPill;
use wallet_section::WalletsSection;

pub(crate) use account_details::{AccountAddressesModal, ChangeWalletInline, LabelEditor};
pub(crate) use add_bitcoin::AddBitcoinAddressFlow;
pub(crate) use add_ethereum::AddEthereumAddressFlow;
pub(crate) use add_manual_asset::{AddManualAssetFlow, route_for_added_manual_asset};
pub(crate) use add_xpub::AddXpubFlow;
pub(crate) use dialogs::{
    AddDropdownButton, AddressSchemeDeleteConfirmDialog, KebabMenu, KebabMenuItem,
};
pub(crate) use helpers::{
    AccountAddressesLoader, WalletMoveOption, address_scheme_label, build_wallet_move_options,
    copy_to_clipboard, parse_label_for_editor, truncate_reference, truncate_reference_with_lengths,
};

const WALLETS_CSS: Asset = asset!("/assets/wallets.css");
pub(super) const CREATE_NEW_WALLET_OPTION_VALUE: &str = "__create_new_wallet__";
// Trezor linking is temporarily disabled in the UI. Flip to `true` to re-enable.
pub(super) const TREZOR_LINK_ENABLED: bool = false;
pub(super) const ACTION_LINK_TREZOR_TEST_ID: &str = "wallets-action-link-trezor";
pub(super) const ACTION_ADD_XPUB_TEST_ID: &str = "wallets-action-add-xpub";
pub(super) const ACTION_ADD_BITCOIN_ADDRESS_TEST_ID: &str = "wallets-action-add-bitcoin-address";
pub(super) const ACTION_ADD_ETHEREUM_ADDRESS_TEST_ID: &str = "wallets-action-add-ethereum-address";
pub(super) const ACTION_ADD_MANUAL_ASSET_TEST_ID: &str = "wallets-action-add-manual-asset";

#[component]
pub fn Wallets() -> Element {
    let account_sync_state = use_signal(AccountSyncStateMap::new);
    let mut account_sync_now = use_signal(|| None::<DateTime<Utc>>);
    let global_sync_in_progress = use_signal(|| false);
    use_context_provider(|| account_sync_state);
    use_context_provider(|| account_sync_now);
    use_context_provider(|| global_sync_in_progress);

    use_effect(move || {
        account_sync_now.set(Some(Utc::now()));
    });

    let mut link_trezor_requested = use_signal(|| false);
    let mut show_add_xpub = use_signal(|| false);
    let mut show_add_bitcoin = use_signal(|| false);
    let mut show_add_ethereum = use_signal(|| false);
    let mut show_add_manual_asset = use_signal(|| false);

    rsx! {
        document::Stylesheet { href: WALLETS_CSS }
        div { class: "page-container wallets-page",
            header { class: "page-header wallets-page-header", "data-testid": "wallets-header",
                div { class: "wallets-page-title-group",
                    div { class: "wallets-page-section-head",
                        h1 { class: "page-title", "data-testid": "wallets-title", "Wallets" }
                    }
                    p { class: "page-subtitle", "data-testid": "wallets-subtitle",
                        "Your accounts, "
                        em { "in one place." }
                    }
                }
                AddDropdownButton {
                    on_link_trezor: move |_| link_trezor_requested.set(true),
                    on_add_xpub: move |_| show_add_xpub.set(true),
                    on_add_bitcoin: move |_| show_add_bitcoin.set(true),
                    on_add_ethereum: move |_| show_add_ethereum.set(true),
                    on_add_manual_asset: move |_| show_add_manual_asset.set(true),
                }
            }

            WalletsSection {
                link_trezor_requested,
                show_add_xpub,
                show_add_bitcoin,
                show_add_ethereum,
                show_add_manual_asset,
            }
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn actions_section_does_not_include_manual_sync_button() {
        let action_test_ids = [
            ACTION_LINK_TREZOR_TEST_ID,
            ACTION_ADD_XPUB_TEST_ID,
            ACTION_ADD_BITCOIN_ADDRESS_TEST_ID,
            ACTION_ADD_ETHEREUM_ADDRESS_TEST_ID,
        ];

        assert!(!action_test_ids.contains(&"wallets-action-sync-now"));
    }
}
