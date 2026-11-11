//! A sync's waveforms (the firmware repo's docs/WAVEFORM-INDEX.md, whose
//! sections the numbers below are): at the final commit, the peaks of each
//! recording no old slot holds, from the server's own waveform route, and 7.3's
//! answer for what came ([`index`]). What a row is and which old slot holds are
//! [`card::waves`](super::super::waves)'s.
//!
//! **One recording at a time, a few at once** ([`fetch`]). The route has no
//! batch and no way to ask only for what it has: a track it hasn't seen takes
//! a decode, up to 30 s, and the server runs two. So [`AT_ONCE`] workers ask, on
//! threads of their own, and one goes once an answer has been slow enough to be
//! a decode ([`Pace::decode`]): a third would only wait behind those two. The
//! answers come back to the run's thread, which says them ([`Event::Waves`]).
//!
//! **No answer here fails a run** (12). The listener's stop, a sign-in refused,
//! a server out of reach ([`TRIES`]) or asking to be left alone, half an hour
//! gone ([`Pace::budget`]): nothing more is asked, what is on its way is
//! dropped, what came goes beside the old rows that still hold, 7.3's table
//! answers, and one note says so. The next run asks for the rest. One
//! recording's 503 says nothing of the next's: the route says it only on a miss,
//! and nothing latches ([`Peaks`]); but its Retry-After is waited out a few
//! times a run at most ([`FFMPEG_WAITS`]), and a server that has answered
//! "no ffmpeg" [`Pace::no_ffmpeg`] times with no bars yet is asked no more:
//! the run counts as one that found no ffmpeg ([`Lack::Ffmpeg`]). Nor does a
//! 404 for one path of a recording say anything of its other paths
//! ([`Asking::one`]).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use crate::api::transfer::Peaks;
use crate::api::{ApiError, Client};
use crate::device::card::contract::mpwf::bars_from;
use crate::device::card::engine::{Commit, Event, Waves};
use crate::device::card::source::http::until_with;
use crate::device::card::waves::{self, Ask, Index, NoIndex, Shapes};

/// The workers that ask at first, and those that go on once a decode shows.
const AT_ONCE: usize = 3;
const AT_ONCE_DECODING: usize = 2;
/// Tries of one recording: a first, then two more. Network errors wait a
/// second, then two, as a conversion's do; the third ends the asking.
const TRIES: u32 = 3;
/// What a 429 with no Retry-After waits.
const LIMITED_WAIT: Duration = Duration::from_secs(5);
/// A Retry-After past this isn't waited out with the card held: the next run
/// asks.
const LONGEST_WAIT: Duration = Duration::from_secs(300);
/// How often a wait looks at the stop.
const BEAT: Duration = Duration::from_millis(100);
/// A "no ffmpeg" 503's Retry-After is waited out this many times a run;
/// past them such an answer is taken at once, for the next run to ask again.
const FFMPEG_WAITS: usize = 3;

/// How a run's fetch goes.
#[derive(Clone, Copy, Debug)]
pub(super) struct Pace {
    /// How long a run asks, at most: past it what came is kept.
    pub(super) budget: Duration,
    /// An answer this slow was a decode.
    pub(super) decode: Duration,
    /// "No ffmpeg" answers, with no bars come yet, that end the asking
    /// ([`Cut::NoFfmpeg`]).
    pub(super) no_ffmpeg: usize,
}

pub(super) const PACE: Pace =
    Pace { budget: Duration::from_secs(30 * 60), decode: Duration::from_secs(2), no_ffmpeg: 20 };

/// The runs in a row that came to [`Lack::Ffmpeg`] before a look stops offering
/// the card its waveforms: a server bringing its ffmpeg up has it by then, and
/// one that has none isn't a Sync ▸ for ever.
pub(super) const NO_FFMPEG_RUNS: u32 = 3;

/// Why a fetch stopped asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Cut {
    /// The listener's stop.
    Stopped,
    /// A 401.
    SignIn,
    /// One recording's tries spent on an error: its words.
    Lost(String),
    /// The server keeps asking to be left alone, or for longer than a run waits.
    Limited,
    /// The budget is spent.
    Budget,
    /// The server answered "no ffmpeg" [`Pace::no_ffmpeg`] times, and no
    /// bars came.
    NoFfmpeg,
}

/// What a fetch brought.
pub(super) struct Fetched {
    pub(super) shapes: Shapes,
    /// Every recording asked was answered.
    pub(super) whole: bool,
    /// Those of them the server had no ffmpeg to make: no shape yet. The others
    /// with no bars have none to give.
    pub(super) no_ffmpeg: usize,
    /// Why it stopped asking, when it did.
    pub(super) cut: Option<Cut>,
    /// What it passed by, in a line.
    pub(super) note: Option<String>,
}

impl Fetched {
    /// What a card it leaves with no row lacks.
    pub(super) fn lack(&self) -> Lack {
        match (self.whole, self.no_ffmpeg, &self.cut) {
            (false, _, Some(Cut::NoFfmpeg)) => Lack::Ffmpeg,
            (false, _, _) => Lack::Unknown,
            (true, 0, _) => Lack::Peaks,
            (true, _, _) => Lack::Ffmpeg,
        }
    }
}

/// What a run's asking says a card it leaves with no row lacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lack {
    /// It can't say: the card has rows, or the asking was cut short.
    Unknown,
    /// Peaks the server doesn't have: every recording was answered, with none
    /// ([`crate::device::card::state::State::waves_none`]).
    Peaks,
    /// An ffmpeg on the server to make them: every recording was answered, some
    /// of them only so. A server still bringing its ffmpeg up says the same, and
    /// has them a minute later
    /// ([`crate::device::card::state::State::waves_no_ffmpeg`]).
    Ffmpeg,
}

/// What came of a run's asking.
pub(super) struct Came {
    /// What the new root names.
    pub(super) waves: Waves,
    pub(super) note: Option<String>,
    /// What it says the card lacks, when it leaves it with no row.
    pub(super) lack: Lack,
}

/// A run's step: the records' recordings no old slot holds, asked of `client`,
/// the run's own server (a peer's rows have no route to ask, unless its
/// recording is the server's too: [`waves::asks`]), and 7.3's answer.
pub(super) fn index(
    client: &Client,
    commit: &Commit<'_>,
    stop: Option<&AtomicBool>,
    events: &mut dyn FnMut(Event),
) -> Came {
    let own = commit.origins.first().copied().flatten();
    let asks = waves::asks(commit.records, commit.old_waves, |server| own == Some(server));
    let fetched = fetch(client, &asks, stop, PACE, events);
    match commit.waves_index(&fetched.shapes) {
        Ok(Index::None(NoIndex::NoRows)) => {
            let none = "no waveform index: the server has no peaks for the card's files";
            let lack = fetched.lack();
            let note = fetched.note.or_else(|| (!asks.is_empty()).then(|| none.to_string()));
            Came { waves: Waves::None, note, lack }
        }
        Ok(index) => Came { waves: Waves::from(index), note: fetched.note, lack: Lack::Unknown },
        // More rows than a file holds, which no card reaches: what the root
        // names stays.
        Err(e) => {
            let note = Some(format!("no waveform index: {e}"));
            Came { waves: Waves::Same, note, lack: Lack::Unknown }
        }
    }
}

/// See the module's note: each of `asks` asked of `client`, the answers told
/// to `events` as they come.
pub(super) fn fetch(
    client: &Client,
    asks: &[Ask],
    stop: Option<&AtomicBool>,
    pace: Pace,
    events: &mut dyn FnMut(Event),
) -> Fetched {
    let of = asks.len();
    let mut shapes = Shapes::new();
    let (mut done, mut no_ffmpeg) = (0, 0);
    if of == 0 {
        return Fetched { shapes, whole: true, no_ffmpeg, cut: None, note: None };
    }
    let asking = Asking {
        client,
        asks,
        stop,
        pace,
        deadline: Instant::now() + pace.budget,
        next: AtomicUsize::new(0),
        width: AtomicUsize::new(AT_ONCE),
        cut: Mutex::new(None),
        not_before: Mutex::new(Instant::now()),
        ffmpeg_waits: AtomicUsize::new(FFMPEG_WAITS),
        no_ffmpeg: AtomicUsize::new(0),
        bars: AtomicBool::new(false),
    };
    events(Event::Waves { done, of });
    let (tx, answers) = channel();
    std::thread::scope(|scope| {
        for me in 0..AT_ONCE.min(of) {
            let (asking, tx) = (&asking, tx.clone());
            scope.spawn(move || asking.work(me, &tx));
        }
        drop(tx);
        for (at, peaks) in answers {
            done += 1;
            match peaks {
                Peaks::Bars(peaks) => {
                    shapes.insert(asks[at].hash, bars_from(&peaks));
                }
                Peaks::NoFfmpeg(_) => no_ffmpeg += 1,
                Peaks::None | Peaks::Limited(_) => {}
            }
            events(Event::Waves { done, of });
        }
    });
    let cut = asking.cut.into_inner().unwrap_or_else(|e| e.into_inner());
    let note = note_of(cut.as_ref(), done, shapes.len(), no_ffmpeg, of);
    let cut = cut.filter(|_| done < of);
    Fetched { shapes, whole: done == of, no_ffmpeg, cut, note }
}

/// What a fetch passed by, in a line: why it stopped asking, unless every one
/// was answered all the same, else what the server had no ffmpeg to make. A
/// track the server just has no peaks for is a plain line, nothing to say (12).
pub(super) fn note_of(
    cut: Option<&Cut>,
    done: usize,
    came: usize,
    no_ffmpeg: usize,
    of: usize,
) -> Option<String> {
    let why = match cut.filter(|_| done < of) {
        Some(Cut::Stopped) => "stopped at the listener's word".to_string(),
        Some(Cut::SignIn) => "the server no longer takes this sign-in".to_string(),
        Some(Cut::Lost(why)) => format!("the server stopped answering ({why})"),
        Some(Cut::Limited) => "the server asks to be left alone for now".to_string(),
        Some(Cut::Budget) => "a run gives them half an hour".to_string(),
        Some(Cut::NoFfmpeg) => "the server has no ffmpeg to make them".to_string(),
        None if no_ffmpeg > 0 => {
            let words = "the server has no ffmpeg to make";
            return Some(format!("waveforms: {words} {no_ffmpeg} of the {of} asked for: no shape yet"));
        }
        None => return None,
    };
    Some(format!("waveforms: {came} of {of} came: {why}; the next sync asks for the rest"))
}

/// What the workers share.
struct Asking<'a> {
    client: &'a Client,
    asks: &'a [Ask],
    stop: Option<&'a AtomicBool>,
    pace: Pace,
    deadline: Instant,
    /// The next of `asks` no worker has taken.
    next: AtomicUsize,
    /// How many of the workers go on taking them.
    width: AtomicUsize,
    /// Why nothing more is asked, once that is so: the first reason.
    cut: Mutex<Option<Cut>>,
    /// Nothing is asked before this: the server's Retry-After.
    not_before: Mutex<Instant>,
    /// The "no ffmpeg" Retry-Afters still to wait out this run.
    ffmpeg_waits: AtomicUsize,
    /// The "no ffmpeg" answers so far, and whether any bars came.
    no_ffmpeg: AtomicUsize,
    bars: AtomicBool,
}

impl Asking<'_> {
    /// Worker `me`: until nothing is left to ask, nothing more is to be asked,
    /// or a decode showed it is one too many. What is left comes first: a stop
    /// or the budget past the last answer cut nothing.
    fn work(&self, me: usize, answers: &Sender<(usize, Peaks)>) {
        let left = || self.next.load(Ordering::Relaxed) < self.asks.len();
        while me < self.width.load(Ordering::Relaxed) && left() && !self.over() {
            let at = self.next.fetch_add(1, Ordering::Relaxed);
            let Some(ask) = self.asks.get(at) else { return };
            let Some(peaks) = self.one(ask) else { return };
            if answers.send((at, peaks)).is_err() {
                return;
            }
        }
    }

    /// One recording's answer; None when nothing more is to be asked. The route
    /// looks for the file before anything else, so one of the recording's files
    /// gone from its path since the listing is a 404 with the peaks still there
    /// under another of them: a path with nothing for it gives way to the next.
    fn one(&self, ask: &Ask) -> Option<Peaks> {
        for path in &ask.server_paths {
            match self.of_path(path)? {
                Peaks::None => {}
                peaks => return Some(peaks),
            }
        }
        Some(Peaks::None)
    }

    /// The route's answer for one path, its tries spent; None when nothing more
    /// is to be asked.
    fn of_path(&self, path: &str) -> Option<Peaks> {
        let mut mine = Instant::now();
        for attempt in 1..=TRIES {
            if !self.held(mine) {
                return None;
            }
            let last = attempt == TRIES;
            let t0 = Instant::now();
            let asked = self.client.waveform_peaks_async(path);
            let asked = until_with(asked, &|| self.over())?;
            if asked.is_ok() && t0.elapsed() >= self.pace.decode {
                self.width.store(AT_ONCE_DECODING, Ordering::Relaxed);
            }
            match asked {
                Ok(Peaks::Limited(_)) if last => self.cut_for(Cut::Limited),
                Ok(Peaks::Limited(after)) => self.later(after.unwrap_or(LIMITED_WAIT)),
                Ok(Peaks::NoFfmpeg(Some(after))) if !last && self.waits_for_ffmpeg() => self.later(after),
                Ok(peaks) => {
                    self.answered(&peaks);
                    return Some(peaks);
                }
                Err(ApiError::Unauthorized) => self.cut_for(Cut::SignIn),
                Err(e) if last => self.cut_for(Cut::Lost(e.to_string())),
                Err(_) => mine = Instant::now() + Duration::from_secs(u64::from(attempt)),
            }
        }
        None
    }

    /// A "no ffmpeg" 503's Retry-After is still one to wait out this run.
    fn waits_for_ffmpeg(&self) -> bool {
        self.ffmpeg_waits.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok()
    }

    /// An answer taken: enough "no ffmpeg" ones with no bars yet end the
    /// asking.
    fn answered(&self, peaks: &Peaks) {
        match peaks {
            Peaks::Bars(_) => self.bars.store(true, Ordering::SeqCst),
            Peaks::NoFfmpeg(_) => {
                let n = self.no_ffmpeg.fetch_add(1, Ordering::SeqCst) + 1;
                if n >= self.pace.no_ffmpeg && !self.bars.load(Ordering::SeqCst) {
                    self.cut_for(Cut::NoFfmpeg);
                }
            }
            _ => {}
        }
    }

    /// Nothing more is to be asked, for the first reason that came.
    fn cut_for(&self, why: Cut) {
        self.cut.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(why);
    }

    /// Nothing more is to be asked, the listener's stop and the budget looked at.
    fn over(&self) -> bool {
        if self.stop.is_some_and(|stop| stop.load(Ordering::SeqCst)) {
            self.cut_for(Cut::Stopped);
        } else if Instant::now() >= self.deadline {
            self.cut_for(Cut::Budget);
        }
        self.cut.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    /// The server's Retry-After, every worker's to keep.
    fn later(&self, after: Duration) {
        if after > LONGEST_WAIT {
            self.cut_for(Cut::Limited);
            return;
        }
        let mut not_before = self.not_before.lock().unwrap_or_else(|e| e.into_inner());
        *not_before = (*not_before).max(Instant::now() + after);
    }

    /// Waits for `mine` and the server's Retry-After, whichever is later; false
    /// when nothing more is to be asked.
    fn held(&self, mine: Instant) -> bool {
        loop {
            if self.over() {
                return false;
            }
            let until = mine.max(*self.not_before.lock().unwrap_or_else(|e| e.into_inner()));
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            std::thread::sleep(left.min(BEAT));
        }
    }
}
