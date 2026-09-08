//! The Backups room: the webapp's Backups page (beta), in the kit.
//!
//! The page's live-progress card and its queue notice are one state line
//! (idle dim, running green with the kit's scan bar beneath, tasks queued
//! gold); the add form is the affirmative card holding one spot above the
//! table; the table keeps the page's columns but two — the throttle and
//! the exclude list ride the cursor row's note line. Add and edit share
//! one modal form; a destination's runs are a modal; removing is a gold
//! gate. The room polls every two seconds while a run is in flight and
//! every five otherwise, as the page polls.
//!
//! Every server call runs on a worker thread (the wizard's Job/Done
//! pattern); the destination folder is browsed the Libraries way — the
//! server's own browser, or the OS dialog with `--same-machine`.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{
    Outcome, Screen, age_text, draw_bottom, draw_header, fmt_bytes, fmt_count, frame_ground,
    gate_message, host_of, iso_unix, printable, unix_now,
};
use crate::api::types::{ActiveBackup, BackupDestination, BackupRun, BackupStatus, DirListing, PathCheck, RunAnswer};
use crate::api::{ApiError, BackupPatch, Client, NewBackupDestination};
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, bold, dim};
use crate::setup::{Browse, ellipsize_path_start, g, join_server_path, parent_server_path, picker};

/// The page's cadences: two seconds with a run in flight, five otherwise.
const POLL_ACTIVE: Duration = Duration::from_secs(2);
const POLL_IDLE: Duration = Duration::from_secs(5);
const MIN_W: u16 = 80;
const MIN_H: u16 = 24;
/// Where a server-side browse starts: the server user's home.
const SERVER_HOME: &str = "~";
/// The server's caps.
const MAX_THROTTLE_MS: u32 = 60_000;
const MAX_GLOBS: usize = 64;
/// How many runs the history modal asks for.
const HISTORY_LIMIT: u32 = 50;

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    AfterScan,
    Daily,
    Manual,
}

impl Trigger {
    const ALL: [Trigger; 3] = [Trigger::AfterScan, Trigger::Daily, Trigger::Manual];

    fn as_str(self) -> &'static str {
        match self {
            Trigger::AfterScan => "after-scan",
            Trigger::Daily => "daily",
            Trigger::Manual => "manual",
        }
    }

    fn parse(s: &str) -> Trigger {
        match s {
            "daily" => Trigger::Daily,
            "manual" => Trigger::Manual,
            _ => Trigger::AfterScan,
        }
    }

    fn label(self) -> String {
        match self {
            Trigger::AfterScan => t!("bak.trig_after_scan"),
            Trigger::Daily => t!("bak.trig_daily"),
            Trigger::Manual => t!("bak.trig_manual"),
        }
        .to_string()
    }

    fn description(self) -> String {
        match self {
            Trigger::AfterScan => t!("bak.trig_after_scan_desc"),
            Trigger::Daily => t!("bak.trig_daily_desc"),
            Trigger::Manual => t!("bak.trig_manual_desc"),
        }
        .to_string()
    }
}

/// One form field, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Library,
    Trigger,
    Dest,
    Retention,
    Throttle,
    Hour,
    Excludes,
}

/// The add and edit forms — one shape.
#[derive(Debug, Clone)]
pub(crate) struct Form {
    /// The destination being edited; None while adding.
    pub editing: Option<i64>,
    /// Adding: the choice among the room's libraries.
    pub library: usize,
    /// Editing: the fixed library.
    pub library_name: String,
    pub library_id: i64,
    pub trigger: Trigger,
    pub dest: Input,
    pub retention: Input,
    pub throttle: Input,
    pub hour: Input,
    /// Comma-separated patterns.
    pub excludes: Input,
    /// Editing: `p` pressed — send null so the row follows the defaults.
    pub excludes_reset: bool,
    /// Editing: the row as loaded, to send only what changed.
    pub original: Option<BackupDestination>,
    /// The server's answer for `checked_for`, when it has one.
    pub check: Option<PathCheck>,
    pub checked_for: String,
    /// Index into [`Form::fields`].
    pub focus: usize,
    pub error: Option<String>,
}

impl Form {
    fn add(libraries: &[(String, i64)], defaults: &[String]) -> Self {
        Form {
            editing: None,
            library: 0,
            library_name: libraries.first().map(|(n, _)| n.clone()).unwrap_or_default(),
            library_id: libraries.first().map(|(_, id)| *id).unwrap_or(0),
            trigger: Trigger::AfterScan,
            dest: Input::default(),
            retention: Input::new("30".into()),
            throttle: Input::new("0".into()),
            hour: Input::new("3".into()),
            excludes: Input::new(defaults.join(", ")),
            excludes_reset: false,
            original: None,
            check: None,
            checked_for: String::new(),
            focus: 0,
            error: None,
        }
    }

    fn edit(d: &BackupDestination) -> Self {
        Form {
            editing: Some(d.id),
            library: 0,
            library_name: printable(&d.library_name, 64),
            library_id: d.library_id,
            trigger: Trigger::parse(&d.trigger_type),
            dest: Input::new(d.dest_path.clone()),
            retention: Input::new(d.retention_days.to_string()),
            throttle: Input::new(d.inter_file_delay_ms.to_string()),
            hour: Input::new(d.daily_at_hour.unwrap_or(3).to_string()),
            excludes: Input::new(d.exclude_globs.join(", ")),
            excludes_reset: false,
            original: Some(d.clone()),
            check: None,
            checked_for: String::new(),
            focus: 0,
            error: None,
        }
    }

    /// The fields in Tab order: the library only while adding, the hour
    /// only for a daily trigger.
    fn fields(&self) -> Vec<Field> {
        let mut fields = Vec::new();
        if self.editing.is_none() {
            fields.push(Field::Library);
        }
        fields.extend([Field::Trigger, Field::Dest, Field::Retention, Field::Throttle]);
        if self.trigger == Trigger::Daily {
            fields.push(Field::Hour);
        }
        fields.push(Field::Excludes);
        fields
    }

    fn focused(&self) -> Field {
        let fields = self.fields();
        fields[self.focus.min(fields.len() - 1)]
    }

    fn input_mut(&mut self, field: Field) -> Option<&mut Input> {
        match field {
            Field::Dest => Some(&mut self.dest),
            Field::Retention => Some(&mut self.retention),
            Field::Throttle => Some(&mut self.throttle),
            Field::Hour => Some(&mut self.hour),
            Field::Excludes => Some(&mut self.excludes),
            Field::Library | Field::Trigger => None,
        }
    }

    /// The patterns as the API takes them: trimmed, empties dropped.
    fn globs(&self) -> Vec<String> {
        self.excludes.value().split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
    }

    /// The numbers, or which one is wrong.
    fn numbers(&self) -> Result<(u32, u32, Option<u32>), String> {
        let retention: u32 = self.retention.value().trim().parse().map_err(|_| t!("bak.err_retention").to_string())?;
        let throttle: u32 = self
            .throttle
            .value()
            .trim()
            .parse()
            .ok()
            .filter(|t| *t <= MAX_THROTTLE_MS)
            .ok_or_else(|| t!("bak.err_throttle").to_string())?;
        let hour = if self.trigger == Trigger::Daily {
            Some(
                self.hour
                    .value()
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|h| *h <= 23)
                    .ok_or_else(|| t!("bak.err_hour").to_string())?,
            )
        } else {
            None
        };
        Ok((retention, throttle, hour))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Modal {
    None,
    Form(Box<Form>),
    /// The server's folder browser, opened from the form's destination
    /// field; the form waits underneath.
    Browser { browse: Browse, form: Box<Form> },
    History { dest: Box<BackupDestination>, runs: Vec<BackupRun>, loaded: bool, sel: usize },
    Remove(i64),
}

/// Everything a click can mean. Rebuilt into a rect registry every draw;
/// the last-drawn rect wins, which is what puts modals above the room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Act {
    Add,
    Select(usize),
    OpenHistory(i64),
    RunNow(i64),
    Edit(i64),
    ToggleEnabled(i64),
    Remove(i64),
    RemoveConfirm,
    RemoveCancel,
    FormFocus(usize),
    /// A radio choice by click: the field and the option.
    FormPick(usize, usize),
    FormBrowse,
    FormResetExcludes,
    FormSubmit,
    FormCancel,
    BrowseRow(usize),
    BrowseEnter,
    BrowseUp,
    BrowseChoose,
    BrowseCancel,
    HistoryPick(usize),
    HistoryClose,
    TableScroll(i8),
    TableScrollTo(usize),
    Quit,
}

/// A server call queued from input handling and run right after the next
/// draw. Ops carry everything they need — the worker never sees the room.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    /// The destinations, the live status, and the library names → ids.
    Load,
    Platform,
    /// The path preview — quiet, and superseded by a newer one.
    Check { library_id: i64, dest: String, exclude_dest_id: Option<i64> },
    Add(NewBackupDestination),
    Patch { id: i64, patch: BackupPatch, what: Did },
    Remove(i64),
    Run(i64),
    History(i64),
    PickNative,
    Browse(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Did {
    Saved,
    Enabled(bool),
}

struct Loaded {
    dests: Vec<BackupDestination>,
    status: BackupStatus,
    libraries: Option<Vec<(String, i64)>>,
}

/// What the worker sends back for each [`Op`].
enum Done {
    Loaded(Result<Box<Loaded>, ApiError>),
    Platform(Result<Vec<String>, ApiError>),
    Checked { dest: String, result: Result<PathCheck, ApiError> },
    Added(Result<(), ApiError>),
    Patched { id: i64, what: Did, result: Result<(), ApiError> },
    Removed(Result<(), ApiError>),
    Ran(Result<RunAnswer, ApiError>),
    History { id: i64, result: Result<Vec<BackupRun>, ApiError> },
    Picked(picker::Pick),
    Browsed(Result<DirListing, String>),
}

fn spawn_worker() -> (Sender<(Arc<Client>, Op)>, Receiver<Done>) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<(Arc<Client>, Op)>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Done>();
    std::thread::spawn(move || {
        while let Ok((client, op)) = job_rx.recv() {
            let done = match op {
                Op::Load => Done::Loaded(client.admin_backup_destinations().and_then(|dests| {
                    let status = client.admin_backup_status()?;
                    // The library names → ids, for the add form; best-effort.
                    let libraries = client
                        .admin_directories()
                        .ok()
                        .map(|dirs| dirs.into_iter().map(|(name, d)| (name, d.id)).collect());
                    Ok(Box::new(Loaded { dests, status, libraries }))
                })),
                Op::Platform => Done::Platform(client.admin_backup_platform().map(|p| p.default_excludes)),
                Op::Check { library_id, dest, exclude_dest_id } => {
                    let result = client.admin_backup_check_path(library_id, &dest, exclude_dest_id);
                    Done::Checked { dest, result }
                }
                Op::Add(dest) => Done::Added(client.admin_backup_add(&dest).map(|_| ())),
                Op::Patch { id, patch, what } => {
                    Done::Patched { id, what, result: client.admin_backup_patch(id, &patch).map(|_| ()) }
                }
                Op::Remove(id) => Done::Removed(client.admin_backup_remove(id).map(|_| ())),
                Op::Run(id) => Done::Ran(client.admin_backup_run(id)),
                Op::History(id) => Done::History { id, result: client.admin_backup_history(id, HISTORY_LIMIT) },
                Op::PickNative => Done::Picked(picker::pick_folder()),
                Op::Browse(path) => Done::Browsed(client.admin_file_explorer(&path).map_err(|e| e.to_string())),
            };
            if done_tx.send(done).is_err() {
                return;
            }
        }
    });
    (job_tx, done_rx)
}

pub(crate) struct Room {
    client: Arc<Client>,
    to_worker: Sender<(Arc<Client>, Op)>,
    from_worker: Receiver<Done>,
    /// Booted with `--same-machine`: the OS folder dialog picks the
    /// destination, and its paths are the server's paths.
    same_machine: bool,
    pub dests: Vec<BackupDestination>,
    /// The first load has answered.
    loaded: bool,
    pub status: BackupStatus,
    /// The libraries, name → id, for the add form.
    pub libraries: Vec<(String, i64)>,
    /// The server's default exclude patterns.
    defaults: Vec<String>,
    /// The KEYBOARD cursor — `None` until ↑/↓ (a click on a row does the same).
    pub sel: Option<usize>,
    pub modal: Modal,
    /// One line of status above the tips: (text, is_error).
    pub note: Option<(String, bool)>,
    busy: Option<String>,
    queued: Option<Op>,
    in_flight: bool,
    /// A path check that arrived while another op ran — only the newest counts.
    pending_check: Option<Op>,
    tscroll: usize,
    sel_anchor: Option<usize>,
    last_load: Option<Instant>,
    ui: Surface<Act>,
}

impl Room {
    pub(super) fn new(client: Client, same_machine: bool) -> Self {
        let (to_worker, from_worker) = spawn_worker();
        Room {
            client: Arc::new(client),
            to_worker,
            from_worker,
            same_machine,
            dests: Vec::new(),
            loaded: false,
            status: BackupStatus::default(),
            libraries: Vec::new(),
            defaults: Vec::new(),
            sel: None,
            modal: Modal::None,
            // The page's beta notice, until anything else has a say.
            note: Some((t!("bak.beta_note").to_string(), false)),
            busy: None,
            queued: None,
            in_flight: false,
            pending_check: None,
            tscroll: 0,
            sel_anchor: None,
            last_load: None,
            ui: Surface::new(),
        }
    }

    fn queue(&mut self, op: Op, busy: impl Into<String>) {
        self.queued = Some(op);
        self.busy = Some(busy.into());
    }

    fn reload(&mut self, loud: bool) {
        if loud {
            self.queue(Op::Load, t!("bak.busy_loading"));
        } else {
            self.queued = Some(Op::Load);
        }
    }

    fn dest(&self, id: i64) -> Option<&BackupDestination> {
        self.dests.iter().find(|d| d.id == id)
    }

    fn selected(&self) -> Option<&BackupDestination> {
        self.sel.and_then(|s| self.dests.get(s))
    }

    fn running_id(&self) -> Option<i64> {
        self.status.active.as_ref().map(|a| a.destination_id)
    }

    // ── Screen-level input ──────────────────────────────────────────────────

    fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::Add => {
                if self.libraries.is_empty() {
                    self.note = Some((t!("bak.no_libraries").to_string(), true));
                } else {
                    let form = Form::add(&self.libraries, &self.defaults);
                    self.modal = Modal::Form(Box::new(form));
                }
            }
            Act::Select(i) => {
                if i < self.dests.len() {
                    self.sel = Some(i);
                }
            }
            Act::OpenHistory(id) => {
                if let Some(d) = self.dest(id) {
                    self.modal = Modal::History { dest: Box::new(d.clone()), runs: Vec::new(), loaded: false, sel: 0 };
                    self.queue(Op::History(id), t!("bak.busy_history"));
                }
            }
            Act::RunNow(id) => {
                if let Some(d) = self.dest(id) {
                    if !d.enabled {
                        self.note = Some((t!("bak.run_disabled").to_string(), true));
                    } else {
                        let name = printable(&d.library_name, 64);
                        self.queue(Op::Run(id), t!("bak.busy_run", name = name));
                    }
                }
            }
            Act::Edit(id) => {
                if let Some(d) = self.dest(id) {
                    let form = Form::edit(d);
                    self.modal = Modal::Form(Box::new(form));
                    self.check_path();
                }
            }
            Act::ToggleEnabled(id) => {
                if let Some(d) = self.dest(id) {
                    let on = !d.enabled;
                    let patch = BackupPatch { enabled: Some(on), ..Default::default() };
                    self.queue(Op::Patch { id, patch, what: Did::Enabled(on) }, t!("bak.busy_saving"));
                }
            }
            Act::Remove(id) => {
                if self.dest(id).is_some() {
                    self.modal = Modal::Remove(id);
                }
            }
            Act::RemoveConfirm => {
                if let Modal::Remove(id) = self.modal {
                    self.modal = Modal::None;
                    self.queue(Op::Remove(id), t!("bak.busy_removing"));
                }
            }
            Act::RemoveCancel | Act::FormCancel | Act::HistoryClose => self.modal = Modal::None,
            Act::FormFocus(i) => {
                if let Modal::Form(f) = &mut self.modal {
                    f.focus = i.min(f.fields().len().saturating_sub(1));
                }
            }
            Act::FormPick(field_index, option) => {
                if let Modal::Form(f) = &mut self.modal
                    && let Some(field) = f.fields().get(field_index).copied()
                {
                    f.focus = field_index;
                    self.pick_option(field, Some(option));
                }
            }
            Act::FormBrowse => self.start_browse(),
            Act::FormResetExcludes => {
                if let Modal::Form(f) = &mut self.modal {
                    f.excludes = Input::new(self.defaults.join(", "));
                    f.excludes_reset = f.editing.is_some();
                    f.error = None;
                }
            }
            Act::FormSubmit => self.submit_form(),
            Act::BrowseRow(i) => {
                if let Modal::Browser { browse, .. } = &mut self.modal {
                    browse.sel = i.min(browse.dirs.len().saturating_sub(1));
                }
            }
            Act::BrowseEnter => {
                if let Modal::Browser { browse, .. } = &self.modal
                    && let Some(dir) = browse.dirs.get(browse.sel)
                {
                    let to = join_server_path(&browse.path, dir);
                    self.queue(Op::Browse(to), t!("busy.listing"));
                }
            }
            Act::BrowseUp => {
                if let Modal::Browser { browse, .. } = &self.modal
                    && let Some(parent) = parent_server_path(&browse.path)
                {
                    self.queue(Op::Browse(parent), t!("busy.listing"));
                }
            }
            Act::BrowseChoose => {
                if let Modal::Browser { browse, form } = &self.modal {
                    let path = browse.path.clone();
                    let mut form = form.clone();
                    form.dest = Input::new(path);
                    self.modal = Modal::Form(form);
                    self.check_path();
                }
            }
            Act::BrowseCancel => {
                if let Modal::Browser { form, .. } = &self.modal {
                    let form = form.clone();
                    self.modal = Modal::Form(form);
                }
            }
            Act::HistoryPick(i) => {
                if let Modal::History { sel, runs, .. } = &mut self.modal {
                    *sel = i.min(runs.len().saturating_sub(1));
                }
            }
            Act::TableScroll(delta) => {
                self.tscroll = if delta < 0 { self.tscroll.saturating_sub(1) } else { self.tscroll.saturating_add(1) };
            }
            Act::TableScrollTo(pos) => self.tscroll = pos,
            Act::Quit => return Some(Outcome::Quit),
        }
        None
    }

    /// A radio field's choice: the next option (`None`), or one by index.
    fn pick_option(&mut self, field: Field, option: Option<usize>) {
        let libraries = self.libraries.clone();
        let Modal::Form(f) = &mut self.modal else { return };
        match field {
            Field::Library if !libraries.is_empty() => {
                f.library = option.unwrap_or((f.library + 1) % libraries.len()).min(libraries.len() - 1);
                let (name, id) = &libraries[f.library];
                f.library_name = name.clone();
                f.library_id = *id;
                f.check = None;
                self.check_path();
            }
            Field::Trigger => {
                let i = Trigger::ALL.iter().position(|t| *t == f.trigger).unwrap_or(0);
                f.trigger = Trigger::ALL[option.unwrap_or((i + 1) % 3).min(2)];
            }
            _ => {}
        }
    }

    /// Browse for the destination: the OS dialog when this terminal shares
    /// the server's disk, the server's own browser otherwise.
    fn start_browse(&mut self) {
        if !matches!(self.modal, Modal::Form(_)) {
            return;
        }
        if self.same_machine {
            self.queue(Op::PickNative, t!("admin.busy_picker"));
        } else {
            let start = match &self.modal {
                Modal::Form(f) if !f.dest.value().trim().is_empty() => f.dest.value().trim().to_string(),
                _ => SERVER_HOME.to_string(),
            };
            self.queue(Op::Browse(start), t!("busy.listing"));
        }
    }

    /// Ask the server what saving the form's path would say — quiet, and
    /// only the newest ask counts.
    fn check_path(&mut self) {
        let Modal::Form(f) = &mut self.modal else { return };
        let dest = f.dest.value().trim().to_string();
        if dest.is_empty() || f.library_id == 0 {
            f.check = None;
            f.checked_for.clear();
            return;
        }
        let op = Op::Check { library_id: f.library_id, dest, exclude_dest_id: f.editing };
        if self.in_flight {
            self.pending_check = Some(op);
        } else {
            self.queued = Some(op);
        }
    }

    /// Send the form — after the refusals that need no server.
    fn submit_form(&mut self) {
        let defaults = self.defaults.clone();
        let Modal::Form(f) = &mut self.modal else { return };
        let dest = f.dest.value().trim().to_string();
        if dest.is_empty() {
            f.error = Some(t!("bak.err_dest").to_string());
            return;
        }
        let (retention, throttle, hour) = match f.numbers() {
            Ok(n) => n,
            Err(e) => {
                f.error = Some(e);
                return;
            }
        };
        let globs = f.globs();
        if globs.len() > MAX_GLOBS {
            f.error = Some(t!("bak.err_globs", max = MAX_GLOBS).to_string());
            return;
        }
        if let Some(check) = &f.check
            && f.checked_for == dest
            && !check.errors.is_empty()
        {
            f.error = Some(check.errors.join(" · "));
            return;
        }
        f.error = None;
        match f.original.clone() {
            None => {
                // Untouched patterns are OMITTED so the row keeps following
                // the server's defaults (the API's three states).
                let exclude_globs = (globs != defaults).then_some(globs);
                let op = Op::Add(NewBackupDestination {
                    library_id: f.library_id,
                    dest_path: dest,
                    trigger_type: f.trigger.as_str().to_string(),
                    daily_at_hour: hour,
                    retention_days: retention,
                    inter_file_delay_ms: throttle,
                    exclude_globs,
                });
                self.queue(op, t!("bak.busy_adding"));
            }
            Some(orig) => {
                let mut patch = BackupPatch::default();
                if dest != orig.dest_path {
                    patch.dest_path = Some(dest);
                }
                let trigger = f.trigger.as_str();
                if trigger != orig.trigger_type {
                    patch.trigger_type = Some(trigger.to_string());
                }
                if hour != orig.daily_at_hour && (f.trigger == Trigger::Daily || orig.daily_at_hour.is_some()) {
                    patch.daily_at_hour = Some(hour);
                }
                if retention != orig.retention_days {
                    patch.retention_days = Some(retention);
                }
                if throttle != orig.inter_file_delay_ms {
                    patch.inter_file_delay_ms = Some(throttle);
                }
                if f.excludes_reset {
                    patch.exclude_globs = Some(None);
                } else if globs != orig.exclude_globs {
                    patch.exclude_globs = Some(Some(globs));
                }
                if patch == BackupPatch::default() {
                    self.modal = Modal::None;
                    return;
                }
                let id = orig.id;
                self.queue(Op::Patch { id, patch, what: Did::Saved }, t!("bak.busy_saving"));
            }
        }
    }

    // ── Server calls ────────────────────────────────────────────────────────

    fn dispatch_queued(&mut self) {
        if self.in_flight {
            if let Some(Op::Check { .. }) = self.queued {
                self.pending_check = self.queued.take();
            }
            return;
        }
        let Some(op) = self.queued.take() else { return };
        self.in_flight = true;
        if op == Op::Load {
            self.last_load = Some(Instant::now());
        }
        if self.to_worker.send((self.client.clone(), op)).is_err() {
            self.in_flight = false;
            self.note = Some((t!("note.worker_gone").to_string(), true));
        }
    }

    /// Fold one worker result back into the state.
    fn apply(&mut self, done: Done) {
        self.in_flight = false;
        self.busy = None;
        if let Some(check) = self.pending_check.take() {
            self.queued = Some(check);
        }
        match done {
            Done::Loaded(Ok(loaded)) => {
                let loaded = *loaded;
                self.loaded = true;
                self.dests = loaded.dests;
                self.status = loaded.status;
                if let Some(libs) = loaded.libraries {
                    self.libraries = libs;
                }
                let n = self.dests.len();
                self.sel = self.sel.filter(|_| n > 0).map(|s| s.min(n - 1));
            }
            Done::Loaded(Err(e)) => {
                self.note = Some((gate_message(&e, &t!("bak.load_failed")), true));
            }
            Done::Platform(Ok(defaults)) => {
                // A form opened before the defaults landed seeds them now,
                // if the operator has not typed into the field.
                if let Modal::Form(f) = &mut self.modal
                    && f.editing.is_none()
                    && f.excludes.value() == self.defaults.join(", ")
                {
                    f.excludes = Input::new(defaults.join(", "));
                }
                self.defaults = defaults;
                if !self.loaded {
                    self.reload(true);
                }
            }
            Done::Platform(Err(_)) => {
                // Quiet — the form then starts blank; the room still loads.
                if !self.loaded {
                    self.reload(true);
                }
            }
            Done::Checked { dest, result } => {
                if let Modal::Form(f) = &mut self.modal
                    && f.dest.value().trim() == dest
                {
                    match result {
                        Ok(check) => {
                            f.check = Some(check);
                            f.checked_for = dest;
                        }
                        Err(e) => {
                            f.check = Some(PathCheck { ok: false, errors: vec![e.to_string()], ..Default::default() });
                            f.checked_for = dest;
                        }
                    }
                }
            }
            Done::Added(Ok(())) => {
                self.modal = Modal::None;
                self.note = Some((t!("bak.done_added").to_string(), false));
                self.reload(false);
            }
            Done::Added(Err(e)) => {
                let what = t!("bak.fail_add").to_string();
                match &mut self.modal {
                    Modal::Form(f) => f.error = Some(format!("{what}: {e}")),
                    _ => self.fail(&what, e),
                }
            }
            Done::Patched { id, what, result: Ok(()) } => {
                match what {
                    Did::Saved => {
                        self.modal = Modal::None;
                        self.note = Some((t!("bak.done_saved").to_string(), false));
                    }
                    Did::Enabled(on) => {
                        if let Some(d) = self.dests.iter_mut().find(|d| d.id == id) {
                            d.enabled = on;
                        }
                        self.note = Some((if on { t!("bak.done_on") } else { t!("bak.done_off") }.to_string(), false));
                    }
                }
                self.reload(false);
            }
            Done::Patched { what, result: Err(e), .. } => {
                let text = match what {
                    Did::Saved => t!("bak.fail_save"),
                    Did::Enabled(_) => t!("bak.fail_toggle"),
                }
                .to_string();
                match &mut self.modal {
                    Modal::Form(f) if what == Did::Saved => f.error = Some(format!("{text}: {e}")),
                    _ => self.fail(&text, e),
                }
            }
            Done::Removed(Ok(())) => {
                self.note = Some((t!("bak.done_removed").to_string(), false));
                self.reload(false);
            }
            Done::Removed(Err(e)) => self.fail(&t!("bak.fail_remove"), e),
            Done::Ran(Ok(answer)) => {
                self.note = Some(if answer.status == "skipped" {
                    (t!("bak.run_skipped").to_string(), true)
                } else {
                    (t!("bak.run_started").to_string(), false)
                });
                self.reload(false);
            }
            Done::Ran(Err(e)) => self.fail(&t!("bak.fail_run"), e),
            Done::History { id, result } => {
                let mut failure = None;
                if let Modal::History { dest, runs, loaded, .. } = &mut self.modal
                    && dest.id == id
                {
                    *loaded = true;
                    match result {
                        Ok(list) => *runs = list,
                        Err(e) => failure = Some(e),
                    }
                }
                if let Some(e) = failure {
                    self.fail(&t!("bak.fail_history"), e);
                }
            }
            Done::Picked(picker::Pick::Folder(path)) => {
                if let Modal::Form(f) = &mut self.modal {
                    f.dest = Input::new(path.display().to_string());
                    self.check_path();
                }
            }
            Done::Picked(picker::Pick::Cancelled) => {}
            Done::Picked(picker::Pick::Unavailable(why)) => {
                self.note = Some((t!("note.no_picker", why = why).to_string(), false));
                self.queue(Op::Browse(SERVER_HOME.to_string()), t!("busy.listing"));
            }
            Done::Browsed(Ok(listing)) => {
                let form = match &self.modal {
                    Modal::Form(f) => Some(f.clone()),
                    Modal::Browser { form, .. } => Some(form.clone()),
                    _ => None,
                };
                if let Some(form) = form {
                    let browse = Browse {
                        path: listing.path,
                        dirs: listing.directories.into_iter().map(|d| d.name).collect(),
                        sel: 0,
                    };
                    self.modal = Modal::Browser { browse, form };
                }
            }
            Done::Browsed(Err(e)) => {
                self.note = Some((t!("note.browse_failed", err = e).to_string(), true));
            }
        }
    }

    fn fail(&mut self, what: &str, e: ApiError) {
        self.note = Some((format!("{what}: {e}"), true));
    }
}

// ── The hub's view of the room ───────────────────────────────────────────────

impl Screen for Room {
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

    /// The page's poll, quiet: two seconds with a run in flight or tasks
    /// queued, five otherwise — never on top of a call already queued or
    /// running.
    fn tick(&mut self) {
        let every = if self.status.active.is_some() || self.status.queue_length > 0 { POLL_ACTIVE } else { POLL_IDLE };
        if self.loaded && !self.in_flight && self.queued.is_none() && self.last_load.is_none_or(|t| t.elapsed() >= every) {
            self.reload(false);
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        render(frame, self)
    }

    fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        handle_key(self, key)
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        Room::act(self, act)
    }

    fn wheel(&mut self, up: bool, _at: Position) {
        if matches!(self.modal, Modal::None) {
            self.tscroll = if up { self.tscroll.saturating_sub(1) } else { self.tscroll.saturating_add(1) };
        }
    }
}

/// The room, loading: what `mstream-player admin backups` opens. The
/// platform's defaults first, then the list — the worker is single-flight
/// and the add form needs both.
pub(super) fn start(client: Client, same_machine: bool) -> Room {
    let mut room = Room::new(client, same_machine);
    room.queue(Op::Platform, t!("bak.busy_loading"));
    room
}

// ── Keys ─────────────────────────────────────────────────────────────────────

fn handle_key(room: &mut Room, key: KeyEvent) -> Option<Outcome> {
    let code = key.code;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match &mut room.modal {
        Modal::Form(f) => {
            let n = f.fields().len();
            let field = f.focused();
            return match code {
                KeyCode::Esc => room.act(Act::FormCancel),
                KeyCode::Enter => room.act(Act::FormSubmit),
                KeyCode::Tab | KeyCode::Down => {
                    f.focus = (f.focus + 1) % n.max(1);
                    None
                }
                KeyCode::BackTab | KeyCode::Up => {
                    f.focus = (f.focus + n.max(1) - 1) % n.max(1);
                    None
                }
                KeyCode::Char('b') if ctrl => room.act(Act::FormBrowse),
                KeyCode::Char('p') if ctrl => room.act(Act::FormResetExcludes),
                // A radio field: ←/→ and Space walk its choices.
                KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                    if matches!(field, Field::Library | Field::Trigger) =>
                {
                    let count = if field == Field::Library { room.libraries.len() } else { Trigger::ALL.len() };
                    let current = if field == Field::Library {
                        f.library
                    } else {
                        Trigger::ALL.iter().position(|t| *t == f.trigger).unwrap_or(0)
                    };
                    if count > 0 {
                        let next = if code == KeyCode::Left { (current + count - 1) % count } else { (current + 1) % count };
                        room.pick_option(field, Some(next));
                    }
                    None
                }
                _ => {
                    // The focused editor takes the rest: digits only in the
                    // numbers, anything printable in the paths and patterns.
                    let numeric = matches!(field, Field::Retention | Field::Throttle | Field::Hour);
                    if let KeyCode::Char(ch) = code {
                        let allowed = if numeric { ch.is_ascii_digit() } else { !ch.is_control() };
                        let max = if numeric { 6 } else { 4096 };
                        let full = f.input_mut(field).is_some_and(|i| i.value().chars().count() >= max);
                        if !allowed || full {
                            return None;
                        }
                    }
                    let mut changed = false;
                    if let Some(input) = f.input_mut(field) {
                        changed = input.handle_event(&TermEvent::Key(key)).is_some_and(|c| c.value);
                    }
                    if changed {
                        f.error = None;
                        if field == Field::Excludes {
                            f.excludes_reset = false;
                        }
                    }
                    if changed && field == Field::Dest {
                        room.check_path();
                    }
                    None
                }
            };
        }
        Modal::Browser { browse, .. } => {
            return match code {
                KeyCode::Esc => room.act(Act::BrowseCancel),
                KeyCode::Up => {
                    browse.sel = browse.sel.saturating_sub(1);
                    None
                }
                KeyCode::Down => {
                    browse.sel = (browse.sel + 1).min(browse.dirs.len().saturating_sub(1));
                    None
                }
                KeyCode::Enter | KeyCode::Right => room.act(Act::BrowseEnter),
                KeyCode::Left | KeyCode::Backspace => room.act(Act::BrowseUp),
                KeyCode::Char('a') => room.act(Act::BrowseChoose),
                _ => None,
            };
        }
        Modal::History { runs, sel, .. } => {
            return match code {
                KeyCode::Esc | KeyCode::Enter => room.act(Act::HistoryClose),
                KeyCode::Up => {
                    *sel = sel.saturating_sub(1);
                    None
                }
                KeyCode::Down => {
                    *sel = (*sel + 1).min(runs.len().saturating_sub(1));
                    None
                }
                _ => None,
            };
        }
        Modal::Remove(_) => {
            return match code {
                KeyCode::Char('y') => room.act(Act::RemoveConfirm),
                // Enter is the SAFE choice on a warning gate.
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n') => room.act(Act::RemoveCancel),
                _ => None,
            };
        }
        Modal::None => {}
    }

    let n = room.dests.len();
    let id = room.selected().map(|d| d.id);
    match code {
        // ↑/↓ are the ONLY way a row gets highlighted (a click on a row
        // does the same); Esc puts the cursor away — and, with nothing
        // selected, leaves.
        KeyCode::Up => {
            if n > 0 {
                room.sel = Some(room.sel.map_or(n - 1, |s| s.saturating_sub(1)));
            }
            None
        }
        KeyCode::Down => {
            if n > 0 {
                room.sel = Some(room.sel.map_or(0, |s| (s + 1).min(n - 1)));
            }
            None
        }
        KeyCode::Esc => {
            if room.sel.is_some() {
                room.sel = None;
                None
            } else {
                room.act(Act::Quit)
            }
        }
        KeyCode::Enter | KeyCode::Char('h') => id.and_then(|id| room.act(Act::OpenHistory(id))),
        KeyCode::Char('n') => id.and_then(|id| room.act(Act::RunNow(id))),
        KeyCode::Char('e') => id.and_then(|id| room.act(Act::Edit(id))),
        KeyCode::Char('s') | KeyCode::Char(' ') => id.and_then(|id| room.act(Act::ToggleEnabled(id))),
        KeyCode::Char('r') | KeyCode::Delete => id.and_then(|id| room.act(Act::Remove(id))),
        KeyCode::Char('a') => room.act(Act::Add),
        KeyCode::Char('q') => room.act(Act::Quit),
        _ => None,
    }
}

// ── Drawing ──────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, room: &mut Room) {
    room.ui.begin_frame();
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };

    // A modal makes the room beneath INERT: the base draw sees no pointer,
    // and every rect it registered is dropped before the modal draws.
    let modal_open = !matches!(room.modal, Modal::None);
    let live_pointer = room.ui.pointer;
    if modal_open {
        room.ui.pointer = None;
    }

    let title = t!("bak.title").to_string();
    draw_header(frame, area, &title, &host_of(&room.client));
    // The page's BETA banner, as a chip beside the title.
    frame.render_widget(
        Paragraph::new(Span::styled(t!("bak.beta").to_string(), Style::default().fg(th().gold))),
        Rect { x: 2 + title.chars().count() as u16 + 1, y: 0, width: 8, height: 1 },
    );
    let column = Rect { x: 2, y: 2, width: area.width.saturating_sub(4), height: area.height.saturating_sub(5) };
    if room.loaded {
        draw_state(frame, room, column);
        draw_table(frame, room, column);
    }
    draw_bottom(frame, area, room.note.as_ref(), room.busy.as_deref(), &footer_hint(room));

    if modal_open {
        room.ui.pointer = live_pointer;
        room.ui.clear_registries();
    }
    match room.modal.clone() {
        Modal::None => {}
        Modal::Form(f) => draw_form(frame, room, area, &f),
        Modal::Browser { browse, .. } => draw_browser(frame, room, area, &browse),
        Modal::History { dest, runs, loaded, sel } => draw_history(frame, room, area, &dest, &runs, loaded, sel),
        Modal::Remove(id) => {
            let (lib, path) = room
                .dest(id)
                .map(|d| (printable(&d.library_name, 64), d.dest_path.clone()))
                .unwrap_or_default();
            draw_gate(
                frame,
                room,
                area,
                t!("bak.remove_title", library = lib, path = path).to_string(),
                vec![t!("bak.remove_1").to_string(), t!("bak.remove_2").to_string()],
                (t!("bak.remove_keep").to_string(), Act::RemoveCancel),
                (t!("bak.remove_confirm").to_string(), Act::RemoveConfirm),
            );
        }
    }
    if let Some((target, text)) = room.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

fn footer_hint(room: &Room) -> String {
    match &room.modal {
        Modal::Form(f) => if f.editing.is_some() { t!("bak.hint_edit") } else { t!("bak.hint_add") },
        Modal::Browser { .. } => t!("bak.hint_browser"),
        Modal::History { .. } => t!("bak.hint_history"),
        Modal::Remove(_) => t!("bak.hint_remove"),
        Modal::None if !room.loaded => t!("bak.hint_loading"),
        Modal::None => match (room.dests.is_empty(), room.sel) {
            (true, _) => t!("bak.hint_empty"),
            (false, None) => t!("bak.hint_rows"),
            (false, Some(_)) => t!("bak.hint_selected"),
        },
    }
    .to_string()
}

/// The state line: the run in flight with the kit's scan bar, the queue,
/// or idle.
fn draw_state(frame: &mut Frame, room: &mut Room, column: Rect) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let now = unix_now();
    let (spans, polls) = match &room.status.active {
        Some(a) => {
            let elapsed = a.started_at.as_deref().and_then(iso_unix).map(|t| elapsed_text(now - t)).unwrap_or_default();
            let trigger = a.trigger_reason.as_deref().map(trigger_words).unwrap_or_default();
            (
                vec![
                    Span::styled(t!("bak.state_running").to_string(), Style::default().fg(th().ok).add_modifier(Modifier::BOLD)),
                    Span::raw(
                        t!(
                            "bak.state_running_detail",
                            library = printable(a.library_name.as_deref().unwrap_or(""), 64),
                            path = a.dest_path.clone().unwrap_or_default(),
                            trigger = trigger,
                            elapsed = elapsed
                        )
                        .to_string(),
                    ),
                ],
                t!("bak.polls_2"),
            )
        }
        None if room.status.queue_length > 0 => (
            vec![
                Span::styled(
                    if room.status.queue_length == 1 { t!("bak.state_queued_one") } else { t!("bak.state_queued", n = room.status.queue_length) }.to_string(),
                    Style::default().fg(th().gold).add_modifier(Modifier::BOLD),
                ),
                Span::raw(t!("bak.state_queued_detail").to_string()),
            ],
            t!("bak.polls_2"),
        ),
        None => {
            let on = room.dests.iter().filter(|d| d.enabled).count();
            (
                vec![
                    Span::styled(t!("bak.state_idle").to_string(), dim()),
                    Span::raw(t!("bak.state_idle_detail", on = on, total = room.dests.len()).to_string()),
                ],
                t!("bak.polls_5"),
            )
        }
    };
    let state_w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    frame.render_widget(Paragraph::new(Line::from(spans)), line(column.y));
    let polls = polls.to_string();
    if state_w + 2 + polls.chars().count() <= column.width as usize {
        frame.render_widget(Paragraph::new(Span::styled(polls, dim())).alignment(Alignment::Right), line(column.y));
    }
    if let Some(a) = &room.status.active {
        frame.render_widget(Paragraph::new(progress_line(a, column.width as usize)), line(column.y + 1));
    }
}

/// The kit's scan bar for a run: ten cells, the percentage, the counts —
/// or the all-dim bar and "no estimate yet" on a destination's first run.
/// The count groups drop off the right when the column is narrow.
fn progress_line(a: &ActiveBackup, width: usize) -> Line<'static> {
    let done = a.files_copied + a.files_unchanged + a.files_trashed;
    let mut spans = Vec::new();
    match a.expected_files.filter(|e| *e > 0) {
        Some(expected) => {
            let pct = ((done * 100) / expected).min(100);
            let filled = (pct / 10) as usize;
            spans.push(Span::styled("▰".repeat(filled), Style::default().fg(th().ok)));
            spans.push(Span::styled("▱".repeat(10 - filled), dim()));
            spans.push(Span::styled(
                t!("bak.progress_pct", pct = pct, done = fmt_count(done), expected = fmt_count(expected)).to_string(),
                dim(),
            ));
        }
        None => {
            spans.push(Span::styled("▱".repeat(10), dim()));
            spans.push(Span::styled(t!("bak.progress_no_estimate").to_string(), dim()));
        }
    }
    let written = if a.bytes_copied > 0 {
        t!("bak.progress_written", bytes = fmt_bytes(a.bytes_copied)).to_string()
    } else {
        t!("bak.progress_no_bytes").to_string()
    };
    let mut groups = vec![
        t!("bak.sum_copied", n = fmt_count(a.files_copied)).to_string(),
        t!("bak.sum_unchanged", n = fmt_count(a.files_unchanged)).to_string(),
        t!("bak.sum_trashed", n = fmt_count(a.files_trashed)).to_string(),
        written,
    ];
    let head: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    while !groups.is_empty() && head + groups.iter().map(|g| g.chars().count() + 3).sum::<usize>() > width {
        groups.pop();
    }
    for group in groups {
        spans.push(Span::styled(format!(" · {group}"), dim()));
    }
    Line::from(spans)
}

/// The add card, then the destinations table.
fn draw_table(frame: &mut Frame, room: &mut Room, column: Rect) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let card = Rect { x: column.x, y: column.y + 2, width: column.width, height: 3 };
    let hover = room.ui.pointer.is_some_and(|p| card.contains(p));
    let color = if hover { th().bright } else { th().ok };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(color));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("bak.add_card").to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD)))
            .alignment(Alignment::Center),
        inner,
    );
    room.ui.click(card, Act::Add);
    room.ui.tip(card, t!("bak.tip_add"));

    let table = Rect { x: column.x, y: column.y + 6, width: column.width, height: column.height.saturating_sub(6) };
    if table.height < 3 {
        return;
    }
    let widest = |words: &[String]| words.iter().map(|w| w.chars().count() as u16).max().unwrap_or(0);
    let trig_w = widest(&[
        t!("bak.col_trigger").to_string(),
        t!("bak.trig_after_scan").to_string(),
        t!("bak.trig_daily_at", hour = "23").to_string(),
        t!("bak.trig_manual").to_string(),
    ])
    .clamp(12, 18);
    let ret_w = widest(&[t!("bak.col_retention").to_string(), t!("bak.no_trash").to_string()]).clamp(9, 14);
    let (lib_w, last_w, on_w) = (10u16, 18u16, 3u16);
    let fixed = lib_w + 2 + trig_w + 2 + ret_w + 2 + last_w + 2 + on_w + 2;
    let dest_w = table.width.saturating_sub(fixed).max(12);
    let dest_x = table.x + lib_w + 2;
    let trig_x = dest_x + dest_w + 2;
    let ret_x = trig_x + trig_w + 2;
    let last_x = ret_x + ret_w + 2;
    let on_x = last_x + last_w + 2;
    let head = |x: u16, w: u16| Rect { x, y: table.y, width: w, height: 1 };
    for (x, w, word) in [
        (table.x, lib_w, t!("bak.col_library")),
        (dest_x, dest_w, t!("bak.col_destination")),
        (trig_x, trig_w, t!("bak.col_trigger")),
        (ret_x, ret_w, t!("bak.col_retention")),
        (last_x, last_w, t!("bak.col_last_run")),
        (on_x, on_w, t!("bak.col_on")),
    ] {
        frame.render_widget(Paragraph::new(Span::styled(word.to_string(), dim())), head(x, w));
    }
    frame.render_widget(Paragraph::new(Span::styled("─".repeat(table.width as usize), dim())), line(table.y + 1));
    let rows_y = table.y + 2;
    if room.dests.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("bak.empty").to_string(), dim())), line(rows_y));
        return;
    }
    let avail = table.bottom().saturating_sub(rows_y) as usize;
    let sel_moved = room.sel != room.sel_anchor;
    room.sel_anchor = room.sel;
    let reveal = if sel_moved { room.sel } else { None };
    let (first, visible) = kit::table_view(room.dests.len(), reveal, room.tscroll, avail);
    room.tscroll = first;
    let now = unix_now();
    let running = room.running_id();
    let pct = room.status.active.as_ref().and_then(|a| {
        a.expected_files.filter(|e| *e > 0).map(|e| ((a.files_copied + a.files_unchanged + a.files_trashed) * 100 / e).min(100))
    });
    let dests = room.dests.clone();
    for (row, i) in (first..first + visible).enumerate() {
        let d = &dests[i];
        let y = rows_y + row as u16;
        let rect = line(y);
        let selected = room.sel == Some(i);
        let hovered = !selected && room.ui.pointer.is_some_and(|p| rect.contains(p));
        let base = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else {
            Style::default()
        };
        let faint = if selected || hovered { base } else { dim() };
        if selected {
            frame.render_widget(Paragraph::new(Span::styled(" ".repeat(table.width as usize), base)), rect);
        }
        let cell = |x: u16, w: u16| Rect { x, y, width: w, height: 1 };
        frame.render_widget(
            Paragraph::new(Span::styled(
                clip(&printable(&d.library_name, 64), lib_w),
                if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base },
            )),
            cell(table.x, lib_w),
        );
        let (shown, clipped) = ellipsize_path_start(&d.dest_path, dest_w as usize);
        frame.render_widget(Paragraph::new(Span::styled(shown, base)), cell(dest_x, dest_w));
        if clipped {
            room.ui.tip(cell(dest_x, dest_w), d.dest_path.clone());
        }
        frame.render_widget(Paragraph::new(Span::styled(clip(&trigger_cell(d), trig_w), base)), cell(trig_x, trig_w));
        let retention = if d.retention_days == 0 { (t!("bak.no_trash").to_string(), faint) } else { (t!("bak.days", n = d.retention_days).to_string(), base) };
        frame.render_widget(Paragraph::new(Span::styled(retention.0, retention.1)), cell(ret_x, ret_w));
        let (last, own) = if running == Some(d.id) {
            (
                match pct {
                    Some(p) => t!("bak.running_pct", pct = p).to_string(),
                    None => t!("bak.running").to_string(),
                },
                accent(),
            )
        } else {
            match &d.last_run {
                None => (t!("bak.never").to_string(), dim()),
                Some(run) => {
                    let age = iso_unix(&run.started_at).map(|t| age_text(now - t)).unwrap_or_default();
                    (format!("{} · {}", status_word(&run.status), age), status_style(&run.status))
                }
            }
        };
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&last, last_w), if selected || hovered { base } else { own })),
            cell(last_x, last_w),
        );
        // ON: the checkbox glyph wearing the state, clickable like every
        // other control, s on the cursor row by key.
        let on_rect = cell(on_x, on_w);
        let on_hover = room.ui.pointer.is_some_and(|p| on_rect.contains(p));
        let glyph = if d.enabled { g("[✓]", "[x]") } else { "[ ]" };
        let on_style = match (selected, on_hover, d.enabled) {
            (true, _, _) => base,
            (false, true, _) => Style::default().fg(th().bright).add_modifier(Modifier::BOLD),
            (false, false, true) => Style::default().fg(th().ok),
            (false, false, false) => dim(),
        };
        frame.render_widget(Paragraph::new(Span::styled(glyph, on_style)), on_rect);
        room.ui.click(on_rect, Act::ToggleEnabled(d.id));
        room.ui.tip(on_rect, t!("bak.tip_on"));
        room.ui.click(cell(table.x, on_x.saturating_sub(table.x + 1)), Act::Select(i));
    }
    let bar = Rect { x: table.x + table.width, y: rows_y, width: 1, height: visible as u16 };
    kit::scroll_list(frame, &mut room.ui, bar, dests.len(), visible, first, Act::TableScroll(-1), Act::TableScroll(1), Act::TableScrollTo);

    // The cursor row's extras ride the note line, when it is free.
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(d) = room.selected()
    {
        let mut spans = vec![Span::raw(d.dest_path.clone())];
        if d.inter_file_delay_ms > 0 {
            spans.push(Span::raw(format!(" · {}", t!("bak.note_throttle", ms = d.inter_file_delay_ms))));
        }
        let excludes = if d.exclude_globs == room.defaults {
            t!("bak.note_default_excludes").to_string()
        } else if d.exclude_globs.is_empty() {
            t!("bak.note_no_excludes").to_string()
        } else {
            t!("bak.note_patterns", n = d.exclude_globs.len()).to_string()
        };
        spans.push(Span::raw(format!(" · {excludes}")));
        if let Some(run) = &d.last_run {
            match run.status.as_str() {
                "failed" | "partial" => {
                    let why = printable(run.error_message.as_deref().unwrap_or(""), 200);
                    spans.push(Span::styled(format!(" · {}: {why}", status_word(&run.status)), Style::default().fg(th().gold)));
                }
                _ => spans.push(Span::styled(format!(" · {}", run_summary(run)), dim())),
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), line(column.bottom() + 1));
    }
}

// ── Modals ───────────────────────────────────────────────────────────────────

/// A labelled 3-row input inside a modal.
#[allow(clippy::too_many_arguments)]
fn modal_field(frame: &mut Frame, room: &mut Room, at: Rect, label: &str, input: &Input, focused: bool, muted: bool, act: Act) {
    modal_field_labelled(frame, room, at, at.width, label, input, focused, muted, act);
}

/// A labelled field whose label may run wider than its card (the number
/// cards sit side by side with their words in the gaps).
#[allow(clippy::too_many_arguments)]
fn modal_field_labelled(frame: &mut Frame, room: &mut Room, at: Rect, label_w: u16, label: &str, input: &Input, focused: bool, muted: bool, act: Act) {
    frame.render_widget(Paragraph::new(Span::styled(label.to_string(), dim())), Rect { x: at.x, y: at.y, width: label_w, height: 1 });
    let field = Rect { x: at.x, y: at.y + 1, width: at.width, height: 3 };
    let hover = !muted && room.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = card.inner(field);
    frame.render_widget(card, field);
    let shown = if focused {
        kit::input_display(input.value(), input.cursor(), inner.width.saturating_sub(2))
    } else {
        clip_tail(input.value(), inner.width.saturating_sub(2))
    };
    frame.render_widget(
        Paragraph::new(Span::styled(shown, if muted { dim() } else { Style::default() })),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 },
    );
    if !muted {
        room.ui.click(field, act);
    }
}

/// A radio choice row: `(•) name` per option, wrapping to the width.
fn radio_row(frame: &mut Frame, room: &mut Room, at: Rect, options: &[String], chosen: usize, focused: bool, field_index: usize) -> u16 {
    let mut x = at.x;
    let mut y = at.y;
    for (i, name) in options.iter().enumerate() {
        let w = name.chars().count() as u16 + 4;
        if x + w > at.right() && x > at.x {
            x = at.x;
            y += 1;
        }
        let rect = Rect { x, y, width: w, height: 1 };
        let on = i == chosen;
        let hovered = room.ui.pointer.is_some_and(|p| rect.contains(p));
        let glyph_style = if on && focused {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if on {
            accent()
        } else {
            dim()
        };
        let name_style = if hovered { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if on { bold() } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(if on { "(•)" } else { "( )" }, glyph_style), Span::raw(" "), Span::styled(name.clone(), name_style)])),
            rect,
        );
        room.ui.click(rect, Act::FormPick(field_index, i));
        x += w + 3;
    }
    y + 1 - at.y
}

/// The add and edit forms, one drawing.
fn draw_form(frame: &mut Frame, room: &mut Room, area: Rect, f: &Form) {
    let fields = f.fields();
    let focused = f.focused();
    let index_of = |field: Field| fields.iter().position(|x| *x == field).unwrap_or(0);
    let editing = f.editing.is_some();
    let library_names: Vec<String> = room.libraries.iter().map(|(n, _)| n.clone()).collect();
    // Rows the library line takes: a wrapped radio row while adding.
    let lib_rows = if editing {
        1
    } else {
        let width = 74u16.saturating_sub(4 + 10) as usize;
        let mut rows = 1;
        let mut x = 0;
        for name in &library_names {
            let w = name.chars().count() + 4;
            if x + w > width && x > 0 {
                rows += 1;
                x = 0;
            }
            x += w + 3;
        }
        rows
    };
    let height = 25 + lib_rows;
    let inner = kit::modal_frame(frame, area, 74, height, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let title = if editing {
        t!("bak.edit_title", library = f.library_name, path = f.original.as_ref().map(|d| d.dest_path.clone()).unwrap_or_default())
    } else {
        t!("bak.add_title")
    };
    frame.render_widget(
        Paragraph::new(Span::styled(clip(&title, w.saturating_sub(4)), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::FormCancel, t!("path_modal.tip_close"));
    let mut y = inner.y + 2;

    // LIBRARY
    frame.render_widget(Paragraph::new(Span::styled(t!("bak.field_library").to_string(), dim())), Rect { x, y, width: 10, height: 1 });
    if editing {
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(f.library_name.clone(), bold()), Span::styled(t!("bak.library_fixed").to_string(), dim())])),
            Rect { x: x + 10, y, width: w.saturating_sub(10), height: 1 },
        );
        y += 2;
    } else if library_names.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("bak.no_libraries").to_string(), dim())), Rect { x: x + 10, y, width: w.saturating_sub(10), height: 1 });
        y += 2;
    } else {
        let used = radio_row(
            frame,
            room,
            Rect { x: x + 10, y, width: w.saturating_sub(10), height: lib_rows },
            &library_names,
            f.library,
            focused == Field::Library,
            index_of(Field::Library),
        );
        y += used + 1;
    }

    // TRIGGER: the kit's radio group, one row per option with its words.
    frame.render_widget(Paragraph::new(Span::styled(t!("bak.field_trigger").to_string(), dim())), Rect { x, y, width: 10, height: 1 });
    for (i, trigger) in Trigger::ALL.iter().enumerate() {
        let rect = Rect { x: x + 10, y: y + i as u16, width: w.saturating_sub(10), height: 1 };
        let on = *trigger == f.trigger;
        let hovered = room.ui.pointer.is_some_and(|p| rect.contains(p));
        let glyph_style = if on && focused == Field::Trigger {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if on {
            accent()
        } else {
            dim()
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if on { "(•)" } else { "( )" }, glyph_style),
                Span::raw(" "),
                Span::styled(trigger.label(), if hovered { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if on { bold() } else { Style::default() }),
                Span::styled(format!(" — {}", trigger.description()), dim()),
            ])),
            rect,
        );
        room.ui.click(rect, Act::FormPick(index_of(Field::Trigger), i));
    }
    y += 4;

    // DESTINATION: the path, ^B to browse, the server's verdict beneath.
    modal_field(
        frame,
        room,
        Rect { x, y, width: w, height: 4 },
        &t!("bak.field_dest"),
        &f.dest,
        focused == Field::Dest,
        false,
        Act::FormFocus(index_of(Field::Dest)),
    );
    let browse = t!("bak.browse_button").to_string();
    let browse_w = browse.chars().count() as u16 + 4;
    kit::button(frame, &mut room.ui, Rect { x: inner.right().saturating_sub(browse_w + 1), y, width: browse_w, height: 1 }, &browse, false, Act::FormBrowse);
    y += 4;
    let dest_text = f.dest.value().trim();
    let verdict: Option<(String, Style)> = if dest_text.is_empty() {
        None
    } else if f.checked_for != dest_text || f.check.is_none() {
        Some((t!("bak.checking").to_string(), dim()))
    } else if let Some(check) = &f.check {
        if let Some(err) = check.errors.first() {
            Some((format!("− {}", printable(err, 200)), Style::default().fg(th().gold)))
        } else if let Some(warn) = check.warnings.first() {
            let more = if check.warnings.len() > 1 { format!(" (+{})", check.warnings.len() - 1) } else { String::new() };
            Some((format!("− {}{more}", printable(warn, 200)), Style::default().fg(th().gold)))
        } else {
            Some((format!("{} {}", g("✓", "+"), t!("bak.check_ok")), Style::default().fg(th().ok)))
        }
    } else {
        None
    };
    // The server's sentence may run long: two rows, wrapped at words.
    if let Some((text, style)) = verdict {
        frame.render_widget(
            Paragraph::new(Span::styled(printable(&text, 2 * w as usize), style)).wrap(Wrap { trim: true }),
            Rect { x, y, width: w, height: 2 },
        );
    }
    y += 2;

    // The numbers, side by side; the hour muted unless the trigger is daily.
    let numbers: [(Field, String, String, &Input, bool); 3] = [
        (Field::Retention, t!("bak.field_retention").to_string(), t!("bak.hint_retention").to_string(), &f.retention, false),
        (Field::Throttle, t!("bak.field_throttle").to_string(), t!("bak.hint_throttle").to_string(), &f.throttle, false),
        (Field::Hour, t!("bak.field_hour").to_string(), t!("bak.hint_hour").to_string(), &f.hour, f.trigger != Trigger::Daily),
    ];
    for (i, (field, label, hint, input, muted)) in numbers.iter().enumerate() {
        let fx = x + i as u16 * 21;
        let room_w = w.saturating_sub(i as u16 * 21);
        modal_field_labelled(
            frame,
            room,
            Rect { x: fx, y, width: 12.min(room_w), height: 4 },
            20.min(room_w),
            label,
            input,
            focused == *field,
            *muted,
            Act::FormFocus(index_of(*field)),
        );
        frame.render_widget(Paragraph::new(Span::styled(hint.clone(), dim())), Rect { x: fx, y: y + 4, width: 20.min(room_w), height: 1 });
    }
    y += 5;

    // EXCLUDE PATTERNS
    modal_field(
        frame,
        room,
        Rect { x, y, width: w, height: 4 },
        &t!("bak.field_excludes"),
        &f.excludes,
        focused == Field::Excludes,
        false,
        Act::FormFocus(index_of(Field::Excludes)),
    );
    y += 4;
    if let Some(err) = &f.error {
        frame.render_widget(Paragraph::new(Span::styled(clip(err, w), Style::default().fg(th().gold))), line(y));
    }
    let by = inner.bottom().saturating_sub(1);
    if editing {
        kit::button(frame, &mut room.ui, Rect { x, y: by, width: w, height: 1 }, &t!("bak.reset_patterns"), false, Act::FormResetExcludes);
    }
    let label = if editing { t!("bak.save") } else { t!("bak.add_submit") }.to_string();
    let bx = inner.right().saturating_sub(label.chars().count() as u16 + 4);
    kit::button(frame, &mut room.ui, Rect { x: bx, y: by, width: inner.width, height: 1 }, &label, true, Act::FormSubmit);
}

/// The server's folder browser, over the form.
fn draw_browser(frame: &mut Frame, room: &mut Room, area: Rect, browse: &Browse) {
    let inner = kit::modal_frame(frame, area, 66, 18, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("bak.browser_title").to_string(), bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::BrowseCancel, t!("path_modal.tip_close"));
    frame.render_widget(
        Paragraph::new(Span::styled(browse.path.clone(), dim())),
        Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
    );
    let list_top = inner.y + 3;
    let visible = inner.height.saturating_sub(5) as usize;
    let first = browse.sel.saturating_sub(visible.saturating_sub(1));
    for (row, i) in (first..browse.dirs.len().min(first + visible)).enumerate() {
        let selected = i == browse.sel;
        let style = if selected { Style::default().fg(th().on_accent).bg(th().accent) } else { Style::default() };
        let rect = Rect { x: inner.x, y: list_top + row as u16, width: inner.width, height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(format!("{} {}", g("▸", "►"), browse.dirs[i]), style)), rect);
        room.ui.click(rect, Act::BrowseRow(i));
    }
    if browse.dirs.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("browse.empty").to_string(), dim())),
            Rect { x: inner.x, y: list_top, width: inner.width, height: 1 },
        );
    }
    let y = inner.bottom().saturating_sub(1);
    let row = |x| Rect { x, y, width: inner.width, height: 1 };
    let up = kit::button(frame, &mut room.ui, row(inner.x), &t!("browse.up"), false, Act::BrowseUp);
    let open = kit::button(frame, &mut room.ui, row(up.right() + 1), &t!("browse.open"), false, Act::BrowseEnter);
    let choose = kit::button(frame, &mut room.ui, row(open.right() + 1), &t!("bak.browse_use"), true, Act::BrowseChoose);
    kit::button(frame, &mut room.ui, row(choose.right() + 1), &t!("browse.close"), false, Act::BrowseCancel);
}

/// A destination's runs, newest first; the selected run's notes beneath.
fn draw_history(frame: &mut Frame, room: &mut Room, area: Rect, dest: &BackupDestination, runs: &[BackupRun], loaded: bool, sel: usize) {
    let inner = kit::modal_frame(frame, area, 84, 21, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    frame.render_widget(
        Paragraph::new(Span::styled(
            clip(&t!("bak.history_title", library = printable(&dest.library_name, 64), path = dest.dest_path), w.saturating_sub(4)),
            Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
        )),
        line(inner.y),
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::HistoryClose, t!("path_modal.tip_close"));
    if !loaded {
        return;
    }
    if runs.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("bak.history_empty").to_string(), dim())), line(inner.y + 2));
        return;
    }
    let (started_w, status_w, trig_w, count_w, bytes_w) = (9u16, 8u16, 15u16, 9u16, 9u16);
    let started_x = x;
    let status_x = started_x + started_w + 2;
    let trig_x = status_x + status_w + 2;
    let copied_x = trig_x + trig_w + 2;
    let unch_x = copied_x + count_w + 2;
    let trash_x = unch_x + count_w + 2;
    let bytes_x = trash_x + count_w + 2;
    let hy = inner.y + 2;
    let head = |x: u16, w: u16, word: String, right: bool| {
        let p = Paragraph::new(Span::styled(word, dim()));
        (Rect { x, y: hy, width: w, height: 1 }, if right { p.alignment(Alignment::Right) } else { p })
    };
    for (rect, p) in [
        head(started_x, started_w, t!("bak.hcol_started").to_string(), false),
        head(status_x, status_w, t!("bak.hcol_status").to_string(), false),
        head(trig_x, trig_w, t!("bak.hcol_trigger").to_string(), false),
        head(copied_x, count_w, t!("bak.hcol_copied").to_string(), true),
        head(unch_x, count_w, t!("bak.hcol_unchanged").to_string(), true),
        head(trash_x, count_w, t!("bak.hcol_trashed").to_string(), true),
        head(bytes_x, bytes_w, t!("bak.hcol_bytes").to_string(), true),
    ] {
        frame.render_widget(p, rect);
    }
    frame.render_widget(Paragraph::new(Span::styled("─".repeat(w as usize), dim())), line(hy + 1));
    let rows_y = hy + 2;
    let visible = 8usize.min(runs.len());
    let first = sel.saturating_sub(visible.saturating_sub(1)).min(runs.len().saturating_sub(visible));
    let now = unix_now();
    for (row, i) in (first..first + visible).enumerate() {
        let run = &runs[i];
        let y = rows_y + row as u16;
        let selected = i == sel;
        let base = if selected { Style::default().fg(th().on_accent).bg(th().accent) } else { Style::default() };
        let rect = line(y);
        if selected {
            frame.render_widget(Paragraph::new(Span::styled(" ".repeat(w as usize), base)), rect);
        }
        let cell = |x: u16, w: u16| Rect { x, y, width: w, height: 1 };
        let started = iso_unix(&run.started_at).map(|t| age_text(now - t)).unwrap_or_default();
        frame.render_widget(Paragraph::new(Span::styled(started, base)), cell(started_x, started_w));
        frame.render_widget(
            Paragraph::new(Span::styled(status_word(&run.status), if selected { base } else { status_style(&run.status) })),
            cell(status_x, status_w),
        );
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&trigger_words(run.trigger_reason.as_deref().unwrap_or("")), trig_w), base)),
            cell(trig_x, trig_w),
        );
        for (cx, n) in [(copied_x, run.files_copied), (unch_x, run.files_unchanged), (trash_x, run.files_trashed)] {
            frame.render_widget(Paragraph::new(Span::styled(fmt_count(n), base)).alignment(Alignment::Right), cell(cx, count_w));
        }
        let bytes = if run.bytes_copied > 0 { (fmt_bytes(run.bytes_copied), base) } else { ("—".to_string(), if selected { base } else { dim() }) };
        frame.render_widget(Paragraph::new(Span::styled(bytes.0, bytes.1)).alignment(Alignment::Right), cell(bytes_x, bytes_w));
        room.ui.click(rect, Act::HistoryPick(i));
    }
    let after = rows_y + visible as u16;
    frame.render_widget(Paragraph::new(Span::styled(t!("bak.history_note", n = HISTORY_LIMIT).to_string(), dim())), line(after + 1));
    if let Some(run) = runs.get(sel) {
        let started = iso_unix(&run.started_at).map(|t| age_text(now - t)).unwrap_or_default();
        let duration = match (iso_unix(&run.started_at), run.finished_at.as_deref().and_then(iso_unix)) {
            (Some(a), Some(b)) if b >= a => elapsed_text(b - a),
            _ => String::new(),
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{started} · {}", status_word(&run.status)), status_style(&run.status).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" · {} · {duration}", trigger_words(run.trigger_reason.as_deref().unwrap_or(""))), dim()),
            ])),
            line(after + 3),
        );
        let notes = match run.error_message.as_deref().filter(|m| !m.trim().is_empty()) {
            Some(m) => printable(m, 400),
            None => run_summary(run),
        };
        frame.render_widget(
            Paragraph::new(Span::styled(notes, if run.status == "success" { dim() } else { Style::default().fg(th().gold) })).wrap(Wrap { trim: false }),
            Rect { x, y: after + 4, width: w, height: inner.bottom().saturating_sub(after + 4) },
        );
    }
}

/// A warning gate: gold, consequences before verbs, the safe choice as
/// the primary, no [X].
fn draw_gate(frame: &mut Frame, room: &mut Room, area: Rect, title: String, body: Vec<String>, safe: (String, Act), go: (String, Act)) {
    let inner = kit::modal_frame(frame, area, 68, 8 + body.len() as u16, th().gold);
    let gold = Style::default().fg(th().gold);
    let mut lines = vec![Line::from(Span::styled(title, gold.add_modifier(Modifier::BOLD))), Line::from("")];
    lines.extend(body.into_iter().map(|b| Line::from(Span::styled(b, gold))));
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(2) },
    );
    let y = inner.bottom().saturating_sub(1);
    let go_w = go.0.chars().count() as u16 + 4;
    let safe_w = safe.0.chars().count() as u16 + 4;
    let safe_x = inner.right().saturating_sub(go_w + 2 + safe_w);
    let safe_rect = kit::button(frame, &mut room.ui, Rect { x: safe_x, y, width: inner.width, height: 1 }, &safe.0, true, safe.1);
    kit::button(frame, &mut room.ui, Rect { x: safe_rect.right() + 2, y, width: inner.width, height: 1 }, &go.0, false, go.1);
}

// ── Words ────────────────────────────────────────────────────────────────────

fn clip(text: &str, width: u16) -> String {
    let w = width as usize;
    if text.chars().count() <= w { text.to_string() } else { format!("{}…", text.chars().take(w.saturating_sub(1)).collect::<String>()) }
}

/// Keep the END of a value visible (paths, long pattern lists).
fn clip_tail(text: &str, width: u16) -> String {
    let w = width as usize;
    let n = text.chars().count();
    if n <= w { text.to_string() } else { format!("…{}", text.chars().skip(n + 1 - w).collect::<String>()) }
}

/// The trigger column: the webapp's formatTrigger.
fn trigger_cell(d: &BackupDestination) -> String {
    match d.trigger_type.as_str() {
        "daily" => t!("bak.trig_daily_at", hour = format!("{:02}", d.daily_at_hour.unwrap_or(0))).to_string(),
        other => trigger_words(other),
    }
}

/// A trigger reason as a word: `after-scan`, `daily`, `manual`, or the
/// server's own token.
fn trigger_words(reason: &str) -> String {
    match reason {
        "after-scan" => t!("bak.trig_after_scan").to_string(),
        "daily" => t!("bak.trig_daily_word").to_string(),
        "manual" => t!("bak.trig_manual").to_string(),
        other => printable(other, 30),
    }
}

fn status_word(status: &str) -> String {
    match status {
        "success" => t!("bak.st_success"),
        "failed" => t!("bak.st_failed"),
        "partial" => t!("bak.st_partial"),
        "skipped" => t!("bak.st_skipped"),
        "running" => t!("bak.st_running"),
        other => std::borrow::Cow::Owned(printable(other, 20)),
    }
    .to_string()
}

/// success green; failed, partial and skipped gold; running the accent.
fn status_style(status: &str) -> Style {
    match status {
        "success" => Style::default().fg(th().ok),
        "failed" | "partial" | "skipped" => Style::default().fg(th().gold),
        "running" => accent(),
        _ => dim(),
    }
}

/// The webapp's formatRunSummary: each non-zero count, or "no changes".
fn run_summary(run: &BackupRun) -> String {
    let mut parts = Vec::new();
    if run.files_copied > 0 {
        parts.push(t!("bak.sum_copied", n = fmt_count(run.files_copied)).to_string());
    }
    if run.files_unchanged > 0 {
        parts.push(t!("bak.sum_unchanged", n = fmt_count(run.files_unchanged)).to_string());
    }
    if run.files_trashed > 0 {
        parts.push(t!("bak.sum_trashed", n = fmt_count(run.files_trashed)).to_string());
    }
    if parts.is_empty() && run.status == "success" {
        parts.push(t!("bak.sum_no_changes").to_string());
    }
    let mut s = parts.join(", ");
    if run.bytes_copied > 0 {
        s.push_str(&format!(" · {}", fmt_bytes(run.bytes_copied)));
    }
    s
}

/// The webapp's formatElapsed: `42s`, `2m 14s`, `1h 03m`.
fn elapsed_text(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}h {:02}m", s / 3600, s % 3600 / 60)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::PathCheckInfo;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// English strings under assertion: hold the wizard tests' locale lock
    /// (one of them flips the process-global locale) and pin English.
    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        guard
    }

    fn new_room() -> Room {
        Room::new(Client::new("http://home.mstream.example:3000").expect("client"), false)
    }

    /// SQLite's UTC form, as the rows carry it.
    fn sqlite_at(t: i64) -> String {
        super::super::iso_at(t).replace('T', " ")[..19].to_string()
    }

    const DEFAULTS: [&str; 4] = ["Thumbs.db", "desktop.ini", ".DS_Store", "._*"];

    fn defaults() -> Vec<String> {
        DEFAULTS.iter().map(|s| s.to_string()).collect()
    }

    fn run(status: &str, ago: i64, copied: u64, unchanged: u64, trashed: u64, bytes: u64, error: Option<&str>) -> BackupRun {
        BackupRun {
            id: 7,
            started_at: sqlite_at(unix_now() - ago),
            finished_at: Some(sqlite_at(unix_now() - ago + 134)),
            status: status.into(),
            trigger_reason: Some(if status == "failed" { "daily".into() } else { "after-scan".into() }),
            files_copied: copied,
            files_unchanged: unchanged,
            files_trashed: trashed,
            bytes_copied: bytes,
            error_message: error.map(str::to_string),
        }
    }

    fn dest(id: i64, library: &str, path: &str, trigger: &str, retention: u32, enabled: bool, last: Option<BackupRun>) -> BackupDestination {
        BackupDestination {
            id,
            library_id: id * 10,
            library_name: library.into(),
            dest_path: path.into(),
            trigger_type: trigger.into(),
            daily_at_hour: (trigger == "daily").then_some(3),
            retention_days: retention,
            enabled,
            inter_file_delay_ms: if id == 2 { 200 } else { 0 },
            exclude_globs: defaults(),
            last_run: last,
            created_at: sqlite_at(unix_now() - 30 * 86_400),
        }
    }

    fn dests() -> Vec<BackupDestination> {
        vec![
            dest(1, "music", "/Volumes/Backup/music", "after-scan", 30, true, Some(run("success", 2 * 3600, 812, 11_204, 3, 3_650_000_000, None))),
            dest(2, "podcasts", "/Volumes/Backup/podcasts", "daily", 30, true, Some(run("failed", 86_400, 0, 0, 0, 0, Some("EACCES: permission denied, mkdir '/Volumes/Backup/podcasts/.mstream-trash'")))),
            dest(3, "vinyl", "/mnt/usb/vinyl", "manual", 0, false, None),
        ]
    }

    fn loaded(dests: Vec<BackupDestination>, status: BackupStatus) -> Done {
        Done::Loaded(Ok(Box::new(Loaded {
            dests,
            status,
            libraries: Some(vec![("music".into(), 10), ("podcasts".into(), 20), ("vinyl".into(), 30)]),
        })))
    }

    fn idle() -> Room {
        let mut room = new_room();
        room.apply(Done::Platform(Ok(defaults())));
        room.queued = None;
        room.busy = None;
        room.apply(loaded(dests(), BackupStatus::default()));
        room
    }

    fn key_press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(room: &mut Room, code: KeyCode) -> Option<Outcome> {
        handle_key(room, key_press(code))
    }

    fn ctrl(room: &mut Room, c: char) -> Option<Outcome> {
        handle_key(room, KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn type_text(room: &mut Room, text: &str) {
        for c in text.chars() {
            press(room, KeyCode::Char(c));
        }
    }

    /// Render one frame and flatten the buffer to text for assertions.
    fn draw(room: &mut Room) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, room)).unwrap();
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

    fn row(frame: &str, name: &str) -> String {
        frame.lines().find(|l| l.starts_with(&format!("  {name}"))).map(str::to_string).unwrap_or_default()
    }

    #[test]
    fn the_room_boots_defaults_first_and_draws_the_table() {
        let _en = english();
        let mut room = new_room();
        room.queue(Op::Platform, "x");
        room.queued = None;
        room.apply(Done::Platform(Ok(defaults())));
        assert_eq!(room.queued, Some(Op::Load), "the list follows the defaults");
        room.queued = None;
        room.busy = None;
        room.apply(loaded(Vec::new(), BackupStatus::default()));
        let frame = draw(&mut room);
        assert!(frame.contains("Backups beta"), "{frame}");
        assert!(frame.contains("• idle — nothing running · 0 of 0 destinations on · trash swept hourly"), "{frame}");
        assert!(frame.contains("Add a backup destination"), "{frame}");
        assert!(frame.contains("(no destinations yet — a adds one)"), "{frame}");
        assert!(frame.contains("beta — the configuration may change in a future release"), "{frame}");
        assert!(frame.contains("a add a destination · Esc back"), "{frame}");
        room.apply(loaded(dests(), BackupStatus::default()));
        let frame = draw(&mut room);
        for word in ["LIBRARY", "DESTINATION", "TRIGGER", "RETENTION", "LAST RUN", "ON", "polls every 5 s"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        let music = row(&frame, "music");
        assert!(music.contains("/Volumes/Backup/music") && music.contains("after each scan") && music.contains("30d") && music.contains("success · 2h") && music.contains("[✓]"), "{music}");
        let podcasts = row(&frame, "podcasts");
        assert!(podcasts.contains("daily at 03:00") && podcasts.contains("failed · 24h"), "{podcasts}");
        let vinyl = row(&frame, "vinyl");
        assert!(vinyl.contains("manual only") && vinyl.contains("no trash") && vinyl.contains("never") && vinyl.contains("[ ]"), "{vinyl}");
        assert!(frame.contains("• idle — nothing running · 2 of 3 destinations on"), "{frame}");
        // The cursor row's extras on the note line, the failure in its words.
        room.note = None;
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("/Volumes/Backup/podcasts · 200 ms/file · default excludes · failed: EACCES: permission denied"), "{frame}");
        assert!(frame.contains("↑↓ rows · Enter history · n run now · e edit · s on/off · r remove · Esc deselect"), "{frame}");
        press(&mut room, KeyCode::Up);
        let frame = draw(&mut room);
        assert!(frame.contains("/Volumes/Backup/music · default excludes · 812 copied, 11,204 unchanged, 3 trashed · 3.4 GB"), "{frame}");
    }

    #[test]
    fn the_state_line_shows_the_run_the_queue_and_the_bar() {
        let _en = english();
        let mut room = idle();
        let active = ActiveBackup {
            destination_id: 2,
            history_id: 9,
            library_name: Some("podcasts".into()),
            dest_path: Some("/Volumes/Backup/podcasts".into()),
            started_at: Some(sqlite_at(unix_now() - 134)),
            trigger_reason: Some("after-scan".into()),
            files_copied: 812,
            files_unchanged: 380,
            files_trashed: 12,
            bytes_copied: 3_650_000_000,
            expected_files: Some(2930),
        };
        room.apply(loaded(dests(), BackupStatus { active: Some(active.clone()), queue_length: 1 }));
        let frame = draw(&mut room);
        assert!(frame.contains("• running — podcasts → /Volumes/Backup/podcasts · after each scan · 2m 14s"), "{frame}");
        assert!(frame.contains("▰▰▰▰▱▱▱▱▱▱ 41% · 1,204 of 2,930 · 812 copied · 380 unchanged · 12 trashed · 3.4 GB written"), "{frame}");
        assert!(frame.contains("polls every 2 s"), "{frame}");
        assert!(row(&frame, "podcasts").contains("running… 41%"), "{frame}");
        let mut first = active.clone();
        first.expected_files = None;
        first.bytes_copied = 0;
        room.apply(loaded(dests(), BackupStatus { active: Some(first), queue_length: 0 }));
        let frame = draw(&mut room);
        assert!(frame.contains("▱▱▱▱▱▱▱▱▱▱ first run — no estimate yet · 812 copied · 380 unchanged · 12 trashed"), "{frame}");
        assert!(!frame.contains("no bytes writ"), "the last group drops off a narrow column\n{frame}");
        assert!(row(&frame, "podcasts").contains("running…"), "{frame}");
        room.apply(loaded(dests(), BackupStatus { active: None, queue_length: 2 }));
        let frame = draw(&mut room);
        assert!(frame.contains("• 2 tasks queued — waiting for the active scan or backup to finish"), "{frame}");
        // The poll cadence follows the state.
        room.last_load = Some(Instant::now() - Duration::from_secs(3));
        room.queued = None;
        room.tick();
        assert_eq!(room.queued, Some(Op::Load), "two seconds while tasks wait");
        room.queued = None;
        room.apply(loaded(dests(), BackupStatus::default()));
        room.last_load = Some(Instant::now() - Duration::from_secs(3));
        room.queued = None;
        room.tick();
        assert!(room.queued.is_none(), "five seconds when idle");
    }

    #[test]
    fn the_add_form_walks_its_fields_checks_the_path_and_sends_what_changed() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Char('a'));
        let Modal::Form(f) = &room.modal else { panic!("the add form") };
        assert_eq!((f.focused(), f.library_name.as_str(), f.library_id, f.trigger), (Field::Library, "music", 10, Trigger::AfterScan));
        assert_eq!(f.excludes.value(), "Thumbs.db, desktop.ini, .DS_Store, ._*");
        let frame = draw(&mut room);
        assert!(frame.contains("Add a backup destination") && frame.contains("(•) music") && frame.contains("( ) podcasts"), "{frame}");
        assert!(frame.contains("(•) after each scan — when the library finishes scanning"), "{frame}");
        assert!(frame.contains("DESTINATION · another drive, not inside the library") && frame.contains("^B browse"), "{frame}");
        assert!(frame.contains("RETENTION DAYS") && frame.contains("THROTTLE MS/FILE") && frame.contains("HOUR (0–23)"), "{frame}");
        assert!(frame.contains("Tab next field · ←→ or Space choose · ^B browse · Enter add · Esc cancel"), "{frame}");
        // Radio fields walk with the arrows; the library choice re-asks the server.
        press(&mut room, KeyCode::Right);
        let Modal::Form(f) = &room.modal else { panic!("the add form") };
        assert_eq!((f.library_name.as_str(), f.library_id), ("podcasts", 20));
        press(&mut room, KeyCode::Tab); // trigger
        press(&mut room, KeyCode::Right);
        press(&mut room, KeyCode::Right);
        let Modal::Form(f) = &room.modal else { panic!("the add form") };
        assert_eq!(f.trigger, Trigger::Manual);
        assert!(!f.fields().contains(&Field::Hour), "the hour is a daily thing");
        press(&mut room, KeyCode::Tab); // destination
        type_text(&mut room, "/Volumes/Backup/pods");
        assert_eq!(room.queued, Some(Op::Check { library_id: 20, dest: "/Volumes/Backup/pods".into(), exclude_dest_id: None }));
        assert!(draw(&mut room).contains("checking the path…"));
        room.queued = None;
        room.apply(Done::Checked {
            dest: "/Volumes/Backup/pods".into(),
            result: Ok(PathCheck { ok: true, errors: Vec::new(), warnings: vec!["Destination does not exist yet. It will be created on the first backup run.".into()], info: PathCheckInfo::default() }),
        });
        let frame = draw(&mut room);
        assert!(frame.contains("− Destination does not exist yet. It will be created on the first"), "{frame}");
        assert!(frame.lines().any(|l| l.trim_matches(|c| c == ' ' || c == '│') == "backup run."), "the sentence wraps at a word\n{frame}");
        // An untouched pattern field is OMITTED so the row follows the defaults.
        press(&mut room, KeyCode::Enter);
        assert_eq!(
            room.queued,
            Some(Op::Add(NewBackupDestination {
                library_id: 20,
                dest_path: "/Volumes/Backup/pods".into(),
                trigger_type: "manual".into(),
                daily_at_hour: None,
                retention_days: 30,
                inter_file_delay_ms: 0,
                exclude_globs: None,
            }))
        );
        room.queued = None;
        room.busy = None;
        // A hard error from the check blocks the send; a refusal keeps the form.
        room.apply(Done::Checked {
            dest: "/Volumes/Backup/pods".into(),
            result: Ok(PathCheck { ok: false, errors: vec!["Destination is inside the source library".into()], ..Default::default() }),
        });
        press(&mut room, KeyCode::Enter);
        assert!(room.queued.is_none());
        let Modal::Form(f) = &room.modal else { panic!("the add form") };
        assert_eq!(f.error.as_deref(), Some("Destination is inside the source library"));
        room.apply(Done::Added(Err(ApiError::Server { status: 409, message: "A destination already exists for this library + path".into() })));
        let Modal::Form(f) = &room.modal else { panic!("a refusal keeps the form") };
        assert!(f.error.as_deref().is_some_and(|e| e.contains("already exists")));
        room.apply(Done::Added(Ok(())));
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "destination added"));
        assert_eq!(room.queued, Some(Op::Load));
    }

    #[test]
    fn the_edit_form_sends_only_what_changed_and_resets_patterns_to_null() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down); // podcasts: daily at 3, 200 ms
        press(&mut room, KeyCode::Char('e'));
        let Modal::Form(f) = &room.modal else { panic!("the edit form") };
        assert_eq!((f.editing, f.focused(), f.trigger), (Some(2), Field::Trigger, Trigger::Daily));
        assert_eq!(f.fields(), vec![Field::Trigger, Field::Dest, Field::Retention, Field::Throttle, Field::Hour, Field::Excludes]);
        assert_eq!(room.queued, Some(Op::Check { library_id: 20, dest: "/Volumes/Backup/podcasts".into(), exclude_dest_id: Some(2) }), "an edit checks its own path, skipping its own row");
        room.queued = None;
        let frame = draw(&mut room);
        assert!(frame.contains("Edit — podcasts → /Volumes/Backup/podcasts"), "{frame}");
        assert!(frame.contains("podcasts  — fixed: remove and re-add to retarget"), "{frame}");
        assert!(frame.contains("^P reset patterns to the defaults") && frame.contains("Save ▸"), "{frame}");
        // Nothing changed: the form just closes.
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none());
        press(&mut room, KeyCode::Char('e'));
        room.queued = None;
        for _ in 0..4 {
            press(&mut room, KeyCode::Tab); // to the hour
        }
        let Modal::Form(f) = &room.modal else { panic!("the edit form") };
        assert_eq!(f.focused(), Field::Hour);
        press(&mut room, KeyCode::Backspace);
        type_text(&mut room, "2x3");
        press(&mut room, KeyCode::Tab); // excludes
        type_text(&mut room, ", *.log");
        press(&mut room, KeyCode::Enter);
        assert_eq!(
            room.queued,
            Some(Op::Patch {
                id: 2,
                patch: BackupPatch {
                    daily_at_hour: Some(Some(23)),
                    exclude_globs: Some(Some(vec!["Thumbs.db".into(), "desktop.ini".into(), ".DS_Store".into(), "._*".into(), "*.log".into()])),
                    ..Default::default()
                },
                what: Did::Saved,
            }),
            "digits only reach the hour; only the changed fields go"
        );
        room.queued = None;
        room.busy = None;
        ctrl(&mut room, 'p');
        let Modal::Form(f) = &room.modal else { panic!("the edit form") };
        assert!(f.excludes_reset && f.excludes.value() == "Thumbs.db, desktop.ini, .DS_Store, ._*");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.queued, Some(Op::Patch { patch: BackupPatch { exclude_globs: Some(None), .. }, .. })), "the reset sends null");
        room.queued = None;
        room.apply(Done::Patched { id: 2, what: Did::Saved, result: Ok(()) });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.starts_with("saved")));
    }

    #[test]
    fn row_keys_run_toggle_and_gate_the_remove() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down); // vinyl: off
        press(&mut room, KeyCode::Char('n'));
        assert!(room.queued.is_none());
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("this destination is off")));
        press(&mut room, KeyCode::Char('s'));
        assert_eq!(room.queued, Some(Op::Patch { id: 3, patch: BackupPatch { enabled: Some(true), ..Default::default() }, what: Did::Enabled(true) }));
        room.queued = None;
        room.apply(Done::Patched { id: 3, what: Did::Enabled(true), result: Ok(()) });
        assert!(room.dests[2].enabled);
        room.queued = None;
        room.busy = None;
        press(&mut room, KeyCode::Char('n'));
        assert_eq!(room.queued, Some(Op::Run(3)));
        room.queued = None;
        room.apply(Done::Ran(Ok(RunAnswer { status: "skipped".into() })));
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("previous run is still in progress")));
        room.apply(Done::Ran(Ok(RunAnswer { status: "queued".into() })));
        assert!(room.note.as_ref().is_some_and(|(n, e)| !*e && n.contains("backup started")));
        room.queued = None;
        press(&mut room, KeyCode::Char('r'));
        assert!(matches!(room.modal, Modal::Remove(3)));
        let frame = draw(&mut room);
        assert!(frame.contains("Remove vinyl → /mnt/usb/vinyl?") && frame.contains("NOT deleted") && frame.contains("y remove · Esc keep"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Enter is the safe choice");
        press(&mut room, KeyCode::Char('r'));
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::Remove(3)));
        room.queued = None;
        room.apply(Done::Removed(Ok(())));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("the files on disk stay")));
        assert!(press(&mut room, KeyCode::Esc).is_none());
        assert!(matches!(press(&mut room, KeyCode::Esc), Some(Outcome::Quit)));
    }

    #[test]
    fn the_history_modal_lists_runs_and_tells_the_selected_ones_story() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::History { loaded: false, .. }));
        assert_eq!(room.queued, Some(Op::History(2)));
        room.queued = None;
        room.apply(Done::History {
            id: 2,
            result: Ok(vec![
                run("success", 3 * 3600, 12, 11_204, 3, 432_000_000, None),
                run("failed", 86_400, 0, 0, 0, 0, Some("EACCES: permission denied, mkdir '/Volumes/Backup/podcasts/.mstream-trash'")),
                run("skipped", 3 * 86_400, 0, 0, 0, 0, None),
            ]),
        });
        let frame = draw(&mut room);
        assert!(frame.contains("History — podcasts → /Volumes/Backup/podcasts"), "{frame}");
        for word in ["STARTED", "STATUS", "TRIGGER", "COPIED", "UNCHANGED", "TRASHED", "BYTES", "50 most recent · newest first"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        let first = frame.lines().find(|l| l.contains("3h") && l.contains("success")).map(str::to_string).unwrap_or_default();
        assert!(first.contains("after each scan") && first.contains("11,204") && first.contains("412.0 MB"), "{first}");
        assert!(frame.contains("3h · success · after each scan · 2m 14s"), "{frame}");
        assert!(frame.contains("12 copied, 11,204 unchanged, 3 trashed"), "{frame}");
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("24h · failed · daily · 2m 14s"), "{frame}");
        assert!(frame.contains("EACCES: permission denied, mkdir"), "{frame}");
        assert!(frame.contains("↑↓ runs · Esc close"), "{frame}");
        press(&mut room, KeyCode::Esc);
        assert!(matches!(room.modal, Modal::None));
    }

    #[test]
    fn browsing_fills_the_destination_and_returns_to_the_form() {
        let _en = english();
        let mut room = idle();
        press(&mut room, KeyCode::Char('a'));
        ctrl(&mut room, 'b');
        assert_eq!(room.queued, Some(Op::Browse("~".into())), "the server's home, as Libraries browses");
        room.queued = None;
        room.apply(Done::Browsed(Ok(DirListing {
            path: "/Volumes".into(),
            directories: vec![crate::api::types::DirEntry { name: "Backup".into() }, crate::api::types::DirEntry { name: "NAS".into() }],
            files: Vec::new(),
        })));
        assert!(matches!(room.modal, Modal::Browser { .. }));
        let frame = draw(&mut room);
        assert!(frame.contains("Choose the destination folder") && frame.contains("▸ Backup") && frame.contains("Use this folder ▸"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::Browse("/Volumes/Backup".into())));
        room.queued = None;
        room.apply(Done::Browsed(Ok(DirListing { path: "/Volumes/Backup".into(), directories: Vec::new(), files: Vec::new() })));
        press(&mut room, KeyCode::Char('a'));
        let Modal::Form(f) = &room.modal else { panic!("back in the form") };
        assert_eq!(f.dest.value(), "/Volumes/Backup");
        assert!(matches!(room.queued, Some(Op::Check { .. })), "the chosen path is checked at once");
        // Esc in the browser returns to the form with its draft intact.
        room.queued = None;
        ctrl(&mut room, 'b');
        room.queued = None;
        room.apply(Done::Browsed(Ok(DirListing { path: "/Volumes/Backup".into(), directories: Vec::new(), files: Vec::new() })));
        press(&mut room, KeyCode::Esc);
        let Modal::Form(f) = &room.modal else { panic!("back in the form") };
        assert_eq!(f.dest.value(), "/Volumes/Backup");
        // With --same-machine the OS dialog picks instead.
        let mut local = Room::new(Client::new("http://home.mstream.example:3000").unwrap(), true);
        local.apply(Done::Platform(Ok(defaults())));
        local.queued = None;
        local.apply(loaded(dests(), BackupStatus::default()));
        press(&mut local, KeyCode::Char('a'));
        ctrl(&mut local, 'b');
        assert_eq!(local.queued, Some(Op::PickNative));
        local.queued = None;
        local.apply(Done::Picked(picker::Pick::Folder("/Volumes/USB/music".into())));
        let Modal::Form(f) = &local.modal else { panic!("the form") };
        assert_eq!(f.dest.value(), "/Volumes/USB/music");
    }

    #[test]
    fn words_for_triggers_runs_and_time() {
        let _en = english();
        assert_eq!(elapsed_text(42), "42s");
        assert_eq!(elapsed_text(134), "2m 14s");
        assert_eq!(elapsed_text(3780), "1h 03m");
        let ok = run("success", 0, 812, 11_204, 3, 3_650_000_000, None);
        assert_eq!(run_summary(&ok), "812 copied, 11,204 unchanged, 3 trashed · 3.4 GB");
        let quiet = run("success", 0, 0, 0, 0, 0, None);
        assert_eq!(run_summary(&quiet), "no changes");
        let failed = run("failed", 0, 0, 0, 0, 0, None);
        assert_eq!(run_summary(&failed), "");
        assert_eq!(trigger_cell(&dest(1, "m", "/x", "daily", 1, true, None)), "daily at 03:00");
        assert_eq!(trigger_cell(&dest(1, "m", "/x", "manual", 1, true, None)), "manual only");
        assert_eq!(trigger_words("after-scan"), "after each scan");
        assert_eq!(status_word("partial"), "partial");
        assert_eq!(clip_tail("/Volumes/Backup/podcasts", 10), "…/podcasts");
        let mut f = Form::add(&[("music".into(), 10)], &defaults());
        f.throttle = Input::new("60001".into());
        assert_eq!(f.numbers().unwrap_err(), "the throttle takes a whole number of milliseconds, up to 60000");
        f.throttle = Input::new("200".into());
        f.trigger = Trigger::Daily;
        f.hour = Input::new("24".into());
        assert_eq!(f.numbers().unwrap_err(), "the hour is 0 to 23");
        f.hour = Input::new("3".into());
        assert_eq!(f.numbers().unwrap(), (30, 200, Some(3)));
    }
}
