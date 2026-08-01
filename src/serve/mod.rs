//! Headless jukebox mode: the JSON control API over HTTP.
//!
//! Route-for-route and shape-for-shape compatible with mStream's
//! rust-server-audio (control API v1 — see PLAN.md). Additions are additive
//! only: GET /version, optional x-auth-token auth, configurable bind address,
//! and an --exit-with-parent stdin watchdog.

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
    let mut body = String::new();
    request.as_reader().read_to_string(&mut body).ok()?;
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

// ── Main loop ───────────────────────────────────────────────────────────────

pub fn run(opts: ServeOptions) -> Result<(), String> {
    let engine = Engine::new().map_err(|e| format!("could not initialize audio output: {}", e))?;

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
        let mut request = match request {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(_) => continue,
        };

        let method = request.method().clone();
        let path = request.url().to_string();

        // Auth gate: everything except GET /version when a token is set.
        if let Some(expected) = &opts.auth_token {
            let is_version = method == Method::Get && path == "/version";
            if !is_version {
                let supplied = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("x-auth-token"))
                    .map(|h| h.value.as_str().to_string());
                let authorized =
                    supplied.as_deref().map(|t| constant_time_eq(expected, t)).unwrap_or(false);
                if !authorized {
                    let _ = request.respond(error_response_with_status("Unauthorized", 401));
                    continue;
                }
            }
        }

        let body = read_body(&mut request);

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
