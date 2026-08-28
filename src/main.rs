// On wasm the whole native half — config persistence, the HTTP client's
// response shapes, the Auto-DJ math — is compiled (the types are shared) but
// never called, and dead-code analysis would name every function of it. The
// native build keeps the lint; it's the one where dead code means something.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

mod advance;
mod api;
mod clock;
mod config;
mod console;
mod dj;
mod input;
/// The debug log. Its file half is native-only inside the module; the
/// level type and switches are shared, because the Settings tab that
/// drives them is drawing code the browser build keeps.
mod logging;
mod player;
mod tui;

#[cfg(not(target_arch = "wasm32"))]
mod cmd_graphics;
#[cfg(not(target_arch = "wasm32"))]
mod cmd_library;
#[cfg(not(target_arch = "wasm32"))]
mod cmd_play;
#[cfg(not(target_arch = "wasm32"))]
mod replay;
#[cfg(not(target_arch = "wasm32"))]
mod runtime;
#[cfg(not(target_arch = "wasm32"))]
mod serve;
#[cfg(not(target_arch = "wasm32"))]
mod gui;
#[cfg(not(target_arch = "wasm32"))]
mod kit;
#[cfg(not(target_arch = "wasm32"))]
mod setup;

/// The browser build (see its module note). Everything below this line that
/// swaps a module out for a hand-written stand-in exists so the App, the
/// drawing code and the worker message types compile unchanged there.
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
mod discovery;
/// Stand-in for [discovery.rs]: the browse machinery is mDNS and can never
/// run in a browser, but the found-server shape appears in the worker Event
/// enum, so the type itself must exist. Kept field-for-field identical.
#[cfg(target_arch = "wasm32")]
mod discovery {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DiscoveredServer {
        pub name: String,
        pub base_url: String,
        pub version: Option<String>,
        pub quick_connect: bool,
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod engine;
/// Only the tap: pure sample buffering the visualizer reads. The rest of the
/// engine is rodio and the streaming pipeline, which stay native — the wasm
/// stub feeds the tap a synthesised signal instead.
#[cfg(target_arch = "wasm32")]
mod engine {
    #[path = "tap.rs"]
    pub(crate) mod tap;

    /// The crossfade prepare thread's name, which the panic-hook check
    /// (worker::panics_are_caught) compares against. No such thread ever
    /// runs here — the name just has to exist to be not-matched.
    pub(crate) const PREPARE_THREAD: &str = "mstream-prepare";
}

#[cfg(not(target_arch = "wasm32"))]
mod quickconnect;
/// Stand-in for the pure items of [quickconnect.rs] that the app logic
/// reaches for (tunnel identities appear in saved-server lists, and the
/// tunnel-path words appear in the header); the tunnel itself is iroh and
/// stays native. Kept line-for-line identical.
#[cfg(target_arch = "wasm32")]
mod quickconnect {
    /// Marks a remembered server as one reached through a tunnel rather than
    /// at a URL. Deliberately not a real scheme: nothing may hand it to an
    /// HTTP client.
    pub const TUNNEL_ID_PREFIX: &str = "mstream+iroh://";

    /// How the tunnel is reaching the server right now. iroh starts a
    /// connection on its relay path and holepunches toward a direct one, so
    /// this can change moments after connecting — and change back when a
    /// network path dies.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TunnelPath {
        /// A holepunched peer-to-peer path carries the traffic.
        Direct,
        /// Traffic bounces through an iroh relay server.
        Relay,
        /// Between tunnels: the last one died and a fresh dial has not won yet.
        Reconnecting,
    }

    impl TunnelPath {
        /// The word the UI shows for this state.
        pub fn label(self) -> &'static str {
            match self {
                TunnelPath::Direct => "direct",
                TunnelPath::Relay => "relay",
                TunnelPath::Reconnecting => "reconnecting…",
            }
        }
    }

    pub fn is_tunnel_id(server: &str) -> bool {
        server.starts_with(TUNNEL_ID_PREFIX)
    }

    /// A tunnel identity in a form worth showing someone, since the raw
    /// endpoint id is a 52-character public key. Anything else is returned
    /// unchanged.
    pub fn display_server(server: &str) -> String {
        match server.strip_prefix(TUNNEL_ID_PREFIX) {
            Some(id) => format!("quick connect · {}", id.chars().take(12).collect::<String>()),
            None => server.to_string(),
        }
    }
}

// The wizard's locale table, embedded at compile time from locales/*.yml.
// Crate root because t!() resolves crate::_rust_i18n_translate.
#[cfg(not(target_arch = "wasm32"))]
rust_i18n::i18n!("locales", fallback = "en");

#[cfg(not(target_arch = "wasm32"))]
use clap::{Args, Parser, Subcommand};

#[cfg(not(target_arch = "wasm32"))]
#[derive(Parser)]
#[command(
    name = "mstream-player",
    version,
    about = "Terminal player and headless server-audio engine for mStream"
)]
struct Cli {
    /// Legacy rust-server-audio compatibility: `mstream-player --port N` is
    /// equivalent to `mstream-player serve --port N`.
    #[arg(long, hide = true)]
    port: Option<u16>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Subcommand)]
enum Command {
    /// Launch the interactive terminal player (the default)
    Tui(cmd_library::ConnArgs),
    /// Launch the GUI player — the mouse-first surface the installers open
    /// (preview: Files browsing and playback, the bottom bar, Settings)
    Gui(cmd_library::ConnArgs),
    /// Run the headless server-audio engine (jukebox mode)
    Serve(ServeArgs),
    /// Play one source and exit — end-to-end streaming/seek smoke test
    Play(cmd_play::PlayArgs),
    /// First-run setup for a fresh mStream server: folders, login, extras
    Setup(setup::SetupArgs),
    /// Show the server's Quick Connect code as a scannable QR page
    Qr(setup::QrArgs),
    /// Authenticate against an mStream server and save the session
    Login(cmd_library::LoginArgs),
    /// Forget the saved session
    Logout,
    /// Show server capabilities and the current session
    Info(cmd_library::ConnArgs),
    /// List a library directory
    Ls(cmd_library::LsArgs),
    /// Browse by tags: artists, an artist's albums, or an album's tracks
    Browse(cmd_library::BrowseArgs),
    /// Search the library
    Search(cmd_library::SearchArgs),
    /// Show what Auto-DJ would play next, and why
    Dj(cmd_library::DjArgs),
    /// Dial a Quick Connect pairing code and probe the tunnel (diagnostic)
    QuickconnectProbe { code: String },
    /// Show whether this terminal can draw covers as pixels (diagnostic)
    GraphicsProbe,
    /// Drive the interactive player from a script (smoke testing)
    Replay(replay::ReplayArgs),
    /// Print the key bindings as a config.toml section, ready to edit
    Keys,
    /// List mStream servers advertising themselves on this network
    Discover {
        /// How long to listen for adverts
        ///
        /// Bounded because the value reaches `Duration::from_secs_f64`, which
        /// panics on a negative, a NaN or anything past what a Duration can
        /// hold — a typo in a flag should print a sentence, not a backtrace.
        #[arg(long, default_value_t = 3.0, value_parser = listening_seconds)]
        seconds: f64,
    },
    /// List playlists, or show one playlist's tracks
    Playlists(cmd_library::PlaylistArgs),
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Args)]
struct ServeArgs {
    /// Port for the JSON control API
    #[arg(long, default_value_t = 3333)]
    port: u16,

    /// Bind address. Loopback by default; 0.0.0.0 restores the old
    /// LAN-exposed rust-server-audio behavior.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Require this token in the x-auth-token header on every route except
    /// GET /version. Prefer the env var so the token stays out of the
    /// process list.
    #[arg(long, env = "MSTREAM_AUDIO_TOKEN", hide_env_values = true)]
    auth_token: Option<String>,

    /// Exit when stdin reaches EOF. Pass this only when the parent process
    /// holds stdin open as a pipe — with an ignored/closed stdin the engine
    /// would exit immediately.
    #[arg(long)]
    exit_with_parent: bool,

    /// Blend each track into the next for this many seconds when one ends
    /// on its own. 0 keeps the hard cut; manual /next stays immediate
    /// either way.
    #[arg(long, default_value_t = 0.0, value_parser = crossfade_seconds)]
    crossfade: f32,

    /// Cross track boundaries sample-tight when no crossfade is set, by
    /// feeding the next track into the playing sink ahead of time.
    #[arg(long)]
    gapless: bool,
}

/// A blend length the engine will accept. Bounded like `listening_seconds`
/// and for the same reason — a typo should print a sentence, not misbehave.
/// Thirty seconds is already past any musical blend; the engine clamps
/// there too, so the flag and the behavior agree.
fn crossfade_seconds(raw: &str) -> Result<f32, String> {
    let seconds: f32 = raw.parse().map_err(|_| format!("'{raw}' is not a number"))?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(format!("'{raw}' is not a length of time"));
    }
    if seconds > 30.0 {
        return Err("that is longer than any blend — try something under 30".to_string());
    }
    Ok(seconds)
}

/// A listening window that `Duration::from_secs_f64` will accept.
///
/// The upper bound is a day rather than the type's limit: past that the flag
/// is a typo, and a browse that never returns looks exactly like a hang.
#[cfg(not(target_arch = "wasm32"))]
fn listening_seconds(raw: &str) -> Result<f64, String> {
    let seconds: f64 = raw.parse().map_err(|_| format!("'{raw}' is not a number"))?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(format!("'{raw}' is not a length of time"));
    }
    if seconds > 86_400.0 {
        return Err("that is longer than a day — try something under 86400".to_string());
    }
    Ok(seconds)
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let cli = Cli::parse();

    // The debug log first, before anything can dial: a subscriber installed
    // after the first connection has already missed the interesting part.
    // The boot line goes to stderr — in the TUI it scrolls away under the
    // alternate screen and comes back in scrollback after quit, and the
    // goodbye line at teardown says it again for whoever missed it.
    //
    // A one-shot subcommand keeps its hands off the default location: see
    // logging::init.
    let run = match &cli.command {
        None | Some(Command::Tui(_)) | Some(Command::Gui(_)) | Some(Command::Serve(_)) => {
            logging::Run::Session
        }
        Some(_) => logging::Run::OneShot,
    };
    if let Some(path) = logging::init(run) {
        eprintln!("logging to {}", path.display());
    }

    // Streaming scratch space (PLAN A1): each playing track spools to a temp
    // file. Decide where those belong before anything can open a stream, and
    // sweep leftovers from killed runs while we're at it. When no cache dir
    // can be resolved the engine falls back to the OS temp dir, so that's
    // where leftovers would be, too.
    let spool_dir = config::spool_dir();
    let sweep = spool_dir.clone().unwrap_or_else(std::env::temp_dir);
    engine::http::clean_spool_dir(&sweep);
    engine::http::set_spool_dir(spool_dir);

    let serve_args = match (cli.command, cli.port) {
        (Some(Command::Tui(conn)), _) => {
            std::process::exit(tui::run(conn.server, conn.token));
        }
        (Some(Command::Gui(conn)), _) => std::process::exit(gui::run(conn.server, conn.token)),
        (Some(Command::Play(args)), _) => std::process::exit(cmd_play::run(args)),
        (Some(Command::Setup(args)), _) => std::process::exit(setup::run(args)),
        (Some(Command::Qr(args)), _) => std::process::exit(setup::run_qr(args)),
        (Some(Command::Login(args)), _) => std::process::exit(cmd_library::login(args)),
        (Some(Command::Logout), _) => std::process::exit(cmd_library::logout()),
        (Some(Command::Info(conn)), _) => std::process::exit(cmd_library::info(conn)),
        (Some(Command::Ls(args)), _) => std::process::exit(cmd_library::ls(args)),
        (Some(Command::Browse(args)), _) => std::process::exit(cmd_library::browse(args)),
        (Some(Command::Search(args)), _) => std::process::exit(cmd_library::search(args)),
        (Some(Command::Dj(args)), _) => std::process::exit(cmd_library::dj(args)),
        (Some(Command::QuickconnectProbe { code }), _) => {
            std::process::exit(quickconnect::probe(&code));
        }
        (Some(Command::GraphicsProbe), _) => std::process::exit(cmd_graphics::run()),
        (Some(Command::Replay(args)), _) => std::process::exit(replay::run(args)),
        (Some(Command::Keys), _) => {
            // The bindings in force, not the built-in ones: someone asking
            // what their keys are wants the answer for their config.
            let start = tui::startup(None, None);
            print!("{}", tui::keymap_for(&start.keys).to_config_toml());
            std::process::exit(0);
        }
        (Some(Command::Discover { seconds }), _) => {
            std::process::exit(discovery::print_found(seconds));
        }
        (Some(Command::Playlists(args)), _) => std::process::exit(cmd_library::playlists(args)),
        (Some(Command::Serve(args)), _) => Some(args),
        // Legacy spawn contract: bare `--port N`.
        (None, Some(port)) => Some(ServeArgs {
            port,
            host: "127.0.0.1".to_string(),
            auth_token: std::env::var("MSTREAM_AUDIO_TOKEN").ok(),
            exit_with_parent: false,
            crossfade: 0.0,
            gapless: false,
        }),
        (None, None) => None,
    };

    match serve_args {
        Some(args) => {
            let opts = serve::ServeOptions {
                host: args.host,
                port: args.port,
                auth_token: args.auth_token,
                exit_with_parent: args.exit_with_parent,
                crossfade: args.crossfade,
                gapless: args.gapless,
            };
            if let Err(e) = serve::run(opts) {
                eprintln!("mstream-player: {e}");
                std::process::exit(1);
            }
        }
        // Bare `mstream-player` launches the player.
        None => std::process::exit(tui::run(None, None)),
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    web::run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listening_window_a_duration_cannot_hold_is_refused_in_words() {
        // Every one of these reached Duration::from_secs_f64 and panicked
        // with a backtrace prompt — from a flag, which is the one place a
        // typo should be answered with a sentence.
        for bad in ["-1", "nan", "-0.5", "1e300", "inf", "banana"] {
            let err = listening_seconds(bad).unwrap_err();
            assert!(!err.is_empty(), "{bad:?} was accepted");
            assert!(!err.contains("panic"), "{bad:?}: {err}");
        }

        assert_eq!(listening_seconds("3").unwrap(), 3.0);
        assert_eq!(listening_seconds("0").unwrap(), 0.0, "an instant browse is a legal ask");
        assert_eq!(listening_seconds("0.5").unwrap(), 0.5);
        // The boundary is inclusive, and everything accepted here is a
        // Duration the browse can actually be handed.
        assert!(listening_seconds("86400").is_ok());
        for good in ["0", "0.5", "3", "86400"] {
            let seconds = listening_seconds(good).unwrap();
            let _ = std::time::Duration::from_secs_f64(seconds);
        }
    }

    #[test]
    fn a_blend_length_is_vetted_the_same_way() {
        for bad in ["-1", "nan", "inf", "31", "1e300", "banana"] {
            let err = crossfade_seconds(bad).unwrap_err();
            assert!(!err.is_empty(), "{bad:?} was accepted");
        }
        assert_eq!(crossfade_seconds("0").unwrap(), 0.0, "off is a legal ask");
        assert_eq!(crossfade_seconds("4.5").unwrap(), 4.5);
        assert!(crossfade_seconds("30").is_ok(), "the boundary is inclusive");
    }
}
