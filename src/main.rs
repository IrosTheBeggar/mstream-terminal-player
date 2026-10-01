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
mod cmd_viz;
#[cfg(not(target_arch = "wasm32"))]
mod replay;
#[cfg(not(target_arch = "wasm32"))]
mod runtime;
#[cfg(not(target_arch = "wasm32"))]
mod serve;
/// The visualizer presets: their format, their GLSL, their audio texture,
/// and the GPU renderer that draws them. Native only — the browser build has
/// no GPU path yet, and none of this belongs in it until it does.
#[cfg(not(target_arch = "wasm32"))]
mod shader;
/// The wgpu instance and adapter every window and probe draws with: on
/// Windows, DX12 or Vulkan alone, never GL.
#[cfg(not(target_arch = "wasm32"))]
mod gpu_pick;
/// The visualizer's window: a child process of the player (PLAN.md, Phase
/// 11.1; docs/ux-contracts/visualizer-window.md).
#[cfg(not(target_arch = "wasm32"))]
mod viz_window;
#[cfg(not(target_arch = "wasm32"))]
mod gui;
#[cfg(not(target_arch = "wasm32"))]
mod instance;
#[cfg(not(target_arch = "wasm32"))]
mod kit;
#[cfg(not(target_arch = "wasm32"))]
mod setup;
#[cfg(not(target_arch = "wasm32"))]
mod admin;
/// The desktop flavour's launch contract: what an empty argv opens, its
/// default instance lock, and the console a double-click leaves behind.
#[cfg(all(feature = "desktop", not(target_arch = "wasm32")))]
mod desktop;

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
/// stays native. The shared items are kept identical; the parse is a
/// refusal.
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

        pub fn from_kind(kind: u8) -> TunnelPath {
            match kind {
                1 => TunnelPath::Direct,
                2 => TunnelPath::Relay,
                _ => TunnelPath::Reconnecting,
            }
        }
    }

    /// What a tunnel's supervisor is doing, as the shared client reports it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TunnelStatus {
        Connecting,
        Connected,
        Reconnecting,
        Rejected,
        Down,
    }

    impl TunnelStatus {
        pub fn from_code(code: u8) -> TunnelStatus {
            match code {
                0 => TunnelStatus::Connecting,
                1 => TunnelStatus::Connected,
                2 => TunnelStatus::Reconnecting,
                3 => TunnelStatus::Rejected,
                _ => TunnelStatus::Down,
            }
        }
    }

    pub fn is_tunnel_id(server: &str) -> bool {
        server.starts_with(TUNNEL_ID_PREFIX)
    }

    pub fn local_url(port: u16) -> String {
        format!("http://127.0.0.1:{port}")
    }

    /// The identity a code names; the browser cannot dial one, and it
    /// cannot read a ticket either, so a pasted code is refused here.
    pub struct PairingCode;

    impl PairingCode {
        pub fn server_id(&self) -> String {
            String::new()
        }
    }

    pub fn parse_code(_raw: &str) -> Result<PairingCode, String> {
        Err("Quick Connect needs the native player — the tunnel is iroh, not HTTP".to_string())
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

// The locale table, embedded at compile time from locales/*.yml: the
// wizard's, the GUI's, and the shared App's own notes — every target.
// Crate root because t!() resolves crate::_rust_i18n_translate.
rust_i18n::i18n!("locales", fallback = "en");

#[cfg(not(target_arch = "wasm32"))]
use clap::{Args, Parser, Subcommand};

/// What `-V` and `--version` print after the name: the first line is
/// `mstream-player X.Y.Z` exactly in every build, because mStream's
/// launcher probes it with a prefix-anchored pattern; a build with the
/// window (the desktop flavour) adds a second line saying so. The version
/// itself rather than clap's `long_version`, which only `--version` prints
/// (`-V` is clap's short form): both flags answer the same.
#[cfg(not(target_arch = "wasm32"))]
const VERSION: &str = if cfg!(feature = "window") {
    concat!(env!("CARGO_PKG_VERSION"), "\nfeatures: window")
} else {
    env!("CARGO_PKG_VERSION")
};

#[cfg(not(target_arch = "wasm32"))]
#[derive(Parser)]
#[command(
    name = "mstream-player",
    version = VERSION,
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
    // A bare `mstream-player` opens this in the terminal flavour; the
    // desktop flavour opens the GUI's window instead, unless no window can
    // be expected (desktop.rs).
    #[cfg_attr(
        not(feature = "desktop"),
        doc = "Launch the interactive terminal player (the default)"
    )]
    #[cfg_attr(
        feature = "desktop",
        doc = "Launch the interactive terminal player (the default where no window can open)"
    )]
    Tui(TuiArgs),
    /// Launch the GUI player — the mouse-first surface the installers open:
    /// the library rooms, the queue, Auto DJ, servers and tunnels, Now Playing
    Gui(GuiArgs),
    /// Run the headless server-audio engine (jukebox mode)
    Serve(ServeArgs),
    /// Play one source and exit — end-to-end streaming/seek smoke test
    Play(cmd_play::PlayArgs),
    /// First-run setup for a fresh mStream server: folders, login, extras
    Setup(setup::SetupArgs),
    /// Manage the server: libraries, discovery, federation, backups, torrents, users (needs an admin session)
    Admin(admin::AdminArgs),
    /// Your listening on the server: the period's totals, top tracks, the log (mStream 6.27 and up)
    Stats(admin::StatsArgs),
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
    /// Draw the visualizer presets on this machine's GPU (diagnostic)
    VizProbe(cmd_viz::VizProbeArgs),
    /// The visualizer's window — the player opens it as a child process
    #[command(hide = true)]
    VizWindow(viz_window::WindowArgs),
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
struct TuiArgs {
    #[command(flatten)]
    conn: cmd_library::ConnArgs,

    /// The server this player was installed beside — the installers'
    /// launcher passes it. The entry is seeded as the default on first boot
    /// and can never be removed from here; other servers come and go as
    /// usual. A launch property: nothing is persisted about the mode.
    #[arg(long, value_name = "URL")]
    bundled_server: Option<String>,

    /// The launcher passes it beside `--bundled-server` (multi-server
    /// contract, clause 57); the player has nothing to do with it yet.
    #[arg(long, hide = true)]
    same_machine: bool,

    /// The launcher's instance lock: an exclusive lock on this file for the
    /// player's lifetime, so the tray never opens a second desktop player
    /// beside one that is open (instance.rs). A player started by hand
    /// passes nothing and is not counted.
    #[arg(long, hide = true, value_name = "PATH")]
    instance_lock: Option<std::path::PathBuf>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Args)]
struct GuiArgs {
    #[command(flatten)]
    conn: cmd_library::ConnArgs,

    /// The server this player was installed beside — the installers'
    /// launcher passes it. The entry is seeded as the default on first boot
    /// and can never be removed from here; other servers come and go as
    /// usual. A launch property: nothing is persisted about the mode.
    #[arg(long, value_name = "URL")]
    bundled_server: Option<String>,

    /// Open with a torrent — a .torrent file's path or a magnet link — the
    /// way the OS hands one to the app it registered for them. The GUI
    /// asks whether to add it to the server or hand it to another app
    /// (Settings › Torrents decides whether it keeps asking).
    #[arg(long, value_name = "FILE-OR-MAGNET")]
    torrent: Option<String>,

    /// Host the server-audio control API on this loopback port
    /// (gui/control.rs): the launcher passes the server's configured
    /// engine port when server audio is on, so the desktop player IS the
    /// engine the web remote drives. The port and a fresh token are
    /// published in the instance lock's sidecar.
    #[arg(long, hide = true, value_name = "PORT")]
    serve_port: Option<u16>,

    /// The launcher passes it beside `--bundled-server` (multi-server
    /// contract, clause 57); the player has nothing to do with it yet.
    #[arg(long, hide = true)]
    same_machine: bool,

    /// The launcher's instance lock: an exclusive lock on this file for the
    /// player's lifetime, so the tray never opens a second desktop player
    /// beside one that is open (instance.rs). A player started by hand
    /// passes nothing and is not counted.
    #[arg(long, hide = true, value_name = "PATH")]
    instance_lock: Option<std::path::PathBuf>,

    /// The spike's own window: the GUI in a native window through
    /// ratatui-wgpu instead of this terminal (gui/window/) — the real
    /// player, workers and all, with the window's keys, pointer, wheel and
    /// IME translated into the GUI's own events. Only in a build with the
    /// `window` feature (the desktop releases): the terminal releases'
    /// CLI is v0.9.0's, where `gui --window` is a usage error. In the
    /// desktop flavour a bare `mstream-player` opens this window too.
    #[cfg(feature = "window")]
    #[arg(long, hide = true)]
    window: bool,
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

/// A token for the control face: 128 random bits as hex. It lives only in
/// the sidecar (the launcher's data home, the user's own) and in the
/// requests the server signs with it.
#[cfg(not(target_arch = "wasm32"))]
fn fresh_token() -> String {
    format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
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
    // The desktop flavour's empty argv (desktop.rs): the GUI's window, when
    // one can be expected — decided before clap, from the argv and the
    // environment alone, so that on Windows the console Explorer made for a
    // double-click is let go before anything else happens. An explicit argv
    // parses exactly as it does in the terminal flavour.
    #[cfg(feature = "desktop")]
    let (cli, desktop_window) = {
        let args = desktop::launch_argv(std::env::args_os().collect());
        let window = args.len() <= 1 && desktop::window_expected_here();
        #[cfg(windows)]
        if window {
            desktop::leave_own_console();
        }
        (Cli::parse_from(args), window)
    };
    #[cfg(not(feature = "desktop"))]
    let (cli, desktop_window) = (Cli::parse(), false);
    // Its default instance lock, where the launcher's flag would name one.
    #[cfg(feature = "desktop")]
    let default_lock = desktop_window.then(desktop::default_lock).flatten();
    #[cfg(not(feature = "desktop"))]
    let default_lock: Option<std::path::PathBuf> = None;

    // One desktop player per install, settled before anything else starts:
    // the launcher's instance lock (instance.rs). A second player finds it
    // held, says so and leaves — before the log, the spool sweep or the
    // terminal are touched, so the window it was opened in closes at once.
    let (lock_path, face) = match &cli.command {
        Some(Command::Tui(args)) => (args.instance_lock.as_deref(), "tui"),
        Some(Command::Gui(args)) => (args.instance_lock.as_deref(), "gui"),
        None if desktop_window => (default_lock.as_deref(), "gui"),
        _ => (None, ""),
    };
    // Whether this run draws in its own window: the sidecar says so, since a
    // launcher focuses a window and a terminal differently.
    let window = match &cli.command {
        #[cfg(feature = "window")]
        Some(Command::Gui(args)) => args.window,
        None => desktop_window,
        _ => false,
    };
    // The control face's port and token, minted here so the sidecar can
    // publish them with the claim (the face itself binds later, in the GUI).
    let control = match &cli.command {
        Some(Command::Gui(args)) => args.serve_port.map(|port| (port, fresh_token())),
        _ => None,
    };
    let control_claim = control.as_ref().map(|(port, token)| (*port, token.as_str()));
    let mut instance = match instance::claim(lock_path, face, window, control_claim) {
        Ok(instance::Claim::Held(held)) => Some(held),
        Ok(instance::Claim::Unlocked) => None,
        Ok(instance::Claim::Taken(who)) => {
            let line = instance::already_open_line(who.as_ref());
            println!("{line}");
            // A double-clicked app's stdout reaches nobody; the debug log,
            // when one is on, keeps the line. (Focusing the holder is a
            // later step.) The launcher's flag keeps v0.9.0's stdout alone.
            #[cfg(feature = "desktop")]
            if desktop_window {
                desktop::log_refusal(&line);
            }
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("warning: instance lock unavailable ({e}) - continuing without it");
            None
        }
    };

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
        // The faces return their exit code so the instance lock's sidecar
        // is removed on the way out (process::exit runs no destructors).
        (Some(Command::Tui(args)), _) => {
            let code = tui::run(args.conn.server, args.conn.token, args.bundled_server);
            drop(instance);
            std::process::exit(code);
        }
        (Some(Command::Gui(args)), _) => {
            // `window` is the flag where the window exists (GuiArgs::window),
            // false where it does not.
            let code = gui::run(
                args.conn.server,
                args.conn.token,
                args.torrent,
                args.bundled_server,
                control.map(|(port, token)| gui::control::Face { port, token }),
                window,
                // The window takes the lock to drop at its own teardown
                // (gui::run says why); the terminal leaves it for here.
                &mut instance,
            );
            drop(instance);
            std::process::exit(code);
        }
        (Some(Command::Play(args)), _) => std::process::exit(cmd_play::run(args)),
        (Some(Command::Setup(args)), _) => std::process::exit(setup::run(args)),
        (Some(Command::Admin(args)), _) => std::process::exit(admin::run(args)),
        (Some(Command::Stats(args)), _) => std::process::exit(admin::run_stats(args)),
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
        (Some(Command::VizProbe(args)), _) => std::process::exit(cmd_viz::run(args)),
        (Some(Command::VizWindow(args)), _) => std::process::exit(viz_window::run(args)),
        (Some(Command::Replay(args)), _) => std::process::exit(replay::run(args)),
        (Some(Command::Keys), _) => {
            // The bindings in force, not the built-in ones: someone asking
            // what their keys are wants the answer for their config.
            let start = tui::startup(None, None, None);
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
        // The desktop flavour's empty argv: what `gui --window` opens, with
        // nothing else asked for — no server override, no torrent, no
        // bundled server, no control face — under its default lock.
        #[cfg(feature = "desktop")]
        (None, None) if desktop_window => {
            let code = gui::run(None, None, None, None, None, true, &mut instance);
            drop(instance);
            std::process::exit(code);
        }
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
        // Bare `mstream-player` launches the player: the TUI in the
        // terminal flavour, and in the desktop one where no window can be
        // expected (the window's arm above takes every other).
        None => std::process::exit(tui::run(None, None, None)),
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

    /// The two flavours' CLIs (Cargo.toml's [features]): the terminal
    /// releases have no `gui --window`, so asking for one is clap's usage
    /// error, as on v0.9.0; a build with the window parses it.
    #[test]
    fn gui_window_exists_only_where_the_window_does() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
        let parsed = Cli::try_parse_from(["mstream-player", "gui", "--window"]);
        #[cfg(not(feature = "window"))]
        {
            let err = parsed.err().expect("the terminal flavour has no --window");
            assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
            assert_eq!(err.exit_code(), 2);
        }
        #[cfg(feature = "window")]
        {
            let Some(Command::Gui(args)) = parsed.expect("the window flavour parses it").command
            else {
                panic!("`gui --window` is the gui command");
            };
            assert!(args.window);
            let Some(Command::Gui(args)) =
                Cli::try_parse_from(["mstream-player", "gui"]).unwrap().command
            else {
                panic!("`gui` is the gui command");
            };
            assert!(!args.window, "the terminal is still the default face");
        }
    }

    /// mStream's launcher reads the first line of `--version` with a
    /// prefix-anchored pattern, so it is `mstream-player X.Y.Z` exactly in
    /// both flavours; a build with the window says so on a second line, and
    /// `-V` answers the same as `--version`.
    #[test]
    fn the_version_line_is_the_launchers_and_the_window_adds_one() {
        use clap::CommandFactory;
        let first = format!("mstream-player {}", env!("CARGO_PKG_VERSION"));
        for flag in ["-V", "--version"] {
            let err = Cli::try_parse_from(["mstream-player", flag]).err().expect("it prints");
            assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion, "{flag}");
            assert_eq!(err.exit_code(), 0);
            let text = err.to_string();
            let mut lines = text.lines();
            assert_eq!(lines.next(), Some(first.as_str()), "{flag}: {text:?}");
            #[cfg(feature = "window")]
            assert_eq!(lines.next(), Some("features: window"), "{flag}: {text:?}");
            assert_eq!(lines.next(), None, "{flag}: {text:?}");
        }
        let rendered = Cli::command().render_version();
        assert_eq!(rendered.lines().next(), Some(first.as_str()));
    }

    /// The flavours' one difference is what an empty argv parses to being
    /// read differently by main; the parse itself is the same, and a
    /// Finder launch's process serial number is an empty argv.
    #[test]
    fn an_empty_argv_parses_to_no_command() {
        let bare = Cli::try_parse_from(["mstream-player"]).unwrap();
        assert!(bare.command.is_none() && bare.port.is_none());
        #[cfg(feature = "desktop")]
        {
            let argv = ["mstream-player", "-psn_0_4567"].map(std::ffi::OsString::from).to_vec();
            let finder = Cli::try_parse_from(desktop::launch_argv(argv)).unwrap();
            assert!(finder.command.is_none() && finder.port.is_none());
        }
        // Without the rule (the terminal flavour) it is clap's error, as on
        // v0.9.0.
        #[cfg(not(feature = "desktop"))]
        assert!(Cli::try_parse_from(["mstream-player", "-psn_0_4567"]).is_err());
    }
}
