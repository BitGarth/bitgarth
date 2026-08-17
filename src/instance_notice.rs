//! Operator-supplied instance notice banner.
//!
//! Reads the `BITGARTH_INSTANCE_NOTICE_INFO` environment variable once at
//! server startup, sanitizes it to a small markdown whitelist, and caches
//! the rendered HTML in a process-global `OnceLock`. The banner component
//! reads the cached value via a server function and renders it on every
//! page of the web app.

use dioxus::logger::tracing;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html};
use std::sync::OnceLock;

pub(crate) const ENV_VAR: &str = "BITGARTH_INSTANCE_NOTICE_INFO";
pub(crate) const MAX_BYTES: usize = 4096;

static CACHED: OnceLock<Option<String>> = OnceLock::new();

/// Read the env var once and populate the cache. Idempotent: subsequent
/// calls in the same process are no-ops. Call once during server startup.
pub(crate) fn load_from_env() {
    CACHED.get_or_init(|| {
        let raw = std::env::var(ENV_VAR).ok();
        compute(raw.as_deref())
    });
}

/// Return the cached sanitized HTML, if any. Returns `None` when the env
/// var was unset, empty, oversized, or produced no whitelisted content.
pub(crate) fn cached_html() -> Option<String> {
    CACHED.get().cloned().flatten()
}

fn compute(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let len = trimmed.len();
    if len > MAX_BYTES {
        tracing::warn!(
            env_var = ENV_VAR,
            bytes = len,
            cap = MAX_BYTES,
            "instance_notice rejected: bytes exceeds cap"
        );
        return None;
    }

    let html = sanitize(trimmed);
    if html.trim().is_empty() {
        return None;
    }

    tracing::info!(env_var = ENV_VAR, bytes = len, "instance_notice loaded");
    Some(html)
}

/// Parse `input` as CommonMark and emit HTML restricted to the whitelist:
/// `<p>`, `<strong>`, `<em>`, `<br>`, and `<a>` with `http`/`https`/`mailto`
/// schemes. Everything else is dropped or rendered as plain text.
fn sanitize(input: &str) -> String {
    let parser = Parser::new_ext(input, Options::empty());
    let filtered = filter_events(parser);
    let mut out = String::with_capacity(input.len() + 32);
    html::push_html(&mut out, filtered.into_iter());
    out
}

/// Filter the `pulldown-cmark` event stream to the whitelist. Link events
/// are rewritten so the rendered `<a>` carries `rel="noopener noreferrer"`
/// and `target="_blank"` for non-`mailto:` schemes. Links with a rejected
/// scheme have their start/end tags dropped but inner text preserved.
fn filter_events<'a, I>(iter: I) -> Vec<Event<'a>>
where
    I: Iterator<Item = Event<'a>>,
{
    let mut out: Vec<Event<'a>> = Vec::new();
    let mut link_stack: Vec<bool> = Vec::new(); // true = link kept, false = dropped

    for event in iter {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => out.push(Event::Start(Tag::Paragraph)),
                Tag::Strong => out.push(Event::Start(Tag::Strong)),
                Tag::Emphasis => out.push(Event::Start(Tag::Emphasis)),
                Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                } => {
                    if let Some(safe_url) = sanitize_link_url(&dest_url) {
                        let target_blank = !is_mailto(&safe_url);
                        let rel = "noopener noreferrer";
                        let target = if target_blank { "_blank" } else { "" };
                        let html_tag = if target_blank {
                            format!(
                                r#"<a href="{href}" rel="{rel}" target="{target}">"#,
                                href = escape_attr(&safe_url),
                            )
                        } else {
                            format!(
                                r#"<a href="{href}" rel="{rel}">"#,
                                href = escape_attr(&safe_url),
                            )
                        };
                        out.push(Event::Html(html_tag.into()));
                        link_stack.push(true);
                        // Suppress fields warning by binding without use.
                        let _ = (link_type, title, id);
                    } else {
                        link_stack.push(false);
                    }
                }
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Paragraph => out.push(Event::End(TagEnd::Paragraph)),
                TagEnd::Strong => out.push(Event::End(TagEnd::Strong)),
                TagEnd::Emphasis => out.push(Event::End(TagEnd::Emphasis)),
                TagEnd::Link => {
                    if let Some(true) = link_stack.pop() {
                        out.push(Event::Html("</a>".into()));
                    }
                }
                _ => {}
            },
            Event::Text(text) => out.push(Event::Text(text)),
            Event::SoftBreak => out.push(Event::Text(" ".into())),
            Event::HardBreak => out.push(Event::Html("<br>".into())),
            // Drop everything else: raw HTML, code, math, footnotes, lists,
            // headings, images, tables, rules, task list markers, etc.
            _ => {}
        }
    }

    out
}

/// Validate a link URL against the scheme allowlist. Returns the URL
/// unchanged on success; returns `None` for any rejected scheme,
/// schemeless URL, or relative path.
fn sanitize_link_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn is_mailto(url: &str) -> bool {
    url.to_ascii_lowercase().starts_with("mailto:")
}

fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn render(input: &str) -> String {
        sanitize(input)
    }

    #[test]
    fn compute_returns_none_for_unset() {
        assert_eq!(compute(None), None);
    }

    #[test]
    fn compute_returns_none_for_empty() {
        assert_eq!(compute(Some("")), None);
    }

    #[test]
    fn compute_returns_none_for_whitespace_only() {
        assert_eq!(compute(Some("   \n\t  ")), None);
    }

    #[test]
    fn compute_rejects_oversize_input() {
        let big = "a".repeat(MAX_BYTES + 1);
        assert_eq!(compute(Some(&big)), None);
    }

    #[test]
    fn compute_accepts_input_at_cap() {
        let at_cap = "a".repeat(MAX_BYTES);
        let result = compute(Some(&at_cap));
        assert!(result.is_some());
    }

    #[test]
    fn compute_trims_before_length_check() {
        // Trailing whitespace pushes the raw value over the cap, but the
        // trimmed value is at the cap so the input is accepted.
        let body = "a".repeat(MAX_BYTES);
        let with_trailing = format!("{body}   \n");
        assert!(with_trailing.len() > MAX_BYTES);
        assert!(compute(Some(&with_trailing)).is_some());
    }

    #[test]
    fn plain_text_renders_as_paragraph() {
        let html = render("hello world");
        assert_eq!(html, "<p>hello world</p>\n");
    }

    #[test]
    fn external_link_gets_rel_and_target_blank() {
        let html = render("[label](https://example.com)");
        assert_eq!(
            html,
            r#"<p><a href="https://example.com" rel="noopener noreferrer" target="_blank">label</a></p>
"#
        );
    }

    #[test]
    fn http_link_is_allowed() {
        let html = render("[label](http://example.com)");
        assert!(html.contains(r#"href="http://example.com""#));
        assert!(html.contains(r#"target="_blank""#));
    }

    #[test]
    fn mailto_link_omits_target_blank() {
        let html = render("[label](mailto:hello@bitgarth.app)");
        assert_eq!(
            html,
            r#"<p><a href="mailto:hello@bitgarth.app" rel="noopener noreferrer">label</a></p>
"#
        );
    }

    #[test]
    fn javascript_scheme_is_dropped() {
        let html = render("[label](javascript:alert(1))");
        assert!(!html.contains("<a"));
        assert!(!html.contains("javascript"));
        assert!(html.contains("label"));
    }

    #[test]
    fn data_scheme_is_dropped() {
        let html = render("[x](data:text/html,<script>alert(1)</script>)");
        assert!(!html.contains("<a"));
        assert!(!html.contains("script"));
    }

    #[test]
    fn relative_url_is_dropped() {
        let html = render("[label](/relative/path)");
        assert!(!html.contains("<a"));
        assert!(html.contains("label"));
    }

    #[test]
    fn schemeless_url_is_dropped() {
        let html = render("[label](example.com)");
        assert!(!html.contains("<a"));
        assert!(html.contains("label"));
    }

    #[test]
    fn raw_script_tag_is_escaped() {
        let html = render("<script>alert(1)</script>");
        assert!(!html.contains("<script"));
        assert!(!html.contains("alert"));
    }

    #[test]
    fn bold_renders_as_strong() {
        let html = render("**bold**");
        assert_eq!(html, "<p><strong>bold</strong></p>\n");
    }

    #[test]
    fn emphasis_renders_as_em() {
        let html = render("*em*");
        assert_eq!(html, "<p><em>em</em></p>\n");
    }

    #[test]
    fn heading_is_stripped_to_text() {
        let html = render("# Heading text");
        // The Tag::Heading start/end events are dropped; the inner text
        // remains. With Options::empty() ATX headings still parse, so the
        // text events fire but produce no surrounding tags.
        assert!(html.contains("Heading text"));
        assert!(!html.contains("<h1"));
        assert!(!html.contains("<h2"));
    }

    #[test]
    fn list_is_stripped_to_text() {
        let html = render("- item one\n- item two");
        assert!(html.contains("item one"));
        assert!(html.contains("item two"));
        assert!(!html.contains("<ul"));
        assert!(!html.contains("<li"));
    }

    #[test]
    fn code_is_stripped() {
        let html = render("here is `inline code` and end");
        assert!(html.contains("here is"));
        assert!(html.contains("and end"));
        assert!(!html.contains("<code"));
        // Inline code event is dropped entirely.
        assert!(!html.contains("inline code"));
    }

    #[test]
    fn image_is_stripped() {
        let html = render("![alt text](https://example.com/img.png)");
        assert!(!html.contains("<img"));
        assert!(!html.contains("example.com"));
    }

    #[test]
    fn two_paragraphs_render_as_two_p_tags() {
        let html = render("first\n\nsecond");
        assert_eq!(html, "<p>first</p>\n<p>second</p>\n");
    }

    #[test]
    fn hard_break_renders_as_br() {
        // Two trailing spaces is the CommonMark hard-break syntax.
        let html = render("line one  \nline two");
        assert!(html.contains("<br>"));
        assert!(html.contains("line one"));
        assert!(html.contains("line two"));
    }

    #[test]
    fn soft_break_collapses_to_space() {
        let html = render("line one\nline two");
        // No <br>; the two lines collapse via the SoftBreak -> space mapping.
        assert!(!html.contains("<br"));
        assert!(html.contains("line one"));
        assert!(html.contains("line two"));
    }

    #[test]
    fn link_url_with_quote_is_attribute_escaped() {
        // `pulldown-cmark` will accept this dest_url; our escape_attr layer
        // must encode the embedded quote so it cannot break out of the
        // attribute.
        let html = render(r#"[label](https://example.com/"onerror=x)"#);
        // Whether the URL is accepted depends on parsing, but if it is, no
        // raw quote may appear inside the href attribute value.
        if html.contains("<a") {
            assert!(!html.contains(r#"="https://example.com/""#));
            assert!(html.contains("&quot;"));
        }
    }

    #[test]
    fn locked_in_variant_b_renders_with_two_links() {
        let copy = "This is a demo of BitGarth. Data and accounts are deleted roughly every two days. For long-term use, install BitGarth locally from [bitgarth.app](https://bitgarth.app/). Feedback is very welcome at [hello@bitgarth.app](mailto:hello@bitgarth.app).";
        let html = render(copy);
        assert!(html.starts_with("<p>"));
        assert!(html.trim_end().ends_with("</p>"));
        assert_eq!(html.matches("<a ").count(), 2);
        assert!(html.contains(r#"href="https://bitgarth.app/""#));
        assert!(html.contains(r#"target="_blank""#));
        assert!(html.contains(r#"href="mailto:hello@bitgarth.app""#));
        // mailto link must not get target="_blank"
        let mailto_idx = html.find("mailto:").unwrap_or(0);
        let mailto_anchor = &html[mailto_idx..];
        let next_close = mailto_anchor.find('>').unwrap_or(mailto_anchor.len());
        assert!(!mailto_anchor[..next_close].contains("target="));
    }
}
