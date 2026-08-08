//! The real api worker for the browser: the native api thread's job, done
//! with futures instead of a thread.
//!
//! Commands arrive from the frame loop, each spawns onto the browser's event
//! loop (`spawn_local`), and whatever event the reply maps to is queued for
//! the next frame to fold in. The command→endpoint logic itself is the same
//! code the native thread runs — worker::load_library and friends — awaited
//! directly instead of parked on the tokio runtime.
//!
//! What this worker does NOT speak: Quick Connect (the tunnel is iroh and
//! native), and mDNS discovery (no multicast in a browser) — the shell keeps
//! answering that one with canned servers so the tab demonstrates itself.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use wasm_bindgen_futures::spawn_local;

use crate::api::types::Capabilities;
use crate::api::{ApiError, Client};
use crate::tui::art;
use crate::tui::worker::{self, ApiCmd, Event};

/// Client plus what its server said it can do — replaced wholesale on every
/// successful connect, exactly like the native loop's `client`/`caps` pair.
struct Session {
    client: Rc<Client>,
    caps: Capabilities,
}

pub struct WebApi {
    session: Rc<RefCell<Option<Session>>>,
    queue: Rc<RefCell<VecDeque<Event>>>,
}

impl WebApi {
    pub fn new(queue: Rc<RefCell<VecDeque<Event>>>) -> Self {
        WebApi { session: Rc::new(RefCell::new(None)), queue }
    }

    pub fn dispatch(&self, cmd: ApiCmd) {
        if matches!(cmd, ApiCmd::Shutdown) {
            return;
        }
        let session = self.session.clone();
        let queue = self.queue.clone();
        spawn_local(async move {
            let event = handle(&session, cmd).await;
            if let Some(event) = event {
                queue.borrow_mut().push_back(event);
            }
        });
    }
}

/// One command, one reply — the async body of the native loop's match.
async fn handle(session: &Rc<RefCell<Option<Session>>>, cmd: ApiCmd) -> Option<Event> {
    match cmd {
        ApiCmd::Shutdown => None,

        ApiCmd::Connect { server, token } => Some(connect(session, &server, token).await),

        ApiCmd::Login { server, username, password } => {
            Some(login(session, &server, &username, &password).await)
        }

        ApiCmd::QuickConnect { .. } => Some(Event::Error(
            "Quick Connect needs the native player — the tunnel is iroh, not HTTP".to_string(),
        )),

        ApiCmd::Browse(path) => {
            with_session(session, async |s| {
                s.client.file_explorer_async(&path).await.map(|l| Event::Listing(Box::new(l)))
            })
            .await
        }

        ApiCmd::Library { node, dest } => {
            with_session(session, async |s| {
                worker::load_library(&s.client, &node)
                    .await
                    .map(|data| Event::Library { node: node.clone(), dest, data })
            })
            .await
        }

        ApiCmd::AutoDj(request) => {
            with_session(session, async |s| {
                worker::autodj_pick(&s.client, s.caps, &request).await.map(|picked| {
                    Event::AutoDjPick {
                        candidates: picked.tracks,
                        ignore_list: picked.ignore_list,
                        note: picked.note,
                    }
                })
            })
            .await
        }

        ApiCmd::AutoDjSample { request, count } => {
            with_session(session, async |s| {
                worker::autodj_sample(&s.client, s.caps, &request, count).await
            })
            .await
        }

        ApiCmd::Genres => {
            with_session(session, async |s| s.client.genres_async().await.map(Event::Genres))
                .await
        }

        ApiCmd::Journey { start, end, length } => {
            with_session(session, async |s| {
                worker::journey(&s.client, &start, &end, length).await
            })
            .await
        }

        ApiCmd::Discover { node, seed, dest } => {
            with_session(session, async |s| {
                worker::discover(&s.client, &node, &seed, dest).await
            })
            .await
        }

        ApiCmd::SavePlaylist { name, files } => {
            with_session(session, async |s| {
                let count = files.len();
                s.client
                    .playlist_save_async(&name, &files)
                    .await
                    .map(|()| Event::PlaylistSaved { name: name.clone(), count })
            })
            .await
        }

        ApiCmd::Search(query) => {
            with_session(session, async |s| {
                s.client.search_async(&query).await.map(|r| Event::SearchResults {
                    query: query.clone(),
                    results: Box::new(r),
                })
            })
            .await
        }

        ApiCmd::AlbumArt { file } => {
            // Every failure is "no art", exactly as the native worker treats
            // it — even a missing session, which the next real request will
            // report in its own voice. Not `with_session`, whose "not
            // connected" error toast is precisely what art must never raise.
            let client = session.borrow().as_ref().map(|s| s.client.clone());
            let art = match client {
                Some(c) => {
                    c.album_art_async(&file).await.ok().and_then(|bytes| art::decode(&bytes))
                }
                None => None,
            };
            Some(Event::AlbumArt { file, art })
        }

        ApiCmd::Waveform { filepath } => {
            // Art's rule, for the same reason: a shape nobody could draw is
            // not worth the "not connected" toast `with_session` would raise.
            let client = session.borrow().as_ref().map(|s| s.client.clone());
            let bars = match client {
                Some(c) => c.waveform_async(&filepath).await.ok().flatten(),
                None => None,
            };
            Some(Event::Waveform { filepath, bars })
        }
    }
}

/// The web copy of the native `connect`: build a client, ping, and store the
/// session on success. No tunnel arm — the identity and the endpoint are
/// always the same URL here.
async fn connect(
    session: &Rc<RefCell<Option<Session>>>,
    server: &str,
    token: Option<String>,
) -> Event {
    let client = match Client::new(server) {
        Ok(c) => c.with_token(token.clone()),
        Err(e) => return Event::Error(e.to_string()),
    };
    match client.ping_async().await {
        Ok(ping) => {
            let server = client.server();
            let caps = Capabilities::from(&ping);
            *session.borrow_mut() = Some(Session { client: Rc::new(client), caps });
            Event::Connected {
                server: server.clone(),
                id: server,
                username: None,
                token,
                ping: Box::new(ping),
            }
        }
        // Reaching the server and being asked to sign in is a normal outcome
        // of picking one, not an authorization failure.
        Err(ApiError::Unauthorized) => Event::NeedsLogin { server: client.server() },
        Err(e) => Event::Error(e.to_string()),
    }
}

/// The web copy of the native `login`, minus the tunnel bookkeeping.
async fn login(
    session: &Rc<RefCell<Option<Session>>>,
    server: &str,
    username: &str,
    password: &str,
) -> Event {
    let client = match Client::new(server) {
        Ok(c) => c,
        Err(e) => return Event::Error(e.to_string()),
    };
    let token = match client.login_async(username, password).await {
        Ok(resp) => resp.token,
        Err(ApiError::Unauthorized) => {
            return Event::Error("login failed — check the username and password".into());
        }
        Err(e) => return Event::Error(e.to_string()),
    };
    let client = client.with_token(Some(token.clone()));
    match client.ping_async().await {
        Ok(ping) => {
            let server = client.server();
            let caps = Capabilities::from(&ping);
            *session.borrow_mut() = Some(Session { client: Rc::new(client), caps });
            Event::Connected {
                server: server.clone(),
                id: server,
                username: Some(username.to_string()),
                token: Some(token),
                ping: Box::new(ping),
            }
        }
        Err(e) => Event::Error(e.to_string()),
    }
}

/// The web copy of the native `with_client`, one `await` deeper.
async fn with_session<F>(session: &Rc<RefCell<Option<Session>>>, f: F) -> Option<Event>
where
    F: AsyncFnOnce(&Session) -> Result<Event, ApiError>,
{
    // Cloned out rather than held across the await: a borrow across a
    // suspension point would panic the next dispatch that lands mid-request.
    let current = match &*session.borrow() {
        Some(s) => Session { client: s.client.clone(), caps: s.caps },
        None => return Some(Event::Error("not connected to a server".into())),
    };
    match f(&current).await {
        Ok(event) => Some(event),
        // Only 401 means the session is no good; 403 is a permission or
        // feature-flag answer that shouldn't bounce the user to a login form.
        Err(ApiError::Unauthorized) => Some(Event::Unauthorized),
        Err(e) => Some(Event::Error(e.to_string())),
    }
}
