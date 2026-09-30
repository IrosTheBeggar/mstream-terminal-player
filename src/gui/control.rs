//! The control API as a face of the desktop player — server audio's
//! one-engine rule (mStream Phase C).
//!
//! mStream's server-audio feature drives a player over the jukebox control
//! API (serve/mod.rs — control API v1). Headless, that player is
//! `mstream-player serve`; on a desktop where the GUI is open it must be
//! THE GUI, or two players share one machine's speakers. So the GUI can
//! host the same API, over the same wire, answered from its own queue and
//! worker: `gui --serve-port <port>` — the launcher passes the server's
//! configured engine port when server audio is on — bound to loopback
//! only, with the port and a per-session token published in the
//! launcher's sidecar (instance.rs) so the server can find the face and
//! prove it is us.
//!
//! Threads: one listener thread owns the socket, vets each request with
//! the serve module's parser, and hands the resulting [`Command`] to the
//! GUI thread over a channel with a reply slot. The GUI loop drains that
//! channel every tick ([`pump`]), executes the command against the App —
//! the same actions a click sends — and answers. A request the GUI cannot
//! take in time gets a 503, never a stuck socket.
//!
//! The bind retries for a while: the server stops its headless engine when
//! it sees the sidecar's claim, and that engine may hold the port for a
//! few seconds more.
//!
//! What the remote sees: paths are the player's own — the server's
//! library paths (vpaths), the same strings the GUI queues from a listing
//! — never absolute paths, which is why the server passes its library
//! paths through untranslated for this face. A row queued from here gets
//! its tags the way the Song Info sheet gets them: the App asks the track's
//! server and fills every copy of the path when the block lands.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::api::types::{Track, TrackMetadata};
use crate::engine::{QueueSnapshot, Status};
use crate::serve::{self, Answer, Command, Verdict};
use crate::tui::app::{Action, Effect, Queued, Repeat};
use crate::tui::worker::AudioCmd;

use super::Gui;

/// What the launcher asked for: the port to serve on, and the token that
/// guards every route but `GET /version`.
pub struct Face {
    pub port: u16,
    pub token: String,
}

/// One vetted request on its way to the GUI thread, and where its answer
/// goes back.
pub struct Request {
    cmd: Command,
    reply: Sender<Answer>,
}

/// The listener's handle. Dropping it stops the thread at its next tick;
/// `port` is where it actually listens, once it does.
pub struct Handle {
    stop: Arc<AtomicBool>,
    /// Read by the tests, which bind port 0 and need the real one; the
    /// GUI itself is told the port by the launcher.
    #[cfg_attr(not(test), allow(dead_code))]
    bound: Arc<Mutex<Option<u16>>>,
}

impl Handle {
    /// The bound port — None until the bind succeeds (or if it never does).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn port(&self) -> Option<u16> {
        *self.bound.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// How long the bind keeps trying. The server's headless engine lets go of
/// the port within its stop wait (three seconds); this clears that with
/// room for a slow machine.
const BIND_PATIENCE: Duration = Duration::from_secs(15);
const BIND_RETRY: Duration = Duration::from_millis(500);
/// How long a request waits for the GUI thread — a frame is 100 ms, a busy
/// page turn a few hundred; past this the remote gets a 503 rather than a
/// hung socket.
const GUI_PATIENCE: Duration = Duration::from_secs(3);

/// Start the listener. Requests arrive on the returned channel; the GUI
/// answers them with [`pump`].
pub fn spawn(face: Face) -> (Handle, Receiver<Request>) {
    let (tx, rx) = channel();
    let stop = Arc::new(AtomicBool::new(false));
    let bound = Arc::new(Mutex::new(None));
    {
        let stop = stop.clone();
        let bound = bound.clone();
        std::thread::Builder::new()
            .name("mstream-control".into())
            .spawn(move || listen(face, tx, stop, bound))
            .expect("spawn the control listener");
    }
    (Handle { stop, bound }, rx)
}

fn listen(face: Face, tx: Sender<Request>, stop: Arc<AtomicBool>, bound: Arc<Mutex<Option<u16>>>) {
    let addr = format!("127.0.0.1:{}", face.port);
    let deadline = Instant::now() + BIND_PATIENCE;
    let server = loop {
        match tiny_http::Server::http(&addr) {
            Ok(server) => break server,
            Err(e) => {
                if stop.load(Ordering::SeqCst) || Instant::now() >= deadline {
                    tracing::warn!(
                        "control face: could not bind {addr} within {}s ({e}) — the server-audio remote will not reach this player",
                        BIND_PATIENCE.as_secs()
                    );
                    return;
                }
                std::thread::sleep(BIND_RETRY);
            }
        }
    };
    let port = server.server_addr().to_ip().map_or(face.port, |a| a.port());
    *bound.lock().unwrap_or_else(|e| e.into_inner()) = Some(port);
    tracing::info!("control face: listening on http://127.0.0.1:{port}");

    while !stop.load(Ordering::SeqCst) {
        let request = match server.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(request)) => request,
            _ => continue,
        };
        match serve::vet(request, "127.0.0.1", port, Some(&face.token)) {
            Verdict::Refuse(request, response) => serve::respond_unread(request, response),
            Verdict::Abandoned => {}
            Verdict::Route(request, Err(response)) => {
                let _ = request.respond(response);
            }
            Verdict::Route(request, Ok(cmd)) => {
                let (reply, answered) = channel();
                let answer = if tx.send(Request { cmd, reply }).is_err() {
                    Answer::error(503, "The player is not accepting commands")
                } else {
                    answered
                        .recv_timeout(GUI_PATIENCE)
                        .unwrap_or_else(|_| Answer::error(503, "The player did not answer in time"))
                };
                let _ = request.respond(serve::respond_with(&answer));
            }
        }
    }
}

/// Answer everything the listener has queued — called by the GUI loop once
/// a tick, on the thread that owns the App.
pub(super) fn pump(gui: &mut Gui, rx: &Receiver<Request>) {
    while let Ok(request) = rx.try_recv() {
        let answer = execute(gui, request.cmd);
        let _ = request.reply.send(answer);
    }
}

/// One command against the App — the same funnel a click uses, so what the
/// remote does is what a user could do. Errors keep the headless engine's
/// wording where the situation is the same (the remote page reads them).
fn execute(gui: &mut Gui, cmd: Command) -> Answer {
    let len = gui.app.queue.items.len();
    match cmd {
        Command::Version => Answer::json(serve::version_body("gui")),
        Command::Status => Answer::json(serde_json::to_value(status_of(gui)).unwrap_or_default()),
        Command::Queue => Answer::json(serde_json::to_value(queue_of(gui)).unwrap_or_default()),

        // The jukebox's play: the queue becomes this one track, playing.
        Command::Play(file) => {
            gui.forward(Action::ClearQueue);
            enqueue(gui, file);
            let effects = gui.app.play_index(0);
            gui.pend(effects);
            Answer::ok()
        }
        Command::QueueAdd(file) => {
            enqueue(gui, file);
            Answer::ok()
        }
        Command::QueueAddMany(files) => {
            for file in files {
                enqueue(gui, file);
            }
            Answer::ok()
        }
        Command::QueuePlayIndex(index) => {
            if index >= len {
                return Answer::error(400, "Index out of bounds");
            }
            let effects = gui.app.play_index(index);
            gui.pend(effects);
            Answer::ok()
        }
        Command::QueueRemove(index) => {
            if index >= len {
                return Answer::error(400, "Index out of bounds");
            }
            let effects = gui.app.remove_queue_row(index);
            gui.pend(effects);
            Answer::ok()
        }
        Command::QueueClear => {
            gui.forward(Action::ClearQueue);
            Answer::ok()
        }

        // Pause and resume are explicit on the wire and a toggle in the
        // App: only flip when the state is the other one, so a repeated
        // /pause never resumes. Resume on an idle player is a no-op like
        // the engine's (a stopped jukebox starts through /play).
        Command::Pause => {
            if !gui.app.status.is_idle() && !gui.app.status.paused {
                gui.forward(Action::PlayPause);
            }
            Answer::ok()
        }
        Command::Resume => {
            if !gui.app.status.is_idle() && gui.app.status.paused {
                gui.forward(Action::PlayPause);
            }
            Answer::ok()
        }
        Command::Stop => {
            stop(gui);
            Answer::ok()
        }
        Command::Next => {
            if len == 0 {
                return Answer::error(400, "Already at end of queue");
            }
            gui.forward(Action::NextTrack);
            Answer::ok()
        }
        Command::Previous => {
            if len == 0 {
                return Answer::error(400, "Failed to play previous track");
            }
            gui.forward(Action::PrevTrack);
            Answer::ok()
        }
        Command::Seek(position) => {
            if gui.app.status.is_idle() {
                return Answer::error(400, "Nothing is playing");
            }
            let effects = gui.app.seek_to(position);
            gui.pend(effects);
            Answer::ok()
        }
        Command::Volume(volume) => {
            gui.set_volume(volume);
            Answer::ok()
        }
        Command::Shuffle(value) => {
            gui.app.queue.shuffle = value;
            Answer::ok()
        }
        Command::CycleLoop => {
            gui.forward(Action::ToggleRepeat);
            Answer::json(serde_json::json!({ "ok": true, "loop_mode": loop_mode(gui.app.queue.repeat) }))
        }
    }
}

/// Stop playback and keep the queue — the jukebox's /stop, as opposed to
/// /queue/clear. The App has no such verb of its own (its Space restarts
/// the queue), so this is ClearQueue's bookkeeping minus the clearing.
fn stop(gui: &mut Gui) {
    gui.app.now_playing = None;
    gui.app.stall = None;
    gui.app.tunnel_wait = None;
    gui.app.queue.current = None;
    gui.pend(vec![Effect::Audio(AudioCmd::Stop)]);
}

/// A path from the remote becomes a row of the session's server — the
/// bundled server, on a launcher-driven player — with no tags yet: the
/// row reads as its file name until the App's track-info fill lands.
fn enqueue(gui: &mut Gui, filepath: String) {
    let origin = gui.app.origin();
    let track = Track { filepath: filepath.clone(), metadata: TrackMetadata::default() };
    gui.app.queue.push(Queued { origin: origin.clone(), dj: None, track });
    let effects = gui.app.fetch_track_info(&origin, &filepath);
    gui.pend(effects);
}

/// The wire's status, read off the App: the engine's own fields, from the
/// worker's last status report and the queue.
fn status_of(gui: &Gui) -> Status {
    let app = &gui.app;
    let file = app
        .queue
        .current
        .and_then(|i| app.queue.items.get(i))
        .map(|row| row.filepath.clone())
        .unwrap_or_default();
    Status {
        playing: app.status.playing,
        paused: app.status.paused,
        position: app.status.position,
        duration: app.status.duration,
        volume: app.volume,
        file,
        queue_index: app.queue.current.unwrap_or(0),
        queue_length: app.queue.items.len(),
        shuffle: app.queue.shuffle,
        loop_mode: loop_mode(app.queue.repeat).to_string(),
    }
}

fn queue_of(gui: &Gui) -> QueueSnapshot {
    QueueSnapshot {
        queue: gui.app.queue.items.iter().map(|row| row.filepath.clone()).collect(),
        current_index: gui.app.queue.current.unwrap_or(0),
    }
}

/// The engine's loop-mode words for the App's repeat states.
fn loop_mode(repeat: Repeat) -> &'static str {
    match repeat {
        Repeat::Off => "none",
        Repeat::One => "one",
        Repeat::All => "all",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::App;
    use crate::tui::worker::ApiCmd;

    fn gui() -> Gui {
        Gui::new(Config::default(), false, App::new(Some("http://localhost:3000".into()), None, None))
    }

    fn body(answer: &Answer) -> &serde_json::Value {
        &answer.body
    }

    #[test]
    fn queued_paths_become_rows_of_the_sessions_server_awaiting_their_tags() {
        let mut g = gui();
        let answer = execute(&mut g, Command::QueueAdd("Music/a.mp3".into()));
        assert_eq!(answer.code, 200);
        assert_eq!(g.app.queue.items.len(), 1);
        let row = &g.app.queue.items[0];
        assert_eq!(row.filepath, "Music/a.mp3");
        assert_eq!(row.origin.server, "http://localhost:3000");
        assert_eq!(row.metadata.title, None, "no tags until the block lands");
        assert_eq!(row.title_or_file(), "a.mp3");
        // The block was asked for, the way the Song Info sheet asks.
        assert!(
            g.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::TrackInfo { filepath, .. }) if filepath == "Music/a.mp3")),
            "{:?}",
            g.pending
        );
        execute(&mut g, Command::QueueAddMany(vec!["Music/b.mp3".into(), "Music/c.mp3".into()]));
        assert_eq!(queue_of(&g).queue, ["Music/a.mp3", "Music/b.mp3", "Music/c.mp3"]);
    }

    #[test]
    fn play_replaces_the_queue_with_the_one_track_and_starts_it() {
        let mut g = gui();
        execute(&mut g, Command::QueueAdd("Music/a.mp3".into()));
        execute(&mut g, Command::QueueAdd("Music/b.mp3".into()));
        let answer = execute(&mut g, Command::Play("Music/c.mp3".into()));
        assert_eq!(answer.code, 200);
        assert_eq!(queue_of(&g).queue, ["Music/c.mp3"]);
        assert_eq!(g.app.queue.current, Some(0), "the row is the one playing");
        assert!(
            g.pending.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { url, .. }) if url.contains("c.mp3"))),
            "{:?}",
            g.pending
        );
        // Status names it, the wire's way.
        let status = status_of(&g);
        assert_eq!(status.file, "Music/c.mp3");
        assert_eq!((status.queue_index, status.queue_length), (0, 1));
    }

    #[test]
    fn status_queue_loop_and_shuffle_mirror_the_app() {
        let mut g = gui();
        let idle = status_of(&g);
        assert!(!idle.playing && !idle.paused);
        assert_eq!(idle.file, "");
        assert_eq!(idle.loop_mode, "none");
        assert_eq!(idle.queue_length, 0);
        // The App cycles off → all → one; the wire words are the engine's.
        for expected in ["all", "one", "none"] {
            let answer = execute(&mut g, Command::CycleLoop);
            assert_eq!(body(&answer)["loop_mode"], expected);
            assert_eq!(status_of(&g).loop_mode, expected);
        }
        execute(&mut g, Command::Shuffle(true));
        assert!(status_of(&g).shuffle);
        execute(&mut g, Command::Shuffle(false));
        assert!(!status_of(&g).shuffle);
        let answer = execute(&mut g, Command::Version);
        assert_eq!(body(&answer)["face"], "gui");
        assert_eq!(body(&answer)["name"], "mstream-player");
    }

    #[test]
    fn indexes_are_bounded_and_removal_keeps_the_rest() {
        let mut g = gui();
        assert_eq!(execute(&mut g, Command::QueuePlayIndex(0)).code, 400, "nothing to play");
        assert_eq!(execute(&mut g, Command::QueueRemove(0)).code, 400);
        execute(&mut g, Command::QueueAddMany(vec!["a".into(), "b".into(), "c".into()]));
        assert_eq!(execute(&mut g, Command::QueuePlayIndex(3)).code, 400);
        assert_eq!(execute(&mut g, Command::QueueRemove(1)).code, 200);
        assert_eq!(queue_of(&g).queue, ["a", "c"]);
        assert_eq!(execute(&mut g, Command::QueueClear).code, 200);
        assert!(g.app.queue.items.is_empty());
    }

    #[test]
    fn transport_needs_something_to_act_on() {
        let mut g = gui();
        assert_eq!(execute(&mut g, Command::Next).code, 400);
        assert_eq!(execute(&mut g, Command::Previous).code, 400);
        assert_eq!(execute(&mut g, Command::Seek(5.0)).code, 400, "nothing is playing");
        // Pause and resume on an idle player are no-ops, never a start.
        assert_eq!(execute(&mut g, Command::Pause).code, 200);
        assert_eq!(execute(&mut g, Command::Resume).code, 200);
        assert!(!g.pending.iter().any(|e| matches!(e, Effect::Audio(_))), "{:?}", g.pending);
        // Stop keeps the queue, drops the playing row.
        execute(&mut g, Command::Play("x".into()));
        assert_eq!(g.app.queue.current, Some(0));
        assert_eq!(execute(&mut g, Command::Stop).code, 200);
        assert_eq!(g.app.queue.current, None);
        assert_eq!(queue_of(&g).queue, ["x"], "the queue survives a stop");
        assert!(g.pending.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Stop))));
    }

    #[test]
    fn volume_is_clamped_like_the_bars_cells() {
        let mut g = gui();
        execute(&mut g, Command::Volume(2.0));
        assert_eq!(g.app.volume, 1.0);
        execute(&mut g, Command::Volume(-1.0));
        assert_eq!(g.app.volume, 0.0);
        execute(&mut g, Command::Volume(0.3));
        assert!((g.app.volume - 0.3).abs() < 1e-6);
        assert!(g.pending.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::SetVolume(_)))));
    }

    /// The listener over a real loopback socket: the token gate, the
    /// version probe that needs none, and the 503 when nobody answers.
    #[test]
    fn the_listener_answers_over_loopback() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let (handle, rx) = spawn(Face { port: 0, token: "s3cret".into() });
        let started = Instant::now();
        let port = loop {
            if let Some(p) = handle.port() {
                break p;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "the listener never bound");
            std::thread::sleep(Duration::from_millis(20));
        };
        // A stand-in for the GUI thread: answers what it is asked, blind.
        let answerer = std::thread::spawn(move || {
            while let Ok(request) = rx.recv() {
                let answer = match request.cmd {
                    Command::Version => Answer::json(serve::version_body("gui")),
                    Command::Status => Answer::json(serde_json::json!({ "playing": false })),
                    _ => Answer::ok(),
                };
                let _ = request.reply.send(answer);
            }
        });
        let ask = |raw: &str| -> String {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            s.write_all(raw.as_bytes()).unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        };
        let host = format!("Host: 127.0.0.1:{port}\r\n");
        let version = ask(&format!("GET /version HTTP/1.1\r\n{host}Connection: close\r\n\r\n"));
        assert!(version.starts_with("HTTP/1.1 200"), "{version}");
        assert!(version.contains("\"face\":\"gui\""), "{version}");
        let refused = ask(&format!("POST /pause HTTP/1.1\r\n{host}Content-Length: 0\r\nConnection: close\r\n\r\n"));
        assert!(refused.starts_with("HTTP/1.1 401"), "{refused}");
        let paused = ask(&format!(
            "POST /pause HTTP/1.1\r\n{host}x-auth-token: s3cret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ));
        assert!(paused.starts_with("HTTP/1.1 200"), "{paused}");
        assert!(paused.contains("\"ok\":true"), "{paused}");
        let status = ask(&format!("GET /status HTTP/1.1\r\n{host}x-auth-token: s3cret\r\nConnection: close\r\n\r\n"));
        assert!(status.contains("\"playing\":false"), "{status}");
        let lost = ask(&format!("GET /nowhere HTTP/1.1\r\n{host}x-auth-token: s3cret\r\nConnection: close\r\n\r\n"));
        assert!(lost.starts_with("HTTP/1.1 404"), "{lost}");
        // Nobody left to answer: the socket still gets a reply.
        drop(handle);
        let (handle2, rx2) = spawn(Face { port: 0, token: "t".into() });
        let started = Instant::now();
        let port2 = loop {
            if let Some(p) = handle2.port() {
                break p;
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        };
        drop(rx2);
        let mut s = TcpStream::connect(("127.0.0.1", port2)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(format!("POST /stop HTTP/1.1\r\nHost: 127.0.0.1:{port2}\r\nx-auth-token: t\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 503"), "{out}");
        drop(handle2);
        let _ = answerer.join();
    }
}
