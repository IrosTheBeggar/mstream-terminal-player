mod api;
mod cmd_library;
mod cmd_play;
mod dj;
mod engine;
mod player;
mod runtime;
mod serve;
mod tui;

use clap::{Args, Parser, Subcommand};

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

#[derive(Subcommand)]
enum Command {
    /// Launch the interactive terminal player (the default)
    Tui(cmd_library::ConnArgs),
    /// Run the headless server-audio engine (jukebox mode)
    Serve(ServeArgs),
    /// Play one source and exit — end-to-end streaming/seek smoke test
    Play(cmd_play::PlayArgs),
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
    /// List playlists, or show one playlist's tracks
    Playlists(cmd_library::PlaylistArgs),
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
}

fn main() {
    let cli = Cli::parse();

    let serve_args = match (cli.command, cli.port) {
        (Some(Command::Tui(conn)), _) => {
            std::process::exit(tui::run(conn.server, conn.token));
        }
        (Some(Command::Play(args)), _) => std::process::exit(cmd_play::run(args)),
        (Some(Command::Login(args)), _) => std::process::exit(cmd_library::login(args)),
        (Some(Command::Logout), _) => std::process::exit(cmd_library::logout()),
        (Some(Command::Info(conn)), _) => std::process::exit(cmd_library::info(conn)),
        (Some(Command::Ls(args)), _) => std::process::exit(cmd_library::ls(args)),
        (Some(Command::Browse(args)), _) => std::process::exit(cmd_library::browse(args)),
        (Some(Command::Search(args)), _) => std::process::exit(cmd_library::search(args)),
        (Some(Command::Dj(args)), _) => std::process::exit(cmd_library::dj(args)),
        (Some(Command::Playlists(args)), _) => std::process::exit(cmd_library::playlists(args)),
        (Some(Command::Serve(args)), _) => Some(args),
        // Legacy spawn contract: bare `--port N`.
        (None, Some(port)) => Some(ServeArgs {
            port,
            host: "127.0.0.1".to_string(),
            auth_token: std::env::var("MSTREAM_AUDIO_TOKEN").ok(),
            exit_with_parent: false,
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
