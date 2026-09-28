//! The visualizer window's host (docs/ux-contracts/visualizer-window.md):
//! the child process `mstream-player viz-window`, the audio texture built
//! here from the player's own tap and written down its stdin thirty times a
//! second, and the top bar's word on whether a window is open. The child
//! only draws; if it dies the note says so and the player plays on.
//!
//! The way back is the child's stdout: what its controls changed, a line
//! each (clauses 13–14). The curve applies to the texture at once — it is
//! built here — and everything is kept in `[visualizer]`, saved the way the
//! GUI saves its settings, a moment after the last change.

use std::process::Child;
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rust_i18n::t;

use super::Gui;
use crate::config::{self, VisualizerPrefs};
use crate::engine::tap::TapFrame;
use crate::shader::audio::{AudioTexture, Curve};
use crate::viz_window::controls::curve_from;
use crate::viz_window::pipe::{Message, Report};

/// How often the texture goes down the pipe: Android's own batch rate,
/// which the curve's smoothing was tuned at.
const FEED: Duration = Duration::from_millis(33);
/// How long a window gets to close on EOF before it is killed.
const GRACE: Duration = Duration::from_millis(800);
/// How long the choices rest before they are saved: a slider being dragged
/// reports every frame, and the file is written once, after.
const SAVE_AFTER: Duration = Duration::from_secs(1);

pub(crate) struct VizWindow {
    child: Option<Child>,
    to_writer: Option<SyncSender<Vec<u8>>>,
    /// The window's reports, parsed off its stdout by a thread of their own.
    from_window: Option<Receiver<Report>>,
    texture: AudioTexture,
    frame: TapFrame,
    mono: Vec<f32>,
    last_feed: Instant,
    /// The child's last stderr line: what a failed open is explained with.
    last_words: Arc<Mutex<Option<String>>>,
    /// `[visualizer]` as the window's controls have left it: read when the
    /// window opens, changed by its reports.
    pub(crate) prefs: VisualizerPrefs,
    /// When the choices last changed, while they are not yet saved.
    unsaved: Option<Instant>,
    /// Nothing plays, and the quiet texture — silence, settled — is down
    /// the pipe: the window keeps it, and nothing more is built or sent
    /// until something plays (performance audit #119). Once settled, every
    /// texture after it was the same bytes, thirty times a second, and the
    /// GUI's loop woke at the feed's pace for them.
    settled: bool,
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
            from_window: None,
            texture: AudioTexture::new(),
            frame: TapFrame { samples: Vec::new(), rate: 0, channels: 0 },
            mono: Vec::new(),
            last_feed: Instant::now(),
            last_words: Arc::new(Mutex::new(None)),
            prefs: VisualizerPrefs::default(),
            unsaved: None,
            settled: false,
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

    /// Whether the message went into the pipe's queue.
    fn send(&self, message: Message) -> bool {
        match &self.to_writer {
            // Full means the window is behind: this one is dropped, the
            // next one says the same thing a frame later.
            Some(tx) => tx.try_send(message.encode()).is_ok(),
            None => false,
        }
    }
}

pub(crate) fn is_open(gui: &Gui) -> bool {
    gui.vizwin.is_open()
}

/// Whether the window wants the loop at the feed's pace: open, and
/// something plays or the silence has not settled yet. Settled and quiet,
/// the loop goes back to its own (performance audit #119).
pub(crate) fn wants_frames(gui: &Gui) -> bool {
    gui.vizwin.is_open() && (sounding(gui) || !gui.vizwin.settled)
}

/// Something is playing, and not paused: the tap has samples to show.
fn sounding(gui: &Gui) -> bool {
    gui.app.status.playing && !gui.app.status.paused
}

/// The texture silence settles to, whatever the curve: every bin under
/// the lowest floor the panel allows (the smoothing's -140 dB floor against
/// its -120), and the waveform row at its midline, (0.5 + 0.5 · 0) · 255.
fn quiet(bytes: &[u8]) -> bool {
    let (spectrum, wave) = bytes.split_at(crate::shader::audio::WIDTH);
    spectrum.iter().all(|&b| b == 0) && wave.iter().all(|&b| b == 127)
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

/// The saved choices, read fresh as the window will read them, and the
/// texture's curve set from them by the same function the window's panel
/// uses — so the curve the panel shows is the curve the presets hear.
fn remember(gui: &mut Gui) {
    let prefs = config::load().map(|config| config.visualizer).unwrap_or_default();
    gui.vizwin.texture.set_curve(curve_from(&prefs));
    gui.vizwin.prefs = prefs;
    gui.vizwin.unsaved = None;
}

/// Under test the window is a flag, and the config is left alone: a test
/// that wants the saved choices read calls [`remember`] itself.
#[cfg(test)]
fn open(gui: &mut Gui) {
    gui.vizwin.dry_open = true;
    gui.vizwin.settled = false;
}

/// Spawn the child, and the three threads that serve it: the writer that
/// takes frames off a two-deep channel — a window that stalls costs frames,
/// never the player's loop (contract clause 3) — the stderr reader, and the
/// reader of its reports.
#[cfg(not(test))]
fn open(gui: &mut Gui) {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::{channel, sync_channel};
    const IN_FLIGHT: usize = 2;

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return failed(gui, e.to_string()),
    };
    remember(gui);
    let mut command = Command::new(exe);
    command.arg("viz-window").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
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
    // Its reports (clause 13). A line that is not one goes to the log with
    // the stderr lines, and costs nothing else.
    let (report_tx, report_rx) = channel();
    if let Some(stdout) = child.stdout.take() {
        let _ = std::thread::Builder::new().name("viz-window reports".into()).spawn(move || {
            for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                match Report::parse(&line) {
                    Some(report) => {
                        if report_tx.send(report).is_err() {
                            break;
                        }
                    }
                    None => tracing::info!("[viz-window] {line}"),
                }
            }
        });
    }
    gui.vizwin.child = Some(child);
    gui.vizwin.to_writer = Some(tx);
    gui.vizwin.from_window = Some(report_rx);
    gui.vizwin.last_feed = Instant::now();
    // A new window starts from a blank texture, not the quiet one.
    gui.vizwin.settled = false;
}

fn failed(gui: &mut Gui, why: String) {
    gui.note = Some((t!("gui.viz.failed", why = why).to_string(), true));
}

/// Once a frame: the window's reports, the child's exit if it exited, the
/// choices saved once they rest, and the next texture down the pipe when
/// the last is a batch old (contract clause 2). True when a report came or
/// the window went — the top bar's word, a frame to draw.
pub(crate) fn tick(gui: &mut Gui) -> bool {
    let reported = take_reports(gui, false);
    if gui.vizwin.unsaved.is_some_and(|since| since.elapsed() >= SAVE_AFTER) {
        save(gui);
    }
    let Some(child) = gui.vizwin.child.as_mut() else { return reported };
    match child.try_wait() {
        Ok(None) => {}
        Ok(Some(status)) => {
            gui.vizwin.child = None;
            gui.vizwin.to_writer = None;
            gone(gui);
            if !status.success() {
                let why = gui.vizwin.last_words.lock().unwrap_or_else(|e| e.into_inner()).clone();
                failed(gui, why.unwrap_or_else(|| status.to_string()));
            }
            return true;
        }
        Err(_) => {
            gui.vizwin.child = None;
            gui.vizwin.to_writer = None;
            gone(gui);
            return true;
        }
    }
    if gui.vizwin.last_feed.elapsed() >= FEED {
        feed(gui);
    }
    reported
}

/// Every report the window has sent so far; true when there were any. With
/// `to_the_end`, the reader is waited for until it reaches the end of the
/// pipe — the window is gone, and a pick made on its way out is still kept.
fn take_reports(gui: &mut Gui, to_the_end: bool) -> bool {
    let Some(rx) = gui.vizwin.from_window.as_ref() else { return false };
    let mut reports = Vec::new();
    loop {
        let next = if to_the_end { rx.recv_timeout(GRACE).ok() } else { rx.try_recv().ok() };
        match next {
            Some(report) => reports.push(report),
            None => break,
        }
    }
    let any = !reports.is_empty();
    for report in reports {
        observe(gui, report);
    }
    any
}

/// The window closed: its last words heard, its choices saved now rather
/// than a moment from now.
fn gone(gui: &mut Gui) {
    take_reports(gui, true);
    gui.vizwin.from_window = None;
    save(gui);
}

/// One report from the window (contract clauses 13–14). The curve reaches
/// the texture at once; everything is kept for the next time the window
/// opens. The calibrated curve is kept as no curve at all, and a preset's
/// knobs all at their defaults as no line for it.
pub(crate) fn observe(gui: &mut Gui, report: Report) {
    let vizwin = &mut gui.vizwin;
    let prefs = &mut vizwin.prefs;
    match report {
        Report::Preset(file) => prefs.preset = Some(file),
        Report::Curve(curve) => {
            vizwin.texture.set_curve(curve);
            (prefs.min_db, prefs.max_db, prefs.smoothing) = if curve == Curve::default() {
                (None, None, None)
            } else {
                (Some(curve.min_db), Some(curve.max_db), Some(curve.smoothing))
            };
        }
        Report::Knobs { file, turned } => {
            if turned.is_empty() {
                prefs.knobs.remove(&file);
            } else {
                prefs.knobs.insert(file, turned.into_iter().collect());
            }
        }
    }
    vizwin.unsaved = Some(Instant::now());
}

/// `[visualizer]` to disk, the way the GUI saves its settings: the file
/// loaded fresh — other flows write it behind this copy's back — its
/// section's fields replaced, keys a newer player wrote there kept, and
/// nothing written over a file that would not load at start.
fn save(gui: &mut Gui) {
    if gui.vizwin.unsaved.take().is_none() || !gui.config_ok {
        return;
    }
    let mut config = match config::load() {
        Ok(config) => config,
        Err(e) => {
            gui.note = Some((t!("note.settings_save_failed", err = e).to_string(), true));
            return;
        }
    };
    let extra = std::mem::take(&mut config.visualizer.extra);
    config.visualizer = VisualizerPrefs { extra, ..gui.vizwin.prefs.clone() };
    match config::save(&config) {
        Ok(()) => gui.config = config,
        Err(e) => gui.note = Some((t!("note.settings_save_failed", err = e).to_string(), true)),
    }
}

/// The texture for what is playing — silence when nothing is, or it is
/// paused, so the presets settle rather than freeze on the last sound —
/// until the silence has settled and gone down the pipe; after that the
/// window keeps it, and nothing is built or sent until something plays.
/// Settled is judged against the quiet texture itself, never against the
/// last one sent: two smoothing steps can round to the same bytes long
/// before the bins reach the floor, and stopping there would freeze a
/// picture that is not quiet.
fn feed(gui: &mut Gui) {
    let now = Instant::now();
    let elapsed = now.duration_since(gui.vizwin.last_feed).as_secs_f32().min(0.5);
    gui.vizwin.last_feed = now;
    let playing = sounding(gui);
    if !playing && gui.vizwin.settled {
        return;
    }
    let vizwin = &mut gui.vizwin;
    vizwin.mono.clear();
    if playing
        && let Some(tap) = gui.app.tap.as_ref()
        && tap.frame_into(&mut vizwin.frame)
    {
        vizwin.frame.mono_into(&mut vizwin.mono);
    }
    let bytes = vizwin.texture.update(&vizwin.mono, elapsed).to_vec();
    let settles = !playing && quiet(&bytes);
    // Settled only once the quiet texture is actually queued: one dropped
    // on a full pipe is built and sent again next time.
    vizwin.settled = vizwin.send(Message::Audio(bytes)) && settles;
}

/// The player is quitting: the pipe's end is the window's cue to quit; one
/// that lingers is killed. What it reported on the way out is saved.
pub(crate) fn close(gui: &mut Gui) {
    gui.vizwin.to_writer = None;
    if let Some(mut child) = gui.vizwin.child.take() {
        let deadline = Instant::now() + GRACE;
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !matches!(child.try_wait(), Ok(Some(_))) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    gone(gui);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::testing::Scratch;
    use crate::tui::app::App;

    fn gui() -> Gui {
        Gui::new(config::Config::default(), true, App::new(None, None, None))
    }

    /// The spectrum row's brightest bin for a steady -40 dB tone: where the
    /// texture's dB window puts it.
    fn peak(gui: &mut Gui) -> u8 {
        let tone: Vec<f32> = (0..1024).map(|i| 0.01 * (i as f32 * 0.4).sin()).collect();
        *gui.vizwin.texture.update(&tone, 1.0).iter().take(512).max().unwrap()
    }

    #[test]
    fn the_saved_curve_and_the_windows_reports_reach_the_texture() {
        let scratch = Scratch::new("vizwin-curve");
        let mut gui = gui();
        let calibrated = peak(&mut gui);

        // A window that sets -40 dB lower down, saved: the texture opens on it.
        let saved = "[visualizer]\nmin_db = -60.0\nmax_db = -10.0\n";
        std::fs::write(scratch.dir.join("config.toml"), saved).unwrap();
        remember(&mut gui);
        assert_eq!(gui.vizwin.prefs.min_db, Some(-60.0));
        let saved = peak(&mut gui);
        assert!(saved < calibrated, "{saved} vs {calibrated}");

        // The panel moves it back while the window is open.
        observe(&mut gui, Report::Curve(Curve::default()));
        let prefs = &gui.vizwin.prefs;
        let curve = (prefs.min_db, prefs.max_db, prefs.smoothing);
        assert_eq!(curve, (None, None, None), "the calibrated curve is kept as no curve at all");
        assert_eq!(peak(&mut gui), calibrated);
        let tuned = Curve { min_db: -90.0, max_db: -30.0, smoothing: 0.5 };
        observe(&mut gui, Report::Curve(tuned));
        let prefs = &gui.vizwin.prefs;
        assert_eq!((prefs.min_db, prefs.max_db, prefs.smoothing), (Some(-90.0), Some(-30.0), Some(0.5)));
    }

    /// One feed a batch after the last: the texture falls away at its
    /// pace, as the loop's feeding would have it.
    fn feed_later(gui: &mut Gui) {
        gui.vizwin.last_feed = Instant::now() - FEED;
        feed(gui);
    }

    /// Paused, the window is fed silence until it has settled and then
    /// nothing more, and the loop leaves the feed's pace; a play starts
    /// both again (performance audit #119).
    #[test]
    fn settled_silence_is_sent_once_and_the_loop_leaves_the_feed_pace() {
        let mut gui = gui();
        toggle(&mut gui);
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        gui.vizwin.to_writer = Some(tx);
        let tone: Vec<f32> = (0..1024).map(|i| 0.5 * (i as f32 * 0.4).sin()).collect();
        gui.vizwin.texture.update(&tone, 1.0);
        assert!(wants_frames(&gui), "a window just opened on sound");

        let mut sent = Vec::new();
        for _ in 0..50 {
            feed_later(&mut gui);
            sent.extend(rx.try_iter());
            if gui.vizwin.settled {
                break;
            }
        }
        assert!(gui.vizwin.settled, "silence settles");
        assert!(sent.len() > 1, "it fell away over several textures, not one");
        let last = sent.last().unwrap();
        assert!(quiet(&last[1..]), "the last one sent is the quiet texture");
        assert!(!sent[0][1..].iter().take(512).all(|&b| b == 0), "the first was not");
        assert!(!wants_frames(&gui), "settled: the loop goes back to its own pace");

        feed_later(&mut gui);
        assert!(rx.try_recv().is_err(), "nothing more is sent while nothing plays");

        gui.app.status.playing = true;
        assert!(wants_frames(&gui), "a play wants the feed again");
        feed_later(&mut gui);
        assert!(rx.try_recv().is_ok());
        assert!(!gui.vizwin.settled);
    }

    /// The quiet texture dropped on a full pipe is not taken as sent.
    #[test]
    fn silence_dropped_on_a_full_pipe_is_sent_again() {
        let mut gui = gui();
        toggle(&mut gui);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        gui.vizwin.to_writer = Some(tx);
        feed_later(&mut gui);
        assert!(gui.vizwin.settled, "a fresh texture is already quiet");
        rx.try_recv().unwrap();

        // Open again on a window that is behind: its queue is full.
        toggle(&mut gui);
        gui.vizwin.dry_open = false;
        toggle(&mut gui);
        gui.vizwin.send(Message::Raise);
        feed_later(&mut gui);
        assert!(!gui.vizwin.settled, "dropped, so not settled");
        assert!(wants_frames(&gui));
        rx.try_recv().unwrap();
        feed_later(&mut gui);
        assert!(gui.vizwin.settled);
        assert!(quiet(&rx.try_recv().unwrap()[1..]));
    }

    #[test]
    fn the_choices_are_saved_once_they_rest_and_when_the_window_goes() {
        let scratch = Scratch::new("vizwin-save");
        let mut gui = gui();
        config::save(&gui.config).unwrap();
        toggle(&mut gui);
        observe(&mut gui, Report::Preset("05-hex-marching.glsl".into()));
        let turned = vec![("bars".into(), 48.0)];
        observe(&mut gui, Report::Knobs { file: "01-spectrum-bars.glsl".into(), turned });
        tick(&mut gui);
        let early = config::load().unwrap().visualizer;
        assert!(early.preset.is_none(), "not while the choices are still moving");

        // A while later, one save carries both.
        gui.vizwin.unsaved = Some(Instant::now() - SAVE_AFTER);
        tick(&mut gui);
        let saved = config::load().unwrap().visualizer;
        assert_eq!(saved.preset.as_deref(), Some("05-hex-marching.glsl"));
        assert_eq!(saved.knobs["01-spectrum-bars.glsl"]["bars"], 48.0);
        assert_eq!(gui.config.visualizer, saved, "the GUI's copy follows the file");

        // Knobs all back at their defaults leave no line behind; the window
        // closing saves at once.
        observe(&mut gui, Report::Knobs { file: "01-spectrum-bars.glsl".into(), turned: Vec::new() });
        close(&mut gui);
        assert!(config::load().unwrap().visualizer.knobs.is_empty());
        let _ = &scratch;
    }

    #[test]
    fn a_key_a_newer_player_wrote_in_the_section_survives_the_save() {
        let scratch = Scratch::new("vizwin-keep");
        std::fs::write(scratch.dir.join("config.toml"), "[visualizer]\nmode = \"bars\"\n").unwrap();
        let mut gui = gui();
        toggle(&mut gui);
        observe(&mut gui, Report::Preset("02-audio-tunnel.glsl".into()));
        close(&mut gui);
        let saved = config::load().unwrap().visualizer;
        assert_eq!(saved.preset.as_deref(), Some("02-audio-tunnel.glsl"));
        assert_eq!(saved.extra.get("mode").and_then(toml::Value::as_str), Some("bars"));
    }

    #[test]
    fn a_config_that_would_not_load_at_start_is_not_written_over() {
        let scratch = Scratch::new("vizwin-guard");
        std::fs::write(scratch.dir.join("config.toml"), "version = 1\n[player\n").unwrap();
        let mut gui = Gui::new(config::Config::default(), false, App::new(None, None, None));
        toggle(&mut gui);
        observe(&mut gui, Report::Preset("02-audio-tunnel.glsl".into()));
        close(&mut gui);
        let raw = std::fs::read_to_string(scratch.dir.join("config.toml")).unwrap();
        assert_eq!(raw, "version = 1\n[player\n", "the broken file is the user's to fix");
    }
}
