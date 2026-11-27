//! The top bar's strip, row 0 of every screen but the mini player: the
//! four tabs (Library, Stats, Admin, MP3 Player) with a sync's mark on
//! the last, the Visualizer item, and at the right the server label, a
//! menu's trigger while two servers or more can be picked, and the [+]
//! that adds one. Both its faces draw from one [`StripView`]: the cells
//! face here, moved whole from the shell and the servers module, and the
//! window's egui face (`egui_view`).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::config;
use crate::kit::dim;
use crate::kit::theme::{legacy_conhost, th};

use super::{Act, Gui, Screen, bright_bold, device, put, sel, vizwin};

/// The strip drawn by egui in the GUI's own window (src/gui/chrome.rs).
#[cfg(feature = "window")]
pub(crate) mod egui_view;

/// What the top bar's strip holds this frame, for either face to draw.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StripView {
    /// Library, Stats, Admin, MP3 Player: each opens `Act::Screen(screen)`.
    pub tabs: [StripTab; 4],
    /// The Visualizer item: `Act::VizWindow`.
    pub viz: StripViz,
    /// A sync's percentage after the MP3 Player tab's name
    /// (`device::sync_mark`).
    pub mark: Option<SyncMark>,
    /// The tabs and the Visualizer take a press: no MP3 write runs
    /// (`device::writing`), which holds the tab.
    pub live: bool,
    /// The server label, absent while no server is named at all.
    pub server: Option<StripServer>,
    /// [+]'s tooltip, a plain one; [+] is `Act::SrvAdd`.
    pub add_tip: String,
    /// Whether tooltips keep their " — key" tails (`Surface::key_hints`).
    pub key_hints: bool,
}

/// A tab: the screen it opens, its name, and whether that screen is up.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StripTab {
    pub screen: Screen,
    pub label: String,
    pub up: bool,
}

/// The Visualizer item: its name, its tooltip whole with its key's tail
/// (cut where key hints are off, as `Surface::tip_keyed` cuts it), and
/// whether the window it opens is open.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StripViz {
    pub label: String,
    pub tip: String,
    pub lit: bool,
}

/// A sync's percentage, and whether the sync waits paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SyncMark {
    pub pct: u8,
    pub paused: bool,
}

/// The server's name as `App::server_display` gives it; `menu` while two
/// servers or more can be picked (contract clause 12), when the label is
/// `Act::SrvMenu`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StripServer {
    pub name: String,
    pub menu: bool,
}

/// The strip as the Gui stands: the words in the language on screen.
pub(crate) fn view(gui: &Gui) -> StripView {
    let tab = |screen: Screen, label: String| StripTab { screen, label, up: gui.screen == screen };
    let tabs = [
        tab(Screen::Library, t!("gui.top.library").to_string()),
        tab(Screen::Stats, t!("sta.title").to_string()),
        tab(Screen::Admin, t!("gui.top.admin").to_string()),
        tab(Screen::Device, t!("gui.top.mp3").to_string()),
    ];
    let viz = StripViz { label: t!("gui.top.viz").to_string(), tip: t!("gui.top.viz_tip").to_string(), lit: vizwin::is_open(gui) };
    let mark = device::sync_mark(gui).map(|(_, pct, paused)| SyncMark { pct, paused });
    // The label names the session's server whether or not it answered: a
    // server that would not connect keeps its place here, and the picker
    // with it, so there is always a way to another (contract clause 13).
    let named = !gui.app.session.server_id.is_empty() || !gui.app.session.server.is_empty();
    // A menu only while more than one server can be picked: parked and
    // missing peers do not count (contract clause 12).
    let server = named.then(|| StripServer {
        name: gui.app.server_display(),
        menu: gui.config.servers.iter().filter(|e| config::selectable(e)).count() > 1,
    });
    StripView {
        tabs,
        viz,
        mark,
        live: !device::writing(gui),
        server,
        add_tip: t!("gui.srv.add").to_string(),
        key_hints: gui.ui.key_hints,
    }
}

impl SyncMark {
    /// The mark's words, as the cells set them after the tab's name and
    /// measure the strip with them (`device::strip_mark`).
    pub(crate) fn words(&self) -> String {
        format!("{}%", self.pct)
    }
}

/// The strip in cells: the tabs and the Visualizer item, then the server
/// label and [+], drawn from the strip's view.
pub(super) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let view = view(gui);
    draw_top_tabs(frame, gui, &view);
    draw_header(frame, gui, area, &view);
}

/// The top bar's tabs at the left — Library, Stats, Admin, MP3 Player: the
/// kit's tab slab for the screen that is up, dim text for the others,
/// bright under the pointer. Now Playing has no tab (it opens on `0`), so
/// while it is up no tab wears the slab. While the MP3 Player tab's page
/// writes, the strip is drawn inert — nothing lights, nothing answers a
/// click — since the tab cannot be left (mp3-player-screen contract,
/// clause 9).
fn draw_top_tabs(frame: &mut Frame, gui: &mut Gui, view: &StripView) {
    // While a sync runs the MP3 Player tab wears its percentage after its
    // name, in the accent, dim while paused (mp3-player-screen contract,
    // clause 16; the pick of 2026-10-19 over a chip at the bar's right).
    let mp3 = match &view.mark {
        Some(mark) => format!("{} {}", view.tabs[3].label, mark.words()),
        None => view.tabs[3].label.clone(),
    };
    let viz = &view.viz.label;
    let rects = top_rects([&view.tabs[0].label, &view.tabs[1].label, &view.tabs[2].label, &mp3, viz]);
    let live = view.live;
    for (tab, rect) in view.tabs.iter().zip(rects) {
        let label = if tab.screen == Screen::Device { &mp3 } else { &tab.label };
        let style = if tab.up {
            sel().add_modifier(Modifier::BOLD)
        } else if live && gui.ui.hovers(rect) {
            bright_bold()
        } else {
            dim()
        };
        put(frame, rect.x, 0, &format!(" {label} "), style);
        if let Some(mark) = view.mark.as_ref().filter(|_| tab.screen == Screen::Device && !tab.up) {
            let words = mark.words();
            let x = rect.right().saturating_sub(crate::kit::width(&words) as u16 + 1);
            let tone = if mark.paused { dim() } else { Style::default().fg(th().accent) };
            put(frame, x, 0, &words, tone);
        }
        if live {
            gui.ui.click(rect, Act::Screen(tab.screen));
        }
    }
    // The Visualizer item: not a screen but a window (visualizer-window
    // contract, entry 1) — lit while one is open, bright under the pointer.
    let rect = rects[4];
    let style = if view.viz.lit {
        Style::default().fg(th().accent).add_modifier(Modifier::BOLD)
    } else if live && gui.ui.hovers(rect) {
        bright_bold()
    } else {
        dim()
    };
    put(frame, rect.x, 0, &format!(" {viz} "), style);
    if live {
        gui.ui.click(rect, Act::VizWindow);
        gui.ui.tip_keyed(rect, view.viz.tip.clone());
    }
}

/// Where the top bar's items stand, left to right from x 1 with a cell
/// between them: each its label's cells and a space either side. Cells,
/// not characters: a Japanese or Chinese label is two cells a character,
/// and counting characters left the Admin tab's right half dead under the
/// Visualizer item's rect.
pub(super) fn top_rects<const N: usize>(labels: [&str; N]) -> [Rect; N] {
    let mut x = 1;
    labels.map(|label| {
        let rect = Rect { x, y: 0, width: crate::kit::width(label) as u16 + 2, height: 1 };
        x += rect.width + 1;
        rect
    })
}

/// The top bar's labels, left to right: the four tabs, then the
/// Visualizer item.
pub(super) const TOP_KEYS: [&str; 5] =
    ["gui.top.library", "sta.title", "gui.top.admin", "gui.top.mp3", "gui.top.viz"];

/// Where the top bar's strip ends — the cell after the Visualizer item —
/// in the language on screen: the header's server label starts a blank
/// cell past it, cut short where it would not (servers::draw_header;
/// mp3-player-screen contract, clause 16).
#[cfg(test)]
pub(super) fn top_strip_end() -> u16 {
    top_strip_end_with("")
}

/// [`top_strip_end`] with the MP3 Player tab wearing `mark` (a sync's
/// percentage, clause 16): the server label's cut measures the strip with
/// it.
pub(super) fn top_strip_end_with(mark: &str) -> u16 {
    let mut labels = TOP_KEYS.map(|key| t!(key).to_string());
    if !mark.is_empty() {
        labels[3] = format!("{} {mark}", labels[3]);
    }
    top_rects([&labels[0], &labels[1], &labels[2], &labels[3], &labels[4]])[4].right()
}

/// The header's right corner: the server label (a dropdown trigger once a
/// second server is saved) and the [+] that adds one.
fn draw_header(frame: &mut Frame, gui: &mut Gui, area: Rect, view: &StripView) {
    let plus = Rect { x: area.width.saturating_sub(5), y: 0, width: 3, height: 1 };
    let plus_hover = gui.ui.hovers(plus);
    put(frame, plus.x, 0, "[+]", if plus_hover { bright_bold() } else { dim() });
    gui.ui.click(plus, Act::SrvAdd);
    gui.ui.tip(plus, view.add_tip.clone());

    let Some(server) = &view.server else { return };
    let many = server.menu;
    let chevron = if legacy_conhost() { " v" } else { " ▾" };
    let mark = if many { chevron } else { "" };
    let strip_end = top_strip_end_with(&device::strip_mark(gui));
    let Some((x, label)) = header_label(&server.name, mark, strip_end, plus.x) else { return };
    let width = crate::kit::width(&label) as u16;
    let rect = Rect { x, y: 0, width, height: 1 };
    let hover = many && gui.ui.hovers(rect);
    put(frame, x, 0, &label, if hover { bright_bold() } else { dim() });
    if many {
        // No dwell tooltip here: it matured right where the dropdown
        // opens and sat on top of the first rows.
        gui.ui.click(rect, Act::SrvMenu);
    }
}

/// Where the header's server label stands, and what it says: right-aligned
/// a blank cell short of `[+]` at `plus`, and starting at least a blank cell
/// past the top bar's strip, which ends at `strip_end` — the label must
/// never cover the strip, whose clicks it would take (mp3-player-screen
/// contract, clause 16). A name longer than the room between is cut short
/// with the kit's mark, the menu `mark` kept after it. Cells, not
/// characters. None when not even two cells of the name fit.
pub(super) fn header_label(
    server: &str,
    mark: &str,
    strip_end: u16,
    plus: u16,
) -> Option<(u16, String)> {
    let room = plus.saturating_sub(1).saturating_sub(strip_end + 1) as usize;
    let fits = room.saturating_sub(crate::kit::width(mark));
    if fits < 2 {
        return None;
    }
    let label = format!("{}{mark}", super::bar::clip(server, fits));
    let width = crate::kit::width(&label) as u16;
    Some((plus.saturating_sub(width + 1), label))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ServerEntry};
    use crate::tui::app::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;

    /// The strip's view says what its cells draw: the four tabs by name
    /// with the screen up wearing the slab, the Visualizer and its keyed
    /// tooltip, [+]'s tooltip, the label once a server is named and its
    /// menu once two can be picked; and each item's cells act as the view
    /// says.
    #[test]
    fn the_strips_view_says_what_its_cells_draw() {
        let _en = crate::setup::tests::in_locale("en");
        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        let shown = view(&gui);
        let names: Vec<&str> = shown.tabs.iter().map(|tab| tab.label.as_str()).collect();
        assert_eq!(names, ["Library", "Stats", "Admin", "MP3 Player"]);
        assert_eq!(shown.tabs.iter().filter(|tab| tab.up).map(|tab| tab.screen).collect::<Vec<_>>(), [Screen::Library]);
        assert_eq!((shown.viz.label.as_str(), shown.viz.lit), ("Visualizer", false));
        assert!(shown.viz.tip.ends_with(" — V"), "keyed whole: {:?}", shown.viz.tip);
        assert_eq!((shown.mark, shown.live, shown.server.clone()), (None, true, None), "no sync, no write, no server named");
        assert_eq!(shown.add_tip, t!("gui.srv.add"));

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| super::super::render(frame, &mut gui)).unwrap();
        let buffer = terminal.backend().buffer();
        let top: String = (0..100).map(|x| buffer[(x, 0)].symbol()).collect();
        let rects = top_rects([&names[0], &names[1], &names[2], &names[3], &shown.viz.label]);
        for (tab, rect) in shown.tabs.iter().zip(rects) {
            assert!(top[usize::from(rect.x)..].starts_with(&format!(" {} ", tab.label)), "{top}");
            assert_eq!(gui.ui.hit(Position { x: rect.x, y: 0 }), Some(Act::Screen(tab.screen)));
        }
        assert_eq!(gui.ui.hit(Position { x: rects[4].x, y: 0 }), Some(Act::VizWindow));

        gui.app.session.server = "http://attic.local:3000".into();
        gui.app.session.server_id = "http://attic.local:3000".into();
        let server = view(&gui).server.expect("named");
        assert!(!server.menu, "one server: no menu");
        for url in ["http://attic.local:3000", "http://office.local:3000"] {
            gui.config.servers.push(ServerEntry { url: url.into(), ..Default::default() });
        }
        let server = view(&gui).server.expect("named");
        assert!(server.menu, "two: a menu");
        terminal.draw(|frame| super::super::render(frame, &mut gui)).unwrap();
        let buffer = terminal.backend().buffer();
        let top: String = (0..100).map(|x| buffer[(x, 0)].symbol()).collect();
        let at = top.find(&server.name).expect("the label");
        assert_eq!(gui.ui.hit(Position { x: top[..at].chars().count() as u16, y: 0 }), Some(Act::SrvMenu), "{top}");
    }
}
