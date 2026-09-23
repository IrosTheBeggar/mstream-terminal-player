//! The queue panel: the App's real queue as rows three cells tall — the
//! cover on the left at the card's own size, the title over the artist
//! over the album, the length on the last line where the hover [x] lands
//! (contract clauses 32 and 34), and the kit's live scrollbar down the
//! screen's edge. The playing row is told by colour and weight rather
//! than a mark: the column is thirty cells wide and the ▸ cost two of
//! them. An experiment, 2026-09-20; the rows grew from two lines to three
//! the same day, for a bigger cover.
//!
//! Covers ride the wall's discipline: the App's own claim, so a cover is
//! asked for once and from the row's own server (contract clause 30), and
//! a [`Slot`] per cover on view with caches of its own, paced by the
//! frame's encode budget. The slots are keyed by the cover, not the row's
//! place: an encoded picture draws anywhere for free, so a scroll moves
//! each row's cover with it and only a cover newly revealed pays an
//! encode — from the thumbnail, which has every pixel a 6x3 box can show.
//! A cover an overlay touched last frame — the header dropdown, a modal,
//! the tooltip — draws as the ▀-mosaic for that frame (the kit's
//! `Surface::overlay`): a picture's cells are skipped by the terminal
//! writer, so the overlay's edge and the frame after it leaves need plain
//! cells to repaint. Every other cover stays pixels.

use std::collections::{HashMap, HashSet};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::kit::theme::th;
use crate::kit::{dim, scroll_list, table_view};

use super::cover::{Pace, Slot};
use super::{Act, Gui, List, bar, bright_bold, put, sel, text_button};

/// The cover's cells: 6x3 is square at the common 10x20 font — the
/// now-playing card's cover, so the panel and the bar agree on a size.
const COVER_W: u16 = 6;
/// A row is as tall as its cover.
const ROW_H: u16 = 3;
/// The words start one cell of air past the cover.
const TEXT_X: u16 = COVER_W + 1;
/// The panel's width from its left edge to the screen's; the separator
/// stands two cells further left, and the rows begin under the title and
/// its rule on rows 2 and 3.
const PANEL_W: u16 = 32;
pub(crate) const TOP: u16 = 4;

/// How many covers past the view stay encoded, so a wheel back finds
/// them warm.
const SLACK: usize = 8;

pub(crate) struct QueueUi {
    /// A slot per cover on view, keyed by the art's id, each forked from
    /// the probed answer so it has caches of its own. A whole album shares
    /// one slot — one transmission, drawn on every row that wears it.
    slots: HashMap<u64, Slot>,
}

impl QueueUi {
    pub(crate) fn new() -> QueueUi {
        QueueUi { slots: HashMap::new() }
    }

    /// A resize changes the cell-to-pixel mapping every slot encoded
    /// against — the wall's rule, per cover.
    pub(crate) fn on_resize(&mut self) {
        for slot in self.slots.values_mut() {
            slot.on_resize();
        }
    }

    /// Every encode the panel's slots have paid — tests only.
    #[cfg(test)]
    fn encodes(&self) -> u32 {
        self.slots.values().map(Slot::encodes).sum()
    }
}

/// How many rows fit between the panel's head and the note line above
/// the bar: the single-line panel had `height - 13` lines to spend.
fn rows_that_fit(height: u16) -> usize {
    (height.saturating_sub(13) / ROW_H) as usize
}

/// The queue row under screen row `y`, in the panel's current window —
/// what a grip drag is over (track-actions contract, clause 18).
pub(crate) fn row_at(gui: &Gui, y: u16) -> Option<usize> {
    if y < TOP {
        return None;
    }
    let index = gui.queue_view.scroll + usize::from((y - TOP) / ROW_H);
    (index < gui.app.queue.items.len()).then_some(index)
}

pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let x = area.width - PANEL_W;
    for y in 2..area.height - 8 {
        put(frame, x - 2, y, "│", dim());
    }
    put(frame, x, 2, &t!("gui.queue.title"), dim());
    put(frame, x, 3, &"─".repeat(31), dim());
    let len = gui.app.queue.items.len();
    if len > 0 {
        // The header's clear (track-actions contract, clause 20): a small
        // dim word, no confirmation.
        let cx = x + t!("gui.queue.title").chars().count() as u16 + 2;
        let w = text_button(frame, gui, cx, 2, &t!("gui.queue.clear"), false, Act::QueueClear);
        gui.ui.tip(Rect { x: cx, y: 2, width: w, height: 1 }, t!("gui.queue.clear_tip").to_string());
    }
    if len == 0 {
        put(frame, x, TOP, &t!("gui.queue.empty"), dim());
        // While the DJ is armed, the ways to give it an opening song (auto-dj
        // contract, clause 16).
        super::dj::draw_empty_queue(frame, gui, x, TOP, 31);
        return;
    }
    let total: f64 = gui.app.queue.items.iter().filter_map(|t| t.metadata.duration).sum();
    let count = if len == 1 {
        t!("gui.queue.one").to_string()
    } else {
        t!("gui.queue.many", n = len).to_string()
    };
    let head = if total > 0.0 { format!("{count} · {}", bar::fmt_time(total)) } else { count };
    put(frame, area.width - 2 - head.chars().count() as u16, 2, &head, dim());

    // The view: the playing row is revealed only when it CHANGES, so the
    // wheel roams freely in between (the kit's table contract).
    let current = gui.app.queue.current;
    let focused = gui.app.focus == crate::tui::app::Focus::Queue;
    let selected = focused.then_some(gui.app.queue.state.selected()).flatten();
    let reveal = (current != gui.last_current)
        .then_some(current)
        .flatten()
        .or_else(|| (selected != gui.last_qsel).then_some(selected).flatten());
    gui.last_current = current;
    gui.last_qsel = selected;
    let (first, visible) = table_view(len, reveal, gui.queue_view.scroll, rows_that_fit(area.height));
    gui.queue_view.scroll = first;

    // The covers these rows still owe the cache, claimed through the App's
    // own fetch — hashmap lookups after the first frame.
    let mut fetches = Vec::new();
    for index in first..first + visible {
        if let Some(effect) = gui.app.fetch_queue_art(index) {
            fetches.push(effect);
        }
    }
    gui.pend(fetches);


    // Split borrows: the rows are READ from the App while each slot's
    // caches are written — disjoint fields of the one Gui, the wall's
    // move, so a frame never clones a row or its decoded cover.
    let pace = Pace::frame();
    let mut on_view: HashSet<u64> = HashSet::new();
    {
        let Gui { app, queue: panel, ui, .. } = &mut *gui;
        // The words run from the cover's air to the column's last cell,
        // one short of the rule's end.
        let text_w = usize::from(area.width - 2 - x - TEXT_X);
        for (row, index) in (first..first + visible).enumerate() {
            let Some(item) = app.queue.items.get(index) else { break };
            let y = TOP + row as u16 * ROW_H;
            let is_current = current == Some(index);
            let is_sel = selected == Some(index);
            let rect = Rect { x, y, width: area.width - 1 - x, height: ROW_H };
            let hover = ui.hovers(rect);
            let (title_style, sub_style) = match (is_sel, is_current, hover) {
                // The panel's keyboard cursor (track-actions contract,
                // clause 19): the title line takes the selection colour.
                (true, _, _) => (sel().add_modifier(Modifier::BOLD), sel()),
                (false, true, _) => (
                    Style::default().fg(th().ok).add_modifier(Modifier::BOLD),
                    Style::default().fg(th().ok),
                ),
                (false, false, true) => (bright_bold(), dim()),
                (false, false, false) => (Style::default(), dim()),
            };
            if is_sel {
                let line = Rect { x: x + TEXT_X, y, width: (area.width - 2 - x - TEXT_X).max(1), height: 1 };
                frame.render_widget(ratatui::widgets::Block::default().style(sel()), line);
            }

            // The cover: the picture once it is decoded; the empty slot
            // frame — the card's idiom — until then, or for a track with
            // no art at all.
            let cover = Rect { x, y, width: COVER_W, height: ROW_H };
            let art = item
                .metadata
                .album_art
                .as_deref()
                .and_then(|file| app.art.get(file))
                .and_then(|art| art.as_ref());
            match art {
                Some(art) => {
                    let slot = panel
                        .slots
                        .entry(art.id())
                        .or_insert_with(|| Slot::new(app.graphics.fork()));
                    on_view.insert(art.id());
                    // Text where an overlay stood last frame — the dropdown,
                    // a modal, the tooltip — pixels everywhere else.
                    if ui.covered_last_frame(cover) {
                        slot.draw_mosaic(frame, cover, art);
                    } else {
                        slot.draw_paced(frame, cover, art, &pace);
                    }
                }
                None => bar::cover_slot(frame, x, y, COVER_W, ROW_H),
            }

            let title = item.title_or_file();
            // The DJ's rows wear a badge before the title (auto-dj contract,
            // clause 60): a classic pick and a sonic one differently.
            let badge = match (item.dj, crate::kit::theme::legacy_conhost()) {
                (None, _) => "",
                (Some(_), true) => "* ",
                (Some(mark), false) if mark.sonic => "≈ ",
                (Some(_), false) => "∞ ",
            };
            if !badge.is_empty() {
                put(frame, x + TEXT_X, y, badge, dim());
            }
            let shift = badge.chars().count();
            // On hover the title yields its tail to the grip and the sheet's
            // verb on this line (clauses 17–18).
            let title_w = if hover { text_w.saturating_sub(shift + 7) } else { text_w.saturating_sub(shift) };
            put(frame, x + TEXT_X + shift as u16, y, &bar::clip(title, title_w), title_style);

            // Under the title, the artist then the album, each on a line of
            // its own — a missing one lets the other rise, so the words
            // stand under the title rather than over a gap. The first of
            // them follows the row's colour; the second stays dim. The
            // last line also carries the length, right — or the hover [x]
            // in its place (contract clause 32) — and whatever word shares
            // that line is clipped for the wider of the two, so a hover
            // never moves it.
            let time = item.metadata.duration.map(bar::fmt_time).unwrap_or_default();
            let tail = time.chars().count().max(3) + 1;
            let subs = [item.metadata.artist.as_deref(), item.metadata.album.as_deref()]
                .into_iter()
                .flatten()
                .filter(|words| !words.is_empty());
            for (line, words) in subs.take(usize::from(ROW_H) - 1).enumerate() {
                let ly = y + 1 + line as u16;
                let room = if ly == y + ROW_H - 1 { text_w.saturating_sub(tail) } else { text_w };
                let style = if line == 0 { sub_style } else { dim() };
                put(frame, x + TEXT_X, ly, &bar::clip(words, room), style);
            }
            // A click anywhere on the row plays it; the [x] on hover wins
            // its own cells (last registered, first hit).
            ui.click(rect, Act::QueueRow(index));
            ui.context(rect, Act::QueueMore(index));
            let last = y + ROW_H - 1;
            if hover {
                // The grip and the sheet's verb on the first line (clauses
                // 17–18), registered after the row so they win their cells.
                let grip = Rect { x: area.width - 7, y, width: 1, height: 1 };
                put(frame, grip.x, y, if crate::kit::theme::legacy_conhost() { "=" } else { "≡" }, dim());
                ui.click(grip, Act::QueueGrip(index));
                ui.tip(grip, t!("gui.queue.grip_tip").to_string());
                let more = Rect { x: area.width - 5, y, width: 3, height: 1 };
                put(frame, more.x, y, if crate::kit::theme::legacy_conhost() { "[.]" } else { "[⋯]" }, dim());
                ui.click(more, Act::QueueMore(index));
                ui.tip(more, t!("gui.act.more_tip").to_string());
                let cell = Rect { x: area.width - 5, y: last, width: 3, height: 1 };
                put(frame, cell.x, cell.y, "[x]", dim());
                ui.click(cell, Act::QueueRemove(index));
                ui.tip(cell, t!("gui.queue.remove_tip").to_string());
            } else if !time.is_empty() {
                put(frame, area.width - 2 - time.chars().count() as u16, last, &time, dim());
            }
        }
        // The kit's live scrollbar down the screen's last column, the
        // rows' full height: it draws only when rows overflow, and its
        // endcaps, track and thumb answer the pointer.
        scroll_list(
            frame,
            ui,
            Rect {
                x: area.width - 1,
                y: TOP,
                width: 1,
                height: rows_that_fit(area.height) as u16 * ROW_H,
            },
            len,
            visible,
            first,
            Act::ScrollBy(List::Queue, -1),
            Act::ScrollBy(List::Queue, 1),
            |first| Act::ScrollTo(List::Queue, first),
        );
    }
    // Slots for covers that have left the view are let go once the pool
    // outgrows the view by its slack — not before, so a wheel back finds
    // the last few still encoded.
    if gui.queue.slots.len() > on_view.len() + SLACK {
        gui.queue.slots.retain(|id, _| on_view.contains(id));
    }
    // Rows the budget turned away want the very next frame, not the next
    // poll tick — the event loop shortens its wait while this stands.
    if pace.starved() {
        gui.hot = true;
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Position;
    use ratatui::style::Modifier;

    use super::super::{Act, Gui, List, render};
    use crate::api::types::{Track, TrackMetadata};
    use crate::config::Config;
    use crate::kit::theme::th;
    use crate::tui::app::{App, Effect, KnownServer, Origin, Queued, TunnelState};
    use crate::tui::worker::ApiCmd;

    const HOME: &str = "http://host:3000";
    /// The panel's left edge and the words' column on a 100-wide frame;
    /// the rows start on line 4, three lines apiece.
    const X: usize = 68;
    const TEXT: usize = 75;
    const BAR: u16 = 99;

    fn queued(
        filepath: &str,
        title: &str,
        artist: Option<&str>,
        art: Option<&str>,
        duration: f64,
        server: &str,
    ) -> Queued {
        Queued {
            dj: None,
            origin: Origin { server: server.to_string(), peer: None },
            track: Track {
                filepath: filepath.to_string(),
                metadata: TrackMetadata {
                    title: Some(title.to_string()),
                    artist: artist.map(str::to_string),
                    album_art: art.map(str::to_string),
                    duration: Some(duration),
                    ..TrackMetadata::default()
                },
            },
        }
    }

    /// A connected session on HOME with these rows queued.
    fn gui_with(rows: Vec<Queued>) -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        gui.app.connected = true;
        gui.app.session.server = HOME.into();
        gui.app.session.server_id = HOME.into();
        gui.app.queue.items = rows;
        gui
    }

    fn two_rows() -> Gui {
        let mut first =
            queued("a.mp3", "Night Drive", Some("Moon Parade"), Some("aa.jpeg"), 252.0, HOME);
        first.track.metadata.album = Some("Late Shift".into());
        gui_with(vec![first, queued("b.mp3", "Aurora", None, None, 228.0, HOME)])
    }

    fn draw(gui: &mut Gui) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn lines(buffer: &Buffer) -> Vec<String> {
        let area = *buffer.area();
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    /// `n` cells of a line from column `x`, by character.
    fn cells(line: &str, x: usize, n: usize) -> String {
        line.chars().skip(x).take(n).collect()
    }

    fn solid_cover() -> crate::tui::art::Art {
        let png = image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        png.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        crate::tui::art::decode(&bytes.into_inner()).unwrap()
    }

    fn art_asks(gui: &Gui) -> Vec<(String, Option<String>)> {
        gui.pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::AlbumArt { file, reach }) => {
                    Some((file.clone(), reach.as_ref().map(|r| r.base.clone())))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn rows_stand_three_tall_with_the_artist_and_album_under_the_title_and_no_mark() {
        let mut gui = two_rows();
        gui.app.queue.current = Some(0);
        let rows = lines(&draw(&mut gui));
        assert!(cells(&rows[4], TEXT, 11) == "Night Drive", "the title: {:?}", rows[4]);
        assert!(cells(&rows[5], TEXT, 11) == "Moon Parade", "the artist beneath: {:?}", rows[5]);
        assert!(cells(&rows[6], TEXT, 10) == "Late Shift", "the album beneath that: {:?}", rows[6]);
        assert_eq!(cells(&rows[6], 94, 4), "4:12", "the length on the last line");
        assert!(cells(&rows[7], TEXT, 6) == "Aurora", "the next row three lines down: {:?}", rows[7]);
        assert_eq!(cells(&rows[8], TEXT, 20).trim(), "", "no artist, no album: nothing rises");
        assert_eq!(cells(&rows[9], 94, 4), "3:48");
        for (y, row) in rows.iter().enumerate().take(20).skip(4) {
            let column: String = row.chars().skip(X - 2).collect();
            assert!(!column.contains('▸') && !column.contains('>'), "row {y} wears a mark: {column:?}");
        }
        // The slot frame holds the cover's cells while nothing is decoded.
        assert!(cells(&rows[4], X, 6).contains('╭'), "the slot frame: {:?}", cells(&rows[4], X, 6));
        assert_eq!(cells(&rows[4], BAR as usize, 1), " ", "two rows: no scrollbar");
    }

    #[test]
    fn a_lone_album_rises_under_the_title() {
        let mut only_album = queued("c.mp3", "Cassini IV", None, None, 200.0, HOME);
        only_album.track.metadata.album = Some("Orbital".into());
        let mut gui = gui_with(vec![only_album]);
        let rows = lines(&draw(&mut gui));
        assert!(cells(&rows[5], TEXT, 7) == "Orbital", "the album takes the artist's line: {:?}", rows[5]);
        assert_eq!(cells(&rows[6], TEXT, 18).trim(), "", "and leaves the last line to the length");
        assert_eq!(cells(&rows[6], 94, 4), "3:20");
    }

    #[test]
    fn the_playing_row_is_told_by_colour_and_weight() {
        let mut gui = two_rows();
        gui.app.queue.current = Some(0);
        let buffer = draw(&mut gui);
        let title = &buffer[(TEXT as u16, 4)];
        assert_eq!(title.fg, th().ok, "the playing title wears the ok colour");
        assert!(title.modifier.contains(Modifier::BOLD), "and weight");
        let artist = &buffer[(TEXT as u16, 5)];
        assert_eq!(artist.fg, th().ok, "its artist line follows the colour");
        assert!(!artist.modifier.contains(Modifier::BOLD), "without the weight");
        let album = &buffer[(TEXT as u16, 6)];
        assert_ne!(album.fg, th().ok, "the album line stays dim");
        let other = &buffer[(TEXT as u16, 7)];
        assert_ne!(other.fg, th().ok, "the row that is not playing does not");
    }

    #[test]
    fn covers_are_claimed_once_and_through_the_rows_own_server() {
        let other = "http://other:3000";
        let mut gui = gui_with(vec![
            queued("a.mp3", "Home", None, Some("aa.jpeg"), 200.0, HOME),
            queued("b.mp3", "Away", None, Some("bb.jpeg"), 200.0, other),
            queued("c.mp3", "Plain", None, None, 200.0, HOME),
        ]);
        gui.app.servers.push(KnownServer {
            id: other.into(),
            name: "Other".into(),
            token: Some("tok".into()),
            self_signed: false,
            peer: None,
            pairing: None, dj: Default::default(),
        });
        draw(&mut gui);
        let asks = art_asks(&gui);
        assert_eq!(asks.len(), 2, "one ask per cover the rows name: {asks:?}");
        assert!(asks.contains(&("aa.jpeg".to_string(), None)), "the session's own row: {asks:?}");
        assert!(
            asks.contains(&("bb.jpeg".to_string(), Some(other.to_string()))),
            "the other server's row asks its own server: {asks:?}"
        );
        let reach = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::AlbumArt { file, reach }) if file == "bb.jpeg" => reach.clone(),
            _ => None,
        });
        assert_eq!(reach.unwrap().token.as_deref(), Some("tok"), "with its token");

        // A second frame finds every cover claimed and asks for nothing.
        draw(&mut gui);
        assert_eq!(art_asks(&gui).len(), 2, "the claim keeps the panel from asking twice");
    }

    #[test]
    fn a_row_on_a_tunnel_that_is_down_waits_to_be_claimed() {
        let id = crate::quickconnect::testing::sample_id();
        let mut gui =
            gui_with(vec![queued("t.mp3", "Far", None, Some("cc.jpeg"), 200.0, &id)]);
        draw(&mut gui);
        assert!(art_asks(&gui).is_empty(), "nothing to ask while the tunnel is closed");
        assert!(!gui.app.art.contains_key("cc.jpeg"), "and no placeholder stands in the way");

        gui.app.tunnels.insert(
            id,
            TunnelState::Up {
                local_url: "http://127.0.0.1:5555".into(),
                local_token: "lt".into(),
                status: crate::quickconnect::TunnelStatus::Connected,
                path: None,
            },
        );
        draw(&mut gui);
        let asks = art_asks(&gui);
        assert_eq!(asks, vec![("cc.jpeg".to_string(), Some("http://127.0.0.1:5555".to_string()))]);
        let reach = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::AlbumArt { reach, .. }) => reach.clone(),
            _ => None,
        });
        assert_eq!(reach.unwrap().local_token.as_deref(), Some("lt"), "over the bridge, with its token");
    }

    #[test]
    fn a_decoded_cover_fills_the_rows_left_cells() {
        let mut gui = two_rows();
        gui.app.art.insert("aa.jpeg".into(), Some(solid_cover()));
        let rows = lines(&draw(&mut gui));
        for y in [4usize, 5, 6] {
            let cover = cells(&rows[y], X, 6);
            assert!(
                cover.chars().all(|c| "█▀▄".contains(c)),
                "the mosaic holds the cells of line {y}: {cover:?}"
            );
        }
        // The row without art keeps the frame.
        assert!(cells(&rows[7], X, 6).contains('╭'));
    }

    #[test]
    fn the_hover_x_takes_the_lengths_place_on_the_last_line() {
        let mut gui = two_rows();
        gui.ui.pointer = Some(Position { x: 80, y: 4 });
        let rows = lines(&draw(&mut gui));
        assert_eq!(cells(&rows[6], 95, 3), "[x]", "the [x] on the hovered row's last line");
        assert!(!rows[6].contains("4:12"), "in the length's place: {:?}", rows[6]);
        assert!(cells(&rows[6], TEXT, 10) == "Late Shift", "the album did not move");
        assert_eq!(cells(&rows[9], 94, 4), "3:48", "the other row keeps its length");
        assert_eq!(gui.ui.hit(Position { x: 96, y: 6 }), Some(Act::QueueRemove(0)));
        assert_eq!(gui.ui.hit(Position { x: 80, y: 6 }), Some(Act::QueueRow(0)), "any line plays");
        assert_eq!(gui.ui.hit(Position { x: 80, y: 8 }), Some(Act::QueueRow(1)));
    }

    #[test]
    fn a_cover_stands_down_only_under_an_overlay_and_only_while_it_stands() {
        use ratatui_image::picker::ProtocolType;
        // Two rows with covers of their own; two saved servers, so the
        // header dropdown is tall enough (five lines from line 1) to stand
        // over the first row's cover and not the second's.
        let mut gui = gui_with(vec![
            queued("a.mp3", "First", None, Some("aa.jpeg"), 200.0, HOME),
            queued("b.mp3", "Second", None, Some("bb.jpeg"), 200.0, HOME),
        ]);
        gui.config.servers = vec![
            crate::config::ServerEntry { url: "http://attic.local:3000".into(), ..Default::default() },
            crate::config::ServerEntry { url: "http://office.local:3000".into(), ..Default::default() },
        ];
        gui.app.graphics = crate::tui::graphics::Graphics::forced(ProtocolType::Kitty);
        gui.app.art.insert("aa.jpeg".into(), Some(solid_cover()));
        gui.app.art.insert("bb.jpeg".into(), Some(solid_cover()));
        let placeholders = |buffer: &Buffer, y: u16| buffer[(X as u16, y)].symbol().contains('\u{10EEEE}');
        let mosaic = |buffer: &Buffer, y: u16| "█▀▄".contains(buffer[(X as u16, y)].symbol());

        settle(&mut gui);
        assert_eq!(gui.queue.encodes(), 2, "both covers encoded");

        // The dropdown opens: this frame draws before it and does not know
        // yet; the frame after, the first row's uncovered line is text and
        // the second row is still pixels.
        gui.servers.drop_open = true;
        draw(&mut gui);
        let buffer = draw(&mut gui);
        assert!(mosaic(&buffer, 6), "the touched cover's free line is text: {:?}", buffer[(X as u16, 6)].symbol());
        assert!(placeholders(&buffer, 7), "the cover below the dropdown keeps its pixels");

        // It closes: one more frame of text repaints what it wrote, then
        // the pixels return — from the warm cache, no encode.
        gui.servers.drop_open = false;
        let buffer = draw(&mut gui);
        assert!(mosaic(&buffer, 4), "the frame after the overlay leaves is text");
        let buffer = draw(&mut gui);
        assert!(placeholders(&buffer, 4), "then pixels again");
        assert_eq!(gui.queue.encodes(), 2, "the overlay's passing cost no encode");
    }

    fn ten_rows() -> Gui {
        gui_with(
            (0..10)
                .map(|i| queued(&format!("{i}.mp3"), &format!("Track {i:02}"), None, None, 100.0, HOME))
                .collect(),
        )
    }

    #[test]
    fn the_view_holds_as_many_rows_as_fit_and_the_wheel_moves_it() {
        let mut gui = ten_rows();
        let all = lines(&draw(&mut gui)).join("\n");
        assert_eq!(super::rows_that_fit(30), 5);
        assert!(all.contains("Track 04") && !all.contains("Track 05"), "five rows fit:\n{all}");

        gui.queue_view.scroll = 7;
        let all = lines(&draw(&mut gui)).join("\n");
        assert!(all.contains("Track 09") && !all.contains("Track 04"), "the wheel clamps to the end:\n{all}");
        assert_eq!(gui.queue_view.scroll, 5, "and the offset is written back clamped");
    }

    #[test]
    fn the_scrollbar_stands_on_overflow_and_its_ends_and_track_answer() {
        let mut gui = ten_rows();
        let rows = lines(&draw(&mut gui));
        // Down the screen's last column, the rows' full height: fifteen
        // lines for five rows of three.
        assert_eq!(cells(&rows[4], BAR as usize, 1), "▲", "the top cap: {:?}", rows[4]);
        assert_eq!(cells(&rows[18], BAR as usize, 1), "▼", "the bottom cap: {:?}", rows[18]);
        assert_eq!(cells(&rows[5], BAR as usize, 1), "█", "the thumb at the top while first is 0");
        assert_eq!(gui.ui.hit(Position { x: BAR, y: 18 }), Some(Act::ScrollBy(List::Queue, 1)));
        assert_eq!(gui.ui.hit(Position { x: BAR, y: 4 }), Some(Act::ScrollBy(List::Queue, -1)));
        assert!(
            matches!(gui.ui.hit(Position { x: BAR, y: 17 }), Some(Act::ScrollTo(List::Queue, _))),
            "a track cell jumps"
        );
        gui.act(Act::ScrollBy(List::Queue, 1));
        assert_eq!(gui.queue_view.scroll, 1);
        gui.act(Act::ScrollTo(List::Queue, 4));
        let rows = lines(&draw(&mut gui));
        assert!(rows[4].contains("Track 04"), "the jump landed: {:?}", rows[4]);
        assert_eq!(cells(&rows[17], BAR as usize, 1), "█", "the thumb rode to the end");
        // The wheel over the panel rides the same act.
        gui.wheel(Position { x: 90, y: 10 }, -1);
        assert_eq!(gui.queue_view.scroll, 3);
    }

    /// Ten rows, each with a cover of its own decoded and waiting.
    fn ten_covered_rows() -> Gui {
        use ratatui_image::picker::ProtocolType;
        let mut gui = gui_with(
            (0..10)
                .map(|i| {
                    queued(&format!("{i}.mp3"), &format!("Track {i:02}"), None, Some(&format!("c{i}.jpeg")), 100.0, HOME)
                })
                .collect(),
        );
        gui.app.graphics = crate::tui::graphics::Graphics::forced(ProtocolType::Kitty);
        for i in 0..10 {
            gui.app.art.insert(format!("c{i}.jpeg"), Some(solid_cover()));
        }
        gui
    }

    /// Frames until the budget has let every cover on view upgrade.
    fn settle(gui: &mut Gui) -> Buffer {
        let mut buffer = draw(gui);
        for _ in 0..10 {
            if !gui.hot {
                break;
            }
            buffer = draw(gui);
        }
        buffer
    }

    #[test]
    fn a_scroll_moves_the_covers_with_their_rows_and_encodes_only_the_new_one() {
        let mut gui = ten_covered_rows();
        settle(&mut gui);
        assert_eq!(gui.queue.encodes(), 5, "five covers on view, five encodes");

        gui.act(Act::ScrollBy(List::Queue, 1));
        settle(&mut gui);
        assert_eq!(gui.queue.encodes(), 6, "one row came into view: one encode, four covers moved");

        gui.act(Act::ScrollBy(List::Queue, -1));
        settle(&mut gui);
        assert_eq!(gui.queue.encodes(), 6, "and back: the cover that left was still warm");

        // The pool holds the view plus its slack, then lets the rest go.
        for _ in 0..5 {
            gui.act(Act::ScrollBy(List::Queue, 1));
            settle(&mut gui);
        }
        assert_eq!(gui.queue.encodes(), 10, "every cover encoded once on its way through");
        assert!(gui.queue.slots.len() <= 5 + super::SLACK, "{} slots", gui.queue.slots.len());
    }

    #[test]
    fn rows_sharing_a_cover_share_one_slot_and_one_encode() {
        use ratatui_image::picker::ProtocolType;
        let mut gui = gui_with(
            (0..5)
                .map(|i| queued(&format!("{i}.mp3"), &format!("Track {i}"), None, Some("aa.jpeg"), 100.0, HOME))
                .collect(),
        );
        gui.app.graphics = crate::tui::graphics::Graphics::forced(ProtocolType::Kitty);
        gui.app.art.insert("aa.jpeg".into(), Some(solid_cover()));
        let buffer = settle(&mut gui);
        assert_eq!(gui.queue.slots.len(), 1, "one album, one slot");
        assert_eq!(gui.queue.encodes(), 1, "transmitted once");
        for row in 0..5u16 {
            let y = 4 + row * 3;
            assert!(
                buffer[(X as u16, y)].symbol().contains('\u{10EEEE}'),
                "row {row} wears the picture by reference"
            );
        }
    }

    /// The panel with a cover decoded, same eyeball as the shell's:
    /// `cargo test dump_queue_panel -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_queue_panel() {
        let mut first =
            queued("a.mp3", "Night Drive", Some("Moon Parade"), Some("aa.jpeg"), 252.0, HOME);
        first.track.metadata.album = Some("Late Shift".into());
        let mut gui = gui_with(vec![
            first,
            queued("b.mp3", "Aurora Over the Long Meadow Road", Some("The Very Long Band Name Ensemble"), None, 228.0, HOME),
            queued("c.mp3", "Cassini IV", Some("Orbital Drift"), Some("aa.jpeg"), 3725.0, HOME),
            queued("d.mp3", "Fourth", None, None, 100.0, HOME),
            queued("e.mp3", "Fifth", None, None, 100.0, HOME),
            queued("f.mp3", "Sixth, off the end", None, None, 100.0, HOME),
        ]);
        gui.app.queue.current = Some(1);
        gui.app.art.insert("aa.jpeg".into(), Some(solid_cover()));
        for row in lines(&draw(&mut gui)) {
            println!("|{row}|");
        }
    }
}
