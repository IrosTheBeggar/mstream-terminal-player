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

type Resp = Response<std::io::Cursor<Vec<u8>>>;

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
fn respond_unread(request: tiny_http::Request, response: Resp) {
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

        let request = server.recv_timeout(Duration::from_millis(250));
        let request = match request {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(_) => continue,
        };

        let method = request.method().clone();
        let path = request.url().to_string();

        // Hygiene before anything else answers: refuse what we can't
        // account for (finding #28) and what a browser could be driving
        // (finding #30). Nothing here reads the body.
        if request.body_length().is_none() && header_value(&request, "transfer-encoding").is_some()
        {
            respond_unread(request, error_response_with_status("Length required", 411));
            continue;
        }
        if !host_allowed(header_value(&request, "host"), &opts.host, opts.port) {
            respond_unread(
                request,
                error_response_with_status("Forbidden: unrecognized Host", 403),
            );
            continue;
        }

        // Auth gate: everything except GET /version when a token is set.
        if let Some(expected) = &opts.auth_token {
            let is_version = method == Method::Get && path == "/version";
            if !is_version {
                let supplied = header_value(&request, "x-auth-token");
                let authorized =
                    supplied.map(|t| constant_time_eq(expected, t)).unwrap_or(false);
                if !authorized {
                    respond_unread(request, error_response_with_status("Unauthorized", 401));
                    continue;
                }
            }
        }

        if request.body_length().unwrap_or(0) > BODY_CAP {
            respond_unread(request, error_response_with_status("Payload too large", 413));
            continue;
        }
        if method == Method::Post {
            // A browser is the only client that announces itself with an
            // Origin header, and no page anywhere has a legitimate call
            // here — this is what stops a visited site driving the jukebox.
            if header_value(&request, "origin").is_some() {
                respond_unread(
                    request,
                    error_response_with_status("Forbidden: cross-origin request", 403),
                );
                continue;
            }
            if request.body_length().unwrap_or(0) > 0
                && !is_json(header_value(&request, "content-type"))
            {
                respond_unread(
                    request,
                    error_response_with_status("Content-Type must be application/json", 415),
                );
                continue;
            }
        }

        let Some((request, body)) = take_body(request) else {
            // The helper still owns it and will answer for itself.
            continue;
        };

        let response = match (method, path.as_str()) {
            (Method::Post, "/play") => match body.as_deref() {
                Some(b) => match parse::<PlayRequest>(b) {
                    Ok(req) => match engine.play_source(req.file, None) {
                        Ok(()) => ok_resp(),
                        Err(e) => engine_error(e, "Failed to play file"),
                    },
                    Err(resp) => resp,
                },
                None => error_response("Missing request body"),
            },

            (Method::Post, "/pause") => {
                engine.pause();
                ok_resp()
            }
            (Method::Post, "/resume") => {
                engine.resume();
                ok_resp()
            }
            (Method::Post, "/stop") => {
                engine.stop();
                ok_resp()
            }

            (Method::Post, "/next") => match engine.next_manual() {
                Ok(()) => ok_resp(),
                Err(e) => engine_error(e, "Failed to play next track"),
            },
            (Method::Post, "/previous") => match engine.previous_manual() {
                Ok(()) => ok_resp(),
                Err(e) => engine_error(e, "Failed to play previous track"),
            },

            (Method::Post, "/seek") => match body.as_deref() {
                Some(b) => match parse::<SeekRequest>(b) {
                    Ok(req) => match engine.seek(req.position) {
                        Ok(()) => ok_resp(),
                        Err(e) => engine_error(e, "Seek failed"),
                    },
                    Err(resp) => resp,
                },
                None => error_response("Missing request body"),
            },

            (Method::Post, "/volume") => match body.as_deref() {
                Some(b) => match parse::<VolumeRequest>(b) {
                    Ok(req) => {
                        engine.set_volume(req.volume);
                        ok_resp()
                    }
                    Err(resp) => resp,
                },
                None => error_response("Missing request body"),
            },

            (Method::Post, "/shuffle") => match body.as_deref() {
                Some(b) => match parse::<BoolRequest>(b) {
                    Ok(req) => {
                        engine.set_shuffle(req.value);
                        ok_resp()
                    }
                    Err(resp) => resp,
                },
                None => error_response("Missing request body"),
            },

            (Method::Post, "/loop") => {
                let mode = engine.cycle_loop();
                json_response(&serde_json::json!({ "ok": true, "loop_mode": mode.as_str() }))
            }

            (Method::Get, "/status") => json_response(&engine.status()),

            (Method::Post, "/queue/add") => match body.as_deref() {
                Some(b) => match parse::<PlayRequest>(b) {
                    Ok(req) => {
                        engine.queue_add(req.file);
                        ok_resp()
                    }
                    Err(resp) => resp,
                },
                None => error_response("Missing request body"),
            },

            (Method::Post, "/queue/add-many") => match body.as_deref() {
                Some(b) => match parse::<AddManyRequest>(b) {
                    Ok(req) => {
                        engine.queue_add_many(req.files);
                        ok_resp()
                    }
                    Err(resp) => resp,
                },
                None => error_response("Missing request body"),
            },

            (Method::Post, "/queue/play-index") => match body.as_deref() {
                Some(b) => match parse::<IndexRequest>(b) {
                    Ok(req) => match engine.queue_play_index(req.index) {
                        Ok(()) => ok_resp(),
                        Err(e) => engine_error(e, "Failed to play track at index"),
                    },
                    Err(resp) => resp,
                },
                None => error_response("Missing request body"),
            },

            (Method::Post, "/queue/remove") => match body.as_deref() {
                Some(b) => match parse::<IndexRequest>(b) {
                    Ok(req) => match engine.queue_remove(req.index) {
                        Ok(()) => ok_resp(),
                        Err(e) => engine_error(e, "Failed to remove track"),
                    },
                    Err(resp) => resp,
                },
                None => error_response("Missing request body"),
            },

            (Method::Post, "/queue/clear") => {
                engine.queue_clear();
                ok_resp()
            }

            (Method::Get, "/queue") => json_response(&engine.queue_snapshot()),

            (Method::Get, "/version") => json_response(&serde_json::json!({
                "name": "mstream-player",
                "version": env!("CARGO_PKG_VERSION"),
                "apiVersion": API_VERSION,
            })),

            _ => error_response_with_status("Not found", 404),
        };

        let _ = request.respond(response);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
