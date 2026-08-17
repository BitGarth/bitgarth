use dioxus::prelude::*;

/// Operator-supplied notice html, provided by `App` and read by layouts.
/// `None` means no banner; the consumer renders nothing in that case.
pub(crate) type InstanceNoticeState = Signal<Option<String>>;

#[component]
pub fn InstanceNoticeBanner(html: Option<String>) -> Element {
    let Some(html) = html else {
        return rsx! {};
    };
    rsx! {
        div {
            class: "instance-notice",
            role: "status",
            dangerous_inner_html: "{html}",
        }
    }
}

#[cfg(all(test, feature = "server", not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn render(html: Option<String>) -> String {
        let mut dom = VirtualDom::new_with_props(ParametricBanner, ParametricBannerProps { html });
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[component]
    fn ParametricBanner(html: Option<String>) -> Element {
        rsx! { InstanceNoticeBanner { html } }
    }

    #[test]
    fn banner_absent_when_html_is_none() {
        let rendered = render(None);
        assert!(!rendered.contains("instance-notice"));
        assert!(!rendered.contains(r#"role="status""#));
    }

    #[test]
    fn banner_present_when_html_is_some() {
        let rendered = render(Some("<p>hello</p>".to_string()));
        assert!(rendered.contains(r#"class="instance-notice""#));
        assert!(rendered.contains(r#"role="status""#));
        assert!(rendered.contains("<p>hello</p>"));
    }

    #[test]
    fn banner_renders_link_html_unescaped() {
        let html = r#"<p><a href="https://bitgarth.app/" rel="noopener noreferrer" target="_blank">bitgarth.app</a></p>"#;
        let rendered = render(Some(html.to_string()));
        assert!(rendered.contains(r#"href="https://bitgarth.app/""#));
        assert!(rendered.contains(r#"target="_blank""#));
        assert!(rendered.contains(r#"rel="noopener noreferrer""#));
    }

    /// Regression: the production wiring is App provides `InstanceNoticeState`
    /// via context and the layout reads it, rendering the banner inside its
    /// `<main>`. If a consumer drops the context lookup or wraps the banner
    /// outside the main content, the layout fix from this commit silently
    /// regresses. This exercises the provider/consumer contract end-to-end.
    fn render_with_context(initial: Option<String>) -> String {
        #[component]
        fn Layout() -> Element {
            let state = try_consume_context::<InstanceNoticeState>().and_then(|s| s.read().clone());
            rsx! {
                main {
                    InstanceNoticeBanner { html: state }
                }
            }
        }

        #[component]
        fn Wrapper(initial: Option<String>) -> Element {
            let state: InstanceNoticeState = use_signal(|| initial.clone());
            use_context_provider(|| state);
            rsx! { Layout {} }
        }

        let mut dom = VirtualDom::new_with_props(Wrapper, WrapperProps { initial });
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn banner_renders_inside_main_when_context_provides_html() {
        let rendered = render_with_context(Some("<p>notice</p>".to_string()));
        let main_open = rendered.find("<main").expect("layout renders <main>");
        let main_close = rendered.find("</main>").expect("layout closes <main>");
        let banner = rendered
            .find(r#"class="instance-notice""#)
            .expect("banner renders when context has html");
        assert!(
            main_open < banner && banner < main_close,
            "banner must render inside <main>, got: {rendered}"
        );
    }

    #[test]
    fn banner_absent_when_context_value_is_none() {
        let rendered = render_with_context(None);
        assert!(!rendered.contains("instance-notice"));
    }

    #[test]
    fn banner_absent_when_context_is_missing() {
        // Layout must not panic if it ever renders without a provider above it.
        #[component]
        fn LayoutOnly() -> Element {
            let state = try_consume_context::<InstanceNoticeState>().and_then(|s| s.read().clone());
            rsx! { main { InstanceNoticeBanner { html: state } } }
        }
        let mut dom = VirtualDom::new(LayoutOnly);
        dom.rebuild_in_place();
        let rendered = dioxus_ssr::render(&dom);
        assert!(!rendered.contains("instance-notice"));
    }
}
