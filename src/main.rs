mod advance;
mod api;
mod clock;
mod config;
mod console;
mod dj;
mod input;
/// The debug log: the level and its switches, and the file it writes to.
mod logging;
/// Tests over Cargo.toml and Cargo.lock: dependency and profile choices.
#[cfg(test)]
mod manifest;
mod player;
mod tui;

mod cmd_graphics;
mod cmd_library;
mod cmd_play;
mod cmd_viz;
mod runtime;
mod serve;
/// The visualizer presets: their format, their GLSL, their audio texture,
/// and the GPU renderer that draws them.
mod shader;
/// The wgpu instance and adapter every window and probe draws with: on
/// Windows, DX12 or Vulkan alone, never GL, and a driver crash that ends the
/// process instead of hanging it behind WER's dialog.
mod gpu_pick;
/// On Windows, a window's picture put back in place after the graphics stack
/// restarts (a driver reset, or the PC out of connected standby), for the
/// player's window and the visualizer's; its rule is tested on every OS.
#[cfg(any(windows, test))]
mod display_watch;
/// The visualizer's window: a child process of the player (PLAN.md, Phase
/// 11.1; docs/ux-contracts/visualizer-window.md).
mod viz_window;
mod gui;
/// Add from YouTube's pure rules: links, the tag guess, where a song lands,
/// formats, file names and failures (src/ytdl/mod.rs). The GUI is its one
/// surface.
mod ytdl;
mod instance;
mod kit;
mod setup;
mod admin;
/// The desktop flavour's launch contract: what an empty argv opens, its
/// default instance lock, and the console a double-click leaves behind.
#[cfg(feature = "desktop")]
mod desktop;
/// The desktop app's identity: its app id and Windows AppUserModelID,
/// shared with the Windows launcher stub. Only the window's builds.
#[cfg(feature = "window")]
mod identity;
/// The MP3 player (an M5Stack Core2) over USB: `device list`, `device
/// flash`.
mod device;
mod discovery;
mod engine;
mod quickconnect;

// The locale table, embedded at compile time from locales/*.yml: the
// wizard's, the GUI's, and the shared App's own notes.
// Crate root because t!() resolves crate::_rust_i18n_translate.
rust_i18n::i18n!("locales", fallback = "en");

use clap::{Args, Parser, Subcommand};

/// What `-V` and `--version` print after the name: the first line is
/// `mstream-player X.Y.Z` exactly in every build, because mStream's
/// launcher probes it with a prefix-anchored pattern; a build with the
/// window (the desktop flavour) adds a second line saying so. The version
/// itself rather than clap's `long_version`, which only `--version` prints
/// (`-V` is clap's short form): both flags answer the same.
const VERSION: &str = if cfg!(feature = "window") {
    // One second line, space-separated words, never a third: mStream's
    // launcher reads only line 2, strips `features:` and splits on commas or
    // whitespace, so `window` still names the desktop build to every launcher
    // that ever read it, and `window-pages` tells one that knows the word that
    // `setup --window` and `qr --window` open the wizard and the Quick
    // Connect page in windows of their own (exit 3 when none can open).
    concat!(env!("CARGO_PKG_VERSION"), "\nfeatures: window window-pages")
} else {
    env!("CARGO_PKG_VERSION")
};

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

#[derive(Subcommand)]
enum Command {
    // A bare `mstream-player` opens this: in the terminal in the terminal
    // flavour, and in the desktop flavour in the GUI's window, or in the
    // terminal where no window can be expected (desktop.rs). `tui` named
    // the player this one replaced, and still parses, to this.
    /// Launch the GUI player (the default) — the mouse-first surface the
    /// installers open: the library rooms, the queue, Auto DJ, servers and
    /// tunnels, Now Playing, Stats, Admin, and the MP3 player's firmware
    #[command(alias = "tui")]
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
    /// The mStream MP3 player (an M5Stack Core2) over USB: list the boards, install or update its firmware
    Device(device::DeviceArgs),
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
fn fresh_token() -> String {
    format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
}

/// A listening window that `Duration::from_secs_f64` will accept.
///
/// The upper bound is a day rather than the type's limit: past that the flag
/// is a typo, and a browse that never returns looks exactly like a hang.
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
            // The desktop flavour also brings the holder's window to the
            // front, when the holder draws in one (its sidecar's host): a
            // holder in a terminal is that terminal's to show, as before.
            // The line stays, for whoever started this one in a terminal.
            #[cfg(feature = "desktop")]
            let focused = who
                .as_ref()
                .filter(|holder| holder.host == "window")
                .map(|holder| desktop::focus_window_holder(holder.pid));
            // A double-clicked app's stdout reaches nobody; the debug log,
            // when one is on, keeps the line. The launcher's flag keeps
            // v0.9.0's stdout alone.
            #[cfg(feature = "desktop")]
            if desktop_window {
                desktop::log_refusal(&line, focused);
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
    // The boot line goes to stderr — in the terminal it scrolls away under
    // the alternate screen and comes back in scrollback after quit, and the
    // goodbye line at teardown says it again for whoever missed it.
    //
    // A one-shot subcommand keeps its hands off the default location: see
    // logging::init.
    let run = match &cli.command {
        None | Some(Command::Gui(_)) | Some(Command::Serve(_)) => logging::Run::Session,
        Some(_) => logging::Run::OneShot,
    };
    if let Some(path) = logging::init(run) {
        eprintln!("logging to {}", path.display());
    }

    // The TLS provider, before anything can dial (see runtime.rs).
    runtime::install_tls_provider();

    // Streaming scratch space (PLAN A1): each playing track spools to a temp
    // file. Decide where those belong before anything can open a stream, and
    // sweep leftovers from killed runs while we're at it. When no cache dir
    // can be resolved the engine falls back to the OS temp dir, so that's
    // where leftovers would be, too.
    let spool_dir = config::spool_dir();
    let sweep = spool_dir.clone().unwrap_or_else(std::env::temp_dir);
    engine::http::clean_spool_dir(&sweep);
    engine::http::set_spool_dir(spool_dir);

    let args = match (cli.command, cli.port) {
        // The player returns its exit code so the instance lock's sidecar
        // is removed on the way out (process::exit runs no destructors).
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
        // The wizard and the Quick Connect page, in this terminal or, with
        // `--window` where the window exists, in a window of their own.
        // Either way a page is not the player: it takes no instance lock
        // and writes no sidecar (the lock's match above gives it none), so
        // it opens beside an open player, as `qr` always has.
        (Some(Command::Setup(args)), _) => std::process::exit(setup::run(args)),
        (Some(Command::Admin(args)), _) => std::process::exit(admin::run(args)),
        (Some(Command::Stats(args)), _) => std::process::exit(admin::run_stats(args)),
        (Some(Command::Qr(args)), _) => std::process::exit(setup::run_qr(args)),
        (Some(Command::Device(args)), _) => std::process::exit(device::run(args)),
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
        (Some(Command::Discover { seconds }), _) => {
            std::process::exit(discovery::print_found(seconds));
        }
        (Some(Command::Playlists(args)), _) => std::process::exit(cmd_library::playlists(args)),
        (Some(Command::Serve(args)), _) => args,
        // Legacy spawn contract: bare `--port N`.
        (None, Some(port)) => ServeArgs {
            port,
            host: "127.0.0.1".to_string(),
            auth_token: std::env::var("MSTREAM_AUDIO_TOKEN").ok(),
            exit_with_parent: false,
            crossfade: 0.0,
            gapless: false,
        },
        // An empty argv is the player with nothing else asked for — no
        // server override, no torrent, no bundled server, no control face.
        // The terminal flavour opens it in this terminal. The desktop
        // flavour opens what `gui --window` opens, under its default lock,
        // and this terminal where no window can be expected.
        (None, None) => {
            let code = gui::run(None, None, None, None, None, desktop_window, &mut instance);
            drop(instance);
            std::process::exit(code);
        }
    };

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
    /// error, as on v0.9.0; a build with the window parses it. The pages'
    /// windows (`setup --window`, `qr --window`) follow the GUI's, and each
    /// command alone is the terminal's in both flavours.
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

        for page in ["setup", "qr"] {
            let parsed = Cli::try_parse_from(["mstream-player", page, "--window"]);
            let bare = Cli::try_parse_from(["mstream-player", page]).unwrap().command;
            assert!(matches!(bare, Some(Command::Setup(_) | Command::Qr(_))), "{page}");
            #[cfg(not(feature = "window"))]
            {
                let err = parsed.err().expect("the terminal flavour has no --window");
                assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument, "{page}");
                assert_eq!(err.exit_code(), 2, "{page}");
            }
            #[cfg(feature = "window")]
            {
                let windowed = |command: Option<Command>| match command {
                    Some(Command::Setup(args)) => args.window,
                    Some(Command::Qr(args)) => args.window,
                    _ => panic!("`{page}` is its own command"),
                };
                assert!(windowed(parsed.expect("the window flavour parses it").command), "{page}");
                assert!(!windowed(bare), "`{page}` alone is still the terminal's");
            }
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
            {
                let second = lines.next().expect("the window adds a second line");
                assert_eq!(second, "features: window window-pages", "{flag}: {text:?}");
                // The prefix and the first word are what every launcher that
                // ever read this line keys on; the pages' word rides behind.
                assert!(second.starts_with("features: window"), "{flag}: {text:?}");
                assert!(second.split_whitespace().any(|w| w == "window"), "{flag}: {text:?}");
            }
            assert_eq!(lines.next(), None, "{flag}: {text:?}");
        }
        let rendered = Cli::command().render_version();
        assert_eq!(rendered.lines().next(), Some(first.as_str()));
    }

    /// `tui` named the terminal player the GUI replaced: the word still
    /// parses, to the GUI, with the flags that player took, and the help
    /// names one player. The commands that served the old one alone are
    /// gone.
    #[test]
    fn tui_still_opens_the_player_and_its_own_commands_are_gone() {
        use clap::CommandFactory;
        let parsed = Cli::try_parse_from([
            "mstream-player",
            "tui",
            "--server",
            "nas:3000",
            "--bundled-server",
            "http://localhost:3000",
        ])
        .expect("`tui` parses");
        let Some(Command::Gui(args)) = parsed.command else {
            panic!("`tui` is the gui command");
        };
        assert_eq!(args.conn.server.as_deref(), Some("nas:3000"));
        assert_eq!(args.bundled_server.as_deref(), Some("http://localhost:3000"));

        let help = Cli::command().render_help().to_string();
        assert!(help.lines().any(|l| l.trim_start().starts_with("gui ")), "{help}");
        assert!(!help.lines().any(|l| l.trim_start().starts_with("tui")), "{help}");

        for gone in ["keys", "replay"] {
            let err = Cli::try_parse_from(["mstream-player", gone]).err().expect("no such command");
            assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand, "{gone}");
        }
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
