//! The bar's face, headless, on the GUI's own fixtures: the frame drawn by
//! the cells first with the bar's face forced on (no window, so the frame
//! runs no pass of its own, and what lights is the cells registry's reading
//! of the pointer), then the face run in a context of its own over the
//! bar's rows, as the chrome's pass runs it. Every relation is measured
//! from the face in use, never from one platform's metrics.

use egui::epaint::{ClippedShape, ColorMode};
use egui::{FullOutput, ViewportId};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event as TermEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use super::*;
use crate::api::types::{Track, TrackMetadata};
use crate::config::Config;
use crate::gui::bar::cell_fraction;
use crate::gui::{Faces, Gui, List, RowVerb, bar_view, demo_now, over_control, playing_art, render};
use crate::tui::app::{App, Entry, Message, MessageKind, Repeat};

/// The demo seat playing (0:47 of 5:02) at 70 %, the bar's face alone
/// forced on.
fn bar_gui() -> Gui {
    let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
    gui.demo = Some(demo_now());
    gui.app.volume = 0.7;
    gui.chrome.forced = Some(Faces { bar: true, ..Faces::default() });
    gui
}

fn track(name: &str, seconds: f64) -> Track {
    Track {
        filepath: format!("music/{name}.mp3"),
        metadata: TrackMetadata { title: Some(name.to_string()), duration: Some(seconds), ..TrackMetadata::default() },
    }
}

/// [`bar_gui`] on a session, three tracks queued and the second playing,
/// at a volume no segment sets: every press on the bar then changes
/// something a test can see.
fn queued_gui() -> Gui {
    let mut gui = bar_gui();
    gui.app.connected = true;
    gui.app.session.server = "http://host:3000".into();
    gui.app.session.server_id = "http://host:3000".into();
    let rows = ["Aurora", "Night Drive", "Vela"].map(|name| Entry::Track { label: name.into(), track: Box::new(track(name, 240.0)) });
    gui.app.files.set(Vec::from(rows));
    for row in 0..3 {
        gui.act(Act::PaneRow(List::Files, row, RowVerb::Queue));
    }
    gui.act(Act::QueueRow(1));
    gui.app.status = crate::player::PlayerStatus { playing: true, source: "x".into(), ..Default::default() };
    gui.app.volume = 0.72;
    gui.pending.clear();
    gui
}

/// What a press on the bar can change, said whole.
fn state(gui: &Gui) -> String {
    format!(
        "repeat {:?} · shuffle {} · current {:?} · volume {:.3} · queue {} · demo {} {:?} · quick {} · dj {} · sheet {} · note {:?} · effects {}",
        gui.app.queue.repeat,
        gui.app.queue.shuffle,
        gui.app.queue.current,
        gui.app.volume,
        gui.queue_open,
        gui.demo_paused,
        gui.demo.as_ref().map(|demo| demo.elapsed),
        gui.dj.quick.is_some(),
        gui.app.dj_armed(),
        crate::gui::actions::modal_open(gui),
        gui.app.message.as_ref().map(|m| m.text.clone()),
        gui.pending.len(),
    )
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    crate::gui::window::panel::fonts(&ctx);
    ctx
}

/// One frame of the bar: its shapes, the layout the cells registered and
/// the face painted, where the cells stand, and what egui asked of the host.
struct Painted {
    shapes: Vec<ClippedShape>,
    layout: BarLayout,
    cells: Cells,
    out: FullOutput,
}

/// `gui` drawn by the cells at `w`×`h`, then the face's pass over the bar's
/// rows in `ctx`, the cells' edges rounded at `ppp` pixels a point.
fn paint_in(ctx: &egui::Context, gui: &mut Gui, w: u16, h: u16, ppp: f32) -> Painted {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|frame| render(frame, gui)).unwrap();
    let grid = Rect::new(0, 0, w, h);
    let points = egui::Rect::from_min_size(Pos2::ZERO, vec2(f32::from(w) * 8.0, f32::from(h) * 16.0));
    let cells = Cells { points, grid, ppp };
    let now = gui.bar_now();
    let mut view = bar_view(gui, now.as_ref());
    view.art = playing_art(&gui.app);
    let layout = crate::gui::bar::layout(grid, &view);
    let pal = Pal::now();
    let cx = BarCtx { cells, hot: gui.chrome.hot.clone(), pal: &pal };
    let input = egui::RawInput { screen_rect: Some(points), ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| {
        let rect = cells.rect(layout.rect);
        let mut part = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        part.set_clip_rect(rect);
        let _ = studio::begin(&part);
        show(&mut part, &view, &layout, &cx);
    });
    out.textures_delta.clear();
    Painted { shapes: std::mem::take(&mut out.shapes), layout, cells, out }
}

/// [`paint_in`] at the window's opening size, in a context of its own.
fn paint(gui: &mut Gui) -> Painted {
    paint_in(&context(), gui, 100, 30, 1.0)
}

impl Painted {
    /// Every text drawn: its words, its first run's colour and face, and
    /// where its galley stands.
    fn texts(&self) -> Vec<(String, Color32, FontId, egui::Rect)> {
        self.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => {
                    let first = text.galley.job.sections.first()?;
                    let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                    Some((text.galley.text().to_string(), first.format.color, first.format.font_id.clone(), rect))
                }
                _ => None,
            })
            .collect()
    }

    /// Where `words` were drawn, in what colour and face.
    fn drawn(&self, words: &str) -> (Color32, FontId, egui::Rect) {
        let texts = self.texts();
        let found = texts.iter().find(|(s, ..)| s == words).map(|(_, color, font, rect)| (*color, font.clone(), *rect));
        found.unwrap_or_else(|| panic!("{words:?} drawn: {:?}", texts.iter().map(|(s, ..)| s.as_str()).collect::<Vec<_>>()))
    }

    fn has(&self, words: &str) -> bool {
        self.texts().iter().any(|(s, ..)| s == words)
    }

    /// The rects filled in `color`.
    fn filled(&self, color: Color32) -> Vec<egui::Rect> {
        self.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Rect(r) if r.fill == color && r.fill_texture_id() == egui::TextureId::default() => Some(r.rect),
                _ => None,
            })
            .collect()
    }

    /// The rects outlined, with their strokes.
    fn outlined(&self) -> Vec<(egui::Rect, Stroke)> {
        self.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Rect(r) if r.stroke.width > 0.0 => Some((r.rect, r.stroke)),
                _ => None,
            })
            .collect()
    }

    /// The stroke round `rect`.
    fn outline_of(&self, rect: egui::Rect) -> Option<Stroke> {
        self.outlined().into_iter().find(|(r, _)| *r == rect).map(|(_, stroke)| stroke)
    }

    /// The line strokes within `reach` points of `at`.
    fn strokes_near(&self, at: Pos2, reach: f32) -> Vec<([Pos2; 2], Stroke)> {
        self.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::LineSegment { points, stroke } if points.iter().all(|p| (*p - at).length() <= reach) => Some((*points, *stroke)),
                _ => None,
            })
            .collect()
    }

    /// The chevron drawn about `at` (`studio::chevron`'s two strokes, which
    /// meet at its point): whether it points up, and its colour.
    fn chevron(&self, at: Pos2) -> (bool, Color32) {
        let strokes = self.strokes_near(at, 6.0);
        let [(a, stroke), (b, _)] = strokes[..] else { panic!("one chevron at {at:?}: {strokes:?}") };
        let point = a.into_iter().find(|p| b.iter().any(|q| (*p - *q).length() < 0.01)).expect("the strokes meet");
        (point.y < at.y, stroke.color)
    }

    /// The cell a point of the paint stands on.
    fn cell(&self, at: Pos2) -> Position {
        self.cells.under(at)
    }
}

fn rect_of(p: &Painted, cells: Rect) -> egui::Rect {
    p.cells.rect(cells)
}

/// The control of the group whose act is `act`, and its cells.
fn control(layout: &BarLayout, act: &Act) -> Rect {
    layout.group.iter().find(|(_, a)| a == act).map(|(rect, _)| *rect).expect("in the group")
}

/// Where the rule between the split's halves stands.
fn rule_x(p: &Painted) -> f32 {
    rect_of(p, Rect { x: p.layout.dj.rule_x, width: 1, ..p.layout.dj.frame }).center().x
}

/// A press of `button` at `at` through the loop's input half, as the
/// window hands it the grid's.
fn press(gui: &mut Gui, at: Position, button: MouseButton) {
    let mut ctx = crate::gui::tests::quiet_ctx(gui);
    let event = MouseEvent { kind: MouseEventKind::Down(button), column: at.x, row: at.y, modifiers: KeyModifiers::NONE };
    let _ = crate::gui::input(gui, &mut ctx, TermEvent::Mouse(event));
}

/// The demo seat at rest: every line in its type, tone and box, as the Main
/// board sets it; the times at their slots' edges; the card's words from its
/// lines' left; the chevrons as the queue and quick config stand.
#[test]
fn the_bar_at_rest_draws_its_words_in_their_type_tone_and_cells() {
    let _en = crate::setup::tests::in_locale("en");
    let pal = Pal::now();
    let mut gui = bar_gui();
    let p = paint(&mut gui);
    let seek = p.layout.seek.expect("the seek row while the demo plays");
    let near = |a: f32, b: f32| (a - b).abs() <= 0.5;

    let (color, font, at) = p.drawn("0:47");
    assert_eq!((color, font), (pal.dim, studio::mono(12.0)), "the elapsed time");
    assert!(near(at.max.x, rect_of(&p, seek.elapsed).max.x), "at its slot's right: {at:?}");
    let (color, font, at) = p.drawn("5:02");
    assert_eq!((color, font), (pal.dim, studio::mono(12.0)), "the total");
    assert!(near(at.min.x, rect_of(&p, seek.total).min.x), "at its slot's left: {at:?}");
    // Its size is the face's to settle (the_auto_dj_word_stands_whole_in_its_half).
    let (color, font, _) = p.drawn("AUTO-DJ");
    assert_eq!((color, font.family), (pal.dim, studio::strong(12.5).family), "the cells' word in spaced capitals");
    let (color, font, at) = p.drawn("70%");
    assert_eq!((color, font), (pal.dim, studio::mono(12.0)), "the readout");
    assert!(near(at.max.x, rect_of(&p, p.layout.volume.readout).max.x), "at its slot's right: {at:?}");

    let lines = p.layout.card.lines.map(|line| rect_of(&p, line));
    for (words, color, font, line) in [
        ("Cassini IV", pal.text, studio::strong(13.0), lines[0]),
        ("Vela — Cassini · 2019", pal.dim, studio::body(12.5), lines[1]),
        ("FLAC · 912 kbps · 44.1 kHz", pal.dim, studio::mono(11.5), lines[2]),
    ] {
        let (drawn, face, at) = p.drawn(words);
        assert_eq!((drawn, face), (color, font), "{words}");
        assert!(near(at.min.x, line.min.x) && line.contains(at.center()), "{words} in its line {line:?}: {at:?}");
    }
    let (color, font, at) = p.drawn("♪ 8A · 120 BPM");
    assert_eq!((color, font), (pal.dim, studio::mono(11.5)), "the facts");
    assert!(lines[3].contains(at.center()) && at.min.x > lines[3].min.x, "after the stars, on the fourth line: {at:?}");

    // The queue open, its chevron down; quick config down, the split's up.
    let card_chevron = rect_of(&p, p.layout.card.chevron).center();
    assert_eq!(p.chevron(card_chevron), (false, pal.dim));
    let frame = framed(rect_of(&p, p.layout.dj.frame));
    assert_eq!(p.chevron(pos2((rule_x(&p) + frame.max.x) / 2.0, frame.center().y)), (true, pal.dim));
    // No note stands: nothing in its slot.
    let note = rect_of(&p, p.layout.note);
    assert!(p.texts().iter().all(|(_, _, _, at)| !note.contains(at.center())), "the note slot empty");
    // The bar's fill under all five rows, the seek row its edge.
    assert!(p.filled(pal.panel).contains(&rect_of(&p, p.layout.rect)), "the panel's fill");
    // The face takes no pointer: the hand over the bar is the cells'.
    assert_eq!(p.out.platform_output.cursor_icon, egui::CursorIcon::Default);
}

/// Each state's look: the play slot's triangle while paused and the pause
/// bars otherwise, nothing loaded included (ruling R14); the toggles in the
/// ok green while on, repeat One as All (R15); the split armed; the
/// chevrons for quick config up and the queue folded.
#[test]
fn each_state_wears_its_look() {
    let _en = crate::setup::tests::in_locale("en");
    let pal = Pal::now();
    let gold_inside = |p: &Painted, rect: egui::Rect| -> (usize, usize) {
        let bars = p.filled(pal.gold).into_iter().filter(|r| rect.contains_rect(*r)).count();
        let triangles = p
            .shapes
            .iter()
            .filter(|c| matches!(&c.shape, Shape::Path(path) if path.fill == pal.gold && path.points.iter().all(|q| rect.contains(*q))))
            .count();
        (bars, triangles)
    };
    let mut gui = bar_gui();
    let p = paint(&mut gui);
    let play = framed(rect_of(&p, control(&p.layout, &Act::PlayPause)));
    assert_eq!(gold_inside(&p, play), (2, 0), "playing: the pause bars");
    assert_eq!(p.outline_of(play), Some(Stroke::new(2.0, pal.gold)), "the primary's 2-point gold frame");
    gui.demo_paused = true;
    let p = paint(&mut gui);
    assert_eq!(gold_inside(&p, play), (0, 1), "paused: the triangle");
    gui.demo = None;
    gui.demo_paused = false;
    let p = paint(&mut gui);
    assert_eq!(gold_inside(&p, play), (2, 0), "nothing loaded: the pause bars, as the cells' slot (R14)");
    assert_eq!(framed(rect_of(&p, control(&p.layout, &Act::PlayPause))), play, "the slot keeps its width");

    let mut gui = bar_gui();
    for (repeat, shuffle) in [(Repeat::Off, false), (Repeat::All, true), (Repeat::One, false)] {
        gui.app.queue.repeat = repeat;
        gui.app.queue.shuffle = shuffle;
        let p = paint(&mut gui);
        let tone = |on: bool| if on { pal.ok } else { pal.edge_strong };
        for (act, on) in [(Act::Repeat, repeat != Repeat::Off), (Act::Shuffle, shuffle)] {
            let at = framed(rect_of(&p, control(&p.layout, &act)));
            assert_eq!(p.outline_of(at), Some(Stroke::new(1.0, tone(on))), "{act:?} with {repeat:?}, shuffle {shuffle}");
        }
        for act in [Act::Prev, Act::Next] {
            assert_eq!(p.outline_of(framed(rect_of(&p, control(&p.layout, &act)))), Some(Stroke::new(1.0, pal.edge_strong)));
        }
    }

    let mut gui = bar_gui();
    gui.app.dj_server = Some("http://host:3000".into());
    let p = paint(&mut gui);
    let frame = framed(rect_of(&p, p.layout.dj.frame));
    assert_eq!(p.outline_of(frame), Some(Stroke::new(1.0, pal.ok)), "armed: the frame ok");
    assert_eq!(p.drawn("AUTO-DJ").0, pal.ok, "and the word");
    assert!(p.filled(pal.ok).iter().any(|r| frame.contains_rect(*r) && r.width() == 6.0 && r.height() == 6.0), "and its LED lit");
    assert!(p.filled(pal.ok).iter().any(|r| r.width() == 1.0 && r.height() == frame.height() - 2.0), "and the rule between the halves");
    let chevron_at = pos2((rule_x(&p) + frame.max.x) / 2.0, frame.center().y);
    assert_eq!(p.chevron(chevron_at), (true, pal.dim), "the chevron never wears the armed green");

    gui.app.dj_server = None;
    gui.act(Act::DjQuick(QuickAct::Toggle));
    gui.queue_open = false;
    let p = paint(&mut gui);
    assert_eq!(p.chevron(chevron_at), (false, pal.text), "quick config up: down, lit");
    assert_eq!(p.chevron(rect_of(&p, p.layout.card.chevron).center()), (true, pal.dim), "the queue folded: up");
}

/// The split's word stands whole in its half whatever the face: at the
/// board's 12.5 points where the face's capitals fit beside the LED there
/// (Segoe UI's semibold does), else smaller, never cut to "AUTO-D…" as a
/// wide bold's (DejaVu Sans Bold, Linux's) is at 12.5 in the half's cells;
/// the LED its gap before it, both inside the half; at the opening size, the
/// floor and wide, scaled.
#[test]
fn the_auto_dj_word_stands_whole_in_its_half() {
    let _en = crate::setup::tests::in_locale("en");
    let pal = Pal::now();
    let ctx = context();
    let mut whole = 0.0;
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| whole = studio::caps_width(ui, "AUTO-DJ", WORD_SIZE));
    // The context's first pass hands over the font atlas: let go of it.
    out.textures_delta.clear();
    for (w, h, ppp) in [(100, 30, 1.0), (100, 24, 1.25), (160, 50, 2.0)] {
        let mut gui = bar_gui();
        let p = paint_in(&ctx, &mut gui, w, h, ppp);
        let frame = framed(rect_of(&p, p.layout.dj.frame));
        let rule = rule_x(&p);
        let word = p.shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(text) if text.galley.text() == "AUTO-DJ" => Some(text.clone()),
            _ => None,
        });
        let word = word.expect("the word");
        let set: String = word.galley.rows.iter().flat_map(|row| row.row.glyphs.iter().map(|glyph| glyph.chr)).collect();
        assert_eq!(set, "AUTO-DJ", "{w}×{h}: the word whole, in a face whose capitals take {whole} points at {WORD_SIZE}");
        let at = egui::Rect::from_min_size(word.pos, word.galley.size());
        assert!(at.max.x <= rule - 1.0, "{w}×{h}: short of the rule at {rule}: {at:?}");
        let lamps: Vec<egui::Rect> = p.filled(pal.unlit).into_iter().filter(|r| frame.contains_rect(*r) && r.size() == vec2(LAMP, LAMP)).collect();
        let [lamp] = lamps[..] else { panic!("{w}×{h}: one LED in the split: {lamps:?}") };
        assert!(lamp.min.x >= frame.min.x + 1.0, "{w}×{h}: the LED inside the frame: {lamp:?} in {frame:?}");
        assert!((at.min.x - (lamp.max.x + LAMP_GAP)).abs() <= 0.5, "{w}×{h}: the word {LAMP_GAP} points after the LED: {at:?}, {lamp:?}");
        let size = word.galley.job.sections[0].format.font_id.size;
        assert!(size <= WORD_SIZE, "{w}×{h}: never over the board's size: {size}");
        if whole + 20.0 <= rule - (frame.min.x + 1.0) {
            assert_eq!(size, WORD_SIZE, "{w}×{h}: the board's size where the face's capitals fit");
        }
    }
}

/// Idle: the gold hairline across the whole row, a column wider each side
/// than the seek (R16), no times and nothing to press, the empty cover's
/// hairline, the cells' words for nothing playing on the card's first line.
/// An unknown length: its time against 0:00, the thumb at the track's
/// start, no played span; past ten minutes its time whole, from its slot's
/// left into the gap before the track, as the cells write it.
#[test]
fn idle_and_an_unknown_length_draw_as_the_cells_treat_them() {
    let _en = crate::setup::tests::in_locale("en");
    let pal = Pal::now();
    let mut gui = bar_gui();
    gui.demo = None;
    let p = paint(&mut gui);
    assert!(p.layout.seek.is_none());
    let row = rect_of(&p, Rect { height: 1, ..p.layout.rect });
    let rule = egui::Rect::from_min_max(pos2(row.min.x, row.center().y - 1.0), pos2(row.max.x, row.center().y));
    assert!(p.filled(pal.gold).contains(&rule), "the gold hairline across the row: {:?}", p.filled(pal.gold));
    assert!(p.texts().iter().all(|(s, ..)| !s.contains(':')), "no times: {:?}", p.texts());
    let words = rust_i18n::t!("gui.nothing_playing").to_string();
    let (color, font, at) = p.drawn(&words);
    assert_eq!((color, font), (pal.dim, studio::body(12.5)));
    assert!(rect_of(&p, p.layout.card.lines[0]).contains(at.center()));
    let cover = rect_of(&p, p.layout.card.cover);
    assert_eq!(p.outline_of(cover), Some(Stroke::new(1.0, pal.edge_strong)), "the empty slot");

    let mut gui = bar_gui();
    gui.demo = Some(Now { duration: 0.0, elapsed: 72.0, rating: None, key: None, bpm: None, ..demo_now() });
    let p = paint(&mut gui);
    let track = rect_of(&p, p.layout.seek.expect("the seek row").track);
    assert!(p.has("1:12") && p.has("0:00"), "{:?}", p.texts());
    let thumb = p.filled(pal.gold).into_iter().find(|r| r.width() == studio::THUMB_W).expect("the thumb");
    assert!((thumb.min.x - track.min.x).abs() <= 0.5, "at the track's start: {thumb:?} on {track:?}");
    assert!(!p.has("♪ 8A · 120 BPM"), "no facts, no line");

    // Past ten minutes its time is wider than its slot, which is as wide as
    // "0:00": the cells write it on from the slot's left into the gap before
    // the track, and the face sets it there whole, not cut in the slot.
    gui.demo = Some(Now { duration: 0.0, elapsed: 754.0, ..demo_now() });
    let p = paint(&mut gui);
    let seek = p.layout.seek.expect("the seek row");
    let mut cells = bar_gui();
    cells.demo = gui.demo.clone();
    cells.chrome.forced = Some(Faces::default());
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| render(frame, &mut cells)).unwrap();
    let buffer = terminal.backend().buffer();
    let written: String = (seek.elapsed.x..seek.track.x).map(|x| buffer[(x, seek.elapsed.y)].symbol().to_string()).collect();
    assert_eq!(written, "12:34", "the cells write it from the slot's left to the track");
    let time = p.shapes.iter().find_map(|c| match &c.shape {
        Shape::Text(text) if text.galley.text() == "12:34" => Some(text.clone()),
        _ => None,
    });
    let time = time.expect("the elapsed time");
    let set: String = time.galley.rows.iter().flat_map(|row| row.row.glyphs.iter().map(|glyph| glyph.chr)).collect();
    assert_eq!(set, "12:34", "whole, every glyph drawn");
    let at = egui::Rect::from_min_size(time.pos, time.galley.size());
    let (slot, track) = (rect_of(&p, seek.elapsed), rect_of(&p, seek.track));
    assert!(at.width() > slot.width(), "wider than its slot in the monospace: {at:?} in {slot:?}");
    assert!((at.min.x - slot.min.x).abs() <= 0.5 && at.max.x <= track.min.x, "from the slot's left, short of the track at {}: {at:?}", track.min.x);
}

/// The seek row agrees with its times: the thumb at the share elapsed; the
/// pointer on a cell of the track lights the ghost thumb at the share a
/// press there seeks to and previews the time it lands at, the cells'
/// preview, in the text colour; the real thumb holds still. An unknown
/// length previews 0:00, as every press there seeks to it.
#[test]
fn the_seek_row_agrees_with_its_times_and_previews_the_press() {
    let _en = crate::setup::tests::in_locale("en");
    let pal = Pal::now();
    let mut gui = bar_gui();
    let p = paint(&mut gui);
    let seek = p.layout.seek.expect("the seek row");
    let track = rect_of(&p, seek.track);
    let thumbs = |p: &Painted| p.filled(pal.gold).into_iter().filter(|r| r.width() == studio::THUMB_W).collect::<Vec<_>>();
    let played = track.min.x + (47.0 / 302.0) * track.width();
    assert!(matches!(thumbs(&p)[..], [thumb] if (thumb.center().x - played).abs() < 0.5), "the thumb at 47 of 302: {:?}", thumbs(&p));
    assert!(p.outlined().iter().all(|(r, _)| r.width() != studio::THUMB_W), "no ghost at rest");

    let i = seek.cells * 3 / 5;
    gui.ui.pointer = Some(Position { x: seek.track.x + i as u16, y: seek.track.y });
    let p = paint(&mut gui);
    let frac = cell_fraction(i, seek.cells);
    assert_eq!(gui.chrome.hot.as_ref().map(|(_, act)| act.clone()), Some(Act::Seek(frac)), "the cell's seek lights");
    let ghost: Vec<(egui::Rect, Stroke)> = p.outlined().into_iter().filter(|(r, _)| r.width() == studio::THUMB_W).collect();
    assert!(
        matches!(ghost[..], [(r, stroke)] if stroke.color == pal.text && (r.center().x - (track.min.x + frac as f32 * track.width())).abs() < 0.5),
        "the ghost where the press lands: {ghost:?}"
    );
    let preview = fmt_time(frac * 302.0);
    let (color, _, at) = p.drawn(&preview);
    assert_eq!(color, pal.text, "the would-land time in the text colour");
    assert!((at.max.x - rect_of(&p, seek.elapsed).max.x).abs() <= 0.5, "in the elapsed slot");
    assert!(!p.has("0:47"), "in the elapsed time's stead");
    assert!(matches!(thumbs(&p)[..], [thumb] if (thumb.center().x - played).abs() < 0.5), "the real thumb holds still");

    gui.demo = Some(Now { duration: 0.0, ..demo_now() });
    let p = paint(&mut gui);
    assert_eq!(p.drawn("0:00").0, pal.text, "an unknown length previews 0:00: {:?}", p.texts());
}

/// The note slot: the shell's note before the App's, in the shell's tone;
/// the App's in its kind's tone, every one of the five; a long one cut with
/// an ellipsis inside the slot.
#[test]
fn the_note_slot_says_the_shells_note_first_and_each_tone_in_its_colour() {
    let _en = crate::setup::tests::in_locale("en");
    let pal = Pal::now();
    let mut gui = bar_gui();
    gui.app.message = Some(Message { text: "copying Borrowed Tapes…".into(), kind: MessageKind::Busy });
    gui.note = Some(("Queue saved".into(), false));
    let p = paint(&mut gui);
    let note = rect_of(&p, p.layout.note);
    let (color, _, at) = p.drawn("Queue saved");
    assert_eq!(color, pal.dim, "the shell's own note, dim");
    assert!(note.contains(at.center()) && (at.min.x - note.min.x).abs() <= 0.5, "in the slot: {at:?} of {note:?}");
    assert!(!p.has("copying Borrowed Tapes…"), "it outranks the App's");
    gui.note = Some(("the visualizer window could not open".into(), true));
    assert_eq!(paint(&mut gui).drawn("the visualizer window could not open").0, pal.gold, "the shell's error in the gold");
    gui.note = None;
    for (kind, color) in [
        (MessageKind::Info, pal.dim),
        (MessageKind::Error, pal.gold),
        (MessageKind::Busy, pal.accent),
        (MessageKind::Success, pal.ok),
        (MessageKind::Plain, pal.text),
    ] {
        gui.app.message = Some(Message { text: "repeat: all".into(), kind });
        assert_eq!(paint(&mut gui).drawn("repeat: all").0, color, "{kind:?}");
    }
    let long = "Sonic similarity is too strict for this library: nothing it would pick passes. Loosen the match slider to use it again.";
    gui.app.message = Some(Message { text: long.into(), kind: MessageKind::Info });
    let p = paint(&mut gui);
    let cut = p.shapes.iter().find_map(|c| match &c.shape {
        Shape::Text(text) if text.galley.text() == long => Some(text.clone()),
        _ => None,
    });
    let cut = cut.expect("the long note");
    assert!(cut.galley.size().x <= note.width() + 0.5, "inside the slot: {} of {}", cut.galley.size().x, note.width());
    let last = cut.galley.rows.last().and_then(|row| row.row.glyphs.last()).map(|glyph| glyph.chr);
    assert_eq!(last, Some('…'), "cut with an ellipsis");
}

/// Only what a press would work lights, as the cells registry reads the
/// pointer: each half of the split its own raised fill; each control of the
/// group its own, its frame and mark a step brighter; − and + their mark in
/// the text colour; a volume segment nothing (R17); the card its raised
/// fill, its byline and chevron in the text colour. The hand is the cells'
/// over every one of them, and over the readout, the times and the note
/// none.
#[test]
fn only_what_a_press_would_work_lights_under_the_pointer() {
    let _en = crate::setup::tests::in_locale("en");
    let pal = Pal::now();
    let ctx = context();
    let mut gui = bar_gui();
    gui.app.message = Some(Message { text: "repeat: all".into(), kind: MessageKind::Info });
    let rest = paint_in(&ctx, &mut gui, 100, 30, 1.0);
    assert!(rest.filled(pal.raised).is_empty(), "nothing lit at rest");
    let layout = rest.layout.clone();
    let frame = framed(rect_of(&rest, layout.dj.frame));
    let rule = rule_x(&rest);
    let at_cell = |rect: Rect| Position { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
    let segments = |p: &Painted| (p.filled(pal.accent), p.filled(pal.unlit));

    let mut targets = vec![(at_cell(layout.dj.word), Act::AutoDj), (at_cell(layout.dj.chevron), Act::DjQuick(QuickAct::Toggle))];
    targets.extend(layout.group.iter().map(|(rect, act)| (at_cell(*rect), act.clone())));
    targets.extend([
        (at_cell(layout.volume.minus), Act::VolDown),
        (at_cell(layout.volume.plus), Act::VolUp),
        (at_cell(layout.volume.cells[4]), Act::VolSet(4)),
        (at_cell(layout.card.lines[1]), Act::ToggleQueue),
    ]);
    for (cell, act) in targets {
        gui.ui.pointer = Some(cell);
        let p = paint_in(&ctx, &mut gui, 100, 30, 1.0);
        assert_eq!(gui.chrome.hot.as_ref().map(|(_, a)| a.clone()), Some(act.clone()), "{cell:?} lights {act:?}");
        assert!(over_control(&gui), "the cells' hand over {act:?}");
        let raised = p.filled(pal.raised);
        match &act {
            Act::AutoDj => {
                assert!(matches!(raised[..], [r] if frame.contains_rect(r) && r.max.x <= rule && r.min.x <= frame.min.x + 1.0), "the word's half: {raised:?}");
                assert_eq!(p.drawn("AUTO-DJ").0, pal.text, "an idle word to the text colour");
            }
            Act::DjQuick(_) => {
                assert!(matches!(raised[..], [r] if frame.contains_rect(r) && r.min.x >= rule - 0.01 && r.max.x >= frame.max.x - 1.0), "the chevron's half: {raised:?}");
                assert_eq!(p.chevron(pos2((rule + frame.max.x) / 2.0, frame.center().y)), (true, pal.text));
                assert_eq!(p.drawn("AUTO-DJ").0, pal.dim, "the word's half unlit");
            }
            Act::VolDown | Act::VolUp => {
                assert!(raised.is_empty());
                let mark = rect_of(&p, if act == Act::VolDown { layout.volume.minus } else { layout.volume.plus }).center();
                let strokes = p.strokes_near(mark, 4.0);
                assert!(!strokes.is_empty() && strokes.iter().all(|(_, s)| s.color == pal.text), "the mark in the text colour: {strokes:?}");
            }
            Act::VolSet(_) => {
                assert!(raised.is_empty());
                assert_eq!(segments(&p), segments(&rest), "the segments wear no hover (R17)");
            }
            Act::ToggleQueue => {
                assert_eq!(raised, [rect_of(&p, layout.card.rect)], "the whole card raised");
                assert_eq!(p.drawn("Vela — Cassini · 2019").0, pal.text, "the byline lit");
                assert_eq!(p.drawn("FLAC · 912 kbps · 44.1 kHz").0, pal.dim, "the spec never");
                assert_eq!(p.chevron(rect_of(&p, layout.card.chevron).center()).1, pal.text);
            }
            _ => {
                let at = framed(rect_of(&p, control(&layout, &act)));
                assert_eq!(raised, [at], "{act:?} alone raised");
                let edge = p.outline_of(at).expect("its frame");
                let lit = if act == Act::PlayPause { Stroke::new(2.0, pal.accent_hot) } else { Stroke::new(1.0, pal.dim) };
                assert_eq!(edge, lit, "{act:?}'s frame a step brighter");
            }
        }
        if !matches!(act, Act::DjQuick(_)) {
            assert_eq!(p.chevron(pos2((rule + frame.max.x) / 2.0, frame.center().y)).1, pal.dim, "{act:?}: the chevron unlit");
        }
    }
    let seek = layout.seek.expect("the seek row");
    for cell in [at_cell(layout.volume.readout), at_cell(seek.elapsed), at_cell(seek.total), at_cell(layout.note)] {
        gui.ui.pointer = Some(cell);
        let p = paint_in(&ctx, &mut gui, 100, 30, 1.0);
        assert_eq!(gui.chrome.hot, None, "{cell:?} works nothing");
        assert!(!over_control(&gui) && p.filled(pal.raised).is_empty(), "no hand and no light at {cell:?}");
    }
}

/// Under a modal the cells withhold the pointer for, under quick config's
/// catcher, and under the face's own quick config, nothing of the bar
/// lights; a press there is the layer's, which goes away, and works nothing
/// of the bar.
#[test]
fn under_a_modal_or_a_popup_nothing_lights_and_a_press_is_the_layers() {
    let _en = crate::setup::tests::in_locale("en");
    let pal = Pal::now();
    let mut gui = queued_gui();
    gui.app.now_playing = Some(track("Night Drive", 240.0));
    let layout = paint(&mut gui).layout;
    let play = control(&layout, &Act::PlayPause);
    let on_play = Position { x: play.x + 2, y: play.y + 1 };
    gui.ui.pointer = Some(on_play);
    gui.act(Act::NowMore);
    assert!(crate::gui::actions::modal_open(&gui), "the track's sheet");
    let p = paint(&mut gui);
    assert_eq!(gui.chrome.hot, None);
    assert!(p.filled(pal.raised).is_empty(), "nothing lit under the sheet");
    let before = state(&gui);
    press(&mut gui, on_play, MouseButton::Left);
    assert!(!crate::gui::actions::modal_open(&gui), "the press put the sheet away");
    assert_eq!(state(&gui).replace("sheet false", "sheet true"), before, "and worked nothing of the bar");

    for quick_face in [false, true] {
        let mut gui = queued_gui();
        gui.chrome.forced = Some(Faces { bar: true, quick: quick_face, ..Faces::default() });
        gui.act(Act::DjQuick(QuickAct::Toggle));
        gui.ui.pointer = Some(on_play);
        let p = paint(&mut gui);
        assert_eq!(gui.chrome.hot, None, "quick config up, its face {quick_face}");
        assert!(p.filled(pal.raised).is_empty(), "nothing lit under it");
        let before = state(&gui);
        press(&mut gui, on_play, MouseButton::Left);
        if !quick_face {
            // The cells' catcher; the face's takes its presses in its own
            // pass, which the hosting's tests run.
            assert!(gui.dj.quick.is_none(), "the press put it away");
            assert_eq!(state(&gui).replace("quick false", "quick true"), before, "and worked nothing of the bar");
        }
    }
}

/// A press on each control egui paints is the cells' press on it: the cell
/// under each painted box is that control's in the cells registry, and a
/// press there through the loop's input does what the control's act does
/// through `Gui::act`, which changes what the bar shows. A right press on
/// the card is the track's sheet while something plays.
#[test]
fn a_press_on_each_control_egui_paints_does_what_its_act_does() {
    let _en = crate::setup::tests::in_locale("en");
    let pal = Pal::now();
    let p = paint(&mut queued_gui());
    let layout = p.layout.clone();
    let frame = framed(rect_of(&p, layout.dj.frame));
    let rule = rule_x(&p);
    assert!(p.outline_of(frame).is_some(), "the split's frame painted");
    let mut boxes: Vec<(Pos2, Act)> = vec![
        (pos2((frame.min.x + rule - 1.0) / 2.0, frame.center().y), Act::AutoDj),
        (pos2((rule + frame.max.x) / 2.0, frame.center().y), Act::DjQuick(QuickAct::Toggle)),
    ];
    for (rect, act) in &layout.group {
        let at = framed(rect_of(&p, *rect));
        assert!(p.outline_of(at).is_some(), "{act:?}'s frame painted at {at:?}");
        boxes.push((at.center(), act.clone()));
    }
    let segments: Vec<egui::Rect> = p.filled(pal.accent).into_iter().chain(p.filled(pal.unlit)).filter(|r| r.size() == SEGMENT).collect();
    assert_eq!(segments.len(), 10, "ten segments painted");
    for (i, cell) in layout.volume.cells.iter().enumerate() {
        let segment = segments.iter().find(|s| rect_of(&p, *cell).contains_rect(**s)).expect("a segment in each cell");
        boxes.push((segment.center(), Act::VolSet(i as u8)));
    }
    for (rect, act) in [(layout.volume.minus, Act::VolDown), (layout.volume.plus, Act::VolUp)] {
        let mark = rect_of(&p, rect).center();
        assert!(!p.strokes_near(mark, 4.0).is_empty(), "{act:?}'s mark painted");
        boxes.push((mark, act));
    }
    boxes.push((rect_of(&p, layout.card.cover).center(), Act::ToggleQueue));
    boxes.push((rect_of(&p, layout.card.lines[0]).center(), Act::ToggleQueue));
    let seek = layout.seek.expect("the seek row");
    let lane = rect_of(&p, seek.track);
    let on_lane = pos2(lane.min.x + 0.6 * lane.width(), lane.center().y);
    let seek_act = Act::Seek(cell_fraction(usize::from(p.cell(on_lane).x - seek.track.x), seek.cells));
    boxes.push((on_lane, seek_act));

    for (point, act) in boxes {
        let cell = p.cell(point);
        let mut gui = queued_gui();
        let mut twin = queued_gui();
        paint(&mut gui);
        paint(&mut twin);
        assert_eq!(gui.ui.hit(cell), Some(act.clone()), "the cells' act under {act:?}'s paint at {cell:?}");
        let before = state(&gui);
        press(&mut gui, cell, MouseButton::Left);
        twin.act(act.clone());
        assert_eq!(state(&gui), state(&twin), "a press on {act:?} is its act");
        assert_ne!(state(&gui), before, "{act:?} did something");
    }

    let cover = p.cell(rect_of(&p, layout.card.cover).center());
    let mut gui = queued_gui();
    gui.app.now_playing = Some(track("Night Drive", 240.0));
    paint(&mut gui);
    assert_eq!(gui.ui.hit_context(cover), Some(Act::NowMore));
    press(&mut gui, cover, MouseButton::Right);
    assert!(crate::gui::actions::modal_open(&gui), "the right press: the playing track's sheet");
    let mut gui = bar_gui();
    gui.demo = None;
    paint(&mut gui);
    assert_eq!(gui.ui.hit_context(cover), None, "no right press while nothing plays");
    assert!(gui.ui.hit(cover) == Some(Act::ToggleQueue), "but the left one still folds the queue");
}

/// The cover: the playing track's picture as a texture in the 64-point slot
/// with the desk's corners, fitted down to the size it is drawn at; until it
/// is decoded, the empty slot's hairline.
#[test]
fn the_card_wears_its_cover_fitted_or_the_empty_slot() {
    let _en = crate::setup::tests::in_locale("en");
    let pal = Pal::now();
    let ctx = context();
    let mut gui = bar_gui();
    let mut playing = track("Night Drive", 240.0);
    playing.metadata.album_art = Some("aa.jpeg".into());
    gui.app.now_playing = Some(playing);
    let p = paint_in(&ctx, &mut gui, 100, 30, 1.0);
    let cover = rect_of(&p, p.layout.card.cover);
    assert_eq!(p.outline_of(cover), Some(Stroke::new(1.0, pal.edge_strong)), "not decoded yet: the empty slot");

    let art = Art::from_rgb(256, 256, vec![180; 256 * 256 * 3]).expect("a picture");
    gui.app.art.insert("aa.jpeg".into(), Some(art));
    let p = paint_in(&ctx, &mut gui, 100, 30, 1.0);
    let pictured = p.shapes.iter().find_map(|c| match &c.shape {
        Shape::Rect(r) if r.fill_texture_id() != egui::TextureId::default() => Some((r.rect, r.corner_radius, r.fill_texture_id())),
        _ => None,
    });
    let (at, corners, texture) = pictured.expect("the cover's texture");
    assert_eq!((at, corners), (cover, CornerRadius::same(COVER_RADIUS)), "whole in the slot, the desk's corners");
    assert_eq!(cover.size(), vec2(64.0, 64.0), "64 points square at the opening size");
    let size = ctx.tex_manager().read().meta(texture).map(|meta| meta.size);
    assert_eq!(size, Some([64, 64]), "the 256 pixels halved twice for a 64-pixel slot");
    assert!(p.outline_of(cover).is_none(), "no slot's hairline round the picture");
}

/// The fourth line: the rating's five stars in gold, filled whole, by half
/// and not at all from the cells' stars, each outlined, centred on the
/// facts' capitals in the face in use; the facts 8 points after the fifth.
#[test]
fn the_rating_stars_stand_in_gold_on_the_facts_capitals() {
    let _en = crate::setup::tests::in_locale("en");
    let pal = Pal::now();
    let ctx = context();
    let mut gui = bar_gui();
    gui.demo = Some(Now { rating: Some(7), ..demo_now() });
    let p = paint_in(&ctx, &mut gui, 100, 30, 1.0);
    let line = rect_of(&p, p.layout.card.lines[3]);
    let fills: Vec<(egui::Rect, egui::Rect)> = p
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            Shape::Mesh(mesh) if mesh.vertices.iter().all(|v| v.color == pal.gold) && line.contains_rect(mesh.calc_bounds()) => Some((mesh.calc_bounds(), c.clip_rect)),
            _ => None,
        })
        .collect();
    let outlines: Vec<egui::Rect> = p
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            Shape::Path(path) if path.closed && path.stroke.color == ColorMode::Solid(pal.gold) => Some(egui::Rect::from_points(&path.points)),
            _ => None,
        })
        .collect();
    assert_eq!(outlines.len(), 5, "five stars: {outlines:?}");
    assert_eq!(fills.len(), 4, "seven of ten: three whole and a half filled: {fills:?}");
    let half = fills.iter().filter(|(ink, clip)| clip.max.x < ink.center().x + 1.5).count();
    assert_eq!(half, 1, "one filled by half: {fills:?}");
    let (_, _, facts) = p.drawn("♪ 8A · 120 BPM");
    let fifth = outlines.iter().map(|r| r.max.x).fold(f32::MIN, f32::max);
    assert!((facts.min.x - (fifth + STARS_GAP)).abs() <= 1.0, "the facts {STARS_GAP} points after the fifth: {facts:?} after {fifth}");
    // The facts' capitals' middle, from the face in use: their baseline
    // under their galley's top, less half a capital's height.
    let mut capital = 0.0;
    let mut baseline = 0.0;
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        let face = studio::metrics(ui, &studio::mono(FIGURE_SIZE));
        capital = face.capital;
        baseline = face.baseline;
    });
    let capitals = facts.min.y + baseline - capital / 2.0;
    for star in &outlines {
        assert!((star.center().y - capitals).abs() <= 0.75, "a star {star:?} on the capitals' middle {capitals}");
    }
}

/// The bar stands in the cells' footprint at the floor with the tips row
/// under it and without, at the opening size and wide, at a fractional
/// scale too: every shape inside the bar's rows; the group's frames where
/// the cells lay them, re-centred between the split and the card; nothing
/// panics.
#[test]
fn the_bar_stands_in_its_cells_at_the_floor_wide_and_scaled() {
    let _en = crate::setup::tests::in_locale("en");
    let ctx = context();
    for (w, h, hints, ppp) in [(100, 24, true, 1.0), (100, 24, false, 1.25), (100, 30, false, 1.0), (160, 50, false, 2.0), (160, 50, true, 1.0)] {
        let mut gui = bar_gui();
        gui.config.gui.key_hints = hints;
        gui.ui.key_hints = hints;
        gui.app.queue.repeat = Repeat::All;
        gui.note = Some(("Queue saved".into(), false));
        let p = paint_in(&ctx, &mut gui, w, h, ppp);
        let bar = rect_of(&p, p.layout.rect);
        assert_eq!(p.layout.rect.bottom(), h - u16::from(hints), "{w}×{h}: the tips row under the bar stays the cells'");
        for clipped in &p.shapes {
            assert!(bar.expand(0.01).contains_rect(clipped.clip_rect), "{w}×{h}: clipped to the bar: {:?}", clipped.clip_rect);
            let ink = clipped.shape.visual_bounding_rect();
            assert!(!ink.is_positive() || bar.expand(1.0).contains_rect(ink), "{w}×{h}: inside the bar's rows: {ink:?} of {bar:?}");
        }
        for (rect, act) in &p.layout.group {
            assert!(p.outline_of(framed(rect_of(&p, *rect))).is_some(), "{w}×{h}: {act:?} in its cells");
        }
        let first = rect_of(&p, p.layout.group[0].0).min.x;
        let last = rect_of(&p, p.layout.group[4].0).max.x;
        let room = (rect_of(&p, p.layout.dj.frame).max.x, rect_of(&p, p.layout.card.rect).min.x);
        let cell = p.cells.cell().x;
        assert!((((first + last) - (room.0 + room.1)) / 2.0).abs() <= 1.5 * cell, "{w}×{h}: the group centred between the split and the card");
        for words in ["AUTO-DJ", "Cassini IV", "Queue saved", "70%"] {
            assert!(p.has(words), "{w}×{h}: {words}");
        }
    }
}

/// The bar asks egui for no pass of its own: no tooltip, no hover of
/// egui's, so its passes are the GUI's own redraws, a seek's and a note's
/// among them. egui's ask for the pass after one that had news is no ask of
/// the bar's: the host drops a zero delay, and keeps only one a dwell gives
/// (`panel::chrome_repaint`).
#[test]
fn the_bar_asks_for_no_pass_of_its_own() {
    let _en = crate::setup::tests::in_locale("en");
    let ctx = context();
    let mut gui = bar_gui();
    let _ = paint_in(&ctx, &mut gui, 100, 30, 1.0);
    let seek = crate::gui::bar::layout(Rect::new(0, 0, 100, 30), &bar_view(&gui, gui.bar_now().as_ref())).seek.expect("the seek row");
    gui.ui.pointer = Some(Position { x: seek.track.x + 4, y: seek.track.y });
    for _ in 0..3 {
        let p = paint_in(&ctx, &mut gui, 100, 30, 1.0);
        let delay = p.out.viewport_output.get(&ViewportId::ROOT).map(|viewport| viewport.repaint_delay);
        let kept = delay.filter(|delay| !delay.is_zero() && *delay < std::time::Duration::from_secs(60));
        assert_eq!(kept, None, "no pass asked for that the host keeps: {delay:?}");
    }
}
