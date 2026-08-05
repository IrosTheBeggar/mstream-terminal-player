//! The key-event types the app logic consumes, under one name for both
//! targets.
//!
//! Native builds re-export crossterm's types unchanged, so the terminal loop
//! hands what `event::read()` gives it straight to the keymap. The wasm build
//! has no crossterm; it gets data-only stand-ins covering exactly the surface
//! the keymap touches — `code`, and whether Ctrl is held — and the web shell
//! builds them from browser key events.

#[cfg(not(target_arch = "wasm32"))]
pub use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[cfg(target_arch = "wasm32")]
mod wasm {
    /// The keys the bindings can name, plus two (`Delete`, `F`) the browser
    /// can send and the keymap ignores — without at least one variant the
    /// match arms never name, the keymap's catch-alls would be dead code on
    /// this target only. Variants match crossterm's spellings so the keymap
    /// code reads the same on both targets.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum KeyCode {
        Char(char),
        F(u8),
        Enter,
        Esc,
        Tab,
        Backspace,
        Delete,
        Up,
        Down,
        Left,
        Right,
        Home,
        End,
        PageUp,
        PageDown,
    }

    /// Just enough of crossterm's bitflags to answer
    /// `modifiers.contains(KeyModifiers::CONTROL)`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct KeyModifiers(u8);

    impl KeyModifiers {
        pub const NONE: KeyModifiers = KeyModifiers(0);
        pub const SHIFT: KeyModifiers = KeyModifiers(1);
        pub const CONTROL: KeyModifiers = KeyModifiers(2);
        pub const ALT: KeyModifiers = KeyModifiers(4);

        pub fn contains(self, other: KeyModifiers) -> bool {
            self.0 & other.0 == other.0
        }
    }

    impl std::ops::BitOr for KeyModifiers {
        type Output = KeyModifiers;
        fn bitor(self, rhs: KeyModifiers) -> KeyModifiers {
            KeyModifiers(self.0 | rhs.0)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct KeyEvent {
        pub code: KeyCode,
        pub modifiers: KeyModifiers,
    }

    impl KeyEvent {
        pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
            KeyEvent { code, modifiers }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::{KeyCode, KeyEvent, KeyModifiers};
