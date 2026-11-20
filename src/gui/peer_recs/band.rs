//! The Peer recs room's row 6 in its two forms (peer-recs contract §1.6, §1.8).
//!
//! At rest it is the verbs line: `▸ preview all` in the accent, then ` · ` and `copy list`
//! dim, or `copy list` alone when the room offers no preview. While a clip session lives the
//! band takes the row: the toggle, `▸▸` and the clip's title and artist on the left, and on the
//! right, ending at the content's last cell, the 10-cell clip bar, the catalogue's attribution,
//! `[ get it ]` and `[X]`.
//!
//! The band's fit is a pure plan ([`band_plan`]). When the text would get fewer than 16 cells
//! the parts drop in this order: the bar, then `[ get it ]`, then the text clips down to 8
//! cells, then `▸▸`. The attribution and `[X]` never drop: a clip never plays out of sight of
//! its attribution. The artist drops whole when the clip would keep fewer than 4 of its cells,
//! measured on the unclipped widths, before the kit's `clip` runs.
//!
//! The band draws from the App's `BandFacts` alone, so it still draws when the clip's lead has
//! left the shown list (build plan D30). Its clicks are the room's acts: the shell routes them
//! back to the room, which ends, toggles or moves the clip.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::kit::theme::{busy_glyph, legacy_conhost, th};
use crate::kit::{Surface, dim};
use crate::tui::app::peer_recs::previews::{BandFacts, CLIP_SECONDS, Slot};

use super::super::bar::{clip, play_glyphs};
use super::super::{Act, bright_bold, forward_glyph, put, text_button_in};
use super::PeerAct;

/// The toggle's slot: `▮▮`, `▸` or `◌`, at the content's first cell.
const TOGGLE_W: u16 = 2;
/// `▸▸`, one cell after the toggle.
const NEXT_X: u16 = 3;
/// Where the text starts with `▸▸` standing, and once it has dropped.
const TEXT_X: u16 = 6;
const TEXT_X_BARE: u16 = 3;
/// The cells between the text's end and the right cluster.
const TEXT_GAP: u16 = 2;
/// The clip bar's cells, and the cells after it.
const BAR_W: u16 = 10;
const BAR_GAP: u16 = 2;
/// The cell after the attribution, and after `[ get it ]`.
const GAP: u16 = 1;
/// The kit's close, `[X]`.
const CLOSE: &str = "[X]";
const CLOSE_W: u16 = 3;
/// Below this the parts drop (the bar, then `[ get it ]`); below the floor `▸▸` drops too.
const TEXT_WANT: u16 = 16;
const TEXT_FLOOR: u16 = 8;
/// Fewer of the artist's cells than this and ` · artist` drops whole.
const ARTIST_MIN: i32 = 4;
/// Between the title and the artist, and between the verbs.
const SEP: &str = " · ";
const SEP_W: u16 = 3;

/// Where the band's parts stand, every x an offset from the content's first cell. A part that
/// does not stand keeps an x of 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct BandPlan {
    /// The clip bar stands: the clip is answered and the text keeps its 16 cells with it.
    pub bar: bool,
    /// `[ get it ]` stands: the gate holds and the text keeps its 16 cells with it.
    pub get_it: bool,
    /// `▸▸` stands: the text keeps its 8 cells with it.
    pub next: bool,
    /// The text's start and its cells, ending 2 cells before the cluster.
    pub text: (u16, u16),
    /// ` · artist` stands after the title.
    pub artist: bool,
    pub bar_x: u16,
    pub attribution_x: u16,
    /// The attribution's cells: all of them, unless even the bare band leaves it less (it is
    /// then clipped, never dropped); 0 before the resolve or when the catalogue sent none.
    pub attribution_w: u16,
    pub get_it_x: u16,
    pub close_x: u16,
}

/// The band's fit in `width` cells (peer-recs contract §1.8).
///
/// - `attribution_w`: the catalogue's words, in cells (0: none).
/// - `get_it`: `[ get it ]`'s cells in the room's language, when the Get it gate holds.
/// - `title_w`, `artist_w`: the unclipped widths (0: no artist).
/// - `answered`: the resolve has answered. Before it there is no bar and no attribution.
///
/// The cluster is laid right to left from the last cell: `[X]`, then `[ get it ]` and a cell,
/// then the attribution and a cell, then the bar and 2 cells. The text runs from x 6 (x 3 once
/// `▸▸` has dropped) to 2 cells before it.
pub(super) fn band_plan(
    width: u16,
    attribution_w: u16,
    get_it: Option<u16>,
    title_w: u16,
    artist_w: u16,
    answered: bool,
) -> BandPlan {
    let attribution_w = if answered { attribution_w } else { 0 };
    let cluster = |bar: bool, get_it: Option<u16>, attribution_w: u16| -> i32 {
        let mut cells = i32::from(CLOSE_W);
        if let Some(button) = get_it {
            cells += i32::from(button) + i32::from(GAP);
        }
        if attribution_w > 0 {
            cells += i32::from(attribution_w) + i32::from(GAP);
        }
        if bar {
            cells += i32::from(BAR_W) + i32::from(BAR_GAP);
        }
        cells
    };
    let room = |next: bool, cluster: i32| -> i32 {
        let from = if next { TEXT_X } else { TEXT_X_BARE };
        i32::from(width) - i32::from(from) - i32::from(TEXT_GAP) - cluster
    };
    let (mut bar, mut get_it, mut next) = (answered, get_it, true);
    let want = i32::from(TEXT_WANT);
    if bar && room(next, cluster(bar, get_it, attribution_w)) < want {
        bar = false;
    }
    if get_it.is_some() && room(next, cluster(bar, get_it, attribution_w)) < want {
        get_it = None;
    }
    if room(next, cluster(bar, get_it, attribution_w)) < i32::from(TEXT_FLOOR) {
        next = false;
    }
    // Only a band narrower than any the GUI draws (its floor is 47 cells) gets here with the
    // attribution too wide for it: the attribution clips, and the text gets nothing.
    let short = (-room(next, cluster(bar, get_it, attribution_w))).max(0);
    let attribution_w = attribution_w.saturating_sub(u16::try_from(short).unwrap_or(u16::MAX));

    let close_x = width.saturating_sub(CLOSE_W);
    let mut left = close_x;
    let get_it_x = match get_it {
        Some(button) => {
            left = left.saturating_sub(button + GAP);
            left
        }
        None => 0,
    };
    let attribution_x = if attribution_w > 0 {
        left = left.saturating_sub(attribution_w + GAP);
        left
    } else {
        0
    };
    let bar_x = if bar {
        left = left.saturating_sub(BAR_W + BAR_GAP);
        left
    } else {
        0
    };
    let from = if next { TEXT_X } else { TEXT_X_BARE };
    let text = (from, left.saturating_sub(TEXT_GAP).saturating_sub(from));
    BandPlan {
        bar,
        get_it: get_it.is_some(),
        next,
        text,
        artist: artist_stands(text.1, title_w, artist_w),
        bar_x,
        attribution_x,
        attribution_w,
        get_it_x,
        close_x,
    }
}

/// ` · artist` stands in `cells` after a title `title_w` wide when it fits whole, or when the
/// kit's clip (the cells that fit less one, then `…`) would keep at least 4 of its cells.
fn artist_stands(cells: u16, title_w: u16, artist_w: u16) -> bool {
    if artist_w == 0 {
        return false;
    }
    let before = i32::from(title_w) + i32::from(SEP_W);
    if before + i32::from(artist_w) <= i32::from(cells) {
        return true;
    }
    i32::from(cells) - 1 - before >= ARTIST_MIN
}

/// The clip bar's played cells of 10: `round(position / duration × 10)`, over 30 s when the
/// length is unknown (peer-recs contract §1.8).
pub(super) fn clip_cells(position: f64, duration: f64) -> u16 {
    let share = played(position, duration);
    (share * f64::from(BAR_W)).round() as u16
}

/// The share of the clip played, 0 to 1; over 30 s when the length is unknown.
fn played(position: f64, duration: f64) -> f64 {
    let duration = if duration.is_finite() && duration > 0.0 { duration } else { CLIP_SECONDS };
    let share = position / duration;
    if share.is_finite() { share.clamp(0.0, 1.0) } else { 0.0 }
}

fn cells(text: &str) -> u16 {
    u16::try_from(crate::kit::width(text)).unwrap_or(u16::MAX)
}

/// `[ get it ]` in the room's language: the kit's bracketed form, with its inner spaces.
fn get_it_label() -> String {
    format!("[ {} ]", t!("gui.peer_recs.get_it_band"))
}

/// The live clip's band on row `y` (peer-recs contract §1.8). `get_it` is the lead's Get it
/// gate. It registers `Toggle`, `Next`, `Attribution` (only with an https link), `GetIt` and
/// `Close`.
pub(super) fn draw_band(
    frame: &mut Frame,
    ui: &mut Surface<Act>,
    content: Rect,
    y: u16,
    band: &BandFacts,
    get_it: bool,
) {
    let x = content.x;
    let artist = band.artist.as_deref().filter(|artist| !artist.is_empty());
    let attribution = band.attribution.as_deref().unwrap_or("");
    // A sounding clip was answered; one still on its way is answered once its words came.
    let answered = band.attribution.is_some() || matches!(band.slot, Slot::Playing | Slot::Paused);
    let button = get_it_label();
    let plan = band_plan(
        content.width,
        cells(attribution),
        get_it.then(|| cells(&button)),
        cells(&band.title),
        artist.map_or(0, cells),
        answered,
    );

    // The toggle: `▮▮` accent BOLD sounding, `▸` paused, `◌` gold on its way. The band is never
    // drawn for an idle or muted slot; those read as paused.
    let strong = Style::default().fg(th().accent).add_modifier(Modifier::BOLD);
    let (glyph, rest) = match band.slot {
        Slot::Busy => (busy_glyph(), Style::default().fg(th().gold)),
        Slot::Playing => (play_glyphs(false).1, strong),
        Slot::Paused | Slot::Idle | Slot::Muted => (play_glyphs(true).1, strong),
    };
    let rect = Rect { x, y, width: TOGGLE_W.min(content.width), height: 1 };
    put(frame, x, y, glyph, if ui.hovers(rect) { bright_bold() } else { rest });
    ui.click(rect, Act::Peer(PeerAct::Toggle));

    if plan.next {
        let next = play_glyphs(false).2;
        text_button_in(frame, ui, x + NEXT_X, y, next, false, Act::Peer(PeerAct::Next));
    }

    let (text_x, text_w) = plan.text;
    if text_w > 0 {
        let at = x + text_x;
        match artist.filter(|_| plan.artist) {
            Some(artist) => {
                // The title fits whole here: the artist stands only when it fits whole too, or
                // keeps 4 of its cells after the title.
                let title_w = cells(&band.title);
                put(frame, at, y, &band.title, Style::default());
                put(frame, at + title_w, y, SEP, dim());
                let room = text_w.saturating_sub(title_w + SEP_W);
                put(frame, at + title_w + SEP_W, y, &clip(artist, usize::from(room)), dim());
            }
            None => put(frame, at, y, &clip(&band.title, usize::from(text_w)), Style::default()),
        }
    }

    if plan.bar {
        draw_clip_bar(frame, x + plan.bar_x, y, band.position, band.duration);
    }

    if plan.attribution_w > 0 {
        let words = clip(attribution, usize::from(plan.attribution_w));
        let at = x + plan.attribution_x;
        // Dim and never underlined; a text button only when it has an https page to open.
        if band.link.is_some() {
            text_button_in(frame, ui, at, y, &words, false, Act::Peer(PeerAct::Attribution));
        } else {
            put(frame, at, y, &words, dim());
        }
    }

    if plan.get_it {
        text_button_in(frame, ui, x + plan.get_it_x, y, &button, false, Act::Peer(PeerAct::GetIt));
    }
    text_button_in(frame, ui, x + plan.close_x, y, CLOSE, false, Act::Peer(PeerAct::Close));
}

/// The clip bar: 10 cells, `▰` ok for the share played and `▱` dim to come. The legacy
/// console's fonts lack both, so it says the share in words, `NN%`, as Keep a copy's bar does.
fn draw_clip_bar(frame: &mut Frame, x: u16, y: u16, position: f64, duration: f64) {
    if legacy_conhost() {
        let pct = (played(position, duration) * 100.0).round() as u32;
        put(frame, x, y, &format!("{pct}%"), Style::default());
        return;
    }
    let filled = clip_cells(position, duration).min(BAR_W);
    put(frame, x, y, &"▰".repeat(usize::from(filled)), Style::default().fg(th().ok));
    put(frame, x + filled, y, &"▱".repeat(usize::from(BAR_W - filled)), dim());
}

/// The verbs line on row `y` (peer-recs contract §1.6): `▸ preview all` in the accent, ` · `
/// and `copy list` dim, from the content's first cell; `copy list` alone when `preview` is
/// false (no preview plug-in, or no audio device). It registers `PreviewAll` and `CopyList`,
/// and no tooltip: the room's only ones are `Add` and the header's (contract §1.2.9).
pub(super) fn draw_verbs(
    frame: &mut Frame,
    ui: &mut Surface<Act>,
    content: Rect,
    y: u16,
    preview: bool,
) {
    let mut x = content.x;
    if preview {
        let label = format!("{} {}", forward_glyph(), t!("gui.peer_recs.preview_all"));
        x += text_button_in(frame, ui, x, y, &label, true, Act::Peer(PeerAct::PreviewAll));
        put(frame, x, y, SEP, dim());
        x += SEP_W;
    }
    let label = t!("gui.peer_recs.copy_list");
    text_button_in(frame, ui, x, y, &label, false, Act::Peer(PeerAct::CopyList));
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Position;

    use super::*;
    use crate::api::types::{PluginPreview, fixtures};
    use crate::kit::boards::{Area, assert_frame};

    /// The content column at 100 cells: 80 cells from x 17, or 47 with the queue open.
    fn content(width: u16) -> Rect {
        Rect { x: 17, y: 2, width, height: 23 }
    }

    /// A resolve's answer from the stage's fixtures (`preview_deezer`, `preview_itunes`).
    fn preview(name: &str) -> PluginPreview {
        let answer = fixtures::preview(name);
        let plugin = answer["plugin"].as_str();
        PluginPreview::from_value(&answer["result"]["preview"], plugin).expect("a preview")
    }

    /// The band over a lead of the stage, with the catalogue's words from `name`'s fixture, or
    /// none before the resolve.
    fn facts(title: &str, artist: &str, slot: Slot, name: Option<&str>) -> BandFacts {
        let found = name.map(preview);
        BandFacts {
            index: Some(0),
            slot,
            title: title.into(),
            artist: Some(artist.into()),
            position: if found.is_some() { 12.0 } else { 0.0 },
            duration: CLIP_SECONDS,
            attribution: found.as_ref().and_then(|p| p.attribution.clone()),
            link: None,
        }
    }

    fn signal_fires(slot: Slot, name: &str) -> BandFacts {
        facts("Signal Fires", "Paper Lantern", slot, Some(name))
    }

    /// The band drawn on row 6 of a 100×30 screen, `width` cells from x 17.
    fn drawn(width: u16, band: &BandFacts, get_it: bool) -> (Buffer, Surface<Act>) {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut ui = Surface::new();
        terminal.draw(|frame| draw_band(frame, &mut ui, content(width), 6, band, get_it)).unwrap();
        (terminal.backend().buffer().clone(), ui)
    }

    fn verbs(width: u16, preview: bool) -> (Buffer, Surface<Act>) {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut ui = Surface::new();
        terminal.draw(|frame| draw_verbs(frame, &mut ui, content(width), 6, preview)).unwrap();
        (terminal.backend().buffer().clone(), ui)
    }

    fn row(buf: &Buffer, y: u16) -> String {
        span(buf, y, 0, buf.area.width - 1)
    }

    /// Row `y`'s text from x `from` to x `to`, as it reads: the cell a wide glyph covers after
    /// its own adds nothing.
    fn span(buf: &Buffer, y: u16, from: u16, to: u16) -> String {
        let mut text = String::new();
        let mut x = from;
        while x <= to {
            let symbol = buf[(x, y)].symbol();
            text.push_str(symbol);
            x += crate::kit::width(symbol).max(1) as u16;
        }
        text
    }

    /// The cells drawn past x `last` on row `y`.
    fn past(buf: &Buffer, y: u16, last: u16) -> String {
        (last + 1..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>().trim().to_string()
    }

    fn hit(ui: &Surface<Act>, x: u16) -> Option<Act> {
        ui.hit(Position { x, y: 6 })
    }

    #[test]
    fn reg_band_frames_match() {
        let _en = super::super::kit::english();
        crate::kit::theme::pin_modern_terminal();
        let quiet = facts("Quiet Engine", "Tidewater", Slot::Busy, None);
        let cases = [
            (1, signal_fires(Slot::Playing, "preview_deezer")),
            (2, signal_fires(Slot::Paused, "preview_deezer")),
            (3, quiet),
            (4, signal_fires(Slot::Playing, "preview_itunes")),
        ];
        for (mock, band) in cases {
            let (buf, _) = drawn(80, &band, true);
            assert_frame(&buf, "REG-BAND", "80", Area::at(6, mock));
            assert_eq!(past(&buf, 6, 96), "", "REG-BAND {mock:02} stays in the content");
        }
        let (buf, _) = drawn(47, &signal_fires(Slot::Playing, "preview_deezer"), true);
        assert_frame(&buf, "REG-BAND", "47", Area::at(6, 1));
        assert_eq!(past(&buf, 6, 63), "", "the 47-cell band stays in the content");
    }

    #[test]
    fn band_plan_drops_bar_then_get_it_then_text_then_next() {
        let (deezer, itunes) = (19, 27);
        let (title, artist) = (12, 13);
        let all = |text| BandPlan {
            bar: true,
            get_it: true,
            next: true,
            text: (6, text),
            artist: false,
            bar_x: 0,
            attribution_x: 0,
            attribution_w: 0,
            get_it_x: 66,
            close_x: 77,
        };
        // 80 cells: everything stands. Deezer leaves 26 cells of text; iTunes 18.
        let plan = band_plan(80, deezer, Some(10), title, artist, true);
        let deezer_80 =
            BandPlan { artist: true, bar_x: 34, attribution_x: 46, attribution_w: 19, ..all(26) };
        assert_eq!(plan, deezer_80);
        let plan = band_plan(80, itunes, Some(10), title, artist, true);
        assert_eq!(plan, BandPlan { bar_x: 26, attribution_x: 38, attribution_w: 27, ..all(18) });

        // 47 cells, the queue open: the bar drops (5 cells left), then [ get it ] (16 cells).
        let plan = band_plan(47, deezer, Some(10), title, artist, true);
        assert_eq!((plan.bar, plan.get_it, plan.next, plan.text), (false, false, true, (6, 16)));
        assert_eq!((plan.attribution_x, plan.close_x), (24, 44));
        // iTunes there: both drop, and the text clips down to its 8 cells with ▸▸ kept.
        let plan = band_plan(47, itunes, Some(10), title, artist, true);
        assert_eq!((plan.bar, plan.get_it, plan.next, plan.text), (false, false, true, (6, 8)));
        // Past the floor ▸▸ drops, and the text starts where it stood.
        let plan = band_plan(47, 30, Some(10), title, artist, true);
        assert_eq!((plan.bar, plan.get_it, plan.next, plan.text), (false, false, false, (3, 8)));
        assert_eq!((plan.attribution_x, plan.attribution_w, plan.close_x), (13, 30, 44));

        // A long locale's [ get it ] (16 cells, the most any may take) at 80 cells with iTunes:
        // the bar drops first, though the button is the wider part.
        let plan = band_plan(80, itunes, Some(16), title, artist, true);
        assert_eq!((plan.bar, plan.get_it, plan.text), (false, true, (6, 24)));
        assert_eq!((plan.get_it_x, plan.attribution_x), (60, 32));

        // From 80 cells down to the floor's 47 the parts go in that order, and never come back.
        for (attribution, button) in [(deezer, 10), (itunes, 10), (deezer, 16), (itunes, 16)] {
            let mut was = band_plan(80, attribution, Some(button), title, artist, true);
            for width in (47..80).rev() {
                let plan = band_plan(width, attribution, Some(button), title, artist, true);
                let gone = |then: bool, now: bool| then || !now;
                assert!(gone(was.bar, plan.bar) && gone(was.get_it, plan.get_it), "{width}: {plan:?}");
                assert!(gone(was.next, plan.next), "{width}: {plan:?}");
                assert!(plan.bar <= plan.get_it, "{width}: the bar goes before [ get it ]: {plan:?}");
                assert!(plan.get_it <= plan.next, "{width}: [ get it ] goes before ▸▸: {plan:?}");
                if plan.bar || plan.get_it {
                    assert!(plan.text.1 >= TEXT_WANT, "{width}: a part stood with less text: {plan:?}");
                }
                if plan.next {
                    assert!(plan.text.1 >= TEXT_FLOOR, "{width}: ▸▸ stood with less text: {plan:?}");
                }
                was = plan;
            }
        }
    }

    #[test]
    fn the_artist_drops_whole_below_4_cells() {
        // With no bar, no attribution and no button the text has width − 11 cells.
        let title = 12;
        // 20 cells: "Signal Fires · " then 4 of the artist's cells and `…`.
        assert!(band_plan(31, 0, None, title, 13, false).artist);
        // 19 cells: only 3 of the artist's cells would stand, so it drops whole.
        assert!(!band_plan(30, 0, None, title, 13, false).artist);
        // An artist that fits whole stands, however short.
        assert!(band_plan(30, 0, None, title, 2, false).artist);
        assert!(!band_plan(30, 0, None, title, 0, false).artist, "no artist, nothing to stand");
        // A title too long for the text drops the artist and clips alone.
        assert!(!band_plan(30, 0, None, 40, 2, false).artist);

        // Drawn, the title stands alone and the cells to the cluster stay blank.
        let _en = super::super::kit::english();
        crate::kit::theme::pin_modern_terminal();
        let (buf, _) = drawn(80, &signal_fires(Slot::Playing, "preview_itunes"), true);
        let line = row(&buf, 6);
        assert!(!line.contains("Paper") && !line.contains('·'), "{line}");
        assert_eq!(span(&buf, 6, 17, 52), "▮▮ ▸▸ Signal Fires        ▰▰▰▰▱▱▱▱▱▱", "{line}");
        // A title longer than its room clips with the kit's `…`.
        let band = facts("Lighthouse Keeper's Long Goodbye Waltz", "Paper Lantern", Slot::Playing,
            Some("preview_itunes"));
        let (buf, _) = drawn(80, &band, true);
        let line = row(&buf, 6);
        assert_eq!(span(&buf, 6, 23, 42), "Lighthouse Keeper…  ", "{line}");
        assert!(!line.contains("Paper"), "{line}");
    }

    #[test]
    fn the_attribution_and_close_never_drop() {
        // 38 cells is the most the 47-cell band holds whole, with no text beside it.
        for attribution in [19, 27, 38] {
            for button in [None, Some(10), Some(16)] {
                for width in 47..=80 {
                    let plan = band_plan(width, attribution, button, 12, 13, true);
                    assert_eq!(plan.attribution_w, attribution, "{width}: {plan:?}");
                    assert_eq!(plan.close_x, width - 3, "{width}: [X] ends at the last cell");
                    let end = plan.get_it.then_some(plan.get_it_x).unwrap_or(plan.close_x);
                    assert_eq!(plan.attribution_x + attribution + 1, end, "{width}: {plan:?}");
                }
            }
        }
        // Narrower than the GUI ever draws, a long attribution clips rather than drops, and the
        // text gives way first.
        let plan = band_plan(40, 60, Some(10), 12, 13, true);
        assert_eq!((plan.next, plan.text.1, plan.close_x), (false, 0, 37));
        assert_eq!((plan.attribution_x, plan.attribution_w), (5, 31));

        // Drawn at 47 cells, the queue open, with an https page: the attribution opens it.
        let _en = super::super::kit::english();
        crate::kit::theme::pin_modern_terminal();
        let mut band = signal_fires(Slot::Playing, "preview_deezer");
        band.link = Some("https://www.deezer.com/track/3995003".into());
        let (buf, ui) = drawn(47, &band, true);
        let want = "▮▮ ▸▸ Signal Fires      Preview from Deezer [X]";
        assert_eq!(span(&buf, 6, 17, 63), want, "{}", row(&buf, 6));
        assert_eq!(hit(&ui, 41), Some(Act::Peer(PeerAct::Attribution)));
        assert_eq!(hit(&ui, 63), Some(Act::Peer(PeerAct::Close)));
    }

    #[test]
    fn before_a_resolve_there_is_no_bar_or_attribution() {
        let plan = band_plan(80, 19, Some(10), 12, 9, false);
        assert!(!plan.bar && plan.attribution_w == 0, "{plan:?}");
        assert_eq!((plan.get_it_x, plan.close_x, plan.text), (66, 77, (6, 58)));

        let _en = super::super::kit::english();
        crate::kit::theme::pin_modern_terminal();
        let (buf, ui) = drawn(80, &facts("Quiet Engine", "Tidewater", Slot::Busy, None), true);
        let line = row(&buf, 6);
        assert!(!line.contains('▰') && !line.contains('▱') && !line.contains("Deezer"), "{line}");
        assert_eq!(hit(&ui, 17), Some(Act::Peer(PeerAct::Toggle)), "◌ drops a clip on its way");
        // Still on its way once the catalogue answered, the clip shows its words and its bar.
        let mut opening = facts("Quiet Engine", "Tidewater", Slot::Busy, Some("preview_deezer"));
        opening.position = 0.0;
        let (buf, _) = drawn(80, &opening, true);
        let line = row(&buf, 6);
        assert!(line.contains("▱▱▱▱▱▱▱▱▱▱  Preview from Deezer [ get it ] [X]"), "{line}");
        assert!(line.starts_with(&format!("{:17}◌  ▸▸ Quiet Engine · Tidewater ", "")), "{line}");
    }

    #[test]
    fn the_clip_bar_counts_tenths_of_30_s_when_the_length_is_unknown() {
        assert_eq!(clip_cells(12.0, 30.0), 4);
        assert_eq!(clip_cells(0.0, 30.0), 0);
        assert_eq!(clip_cells(30.0, 30.0), 10);
        assert_eq!(clip_cells(45.0, 30.0), 10, "past the end it stays full");
        assert_eq!(clip_cells(-1.0, 30.0), 0);
        assert_eq!(clip_cells(14.0, 60.0), 2, "a longer clip counts its own tenths");
        // Unknown lengths count over 30 s.
        for unknown in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            assert_eq!(clip_cells(12.0, unknown), 4, "{unknown}");
        }
        assert_eq!(clip_cells(f64::NAN, 30.0), 0);

        let _en = super::super::kit::english();
        crate::kit::theme::pin_modern_terminal();
        let mut band = signal_fires(Slot::Playing, "preview_deezer");
        band.duration = 0.0;
        band.position = 27.0;
        let (buf, _) = drawn(80, &band, true);
        assert!(row(&buf, 6).contains("▰▰▰▰▰▰▰▰▰▱  Preview"), "{}", row(&buf, 6));
    }

    #[test]
    fn the_legacy_console_draws_words_and_stand_ins() {
        let _en = super::super::kit::english();
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let (buf, _) = drawn(80, &signal_fires(Slot::Playing, "preview_deezer"), true);
        let want =
            format!("|| >> Signal Fires · Paper Lant»  {:10}  Preview from Deezer [ get it ] [X]", "40%");
        assert_eq!(span(&buf, 6, 17, 96), want, "{}", row(&buf, 6));
        let (buf, _) = drawn(80, &signal_fires(Slot::Paused, "preview_deezer"), true);
        assert!(row(&buf, 6)[17..].starts_with(">  >> Signal Fires"), "{}", row(&buf, 6));
        let (buf, _) = drawn(80, &facts("Quiet Engine", "Tidewater", Slot::Busy, None), true);
        assert!(row(&buf, 6)[17..].starts_with("~  >> Quiet Engine · Tidewater "), "{}", row(&buf, 6));
        let (buf, _) = verbs(80, true);
        assert!(row(&buf, 6)[17..].starts_with("> preview all · copy list "), "{}", row(&buf, 6));
    }

    #[test]
    fn the_verbs_line_is_copy_list_alone_without_preview() {
        let _en = super::super::kit::english();
        crate::kit::theme::pin_modern_terminal();
        // M1's row 6, with a preview; V2's (NO PREVIEW PLUG-IN), without.
        let (buf, ui) = verbs(80, true);
        assert_frame(&buf, "M1", "", Area::rect(17, 96, 6, 6));
        assert_eq!(hit(&ui, 17), Some(Act::Peer(PeerAct::PreviewAll)));
        assert_eq!(hit(&ui, 29), Some(Act::Peer(PeerAct::PreviewAll)));
        assert_eq!(hit(&ui, 31), None, "the separator presses nothing");
        assert_eq!(hit(&ui, 33), Some(Act::Peer(PeerAct::CopyList)));
        // No tooltip: the room's only ones are `Add` and the header's (contract §1.2.9).
        assert!(ui.tips.is_empty(), "{:?}", ui.tips);

        let (buf, ui) = verbs(80, false);
        assert_frame(&buf, "V2", "", Area::rect(17, 96, 6, 6));
        assert_eq!(hit(&ui, 17), Some(Act::Peer(PeerAct::CopyList)));
        assert!(ui.clicks.iter().all(|(_, act)| *act != Act::Peer(PeerAct::PreviewAll)));
        assert!(ui.tips.is_empty(), "{:?}", ui.tips);
        // The queue open draws the same line in its 47 cells (V3's row 6).
        let (buf, _) = verbs(47, true);
        assert_frame(&buf, "V3", "", Area::rect(17, 63, 6, 6));
    }

    #[test]
    fn the_band_registers_its_acts() {
        let _en = super::super::kit::english();
        crate::kit::theme::pin_modern_terminal();
        let mut band = signal_fires(Slot::Playing, "preview_deezer");
        let (_, ui) = drawn(80, &band, true);
        let peer = |act| Some(Act::Peer(act));
        assert_eq!((hit(&ui, 17), hit(&ui, 18)), (peer(PeerAct::Toggle), peer(PeerAct::Toggle)));
        assert_eq!((hit(&ui, 20), hit(&ui, 21)), (peer(PeerAct::Next), peer(PeerAct::Next)));
        assert_eq!(hit(&ui, 30), None, "the text presses nothing");
        assert_eq!(hit(&ui, 55), None, "the bar presses nothing");
        assert_eq!(hit(&ui, 70), None, "no https page, no link to open");
        assert_eq!((hit(&ui, 83), hit(&ui, 92)), (peer(PeerAct::GetIt), peer(PeerAct::GetIt)));
        assert_eq!((hit(&ui, 94), hit(&ui, 96)), (peer(PeerAct::Close), peer(PeerAct::Close)));
        band.link = Some("https://www.deezer.com/track/3995003".into());
        let (_, ui) = drawn(80, &band, true);
        let opens = peer(PeerAct::Attribution);
        assert_eq!((hit(&ui, 63), hit(&ui, 81)), (opens.clone(), opens));
        // Without the Get it gate there is no button, and its cells read blank.
        let (buf, ui) = drawn(80, &band, false);
        assert!(ui.clicks.iter().all(|(_, act)| *act != Act::Peer(PeerAct::GetIt)));
        assert_eq!(span(&buf, 6, 74, 96), "Preview from Deezer [X]", "{}", row(&buf, 6));
        // The buttons are bright BOLD under the pointer, dim at rest.
        let mut ui = Surface::new();
        ui.pointer = Some(Position { x: 95, y: 6 });
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw_band(frame, &mut ui, content(80), 6, &band, true)).unwrap();
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(95, 6)].fg, th().bright);
        assert!(buf[(95, 6)].modifier.contains(Modifier::BOLD));
        assert_eq!(buf[(85, 6)].fg, th().dim, "[ get it ] rests dim");
    }

    #[test]
    fn the_band_fits_at_47_cells_in_every_locale() {
        let codes = ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"];
        let Some(code) = crate::setup::tests::in_own_process(&codes) else { return };
        let button = get_it_label();
        for name in ["preview_deezer", "preview_itunes"] {
            let band = signal_fires(Slot::Playing, name);
            let words = band.attribution.clone().expect("the catalogue's words");
            // 80 cells: the words and the button stand, in the content, and the bar with them
            // while the button leaves the text its 16 cells (all but iTunes beside Spanish's
            // `[ conseguir ]`, whose bar drops first).
            let (buf, _) = drawn(80, &band, true);
            let line = row(&buf, 6);
            assert!(line.contains(&format!("{words} {button} [X]")), "{code}: {line}");
            let room = 80 - 6 - 2 - (3 + cells(&button) + 1 + cells(&words) + 1 + 10 + 2);
            assert_eq!(line.contains('▰'), room >= 16, "{code}: {line}");
            assert_eq!(past(&buf, 6, 96), "", "{code}: {line}");
            // 47 cells: the toggle, ▸▸, the title (clipped to its 8 cells beside iTunes), the
            // words and [X] stand, in the content.
            let (buf, _) = drawn(47, &band, true);
            let line = row(&buf, 6);
            assert!(line[17..].starts_with("▮▮ ▸▸ Signal"), "{code}: {line}");
            assert!(line.contains(&format!("{words} [X]")), "{code}: {line}");
            assert_eq!(past(&buf, 6, 63), "", "{code}: {line}");
            // The verbs line fits there too.
            let (buf, _) = verbs(47, true);
            assert_eq!(past(&buf, 6, 63), "", "{code}: {}", row(&buf, 6));
        }
    }
}
