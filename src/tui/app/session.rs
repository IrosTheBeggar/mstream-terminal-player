//! The way in: the connect screen, and the [`Session`] it produces.
//!
//! Three paths lead here — a saved session reconnecting, an address
//! typed into the form, a pairing code dialled into a tunnel — and each
//! was maintaining five parallel identity fields on App by hand
//! (audit #56). [`Session`] is those five as one value, and the flow
//! that writes them lives next to it.

use crate::discovery::DiscoveredServer;

use super::{Action, App, Effect, Tab};
use crate::tui::worker::{ApiCmd, AudioCmd, Event};

/// Which server this session is talking to, as one value.
///
/// These five travelled as parallel fields on [`App`] — written together by
/// three different connect paths, read separately everywhere else, and every
/// fix had to remember all of them (audit #56).
#[derive(Debug, Default)]
pub struct Session {
    /// Where this session's requests and stream URLs go. For a Quick Connect
    /// session that is the loopback bridge, which lives and dies with the
    /// process — see [`Session::server_id`] for the part worth remembering.
    pub server: String,
    /// What the current server is remembered as: the same URL for a direct
    /// connection, a `mstream+iroh://` identity for a tunnel.
    pub server_id: String,
    /// Pairing code for the tunnel this session is using, if any. Held so it
    /// can be saved alongside the session — without it, a remembered tunnel
    /// server cannot be reached again.
    pub tunnel_code: Option<String>,
    pub token: Option<String>,
    pub username: Option<String>,
    /// Trust this server's own TLS certificate. Seated from the saved
    /// entry's flag; every client the workers build for this session
    /// carries it.
    pub self_signed: bool,
}

/// Which step of the connect screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectStage {
    /// Pick how to reach the server.
    #[default]
    Choosing,
    /// Server address plus credentials.
    Direct,
    /// Paste a pairing code and reach the server over its Iroh tunnel.
    QuickConnect,
}

/// The two ways in, in menu order.
pub const CONNECT_METHODS: [(&str, &str); 2] = [
    ("Direct", "server address on your network"),
    ("Quick Connect", "pairing code — works from anywhere"),
];

/// The connect screen, shown when there is no usable session.
#[derive(Debug, Default)]
pub struct ConnectForm {
    pub stage: ConnectStage,
    pub choice: usize,
    pub server: String,
    pub username: String,
    pub password: String,
    pub code: String,
    pub field: usize,
    pub submitting: bool,
    /// Servers found on the network, for the Quick Connect screen.
    pub found: Vec<DiscoveredServer>,
    pub searching: bool,
    /// Row selected on the Quick Connect screen: an index into `found`, or
    /// `found.len()` for the paste-a-code row, which is always last.
    pub row: usize,
    /// The server URL whose plaintext warning has been acknowledged. Held as
    /// the URL rather than a flag so that editing the address asks again
    /// instead of carrying consent over to a different host.
    pub insecure_ack: Option<String>,
}

impl ConnectForm {
    /// The paste-a-code row sits after any discovered servers.
    pub fn paste_row(&self) -> usize {
        self.found.len()
    }

    pub fn on_paste_row(&self) -> bool {
        self.row >= self.paste_row()
    }
}

impl ConnectForm {
    pub const FIELDS: usize = 3;

    fn value_mut(&mut self) -> &mut String {
        match self.field {
            0 => &mut self.server,
            1 => &mut self.username,
            _ => &mut self.password,
        }
    }

    fn next_field(&mut self) {
        self.field = (self.field + 1) % Self::FIELDS;
    }
}

impl App {
    pub(super) fn begin(&mut self) -> Vec<Effect> {
        // A tunnel server has no address to connect to until its code is
        // dialled, so reconnecting means opening the tunnel again first.
        if crate::quickconnect::is_tunnel_id(&self.session.server_id) {
            let Some(code) = self.session.tunnel_code.clone() else {
                // Remembered, but the code that reaches it is gone — deleted
                // credentials, or a config copied without them.
                self.session.server_id.clear();
                self.error(
                    "the pairing code for the last server is gone — paste it again to reconnect",
                );
                return Vec::new();
            };
            self.connecting = true;
            return vec![Effect::Api(ApiCmd::QuickConnect { code, token: self.session.token.clone() })];
        }
        if self.session.server.is_empty() {
            return Vec::new(); // connect form is showing
        }
        self.connecting = true;
        vec![Effect::Api(ApiCmd::Connect {
            server: self.session.server.clone(),
            token: self.session.token.clone(),
            self_signed: self.session.self_signed,
        })]
    }

    /// Point the session at another saved server and reconnect — the GUI's
    /// server switch. The same door as [`App::begin`], with the teardown a
    /// mid-session change needs first.
    ///
    /// What is already streaming keeps playing: its URL was resolved when it
    /// started and the engine holds it. The REST of the queue cannot come
    /// along — queued tracks are filepaths resolved against the session's
    /// server at play time ([`App::play_index`]), so on another server they
    /// would be wrong songs or dead URLs. Clearing them is the honest move,
    /// and the caller says so in a note.
    pub(crate) fn adopt_server(
        &mut self,
        server: String,
        server_id: String,
        username: Option<String>,
        token: Option<String>,
        tunnel_code: Option<String>,
        self_signed: bool,
        last_path: Option<String>,
    ) -> Vec<Effect> {
        self.connected = false;
        self.connect = ConnectForm::default();
        self.session = Session {
            server,
            server_id,
            tunnel_code,
            token,
            username,
            self_signed,
        };
        self.shed_server_state();
        self.path = last_path.unwrap_or_default();
        self.begin()
    }

    /// Drop everything that belonged to the server being left: the queue
    /// (its filepaths resolve against the session's server at play time),
    /// the announced next, and the search. What is already streaming keeps
    /// playing — its URL was resolved when it started. Shared by
    /// [`App::adopt_server`] and the GUI's pairing-code dial, which can
    /// only shed once the new tunnel has actually answered.
    pub(crate) fn shed_server_state(&mut self) {
        self.queue.items.clear();
        self.queue.current = None;
        self.queue.state.select(None);
        // The Connected handler rebuilds capabilities, libraries and panes.
        self.announced = None;
        self.search_hits = None;
        self.query.clear();
        self.search.set(Vec::new());
        self.files.set(Vec::new());
        self.files.loading = true;
    }

    pub(super) fn handle_connect_action(&mut self, action: Action) -> Vec<Effect> {
        if action == Action::Quit {
            self.should_quit = true;
            return vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)];
        }

        // A connection attempt takes seconds. Keys pressed in the meantime
        // must not edit the credentials or code being used, nor fire a second
        // attempt — a few stray characters appended to a pairing code turn it
        // into unreadable base64. Esc still abandons the attempt.
        if self.connect.submitting {
            return match action {
                Action::Cancel => {
                    self.connect.submitting = false;
                    self.connecting = false;
                    self.connect.stage = ConnectStage::Choosing;
                    self.message = None;
                    Vec::new()
                }
                _ => Vec::new(),
            };
        }

        match self.connect.stage {
            ConnectStage::Choosing => match action {
                Action::Up => {
                    self.connect.choice = self.connect.choice.saturating_sub(1);
                    Vec::new()
                }
                Action::Down | Action::CycleFocus => {
                    self.connect.choice = (self.connect.choice + 1).min(CONNECT_METHODS.len() - 1);
                    Vec::new()
                }
                Action::Submit | Action::Activate => {
                    self.message = None;
                    if self.connect.choice == 0 {
                        self.connect.stage = ConnectStage::Direct;
                        return Vec::new();
                    }
                    self.connect.stage = ConnectStage::QuickConnect;
                    self.connect.searching = true;
                    self.connect.found.clear();
                    self.connect.row = 0;
                    vec![Effect::Discover]
                }
                _ => Vec::new(),
            },

            ConnectStage::Direct => match action {
                Action::Input(c) => {
                    self.connect.value_mut().push(c);
                    Vec::new()
                }
                Action::Backspace => {
                    self.connect.value_mut().pop();
                    Vec::new()
                }
                Action::CycleFocus | Action::Down => {
                    self.connect.next_field();
                    Vec::new()
                }
                Action::Up => {
                    self.connect.field =
                        (self.connect.field + ConnectForm::FIELDS - 1) % ConnectForm::FIELDS;
                    Vec::new()
                }
                Action::Cancel => {
                    self.connect.stage = ConnectStage::Choosing;
                    self.message = None;
                    Vec::new()
                }
                Action::Submit => self.submit_connect(),
                _ => Vec::new(),
            },

            ConnectStage::QuickConnect => match action {
                Action::Up => {
                    self.connect.row = self.connect.row.saturating_sub(1);
                    Vec::new()
                }
                Action::Down => {
                    self.connect.row = (self.connect.row + 1).min(self.connect.paste_row());
                    Vec::new()
                }
                Action::Input(c) => {
                    // Typing anywhere means "I have a code", so jump to it.
                    self.connect.row = self.connect.paste_row();
                    self.connect.code.push(c);
                    Vec::new()
                }
                Action::Backspace => {
                    self.connect.code.pop();
                    Vec::new()
                }
                Action::Cancel => {
                    self.connect.stage = ConnectStage::Choosing;
                    self.message = None;
                    Vec::new()
                }
                Action::Submit => {
                    // A server found on this network is reachable directly —
                    // no tunnel needed, and no code to paste.
                    if let Some(server) = self.connect.found.get(self.connect.row).cloned() {
                        self.connecting = true;
                        self.connect.submitting = true;
                        self.connect.server = server.base_url.clone();
                        self.info(format!("connecting to {}…", server.name));
                        return vec![Effect::Api(ApiCmd::Connect {
                            server: server.base_url,
                            token: None,
                            self_signed: false,
                        })];
                    }
                    self.submit_quick_connect()
                }
                _ => Vec::new(),
            },
        }
    }

    fn submit_quick_connect(&mut self) -> Vec<Effect> {
        let code = self.connect.code.trim().to_string();
        if code.is_empty() {
            self.error("paste a pairing code first");
            return Vec::new();
        }
        self.connecting = true;
        self.connect.submitting = true;
        // Kept from here on: it is the only way back to this server, and
        // nothing is written until the connection actually succeeds.
        self.session.tunnel_code = Some(code.clone());
        self.info("dialling the tunnel — this can take a few seconds…");
        vec![Effect::Api(ApiCmd::QuickConnect { code, token: self.session.token.clone() })]
    }

    fn submit_connect(&mut self) -> Vec<Effect> {
        // Everything that can be settled without the network is settled here:
        // a round trip to learn that an address was mistyped is a slow way to
        // be told something we already know.
        let server = match crate::api::server_url::normalize(&self.connect.server) {
            Ok(server) => server,
            Err(message) => {
                self.error(message);
                return Vec::new();
            }
        };
        // Show what was filled in. "nas:3000" becoming "http://nas:3000" is
        // exactly what someone needs to see when it doesn't connect.
        self.connect.server = server.clone();

        let username = self.connect.username.trim().to_string();

        // No username means the server is expected to be in public mode, where
        // every request authenticates anyway.
        if username.is_empty() {
            self.connecting = true;
            self.connect.submitting = true;
            self.message = None;
            self.session.server = server.clone();
            return vec![Effect::Api(ApiCmd::Connect {
                server,
                token: None,
                self_signed: self.session.self_signed,
            })];
        }

        if self.connect.password.is_empty() {
            self.error("enter a password, or clear the username for a public server");
            return Vec::new();
        }

        // Plain http past the local network puts the password on the wire in
        // the clear. Say so once, and let the answer be yes.
        if crate::api::server_url::crosses_the_internet_unencrypted(&server)
            && self.connect.insecure_ack.as_deref() != Some(server.as_str())
        {
            self.connect.insecure_ack = Some(server.clone());
            self.error(format!(
                "{server} is plain http — your password would cross the internet \
                 unencrypted. Enter again to send it anyway."
            ));
            return Vec::new();
        }

        self.connecting = true;
        self.connect.submitting = true;
        self.message = None;
        vec![Effect::Api(ApiCmd::Login {
            server,
            username,
            password: std::mem::take(&mut self.connect.password),
            self_signed: self.session.self_signed,
        })]
    }

    /// Every reply about who we are connected to, and how.
    ///
    /// One door, like the DJ panel's (audit #57), for the same reason:
    /// these five all land on the connect screen this module owns, and
    /// they all decide the same three flags -- connected, connecting, and
    /// whether the form is still submitting. Reading them together is how
    /// you can see that they agree.
    pub(super) fn consume_session(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Connected { server, id, username, token, ping } => {
                // A fresh session starts with no verdict on how its tunnel
                // runs; the sampler speaks within a couple of seconds when
                // there is one, and a direct server never sets it at all.
                self.tunnel_path = None;
                self.connected = true;
                self.connecting = false;
                self.connect.submitting = false;
                self.session.server = server;
                self.session.server_id = id;
                if token.is_some() {
                    self.session.token = token;
                }
                if username.is_some() {
                    self.session.username = username;
                }
                self.capabilities = crate::api::types::Capabilities::from(ping.as_ref());
                // The Auto-DJ rows and the Sonic Path tab both turn on what
                // the ping just said; a reconnect can be a different server.
                self.dj_panel.rebuild(self.capabilities);
                self.reset_sonic_path();
                // A tab this server cannot serve is off the strip, so being
                // left standing on one is being on a tab with no number.
                if !self.tab.available(self.capabilities) {
                    self.tab = Tab::Files;
                }
                self.libraries = ping.vpaths.clone();
                // Cover filenames only mean anything to the server that
                // minted them; a reconnect may be a different server.
                self.art.clear();
                let libraries = ping.vpaths.len();
                self.info(format!(
                    "connected to {} ({} librar{})",
                    self.server_display(),
                    libraries,
                    if libraries == 1 { "y" } else { "ies" }
                ));

                // A remembered mode can outlive the server that supported it —
                // preferences are global, capabilities are per-server. Say so
                // rather than leaving a mode selected that quietly does
                // something else.
                if !self.autodj.available(self.capabilities) {
                    self.autodj = self.autodj.next_available(self.capabilities);
                    self.info(format!(
                        "this server has no similarity index — auto-dj is on {}",
                        self.autodj.label()
                    ));
                }

                // Opening the browser again: whatever this browse comes back
                // with is where we are, since neither `~` nor a remembered
                // path is a promise about how the server will spell it.
                self.opening = true;
                let mut effects = vec![
                    Effect::Api(ApiCmd::Browse(self.opening_path())),
                    Effect::Audio(AudioCmd::SetVolume(self.volume)),
                    Effect::Audio(AudioCmd::SetCrossfade(self.crossfade)),
                    Effect::Audio(AudioCmd::SetGapless(self.gapless)),
                    Effect::Audio(AudioCmd::SetBlendSkips(self.blend_skips)),
                    Effect::Audio(AudioCmd::SetPauseFade(self.pause_fade)),
                ];
                // Worth persisting when we hold a token we logged in for — or
                // a pairing code, which is the only way back to this server
                // even when it needs no login at all.
                let signed_in = self.session.token.is_some() && self.session.username.is_some();
                if signed_in || self.session.tunnel_code.is_some() {
                    effects.push(Effect::SaveSession);
                }
                effects
            }
            Event::ServersDiscovered(found) => {
                // Results can land after the user has already made a choice.
                // Row 0 means "the paste row" while the list is empty and
                // "the first server" once it isn't, so without this the
                // cursor silently retargets and Enter connects somewhere the
                // user never picked.
                let entered_a_code = !self.connect.code.trim().is_empty();
                self.connect.searching = false;
                self.connect.found = found;
                self.connect.row = if entered_a_code {
                    // Someone mid-paste keeps their place; otherwise the
                    // cursor lands on the first server, which is what a user
                    // who simply waited expects.
                    self.connect.paste_row()
                } else {
                    self.connect.row.min(self.connect.paste_row())
                };
                Vec::new()
            }
            Event::TunnelReady { local_url, id } => {
                self.connecting = false;
                self.connect.submitting = false;
                // The form carries the loopback address, which is a real,
                // working endpoint for the sign-in about to happen; the
                // identity is what the session will be filed under.
                self.connect.server = local_url;
                self.session.server_id = id;
                self.connect.stage = ConnectStage::Direct;
                self.connect.field = 1; // straight to the username
                self.info("tunnel open — sign in to continue");
                Vec::new()
            }
            Event::NeedsLogin { server } => {
                // A reply from a connection attempt that has been overtaken —
                // we already reached somewhere else. Applying it would drag a
                // connected session back to a login form.
                if self.connected {
                    return Vec::new();
                }
                self.connecting = false;
                self.connect.submitting = false;
                self.connect.server = server;
                self.connect.stage = ConnectStage::Direct;
                self.connect.field = 1; // straight to the username
                self.info("this server needs a sign-in");
                Vec::new()
            }
            Event::Unauthorized => {
                // An established session went bad. Offer the login form for
                // the server we were already using rather than dumping the
                // user back at "how do you want to connect?".
                self.connected = false;
                self.connecting = false;
                self.connect.submitting = false;
                if !self.session.server.is_empty() {
                    self.connect.server = self.session.server.clone();
                }
                self.connect.stage = ConnectStage::Direct;
                self.connect.field = 1;
                self.session.token = None;
                self.error("session expired — sign in again");
                Vec::new()
            }
            // The caller matches exactly the five arms above.
            _ => Vec::new(),
        }
    }
}
