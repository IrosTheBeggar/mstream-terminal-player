//! The Advanced options sheet (docs/ux-contracts/mp3-player-screen.md,
//! clauses 33–43; mStream docs/designs/mp3-tab, card 07, "An options
//! sheet"): the kit's neutral accent modal over the card, for the board in
//! view. Three radio groups — the firmware (this player's release, another
//! release listed from GitHub only when chosen, a local build), the flash
//! mode, an erase — then a help line and Use defaults / Apply. Nothing in
//! it writes or resets anything: Apply makes the board's next write
//! (desk::Cmd::Choose), which the card shows on one line and the gate names
//! again before the `y`.
//!
//! The sheet keeps only what is being chosen. The release list is the
//! page's for the visit ([`Listing`]); what a local path holds is the
//! worker's to read (desk::Cmd::Vet); the native dialogs run on a thread
//! and answer on a channel the page reads in its pump, so the page never
//! waits on a window.
//!
//! This file is the sheet's model: what is being chosen, its keys and the
//! dialogs. cells/sheet.rs draws it in character cells.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::crossterm::event::Event as TermEvent;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::*;
use crate::device::board::Next;
use crate::device::desk::{Choice, ListFailed, ReleaseList};
use crate::device::firmware::{Cached, ImageFacts, Images, Mode, PINNED_DIO_SHA256, PINNED_TAG, Release};
use crate::setup::picker::{self, Pick};

// ── The sheet's state ───────────────────────────────────────────────────────

/// Where the firmware comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Pin,
    Release,
    Local,
}

impl Source {
    const ALL: [Source; 3] = [Source::Pin, Source::Release, Source::Local];

    fn index(self) -> usize {
        Source::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// The sheet's keyboard stops, in Tab's order: the firmware's choices, the
/// row under the chosen one (the release control, the local choosers), the
/// flash mode, the erase box, the buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    Firmware,
    Sub,
    Mode,
    Erase,
    Buttons,
}

/// The local build's three ways in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Chooser {
    File,
    Folder,
    Type,
}

impl Chooser {
    pub(super) const ALL: [Chooser; 3] = [Chooser::File, Chooser::Folder, Chooser::Type];
}

/// A release picked from the list, or set by the flags: what the page
/// knows of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Picked {
    pub tag: String,
    /// GitHub's date, once the list said it.
    pub date: Option<String>,
    pub images: Option<Images>,
    pub pre: bool,
}

/// The local build's pick, as far as it has come.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Local {
    Nothing,
    /// Sent to the worker to be read and vetted.
    Reading(PathBuf),
    Vetted(PathBuf, ImageFacts),
    /// Not one the tab writes: why, in `--firmware`'s words.
    Refused(PathBuf, String),
}

/// The release list as the page has it this visit (clause 35): asked of
/// GitHub only when the sheet's Another release is chosen, kept once it
/// came, asked again after a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Listing {
    NotAsked,
    Asking,
    Listed(ReleaseList),
    Failed(ListFailed),
}

/// The sheet while it is up: for one board, what it would apply, and where
/// the keyboard is.
#[derive(Clone, Debug)]
pub(crate) struct Sheet {
    pub port: String,
    pub source: Source,
    pub picked: Option<Picked>,
    /// The build asked for: a release with only the other one writes that.
    pub mode: Mode,
    pub erase: bool,
    pub focus: Stop,
    /// The local choosers' keyboard cursor.
    pub chooser: Chooser,
    /// The buttons' keyboard cursor: Apply, else Use defaults.
    pub on_apply: bool,
    /// The release list hangs open, its keyboard cursor among the rows it
    /// can pick.
    pub list: Option<usize>,
    /// The path field, while it has the keyboard.
    pub field: Option<Input>,
    pub local: Local,
    /// A native dialog is up.
    pub dialog: bool,
    /// Why no dialog can open here, once one could not.
    pub no_dialog: Option<String>,
}

impl Sheet {
    /// The stops Tab walks: the row under a choice only where it has one,
    /// the flash mode only where it is a choice.
    fn stops(&self) -> Vec<Stop> {
        let mut stops = vec![Stop::Firmware];
        if self.source != Source::Pin {
            stops.push(Stop::Sub);
        }
        if self.source != Source::Local {
            stops.push(Stop::Mode);
        }
        stops.extend([Stop::Erase, Stop::Buttons]);
        stops
    }

    /// The source has `mode`'s build, as far as the page knows: the pin's
    /// DIO by its second checksum, a release by the list or the table, a
    /// local build by its own header.
    pub(super) fn offers(&self, mode: Mode) -> bool {
        match self.source {
            Source::Pin => mode == Mode::Qio || PINNED_DIO_SHA256.is_some(),
            Source::Release => self.picked.as_ref().and_then(|p| p.images).is_none_or(|i| i.has(mode)),
            Source::Local => matches!(&self.local, Local::Vetted(_, facts) if facts.mode == Some(mode)),
        }
    }

    /// The build the write would be: the one asked for where the source has
    /// it, else the one it has; a local build's own.
    pub(super) fn effective_mode(&self) -> Option<Mode> {
        match self.source {
            Source::Local => match &self.local {
                Local::Vetted(_, facts) => facts.mode,
                _ => None,
            },
            _ if self.offers(self.mode) => Some(self.mode),
            _ => [Mode::Qio, Mode::Dio].into_iter().find(|m| self.offers(*m)),
        }
    }

    /// The image Apply would choose: none while a release is not picked or
    /// a local build not vetted.
    pub(super) fn image(&self) -> Option<Image> {
        match self.source {
            Source::Pin => Some(Image::Pin(self.effective_mode()?)),
            Source::Release => {
                let picked = self.picked.as_ref()?;
                Some(Image::release(&picked.tag, self.effective_mode()?))
            }
            Source::Local => match &self.local {
                Local::Vetted(path, _) => Some(Image::Local(path.clone())),
                _ => None,
            },
        }
    }

    /// The local choosers the row offers: the dialogs where one can open.
    pub(super) fn choosers(&self) -> Vec<Chooser> {
        match self.no_dialog {
            Some(_) => vec![Chooser::Type],
            None => Chooser::ALL.to_vec(),
        }
    }
}

/// The release list's rows that can be picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Release(Release),
    /// A release already on this computer, offered while GitHub is out of
    /// reach.
    Cached(Cached),
    /// Show the pre-releases, or hide them again.
    Toggle,
    Retry,
}

// ── The dialogs and the last local path ─────────────────────────────────────

/// How the page opens the system's file and folder dialogs: the platform's
/// own (setup::picker), or under test a stand-in the test sets, so no
/// window opens on whoever runs `cargo test`.
#[derive(Clone)]
pub(crate) struct Dialogs {
    pub(crate) stub: Option<Arc<dyn Fn(Chooser) -> Pick + Send + Sync>>,
}

impl Default for Dialogs {
    fn default() -> Dialogs {
        let unavailable = |_: Chooser| Pick::Unavailable("no dialog opens under test".to_string());
        Dialogs { stub: cfg!(test).then(|| Arc::new(unavailable) as Arc<dyn Fn(Chooser) -> Pick + Send + Sync>) }
    }
}

impl Dialogs {
    /// The answer, on the thread that asks: the dialogs block until the
    /// person answers.
    fn pick(&self, chooser: Chooser, title: &str, start: Option<&Path>) -> Pick {
        if let Some(stub) = &self.stub {
            return stub(chooser);
        }
        match chooser {
            Chooser::File => picker::pick_firmware(title, start),
            Chooser::Folder => picker::pick_build_folder(title),
            Chooser::Type => Pick::Cancelled,
        }
    }
}

/// The last local path vetted: the one thing the player keeps between
/// visits (Q1), offered again in the next visit's path field. In memory,
/// for the run — never on disk. Each test thread has its own, since tests
/// run side by side.
#[cfg(not(test))]
static LAST_LOCAL: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
thread_local! {
    static LAST_LOCAL: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn remember(path: &Path) {
    #[cfg(not(test))]
    {
        *LAST_LOCAL.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.to_path_buf());
    }
    #[cfg(test)]
    LAST_LOCAL.with(|last| *last.borrow_mut() = Some(path.to_path_buf()));
}

pub(super) fn remembered() -> Option<PathBuf> {
    #[cfg(not(test))]
    {
        LAST_LOCAL.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    #[cfg(test)]
    LAST_LOCAL.with(|last| last.borrow().clone())
}

/// `~` and `~/…` (or `~\…`) as the home directory, as the kit's path field
/// expands them.
fn expand_home(path: &str) -> String {
    let rest = path.strip_prefix('~').filter(|rest| rest.is_empty() || rest.starts_with(['/', '\\']));
    if let Some(rest) = rest
        && let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        return format!("{}{rest}", home.to_string_lossy());
    }
    path.to_string()
}

/// Tab in the path field: what is typed completed to the longest start the
/// folder's entries share — folders, and the `.bin` files a build leaves —
/// read on the key's own turn, as the Torrents room's seed path is.
fn complete(input: &mut Input) {
    let raw = expand_home(input.value());
    let cut = raw.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let (dir, prefix) = raw.split_at(cut);
    let listed = if dir.is_empty() { "." } else { dir };
    let Ok(entries) = std::fs::read_dir(listed) else { return };
    let lower = prefix.to_lowercase();
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_lowercase().starts_with(&lower) {
                return None;
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                Some(format!("{name}{}", std::path::MAIN_SEPARATOR))
            } else {
                name.to_lowercase().ends_with(".bin").then_some(name)
            }
        })
        .collect();
    names.sort();
    let common = crate::setup::common_prefix(&names);
    if names.is_empty() || common.chars().count() < prefix.chars().count() {
        return;
    }
    let value = format!("{dir}{common}");
    let cursor = value.chars().count();
    *input = Input::new(value).with_cursor(cursor);
}

/// A path's last part with `…` and a separator before it, where the whole
/// would crowd a line: `…\core2`.
fn tail(path: &Path) -> String {
    match path.file_name() {
        Some(name) if path.parent().is_some_and(|p| !p.as_os_str().is_empty()) => {
            format!("…{}{}", std::path::MAIN_SEPARATOR, name.to_string_lossy())
        }
        _ => path.display().to_string(),
    }
}

/// What the player knows of a release's builds without asking GitHub.
fn known_images(tag: &str) -> Option<Images> {
    let image = Image::release(tag, Mode::Qio);
    match (image.has(Mode::Qio)?, image.has(Mode::Dio)?) {
        (true, true) => Some(Images::Both),
        (false, true) => Some(Images::DioOnly),
        (true, false) => Some(Images::QioOnly),
        (false, false) => None,
    }
}

/// A build's words with its meaning beside it: "QIO (faster)", "DIO (runs
/// on every Core2)" — never the bare name to a listener (clause 37).
pub(super) fn mode_words(mode: Option<Mode>) -> String {
    match mode {
        Some(Mode::Qio) => t!("dev.mode_qio").to_string(),
        Some(Mode::Dio) => t!("dev.mode_dio").to_string(),
        None => t!("dev.mode_unknown").to_string(),
    }
}

// ── What the sheet does ─────────────────────────────────────────────────────

impl Page {
    /// Advanced… is offered (clause 33): a board heard as one the tab can
    /// write — ours, other firmware, a blank one, one written half — with
    /// nothing under way on the page or the board.
    pub(super) fn can_advance(&self, board: &Board) -> bool {
        let heard = matches!(
            board.verdict,
            Verdict::UpToDate
                | Verdict::Update
                | Verdict::DevUpdate
                | Verdict::Newer
                | Verdict::Unplaced
                | Verdict::Other { .. }
                | Verdict::Blank
                | Verdict::HalfWritten
        );
        heard
            && self.lock.is_none()
            && self.target.is_some()
            && !self.written_now(board)
            && matches!(board.work, Work::Idle | Work::Listening)
    }

    /// Reset (`x`) is offered: a next write chosen, and nothing written.
    pub(super) fn offers_reset(&self, board: &Board) -> bool {
        self.lock.is_none() && board.pending().is_some() && !self.written_now(board)
    }

    /// `o`, or a click on Advanced…: the sheet for the board in view, preset
    /// from its next write — a choice, the flags, or the loop's cure.
    pub(super) fn open_sheet(&mut self) {
        let Some(board) = self.shown().cloned() else { return };
        if !self.can_advance(&board) {
            return;
        }
        let mut sheet = Sheet {
            port: board.port().to_string(),
            source: Source::Pin,
            picked: None,
            mode: board.default_mode(),
            erase: false,
            focus: Stop::Firmware,
            chooser: Chooser::File,
            on_apply: true,
            list: None,
            field: None,
            local: Local::Nothing,
            dialog: false,
            no_dialog: None,
        };
        let mut vet = None;
        if let Some(next) = board.pending() {
            sheet.erase = next.erase;
            match &next.image {
                Image::Pin(mode) => sheet.mode = *mode,
                Image::Release { tag, mode } => {
                    sheet.source = Source::Release;
                    sheet.mode = *mode;
                    sheet.picked = Some(self.picked_for(tag));
                }
                Image::Local(path) => {
                    sheet.source = Source::Local;
                    sheet.local = match next.facts() {
                        Some(facts) => Local::Vetted(path.clone(), facts.clone()),
                        None => {
                            vet = Some(path.clone());
                            Local::Reading(path.clone())
                        }
                    };
                }
            }
        }
        self.sheet = Some(sheet);
        if let Some(path) = vet {
            self.send(Cmd::Vet { path });
        }
    }

    /// A release by its tag, with what the list (or the table) says of it.
    fn picked_for(&self, tag: &str) -> Picked {
        if let Listing::Listed(list) = &self.listing
            && let Some(release) = list.releases.iter().find(|r| r.tag == tag)
        {
            return Picked::of(release);
        }
        Picked { tag: tag.to_string(), date: None, images: known_images(tag), pre: false }
    }

    /// Reset, Use defaults, or Apply of the defaults: the board's next write
    /// goes back to the pin in its own mode, and the busy row says so once.
    pub(super) fn reset_next(&mut self, port: &str) {
        let Some(board) = self.boards.iter().find(|b| b.port() == port).cloned() else { return };
        if board.pending().is_none() && board.next.is_none() {
            return;
        }
        if !self.send(Cmd::Choose { port: port.to_string(), choice: None }) {
            return;
        }
        let version = self.target.as_ref().map_or("?", |t| t.version.as_str()).to_string();
        let mode = mode_words(Some(board.default_mode()));
        let text = t!("dev.note_reset", port = short(port), version = version, mode = mode);
        self.say(text, false, Some(Instant::now() + NOTE_FOR));
    }

    /// Apply: the board's next write, or its defaults again. Nothing is
    /// written; the card shows the choice on one line.
    fn sheet_apply(&mut self) {
        let Some(sheet) = self.sheet.as_ref() else { return };
        let Some(image) = sheet.image() else { return self.sheet_unready() };
        let (port, erase) = (sheet.port.clone(), sheet.erase);
        self.sheet = None;
        let Some(board) = self.boards.iter().find(|b| b.port() == port).cloned() else { return };
        if !erase && image == board.default_image() {
            return self.reset_next(&port);
        }
        self.send(Cmd::Choose { port, choice: Some(Choice { image, erase }) });
    }

    /// Enter with nothing to apply yet: where the missing piece is chosen.
    fn sheet_unready(&mut self) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        match sheet.source {
            Source::Release => self.open_list(),
            Source::Local => sheet.focus = Stop::Sub,
            Source::Pin => {}
        }
    }

    fn sheet_defaults(&mut self) {
        let Some(sheet) = self.sheet.take() else { return };
        self.reset_next(&sheet.port);
    }

    /// The sheet's keys (clause 34): the path field's while it has the
    /// keyboard, the release list's while it hangs open, else the groups'.
    pub(super) fn sheet_key(&mut self, key: KeyEvent) {
        let Some(sheet) = self.sheet.as_ref() else { return };
        if sheet.field.is_some() {
            return self.field_key(key);
        }
        if sheet.list.is_some() {
            return self.list_key(key);
        }
        match key.code {
            KeyCode::Esc => self.sheet = None,
            KeyCode::Tab => self.sheet_step(1),
            KeyCode::BackTab => self.sheet_step(-1),
            KeyCode::Up => self.sheet_arrow(-1),
            KeyCode::Down => self.sheet_arrow(1),
            KeyCode::Left => self.sheet_side(-1),
            KeyCode::Right => self.sheet_side(1),
            KeyCode::Char(' ') => self.sheet_space(),
            KeyCode::Enter => self.sheet_enter(),
            _ => {}
        }
    }

    fn sheet_step(&mut self, step: isize) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        let stops = sheet.stops();
        let at = stops.iter().position(|s| *s == sheet.focus).unwrap_or(0) as isize;
        sheet.focus = stops[(at + step).rem_euclid(stops.len() as isize) as usize];
    }

    /// ↑ ↓: a radio's choice inside the focused group.
    fn sheet_arrow(&mut self, step: isize) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        match sheet.focus {
            Stop::Firmware => {
                let at = (sheet.source.index() as isize + step).clamp(0, 2) as usize;
                self.set_source(Source::ALL[at]);
            }
            Stop::Mode => {
                let mode = if step < 0 { Mode::Qio } else { Mode::Dio };
                if sheet.source != Source::Local && sheet.offers(mode) {
                    sheet.mode = mode;
                }
            }
            Stop::Sub if sheet.source == Source::Release && step > 0 => self.open_list(),
            _ => {}
        }
    }

    /// ← →: along the local choosers, and between the buttons.
    fn sheet_side(&mut self, step: isize) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        match sheet.focus {
            Stop::Sub if sheet.source == Source::Local && !sheet.dialog => {
                let choosers = sheet.choosers();
                let at = choosers.iter().position(|c| *c == sheet.chooser).unwrap_or(0) as isize;
                sheet.chooser = choosers[(at + step).clamp(0, choosers.len() as isize - 1) as usize];
            }
            Stop::Buttons => sheet.on_apply = step > 0,
            _ => {}
        }
    }

    fn sheet_space(&mut self) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        match (sheet.focus, sheet.source) {
            (Stop::Erase, _) => sheet.erase = !sheet.erase,
            (Stop::Sub | Stop::Firmware, Source::Release) => self.open_list(),
            (Stop::Sub, Source::Local) => {
                let chooser = sheet.chooser;
                self.open_chooser(chooser);
            }
            _ => {}
        }
    }

    fn sheet_enter(&mut self) {
        let Some(sheet) = self.sheet.as_ref() else { return };
        match (sheet.focus, sheet.source) {
            (Stop::Sub, Source::Release) => self.open_list(),
            (Stop::Sub, Source::Local) => {
                let chooser = sheet.chooser;
                self.open_chooser(chooser);
            }
            (Stop::Buttons, _) if !sheet.on_apply => self.sheet_defaults(),
            _ => self.sheet_apply(),
        }
    }

    /// The firmware's choice. Another release opens its list the first time
    /// it is chosen — that is when GitHub is asked (clause 35).
    fn set_source(&mut self, source: Source) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        if sheet.source == source {
            return;
        }
        sheet.source = source;
        sheet.list = None;
        if source == Source::Release && sheet.picked.is_none() {
            self.open_list();
        }
    }

    /// The release list hangs open; GitHub is asked unless the list is in
    /// hand or on its way. A failure is never kept, so it asks again.
    fn open_list(&mut self) {
        if matches!(self.listing, Listing::NotAsked | Listing::Failed(_)) && self.send(Cmd::Releases) {
            self.listing = Listing::Asking;
        }
        let entries = self.entries();
        let Some(sheet) = self.sheet.as_mut() else { return };
        let picked = sheet.picked.as_ref().map(|p| p.tag.clone());
        let at = entries.iter().position(|e| match e {
            Entry::Release(r) => Some(&r.tag) == picked.as_ref(),
            Entry::Cached(c) => Some(&c.tag) == picked.as_ref(),
            _ => false,
        });
        sheet.list = Some(at.unwrap_or(0));
    }

    /// The release the sheet measures against: the target the worker told
    /// the page — the pin, as everything on the page is measured — or,
    /// before the worker said it, the one this build carries.
    pub(super) fn pin_tag(&self) -> Option<&str> {
        self.target.as_ref().map(|t| t.version.as_str()).or(PINNED_TAG)
    }

    /// The release list's rows that can be picked, in their order: the
    /// releases (the pin left out — it is the first choice; pre-releases only
    /// once shown) and the switch for the pre-releases; offline, the
    /// releases already on this computer and Try again.
    pub(super) fn entries(&self) -> Vec<Entry> {
        let pin = self.pin_tag();
        let not_pin = |tag: &str| pin != Some(tag);
        match &self.listing {
            Listing::Listed(list) => {
                let shown = |r: &&Release| not_pin(&r.tag) && (self.show_pre || !r.pre);
                let mut entries: Vec<Entry> =
                    list.releases.iter().filter(shown).cloned().map(Entry::Release).collect();
                if list.releases.iter().any(|r| r.pre && not_pin(&r.tag)) {
                    entries.push(Entry::Toggle);
                }
                entries
            }
            Listing::Failed(failed) => {
                let mut entries: Vec<Entry> =
                    failed.cached.iter().filter(|c| not_pin(&c.tag)).cloned().map(Entry::Cached).collect();
                entries.push(Entry::Retry);
                entries
            }
            Listing::NotAsked | Listing::Asking => Vec::new(),
        }
    }

    fn list_key(&mut self, key: KeyEvent) {
        let count = self.entries().len();
        let Some(sheet) = self.sheet.as_mut() else { return };
        let Some(cursor) = sheet.list else { return };
        match key.code {
            KeyCode::Esc => sheet.list = None,
            KeyCode::Tab | KeyCode::BackTab => {
                sheet.list = None;
                self.sheet_step(if key.code == KeyCode::Tab { 1 } else { -1 });
            }
            KeyCode::Up => sheet.list = Some(cursor.saturating_sub(1)),
            KeyCode::Down => sheet.list = Some((cursor + 1).min(count.saturating_sub(1))),
            KeyCode::Enter | KeyCode::Char(' ') => self.list_pick(cursor),
            _ => {}
        }
    }

    /// A row of the list picked: a release (the list closes), the
    /// pre-releases' switch (the list stays, its cursor on the switch), Try
    /// again (GitHub asked again).
    fn list_pick(&mut self, at: usize) {
        let Some(entry) = self.entries().get(at).cloned() else { return };
        match entry {
            Entry::Release(release) => self.pick_release(Picked::of(&release)),
            Entry::Cached(cached) => {
                let images = match (cached.modes.contains(&Mode::Qio), cached.modes.contains(&Mode::Dio)) {
                    (true, true) => Some(Images::Both),
                    (false, true) => Some(Images::DioOnly),
                    (true, false) => Some(Images::QioOnly),
                    (false, false) => None,
                };
                self.pick_release(Picked { tag: cached.tag, date: None, images, pre: false });
            }
            Entry::Toggle => {
                self.show_pre = !self.show_pre;
                let toggle = self.entries().iter().position(|e| *e == Entry::Toggle);
                if let Some(sheet) = self.sheet.as_mut() {
                    sheet.list = toggle.or(sheet.list);
                }
            }
            Entry::Retry => {
                if self.send(Cmd::Releases) {
                    self.listing = Listing::Asking;
                }
                if let Some(sheet) = self.sheet.as_mut() {
                    sheet.list = Some(0);
                }
            }
        }
    }

    fn pick_release(&mut self, picked: Picked) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        sheet.source = Source::Release;
        sheet.picked = Some(picked);
        sheet.list = None;
    }

    /// A chooser opened: the path field takes the keyboard, or a dialog
    /// opens on a thread of its own.
    fn open_chooser(&mut self, chooser: Chooser) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        sheet.focus = Stop::Sub;
        sheet.chooser = chooser;
        if chooser == Chooser::Type || sheet.no_dialog.is_some() {
            sheet.chooser = Chooser::Type;
            let start = remembered().map(|p| p.display().to_string()).unwrap_or_default();
            let cursor = start.chars().count();
            sheet.field = Some(Input::new(start).with_cursor(cursor));
            return;
        }
        if sheet.dialog {
            return;
        }
        let title = t!(match chooser {
            Chooser::Folder => "dev.adv_dialog_folder",
            _ => "dev.adv_dialog_file",
        })
        .to_string();
        let start = remembered().and_then(|p| if p.is_dir() { Some(p) } else { p.parent().map(Path::to_path_buf) });
        let (picks, dialogs) = (self.picks.0.clone(), self.dialogs.clone());
        let spawned = std::thread::Builder::new().name("mstream-dialog".to_string()).spawn(move || {
            let _ = picks.send(dialogs.pick(chooser, &title, start.as_deref()));
        });
        sheet.dialog = spawned.is_ok();
    }

    /// A dialog's answer: a path to vet; a dialog that cannot open here
    /// hands the keyboard to the path field (clause 36).
    pub(super) fn picked(&mut self, pick: Pick) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        sheet.dialog = false;
        match pick {
            Pick::File(path) | Pick::Folder(path) => self.vet(path),
            Pick::Cancelled => {}
            Pick::Unavailable(why) => {
                sheet.no_dialog = Some(why);
                self.open_chooser(Chooser::Type);
            }
        }
    }

    /// The path field's keys: the line editor's, Tab completing, Enter
    /// reading what is typed, Esc giving the keys back.
    fn field_key(&mut self, key: KeyEvent) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        let Some(field) = sheet.field.as_mut() else { return };
        match key.code {
            KeyCode::Esc => sheet.field = None,
            KeyCode::Tab => complete(field),
            KeyCode::Enter => {
                let typed = expand_home(field.value().trim());
                if typed.is_empty() {
                    return;
                }
                sheet.field = None;
                self.vet(PathBuf::from(typed));
            }
            _ => {
                field.handle_event(&TermEvent::Key(key));
            }
        }
    }

    /// A local path picked: read and vetted on the worker at once.
    fn vet(&mut self, path: PathBuf) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        sheet.source = Source::Local;
        sheet.local = Local::Reading(path.clone());
        self.send(Cmd::Vet { path });
    }

    /// What the worker read of the path the sheet waits for; any other
    /// answer is an old one.
    pub(super) fn vetted(&mut self, path: PathBuf, result: Result<ImageFacts, DeviceError>) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        if !matches!(&sheet.local, Local::Reading(waiting) if *waiting == path) {
            return;
        }
        sheet.local = match result {
            Ok(facts) => {
                remember(&path);
                Local::Vetted(path, facts)
            }
            Err(e) => {
                let words = e.text().replace(&path.display().to_string(), &tail(&path));
                Local::Refused(path, words)
            }
        };
    }

    /// The release list came, or did not.
    pub(super) fn listed(&mut self, result: Result<ReleaseList, ListFailed>) {
        self.listing = match result {
            Ok(list) => Listing::Listed(list),
            Err(failed) => Listing::Failed(failed),
        };
        let count = self.entries().len();
        if let Some(sheet) = self.sheet.as_mut()
            && let Some(cursor) = sheet.list
        {
            sheet.list = Some(cursor.min(count.saturating_sub(1)));
        }
    }

    /// A click in the sheet.
    pub(super) fn sheet_act(&mut self, act: Act) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        match act {
            Act::SheetClose => self.sheet = None,
            Act::SheetSource(at) => {
                sheet.focus = Stop::Firmware;
                if let Some(source) = Source::ALL.get(at) {
                    self.set_source(*source);
                }
            }
            Act::SheetMode(mode) => {
                sheet.focus = Stop::Mode;
                if sheet.source != Source::Local && sheet.offers(mode) {
                    sheet.mode = mode;
                }
            }
            Act::SheetErase => {
                sheet.focus = Stop::Erase;
                sheet.erase = !sheet.erase;
            }
            Act::ListOpen => {
                sheet.focus = Stop::Sub;
                self.open_list();
            }
            Act::ListClose => sheet.list = None,
            Act::ListPick(at) => self.list_pick(at),
            Act::SheetChooser(at) => {
                if let Some(chooser) = Chooser::ALL.get(at) {
                    self.open_chooser(*chooser);
                }
            }
            Act::Defaults => self.sheet_defaults(),
            Act::Apply => self.sheet_apply(),
            _ => {}
        }
    }

    /// The footer while the sheet is up (clause 34): the keys of the focused
    /// group, Enter and Esc never giving way.
    pub(super) fn sheet_hint_parts(&self) -> Vec<(String, u8)> {
        let g = glyphs();
        let Some(sheet) = &self.sheet else { return Vec::new() };
        let choose = |keys: &str| format!("{keys} {}", t!("dev.hint_sheet_choose"));
        let hint = |key: &str| t!(key).to_string();
        if sheet.field.is_some() {
            let (complete, read) = (hint("dev.hint_field_complete"), hint("dev.hint_field_read"));
            return vec![(complete, 1), (read, 0), (hint("dev.hint_field_back"), 0)];
        }
        if sheet.list.is_some() {
            if matches!(self.listing, Listing::NotAsked | Listing::Asking) {
                return vec![(hint("dev.hint_list_close"), 0)];
            }
            return vec![(choose(g.updown), 1), (hint("dev.hint_list_pick"), 0), (hint("dev.hint_list_close"), 0)];
        }
        match (sheet.focus, sheet.source) {
            (Stop::Sub, Source::Local) => vec![
                (choose(g.switch), 1),
                (hint("dev.hint_sheet_open"), 0),
                (hint("dev.hint_sheet_tab"), 1),
                (hint("dev.hint_sheet_close"), 0),
            ],
            (Stop::Buttons, _) => {
                let enter = if sheet.on_apply { "dev.hint_sheet_apply" } else { "dev.hint_sheet_defaults" };
                let (tab, close) = (hint("dev.hint_sheet_tab"), hint("dev.hint_sheet_close"));
                vec![(choose(g.switch), 1), (hint(enter), 0), (tab, 1), (close, 0)]
            }
            _ => vec![
                (hint("dev.hint_sheet_tab"), 1),
                (choose(g.updown), 1),
                (hint("dev.hint_sheet_tick"), 2),
                (hint("dev.hint_sheet_apply"), 0),
                (hint("dev.hint_sheet_close"), 0),
            ],
        }
    }

    /// The path field has the keyboard: every key is the field's.
    pub(super) fn typing(&self) -> bool {
        self.sheet.as_ref().is_some_and(|s| s.field.is_some())
    }
}

impl Picked {
    fn of(release: &Release) -> Picked {
        Picked {
            tag: release.tag.clone(),
            date: (!release.date.is_empty()).then(|| release.date.clone()),
            images: Some(release.images),
            pre: release.pre,
        }
    }
}

/// A next write's source in a few words, for Update all's gate and the
/// card: `v0.7.0`, `local build v0.8.0-5-g4e94418`.
pub(super) fn next_what(next: &Next) -> String {
    match &next.image {
        Image::Local(path) => {
            let version = next.version().unwrap_or_else(|| tail(path));
            t!("dev.next_local", version = version).to_string()
        }
        _ => next.version().unwrap_or_else(|| "?".to_string()),
    }
}

/// A local build's path as the card and the gate show it.
pub(super) fn local_tail(path: &Path) -> String {
    tail(path)
}
