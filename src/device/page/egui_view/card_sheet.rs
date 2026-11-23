//! A card's Settings sheet drawn by egui (contract clauses 48–50), after
//! the canvas's Settings board (Settings.dc.html): the kit's Dialog over
//! the page's area and its scrim, the page inert beneath, for the card it
//! was opened for — and the Set-up sheet it is for a new card, the same
//! sheet on Music alone with Cancel and Set up.
//!
//! The sheet is the page's (page/card.rs's `CardSheet`): its tab, its rows
//! (`CardSheet::rows`), the row with the keyboard, the Name field while it
//! is open and the list that hangs from From or Into. The keys still edit
//! it (`card_sheet_key`); this draws what it holds, in the cells' order and
//! their words (cells/card.rs's `draw_card_sheet`): the tab row, the rows,
//! the fit line under them, the rule and the help about the row with the
//! keyboard, and the foot. The keyboard's row wears the kit's cursor look
//! (the raised fill and the accent's ring), its words in the accent's
//! semibold as the cells bold them; egui's own focus is never asked for.
//!
//! A click asks for the act the cells register for the same place, and
//! only where they register one: a row's `CardSheetRow` (the keyboard
//! taken there, and Space's act: a tick flipped, a choice turned, the Name
//! field or a list opened), a tab's `CardSheetTab`, an entry of the list
//! that hangs `CardSheetPick`, the foot's `CardDefaults` (or, setting up,
//! `CardSheetClose`) and `CardApply`, and the close's `CardSheetClose`.
//!
//! Short of room, as the cells give way: the rule and the help go first,
//! then the room between the blocks closes up, and what is left scrolls in
//! the Dialog's body, the row with the keyboard brought into its view, and
//! brought in again when its field or its list opens. A list hangs only
//! from a box in that view.

use egui::{Align2, Color32, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};

use super::super::card::{CardSheet, SRow, SheetTab, card_name, codec_words, list_entries, server_name};
use super::super::cells::card::row_help;
use super::super::*;
use super::{Drawn, Face, close_tip};
use crate::device::board::gb;
use crate::device::card::budget::DEFAULT_MB;
use crate::device::card::engine::Removal;
use crate::device::card::plan::{Fit, TRACK_CAP};
use crate::device::card::state::Convert;
use crate::device::card::worker::{CardView, Next, ServerLink};
use crate::gui::window::studio::{self, BOX, Dialog, FIELD_H, Pal, Para, SEG_H, Seg, TICK_H, body, mono, strong};

/// The sheet's window: the canvas's forms'.
const SHEET_W: f32 = 640.0;
/// The body of the sheet's tallest state, where its top is held (the cells
/// hold it at their 22 rows, cells/card.rs's `CARD_SHEET_MAX`): the board's
/// 619-point window, less the 2 points of frame the kit draws inside its
/// edge, its 48 of padding, its title's 20 and the 20 under the title.
const HELD_BODY: f32 = 529.0;
/// The room between the body's blocks (the tab row, the rows, the fit
/// line, the help, the foot), and where the body is short of room once the
/// help has gone.
const BLOCK_GAP: f32 = 20.0;
const TIGHT_GAP: f32 = 10.0;
/// The room between two rows (the kit's tick gap), and how far a row's
/// look reaches past the body either side, as the kit's ticks reach.
const ROW_GAP: f32 = 4.0;
const ROW_OUT: f32 = 8.0;
/// A row's label column: the board card's 140 points, the label cut 12
/// short of the value.
const LABEL_W: f32 = 140.0;
const LABEL_CLEAR: f32 = 12.0;
/// A row's value, the board's 14 points.
const VALUE: f32 = 14.0;
/// A closed list's box (From, Into), the kit's: 30 points tall, 2 clear of
/// the row's ring.
const BOX_H: f32 = 30.0;
/// A choice of a pair: its words where the kit's choice's stand, and 24
/// apart.
const RADIO_IN: f32 = studio::RADIO_IN;
const RADIO_APART: f32 = 24.0;
/// A tick's words after its box.
const TICK_IN: f32 = BOX + 10.0;
/// A library's size at its row's right: the board's 10 points clear of the
/// name, and the least of the name it leaves before it is left out.
const SIZE_GAP: f32 = 10.0;
const NAME_LEAST: f32 = 48.0;
/// The fit's lines, the help's lines and the room under the rule, the
/// room over a tight fit's dim line.
const FIT_LINE: f32 = 20.0;
const HELP_LINE: f32 = 18.0;
const UNDER_RULE: f32 = 10.0;
const MORE_GAP: f32 = 4.0;
/// The reveal's numbers for the Writing tab's rows, apart from Music's, so
/// a tab switched to brings its first row in as a move would; and for a
/// row with its field or its list open, apart from the row, so the one
/// that opens brings its row in as the keyboard's move to it would.
const WRITING: usize = 1_000;
const OPENED: usize = 2_000;

/// The card's Settings sheet over `face.full`, its top held where its
/// tallest state stands. What a click asked for comes back as the cells'
/// acts; the Name field, while it has the keyboard, draws the input
/// method's composition and gives back its caret.
pub(super) fn show(ui: &mut Ui, page: &mut Page, face: &Face) -> Drawn {
    let mut drawn = Drawn::default();
    let Some(sheet) = page.card.sheet.as_ref() else { return drawn };
    let Some(card) = page.card_of(&sheet.key) else { return drawn };
    let servers = page.servers();
    let server = sheet
        .want
        .server
        .as_ref()
        .and_then(|id| servers.iter().find(|s| &s.id == id))
        .map(|s| s.name.clone())
        .unwrap_or_else(|| server_name(&card));
    let view = View { sheet, card: &card, servers: &servers, server };
    let title = if sheet.setup {
        t!("dev.card.setup_title", drive = card.drive()).to_string()
    } else {
        t!("dev.card.set_title", card = card_name(&card)).to_string()
    };
    let id = Id::new("mp3-card-sheet");
    let ctx = ui.ctx().clone();
    // The top held for the tallest state, or for this one where it is
    // taller still, which the cells centre whole: what the last run of the
    // body measured, run again when that moved it.
    let held_key = id.with("held");
    let held = ctx.data(|d| d.get_temp::<f32>(held_key)).unwrap_or(HELD_BODY);
    let shown = Dialog::new(id, face.full, SHEET_W, &title).hold(held).show(ui, face.pal, |ui| sheet_body(ui, &view, face, id));
    let wanted = HELD_BODY.max(shown.inner.height);
    if (wanted - held).abs() > 0.5 {
        ctx.data_mut(|d| d.insert_temp(held_key, wanted));
        ctx.request_discard("the card's sheet held its top for a taller body");
    }
    drawn.acts.extend(shown.inner.acts);
    drawn.caret = shown.inner.caret;
    if let (Some(cursor), Some(anchor)) = (sheet.list, shown.inner.hang)
        && let Some(i) = hang(ui, &view, face, id, anchor, cursor)
    {
        drawn.acts.push(Act::CardSheetPick(i));
    }
    // The cells' tooltip on their [X], its key named as their surface
    // names keys.
    close_tip(ui, id, face);
    if shown.closed {
        drawn.acts.push(Act::CardSheetClose);
    }
    drawn
}

/// What the sheet's drawing reads: the sheet, the card it is for, this
/// computer's saved servers, and the server From names (the help's, the
/// peers' header's and Removals' words), as the cells name it.
struct View<'a> {
    sheet: &'a CardSheet,
    card: &'a CardView,
    servers: &'a [ServerLink],
    server: String,
}

/// What a run of the body hands back: the acts its clicks asked for, the
/// Name field's caret while it has the keyboard, where the list that hangs
/// hangs from (its box's left and width, the row's foot), and how tall the
/// body stands.
#[derive(Default)]
struct Body {
    acts: Vec<Act>,
    caret: Option<Pos2>,
    hang: Option<Rect>,
    height: f32,
}

/// A line of the rows' block, in the cells' order: a row of the tab in
/// view by its number in `CardSheet::rows`, or the header over a peer's
/// libraries.
enum Line {
    Row(usize, SRow),
    Peer(String),
}

impl Line {
    /// A row with a box in its value (the Name field's, a closed list's)
    /// stands the field's height, so the field that opens in its place
    /// moves nothing; the others a tick's.
    fn height(&self) -> f32 {
        match self {
            Line::Row(_, SRow::Name | SRow::From | SRow::Into) => FIELD_H,
            _ => TICK_H,
        }
    }
}

/// The fit under the rows (clause 49): asked and not yet answered, the
/// kit's searching shape and its words; answered, its LED, its words in
/// their tone and a tight fit's dim line under them.
enum Fitted {
    Asking(String),
    Said { lamp: Color32, tone: Color32, words: String, more: Option<String> },
}

/// The body's blocks as this run lays them, and how tall each stands.
struct Plan {
    tabs: bool,
    lines: Vec<Line>,
    fit: Option<(Fitted, f32)>,
    help: Option<(String, f32)>,
    foot: (String, String),
    foot_h: f32,
    gap: f32,
}

impl Plan {
    /// The sheet's body for a column `ui`'s width, measured in an invisible
    /// child of it, and given way where `ui`'s room is short of it: the
    /// rule and the help first, then the room between the blocks.
    fn of(ui: &mut Ui, v: &View, pal: &Pal) -> Plan {
        let w = ui.available_width();
        let room = ui.available_height();
        let mut probe = ui.new_child(UiBuilder::new().id_salt("mp3-card-sheet-measure").max_rect(ui.max_rect()));
        probe.set_invisible();
        let at = probe.max_rect().min;
        let fit = fitted(v, pal).map(|fit| {
            let h = match &fit {
                Fitted::Asking(_) => FIT_LINE,
                Fitted::Said { tone, words, more, .. } => {
                    let said = fit_para(words, *tone).paint(&probe, at, (w - studio::STATE_INDENT).max(1.0)).height();
                    said + more.as_ref().map_or(0.0, |_| MORE_GAP + HELP_LINE)
                }
            };
            (fit, h)
        });
        let help = row_help(&v.sheet.row(), &v.sheet.offered, &v.server);
        let help_h = 1.0 + UNDER_RULE + help_para(&help, pal).paint(&probe, at, w).height();
        let foot = if v.sheet.setup {
            (t!("dev.card.btn_cancel").to_string(), t!("dev.card.btn_do_setup").to_string())
        } else {
            (t!("dev.card.btn_defaults").to_string(), t!("dev.card.btn_apply").to_string())
        };
        let foot_h = studio::foot_rects(ui, &[], &[(&foot.0, false), (&foot.1, true)], w).1;
        let mut plan = Plan { tabs: !v.sheet.setup, lines: lines(v), fit, help: Some((help, help_h)), foot, foot_h, gap: BLOCK_GAP };
        if plan.height() > room + 0.5 {
            plan.help = None;
        }
        if plan.height() > room + 0.5 {
            plan.gap = TIGHT_GAP;
        }
        plan
    }

    /// The rows' block's height.
    fn rows_h(&self) -> f32 {
        self.lines.iter().map(Line::height).sum::<f32>() + ROW_GAP * self.lines.len().saturating_sub(1) as f32
    }

    /// The body's height: its blocks and the room between them.
    fn height(&self) -> f32 {
        let blocks = [
            self.tabs.then_some(SEG_H),
            Some(self.rows_h()),
            self.fit.as_ref().map(|(_, h)| *h),
            self.help.as_ref().map(|(_, h)| *h),
            Some(self.foot_h),
        ];
        let shown: Vec<f32> = blocks.into_iter().flatten().collect();
        shown.iter().sum::<f32>() + self.gap * shown.len().saturating_sub(1) as f32
    }
}

/// The rows' lines (the cells' loop): each row of the tab in view, and over
/// each peer's first library the header that names it.
fn lines(v: &View) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut peer_head: Option<u32> = None;
    for (i, row) in v.sheet.rows().into_iter().enumerate() {
        match &row {
            SRow::Buttons => continue,
            SRow::PeerLibrary(id, _) if peer_head != Some(*id) => {
                peer_head = Some(*id);
                lines.push(Line::Peer(peer_words(v, *id)));
            }
            _ => {}
        }
        lines.push(Line::Row(i, row));
    }
    lines
}

/// A peer's header: the peer, the server From names and the budget a day.
fn peer_words(v: &View, id: u32) -> String {
    let name = v.sheet.peers.iter().find(|(p, ..)| *p == id).map(|(_, n, _)| n.clone()).unwrap_or_default();
    let mb = fmt_count(v.sheet.want.budget_mb.unwrap_or(DEFAULT_MB));
    t!("dev.card.grp_peer", peer = name, server = v.server.clone(), mb = mb).to_string()
}

/// The fit line as the cells choose it: the sheet's own answer, by its
/// request's number, else the card's last look; asked, the scan; a plan
/// with a fit, its words; anything else, none.
fn fitted(v: &View, pal: &Pal) -> Option<Fitted> {
    let card = v.card;
    let own = card.fit.as_ref().filter(|(seq, _)| *seq == v.sheet.seq).map(|(_, next)| next);
    let size = card.found.volume.space.map_or(0, |s| s.size);
    match own.unwrap_or(&card.next) {
        Next::Asking => Some(Fitted::Asking(t!("dev.card.sync_planning").to_string())),
        Next::Plan(plan) => plan.fit.map(|fit| said(&fit, size, pal)),
        _ => None,
    }
}

/// A fit's words and tones from a plan's fit and the card's size, as the
/// cells' `fit_line` says them (the same keys with the same figures): the
/// cells' ✓ as the LED in the ok green with the words plain; their ! and ✗
/// as the LED in the gold with the words gold too, a tight fit's dim line
/// under them.
fn said(fit: &Fit, size: u64, pal: &Pal) -> Fitted {
    let used = size.saturating_sub(fit.room).saturating_add(fit.needed);
    let gold = |words: String, more: Option<String>| Fitted::Said { lamp: pal.gold, tone: pal.gold, words, more };
    if fit.over_cap() {
        let words = t!("dev.card.fit_cap", n = fmt_count(fit.tracks as u64), max = fmt_count(TRACK_CAP as u64));
        return gold(words.to_string(), None);
    }
    if !fit.fits() {
        let head = t!("dev.card.fit_no", gb = gb(fit.short()));
        let over = t!("dev.card.fit_over", used = gb(used), size = gb(size));
        return gold(format!("{head} · {over}"), None);
    }
    let spare = fit.room.saturating_sub(fit.needed);
    let after = t!("dev.card.fit_after", used = gb(used), size = gb(size), spare = gb(spare));
    if size > 0 && spare.saturating_mul(20) < size {
        return gold(format!("{} · {after}", t!("dev.card.fit_tight")), Some(t!("dev.card.fit_tight_2").to_string()));
    }
    Fitted::Said { lamp: pal.ok, tone: pal.text, words: format!("{} · {after}", t!("dev.card.fit_ok")), more: None }
}

/// A card's or a library's size in the locale's unit, as the cells say it
/// (cells/card.rs's `size_words`).
fn size_words(bytes: u64) -> String {
    t!("dev.card_size", size = gb(bytes)).to_string()
}

/// The fit's words: 13.5 points in `tone` on 20-point lines, their
/// figures in the monospace.
fn fit_para(words: &str, tone: Color32) -> Para {
    Para::runs(&studio::figured(words, &[], body(13.5), mono(12.5), tone)).spaced(FIT_LINE)
}

/// The help's words: two dim 13-point lines at most, on 18, an ellipsis
/// past them, their figures in the monospace.
fn help_para(words: &str, pal: &Pal) -> Para {
    Para::runs(&studio::figured(words, &[], body(13.0), mono(12.5), pal.dim)).spaced(HELP_LINE).rows(2)
}

/// The sheet's body, laid out top-down in `ui`, the Dialog's: the tab row
/// (not setting up), the rows, the fit, the rule and the help, the foot.
fn sheet_body(ui: &mut Ui, v: &View, face: &Face, id: Id) -> Body {
    ui.spacing_mut().item_spacing.y = 0.0;
    let pal = face.pal;
    let plan = Plan::of(ui, v, pal);
    let mut out = Body { height: plan.height(), ..Body::default() };
    let sheet = v.sheet;
    let focus = sheet.focus.min(sheet.rows().len().saturating_sub(1));
    // The keyboard's row wears its look only while it is the row's, not a
    // field's or a list's it opened.
    let calm = sheet.field.is_none() && sheet.list.is_none();
    let writing = sheet.tab == SheetTab::Writing;
    let at = focus + if writing { WRITING } else { 0 } + if calm { 0 } else { OPENED };
    // The body's view, scrolled or not: what a list may hang from.
    let view = ui.clip_rect();
    let w = ui.available_width();
    if plan.tabs {
        tabs(ui, v, face, id, &mut out.acts);
        ui.add_space(plan.gap);
    }
    for (k, line) in plan.lines.iter().enumerate() {
        if k > 0 {
            ui.add_space(ROW_GAP);
        }
        let (slot, _) = ui.allocate_exact_size(vec2(w, line.height()), Sense::hover());
        match line {
            Line::Peer(words) => {
                let runs = studio::figured(words, &[], body(13.0), mono(12.5), pal.dim);
                studio::runs(ui, slot.left_center(), &runs, slot.width(), Align2::LEFT_CENTER);
            }
            Line::Row(i, row) => {
                // A row's id is its tab's and its number's: a press taken
                // on one and let go after a key turned the tab is lost with
                // its row, never the click of the other tab's row that now
                // stands under it.
                let row_id = id.with("row").with(writing).with(*i);
                let row_drawn = row_at(ui, v, face, row_id, slot, row, *i == focus && calm);
                if row_drawn.clicked {
                    out.acts.push(Act::CardSheetRow(*i));
                }
                out.caret = out.caret.or(row_drawn.caret);
                if *i == focus {
                    studio::reveal(ui, id, at, row_drawn.rect);
                    // A list hangs only from a box in the body's view, as
                    // the cells hang none from a row they cut: never over
                    // the title, its close or past the window.
                    if sheet.list.is_some() {
                        let shown = row_drawn.boxed.filter(|b| view.intersects(*b));
                        out.hang = shown.map(|b| Rect::from_min_max(b.min, pos2(b.max.x, slot.max.y)));
                    }
                }
            }
        }
    }
    if let Some((fit, h)) = &plan.fit {
        ui.add_space(plan.gap);
        fit_block(ui, fit, *h, pal);
    }
    if let Some((help, h)) = &plan.help {
        ui.add_space(plan.gap);
        let (rect, _) = ui.allocate_exact_size(vec2(w, *h), Sense::hover());
        studio::rule(ui, rect.min.x, rect.max.x, rect.min.y, pal);
        help_para(help, pal).paint(ui, pos2(rect.min.x, rect.min.y + 1.0 + UNDER_RULE), w);
    }
    ui.add_space(plan.gap);
    let on_buttons = calm && sheet.row() == SRow::Buttons;
    if let Some(act) = foot(ui, v, face, id, &plan, on_buttons, at) {
        out.acts.push(act);
    }
    out
}

/// The tab row (contract clause 48, the kit's segmented control as the
/// board draws it): Music and Writing, the tab in view lit, and at the
/// row's right, dim in the monospace, the card's drive and size, gone whole
/// where they would come within 16 points of the tabs. A click on a tab is
/// the cells' `CardSheetTab`, the open one's too (the keyboard back on its
/// first row).
fn tabs(ui: &mut Ui, v: &View, face: &Face, id: Id, acts: &mut Vec<Act>) {
    let pal = face.pal;
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, SEG_H), Sense::hover());
    let labels = [t!("dev.card.tab_music").to_string(), t!("dev.card.tab_writing").to_string()];
    let chosen = usize::from(v.sheet.tab == SheetTab::Writing);
    let tabs_id = id.with("tabs");
    let (control, clicked) = studio::segmented(ui, tabs_id, row.min.x, row.center().y, &labels, chosen, Seg::Lit, pal);
    // The kit's control reports the others; the cells take the open one too.
    let again = ui.ctx().read_response(tabs_id.with(chosen)).is_some_and(|r| r.clicked());
    if let Some(i) = clicked.or(again.then_some(chosen)) {
        acts.push(Act::CardSheetTab(i));
    }
    if let Some(space) = v.card.found.volume.space {
        let right = format!("{} · {}", v.card.drive(), size_words(space.size));
        let right_w = studio::width_of(ui, &right, mono(12.5));
        if control.max.x + 16.0 + right_w <= row.max.x {
            studio::text(ui, pos2(row.max.x, row.center().y), &right, mono(12.5), pal.dim, right_w + 1.0, Align2::RIGHT_CENTER);
        }
    }
}

/// What a row's drawing hands back: whether a click landed on it, where
/// its look stands (what is brought into view), the box of its closed
/// list, and the Name field's caret.
struct RowDrawn {
    clicked: bool,
    rect: Rect,
    boxed: Option<Rect>,
    caret: Option<Pos2>,
}

/// A row of the sheet in its `slot` (the body's width), its look reaching
/// 8 points past either side: the keyboard's look while `lit`, raised under
/// the pointer, the whole of it the click the cells register on the row.
/// Its label in the dim small capitals, then its value as its kind draws
/// it, its words in the accent's semibold while `lit`.
fn row_at(ui: &Ui, v: &View, face: &Face, id: Id, slot: Rect, row: &SRow, lit: bool) -> RowDrawn {
    let pal = face.pal;
    let sheet = v.sheet;
    let look = slot.expand2(vec2(ROW_OUT, 0.0));
    let response = face.takes().then(|| studio::hand(ui.interact(look, id, Sense::CLICK)));
    let hovered = response.as_ref().is_some_and(|r| r.hovered());
    if lit {
        studio::cursor_look(ui, look, pal);
    } else if hovered {
        ui.painter().rect_filled(look, studio::square(), pal.raised);
    }
    let mut drawn = RowDrawn { clicked: response.is_some_and(|r| r.clicked()), rect: look, boxed: None, caret: None };
    let (font, ink) = if lit { (strong(VALUE), pal.accent) } else { (body(VALUE), pal.text) };
    let mid = slot.center().y;
    let value_x = slot.min.x + LABEL_W;
    let room = (slot.max.x - value_x).max(1.0);
    let label = |key: &str| {
        studio::small_caps(ui, pos2(slot.min.x, mid), &t!(key), pal.dim, LABEL_W - LABEL_CLEAR, Align2::LEFT_CENTER);
    };
    let words = |s: &str, at: f32| studio::text(ui, pos2(at, mid), s, font.clone(), ink, (slot.max.x - at).max(1.0), Align2::LEFT_CENTER);
    match row {
        SRow::Name => {
            label("dev.card.row_name");
            match &sheet.field {
                // The field in the value column, the keyboard's, its caret
                // for the input method; a click on it is the row's.
                Some(field) => {
                    let rect = Rect::from_min_max(pos2(value_x, slot.min.y), pos2(slot.max.x, slot.max.y));
                    let input = studio::Input::of(field).focused(true, face.composition).enabled(face.takes());
                    let typed = studio::field_box(ui, id.with("name"), rect, &input, pal);
                    drawn.clicked |= typed.response.clicked();
                    drawn.caret = typed.caret;
                }
                None => {
                    words(&sheet.want.name, value_x);
                }
            }
        }
        SRow::From => {
            label("dev.card.grp_from");
            let picked = sheet.want.server.as_ref().and_then(|id| v.servers.iter().find(|s| &s.id == id));
            let name = picked.map_or_else(|| "—".to_string(), |s| s.name.clone());
            drawn.boxed = Some(closed_box(ui, value_x, mid, slot.max.x, &name, lit, pal));
        }
        SRow::Library(lib) => {
            let on = sheet.libraries().contains(lib);
            let size = sheet.offered.iter().find(|(l, _)| l == lib).and_then(|(_, s)| *s);
            let right = size.map(|(n, b)| format!("{} · {}", size_words(b), t!("dev.card_tracks", n = fmt_count(n as u64))));
            tick_line(ui, slot, on, hovered, lib, right.as_deref(), (&font, ink), pal);
        }
        SRow::PeerLibrary(peer, lib) => {
            let on = sheet.want.peers.iter().any(|(p, libs)| p == peer && libs.contains(lib));
            tick_line(ui, slot, on, hovered, lib, None, (&font, ink), pal);
        }
        SRow::Convert => {
            label("dev.card.row_convert");
            let needed = sheet.want.convert == Convert::Unplayable;
            let choices = [(t!("dev.card.opt_needed").to_string(), needed), (t!("dev.card.opt_all").to_string(), !needed)];
            radios(ui, value_x, mid, slot.max.x, &choices, (&font, ink), pal);
        }
        SRow::Into => {
            label("dev.card.row_into");
            let codec = codec_words(sheet.want.codec, sheet.want.bitrate);
            drawn.boxed = Some(closed_box(ui, value_x, mid, slot.max.x, &codec, lit, pal));
        }
        SRow::Removals => {
            label("dev.card.row_removals");
            let follow = sheet.want.removal == Removal::Mirror;
            let mirror = t!("dev.card.opt_follow", server = v.server.clone()).to_string();
            let choices = [(mirror, follow), (t!("dev.card.opt_never").to_string(), !follow)];
            radios(ui, value_x, mid, slot.max.x, &choices, (&font, ink), pal);
        }
        SRow::Covers | SRow::AutoDj => {
            let (key, on, what) = match row {
                SRow::Covers => ("dev.card.row_covers", sheet.want.covers, t!("dev.card.opt_covers")),
                _ => ("dev.card.row_autodj", sheet.want.auto_dj, t!("dev.card.opt_index")),
            };
            label(key);
            let mark = Rect::from_min_size(pos2(value_x, mid - BOX / 2.0), Vec2::splat(BOX));
            studio::tick_box(ui, mark, on, hovered, pal.panel, pal);
            words(&what, value_x + TICK_IN);
        }
        SRow::For => {
            label("dev.card.row_for");
            let me = !sheet.want.for_someone_else;
            let choices = [(t!("dev.card.opt_me").to_string(), me), (t!("dev.card.opt_else").to_string(), !me)];
            radios(ui, value_x, mid, slot.max.x, &choices, (&font, ink), pal);
        }
        SRow::Player => {
            // Plain words, never the accent: the cells set them raw, as
            // device.txt says them.
            label("dev.card.row_player");
            let device = v.card.facts().map(|f| f.device.clone()).filter(|d| d.present);
            let said = match device {
                Some(d) => t!("dev.card.player_txt", name = "M5Stack Core2", version = d.firmware).to_string(),
                None => t!("dev.card.player_none").to_string(),
            };
            let runs = studio::figured(&said, &[], body(VALUE), mono(12.5), pal.text);
            studio::runs(ui, pos2(value_x, mid), &runs, room, Align2::LEFT_CENTER);
        }
        SRow::Budget => {
            label("dev.card.row_budget");
            budget(ui, value_x, mid, slot.max.x, sheet.want.budget_mb.unwrap_or(DEFAULT_MB), (&font, ink), pal);
        }
        SRow::Buttons => {}
    }
    drawn
}

/// A tick and its library's name across `slot`, the box at the body's
/// left on the panel's fill (its edge brightened while the row is
/// `hovered`), the name 10 points after it in the row's face and ink, and
/// at the slot's right, dim with its figures in the monospace, `right` (a
/// library's size and tracks), whole. A long name gives way to it, cut with
/// an ellipsis 10 points short of it, as the board's row cuts its name
/// beside the size; only where the name would keep less than its least is
/// the size left out.
#[allow(clippy::too_many_arguments)]
fn tick_line(ui: &Ui, slot: Rect, on: bool, hovered: bool, name: &str, right: Option<&str>, (font, ink): (&FontId, Color32), pal: &Pal) {
    let mid = slot.center().y;
    let mark = Rect::from_min_size(pos2(slot.min.x, mid - BOX / 2.0), Vec2::splat(BOX));
    studio::tick_box(ui, mark, on, hovered, pal.panel, pal);
    let x = slot.min.x + TICK_IN;
    let mut end = slot.max.x;
    if let Some(right) = right {
        let runs = studio::figured(right, &[], body(13.0), mono(12.5), pal.dim);
        let right_w = studio::runs_width(ui, &runs);
        if x + NAME_LEAST + SIZE_GAP + right_w <= slot.max.x {
            studio::runs(ui, pos2(slot.max.x, mid), &runs, right_w + 1.0, Align2::RIGHT_CENTER);
            end = slot.max.x - right_w - SIZE_GAP;
        }
    }
    studio::text(ui, pos2(x, mid), name, font.clone(), ink, (end - x).max(1.0), Align2::LEFT_CENTER);
}

/// A closed list's box from `x` on the row's middle `mid` (From's server,
/// Into's codec): the kit's (`studio::select_box`), [`BOX_H`] tall, its
/// words in the row's 14 points, in the accent's semibold while the row is
/// `lit`, as wide as they are in that semibold and the kit's least at
/// least, no further than `end`. The row takes the click and the pointer.
/// Returns where.
fn closed_box(ui: &Ui, x: f32, mid: f32, end: f32, words: &str, lit: bool, pal: &Pal) -> Rect {
    let w = studio::select_box_width(ui, words, &[], VALUE).min(end - x).max(1.0);
    let rect = Rect::from_min_size(pos2(x, mid - BOX_H / 2.0), vec2(w, BOX_H));
    studio::select_box(ui, rect, words, &[], VALUE, lit, false, pal);
    rect
}

/// A pair of choices on one line from `x` (Convert, Removals, For), each
/// the kit's round mark — its chosen one in the row's ink, the accent on
/// the keyboard's row and the text colour on any other, as the cells set
/// the whole row in its style — and its words in the row's face and ink,
/// the second 24 points after the first, cut at `end`. The row takes the
/// click, as the cells' does: either choice turns it to the other.
fn radios(ui: &Ui, x: f32, mid: f32, end: f32, choices: &[(String, bool)], (font, ink): (&FontId, Color32), pal: &Pal) {
    let mut x = x;
    for (words, on) in choices {
        if x + RADIO_IN >= end {
            break;
        }
        studio::radio_mark(ui, pos2(x + 7.0, mid), *on, ink, pal);
        let drawn = studio::text(ui, pos2(x + RADIO_IN, mid), words, font.clone(), ink, end - x - RADIO_IN, Align2::LEFT_CENTER);
        x = drawn.max.x + RADIO_APART;
    }
}

/// The peer budget from `x` (the cells' "- ▰▰▰▱ +  1,500 MB a day per
/// peer"): the minus, the desk's ten LEDs lit one for every 300 MB, the
/// plus, then its words with the figure in the monospace, in the row's
/// face and ink. The row takes the click, as the cells': 250 MB more.
fn budget(ui: &Ui, x: f32, mid: f32, end: f32, mb: u64, (font, ink): (&FontId, Color32), pal: &Pal) {
    let lit = (mb / 300).min(10) as usize;
    step_mark(ui, pos2(x + 6.0, mid), false, ink);
    let bar = studio::meter(ui, pos2(x + 18.0, mid), 10, lit, vec2(5.0, 10.0), 2.0, pal.accent, pal);
    step_mark(ui, pos2(bar.max.x + 12.0, mid), true, ink);
    let from = bar.max.x + 28.0;
    let words = t!("dev.card.budget_mb", mb = fmt_count(mb)).to_string();
    let runs = studio::figured(&words, &[], font.clone(), mono(12.5), ink);
    studio::runs(ui, pos2(from, mid), &runs, (end - from).max(1.0), Align2::LEFT_CENTER);
}

/// A minus, or a plus, centred on `at`: the canvas's 12-point strokes.
fn step_mark(ui: &Ui, at: Pos2, plus: bool, color: Color32) {
    let stroke = Stroke::new(1.6, color);
    ui.painter().line_segment([at - vec2(4.0, 0.0), at + vec2(4.0, 0.0)], stroke);
    if plus {
        ui.painter().line_segment([at - vec2(0.0, 4.0), at + vec2(0.0, 4.0)], stroke);
    }
}

/// The fit's block, `h` tall, the body wide: asked, the kit's searching
/// shape (ten unlit segments) and its words in the accent; answered, the
/// LED at the line's start, the words 16 points in wrapped beside it, and
/// a tight fit's dim line under them.
fn fit_block(ui: &mut Ui, fit: &Fitted, h: f32, pal: &Pal) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    match fit {
        Fitted::Asking(words) => {
            studio::searching_at(ui, pos2(rect.min.x, rect.min.y + FIT_LINE / 2.0), words, w, pal);
        }
        Fitted::Said { lamp, tone, words, more } => {
            let at = pos2(rect.min.x + studio::STATE_INDENT, rect.min.y);
            let room = (w - studio::STATE_INDENT).max(1.0);
            let drawn = fit_para(words, *tone).paint(ui, at, room);
            // On the middle of the first line's small letters.
            let face = studio::metrics(ui, &body(13.5));
            studio::led(ui, pos2(rect.min.x + 3.0, rect.min.y + face.ascent - face.small / 2.0), *lamp);
            if let Some(more) = more {
                let y = drawn.max.y + MORE_GAP + HELP_LINE / 2.0;
                studio::text(ui, pos2(at.x, y), more, body(13.0), pal.dim, room, Align2::LEFT_CENTER);
            }
        }
    }
}

/// The foot (the kit's, its two at its end): Use defaults and Apply, or
/// setting up, Cancel and Set up, the second the lit primary. While the
/// keyboard is on the buttons (`on_buttons`) the one it is on wears the
/// accent's ring just outside it, and the foot is brought into the body's
/// view. The act a click asks for, the cells' for the button.
fn foot(ui: &mut Ui, v: &View, face: &Face, id: Id, plan: &Plan, on_buttons: bool, at: usize) -> Option<Act> {
    let pal = face.pal;
    let w = ui.available_width();
    let top = ui.cursor().min;
    let end = [(plan.foot.0.as_str(), false), (plan.foot.1.as_str(), true)];
    let clicked = studio::foot(ui, id.with("foot"), &[], &end, None, pal);
    if on_buttons {
        let (rects, h) = studio::foot_rects(ui, &[], &end, w);
        if let Some(rect) = rects.get(usize::from(v.sheet.on_apply)) {
            studio::button_ring(ui, rect.translate(top.to_vec2()), pal);
        }
        studio::reveal(ui, id, at, Rect::from_min_size(top, vec2(w, h)));
    }
    let other = if v.sheet.setup { Act::CardSheetClose } else { Act::CardDefaults };
    clicked.map(|k| if k == 0 { other } else { Act::CardApply })
}

/// The list that hangs from a closed list's box (From's servers, Into's
/// codecs): the kit's list that hangs, on a layer of its own over the
/// Dialog's (`studio::hung_over`), so it hangs over the sheet's lower rows
/// past the Dialog's body: from `anchor`'s left at its foot (the row's), as
/// wide as its widest entry and the box at least, the keyboard's `cursor`
/// lit in the accent with its words on it, another raised under the
/// pointer, each entry's words in the row's 14 points with their figures in
/// the monospace; whole entries only down to the page's area's foot, where
/// the cells cut it, and none at all, frame and all, with nothing to list.
/// A click on a row beneath it is the row's, as the cells' is. The entry a
/// click landed on: the cells' `CardSheetPick`.
fn hang(ui: &Ui, v: &View, face: &Face, parent: Id, anchor: Rect, cursor: usize) -> Option<usize> {
    let entries = list_entries(v.sheet, v.servers);
    let pal = face.pal;
    let over = face.full;
    let top = anchor.max.y;
    let n = entries.len().min(studio::hung_fits(top, over.max.y));
    if n == 0 {
        return None;
    }
    let runs = |entry: &str, ink: Color32| studio::figured(entry, &[], body(VALUE), mono(12.5), ink);
    let widest = entries.iter().map(|e| studio::runs_width(ui, &runs(e, pal.text))).fold(0.0, f32::max);
    let w = studio::hung_width(widest).max(anchor.width()).min(over.max.x - anchor.min.x).max(1.0);
    let frame = studio::hung_frame(pos2(anchor.min.x, top), w, n);
    // Its entries' ids are their list's (From's on Music, Into's on
    // Writing) and their numbers, as the rows' are their tab's.
    let entry_id = parent.with("entry").with(v.sheet.tab == SheetTab::Writing);
    let id_of = |i: usize| face.takes().then(|| entry_id.with(i));
    let words = |ui: &Ui, i: usize, room: Rect, on: bool| {
        let ink = if on { pal.on_accent } else { pal.text };
        studio::runs(ui, room.left_center(), &runs(&entries[i], ink), room.width(), Align2::LEFT_CENTER);
    };
    studio::hung_over(ui, parent.with("list"), parent, frame, Some(cursor), n, id_of, pal, words)
}

/// The sheet's egui face run headless, as the window's panel runs it: the
/// page's whole pass (egui_view.rs's `show`) over its area, on the page's
/// own fixtures — a card told into the transfer worker's fold, the
/// commands the page sent read back from its inbox. No worker runs, no card
/// or server is touched, the budget kept on this computer is never read (no
/// sheet here opens on a card with peers: they are added to the open
/// sheet).
#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc::Receiver;

    use egui::epaint::Shape;
    use egui::{Event, FullOutput, PointerButton};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::device::card::worker::{self, Fold, In, Link, Planned, fixtures as cards};

    fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    /// The page's area in points: in the boards' 1440 by 900 window, and at
    /// the GUI's floor (100×24 cells: 22 rows of 16 points).
    const BOARD: Vec2 = vec2(1440.0, 824.0);
    const FLOOR: Vec2 = vec2(800.0, 352.0);

    /// A context styled as the host's, its passes run over a page's area.
    struct Probe {
        ctx: egui::Context,
        size: Vec2,
    }

    impl Probe {
        fn new(size: Vec2) -> Probe {
            let ctx = egui::Context::default();
            crate::gui::window::panel::fonts(&ctx);
            studio::style_once(&ctx, &Pal::now());
            Probe { ctx, size }
        }

        fn input(&self, events: Vec<Event>) -> egui::RawInput {
            egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), events, ..Default::default() }
        }

        /// One pass with `events` and the input method composing
        /// `composition`: what egui drew, and the caret the page gave back.
        fn pass_with(&self, page: &mut Page, events: Vec<Event>, composition: &str) -> (FullOutput, Option<Pos2>) {
            let mut caret = None;
            let mut out = self.ctx.run_ui(self.input(events), |ui| caret = super::super::show(ui, page, composition, false).caret);
            // No GPU here to take the font atlas: let go of it on purpose, or
            // epaint's debug check fails the drop.
            out.textures_delta.clear();
            (out, caret)
        }

        fn pass(&self, page: &mut Page, events: Vec<Event>) -> (FullOutput, Option<Pos2>) {
            self.pass_with(page, events, "")
        }

        /// One pass with `events` that egui runs twice, as it does for a
        /// modal that took its body's height. The runs egui made.
        fn pass_twice(&self, page: &mut Page, events: Vec<Event>) -> u32 {
            let mut runs = 0;
            let mut out = self.ctx.run_ui(self.input(events), |ui| {
                runs += 1;
                let _ = super::super::show(ui, page, "", false);
                if runs == 1 {
                    ui.ctx().request_discard("the test's second run");
                }
            });
            out.textures_delta.clear();
            runs
        }

        /// Two passes (egui sizes a modal on the first), the second's.
        fn settle(&self, page: &mut Page) -> FullOutput {
            let _ = self.pass(page, Vec::new());
            self.pass(page, Vec::new()).0
        }

        /// A press and a release at `at`, a pass each after the move, as
        /// hands make them.
        fn click(&self, page: &mut Page, at: Pos2) {
            for events in [vec![Event::PointerMoved(at)], vec![button(at, true)], vec![button(at, false)]] {
                let _ = self.pass(page, events);
            }
        }

        /// A click on the middle of `words` as the page stands now.
        fn click_on(&self, page: &mut Page, words: &str) {
            let out = self.settle(page);
            self.click(page, find(&out, words).center());
        }
    }

    fn button(at: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    /// Every text a pass drew, whole, with where, in the order drawn: the
    /// page beneath first, the sheet over it, the list that hangs last.
    fn texts(out: &FullOutput) -> Vec<(String, Rect)> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(t) => Some((t.galley.text().to_string(), Rect::from_min_size(t.pos, t.galley.size()))),
                _ => None,
            })
            .collect()
    }

    fn drew(out: &FullOutput, words: &str) -> bool {
        texts(out).iter().any(|(t, _)| t == words)
    }

    fn assert_drew(out: &FullOutput, words: &[&str]) {
        for w in words {
            assert!(drew(out, w), "{w:?} drawn: {:?}", texts(out).into_iter().map(|(t, _)| t).collect::<Vec<_>>());
        }
    }

    /// Where the pass drew `words` last: the topmost of them (the sheet's
    /// "Car music" over the card's own beneath it).
    fn find(out: &FullOutput, words: &str) -> Rect {
        let found = texts(out).into_iter().rev().find(|(t, _)| t == words);
        found.map(|(_, r)| r).unwrap_or_else(|| panic!("{words:?} drawn: {:?}", texts(out).into_iter().map(|(t, _)| t).collect::<Vec<_>>()))
    }

    /// The faces `words` was set in, run by run, where it was drawn last.
    fn fonts_of(out: &FullOutput, words: &str) -> Vec<FontId> {
        let galley = out.shapes.iter().rev().find_map(|clipped| match &clipped.shape {
            Shape::Text(t) if t.galley.text() == words => Some(t.galley.clone()),
            _ => None,
        });
        galley.unwrap_or_else(|| panic!("{words:?} drawn")).job.sections.iter().map(|s| s.format.font_id.clone()).collect()
    }

    /// The rects a pass ringed in the accent: the keyboard's look, the list's
    /// frame, a focused field's edge.
    fn ringed(out: &FullOutput) -> Vec<Rect> {
        let accent = Pal::now().accent;
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Rect(r) if r.stroke.color == accent && r.stroke.width == 1.0 && r.fill == Color32::TRANSPARENT => Some(r.rect),
                _ => None,
            })
            .collect()
    }

    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }

    /// A page with the boards `tell` says and these cards in its readers,
    /// this computer's saved servers Home and Studio, on channels the test
    /// holds: the board worker's ends, the transfer worker's inbox, its
    /// fold.
    fn with_cards(tell: impl FnOnce(&Ends), list: Vec<worker::CardView>) -> (Page, Ends, Receiver<In>, Rc<RefCell<Fold>>) {
        let (mut page, ends) = Page::quiet();
        let (link, inbox) = Link::pair();
        let fold = Rc::new(RefCell::new(Fold::default()));
        {
            let mut f = fold.borrow_mut();
            f.watched = true;
            f.servers = vec![cards::home(), studio_server()];
            for card in list {
                f.apply(worker::Event::Card(Box::new(card)));
            }
        }
        page.attach(Transfer { link, fold: fold.clone() });
        tell(&ends);
        page.pump();
        (page, ends, inbox, fold)
    }

    /// The design's ready card alone, its Settings sheet open on Music.
    fn ready_sheet() -> (Page, Ends, Receiver<In>, Rc<RefCell<Fold>>) {
        let (mut page, ends, inbox, fold) = with_cards(|_| {}, vec![cards::ready("E:", now())]);
        page.act(Act::CardSettings);
        assert!(page.card.sheet.is_some(), "o's act, Settings…: the sheet");
        (page, ends, inbox, fold)
    }

    /// Studio, a second server saved on this computer.
    fn studio_server() -> ServerLink {
        ServerLink {
            id: "http://127.0.0.1:4000".into(),
            name: "Studio".into(),
            base: Ok("http://127.0.0.1:4000".into()),
            ..ServerLink::default()
        }
    }

    /// The board's peer, Lisbon, and its Fado, ticked, added to the open
    /// sheet as a card with that peer opens it.
    fn lisbon(page: &mut Page) {
        let sheet = page.card.sheet.as_mut().expect("the sheet");
        sheet.peers.push((40_417, "Lisbon".into(), vec!["Fado".into()]));
        sheet.want.peers.push((40_417, vec!["Fado".into()]));
    }

    fn sheet(page: &Page) -> &CardSheet {
        page.card.sheet.as_ref().expect("the sheet")
    }

    fn press(page: &mut Page, code: KeyCode) {
        let _ = <Page as Screen>::key(page, KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// The fit the sheet asked for answered with `next`, by number `seq`.
    fn answer(fold: &Rc<RefCell<Fold>>, seq: u64, next: Next) {
        fold.borrow_mut().cards[0].fit = Some((seq, next));
    }

    /// A plan whose fit leaves the card 2.6 GB to spare: under a twentieth
    /// of its 63.8, so tight.
    fn tight() -> Next {
        let fit = Fit { needed: 20_000_000_000, room: 22_600_000_000, tracks: 4_822 };
        Next::Plan(Box::new(Planned { fit: Some(fit), ..Planned::default() }))
    }

    #[test]
    fn the_music_tab_draws_its_rows_their_sizes_a_peers_header_the_fit_the_help_and_the_foot() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends, inbox, _fold) = ready_sheet();
        lisbon(&mut page);
        let (_, caret) = probe.pass(&mut page, Vec::new());
        let out = probe.settle(&mut page);
        let drive = page.cards()[0].drive();
        assert_drew(&out, &["Settings · Car music", "MUSIC", "WRITING", &format!("{drive} · 63.8 GB")]);
        assert_drew(&out, &["NAME", "Car music", "FROM", "Home", "Rock", "Jazz", "Audiobooks", "Live", "Fado"]);
        assert_drew(&out, &["27.9 GB · 2,904 tracks", "12.6 GB · 1,908 tracks", "From Lisbon · a peer of Home · 1,500 MB a day"]);
        assert_drew(&out, &["Fits · 43.2 of 63.8 GB after this sync · 20.6 GB to spare", &t!("dev.card.help_name"), "USE DEFAULTS", "APPLY"]);
        // Only the libraries already listed have sizes, and their figures
        // are the monospace's.
        let sizes = texts(&out).into_iter().filter(|(t, _)| t.contains(" GB · ") && t.ends_with(" tracks")).count();
        assert_eq!(sizes, 2, "Audiobooks and Live have none yet");
        assert!(fonts_of(&out, "27.9 GB · 2,904 tracks").contains(&mono(12.5)));
        // The sizes at the body's right on their library's line.
        let (rock, size) = (find(&out, "Rock"), find(&out, "27.9 GB · 2,904 tracks"));
        assert!((rock.center().y - size.center().y).abs() < 2.0 && size.min.x > rock.max.x + 200.0, "{rock:?} {size:?}");
        // The rows in the cells' order, the peer's header over its library.
        let order = ["NAME", "FROM", "Rock", "Jazz", "Audiobooks", "Live", "From Lisbon · a peer of Home · 1,500 MB a day", "Fado"];
        let ys: Vec<f32> = order.iter().map(|w| find(&out, w).center().y).collect();
        assert!(ys.windows(2).all(|p| p[0] < p[1]), "{ys:?}");
        // The fit's LED in the ok green, its words plain.
        let ok = Pal::now().ok;
        let leds = out.shapes.iter().filter(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == ok && r.rect.width() == 6.0));
        assert_eq!(leds.count(), 1, "the cells' ✓ as the LED");
        assert!(caret.is_none(), "no field has the keyboard: no caret");
        assert!(worker::sent(&inbox).is_empty(), "drawing asks the worker nothing");
    }

    #[test]
    fn a_click_on_a_row_takes_the_keyboard_there_and_does_what_space_does() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends, inbox, _fold) = ready_sheet();
        lisbon(&mut page);
        // A library: ticked, the keyboard on it, the fit asked again by its
        // number.
        probe.click_on(&mut page, "Audiobooks");
        assert_eq!(sheet(&page).row(), SRow::Library("Audiobooks".into()));
        assert!(sheet(&page).libraries().contains(&"Audiobooks".to_string()));
        let seq = sheet(&page).seq;
        assert!(matches!(worker::sent(&inbox).as_slice(), [worker::Cmd::Fit { seq: s, .. }] if *s == seq), "the fit asked once");
        // Once a click, however many runs egui makes of the pass.
        let at = find(&probe.settle(&mut page), "Rock").center();
        let _ = probe.pass(&mut page, vec![Event::PointerMoved(at)]);
        let _ = probe.pass(&mut page, vec![button(at, true)]);
        assert_eq!(probe.pass_twice(&mut page, vec![button(at, false)]), 2, "egui ran the release's pass twice");
        assert!(!sheet(&page).libraries().contains(&"Rock".to_string()), "Rock unticked, once");
        assert_eq!(worker::sent(&inbox).len(), 1);
        // A peer's library: unticked for that peer.
        probe.click_on(&mut page, "Fado");
        assert!(sheet(&page).want.peers.iter().all(|(_, libs)| libs.is_empty()), "{:?}", sheet(&page).want.peers);
        assert!(matches!(worker::sent(&inbox).as_slice(), [worker::Cmd::Fit { .. }]));
        // From: its list hangs, the cursor on its first entry, over the rows
        // beneath; a pick is the cells' CardSheetPick.
        probe.click_on(&mut page, "Home");
        assert_eq!(sheet(&page).list, Some(0));
        let out = probe.settle(&mut page);
        let (studio, rock) = (find(&out, "Studio"), find(&out, "Rock"));
        assert!(studio.max.y > find(&out, "FROM").max.y && studio.min.y < rock.max.y + 40.0, "under From, over the rows: {studio:?}");
        probe.click(&mut page, studio.center());
        assert_eq!(sheet(&page).want.server.as_deref(), Some("http://127.0.0.1:4000"));
        assert_eq!(sheet(&page).list, None);
        assert!(matches!(worker::sent(&inbox).as_slice(), [worker::Cmd::Fit { .. }]));
        let out = probe.settle(&mut page);
        assert_drew(&out, &["Studio", "From Lisbon · a peer of Studio · 1,500 MB a day"]);
        // Name: its field opens with the keyboard, the composition drawn in
        // it before the caret, the caret's foot in the box for the input
        // method; Esc closes it, and the caret goes with it.
        probe.click_on(&mut page, "Car music");
        assert!(sheet(&page).field.is_some(), "the cells' CardSheetRow(0): the field");
        let _ = probe.pass_with(&mut page, Vec::new(), "かな");
        let (out, caret) = probe.pass_with(&mut page, Vec::new(), "かな");
        assert_drew(&out, &["Car musicかな"]);
        let field = find(&out, "Car musicかな");
        let caret = caret.expect("the Name field has the keyboard");
        assert!(caret.x >= field.max.x - 1.0 && (caret.y - field.center().y).abs() < 20.0, "{caret:?} {field:?}");
        assert!(worker::sent(&inbox).is_empty(), "opening the field asks nothing");
        press(&mut page, KeyCode::Esc);
        let (_, caret) = probe.pass(&mut page, Vec::new());
        assert!(sheet(&page).field.is_none() && caret.is_none(), "no field, no caret");
    }

    #[test]
    fn the_writing_tab_draws_the_cells_rows_and_a_click_turns_each_as_space_does() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends, inbox, _fold) = ready_sheet();
        probe.click_on(&mut page, "WRITING");
        assert_eq!((sheet(&page).tab, sheet(&page).focus), (SheetTab::Writing, 0), "the cells' CardSheetTab(1)");
        let out = probe.settle(&mut page);
        let codec = codec_words(sheet(&page).want.codec, sheet(&page).want.bitrate);
        assert_drew(&out, &["CONVERT", "only what it can't play", "everything", "INTO", &codec, "REMOVALS", "follow Home", "never remove"]);
        assert_drew(&out, &["COVERS", "small covers for the player", "AUTO DJ", "build its index", "FOR", "me", "someone else"]);
        assert_drew(&out, &["PLAYER", "M5Stack Core2 · 0.14.0, as its device.txt says", &t!("dev.card.help_writing")]);
        assert!(!drew(&out, "PEER BUDGET"), "no peers: no budget");
        assert!(!drew(&out, "NAME") && !drew(&out, "Rock"), "Music's rows are gone");
        // A choice of a pair: the row's click turns it, as Space does.
        probe.click_on(&mut page, "never remove");
        assert_eq!(sheet(&page).want.removal, Removal::Never);
        assert_eq!(sheet(&page).row(), SRow::Removals);
        assert!(matches!(worker::sent(&inbox).as_slice(), [worker::Cmd::Fit { .. }]));
        let covers = sheet(&page).want.covers;
        probe.click_on(&mut page, "small covers for the player");
        assert_eq!(sheet(&page).want.covers, !covers);
        let _ = worker::sent(&inbox);
        // Player is no control, but its row takes the keyboard, as the cells'
        // click does, and asks nothing.
        probe.click_on(&mut page, "M5Stack Core2 · 0.14.0, as its device.txt says");
        assert_eq!(sheet(&page).row(), SRow::Player);
        assert!(worker::sent(&inbox).is_empty());
        // Into hangs the eight codecs; a pick sets the codec.
        probe.click_on(&mut page, &codec);
        assert_eq!(sheet(&page).list, Some(0));
        let out = probe.settle(&mut page);
        let all = list_entries(sheet(&page), &page.servers());
        assert_eq!(all.len(), 8);
        assert_drew(&out, &all.iter().map(String::as_str).collect::<Vec<_>>());
        probe.click(&mut page, find(&out, "MP3 128k").center());
        assert_eq!(codec_words(sheet(&page).want.codec, sheet(&page).want.bitrate), "MP3 128k");
        assert!(matches!(worker::sent(&inbox).as_slice(), [worker::Cmd::Fit { .. }]));
        // The open tab's click too: back to its first row.
        probe.click_on(&mut page, "WRITING");
        assert_eq!((sheet(&page).tab, sheet(&page).focus), (SheetTab::Writing, 0));
        // With a peer, the budget: a click is 250 MB more, as Space.
        lisbon(&mut page);
        let out = probe.settle(&mut page);
        assert_drew(&out, &["PEER BUDGET", "1,500 MB a day per peer"]);
        probe.click_on(&mut page, "1,500 MB a day per peer");
        assert_eq!(sheet(&page).want.budget_mb, Some(1_750));
        assert_drew(&probe.settle(&mut page), &["1,750 MB a day per peer"]);
    }

    #[test]
    fn the_foot_asks_for_use_defaults_and_apply_the_close_for_close_and_set_up_for_its_own() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends, inbox, _fold) = ready_sheet();
        page.act(Act::CardSheetTab(1));
        page.act(Act::CardSheetRow(2));
        assert_eq!(sheet(&page).want.removal, Removal::Never);
        let _ = worker::sent(&inbox);
        // Use defaults: the Writing choices back, the fit asked again.
        probe.click_on(&mut page, "USE DEFAULTS");
        assert_eq!(sheet(&page).want.removal, Removal::Mirror, "the cells' CardDefaults");
        assert!(matches!(worker::sent(&inbox).as_slice(), [worker::Cmd::Fit { .. }]));
        // Apply: the settings written, the sheet closed, nothing synced.
        page.act(Act::CardSheetRow(2));
        let _ = worker::sent(&inbox);
        probe.click_on(&mut page, "APPLY");
        let applied = worker::sent(&inbox);
        assert!(matches!(applied.as_slice(), [worker::Cmd::Apply { want, .. }] if want.removal == Removal::Never), "{applied:?}");
        assert!(page.card.sheet.is_none(), "the cells' CardApply");
        // The close: nothing written.
        page.act(Act::CardSettings);
        let _ = probe.settle(&mut page);
        let close = probe.ctx.read_response(Id::new("mp3-card-sheet").with("close")).expect("the close drawn").rect.center();
        probe.click(&mut page, close);
        assert!(page.card.sheet.is_none() && worker::sent(&inbox).is_empty(), "the cells' CardSheetClose");
        // A new card's Set-up sheet: Music alone, Cancel and Set up.
        let (mut page, _ends, inbox, _fold) = with_cards(|_| {}, vec![cards::new_card("F:")]);
        page.act(Act::Primary);
        let out = probe.settle(&mut page);
        assert_drew(&out, &["Set up F:", "NAME", "FROM", "Home", "CANCEL", "SET UP"]);
        assert!(!drew(&out, "MUSIC") && !drew(&out, "WRITING"), "no tabs setting up");
        probe.click_on(&mut page, "CANCEL");
        assert!(page.card.sheet.is_none() && worker::sent(&inbox).is_empty(), "Cancel is the cells' CardSheetClose");
        page.act(Act::Primary);
        probe.click_on(&mut page, "SET UP");
        assert!(matches!(worker::sent(&inbox).as_slice(), [worker::Cmd::SetUp { .. }]), "the cells' CardApply: set up");
    }

    #[test]
    fn the_close_wears_the_cells_tooltip_its_key_named_only_as_their_surface_names_keys() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends, _inbox, _fold) = ready_sheet();
        let _ = probe.settle(&mut page);
        let close = probe.ctx.read_response(Id::new("mp3-card-sheet").with("close")).expect("the close drawn").rect.center();
        let _ = probe.pass(&mut page, vec![Event::PointerMoved(close)]);
        let mut out = probe.pass(&mut page, Vec::new()).0;
        for _ in 0..10 {
            out = probe.pass(&mut page, Vec::new()).0;
        }
        // The probe's host names no keys: the cells' tip_keyed drops the
        // " — Esc" so.
        let words = t!("path_modal.tip_close").to_string();
        let bare = words.rsplit_once(" — ").map_or(words.as_str(), |(w, _)| w);
        assert!(drew(&out, bare) && !drew(&out, &words), "{:?}", texts(&out).into_iter().map(|(t, _)| t).collect::<Vec<_>>());
        assert!(page.card.sheet.is_some(), "a hover closes nothing");
    }

    #[test]
    fn a_long_library_name_gives_way_to_its_size_cut_short_of_it_as_the_board_cuts_it() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends, _inbox, _fold) = ready_sheet();
        let long = "The Complete Tuesday Night Sessions at the Village Vanguard, Every Set Ever Taped";
        page.card.sheet.as_mut().expect("the sheet").offered[0].0 = long.to_string();
        let out = probe.settle(&mut page);
        let (name, size) = (find(&out, long), find(&out, "27.9 GB · 2,904 tracks"));
        let body_right = find(&out, "12.6 GB · 1,908 tracks").max.x;
        assert!((size.max.x - body_right).abs() < 1.0, "the size whole at the row's right: {size:?}");
        assert!(name.max.x <= size.min.x - SIZE_GAP + 1.0, "the name cut short of it: {name:?} {size:?}");
        assert!((name.center().y - size.center().y).abs() < 2.0);
    }

    #[test]
    fn a_press_let_go_after_a_key_turned_the_tab_clicks_nothing_on_the_other_tab() {
        let _en = english();
        let probe = Probe::new(BOARD);
        // egui gives a click to the id a press was taken on, wherever the
        // pointer is let go; the cells act at the press.
        for passes_between in [0, 1] {
            let (mut page, _ends, inbox, _fold) = ready_sheet();
            // Rock is Music's third row; Writing's third, Removals, stands
            // where it was.
            let rock = find(&probe.settle(&mut page), "Rock").center();
            let _ = probe.pass(&mut page, vec![Event::PointerMoved(rock)]);
            let _ = probe.pass(&mut page, vec![button(rock, true)]);
            press(&mut page, KeyCode::Right);
            assert_eq!(sheet(&page).tab, SheetTab::Writing);
            for _ in 0..passes_between {
                let _ = probe.pass(&mut page, Vec::new());
            }
            let want = sheet(&page).want.clone();
            let _ = probe.pass(&mut page, vec![button(rock, false)]);
            assert_eq!(sheet(&page).want, want, "nothing on Writing turned, {passes_between} passes between");
            assert_eq!(sheet(&page).focus, 0, "the keyboard where Right put it");
            assert!(worker::sent(&inbox).is_empty());
            // An entry of From's list, let go once the keys have Into's
            // hanging: no codec picked.
            page.act(Act::CardSheetTab(0));
            page.act(Act::CardSheetRow(1));
            let studio = find(&probe.settle(&mut page), "Studio").center();
            let _ = probe.pass(&mut page, vec![Event::PointerMoved(studio)]);
            let _ = probe.pass(&mut page, vec![button(studio, true)]);
            for code in [KeyCode::Esc, KeyCode::Right, KeyCode::Down, KeyCode::Char(' ')] {
                press(&mut page, code);
            }
            assert_eq!((sheet(&page).row(), sheet(&page).list), (SRow::Into, Some(0)));
            for _ in 0..passes_between {
                let _ = probe.pass(&mut page, Vec::new());
            }
            let want = sheet(&page).want.clone();
            let _ = probe.pass(&mut page, vec![button(studio, false)]);
            assert_eq!(sheet(&page).want, want, "no codec picked, {passes_between} passes between");
            assert_eq!(sheet(&page).list, Some(0), "Into's list still hangs");
            assert!(worker::sent(&inbox).is_empty());
        }
    }

    /// A wheel's turn of `dy` points over `at` (down the body for a
    /// negative one), and the passes egui eases the scroll over.
    fn wheel(probe: &Probe, page: &mut Page, at: Pos2, dy: f32) {
        let turn = Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, dy),
            phase: egui::TouchPhase::Move,
            modifiers: Default::default(),
        };
        let _ = probe.pass(page, vec![Event::PointerMoved(at), turn]);
        for _ in 0..40 {
            let _ = probe.pass(page, Vec::new());
        }
    }

    #[test]
    fn a_list_hangs_only_from_its_box_in_the_bodys_view_and_its_opening_brings_the_box_in() {
        let _en = english();
        let probe = Probe::new(FLOOR);
        let (mut page, _ends, _inbox, _fold) = ready_sheet();
        // More libraries than the floor's body holds: it scrolls.
        let more = page.card.sheet.as_mut().expect("the sheet");
        more.offered.extend((1..=6).map(|k| (format!("Shelf {k}"), None)));
        press(&mut page, KeyCode::Down);
        assert_eq!(sheet(&page).row(), SRow::From);
        let title = find(&probe.settle(&mut page), "Settings · Car music");
        // The pointer on the body's right, clear of where the list hangs.
        let body = pos2(FLOOR.x / 2.0 + 220.0, FLOOR.y / 2.0 + 20.0);
        // The wheel takes the body to its foot, From out of its view above.
        wheel(&probe, &mut page, body, -2000.0);
        assert!(find(&probe.settle(&mut page), "FROM").max.y < title.max.y, "From scrolled away");
        // Space opens its list: the box comes back into view and the list
        // hangs under it.
        press(&mut page, KeyCode::Char(' '));
        assert_eq!(sheet(&page).list, Some(0));
        for _ in 0..40 {
            let _ = probe.pass(&mut page, Vec::new());
        }
        let out = probe.settle(&mut page);
        let (back, studio) = (find(&out, "FROM"), find(&out, "Studio"));
        // The body's view starts 20 points under the title's row.
        assert!(back.min.y > title.max.y + 20.0, "From in view again: {back:?} under {title:?}");
        assert!(studio.min.y > back.max.y, "the list under it: {studio:?}");
        // The wheel takes the box away again: no list hangs over the title,
        // though it is still the keyboard's.
        wheel(&probe, &mut page, body, -2000.0);
        let out = probe.settle(&mut page);
        assert!(find(&out, "FROM").max.y < title.max.y, "From scrolled away");
        assert!(!drew(&out, "Studio"), "{:?}", texts(&out).into_iter().map(|(t, _)| t).collect::<Vec<_>>());
        assert_eq!(sheet(&page).list, Some(0));
        // And back with it.
        wheel(&probe, &mut page, body, 2000.0);
        assert!(drew(&probe.settle(&mut page), "Studio"));
    }

    #[test]
    fn the_keyboards_row_wears_the_ring_its_words_the_semibold_and_egui_keeps_no_focus_of_its_own() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends, _inbox, _fold) = ready_sheet();
        for _ in 0..3 {
            press(&mut page, KeyCode::Down);
        }
        assert_eq!(sheet(&page).row(), SRow::Library("Jazz".into()));
        let out = probe.settle(&mut page);
        assert_eq!(fonts_of(&out, "Jazz"), [strong(14.0)], "the cells' accent bold");
        assert_eq!(fonts_of(&out, "Rock"), [body(14.0)]);
        let rings = ringed(&out);
        assert!(rings.iter().any(|r| r.contains(find(&out, "Jazz").center()) && r.height() == TICK_H), "{rings:?}");
        assert!(!rings.iter().any(|r| r.contains(find(&out, "Rock").center())), "{rings:?}");
        let help = t!("dev.card.help_lib_short", lib = "Jazz", server = "Home", n = "1,908").to_string();
        assert_drew(&out, &[&help]);
        // A click leaves egui's focus where it was: nowhere. The keyboard is
        // the page's.
        probe.click_on(&mut page, "Rock");
        assert_eq!(sheet(&page).row(), SRow::Library("Rock".into()));
        assert!(probe.ctx.memory(|m| m.focused()).is_none());
        // While a list hangs, no row wears the look; the foot's button with
        // the keyboard wears the ring just outside it.
        page.card.sheet.as_mut().expect("the sheet").focus = 1;
        press(&mut page, KeyCode::Enter);
        assert!(sheet(&page).list.is_some());
        let out = probe.settle(&mut page);
        assert!(!ringed(&out).iter().any(|r| r.height() == TICK_H || r.height() == FIELD_H), "{:?}", ringed(&out));
        press(&mut page, KeyCode::Esc);
        page.card.sheet.as_mut().expect("the sheet").focus = 99;
        press(&mut page, KeyCode::Left);
        assert!(!sheet(&page).on_apply);
        let out = probe.settle(&mut page);
        let defaults = find(&out, "USE DEFAULTS");
        assert!(ringed(&out).iter().any(|r| r.contains(defaults.center()) && r.height() > studio::FORM_BUTTON_H), "{:?}", ringed(&out));
    }

    #[test]
    fn the_fit_is_the_sheets_own_answer_by_number_and_the_scan_while_it_is_asked() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends, _inbox, fold) = ready_sheet();
        page.act(Act::CardSheetRow(4));
        let seq = sheet(&page).seq;
        let last_look = "Fits · 43.2 of 63.8 GB after this sync · 20.6 GB to spare";
        answer(&fold, seq, Next::Asking);
        let out = probe.settle(&mut page);
        assert_drew(&out, &["working out what changes…"]);
        assert!(!drew(&out, last_look));
        // An older answer is never drawn: the card's last look stands, as the
        // cells' does.
        answer(&fold, seq - 1, tight());
        let out = probe.settle(&mut page);
        assert!(drew(&out, last_look) && !drew(&out, "working out what changes…"));
        // Its own: tight, the LED and the words gold, the dim line under.
        answer(&fold, seq, tight());
        let out = probe.settle(&mut page);
        let words = "Tight · 61.2 of 63.8 GB after this sync · 2.6 GB to spare";
        assert_drew(&out, &[words, &t!("dev.card.fit_tight_2")]);
        let gold = Pal::now().gold;
        assert!(out.shapes.iter().any(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == gold && r.rect.width() == 6.0)), "the LED gold");
        assert!(find(&out, &t!("dev.card.fit_tight_2")).min.y > find(&out, words).max.y);
        // Unasked: no line at all.
        answer(&fold, seq, Next::Unasked);
        let out = probe.settle(&mut page);
        assert!(!texts(&out).iter().any(|(t, _)| t.contains("after this sync")));
    }

    #[test]
    fn the_page_beneath_is_inert_and_the_sheets_top_is_held_whatever_its_tab() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, ends, inbox, _fold) = with_cards(Ends::update_available, vec![cards::ready("E:", now())]);
        page.act(Act::Card(0));
        page.act(Act::CardSettings);
        let out = probe.settle(&mut page);
        let title = find(&out, "Settings · Car music");
        // The tallest state's window centred: the board's 102 points down.
        let top = BOARD.y / 2.0 - (88.0 + HELD_BODY) / 2.0;
        assert!((title.center().y - (top + 24.0 + 10.0)).abs() < 2.0, "{title:?} under {top}");
        // The board's tab beneath, under the scrim: no click reaches it.
        probe.click(&mut page, find(&out, "COM3").center());
        assert!(page.card_open.is_some() && page.card.sheet.is_some(), "the page under the sheet is inert");
        assert!(ends.sent().is_empty() && worker::sent(&inbox).is_empty());
        // Writing, and Music with a peer: the same top.
        page.act(Act::CardSheetTab(1));
        assert_eq!(find(&probe.settle(&mut page), "Settings · Car music").min.y, title.min.y);
        page.act(Act::CardSheetTab(0));
        lisbon(&mut page);
        assert_eq!(find(&probe.settle(&mut page), "Settings · Car music").min.y, title.min.y);
    }

    #[test]
    fn at_the_guis_floor_the_help_gives_way_and_the_keyboards_row_is_brought_into_view() {
        let _en = english();
        let probe = Probe::new(FLOOR);
        let (mut page, _ends, _inbox, _fold) = ready_sheet();
        let out = probe.settle(&mut page);
        assert_drew(&out, &["Settings · Car music", "NAME", "Car music"]);
        assert!(!drew(&out, &t!("dev.card.help_name")), "the help goes first, as the cells' does");
        // Into's list hangs down to the area's foot, whole entries only:
        // from the body's top, the second row's foot leaves room for five.
        page.act(Act::CardSheetTab(1));
        page.card.sheet.as_mut().expect("the sheet").focus = 1;
        press(&mut page, KeyCode::Enter);
        let out = probe.settle(&mut page);
        let all = list_entries(sheet(&page), &page.servers());
        let shown: Vec<Rect> = all.iter().filter(|e| drew(&out, e)).map(|e| find(&out, e)).collect();
        assert!(!shown.is_empty() && shown.len() < all.len(), "{} of {}", shown.len(), all.len());
        assert!(shown.iter().all(|r| r.max.y <= FLOOR.y), "{shown:?}");
        assert!(!drew(&out, "MP3 320k"), "the last ones past the foot");
        // The keyboard on the buttons: the foot brought into the body's view.
        press(&mut page, KeyCode::Esc);
        page.card.sheet.as_mut().expect("the sheet").focus = 99;
        press(&mut page, KeyCode::Down);
        let out = probe.settle(&mut page);
        let apply = find(&out, "APPLY");
        assert!(apply.min.y > 0.0 && apply.max.y < FLOOR.y - 16.0, "{apply:?}");
    }
}
