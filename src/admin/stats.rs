//! The stats page: `mstream-player stats` — the server's /stats page (the
//! Stats API v2, mStream 6.27) as one page in the hub's chrome, for any
//! signed-in account: stats are per account, so it lives beside the admin
//! rooms, not under them. One state line carries the period and its
//! totals; three tabs on ←→ — Overview (the webapp's six tiles, plays per
//! day, when you listen, where the tracks live), Top (tracks, artists,
//! albums or genres ranked by plays or by time) and Recent (the log, newest
//! first, paged as the cursor nears its end; `x` forgets a play behind a
//! gate). The period steps on `[` `]` and lists on `p`; the origin (all ·
//! this server · peers) cycles on `o` while the log holds peer plays.
//! Everything loads on entry and on a change — no poll, like the webapp.
//! Nothing here plays music: it is a report.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use rust_i18n::t;

use super::tz::{self, Zone};
use super::{
    Outcome, Screen, draw_bottom, draw_header_as, fmt_count, frame_ground, gate_message, host_of,
    iso_unix, printable, unix_now,
};
use crate::api::types::{HistoryItem, PeriodOption, StatsHistory, StatsPeriods, StatsSummary, StatsTimeseries, StatsTop, TopItem};
use crate::api::{ApiError, Client};
use crate::kit::theme::th;
use crate::kit::{self, Surface, bold, dim};
use crate::setup::g;

const MIN_W: u16 = 80;
const MIN_H: u16 = 24;
/// The webapp's "show all": twenty rows of the ranking.
const TOP_LIMIT: u32 = 20;
/// One page of the log; the next is asked for when the cursor is within
/// [`PAGE_AHEAD`] rows of the end.
const HISTORY_PAGE: u32 = 50;
const PAGE_AHEAD: usize = 10;
const TAB_GAP: u16 = 2;
const TEXT_MAX: usize = 120;
/// Both charts are four rows of eighth-block columns.
const CHART_ROWS: u16 = 4;
/// The share bar's cells.
const SHARE_W: usize = 10;

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

    fn note(self) -> String {
        match self {
            Tab::Overview => t!("sta.note_overview"),
            Tab::Top => t!("sta.note_top"),
            Tab::Recent => t!("sta.note_recent"),
        }
        .to_string()
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
    /// `p`: the period list, with its cursor.
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
    PeriodList,
    PeriodRow(usize),
    PeriodChoose,
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
                    let result = (|| {
                        // The periods list is best-effort: the totals are the point.
                        let periods = client.stats_periods(&tz).ok();
                        let summary = client.stats_summary(&period.period, period.offset, &tz, origin.param())?;
                        let previous = (period.period != "all")
                            .then(|| client.stats_summary(&period.period, period.offset - 1, &tz, origin.param()).ok())
                            .flatten();
                        let series = client.stats_timeseries(series_bucket(&period.period), &period.period, period.offset, &tz, origin.param())?;
                        let hours = client.stats_timeseries("hourOfDay", &period.period, period.offset, &tz, origin.param())?;
                        let top = client.stats_top(entity.param(), metric.param(), &period.period, period.offset, &tz, origin.param(), TOP_LIMIT)?;
                        let history = client.stats_history(&period.period, period.offset, &tz, origin.param(), None, HISTORY_PAGE)?;
                        Ok(Box::new(Loaded { periods, summary, previous, series, hours, top, history }))
                    })();
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
    pub period: Period,
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
    sel_anchor: Option<usize>,
    ui: Surface<Act>,
}

/// The page, loading: what `mstream-player stats` opens.
pub(super) fn start(client: Client, username: Option<String>) -> Page {
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
            sel_anchor: None,
            ui: Surface::new(),
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

    /// Ask for everything again, for the period and origin as they stand.
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

    /// A new period or origin: the old numbers go before the new arrive,
    /// so the state line never names one period over another's totals.
    fn change_range(&mut self) {
        self.data = None;
        self.history.clear();
        self.next = None;
        self.top.clear();
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

    fn act(&mut self, act: Act) -> Option<Outcome> {
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
                let options = if self.options.is_empty() { vec![Period::this_month(), Period::all_time()] } else { self.options.clone() };
                let i = options.iter().position(|o| o.is(&self.period.period, self.period.offset)).unwrap_or(0);
                let j = if d < 0 { i.saturating_sub(1) } else { (i + 1).min(options.len() - 1) };
                self.set_period(options[j].clone());
            }
            Act::PeriodList => {
                let i = self.options.iter().position(|o| o.is(&self.period.period, self.period.offset)).unwrap_or(0);
                self.modal = Modal::Period(i);
            }
            Act::PeriodRow(i) => {
                if let Modal::Period(cursor) = &mut self.modal {
                    *cursor = i.min(self.options.len().saturating_sub(1));
                }
            }
            Act::PeriodChoose => {
                if let Modal::Period(i) = self.modal.clone() {
                    self.modal = Modal::None;
                    if let Some(chosen) = self.options.get(i).cloned() {
                        self.set_period(chosen);
                    }
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
}

// ── The hub's view of the page ────────────────────────────────────────────

impl Screen for Page {
    type Act = Act;

    fn ui(&mut self) -> &mut Surface<Act> {
        &mut self.ui
    }

    fn pump(&mut self) {
        loop {
            match self.from_worker.try_recv() {
                Ok(done) => self.apply(done),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.note = Some((t!("note.worker_gone").to_string(), true));
                    break;
                }
            }
        }
        self.dispatch_queued();
    }

    fn render(&mut self, frame: &mut Frame) {
        render(frame, self)
    }

    fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        handle_key(self, key)
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        Page::act(self, act)
    }

    fn wheel(&mut self, up: bool, _at: Position) {
        match &mut self.modal {
            Modal::None => {
                self.tscroll = if up { self.tscroll.saturating_sub(1) } else { self.tscroll.saturating_add(1) };
            }
            Modal::Period(cursor) => {
                let n = self.options.len();
                if n > 0 {
                    *cursor = if up { cursor.saturating_sub(1) } else { (*cursor + 1).min(n - 1) };
                }
            }
            Modal::Forget(_) => {}
        }
    }
}

// ── Keys ──────────────────────────────────────────────────────────────────

fn handle_key(page: &mut Page, key: KeyEvent) -> Option<Outcome> {
    let code = key.code;
    match page.modal.clone() {
        Modal::Period(cursor) => {
            let n = page.options.len();
            return match code {
                KeyCode::Esc => page.act(Act::PeriodClose),
                KeyCode::Enter => page.act(Act::PeriodChoose),
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

// ── Drawing ───────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, page: &mut Page) {
    page.ui.begin_frame();
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };

    // A modal makes the page beneath INERT: the base draw sees no pointer,
    // and every rect it registered is dropped before the modal draws.
    let modal_open = !matches!(page.modal, Modal::None);
    let live_pointer = page.ui.pointer;
    if modal_open {
        page.ui.pointer = None;
    }

    let host = host_of(&page.client);
    let right = match &page.username {
        Some(user) => format!("{host} · {}", printable(user, 64)),
        None => host,
    };
    draw_header_as(frame, area, &t!("sta.title"), &right);
    // The bottom edge is the note and the tips; the page keeps the row
    // above them, which the rooms leave blank — a chart's axis lands there.
    let column = Rect { x: 2, y: 2, width: area.width.saturating_sub(4), height: area.height.saturating_sub(4) };
    draw_body(frame, page, column);

    // The cursor row's own line, when nothing louder holds the note line.
    let row_note = match (&page.note, &page.busy, page.sel) {
        (None, None, Some(i)) if !modal_open => row_words(page, i).map(|w| (w, false)),
        _ => None,
    };
    let note = page.note.clone().or(row_note);
    draw_bottom(frame, area, note.as_ref(), page.busy.as_deref(), &footer_hint(page));

    if modal_open {
        page.ui.pointer = live_pointer;
        page.ui.clear_registries();
    }
    match page.modal.clone() {
        Modal::None => {}
        Modal::Period(cursor) => draw_period_list(frame, page, area, cursor),
        Modal::Forget(row) => draw_forget(frame, page, area, row),
    }
}

/// The tips line: what the keys do here, in the order the design lists
/// them — the origin only while the control is drawn.
fn footer_hint(page: &Page) -> String {
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

/// The state line, the tabs, the tab's body.
fn draw_body(frame: &mut Frame, page: &mut Page, column: Rect) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    draw_state(frame, page, line(column.y));

    let tabs_y = column.y + 2;
    let mut x = column.x;
    for tab in Tab::ALL {
        let label = format!(" {} ", tab.name());
        let rect = Rect { x, y: tabs_y, width: label.chars().count() as u16, height: 1 };
        let hover = page.ui.pointer.is_some_and(|pt| rect.contains(pt));
        let style = if tab == page.tab {
            Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD)
        } else if hover {
            Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        frame.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        page.ui.click(rect, Act::Tab(tab));
        x = rect.right() + TAB_GAP;
    }
    let note = page.tab.note();
    if x as usize + 2 + note.chars().count() <= column.right() as usize {
        frame.render_widget(Paragraph::new(Span::styled(note, dim())).alignment(Alignment::Right), line(tabs_y));
    }

    let body = Rect { x: column.x, y: tabs_y + 2, width: column.width, height: column.bottom().saturating_sub(tabs_y + 2) };
    if body.height == 0 {
        return;
    }
    match page.tab {
        Tab::Overview => draw_overview(frame, page, body),
        Tab::Top => draw_top(frame, page, body),
        Tab::Recent => draw_recent(frame, page, body),
    }
}

/// `• This month — 388 plays · 31h 12m · 188 tracks · times in your zone`,
/// or the gold sentence for a log with nothing in it, a period with
/// nothing in it, or a server without the API.
fn draw_state(frame: &mut Frame, page: &Page, at: Rect) {
    let gold = Style::default().fg(th().gold);
    let gold_bold = gold.add_modifier(Modifier::BOLD);
    let spans = if page.no_api {
        vec![
            Span::styled(t!("sta.noapi_state").to_string(), gold_bold),
            Span::styled(t!("sta.noapi_detail").to_string(), gold),
        ]
    } else if let Some(s) = page.summary() {
        if page.no_plays_ever() {
            vec![
                Span::styled(t!("sta.empty_state").to_string(), gold_bold),
                Span::styled(t!("sta.empty_state_detail").to_string(), gold),
            ]
        } else if s.events == 0 {
            vec![
                Span::styled(format!("• {}", page.period.name), bold()),
                Span::styled(t!("sta.nothing_state").to_string(), dim()),
            ]
        } else {
            let zone = if page.zone.is_some() { t!("sta.zone_local") } else { t!("sta.zone_utc") };
            let facts = t!(
                "sta.facts",
                plays = plays_words(s.plays),
                time = fmt_duration(s.listened_ms),
                tracks = tracks_words(s.unique_tracks),
                zone = zone
            );
            vec![Span::styled(format!("• {}", page.period.name), bold()), Span::raw(facts.to_string())]
        }
    } else {
        return;
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), at);
}

/// `PERIOD  ‹ This month ›   [ ] step · p list        ORIGIN (•) all ( ) this server ( ) peers`
fn draw_controls(frame: &mut Frame, page: &mut Page, at: Rect) {
    let mut x = at.x;
    let put = |frame: &mut Frame, x: u16, text: &str, style: Style| -> u16 {
        let w = text.chars().count() as u16;
        frame.render_widget(Paragraph::new(Span::styled(text.to_string(), style)), Rect { x, y: at.y, width: w, height: 1 });
        x + w
    };
    x = put(frame, x, &t!("sta.ctl_period"), dim()) + 2;
    let prev = Rect { x, y: at.y, width: 2, height: 1 };
    let prev_hover = page.ui.pointer.is_some_and(|p| prev.contains(p));
    x = put(frame, x, g("‹ ", "< "), if prev_hover { Style::default().fg(th().bright) } else { dim() });
    page.ui.click(prev, Act::PeriodStep(-1));
    x = put(frame, x, &page.period.name, bold());
    let next = Rect { x, y: at.y, width: 2, height: 1 };
    let next_hover = page.ui.pointer.is_some_and(|p| next.contains(p));
    x = put(frame, x, g(" ›", " >"), if next_hover { Style::default().fg(th().bright) } else { dim() });
    page.ui.click(next, Act::PeriodStep(1));
    let step = t!("sta.ctl_step").to_string();
    let step_rect = Rect { x: x + 3, y: at.y, width: step.chars().count() as u16, height: 1 };
    let step_hover = page.ui.pointer.is_some_and(|p| step_rect.contains(p));
    x = put(frame, x + 3, &step, if step_hover { Style::default().fg(th().bright) } else { dim() });
    page.ui.click(step_rect, Act::PeriodList);

    if page.peer_plays() {
        let label = t!("sta.ctl_origin").to_string();
        let options: Vec<String> = Origin::ALL.iter().map(|o| o.label()).collect();
        let need = label.chars().count() + 2 + options.iter().map(|o| o.chars().count() + 7).sum::<usize>();
        let start = at.right().saturating_sub(need as u16).max(x + 4);
        if start as usize + need <= at.right() as usize {
            let x = put(frame, start, &label, dim()) + 2;
            radio_row(frame, page, Rect { x, y: at.y, width: at.right().saturating_sub(x), height: 1 }, &options, page.origin.index(), Act::Origin);
        }
    }
}

/// `(•) name   ( ) name   ( ) name` across one row; returns the x past it.
fn radio_row(frame: &mut Frame, page: &mut Page, at: Rect, options: &[String], chosen: usize, act: impl Fn(usize) -> Act) -> u16 {
    let mut x = at.x;
    for (i, name) in options.iter().enumerate() {
        let on = i == chosen;
        let w = name.chars().count() as u16 + 4;
        if x + w > at.right() {
            break;
        }
        let rect = Rect { x, y: at.y, width: w, height: 1 };
        let hover = page.ui.pointer.is_some_and(|p| rect.contains(p));
        let glyph_style = if on { Style::default().fg(th().accent) } else { dim() };
        let name_style = if hover { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if on { bold() } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if on { g("(•)", "(*)") } else { "( )" }, glyph_style),
                Span::raw(" "),
                Span::styled(name.clone(), name_style),
            ])),
            rect,
        );
        page.ui.click(rect, act(i));
        x += w + 3;
    }
    x
}

// ── Overview ──────────────────────────────────────────────────────────────

fn draw_overview(frame: &mut Frame, page: &mut Page, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let mut y = body.y;
    if page.no_api || page.data.is_none() {
        return;
    }
    if page.no_plays_ever() {
        // The webapp's empty state, in its words.
        for (i, key) in ["sta.empty_1", "sta.empty_2"].iter().enumerate() {
            if y + 1 + (i as u16) < body.bottom() {
                frame.render_widget(Paragraph::new(t!(*key).to_string()), line(y + 1 + i as u16));
            }
        }
        if y + 4 < body.bottom() {
            let zone = if page.zone.is_some() { t!("sta.zone_local") } else { t!("sta.zone_utc") };
            frame.render_widget(Paragraph::new(Span::styled(t!("sta.empty_prov", zone = zone).to_string(), dim())), line(y + 4));
        }
        return;
    }
    draw_controls(frame, page, line(y));
    y += 2;

    let Some(data) = page.data.as_ref() else { return };
    if data.summary.events == 0 {
        if y + 1 < body.bottom() {
            frame.render_widget(Paragraph::new(Span::styled(t!("sta.nothing_title", period = page.period.name.clone()).to_string(), bold())), line(y + 1));
        }
        if y + 3 < body.bottom() {
            let begins = data
                .periods
                .as_ref()
                .and_then(|p| p.earliest.as_deref())
                .and_then(iso_unix)
                .map(|t| date_long(t, page.offset_at(t)));
            let hint = match begins {
                Some(date) => t!("sta.nothing_hint", date = date).to_string(),
                None => t!("sta.nothing_hint_plain").to_string(),
            };
            frame.render_widget(Paragraph::new(Span::styled(hint, dim())), line(y + 3));
        }
        return;
    }

    // The six tiles, two rows of three: value bold, label and detail dim.
    let versus = versus_label(&page.period, &page.options);
    let tiles = tiles(&data.summary, data.previous.as_ref(), versus.as_deref());
    let tile_w = body.width / 3;
    for (i, (value, label, detail)) in tiles.iter().enumerate() {
        let x = body.x + (i as u16 % 3) * tile_w;
        let top = y + (i as u16 / 3) * 4;
        if top + 3 > body.bottom() {
            break;
        }
        let cell = |dy: u16| Rect { x, y: top + dy, width: tile_w.saturating_sub(1), height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(clip(value, tile_w - 1), bold())), cell(0));
        frame.render_widget(Paragraph::new(Span::styled(clip(label, tile_w - 1), dim())), cell(1));
        frame.render_widget(Paragraph::new(Span::styled(clip(detail, tile_w - 1), dim())), cell(2));
    }
    y += 8;

    // Plays per day (or week, or month): one eighth-block column per bucket.
    if y + CHART_ROWS + 2 > body.bottom() {
        return;
    }
    let from = match (page.period.period.as_str(), data.periods.as_ref().and_then(|p| p.earliest.as_deref())) {
        // All time starts at the retention floor, years before the first
        // play; the chart starts where the log does.
        ("all", Some(earliest)) => earliest.to_string(),
        _ => data.summary.period.from.clone(),
    };
    let (keys, values, bucket) = calendar_series(&data.series, &from, &data.summary.period.to, |t| page.offset_at(t));
    let width = body.width as usize;
    let fold = values.len().div_ceil(width.max(1)).max(1);
    let values: Vec<u64> = values.chunks(fold).map(|c| c.iter().sum()).collect();
    let title = match (bucket, fold) {
        ("day", 1) => t!("sta.chart_days").to_string(),
        ("day", n) => t!("sta.chart_days_folded", n = n).to_string(),
        ("week", _) => t!("sta.chart_weeks").to_string(),
        _ => t!("sta.chart_months").to_string(),
    };
    frame.render_widget(Paragraph::new(Span::styled(title.clone(), dim())), line(y));
    let days_note = days_note(&data.summary);
    if title.chars().count() + 2 + days_note.chars().count() <= width {
        frame.render_widget(Paragraph::new(Span::styled(days_note, dim())).alignment(Alignment::Right), line(y));
    }
    let cell_w = ((width / values.len().max(1)) as u16).clamp(1, 3);
    columns(frame, Rect { x: body.x, y: y + 1, width: body.width, height: CHART_ROWS }, &values, cell_w);
    let labels: Vec<(usize, String)> = match bucket {
        "day" => day_axis(&keys, fold),
        "week" => week_axis(&keys, fold),
        _ => month_axis(&keys, fold),
    };
    axis(frame, line(y + 1 + CHART_ROWS), &labels, cell_w);
    y += CHART_ROWS + 2;

    // When you listen — the 24-hour profile — and, beside it, where the
    // tracks live, while the log holds peer plays.
    if y + CHART_ROWS + 2 > body.bottom() {
        return;
    }
    let hours = hour_series(&data.hours);
    let hours_w: u16 = 48;
    let right_w = body.width.saturating_sub(hours_w + 3);
    let left_w = if page.peer_plays() && right_w >= 40 { hours_w } else { body.width };
    let title = t!("sta.chart_hours").to_string();
    frame.render_widget(Paragraph::new(Span::styled(title.clone(), dim())), line(y));
    if let Some(note) = hours_note(&data.summary)
        && title.chars().count() + 2 + note.chars().count() <= left_w as usize
    {
        frame.render_widget(
            Paragraph::new(Span::styled(note, dim())),
            Rect { x: body.x + title.chars().count() as u16 + 2, y, width: left_w - title.chars().count() as u16 - 2, height: 1 },
        );
    }
    columns(frame, Rect { x: body.x, y: y + 1, width: hours_w.min(body.width), height: CHART_ROWS }, &hours, 2);
    let hour_labels: Vec<(usize, String)> = [0, 6, 12, 18, 23].iter().map(|h| (*h, h.to_string())).collect();
    axis(frame, Rect { x: body.x, y: y + 1 + CHART_ROWS, width: hours_w.min(body.width), height: 1 }, &hour_labels, 2);

    if left_w < body.width {
        let x = body.x + left_w + 3;
        let w = body.right().saturating_sub(x);
        let row = |dy: u16| Rect { x, y: y + dy, width: w, height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(t!("sta.origins_title").to_string(), dim())), row(0));
        let o = &data.summary.origins;
        let total = (o.local.plays + o.peers.plays).max(1);
        let local_share = ((o.local.plays * SHARE_W as u64 + total / 2) / total) as usize;
        let name_w = [t!("sta.origin_local_row"), t!("sta.origin_peers_row")].iter().map(|s| s.chars().count()).max().unwrap_or(0) + 1;
        let split = |frame: &mut Frame, at: Rect, name: String, filled: usize, plays: u64, ms: u64| {
            let spans = vec![
                Span::raw(format!("{name:<name_w$}")),
                Span::styled(g("▰", "■").repeat(filled), Style::default().fg(th().accent)),
                Span::styled(g("▱", "·").repeat(SHARE_W - filled), dim()),
                Span::raw(format!("  {}", t!("sta.plays_time", plays = plays_words(plays), time = fmt_duration(ms)))),
            ];
            frame.render_widget(Paragraph::new(Line::from(spans)), at);
        };
        split(frame, row(1), t!("sta.origin_local_row").to_string(), local_share, o.local.plays, o.local.listened_ms);
        frame.render_widget(Paragraph::new(Span::styled(t!("sta.origin_local_note").to_string(), dim())), Rect { x: x + name_w as u16, ..row(2) });
        split(frame, row(3), t!("sta.origin_peers_row").to_string(), SHARE_W - local_share, o.peers.plays, o.peers.listened_ms);
        frame.render_widget(Paragraph::new(Span::styled(t!("sta.origin_peers_note_1").to_string(), dim())), Rect { x: x + name_w as u16, ..row(4) });
        if y + 5 < body.bottom() {
            frame.render_widget(Paragraph::new(Span::styled(t!("sta.origin_peers_note_2").to_string(), dim())), Rect { x: x + name_w as u16, ..row(5) });
        }
    }
}

/// A column chart in eighth blocks: `values` left to right, each `cell_w`
/// cells wide (the last cell a gap when there is room), `at.height` rows
/// tall, scaled to the tallest.
fn columns(frame: &mut Frame, at: Rect, values: &[u64], cell_w: u16) {
    let rows = at.height as usize;
    let vmax = values.iter().copied().max().unwrap_or(0).max(1);
    let glyphs = crate::tui::ui::glyphs();
    let bar_w = if cell_w > 1 { cell_w as usize - 1 } else { 1 };
    let per_row = (at.width / cell_w.max(1)) as usize;
    for row in 0..rows {
        let mut spans = Vec::with_capacity(values.len());
        for v in values.iter().take(per_row) {
            let eighths = ((*v as f64 / vmax as f64) * (rows * 8) as f64).round() as i64;
            let level = (eighths - ((rows - 1 - row) * 8) as i64).clamp(0, 8) as usize;
            spans.push(Span::styled(glyphs.eighths[level].repeat(bar_w), Style::default().fg(th().accent)));
            if cell_w > 1 {
                spans.push(Span::raw(" "));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), Rect { x: at.x, y: at.y + row as u16, width: at.width, height: 1 });
    }
}

/// The labels under a chart, at their columns, none overlapping the last.
fn axis(frame: &mut Frame, at: Rect, labels: &[(usize, String)], cell_w: u16) {
    let mut end = at.x;
    for (i, text) in labels {
        let x = at.x + *i as u16 * cell_w;
        let w = text.chars().count() as u16;
        if x < end || x + w > at.right() {
            continue;
        }
        frame.render_widget(Paragraph::new(Span::styled(text.clone(), dim())), Rect { x, y: at.y, width: w, height: 1 });
        end = x + w + 1;
    }
}

// ── Top ───────────────────────────────────────────────────────────────────

fn draw_top(frame: &mut Frame, page: &mut Page, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let mut y = body.y;
    if page.no_api || page.data.is_none() {
        return;
    }
    // SHOW and RANK BY, both radio rows.
    let mut x = body.x;
    let put = |frame: &mut Frame, x: u16, text: &str| -> u16 {
        let w = text.chars().count() as u16;
        frame.render_widget(Paragraph::new(Span::styled(text.to_string(), dim())), Rect { x, y, width: w, height: 1 });
        x + w
    };
    x = put(frame, x, &t!("sta.ctl_show")) + 2;
    let entities: Vec<String> = Entity::ALL.iter().map(|e| e.label()).collect();
    x = radio_row(frame, page, Rect { x, y, width: body.right().saturating_sub(x), height: 1 }, &entities, page.entity.index(), Act::Entity);
    let rank = t!("sta.ctl_rank").to_string();
    let metrics = [Metric::Plays.label(), Metric::Time.label()];
    let need = rank.chars().count() + 2 + metrics.iter().map(|m| m.chars().count() + 7).sum::<usize>();
    let start = body.right().saturating_sub(need as u16).max(x + 2);
    if start as usize + need <= body.right() as usize {
        let x = put(frame, start, &rank) + 2;
        radio_row(frame, page, Rect { x, y, width: body.right().saturating_sub(x), height: 1 }, &metrics, if page.metric == Metric::Plays { 0 } else { 1 }, Act::Metric);
    }
    y += 2;

    // The table: rank, name, [tracks], share, plays, time, [last played].
    let tracks = page.entity == Entity::Tracks;
    let right = body.right();
    let (last_x, last_w) = if tracks { (right.saturating_sub(12), 12) } else { (right, 0) };
    let time_w = 7;
    let time_x = if tracks { last_x.saturating_sub(2 + time_w) } else { right.saturating_sub(time_w) };
    let plays_w = 6;
    let plays_x = time_x.saturating_sub(2 + plays_w);
    let share_x = plays_x.saturating_sub(2 + SHARE_W as u16);
    let (tracks_x, tracks_w) = if tracks { (share_x, 0) } else { (share_x.saturating_sub(12), 10) };
    let name_x = body.x + 4;
    let name_w = tracks_x.saturating_sub(2).saturating_sub(name_x);
    let mut cols: Vec<(u16, u16, String, bool)> = vec![(body.x, 2, t!("sta.col_rank").to_string(), false), (name_x, name_w, page.entity.header(), false)];
    if !tracks {
        cols.push((tracks_x, tracks_w, t!("sta.col_tracks").to_string(), false));
    }
    cols.push((share_x, SHARE_W as u16, t!("sta.col_share").to_string(), false));
    cols.push((plays_x, plays_w, t!("sta.col_plays").to_string(), true));
    cols.push((time_x, time_w, t!("sta.col_time").to_string(), false));
    if tracks {
        cols.push((last_x, last_w, t!("sta.col_last").to_string(), false));
    }
    y = table_head(frame, line(y), &cols);
    if page.top.is_empty() {
        if y < body.bottom() {
            frame.render_widget(Paragraph::new(Span::styled(t!("sta.nothing_here").to_string(), dim())), line(y));
        }
        return;
    }
    let rows_rect = Rect { x: body.x, y, width: body.width, height: body.bottom().saturating_sub(y) };
    if rows_rect.height == 0 {
        return;
    }
    let metric = page.metric;
    let top_value = page.top.iter().map(|i| metric.of(i)).max().unwrap_or(0).max(1);
    let now = unix_now();
    let items: Vec<(String, Option<String>, String, u64, String, String)> = page
        .top
        .iter()
        .map(|item| {
            let (name, via) = if tracks {
                let (title, artist, _, _) = track_words(item.track.as_ref());
                (join_artist(&artist, &title), (item.origin.as_deref() == Some("peer")).then(|| via_words(item.peer_name.as_deref())))
            } else {
                (item.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| t!("sta.unknown").to_string()), None)
            };
            let extra = if tracks {
                item.last_played.as_deref().and_then(iso_unix).map(|t| day_label(t, now, page.offset_at(t), page.offset_at(now))).unwrap_or_default()
            } else {
                tracks_words(item.tracks.unwrap_or(0))
            };
            let filled = ((metric.of(item) * SHARE_W as u64 + top_value / 2) / top_value) as usize;
            (printable(&name, TEXT_MAX), via, extra, item.plays, fmt_duration(item.listened_ms), g("▰", "■").repeat(filled) + &g("▱", "·").repeat(SHARE_W - filled))
        })
        .collect();
    table_rows(frame, page, rows_rect, items.len(), |frame, page, i, rect, selected, hovered| {
        let (name, via, extra, plays, time, share) = &items[i];
        let _ = page;
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        frame.render_widget(Paragraph::new(Span::styled(format!("{:>2}", i + 1), cell_style(selected, hovered, dim()))), cell(body.x, 2));
        let via_w = via.as_ref().map_or(0, |v| v.chars().count() as u16);
        let title_w = name_w.saturating_sub(via_w);
        let title = clip(name, title_w);
        let title_len = title.chars().count() as u16;
        frame.render_widget(
            Paragraph::new(Span::styled(title, if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(name_x, title_w),
        );
        if let Some(via) = via {
            frame.render_widget(Paragraph::new(Span::styled(via.clone(), cell_style(selected, hovered, dim()))), cell(name_x + title_len, via_w));
        }
        if !tracks {
            frame.render_widget(Paragraph::new(Span::styled(clip(extra, tracks_w), cell_style(selected, hovered, dim()))), cell(tracks_x, tracks_w));
        }
        frame.render_widget(Paragraph::new(Span::styled(share.clone(), cell_style(selected, hovered, Style::default().fg(th().accent)))), cell(share_x, SHARE_W as u16));
        frame.render_widget(Paragraph::new(Span::styled(fmt_count(*plays), base)).alignment(Alignment::Right), cell(plays_x, plays_w));
        frame.render_widget(Paragraph::new(Span::styled(clip(time, time_w), cell_style(selected, hovered, dim()))), cell(time_x, time_w));
        if tracks {
            frame.render_widget(Paragraph::new(Span::styled(clip(extra, last_w), cell_style(selected, hovered, dim()))), cell(last_x, last_w));
        }
    });
}

// ── Recent ────────────────────────────────────────────────────────────────

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
    RecentRow {
        time: t.map(|t| time_label(t, page.offset_at(t))).unwrap_or_default(),
        day: t.map(|t| day_label(t, now, page.offset_at(t), page.offset_at(now))).unwrap_or_default(),
        track: printable(&join_artist(&artist, &title), TEXT_MAX),
        via: (item.origin == "peer").then(|| via_words(item.peer_name.as_deref())),
        outcome: outcome_words(item),
        completed: item.outcome == "completed" && !legacy,
        listened: listened_words(item),
        counted: item.counted,
        client: client_words(item).0,
        client_short: client_words(item).1,
    }
}

fn draw_recent(frame: &mut Frame, page: &mut Page, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    if page.no_api || page.data.is_none() {
        return;
    }
    let right = body.right();
    let client_w = 14;
    let client_x = right.saturating_sub(client_w);
    let listened_w = 12;
    let listened_x = client_x.saturating_sub(2 + listened_w);
    let outcome_w = 17;
    let outcome_x = listened_x.saturating_sub(2 + outcome_w);
    let time_x = body.x;
    let day_x = body.x + 7;
    let day_w = 12;
    let track_x = day_x + day_w + 2;
    let track_w = outcome_x.saturating_sub(2).saturating_sub(track_x);
    let cols = [
        (time_x, 5, t!("sta.col_when").to_string(), false),
        (day_x, day_w, t!("sta.col_day").to_string(), false),
        (track_x, track_w, t!("sta.col_track").to_string(), false),
        (outcome_x, outcome_w, t!("sta.col_outcome").to_string(), false),
        (listened_x, listened_w, t!("sta.col_listened").to_string(), false),
        (client_x, client_w, t!("sta.col_client").to_string(), false),
    ];
    let y = table_head(frame, line(body.y), &cols);
    if page.history.is_empty() {
        if y < body.bottom() {
            frame.render_widget(Paragraph::new(Span::styled(t!("sta.no_rows_recent").to_string(), dim())), line(y));
        }
        return;
    }
    let rows_rect = Rect { x: body.x, y, width: body.width, height: body.bottom().saturating_sub(y) };
    if rows_rect.height == 0 {
        return;
    }
    let now = unix_now();
    let rows: Vec<RecentRow> = page.history.iter().map(|item| recent_row(page, item, now)).collect();
    table_rows(frame, page, rows_rect, rows.len(), |frame, _page, i, rect, selected, hovered| {
        let r = &rows[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        let dimmed = cell_style(selected, hovered, dim());
        frame.render_widget(Paragraph::new(Span::styled(r.time.clone(), base)), cell(time_x, 5));
        frame.render_widget(Paragraph::new(Span::styled(clip(&r.day, day_w), dimmed)), cell(day_x, day_w));
        let via_w = r.via.as_ref().map_or(0, |v| v.chars().count() as u16);
        let title_w = track_w.saturating_sub(via_w);
        let title = clip(&r.track, title_w);
        let title_len = title.chars().count() as u16;
        frame.render_widget(
            Paragraph::new(Span::styled(title, if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(track_x, title_w),
        );
        if let Some(via) = &r.via {
            frame.render_widget(Paragraph::new(Span::styled(via.clone(), dimmed)), cell(track_x + title_len, via_w));
        }
        let outcome_style = if r.completed { Style::default().fg(th().ok) } else { dim() };
        frame.render_widget(Paragraph::new(Span::styled(clip(&r.outcome, outcome_w), cell_style(selected, hovered, outcome_style))), cell(outcome_x, outcome_w));
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&r.listened, listened_w), cell_style(selected, hovered, if r.counted { Style::default() } else { dim() }))),
            cell(listened_x, listened_w),
        );
        let client = if !r.counted {
            t!("sta.not_counted").to_string()
        } else if r.client.chars().count() as u16 <= client_w {
            r.client.clone()
        } else {
            r.client_short.clone()
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&client, client_w), dimmed)), cell(client_x, client_w));
    });
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

// ── Modals ────────────────────────────────────────────────────────────────

/// `p`: the period list — every period the log has data in, then All time.
fn draw_period_list(frame: &mut Frame, page: &mut Page, area: Rect, cursor: usize) {
    let options = if page.options.is_empty() { vec![Period::this_month(), Period::all_time()] } else { page.options.clone() };
    let foot = page
        .data
        .as_ref()
        .and_then(|d| d.periods.as_ref())
        .and_then(|p| p.earliest.as_deref())
        .and_then(iso_unix)
        .map(|t| t!("sta.period_foot", date = date_long(t, page.offset_at(t))).to_string());
    let rows_max = area.height.saturating_sub(8).max(3) as usize;
    let visible = options.len().min(rows_max);
    let height = 5 + visible as u16 + foot.is_some() as u16;
    let inner = kit::modal_frame(frame, area, 46, height, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("sta.period_title").to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 },
    );
    kit::modal_close_plain(frame, &mut page.ui, inner, Act::PeriodClose);
    let (first, visible) = kit::table_view(options.len(), Some(cursor), cursor.saturating_sub(visible.saturating_sub(1)), visible);
    for (row, i) in (first..first + visible).enumerate() {
        let rect = Rect { x: inner.x + 1, y: inner.y + 2 + row as u16, width: inner.width.saturating_sub(2), height: 1 };
        let on = i == cursor;
        let hover = !on && page.ui.pointer.is_some_and(|p| rect.contains(p));
        let base = if on {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hover {
            Style::default().fg(th().bright)
        } else {
            Style::default()
        };
        if on {
            frame.render_widget(Paragraph::new(Span::styled(" ".repeat(rect.width as usize), base)), rect);
        }
        let option = &options[i];
        let mut spans = vec![Span::styled(format!(" {}", option.name), if on || hover { base.add_modifier(Modifier::BOLD) } else { base })];
        if let Some(detail) = &option.detail {
            spans.push(Span::styled(format!("  {detail}"), if on { base } else { dim() }));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), rect);
        page.ui.click(rect, Act::PeriodRow(i));
    }
    if let Some(foot) = foot {
        frame.render_widget(
            Paragraph::new(Span::styled(foot, dim())),
            Rect { x: inner.x + 2, y: inner.y + 3 + visible as u16, width: inner.width.saturating_sub(3), height: 1 },
        );
    }
}

/// `x`: the forget gate, gold because it is destructive — the play in
/// words, the one fact, Keep (Enter, Esc) and Forget (y).
fn draw_forget(frame: &mut Frame, page: &mut Page, area: Rect, row: usize) {
    let words = page.history.get(row).map(|item| {
        let r = recent_row(page, item, unix_now());
        let mut w = t!("sta.row_recent", track = r.track, day = r.day, time = r.time, outcome = r.outcome).to_string();
        if !r.completed && item.duration_ms.is_some_and(|d| d > 0) {
            w.push_str(&t!("sta.row_recent_of", total = fmt_clock(item.duration_ms.unwrap_or(0))));
        }
        w
    });
    let inner = kit::modal_frame(frame, area, 70, 10, th().gold);
    let gold = Style::default().fg(th().gold);
    let lines = vec![
        Line::from(Span::styled(t!("sta.forget_title").to_string(), gold.add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::raw(words.unwrap_or_default())),
        Line::from(""),
        Line::from(Span::styled(t!("sta.forget_fact").to_string(), gold)),
    ];
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(2) },
    );
    let y = inner.bottom().saturating_sub(1);
    let keep = t!("sta.forget_keep").to_string();
    let go = t!("sta.forget_go").to_string();
    let go_w = go.chars().count() as u16 + 4;
    let keep_w = keep.chars().count() as u16 + 4;
    let keep_x = inner.right().saturating_sub(go_w + 2 + keep_w);
    let keep_rect = kit::button(frame, &mut page.ui, Rect { x: keep_x, y, width: inner.width, height: 1 }, &keep, true, Act::ForgetCancel);
    kit::button(frame, &mut page.ui, Rect { x: keep_rect.right() + 2, y, width: inner.width, height: 1 }, &go, false, Act::ForgetConfirm);
}

// ── Tables ────────────────────────────────────────────────────────────────

/// The header words at their columns, the full-width rule beneath.
/// Returns the first row's y.
fn table_head(frame: &mut Frame, at: Rect, cols: &[(u16, u16, String, bool)]) -> u16 {
    for (x, w, word, right) in cols {
        let mut p = Paragraph::new(Span::styled(clip(word, *w), dim()));
        if *right {
            p = p.alignment(Alignment::Right);
        }
        frame.render_widget(p, Rect { x: *x, y: at.y, width: *w, height: 1 });
    }
    frame.render_widget(Paragraph::new(Span::styled("─".repeat(at.width as usize), dim())), Rect { y: at.y + 1, ..at });
    at.y + 2
}

fn table_rows(
    frame: &mut Frame,
    page: &mut Page,
    at: Rect,
    len: usize,
    mut draw_row: impl FnMut(&mut Frame, &mut Page, usize, Rect, bool, bool),
) {
    let avail = at.height as usize;
    let sel_moved = page.sel != page.sel_anchor;
    page.sel_anchor = page.sel;
    let reveal = if sel_moved { page.sel } else { None };
    let (first, visible) = kit::table_view(len, reveal, page.tscroll, avail);
    page.tscroll = first;
    for (row, i) in (first..first + visible).enumerate() {
        let rect = Rect { x: at.x, y: at.y + row as u16, width: at.width, height: 1 };
        let selected = page.sel == Some(i);
        let hovered = !selected && page.ui.pointer.is_some_and(|p| rect.contains(p));
        if selected {
            frame.render_widget(
                Paragraph::new(Span::styled(" ".repeat(at.width as usize), Style::default().fg(th().on_accent).bg(th().accent))),
                rect,
            );
        }
        draw_row(frame, page, i, rect, selected, hovered);
        page.ui.click(rect, Act::Select(i));
    }
    let bar = Rect { x: at.x + at.width, y: at.y, width: 1, height: visible as u16 };
    kit::scroll_list(frame, &mut page.ui, bar, len, visible, first, Act::TableScroll(-1), Act::TableScroll(1), Act::TableScrollTo);
}

fn cell_style(selected: bool, hovered: bool, own: Style) -> Style {
    if selected {
        Style::default().fg(th().on_accent).bg(th().accent)
    } else if hovered {
        Style::default().fg(th().bright)
    } else {
        own
    }
}

fn clip(s: &str, w: u16) -> String {
    let w = w as usize;
    if s.chars().count() <= w {
        s.to_string()
    } else if w == 0 {
        String::new()
    } else {
        let mut out: String = s.chars().take(w - 1).collect();
        out.push('…');
        out
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

/// `9h 24m` · `2h` · `44 min` · `0 min` — for totals, the webapp's shape.
pub(crate) fn fmt_duration(ms: u64) -> String {
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
    } else if c.starts_with("mstream-player") {
        both(t!("sta.client_this").to_string())
    } else {
        let (name, version) = c.split_once('/').unwrap_or((c, ""));
        let name = printable(name, 40);
        (if version.is_empty() { name.clone() } else { format!("{name} {}", printable(version, 20)) }, name)
    }
}

/// Names from a locale list: `Jan · Feb · …`.
fn names(key: &str) -> Vec<String> {
    t!(key).split(" · ").map(str::to_string).collect()
}

/// A month's name from one of the locale's three lists: `sta.months_short`
/// for axes and day labels, `sta.months_long` for a date (declined where
/// the language declines), `sta.months_title` for a month named on its own.
fn month_name(m: u32, list: &str) -> String {
    names(list).get((m as usize).saturating_sub(1)).cloned().unwrap_or_default()
}

fn weekday_short(w: u32) -> String {
    names("sta.weekdays_short").get(w as usize).cloned().unwrap_or_default()
}

fn weekday_plural(w: u32) -> String {
    names("sta.weekdays_plural").get(w as usize).cloned().unwrap_or_default()
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use serde_json::json;

    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    /// A page in UTC (no zone), so every label is deterministic.
    fn page() -> Page {
        Page::new(Client::new("http://home.mstream.example:3000").expect("client"), Some("anna".into()), None)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(page: &mut Page, code: KeyCode) -> Option<Outcome> {
        handle_key(page, key(code))
    }

    fn iso(t: i64) -> String {
        super::super::iso_at(t)
    }

    /// The start of today (UTC): fixture instants hang off it, so their
    /// day labels hold whatever the hour.
    fn today0() -> i64 {
        unix_now() / 86_400 * 86_400
    }

    fn track(artist: &str, title: &str, album: &str, year: i64) -> serde_json::Value {
        json!({"filepath": format!("music/{artist}/{title}.flac"), "metadata": {"title": title, "artist": artist, "album": album, "year": year, "duration": 495.2}})
    }

    fn summary(plays: u64, events: u64) -> StatsSummary {
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

    fn periods() -> StatsPeriods {
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

    fn bucket(key: &str, plays: u64) -> TimeBucket {
        TimeBucket { bucket: key.into(), plays, skips: 0, listened_ms: plays * 240_000 }
    }

    fn top_item(rank: u32, plays: u64, ms: u64, track: Option<serde_json::Value>, name: Option<&str>, peer: Option<&str>) -> TopItem {
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
    fn play(id: &str, started: i64, outcome: &str, played: u64, duration: Option<u64>, counted: bool, client: Option<&str>, source: Option<&str>) -> HistoryItem {
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

    fn loaded(summary: StatsSummary, periods: Option<StatsPeriods>) -> Box<Loaded> {
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
                play("p2", n + 600, "skipped", 8_000, Some(495_000), false, Some("mstream-player/0.7.0"), Some("shuffle")),
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

    fn ready() -> Page {
        let mut p = page();
        p.reload(true);
        p.queued = None;
        p.busy = None;
        let seq = p.seq;
        p.apply(Done::Loaded { seq, result: Ok(loaded(summary(388, 425), Some(periods()))) });
        p
    }

    fn note(page: &Page) -> String {
        page.note.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
    }

    fn draw(page: &mut Page) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, page)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn row(frame: &str, needle: &str) -> String {
        frame.lines().find(|l| l.contains(needle)).map(|l| l.to_string()).unwrap_or_else(|| panic!("no row with {needle:?}:\n{frame}"))
    }

    #[test]
    fn boots_loading_then_draws_the_overview_with_its_tiles_charts_and_state_line() {
        let _en = english();
        let mut p = start(Client::new("http://home.mstream.example:3000").expect("client"), Some("anna".into()));
        assert!(matches!(p.queued, Some(Op::Load { .. })), "the first load is queued");
        assert_eq!(p.busy.as_deref(), Some("loading your listening…"));
        p.zone = None;
        p.queued = None;
        p.busy = None;
        let frame = draw(&mut p);
        assert!(frame.contains("Stats") && frame.contains("home.mstream.example · anna"), "{frame}");
        assert!(!frame.contains("• This month"), "no state line before the load lands:\n{frame}");

        let seq = p.seq;
        p.apply(Done::Loaded { seq, result: Ok(loaded(summary(388, 425), Some(periods()))) });
        let frame = draw(&mut p);
        assert!(frame.contains("• This month — 388 plays · 31h 12m · 188 tracks · times in UTC"), "{frame}");
        assert!(frame.contains(" Overview ") && frame.contains(" Top ") && frame.contains(" Recent "), "{frame}");
        assert!(frame.contains("PERIOD  ‹ This month ›   [ ] step · p list"), "{frame}");
        assert!(frame.contains("ORIGIN  (•) all   ( ) this server   ( ) peers"), "the log holds peer plays:\n{frame}");
        // The six tiles and their details, against August by its own name.
        let tiles = [("388", "plays", "+12% vs August 2026"), ("31h 12m", "listening time", "+1h 12m vs August 2026"), ("188", "tracks", "14% of the library"),
            ("37", "skips", "8% of starts"), ("6 days", "streak", "longest 11"), ("42", "sessions", "about 44 min each")];
        for (value, label, detail) in tiles {
            assert!(frame.contains(value) && frame.contains(label) && frame.contains(detail), "tile {label}:\n{frame}");
        }
        assert!(row(&frame, "PLAYS PER DAY").contains("most on Tue 8 · 41 plays, 2h 10m"), "{frame}");
        assert!(frame.contains("WHEN YOU LISTEN  most around 21:00, Sundays"), "{frame}");
        assert!(frame.contains("WHERE THE TRACKS LIVE"), "{frame}");
        assert!(row(&frame, "this server   ▰").contains("▰▰▰▰▰▰▰▰▰▱  374 plays · 28h 40m"), "{frame}");
        assert!(row(&frame, "peers’ tracks").contains("▰▱▱▱▱▱▱▱▱▱  38 plays · 2h 32m"), "{frame}");
        assert!(frame.contains("█"), "the tallest day is a full column:\n{frame}");
        assert!(frame.contains("←→ tab · [ ] period · p pick a period · o origin · q quit"), "{frame}");
        // Every period the fixture names, in the webapp's order, then All time.
        let names: Vec<&str> = p.options.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["This week", "Last week", "This month", "Last month", "July 2026", "This quarter", "Last quarter", "This half-year", "This year", "Last year", "All time"]);
        assert_eq!(p.options[0].detail.as_deref(), Some("Week of 2026-09-07"));
        assert_eq!(p.options[2].detail.as_deref(), Some("September 2026"));
    }

    #[test]
    fn the_period_steps_on_brackets_lists_on_p_and_a_new_choice_reloads() {
        let _en = english();
        let mut p = ready();
        let seq = p.seq;
        press(&mut p, KeyCode::Char('['));
        assert_eq!(p.period.name, "Last week");
        assert!(p.data.is_none(), "the old numbers go before the new arrive");
        assert!(matches!(&p.queued, Some(Op::Load { seq: s, period, .. }) if *s > seq && period.period == "week" && period.offset == -1), "{:?}", p.queued);
        assert_eq!(p.busy.as_deref(), Some("loading your listening…"));
        let frame = draw(&mut p);
        assert!(frame.contains("loading your listening…"), "{frame}");

        // An answer to the older question is dropped.
        p.apply(Done::Loaded { seq, result: Ok(loaded(summary(1, 1), Some(periods()))) });
        assert!(p.data.is_none());
        let live = p.seq;
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(summary(120, 130), Some(periods()))) });
        assert!(draw(&mut p).contains("• Last week — 120 plays"));

        press(&mut p, KeyCode::Char('p'));
        assert_eq!(p.modal, Modal::Period(1));
        let frame = draw(&mut p);
        assert!(frame.contains("Period") && frame.contains("[X]"), "{frame}");
        assert!(frame.contains("Last week  Week of 2026-08-31"), "{frame}");
        assert!(frame.contains("first play 3 March 2026"), "{frame}");
        assert!(frame.contains("↑↓ pick · Enter choose · Esc keep Last week"), "{frame}");
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Esc);
        assert_eq!(p.modal, Modal::None);
        assert_eq!(p.period.name, "Last week", "Esc keeps");
        press(&mut p, KeyCode::Char('p'));
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Enter);
        assert_eq!(p.period.name, "Last month");
        assert!(matches!(&p.queued, Some(Op::Load { period, .. }) if period.is("month", -1)));
        // The last option is All time; stepping past it stays there.
        for _ in 0..20 {
            press(&mut p, KeyCode::Char(']'));
        }
        assert_eq!(p.period.name, "All time");
        assert!(matches!(&p.queued, Some(Op::Load { period, .. }) if period.period == "all"));
    }

    #[test]
    fn the_origin_cycles_on_o_only_while_the_log_holds_peer_plays() {
        let _en = english();
        let mut p = ready();
        press(&mut p, KeyCode::Char('o'));
        assert_eq!(p.origin, Origin::Local);
        assert!(matches!(&p.queued, Some(Op::Load { origin: Origin::Local, .. })));
        let live = p.seq;
        let mut only_local = summary(374, 400);
        only_local.origins.peers = OriginSlice::default();
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(only_local, Some(periods()))) });
        let frame = draw(&mut p);
        assert!(frame.contains("(•) this server"), "the control stays while the filter is on:\n{frame}");
        press(&mut p, KeyCode::Char('o'));
        press(&mut p, KeyCode::Char('o'));
        assert_eq!(p.origin, Origin::All);

        let mut p = page();
        p.reload(true);
        p.queued = None;
        let mut no_peers = summary(374, 400);
        no_peers.origins.peers = OriginSlice::default();
        let live = p.seq;
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(no_peers, Some(periods()))) });
        let frame = draw(&mut p);
        assert!(!frame.contains("ORIGIN") && !frame.contains("WHERE THE TRACKS LIVE") && !frame.contains("o origin"), "{frame}");
        assert!(press(&mut p, KeyCode::Char('o')).is_none());
        assert_eq!(p.origin, Origin::All);
        assert!(p.queued.is_none());
    }

    #[test]
    fn the_top_tab_ranks_with_a_share_bar_and_reranks_on_t_and_m() {
        let _en = english();
        let mut p = ready();
        press(&mut p, KeyCode::Right);
        assert_eq!(p.tab, Tab::Top);
        let frame = draw(&mut p);
        assert!(frame.contains("SHOW  (•) tracks   ( ) artists   ( ) albums   ( ) genres"), "{frame}");
        assert!(frame.contains("RANK BY  (•) plays   ( ) time"), "{frame}");
        assert!(frame.contains("#   TRACK") && frame.contains("SHARE") && frame.contains("PLAYS") && frame.contains("TIME") && frame.contains("LAST"), "{frame}");
        let first = row(&frame, "Boards of Canada - Roygbiv");
        assert!(first.contains("▰▰▰▰▰▰▰▰▰▰") && first.contains("38") && first.contains("2h 31m") && first.contains("today"), "{first}");
        let peer = row(&frame, "Bassnectar - Rewind The Track");
        assert!(peer.contains(" · via attic") && peer.contains("▰▰▰▰▰▱▱▱▱▱") && peer.contains("1h 24m"), "{peer}");
        assert!(frame.contains("↑↓ select · ←→ tab · t what · m rank by · [ ] period · o origin · q quit"), "{frame}");

        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        let frame = draw(&mut p);
        assert!(frame.contains("Aphex Twin - Xtal · Selected Ambient Works 85-92 (1992) · 31 plays · 2h 32m · last played today"), "{frame}");
        assert!(frame.contains("↑↓ rows · ←→ tab · t what · m rank by · [ ] period · o origin · Esc deselect"), "{frame}");

        press(&mut p, KeyCode::Char('m'));
        assert_eq!(p.metric, Metric::Time);
        assert!(matches!(&p.queued, Some(Op::Top { metric: Metric::Time, entity: Entity::Tracks, .. })));
        assert!(p.sel.is_none(), "a re-rank drops the cursor");
        p.queued = None;
        let frame = draw(&mut p);
        let first = row(&frame, "Aphex Twin - Xtal");
        assert!(first.contains("▰▰▰▰▰▰▰▰▰▰"), "the share follows the metric:\n{first}");

        press(&mut p, KeyCode::Char('t'));
        assert_eq!(p.entity, Entity::Artists);
        assert!(matches!(&p.queued, Some(Op::Top { entity: Entity::Artists, metric: Metric::Time, .. })));
        p.queued = None;
        let live = p.seq;
        let artists = StatsTop {
            entity: "artists".into(),
            metric: "time".into(),
            items: vec![top_item(1, 96, 25_320_000, None, Some("Boards of Canada"), None), top_item(2, 61, 17_880_000, None, Some("Aphex Twin"), None)],
        };
        p.apply(Done::Top { seq: live, result: Ok(artists) });
        let frame = draw(&mut p);
        assert!(frame.contains("#   ARTIST") && frame.contains("TRACKS") && !frame.contains("LAST"), "{frame}");
        assert!(row(&frame, "Boards of Canada").contains("14 tracks"), "{frame}");
        press(&mut p, KeyCode::Down);
        assert!(draw(&mut p).contains("Boards of Canada · 14 tracks · 96 plays · 7h 02m"));
    }

    #[test]
    fn the_recent_tab_words_each_play_and_pages_as_the_cursor_nears_the_end() {
        let _en = english();
        let mut p = ready();
        press(&mut p, KeyCode::Left);
        assert_eq!(p.tab, Tab::Recent);
        let frame = draw(&mut p);
        assert!(frame.contains("TIME   DAY") && frame.contains("TRACK") && frame.contains("OUTCOME") && frame.contains("LISTENED") && frame.contains("CLIENT"), "{frame}");
        let done = row(&frame, "completed");
        assert!(done.contains("today") && done.contains("2:31") && done.contains("web player"), "{done}");
        let skipped = row(&frame, "skipped at 0:08");
        assert!(skipped.contains("0:08 of 8:15") && skipped.contains("not counted") && !skipped.contains("this player"), "not counted takes the client's place:\n{skipped}");
        let stopped = row(&frame, "stopped at 5:40");
        assert!(stopped.contains("yesterday") && stopped.contains("5:40 of 9:05") && stopped.contains("mStream Mobile") && !stopped.contains("2.4.1"), "the name alone where the version does not fit:\n{stopped}");
        let legacy = row(&frame, "scrobbled at 0:30");
        assert!(legacy.contains("older client"), "{legacy}");

        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        // Four rows and a cursor near the end: the next page is asked for once.
        assert!(matches!(&p.queued, Some(Op::History { before, .. }) if before == "cursor-2"), "{:?}", p.queued);
        assert!(p.next.is_none());
        assert_eq!(p.busy.as_deref(), Some("loading older plays…"));
        let frame = draw(&mut p);
        assert!(frame.contains("loading older plays…"), "the busy line wins the note line:\n{frame}");
        p.busy = None;
        let frame = draw(&mut p);
        assert!(row(&frame, "Nils Frahm - Says · today").contains("· skipped at 0:08 of 8:15 · not counted · x forgets it"), "{frame}");
        assert!(frame.contains("↑↓ rows · ←→ tab · x forget this play · [ ] period · o origin · Esc deselect"), "{frame}");
        p.queued = None;
        press(&mut p, KeyCode::Down);
        assert!(p.queued.is_none(), "no second ask while the cursor is taken");
        let live = p.seq;
        let more = StatsHistory { items: vec![play("p5", today0() - 86_400 * 12, "completed", 200_000, Some(200_000), true, Some("mstream-webapp/6.27.0"), None)], next: None };
        p.apply(Done::History { seq: live, result: Ok(more) });
        assert_eq!(p.history.len(), 5);
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        assert!(p.queued.is_none(), "the log has no more pages");
    }

    #[test]
    fn the_forget_gate_keeps_on_enter_and_forgets_on_y_then_reloads() {
        let _en = english();
        let mut p = ready();
        press(&mut p, KeyCode::Left);
        press(&mut p, KeyCode::Down);
        press(&mut p, KeyCode::Down);
        p.queued = None;
        p.busy = None;
        press(&mut p, KeyCode::Char('x'));
        assert_eq!(p.modal, Modal::Forget(1));
        let frame = draw(&mut p);
        assert!(frame.contains("Forget this play?"), "{frame}");
        assert!(frame.contains("Nils Frahm - Says · today") && frame.contains("skipped at 0:08 of 8:15"), "{frame}");
        assert!(frame.contains("It leaves the log and the counts. Nothing else changes."), "{frame}");
        assert!(frame.contains("◂ Keep") && frame.contains("  Forget  ") && frame.contains("y forget · Esc keep"), "{frame}");
        press(&mut p, KeyCode::Enter);
        assert_eq!(p.modal, Modal::None, "Enter is the safe choice");
        assert!(p.queued.is_none());

        press(&mut p, KeyCode::Char('x'));
        press(&mut p, KeyCode::Char('y'));
        assert!(matches!(&p.queued, Some(Op::Forget { id, row: 1 }) if id == "p2"));
        assert_eq!(p.busy.as_deref(), Some("forgetting the play…"));
        p.queued = None;
        p.apply(Done::Forgot { id: "p2".into(), row: 1, result: Ok(()) });
        assert_eq!(p.history.len(), 3);
        assert!(p.history.iter().all(|h| h.id != "p2"));
        assert_eq!(note(&p), "the play is forgotten — the log and the totals reload");
        assert!(matches!(&p.queued, Some(Op::Load { .. })), "the totals reload");
        assert!(p.busy.is_none(), "quietly");

        p.queued = None;
        press(&mut p, KeyCode::Char('x'));
        press(&mut p, KeyCode::Char('y'));
        p.apply(Done::Forgot { id: "p3".into(), row: 1, result: Err(ApiError::Server { status: 404, message: "No such play".into() }) });
        assert_eq!(note(&p), "could not forget the play: server error 404: No such play");
        assert_eq!(p.history.len(), 3);
    }

    #[test]
    fn a_log_with_nothing_in_it_and_a_period_with_nothing_in_it_say_so() {
        let _en = english();
        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        let empty = StatsPeriods { earliest: None, latest: None, periods: Vec::new() };
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(StatsSummary::default(), Some(empty))) });
        let frame = draw(&mut p);
        assert!(frame.contains("• no plays yet — plays land here as you listen"), "{frame}");
        assert!(frame.contains("The web player reports each track when it ends"), "{frame}");
        assert!(frame.contains("Plays this account reports through this server · times in UTC"), "{frame}");
        assert!(!frame.contains("PERIOD"), "nothing to step through:\n{frame}");
        assert!(frame.contains("←→ tab · q quit"), "{frame}");
        assert_eq!(p.options.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["This month", "All time"]);

        // A period the log covers but nothing was started in.
        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        let mut nothing = StatsSummary::default();
        nothing.period.from = "2026-08-01T00:00:00.000Z".into();
        p.period = Period { period: "month".into(), offset: -1, name: "Last month".into(), detail: Some("August 2026".into()) };
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(nothing, Some(periods()))) });
        let frame = draw(&mut p);
        assert!(frame.contains("• Last month — no plays started in this period"), "{frame}");
        assert!(frame.contains("Nothing in Last month"), "{frame}");
        assert!(frame.contains("Your log begins on 3 March 2026 — [ ] walks the periods, p lists them."), "{frame}");
        assert!(frame.contains("PERIOD  ‹ Last month ›"), "{frame}");

        // The default period with nothing in it, on a log that has data
        // elsewhere: the webapp's move, the first period with data.
        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        let only_old = StatsPeriods { earliest: Some("2025-03-03T14:00:00.000Z".into()), latest: None, periods: vec![PeriodOption { period: "year".into(), offset: -1, label: "2025".into(), from: "2025-01-01T00:00:00.000Z".into(), to: String::new() }] };
        p.apply(Done::Loaded { seq: live, result: Ok(loaded(StatsSummary::default(), Some(only_old))) });
        assert_eq!(p.period.name, "Last year");
        assert!(matches!(&p.queued, Some(Op::Load { period, .. }) if period.is("year", -1)));
    }

    #[test]
    fn load_errors_name_the_gate_and_an_old_server_is_told_apart() {
        let _en = english();
        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        p.apply(Done::Loaded { seq: live, result: Err(ApiError::NotFound("api/v1/stats/summary".into())) });
        assert!(p.no_api);
        let frame = draw(&mut p);
        assert!(frame.contains("• no Stats API — this server does not have it yet; it arrived in mStream 6.27"), "{frame}");
        assert!(frame.contains("←→ tab · q quit"), "{frame}");

        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        p.apply(Done::Loaded { seq: live, result: Err(ApiError::Forbidden("Forbidden".into())) });
        assert_eq!(note(&p), "this session has no account behind it — the log is per account; sign in with `mstream-player login`");
        let mut p = page();
        p.reload(true);
        p.queued = None;
        let live = p.seq;
        p.apply(Done::Loaded { seq: live, result: Err(ApiError::Network("connection refused".into())) });
        assert_eq!(note(&p), "could not load your listening: could not reach server: connection refused");
        assert!(p.note.as_ref().is_some_and(|(_, err)| *err));
    }

    #[test]
    fn the_words_follow_the_webapp() {
        let _en = english();
        assert_eq!(fmt_duration(0), "0 min");
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
}
