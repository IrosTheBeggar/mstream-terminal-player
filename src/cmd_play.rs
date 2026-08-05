//! `mstream-player play` — one-shot playback smoke test and debugging tool.
//!
//! Plays one source (a library path via the saved session, a raw URL, or a
//! local file) for a bounded number of seconds, optionally seeking mid-play,
//! then exits 0 (PASS) or 2 (FAIL) based on whether playback actually
//! advanced. Scriptable end-to-end coverage of HTTP streaming and range
//! seeking.

use std::time::{Duration, Instant};

use clap::Args;

use crate::api::Client;
use crate::api::urls::TranscodeCodec;
use crate::api::types::fmt_duration;
use crate::cmd_library::ConnArgs;
use crate::engine::Engine;
use crate::engine::http::redact_source;

#[derive(Args)]
pub struct PlayArgs {
    /// Library path to play, e.g. "music/Artist/Album/01.flac"
    pub path: Option<String>,

    /// Play this URL or local file directly, skipping the API entirely
    #[arg(long, conflicts_with = "path")]
    pub url: Option<String>,

    /// Ask the server to transcode: mp3 or aac (opus is not decodable here)
    #[arg(long, value_name = "CODEC")]
    pub transcode: Option<TranscodeCodec>,

    /// Bitrate for --transcode, e.g. 192k
    #[arg(long, requires = "transcode")]
    pub bitrate: Option<String>,

    /// Known duration in seconds (skips the metadata lookup)
    #[arg(long)]
    pub duration: Option<f64>,

    /// Seek to this position (seconds) a few seconds into playback
    #[arg(long)]
    pub seek_to: Option<f64>,

    /// How long to play before ending the test
    #[arg(long, default_value_t = 12.0)]
    pub test_seconds: f64,

    #[arg(long, default_value_t = 0.5)]
    pub volume: f32,

    #[command(flatten)]
    pub conn: ConnArgs,
}

const SEEK_AT_SECS: f64 = 4.0;

/// Resolve what to play and (when known) how long it is.
fn resolve_target(args: &PlayArgs) -> Result<(String, Option<f64>), String> {
    if let Some(url) = &args.url {
        return Ok((url.clone(), args.duration));
    }

    let Some(path) = &args.path else {
        return Err(
            "nothing to play: pass a library path, or --url for a raw URL / local file".to_string(),
        );
    };

    let client = Client::resolve(args.conn.server.as_deref(), args.conn.token.as_deref())
        .map_err(|e| e.to_string())?;

    let url = match args.transcode {
        Some(codec) => client
            .transcode_url(path, codec, args.bitrate.as_deref())
            .map_err(|e| e.to_string())?,
        None => client.media_url(path).map_err(|e| e.to_string())?,
    };

    // A duration hint spares the engine a probe fetch. Best effort: an older
    // server, an unscanned file, or a permissions quirk shouldn't block
    // playback.
    let duration = args.duration.or_else(|| match client.metadata(path) {
        Ok(track) => track.metadata.duration,
        Err(e) => {
            eprintln!("note: could not fetch metadata for a duration hint ({e})");
            None
        }
    });

    Ok((url, duration))
}

pub fn run(args: PlayArgs) -> i32 {
    let (target, duration_hint) = match resolve_target(&args) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    println!("source: {}", redact_source(&target));
    if let Some(d) = duration_hint {
        println!("duration hint: {} ({d:.2}s)", fmt_duration(d));
    }

    let engine = match Engine::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("audio init failed: {e}");
            return 1;
        }
    };
    engine.set_volume(args.volume);

    let open_started = Instant::now();
    if let Err(e) = engine.play_source(target, duration_hint) {
        eprintln!("FAIL: {e}");
        return 2;
    }
    println!("opened + decoding after {:.2}s", open_started.elapsed().as_secs_f64());

    let started = Instant::now();
    let mut seek_done = false;
    let mut seek_verified_pos: Option<f64> = None;
    let mut max_position: f64 = 0.0;
    let mut track_ended = false;

    loop {
        std::thread::sleep(Duration::from_millis(500));
        let elapsed = started.elapsed().as_secs_f64();
        let st = engine.status();
        max_position = max_position.max(st.position);

        println!(
            "t={:5.1}s  pos={:6.2}s / {:6.2}s  playing={} vol={:.2}",
            elapsed, st.position, st.duration, st.playing, st.volume
        );

        if let Some(seek_to) = args.seek_to {
            if !seek_done && elapsed >= SEEK_AT_SECS {
                println!("-- seeking to {seek_to:.1}s --");
                match engine.seek(seek_to) {
                    Ok(()) => seek_done = true,
                    Err(e) => {
                        eprintln!("FAIL: seek: {e}");
                        engine.stop();
                        return 2;
                    }
                }
            } else if seek_done && seek_verified_pos.is_none() {
                seek_verified_pos = Some(st.position);
            }
        }

        if elapsed >= args.test_seconds {
            break;
        }
        if !st.playing && !st.paused && elapsed > 2.0 {
            track_ended = true;
            println!("-- track ended --");
            break;
        }
    }

    engine.stop();

    let mut failures: Vec<String> = Vec::new();
    if let Some(seek_to) = args.seek_to {
        match seek_verified_pos {
            Some(p) if p >= seek_to - 0.75 => {}
            Some(p) => failures.push(format!(
                "position after seek was {:.2}s, expected >= {:.2}s",
                p,
                seek_to - 0.75
            )),
            None => {
                if !track_ended {
                    failures.push("seek was never verified".to_string());
                }
            }
        }
        if max_position < seek_to - 0.75 {
            failures.push(format!(
                "max position {max_position:.2}s never reached seek target {seek_to:.2}s"
            ));
        }
    } else if max_position < 2.0 {
        failures.push(format!("playback barely advanced (max position {max_position:.2}s)"));
    }

    if failures.is_empty() {
        println!(
            "PASS  (max position {:.2}s{})",
            max_position,
            if track_ended { ", track ended" } else { "" }
        );
        0
    } else {
        for f in &failures {
            eprintln!("FAIL: {f}");
        }
        2
    }
}
