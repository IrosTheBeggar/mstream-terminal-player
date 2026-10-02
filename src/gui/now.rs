//! The Now Playing screen: the TUI's full-screen view, whole, under the
//! GUI's top bar (docs/ux-contracts/now-playing.md) — the facts column
//! with the cover beneath, the tabbed panel (Queue · Lyrics · Discover ·
//! Auto-DJ · Visualizer, as the session offers them), the rule, the
//! mirrored waveform band with the scrubber, the modes on the last row —
//! plus the one thing a pointer surface adds: prev · play · next in the
//! bar's own frames under the cover. The GUI's queue panel and bar stand
//! down here; the view has its own queue tab and its own seek control.
//!
//! The App's `fullscreen` flag is up while the screen is, so the App's
//! full-screen keys mean here what they mean in the TUI: Enter on the
//! queue tab plays, ↑↓ walk the tab's list, `i` picks the Queue tab, and
//! the visualizer asks for its thirty frames a second.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::bar::{TallKind, play_glyphs, tall_compact};
use super::{Act, Gui, Screen, put};
use crate::kit::dim;
use crate::kit::theme::th;
use crate::tui::app::Action;
use crate::tui::ui::{NowExtras, now_regions, render_now_view};

/// What the cover yields to the transport: one row of air, then the
/// three-row frames.
const TRANSPORT_ROWS: u16 = 4;

/// What the screen keeps between frames: where the band and the cover
/// stood — a click on the band maps against the band's width, and the
/// cover draws as the mosaic when an overlay stood over it last frame.
#[derive(Debug, Default)]
pub(crate) struct NowUi {
    pub band: Rect,
    cover: Option<Rect>,
}

impl NowUi {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

/// The view's area: the whole frame under the top bar — a row lower while
/// a pick's banner is up — and short of the footer row while the key hints
/// are on. `now_regions` spends the area's first row on the TUI's title,
/// which is the top bar's (or the banner's) row here.
pub(crate) fn view_rect(area: Rect, banner: bool, footer: bool) -> Rect {
    let top = u16::from(banner);
    Rect {
        x: 0,
        y: top,
        width: area.width,
        height: area.height.saturating_sub(top + u16::from(footer)),
    }
}

pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, view: Rect) {
    let regions = now_regions(view);
    let mosaic = gui.now.cover.is_some_and(|cover| gui.ui.covered_last_frame(cover));
    // The band's waveform by the same rule: painted by the window only
    // where nothing stood over the band last frame.
    let wave_text = gui.ui.covered_last_frame(gui.now.band);
    let extras = NowExtras { reserve: TRANSPORT_ROWS, mosaic, no_hints: true, wave_text };
    let layout = render_now_view(frame, &regions, &mut gui.app, &extras);
    gui.now.band = layout.band;
    gui.now.cover = layout.cover;

    // The pointer's ways in, on the view's own elements (contract clauses
    // 4–6): a tab to pick, the band to seek — lit under the pointer as the
    // TUI lights it — and the cover's right click for the track's sheet.
    for (index, rect) in &layout.strip.tabs {
        gui.ui.click(*rect, Act::NowTab(*index));
    }
    if let Some((back, forward)) = layout.strip.arrows {
        gui.ui.click(back, Act::NowTabStep(-1));
        gui.ui.click(forward, Act::NowTabStep(1));
    }
    for column in 0..layout.band.width {
        let cell = Rect { x: layout.band.x + column, y: layout.band.y, width: 1, height: layout.band.height };
        gui.ui.click(cell, Act::NowSeek(column));
    }
    if let Some(cover) = layout.cover {
        gui.ui.context(cover, Act::NowMore);
    }

    // The transport under the cover (clause 3): prev, play, next in the
    // bar's frames, centred in the column — one row of air under the
    // picture, none under the card's own blank row when there is none.
    let air = u16::from(layout.cover.is_some());
    let spare = layout.spare;
    if spare.height >= air + 3 {
        let (prev, play, next) = play_glyphs(gui.bar_paused());
        let group_w: u16 = [prev, play, next].iter().map(|l| l.chars().count() as u16 + 4).sum::<u16>() + 2;
        let y = spare.y + air;
        let mut x = spare.x + spare.width.saturating_sub(group_w) / 2;
        x += tall_compact(frame, &mut gui.ui, x, y, prev, TallKind::Strong, Act::Prev) + 1;
        x += tall_compact(frame, &mut gui.ui, x, y, play, TallKind::Primary, Act::PlayPause) + 1;
        tall_compact(frame, &mut gui.ui, x, y, next, TallKind::Strong, Act::Next);
    }

    // The Auto-DJ tab's sources picker is the TUI's own overlay: the GUI
    // draws its genre picker and chooser as kit modals; this one it lends
    // from the view it reuses. In the window it is registered as an
    // overlay as theirs are, so the pictures the window paints beside the
    // cells (the cover beside it, the band's waveform under a long list)
    // stand down rather than paint over it. Only there: in a terminal the
    // picture goes out with the cells (kitty's placeholders are cells the
    // picker's draw replaces where they meet), and the TUI it is lent from
    // never stood its cover down for it either. A footprint there only
    // traded the picture for the mosaic and spent a hot frame on the
    // picker's open and close.
    if gui.app.dj_panel.sources.is_some() {
        let picker = crate::tui::ui::render_dj_picker(frame, view, &gui.app);
        if gui.app.graphics.is_hosted() && !picker.is_empty() {
            gui.ui.overlay(picker);
        }
    }

    // The screen's note, where the TUI's key hints would be (clause 9): the
    // GUI names its keys on the footer, and the row's right is the modes.
    if let Some((text, is_err)) = gui.note_words() {
        // Up to the modes readout, whatever it is wearing: a fixed forty
        // cells let a long note run into a wide readout.
        let room = layout.modes.x.saturating_sub(regions.keys.x + 1) as usize;
        let style = if is_err { Style::default().fg(th().gold) } else { dim() };
        put(frame, regions.keys.x, regions.keys.y, &super::bar::clip(&text, room), style);
    }
}

/// The screen's keys (contract clause 8): the TUI's full-screen keys,
/// forwarded to the App with its `fullscreen` flag up, and the GUI's own
/// transport letters. Returns true to quit.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> bool {
    // The App's own input modes first, the TUI keymap's way: the Auto-DJ
    // tab's keyword field takes the letters, and its sources picker (the
    // TUI's overlay, drawn here) takes the list keys — else Enter on those
    // rows left a mode nothing could serve or leave.
    if gui.app.dj_keyword.is_some() {
        match key.code {
            KeyCode::Char(c) => gui.forward(Action::Input(c)),
            KeyCode::Backspace => gui.forward(Action::Backspace),
            KeyCode::Enter => gui.forward(Action::Submit),
            KeyCode::Esc => gui.forward(Action::Cancel),
            _ => {}
        }
        return false;
    }
    if gui.app.dj_panel.sources.is_some() {
        match key.code {
            KeyCode::Up => gui.forward(Action::Up),
            KeyCode::Down => gui.forward(Action::Down),
            KeyCode::Char(' ') => gui.forward(Action::PlayPause),
            KeyCode::Enter | KeyCode::Esc => gui.forward(Action::Cancel),
            _ => {}
        }
        return false;
    }
    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc | KeyCode::Char('0') => return gui.act(Act::Screen(Screen::Library)),
        // The Stats screen, from anywhere (stats-screen contract, entry 2).
        KeyCode::Char('T') => return gui.act(Act::Screen(Screen::Stats)),
        KeyCode::Char('V') => return gui.act(Act::VizWindow),
        // The strip's numbers pick its tabs — a digit past the strip does
        // nothing, the App bounds-checks — and Tab, Shift-Tab cycle them.
        KeyCode::Char(c @ '1'..='9') => gui.forward(Action::SelectNowTab(c as usize - '1' as usize)),
        KeyCode::Tab => gui.forward(Action::NowTabNext),
        KeyCode::BackTab => gui.forward(Action::NowTabPrev),
        // The tab in front takes the list keys: the queue's rows, Discover's
        // neighbours, the Auto-DJ rows, the lyrics' scroll.
        KeyCode::Up => gui.forward(Action::Up),
        KeyCode::Down => gui.forward(Action::Down),
        KeyCode::PageUp => gui.forward(Action::PageUp),
        KeyCode::PageDown => gui.forward(Action::PageDown),
        KeyCode::Enter => gui.forward(Action::Activate),
        KeyCode::Left => gui.forward(Action::NowLeft),
        KeyCode::Right => gui.forward(Action::NowRight),
        KeyCode::Char('a') => gui.forward(Action::AddToQueue),
        KeyCode::Char('d') | KeyCode::Delete => gui.forward(Action::RemoveFromQueue),
        KeyCode::Char('i') => gui.forward(Action::JumpToPlaying),
        KeyCode::Char('v') => gui.forward(Action::CycleViz),
        KeyCode::Char('.') => gui.forward(Action::ToggleScatter),
        // The Library's Auto DJ room, the GUI's own key for it.
        KeyCode::Char('D') => return gui.act(Act::Nav(super::DJ_NAV)),
        KeyCode::Char(' ') => return gui.act(Act::PlayPause),
        KeyCode::Char('p') => return gui.act(Act::Prev),
        KeyCode::Char('n') => return gui.act(Act::Next),
        KeyCode::Char('s') => return gui.act(Act::Shuffle),
        KeyCode::Char('r') => return gui.act(Act::Repeat),
        KeyCode::Char('A') => return gui.act(Act::AutoDj),
        // The playing track's sheet, as the cover's right click.
        KeyCode::Char('m') => return gui.act(Act::NowMore),
        KeyCode::Char('-') => return gui.act(Act::VolDown),
        KeyCode::Char('+') | KeyCode::Char('=') => return gui.act(Act::VolUp),
        _ => {}
    }
    false
}
