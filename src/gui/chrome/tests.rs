//! The chrome's hosting, headless: the faces forced on (no window, so no
//! pass runs in the frame and the registry's reading is what is tested),
//! and the pass's body run in a context of its own.

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::*;
use crate::config::{Config, ServerEntry};
use crate::gui::window::panel::Cells;
use crate::gui::{Screen, demo_now, render};
use crate::tui::app::{App, Focus};

const EVERY: Faces = Faces { strip: true, bar: true, quick: true };

/// The demo seat playing, the faces forced on.
fn forced_gui() -> Gui {
    let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
    gui.demo = Some(demo_now());
    gui.chrome.forced = Some(EVERY);
    gui
}

/// One frame at `w`×`h`, its rows.
fn draw_at(gui: &mut Gui, w: u16, h: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|frame| render(frame, gui)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..h).map(|y| (0..w).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect()
}

fn draw(gui: &mut Gui) -> Vec<String> {
    draw_at(gui, 100, 30)
}

fn grid() -> Rect {
    Rect::new(0, 0, 100, 30)
}

fn row0() -> Rect {
    Rect { x: 0, y: 0, width: 100, height: 1 }
}

/// Where egui draws the strip and the bar, the cells leave their rows
/// blank and register nothing on row 0; the bar is still registered, every
/// act at its layout's box; and row 0 is the chrome's to take whole.
#[test]
fn where_egui_draws_the_chrome_the_cells_leave_its_rows_and_keep_the_bars_acts() {
    let _en = crate::setup::tests::in_locale("en");
    let mut gui = forced_gui();
    let rows = draw(&mut gui);
    let layout = gui.chrome.bar.as_ref().expect("the bar's mark").layout.clone();
    assert!(rows[0].trim().is_empty(), "row 0 is egui's: {:?}", rows[0]);
    for y in layout.rect.top()..layout.rect.bottom() {
        assert!(rows[usize::from(y)].trim().is_empty(), "the bar's row {y} is egui's: {:?}", rows[usize::from(y)]);
    }
    for x in 0..100 {
        assert_eq!(gui.ui.hit(Position { x, y: 0 }), None, "nothing of the cells on row 0 at {x}");
    }
    for (rect, act) in &layout.group {
        assert_eq!(gui.ui.hit(Position { x: rect.x + 1, y: rect.y + 1 }), Some(act.clone()));
    }
    assert_eq!(gui.ui.hit(Position { x: layout.dj.word.x + 2, y: layout.dj.word.y + 1 }), Some(Act::AutoDj));
    assert_eq!(gui.ui.hit_context(Position { x: layout.card.rect.x + 1, y: layout.card.rect.y }), Some(Act::NowMore), "the card's right press, the demo seat's too");
    assert_eq!(gui.chrome.claims, Claims { live: vec![row0()], popup: false });
    assert!(chrome_free_of_cells_text(&rows), "the note is egui's too");
}

/// The note is egui's where egui paints the bar: no cells words on the
/// bar's bottom row.
fn chrome_free_of_cells_text(rows: &[String]) -> bool {
    !rows.iter().any(|row| row.contains("Cassini"))
}

/// The cells chrome stays wherever egui does not draw it: no faces, no
/// claims, the strip and the bar in cells as ever.
#[test]
fn without_the_faces_the_cells_draw_the_chrome_as_ever() {
    let _en = crate::setup::tests::in_locale("en");
    let mut gui = forced_gui();
    gui.chrome.forced = Some(Faces::default());
    let rows = draw(&mut gui);
    assert!(rows[0].contains(" Library ") && rows[0].contains("[+]"), "{:?}", rows[0]);
    assert!(rows.iter().any(|row| row.contains("Cassini IV")), "the card in cells");
    assert_eq!(gui.chrome.claims, Claims::default());
    assert!(gui.chrome.bar.is_none() && gui.chrome.strip.is_none());
}

/// Row 0 is the chrome's but during an MP3 write; under the add-server
/// form's guard and the track sheet's backdrop it is theirs; where a layer
/// dropped the registries after the strip's slot, nothing of the strip's
/// stands, and nothing panics on a stale count.
#[test]
fn the_strip_is_live_where_the_cells_registry_leaves_it() {
    let _en = crate::setup::tests::in_locale("en");
    let mut gui = forced_gui();
    draw(&mut gui);
    assert_eq!(live(&gui, grid()).strip, [row0()]);
    assert_eq!(live_with(&gui, grid(), true), Live::default(), "a write holds every press");

    gui.act(Act::SrvAdd);
    draw(&mut gui);
    assert_eq!(live(&gui, grid()), Live::default(), "the form's guard is over row 0");
    gui.act(Act::FormCancel);
    draw(&mut gui);
    assert_eq!(live(&gui, grid()).strip, [row0()]);

    gui.app.now_playing = Some(crate::api::types::Track { filepath: "a.mp3".into(), metadata: Default::default() });
    gui.act(Act::NowMore);
    assert!(crate::gui::actions::modal_open(&gui), "the sheet");
    draw(&mut gui);
    assert_eq!(live(&gui, grid()).strip, Vec::<Rect>::new(), "the sheet's backdrop is over row 0");
    assert_eq!(bar_hot(&gui, &gui.chrome.claims), None);
    gui.act(Act::SheetClose);
    draw(&mut gui);
    let play = gui.chrome.bar.as_ref().unwrap().layout.group[2].0;
    gui.ui.pointer = Some(Position { x: play.x + 1, y: play.y + 1 });
    draw(&mut gui);
    assert!(bar_hot(&gui, &Claims::default()).is_some(), "the bar lit");

    // A layer that drops the registries, then lays its guard.
    gui.ui.clear_registries();
    gui.ui.click(Rect::new(30, 10, 10, 2), Act::Guard);
    assert_eq!(live(&gui, grid()), Live::default(), "the strip's clicks would be gone with the drop");
    assert_eq!(bar_hot(&gui, &Claims::default()), None, "nor does anything of the bar light");
}

/// The server menu and quick config hold the whole grid while up, but
/// what the cells stack over them: a sheet a right press opened over the
/// open menu takes every press.
#[test]
fn a_popup_holds_the_grid_less_what_the_cells_stack_over_it() {
    let _en = crate::setup::tests::in_locale("en");
    let mut gui = forced_gui();
    gui.servers.drop_open = true;
    draw(&mut gui);
    let held = live(&gui, grid());
    assert_eq!(held.popup, [grid()], "the menu up: the whole grid");
    assert!(claims(&held).popup);
    assert_eq!(gui.chrome.claims.live.len(), 2, "row 0's and the grid");

    gui.app.now_playing = Some(crate::api::types::Track { filepath: "a.mp3".into(), metadata: Default::default() });
    gui.act(Act::NowMore);
    draw(&mut gui);
    assert!(gui.servers.drop_open, "the right press left the menu open");
    let held = live(&gui, grid());
    assert!(held.popup.is_empty() && !claims(&held).popup, "the sheet's backdrop over the whole grid: {held:?}");
    gui.act(Act::SheetClose);
    gui.servers.drop_open = false;

    gui.act(Act::DjQuick(crate::gui::dj::QuickAct::Toggle));
    assert!(gui.dj.quick.is_some());
    draw(&mut gui);
    assert_eq!(live(&gui, grid()).popup, [grid()], "quick config up: the whole grid");
    gui.act(Act::Screen(Screen::Stats));
    draw(&mut gui);
    assert!(gui.dj.quick.is_none(), "put down on a screen with no bar");
    assert!(live(&gui, grid()).popup.is_empty());
}

/// The bar lights the control under the pointer, as the cells registered
/// it; nothing under a modal's guard over it, under a popup, or where a
/// modal stashed the pointer for the screen.
#[test]
fn the_bar_lights_only_what_a_press_would_work() {
    let _en = crate::setup::tests::in_locale("en");
    let mut gui = forced_gui();
    draw(&mut gui);
    let play = gui.chrome.bar.as_ref().unwrap().layout.group[2].0;
    gui.ui.pointer = Some(Position { x: play.x + 1, y: play.y + 1 });
    draw(&mut gui);
    assert_eq!(gui.chrome.hot, Some((play, Act::PlayPause)));
    let cell = gui.chrome.bar.as_ref().unwrap().layout.volume.cells[4];
    gui.ui.pointer = Some(Position { x: cell.x, y: cell.y });
    draw(&mut gui);
    assert_eq!(gui.chrome.hot, Some((cell, Act::VolSet(4))));

    gui.ui.pointer = Some(Position { x: play.x + 1, y: play.y + 1 });
    gui.act(Act::SrvAdd);
    draw(&mut gui);
    assert_eq!(gui.chrome.hot, None, "under the form's guard");
    gui.act(Act::FormCancel);
    gui.servers.drop_open = true;
    draw(&mut gui);
    assert_eq!(gui.chrome.hot, None, "under the open menu");
    gui.servers.drop_open = false;
    gui.app.now_playing = Some(crate::api::types::Track { filepath: "a.mp3".into(), metadata: Default::default() });
    gui.act(Act::NowMore);
    draw(&mut gui);
    assert!(!gui.chrome.bar.as_ref().unwrap().pointer, "the sheet stashes the pointer for the screen");
    assert_eq!(gui.chrome.hot, None);
}

/// Each part is cut round the overlays the cells registered after its
/// slot, and paint in no part round every overlay: a layer the cells draw
/// under the menu cuts nothing of it, one drawn over it does.
#[test]
fn a_part_is_cut_round_only_what_the_cells_stack_over_it() {
    let mut gui = forced_gui();
    let (under, over) = (Rect::new(60, 2, 20, 6), Rect::new(10, 10, 40, 8));
    gui.ui.begin_frame();
    strip_mark(&mut gui);
    gui.ui.overlay(under);
    gui.servers.drop_open = true;
    menu_mark(&mut gui);
    gui.ui.overlay(over);
    let holes = holes(&gui, grid());
    assert_eq!(holes.menu, [over], "the layer under the menu cuts none of it");
    assert_eq!(holes.strip, [under, over], "the strip is under both");
    assert_eq!(holes.every, [under, over]);
    assert!(holes.bar.is_empty() && holes.quick.is_empty(), "no marks, nothing to cut");
}

/// The frame's duties run each press the chrome took, in order, through
/// the cells' preamble then its act: a press on a tab gives the keys back
/// from the queue and opens its screen; a press that pressed nothing runs
/// the preamble alone. Once: the presses are gone after.
#[test]
fn the_frames_duties_run_the_chromes_presses_once_preamble_first() {
    let mut gui = forced_gui();
    draw(&mut gui);
    gui.app.focus = Focus::Queue;
    gui.queue_view.held = true;
    gui.chrome.presses = vec![(Position { x: 99, y: 0 }, None), (Position { x: 10, y: 0 }, Some(Act::Screen(Screen::Stats)))];
    gui.hot = false;
    assert!(after_draw(&mut gui));
    assert_eq!(gui.screen, Screen::Stats);
    assert_eq!((gui.app.focus, gui.queue_view.held), (Focus::Browser, false), "the keys back from the queue");
    assert!(gui.hot, "the next frame comes briskly");
    assert!(gui.chrome.presses.is_empty());
    assert!(!after_draw(&mut gui), "nothing more");
}

/// The pass's body, run in a context of its own: a press on the Stats
/// tab's cells reports its act once, though egui runs the pass again, and
/// at its cell; every part reports where it stands.
#[test]
fn the_pass_reports_a_press_once_with_its_act_and_its_parts() {
    let _en = crate::setup::tests::in_locale("en");
    let mut gui = forced_gui();
    draw(&mut gui);
    let ctx = egui::Context::default();
    crate::gui::window::panel::fonts(&ctx);
    let cells = Cells { points: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 480.0)), grid: grid(), ppp: 1.0 };
    let stats = strip::top_rects(strip::TOP_KEYS.map(|key| rust_i18n::t!(key).to_string()).each_ref().map(String::as_str))[1];
    let at = cells.rect(stats).center();
    let now = gui.bar_now();
    // One pass, its runs' presses added up, the screen's facts its last
    // run's, as the host keeps them.
    let pass = |events: Vec<egui::Event>, again: bool| -> Out {
        let input = egui::RawInput { screen_rect: Some(cells.points), events, ..Default::default() };
        let live = live(&gui, grid());
        let views = views(&gui, grid(), now.as_ref(), live, None, holes(&gui, grid()));
        let mut asked = false;
        let mut out = Out::default();
        let mut output = ctx.run_ui(input, |ui| {
            let mut run = Out::default();
            super::draw(ui, cells, &views, &mut run);
            out.presses.extend(run.presses);
            out.parts = run.parts;
            out.spots = run.spots;
            if again && !asked {
                asked = true;
                ui.ctx().request_discard("a second run");
            }
        });
        output.textures_delta.clear();
        out
    };
    assert!(pass(vec![egui::Event::PointerMoved(at)], false).presses.is_empty());
    assert!(pass(Vec::new(), false).presses.is_empty());
    let press = egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::default() };
    let out = pass(vec![press], true);
    assert_eq!(out.presses.len(), 1, "once: {:?}", out.presses);
    assert_eq!(out.presses[0].1, Some(Act::Screen(Screen::Stats)));
    assert_eq!(cells.under(out.presses[0].0), Position { x: stats.x + stats.width / 2, y: 0 });
    let strip = cells.rect(row0());
    assert!(out.parts.iter().any(|part| part.rect == strip), "the strip's part");
    let layout = gui.chrome.bar.as_ref().unwrap().layout.clone();
    assert!(out.parts.iter().any(|part| part.rect == cells.rect(layout.rect)), "the bar's");
    assert!(out.spots.iter().any(|(_, act)| *act == Act::Screen(Screen::Stats)));
}

/// Nothing panics at the floor with the tips row under the bar, nor wide,
/// popups up and every face forced, the pass run in a context of its own.
#[test]
fn every_face_draws_at_the_floor_and_wide() {
    let _en = crate::setup::tests::in_locale("en");
    for (w, h, hints) in [(100, 24, true), (100, 30, false), (160, 50, false), (100, 24, false)] {
        let mut gui = forced_gui();
        gui.config.gui.key_hints = hints;
        gui.ui.key_hints = hints;
        gui.app.session.server = "http://attic.local:3000".into();
        gui.app.session.server_id = "http://attic.local:3000".into();
        for url in ["http://attic.local:3000", "http://office.local:3000"] {
            gui.config.servers.push(ServerEntry { url: url.into(), ..Default::default() });
        }
        gui.servers.drop_open = true;
        draw_at(&mut gui, w, h);
        let grid = Rect::new(0, 0, w, h);
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        let cells = Cells { points: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(f32::from(w) * 8.0, f32::from(h) * 16.0)), grid, ppp: 2.0 };
        let now = gui.bar_now();
        for popup in [true, false] {
            gui.servers.drop_open = popup;
            if !popup {
                gui.act(Act::DjQuick(crate::gui::dj::QuickAct::Toggle));
            }
            draw_at(&mut gui, w, h);
            let views = views(&gui, grid, now.as_ref(), live(&gui, grid), gui.chrome.hot.clone(), holes(&gui, grid));
            assert!(popup == views.menu.is_some() && popup != views.quick.is_some(), "{w}×{h}: the popup's view");
            for _ in 0..2 {
                let input = egui::RawInput { screen_rect: Some(cells.points), ..Default::default() };
                let mut output = ctx.run_ui(input, |ui| super::draw(ui, cells, &views, &mut Out::default()));
                output.textures_delta.clear();
            }
        }
    }
}
