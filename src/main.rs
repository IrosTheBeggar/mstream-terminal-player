mod engine;
mod serve;

use clap::{Args, CommandFactory, Parser, Subcommand};

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
    /// Run the headless server-audio engine (jukebox mode)
    Serve(ServeArgs),
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
    #[arg(long, env = "MSTREAM_AUDIO_TOKEN")]
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
                eprintln!("mstream-player: {}", e);
                std::process::exit(1);
            }
        }
        None => {
            let _ = Cli::command().print_help();
            println!();
            println!("The interactive terminal player arrives in a later phase (see PLAN.md).");
            println!("For the headless jukebox engine: mstream-player serve --help");
        }
    }
}
