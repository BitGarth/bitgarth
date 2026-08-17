use dioxus::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UserIdenticonData {
    /// The chosen ink, as a CSS custom-property reference (e.g. `var(--moss)`).
    /// Exposed via `--fg` on the SVG so the generated markup can reference it.
    pub foreground: String,
    /// The second ink, exposed via `--fg2`, used by the few two-tone glyphs
    /// (berry, flower, bramble, holly).
    pub alt: String,
    /// Pre-rendered SVG body (the glyph) for the `0 0 24 24` view.
    pub inner_svg: String,
}

#[component]
pub fn PencilIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M12 20h9" }
            path { d: "M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z" }
        }
    }
}

#[component]
pub fn CopyIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            rect {
                x: "9",
                y: "9",
                width: "13",
                height: "13",
                rx: "2",
                ry: "2",
            }
            path { d: "M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" }
        }
    }
}

#[component]
pub fn ChevronLeftIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            polyline { points: "15 18 9 12 15 6" }
        }
    }
}

#[component]
pub fn ChevronRightIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            polyline { points: "9 18 15 12 9 6" }
        }
    }
}

#[component]
pub fn RefreshIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            polyline { points: "23 4 23 10 17 10" }
            polyline { points: "1 20 1 14 7 14" }
            path { d: "M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15" }
        }
    }
}

/// A six-sided die showing five pips. Used as the "roll again" affordance
/// on the registration username field — visually distinct from the refresh
/// arrow icon used elsewhere for sync.
#[component]
pub fn DiceIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            rect {
                x: "3",
                y: "3",
                width: "18",
                height: "18",
                rx: "3",
                ry: "3",
            }
            circle { cx: "8", cy: "8", r: "1.1", fill: "currentColor", stroke: "none" }
            circle { cx: "16", cy: "8", r: "1.1", fill: "currentColor", stroke: "none" }
            circle { cx: "12", cy: "12", r: "1.1", fill: "currentColor", stroke: "none" }
            circle { cx: "8", cy: "16", r: "1.1", fill: "currentColor", stroke: "none" }
            circle { cx: "16", cy: "16", r: "1.1", fill: "currentColor", stroke: "none" }
        }
    }
}

#[component]
pub fn WalletIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            rect { x: "1", y: "4", width: "22", height: "16", rx: "2", ry: "2" }
            line { x1: "1", y1: "10", x2: "23", y2: "10" }
        }
    }
}

#[component]
pub fn FileExportIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" }
            polyline { points: "14 2 14 8 20 8" }
            line { x1: "16", y1: "13", x2: "8", y2: "13" }
            line { x1: "16", y1: "17", x2: "8", y2: "17" }
            polyline { points: "10 9 9 9 8 9" }
        }
    }
}

#[component]
pub fn ArchiveIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            rect { x: "2", y: "3", width: "20", height: "5", rx: "1" }
            path { d: "M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8" }
            path { d: "M10 12h4" }
        }
    }
}

#[component]
pub fn SettingsIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            circle { cx: "12", cy: "12", r: "3" }
            path { d: "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" }
        }
    }
}

#[component]
pub fn PaymentsIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            rect { x: "2", y: "5", width: "20", height: "14", rx: "2", ry: "2" }
            line { x1: "2", y1: "10", x2: "22", y2: "10" }
            path { d: "M16 15h2" }
            path { d: "M12 15h1" }
        }
    }
}

#[component]
pub fn LogoutIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" }
            polyline { points: "16 17 21 12 16 7" }
            line { x1: "21", y1: "12", x2: "9", y2: "12" }
        }
    }
}

#[component]
pub fn LoginIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4" }
            polyline { points: "10 17 15 12 10 7" }
            line { x1: "15", y1: "12", x2: "3", y2: "12" }
        }
    }
}

#[component]
pub fn CheckIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            polyline { points: "20 6 9 17 4 12" }
        }
    }
}

#[component]
pub fn CloseIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            line { x1: "18", y1: "6", x2: "6", y2: "18" }
            line { x1: "6", y1: "6", x2: "18", y2: "18" }
        }
    }
}

#[component]
pub fn SearchIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            circle { cx: "11", cy: "11", r: "7" }
            line { x1: "16.5", y1: "16.5", x2: "21", y2: "21" }
        }
    }
}

#[component]
pub fn SunIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            circle { cx: "12", cy: "12", r: "5" }
            line { x1: "12", y1: "1", x2: "12", y2: "3" }
            line { x1: "12", y1: "21", x2: "12", y2: "23" }
            line { x1: "4.22", y1: "4.22", x2: "5.64", y2: "5.64" }
            line { x1: "18.36", y1: "18.36", x2: "19.78", y2: "19.78" }
            line { x1: "1", y1: "12", x2: "3", y2: "12" }
            line { x1: "21", y1: "12", x2: "23", y2: "12" }
            line { x1: "4.22", y1: "19.78", x2: "5.64", y2: "18.36" }
            line { x1: "18.36", y1: "5.64", x2: "19.78", y2: "4.22" }
        }
    }
}

#[component]
pub fn MoonIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" }
        }
    }
}

#[component]
pub fn ExternalLinkIcon() -> Element {
    rsx! {
        svg {
            width: "12",
            height: "12",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" }
            polyline { points: "15 3 21 3 21 9" }
            line { x1: "10", y1: "14", x2: "21", y2: "3" }
        }
    }
}

#[component]
pub fn ChevronDoubleLeftIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            polyline { points: "11 17 6 12 11 7" }
            polyline { points: "18 17 13 12 18 7" }
        }
    }
}

#[component]
pub fn ChevronDoubleRightIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            polyline { points: "13 17 18 12 13 7" }
            polyline { points: "6 17 11 12 6 7" }
        }
    }
}

#[component]
pub fn ArrowLeftIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            line { x1: "19", y1: "12", x2: "5", y2: "12" }
            polyline { points: "12 19 5 12 12 5" }
        }
    }
}

#[component]
pub fn SortAscIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            line { x1: "12", y1: "19", x2: "12", y2: "5" }
            polyline { points: "5 12 12 5 19 12" }
        }
    }
}

#[component]
pub fn SortDescIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            line { x1: "12", y1: "5", x2: "12", y2: "19" }
            polyline { points: "19 12 12 19 5 12" }
        }
    }
}

#[component]
pub fn ArrowDownRightIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            line { x1: "7", y1: "7", x2: "17", y2: "17" }
            polyline { points: "17 7 17 17 7 17" }
        }
    }
}

#[component]
pub fn ArrowUpRightIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            line { x1: "7", y1: "17", x2: "17", y2: "7" }
            polyline { points: "7 7 17 7 17 17" }
        }
    }
}

#[component]
pub fn ArrowRightLeftIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M16 3h5v5" }
            path { d: "M8 3H3v5" }
            path { d: "M12 22v-8.3a4 4 0 0 0-1.172-2.872L3 3" }
            path { d: "m15 9 6-6" }
        }
    }
}

#[component]
pub fn EyeIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7Z" }
            circle { cx: "12", cy: "12", r: "3" }
        }
    }
}

#[component]
pub fn EyeOffIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M9.88 9.88a3 3 0 1 0 4.24 4.24" }
            path { d: "M10.73 5.08A10.43 10.43 0 0 1 12 5c7 0 10 7 10 7a13.16 13.16 0 0 1-1.67 2.68" }
            path { d: "M6.61 6.61A13.526 13.526 0 0 0 2 12s3 7 10 7a9.74 9.74 0 0 0 5.39-1.61" }
            line { x1: "2", y1: "2", x2: "22", y2: "22" }
        }
    }
}

/// Empty state illustration: a wallet outline with a "+" for no-wallets state
#[component]
pub fn EmptyWalletIllustration() -> Element {
    rsx! {
        svg {
            class: "empty-state-illustration",
            view_box: "0 0 80 80",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.5",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            // Wallet body
            rect { x: "10", y: "22", width: "60", height: "40", rx: "4" }
            // Card slot line
            line { x1: "10", y1: "34", x2: "70", y2: "34" }
            // Clasp circle
            circle { cx: "62", cy: "45", r: "5" }
            // Plus sign
            line { x1: "40", y1: "65", x2: "40", y2: "75" }
            line { x1: "35", y1: "70", x2: "45", y2: "70" }
        }
    }
}

/// Empty state illustration: a receipt/document for no-transactions state
#[component]
pub fn EmptyTransactionsIllustration() -> Element {
    rsx! {
        svg {
            class: "empty-state-illustration",
            view_box: "0 0 80 80",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.5",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            // Document outline
            path { d: "M22 8h24l14 14v50a4 4 0 0 1-4 4H22a4 4 0 0 1-4-4V12a4 4 0 0 1 4-4z" }
            // Fold corner
            path { d: "M46 8v14h14" }
            // Empty lines
            line { x1: "28", y1: "36", x2: "52", y2: "36" }
            line { x1: "28", y1: "44", x2: "48", y2: "44" }
            line { x1: "28", y1: "52", x2: "44", y2: "52" }
        }
    }
}

#[component]
pub fn KebabIcon() -> Element {
    rsx! {
        svg {
            width: "16",
            height: "16",
            view_box: "0 0 16 16",
            fill: "currentColor",
            circle { cx: "8", cy: "3", r: "1.5" }
            circle { cx: "8", cy: "8", r: "1.5" }
            circle { cx: "8", cy: "13", r: "1.5" }
        }
    }
}

#[component]
pub fn PlusIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            line { x1: "12", y1: "5", x2: "12", y2: "19" }
            line { x1: "5", y1: "12", x2: "19", y2: "12" }
        }
    }
}

/// Table/grid icon for view toggle
#[component]
pub fn TableIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            // Grid lines
            rect { x: "3", y: "3", width: "18", height: "18", rx: "2" }
            line { x1: "3", y1: "9", x2: "21", y2: "9" }
            line { x1: "3", y1: "15", x2: "21", y2: "15" }
            line { x1: "9", y1: "3", x2: "9", y2: "21" }
        }
    }
}

/// Card/list icon for view toggle
#[component]
pub fn CardViewIcon() -> Element {
    rsx! {
        svg {
            width: "14",
            height: "14",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            rect { x: "3", y: "3", width: "18", height: "7", rx: "1" }
            rect { x: "3", y: "14", width: "18", height: "7", rx: "1" }
        }
    }
}

#[component]
pub fn UserIdenticon(icon: UserIdenticonData) -> Element {
    // `--fg` is the hashed ink, `--fg2` the second ink for two-tone glyphs, and
    // `--bg` the parchment ground. All referenced by name inside `inner_svg`.
    let style = format!(
        "--fg:{};--fg2:{};--bg:var(--paper-deep)",
        icon.foreground, icon.alt
    );
    rsx! {
        svg {
            class: "user-identicon",
            view_box: "0 0 24 24",
            role: "img",
            "aria-hidden": "true",
            style: "{style}",
            dangerous_inner_html: "{icon.inner_svg}"
        }
    }
}
