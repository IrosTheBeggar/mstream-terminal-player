//! The visualizer window's host (docs/ux-contracts/visualizer-window.md):
//! the child process `mstream-player viz-window`, the audio texture built
//! here from the player's own tap and written down its stdin thirty times a
//! second, and the top bar's word on whether a window is open. The child
//! only draws; if it dies the note says so and the player plays on.

use std::process::Child;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rust_i18n::t;

use super::Gui;
use crate::engine::tap::TapFrame;
use crate::shader::audio::AudioTexture;
use crate::viz_window::pipe::Message;

/// How often the texture goes down the pipe: Android's own batch rate,
/// which the curve's smoothing was tuned at.
const FEED: Duration = Duration::from_millis(33);
/// How long a window gets to close on EOF before it is killed.
const GRACE: Duration = Duration::from_millis(800);

pub(crate) struct VizWindow {
    child: Option<Child>,
    to_writer: Option<SyncSender<Vec<u8>>>,
    texture: AudioTexture,
    frame: TapFrame,
    mono: Vec<f32>,
    last_feed: Instant,
    /// The child's last stderr line: what a failed open is explained with.
    last_words: Arc<Mutex<Option<String>>>,
    /// Tests never spawn a process: the window is a flag there.
    #[cfg(test)]
    pub(crate) dry_open: bool,
    #[cfg(test)]
    pub(crate) raised: u32,
}

impl VizWindow {
    pub(crate) fn new() -> Self {
        VizWindow {
            child: None,
            to_writer: None,
            texture: AudioTexture::new(),
            frame: TapFrame { samples: Vec::new(), rate: 0, channels: 0 },
            mono: Vec::new(),
            last_feed: Instant::now(),
            last_words: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            dry_open: false,
            #[cfg(test)]
            raised: 0,
        }
    }

    #[cfg(not(test))]
    pub(crate) fn is_open(&self) -> bool {
        self.child.is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_open(&self) -> bool {
        self.dry_open
    }

    fn send(&self, message: Message) {
        if let Some(tx) = &self.to_writer {
            // Full means the window is behind: this one is dropped, the
            // next one says the same thing a frame later.
            let _ = tx.try_send(message.encode());
        }
    }
}

pub(crate) fn is_open(gui: &Gui) -> bool {
    gui.vizwin.is_open()
}

/// The top bar's item and `V` (contract entries 1–2): open the window, or
/// bring an open one to the front.
pub(crate) fn toggle(gui: &mut Gui) {
    if gui.vizwin.is_open() {
        #[cfg(test)]
        {
            gui.vizwin.raised += 1;
        }
        gui.vizwin.send(Message::Raise);
        return;
    }
    open(gui);
}

#[cfg(test)]
fn open(gui: &mut Gui) {
    gui.vizwin.dry_open = true;
}

/// Spawn the child, and the two threads that serve it: the writer that
/// takes frames off a two-deep channel — a window that stalls costs frames,
/// never the player's loop (contract clause 3) — and the stderr reader.
#[cfg(not(test))]
fn open(gui: &mut Gui) {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::sync_channel;
    const IN_FLIGHT: usize = 2;

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return failed(gui, e.to_string()),
    };
    let mut command = Command::new(exe);
    command.arg("viz-window").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        // No console window for the child (contract clause 3).
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return failed(gui, e.to_string()),
    };
    let (tx, rx) = sync_channel::<Vec<u8>>(IN_FLIGHT);
    if let Some(mut stdin) = child.stdin.take() {
        let _ = std::thread::Builder::new().name("viz-window feed".into()).spawn(move || {
            for bytes in rx {
                if stdin.write_all(&bytes).is_err() {
                    break;
                }
            }
        });
    }
    // The child's stderr goes to the log, never to the terminal (contract
    // clause 3); its last line is what a failure is explained with.
    let last_words = gui.vizwin.last_words.clone();
    *last_words.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if let Some(stderr) = child.stderr.take() {
        let _ = std::thread::Builder::new().name("viz-window stderr".into()).spawn(move || {
            for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                tracing::info!("[viz-window] {line}");
                *last_words.lock().unwrap_or_else(|e| e.into_inner()) = Some(line);
            }
        });
    }
    gui.vizwin.child = Some(child);
    gui.vizwin.to_writer = Some(tx);
    gui.vizwin.last_feed = Instant::now();
}

fn failed(gui: &mut Gui, why: String) {
    gui.note = Some((t!("gui.viz.failed", why = why).to_string(), true));
}

/// Once a frame: the child's exit, if it exited, and the next texture down
/// the pipe when the last is a batch old (contract clause 2).
pub(crate) fn tick(gui: &mut Gui) {
    let Some(child) = gui.vizwin.child.as_mut() else { return };
    match child.try_wait() {
        Ok(None) => {}
        Ok(Some(status)) => {
            gui.vizwin.child = None;
            gui.vizwin.to_writer = None;
            if !status.success() {
                let why = gui.vizwin.last_words.lock().unwrap_or_else(|e| e.into_inner()).clone();
                failed(gui, why.unwrap_or_else(|| status.to_string()));
            }
            return;
        }
        Err(_) => {
            gui.vizwin.child = None;
            gui.vizwin.to_writer = None;
            return;
        }
    }
    if gui.vizwin.last_feed.elapsed() >= FEED {
        feed(gui);
    }
}

/// The texture for what is playing — silence when nothing is, or it is
/// paused, so the presets settle rather than freeze on the last sound.
fn feed(gui: &mut Gui) {
    let now = Instant::now();
    let elapsed = now.duration_since(gui.vizwin.last_feed).as_secs_f32().min(0.5);
    gui.vizwin.last_feed = now;
    let playing = gui.app.status.playing && !gui.app.status.paused;
    let vizwin = &mut gui.vizwin;
    vizwin.mono.clear();
    if playing
        && let Some(tap) = gui.app.tap.as_ref()
        && tap.frame_into(&mut vizwin.frame)
    {
        vizwin.frame.mono_into(&mut vizwin.mono);
    }
    let bytes = vizwin.texture.update(&vizwin.mono, elapsed).to_vec();
    vizwin.send(Message::Audio(bytes));
}

/// The player is quitting: the pipe's end is the window's cue to quit; one
/// that lingers is killed.
pub(crate) fn close(gui: &mut Gui) {
    gui.vizwin.to_writer = None;
    let Some(mut child) = gui.vizwin.child.take() else { return };
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}
