//! The mood Filters (mStream #1092; library-rooms contract, clauses 30–33;
//! discovery-page contract, clause 33): `filters ▾`, a word control on the
//! Albums and Artists rooms' controls line and in the Discover header, and
//! the panel it opens under them — hidden until asked for. The panel's
//! choices are words, three scales and the tags: the chosen ones in the ok
//! colour, the others dim, each a click — a pointer control, as the Genres
//! switch is (clause 27). Choosing asks for the list again in place.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::api::types::{MOOD_TAGS, MoodFilter, MoodScale};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{dim, width};
use crate::tui::worker::LibraryNode;

use super::{ALBUMS_NAV, ARTISTS_NAV, Act, DISCOVER_NAV, Gui, accent, bright_bold, put};

/// The Albums and Artists rooms' filters — the Discover room's is the
/// App's (`taste.moods`), which its asks read — kept while the app runs,
/// and the room whose panel is open.
#[derive(Debug, Clone, Default)]
pub(crate) struct MoodRooms {
    pub albums: MoodFilter,
    pub artists: MoodFilter,
    /// The nav row whose panel is open: hidden by default, and closed by
    /// any move to a room, that one's own row included (clause 32).
    pub open: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoodAct {
    /// `filters ▾`: open or close the panel.
    Toggle,
    /// A scale's value: chosen, or let go when it was the one chosen.
    Pick(MoodScale, &'static str),
    /// A tag on, or off.
    Tag(&'static str),
    /// Every group let go.
    Clear,
}

/// Cells between two groups on one of the panel's rows.
const GAP: u16 = 3;

/// Whether the room on screen offers Filters (clause 30): the Albums wall
/// and the Artists list at their roots, on a server with scores to filter
/// by; the Discover room where its taste route takes the filter. A peer's
/// session has none: its capabilities are the parent's business.
pub(crate) fn offered(gui: &Gui) -> bool {
    let caps = &gui.app.capabilities;
    match gui.active {
        ALBUMS_NAV => caps.mood_filters && matches!(gui.app.library_stack.here(), LibraryNode::Albums(_)),
        ARTISTS_NAV => caps.mood_filters && matches!(gui.app.library_stack.here(), LibraryNode::Artists(_)),
        DISCOVER_NAV => caps.taste_moods(),
        _ => false,
    }
}

/// The room's filter.
pub(crate) fn filter(gui: &Gui) -> MoodFilter {
    match gui.active {
        ALBUMS_NAV => gui.moods.albums,
        ARTISTS_NAV => gui.moods.artists,
        DISCOVER_NAV => gui.app.taste.moods,
        _ => MoodFilter::NONE,
    }
}

pub(crate) fn is_open(gui: &Gui) -> bool {
    gui.moods.open == Some(gui.active) && offered(gui)
}

/// The dot that says the room's filter is set, and the chevron — each led
/// by a space beside the word, bare in the glyphs-alone form.
fn set_dot(spaced: bool) -> &'static str {
    match (legacy_conhost(), spaced) {
        (true, true) => " *",
        (true, false) => "*",
        (false, true) => " ●",
        (false, false) => "●",
    }
}

fn chevron(open: bool, spaced: bool) -> &'static str {
    let glyph = match (legacy_conhost(), open) {
        (true, false) => " v",
        (true, true) => " ^",
        (false, false) => " ▾",
        (false, true) => " ▴",
    };
    if spaced { glyph } else { &glyph[1..] }
}

/// The control's width in cells for `label` (its word, or none for the
/// glyphs alone): the word, the dot while the filter is `set`, the chevron.
pub(crate) fn toggle_width_of(label: &str, set: bool, word: bool) -> u16 {
    let label = if word { width(label) } else { 0 };
    let dot = if set { width(set_dot(word)) } else { 0 };
    (label + dot + width(chevron(false, word))) as u16
}

pub(crate) fn toggle_width(gui: &Gui, word: bool) -> u16 {
    toggle_width_of(&t!("gui.moods.filters"), !filter(gui).is_empty(), word)
}

/// `filters ● ▾` from `x` on row `y` (clause 31): the word in the row's
/// weight, the dot in the ok colour while the room's filter is set — so a
/// closed panel still says the list is filtered — and the chevron dim, `▴`
/// while the panel is open. A click opens or closes the panel. Without
/// `word`, the glyphs alone (the Discover header's last step).
pub(crate) fn draw_toggle(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, word: bool) {
    let label = t!("gui.moods.filters").to_string();
    let set = !filter(gui).is_empty();
    let open = is_open(gui);
    let rect = Rect { x, y, width: toggle_width(gui, word), height: 1 };
    let hover = gui.ui.hovers(rect);
    let mut at = x;
    if word {
        put(frame, at, y, &label, if hover { bright_bold() } else { Style::default() });
        at += width(&label) as u16;
    }
    if set {
        put(frame, at, y, set_dot(word), Style::default().fg(th().ok));
        at += width(set_dot(word)) as u16;
    }
    put(frame, at, y, chevron(open, word), if hover { bright_bold() } else { dim() });
    gui.ui.click(rect, Act::Mood(MoodAct::Toggle));
    // Named while closed: open, its tip would stand over the panel's first
    // row, under the pointer that just opened it.
    if !open {
        gui.ui.tip(rect, t!("gui.moods.tip").to_string());
    }
}

/// A choice's word in `locale` — passed, never switched, so a test can
/// lay the panel out in every language while others read English.
fn label(word: &str, locale: &str) -> String {
    match word {
        "happy" => t!("gui.moods.happy", locale = locale),
        "neutral" => t!("gui.moods.neutral", locale = locale),
        "sad" => t!("gui.moods.sad", locale = locale),
        "relaxed" => t!("gui.moods.relaxed", locale = locale),
        "aggressive" => t!("gui.moods.aggressive", locale = locale),
        "instrumental" => t!("gui.moods.instrumental", locale = locale),
        "vocals" => t!("gui.moods.vocals", locale = locale),
        "acoustic" => t!("gui.moods.acoustic", locale = locale),
        "party" => t!("gui.moods.party", locale = locale),
        "danceable" => t!("gui.moods.danceable", locale = locale),
        _ => return word.to_string(),
    }
    .to_string()
}

/// One word of the panel where it stands, from the panel's top left: what
/// it says, its click (none for a separator), and whether it is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
    row: u16,
    x: u16,
    text: String,
    act: Option<MoodAct>,
    on: bool,
}

/// The panel's words flowed into `w` cells (clause 32): the scales and the
/// tags in order, `·` between a group's words and three cells between
/// groups, a group wrapped whole to the next row where it does not fit —
/// and `clear` at the last row's right while the filter is set. Returns the
/// words and the rows they take.
fn layout(f: MoodFilter, w: u16, locale: &str) -> (Vec<Word>, u16) {
    let sep = " · ";
    let mut groups: Vec<Vec<(String, MoodAct, bool)>> = MoodScale::ALL
        .iter()
        .map(|&scale| {
            scale
                .choices()
                .iter()
                .map(|&v| (label(v, locale), MoodAct::Pick(scale, v), f.value_of(scale) == Some(v)))
                .collect()
        })
        .collect();
    groups.push(MOOD_TAGS.iter().map(|&t| (label(t, locale), MoodAct::Tag(t), f.has_tag(t))).collect());

    let mut words = Vec::new();
    let (mut x, mut row) = (0u16, 0u16);
    for group in groups {
        let group_w = group.iter().map(|(text, ..)| width(text) as u16).sum::<u16>()
            + width(sep) as u16 * (group.len() as u16 - 1);
        if x > 0 && x + GAP + group_w > w {
            (x, row) = (0, row + 1);
        } else if x > 0 {
            x += GAP;
        }
        for (i, (text, act, on)) in group.into_iter().enumerate() {
            if i > 0 {
                words.push(Word { row, x, text: sep.to_string(), act: None, on: false });
                x += width(sep) as u16;
            }
            let text_w = width(&text) as u16;
            words.push(Word { row, x, text, act: Some(act), on });
            x += text_w;
        }
    }
    if !f.is_empty() {
        let clear = t!("gui.moods.clear", locale = locale).to_string();
        let clear_w = width(&clear) as u16;
        if x + GAP + clear_w > w {
            row += 1;
        }
        words.push(Word { row, x: w.saturating_sub(clear_w), text: clear, act: Some(MoodAct::Clear), on: false });
    }
    (words, row + 1)
}

/// The rows the open panel takes in a content `w` cells wide; none while
/// it is closed. The rooms stand their lists that many rows lower.
pub(crate) fn panel_height(gui: &Gui, w: u16) -> u16 {
    if is_open(gui) { layout(filter(gui), w, &rust_i18n::locale()).1 } else { 0 }
}

/// The open panel from row `y` in `content`'s width.
pub(crate) fn draw_panel(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    if !is_open(gui) {
        return;
    }
    let (words, _) = layout(filter(gui), content.width, &rust_i18n::locale());
    for word in words {
        let (x, at_y) = (content.x + word.x, y + word.row);
        let Some(act) = word.act else {
            put(frame, x, at_y, &word.text, dim());
            continue;
        };
        let rect = Rect { x, y: at_y, width: width(&word.text) as u16, height: 1 };
        let style = if gui.ui.hovers(rect) {
            bright_bold()
        } else if act == MoodAct::Clear {
            accent()
        } else if word.on {
            Style::default().fg(th().ok).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        put(frame, x, at_y, &word.text, style);
        gui.ui.click(rect, Act::Mood(act));
    }
}

/// The Albums and Artists rooms' share (clauses 31–32): the control at
/// the controls line's right — where a list of tracks wears its verbs, and
/// these lists have none — unless the filter's field holds the line, and
/// the open panel under that line. Nothing where the room does not offer
/// Filters.
pub(crate) fn draw_for_list(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    if !offered(gui) {
        return;
    }
    if !gui.app.filtering {
        let w = toggle_width(gui, true);
        draw_toggle(frame, gui, content.right().saturating_sub(w + 1), content.y + 1, true);
    }
    draw_panel(frame, gui, content, content.y + 2);
}

/// A click of the control or a choice (clauses 31–33). Nothing outside a
/// room that offers Filters.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    let Act::Mood(mood) = act else { return false };
    if !offered(gui) {
        return true;
    }
    let now = filter(gui);
    let next = match *mood {
        MoodAct::Toggle => {
            gui.moods.open = if is_open(gui) { None } else { Some(gui.active) };
            return true;
        }
        MoodAct::Pick(scale, value) => now.press(scale, value),
        MoodAct::Tag(tag) => now.toggle_tag(tag),
        MoodAct::Clear => MoodFilter::NONE,
    };
    if next != now {
        set(gui, next);
    }
    true
}

/// The room's filter becomes `next`, and the room asks again under it, in
/// place: the wall from its first page, the list from its top, Discover a
/// new batch — what is on screen stays until the answer lands.
fn set(gui: &mut Gui, next: MoodFilter) {
    match gui.active {
        ALBUMS_NAV => {
            gui.moods.albums = next;
            gui.albums.wall.page = 0;
            gui.albums.wall.stow();
            let effects = gui.app.switch_moods(next);
            gui.pend(effects);
        }
        ARTISTS_NAV => {
            gui.moods.artists = next;
            gui.library.view.scroll = 0;
            gui.library.view.reveal = false;
            gui.library.view.stow();
            let effects = gui.app.switch_moods(next);
            gui.pend(effects);
        }
        DISCOVER_NAV => {
            gui.app.taste.moods = next;
            let effects = gui.app.taste_ask_again();
            gui.pend(effects);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(words: &[Word]) -> Vec<String> {
        let last = words.iter().map(|w| w.row).max().unwrap_or(0);
        (0..=last)
            .map(|r| {
                let mut line = String::new();
                for w in words.iter().filter(|w| w.row == r) {
                    while (line.chars().count() as u16) < w.x {
                        line.push(' ');
                    }
                    line.push_str(&w.text);
                }
                line
            })
            .collect()
    }

    #[test]
    fn the_panel_flows_its_groups_whole_and_puts_clear_at_the_last_rows_right() {
        let (words, h) = layout(MoodFilter::NONE, 80, "en");
        assert_eq!(h, 2);
        assert_eq!(
            rows(&words),
            vec![
                "Happy · Neutral · Sad   Relaxed · Neutral · Aggressive   Instrumental · Vocals",
                "Acoustic · Party · Danceable",
            ]
        );
        // The queue open: 47 cells, one group a row.
        let relaxed = MoodFilter::NONE.press(MoodScale::Energy, "relaxed");
        let (words, h) = layout(relaxed, 47, "en");
        assert_eq!(h, 4);
        let lines = rows(&words);
        assert_eq!(lines[1], "Relaxed · Neutral · Aggressive");
        assert_eq!(lines[3], format!("Acoustic · Party · Danceable{}clear", " ".repeat(47 - 28 - 5)));
        let on: Vec<&str> = words.iter().filter(|w| w.on).map(|w| w.text.as_str()).collect();
        assert_eq!(on, vec!["Relaxed"]);
        assert!(words.iter().any(|w| w.act == Some(MoodAct::Pick(MoodScale::Energy, "aggressive"))));
        assert!(words.iter().any(|w| w.act == Some(MoodAct::Tag("party"))));
    }

    #[test]
    fn every_locale_fits_the_panel_in_the_queues_47_cells() {
        // Each language by `t!`'s locale argument — never by switching the
        // process-wide locale, which the tests beside this one read.
        for code in ["en", "de", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
            let all = MoodFilter::NONE.press(MoodScale::Valence, "happy").toggle_tag("acoustic");
            let (words, _) = layout(all, 47, code);
            for w in &words {
                assert!(w.x + width(&w.text) as u16 <= 47, "{code}: {:?} runs past the content", w.text);
            }
        }
    }
}
