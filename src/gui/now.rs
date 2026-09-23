//! The Now Playing screen: the playing track large — its cover as pixels
//! where the terminal draws them (the mosaic elsewhere), its words and the
//! facts the sheet and the bar's card already wear beneath — in place of
//! the Library's nav column and room. The queue panel and the bar stand
//! under it as under the Library.
//!
//! A first cut, ahead of its contract (the mobile record's player panel,
//! lyrics screen and visualizer; see PLAN.md): the screen is the slot the
//! contract's tabs will fill.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use super::cover::{Pace, Slot};
use super::{Act, Gui, Screen, put};
use crate::kit::dim;
use crate::kit::theme::{legacy_conhost, th};

/// What the screen keeps: the big cover's own slot (its pixel cache and
/// mosaic grid), forked from the App's graphics on first use.
#[derive(Default)]
pub(crate) struct NowUi {
    slot: Option<Slot>,
}

impl NowUi {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A resize changes the cell-to-pixel mapping the slot encoded against.
    pub(crate) fn on_resize(&mut self) {
        if let Some(slot) = &mut self.slot {
            slot.on_resize();
        }
    }
}

/// The screen's stage: the full width under the top bar, less the queue
/// panel when it is open, down to the row above the bar.
pub(crate) fn stage_rect(area: Rect, queue_open: bool, footer: bool) -> Rect {
    let right = if queue_open { area.width - 36 } else { area.width - 3 };
    Rect { x: 1, y: 2, width: right - 1, height: area.height - 2 - super::bar::BAR_ROWS - u16::from(footer) }
}

pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, stage: Rect) {
    let now = gui.bar_now();
    let Gui { app, now: screen, ui, hot, .. } = &mut *gui;
    let Some(now) = now else {
        let words = t!("gui.nothing_playing");
        let x = stage.x + stage.width.saturating_sub(words.chars().count() as u16) / 2;
        put(frame, x, stage.y + stage.height / 2, &words, dim());
        return;
    };

    // The cover: square in cells (a cell is twice as tall as it is wide),
    // as large as the stage allows with five rows of words beneath it.
    let h = stage.height.saturating_sub(7).min(stage.width.saturating_sub(4) / 2).max(4);
    let cover = Rect { x: stage.x + 2, y: stage.y + 1, width: h * 2, height: h };
    let art = app
        .now_playing
        .as_ref()
        .and_then(|track| track.metadata.album_art.as_deref())
        .and_then(|file| app.art.get(file))
        .and_then(|art| art.as_ref());
    match art {
        Some(art) => {
            let slot = screen.slot.get_or_insert_with(|| Slot::new(app.graphics.fork()));
            // Under an overlay the mosaic stands in (a picture's cells are
            // skipped for the terminal writer); a slot the budget turned
            // away wants the very next frame.
            if ui.covered_last_frame(cover) {
                slot.draw_mosaic(frame, cover, art);
            } else {
                let pace = Pace::frame();
                slot.draw_paced(frame, cover, art, &pace);
                if pace.starved() {
                    *hot = true;
                }
            }
        }
        None => super::bar::cover_slot(frame, cover.x, cover.y, cover.width, cover.height),
    }

    // The words beneath, the card's four lines at full width.
    let x = cover.x;
    let width = stage.right().saturating_sub(x + 1) as usize;
    let mut y = cover.bottom() + 1;
    put(frame, x, y, &super::bar::clip(&now.title, width), Style::default().add_modifier(Modifier::BOLD));
    y += 1;
    let mut byline: Vec<String> = Vec::new();
    if !now.artist.is_empty() {
        byline.push(now.artist.clone());
    }
    if !now.album.is_empty() {
        byline.push(now.album.clone());
    }
    if let Some(year) = now.year {
        byline.push(year.to_string());
    }
    if !byline.is_empty() {
        put(frame, x, y, &super::bar::clip(&byline.join(" · "), width), dim());
        y += 1;
    }
    if !now.spec.is_empty() {
        put(frame, x, y, &super::bar::clip(&now.spec, width), dim());
        y += 1;
    }
    let mut fx = x;
    if let Some(rating) = now.rating.filter(|r| *r > 0) {
        let glyphs = super::actions::stars(Some(rating));
        put(frame, fx, y, &glyphs, Style::default().fg(th().gold));
        fx += glyphs.chars().count() as u16 + 2;
    }
    let mut facts: Vec<String> = Vec::new();
    if let Some(key) = now.key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        facts.push(format!("{} {key}", if legacy_conhost() { "key" } else { "♪" }));
    }
    if let Some(bpm) = now.bpm.filter(|b| *b > 0) {
        facts.push(format!("{bpm} BPM"));
    }
    if !facts.is_empty() {
        let room = stage.right().saturating_sub(fx + 1) as usize;
        put(frame, fx, y, &super::bar::clip(&facts.join(" · "), room), dim());
    }
}

/// The screen's keys: the global ones, and the ways back to the Library.
/// Returns true to quit.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('q') => true,
        KeyCode::Esc | KeyCode::Char('0') => gui.act(Act::Screen(Screen::Library)),
        KeyCode::Tab => gui.act(Act::ToggleQueue),
        KeyCode::Char(c @ '1'..='9') => gui.act(Act::Nav(c as usize - '1' as usize)),
        KeyCode::Char(' ') => gui.act(Act::PlayPause),
        KeyCode::Char('p') => gui.act(Act::Prev),
        KeyCode::Char('n') => gui.act(Act::Next),
        KeyCode::Char('s') => gui.act(Act::Shuffle),
        KeyCode::Char('r') => gui.act(Act::Repeat),
        KeyCode::Char('A') => gui.act(Act::AutoDj),
        // The playing track's sheet, as the card's right click.
        KeyCode::Char('m') => gui.act(Act::NowMore),
        KeyCode::Char('-') => gui.act(Act::VolDown),
        KeyCode::Char('+') | KeyCode::Char('=') => gui.act(Act::VolUp),
        _ => false,
    }
}
