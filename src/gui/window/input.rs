//! The window's keyboard and pointer, spoken as the terminal speaks them.
//!
//! The GUI's input half (`gui::input`) takes crossterm's events: a key is a
//! code and whether Ctrl is held, a pointer event is a cell. winit speaks
//! in layouts, text and physical pixels. Between the two sits [`Raw`], a
//! platform-free record of what the window saw, and a [`Translator`] that
//! turns each one into the crossterm events a terminal would have sent for
//! the same gesture. The winit handlers and the spike's script lever both
//! build `Raw`, so everything from the translator down is the same code
//! whichever one drives it — and the translator's tests need no window.

use ratatui::crossterm::event::{
    Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use winit::event::{ElementState, MouseScrollDelta};
use winit::keyboard::{Key, KeyCode as Code, ModifiersState, NamedKey, PhysicalKey};

/// The keys that have a name rather than a character — the ones the
/// keymap can bind, plus Insert, which crossterm also names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Named {
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    F(u8),
}

impl Named {
    /// A name as the script lever spells it: crossterm's own spellings, and
    /// `BackTab` for Tab with Shift, which is what it becomes.
    pub(super) fn parse(name: &str) -> Option<(Named, bool)> {
        let named = match name {
            "Enter" => Named::Enter,
            "Esc" => Named::Esc,
            "Tab" => Named::Tab,
            "BackTab" => return Some((Named::Tab, true)),
            "Backspace" => Named::Backspace,
            "Delete" => Named::Delete,
            "Insert" => Named::Insert,
            "Up" => Named::Up,
            "Down" => Named::Down,
            "Left" => Named::Left,
            "Right" => Named::Right,
            "Home" => Named::Home,
            "End" => Named::End,
            "PageUp" => Named::PageUp,
            "PageDown" => Named::PageDown,
            "Space" => Named::Space,
            _ => {
                let n: u8 = name.strip_prefix('F')?.parse().ok()?;
                if !(1..=24).contains(&n) {
                    return None;
                }
                Named::F(n)
            }
        };
        Some((named, false))
    }
}

/// The modifiers held with a key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Command on a Mac, the Windows key elsewhere.
    pub logo: bool,
}

impl Mods {
    pub(super) fn from_winit(state: ModifiersState) -> Mods {
        Mods {
            shift: state.shift_key(),
            ctrl: state.control_key(),
            alt: state.alt_key(),
            logo: state.super_key(),
        }
    }

    fn crossterm(self) -> KeyModifiers {
        let mut out = KeyModifiers::NONE;
        if self.shift {
            out |= KeyModifiers::SHIFT;
        }
        if self.ctrl {
            out |= KeyModifiers::CONTROL;
        }
        if self.alt {
            out |= KeyModifiers::ALT;
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    fn crossterm(self) -> MouseButton {
        match self {
            Button::Left => MouseButton::Left,
            Button::Right => MouseButton::Right,
            Button::Middle => MouseButton::Middle,
        }
    }
}

/// How far the wheel turned: whole or fractional lines from a mouse, or
/// pixels from a trackpad. Positive is up — the content moving down to
/// show what is above, winit's sign.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Wheel {
    Lines(f32),
    Pixels(f64),
}

/// One thing the window saw, before it means anything to the GUI.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Raw {
    Key {
        named: Option<Named>,
        /// What the layout made of the key — Shift, Option, a dead key's
        /// accent applied. With Ctrl held it is the platform's guess: the
        /// layout's letter on macOS (Shift applied, Ctrl not), often
        /// nothing on Windows.
        text: Option<String>,
        /// The key with no modifiers at all, in the current layout.
        bare: Option<char>,
        /// The letter printed on the key's place in a US layout, for keys
        /// in the letter block: what Ctrl means on a layout whose letters
        /// are not Latin.
        physical: Option<char>,
        mods: Mods,
        pressed: bool,
        repeat: bool,
    },
    /// Text an input method finished composing.
    ImeCommit(String),
    /// Text an input method is still composing; nothing reaches the GUI
    /// as keys (the window hands it to the kit to draw in the field).
    ImePreedit(String),
    /// The clipboard's text, pasted into the field with the keyboard.
    Paste(String),
    /// The pointer left the window, or the window lost the keyboard: a
    /// button let go out there may never be reported.
    Leave,
    /// The pointer, in physical pixels from the window's top-left.
    Move { x: f64, y: f64 },
    /// A button, at wherever the pointer last was: winit's button events
    /// carry no position.
    Button { button: Button, down: bool },
    Wheel(Wheel),
}

/// The window's surface in physical pixels, and the cells drawn on it.
///
/// There is no cell size here on purpose. ratatui-wgpu draws the cells at
/// the face's size into a texture of whole cells, then its default post
/// processor stretches that texture over the whole surface
/// (`DefaultPostProcessor`, blit.wgsl's `uv = position / screen_size`). A
/// surface a few pixels past a whole number of cells — a window dragged to
/// any size, or one the screen clamped — therefore draws every cell a
/// fraction of a pixel larger than the face's, and the error of a whole
/// pixel cell grows along the rows until the pointer names the row below
/// the one under it. What a pixel shows is its share of the surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Grid {
    pub width: u32,
    pub height: u32,
    pub cols: u16,
    pub rows: u16,
}

impl Grid {
    /// The cell drawn under a pointer position, held inside the grid — a
    /// drag carries on reporting outside the window. The pointer is over
    /// the pixel it falls in, and that pixel shows the texel its centre
    /// samples: `(pixel + ½) / size` of the way across. In integers, so a
    /// pixel on a cell's edge falls the way the shader's does.
    fn cell(&self, x: f64, y: f64) -> (u16, u16) {
        let at = |px: f64, size: u32, count: u16| {
            // Every pixel past the surface's far edge is over the last cell,
            // so the pixel is held to the edge before the arithmetic: a
            // position any size (a drag far outside, or an infinity) then
            // cannot overflow it. The cast saturates, and NaN goes to 0.
            let pixel = (px.max(0.0).floor() as u64).min(u64::from(size));
            let index = (2 * pixel + 1) * u64::from(count) / (2 * u64::from(size.max(1)));
            index.min(u64::from(count.saturating_sub(1))) as u16
        };
        (at(x, self.width, self.cols), at(y, self.height, self.rows))
    }

    /// The pixel at a cell's centre, as the surface draws it: the script's
    /// way to point at a cell.
    pub(super) fn centre(&self, col: u16, row: u16) -> (f64, f64) {
        let at = |index: u16, size: u32, count: u16| {
            (f64::from(index) + 0.5) * f64::from(size) / f64::from(count.max(1))
        };
        (at(col, self.width, self.cols), at(row, self.height, self.rows))
    }

    /// A cell's top-left pixel and its size, as the surface draws it: where
    /// an input method's candidate list is told the caret is.
    pub(super) fn cell_rect(&self, col: u16, row: u16) -> ((u32, u32), (u32, u32)) {
        let at = |index: u16, size: u32, count: u16| {
            (u64::from(index.min(count)) * u64::from(size) / u64::from(count.max(1))) as u32
        };
        let (left, top) = (at(col, self.width, self.cols), at(row, self.height, self.rows));
        let right = at(col.saturating_add(1), self.width, self.cols);
        let bottom = at(row.saturating_add(1), self.height, self.rows);
        ((left, top), (right.saturating_sub(left).max(1), bottom.saturating_sub(top).max(1)))
    }

    /// A row's height as drawn, in pixels: a trackpad's pixels are turned
    /// into lines by it.
    fn row_height(&self) -> f64 {
        f64::from(self.height.max(1)) / f64::from(self.rows.max(1))
    }
}

/// The most scroll events one wheel turn becomes. A notch is a few lines
/// and a hard trackpad fling some tens of rows; a thousand is past any of
/// them, and past every list's length a screen at a time.
const MAX_WHEEL_LINES: f64 = 1000.0;

/// The state a terminal keeps for its mouse reports: where the pointer is,
/// which buttons are down, and the part of a wheel turn too small to be a
/// line yet.
#[derive(Debug, Default)]
pub(super) struct Translator {
    px: (f64, f64),
    /// The last cell a motion was reported for; a move inside it is not
    /// news, as a terminal reports motion once per cell.
    cell: Option<(u16, u16)>,
    held: Vec<Button>,
    /// Buttons a [`Raw::Leave`] reported released while still down: the
    /// real release, if it comes after all, is not reported twice.
    ended: Vec<Button>,
    /// Lines turned and not yet reported, in the direction of the last
    /// turn.
    wheel: f64,
}

impl Translator {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// The pixel the pointer was last reported at.
    pub(super) fn pixel(&self) -> (f64, f64) {
        self.px
    }

    /// The display's scale changed by `ratio`: the pointer's pixel is
    /// the same place on a surface that many times the size.
    pub(super) fn rescale_pixel(&mut self, ratio: f64) {
        if ratio.is_finite() && ratio > 0.0 {
            self.px = (self.px.0 * ratio, self.px.1 * ratio);
        }
    }

    /// The buttons held, as the GUI was told.
    pub(super) fn held(&self) -> &[Button] {
        &self.held
    }

    /// The cell the pointer is over now.
    pub(super) fn pointer(&self, grid: Grid) -> (u16, u16) {
        grid.cell(self.px.0, self.px.1)
    }

    /// What a terminal would have reported for `raw`: none, one or several
    /// events.
    pub(super) fn translate(&mut self, raw: Raw, grid: Grid) -> Vec<TermEvent> {
        match raw {
            Raw::Key { named, text, bare, physical, mods, pressed, repeat: _ } => {
                // A held key's repeats are presses, as a terminal sends
                // them; a release is nothing, as a terminal without the
                // kitty protocol never reports one.
                if !pressed {
                    return Vec::new();
                }
                key(named, text.as_deref(), bare, physical, mods)
            }
            Raw::ImeCommit(text) => text
                .chars()
                .filter(|c| !c.is_control())
                .map(|c| TermEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)))
                .collect(),
            Raw::ImePreedit(_) => Vec::new(),
            Raw::Paste(text) => paste(&text),
            // What a terminal reports when the pointer comes back after a
            // release it never saw: the button up. Here it is said at once,
            // at the last cell the pointer was over, so a drag (a thumb, a
            // track on its way to the queue) ends rather than following
            // the pointer back in with no button held.
            Raw::Leave => {
                let at = self.pointer(grid);
                let held = std::mem::take(&mut self.held);
                for button in &held {
                    if !self.ended.contains(button) {
                        self.ended.push(*button);
                    }
                }
                held.into_iter()
                    .map(|button| mouse(MouseEventKind::Up(button.crossterm()), at))
                    .collect()
            }
            // A position that is not a number names no pixel; winit never
            // sends one, and keeping it would leave the pointer nowhere.
            Raw::Move { x, y } if !(x.is_finite() && y.is_finite()) => Vec::new(),
            Raw::Move { x, y } => {
                self.px = (x, y);
                let cell = grid.cell(x, y);
                if self.cell == Some(cell) {
                    return Vec::new();
                }
                self.cell = Some(cell);
                let kind = match self.held.first() {
                    Some(button) => MouseEventKind::Drag(button.crossterm()),
                    None => MouseEventKind::Moved,
                };
                vec![mouse(kind, cell)]
            }
            Raw::Button { button, down } => {
                let cell = self.pointer(grid);
                // A press or release is also where the pointer is: the
                // motion cell moves with it, so a move within the cell
                // afterwards is not reported again.
                self.cell = Some(cell);
                if down {
                    self.ended.retain(|b| *b != button);
                    if !self.held.contains(&button) {
                        self.held.push(button);
                    }
                    vec![mouse(MouseEventKind::Down(button.crossterm()), cell)]
                } else {
                    if let Some(i) = self.ended.iter().position(|b| *b == button) {
                        self.ended.remove(i);
                        return Vec::new();
                    }
                    self.held.retain(|b| *b != button);
                    vec![mouse(MouseEventKind::Up(button.crossterm()), cell)]
                }
            }
            Raw::Wheel(turn) => {
                let lines = match turn {
                    Wheel::Lines(lines) => f64::from(lines),
                    Wheel::Pixels(px) => px / grid.row_height(),
                };
                // A turn that is not a number would poison the leftover for
                // good, so every later turn came to nothing; it is dropped.
                if !lines.is_finite() {
                    return Vec::new();
                }
                // A turn the other way starts afresh: what was left over
                // from scrolling up must not swallow the first line down.
                if lines * self.wheel < 0.0 {
                    self.wheel = 0.0;
                }
                self.wheel += lines;
                let whole = self.wheel.trunc();
                self.wheel -= whole;
                let kind =
                    if whole > 0.0 { MouseEventKind::ScrollUp } else { MouseEventKind::ScrollDown };
                let at = self.pointer(grid);
                // One event per line, as a terminal reports a wheel, but no
                // more than a turn can mean: past `MAX_WHEEL_LINES` the
                // events would only be a long wait (or, for a huge number,
                // more than memory holds), and the lists stop at their ends
                // long before.
                let lines = whole.abs().min(MAX_WHEEL_LINES) as usize;
                (0..lines).map(|_| mouse(kind, at)).collect()
            }
        }
    }
}

/// The most characters one paste types. Every field is a line — a search,
/// a name, a server's address, a ticket — and the longest thing anyone
/// pastes into one, an iroh ticket, is a few hundred; a clipboard holding a
/// whole document would otherwise become that many key events in one
/// frame, each a field edit and a redraw's worth of work.
pub(super) const PASTE_MAX: usize = 4096;

/// Pasted text as a terminal types it without bracketed paste (which the
/// GUI never asks for): one character a key, no modifiers. Only the first
/// line — every field is one line, and the newline a terminal would type
/// next is an Enter, which would submit the field halfway through; the
/// Unicode line and paragraph separators end the line as a newline does.
/// Control characters and Unicode's format characters are dropped: the
/// invisible ones that ride along in copied text (a zero-width space, a
/// byte-order mark, a soft hyphen, bidi marks and overrides) would sit in
/// a search or an address unseen and make it miss, or reorder how the
/// field draws. Spaces of every kind stay. At most [`PASTE_MAX`]
/// characters are typed.
fn paste(text: &str) -> Vec<TermEvent> {
    let line = text.split(['\n', '\r', '\u{2028}', '\u{2029}']).next().unwrap_or("");
    line.chars()
        .filter(|&c| !c.is_control() && !is_format(c))
        .take(PASTE_MAX)
        .map(|c| TermEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)))
        .collect()
}

/// Whether a character is in Unicode's Format category (Cf), as of Unicode
/// 16: listed here rather than pulled from a properties crate for one
/// question. The zero-width joiner is one, so an emoji family pasted
/// arrives as its members side by side; a field's text is a search or a
/// name, where that costs nothing.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// Whether a key is the platform's paste chord: Cmd+V on a Mac; Ctrl+V,
/// and the Ctrl+Shift+V terminals use, everywhere else. The V is the
/// layout's, or the key's place on a layout without Latin letters, as
/// [`ctrl_letter`] reads Ctrl's keys — which is how both platforms read
/// their shortcuts. Option/Alt held is some other chord.
pub(super) fn is_paste(raw: &Raw) -> bool {
    let Raw::Key { named: None, text, bare, physical, mods, pressed: true, .. } = raw else {
        return false;
    };
    let chord = if cfg!(target_os = "macos") {
        mods.logo && !mods.ctrl
    } else {
        mods.ctrl && !mods.logo
    };
    chord && !mods.alt && ctrl_letter(text.as_deref(), *bare, *physical) == Some('v')
}

/// A key press, as crossterm reports it: a named key as its code, text as
/// one character each — so the layout, Shift and a dead key's accent are
/// the operating system's answer, not a guess here.
fn key(
    named: Option<Named>,
    text: Option<&str>,
    bare: Option<char>,
    physical: Option<char>,
    mods: Mods,
) -> Vec<TermEvent> {
    // Command (or the Windows key) belongs to the platform's own shortcuts
    // — Cmd-Q, Cmd-W, Cmd-C — none of which the player binds. Letting them
    // through would make Cmd-C a bare `c`. A guess: the terminal never
    // sees these either.
    if mods.logo {
        return Vec::new();
    }
    let modifiers = mods.crossterm();
    let press = |code| TermEvent::Key(KeyEvent::new(code, modifiers));
    if let Some(named) = named {
        let code = match named {
            Named::Enter => KeyCode::Enter,
            Named::Esc => KeyCode::Esc,
            // crossterm reports Shift+Tab as its own code, and the keymap
            // binds that code.
            Named::Tab if mods.shift => KeyCode::BackTab,
            Named::Tab => KeyCode::Tab,
            Named::Backspace => KeyCode::Backspace,
            Named::Delete => KeyCode::Delete,
            Named::Insert => KeyCode::Insert,
            Named::Up => KeyCode::Up,
            Named::Down => KeyCode::Down,
            Named::Left => KeyCode::Left,
            Named::Right => KeyCode::Right,
            Named::Home => KeyCode::Home,
            Named::End => KeyCode::End,
            Named::PageUp => KeyCode::PageUp,
            Named::PageDown => KeyCode::PageDown,
            // A terminal sends the space bar as the character.
            Named::Space => KeyCode::Char(' '),
            Named::F(n) => KeyCode::F(n),
        };
        return vec![press(code)];
    }
    if mods.ctrl
        && let Some(letter) = ctrl_letter(text, bare, physical)
    {
        return vec![press(KeyCode::Char(letter))];
    }
    let printable: Vec<char> = text.unwrap_or("").chars().filter(|c| !c.is_control()).collect();
    if printable.is_empty() {
        // Ctrl on a key outside the letters, whose text Ctrl made a
        // control character or nothing: the key without modifiers says
        // which, lowercased as the rest of Ctrl's keys are.
        if mods.ctrl
            && let Some(c) = bare.filter(|c| !c.is_control())
        {
            return vec![press(KeyCode::Char(lower(c)))];
        }
        return Vec::new();
    }
    let lowered = |c: char| if mods.ctrl { lower(c) } else { c };
    printable.into_iter().map(|c| press(KeyCode::Char(lowered(c)))).collect()
}

/// The letter Ctrl is held with, as a terminal reads it: a terminal sends
/// Ctrl+letter as its C0 byte, which has no case and no layout, and
/// crossterm reads the byte back as the lowercase ASCII letter. The
/// layout's own letter comes first when it is Latin, so Ctrl+C is the key
/// that types c on Dvorak or AZERTY; a layout whose letters are not Latin
/// (Russian, Greek, Hebrew) has none, and there the key's place names it,
/// as terminals do — Ctrl+С on a Russian keyboard is still Ctrl+C. winit
/// on macOS gives the layout's letter with Shift applied as the text of a
/// Ctrl key, not the control character, so case is dropped here too.
fn ctrl_letter(text: Option<&str>, bare: Option<char>, physical: Option<char>) -> Option<char> {
    let single = |s: &str| {
        let mut chars = s.chars();
        chars.next().filter(|_| chars.next().is_none())
    };
    let latin = |c: char| c.is_ascii_alphabetic().then(|| c.to_ascii_lowercase());
    // macOS's older answer, and any platform that still sends it: the C0
    // byte itself, 0x01 to 0x1a for a to z.
    let control = |c: char| {
        (('\u{1}'..='\u{1a}').contains(&c)).then(|| char::from(b'a' + (c as u8) - 1))
    };
    text.and_then(single)
        .and_then(|c| latin(c).or_else(|| control(c)))
        .or_else(|| bare.and_then(latin))
        .or_else(|| match bare {
            // The key's place stands in only for a LETTER of another script
            // — Ctrl+ф on a Russian layout is the C0 byte of the Latin key at
            // that place, which is what a terminal sends — and for a key the
            // layout could not name at all. Punctuation stays itself: Ctrl+'
            // on Dvorak is an apostrophe, not the q whose place it took, or
            // the player would quit on it (found at a real keyboard,
            // 2026-09-30).
            Some(c) if c.is_alphabetic() && !c.is_ascii() => physical.and_then(latin),
            None => physical.and_then(latin),
            Some(_) => None,
        })
}

/// A character in lower case, when it has a single-character one.
fn lower(c: char) -> char {
    let mut lowered = c.to_lowercase();
    match (lowered.next(), lowered.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

fn mouse(kind: MouseEventKind, (column, row): (u16, u16)) -> TermEvent {
    TermEvent::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE })
}

// ── From winit ──────────────────────────────────────────────────────────────

/// A winit key event as [`Raw`].
pub(super) fn from_winit_key(event: &winit::event::KeyEvent, mods: ModifiersState) -> Raw {
    let named = match &event.logical_key {
        Key::Named(named) => named_key(*named),
        _ => None,
    };
    Raw::Key {
        named,
        text: event.text.as_ref().map(|text| text.to_string()),
        bare: bare_key(event),
        physical: physical_letter(event.physical_key),
        mods: Mods::from_winit(mods),
        pressed: event.state == ElementState::Pressed,
        repeat: event.repeat,
    }
}

/// The key with no modifiers applied, where winit can say (the desktop
/// platforms); a single character or nothing.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn bare_key(event: &winit::event::KeyEvent) -> Option<char> {
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    match event.key_without_modifiers() {
        Key::Character(s) => {
            let mut chars = s.chars();
            chars.next().filter(|_| chars.next().is_none())
        }
        _ => None,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn bare_key(event: &winit::event::KeyEvent) -> Option<char> {
    match &event.logical_key {
        Key::Character(s) => s.chars().next(),
        _ => None,
    }
}

/// The US-layout letter at a key's place, for the letter block only.
fn physical_letter(key: PhysicalKey) -> Option<char> {
    let PhysicalKey::Code(code) = key else { return None };
    let letter = match code {
        Code::KeyA => 'a',
        Code::KeyB => 'b',
        Code::KeyC => 'c',
        Code::KeyD => 'd',
        Code::KeyE => 'e',
        Code::KeyF => 'f',
        Code::KeyG => 'g',
        Code::KeyH => 'h',
        Code::KeyI => 'i',
        Code::KeyJ => 'j',
        Code::KeyK => 'k',
        Code::KeyL => 'l',
        Code::KeyM => 'm',
        Code::KeyN => 'n',
        Code::KeyO => 'o',
        Code::KeyP => 'p',
        Code::KeyQ => 'q',
        Code::KeyR => 'r',
        Code::KeyS => 's',
        Code::KeyT => 't',
        Code::KeyU => 'u',
        Code::KeyV => 'v',
        Code::KeyW => 'w',
        Code::KeyX => 'x',
        Code::KeyY => 'y',
        Code::KeyZ => 'z',
        _ => return None,
    };
    Some(letter)
}

fn named_key(key: NamedKey) -> Option<Named> {
    Some(match key {
        NamedKey::Enter => Named::Enter,
        NamedKey::Escape => Named::Esc,
        NamedKey::Tab => Named::Tab,
        NamedKey::Backspace => Named::Backspace,
        NamedKey::Delete => Named::Delete,
        NamedKey::Insert => Named::Insert,
        NamedKey::ArrowUp => Named::Up,
        NamedKey::ArrowDown => Named::Down,
        NamedKey::ArrowLeft => Named::Left,
        NamedKey::ArrowRight => Named::Right,
        NamedKey::Home => Named::Home,
        NamedKey::End => Named::End,
        NamedKey::PageUp => Named::PageUp,
        NamedKey::PageDown => Named::PageDown,
        NamedKey::Space => Named::Space,
        NamedKey::F1 => Named::F(1),
        NamedKey::F2 => Named::F(2),
        NamedKey::F3 => Named::F(3),
        NamedKey::F4 => Named::F(4),
        NamedKey::F5 => Named::F(5),
        NamedKey::F6 => Named::F(6),
        NamedKey::F7 => Named::F(7),
        NamedKey::F8 => Named::F(8),
        NamedKey::F9 => Named::F(9),
        NamedKey::F10 => Named::F(10),
        NamedKey::F11 => Named::F(11),
        NamedKey::F12 => Named::F(12),
        // Modifiers on their own, media keys and the rest: nothing a
        // terminal would pass on either.
        _ => return None,
    })
}

pub(super) fn from_winit_button(button: winit::event::MouseButton) -> Option<Button> {
    match button {
        winit::event::MouseButton::Left => Some(Button::Left),
        winit::event::MouseButton::Right => Some(Button::Right),
        winit::event::MouseButton::Middle => Some(Button::Middle),
        // Back, forward and the rest: a terminal reports none of them.
        _ => None,
    }
}

/// The vertical part of a wheel turn; sideways turns mean nothing to the
/// GUI.
pub(super) fn from_winit_wheel(delta: MouseScrollDelta) -> Wheel {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => Wheel::Lines(y),
        MouseScrollDelta::PixelDelta(p) => Wheel::Pixels(p.y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This Mac's window as it opens: 16×32 px cells at scale 2, 100×30 of
    /// them, on a surface of exactly that many pixels.
    const GRID: Grid = Grid { width: 1600, height: 960, cols: 100, rows: 30 };

    fn press(named: Option<Named>, text: Option<&str>, mods: Mods) -> Raw {
        Raw::Key {
            named,
            text: text.map(str::to_string),
            bare: text.and_then(|t| t.chars().next()),
            physical: None,
            mods,
            pressed: true,
            repeat: false,
        }
    }

    fn codes(events: &[TermEvent]) -> Vec<(KeyCode, KeyModifiers)> {
        events
            .iter()
            .map(|e| match e {
                TermEvent::Key(k) => {
                    assert_eq!(k.kind, ratatui::crossterm::event::KeyEventKind::Press);
                    (k.code, k.modifiers)
                }
                other => panic!("not a key: {other:?}"),
            })
            .collect()
    }

    fn mice(events: &[TermEvent]) -> Vec<(MouseEventKind, u16, u16)> {
        events
            .iter()
            .map(|e| match e {
                TermEvent::Mouse(m) => (m.kind, m.column, m.row),
                other => panic!("not a mouse event: {other:?}"),
            })
            .collect()
    }

    fn one(raw: Raw) -> Vec<(KeyCode, KeyModifiers)> {
        codes(&Translator::new().translate(raw, GRID))
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;

    #[test]
    fn named_keys_are_their_codes() {
        let cases = [
            (Named::Enter, KeyCode::Enter),
            (Named::Esc, KeyCode::Esc),
            (Named::Tab, KeyCode::Tab),
            (Named::Backspace, KeyCode::Backspace),
            (Named::Delete, KeyCode::Delete),
            (Named::Insert, KeyCode::Insert),
            (Named::Up, KeyCode::Up),
            (Named::Down, KeyCode::Down),
            (Named::Left, KeyCode::Left),
            (Named::Right, KeyCode::Right),
            (Named::Home, KeyCode::Home),
            (Named::End, KeyCode::End),
            (Named::PageUp, KeyCode::PageUp),
            (Named::PageDown, KeyCode::PageDown),
            (Named::Space, KeyCode::Char(' ')),
            (Named::F(1), KeyCode::F(1)),
            (Named::F(12), KeyCode::F(12)),
        ];
        for (named, code) in cases {
            // winit's text for these ("\r", "\t", "\u{8}", " ") must not
            // add a character of its own.
            let text = match named {
                Named::Enter => Some("\r"),
                Named::Tab => Some("\t"),
                Named::Backspace => Some("\u{8}"),
                Named::Esc => Some("\u{1b}"),
                Named::Space => Some(" "),
                _ => None,
            };
            assert_eq!(one(press(Some(named), text, Mods::default())), [(code, NONE)], "{named:?}");
        }
    }

    #[test]
    fn shift_tab_is_backtab() {
        let shift = Mods { shift: true, ..Mods::default() };
        assert_eq!(
            one(press(Some(Named::Tab), Some("\t"), shift)),
            [(KeyCode::BackTab, KeyModifiers::SHIFT)]
        );
    }

    #[test]
    fn text_is_one_char_each_as_the_layout_made_it() {
        assert_eq!(one(press(None, Some("q"), Mods::default())), [(KeyCode::Char('q'), NONE)]);
        let shift = Mods { shift: true, ..Mods::default() };
        assert_eq!(
            one(press(None, Some("T"), shift)),
            [(KeyCode::Char('T'), KeyModifiers::SHIFT)]
        );
        // A dead key's accent, resolved by the layout, and a key that
        // types two characters at once.
        assert_eq!(one(press(None, Some("é"), Mods::default())), [(KeyCode::Char('é'), NONE)]);
        assert_eq!(
            one(press(None, Some("ab"), Mods::default())),
            [(KeyCode::Char('a'), NONE), (KeyCode::Char('b'), NONE)]
        );
        // A dead key itself has no text: nothing yet.
        assert!(one(press(None, None, Mods::default())).is_empty());
    }

    /// A key with Ctrl held: its text, its bare key, and its place; Shift
    /// is held when the text is a capital.
    fn ctrl_key(text: Option<&str>, bare: char, place: Option<char>) -> Raw {
        Raw::Key {
            named: None,
            text: text.map(str::to_string),
            bare: Some(bare),
            physical: place,
            mods: Mods {
                ctrl: true,
                shift: text.is_some_and(|t| t.chars().any(char::is_uppercase)),
                ..Mods::default()
            },
            pressed: true,
            repeat: false,
        }
    }

    fn ctrl(text: Option<&str>, bare: char, place: Option<char>) -> Vec<(KeyCode, KeyModifiers)> {
        one(ctrl_key(text, bare, place))
    }

    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const CTRL_SHIFT: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::SHIFT);

    #[test]
    fn ctrl_with_punctuation_is_the_punctuation_not_the_keys_place() {
        // Dvorak keeps its apostrophe where QWERTY keeps q. At a real keyboard
        // Ctrl+' reached the GUI as q — and q quits the player, whatever the
        // modifier — because the key's place stood in for every key the layout
        // named with something other than a Latin letter. Punctuation is
        // itself; only a letter of another script takes its place.
        assert_eq!(ctrl(Some("'"), '\'', Some('q')), vec![(KeyCode::Char('\''), KeyModifiers::CONTROL)]);
        assert_eq!(ctrl(None, '\'', Some('q')), vec![(KeyCode::Char('\''), KeyModifiers::CONTROL)]);
        assert_eq!(ctrl(None, 'ф', Some('a')), vec![(KeyCode::Char('a'), KeyModifiers::CONTROL)]);
    }

    #[test]
    fn ctrl_c_arrives_as_c_with_control() {
        let c = KeyCode::Char('c');
        // macOS under winit 0.30: the text of a Ctrl key is the layout's
        // letter with Shift applied, not a control character.
        assert_eq!(ctrl(Some("c"), 'c', Some('c')), [(c, CTRL)]);
        // Shift held too: the capital, lowered as a terminal's C0 byte
        // lowers it, so the quit still matches.
        assert_eq!(ctrl(Some("C"), 'c', Some('c')), [(c, CTRL_SHIFT)]);
        // A platform that sends the C0 byte itself, and Windows, which
        // sends no text at all.
        assert_eq!(ctrl(Some("\u{3}"), 'c', Some('c')), [(c, CTRL)]);
        assert_eq!(ctrl(None, 'c', Some('c')), [(c, CTRL)]);
        // And the handler's own test passes on what comes out.
        let out = Translator::new().translate(ctrl_key(Some("C"), 'c', Some('c')), GRID);
        let TermEvent::Key(k) = &out[0] else { panic!() };
        assert!(k.modifiers.contains(KeyModifiers::CONTROL) && k.code == c);
    }

    #[test]
    fn ctrl_on_a_layout_without_latin_letters_is_the_keys_place() {
        // Russian: the key where C sits types с (Cyrillic es); its text
        // and its bare key are both that, and the key's place says c.
        assert_eq!(ctrl(Some("с"), 'с', Some('c')), [(KeyCode::Char('c'), CTRL)]);
        assert_eq!(ctrl(Some("С"), 'с', Some('c')), [(KeyCode::Char('c'), CTRL_SHIFT)]);
        // Ctrl+D and Ctrl+U, which the keymap binds, the same way (в, г).
        assert_eq!(ctrl(Some("в"), 'в', Some('d')), [(KeyCode::Char('d'), CTRL)]);
        assert_eq!(ctrl(Some("г"), 'г', Some('u')), [(KeyCode::Char('u'), CTRL)]);
    }

    #[test]
    fn ctrl_on_a_latin_layout_is_the_layouts_letter() {
        // Dvorak: the key in QWERTY's I place types c, and Ctrl on it is
        // Ctrl+C — the layout's letter, not the key's place.
        assert_eq!(ctrl(Some("c"), 'c', Some('i')), [(KeyCode::Char('c'), CTRL)]);
        // Windows gives no text; the bare key still carries the layout.
        assert_eq!(ctrl(None, 'c', Some('i')), [(KeyCode::Char('c'), CTRL)]);
        // A key outside the letters keeps its own character.
        assert_eq!(ctrl(Some("1"), '1', None), [(KeyCode::Char('1'), CTRL)]);
        // Without Ctrl the text is what is typed, Cyrillic and all.
        assert_eq!(
            one(Raw::Key {
                named: None,
                text: Some("с".into()),
                bare: Some('с'),
                physical: Some('c'),
                mods: Mods::default(),
                pressed: true,
                repeat: false,
            }),
            [(KeyCode::Char('с'), NONE)]
        );
    }

    #[test]
    fn a_release_is_nothing_and_a_repeat_is_a_press() {
        let mut t = Translator::new();
        let release = Raw::Key {
            named: Some(Named::Down),
            text: None,
            bare: None,
            physical: None,
            mods: Mods::default(),
            pressed: false,
            repeat: false,
        };
        assert!(t.translate(release, GRID).is_empty());
        let repeat = Raw::Key {
            named: None,
            text: Some("j".into()),
            bare: Some('j'),
            physical: Some('j'),
            mods: Mods::default(),
            pressed: true,
            repeat: true,
        };
        assert_eq!(codes(&t.translate(repeat, GRID)), [(KeyCode::Char('j'), NONE)]);
    }

    #[test]
    fn command_keys_stay_with_the_platform() {
        let logo = Mods { logo: true, ..Mods::default() };
        assert!(one(press(None, Some("c"), logo)).is_empty());
        assert!(one(press(Some(Named::Enter), None, logo)).is_empty());
    }

    #[test]
    fn an_ime_commit_is_its_characters() {
        assert_eq!(
            one(Raw::ImeCommit("日本".into())),
            [(KeyCode::Char('日'), NONE), (KeyCode::Char('本'), NONE)]
        );
        assert!(one(Raw::ImePreedit("にほ".into())).is_empty());
    }

    #[test]
    fn pixels_become_cells_at_scale_two() {
        let mut t = Translator::new();
        // The top-left pixel, a cell's last pixel, and the next one.
        assert_eq!(
            mice(&t.translate(Raw::Move { x: 0.0, y: 0.0 }, GRID)),
            [(MouseEventKind::Moved, 0, 0)]
        );
        assert!(t.translate(Raw::Move { x: 15.9, y: 31.9 }, GRID).is_empty(), "same cell");
        assert_eq!(
            mice(&t.translate(Raw::Move { x: 16.0, y: 32.0 }, GRID)),
            [(MouseEventKind::Moved, 1, 1)]
        );
        // Cell (41, 7)'s centre.
        assert_eq!(
            mice(&t.translate(Raw::Move { x: 41.0 * 16.0 + 8.0, y: 7.0 * 32.0 + 16.0 }, GRID)),
            [(MouseEventKind::Moved, 41, 7)]
        );
    }

    #[test]
    fn the_pointer_is_held_inside_the_grid() {
        let mut t = Translator::new();
        // Past the last column and row (a drag outside the window), and
        // before the first.
        assert_eq!(
            mice(&t.translate(Raw::Move { x: 1605.0, y: 961.0 }, GRID)),
            [(MouseEventKind::Moved, 99, 29)]
        );
        assert_eq!(
            mice(&t.translate(Raw::Move { x: 99999.0, y: 5.0 }, GRID)),
            [(MouseEventKind::Moved, 99, 0)]
        );
        assert_eq!(
            mice(&t.translate(Raw::Move { x: -40.0, y: -3.0 }, GRID)),
            [(MouseEventKind::Moved, 0, 0)]
        );
    }

    /// The cell a surface pixel shows, the way ratatui-wgpu draws it: the
    /// cells at the face's size in a texture of whole cells, stretched over
    /// the surface by the default post processor — blit.wgsl samples at
    /// `uv = (pixel + ½) / surface`, nearest texel. Exact arithmetic.
    fn drawn(pixel: u64, surface: u32, cells: u16, cell_px: u64) -> u16 {
        let texture = u64::from(cells) * cell_px;
        let texel = (2 * pixel + 1) * texture / (2 * u64::from(surface));
        (texel / cell_px) as u16
    }

    #[test]
    fn a_surface_between_whole_cells_is_pointed_at_as_it_is_drawn() {
        // The opening size; one a person dragged to (the verifier's
        // 1614×988); rows with 30 and 31 px to spare, where a whole-pixel
        // cell came out 33 px tall; the mini player 20 px too tall; and the
        // tall window the screen clamped to 60 rows and 4 px.
        let surfaces = [
            (1600, 960, 100, 30),
            (1614, 988, 100, 30),
            (1600, 990, 100, 30),
            (1600, 991, 100, 30),
            (1120, 660, 70, 20),
            (1600, 1924, 100, 60),
            (1615, 960, 100, 30),
        ];
        for (width, height, cols, rows) in surfaces {
            let grid = Grid { width, height, cols, rows };
            for pixel in 0..u64::from(width) {
                let want = drawn(pixel, width, cols, 16);
                // Anywhere inside the pixel is that pixel.
                for inside in [0.0, 0.3, 0.999] {
                    let (col, _) = grid.cell(pixel as f64 + inside, 0.0);
                    assert_eq!(col, want, "{width}×{height}: x {pixel}+{inside}");
                }
            }
            for pixel in 0..u64::from(height) {
                let want = drawn(pixel, height, rows, 32);
                for inside in [0.0, 0.5, 0.999] {
                    let (_, row) = grid.cell(0.0, pixel as f64 + inside);
                    assert_eq!(row, want, "{width}×{height}: y {pixel}+{inside}");
                }
            }
            // A cell's centre, as the script points, is that cell.
            for col in 0..cols {
                for row in 0..rows {
                    let (x, y) = grid.centre(col, row);
                    assert_eq!(grid.cell(x, y), (col, row), "{width}×{height}: centre");
                }
            }
        }
    }

    #[test]
    fn the_lower_rows_of_a_stretched_surface_are_not_the_row_below() {
        // 1614×988 draws a row 32.93 px tall: row 25 is pixels 823-856.
        // A 32 px row would put 850 on row 26.
        let grid = Grid { width: 1614, height: 988, cols: 100, rows: 30 };
        let mut t = Translator::new();
        assert_eq!(
            mice(&t.translate(Raw::Move { x: 1515.0, y: 850.0 }, grid)),
            [(MouseEventKind::Moved, 93, 25)]
        );
        // And the grip column at the right: col 93 is pixels 1501-1516.
        assert_eq!(grid.cell(1501.0, 823.5), (93, 25));
        assert_eq!(grid.cell(1517.0, 856.9), (94, 26));
    }

    #[test]
    fn buttons_press_release_and_drag() {
        let mut t = Translator::new();
        let at = |c: f64, r: f64| Raw::Move { x: c * 16.0 + 8.0, y: r * 32.0 + 16.0 };
        t.translate(at(3.0, 4.0), GRID);
        assert_eq!(
            mice(&t.translate(Raw::Button { button: Button::Left, down: true }, GRID)),
            [(MouseEventKind::Down(MouseButton::Left), 3, 4)]
        );
        assert_eq!(
            mice(&t.translate(at(3.0, 6.0), GRID)),
            [(MouseEventKind::Drag(MouseButton::Left), 3, 6)]
        );
        assert_eq!(
            mice(&t.translate(Raw::Button { button: Button::Left, down: false }, GRID)),
            [(MouseEventKind::Up(MouseButton::Left), 3, 6)]
        );
        // Released: motion again.
        assert_eq!(mice(&t.translate(at(3.0, 7.0), GRID)), [(MouseEventKind::Moved, 3, 7)]);
        assert_eq!(
            mice(&t.translate(Raw::Button { button: Button::Right, down: true }, GRID)),
            [(MouseEventKind::Down(MouseButton::Right), 3, 7)]
        );
        assert_eq!(
            mice(&t.translate(at(4.0, 7.0), GRID)),
            [(MouseEventKind::Drag(MouseButton::Right), 4, 7)]
        );
    }

    #[test]
    fn absurd_numbers_neither_crash_nor_stick() {
        let mut t = Translator::new();
        t.translate(Raw::Move { x: 8.0, y: 16.0 }, GRID);
        let up = (MouseEventKind::ScrollUp, 0, 0);
        // Far past the surface is the last cell, however far.
        for far in [1e18, 1e300, f64::MAX] {
            assert_eq!(GRID.cell(far, far), (99, 29), "{far}");
        }
        // A position that is not a place is ignored; the pointer stays put.
        for x in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert!(t.translate(Raw::Move { x, y: 5.0 }, GRID).is_empty(), "{x}");
            assert_eq!(t.pixel(), (8.0, 16.0));
        }
        // A huge turn is a long scroll, not an allocation past memory.
        let long = t.translate(Raw::Wheel(Wheel::Lines(1e30)), GRID);
        assert_eq!(long.len(), 1000);
        assert_eq!(mice(&long[..1]), [up]);
        assert_eq!(t.translate(Raw::Wheel(Wheel::Pixels(-1e300)), GRID).len(), 1000);
        // A turn that is not a number comes to nothing, and the wheel still
        // works after it.
        for turn in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(t.translate(Raw::Wheel(Wheel::Lines(turn)), GRID).is_empty(), "{turn}");
        }
        assert!(t.translate(Raw::Wheel(Wheel::Pixels(f64::NAN)), GRID).is_empty());
        assert!(t.translate(Raw::Wheel(Wheel::Pixels(f64::MAX)), GRID).len() == 1000);
        assert_eq!(mice(&t.translate(Raw::Wheel(Wheel::Lines(2.0)), GRID)), [up, up]);
    }

    #[test]
    fn whole_wheel_lines_scroll_and_fractions_wait() {
        let mut t = Translator::new();
        t.translate(Raw::Move { x: 8.0, y: 16.0 }, GRID);
        let up = (MouseEventKind::ScrollUp, 0, 0);
        let down = (MouseEventKind::ScrollDown, 0, 0);
        assert_eq!(mice(&t.translate(Raw::Wheel(Wheel::Lines(3.0)), GRID)), [up, up, up]);
        assert_eq!(mice(&t.translate(Raw::Wheel(Wheel::Lines(-2.0)), GRID)), [down, down]);
        // Fractions add up to a line.
        assert!(t.translate(Raw::Wheel(Wheel::Lines(0.4)), GRID).is_empty());
        assert!(t.translate(Raw::Wheel(Wheel::Lines(0.4)), GRID).is_empty());
        assert_eq!(mice(&t.translate(Raw::Wheel(Wheel::Lines(0.4)), GRID)), [up]);
        // A trackpad's pixels: one cell height (32 px) is a line.
        assert!(t.translate(Raw::Wheel(Wheel::Pixels(-20.0)), GRID).is_empty());
        assert_eq!(mice(&t.translate(Raw::Wheel(Wheel::Pixels(-20.0)), GRID)), [down]);
        assert_eq!(
            mice(&t.translate(Raw::Wheel(Wheel::Pixels(-70.0)), GRID)),
            [down, down],
            "8 px left over plus 70 is 78: two lines, 14 px kept"
        );
        // Turning back drops the remainder the other way.
        assert!(t.translate(Raw::Wheel(Wheel::Pixels(20.0)), GRID).is_empty());
        assert_eq!(mice(&t.translate(Raw::Wheel(Wheel::Pixels(12.0)), GRID)), [up]);
    }

    #[test]
    fn the_wheel_scrolls_where_the_pointer_is() {
        let mut t = Translator::new();
        t.translate(Raw::Move { x: 30.0 * 16.0, y: 12.0 * 32.0 }, GRID);
        assert_eq!(
            mice(&t.translate(Raw::Wheel(Wheel::Lines(-1.0)), GRID)),
            [(MouseEventKind::ScrollDown, 30, 12)]
        );
    }

    #[test]
    fn a_leave_lets_go_of_every_held_button_where_the_pointer_was() {
        let mut t = Translator::new();
        let at = |c: f64, r: f64| Raw::Move { x: c * 16.0 + 8.0, y: r * 32.0 + 16.0 };
        // Nothing held: a leave says nothing.
        assert!(t.translate(Raw::Leave, GRID).is_empty());
        t.translate(at(3.0, 4.0), GRID);
        t.translate(Raw::Button { button: Button::Left, down: true }, GRID);
        t.translate(Raw::Button { button: Button::Right, down: true }, GRID);
        t.translate(at(9.0, 4.0), GRID);
        assert_eq!(
            mice(&t.translate(Raw::Leave, GRID)),
            [
                (MouseEventKind::Up(MouseButton::Left), 9, 4),
                (MouseEventKind::Up(MouseButton::Right), 9, 4)
            ]
        );
        // The drag is over: motion is motion, and a second leave is nothing.
        assert_eq!(mice(&t.translate(at(9.0, 5.0), GRID)), [(MouseEventKind::Moved, 9, 5)]);
        assert!(t.translate(Raw::Leave, GRID).is_empty());
        // The real release, arriving late, is not a second Up; the next
        // press and release are reported as ever.
        assert!(t.translate(Raw::Button { button: Button::Left, down: false }, GRID).is_empty());
        assert_eq!(
            mice(&t.translate(Raw::Button { button: Button::Left, down: true }, GRID)),
            [(MouseEventKind::Down(MouseButton::Left), 9, 5)]
        );
        assert_eq!(
            mice(&t.translate(Raw::Button { button: Button::Left, down: false }, GRID)),
            [(MouseEventKind::Up(MouseButton::Left), 9, 5)]
        );
        // The right button's late release is swallowed too, once.
        assert!(t.translate(Raw::Button { button: Button::Right, down: false }, GRID).is_empty());
        assert_eq!(
            mice(&t.translate(Raw::Button { button: Button::Right, down: false }, GRID)),
            [(MouseEventKind::Up(MouseButton::Right), 9, 5)]
        );
    }

    #[test]
    fn a_new_scale_keeps_the_pointer_over_its_cell() {
        let mut t = Translator::new();
        t.translate(Raw::Move { x: 41.0 * 16.0 + 8.0, y: 7.0 * 32.0 + 16.0 }, GRID);
        t.rescale_pixel(0.5);
        let half = Grid { width: 800, height: 480, cols: 100, rows: 30 };
        assert_eq!((t.pixel(), t.pointer(half)), ((332.0, 120.0), (41, 7)));
        t.rescale_pixel(f64::NAN);
        assert_eq!(t.pixel(), (332.0, 120.0), "a ratio that is no number moves nothing");
    }

    #[test]
    fn a_paste_types_its_first_line() {
        assert_eq!(
            one(Raw::Paste("pasted text".into())).len(),
            "pasted text".len(),
        );
        assert_eq!(
            one(Raw::Paste("日本\r\nsecond line".into())),
            [(KeyCode::Char('日'), NONE), (KeyCode::Char('本'), NONE)]
        );
        assert_eq!(
            one(Raw::Paste("a\tb\n".into())),
            [(KeyCode::Char('a'), NONE), (KeyCode::Char('b'), NONE)]
        );
        assert!(one(Raw::Paste("\nafter".into())).is_empty(), "an empty first line types nothing");
    }

    /// A paste types at most [`PASTE_MAX`] characters, drops the invisible
    /// format characters copied text carries, keeps every kind of space,
    /// and ends at a Unicode line or paragraph separator as at a newline.
    #[test]
    fn a_paste_is_capped_and_types_no_invisible_characters() {
        let huge = "x".repeat(PASTE_MAX * 3);
        assert_eq!(one(Raw::Paste(huge)).len(), PASTE_MAX);
        let wide = "日".repeat(PASTE_MAX + 1);
        assert_eq!(one(Raw::Paste(wide)).len(), PASTE_MAX, "characters, not bytes");
        let typed = |text: &str| -> String {
            one(Raw::Paste(text.into()))
                .into_iter()
                .map(|(code, _)| match code {
                    KeyCode::Char(c) => c,
                    other => panic!("{other:?} is not typed text"),
                })
                .collect()
        };
        // A byte-order mark, zero-width space, soft hyphen, bidi override
        // and isolate, word joiner and a tag character: all gone.
        let dirty = "\u{FEFF}mu\u{200B}sic\u{00AD} \u{202E}rev\u{2066}x\u{2069}\u{2060}\u{E0041}";
        assert_eq!(typed(dirty), "music revx");
        // Spaces stay, the no-break and ideographic ones included.
        assert_eq!(typed("a b\u{00A0}c\u{3000}d"), "a b\u{00A0}c\u{3000}d");
        // A line separator ends the line; so does a paragraph separator.
        assert_eq!(typed("first\u{2028}second"), "first");
        assert_eq!(typed("one\u{2029}two"), "one");
        // Format characters do not count toward the cap.
        let padded = "\u{200B}".repeat(10) + &"y".repeat(PASTE_MAX);
        assert_eq!(typed(&padded).len(), PASTE_MAX);
    }

    #[test]
    fn the_paste_chord_is_the_platforms() {
        let key = |text: &str, bare: char, place: Option<char>, mods, pressed| Raw::Key {
            named: None,
            text: Some(text.into()),
            bare: Some(bare),
            physical: place,
            mods,
            pressed,
            repeat: false,
        };
        let press =
            |text: &str, bare: char, place: char, mods| key(text, bare, Some(place), mods, true);
        let logo = Mods { logo: true, ..Mods::default() };
        let ctrl = Mods { ctrl: true, ..Mods::default() };
        let ctrl_shift = Mods { ctrl: true, shift: true, ..Mods::default() };
        let mac = cfg!(target_os = "macos");
        assert_eq!(is_paste(&press("v", 'v', 'v', logo)), mac);
        assert_eq!(is_paste(&press("v", 'v', 'v', ctrl)), !mac);
        assert_eq!(is_paste(&press("V", 'v', 'v', ctrl_shift)), !mac);
        let chord = if mac { logo } else { ctrl };
        // Russian: the key at V's place types м; it is still the chord.
        assert!(is_paste(&press("м", 'м', 'v', chord)));
        assert!(is_paste(&press("V", 'v', 'v', Mods { shift: true, ..chord })));
        // Dvorak: the key at QWERTY's V types k, and v is on the key at
        // QWERTY's period, a place outside the letters — the layout's
        // letter is the chord.
        assert!(!is_paste(&press("k", 'k', 'v', chord)));
        assert!(is_paste(&key("v", 'v', None, chord, true)));
        // Not the chord: a bare v, another letter, Alt held, a release.
        assert!(!is_paste(&press("v", 'v', 'v', Mods::default())));
        assert!(!is_paste(&press("c", 'c', 'c', chord)));
        assert!(!is_paste(&press("v", 'v', 'v', Mods { alt: true, ..chord })));
        assert!(!is_paste(&key("v", 'v', Some('v'), chord, false)));
    }

    #[test]
    fn a_cells_rect_is_its_share_of_the_surface() {
        assert_eq!(GRID.cell_rect(0, 0), ((0, 0), (16, 32)));
        assert_eq!(GRID.cell_rect(41, 7), ((656, 224), (16, 32)));
        // A stretched surface: row 25 of 988 px is pixels 823-855.
        let grid = Grid { width: 1614, height: 988, cols: 100, rows: 30 };
        assert_eq!(grid.cell_rect(93, 25), ((1501, 823), (16, 33)));
        // A cell past the grid is held to its far edge, one pixel big.
        assert_eq!(GRID.cell_rect(500, 500).0, (1600, 960));
    }

    #[test]
    fn script_names_parse() {
        assert_eq!(Named::parse("Enter"), Some((Named::Enter, false)));
        assert_eq!(Named::parse("BackTab"), Some((Named::Tab, true)));
        assert_eq!(Named::parse("F5"), Some((Named::F(5), false)));
        assert_eq!(Named::parse("F0"), None);
        assert_eq!(Named::parse("Return"), None);
    }
}
