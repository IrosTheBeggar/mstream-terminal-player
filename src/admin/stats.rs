//! The stats page: `mstream-player stats` — the server's /stats page (the
//! Stats API v2, mStream 6.27) as one page in the hub's chrome, for any
//! signed-in account: stats are per account, so it lives beside the admin
//! rooms, not under them. Three tabs on ←→ — Overview (the webapp's six
//! tiles, plays per day, when you listen, where the tracks live), Top
//! (tracks, artists, albums or genres ranked by plays or by time) and
//! Recent (the log, newest first, paged as the cursor nears its end; `x`
//! forgets a play behind a gate). The period is a dropdown — `This month ▾`
//! on the Overview's controls row, leading the other tabs' notes — whose
//! list drops on `p` or a click; `[` `]` step it. The origin (all · this
//! server · peers) cycles on `o` while the log holds peer plays. Everything
//! loads on entry and on a change — no poll, like the webapp — and a change
//! keeps the page as it stands until the new numbers land, so nothing
//! blinks. Nothing here plays music: it is a report.
//!
//! This file is the page itself: its state, its worker, its keys and every
//! word it says. It names no type of ratatui's or the kit's. Two views draw
//! it, each a child module that reads the page the same way: `cells`
//! (cells.rs) in character cells, for a terminal and for the GUI's grid,
//! and `egui_view` (egui_view.rs) in pixels, in the window's panel. A click
//! in either is one of the page's [`Act`]s and a key is [`Page::key`], so
//! the two cannot disagree about what the page does. What a view keeps
//! between frames it keeps in a type of its own, held here unopened
//! (`cells::State`).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use rust_i18n::t;

use super::tz::{self, Zone};
use super::{Outcome, fmt_count, gate_message, iso_unix, printable, unix_now};
use crate::api::types::{HistoryItem, PeriodOption, StatsHistory, StatsPeriods, StatsSummary, StatsTimeseries, StatsTop, TopItem};
use crate::api::{ApiError, Client};
use crate::input::{KeyCode, KeyEvent};

/// The page in character cells: the terminal's page and the GUI's grid.
pub(crate) mod cells;
/// The same page drawn by egui, in the window's panel (the egui spike).
#[cfg(feature = "window")]
pub(crate) mod egui_view;

/// The webapp's "show all": twenty rows of the ranking.
const TOP_LIMIT: u32 = 20;
/// One page of the log; the next is asked for when the cursor is within
/// [`PAGE_AHEAD`] rows of the end.
const HISTORY_PAGE: u32 = 50;
const PAGE_AHEAD: usize = 10;
const TEXT_MAX: usize = 120;

// ── The page's vocabulary ─────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tab {
    Overview,
    Top,
    Recent,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Overview, Tab::Top, Tab::Recent];

    fn next(self, forward: bool) -> Tab {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Tab::ALL[if forward { (i + 1) % 3 } else { (i + 2) % 3 }]
    }

    fn name(self) -> String {
        match self {
            Tab::Overview => t!("sta.tab_overview"),
            Tab::Top => t!("sta.tab_top"),
            Tab::Recent => t!("sta.tab_recent"),
        }
        .to_string()
    }

    /// The pane's one-line note at the tab row's right; the Overview has
    /// none — its tiles say what it shows.
    fn note(self) -> Option<String> {
        match self {
            Tab::Overview => None,
            Tab::Top => Some(t!("sta.note_top").to_string()),
            Tab::Recent => Some(t!("sta.note_recent").to_string()),
        }
    }
}

/// Whose tracks: every play, the plays of this server's own library, or
/// the plays of federated peers' tracks made through this server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    All,
    Local,
    Peers,
}

impl Origin {
    const ALL: [Origin; 3] = [Origin::All, Origin::Local, Origin::Peers];

    fn param(self) -> &'static str {
        match self {
            Origin::All => "all",
            Origin::Local => "local",
            Origin::Peers => "peers",
        }
    }

    fn label(self) -> String {
        match self {
            Origin::All => t!("sta.origin_all"),
            Origin::Local => t!("sta.origin_local"),
            Origin::Peers => t!("sta.origin_peers"),
        }
        .to_string()
    }

    fn index(self) -> usize {
        Origin::ALL.iter().position(|o| *o == self).unwrap_or(0)
    }
}

/// What the Top tab ranks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entity {
    Tracks,
    Artists,
    Albums,
    Genres,
}

impl Entity {
    const ALL: [Entity; 4] = [Entity::Tracks, Entity::Artists, Entity::Albums, Entity::Genres];

    fn param(self) -> &'static str {
        match self {
            Entity::Tracks => "tracks",
            Entity::Artists => "artists",
            Entity::Albums => "albums",
            Entity::Genres => "genres",
        }
    }

    fn label(self) -> String {
        match self {
            Entity::Tracks => t!("sta.ent_tracks"),
            Entity::Artists => t!("sta.ent_artists"),
            Entity::Albums => t!("sta.ent_albums"),
            Entity::Genres => t!("sta.ent_genres"),
        }
        .to_string()
    }

    /// The name column's heading.
    fn header(self) -> String {
        match self {
            Entity::Tracks => t!("sta.col_track"),
            Entity::Artists => t!("sta.col_artist"),
            Entity::Albums => t!("sta.col_album"),
            Entity::Genres => t!("sta.col_genre"),
        }
        .to_string()
    }

    fn index(self) -> usize {
        Entity::ALL.iter().position(|e| *e == self).unwrap_or(0)
    }
}

/// What the ranking orders by — and what the share bar measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Metric {
    Plays,
    Time,
}

impl Metric {
    fn param(self) -> &'static str {
        match self {
            Metric::Plays => "plays",
            Metric::Time => "time",
        }
    }

    fn label(self) -> String {
        match self {
            Metric::Plays => t!("sta.met_plays"),
            Metric::Time => t!("sta.met_time"),
        }
        .to_string()
    }

    fn of(self, item: &TopItem) -> u64 {
        match self {
            Metric::Plays => item.plays,
            Metric::Time => item.listened_ms,
        }
    }
}

/// One entry of the period list: a preset and its offset, named the way
/// the webapp's select names it — "This month · September 2026", "Last
/// week · Week of 2026-08-31", the deeper months and years by their own
/// names, then "All time".
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Period {
    pub period: String,
    pub offset: i32,
    pub name: String,
    pub detail: Option<String>,
}

impl Period {
    fn this_month() -> Period {
        Period { period: "month".into(), offset: 0, name: t!("sta.this_month").to_string(), detail: None }
    }

    fn all_time() -> Period {
        Period { period: "all".into(), offset: 0, name: t!("sta.all_time").to_string(), detail: None }
    }

    fn is(&self, period: &str, offset: i32) -> bool {
        self.period == period && self.offset == offset
    }

    /// The words a "vs" clause names it by.
    fn versus(&self) -> String {
        self.detail.clone().unwrap_or_else(|| self.name.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Modal {
    None,
    /// The period list, dropped under the control, with its cursor.
    Period(usize),
    /// `x`: the forget gate, on a row of the log.
    Forget(usize),
}

/// Everything a click can mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Act {
    Tab(Tab),
    Select(usize),
    TableScroll(i8),
    TableScrollTo(usize),
    PeriodStep(i8),
    /// The control: the list drops, or closes if it is open.
    PeriodList,
    /// The cursor onto a row of the list (the keys' way through it).
    PeriodRow(usize),
    /// A row of the list picked: the period, and the list closes.
    PeriodPick(usize),
    PeriodClose,
    Origin(usize),
    Entity(usize),
    Metric(usize),
    Forget,
    ForgetConfirm,
    ForgetCancel,
    Quit,
}

/// What one read of a period comes back as: the answers of every call the
/// page draws from.
pub(crate) struct Loaded {
    pub periods: Option<StatsPeriods>,
    pub summary: StatsSummary,
    pub previous: Option<StatsSummary>,
    pub series: StatsTimeseries,
    pub hours: StatsTimeseries,
    pub top: StatsTop,
    pub history: StatsHistory,
}

/// The reads and the one write, queued from input and run after the next
/// draw. `seq` marks the state a read was asked for: an answer to an older
/// question is dropped.
#[derive(Debug, Clone, PartialEq)]
enum Op {
    Load { seq: u64, period: Period, origin: Origin, entity: Entity, metric: Metric, tz: String },
    Top { seq: u64, period: Period, origin: Origin, entity: Entity, metric: Metric, tz: String },
    History { seq: u64, period: Period, origin: Origin, tz: String, before: String },
    Forget { id: String, row: usize },
}

enum Done {
    Loaded { seq: u64, result: Result<Box<Loaded>, ApiError> },
    Top { seq: u64, result: Result<StatsTop, ApiError> },
    History { seq: u64, result: Result<StatsHistory, ApiError> },
    Forgot { id: String, row: usize, result: Result<(), ApiError> },
}

/// The bucket the period's chart is drawn in: days while a row of days
/// fits, weeks for a half-year, months for a year and for all time — the
/// webapp draws days up to a half-year and months beyond, in pixels.
fn series_bucket(period: &str) -> &'static str {
    match period {
        "half" => "week",
        "year" | "all" => "month",
        _ => "day",
    }
}

fn spawn_worker() -> (Sender<(Arc<Client>, Op)>, Receiver<Done>) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<(Arc<Client>, Op)>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Done>();
    std::thread::spawn(move || {
        while let Ok((client, op)) = job_rx.recv() {
            let done = match op {
                Op::Load { seq, period, origin, entity, metric, tz } => {
                    // No read needs another's answer, so all seven go out at
                    // once, as the webapp's Promise.all sends them: one round
                    // trip, not seven — over a relay that was a second a step
                    // (performance audit #83). The answers are read in the
                    // order they were once asked, so a failure names what it
                    // did: the summary's first (a pre-6.27 server's 404 is
                    // the missing API), then the rest.
                    let (p, o) = (period.period.as_str(), origin.param());
                    let result = crate::api::wait(async {
                        let previous = async {
                            match p {
                                "all" => None,
                                _ => client.stats_summary_async(p, period.offset - 1, &tz, o).await.ok(),
                            }
                        };
                        let (periods, summary, previous, series, hours, top, history) = tokio::join!(
                            client.stats_periods_async(&tz),
                            client.stats_summary_async(p, period.offset, &tz, o),
                            previous,
                            client.stats_timeseries_async(series_bucket(p), p, period.offset, &tz, o),
                            client.stats_timeseries_async("hourOfDay", p, period.offset, &tz, o),
                            client.stats_top_async(entity.param(), metric.param(), p, period.offset, &tz, o, TOP_LIMIT),
                            client.stats_history_async(p, period.offset, &tz, o, None, HISTORY_PAGE),
                        );
                        // The periods list is best-effort: the totals are the point.
                        let periods = periods.ok();
                        let summary = summary?;
                        let (series, hours, top, history) = (series?, hours?, top?, history?);
                        Ok(Box::new(Loaded { periods, summary, previous, series, hours, top, history }))
                    });
                    Done::Loaded { seq, result }
                }
                Op::Top { seq, period, origin, entity, metric, tz } => Done::Top {
                    seq,
                    result: client.stats_top(entity.param(), metric.param(), &period.period, period.offset, &tz, origin.param(), TOP_LIMIT),
                },
                Op::History { seq, period, origin, tz, before } => Done::History {
                    seq,
                    result: client.stats_history(&period.period, period.offset, &tz, origin.param(), Some(&before), HISTORY_PAGE),
                },
                Op::Forget { id, row } => {
                    let result = client.stats_forget(&id).map(|_| ());
                    Done::Forgot { id, row, result }
                }
            };
            if done_tx.send(done).is_err() {
                return;
            }
        }
    });
    (job_tx, done_rx)
}

pub(crate) struct Page {
    client: Arc<Client>,
    to_worker: Sender<(Arc<Client>, Op)>,
    from_worker: Receiver<Done>,
    /// The account the session signs in as, for the header — none on a
    /// public-mode server, which logs under its own sentinel account.
    pub username: Option<String>,
    /// The machine's zone: its name goes to the server, its offsets put
    /// each play on the local clock. None means UTC, and the page says so.
    zone: Option<Zone>,
    tz: String,
    pub tab: Tab,
    /// The period chosen — what the control names.
    pub period: Period,
    /// The period the numbers on the page belong to: `period` once its
    /// read lands, the one before while that read is in flight — the
    /// tiles compare, and an empty period is named, by this one.
    pub shown: Period,
    /// The periods with data, the webapp's select — from `/stats/periods`.
    pub options: Vec<Period>,
    pub origin: Origin,
    pub entity: Entity,
    pub metric: Metric,
    pub data: Option<Loaded>,
    /// The log so far (the first page and the ones scrolled to), and the
    /// cursor for the next page.
    pub history: Vec<HistoryItem>,
    pub next: Option<String>,
    pub top: Vec<TopItem>,
    /// The server answered 404: it predates the Stats API.
    pub no_api: bool,
    pub sel: Option<usize>,
    pub modal: Modal,
    pub note: Option<(String, bool)>,
    busy: Option<String>,
    queued: Option<Op>,
    in_flight: bool,
    seq: u64,
    tscroll: usize,
    /// Drawn inside another shell — the GUI's Stats screen (the stats-screen
    /// contract): no header row and no tips row of its own, the host's top
    /// bar and footer carry those, and the ground is the host's.
    hosted: bool,
    /// What the cells view keeps here between frames (cells.rs): its
    /// surface, and where it last drew the period's name and the cursor.
    cells: cells::State,
}

/// The page, loading: what `mstream-player stats` opens.
pub(crate) fn start(client: Client, username: Option<String>) -> Page {
    let mut page = Page::new(client, username, tz::local());
    page.reload(true);
    page
}

impl Page {
    pub(crate) fn new(client: Client, username: Option<String>, zone: Option<Zone>) -> Self {
        let (to_worker, from_worker) = spawn_worker();
        let tz = zone.as_ref().map_or_else(|| "UTC".to_string(), |z| z.name.clone());
        Page {
            client: Arc::new(client),
            to_worker,
            from_worker,
            username,
            zone,
            tz,
            tab: Tab::Overview,
            period: Period::this_month(),
            shown: Period::this_month(),
            options: Vec::new(),
            origin: Origin::All,
            entity: Entity::Tracks,
            metric: Metric::Plays,
            data: None,
            history: Vec::new(),
            next: None,
            top: Vec::new(),
            no_api: false,
            sel: None,
            modal: Modal::None,
            note: None,
            busy: None,
            queued: None,
            in_flight: false,
            seq: 0,
            tscroll: 0,
            hosted: false,
            cells: cells::State::new(),
        }
    }

    fn summary(&self) -> Option<&StatsSummary> {
        self.data.as_ref().map(|d| &d.summary)
    }

    /// Whether the log holds peer plays — the origin control shows only then.
    fn peer_plays(&self) -> bool {
        self.summary().is_some_and(|s| s.origins.peers.plays > 0) || self.origin != Origin::All
    }

    /// Whether the log has nothing at all, not just nothing in this period.
    fn no_plays_ever(&self) -> bool {
        self.data.as_ref().is_some_and(|d| {
            d.summary.events == 0 && d.periods.as_ref().is_none_or(|p| p.periods.is_empty() && p.earliest.is_none())
        })
    }

    fn rows(&self) -> usize {
        match self.tab {
            Tab::Overview => 0,
            Tab::Top => self.top.len(),
            Tab::Recent => self.history.len(),
        }
    }

    fn offset_at(&self, t: i64) -> i32 {
        self.zone.as_ref().map_or(0, |z| z.offset_at(t))
    }

    fn queue(&mut self, op: Op, busy: impl Into<String>) {
        self.queued = Some(op);
        self.busy = Some(busy.into());
    }

    /// The periods to pick from: the server's list, or This month and All
    /// time before it has answered.
    fn choices(&self) -> Vec<Period> {
        if self.options.is_empty() { vec![Period::this_month(), Period::all_time()] } else { self.options.clone() }
    }

    /// Ask for everything again, for the period and origin as they stand.
    /// The page as the GUI's Stats screen hosts it.
    pub(crate) fn hosted(mut self) -> Self {
        self.hosted = true;
        self
    }

    /// Whether one of the page's own modals — the period list, the forget
    /// gate — is up: it keeps every key while it is.
    pub(crate) fn modal_open(&self) -> bool {
        !matches!(self.modal, Modal::None)
    }

    fn reload(&mut self, loud: bool) {
        self.seq += 1;
        let op = Op::Load {
            seq: self.seq,
            period: self.period.clone(),
            origin: self.origin,
            entity: self.entity,
            metric: self.metric,
            tz: self.tz.clone(),
        };
        if loud {
            self.queue(op, t!("sta.busy_loading"));
        } else {
            self.queued = Some(op);
        }
    }

    /// A new period or origin: the page keeps the numbers it has, under
    /// the new name, until the new ones land — the note row says they are
    /// on their way — so nothing blinks (stats-screen contract, clause 16).
    /// The cursor lets go and the log's cursor is dropped: the rows are
    /// about to change, and the old log must not be paged for the new range.
    fn change_range(&mut self) {
        self.next = None;
        self.sel = None;
        self.tscroll = 0;
        self.note = None;
        self.reload(true);
    }

    fn reload_top(&mut self) {
        self.seq += 1;
        self.sel = None;
        self.tscroll = 0;
        let op = Op::Top {
            seq: self.seq,
            period: self.period.clone(),
            origin: self.origin,
            entity: self.entity,
            metric: self.metric,
            tz: self.tz.clone(),
        };
        self.queue(op, t!("sta.busy_top"));
    }

    /// The next page of the log, once the cursor is near the end of what
    /// is loaded. The cursor is taken while the page is in flight so the
    /// same edge asks only once.
    fn page_history(&mut self) {
        if self.tab != Tab::Recent || self.in_flight || self.queued.is_some() {
            return;
        }
        let near_end = self.sel.is_some_and(|s| s + PAGE_AHEAD >= self.history.len());
        if !near_end {
            return;
        }
        let Some(before) = self.next.take() else { return };
        let op = Op::History { seq: self.seq, period: self.period.clone(), origin: self.origin, tz: self.tz.clone(), before };
        self.queue(op, t!("sta.busy_more"));
    }

    /// A failed call, in words: the rooms' gate sentences, except that a
    /// 403 here is not about admin rights — the Stats API refuses a
    /// session with no account behind it (a federation key, a guest token).
    fn fail(&mut self, what: &str, e: ApiError) {
        let text = match e {
            ApiError::Forbidden(_) => t!("sta.gate_forbidden").to_string(),
            other => gate_message(&other, what),
        };
        self.note = Some((text, true));
    }

    fn set_period(&mut self, period: Period) {
        if period != self.period {
            self.period = period;
            self.change_range();
        }
    }

    // ── Input ───────────────────────────────────────────────────────────────

    /// A key press: the page's keymap, the same under either view.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        handle_key(self, key)
    }

    /// A click, drag or hold a view resolved to one of the page's actions.
    pub(crate) fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::Tab(tab) => {
                if tab != self.tab {
                    self.tab = tab;
                    self.sel = None;
                    self.tscroll = 0;
                    self.note = None;
                }
            }
            Act::Select(i) => {
                if i < self.rows() {
                    self.sel = Some(i);
                    self.note = None;
                    self.page_history();
                }
            }
            Act::TableScroll(d) => {
                self.tscroll = if d < 0 { self.tscroll.saturating_sub(1) } else { self.tscroll + 1 };
            }
            Act::TableScrollTo(row) => self.tscroll = row,
            Act::PeriodStep(d) => {
                let options = self.choices();
                let i = options.iter().position(|o| o.is(&self.period.period, self.period.offset)).unwrap_or(0);
                let j = if d < 0 { i.saturating_sub(1) } else { (i + 1).min(options.len() - 1) };
                self.set_period(options[j].clone());
            }
            Act::PeriodList => {
                if matches!(self.modal, Modal::Period(_)) {
                    self.modal = Modal::None;
                } else {
                    let i = self.choices().iter().position(|o| o.is(&self.period.period, self.period.offset)).unwrap_or(0);
                    self.modal = Modal::Period(i);
                }
            }
            Act::PeriodRow(i) => {
                let n = self.choices().len();
                if let Modal::Period(cursor) = &mut self.modal {
                    *cursor = i.min(n.saturating_sub(1));
                }
            }
            Act::PeriodPick(i) => {
                self.modal = Modal::None;
                if let Some(chosen) = self.choices().get(i).cloned() {
                    self.set_period(chosen);
                }
            }
            Act::PeriodClose => self.modal = Modal::None,
            Act::Origin(i) => {
                let origin = Origin::ALL[i.min(2)];
                if origin != self.origin {
                    self.origin = origin;
                    self.change_range();
                }
            }
            Act::Entity(i) => {
                let entity = Entity::ALL[i.min(3)];
                if entity != self.entity {
                    self.entity = entity;
                    self.reload_top();
                }
            }
            Act::Metric(i) => {
                let metric = if i == 0 { Metric::Plays } else { Metric::Time };
                if metric != self.metric {
                    self.metric = metric;
                    self.reload_top();
                }
            }
            Act::Forget => {
                if self.tab == Tab::Recent
                    && let Some(row) = self.sel
                    && row < self.history.len()
                {
                    self.modal = Modal::Forget(row);
                }
            }
            Act::ForgetConfirm => {
                if let Modal::Forget(row) = self.modal.clone() {
                    self.modal = Modal::None;
                    if let Some(item) = self.history.get(row) {
                        let id = item.id.clone();
                        self.queue(Op::Forget { id, row }, t!("sta.busy_forget"));
                    }
                }
            }
            Act::ForgetCancel => self.modal = Modal::None,
            Act::Quit => return Some(Outcome::Quit),
        }
        None
    }

    fn dispatch_queued(&mut self) {
        if self.in_flight {
            return;
        }
        let Some(op) = self.queued.take() else { return };
        self.in_flight = true;
        if self.to_worker.send((self.client.clone(), op)).is_err() {
            self.in_flight = false;
            self.note = Some((t!("note.worker_gone").to_string(), true));
        }
    }

    /// Fold one worker result back into the state.
    fn apply(&mut self, done: Done) {
        self.in_flight = false;
        self.busy = None;
        match done {
            Done::Loaded { seq, result } => {
                if seq != self.seq {
                    return; // an older question's answer
                }
                match result {
                    Ok(loaded) => {
                        self.no_api = false;
                        self.note = None;
                        // The seq matched: this answer is for the period and
                        // origin as they stand.
                        self.shown = self.period.clone();
                        self.options = loaded
                            .periods
                            .as_ref()
                            .map(|p| period_options(p, |t| self.offset_at(t)))
                            .unwrap_or_else(|| vec![Period::this_month(), Period::all_time()]);
                        self.top = loaded.top.items.clone();
                        self.history = loaded.history.items.clone();
                        self.next = loaded.history.next.clone();
                        let empty = loaded.summary.events == 0;
                        self.data = Some(*loaded);
                        // The webapp's move when the default period has no
                        // data: take the first period that has some.
                        if empty
                            && !self.options.iter().any(|o| o.is(&self.period.period, self.period.offset))
                            && let Some(first) = self.options.first().cloned()
                        {
                            self.period = first;
                            self.reload(true);
                        }
                    }
                    Err(ApiError::NotFound(_)) => {
                        self.no_api = true;
                        self.data = None;
                    }
                    Err(e) => self.fail(&t!("sta.fail_load"), e),
                }
            }
            Done::Top { seq, result } => {
                if seq != self.seq {
                    return;
                }
                match result {
                    Ok(top) => {
                        self.top = top.items;
                        if let Some(d) = &mut self.data {
                            d.top.entity = top.entity;
                            d.top.metric = top.metric;
                        }
                    }
                    Err(e) => self.fail(&t!("sta.fail_top"), e),
                }
            }
            Done::History { seq, result } => {
                if seq != self.seq {
                    return;
                }
                match result {
                    Ok(page) => {
                        self.history.extend(page.items);
                        self.next = page.next;
                    }
                    Err(e) => self.fail(&t!("sta.fail_more"), e),
                }
            }
            Done::Forgot { id, row, result } => match result {
                Ok(()) => {
                    if self.history.get(row).is_some_and(|h| h.id == id) {
                        self.history.remove(row);
                        if self.history.is_empty() {
                            self.sel = None;
                        } else if let Some(s) = self.sel {
                            self.sel = Some(s.min(self.history.len() - 1));
                        }
                    }
                    self.note = Some((t!("sta.done_forgot").to_string(), false));
                    // The totals moved with it: everything again, quietly,
                    // the log included — its first page restarts the paging.
                    self.reload(false);
                }
                Err(e) => self.fail(&t!("sta.fail_forget"), e),
            },
        }
    }

    /// Fold in whatever the worker finished, and say whether anything
    /// came: a frame to draw, whichever view draws it.
    pub(crate) fn absorb(&mut self) -> bool {
        let mut folded = false;
        loop {
            match self.from_worker.try_recv() {
                Ok(done) => self.apply(done),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.note = Some((t!("note.worker_gone").to_string(), true));
                    return true;
                }
            }
            folded = true;
        }
        folded
    }

    /// Hand the worker the next queued op.
    pub(crate) fn pump(&mut self) {
        self.dispatch_queued();
    }

    /// Whether a call is out with the worker.
    pub(crate) fn awaiting(&self) -> bool {
        self.in_flight
    }
}

// ── Keys ──────────────────────────────────────────────────────────────────

fn handle_key(page: &mut Page, key: KeyEvent) -> Option<Outcome> {
    let code = key.code;
    match page.modal.clone() {
        Modal::Period(cursor) => {
            let n = page.choices().len();
            return match code {
                KeyCode::Esc | KeyCode::Char('p') => page.act(Act::PeriodClose),
                KeyCode::Enter => page.act(Act::PeriodPick(cursor)),
                KeyCode::Up | KeyCode::BackTab => page.act(Act::PeriodRow(cursor.saturating_sub(1))),
                KeyCode::Down | KeyCode::Tab => page.act(Act::PeriodRow((cursor + 1).min(n.saturating_sub(1)))),
                _ => None,
            };
        }
        Modal::Forget(_) => {
            return match code {
                KeyCode::Char('y') => page.act(Act::ForgetConfirm),
                // Enter is the SAFE choice on a warning gate.
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n') => page.act(Act::ForgetCancel),
                _ => None,
            };
        }
        Modal::None => {}
    }

    let n = page.rows();
    match code {
        KeyCode::Left => page.act(Act::Tab(page.tab.next(false))),
        KeyCode::Right => page.act(Act::Tab(page.tab.next(true))),
        KeyCode::Up if n > 0 => {
            let i = page.sel.map_or(n - 1, |s| s.saturating_sub(1));
            page.act(Act::Select(i))
        }
        KeyCode::Down if n > 0 => {
            let i = page.sel.map_or(0, |s| (s + 1).min(n - 1));
            page.act(Act::Select(i))
        }
        KeyCode::Esc => {
            if page.sel.is_some() {
                page.sel = None;
                page.note = None;
                None
            } else {
                page.act(Act::Quit)
            }
        }
        KeyCode::Char('q') => page.act(Act::Quit),
        KeyCode::Char('[') => page.act(Act::PeriodStep(-1)),
        KeyCode::Char(']') => page.act(Act::PeriodStep(1)),
        KeyCode::Char('p') => page.act(Act::PeriodList),
        KeyCode::Char('o') if page.peer_plays() => page.act(Act::Origin((page.origin.index() + 1) % 3)),
        KeyCode::Char('t') if page.tab == Tab::Top => page.act(Act::Entity((page.entity.index() + 1) % 4)),
        KeyCode::Char('m') if page.tab == Tab::Top => page.act(Act::Metric(if page.metric == Metric::Plays { 1 } else { 0 })),
        KeyCode::Char('x') | KeyCode::Delete if page.tab == Tab::Recent => page.act(Act::Forget),
        _ => None,
    }
}

// ── The tips, and a row in words ──────────────────────────────────────────

/// The tips line: what the keys do here, in the order the design lists
/// them — the origin only while the control is drawn.
pub(crate) fn footer_hint(page: &Page) -> String {
    match &page.modal {
        Modal::Period(_) => return t!("sta.hint_period_modal", period = page.period.name.clone()).to_string(),
        Modal::Forget(_) => return t!("sta.hint_forget_modal").to_string(),
        Modal::None => {}
    }
    let mut parts: Vec<String> = Vec::new();
    let selected = page.sel.is_some();
    if page.tab != Tab::Overview && page.rows() > 0 {
        parts.push(if selected { t!("sta.hint_rows") } else { t!("sta.hint_select") }.to_string());
    }
    parts.push(t!("sta.hint_tabs").to_string());
    if page.tab == Tab::Top {
        parts.push(t!("sta.hint_entity").to_string());
        parts.push(t!("sta.hint_metric").to_string());
    }
    if page.tab == Tab::Recent && selected {
        parts.push(t!("sta.hint_forget").to_string());
    }
    if !page.no_plays_ever() && !page.no_api {
        parts.push(t!("sta.hint_period").to_string());
        if page.tab == Tab::Overview {
            parts.push(t!("sta.hint_pick").to_string());
        }
        if page.peer_plays() {
            parts.push(t!("sta.hint_origin").to_string());
        }
    }
    parts.push(if selected { t!("sta.hint_deselect") } else { t!("sta.hint_quit") }.to_string());
    parts.join(" · ")
}

/// One row of the log, in words.
struct RecentRow {
    time: String,
    day: String,
    track: String,
    via: Option<String>,
    outcome: String,
    completed: bool,
    listened: String,
    counted: bool,
    client: String,
    /// The client's name alone, for a column too narrow for its version.
    client_short: String,
}

fn recent_row(page: &Page, item: &HistoryItem, now: i64) -> RecentRow {
    let t = iso_unix(&item.started_at);
    let (title, artist, _, _) = track_words(item.track.as_ref());
    let legacy = item.source.as_deref() == Some("legacy");
    let (client, client_short) = client_words(item);
    RecentRow {
        time: t.map(|t| time_label(t, page.offset_at(t))).unwrap_or_default(),
        day: t.map(|t| day_label(t, now, page.offset_at(t), page.offset_at(now))).unwrap_or_default(),
        track: printable(&join_artist(&artist, &title), TEXT_MAX),
        via: (item.origin == "peer").then(|| via_words(item.peer_name.as_deref())),
        outcome: outcome_words(item),
        completed: item.outcome == "completed" && !legacy,
        listened: listened_words(item),
        counted: item.counted,
        client,
        client_short,
    }
}

/// The cursor row in words, for the note line.
fn row_words(page: &Page, i: usize) -> Option<String> {
    match page.tab {
        Tab::Overview => None,
        Tab::Top => {
            let item = page.top.get(i)?;
            let now = unix_now();
            let plays = plays_words(item.plays);
            let time = fmt_duration(item.listened_ms);
            Some(if page.entity == Entity::Tracks {
                let (title, artist, album, year) = track_words(item.track.as_ref());
                let track = printable(&join_artist(&artist, &title), TEXT_MAX);
                let last = item.last_played.as_deref().and_then(iso_unix).map(|t| day_label(t, now, page.offset_at(t), page.offset_at(now))).unwrap_or_default();
                match (album.is_empty(), year) {
                    (true, _) => t!("sta.note_top_track_plain", track = track, plays = plays, time = time, day = last).to_string(),
                    (false, Some(year)) => t!("sta.note_top_track", track = track, album = format!("{} ({year})", printable(&album, TEXT_MAX)), plays = plays, time = time, day = last).to_string(),
                    (false, None) => t!("sta.note_top_track", track = track, album = printable(&album, TEXT_MAX), plays = plays, time = time, day = last).to_string(),
                }
            } else {
                let name = printable(item.name.as_deref().unwrap_or(""), TEXT_MAX);
                t!("sta.note_top_group", name = name, tracks = tracks_words(item.tracks.unwrap_or(0)), plays = plays, time = time).to_string()
            })
        }
        Tab::Recent => {
            let item = page.history.get(i)?;
            let r = recent_row(page, item, unix_now());
            let mut words = t!("sta.row_recent", track = r.track, day = r.day, time = r.time, outcome = r.outcome).to_string();
            if !r.completed && item.duration_ms.is_some_and(|d| d > 0) {
                words.push_str(&t!("sta.row_recent_of", total = fmt_clock(item.duration_ms.unwrap_or(0))));
            }
            if !item.counted {
                words.push_str(&format!(" · {}", t!("sta.not_counted")));
            }
            words.push_str(&t!("sta.row_recent_forget"));
            Some(words)
        }
    }
}

// ── Periods ───────────────────────────────────────────────────────────────

/// The webapp's period options from `/stats/periods`: for each preset with
/// data, the current one as "This …" and the previous as "Last …", the
/// deeper months and years by name; then All time. A log with nothing in
/// it gets This month and All time.
pub(crate) fn period_options(periods: &StatsPeriods, offset_at: impl Fn(i64) -> i32) -> Vec<Period> {
    let mut out: Vec<Period> = Vec::new();
    for preset in ["week", "month", "quarter", "half", "year"] {
        for x in periods.periods.iter().filter(|x| x.period == preset) {
            if out.iter().any(|o| o.is(&x.period, x.offset)) {
                continue;
            }
            let label = period_label(x, &offset_at);
            let (name, detail) = match x.offset {
                0 => (this_name(preset), Some(label)),
                -1 => (last_name(preset), Some(label)),
                _ if preset == "month" || preset == "year" => (label, None),
                _ => continue,
            };
            out.push(Period { period: x.period.clone(), offset: x.offset, name, detail: detail.filter(|d| !d.is_empty()) });
        }
    }
    if out.is_empty() {
        out.push(Period::this_month());
    }
    out.push(Period::all_time());
    out
}

fn this_name(preset: &str) -> String {
    match preset {
        "week" => t!("sta.this_week"),
        "quarter" => t!("sta.this_quarter"),
        "half" => t!("sta.this_half"),
        "year" => t!("sta.this_year"),
        _ => t!("sta.this_month"),
    }
    .to_string()
}

fn last_name(preset: &str) -> String {
    match preset {
        "week" => t!("sta.last_week"),
        "quarter" => t!("sta.last_quarter"),
        "half" => t!("sta.last_half"),
        "year" => t!("sta.last_year"),
        _ => t!("sta.last_month"),
    }
    .to_string()
}

/// A period's own name: the week by its Monday, the month in the page's
/// language, the rest as the server labels it (`Q3 2026`, `H2 2026`, `2026`).
fn period_label(x: &PeriodOption, offset_at: &impl Fn(i64) -> i32) -> String {
    let from = iso_unix(&x.from);
    match (x.period.as_str(), from) {
        ("week", Some(t)) => {
            let (y, m, d) = tz::civil_from_days((t + offset_at(t) as i64).div_euclid(86_400));
            t!("sta.week_of", date = format!("{y:04}-{m:02}-{d:02}")).to_string()
        }
        ("month", Some(t)) => {
            let (y, m, _) = tz::civil_from_days((t + offset_at(t) as i64).div_euclid(86_400));
            t!("sta.month_year", month = month_name(m, "sta.months_title"), year = y).to_string()
        }
        _ => x.label.clone(),
    }
}

/// What a period's tiles compare against: the previous period by its own
/// name when the list has it, else "last month" and its kin. None for
/// All time.
fn versus_label(period: &Period, options: &[Period]) -> Option<String> {
    if period.period == "all" {
        return None;
    }
    if let Some(prev) = options.iter().find(|o| o.is(&period.period, period.offset - 1)) {
        return Some(prev.versus());
    }
    Some(
        match period.period.as_str() {
            "week" => t!("sta.vs_week"),
            "month" => t!("sta.vs_month"),
            "quarter" => t!("sta.vs_quarter"),
            "half" => t!("sta.vs_half"),
            "year" => t!("sta.vs_year"),
            _ => t!("sta.vs_previous"),
        }
        .to_string(),
    )
}

// ── The tiles ─────────────────────────────────────────────────────────────

/// The webapp's six tiles: `(value, label, detail)`.
pub(crate) fn tiles(s: &StatsSummary, previous: Option<&StatsSummary>, versus: Option<&str>) -> Vec<(String, String, String)> {
    let none = t!("sta.sub_none").to_string();
    let plays_sub = match versus {
        Some(vs) => delta_text(s.plays, previous.map(|p| p.plays), vs).unwrap_or_else(|| none.clone()),
        None => t!("sta.sub_counted").to_string(),
    };
    let time_sub = match versus {
        Some(vs) => delta_duration(s.listened_ms, previous.map(|p| p.listened_ms), vs).unwrap_or_else(|| none.clone()),
        None => t!("sta.sub_every").to_string(),
    };
    let tracks_sub = match s.library_coverage_pct {
        Some(pct) if pct.is_finite() => t!("sta.sub_library", pct = pct.round() as i64).to_string(),
        _ => t!("sta.sub_different").to_string(),
    };
    let skips_sub = match s.skip_rate {
        Some(rate) if rate.is_finite() => t!("sta.sub_starts", pct = (rate * 100.0).round() as i64).to_string(),
        _ => t!("sta.sub_early").to_string(),
    };
    let sessions_sub = match s.sessions.avg_ms {
        Some(avg) if avg > 0 => t!("sta.sub_each", d = fmt_duration(avg)).to_string(),
        _ => t!("sta.sub_sittings").to_string(),
    };
    vec![
        (fmt_count(s.plays), t!("sta.tile_plays").to_string(), plays_sub),
        (fmt_duration(s.listened_ms), t!("sta.tile_time").to_string(), time_sub),
        (fmt_count(s.unique_tracks), t!("sta.tile_tracks").to_string(), tracks_sub),
        (fmt_count(s.skips), t!("sta.tile_skips").to_string(), skips_sub),
        (days_words(s.streak_days.current as u64), t!("sta.tile_streak").to_string(), t!("sta.sub_longest", n = fmt_count(s.streak_days.longest as u64)).to_string()),
        (fmt_count(s.sessions.count), t!("sta.tile_sessions").to_string(), sessions_sub),
    ]
}

/// "+12% vs August 2026" · "−5% vs last week" · "same as August 2026" ·
/// None with nothing to compare with.
pub(crate) fn delta_text(current: u64, previous: Option<u64>, versus: &str) -> Option<String> {
    let previous = previous.filter(|p| *p > 0)?;
    let pct = ((current as f64 - previous as f64) / previous as f64 * 100.0).round() as i64;
    Some(match pct {
        0 => t!("sta.sub_same", vs = versus),
        p if p > 0 => t!("sta.sub_up", pct = p, vs = versus),
        p => t!("sta.sub_down", pct = -p, vs = versus),
    }
    .to_string())
}

/// Durations compare as time, not percent: "+1h 05m vs August 2026".
pub(crate) fn delta_duration(current_ms: u64, previous_ms: Option<u64>, versus: &str) -> Option<String> {
    let previous = previous_ms.filter(|p| *p > 0)?;
    let diff = current_ms as i64 - previous as i64;
    Some(if diff.abs() < 60_000 {
        t!("sta.sub_same", vs = versus)
    } else if diff > 0 {
        t!("sta.sub_time_up", d = fmt_duration(diff as u64), vs = versus)
    } else {
        t!("sta.sub_time_down", d = fmt_duration((-diff) as u64), vs = versus)
    }
    .to_string())
}

// ── The charts' series ────────────────────────────────────────────────────

/// The calendar series for a period: one value per bucket of the range
/// (`to` exclusive), zero where the server sent nothing — days, weeks by
/// their Monday, or months, as the answer's bucket says. Returns the
/// bucket keys, the values, and the bucket.
fn calendar_series(series: &StatsTimeseries, from: &str, to: &str, offset_at: impl Fn(i64) -> i32) -> (Vec<String>, Vec<u64>, &'static str) {
    let bucket = match series.bucket.as_str() {
        "week" => "week",
        "month" => "month",
        _ => "day",
    };
    let by_key: std::collections::HashMap<&str, u64> = series.items.iter().map(|b| (b.bucket.as_str(), b.plays)).collect();
    let span = iso_unix(from).zip(iso_unix(to)).filter(|(a, b)| a < b);
    let mut keys: Vec<String> = Vec::new();
    match span {
        None => {
            keys = series.items.iter().map(|b| b.bucket.clone()).collect();
            keys.sort();
        }
        Some((a, b)) => {
            let first = (a + offset_at(a) as i64).div_euclid(86_400);
            let last = (b - 1 + offset_at(b - 1) as i64).div_euclid(86_400);
            match bucket {
                "day" => {
                    let mut d = first;
                    while d <= last && keys.len() < 400 {
                        let (y, m, dd) = tz::civil_from_days(d);
                        keys.push(format!("{y:04}-{m:02}-{dd:02}"));
                        d += 1;
                    }
                }
                "week" => {
                    let mut d = first - (tz::weekday(first) as i64 + 6) % 7; // the Monday
                    while d <= last && keys.len() < 400 {
                        let (y, m, dd) = tz::civil_from_days(d);
                        keys.push(format!("{y:04}-{m:02}-{dd:02}"));
                        d += 7;
                    }
                }
                _ => {
                    let (mut y, mut m, _) = tz::civil_from_days(first);
                    let (ly, lm, _) = tz::civil_from_days(last);
                    while (y < ly || (y == ly && m <= lm)) && keys.len() < 400 {
                        keys.push(format!("{y:04}-{m:02}"));
                        m += 1;
                        if m == 13 {
                            m = 1;
                            y += 1;
                        }
                    }
                }
            }
        }
    }
    let values = keys.iter().map(|k| by_key.get(k.as_str()).copied().unwrap_or(0)).collect();
    (keys, values, bucket)
}

/// Which day columns get a label: the first, every fifth day of the month,
/// the last — every seventh column past forty of them.
fn day_axis(keys: &[String], fold: usize) -> Vec<(usize, String)> {
    let n = keys.len();
    let mut out = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let day: usize = key.get(8..10).and_then(|d| d.parse().ok()).unwrap_or(0);
        let wanted = if n > 40 { i == 0 || i == n - 1 || i.is_multiple_of(7) } else { i == 0 || i == n - 1 || day.is_multiple_of(5) };
        if wanted && i % fold == 0 {
            out.push((i / fold, day.to_string()));
        }
    }
    out
}

/// Week columns: the Monday's day and month, every fourth week.
fn week_axis(keys: &[String], fold: usize) -> Vec<(usize, String)> {
    keys.iter()
        .enumerate()
        .filter(|(i, _)| i % (4 * fold) == 0)
        .filter_map(|(i, key)| {
            let m: u32 = key.get(5..7)?.parse().ok()?;
            let d: u32 = key.get(8..10)?.parse().ok()?;
            Some((i / fold, format!("{d} {}", month_name(m, "sta.months_short"))))
        })
        .collect()
}

/// Month columns: every month up to fourteen, then every third; the year
/// on January.
fn month_axis(keys: &[String], fold: usize) -> Vec<(usize, String)> {
    let step = if keys.len() > 14 { 3 } else { 1 };
    keys.iter()
        .enumerate()
        .filter(|(i, _)| i % (step * fold) == 0)
        .filter_map(|(i, key)| {
            let m: u32 = key.get(5..7)?.parse().ok()?;
            let name = month_name(m, "sta.months_short");
            Some((i / fold, if m == 1 { format!("{name} {}", key.get(0..4).unwrap_or("")) } else { name }))
        })
        .collect()
}

/// Twenty-four values from the hourOfDay buckets.
fn hour_series(hours: &StatsTimeseries) -> Vec<u64> {
    let mut values = vec![0u64; 24];
    for b in &hours.items {
        if let Ok(h) = b.bucket.parse::<usize>()
            && h < 24
        {
            values[h] = b.plays;
        }
    }
    values
}

/// "most on Fri 11 · 41 plays, 2h 10m" from the summary's top day.
fn days_note(s: &StatsSummary) -> String {
    let Some(d) = &s.top_day else { return String::new() };
    if d.date.is_empty() {
        return String::new();
    }
    let day = day_key_label(&d.date);
    if d.listened_ms > 0 {
        t!("sta.note_days", day = day, plays = plays_words(d.plays), time = fmt_duration(d.listened_ms)).to_string()
    } else {
        t!("sta.note_days_plain", day = day, plays = plays_words(d.plays)).to_string()
    }
}

/// "most around 21:00, Sundays" from the summary's peaks.
fn hours_note(s: &StatsSummary) -> Option<String> {
    let hour = hour_label(s.peak_hour?);
    match s.peak_weekday {
        Some(w) if (0..7).contains(&w) => Some(t!("sta.note_hour_day", hour = hour, days = weekday_plural(w as u32)).to_string()),
        _ => Some(t!("sta.note_hour", hour = hour).to_string()),
    }
}

// ── Words ─────────────────────────────────────────────────────────────────

/// `9h 24m` · `2h` · `44 min` · `7 s` — for totals, the webapp's shape,
/// except under a minute: the webapp rounds to `0 minutes`, which made a
/// Top row ranked by a few seconds of listening read as nothing at all.
pub(crate) fn fmt_duration(ms: u64) -> String {
    let secs = (ms + 500) / 1000;
    if secs < 60 {
        return t!("sta.seconds", n = secs).to_string();
    }
    let mins = (ms + 30_000) / 60_000;
    if mins < 60 {
        return t!("sta.minutes", n = mins).to_string();
    }
    let (h, m) = (mins / 60, mins % 60);
    if m > 0 { t!("sta.hours_minutes", h = h, m = format!("{m:02}")).to_string() } else { t!("sta.hours", h = h).to_string() }
}

/// `4:07` · `1:02:15` — for one play or one track.
pub(crate) fn fmt_clock(ms: u64) -> String {
    let s = (ms + 500) / 1000;
    let (h, m, sec) = (s / 3600, s % 3600 / 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") }
}

fn plays_words(n: u64) -> String {
    if n == 1 { t!("sta.plays_one") } else { t!("sta.plays_n", n = fmt_count(n)) }.to_string()
}

fn tracks_words(n: u64) -> String {
    if n == 1 { t!("sta.tracks_one") } else { t!("sta.tracks_n", n = fmt_count(n)) }.to_string()
}

fn days_words(n: u64) -> String {
    if n == 1 { t!("sta.days_one") } else { t!("sta.days_n", n = fmt_count(n)) }.to_string()
}

fn via_words(peer: Option<&str>) -> String {
    let peer = peer.filter(|p| !p.is_empty()).map(|p| printable(p, 40)).unwrap_or_else(|| t!("sta.a_peer").to_string());
    t!("sta.via", peer = peer).to_string()
}

/// `Artist - Title`, or the title alone.
fn join_artist(artist: &str, title: &str) -> String {
    if artist.is_empty() { title.to_string() } else { format!("{artist} - {title}") }
}

/// A row's track — the server's metadata object in either shape, `{filepath,
/// metadata}` or the bare fields — as `(title, artist, album, year)`.
pub(crate) fn track_words(track: Option<&serde_json::Value>) -> (String, String, String, Option<i64>) {
    let Some(t) = track else { return (t!("sta.unknown_track").to_string(), String::new(), String::new(), None) };
    let m = t.get("metadata").filter(|m| m.is_object()).unwrap_or(t);
    let text = |key: &str| m.get(key).and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let filepath = t.get("filepath").or_else(|| m.get("filepath")).and_then(|v| v.as_str()).unwrap_or("");
    let title = text("title")
        .or_else(|| filepath.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_string))
        .unwrap_or_else(|| t!("sta.unknown_track").to_string());
    let year = m.get("year").and_then(|v| v.as_i64()).filter(|y| *y > 0);
    (title, text("artist").unwrap_or_default(), text("album").unwrap_or_default(), year)
}

/// The outcome words: `completed`, `skipped at 0:08`, `stopped at 5:40`,
/// `scrobbled at 0:30` for a legacy row.
fn outcome_words(item: &HistoryItem) -> String {
    let at = fmt_clock(item.played_ms);
    if item.source.as_deref() == Some("legacy") {
        return t!("sta.out_scrobbled", t = at).to_string();
    }
    match item.outcome.as_str() {
        "completed" => t!("sta.out_completed").to_string(),
        "skipped" => t!("sta.out_skipped", t = at).to_string(),
        _ => t!("sta.out_stopped", t = at).to_string(),
    }
}

/// What was listened: the whole track for a completed play, `0:08 of
/// 8:15` when the duration is known, else the time played.
fn listened_words(item: &HistoryItem) -> String {
    let legacy = item.source.as_deref() == Some("legacy");
    if item.outcome == "completed" && !legacy {
        return fmt_clock(item.duration_ms.filter(|d| *d > 0).unwrap_or(item.played_ms));
    }
    match item.duration_ms.filter(|d| *d > 0) {
        Some(total) => t!("sta.listened_of", played = fmt_clock(item.played_ms), total = fmt_clock(total)).to_string(),
        None => fmt_clock(item.played_ms),
    }
}

/// The client column: the webapp's words for its own reports, this player
/// for the terminal's, `name version` for any other — and the name alone,
/// for a column too narrow for both.
fn client_words(item: &HistoryItem) -> (String, String) {
    let c = item.client.as_deref().unwrap_or("");
    let both = |s: String| (s.clone(), s);
    if c.is_empty() {
        return both(if item.source.as_deref() == Some("legacy") { t!("sta.client_old").to_string() } else { String::new() });
    }
    if c == "legacy" {
        both(t!("sta.client_web_old").to_string())
    } else if c.starts_with("mstream-webapp") {
        both(t!("sta.client_web").to_string())
    } else if c.starts_with(crate::tui::app::stats::CLIENT_NAME) {
        // The name the player's own reporting sends (play-reporting
        // contract) — one constant, so the two can never drift apart again.
        both(t!("sta.client_this").to_string())
    } else {
        let (name, version) = c.split_once('/').unwrap_or((c, ""));
        let name = printable(name, 40);
        (if version.is_empty() { name.clone() } else { format!("{name} {}", printable(version, 20)) }, name)
    }
}

/// The `i`th name of a locale list: `Jan · Feb · …` — the one name, not
/// the list split into twelve for it.
fn nth_name(key: &str, i: usize) -> String {
    t!(key).split(" · ").nth(i).map(str::to_string).unwrap_or_default()
}

/// A month's name from one of the locale's three lists: `sta.months_short`
/// for axes and day labels, `sta.months_long` for a date (declined where
/// the language declines), `sta.months_title` for a month named on its own.
fn month_name(m: u32, list: &str) -> String {
    nth_name(list, (m as usize).saturating_sub(1))
}

fn weekday_short(w: u32) -> String {
    nth_name("sta.weekdays_short", w as usize)
}

fn weekday_plural(w: u32) -> String {
    nth_name("sta.weekdays_plural", w as usize)
}

/// The local wall clock of an instant: `(days since the epoch, year,
/// month, day, hour, minute)`.
fn local_parts(t: i64, offset: i32) -> (i64, i64, u32, u32, u32, u32) {
    let local = t + offset as i64;
    let days = local.div_euclid(86_400);
    let rem = local.rem_euclid(86_400);
    let (y, m, d) = tz::civil_from_days(days);
    (days, y, m, d, (rem / 3600) as u32, (rem % 3600 / 60) as u32)
}

/// `today` · `yesterday` · `Sep 8` · `Sep 8, 2025` — relative to `now`.
fn day_label(t: i64, now: i64, offset: i32, now_offset: i32) -> String {
    let (days, y, m, d, _, _) = local_parts(t, offset);
    let (today, ty, _, _, _, _) = local_parts(now, now_offset);
    match today - days {
        0 => t!("sta.today").to_string(),
        1 => t!("sta.yesterday").to_string(),
        _ if y == ty => t!("sta.day_label", month = month_name(m, "sta.months_short"), day = d).to_string(),
        _ => t!("sta.day_label_year", month = month_name(m, "sta.months_short"), day = d, year = y).to_string(),
    }
}

/// `21:14` on the local clock.
fn time_label(t: i64, offset: i32) -> String {
    let (_, _, _, _, hh, mm) = local_parts(t, offset);
    format!("{hh:02}:{mm:02}")
}

/// `3 March 2026`.
fn date_long(t: i64, offset: i32) -> String {
    let (_, y, m, d, _, _) = local_parts(t, offset);
    t!("sta.date_long", day = d, month = month_name(m, "sta.months_long"), year = y).to_string()
}

/// `Fri 11` for a `YYYY-MM-DD` key.
fn day_key_label(key: &str) -> String {
    let parse = |from: usize, to: usize| key.get(from..to).and_then(|s| s.parse::<i64>().ok());
    match (parse(0, 4), parse(5, 7), parse(8, 10)) {
        (Some(y), Some(m), Some(d)) => {
            let days = tz::days_from_civil(y, m as u32, d as u32);
            t!("sta.day_key", weekday = weekday_short(tz::weekday(days)), day = d).to_string()
        }
        _ => key.to_string(),
    }
}

fn hour_label(h: i32) -> String {
    format!("{:02}:00", h.clamp(0, 23))
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{OriginSlice, Origins, Sessions, StreakDays, TimeBucket, TopDay};
    use crate::input::KeyModifiers;
    use serde_json::json;

    // The fixtures are the views' tests' too (cells.rs, egui_view.rs).

    pub(super) fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    /// A page in UTC (no zone), so every label is deterministic.
    pub(super) fn page() -> Page {
        Page::new(Client::new("http://home.mstream.example:3000").expect("client"), Some("anna".into()), None)
    }

    pub(super) fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub(super) fn press(page: &mut Page, code: KeyCode) -> Option<Outcome> {
        handle_key(page, key(code))
    }

    pub(super) fn iso(t: i64) -> String {
        super::super::iso_at(t)
    }

    /// The start of today (UTC): fixture instants hang off it, so their
    /// day labels hold whatever the hour.
    pub(super) fn today0() -> i64 {
        unix_now() / 86_400 * 86_400
    }

    pub(super) fn track(artist: &str, title: &str, album: &str, year: i64) -> serde_json::Value {
        json!({"filepath": format!("music/{artist}/{title}.flac"), "metadata": {"title": title, "artist": artist, "album": album, "year": year, "duration": 495.2}})
    }

    pub(super) fn summary(plays: u64, events: u64) -> StatsSummary {
        StatsSummary {
            events,
            plays,
            unique_tracks: 188,
            unique_artists: 40,
            unique_albums: 61,
            listened_ms: 112_320_000,
            skips: 37,
            skip_rate: Some(0.08),
            completion_rate: Some(0.7),
            discoveries: 12,
            library_coverage_pct: Some(14.0),
            sessions: Sessions { count: 42, avg_ms: Some(44 * 60_000) },
            streak_days: StreakDays { current: 6, longest: 11 },
            top_day: Some(TopDay { date: "2026-09-08".into(), plays: 41, listened_ms: 7_800_000 }),
            peak_hour: Some(21),
            peak_weekday: Some(0),
            origins: Origins { local: OriginSlice { plays: 374, listened_ms: 103_200_000 }, peers: OriginSlice { plays: 38, listened_ms: 9_120_000 } },
            ..Default::default()
        }
    }

    pub(super) fn periods() -> StatsPeriods {
        let opt = |period: &str, offset: i32, label: &str, from: &str| PeriodOption { period: period.into(), offset, label: label.into(), from: from.into(), to: String::new() };
        StatsPeriods {
            earliest: Some("2026-03-03T14:00:00.000Z".into()),
            latest: Some(iso(today0())),
            periods: vec![
                opt("week", 0, "Week of 2026-09-07", "2026-09-07T00:00:00.000Z"),
                opt("week", -1, "Week of 2026-08-31", "2026-08-31T00:00:00.000Z"),
                opt("month", 0, "September 2026", "2026-09-01T00:00:00.000Z"),
                opt("month", -1, "August 2026", "2026-08-01T00:00:00.000Z"),
                opt("month", -2, "July 2026", "2026-07-01T00:00:00.000Z"),
                opt("quarter", 0, "Q3 2026", "2026-07-01T00:00:00.000Z"),
                opt("quarter", -1, "Q2 2026", "2026-04-01T00:00:00.000Z"),
                opt("half", 0, "H2 2026", "2026-07-01T00:00:00.000Z"),
                opt("year", 0, "2026", "2026-01-01T00:00:00.000Z"),
                opt("year", -1, "2025", "2025-01-01T00:00:00.000Z"),
            ],
        }
    }

    pub(super) fn bucket(key: &str, plays: u64) -> TimeBucket {
        TimeBucket { bucket: key.into(), plays, skips: 0, listened_ms: plays * 240_000 }
    }

    pub(super) fn top_item(rank: u32, plays: u64, ms: u64, track: Option<serde_json::Value>, name: Option<&str>, peer: Option<&str>) -> TopItem {
        TopItem {
            rank,
            plays,
            listened_ms: ms,
            share: 0.1,
            name: name.map(str::to_string),
            artist: None,
            tracks: name.map(|_| 14),
            last_played: Some(iso(today0() + 60)),
            origin: Some(if peer.is_some() { "peer" } else { "local" }.into()),
            peer_name: peer.map(str::to_string),
            track,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn play(id: &str, started: i64, outcome: &str, played: u64, duration: Option<u64>, counted: bool, client: Option<&str>, source: Option<&str>) -> HistoryItem {
        HistoryItem {
            id: id.into(),
            started_at: iso(started),
            played_ms: played,
            duration_ms: duration,
            outcome: outcome.into(),
            counted,
            source: source.map(str::to_string),
            client: client.map(str::to_string),
            origin: "local".into(),
            peer_name: None,
            track: Some(track("Nils Frahm", "Says", "Spaces", 2013)),
        }
    }

    pub(super) fn loaded(summary: StatsSummary, periods: Option<StatsPeriods>) -> Box<Loaded> {
        let mut series = StatsTimeseries { bucket: "day".into(), items: Vec::new() };
        series.items.push(bucket("2026-09-08", 41));
        series.items.push(bucket("2026-09-02", 12));
        let mut hours = StatsTimeseries { bucket: "hourOfDay".into(), items: Vec::new() };
        hours.items.push(bucket("21", 47));
        hours.items.push(bucket("9", 6));
        let top = StatsTop {
            entity: "tracks".into(),
            metric: "plays".into(),
            items: vec![
                top_item(1, 38, 9_060_000, Some(track("Boards of Canada", "Roygbiv", "Music Has the Right to Children", 1998)), None, None),
                top_item(2, 31, 9_120_000, Some(track("Aphex Twin", "Xtal", "Selected Ambient Works 85-92", 1992)), None, None),
                top_item(3, 19, 5_040_000, Some(track("Bassnectar", "Rewind The Track", "", 0)), None, Some("attic")),
            ],
        };
        let n = today0();
        let history = StatsHistory {
            items: vec![
                play("p1", n + 1200, "completed", 151_000, Some(151_000), true, Some("mstream-webapp/6.27.0"), Some("manual")),
                play("p2", n + 600, "skipped", 8_000, Some(495_000), false, Some("mstream-terminal-player/0.7.0"), Some("shuffle")),
                play("p3", n - 86_400 + 3600, "stopped", 340_000, Some(545_000), true, Some("mStream Mobile/2.4.1"), Some("manual")),
                play("p4", n - 86_400 * 9, "completed", 30_000, None, true, None, Some("legacy")),
            ],
            next: Some("cursor-2".into()),
        };
        let mut summary = summary;
        summary.period.from = "2026-09-01T00:00:00.000Z".into();
        summary.period.to = "2026-10-01T00:00:00.000Z".into();
        let mut previous = self::summary(346, 400);
        previous.listened_ms = 112_320_000 - 72 * 60_000;
        Box::new(Loaded { periods, summary, previous: Some(previous), series, hours, top, history })
    }

    /// A page with a September loaded (the egui page's tests draw it too).
    pub(super) fn ready() -> Page {
        let mut p = page();
        p.reload(true);
        p.queued = None;
        p.busy = None;
        let seq = p.seq;
        p.apply(Done::Loaded { seq, result: Ok(loaded(summary(388, 425), Some(periods()))) });
        p
    }

    pub(super) fn note(page: &Page) -> String {
        page.note.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
    }

    /// A log of `n` plays, seven hours apart, each its own artist and title.
    pub(super) fn deep_log(n: usize) -> Vec<HistoryItem> {
        (0..n)
            .map(|k| {
                let started = today0() - 3600 * 7 * k as i64;
                let mut item = play(&format!("q{k}"), started, "completed", 200_000, Some(200_000), true, Some("mstream-webapp/6.27.0"), None);
                item.track = Some(track(&format!("Artist {k}"), &format!("Title {k}"), "Album", 2020));
                item
            })
            .collect()
    }

    #[test]
    fn the_client_column_knows_this_players_reports_by_the_shared_name() {
        // The page once looked for `mstream-player`; the player reports as
        // `mstream-terminal-player`, so its own plays showed a clipped name.
        let _en = english();
        let own = format!("{}/0.7.0", crate::tui::app::stats::CLIENT_NAME);
        let mine = play("p9", today0(), "completed", 300_000, Some(300_000), true, Some(&own), Some("manual"));
        assert_eq!(client_words(&mine).0, "this player");
        let other = play("p8", today0(), "completed", 300_000, Some(300_000), true, Some("mstream-player/0.7.0"), Some("manual"));
        assert_eq!(client_words(&other).0, "mstream-player 0.7.0", "a name that is not the player's is spelled out");
    }

    #[test]
    fn the_words_follow_the_webapp() {
        let _en = english();
        assert_eq!(fmt_duration(0), "0 s");
        assert_eq!(fmt_duration(7_000), "7 s", "seconds under a minute, not `0 min`");
        assert_eq!(fmt_duration(59_400), "59 s");
        assert_eq!(fmt_duration(59_600), "1 min", "the minute boundary rounds as the webapp's does");
        assert_eq!(fmt_duration(58 * 60_000), "58 min");
        assert_eq!(fmt_duration(2 * 3_600_000), "2h");
        assert_eq!(fmt_duration(9 * 3_600_000 + 24 * 60_000 + 40_000), "9h 25m");
        assert_eq!(fmt_clock(247_000), "4:07");
        assert_eq!(fmt_clock(3_735_000), "1:02:15");
        assert_eq!(delta_text(388, Some(346), "August 2026").as_deref(), Some("+12% vs August 2026"));
        assert_eq!(delta_text(300, Some(346), "last week").as_deref(), Some("−13% vs last week"));
        assert_eq!(delta_text(346, Some(346), "August 2026").as_deref(), Some("same as August 2026"));
        assert_eq!(delta_text(10, Some(0), "x"), None);
        assert_eq!(delta_text(10, None, "x"), None);
        assert_eq!(delta_duration(3_900_000, Some(0), "x"), None);
        assert_eq!(delta_duration(3_900_000, Some(3_870_000), "August 2026").as_deref(), Some("same as August 2026"));
        assert_eq!(delta_duration(7_500_000, Some(3_600_000), "August 2026").as_deref(), Some("+1h 05m vs August 2026"));
        assert_eq!(day_key_label("2026-09-11"), "Fri 11");
        assert_eq!(hour_label(21), "21:00");
        let flat = json!({"title": "Xtal", "artist": "Aphex Twin"});
        assert_eq!(track_words(Some(&flat)), ("Xtal".into(), "Aphex Twin".into(), String::new(), None));
        let bare = json!({"filepath": "music/Unknown Artist/07 - Untitled.mp3", "metadata": {"title": "", "artist": null}});
        assert_eq!(track_words(Some(&bare)).0, "07 - Untitled.mp3");
        assert_eq!(track_words(None).0, "Unknown track");
        // Local clocks: an offset moves the day label across midnight.
        let t = tz::days_from_civil(2026, 9, 8) * 86_400 + 23 * 3600;
        let n = tz::days_from_civil(2026, 9, 13) * 86_400;
        assert_eq!(day_label(t, n, 0, 0), "Sep 8");
        assert_eq!(day_label(t, n, 3600 * 2, 0), "Sep 9");
        assert_eq!(day_label(t, n, 0, 0).len(), 5);
        assert_eq!(day_label(tz::days_from_civil(2025, 9, 8) * 86_400, n, 0, 0), "Sep 8, 2025");
        assert_eq!(time_label(t, -4 * 3600), "19:00");
        assert_eq!(date_long(t, 0), "8 September 2026");
        // The calendar series covers the whole range, zeros where nothing played.
        let series = StatsTimeseries { bucket: "day".into(), items: vec![bucket("2026-09-03", 5)] };
        let (keys, values, bucket_name) = calendar_series(&series, "2026-09-01T04:00:00.000Z", "2026-10-01T04:00:00.000Z", |_| -4 * 3600);
        assert_eq!((keys.len(), bucket_name), (30, "day"));
        assert_eq!(keys[0], "2026-09-01");
        assert_eq!(values[2], 5);
        assert_eq!(values.iter().sum::<u64>(), 5);
        let months = StatsTimeseries { bucket: "month".into(), items: vec![bucket("2026-03", 9)] };
        let (keys, values, _) = calendar_series(&months, "2026-01-01T05:00:00.000Z", "2027-01-01T05:00:00.000Z", |_| -5 * 3600);
        assert_eq!(keys.len(), 12);
        assert_eq!(values[2], 9);
        assert_eq!(month_axis(&keys, 1)[0].1, "Jan 2026");
        let weeks = StatsTimeseries { bucket: "week".into(), items: vec![bucket("2026-07-06", 3)] };
        let (keys, values, _) = calendar_series(&weeks, "2026-07-01T04:00:00.000Z", "2027-01-01T05:00:00.000Z", |_| -4 * 3600);
        assert_eq!(keys[0], "2026-06-29", "weeks start on their Monday");
        assert_eq!(values[1], 3);
    }

    /// The worker's answer to one op against a test server.
    fn run_op(url: &str, op: Op) -> Done {
        let (to_worker, from_worker) = spawn_worker();
        to_worker.send((Arc::new(Client::new(url).expect("client")), op)).unwrap();
        from_worker.recv_timeout(std::time::Duration::from_secs(30)).expect("an answer")
    }

    fn load_op(period: Period) -> Op {
        Op::Load { seq: 1, period, origin: Origin::All, entity: Entity::Tracks, metric: Metric::Plays, tz: "UTC".into() }
    }

    #[test]
    fn a_load_sends_its_reads_together() {
        // No read waits on another's answer, so all seven are on the wire
        // at once — the webapp's Promise.all, one round trip, not seven
        // (performance audit #83). The server holds each answer until the
        // wave is in; one at a time, each would wait out its two seconds.
        let server = super::super::waves::serve(&[7], |_| (200, "{}".into()));
        let Done::Loaded { seq: 1, result: Ok(loaded) } = run_op(&server.url, load_op(Period::this_month())) else {
            panic!("a load");
        };
        assert_eq!(server.peak(), 7, "{:?}", server.paths());
        assert!(loaded.periods.is_some() && loaded.previous.is_some());
        // All time has no period before it: six.
        let server = super::super::waves::serve(&[6], |_| (200, "{}".into()));
        let Done::Loaded { result: Ok(loaded), .. } = run_op(&server.url, load_op(Period::all_time())) else {
            panic!("a load");
        };
        assert_eq!((server.peak(), server.paths().len()), (6, 6));
        assert!(loaded.previous.is_none());
    }

    #[test]
    fn a_failed_load_names_what_it_named_when_the_reads_went_in_turn() {
        use super::super::waves::serve;
        let route = |path: &str| path.split('?').next().unwrap_or_default().to_string();
        // Everything fails: the summary's 404 — a server before the Stats
        // API — is what the page hears, as when it was asked first.
        let server = serve(&[7], move |path| match route(path).as_str() {
            "/api/v1/stats/summary" => (404, "{}".into()),
            _ => (500, r#"{"error":"down"}"#.into()),
        });
        let Done::Loaded { result, .. } = run_op(&server.url, load_op(Period::this_month())) else { panic!("a load") };
        assert!(matches!(result, Err(ApiError::NotFound(_))));
        // With the summary in hand, the series' failure comes before the
        // top's and the log's.
        let server = serve(&[7], move |path| match route(path).as_str() {
            "/api/v1/stats/summary" | "/api/v1/stats/periods" => (200, "{}".into()),
            "/api/v1/stats/timeseries" => (403, r#"{"error":"series"}"#.into()),
            _ => (500, r#"{"error":"down"}"#.into()),
        });
        let Done::Loaded { result, .. } = run_op(&server.url, load_op(Period::this_month())) else { panic!("a load") };
        assert!(matches!(result, Err(ApiError::Forbidden(_))));
        // The periods and the period before are best-effort: the load
        // lands without them.
        let server = serve(&[7], move |path| {
            let previous = route(path) == "/api/v1/stats/summary" && path.contains("offset=-1");
            if previous || route(path) == "/api/v1/stats/periods" { (500, "{}".into()) } else { (200, "{}".into()) }
        });
        let Done::Loaded { result: Ok(loaded), .. } = run_op(&server.url, load_op(Period::this_month())) else {
            panic!("a load");
        };
        assert!(loaded.periods.is_none() && loaded.previous.is_none());
    }

    #[test]
    fn a_name_is_picked_from_its_list_without_the_rest() {
        let _en = english();
        assert_eq!(month_name(1, "sta.months_short"), "Jan");
        assert_eq!(month_name(12, "sta.months_long"), "December");
        assert_eq!(month_name(0, "sta.months_short"), "Jan", "a zero month is the first, as it was");
        assert_eq!(month_name(13, "sta.months_short"), "");
        assert_eq!((weekday_short(0), weekday_plural(6)), ("Sun".to_string(), "Saturdays".to_string()));
        assert_eq!(weekday_short(7), "");
    }
}
