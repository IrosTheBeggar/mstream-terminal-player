//! Library commands: `login`, `logout`, `info`, `ls`, `search`, `playlists`.
//!
//! These are the exercise harness for the API client (the role `play` served
//! for the streaming engine) and a debugging tool in their own right. The
//! interactive TUI in Phase 4 drives the same client.

use std::io::Read;

use clap::Args;

use crate::api::session::{self, Session};
use crate::api::{ApiError, Client};

/// Server/token overrides shared by every read-only command. Omit both to use
/// the saved session.
#[derive(Args, Clone)]
pub struct ConnArgs {
    /// mStream server base URL (default: the saved session's server)
    #[arg(long, global = true)]
    pub server: Option<String>,

    /// Auth token override (default: the saved session's token)
    #[arg(long, global = true, env = "MSTREAM_TOKEN", hide_env_values = true)]
    pub token: Option<String>,
}

#[derive(Args)]
pub struct LoginArgs {
    /// mStream server base URL, e.g. http://localhost:3000
    #[arg(long)]
    pub server: String,

    /// Username
    #[arg(long, alias = "username")]
    pub user: String,

    /// Password. Prefer MSTREAM_PASSWORD or --password-stdin: an argument here
    /// is visible in the process list and shell history.
    #[arg(long, env = "MSTREAM_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,

    /// Read the password from stdin (first line)
    #[arg(long, conflicts_with = "password")]
    pub password_stdin: bool,
}

#[derive(Args)]
pub struct LsArgs {
    /// Directory to list, e.g. "music/Artist/Album". Omit to list libraries.
    pub path: Option<String>,

    #[command(flatten)]
    pub conn: ConnArgs,
}

#[derive(Args)]
pub struct SearchArgs {
    /// Text to search for across artists, albums, titles, paths and lyrics
    pub query: String,

    #[command(flatten)]
    pub conn: ConnArgs,
}

#[derive(Args)]
pub struct BrowseArgs {
    /// Artist to browse. Omit to list all artists.
    pub artist: Option<String>,

    /// Album to list tracks for (requires an artist).
    #[arg(requires = "artist")]
    pub album: Option<String>,

    /// List every album in the library instead of browsing by artist
    #[arg(long, conflicts_with_all = ["artist", "album"])]
    pub albums: bool,

    /// List genres
    #[arg(long, conflicts_with_all = ["artist", "album", "albums"])]
    pub genres: bool,

    /// List the tracks in one genre
    #[arg(long, value_name = "NAME", conflicts_with_all = ["artist", "album", "albums", "genres"])]
    pub genre: Option<String>,

    /// List recently added tracks
    #[arg(long, conflicts_with_all = ["artist", "album", "albums", "genres", "genre"])]
    pub recent: bool,

    /// How many tracks --recent returns
    #[arg(long, default_value_t = 25, requires = "recent")]
    pub limit: u32,

    #[command(flatten)]
    pub conn: ConnArgs,
}

#[derive(Args)]
pub struct PlaylistArgs {
    /// Playlist to load. Omit to list all playlists.
    pub name: Option<String>,

    #[command(flatten)]
    pub conn: ConnArgs,
}

#[derive(Args)]
pub struct DjArgs {
    /// Seed track (library path). Omit for an unseeded random pick.
    pub seed: Option<String>,

    /// "similar" for sonic similarity, "tempo" for tempo/key matching
    #[arg(long, default_value = "similar")]
    pub mode: String,

    /// How many candidates to show
    #[arg(long, default_value_t = 10)]
    pub count: u32,

    #[command(flatten)]
    pub conn: ConnArgs,
}

/// Show what Auto-DJ would queue next, and why — the scriptable view of what
/// the player does when Auto-DJ is switched on.
pub fn dj(args: DjArgs) -> i32 {
    let client = match connect(&args.conn) {
        Ok(c) => c,
        Err(code) => return code,
    };

    match args.mode.to_ascii_lowercase().as_str() {
        "similar" => {
            let Some(seed) = &args.seed else {
                eprintln!("error: --mode similar needs a seed track");
                return 2;
            };
            match client.similar_tracks(seed, args.count) {
                Ok(None) => {
                    println!("similarity is switched off on this server");
                    println!("(the player falls back to tempo/key matching)");
                    0
                }
                Ok(Some(found)) if found.not_analyzed => {
                    println!("that track hasn't been analysed yet — no embedding to compare");
                    println!("(the player falls back to tempo/key matching)");
                    0
                }
                Ok(Some(found)) if found.results.is_empty() => {
                    println!("(nothing similar found)");
                    0
                }
                Ok(Some(found)) => {
                    for r in found.results {
                        println!("{:.3}  {}", r.similarity, r.filepath);
                    }
                    0
                }
                Err(e) => fail(e),
            }
        }

        "tempo" | "tempo+key" | "bpm" => {
            // Resolve the seed's tags first — they're what the windows and
            // key set are derived from.
            let seed_meta = match &args.seed {
                Some(path) => match client.metadata(path) {
                    Ok(track) => Some(track),
                    Err(e) => return fail(e),
                },
                None => None,
            };

            let mut request = crate::api::types::RandomSongRequest::default();
            if let Some(seed) = &seed_meta {
                println!(
                    "seed: {}  bpm {}  key {}",
                    seed.display_name(),
                    seed.metadata.bpm.map(|b| b.to_string()).unwrap_or("—".into()),
                    seed.metadata.musical_key.as_deref().unwrap_or("—")
                );
                if let Some(bpm) = seed.metadata.bpm {
                    let to_window = |r: crate::dj::BpmRange| crate::api::types::BpmWindow {
                        min: r.min,
                        max: r.max,
                    };
                    request.bpm_ranges =
                        crate::dj::bpm_windows(f64::from(bpm), crate::dj::TIGHT_TOLERANCE)
                            .into_iter()
                            .map(to_window)
                            .collect();
                    request.bpm_ranges_wide =
                        crate::dj::bpm_windows(f64::from(bpm), crate::dj::WIDE_TOLERANCE)
                            .into_iter()
                            .map(to_window)
                            .collect();
                }
                request.musical_keys =
                    crate::dj::compatible_keys(seed.metadata.musical_key.as_deref());

                let windows: Vec<String> = request
                    .bpm_ranges
                    .iter()
                    .map(|w| format!("{:.1}-{:.1}", w.min, w.max))
                    .collect();
                println!(
                    "matching: bpm [{}]  keys [{}]",
                    if windows.is_empty() { "none".into() } else { windows.join(", ") },
                    if request.musical_keys.is_empty() {
                        "none".to_string()
                    } else {
                        request.musical_keys.join(", ")
                    }
                );
            }

            // The endpoint returns one track per call, so ask repeatedly and
            // echo the cursor back the way the player does.
            for _ in 0..args.count {
                match client.random_song(&request) {
                    Ok(response) if response.songs.is_empty() => {
                        println!("(no more matches)");
                        break;
                    }
                    Ok(response) => {
                        for t in &response.songs {
                            println!(
                                "  {}  bpm {}  key {}",
                                t.display_name(),
                                t.metadata.bpm.map(|b| b.to_string()).unwrap_or("—".into()),
                                t.metadata.musical_key.as_deref().unwrap_or("—")
                            );
                        }
                        request.ignore_list = response.ignore_list;
                    }
                    Err(e) => return fail(e),
                }
            }
            0
        }

        other => {
            eprintln!("error: unknown --mode '{other}' — use 'similar' or 'tempo'");
            2
        }
    }
}

/// Turn an `ApiError` into a printed message plus an exit code.
fn fail(e: ApiError) -> i32 {
    eprintln!("error: {e}");
    match e {
        ApiError::Unauthorized => 3,
        _ => 1,
    }
}

fn connect(conn: &ConnArgs) -> Result<Client, i32> {
    Client::resolve(conn.server.as_deref(), conn.token.as_deref()).map_err(fail)
}

// ── login / logout ──────────────────────────────────────────────────────────

pub fn login(args: LoginArgs) -> i32 {
    let password = match resolve_password(&args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("error: {msg}");
            return 2;
        }
    };

    let mut client = match Client::new(&args.server) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };

    if client.is_insecure_remote() {
        eprintln!(
            "warning: {} is plain HTTP — your password and token will cross the network \
             unencrypted",
            args.server
        );
    }

    let resp = match client.login(&args.user, &password) {
        Ok(r) => r,
        Err(ApiError::Unauthorized) => {
            eprintln!("error: login failed — check the username and password");
            return 3;
        }
        Err(e) => return fail(e),
    };

    let session = Session {
        server: client.server(),
        username: Some(args.user.clone()),
        token: Some(resp.token),
    };
    match session::save(&session) {
        Ok(path) => {
            println!("logged in as {} to {}", args.user, session.server);
            if !resp.vpaths.is_empty() {
                println!("libraries: {}", resp.vpaths.join(", "));
            }
            println!("session saved to {}", path.display());
            0
        }
        Err(e) => {
            eprintln!("error: logged in, but could not save the session: {e}");
            1
        }
    }
}

fn resolve_password(args: &LoginArgs) -> Result<String, String> {
    if args.password_stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("could not read password from stdin: {e}"))?;
        let pw = buf.lines().next().unwrap_or("").to_string();
        if pw.is_empty() {
            return Err("no password on stdin".to_string());
        }
        return Ok(pw);
    }
    args.password.clone().ok_or_else(|| {
        "no password given — set MSTREAM_PASSWORD, or pipe it in with --password-stdin".to_string()
    })
}

pub fn logout() -> i32 {
    match session::clear() {
        Ok(true) => {
            println!("session cleared");
            0
        }
        Ok(false) => {
            println!("no saved session");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

// ── read-only commands ──────────────────────────────────────────────────────

pub fn info(conn: ConnArgs) -> i32 {
    let client = match connect(&conn) {
        Ok(c) => c,
        Err(code) => return code,
    };
    let ping = match client.ping() {
        Ok(p) => p,
        Err(e) => return fail(e),
    };

    println!("server:    {}", client.server());
    println!("auth:      {}", if client.token().is_some() { "token" } else { "none (public mode?)" });
    println!(
        "libraries: {}",
        if ping.vpaths.is_empty() { "(none)".to_string() } else { ping.vpaths.join(", ") }
    );
    if let Some(t) = &ping.transcode {
        let codec = t.default_codec.as_deref().unwrap_or("(unset)");
        let bitrate = t.default_bitrate.as_deref().unwrap_or("(unset)");
        println!("transcode: default codec {codec}, bitrate {bitrate}");
        if codec.eq_ignore_ascii_case("opus") {
            println!(
                "           note: this player cannot decode opus, so it always requests \
                 mp3/aac explicitly"
            );
        }
    }
    0
}

pub fn ls(args: LsArgs) -> i32 {
    let client = match connect(&args.conn) {
        Ok(c) => c,
        Err(code) => return code,
    };
    let dir = args.path.unwrap_or_default();
    let listing = match client.file_explorer(&dir) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };

    // Print paths in the form `play` accepts, so results are copy-pasteable.
    let prefix = listing.path.trim_matches('/');
    let qualify = |name: &str| {
        if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        }
    };

    for d in &listing.directories {
        println!("{:<6} {}/", "dir", qualify(&d.name));
    }
    for f in &listing.files {
        println!("{:<6} {}", f.kind.as_deref().unwrap_or("file"), qualify(&f.name));
    }
    if listing.directories.is_empty() && listing.files.is_empty() {
        println!("(empty)");
    }
    0
}

pub fn search(args: SearchArgs) -> i32 {
    let client = match connect(&args.conn) {
        Ok(c) => c,
        Err(code) => return code,
    };
    let results = match client.search(&args.query) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };

    if results.is_empty() {
        println!("no matches for {:?}", args.query);
        return 0;
    }

    if !results.artists.is_empty() {
        println!("artists:");
        for a in &results.artists {
            println!("  {}", a.name);
        }
    }
    if !results.albums.is_empty() {
        println!("albums:");
        for a in &results.albums {
            println!("  {}", a.name);
        }
    }

    let mut tracks: Vec<_> = results.title.iter().chain(results.files.iter()).collect();
    tracks.sort_by(|a, b| a.filepath.cmp(&b.filepath));
    tracks.dedup_by(|a, b| a.filepath == b.filepath);
    if !tracks.is_empty() {
        println!("tracks:");
        for t in tracks {
            let dur = t
                .metadata
                .duration
                .map(|d| format!("  [{}]", fmt_duration(d)))
                .unwrap_or_default();
            println!("  {}{}", t.filepath, dur);
        }
    }
    if !results.lyrics.is_empty() {
        println!("lyrics matches:");
        for t in &results.lyrics {
            println!("  {}", t.filepath);
        }
    }
    0
}

/// Walk the tag-based library view: artists → albums → tracks.
pub fn browse(args: BrowseArgs) -> i32 {
    let client = match connect(&args.conn) {
        Ok(c) => c,
        Err(code) => return code,
    };

    let print_albums = |albums: Vec<crate::api::types::Album>| {
        if albums.is_empty() {
            println!("(no albums)");
            return;
        }
        for a in albums {
            let name = a.name.as_deref().unwrap_or("(untitled)");
            let year = a.year.map(|y| format!(" ({y})")).unwrap_or_default();
            match a.artist.as_deref() {
                Some(artist) if !artist.is_empty() => println!("{artist} — {name}{year}"),
                _ => println!("{name}{year}"),
            }
        }
    };

    if args.genres {
        return match client.genres() {
            Ok(genres) if genres.is_empty() => {
                println!("(no genres — are the files tagged?)");
                0
            }
            Ok(genres) => {
                for g in genres {
                    match g.track_count {
                        Some(n) => println!("{} ({n})", g.name),
                        None => println!("{}", g.name),
                    }
                }
                0
            }
            Err(e) => fail(e),
        };
    }

    if let Some(genre) = &args.genre {
        return match client.genre_songs(genre) {
            Ok(tracks) => print_tracks(&tracks),
            Err(e) => fail(e),
        };
    }

    if args.recent {
        return match client.recently_added(args.limit) {
            Ok(tracks) => print_tracks(&tracks),
            Err(e) => fail(e),
        };
    }

    match (args.albums, args.artist.as_deref(), args.album.as_deref()) {
        // Every album in the library.
        (true, _, _) => match client.albums() {
            Ok(albums) => {
                print_albums(albums);
                0
            }
            Err(e) => fail(e),
        },

        // Tracks of one album.
        (_, Some(artist), Some(album)) => match client.album_songs(album, Some(artist)) {
            Ok(tracks) if tracks.is_empty() => {
                println!("(no tracks)");
                0
            }
            Ok(tracks) => {
                for t in tracks {
                    let num = t
                        .metadata
                        .track
                        .map(|n| format!("{n:>2}. "))
                        .unwrap_or_else(|| "    ".to_string());
                    let dur = t
                        .metadata
                        .duration
                        .map(|d| format!("  [{}]", fmt_duration(d)))
                        .unwrap_or_default();
                    println!("{num}{}{dur}", t.display_name());
                    println!("    {}", t.filepath);
                }
                0
            }
            Err(e) => fail(e),
        },

        // One artist's albums.
        (_, Some(artist), None) => match client.artist_albums(artist) {
            Ok(albums) => {
                print_albums(albums);
                0
            }
            Err(e) => fail(e),
        },

        // All artists.
        (_, None, _) => match client.artists() {
            Ok(artists) if artists.is_empty() => {
                println!("(no artists — is the library scanned?)");
                0
            }
            Ok(artists) => {
                for a in artists {
                    println!("{a}");
                }
                0
            }
            Err(e) => fail(e),
        },
    }
}

pub fn playlists(args: PlaylistArgs) -> i32 {
    let client = match connect(&args.conn) {
        Ok(c) => c,
        Err(code) => return code,
    };

    match args.name {
        None => match client.playlists() {
            Ok(list) if list.is_empty() => {
                println!("(no playlists)");
                0
            }
            Ok(list) => {
                for p in list {
                    println!("{}", p.name);
                }
                0
            }
            Err(e) => fail(e),
        },
        Some(name) => match client.playlist_load(&name) {
            Ok(tracks) => {
                for t in tracks {
                    let dur = t
                        .metadata
                        .duration
                        .map(|d| format!("  [{}]", fmt_duration(d)))
                        .unwrap_or_default();
                    println!("{}{}", t.filepath, dur);
                }
                0
            }
            Err(e) => fail(e),
        },
    }
}

/// Print a track list as "display name [duration]" followed by its path.
fn print_tracks(tracks: &[crate::api::types::Track]) -> i32 {
    if tracks.is_empty() {
        println!("(no tracks)");
        return 0;
    }
    for t in tracks {
        let dur = t
            .metadata
            .duration
            .map(|d| format!("  [{}]", fmt_duration(d)))
            .unwrap_or_default();
        println!("{}{dur}", t.display_name());
        println!("    {}", t.filepath);
    }
    0
}

pub fn fmt_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "--:--".to_string();
    }
    let total = seconds.round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations() {
        assert_eq!(fmt_duration(0.0), "0:00");
        assert_eq!(fmt_duration(59.6), "1:00");
        assert_eq!(fmt_duration(60.029), "1:00");
        assert_eq!(fmt_duration(3725.0), "62:05");
        assert_eq!(fmt_duration(f64::NAN), "--:--");
        assert_eq!(fmt_duration(-1.0), "--:--");
    }
}
