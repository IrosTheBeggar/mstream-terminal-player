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
//!
//! This file is the room itself: its state, its worker, its keys and
//! every word it says. It names no drawing type of ratatui's or the
//! kit's. Its fields are edited through tui_input, so it keeps that
//! crate's `Input` and the crossterm key event the editor reads; a key
//! comes in as crate::input's KeyEvent. The room's views are child
//! modules that read it the same way: `cells` (cells.rs) draws it in
//! character cells, for a terminal and for the GUI's grid, and a view
//! in pixels would sit beside it. A click in a view is one of the
//! room's [`Act`]s and a key is [`Room::key`], so views cannot disagree
//! about what the room does. What a view keeps between frames it keeps
//! in a type of its own, held here unopened (`cells::State`).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::Event as TermEvent;
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{Outcome, ServerHome, fmt_bytes, fmt_count, gate_message, printable};
use crate::api::types::{BackupDestination, BackupRun, BackupStatus, DirListing, PathCheck, RunAnswer};
use crate::api::{ApiError, BackupPatch, Client, NewBackupDestination};
use crate::input::{KeyCode, KeyEvent, KeyModifiers};
use crate::setup::{Browse, join_server_path, parent_server_path, picker};

/// The room in character cells: the terminal's room and the GUI's grid.
pub(crate) mod cells;
/// The same room drawn by egui, in the desktop window's panel.
#[cfg(feature = "window")]
pub(crate) mod egui_view;

/// The page's cadences: two seconds with a run in flight, five otherwise.
const POLL_ACTIVE: Duration = Duration::from_secs(2);
const POLL_IDLE: Duration = Duration::from_secs(5);
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
        // A typed `~` destination is the server user's home: learned once,
        // expanded here before the check and the save — the backup routes
        // insist on an absolute path.
        let mut home = ServerHome::new();
        while let Ok((client, op)) = job_rx.recv() {
            let done = match op {
                // The three reads go out together — one round trip a poll,
                // not three, so an op a key queues waits behind one at most
                // (performance audit #83) — and are read in the order they
                // were once asked: a destinations error wins.
                Op::Load => Done::Loaded(crate::api::wait(async {
                    let (dests, status, libraries) = tokio::join!(
                        client.admin_backup_destinations_async(),
                        client.admin_backup_status_async(),
                        client.admin_directories_async(),
                    );
                    let (dests, status) = (dests?, status?);
                    // The library names → ids, for the add form; best-effort.
                    let libraries = libraries
                        .ok()
                        .map(|dirs| dirs.into_iter().map(|(name, d)| (name, d.id)).collect());
                    Ok(Box::new(Loaded { dests, status, libraries }))
                })),
                Op::Platform => Done::Platform(client.admin_backup_platform().map(|p| p.default_excludes)),
                Op::Check { library_id, dest, exclude_dest_id } => {
                    let result = home
                        .expand(&client, &dest)
                        .and_then(|path| client.admin_backup_check_path(library_id, &path, exclude_dest_id));
                    Done::Checked { dest, result }
                }
                Op::Add(mut dest) => Done::Added(
                    home.expand(&client, &dest.dest_path)
                        .and_then(|path| {
                            dest.dest_path = path;
                            client.admin_backup_add(&dest)
                        })
                        .map(|_| ()),
                ),
                Op::Patch { id, mut patch, what } => {
                    let expanded = match patch.dest_path.as_deref() {
                        Some(path) => home.expand(&client, path).map(Some),
                        None => Ok(None),
                    };
                    let result = expanded
                        .and_then(|path| {
                            if path.is_some() {
                                patch.dest_path = path;
                            }
                            client.admin_backup_patch(id, &patch)
                        })
                        .map(|_| ());
                    Done::Patched { id, what, result }
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
    last_load: Option<Instant>,
    /// What the cells view keeps here between frames (cells.rs): its
    /// surface, and the cursor the table last followed.
    cells: cells::State,
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
            last_load: None,
            cells: cells::State::new(),
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

    /// A key press: the room's keymap, the same under any view.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        handle_key(self, key)
    }

    /// A click, drag or hold a view resolved to one of the room's actions.
    pub(crate) fn act(&mut self, act: Act) -> Option<Outcome> {
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
            Done::Picked(picker::Pick::Cancelled | picker::Pick::File(_)) => {}
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

    /// The page's poll, quiet: two seconds with a run in flight or tasks
    /// queued, five otherwise — never on top of a call already queued or
    /// running.
    pub(crate) fn tick(&mut self) {
        let every = if self.status.active.is_some() || self.status.queue_length > 0 { POLL_ACTIVE } else { POLL_IDLE };
        if self.loaded && !self.in_flight && self.queued.is_none() && self.last_load.is_none_or(|t| t.elapsed() >= every) {
            self.reload(false);
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

// ── The tips ─────────────────────────────────────────────────────────────────

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
    use super::super::unix_now;
    use super::*;

    // The fixtures are the cells view's tests' too (cells.rs).

    /// English strings under assertion: hold the wizard tests' locale lock
    /// (one of them flips the process-global locale) and pin English.
    pub(super) fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    pub(super) fn new_room() -> Room {
        Room::new(Client::new("http://home.mstream.example:3000").expect("client"), false)
    }

    /// SQLite's UTC form, as the rows carry it.
    pub(super) fn sqlite_at(t: i64) -> String {
        super::super::iso_at(t).replace('T', " ")[..19].to_string()
    }

    const DEFAULTS: [&str; 4] = ["Thumbs.db", "desktop.ini", ".DS_Store", "._*"];

    pub(super) fn defaults() -> Vec<String> {
        DEFAULTS.iter().map(|s| s.to_string()).collect()
    }

    pub(super) fn run(status: &str, ago: i64, copied: u64, unchanged: u64, trashed: u64, bytes: u64, error: Option<&str>) -> BackupRun {
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

    pub(super) fn dest(id: i64, library: &str, path: &str, trigger: &str, retention: u32, enabled: bool, last: Option<BackupRun>) -> BackupDestination {
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

    pub(super) fn dests() -> Vec<BackupDestination> {
        vec![
            dest(1, "music", "/Volumes/Backup/music", "after-scan", 30, true, Some(run("success", 2 * 3600, 812, 11_204, 3, 3_650_000_000, None))),
            dest(2, "podcasts", "/Volumes/Backup/podcasts", "daily", 30, true, Some(run("failed", 86_400, 0, 0, 0, 0, Some("EACCES: permission denied, mkdir '/Volumes/Backup/podcasts/.mstream-trash'")))),
            dest(3, "vinyl", "/mnt/usb/vinyl", "manual", 0, false, None),
        ]
    }

    pub(super) fn loaded(dests: Vec<BackupDestination>, status: BackupStatus) -> Done {
        Done::Loaded(Ok(Box::new(Loaded {
            dests,
            status,
            libraries: Some(vec![("music".into(), 10), ("podcasts".into(), 20), ("vinyl".into(), 30)]),
        })))
    }

    pub(super) fn idle() -> Room {
        let mut room = new_room();
        room.apply(Done::Platform(Ok(defaults())));
        room.queued = None;
        room.busy = None;
        room.apply(loaded(dests(), BackupStatus::default()));
        room
    }

    pub(super) fn key_press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub(super) fn press(room: &mut Room, code: KeyCode) -> Option<Outcome> {
        handle_key(room, key_press(code))
    }

    pub(super) fn ctrl(room: &mut Room, c: char) -> Option<Outcome> {
        handle_key(room, KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    pub(super) fn type_text(room: &mut Room, text: &str) {
        for c in text.chars() {
            press(room, KeyCode::Char(c));
        }
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

    #[test]
    fn a_poll_asks_its_three_reads_together() {
        // The destinations, the status and the libraries at once: one
        // round trip a poll, not three (performance audit #83) — and a
        // destinations error still wins over the status's.
        let run = |url: &str| {
            let (to_worker, from_worker) = spawn_worker();
            to_worker.send((Arc::new(Client::new(url).expect("client")), Op::Load)).unwrap();
            let Done::Loaded(result) = from_worker.recv_timeout(Duration::from_secs(30)).expect("an answer") else {
                panic!("a load");
            };
            result
        };
        let server = super::super::waves::serve(&[3], |_| (200, "{}".into()));
        let loaded = run(&server.url).expect("a load");
        assert_eq!((server.peak(), server.paths().len()), (3, 3));
        assert!(loaded.libraries.is_some());
        let server = super::super::waves::serve(&[3], |path| match path {
            "/api/v1/admin/backup/destinations" => (404, "{}".into()),
            _ => (500, "{}".into()),
        });
        assert!(matches!(run(&server.url), Err(ApiError::NotFound(_))));
    }
}
