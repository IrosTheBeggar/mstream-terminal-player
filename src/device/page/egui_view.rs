//! The MP3 Player page drawn by egui, in the desktop window's panel over
//! the cells the GUI's tab would draw (src/gui/device.rs hosts it): the
//! same [`Page`] the cells view draws (the parent module: its state, its
//! worker's reports, its keys, its hints and its gates' words) — only the
//! drawing is new, in pixels, on the Studio kit (src/gui/window/studio.rs),
//! after the Claude Design canvas "MP3 Player in egui" (its shell and its
//! boards).
//!
//! This module is the page's frame, which every state shares: the page's
//! blank first row, the subjects column (the row of subjects, with two or
//! more, then one card) and the busy line along the area's foot; and the
//! dispatch of what stands in them — the board in view (board.rs), a card
//! in view (card.rs), nothing plugged in (board.rs's empty card) — and,
//! over the kit's scrim with the page inert beneath, the board's gate
//! (gate.rs), the Advanced options sheet (sheet.rs), a card's gate (card.rs)
//! and a card's Settings sheet (card_sheet.rs). The footer's hint is the
//! GUI's row under the area, drawn in cells as on every tab of the GUI.
//!
//! Every click is one of the page's own [`Act`]s, the very one the cells
//! view registers for the same control and only while it registers it — a
//! card's rows are the cells' own (cells.rs's `Row`s, made by the same
//! functions), only set in points — applied through [`Page::act`] at the
//! pass's end, once a click however many runs egui makes of a pass. A
//! control's id says what it is and whose, never where it falls, so a press
//! whose control went or changed before its release is dropped rather than
//! given to the control that took its place. Keys
//! never reach egui: the page's keymap still moves the view, opens Details,
//! answers the gates and edits the sheet's field, and egui draws what the
//! page holds. The view keeps nothing of its own between passes but what
//! egui's memory holds for it; the log's scroll is the page's, which the
//! wheel moves here as it does in cells.
//!
//! Built only with the window: the terminal build has no panel to draw in.

mod board;
mod card;
mod card_sheet;
mod gate;
mod sheet;

use egui::{Align2, Color32, FontId, Id, Pos2, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};
use ratatui::style::Style;

use super::*;
use crate::admin::EguiShown;
use crate::gui::window::studio::{self, Pal, Run, body, mono, strong};
use crate::kit::theme::th;

/// The subjects column's widest: the boards' 760 points, the cells' card.
const COLUMN_W: f32 = 760.0;
/// The page's sides, the boards' 16 points: the column's least margin and
/// the busy line's indent.
const SIDE: f32 = 16.0;
/// The subjects row: its height, a tab's padding at either side, the room
/// between a tab's name and its mark, and between two tabs.
const ROW_H: f32 = 30.0;
const TAB_PAD: f32 = 12.0;
const TAB_GAP: f32 = 7.0;
const TABS_APART: f32 = 6.0;
/// The least room between the last tab and the row's right-edge words,
/// which give way whole rather than come nearer.
const RIGHT_CLEAR: f32 = 22.0;
/// The least a card's name shrinks to on its tab before the right edge's
/// words give way.
const NAME_LEAST: f32 = 48.0;
/// The right edge's words, plain or Update all's text button: the boards'
/// 13 points, a half under the card's text buttons.
const RIGHT_SIZE: f32 = 13.0;

/// The frame's measures, and the cards': the boards' where the page has
/// the room for the card in view at rest, closed up toward a tighter set
/// just as far as it has not (down to the GUI's floor), so a card keeps
/// its bottom row and the page never scrolls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Space {
    /// Over the subjects column: the page's blank first row.
    pub(super) top: f32,
    /// Between blocks: the subjects row and the card, and a card's own
    /// blocks (the cells' blank rows).
    pub(super) gap: f32,
    /// A card's padding at its sides, and at its top and its foot.
    pub(super) pad_x: f32,
    pub(super) pad_y: f32,
    /// Between the column's foot and the busy line, and under the busy
    /// line, over the GUI's footer.
    pub(super) over_busy: f32,
    pub(super) under_busy: f32,
}

/// The boards' measures (the shell's: 20 points under the top bar, blocks
/// 18 apart, the card 24 in at its sides and 20 at its top and foot, the
/// busy line's row 14 over the footer).
pub(super) const ROOMY: Space = Space { top: 20.0, gap: 18.0, pad_x: 24.0, pad_y: 20.0, over_busy: 18.0, under_busy: 14.0 };
/// The closest the frame comes, where the boards' leave a card no room for
/// its bottom row. Its top is the kit's modal's margin (a Dialog stands 16
/// points clear of the area's ends at its tallest), so a sheet held at its
/// highest covers the subjects row whole rather than cut a sliver off it.
pub(super) const TIGHT: Space = Space { top: 16.0, gap: 10.0, pad_x: 16.0, pad_y: 12.0, over_busy: 6.0, under_busy: 6.0 };

impl Space {
    /// `t` of the way from [`ROOMY`] to [`TIGHT`], each measure a whole
    /// number of points (no hairline falls between pixels), never roomier
    /// than the share asked for.
    pub(super) fn at(t: f32) -> Space {
        let t = t.clamp(0.0, 1.0);
        let k = |a: f32, b: f32| (a + (b - a) * t).floor();
        let (r, s) = (ROOMY, TIGHT);
        Space {
            top: k(r.top, s.top),
            gap: k(r.gap, s.gap),
            pad_x: k(r.pad_x, s.pad_x),
            pad_y: k(r.pad_y, s.pad_y),
            over_busy: k(r.over_busy, s.over_busy),
            under_busy: k(r.under_busy, s.under_busy),
        }
    }
}

/// What the frame hands each part of the page beside the page itself.
#[derive(Clone, Copy)]
pub(super) struct Face<'a> {
    pub(super) pal: &'a Pal,
    /// The input method's uncommitted text, for the field with the keyboard
    /// to draw (`studio::Input::focused`).
    pub(super) composition: &'a str,
    /// Tooltips name the key that does the same, as the cells' surface
    /// says (`Surface::key_hints`, the GUI's setting), for the parts whose
    /// cells register a keyed tooltip: the sheets' close ([`close_tip`]).
    /// The board's one tooltip, a waiting primary's why, names no key.
    pub(super) key_hints: bool,
    /// The page's area in points, the pass's screen: what a modal's scrim
    /// covers.
    pub(super) full: Rect,
    pub(super) space: Space,
    /// The boards' ports by name, which a sentence sets in the monospace
    /// with its other figures (`studio::figured`).
    pub(super) ports: &'a [&'a str],
    /// The run draws for real; else it only measures (the invisible run
    /// the frame makes to choose its [`Space`]): no control is made, no act
    /// asked for, nothing read of the input.
    pub(super) live: bool,
    /// A modal lies over the page: drawn at rest, nothing beneath takes the
    /// pointer — no hover, no hand, no tooltip, no click — as the cells
    /// draw it with no pointer under their modal.
    pub(super) inert: bool,
}

impl Face<'_> {
    /// The pointer reaches what the part draws.
    pub(super) fn takes(&self) -> bool {
        self.live && !self.inert
    }
}

/// What a part of the page hands back to the frame. The hand is egui's
/// own, from the cursor each control asks for (`studio::hand`).
#[derive(Default)]
pub(super) struct Drawn {
    /// What its clicks asked for: the cells' acts for the same controls,
    /// applied in this order at the pass's end.
    pub(super) acts: Vec<Act>,
    /// The foot of the caret of its field with the keyboard, in points.
    pub(super) caret: Option<Pos2>,
    /// How tall its card stands at rest, from its top through its bottom
    /// row and its foot's padding — and, while Details is open, through
    /// Details too, its log at its newest lines up to board.rs's
    /// `LOG_REST`: what the frame measures to choose its [`Space`]. Nothing
    /// for a modal.
    pub(super) rest: f32,
}

/// One pass of the page over the whole of `ui`, the page's area in the
/// window's panel, with `composition` the input method's uncommitted text
/// and `key_hints` whether tooltips name their keys. The page is drawn,
/// then its modal over it, which leaves the page inert: what was clicked
/// under it is dropped. The acts asked for are applied through the page's
/// own act path, and what the host must know comes back: the caret of the
/// field with the keyboard (none when no field has it now), and an outcome
/// a click brought.
pub(super) fn show(ui: &mut Ui, page: &mut Page, composition: &str, key_hints: bool) -> EguiShown {
    // The cells' surface holds nothing while egui draws the page: no click
    // of its for the cells' pointer path to take, no hover, no tooltip, no
    // log under the wheel.
    page.ui.begin_frame();
    page.log_rect = None;
    let pal = studio::begin(ui);
    let full = ui.max_rect();
    let names: Vec<String> = page.boards.iter().flat_map(|b| [short(b.port()), b.port().to_string()]).collect();
    let ports: Vec<&str> = names.iter().map(String::as_str).collect();
    let modal = page.modal_open();
    let mut face = Face { pal: &pal, composition, key_hints, full, space: ROOMY, ports: &ports, live: true, inert: modal };
    face.space = measures(ui, page, &face);
    let at = parts(full, face.space, page);
    let mut drawn = Drawn::default();
    if let Some(row) = at.row {
        subjects(ui, page, &face, row, &mut drawn.acts);
    }
    let part = subject(ui, page, &face, at.card);
    drawn.acts.extend(part.acts);
    drawn.caret = part.caret;
    busy_line(ui, at.busy, page, &face);
    if modal {
        drawn = modal_part(ui, page, &face);
    }
    let acted = !drawn.acts.is_empty();
    let mut outcome = None;
    for act in drawn.acts {
        outcome = outcome.or(page.act(act));
    }
    if acted {
        // The page changed after this pass drew it: the next pass shows it.
        ui.ctx().request_repaint();
    }
    // The caret this pass drew is the one of the field that had the keys
    // then; a click that closed the field took the keyboard with it.
    let caret = drawn.caret.filter(|_| page.claim() == Claim::All);
    EguiShown { caret, outcome }
}

/// What the tab draws while the page it was left with lets its ports go
/// and the next waits to be built (contract clause 5; the cells'
/// `draw_waiting`): `words`, dim, on the page's first line under its blank
/// first row, at the subjects column's left, in the 13 points the same
/// words take on the busy line while the page lets go.
pub(crate) fn waiting(ui: &mut Ui, words: &str) {
    let pal = studio::begin(ui);
    let full = ui.max_rect();
    let w = COLUMN_W.min(full.width() - 2.0 * SIDE).max(1.0);
    let at = pos2(full.center().x - w / 2.0, full.min.y + ROOMY.top + studio::NOTE_H / 2.0);
    studio::text(ui, at, words, body(13.0), pal.dim, w, Align2::LEFT_CENTER);
}

/// The measures for this pass: the boards' where the card in view at rest
/// (Details with it while open) stands whole under them, else closed up
/// toward the tighter set just as far as it needs ([`Space::at`]), else the
/// tighter set. Found by invisible runs of the card's own drawing in a room
/// with no floor, so the measuring and the drawing never disagree about its
/// height; each held to the hundredth's grace the drawing's floor allows a
/// row (board.rs's `Paint`), so a card a fraction of a point too tall for
/// its measures never takes them and loses its last value line under the
/// floor.
///
/// Held for the area's size in egui's memory: while the window keeps its
/// size the frame never opens up again, so a sync that starts or ends,
/// Details closed or another subject chosen moves nothing of the page — it
/// closes up once, as far as the tallest card it has shown at that size
/// needed, and opens up again only with the window.
fn measures(ui: &mut Ui, page: &mut Page, face: &Face) -> Space {
    let key = Id::new("mp3-space");
    let size = face.full.size();
    let held: Option<(Vec2, f32)> = ui.ctx().data(|d| d.get_temp(key));
    let held = held.filter(|(was, _)| (*was - size).abs().max_elem() < 0.5).map_or(0.0, |(_, t)| t);
    let fits = |over: f32| over <= 0.01;
    let at_held = overrun(ui, page, face, held, 0);
    let tight = if fits(at_held) { 0.0 } else { overrun(ui, page, face, 1.0, 1) };
    let t = if fits(at_held) {
        held
    } else if !fits(tight) {
        1.0
    } else {
        // The card's height and its room each move with the measures
        // nearly in a line: the share that closes the gap, then halved
        // toward the tighter end where a line's wrap or a point's rounding
        // leaves it a hair short.
        let guess = held + (1.0 - held) * at_held / (at_held - tight);
        let (mut lo, mut hi) = (held, 1.0_f32);
        let mut t = guess;
        for k in 0..5 {
            if fits(overrun(ui, page, face, t, 2 + k)) {
                hi = t;
                if k == 0 {
                    break;
                }
            } else {
                lo = t;
            }
            t = (lo + hi) / 2.0;
        }
        hi
    };
    ui.ctx().data_mut(|d| d.insert_temp(key, (size, t)));
    Space::at(t)
}

/// How far the card in view at rest stands past the room the frame leaves
/// it with the measures `t` of the way to the tighter set: an invisible
/// run of its own drawing, salted `salt` among the pass's runs.
fn overrun(ui: &mut Ui, page: &mut Page, face: &Face, t: f32, salt: usize) -> f32 {
    let space = Space::at(t);
    let at = parts(face.full, space, page);
    let room = Rect::from_min_size(at.card.min, vec2(at.card.width(), 100_000.0));
    let mut probe = ui.new_child(UiBuilder::new().id_salt(("mp3-probe", salt)).max_rect(room));
    probe.set_invisible();
    let probing = Face { space, live: false, ..*face };
    subject(&mut probe, page, &probing, room).rest - at.card.height()
}

/// Where the frame's parts stand: the subjects row (with two subjects or
/// more), the room the card has under it, and the busy line.
struct Parts {
    row: Option<Rect>,
    card: Rect,
    busy: Rect,
}

/// The frame's parts in `full` with `space`: the column 760 points wide at
/// most, centred, narrowing with the window, from under the blank first
/// row to over the busy line; the busy line along the area's foot, 16
/// points in from the window's sides and across the window, not the
/// column (the cells' hosted foot).
fn parts(full: Rect, space: Space, page: &Page) -> Parts {
    let foot = full.max.y - space.under_busy;
    let busy = Rect::from_min_max(pos2(full.min.x + SIDE, foot - studio::NOTE_H), pos2(full.max.x - SIDE, foot));
    let w = COLUMN_W.min(full.width() - 2.0 * SIDE).max(0.0);
    let (left, top) = (full.center().x - w / 2.0, full.min.y + space.top);
    let bottom = (busy.min.y - space.over_busy).max(top);
    let several = page.boards.len() + page.card_count() >= 2;
    let row = several.then(|| Rect::from_min_size(pos2(left, top), vec2(w, ROW_H)));
    let card_top = if several { (top + ROW_H + space.gap).min(bottom) } else { top };
    Parts { row, card: Rect::from_min_max(pos2(left, card_top), pos2(left + w, bottom)), busy }
}

/// The one card in the column's `room`, as the cells choose it (cells.rs's
/// `draw`): a card in view, else the board in view, else the empty card.
fn subject(ui: &mut Ui, page: &mut Page, face: &Face, room: Rect) -> Drawn {
    match (page.open_card(), page.shown().cloned()) {
        (Some(view), _) => card::show(ui, page, face, &view, room),
        (None, Some(board)) => board::show(ui, page, face, &board, room),
        (None, None) => board::empty(ui, page, face, room),
    }
}

/// The modal up, over the page's area and the kit's scrim, in the cells'
/// order (cells.rs's `draw`): the board's gate, the Advanced options sheet,
/// a card's gate, a card's Settings sheet.
fn modal_part(ui: &mut Ui, page: &mut Page, face: &Face) -> Drawn {
    let face = Face { inert: false, ..*face };
    if page.gate.is_some() {
        gate::show(ui, page, &face)
    } else if page.sheet.is_some() {
        sheet::show(ui, page, &face)
    } else if page.card.gate.is_some() {
        card::gate(ui, page, &face)
    } else {
        card_sheet::show(ui, page, &face)
    }
}

/// The busy line across `rect` (the cells' hosted foot): what the page is
/// busy with in the accent, else its note — gold where the page says so,
/// dim otherwise — cut with an ellipsis, its figures and the boards' ports
/// in the monospace (the Done board's version); nothing for nothing, the
/// row kept.
fn busy_line(ui: &Ui, rect: Rect, page: &Page, face: &Face) {
    let busy = page.busy();
    let note = page.note.as_ref().map(|n| (n.text.clone(), n.gold));
    studio::note_line_naming(ui, rect, busy.as_deref(), note.as_ref(), face.ports, face.pal);
}

/// A keep's words bare of the cells' ◂ before them (cells.rs's
/// `gate_buttons`, cells/card.rs's `gate_buttons_of`), and whether it was
/// there: the kit's keep leads them with its pointer turned back
/// (`studio::keep_button`).
pub(super) fn unled(label: &str) -> (&str, bool) {
    match label.trim_start().strip_prefix(glyphs().back) {
        Some(words) => (words.trim_start(), true),
        None => (label.trim(), false),
    }
}

/// The cells' tooltip on a sheet's close (the kit's `modal_close`), on the
/// close the kit's Dialog drew under `id`, its key named only while the
/// GUI names keys, as the cells' surface registers it.
pub(super) fn close_tip(ui: &Ui, id: Id, face: &Face) {
    if let Some(close) = ui.ctx().read_response(id.with("close")) {
        let tip = t!("path_modal.tip_close").to_string();
        let tip = if face.key_hints { tip } else { tip.rsplit_once(" — ").map_or(tip.clone(), |(words, _)| words.to_string()) };
        let _ = close.on_hover_text(tip);
    }
}

/// The colour the cells give `style`, in the desk's palette: their gold,
/// green, accent, dim and danger as the kit's; anything else the text
/// colour.
pub(super) fn tone(style: Style, pal: &Pal) -> Color32 {
    let t = th();
    match style.fg {
        Some(c) if c == t.gold => pal.gold,
        Some(c) if c == t.ok => pal.ok,
        Some(c) if c == t.accent => pal.accent,
        Some(c) if c == t.dim => pal.dim,
        Some(c) if c == t.danger => pal.danger,
        _ => pal.text,
    }
}

// ── The subjects row ──────────────────────────────────────────────────────

/// One tab of the subjects row, as the cells make it (cells.rs's
/// `tab_row`): its name and the part of it the monospace sets (a port
/// whole, a card's drive), its mark and the mark's colour, whether its
/// subject is in view, its act, and its id ([`tab_id`]).
struct Tab {
    name: String,
    figure: Option<String>,
    mark: String,
    tone: Color32,
    open: bool,
    act: Act,
    id: Id,
}

/// A tab's id: its subject's, a board's port or a card's key, never its
/// place on the row. egui gives a release to the id that was under its
/// press, so a press on a tab whose subject moved along the row before its
/// release (another plugged in before it, one before it gone) opens that
/// subject still, and one whose subject went opens nothing, never the
/// subject that took its place.
fn tab_id(subject: &str) -> Id {
    Id::new("mp3-tab").with(subject)
}

/// How a tab's mark is drawn, as the boards draw each: the cells' ✓ as the
/// kit's check and their ✗ as its cross (the panel's faces hold neither
/// glyph), a write's or a sync's percentage as a figure in the monospace at
/// its one weight, any other mark (! ? … in use) its words in the semibold.
enum Look {
    Check,
    Cross,
    Words(FontId),
}

impl Tab {
    /// The tab's name as runs in `ink`, a card's words in the semibold
    /// under the pointer and on the slab (`bold`: the cells' bold), a port
    /// or a drive regular (the monospace has no bold).
    fn runs(&self, ink: Color32, bold: bool) -> Vec<Run> {
        let font = if bold { strong(13.5) } else { body(13.5) };
        match self.figure.as_deref() {
            Some(figure) if figure == self.name => vec![(self.name.clone(), mono(13.0), ink)],
            Some(figure) if self.name.starts_with(figure) => {
                vec![(figure.to_string(), mono(13.0), ink), (self.name[figure.len()..].to_string(), font, ink)]
            }
            _ => vec![(self.name.clone(), font, ink)],
        }
    }

    /// How its mark is drawn.
    fn look(&self) -> Look {
        let g = glyphs();
        if self.mark == g.ok {
            return Look::Check;
        }
        if self.mark == g.no {
            return Look::Cross;
        }
        let figure = self.mark.strip_suffix('%').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        Look::Words(if figure { mono(13.0) } else { strong(14.0) })
    }

    /// The mark's width: the check's or the cross's 10 points, else its
    /// words'.
    fn mark_w(&self, ui: &Ui) -> f32 {
        match self.look() {
            Look::Check | Look::Cross => 10.0,
            Look::Words(font) => studio::width_of(ui, &self.mark, font),
        }
    }
}

/// The page's tabs: the boards in the OS's port order, then the cards in
/// this computer's readers, each with its mark, the one in view lit.
fn tabs(page: &Page, pal: &Pal) -> Vec<Tab> {
    let mut tabs = Vec::new();
    for (i, board) in page.boards.iter().enumerate() {
        let (mark, color) = cells::mark(page, board);
        let name = short(board.port());
        let open = page.card_open.is_none() && page.open.as_deref() == Some(board.port());
        let tone = tone(Style::default().fg(color), pal);
        tabs.push(Tab { figure: Some(name.clone()), name, mark, tone, open, act: Act::Board(i), id: tab_id(board.port()) });
    }
    for (i, card) in page.cards().iter().enumerate() {
        let (mark, color) = cells::card::mark(card);
        let name = cells::card::tab_name(card, usize::MAX);
        let drive = card.drive();
        let figure = name.starts_with(&format!("{drive} ")).then_some(drive);
        let open = page.card_open.as_deref() == Some(card.key.as_str());
        let tone = tone(Style::default().fg(color), pal);
        tabs.push(Tab { name, figure, mark, tone, open, act: Act::Card(i), id: tab_id(&card.key) });
    }
    tabs
}

/// The subjects row across `row` (contract clause 25): the tabs from its
/// left, 6 points apart, each a 30-point slot with its name and its mark —
/// the one in view on the accent's slab, the others dim with their marks in
/// their own colours, raised and bright under the pointer — and at the
/// row's right edge one slot of words (cells.rs's `tabs_right`): a count, a
/// write or a sync under way, or Update all to offer as a text button.
/// Short of room, as the cells give way: the cards' names shrink first,
/// then the right edge's words go whole, then the last tabs. A click on a
/// tab asks for the cells' act for it, the open one's too.
fn subjects(ui: &Ui, page: &Page, face: &Face, row: Rect, acts: &mut Vec<Act>) {
    let pal = face.pal;
    let tabs = tabs(page, pal);
    let (words, style, act) = cells::tabs_right(page);
    let right_tone = tone(style, pal);
    let right_w = match &act {
        Some(_) => studio::text_button_width_sized(ui, &words, None, face.ports, RIGHT_SIZE),
        None => studio::runs_width(ui, &studio::figured(&words, face.ports, body(RIGHT_SIZE), mono(12.5), right_tone)),
    };
    // Each tab's frame (its padding, its gap and its mark) and its name's
    // natural width.
    let frame_of = |ui: &Ui, tab: &Tab| 2.0 * TAB_PAD + TAB_GAP + tab.mark_w(ui);
    let natural: Vec<f32> = tabs.iter().map(|t| studio::runs_width(ui, &t.runs(pal.dim, true))).collect();
    let mut rooms = natural.clone();
    let total = |rooms: &[f32]| -> f32 {
        let gaps = TABS_APART * tabs.len().saturating_sub(1) as f32;
        tabs.iter().zip(rooms).map(|(t, w)| frame_of(ui, t) + w).sum::<f32>() + gaps
    };
    let mut keep_right = !words.is_empty();
    let want = |keep: bool| if keep { RIGHT_CLEAR + right_w } else { 0.0 };
    // The cards' names shrink first, evenly, to no less than their least.
    let cards: Vec<usize> = tabs.iter().enumerate().filter(|(_, t)| matches!(t.act, Act::Card(_))).map(|(i, _)| i).collect();
    let over = total(&rooms) + want(keep_right) - row.width();
    if over > 0.0 && !cards.is_empty() {
        let each = over / cards.len() as f32;
        for &i in &cards {
            rooms[i] = (natural[i] - each).max(NAME_LEAST.min(natural[i]));
        }
    }
    // Then the right edge's words go whole.
    if total(&rooms) + want(keep_right) > row.width() {
        keep_right = false;
    }
    let mut x = row.min.x;
    for (i, tab) in tabs.iter().enumerate() {
        let w = frame_of(ui, tab) + rooms[i];
        // Then the last tabs: the arrows still reach them.
        if x + w > row.max.x + 0.5 {
            break;
        }
        let rect = Rect::from_min_size(pos2(x, row.min.y), vec2(w, ROW_H));
        if draw_tab(ui, face, tab, rect, rooms[i]) {
            acts.push(tab.act.clone());
        }
        x = rect.max.x + TABS_APART;
    }
    if !keep_right || x - TABS_APART + RIGHT_CLEAR > row.max.x - right_w {
        return;
    }
    match act {
        Some(act) => {
            // Flush with the column's end at rest and in the semibold under
            // the pointer alike, as the boards set it: its slot is the
            // semibold's, its words as wide as they are drawn now.
            let slot = Rect::from_min_max(pos2(row.max.x - right_w, row.min.y), row.max);
            let hot = face.takes() && ui.rect_contains_pointer(slot);
            let rest = studio::runs_width(ui, &studio::figured(&words, face.ports, body(RIGHT_SIZE), mono(12.5), pal.dim));
            let w = if hot { right_w } else { rest.min(right_w) };
            let at = pos2(row.max.x - w, row.center().y);
            let id = Id::new("mp3-tabs-right");
            // A point over, so the words' own width never reads as a cut.
            if studio::text_button_sized(ui, id, at, &words, None, face.ports, w + 1.0, face.takes(), pal, RIGHT_SIZE).clicked() {
                acts.push(act);
            }
        }
        None => {
            let runs = studio::figured(&words, face.ports, body(RIGHT_SIZE), mono(12.5), right_tone);
            studio::runs(ui, pos2(row.max.x - right_w, row.center().y), &runs, right_w + 1.0, Align2::LEFT_CENTER);
        }
    }
}

/// A tab in `rect`, its name cut to `name_w`: the slab while its subject is
/// in view, else bare, raised under the pointer. Whether a click landed.
fn draw_tab(ui: &Ui, face: &Face, tab: &Tab, rect: Rect, name_w: f32) -> bool {
    let pal = face.pal;
    let response = face.takes().then(|| studio::hand(ui.interact(rect, tab.id, Sense::click())));
    let hot = !tab.open && response.as_ref().is_some_and(|r| r.hovered());
    let (ink, mark) = if tab.open {
        ui.painter().rect_filled(rect, studio::square(), pal.accent);
        (pal.on_accent, pal.on_accent)
    } else if hot {
        ui.painter().rect_filled(rect, studio::square(), pal.raised);
        (pal.text, tab.tone)
    } else {
        (pal.dim, tab.tone)
    };
    let mid = rect.center().y;
    let runs = tab.runs(ink, hot || tab.open);
    let name = studio::runs(ui, pos2(rect.min.x + TAB_PAD, mid), &runs, name_w + 1.0, Align2::LEFT_CENTER);
    let x = name.max.x.max(rect.min.x + TAB_PAD) + TAB_GAP;
    // The check's and the cross's boxes centred on the row, as the boards
    // centre them: their foot is no letter's to stand on a baseline.
    match tab.look() {
        Look::Check => studio::check(ui, pos2(x + 5.0, mid), mark),
        Look::Cross => studio::cross(ui, pos2(x + 5.0, mid), mark),
        Look::Words(font) => {
            studio::text(ui, pos2(x, mid), &tab.mark, font, mark, rect.max.x - x, Align2::LEFT_CENTER);
        }
    }
    response.is_some_and(|r| r.clicked())
}

#[cfg(test)]
mod tests;
