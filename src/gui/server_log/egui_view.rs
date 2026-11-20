//! The server's log drawn by egui, in the desktop window's panel
//! (src/gui/window/panel.rs): the same [`LogUi`] the cells' [`draw`](super::draw) draws —
//! its lines, following or paused, its highlight, its copy and its
//! download — in the Studio look of the Admin screen's canvas (the boards
//! Main, the log docked beside Libraries, and Log, the log as a room of its
//! own, paused with a run highlighted; 2026-11-20). Only the drawing is new.
//!
//! A line is what the cells say it is: the rows are the model's own
//! ([`view`], [`wrap_line`](super::wrap_line) through it), wrapped at the width in monospace
//! characters that fit beside the clock's ten, and the view holds as many
//! whole rows as fit its height, so the two faces agree on every line, every
//! step and every page. Only the lines on screen are laid out, as in cells:
//! the view wraps what it shows and the bar's test wraps a row past it,
//! never the whole ring. The view moves by whole lines through the model —
//! the wheel's notches as ↑ and ↓, the bar's thumb as its track — never by
//! pixels, and follows again as it does in cells.
//!
//! Every click is one of the log's own [`LogAct`]s, applied in the pass
//! through [`LogUi::act`] as the cells' click is through the host: the
//! paused word follows, copy and download do what `y` and `d` do, a press
//! on the bar's track, or the thumb dragged, moves the bottom line, and a
//! press-drag on the rows highlights whole lines through the model's own
//! hold, which reads the pointer in cells — here, cells of the rows' grid,
//! a monospace character wide and a row tall. egui selects no text here:
//! the rows are painted, not labels. Keys never reach egui: they stay the
//! GUI's, which hands them to [`LogUi::key`] as ever, measured by the rows
//! this pass fitted. What a copy or a download says goes to the GUI's note
//! through the host, as in cells.
//!
//! Built only with the window: the terminal build has no panel to draw in.

use egui::epaint::CornerRadius;
use egui::{Align2, CursorIcon, Event, Id, MouseWheelUnit, PointerButton, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use ratatui::layout::{Position, Rect as Cells};
use rust_i18n::t;

use super::{Drawn, Fit, LATER_LINES, Level, LogAct, LogUi, Look, clock, counted, needs_bar, view};
use crate::gui::window::studio::{
    Icon, Pal, TOOL_H, Tool, body, caps, caps_width, led, mono, panel, square, text, tool_button,
    tool_button_width, width_of,
};
use crate::kit::Grip;

/// A row of lines, and the monospace it is set in: the canvas's 12-point
/// Hack on 17-point rows.
const ROW_H: f32 = 17.0;
const MONO: f32 = 12.0;
/// The header's height: the canvas's 48 points over the column and the
/// room, a band's lower, since it has eleven of the window's rows.
const HEAD_H: f32 = 48.0;
const BAND_HEAD_H: f32 = 36.0;
/// The room inside the panel: the header's and the rows' left edge, the
/// header's right, and the room over and under the rows.
const PAD_L: f32 = 16.0;
const PAD_R: f32 = 12.0;
const PAD_Y: f32 = 10.0;
const BAND_PAD_Y: f32 = 6.0;
/// The follow state's capitals, the gap between the header's controls,
/// the least room left of them, and the hint's face.
const STATE_SIZE: f32 = 11.5;
const CONTROL_GAP: f32 = 8.0;
const STATE_GAP: f32 = 16.0;
const HINT_SIZE: f32 = 12.5;
/// The bar's column beside the rows, its thumb, and the shortest thumb.
const BAR_W: f32 = 14.0;
const THUMB_W: f32 = 6.0;
const THUMB_RIGHT: f32 = 4.0;
const THUMB_MIN: f32 = 20.0;
/// How far a highlighted run's fill reaches left of the clock.
const FILL_IN: f32 = 8.0;
/// Where the rows' grid of cells starts, for the hold: far from zero, so
/// a pointer above the rows or left of them still has a cell of its own
/// (the model clamps it to the first row), and a drag there is a move.
const ORIGIN: u16 = 1 << 14;

/// One pass of the log over the whole of `ui` — its rect in the panel —
/// with the header's hint as `look` says and the tooltips' key tails while
/// `key_hints` is on. An `inert` log (a room's modal is up, or a GUI
/// overlay owns the pointer) neither lights nor shows the hand, and takes
/// no press, click or wheel; a drag held on its rows or its thumb when it
/// turned inert goes on to its release, as the cells' grip does, so no
/// hold outlives the button.
pub(crate) fn show(ui: &mut Ui, log: &mut LogUi, look: &Look, key_hints: bool, inert: bool) {
    let pal = Pal::now();
    let full = ui.max_rect();
    let id = Id::new("server-log");
    panel(ui, full, &pal);
    // The whole log is a target that takes the keys, as its cells are:
    // the host hands it the focus on a press anywhere in it, and the hand
    // shows anywhere over it, its controls drawn over this.
    if !inert {
        let _ = ui.interact(full, id.with("whole"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    }
    let at = Geom::fit(ui, full, look.spaced, log);
    if !inert && ui.rect_contains_pointer(full) {
        let turned = notches(ui, id, at.rows);
        for _ in 0..turned.unsigned_abs() {
            log.wheel(turned > 0);
        }
    }
    let mut drawn = lay_out(log, &at);
    if pointer(ui, id, log, &at, &drawn, inert) {
        drawn = lay_out(log, &at);
    }
    paint_rows(ui, log, &at, &drawn, &pal);
    paint_bar(ui, id, log, &at, &drawn, inert, &pal);
    // The header last, in the state the pass left: what its own controls
    // do comes after, and the next pass, at once, shows it.
    let acts = header(ui, id, at.head, log, look, key_hints, inert, &pal);
    if !acts.is_empty() {
        for act in acts {
            log.act(act);
        }
        ui.ctx().request_repaint();
    }
}

/// Where the log's parts stand this pass, and the rows and cells the
/// model fits its lines to.
struct Geom {
    head: Rect,
    /// The rows' area: from the panel's left edge to the lines' right (the
    /// bar's left, while it stands), as many whole rows tall as fit,
    /// standing on the panel's foot as the canvas sets the newest line
    /// (with the failure's row under them, while it shows); what is left
    /// of a row's height goes over them, under the header. The press-drag
    /// region, over the empty rows under the lines too.
    rows_at: Rect,
    /// Where a row's clock starts, a monospace character's width, and the
    /// lines' right edge.
    text_x: f32,
    char_w: f32,
    right: f32,
    rows: usize,
    cells: usize,
    /// The bar's column beside the rows, while the lines do not all fit.
    bar: Option<Rect>,
    /// A failure takes the row under the lines, while lines are held.
    error_row: bool,
}

impl Geom {
    /// The header, the rows and the bar in `full`, the cells a message row
    /// gets past the clock, and the view settled in them before anything
    /// reads it, as the cells' draw does: the rows whole in the height, the
    /// bar decided at the whole width (so it never comes and goes with the
    /// room it takes), and what the keys measure a step and a page by.
    fn fit(ui: &Ui, full: Rect, spaced: bool, log: &mut LogUi) -> Geom {
        let (head_h, pad_y) = if spaced { (HEAD_H, PAD_Y) } else { (BAND_HEAD_H, BAND_PAD_Y) };
        let head = Rect::from_min_max(full.min, pos2(full.max.x, (full.min.y + head_h).min(full.max.y)));
        let char_w = (width_of(ui, "0000000000", mono(MONO)) / 10.0).max(1.0);
        let text_x = full.min.x + PAD_L;
        let top = head.max.y + pad_y;
        let foot = full.max.y - pad_y;
        let below = ((foot - top) / ROW_H).floor().max(0.0) as usize;
        let held = log.model.shown().len();
        let error_row = log.model.error.is_some() && held > 0 && below > 0;
        let rows = below - usize::from(error_row);
        let cells_to = |right: f32| (((right - text_x) / char_w).floor().max(0.0) as usize).saturating_sub(LATER_LINES.len());
        let whole_width = cells_to(full.max.x - PAD_R);
        let bar = needs_bar(&log.model.shown(), rows, whole_width);
        let right = if bar { full.max.x - BAR_W } else { full.max.x - PAD_R };
        let cells = if bar { cells_to(right).min(whole_width.saturating_sub(1)) } else { whole_width };
        log.rows = rows;
        log.cells = Some(cells);
        log.model.settle(Fit { rows, cells: Some(cells) });
        let first = (foot - below as f32 * ROW_H).max(top);
        let rows_at = Rect::from_min_size(pos2(full.min.x, first), vec2((right - full.min.x).max(0.0), rows as f32 * ROW_H));
        let bar = bar.then(|| Rect::from_min_max(pos2(full.max.x - BAR_W, rows_at.min.y), pos2(full.max.x, rows_at.max.y)));
        Geom { head, rows_at, text_x, char_w, right, rows, cells, bar, error_row }
    }

    /// The cell of the rows' grid under `p` (see [`ORIGIN`]).
    fn cell(&self, p: Pos2) -> Position {
        let along = |v: f32, size: f32| (f32::from(ORIGIN) + (v / size).floor()).clamp(0.0, f32::from(u16::MAX)) as u16;
        Position { x: along(p.x - self.text_x, self.char_w), y: along(p.y - self.rows_at.min.y, ROW_H) }
    }

    /// Row `i`'s rect, across the lines' width.
    fn row(&self, i: usize) -> Rect {
        let y = self.rows_at.min.y + i as f32 * ROW_H;
        Rect::from_min_max(pos2(self.text_x, y), pos2(self.right, y + ROW_H))
    }
}

/// The rows the view shows now, each mapped to its line for the hold —
/// the rows' grid, its first row at [`ORIGIN`] — and a drag in progress
/// re-read against them, as the cells' draw does, so the wheel or lines
/// arriving under a still pointer move the highlight with the lines.
fn lay_out(log: &mut LogUi, at: &Geom) -> Vec<Drawn> {
    let drawn = view(&log.model.shown(), log.model.scroll, log.model.under, at.rows, at.cells);
    let seqs: Vec<u64> = drawn.iter().map(|row| row.seq).collect();
    let span = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
    log.rows_at = (!seqs.is_empty()).then(|| (Cells { x: ORIGIN, y: ORIGIN, width: span(at.cells), height: span(at.rows) }, seqs));
    if let Some(hold) = log.hold.filter(|h| h.moved)
        && let Some(head) = log.seq_at(hold.at.y)
    {
        log.model.highlight = Some((hold.anchor, head));
    }
    drawn
}

/// The wheel's whole notches over the log this pass, up (older) positive:
/// a line's turn is a notch, as the window's grid counts one, a pixel
/// turn a row's height of points, a page the view's rows. What is left of
/// a notch waits for the next turn the same way, and a turn the other way
/// starts afresh.
fn notches(ui: &Ui, id: Id, rows: usize) -> i64 {
    let key = id.with("wheel");
    let turns: Vec<f32> = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|event| match event {
                Event::MouseWheel { unit, delta, .. } => Some(match unit {
                    MouseWheelUnit::Line => delta.y,
                    MouseWheelUnit::Point => delta.y / ROW_H,
                    MouseWheelUnit::Page => delta.y * rows.max(1) as f32,
                }),
                _ => None,
            })
            .collect()
    });
    if turns.is_empty() {
        return 0;
    }
    let mut left: f32 = ui.ctx().data(|d| d.get_temp(key)).unwrap_or(0.0);
    for lines in turns.into_iter().filter(|l| l.is_finite()) {
        if lines * left < 0.0 {
            left = 0.0;
        }
        left += lines;
    }
    let whole = left.trunc();
    ui.ctx().data_mut(|d| d.insert_temp(key, left - whole));
    whole.clamp(-1000.0, 1000.0) as i64
}

/// The bar's numbers this pass: the lines held, and [`counted`]'s first,
/// whole and positions; None while there is nothing to move.
fn bar_numbers(log: &LogUi, drawn: &[Drawn]) -> Option<(usize, usize, usize, usize)> {
    let held = log.model.shown().len();
    let bottom = held.checked_sub(1 + log.model.scroll)?;
    let (first, whole, of) = counted(drawn, bottom, log.model.under, held);
    (of > 0).then_some((held, first.min(of), whole, of))
}

/// The thumb's top and length on `track`: as long as the share of the
/// lines the view shows whole, no shorter than a fingertip, and standing
/// where `first` stands among the `of` positions past the first.
fn thumb(track: Rect, held: usize, whole: usize, first: usize, of: usize) -> (f32, f32) {
    let h = track.height();
    let len = (h * whole as f32 / held.max(1) as f32).clamp(THUMB_MIN.min(h), h);
    (track.min.y + (h - len) * (first as f32 / of.max(1) as f32), len)
}

/// The position past the first that a thumb `len` long with its top at
/// `top` stands for: the track's top is the top of the log, its foot the
/// newest line, which follows.
fn thumb_at(track: Rect, len: f32, top: f32, of: usize) -> usize {
    let room = track.height() - len;
    if room <= 0.0 {
        return of;
    }
    (((top - track.min.y) / room).clamp(0.0, 1.0) * of as f32).round() as usize
}

/// The pointer's presses, moves and releases this pass, in order, as the
/// cells' surface hands them on: a press on the rows begins the hold (the
/// model's [`LogAct::Select`], which clears the highlight), its moves
/// wherever the hand goes extend it and its release ends it; a press on the
/// bar's track moves the bottom line where it lands and arms the thumb, a
/// press on the thumb takes it where it is pressed and moves nothing, and
/// the thumb then follows the hand until the release, kept by where on it
/// it was taken. An `inert` log hears no press, but a hold or a thumb
/// taken before goes on to its release. True when the view moved.
fn pointer(ui: &Ui, id: Id, log: &mut LogUi, at: &Geom, drawn: &[Drawn], inert: bool) -> bool {
    let key = id.with("thumb");
    let mut grab: Option<f32> = ui.ctx().data(|d| d.get_temp(key));
    let numbers = at.bar.zip(bar_numbers(log, drawn));
    let events = ui.input(|i| i.events.clone());
    let mut moved = false;
    let mut jump = |log: &mut LogUi, top: f32, len: f32, track: Rect, of: usize| {
        log.act(LogAct::Jump { at: thumb_at(track, len, top, of), of });
        moved = true;
    };
    for event in &events {
        match *event {
            Event::PointerButton { button: PointerButton::Primary, pressed: true, .. } if inert => {}
            Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, .. } => {
                if let Some((track, (held, first, whole, of))) = numbers
                    && track.contains(pos)
                {
                    let (top, len) = thumb(track, held, whole, first, of);
                    if (top..=top + len).contains(&pos.y) {
                        grab = Some(pos.y - top);
                    } else {
                        grab = Some(len / 2.0);
                        jump(log, pos.y - len / 2.0, len, track, of);
                    }
                } else if at.rows_at.contains(pos) && log.rows_at.is_some() {
                    log.act(LogAct::Select(Grip::Press, at.cell(pos)));
                }
            }
            Event::PointerMoved(pos) => {
                if log.hold.is_some() {
                    log.act(LogAct::Select(Grip::Drag, at.cell(pos)));
                }
                if let (Some(taken), Some((track, (held, first, whole, of)))) = (grab, numbers) {
                    let (_, len) = thumb(track, held, whole, first, of);
                    jump(log, pos.y - taken, len, track, of);
                }
            }
            Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, .. } => {
                if log.hold.is_some() {
                    log.act(LogAct::Select(Grip::Release, at.cell(pos)));
                }
                grab = None;
            }
            _ => {}
        }
    }
    // A release this pass never heard of (the window let go): no grab.
    if !ui.input(|i| i.pointer.primary_down()) {
        grab = None;
    }
    ui.ctx().data_mut(|d| {
        if let Some(taken) = grab {
            d.insert_temp(key, taken);
        } else {
            d.remove::<f32>(key);
        }
    });
    moved
}

/// The rows, the newest line's last at the bottom of the window onto them:
/// a line's first row opens on its clock, its later ones under the
/// message's first character, every one in the line's colour — the danger
/// red for an error, gold for a warning, the text colour for the rest —
/// and a highlighted run filled in the accent, its rows in the selection's
/// ink. With nothing to show, the one sentence; a failure while lines are
/// held takes the row under them.
fn paint_rows(ui: &Ui, log: &LogUi, at: &Geom, drawn: &[Drawn], pal: &Pal) {
    if at.rows == 0 && !at.error_row {
        return;
    }
    let clip = Rect::from_min_max(pos2(at.text_x - FILL_IN, at.rows_at.min.y), pos2(at.right, at.rows_at.max.y));
    let painter = ui.painter().with_clip_rect(clip.intersect(ui.clip_rect()));
    let lit: Vec<bool> = drawn.iter().map(|row| log.model.lit(row.seq)).collect();
    let mut i = 0;
    while i < drawn.len() {
        if !lit[i] {
            i += 1;
            continue;
        }
        let run = lit[i..].iter().take_while(|on| **on).count();
        let fill = Rect::from_min_max(pos2(at.text_x - FILL_IN, at.row(i).min.y), pos2(at.right, at.row(i + run - 1).max.y));
        painter.rect_filled(fill, square(), pal.accent);
        i += run;
    }
    let message_x = at.text_x + LATER_LINES.len() as f32 * at.char_w;
    for (i, row) in drawn.iter().enumerate() {
        let rect = at.row(i);
        let ink = match (lit[i], row.level) {
            (true, _) => pal.on_accent,
            (false, Level::Error) => pal.danger,
            (false, Level::Warn) => pal.gold,
            (false, Level::Other) => pal.text,
        };
        let put = |x: f32, s: String, color| {
            let galley = ui.painter().layout_no_wrap(s, mono(MONO), color);
            painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, color);
        };
        if row.first {
            put(at.text_x, clock(row.at, log.zone.as_ref()), if lit[i] { pal.on_accent } else { pal.dim });
        }
        if !row.text.is_empty() {
            put(message_x, row.text.clone(), ink);
        }
    }

    let held = log.model.shown().len();
    let sentence = |row: usize, words: &str, color| {
        let y = at.rows_at.min.y + (row as f32 + 0.5) * ROW_H;
        text(ui, pos2(at.text_x, y), words, body(13.0), color, (at.right - at.text_x).max(1.0), Align2::LEFT_CENTER);
    };
    if let Some(error) = &log.model.error {
        sentence(if at.error_row { at.rows } else { 0 }, error, pal.gold);
    } else if held == 0 {
        let words = if !log.model.answered {
            t!("gui.admin.log.waiting")
        } else if log.model.capacity == Some(0) {
            t!("gui.admin.log.off")
        } else {
            t!("gui.admin.log.empty")
        };
        sentence(0, &words, pal.dim);
    }
}

/// The bar, counted in lines (see [`counted`]): only its thumb, as the
/// canvas draws it, a slim bar in the strong edge, dim under the pointer
/// and the accent while the hand holds it.
fn paint_bar(ui: &Ui, id: Id, log: &LogUi, at: &Geom, drawn: &[Drawn], inert: bool, pal: &Pal) {
    let Some((track, (held, first, whole, of))) = at.bar.zip(bar_numbers(log, drawn)) else { return };
    let (top, len) = thumb(track, held, whole, first, of);
    let rect = Rect::from_min_size(pos2(track.max.x - THUMB_RIGHT - THUMB_W, top), vec2(THUMB_W, len));
    let held_now = ui.ctx().data(|d| d.get_temp::<f32>(id.with("thumb"))).is_some();
    let color = if held_now {
        pal.accent
    } else if !inert && ui.rect_contains_pointer(track) {
        pal.dim
    } else {
        pal.edge_strong
    };
    ui.painter().rect_filled(rect, CornerRadius::same(3), color);
}

/// What a header tooltip says: the whole of it while key names are shown,
/// else the words before its key's tail, as the cells' `tip_keyed` cuts it.
fn keyed(tip: &str, key_hints: bool) -> String {
    if key_hints {
        return tip.to_string();
    }
    tip.rsplit_once(" — ").map_or(tip, |(label, _)| label).to_string()
}

/// The header: the follow state at the left — the ok green's lit LED and
/// "following", or "paused" by an unlit one, a control that follows again —
/// and at the right copy and download, each drawn whole or not at all in
/// the cells' order, the first that does not fit ending the row, with the
/// hint (what `L` does here, while key names are shown) before them, the
/// first to give way. While a download runs its control reads
/// "downloading…" in the accent and takes no click. Returns the clicks.
#[allow(clippy::too_many_arguments)]
fn header(ui: &Ui, id: Id, head: Rect, log: &LogUi, look: &Look, key_hints: bool, inert: bool, pal: &Pal) -> Vec<LogAct> {
    let mut acts = Vec::new();
    ui.painter().hline(head.x_range(), head.max.y - 0.5, Stroke::new(1.0, pal.edge));
    let mid = head.center().y;
    let (left, right) = (head.min.x + PAD_L, head.max.x - PAD_R);
    let following = log.model.following();
    let state = if following { t!("gui.admin.log.following") } else { t!("gui.admin.log.paused") };
    let state_w = 14.0 + caps_width(ui, &state, STATE_SIZE);

    let copy = t!("gui.admin.log.copy").to_string();
    let busy = log.downloading();
    let download = if busy { t!("gui.admin.log.downloading") } else { t!("gui.admin.log.download") }.to_string();
    let widths = [tool_button_width(ui, &copy), tool_button_width(ui, &download)];
    let room = right - left;
    let mut used = state_w;
    let mut shown = 0;
    for w in widths {
        let gap = if shown == 0 { STATE_GAP } else { CONTROL_GAP };
        if used + gap + w > room {
            break;
        }
        used += gap + w;
        shown += 1;
    }
    let hint = look.hint.as_deref().map(|hint| (hint, width_of(ui, hint, body(HINT_SIZE))));
    let hint = hint.filter(|(_, w)| used + STATE_GAP + w <= room);

    // From the right edge: the download, the copy, the hint.
    let mut x = right;
    let live = if inert { Tool::Inert } else { Tool::Live };
    let mut slots = Vec::new();
    for k in (0..shown).rev() {
        slots.push((k, Rect::from_min_max(pos2(x - widths[k], mid - TOOL_H / 2.0), pos2(x, mid + TOOL_H / 2.0))));
        x -= widths[k] + CONTROL_GAP;
    }
    for (k, rect) in slots {
        let (response, tip, kind) = if k == 0 {
            (tool_button(ui, id.with("copy"), rect, Icon::Copy, &copy, live, pal), t!("gui.admin.log.tip_copy"), live)
        } else {
            let kind = if busy { Tool::Busy } else { live };
            let response = tool_button(ui, id.with("download"), rect, Icon::Download, &download, kind, pal);
            (response, t!("gui.admin.log.tip_download"), kind)
        };
        if kind == Tool::Live {
            let response = response.on_hover_text(keyed(&tip, key_hints));
            if response.clicked() {
                acts.push(if k == 0 { LogAct::CopyLines } else { LogAct::Download });
            }
        }
    }
    let controls_left = if shown > 0 { x + CONTROL_GAP } else { right };
    if let Some((hint, w)) = hint {
        text(ui, pos2(controls_left - STATE_GAP, mid), hint, body(HINT_SIZE), pal.dim, w + 1.0, Align2::RIGHT_CENTER);
    }

    // The follow state, cut with an ellipsis where the row is narrower.
    let state_room = (if shown > 0 { controls_left - STATE_GAP } else { right } - left - 14.0).max(1.0);
    if following {
        led(ui, pos2(left + 3.0, mid), pal.ok);
        caps(ui, pos2(left + 14.0, mid), &state, STATE_SIZE, pal.ok, state_room, Align2::LEFT_CENTER);
    } else {
        let rect = Rect::from_min_max(pos2(left - 8.0, mid - TOOL_H / 2.0), pos2(left + state_w.min(state_room + 14.0) + 8.0, mid + TOOL_H / 2.0));
        let hovered = if inert {
            false
        } else {
            let response = ui.interact(rect, id.with("follow"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            if response.clicked() {
                acts.push(LogAct::Follow);
            }
            response.hovered()
        };
        if hovered {
            ui.painter().rect_filled(rect, square(), pal.raised);
        }
        let ink = if hovered { pal.text } else { pal.dim };
        led(ui, pos2(left + 3.0, mid), pal.unlit);
        caps(ui, pos2(left + 14.0, mid), &state, STATE_SIZE, ink, state_room, Align2::LEFT_CENTER);
    }
    acts
}

#[cfg(test)]
mod tests {
    use egui::epaint::Shape;
    use egui::{FontFamily, Modifiers, Vec2};

    use super::*;
    use crate::api::types::{ActivityEntry, LogTail};
    use crate::gui::server_log::message_rows;

    fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        guard
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        ctx
    }

    fn entry(seq: u64, level: &str, message: &str) -> ActivityEntry {
        ActivityEntry {
            seq,
            t: format!("2026-10-02T09:{:02}:{:02}.000Z", seq / 60 % 60, seq % 60),
            level: level.to_string(),
            message: message.to_string(),
        }
    }

    /// A log on UTC holding `entries`.
    fn holding(entries: Vec<ActivityEntry>) -> LogUi {
        let last_seq = entries.last().map_or(0, |e| e.seq);
        let mut log = LogUi::new(None).utc();
        log.model.take(LogTail { entries, last_seq, capacity: 1000 });
        log
    }

    /// `n` info lines numbered from 1.
    fn lines(n: u64) -> LogUi {
        holding((1..=n).map(|s| entry(s, "info", &format!("line {s}"))).collect())
    }

    fn column() -> Look {
        Look { spaced: true, hint: None }
    }

    /// The log's rect in the panel: the docked column at 176×46, 54 cells
    /// by 38 rows, at 8×16 points a cell.
    const SIZE: Vec2 = vec2(432.0, 608.0);

    /// One pass of the log over a `size` rect at the origin, with `events`.
    fn pass(ctx: &egui::Context, log: &mut LogUi, look: &Look, events: Vec<Event>) -> egui::FullOutput {
        pass_inert(ctx, log, look, events, false)
    }

    fn pass_inert(ctx: &egui::Context, log: &mut LogUi, look: &Look, events: Vec<Event>, inert: bool) -> egui::FullOutput {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| show(ui, log, look, false, inert));
        // No GPU here to take the font atlas: let go of it on purpose, or
        // epaint's debug check fails the drop.
        out.textures_delta.clear();
        out
    }

    fn button(at: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::default() }
    }

    /// A press and a release at `at`, a pass each after the move.
    fn click(ctx: &egui::Context, log: &mut LogUi, look: &Look, at: Pos2) {
        let _ = pass(ctx, log, look, vec![Event::PointerMoved(at)]);
        let _ = pass(ctx, log, look, vec![button(at, true)]);
        let _ = pass(ctx, log, look, vec![button(at, false)]);
    }

    /// Every text the pass drew, with its face's family and where it stood.
    fn texts(out: &egui::FullOutput) -> Vec<(String, FontFamily, Pos2)> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => {
                    let family = text.galley.job.sections.first().map_or(FontFamily::Proportional, |s| s.format.font_id.family.clone());
                    Some((text.galley.text().to_string(), family, text.pos))
                }
                _ => None,
            })
            .collect()
    }

    /// The top of the column's first row: the whole rows that fit under
    /// the header, standing on the panel's foot.
    fn first_row() -> f32 {
        let foot = SIZE.y - PAD_Y;
        foot - ((foot - HEAD_H - PAD_Y) / ROW_H).floor() * ROW_H
    }

    /// The middle of row `i` of the column's lines, a little past its clock.
    fn row(i: f32) -> Pos2 {
        pos2(PAD_L + 30.0, first_row() + (i + 0.5) * ROW_H)
    }

    /// A monospace character's width, as a pass measures it.
    fn char_w(ctx: &egui::Context) -> f32 {
        let mut w = 0.0;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| w = width_of(ui, "0000000000", mono(MONO)) / 10.0);
        out.textures_delta.clear();
        w
    }

    /// The colour a text shape was laid out in.
    fn ink_of(out: &egui::FullOutput, s: &str) -> Option<egui::Color32> {
        out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == s => t.galley.job.sections.first().map(|section| section.format.color),
            _ => None,
        })
    }

    #[test]
    fn lines_are_set_in_the_monospace_and_wrapped_at_the_width_that_fits() {
        let _en = english();
        let ctx = context();
        let long = "scan finished: 1204 files in /srv/music/library, 17 new, 3 changed, 0 removed, and the album art cache was rebuilt from the embedded pictures";
        let mut log = holding(vec![entry(1, "info", "Server started on port 3000"), entry(2, "info", long), entry(3, "error", "Error: ENOENT\n    at open (fs.js:1:1)\n\n    at main")]);
        let _ = pass(&ctx, &mut log, &column(), Vec::new());
        let out = pass(&ctx, &mut log, &column(), Vec::new());
        let cells = log.cells.expect("the pass fitted the cells");
        // The cells that fit: the panel less its room, at the monospace's
        // width, less the clock's ten.
        let char_w = char_w(&ctx);
        let want = ((SIZE.x - PAD_R - PAD_L) / char_w).floor() as usize - LATER_LINES.len();
        assert_eq!(cells, want, "the width in monospace characters that fit");
        assert!(log.rows > 20, "the height in whole rows: {}", log.rows);

        let drawn = texts(&out);
        let rows = message_rows(&log.model.shown()[1].whole, cells);
        assert!(rows.len() > 1, "the long line wraps: {rows:?}");
        for row in &rows {
            let (_, family, _) = drawn.iter().find(|(s, _, _)| s == row).unwrap_or_else(|| panic!("{row:?} drawn: {drawn:?}"));
            assert_eq!(*family, FontFamily::Monospace, "{row:?} in the monospace");
        }
        // The later rows stand under the message's first character, the
        // clock's ten cells in.
        let x_of = |s: &str| drawn.iter().find(|(d, _, _)| d == s).map(|(_, _, at)| at.x).unwrap();
        assert!((x_of(&rows[1]) - x_of(&rows[0])).abs() < 0.5, "under the first");
        assert!((x_of(&rows[0]) - x_of("09:00:02") - 10.0 * char_w).abs() < 0.5);
        assert!(drawn.iter().any(|(s, _, _)| s == "    at open (fs.js:1:1)"), "a trace keeps its indentation");
        assert!(drawn.iter().all(|(s, _, _)| s.chars().count() <= cells || s.starts_with("09:")), "no row past the width");
        assert!(drawn.iter().any(|(s, _, _)| s == "FOLLOWING"), "the header follows: {drawn:?}");
        let pal = Pal::now();
        assert_eq!(ink_of(&out, "Error: ENOENT"), Some(pal.danger), "an error in the danger colour");
        assert_eq!(ink_of(&out, &rows[0]), Some(pal.text), "the rest in the text colour");
        assert_eq!(ink_of(&out, "09:00:02"), Some(pal.dim), "the clock dim");
    }

    #[test]
    fn the_wheel_moves_whole_lines_through_the_model_and_pauses() {
        let _en = english();
        let ctx = context();
        let mut log = lines(80);
        let over = pos2(200.0, 300.0);
        let wheel = |unit, y| Event::MouseWheel { unit, delta: vec2(0.0, y), phase: egui::TouchPhase::Move, modifiers: Modifiers::default() };
        let _ = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(over)]);
        assert!(log.model.following());
        let out = pass(&ctx, &mut log, &column(), vec![wheel(MouseWheelUnit::Line, 2.0)]);
        assert_eq!(log.model.scroll, 2, "a notch a line, up older");
        assert!(!log.model.following(), "which pauses it");
        let drawn = texts(&out);
        assert!(drawn.iter().any(|(s, _, _)| s == "PAUSED"), "on the same pass: {drawn:?}");
        assert!(drawn.iter().any(|(s, _, _)| s == "line 78"), "the bottom line is the one two above the newest");
        assert!(!drawn.iter().any(|(s, _, _)| s == "line 79"));

        // Points: less than a row's turn waits, the rest makes a line.
        let _ = pass(&ctx, &mut log, &column(), vec![wheel(MouseWheelUnit::Point, ROW_H * 0.6)]);
        assert_eq!(log.model.scroll, 2, "no pixel scrolling");
        let _ = pass(&ctx, &mut log, &column(), vec![wheel(MouseWheelUnit::Point, ROW_H * 0.6)]);
        assert_eq!(log.model.scroll, 3);
        // Down to the newest follows again.
        let _ = pass(&ctx, &mut log, &column(), vec![wheel(MouseWheelUnit::Line, -5.0)]);
        assert!(log.model.following(), "back at the newest, it follows");

        // Off the log, the wheel is nobody's here.
        let _ = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(pos2(SIZE.x + 50.0, 10.0)), wheel(MouseWheelUnit::Line, 3.0)]);
        assert!(log.model.following());
    }

    #[test]
    fn a_drag_over_the_rows_highlights_whole_lines_and_a_plain_click_clears() {
        let _en = english();
        let ctx = context();
        let long = "a line long enough to wrap onto a second row of the log at this width, and then some more words to be sure of it";
        let mut entries: Vec<ActivityEntry> = (1..=6).map(|s| entry(s, "info", &format!("line {s}"))).collect();
        entries.push(entry(7, "warn", long));
        entries.push(entry(8, "info", "line 8"));
        let mut log = holding(entries);
        // Rows 0-5 are lines 1-6; the warning takes rows 6 on.
        let _ = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(row(1.0))]);
        let _ = pass(&ctx, &mut log, &column(), vec![button(row(1.0), true)]);
        let _ = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(row(3.0))]);
        assert_eq!(log.model.highlight, Some((2, 4)), "from the pressed line to the one under the hand");
        let out = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(row(7.0)), button(row(7.0), false)]);
        assert_eq!(log.model.highlight, Some((2, 7)), "any row of a wrapped line means the whole line");
        assert_eq!(log.model.highlighted().len(), 6);
        assert!(log.hold.is_none(), "the release ended the hold");
        // Every row of the run in the selection's ink, the clocks too; the
        // lines beside it as they were.
        let pal = Pal::now();
        assert_eq!(ink_of(&out, "line 2"), Some(pal.on_accent), "a lit row's words in the on-accent ink");
        assert_eq!(ink_of(&out, "09:00:02"), Some(pal.on_accent), "and its clock");
        assert_eq!(ink_of(&out, "line 1"), Some(pal.text));
        assert_eq!(ink_of(&out, "line 8"), Some(pal.text));

        // A drag above the rows stops at the first line drawn.
        let above = pos2(PAD_L, HEAD_H - 20.0);
        let _ = pass(&ctx, &mut log, &column(), vec![button(row(4.0), true)]);
        let _ = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(above), button(above, false)]);
        assert_eq!(log.model.highlight, Some((5, 1)));

        // A plain click (no move off its cell) clears it.
        let _ = pass(&ctx, &mut log, &column(), vec![button(row(2.0), true)]);
        let _ = pass(&ctx, &mut log, &column(), vec![button(row(2.0), false)]);
        assert_eq!(log.model.highlight, None);
    }

    #[test]
    fn each_click_does_what_the_cells_click_on_the_same_thing_does() {
        let _en = english();
        let ctx = context();
        let mut log = lines(80);
        let _ = pass(&ctx, &mut log, &column(), Vec::new());
        let id = Id::new("server-log");

        // The paused word follows again.
        log.wheel(true);
        let _ = pass(&ctx, &mut log, &column(), Vec::new());
        let follow = ctx.read_response(id.with("follow")).expect("the paused control");
        click(&ctx, &mut log, &column(), follow.rect.center());
        assert!(log.model.following(), "the paused word follows");

        // Copy: every line shown, through the kit's clipboard, and the note.
        crate::kit::clipboard::catch(crate::kit::clipboard::Copied::Clipboard);
        let copy = ctx.read_response(id.with("copy")).expect("the copy control");
        click(&ctx, &mut log, &column(), copy.rect.center());
        assert_eq!(crate::kit::clipboard::caught().len(), 1);
        assert_eq!(log.take_note(100), Some((t!("gui.admin.log.copied_all").to_string(), false)));

        // The bar: its track's top is the top of the log, its foot follows.
        let out = pass(&ctx, &mut log, &column(), Vec::new());
        let thumbs: Vec<Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Rect(r) if r.rect.width() == THUMB_W && r.rect.min.x > SIZE.x - BAR_W => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(thumbs.len(), 1, "one thumb: {thumbs:?}");
        let track_foot = first_row() + log.rows as f32 * ROW_H;
        assert!((track_foot - (SIZE.y - PAD_Y)).abs() < 0.5, "the rows stand on the panel's foot");
        assert!((thumbs[0].max.y - track_foot).abs() < 0.5, "at the track's foot while following: {thumbs:?}");
        let bar_x = SIZE.x - BAR_W / 2.0;
        click(&ctx, &mut log, &column(), pos2(bar_x, first_row() + 2.0));
        assert!(!log.model.following());
        let oldest = log.model.shown()[0].seq;
        let out = pass(&ctx, &mut log, &column(), Vec::new());
        assert!(texts(&out).iter().any(|(s, _, _)| *s == format!("line {oldest}")), "the top of the log");
        // Dragged to the foot, the thumb follows the hand and the log
        // follows again.
        let foot = pos2(bar_x, SIZE.y - 30.0);
        let _ = pass(&ctx, &mut log, &column(), vec![button(pos2(bar_x, first_row() + 2.0), true)]);
        let _ = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(foot)]);
        assert!(log.model.following(), "the thumb rode the hand to the newest line");
        let _ = pass(&ctx, &mut log, &column(), vec![button(foot, false)]);

        // Download: on its own thread, the busy word, the note.
        let dir = std::env::temp_dir().join(format!("mstream-log-egui-d-{}", std::process::id()));
        let mut log = lines(5).saving_into(dir.clone());
        log.start(crate::api::Client::new_with("http://host.invalid:3000", false).expect("a client"));
        log.model.take(LogTail { entries: (1..=5).map(|s| entry(s, "info", &format!("line {s}"))).collect(), last_seq: 5, capacity: 1000 });
        let _ = pass(&ctx, &mut log, &column(), Vec::new());
        let download = ctx.read_response(id.with("download")).expect("the download control");
        click(&ctx, &mut log, &column(), download.rect.center());
        assert!(log.downloading(), "the download runs");
        assert_eq!(log.take_note(100), Some((t!("gui.admin.log.fetching").to_string(), false)));
        let out = pass(&ctx, &mut log, &column(), Vec::new());
        assert!(texts(&out).iter().any(|(s, _, _)| *s == t!("gui.admin.log.downloading").to_uppercase()), "the busy word");
        log.stop();
    }

    #[test]
    fn under_a_modal_the_log_takes_no_click_and_shows_no_hand() {
        let _en = english();
        let ctx = context();
        let mut log = lines(80);
        let _ = pass(&ctx, &mut log, &column(), Vec::new());
        let copy = ctx.read_response(Id::new("server-log").with("copy")).expect("the copy control").rect.center();
        crate::kit::clipboard::catch(crate::kit::clipboard::Copied::Clipboard);
        let _ = crate::kit::clipboard::caught();
        let mut hand = false;
        for events in [vec![Event::PointerMoved(copy)], vec![button(copy, true)], vec![button(copy, false)]] {
            let out = pass_inert(&ctx, &mut log, &column(), events, true);
            hand |= out.platform_output.cursor_icon == CursorIcon::PointingHand;
        }
        assert!(crate::kit::clipboard::caught().is_empty(), "nothing copied");
        assert!(!hand, "no hand");
        assert_eq!(log.model.highlight, None);
        let _ = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(row(3.0))]);
        let out = pass(&ctx, &mut log, &column(), Vec::new());
        assert_eq!(out.platform_output.cursor_icon, CursorIcon::PointingHand, "live, the hand anywhere over the log");
    }

    #[test]
    fn a_drag_held_when_the_log_turns_inert_still_ends_at_its_release() {
        let _en = english();
        let ctx = context();
        let mut log = lines(8);
        let _ = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(row(1.0))]);
        let _ = pass(&ctx, &mut log, &column(), vec![button(row(1.0), true)]);
        let _ = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(row(3.0))]);
        assert_eq!(log.model.highlight, Some((2, 4)));
        // A room's modal comes up under the held drag: the drag goes on to
        // its release, as the cells' grip does, and the release ends it.
        let _ = pass_inert(&ctx, &mut log, &column(), vec![Event::PointerMoved(row(4.0))], true);
        let _ = pass_inert(&ctx, &mut log, &column(), vec![button(row(4.0), false)], true);
        assert!(log.hold.is_none(), "the release ended the hold");
        assert_eq!(log.model.highlight, Some((2, 5)));
        // The hand moving on with the button up highlights nothing more.
        let _ = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(row(6.0))]);
        assert_eq!(log.model.highlight, Some((2, 5)));
        // And an inert log starts no drag.
        let _ = pass_inert(&ctx, &mut log, &column(), vec![button(row(2.0), true)], true);
        assert!(log.hold.is_none());
        assert_eq!(log.model.highlight, Some((2, 5)), "nor clears one");
        let _ = pass_inert(&ctx, &mut log, &column(), vec![button(row(2.0), false)], true);
    }

    #[test]
    fn a_press_on_the_thumb_moves_nothing_until_the_hand_does() {
        let _en = english();
        let ctx = context();
        // A line taller than the view, read from a row above its last: the
        // view stands neither at the top nor following.
        let tall = (0..60).map(|i| format!("row {i}")).collect::<Vec<_>>().join("\n");
        let mut entries: Vec<ActivityEntry> = (1..=5).map(|s| entry(s, "info", &format!("line {s}"))).collect();
        entries.push(entry(6, "info", &tall));
        let mut log = holding(entries);
        let _ = pass(&ctx, &mut log, &column(), Vec::new());
        log.wheel(true);
        let (scroll, under) = (log.model.scroll, log.model.under);
        assert!(!log.model.following() && under > 0, "reading the tall line: {scroll}, {under}");
        let out = pass(&ctx, &mut log, &column(), Vec::new());
        let thumb = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Rect(r) if r.rect.width() == THUMB_W && r.rect.min.x > SIZE.x - BAR_W => Some(r.rect),
                _ => None,
            })
            .expect("the thumb");
        let _ = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(thumb.center())]);
        let _ = pass(&ctx, &mut log, &column(), vec![button(thumb.center(), true)]);
        assert_eq!((log.model.scroll, log.model.under), (scroll, under), "the press took the thumb and moved nothing");
        // The hand takes it to the track's top: the top of the log.
        let top = pos2(thumb.center().x, first_row() - 40.0);
        let out = pass(&ctx, &mut log, &column(), vec![Event::PointerMoved(top)]);
        assert!(texts(&out).iter().any(|(s, _, _)| s == "line 1"), "the oldest line in view");
        let _ = pass(&ctx, &mut log, &column(), vec![button(top, false)]);
    }
}
