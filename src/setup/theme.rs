//! The setup wizard's FIXED color scheme — scoped to the wizard alone.
//!
//! The wizard is mStream's brand moment, so unlike the player (which
//! inherits the user's terminal theme through `ui::Theme`), these screens
//! ship one look everywhere a terminal can carry it. Three tiers, resolved
//! once at startup:
//!
//!   truecolor — the design canvas's literal hexes, when the terminal
//!               advertises 24-bit color (`COLORTERM=truecolor|24bit`).
//!   256-color — hand-tuned indexes from the standardized 6x6x6 cube
//!               (16..=231) and grayscale ramp; NEVER the first 16
//!               indexes, which terminal themes repaint. This is what
//!               Apple Terminal gets — it has no truecolor.
//!   ansi      — the named-color floor (the pre-fixed-scheme behavior):
//!               the wizard still works on a 16-color console.
//!
//! `MSTREAM_SETUP_THEME=ansi|256|truecolor` overrides detection — the
//! test matrix's lever, and the escape hatch for anyone whose terminal
//! lies about its capabilities. The player TUI is deliberately untouched.

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
    /// The painted background. `None` on the ansi tier — a 16-color
    /// terminal keeps its own ground, everything else still reads.
    pub ground: Option<Color>,
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
pub fn tier_for(
    override_var: Option<&str>,
    colorterm: Option<&str>,
    term: Option<&str>,
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
    Tier::Ansi
}

fn tier() -> Tier {
    tier_for(
        std::env::var("MSTREAM_SETUP_THEME").ok().as_deref(),
        std::env::var("COLORTERM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
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
            on_accent: Color::Black,
        },
    }
}

/// The wizard's palette, resolved once.
pub fn th() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| palette(tier()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_override_beats_detection() {
        assert_eq!(tier_for(Some("ansi"), Some("truecolor"), Some("xterm-256color")), Tier::Ansi);
        assert_eq!(tier_for(Some("256"), Some("truecolor"), None), Tier::C256);
        assert_eq!(tier_for(Some("truecolor"), None, Some("vt100")), Tier::Truecolor);
        assert_eq!(tier_for(Some("garbage"), None, Some("xterm-256color")), Tier::C256);
    }

    #[test]
    fn detection_ladders_truecolor_then_256_then_ansi() {
        assert_eq!(tier_for(None, Some("truecolor"), Some("xterm")), Tier::Truecolor);
        assert_eq!(tier_for(None, Some("24bit"), None), Tier::Truecolor);
        assert_eq!(tier_for(None, None, Some("xterm-256color")), Tier::C256);
        assert_eq!(tier_for(None, None, Some("screen-256color")), Tier::C256);
        assert_eq!(tier_for(None, None, Some("xterm")), Tier::Ansi);
        assert_eq!(tier_for(None, None, None), Tier::Ansi);
    }

    #[test]
    fn fixed_tiers_paint_the_ground_and_the_ansi_floor_does_not() {
        assert!(palette(Tier::Truecolor).ground.is_some());
        assert!(palette(Tier::C256).ground.is_some());
        assert!(palette(Tier::Ansi).ground.is_none(), "16 colors keep the user's ground");
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
