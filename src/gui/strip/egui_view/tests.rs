//! The strip's egui face and the server menu's, headless: each drawn from
//! its view in a context of its own, styled and timed as the chrome runs
//! it, at the window's opening size, at the floor, wide and squeezed, in
//! every language; and through the chrome's pass and the frame's duties,
//! a press on each item applying the cells' act to the Gui. Every measure
//! is taken from the face in use: the panel sets the strip in the system's
//! UI face, which is not Segoe UI on every machine.

use egui::{Event, FontFamily, FullOutput, Modifiers, PointerButton, RawInput, Shape};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::menu::MenuOut;
use super::*;
use crate::config::{Config, PeerEntry, ServerEntry};
use crate::gui::chrome::{self, Out, Views};
use crate::gui::strip::{self, StripServer};
use crate::gui::window::panel::STRONG;
use crate::gui::{Faces, Gui, handle_key, render, servers};
use crate::setup::tests::{in_locale, in_own_process};
use crate::tui::app::App;

const ATTIC: &str = "http://attic.local:3000";
const OFFICE: &str = "http://office.local:3000";

/// Every language the player speaks.
const LOCALES: [&str; 10] = ["en", "de", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"];

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    crate::gui::window::panel::fonts(&ctx);
    ctx
}

/// A `w`×`h` grid in a window of 8 by 16 points a cell, as it opens.
fn cells_at(w: u16, h: u16) -> Cells {
    let points = egui::Rect::from_min_size(Pos2::ZERO, vec2(f32::from(w) * 8.0, f32::from(h) * 16.0));
    Cells { points, grid: Rect::new(0, 0, w, h), ppp: 1.0 }
}

fn row0(cells: Cells) -> egui::Rect {
    cells.rect(Rect { x: cells.grid.x, y: cells.grid.y, width: cells.grid.width, height: 1 })
}

/// A Gui of the saved `urls`, its session on the first, egui's strip
/// forced on so its cells mark the strip's and the menu's slots.
fn gui_on(urls: &[&str]) -> Gui {
    let mut config = Config::default();
    config.servers = urls.iter().map(|url| ServerEntry { url: url.to_string(), ..Default::default() }).collect();
    let mut gui = Gui::new(config, false, App::new(urls.first().map(|url| url.to_string()), None, None));
    gui.chrome.forced = Some(Faces { strip: true, bar: false, quick: false });
    gui
}

/// A peer named `name` reached through `parent`, saved last.
fn add_peer(gui: &mut Gui, parent: &str, name: &str) {
    let peer = PeerEntry { parent: parent.to_string(), id: 3, name: name.to_string(), missing: false, hidden: false };
    gui.config.servers.push(ServerEntry { url: crate::config::peer_identity(parent, 3), peer: Some(peer), ..Default::default() });
}

/// The Gui's frame in cells at `cells`' size: the marks the chrome reads.
fn frame(gui: &mut Gui, cells: Cells) {
    let mut terminal = Terminal::new(TestBackend::new(cells.grid.width, cells.grid.height)).unwrap();
    terminal.draw(|frame| render(frame, gui)).unwrap();
}

fn button(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::default() }
}

/// A text a pass drew: its words, where, its first run's colour and face,
/// and where its first letter stands on its baseline.
#[derive(Clone, Debug)]
struct Drawn {
    words: String,
    rect: egui::Rect,
    color: Color32,
    family: FontFamily,
    baseline: f32,
    galley: std::sync::Arc<egui::Galley>,
}

fn drawn(full: &FullOutput) -> Vec<Drawn> {
    full.shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Text(t) => {
                let section = t.galley.job.sections.first()?;
                let row = t.galley.rows.first()?;
                let baseline = t.pos.y + row.pos.y + row.row.glyphs.first()?.pos.y;
                Some(Drawn {
                    words: t.galley.text().to_string(),
                    rect: egui::Rect::from_min_size(t.pos, t.galley.size()),
                    color: section.format.color,
                    family: section.format.font_id.family.clone(),
                    baseline,
                    galley: t.galley.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

fn find(texts: &[Drawn], words: &str) -> Drawn {
    texts.iter().find(|d| d.words == words).cloned().unwrap_or_else(|| panic!("{words:?} not drawn: {:?}", texts.iter().map(|d| &d.words).collect::<Vec<_>>()))
}

/// The rects a pass filled in `color`.
fn fills(full: &FullOutput, color: Color32) -> Vec<egui::Rect> {
    full.shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Rect(r) if r.fill == color => Some(r.rect),
            _ => None,
        })
        .collect()
}

/// The strokes a pass drew in `color`, by their ends.
fn strokes(full: &FullOutput, color: Color32) -> Vec<[Pos2; 2]> {
    full.shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::LineSegment { points, stroke } if stroke.color == color => Some(*points),
            _ => None,
        })
        .collect()
}

/// The filled polygons a pass drew in `color`, by their points.
fn polygons(full: &FullOutput, color: Color32) -> Vec<Vec<Pos2>> {
    full.shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Path(path) if path.fill == color => Some(path.points.clone()),
            _ => None,
        })
        .collect()
}

fn strong_family() -> FontFamily {
    FontFamily::Name(STRONG.into())
}

/// What a pass of the strip's face drew and did: its last run's out and
/// slots, the acts its runs took added up, and how many runs egui made.
struct Shot {
    out: StripOut,
    full: FullOutput,
    slots: Slots,
    acts: Vec<Act>,
    runs: usize,
}

/// The strip's face over row 0 of a window, as the chrome runs it: styled,
/// the dwell set, in a child over row 0 on the ground; its live cells all
/// of row 0 unless a test says otherwise.
struct Rig {
    ctx: egui::Context,
    cells: Cells,
    live: Vec<egui::Rect>,
    hover: bool,
    since_key: Option<Duration>,
    time: f64,
    /// egui runs the next pass twice.
    again: bool,
}

impl Rig {
    fn new(w: u16, h: u16) -> Rig {
        let cells = cells_at(w, h);
        Rig { ctx: context(), cells, live: vec![row0(cells)], hover: true, since_key: None, time: 0.0, again: false }
    }

    fn pass(&mut self, view: &StripView, events: Vec<Event>) -> Shot {
        let input = RawInput { screen_rect: Some(self.cells.points), time: Some(self.time), events, ..Default::default() };
        let pal = Pal::now();
        let (cells, live, hover, since_key, again) = (self.cells, &self.live, self.hover, self.since_key, std::mem::take(&mut self.again));
        let (mut acts, mut last, mut runs) = (Vec::new(), None, 0);
        let mut full = self.ctx.run_ui(input, |ui| {
            studio::style_once(ui.ctx(), &pal);
            studio::dwell(ui.ctx());
            let row = row0(cells);
            let builder = egui::UiBuilder::new().id_salt("chrome-strip").max_rect(row).layout(egui::Layout::top_down(egui::Align::Min));
            let mut part = ui.new_child(builder);
            part.set_clip_rect(row.intersect(ui.clip_rect()));
            let _ = studio::begin(&part);
            let cx = StripCtx { cells, live, hover, since_key, pal: &pal };
            let out = show(&mut part, view, &cx);
            acts.extend(out.act.clone());
            last = Some((out, slots(&part, view, cells)));
            runs += 1;
            if again && runs == 1 {
                ui.ctx().request_discard("a second run");
            }
        });
        full.textures_delta.clear();
        let (out, slots) = last.expect("a run");
        Shot { out, full, slots, acts, runs }
    }

    /// Two passes with nothing new: egui lays some things out on the first.
    fn settle(&mut self, view: &StripView) -> Shot {
        let _ = self.pass(view, Vec::new());
        self.pass(view, Vec::new())
    }

    /// The pointer brought to `at`, and the pass after, which hovers what
    /// the move's pass laid out.
    fn rest(&mut self, view: &StripView, at: Pos2) -> Shot {
        let _ = self.pass(view, vec![Event::PointerMoved(at)]);
        self.pass(view, Vec::new())
    }

    /// The pointer brought to `at` and a press there, then its release:
    /// the press's pass.
    fn press(&mut self, view: &StripView, at: Pos2) -> Shot {
        let _ = self.rest(view, at);
        let shot = self.pass(view, vec![button(at, true)]);
        let _ = self.pass(view, vec![button(at, false)]);
        shot
    }

    /// How wide `s` lays out in `font` in the rig's faces.
    fn width(&self, s: &str, font: FontId) -> f32 {
        let mut w = 0.0;
        let input = RawInput { screen_rect: Some(self.cells.points), ..Default::default() };
        let mut full = self.ctx.run_ui(input, |ui| w = studio::width_of(ui, s, font.clone()));
        full.textures_delta.clear();
        w
    }
}

#[test]
fn each_state_draws_its_words_in_its_look() {
    let _en = in_locale("en");
    let pal = Pal::now();
    let mut gui = gui_on(&[ATTIC, OFFICE]);
    let mut rig = Rig::new(100, 30);

    // Library up, at rest, two servers (TopStates S1).
    let view = strip::view(&gui);
    let shot = rig.settle(&view);
    let texts = drawn(&shot.full);
    let library = find(&texts, "Library");
    assert_eq!((library.color, &library.family), (pal.text, &strong_family()), "the marked tab's name in the text colour and the semibold");
    for name in ["Stats", "Admin", "MP3 Player", "Visualizer", ATTIC] {
        let d = find(&texts, name);
        assert_eq!((d.color, &d.family), (pal.dim, &FontFamily::Proportional), "{name} dim at rest");
    }
    let up = shot.slots.tabs[0];
    assert!(fills(&shot.full, pal.raised).contains(&up), "the marked tab's raised fill, the whole row tall");
    let rule = fills(&shot.full, pal.accent);
    assert!(rule.iter().any(|r| r.min.x == up.min.x && r.max.x == up.max.x && r.max.y == up.max.y && r.height() == RULE), "its amber rule along the strip's foot: {rule:?}");
    let strip = shot.slots.strip;
    assert!(strokes(&shot.full, pal.edge).iter().any(|[a, b]| a.y == b.y && a.y > strip.max.y - 1.0 && a.x <= strip.min.x && b.x >= strip.max.x), "the edge hairline along the strip's foot");
    let viz = shot.slots.viz;
    assert!(fills(&shot.full, pal.unlit).iter().any(|r| viz.contains_rect(*r) && r.width() == LAMP), "the Visualizer's lamp unlit while its window is closed");
    assert!(strokes(&shot.full, pal.dim).iter().filter(|[a, _]| shot.slots.label.as_ref().is_some_and(|l| l.chevron.is_some_and(|c| c.contains(*a)))).count() == 2, "the menu's chevron, dim");
    // The chevron and [+]'s plus both on the row's middle, as the board
    // centres them.
    let middle = |within: egui::Rect| {
        let ys: Vec<f32> = strokes(&shot.full, pal.dim).iter().filter(|[a, b]| within.contains(*a) && within.contains(*b)).flat_map(|[a, b]| [a.y, b.y]).collect();
        (ys.iter().copied().fold(f32::MAX, f32::min) + ys.iter().copied().fold(f32::MIN, f32::max)) / 2.0
    };
    let chevron = shot.slots.label.as_ref().and_then(|l| l.chevron).expect("the chevron");
    for (what, at) in [("the chevron", chevron), ("[+]", shot.slots.plus)] {
        assert!((middle(at) - strip.center().y).abs() < 0.01, "{what} on the row's middle: {} of {:?}", middle(at), strip);
    }

    // Now Playing up: no tab marked (S4).
    gui.screen = Screen::NowPlaying;
    let shot = rig.settle(&strip::view(&gui));
    assert!(fills(&shot.full, pal.raised).is_empty(), "no tab's fill");
    for name in ["Library", "Stats", "Admin", "MP3 Player"] {
        assert_eq!(find(&drawn(&shot.full), name).color, pal.dim, "{name} at rest on Now Playing");
    }
    gui.screen = Screen::Library;

    // The Visualizer's window open (S5): the lamp amber, the word in the
    // text colour and the semibold; no tab's look.
    let mut view = strip::view(&gui);
    view.viz.lit = true;
    let shot = rig.settle(&view);
    let word = find(&drawn(&shot.full), "Visualizer");
    assert_eq!((word.color, &word.family), (pal.text, &strong_family()));
    assert!(fills(&shot.full, pal.accent).iter().any(|r| shot.slots.viz.contains_rect(*r) && r.width() == LAMP), "the lamp amber");
    assert!(!fills(&shot.full, pal.raised).contains(&shot.slots.viz), "lit is no tab's fill");

    // A sync running at 28 % while the Library is up (S6), paused (S7), and
    // on the MP3 Player tab up (S8): the figure in the monospace on the
    // name's baseline, amber while it runs, dim while it waits, amber on
    // the marked tab too.
    for (screen, paused, tone) in [(Screen::Library, false, pal.accent), (Screen::Library, true, pal.dim), (Screen::Device, false, pal.accent)] {
        gui.screen = screen;
        let mut view = strip::view(&gui);
        view.mark = Some(SyncMark { pct: 28, paused });
        let shot = rig.settle(&view);
        let texts = drawn(&shot.full);
        let (name, figure) = (find(&texts, "MP3 Player"), find(&texts, "28%"));
        assert_eq!((figure.color, &figure.family), (tone, &FontFamily::Monospace), "{screen:?} paused {paused}");
        assert!((figure.baseline - name.baseline).abs() < 0.5, "on the name's baseline: {} {}", figure.baseline, name.baseline);
        assert!(figure.rect.min.x >= name.rect.max.x + MARK_GAP - 0.5, "after the name");
        assert!(shot.slots.tabs[3].contains_rect(name.rect.union(figure.rect).shrink(0.5)), "inside its tab's slot");
        let marked = screen == Screen::Device;
        assert_eq!(name.color, if marked { pal.text } else { pal.dim });
    }
    gui.screen = Screen::Library;

    // One server: its words with no chevron. None named: [+] alone.
    let mut view = strip::view(&gui);
    view.server = Some(StripServer { name: ATTIC.into(), menu: false });
    let shot = rig.settle(&view);
    assert!(shot.slots.label.as_ref().is_some_and(|label| label.chevron.is_none()), "no chevron for one server");
    assert_eq!(find(&drawn(&shot.full), ATTIC).color, pal.dim);
    view.server = None;
    let shot = rig.settle(&view);
    assert!(!drawn(&shot.full).iter().any(|d| d.words.contains("http")), "no label with no server");
    let plus = shot.slots.plus;
    assert_eq!(strokes(&shot.full, pal.dim).iter().filter(|[a, b]| plus.contains(*a) && plus.contains(*b)).count(), 2, "[+]'s plus, dim, in its box");
}

/// The strip stands in row 0 as the board sets it at the window's opening
/// size: the tabs from the second column, each its name with 10 points
/// either side, 2 apart; the hairline 8 points after the MP3 Player tab and
/// the Visualizer 8 after it; [+] in its cells, the label ending a blank
/// cell short of it. Wide, the tabs stay where they were and the right end
/// follows the window's edge.
#[test]
fn the_strips_slots_are_measured_as_the_board_sets_them() {
    let _en = in_locale("en");
    let gui = gui_on(&[ATTIC, OFFICE]);
    let view = strip::view(&gui);
    let mut narrow = Rig::new(100, 30);
    let shot = narrow.settle(&view);
    let slots = shot.slots.clone();
    assert_eq!(slots.strip, egui::Rect::from_min_max(Pos2::ZERO, pos2(800.0, 16.0)), "row 0, 16 points");
    assert_eq!(slots.tabs[0].min.x, 8.0, "from the second column");
    for (k, tab) in view.tabs.iter().enumerate() {
        let font = if tab.up { strong(SIZE) } else { body(SIZE) };
        let words = narrow.width(&tab.label, font);
        let at = slots.tabs[k];
        assert!((at.width() - (2.0 * PAD + words)).abs() < 0.01, "{}: its name and 10 points either side", tab.label);
        assert_eq!((at.min.y, at.max.y), (0.0, 16.0), "{}: the row tall", tab.label);
        if k > 0 {
            assert!((at.min.x - slots.tabs[k - 1].max.x - APART).abs() < 0.01, "2 points apart");
        }
    }
    assert!((slots.divider - slots.tabs[3].max.x - DIVIDER_AIR).abs() < 0.01 && (slots.viz.min.x - slots.divider - 1.0 - DIVIDER_AIR).abs() < 0.01);
    assert_eq!((slots.plus.min.x, slots.plus.max.x), (760.0, 784.0), "[+] in columns 95-97");
    let label = slots.label.clone().expect("the label");
    assert_eq!(label.rect.max.x, 752.0, "a blank cell short of [+]");
    assert!(label.rect.min.x >= slots.viz.max.x + 8.0, "a blank cell past the strip at least");

    // A sync's figure widens the MP3 Player tab by itself and its gap, and
    // moves everything after it as far.
    let mut synced = view.clone();
    synced.mark = Some(SyncMark { pct: 28, paused: false });
    let with = narrow.settle(&synced).slots;
    let grow = MARK_GAP + narrow.width("28%", mono(MARK_SIZE));
    assert!((with.tabs[3].width() - slots.tabs[3].width() - grow).abs() < 0.01 && (with.viz.min.x - slots.viz.min.x - grow).abs() < 0.01);

    let mut wide = Rig::new(160, 50);
    let wide_slots = wide.settle(&view).slots;
    assert_eq!(wide_slots.tabs, slots.tabs, "the tabs stay put");
    assert_eq!((wide_slots.plus.min.x, wide_slots.plus.max.x), (1240.0, 1264.0));
    assert_eq!(wide_slots.label.expect("the label").rect.max.x, 1232.0);
}

/// In every language the strip is laid out in points in the face in use,
/// CJK's two-cell letters among them: the tabs in order and never over one
/// another, each its name's measured width and its padding, their words
/// inside their slots; the label never nearer the strip than a blank cell,
/// with or without a sync's figure, whole wherever its measured width
/// fits the room left, cut with an ellipsis before its chevron where not.
#[test]
fn every_language_measures_the_strip_in_points_and_keeps_the_label_off_it() {
    // A language a process: the locale is the whole process's.
    let Some(locale) = in_own_process(&LOCALES) else { return };
    // Words inside a slot from side to side: a CJK face's line box may
    // stand taller than the row its letters fit.
    let across = |slot: egui::Rect, words: egui::Rect| words.min.x >= slot.min.x - 0.5 && words.max.x <= slot.max.x + 0.5;
    let mut gui = gui_on(&[ATTIC, OFFICE]);
    for (screen, mark) in [(Screen::Library, None), (Screen::Device, Some(5)), (Screen::Stats, Some(100))] {
        gui.screen = screen;
        let mut view = strip::view(&gui);
        view.mark = mark.map(|pct| SyncMark { pct, paused: false });
        let mut rig = Rig::new(100, 30);
        let shot = rig.settle(&view);
        let (slots, texts) = (shot.slots.clone(), drawn(&shot.full));
        let mut right = 0.0;
        for (k, tab) in view.tabs.iter().enumerate() {
            let at = slots.tabs[k];
            assert!(at.min.x >= right, "{locale}: {} after the tab before it", tab.label);
            right = at.max.x;
            let font = if tab.up { strong(SIZE) } else { body(SIZE) };
            let figure = view.mark.filter(|_| tab.screen == Screen::Device).map_or(0.0, |m| MARK_GAP + rig.width(&m.words(), mono(MARK_SIZE)));
            assert!((at.width() - 2.0 * PAD - rig.width(&tab.label, font) - figure).abs() < 0.01, "{locale}: {} measured in points", tab.label);
            assert!(across(at, find(&texts, &tab.label).rect), "{locale}: {} inside its slot", tab.label);
        }
        assert!(slots.viz.min.x > right && across(slots.viz, find(&texts, &view.viz.label).rect), "{locale}: the Visualizer after the tabs");
        let label = slots.label.clone().expect("room for the label at 100 columns");
        assert!(label.rect.min.x >= slots.viz.max.x + 8.0 - 0.01, "{locale} {mark:?}: the label never nearer the strip than a blank cell");
        assert_eq!(label.rect.max.x, 752.0, "{locale}");
        let name = texts.iter().find(|d| d.rect.max.x <= label.words_right + 0.5 && d.rect.min.x >= label.rect.min.x - 0.5 && d.rect.min.y < 16.0).expect("the label's words");
        let whole = rig.width(ATTIC, body(SIZE)) <= label.room;
        assert_eq!(name.words == ATTIC, whole, "{locale} {mark:?}: whole where it fits: {:?} in {}", name.words, label.room);
        assert!(name.galley.size().x <= label.room + 0.01, "{locale}: cut to its room");
        if !whole {
            assert!(name.galley.rows.iter().any(|row| row.row.glyphs.last().is_some_and(|g| g.chr == '…')), "{locale}: cut with an ellipsis");
        }
        assert!(label.chevron.is_some_and(|c| c.min.x >= label.words_right), "{locale}: the chevron kept after the words");
    }
}

/// In Japanese and Chinese, whose tab names the CJK face behind the UI
/// face's draws, the tab that is up stands its name where it stands at
/// rest, the semibold's as the regular's, and so does the lit Visualizer's
/// word: within half a point, whole inside the strip. Japanese's up tab
/// once stood 4 points lower, its feet through the amber rule and cut off
/// at the strip's foot.
#[test]
fn a_cjk_name_stands_as_high_marked_or_lit_as_at_rest() {
    let Some(locale) = in_own_process(&["ja", "zh"]) else { return };
    let mut gui = gui_on(&[ATTIC, OFFICE]);
    let mut rig = Rig::new(100, 30);
    let ink = |shot: &Shot, words: &str| {
        let d = find(&drawn(&shot.full), words);
        d.galley.mesh_bounds.translate(d.rect.min.to_vec2())
    };
    gui.screen = Screen::NowPlaying;
    let rest = rig.settle(&strip::view(&gui));
    gui.screen = Screen::Library;
    let mut view = strip::view(&gui);
    view.viz.lit = true;
    let up = rig.settle(&view);
    for words in [view.tabs[0].label.clone(), view.viz.label.clone()] {
        let (at_rest, lit) = (ink(&rest, &words), ink(&up, &words));
        assert_eq!(find(&drawn(&up.full), &words).family, strong_family(), "{locale}: {words} in the semibold");
        assert!((lit.center().y - at_rest.center().y).abs() <= 0.5, "{locale}: {words} stands at {lit:?}, at rest at {at_rest:?}");
        assert!(lit.min.y >= up.slots.strip.min.y && lit.max.y <= up.slots.strip.max.y, "{locale}: {words} whole inside the strip: {lit:?}");
    }
}

/// The label's place in the room the strip leaves it: whole where its words
/// fit, cut where only part of them does, gone where not even a letter and
/// the ellipsis fit; its chevron last and kept, the whole right-aligned.
#[test]
fn the_label_is_whole_cut_or_gone_by_the_room_it_has() {
    let ctx = context();
    let input = RawInput { screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(800.0, 480.0))), ..Default::default() };
    let mut full = ctx.run_ui(input, |ui| {
        let strip = egui::Rect::from_min_max(Pos2::ZERO, pos2(800.0, 16.0));
        let mark = CHEVRON_GAP + CHEVRON_W;
        let whole = studio::width_of(ui, ATTIC, body(SIZE));
        let label = fit_label(ui, ATTIC, true, 700.0 - whole - mark, 700.0, strip).expect("room enough");
        assert!((label.room - whole).abs() < 0.01 && (label.rect.width() - whole - mark).abs() < 0.01, "whole in its own width");
        assert_eq!((label.rect.max.x, label.words_right), (700.0, 700.0 - mark));
        let cut = fit_label(ui, ATTIC, true, 700.0 - whole / 2.0 - mark, 700.0, strip).expect("cut");
        assert!(cut.rect.width() - mark <= whole / 2.0 + 0.01 && cut.rect.min.x >= 700.0 - whole / 2.0 - mark - 0.01, "cut to its room");
        assert_eq!(cut.chevron.map(|c| c.max.x), Some(700.0), "the chevron kept, last");
        let least = studio::width_of(ui, "h…", body(SIZE));
        assert!(fit_label(ui, ATTIC, true, 700.0 - mark - least + 0.5, 700.0, strip).is_none(), "gone under a letter and the ellipsis");
        assert!(fit_label(ui, ATTIC, true, 700.0 - mark - least - 0.5, 700.0, strip).is_some(), "a letter and the ellipsis");
        assert!(fit_label(ui, ATTIC, false, 700.0 - whole, 700.0, strip).is_some_and(|l| l.chevron.is_none() && l.rect.max.x == 700.0), "one server: no chevron");
        assert!(fit_label(ui, ATTIC, true, 720.0, 700.0, strip).is_none(), "no room at all");
    });
    full.textures_delta.clear();
}

/// A press on each item reports the act its cells register, on the press,
/// once though egui runs the pass again; a press on a gap of the strip or on
/// a single server's words reports none; the tab that is up takes its own,
/// which changes nothing, as its cells do.
#[test]
fn a_press_on_each_item_is_its_cells_act_once() {
    let _en = in_locale("en");
    let mut gui = gui_on(&[ATTIC, OFFICE]);
    let mut rig = Rig::new(100, 30);
    let view = strip::view(&gui);
    let slots = rig.settle(&view).slots;
    let label = slots.label.clone().expect("the label").rect;
    let items = [
        (slots.tabs[0], Act::Screen(Screen::Library)),
        (slots.tabs[1], Act::Screen(Screen::Stats)),
        (slots.tabs[2], Act::Screen(Screen::Admin)),
        (slots.tabs[3], Act::Screen(Screen::Device)),
        (slots.viz, Act::VizWindow),
        (slots.plus, Act::SrvAdd),
        (label, Act::SrvMenu),
    ];
    for (at, act) in &items {
        // Every point of an item answers for it, its padding too.
        for x in [at.min.x + 1.0, at.center().x, at.max.x - 1.0] {
            let shot = rig.press(&view, pos2(x, 8.0));
            assert_eq!(shot.acts, [act.clone()], "{act:?} at {x}");
        }
    }
    for gap in [slots.tabs[0].max.x + APART / 2.0, slots.divider + 4.0, (slots.viz.max.x + label.min.x) / 2.0] {
        assert!(rig.press(&view, pos2(gap, 8.0)).acts.is_empty(), "a gap at {gap} presses nothing");
    }
    let _ = rig.rest(&view, slots.tabs[1].center());
    rig.again = true;
    let shot = rig.pass(&view, vec![button(slots.tabs[1].center(), true)]);
    assert_eq!((shot.runs, shot.acts.clone()), (2, vec![Act::Screen(Screen::Stats)]), "once, though egui ran it again");
    let shot = rig.pass(&view, vec![button(slots.tabs[1].center(), false)]);
    assert!(shot.acts.is_empty(), "nothing on the release");

    gui.config.servers.truncate(1);
    let one = strip::view(&gui);
    let words = rig.settle(&one).slots.label.expect("one server's words").rect;
    assert!(rig.press(&one, words.center()).acts.is_empty(), "one server's words take no press");
    assert!(rig.settle(&one).out.items.iter().all(|(_, act)| *act != Act::SrvMenu), "and are no item");
}

/// A tap whose release comes in the pass its press does, as a touchpad's
/// tap and a click on a busy frame come: egui has let the press go by the
/// time an item asks where it came down, and each item still takes its
/// act, once, whether the pointer rested there first or came with the tap;
/// a tap on a gap or on a cell the registry left no part of the strip
/// takes none.
#[test]
fn a_tap_whose_release_comes_in_its_pass_is_its_items_press() {
    let _en = in_locale("en");
    let gui = gui_on(&[ATTIC, OFFICE]);
    let mut rig = Rig::new(100, 30);
    let view = strip::view(&gui);
    let slots = rig.settle(&view).slots;
    let label = slots.label.clone().expect("the label").rect;
    let tap = |at: Pos2| vec![button(at, true), button(at, false)];
    let items = [
        (slots.tabs[1], Act::Screen(Screen::Stats)),
        (slots.tabs[2], Act::Screen(Screen::Admin)),
        (slots.viz, Act::VizWindow),
        (slots.plus, Act::SrvAdd),
        (label, Act::SrvMenu),
    ];
    for (at, act) in &items {
        let _ = rig.rest(&view, at.center());
        assert_eq!(rig.pass(&view, tap(at.center())).acts, [act.clone()], "{act:?} tapped where the pointer rested");
        let _ = rig.rest(&view, pos2(slots.tabs[0].max.x + APART / 2.0, 8.0));
        let came = [vec![Event::PointerMoved(at.center())], tap(at.center())].concat();
        assert_eq!(rig.pass(&view, came).acts, [act.clone()], "{act:?} tapped as the pointer came");
        let _ = rig.pass(&view, Vec::new());
    }
    let gap = pos2(slots.tabs[0].max.x + APART / 2.0, 8.0);
    let _ = rig.rest(&view, gap);
    assert!(rig.pass(&view, tap(gap)).acts.is_empty(), "a gap takes no tap");
    let stats = slots.tabs[1];
    rig.live = vec![egui::Rect::from_min_max(Pos2::ZERO, pos2(stats.min.x, 16.0)), egui::Rect::from_min_max(pos2(stats.max.x, 0.0), pos2(800.0, 16.0))];
    let _ = rig.rest(&view, stats.center());
    assert!(rig.pass(&view, tap(stats.center())).acts.is_empty(), "nor a cell the registry left no part of the strip");
}

/// Hover lights only what a press would work, and the hand shows there:
/// a tab not up in the text colour and the semibold; the tab that is up and
/// the lit Visualizer as they were, with the hand over their presses; [+]
/// raised; the label and its chevron in the text colour while there is a
/// menu, and nothing for one server's words; nothing on a gap, nor where
/// the cells laid something over the strip, nor while hover may not.
#[test]
fn hover_lights_only_what_would_act_with_the_hand() {
    let _en = in_locale("en");
    let pal = Pal::now();
    let mut gui = gui_on(&[ATTIC, OFFICE]);
    let mut rig = Rig::new(100, 30);
    let mut view = strip::view(&gui);
    view.viz.lit = true;
    let slots = rig.settle(&view).slots;
    let hand = |shot: &Shot| shot.full.platform_output.cursor_icon == egui::CursorIcon::PointingHand;

    let shot = rig.rest(&view, slots.tabs[1].center());
    let stats = find(&drawn(&shot.full), "Stats");
    assert!(hand(&shot) && (stats.color, &stats.family) == (pal.text, &strong_family()), "Stats lit, the hand");
    let shot = rig.rest(&view, slots.tabs[0].center());
    let library = find(&drawn(&shot.full), "Library");
    assert!(hand(&shot) && (library.color, &library.family) == (pal.text, &strong_family()), "the up tab as it was, with the hand");
    assert_eq!(fills(&shot.full, pal.raised), [slots.tabs[0]], "and no fill but its own");
    let shot = rig.rest(&view, slots.viz.center());
    let word = find(&drawn(&shot.full), "Visualizer");
    assert!(hand(&shot) && (word.color, &word.family) == (pal.text, &strong_family()), "the lit Visualizer as it was, with the hand");
    let shot = rig.rest(&view, slots.plus.center());
    assert!(hand(&shot) && fills(&shot.full, pal.raised).iter().any(|r| r.width() == ADD_SQUARE && slots.plus.contains(r.center())), "[+] raised, the hand");
    assert_eq!(strokes(&shot.full, pal.text).iter().filter(|[a, b]| slots.plus.contains(*a) && slots.plus.contains(*b)).count(), 2, "its plus in the text colour");
    let label = slots.label.clone().unwrap();
    let shot = rig.rest(&view, label.rect.center());
    let words = find(&drawn(&shot.full), ATTIC);
    assert!(hand(&shot) && (words.color, &words.family) == (pal.text, &strong_family()), "the label lit, the hand");
    assert_eq!(strokes(&shot.full, pal.text).iter().filter(|[a, _]| label.chevron.unwrap().contains(*a)).count(), 2, "its chevron too");
    let shot = rig.rest(&view, pos2(slots.tabs[1].max.x + APART / 2.0, 8.0));
    assert!(!hand(&shot), "nothing on a gap");

    // A cells layer over Stats' cells: lit and pressed only on the cells
    // the registry left the strip.
    let hole = egui::Rect::from_min_max(pos2(slots.tabs[1].center().x - 4.0, 0.0), pos2(slots.tabs[1].max.x + 4.0, 16.0));
    rig.live = vec![egui::Rect::from_min_max(Pos2::ZERO, pos2(hole.min.x, 16.0)), egui::Rect::from_min_max(pos2(hole.max.x, 0.0), pos2(800.0, 16.0))];
    let shot = rig.rest(&view, hole.center());
    assert!(!hand(&shot) && find(&drawn(&shot.full), "Stats").color == pal.dim, "under the cells' layer: no light, no hand");
    assert!(rig.press(&view, hole.center()).acts.is_empty(), "nor a press");
    assert_eq!(rig.press(&view, pos2(slots.tabs[1].min.x + 2.0, 8.0)).acts, [Act::Screen(Screen::Stats)], "the live part of it still presses");
    rig.live = vec![row0(rig.cells)];

    // Hover may not (a popup holds the pointer): no light; the press still
    // the strip's where it is live.
    rig.hover = false;
    let shot = rig.rest(&view, slots.tabs[1].center());
    assert_eq!(find(&drawn(&shot.full), "Stats").color, pal.dim, "no light while hover may not");
    rig.hover = true;

    gui.config.servers.truncate(1);
    let one = strip::view(&gui);
    let words = rig.settle(&one).slots.label.unwrap().rect;
    let shot = rig.rest(&one, words.center());
    assert!(!hand(&shot) && find(&drawn(&shot.full), ATTIC).color == pal.dim, "one server's words: no light, no hand");
}

/// During an MP3 write the strip takes nothing: the chrome leaves it no
/// live cell and the view holds its tabs. It is drawn at rest, no greying:
/// the marked tab, its figure and the lit lamp kept; no hover, no hand, no
/// tooltip, no press anywhere on it.
#[test]
fn during_a_write_the_strip_is_drawn_at_rest_and_takes_nothing() {
    let _en = in_locale("en");
    let pal = Pal::now();
    let mut gui = gui_on(&[ATTIC, OFFICE]);
    gui.screen = Screen::Device;
    let mut view = strip::view(&gui);
    view.live = false;
    view.viz.lit = true;
    view.mark = Some(SyncMark { pct: 62, paused: false });
    let mut rig = Rig::new(100, 30);
    rig.live.clear();
    let slots = rig.settle(&view).slots;
    for at in [slots.tabs[0].center(), slots.tabs[1].center(), slots.viz.center(), slots.plus.center(), slots.label.clone().unwrap().rect.center()] {
        rig.time += 1.0;
        let _ = rig.rest(&view, at);
        rig.time += 1.0;
        let shot = rig.pass(&view, Vec::new());
        assert_ne!(shot.full.platform_output.cursor_icon, egui::CursorIcon::PointingHand, "no hand at {at:?}");
        assert!(shot.out.tip.is_none() && shot.out.items.is_empty(), "no tooltip, no item at {at:?}");
        let texts = drawn(&shot.full);
        assert_eq!(find(&texts, "Stats").color, pal.dim);
        assert_eq!(find(&texts, ATTIC).color, pal.dim);
        assert_eq!(find(&texts, "MP3 Player").color, pal.text, "the marked tab kept");
        assert_eq!(find(&texts, "62%").color, pal.accent, "its figure kept");
        assert!(fills(&shot.full, pal.accent).iter().any(|r| slots.viz.contains_rect(*r) && r.width() == LAMP), "the lit lamp kept");
        assert!(!fills(&shot.full, pal.raised).iter().any(|r| r.width() == ADD_SQUARE), "[+] at rest");
        assert!(rig.press(&view, at).acts.is_empty(), "no press at {at:?}");
    }
}

/// The strip's two tooltips ripen once the pointer has rested on their
/// item for the cells' dwell, and egui asks for a pass when it ends, not
/// before: Studio's box 4 points under the strip, centred under the item
/// and pulled in at the window's edge; the Visualizer's " — V" only with
/// key hints on, its key in the monospace in the secondary ink; none in
/// the dwell after a key, none while hover may not, none on the label.
#[test]
fn the_tooltips_ripen_after_the_dwell_under_their_items() {
    let _en = in_locale("en");
    let pal = Pal::now();
    let gui = gui_on(&[ATTIC, OFFICE]);
    let mut rig = Rig::new(100, 30);
    let mut view = strip::view(&gui);
    let slots = rig.settle(&view).slots;
    let dwell = crate::kit::TIP_DELAY.as_secs_f64();
    let ripe = |rig: &mut Rig, view: &StripView, at: Pos2| -> (Shot, Shot) {
        rig.time += 10.0;
        let _ = rig.rest(view, at);
        rig.time += dwell * 0.2;
        let early = rig.pass(view, Vec::new());
        rig.time += dwell;
        (early, rig.pass(view, Vec::new()))
    };
    let delay = |shot: &Shot| shot.full.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay.as_secs_f64());

    let (early, shot) = ripe(&mut rig, &view, slots.viz.center());
    assert!(early.out.tip.is_none(), "not in the dwell");
    let asked = delay(&early).expect("a pass asked for");
    assert!(asked > dwell * 0.6 && asked <= dwell * 0.8 + 0.01, "when the dwell ends: {asked}");
    let tip = shot.out.tip.expect("ripe after the dwell");
    let words = find(&drawn(&shot.full), "Open the visualizer in its own window");
    assert!(tip.contains_rect(words.rect.shrink(0.5)), "its words in its box");
    assert_eq!(tip.min.y, slots.strip.max.y + studio::TIP_UNDER, "4 points under the strip");
    assert!((tip.center().x - slots.viz.center().x).abs() <= 1.0, "centred under the item");
    assert!(fills(&shot.full, pal.raised).contains(&tip), "Studio's raised box");

    view.key_hints = true;
    let (_, shot) = ripe(&mut rig, &view, slots.viz.center());
    let keyed = find(&drawn(&shot.full), "Open the visualizer in its own window — V");
    let last = keyed.galley.job.sections.last().unwrap();
    assert_eq!((&last.format.font_id.family, last.format.color), (&FontFamily::Monospace, pal.soft), "the key in the monospace, secondary");
    assert_eq!(&keyed.galley.job.text[last.byte_range.start.0..last.byte_range.end.0], "V");
    let row = keyed.galley.rows.first().unwrap();
    let on = |c: char| row.row.glyphs.iter().find(|g| g.chr == c).map(|g| row.pos.y + g.pos.y).expect("drawn");
    assert!((on('V') - on('O')).abs() < 0.5, "the key on the words' baseline: {} {}", on('V'), on('O'));
    let tail = rig.width(" — ", body(studio::TIP_SIZE)) + rig.width("V", mono(studio::TIP_SIZE));
    let one_line = rig.width("Open the visualizer in its own window", body(studio::TIP_SIZE)) + tail <= studio::TIP_WORDS;
    assert_eq!(keyed.galley.rows.len() == 1, one_line, "wrapped in points past the cells' 40 cells, not in their count");
    // Past Studio's box but within the cells' 40 cells, the board's line
    // stays one; past those it wraps.
    let mut words = String::from("ab");
    while rig.width(&words, body(studio::TIP_SIZE)) + tail <= studio::TIP_W - 2.0 * studio::TIP_PAD.x + 4.0 {
        words.push_str(" ab");
    }
    assert!(rig.width(&words, body(studio::TIP_SIZE)) + tail < studio::TIP_WORDS - 1.0, "a line between the two");
    let whole = view.viz.tip.clone();
    for (tip, lines) in [(format!("{words} — V"), 1), (format!("{words} {words} — V"), 2)] {
        view.viz.tip = tip.clone();
        let (_, shot) = ripe(&mut rig, &view, slots.viz.center());
        assert_eq!(find(&drawn(&shot.full), &tip).galley.rows.len(), lines, "{tip:?}");
        let box_ = shot.out.tip.expect("ripe");
        assert!(rig.cells.points.contains_rect(box_), "inside the window: {box_:?}");
    }
    view.viz.tip = whole;
    view.key_hints = false;

    let (_, shot) = ripe(&mut rig, &view, slots.plus.center());
    let tip = shot.out.tip.expect("[+]'s");
    let _ = find(&drawn(&shot.full), "Add a server");
    assert_eq!(tip.max.x, rig.cells.points.max.x, "pulled in at the window's edge");

    rig.since_key = Some(Duration::from_millis(100));
    let (_, shot) = ripe(&mut rig, &view, slots.viz.center());
    assert!(shot.out.tip.is_none(), "none in the dwell after a key");
    rig.since_key = None;
    rig.hover = false;
    let (_, shot) = ripe(&mut rig, &view, slots.viz.center());
    assert!(shot.out.tip.is_none(), "none while hover may not");
    rig.hover = true;
    let (_, shot) = ripe(&mut rig, &view, slots.label.unwrap().rect.center());
    assert!(shot.out.tip.is_none(), "none on the label");
}

/// The pass the pointer comes to rest on an item in may be the last the
/// host runs before its tooltip is due: egui asks there for a pass at once,
/// as after any input, which the chrome's host drops (`panel::chrome_repaint`),
/// and keeps no other ask beside it. So the strip says from that very pass
/// when the dwell ends, and a pass then, with nothing new, shows the
/// tooltip; a move of a point on the item keeps it up. A slow sweep across
/// the item, never still, ripens it the dwell after the pointer came on, as
/// the cells' dwell does.
#[test]
fn a_tooltip_shows_on_the_wake_its_dwell_asks_from_the_pass_the_pointer_came_in() {
    let _en = in_locale("en");
    let gui = gui_on(&[ATTIC, OFFICE]);
    let mut rig = Rig::new(100, 30);
    let view = strip::view(&gui);
    let slots = rig.settle(&view).slots;
    let dwell = crate::kit::TIP_DELAY.as_secs_f64();
    let away = pos2(4.0, rig.cells.points.max.y - 4.0);
    for (what, item) in [("the Visualizer", slots.viz), ("[+]", slots.plus)] {
        rig.time += 10.0;
        let gone = rig.pass(&view, vec![Event::PointerMoved(away)]);
        assert_eq!((gone.out.tip, gone.out.wake), (None, None), "{what}: the pointer off the strip");
        rig.time += 1.0;
        let moved = rig.pass(&view, vec![Event::PointerMoved(item.center())]);
        assert!(moved.out.tip.is_none(), "{what}: not in the dwell");
        let asked = moved.full.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay);
        assert_eq!(asked, Some(Duration::ZERO), "{what}: egui's own ask is at once, the dwell folded into it");
        let wake = moved.out.wake.unwrap_or_else(|| panic!("{what}: the dwell's wake from the move's pass")).as_secs_f64();
        assert!((wake - dwell).abs() < 1e-6, "{what}: the whole dwell: {wake}");
        rig.time += wake;
        let shot = rig.pass(&view, Vec::new());
        let tip = shot.out.tip.unwrap_or_else(|| panic!("{what}: shown on the wake"));
        assert_eq!(shot.out.wake, None, "{what}: shown, no wake");
        rig.time += 0.05;
        let nudged = rig.pass(&view, vec![Event::PointerMoved(item.center() + vec2(1.0, 0.0))]);
        assert_eq!(nudged.out.tip, Some(tip), "{what}: a point's move on the item keeps it up");
    }

    rig.time += 10.0;
    let _ = rig.pass(&view, vec![Event::PointerMoved(away)]);
    let start = rig.time + 1.0;
    let step = (slots.viz.width() - 4.0) / 5.0;
    for k in 0..5 {
        rig.time = start + 0.1 * f64::from(k);
        let at = pos2(slots.viz.min.x + 2.0 + step * k as f32, slots.viz.center().y);
        let shot = rig.pass(&view, vec![Event::PointerMoved(at)]);
        assert!(shot.out.tip.is_none(), "sweeping, {k}: not yet");
        let wake = shot.out.wake.expect("a wake").as_secs_f64();
        assert!((wake - (dwell - 0.1 * f64::from(k))).abs() < 1e-6, "sweeping, {k}: the dwell from the pointer's coming on: {wake}");
    }
    rig.time = start + dwell;
    let shot = rig.pass(&view, vec![Event::PointerMoved(pos2(slots.viz.max.x - 2.0, slots.viz.center().y))]);
    assert!(shot.out.tip.is_some(), "ripe the dwell after the pointer came on, though it never stood still");
}

/// The chrome's pass over the Gui's last frame, as the host runs it, with
/// `events`: its runs' presses added up, the last run's spots and scroll.
fn chrome_pass(ctx: &egui::Context, gui: &Gui, cells: Cells, events: Vec<Event>) -> Out {
    let grid = cells.grid;
    let live = chrome::live(gui, grid);
    let popup = !live.popup.is_empty();
    let views = Views {
        strip: Some(strip::view(gui)),
        menu: servers::menu_view(gui),
        bar: None,
        hot: None,
        quick: None,
        hover: !popup,
        live,
        since_key: None,
        holes: chrome::holes(gui, grid),
    };
    let input = RawInput { screen_rect: Some(cells.points), events, ..Default::default() };
    let mut out = Out::default();
    let mut full = ctx.run_ui(input, |ui| {
        let mut run = Out::default();
        chrome::draw(ui, cells, &views, &mut run);
        out.presses.extend(run.presses);
        out.spots = run.spots;
        out.scroll = run.scroll;
        out.parts = run.parts;
    });
    full.textures_delta.clear();
    out
}

/// A press on what works `act` this frame, through the chrome's pass: the
/// act it reports, the press left for the frame's duties.
fn press_through(ctx: &egui::Context, gui: &mut Gui, cells: Cells, act: &Act) -> Option<Act> {
    frame(gui, cells);
    let spots = chrome_pass(ctx, gui, cells, Vec::new()).spots;
    let at = spots.iter().rev().find(|(_, a)| a == act).unwrap_or_else(|| panic!("{act:?} drawn: {spots:?}")).0.center();
    let _ = chrome_pass(ctx, gui, cells, vec![Event::PointerMoved(at)]);
    let out = chrome_pass(ctx, gui, cells, vec![button(at, true)]);
    let _ = chrome_pass(ctx, gui, cells, vec![button(at, false)]);
    assert_eq!(out.presses.len(), 1, "one press");
    gui.chrome.presses.extend(out.presses.iter().map(|(at, act)| (cells.under(*at), act.clone())));
    out.presses[0].1.clone()
}

/// A press on what works `act`, applied by the frame's duties.
fn press_and_apply(ctx: &egui::Context, gui: &mut Gui, cells: Cells, act: &Act) {
    assert_eq!(press_through(ctx, gui, cells, act).as_ref(), Some(act));
    assert!(chrome::after_draw(gui), "the frame's duties ran it");
}

/// Through the chrome's pass and the frame's duties, as the window runs
/// them, each item's press does what its cells' press does to the Gui: the
/// tabs switch screens, the up one changes nothing, [+] opens the add
/// form's chooser, the label opens the menu; the MP3 Player tab and the
/// Visualizer report their acts (opening a device page or a window is no
/// test's to do). Then, with the menu up, its rows: the session's server
/// while it answers only closes it, another reports its switch, the add row
/// opens the form, and a press off its rows, the label among them, closes
/// it, as the cells' catcher does; Esc closes it by the cells' keys.
#[test]
fn a_press_applies_the_cells_act_through_the_frames_duties() {
    let _en = in_locale("en");
    let ctx = context();
    let cells = cells_at(100, 30);
    // No session answers: the Stats and Admin screens open on no server,
    // with nothing to ask one.
    let mut gui = gui_on(&[ATTIC, OFFICE]);
    for screen in [Screen::Stats, Screen::Admin, Screen::Library, Screen::Library] {
        press_and_apply(&ctx, &mut gui, cells, &Act::Screen(screen));
        assert_eq!(gui.screen, screen);
    }
    for act in [Act::Screen(Screen::Device), Act::VizWindow] {
        assert_eq!(press_through(&ctx, &mut gui, cells, &act), Some(act));
        gui.chrome.presses.clear();
    }
    press_and_apply(&ctx, &mut gui, cells, &Act::SrvAdd);
    assert!(gui.servers.form.is_some(), "[+] opened the add form");
    gui.act(Act::FormCancel);
    press_and_apply(&ctx, &mut gui, cells, &Act::SrvMenu);
    assert!(gui.servers.drop_open, "the label opened the menu");

    let office = gui.config.servers.iter().position(|e| e.url == OFFICE).unwrap();
    assert_eq!(press_through(&ctx, &mut gui, cells, &Act::SrvSwitch(office)), Some(Act::SrvSwitch(office)), "another server's row");
    gui.chrome.presses.clear();
    // The session's own server, answering: the switch ends before it asks
    // for anything.
    gui.app.connected = true;
    press_and_apply(&ctx, &mut gui, cells, &Act::SrvSwitch(0));
    assert!(!gui.servers.drop_open, "the session's own row, answering: closed, nothing else");
    assert_eq!(gui.app.session.server_id, ATTIC);

    gui.act(Act::SrvMenu);
    press_and_apply(&ctx, &mut gui, cells, &Act::SrvAdd);
    assert!(!gui.servers.drop_open && gui.servers.form.is_some(), "the add row: the form, the menu put away");
    gui.act(Act::FormCancel);

    gui.act(Act::SrvMenu);
    press_and_apply(&ctx, &mut gui, cells, &Act::SrvCloseDrop);
    assert!(!gui.servers.drop_open, "a press off its rows closes it");

    // The label's own cells, under the open menu, are the catcher's: it
    // stacks over the label in the dumps, and takes the press.
    gui.act(Act::SrvMenu);
    frame(&mut gui, cells);
    let spots = chrome_pass(&ctx, &gui, cells, Vec::new()).spots;
    let at = spots.iter().find(|(_, act)| *act == Act::SrvMenu).map(|(rect, _)| rect.center()).expect("the label");
    let top = spots.iter().rev().find(|(rect, _)| rect.contains(at)).map(|(_, act)| act.clone());
    assert_eq!(top, Some(Act::SrvCloseDrop), "the catcher over the label");
    let _ = chrome_pass(&ctx, &gui, cells, vec![Event::PointerMoved(at)]);
    let out = chrome_pass(&ctx, &gui, cells, vec![button(at, true)]);
    assert_eq!(out.presses.iter().map(|(_, act)| act.clone()).collect::<Vec<_>>(), [Some(Act::SrvCloseDrop)], "a press on the label closes it");

    handle_key(&mut gui, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!gui.servers.drop_open, "Esc closes it");
}

/// A tap, its release in its press's pass, through the chrome's pass and
/// the frame's duties: the Stats tab's switches the screen, the label's
/// opens the menu, and a row's tap there switches to its server, as the
/// cells' click on each does.
#[test]
fn a_tap_applies_the_cells_act_through_the_frames_duties() {
    let _en = in_locale("en");
    let ctx = context();
    let cells = cells_at(100, 30);
    let mut gui = gui_on(&[ATTIC, OFFICE]);
    let tap = |gui: &mut Gui, act: &Act| {
        frame(gui, cells);
        let spots = chrome_pass(&ctx, gui, cells, Vec::new()).spots;
        let at = spots.iter().rev().find(|(_, a)| a == act).unwrap_or_else(|| panic!("{act:?} drawn: {spots:?}")).0.center();
        let _ = chrome_pass(&ctx, gui, cells, vec![Event::PointerMoved(at)]);
        let out = chrome_pass(&ctx, gui, cells, vec![button(at, true), button(at, false)]);
        assert_eq!(out.presses.iter().map(|(_, act)| act.clone()).collect::<Vec<_>>(), [Some(act.clone())], "one press, {act:?}'s");
        gui.chrome.presses.extend(out.presses.iter().map(|(at, act)| (cells.under(*at), act.clone())));
        assert!(chrome::after_draw(gui), "the frame's duties ran it");
    };
    tap(&mut gui, &Act::Screen(Screen::Stats));
    assert_eq!(gui.screen, Screen::Stats);
    tap(&mut gui, &Act::SrvMenu);
    assert!(gui.servers.drop_open, "the label's tap opened the menu");
    gui.app.connected = true;
    tap(&mut gui, &Act::SrvSwitch(0));
    assert!(!gui.servers.drop_open, "the session's own row, tapped: closed");
}

/// The menu hangs the kit's list from the strip's foot, its right edge a
/// cell in from the window's: the servers grouped as the view orders them
/// (a peer under its parent, its way in dim after its name), the session's
/// in amber and the semibold behind the pointer mark, the default's gold
/// star at its right, the add row last and dim; the row under the pointer
/// raised, its words in the text colour and the semibold, the hand.
#[test]
fn the_menu_hangs_its_rows_grouped_from_the_strips_foot() {
    let _en = in_locale("en");
    let pal = Pal::now();
    let ctx = context();
    let cells = cells_at(100, 30);
    let mut gui = gui_on(&[ATTIC, OFFICE]);
    add_peer(&mut gui, ATTIC, "Jazz Room");
    gui.config.default_server = Some(ATTIC.into());
    gui.servers.drop_open = true;
    frame(&mut gui, cells);
    let view = servers::menu_view(&gui).expect("open");
    let pass = |events: Vec<Event>| -> FullOutput {
        let live = vec![cells.points];
        let input = RawInput { screen_rect: Some(cells.points), events, ..Default::default() };
        let mut full = ctx.run_ui(input, |ui| {
            studio::style_once(ui.ctx(), &pal);
            let _ = menu(ui, &view, &MenuCtx { cells, live: &live, pal: &pal });
        });
        full.textures_delta.clear();
        full
    };
    let _ = pass(Vec::new());
    let full = pass(Vec::new());
    let texts = drawn(&full);
    let (current, peer, office, add) = (find(&texts, ATTIC), find(&texts, &format!("└ Jazz Room · via {ATTIC}")), find(&texts, OFFICE), find(&texts, "+ Add a server"));
    assert!(current.rect.min.y < peer.rect.min.y && peer.rect.min.y < office.rect.min.y && office.rect.min.y < add.rect.min.y, "grouped, the add row last");
    assert_eq!((current.color, &current.family), (pal.accent, &strong_family()), "the session's server in amber and the semibold");
    assert_eq!((office.color, &office.family), (pal.text, &FontFamily::Proportional));
    assert_eq!(add.color, pal.dim, "the add row dim");
    let tail = peer.galley.job.sections.last().unwrap();
    assert_eq!((&peer.galley.job.text[tail.byte_range.start.0..tail.byte_range.end.0], tail.format.color), (format!(" · via {ATTIC}").as_str(), pal.dim), "the peer's way in dim");
    assert!(current.rect.min.x == office.rect.min.x && office.rect.min.x == add.rect.min.x, "the names in one column");
    let mark = polygons(&full, pal.accent);
    assert!(mark.iter().any(|points| points.iter().all(|p| p.x < current.rect.min.x && (p.y - current.rect.center().y).abs() < 8.0)), "the pointer mark in the lead");
    // The kit's star, whole: its fill and its outline, both gold.
    let star: Vec<egui::Rect> = full
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Path(path) if path.closed && path.stroke.color == egui::epaint::ColorMode::Solid(pal.gold) => Some(egui::Rect::from_points(&path.points)),
            Shape::Mesh(mesh) if !mesh.vertices.is_empty() && mesh.vertices.iter().all(|v| v.color == pal.gold) => Some(mesh.calc_bounds()),
            _ => None,
        })
        .collect();
    assert_eq!(star.len(), 2, "the default's star, filled and outlined: {star:?}");
    assert!(star.iter().all(|r| r.min.x > current.rect.max.x && (r.center().y - current.rect.center().y).abs() < 8.0), "at the row's right: {star:?}");

    let frame_rect = fills(&full, pal.panel).into_iter().max_by(|a, b| a.area().total_cmp(&b.area())).expect("the list's frame");
    assert_eq!((frame_rect.min.y, frame_rect.max.x), (16.0, 792.0), "from the strip's foot, a cell in from the window's edge");

    let _ = pass(vec![Event::PointerMoved(office.rect.center())]);
    let full = pass(Vec::new());
    let lit = find(&drawn(&full), OFFICE);
    assert_eq!((lit.color, &lit.family), (pal.text, &strong_family()), "the row under the pointer");
    assert!(fills(&full, pal.raised).iter().any(|r| r.contains(lit.rect.center()) && r.height() == studio::HUNG_ROW_H), "raised");
    assert_eq!(full.platform_output.cursor_icon, egui::CursorIcon::PointingHand);
    let _ = pass(vec![Event::PointerMoved(current.rect.center())]);
    let full = pass(Vec::new());
    assert_eq!(find(&drawn(&full), ATTIC).color, pal.accent, "the session's server keeps its look under the pointer");
}

/// Past its room the menu shows the rows from the wheel's first, held so
/// the last row ends it and handed back, with the kit's thumb; a press on
/// the thumb's gutter is its row's, as the cells' row runs the menu's whole
/// width; the wheel is the cells': it moves the first row the view hands
/// over. Where the cells stack something over the menu it takes nothing.
#[test]
fn the_menu_scrolls_by_the_wheel_and_takes_no_press_under_the_cells() {
    let _en = in_locale("en");
    let pal = Pal::now();
    let ctx = context();
    let cells = cells_at(100, 24);
    let urls: Vec<String> = (0..12).map(|k| format!("http://host{k}.local:3000")).collect();
    let mut gui = gui_on(&urls.iter().map(String::as_str).collect::<Vec<_>>());
    gui.servers.drop_open = true;
    servers::wheel_dropdown(&mut gui, 30);
    let view = servers::menu_view(&gui).expect("open");
    let fits = studio::hung_fits(16.0, 16.0 + 16.0 * 16.0);
    assert!(fits < view.rows.len() + 1, "more rows than room at the floor");
    let pass = |live: &[egui::Rect], events: Vec<Event>| -> (MenuOut, FullOutput) {
        let input = RawInput { screen_rect: Some(cells.points), events, ..Default::default() };
        let mut out = MenuOut::default();
        let mut full = ctx.run_ui(input, |ui| {
            studio::style_once(ui.ctx(), &pal);
            let run = menu(ui, &view, &MenuCtx { cells, live, pal: &pal });
            out.act = out.act.take().or(run.act);
            out.items = run.items;
            out.scroll = run.scroll;
            out.frame = run.frame;
        });
        full.textures_delta.clear();
        (out, full)
    };
    let all = [cells.points];
    let _ = pass(&all, Vec::new());
    let (out, full) = pass(&all, Vec::new());
    let first = 13 - fits;
    assert_eq!(out.scroll, Some(first), "held so the add row ends it");
    let texts = drawn(&full);
    assert!(!texts.iter().any(|d| d.words == urls[first - 1]) && texts.iter().any(|d| d.words == urls[first]), "from the first row the wheel left");
    let _ = find(&texts, "+ Add a server");
    let thumb = fills(&full, pal.dim.gamma_multiply(studio::THUMB_REST));
    assert_eq!(thumb.len(), 1, "the kit's thumb");

    let (row, act) = out.items.iter().find(|(_, act)| *act == Act::SrvSwitch(first)).cloned().expect("its first row");
    let gutter = pos2(row.max.x - studio::HUNG_GUTTER / 2.0, row.center().y);
    let press = |live: &[egui::Rect], at: Pos2| {
        let _ = pass(live, vec![Event::PointerMoved(at)]);
        let (out, _) = pass(live, vec![button(at, true)]);
        let _ = pass(live, vec![button(at, false)]);
        out.act
    };
    assert_eq!(press(&all, gutter), Some(act.clone()), "the gutter is its row's");
    assert_eq!(press(&all, row.center()), Some(act.clone()), "the row's");
    assert_eq!(press(&all, pos2(40.0, 200.0)), Some(Act::SrvCloseDrop), "off its rows the catcher's");
    assert_eq!(press(&[], row.center()), None, "nothing where the cells stack over it");
    // A tap, its release in the pass its press came in: the same acts.
    let tap = |live: &[egui::Rect], at: Pos2| {
        let _ = pass(live, vec![Event::PointerMoved(at)]);
        pass(live, vec![button(at, true), button(at, false)]).0.act
    };
    assert_eq!(tap(&all, row.center()), Some(act.clone()), "a tap on the row");
    assert_eq!(tap(&all, gutter), Some(act), "on the gutter");
    assert_eq!(tap(&all, pos2(40.0, 200.0)), Some(Act::SrvCloseDrop), "off its rows");
    assert_eq!(tap(&[], row.center()), None, "and none where the cells stack over it");
    let _ = pass(&[], vec![Event::PointerMoved(row.center())]);
    let (_, full) = pass(&[], Vec::new());
    assert_ne!(full.platform_output.cursor_icon, egui::CursorIcon::PointingHand, "nor the hand");
}

/// Nothing panics at the floor, at the opening size, wide, squeezed into a
/// window smaller than its grid, or with no room at all: the strip with a
/// sync and two servers, the menu with a dozen, each run twice.
#[test]
fn nothing_panics_small_squeezed_or_wide() {
    let _en = in_locale("en");
    let pal = Pal::now();
    let urls: Vec<String> = (0..12).map(|k| format!("http://host{k}.local:3000")).collect();
    let mut gui = gui_on(&urls.iter().map(String::as_str).collect::<Vec<_>>());
    gui.servers.drop_open = true;
    let squeezed = Cells { points: egui::Rect::from_min_size(Pos2::ZERO, vec2(200.0, 96.0)), grid: Rect::new(0, 0, 100, 24), ppp: 2.0 };
    let tiny = Cells { points: egui::Rect::from_min_size(Pos2::ZERO, vec2(8.0, 16.0)), grid: Rect::new(0, 0, 1, 1), ppp: 1.0 };
    let none = Cells { points: egui::Rect::from_min_size(Pos2::ZERO, vec2(0.0, 0.0)), grid: Rect::new(0, 0, 0, 0), ppp: 1.0 };
    for cells in [cells_at(100, 24), cells_at(100, 30), cells_at(160, 50), cells_at(400, 120), squeezed, tiny, none] {
        let mut view = strip::view(&gui);
        view.mark = Some(SyncMark { pct: 100, paused: false });
        let menu_view = servers::menu_view(&gui).expect("open");
        let ctx = context();
        for _ in 0..2 {
            let input = RawInput { screen_rect: Some(cells.points), ..Default::default() };
            let mut full = ctx.run_ui(input, |ui| {
                studio::style_once(ui.ctx(), &pal);
                let row = row0(cells);
                let mut part = ui.new_child(egui::UiBuilder::new().max_rect(row));
                let live = [row];
                let _ = show(&mut part, &view, &StripCtx { cells, live: &live, hover: true, since_key: None, pal: &pal });
                let all = [cells.points];
                let _ = menu(ui, &menu_view, &MenuCtx { cells, live: &all, pal: &pal });
            });
            full.textures_delta.clear();
        }
    }
}
