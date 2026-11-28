//! Quick config's egui face run headless, as the chrome runs it: a context
//! with the panel's faces over the whole window, a cell 8 by 16 points as
//! the window opens, the Gui drawn first into a test terminal so the
//! chrome's marks and what quick config holds of the pointer are that
//! frame's, and a press's act applied through the frame's duties, the path
//! the window's presses take.

use egui::epaint::{TextShape, text::Galley};
use egui::{CursorIcon, Event, FontFamily, FullOutput, PointerButton, RawInput, Vec2};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;

use super::super::view;
use super::*;
use crate::api::types::Track;
use crate::config::{Config, ServerEntry};
use crate::gui::{DJ_NAV, Faces, Gui, Screen, chrome, render};
use crate::tui::app::{App, KnownServer, Origin, Queued};
use crate::tui::worker::{DjNote, DjServerInfo, Event as News};

const HOST: &str = "http://host:3000";
const ATTIC: &str = "http://attic:3000";
/// Saved servers past the session's and the attic, for Play from's longer
/// lists.
const MORE: [&str; 3] = ["http://den:3000", "http://loft:3000", "http://shed:3000"];

fn english() -> crate::setup::tests::LocaleGuard {
    crate::setup::tests::in_locale("en")
}

/// What a current server says of itself: everything quick config can do.
fn info(version: &str) -> DjServerInfo {
    DjServerInfo {
        version: Some(version.into()),
        discovery: true,
        discovery_ready: Some(true),
        libraries: vec!["Music".into()],
        mood_filters: true,
        auto_dj: Some(1),
    }
}

fn known(url: &str) -> KnownServer {
    KnownServer {
        id: url.into(),
        name: crate::gui::dj::short_name(url),
        token: None,
        username: None,
        self_signed: false,
        peer: None,
        pairing: None,
        dj: Default::default(),
    }
}

/// The session's server answers its probe.
fn probed(gui: &mut Gui, info: DjServerInfo) {
    let effects = gui.app.apply_event(News::DjProbed { identity: HOST.into(), info: Some(info) });
    gui.pend(effects);
}

/// The session on its server with `servers` saved, quick config up and its
/// server's answer in, its face forced on: no window, so the frame runs no
/// pass of its own and the cells draw nothing of it.
fn quick_gui(servers: &[&str]) -> Gui {
    let mut gui = Gui::new(Config::default(), false, App::new(Some(HOST.into()), None, None));
    gui.app.connected = true;
    gui.app.session.server = HOST.into();
    gui.app.session.server_id = HOST.into();
    for url in servers {
        gui.config.servers.push(ServerEntry { url: (*url).into(), ..Default::default() });
        gui.app.servers.push(known(url));
    }
    gui.chrome.forced = Some(Faces { quick: true, ..Faces::default() });
    gui.act(Act::DjQuick(QuickAct::Toggle));
    probed(&mut gui, info("6.33.1"));
    gui
}

fn key(gui: &mut Gui, code: KeyCode) {
    crate::gui::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
}

fn button(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() }
}

/// A context styled as the chrome's, over a window of cells.
struct Probe {
    ctx: egui::Context,
    cells: Cells,
}

/// One pass: what egui drew, the face's last run's report, the acts its
/// runs took, and how many runs egui made.
struct Pass {
    out: FullOutput,
    shown: Option<QuickOut>,
    acts: Vec<Act>,
    runs: usize,
}

impl Pass {
    fn shown(&self) -> &QuickOut {
        self.shown.as_ref().expect("quick config drawn")
    }

    /// The popover's frame: what a press on takes nothing.
    fn frame(&self) -> Rect {
        self.shown().items.iter().find(|(_, act)| *act == Act::Guard).expect("the frame").0
    }

    /// Where a press works `act`.
    fn spot(&self, act: &Act) -> Rect {
        self.shown().items.iter().rev().find(|(_, a)| a == act).unwrap_or_else(|| panic!("{act:?} somewhere")).0
    }
}

impl Probe {
    /// A window of `w` by `h` cells at the window's opening size.
    fn new(w: u16, h: u16) -> Probe {
        Probe::sized(w, h, vec2(f32::from(w) * 8.0, f32::from(h) * 16.0))
    }

    /// A window of `w` by `h` cells `points` across.
    fn sized(w: u16, h: u16, points: Vec2) -> Probe {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        studio::style_once(&ctx, &Pal::now());
        studio::dwell(&ctx);
        let cells = Cells { points: Rect::from_min_size(Pos2::ZERO, points), grid: ratatui::layout::Rect::new(0, 0, w, h), ppp: 1.0 };
        Probe { ctx, cells }
    }

    /// One frame of the Gui's into a test terminal the window's size: its
    /// rows.
    fn draw(&self, gui: &mut Gui) -> Vec<String> {
        let (w, h) = (self.cells.grid.width, self.cells.grid.height);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..h).map(|y| (0..w).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect()
    }

    /// The point in the middle of `cell`.
    fn at(&self, cell: Position) -> Pos2 {
        self.cells.rect(ratatui::layout::Rect { x: cell.x, y: cell.y, width: 1, height: 1 }).center()
    }

    /// The seek row's top: where the bar begins.
    fn seek(&self, view: &QuickView) -> f32 {
        self.cells.rect(ratatui::layout::Rect { x: 0, y: view.bar_top, width: self.cells.grid.width, height: 1 }).min.y
    }

    fn pass(&self, gui: &mut Gui, events: Vec<Event>) -> Pass {
        self.pass_held(gui, events, None, false)
    }

    /// The Gui drawn as a frame of the window's, then the face's pass over
    /// it with `events`, as the chrome runs it: quick config holding the
    /// pointer over `held` where a test says, else where the frame left it;
    /// egui asked to run it again where `again`.
    fn pass_held(&self, gui: &mut Gui, events: Vec<Event>, held: Option<&[Rect]>, again: bool) -> Pass {
        self.draw(gui);
        let view = view(gui, self.cells.grid);
        let live: Vec<Rect> = match held {
            Some(held) => held.to_vec(),
            None => chrome::live(gui, self.cells.grid).popup.iter().map(|rect| self.cells.rect(*rect)).collect(),
        };
        let pal = Pal::now();
        let input = RawInput { screen_rect: Some(self.cells.points), events, ..Default::default() };
        let (mut shown, mut acts, mut runs) = (None, Vec::new(), 0);
        let mut out = self.ctx.run_ui(input, |ui| {
            runs += 1;
            if let Some(view) = &view {
                let next = show(ui, view, &QuickCtx { cells: self.cells, live: &live, pal: &pal });
                acts.extend(next.act.clone());
                shown = Some(next);
            }
            if again && runs == 1 {
                ui.ctx().request_discard("a second run");
            }
        });
        // No GPU here to take the font atlas: let go of it on purpose.
        out.textures_delta.clear();
        Pass { out, shown, acts, runs }
    }

    /// Two passes with no news, the second's: egui lays the popover's area
    /// out unseen on its first, and only then can a press find it.
    fn settle(&self, gui: &mut Gui) -> Pass {
        let _ = self.pass(gui, Vec::new());
        self.pass(gui, Vec::new())
    }

    /// The pointer brought to `at`, with `held` where quick config holds
    /// it: the pass after the move's, with the cursor it shows.
    fn hover_held(&self, gui: &mut Gui, at: Pos2, held: Option<&[Rect]>) -> Pass {
        let _ = self.settle(gui);
        let _ = self.pass_held(gui, vec![Event::PointerMoved(at)], held, false);
        self.pass_held(gui, Vec::new(), held, false)
    }

    fn hover(&self, gui: &mut Gui, at: Pos2) -> Pass {
        self.hover_held(gui, at, None)
    }

    /// A press at `at` as hands make one, quick config holding the pointer
    /// over `held` where a test says: the pointer brought there, the press,
    /// and the frame's duties run on it as the window runs them after the
    /// pass, then its release. The press's act.
    fn click_held(&self, gui: &mut Gui, at: Pos2, held: Option<&[Rect]>) -> Option<Act> {
        let _ = self.settle(gui);
        let _ = self.pass_held(gui, vec![Event::PointerMoved(at)], held, false);
        let pass = self.pass_held(gui, vec![button(at, true)], held, false);
        assert!(pass.acts.len() <= 1, "one act a press: {:?}", pass.acts);
        let act = pass.acts.into_iter().next();
        gui.chrome.presses.push((self.cells.under(at), act.clone()));
        chrome::after_draw(gui);
        let _ = self.pass_held(gui, vec![button(at, false)], held, false);
        act
    }

    fn click(&self, gui: &mut Gui, at: Pos2) -> Option<Act> {
        self.click_held(gui, at, None)
    }
}

/// Every text a pass drew.
fn texts(out: &FullOutput) -> Vec<&TextShape> {
    out.shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text),
            _ => None,
        })
        .collect()
}

fn drew(out: &FullOutput, words: &str) -> bool {
    texts(out).iter().any(|text| text.galley.text() == words)
}

fn text_of<'a>(out: &'a FullOutput, words: &str) -> &'a TextShape {
    texts(out).into_iter().find(|text| text.galley.text() == words).unwrap_or_else(|| {
        panic!("{words:?} drawn: {:?}", texts(out).iter().map(|text| text.galley.text().to_string()).collect::<Vec<_>>())
    })
}

/// Where `words` were drawn.
fn rect_of(out: &FullOutput, words: &str) -> Rect {
    let text = text_of(out, words);
    Rect::from_min_size(text.pos, text.galley.size())
}

/// The colour `words` were drawn in: their first run's.
fn color_of(out: &FullOutput, words: &str) -> Color32 {
    let text = text_of(out, words);
    text.galley.job.sections.first().map_or(text.fallback_color, |section| section.format.color)
}

/// Where `words` stand their first glyph on its baseline, as egui laid it.
fn baseline_of(out: &FullOutput, words: &str) -> f32 {
    let text = text_of(out, words);
    let galley: &Galley = &text.galley;
    let row = galley.rows.first().expect("a row");
    text.pos.y + row.pos.y + row.row.glyphs.first().expect("a glyph").pos.y
}

/// Whether `words` were set in the semibold.
fn semibold(out: &FullOutput, words: &str) -> bool {
    let text = text_of(out, words);
    text.galley.job.sections.first().is_some_and(|section| section.format.font_id.family == FontFamily::Name(crate::gui::window::panel::STRONG.into()))
}

/// Every filled rect a pass drew, with its fill.
fn fills(out: &FullOutput) -> Vec<(Rect, Color32)> {
    out.shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.fill != Color32::TRANSPARENT => Some((rect.rect, rect.fill)),
            _ => None,
        })
        .collect()
}

/// Every hairline in the accent a pass drew, but `frame`'s own: the
/// keyboard's ring.
fn rings(out: &FullOutput, frame: Rect, pal: &Pal) -> Vec<Rect> {
    out.shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.stroke.color == pal.accent && rect.stroke.width > 0.0 && rect.rect != frame => Some(rect.rect),
            _ => None,
        })
        .collect()
}

/// The match bar's segments as drawn: lit, then unlit.
fn segments(out: &FullOutput, lit: Color32, pal: &Pal) -> (usize, usize) {
    let segs: Vec<Color32> = fills(out).into_iter().filter(|(rect, _)| rect.size() == vec2(SEG_W, SEG_H)).map(|(_, fill)| fill).collect();
    (segs.iter().filter(|fill| **fill == lit).count(), segs.iter().filter(|fill| **fill == pal.unlit).count())
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.75
}

/// The act the cells register under the first cell of `needle`, where the
/// cells draw quick config (the faces let go for one frame).
fn cells_act(probe: &Probe, gui: &mut Gui, needle: &str) -> Option<Act> {
    let forced = gui.chrome.forced.replace(Faces::default());
    let rows = probe.draw(gui);
    let act = rows.iter().enumerate().find_map(|(y, row)| {
        let at = row.find(needle)?;
        Some(gui.ui.hit(Position { x: row[..at].chars().count() as u16, y: y as u16 }))
    });
    gui.chrome.forced = forced;
    act.unwrap_or_else(|| panic!("no {needle:?} in the cells:\n{}", rows.join("\n")))
}

/// The act the cells register at `at`, where the cells draw quick config.
fn cells_hit(probe: &Probe, gui: &mut Gui, at: Position) -> Option<Act> {
    let forced = gui.chrome.forced.replace(Faces::default());
    probe.draw(gui);
    let act = gui.ui.hit(at);
    gui.chrome.forced = forced;
    act
}

/// One server, sonic on: the switch with its floor, the match bar and the
/// way to the Seeds under it, Smart filters and the way to the room, no
/// Play from; rising from the split's divider 42 cells wide, its foot 4
/// points over the seek row; rows 20 points, the groups 8 apart, its
/// padding 10; each value on its name's baseline, the switch's at the
/// content's end and Smart's 16 points short of it; the floor's 13 of 25
/// segments lit.
#[test]
fn one_server_draws_the_switch_its_bar_and_the_ways_on_where_the_board_sets_them() {
    let _en = english();
    let mut gui = quick_gui(&[]);
    let probe = Probe::new(100, 30);
    let pass = probe.settle(&mut gui);
    let out = &pass.out;
    let pal = Pal::now();
    for words in ["Sonic similarity", "55%", "Seeds", "Smart filters", "Any", "All Auto DJ settings"] {
        assert!(drew(out, words), "{words:?}");
    }
    assert!(!drew(out, "PLAY FROM") && !drew(out, "Play from"), "one server: nothing to choose");
    let view = view(&gui, probe.cells.grid).expect("up");
    let frame = pass.frame();
    let cells = probe.cells.rect(view.rect);
    assert_eq!((frame.min.x, frame.max.x), (cells.min.x, cells.max.x), "the cells' 42 from the split's divider");
    assert_eq!((frame.min.x, frame.width()), (112.0, 336.0));
    let seek = probe.seek(&view);
    assert_eq!(frame.max.y, seek - FOOT, "its foot over the seek row");
    assert_eq!(frame.height(), 2.0 * PAD + 5.0 * ROW_H + 2.0 * GAP_H);
    let part = pass.shown().frame.expect("its part");
    assert!(part.contains_rect(frame) && part.max.y <= seek && part.min.y >= 16.0, "its shadow kept to the room: {part:?}");
    let base = |words: &str| baseline_of(out, words);
    assert!(near(base("Seeds") - base("Sonic similarity"), 2.0 * ROW_H));
    assert!(near(base("Smart filters") - base("Seeds"), ROW_H + GAP_H));
    assert!(near(base("All Auto DJ settings") - base("Smart filters"), ROW_H + GAP_H));
    assert!(near(base("55%"), base("Sonic similarity")) && near(base("Any"), base("Smart filters")), "a value on its name's baseline");
    assert!(near(rect_of(out, "55%").max.x, frame.max.x - PAD), "the switch's value at the content's end");
    assert!(near(rect_of(out, "Any").max.x, frame.max.x - PAD - VALUE_END), "a value row's short of its pointer");
    assert!(near(rect_of(out, "Smart filters").min.x, frame.min.x + PAD) && near(rect_of(out, "Seeds").min.x, frame.min.x + PAD + NAMES));
    assert_eq!((color_of(out, "55%"), color_of(out, "Any"), color_of(out, "Seeds")), (pal.ok, pal.dim, pal.text));
    assert_eq!(color_of(out, "All Auto DJ settings"), pal.accent, "the amber way to the room");
    assert!(semibold(out, "Sonic similarity") && !semibold(out, "Smart filters"), "the switch's name bold while on, as the cells bold it");
    assert_eq!(segments(out, pal.accent, &pal), (13, 12), "55 lights 13 of 25");
}

/// Play from: up to four servers inline under its small capitals, each a
/// round mark and its name, the current one's in the semibold, a peer's
/// behind its branch; past four, one row naming the server it plays from.
#[test]
fn play_from_lists_up_to_four_servers_and_past_that_names_one() {
    let _en = english();
    let probe = Probe::new(100, 30);
    let mut gui = quick_gui(&[HOST, ATTIC]);
    let pass = probe.settle(&mut gui);
    for words in ["PLAY FROM", "host:3000", "attic:3000", "Sonic similarity", "Seeds"] {
        assert!(drew(&pass.out, words), "{words:?}");
    }
    assert!(semibold(&pass.out, "host:3000") && !semibold(&pass.out, "attic:3000"), "the current server's name bold");
    assert!(near(rect_of(&pass.out, "attic:3000").min.x, pass.frame().min.x + PAD + NAMES), "the names past the marks");

    // A peer, as the view names it: behind its branch.
    let mut view = view(&gui, probe.cells.grid).expect("up");
    view.lines[2] = QuickLine::Server { stop: QStop::Server(1), words: "Jazz Room".into(), peer: true, current: false };
    let pal = Pal::now();
    let input = RawInput { screen_rect: Some(probe.cells.points), ..Default::default() };
    let mut out = probe.ctx.run_ui(input, |ui| {
        let _ = show(ui, &view, &QuickCtx { cells: probe.cells, live: &[], pal: &pal });
    });
    out.textures_delta.clear();
    assert!(drew(&out, "└ Jazz Room"));

    let mut five = quick_gui(&[HOST, ATTIC, MORE[0], MORE[1], MORE[2]]);
    let pass = probe.settle(&mut five);
    assert!(drew(&pass.out, "Play from") && drew(&pass.out, "host:3000"), "the row and the server it plays from");
    assert!(!drew(&pass.out, "PLAY FROM") && !drew(&pass.out, "attic:3000"));
    assert_eq!(color_of(&pass.out, "host:3000"), pal.dim);
}

/// A row the server cannot do rests: its name and why, dim, the reason
/// whole and the name giving way to it, the switch's reason at the
/// content's end and Smart filters' 16 points short of it; no bar, no
/// Seeds, and a press on either takes nothing. A running session that had
/// to drop the pool says so in gold.
#[test]
fn a_row_the_server_cannot_do_rests_and_says_why() {
    let _en = english();
    let probe = Probe::new(100, 30);
    let mut gui = quick_gui(&[]);
    let mut old = info("6.10.0");
    old.discovery = false;
    old.mood_filters = false;
    probed(&mut gui, old);
    let pass = probe.settle(&mut gui);
    let (out, frame, pal) = (&pass.out, pass.frame(), Pal::now());
    for words in ["Sonic similarity", "Needs server 6.15.2+", "Smart filters", "No mood data", "All Auto DJ settings"] {
        assert_eq!(color_of(out, words), if words.starts_with("All") { pal.accent } else { pal.dim }, "{words:?}");
    }
    assert!(!drew(out, "Seeds") && segments(out, pal.accent, &pal) == (0, 0), "no bar and no Seeds");
    assert!(!text_of(out, "Needs server 6.15.2+").galley.elided && !text_of(out, "No mood data").galley.elided, "the reasons whole");
    assert!(near(rect_of(out, "Needs server 6.15.2+").max.x, frame.max.x - PAD));
    assert!(near(rect_of(out, "No mood data").max.x, frame.max.x - PAD - VALUE_END));
    assert!(rect_of(out, "Sonic similarity").max.x + VALUE_GAP <= rect_of(out, "Needs server 6.15.2+").min.x + 0.5, "the name gives way");
    for words in ["No mood data", "Needs server 6.15.2+", "Smart filters"] {
        let at = rect_of(&probe.settle(&mut gui).out, words).center();
        assert_eq!(probe.click(&mut gui, at), None, "{words:?} takes nothing");
        assert!(gui.dj.quick.is_some());
    }
    assert_eq!(cells_act(&probe, &mut gui, "No mood data"), Some(Act::Guard), "as the cells' Guard");

    let mut gui = quick_gui(&[]);
    gui.app.queue.items = vec![Queued { origin: Origin { server: HOST.into(), peer: None }, dj: None, track: Track { filepath: "a.mp3".into(), metadata: Default::default() } }];
    gui.act(Act::DjStartStop);
    gui.app.lane.sonic_drop = Some(DjNote::SonicRange);
    let pass = probe.settle(&mut gui);
    assert_eq!(color_of(&pass.out, "! Too strict"), pal.gold);
    assert!(near(rect_of(&pass.out, "! Too strict").max.x, pass.frame().max.x - PAD));
}

/// A press on each control works the act the cells register on its words
/// — the switch, a server's name, Seeds, Smart filters, the way to the
/// room — through the frame's duties, and does what the cells' does: the
/// switch flips and the popover stays, a server becomes the home, Seeds and
/// Smart filters open their sheets and the way to the room the Auto DJ
/// room, each putting the popover away.
#[test]
fn a_press_on_each_control_applies_the_act_the_cells_register_there() {
    let _en = english();
    let probe = Probe::new(100, 30);
    let controls = [
        ("Sonic similarity", "[✓] Sonic similarity", Act::DjQuick(QuickAct::Press(QStop::Sonic))),
        ("attic:3000", "( ) attic:3000", Act::DjQuick(QuickAct::Press(QStop::Server(1)))),
        ("Seeds", "Seeds", Act::DjQuick(QuickAct::Press(QStop::Seeds))),
        ("Smart filters", "Smart filters", Act::DjQuick(QuickAct::Press(QStop::Smart))),
        ("All Auto DJ settings", "All Auto DJ settings ▸", Act::DjQuick(QuickAct::Press(QStop::All))),
    ];
    for (words, cells, act) in controls {
        let mut gui = quick_gui(&[HOST, ATTIC]);
        assert_eq!(cells_act(&probe, &mut gui, cells), Some(act.clone()), "the cells' {cells:?}");
        let at = rect_of(&probe.settle(&mut gui).out, words).center();
        assert_eq!(probe.click(&mut gui, at), Some(act), "{words:?}");
        match words {
            "Sonic similarity" => assert!(!gui.app.dj.sonic && gui.dj.quick.is_some(), "the switch, at once; it stays up"),
            "attic:3000" => assert!(gui.app.dj_home.as_deref() == Some(ATTIC) && gui.dj.quick.is_some(), "the home; it stays up"),
            "Seeds" => assert!(gui.dj.quick.is_none() && gui.dj.seeds.is_some(), "the Seeds sheet"),
            "Smart filters" => assert!(gui.dj.quick.is_none() && crate::gui::moods::dj_open(&gui), "the mood modal for the DJ"),
            _ => assert!(gui.dj.quick.is_none() && (gui.screen, gui.active) == (Screen::Library, DJ_NAV), "the Auto DJ room"),
        }
    }
}

/// The match bar's segments set the floor each stands for and its ends
/// step it a point, as the cells' cells and ends do, and the popover stays.
#[test]
fn the_match_bar_sets_and_steps_the_floor_as_the_cells_bar_does() {
    let _en = english();
    let probe = Probe::new(100, 30);
    let mut gui = quick_gui(&[]);
    probe.draw(&mut gui);
    let QuickLine::Bar(bar) = view(&gui, probe.cells.grid).expect("up").lines[1] else { panic!("the bar under the switch") };
    let set = |k: u32| Act::DjQuick(QuickAct::Set(bar.first + k * bar.per));
    let cells_bar = {
        let forced = gui.chrome.forced.replace(Faces::default());
        let rows = probe.draw(&mut gui);
        let (y, row) = rows.iter().enumerate().find(|(_, row)| row.contains('▰')).expect("the cells' bar");
        let x = row[..row.find('▰').unwrap()].chars().count() as u16;
        let hit = |dx: i32| gui.ui.hit(Position { x: (i32::from(x) + dx) as u16, y: y as u16 });
        let acts = (hit(0), hit(3), hit(-2), hit(i32::from(bar.cells) + 1));
        gui.chrome.forced = forced;
        acts
    };
    assert_eq!(cells_bar, (Some(set(0)), Some(set(3)), Some(Act::DjQuick(QuickAct::Step(-1))), Some(Act::DjQuick(QuickAct::Step(1)))));
    for (act, floor) in [(set(0), 0.32), (set(3), 0.38), (Act::DjQuick(QuickAct::Step(-1)), 0.37), (Act::DjQuick(QuickAct::Step(1)), 0.38)] {
        let at = probe.settle(&mut gui).spot(&act).center();
        assert_eq!(probe.click(&mut gui, at), Some(act.clone()));
        assert!((gui.app.dj.sonic_min_similarity - floor).abs() < 1e-4, "{act:?}: {}", gui.app.dj.sonic_min_similarity);
        assert!(gui.dj.quick.is_some(), "it stays up");
    }
    let pass = probe.settle(&mut gui);
    let pal = Pal::now();
    assert_eq!(segments(&pass.out, pal.accent, &pal), (4, 21), "38 lights 4 of 25");
}

/// A press on the frame off its controls takes nothing, as the cells'
/// Guard; anywhere else it puts the popover away, as the cells' catcher
/// does — over the room, on the bar's own controls, on the split's chevron.
#[test]
fn a_press_off_its_controls_is_the_frames_and_anywhere_else_puts_it_away() {
    let _en = english();
    let probe = Probe::new(100, 30);
    let close = Some(Act::DjQuick(QuickAct::Close));
    let mut gui = quick_gui(&[]);
    let frame = probe.settle(&mut gui).frame();
    assert_eq!(probe.click(&mut gui, frame.min + vec2(3.0, 3.0)), None, "the frame's");
    assert!(gui.dj.quick.is_some());
    let room = Position { x: 90, y: 3 };
    assert_eq!(cells_hit(&probe, &mut gui, room), close, "the cells' catcher over the room");
    assert_eq!(probe.click(&mut gui, probe.at(room)), close, "over the room");
    assert!(gui.dj.quick.is_none());
    // The bar's own controls, which the cells register before quick
    // config's slot: quick config holds their cells, and a press there is
    // its catcher's, as it is the cells' catcher's.
    let bar_top = crate::gui::bar::top(probe.cells.grid, gui.footer());
    for act in [Act::PlayPause, Act::DjQuick(QuickAct::Toggle), Act::ToggleQueue] {
        gui.act(Act::DjQuick(QuickAct::Toggle));
        probe.draw(&mut gui);
        let at = (bar_top..bar_top + crate::gui::bar::BAR_ROWS)
            .flat_map(|y| (0..100).map(move |x| Position { x, y }))
            .find(|at| gui.ui.hit(*at) == Some(act.clone()))
            .unwrap_or_else(|| panic!("the bar's {act:?}"));
        assert!(chrome::claims(&chrome::live(&gui, probe.cells.grid)).holds(at), "quick config holds {act:?}'s cells");
        assert_eq!(cells_hit(&probe, &mut gui, at), close, "the cells' catcher over {act:?}");
        assert_eq!(probe.click(&mut gui, probe.at(at)), close, "{act:?}");
        assert!(gui.dj.quick.is_none(), "{act:?} put it away");
    }
}

/// Where quick config does not hold the pointer — nowhere during an MP3
/// write, not under what the cells stacked over it — nothing lights, no
/// hand shows and no press is taken, though the press lands on a control.
#[test]
fn nothing_lights_or_takes_a_press_where_quick_config_does_not_hold_the_pointer() {
    let _en = english();
    let probe = Probe::new(100, 30);
    let mut gui = quick_gui(&[]);
    let pass = probe.settle(&mut gui);
    let seeds = rect_of(&pass.out, "Seeds").center();
    let sonic = rect_of(&pass.out, "Sonic similarity").center();
    let room = probe.at(Position { x: 90, y: 3 });
    let none: &[Rect] = &[];
    assert_eq!(probe.hover_held(&mut gui, seeds, Some(none)).out.platform_output.cursor_icon, CursorIcon::Default);
    assert_eq!(probe.hover_held(&mut gui, room, Some(none)).out.platform_output.cursor_icon, CursorIcon::Default);
    assert_eq!(probe.click_held(&mut gui, sonic, Some(none)), None);
    assert_eq!(probe.click_held(&mut gui, room, Some(none)), None);
    assert!(gui.dj.quick.is_some() && gui.app.dj.sonic, "nothing pressed");
    // Something of the cells' over the Seeds row.
    let screen = probe.cells.points;
    let held = [Rect::from_min_max(screen.min, pos2(screen.max.x, seeds.y - 4.0)), Rect::from_min_max(pos2(screen.min.x, seeds.y + 4.0), screen.max)];
    let pass = probe.hover_held(&mut gui, seeds, Some(&held));
    assert_eq!(pass.out.platform_output.cursor_icon, CursorIcon::Default);
    assert!(!fills(&pass.out).iter().any(|(rect, fill)| *fill == Pal::now().raised && rect.contains(seeds)), "Seeds unlit");
    assert_eq!(probe.click_held(&mut gui, seeds, Some(&held)), None);
    assert_eq!(probe.click_held(&mut gui, sonic, Some(&held)), Some(Act::DjQuick(QuickAct::Press(QStop::Sonic))), "a control it holds");
}

/// The pointer lights what a press would work — the raised fill, the
/// semibold, the hand — and nothing on the frame off them; the keyboard's
/// hand wears the raised fill and the accent's ring, the model's and not
/// egui's focus, on the switch and then round the whole match bar, whose
/// lit segments heat.
#[test]
fn the_pointer_lights_a_control_and_the_keyboards_hand_wears_its_ring() {
    let _en = english();
    let probe = Probe::new(100, 30);
    let pal = Pal::now();
    let mut gui = quick_gui(&[]);
    let pass = probe.settle(&mut gui);
    let (frame, seeds) = (pass.frame(), rect_of(&pass.out, "Seeds").center());
    assert!(!semibold(&pass.out, "Seeds"));
    let pass = probe.hover(&mut gui, seeds);
    assert_eq!(pass.out.platform_output.cursor_icon, CursorIcon::PointingHand);
    assert!(semibold(&pass.out, "Seeds") && fills(&pass.out).iter().any(|(rect, fill)| *fill == pal.raised && rect.contains(seeds)), "Seeds lit");
    assert_eq!(probe.hover(&mut gui, frame.min + vec2(3.0, 3.0)).out.platform_output.cursor_icon, CursorIcon::Default, "the frame's");
    assert_eq!(probe.hover(&mut gui, probe.at(Position { x: 90, y: 3 })).out.platform_output.cursor_icon, CursorIcon::PointingHand, "the catcher's, as the cells'");

    let away = probe.at(Position { x: 90, y: 3 });
    let pass = probe.hover(&mut gui, away);
    assert!(rings(&pass.out, frame, &pal).is_empty(), "the hand stowed");
    key(&mut gui, KeyCode::Down);
    let pass = probe.hover(&mut gui, away);
    let sonic = rect_of(&pass.out, "Sonic similarity");
    let ring = rings(&pass.out, frame, &pal);
    assert!(ring.len() == 1 && ring[0].contains(sonic.center()) && ring[0].height() == ROW_H, "the ring round the switch's row: {ring:?}");
    assert!(probe.ctx.memory(|memory| memory.focused()).is_none(), "never egui's focus");
    key(&mut gui, KeyCode::Down);
    let pass = probe.hover(&mut gui, away);
    let ring = rings(&pass.out, frame, &pal);
    let first = pass.spot(&Act::DjQuick(QuickAct::Set(32))).center();
    assert!(ring.len() == 1 && ring[0].contains(first) && !ring[0].contains(sonic.center()), "round the bar: {ring:?}");
    assert_eq!(segments(&pass.out, pal.accent_hot, &pal), (13, 12), "its lit segments heat");
}

/// egui may run a pass twice; the press counts once, and at rest the face
/// asks egui for no pass of its own.
#[test]
fn a_press_counts_once_though_egui_runs_the_pass_twice() {
    let _en = english();
    let probe = Probe::new(100, 30);
    let mut gui = quick_gui(&[]);
    let at = rect_of(&probe.settle(&mut gui).out, "Sonic similarity").center();
    let _ = probe.pass(&mut gui, vec![Event::PointerMoved(at)]);
    let pass = probe.pass_held(&mut gui, vec![button(at, true)], None, true);
    assert_eq!(pass.runs, 2, "egui ran it again");
    assert_eq!(pass.acts, [Act::DjQuick(QuickAct::Press(QStop::Sonic))]);
    let _ = probe.pass(&mut gui, vec![button(at, false)]);
    let rest = probe.settle(&mut gui).out;
    let delay = rest.viewport_output.get(&egui::ViewportId::ROOT).map(|viewport| viewport.repaint_delay);
    assert!(delay.is_none_or(|delay| delay >= std::time::Duration::from_secs(60)), "no pass asked for: {delay:?}");
}

/// The tallest case, four servers and sonic on, stands whole under the
/// strip at the floor with the tips row on; squeezed into fewer points it
/// leaves out what does not fit and stays in the room; wide, it stays at
/// the split's divider. Nothing panics.
#[test]
fn it_stands_in_the_room_at_the_floor_squeezed_and_wide() {
    let _en = english();
    let servers = [HOST, ATTIC, MORE[0], MORE[1]];
    let floor = Probe::new(100, 24);
    let mut gui = quick_gui(&servers);
    gui.config.gui.key_hints = true;
    gui.ui.key_hints = true;
    let pass = floor.settle(&mut gui);
    let view = view(&gui, floor.cells.grid).expect("up");
    assert_eq!(view.bar_top, 18, "the floor's bar with the tips row under it");
    let frame = pass.frame();
    assert_eq!(frame.height(), 2.0 * PAD + 10.0 * ROW_H + 3.0 * GAP_H, "every line");
    assert!(frame.min.y >= 16.0 && frame.max.y == floor.seek(&view) - FOOT, "under the strip, over the bar: {frame:?}");
    for words in ["PLAY FROM", "host:3000", "loft:3000", "Seeds", "Smart filters", "All Auto DJ settings"] {
        assert!(drew(&pass.out, words), "{words:?} at the floor");
    }
    // The tips row off: the bar a row lower, and its foot with it.
    gui.config.gui.key_hints = false;
    gui.ui.key_hints = false;
    let pass = floor.settle(&mut gui);
    let low = super::super::view(&gui, floor.cells.grid).expect("up");
    assert_eq!(low.bar_top, 19, "the floor's bar on the last rows");
    assert_eq!(pass.frame().max.y, floor.seek(&low) - FOOT, "its foot over the seek row");
    assert_eq!(pass.frame().height(), 2.0 * PAD + 10.0 * ROW_H + 3.0 * GAP_H, "every line");
    gui.config.gui.key_hints = true;
    gui.ui.key_hints = true;

    let squeezed = Probe::sized(100, 24, vec2(400.0, 192.0));
    let pass = squeezed.settle(&mut gui);
    let frame = pass.frame();
    let room_top = squeezed.cells.rect(ratatui::layout::Rect { x: 0, y: 1, width: 100, height: 1 }).min.y;
    assert!(frame.min.y >= room_top && frame.max.y <= squeezed.seek(&view) - FOOT + 0.01, "{frame:?}");
    assert!(!drew(&pass.out, "All Auto DJ settings"), "what does not fit is left out");
    let all = pass.shown().items.iter().any(|(_, act)| *act == Act::DjQuick(QuickAct::Press(QStop::All)));
    assert!(!all, "and takes no press");

    let wide = Probe::new(160, 50);
    let mut gui = quick_gui(&[HOST, ATTIC]);
    let pass = wide.settle(&mut gui);
    let frame = pass.frame();
    assert_eq!((frame.min.x, frame.width()), (112.0, 336.0), "at the split's divider at any width");
    assert!(drew(&pass.out, "All Auto DJ settings"));
    let tiny = Probe::sized(100, 24, vec2(100.0, 48.0));
    let _ = tiny.settle(&mut gui);
}

/// Sonic similarity switched off: the empty tick box with its dim edge,
/// its name in the plain face and no value, as the cells show none; the
/// match bar and Seeds gone with it and the frame shorter by them, its foot
/// where it was. A press on it switches it back on, and they come back.
#[test]
fn the_switch_turned_off_has_no_value_and_takes_its_bar_and_seeds_with_it() {
    let _en = english();
    let probe = Probe::new(100, 30);
    let pal = Pal::now();
    let mut gui = quick_gui(&[]);
    gui.act(Act::DjQuick(QuickAct::Press(QStop::Sonic)));
    assert!(!gui.app.dj.sonic && gui.dj.quick.is_some());
    let pass = probe.settle(&mut gui);
    let (out, frame) = (&pass.out, pass.frame());
    assert!(drew(out, "Sonic similarity") && !semibold(out, "Sonic similarity"), "its name plain while off");
    assert_eq!(color_of(out, "Sonic similarity"), pal.text, "live, not resting");
    assert!(!texts(out).iter().any(|text| text.galley.text().contains('%')), "no value while off");
    let boxed = |edge: Color32| {
        out.shapes.iter().any(|clipped| matches!(&clipped.shape, egui::Shape::Rect(rect) if rect.rect.size() == Vec2::splat(studio::BOX) && rect.stroke.color == edge))
    };
    let ticked = out.shapes.iter().any(|clipped| matches!(&clipped.shape, egui::Shape::Path(path) if !path.closed));
    assert!(boxed(pal.dim) && !boxed(pal.ok) && !ticked, "the box empty, its edge dim");
    assert!(!drew(out, "Seeds") && segments(out, pal.accent, &pal) == (0, 0), "no bar, no Seeds");
    assert_eq!(frame.height(), 2.0 * PAD + 3.0 * ROW_H + 2.0 * GAP_H);
    assert_eq!(frame.max.y, probe.seek(&view(&gui, probe.cells.grid).expect("up")) - FOOT, "its foot where it was");
    let at = rect_of(out, "Sonic similarity").center();
    assert_eq!(probe.click(&mut gui, at), Some(Act::DjQuick(QuickAct::Press(QStop::Sonic))));
    assert!(gui.app.dj.sonic && gui.dj.quick.is_some(), "on again; it stays up");
    let pass = probe.settle(&mut gui);
    assert!(drew(&pass.out, "Seeds") && drew(&pass.out, "55%") && segments(&pass.out, pal.accent, &pal) == (13, 12));
}

/// Past four servers, Play from's row works what the cells' row works
/// there, across the whole row, off its words as on them: the Play from
/// sheet opens and quick config goes, as a press on the cells' row does.
#[test]
fn play_from_past_four_opens_its_sheet_as_the_cells_row_does() {
    let _en = english();
    let probe = Probe::new(100, 30);
    let mut gui = quick_gui(&[HOST, ATTIC, MORE[0], MORE[1], MORE[2]]);
    let act = Act::DjQuick(QuickAct::Press(QStop::PlayFrom));
    assert_eq!(cells_act(&probe, &mut gui, "Play from"), Some(act.clone()), "the cells' row");
    let pass = probe.settle(&mut gui);
    let (frame, row) = (pass.frame(), pass.spot(&act));
    assert!(near(row.min.x, frame.min.x + PAD - REACH) && near(row.max.x, frame.max.x - PAD + REACH), "the whole row: {row:?}");
    assert!(near(rect_of(&pass.out, "host:3000").max.x, frame.max.x - PAD - VALUE_END), "its value short of its pointer");
    let at = pos2(rect_of(&pass.out, "Play from").max.x + VALUE_GAP, row.center().y);
    assert!(!rect_of(&pass.out, "host:3000").contains(at), "between the words and the value");
    assert_eq!(probe.click(&mut gui, at), Some(act));
    assert!(gui.dj.quick.is_none() && gui.dj.sheet.as_ref().is_some_and(|open| open.sheet == crate::gui::dj::Sheet::PlayFrom), "the Play from sheet");
}

/// In Japanese, Chinese and German, measured in points: the tallest case
/// stands whole at the floor with the tips row on; the switch's gold reason
/// and Smart filters' chosen moods in the ok green stand whole at their
/// ends, each name short of its value; and on a server that can do
/// neither, both reasons stand whole where the board ends them, the names
/// giving way.
#[test]
fn in_cjk_and_german_every_value_and_reason_stands_whole_beside_its_name() {
    let Some(_) = crate::setup::tests::in_own_process(&["ja", "zh", "de"]) else { return };
    let floor = Probe::new(100, 24);
    let pal = Pal::now();
    let mut gui = quick_gui(&[HOST, ATTIC, MORE[0], MORE[1]]);
    gui.config.gui.key_hints = true;
    gui.ui.key_hints = true;
    gui.app.dj.moods = crate::api::types::MoodFilter::NONE.press(crate::api::types::MoodScale::Valence, "happy");
    gui.app.queue.items = vec![Queued { origin: Origin { server: HOST.into(), peer: None }, dj: None, track: Track { filepath: "a.mp3".into(), metadata: Default::default() } }];
    gui.act(Act::DjStartStop);
    gui.app.lane.sonic_drop = Some(DjNote::SonicRange);
    let pass = floor.settle(&mut gui);
    let (out, frame) = (&pass.out, pass.frame());
    assert_eq!(frame.height(), 2.0 * PAD + 10.0 * ROW_H + 3.0 * GAP_H, "every line");
    assert!(frame.min.y >= 16.0, "under the strip: {frame:?}");
    let sonic = rust_i18n::t!("gui.dj.sonic").to_string();
    let strict = format!("! {}", rust_i18n::t!("gui.dj.too_strict"));
    let smart = rust_i18n::t!("gui.dj.smart").to_string();
    let (moods, _) = crate::gui::dj::moods_value(gui.app.dj.moods);
    for words in [&rust_i18n::t!("gui.dj.play_from").to_uppercase(), &sonic, &strict, &smart, &moods, &rust_i18n::t!("gui.dj.all_settings").to_string()] {
        assert!(drew(out, words) && !text_of(out, words).galley.elided, "{words:?} whole");
    }
    assert_eq!((color_of(out, &strict), color_of(out, &moods)), (pal.gold, pal.ok));
    assert!(near(rect_of(out, &strict).max.x, frame.max.x - PAD) && near(rect_of(out, &moods).max.x, frame.max.x - PAD - VALUE_END));
    assert!(rect_of(out, &sonic).max.x + VALUE_GAP <= rect_of(out, &strict).min.x + 0.5 && rect_of(out, &smart).max.x + VALUE_GAP <= rect_of(out, &moods).min.x + 0.5, "each name short of its value");

    let probe = Probe::new(100, 30);
    let mut gui = quick_gui(&[]);
    let mut old = info("6.10.0");
    old.discovery = false;
    old.mood_filters = false;
    probed(&mut gui, old);
    let pass = probe.settle(&mut gui);
    let (out, frame) = (&pass.out, pass.frame());
    let needs = rust_i18n::t!("gui.dj.needs_server", version = crate::dj::floor_label(crate::dj::FLOOR_SONIC)).to_string();
    let none = rust_i18n::t!("gui.dj.no_mood_data").to_string();
    for words in [&needs, &none] {
        assert!(!text_of(out, words).galley.elided && color_of(out, words) == pal.dim, "{words:?} whole and dim");
    }
    assert!(near(rect_of(out, &needs).max.x, frame.max.x - PAD) && near(rect_of(out, &none).max.x, frame.max.x - PAD - VALUE_END));
    assert!(rect_of(out, &sonic).max.x + VALUE_GAP <= rect_of(out, &needs).min.x + 0.5 && rect_of(out, &smart).max.x + VALUE_GAP <= rect_of(out, &none).min.x + 0.5, "the names give way");
}
