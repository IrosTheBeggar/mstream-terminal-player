//! The Folders screen's body drawn by egui: the add card and the folders'
//! table, after the canvas's Main, FoldersNested and Unreachable boards.
//! The frame (egui_view.rs) draws the wordmark over it and everything
//! around it: the language chip and the step, the status and tips lines,
//! and the bar with Continue. The screen has no heading of its own: the
//! wordmark heads it, as in the cells.
//!
//! Every click is the act the cells view registers for the same control
//! (`draw_folders`): the card is the system's folder dialog (BrowseNative;
//! the wizard's own browser comes up where there is none, and typing a path
//! is the key t's alone, as in the cells), a name starts its rename
//! (RenameFolder), a row's cross removes it (RemoveAt). A press anywhere but
//! on the name being edited keeps the draft first, as the cells' blur does.
//! Rows take no click as a whole: the keyboard's cursor is the keys'.

use egui::{Align2, Color32, FontId, Id, Rect, Sense, Ui, pos2, vec2};
use rust_i18n::t;

use super::super::{Act, Folder, REMOVE_TIP, Wizard, ellipsize_path_start};
use super::{Drawn, Page, tip};
use crate::gui::window::studio::{self, Col, Glyph, HIT, Icon, RowCtx, Table, body, mono, strong};

/// The table's NAME column (FOLDER takes the rest), the remove's column
/// past the selection area with the gap before it, and the rows: the
/// boards'.
const NAME_W: f32 = 180.0;
const REMOVE_W: f32 = 36.0;
const TRAIL: f32 = REMOVE_W + 8.0;
const ROW_H: f32 = 40.0;
/// The least FOLDER keeps on a narrow page before NAME gives way.
const PATH_LEAST: f32 = 160.0;
/// The name field's box in its cell, and how far it reaches into the row's
/// padding so the draft stands where the name stood (the board's NAME
/// FIELD).
const NAME_FIELD_H: f32 = 30.0;
const REACH: f32 = 9.0;

/// The body over the whole of `ui`, the column under the wordmark: the add
/// card, then the table of the folders chosen, NAME and FOLDER, a remove
/// for each row in a column of its own past the selection area. The
/// keyboard's cursor row wears the kit's ring and is brought into view on
/// the pass it moves, a fresh add's row likewise without being selected,
/// and the wheel scrolls the table (never under a modal, whose scrim takes
/// the pointer); an empty list keeps its headings and says so where the
/// first row would be.
pub(super) fn show(ui: &mut Ui, wizard: &mut Wizard, page: &Page) -> Drawn {
    let pal = &page.pal;
    let mut drawn = Drawn::default();
    if studio::add_card(ui, Id::new("setup-add"), &t!("folders.add_card"), Glyph::Folder, pal).clicked() {
        drawn.acts.push(Act::BrowseNative);
    }
    ui.add_space(studio::ROOM_GAP);

    let cols = vec![Col::left(t!("folders.col_name"), NAME_W), Col::left(t!("folders.col_folder"), 0.0)];
    let cols = studio::fitted(ui.available_width() - TRAIL, cols, PATH_LEAST, 0.5);
    let editing = wizard.editing.as_ref().map(|(row, _)| *row);
    let nothing = t!("folders.nothing_yet");
    // The row being renamed drops the cursor's look, as the cells drop its
    // slab: the field is the row's mark then.
    let table = Table::new(Id::new("setup-folders"), &cols, wizard.folders.len())
        .cursor(wizard.sel.filter(|s| Some(*s) != editing))
        .row_height(ROW_H)
        .trail(TRAIL)
        .inert()
        .reveal(wizard.reveal.take())
        .empty(&nothing);
    let folders = &wizard.folders;
    let draft = wizard.editing.as_ref().map(|(_, draft)| draft);
    let mut field = None;
    table.show(ui, pal, |ui, i, row| {
        let folder = &folders[i];
        match draft.filter(|_| editing == Some(i)) {
            Some(draft) => {
                let cell = row.cell(0);
                let mid = cell.center().y;
                let rect = Rect::from_min_max(pos2(cell.min.x - REACH, mid - NAME_FIELD_H / 2.0), pos2(cell.max.x, mid + NAME_FIELD_H / 2.0));
                let input = studio::Input::of(draft).focused(true, page.composition).mono();
                let typed = studio::field_box(ui, Id::new("setup-rename"), rect, &input, pal);
                // The name being edited takes its own click and keeps its
                // draft, as the cells' chip does.
                if typed.response.clicked() {
                    drawn.acts.push(Act::RenameFolder(i));
                }
                drawn.caret = typed.caret;
                field = Some(rect);
            }
            None => {
                if name(ui, row, i, folder, page) {
                    drawn.acts.push(Act::RenameFolder(i));
                }
            }
        }
        path(ui, row, i, folder, page);
        let at = pos2(row.trail.max.x - REMOVE_W / 2.0, row.rect.center().y);
        let remove = studio::icon_button(ui, Id::new("setup-remove").with(i), at, Icon::Remove, pal.dim, pal);
        tip(page, &remove, REMOVE_TIP);
        if remove.clicked() {
            drawn.acts.push(Act::RemoveAt(i));
        }
    });

    // Blur is Enter: a press anywhere but on the name being edited keeps
    // the draft before that press does what it does, as the cells' press
    // does, a name scrolled out of view included.
    let pressed = ui.input(|i| if i.pointer.primary_pressed() { i.pointer.press_origin() } else { None });
    if wizard.editing.is_some()
        && let Some(at) = pressed
        && field.is_none_or(|field: Rect| !field.contains(at))
    {
        wizard.finish_rename();
        drawn.caret = None;
    }
    drawn
}

/// A row's name, the folder's vpath, in its cell: a button as wide as its
/// words in the text colour, white under the pointer with the hand,
/// semibold on the cursor's row, cut plainly at the cell's edge as the
/// cells cut it. Its tooltip says what a click does, in the cells' words:
/// rename it, or for a folder already on the server that it cannot be
/// renamed here. Whether it was clicked.
fn name(ui: &Ui, row: &RowCtx, i: usize, folder: &Folder, page: &Page) -> bool {
    let pal = &page.pal;
    let cell = row.cell(0);
    let font = if row.selected { strong(14.0) } else { body(14.0) };
    let words = studio::width_of(ui, &folder.name, font.clone()).clamp(8.0, cell.width().max(8.0));
    let rect = Rect::from_min_size(pos2(cell.min.x, cell.center().y - HIT / 2.0), vec2(words, HIT));
    let response = studio::hand(ui.interact(rect, Id::new("setup-name").with(i), Sense::click()));
    let color = if response.hovered() { Color32::WHITE } else { pal.text };
    clipped(ui, cell, &folder.name, font, color);
    let why = if folder.committed { t!("note.committed_rename") } else { t!("folders.tip_name") };
    tip(page, &response, &why);
    response.clicked()
}

/// A row's folder in its cell: the path exactly as it was added, cut at its
/// start so its leaf stays, in the monospace; the soft colour, or the gold
/// behind a gold LED where its check found a problem, on the cursor's row
/// too (the cursor is a ring, not the cells' slab). The whole cell is the
/// tooltip's target, words or blank, as the cells' is: the problem's words,
/// else the whole path where it was cut, at most the cells' three lines.
fn path(ui: &Ui, row: &RowCtx, i: usize, folder: &Folder, page: &Page) {
    let pal = &page.pal;
    let cell = row.cell(1);
    let mid = cell.center().y;
    let font = mono(12.5);
    let problem = folder.problem();
    // The LED's 6 points and the board's 8 after it.
    let x = match problem {
        Some(_) => {
            studio::led(ui, pos2(cell.min.x + 3.0, studio::led_mid(ui, mid, &font, false)), pal.gold);
            cell.min.x + 6.0 + 8.0
        }
        None => cell.min.x,
    };
    let room = (cell.max.x - x).max(1.0);
    let (shown, cut) = studio::cut_start(ui, &folder.path, font.clone(), room);
    let color = if problem.is_some() { pal.gold } else { pal.soft };
    studio::text(ui, pos2(x, mid), &shown, font, color, room, Align2::LEFT_CENTER);
    let response = ui.interact(cell, Id::new("setup-path").with(i), Sense::hover());
    match problem {
        Some(why) => tip(page, &response, &why),
        None if cut => tip(page, &response, &ellipsize_path_start(&folder.path, crate::kit::TIP_WRAP * 3).0),
        None => {}
    }
}

/// `s` in one line from `rect`'s left, its middle on `rect`'s, cut plainly
/// at the rect's right edge: no ellipsis, as the cells and the canvas cut a
/// long name.
fn clipped(ui: &Ui, rect: Rect, s: &str, font: FontId, color: Color32) {
    let galley = ui.painter().layout_no_wrap(s.to_string(), font, color);
    let at = pos2(rect.min.x, rect.center().y - galley.size().y / 2.0);
    ui.painter().with_clip_rect(rect.intersect(ui.clip_rect())).galley(at, galley, color);
}

#[cfg(test)]
mod tests {
    use egui::epaint::Shape;
    use egui::{Event, Id, Pos2, Rect, pos2, vec2};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rust_i18n::t;

    use super::super::tests::{BOARD, Probe, color_of, drawn, has, wizard};
    use crate::gui::window::studio::Pal;
    use crate::setup::tests::LOCALE_LOCK;
    use crate::setup::{Act, Done, FolderCheck, Modal, Op, Wizard, ellipsize_path_start, handle_key};

    /// Row `i`'s control under `id`: an index, as the view numbers them.
    fn nth(id: &str, i: usize) -> Id {
        Id::new(id).with(i)
    }

    fn key(wizard: &mut Wizard, code: KeyCode) {
        handle_key(wizard, KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// A check's answer for `path`, folded in as the frame half folds the
    /// worker's.
    fn checked(wizard: &mut Wizard, path: &str, check: FolderCheck) {
        let canonical = (check == FolderCheck::Ok).then(|| path.to_string());
        wizard.apply(Done::Validated { path: path.to_string(), check, canonical });
    }

    /// The wheel turned over `at`, `dy` points of a swipe.
    fn wheel(probe: &Probe, wizard: &mut Wizard, at: Pos2, dy: f32) {
        let turn = |phase, dy| Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, dy),
            phase,
            modifiers: egui::Modifiers::default(),
        };
        let _ = probe.pass(wizard, vec![Event::PointerMoved(at)]);
        let _ = probe.pass(wizard, vec![turn(egui::TouchPhase::Start, 0.0), turn(egui::TouchPhase::Move, dy)]);
        let _ = probe.pass(wizard, vec![turn(egui::TouchPhase::End, 0.0)]);
    }

    #[test]
    fn rows_draw_their_names_and_paths_and_a_long_path_keeps_its_leaf_and_tips_the_whole() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = wizard();
        wizard.add_folder("C:\\Users\\anna\\Music".into());
        let long = format!("D:\\{}\\Vinyl Rips", "Archive\\Lossless\\".repeat(9).trim_end_matches('\\'));
        wizard.add_folder(long.clone());
        let (out, shown) = probe.settle(&mut wizard);
        let words = drawn(&out);
        let hint = t!("hint.folders_rows").to_string();
        for w in ["music", "vinyl-rips", "C:\\Users\\anna\\Music", "NAME", "FOLDER", "CONTINUE", hint.as_str()] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(!has(&words, "(nothing added yet)"));
        assert!(words.iter().any(|w| w.starts_with('…') && w.ends_with("\\Vinyl Rips")), "the leaf kept: {words:?}");
        assert_eq!(color_of(&out, "C:\\Users\\anna\\Music"), Some(pal.soft), "a path checked or not yet: plain");
        assert_eq!(color_of(&out, "music"), Some(pal.text));
        assert_eq!(color_of(&out, "CONTINUE"), Some(pal.on_accent), "Continue lit: there is something to continue with");
        assert!(shown.caret.is_none(), "no name is being edited");
        // The cut path's whole, in its tooltip, at most the cells' three lines.
        let cell = probe.rect(nth("setup-path", 1)).center();
        let tip = ellipsize_path_start(&long, crate::kit::TIP_WRAP * 3).0;
        assert!(has(&drawn(&probe.rest(&mut wizard, cell, 0.6)), &tip), "the whole path in the tip");
        // A short one, whole in its cell, has none: its words once, the cell's.
        let cell = probe.rect(nth("setup-path", 0)).center();
        let words = drawn(&probe.rest(&mut wizard, cell, 0.6));
        assert_eq!(words.iter().filter(|w| *w == "C:\\Users\\anna\\Music").count(), 1, "{words:?}");
    }

    #[test]
    fn a_folder_inside_another_is_gold_behind_a_gold_led_and_its_tip_says_why() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = wizard();
        for path in ["/home/anna/Music", "/home/anna/Vinyl", "/home/anna/Music/Jazz"] {
            wizard.add_folder(path.into());
        }
        for path in ["/home/anna/Music", "/home/anna/Vinyl", "/home/anna/Music/Jazz"] {
            checked(&mut wizard, path, FolderCheck::Ok);
        }
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!(color_of(&out, "/home/anna/Music/Jazz"), Some(pal.gold), "the problem path in gold");
        assert_eq!(color_of(&out, "/home/anna/Music"), Some(pal.soft), "its parent plain");
        let note = wizard.note.clone().expect("the gold note").0;
        assert_eq!(color_of(&out, &note), Some(pal.gold), "and the status line says so: {note}");
        assert_eq!(color_of(&out, "CONTINUE"), Some(pal.on_accent), "checks advise: Continue stays live");
        // One gold LED, at the start of the problem row's FOLDER cell.
        let cell = probe.rect(nth("setup-path", 2));
        let leds: Vec<Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Rect(r) if r.fill == pal.gold && (r.rect.width() - 6.0).abs() < 0.01 => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(leds.len(), 1, "{leds:?}");
        assert!(cell.contains(leds[0].center()) && leds[0].min.x - cell.min.x < 1.0, "{leds:?} in {cell:?}");
        // Resting anywhere in the cell, the words why.
        let blank = pos2(cell.max.x - 10.0, cell.center().y);
        assert!(has(&drawn(&probe.rest(&mut wizard, blank, 0.6)), &t!("folders.warn_nested")));
        // The local check's other answers wear the same gold and tell their own.
        checked(&mut wizard, "/home/anna/Vinyl", FolderCheck::Missing);
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!(color_of(&out, "/home/anna/Vinyl"), Some(pal.gold));
        let cell = probe.rect(nth("setup-path", 1)).center();
        assert!(has(&drawn(&probe.rest(&mut wizard, cell, 0.6)), &t!("folders.warn_missing")));
    }

    #[test]
    fn a_click_on_each_control_applies_the_cells_act() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = wizard();
        let _ = probe.settle(&mut wizard);
        // The card is the system's folder dialog, as b is.
        let add = probe.rect(Id::new("setup-add")).center();
        let _ = probe.pass(&mut wizard, vec![Event::PointerMoved(add)]);
        assert!(probe.pass(&mut wizard, Vec::new()).1.hand, "the hand over the card");
        let shown = probe.click(&mut wizard, Id::new("setup-add"));
        assert!(shown.outcome.is_none(), "no click here ends the wizard");
        assert_eq!(wizard.queued, Some(Op::PickNative));
        assert_eq!(wizard.busy.as_deref(), Some("opening the folder picker…"));
        (wizard.queued, wizard.busy) = (None, None);

        for path in ["/srv/music", "/srv/vinyl", "/srv/talk"] {
            wizard.add_folder(path.into());
        }
        let _ = probe.settle(&mut wizard);
        // A name starts its rename, and selects nothing.
        probe.click(&mut wizard, nth("setup-name", 1));
        assert_eq!(wizard.editing.as_ref().map(|(row, draft)| (*row, draft.value().to_string())), Some((1, "vinyl".into())));
        assert_eq!(wizard.sel, None);
        let (_, shown) = probe.settle(&mut wizard);
        assert!(shown.caret.is_some(), "the field has the keyboard");
        // A row's cross removes it, the press keeping the draft first.
        probe.click(&mut wizard, nth("setup-remove", 2));
        assert_eq!(wizard.folders.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["music", "vinyl"]);
        assert!(wizard.editing.is_none() && wizard.folders[1].named_by_user, "blur kept the draft");
        // A folder already on the server takes no rename: the note says
        // where to, and its tip.
        wizard.folders[0].committed = true;
        let _ = probe.settle(&mut wizard);
        probe.click(&mut wizard, nth("setup-name", 0));
        assert!(wizard.editing.is_none());
        assert_eq!(wizard.note, Some((t!("note.committed_rename").to_string(), false)));
        let name = probe.rect(nth("setup-name", 0)).center();
        assert!(has(&drawn(&probe.rest(&mut wizard, name, 0.6)), &t!("note.committed_rename")));
        let name = probe.rect(nth("setup-name", 1)).center();
        assert!(has(&drawn(&probe.rest(&mut wizard, name, 0.6)), &t!("folders.tip_name")));
        // A row takes no click as a whole: the cursor is the keys'.
        let row = probe.rect(nth("setup-path", 1));
        probe.press(&mut wizard, pos2(row.max.x - 4.0, row.center().y));
        assert_eq!((wizard.sel, wizard.editing.is_none()), (None, true));
        // The cross's tip is the cells'. (egui shows none on a move that
        // follows a click at once: a beat first.)
        let _ = probe.pass_after(&mut wizard, Vec::new(), 0.2);
        let cross = probe.rect(nth("setup-remove", 1)).center();
        assert!(has(&drawn(&probe.rest(&mut wizard, cross, 0.6)), super::REMOVE_TIP));
    }

    #[test]
    fn the_name_field_draws_the_draft_its_caret_and_a_composition_and_blur_keeps_it() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let mut probe = Probe::new(BOARD);
        let mut wizard = wizard();
        wizard.add_folder("/srv/music".into());
        wizard.add_folder("/srv/vinyl".into());
        wizard.act(Act::RenameFolder(1));
        let (out, shown) = probe.settle(&mut wizard);
        assert!(has(&drawn(&out), "vinyl"));
        let field = probe.rect(Id::new("setup-rename"));
        let caret = shown.caret.expect("the field's caret");
        assert!(field.contains(caret - vec2(0.0, 1.0)), "the caret's foot is in the field: {caret:?} {field:?}");
        // The keys edit the model's draft through the cells' gate, and the
        // input method's text stands at the caret.
        key(&mut wizard, KeyCode::Char('X'));
        key(&mut wizard, KeyCode::Char('!'));
        probe.composition = "かな".into();
        let (out, later) = probe.settle(&mut wizard);
        assert!(has(&drawn(&out), "vinylxかな"), "{:?}", drawn(&out));
        assert!(later.caret.expect("still the field's").x > caret.x, "the caret after the composition");
        probe.composition.clear();
        // A click on the field keeps the draft, as a click on the cells'
        // chip does.
        probe.click(&mut wizard, Id::new("setup-rename"));
        assert_eq!(wizard.editing.as_ref().map(|(_, draft)| draft.value().to_string()), Some("vinylx".into()));
        // A press anywhere else keeps it, and the keyboard goes with it.
        let below = pos2(field.center().x, field.max.y + 200.0);
        let shown = probe.press(&mut wizard, below);
        assert!(wizard.editing.is_none() && shown.caret.is_none(), "blur is Enter");
        assert_eq!(wizard.folders[1].name, "vinylx");
        let (_, shown) = probe.settle(&mut wizard);
        assert!(shown.caret.is_none(), "no field, no caret");
        // Esc drops a draft.
        wizard.act(Act::RenameFolder(0));
        key(&mut wizard, KeyCode::Char('z'));
        key(&mut wizard, KeyCode::Esc);
        let _ = probe.settle(&mut wizard);
        assert_eq!(wizard.folders[0].name, "music");
    }

    #[test]
    fn the_cursor_row_is_brought_into_view_and_the_wheel_scrolls_the_table_only_with_no_modal_up() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = wizard();
        for i in 0..40 {
            wizard.add_folder(format!("/srv/music/{i:02}"));
        }
        // The last add is revealed on its pass, then the cursor brought to row 0.
        let _ = probe.settle(&mut wizard);
        assert!(probe.ctx.read_response(nth("setup-name", 39)).is_some(), "a fresh add's row, brought into view");
        key(&mut wizard, KeyCode::Down);
        let _ = probe.settle(&mut wizard);
        assert!(probe.ctx.read_response(nth("setup-name", 0)).is_some(), "the cursor's row, at the top");
        assert!(probe.ctx.read_response(nth("setup-name", 30)).is_none());
        for _ in 0..30 {
            key(&mut wizard, KeyCode::Down);
        }
        assert_eq!(wizard.sel, Some(30));
        let (out, _) = probe.settle(&mut wizard);
        let row = probe.rect(nth("setup-path", 30));
        assert!(row.max.y <= 640.0, "the cursor's row whole above the foot: {row:?}");
        assert_eq!(color_of(&out, "30"), Some(Pal::now().text), "{:?}", drawn(&out));
        // The wheel scrolls the table; under a modal it does not.
        wheel(&probe, &mut wizard, row.center(), 4000.0);
        let _ = probe.settle(&mut wizard);
        assert!(probe.ctx.read_response(nth("setup-name", 30)).is_none(), "the wheel took the table to its top");
        let top = probe.rect(nth("setup-name", 0));
        wizard.modal = Modal::Language(0);
        let _ = probe.settle(&mut wizard);
        wheel(&probe, &mut wizard, top.center(), -4000.0);
        let _ = probe.settle(&mut wizard);
        assert!(probe.ctx.read_response(nth("setup-name", 0)).is_some(), "the modal's scrim took the wheel");
        assert_eq!(wizard.sel, Some(30), "and the cursor never moved");
    }
}
