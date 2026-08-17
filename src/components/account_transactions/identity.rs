use dioxus::prelude::*;

use crate::backend::AccountReferenceKind;
use crate::wallets::{AddressScheme, Network, SyncedAssetId};

use super::super::wallets::{address_scheme_label, truncate_reference};
use super::native_table::CopyIconButton;

/// Short type text for the collapsed strip: the scheme for Bitcoin
/// accounts, the asset name for Ethereum (its internal "Standard" scheme
/// carries no user meaning).
pub(super) fn identity_summary_type(asset: SyncedAssetId, address_scheme: AddressScheme) -> String {
    match asset {
        SyncedAssetId::Ethereum => asset.display_name().to_string(),
        SyncedAssetId::Bitcoin => address_scheme_label(address_scheme).to_string(),
    }
}

/// Full "Type" row value, e.g. "Bitcoin xpub — Native SegWit".
pub(super) fn native_type_label(
    asset: SyncedAssetId,
    reference_kind: AccountReferenceKind,
    address_scheme: AddressScheme,
) -> String {
    match asset {
        SyncedAssetId::Ethereum => format!("{} address", asset.display_name()),
        SyncedAssetId::Bitcoin => {
            let noun = match reference_kind {
                AccountReferenceKind::ExtendedPubkey => "xpub",
                AccountReferenceKind::SingleAddress => "address",
            };
            format!(
                "{} {noun} \u{2014} {}",
                asset.display_name(),
                address_scheme_label(address_scheme)
            )
        }
    }
}

pub(super) fn manual_type_label() -> &'static str {
    "Manual asset"
}

pub(super) fn manual_unit_display(unit_code: &str, symbol: Option<&str>) -> String {
    match symbol {
        Some(symbol) => format!("{unit_code} ({symbol})"),
        None => unit_code.to_string(),
    }
}

pub(super) fn precision_display(decimal_precision: u8) -> String {
    if decimal_precision == 1 {
        "1 decimal place".to_string()
    } else {
        format!("{decimal_precision} decimal places")
    }
}

pub(super) fn network_display(asset: SyncedAssetId, network: Network) -> String {
    format!("{} {}", asset.display_name(), network.as_str())
}

#[derive(Clone, PartialEq)]
pub(super) enum AccountIdentity {
    Native {
        asset: SyncedAssetId,
        network: Network,
        reference_kind: AccountReferenceKind,
        reference: String,
        address_scheme: AddressScheme,
    },
    Manual {
        unit_code: String,
        symbol: Option<String>,
        decimal_precision: u8,
        asset_name: Option<String>,
        network_name: Option<String>,
    },
}

#[component]
pub(super) fn AccountIdentitySection(
    identity: AccountIdentity,
    on_view_addresses: EventHandler<()>,
) -> Element {
    let mut expanded = use_signal(|| false);
    let chevron = if expanded() { "\u{2303}" } else { "\u{2304}" };

    let (summary_type, summary_reference) = match &identity {
        AccountIdentity::Native {
            asset,
            address_scheme,
            reference,
            ..
        } => (
            identity_summary_type(*asset, *address_scheme),
            truncate_reference(reference),
        ),
        AccountIdentity::Manual { unit_code, .. } => {
            ("Manual asset".to_string(), unit_code.clone())
        }
    };

    rsx! {
        section { class: "account-identity-section", "data-testid": "account-identity-section",
            button {
                class: "account-identity-strip",
                r#type: "button",
                "data-testid": "account-identity-strip",
                "aria-expanded": "{expanded()}",
                onclick: move |_| expanded.toggle(),
                span { class: "account-identity-strip-left",
                    span { class: "account-identity-title", "§ Account" }
                    span { class: "account-identity-summary-type", "{summary_type}" }
                    if !summary_reference.is_empty() {
                        span { class: "account-identity-summary-sep", "·" }
                        span { class: "account-identity-summary-ref", "{summary_reference}" }
                    }
                }
                span { class: "account-identity-chevron", "{chevron}" }
            }
            if expanded() {
                div { class: "account-identity-panel", "data-testid": "account-identity-panel",
                    match &identity {
                        AccountIdentity::Native {
                            asset,
                            network,
                            reference_kind,
                            reference,
                            address_scheme,
                        } => {
                            let type_label =
                                native_type_label(*asset, *reference_kind, *address_scheme);
                            let reference_label = match reference_kind {
                                AccountReferenceKind::ExtendedPubkey => "Xpub",
                                AccountReferenceKind::SingleAddress => "Address",
                            };
                            let network_label = network_display(*asset, *network);
                            let copy_value = reference.clone();
                            let copy_aria = format!("Copy {reference_label}");
                            rsx! {
                                dl { class: "account-identity-rows",
                                    dt { "Type" }
                                    dd { "data-testid": "account-identity-type", "{type_label}" }
                                    if !reference.is_empty() {
                                        dt { "{reference_label}" }
                                        dd { class: "account-identity-reference",
                                            span {
                                                class: "account-identity-mono",
                                                "data-testid": "account-identity-reference",
                                                "{reference}"
                                            }
                                            CopyIconButton { value: copy_value, aria_label: copy_aria }
                                        }
                                    }
                                    dt { "Network" }
                                    dd { "{network_label}" }
                                    dt { "Addresses" }
                                    dd {
                                        button {
                                            class: "account-identity-addresses-btn",
                                            r#type: "button",
                                            "data-testid": "account-identity-view-addresses",
                                            onclick: move |_| on_view_addresses.call(()),
                                            "View addresses \u{2192}"
                                        }
                                    }
                                }
                            }
                        }
                        AccountIdentity::Manual {
                            unit_code,
                            symbol,
                            decimal_precision,
                            asset_name,
                            network_name,
                        } => {
                            let type_label = manual_type_label();
                            let unit_label = manual_unit_display(unit_code, symbol.as_deref());
                            let precision_label = precision_display(*decimal_precision);
                            rsx! {
                                dl { class: "account-identity-rows",
                                    dt { "Type" }
                                    dd { "data-testid": "account-identity-type", "{type_label}" }
                                    dt { "Unit" }
                                    dd { class: "account-identity-mono", "{unit_label}" }
                                    dt { "Precision" }
                                    dd { "{precision_label}" }
                                    if let Some(asset_name) = asset_name {
                                        dt { "Asset" }
                                        dd { "{asset_name}" }
                                    }
                                    if let Some(network_name) = network_name {
                                        dt { "Network" }
                                        dd { "{network_name}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    #[test]
    fn native_type_label_formats_bitcoin_xpub() {
        assert_eq!(
            native_type_label(
                SyncedAssetId::Bitcoin,
                AccountReferenceKind::ExtendedPubkey,
                AddressScheme::NativeSegwit,
            ),
            "Bitcoin xpub \u{2014} Native SegWit"
        );
    }

    #[test]
    fn native_type_label_formats_bitcoin_single_address() {
        assert_eq!(
            native_type_label(
                SyncedAssetId::Bitcoin,
                AccountReferenceKind::SingleAddress,
                AddressScheme::Legacy,
            ),
            "Bitcoin address \u{2014} Legacy"
        );
    }

    #[test]
    fn native_type_label_suppresses_scheme_for_ethereum() {
        assert_eq!(
            native_type_label(
                SyncedAssetId::Ethereum,
                AccountReferenceKind::SingleAddress,
                AddressScheme::Standard,
            ),
            "Ethereum address"
        );
    }

    #[test]
    fn identity_summary_type_uses_scheme_for_bitcoin_and_asset_for_ethereum() {
        assert_eq!(
            identity_summary_type(SyncedAssetId::Bitcoin, AddressScheme::Legacy),
            "Legacy"
        );
        assert_eq!(
            identity_summary_type(SyncedAssetId::Ethereum, AddressScheme::Standard),
            "Ethereum"
        );
    }

    #[test]
    fn manual_type_label_describes_manual_assets() {
        assert_eq!(manual_type_label(), "Manual asset");
    }

    #[test]
    fn manual_unit_display_appends_symbol() {
        assert_eq!(manual_unit_display("XMR", None), "XMR");
        assert_eq!(manual_unit_display("GOLD", Some("GLD")), "GOLD (GLD)");
    }

    #[test]
    fn precision_display_handles_singular() {
        assert_eq!(precision_display(0), "0 decimal places");
        assert_eq!(precision_display(1), "1 decimal place");
        assert_eq!(precision_display(8), "8 decimal places");
    }

    #[test]
    fn network_display_combines_asset_and_network() {
        assert_eq!(
            network_display(SyncedAssetId::Bitcoin, Network::Mainnet),
            "Bitcoin mainnet"
        );
    }
}
