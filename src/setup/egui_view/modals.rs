//! The modals over the wizard's page drawn by egui: the language list, the
//! type-a-path window and the folder browser, each a window over the kit's
//! scrim (`studio::Dialog`), the page beneath inert (the frame drops what
//! was clicked under it). The skip gate is login.rs's.
//!
//! Still to be drawn, after the canvas's Language, FoldersPath and
//! FoldersBrowse boards: each is a placeholder that shows its modal's title,
//! in the cells' words for it, and its close, which does what the cells'
//! [X] does, with the cells' tooltip. The keys answer every one as they
//! always have. Each takes the root `ui`, `over` (the whole page, the
//! scrim's reach), the wizard and the frame's [`Page`], and hands back what
//! its clicks asked for and the caret of its field with the keyboard.

use egui::{Id, Rect, Ui};
use rust_i18n::t;

use super::super::{Act, Wizard};
use super::{Drawn, Page, tip_keyed};
use crate::gui::window::studio::{BROWSER_W, Dialog};

/// The language list's window until it is drawn.
const LIST_W: f32 = 360.0;

/// The language list (`Modal::Language`, its cursor in it). Still to be
/// drawn: its title, and its close, which is the cells' (the path modal's
/// cancel, which closes whatever modal is up).
pub(super) fn language(ui: &mut Ui, over: Rect, _wizard: &mut Wizard, page: &Page) -> Drawn {
    placeholder(ui, Id::new("setup-language-list"), over, LIST_W, &t!("lang.modal_title"), Act::PathCancel, page)
}

/// The type-a-path window (`Modal::PathEntry`, its draft in it). Still to
/// be drawn: its title and its close. Its field, which has the keyboard,
/// is not drawn yet, so it notes no caret.
pub(super) fn path(ui: &mut Ui, over: Rect, _wizard: &mut Wizard, page: &Page) -> Drawn {
    placeholder(ui, Id::new("setup-path-entry"), over, BROWSER_W, &t!("path_modal.title"), Act::PathCancel, page)
}

/// The folder browser (`Modal::Browser`, its listing in it). Still to be
/// drawn: its title and its close.
pub(super) fn browser(ui: &mut Ui, over: Rect, _wizard: &mut Wizard, page: &Page) -> Drawn {
    placeholder(ui, Id::new("setup-browser"), over, BROWSER_W, &t!("browse.title"), Act::BrowseCancel, page)
}

/// A modal's window `width` wide over `over` under `id`, with `title` and
/// a close that asks for `close`, the cells' close tooltip on it, and no
/// body yet.
fn placeholder(ui: &Ui, id: Id, over: Rect, width: f32, title: &str, close: Act, page: &Page) -> Drawn {
    let shown = Dialog::new(id, over, width, title).show(ui, &page.pal, |_| {});
    if let Some(response) = ui.ctx().read_response(id.with("close")) {
        tip_keyed(page, &response, &t!("path_modal.tip_close"));
    }
    Drawn { acts: if shown.closed { vec![close] } else { Vec::new() }, caret: None }
}
