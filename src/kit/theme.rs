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
//! lies about its capabilities. The player TUI is deliberately untouched.
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Truecolor,
    C256,
    Ansi,
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
            dim: Color::Rgb(0x69, 0x71, 0x8f),
            gold: Color::Rgb(0xe5, 0xc0, 0x7b),
            ok: Color::Rgb(0x98, 0xc3, 0x79),
            danger: Color::Rgb(0xe0, 0x6c, 0x75),
            text: Color::Rgb(0xd8, 0xde, 0xe9),
            ground: Some(Color::Rgb(0x12, 0x13, 0x1c)),
            ground_rgb: Some((0x12, 0x13, 0x1c)),
            on_accent: Color::Rgb(0x0d, 0x10, 0x17),
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
        },
    }
}

/// The wizard's palette, resolved once.
pub fn th() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| palette(tier()))
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
