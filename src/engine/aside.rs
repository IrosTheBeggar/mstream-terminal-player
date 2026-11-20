//! The aside clip: a preview played beside main, for the Peer recs room
//! (peer-recs contract §1.8; peer-recommendations plan §2 items 13–14).
//!
//! The clip is a second rodio Player on the engine's own bus, owned by the
//! state beside the main sink. So it follows every device move for free,
//! and every main command can end it under the same lock.
//!
//! Its open runs on a thread of its own, as a blend's prepare does, and the
//! tick installs the answer. The audio thread never waits on a CDN, so
//! main's controls and status keep flowing while a clip loads.
//!
//! A clip session begins with the first clip and lives through every clip
//! that follows it (Preview all). When its first clip has opened, main is
//! paused in place if it was audibly playing, and the session remembers
//! that it owes main a resume. The session ends by a run-out, a stop, a
//! hold that expires, an outage, or any main transport command. Main comes
//! back only on the first four, and only if the session paused it and main
//! is still that track, still paused. Every main transport command ends the
//! clip first, as `Taken`, and gives nothing back: main always wins.
//!
//! The clip is never tapped (the visualizer shows main, which is paused),
//! never appears in main's status, and is never a play. Its fate reaches
//! the driver as one [`AsideEnded`] per ticket.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rodio::Player;

use super::trace::etrace;
use super::{
    Engine, OUTGOING_SLACK, Opener, Outgoing, Prepared, QueueEntry, STOP_FADE, State, attach,
    bus, fade, feed, http, spawn_open,
};
use crate::player::{AsideEnd, AsideEnded, AsidePhase, AsideStatus, AsideThen};

/// How long a clip may take to open before it counts as failed: the
/// request, the head start and the decode probe. The phone's load timeout,
/// and under a main open's `START_TIMEOUT`.
#[cfg(not(test))]
const ASIDE_OPEN_PATIENCE: Duration = Duration::from_secs(15);
#[cfg(test)]
const ASIDE_OPEN_PATIENCE: Duration = Duration::from_millis(1500);

/// How long a sounding clip's position may stand still, while the device
/// is taking sound, before the clip counts as stalled. stream-download's
/// watchdog reconnects a stalled download for ever, so without this a dead
/// CDN would leave the band frozen at one second for good.
#[cfg(not(test))]
const ASIDE_STALL: Duration = Duration::from_secs(8);
/// A second, not less: a debug build's decode thread on a busy machine
/// can fall behind for a few hundred milliseconds, and that is no stall.
#[cfg(test)]
const ASIDE_STALL: Duration = Duration::from_secs(1);

/// A run-out this far before the clip's known length is a break: a CDN
/// that cut the stream and refused the range reconnect.
const ASIDE_SHORT: Duration = Duration::from_millis(1500);

/// How long a held run-out (Preview all) keeps main paused for the next
/// clip, unless the driver stretches it.
#[cfg(not(test))]
const ASIDE_HOLD: Duration = Duration::from_secs(5);
#[cfg(test)]
const ASIDE_HOLD: Duration = Duration::from_millis(300);

/// The longest hold a driver may ask for: it covers iTunes' 60 s wait
/// after a 429, its retry and a resolve.
const ASIDE_HOLD_MOST: Duration = Duration::from_secs(90);

/// How many clip ends wait for a driver before the oldest goes. Every
/// driver drains them each tick; nothing requires one to.
const ASIDE_ENDS_MOST: usize = 16;

/// How many clips may breathe out at once. Past three, someone is mashing
/// keys, and the oldest, the quietest, is cut.
const ASIDE_DRAINS_MOST: usize = 3;

/// What the tick knows of the device, for the clip's sake. It is read
/// under the output lock before the state lock is taken.
#[derive(Debug, Clone, Copy)]
pub(super) struct Device {
    /// An outage has been announced, and still holds.
    pub(super) lost: bool,
    /// The device is taking sound right now: usable, awake, and its
    /// callback has asked for a buffer a moment ago. A clip whose position
    /// stands still while this is false is waiting on the device, not
    /// stalled.
    pub(super) taking: bool,
}

/// One clip session: the clip in hand, or the last one while main is held.
pub(super) struct Aside {
    /// The driver's name for the clip in hand; while held, the last one's.
    ticket: u64,
    then: AsideThen,
    phase: Phase,
    promise: Promise,
}

enum Phase {
    /// The clip is opening on its own thread.
    Opening { opener: Opener, since: Instant },
    /// The clip is on the bus. `moved` is when its position last moved,
    /// and where it stood then: the stall clock.
    Sounding { clip: Clip, paused: bool, moved: (Instant, f64) },
    /// No clip: main is held paused for the next one until `until`.
    Held { until: Instant },
}

/// A clip on the bus, with the controls its source was wrapped in.
struct Clip {
    sink: Arc<Player>,
    fade: Arc<fade::FadeHandle>,
    feed: Arc<feed::Feed>,
    /// Its length as its decoder knows it; 0 when unknown.
    duration: f64,
}

/// What the session owes main when it ends.
enum Promise {
    /// No clip has opened yet: main is as the listener left it.
    Undecided,
    /// The session paused main while it played `file`.
    Resume { file: String },
    /// Main was not audibly playing when the first clip opened.
    Nothing,
}

/// What one look at the session found, for the tick to act on.
enum Step {
    Wait,
    Install(Prepared),
    Fail(String),
    Refuse(&'static str),
    RanOut,
    Expire,
}

impl Aside {
    /// One look at the session: an open's answer, a run-out, a stall, an
    /// expired hold. The stall clock is kept up to date in place.
    fn look(&mut self, device: Device, now: Instant) -> Step {
        match &mut self.phase {
            Phase::Opening { opener, since } => match opener.rx.try_recv() {
                Ok(Ok(_)) if device.lost => Step::Refuse("the output device was lost"),
                Ok(Ok(prepared)) => Step::Install(prepared),
                // An expired preview URL's 403, 404 or 410 lands here, as
                // does a probe that cannot decode.
                Ok(Err(e)) => Step::Fail(e),
                // The open panicked, and the catch dropped the sender.
                Err(mpsc::TryRecvError::Disconnected) => {
                    Step::Fail("the decoder gave up on the clip".into())
                }
                Err(mpsc::TryRecvError::Empty)
                    if now.saturating_duration_since(*since) >= ASIDE_OPEN_PATIENCE =>
                {
                    Step::Fail("the clip did not load".into())
                }
                Err(mpsc::TryRecvError::Empty) => Step::Wait,
            },
            // A paused clip is frozen: nothing about it can change.
            Phase::Sounding { paused: true, .. } => Step::Wait,
            Phase::Sounding { clip, moved, .. } => {
                let at = clip.feed.position();
                if device.lost {
                    return Step::Refuse("the output device was lost");
                }
                if clip.sink.empty() {
                    let short = clip.duration > 0.0
                        && at < clip.duration - ASIDE_SHORT.as_secs_f64();
                    if short {
                        return Step::Fail("the clip broke off".into());
                    }
                    return Step::RanOut;
                }
                // The clock runs only while the device takes sound and the
                // clip has none to give it.
                if at != moved.1 || !device.taking {
                    *moved = (now, at);
                    return Step::Wait;
                }
                if now.saturating_duration_since(moved.0) >= ASIDE_STALL {
                    Step::Fail("the clip stalled".into())
                } else {
                    Step::Wait
                }
            }
            Phase::Held { until } => {
                if now >= *until {
                    Step::Expire
                } else {
                    Step::Wait
                }
            }
        }
    }

    fn status(&self) -> AsideStatus {
        let (phase, position, duration) = match &self.phase {
            Phase::Opening { .. } => (AsidePhase::Loading, 0.0, 0.0),
            Phase::Sounding { clip, paused, .. } => (
                if *paused { AsidePhase::Paused } else { AsidePhase::Playing },
                clip.feed.position(),
                clip.duration,
            ),
            Phase::Held { .. } => (AsidePhase::Held, 0.0, 0.0),
        };
        AsideStatus { ticket: self.ticket, phase, position, duration }
    }
}

impl State {
    /// Note how a clip ended, for the driver. Bounded: the oldest goes.
    fn push_aside_end(&mut self, ticket: u64, end: AsideEnd) {
        etrace!("aside {ticket} ended: {end:?}");
        if self.aside_ends.len() >= ASIDE_ENDS_MOST {
            self.aside_ends.remove(0);
        }
        self.aside_ends.push(AsideEnded { ticket, end });
    }

    /// End the clip in hand, if there is one, as `end`: an open is walked
    /// away from (its answer drops into a closed channel on its own
    /// thread), and a sounding clip breathes out. The session stays, held
    /// until now; the caller says what follows. A held session has no clip
    /// in hand, and is left as it is.
    fn end_clip(&mut self, end: AsideEnd) {
        let Some(aside) = self.aside.as_mut() else { return };
        if matches!(aside.phase, Phase::Held { .. }) {
            return;
        }
        let ticket = aside.ticket;
        match std::mem::replace(&mut aside.phase, Phase::Held { until: Instant::now() }) {
            Phase::Opening { opener, .. } => drop(opener),
            Phase::Sounding { clip, paused, .. } => self.breathe_out(clip, paused),
            Phase::Held { .. } => {}
        }
        self.push_aside_end(ticket, end);
    }

    /// Let a clip go with a breath instead of a click: its fade ramps to
    /// silence over [`STOP_FADE`] and the sink drains in `aside_drains`. A
    /// clip with nothing audible left, paused or run out, is just stopped.
    fn breathe_out(&mut self, clip: Clip, paused: bool) {
        if paused || clip.sink.empty() {
            clip.sink.stop();
            return;
        }
        clip.fade.ramp_to(0.0, STOP_FADE);
        if self.aside_drains.len() >= ASIDE_DRAINS_MOST {
            self.aside_drains.remove(0).sink.stop();
        }
        self.aside_drains.push(Outgoing {
            sink: clip.sink,
            fade: clip.fade,
            fade_dur: STOP_FADE,
            deadline: Instant::now() + STOP_FADE + OUTGOING_SLACK,
        });
    }

    /// End the session: the clip in hand as `end`, then main is given back
    /// when `restore` and the promise allows ([`State::restore_main`]).
    fn finish_aside(&mut self, end: AsideEnd, restore: bool) {
        self.end_clip(end);
        let Some(aside) = self.aside.take() else { return };
        if restore {
            self.restore_main(aside.promise);
        }
    }

    /// Main always wins. Every main transport command calls this first,
    /// under the state lock: the clip in hand ends as `Taken`, the session
    /// ends, and nothing is given back. Without a session it is a `None`
    /// check, so an engine that never plays a clip behaves as before.
    pub(super) fn end_aside_for_main(&mut self) {
        if self.aside.is_some() {
            etrace!("a main command takes over from the aside clip");
            self.finish_aside(AsideEnd::Taken, false);
        }
    }

    /// Give main back, when the session paused it and nobody has touched
    /// it since: still that track, not stopped, paused or its soft pause
    /// still landing. Every main command ends the session first, so
    /// "nobody touched it" holds by construction; the file is the seatbelt
    /// the phone's `resumesSong` keys on.
    fn restore_main(&mut self, promise: Promise) {
        let Promise::Resume { file } = promise else { return };
        let paused = self.sink.is_paused() || self.pausing.is_some();
        if !self.stopped && self.current_file == file && paused {
            etrace!("the aside session gives main back");
            self.resume_now();
        }
    }

    /// Main crossed a gapless seam from `left` ([`State::promote_appended`]):
    /// a promise to give `left` back now names the track that sounds. The
    /// crossing can land inside a soft pause's ramp, after the promise.
    pub(super) fn aside_follow_seam(&mut self, left: &str) {
        if let Some(Aside { promise: Promise::Resume { file }, .. }) = &mut self.aside {
            if file == left {
                file.clone_from(&self.current_file);
            }
        }
    }

    /// Decide what the session owes main, the moment its first clip has
    /// opened: main audibly playing is paused in place, and owed a resume.
    /// Paused, stopped, ramping into a pause or run dry, it is left alone
    /// and owed nothing.
    fn pause_main_for_clip(&mut self) -> Promise {
        // The seam first, as every mutator does: the tick reaches here
        // before its own promotion, and the pause and the promise are for
        // the track that sounds.
        self.promote_if_crossed();
        let audible = !self.stopped
            && !self.sink.empty()
            && !self.sink.is_paused()
            && self.pausing.is_none();
        if !audible {
            return Promise::Nothing;
        }
        self.pause_now();
        Promise::Resume { file: self.current_file.clone() }
    }

    /// Whether the clip leaves the engine at rest: no session, or a paused
    /// clip, which lets the device sleep as a landed pause does. A clip
    /// that opens, sounds or holds main keeps the tick brisk.
    pub(super) fn aside_quiet(&self) -> bool {
        match &self.aside {
            None => true,
            Some(aside) => matches!(aside.phase, Phase::Sounding { paused: true, .. }),
        }
    }

    /// Drop a clip's breath once nothing audible remains of it — the rule
    /// main's [`State::retire_outgoing`] keeps.
    fn reap_aside_drains(&mut self) {
        self.aside_drains.retain(|out| {
            let spent = out.sink.empty()
                || out.fade.position() <= 0.0
                || (!out.sink.is_paused() && Instant::now() >= out.deadline);
            if spent {
                out.sink.stop();
            }
            !spent
        });
    }

    /// The listener's volume reaches the clip and its breaths.
    pub(super) fn aside_volume(&mut self, volume: f32) {
        if let Some(Aside { phase: Phase::Sounding { clip, .. }, .. }) = &self.aside {
            clip.sink.set_volume(volume);
        }
        for out in &self.aside_drains {
            out.sink.set_volume(volume);
        }
    }

    /// One tick of the clip, from [`Engine::advance_tick`] with the state
    /// lock held: reap the breaths, install an open that has answered, and
    /// end a clip that ran out, broke off, stalled or lost its device.
    /// Nothing here waits.
    pub(super) fn aside_step(&mut self, bus: &bus::Bus, device: Device, now: Instant) {
        self.reap_aside_drains();
        let step = match self.aside.as_mut() {
            None => return,
            Some(aside) => aside.look(device, now),
        };
        match step {
            Step::Wait => {}
            Step::Install(prepared) => self.install_clip(bus, prepared, now),
            // Not the clip's fault, and main comes back: when a device
            // returns the listener hears their music, not half a preview.
            Step::Refuse(why) => self.finish_aside(AsideEnd::Refused(why.into()), true),
            Step::Fail(why) => self.clip_over(AsideEnd::Failed(why), now),
            Step::RanOut => self.clip_over(AsideEnd::RanOut, now),
            // A held session has no clip in hand, so the end is unused.
            Step::Expire => self.finish_aside(AsideEnd::Stopped, true),
        }
    }

    /// Put an opened clip on the bus. The session's first clip decides the
    /// promise here, so main pauses in the same tick the clip starts: a
    /// slow CDN never leaves the listener in silence.
    fn install_clip(&mut self, bus: &bus::Bus, prepared: Prepared, now: Instant) {
        if matches!(self.aside.as_ref().map(|aside| &aside.promise), Some(Promise::Undecided)) {
            let promise = self.pause_main_for_clip();
            if let Some(aside) = self.aside.as_mut() {
                aside.promise = promise;
            }
        }
        let sink = bus.connect();
        sink.set_volume(self.volume);
        // No tap: two sources must never share the visualizer's ring, and
        // a clip is never a play.
        let (fade, _, feed) = attach(&sink, prepared.source, None, 1.0);
        let clip = Clip { sink: Arc::new(sink), fade, feed, duration: prepared.duration };
        if let Some(aside) = self.aside.as_mut() {
            etrace!("aside {}: sounding ({:.1}s)", aside.ticket, clip.duration);
            aside.phase = Phase::Sounding { clip, paused: false, moved: (now, 0.0) };
        }
    }

    /// The clip in hand is over by its own doing: it ran out, or failed.
    /// A single press gives main back at once. A chain (`Hold`) keeps main
    /// paused for [`ASIDE_HOLD`], so the next clip takes over without main
    /// sounding in between; a failed link in a chain is passed over the
    /// same way.
    fn clip_over(&mut self, end: AsideEnd, now: Instant) {
        let hold = self.aside.as_ref().is_some_and(|aside| aside.then == AsideThen::Hold);
        if !hold {
            self.finish_aside(end, true);
            return;
        }
        self.end_clip(end);
        if let Some(aside) = self.aside.as_mut() {
            aside.phase = Phase::Held { until: now + ASIDE_HOLD };
        }
    }
}

impl Engine {
    /// Play `url` aside of main, as the clip named `ticket`. Nothing here
    /// waits on the network: the open starts on its own thread, and the
    /// tick installs it. A clip already in hand ends as `Superseded`, and a
    /// held session is claimed, so its promise carries on. Main is not
    /// touched here: it pauses once the clip has opened.
    ///
    /// An `Err` is a refusal, never the clip's fault, and leaves the
    /// session as it was: a path or a `file:` URL, an announced outage, or
    /// a box that cannot start a thread. The URL is used verbatim, with no
    /// stream-URL builder and no token.
    pub fn play_aside(&self, ticket: u64, url: &str, then: AsideThen) -> Result<(), String> {
        if !http::is_http_url(url) {
            return Err("not an http(s) URL".into());
        }
        if self.output.lock().unwrap().no_output() {
            return Err("no output device".into());
        }
        self.wake_output();
        let opener = spawn_open(QueueEntry::new(url.to_string()))
            .map_err(|e| format!("the clip's open would not start: {e}"))?;
        etrace!("aside {ticket}: open {} ({then:?})", http::redact_source(url));
        let phase = Phase::Opening { opener, since: Instant::now() };
        let mut s = self.state.lock().unwrap();
        s.end_clip(AsideEnd::Superseded);
        match s.aside.as_mut() {
            Some(aside) => {
                aside.ticket = ticket;
                aside.then = then;
                aside.phase = phase;
            }
            None => s.aside = Some(Aside { ticket, then, phase, promise: Promise::Undecided }),
        }
        Ok(())
    }

    /// Pause the sounding clip named `ticket`, at once. Main stays paused.
    /// Any other state: nothing.
    pub fn pause_aside(&self, ticket: u64) {
        let mut s = self.state.lock().unwrap();
        if let Some(Aside { ticket: live, phase: Phase::Sounding { clip, paused, .. }, .. }) =
            s.aside.as_mut()
        {
            if *live == ticket && !*paused {
                clip.sink.pause();
                *paused = true;
            }
        }
    }

    /// Resume the paused clip named `ticket`. A long clip pause may have
    /// let the device sleep, so it wakes first; the stall clock starts
    /// over.
    pub fn resume_aside(&self, ticket: u64) {
        self.wake_output();
        let mut s = self.state.lock().unwrap();
        if let Some(Aside { ticket: live, phase: Phase::Sounding { clip, paused, moved }, .. }) =
            s.aside.as_mut()
        {
            if *live == ticket && *paused {
                clip.sink.play();
                *paused = false;
                *moved = (Instant::now(), clip.feed.position());
            }
        }
    }

    /// End the clip session: `None` whatever runs (the room leaving),
    /// `Some` only when it names the session's clip. The clip in hand ends
    /// as `Stopped`, and main comes back when `restore` and the session
    /// paused it.
    pub fn stop_aside(&self, ticket: Option<u64>, restore: bool) {
        if restore {
            self.wake_output();
        }
        let mut s = self.state.lock().unwrap();
        let named = s.aside.as_ref().is_some_and(|aside| ticket.is_none_or(|t| t == aside.ticket));
        if named {
            s.finish_aside(AsideEnd::Stopped, restore);
        }
    }

    /// End the clip in hand as `Superseded` and hold main paused for
    /// `span`, at most [`ASIDE_HOLD_MOST`], while the driver finds the next
    /// clip. Without a session, nothing: a first press while main plays
    /// leaves main playing until its clip has opened.
    pub fn hold_aside(&self, span: Duration) {
        let mut s = self.state.lock().unwrap();
        if s.aside.is_none() {
            return;
        }
        s.end_clip(AsideEnd::Superseded);
        if let Some(aside) = s.aside.as_mut() {
            aside.phase = Phase::Held { until: Instant::now() + span.min(ASIDE_HOLD_MOST) };
        }
    }

    /// The clip session as it stands; `None` without one.
    pub fn aside_status(&self) -> Option<AsideStatus> {
        self.state.lock().unwrap().aside.as_ref().map(Aside::status)
    }

    /// How each clip ended since the last call, oldest first.
    pub fn take_aside_ends(&self) -> Vec<AsideEnded> {
        std::mem::take(&mut self.state.lock().unwrap().aside_ends)
    }
}

#[cfg(test)]
mod tests {
    //! Clips really open, sound, run out, stall and break here: the engine
    //! is on a stage (`output::stage`), whose devices pull the bus at a
    //! device's pace, and the clips come from local servers. Clips are one
    //! to four seconds of WAV, and the test build's timers are short.

    use super::super::tests::{
        bare_state, counting_wav_server, held_wav_server, settled_somewhere, stalled_url,
        staged, tick_for, tick_until, wav_bytes,
    };
    use super::*;
    use crate::engine::EngineError;

    /// A server that answers every request with a whole WAV clip.
    fn clip(seconds: usize) -> String {
        counting_wav_server(seconds).0
    }

    /// A server that answers every request with a 404, as an expired
    /// preview URL does.
    fn missing_clip() -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut head = [0u8; 2048];
                    let _ = stream.read(&mut head);
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\
                          Connection: close\r\n\r\n",
                    );
                });
            }
        });
        format!("http://{addr}/gone.mp3")
    }

    /// A server that promises a WAV of `seconds`, sends the first `sent`
    /// of them and hangs up, and refuses to take it up again: a CDN that
    /// cut the stream.
    fn cut_clip(seconds: usize, sent: f64) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = Arc::new(wav_bytes(seconds));
        let cut = 44 + (sent * 44_100.0) as usize * 4;
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let body = body.clone();
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut head = [0u8; 2048];
                    let read = stream.read(&mut head).unwrap_or(0);
                    let asked = String::from_utf8_lossy(&head[..read]).to_ascii_lowercase();
                    if asked.contains("range:") {
                        let _ = stream.write_all(
                            b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\
                              Connection: close\r\n\r\n",
                        );
                        return;
                    }
                    let sent = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(sent.as_bytes());
                    let _ = stream.write_all(&body[..cut]);
                });
            }
        });
        format!("http://{addr}/cut.wav")
    }

    /// `seconds` of WAV at a path of this test's own, for main.
    fn track(name: &str, seconds: usize) -> String {
        let path =
            std::env::temp_dir().join(format!("mstream-aside-{}-{name}.wav", std::process::id()));
        std::fs::write(&path, wav_bytes(seconds)).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn phase(engine: &Engine) -> Option<AsidePhase> {
        engine.aside_status().map(|status| status.phase)
    }

    /// Tick until the clip named `ticket` is in `want`.
    fn until_clip(engine: &Engine, ticket: u64, want: AsidePhase) {
        tick_until(engine, |e, _| {
            e.aside_status().is_some_and(|status| status.ticket == ticket && status.phase == want)
        });
    }

    /// Tick until a clip ends, and say how. Ends are collected as they
    /// come, so none is lost to the wait.
    fn until_ended(engine: &Engine, ends: &mut Vec<AsideEnded>) -> AsideEnded {
        let before = ends.len();
        tick_until(engine, |e, _| {
            ends.extend(e.take_aside_ends());
            ends.len() > before
        });
        ends[before].clone()
    }

    fn ended(ticket: u64, end: AsideEnd) -> AsideEnded {
        AsideEnded { ticket, end }
    }

    /// Main playing `track`, and under way.
    fn main_playing(engine: &Engine, track: &str) {
        engine.play_source(track.to_string(), None).unwrap();
        tick_for(engine, Duration::from_millis(300));
        assert!(engine.status().playing, "main plays");
    }

    /// A clip named `ticket` over a playing main, until it sounds and main
    /// has paused for it.
    fn clip_over_main(engine: &Engine, ticket: u64, url: &str) {
        assert!(engine.status().playing, "main plays before the clip");
        engine.play_aside(ticket, url, AsideThen::Restore).unwrap();
        until_clip(engine, ticket, AsidePhase::Playing);
        let main = engine.status();
        assert!(main.paused && !main.playing, "main paused for the clip");
    }

    /// The only end since the last look is `ticket`'s, as `Taken`, and the
    /// session is over.
    fn taken(engine: &Engine, ticket: u64) {
        assert_eq!(engine.take_aside_ends(), [ended(ticket, AsideEnd::Taken)]);
        assert_eq!(engine.aside_status(), None, "the session is over");
    }

    /// A clip as the bookkeeping sees it: a sink with a sound in it that
    /// nothing pulls.
    fn bare_clip() -> Clip {
        let sink = Player::new().0;
        sink.append(rodio::source::Zero::new(
            std::num::NonZero::new(2).unwrap(),
            std::num::NonZero::new(44_100).unwrap(),
        ));
        Clip {
            sink: Arc::new(sink),
            fade: fade::FadeHandle::new(1.0),
            feed: feed::Feed::idle(),
            duration: 30.0,
        }
    }

    #[test]
    fn a_clip_over_a_playing_main_pauses_it_and_its_stop_resumes_it_where_it_stood() {
        let (engine, _stage) = staged("Speakers");
        let main = track("resume", 20);
        main_playing(&engine, &main);
        clip_over_main(&engine, 1, &clip(4));
        tick_for(&engine, Duration::from_millis(100));
        let held = engine.status();
        assert_eq!(held.file, main, "main is still main");
        tick_for(&engine, Duration::from_millis(500));
        let still = engine.status();
        assert!(still.paused, "main stays paused while the clip plays");
        assert!(
            (still.position - held.position).abs() < 0.05,
            "main stood still: {:.2} then {:.2}",
            held.position,
            still.position
        );
        let sounding = engine.aside_status().unwrap();
        assert!(sounding.position > 0.2, "the clip runs: {:.2}", sounding.position);
        assert!((sounding.duration - 4.0).abs() < 0.1, "its length: {:.2}", sounding.duration);

        engine.stop_aside(Some(1), true);
        assert_eq!(engine.take_aside_ends(), [ended(1, AsideEnd::Stopped)]);
        assert_eq!(engine.aside_status(), None);
        let back = engine.status();
        assert!(back.playing, "main is given back at once");
        assert!((back.position - held.position).abs() < 0.1, "where it stood");
        tick_for(&engine, Duration::from_millis(400));
        assert!(engine.status().position > held.position + 0.2, "and it runs on");
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn a_soft_pause_rides_its_ramp_under_the_clip_and_is_given_back() {
        // With the pause fade on, the clip's pause of main is the listener's
        // own soft pause: a ramp, landed by the tick.
        let (engine, _stage) = staged("Speakers");
        engine.set_pause_fade(true);
        let main = track("soft", 20);
        main_playing(&engine, &main);
        engine.play_aside(1, &clip(4), AsideThen::Restore).unwrap();
        tick_until(&engine, |e, _| {
            e.state.lock().unwrap().pausing.is_some() || e.status().paused
        });
        tick_until(&engine, |e, _| e.status().paused);
        assert!(engine.state.lock().unwrap().pausing.is_none(), "the tick landed the pause");
        engine.stop_aside(None, true);
        let back = engine.status();
        assert!(back.playing, "main is given back");
        assert!(engine.state.lock().unwrap().fade.position() < 1.0, "on a ramp up");
        tick_for(&engine, Duration::from_millis(300));
        assert!(engine.state.lock().unwrap().fade.position() > 0.99, "to full");
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn a_clip_over_a_paused_or_stopped_main_restores_nothing() {
        let (engine, _stage) = staged("Speakers");
        let main = track("paused", 20);
        main_playing(&engine, &main);
        engine.pause();
        engine.play_aside(1, &clip(4), AsideThen::Restore).unwrap();
        until_clip(&engine, 1, AsidePhase::Playing);
        engine.stop_aside(None, true);
        tick_for(&engine, Duration::from_millis(300));
        let paused = engine.status();
        assert!(paused.paused && !paused.playing, "a paused main stays paused");
        assert_eq!(paused.file, main);

        engine.stop();
        engine.play_aside(2, &clip(4), AsideThen::Restore).unwrap();
        until_clip(&engine, 2, AsidePhase::Playing);
        engine.stop_aside(Some(2), true);
        tick_for(&engine, Duration::from_millis(300));
        let stopped = engine.status();
        assert!(!stopped.playing && stopped.file.is_empty(), "a stopped main stays stopped");
        assert_eq!(
            engine.take_aside_ends(),
            [ended(1, AsideEnd::Stopped), ended(2, AsideEnd::Stopped)]
        );
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn main_pauses_only_once_the_clip_has_opened() {
        let (engine, _stage) = staged("Speakers");
        let main = track("open-first", 20);
        main_playing(&engine, &main);
        // The open is held back until the server is told to go on.
        let (url, release) = held_wav_server(2, 1000);
        engine.play_aside(1, &url, AsideThen::Restore).unwrap();
        let from = engine.status().position;
        tick_for(&engine, Duration::from_millis(600));
        assert_eq!(phase(&engine), Some(AsidePhase::Loading));
        let loading = engine.status();
        assert!(loading.playing, "main plays on while the clip loads");
        assert!(loading.position > from + 0.2, "and its music moves");

        release.store(true, std::sync::atomic::Ordering::Release);
        until_clip(&engine, 1, AsidePhase::Playing);
        let opened = engine.status();
        assert!(opened.paused, "main pauses in the tick the clip opens");
        engine.stop_aside(None, true);
        assert!(engine.status().playing);
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn a_second_clip_carries_the_first_ones_promise() {
        let (engine, _stage) = staged("Speakers");
        let main = track("carry", 20);
        main_playing(&engine, &main);
        let url = clip(4);
        clip_over_main(&engine, 1, &url);
        engine.play_aside(2, &url, AsideThen::Restore).unwrap();
        assert_eq!(engine.take_aside_ends(), [ended(1, AsideEnd::Superseded)]);
        until_clip(&engine, 2, AsidePhase::Playing);
        assert!(engine.status().paused, "main is still paused for the session");

        // A stop that names the old clip is not this one's.
        engine.stop_aside(Some(1), true);
        assert_eq!(phase(&engine), Some(AsidePhase::Playing));
        assert!(engine.status().paused);

        // The promise made when main paused is the second clip's to keep.
        engine.stop_aside(Some(2), true);
        assert_eq!(engine.take_aside_ends(), [ended(2, AsideEnd::Stopped)]);
        assert!(engine.status().playing, "main comes back once, at the session's end");
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn main_transport_ends_the_clip_as_taken_and_restores_nothing() {
        let (engine, _stage) = staged("Speakers");
        let (a, b, c) = (track("taken-a", 20), track("taken-b", 20), track("taken-c", 20));
        let url = clip(4);
        // Each case ticks past the clip's whole length afterwards: a
        // session that owed main a resume would have given it by then.
        let past = Duration::from_millis(400);

        // Play.
        main_playing(&engine, &a);
        clip_over_main(&engine, 1, &url);
        engine.play_source(b.clone(), None).unwrap();
        taken(&engine, 1);
        tick_for(&engine, past);
        assert!(engine.status().playing && engine.status().file == b, "the new track plays");

        // Pause: main stays paused, and nothing resumes it later.
        clip_over_main(&engine, 2, &url);
        engine.pause();
        taken(&engine, 2);
        tick_for(&engine, past);
        assert!(engine.status().paused, "pause keeps main paused");

        // Resume.
        engine.resume();
        tick_for(&engine, Duration::from_millis(100));
        clip_over_main(&engine, 3, &url);
        engine.resume();
        taken(&engine, 3);
        assert!(engine.status().playing, "resume plays main");

        // Stop.
        clip_over_main(&engine, 4, &url);
        engine.stop();
        taken(&engine, 4);
        tick_for(&engine, past);
        assert!(!engine.status().playing && engine.status().file.is_empty(), "stopped");

        // Seek: main stays paused at its new place.
        main_playing(&engine, &a);
        clip_over_main(&engine, 5, &url);
        engine.seek(8.0).unwrap();
        taken(&engine, 5);
        tick_for(&engine, past);
        assert!(engine.status().paused, "a seek leaves main paused");

        // Serve's queue: next, previous, a row played, the queue cleared.
        engine.queue_clear();
        engine.queue_add_many(vec![a.clone(), b.clone(), c.clone()]);
        tick_for(&engine, Duration::from_millis(300));
        assert_eq!(engine.status().file, a);
        clip_over_main(&engine, 6, &url);
        engine.next_manual().unwrap();
        taken(&engine, 6);
        assert_eq!(engine.status().file, b, "next plays the next row");
        tick_for(&engine, Duration::from_millis(200));
        clip_over_main(&engine, 7, &url);
        engine.previous_manual().unwrap();
        taken(&engine, 7);
        assert_eq!(engine.status().file, a, "previous plays the row before");
        tick_for(&engine, Duration::from_millis(200));
        clip_over_main(&engine, 8, &url);
        engine.queue_play_index(2).unwrap();
        taken(&engine, 8);
        assert_eq!(engine.status().file, c, "the row asked for plays");
        tick_for(&engine, Duration::from_millis(200));
        clip_over_main(&engine, 9, &url);
        engine.queue_clear();
        taken(&engine, 9);
        tick_for(&engine, past);
        assert!(!engine.status().playing, "a cleared queue plays nothing");
        for path in [a, b, c] {
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn a_seek_during_a_clip_leaves_main_paused_at_the_new_place() {
        let (engine, _stage) = staged("Speakers");
        let main = track("seek", 20);
        main_playing(&engine, &main);
        clip_over_main(&engine, 1, &clip(4));
        engine.seek(10.0).unwrap();
        taken(&engine, 1);
        let sought = engine.status();
        assert!(sought.paused, "main is paused, as the bar showed it");
        assert!((sought.position - 10.0).abs() < 0.3, "at the new place: {:.2}", sought.position);
        tick_for(&engine, Duration::from_millis(500));
        let later = engine.status();
        assert!(later.paused && (later.position - sought.position).abs() < 0.05, "and stays there");
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn a_run_out_with_restore_brings_main_back_at_once() {
        let (engine, _stage) = staged("Speakers");
        let main = track("run-out", 20);
        main_playing(&engine, &main);
        clip_over_main(&engine, 1, &clip(1));
        let mut ends = Vec::new();
        assert_eq!(until_ended(&engine, &mut ends), ended(1, AsideEnd::RanOut));
        // In the same tick the end was noted.
        assert!(engine.status().playing, "main is back the moment the clip ran out");
        assert_eq!(engine.aside_status(), None);
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    /// Main's queue `first`, `second` with gapless on, ticked until
    /// `second` sits appended in the sink behind `first`.
    fn appended_behind(engine: &Engine, first: &str, second: &str) {
        engine.set_gapless(true);
        engine.queue_add_many(vec![first.to_string(), second.to_string()]);
        tick_until(engine, |e, _| e.appended_waiting());
    }

    /// Wait, with no tick and no status (either would promote), until
    /// `ready` holds of the state; it fails after five seconds.
    fn unticked_until(engine: &Engine, mut ready: impl FnMut(&State) -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        while !ready(&engine.state.lock().unwrap()) {
            assert!(Instant::now() < until, "the state never got there");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_clip_that_opens_just_past_a_gapless_seam_gives_back_the_track_that_sounds() {
        // The seam crossed into `second`, and no status or tick has caught
        // the bookkeeping up yet, when the clip's open is installed.
        let (engine, _stage) = staged("Speakers");
        let (first, second) = (track("seam-a", 3), track("seam-b", 20));
        appended_behind(&engine, &first, &second);
        engine.play_aside(1, &clip(1), AsideThen::Restore).unwrap();
        unticked_until(&engine, |s| s.appended.is_some() && s.sink.len() <= 1);
        // The open, from a server on this machine, has answered by now.
        std::thread::sleep(Duration::from_millis(300));
        engine.advance_tick();
        assert_eq!(phase(&engine), Some(AsidePhase::Playing), "installed on the crossed seam");
        let held = engine.status();
        assert!(held.paused && held.file == second, "the track that sounds is paused");
        let mut ends = Vec::new();
        assert_eq!(until_ended(&engine, &mut ends), ended(1, AsideEnd::RanOut));
        let back = engine.status();
        assert!(back.playing, "main is given back");
        assert_eq!(back.file, second);
        engine.stop();
        for path in [first, second] {
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn a_seam_that_crosses_under_the_soft_pause_still_gives_main_back() {
        // The clip opens a moment before the seam, and the seam crosses
        // while main's soft pause is still ramping down.
        let (engine, _stage) = staged("Speakers");
        engine.set_pause_fade(true);
        let (first, second) = (track("ramp-a", 3), track("ramp-b", 20));
        appended_behind(&engine, &first, &second);
        engine.play_aside(1, &clip(2), AsideThen::Restore).unwrap();
        unticked_until(&engine, |s| {
            assert!(s.sink.len() >= 2, "the seam crossed before the clip could open");
            s.duration - s.feed.position() < 0.06
        });
        engine.advance_tick();
        assert_eq!(phase(&engine), Some(AsidePhase::Playing), "the clip went on");
        {
            let s = engine.state.lock().unwrap();
            assert!(s.pausing.is_some() && s.current_file == first, "first ramps into a pause");
        }
        tick_until(&engine, |e, _| e.state.lock().unwrap().current_file == second);
        {
            let s = engine.state.lock().unwrap();
            assert!(s.pausing.is_some(), "the seam crossed under the ramp");
        }
        tick_for(&engine, Duration::from_millis(80));
        {
            let s = engine.state.lock().unwrap();
            assert!(
                s.sink.is_paused() || s.fade.position() < 1.0,
                "second rides the ramp down, not full under the clip: {:.2}",
                s.fade.position()
            );
        }
        let mut ends = Vec::new();
        assert_eq!(until_ended(&engine, &mut ends), ended(1, AsideEnd::RanOut));
        let back = engine.status();
        assert!(back.playing, "main is given back");
        assert_eq!(back.file, second);
        engine.stop();
        for path in [first, second] {
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn a_held_run_out_restores_main_after_the_hold_unless_a_clip_follows() {
        let (engine, _stage) = staged("Speakers");
        let main = track("held", 20);
        main_playing(&engine, &main);
        let url = clip(1);
        engine.play_aside(1, &url, AsideThen::Hold).unwrap();
        until_clip(&engine, 1, AsidePhase::Playing);
        let mut ends = Vec::new();
        assert_eq!(until_ended(&engine, &mut ends), ended(1, AsideEnd::RanOut));
        assert_eq!(phase(&engine), Some(AsidePhase::Held), "the session holds");
        assert!(engine.status().paused, "and main with it");

        // The next clip claims the hold: main never sounds in between.
        engine.play_aside(2, &url, AsideThen::Hold).unwrap();
        tick_until(&engine, |e, _| {
            assert!(e.status().paused, "main sounded between two clips of a chain");
            e.aside_status().is_some_and(|s| s.ticket == 2 && s.phase == AsidePhase::Playing)
        });
        assert_eq!(until_ended(&engine, &mut ends), ended(2, AsideEnd::RanOut));
        let ran_out = Instant::now();

        // Nothing follows: the hold ends, and main is given back.
        tick_until(&engine, |e, _| {
            let over = e.aside_status().is_none();
            if !over {
                assert!(e.status().paused, "main stays paused through the hold");
            }
            over
        });
        assert!(ran_out.elapsed() >= ASIDE_HOLD - Duration::from_millis(50), "held its while");
        assert!(engine.status().playing, "main is back after the hold");
        assert_eq!(engine.take_aside_ends(), [], "a hold has no clip to end");
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn a_hold_stops_the_clip_keeps_main_paused_and_is_capped() {
        let (engine, _stage) = staged("Speakers");
        let main = track("hold", 20);

        // Without a session a hold is nothing: main is not paused for it.
        main_playing(&engine, &main);
        engine.hold_aside(Duration::from_secs(1));
        assert_eq!(engine.aside_status(), None);
        assert!(engine.status().playing);

        let url = clip(4);
        clip_over_main(&engine, 1, &url);
        engine.hold_aside(Duration::from_millis(600));
        let held = Instant::now();
        assert_eq!(engine.take_aside_ends(), [ended(1, AsideEnd::Superseded)]);
        assert_eq!(phase(&engine), Some(AsidePhase::Held));
        tick_for(&engine, Duration::from_millis(300));
        assert!(engine.status().paused, "main is held paused");
        tick_until(&engine, |e, _| e.aside_status().is_none());
        assert!(held.elapsed() >= Duration::from_millis(550), "for the span asked");
        assert!(engine.status().playing, "then given back");

        // A span past the cap is held for the cap.
        clip_over_main(&engine, 2, &url);
        engine.hold_aside(Duration::from_secs(3600));
        let until = match &engine.state.lock().unwrap().aside {
            Some(Aside { phase: Phase::Held { until }, .. }) => *until,
            _ => panic!("not held"),
        };
        assert!(until <= Instant::now() + ASIDE_HOLD_MOST, "capped at {ASIDE_HOLD_MOST:?}");
        assert!(until > Instant::now() + ASIDE_HOLD_MOST - Duration::from_secs(5));
        engine.stop_aside(None, true);
        assert_eq!(engine.take_aside_ends(), [ended(2, AsideEnd::Superseded)]);
        assert!(engine.status().playing);
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn a_clip_that_will_not_open_is_failed_and_main_is_untouched() {
        let (engine, _stage) = staged("Speakers");
        let main = track("404", 20);
        main_playing(&engine, &main);
        engine.play_aside(1, &missing_clip(), AsideThen::Restore).unwrap();
        let mut ends = Vec::new();
        tick_until(&engine, |e, _| {
            assert!(e.status().playing, "main never paused for a clip that never opened");
            ends.extend(e.take_aside_ends());
            !ends.is_empty()
        });
        assert!(
            matches!(&ends[..], [AsideEnded { ticket: 1, end: AsideEnd::Failed(_) }]),
            "{ends:?}"
        );
        assert_eq!(engine.aside_status(), None);
        assert!(engine.status().playing);
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn an_open_that_never_answers_is_failed_after_its_patience() {
        let (engine, _stage) = staged("Speakers");
        let main = track("patience", 20);
        main_playing(&engine, &main);
        let asked = Instant::now();
        engine.play_aside(1, &stalled_url(), AsideThen::Restore).unwrap();
        let mut ends = Vec::new();
        tick_until(&engine, |e, _| {
            assert!(e.status().playing, "main plays on while the clip will not load");
            ends.extend(e.take_aside_ends());
            !ends.is_empty()
        });
        assert_eq!(ends, [ended(1, AsideEnd::Failed("the clip did not load".into()))]);
        let waited = asked.elapsed();
        assert!(waited >= ASIDE_OPEN_PATIENCE, "gave up early: {waited:?}");
        assert!(waited < ASIDE_OPEN_PATIENCE + Duration::from_secs(3), "took {waited:?}");
        assert!(engine.status().playing);
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn a_clip_that_breaks_off_early_is_failed_not_ran_out() {
        let (engine, _stage) = staged("Speakers");
        engine.play_aside(1, &cut_clip(10, 0.8), AsideThen::Restore).unwrap();
        until_clip(&engine, 1, AsidePhase::Playing);
        let mut ends = Vec::new();
        assert_eq!(
            until_ended(&engine, &mut ends),
            ended(1, AsideEnd::Failed("the clip broke off".into()))
        );
    }

    #[test]
    fn a_stalled_clip_is_failed_but_not_while_the_device_is_away() {
        // The download stops a second and a half in, and never comes back.
        let (engine, _stage) = staged("Speakers");
        let (url, _never) = held_wav_server(10, 44 + 44_100 * 4 * 3 / 2);
        engine.play_aside(1, &url, AsideThen::Restore).unwrap();
        until_clip(&engine, 1, AsidePhase::Playing);
        let sounding = Instant::now();
        let mut ends = Vec::new();
        assert_eq!(
            until_ended(&engine, &mut ends),
            ended(1, AsideEnd::Failed("the clip stalled".into()))
        );
        assert!(sounding.elapsed() >= ASIDE_STALL + Duration::from_secs(1), "not before it stood");

        // The device stops asking for sound, for longer than a stall, and
        // comes back: the clip stood still for the device, not of itself.
        let (engine, stage) = staged("Speakers");
        engine.play_aside(2, &clip(3), AsideThen::Restore).unwrap();
        until_clip(&engine, 2, AsidePhase::Playing);
        tick_for(&engine, Duration::from_millis(300));
        stage.go_quiet();
        let quiet = Instant::now();
        let watched = tick_until(&engine, settled_somewhere);
        assert!(!watched.said().is_empty());
        assert!(quiet.elapsed() >= ASIDE_STALL, "the device was away longer than a stall");
        let mut ends = Vec::new();
        assert_eq!(until_ended(&engine, &mut ends), ended(2, AsideEnd::RanOut));
    }

    #[test]
    fn a_path_or_a_file_url_is_refused_without_an_open() {
        let (engine, _stage) = staged("Speakers");
        for (ticket, url) in
            [(1, "/tmp/clip.mp3"), (2, "file:///tmp/clip.mp3"), (3, "ftp://example.com/c.mp3")]
        {
            let refused = engine.play_aside(ticket, url, AsideThen::Restore);
            assert!(refused.is_err(), "{url} was taken");
        }
        assert_eq!(engine.aside_status(), None, "no session began");
        assert_eq!(engine.take_aside_ends(), [], "a refusal is the caller's to report");
        assert!(engine.settled(), "and nothing is in flight");
    }

    #[test]
    fn an_outage_refuses_the_clip_and_restores_main() {
        let (engine, stage) = staged("Speakers");
        let main = track("outage", 20);
        main_playing(&engine, &main);
        clip_over_main(&engine, 1, &clip(4));
        stage.refuse(Some("the endpoint is gone"));
        stage.unplug();
        let mut ends = Vec::new();
        assert_eq!(
            until_ended(&engine, &mut ends),
            ended(1, AsideEnd::Refused("the output device was lost".into()))
        );
        let main_now = engine.status();
        assert!(main_now.playing && main_now.file == main, "main is given back into the outage");
        assert_eq!(engine.aside_status(), None);
        // A clip asked for in the outage is refused at once.
        assert_eq!(
            engine.play_aside(2, &clip(1), AsideThen::Restore),
            Err("no output device".to_string())
        );
        assert_eq!(engine.aside_status(), None);

        // A device answers, and the listener hears their music.
        stage.refuse(None);
        tick_until(&engine, settled_somewhere);
        let stood = engine.status().position;
        tick_for(&engine, Duration::from_millis(400));
        assert!(engine.status().position > stood + 0.1, "main sounds again");
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn a_default_move_carries_the_clip_on_from_where_it_stood() {
        let (engine, stage) = staged("Speakers");
        engine.play_aside(1, &clip(4), AsideThen::Restore).unwrap();
        until_clip(&engine, 1, AsidePhase::Playing);
        tick_for(&engine, Duration::from_millis(400));
        let before = engine.aside_status().unwrap().position;
        stage.set_default(Some("Headphones"));
        let watched = tick_until(&engine, settled_somewhere);
        assert_eq!(watched.said(), ["audio moved to Headphones"]);
        let after = engine.aside_status().unwrap();
        assert_eq!((after.ticket, after.phase), (1, AsidePhase::Playing), "the clip plays on");
        assert!(after.position >= before, "from where it stood");
        tick_for(&engine, Duration::from_millis(300));
        assert!(engine.aside_status().unwrap().position > after.position, "and runs on");
        assert_eq!(engine.take_aside_ends(), [], "nothing ended");
    }

    #[test]
    fn volume_reaches_the_clip_and_its_breath() {
        let (engine, _stage) = staged("Speakers");
        engine.set_volume(0.5);
        engine.play_aside(1, &clip(4), AsideThen::Restore).unwrap();
        until_clip(&engine, 1, AsidePhase::Playing);
        let clip_volume = |engine: &Engine| match &engine.state.lock().unwrap().aside {
            Some(Aside { phase: Phase::Sounding { clip, .. }, .. }) => clip.sink.volume(),
            _ => panic!("no clip sounding"),
        };
        assert_eq!(clip_volume(&engine), 0.5, "a clip opens at the listener's volume");
        engine.set_volume(0.25);
        assert_eq!(clip_volume(&engine), 0.25, "and follows the knob");

        engine.stop_aside(None, false);
        engine.set_volume(0.1);
        let s = engine.state.lock().unwrap();
        assert_eq!(s.aside_drains.len(), 1, "the clip is breathing out");
        assert_eq!(s.aside_drains[0].sink.volume(), 0.1, "and its breath follows the knob too");
    }

    #[test]
    fn the_clip_never_shows_in_main_status() {
        let (engine, _stage) = staged("Speakers");
        engine.play_aside(1, &clip(4), AsideThen::Restore).unwrap();
        until_clip(&engine, 1, AsidePhase::Playing);
        tick_for(&engine, Duration::from_millis(300));
        let idle = engine.status();
        assert!(!idle.playing && !idle.paused, "main plays nothing: {}", idle.playing);
        assert!(idle.file.is_empty() && idle.position == 0.0 && idle.duration == 0.0);
        engine.stop_aside(None, true);

        let main = track("status", 20);
        main_playing(&engine, &main);
        clip_over_main(&engine, 2, &clip(4));
        tick_for(&engine, Duration::from_millis(300));
        let status = engine.status();
        assert_eq!(status.file, main, "main's file, never the clip's");
        assert!((status.duration - 20.0).abs() < 0.1, "main's length");
        assert!(status.paused && !status.playing);
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn at_rest_is_false_while_a_clip_opens_sounds_or_holds_and_true_once_it_is_paused() {
        let mut s = bare_state();
        assert!(s.at_rest(), "no clip: at rest, as before");
        let (_answer, rx) = mpsc::channel();
        let opener = Opener { rx, thread: std::thread::spawn(|| {}) };
        let now = Instant::now();
        s.aside = Some(Aside {
            ticket: 1,
            then: AsideThen::Restore,
            phase: Phase::Opening { opener, since: now },
            promise: Promise::Undecided,
        });
        assert!(!s.at_rest(), "an opening clip is not at rest");
        let set = |s: &mut State, phase: Phase| s.aside.as_mut().unwrap().phase = phase;
        set(&mut s, Phase::Sounding { clip: bare_clip(), paused: false, moved: (now, 0.0) });
        assert!(!s.at_rest(), "a sounding clip is not");
        set(&mut s, Phase::Held { until: now });
        assert!(!s.at_rest(), "a hold is not");
        set(&mut s, Phase::Sounding { clip: bare_clip(), paused: true, moved: (now, 0.0) });
        assert!(s.at_rest(), "a paused clip is, as a landed pause is");
        s.breathe_out(bare_clip(), false);
        assert!(!s.at_rest(), "a clip breathing out is not");
    }

    #[test]
    fn a_paused_clip_lets_the_device_sleep_as_a_pause_does_and_resume_wakes_it() {
        let (engine, _stage) = staged("Speakers");
        engine.play_aside(1, &clip(4), AsideThen::Restore).unwrap();
        until_clip(&engine, 1, AsidePhase::Playing);
        tick_for(&engine, Duration::from_millis(200));
        assert!(!engine.settled(), "a sounding clip keeps the tick brisk");
        engine.pause_aside(1);
        let paused = Instant::now();
        assert_eq!(phase(&engine), Some(AsidePhase::Paused));
        assert!(engine.settled(), "a paused clip is at rest");
        tick_until(&engine, |e, _| !e.output_awake());
        // Over a stopped main, still the pause's grace and not the stop's.
        assert!(
            paused.elapsed() >= super::super::IDLE_STOPPED + Duration::from_millis(200),
            "slept after {:?}",
            paused.elapsed()
        );
        let stood = engine.aside_status().unwrap().position;
        engine.resume_aside(1);
        assert!(engine.output_awake(), "the resume wakes the device");
        tick_for(&engine, Duration::from_millis(400));
        let resumed = engine.aside_status().unwrap();
        assert_eq!(resumed.phase, AsidePhase::Playing);
        assert!(resumed.position > stood + 0.1, "and the clip runs on");
    }

    #[test]
    fn the_clip_breathes_out_instead_of_clicking() {
        let (engine, _stage) = staged("Speakers");
        let url = clip(4);
        engine.play_aside(1, &url, AsideThen::Restore).unwrap();
        until_clip(&engine, 1, AsidePhase::Playing);
        tick_for(&engine, Duration::from_millis(200));
        // A newer clip takes the first one's place with a breath.
        engine.play_aside(2, &url, AsideThen::Restore).unwrap();
        {
            let s = engine.state.lock().unwrap();
            assert_eq!(s.aside_drains.len(), 1, "the superseded clip breathes out");
            assert!(!s.aside_drains[0].sink.empty(), "still sounding its breath");
            assert!(s.outgoing.is_empty(), "main's drainers never see a clip");
        }
        until_clip(&engine, 2, AsidePhase::Playing);
        tick_for(&engine, Duration::from_millis(200));
        engine.stop_aside(None, false);
        let stopped = Instant::now();
        {
            let s = engine.state.lock().unwrap();
            let breath = s.aside_drains.last().expect("the stopped clip breathes out");
            assert!(breath.fade.position() > 0.5, "not cut: {}", breath.fade.position());
        }
        tick_until(&engine, |e, _| e.state.lock().unwrap().aside_drains.is_empty());
        assert!(stopped.elapsed() < Duration::from_secs(2), "the breath ends");
        tick_until(&engine, |e, _| e.settled());
    }

    #[test]
    fn every_ticket_ends_exactly_once() {
        let (engine, _stage) = staged("Speakers");
        let main = track("once", 20);
        main_playing(&engine, &main);
        let (long, short) = (clip(4), clip(1));
        let mut ends = Vec::new();
        let collect = |engine: &Engine, ends: &mut Vec<AsideEnded>| {
            ends.extend(engine.take_aside_ends());
        };

        engine.play_aside(1, &long, AsideThen::Restore).unwrap();
        until_clip(&engine, 1, AsidePhase::Playing);
        engine.play_aside(2, &long, AsideThen::Restore).unwrap();
        engine.hold_aside(Duration::from_secs(2));
        engine.play_aside(3, &long, AsideThen::Hold).unwrap();
        until_clip(&engine, 3, AsidePhase::Playing);
        engine.pause();
        engine.resume();
        // Each wait below is for the clip just asked for, so whatever ended
        // before it is collected first.
        collect(&engine, &mut ends);
        engine.play_aside(4, &short, AsideThen::Restore).unwrap();
        until_ended(&engine, &mut ends);
        engine.play_aside(5, &missing_clip(), AsideThen::Restore).unwrap();
        until_ended(&engine, &mut ends);
        engine.play_aside(6, &long, AsideThen::Restore).unwrap();
        engine.stop_aside(Some(6), true);
        engine.play_aside(7, &long, AsideThen::Restore).unwrap();
        engine.stop_aside(Some(99), true);
        engine.stop_aside(None, true);
        engine.stop_aside(None, true);
        collect(&engine, &mut ends);
        engine.play_aside(8, &short, AsideThen::Hold).unwrap();
        until_ended(&engine, &mut ends);
        tick_until(&engine, |e, _| e.aside_status().is_none());
        tick_for(&engine, Duration::from_millis(200));
        collect(&engine, &mut ends);

        let mut tickets: Vec<u64> = ends.iter().map(|end| end.ticket).collect();
        tickets.sort_unstable();
        assert_eq!(tickets, (1..=8).collect::<Vec<_>>(), "{ends:?}");
        let end_of = |ticket: u64| &ends.iter().find(|end| end.ticket == ticket).unwrap().end;
        assert_eq!(end_of(1), &AsideEnd::Superseded);
        assert_eq!(end_of(2), &AsideEnd::Superseded);
        assert_eq!(end_of(3), &AsideEnd::Taken);
        assert_eq!(end_of(4), &AsideEnd::RanOut);
        assert!(matches!(end_of(5), AsideEnd::Failed(_)));
        assert_eq!(end_of(6), &AsideEnd::Stopped);
        assert_eq!(end_of(7), &AsideEnd::Stopped);
        assert_eq!(end_of(8), &AsideEnd::RanOut);
        engine.stop();
        let _ = std::fs::remove_file(&main);
    }

    #[test]
    fn without_a_clip_every_main_command_behaves_as_before() {
        // Serve's way: the engine's own queue, every command it has.
        let (engine, _stage) = staged("Speakers");
        let (a, b, c) = (track("serve-a", 12), track("serve-b", 12), track("serve-c", 12));
        let quiet = |engine: &Engine| {
            assert_eq!(engine.aside_status(), None, "no clip session");
            assert_eq!(engine.take_aside_ends(), [], "and no clip ends");
            assert!(engine.state.lock().unwrap().aside_drains.is_empty());
        };
        engine.queue_add_many(vec![a.clone(), b.clone()]);
        tick_for(&engine, Duration::from_millis(300));
        assert!(engine.status().playing && engine.status().file == a);
        quiet(&engine);
        engine.pause();
        tick_for(&engine, Duration::from_millis(100));
        assert!(engine.status().paused);
        engine.resume();
        tick_for(&engine, Duration::from_millis(100));
        assert!(engine.status().playing);
        engine.seek(5.0).unwrap();
        assert!((engine.status().position - 5.0).abs() < 0.3);
        quiet(&engine);
        engine.next_manual().unwrap();
        assert_eq!(engine.status().file, b);
        engine.previous_manual().unwrap();
        assert_eq!(engine.status().file, a);
        engine.queue_add(c.clone());
        engine.queue_play_index(2).unwrap();
        assert_eq!(engine.status().file, c);
        engine.queue_remove(0).unwrap();
        assert_eq!(engine.queue_snapshot().current_index, 1);
        engine.set_volume(0.4);
        assert_eq!(engine.status().volume, 0.4);
        quiet(&engine);
        engine.stop();
        tick_for(&engine, Duration::from_millis(200));
        assert!(!engine.status().playing);
        engine.queue_clear();
        tick_until(&engine, |e, _| e.settled());
        quiet(&engine);
        assert!(matches!(engine.next_manual(), Err(EngineError::EndOfQueue)));
        for path in [a, b, c] {
            let _ = std::fs::remove_file(&path);
        }
    }
}
