//! One table per mode, and the help screen is rendered from it.
//!
//! One table per mode, and the help screen is rendered from it. Keeping a
//! second hand-written list of "what the keys are" meant the help drifted
//! from the truth: it still advertised four tabs after a fifth was added.
//!
//! Out of app.rs since audit #55: none of this reads App state, it is
//! reached only through [`Keymap::action`], and it is the part of the
//! input story most likely to keep growing.

use crate::input::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{Action, InputMode};

/// A key press, modifier included. Carrying the modifier is what stops
/// `Ctrl+D` from being mistaken for `d` — which it was, quietly deleting a
/// queue entry when a vim user reached for half-page-down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
}

pub(crate) const fn key(code: KeyCode) -> Key {
    Key { code, ctrl: false }
}
pub(crate) const fn ch(c: char) -> Key {
    Key { code: KeyCode::Char(c), ctrl: false }
}
pub(crate) const fn ctrl(c: char) -> Key {
    Key { code: KeyCode::Char(c), ctrl: true }
}

impl Key {
    /// How this key is written on the help screen.
    pub fn label(self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::Backspace => "Bksp".to_string(),
            KeyCode::Up => "↑".to_string(),
            KeyCode::Down => "↓".to_string(),
            KeyCode::Left => "←".to_string(),
            KeyCode::Right => "→".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PgUp".to_string(),
            KeyCode::PageDown => "PgDn".to_string(),
            other => format!("{other:?}"),
        };
        if self.ctrl { format!("^{base}") } else { base }
    }
}

impl Key {
    /// How this key is written in config.toml. Chosen to be typeable —
    /// `ctrl+d` rather than the `^d` the help screen shows.
    pub fn spec(self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "enter".to_string(),
            KeyCode::Esc => "esc".to_string(),
            KeyCode::Tab => "tab".to_string(),
            KeyCode::Backspace => "backspace".to_string(),
            KeyCode::Up => "up".to_string(),
            KeyCode::Down => "down".to_string(),
            KeyCode::Left => "left".to_string(),
            KeyCode::Right => "right".to_string(),
            KeyCode::Home => "home".to_string(),
            KeyCode::End => "end".to_string(),
            KeyCode::PageUp => "pageup".to_string(),
            KeyCode::PageDown => "pagedown".to_string(),
            other => format!("{other:?}").to_lowercase(),
        };
        if self.ctrl { format!("ctrl+{base}") } else { base }
    }

    /// Read a key back out of config.toml. Accepts the spelling [`Key::spec`]
    /// writes plus the obvious near-misses, since this is hand-edited.
    pub fn parse(spec: &str) -> Option<Key> {
        let spec = spec.trim();
        // `ctrl+d`, `Ctrl-D` and the help screen's own `^d` all mean one thing.
        let (ctrl, rest) = match spec.strip_prefix('^') {
            Some(rest) => (true, rest),
            None => {
                let lower = spec.to_ascii_lowercase();
                match lower.strip_prefix("ctrl+").or_else(|| lower.strip_prefix("ctrl-")) {
                    // The suffix is taken from the original so a bound
                    // capital survives the lowercasing done to find it.
                    Some(rest) => (true, &spec[spec.len() - rest.len()..]),
                    None => (false, spec),
                }
            }
        };
        if rest.is_empty() {
            return None;
        }
        let code = match rest.to_ascii_lowercase().as_str() {
            "space" => KeyCode::Char(' '),
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" | "bksp" => KeyCode::Backspace,
            "up" | "↑" => KeyCode::Up,
            "down" | "↓" => KeyCode::Down,
            "left" | "←" => KeyCode::Left,
            "right" | "→" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            _ => {
                let mut chars = rest.chars();
                let only = chars.next()?;
                // More than one character and not a name above: not a key.
                if chars.next().is_some() {
                    return None;
                }
                // Case matters for a bare letter — `g` and `G` are different
                // bindings — but not with Ctrl, where the terminal sends the
                // lowercase form either way. Keeping the capital there would
                // make a binding that can never fire.
                if ctrl { KeyCode::Char(only.to_ascii_lowercase()) } else { KeyCode::Char(only) }
            }
        };
        Some(Key { code, ctrl })
    }
}

impl Action {
    /// The name this action answers to in `[keys]`. `None` for actions that
    /// are the app talking to itself rather than something to bind.
    pub fn name(&self) -> Option<&'static str> {
        Some(match self {
            Action::Up => "up",
            Action::Down => "down",
            Action::PageUp => "page-up",
            Action::PageDown => "page-down",
            Action::HalfPageUp => "half-page-up",
            Action::HalfPageDown => "half-page-down",
            Action::First => "first",
            Action::Last => "last",
            Action::Activate => "open",
            Action::Back => "back",
            Action::AddToQueue => "add-to-queue",
            Action::CycleFocus => "switch-pane",
            Action::SelectTab(0) => "tab-1",
            Action::SelectTab(1) => "tab-2",
            Action::SelectTab(2) => "tab-3",
            Action::SelectTab(3) => "tab-4",
            Action::SelectTab(4) => "tab-5",
            Action::SelectTab(5) => "tab-6",
            Action::StartSearch => "search",
            Action::StartFilter => "filter",
            Action::CycleViz => "visualiser-mode",
            Action::ToggleScatter => "visualiser-dots",
            Action::PlayPause => "play-pause",
            Action::NextTrack => "next-track",
            Action::PrevTrack => "previous-track",
            Action::JumpToPlaying => "jump-to-playing",
            Action::ToggleNowPlaying => "now-playing",
            Action::SeekForward => "seek-forward",
            Action::SeekBackward => "seek-back",
            Action::SeekForwardFar => "seek-forward-far",
            Action::SeekBackwardFar => "seek-back-far",
            Action::VolumeUp => "volume-up",
            Action::VolumeDown => "volume-down",
            Action::RemoveFromQueue => "remove-from-queue",
            Action::ClearQueue => "clear-queue",
            Action::ToggleRepeat => "repeat",
            Action::ToggleShuffle => "shuffle",
            Action::ToggleAutoDj => "auto-dj",
            Action::StartJourney => "sonic-path",
            Action::ToggleHelp => "help",
            Action::Quit => "quit",
            // Text entry, and the keys the overlays claim for themselves.
            // Those tables aren't rebindable — the panel and the full-screen
            // view both draw their own hints, so the keys are on screen where
            // they apply rather than in the config file.
            Action::SelectTab(_)
            | Action::Input(_)
            | Action::Backspace
            | Action::Submit
            | Action::Cancel
            | Action::NowTabNext
            | Action::NowTabPrev
            | Action::SelectNowTab(_)
            | Action::NowLeft
            | Action::NowRight => return None,
        })
    }

    pub fn from_name(name: &str) -> Option<Action> {
        // Derived from `name()` rather than a second list, so the two cannot
        // disagree about what anything is called.
        default_normal().into_iter().map(|b| b.action).find(|a| a.name() == Some(name))
    }
}

/// One action and every key that fires it.
#[derive(Debug, Clone)]
pub struct Binding {
    pub keys: Vec<Key>,
    pub action: Action,
    /// What it does, for the help screen. `None` keeps a binding working but
    /// off the list.
    pub help: Option<&'static str>,
}

/// The player's keys. Order is the order the help lists them, so related
/// things are grouped rather than sorted.
fn default_normal() -> Vec<Binding> {
    vec![
    Binding { keys: vec![ch('j'), key(KeyCode::Down)], action: Action::Down, help: Some("move down") },
    Binding { keys: vec![ch('k'), key(KeyCode::Up)], action: Action::Up, help: Some("move up") },
    Binding {
        keys: vec![key(KeyCode::PageDown)],
        action: Action::PageDown,
        help: Some("a screenful down"),
    },
    Binding { keys: vec![key(KeyCode::PageUp)], action: Action::PageUp, help: Some("a screenful up") },
    // Half a page, as in vim — and reachable without a Fn key, which is why
    // it earns its place beside PgUp/PgDn rather than replacing them.
    Binding {
        keys: vec![ctrl('d')],
        action: Action::HalfPageDown,
        help: Some("half that, down"),
    },
    Binding { keys: vec![ctrl('u')], action: Action::HalfPageUp, help: Some("half that, up") },
    Binding {
        keys: vec![ch('g'), key(KeyCode::Home)],
        action: Action::First,
        help: Some("first"),
    },
    Binding { keys: vec![ch('G'), key(KeyCode::End)], action: Action::Last, help: Some("last") },
    Binding {
        keys: vec![key(KeyCode::Enter), ch('l'), key(KeyCode::Right)],
        action: Action::Activate,
        help: Some("open, or play from here"),
    },
    Binding {
        keys: vec![ch('h'), key(KeyCode::Left)],
        action: Action::Back,
        help: Some("go back"),
    },
    Binding { keys: vec![ch('a')], action: Action::AddToQueue, help: Some("add to queue") },
    Binding {
        keys: vec![key(KeyCode::Tab)],
        action: Action::CycleFocus,
        help: Some("show / hide the queue"),
    },
    // One digit per visible tab; `select_tab` ignores a number past the end,
    // so a server without Discover simply has nothing on 4. Listed one per
    // row because "which tab is 3" is the thing a reader actually wants.
    Binding { keys: vec![ch('1')], action: Action::SelectTab(0), help: Some("Files") },
    Binding { keys: vec![ch('2')], action: Action::SelectTab(1), help: Some("Library") },
    Binding { keys: vec![ch('3')], action: Action::SelectTab(2), help: Some("Search") },
    Binding {
        keys: vec![ch('4')],
        action: Action::SelectTab(3),
        help: Some("Discover, if enabled"),
    },
    Binding {
        keys: vec![ch('5')],
        action: Action::SelectTab(4),
        help: Some("Sonic Path, if enabled"),
    },
    // Settings is always available, so on a server with neither discovery
    // tab it slides onto 4 — the strip numbers by position, and the strip
    // is the truth.
    Binding { keys: vec![ch('6')], action: Action::SelectTab(5), help: Some("Settings") },
    Binding { keys: vec![ch('/')], action: Action::StartSearch, help: Some("search") },
    // `/` already asks the server. This one narrows what is already on
    // screen, which is a different enough job to want its own key.
    Binding { keys: vec![ch('f')], action: Action::StartFilter, help: Some("filter this list") },
    Binding { keys: vec![ch(' ')], action: Action::PlayPause, help: Some("play or pause") },
    Binding { keys: vec![ch('n')], action: Action::NextTrack, help: Some("next track") },
    Binding { keys: vec![ch('p')], action: Action::PrevTrack, help: Some("previous track") },
    Binding {
        keys: vec![ch('i')],
        action: Action::JumpToPlaying,
        help: Some("jump to what's playing"),
    },
    // Zero reads as "the screen before the tabs": 1..5 are places to go, this
    // is where you already are.
    Binding {
        keys: vec![ch('0')],
        action: Action::ToggleNowPlaying,
        help: Some("full-screen now playing"),
    },
    Binding {
        keys: vec![ch(']')],
        action: Action::SeekForward,
        help: Some("seek 5s forward"),
    },
    Binding { keys: vec![ch('[')], action: Action::SeekBackward, help: Some("seek 5s back") },
    Binding {
        keys: vec![ch('}')],
        action: Action::SeekForwardFar,
        help: Some("seek a minute forward"),
    },
    Binding {
        keys: vec![ch('{')],
        action: Action::SeekBackwardFar,
        help: Some("seek a minute back"),
    },
    Binding { keys: vec![ch('+'), ch('=')], action: Action::VolumeUp, help: Some("louder") },
    Binding { keys: vec![ch('-')], action: Action::VolumeDown, help: Some("quieter") },
    Binding {
        keys: vec![ch('d')],
        action: Action::RemoveFromQueue,
        help: Some("remove from queue"),
    },
    Binding { keys: vec![ch('C')], action: Action::ClearQueue, help: Some("clear the queue") },
    Binding { keys: vec![ch('r')], action: Action::ToggleRepeat, help: Some("repeat") },
    Binding { keys: vec![ch('s')], action: Action::ToggleShuffle, help: Some("shuffle") },
    Binding { keys: vec![ch('A')], action: Action::ToggleAutoDj, help: Some("auto-dj on / off") },
    // The shortcut into the Sonic Path tab: it fills the destination in
    // rather than being a second way of plotting the same arc.
    Binding {
        keys: vec![ch('J')],
        action: Action::StartJourney,
        help: Some("sonic path to here"),
    },
    Binding {
        keys: vec![ch('?'), key(KeyCode::Esc)],
        action: Action::ToggleHelp,
        help: Some("this help"),
    },
    Binding { keys: vec![ch('q'), ctrl('c')], action: Action::Quit, help: Some("quit") },
    ]
}

/// Keys while a modal overlay is up. It gets its own set rather than
/// borrowing the player's: sharing them meant `p` arrived as "previous
/// track" and the panel's own sample key did nothing.
///
/// Not configurable, deliberately: a panel draws its own hints along the
/// bottom, and those would start lying the moment its keys could move.
fn default_panel() -> Vec<Binding> {
    vec![
    Binding { keys: vec![ch('j'), key(KeyCode::Down)], action: Action::Down, help: None },
    Binding { keys: vec![ch('k'), key(KeyCode::Up)], action: Action::Up, help: None },
    Binding {
        keys: vec![ch('h'), ch('['), key(KeyCode::Left)],
        action: Action::Back,
        help: None,
    },
    Binding {
        keys: vec![ch('l'), ch(']'), key(KeyCode::Right)],
        action: Action::SeekForward,
        help: None,
    },
    Binding { keys: vec![ch('g'), key(KeyCode::Home)], action: Action::First, help: None },
    Binding { keys: vec![ch('G'), key(KeyCode::End)], action: Action::Last, help: None },
    Binding { keys: vec![key(KeyCode::Enter)], action: Action::Submit, help: None },
    Binding { keys: vec![ch(' ')], action: Action::PlayPause, help: None },
    Binding { keys: vec![key(KeyCode::Esc), ch('D')], action: Action::Cancel, help: None },
    Binding { keys: vec![ch('q'), ctrl('c')], action: Action::Quit, help: None },
    ]
}

/// What the full-screen view claims for itself. Everything absent from here
/// falls through to the normal bindings, which is the point: this list is only
/// the keys that mean something different once the browser is off screen.
fn default_now() -> Vec<Binding> {
    vec![
    // The numbers are the navigation here, as they are on the browser
    // screen. ←→ used to switch tabs with Auto-DJ as the exception — which
    // put the only escape key that mattered on the only screen where it was
    // different, and left people stuck on it. A key that means one thing
    // everywhere beats a key that means the right thing four times out of
    // five. Outside this view these digits pick browser tabs; in here that
    // change would happen behind a screen nobody can see.
    Binding { keys: vec![ch('1')], action: Action::SelectNowTab(0), help: None },
    Binding { keys: vec![ch('2')], action: Action::SelectNowTab(1), help: None },
    Binding { keys: vec![ch('3')], action: Action::SelectNowTab(2), help: None },
    Binding { keys: vec![ch('4')], action: Action::SelectNowTab(3), help: None },
    Binding { keys: vec![ch('5')], action: Action::SelectNowTab(4), help: None },
    // Kept as a second way round, and as the one that needs no counting.
    Binding { keys: vec![key(KeyCode::Tab)], action: Action::NowTabNext, help: None },
    Binding { keys: vec![key(KeyCode::BackTab)], action: Action::NowTabPrev, help: None },
    // Handed to whichever tab is in front. Auto-DJ adjusts the row under
    // the cursor with them; nothing else wants them.
    Binding {
        keys: vec![key(KeyCode::Right), ch('l')],
        action: Action::NowRight,
        help: None,
    },
    Binding { keys: vec![key(KeyCode::Left), ch('h')], action: Action::NowLeft, help: None },
    // The visualiser's own tab, on its own key: `←→` already belongs to the
    // panel's tabs and `↑↓` to whatever list is in them.
    Binding { keys: vec![ch('v')], action: Action::CycleViz, help: None },
    // A dot, for the mode that draws dots. `s` and `p` would have been
    // the mnemonic picks and both are taken by transport keys that this
    // view deliberately lets through.
    Binding { keys: vec![ch('.')], action: Action::ToggleScatter, help: None },
    // Esc leaves, because a screen filling the terminal should close the way
    // every other full-screen thing does. `0` still toggles.
    Binding { keys: vec![key(KeyCode::Esc)], action: Action::ToggleNowPlaying, help: None },
    ]
}

/// The bindings in force: the defaults, with whatever `[keys]` in config.toml
/// had to say about them.
#[derive(Debug, Clone)]
pub struct Keymap {
    normal: Vec<Binding>,
    panel: Vec<Binding>,
    /// Claimed by the full-screen view, which then falls through to `normal`.
    /// Only the keys whose meaning actually changes go here; everything else
    /// keeps working there, including whatever `[keys]` moved it to.
    now: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap { normal: default_normal(), panel: default_panel(), now: default_now() }
    }
}

impl Keymap {
    /// Layer a config's `[keys]` over the defaults.
    ///
    /// Naming an action **replaces** its keys rather than adding to them, so
    /// a binding can be moved or removed outright (`action = []`). Everything
    /// unmentioned keeps its default.
    ///
    /// A key the user asks for is taken from whatever held it before: binding
    /// `d` to something new should not also require unbinding it from the old
    /// thing. Problems are collected rather than raised — a typo in one line
    /// must not cost someone their music player.
    pub fn with_overrides(
        mut self,
        overrides: &std::collections::BTreeMap<String, Vec<String>>,
    ) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        // Who has asked for each key. First claim wins: two lines wanting one
        // key is a contradiction, and cancelling both — which is what
        // stripping each in turn does — would leave the key doing nothing at
        // all, which is nobody's intent.
        let mut claimed: std::collections::HashMap<Key, Action> = std::collections::HashMap::new();

        for (name, specs) in overrides {
            let Some(action) = Action::from_name(name) else {
                warnings.push(format!("[keys] has no action called '{name}'"));
                continue;
            };
            let Some(slot) = self.normal.iter().position(|b| b.action == action) else {
                warnings.push(format!("[keys] '{name}' cannot be rebound"));
                continue;
            };
            let mut keys = Vec::new();
            for spec in specs {
                let Some(key) = Key::parse(spec) else {
                    warnings.push(format!("[keys] '{name}': '{spec}' is not a key"));
                    continue;
                };
                match claimed.get(&key) {
                    Some(first) if *first != action => {
                        warnings.push(format!(
                            "[keys] '{spec}' is bound to both {} and {name} — keeping {}",
                            first.name().unwrap_or("?"),
                            first.name().unwrap_or("?"),
                        ));
                        continue;
                    }
                    _ => {
                        claimed.insert(key, action.clone());
                        keys.push(key);
                    }
                }
            }
            // A line whose every key was unreadable is a mistake, not a
            // request to unbind — that is what an explicit `[]` is for. Leave
            // the default alone; the warning already says what went wrong.
            if keys.is_empty() && !specs.is_empty() {
                warnings.push(format!("[keys] '{name}' left as it was"));
                continue;
            }
            self.normal[slot].keys = keys;
        }

        // Take each claimed key off whatever held it before, so binding `d`
        // to something new doesn't also mean unbinding it from the old thing
        // by hand.
        for (key, owner) in &claimed {
            for binding in &mut self.normal {
                if binding.action != *owner {
                    binding.keys.retain(|k| k != key);
                }
            }
        }

        (self, warnings)
    }

    /// What a key press means in this mode.
    pub fn action(&self, key: KeyEvent, mode: InputMode) -> Option<Action> {
        let pressed = Key { code: key.code, ctrl: key.modifiers.contains(KeyModifiers::CONTROL) };
        // Ctrl+C quits from anywhere, including mid-typing, and is not
        // rebindable — it is the way out when everything else is confusing.
        if pressed == ctrl('c') {
            return Some(Action::Quit);
        }

        if mode == InputMode::Editing {
            return match key.code {
                KeyCode::Char(c) => Some(Action::Input(c)),
                KeyCode::Backspace => Some(Action::Backspace),
                KeyCode::Enter => Some(Action::Submit),
                KeyCode::Esc => Some(Action::Cancel),
                KeyCode::Tab => Some(Action::CycleFocus),
                KeyCode::Down => Some(Action::Down),
                KeyCode::Up => Some(Action::Up),
                _ => None,
            };
        }

        // The full-screen view is a view, not a modal: it takes the handful of
        // keys whose meaning changes there and lets the rest through, so
        // pause, skip and seek go on working and go on obeying `[keys]`.
        if mode == InputMode::Now {
            if let Some(binding) = self.now.iter().find(|b| b.keys.contains(&pressed)) {
                return Some(binding.action.clone());
            }
        }

        let table = if mode == InputMode::Panel { &self.panel } else { &self.normal };
        if let Some(binding) = table.iter().find(|b| b.keys.contains(&pressed)) {
            return Some(binding.action.clone());
        }

        // Anything else typed into a panel is the panel's own business (the
        // Auto-DJ sample key, say). Ctrl combinations are not: an unbound one
        // must do nothing rather than arrive as a bare letter.
        if mode == InputMode::Panel && !pressed.ctrl {
            if let KeyCode::Char(c) = key.code {
                return Some(Action::Input(c));
            }
        }
        None
    }

    /// The rows the help screen draws, in table order.
    pub fn help_rows(&self) -> Vec<(String, &'static str)> {
        self.normal
            .iter()
            .filter_map(|binding| {
                let what = binding.help?;
                let keys = binding.keys.iter().map(|k| k.label()).collect::<Vec<_>>().join(" ");
                // A binding with every key taken away has nothing to show.
                (!keys.is_empty()).then_some((keys, what))
            })
            .collect()
    }

    /// The whole map as a `[keys]` section, ready to paste into config.toml.
    pub fn to_config_toml(&self) -> String {
        let mut out = String::from(
            "# The full-screen view and the genre chooser keep their own keys.\n\
             # Naming an action replaces its keys; give it [] to unbind it.\n\
             [keys]\n",
        );
        for binding in &self.normal {
            let Some(name) = binding.action.name() else { continue };
            let keys: Vec<String> =
                binding.keys.iter().map(|k| format!("\"{}\"", k.spec())).collect();
            out.push_str(&format!("{name} = [{}]\n", keys.join(", ")));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bindable_action_has_a_name_and_finds_its_way_back() {
        for binding in default_normal() {
            let name = binding
                .action
                .name()
                .unwrap_or_else(|| panic!("{:?} is bound but unnameable", binding.action));
            assert_eq!(
                Action::from_name(name),
                Some(binding.action.clone()),
                "'{name}' does not resolve back to what it names"
            );
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "'{name}' is not a kebab-case name"
            );
        }
    }

    #[test]
    fn every_binding_is_reachable_and_unambiguous() {
        // Two rows claiming the same key means the second is dead code, and
        // which one wins depends on table order — worth catching here rather
        // than as "that key stopped working".
        for table in [default_normal(), default_panel()] {
            let mut seen = std::collections::HashMap::new();
            for binding in &table {
                for key in &binding.keys {
                    let previous = seen.insert(*key, binding.action.clone());
                    assert!(
                        previous.is_none(),
                        "{} is bound twice: {:?} and {:?}",
                        key.label(),
                        previous.unwrap(),
                        binding.action
                    );
                }
                assert!(!binding.keys.is_empty(), "{:?} has no key", binding.action);
            }
        }
    }
}
