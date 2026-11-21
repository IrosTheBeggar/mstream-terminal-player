//! The modals over the wizard's page drawn by egui, after the canvas's
//! FoldersPath, FoldersBrowse and Language boards: the type-a-path window,
//! the folder browser and the language list, each the kit's window over its
//! scrim (`studio::Dialog`), the page beneath inert (the frame drops what
//! was clicked under it). The skip gate is login.rs's.
//!
//! Each draws what the wizard's modal holds and nothing else. The keys
//! answer every one as they always have (Tab, the arrows and Enter in the
//! path, the arrows, Enter and a in the browser, the arrows and Enter in
//! the list, Esc in all three), and a click is the act the cells view
//! registers for the same control (draw_path_entry, draw_browser and
//! draw_language in cells.rs). Each has the close the boards draw at its
//! title's right, where the cells draw their [X], with the cells' tooltip.
//! Where the window leaves them clear, the frame's status line and tips
//! line are drawn again over the scrim, lit, as every board stands them
//! ([`lift_lines`]): the modal's hint, a listing being read, a refusal, the
//! machine translation's note.
//!
//! Each takes the root `ui`, `over` (the whole page, the scrim's reach),
//! the wizard and the frame's [`Page`], and hands back what its clicks
//! asked for and the caret of its field with the keyboard.

use egui::{Align2, Id, LayerId, Order, Rect, Sense, Ui, pos2, vec2};
use rust_i18n::t;

use super::super::{Act, LANGS, Modal, Wizard};
use super::{Drawn, Page, parts, status_line, tip_keyed, tips_line};
use crate::gui::window::studio::{self, BROWSER_W, Dialog, FIELD_H, FOLDER_ROW_H, FORM_BUTTON_H, ModalShown, Table, body, strong};

/// The language list's window at its narrowest, the board's 300: wider
/// where the title or an autonym asks for more.
const LIST_W: f32 = 300.0;
/// The most suggestions the path shows at once, the cells' six.
const SUGGEST_ROWS: usize = 6;
/// The most folders the browser shows at once: the board's nine, where the
/// kit's browser shows eleven, so that on the boards' page the window ends
/// over the status line it leaves lit (FoldersBrowse: y 178 to 622, the
/// line at 640).
const BROWSE_ROWS: usize = 9;
/// What the kit's browser lays round its list: the path's line and the 12
/// points under it, then the 20 over the foot.
const BROWSE_ROUND: f32 = 18.0 + 12.0 + 20.0;
/// The kit's window round a modal's body: the padding that stands its foot
/// this far under the body's, and the room its close takes beside the
/// title (studio.rs keeps both its own).
const WINDOW_PAD: f32 = 24.0;
const CLOSE_ROOM: f32 = 40.0;
/// Where a list row's words start, and the room after them: the kit's
/// folder row's, whose marker stands centred 13 points in.
const WORDS_IN: f32 = 26.0;
const WORDS_AFTER: f32 = 10.0;
const MARK_IN: f32 = 13.0;

/// The language list (`Modal::Language`, its cursor in it), after the
/// canvas's Language board: every language the wizard speaks by its own
/// name, in the order of [`LANGS`], a row each, the current one's LED lit in
/// the accent where the cells put their "•" and the rest unlit, its name,
/// the cursor's and the pointer's in the text colour and the others' a step
/// under it. The keys' cursor is the model's, ringed and kept in view. A click on a row
/// chooses that language at once, as the cells' click and Enter do, and
/// the close is the cells' (the path's cancel, which closes whatever modal
/// is up). The window is as wide as the widest name or the title wants,
/// measured from their words in the faces drawn (a CJK name is narrower
/// than its cells; a German title may be longer than the English).
pub(super) fn language(ui: &mut Ui, over: Rect, wizard: &mut Wizard, page: &Page) -> Drawn {
    let Modal::Language(sel) = wizard.modal else { return Drawn::default() };
    let pal = &page.pal;
    let id = Id::new("setup-language-list");
    let title = t!("lang.modal_title");
    let widest = LANGS.iter().map(|(_, name)| studio::width_of(ui, name, body(14.0))).fold(0.0, f32::max);
    let wants = (WORDS_IN + widest + WORDS_AFTER).max(studio::width_of(ui, &title, strong(16.0)) + CLOSE_ROOM);
    let width = LIST_W.max((wants + 2.0 * WINDOW_PAD).ceil());
    let current = wizard.lang;
    let dialog = Dialog::new(id, over, width, &title);
    let shown = window(ui, over, id, dialog, wizard, page, |ui| {
        let h = (LANGS.len() as f32 * FOLDER_ROW_H).min(ui.available_height());
        let list = Table::new(id.with("list"), &[], LANGS.len()).headless().row_height(FOLDER_ROW_H).height(h).cursor(Some(sel)).content(id);
        let shown = list.show(ui, pal, |ui, i, row| {
            let on = i == current;
            let mid = row.rect.center().y;
            let lamp = if on { pal.accent } else { pal.unlit };
            studio::led(ui, pos2(row.rect.min.x + MARK_IN, studio::led_mid(ui, mid, &body(14.0), false)), lamp);
            let ink = if on || row.selected || row.hovered { pal.text } else { pal.soft };
            let x = row.rect.min.x + WORDS_IN;
            studio::text(ui, pos2(x, mid), LANGS[i].1, body(14.0), ink, (row.rect.max.x - x - WORDS_AFTER).max(1.0), Align2::LEFT_CENTER);
        });
        shown.row
    });
    let mut acts = Vec::new();
    if shown.closed {
        acts.push(Act::PathCancel);
    }
    acts.extend(shown.inner.map(Act::SetLanguage));
    Drawn { acts, caret: None }
}

/// The type-a-path window (`Modal::PathEntry`, its draft in it), after the
/// canvas's FoldersPath board, as the admin Libraries room draws its own:
/// the field, the draft's text and caret in the monospace, which has the
/// keyboard from the start (its caret is the one this view notes, and the
/// input method's composition stands at it); under it the suggestions, the
/// listing filtered by what follows the text's last separator, dim, six at
/// once, the keys' pick ringed and kept in view, a new text's list from its
/// top; or, where the folder the text is in could not be listed, the
/// wizard's words why in gold in the list's place. The window holds its top
/// where it would stand with all six, so the title and the field never move
/// as the list grows and shrinks under them, as the cells anchor theirs. A
/// click on a suggestion takes it into the text, as the cells' click does
/// (Tab's taking, which adds nothing: Enter adds the text as typed, and
/// nothing here does). The close is the cells'.
pub(super) fn path(ui: &mut Ui, over: Rect, wizard: &mut Wizard, page: &Page) -> Drawn {
    let Modal::PathEntry(draft) = &wizard.modal else { return Drawn::default() };
    let pal = &page.pal;
    let id = Id::new("setup-path-entry");
    let most = FIELD_H + 12.0 + SUGGEST_ROWS as f32 * FOLDER_ROW_H;
    let title = t!("path_modal.title");
    let dialog = Dialog::new(id, over, BROWSER_W, &title).hold(most);
    let shown = window(ui, over, id, dialog, wizard, page, |ui| {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, FIELD_H), Sense::hover());
        let input = studio::Input::of(&draft.text).focused(true, page.composition).mono();
        let typed = studio::field_box(ui, id.with("field"), rect, &input, pal);
        let count = draft.suggestion_count();
        let mut picked = None;
        if let Some(err) = &draft.error {
            ui.add_space(12.0);
            studio::line(ui, FOLDER_ROW_H, err, body(13.0), pal.gold);
        } else if count > 0 {
            ui.add_space(12.0);
            let h = (count.min(SUGGEST_ROWS) as f32 * FOLDER_ROW_H).min(ui.available_height()).max(FOLDER_ROW_H);
            // The suggestions are the listing filtered: walked once a pass
            // for the rows on screen, never collected whole (a listing can
            // run to thousands, as the cells' draw keeps in mind).
            let (mut from, mut near): (usize, Vec<&str>) = (0, Vec::new());
            let list = Table::new(id.with("list"), &[], count)
                .headless()
                .row_height(FOLDER_ROW_H)
                .height(h)
                .cursor(draft.sel)
                .content(Id::new(draft.text.value()));
            let shown = list.show(ui, pal, |ui, i, row| {
                if i < from || i >= from + near.len() {
                    from = i;
                    near = draft.suggestions().skip(i).take(4 * SUGGEST_ROWS).collect();
                }
                // Dim, the pointer's brighter, as the cells draw them.
                if let Some(entry) = near.get(i - from) {
                    studio::folder_row(ui, row.rect, entry, row.selected, if row.hovered { pal.text } else { pal.dim }, pal);
                }
            });
            picked = shown.row;
        }
        (typed.caret, picked)
    });
    let mut acts = Vec::new();
    if shown.closed {
        acts.push(Act::PathCancel);
    }
    let (caret, picked) = shown.inner;
    acts.extend(picked.map(Act::PathSuggest));
    Drawn { acts, caret }
}

/// The folder browser (`Modal::Browser`, its listing in it), the wizard's
/// own where no system dialog could open, after the canvas's FoldersBrowse
/// board, as the admin Libraries room draws its own: the kit's browser, the
/// folder listed cut at its start so its leaf stays, its folders under it,
/// nine at once, the keys' cursor ringed and kept in view, a new listing
/// from its top, an empty folder saying so; then the cells' four acts in
/// their order from the left, up, open, the add (lit) and close. A click on
/// a folder moves the cursor to it and opens nothing, as in the cells; the
/// add takes the folder listed, not the cursor's. The title says the
/// server's folders, as the cells' does, though the listing is this
/// machine's (the code's, left for the owner to rule on).
pub(super) fn browser(ui: &mut Ui, over: Rect, wizard: &mut Wizard, page: &Page) -> Drawn {
    let Modal::Browser(browse) = &wizard.modal else { return Drawn::default() };
    let pal = &page.pal;
    let id = Id::new("setup-browser");
    let title = t!("browse.title");
    let dialog = Dialog::new(id, over, BROWSER_W, &title);
    let shown = window(ui, over, id, dialog, wizard, page, |ui| {
        // The kit's browser takes as many rows as the window has room for,
        // up to its eleven: the room it is given is nine's at most.
        let most = BROWSE_ROUND + BROWSE_ROWS as f32 * FOLDER_ROW_H + FORM_BUTTON_H;
        ui.set_max_height(most.min(ui.available_height()));
        let picked = studio::folder_browser(ui, id.with("list"), &browse.path, &browse.dirs, browse.sel, &t!("browse.empty"), pal);
        let (up, open, add, close) = (t!("browse.up"), t!("browse.open"), t!("browse.add"), t!("browse.close"));
        let start = [(up.as_ref(), false), (open.as_ref(), false), (add.as_ref(), true), (close.as_ref(), false)];
        let meant = [Act::BrowseUp, Act::BrowseEnter, Act::BrowseAdd, Act::BrowseCancel];
        studio::foot(ui, id.with("foot"), &start, &[], None, pal).map(|k| meant[k].clone()).or(picked.map(Act::BrowseRow))
    });
    let mut acts = Vec::new();
    if shown.closed {
        acts.push(Act::BrowseCancel);
    }
    acts.extend(shown.inner);
    Drawn { acts, caret: None }
}

/// The modal `id`'s window, `dialog` over `over` with `body` in it: the
/// cells' tooltip on its close ("Close — Esc", the key named where the
/// wizard names its keys), and the frame's lines lifted over its scrim
/// where the window leaves them clear ([`lift_lines`]).
fn window<R>(ui: &Ui, over: Rect, id: Id, dialog: Dialog<'_>, wizard: &Wizard, page: &Page, body: impl FnOnce(&mut Ui) -> R) -> ModalShown<R> {
    let shown = dialog.show(ui, &page.pal, |ui| {
        let inner = body(ui);
        // The window's foot: the padding's under the body's, or under its
        // view's where the body is taller and scrolls.
        let foot = ui.min_rect().max.y.min(ui.clip_rect().max.y) + WINDOW_PAD;
        (inner, foot)
    });
    if let Some(response) = ui.ctx().read_response(id.with("close")) {
        tip_keyed(page, &response, &t!("path_modal.tip_close"));
    }
    let (inner, foot) = shown.inner;
    lift_lines(ui, id, over, foot, wizard, page);
    ModalShown { closed: shown.closed, inner }
}

/// The frame's status line and tips line drawn again over the scrim of the
/// modal `modal`, lit, as every board stands them over every modal's scrim
/// and as the cells leave them beside theirs: the modal's hint, what the
/// wizard is busy with (a listing being read), its note (a refusal, the
/// machine translation's). Only where the window, its foot at `foot`,
/// leaves them clear: where it reaches them (a short page), they stay under
/// the scrim where the frame drew them, never over the window. Drawn in a
/// layer of their own that egui keeps just over the modal's, which takes no
/// pointer.
pub(super) fn lift_lines(ui: &Ui, modal: Id, over: Rect, foot: f32, wizard: &Wizard, page: &Page) {
    let at = parts(over);
    if foot > at.status.min.y {
        return;
    }
    let lines = Id::new("setup-modal-lines");
    ui.ctx().set_sublayer(LayerId::new(Order::Foreground, modal), LayerId::new(Order::Foreground, lines));
    let area = egui::Area::new(lines).order(Order::Foreground).fixed_pos(over.min).constrain(false).fade_in(false).interactable(false);
    let _ = area.show(ui.ctx(), |ui| {
        status_line(ui, at.status, wizard, &page.pal);
        tips_line(ui, at.tips, wizard, &page.pal);
    });
}

#[cfg(test)]
mod tests {
    use egui::epaint::Shape;
    use egui::{Color32, Event, FullOutput, Id, Pos2, Rect, pos2, vec2};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rust_i18n::t;

    use super::super::FLOOR;
    use super::super::tests::{BOARD, Probe, color_of, drawn, has, tipped, wizard};
    use crate::api::types::{DirEntry, DirListing};
    use crate::gui::window::studio::Pal;
    use crate::setup::cells::footer_hint;
    use crate::setup::tests::{LOCALE_LOCK, in_own_process};
    use crate::setup::{Act, Done, LANGS, Modal, Op, PathDraft, Wizard, handle_key};

    /// The three windows' ids, as the frame's test names them.
    const LIST: &str = "setup-language-list";
    const PATH: &str = "setup-path-entry";
    const BROWSE: &str = "setup-browser";

    fn key(wizard: &mut Wizard, code: KeyCode) {
        handle_key(wizard, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_text(wizard: &mut Wizard, s: &str) {
        for c in s.chars() {
            key(wizard, KeyCode::Char(c));
        }
    }

    fn listing(path: &str, names: &[&str]) -> DirListing {
        DirListing { path: path.into(), directories: names.iter().map(|n| DirEntry { name: (*n).into() }).collect(), files: Vec::new() }
    }

    /// Row `i` of the window `modal`'s list, as the kit's table numbers
    /// them (a bare literal would hash as another integer type).
    fn row(modal: &str, i: usize) -> Id {
        Id::new(modal).with("list").with("row").with(i)
    }

    /// Button `k` of the browser's foot, counted from the left.
    fn foot(k: usize) -> Id {
        Id::new(BROWSE).with("foot").with(k)
    }

    /// The colour `words` were drawn in inside `rect`, where the pass drew
    /// them there: the list's own, not the chip's.
    fn color_in(out: &FullOutput, words: &str, rect: Rect) -> Option<Color32> {
        out.shapes.iter().find_map(|clipped| match &clipped.shape {
            Shape::Text(text) if text.galley.text() == words && rect.contains(Rect::from_min_size(text.pos, text.galley.size()).center()) => {
                text.galley.job.sections.first().map(|s| s.format.color)
            }
            _ => None,
        })
    }

    /// The fill of the LED drawn in `rect`, a 6-point square.
    fn led_in(out: &FullOutput, rect: Rect) -> Option<Color32> {
        out.shapes.iter().find_map(|clipped| match &clipped.shape {
            Shape::Rect(r) if (r.rect.width() - 6.0).abs() < 0.01 && (r.rect.height() - 6.0).abs() < 0.01 && rect.contains(r.rect.center()) => Some(r.fill),
            _ => None,
        })
    }

    /// Whether `words` were drawn over the scrim the page `size` lies under:
    /// their last drawing comes after it.
    fn lifted(out: &FullOutput, size: egui::Vec2, words: &str) -> bool {
        let page = Rect::from_min_size(Pos2::ZERO, size);
        let at = |hit: &dyn Fn(&Shape) -> bool| out.shapes.iter().rposition(|c| hit(&c.shape));
        let scrim = at(&|s| matches!(s, Shape::Rect(r) if r.rect == page && r.fill.a() > 0 && r.fill.a() < 255));
        let words = at(&|s| matches!(s, Shape::Text(t) if t.galley.text() == words));
        matches!((scrim, words), (Some(scrim), Some(words)) if words > scrim)
    }

    #[test]
    fn the_path_window_draws_the_draft_its_caret_and_its_suggestions_and_a_click_takes_one() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let mut probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = wizard();
        let _ = probe.settle(&mut wizard);
        key(&mut wizard, KeyCode::Char('t'));
        assert!(matches!(wizard.modal, Modal::PathEntry(_)), "t opens it");
        let path = Id::new(PATH);

        // Nothing typed: the field alone, with the keyboard.
        let (out, shown) = probe.settle(&mut wizard);
        let (title, hint) = (t!("path_modal.title").to_string(), t!("hint.path").to_string());
        for w in [title.as_str(), hint.as_str()] {
            assert!(has(&drawn(&out), w), "{w} drawn: {:?}", drawn(&out));
        }
        let field = probe.rect(path.with("field"));
        let caret = shown.caret.expect("the path's field has the keyboard");
        assert!(field.contains(caret - vec2(0.0, 1.0)), "the caret's foot is in the field: {caret:?} {field:?}");
        let close = probe.rect(path.with("close"));

        // Typed, and the listing in: what the partial matches, dim.
        type_text(&mut wizard, "/srv/m");
        assert_eq!(wizard.queued, Some(Op::Complete("/srv/".into())), "the keys are the model's");
        wizard.queued = None;
        wizard.apply(Done::Completed { dir: "/srv/".into(), listing: Ok(listing("/srv", &["media", "music", "www"])) });
        let (out, shown) = probe.settle(&mut wizard);
        let words = drawn(&out);
        for w in ["/srv/m", "media", "music"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(!has(&words, "www"), "only what the partial matches");
        assert_eq!(color_of(&out, "media"), Some(pal.dim));
        assert!(shown.caret.expect("still the field's").x > caret.x, "the caret after the text");
        assert_eq!(probe.rect(path.with("close")), close, "held at its top while the list grows under the field");

        // ↓ picks, and the pick is drawn in the text colour.
        key(&mut wizard, KeyCode::Down);
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!((color_of(&out, "media"), color_of(&out, "music")), (Some(pal.text), Some(pal.dim)), "the keys' pick");

        // A click takes its row into the text, as the cells' click: that
        // folder listed next, nothing added.
        let shown = probe.click(&mut wizard, row(PATH, 1));
        assert!(shown.outcome.is_none(), "no click here ends the wizard");
        let Modal::PathEntry(draft) = &wizard.modal else { panic!("the path window stays up") };
        assert_eq!(draft.text.value(), "/srv/music/");
        assert_eq!(wizard.queued, Some(Op::Complete("/srv/music/".into())));
        assert!(wizard.folders.is_empty(), "a click adds nothing: Enter does");
        wizard.queued = None;

        // The input method's text stands at the caret.
        probe.composition = "かな".into();
        let (out, _) = probe.settle(&mut wizard);
        assert!(has(&drawn(&out), "/srv/music/かな"), "{:?}", drawn(&out));
        probe.composition.clear();

        // The folder the text is in refused: the wizard's words in gold,
        // where the list was.
        wizard.apply(Done::Completed { dir: "/srv/music/".into(), listing: Err("EACCES".into()) });
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!(color_of(&out, "could not list /srv/music/: EACCES"), Some(pal.gold), "{:?}", drawn(&out));

        // The close is Esc's: nothing added, and the keyboard goes with the
        // window on the pass it closes.
        let shown = probe.click(&mut wizard, path.with("close"));
        assert_eq!(wizard.modal, Modal::None);
        assert!(wizard.folders.is_empty() && shown.caret.is_none() && shown.outcome.is_none());
        assert!(probe.settle(&mut wizard).1.caret.is_none(), "no field, no caret");
    }

    #[test]
    fn the_paths_suggestions_show_six_at_once_and_the_keys_pick_is_kept_in_view() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = wizard();
        let entries: Vec<String> = (0..40).map(|i| format!("d{i:02}")).collect();
        let text = "/srv/".into();
        let draft = PathDraft { text, listed_for: "/srv/".into(), listed_path: "/srv".into(), entries, ..PathDraft::default() };
        wizard.modal = Modal::PathEntry(draft);
        let _ = probe.settle(&mut wizard);
        let first = probe.rect(row(PATH, 0));
        assert!(probe.ctx.read_response(row(PATH, 10)).is_none(), "six at once, not forty");
        let sixth = probe.rect(row(PATH, 5));
        assert!((sixth.max.y - first.min.y - 6.0 * 30.0).abs() < 0.5, "six rows of 30: {first:?} {sixth:?}");
        // ↑ wraps to the last, which is brought into view.
        key(&mut wizard, KeyCode::Up);
        assert!(matches!(&wizard.modal, Modal::PathEntry(d) if d.sel == Some(39)));
        let _ = probe.settle(&mut wizard);
        let last = probe.rect(row(PATH, 39));
        assert!(last.min.y >= first.min.y - 0.5 && last.max.y <= first.min.y + 180.5, "whole in the list: {last:?}");
        assert!(probe.ctx.read_response(row(PATH, 0)).is_none(), "the list moved to it");
    }

    #[test]
    fn the_browser_draws_the_listing_and_each_click_does_the_cells_act() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = wizard();
        let home = || Done::Browsed(Ok(listing("/home/anna", &["Audiobooks", "Downloads", "Music", "Videos"])));
        wizard.apply(home());
        let (out, shown) = probe.settle(&mut wizard);
        let words = drawn(&out);
        let (title, hint) = (t!("browse.title").to_string(), footer_hint(&wizard));
        for w in [title.as_str(), "/home/anna", "Audiobooks", "Videos", "UP", "OPEN", "ADD THIS FOLDER", "CLOSE", hint.as_str()] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(shown.caret.is_none(), "no field has the keyboard");

        // A click on a folder moves the cursor to it and opens nothing.
        let shown = probe.click(&mut wizard, row(BROWSE, 2));
        assert!(matches!(&wizard.modal, Modal::Browser(b) if b.sel == 2));
        assert!(wizard.queued.is_none() && shown.outcome.is_none());
        // Open lists the cursor's folder, up the parent, each busy while it reads.
        probe.click(&mut wizard, foot(1));
        assert_eq!(wizard.queued, Some(Op::BrowseTo("/home/anna/Music".into())), "open");
        assert_eq!(wizard.busy.as_deref(), Some(t!("busy.listing").as_ref()));
        (wizard.queued, wizard.busy) = (None, None);
        probe.click(&mut wizard, foot(0));
        assert_eq!(wizard.queued, Some(Op::BrowseTo("/home".into())), "up");
        (wizard.queued, wizard.busy) = (None, None);
        // The add takes the folder listed, not the cursor's, and closes.
        probe.click(&mut wizard, foot(2));
        assert_eq!(wizard.modal, Modal::None);
        assert_eq!(wizard.folders.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["/home/anna"]);

        // The foot's close and the window's are Esc's: nothing added.
        for close in [foot(3), Id::new(BROWSE).with("close")] {
            wizard.apply(home());
            let _ = probe.settle(&mut wizard);
            let shown = probe.click(&mut wizard, close);
            assert_eq!(wizard.modal, Modal::None, "{close:?}");
            assert!(shown.outcome.is_none() && wizard.folders.len() == 1);
        }

        // An empty folder says so.
        wizard.apply(Done::Browsed(Ok(listing("/srv/empty", &[]))));
        let (out, _) = probe.settle(&mut wizard);
        assert!(has(&drawn(&out), &t!("browse.empty")), "{:?}", drawn(&out));
    }

    #[test]
    fn the_browser_shows_nine_folders_at_once_and_ends_clear_of_the_lines_it_leaves_lit() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = wizard();
        let names: Vec<String> = (0..40).map(|i| format!("d{i:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        wizard.apply(Done::Browsed(Ok(listing("/srv", &names))));
        let _ = probe.settle(&mut wizard);
        let first = probe.rect(row(BROWSE, 0));
        let ninth = probe.rect(row(BROWSE, 8));
        assert!((ninth.max.y - first.min.y - 9.0 * 30.0).abs() < 0.5, "nine rows of 30: {first:?} {ninth:?}");
        assert!(probe.ctx.read_response(row(BROWSE, 12)).is_none(), "nine at once, not the kit's eleven");
        // The board's window, y 178 to 622 (its edge outside where the
        // kit's is inside): its foot 18 points over the status line at 640.
        let buttons = probe.rect(foot(0));
        assert!((buttons.max.y + 24.0 - 621.0).abs() < 1.0, "the window's foot: {buttons:?}");
        assert!(ninth.max.y + 20.0 <= buttons.min.y + 0.5, "the list ends 20 over the foot");
        // The keys' cursor is kept in view.
        for _ in 0..20 {
            key(&mut wizard, KeyCode::Down);
        }
        let _ = probe.settle(&mut wizard);
        let cursor = probe.rect(row(BROWSE, 20));
        assert!(cursor.max.y <= buttons.min.y - 20.0 + 0.5, "whole in the list: {cursor:?}");
    }

    #[test]
    fn the_language_list_names_every_language_in_its_own_words_and_the_cursor_is_the_wizards() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = wizard();
        let _ = probe.settle(&mut wizard);
        // The chip opens it, on the current language.
        probe.click(&mut wizard, Id::new("setup-language"));
        assert_eq!(wizard.modal, Modal::Language(0));
        let (out, shown) = probe.settle(&mut wizard);
        let (title, hint) = (t!("lang.modal_title").to_string(), t!("lang.hint").to_string());
        for w in [title.as_str(), hint.as_str()] {
            assert!(has(&drawn(&out), w), "{w} drawn: {:?}", drawn(&out));
        }
        assert!(shown.caret.is_none(), "no field has the keyboard");
        // Every language by its own name, whole: none cut, the CJK ones in
        // their own face; the current one's LED lit and the rest unlit; its
        // name, the cursor's, in the text colour and the others a step under.
        for (i, (_, name)) in LANGS.iter().enumerate() {
            let at = probe.rect(row(LIST, i));
            let galley = out.shapes.iter().find_map(|c| match &c.shape {
                Shape::Text(text) if text.galley.text() == *name && at.contains(text.pos + text.galley.size() / 2.0) => Some(text.galley.clone()),
                _ => None,
            });
            let galley = galley.unwrap_or_else(|| panic!("{name} drawn in its row"));
            assert!(!galley.elided && galley.size().x <= at.width() - 36.0, "{name} whole: {:?}", galley.size());
            let (ink, lamp) = if i == 0 { (pal.text, pal.accent) } else { (pal.soft, pal.unlit) };
            assert_eq!((color_in(&out, name, at), led_in(&out, at)), (Some(ink), Some(lamp)), "{name}");
        }
        assert!(probe.rect(row(LIST, 0)).width() >= 300.0 - 48.0 - 0.5, "the board's 300 at the least");

        // The keys move the cursor, the model's; the list follows it.
        key(&mut wizard, KeyCode::Down);
        key(&mut wizard, KeyCode::Down);
        assert_eq!(wizard.modal, Modal::Language(2));
        let (out, _) = probe.settle(&mut wizard);
        let (english, spanish, german) = (probe.rect(row(LIST, 0)), probe.rect(row(LIST, 2)), probe.rect(row(LIST, 1)));
        assert_eq!(color_in(&out, "Español", spanish), Some(pal.text), "the cursor's");
        assert_eq!(color_in(&out, "English", english), Some(pal.text), "the current one's");
        assert_eq!(color_in(&out, "Deutsch", german), Some(pal.soft));

        // The close carries the cells' tooltip, and is Esc's: nothing changes.
        let close = probe.rect(Id::new(LIST).with("close")).center();
        let _ = probe.pass_after(&mut wizard, vec![Event::PointerMoved(pos2(4.0, 4.0))], 0.2);
        assert!(has(&drawn(&probe.rest(&mut wizard, close, 0.6)), &tipped(&t!("path_modal.tip_close"))));
        let shown = probe.click(&mut wizard, Id::new(LIST).with("close"));
        assert_eq!((wizard.modal.clone(), wizard.lang), (Modal::None, 0));
        assert!(shown.outcome.is_none() && wizard.note.is_none());
    }

    /// The board's moment: the page in German, the list reopened from the
    /// chip, its words from locales/de.yml. A click on a language switches
    /// the page to it at once and its machine note takes the status line,
    /// as Enter on it does.
    #[test]
    fn in_german_the_list_is_the_boards_and_a_click_on_a_language_switches_the_page_to_it() {
        // It switches the language, so in a process of its own, which
        // starts in German.
        let Some(_) = in_own_process(&["de"]) else { return };
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = wizard();
        wizard.lang = 1;
        wizard.note = Some((t!("lang.machine_note").to_string(), false));
        let _ = probe.settle(&mut wizard);
        probe.click(&mut wizard, Id::new("setup-language"));
        assert_eq!(wizard.modal, Modal::Language(1), "on Deutsch");
        let (out, _) = probe.settle(&mut wizard);
        let note = "Diese Übersetzung ist maschinell erstellt — kleine Fehler sind möglich.";
        let hint = "↑ ↓ wählen · Enter übernehmen · Esc schließen";
        for w in ["Sprache", "Deutsch", "日本語", "中文", note, hint] {
            assert!(has(&drawn(&out), w), "{w} drawn: {:?}", drawn(&out));
        }
        // The note and the hint stand lit over the scrim.
        assert!(lifted(&out, BOARD, note) && lifted(&out, BOARD, hint));
        let german = probe.rect(row(LIST, 1));
        assert_eq!((color_in(&out, "Deutsch", german), led_in(&out, german)), (Some(pal.text), Some(pal.accent)));
        let close = probe.rect(Id::new(LIST).with("close")).center();
        let _ = probe.pass_after(&mut wizard, vec![Event::PointerMoved(pos2(4.0, 4.0))], 0.2);
        assert!(has(&drawn(&probe.rest(&mut wizard, close, 0.6)), &tipped("Schließen — Esc")));

        // Français, clicked: chosen as Enter chooses it.
        let shown = probe.click(&mut wizard, row(LIST, 3));
        assert!(shown.outcome.is_none());
        assert_eq!((wizard.lang, wizard.modal.clone()), (3, Modal::None));
        assert_eq!(rust_i18n::locale().to_string(), "fr");
        let (out, _) = probe.settle(&mut wizard);
        let (french, card) = (t!("lang.machine_note").to_string(), t!("folders.add_card").to_string());
        assert_ne!(french, note);
        for w in [french.as_str(), "Français", card.as_str()] {
            assert!(has(&drawn(&out), w), "{w} drawn: {:?}", drawn(&out));
        }
    }

    /// Over every modal of the three, the status line and the tips line
    /// stand lit over the scrim, as the boards draw them, where the window
    /// leaves them clear; where it reaches them, on a page at the cells'
    /// floor, they stay under it, where the frame drew them.
    #[test]
    fn the_status_and_tips_lines_stand_over_the_scrim_where_the_window_leaves_them_clear() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        type Open = fn(&mut Wizard);
        let modals: [(&str, Open); 3] = [
            (LIST, |w| {
                let _ = w.act(Act::OpenLanguage);
            }),
            (PATH, |w| {
                let _ = w.act(Act::TypePath);
            }),
            (BROWSE, |w| w.apply(Done::Browsed(Ok(listing("/home/anna", &["Music"]))))),
        ];
        for (size, clear) in [(BOARD, [true, true, true]), (FLOOR, [false, true, false])] {
            let probe = Probe::new(size);
            for ((name, open), clear) in modals.iter().zip(clear) {
                let mut wizard = wizard();
                open(&mut wizard);
                wizard.busy = Some(t!("busy.listing").to_string());
                let (out, _) = probe.settle(&mut wizard);
                let (hint, busy) = (footer_hint(&wizard), t!("busy.listing").to_string());
                assert!(has(&drawn(&out), &hint), "{name} at {size:?}: the hint drawn");
                assert_eq!(lifted(&out, size, &hint), clear, "{name} at {size:?}: the hint");
                assert_eq!(lifted(&out, size, &busy), clear, "{name} at {size:?}: the busy line");
                let close = probe.rect(Id::new(*name).with("close"));
                let tip = tipped(&t!("path_modal.tip_close"));
                assert!(has(&drawn(&probe.rest(&mut wizard, close.center(), 0.6)), &tip), "{name}: the close's tip");
            }
        }
    }
}
