//! Drive the TUI from a script, so a whole flow can be walked without a human
//! at the keyboard.
//!
//! Keys go through exactly the path a real key press takes —
//! `map_key(key, app.input_mode())` then `App::handle_action` then
//! `ui::render` — so this exercises the real state machine and the real
//! drawing code, not a parallel imitation of them. Anything it proves, the
//! app does.
//!
//! Two modes:
//!   * **offline** (default) — no server, no threads. Worker replies are
//!     supplied by `@event` steps, so a run is deterministic and belongs in
//!     CI.
//!   * **`--live`** — spawns the real audio and API workers and talks to a
//!     real server, so genuine replies (and genuine surprises) come back.

use std::time::{Duration, Instant};

use clap::Args;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::api::types::Ping;
use crate::discovery::DiscoveredServer;
use crate::tui::app::{App, Effect, map_key};
use crate::tui::ui;
use crate::tui::worker::{self, Event};

#[derive(Args)]
pub struct ReplayArgs {
    /// Comma-separated steps. Keys by name (Down, Enter, Esc, Tab,
    /// Backspace, Space, PageDown, ctrl+c), single characters (q, a, 2),
    /// quoted text to type ('hunter2'), `@event` to inject a worker reply,
    /// `wait:500` to pause, and `frame` to print the screen.
    pub script: String,

    /// Talk to a real server with real workers instead of injected replies.
    #[arg(long)]
    pub live: bool,

    /// Print the screen after every step, not just at the end.
    #[arg(long)]
    pub frames: bool,

    #[arg(long, default_value_t = 100)]
    pub width: u16,

    #[arg(long, default_value_t = 32)]
    pub height: u16,

    /// How long to wait for worker replies after each step, in live mode.
    #[arg(long, default_value_t = 400)]
    pub wait_ms: u64,

    #[command(flatten)]
    pub conn: crate::cmd_library::ConnArgs,
}

enum Step {
    Key(KeyEvent),
    Inject(Event),
    Wait(Duration),
    Frame,
}

fn key(code: KeyCode) -> Step {
    Step::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// Sample servers for `@servers`, shaped like a real advert: one that can be
/// paired with, one that can't.
fn sample_servers() -> Vec<DiscoveredServer> {
    vec![
        DiscoveredServer {
            name: "Living Room".into(),
            base_url: "http://192.168.1.71:3999".into(),
            version: Some("6.19.2".into()),
            quick_connect: true,
        },
        DiscoveredServer {
            name: "Attic".into(),
            base_url: "http://192.168.1.9:3000".into(),
            version: None,
            quick_connect: false,
        },
    ]
}

fn parse_step(raw: &str, app_server: &str) -> Result<Step, String> {
    let token = raw.trim();
    if token.is_empty() {
        return Err("empty step".to_string());
    }

    // 'quoted text' types each character.
    if let Some(text) = token
        .strip_prefix('\'')
        .and_then(|t| t.strip_suffix('\''))
        .or_else(|| token.strip_prefix('"').and_then(|t| t.strip_suffix('"')))
    {
        return Ok(Step::Key(KeyEvent::new(
            KeyCode::Char(text.chars().next().unwrap_or(' ')),
            KeyModifiers::NONE,
        )));
    }

    if let Some(ms) = token.strip_prefix("wait:") {
        let ms: u64 = ms.parse().map_err(|_| format!("bad wait '{token}'"))?;
        return Ok(Step::Wait(Duration::from_millis(ms)));
    }

    if let Some(name) = token.strip_prefix('@') {
        let (name, arg) = match name.split_once(':') {
            Some((name, arg)) => (name, Some(arg.to_string())),
            None => (name, None),
        };
        let event = match name {
            "servers" => Event::ServersDiscovered(sample_servers()),
            "no-servers" => Event::ServersDiscovered(Vec::new()),
            "needs-login" => Event::NeedsLogin {
                server: arg.unwrap_or_else(|| app_server.to_string()),
            },
            "connected" => Event::Connected {
                server: arg.unwrap_or_else(|| app_server.to_string()),
                username: Some("tester".into()),
                token: Some("token".into()),
                ping: Box::new(Ping { vpaths: vec!["testlib".into()], ..Default::default() }),
            },
            "unauthorized" => Event::Unauthorized,
            "tunnel" => Event::TunnelReady {
                local_url: arg.unwrap_or_else(|| "http://127.0.0.1:7000".to_string()),
            },
            "error" => Event::Error(arg.unwrap_or_else(|| "something went wrong".into())),
            other => return Err(format!("unknown event '@{other}'")),
        };
        return Ok(Step::Inject(event));
    }

    if token.eq_ignore_ascii_case("frame") {
        return Ok(Step::Frame);
    }

    if let Some(rest) = token.strip_prefix("ctrl+") {
        let c = rest.chars().next().ok_or_else(|| format!("bad key '{token}'"))?;
        return Ok(Step::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)));
    }

    let step = match token.to_ascii_lowercase().as_str() {
        "up" => key(KeyCode::Up),
        "down" => key(KeyCode::Down),
        "left" => key(KeyCode::Left),
        "right" => key(KeyCode::Right),
        "enter" => key(KeyCode::Enter),
        "esc" => key(KeyCode::Esc),
        "tab" => key(KeyCode::Tab),
        "backspace" => key(KeyCode::Backspace),
        "space" => key(KeyCode::Char(' ')),
        "home" => key(KeyCode::Home),
        "end" => key(KeyCode::End),
        "pageup" => key(KeyCode::PageUp),
        "pagedown" => key(KeyCode::PageDown),
        _ => {
            let mut chars = token.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => key(KeyCode::Char(c)),
                _ => return Err(format!("unknown step '{token}'")),
            }
        }
    };
    Ok(step)
}

/// Split a script on commas, keeping quoted runs intact.
fn split_script(script: &str) -> Vec<String> {
    let mut steps = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in script.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                current.push(c);
            }
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                current.push(c);
            }
            (None, ',') => {
                steps.push(std::mem::take(&mut current));
            }
            (None, c) => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        steps.push(current);
    }
    steps.into_iter().filter(|s| !s.trim().is_empty()).collect()
}

/// Expand quoted text into one key step per character.
fn expand(raw: &str, app_server: &str) -> Result<Vec<Step>, String> {
    let token = raw.trim();
    let quoted = token
        .strip_prefix('\'')
        .and_then(|t| t.strip_suffix('\''))
        .or_else(|| token.strip_prefix('"').and_then(|t| t.strip_suffix('"')));
    match quoted {
        Some(text) => Ok(text
            .chars()
            .map(|c| Step::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)))
            .collect()),
        None => Ok(vec![parse_step(token, app_server)?]),
    }
}

fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        let mut row = String::new();
        for x in 0..buffer.area.width {
            row.push_str(buffer[(x, y)].symbol());
        }
        out.push_str(row.trim_end());
        out.push('\n');
    }
    // Collapse the blank padding a centred splash leaves behind.
    out.trim_matches('\n').to_string() + "\n"
}

pub fn run(args: ReplayArgs) -> i32 {
    let mut terminal = match Terminal::new(TestBackend::new(args.width, args.height)) {
        Ok(terminal) => terminal,
        Err(e) => {
            eprintln!("error: could not create a test terminal: {e}");
            return 1;
        }
    };

    // Live mode starts from the same place the real binary would.
    let (server, token) = if args.live {
        (args.conn.server.clone(), args.conn.token.clone())
    } else {
        (None, None)
    };
    let mut app = App::new(server, token, None);
    let mut pending = if args.live { app.start() } else { Vec::new() };

    let live = args.live.then(|| {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let audio_tx = worker::spawn_audio(event_tx.clone());
        let api_tx = worker::spawn_api(event_tx.clone());
        (event_tx, event_rx, audio_tx, api_tx)
    });

    let raw_steps = split_script(&args.script);
    let mut steps: Vec<(String, Step)> = Vec::new();
    for raw in &raw_steps {
        match expand(raw, &app.connect.server) {
            Ok(expanded) => {
                for step in expanded {
                    steps.push((raw.trim().to_string(), step));
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
    }

    let settle = |app: &mut App, pending: &mut Vec<Effect>| {
        let Some((event_tx, event_rx, audio_tx, api_tx)) = live.as_ref() else {
            return;
        };
        // Wait only when a worker owes us an answer, and wait for *every*
        // outstanding one. Counting matters: a discovery browse and a tunnel
        // dial can be in flight together, and stopping at the first reply
        // would let later steps run while the second is still connecting.
        let mut outstanding = 0usize;
        let mut deadline = Instant::now();
        loop {
            let asked = pending
                .iter()
                .filter(|effect| matches!(effect, Effect::Api(_) | Effect::Discover))
                .count();
            if asked > 0 {
                outstanding += asked;
                deadline = Instant::now() + Duration::from_millis(args.wait_ms);
            }
            crate::tui::dispatch(app, pending, audio_tx, api_tx, event_tx);

            let event = if outstanding > 0 {
                deadline
                    .checked_duration_since(Instant::now())
                    .and_then(|remaining| event_rx.recv_timeout(remaining).ok())
            } else {
                event_rx.try_recv().ok()
            };
            let Some(event) = event else { break };

            // Status ticks arrive every hundred milliseconds and say nothing
            // about a flow, so they are neither reported nor counted as an
            // answer.
            if !matches!(event, Event::Status(_)) {
                outstanding = outstanding.saturating_sub(1);
                println!("   ← {event:?}");
            }
            let more = app.apply_event(event);
            pending.extend(more);
        }
    };

    for (index, (label, step)) in steps.iter().enumerate() {
        match step {
            Step::Key(key) => {
                println!("── {}. {label}", index + 1);
                match map_key(*key, app.input_mode()) {
                    Some(action) => {
                        let effects = app.handle_action(action.clone());
                        report(&effects);
                        pending.extend(effects);
                    }
                    None => println!("   (key ignored in this mode)"),
                }
            }
            Step::Inject(_) => {
                println!("── {}. {label}", index + 1);
                // Rebuilt so `@needs-login` picks up the server as it is now.
                let event = match parse_step(label, &app.connect.server) {
                    Ok(Step::Inject(event)) => event,
                    _ => unreachable!("re-parsed an injected event"),
                };
                let effects = app.apply_event(event);
                report(&effects);
                pending.extend(effects);
            }
            Step::Wait(duration) => {
                println!("── {}. {label}", index + 1);
                std::thread::sleep(*duration);
            }
            Step::Frame => {}
        }

        settle(&mut app, &mut pending);

        if let Err(e) = terminal.draw(|frame| ui::render(frame, &mut app)) {
            eprintln!("error: render failed: {e}");
            return 1;
        }
        if args.frames || matches!(step, Step::Frame) {
            println!("{}", buffer_text(&terminal));
        }
        if app.should_quit {
            println!("(app quit)");
            break;
        }
    }

    if !args.frames {
        println!("{}", buffer_text(&terminal));
    }
    0
}

fn report(effects: &[Effect]) {
    for effect in effects {
        println!("   → {effect:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_scripts_keeping_quoted_text_together() {
        let steps = split_script("Down, Enter, 'a,b', q");
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[2].trim(), "'a,b'");
    }

    #[test]
    fn expands_quoted_text_into_one_key_per_character() {
        let steps = expand("'abc'", "").unwrap();
        assert_eq!(steps.len(), 3);
    }

    #[test]
    fn parses_the_key_vocabulary() {
        for name in ["Down", "enter", "Esc", "Tab", "PageDown", "space", "q", "2"] {
            assert!(parse_step(name, "").is_ok(), "failed on {name}");
        }
        assert!(matches!(parse_step("ctrl+c", ""), Ok(Step::Key(k)) if k.modifiers == KeyModifiers::CONTROL));
        assert!(parse_step("nonsense", "").is_err());
        assert!(parse_step("@nope", "").is_err());
    }

    #[test]
    fn needs_login_defaults_to_the_server_in_hand() {
        let step = parse_step("@needs-login", "http://host:3999").unwrap();
        match step {
            Step::Inject(Event::NeedsLogin { server }) => {
                assert_eq!(server, "http://host:3999");
            }
            _ => panic!("expected a NeedsLogin injection"),
        }
    }
}
