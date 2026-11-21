//! The Libraries room drawn by egui, in the desktop window's panel: the
//! same [`Room`] the cells view draws (the parent module: its state, its
//! worker, its keys and its words), only the drawing is new, in pixels, on
//! the Studio kit (src/gui/window/studio.rs), after the Claude Design
//! canvas of the Admin screen (its Main and LibrariesName boards).
//!
//! Every click is one of the room's own [`Act`]s, the one the cells view
//! registers for the same control, applied through [`Room::act`] in the
//! pass. Keys never reach egui: the room's keymap still moves the cursor,
//! edits the two fields (the typed path and the name) and answers the
//! modals, and egui draws whatever the room holds. What the view keeps
//! between passes (where a list stands, the window over a field's line)
//! it keeps in egui's memory under its own ids; the room holds nothing for
//! it.
//!
//! Built only with the window: the terminal build has no panel to draw in.

use egui::{Color32, FontId, Id, Pos2, Rect, Ui, pos2, vec2};

use super::*;
use crate::admin::EguiShown;
use crate::gui::window::studio::{
    self, BROWSER_W, Col, Dialog, FIELD_H, FOLDER_ROW_H, Glyph, Icon, Pal, Para, Table, add_card, body, checkbox,
    cut_start, field_beside, field_box, folder_browser, folder_row, foot, gate, icon_button, ink, label, line, mono,
    path_cell, room_rects, strong,
};

/// The table's NAME and SYMLINKS columns (FOLDER takes the rest), and the
/// remove column beyond the selection area with the gap before it.
const NAME_W: f32 = 220.0;
const SYM_W: f32 = 96.0;
const REMOVE_W: f32 = 36.0;
const TRAIL: f32 = REMOVE_W + 8.0;
/// The table's rows.
const ROW_H: f32 = 40.0;
/// The Name window, the canvas's.
const NARROW: f32 = 560.0;
/// The name field's box, the canvas's.
const NAME_BOX_W: f32 = 264.0;
/// The most suggestions the typed path shows, as the cells'.
const SUGGEST_ROWS: usize = 6;

/// One pass of the room over the whole of `ui`, the room's area less its
/// first row, with `composition` the input method's uncommitted text for
/// the focused field. What it returns is the focused field's caret and a
/// click that ended the room (none can: leaving is a key's).
pub(crate) fn show(ui: &mut Ui, room: &mut Room, composition: &str) -> EguiShown {
    let pal = Pal::now();
    let full = ui.max_rect();
    let (body_rect, note_rect) = room_rects(full);

    let mut acts: Vec<Act> = Vec::new();
    let mut ui_body = ui.new_child(
        egui::UiBuilder::new().id_salt("libraries-body").max_rect(body_rect).layout(egui::Layout::top_down(egui::Align::Min)),
    );
    ui_body.set_clip_rect(body_rect.expand(1.0).intersect(ui.clip_rect()));
    libraries(&mut ui_body, room, &pal, &mut acts);
    studio::note_line(ui, note_rect, room.busy.as_deref(), room.note.as_ref(), &pal);

    // A modal makes the room beneath inert: what was clicked under it this
    // pass is dropped, as the cells view drops what the room registered.
    if !matches!(room.modal, Modal::None) {
        acts.clear();
    }
    let mut caret = None;
    match &room.modal {
        Modal::None => {}
        Modal::Browser(browse) => browser(ui, full, browse, &pal, &mut acts),
        Modal::PathEntry(draft) => caret = path_entry(ui, full, draft, composition, &pal, &mut acts),
        Modal::Name(draft) => caret = name(ui, full, draft, composition, &pal, &mut acts),
        Modal::Remove(i) => remove(ui, full, room, *i, &pal, &mut acts),
    }

    let mut outcome = None;
    for act in acts {
        outcome = outcome.or(room.act(act));
    }
    // A click that closed the field's modal took the keyboard with it.
    if !matches!(room.modal, Modal::Name(_) | Modal::PathEntry(_)) {
        caret = None;
    }
    EguiShown { caret, outcome }
}

// ── The room ──────────────────────────────────────────────────────────────

/// The add card, then the table: NAME · FOLDER · SYMLINKS, the remove
/// button in a column of its own past the selection area. Rows take no
/// click as a whole, only the tick box and the remove; the cursor's ring
/// is the keys'. A fresh add's row is brought into view, not selected, as
/// the cells view takes the same note when it draws; an empty list keeps
/// its headings and says so where the first row would be.
fn libraries(ui: &mut Ui, room: &mut Room, pal: &Pal, acts: &mut Vec<Act>) {
    if add_card(ui, Id::new("libraries-add"), &t!("folders.add_card"), Glyph::Folder, pal).clicked() {
        acts.push(Act::Add);
    }
    ui.add_space(studio::ROOM_GAP);

    let cols = [
        Col::left(t!("folders.col_name"), NAME_W),
        Col::left(t!("folders.col_folder"), 0.0),
        Col::center(t!("admin.col_symlinks"), SYM_W),
    ];
    let nothing = t!("folders.nothing_yet");
    let libs = &room.libs;
    let table = Table::new(Id::new("libraries-table"), &cols, libs.len())
        .cursor(room.sel)
        .row_height(ROW_H)
        .trail(TRAIL)
        .inert()
        .reveal(room.reveal.take())
        .empty(&nothing);
    table.show(ui, pal, |ui, i, row| {
        let lib = &libs[i];
        // The vpath, plain: the server has no rename, so nothing here
        // offers one. A long one is cut at its column with no ellipsis, as
        // the canvas and the cells cut it.
        let font = if row.selected { strong(14.0) } else { body(14.0) };
        clipped(ui, row.cell(0), &lib.name, font, ink(row, pal.text, pal));
        // The folder keeps its leaf; the whole path rides a tip when cut.
        path_cell(ui, row, 1, &lib.root, mono(12.5), ink(row, pal.soft, pal), Id::new("libraries-root").with(i));
        if checkbox(ui, Id::new("libraries-symlinks").with(i), row.cell(2).center(), lib.follow_symlinks, pal).clicked() {
            acts.push(Act::ToggleSymlinks(i));
        }
        let at = pos2(row.trail.max.x - REMOVE_W / 2.0, row.rect.center().y);
        if icon_button(ui, Id::new("libraries-remove").with(i), at, Icon::Remove, pal.dim, pal).clicked() {
            acts.push(Act::RemoveAt(i));
        }
    });
}

// ── Modals ────────────────────────────────────────────────────────────────

/// The server's folders, the kit's browser (Backups' too): the listed
/// path, its folders (a click moves the cursor, as in the cells), and the
/// four acts in the cells' order from the left — up, open, the add (lit)
/// and close.
fn browser(ui: &Ui, over: Rect, browse: &Browse, pal: &Pal, acts: &mut Vec<Act>) {
    let id = Id::new("libraries-browse");
    let shown = Dialog::new(id, over, BROWSER_W, &t!("browse.title")).show(ui, pal, |ui| {
        let picked = folder_browser(ui, id.with("list"), &browse.path, &browse.dirs, browse.sel, &t!("browse.empty"), pal);
        let act = picked.map(Act::BrowseRow);
        let (up, open, add, close) = (t!("browse.up"), t!("browse.open"), t!("browse.add"), t!("browse.close"));
        let start = [(up.as_ref(), false), (open.as_ref(), false), (add.as_ref(), true), (close.as_ref(), false)];
        let meant = [Act::BrowseUp, Act::BrowseEnter, Act::BrowseAdd, Act::BrowseCancel];
        foot(ui, id.with("foot"), &start, &[], None, pal).map(|k| meant[k].clone()).or(act)
    });
    if shown.closed {
        acts.push(Act::BrowseCancel);
    }
    acts.extend(shown.inner);
}

/// The typed path: the field (the room's, its value and caret, never
/// edited here) and under it the server's suggestions, a click taking
/// one, or the gold reason the directory could not be listed. The window
/// holds its top where it stands with the most suggestions it shows, and
/// the list grows down from the field as the suggestions come and go, as
/// the cells anchor theirs. As wide as the browser, the other way to a
/// folder.
fn path_entry(ui: &Ui, over: Rect, draft: &PathDraft, composition: &str, pal: &Pal, acts: &mut Vec<Act>) -> Option<Pos2> {
    let id = Id::new("libraries-path");
    let most = FIELD_H + 12.0 + SUGGEST_ROWS as f32 * FOLDER_ROW_H;
    let shown = Dialog::new(id, over, BROWSER_W, &t!("path_modal.title")).hold(most).show(ui, pal, |ui| {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, FIELD_H), egui::Sense::hover());
        let input = studio::Input::of(&draft.text).focused(true, composition).mono();
        let typed = field_box(ui, id.with("field"), rect, &input, pal);
        let mut picked = None;
        if let Some(err) = &draft.error {
            ui.add_space(12.0);
            line(ui, FOLDER_ROW_H, err, body(13.0), pal.gold);
        } else if draft.suggestion_count() > 0 {
            ui.add_space(12.0);
            let rows = draft.suggestion_count().min(SUGGEST_ROWS);
            let h = (rows as f32 * FOLDER_ROW_H).min(ui.available_height()).max(FOLDER_ROW_H);
            // The suggestions are the listing filtered: walked once a pass
            // for the rows on screen, never collected whole (a listing can
            // run to thousands).
            let (mut from, mut window): (usize, Vec<&str>) = (0, Vec::new());
            let list = Table::new(id.with("list"), &[], draft.suggestion_count())
                .headless()
                .row_height(FOLDER_ROW_H)
                .height(h)
                .cursor(draft.sel)
                .content(Id::new(draft.text.value()));
            let shown = list.show(ui, pal, |ui, i, row| {
                if i < from || i >= from + window.len() {
                    from = i;
                    window = draft.suggestions().skip(i).take(4 * SUGGEST_ROWS).collect();
                }
                // Dim, the pointer's brighter, as the cells draw them.
                if let Some(entry) = window.get(i - from) {
                    folder_row(ui, row.rect, entry, row.selected, if row.hovered { pal.text } else { pal.dim }, pal);
                }
            });
            picked = shown.row;
        }
        (typed.caret, picked)
    });
    if shown.closed {
        acts.push(Act::PathCancel);
    }
    let (caret, picked) = shown.inner;
    acts.extend(picked.map(Act::PathSuggest));
    caret
}

/// The Name window every way of adding ends in: the folder, the name field
/// with the charset beside it, the permanence in gold, a refusal's words,
/// and the add.
fn name(ui: &Ui, over: Rect, draft: &NameDraft, composition: &str, pal: &Pal, acts: &mut Vec<Act>) -> Option<Pos2> {
    let id = Id::new("libraries-name");
    let shown = Dialog::new(id, over, NARROW, &t!("admin.name_title")).show(ui, pal, |ui| {
        let w = ui.available_width();
        ui.add_space(2.0);
        label(ui, &t!("admin.name_folder"), pal);
        ui.add_space(6.0);
        let (folder, _) = cut_start(ui, &draft.directory, mono(12.5), w);
        line(ui, 18.0, &folder, mono(12.5), pal.text);
        ui.add_space(18.0);

        let input = studio::Input::of(&draft.name).focused(true, composition);
        let typed = field_beside(ui, id.with("field"), &t!("admin.name_label"), &input, NAME_BOX_W, &t!("admin.name_charset"), pal);
        ui.add_space(18.0);

        // One paragraph, as the canvas sets it: the cells' two lines, the
        // second indented under the first's words in their grid, joined and
        // hung beside the dash.
        let permanent = format!("{} {}", t!("admin.name_permanent_1").trim(), t!("admin.name_permanent_2").trim());
        listed(&permanent, body(13.0), pal.gold).line(1.45).show(ui);
        ui.add_space(12.0);
        // The refusal's line is kept empty until there is one, so the
        // window does not grow under the hand that pressed Add.
        line(ui, 18.0, draft.error.as_deref().unwrap_or(""), body(13.0), pal.gold);
        ui.add_space(12.0);
        let add = t!("admin.name_add");
        let add = foot(ui, id.with("foot"), &[], &[(add.as_ref(), true)], None, pal).is_some();
        (typed.caret, add)
    });
    if shown.closed {
        acts.push(Act::NameCancel);
    }
    let (caret, add) = shown.inner;
    if add {
        acts.push(Act::NameAdd);
    }
    caret
}

/// The warning gate before a remove, the kit's: what stays and what goes,
/// then Keep (the safe choice, lit) and Remove.
fn remove(ui: &Ui, over: Rect, room: &Room, i: usize, pal: &Pal, acts: &mut Vec<Act>) {
    let name = room.libs.get(i).map(|l| l.name.clone()).unwrap_or_default();
    let title = t!("admin.remove_title", name = name);
    let sentences = vec![
        listed(&t!("admin.remove_files_stay"), body(14.0), pal.ok),
        listed(&t!("admin.remove_users", name = name), body(14.0), pal.gold),
        listed(&t!("admin.remove_tracks"), body(14.0), pal.gold),
    ];
    let (keep, go) = (t!("admin.remove_keep"), t!("admin.remove_confirm"));
    match gate(ui, Id::new("libraries-gate"), over, &title, sentences, [&keep, &go], pal) {
        Some(true) => acts.push(Act::RemoveConfirm),
        Some(false) => acts.push(Act::RemoveCancel),
        None => {}
    }
}

// ── Pieces ────────────────────────────────────────────────────────────────

/// A sentence of the room's in `font` and `color`, wrapped to the column;
/// one the cells lead with a dash ("− …") hangs its lines beside the dash,
/// as a list item does, rather than under it.
fn listed(s: &str, font: FontId, color: Color32) -> Para {
    match s.strip_prefix('−') {
        Some(rest) => Para::new(rest.trim_start(), font, color).lead("−"),
        None => Para::new(s, font, color),
    }
}

/// `s` in one line from `rect`'s left, its middle on `rect`'s, cut plainly
/// at the rect's right edge: no ellipsis, as the canvas clips a long name.
fn clipped(ui: &Ui, rect: Rect, s: &str, font: FontId, color: Color32) {
    let galley = ui.painter().layout_no_wrap(s.to_string(), font, color);
    let at = pos2(rect.min.x, rect.center().y - galley.size().y / 2.0);
    ui.painter().with_clip_rect(rect.intersect(ui.clip_rect())).galley(at, galley, color);
}

#[cfg(test)]
mod tests {
    use egui::epaint::Shape;
    use egui::Vec2;

    use super::super::tests::{english, key, room, seeded, type_text};
    use super::*;
    use crate::api::types::DirEntry;

    /// The room's region in the default 100×30 window, in points.
    const SIZE: Vec2 = vec2(800.0, 420.0);

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        ctx
    }

    /// One pass of the room with `events`, the input method composing
    /// `composition`: what egui drew, and what the room told its host.
    fn pass(ctx: &egui::Context, room: &mut Room, events: Vec<egui::Event>, composition: &str) -> (egui::FullOutput, EguiShown) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)), events, ..Default::default() };
        let mut shown = None;
        let mut out = ctx.run_ui(input, |ui| {
            // The host styles egui and paints the ground first.
            let _ = studio::begin(ui);
            let this = show(ui, room, composition);
            // A discarded pass's click stands, as the host keeps it.
            let outcome = shown.as_ref().and_then(|s: &EguiShown| s.outcome).or(this.outcome);
            shown = Some(EguiShown { caret: this.caret, outcome });
        });
        // No GPU here to take the font atlas: let go of it on purpose, or
        // epaint's debug check fails the drop.
        out.textures_delta.clear();
        (out, shown.expect("the pass ran"))
    }

    /// Two passes (egui sizes some things on the first), the second's.
    fn settle(ctx: &egui::Context, room: &mut Room) -> (egui::FullOutput, EguiShown) {
        let _ = pass(ctx, room, Vec::new(), "");
        pass(ctx, room, Vec::new(), "")
    }

    /// A press and a release at `at`, a pass each after the move, as hands
    /// would make them: what the room told its host, the outcome over the
    /// three and the caret of the release's pass, the one the click acted in.
    fn press(ctx: &egui::Context, room: &mut Room, at: Pos2) -> EguiShown {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let mut told = EguiShown { caret: None, outcome: None };
        for events in [vec![egui::Event::PointerMoved(at)], vec![button(true)], vec![button(false)]] {
            let (_, shown) = pass(ctx, room, events, "");
            told = EguiShown { caret: shown.caret, outcome: told.outcome.or(shown.outcome) };
        }
        told
    }

    /// [`press`] on the middle of what `id` drew last pass.
    fn click(ctx: &egui::Context, room: &mut Room, id: Id) -> EguiShown {
        let at = ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} was drawn")).rect.center();
        press(ctx, room, at)
    }

    /// The wheel turned over `at`, `dy` points of a trackpad's swipe (a
    /// positive one moves the content down, toward its top), a pass for the
    /// pointer's move, one for the swipe and one for its end.
    fn wheel(ctx: &egui::Context, room: &mut Room, at: Pos2, dy: f32) {
        let turn = |phase, dy| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, dy),
            phase,
            modifiers: egui::Modifiers::default(),
        };
        let _ = pass(ctx, room, vec![egui::Event::PointerMoved(at)], "");
        let _ = pass(ctx, room, vec![turn(egui::TouchPhase::Start, 0.0), turn(egui::TouchPhase::Move, dy)], "");
        let _ = pass(ctx, room, vec![turn(egui::TouchPhase::End, 0.0)], "");
    }

    /// Row or button `k` of the control `id` names: an index, as the view
    /// numbers them (a bare literal would hash as another integer type).
    fn nth(id: Id, k: usize) -> Id {
        id.with(k)
    }

    /// Every text the pass drew, as laid out.
    fn drawn(out: &egui::FullOutput) -> Vec<String> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    fn has(drawn: &[String], words: &str) -> bool {
        drawn.iter().any(|d| d == words)
    }

    /// The colour `words` were drawn in, where the pass drew them.
    fn color_of(out: &egui::FullOutput, words: &str) -> Option<Color32> {
        out.shapes.iter().find_map(|clipped| match &clipped.shape {
            Shape::Text(text) if text.galley.text() == words => text.galley.job.sections.first().map(|s| s.format.color),
            _ => None,
        })
    }

    fn listing(path: &str, names: &[&str]) -> DirListing {
        DirListing {
            path: path.into(),
            directories: names.iter().map(|n| DirEntry { name: (*n).into() }).collect(),
            files: Vec::new(),
        }
    }

    #[test]
    fn the_room_draws_its_card_its_table_and_its_note_and_no_header_or_tips() {
        let _en = english();
        let ctx = context();
        let mut room = seeded();
        room.queue(Op::Load, t!("admin.busy_loading"));
        let (out, shown) = settle(&ctx, &mut room);
        let words = drawn(&out);
        for w in ["Add a music folder", "NAME", "FOLDER", "SYMLINKS", "music", "/srv/music", "vinyl", "/Volumes/NAS/Vinyl Rips", "loading libraries…"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(!has(&words, "Libraries") && !words.iter().any(|w| w.contains("b browse")), "no header, no tips: {words:?}");
        assert!(shown.caret.is_none() && shown.outcome.is_none(), "no field has the keyboard");
        let pal = Pal::now();
        assert_eq!(color_of(&out, "loading libraries…"), Some(pal.accent), "the busy line in the accent");

        // Empty, the headings stay and the list says so; a gold note shows
        // once nothing is busy.
        room.libs.clear();
        room.busy = None;
        room.note = Some(("could not load libraries: could not reach server".into(), true));
        let (out, _) = settle(&ctx, &mut room);
        let words = drawn(&out);
        assert!(has(&words, "(nothing added yet)") && has(&words, "NAME"), "{words:?}");
        assert_eq!(color_of(&out, "could not load libraries: could not reach server"), Some(pal.gold), "{words:?}");
    }

    #[test]
    fn a_click_on_each_control_in_the_room_applies_the_cells_act() {
        let _en = english();
        let ctx = context();
        let mut room = seeded();
        let _ = settle(&ctx, &mut room);

        // A row takes no click as a whole: the cursor is the keys'.
        let row = ctx.read_response(nth(Id::new("libraries-symlinks"), 0)).expect("row 0 drawn").rect;
        assert_eq!(press(&ctx, &mut room, pos2(24.0 + 2.0 * studio::CELL_PAD, row.center().y)).outcome, None);
        assert_eq!(room.sel, None, "a click on a row selects nothing");
        assert!(room.queued.is_none() && matches!(room.modal, Modal::None));

        // The add card is `b`: the server's browser, this not being its machine.
        assert_eq!(click(&ctx, &mut room, Id::new("libraries-add")).outcome, None, "no click ends the room");
        assert_eq!(room.queued, Some(Op::OpenBrowser("~".into())));
        room.queued = None;

        // The tick box flips its own row's flag, through the server.
        click(&ctx, &mut room, nth(Id::new("libraries-symlinks"), 0));
        assert_eq!(room.queued, Some(Op::Symlinks { vpath: "music".into(), on: true }));
        assert!(!room.libs[0].follow_symlinks, "not until the server agrees");
        room.queued = None;

        // The remove opens its row's gate.
        click(&ctx, &mut room, nth(Id::new("libraries-remove"), 1));
        assert!(matches!(room.modal, Modal::Remove(1)));
    }

    #[test]
    fn the_cursor_row_is_scrolled_into_view_and_a_long_folder_keeps_its_leaf() {
        let _en = english();
        let ctx = context();
        let mut room = room(false);
        room.libs = (0..40)
            .map(|i| Lib { name: format!("lib{i:02}"), root: format!("/srv/{}/leaf{i:02}", "deep".repeat(40)), follow_symlinks: false })
            .collect();
        let _ = settle(&ctx, &mut room);
        // Where the table's rows stop: the note line's gap above it. A row
        // is whole in view when its foot, half a row under its tick box's
        // middle, stands no lower.
        let floor = room_rects(Rect::from_min_size(Pos2::ZERO, SIZE)).0.max.y;
        let foot = |tick: Rect| tick.center().y + ROW_H / 2.0;
        let tick = nth(Id::new("libraries-symlinks"), 30);
        assert!(ctx.read_response(tick).is_none(), "row 30 starts off screen");
        for _ in 0..31 {
            room.key(key(KeyCode::Down));
        }
        assert_eq!(room.sel, Some(30));
        let (out, _) = settle(&ctx, &mut room);
        let row = ctx.read_response(tick).expect("the cursor's row is drawn").rect;
        assert!(foot(row) <= floor + 0.5, "and whole above the note line: {row:?}");
        let words = drawn(&out);
        assert!(words.iter().any(|w| w.starts_with('…') && w.ends_with("leaf30")), "the folder keeps its leaf: {words:?}");

        // Brought into view once, on the pass it moved: the wheel takes the
        // table away from the cursor's row, and no later pass brings it back.
        wheel(&ctx, &mut room, row.center(), 4000.0);
        let _ = settle(&ctx, &mut room);
        assert!(ctx.read_response(tick).is_none(), "the wheel moved the table off the cursor's row");
        assert_eq!(room.sel, Some(30), "and left the cursor where it was");

        // A fresh add's row is brought into view and not taken by the cursor.
        for _ in 0..31 {
            room.key(key(KeyCode::Up));
        }
        assert_eq!(room.sel, Some(0));
        let _ = settle(&ctx, &mut room);
        room.key(key(KeyCode::Esc));
        let _ = settle(&ctx, &mut room);
        let tick = nth(Id::new("libraries-symlinks"), 36);
        assert!(ctx.read_response(tick).is_none(), "row 36 is off screen with the cursor gone home");
        // On the pass it is asked for, with no other to wait on.
        room.reveal = Some(36);
        let _ = pass(&ctx, &mut room, Vec::new(), "");
        let row = ctx.read_response(tick).expect("the revealed row is drawn").rect;
        assert!(foot(row) <= floor + 0.5, "and whole above the note line: {row:?}");
        assert_eq!((room.sel, room.reveal), (None, None), "revealed, not selected, and only once");
    }

    #[test]
    fn the_remove_gate_draws_its_facts_has_no_close_and_keeps_or_removes() {
        let _en = english();
        let ctx = context();
        let mut room = seeded();
        let _ = settle(&ctx, &mut room);
        let card = ctx.read_response(Id::new("libraries-add")).expect("the card").rect.center();
        room.act(Act::RemoveAt(1));
        let (out, shown) = settle(&ctx, &mut room);
        let words = drawn(&out);
        for w in ["Remove vinyl from mStream?", "The folder and its files stay on disk.", "−", "Users who hold vinyl lose it; playlists into it break.", "KEEP IT", "REMOVE"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(shown.caret.is_none(), "a gate has no field");
        let gate = Id::new("libraries-gate");
        assert!(ctx.read_response(gate.with("close")).is_none(), "a gate has no close: it is answered");

        // The room beneath is inert: a press where the card stands does nothing.
        for pressed in [true, false] {
            let press = egui::Event::PointerButton { pos: card, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() };
            let _ = pass(&ctx, &mut room, vec![egui::Event::PointerMoved(card), press], "");
        }
        assert!(room.queued.is_none() && matches!(room.modal, Modal::Remove(1)), "the scrim swallowed it");

        click(&ctx, &mut room, nth(gate.with("foot"), 0));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Keep keeps");
        room.act(Act::RemoveAt(1));
        let _ = settle(&ctx, &mut room);
        click(&ctx, &mut room, nth(gate.with("foot"), 1));
        assert_eq!(room.queued, Some(Op::Remove("vinyl".into())));
        assert!(matches!(room.modal, Modal::None));
    }

    #[test]
    fn the_name_window_draws_the_rooms_field_and_its_caret_and_adds_or_cancels() {
        let _en = english();
        let ctx = context();
        let mut room = seeded();
        room.open_name("/Volumes/NAS/Field Recordings 2024".into());
        let (out, shown) = settle(&ctx, &mut room);
        let words = drawn(&out);
        for w in ["Name this library", "FOLDER", "NAME", "/Volumes/NAS/Field Recordings 2024", "field-recordings-2024", "a-z, 0-9 and dashes", "ADD FOLDER"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        // The cells' two lines, one paragraph hung beside its dash.
        let permanent = format!("{} {}", t!("admin.name_permanent_1").trim_start_matches('−').trim(), t!("admin.name_permanent_2").trim());
        assert!(has(&words, &permanent), "the permanence, in its own words: {words:?}");
        let field = ctx.read_response(Id::new("libraries-name").with("field")).expect("the field").rect;
        let caret = shown.caret.expect("the field has the keyboard");
        assert!(field.contains(caret - vec2(0.0, 1.0)), "the caret's foot is in the box: {caret:?} {field:?}");

        // The input method's text stands at the caret; the keys still edit
        // the room's own value, through its gate.
        let (out, _) = pass(&ctx, &mut room, Vec::new(), "かな");
        assert!(has(&drawn(&out), "field-recordings-2024かな"), "{:?}", drawn(&out));
        type_text(&mut room, "X");
        let (out, _) = settle(&ctx, &mut room);
        assert!(has(&drawn(&out), "field-recordings-2024x"));

        click(&ctx, &mut room, nth(Id::new("libraries-name").with("foot"), 0));
        let directory = "/Volumes/NAS/Field Recordings 2024".to_string();
        assert_eq!(room.queued, Some(Op::Add { directory, vpath: "field-recordings-2024x".into() }));
        room.queued = None;
        room.apply(Done::Added {
            vpath: "field-recordings-2024x".into(),
            result: Err(crate::api::ApiError::Server { status: 500, message: "EACCES".into() }),
        });
        let (out, _) = settle(&ctx, &mut room);
        assert!(drawn(&out).iter().any(|w| w.starts_with("could not add") && w.contains("EACCES")), "{:?}", drawn(&out));

        // The close takes the keyboard from the field on the pass it acts
        // in: no caret is left noted for the window that is gone.
        let shown = click(&ctx, &mut room, Id::new("libraries-name").with("close"));
        assert!(matches!(room.modal, Modal::None));
        assert!(shown.caret.is_none(), "the closing pass notes no caret");
        let (_, shown) = settle(&ctx, &mut room);
        assert!(shown.caret.is_none(), "no field, no caret");
    }

    #[test]
    fn the_path_window_suggests_and_a_click_takes_one() {
        let _en = english();
        let ctx = context();
        let mut room = room(false);
        room.key(key(KeyCode::Char('t')));
        type_text(&mut room, "/srv/m");
        room.queued = None;
        room.apply(Done::Completed { dir: "/srv/".into(), listing: Ok(listing("/srv", &["media", "music", "www"])) });
        let (out, shown) = settle(&ctx, &mut room);
        let words = drawn(&out);
        for w in ["Type the full path of a music folder", "/srv/m", "media", "music"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(!has(&words, "www"), "only what the partial matches");
        assert!(shown.caret.is_some(), "the typed path has the keyboard");

        let list = Id::new("libraries-path").with("list");
        click(&ctx, &mut room, nth(list.with("row"), 1));
        let Modal::PathEntry(draft) = &room.modal else { panic!("the path window") };
        assert_eq!(draft.text.value(), "/srv/music/", "the suggestion taken, completion going on inside it");
        assert_eq!(room.queued, Some(Op::Complete("/srv/music/".into())));
        room.queued = None;

        // The current directory refused: its reason, in gold, where the list was.
        room.apply(Done::Completed { dir: "/srv/music/".into(), listing: Err("EACCES".into()) });
        let (out, _) = settle(&ctx, &mut room);
        assert_eq!(color_of(&out, "could not list /srv/music/: EACCES"), Some(Pal::now().gold), "{:?}", drawn(&out));

        click(&ctx, &mut room, Id::new("libraries-path").with("close"));
        assert!(matches!(room.modal, Modal::None));
    }

    #[test]
    fn the_browser_moves_opens_climbs_adds_and_closes() {
        let _en = english();
        let ctx = context();
        let mut room = room(false);
        let home = || Done::Browsed(Ok(listing("/home/sherika", &["Audiobooks", "Downloads", "Music", "Videos"])));
        room.apply(home());
        let (out, shown) = settle(&ctx, &mut room);
        let words = drawn(&out);
        for w in ["Browse the server's folders", "/home/sherika", "Audiobooks", "Videos", "UP", "OPEN", "CLOSE", "ADD THIS FOLDER"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(shown.caret.is_none());

        let browse = Id::new("libraries-browse");
        click(&ctx, &mut room, nth(browse.with("list").with("row"), 2));
        let Modal::Browser(b) = &room.modal else { panic!("the browser") };
        assert_eq!(b.sel, 2, "a click moves the cursor and opens nothing");
        assert!(room.queued.is_none());

        let foot = browse.with("foot");
        click(&ctx, &mut room, nth(foot, 1));
        assert_eq!(room.queued, Some(Op::BrowseTo("/home/sherika/Music".into())), "open");
        room.queued = None;
        click(&ctx, &mut room, nth(foot, 0));
        assert_eq!(room.queued, Some(Op::BrowseTo("/home".into())), "up");
        room.queued = None;
        // The cells' order: up, open, the add, close.
        click(&ctx, &mut room, nth(foot, 2));
        let Modal::Name(draft) = &room.modal else { panic!("the add names the folder listed") };
        assert_eq!(draft.directory, "/home/sherika");

        room.apply(home());
        let _ = settle(&ctx, &mut room);
        click(&ctx, &mut room, nth(foot, 3));
        assert!(matches!(room.modal, Modal::None), "close");

        room.apply(home());
        let _ = settle(&ctx, &mut room);
        click(&ctx, &mut room, browse.with("close"));
        assert!(matches!(room.modal, Modal::None), "the window's close");

        room.apply(Done::Browsed(Ok(listing("/srv/empty", &[]))));
        let (out, _) = settle(&ctx, &mut room);
        assert!(has(&drawn(&out), "(no folders in here)"), "{:?}", drawn(&out));
    }

    #[test]
    fn the_browsers_cursor_row_is_kept_in_view_and_a_window_opened_again_shows_it() {
        let _en = english();
        let ctx = context();
        let mut room = room(false);
        let names: Vec<String> = (0..40).map(|i| format!("d{i:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let srv = || Done::Browsed(Ok(listing("/srv", &names)));
        let browse = Id::new("libraries-browse");
        let rows = browse.with("list").with("row");
        // The list ends 20 points above the window's buttons.
        let list_foot = |ctx: &egui::Context| ctx.read_response(nth(browse.with("foot"), 0)).expect("the buttons").rect.min.y - 20.0;
        room.apply(srv());
        let _ = settle(&ctx, &mut room);
        assert!(ctx.read_response(nth(rows, 0)).is_some(), "the list opens at its top");
        assert!(ctx.read_response(nth(rows, 25)).is_none(), "with row 25 below it");

        // The keys move the cursor and the list follows it, the row whole.
        for _ in 0..25 {
            room.key(key(KeyCode::Down));
        }
        let _ = settle(&ctx, &mut room);
        let row = ctx.read_response(nth(rows, 25)).expect("the cursor's row is drawn").rect;
        assert!(row.max.y <= list_foot(&ctx) + 0.5, "and whole in the list: {row:?}");

        // Followed on the pass it moved, not every pass: the wheel takes the
        // list back to its top and leaves the cursor where it was.
        wheel(&ctx, &mut room, row.center(), 4000.0);
        let _ = settle(&ctx, &mut room);
        assert!(ctx.read_response(nth(rows, 25)).is_none(), "the wheel moved the list off the cursor's row");
        assert!(matches!(&room.modal, Modal::Browser(b) if b.sel == 25));

        // The window opened again on the listing it last showed shows its
        // cursor, wherever the wheel last left the list.
        room.key(key(KeyCode::Esc));
        let _ = settle(&ctx, &mut room);
        room.apply(srv());
        let _ = settle(&ctx, &mut room);
        let top = ctx.read_response(nth(rows, 0)).expect("row 0 is drawn").rect;
        wheel(&ctx, &mut room, top.center(), -4000.0);
        let _ = settle(&ctx, &mut room);
        assert!(ctx.read_response(nth(rows, 0)).is_none(), "the wheel took the list to its foot");
        room.key(key(KeyCode::Esc));
        let _ = settle(&ctx, &mut room);
        room.apply(srv());
        let _ = settle(&ctx, &mut room);
        assert!(matches!(&room.modal, Modal::Browser(b) if b.sel == 0));
        assert!(ctx.read_response(nth(rows, 0)).is_some(), "the cursor's row, at the top again");
    }
}
