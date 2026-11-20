//! The taste bar: the header a taste room wears, and the two modals that hang from it. The
//! header is the room's name BOLD at the left and, at the right, the window dropdown, "more
//! options" (the seeds modal) and "↻ new seeds"; the window's list and the seeds modal draw
//! over the room from [`draw_modals`].
//!
//! Discover drew all of it from its own state until the Peer recs room came (peer-recs
//! contract §1.4, REG-HEADER). Now each room implements [`BarRoom`]: it lends the bar its
//! `BarUi`, its `TasteState` (the seeds modal's pins, seeds and drops), its header facts and
//! its verbs, and the bar draws, keys and acts the same for both. Discover's cells, steps,
//! acts and keys are as they were (discovery-page clauses 11, 13–17); what the extraction
//! adds is the room's choice:
//! - a short name, tried after the refresh's words go and before "more options" shortens
//!   (peer-recs contract §1.4's narrow steps);
//! - no mood opener, for a room that passes none;
//! - no refresh tooltip, for a room whose ask key is not Discover's `G`;
//! - [`BarRoom::modal_opened`], called when the window's list or the seeds modal opens: Peer
//!   recs ends a live clip there, because both modals cover the band's row.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::api::types::{SeedSource, TasteOutcome, Track};
use crate::kit::theme::th;
use crate::kit::{ListView, dim, frame_at, modal_close, modal_frame_on, width};
use crate::tui::app::taste::TasteState;
use crate::tui::app::{App, Effect, TASTE_MAX_PINS, TastePinRefusal};

/// The window dropdown's choices, in days (0 = all time) — discovery-page clause 15.
use crate::tui::app::taste::TASTE_WINDOWS as WINDOWS;

/// The window the header shows before any was chosen or answered: the server's own default
/// (discovery-page plan §2.2).
use crate::tui::app::taste::TASTE_WINDOW_DEFAULT as WINDOW_DEFAULT;

use super::super::{Act, Gui, Screen, accent, bright_bold, bullet_glyph, check_glyphs, put, sel};
use super::{HEAD_GAP, age_words, bold, chevron, refresh_glyph, track_words, window_label, window_label_in, word_refusal};

// ── The room's side ─────────────────────────────────────────────────────────

/// What a taste room lends the bar. Every method is the room's own: where its bar state
/// lives, which `TasteState` its seeds modal reads, what its header says, how a bar act
/// becomes the room's `Act`, and what its verbs do.
pub(crate) trait BarRoom {
    /// The room's nav row: the bar's keys and modals are the room's only while it is up.
    const NAV: usize;
    /// The room's bar state.
    fn bar(gui: &mut Gui) -> &mut BarUi;
    fn bar_ref(gui: &Gui) -> &BarUi;
    /// The state the seeds modal reads: its pins, the answer's seeds and the visit's drops.
    fn taste(app: &App) -> &TasteState;
    /// What the header shows this frame.
    fn facts(gui: &Gui) -> HeadFacts;
    /// A bar act as the room's own `Act`, which the shell routes back to [`act`].
    fn wrap(act: BarAct) -> Act;
    /// Whether the seeds modal offers "Pin the playing song" (discovery-page clause 14).
    fn can_pin_playing(app: &App) -> bool;
    /// Pin the playing song; the ask waits for the modal to close.
    fn pin_playing(app: &mut App) -> bool;
    /// Unpin a pin, by its path; the ask waits for the modal to close.
    fn unpin(app: &mut App, path: &str) -> bool;
    /// The seeds modal closed: the hashes unchecked and checked again, and whether a pin came
    /// or went (discovery-page clause 13).
    fn seeds_closed(app: &mut App, dropped: &[String], restored: &[String], pins_changed: bool) -> Vec<Effect>;
    /// A window picked: ask with it, and save it in `[gui] discover_window`, which both rooms
    /// read (peer-recs contract §1.4).
    fn set_window(gui: &mut Gui, window: u32);
    /// "Pin a song…": arm the room's pick and go to Files.
    fn choose_song(gui: &mut Gui);
    /// "↻ new seeds": ask again now.
    fn new_seeds(gui: &mut Gui);
    /// The window's list or the seeds modal opened.
    fn modal_opened(_gui: &mut Gui) {}
}

/// A room's bar state: the window's list or the seeds modal while one is up, and where the
/// window control stood last frame.
#[derive(Default)]
pub(crate) struct BarUi {
    pub(crate) menu: Option<Menu>,
    pub(crate) seeds: Option<SeedsModal>,
    /// The header's window control as last drawn: its list hangs from it.
    window_at: Rect,
    /// The local zone for "today" and "yesterday" in the seeds modal, read once.
    zone: Option<Option<crate::admin::tz::Zone>>,
}

/// A list hanging from a control (the kit's Dropdown grammar), with its keyboard cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Menu {
    /// The header's window list; the cursor indexes the four windows.
    Window(usize),
}

/// The seeds modal's own state (discovery-page clause 13): which rows were toggled, and
/// whether a pin came or went. Nothing is sent until it closes.
#[derive(Debug, Clone, Default)]
pub(crate) struct SeedsModal {
    pub cursor: Option<usize>,
    /// Hashes whose box was flipped: a history seed unchecked (dropped), a dropped one checked
    /// (restored).
    pub flipped: Vec<String>,
    pub pins_changed: bool,
    pub view: ListView,
}

/// What a pointer or key did in the bar. Each room wraps it in its own `Act`
/// ([`BarRoom::wrap`]).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BarAct {
    /// The header's window control, and a pick from its list.
    Window,
    WindowPick(u32),
    /// A click outside the open list closes and keeps (the Dropdown law).
    MenuClose,
    /// The header's "more options" (and `e`): the seeds modal.
    Seeds,
    /// The header's "↻ new seeds": ask again now.
    NewSeeds,
    /// A modal row, clicked: toggle it, or do what it says.
    SeedRow(usize),
    /// A pin's `[X]` in the modal, by its index in the pins.
    SeedUnpin(usize),
    /// Done, Esc or the modal's `[X]`: the changes apply (discovery-page clause 13).
    SeedsDone,
}

/// What the header shows, as the room states it this frame.
#[derive(Debug, Clone, Default)]
pub(crate) struct HeadFacts {
    /// The room's name, BOLD at the header's left.
    pub name: String,
    /// The name's short form, tried before "more options" shortens (peer-recs contract §1.4);
    /// `None`: the room has no such step.
    pub short: Option<String>,
    /// The window the dropdown shows (`TasteState::shown_window`); `None` shows the server's
    /// default.
    pub window: Option<u32>,
    /// The asked window where the shown answer widened it to all time
    /// (`TasteState::widened_shown`): the dropdown reads "Last 30 days → All time".
    pub widened_from: Option<u32>,
    /// The widened fact in words, said before the window's tooltip while the mark stands.
    pub widened: Option<String>,
    /// An ask is out: the refresh reads "asking…" in gold.
    pub asking: bool,
    /// The mood opener's act, where the room offers the filters (discovery-page clause 33);
    /// `None` draws no opener.
    pub moods: Option<Act>,
    /// The refresh's tooltip; `None` gives it none.
    pub refresh_tip: Option<String>,
}

// ── The header ──────────────────────────────────────────────────────────────

/// One way the header's controls can stand (discovery-page clause 17), the widest first: the
/// refresh with its words, the name short or long, "more options" long, the widened window's
/// mark ("Last 30 days → All time") and the room's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HeadStep {
    pub words: bool,
    /// The room's short name in place of its name.
    pub short: bool,
    pub long: bool,
    pub mark: bool,
    pub name: bool,
    /// "options" drawn at all.
    pub more: bool,
}

/// The header's steps, in the drop order (discovery-page clause 17): the refresh's words (its
/// glyph stays; the legacy console has none, so its words always stand), then the short name
/// where the room has one (peer-recs contract §1.4), then "more options" shortens to
/// "options", then a widened window's mark gives way to the window the answer came from, then
/// the room's name — the nav row already names the room — so the controls stand whole at the
/// queue's 47 cells in every locale. With the mood opener in the header (discovery-page clause
/// 33) the legacy console, whose refresh keeps its words, can still be short: last, "options"
/// gives way (its modal is `e`).
pub(crate) fn head_steps(glyph: bool, filters: bool, short: bool) -> Vec<HeadStep> {
    let whole = HeadStep { words: true, short: false, long: true, mark: true, name: true, more: true };
    let mut steps = vec![whole];
    if glyph {
        steps.push(HeadStep { words: false, ..whole });
    }
    if short {
        steps.push(HeadStep { words: !glyph, short: true, ..whole });
    }
    for (mark, name) in [(true, true), (false, true), (false, false)] {
        steps.push(HeadStep { words: !glyph, short, long: false, mark, name, ..whole });
    }
    if filters && !glyph {
        let last = steps[steps.len() - 1];
        steps.push(HeadStep { more: false, ..last });
    }
    steps
}

fn widen_arrow() -> &'static str {
    if crate::kit::theme::legacy_conhost() { "->" } else { "→" }
}

/// The header's words in one language: what [`head_plan`] lays out.
#[derive(Debug, Clone)]
pub(crate) struct HeadWords {
    pub name: String,
    pub short: Option<String>,
    /// The window dropdown's value (discovery-page clauses 11, 15) with a widened answer's mark
    /// and without it: the window asked, or — where the server widened it to all time — the
    /// asked window and where it went, "Last 30 days → All time"; without the mark, the window
    /// the answer came from, "All time". Never a window the answer did not use.
    pub window: (String, String),
    /// The widest of the four windows: the dropdown's slot.
    pub widest: u16,
    /// "more options" and its short "options".
    pub more: (String, String),
    /// "new seeds" and "asking…".
    pub refresh: (String, String),
    /// The mood opener's cells (discovery-page clause 33), where the room offers the filters;
    /// none elsewhere, and the header is as it was.
    pub filters: Option<u16>,
}

impl HeadWords {
    /// The words of `facts` in `locale`. The room's names come as the room wrote them; every
    /// other word is read in `locale`, so a test can lay the header out in each language
    /// without switching the process-wide one.
    pub(crate) fn of(facts: &HeadFacts, locale: &str) -> Self {
        let asked = facts.window.unwrap_or(WINDOW_DEFAULT);
        let window = match facts.widened_from {
            Some(from) => (
                format!("{} {} {}", window_label_in(from, locale), widen_arrow(), window_label_in(0, locale)),
                window_label_in(0, locale),
            ),
            None => (window_label_in(asked, locale), window_label_in(asked, locale)),
        };
        HeadWords {
            name: facts.name.clone(),
            short: facts.short.clone(),
            window,
            widest: WINDOWS.iter().map(|w| width(&window_label_in(*w, locale))).max().unwrap_or(0) as u16,
            more: (
                t!("gui.discover.more_options", locale = locale).to_string(),
                t!("gui.discover.options_short", locale = locale).to_string(),
            ),
            refresh: (
                t!("gui.discover.new_seeds", locale = locale).to_string(),
                t!("gui.discover.asking", locale = locale).to_string(),
            ),
            filters: facts.moods.as_ref().map(|_| width(&super::super::moods::opener_word(locale)) as u16),
        }
    }

    /// The name as `step` draws it.
    pub(crate) fn name_at(&self, step: HeadStep) -> &str {
        match (&self.short, step.short) {
            (Some(short), true) => short,
            _ => &self.name,
        }
    }
}

/// Where the header's parts stand on its row: the step taken, and each control's x and width
/// (the refresh's is its slot's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HeadPlan {
    pub step: HeadStep,
    pub window: (u16, u16),
    /// The mood opener, between the window and "more options".
    pub filters: Option<(u16, u16)>,
    pub more: (u16, u16),
    pub refresh: (u16, u16),
}

/// The header's geometry (discovery-page clause 17) in `content`'s width: the first of
/// [`head_steps`] whose row fits, the controls right-aligned two cells apart. The refresh's
/// slot is as wide as the wider of its words, or the glyph and the two cells it keeps for " …"
/// while asking; the window's slot is the widest window's (or the widened value, when wider).
pub(crate) fn head_plan(words: &HeadWords, content: Rect, glyph: Option<&str>) -> HeadPlan {
    let words_w = width(&words.refresh.0).max(width(&words.refresh.1)) as u16;
    let refresh_w = |with_words: bool| match glyph {
        Some(g) if with_words => width(g) as u16 + 1 + words_w,
        Some(g) => width(g) as u16 + 2,
        None => words_w,
    };
    let window_w = |mark: bool| {
        let value = if mark { &words.window.0 } else { &words.window.1 };
        (width(value) as u16).max(words.widest) + 2
    };
    let more_w = |long: bool| width(if long { &words.more.0 } else { &words.more.1 }) as u16;
    let need = |s: &HeadStep| {
        let name = if s.name { width(words.name_at(*s)) as u16 + HEAD_GAP } else { 0 };
        let filters = words.filters.map_or(0, |w| w + HEAD_GAP);
        let more = if s.more { more_w(s.long) + HEAD_GAP } else { 0 };
        name + window_w(s.mark) + HEAD_GAP + filters + more + refresh_w(s.words)
    };
    let steps = head_steps(glyph.is_some(), words.filters.is_some(), words.short.is_some());
    let step = steps.iter().copied().find(|s| need(s) <= content.width).unwrap_or(steps[steps.len() - 1]);
    let slot_w = refresh_w(step.words);
    let slot_x = content.right().saturating_sub(slot_w);
    // "options" given way: no width, at the refresh's slot.
    let (more_x, more_w) = if step.more {
        let w = more_w(step.long);
        (slot_x.saturating_sub(HEAD_GAP + w), w)
    } else {
        (slot_x, 0)
    };
    let filters = words.filters.map(|w| (more_x.saturating_sub(HEAD_GAP + w), w));
    let window_w = window_w(step.mark);
    let window_x = filters.map_or(more_x, |(x, _)| x).saturating_sub(HEAD_GAP + window_w).max(content.x);
    HeadPlan { step, window: (window_x, window_w), filters, more: (more_x, more_w), refresh: (slot_x, slot_w) }
}

/// The header (discovery-page clauses 11, 15, 17, 91; peer-recs contract §1.4): the room's
/// name BOLD, then, at the right, the controls: the window dropdown (the value in the row's
/// own weight, the chevron dim, both BRIGHT + BOLD under the pointer — the kit's Dropdown — in
/// a slot as wide as its widest window, so choosing one moves nothing), the mood opener where
/// the room offers it, "more options" (a DIM text button: the seeds modal) and "↻ new seeds"
/// (the accent's text button, the row's one primary: ask again). While an ask is out the
/// refresh reads "asking…" in the getting-there colour — "↻ …" where its words dropped, never
/// the colour alone — in a slot as wide as the wider of its words, so nothing moves. Short of
/// width the steps of [`head_steps`] apply.
pub(crate) fn draw_header<R: BarRoom>(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let locale = rust_i18n::locale();
    let facts = R::facts(gui);
    let words = HeadWords::of(&facts, &locale);
    let glyph = refresh_glyph();
    let plan = head_plan(&words, content, glyph);
    let step = plan.step;
    if step.name {
        put(frame, content.x, y, words.name_at(step), bold());
    }

    let (window_x, window_w) = plan.window;
    let window = if step.mark { &words.window.0 } else { &words.window.1 };
    let rect = Rect { x: window_x, y, width: window_w, height: 1 };
    let hover = gui.ui.hovers(rect);
    put(frame, rect.x, y, window, if hover { bright_bold() } else { Style::default() });
    put(frame, rect.x + window_w - 1, y, chevron(), if hover { bright_bold() } else { dim() });
    gui.ui.click(rect, R::wrap(BarAct::Window));
    let tip = t!("gui.discover.window_tip").to_string();
    // A widened answer's fact, in full, under the pointer (discovery-page clause 11).
    let tip = match &facts.widened {
        Some(fact) if facts.widened_from.is_some() => format!("{fact} · {tip}"),
        _ => tip,
    };
    gui.ui.tip_keyed(rect, tip);
    R::bar(gui).window_at = rect;

    // The mood opener (discovery-page clause 33): its modal opens over the room.
    if let (Some((x, _)), Some(act)) = (plan.filters, facts.moods.clone()) {
        let word = super::super::moods::opener_word(&locale);
        super::super::text_button(frame, gui, x, y, &word, false, act);
    }

    let (more_x, more_w) = plan.more;
    if step.more {
        let more = if step.long { &words.more.0 } else { &words.more.1 };
        super::super::text_button(frame, gui, more_x, y, more, false, R::wrap(BarAct::Seeds));
        gui.ui.tip_keyed(Rect { x: more_x, y, width: more_w, height: 1 }, t!("gui.discover.seeds_tip").to_string());
    }

    // An ask out: the refresh says so (discovery-page clause 17).
    let asking = facts.asking;
    let said = if asking { &words.refresh.1 } else { &words.refresh.0 };
    let label = match glyph {
        Some(g) if step.words => format!("{g} {said}"),
        Some(g) if asking => format!("{g} …"),
        Some(g) => g.to_string(),
        None => said.clone(),
    };
    let label_w = width(&label) as u16;
    // From its slot's left: the glyph stays put, under the pointer that clicked it, while the
    // words turn to "asking…" and back.
    let refresh = Rect { x: plan.refresh.0, y, width: label_w, height: 1 };
    let style = if gui.ui.hovers(refresh) {
        bright_bold()
    } else if asking {
        Style::default().fg(th().gold)
    } else {
        accent()
    };
    put(frame, refresh.x, y, &label, style);
    gui.ui.click(refresh, R::wrap(BarAct::NewSeeds));
    if let Some(tip) = facts.refresh_tip {
        gui.ui.tip_keyed(refresh, tip);
    }
}

// ── The window's list and the seeds modal ───────────────────────────────────

/// The seeds modal's lines (discovery-page clause 13).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SeedLine {
    Head(String),
    Pin(usize),
    Seed(String),
    Dropped(String),
    PinPlaying,
    PinSong,
}

impl SeedLine {
    fn is_row(&self) -> bool {
        !matches!(self, SeedLine::Head(_))
    }
}

/// The seeds modal's lines, from the room's own `TasteState`.
pub(crate) fn seed_lines<R: BarRoom>(app: &App) -> Vec<SeedLine> {
    let taste = R::taste(app);
    let mut out = Vec::new();
    if !taste.pins.is_empty() {
        out.push(SeedLine::Head(t!("gui.discover.sec_pinned").to_uppercase()));
        out.extend((0..taste.pins.len()).map(SeedLine::Pin));
    }
    if let Some(head) = &taste.head {
        let history: Vec<_> = if head.seed_source == SeedSource::Random {
            Vec::new()
        } else {
            head.seeds.iter().filter(|s| !s.pinned).map(|s| SeedLine::Seed(s.hash.clone())).collect()
        };
        if !history.is_empty() {
            out.push(SeedLine::Head(t!("gui.discover.sec_seeds").to_uppercase()));
            out.extend(history);
        }
        // This visit's drops the answer resolved (its `excluded`), never a random pick passed
        // over with "another pick" (discovery-page clause 21).
        let dropped = taste.dropped_seeds();
        if !dropped.is_empty() {
            out.push(SeedLine::Head(t!("gui.discover.sec_dropped").to_uppercase()));
            out.extend(dropped.iter().map(|e| SeedLine::Dropped(e.hash.clone())));
        }
    }
    // Hidden, never grey, when it cannot be done (discovery-page clause 14).
    if R::can_pin_playing(app) {
        out.push(SeedLine::PinPlaying);
    }
    out.push(SeedLine::PinSong);
    out
}

/// The window's list and the seeds modal, drawn (and registered) after the base screen so
/// their rects win the pointer. Nothing draws unless the room is up.
pub(crate) fn draw_modals<R: BarRoom>(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if gui.screen != Screen::Library || gui.active != R::NAV {
        return;
    }
    if let Some(Menu::Window(cursor)) = R::bar_ref(gui).menu {
        let current = R::facts(gui).window.unwrap_or(WINDOW_DEFAULT);
        let rows: Vec<(String, bool, BarAct)> =
            WINDOWS.iter().map(|w| (window_label(*w), *w == current, BarAct::WindowPick(*w))).collect();
        let anchor = R::bar_ref(gui).window_at;
        draw_list::<R>(frame, gui, (area, anchor), rows, cursor);
    }
    if R::bar_ref(gui).seeds.is_some() {
        draw_seeds_modal::<R>(frame, gui, area);
    }
}

fn draw_list<R: BarRoom>(
    frame: &mut Frame,
    gui: &mut Gui,
    (area, anchor): (Rect, Rect),
    rows: Vec<(String, bool, BarAct)>,
    cursor: usize,
) {
    gui.ui.click(area, R::wrap(BarAct::MenuClose));
    let widest = rows.iter().map(|(label, _, _)| width(label)).max().unwrap_or(10) as u16;
    let w = (widest + 6).max(anchor.width + 4).min(area.width.saturating_sub(2));
    let h = rows.len() as u16 + 2;
    let x = anchor.x.saturating_sub(1).min(area.right().saturating_sub(w + 1));
    let rect = Rect { x, y: anchor.bottom(), width: w, height: h };
    gui.ui.overlay(rect);
    let inner = frame_at(frame, rect, th().accent);
    for (i, (label, current, act)) in rows.into_iter().enumerate() {
        let row = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        let on = i == cursor;
        let hover = !on && gui.ui.hovers(row);
        if on {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), row);
        }
        let mark = if current { bullet_glyph() } else { " " };
        put(frame, row.x + 1, row.y, mark, if on { sel() } else { accent() });
        let style = match (on, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        let label = super::super::bar::clip(&label, inner.width.saturating_sub(4) as usize).into_owned();
        put(frame, row.x + 3, row.y, &label, style);
        gui.ui.click(row, R::wrap(act));
    }
}

/// The seeds modal (discovery-page clause 13): "Based on", its sections, the pin rows and Done,
/// from the room's own `TasteState`.
fn draw_seeds_modal<R: BarRoom>(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    gui.ui.click(area, Act::Guard);
    let rows = seed_lines::<R>(&gui.app);
    let w = 72.min(area.width.saturating_sub(4));
    let h = (rows.len() as u16 + 7).min(area.height.saturating_sub(2));
    let inner = modal_frame_on(frame, &mut gui.ui, area, w, h, th().accent);
    put(frame, inner.x + 1, inner.y, &t!("gui.discover.seeds_title"), accent().add_modifier(Modifier::BOLD));
    modal_close(frame, &mut gui.ui, inner, R::wrap(BarAct::SeedsDone));
    let sub = t!("gui.discover.seeds_sub").to_string();
    put(frame, inner.x + 1, inner.y + 1, &super::super::bar::clip(&sub, inner.width.saturating_sub(2) as usize), dim());
    let list = Rect { y: inner.y + 3, height: inner.height.saturating_sub(5), ..inner };
    let now = crate::admin::unix_now();
    let offset = {
        let zone = R::bar(gui).zone.get_or_insert_with(crate::admin::tz::local);
        zone.as_ref().map_or(0, |z| z.offset_at(now))
    };
    let Some(modal) = R::bar_ref(gui).seeds.clone() else { return };
    let mut view = modal.view;
    let shown = view.shown(modal.cursor);
    let (first, visible) = view.window(rows.len(), modal.cursor, list.height as usize);
    let (check_on, check_off) = check_glyphs();
    for (row, index) in (first..(first + visible).min(rows.len())).enumerate() {
        let y = list.y + row as u16;
        let rect = Rect { x: list.x, y, width: list.width, height: 1 };
        let line = &rows[index];
        if !line.is_row() {
            if let SeedLine::Head(text) = line {
                put(frame, rect.x + 1, y, text, dim());
            }
            continue;
        }
        let is_sel = shown == Some(index);
        let hover = !is_sel && gui.ui.hovers(rect);
        if is_sel {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let style = match (is_sel, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        let facts_style = if is_sel { sel() } else { dim() };
        let taste = R::taste(&gui.app);
        let room = rect.width.saturating_sub(2) as usize;
        let (check, words, facts): (Option<bool>, String, String) = match line {
            SeedLine::Pin(i) => {
                let pin = &taste.pins[*i];
                (None, track_words(&pin.track), t!("gui.discover.seed_pinned").to_string())
            }
            SeedLine::Seed(hash) => {
                let Some(seed) = taste.seed(hash) else { continue };
                let mut facts = Vec::new();
                if let Some(then) = seed.last_played.as_deref().and_then(crate::admin::iso_unix) {
                    facts.push(age_words(then, now, offset));
                }
                match seed.outcome {
                    TasteOutcome::Completed => facts.push(t!("gui.discover.outcome_completed").to_string()),
                    TasteOutcome::Stopped => facts.push(t!("gui.discover.outcome_stopped").to_string()),
                    TasteOutcome::Other => {}
                }
                if !seed.analyzed {
                    facts.push(t!("gui.discover.not_analyzed").to_string());
                }
                (Some(!modal.flipped.contains(hash)), track_words(&seed.to_track()), facts.join(" · "))
            }
            SeedLine::Dropped(hash) => {
                let found = taste.head.as_ref().and_then(|h| h.excluded.iter().find(|e| &e.hash == hash));
                let Some(excluded) = found else { continue };
                let track = Track { filepath: excluded.filepath.clone(), metadata: excluded.metadata.clone() };
                (Some(modal.flipped.contains(hash)), track_words(&track), String::new())
            }
            SeedLine::PinPlaying => (None, format!("+ {}", t!("gui.discover.pin_playing")), String::new()),
            SeedLine::PinSong => (None, format!("+ {}", t!("gui.discover.pin_song")), String::new()),
            SeedLine::Head(_) => continue,
        };
        let mut x = rect.x + 1;
        if let Some(on) = check {
            let (glyph, gstyle) = if on {
                (check_on, if is_sel { sel() } else { Style::default().fg(th().ok) })
            } else {
                (check_off, facts_style)
            };
            put(frame, x, y, glyph, gstyle);
            x += 4;
        }
        let unpin = matches!(line, SeedLine::Pin(_));
        let tail = if unpin { 5 } else { 1 };
        let avail = (rect.right().saturating_sub(x + tail)) as usize;
        let words = super::super::bar::clip(&words, avail.min(room)).into_owned();
        put(frame, x, y, &words, style);
        let after = x + width(&words) as u16 + 2;
        if !facts.is_empty() && after < rect.right().saturating_sub(tail) {
            let room = rect.right().saturating_sub(after + tail) as usize;
            put(frame, after, y, &super::super::bar::clip(&facts, room), facts_style);
        }
        gui.ui.click(rect, R::wrap(BarAct::SeedRow(index)));
        if let SeedLine::Pin(i) = line {
            let cell = Rect { x: rect.right().saturating_sub(4), y, width: 3, height: 1 };
            let style = if gui.ui.hovers(cell) {
                Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
            } else {
                facts_style
            };
            put(frame, cell.x, y, "[X]", style);
            gui.ui.click(cell, R::wrap(BarAct::SeedUnpin(*i)));
            gui.ui.tip_keyed(cell, t!("gui.discover.unpin_tip").to_string());
        }
    }
    if let Some(modal) = R::bar(gui).seeds.as_mut() {
        modal.view = view;
    }
    // The modal primary, the safe choice: Done applies (discovery-page clause 13).
    let done = t!("gui.discover.seeds_done").to_string();
    let done_w = width(&done) as u16;
    let rect = Rect {
        x: inner.right().saturating_sub(done_w + 1),
        y: inner.bottom().saturating_sub(1),
        width: done_w,
        height: 1,
    };
    let style = if gui.ui.hovers(rect) { bright_bold() } else { accent().add_modifier(Modifier::BOLD) };
    put(frame, rect.x, rect.y, &done, style);
    gui.ui.click(rect, R::wrap(BarAct::SeedsDone));
}

/// Whether one of the bar's modals is up.
pub(crate) fn modal_open(bar: &BarUi) -> bool {
    bar.menu.is_some() || bar.seeds.is_some()
}

/// Both modals go, and the seeds modal's changes with it: nothing is asked.
pub(crate) fn close(bar: &mut BarUi) {
    bar.menu = None;
    bar.seeds = None;
}

// ── Acts ────────────────────────────────────────────────────────────────────

/// A bar act, wrapped by the room ([`BarRoom::wrap`]) and routed back here.
pub(crate) fn act<R: BarRoom>(gui: &mut Gui, act: &BarAct) {
    match act {
        BarAct::Window => {
            if R::bar_ref(gui).menu.is_some() {
                R::bar(gui).menu = None;
            } else {
                let current = R::facts(gui).window.unwrap_or(WINDOW_DEFAULT);
                R::bar(gui).menu = Some(Menu::Window(WINDOWS.iter().position(|w| *w == current).unwrap_or(1)));
                R::modal_opened(gui);
            }
        }
        BarAct::WindowPick(window) => {
            R::bar(gui).menu = None;
            R::set_window(gui, *window);
        }
        BarAct::MenuClose => R::bar(gui).menu = None,
        BarAct::Seeds => {
            let bar = R::bar(gui);
            bar.menu = None;
            bar.seeds = Some(SeedsModal::default());
            R::modal_opened(gui);
        }
        BarAct::NewSeeds => R::new_seeds(gui),
        // A click, never a key (Space and Enter call `seed_row` and keep the cursor up): the
        // modal's hand goes down (the list-cursor law).
        BarAct::SeedRow(index) => {
            if let Some(modal) = R::bar(gui).seeds.as_mut() {
                modal.view.stow();
            }
            seed_row::<R>(gui, *index);
        }
        BarAct::SeedUnpin(i) => {
            let path = R::taste(&gui.app).pins.get(*i).map(|pin| pin.track.filepath.clone());
            if let Some(path) = path
                && R::unpin(&mut gui.app, &path)
                && let Some(modal) = R::bar(gui).seeds.as_mut()
            {
                modal.pins_changed = true;
            }
        }
        BarAct::SeedsDone => close_seeds::<R>(gui),
    }
}

/// A modal row, clicked or toggled by key (discovery-page clause 13).
fn seed_row<R: BarRoom>(gui: &mut Gui, index: usize) {
    let rows = seed_lines::<R>(&gui.app);
    let Some(line) = rows.get(index).cloned() else { return };
    match line {
        SeedLine::Seed(hash) | SeedLine::Dropped(hash) => {
            if let Some(modal) = R::bar(gui).seeds.as_mut() {
                if let Some(at) = modal.flipped.iter().position(|h| *h == hash) {
                    modal.flipped.remove(at);
                } else {
                    modal.flipped.push(hash);
                }
            }
        }
        SeedLine::Pin(i) => {
            let path = R::taste(&gui.app).pins.get(i).map(|pin| pin.track.filepath.clone());
            if let Some(path) = path
                && R::unpin(&mut gui.app, &path)
                && let Some(modal) = R::bar(gui).seeds.as_mut()
            {
                modal.pins_changed = true;
            }
        }
        SeedLine::PinPlaying => {
            if R::pin_playing(&mut gui.app)
                && let Some(modal) = R::bar(gui).seeds.as_mut()
            {
                modal.pins_changed = true;
            } else if R::taste(&gui.app).pins.len() >= TASTE_MAX_PINS {
                word_refusal(gui, TastePinRefusal::Full);
            }
        }
        SeedLine::PinSong => choose::<R>(gui),
        SeedLine::Head(_) => {}
    }
    // Rows come and go (a pin left): the cursor stays on a row that exists.
    let len = seed_lines::<R>(&gui.app).len();
    if let Some(modal) = R::bar(gui).seeds.as_mut()
        && let Some(cursor) = modal.cursor
        && cursor >= len
    {
        modal.cursor = len.checked_sub(1);
    }
}

/// The modal closes: the visit's drops (and "use again") change the `exclude` the asks carry,
/// pins came or went — then one ask; nothing changed asks nothing (discovery-page clause 13).
fn close_seeds<R: BarRoom>(gui: &mut Gui) {
    let Some(modal) = R::bar(gui).seeds.take() else { return };
    let (mut dropped, mut restored) = (Vec::new(), Vec::new());
    if let Some(head) = &R::taste(&gui.app).head {
        for hash in &modal.flipped {
            if head.seeds.iter().any(|s| &s.hash == hash && !s.pinned) {
                dropped.push(hash.clone());
            } else if head.excluded.iter().any(|e| &e.hash == hash) {
                restored.push(hash.clone());
            }
        }
    }
    let effects = R::seeds_closed(&mut gui.app, &dropped, &restored, modal.pins_changed);
    gui.pend(effects);
}

/// "Pin a song…" (discovery-page clauses 14, 21): the modal's changes apply first, then the
/// room arms its pick in the sonic room's grammar — the banner over the library, the pick
/// consumed, Esc back to the room.
pub(crate) fn choose<R: BarRoom>(gui: &mut Gui) {
    if R::bar_ref(gui).seeds.is_some() {
        close_seeds::<R>(gui);
    }
    R::bar(gui).menu = None;
    R::choose_song(gui);
}

/// The bar's wheel: the seeds modal's rows while it is up. The window's four choices always
/// show whole: over its list the wheel moves nothing, and not the room under it either. True
/// when the bar took the notch.
pub(crate) fn wheel<R: BarRoom>(gui: &mut Gui, delta: i32) -> bool {
    if let Some(modal) = R::bar(gui).seeds.as_mut() {
        modal.view.step(delta);
        return true;
    }
    R::bar_ref(gui).menu.is_some()
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The bar's keys while the room is up: every key while one of its modals is up, and, while
/// connected, `w` (the window's list), `e` (the seeds modal) and `[` `]` (the window a step
/// shorter or longer). `Some(quit)` when the key was the bar's.
pub(crate) fn handle_key<R: BarRoom>(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if gui.screen != Screen::Library || gui.active != R::NAV {
        return None;
    }
    if R::bar_ref(gui).seeds.is_some() {
        seeds_key::<R>(gui, key);
        return Some(false);
    }
    if let Some(menu) = R::bar_ref(gui).menu {
        menu_key::<R>(gui, key, menu);
        return Some(false);
    }
    if !gui.app.connected {
        return None;
    }
    match key.code {
        KeyCode::Char('w') => Some(gui.act(R::wrap(BarAct::Window))),
        KeyCode::Char('e') => Some(gui.act(R::wrap(BarAct::Seeds))),
        KeyCode::Char(c @ ('[' | ']')) => {
            let current = R::facts(gui).window.unwrap_or(WINDOW_DEFAULT);
            let at = WINDOWS.iter().position(|w| *w == current).unwrap_or(1);
            let next = if c == '[' { at.saturating_sub(1) } else { (at + 1).min(WINDOWS.len() - 1) };
            if next != at {
                R::set_window(gui, WINDOWS[next]);
            }
            Some(false)
        }
        _ => None,
    }
}

fn menu_key<R: BarRoom>(gui: &mut Gui, key: KeyEvent, menu: Menu) {
    let Menu::Window(cursor) = menu;
    let set = |gui: &mut Gui, c: usize| R::bar(gui).menu = Some(Menu::Window(c));
    match key.code {
        // Esc, or the control's own key, closes and keeps (the Dropdown law).
        KeyCode::Esc | KeyCode::Char('w') => R::bar(gui).menu = None,
        KeyCode::Up => set(gui, cursor.saturating_sub(1)),
        KeyCode::Down => set(gui, (cursor + 1).min(WINDOWS.len() - 1)),
        KeyCode::Enter => {
            gui.act(R::wrap(BarAct::WindowPick(WINDOWS[cursor.min(WINDOWS.len() - 1)])));
        }
        _ => {}
    }
}

fn seeds_key<R: BarRoom>(gui: &mut Gui, key: KeyEvent) {
    let rows = seed_lines::<R>(&gui.app);
    let picks: Vec<usize> = rows.iter().enumerate().filter(|(_, l)| l.is_row()).map(|(i, _)| i).collect();
    let Some(modal) = R::bar(gui).seeds.as_mut() else { return };
    let held = modal.view.held.then_some(modal.cursor).flatten();
    match key.code {
        KeyCode::Esc => close_seeds::<R>(gui),
        KeyCode::Down | KeyCode::Up if !picks.is_empty() => {
            let down = key.code == KeyCode::Down;
            let next = match held.and_then(|c| picks.iter().position(|p| *p == c)) {
                None if down => picks[0],
                None => picks[picks.len() - 1],
                Some(at) if down => picks[(at + 1).min(picks.len() - 1)],
                Some(at) => picks[at.saturating_sub(1)],
            };
            modal.cursor = Some(next);
            modal.view.pick_up();
        }
        KeyCode::Char(' ') | KeyCode::Enter => {
            if let Some(index) = held {
                seed_row::<R>(gui, index);
            }
        }
        KeyCode::Char('x') => {
            if let Some(SeedLine::Pin(i)) = held.and_then(|index| rows.get(index)) {
                gui.act(R::wrap(BarAct::SeedUnpin(*i)));
            }
        }
        _ => {}
    }
}

// ── Tips ────────────────────────────────────────────────────────────────────

/// The tips line while one of the bar's modals is up; `None` otherwise.
pub(crate) fn tips<R: BarRoom>(gui: &Gui) -> Option<String> {
    let bar = R::bar_ref(gui);
    if bar.seeds.is_some() {
        return Some(t!("gui.tips.discover_seeds").to_string());
    }
    if bar.menu.is_some() {
        return Some(t!("gui.tips.discover_menu").to_string());
    }
    None
}
