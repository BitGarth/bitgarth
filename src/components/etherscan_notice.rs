use super::ExternalLinkIcon;
use dioxus::prelude::*;

/// URL where users obtain a free Etherscan API key.
const ETHERSCAN_API_KEY_URL: &str = "https://etherscan.io/apidashboard";
/// Internal deep-link to the Settings page, Digital Assets tab.
const SETTINGS_DIGITAL_ASSETS_HREF: &str = "/settings?section=digital-assets";

/// Notice shown wherever an Ethereum account is (or is about to be) present but
/// no Etherscan API key is configured. Uses plain anchors so it renders without
/// a Router context and stays unit-testable.
#[component]
pub fn EtherscanApiKeyNotice() -> Element {
    rsx! {
        div {
            class: "alert alert-info",
            "data-testid": "etherscan-api-key-notice",
            "Ethereum transaction fetching requires an Etherscan API key. Add one in "
            a { href: SETTINGS_DIGITAL_ASSETS_HREF, title: "Go to Digital Assets settings", "Settings" }
            " — get a free key from "
            a {
                href: ETHERSCAN_API_KEY_URL,
                target: "_blank",
                rel: "noopener noreferrer",
                title: "Etherscan API dashboard opens in a new tab",
                "Etherscan"
                ExternalLinkIcon {}
            }
            "."
        }
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn render() -> String {
        let mut dom = VirtualDom::new(EtherscanApiKeyNotice);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn notice_links_to_settings_digital_assets_and_etherscan() {
        let html = render();
        assert!(html.contains(r#"data-testid="etherscan-api-key-notice""#));
        assert!(html.contains(&format!(r#"href="{SETTINGS_DIGITAL_ASSETS_HREF}""#)));
        assert!(html.contains(&format!(r#"href="{ETHERSCAN_API_KEY_URL}""#)));
        assert!(html.contains(r#"target="_blank""#));
        assert!(html.contains(r#"rel="noopener noreferrer""#));
        // ExternalLinkIcon SVG is rendered (stable polyline marker).
        assert!(html.contains(r#"points="15 3 21 3 21 9""#));
    }
}
