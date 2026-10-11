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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::crossterm::event::Event as TermEvent;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::*;
use crate::device::board::Next;
use crate::device::desk::{Choice, ListFailed, ReleaseList};
use crate::device::firmware::{Cached, ImageFacts, Images, Mode, PINNED_DIO_SHA256, PINNED_TAG, Release};
use crate::device::firmware::{clock_at, header_mhz};
use crate::setup::picker::{self, Pick};

/// The sheet's rows in its tallest state — a local build vetted, its help
/// shown — so its top is held where that state would put it and a row
/// appearing under a choice never moves it (clause 34).
pub(super) const SHEET_MAX: u16 = 23;
/// Where a choice's own lines start: under its name, past the radio.
const SUB_X: u16 = 6;
/// The help's lines, at most.
const HELP_LINES: usize = 2;

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
    const ALL: [Chooser; 3] = [Chooser::File, Chooser::Folder, Chooser::Type];
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
    fn offers(&self, mode: Mode) -> bool {
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
    fn choosers(&self) -> Vec<Chooser> {
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

/// `text` cut at its front to `width` cells, `…` first: a path whose end is
/// what tells it apart.
fn fit_front(text: &str, width: usize) -> String {
    if kit::width(text) <= width {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = kit::width(c.encode_utf8(&mut [0u8; 4]));
        if used + w > width {
            break;
        }
        kept.push(c);
        used += w;
    }
    kept.reverse();
    format!("…{}", kept.into_iter().collect::<String>())
}

/// `2026-10-07` for a moment, by the calendar (UTC: the release list's
/// dates are GitHub's, which are too).
fn date_of(at: std::time::SystemTime) -> Option<String> {
    let secs = at.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    let (y, m, d) = crate::admin::tz::civil_from_days(secs.div_euclid(86_400));
    Some(format!("{y:04}-{m:02}-{d:02}"))
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

fn images_words(images: Images) -> String {
    t!(match images {
        Images::Both => "dev.adv_images_both",
        Images::DioOnly => "dev.adv_images_dio",
        Images::QioOnly => "dev.adv_images_qio",
    })
    .to_string()
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

    /// The release list's rows that can be picked, in their order: the
    /// releases (the pin left out — it is the first choice; pre-releases only
    /// once shown) and the switch for the pre-releases; offline, the
    /// releases already on this computer and Try again.
    pub(super) fn entries(&self) -> Vec<Entry> {
        let not_pin = |tag: &str| PINNED_TAG != Some(tag);
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

// ── Drawing ─────────────────────────────────────────────────────────────────

/// One line of the sheet, laid out before it is drawn so the sheet knows
/// its height and what gives way at the floor.
enum SheetLine {
    Title,
    /// A blank that gives way, after the help, when rows run short.
    Blank,
    /// A group's dim uppercase label.
    Label(String),
    Radio(Radio),
    /// A dim line under a choice, at the choice's name.
    Sub(Vec<(String, Style)>),
    Choosers,
    Field,
    Erase,
    /// The help's rule and its lines: the first to give way.
    Rule,
    Help(Line<'static>),
    Buttons,
}

/// A radio row: `(•) name — description`, or with the release control
/// after the name.
struct Radio {
    on: bool,
    /// Its group has the keyboard.
    focused: bool,
    /// A choice the source has (drawn dim when not).
    usable: bool,
    name: String,
    desc: Vec<(String, Style)>,
    /// The release control: its words, and whether the keyboard is on it.
    control: Option<(String, bool)>,
    /// The description is a path, cut at its front where it is too long.
    front: bool,
    act: Option<Act>,
}

/// The kit's slab for a keyboard-selected row.
fn slab() -> Style {
    Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD)
}

/// The sheet's lines for `board`, `w` cells wide.
fn sheet_lines(page: &Page, sheet: &Sheet, board: &Board, w: u16) -> Vec<SheetLine> {
    let g = glyphs();
    let pin = page.target.as_ref().map_or("?", |t| t.version.as_str()).to_string();
    let dim_text = |text: String| vec![(text, dim())];
    let in_list = sheet.list.is_some();
    let focus = |stop: Stop| sheet.focus == stop && !in_list && sheet.field.is_none();
    let firmware = SheetLine::Label(t!("dev.adv_group_firmware").to_uppercase());
    let mut lines = vec![SheetLine::Title, SheetLine::Blank, firmware];
    let source = sheet.source;
    lines.push(SheetLine::Radio(Radio {
        on: source == Source::Pin,
        focused: focus(Stop::Firmware),
        usable: true,
        name: pin.clone(),
        desc: dim_text(t!("dev.adv_pin_desc").to_string()),
        control: None,
        front: false,
        act: Some(Act::SheetSource(0)),
    }));
    let control = (source == Source::Release).then(|| {
        let value = sheet.picked.as_ref().map_or_else(|| t!("dev.adv_release_pick").to_string(), |p| p.tag.clone());
        (format!("{value} {}", g.open), focus(Stop::Sub))
    });
    let release_desc = if control.is_some() { Vec::new() } else { dim_text(t!("dev.adv_release_desc").to_string()) };
    lines.push(SheetLine::Radio(Radio {
        on: source == Source::Release,
        focused: focus(Stop::Firmware),
        usable: true,
        name: t!("dev.adv_release").to_string(),
        desc: release_desc,
        control,
        front: false,
        act: Some(Act::SheetSource(1)),
    }));
    if source == Source::Release
        && let Some(picked) = &sheet.picked
    {
        lines.push(SheetLine::Sub(dim_text(release_facts(page, board, picked))));
    }
    let local_path = match &sheet.local {
        Local::Reading(path) | Local::Vetted(path, _) | Local::Refused(path, _) if source == Source::Local => {
            Some(path.display().to_string())
        }
        _ => None,
    };
    let front = local_path.is_some();
    let local_desc = match local_path {
        Some(path) => vec![(path, Style::default())],
        None => dim_text(t!("dev.adv_local_desc").to_string()),
    };
    lines.push(SheetLine::Radio(Radio {
        on: source == Source::Local,
        focused: focus(Stop::Firmware),
        usable: true,
        name: t!("dev.adv_local").to_string(),
        desc: local_desc,
        control: None,
        front,
        act: Some(Act::SheetSource(2)),
    }));
    if source == Source::Local {
        let sub_w = usize::from(w.saturating_sub(SUB_X));
        match &sheet.local {
            Local::Vetted(_, facts) => {
                let file = facts.file.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().to_string());
                let file = file.unwrap_or_default();
                let bytes = fmt_count(facts.bytes as u64);
                let words = t!("dev.adv_local_facts", file = file, version = facts.version, bytes = bytes);
                lines.push(SheetLine::Sub(dim_text(words.to_string())));
                let mode = facts.mode.map_or_else(|| t!("dev.mode_unknown").to_string(), |m| m.word().to_string());
                lines.push(SheetLine::Sub(dim_text(t!("dev.adv_local_kind", mode = mode).to_string())));
            }
            Local::Refused(_, why) => {
                let gold = Style::default().fg(th().gold);
                let wrapped = wrap_spans(&[(format!("{} {why}", g.no), gold)], sub_w);
                for line in wrapped.into_iter().take(2) {
                    let pieces = line.spans.into_iter().map(|s| (s.content.to_string(), s.style)).collect();
                    lines.push(SheetLine::Sub(pieces));
                }
            }
            Local::Reading(_) => lines.push(SheetLine::Sub(dim_text(t!("dev.adv_local_reading").to_string()))),
            Local::Nothing => {}
        }
        lines.push(if sheet.field.is_some() { SheetLine::Field } else { SheetLine::Choosers });
    }
    lines.push(SheetLine::Blank);
    lines.push(SheetLine::Label(t!("dev.adv_group_mode").to_uppercase()));
    lines.extend(mode_lines(page, sheet, board, focus(Stop::Mode)));
    lines.push(SheetLine::Blank);
    lines.push(SheetLine::Label(t!("dev.adv_group_erase").to_uppercase()));
    lines.push(SheetLine::Erase);
    lines.push(SheetLine::Rule);
    let help = wrap_spans(&[(help_text(page, sheet), dim())], usize::from(w));
    let more = help.len() > HELP_LINES;
    for (i, line) in help.into_iter().take(HELP_LINES).enumerate() {
        // A help too long for its two lines ends in an ellipsis.
        let line = if more && i + 1 == HELP_LINES {
            Line::from(Span::styled(fit(&format!("{line} …"), usize::from(w)), dim()))
        } else {
            line
        };
        lines.push(SheetLine::Help(line));
    }
    lines.push(SheetLine::Blank);
    lines.push(SheetLine::Buttons);
    lines
}

/// The Flash mode group's rows (clause 34): the pin and a release offer
/// the builds they have, the board's own or else its default marked; a
/// local build's is its own.
fn mode_lines(page: &Page, sheet: &Sheet, board: &Board, focused: bool) -> Vec<SheetLine> {
    let port = short(board.port());
    let runs = board.mode().map(|m| m.mode).filter(|_| board.ours());
    let mut lines = Vec::new();
    let mut note = None;
    for mode in [Mode::Qio, Mode::Dio] {
        let base = t!(if mode == Mode::Qio { "dev.adv_qio_desc" } else { "dev.adv_dio_desc" }).to_string();
        let (on, usable, desc) = match sheet.source {
            Source::Pin | Source::Release => {
                let usable = sheet.offers(mode);
                let desc = if usable {
                    // One mark a row, and DIO's meaning short beside it, so
                    // the row fits the console's sheet in every language:
                    // the mode the board runs now — its default as well,
                    // but on v0.5.0's one image, whose QIO row then says
                    // so — else the default.
                    let mark = if runs == Some(mode) {
                        Some(t!("dev.adv_as_runs", port = port))
                    } else if board.default_mode() == mode {
                        Some(t!("dev.adv_default"))
                    } else {
                        None
                    };
                    match mark {
                        Some(mark) if mode == Mode::Dio => format!("{}{mark}", t!("dev.adv_dio_short")),
                        Some(mark) => format!("{base}{mark}"),
                        None => base,
                    }
                } else {
                    let version = match (&sheet.picked, sheet.source) {
                        (Some(picked), Source::Release) => picked.tag.clone(),
                        _ => page.target.as_ref().map_or("?", |t| t.version.as_str()).to_string(),
                    };
                    t!("dev.adv_mode_fixed", version = version).to_string()
                };
                (sheet.effective_mode() == Some(mode), usable, desc)
            }
            Source::Local => match &sheet.local {
                Local::Vetted(_, facts) if facts.mode == Some(mode) => {
                    let mhz = facts.header.and_then(|h| header_mhz(&h)).map_or("?".to_string(), |m| m.to_string());
                    (true, true, t!("dev.adv_mode_own", mhz = mhz).to_string())
                }
                Local::Vetted(_, facts) if facts.mode.is_some() => {
                    (false, false, t!("dev.adv_mode_not_own", mode = mode.word()).to_string())
                }
                Local::Vetted(..) => {
                    note = Some(t!("dev.mode_unknown").to_string());
                    (false, false, base)
                }
                _ => {
                    note = Some(t!("dev.adv_mode_later").to_string());
                    (false, false, base)
                }
            },
        };
        let act = (sheet.source != Source::Local && usable).then_some(Act::SheetMode(mode));
        let desc = vec![(desc, dim())];
        let name = mode.word().to_string();
        lines.push(SheetLine::Radio(Radio { on, focused, usable, name, desc, control: None, front: false, act }));
    }
    if let Some(note) = note {
        lines.push(SheetLine::Sub(vec![(note, dim())]));
    }
    lines
}

/// The line under a picked release: its date, its builds, and its
/// direction against this board (clause 35).
fn release_facts(page: &Page, board: &Board, picked: &Picked) -> String {
    let port = short(board.port());
    let pin = page.target.as_ref().map(|t| t.version.clone());
    let newer = pin.as_deref().is_some_and(|pin| place(pin, &picked.tag) == Place::Older);
    let direction = if newer {
        Some(t!("dev.adv_newer").to_string())
    } else {
        board.version().and_then(|version| match place(version, &picked.tag) {
            Place::Older => Some(t!("dev.adv_update_from", version = version).to_string()),
            Place::Newer => Some(t!("dev.adv_step_back", port = port, version = version).to_string()),
            Place::Same => Some(t!("dev.adv_same", port = port).to_string()),
            Place::Unknown => None,
        })
    };
    let mut parts: Vec<String> = picked.date.iter().cloned().collect();
    if let Some(images) = picked.images {
        let mut words = images_words(images);
        if picked.pre {
            words = format!("{}, {words}", t!("dev.adv_pre"));
        }
        parts.push(words);
    }
    parts.extend(direction);
    parts.join(" · ")
}

/// The help line's words, about the focused group (clause 34).
fn help_text(page: &Page, sheet: &Sheet) -> String {
    let pin = page.target.as_ref().map_or("?", |t| t.version.as_str()).to_string();
    match (sheet.focus, sheet.source) {
        (Stop::Mode, _) if sheet.effective_mode() == Some(Mode::Dio) => t!("dev.adv_help_dio").to_string(),
        (Stop::Mode, _) => t!("dev.adv_help_qio").to_string(),
        (Stop::Erase, _) => t!("dev.adv_help_erase").to_string(),
        (_, Source::Pin) => t!("dev.adv_help_pin", pin = pin).to_string(),
        (_, Source::Release) => match &sheet.picked {
            Some(picked) if sheet.list.is_none() => {
                t!("dev.adv_help_picked", tag = picked.tag, pin = pin).to_string()
            }
            _ => t!("dev.adv_help_release").to_string(),
        },
        (_, Source::Local) => match &sheet.local {
            Local::Vetted(..) => t!("dev.adv_help_local_picked").to_string(),
            Local::Refused(..) => t!("dev.adv_help_refused").to_string(),
            _ => t!("dev.adv_help_local").to_string(),
        },
    }
}

/// The sheet over the card, centred in `area` (clause 34): the kit's
/// neutral modal, its top held for its tallest state. Short of rows the
/// help gives way first, then its blank lines from the end.
pub(super) fn draw_sheet(frame: &mut Frame, page: &mut Page, area: Rect) {
    let Some(sheet) = page.sheet.clone() else { return };
    let Some(board) = page.boards.iter().find(|b| b.port() == sheet.port).cloned() else { return };
    let width = GATE_W.min(area.width.saturating_sub(4));
    if width < 24 || area.height < 8 {
        return;
    }
    let text_w = width.saturating_sub(4);
    let mut lines = sheet_lines(page, &sheet, &board, text_w);
    let most = usize::from(area.height.saturating_sub(2)).saturating_sub(2);
    if lines.len() > most {
        lines.retain(|l| !matches!(l, SheetLine::Rule | SheetLine::Help(_)));
    }
    while lines.len() > most {
        let Some(at) = lines.iter().rposition(|l| matches!(l, SheetLine::Blank)) else { break };
        lines.remove(at);
    }
    if lines.len() > most {
        // Still short: the rows before the buttons are cut, never the
        // buttons themselves.
        let buttons = lines.pop();
        lines.truncate(most.saturating_sub(1));
        lines.extend(buttons);
    }
    let height = (lines.len() + 2) as u16;
    let inner = kit::modal_frame_anchored_on(frame, &mut page.ui, area, width, height, SHEET_MAX, th().accent);
    if inner.height == 0 || inner.width < 4 {
        return;
    }
    let content = Rect { x: inner.x + 1, y: inner.y, width: inner.width - 2, height: inner.height };
    let mut anchor = None;
    for (i, line) in lines.iter().enumerate() {
        let y = content.y + i as u16;
        if y >= content.bottom() {
            break;
        }
        let row = Rect { y, height: 1, ..content };
        match line {
            SheetLine::Title => {
                let title = t!("dev.adv_title", port = short(&sheet.port)).to_string();
                let style = accent().add_modifier(Modifier::BOLD);
                let at = Rect { width: row.width.saturating_sub(4), ..row };
                frame.render_widget(Paragraph::new(Span::styled(fit(&title, usize::from(at.width)), style)), at);
                kit::modal_close(frame, &mut page.ui, inner, Act::SheetClose);
            }
            SheetLine::Blank => {}
            SheetLine::Label(label) => {
                frame.render_widget(Paragraph::new(Span::styled(fit(label, usize::from(row.width)), dim())), row);
            }
            SheetLine::Radio(radio) => {
                if let Some(rect) = draw_radio(frame, page, row, radio) {
                    anchor = Some(rect);
                }
            }
            SheetLine::Sub(pieces) => {
                let at = Rect { x: row.x + SUB_X, width: row.width.saturating_sub(SUB_X), ..row };
                frame.render_widget(Paragraph::new(fit_line(pieces, usize::from(at.width))), at);
            }
            SheetLine::Choosers => draw_choosers(frame, page, &sheet, row),
            SheetLine::Field => {
                let at = Rect { x: row.x + SUB_X, width: row.width.saturating_sub(SUB_X), ..row };
                if let Some(field) = &sheet.field {
                    let (value, cursor) = (field.value(), field.cursor());
                    let shown = kit::field_display(&mut page.ui, at.x, at.y, value, cursor, at.width, None);
                    frame.render_widget(Paragraph::new(Span::styled(shown, accent())), at);
                }
            }
            SheetLine::Erase => draw_erase(frame, page, &sheet, row),
            SheetLine::Rule => {
                let rule = glyphs().rule.repeat(usize::from(row.width));
                frame.render_widget(Paragraph::new(Span::styled(rule, dim())), row);
            }
            SheetLine::Help(line) => frame.render_widget(Paragraph::new(line.clone()), row),
            // The buttons in the frame's own width, as the gate's: the
            // write's two cells from the border.
            SheetLine::Buttons => {
                let at = Rect { x: inner.x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 };
                draw_buttons(frame, page, &sheet, at);
            }
        }
    }
    if sheet.list.is_some() {
        let anchor = anchor.unwrap_or(Rect { x: content.x + 20, y: content.y + 4, width: 1, height: 1 });
        draw_list(frame, page, &sheet, area, content, anchor);
    }
}

/// `pieces` on one line, cut at `width` cells with an ellipsis.
fn fit_line(pieces: &[(String, Style)], width: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut used = 0;
    for (text, style) in pieces {
        let cells = kit::width(text);
        if used + cells <= width {
            spans.push(Span::styled(text.clone(), *style));
            used += cells;
            continue;
        }
        spans.push(Span::styled(fit(text, width.saturating_sub(used)), *style));
        break;
    }
    Line::from(spans)
}

/// The words of the sheet's radio rows as they are meant to read whole —
/// the name, ` — `, the description — but a path's, which is cut at its
/// front by design, and the release control's: what a test finds in the
/// frame, or the row was cut. None while the release list hangs over them.
#[cfg(test)]
pub(super) fn radio_words(page: &Page) -> Vec<String> {
    let Some(sheet) = page.sheet.clone().filter(|s| s.list.is_none()) else { return Vec::new() };
    let Some(board) = page.boards.iter().find(|b| b.port() == sheet.port).cloned() else { return Vec::new() };
    let mut words = Vec::new();
    for line in sheet_lines(page, &sheet, &board, GATE_W) {
        if let SheetLine::Radio(radio) = line
            && !radio.front
            && radio.control.is_none()
        {
            let desc: String = radio.desc.iter().map(|(text, _)| text.as_str()).collect();
            words.push(if desc.is_empty() { radio.name } else { format!("{} — {desc}", radio.name) });
        }
    }
    words
}

/// A radio row (the kit's: `(•)` in the accent on the focused group's
/// choice, the chosen name bold, a dim description); returns the release
/// control's rect, where the list hangs from.
fn draw_radio(frame: &mut Frame, page: &mut Page, row: Rect, radio: &Radio) -> Option<Rect> {
    let g = glyphs();
    let at = Rect { x: row.x + 2, width: row.width.saturating_sub(2), ..row };
    let hovered = radio.act.is_some() && page.ui.hovers(at);
    let glyph_style = match (radio.on, radio.focused) {
        (true, true) => accent(),
        (true, false) => Style::default(),
        _ => dim(),
    };
    let name_style = if !radio.usable {
        dim()
    } else if hovered {
        hover()
    } else if radio.on {
        bold()
    } else {
        Style::default()
    };
    let glyph = if radio.on { g.radio_on } else { "( )" };
    let mut pieces = vec![(format!("{glyph} "), glyph_style), (radio.name.clone(), name_style)];
    let mut control = None;
    if let Some((words, focused)) = &radio.control {
        let x = at.x + kit::width(&format!("{glyph} {}  ", radio.name)) as u16;
        let width = (kit::width(words) as u16).min(at.right().saturating_sub(x));
        let rect = Rect { x, y: row.y, width, height: 1 };
        let style = if page.ui.hovers(rect) {
            hover()
        } else if *focused {
            accent().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        pieces.push(("  ".to_string(), Style::default()));
        pieces.push((words.clone(), style));
        control = Some(rect);
    } else if !radio.desc.is_empty() {
        pieces.push((" — ".to_string(), dim()));
        pieces.extend(radio.desc.iter().cloned());
    }
    let width = usize::from(at.width);
    // A path is told apart by its end: cut at its front.
    if radio.front
        && let Some((last, style)) = pieces.pop()
    {
        let head: usize = pieces.iter().map(|(t, _)| kit::width(t)).sum();
        pieces.push((fit_front(&last, width.saturating_sub(head)), style));
    }
    frame.render_widget(Paragraph::new(fit_line(&pieces, width)), at);
    if let Some(act) = &radio.act {
        page.ui.click(at, act.clone());
    }
    if let Some(rect) = control {
        page.ui.click(rect, Act::ListOpen);
    }
    control
}

/// The local build's choosers, the dialog waiting, or why none can open.
fn draw_choosers(frame: &mut Frame, page: &mut Page, sheet: &Sheet, row: Rect) {
    let at = Rect { x: row.x + SUB_X, width: row.width.saturating_sub(SUB_X), ..row };
    if sheet.dialog {
        let words = t!("dev.adv_local_dialog").to_string();
        frame.render_widget(Paragraph::new(Span::styled(fit(&words, usize::from(at.width)), dim())), at);
        return;
    }
    let focused = sheet.focus == Stop::Sub && sheet.list.is_none();
    let mut x = at.x;
    for chooser in sheet.choosers() {
        let words = t!(match chooser {
            Chooser::File => "dev.adv_choose_file",
            Chooser::Folder => "dev.adv_choose_folder",
            Chooser::Type => "dev.adv_type_path",
        })
        .to_string();
        let cells = kit::width(&words) as u16;
        if x + cells > at.right() {
            break;
        }
        let rect = Rect { x, y: row.y, width: cells, height: 1 };
        let style = if page.ui.hovers(rect) {
            hover()
        } else if focused && sheet.chooser == chooser {
            accent().add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        frame.render_widget(Paragraph::new(Span::styled(words, style)), rect);
        let at = Chooser::ALL.iter().position(|c| *c == chooser).unwrap_or(0);
        page.ui.click(rect, Act::SheetChooser(at));
        x = rect.right() + 3;
    }
    if let Some(why) = &sheet.no_dialog
        && x < at.right()
    {
        let words = t!("dev.adv_no_dialog", why = why).to_string();
        let rest = Rect { x, width: at.right() - x, ..at };
        frame.render_widget(Paragraph::new(Span::styled(fit(&words, usize::from(rest.width)), dim())), rest);
    }
}

/// The erase box: `[ ]`, or `[✓]` in green; its name in the accent while the
/// keyboard is on it.
fn draw_erase(frame: &mut Frame, page: &mut Page, sheet: &Sheet, row: Rect) {
    let g = glyphs();
    let at = Rect { x: row.x + 2, width: row.width.saturating_sub(2), ..row };
    let focused = sheet.focus == Stop::Erase && sheet.list.is_none() && sheet.field.is_none();
    let box_style = if sheet.erase { Style::default().fg(th().ok) } else { dim() };
    let name_style = if page.ui.hovers(at) {
        hover()
    } else if focused {
        accent().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let pieces = vec![
        (if sheet.erase { g.checked } else { "[ ]" }.to_string(), box_style),
        (" ".to_string(), Style::default()),
        (t!("dev.adv_erase").to_string(), name_style),
        (format!(" — {}", t!("dev.adv_erase_desc")), dim()),
    ];
    frame.render_widget(Paragraph::new(fit_line(&pieces, usize::from(at.width))), at);
    page.ui.click(at, Act::SheetErase);
}

/// Use defaults (dim) and Apply (the modal primary, dim while it cannot
/// apply), bottom right like the gate's; the keyboard's one wears the slab.
fn draw_buttons(frame: &mut Frame, page: &mut Page, sheet: &Sheet, row: Rect) {
    let focused = sheet.focus == Stop::Buttons && sheet.list.is_none() && sheet.field.is_none();
    let can = sheet.image().is_some();
    let defaults = format!("  {}  ", t!("dev.adv_defaults"));
    let apply = format!("  {}  ", t!("dev.adv_apply"));
    let (dw, aw) = (kit::width(&defaults) as u16, kit::width(&apply) as u16);
    let apply_x = row.right().saturating_sub(aw).max(row.x);
    let defaults_x = apply_x.saturating_sub(dw + 2).max(row.x);
    let buttons = [
        (defaults, defaults_x, dw, Act::Defaults, true, false),
        (apply, apply_x, aw, Act::Apply, can, true),
    ];
    for (label, x, w, act, enabled, primary) in buttons {
        let rect = Rect { x, y: row.y, width: w.min(row.right().saturating_sub(x)), height: 1 };
        let mine = focused && sheet.on_apply == primary;
        let style = if !enabled {
            dim()
        } else if mine {
            slab()
        } else if page.ui.hovers(rect) {
            hover()
        } else if primary {
            accent().add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        frame.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        if enabled {
            page.ui.click(rect, act);
        }
    }
}

/// One row of the release list: its words, the pickable entry it is, and
/// whether it stands in the rows' column (the asking and the failure lines
/// start at the frame's first cell).
type ListRow = (Vec<(String, Style)>, Option<usize>, bool);

/// The release list's rows.
fn list_rows(page: &Page) -> Vec<ListRow> {
    let g = glyphs();
    let entries = page.entries();
    let gold = Style::default().fg(th().gold);
    let mut rows: Vec<ListRow> = Vec::new();
    // The tag column as wide as every release's, shown or not, so showing
    // the pre-releases never moves it.
    let tag_w = match &page.listing {
        Listing::Listed(list) => list.releases.iter().map(|r| kit::width(&r.tag)).max().unwrap_or(0) + 2,
        _ => 0,
    };
    let pad = |text: &str, cells: usize| format!("{text}{}", " ".repeat(cells.saturating_sub(kit::width(text))));
    match &page.listing {
        Listing::NotAsked | Listing::Asking => rows.push((scan(None, &t!("dev.adv_list_asking")), None, false)),
        Listing::Listed(list) => {
            for (i, entry) in entries.iter().enumerate() {
                match entry {
                    Entry::Release(release) => {
                        let mut words = images_words(release.images);
                        if release.pre {
                            words = format!("{}, {words}", t!("dev.adv_pre"));
                        }
                        if release.newer_than_pin() {
                            words = format!("{words} · {}", t!("dev.adv_newer"));
                        }
                        let pieces = vec![
                            (pad(&release.tag, tag_w), Style::default()),
                            (pad(&release.date, 12), dim()),
                            (words, Style::default()),
                        ];
                        rows.push((pieces, Some(i), true));
                    }
                    Entry::Toggle => {
                        let not_pin = |r: &&Release| PINNED_TAG != Some(r.tag.as_str());
                        let pre = list.releases.iter().filter(|r| r.pre).filter(not_pin).count();
                        let words = match (page.show_pre, pre) {
                            (true, _) => t!("dev.adv_list_pre_hide").to_string(),
                            (false, 1) => t!("dev.adv_list_pre_show_one").to_string(),
                            (false, n) => t!("dev.adv_list_pre_show", n = n).to_string(),
                        };
                        rows.push((vec![(words, Style::default())], Some(i), true));
                    }
                    _ => {}
                }
            }
            if !entries.iter().any(|e| matches!(e, Entry::Release(_))) {
                rows.push((vec![(t!("dev.adv_list_none").to_string(), dim())], None, true));
            }
            let asked = list.asked.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            rows.push((vec![(t!("dev.adv_list_asked", time = clock_at(asked)).to_string(), dim())], None, true));
        }
        Listing::Failed(failed) => {
            rows.push((vec![(format!("{} {}", g.no, failed.why.text()), gold)], None, false));
            for (i, entry) in entries.iter().enumerate() {
                let words = match entry {
                    Entry::Cached(cached) => match cached.since.and_then(date_of) {
                        Some(date) => t!("dev.adv_list_cached", tag = cached.tag, date = date).to_string(),
                        None => t!("dev.adv_list_cached_plain", tag = cached.tag).to_string(),
                    },
                    Entry::Retry => t!("dev.adv_list_retry").to_string(),
                    _ => continue,
                };
                rows.push((vec![(words, Style::default())], Some(i), true));
            }
        }
    }
    rows
}

/// The kit's dropdown hanging from the release control (clause 35): a
/// rounded accent frame over the sheet's lower rows, no title, no `[X]`,
/// kept inside the sheet. A click anywhere else closes it; the rows' own
/// clicks pick.
fn draw_list(frame: &mut Frame, page: &mut Page, sheet: &Sheet, area: Rect, content: Rect, anchor: Rect) {
    let rows = list_rows(page);
    let picked = sheet.picked.as_ref().map(|p| p.tag.clone());
    let entries = page.entries();
    let cursor = sheet.list.unwrap_or(0);
    // A row is a cell of padding, the column's lead where it has one, its
    // words, and a cell of padding; the frame is two more.
    let cells = |(pieces, _, column): &ListRow| {
        2 + if *column { 2 } else { 0 } + pieces.iter().map(|(t, _)| kit::width(t)).sum::<usize>()
    };
    let widest = rows.iter().map(cells).max().unwrap_or(0);
    let w = (widest as u16 + 2).min(content.width);
    let x = anchor.x.saturating_sub(3).max(content.x).min(content.right().saturating_sub(w));
    let y = anchor.y + 1;
    let room = content.bottom().saturating_sub(y);
    let h = (rows.len() as u16 + 2).min(room);
    if h < 3 {
        return;
    }
    page.ui.click(area, Act::ListClose);
    let rect = Rect { x, y, width: w, height: h };
    // A wide character just left of the frame would run into its border
    // (a Japanese choice's last kana): it goes, the border stays whole.
    if rect.x > content.x {
        for row in rect.top()..rect.bottom() {
            let cell = &mut frame.buffer_mut()[(rect.x - 1, row)];
            if kit::width(cell.symbol()) > 1 {
                cell.set_symbol(" ");
            }
        }
    }
    // The rows it hangs over are blank to the sheet's edge: no half a
    // description shows past the list.
    kit::blank(frame, Rect { width: content.right() - x, ..rect });
    page.ui.overlay(rect);
    let inner = kit::frame_at(frame, rect, th().accent);
    let visible = usize::from(inner.height);
    let at = rows.iter().position(|(_, pick, _)| *pick == Some(cursor)).unwrap_or(0);
    let start = (at + 1).saturating_sub(visible).min(rows.len().saturating_sub(visible));
    for (i, (pieces, pick, column)) in rows.iter().skip(start).take(visible).enumerate() {
        let row = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        let is_picked = pick.and_then(|p| entries.get(p)).is_some_and(|e| match e {
            Entry::Release(r) => Some(&r.tag) == picked.as_ref(),
            Entry::Cached(c) => Some(&c.tag) == picked.as_ref(),
            _ => false,
        });
        let lead = if is_picked { glyphs().bullet } else { " " };
        let selected = *pick == Some(cursor);
        let hovered = pick.is_some() && page.ui.hovers(row);
        let mut line = vec![(" ".to_string(), Style::default())];
        if *column {
            line.push((format!("{lead} "), accent()));
        }
        line.extend(pieces.iter().cloned());
        let line: Vec<(String, Style)> = if selected {
            let used: usize = line.iter().map(|(t, _)| kit::width(t)).sum();
            let mut line: Vec<(String, Style)> = line.into_iter().map(|(t, _)| (t, slab())).collect();
            line.push((" ".repeat(usize::from(row.width).saturating_sub(used)), slab()));
            line
        } else if hovered {
            line.into_iter().map(|(t, s)| (t, if s == accent() { s } else { hover() })).collect()
        } else {
            line
        };
        frame.render_widget(Paragraph::new(fit_line(&line, usize::from(row.width))), row);
        if let Some(pick) = pick {
            page.ui.click(row, Act::ListPick(*pick));
        }
    }
}
