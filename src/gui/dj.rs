//! Auto DJ in the GUI shell (docs/ux-contracts/auto-dj.md): the empty-queue
//! chooser as a kit modal (clause 2), the opening-song banner (clause 4),
//! and the acts the queue panel's empty state sends (clause 16). The room
//! itself — the settings rows — is the next slice; the bar's `auto-dj`
//! toggle and `A` already ride the shared App's toggle (entry point 1).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::dj::EmptyQueueStart;
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{dim, modal_close, modal_frame_on};
use crate::tui::app::{Action, Capture};

use super::{Act, Gui, accent, bright_bold, put, sel};

/// The chooser's three rows: the two answers, then the remember box.
const ROWS: usize = 3;

/// Whether the chooser owns the pointer and keyboard.
pub(crate) fn modal_open(gui: &Gui) -> bool {
    gui.app.dj_chooser.is_some()
}

/// "Start Auto DJ with what?" (clause 2): a kit modal over everything — the
/// two answers with their descriptions, the remember box, Esc to leave the
/// DJ off. The App holds the row and the box; the shell draws and relays.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(chooser) = gui.app.dj_chooser.clone() else { return };
    // A click anywhere else dismisses — the DJ stays off.
    gui.ui.click(area, Act::DjCancel);
    let width: u16 = 74.min(area.width.saturating_sub(2)).max(40);
    let text_w = width as usize - 4;
    let subtitle = super::sonic::wrap(&t!("gui.dj.start_subtitle"), text_w);
    let (check_on, check_off) = if legacy_conhost() { ("[x]", "[ ]") } else { ("[✓]", "[ ]") };
    let rows: [(String, Vec<String>, Act); ROWS] = [
        (t!("gui.dj.surprise").to_string(), super::sonic::wrap(&t!("gui.dj.surprise_sub"), text_w - 2), Act::DjChoose(0)),
        (t!("gui.dj.pick").to_string(), super::sonic::wrap(&t!("gui.dj.pick_sub"), text_w - 2), Act::DjChoose(1)),
        (
            format!("{} {}", if chooser.remember { check_on } else { check_off }, t!("gui.dj.remember")),
            super::sonic::wrap(&t!("gui.dj.remember_sub"), text_w - 2),
            Act::DjRemember,
        ),
    ];
    // Title, subtitle, a gap, the rows (label + detail lines), trailing gap.
    let rows_h: usize = rows.iter().map(|(_, detail, _)| 1 + detail.len()).sum();
    let height = (1 + subtitle.len() + 1 + rows_h + 1) as u16 + 2;
    let inner = modal_frame_on(frame, &mut gui.ui, area, width, height, th().accent);
    put(frame, inner.x + 1, inner.y, &t!("gui.dj.start_title"), accent().add_modifier(Modifier::BOLD));
    modal_close(frame, &mut gui.ui, inner, Act::DjCancel, t!("gui.srv.close_tip").to_string());
    let mut y = inner.y + 1;
    for line in &subtitle {
        put(frame, inner.x + 1, y, line, dim());
        y += 1;
    }
    y += 1;
    for (i, (label, detail, act)) in rows.into_iter().enumerate() {
        let rect = Rect { x: inner.x, y, width: inner.width, height: 1 + detail.len() as u16 };
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
        let is_sel = chooser.row == i;
        if is_sel {
            let line = Rect { x: inner.x, y, width: inner.width, height: 1 };
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), line);
        }
        let style = match (is_sel, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        put(frame, inner.x + 1, y, &label, style);
        for line in &detail {
            y += 1;
            put(frame, inner.x + 3, y, line, dim());
        }
        y += 1;
        gui.ui.click(rect, act);
    }
}

/// The chooser's keys, and Esc on the opening-song road; `None` when
/// neither is up.
pub(crate) fn handle_key(
    gui: &mut Gui,
    key: ratatui::crossterm::event::KeyEvent,
) -> Option<bool> {
    use ratatui::crossterm::event::KeyCode;
    if gui.app.dj_chooser.is_some() {
        match key.code {
            KeyCode::Esc => gui.forward(Action::Cancel),
            KeyCode::Up => gui.forward(Action::Up),
            KeyCode::Down => gui.forward(Action::Down),
            KeyCode::Enter => gui.forward(Action::Activate),
            KeyCode::Char(' ') => gui.forward(Action::PlayPause),
            _ => {}
        }
        return Some(false);
    }
    if gui.app.capture == Some(Capture::DjSeed) && key.code == KeyCode::Esc {
        gui.forward(Action::Cancel);
        return Some(false);
    }
    None
}

/// The DJ's side of [`Gui::act`]. True when the act was one of ours.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    match act {
        Act::DjChoose(row) => {
            let remember = gui.app.dj_chooser.as_ref().is_some_and(|c| c.remember);
            let choice = if *row == 0 { EmptyQueueStart::Random } else { EmptyQueueStart::Pick };
            let effects = gui.app.answer_dj_start(choice, remember);
            gui.pend(effects);
        }
        Act::DjRemember => {
            if let Some(chooser) = gui.app.dj_chooser.as_mut() {
                chooser.remember = !chooser.remember;
            }
        }
        Act::DjCancel => gui.forward(Action::Cancel),
        // The empty queue's openers (clause 16).
        Act::DjSurprise => gui.forward(Action::DjSurprise),
        Act::DjPick => gui.forward(Action::DjPick),
        _ => return false,
    }
    true
}

/// The banner the "Let me choose" road wears (clause 4), when it is armed.
pub(crate) fn banner(gui: &Gui) -> Option<String> {
    (gui.app.capture == Some(Capture::DjSeed)).then(|| t!("gui.dj.pick_banner").to_string())
}

/// The empty queue panel's openers (clause 16): while the DJ is armed, the
/// hint and the two ways to give it an opening song, under the label.
pub(crate) fn draw_empty_queue(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, width: usize) {
    if !gui.app.dj_armed() {
        return;
    }
    let forward = if legacy_conhost() { ">" } else { "▸" };
    // The hint wraps in the narrow panel; the buttons follow it.
    let mut y = y + 1;
    for line in super::sonic::wrap(&t!("gui.queue.empty_dj_hint"), width).into_iter().take(3) {
        put(frame, x, y, &line, dim());
        y += 1;
    }
    let random = format!("{} {forward}", t!("gui.queue.empty_dj_random"));
    let choose = format!("{} {forward}", t!("gui.queue.empty_dj_choose"));
    super::torrent::text_button(frame, gui, x, y + 1, &random, true, Act::DjSurprise);
    super::torrent::text_button(frame, gui, x, y + 2, &choose, false, Act::DjPick);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Position;

    use super::super::{Act, Gui, render};
    use crate::config::Config;
    use crate::tui::app::{App, Capture, DjMark, Effect, Origin, Queued};
    use crate::tui::worker::ApiCmd;

    const HOST: &str = "http://host:3000";

    /// A connected session with a server to arm the DJ for.
    fn session_gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(Some(HOST.into()), None, None));
        gui.app.connected = true;
        gui.app.session.server = HOST.into();
        gui.app.session.server_id = HOST.into();
        gui
    }

    fn draw(gui: &mut Gui) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    fn opener_asked(gui: &Gui) -> bool {
        gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::AutoDj(request)) if request.ask.opener))
    }

    #[test]
    fn the_toggle_on_an_empty_queue_draws_the_chooser_and_a_click_answers_it() {
        let mut gui = session_gui();
        gui.act(Act::AutoDj);
        assert!(gui.app.dj_chooser.is_some(), "the opening question");
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Start Auto DJ with what?"), "{all}");
        assert!(all.contains("Surprise me") && all.contains("Let me choose") && all.contains("Remember this"));

        // Esc leaves the DJ off.
        super::handle_key(&mut gui, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(gui.app.dj_chooser.is_none() && !gui.app.dj_armed());

        // Back again; the remember box by click, then the first answer.
        gui.act(Act::AutoDj);
        draw(&mut gui);
        let (ry, _) = rows.iter().enumerate().find(|(_, r)| r.contains("Remember this")).map(|(y, r)| (y as u16, r.find("Remember").unwrap() as u16)).unwrap();
        let rx = rows[ry as usize].char_indices().position(|(i, _)| rows[ry as usize][i..].starts_with("Remember")).unwrap() as u16;
        assert_eq!(gui.ui.hit(Position { x: rx, y: ry }), Some(Act::DjRemember));
        gui.act(Act::DjRemember);
        assert!(gui.app.dj_chooser.as_ref().unwrap().remember);
        let sy = rows.iter().position(|r| r.contains("Surprise me")).unwrap() as u16;
        let sx = rows[sy as usize].char_indices().position(|(i, _)| rows[sy as usize][i..].starts_with("Surprise")).unwrap() as u16;
        assert_eq!(gui.ui.hit(Position { x: sx, y: sy }), Some(Act::DjChoose(0)));
        gui.act(Act::DjChoose(0));
        assert!(gui.app.dj_chooser.is_none());
        assert!(opener_asked(&gui), "the filtered opener goes out: {:?}", gui.pending);
        assert_eq!(gui.app.dj.empty_queue, crate::dj::EmptyQueueStart::Random, "remembered");
    }

    /// `cargo test dump_dj_chooser -- --ignored --nocapture` to eyeball the
    /// chooser and the armed empty queue.
    #[test]
    #[ignore]
    fn dump_dj_chooser() {
        let mut gui = session_gui();
        gui.act(Act::AutoDj);
        println!("{}", draw(&mut gui).join("\n"));
        gui.forward(crate::tui::app::Action::Cancel);
        gui.app.dj_server = Some(HOST.into());
        println!("{}", draw(&mut gui).join("\n"));
    }

    #[test]
    fn the_empty_queue_offers_the_openers_only_while_the_dj_is_armed() {
        let mut gui = session_gui();
        let all = draw(&mut gui).join("\n");
        assert!(!all.contains("Pick a random song"), "off: just the label");

        gui.app.dj_server = Some(HOST.into());
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Auto DJ is on and needs an") && all.contains("opening song."), "wrapped hint: {all}");
        assert!(all.contains("Pick a random song") && all.contains("Choose a song"));
        let y = rows.iter().position(|r| r.contains("Pick a random song")).unwrap() as u16;
        let x = rows[y as usize].char_indices().position(|(i, _)| rows[y as usize][i..].starts_with("Pick a random")).unwrap() as u16;
        assert_eq!(gui.ui.hit(Position { x, y }), Some(Act::DjSurprise));
        gui.act(Act::DjSurprise);
        assert!(opener_asked(&gui), "the button asks the opener of the armed DJ");
    }

    #[test]
    fn dj_picked_rows_wear_the_badge_and_the_road_wears_its_banner() {
        let mut gui = session_gui();
        let row = |path: &str, dj: Option<DjMark>| Queued {
            origin: Origin { server: HOST.into(), peer: None },
            dj,
            track: crate::api::types::Track { filepath: path.into(), metadata: Default::default() },
        };
        gui.app.queue.items = vec![
            row("mine.mp3", None),
            row("random.mp3", Some(DjMark { sonic: false })),
            row("sonic.mp3", Some(DjMark { sonic: true })),
        ];
        let rows = draw(&mut gui);
        let title_at = |name: &str| rows.iter().find(|r| r.contains(name)).unwrap().clone();
        assert!(title_at("mine.mp3").contains(" mine.mp3"), "no badge on the user's row");
        assert!(title_at("random.mp3").contains("∞ random.mp3"), "{}", title_at("random.mp3"));
        assert!(title_at("sonic.mp3").contains("≈ sonic.mp3"), "{}", title_at("sonic.mp3"));

        gui.app.capture = Some(Capture::DjSeed);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Pick the opening song"), "{all}");
    }
}
