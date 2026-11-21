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

use egui::{Align2, Color32, Id, Rect, Sense, Ui, pos2, vec2};
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
/// The room the column keeps for the add card at its full height: the card
/// and its gap, the table's headings and two of its rows. A shorter column
/// (the cells' floor, where the tall card left the table a sliver of one
/// row) takes the short look: the kit's short card [`SHORT_GAP`] over the
/// table, and rows [`SHORT_ROW_H`] tall, the name field's box with 3 points
/// over and under it, so that the floor shows two rows whole.
const TALL_ROOM: f32 = studio::CARD_H + studio::ROOM_GAP + studio::TABLE_HEAD + 2.0 * ROW_H;
const SHORT_GAP: f32 = 8.0;
const SHORT_ROW_H: f32 = NAME_FIELD_H + 6.0;

/// The body over the whole of `ui`, the column under the wordmark: the add
/// card, then the table of the folders chosen, NAME and FOLDER, a remove
/// for each row in a column of its own past the selection area, both in the
/// short look where the column has not [`TALL_ROOM`]. The keyboard's cursor
/// row wears the kit's ring and is brought into view on the pass it moves,
/// a fresh add's row likewise without being selected, and the wheel scrolls
/// the table (never under a modal, whose scrim takes the pointer); an empty
/// list keeps its headings and says so where the first row would be.
pub(super) fn show(ui: &mut Ui, wizard: &mut Wizard, page: &Page) -> Drawn {
    let pal = &page.pal;
    let mut drawn = Drawn::default();
    let short = ui.available_height() < TALL_ROOM;
    let (id, label) = (Id::new("setup-add"), t!("folders.add_card"));
    let add = if short {
        studio::short_add_card(ui, id, &label, Glyph::Folder, pal)
    } else {
        studio::add_card(ui, id, &label, Glyph::Folder, pal)
    };
    if add.clicked() {
        drawn.acts.push(Act::BrowseNative);
    }
    ui.add_space(if short { SHORT_GAP } else { studio::ROOM_GAP });

    let cols = vec![Col::left(t!("folders.col_name"), NAME_W), Col::left(t!("folders.col_folder"), 0.0)];
    let cols = studio::fitted(ui.available_width() - TRAIL, cols, PATH_LEAST, 0.5);
    let editing = wizard.editing.as_ref().map(|(row, _)| *row);
    let nothing = t!("folders.nothing_yet");
    // The row being renamed drops the cursor's look, as the cells drop its
    // slab: the field is the row's mark then. The cursor itself stays, so
    // the rename's end is no move of it and the view stands where the wheel
    // put it, as the cells' does.
    let table = Table::new(Id::new("setup-folders"), &cols, wizard.folders.len())
        .cursor(wizard.sel)
        .unmarked(editing)
        .row_height(if short { SHORT_ROW_H } else { ROW_H })
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
                // Where it shows: a row the wheel has half scrolled under
                // the headings shows the rest of its field to no press.
                field = Some(rect.intersect(ui.clip_rect()));
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
/// semibold on the cursor's row, cut with an ellipsis at the cell's edge,
/// as the boards' button cuts it and the kit cuts a name (the cells cut it
/// plainly, its last letter touching the path: no gap in the cells, the
/// columns' 16 points here). Its tooltip says what a click does, in the
/// cells' words: rename it, or for a folder already on the server that it
/// cannot be renamed here. Whether it was clicked.
fn name(ui: &Ui, row: &RowCtx, i: usize, folder: &Folder, page: &Page) -> bool {
    let pal = &page.pal;
    let cell = row.cell(0);
    let font = if row.selected { strong(14.0) } else { body(14.0) };
    let words = studio::width_of(ui, &folder.name, font.clone()).clamp(8.0, cell.width().max(8.0));
    let rect = Rect::from_min_size(pos2(cell.min.x, cell.center().y - HIT / 2.0), vec2(words, HIT));
    let response = studio::hand(ui.interact(rect, Id::new("setup-name").with(i), Sense::click()));
    let color = if response.hovered() { Color32::WHITE } else { pal.text };
    studio::text(ui, cell.left_center(), &folder.name, font, color, cell.width().max(1.0), Align2::LEFT_CENTER);
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

#[cfg(test)]
mod tests {
    use egui::epaint::Shape;
    use egui::{Event, Id, Rect, pos2, vec2};
    use ratatui::crossterm::event::KeyCode;
    use rust_i18n::t;

    use super::super::tests::{BOARD, Probe, color_of, drawn, has, key, tipped, wheel, wizard};
    use crate::gui::window::studio::{self, Pal};
    use crate::setup::tests::LOCALE_LOCK;
    use crate::setup::{Act, Done, FolderCheck, Modal, Op, Wizard, ellipsize_path_start};

    /// Row `i`'s control under `id`: an index, as the view numbers them.
    fn nth(id: &str, i: usize) -> Id {
        Id::new(id).with(i)
    }

    /// A check's answer for `path`, folded in as the frame half folds the
    /// worker's.
    fn checked(wizard: &mut Wizard, path: &str, check: FolderCheck) {
        let canonical = (check == FolderCheck::Ok).then(|| path.to_string());
        wizard.apply(Done::Validated { path: path.to_string(), check, canonical });
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
        wizard.add_folder("/srv/The Complete Recordings Of Everything Ever".into());
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
        // A name longer than NAME is cut with an ellipsis inside it, as the
        // boards' name button cuts one, and its click is no wider.
        let cut = out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == "the-complete-recordings-of-everything-ever" => Some(t.galley.clone()),
            _ => None,
        });
        let cut = cut.expect("the long name drawn");
        assert!(cut.elided && cut.size().x <= super::NAME_W + 0.5, "cut with an ellipsis: {:?}", cut.size());
        let name = probe.rect(nth("setup-name", 2));
        assert!(name.width() <= super::NAME_W + 0.5, "{name:?}");
        // The cut path's whole, in its tooltip, at most the cells' three lines.
        let cell = probe.rect(nth("setup-path", 1)).center();
        let tip = tipped(&ellipsize_path_start(&long, crate::kit::TIP_WRAP * 3).0);
        assert_eq!(tip.lines().count(), 3, "{tip}");
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
        // Resting anywhere in the cell, the words why, in the lines the
        // cells' 40-cell wrap breaks them into (the board's two).
        let blank = pos2(cell.max.x - 10.0, cell.center().y);
        let why = "Inside another chosen folder — it would\nbe scanned twice";
        assert_eq!(tipped(&t!("folders.warn_nested")), why);
        assert!(has(&drawn(&probe.rest(&mut wizard, blank, 0.6)), why));
        // The local check's other answers wear the same gold and tell their own.
        checked(&mut wizard, "/home/anna/Vinyl", FolderCheck::Missing);
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!(color_of(&out, "/home/anna/Vinyl"), Some(pal.gold));
        let cell = probe.rect(nth("setup-path", 1)).center();
        assert!(has(&drawn(&probe.rest(&mut wizard, cell, 0.6)), &tipped(&t!("folders.warn_missing"))));
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
        // (Its tip in the cells' lines, which the note's one line is not;
        // egui shows none where the pointer rests after a click until it
        // has moved a beat later, so it goes away and comes back.)
        let name = probe.rect(nth("setup-name", 0)).center();
        let _ = probe.pass_after(&mut wizard, vec![Event::PointerMoved(pos2(4.0, 4.0))], 0.2);
        let seen = drawn(&probe.rest(&mut wizard, name, 0.6));
        assert!(has(&seen, &tipped(&t!("note.committed_rename"))), "{seen:?}");
        let name = probe.rect(nth("setup-name", 1)).center();
        assert!(has(&drawn(&probe.rest(&mut wizard, name, 0.6)), &tipped(&t!("folders.tip_name"))));
        // A row takes no click as a whole: the cursor is the keys'.
        let row = probe.rect(nth("setup-path", 1));
        probe.press(&mut wizard, pos2(row.max.x - 4.0, row.center().y));
        assert_eq!((wizard.sel, wizard.editing.is_none()), (None, true));
        // The cross's tip is the cells'. (egui shows none on a move that
        // follows a click at once: a beat first.)
        let _ = probe.pass_after(&mut wizard, Vec::new(), 0.2);
        let cross = probe.rect(nth("setup-remove", 1)).center();
        assert!(has(&drawn(&probe.rest(&mut wizard, cross, 0.6)), &tipped(super::REMOVE_TIP)));
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

    /// The name being edited keeps a press only where its field shows: over
    /// the part of it the wheel has scrolled under the headings, a press is
    /// one anywhere else, which keeps the draft and ends the edit, as the
    /// cells' press off their chip does.
    #[test]
    fn a_press_over_the_name_field_scrolled_out_of_view_ends_the_edit() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = wizard();
        for i in 0..40 {
            wizard.add_folder(format!("/srv/music/{i:02}"));
        }
        let _ = probe.settle(&mut wizard);
        wheel(&probe, &mut wizard, probe.rect(nth("setup-path", 39)).center(), 4000.0);
        wizard.act(Act::RenameFolder(0));
        let _ = probe.settle(&mut wizard);
        // Half a row down: the field's top half under the headings.
        wheel(&probe, &mut wizard, probe.rect(Id::new("setup-rename")).center(), -20.0);
        for _ in 0..10 {
            let _ = probe.pass_after(&mut wizard, Vec::new(), 0.1);
        }
        let field = probe.ctx.read_response(Id::new("setup-rename")).expect("the field, half in view");
        assert!(field.rect.min.y < field.interact_rect.min.y - 8.0, "half under the headings: {field:?}");
        probe.press(&mut wizard, pos2(field.rect.center().x, field.rect.min.y + 2.0));
        assert!(wizard.editing.is_none(), "a press where the field does not show is no press on it");
        assert_eq!(wizard.folders[0].name, "00", "the draft kept");
    }

    /// At the cells' floor the column has no room for the tall add card
    /// and two rows under it: the card and the rows are the short ones, and
    /// the table shows both folders whole, where the tall card left it 35
    /// points of one 40-point row. On the boards' page both keep their
    /// height.
    #[test]
    fn at_the_cells_floor_the_add_card_is_short_and_the_table_shows_a_row_whole() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let mut wizard = wizard();
        wizard.add_folder("/srv/music".into());
        wizard.add_folder("/srv/vinyl".into());
        let probe = Probe::new(super::super::FLOOR);
        let _ = probe.settle(&mut wizard);
        // The reveal's scroll is egui's, eased over a few passes.
        for _ in 0..10 {
            let _ = probe.pass_after(&mut wizard, Vec::new(), 0.1);
        }
        let (out, _) = probe.pass(&mut wizard, Vec::new());
        assert!(has(&drawn(&out), "Add a music folder"), "the card keeps its words");
        assert_eq!(probe.rect(Id::new("setup-add")).height(), studio::HIT, "the short card");
        let seen = |i: usize| probe.ctx.read_response(nth("setup-path", i)).map(|r| r.interact_rect.height());
        assert_eq!((seen(0), seen(1)), (Some(super::SHORT_ROW_H), Some(super::SHORT_ROW_H)), "both rows whole");
        // The add card's click is the same at either height, and a name's
        // field fits its short row.
        probe.click(&mut wizard, Id::new("setup-add"));
        assert_eq!(wizard.queued, Some(Op::PickNative));
        wizard.act(Act::RenameFolder(0));
        let (_, shown) = probe.settle(&mut wizard);
        let (field, row) = (probe.rect(Id::new("setup-rename")), probe.rect(nth("setup-path", 0)));
        assert!(field.min.y >= row.min.y && field.max.y <= row.max.y && shown.caret.is_some(), "{field:?} in {row:?}");

        let probe = Probe::new(BOARD);
        let _ = probe.settle(&mut wizard);
        assert_eq!(probe.rect(Id::new("setup-add")).height(), studio::CARD_H, "the tall card on the boards' page");
        assert_eq!(probe.rect(nth("setup-path", 0)).height(), super::ROW_H, "and the tall rows");
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

    /// Renaming the cursor's own row hides the cursor's look, not the
    /// cursor: when the rename ends, by a blur or by Enter, the view stays
    /// where the wheel put it, as the cells' stays (they bring the cursor in
    /// only when it moves), and a click that ended it does what it was for.
    #[test]
    fn ending_the_cursor_rows_rename_leaves_the_view_where_the_wheel_put_it() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = wizard();
        for i in 0..40 {
            wizard.add_folder(format!("/srv/music/{i:02}"));
        }
        let _ = probe.settle(&mut wizard);
        for _ in 0..4 {
            key(&mut wizard, KeyCode::Down);
        }
        assert_eq!(wizard.sel, Some(3));
        let _ = probe.settle(&mut wizard);
        let at_foot = |probe: &Probe| probe.ctx.read_response(nth("setup-path", 35)).is_some() && probe.ctx.read_response(nth("setup-path", 3)).is_none();
        let settled = |probe: &Probe, wizard: &mut Wizard| {
            for _ in 0..10 {
                let _ = probe.pass_after(wizard, Vec::new(), 0.1);
            }
        };

        // A blur: the click on another row's name that ends the rename
        // starts that row's, the view left at the foot.
        key(&mut wizard, KeyCode::Enter);
        assert_eq!(wizard.editing.as_ref().map(|(row, _)| *row), Some(3));
        let _ = probe.settle(&mut wizard);
        wheel(&probe, &mut wizard, probe.rect(nth("setup-path", 3)).center(), -4000.0);
        settled(&probe, &mut wizard);
        assert!(at_foot(&probe), "the wheel took the table to its foot");
        probe.click(&mut wizard, nth("setup-name", 35));
        assert_eq!(wizard.editing.as_ref().map(|(row, _)| *row), Some(35), "the click started row 35's rename");
        assert_eq!(wizard.folders[3].name, "03", "and kept row 3's draft");
        assert_eq!(wizard.sel, Some(3), "the cursor never moved");
        settled(&probe, &mut wizard);
        assert!(at_foot(&probe), "the view stayed at the foot");
        assert!(probe.pass(&mut wizard, Vec::new()).1.caret.is_some(), "the field in view keeps the keyboard");
        let field = probe.rect(Id::new("setup-rename"));
        assert!(field.max.y <= BOARD.y && field.min.y >= 0.0, "{field:?}");

        // Enter on the cursor's own row's rename, likewise.
        key(&mut wizard, KeyCode::Esc);
        key(&mut wizard, KeyCode::Up);
        let _ = probe.settle(&mut wizard);
        key(&mut wizard, KeyCode::Down);
        let _ = probe.settle(&mut wizard);
        assert!(probe.ctx.read_response(nth("setup-path", 3)).is_some(), "the cursor's move brought it in");
        key(&mut wizard, KeyCode::Enter);
        assert_eq!(wizard.editing.as_ref().map(|(row, _)| *row), Some(3));
        let _ = probe.settle(&mut wizard);
        wheel(&probe, &mut wizard, probe.rect(nth("setup-path", 3)).center(), -4000.0);
        settled(&probe, &mut wizard);
        assert!(at_foot(&probe), "the wheel took the table to its foot");
        key(&mut wizard, KeyCode::Enter);
        assert!(wizard.editing.is_none());
        settled(&probe, &mut wizard);
        assert!(at_foot(&probe), "the view stayed at the foot");
    }

    /// The row being renamed wears no cursor: no raised fill and no ring,
    /// as the cells drop its slab, while the cursor is still there for the
    /// rows after the rename.
    #[test]
    fn the_row_being_renamed_drops_the_cursors_look() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = wizard();
        for path in ["/srv/music", "/srv/vinyl"] {
            wizard.add_folder(path.into());
        }
        key(&mut wizard, KeyCode::Down);
        let raised = |out: &egui::FullOutput, row: Rect| {
            out.shapes.iter().any(|c| matches!(&c.shape, Shape::Rect(r) if r.fill == pal.raised && r.rect.intersects(row.shrink(1.0))))
        };
        let (out, _) = probe.settle(&mut wizard);
        let row = probe.rect(nth("setup-path", 0));
        assert!(raised(&out, row), "the cursor's row raised");
        key(&mut wizard, KeyCode::Enter);
        let (out, _) = probe.settle(&mut wizard);
        assert!(!raised(&out, row), "the row being renamed is not");
        assert_eq!(color_of(&out, "vinyl"), Some(pal.text));
        key(&mut wizard, KeyCode::Enter);
        let (out, _) = probe.settle(&mut wizard);
        assert!(raised(&out, row), "the cursor's look back with the rename's end");
    }
}
