//! The way in: the [`Session`], and the flow that seats one.
//!
//! Three paths lead here — a saved session reconnecting, an address a
//! shell's form sends, a pairing code dialled into a tunnel — and each was
//! maintaining five parallel identity fields on App by hand (audit #56).
//! [`Session`] is those five as one value, and the flow that writes them
//! lives next to it. The forms themselves are the shell's (`gui::servers`).

use super::{App, Effect, Tab};
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
    /// A federated peer session: the parent's identity, and the peer's
    /// row id on it. `server` and `token` are then the parent's, and
    /// `server_id` the peer's own identity (contract clauses 26–27).
    pub peer: Option<(String, i64)>,
}

impl App {
    pub(super) fn begin(&mut self) -> Vec<Effect> {
        // A tunnel server — or a peer reached through one — has no address
        // until its tunnel is up: connect through the bridge when it is,
        // open it first when it is not. The worker keeps a tunnel open for
        // as long as it is asked to, whichever session is current
        // (contract clause 38).
        // A peer whose own tunnel is up is browsed through it — plain
        // paths, the guest token — rather than through the parent's
        // proxies (contract clause 27).
        if let Some(effects) = self.connect_direct() {
            return effects;
        }
        let transport = self.session_transport().to_string();
        if crate::quickconnect::is_tunnel_id(&transport) {
            let credential = self.session.tunnel_code.clone().or_else(|| {
                self.servers
                    .iter()
                    .find(|s| crate::config::same_server(&s.id, &transport))
                    .and_then(|s| s.pairing.clone())
            });
            let Some(credential) = credential else {
                // Remembered, but the code that reaches it is gone — deleted
                // credentials, or a config copied without them.
                self.session.server_id.clear();
                self.error(
                    "the pairing code for the last server is gone — paste it again to reconnect",
                );
                return Vec::new();
            };
            self.connecting = true;
            return self.connect_through(&transport, credential);
        }
        if self.session.server.is_empty() {
            return Vec::new(); // connect form is showing
        }
        self.connecting = true;
        vec![Effect::Api(ApiCmd::Connect {
            server: self.session.server.clone(),
            identity: self.identity_or_server(),
            token: self.session.token.clone(),
            self_signed: self.session.self_signed,
            peer: self.session.peer.as_ref().map(|(_, id)| *id),
            local_token: None,
        })]
    }

    /// Try the session's server again — the GUI's "Try again" on a server
    /// that would not answer (multi-server contract, clause 13: the
    /// selection stood, the server is the thing that failed, so nothing is
    /// re-adopted). A no-op while a session is up or a connect is out.
    pub(crate) fn reconnect(&mut self) -> Vec<Effect> {
        if self.connected || self.connecting {
            return Vec::new();
        }
        self.message = None;
        self.begin()
    }

    /// The Connect for a peer session whose own tunnel is up and whose
    /// guest ticket is in hand; `None` when it rides the parent instead.
    fn connect_direct(&mut self) -> Option<Vec<Effect>> {
        self.session.peer.as_ref()?;
        let pid = self.session.server_id.clone();
        let (local_url, local_token) = match self.tunnels.get(&pid) {
            Some(super::TunnelState::Up { local_url, local_token, .. }) => {
                (local_url.clone(), local_token.clone())
            }
            _ => return None,
        };
        let guest = self.direct_ticket(&pid)?.guest_token.clone();
        self.connecting = true;
        self.session.server = local_url.clone();
        Some(vec![Effect::Api(ApiCmd::Connect {
            server: local_url,
            identity: pid,
            token: Some(guest),
            self_signed: false,
            peer: None,
            local_token: Some(local_token),
        })])
    }

    /// The session moving onto the browsed peer's own tunnel: the same
    /// identity, a new address and token, nothing else (clause 27).
    fn retarget_to_direct(&self, pid: &str) -> Option<Effect> {
        let Some(super::TunnelState::Up { local_url, local_token, .. }) = self.tunnels.get(pid) else {
            return None;
        };
        let guest = self.direct_ticket(pid)?.guest_token.clone();
        Some(Effect::Api(ApiCmd::Retarget {
            identity: pid.to_string(),
            server: local_url.clone(),
            token: Some(guest),
            self_signed: false,
            peer: None,
            local_token: Some(local_token.clone()),
        }))
    }

    /// The session moving back onto the parent's proxies, however the
    /// parent is reached.
    fn retarget_to_proxy(&self) -> Option<Effect> {
        let (parent, id) = self.session.peer.clone()?;
        let reach = self.reach(&super::Origin { server: parent, peer: None }).ok()?;
        Some(Effect::Api(ApiCmd::Retarget {
            identity: self.session.server_id.clone(),
            server: reach.base,
            token: reach.token,
            self_signed: reach.self_signed,
            peer: Some(id),
            local_token: reach.local_token,
        }))
    }

    /// Connect the session through the tunnel under `transport`: at once
    /// when it is up, after opening it when it is not — `TunnelUp` then
    /// finishes the job.
    fn connect_through(&mut self, transport: &str, credential: String) -> Vec<Effect> {
        match self.tunnels.get(transport) {
            Some(super::TunnelState::Up { .. }) => self.connect_over_tunnel(transport),
            Some(super::TunnelState::Dialling) => {
                self.pending_tunnel = Some(transport.to_string());
                Vec::new()
            }
            _ => {
                self.pending_tunnel = Some(transport.to_string());
                self.tunnels.insert(transport.to_string(), super::TunnelState::Dialling);
                vec![Effect::Api(ApiCmd::TunnelOpen { id: transport.to_string(), credential })]
            }
        }
    }

    /// The Connect for a session whose transport tunnel is up.
    fn connect_over_tunnel(&mut self, transport: &str) -> Vec<Effect> {
        let Some(super::TunnelState::Up { local_url, local_token, .. }) = self.tunnels.get(transport)
        else {
            return Vec::new();
        };
        let (local_url, local_token) = (local_url.clone(), local_token.clone());
        self.session.server = local_url.clone();
        vec![Effect::Api(ApiCmd::Connect {
            server: local_url,
            identity: self.identity_or_server(),
            token: self.session.token.clone(),
            // Plain http on loopback: TLS trust never comes up.
            self_signed: false,
            peer: self.session.peer.as_ref().map(|(_, id)| *id),
            local_token: Some(local_token),
        })]
    }

    /// What the session is filed under: its identity, or its address for a
    /// server that has no other.
    fn identity_or_server(&self) -> String {
        if self.session.server_id.is_empty() {
            self.session.server.clone()
        } else {
            self.session.server_id.clone()
        }
    }

    /// The saved server `old` was re-addressed as `new` (the servers room's
    /// edit; multi-server clause 1): every reference the App holds follows
    /// — the queued rows' origins and the plays owed under them, the
    /// session's own identity or its peer's parent, the DJ's server and
    /// Play from's home, Auto DJ's seed rows (auto-dj clause 66) and the
    /// lane's marks of them (decision 39).
    pub(crate) fn rename_server(&mut self, old: &str, new: &str) {
        let rename = |id: &mut String| {
            if let Some(renamed) = crate::config::renamed_identity(id, old, new) {
                *id = renamed;
            }
        };
        for item in &mut self.queue.items {
            rename(&mut item.origin.server);
        }
        self.queue.touch();
        for owed in &mut self.stats.outbox {
            rename(&mut owed.origin.server);
        }
        if let Some(session) = self.stats.session.as_mut() {
            rename(&mut session.origin.server);
        }
        let backoff = std::mem::take(&mut self.stats.backoff);
        self.stats.backoff = backoff
            .into_iter()
            .map(|(mut server, until)| {
                rename(&mut server);
                (server, until)
            })
            .collect();
        rename(&mut self.session.server_id);
        if let Some((parent, _)) = self.session.peer.as_mut() {
            rename(parent);
        }
        if let Some(server) = self.dj_server.as_mut() {
            rename(server);
        }
        if let Some(target) = self.dj_target.as_mut() {
            rename(target);
        }
        // Left out before, `prefs()` wrote the old home back over the
        // file's own fix (`config::follow_rename`).
        if let Some(home) = self.dj_home.as_mut() {
            rename(home);
        }
        self.seeds.rename_server(old, new);
        let rekey = |key: &super::SeedKey| match crate::config::renamed_identity(&key.server, old, new) {
            Some(server) => super::SeedKey { server: server.trim_end_matches('/').to_string(), ..key.clone() },
            None => key.clone(),
        };
        self.lane.picked = self.lane.picked.iter().map(rekey).collect();
        self.lane.last_started = self.lane.last_started.as_ref().map(rekey);
        // The blend's marks follow too (decision 39): a quick skip of the
        // renamed row is still the blend's, and the pass-over still names it.
        self.lane.blended = self.lane.blended.as_ref().map(rekey);
        let passed: Vec<super::SeedKey> = self.lane.passed_over.iter().map(rekey).collect();
        self.lane.passed_over.clear();
        for key in passed {
            if !self.lane.passed_over.contains(&key) {
                self.lane.passed_over.push(key);
            }
        }
    }

    /// The peer a sign-in at `identity` is for: the session's own, when
    /// the identity is that peer's parent. A peer has no accounts of its
    /// own — the parent's token went stale, the parent takes the sign-in —
    /// and the session that follows must go on browsing the peer
    /// (multi-server contract, clause 26), not the parent's library.
    pub(crate) fn peer_behind(&self, identity: &str) -> Option<i64> {
        self.session
            .peer
            .as_ref()
            .filter(|(parent, _)| crate::config::same_server(parent, identity))
            .map(|(_, id)| *id)
    }

    /// Point the session at another saved server and reconnect — the GUI's
    /// server switch. The same door as [`App::begin`], with the teardown a
    /// mid-session change needs first.
    ///
    /// What is playing keeps playing, and so does the rest of the queue:
    /// every row carries its own server and resolves against it at play
    /// time ([`App::play_index`]), so a switch changes what the browser
    /// shows and nothing else (contract clause 11).
    pub(crate) fn adopt_server(&mut self, session: Session, last_path: Option<String>) -> Vec<Effect> {
        self.connected = false;
        self.session = session;
        self.shed_server_state();
        self.path = last_path.unwrap_or_default();
        self.begin()
    }

    /// Drop everything that belonged to the server being left — the
    /// announced next, the search, the browse — and keep the queue: its rows
    /// carry their own servers (contract clause 11). What is already
    /// streaming keeps playing. Shared by [`App::adopt_server`] and the
    /// GUI's pairing-code dial, which can only shed once the new tunnel has
    /// actually answered.
    pub(crate) fn shed_server_state(&mut self) {
        // The Connected handler rebuilds capabilities, libraries and panes.
        // Every library and search ask still out belongs to the old server:
        // the generation moves on, so their answers are dropped on arrival.
        self.session_gen = self.session_gen.wrapping_add(1);
        self.announced = None;
        self.search_hits = None;
        self.query.clear();
        // The box is empty, so no search is wanted: the marker that guards
        // stale results goes with it.
        self.search_submitted = None;
        self.search.set(Vec::new());
        self.files.set(Vec::new());
        self.files.loading = true;
        // The album wall was the old server's too, and so were the
        // Artists and Genres lists.
        self.albums = None;
        self.artist_albums = None;
        self.albums_rev = self.albums_rev.wrapping_add(1);
        self.artists = None;
        self.genre_list = None;
        self.song_ranks = None;
        self.mood_undo = None;
        self.library.set(Vec::new());
        self.library_stack = super::nav::Drill::new(crate::tui::worker::LibraryNode::Root);
        // The Discover room's answer, pins and prefs were the old server's
        // (discovery-page clause 31).
        self.reset_taste();
        // So were the Peer recs room's visit and its preview answers.
        self.reset_peer_recs();
        // Keep a copy's window and page marks were too; its state follows
        // the session's own server (keep-a-copy spec §3.2).
        self.keep_session_moved();
    }

    /// Every reply about who we are connected to, and how.
    ///
    /// One door, like the DJ panel's (audit #57), for the same reason:
    /// these all decide the same two flags -- connected and connecting.
    /// Reading them together is how you can see that they agree.
    pub(super) fn consume_session(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Connected { server, id, username, token, ping } => {
                // A Connected for a server this session was never seated on
                // — a pasted code's dial answering while another server is
                // up — is a fresh session: nothing of the old one's
                // identity, token, name or place may carry over, or the old
                // server's token rides every request to the new one (the
                // review's token leak). A reconnect to the same identity,
                // or to a peer's parent, keeps what it had.
                let fresh = !self.session.server_id.is_empty()
                    && !crate::config::same_server(&id, &self.session.server_id)
                    && !self.session.peer.as_ref().is_some_and(|(parent, _)| crate::config::same_server(parent, &id));
                if fresh {
                    self.session.peer = None;
                    self.session.server_id = id.clone();
                    self.session.token = None;
                    self.session.username = None;
                    if !crate::quickconnect::is_tunnel_id(&id) {
                        self.session.tunnel_code = None;
                    }
                    self.shed_server_state();
                }
                self.connected = true;
                self.connecting = false;
                self.session.server = server;
                // A peer session is filed under the peer's own identity; the
                // worker answered with the parent's.
                if self.session.peer.is_none() {
                    self.session.server_id = id;
                }
                if token.is_some() {
                    self.session.token = token;
                }
                if username.is_some() && username != self.session.username {
                    // Another account on the same server can see other
                    // libraries: the Artists and Genres lists kept for the
                    // rooms were the last one's (performance audit #99). The
                    // album wall keeps its own lifecycle — dropped while it
                    // stands, it would wait on an ask nothing sent.
                    self.artists = None;
                    self.genre_list = None;
                }
                if username.is_some() {
                    self.session.username = username;
                }
                self.capabilities = crate::api::types::Capabilities::from(ping.as_ref());
                // A peer keeps none of the optional features: its similar
                // tracks, sonic path and playlists are off the federation
                // allowlist, whatever the peer reports (contract clause 26).
                if self.session.peer.is_some() {
                    self.capabilities = crate::api::types::Capabilities::default();
                }
                // The Auto-DJ rows and the Sonic Path tab both turn on what
                // the ping just said; a reconnect can be a different server.
                self.dj_panel.rebuild(&self.dj, self.dj_info.as_ref(), self.dj_is_peer());
                self.reset_sonic_path();
                // A tab this server cannot serve is off the strip, so being
                // left standing on one is being on a tab with no number.
                if !self.tab.available(self.capabilities) {
                    self.tab = Tab::Files;
                }
                self.libraries = ping.vpaths.clone();
                // Cover filenames only mean anything to the server that
                // minted them; a reconnect may be a different server. The
                // order goes with the map: a name left behind in it would
                // come back doubled once a late answer files it again. A
                // claim still unanswered is withdrawn as it goes, as an
                // eviction's is: no lane asks for it once the session has
                // moved on (the review of performance audit #88).
                let withdrawn: Vec<Effect> =
                    self.art.drain().filter_map(|(name, value)| super::withdrawal(name, value)).collect();
                self.art_order.clear();
                self.art_small.clear();
                self.path_came_up();
                // The playing track's cover went with them, and only a
                // track's start asked for the original: the queue panel and
                // the wall, asking first, left its big box on their small
                // copy for the rest of the track (the integration check of
                // performance audit #92).
                let playing_cover = self.fetch_art();
                let libraries = ping.vpaths.len();
                self.info(format!(
                    "connected to {} ({} librar{})",
                    self.server_display(),
                    libraries,
                    if libraries == 1 { "y" } else { "ies" }
                ));

                // A DJ armed on this very server learns what it offers from
                // this ping until its probe answers (auto-dj contract, 19).
                let dj_effects = self.dj_session_connected();

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
                effects.extend(withdrawn);
                effects.extend(playing_cover);
                effects.extend(dj_effects);
                // Worth persisting when we hold a token we logged in for — or
                // a pairing code, which is the only way back to this server
                // even when it needs no login at all. A peer session holds
                // the parent's, already saved under the parent.
                let signed_in = self.session.token.is_some() && self.session.username.is_some();
                if (signed_in || self.session.tunnel_code.is_some()) && self.session.peer.is_none() {
                    effects.push(Effect::SaveSession);
                }
                // The peers this server lists, folded into the saved list
                // (contract clause 20); a server that stopped listing any
                // marks the ones it had missing.
                // Whether this server hands its devices direct access to
                // its peers (contract clause 27) — read here, with the
                // token, since the flag is caller-scoped.
                if self.session.peer.is_none() && !self.session.server_id.is_empty() {
                    if ping.federation_direct {
                        self.direct_offered.insert(self.session.server_id.clone());
                    } else {
                        self.direct_offered.remove(&self.session.server_id);
                    }
                }
                if self.session.peer.is_none() && !self.session.server_id.is_empty() {
                    let parent = self.session.server_id.clone();
                    if self.capabilities.federation_browse {
                        effects.push(Effect::Api(ApiCmd::FederationPeers { parent }));
                    } else {
                        effects.push(Effect::SavePeers { parent, listed: Vec::new() });
                    }
                }
                effects
            }
            // What the network answered is the shell's to list (its servers
            // room reads the event itself).
            Event::ServersDiscovered(_) => Vec::new(),
            Event::NeedsLogin { server } => {
                // A reply from a connection attempt that has been overtaken —
                // we already reached somewhere else. Applying it would drag a
                // connected session back to a login form.
                if self.connected {
                    return Vec::new();
                }
                self.connecting = false;
                // A tunnel that came up and was asked to sign in: the
                // shell's form carries the loopback address, a real, working
                // endpoint for the sign-in about to happen; the identity is
                // what the session is filed under.
                match self.tunnel_at(&server).map(|(id, _)| id.to_string()) {
                    Some(id) => {
                        self.session.server_id = id;
                        self.info("tunnel open — sign in to continue");
                    }
                    None => self.info("this server needs a sign-in"),
                }
                Vec::new()
            }
            Event::Unauthorized => {
                // A direct peer's wall stopped honouring the guest token:
                // renew it through the parent rather than asking anyone to
                // sign in (contract clause 27).
                if self.session_is_direct() {
                    let pid = self.session.server_id.clone();
                    let what = self.server_name_of(&pid);
                    if let Some(state) = self.direct.get_mut(&pid) {
                        state.refused = state.ticket.as_ref().map(|t| t.ticket.clone());
                        state.last_ask = None;
                    }
                    self.info(format!("Renewing access to {what}…"));
                    return Vec::new();
                }
                // An established session went bad: the shell offers its
                // sign-in for the server we were already using.
                self.connected = false;
                self.connecting = false;
                self.session.token = None;
                self.error("session expired — sign in again");
                Vec::new()
            }
            // The caller matches exactly the four arms above.
            _ => Vec::new(),
        }
    }

    /// Every word from the worker about a tunnel: the registry the queue's
    /// rows resolve against (contract clause 38), and the session waiting on
    /// one to come up.
    pub(super) fn consume_tunnel(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::TunnelUp { id, local_url, local_token } => {
                self.tunnels.insert(
                    id.clone(),
                    super::TunnelState::Up {
                        local_url,
                        local_token,
                        status: crate::quickconnect::TunnelStatus::Connected,
                        path: None,
                    },
                );
                self.tunnel_retry.remove(&id);
                self.path_came_up();
                let mut effects = Vec::new();
                if self.pending_tunnel.as_deref() == Some(id.as_str()) {
                    self.pending_tunnel = None;
                    effects.extend(self.connect_over_tunnel(&id));
                }
                // The row parked on it starts — at its restored spot when it
                // holds one (contract clause 37).
                if let Some(wait) = self.tunnel_wait.as_ref().filter(|w| w.id == id) {
                    let index = wait.index;
                    self.tunnel_wait = None;
                    effects.extend(self.play_row_resuming(index));
                }
                // A DJ armed on this tunnel probes and asks for an owed
                // turn (auto-dj contract, clause 35).
                effects.extend(self.dj_tunnel_up(&id));
                // The browsed peer's own tunnel: the session moves onto it,
                // browse stack and all (contract clause 27).
                if self.connected
                    && self.session.peer.is_some()
                    && crate::config::same_server(&id, &self.session.server_id)
                    && !self.session_is_direct()
                    && let Some(effect) = self.retarget_to_direct(&id)
                {
                    effects.push(effect);
                }
                effects
            }
            Event::TunnelFailed { id, rejected, why } => {
                // A failed swap on a tunnel that is up leaves it up.
                if !matches!(self.tunnels.get(&id), Some(super::TunnelState::Up { .. })) {
                    self.tunnels.insert(id.clone(), super::TunnelState::Down { rejected, why: why.clone() });
                    // One more rung on the ladder (contract clause 38).
                    let now = crate::clock::Instant::now();
                    let retry = self
                        .tunnel_retry
                        .entry(id.clone())
                        .or_insert(super::TunnelRetry { failed_at: now, failures: 0 });
                    retry.failures += 1;
                    retry.failed_at = now;
                }
                if self.pending_tunnel.as_deref() == Some(id.as_str()) {
                    self.pending_tunnel = None;
                    self.connecting = false;
                    self.error(why.clone());
                }
                // A peer refusing its guest ticket is not a re-pair: the
                // parent re-mints it, and the reconcile asks at once
                // (contract clause 27). The proxy serves meanwhile.
                let peer = id.starts_with(crate::config::PEER_ID_PREFIX);
                if rejected
                    && peer
                    && let Some(state) = self.direct.get_mut(&id)
                {
                    state.refused = state.ticket.as_ref().map(|t| t.ticket.clone());
                    state.last_ask = None;
                }
                // A row parked on a tunnel the server refused walks on like a
                // row the engine refused; one whose server did not answer
                // keeps waiting for the ladder.
                if rejected
                    && !peer
                    && let Some(wait) = self.tunnel_wait.as_ref().filter(|w| w.id == id)
                {
                    let index = wait.index;
                    self.tunnel_wait = None;
                    return self.unplayable(index, why);
                }
                Vec::new()
            }
            Event::TunnelClosed { id } => {
                let was_direct = self.session_is_direct()
                    && crate::config::same_server(&id, &self.session.server_id);
                self.tunnels.remove(&id);
                if self.pending_tunnel.as_deref() == Some(id.as_str()) {
                    self.pending_tunnel = None;
                    self.connecting = false;
                }
                // The browsed peer's own tunnel went: back through the
                // parent's proxies, browse stack and all.
                if was_direct && self.connected {
                    return self.retarget_to_proxy().into_iter().collect();
                }
                Vec::new()
            }
            Event::TunnelStatus { id, status } => {
                if let Some(super::TunnelState::Up { status: current, .. }) = self.tunnels.get_mut(&id) {
                    *current = status;
                }
                // A running peer tunnel whose supervisor gave up on the guest
                // token: the parent re-mints it, and the swap re-dials at
                // once (contract clause 27). A Quick Connect server's
                // rotated code is a re-pair, not a retry.
                if status == crate::quickconnect::TunnelStatus::Rejected
                    && id.starts_with(crate::config::PEER_ID_PREFIX)
                    && let Some(state) = self.direct.get_mut(&id)
                    && state.refused.is_none()
                {
                    state.refused = state.ticket.as_ref().map(|t| t.ticket.clone());
                    state.last_ask = None;
                }
                Vec::new()
            }
            Event::TunnelPath { id, path } => {
                if let Some(super::TunnelState::Up { path: current, .. }) = self.tunnels.get_mut(&id) {
                    *current = Some(path);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The parent's answer about direct access to one of its peers
    /// (contract clause 27): a ticket dials the peer's own tunnel — or swaps
    /// into the running one and moves a browsed peer's session onto it; a
    /// refusal holds for the session and puts the peer back on the proxy;
    /// a failure waits for the gap.
    pub(super) fn consume_direct(
        &mut self,
        parent: &str,
        id: i64,
        answer: crate::api::types::DirectAnswer,
    ) -> Vec<Effect> {
        use crate::api::types::DirectAnswer;
        let pid = crate::config::peer_identity(parent, id);
        let now = crate::clock::Instant::now();
        let browsed = self.session.peer.is_some()
            && crate::config::same_server(&pid, &self.session.server_id);
        let mut effects = Vec::new();
        match answer {
            DirectAnswer::Granted(ticket) => {
                let state = self.direct.entry(pid.clone()).or_default();
                state.asking = false;
                state.denied = false;
                state.last_failure = None;
                let changed = state.ticket.as_ref().map(|t| t.ticket.as_str()) != Some(ticket.ticket.as_str());
                if !changed {
                    // The same ticket again — the parent's cache was not due
                    // for a re-mint. A refused one stays refused, and the gap
                    // applies before the next ask.
                    if state.refused.is_some() {
                        state.last_failure = Some(now);
                    }
                    return effects;
                }
                state.ticket = Some(ticket.clone());
                state.fetched_at = Some(now);
                state.refused = None;
                match self.tunnels.get(&pid) {
                    // In place: same port, the queued URLs survive; only what
                    // is asked for from now on carries the new token.
                    Some(super::TunnelState::Up { .. }) => {
                        effects.push(Effect::Api(ApiCmd::TunnelCredential {
                            id: pid.clone(),
                            credential: ticket.ticket.clone(),
                        }));
                        if browsed
                            && self.connected
                            && let Some(effect) = self.retarget_to_direct(&pid)
                        {
                            effects.push(effect);
                        }
                        // A row held for the renewal plays again.
                        if let Some(wait) = self.tunnel_wait.as_ref().filter(|w| w.id == pid) {
                            let index = wait.index;
                            self.tunnel_wait = None;
                            effects.extend(self.play_row_resuming(index));
                        }
                    }
                    // A refused dial left it down: the reconcile dials again
                    // with the new ticket.
                    Some(super::TunnelState::Down { .. }) => {
                        self.tunnels.remove(&pid);
                    }
                    _ => {}
                }
            }
            DirectAnswer::Denied(_reason) => {
                let state = self.direct.entry(pid.clone()).or_default();
                state.asking = false;
                state.denied = true;
                state.ticket = None;
                state.refused = None;
                let was_direct = browsed && self.session_is_direct();
                if self.tunnels.remove(&pid).is_some() {
                    self.tunnel_retry.remove(&pid);
                    effects.push(Effect::Api(ApiCmd::TunnelClose { id: pid.clone() }));
                }
                if was_direct
                    && self.connected
                    && let Some(effect) = self.retarget_to_proxy()
                {
                    effects.push(effect);
                }
                // A row held for the renewal plays through the proxy now.
                if let Some(wait) = self.tunnel_wait.as_ref().filter(|w| w.id == pid) {
                    let index = wait.index;
                    self.tunnel_wait = None;
                    effects.extend(self.play_row_resuming(index));
                }
            }
            DirectAnswer::Failed(why) => {
                let state = self.direct.entry(pid.clone()).or_default();
                state.asking = false;
                state.last_failure = Some(now);
                if let Some(wait) = self.tunnel_wait.as_ref().filter(|w| w.id == pid) {
                    let index = wait.index;
                    self.tunnel_wait = None;
                    return self.unplayable(index, why);
                }
            }
        }
        effects
    }
}
