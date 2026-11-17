//! Decoding, kept off the device callback.
//!
//! A device asks for sound every few milliseconds and plays whatever it has
//! been handed by the time it must: the callback that answers it cannot wait
//! for anything. It used to decode the track all the same, and a decoder
//! reads — a file, or a download that may have nothing more to give yet.
//! With a download stalled the callback sat in that read. Every other track
//! sat behind the callback (a file wholly on disk was installed, reported as
//! playing, and stood silent at 0:00), and a seek, which rodio performs
//! inside the callback and waits for, sat behind both — on the thread that
//! ticks and runs every command, for as long as the server took, which with
//! a server that never comes back is until the player is restarted.
//!
//! So each source is decoded by a thread of its own ([`DECODE_THREAD`]),
//! a second ahead ([`AHEAD`]), into blocks the callback takes without
//! waiting ([`Fed`]). With no block to take it plays silence and says so
//! ([`Feed::is_starved`]), and the other tracks on the bus play on. The
//! track's position stands still meanwhile: it is the sound the device has
//! been handed ([`Feed::position`]), and silence is not the track. rodio's
//! own count takes every sample that passes, the silence included, which is
//! why the engine reads this one instead.
//!
//! A seek is an order, not a wait: the callback notes it, drops what was
//! decoded for the old place and goes on in silence, and the decode thread
//! seeks when it next looks. If it is itself in a read that is waiting on a
//! download, the read is told to give up ([`Orders::unserved`],
//! [`Feed::nudge`]), so a seek to sound that has already arrived lands at
//! once however long the server stays away; one to sound that has not
//! lands when it comes. What the decoder made of the order reaches whoever
//! asked a little later ([`Feed::answer_within`]).
//!
//! Everything a source's controls do to it — volume, pause, the fades, the
//! visualizer's copy — still happens in the callback, on the far side of
//! the blocks, so none of it is a second late.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use rodio::source::{SeekError, UniformSourceIterator};
use rodio::{ChannelCount, Sample, SampleRate, Source};

use super::trace::etrace;

/// The decode threads' name. Whatever panics under one — symphonia has
/// known panics on malformed files — is caught there and ends that track,
/// so the panic hooks stand back for it as they do for the audio thread
/// (`tui::worker::panics_are_caught`).
pub(crate) const DECODE_THREAD: &str = "mstream-decode";

/// How much sound one block carries. Short enough that the callback never
/// takes more than it is about to play, long enough that taking one is a
/// rare thing next to the samples in it.
const BLOCK: Duration = Duration::from_millis(20);

/// How many blocks are decoded ahead: a second of sound. It is not a
/// buffer against the network — the spool is that — but against the decode
/// thread itself being kept from running for a moment on a busy machine.
/// It bounds the memory a source holds (under half a megabyte of stereo at
/// 48 kHz) and costs nothing in latency: every control acts after it.
const AHEAD: usize = 50;

/// How long sound takes to rise from nothing when it resumes after a
/// silence: a seek's landing, or the end of a starvation. Either way it
/// resumes mid-waveform, and a step from zero is a click.
const RISE: Duration = Duration::from_millis(5);

/// How often a decode thread with nothing to do looks up anyway. It is
/// woken for everything it waits on; this is the net under a wake that got
/// lost.
const IDLE: Duration = Duration::from_millis(500);

/// What the decode thread hands the callback.
enum Block {
    /// Whole frames of sound, decoded for seek order `order`. `at` is the
    /// track position of the first sample, in samples — said on the first
    /// block after the track starts or an order is answered, because only
    /// the decode thread knows where the decoder really is.
    Sound { order: u64, at: Option<u64>, samples: Vec<Sample> },
    /// The source ran out, as of `order`.
    End { order: u64 },
}

/// What the decoder made of the newest seek order it has looked at.
struct Answer {
    order: u64,
    said: Result<(), String>,
}

/// What a decode thread is told from outside: the seeks, and when nobody is
/// listening any more. Kept apart from the [`Feed`] because a download's
/// reader is built before the feed is, and has to be able to ask whether
/// to go on waiting.
pub(crate) struct Orders {
    /// The newest seek order's number, and where it aims, in nanoseconds.
    order: AtomicU64,
    target: AtomicU64,
    /// The order the decode thread has taken up.
    serving: AtomicU64,
    /// The callback's end is gone: nobody is listening.
    closed: AtomicBool,
}

impl Orders {
    pub(crate) fn new() -> Arc<Orders> {
        Arc::new(Orders {
            order: AtomicU64::new(0),
            target: AtomicU64::new(0),
            serving: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        })
    }

    /// Whether there is something the decode thread has not been told yet:
    /// a seek it has not taken up, or nobody left to decode for. A read
    /// that is waiting on a download gives up while this holds — the
    /// decoder has somewhere else to be.
    pub(crate) fn unserved(&self) -> bool {
        self.closed.load(Ordering::Acquire)
            || self.order.load(Ordering::Acquire) != self.serving.load(Ordering::Acquire)
    }
}

/// How a download is reached from outside the decoder that reads it.
pub(crate) struct Download {
    /// The orders its reader asks about ([`Orders::unserved`]).
    pub(crate) orders: Arc<Orders>,
    /// Makes a read that is waiting on the download ask again. `None` for a
    /// stream that cannot be told to give up — one of unknown length, which
    /// may not be able to seek, and a read it gave up could not be put
    /// right.
    pub(crate) nudge: Option<Box<dyn Fn() + Send + Sync>>,
    /// Ends the download, and with it any read waiting on it: run when
    /// nobody is listening, so a decode thread in a stalled read — and the
    /// spool file it holds — does not outlive the track by however long the
    /// server takes.
    pub(crate) cancel: Box<dyn Fn() + Send + Sync>,
}

/// What the engine can know about a source being decoded, and the little
/// the callback and the decode thread say to each other outside the blocks.
pub(crate) struct Feed {
    channels: ChannelCount,
    rate: SampleRate,
    /// The track position, in samples: how much of the track the device
    /// has been handed, counted from its start.
    played: AtomicU64,
    /// Whether the callback, the last time it wanted a block, found none.
    starved: AtomicBool,
    orders: Arc<Orders>,
    /// The decode thread, to wake.
    thread: OnceLock<std::thread::Thread>,
    answer: Mutex<Answer>,
    answered: Condvar,
    /// Why the decoding ended, if it ended by panicking.
    failure: Mutex<Option<String>>,
    /// See [`Download`]; both `None` for a file.
    nudge: Option<Box<dyn Fn() + Send + Sync>>,
    cancel: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Feed {
    fn new(channels: ChannelCount, rate: SampleRate, download: Option<Download>) -> Feed {
        let (orders, nudge, cancel) = match download {
            Some(download) => (download.orders, download.nudge, Some(download.cancel)),
            None => (Orders::new(), None, None),
        };
        Feed {
            channels,
            rate,
            played: AtomicU64::new(0),
            starved: AtomicBool::new(false),
            orders,
            thread: OnceLock::new(),
            answer: Mutex::new(Answer { order: 0, said: Ok(()) }),
            answered: Condvar::new(),
            failure: Mutex::new(None),
            nudge,
            cancel,
        }
    }

    /// The feed of nothing: where an engine that has not played yet reads
    /// its position from.
    pub(crate) fn idle() -> Arc<Feed> {
        let one = std::num::NonZero::<u32>::MIN;
        Arc::new(Feed::new(std::num::NonZero::<u16>::MIN, one, None))
    }

    fn per_second(&self) -> f64 {
        f64::from(self.rate.get()) * f64::from(self.channels.get())
    }

    /// Where the track stands, in seconds: the sound the device has been
    /// handed, not the samples that have gone by. It stands still through a
    /// starvation and jumps with a seek.
    pub(crate) fn position(&self) -> f64 {
        self.played.load(Ordering::Relaxed) as f64 / self.per_second()
    }

    /// Whether the device is being played silence for want of decoded
    /// sound: the download has stalled, or a seek has not landed yet.
    pub(crate) fn is_starved(&self) -> bool {
        self.starved.load(Ordering::Relaxed)
    }

    /// How many seek orders have been placed.
    pub(crate) fn orders(&self) -> u64 {
        self.orders.order.load(Ordering::Acquire)
    }

    /// Make a read that is waiting on the download look at its orders
    /// again — after a seek has been placed, from the thread that placed
    /// it: this takes the waiting reader's lock for a moment, which is not
    /// the callback's to wait on.
    pub(crate) fn nudge(&self) {
        if let Some(nudge) = &self.nudge {
            nudge();
        }
    }

    /// Why the decoding ended, if a panic ended it.
    #[cfg(test)]
    pub(crate) fn failure(&self) -> Option<String> {
        self.failure.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// The track position of `at`, in samples: a whole frame's worth.
    fn samples_at(&self, at: Duration) -> u64 {
        let frames = (at.as_secs_f64() * f64::from(self.rate.get())).round() as u64;
        frames * u64::from(self.channels.get())
    }

    /// Place a seek order — from inside the callback, so nothing here
    /// waits — and say its number.
    fn ask(&self, at: Duration) -> u64 {
        let orders = &self.orders;
        orders.target.store(u64::try_from(at.as_nanos()).unwrap_or(u64::MAX), Ordering::Release);
        let order = orders.order.fetch_add(1, Ordering::AcqRel) + 1;
        self.wake();
        order
    }

    fn wake(&self) {
        if let Some(thread) = self.thread.get() {
            thread.unpark();
        }
    }

    /// The decode thread's word on order `order`.
    fn say(&self, order: u64, said: Result<(), String>) {
        *self.answer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Answer { order, said };
        self.answered.notify_all();
    }

    /// What the decoder made of the newest seek, waited for no longer than
    /// `patience` — on the caller's thread, never the callback's. `None`
    /// when it has not looked yet: it is in a read of its own, and the seek
    /// lands when that read returns.
    pub(crate) fn answer_within(&self, patience: Duration) -> Option<Result<(), String>> {
        let wanted = self.orders();
        let answer = self.answer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (answer, _) = self
            .answered
            .wait_timeout_while(answer, patience, |answer| answer.order < wanted)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (answer.order >= wanted).then(|| answer.said.clone())
    }
}

/// A source the device callback can pull without ever waiting: the sound a
/// decode thread has made ready, and silence when it has made none.
pub(crate) struct Fed {
    feed: Arc<Feed>,
    blocks: Receiver<Block>,
    /// Spent blocks go back for the decode thread to fill again, so the
    /// callback frees nothing in the ordinary run.
    spare: SyncSender<Vec<Sample>>,
    /// The block in hand, and how far into it the callback is.
    block: Vec<Sample>,
    at: usize,
    /// The seek order this end is on: blocks decoded for an earlier one are
    /// for a place it has left.
    order: u64,
    channels: usize,
    /// Silent samples owed to finish the frame in hand. Sound always starts
    /// on a frame, so the channels never swap.
    owed: usize,
    /// The frames of rise still to go, of `rise_len`.
    rise: u32,
    rise_len: u32,
    /// Sound has been played, and a silence has followed it.
    sounded: bool,
    quiet: bool,
    ended: bool,
    total: Option<Duration>,
}

impl Fed {
    /// Start decoding `source` on a thread of its own, and hand back the
    /// end the callback pulls. `download` is how the stream `source` reads
    /// is reached, when it reads one.
    pub(crate) fn spawn<S>(source: S, download: Option<Download>) -> Result<Fed, String>
    where
        S: Source + Send + 'static,
    {
        let (channels, rate, total) =
            (source.channels(), source.sample_rate(), source.total_duration());
        let per_block = (u128::from(rate.get()) * BLOCK.as_millis() / 1000).max(1) as usize;
        let block_len = per_block * usize::from(channels.get());
        let feed = Arc::new(Feed::new(channels, rate, download));
        let (blocks_tx, blocks) = mpsc::sync_channel(AHEAD);
        let (spare, spare_rx) = mpsc::sync_channel(AHEAD + 2);
        let theirs = feed.clone();
        let thread = std::thread::Builder::new()
            .name(DECODE_THREAD.into())
            .spawn(move || {
                // Pinned to the format it started in: a stream that changes
                // rate or channels part-way is converted here, and the
                // callback's end has one format to answer for.
                let pinned = UniformSourceIterator::new(Asked(source), channels, rate);
                let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    decode(pinned, &theirs, &blocks_tx, &spare_rx, block_len)
                }));
                if let Err(panic) = decoded {
                    let said = panic
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("no message");
                    etrace!("the decoder gave up on the track: {said}");
                    *theirs.failure.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(said.to_string());
                }
                // The sender goes with the thread: the callback's end plays
                // out what was decoded and then finds the track at its end.
            })
            .map_err(|e| format!("the decode thread would not start: {e}"))?;
        let _ = feed.thread.set(thread.thread().clone());
        Ok(Fed::pulling(feed, blocks, spare, total))
    }

    /// The callback's end of `feed`: it takes what comes through `blocks`
    /// and hands the spent ones back through `spare`.
    fn pulling(
        feed: Arc<Feed>,
        blocks: Receiver<Block>,
        spare: SyncSender<Vec<Sample>>,
        total: Option<Duration>,
    ) -> Fed {
        let rise_len = (u128::from(feed.rate.get()) * RISE.as_millis() / 1000).max(1) as u32;
        Fed {
            channels: usize::from(feed.channels.get()),
            feed,
            blocks,
            spare,
            block: Vec::new(),
            at: 0,
            order: 0,
            owed: 0,
            rise: 0,
            rise_len,
            sounded: false,
            quiet: false,
            ended: false,
            total,
        }
    }

    /// The engine's view of this source.
    pub(crate) fn feed(&self) -> Arc<Feed> {
        self.feed.clone()
    }

    fn give_back(&self, mut spent: Vec<Sample>) {
        if spent.capacity() > 0 {
            spent.clear();
            // Full is fine: the decode thread has spares enough, and this
            // one is freed here.
            let _ = self.spare.try_send(spent);
        }
    }

    /// The next block decoded for the order in hand, if one is ready.
    fn take(&mut self) -> Took {
        loop {
            match self.blocks.try_recv() {
                Ok(Block::Sound { order, at, samples }) => {
                    if order != self.order {
                        self.give_back(samples);
                        continue;
                    }
                    if let Some(at) = at {
                        self.feed.played.store(at, Ordering::Relaxed);
                    }
                    let spent = std::mem::replace(&mut self.block, samples);
                    self.give_back(spent);
                    self.at = 0;
                    self.feed.starved.store(false, Ordering::Relaxed);
                    if self.quiet && self.sounded {
                        self.rise = self.rise_len;
                    }
                    self.quiet = false;
                    self.sounded = true;
                    return Took::Sound;
                }
                Ok(Block::End { order }) if order == self.order => return Took::End,
                Ok(Block::End { .. }) => continue,
                Err(TryRecvError::Empty) => return Took::Nothing,
                // The decode thread is gone: what it decoded has been
                // played, and there will be no more.
                Err(TryRecvError::Disconnected) => return Took::End,
            }
        }
    }
}

enum Took {
    Sound,
    Nothing,
    End,
}

impl Iterator for Fed {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        loop {
            if self.owed > 0 {
                self.owed -= 1;
                return Some(0.0);
            }
            if self.at < self.block.len() {
                let mut sample = self.block[self.at];
                self.at += 1;
                self.feed.played.fetch_add(1, Ordering::Relaxed);
                if self.rise > 0 {
                    sample *= 1.0 - self.rise as Sample / self.rise_len as Sample;
                    if self.at % self.channels == 0 {
                        self.rise -= 1;
                    }
                }
                return Some(sample);
            }
            if self.ended {
                return None;
            }
            // The block in hand is spent, on a frame's end.
            match self.take() {
                Took::Sound => {}
                Took::End => {
                    self.ended = true;
                    self.feed.starved.store(false, Ordering::Relaxed);
                    return None;
                }
                // Nothing decoded: a frame of silence, and another look.
                Took::Nothing => {
                    self.feed.starved.store(true, Ordering::Relaxed);
                    self.quiet = true;
                    self.owed = self.channels;
                }
            }
        }
    }
}

impl Source for Fed {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.feed.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.feed.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total
    }

    /// Called from inside the callback, by rodio's seek order: it places
    /// the order and returns. Whether the decoder can do it is the decode
    /// thread's to find out ([`Feed::answer_within`]).
    fn try_seek(&mut self, at: Duration) -> Result<(), SeekError> {
        // A frame cut short by the jump is finished in silence.
        let into = if self.at < self.block.len() { self.at % self.channels } else { 0 };
        if into != 0 {
            self.owed = self.channels - into;
        }
        self.order = self.feed.ask(at);
        self.feed.played.store(self.feed.samples_at(at), Ordering::Relaxed);
        // What is in hand was decoded for the old place.
        let spent = std::mem::take(&mut self.block);
        self.give_back(spent);
        self.at = 0;
        self.quiet = true;
        self.ended = false;
        // So was what waits, and letting it go makes room, which is what a
        // decode thread waiting to hand over a block is waiting for — but
        // only as far as the first block made for this order. The thread
        // is woken by the room, and can have taken the order up, sought
        // and handed that block over while this is still looking through
        // the old ones. It says where the decoder really is, and says it
        // once; so does the end of the track, if that is what the seek
        // found.
        if let Took::End = self.take() {
            self.ended = true;
            self.feed.starved.store(false, Ordering::Relaxed);
        }
        Ok(())
    }
}

impl Drop for Fed {
    fn drop(&mut self) {
        self.feed.orders.closed.store(true, Ordering::Release);
        if let Some(cancel) = &self.feed.cancel {
            cancel();
        }
        self.feed.wake();
        // The blocks' receiver goes with this: a decode thread waiting to
        // hand one over is told there is nobody to hand it to.
    }
}

/// A source that is asked for its next sample even when it says the span it
/// has is empty.
///
/// An empty span is rodio's word for a source that has ended
/// ([`Source::current_span_len`]), and its converter takes it at that: a
/// span's length is how much the converter takes before it asks again, and
/// of an empty one it takes nothing — the source is never asked for a
/// sample at all. rodio's own decoder says it of a track that has not
/// begun. The span it reports is the packet it has in hand, it decodes the
/// next one only when a sample of it is asked for, and the first packet of
/// a Vorbis stream decodes to no samples: every Ogg Vorbis track was over
/// as it started.
///
/// So an empty span goes on to the converter as a span of one frame. The
/// frame is asked for, and the asking is what tells: the source finds its
/// next span and says how long that really is, or it returns `None`, which
/// is the end in anyone's words. One frame, because a span is whole frames
/// and the shorter the one made up here, the sooner the source is asked
/// for its own.
struct Asked<S>(S);

impl<S: Source> Iterator for Asked<S> {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        self.0.next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<S: Source> Source for Asked<S> {
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        match self.0.current_span_len() {
            Some(0) => Some(usize::from(self.0.channels().get())),
            len => len,
        }
    }

    #[inline]
    fn channels(&self) -> ChannelCount {
        self.0.channels()
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.0.sample_rate()
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        self.0.total_duration()
    }

    #[inline]
    fn try_seek(&mut self, at: Duration) -> Result<(), SeekError> {
        self.0.try_seek(at)
    }
}

/// The decode thread: decode a block, hand it over, and look at the orders
/// between blocks. It waits in three places, and each is woken: handing a
/// block to a callback that has room for none (the callback takes one, or
/// goes), a source that has run out (an order, or the callback going), and
/// the source's own reads — which, on a download that can be told to, give
/// up while there is an order it has not taken up.
fn decode<S: Source>(
    mut source: UniformSourceIterator<S>,
    feed: &Feed,
    blocks: &SyncSender<Block>,
    spare: &Receiver<Vec<Sample>>,
    block_len: usize,
) {
    let orders = &*feed.orders;
    let channels = usize::from(feed.channels.get());
    let mut order = 0;
    // The track position of the next sample decoded, and whether the next
    // block is to say it.
    let mut at: u64 = 0;
    let mut say_at = true;
    let (mut ended, mut end_said) = (false, false);
    // A read or a seek was told to give up part-way. The decoder may have
    // taken part of a packet, so it seeks before it reads again.
    let mut torn = false;
    loop {
        if orders.closed.load(Ordering::Acquire) {
            return;
        }
        let wanted = orders.order.load(Ordering::Acquire);
        if wanted != order {
            order = wanted;
            // From here a read that waits is this order's own, and only a
            // newer one makes it give up.
            orders.serving.store(order, Ordering::Release);
            let target = Duration::from_nanos(orders.target.load(Ordering::Acquire));
            let sought = source.try_seek(target).map_err(|e| e.to_string());
            if sought.is_ok() {
                at = feed.samples_at(target);
                ended = false;
                torn = false;
            }
            if orders.unserved() {
                // A newer order, or nobody left to answer: this one's
                // outcome is nobody's. A seek that failed may be one that
                // was told to give up.
                torn |= sought.is_err();
                continue;
            }
            if let Err(refused) = &sought {
                etrace!("seek to {:.2} refused by the decoder: {refused}", target.as_secs_f64());
                if torn {
                    // The read it was pulled out of is not put right by a
                    // seek that failed: back to where the decoder stood.
                    let stood = Duration::from_secs_f64(at as f64 / feed.per_second());
                    match source.try_seek(stood) {
                        Ok(()) => torn = false,
                        Err(_) if orders.unserved() => continue,
                        Err(e) => {
                            etrace!("and the decoder could not go back either ({e}): the track ends");
                            ended = true;
                        }
                    }
                }
                // Otherwise the decoder is where it was, and the track
                // plays on from there — a second on from where it was
                // heard, since what had been decoded ahead went when the
                // order was placed.
            }
            feed.say(order, sought);
            say_at = true;
            end_said = false;
            continue;
        }
        if ended {
            if !end_said {
                if blocks.send(Block::End { order }).is_err() {
                    return;
                }
                end_said = true;
            }
            std::thread::park_timeout(IDLE);
            continue;
        }
        let mut samples = spare.try_recv().unwrap_or_default();
        samples.clear();
        samples.reserve(block_len);
        let mut gave_up = false;
        while samples.len() < block_len {
            match source.next() {
                Some(sample) => samples.push(sample),
                // Out of a read it was told to give up — for an order the
                // top of the loop is about to find, or because nobody is
                // listening — and not out of sound.
                None if orders.unserved() => {
                    gave_up = true;
                    break;
                }
                None => {
                    ended = true;
                    break;
                }
            }
        }
        if gave_up {
            torn = true;
            continue;
        }
        // Whole frames, always: a source that ran out mid-frame is padded.
        while samples.len() % channels != 0 {
            samples.push(0.0);
        }
        if samples.is_empty() {
            continue;
        }
        let first = say_at.then_some(at);
        say_at = false;
        at += samples.len() as u64;
        if blocks.send(Block::Sound { order, at: first, samples }).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZero;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    /// What a test's source is told to do, and what it says it did.
    #[derive(Default)]
    struct Script {
        /// The source reads no further than this many frames until told —
        /// a download that has stalled.
        held_at: AtomicUsize,
        released: AtomicBool,
        /// Frames decoded so far.
        decoded: AtomicUsize,
        refuses_seeks: AtomicBool,
        /// Seeks to this frame or past it are refused.
        refuses_from: AtomicUsize,
        panics_at: AtomicUsize,
        dropped: AtomicBool,
        /// The orders a held read asks about, as a download's reader does:
        /// with them, it gives up while there is one it has not been told.
        orders: Mutex<Option<Arc<Orders>>>,
        /// A read was given up and no seek has followed it yet — and
        /// whether the source was read on in that state, which is the one
        /// thing a decoder pulled out of a read must not do.
        torn: AtomicBool,
        read_on_torn: AtomicBool,
        /// Every frame a seek landed on.
        sought: Mutex<Vec<usize>>,
    }

    /// A source whose sample says where it is: frame `n`'s left channel is
    /// `n + 1`, its right `-(n + 1)`.
    struct Ramp {
        frame: usize,
        channel: u16,
        frames: usize,
        script: Arc<Script>,
    }

    impl Ramp {
        fn of(frames: usize) -> (Ramp, Arc<Script>) {
            let script = Arc::new(Script::default());
            script.held_at.store(usize::MAX, Ordering::Release);
            script.panics_at.store(usize::MAX, Ordering::Release);
            script.refuses_from.store(usize::MAX, Ordering::Release);
            (Ramp { frame: 0, channel: 0, frames, script: script.clone() }, script)
        }

        /// A ramp read through a download that can be told to give up.
        fn downloading(frames: usize) -> (Ramp, Arc<Script>, Download) {
            let (ramp, script) = Ramp::of(frames);
            let orders = Orders::new();
            *script.orders.lock().unwrap() = Some(orders.clone());
            let theirs = script.clone();
            let cancel: Box<dyn Fn() + Send + Sync> =
                Box::new(move || theirs.released.store(true, Ordering::Release));
            (ramp, script, Download { orders, nudge: None, cancel })
        }
    }

    impl Iterator for Ramp {
        type Item = Sample;
        fn next(&mut self) -> Option<Sample> {
            if self.frame >= self.frames {
                return None;
            }
            if self.channel == 0 {
                while self.frame >= self.script.held_at.load(Ordering::Acquire)
                    && !self.script.released.load(Ordering::Acquire)
                {
                    let orders = self.script.orders.lock().unwrap().clone();
                    if orders.is_some_and(|orders| orders.unserved()) {
                        self.script.torn.store(true, Ordering::Release);
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                if self.frame >= self.script.panics_at.load(Ordering::Acquire) {
                    panic!("a frame it could not decode");
                }
            }
            if self.script.torn.load(Ordering::Acquire) {
                self.script.read_on_torn.store(true, Ordering::Release);
            }
            let value = (self.frame + 1) as Sample;
            let sample = if self.channel == 0 { value } else { -value };
            self.channel += 1;
            if self.channel == 2 {
                self.channel = 0;
                self.frame += 1;
                self.script.decoded.store(self.frame, Ordering::Release);
            }
            Some(sample)
        }
    }

    impl Source for Ramp {
        fn current_span_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> ChannelCount {
            NonZero::new(2).unwrap()
        }
        fn sample_rate(&self) -> SampleRate {
            NonZero::new(RATE).unwrap()
        }
        fn total_duration(&self) -> Option<Duration> {
            Some(Duration::from_secs_f64(self.frames as f64 / f64::from(RATE)))
        }
        fn try_seek(&mut self, at: Duration) -> Result<(), SeekError> {
            let frame = (at.as_secs_f64() * f64::from(RATE)).round() as usize;
            if self.script.refuses_seeks.load(Ordering::Acquire)
                || frame >= self.script.refuses_from.load(Ordering::Acquire)
            {
                return Err(SeekError::NotSupported { underlying_source: "a test's ramp" });
            }
            self.frame = frame;
            self.channel = 0;
            self.script.torn.store(false, Ordering::Release);
            self.script.sought.lock().unwrap().push(frame);
            Ok(())
        }
    }

    impl Drop for Ramp {
        fn drop(&mut self) {
            self.script.dropped.store(true, Ordering::Release);
        }
    }

    /// A ramp handed over the way rodio's decoder hands a track over: in
    /// packets, each decoded only when a sample of it is asked for, the
    /// span it reports being the packet in hand, the whole of it. It
    /// starts with a packet of no samples in hand, as that decoder does on
    /// a Vorbis stream, whose first packet decodes to nothing.
    struct Packets {
        ramp: Ramp,
        /// The samples in the packet in hand, and how many have been taken.
        len: usize,
        taken: usize,
        packets: usize,
    }

    impl Packets {
        /// What the packets hold, in frames, turn about: a Vorbis stream
        /// has short blocks and long ones.
        const FRAMES: [usize; 2] = [64, 448];

        fn of(ramp: Ramp) -> Packets {
            Packets { ramp, len: 0, taken: 0, packets: 0 }
        }
    }

    impl Iterator for Packets {
        type Item = Sample;
        fn next(&mut self) -> Option<Sample> {
            if self.taken >= self.len {
                // The next packet, if there is one: with none, what is in
                // hand stays in hand.
                let first = self.ramp.next()?;
                self.len = Self::FRAMES[self.packets % 2] * 2;
                self.packets += 1;
                self.taken = 1;
                return Some(first);
            }
            self.taken += 1;
            self.ramp.next()
        }
    }

    impl Source for Packets {
        fn current_span_len(&self) -> Option<usize> {
            Some(self.len)
        }
        fn channels(&self) -> ChannelCount {
            self.ramp.channels()
        }
        fn sample_rate(&self) -> SampleRate {
            self.ramp.sample_rate()
        }
        fn total_duration(&self) -> Option<Duration> {
            self.ramp.total_duration()
        }
        fn try_seek(&mut self, at: Duration) -> Result<(), SeekError> {
            self.ramp.try_seek(at)?;
            // The packet in hand is of the place it has left.
            self.taken = usize::MAX;
            Ok(())
        }
    }

    const RATE: u32 = 8_000;
    /// One block of a ramp, in frames.
    const PER_BLOCK: usize = 160;

    /// Pull until `count` samples of sound have come, the silences between
    /// them left out, or fail after five seconds without any. Sound is what
    /// moves the position; silence never does.
    fn sound(fed: &mut Fed, count: usize) -> Vec<Sample> {
        let mut heard = Vec::with_capacity(count);
        let until = Instant::now() + Duration::from_secs(5);
        while heard.len() < count {
            let before = fed.feed.played.load(Ordering::Relaxed);
            let sample = fed.next().expect("the source has not run out");
            if fed.feed.played.load(Ordering::Relaxed) != before {
                heard.push(sample);
            } else {
                assert_eq!(sample, 0.0, "what is not the track is silence");
                assert!(Instant::now() < until, "no sound came; heard {} samples", heard.len());
                std::thread::sleep(Duration::from_micros(200));
            }
        }
        heard
    }

    /// Wait for something another thread is to do. It sleeps between
    /// looks, and on a busy machine a sleep comes back far later than it
    /// was asked to, so it is not for pulling sound that is already there:
    /// a block is 320 looks ([`sound`] pulls that without a wait).
    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Wait until the decoder has made `frames` frames and handed them over.
    /// A test pulls as fast as the machine runs, which is faster than
    /// anything decodes: without this it outruns the decode thread, is
    /// played a silence, and hears the sound rise again — rightly, and not
    /// what a test of the samples themselves wants to be measuring.
    fn decoded(script: &Script, frames: usize) {
        wait_for("the decoder never got that far", || {
            script.decoded.load(Ordering::Acquire) >= frames
        });
        std::thread::sleep(Duration::from_millis(50));
    }

    /// Wait until the decode thread has stopped decoding: its second ahead
    /// is full and it is waiting for room, or the source has run out or
    /// stalled. For where "so many frames further" would be a guess — the
    /// thread may have got there already and be waiting.
    fn at_rest(script: &Script) {
        let until = Instant::now() + Duration::from_secs(5);
        let mut seen = script.decoded.load(Ordering::Acquire);
        loop {
            std::thread::sleep(Duration::from_millis(40));
            let now = script.decoded.load(Ordering::Acquire);
            if now == seen {
                return;
            }
            assert!(Instant::now() < until, "the decoder never came to rest");
            seen = now;
        }
    }

    /// The frame a left-channel sample says it is from.
    fn frame_of(sample: Sample) -> usize {
        assert!(sample > 0.0, "{sample} is not a left-channel sample");
        sample as usize - 1
    }

    /// The callback's end with no decode thread behind it: the test hands
    /// the blocks over itself, in the order a race would have them come.
    fn by_hand() -> (Fed, SyncSender<Block>) {
        let (hand_over, blocks) = mpsc::sync_channel(AHEAD);
        let (spare, _) = mpsc::sync_channel(AHEAD + 2);
        let stereo = NonZero::new(2).unwrap();
        let feed = Arc::new(Feed::new(stereo, NonZero::new(RATE).unwrap(), None));
        (Fed::pulling(feed, blocks, spare, None), hand_over)
    }

    #[test]
    fn what_the_callback_pulls_is_what_the_decoder_made_sample_for_sample() {
        let frames = PER_BLOCK * 7 + 33;
        let (ramp, script) = Ramp::of(frames);
        let mut fed = Fed::spawn(ramp, None).unwrap();
        decoded(&script, frames);
        assert_eq!(fed.channels().get(), 2);
        assert_eq!(fed.sample_rate().get(), RATE);
        assert_eq!(fed.total_duration(), Some(Duration::from_secs_f64(frames as f64 / 8000.0)));

        let heard = sound(&mut fed, frames * 2);
        for (n, frame) in heard.chunks(2).enumerate() {
            assert_eq!(frame, [(n + 1) as Sample, -((n + 1) as Sample)], "frame {n}");
        }
        // The position is the sound handed over, to the sample.
        assert!((fed.feed.position() - frames as f64 / 8000.0).abs() < 1e-9);
        wait_for("the end never came", || fed.next().is_none());
        assert!(fed.next().is_none(), "and it stays ended");
        assert!(!fed.feed.is_starved(), "ended is not starved");
    }

    #[test]
    fn a_track_that_starts_on_an_empty_span_is_played_from_its_first_sample_to_its_last() {
        // Every Ogg Vorbis track: rodio's decoder has the stream's first
        // packet in hand when it is handed over, and that packet decodes
        // to nothing. An empty span is rodio's word for a source that has
        // ended, and the track was taken at it — over as it began, with
        // all of it still to come.
        let frames = PER_BLOCK * 7 + 33;
        let (ramp, script) = Ramp::of(frames);
        let mut fed = Fed::spawn(Packets::of(ramp), None).unwrap();
        decoded(&script, frames);

        let heard = sound(&mut fed, frames * 2);
        for (n, frame) in heard.chunks(2).enumerate() {
            assert_eq!(frame, [(n + 1) as Sample, -((n + 1) as Sample)], "frame {n}");
        }
        assert!((fed.feed.position() - frames as f64 / 8000.0).abs() < 1e-9);
        wait_for("the end never came", || fed.next().is_none());
        assert!(fed.next().is_none(), "and it stays ended");
    }

    #[test]
    fn a_seek_placed_before_such_a_track_gave_a_sample_lands_where_it_was_sent() {
        // Nothing of the download has come when the seek is placed — a
        // track resumed where it was left: the empty span is still what
        // the source has in hand, and is no more its end after a seek than
        // before one.
        let (ramp, script, download) = Ramp::downloading(RATE as usize * 60);
        script.held_at.store(0, Ordering::Release);
        let mut fed = Fed::spawn(Packets::of(ramp), Some(download)).unwrap();
        assert_eq!(fed.next(), Some(0.0), "nothing has come");

        fed.try_seek(Duration::from_secs(30)).unwrap();
        assert_eq!(fed.feed.answer_within(Duration::from_secs(2)), Some(Ok(())));
        script.released.store(true, Ordering::Release);
        let from = RATE as usize * 30;
        decoded(&script, from + PER_BLOCK * 2);
        let heard = sound(&mut fed, PER_BLOCK * 2);
        for (n, frame) in heard.chunks(2).enumerate() {
            let at = (from + n + 1) as Sample;
            assert_eq!(frame, [at, -at], "frame {n} after the seek");
        }
        assert_eq!(*script.sought.lock().unwrap(), [from]);
        assert!(!script.read_on_torn.load(Ordering::Acquire), "it read on from a read it gave up");
    }

    #[test]
    fn with_nothing_decoded_it_plays_silence_and_the_position_stands_still() {
        // A download that stalls two blocks in: the callback is never kept
        // waiting — it is handed silence, a whole frame at a time — and the
        // track does not move for it.
        let (ramp, script) = Ramp::of(PER_BLOCK * 20);
        script.held_at.store(PER_BLOCK * 2, Ordering::Release);
        let mut fed = Fed::spawn(ramp, None).unwrap();
        decoded(&script, PER_BLOCK * 2);
        let heard = sound(&mut fed, PER_BLOCK * 2 * 2);
        assert_eq!(frame_of(heard[heard.len() - 2]), PER_BLOCK * 2 - 1);

        let stood = fed.feed.position();
        let started = Instant::now();
        for _ in 0..4_000 {
            assert_eq!(fed.next(), Some(0.0));
        }
        assert!(started.elapsed() < Duration::from_millis(250), "the callback was kept waiting");
        assert!(fed.feed.is_starved());
        assert_eq!(fed.feed.position(), stood, "silence is not the track");

        // The download moves again: the track goes on from the very frame,
        // on a frame's first channel, rising from nothing.
        script.released.store(true, Ordering::Release);
        decoded(&script, PER_BLOCK * 6);
        let resumed = sound(&mut fed, PER_BLOCK * 2);
        assert_eq!(resumed[0], 0.0, "it rises from silence");
        assert!(resumed[2] > 0.0 && resumed[3] < 0.0, "left then right: {:?}", &resumed[..4]);
        let risen = &resumed[resumed.len() - 2..];
        assert_eq!(frame_of(risen[0]), PER_BLOCK * 3 - 1, "nothing was skipped");
        assert_eq!(risen[1], -risen[0], "and by now at full level");
        assert!(!fed.feed.is_starved());
    }

    #[test]
    fn a_seek_is_an_order_and_what_was_decoded_for_the_old_place_is_never_heard() {
        let (ramp, script) = Ramp::of(RATE as usize * 60);
        let mut fed = Fed::spawn(ramp, None).unwrap();
        decoded(&script, PER_BLOCK * 4);
        // An odd number of samples: the seek lands mid-frame.
        let before = sound(&mut fed, 301);
        assert!(before[300] > 0.0, "a left channel was the last thing heard");

        let started = Instant::now();
        fed.try_seek(Duration::from_secs(30)).unwrap();
        assert!(started.elapsed() < Duration::from_millis(50), "the callback waited on the seek");
        // It says where it is going at once.
        assert!((fed.feed.position() - 30.0).abs() < 1e-9);
        // The frame the jump cut is finished in silence, so left stays left.
        assert_eq!(fed.next(), Some(0.0));

        decoded(&script, RATE as usize * 30 + PER_BLOCK * 4);
        let after = sound(&mut fed, PER_BLOCK * 2);
        assert_eq!(after[0], 0.0, "it rises from silence");
        for (n, frame) in after.chunks(2).enumerate().skip(RATE as usize / 200) {
            let from = RATE as usize * 30 + n;
            assert_eq!(frame, [(from + 1) as Sample, -((from + 1) as Sample)], "frame {n} after");
        }
        assert_eq!(fed.feed.answer_within(Duration::from_secs(2)), Some(Ok(())));
    }

    #[test]
    fn a_seek_reaches_a_decode_thread_that_is_waiting_for_room() {
        // Paused: nothing is pulled, the second ahead fills, and the decode
        // thread waits to hand over one block more. It decodes no further —
        // and a seek, which makes room, is what it wakes to.
        let (ramp, script) = Ramp::of(RATE as usize * 600);
        let mut fed = Fed::spawn(ramp, None).unwrap();
        let most = (AHEAD + 2) * PER_BLOCK;
        wait_for("the second ahead never filled", || {
            script.decoded.load(Ordering::Acquire) >= AHEAD * PER_BLOCK
        });
        std::thread::sleep(Duration::from_millis(100));
        let ahead = script.decoded.load(Ordering::Acquire);
        assert!(ahead <= most, "{ahead} frames decoded with nobody listening; at most {most}");

        fed.try_seek(Duration::from_secs(300)).unwrap();
        let after = sound(&mut fed, 2);
        assert_eq!(after, [(RATE as usize * 300 + 1) as Sample, -((RATE as usize * 300 + 1) as Sample)]);
    }

    #[test]
    fn a_seek_the_decoder_refuses_is_said_and_the_track_plays_on() {
        let (ramp, script) = Ramp::of(RATE as usize * 60);
        script.refuses_seeks.store(true, Ordering::Release);
        let mut fed = Fed::spawn(ramp, None).unwrap();
        at_rest(&script);
        sound(&mut fed, 400);

        fed.try_seek(Duration::from_secs(30)).unwrap();
        let said = fed.feed.answer_within(Duration::from_secs(2)).expect("answered");
        assert!(said.is_err(), "the decoder's refusal reaches whoever asked: {said:?}");
        // It plays on from where the decoder stands, and the position says
        // that place, not the one that was asked for.
        at_rest(&script);
        let after = sound(&mut fed, PER_BLOCK * 2);
        let from = frame_of(after[after.len() - 2]);
        assert!(from > 200 && from < RATE as usize * 3, "it went on from frame {from}");
        let position = fed.feed.position();
        assert!((position - (from + 1) as f64 / 8000.0).abs() < 1e-9, "at {position}");
    }

    #[test]
    fn what_was_decoded_for_a_seek_while_it_was_still_being_placed_is_kept() {
        // Placing a seek lets go of what waits, which makes room, and room
        // is what a decode thread with a block to hand over is waiting
        // for. Woken by it, the thread can take the order up, seek and
        // hand over the first block of the new place while the callback is
        // still looking through the old ones. Here the blocks lie as that
        // leaves them: one of the old place, then the first for the order
        // about to be placed — from a decoder that refused the seek, so it
        // says where the decoder stands instead.
        let (mut fed, hand_over) = by_hand();
        let placed = fed.feed.orders() + 1;
        hand_over.send(Block::Sound { order: 0, at: Some(0), samples: vec![1.0, -1.0] }).unwrap();
        let stands = Block::Sound { order: placed, at: Some(1_000), samples: vec![501.0, -501.0] };
        hand_over.send(stands).unwrap();

        fed.try_seek(Duration::from_secs(30)).unwrap();
        // Let go with the old one, it left the position reading the place
        // that was asked for, for the rest of the track.
        let position = fed.feed.position();
        assert!((position - 1_000.0 / 16_000.0).abs() < 1e-9, "at {position}");
        assert_eq!([fed.next(), fed.next()], [Some(501.0), Some(-501.0)]);
        assert_eq!(fed.next(), Some(0.0), "and then it waits for the next block");

        // The end of the track is said once. A seek to the end that let go
        // of it left the track waiting for sound that was never to come.
        let (mut fed, hand_over) = by_hand();
        hand_over.send(Block::Sound { order: 0, at: Some(0), samples: vec![1.0, -1.0] }).unwrap();
        hand_over.send(Block::End { order: fed.feed.orders() + 1 }).unwrap();
        fed.try_seek(Duration::from_secs(30)).unwrap();
        assert_eq!(fed.next(), None);
        assert!(!fed.feed.is_starved(), "ended is not starved");
    }

    #[test]
    fn a_decoder_that_panics_ends_its_track_and_nothing_else() {
        // symphonia has known panics on malformed files. In the callback a
        // panic took the stream's thread with it, and on macOS the process.
        let (ramp, script) = Ramp::of(RATE as usize * 60);
        script.panics_at.store(PER_BLOCK * 3, Ordering::Release);
        let mut fed = Fed::spawn(ramp, None).unwrap();
        decoded(&script, PER_BLOCK * 3);
        // Everything decoded before it is still played: all three blocks,
        // heard here and not left for the wait below to pull.
        let heard = sound(&mut fed, PER_BLOCK * 3 * 2);
        assert_eq!(frame_of(heard[heard.len() - 2]), PER_BLOCK * 3 - 1);
        wait_for("the track never ended", || fed.next().is_none());
        assert_eq!(fed.feed.failure().as_deref(), Some("a frame it could not decode"));
    }

    #[test]
    fn dropping_the_callbacks_end_ends_the_decode_thread_even_one_stuck_in_a_read() {
        // Waiting for room: told there is nobody to hand the block to.
        let (ramp, script) = Ramp::of(RATE as usize * 600);
        let fed = Fed::spawn(ramp, None).unwrap();
        wait_for("it never got going", || script.decoded.load(Ordering::Acquire) > 0);
        drop(fed);
        wait_for("the decoder outlived its listener", || script.dropped.load(Ordering::Acquire));

        // Run out and parked: woken.
        let (ramp, script) = Ramp::of(PER_BLOCK);
        let mut fed = Fed::spawn(ramp, None).unwrap();
        sound(&mut fed, PER_BLOCK * 2);
        wait_for("the end never came", || fed.next().is_none());
        drop(fed);
        wait_for("the decoder outlived its listener", || script.dropped.load(Ordering::Acquire));

        // Stuck in a read of a download that has stalled, and one that
        // cannot be told to give up for a seek: cancelling the download is
        // what ends that read.
        let (ramp, script, mut download) = Ramp::downloading(RATE as usize * 600);
        *script.orders.lock().unwrap() = None;
        download.nudge = None;
        script.held_at.store(PER_BLOCK, Ordering::Release);
        let mut fed = Fed::spawn(ramp, Some(download)).unwrap();
        sound(&mut fed, PER_BLOCK * 2);
        assert_eq!(fed.next(), Some(0.0), "stalled");
        drop(fed);
        wait_for("the decoder outlived its listener", || script.dropped.load(Ordering::Acquire));
    }

    #[test]
    fn a_seek_pulls_the_decoder_out_of_a_read_that_is_waiting_on_its_download() {
        // The download stalls two blocks in and never moves again. The
        // decoder is in a read of what has not come — and a seek back into
        // what has is taken up at once: the read gives up, the decoder
        // seeks, and the sound that is there plays. It used to land when
        // the server next answered.
        let (ramp, script, download) = Ramp::downloading(RATE as usize * 60);
        script.held_at.store(PER_BLOCK * 2, Ordering::Release);
        let mut fed = Fed::spawn(ramp, Some(download)).unwrap();
        decoded(&script, PER_BLOCK * 2);
        sound(&mut fed, PER_BLOCK * 2 * 2);
        assert_eq!(fed.next(), Some(0.0), "stalled");
        assert_eq!(fed.next(), Some(0.0));

        fed.try_seek(Duration::from_secs_f64(80.0 / 8000.0)).unwrap();
        assert_eq!(fed.feed.answer_within(Duration::from_secs(2)), Some(Ok(())));
        let again = sound(&mut fed, PER_BLOCK * 2);
        let last = &again[again.len() - 2..];
        assert_eq!(frame_of(last[0]), 80 + PER_BLOCK - 1, "it plays from where it was sent");
        assert_eq!(*script.sought.lock().unwrap(), [80]);
        assert!(!script.read_on_torn.load(Ordering::Acquire), "it read on from a read it gave up");
        // And then it waits on the download again, as it did. (The block
        // the stall caught half-made is not handed over until it is whole:
        // up to a block's worth, 20 ms, of what has arrived waits with it.)
        assert_eq!(fed.next(), Some(0.0));
        assert!(fed.feed.is_starved());
    }

    #[test]
    fn a_decoder_pulled_out_of_a_read_for_a_seek_it_cannot_make_goes_back_where_it_stood() {
        // A read that was given up may have taken part of a packet, and a
        // seek is what puts a decoder right. When the seek it was pulled
        // out for is refused, it seeks back to where it stood: the refusal
        // is said, and the track plays on with nothing lost.
        let (ramp, script, download) = Ramp::downloading(RATE as usize * 60);
        script.held_at.store(PER_BLOCK * 2, Ordering::Release);
        script.refuses_from.store(RATE as usize * 10, Ordering::Release);
        let mut fed = Fed::spawn(ramp, Some(download)).unwrap();
        decoded(&script, PER_BLOCK * 2);
        sound(&mut fed, PER_BLOCK * 2 * 2);
        assert_eq!(fed.next(), Some(0.0), "stalled");
        assert_eq!(fed.next(), Some(0.0));

        fed.try_seek(Duration::from_secs(30)).unwrap();
        let said = fed.feed.answer_within(Duration::from_secs(2)).expect("answered");
        assert!(said.is_err(), "{said:?}");
        wait_for("it never went back", || *script.sought.lock().unwrap() == [PER_BLOCK * 2]);

        // The download moves again: the very next frame, and the position
        // says that place.
        script.released.store(true, Ordering::Release);
        decoded(&script, PER_BLOCK * 6);
        let on = sound(&mut fed, PER_BLOCK * 2);
        let last = &on[on.len() - 2..];
        assert_eq!(frame_of(last[0]), PER_BLOCK * 3 - 1, "nothing was skipped");
        assert!((fed.feed.position() - (PER_BLOCK * 3) as f64 / 8000.0).abs() < 1e-9);
        assert!(!script.read_on_torn.load(Ordering::Acquire), "it read on from a read it gave up");
    }
}
