//! Botanical Ledger style guide — visual reference for Phase 2+
//! page migrations. Gated on the `dev-config` Cargo feature so it
//! does not ship in production binaries.
//!
//! Renders the palette, type ramp, base elements, button variants,
//! link states, a `.code-card`, and form-input states on one
//! scrolling page. Compare side-by-side with the landing page when
//! migrating a route.

use dioxus::prelude::*;

/// In non-dev-config builds the `/style-guide` route resolves to a
/// 404-equivalent — the gallery itself is compiled out.
#[cfg(not(feature = "dev-config"))]
#[component]
pub(crate) fn StyleGuide() -> Element {
    rsx! {
        crate::components::NotFound { segments: vec!["style-guide".to_string()] }
    }
}

#[cfg(feature = "dev-config")]
mod gallery {
    use super::*;

    /// Token swatches: (CSS variable, label).
    const PALETTE_TOKENS: &[(&str, &str)] = &[
        ("--paper", "Paper"),
        ("--paper-deep", "Paper deep"),
        ("--paper-edge", "Paper edge"),
        ("--ink", "Ink"),
        ("--ink-deeper", "Ink deeper"),
        ("--ink-soft", "Ink soft"),
        ("--paper-on-ink", "Paper on ink"),
        ("--moss", "Moss"),
        ("--moss-bright", "Moss bright"),
        ("--copper", "Copper"),
        ("--copper-soft", "Copper soft"),
        ("--sage", "Sage"),
        ("--positive", "Positive"),
        ("--positive-soft", "Positive soft"),
        ("--caution", "Caution"),
        ("--caution-soft", "Caution soft"),
        ("--danger", "Danger"),
        ("--danger-soft", "Danger soft"),
        ("--info", "Info"),
    ];

    /// Type-ramp entries: (CSS variable for size, label, example text).
    const TYPE_RAMP: &[(&str, &str, &str)] = &[
        ("--fs-display-xl", "display-xl", "Display XL"),
        ("--fs-display-lg", "display-lg", "Display LG"),
        ("--fs-display-md", "display-md", "Display MD"),
        ("--fs-display-sm", "display-sm", "Display SM"),
        (
            "--fs-lede",
            "lede",
            "A lede paragraph sits between a headline and the body.",
        ),
        (
            "--fs-body",
            "body",
            "Body — Instrument Sans at the 17 px base.",
        ),
        (
            "--fs-body-sm",
            "body-sm",
            "Body-sm for nav and secondary copy.",
        ),
        ("--fs-caption", "caption", "Caption for footnotes."),
        ("--fs-eyebrow", "eyebrow", "Eyebrow for small-caps labels."),
    ];

    #[component]
    pub(crate) fn StyleGuide() -> Element {
        rsx! {
            main {
                style: "max-width: 76rem; margin: 0 auto; padding: 3rem clamp(1.25rem, 4vw, 2.5rem); display: flex; flex-direction: column; gap: 3rem;",

                header {
                    h1 {
                        style: "font-size: var(--fs-display-lg); margin: 0;",
                        "Botanical Ledger "
                        em { "style guide." }
                    }
                    p {
                        style: "color: var(--ink-soft); font-size: var(--fs-lede); max-width: 36rem; margin-top: 0.75rem;",
                        "Visual contract for Phase 2+ page migrations. Open the landing page side-by-side and verify every section matches."
                    }
                }

                { palette_section() }
                { type_ramp_section() }
                { headings_section() }
                { body_text_section() }
                { buttons_section() }
                { link_section() }
                { code_section() }
                { form_section() }
            }
        }
    }

    fn palette_section() -> Element {
        rsx! {
            section {
                { section_heading("01", "Palette") }
                div {
                    style: "display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: 1rem;",
                    for (token, label) in PALETTE_TOKENS.iter() {
                        article {
                            style: "border: 1px solid var(--paper-edge); border-radius: var(--radius-input); overflow: hidden; background: var(--paper);",
                            div {
                                style: "height: 64px; background: var({token});",
                            }
                            div {
                                style: "padding: 0.6rem 0.8rem;",
                                div { style: "font-weight: 500; font-size: var(--fs-body-sm);", "{label}" }
                                code { style: "font-size: 0.78rem; color: var(--sage);", "var({token})" }
                            }
                        }
                    }
                }
            }
        }
    }

    fn type_ramp_section() -> Element {
        rsx! {
            section {
                { section_heading("02", "Type ramp") }
                div {
                    style: "display: flex; flex-direction: column; gap: 1rem; border-top: 1px solid var(--paper-edge); padding-top: 1.5rem;",
                    for (token, label, sample) in TYPE_RAMP.iter() {
                        div {
                            style: "display: grid; grid-template-columns: 8rem 1fr; gap: 1.5rem; align-items: baseline;",
                            code {
                                style: "font-size: 0.78rem; color: var(--sage); padding-top: 0.4rem;",
                                "{label}"
                            }
                            div {
                                style: "font-size: var({token}); line-height: 1.2; font-family: var(--display); font-weight: 500;",
                                "{sample}"
                            }
                        }
                    }
                }
            }
        }
    }

    fn headings_section() -> Element {
        rsx! {
            section {
                { section_heading("03", "Headings") }
                div {
                    style: "display: flex; flex-direction: column; gap: 1.2rem; border-top: 1px solid var(--paper-edge); padding-top: 1.5rem;",
                    h1 { "A headline, " em { "in a quiet garden." } }
                    h2 { "Section headline, " em { "italic for meaning." } }
                    h3 { "Subhead, " em { "Fraunces italic." } }
                    h4 { "A nested heading." }
                }
            }
        }
    }

    fn body_text_section() -> Element {
        rsx! {
            section {
                { section_heading("04", "Body & lede") }
                div {
                    style: "max-width: var(--measure); border-top: 1px solid var(--paper-edge); padding-top: 1.5rem;",
                    p {
                        style: "font-size: var(--fs-lede); color: var(--ink-soft); margin-bottom: 1.4rem;",
                        "BitGarth downloads your Bitcoin and Ethereum transactions into a local, encrypted database — then exports them as plain-text accounting files."
                    }
                    p {
                        style: "font-size: var(--fs-body); color: var(--ink); margin-bottom: 1em;",
                        "Body copy is Instrument Sans at the 17 px base. Italic ", em { "Fraunces" }, " carries emphasis sparingly, only on phrases that "
                        em { "mean something." }
                    }
                    p {
                        style: "font-size: var(--fs-caption); color: var(--sage); margin: 0;",
                        "Footnotes and fine print sit in sage on parchment."
                    }
                }
            }
        }
    }

    fn buttons_section() -> Element {
        rsx! {
            section {
                { section_heading("05", "Buttons") }
                div {
                    style: "display: flex; flex-wrap: wrap; gap: 0.85rem; align-items: center; border-top: 1px solid var(--paper-edge); padding-top: 1.5rem;",
                    button { class: "btn", "Primary action" }
                    button { class: "btn ghost", "Ghost / secondary" }
                    button { class: "btn", disabled: true, "Disabled" }
                    button { class: "btn btn-danger", "Destructive" }
                }
                div {
                    style: "margin-top: 1.5rem; padding: 1.5rem; background: var(--ink); border-radius: var(--radius-code); display: flex; gap: 0.85rem;",
                    button {
                        class: "btn",
                        style: "background: var(--copper); border-color: var(--copper);",
                        "Inverted primary"
                    }
                    button {
                        class: "btn ghost",
                        style: "color: var(--paper-on-ink); border-color: color-mix(in srgb, var(--paper) 30%, transparent);",
                        "Inverted ghost"
                    }
                }
            }
        }
    }

    fn link_section() -> Element {
        rsx! {
            section {
                { section_heading("06", "Links") }
                p {
                    style: "border-top: 1px solid var(--paper-edge); padding-top: 1.5rem;",
                    "An "
                    a { href: "#", "inline link" }
                    " grows its underline on hover. External links open in a new tab and carry "
                    code { "rel=\"noopener noreferrer\"" }
                    "."
                }
            }
        }
    }

    fn code_section() -> Element {
        rsx! {
            section {
                { section_heading("07", "Code") }
                p {
                    style: "border-top: 1px solid var(--paper-edge); padding-top: 1.5rem;",
                    "Inline code like "
                    code { "docker pull bitgarth/bitgarth" }
                    " sits on a quiet ink wash. Default "
                    code { "<pre>" }
                    " is the lightweight on-paper panel:"
                }
                pre { "let garth = \"a clearing in the woods\";\nlet noise = SvgNoise::default();\n" }
                p {
                    style: "margin-top: 1.5rem;",
                    "Opt in to "
                    code { ".code-card" }
                    " for the inverted, shell-styled block:"
                }
                div {
                    class: "code-card",
                    pre {
                        span { class: "tok-cmd", "docker" }
                        " "
                        span { class: "tok-cmd", "run" }
                        " "
                        span { class: "tok-flag", "-d" }
                        " "
                        span { class: "tok-flag", "-p" }
                        " "
                        span { class: "tok-str", "8080:8080" }
                        " "
                        span { class: "tok-img", "bitgarth/bitgarth:latest" }
                    }
                }
            }
        }
    }

    fn form_section() -> Element {
        rsx! {
            section {
                { section_heading("08", "Forms") }
                div {
                    style: "display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 1.2rem; border-top: 1px solid var(--paper-edge); padding-top: 1.5rem; max-width: 48rem;",
                    div { class: "form-group",
                        label { class: "form-label", "Default" }
                        input { class: "form-input", r#type: "text", placeholder: "BitGarth address" }
                        p { class: "form-help-text", "Helper text sits beneath in sage." }
                    }
                    div { class: "form-group",
                        label { class: "form-label", "Disabled" }
                        input { class: "form-input", r#type: "text", value: "read-only", disabled: true }
                    }
                    div { class: "form-group",
                        label { class: "form-label", "Error" }
                        input {
                            class: "form-input input-error",
                            r#type: "text",
                            value: "bc1qinvalid",
                        }
                        p { class: "form-error", "That doesn't look like a valid Bitcoin address." }
                    }
                    div { class: "form-group",
                        label { class: "form-label", "Textarea" }
                        textarea {
                            class: "form-input",
                            rows: 3,
                            placeholder: "Notes about this wallet…",
                        }
                    }
                    div { class: "form-group",
                        label { class: "form-label", "Selector" }
                        select { class: "selector",
                            option { "USD" }
                            option { "EUR" }
                            option { "GBP" }
                        }
                    }
                }
            }
        }
    }

    fn section_heading(num: &str, title: &str) -> Element {
        rsx! {
            div {
                style: "display: flex; align-items: baseline; gap: 1rem; margin-bottom: 0.75rem;",
                span {
                    style: "font-family: var(--display); font-style: italic; color: var(--copper); font-size: 1rem; letter-spacing: 0.04em;",
                    "§ {num}"
                }
                h2 {
                    style: "margin: 0; font-size: var(--fs-display-md);",
                    "{title}"
                }
            }
        }
    }
}

#[cfg(feature = "dev-config")]
pub(crate) use gallery::StyleGuide;
