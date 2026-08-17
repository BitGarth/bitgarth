use crate::Route;
use crate::backend::{UpdateStatus, logout, refresh_update_status, update_status};
use crate::channel::{UpgradeUi, channel, parse_channel};
use crate::{AuthState, AuthStatus};
use dioxus::logger::tracing;
use dioxus::prelude::*;

use super::{
    ArchiveIcon, Banner, BuildDriftWatcher, CheckIcon, CoinGeckoPriceControl, CommandPalette,
    ContactModal, CopyIcon, ExternalLinkIcon, FileExportIcon, InstanceNoticeBanner,
    InstanceNoticeState, LoginIcon, LogoutIcon, PaymentsIcon, SettingsIcon, SidebarContact,
    ToastContainer, UpdateNotice, UserIdenticon, UserIdenticonData, WalletIcon, copy_to_clipboard,
};

/// Freshest update status observed client-side, or `None` when nothing has been
/// re-checked since the page loaded.
///
/// Deliberately a value and not a refresh tick: a tick would have to restart
/// `NavBar`'s `use_server_future`, and restarting a resource that `NavBar`
/// suspends on (`?`) re-suspends an already-mounted layout. Dioxus then tears
/// down and rebuilds the whole app subtree, double-reclaims element ids
/// (`cannot reclaim ElementId(..)`) and can trap the WASM module. Callers that
/// re-check already hold the new `UpdateStatus`, so they publish it here and
/// nothing needs to suspend again.
pub(crate) type UpdateAwarenessRefreshState = Signal<Option<UpdateStatus>>;

/// What update banner (if any) to render for the current channel/state.
///
/// Channels actuate updates differently: Docker users run a shell command, so
/// they get an actionable copy-the-command card. Native-package channels
/// (Umbrel) update through their own store, so a shell command would be wrong —
/// they get a quiet, command-less "new release available" notice instead.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UpdateBanner {
    /// Docker: actionable shell-command upgrade card.
    Script {
        latest: String,
        current: String,
        channel: String,
    },
    /// Native package manager (Umbrel): subtle, command-less notice.
    Native {
        latest: String,
        current: String,
        store: &'static str,
    },
}

/// Decide which update banner to show. Pure so the gating is unit-testable.
///
/// Honours the user's "automatic update checks" setting: when checks are
/// disabled (or no update is available, or the banner was dismissed) nothing
/// shows, regardless of channel.
fn decide_update_banner(status: &UpdateStatus, dismissed: bool) -> Option<UpdateBanner> {
    if !status.available || !status.update_check_enabled || dismissed {
        return None;
    }
    let latest = status.latest.clone()?;
    let current = status.current.clone();
    match parse_channel(Some(status.channel.as_str())).upgrade_kind() {
        UpgradeUi::Script => Some(UpdateBanner::Script {
            latest,
            current,
            channel: status.channel.clone(),
        }),
        UpgradeUi::Native(store) => Some(UpdateBanner::Native {
            latest,
            current,
            store,
        }),
        UpgradeUi::NativeUpdater | UpgradeUi::AppStore | UpgradeUi::None => None,
    }
}

fn fnv1a64(text: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    if hash == 0 { FNV_OFFSET_BASIS } else { hash }
}

fn xorshift64(mut state: u64) -> u64 {
    if state == 0 {
        state = 0x9e3779b97f4a7c15;
    }

    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    state
}

/// Deterministic PRNG stream over a seed. Each pull advances the xorshift64
/// state; the pull order here is the contract the identicon shape depends on,
/// so reordering pulls changes every existing avatar.
struct Prng(u64);

impl Prng {
    fn new(seed: &str) -> Self {
        Prng(fnv1a64(seed))
    }

    fn next(&mut self) -> u64 {
        self.0 = xorshift64(self.0);
        self.0
    }

    /// Uniform integer in `0..n`.
    fn int(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A filled (or stroked-outline) leaf / petal blade growing from `(cx, cy)`
/// along `ang` (radians, 0 = up). The core botanical primitive. `0 0 24 24`.
fn blade(cx: f64, cy: f64, ang: f64, len: f64, wid: f64, filled: bool) -> String {
    let (dx, dy) = (ang.sin(), -ang.cos());
    let (tx, ty) = (cx + dx * len, cy + dy * len);
    let (mx, my) = (cx + dx * len * 0.5, cy + dy * len * 0.5);
    let (px, py) = (-dy * wid, dx * wid);
    let fill = if filled { "var(--fg)" } else { "none" };
    format!(
        "<path d=\"M{:.2} {:.2} Q{:.2} {:.2} {:.2} {:.2} Q{:.2} {:.2} {:.2} {:.2} Z\" fill=\"{}\"/>",
        cx,
        cy,
        mx + px,
        my + py,
        tx,
        ty,
        mx - px,
        my - py,
        cx,
        cy,
        fill
    )
}

/// Wrap `inner` in a rotation about `(px, py)` (degrees); identity when `a == 0`.
fn rot(a: f64, px: f64, py: f64, inner: &str) -> String {
    if a != 0.0 {
        format!(
            "<g transform=\"rotate({:.2} {:.2} {:.2})\">{}</g>",
            a, px, py, inner
        )
    } else {
        inner.to_string()
    }
}

/// `n` evenly-spaced stroked petal circles (radius `pr`) on a ring of radius `r`.
fn ring(cx: f64, cy: f64, r: f64, pr: f64, n: u64) -> String {
    let mut s = String::new();
    for k in 0..n {
        let a = k as f64 / n as f64 * std::f64::consts::TAU;
        s.push_str(&format!(
            "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"{}\" fill=\"none\"/>",
            cx + a.sin() * r,
            cy - a.cos() * r,
            pr
        ));
    }
    s
}

/// Slide `inner` horizontally: a stem grows from a range of ground positions, or
/// a hanging fruit attaches at a range of points along the top.
fn shift(r: &mut Prng, inner: String) -> String {
    let dx = r.int(7) as i64 - 3;
    if dx != 0 {
        format!("<g transform=\"translate({} 0)\">{}</g>", dx, inner)
    } else {
        inner
    }
}

/// Plant a glyph: a 3-way slant (`(int(3)-1) * mul` degrees) about `(px, py)`,
/// then a ground-x shift. Sequences the two PRNG pulls so the slant is read
/// before the shift (the order the prototype uses).
fn grounded(r: &mut Prng, mul: f64, px: f64, py: f64, s: &str) -> String {
    let ang = (r.int(3) as f64 - 1.0) * mul;
    shift(r, rot(ang, px, py, s))
}

/// Point on the quadratic Bézier `p0→p1→p2` at parameter `t`.
fn qbez(p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    (
        u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
        u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
    )
}

// ── glyph vocabulary ────────────────────────────────────────────────────
// Each glyph pulls its own seed-driven variations from `r`; the pull order is
// the contract the shape depends on. Geometry mirrors
// docs/ux/identicon-glyph-prototype.html. `var(--fg)` is the ink, `var(--fg2)`
// the second ink (two-tone glyphs), `var(--bg)` the parchment ground.

fn g_leaf(r: &mut Prng) -> String {
    let mut s = String::from(
        "<path d=\"M12 3.5C7 8 7 16 12 20.5C17 16 17 8 12 3.5Z\"/><path d=\"M12 5.5V19\"/>",
    );
    let veins = r.int(3);
    for _ in 0..veins {
        let y = 6.5 + r.int(4) as f64 * 2.6;
        let side = if r.int(2) != 0 { 1.0 } else { -1.0 };
        s += &format!(
            "<path d=\"M12 {:.2} L{:.2} {:.2}\"/>",
            y,
            12.0 + side * 3.4,
            y - 2.7
        );
    }
    rot(r.int(12) as f64 * 30.0, 12.0, 12.0, &s)
}

fn g_fern(r: &mut Prng) -> String {
    let mut pin: Vec<(f64, f64)> = Vec::new();
    for y in [7.0, 11.0, 15.0] {
        pin.push((y, -1.0));
        pin.push((y, 1.0));
    }
    let miss = r.int(3);
    for _ in 0..miss {
        if pin.len() > 2 {
            let idx = r.int(pin.len() as u64) as usize;
            pin.remove(idx);
        }
    }
    let mut s = String::from("<path d=\"M12 21V4\"/>");
    for &(y, side) in &pin {
        let reach = 2.0 + (21.0 - y) * 0.16;
        s += &format!(
            "<path d=\"M12 {:.2} L{:.2} {:.2}\"/>",
            y,
            12.0 + side * reach,
            y - 2.3
        );
    }
    grounded(r, 8.0, 12.0, 21.0, &s)
}

fn g_sprout(r: &mut Prng) -> String {
    let mut s = String::from("<path d=\"M12 22V10\"/>");
    s += &blade(12.0, 10.0, -0.6, 7.0, 2.1, true);
    s += &blade(12.0, 10.0, 0.6, 7.0, 2.1, true);
    let extra = r.int(3);
    for i in 0..extra {
        let y = (13.5 + i as f64 * 2.6 + r.int(2) as f64).min(19.0);
        let side = if r.int(2) != 0 { 1.0 } else { -1.0 };
        s += &blade(12.0, y, side * 1.2, 3.6, 1.3, true);
    }
    grounded(r, 7.0, 12.0, 22.0, &s)
}

fn g_acorn(r: &mut Prng) -> String {
    let nut = "<path d=\"M7 11.5Q7 20.5 12 20.5Q17 20.5 17 11.5Z\" fill=\"var(--fg)\"/>";
    let cap = "<path d=\"M5 11Q12 5.5 19 11Z\"/><path d=\"M8.2 8.8Q12 7 15.8 8.8\"/>";
    let stalk = rot(
        (r.int(3) as f64 - 1.0) * 20.0,
        12.0,
        6.4,
        "<path d=\"M12 6.4V3.6\"/>",
    );
    rot(
        r.int(12) as f64 * 30.0,
        12.0,
        12.0,
        &format!("{nut}{cap}{stalk}"),
    )
}

fn g_mushroom(r: &mut Prng) -> String {
    let inv = r.int(2) == 1;
    let cap_fill = if inv { "none" } else { "var(--fg)" };
    let stem_fill = if inv { "var(--fg)" } else { "none" };
    let mut s = format!("<path d=\"M4 13Q12 2.5 20 13Z\" fill=\"{cap_fill}\"/>");
    s += &format!(
        "<path d=\"M9 13V18.8Q12 20.8 15 18.8V13{}\" fill=\"{stem_fill}\"/>",
        if inv { "Z" } else { "" }
    );
    if !inv {
        match r.int(3) {
            1 => {
                s += "<circle cx=\"11\" cy=\"9.8\" r=\"2.6\" fill=\"var(--bg)\" stroke=\"var(--bg)\"/>"
            }
            2 => {
                s += "<circle cx=\"9.6\" cy=\"10.4\" r=\"1.3\" fill=\"var(--bg)\" stroke=\"var(--bg)\"/><circle cx=\"14.6\" cy=\"11.2\" r=\"1\" fill=\"var(--bg)\" stroke=\"var(--bg)\"/>"
            }
            _ => {}
        }
    } else if r.int(2) != 0 {
        s += "<circle cx=\"11\" cy=\"9.8\" r=\"2\" fill=\"var(--fg)\"/>";
    }
    let tilt = (r.int(5) as f64 - 2.0) * 7.0;
    shift(r, rot(tilt, 12.0, 19.0, &s))
}

fn g_flower(r: &mut Prng) -> String {
    let petals = 4 + r.int(4);
    let cf = match r.int(3) {
        0 => "var(--fg)",
        1 => "var(--bg)",
        _ => "var(--fg2)",
    };
    let body = format!(
        "<circle cx=\"12\" cy=\"12\" r=\"2.6\" fill=\"{cf}\" stroke=\"{cf}\"/>{}",
        ring(12.0, 12.0, 6.0, 2.5, petals)
    );
    rot(r.int(3) as f64 * 15.0, 12.0, 12.0, &body)
}

fn g_berry(r: &mut Prng) -> String {
    let three = r.int(2) == 1;
    let lside = if r.int(2) != 0 { 1.9 } else { -1.9 };
    let stalk = format!("<path d=\"M12 4.6L{:.2} 2.9\"/>", 12.0 + lside);
    let s = if three {
        format!(
            "{stalk}<path d=\"M12 5C10.5 8 8.8 10 8 12.2\"/><path d=\"M12 5C13.5 8 15.2 10 16 12.2\"/><path d=\"M12 5V13.4\"/><circle cx=\"8\" cy=\"13\" r=\"2.3\" fill=\"var(--fg)\"/><circle cx=\"16\" cy=\"13\" r=\"2.3\" fill=\"var(--fg)\"/><circle cx=\"12\" cy=\"15.8\" r=\"2.3\" fill=\"var(--fg2)\" stroke=\"var(--fg2)\"/>"
        )
    } else {
        format!(
            "{stalk}<path d=\"M12 5C11 9 9.6 11 8.6 12.8\"/><path d=\"M12 5C13 9 14.4 11 15.4 12.8\"/><circle cx=\"8.2\" cy=\"15.5\" r=\"2.7\" fill=\"var(--fg)\"/><circle cx=\"15.8\" cy=\"15.5\" r=\"2.7\" fill=\"var(--fg)\"/>"
        )
    };
    let sway = (r.int(5) as f64 - 2.0) * 6.0;
    shift(r, rot(sway, 12.0, 5.0, &s))
}

fn g_clover(r: &mut Prng) -> String {
    let four = r.int(2) == 1;
    let lobes: &[(f64, f64)] = if four {
        &[(9.4, 9.0), (14.6, 9.0), (9.4, 13.4), (14.6, 13.4)]
    } else {
        &[(12.0, 8.2), (8.4, 12.2), (15.6, 12.2)]
    };
    let mut lob = String::new();
    for &(x, y) in lobes {
        lob += &format!("<circle cx=\"{x}\" cy=\"{y}\" r=\"2.8\" fill=\"var(--fg)\"/>");
    }
    if four {
        lob += "<path d=\"M12 8V14.2M8.8 11.2H15.2\" stroke=\"var(--bg)\" stroke-width=\"1.2\"/>";
    } else {
        lob += "<path d=\"M12 11V14M12 11L9.2 9.4M12 11L14.8 9.4\" stroke=\"var(--bg)\" stroke-width=\"1.1\"/>";
    }
    let rot_lob = rot(r.int(6) as f64 * 15.0, 12.0, 11.0, &lob);
    let stem = format!(
        "<path d=\"M12 {}V{}\"/>",
        if four { "14.4" } else { "13.4" },
        if r.int(2) != 0 { "21" } else { "19" }
    );
    shift(r, format!("{rot_lob}{stem}"))
}

fn g_wheat(r: &mut Prng) -> String {
    let mut s = String::from("<path d=\"M12 21V8\"/>");
    s += &blade(12.0, 7.6, 0.0, 3.0, 1.2, true);
    let pairs = 2 + r.int(3);
    for i in 0..pairs {
        let y = 10.4 + i as f64 * 2.3;
        if y > 18.5 {
            break;
        }
        s += &blade(12.0, y, -0.9, 2.8, 1.2, true);
        s += &blade(12.0, y, 0.9, 2.8, 1.2, true);
    }
    grounded(r, 6.0, 12.0, 21.0, &s)
}

fn g_tulip(r: &mut Prng) -> String {
    let mut s = String::from(
        "<path d=\"M8.5 12C8.5 7 9.4 5.4 12 5.4C14.6 5.4 15.5 7 15.5 12Z\" fill=\"var(--fg)\"/>",
    );
    // 2 or 3 internal stripes → 3 or 4 lobes; outer stripes start lower (petals
    // splay), centre runs tallest — still a tulip cup.
    let segs = 2 + r.int(2);
    for i in 1..=segs {
        let x = 8.5 + 7.0 * i as f64 / (segs + 1) as f64;
        let top_y = 5.6 + (i as f64 / (segs + 1) as f64 - 0.5).abs() * 4.0;
        s += &format!(
            "<path d=\"M{:.2} {:.2}V11.6\" stroke=\"var(--bg)\" stroke-width=\"1\"/>",
            x, top_y
        );
    }
    s += "<path d=\"M12 12V21\"/>";
    let leaves = r.int(3);
    if leaves >= 1 {
        s += &blade(11.6, 20.5, -0.32, 5.5, 1.5, true);
    }
    if leaves >= 2 {
        s += &blade(12.4, 20.5, 0.32, 5.5, 1.5, true);
    }
    grounded(r, 8.0, 12.0, 21.0, &s)
}

fn g_pinecone(r: &mut Prng) -> String {
    let rows = 3 + r.int(3);
    let mut s = String::from(
        "<path d=\"M12 4C8.2 6 7.2 12 12 21C16.8 12 15.8 6 12 4Z\" fill=\"var(--fg)\"/>",
    );
    for i in 0..rows {
        let y = 7.0 + i as f64 * (11.0 / rows as f64);
        let w = 4.0 - (i as f64 - rows as f64 / 2.0).abs() * 0.4;
        s += &format!(
            "<path d=\"M{:.2} {:.2}Q12 {:.2} {:.2} {:.2}\" stroke=\"var(--bg)\" stroke-width=\"1\" fill=\"none\"/>",
            12.0 - w,
            y,
            y + 1.8,
            12.0 + w,
            y
        );
    }
    rot(r.int(12) as f64 * 30.0, 12.0, 12.0, &s)
}

fn g_peapod(r: &mut Prng) -> String {
    let peas = 3 + r.int(3);
    let mut s = String::from(
        "<path d=\"M7 6C4.5 12 6.5 18.5 12.5 20.5\"/><path d=\"M7 6C9.8 9.5 11.2 14 12.5 20.5\"/>",
    );
    for i in 0..peas {
        let t = if peas > 1 {
            i as f64 / (peas - 1) as f64
        } else {
            0.5
        };
        s += &format!(
            "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"1.5\" fill=\"var(--fg)\"/>",
            8.3 + t * 3.2,
            8.5 + t * 9.5
        );
    }
    rot(r.int(12) as f64 * 30.0, 12.0, 12.0, &s)
}

fn g_thistle(r: &mut Prng) -> String {
    let spikes = 6 + r.int(3);
    let mut s = String::from("<path d=\"M8 13Q8 19 12 19Q16 19 16 13Z\" fill=\"var(--fg)\"/>");
    let bands = 2 + r.int(3); // 2..4 horizontal bands on the bulb
    for i in 0..bands {
        let y = 14.0 + i as f64 * (4.0 / bands as f64);
        s += &format!(
            "<path d=\"M8.7 {:.2}H15.3\" stroke=\"var(--bg)\" stroke-width=\"0.8\"/>",
            y
        );
    }
    for i in 0..spikes {
        let a = -0.8 + i as f64 * (1.6 / (spikes - 1) as f64);
        s += &format!(
            "<path d=\"M{:.2} {:.2}L{:.2} {:.2}\"/>",
            12.0 + a.sin() * 1.8,
            13.0 - a.cos() * 1.2,
            12.0 + a.sin() * 6.5,
            13.0 - a.cos() * 6.5
        );
    }
    s += "<path d=\"M12 19V22\"/>";
    grounded(r, 7.0, 12.0, 22.0, &s)
}

fn g_dandelion(r: &mut Prng) -> String {
    let spokes = 8 + r.int(3) * 2;
    let mut s = String::from("<circle cx=\"12\" cy=\"9\" r=\"1\" fill=\"var(--fg)\"/>");
    for i in 0..spokes {
        if r.int(6) == 0 {
            continue;
        }
        let a = i as f64 / spokes as f64 * std::f64::consts::TAU;
        let (ex, ey) = (12.0 + a.sin() * 5.0, 9.0 - a.cos() * 5.0);
        s += &format!(
            "<path d=\"M12 9L{:.2} {:.2}\"/><circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"0.9\" fill=\"none\"/>",
            ex, ey, ex, ey
        );
    }
    s += "<path d=\"M12 9V21\"/>";
    grounded(r, 6.0, 12.0, 21.0, &s)
}

fn g_bee(r: &mut Prng) -> String {
    let stripes = 2 + r.int(2);
    let mut s = String::from(
        "<path d=\"M8 12Q8 18.5 12 18.5Q16 18.5 16 12Q16 7 12 7Q8 7 8 12Z\" fill=\"var(--fg)\"/>",
    );
    for i in 0..stripes {
        s += &format!(
            "<path d=\"M8.4 {:.2}H15.6\" stroke=\"var(--bg)\" stroke-width=\"1.1\"/>",
            9.8 + i as f64 * 2.4
        );
    }
    s += "<path d=\"M10.5 8C6.5 4.5 3.5 6.5 5.5 9.8C6.8 11.6 9.3 10.4 10.5 8Z\" fill=\"none\"/><path d=\"M13.5 8C17.5 4.5 20.5 6.5 18.5 9.8C17.2 11.6 14.7 10.4 13.5 8Z\" fill=\"none\"/><path d=\"M10.8 7.3L9.5 5M13.2 7.3L14.5 5\"/>";
    let body = rot(r.int(12) as f64 * 30.0, 12.0, 12.0, &s);
    if r.int(2) != 0 {
        format!("<g transform=\"translate(24 0) scale(-1 1)\">{body}</g>")
    } else {
        body
    }
}

fn g_butterfly(r: &mut Prng) -> String {
    let mut s = String::from(
        "<path d=\"M12 7V17\"/><path d=\"M12 9C7.5 4.5 4 7 5.5 11C7 13.5 10.5 12 12 9Z\" fill=\"var(--fg)\"/><path d=\"M12 9C16.5 4.5 20 7 18.5 11C17 13.5 13.5 12 12 9Z\" fill=\"var(--fg)\"/><path d=\"M12 11C9 11 6.5 13 7.5 16.5C8.3 18.5 11 17.5 12 14.5Z\" fill=\"var(--fg)\"/><path d=\"M12 11C15 11 17.5 13 16.5 16.5C15.7 18.5 13 17.5 12 14.5Z\" fill=\"var(--fg)\"/>",
    );
    for (x, y) in [(8.0, 8.5), (16.0, 8.5), (9.0, 14.7), (15.0, 14.7)] {
        if r.int(2) != 0 {
            s += &format!(
                "<circle cx=\"{x}\" cy=\"{y}\" r=\"1\" fill=\"var(--bg)\" stroke=\"var(--bg)\"/>"
            );
        }
    }
    s += "<path d=\"M12 7L10 4.5M12 7L14 4.5\"/>";
    let body = rot(r.int(12) as f64 * 30.0, 12.0, 12.0, &s);
    if r.int(2) != 0 {
        format!("<g transform=\"translate(24 0) scale(-1 1)\">{body}</g>")
    } else {
        body
    }
}

fn g_pumpkin(r: &mut Prng) -> String {
    let mut s = String::from(
        "<path d=\"M5 14C5 9.5 8 9 12 9C16 9 19 9.5 19 14C19 18.5 16 19 12 19C8 19 5 18.5 5 14Z\" fill=\"var(--fg)\"/><path d=\"M9.3 9.4C8 12 8 16 9.3 18.6\" stroke=\"var(--bg)\" stroke-width=\"1\" fill=\"none\"/><path d=\"M14.7 9.4C16 12 16 16 14.7 18.6\" stroke=\"var(--bg)\" stroke-width=\"1\" fill=\"none\"/>",
    );
    if r.int(2) != 0 {
        s += "<path d=\"M12 9.2V18.8\" stroke=\"var(--bg)\" stroke-width=\"1\"/>";
    }
    s += "<path d=\"M12 9V6.2\"/>";
    if r.int(2) != 0 {
        s += "<path d=\"M12 7C14 6 15.5 7 15.5 8.5\"/>";
    }
    rot((r.int(3) as f64 - 1.0) * 5.0, 12.0, 19.0, &s)
}

fn g_bramble(r: &mut Prng) -> String {
    let ripe = r.int(2) == 1;
    let pts: [(f64, f64); 12] = [
        (12.0, 9.7),
        (9.3, 10.6),
        (14.7, 10.6),
        (7.6, 13.0),
        (10.4, 12.9),
        (13.6, 12.9),
        (16.4, 13.0),
        (8.8, 15.7),
        (12.0, 16.0),
        (15.2, 15.7),
        (10.4, 18.2),
        (13.6, 18.2),
    ];
    let mut s = String::new();
    for (i, &(x, y)) in pts.iter().enumerate() {
        let c = if ripe && i % 4 == 0 {
            "var(--fg2)"
        } else {
            "var(--fg)"
        };
        s += &format!("<circle cx=\"{x}\" cy=\"{y}\" r=\"2.1\" fill=\"{c}\" stroke=\"{c}\"/>");
    }
    s += "<path d=\"M12 9.7L9.9 7M12 9.7L14.1 7M12 9.7V6.6\"/>";
    rot(r.int(12) as f64 * 30.0, 12.0, 13.5, &s)
}

fn g_rosehip(r: &mut Prng) -> String {
    let mut s = String::from(
        "<path d=\"M8.5 11Q8.5 17.5 12 18Q15.5 17.5 15.5 11Q15.5 6.5 12 6.5Q8.5 6.5 8.5 11Z\" fill=\"var(--fg)\"/><path d=\"M10 17.6L9 20M12 18.1V20.6M14 17.6L15 20\"/><path d=\"M12 6.5V3.8\"/>",
    );
    if r.int(2) != 0 {
        s += "<circle cx=\"10.4\" cy=\"10\" r=\"1\" fill=\"var(--bg)\" stroke=\"var(--bg)\"/>";
    }
    rot(r.int(12) as f64 * 30.0, 12.0, 12.0, &s)
}

fn g_sloe(r: &mut Prng) -> String {
    let mut s = String::from(
        "<circle cx=\"12\" cy=\"14\" r=\"4.2\" fill=\"var(--fg)\"/><path d=\"M12 9.8V4.6\"/>",
    );
    if r.int(2) != 0 {
        s += "<circle cx=\"10.3\" cy=\"12.3\" r=\"1\" fill=\"var(--bg)\" stroke=\"var(--bg)\"/>";
    }
    s += &blade(12.0, 8.2, 0.85, 3.4, 1.1, true);
    rot(r.int(12) as f64 * 30.0, 12.0, 13.0, &s)
}

fn g_hawthorn(r: &mut Prng) -> String {
    let n = 3 + r.int(2);
    let pos: &[(f64, f64)] = if n == 3 {
        &[(9.5, 13.0), (14.5, 13.0), (12.0, 16.5)]
    } else {
        &[(9.0, 12.0), (15.0, 12.0), (10.5, 16.0), (13.5, 16.0)]
    };
    let mut s = String::from("<path d=\"M12 8V4.5\"/>");
    for &(x, y) in pos {
        s += &format!(
            "<path d=\"M12 8L{:.2} {:.2}\" stroke-width=\"0.9\"/><circle cx=\"{x}\" cy=\"{y}\" r=\"2.1\" fill=\"var(--fg)\"/><circle cx=\"{x}\" cy=\"{:.2}\" r=\"0.6\" fill=\"var(--bg)\" stroke=\"var(--bg)\"/>",
            x,
            y - 1.8,
            y + 1.3
        );
    }
    rot(r.int(12) as f64 * 30.0, 12.0, 12.0, &s)
}

fn g_bluebell(r: &mut Prng) -> String {
    let dir = if r.int(2) != 0 { 1.0 } else { -1.0 };
    let top = (12.0 + dir * 4.5, 7.5);
    let mut s = format!("<path d=\"M12 22Q12 13 {:.2} {:.2}\"/>", top.0, top.1);
    let n = 1 + r.int(2);
    for i in 0..n {
        let bx = top.0 - i as f64 * dir * 3.6;
        let by = top.1 + i as f64 * 1.9;
        s += &format!(
            "<path d=\"M{:.2} {:.2}C{:.2} {:.2} {:.2} {:.2} {:.2} {:.2}Q{:.2} {:.2} {:.2} {:.2}C{:.2} {:.2} {:.2} {:.2} {:.2} {:.2}Z\" fill=\"var(--fg)\"/><path d=\"M{:.2} {:.2}v-1.8M{:.2} {:.2}v-1.8\" stroke=\"var(--bg)\" stroke-width=\"0.9\"/>",
            bx,
            by,
            bx - 3.2,
            by + 1.2,
            bx - 3.4,
            by + 6.4,
            bx - 1.9,
            by + 7.8,
            bx,
            by + 8.7,
            bx + 1.9,
            by + 7.8,
            bx + 3.4,
            by + 6.4,
            bx + 3.2,
            by + 1.2,
            bx,
            by,
            bx - 1.1,
            by + 8.0,
            bx + 1.1,
            by + 8.0
        );
    }
    grounded(r, 6.0, 12.0, 22.0, &s)
}

fn g_strawberry(r: &mut Prng) -> String {
    let mut s = String::from(
        "<path d=\"M12 9.6C12 6.9 7.4 6.3 6.6 10.6C6 14 9 18.4 12 20.5C15 18.4 18 14 17.4 10.6C16.6 6.3 12 6.9 12 9.6Z\" fill=\"var(--fg)\"/>",
    );
    for (x, y) in [
        (9.6, 11.0),
        (12.0, 11.5),
        (14.4, 11.0),
        (8.5, 13.5),
        (11.0, 13.8),
        (13.0, 13.8),
        (15.5, 13.5),
        (10.0, 16.0),
        (12.2, 16.2),
        (14.0, 16.0),
        (11.0, 18.4),
        (13.0, 18.4),
    ] {
        s += &format!(
            "<circle cx=\"{x}\" cy=\"{y}\" r=\"0.7\" fill=\"var(--bg)\" stroke=\"var(--bg)\"/>"
        );
    }
    s += "<path d=\"M12 9V5.6\"/>";
    s += &blade(12.0, 9.3, -0.9, 4.5, 1.5, true);
    s += &blade(12.0, 9.3, -0.3, 4.5, 1.5, true);
    s += &blade(12.0, 9.3, 0.3, 4.5, 1.5, true);
    s += &blade(12.0, 9.3, 0.9, 4.5, 1.5, true);
    rot(r.int(12) as f64 * 30.0, 12.0, 12.5, &s)
}

fn g_cattail(r: &mut Prng) -> String {
    fn stalk(x: f64) -> String {
        format!(
            "<path d=\"M{:.2} 21V13\"/><rect x=\"{:.2}\" y=\"7\" width=\"3.2\" height=\"6\" rx=\"1.6\" fill=\"var(--fg)\"/><path d=\"M{:.2} 7V3.5\"/>",
            x,
            x - 1.6,
            x
        )
    }
    let mut s = if r.int(2) != 0 {
        format!("{}{}", stalk(9.5), stalk(14.5))
    } else {
        stalk(12.0)
    };
    s += &blade(12.0, 21.0, -0.3, 7.5, 1.4, true);
    s += &blade(12.0, 21.0, 0.3, 7.5, 1.4, true);
    grounded(r, 5.0, 12.0, 21.0, &s)
}

fn g_thornbranch(r: &mut Prng) -> String {
    let (p0, p1, p2) = ((5.0, 20.5), (12.0, 14.0), (20.0, 5.0));
    let mut s = String::from("<path d=\"M5 20.5Q12 14 20 5\"/>");
    for i in 0..7u64 {
        let t = 0.08 + i as f64 * 0.13;
        let p = qbez(p0, p1, p2, t);
        let side = if i % 2 != 0 { 1.0 } else { -1.0 };
        s += &format!(
            "<path d=\"M{:.2} {:.2}L{:.2} {:.2}L{:.2} {:.2}Z\" fill=\"var(--fg)\"/>",
            p.0,
            p.1,
            p.0 + side * 3.2,
            p.1 - 0.2,
            p.0 + side * 0.5,
            p.1 + 1.6
        );
    }
    let body = if r.int(2) != 0 {
        format!("<g transform=\"translate(24 0) scale(-1 1)\">{s}</g>")
    } else {
        s
    };
    rot((r.int(3) as f64 - 1.0) * 8.0, 12.0, 12.0, &body)
}

fn g_teasel(r: &mut Prng) -> String {
    let mut s = String::from(
        "<path d=\"M12 21V13\"/><path d=\"M9 9Q9 14 12 14Q15 14 15 9Q15 5 12 5Q9 5 9 9Z\" fill=\"var(--fg)\"/>",
    );
    // body texture: plain blob, pitted seed-head (bg dots), or banded rows
    match r.int(3) {
        1 => {
            for (x, y) in [
                (12.0, 6.6),
                (10.6, 8.0),
                (13.4, 8.0),
                (10.0, 10.0),
                (12.0, 10.0),
                (14.0, 10.0),
                (10.6, 12.0),
                (13.4, 12.0),
                (12.0, 13.3),
            ] {
                s += &format!(
                    "<circle cx=\"{x}\" cy=\"{y}\" r=\"0.55\" fill=\"var(--bg)\" stroke=\"var(--bg)\"/>"
                );
            }
        }
        2 => {
            s += "<path d=\"M9.7 7.6H14.3M9.4 9.8H14.6M9.7 12H14.3\" stroke=\"var(--bg)\" stroke-width=\"0.7\"/>";
        }
        _ => {}
    }
    for i in 0..12u64 {
        let a = i as f64 / 12.0 * std::f64::consts::TAU;
        s += &format!(
            "<path d=\"M{:.2} {:.2}L{:.2} {:.2}\" stroke-width=\"0.9\"/>",
            12.0 + a.sin() * 3.2,
            9.0 - a.cos() * 4.0,
            12.0 + a.sin() * 5.1,
            9.0 - a.cos() * 6.1
        );
    }
    s += "<path d=\"M12 5V2.6\"/>";
    grounded(r, 6.0, 12.0, 21.0, &s)
}

fn g_holly(r: &mut Prng) -> String {
    // single holly leaf: vertical blade with sharp spikes each side + central vein.
    let pairs = 3 + r.int(2); // 3 or 4 spikes per side → leaf length
    let breadth = 3.6 + r.int(2) as f64 * 0.8; // narrow vs broad blade
    let top = 3.0;
    let bot = top + if pairs == 4 { 18.0 } else { 15.0 };
    let seg = (bot - top) / (pairs as f64 * 2.0);
    let in_x = 0.7;
    let mut pts: Vec<(f64, f64)> = vec![(12.0, top)]; // right edge, top→bottom
    for i in 0..pairs {
        let grow = (((i as f64 + 0.5) / pairs as f64) * std::f64::consts::PI).sin();
        pts.push((
            12.0 + breadth * (0.45 + 0.55 * grow),
            top + seg * (2.0 * i as f64 + 1.0),
        ));
        pts.push((12.0 + in_x, top + seg * (2.0 * i as f64 + 2.0)));
    }
    let last = pts.len() - 1;
    pts[last] = (12.0, bot); // last notch → bottom tip
    let mut d = format!("M12 {:.2}", top);
    for &(x, y) in &pts[1..] {
        d += &format!("L{x:.2} {y:.2}");
    }
    for &(x, y) in pts[1..pts.len() - 1].iter().rev() {
        d += &format!("L{:.2} {:.2}", 24.0 - x, y); // mirror left edge up
    }
    let s = format!(
        "<path d=\"{}Z\" fill=\"var(--fg)\"/><path d=\"M12 {:.2}V{:.2}\" stroke=\"var(--bg)\" stroke-width=\"0.8\"/>",
        d,
        top + 1.5,
        bot - 1.5
    );
    rot(r.int(12) as f64 * 30.0, 12.0, 12.0, &s) // whole-leaf slant, any direction
}

fn g_snowflake(r: &mut Prng) -> String {
    // 6-fold dendrite: one recursive arm, replicated by rotation. true self-similar.
    fn branch(x: f64, y: f64, ang: f64, l: f64, d: i64, tw: f64, bang: f64) -> String {
        let (ex, ey) = (x + ang.sin() * l, y - ang.cos() * l);
        let mut p = format!("<path d=\"M{x:.2} {y:.2} L{ex:.2} {ey:.2}\"/>");
        if d <= 0 {
            return p;
        }
        for t in [0.5_f64, 0.8] {
            let (bx, by) = (x + ang.sin() * l * t, y - ang.cos() * l * t);
            p += &branch(bx, by, ang - bang, l * tw, d - 1, tw, bang);
            p += &branch(bx, by, ang + bang, l * tw, d - 1, tw, bang);
        }
        p
    }
    let (cx, cy, len) = (12.0, 12.0, 8.5);
    let depth = 1 + r.int(2) as i64; // 1 or 2 levels of side twigs
    let tw = 0.30 + r.int(2) as f64 * 0.12; // twig length ratio
    let bang = std::f64::consts::FRAC_PI_3; // 60° twigs
    let arm = branch(cx, cy, 0.0, len, depth, tw, bang);
    let mut s = String::new();
    for k in 0..6 {
        s += &rot(k as f64 * 60.0, cx, cy, &arm);
    }
    rot(r.int(2) as f64 * 30.0, cx, cy, &s) // 30° jitter
}

fn g_tree(r: &mut Prng) -> String {
    // recursive bifurcating tree (L-system). depth + fork angle + per-fork lean vary.
    fn branch(r: &mut Prng, x: f64, y: f64, ang: f64, l: f64, d: i64, spread: f64) -> String {
        let (ex, ey) = (x + ang.sin() * l, y - ang.cos() * l);
        let mut p = format!("<path d=\"M{x:.2} {y:.2} L{ex:.2} {ey:.2}\"/>");
        if d <= 0 {
            return p;
        }
        let lean = (r.int(3) as f64 - 1.0) * 0.12; // slight asymmetry per fork
        p += &branch(r, ex, ey, ang - spread + lean, l * 0.74, d - 1, spread);
        p += &branch(r, ex, ey, ang + spread + lean, l * 0.74, d - 1, spread);
        p
    }
    let depth = 3 + r.int(2) as i64; // 3 or 4 levels
    let spread = (17.0 + r.int(3) as f64 * 8.0) * std::f64::consts::PI / 180.0; // 17/25/33°
    branch(r, 12.0, 21.5, 0.0, 6.2, depth, spread) // trunk grows up from ground
}

fn g_fernfrond(r: &mut Prng) -> String {
    // recursive frond: curved rachis bearing pinnae; pinnae are themselves fronds.
    // v = (curl, pinna-angle, length-scale, tip-sweep) — fixed for the whole frond.
    fn frond(x: f64, y: f64, ang: f64, l: f64, d: i64, n: u64, v: (f64, f64, f64, f64)) -> String {
        let (curl, pa, full, sweep) = v;
        let seg = l / n as f64;
        let (mut cx, mut cy, mut a) = (x, y, ang);
        let mut p = String::new();
        for i in 0..=n {
            let (nx, ny) = (cx + a.sin() * seg, cy - a.cos() * seg);
            p += &format!("<path d=\"M{cx:.2} {cy:.2} L{nx:.2} {ny:.2}\"/>");
            a += curl * 0.12;
            cx = nx;
            cy = ny;
            if i < n {
                let pl = seg * full * (1.0 - i as f64 / (n as f64 + 1.5)); // shrink toward tip
                let o = pa + sweep * (i as f64 / n as f64); // optional forward sweep
                if d > 0 {
                    // pinna is a smaller frond
                    p += &frond(cx, cy, a - o, pl, d - 1, 3, v);
                    p += &frond(cx, cy, a + o, pl, d - 1, 3, v);
                } else {
                    // leaflet = short line each side
                    p += &format!(
                        "<path d=\"M{:.2} {:.2} L{:.2} {:.2}\"/>",
                        cx,
                        cy,
                        cx + (a - o).sin() * pl,
                        cy - (a - o).cos() * pl
                    );
                    p += &format!(
                        "<path d=\"M{:.2} {:.2} L{:.2} {:.2}\"/>",
                        cx,
                        cy,
                        cx + (a + o).sin() * pl,
                        cy - (a + o).cos() * pl
                    );
                }
            }
        }
        p
    }
    let depth = 1 + r.int(2) as i64; // 1 simple, 2 bipinnate (fractal)
    let curl = (r.int(3) as f64 - 1.0) * 0.6; // rachis curve dir/amount
    let pairs = 4 + r.int(2); // 4 or 5 pinnae pairs → density
    let pa = 0.95 + r.int(3) as f64 * 0.16; // pinna angle off rachis
    let full = 1.25 + r.int(3) as f64 * 0.2; // pinna/leaflet length scale
    let sweep = if r.int(2) != 0 { -0.18 } else { 0.0 }; // sweep toward tip or square
    shift(
        r,
        frond(12.0, 21.0, 0.0, 15.0, depth, pairs, (curl, pa, full, sweep)),
    ) // grows up from a range of ground x
}

/// Glyph vocabulary in a fixed order — the order is part of the seed→glyph map,
/// so reordering changes every avatar. Mirrors the prototype's `GLYPH_NAMES`.
const GLYPHS: [fn(&mut Prng) -> String; 30] = [
    g_leaf,
    g_fern,
    g_sprout,
    g_acorn,
    g_mushroom,
    g_flower,
    g_berry,
    g_clover,
    g_wheat,
    g_tulip,
    g_pinecone,
    g_peapod,
    g_thistle,
    g_dandelion,
    g_bee,
    g_butterfly,
    g_pumpkin,
    g_bramble,
    g_rosehip,
    g_sloe,
    g_hawthorn,
    g_bluebell,
    g_strawberry,
    g_cattail,
    g_thornbranch,
    g_teasel,
    g_holly,
    g_snowflake,
    g_tree,
    g_fernfrond,
];

/// Botanical-ledger glyph identicon. The seed picks an ink, a second ink (for
/// the two-tone glyphs), and one curated garden glyph grown with its own
/// seed-driven variations. Ink stays in the palette on a parchment ground, so
/// every avatar stays inside the design system (docs/ux/design-system.md
/// §1.1, §1.3, §2.3). Geometry mirrors docs/ux/identicon-glyph-prototype.html.
pub(crate) fn build_identicon(seed: &str) -> UserIdenticonData {
    // CSS custom properties from tokens.css, resolved at render because the SVG
    // lives in the DOM. The component sets --fg / --fg2 / --bg on the <svg>.
    const INKS: [&str; 5] = [
        "var(--moss)",     // #2F4A30
        "var(--copper)",   // #A8693A
        "var(--info)",     // #3A5A6B
        "var(--ink-soft)", // #2E352E
        "var(--danger)",   // #8A3A2C — terracotta, used here as a pure colour
    ];

    let mut r = Prng::new(seed);
    let idx = r.int(INKS.len() as u64) as usize;
    let foreground = INKS[idx].to_string();
    let alt = INKS[(idx + 2) % INKS.len()].to_string();

    let glyph = GLYPHS[r.int(GLYPHS.len() as u64) as usize];
    let mut body =
        String::from("<rect x=\"0\" y=\"0\" width=\"24\" height=\"24\" fill=\"var(--bg)\"/>");
    // Default ink group: line-only glyphs (tree, fernfrond, snowflake, …) emit
    // bare M..L.. paths and rely on this group for stroke; filled glyphs set
    // fill="var(--fg)" inline to override fill="none". Mirrors the prototype's
    // svgFor() wrapper (docs/ux/identicon-glyph-prototype.html).
    body.push_str(
        "<g fill=\"none\" stroke=\"var(--fg)\" stroke-width=\"1.6\" \
         stroke-linecap=\"round\" stroke-linejoin=\"round\">",
    );
    body.push_str(&glyph(&mut r));
    body.push_str("</g>");

    UserIdenticonData {
        foreground,
        alt,
        inner_svg: body,
    }
}

/// How long after login the automatic update check waits before running.
///
/// The check is background work with no deadline, and letting its result land
/// while the routed page under `Outlet` is still resolving its
/// `use_server_future`s makes this banner appear mid-settle. dioxus-core 0.7.9
/// then reads a stale mount for the banner and reclaims an element id that
/// belongs to `NavBar` (`cannot reclaim ElementId(..)`), which intermittently
/// escalates to a WASM `unreachable` trap. Waiting until the first render has
/// finished keeps the banner's appearance out of that window.
const AUTO_UPDATE_CHECK_DELAY_MS: u32 = 3_000;

/// Update-available banner for authenticated sessions.
///
/// Owns the update-status subscription and the "check on login" effect so a
/// re-check re-renders only this banner. `NavBar` is the layout for every
/// route: re-rendering it while the routed page underneath is still suspended
/// on its own `use_server_future` makes dioxus-core double-reclaim element ids
/// (`cannot reclaim ElementId(..)`), which can trap the WASM module.
///
/// `initial_status` is the SSR-resolved status; anything re-checked afterwards
/// arrives through [`UpdateAwarenessRefreshState`].
#[component]
pub(crate) fn UpdateAvailableBanner(initial_status: Option<UpdateStatus>) -> Element {
    let mut latest_update_status = use_context::<UpdateAwarenessRefreshState>();
    let mut dismissed = use_signal(|| false);
    let mut copied = use_signal(|| false);

    // Auto update-check: once per authenticated session, kick a throttled
    // server-side refresh so checks don't depend on the user pressing "Check
    // now". The 24h throttle lives in `refresh_update_status`, so repeated
    // mounts are cheap. This component only exists while logged in, so mounting
    // is the login edge. Runs client-side and never blocks SSR.
    use_effect(move || {
        spawn(async move {
            let mut wait = dioxus::document::eval(&format!(
                "setTimeout(() => dioxus.send(true), {AUTO_UPDATE_CHECK_DELAY_MS});"
            ));
            if wait.recv::<bool>().await.is_err() {
                return;
            }
            if let Ok(status) = refresh_update_status(false).await {
                latest_update_status.set(Some(status));
            }
        });
    });

    let status = latest_update_status().or(initial_status);
    // Single root, or nothing at all — same shape `UpdateNotice` uses. Two
    // sibling `if let` roots make dioxus-core reclaim a placeholder id twice
    // (`cannot reclaim ElementId(..)`) when the banner appears.
    let Some(banner) = status
        .as_ref()
        .and_then(|status| decide_update_banner(status, dismissed()))
    else {
        return rsx! {};
    };

    match banner {
        UpdateBanner::Script {
            latest,
            current,
            channel: banner_channel,
        } => rsx! {
            div { class: "page-container",
            section {
                class: "upgrade-notice",
                "aria-label": "Application update available",
                div { class: "upgrade-notice-head",
                    span { class: "upgrade-notice-eyebrow", "Update" }
                    p { class: "upgrade-notice-title", "Version {latest} is available" }
                    p { class: "upgrade-notice-current", "You have version {current}" }
                }
                div { class: "code-card", "data-label": "shell",
                    button {
                        class: "code-card-copy",
                        r#type: "button",
                        "aria-live": "polite",
                        "aria-label": "Copy upgrade command",
                        onclick: {
                            let command = format!(
                                "curl -fsSL https://bitgarth.app/{banner_channel}.sh | sh",
                            );
                            move |_| {
                                copy_to_clipboard(&command);
                                copied.set(true);
                                spawn(async move {
                                    let mut wait = dioxus::document::eval(
                                        "setTimeout(() => dioxus.send(true), 1600);",
                                    );
                                    let _ = wait.recv::<bool>().await;
                                    copied.set(false);
                                });
                            }
                        },
                        if copied() {
                            CheckIcon {}
                            span { "Copied" }
                        } else {
                            CopyIcon {}
                            span { "Copy" }
                        }
                    }
                    pre {
                        code {
                            span { class: "tok-cmd", "curl" }
                            " "
                            span { class: "tok-flag", "-fsSL" }
                            " "
                            span { class: "tok-img", "https://bitgarth.app/{banner_channel}.sh" }
                            " "
                            span { class: "tok-dim", "|" }
                            " "
                            span { class: "tok-cmd", "sh" }
                        }
                    }
                }
                div { class: "upgrade-notice-actions",
                    a {
                        class: "btn ghost",
                        href: "https://bitgarth.app/#upgrade-{banner_channel}",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        title: "Upgrade instructions (opens in a new tab)",
                        "Upgrade instructions"
                        " "
                        ExternalLinkIcon {}
                    }
                    a {
                        class: "btn ghost",
                        href: "https://bitgarth.app/#inspect-{banner_channel}",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        title: "Inspect script (opens in a new tab)",
                        "Inspect script"
                        " "
                        ExternalLinkIcon {}
                    }
                    button {
                        class: "btn ghost",
                        r#type: "button",
                        "aria-label": "Remind me later",
                        onclick: move |_| dismissed.set(true),
                        "Remind me later"
                    }
                }
            }
            }
        },
        UpdateBanner::Native {
            latest,
            current,
            store,
        } => rsx! {
            div { class: "page-container",
            section {
                class: "upgrade-notice",
                "aria-label": "Application update available",
                div { class: "upgrade-notice-head",
                    span { class: "upgrade-notice-eyebrow", "Update" }
                    p { class: "upgrade-notice-title", "Version {latest} is available" }
                    p { class: "upgrade-notice-current", "You have version {current}" }
                    p { class: "upgrade-notice-current", "Update from the {store} app store." }
                }
                div { class: "upgrade-notice-actions",
                    button {
                        class: "btn ghost",
                        r#type: "button",
                        "aria-label": "Remind me later",
                        onclick: move |_| dismissed.set(true),
                        "Remind me later"
                    }
                }
            }
            }
        },
    }
}

#[component]
pub fn NavBar() -> Element {
    let mut auth_state = use_context::<AuthState>();
    let navigator = use_navigator();
    let mut sidebar_open = use_signal(|| false);
    let mut user_menu_open = use_signal(|| false);
    let mut contact_open = use_signal(|| false);
    let latest_update_status: UpdateAwarenessRefreshState = use_signal(|| None);
    use_context_provider(|| latest_update_status);
    let current_route = use_route::<Route>();
    // Banner html provided by App; read defensively so a missing or pending
    // value never blanks the navbar — the banner just doesn't render.
    let notice_html =
        try_consume_context::<InstanceNoticeState>().and_then(|state| state.read().clone());

    let (is_logged_in, user_id, username) = {
        let auth_snapshot = auth_state.read();
        match &*auth_snapshot {
            AuthStatus::Authenticated(auth) => (
                true,
                Some(auth.user.user_id.to_string()),
                Some(auth.user.username.clone()),
            ),
            _ => (false, None, None),
        }
    };
    let identicon = user_id.as_deref().map(build_identicon);
    // Initial status only, resolved during SSR. This closure reads no signals
    // on purpose: `NavBar` is the layout for every route, and any restart of a
    // resource it suspends on (`?`) would re-suspend the mounted layout and
    // rebuild the entire app subtree. Later status is owned by
    // `UpdateAvailableBanner`, so re-checks never re-render this layout.
    let update_status_resource = use_server_future(move || async move {
        if is_logged_in {
            update_status().await.map(Some)
        } else {
            Ok(None)
        }
    })?;
    let initial_update_status = update_status_resource()
        .and_then(|result| result.ok())
        .flatten();
    let runtime_channel = channel();
    let _runtime_channel_header = runtime_channel.as_header_value();
    let _runtime_upgrade_kind = runtime_channel.upgrade_kind();

    let toggle_sidebar = move |_| {
        sidebar_open.set(!sidebar_open());
    };

    let close_sidebar = move |_| {
        sidebar_open.set(false);
    };

    let toggle_user_menu = move |_| {
        user_menu_open.set(!user_menu_open());
    };

    let close_user_menu_on_escape = move |evt: Event<KeyboardData>| {
        if evt.key() == Key::Escape {
            user_menu_open.set(false);
        }
    };

    let sidebar_class = if sidebar_open() {
        "sidebar open"
    } else {
        "sidebar"
    };

    let overlay_class = if sidebar_open() {
        "sidebar-overlay visible"
    } else {
        "sidebar-overlay"
    };

    let user_dropdown_class = if user_menu_open() {
        "user-menu-dropdown visible"
    } else {
        "user-menu-dropdown"
    };

    let main_content_class = if is_logged_in {
        "main-content with-sidebar"
    } else {
        "main-content"
    };

    let wallets_active = matches!(
        current_route,
        Route::Wallets | Route::AccountTransactions { .. }
    );
    let reports_active = matches!(current_route, Route::HoldingsReport { .. });
    let transactions_export_active = matches!(current_route, Route::HledgerExport);
    let wallet_data_export_active = matches!(current_route, Route::WalletDataExport);
    let payments_active = matches!(current_route, Route::Payments);
    let settings_active = matches!(current_route, Route::Settings { .. });

    rsx! {
        div { class: if is_logged_in { "app-layout logged-in" } else { "app-layout" },
            onkeydown: close_user_menu_on_escape,
            // Top Navigation Bar (always visible)
            nav { class: "navbar",
                // Left side: Brand + sidebar toggle (when logged in)
                div { class: "navbar-nav",
                    // Sidebar toggle (only on mobile when logged in)
                    if is_logged_in {
                        button {
                            class: "sidebar-toggle",
                            "aria-label": "Toggle sidebar",
                            onclick: toggle_sidebar,
                            span { class: "sidebar-toggle-line" }
                            span { class: "sidebar-toggle-line" }
                            span { class: "sidebar-toggle-line" }
                        }
                    }

                    Link {
                        to: Route::HomeView,
                        class: "navbar-brand",
                        "BitGarth"
                    }
                }

                // Right side: Auth
                div { class: "navbar-nav",
                    if is_logged_in {
                        CoinGeckoPriceControl {}

                        // User menu when logged in
                        div { class: "user-menu",
                            if user_menu_open() {
                                div {
                                    class: "user-menu-dismiss-overlay",
                                    onclick: move |_| user_menu_open.set(false),
                                }
                            }

                            button {
                                class: "navbar-icon-btn user-menu-trigger",
                                "aria-label": "User menu",
                                "aria-haspopup": "menu",
                                "aria-expanded": user_menu_open(),
                                onclick: toggle_user_menu,
                                if let Some(icon) = identicon.clone() {
                                    UserIdenticon { icon: icon }
                                }
                            }

                            div { class: "{user_dropdown_class}", role: "menu",
                                Link {
                                    to: Route::Settings { section: Some("account".to_string()) },
                                    class: "user-menu-identity",
                                    role: "menuitem",
                                    onclick: move |_| {
                                        user_menu_open.set(false);
                                    },
                                    if let Some(icon) = identicon.clone() {
                                        div { class: "user-menu-identity-avatar",
                                            UserIdenticon { icon: icon }
                                        }
                                    }
                                    div { class: "user-menu-identity-text",
                                        if let Some(ref name) = username {
                                            span { class: "user-menu-identity-name", "{name}" }
                                        }
                                        span { class: "user-menu-identity-link", "View account →" }
                                    }
                                }
                                div { class: "user-menu-body",
                                    div { class: "user-menu-section-label", "Settings" }
                                    Link {
                                        to: Route::Settings { section: Some("regional".to_string()) },
                                        class: "user-menu-item",
                                        role: "menuitem",
                                        onclick: move |_| {
                                            user_menu_open.set(false);
                                        },
                                        "Regional"
                                    }
                                    Link {
                                        to: Route::Settings { section: Some("account".to_string()) },
                                        class: "user-menu-item",
                                        role: "menuitem",
                                        onclick: move |_| {
                                            user_menu_open.set(false);
                                        },
                                        "Account"
                                    }
                                    Link {
                                        to: Route::Settings { section: Some("digital-assets".to_string()) },
                                        class: "user-menu-item",
                                        role: "menuitem",
                                        onclick: move |_| {
                                            user_menu_open.set(false);
                                        },
                                        "Digital Assets"
                                    }
                                    Link {
                                        to: Route::Settings { section: Some("system-info".to_string()) },
                                        class: "user-menu-item",
                                        role: "menuitem",
                                        onclick: move |_| {
                                            user_menu_open.set(false);
                                        },
                                        "System Info"
                                    }
                                    div { class: "user-menu-divider" }
                                    button {
                                        class: "user-menu-item",
                                        role: "menuitem",
                                        onclick: move |_| {
                                            let user_id = {
                                            let auth_snapshot = auth_state.read();
                                            match &*auth_snapshot {
                                                AuthStatus::Authenticated(auth) => Some(auth.user.user_id),
                                                _ => None,
                                            }
                                        };
                                        tracing::debug!(
                                            user_id = ?user_id,
                                            "auth ui: logout clicked"
                                        );
                                        // Apply local logout state first so the UI responds even if
                                        // the server request fails.
                                        auth_state.set(AuthStatus::Unauthenticated);
                                        user_menu_open.set(false);
                                        navigator.replace(Route::Login);

                                        spawn(async move {
                                            if let Err(err) = logout().await {
                                                tracing::warn!(
                                                    user_id = ?user_id,
                                                    error = %err,
                                                    "auth ui: logout request failed after local sign-out"
                                                );
                                            }
                                        });
                                    },
                                    LogoutIcon {}
                                    " "
                                    "Logout"
                                    }
                                }
                            }
                        }
                    } else {
                        // Mirror link: on `/login` show "Create account" → `/register`,
                        // on `/register` show "Login" → `/login`, elsewhere show "Login".
                        if matches!(current_route, Route::Login) {
                            Link {
                                to: Route::Register,
                                class: "navbar-link",
                                LoginIcon {}
                                " "
                                "Create account"
                            }
                        } else {
                            Link {
                                to: Route::Login,
                                class: "navbar-link",
                                LoginIcon {}
                                " "
                                "Login"
                            }
                        }
                    }
                }
            }

            // Sidebar (only when logged in)
            if is_logged_in {
                // Sidebar overlay (mobile)
                div {
                    class: "{overlay_class}",
                    onclick: close_sidebar,
                }

                aside { class: "{sidebar_class}",
                    nav { class: "sidebar-nav",
                        // Accounts section
                        div { class: "sidebar-section",
                            div { class: "sidebar-section-title", "Accounts" }
                            Link {
                                to: Route::Wallets,
                                class: if wallets_active { "sidebar-link active" } else { "sidebar-link" },
                                onclick: close_sidebar,
                                span { class: "sidebar-link-icon", WalletIcon {} }
                                "Wallets"
                            }
                        }

                        // Reports section
                        div { class: "sidebar-section",
                            div { class: "sidebar-section-title", "Reports" }
                            Link {
                                to: Route::HoldingsReport { start: None, end: None },
                                class: if reports_active { "sidebar-link active" } else { "sidebar-link" },
                                onclick: close_sidebar,
                                span { class: "sidebar-link-icon", FileExportIcon {} }
                                "Holdings Report"
                            }
                        }

                        // Data section
                        div { class: "sidebar-section",
                            div { class: "sidebar-section-title", "Data" }
                            Link {
                                to: Route::HledgerExport,
                                class: if transactions_export_active { "sidebar-link active" } else { "sidebar-link" },
                                onclick: close_sidebar,
                                span { class: "sidebar-link-icon", FileExportIcon {} }
                                "Accounting Export"
                            }
                            Link {
                                to: Route::WalletDataExport,
                                class: if wallet_data_export_active { "sidebar-link active" } else { "sidebar-link" },
                                onclick: close_sidebar,
                                span { class: "sidebar-link-icon", ArchiveIcon {} }
                                "Backup / Restore"
                            }
                        }

                        div { class: "sidebar-section",
                            Link {
                                to: Route::Payments,
                                class: if payments_active { "sidebar-link active" } else { "sidebar-link" },
                                "data-testid": "sidebar-link-upgrade",
                                onclick: close_sidebar,
                                span { class: "sidebar-link-icon", PaymentsIcon {} }
                                "Upgrade"
                            }
                        }

                        // Settings section
                        div { class: "sidebar-section",
                            Link {
                                to: Route::Settings { section: None },
                                class: if settings_active { "sidebar-link active" } else { "sidebar-link" },
                                onclick: close_sidebar,
                                span { class: "sidebar-link-icon", SettingsIcon {} }
                                "Settings"
                            }
                        }

                        // Contact footer, pinned to the bottom of the rail.
                        SidebarContact {
                            on_show_key: move |_| {
                                sidebar_open.set(false);
                                contact_open.set(true);
                            },
                        }
                    }
                }
            }

            if contact_open() {
                ContactModal { on_close: move |_| contact_open.set(false) }
            }


            // Mobile Bottom App Bar
            if is_logged_in {
                div { class: "mobile-bottom-bar",
                    nav { class: "mobile-bottom-bar-nav",
                        Link {
                            to: Route::Wallets,
                            class: if wallets_active { "mobile-bottom-bar-link active" } else { "mobile-bottom-bar-link" },
                            WalletIcon {}
                            "Accounts"
                        }
                        Link {
                            to: Route::HledgerExport,
                            class: if transactions_export_active { "mobile-bottom-bar-link active" } else { "mobile-bottom-bar-link" },
                            FileExportIcon {}
                            "Export"
                        }
                        Link {
                            to: Route::WalletDataExport,
                            class: if wallet_data_export_active { "mobile-bottom-bar-link active" } else { "mobile-bottom-bar-link" },
                            ArchiveIcon {}
                            "Backup"
                        }
                        Link {
                            to: Route::Settings { section: None },
                            class: if settings_active { "mobile-bottom-bar-link active" } else { "mobile-bottom-bar-link" },
                            SettingsIcon {}
                            "Settings"
                        }
                    }
                }
            }

            // Main content area
            main { class: "{main_content_class}",
                InstanceNoticeBanner { html: notice_html }
                BuildDriftWatcher {}
                UpdateNotice {}
                if is_logged_in {
                    UpdateAvailableBanner { initial_status: initial_update_status }
                    Banner {}
                    CommandPalette {}
                }
                Outlet::<Route> {}
            }

            // Toast notifications (always available)
            ToastContainer {}
        }
    }
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    fn status(channel: &str, available: bool, enabled: bool) -> UpdateStatus {
        UpdateStatus {
            available,
            latest: Some("0.1.8".to_string()),
            current: "0.1.7".to_string(),
            channel: channel.to_string(),
            release_url: None,
            update_check_enabled: enabled,
            last_checked_at: None,
        }
    }

    #[test]
    fn docker_gets_actionable_script_banner() {
        let banner = decide_update_banner(&status("docker", true, true), false);
        assert_eq!(
            banner,
            Some(UpdateBanner::Script {
                latest: "0.1.8".to_string(),
                current: "0.1.7".to_string(),
                channel: "docker".to_string(),
            })
        );
    }

    #[test]
    fn umbrel_gets_command_less_native_notice() {
        let banner = decide_update_banner(&status("umbrel", true, true), false);
        assert_eq!(
            banner,
            Some(UpdateBanner::Native {
                latest: "0.1.8".to_string(),
                current: "0.1.7".to_string(),
                store: "Umbrel",
            })
        );
    }

    #[test]
    fn disabled_update_checks_suppress_banner_on_every_channel() {
        // Honours the "automatic update checks" setting: off => nothing shows.
        assert_eq!(
            decide_update_banner(&status("docker", true, false), false),
            None
        );
        assert_eq!(
            decide_update_banner(&status("umbrel", true, false), false),
            None
        );
    }

    #[test]
    fn no_banner_when_no_update_available_or_dismissed() {
        assert_eq!(
            decide_update_banner(&status("umbrel", false, true), false),
            None
        );
        assert_eq!(
            decide_update_banner(&status("umbrel", true, true), true),
            None
        );
    }

    #[test]
    fn channels_without_a_self_serve_path_show_nothing() {
        assert_eq!(
            decide_update_banner(&status("hosted", true, true), false),
            None
        );
        assert_eq!(
            decide_update_banner(&status("unknown", true, true), false),
            None
        );
    }

    #[test]
    fn test_identicon_is_deterministic_for_same_seed() {
        let first = build_identicon("01ARZ3NDEKTSV4RRFFQ69G5FAV");
        let second = build_identicon("01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(first, second);
    }

    #[test]
    fn test_identicon_differs_for_different_seeds() {
        let first = build_identicon("01ARZ3NDEKTSV4RRFFQ69G5FAV");
        let second = build_identicon("01ARZ3NDEKTSV4RRFFQ69G5FAW");
        assert_ne!(first, second);
    }

    const PALETTE: [&str; 5] = [
        "var(--moss)",
        "var(--copper)",
        "var(--info)",
        "var(--ink-soft)",
        "var(--danger)",
    ];

    #[test]
    fn test_identicon_renders_palette_glyph() {
        // Sweep many seeds: every glyph must render, stay in the palette, sit on
        // the parchment ground, and never emit a raw hue.
        for i in 0..500 {
            let icon = build_identicon(&format!("seed-{i}"));
            assert!(PALETTE.contains(&icon.foreground.as_str()));
            assert!(PALETTE.contains(&icon.alt.as_str()));
            assert_ne!(icon.foreground, icon.alt);
            assert!(icon.inner_svg.starts_with("<rect"));
            assert!(icon.inner_svg.len() > 40);
            assert!(!icon.inner_svg.contains("hsl("));
            // Line-only glyphs (tree, fernfrond, …) emit bare M..L.. paths and
            // are invisible without the default ink-stroke group. Guard it so a
            // dropped wrapper can't silently blank those avatars again.
            assert!(icon.inner_svg.contains("stroke=\"var(--fg)\""));
        }
    }
}
