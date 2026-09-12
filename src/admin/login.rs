//! The admin hub's sign-in page: `mstream-player admin` against a server
//! whose accounts exist, with no saved session for it — or one the server
//! no longer accepts. The hub's pre-flight ping answered 401, so this page
//! asks once; what the server issues is kept the way `mstream-player
//! login` keeps it (and the way the wizard keeps the account it creates),
//! and the room opens. Nothing here is room-specific: every room needs the
//! same admin session, and the rooms keep their own 403/405 gates.
//!
//! Same shape as a room — the kit's surface, the hub's loop, a worker for
//! the one blocking call: the server answers a wrong password after a
//! deliberate delay, and the page must keep drawing through it.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{Outcome, Screen, draw_bottom, draw_header, frame_ground, host_of};
use crate::api::types::LoginResponse;
use crate::api::{ApiError, Client};
use crate::config;
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, dim};

/// The header, the subtitle, two four-row fields, the tall button, the two
/// bottom lines — and the wizard's field width.
const MIN_W: u16 = 58;
const MIN_H: u16 = 20;
const FIELD_W: u16 = 44;

/// The two fields, in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    Username,
    Password,
}

#[derive(Clone, Debug)]
pub(crate) enum Act {
    Focus(Field),
    SignIn,
}

/// The worker's one answer.
type Done = Result<LoginResponse, ApiError>;

/// The page. `session()` after the loop says whether it signed in.
pub(crate) struct SignIn {
    client: Arc<Client>,
    username: Input,
    password: Input,
    field: Field,
    /// One line of status above the tips: (text, is_error).
    note: Option<(String, bool)>,
    busy: Option<String>,
    /// An attempt asked for by Enter or the button, sent on the next pump
    /// so the busy line is on screen before the call goes out.
    queued: bool,
    in_flight: Option<Receiver<Done>>,
    /// `(username, token)` once the server signed the page in.
    session: Option<(String, String)>,
    ui: Surface<Act>,
}

/// The page, empty: what the hub opens on a 401.
pub(super) fn start(client: Client) -> SignIn {
    SignIn::new(client)
}

/// Keep the session the way `mstream-player login` does: the server entry
/// with its username in config.toml, the token in credentials.toml. A file
/// that fails to LOAD is never overwritten with defaults — the house rule,
/// and for the credentials a real stake: defaulting them would drop every
/// other server's token and every pairing code.
pub(super) fn remember(server: &str, username: &str, token: &str) -> Result<(), String> {
    let mut cfg = config::load()?;
    config::touch_server(&mut cfg, server, Some(username.to_string()));
    let mut creds = config::load_credentials()?;
    config::store_token(&mut creds, server, Some(token.to_string()));
    config::save(&cfg)?;
    config::save_credentials(&creds)
}

impl SignIn {
    pub(super) fn new(client: Client) -> Self {
        SignIn {
            client: Arc::new(client),
            username: Input::default(),
            password: Input::default(),
            field: Field::Username,
            note: None,
            busy: None,
            queued: false,
            in_flight: None,
            session: None,
            ui: Surface::new(),
        }
    }

    pub(super) fn server(&self) -> String {
        self.client.server()
    }

    /// `(username, token)` once the server signed the page in.
    pub(super) fn session(&self) -> Option<(String, String)> {
        self.session.clone()
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::Focus(field) => self.field = field,
            Act::SignIn => self.submit(),
        }
        None
    }

    /// Enter or the button: the two empty-field gates, then the call —
    /// queued, never twice.
    fn submit(&mut self) {
        if self.queued || self.in_flight.is_some() {
            return;
        }
        if self.username.value().trim().is_empty() {
            self.field = Field::Username;
            self.note = Some((t!("admin.signin_need_username").to_string(), true));
            return;
        }
        if self.password.value().is_empty() {
            self.field = Field::Password;
            self.note = Some((t!("admin.signin_need_password").to_string(), true));
            return;
        }
        self.note = None;
        self.queued = true;
        self.busy = Some(t!("admin.signin_busy").to_string());
    }

    fn dispatch_queued(&mut self) {
        if !self.queued || self.in_flight.is_some() {
            return;
        }
        self.queued = false;
        let client = self.client.clone();
        let username = self.username.value().trim().to_string();
        let password = self.password.value().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(client.login_shared(&username, &password));
        });
        self.in_flight = Some(rx);
    }

    /// Fold the worker's answer back into the page.
    fn apply(&mut self, done: Done) {
        self.in_flight = None;
        self.busy = None;
        match done {
            Ok(resp) => {
                self.note = None;
                self.session = Some((self.username.value().trim().to_string(), resp.token));
            }
            Err(ApiError::Unauthorized) => {
                // The name stays and the password goes: the usual second
                // try is a retyped password.
                self.password = Input::default();
                self.field = Field::Password;
                self.note = Some((t!("admin.signin_rejected").to_string(), true));
            }
            Err(e) => {
                self.note = Some((format!("{}: {e}", t!("admin.signin_failed")), true));
            }
        }
    }
}

impl Screen for SignIn {
    type Act = Act;

    fn ui(&mut self) -> &mut Surface<Act> {
        &mut self.ui
    }

    fn pump(&mut self) {
        let answer = match &self.in_flight {
            Some(rx) => match rx.try_recv() {
                Ok(done) => Some(done),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err(ApiError::Network(t!("note.worker_gone").to_string())))
                }
            },
            None => None,
        };
        if let Some(done) = answer {
            self.apply(done);
        }
        self.dispatch_queued();
    }

    /// Signed in: the loop ends and the hub reads the session.
    fn finished(&self) -> Option<Outcome> {
        self.session.is_some().then_some(Outcome::Quit)
    }

    fn render(&mut self, frame: &mut Frame) {
        render(frame, self)
    }

    fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        match key.code {
            KeyCode::Esc => return Some(Outcome::Quit),
            KeyCode::Enter => return self.act(Act::SignIn),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Up => {
                self.field = match self.field {
                    Field::Username => Field::Password,
                    Field::Password => Field::Username,
                };
            }
            // The focused field's line editor takes everything else —
            // chars, Backspace, Delete, ←/→, Home/End, the ctrl ops.
            _ => {
                let field = match self.field {
                    Field::Username => &mut self.username,
                    Field::Password => &mut self.password,
                };
                field.handle_event(&TermEvent::Key(key));
            }
        }
        None
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        SignIn::act(self, act)
    }

    fn wheel(&mut self, _up: bool, _at: Position) {}
}

// ── Drawing ───────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, page: &mut SignIn) {
    page.ui.begin_frame();
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };
    let host = host_of(&page.client);
    draw_header(frame, area, &t!("admin.signin_title"), &host);

    // The wizard's column and field width, centered.
    let column_w = area.width.saturating_sub(4).min(74);
    let column_x = area.x + (area.width - column_w) / 2;
    let width = FIELD_W.min(column_w);
    let x = area.x + (area.width - width) / 2;
    let mut y = area.y + 3;
    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.signin_subtitle", host = host).to_string(), dim()))
            .wrap(Wrap { trim: true })
            .alignment(Alignment::Center),
        Rect { x: column_x, y, width: column_w, height: 2 },
    );
    y += 3;

    let focus = page.field;
    let show = |input: &Input, secret: bool, focused: bool| -> String {
        let value = if secret { mask(input.value()) } else { input.value().to_string() };
        if focused {
            kit::input_display(&value, input.cursor(), width.saturating_sub(2))
        } else {
            value
        }
    };
    let username = show(&page.username, false, focus == Field::Username);
    let password = show(&page.password, true, focus == Field::Password);
    y = field_row(frame, page, x, y, width, &t!("login.field_username"), username, Field::Username);
    y = field_row(frame, page, x, y, width, &t!("login.field_password"), password, Field::Password);
    y += 1;

    let enabled = !page.queued && page.in_flight.is_none();
    let label = t!("admin.signin_button").to_string();
    kit::tall_button(frame, &mut page.ui, Rect { x, y, width, height: 3 }, &label, enabled, Act::SignIn);

    draw_bottom(frame, area, page.note.as_ref(), page.busy.as_deref(), &t!("admin.hint_signin"));

    if let Some((target, text)) = page.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// A labelled three-row field in the wizard's Rounded card: accent when
/// focused, bright under the pointer, dim otherwise; the card is the
/// click target that focuses it. Returns the next free row.
#[allow(clippy::too_many_arguments)]
fn field_row(
    frame: &mut Frame,
    page: &mut SignIn,
    x: u16,
    y: u16,
    width: u16,
    label: &str,
    value: String,
    field: Field,
) -> u16 {
    frame.render_widget(
        Paragraph::new(Span::styled(label.to_string(), dim())),
        Rect { x, y, width, height: 1 },
    );
    let rect = Rect { x, y: y + 1, width, height: 3 };
    let focused = page.field == field;
    let hovered = page.ui.pointer.is_some_and(|p| rect.contains(p));
    let style = if hovered {
        Style::default().fg(th().bright)
    } else if focused {
        accent()
    } else {
        dim()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(style);
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(Span::raw(value)), inner);
    page.ui.click(rect, Act::Focus(field));
    y + 4
}

fn mask(secret: &str) -> String {
    "•".repeat(secret.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    /// English strings under assertion: hold the wizard tests' locale lock
    /// (one of them flips the process-global locale) and pin English.
    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    fn page() -> SignIn {
        SignIn::new(Client::new("http://home.mstream.example:3000").expect("client"))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(page: &mut SignIn, text: &str) {
        for c in text.chars() {
            page.key(key(KeyCode::Char(c)));
        }
    }

    fn note(page: &SignIn) -> String {
        page.note.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
    }

    /// Render one frame and flatten the buffer to text for assertions.
    fn draw(page: &mut SignIn) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, page)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn enter_asks_for_the_username_then_the_password_before_calling_anyone() {
        let _en = english();
        let mut p = page();
        p.key(key(KeyCode::Enter));
        assert_eq!(note(&p), "enter a username");
        assert!(!p.queued && p.in_flight.is_none(), "nothing was sent");
        type_text(&mut p, "alice");
        p.key(key(KeyCode::Enter));
        assert_eq!(note(&p), "enter a password");
        assert_eq!(p.field, Field::Password, "focus moves to the empty field");
        type_text(&mut p, "hunter2");
        p.key(key(KeyCode::Enter));
        assert!(p.queued, "a filled form queues the call");
        assert_eq!(p.busy.as_deref(), Some("signing in…"));
        assert!(note(&p).is_empty(), "the gate note clears once the form passes");
    }

    #[test]
    fn tab_switches_fields_and_the_password_draws_masked() {
        let _en = english();
        let mut p = page();
        type_text(&mut p, "alice");
        p.key(key(KeyCode::Tab));
        assert_eq!(p.field, Field::Password);
        type_text(&mut p, "s3cret");
        let frame = draw(&mut p);
        assert!(frame.contains("alice"), "{frame}");
        assert!(frame.contains("••••••"), "{frame}");
        assert!(!frame.contains("s3cret"), "the password never draws in clear:\n{frame}");
        assert!(frame.contains("Sign in"), "{frame}");
        assert!(frame.contains("home.mstream.example"), "the host is named:\n{frame}");
        assert!(frame.contains("USERNAME") && frame.contains("PASSWORD"), "{frame}");
        p.key(key(KeyCode::BackTab));
        assert_eq!(p.field, Field::Username);
    }

    #[test]
    fn a_rejected_sign_in_keeps_the_name_clears_the_password_and_says_so() {
        let _en = english();
        let mut p = page();
        type_text(&mut p, "alice");
        p.key(key(KeyCode::Tab));
        type_text(&mut p, "wrong");
        p.key(key(KeyCode::Enter));
        p.apply(Err(ApiError::Unauthorized));
        assert_eq!(p.username.value(), "alice");
        assert_eq!(p.password.value(), "");
        assert_eq!(p.field, Field::Password);
        assert!(note(&p).contains("check the username and password"), "{}", note(&p));
        assert!(p.session().is_none() && p.finished().is_none());
        assert!(p.busy.is_none(), "the busy line clears");
    }

    #[test]
    fn a_successful_sign_in_ends_the_page_with_the_session() {
        let _en = english();
        let mut p = page();
        type_text(&mut p, " alice ");
        p.key(key(KeyCode::Tab));
        type_text(&mut p, "hunter2");
        p.key(key(KeyCode::Enter));
        p.apply(Ok(LoginResponse { token: "tok".into(), vpaths: vec![] }));
        assert_eq!(p.session(), Some(("alice".to_string(), "tok".to_string())), "trimmed name");
        assert!(matches!(p.finished(), Some(Outcome::Quit)));
        assert!(p.busy.is_none() && note(&p).is_empty());
    }

    #[test]
    fn a_failure_that_is_not_a_refusal_is_reported_with_its_cause() {
        let _en = english();
        let mut p = page();
        p.apply(Err(ApiError::Network("connection refused".into())));
        let n = note(&p);
        assert!(n.starts_with("could not sign in") && n.contains("connection refused"), "{n}");
        assert!(p.session().is_none());
    }

    #[test]
    fn esc_leaves_without_a_session() {
        let mut p = page();
        assert!(matches!(p.key(key(KeyCode::Esc)), Some(Outcome::Quit)));
        assert!(p.session().is_none() && p.finished().is_none());
    }

    #[test]
    fn remember_keeps_the_token_and_the_server_entry() {
        let scratch = crate::config::testing::Scratch::new("admin-signin-remember");
        let _ = &scratch;
        remember("http://home.mstream.example:3000/", "alice", "tok").unwrap();
        let creds = config::load_credentials().unwrap();
        assert_eq!(
            config::token_for(&creds, "http://home.mstream.example:3000"),
            Some("tok".to_string()),
            "found under the slash-insensitive key the rooms resolve with"
        );
        let cfg = config::load().unwrap();
        assert_eq!(cfg.servers[0].username.as_deref(), Some("alice"));
    }
}
