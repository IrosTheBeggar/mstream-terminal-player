//! `mstream-player play` — Phase 2 smoke command.
//!
//! Plays one source (local path, raw URL, or an mStream server + vpath) for a
//! bounded number of seconds, optionally performing a mid-play seek, then
//! exits 0 (PASS) or 2 (FAIL) based on whether playback actually advanced.
//! Built as a scriptable end-to-end test of HTTP streaming and range seeking;
//! it also doubles as a handy debugging tool.

use std::time::{Duration, Instant};

use clap::Args;
use stream_download::http::reqwest::Url;

use crate::engine::http::redact_source;
use crate::engine::Engine;

#[derive(Args)]
pub struct PlayArgs {
    /// Full URL or local file path to play directly
    #[arg(long, conflicts_with_all = ["server", "token", "path"])]
    url: Option<String>,

    /// mStream server base URL, e.g. http://localhost:3000
    #[arg(long, requires = "token", requires = "path")]
    server: Option<String>,

    /// mStream auth token (JWT). Prefer the env var to keep it out of the
    /// process list.
    #[arg(long, env = "MSTREAM_TOKEN")]
    token: Option<String>,

    /// Library-relative path (vpath), e.g. "music/Artist/Album/01.flac"
    path: Option<String>,

    /// Known duration in seconds (skips remote probing)
    #[arg(long)]
    duration: Option<f64>,

    /// Seek to this position (seconds) a few seconds into playback
    #[arg(long)]
    seek_to: Option<f64>,

    /// How long to play before ending the test
    #[arg(long, default_value_t = 12.0)]
    test_seconds: f64,

    #[arg(long, default_value_t = 0.5)]
    volume: f32,
}

/// Build `{server}/media/{vpath}?token={token}` with proper percent-encoding
/// per path segment. Honors a server base URL that itself lives under a
/// subpath (reverse-proxy setups).
pub(crate) fn build_media_url(server: &str, vpath: &str, token: &str) -> Result<String, String> {
    let mut url = Url::parse(server).map_err(|e| format!("invalid --server URL: {}", e))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "invalid --server URL: cannot be a base".to_string())?;
        segments.pop_if_empty();
        segments.push("media");
        for part in vpath.split('/').filter(|p| !p.is_empty()) {
            segments.push(part);
        }
    }
    url.query_pairs_mut().append_pair("token", token);
    Ok(url.to_string())
}

const SEEK_AT_SECS: f64 = 4.0;

pub fn run(args: PlayArgs) -> i32 {
    let target = match (&args.url, &args.server) {
        (Some(u), _) => u.clone(),
        (None, Some(server)) => {
            // clap enforces `requires`, but belt-and-braces for direct calls.
            let (Some(token), Some(path)) = (&args.token, &args.path) else {
                eprintln!("--server requires --token and a vpath argument");
                return 2;
            };
            match build_media_url(server, path, token) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("{}", e);
                    return 2;
                }
            }
        }
        (None, None) => {
            eprintln!("Nothing to play: pass --url <url-or-path>, or --server + --token + <vpath>");
            return 2;
        }
    };

    println!("source: {}", redact_source(&target));

    let engine = match Engine::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("audio init failed: {}", e);
            return 1;
        }
    };
    engine.set_volume(args.volume);

    let open_started = Instant::now();
    if let Err(e) = engine.play_source(target, args.duration) {
        eprintln!("FAIL: {}", e);
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
                println!("-- seeking to {:.1}s --", seek_to);
                match engine.seek(seek_to) {
                    Ok(()) => seek_done = true,
                    Err(e) => {
                        eprintln!("FAIL: seek: {}", e);
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

    // Verdict.
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
                "max position {:.2}s never reached seek target {:.2}s",
                max_position, seek_to
            ));
        }
    } else if max_position < 2.0 {
        failures.push(format!("playback barely advanced (max position {:.2}s)", max_position));
    }

    if failures.is_empty() {
        println!("PASS  (max position {:.2}s{})", max_position, if track_ended { ", track ended" } else { "" });
        0
    } else {
        for f in &failures {
            eprintln!("FAIL: {}", f);
        }
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_encoded_media_urls() {
        let u = build_media_url("http://localhost:3000", "music/Some Artist/Söng #1.flac", "tok123")
            .unwrap();
        assert_eq!(
            u,
            "http://localhost:3000/media/music/Some%20Artist/S%C3%B6ng%20%231.flac?token=tok123"
        );
    }

    #[test]
    fn honors_server_subpath_and_trailing_slash() {
        let u = build_media_url("http://host/mstream/", "lib/a.mp3", "t").unwrap();
        assert_eq!(u, "http://host/mstream/media/lib/a.mp3?token=t");
    }

    #[test]
    fn rejects_bad_server_url() {
        assert!(build_media_url("not a url", "a.mp3", "t").is_err());
    }
}
