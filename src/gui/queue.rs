//! The queue panel: the App's real queue as rows two cells tall — the
//! cover on the left, the title over the artist, the length on the second
//! line where the hover [x] lands (contract clauses 32 and 34). The
//! playing row is told by colour and weight rather than a mark: the column
//! is thirty cells wide and the ▸ cost two of them, so the title now runs
//! twenty-five cells where it ran twenty-two. An experiment, 2026-09-20.
//!
//! Covers ride the wall's discipline: the App's own claim, so a cover is
//! asked for once and from the row's own server (contract clause 30), and
//! a [`Slot`] per visible row with caches of its own, paced by the frame's
//! encode budget. Pixels stand down under anything drawn over the column
//! — the header dropdown, a modal, the pairing QR — because a picture's
//! cells are skipped by the terminal writer and a modal's text over them
//! would never land; the ▀-mosaic is plain cells and layers like any text.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::kit::theme::th;
use crate::kit::{dim, table_view};

use super::cover::{Pace, Slot};
use super::{Act, Gui, bar, bright_bold, put};

/// The cover's cells: 4x2 is square at the common 10x20 font.
const COVER_W: u16 = 4;
/// A row is as tall as its cover.
const ROW_H: u16 = 2;
/// The words start one cell of air past the cover.
const TEXT_X: u16 = COVER_W + 1;
/// The panel's width from its left edge to the screen's; the separator
/// stands two cells further left, and the rows begin under the title and
/// its rule on rows 2 and 3.
const PANEL_W: u16 = 32;
const TOP: u16 = 4;

pub(crate) struct QueueUi {
    /// One per visible row position, forked from the probed answer so each
    /// has caches of its own.
    slots: Vec<Slot>,
}

impl QueueUi {
    pub(crate) fn new() -> QueueUi {
        QueueUi { slots: Vec::new() }
    }

    /// A resize changes the cell-to-pixel mapping every slot encoded
    /// against — the wall's rule, per row.
    pub(crate) fn on_resize(&mut self) {
        for slot in &mut self.slots {
            slot.on_resize();
        }
    }
}

/// How many rows fit between the panel's head and the note line above
/// the bar: the single-row panel had `height - 13` lines to spend.
fn rows_that_fit(height: u16) -> usize {
    (height.saturating_sub(13) / ROW_H) as usize
}

pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let x = area.width - PANEL_W;
    for y in 2..area.height - 8 {
        put(frame, x - 2, y, "│", dim());
    }
    put(frame, x, 2, &t!("gui.queue.title"), dim());
    put(frame, x, 3, &"─".repeat(31), dim());
    let len = gui.app.queue.items.len();
    if len == 0 {
        put(frame, x, TOP, &t!("gui.queue.empty"), dim());
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
    let reveal = (current != gui.last_current).then_some(current).flatten();
    gui.last_current = current;
    let (first, visible) = table_view(len, reveal, gui.qscroll, rows_that_fit(area.height));
    gui.qscroll = first;

    // The covers these rows still owe the cache, claimed through the App's
    // own fetch — hashmap lookups after the first frame.
    let mut fetches = Vec::new();
    for index in first..first + visible {
        if let Some(effect) = gui.app.fetch_queue_art(index) {
            fetches.push(effect);
        }
    }
    gui.pend(fetches);

    // Pixels only while nothing stands over the column.
    let pixels = !super::overlay_open(gui);
    while gui.queue.slots.len() < visible {
        let fork = gui.app.graphics.fork();
        gui.queue.slots.push(Slot::new(fork));
    }

    // Split borrows: the rows are READ from the App while each slot's
    // caches are written — disjoint fields of the one Gui, the wall's
    // move, so a frame never clones a row or its decoded cover.
    let pace = Pace::frame();
    {
        let Gui { app, queue: panel, ui, .. } = &mut *gui;
        // The words run from the cover's air to the column's last cell,
        // one short of the rule's end.
        let text_w = usize::from(area.width - 2 - x - TEXT_X);
        for (row, index) in (first..first + visible).enumerate() {
            let Some(item) = app.queue.items.get(index) else { break };
            let y = TOP + row as u16 * ROW_H;
            let is_current = current == Some(index);
            let rect = Rect { x, y, width: area.width - 1 - x, height: ROW_H };
            let hover = ui.pointer.is_some_and(|p| rect.contains(p));
            let (title_style, sub_style) = match (is_current, hover) {
                (true, _) => (
                    Style::default().fg(th().ok).add_modifier(Modifier::BOLD),
                    Style::default().fg(th().ok),
                ),
                (false, true) => (bright_bold(), dim()),
                (false, false) => (Style::default(), dim()),
            };

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
                Some(art) if pixels => panel.slots[row].draw_paced(frame, cover, art, &pace),
                Some(art) => panel.slots[row].draw_mosaic(frame, cover, art),
                None => bar::cover_slot(frame, x, y, COVER_W, ROW_H),
            }

            let title = item.metadata.display_title().unwrap_or_else(|| item.file_name());
            put(frame, x + TEXT_X, y, &bar::clip(title, text_w), title_style);

            // The second line: the artist (the album when there is none)
            // left, the length right — or the hover [x] in its place
            // (contract clause 32). The artist's clip leaves the wider of
            // the two its room, so a hover never moves a word.
            let time = item.metadata.duration.map(bar::fmt_time).unwrap_or_default();
            let tail = time.chars().count().max(3) + 1;
            let sub = item
                .metadata
                .artist
                .as_deref()
                .filter(|artist| !artist.is_empty())
                .or(item.metadata.album.as_deref())
                .unwrap_or_default();
            if !sub.is_empty() {
                put(frame, x + TEXT_X, y + 1, &bar::clip(sub, text_w.saturating_sub(tail)), sub_style);
            }
            // A click anywhere on the row plays it; the [x] on hover wins
            // its own cells (last registered, first hit).
            ui.click(rect, Act::QueueRow(index));
            if hover {
                let cell = Rect { x: area.width - 5, y: y + 1, width: 3, height: 1 };
                put(frame, cell.x, cell.y, "[x]", dim());
                ui.click(cell, Act::QueueRemove(index));
                ui.tip(cell, t!("gui.queue.remove_tip").to_string());
            } else if !time.is_empty() {
                put(frame, area.width - 2 - time.chars().count() as u16, y + 1, &time, dim());
            }
        }
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

    use super::super::{Act, Gui, render};
    use crate::api::types::{Track, TrackMetadata};
    use crate::config::Config;
    use crate::kit::theme::th;
    use crate::tui::app::{App, Effect, KnownServer, Origin, Queued, TunnelState};
    use crate::tui::worker::ApiCmd;

    const HOME: &str = "http://host:3000";
    /// The panel's left edge and the words' column on a 100-wide frame.
    const X: usize = 68;
    const TEXT: usize = 73;

    fn queued(
        filepath: &str,
        title: &str,
        artist: Option<&str>,
        art: Option<&str>,
        duration: f64,
        server: &str,
    ) -> Queued {
        Queued {
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
        gui_with(vec![
            queued("a.mp3", "Night Drive", Some("Moon Parade"), Some("aa.jpeg"), 252.0, HOME),
            queued("b.mp3", "Aurora", None, None, 228.0, HOME),
        ])
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
    fn rows_stand_two_tall_with_the_artist_under_the_title_and_no_mark() {
        let mut gui = two_rows();
        gui.app.queue.current = Some(0);
        let rows = lines(&draw(&mut gui));
        assert!(rows[4][..].chars().skip(TEXT).collect::<String>().starts_with("Night Drive"));
        assert!(cells(&rows[5], TEXT, 11) == "Moon Parade", "the artist beneath: {:?}", rows[5]);
        assert_eq!(cells(&rows[5], 94, 4), "4:12", "the length on the second line");
        assert!(cells(&rows[6], TEXT, 6) == "Aurora", "the next row two lines down: {:?}", rows[6]);
        assert_eq!(cells(&rows[7], TEXT, 20).trim(), "", "no artist, no second-line words");
        assert_eq!(cells(&rows[7], 94, 4), "3:48");
        for (y, row) in rows.iter().enumerate().take(20).skip(4) {
            let column: String = row.chars().skip(X - 2).collect();
            assert!(!column.contains('▸') && !column.contains('>'), "row {y} wears a mark: {column:?}");
        }
        // The slot frame holds the cover's cells while nothing is decoded.
        assert!(cells(&rows[4], X, 4).contains('╭'), "the slot frame: {:?}", cells(&rows[4], X, 4));
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
        let other = &buffer[(TEXT as u16, 6)];
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
            pairing: None,
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
        for y in [4usize, 5] {
            let cover = cells(&rows[y], X, 4);
            assert!(
                cover.chars().all(|c| "█▀▄".contains(c)),
                "the mosaic holds the cells of line {y}: {cover:?}"
            );
        }
        // The row without art keeps the frame.
        assert!(cells(&rows[6], X, 4).contains('╭'));
    }

    #[test]
    fn the_hover_x_takes_the_lengths_place_on_the_second_line() {
        let mut gui = two_rows();
        gui.ui.pointer = Some(Position { x: 80, y: 4 });
        let rows = lines(&draw(&mut gui));
        assert_eq!(cells(&rows[5], 95, 3), "[x]", "the [x] on the hovered row's second line");
        assert!(!rows[5].contains("4:12"), "in the length's place: {:?}", rows[5]);
        assert!(cells(&rows[5], TEXT, 11) == "Moon Parade", "the artist did not move");
        assert_eq!(cells(&rows[7], 94, 4), "3:48", "the other row keeps its length");
        assert_eq!(gui.ui.hit(Position { x: 96, y: 5 }), Some(Act::QueueRemove(0)));
        assert_eq!(gui.ui.hit(Position { x: 80, y: 5 }), Some(Act::QueueRow(0)), "either line plays");
        assert_eq!(gui.ui.hit(Position { x: 80, y: 7 }), Some(Act::QueueRow(1)));
    }

    #[test]
    fn pixels_stand_down_while_an_overlay_is_up() {
        use ratatui_image::picker::ProtocolType;
        let mut gui = two_rows();
        gui.app.graphics = crate::tui::graphics::Graphics::forced(ProtocolType::Kitty);
        gui.app.art.insert("aa.jpeg".into(), Some(solid_cover()));

        gui.servers.drop_open = true;
        let rows = lines(&draw(&mut gui));
        assert!(gui.queue.slots[0].key.is_none(), "no pixels under the dropdown");
        // The dropdown itself stands over the column's first rows, so the
        // mosaic is read where the dropdown ends — its rule stands in the
        // frame; what matters is that no picture was placed.
        assert!(rows.len() == 30);

        gui.servers.drop_open = false;
        draw(&mut gui);
        assert!(gui.queue.slots[0].key.is_some(), "pixels once the column is clear");

        gui.servers.drop_open = true;
        draw(&mut gui);
        assert!(gui.queue.slots[0].key.is_none(), "and down again when it returns");
    }

    #[test]
    fn the_view_holds_as_many_rows_as_fit_and_the_wheel_moves_it() {
        let rows: Vec<Queued> =
            (0..10).map(|i| queued(&format!("{i}.mp3"), &format!("Track {i:02}"), None, None, 100.0, HOME)).collect();
        let mut gui = gui_with(rows);
        let all = lines(&draw(&mut gui)).join("\n");
        assert_eq!(super::rows_that_fit(30), 8);
        assert!(all.contains("Track 07") && !all.contains("Track 08"), "eight rows fit:\n{all}");

        gui.qscroll = 5;
        let all = lines(&draw(&mut gui)).join("\n");
        assert!(all.contains("Track 09") && !all.contains("Track 01"), "the wheel clamps to the end:\n{all}");
        assert_eq!(gui.qscroll, 2, "and the offset is written back clamped");
    }

    /// The panel with a cover decoded, same eyeball as the shell's:
    /// `cargo test dump_queue_panel -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_queue_panel() {
        let mut gui = gui_with(vec![
            queued("a.mp3", "Night Drive", Some("Moon Parade"), Some("aa.jpeg"), 252.0, HOME),
            queued("b.mp3", "Aurora Over the Long Meadow Road", Some("The Very Long Band Name Ensemble"), None, 228.0, HOME),
            queued("c.mp3", "Cassini IV", Some("Orbital Drift"), Some("aa.jpeg"), 3725.0, HOME),
        ]);
        gui.app.queue.current = Some(1);
        gui.app.art.insert("aa.jpeg".into(), Some(solid_cover()));
        for row in lines(&draw(&mut gui)) {
            println!("|{row}|");
        }
    }
}
