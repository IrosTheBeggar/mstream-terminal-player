//! The spike's scripted input: `MSTREAM_WINDOW_SCRIPT=<file>`, hidden.
//!
//! Nothing on this Mac may send the window synthetic keystrokes or clicks
//! (that needs Accessibility), so the spike proves input by playing a
//! script instead. Each line becomes the same [`Raw`] events the winit
//! handlers build, so the translator and everything below it runs exactly
//! as it would for a person; a few commands only look. One line per
//! command, `#` starts a comment:
//!
//! - `wait <ms>`: nothing for that long
//! - `key <Name>`: a named key pressed and released — Enter, Esc, Tab,
//!   BackTab, Up, Down, Left, Right, Backspace, Delete, Insert, Home, End,
//!   PageUp, PageDown, Space, F1..F24
//! - `text <utf8>`: one key per character, the character as its text
//! - `ctrl <char> [<place>]`: a key with Ctrl held, as winit 0.30 reports
//!   it on macOS: its text is the layout's character with Shift applied
//!   (`ctrl C` is Ctrl+Shift+C). `<place>` is the US letter at the key's
//!   place, for a layout whose letters are not Latin: `ctrl с c` is the
//!   Russian layout's Ctrl+С. It defaults to the character's own letter.
//! - `alt <key> [<text>]` / `ctrlalt <key> [<text>]`: the letter key `<key>`
//!   with Alt (Option) held, or with Ctrl and Alt (a left Ctrl+Alt, which
//!   Windows reads as AltGr), and `<text>` as what the OS composed for it:
//!   `alt l ¬` is a Mac's Option+L, `alt q @` a German AltGr+Q, `ctrlalt q`
//!   a US Ctrl+Alt+Q, which composes nothing
//! - `ime <utf8>` / `preedit <utf8>`: an input method's commit, or its
//!   composition in progress
//! - `move <col>,<row>`: the pointer to the centre of that cell, as the
//!   surface draws it
//! - `movepx <x>,<y>` / `clickpx <x>,<y>`: the pointer to a physical pixel
//!   of the surface (then a left click) — a place read off a screenshot,
//!   which the window's own grid had no part in choosing
//! - `click <col>,<row>` / `rclick <col>,<row>`: move there, then the left
//!   (right) button down and up
//! - `press <col>,<row>` / `release`: a `click` in halves, move there and
//!   the left button down, then up where the pointer is — for a run that
//!   dumps the frame between them
//! - `drag <c1>,<r1> <c2>,<r2> [leave]`: down at the first cell, a few
//!   moves, up at the second — or, with `leave`, no up: the pointer leaves
//!   the window there instead, as a release outside it would look
//! - `leave`: the pointer leaves the window (or the window loses focus)
//! - `wheel <lines> <col>,<row>`: move there, then turn the wheel; positive
//!   is up, winit's sign
//! - `resize <w>,<h>`: ask the platform for a surface that size in
//!   physical pixels, as a drag on the window's corner would
//! - `scale <factor>`: what the window does when the platform says the
//!   display's scale changed (a move to another screen), at that factor:
//!   the type re-sized and the window re-fitted to the same grid
//! - `minimise <ms>`: the window minimised (as Cmd+M or the yellow button
//!   does it), and restored that long after by the loop's own clock, which
//!   runs whether or not the platform delivers redraws to a hidden window.
//!   The script goes on meanwhile, as far as frames come to run it: a
//!   `dump` every second after it shows whether they do
//! - `frame`: nothing until the next frame, which comes when the loop's own
//!   wait says (a `wait` would bring it forward): how a run reads the
//!   cadence, from the dumps on either side
//! - `freeze <ms>`: no frame for that long, so the one just drawn stays on
//!   screen for a screenshot — the frame a modal opens on, say, which the
//!   next (10 ms later) would otherwise replace
//! - `dump <path>`: the window's text, then the pointer's state, the
//!   covers painted as pictures and the last press: its cell, the action
//!   the GUI's hit found there (or none) and whether a drag began
//! - `say <text>`: the text on stderr, for whatever is watching
//! - `quit`: as if the close button was pressed
//!
//! One step runs per frame, so what an input did is drawn before the next
//! one lands — the hover a click needs, the room a key opened — as it
//! would be for hands that are never faster than a frame. A line that
//! does not parse is reported on stderr and skipped.
//!
//! The script starts as the window is made, not at its first frame: on
//! this Mac the two are a few tens of milliseconds apart, but on a slow
//! GPU the window stands blank for a second or more while its renderer is
//! built, and a script is how a run types into it then. A `wait` is time
//! from its own step, as it always was. Before the first frame the steps
//! that need none run on the loop's own clock — `wait`, `say`, `resize`,
//! `quit`, and keys, text and pointer steps that name pixels rather than
//! cells; their inputs are held and replayed after the first frame, one
//! act a frame, as a person's are (held.rs). The first step that needs a frame or a grid
//! (`move`, `click`, `dump`, `frame`, `scale`, `minimise`, `freeze`)
//! waits for the first frame.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use super::input::{Button, Mods, Named, Raw, Wheel};

/// What one step does. Pointer positions stay in cells until the step
/// runs, when the grid the window holds then turns them into pixels.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Step {
    Wait(Duration),
    Inputs(Vec<Input>),
    Dump(PathBuf),
    Say(String),
    /// A surface size to ask the platform for, in physical pixels.
    Resize(u32, u32),
    /// A display scale factor, as if the window had moved to that screen.
    Scale(f64),
    /// Minimised for this long, then restored.
    Minimise(Duration),
    /// The next frame, at the loop's pace.
    Frame,
    /// No frame for this long.
    Freeze(Duration),
    Quit,
}

impl Step {
    /// Whether the step can run before the window's first frame, while
    /// its renderer is being built: it needs no grid (a cell is a place
    /// only on one) and nothing drawn.
    pub(super) fn needs_no_frame(&self) -> bool {
        match self {
            Step::Wait(_) | Step::Say(_) | Step::Resize(..) | Step::Quit => true,
            Step::Inputs(inputs) => inputs.iter().all(|input| matches!(input, Input::Raw(_))),
            Step::Dump(_) | Step::Scale(_) | Step::Minimise(_) | Step::Frame | Step::Freeze(_) => {
                false
            }
        }
    }
}

/// A [`Raw`] waiting for the grid.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Input {
    Raw(Raw),
    MoveTo(u16, u16),
}

pub(super) struct Script {
    steps: VecDeque<Step>,
}

impl Script {
    /// The script `MSTREAM_WINDOW_SCRIPT` names, if one does and it reads.
    pub(super) fn from_env() -> Option<Script> {
        let path = std::env::var_os("MSTREAM_WINDOW_SCRIPT")?;
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let (steps, errors) = parse(&text);
                for error in &errors {
                    eprintln!("gui --window: script: {error}");
                }
                let path = path.to_string_lossy();
                eprintln!("gui --window: script {path} with {} steps", steps.len());
                Some(Script { steps: steps.into() })
            }
            Err(e) => {
                eprintln!("gui --window: script {}: {e}", path.to_string_lossy());
                None
            }
        }
    }

    pub(super) fn next(&mut self) -> Option<Step> {
        self.steps.pop_front()
    }

    /// A step taken that could not run yet, back at the front.
    pub(super) fn push_front(&mut self, step: Step) {
        self.steps.push_front(step);
    }

    pub(super) fn is_done(&self) -> bool {
        self.steps.is_empty()
    }
}

/// The steps a script's text makes, and a message for each line that
/// makes none.
pub(super) fn parse(text: &str) -> (Vec<Step>, Vec<String>) {
    let mut steps = Vec::new();
    let mut errors = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match parse_line(trimmed) {
            Ok(mut more) => steps.append(&mut more),
            Err(why) => errors.push(format!("line {}: {why}: {line:?}", n + 1)),
        }
    }
    (steps, errors)
}

fn parse_line(line: &str) -> Result<Vec<Step>, String> {
    // The argument is everything after the first space, kept as written:
    // `text` may want a trailing space, or several.
    let (command, arg) = line.split_once(' ').unwrap_or((line, ""));
    let one = |input: Input| Step::Inputs(vec![input]);
    let tap = |raw: Raw| {
        let Raw::Key { named, text, bare, physical, mods, .. } = raw.clone() else {
            unreachable!()
        };
        let release = Raw::Key { named, text, bare, physical, mods, pressed: false, repeat: false };
        Step::Inputs(vec![Input::Raw(raw), Input::Raw(release)])
    };
    match command {
        "wait" => {
            let ms: u64 = arg.trim().parse().map_err(|_| "wait wants milliseconds")?;
            Ok(vec![Step::Wait(Duration::from_millis(ms))])
        }
        "key" => {
            let (named, shift) = Named::parse(arg.trim()).ok_or("no such key")?;
            // The text winit gives these keys, so the translator sees
            // what a keyboard would give it.
            let text = match named {
                Named::Enter => Some("\r"),
                Named::Tab => Some("\t"),
                Named::Esc => Some("\u{1b}"),
                Named::Backspace => Some("\u{8}"),
                Named::Space => Some(" "),
                _ => None,
            };
            Ok(vec![tap(Raw::Key {
                named: Some(named),
                text: text.map(str::to_string),
                bare: None,
                physical: None,
                mods: Mods { shift, ..Mods::default() },
                pressed: true,
                repeat: false,
            })])
        }
        "text" => {
            if arg.is_empty() {
                return Err("text wants something to type".into());
            }
            Ok(arg
                .chars()
                .map(|c| {
                    // A space is the space bar, a named key in winit.
                    let named = (c == ' ').then_some(Named::Space);
                    tap(Raw::Key {
                        named,
                        text: Some(c.to_string()),
                        bare: c.to_lowercase().next(),
                        physical: c.is_ascii_alphabetic().then(|| c.to_ascii_lowercase()),
                        mods: Mods { shift: c.is_uppercase(), ..Mods::default() },
                        pressed: true,
                        repeat: false,
                    })
                })
                .collect())
        }
        "ctrl" => {
            let mut words = arg.split_whitespace();
            let one_char = |word: Option<&str>| {
                let mut chars = word?.chars();
                chars.next().filter(|_| chars.next().is_none())
            };
            let c = one_char(words.next()).ok_or("ctrl wants one character")?;
            let place = match words.next() {
                None => c.is_ascii_alphabetic().then(|| c.to_ascii_lowercase()),
                Some(word) => Some(
                    one_char(Some(word))
                        .filter(char::is_ascii_alphabetic)
                        .ok_or("a key's place is one letter a to z")?
                        .to_ascii_lowercase(),
                ),
            };
            if words.next().is_some() {
                return Err("ctrl wants a character and at most a place".into());
            }
            // winit 0.30 on macOS: with Ctrl held the text is the logical
            // key — the layout's character with Shift applied, Ctrl not —
            // and the bare key is the layout's character with nothing.
            Ok(vec![tap(Raw::Key {
                named: None,
                text: Some(c.to_string()),
                bare: c.to_lowercase().next(),
                physical: place,
                mods: Mods { ctrl: true, shift: c.is_uppercase(), ..Mods::default() },
                pressed: true,
                repeat: false,
            })])
        }
        "alt" | "ctrlalt" => {
            let mut words = arg.split_whitespace();
            let key = words
                .next()
                .and_then(|word| {
                    let mut chars = word.chars();
                    chars.next().filter(|c| c.is_ascii_alphabetic() && chars.next().is_none())
                })
                .ok_or("alt wants a letter key")?
                .to_ascii_lowercase();
            let text = words.next().map(str::to_string);
            if words.next().is_some() {
                return Err("alt wants a key and at most the text it composed".into());
            }
            let ctrl = command == "ctrlalt";
            Ok(vec![tap(Raw::Key {
                named: None,
                text,
                bare: Some(key),
                physical: Some(key),
                mods: Mods { ctrl, alt: true, ..Mods::default() },
                pressed: true,
                repeat: false,
            })])
        }
        "ime" | "preedit" => {
            if arg.is_empty() {
                return Err("wants text".into());
            }
            let raw = if command == "ime" {
                Raw::ImeCommit(arg.to_string())
            } else {
                Raw::ImePreedit(arg.to_string())
            };
            Ok(vec![one(Input::Raw(raw))])
        }
        "move" => {
            let (col, row) = cell(arg.trim())?;
            Ok(vec![one(Input::MoveTo(col, row))])
        }
        "click" | "rclick" => {
            let (col, row) = cell(arg.trim())?;
            let button = if command == "click" { Button::Left } else { Button::Right };
            Ok(vec![
                one(Input::MoveTo(col, row)),
                one(Input::Raw(Raw::Button { button, down: true })),
                one(Input::Raw(Raw::Button { button, down: false })),
            ])
        }
        "press" => {
            let (col, row) = cell(arg.trim())?;
            Ok(vec![
                one(Input::MoveTo(col, row)),
                one(Input::Raw(Raw::Button { button: Button::Left, down: true })),
            ])
        }
        "release" => Ok(vec![one(Input::Raw(Raw::Button { button: Button::Left, down: false }))]),
        "movepx" | "clickpx" => {
            let (x, y) = pixel(arg.trim())?;
            let mut steps = vec![one(Input::Raw(Raw::Move { x, y }))];
            if command == "clickpx" {
                for down in [true, false] {
                    steps.push(one(Input::Raw(Raw::Button { button: Button::Left, down })));
                }
            }
            Ok(steps)
        }
        "resize" => {
            let (width, height) = arg.trim().split_once(',').ok_or("resize wants <w>,<h>")?;
            let size = |n: &str| n.trim().parse::<u32>().ok().filter(|n| *n > 0);
            let (Some(width), Some(height)) = (size(width), size(height)) else {
                return Err("resize wants two sizes in pixels".into());
            };
            Ok(vec![Step::Resize(width, height)])
        }
        "drag" => {
            let words: Vec<&str> = arg.split_whitespace().collect();
            let (from, to, leave) = match words[..] {
                [from, to] => (from, to, false),
                [from, to, "leave"] => (from, to, true),
                _ => return Err("drag wants two cells, then at most `leave`".into()),
            };
            let (from, to) = (cell(from)?, cell(to)?);
            let mut steps = vec![
                one(Input::MoveTo(from.0, from.1)),
                one(Input::Raw(Raw::Button { button: Button::Left, down: true })),
            ];
            // Four moves along the way, the last on the target, as a hand
            // crosses cells rather than jumping.
            const HOPS: i32 = 4;
            for hop in 1..=HOPS {
                let along = |a: u16, b: u16| {
                    (i32::from(a) + (i32::from(b) - i32::from(a)) * hop / HOPS) as u16
                };
                steps.push(one(Input::MoveTo(along(from.0, to.0), along(from.1, to.1))));
            }
            let end =
                if leave { Raw::Leave } else { Raw::Button { button: Button::Left, down: false } };
            steps.push(one(Input::Raw(end)));
            Ok(steps)
        }
        "wheel" => {
            let (lines, at) = arg.trim().split_once(' ').ok_or("wheel wants <lines> <col>,<row>")?;
            let lines: f32 = lines
                .trim()
                .parse()
                .ok()
                .filter(|lines: &f32| lines.is_finite())
                .ok_or("wheel wants a number of lines")?;
            let (col, row) = cell(at.trim())?;
            Ok(vec![
                one(Input::MoveTo(col, row)),
                one(Input::Raw(Raw::Wheel(Wheel::Lines(lines)))),
            ])
        }
        "dump" => {
            if arg.trim().is_empty() {
                return Err("dump wants a path".into());
            }
            Ok(vec![Step::Dump(PathBuf::from(arg.trim()))])
        }
        "leave" => Ok(vec![one(Input::Raw(Raw::Leave))]),
        "scale" => {
            // A screen's scale is a small positive number; past 8 no display
            // goes, and a type size from it would be past any texture.
            let factor: f64 = arg
                .trim()
                .parse()
                .ok()
                .filter(|f: &f64| f.is_finite() && (0.25..=8.0).contains(f))
                .ok_or("scale wants a factor from 0.25 to 8")?;
            Ok(vec![Step::Scale(factor)])
        }
        "say" => Ok(vec![Step::Say(arg.to_string())]),
        "minimise" | "minimize" => {
            let ms: u64 = arg.trim().parse().map_err(|_| "minimise wants milliseconds")?;
            Ok(vec![Step::Minimise(Duration::from_millis(ms))])
        }
        "frame" => Ok(vec![Step::Frame]),
        "freeze" => {
            let ms: u64 = arg.trim().parse().map_err(|_| "freeze wants milliseconds")?;
            Ok(vec![Step::Freeze(Duration::from_millis(ms))])
        }
        "quit" => Ok(vec![Step::Quit]),
        _ => Err("no such command".into()),
    }
}

fn cell(arg: &str) -> Result<(u16, u16), String> {
    let (col, row) = arg.split_once(',').ok_or("a cell is <col>,<row>")?;
    let col = col.trim().parse().map_err(|_| "a column is a number")?;
    let row = row.trim().parse().map_err(|_| "a row is a number")?;
    Ok((col, row))
}

fn pixel(arg: &str) -> Result<(f64, f64), String> {
    let (x, y) = arg.split_once(',').ok_or("a pixel is <x>,<y>")?;
    // `parse` takes "inf" and "NaN" too; neither is a place on a screen.
    let number = |n: &str| n.trim().parse::<f64>().ok().filter(|n| n.is_finite());
    let x = number(x).ok_or("x is a number")?;
    let y = number(y).ok_or("y is a number")?;
    Ok((x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_parses_and_bad_lines_are_skipped() {
        let text = "\
# a comment
wait 500
key Down
text ab
ctrl c
click 3,4
bogus
wheel -3 10,2
move x,1
drag 0,0 8,4
dump out.txt
quit
";
        let (steps, errors) = parse(text);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("line 7") && errors[1].contains("line 9"), "{errors:?}");
        assert_eq!(steps[0], Step::Wait(Duration::from_millis(500)));
        // key: one step, a press and its release.
        let Step::Inputs(key) = &steps[1] else { panic!() };
        assert_eq!(key.len(), 2);
        // text ab: a step per character.
        assert!(matches!(&steps[2], Step::Inputs(v) if matches!(&v[0],
            Input::Raw(Raw::Key { text: Some(t), .. }) if t == "a")));
        assert!(matches!(&steps[3], Step::Inputs(v) if matches!(&v[0],
            Input::Raw(Raw::Key { text: Some(t), .. }) if t == "b")));
        // ctrl c: the letter as macOS's winit gives it, and its place.
        assert!(matches!(&steps[4], Step::Inputs(v) if matches!(&v[0],
            Input::Raw(Raw::Key { text: Some(t), bare: Some('c'), physical: Some('c'), mods, .. })
                if t == "c" && mods.ctrl && !mods.shift)));
        // click: move, down, up.
        assert_eq!(steps[5], Step::Inputs(vec![Input::MoveTo(3, 4)]));
        assert!(matches!(&steps[6], Step::Inputs(v)
            if v[0] == Input::Raw(Raw::Button { button: Button::Left, down: true })));
        // wheel: move, turn.
        assert_eq!(steps[8], Step::Inputs(vec![Input::MoveTo(10, 2)]));
        assert_eq!(steps[9], Step::Inputs(vec![Input::Raw(Raw::Wheel(Wheel::Lines(-3.0)))]));
        // drag: move, down, four moves ending on the target, up.
        assert_eq!(steps[13], Step::Inputs(vec![Input::MoveTo(4, 2)]));
        assert_eq!(steps[15], Step::Inputs(vec![Input::MoveTo(8, 4)]));
        assert_eq!(steps[17], Step::Dump(PathBuf::from("out.txt")));
        assert_eq!(steps[18], Step::Quit);
        assert_eq!(steps.len(), 19);
    }

    #[test]
    fn ctrl_places_pixels_and_sizes_parse() {
        let (steps, errors) = parse(
            "ctrl с c\nctrl C\nctrl с\nctrl с 7\nmovepx 12.5,850\nclickpx 1515,850\n\
             resize 1614,988\nresize 0,5\n",
        );
        // `ctrl с` alone parses (no place to fall back on); a place that is
        // not a letter and a zero size do not.
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("line 4") && errors[1].contains("line 8"), "{errors:?}");
        assert!(matches!(&steps[0], Step::Inputs(v) if matches!(&v[0],
            Input::Raw(Raw::Key { text: Some(t), physical: Some('c'), mods, .. })
                if t == "с" && mods.ctrl)));
        assert!(matches!(&steps[1], Step::Inputs(v) if matches!(&v[0],
            Input::Raw(Raw::Key { text: Some(t), physical: Some('c'), mods, .. })
                if t == "C" && mods.ctrl && mods.shift)));
        assert!(matches!(&steps[2], Step::Inputs(v) if matches!(&v[0],
            Input::Raw(Raw::Key { physical: None, .. }))));
        assert_eq!(steps[3], Step::Inputs(vec![Input::Raw(Raw::Move { x: 12.5, y: 850.0 })]));
        // clickpx: the move, then down and up.
        assert_eq!(steps[4], Step::Inputs(vec![Input::Raw(Raw::Move { x: 1515.0, y: 850.0 })]));
        assert_eq!(steps[6], Step::Inputs(vec![Input::Raw(Raw::Button {
            button: Button::Left,
            down: false,
        })]));
        assert_eq!(steps[7], Step::Resize(1614, 988));
        assert_eq!(steps.len(), 8);
    }

    #[test]
    fn numbers_that_are_no_place_are_refused() {
        let (steps, errors) = parse(
            "wheel inf 1,1\nwheel NaN 1,1\nwheel 1e39 1,1\nmovepx 1e999,5\nclickpx NaN,3\n\
             movepx 5,-inf\nwheel 1e30 1,1\nmovepx 1e18,5\n",
        );
        // 1e39 is past f32's range and parses as an infinity.
        assert_eq!(errors.len(), 6, "{errors:?}");
        for (n, error) in errors.iter().enumerate() {
            assert!(error.starts_with(&format!("line {}:", n + 1)), "{error}");
        }
        // Huge but finite numbers are the translator's to hold.
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[1], Step::Inputs(vec![Input::Raw(Raw::Wheel(Wheel::Lines(1e30)))]));
        assert_eq!(steps[2], Step::Inputs(vec![Input::Raw(Raw::Move { x: 1e18, y: 5.0 })]));
    }

    #[test]
    fn a_drag_may_end_in_a_leave_and_scales_parse() {
        let (steps, errors) = parse(
            "drag 1,1 5,1 leave\nleave\nscale 1.0\nscale 2\nscale 0\nscale NaN\n\
             drag 1,1 2,2 up\n",
        );
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors[0].contains("line 5") && errors[2].contains("line 7"), "{errors:?}");
        // The drag's last step is a leave, not a release.
        assert_eq!(steps[5], Step::Inputs(vec![Input::MoveTo(5, 1)]));
        assert_eq!(steps[6], Step::Inputs(vec![Input::Raw(Raw::Leave)]));
        assert_eq!(steps[7], Step::Inputs(vec![Input::Raw(Raw::Leave)]));
        assert_eq!(steps[8..], [Step::Scale(1.0), Step::Scale(2.0)]);
    }

    #[test]
    fn a_minimise_a_frame_and_a_click_in_halves_parse() {
        let (steps, errors) = parse(
            "minimise 20000\nminimize 5\nminimise\nminimise soon\nframe\npress 3,4\nrelease\n",
        );
        assert_eq!(errors.len(), 2, "{errors:?}");
        let button =
            |down| Step::Inputs(vec![Input::Raw(Raw::Button { button: Button::Left, down })]);
        assert_eq!(
            steps,
            [
                Step::Minimise(Duration::from_secs(20)),
                Step::Minimise(Duration::from_millis(5)),
                Step::Frame,
                Step::Inputs(vec![Input::MoveTo(3, 4)]),
                button(true),
                button(false),
            ]
        );
    }

    /// What may run before the first frame: what names no cell and needs
    /// nothing drawn.
    #[test]
    fn a_freeze_parses_and_only_frameless_steps_run_before_the_first_frame() {
        let (steps, errors) = parse(
            "freeze 3000\nfreeze\nwait 300\ntext /\nclickpx 5,5\nclick 3,4\ndump d\n\
             resize 800,600\nscale 2\nframe\nsay hi\nquit\n",
        );
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(steps[0], Step::Freeze(Duration::from_secs(3)));
        let early: Vec<bool> = steps.iter().map(Step::needs_no_frame).collect();
        assert_eq!(
            early,
            [
                false, // freeze
                true,  // wait
                true,  // text /
                true,  // clickpx: the move,
                true,  // the press
                true,  // and the release, in pixels
                false, // click: a cell,
                true,  // then its press
                true,  // and release
                false, // dump
                true,  // resize
                false, // scale
                false, // frame
                true,  // say
                true,  // quit
            ]
        );
    }

    #[test]
    fn alt_chords_parse_with_the_text_composed_or_none() {
        let (steps, errors) = parse("alt l ¬\nctrlalt q\nalt 1 x\nalt q @ extra\n");
        assert_eq!(errors.len(), 2, "{errors:?}");
        let pressed = |step: &Step| match step {
            Step::Inputs(inputs) => match &inputs[0] {
                Input::Raw(Raw::Key { text, bare, mods, .. }) => (text.clone(), *bare, *mods),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        };
        let alt = Mods { alt: true, ..Mods::default() };
        assert_eq!(pressed(&steps[0]), (Some("¬".into()), Some('l'), alt));
        assert_eq!(pressed(&steps[1]), (None, Some('q'), Mods { ctrl: true, ..alt }));
    }

    #[test]
    fn text_keeps_its_spaces_and_script_keys_translate() {
        let (steps, errors) = parse("text a b\nkey BackTab\nkey F13\nkey Nope\n");
        assert_eq!(errors.len(), 1);
        assert_eq!(steps.len(), 5);
        assert!(matches!(&steps[1], Step::Inputs(v)
            if matches!(&v[0], Input::Raw(Raw::Key { named: Some(Named::Space), .. }))));
        assert!(matches!(&steps[3], Step::Inputs(v)
            if matches!(&v[0], Input::Raw(Raw::Key { named: Some(Named::Tab), mods, .. })
                if mods.shift)));
    }
}
