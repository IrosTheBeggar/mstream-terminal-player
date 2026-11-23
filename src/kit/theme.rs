//! The setup wizard's FIXED color scheme — scoped to the wizard alone.
//!
//! The wizard is mStream's brand moment, so unlike the player (which
//! inherits the user's terminal theme through `ui::Theme`), these screens
//! ship one look everywhere a terminal can carry it. Three tiers, resolved
//! once at startup:
//!
//!   truecolor — the design canvas's literal hexes, when the terminal
//!               advertises 24-bit color (`COLORTERM=truecolor|24bit`).
//!               macOS 26's Terminal.app advertises this too.
//!   256-color — hand-tuned indexes from the standardized 6x6x6 cube
//!               (16..=231) and grayscale ramp; NEVER the first 16
//!               indexes, which terminal themes repaint. Older Apple
//!               Terminals (no truecolor) land here.
//!   ansi      — the named-color floor (the pre-fixed-scheme behavior):
//!               the wizard still works on a 16-color console.
//!
//! `MSTREAM_SETUP_THEME=ansi|256|truecolor` overrides detection — the
//! test matrix's lever, and the escape hatch for anyone whose terminal
//! lies about its capabilities.
//!
//! The GROUND is all-or-nothing. Terminals reserve margin pixels around
//! the cell grid, painted with their DEFAULT background — cell fills
//! can't reach them, so painting only cells leaves the fixed ground
//! sitting inside a border of the user's own color. [`acquire_ground`]
//! therefore asks for the whole window: it queries the terminal's default
//! background (OSC 11 — the answer doubles as capability detection and as
//! the exact value to restore) and only if the terminal answers does the
//! wizard set the default background and paint cell grounds. No answer →
//! no ground anywhere: accents stay fixed, text stays the terminal's
//! default, and there is never a two-tone border.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use ratatui::style::Color;

/// The resolved palette. Field names match the kit's tokens.
pub struct Theme {
    /// Actions, name chips, focus borders, selection bg.
    pub accent: Color,
    /// Hover states, the active rename chip and caret.
    pub bright: Color,
    /// Hints, labels, idle borders, text buttons, table headers.
    pub dim: Color,
    /// The bottom rule, warnings/errors, the warning modal.
    pub gold: Color,
    /// Checked boxes, progress, success.
    pub ok: Color,
    /// Destructive hover (the row [X]); never decoration.
    pub danger: Color,
    /// Body text — explicit, never the terminal default: on a painted
    /// ground the terminal's own default fg may be invisible.
    pub text: Color,
    /// The cell-fill background. `None` on the ansi tier — a 16-color
    /// terminal keeps its own ground, everything else still reads.
    /// Painted only while [`ground_owned`] holds.
    pub ground: Option<Color>,
    /// The same ground as raw RGB, for the OSC 11 default-background set
    /// (the 256 tier uses its index's actual value, #121212).
    pub ground_rgb: Option<(u8, u8, u8)>,
    /// Foreground on accent-filled cells (selection rows).
    pub on_accent: Color,
    /// The tier this palette was resolved for, so a colour the palette
    /// does not hold (a lead's swatch) follows the same ladder.
    pub tier: Tier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Truecolor,
    C256,
    Ansi,
}

/// Bare classic conhost - a stock cmd/PowerShell console window. It
/// exports none of the identifying variables every richer Windows
/// terminal sets (Windows Terminal: WT_SESSION; WezTerm: TERM_PROGRAM;
/// Alacritty: TERM; ConEmu: ConEmuANSI). Callers degrade gracefully
/// there: graphics queries go unanswered - and leak a blocked stdin
/// reader that eats all input (see tui::graphics) - and the legacy
/// fonts draw many glyphs as '?'. Resolved once, from the environment,
/// unless [`pin_modern_glyphs`] answered first.
pub(crate) fn legacy_conhost() -> bool {
    #[cfg(test)]
    if let Some(legacy) = TEST_GLYPHS.with(std::cell::Cell::get) {
        return legacy;
    }
    legacy_conhost_in(&GLYPHS, cfg!(windows), |var| std::env::var(var).ok())
}

// A frame test's own glyph tier. Per THREAD, not the process-wide cell:
// pinning the legacy stand-ins there would flip every frame test running
// beside it, and each test draws on its own thread.
#[cfg(test)]
thread_local! {
    static TEST_GLYPHS: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Draw this test thread's frames with the legacy console's stand-ins
/// until the guard drops — the rooms' glyph fallbacks, pinned by render.
#[cfg(test)]
pub(crate) fn legacy_glyphs_here() -> LegacyGlyphs {
    TEST_GLYPHS.with(|cell| cell.set(Some(true)));
    LegacyGlyphs
}

#[cfg(test)]
pub(crate) struct LegacyGlyphs;

#[cfg(test)]
impl Drop for LegacyGlyphs {
    fn drop(&mut self) {
        TEST_GLYPHS.with(|cell| cell.set(None));
    }
}

/// The glyph tier, once resolved: [`UNASKED`], [`LEGACY`] or [`MODERN`]. An
/// atomic rather than a `OnceLock`, so a pin lands even after something
/// has asked: a test binary resolves it from a bare CI console before the
/// frame tests pin it.
static GLYPHS: AtomicU8 = AtomicU8::new(UNASKED);
const UNASKED: u8 = 0;
const LEGACY: u8 = 1;
const MODERN: u8 = 2;

/// The GUI's own window draws with its own faces, which carry every glyph
/// the kit names — the console that launched it is not where it draws, so
/// a plain conhost must not turn its checkboxes, carets and arrows into the
/// CP437 stand-ins (PR #41's Windows report: `[>]`, `repeat` boxes, a
/// placeholder visualizer tab). Only the window path calls this, beside
/// [`pin_truecolor`], before anything draws; the frame tests use it too.
// Without the `window` feature only the frame tests call it.
#[cfg_attr(not(feature = "window"), allow(dead_code))]
pub(crate) fn pin_modern_glyphs() {
    GLYPHS.store(MODERN, Ordering::Relaxed);
}

/// Frame tests name the fancy glyphs. A bare CI console — windows-latest
/// exports none of the identifying variables — would flip every checkbox,
/// caret and arrow to the CP437 stand-ins, so those tests pin the answer.
#[cfg(test)]
pub(crate) fn pin_modern_terminal() {
    pin_modern_glyphs();
}

/// [`legacy_conhost`] against a given cell and environment: the first
/// answer stays, unless a pin replaced it.
fn legacy_conhost_in(
    cell: &AtomicU8,
    windows: bool,
    env: impl Fn(&str) -> Option<String>,
) -> bool {
    match cell.load(Ordering::Relaxed) {
        LEGACY => true,
        MODERN => false,
        _ => {
            let found = if legacy_conhost_for(windows, env) { LEGACY } else { MODERN };
            // A pin that landed while the environment was read wins.
            match cell.compare_exchange(UNASKED, found, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => found == LEGACY,
                Err(now) => now == LEGACY,
            }
        }
    }
}

/// Pure form - unit-tested; the env reads live in [`legacy_conhost`].
pub(crate) fn legacy_conhost_for(windows: bool, env: impl Fn(&str) -> Option<String>) -> bool {
    windows
        && env("WT_SESSION").is_none()
        && env("TERM_PROGRAM").is_none()
        && env("TERM").is_none()
        && env("ConEmuANSI").is_none()
}

/// Pure capability decision — unit-tested; the env reads live in [`tier`].
///
/// `windows_vt`: compiled for Windows. Windows 10+ consoles — Windows
/// Terminal and classic conhost alike — speak 24-bit color once virtual
/// terminal processing is on (the player enables it at startup), but none
/// of them export `COLORTERM` or `TERM`, so the unix heuristics bottomed
/// out at the Ansi floor for every Windows user: no truecolor palette, no
/// `ground_rgb`, and therefore no pixel wordmark/QR even on sixel-capable
/// terminals (mStream#908's Windows smoke; issue #11). The floor lifts to
/// Truecolor there — but only as the FLOOR: the override always wins, an
/// explicit `COLORTERM`/`TERM` (MSYS and mintty shells set them) keeps
/// meaning what it says, and `TERM=dumb` stays dumb.
pub fn tier_for(
    override_var: Option<&str>,
    colorterm: Option<&str>,
    term: Option<&str>,
    windows_vt: bool,
) -> Tier {
    match override_var.map(str::trim) {
        Some("ansi") => return Tier::Ansi,
        Some("256") => return Tier::C256,
        Some("truecolor") => return Tier::Truecolor,
        _ => {}
    }
    if colorterm.is_some_and(|v| v.contains("truecolor") || v.contains("24bit")) {
        return Tier::Truecolor;
    }
    if term.is_some_and(|v| v.contains("256color")) {
        return Tier::C256;
    }
    if windows_vt && term.is_none_or(|v| v != "dumb") {
        return Tier::Truecolor;
    }
    Tier::Ansi
}

fn tier() -> Tier {
    tier_for(
        std::env::var("MSTREAM_SETUP_THEME").ok().as_deref(),
        std::env::var("COLORTERM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
        cfg!(windows),
    )
}

fn palette(tier: Tier) -> Theme {
    match tier {
        // The canvas hexes, verbatim.
        Tier::Truecolor => Theme {
            accent: Color::Rgb(0x7a, 0xab, 0xdf),
            bright: Color::Rgb(0x8f, 0xd6, 0xe8),
            dim: Color::Rgb(0x7a, 0x82, 0xa0),
            gold: Color::Rgb(0xe5, 0xc0, 0x7b),
            ok: Color::Rgb(0x98, 0xc3, 0x79),
            danger: Color::Rgb(0xe0, 0x6c, 0x75),
            text: Color::Rgb(0xd8, 0xde, 0xe9),
            ground: Some(Color::Rgb(0x12, 0x13, 0x1c)),
            ground_rgb: Some((0x12, 0x13, 0x1c)),
            on_accent: Color::Rgb(0x0d, 0x10, 0x17),
            tier,
        },
        // Hand-tuned cube/ramp indexes (16..=255 only — the first 16 are
        // theme-controlled and would defeat the point).
        Tier::C256 => Theme {
            accent: Color::Indexed(110),   // #87afd7
            bright: Color::Indexed(117),   // #87d7ff
            dim: Color::Indexed(60),       // #5f5f87
            gold: Color::Indexed(179),     // #d7af5f
            ok: Color::Indexed(114),       // #87d787
            danger: Color::Indexed(167),   // #d75f5f
            text: Color::Indexed(253),     // #dadada
            ground: Some(Color::Indexed(233)), // #121212
            ground_rgb: Some((0x12, 0x12, 0x12)),
            on_accent: Color::Indexed(232),    // #080808
            tier,
        },
        // The named floor — the wizard's original adaptive palette.
        Tier::Ansi => Theme {
            accent: Color::LightBlue,
            bright: Color::Cyan,
            dim: Color::DarkGray,
            gold: Color::Yellow,
            ok: Color::Green,
            danger: Color::Red,
            text: Color::Reset,
            ground: None,
            ground_rgb: None,
            on_accent: Color::Black,
            tier,
        },
    }
}

/// The palette, once resolved: by [`th`] from the terminal's environment,
/// or by [`pin_truecolor`] for a surface that is not a terminal.
static THEME: OnceLock<Theme> = OnceLock::new();

/// The wizard's palette, resolved once.
pub fn th() -> &'static Theme {
    THEME.get_or_init(|| palette(tier()))
}

/// The GUI's own window (the window-mode spike) draws every colour it is
/// handed, so the canvas hexes are right whatever terminal launched it —
/// the environment `tier` reads describes that terminal, not the window.
/// Only the window path calls this, before anything has asked [`th`]; a
/// palette already resolved stays as it is.
#[cfg(feature = "window")]
pub(crate) fn pin_truecolor() {
    let _ = THEME.get_or_init(|| palette(Tier::Truecolor));
}

// ── Lead swatches and the room's glyphs ─────────────────────────────────────

/// True when the resolved palette is the truecolor tier: the terminal said
/// so, or the window pinned it. A colour the palette does not hold, such as
/// a lead's swatch, is drawn only on this tier (peer-recs contract §0.6).
pub fn truecolor() -> bool {
    th().tier == Tier::Truecolor
}

/// A lead's swatch as RGB: `hsl(hue, 45%, 60%)` of the server-stamped hue
/// (peer-recommendations plan §5.1, whose vectors the tests pin). A hue of
/// 360 or more wraps, as the server's own `rem_euclid(360)` does.
pub fn swatch_rgb(hue: u16) -> (u8, u8, u8) {
    let (saturation, lightness) = (0.45_f64, 0.60_f64);
    let sector = f64::from(hue % 360) / 60.0;
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let second = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (r, g, b) = match sector as u8 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let floor = lightness - chroma / 2.0;
    let byte = |v: f64| ((v + floor) * 255.0).round() as u8;
    (byte(r), byte(g), byte(b))
}

/// A lead's swatch colour: its hue in truecolor; dim on the 256- and
/// 16-colour tiers, which cannot hold it, and for a lead without a tile.
pub fn swatch(hue: Option<u16>) -> Color {
    swatch_for(truecolor(), th().dim, hue)
}

/// [`swatch`] for a given tier's answer and dim, so each tier is tested
/// whatever tier the test process resolved.
fn swatch_for(truecolor: bool, dim: Color, hue: Option<u16>) -> Color {
    match hue {
        Some(hue) if truecolor => {
            let (r, g, b) = swatch_rgb(hue);
            Color::Rgb(r, g, b)
        }
        _ => dim,
    }
}

/// The lead swatch's glyph, on every console: CP437 has the left half
/// block (0xDD), so the legacy console draws it as it is.
pub fn swatch_glyph() -> &'static str {
    "▌"
}

/// The glyph of a slot that is resolving, or waiting out a 429: the dotted
/// circle, which the legacy console's fonts lack, so it draws `~` there.
pub fn busy_glyph() -> &'static str {
    if legacy_conhost() { "~" } else { "◌" }
}

// ── Ground ownership ─────────────────────────────────────────────────────────

/// The terminal's original default background as an OSC 11 color spec,
/// captured by [`acquire_ground`]'s query. `Some(None)` = asked, refused.
static LEASE: OnceLock<Option<String>> = OnceLock::new();

fn osc11_set_seq(rgb: (u8, u8, u8)) -> String {
    format!("\x1b]11;#{:02x}{:02x}{:02x}\x07", rgb.0, rgb.1, rgb.2)
}

fn osc11_restore_seq(spec: &str) -> String {
    format!("\x1b]11;{spec}\x07")
}

/// Ask the terminal for ownership of the whole window background. Returns
/// the escape that claims it (emit once the terminal is set up), or `None`
/// when the terminal keeps its ground — the ansi floor, or a terminal that
/// did not answer the OSC 11 query.
///
/// Call BEFORE `ratatui::init()`: the query runs its own raw-mode
/// transaction on the tty.
pub fn acquire_ground() -> Option<String> {
    let rgb = th().ground_rgb?;
    let lease = LEASE.get_or_init(|| {
        terminal_colorsaurus::background_color(terminal_colorsaurus::QueryOptions::default())
            .ok()
            .map(|c| format!("rgb:{:04x}/{:04x}/{:04x}", c.r, c.g, c.b))
    });
    lease.as_ref()?;
    Some(osc11_set_seq(rgb))
}

/// True once the terminal answered the query — the gate for painting cell
/// grounds. All or nothing: without the window margin there is no ground
/// anywhere, so the fixed look never sits inside a two-tone border.
pub fn ground_owned() -> bool {
    LEASE.get().is_some_and(|l| l.is_some())
}

/// The escape that hands the background back — the exact original the
/// query captured, not a generic reset.
pub fn release_ground() -> Option<String> {
    let original = LEASE.get()?.as_ref()?;
    Some(osc11_restore_seq(original))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_beats_detection() {
        assert_eq!(tier_for(Some("ansi"), Some("truecolor"), Some("xterm-256color"), false), Tier::Ansi);
        assert_eq!(tier_for(Some("256"), Some("truecolor"), None, false), Tier::C256);
        assert_eq!(tier_for(Some("truecolor"), None, Some("vt100"), false), Tier::Truecolor);
        assert_eq!(tier_for(Some("garbage"), None, Some("xterm-256color"), false), Tier::C256);
    }

    #[test]
    fn detection_ladders_truecolor_then_256_then_ansi() {
        assert_eq!(tier_for(None, Some("truecolor"), Some("xterm"), false), Tier::Truecolor);
        assert_eq!(tier_for(None, Some("24bit"), None, false), Tier::Truecolor);
        assert_eq!(tier_for(None, None, Some("xterm-256color"), false), Tier::C256);
        assert_eq!(tier_for(None, None, Some("screen-256color"), false), Tier::C256);
        assert_eq!(tier_for(None, None, Some("xterm"), false), Tier::Ansi);
        assert_eq!(tier_for(None, None, None, false), Tier::Ansi);
    }

    #[test]
    fn bare_conhost_is_fingerprinted_only_when_nothing_identifies_the_terminal() {
        let bare = |_: &str| None;
        assert!(legacy_conhost_for(true, bare));
        assert!(!legacy_conhost_for(false, bare), "unix is never conhost");
        let wt = |v: &str| (v == "WT_SESSION").then(|| "guid".to_string());
        assert!(!legacy_conhost_for(true, wt), "Windows Terminal identifies itself");
        let wez = |v: &str| (v == "TERM_PROGRAM").then(|| "WezTerm".to_string());
        assert!(!legacy_conhost_for(true, wez));
        let ala = |v: &str| (v == "TERM").then(|| "alacritty".to_string());
        assert!(!legacy_conhost_for(true, ala));
        let conemu = |v: &str| (v == "ConEmuANSI").then(|| "ON".to_string());
        assert!(!legacy_conhost_for(true, conemu));
    }

    /// The window's pin: an environment that reads as bare conhost still
    /// draws the modern glyphs once pinned, whether the pin came before
    /// anything asked or after the environment had answered.
    #[test]
    fn the_window_pins_the_modern_glyphs_over_a_bare_conhost() {
        let bare = |_: &str| None;
        let unpinned = AtomicU8::new(UNASKED);
        assert!(legacy_conhost_in(&unpinned, true, bare), "unpinned, the env answers");
        assert!(legacy_conhost_in(&unpinned, false, bare), "and the answer stays");
        let early = AtomicU8::new(UNASKED);
        early.store(MODERN, Ordering::Relaxed);
        assert!(!legacy_conhost_in(&early, true, bare));
        unpinned.store(MODERN, Ordering::Relaxed);
        assert!(!legacy_conhost_in(&unpinned, true, bare), "a late pin still lands");
        // The live cell, through the window's own call.
        pin_modern_glyphs();
        assert!(!legacy_conhost());
    }

    #[test]
    fn windows_vt_lifts_the_floor_but_never_beats_the_env() {
        // The WT/conhost reality: no COLORTERM, no TERM — truecolor floor.
        assert_eq!(tier_for(None, None, None, true), Tier::Truecolor);
        // Explicit env still wins in both directions.
        assert_eq!(tier_for(None, None, Some("xterm-256color"), true), Tier::C256);
        assert_eq!(tier_for(Some("ansi"), None, None, true), Tier::Ansi);
        assert_eq!(tier_for(Some("256"), None, None, true), Tier::C256);
        // TERM=dumb stays dumb, even on Windows.
        assert_eq!(tier_for(None, None, Some("dumb"), true), Tier::Ansi);
        // windows_vt=false leaves the unix ladder byte-identical (pinned
        // above in detection_ladders_truecolor_then_256_then_ansi).
        assert_eq!(tier_for(None, None, None, false), Tier::Ansi);
    }

    #[test]
    fn fixed_tiers_paint_the_ground_and_the_ansi_floor_does_not() {
        assert!(palette(Tier::Truecolor).ground.is_some());
        assert!(palette(Tier::C256).ground.is_some());
        assert!(palette(Tier::Ansi).ground.is_none(), "16 colors keep the user's ground");
        for t in [Tier::Truecolor, Tier::C256, Tier::Ansi] {
            assert_eq!(
                palette(t).ground.is_some(),
                palette(t).ground_rgb.is_some(),
                "cell ground and OSC ground travel together"
            );
        }
    }

    #[test]
    fn the_osc11_sequences_are_exact() {
        assert_eq!(osc11_set_seq((0x12, 0x13, 0x1c)), "\x1b]11;#12131c\x07");
        assert_eq!(
            osc11_restore_seq("rgb:213d/2743/33e7"),
            "\x1b]11;rgb:213d/2743/33e7\x07"
        );
    }

    #[test]
    fn ground_is_not_owned_until_the_terminal_answers() {
        // Nothing in tests runs acquire_ground (it would query the test
        // runner's tty) — the gate must default closed.
        assert!(!ground_owned());
        assert!(release_ground().is_none());
    }

    /// The plan's vectors (peer-recommendations plan §5.1, DS §0.6), within
    /// one step a channel: the web and the phone round on their own.
    #[test]
    fn swatch_matches_the_spec_vectors() {
        let vectors: [(u16, u32); 9] = [
            (203, 0x6ba4c7),
            (39, 0xc7a76b),
            (4, 0xc7716b),
            (352, 0xc76b77),
            (78, 0xabc76b),
            (154, 0x6bc79f),
            (353, 0xc76b76),
            (53, 0xc7bc6b),
            (81, 0xa7c76b),
        ];
        for (hue, hex) in vectors {
            let want = [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8];
            let (r, g, b) = swatch_rgb(hue);
            let close = [r, g, b].into_iter().zip(want).all(|(got, want)| got.abs_diff(want) <= 1);
            assert!(close, "hue {hue}: got {r:02x}{g:02x}{b:02x}, want {hex:06x}");
        }
        assert_eq!(swatch_rgb(203 + 360), swatch_rgb(203), "a hue past 360 wraps");
    }

    #[test]
    fn swatch_is_dim_without_truecolor_or_a_hue() {
        let on = |p: &Theme, hue| swatch_for(p.tier == Tier::Truecolor, p.dim, hue);
        let (r, g, b) = swatch_rgb(203);
        assert_eq!(on(&palette(Tier::Truecolor), Some(203)), Color::Rgb(r, g, b));
        for t in [Tier::Truecolor, Tier::C256, Tier::Ansi] {
            let p = palette(t);
            assert_eq!(p.tier, t, "each palette knows its tier");
            assert_eq!(on(&p, None), p.dim, "{t:?}: no tile, no hue");
        }
        for t in [Tier::C256, Tier::Ansi] {
            let p = palette(t);
            assert_eq!(on(&p, Some(203)), p.dim, "{t:?} cannot hold the hue");
        }
        // The live swatch follows whatever tier this process resolved.
        assert_eq!(truecolor(), th().tier == Tier::Truecolor);
        assert_eq!(swatch(Some(203)), on(th(), Some(203)));
        assert_eq!(swatch(None), th().dim);
    }

    #[test]
    fn busy_glyph_has_a_legacy_stand_in() {
        {
            let _legacy = legacy_glyphs_here();
            assert_eq!(busy_glyph(), "~");
            assert_eq!(swatch_glyph(), "▌", "CP437 has the half block");
        }
        pin_modern_terminal();
        assert_eq!(busy_glyph(), "◌");
        assert_eq!(swatch_glyph(), "▌");
    }

    #[test]
    fn the_256_tier_never_touches_theme_controlled_indexes() {
        let t = palette(Tier::C256);
        for c in [t.accent, t.bright, t.dim, t.gold, t.ok, t.danger, t.text, t.on_accent, t.ground.unwrap()] {
            match c {
                Color::Indexed(i) => assert!(i >= 16, "index {i} is theme-controlled"),
                other => panic!("256 tier must use Indexed, got {other:?}"),
            }
        }
    }
}
