//! Headless jukebox mode: the JSON control API over HTTP.
//!
//! Route-for-route and shape-for-shape compatible with mStream's
//! rust-server-audio (control API v1 — see PLAN.md). Additions are additive
//! only: GET /version, optional x-auth-token auth, configurable bind address,
//! an --exit-with-parent stdin watchdog, and the request hygiene the
//! original went without — Host/Origin/Content-Type validation and a body
//! cap (findings #27/#28/#30). A legitimate client never notices the
//! hygiene: correct Host and application/json are what every HTTP library
//! sends anyway, and no page in a browser has any business here.

use std::io::Read;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use tiny_http::{Header, Method, Response, Server};

use crate::engine::{Engine, EngineError};

pub const API_VERSION: u32 = 1;

pub struct ServeOptions {
    pub host: String,
    pub port: u16,
    pub auth_token: Option<String>,
    pub exit_with_parent: bool,
    /// Seconds of blend when one track ends and the next begins; 0 keeps
    /// the plain cut between tracks. Not reachable from the legacy
    /// `--port N` spawn contract, which is deliberate — the wire and the
    /// queue behavior never change unasked. (The C4 soft cuts on manual
    /// /next and /stop are the one global departure: 150/80 ms fade tails
    /// where the original clicked.)
    pub crossfade: f32,
    /// Sample-tight transitions when no blend is configured. Same legacy
    /// stance: unreachable from `--port N`.
    pub gapless: bool,
}

// ── Request types (wire-compatible with rust-server-audio) ──────────────────

#[derive(Deserialize)]
struct PlayRequest {
    file: String,
}

#[derive(Deserialize)]
struct AddManyRequest {
    files: Vec<String>,
}

#[derive(Deserialize)]
struct IndexRequest {
    index: usize,
}

#[derive(Deserialize)]
struct SeekRequest {
    position: f64,
}

#[derive(Deserialize)]
struct VolumeRequest {
    volume: f32,
}

#[derive(Deserialize)]
struct BoolRequest {
    value: bool,
}

#[derive(Serialize)]
struct OkResponse {
    ok: bool,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ── HTTP helpers ────────────────────────────────────────────────────────────

pub type Resp = Response<std::io::Cursor<Vec<u8>>>;

fn json_response<T: Serialize>(data: &T) -> Resp {
    let body = serde_json::to_vec(data).unwrap_or_default();
    let header = Header::from_bytes("Content-Type", "application/json").unwrap();
    Response::from_data(body).with_header(header)
}

fn error_response_with_status(msg: &str, status: u16) -> Resp {
    json_response(&ErrorResponse { error: msg.to_string() }).with_status_code(status)
}

fn error_response(msg: &str) -> Resp {
    error_response_with_status(msg, 400)
}

fn ok_resp() -> Resp {
    json_response(&OkResponse { ok: true })
}

fn read_body(request: &mut tiny_http::Request) -> Option<String> {
    // Only called after the loop has vetted the declared length (≤ BODY_CAP).
    // Reading to EOF matters beyond getting the bytes: it is what disarms
    // tiny_http's drop-time drain (see respond_unread). The take() is a belt
    // in case the two counts ever disagree, one byte over the cap so nothing
    // legitimate can hit it.
    let mut body = String::new();
    request.as_reader().take(BODY_CAP as u64 + 1).read_to_string(&mut body).ok()?;
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

fn parse<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, Resp> {
    serde_json::from_str(body).map_err(|e| error_response(&format!("Invalid JSON: {}", e)))
}

/// Map an engine error onto the wire, preserving the original error strings
/// where the original had them. `fallback` is the route-specific message the
/// original used for play failures.
fn engine_error(e: EngineError, fallback: &str) -> Resp {
    match e {
        EngineError::NoDevice(_) => error_response_with_status(&e.to_string(), 500),
        EngineError::OutOfBounds => error_response("Index out of bounds"),
        EngineError::EndOfQueue => error_response("Already at end of queue"),
        EngineError::Seek(_) => error_response(&e.to_string()),
        EngineError::Unplayable(_) => error_response(fallback),
    }
}

fn constant_time_eq(expected: &str, got: &str) -> bool {
    if expected.len() != got.len() {
        return false;
    }
    expected
        .bytes()
        .zip(got.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

// ── Request hygiene ─────────────────────────────────────────────────────────
//
// tiny_http hands bodies over in two shapes. At or under 1024 declared bytes
// the body is read into memory before the request ever reaches us: reading
// it can't block and dropping it costs nothing. Over that — or deferred by
// Expect: 100-continue — the "body" is the socket itself, and both halves
// become the client's to dictate. Reading waits on them; dropping unread
// inherits tiny_http's cleanup, a drain that sizes its buffer from the
// *declared* Content-Length and reads until the peer obliges
// (EqualReader::drop). Either one on the serve loop hands our schedule and
// our memory to whoever wrote the headers, auto-advance with it (#28).
//
// tiny_http exposes no socket timeouts, so the rule here is structural: the
// loop never reads a socket. A body on the socket is read on a helper
// thread and waited for with a deadline; a request being refused unread is
// disposed of on a helper too. What that cannot prevent is a helper parked
// on a client that stops sending — no timeout exists to cut it loose — so
// a stalling connection still costs one thread. It no longer costs the
// jukebox, which is the part that was broken.

/// Largest body any route has a use for. The fattest legitimate request is
/// /queue/add-many with a few hundred URLs; this clears that by an order of
/// magnitude while staying too small to matter as an allocation.
const BODY_CAP: usize = 64 * 1024;

/// What tiny_http prebuffers before the request reaches us (request.rs,
/// `content_length <= 1024 && !expects_continue`). Above this, the reader
/// is the socket.
const PREBUFFERED_MAX: usize = 1024;

/// How long the loop will wait for a body that has to come off a socket.
/// Loopback delivers 64 KB in under a millisecond, so this only expires on
/// a client that has stopped sending — and then the loop leaves rather than
/// waits, because auto-advance is behind it.
const BODY_DEADLINE: Duration = Duration::from_secs(2);

fn header_value<'r>(request: &'r tiny_http::Request, name: &'static str) -> Option<&'r str> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

/// True when the body is still on the socket rather than in a buffer — the
/// one shape that can both block a read and cost something to drop.
///
/// Chunked is deliberately not counted. It has no declared length, so
/// tiny_http wraps it in a decoder with no `Drop` at all: nothing to drain,
/// nothing to allocate, dropping one is free. The loop refuses it with 411
/// before any body is touched, so it never reaches the reading path either.
fn body_is_on_the_socket(request: &tiny_http::Request) -> bool {
    match request.body_length() {
        Some(n) => n > PREBUFFERED_MAX || (n > 0 && header_value(request, "expect").is_some()),
        None => false,
    }
}

/// Answer without having read the body. Prebuffered requests respond in
/// place. A request with a live body goes to a disposal thread, where the
/// body is drained through a fixed buffer so that tiny_http's cleanup finds
/// nothing left to size an allocation by — a peer that stalls or trickles
/// parks that thread, not the jukebox. If the socket errors mid-drain, or
/// ends short of what its headers declared, the request is deliberately
/// leaked: cleanup would allocate the declared remainder to drain a socket
/// that has nothing more to give, and an attacker picks that number. One
/// leaked handle against a wedge or an abort.
pub fn respond_unread(request: tiny_http::Request, response: Resp) {
    if !body_is_on_the_socket(&request) {
        let _ = request.respond(response);
        return;
    }
    std::thread::spawn(move || {
        let mut request = request;
        // Only reached with a declared length, so the loop terminates on
        // the client's own number rather than on trust.
        let mut remaining = request.body_length().unwrap_or(0);
        let mut buf = [0u8; 8192];
        while remaining > 0 {
            match request.as_reader().read(&mut buf) {
                // EOF short of the declared length, or a broken socket:
                // either way there is nothing left to drain, and letting
                // cleanup run would allocate the declared remainder to
                // find that out — a number the client chose.
                Ok(0) | Err(_) => {
                    std::mem::forget(request);
                    return;
                }
                Ok(n) => remaining = remaining.saturating_sub(n),
            }
        }
        let _ = request.respond(response);
    });
}

/// Get the body without letting a socket decide how long the loop waits.
///
/// A prebuffered body is already in memory, so reading it can't block and
/// the request never leaves this thread. A live one is read on a helper,
/// which hands the request back through the channel; if it doesn't arrive
/// in time the loop abandons it and the helper answers 408 whenever the
/// client finally moves. Either way the next `recv_timeout` happens on
/// schedule and the queue keeps advancing.
fn take_body(request: tiny_http::Request) -> Option<(tiny_http::Request, Option<String>)> {
    if !body_is_on_the_socket(&request) {
        let mut request = request;
        let body = read_body(&mut request);
        return Some((request, body));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut request = request;
        let body = read_body(&mut request);
        if let Err(returned) = tx.send((request, body)) {
            let (request, _) = returned.0;
            respond_unread(request, error_response_with_status("Request timed out", 408));
        }
    });
    rx.recv_timeout(BODY_DEADLINE).ok()
}

/// The Host values a legitimate client can arrive under. DNS rebinding turns
/// "a page you visited" into "a client on localhost" (finding #30), but the
/// browser still stamps the attacker's own domain into Host — and an address
/// literal can't be rebound. So: literals pass, names must be ours.
fn host_allowed(host: Option<&str>, bind_host: &str, port: u16) -> bool {
    let Some(host) = host else { return false };
    let (name, host_port) = match host.strip_prefix('[') {
        // Bracketed IPv6: [::1] or [::1]:3333.
        Some(rest) => match rest.split_once(']') {
            Some((v6, "")) => (v6, None),
            Some((v6, p)) => match p.strip_prefix(':').and_then(|p| p.parse().ok()) {
                Some(p) => (v6, Some(p)),
                None => return false,
            },
            None => return false,
        },
        None => match host.rsplit_once(':') {
            Some((n, p)) => match p.parse() {
                Ok(p) => (n, Some(p)),
                Err(_) => return false,
            },
            None => (host, None),
        },
    };
    if host_port.unwrap_or(80) != port {
        return false;
    }
    let name = name.to_ascii_lowercase();
    name == bind_host.to_ascii_lowercase()
        || name == "localhost"
        || name.parse::<std::net::IpAddr>().is_ok()
}

/// Cross-site requests without a preflight can only carry a handful of
/// content types, and application/json is not one of them. Requiring it on
/// every request with a body forces a preflight this server never grants;
/// the Origin check covers the body-less routes a simple POST still reaches.
fn is_json(content_type: Option<&str>) -> bool {
    content_type
        .and_then(|ct| ct.split(';').next())
        .map(|ct| ct.trim().eq_ignore_ascii_case("application/json"))
        .unwrap_or(false)
}

// ── Main loop ───────────────────────────────────────────────────────────────

pub fn run(opts: ServeOptions) -> Result<(), String> {
    let engine = Engine::new().map_err(|e| format!("could not initialize audio output: {}", e))?;
    engine.set_crossfade(opts.crossfade);
    engine.set_gapless(opts.gapless);

    if opts.exit_with_parent {
        std::thread::spawn(|| {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 256];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        eprintln!("[serve] stdin closed — exiting (--exit-with-parent)");
                        std::process::exit(0);
                    }
                    Ok(_) => {}
                }
            }
        });
    }

    let addr = format!("{}:{}", opts.host, opts.port);
    let server = Server::http(&addr).map_err(|e| format!("failed to bind {}: {}", addr, e))?;

    println!("mstream-player serve listening on http://{}", addr);

    loop {
        // Auto-advance: check if the sink emptied and move to the next track.
        engine.advance_tick();
        // The tick also watches the output device (headphones plugged in,
        // a Bluetooth speaker dropping); what it did about it prints here.
        for notice in engine.take_device_notices() {
            eprintln!("[serve] {}", notice.text);
        }

        let request = server.recv_timeout(Duration::from_millis(250));
        let request = match request {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(_) => continue,
        };

        match vet(request, &opts.host, opts.port, opts.auth_token.as_deref()) {
            Verdict::Refuse(request, response) => respond_unread(request, response),
            Verdict::Abandoned => {}
            Verdict::Route(request, parsed) => {
                let response = match parsed {
                    Ok(cmd) => execute(&engine, cmd),
                    Err(response) => response,
                };
                let _ = request.respond(response);
            }
        }
    }
}

// ── The wire, parsed ────────────────────────────────────────────────────────
//
// Two faces answer the control API: the headless engine here, and the
// desktop player's own queue (gui::control). They share one parser — every
// hygiene rule, the token gate, the body rules and the route table live in
// `vet` — and each executes the resulting Command its own way, so the wire
// contract cannot drift between them.

/// What one vetted request asks of the player: the control API v1 as a
/// value.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// The queue becomes this one source, playing.
    Play(String),
    Pause,
    Resume,
    Stop,
    Next,
    Previous,
    Seek(f64),
    Volume(f32),
    Shuffle(bool),
    /// Advance the loop mode; the answer names the new one.
    CycleLoop,
    Status,
    QueueAdd(String),
    QueueAddMany(Vec<String>),
    QueuePlayIndex(usize),
    QueueRemove(usize),
    QueueClear,
    Queue,
    /// Unauthenticated on purpose: the liveness probe.
    Version,
}

/// A request after the hygiene, auth and body rules have had their say.
pub enum Verdict {
    /// Refused before its body was touched — answer it with
    /// [`respond_unread`], which never reads a socket on the caller's
    /// thread.
    Refuse(tiny_http::Request, Resp),
    /// The body is read; the route resolved to a command, or to the error
    /// its shape earned (bad JSON, a missing body, no such route).
    Route(tiny_http::Request, Result<Command, Resp>),
    /// The body is still coming off the socket on a helper thread, which
    /// answers for itself if the client ever finishes.
    Abandoned,
}

/// Vet one request: the hygiene of findings #28/#30, the token gate (every
/// route but `GET /version` when a token is set), the body cap and type,
/// then the route table.
pub fn vet(request: tiny_http::Request, bind_host: &str, port: u16, auth_token: Option<&str>) -> Verdict {
    let method = request.method().clone();
    let path = request.url().to_string();

    // Hygiene before anything else answers: refuse what we can't account
    // for (finding #28) and what a browser could be driving (finding #30).
    // Nothing here reads the body.
    if request.body_length().is_none() && header_value(&request, "transfer-encoding").is_some() {
        return Verdict::Refuse(request, error_response_with_status("Length required", 411));
    }
    if !host_allowed(header_value(&request, "host"), bind_host, port) {
        return Verdict::Refuse(request, error_response_with_status("Forbidden: unrecognized Host", 403));
    }

    // Auth gate: everything except GET /version when a token is set.
    if let Some(expected) = auth_token {
        let is_version = method == Method::Get && path == "/version";
        if !is_version {
            let authorized =
                header_value(&request, "x-auth-token").map(|t| constant_time_eq(expected, t)).unwrap_or(false);
            if !authorized {
                return Verdict::Refuse(request, error_response_with_status("Unauthorized", 401));
            }
        }
    }

    if request.body_length().unwrap_or(0) > BODY_CAP {
        return Verdict::Refuse(request, error_response_with_status("Payload too large", 413));
    }
    if method == Method::Post {
        // A browser is the only client that announces itself with an
        // Origin header, and no page anywhere has a legitimate call
        // here — this is what stops a visited site driving the jukebox.
        if header_value(&request, "origin").is_some() {
            return Verdict::Refuse(request, error_response_with_status("Forbidden: cross-origin request", 403));
        }
        if request.body_length().unwrap_or(0) > 0 && !is_json(header_value(&request, "content-type")) {
            return Verdict::Refuse(
                request,
                error_response_with_status("Content-Type must be application/json", 415),
            );
        }
    }

    let Some((request, body)) = take_body(request) else {
        // The helper still owns it and will answer for itself.
        return Verdict::Abandoned;
    };
    Verdict::Route(request, route(method, &path, body.as_deref()))
}

/// The route table: method and path to a Command, with the body each one
/// needs parsed — or the 400/404 the request earned.
fn route(method: Method, path: &str, body: Option<&str>) -> Result<Command, Resp> {
    fn with_body<T: for<'de> Deserialize<'de>>(body: Option<&str>) -> Result<T, Resp> {
        match body {
            Some(b) => parse::<T>(b),
            None => Err(error_response("Missing request body")),
        }
    }
    Ok(match (method, path) {
        (Method::Post, "/play") => Command::Play(with_body::<PlayRequest>(body)?.file),
        (Method::Post, "/pause") => Command::Pause,
        (Method::Post, "/resume") => Command::Resume,
        (Method::Post, "/stop") => Command::Stop,
        (Method::Post, "/next") => Command::Next,
        (Method::Post, "/previous") => Command::Previous,
        (Method::Post, "/seek") => Command::Seek(with_body::<SeekRequest>(body)?.position),
        (Method::Post, "/volume") => Command::Volume(with_body::<VolumeRequest>(body)?.volume),
        (Method::Post, "/shuffle") => Command::Shuffle(with_body::<BoolRequest>(body)?.value),
        (Method::Post, "/loop") => Command::CycleLoop,
        (Method::Get, "/status") => Command::Status,
        (Method::Post, "/queue/add") => Command::QueueAdd(with_body::<PlayRequest>(body)?.file),
        (Method::Post, "/queue/add-many") => Command::QueueAddMany(with_body::<AddManyRequest>(body)?.files),
        (Method::Post, "/queue/play-index") => Command::QueuePlayIndex(with_body::<IndexRequest>(body)?.index),
        (Method::Post, "/queue/remove") => Command::QueueRemove(with_body::<IndexRequest>(body)?.index),
        (Method::Post, "/queue/clear") => Command::QueueClear,
        (Method::Get, "/queue") => Command::Queue,
        (Method::Get, "/version") => Command::Version,
        _ => return Err(error_response_with_status("Not found", 404)),
    })
}

/// `GET /version`: who answers, and with which face — `serve` for the
/// headless engine, `gui` for the desktop player hosting the API. The
/// field is additive (apiVersion stays 1); mStream reads it to tell the
/// engine it spawned from the player it is adopting.
pub fn version_body(face: &str) -> serde_json::Value {
    serde_json::json!({
        "name": "mstream-player",
        "version": env!("CARGO_PKG_VERSION"),
        "apiVersion": API_VERSION,
        "face": face,
    })
}

/// An answer composed away from tiny_http — the GUI face builds these on
/// its own thread, and its listener turns them into responses.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub code: u16,
    pub body: serde_json::Value,
}

impl Answer {
    pub fn ok() -> Self {
        Answer { code: 200, body: serde_json::json!({ "ok": true }) }
    }
    pub fn json(body: serde_json::Value) -> Self {
        Answer { code: 200, body }
    }
    pub fn error(code: u16, message: &str) -> Self {
        Answer { code, body: serde_json::json!({ "error": message }) }
    }
}

pub fn respond_with(answer: &Answer) -> Resp {
    json_response(&answer.body).with_status_code(answer.code)
}

/// The headless engine's execution of a command: route-for-route what
/// rust-server-audio answered, error strings included.
fn execute(engine: &Engine, cmd: Command) -> Resp {
    match cmd {
        Command::Play(file) => match engine.play_source(file, None) {
            Ok(()) => ok_resp(),
            Err(e) => engine_error(e, "Failed to play file"),
        },
        Command::Pause => {
            engine.pause();
            ok_resp()
        }
        Command::Resume => {
            engine.resume();
            ok_resp()
        }
        Command::Stop => {
            engine.stop();
            ok_resp()
        }
        Command::Next => match engine.next_manual() {
            Ok(()) => ok_resp(),
            Err(e) => engine_error(e, "Failed to play next track"),
        },
        Command::Previous => match engine.previous_manual() {
            Ok(()) => ok_resp(),
            Err(e) => engine_error(e, "Failed to play previous track"),
        },
        Command::Seek(position) => match engine.seek(position) {
            Ok(()) => ok_resp(),
            Err(e) => engine_error(e, "Seek failed"),
        },
        Command::Volume(volume) => {
            engine.set_volume(volume);
            ok_resp()
        }
        Command::Shuffle(value) => {
            engine.set_shuffle(value);
            ok_resp()
        }
        Command::CycleLoop => {
            let mode = engine.cycle_loop();
            json_response(&serde_json::json!({ "ok": true, "loop_mode": mode.as_str() }))
        }
        Command::Status => json_response(&engine.status()),
        Command::QueueAdd(file) => {
            engine.queue_add(file);
            ok_resp()
        }
        Command::QueueAddMany(files) => {
            engine.queue_add_many(files);
            ok_resp()
        }
        Command::QueuePlayIndex(index) => match engine.queue_play_index(index) {
            Ok(()) => ok_resp(),
            Err(e) => engine_error(e, "Failed to play track at index"),
        },
        Command::QueueRemove(index) => match engine.queue_remove(index) {
            Ok(()) => ok_resp(),
            Err(e) => engine_error(e, "Failed to remove track"),
        },
        Command::QueueClear => {
            engine.queue_clear();
            ok_resp()
        }
        Command::Queue => json_response(&engine.queue_snapshot()),
        Command::Version => json_response(&version_body("serve")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: Method, path: &str, body: Option<&'static str>) -> tiny_http::Request {
        let mut t = tiny_http::TestRequest::new()
            .with_method(method)
            .with_path(path)
            .with_header(Header::from_bytes("Host", "127.0.0.1:3333").unwrap());
        if let Some(body) = body {
            t = t.with_body(body).with_header(Header::from_bytes("Content-Type", "application/json").unwrap());
        }
        t.into()
    }

    /// What `vet` made of a request: the command, or the status it earned.
    fn routed(method: Method, path: &str, body: Option<&'static str>, token: Option<&str>) -> Result<Command, u16> {
        match vet(request(method, path, body), "127.0.0.1", 3333, token) {
            Verdict::Route(_, Ok(cmd)) => Ok(cmd),
            Verdict::Route(_, Err(resp)) | Verdict::Refuse(_, resp) => Err(resp.status_code().0),
            Verdict::Abandoned => panic!("a prebuffered body is never abandoned"),
        }
    }

    #[test]
    fn the_route_table_is_the_control_api() {
        use Method::{Get, Post};
        let table: Vec<(Method, &str, Option<&'static str>, Command)> = vec![
            (Post, "/play", Some(r#"{"file":"Music/a.mp3"}"#), Command::Play("Music/a.mp3".into())),
            (Post, "/pause", None, Command::Pause),
            (Post, "/resume", None, Command::Resume),
            (Post, "/stop", None, Command::Stop),
            (Post, "/next", None, Command::Next),
            (Post, "/previous", None, Command::Previous),
            (Post, "/seek", Some(r#"{"position":12.5}"#), Command::Seek(12.5)),
            (Post, "/volume", Some(r#"{"volume":0.5}"#), Command::Volume(0.5)),
            (Post, "/shuffle", Some(r#"{"value":true}"#), Command::Shuffle(true)),
            (Post, "/loop", None, Command::CycleLoop),
            (Get, "/status", None, Command::Status),
            (Post, "/queue/add", Some(r#"{"file":"x"}"#), Command::QueueAdd("x".into())),
            (Post, "/queue/add-many", Some(r#"{"files":["a","b"]}"#), Command::QueueAddMany(vec!["a".into(), "b".into()])),
            (Post, "/queue/play-index", Some(r#"{"index":2}"#), Command::QueuePlayIndex(2)),
            (Post, "/queue/remove", Some(r#"{"index":0}"#), Command::QueueRemove(0)),
            (Post, "/queue/clear", None, Command::QueueClear),
            (Get, "/queue", None, Command::Queue),
            (Get, "/version", None, Command::Version),
        ];
        for (method, path, body, expected) in table {
            assert_eq!(routed(method, path, body, None), Ok(expected), "{path}");
        }
    }

    #[test]
    fn malformed_asks_are_answered_not_routed() {
        assert_eq!(routed(Method::Post, "/play", None, None), Err(400), "a body is required");
        assert_eq!(routed(Method::Post, "/play", Some("{not json"), None), Err(400));
        assert_eq!(routed(Method::Post, "/seek", Some(r#"{"position":"soon"}"#), None), Err(400));
        assert_eq!(routed(Method::Get, "/nowhere", None, None), Err(404));
        assert_eq!(routed(Method::Get, "/play", None, None), Err(404), "the method is part of the route");
    }

    #[test]
    fn the_token_gate_spares_only_the_version_probe() {
        assert_eq!(routed(Method::Post, "/pause", None, Some("s3cret")), Err(401));
        assert_eq!(routed(Method::Get, "/status", None, Some("s3cret")), Err(401));
        assert_eq!(routed(Method::Get, "/version", None, Some("s3cret")), Ok(Command::Version));
        // The right token opens every route; a wrong one is the same 401
        // as none.
        let signed = |token: &'static str| -> Result<Command, u16> {
            let req = tiny_http::TestRequest::new()
                .with_method(Method::Post)
                .with_path("/pause")
                .with_header(Header::from_bytes("Host", "127.0.0.1:3333").unwrap())
                .with_header(Header::from_bytes("x-auth-token", token).unwrap());
            match vet(req.into(), "127.0.0.1", 3333, Some("s3cret")) {
                Verdict::Route(_, Ok(cmd)) => Ok(cmd),
                Verdict::Route(_, Err(resp)) | Verdict::Refuse(_, resp) => Err(resp.status_code().0),
                Verdict::Abandoned => unreachable!(),
            }
        };
        assert_eq!(signed("s3cret"), Ok(Command::Pause));
        assert_eq!(signed("s3cre7"), Err(401));
        assert_eq!(signed("s3cret-and-more"), Err(401));
    }

    #[test]
    fn hygiene_refuses_before_reading_a_body() {
        // The wrong Host is a rebinding page, an Origin is a browser, and a
        // body that is not JSON is a form a page could post — each refused
        // unread.
        let refused = |req: tiny_http::TestRequest| -> Option<u16> {
            match vet(req.into(), "127.0.0.1", 3333, None) {
                Verdict::Refuse(_, resp) => Some(resp.status_code().0),
                Verdict::Route(..) => None,
                Verdict::Abandoned => Some(0),
            }
        };
        let host = |h: &'static str| {
            tiny_http::TestRequest::new()
                .with_method(Method::Get)
                .with_path("/status")
                .with_header(Header::from_bytes("Host", h).unwrap())
        };
        assert_eq!(refused(host("evil.example:3333")), Some(403));
        assert_eq!(refused(host("127.0.0.1:3333")), None, "a legitimate Host is routed");
        let origin = tiny_http::TestRequest::new()
            .with_method(Method::Post)
            .with_path("/pause")
            .with_header(Header::from_bytes("Host", "127.0.0.1:3333").unwrap())
            .with_header(Header::from_bytes("Origin", "https://evil.example").unwrap());
        assert_eq!(refused(origin), Some(403));
        let form = tiny_http::TestRequest::new()
            .with_method(Method::Post)
            .with_path("/play")
            .with_header(Header::from_bytes("Host", "127.0.0.1:3333").unwrap())
            .with_header(Header::from_bytes("Content-Type", "text/plain").unwrap())
            .with_body(r#"{"file":"x"}"#);
        assert_eq!(refused(form), Some(415));
    }

    #[test]
    fn hosts_we_answer_to() {
        // Direct hits, loopback aliases, and any address literal. A DNS
        // rebinding page arrives under the attacker's own domain: an
        // address can't be rebound, only a name can, so names must be ours.
        for ok in [
            "127.0.0.1:3333",
            "localhost:3333",
            "LOCALHOST:3333",
            "[::1]:3333",
            "192.168.1.7:3333",
        ] {
            assert!(host_allowed(Some(ok), "127.0.0.1", 3333), "{ok}");
        }
        for bad in [
            "evil.example:3333", // the rebinding case
            "127.0.0.1:9999",    // wrong port
            "127.0.0.1",         // no port means 80
            "127.0.0.1:x",
            "",
        ] {
            assert!(!host_allowed(Some(bad), "127.0.0.1", 3333), "{bad}");
        }
        assert!(!host_allowed(None, "127.0.0.1", 3333), "no Host, no service");
        // Port 80 is the one place a bare Host is legitimate.
        assert!(host_allowed(Some("127.0.0.1"), "127.0.0.1", 80));
        // A named bind answers to that name — and still not to others.
        assert!(host_allowed(Some("jukebox.lan:3333"), "jukebox.lan", 3333));
        assert!(host_allowed(Some("JukeBox.LAN:3333"), "jukebox.lan", 3333));
        assert!(!host_allowed(Some("jukebox.lan:3333"), "127.0.0.1", 3333));
    }

    #[test]
    fn json_is_the_only_body_we_parse() {
        assert!(is_json(Some("application/json")));
        assert!(is_json(Some("Application/JSON; charset=utf-8")));
        assert!(is_json(Some(" application/json ")));
        // The content types a cross-site request may carry without a
        // preflight — accepting any of these would reopen finding #30.
        for simple in ["text/plain", "application/x-www-form-urlencoded", "multipart/form-data"] {
            assert!(!is_json(Some(simple)), "{simple}");
        }
        assert!(!is_json(None));
    }
}
