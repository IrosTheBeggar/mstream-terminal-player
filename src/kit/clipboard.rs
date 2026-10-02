//! Text to the system clipboard, by whichever route reaches it from where
//! the player runs. Every copy in the player goes through [`copy`], which
//! answers how the text left ([`Copied`]) so the caller's note can say
//! what the user should expect.
//!
//! The routes, in the order they are tried:
//!
//! In the GUI's own window, the pasteboard (arboard), then the platform's
//! tool. Never OSC 52: a window launched from a shell would write the
//! escape into that shell, where it means nothing or something else.
//!
//! In a terminal over SSH, OSC 52 alone. The pasteboard and the tools
//! would fill the clipboard of the machine the player runs on, which is
//! not the one in front of the user; the terminal's escape travels back
//! down the connection to the terminal that is.
//!
//! In a local terminal, the pasteboard (in a build that has the window,
//! the only one that links arboard), then the tool, then OSC 52, which
//! a terminal may refuse without a word (Apple Terminal ignores it,
//! iTerm2 asks for a setting), so it is the last resort and the note
//! says it is one.
//!
//! A tool is fed on stdin and given a second: one that exits non-zero, or
//! hangs (an X11 tool with no server to talk to can), is passed over for
//! the next route.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;

use super::os::{Os, Vars, process_var};

/// How a copy left the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Copied {
    /// The pasteboard or a platform tool took it: it is on the clipboard.
    Clipboard,
    /// It was handed to the terminal as OSC 52, which may or may not have
    /// put it on the clipboard; nothing tells the player which.
    Terminal,
    /// No route took it.
    Failed,
}

/// What the routes depend on, read once per copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    pub os: Os,
    /// Where the tools are looked for.
    pub path: Option<OsString>,
    /// The terminal is at the other end of an SSH connection.
    pub remote: bool,
    /// A Wayland session (`WAYLAND_DISPLAY`), where `wl-copy` reaches the
    /// clipboard.
    pub wayland: bool,
    /// An X11 display (`DISPLAY`, XWayland's included), where `xclip` and
    /// `xsel` do.
    pub x11: bool,
    /// The player draws into its own window rather than a terminal.
    pub windowed: bool,
    /// This build links arboard, the window's pasteboard.
    pub pasteboard: bool,
}

impl Env {
    /// The process's own environment.
    pub fn from_process() -> Env {
        Env::from_vars(Os::HERE, &process_var, windowed(), cfg!(feature = "window"))
    }

    /// The environment `vars` describes, empty values read as unset
    /// whatever the lookup does with them.
    pub(crate) fn from_vars(os: Os, vars: Vars, windowed: bool, pasteboard: bool) -> Env {
        let set = |name: &str| vars(name).is_some_and(|value| !value.is_empty());
        Env {
            os,
            path: vars("PATH"),
            remote: ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"].into_iter().any(set),
            wayland: set("WAYLAND_DISPLAY"),
            x11: set("DISPLAY"),
            windowed,
            pasteboard,
        }
    }
}

/// A platform's clipboard tool: the program, its arguments, and what the
/// text needs to reach it intact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tool {
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// The child is given `LC_CTYPE=UTF-8`: pbcopy reads its input in the
    /// locale's encoding, and an app launched from the Dock has no `LANG`,
    /// which turns every accent into mojibake.
    pub utf8_locale: bool,
    /// The text is sent as UTF-16LE behind a byte-order mark, with CRLF
    /// line ends: clip.exe reads anything else in the console's code page.
    pub utf16: bool,
}

/// One way to the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Pasteboard,
    Tool(Tool),
    Terminal,
}

/// The longest text the terminal route sends, in bytes before encoding.
/// Terminals cap the escape themselves (xterm and kitty at a few MiB of
/// base64, some far lower) and drop what is longer without a word; past
/// this the answer is honestly `Failed`, and the caller's note offers
/// another way.
pub const OSC52_MAX: usize = 1 << 20;

/// How long a tool may take to read its text and exit.
pub(crate) const TOOL_WAIT: Duration = Duration::from_secs(1);

/// The routes tried from `env`, in order (see the module docs).
pub(crate) fn routes(env: &Env) -> Vec<Route> {
    if env.remote && !env.windowed {
        return vec![Route::Terminal];
    }
    let mut routes = Vec::new();
    if env.pasteboard {
        routes.push(Route::Pasteboard);
    }
    routes.extend(tools(env).into_iter().map(Route::Tool));
    if !env.windowed {
        routes.push(Route::Terminal);
    }
    routes
}

/// The platform's clipboard tools, in the order they are tried. On Linux
/// and the BSDs a tool is offered only for a session it can reach: an X11
/// tool with no `DISPLAY` would only fail, or hang looking for one.
pub(crate) fn tools(env: &Env) -> Vec<Tool> {
    const PBCOPY: Tool = Tool { program: "pbcopy", args: &[], utf8_locale: true, utf16: false };
    const CLIP: Tool = Tool { program: "clip.exe", args: &[], utf8_locale: false, utf16: true };
    const WL_COPY: Tool = Tool { program: "wl-copy", args: &[], utf8_locale: false, utf16: false };
    const XCLIP: Tool =
        Tool { program: "xclip", args: &["-selection", "clipboard"], utf8_locale: false, utf16: false };
    const XSEL: Tool =
        Tool { program: "xsel", args: &["--clipboard", "--input"], utf8_locale: false, utf16: false };
    match env.os {
        Os::Mac => vec![PBCOPY],
        Os::Windows => vec![CLIP],
        Os::Unix => {
            let mut tools = Vec::new();
            if env.wayland {
                tools.push(WL_COPY);
            }
            if env.x11 {
                tools.extend([XCLIP, XSEL]);
            }
            tools
        }
    }
}

/// The first `program` in the directories of `path`, the way a shell
/// finds it. Off Windows it must be a regular file someone may execute.
/// A relative entry (`.`, an empty one) is skipped: the player's working
/// directory is wherever it was launched from, and a clipboard tool is
/// never meant to come from there.
pub(crate) fn find_on(path: &OsStr, program: &str, os: Os) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| runnable(candidate, os))
}

fn runnable(candidate: &Path, os: Os) -> bool {
    let Ok(meta) = candidate.metadata() else { return false };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    if os != Os::Windows {
        use std::os::unix::fs::PermissionsExt;
        return meta.permissions().mode() & 0o111 != 0;
    }
    let _ = os;
    true
}

/// The bytes `tool` reads for `text`.
pub(crate) fn tool_bytes(tool: &Tool, text: &str) -> Vec<u8> {
    if !tool.utf16 {
        return text.as_bytes().to_vec();
    }
    let crlf = text.replace("\r\n", "\n").replace('\n', "\r\n");
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(crlf.encode_utf16().flat_map(u16::to_le_bytes));
    bytes
}

/// Run `exe` as `tool` with `text` on its stdin; true when it exited 0
/// within `wait`. Its stdout and stderr go nowhere: xclip, xsel and
/// wl-copy fork a child that keeps serving the selection, and a pipe that
/// child inherited would never close. The text is written from a thread of
/// its own, so a tool that never reads cannot stall the caller past
/// `wait`; when the tool is killed the write ends on a broken pipe, and a
/// write still blocked by a grandchild holding the pipe ends with it.
pub(crate) fn feed(exe: &Path, tool: &Tool, text: &str, wait: Duration) -> bool {
    let mut command = Command::new(exe);
    command.args(tool.args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    if tool.utf8_locale {
        command.env("LC_CTYPE", "UTF-8");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: the window build has no console, and clip.exe
        // would otherwise flash one of its own.
        command.creation_flags(0x0800_0000);
    }
    let Ok(mut child) = command.spawn() else { return false };
    if let Some(mut stdin) = child.stdin.take() {
        let bytes = tool_bytes(tool, text);
        let spawned = std::thread::Builder::new().name("clipboard tool feed".into()).spawn(move || {
            let _ = stdin.write_all(&bytes);
        });
        if spawned.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
    }
    let deadline = Instant::now() + wait;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// The OSC 52 escape that asks a terminal to put `text` on the clipboard.
pub fn osc52(text: &str) -> String {
    let payload = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{payload}\x1b\\")
}

/// [`copy`] with its outside world handed in: the environment, where the
/// terminal's escape is written, and the pasteboard.
pub(crate) fn copy_with(
    text: &str,
    env: &Env,
    out: &mut dyn Write,
    pasteboard: &mut dyn FnMut(&str) -> bool,
) -> Copied {
    for route in routes(env) {
        let took = match route {
            Route::Pasteboard => pasteboard(text),
            Route::Tool(tool) => env
                .path
                .as_deref()
                .and_then(|path| find_on(path, tool.program, env.os))
                .is_some_and(|exe| feed(&exe, &tool, text, TOOL_WAIT)),
            Route::Terminal => {
                if text.len() <= OSC52_MAX
                    && out.write_all(osc52(text).as_bytes()).and_then(|()| out.flush()).is_ok()
                {
                    return Copied::Terminal;
                }
                false
            }
        };
        if took {
            return Copied::Clipboard;
        }
    }
    Copied::Failed
}

/// Put `text` on the clipboard by the first route that takes it. Under
/// test it is caught on the calling thread instead and never reaches the
/// machine's clipboard, its tools or the terminal (see `catch`).
pub fn copy(text: &str) -> Copied {
    #[cfg(test)]
    {
        catcher::copy(text)
    }
    #[cfg(not(test))]
    {
        #[cfg(feature = "window")]
        let mut board = pasteboard;
        #[cfg(not(feature = "window"))]
        let mut board = |_: &str| false;
        copy_with(text, &Env::from_process(), &mut std::io::stdout(), &mut board)
    }
}

static WINDOWED: AtomicBool = AtomicBool::new(false);

/// The player now draws into its own window: from here a copy never writes
/// OSC 52. Set once the window's event loop exists, not before, so a
/// window that cannot open and falls back to the terminal keeps the
/// terminal's route.
#[cfg_attr(not(feature = "window"), allow(dead_code))]
pub fn set_windowed() {
    WINDOWED.store(true, Ordering::Relaxed);
}

fn windowed() -> bool {
    WINDOWED.load(Ordering::Relaxed)
}

/// The window's pasteboard. One clipboard for the life of the thread that
/// copies (the UI's, which lives as long as the process): on X11 the
/// clipboard's contents belong to whoever set them and vanish when that
/// connection closes, so a clipboard opened per copy would take its text
/// with it the moment it dropped.
#[cfg(all(feature = "window", not(test)))]
fn pasteboard(text: &str) -> bool {
    use std::cell::RefCell;
    thread_local! {
        static BOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
    }
    BOARD.with(|cell| {
        let mut board = cell.borrow_mut();
        if board.is_none() {
            *board = arboard::Clipboard::new().ok();
        }
        board.as_mut().is_some_and(|board| board.set_text(text).is_ok())
    })
}

/// What [`copy`] does under test: it records the text on the calling
/// thread and answers what the test asked it to.
#[cfg(test)]
mod catcher {
    use std::cell::{Cell, RefCell};

    use super::Copied;

    thread_local! {
        static ANSWER: Cell<Copied> = const { Cell::new(Copied::Clipboard) };
        static CAUGHT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn copy(text: &str) -> Copied {
        CAUGHT.with_borrow_mut(|caught| caught.push(text.to_string()));
        ANSWER.get()
    }

    pub(super) fn catch(answer: Copied) {
        ANSWER.set(answer);
    }

    pub(super) fn caught() -> Vec<String> {
        CAUGHT.take()
    }
}

/// What [`copy`] answers on this thread from now on (`Clipboard` until
/// told otherwise).
#[cfg(test)]
pub(crate) fn catch(answer: Copied) {
    catcher::catch(answer);
}

/// The texts [`copy`] was handed on this thread since the last call.
#[cfg(test)]
pub(crate) fn caught() -> Vec<String> {
    catcher::caught()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(os: Os) -> Env {
        Env { os, path: None, remote: false, wayland: false, x11: false, windowed: false, pasteboard: false }
    }

    fn tool_named(name: &str) -> Tool {
        let all = Env { wayland: true, x11: true, ..env(Os::Unix) };
        [tools(&env(Os::Mac)), tools(&env(Os::Windows)), tools(&all)]
            .concat()
            .into_iter()
            .find(|tool| tool.program == name)
            .expect("a known tool")
    }

    /// A fresh directory under the system's temp dir, emptied first.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mstream-clipboard-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A shell script named `name` in `dir`, executable.
    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn never(_: &str) -> bool {
        false
    }

    #[test]
    fn osc52_is_the_base64_text_between_the_escape_and_st() {
        assert_eq!(osc52("hello"), "\x1b]52;c;aGVsbG8=\x1b\\");
        assert_eq!(osc52(""), "\x1b]52;c;\x1b\\");
    }

    #[test]
    fn the_terminal_route_refuses_a_payload_past_the_cap() {
        let remote = Env { remote: true, ..env(Os::Unix) };
        let fits = "a".repeat(OSC52_MAX);
        let mut out = Vec::new();
        assert_eq!(copy_with(&fits, &remote, &mut out, &mut never), Copied::Terminal);
        assert_eq!(out, osc52(&fits).into_bytes());
        let over = "a".repeat(OSC52_MAX + 1);
        let mut out = Vec::new();
        assert_eq!(copy_with(&over, &remote, &mut out, &mut never), Copied::Failed);
        assert!(out.is_empty(), "nothing is written past the cap");
    }

    #[test]
    fn routes_follow_where_the_player_runs() {
        let mac = env(Os::Mac);
        let pbcopy = Route::Tool(tool_named("pbcopy"));
        let window = Env { windowed: true, ..mac.clone() };
        assert_eq!(routes(&window), [pbcopy]);
        assert_eq!(routes(&Env { pasteboard: true, ..window.clone() }), [Route::Pasteboard, pbcopy]);
        assert_eq!(
            routes(&Env { remote: true, pasteboard: true, ..window }),
            [Route::Pasteboard, pbcopy],
            "the window never writes the escape, wherever its shell came from"
        );
        let remote = Env { remote: true, pasteboard: true, ..mac.clone() };
        assert_eq!(routes(&remote), [Route::Terminal], "over SSH only the terminal reaches the user's machine");
        assert_eq!(routes(&mac), [pbcopy, Route::Terminal]);
        assert_eq!(routes(&Env { pasteboard: true, ..mac }), [Route::Pasteboard, pbcopy, Route::Terminal]);
        assert_eq!(routes(&env(Os::Unix)), [Route::Terminal], "no display, no tool");
    }

    #[test]
    fn a_session_over_ssh_is_remote() {
        let with = |pairs: &'static [(&'static str, &'static str)]| {
            let vars = move |name: &str| {
                pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| OsString::from(value))
            };
            Env::from_vars(Os::Unix, &vars, false, false)
        };
        assert!(!with(&[]).remote);
        assert!(with(&[("SSH_CONNECTION", "10.0.0.2 51000 10.0.0.1 22")]).remote);
        assert!(with(&[("SSH_CLIENT", "10.0.0.2 51000 22")]).remote);
        assert!(with(&[("SSH_TTY", "/dev/pts/3")]).remote);
        assert!(!with(&[("SSH_TTY", ""), ("SSH_CLIENT", "")]).remote, "empty values do not count");
        let local = with(&[("PATH", "/usr/bin"), ("DISPLAY", ":0"), ("WAYLAND_DISPLAY", "")]);
        assert_eq!(local.path, Some(OsString::from("/usr/bin")));
        assert!(local.x11 && !local.wayland);
    }

    #[test]
    fn each_platform_names_its_tools_and_linux_asks_for_a_display() {
        let names = |e: &Env| tools(e).iter().map(|t| t.program).collect::<Vec<_>>();
        let mac = tools(&env(Os::Mac));
        assert_eq!(mac.len(), 1);
        assert!(mac[0].program == "pbcopy" && mac[0].utf8_locale && !mac[0].utf16);
        let windows = tools(&env(Os::Windows));
        assert_eq!(windows.len(), 1);
        assert!(windows[0].program == "clip.exe" && windows[0].utf16 && !windows[0].utf8_locale);
        let unix = env(Os::Unix);
        assert!(names(&unix).is_empty(), "neither display: nothing to ask");
        assert_eq!(names(&Env { wayland: true, ..unix.clone() }), ["wl-copy"]);
        assert_eq!(names(&Env { x11: true, ..unix.clone() }), ["xclip", "xsel"]);
        assert_eq!(names(&Env { wayland: true, x11: true, ..unix }), ["wl-copy", "xclip", "xsel"]);
        assert_eq!(tool_named("xclip").args, ["-selection", "clipboard"]);
        assert_eq!(tool_named("xsel").args, ["--clipboard", "--input"]);
    }

    #[test]
    fn the_first_tool_present_on_the_path_wins() {
        let root = scratch("find");
        let (empty, first, second) = (root.join("empty"), root.join("first"), root.join("second"));
        for dir in [&empty, &first, &second] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let make = |dir: &Path, exec: bool| {
            let path = dir.join("tool");
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = if exec { 0o755 } else { 0o644 };
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            }
            #[cfg(not(unix))]
            let _ = exec;
            path
        };
        let path = std::env::join_paths([&empty, &first, &second]).unwrap();
        let os = if cfg!(windows) { Os::Windows } else { Os::Unix };
        assert_eq!(find_on(&path, "tool", os), None, "none anywhere");
        let later = make(&second, true);
        assert_eq!(find_on(&path, "tool", os), Some(later.clone()));
        let earlier = make(&first, true);
        assert_eq!(find_on(&path, "tool", os), Some(earlier), "the earlier directory wins");
        if cfg!(unix) {
            make(&first, false);
            assert_eq!(find_on(&path, "tool", os), Some(later), "a file nobody may run is skipped");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_tool_is_fed_the_text_on_stdin() {
        let dir = scratch("feed");
        let got = dir.join("got");
        let seen = dir.join("env");
        let exe = script(
            &dir,
            "pbcopy",
            &format!("cat > '{}'; printf '%s' \"$LC_CTYPE\" > '{}'", got.display(), seen.display()),
        );
        let text = "héllo\nworld";
        assert!(feed(&exe, &tool_named("pbcopy"), text, Duration::from_secs(5)));
        assert_eq!(std::fs::read_to_string(&got).unwrap(), text, "UTF-8 arrives as it left");
        assert_eq!(std::fs::read_to_string(&seen).unwrap(), "UTF-8", "pbcopy is told the encoding");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_tool_falls_through_to_the_next() {
        let dir = scratch("fallthrough");
        let got = dir.join("got");
        script(&dir, "xclip", "exit 1");
        script(&dir, "xsel", &format!("cat > '{}'", got.display()));
        let window = Env { path: Some(dir.clone().into_os_string()), x11: true, windowed: true, ..env(Os::Unix) };
        let mut out = Vec::new();
        assert_eq!(copy_with("the ticket", &window, &mut out, &mut never), Copied::Clipboard);
        assert_eq!(std::fs::read_to_string(&got).unwrap(), "the ticket");
        assert!(out.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_stuck_tool_is_killed_and_the_next_route_taken() {
        let dir = scratch("stuck");
        script(&dir, "xclip", "exec sleep 5");
        let local = Env { path: Some(dir.clone().into_os_string()), x11: true, ..env(Os::Unix) };
        let mut out = Vec::new();
        let started = Instant::now();
        assert_eq!(copy_with("the ticket", &local, &mut out, &mut never), Copied::Terminal);
        assert!(started.elapsed() < TOOL_WAIT + Duration::from_secs(2), "{:?}", started.elapsed());
        assert_eq!(out, osc52("the ticket").into_bytes());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pasteboard_is_tried_first_and_a_refusal_falls_through() {
        let local = Env { pasteboard: true, ..env(Os::Unix) };
        let mut asked = Vec::new();
        let mut out = Vec::new();
        let mut taking = |text: &str| {
            asked.push(text.to_string());
            true
        };
        assert_eq!(copy_with("one", &local, &mut out, &mut taking), Copied::Clipboard);
        assert_eq!(asked, ["one"]);
        assert!(out.is_empty(), "nothing after the pasteboard took it");
        assert_eq!(copy_with("two", &local, &mut out, &mut never), Copied::Terminal);
        assert_eq!(out, osc52("two").into_bytes());
    }

    #[test]
    fn nothing_reachable_in_the_window_is_a_failure_and_writes_no_escape() {
        let dir = scratch("window");
        let window = Env {
            path: Some(dir.clone().into_os_string()),
            wayland: true,
            x11: true,
            windowed: true,
            pasteboard: true,
            ..env(Os::Unix)
        };
        let mut out = Vec::new();
        assert_eq!(copy_with("the ticket", &window, &mut out, &mut never), Copied::Failed);
        assert!(out.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clip_exe_reads_utf16_with_a_bom_and_crlf() {
        let clip = tool_named("clip.exe");
        let want: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("é\r\nb\r\nc".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(tool_bytes(&clip, "é\nb\r\nc"), want, "a CRLF already there is not doubled");
        assert_eq!(tool_bytes(&tool_named("pbcopy"), "é\nb"), "é\nb".as_bytes());
    }

    #[test]
    fn arboard_is_a_route_only_in_a_window_build() {
        assert_eq!(Env::from_process().pasteboard, cfg!(feature = "window"));
    }

    #[test]
    fn under_test_a_copy_is_caught_and_never_reaches_the_os() {
        assert_eq!(copy("first"), Copied::Clipboard, "the default answer");
        catch(Copied::Failed);
        assert_eq!(copy("second"), Copied::Failed);
        assert_eq!(caught(), ["first", "second"]);
        assert!(caught().is_empty(), "drained");
    }
}
