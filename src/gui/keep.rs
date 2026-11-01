//! The Discover room's Keep a copy window (docs/ux-contracts/discovery-page.md,
//! clauses 106–110; the "Keep a copy, calmer" boards), the room's job
//! marks' glyphs and words (clauses 113–114, 124), the Copies list and
//! Open elsewhere's list (clauses 117, 140–141; the canvas's TermCopies
//! board), and the status line's copy and link notes (clauses 118, 141) —
//! the GUI half of the App's `tui::app::keep`, which holds every fact drawn
//! here.
//!
//! The window is the kit's Modal, 76 cells wide and centred in the room,
//! whose width it clears on its own rows (the band); anchored as if 17 rows
//! tall, so its top holds while its body changes, it grows downward and
//! never past the room's last row — at the floor its blanks give way
//! first. Its title rides the top border, its way out the bottom ("Esc
//! close"). Under the head (the item's title BOLD, a DIM byline, a 4 × 2
//! pixel cover where the terminal draws pixels) each state is one table of
//! rows: the choice — its radios, and "Only the 1 album you don't have" as
//! their fourth stop; the place in one sentence ("Files go to …", "Into
//! …", "In …", "Retry adds to …"); the copy's two-line progress or its
//! outcome; and the action row — DIM key words at the left, each drawn only
//! where its key works, and the one bracketed primary at the right, which
//! Enter always presses. `d` (or `d change`) opens **Where copies land** in
//! its place (clauses 142–148; the phone's destination sheet and the
//! webapp's editor in the terminal's grammar): the account's destination,
//! edited and saved on the server.

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::api::types::{
    CollectionDestination, CollectionDestinationView, DEFAULT_LAYOUT, JobAction, JobMark, JobOutcome, JobScope, Landing,
    LayoutProblem, MAX_RESOLVED_LEN, MAX_TEMPLATE_LEN, ManyCounts, PEER_LAYOUT, PluginJob, PluginLink, Run, SERVER_FOLDER,
    SERVERFOLDER_LAYOUT, SongTag, StopReason, base_folders, braced_folder, fold_base, layout_lead, sanitize_segment,
    start_layout_at, validate_layout, validate_resolved_path,
};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, dim, scroll_list, width};
use crate::tui::app::keep::{
    ChoiceSize, CopyRow, CopyTitle, Fact, Folders, KeepChoice, KeepNote, KeepPhase, KeepStage, KeepVerb, KeepView,
    LinksStatus, OffWhy,
};
use crate::tui::app::MessageKind;

use super::cover::{Pace, Slot};
use super::discover::DiscAct;
use super::opener::{HandOff, open_target};
use super::{Act, Gui, accent, bright_bold, put, sel};

/// The window's width (clause 106) — the Copies list's: its text in inner
/// cols 1–72, the last inner cell blank before the right border. An area
/// narrower than it and four more takes it down to the area less 4, never
/// under 44.
pub(crate) const KEEP_W: u16 = 76;
/// The height it anchors as: its top stands where a 17-row window's would
/// (row 6 at 30 rows, row 3 at 24), and it grows downward.
const KEEP_ANCHOR: u16 = 17;
/// The head under the title on the top border: the item's title and its
/// byline. The blank under them (B1) is the table's — the give-way may
/// drop it.
const HEAD_ROWS: u16 = 2;
/// From the text's column (inner col 1): a radio's label and the box's
/// glyph, the box's label, and where a wrapped row (a place, a GOLD line, a
/// headline) continues.
const LABEL_X: u16 = 4;
const BOX_LABEL_X: u16 = 8;
const CONT_X: u16 = 2;
/// The primary's cells at most, its brackets in: past them its count drops.
const PRIMARY_MAX: usize = 34;
/// The cells between key words, and between them and the primary at least.
const KEY_GAP: usize = 3;
/// The cells between a place's last crumb and `d change` at least.
const CHANGE_GAP: usize = 2;

// ── Geometry (shared with the Copies list) ──────────────────────────────────

/// The top a window anchored as if [`KEEP_ANCHOR`] rows tall stands at.
fn anchored_top(area: Rect) -> u16 {
    area.y + area.height.saturating_sub(KEEP_ANCHOR) / 2
}

/// The rows a room's window has from its anchored top down to the room's
/// last row (the one above the player bar's seek line, which no window
/// covers).
fn rows_anchored(area: Rect, room: Rect) -> u16 {
    room.bottom().saturating_sub(anchored_top(area))
}

/// The rows it has once it rises toward row 1 — never row 0, the tab bar.
fn rows_risen(area: Rect, room: Rect) -> u16 {
    room.bottom().saturating_sub(area.y + 1)
}

/// Whether a window `w` wide centres in the room with its band — the room
/// at least four cells wider — or over the whole frame (the queue panel
/// open), no band.
fn banded(room: Rect, w: u16) -> bool {
    room.width >= w + 4
}

/// The window's width in this frame: `w`, or the area less 4 where the area
/// is narrower, never under 44.
fn window_width(area: Rect, room: Rect, w: u16) -> u16 {
    if banded(room, w) { w } else { w.min(area.width.saturating_sub(4)).max(44).min(area.width) }
}

/// Where a room's window stands (clauses 106, 117) and its band: `w` wide,
/// centred in the room and the room's width cleared on its rows — or
/// centred over the whole frame where the room is too narrow, no band; its
/// top where a [`KEEP_ANCHOR`]-row window's would be, rising toward row 1
/// only as far as its `h` rows need, its bottom border never under the
/// room's last row. Keep a copy and the Copies list open in the same place.
pub(crate) fn window_rect(area: Rect, room: Rect, w: u16, h: u16) -> (Rect, Option<Rect>) {
    let w = window_width(area, room, w);
    let x = if banded(room, w) { room.x + (room.width - w) / 2 } else { area.x + area.width.saturating_sub(w) / 2 };
    let h = h.min(rows_risen(area, room));
    let top = anchored_top(area).min(room.bottom().saturating_sub(h)).max(area.y + 1);
    let rect = Rect { x, y: top, width: w, height: h };
    let band = banded(room, w).then_some(Rect { x: room.x, y: top, width: room.width, height: h });
    (rect, band)
}

/// Clear a window's footprint — the band where it has one — register it as
/// the overlay the room's covers stand down under (Over pixels) and as
/// inert, the whole frame inert beneath (the kit's Modal: what drew before
/// it registered is dropped first, so no tooltip, context or click of the
/// room's outlives it — the room drew without the pointer); its frame then
/// drawn inside. The overlay registered is returned: what a pixel cover
/// inside the window asks about ([`Slot`]'s one-frame mosaic).
fn open_window(frame: &mut Frame, gui: &mut Gui, area: Rect, rect: Rect, band: Option<Rect>) -> (Rect, Rect) {
    gui.ui.clear_registries();
    gui.ui.click(area, Act::Guard);
    let own = band.unwrap_or(rect);
    gui.ui.overlay(own);
    gui.ui.modal_over();
    if let Some(band) = band {
        crate::kit::blank(frame, band);
    }
    let inner = modal_box(frame, rect);
    gui.ui.click(own, Act::Guard);
    (inner, own)
}

// ── State ───────────────────────────────────────────────────────────────────

/// The window's own state: the outcome Retry was pressed on, the head's
/// cover and what the last frame drew. Everything it shows is the App's
/// flow ([`crate::tui::app::keep::KeepFlow`]).
#[derive(Default)]
pub(crate) struct KeepUi {
    /// Retry pressed: the outcome it was pressed on, drawn — its key words
    /// gone, `[ Retrying… ]` — while the start it sent is out (E20).
    pub frozen: Option<KeepView>,
    /// The head's cover's slot: pixels while the window is the top layer.
    slot: Option<Slot>,
    /// The window's footprint last frame: its own, never another overlay
    /// over its cover, when its height or its top changed since.
    last_own: Option<Rect>,
    /// The key words the last frame drew, for the view it drew them for:
    /// a key acts only where its word stood on screen — the give-way's
    /// last resort may have taken its row (§3.6, §6.2).
    drawn: Option<(KeepView, Shown, Vec<KeyWord>)>,
    /// "Where copies land" while it is open over the window (clause 142).
    pub dest: Option<DestUi>,
}

impl KeepUi {
    pub(crate) fn on_resize(&mut self) {
        if let Some(slot) = &mut self.slot {
            slot.on_resize();
        }
    }
}

/// What the window's pointer and keys do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeepAct {
    /// Esc, "Esc close" on the bottom border, `[ Close ]`: the copy keeps
    /// running (clause 109).
    Close,
    /// A choice's row, clicked.
    Pick(KeepChoice),
    /// The box, clicked.
    Missing,
    /// The primary, clicked — Enter presses it too.
    Primary,
    /// A key word, clicked — its key does the same.
    Key(KeyWord),
    /// The picker's controls (clauses 142–144).
    Dest(DestAct),
}

/// A key word (the window's other controls, §4.8): a DIM text button whose
/// first word is its key, drawn only where that key works — and the key
/// works only where it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyWord {
    /// `d change`: Where copies land.
    Change,
    /// `a add to queue`: the copied song at the queue's end.
    Queue,
    /// `c choose again`: back to the choice.
    Choose,
    /// `x cancel copy`: this window's copy.
    Cancel,
    /// `x cancel that copy`: the other server's.
    CancelOther,
}

impl KeyWord {
    /// Its key — the words' first letter in English, never translated.
    fn key(self) -> char {
        match self {
            KeyWord::Change => 'd',
            KeyWord::Queue => 'a',
            KeyWord::Choose => 'c',
            KeyWord::Cancel | KeyWord::CancelOther => 'x',
        }
    }

    fn words(self) -> String {
        match self {
            KeyWord::Change => t!("gui.discover.keep.key_change"),
            KeyWord::Queue => t!("gui.discover.keep.key_queue"),
            KeyWord::Choose => t!("gui.discover.keep.key_choose"),
            KeyWord::Cancel => t!("gui.discover.keep.key_cancel"),
            KeyWord::CancelOther => t!("gui.discover.keep.key_cancel_other"),
        }
        .to_string()
    }

    /// Its words in an ended copy's tips line (§6.4): `d` names the picker
    /// there, the others as drawn.
    fn tip(self) -> String {
        match self {
            KeyWord::Change => t!("gui.tips.discover_keep_d").to_string(),
            other => other.words(),
        }
    }
}

/// What the primary does when pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    /// Add …: start the chosen copy.
    Add,
    /// An ended copy's own: Retry, Play your copy, Add again.
    Again(JobAction),
    Close,
}

/// The one bracketed button: its words, and what Enter or a click does —
/// `None` while a start it sent is out (`[ Adding… ]`, `[ Retrying… ]`:
/// DIM, no click, no hover, the kit's modal primary that goes off).
#[derive(Debug, Clone, PartialEq)]
struct Primary {
    words: String,
    press: Option<Press>,
}

impl Primary {
    fn close() -> Primary {
        Primary { words: t!("gui.discover.keep.close").to_string(), press: Some(Press::Close) }
    }

    fn label(&self) -> String {
        format!("[ {} ]", self.words)
    }
}

// ── Glyphs and words ────────────────────────────────────────────────────────

/// A word's `…` as the legacy console draws it: three dots (clause 124).
pub(crate) fn ell(words: &str) -> String {
    if legacy_conhost() { words.replace('…', "...") } else { words.to_string() }
}

fn tick() -> &'static str {
    if legacy_conhost() { "+" } else { "✓" }
}

fn cross() -> &'static str {
    if legacy_conhost() { "x" } else { "×" }
}

fn crumb_sep() -> &'static str {
    if legacy_conhost() { " > " } else { " › " }
}

/// The mark a clipped text ends with — the kit's clip's.
fn clip_mark() -> &'static str {
    if legacy_conhost() { "»" } else { "…" }
}

/// A landing's crumbs in their tokens (clauses 105, 143): the library
/// BOLD, each folder TEXT — a variable the server would refuse GOLD, as
/// typed; a variable each song fills from its own tags its slot DIM,
/// `‹artist›`, inside the folder's own words — and an artist copy's "one
/// folder per album" DIM: the slot and the stand-in are hints, never a
/// folder's name.
fn landing_crumbs(landing: &Landing) -> Vec<Vec<(String, Style)>> {
    let gold = Style::default().fg(th().gold);
    let mut out = vec![vec![(landing.library.clone(), Style::default().add_modifier(Modifier::BOLD))]];
    out.extend(landing.folders.iter().map(|folder| {
        folder
            .iter()
            .map(|(text, run)| match run {
                Run::Text => (text.clone(), Style::default()),
                Run::Unknown => (text.clone(), gold),
                Run::Slot(slot) => (slot_words(*slot), dim()),
            })
            .collect()
    }));
    if landing.per_album {
        out.push(vec![(t!("gui.discover.dest.per_album").to_string(), dim())]);
    }
    out
}

/// A slot as a path draws it: the locale's word inside `‹ ›` (legacy
/// `< >`).
fn slot_words(slot: SongTag) -> String {
    let word = match slot {
        SongTag::Artist => t!("gui.discover.dest.slot_artist"),
        SongTag::Year => t!("gui.discover.dest.slot_year"),
        SongTag::Genre => t!("gui.discover.dest.slot_genre"),
        SongTag::ServerFolder => t!("gui.discover.dest.slot_server"),
    };
    if legacy_conhost() { format!("<{word}>") } else { format!("‹{word}›") }
}

/// The note a path with a slot carries (clause 105; the webapp's and the
/// phone's words): which variables come from each song's own tags, in the
/// path's order, so the songs can land in more than one folder — and,
/// with a genre, that a song with none leaves its folder out; a song's
/// copy alone speaks of its own tags. `‹server folder›` comes from where
/// each song sits on the server: alone it has its own words; beside the
/// tags it is said after them, and the tags alone pick the verb's number
/// ("‹artist› comes from each song's own tags and ‹server folder› from
/// where it sits on the server, …"). `None` where the path shows no slot.
fn slot_note(landing: &Landing) -> Option<String> {
    let slots = landing.slots();
    if slots.is_empty() {
        return None;
    }
    let tags: Vec<SongTag> = slots.iter().copied().filter(|slot| *slot != SongTag::ServerFolder).collect();
    let server = (tags.len() < slots.len()).then(|| slot_words(SongTag::ServerFolder));
    if tags.is_empty() {
        return server.map(|server| t!("gui.discover.dest.slot_note_server", server = server).to_string());
    }
    let names = tags.iter().map(|slot| slot_words(*slot)).collect::<Vec<_>>().join(", ");
    if landing.one_song && server.is_none() {
        return Some(t!("gui.discover.dest.slot_note_song", slots = names).to_string());
    }
    let stem = match (server.is_some(), tags.contains(&SongTag::Genre)) {
        (true, true) => "gui.discover.dest.slot_note_mixed_genre",
        (true, false) => "gui.discover.dest.slot_note_mixed",
        (false, true) => "gui.discover.dest.slot_note_genre",
        (false, false) => "gui.discover.dest.slot_note",
    };
    let key = format!("{stem}_{}", super::discover::plural_form(tags.len() as u64));
    Some(t!(key.as_str(), slots = names, server = server.unwrap_or_default()).to_string())
}

/// A place's whole path, as its tooltip says it where a row clips it
/// (§4.10): `a/b/c`, a slot in its words, a done song's file last.
fn whole_path(landing: &Landing, file: Option<&str>) -> String {
    let mut words = landing.words(&slot_words);
    words.extend(file.map(str::to_string));
    words.join("/")
}

/// The cells a row of runs takes.
fn cells_of(runs: &[(String, Style)]) -> usize {
    runs.iter().map(|(text, _)| width(text)).sum()
}

/// Runs cut to `room` cells, the clip's mark in the last cell where they
/// outgrow it (the kit's clip, across runs) — and whether they were cut.
fn clip_runs(runs: &[(String, Style)], room: usize) -> (Vec<(String, Style)>, bool) {
    if cells_of(runs) <= room {
        return (runs.to_vec(), false);
    }
    let mut out = Vec::new();
    let mut used = 0;
    for (text, style) in runs {
        let w = width(text);
        if used + w < room {
            out.push((text.clone(), *style));
            used += w;
            continue;
        }
        // The run that reaches the edge gives its last cell to the mark —
        // one that ends exactly there too, since more runs follow it.
        let left = room - used;
        let padded = if w <= left { format!("{text} ") } else { text.clone() };
        out.push((super::bar::clip(&padded, left).into_owned(), *style));
        break;
    }
    (out, true)
}

/// A path's runs in rows of `room` cells (clause 105): after `lead`, the
/// crumbs with `›` DIM between them; where they outgrow the first row
/// they break before a crumb, ` ›` ending it, and the rest stands on a
/// second.
fn path_rows(lead: Vec<(String, Style)>, crumbs: &[Vec<(String, Style)>], room: usize) -> Vec<Vec<(String, Style)>> {
    let sep = crumb_sep();
    let cont = sep.trim_end();
    let mut rows = vec![lead];
    let mut placed = false;
    for (i, crumb) in crumbs.iter().enumerate() {
        // A crumb that is not the last keeps room for the ` ›` a break
        // after it would need.
        let reserve = if i + 1 == crumbs.len() { 0 } else { width(cont) };
        let need = if placed { width(sep) } else { 0 } + cells_of(crumb) + reserve;
        if placed && rows.len() == 1 && cells_of(&rows[0]) + need > room {
            rows[0].push((cont.to_string(), dim()));
            rows.push(Vec::new());
            placed = false;
        }
        let row = rows.last_mut().expect("a row");
        if placed {
            row.push((sep.to_string(), dim()));
        }
        row.extend(crumb.iter().cloned());
        placed = true;
    }
    rows
}

/// A plural family's words for `count` (`_one`, `_few`, `_many`, `_other`).
pub(crate) fn plural(stem: &str, count: u64) -> String {
    super::discover::plural(stem, count)
}

/// The row's LEAD glyph for a mark (clause 113), behind `legacy_conhost`.
pub(crate) fn mark_glyph(mark: JobMark) -> &'static str {
    let legacy = legacy_conhost();
    match mark {
        JobMark::Waiting => {
            if legacy {
                "~"
            } else {
                "◷"
            }
        }
        JobMark::Copying | JobMark::Stopping => {
            if legacy {
                "v"
            } else {
                "↓"
            }
        }
        JobMark::Kept | JobMark::Yours => tick(),
        JobMark::Failed | JobMark::Stopped | JobMark::Refused => "!",
        JobMark::Cancelled => cross(),
    }
}

/// A mark's token (clauses 113–114): live ACCENT, waiting and stopping and
/// the quiet outcomes DIM, kept OK, the troubled GOLD.
pub(crate) fn mark_style(mark: JobMark) -> Style {
    match mark {
        JobMark::Copying => accent(),
        JobMark::Kept => Style::default().fg(th().ok),
        JobMark::Failed | JobMark::Stopped | JobMark::Refused => Style::default().fg(th().gold),
        JobMark::Waiting | JobMark::Stopping | JobMark::Yours | JobMark::Cancelled => dim(),
    }
}

/// The tile's state word (clause 114) — the glyph in code, the word the
/// locale's; "copying…" and "stopping…" carry none.
pub(crate) fn mark_word(mark: JobMark) -> String {
    let word = |key: &str| ell(&t!(key));
    match mark {
        JobMark::Copying => word("gui.discover.mark.copying"),
        JobMark::Stopping => word("gui.discover.mark.stopping"),
        JobMark::Waiting => format!("{} {}", mark_glyph(mark), word("gui.discover.mark.waiting")),
        JobMark::Kept => format!("{} {}", mark_glyph(mark), word("gui.discover.mark.kept")),
        JobMark::Yours => format!("{} {}", mark_glyph(mark), word("gui.discover.mark.yours")),
        JobMark::Failed => format!("{} {}", mark_glyph(mark), word("gui.discover.mark.failed")),
        JobMark::Stopped => format!("{} {}", mark_glyph(mark), word("gui.discover.mark.stopped")),
        JobMark::Refused => format!("{} {}", mark_glyph(mark), word("gui.discover.mark.refused")),
        JobMark::Cancelled => format!("{} {}", mark_glyph(mark), word("gui.discover.mark.cancelled")),
    }
}

/// A count's words in the windows: "3 songs", "1 copied".
fn count(stem: &str, n: u64) -> String {
    plural(stem, n)
}

/// A many-song copy's counts as the Copies list's runs (clause 117):
/// copied, already yours, failed (GOLD), albums left out — what the
/// result's counts hold.
fn many_runs(n: &ManyCounts, base: Style) -> Vec<(String, Style)> {
    let mut runs = vec![(count("gui.discover.keep.copied", n.copied), base)];
    if n.skipped > 0 {
        runs.push((count("gui.discover.keep.owned", n.skipped), base));
    }
    if n.failed > 0 {
        runs.push((count("gui.discover.keep.failed", n.failed), Style::default().fg(th().gold)));
    }
    if n.albums_left_out > 0 {
        runs.push((count("gui.discover.keep.left_out", n.albums_left_out as u64), base));
    }
    runs
}

/// A choice's name, as its radio says it.
fn choice_name(choice: KeepChoice, view: &KeepView) -> String {
    match choice {
        KeepChoice::Song => t!("gui.discover.keep.song").to_string(),
        KeepChoice::Album => t!("gui.discover.keep.album").to_string(),
        KeepChoice::Artist => {
            let artist = view.subject.track_artist.clone().unwrap_or_default();
            t!("gui.discover.keep.artist", artist = artist).to_string()
        }
    }
}

/// The primary's words while choosing (§4.8) — the size lives here: Add
/// song · "Add album · 3 songs" · "Add 2 albums" (the connected server's
/// albums by the artist) · "Add 1 album" (only the missing; "Add the albums
/// you don't have" where a copy of them keeps the box with none missing by
/// name) — a count only once it is known ("Add album", "Add every album"
/// meanwhile), and dropped where the label would pass [`PRIMARY_MAX`]
/// cells.
fn add_words(verb: KeepVerb, view: &KeepView) -> String {
    let size = |choice: KeepChoice| view.choices.iter().find(|(c, _)| *c == choice).map(|(_, size)| size);
    match verb {
        KeepVerb::Song => t!("gui.discover.keep.add_song").to_string(),
        KeepVerb::Album => {
            let add = t!("gui.discover.keep.add_album").to_string();
            match size(KeepChoice::Album) {
                Some(ChoiceSize::Album { songs: Fact::Known(n) }) => {
                    let whole = format!("{add} · {}", count("gui.discover.keep.songs", *n as u64));
                    if width(&whole) + 4 <= PRIMARY_MAX { whole } else { add }
                }
                _ => add,
            }
        }
        KeepVerb::Artist => match size(KeepChoice::Artist) {
            Some(ChoiceSize::Artist { albums: Fact::Known(n), .. }) if *n > 0 => {
                count("gui.discover.keep.add_albums", *n as u64)
            }
            _ => t!("gui.discover.keep.add_every").to_string(),
        },
        // Kept by a copy that still wants the user, none missing by name:
        // no count to say.
        KeepVerb::Missing(0) => t!("gui.discover.keep.add_missing").to_string(),
        KeepVerb::Missing(n) => count("gui.discover.keep.add_albums", n as u64),
    }
}

/// The server's words, or "your server didn't answer" when it did not
/// (clause 130); a start refused for a destination no longer in force has
/// the room's own (clause 146: retried from the Copies list, it is Keep a
/// copy's to show where files land now).
fn failure_words(failure: &crate::api::PluginFailure) -> String {
    match failure {
        crate::api::PluginFailure::DestinationChanged(_) => t!("gui.discover.dest.changed_retry").to_string(),
        failure if failure.unreachable() => t!("gui.discover.keep.no_answer").to_string(),
        failure => failure.words().to_string(),
    }
}

/// Greedy wrap at `width` cells that keeps each of `keep` (a server's name)
/// whole on one line. A token wider than a row — a sentence with no blank
/// in it (Japanese, Chinese), a long path — fills the row it starts on and
/// breaks between grapheme clusters, by cells, as the kit's tooltip wrap
/// does: no row outgrows `width`.
pub(crate) fn wrap_keeping(text: &str, width_cells: usize, keep: &[&str]) -> Vec<String> {
    use unicode_segmentation::UnicodeSegmentation;
    // Tokens: words, the kept phrases glued into one.
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    let phrases: Vec<Vec<&str>> =
        keep.iter().filter(|k| !k.trim().is_empty()).map(|k| k.split(' ').filter(|w| !w.is_empty()).collect()).collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut i = 0;
    'outer: while i < words.len() {
        for phrase in &phrases {
            let n = phrase.len();
            if n > 1 && i + n <= words.len() {
                let last = words[i + n - 1];
                let head_ok = words[i..i + n - 1].iter().zip(phrase.iter()).all(|(a, b)| a == b);
                // The phrase's last word may carry a tail ("Peer B's").
                if head_ok && last.starts_with(phrase[n - 1]) {
                    tokens.push(words[i..i + n].join(" "));
                    i += n;
                    continue 'outer;
                }
            }
        }
        tokens.push(words[i].to_string());
        i += 1;
    }
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for token in tokens {
        if width(&token) > width_cells {
            let mut cells = width(&line);
            if !line.is_empty() {
                line.push(' ');
                cells += 1;
            }
            for grapheme in token.graphemes(true) {
                let w = crate::kit::grapheme_cells(grapheme);
                if cells + w > width_cells && cells > 0 {
                    lines.push(std::mem::take(&mut line).trim_end().to_string());
                    cells = 0;
                }
                line.push_str(grapheme);
                cells += w;
            }
            continue;
        }
        let need = if line.is_empty() { width(&token) } else { width(&line) + 1 + width(&token) };
        if need > width_cells && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&token);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Words in the window's rows (§4.6–4.7, §9): wrapped at `room` keeping
/// each of `keep` (a server's name) whole, the rows after the first `hang`
/// cells in; two rows at most — the second clipped where words are left —
/// or one where the give-way asks (`one`), cut at a word. The whole words
/// are the rows' tooltip wherever they were cut — a row that would still
/// outgrow its cells is clipped there too.
fn wrap_rows(text: &str, room: usize, hang: usize, keep: &[&str], one: bool) -> (Vec<String>, Option<String>) {
    let lines = wrap_keeping(text, room, keep);
    let tip = Some(text.to_string());
    match lines.as_slice() {
        [] => return (lines, None),
        [only] if width(only) <= room => return (lines, None),
        [only] => return (vec![super::bar::clip(only, room).into_owned()], tip),
        _ => {}
    }
    if one {
        let cut = format!("{}{}", lines[0], clip_mark());
        let shown = if width(&cut) <= room { cut } else { super::bar::clip(text, room).into_owned() };
        return (vec![shown], tip);
    }
    let room = room.saturating_sub(hang);
    // The words after the first row as they run: a row broken inside a
    // token (a sentence with no blank) carries on where it broke.
    let flat = text.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    let rest = match flat.strip_prefix(lines[0].as_str()) {
        Some(rest) => rest.trim_start().to_string(),
        None => lines[1..].join(" "),
    };
    match wrap_keeping(&rest, room, keep).as_slice() {
        [second] if width(second) <= room => (vec![lines[0].clone(), second.clone()], None),
        _ => (vec![lines[0].clone(), super::bar::clip(&rest, room).into_owned()], tip),
    }
}

/// A right gone, as its one plain line says it (clause 147).
fn off_words(why: OffWhy) -> String {
    match why {
        OffWhy::Uploads => t!("gui.discover.dest.nowhere"),
        OffWhy::Jobs => t!("gui.discover.keep.off_jobs"),
        OffWhy::Plugin => t!("gui.discover.keep.off_plugin"),
    }
    .to_string()
}

/// Its first letter raised — a sub that starts with words written to
/// follow another's (" · with your copy of …").
fn raised(words: &str) -> String {
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

// ── The table, worked out ───────────────────────────────────────────────────

/// A blank row the give-way may drop (§3.6), by where it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gap {
    /// Under the head.
    B1,
    /// Under the choice, or under the progress or the outcome, where more
    /// of the body follows.
    B2,
    /// Above the action row.
    B3,
}

/// One row of the window under its head (§3.3).
#[derive(Debug, Clone, PartialEq)]
enum Low {
    Blank(Gap),
    /// A radio (shown where the subject offers two choices or more).
    Choice(KeepChoice),
    /// "Only the N albums you don't have" under Every album by: ticked or
    /// not, its count — 0 for none to say ("Only the albums you don't
    /// have").
    Missing { on: bool, count: usize },
    /// A row of the place (§4.4): its lead DIM and its crumbs, at col 3
    /// where it continues a first; `change` stands `d change` at its end;
    /// `tip` the whole path where a row clips it.
    Place { runs: Vec<(String, Style)>, cont: bool, change: bool, tip: Option<String> },
    /// A row of words in their tokens — a headline, a sub, the slot note, a
    /// GOLD line, a plain line — at col 3 where it continues a headline or
    /// a GOLD line; `tip` the whole words where they were cut.
    Text { runs: Vec<(String, Style)>, cont: bool, tip: Option<String> },
    /// The progress bar (§4.7): `None` with no estimate; `tail` its DIM
    /// words after it ("starts on its own").
    Bar { progress: Option<f64>, tail: Option<String> },
    /// The key words on their own row, where they would crowd the primary.
    Keys(Vec<KeyWord>),
    /// The key words at the left, the primary at the right.
    Action { keys: Vec<KeyWord>, primary: Primary },
}

/// How far the give-way went (§3.6). Each step only while the window is
/// still too tall for the rows it has; the top's rise (step 3) is the
/// geometry's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Fit {
    /// Step 1: the blanks dropped, B2 first, then B1, then B3 (0–3).
    blanks: u8,
    /// Step 2: the place on one row.
    place: bool,
    /// Step 4: the slot note on one row.
    note: bool,
    /// Step 5: a GOLD line, a headline and a sub on one row each.
    lines: bool,
}

impl Fit {
    fn drops(self, gap: Gap) -> bool {
        match gap {
            Gap::B2 => self.blanks >= 1,
            Gap::B1 => self.blanks >= 2,
            Gap::B3 => self.blanks >= 3,
        }
    }
}

/// What a frame shows besides the App's view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Shown {
    /// The view is the outcome Retry was pressed on, held while its start
    /// is out (E20): its key words gone, the primary off.
    retrying: bool,
    /// The followed copy's cancel is out: it reads as stopping until the
    /// server says so.
    cancel_out: bool,
}

/// The view a frame draws and what it shows besides: the outcome Retry
/// was pressed on while its start is out, else the App's own.
fn shown_of(gui: &Gui) -> Option<(KeepView, Shown)> {
    let view = gui.app.keep_view()?;
    if matches!(view.stage, KeepStage::Starting)
        && let Some(frozen) = &gui.discover.keep.frozen
    {
        return Some((frozen.clone(), Shown { retrying: true, cancel_out: false }));
    }
    let cancel_out = match &view.stage {
        KeepStage::Job { job, .. } if job.live() => gui.app.copies_rows().iter().any(|r| r.job.id == job.id && r.cancelling),
        _ => false,
    };
    Some((view, Shown { retrying: false, cancel_out }))
}

/// Whether a copy reads as stopping: the server says its cancel was asked,
/// or this window's cancel is out.
fn stopping(outcome: &JobOutcome, shown: Shown) -> bool {
    matches!(outcome, JobOutcome::Running { stopping: true, .. }) || (outcome.live() && shown.cancel_out)
}

/// Whether the window's own copy is queued or running — the footer says
/// it keeps going (§4.9).
fn going(view: &KeepView, shown: Shown) -> bool {
    !shown.retrying && matches!(&view.stage, KeepStage::Job { outcome, .. } if outcome.live() && !stopping(outcome, shown))
}

/// The place line (§4.4): `lead` DIM, then the crumbs — a done song's file
/// DIM last — on one row where the sentence fits (ten cells fewer where
/// `d change` ends it); else broken before a crumb, ` ›` ending the first
/// row and the rest on a second at col 3, which is clipped at the right
/// where it still outgrows its cells — the library always shows — the whole
/// path then both rows' tooltip. `one`: the give-way's step 2 — one row,
/// clipped before `d change`.
fn place_lows(lead: &str, landing: &Landing, file: Option<&str>, change: bool, room: usize, one: bool) -> Vec<Low> {
    let mut crumbs = landing_crumbs(landing);
    if let Some(file) = file {
        crumbs.push(vec![(file.to_string(), dim())]);
    }
    let lead = vec![(format!("{lead} "), dim())];
    let mut whole = lead.clone();
    for (i, crumb) in crumbs.iter().enumerate() {
        if i > 0 {
            whole.push((crumb_sep().to_string(), dim()));
        }
        whole.extend(crumb.iter().cloned());
    }
    let beside = if change { width(&KeyWord::Change.words()) + CHANGE_GAP } else { 0 };
    let first = room.saturating_sub(beside);
    let tip = |clipped: bool| clipped.then(|| whole_path(landing, file));
    if cells_of(&whole) <= first || one || crumbs.len() < 2 {
        let (runs, clipped) = clip_runs(&whole, first);
        return vec![Low::Place { runs, cont: false, change, tip: tip(clipped) }];
    }
    let mut rows = path_rows(lead.clone(), &crumbs, room);
    if rows.len() == 1 {
        // The sentence fits the row only where `d change` does not stand:
        // its last crumb goes down beside it.
        let (head, last) = crumbs.split_at(crumbs.len() - 1);
        let mut top = path_rows(lead, head, room).remove(0);
        top.push((crumb_sep().trim_end().to_string(), dim()));
        rows = vec![top, last[0].clone()];
    }
    let (top, top_cut) = clip_runs(&rows[0], room);
    let (rest, rest_cut) = clip_runs(&rows[1], room.saturating_sub(usize::from(CONT_X) + beside));
    let tip = tip(top_cut || rest_cut);
    vec![
        Low::Place { runs: top, cont: false, change: false, tip: tip.clone() },
        Low::Place { runs: rest, cont: true, change, tip },
    ]
}

/// A place's slot note (§4.5): its own DIM rows under the place, two at
/// most (one where the give-way asks), the whole note the tooltip where cut.
fn note_lows(low: &mut Vec<Low>, landing: &Landing, room: usize, one: bool) {
    if let Some(note) = slot_note(landing) {
        lines(low, &note, dim(), room, 0, &[], one);
    }
}

/// Words as rows of one token at col 1 — a hanging `hang` after the first
/// (a headline, a GOLD line).
fn lines(low: &mut Vec<Low>, text: &str, style: Style, room: usize, hang: u16, keep: &[&str], one: bool) {
    let (rows, tip) = wrap_rows(text, room, usize::from(hang), keep, one);
    for (i, row) in rows.into_iter().enumerate() {
        low.push(Low::Text { runs: vec![(row, style)], cont: i > 0 && hang > 0, tip: tip.clone() });
    }
}

/// An outcome's headline (§4.7): its glyph in code, its words the locale's
/// — `✓ In your collection` OK BOLD; `! …` GOLD BOLD; a neutral end's
/// glyph DIM and its words TEXT BOLD — wrapped at col 3, two rows at most.
fn headline(low: &mut Vec<Low>, glyph: Option<(&str, Style)>, words: &str, style: Style, room: usize, keep: &[&str], one: bool) {
    let text = match glyph {
        Some((glyph, _)) => format!("{glyph} {words}"),
        None => words.to_string(),
    };
    let (rows, tip) = wrap_rows(&text, room, usize::from(CONT_X), keep, one);
    for (i, row) in rows.into_iter().enumerate() {
        let runs = match glyph {
            Some((glyph, glyph_style)) if i == 0 && row.starts_with(glyph) => {
                vec![(glyph.to_string(), glyph_style), (row[glyph.len()..].to_string(), style)]
            }
            _ => vec![(row, style)],
        };
        low.push(Low::Text { runs, cont: i > 0, tip: tip.clone() });
    }
}

/// The action row (§4.8): the key words from col 1, [`KEY_GAP`] apart,
/// the primary right-aligned — the key words on the row above where the
/// two would come closer than that.
fn action(low: &mut Vec<Low>, keys: Vec<KeyWord>, primary: Primary, room: usize) {
    let keys_w: usize =
        keys.iter().map(|k| width(&k.words())).sum::<usize>() + KEY_GAP * keys.len().saturating_sub(1);
    if !keys.is_empty() && keys_w + KEY_GAP + width(&primary.label()) > room {
        low.push(Low::Keys(keys));
        low.push(Low::Action { keys: Vec::new(), primary });
    } else {
        low.push(Low::Action { keys, primary });
    }
}

/// A choosing state's rows (§5.1–5.2): the radios and the box where the
/// subject offers two choices or more, then — after a blank — the place
/// with `d change`, its slot note and `aside` (a refusal, a moved place,
/// another server's copy); the key words and the primary. Starting draws
/// the same, its primary off and its controls inert ([`draw_low`]).
fn choosing(low: &mut Vec<Low>, view: &KeepView, room: usize, fit: Fit, aside: Vec<Low>, keys: Vec<KeyWord>, primary: Primary) {
    let radios = view.choices.len() >= 2;
    if radios {
        for (choice, _) in &view.choices {
            low.push(Low::Choice(*choice));
            if *choice == KeepChoice::Artist
                && let Some((on, count)) = view.missing
            {
                low.push(Low::Missing { on, count });
            }
        }
    }
    let mut body = Vec::new();
    if let Some(landing) = &view.landing {
        body.extend(place_lows(&t!("gui.discover.keep.goes_to"), landing, None, true, room, fit.place));
        note_lows(&mut body, landing, room, fit.note);
    }
    body.extend(aside);
    if radios && !body.is_empty() {
        low.push(Low::Blank(Gap::B2));
    }
    low.extend(body);
    low.push(Low::Blank(Gap::B3));
    action(low, keys, primary, room);
}

/// Whose wider copy speaks for this choice (§5.4), as its sub says it: a
/// song your album copy kept; an album your copy of the artist's albums,
/// or of their missing ones, walked.
fn borrowed_words(view: &KeepView) -> Option<String> {
    let artist = view.subject.track_artist.clone().unwrap_or_default();
    match view.borrowed? {
        JobScope::Album => Some(t!("gui.discover.keep.with_album").to_string()),
        JobScope::Artist => Some(t!("gui.discover.keep.with_artist", artist = artist).to_string()),
        JobScope::ArtistMissing => Some(t!("gui.discover.keep.with_missing", artist = artist).to_string()),
        _ => None,
    }
}

/// A sub with whose copy spoke after it (" · with your copy of …") — or
/// alone, its first letter raised.
fn with_borrowed(sub: Option<String>, view: &KeepView) -> Option<String> {
    match (sub, borrowed_words(view)) {
        (Some(sub), Some(whose)) => Some(format!("{sub} · {whose}")),
        (None, Some(whose)) => Some(raised(&whose)),
        (sub, None) => sub,
    }
}

/// A many-song copy's counts as its sub says them (§5.4): copied, already
/// yours, albums left out, zeros dropped — the gap is the headline's.
fn kept_counts(n: &ManyCounts) -> Option<String> {
    let mut parts = Vec::new();
    if n.copied > 0 {
        parts.push(count("gui.discover.keep.copied", n.copied));
    }
    if n.skipped > 0 {
        parts.push(count("gui.discover.keep.owned", n.skipped));
    }
    if n.albums_left_out > 0 {
        parts.push(count("gui.discover.keep.left_out", n.albums_left_out as u64));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A many-song copy's gap as its headline says it (done ≠ kept, made
/// visible): the songs that did not copy — failed or never reached — else
/// the albums whose listing failed.
fn gap_words(n: &ManyCounts) -> Option<String> {
    let songs = n.failed.max(n.total.saturating_sub(n.copied + n.skipped));
    if songs > 0 {
        return Some(count("gui.discover.keep.gap_songs", songs));
    }
    (n.albums_failed > 0).then(|| count("gui.discover.keep.gap_albums", n.albums_failed as u64))
}

/// "1 copied", or "Nothing was copied".
fn copied_words(copied: u64) -> String {
    if copied > 0 { count("gui.discover.keep.copied", copied) } else { t!("gui.discover.keep.nothing_copied").to_string() }
}

/// Whether an ended copy left files where its "In" row would say (§4.4):
/// whatever it copied; what it found already there, or in the way, where
/// the result names the folder. Failed, refused or cancelled before a
/// song, nothing to copy, a result this client cannot read: no row claims
/// files landed.
fn landed_any(job: &PluginJob, outcome: &JobOutcome) -> bool {
    let copied = |many: &Option<ManyCounts>| many.is_some_and(|n| n.copied > 0);
    match outcome {
        JobOutcome::Copied { .. } | JobOutcome::CopiedMany(_) => true,
        JobOutcome::Owned { .. } | JobOutcome::Exists { .. } => job.copy_result().landed().is_some_and(|l| l.vpath.is_some()),
        JobOutcome::Stopped { many, .. } => many.copied > 0,
        JobOutcome::Refused { many } | JobOutcome::Cancelled { many } | JobOutcome::Failed { many, .. } => copied(many),
        _ => false,
    }
}

/// A copy's rows (§5.3–5.4): its two-line progress, or its outcome's
/// headline and subs; then — after a blank — where it lands ("Into" its
/// pinned place while it runs, "In" where its files landed, a song's file
/// its last crumb), "Retry adds to" where Retry would land elsewhere, a
/// slot's note; the key words and the primary of the window's own table.
#[allow(clippy::too_many_arguments)]
fn job_lows(
    low: &mut Vec<Low>,
    view: &KeepView,
    job: &PluginJob,
    outcome: &JobOutcome,
    act: Option<JobAction>,
    shown: Shown,
    room: usize,
    fit: Fit,
) {
    let server = job.peer_name().unwrap_or_else(|| view.subject.server.clone());
    let keep = [server.as_str(), view.subject.server.as_str()];
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let ok = Style::default().fg(th().ok).add_modifier(Modifier::BOLD);
    let gold = Style::default().fg(th().gold).add_modifier(Modifier::BOLD);
    let quiet = Some((tick(), dim()));
    let live = outcome.live();
    let halting = stopping(outcome, shown);
    let one = fit.lines;
    // The headline, and the DIM lines under it.
    let mut subs: Vec<String> = Vec::new();
    match outcome {
        JobOutcome::Queued | JobOutcome::Running { .. } => {
            let (head, step, progress, tail) = match outcome {
                JobOutcome::Running { progress, .. } if halting => (t!("gui.discover.keep.stopping"), None, *progress, None),
                JobOutcome::Running { progress, step, .. } => (t!("gui.discover.keep.copying"), step.clone(), *progress, None),
                _ if halting => (t!("gui.discover.keep.stopping"), None, None, None),
                _ => (t!("gui.discover.keep.waiting"), None, None, Some(t!("gui.discover.keep.waiting_sub").to_string())),
            };
            // One row: the server's step stays its words, clipped — the
            // whole of them the row's tooltip.
            let mut runs = vec![(ell(&head), bold)];
            if let Some(step) = step {
                runs.push((" · ".to_string(), dim()));
                runs.push((step, Style::default()));
            }
            let whole: String = runs.iter().map(|(t, _)| t.as_str()).collect();
            let (runs, clipped) = clip_runs(&runs, room);
            let tip = clipped.then_some(whole);
            low.push(Low::Text { runs, cont: false, tip });
            low.push(Low::Bar { progress, tail });
        }
        JobOutcome::Copied { .. } => {
            headline(low, Some((tick(), ok)), &t!("gui.discover.keep.kept"), ok, room, &keep, one);
            let from = view.copied_from.as_ref().map(|server| t!("gui.discover.keep.copied_from", server = server).to_string());
            subs.extend(with_borrowed(from, view));
        }
        JobOutcome::CopiedMany(n) => {
            let gap = gap_words(n).filter(|_| act == Some(JobAction::Retry));
            match &gap {
                Some(gap) => headline(low, Some(("!", gold)), gap, gold, room, &keep, one),
                None => headline(low, Some((tick(), ok)), &t!("gui.discover.keep.kept"), ok, room, &keep, one),
            }
            subs.extend(with_borrowed(kept_counts(n), view));
        }
        JobOutcome::Owned { many, .. } => {
            headline(low, quiet, &t!("gui.discover.keep.yours"), bold, room, &keep, one);
            let has = if many.is_some() { t!("gui.discover.keep.yours_many") } else { t!("gui.discover.keep.yours_song") };
            subs.extend(with_borrowed(Some(has.to_string()), view));
        }
        JobOutcome::Exists { .. } => {
            headline(low, Some(("!", gold)), &t!("gui.discover.keep.exists"), gold, room, &keep, one);
            subs.push(t!("gui.discover.keep.exists_sub").to_string());
        }
        JobOutcome::Stopped { why, many } => {
            let reason = match why {
                StopReason::Quota => t!("gui.discover.keep.stopped_quota", server = server),
                StopReason::Busy => t!("gui.discover.keep.stopped_busy", server = server),
                StopReason::Peer | StopReason::Refused => t!("gui.discover.keep.stopped_peer", server = server),
                StopReason::Other => t!("gui.discover.keep.stopped_other"),
            };
            headline(low, Some(("!", gold)), &reason, gold, room, &keep, one);
            let not = many.total.saturating_sub(many.copied + many.skipped);
            let mut parts = Vec::new();
            if many.copied > 0 {
                parts.push(count("gui.discover.keep.copied", many.copied));
            }
            if not > 0 {
                parts.push(count("gui.discover.keep.not_copied", not));
            }
            if parts.is_empty() {
                parts.push(t!("gui.discover.keep.nothing_copied").to_string());
            }
            match why {
                StopReason::Quota => parts.push(t!("gui.discover.keep.later").to_string()),
                StopReason::Busy => parts.push(t!("gui.discover.keep.moment").to_string()),
                _ => {}
            }
            subs.push(parts.join(" · "));
        }
        JobOutcome::Refused { many } => {
            headline(low, Some(("!", gold)), &t!("gui.discover.keep.refused", server = server), gold, room, &keep, one);
            let copied = many.map(|n| n.copied).unwrap_or(0);
            subs.push(format!("{} · {}", copied_words(copied), t!("gui.discover.keep.refused_sub")));
            subs.push(t!("gui.discover.keep.refused_admin", server = server).to_string());
        }
        JobOutcome::Failed { error, many } => {
            let words = match error {
                Some(words) => t!("gui.discover.keep.failed", words = words).to_string(),
                None => t!("gui.discover.keep.failed_plain").to_string(),
            };
            headline(low, Some(("!", gold)), &words, gold, room, &keep, one);
            let mut parts = vec![copied_words(many.map(|n| n.copied).unwrap_or(0))];
            if let Some(n) = many.filter(|n| n.failed > 0) {
                parts.push(count("gui.discover.keep.failed", n.failed));
            }
            subs.push(parts.join(" · "));
        }
        JobOutcome::Cancelled { many } => {
            headline(low, Some((cross(), dim())), &t!("gui.discover.keep.cancelled"), bold, room, &keep, one);
            let sub = match many {
                // What it took, and what it left there: the rest of the album.
                Some(n) if n.copied > 0 => {
                    let rest = n.total.saturating_sub(n.copied + n.skipped);
                    let mut parts = vec![count("gui.discover.keep.copied", n.copied)];
                    if rest > 0 {
                        let key = format!("gui.discover.keep.stay_{}", super::discover::plural_form(rest));
                        parts.push(t!(key.as_str(), count = rest, server = server).to_string());
                    }
                    parts.join(" · ")
                }
                _ => t!("gui.discover.keep.nothing_copied").to_string(),
            };
            subs.push(sub);
        }
        JobOutcome::Nothing { appearances, .. } => {
            if *appearances > 0 {
                headline(low, None, &t!("gui.discover.keep.nothing_theirs"), bold, room, &keep, one);
                let artist = view.subject.track_artist.clone().unwrap_or_default();
                subs.push(t!("gui.discover.keep.appearances", artist = artist).to_string());
            } else {
                headline(low, None, &t!("gui.discover.keep.nothing"), bold, room, &keep, one);
                subs.push(t!("gui.discover.keep.all_yours").to_string());
            }
        }
        JobOutcome::Done | JobOutcome::Unknown => {
            headline(low, None, &t!("gui.discover.keep.finished"), bold, room, &keep, one);
        }
    }
    for sub in &subs {
        lines(low, sub, dim(), room, 0, &keep, one);
    }
    // Where it lands, and where Retry would.
    let mut body = Vec::new();
    if live {
        if let Some(landing) = &view.landing {
            body.extend(place_lows(&t!("gui.discover.keep.into"), landing, None, false, room, fit.place));
            note_lows(&mut body, landing, room, fit.note);
        }
    } else {
        if let Some(landing) = view.landing.as_ref().filter(|_| landed_any(job, outcome)) {
            // A song's own file is the place's last crumb.
            let file = match outcome {
                JobOutcome::Copied { .. } | JobOutcome::Exists { .. } | JobOutcome::Owned { many: None, .. } => {
                    job.copy_result().landed().and_then(|l| l.file)
                }
                _ => None,
            };
            body.extend(place_lows(&t!("gui.discover.keep.in_place"), landing, file.as_deref(), false, room, fit.place));
        }
        if let Some(again) = &view.again {
            body.extend(place_lows(&t!("gui.discover.dest.retry_adds_to"), again, None, false, room, fit.place));
            note_lows(&mut body, again, room, fit.note);
        }
    }
    if !body.is_empty() {
        low.push(Low::Blank(Gap::B2));
    }
    low.extend(body);
    low.push(Low::Blank(Gap::B3));
    // The key words and the primary (§5.4): what works, and only that.
    let (keys, primary) = if shown.retrying {
        (Vec::new(), Primary { words: ell(&t!("gui.discover.keep.retrying")), press: None })
    } else if live {
        let keys = if act == Some(JobAction::Cancel) && !halting { vec![KeyWord::Cancel] } else { Vec::new() };
        (keys, Primary::close())
    } else {
        let mut keys = Vec::new();
        if matches!(outcome, JobOutcome::Exists { .. }) && view.destination.is_some() {
            keys.push(KeyWord::Change);
        }
        if matches!(outcome, JobOutcome::Copied { .. }) && act == Some(JobAction::Play) {
            keys.push(KeyWord::Queue);
        }
        if view.choose_again() {
            keys.push(KeyWord::Choose);
        }
        let primary = match act {
            Some(JobAction::Play) => Primary { words: t!("gui.discover.keep.play_copy").to_string(), press: Some(Press::Again(JobAction::Play)) },
            Some(again @ (JobAction::Retry | JobAction::CopyAgain)) => {
                Primary { words: t!("gui.discover.keep.retry").to_string(), press: Some(Press::Again(again)) }
            }
            Some(JobAction::StartAgain) => {
                Primary { words: t!("gui.discover.keep.add_again").to_string(), press: Some(Press::Again(JobAction::StartAgain)) }
            }
            Some(JobAction::Cancel) | None => Primary::close(),
        };
        (keys, primary)
    };
    action(low, keys, primary, room);
}

/// The window's rows under its head for a view (§3.3, §4, §5), as far as
/// the give-way went: B1, the body, B3, the action row.
fn lower(view: &KeepView, shown: Shown, room: usize, fit: Fit) -> Vec<Low> {
    let mut low = vec![Low::Blank(Gap::B1)];
    let server = view.subject.server.clone();
    match &view.stage {
        KeepStage::Choose { verb, other } => {
            let (aside, keys, primary) = match (verb, other) {
                // Another server's live copy covers the choice: whose, and
                // its cancel — the primary Close until it ends.
                (_, Some(other)) => {
                    let mut aside = Vec::new();
                    let title = t!("gui.discover.keep.other_title", server = other.server);
                    lines(&mut aside, &title, Style::default().add_modifier(Modifier::BOLD), room, 0, &[other.server.as_str()], fit.lines);
                    let sub = t!("gui.discover.keep.other_sub", server = server);
                    lines(&mut aside, &sub, dim(), room, 0, &[server.as_str()], fit.lines);
                    let keys = if other.cancelling { Vec::new() } else { vec![KeyWord::CancelOther] };
                    (aside, keys, Primary::close())
                }
                (Some(verb), None) => {
                    (Vec::new(), Vec::new(), Primary { words: add_words(*verb, view), press: Some(Press::Add) })
                }
                (None, None) => (Vec::new(), Vec::new(), Primary::close()),
            };
            choosing(&mut low, view, room, fit, aside, keys, primary);
        }
        KeepStage::Refused { failure, verb } => {
            // A place no longer in force has its own line: the place above
            // already reads the one that is (clause 146).
            let words = match failure {
                crate::api::PluginFailure::DestinationChanged(_) => t!("gui.discover.dest.changed").to_string(),
                failure => t!("gui.discover.keep.refused_start", words = failure_words(failure)).to_string(),
            };
            let mut aside = Vec::new();
            let gold = Style::default().fg(th().gold);
            lines(&mut aside, &format!("! {words}"), gold, room, CONT_X, &[server.as_str()], fit.lines);
            let primary = match verb {
                Some(verb) => Primary { words: add_words(*verb, view), press: Some(Press::Add) },
                None => Primary::close(),
            };
            choosing(&mut low, view, room, fit, aside, Vec::new(), primary);
        }
        // Enter pressed: nothing moves but the primary, off until the
        // server answers.
        KeepStage::Starting => {
            let primary = Primary { words: ell(&t!("gui.discover.keep.adding")), press: None };
            choosing(&mut low, view, room, fit, Vec::new(), Vec::new(), primary);
        }
        // A right gone: one plain line and Close (clause 147).
        KeepStage::Off(why) => {
            lines(&mut low, &off_words(*why), Style::default(), room, 0, &[], fit.lines);
            low.push(Low::Blank(Gap::B3));
            action(&mut low, Vec::new(), Primary::close(), room);
        }
        KeepStage::Job { job, outcome, action } => job_lows(&mut low, view, job, outcome, *action, shown, room, fit),
    }
    low.retain(|line| !matches!(line, Low::Blank(gap) if fit.drops(*gap)));
    low
}

/// The window's rows in the rows it has (§3.6): as they are where they fit
/// under the anchored top; else the blanks go — B2, B1, B3 — and the place
/// keeps one row; then the top rises toward row 1; then the slot note, and
/// the GOLD line or the headline, keep one row each. Where even that is too
/// tall the body's last rows above the action row go, so the action row
/// and both borders always stand.
fn fitted(view: &KeepView, shown: Shown, room: usize, anchored: u16, risen: u16) -> Vec<Low> {
    let fits = |low: &[Low], rows: u16| HEAD_ROWS + 2 + low.len() as u16 <= rows;
    let tight = Fit { blanks: 3, place: true, ..Fit::default() };
    let under = [0, 1, 2, 3].map(|blanks| Fit { blanks, ..Fit::default() });
    for fit in under.into_iter().chain([tight]) {
        let low = lower(view, shown, room, fit);
        if fits(&low, anchored) {
            return low;
        }
    }
    let tightest = Fit { note: true, lines: true, ..tight };
    for fit in [tight, Fit { note: true, ..tight }, tightest] {
        let low = lower(view, shown, room, fit);
        if fits(&low, risen) {
            return low;
        }
    }
    let mut low = lower(view, shown, room, tightest);
    while !fits(&low, risen) && low.len() > 1 {
        low.remove(low.len() - 2);
    }
    low
}

/// The primary a table stands, if any.
fn primary_of(low: &[Low]) -> Option<&Primary> {
    low.iter().find_map(|line| match line {
        Low::Action { primary, .. } => Some(primary),
        _ => None,
    })
}

/// The key words a table draws — and so the keys that work (§6.2): its
/// action row's and their own row's, and `d change` on the place while
/// nothing is starting.
fn key_words(low: &[Low], view: &KeepView) -> Vec<KeyWord> {
    let starting = matches!(view.stage, KeepStage::Starting);
    let mut out = Vec::new();
    for line in low {
        match line {
            Low::Place { change: true, .. } if !starting => out.push(KeyWord::Change),
            Low::Keys(keys) | Low::Action { keys, .. } => out.extend(keys.iter().copied()),
            _ => {}
        }
    }
    out
}

/// The key words that act now (§6.2): those the view's table draws — and,
/// where the last frame drew this very view, only those it drew there (the
/// give-way's last resort may have taken their row).
fn acting_keys(gui: &Gui, view: &KeepView, shown: Shown) -> Vec<KeyWord> {
    let low = lower(view, shown, usize::from(KEEP_W - 4), Fit::default());
    let mut keys = key_words(&low, view);
    if let Some((drawn_view, drawn_shown, drawn)) = &gui.discover.keep.drawn
        && drawn_view == view
        && *drawn_shown == shown
    {
        keys.retain(|key| drawn.contains(key));
    }
    keys
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The legacy console's frame: `+ - |` (clause 124); elsewhere the kit's
/// rounded one. Clear beneath, the ground repainted, the footprint the
/// caller registered.
pub(crate) fn modal_box(frame: &mut Frame, rect: Rect) -> Rect {
    if !legacy_conhost() {
        return crate::kit::frame_at(frame, rect, th().accent);
    }
    crate::kit::blank(frame, rect);
    let set = ratatui::symbols::border::Set {
        top_left: "+",
        top_right: "+",
        bottom_left: "+",
        bottom_right: "+",
        vertical_left: "|",
        vertical_right: "|",
        horizontal_top: "-",
        horizontal_bottom: "-",
    };
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_set(set)
        .border_style(Style::default().fg(th().accent));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

/// A wide glyph of the room's just left of `rect` spans `rect`'s left
/// border, and a terminal never draws the cell a wide glyph spans: a blank
/// in its place on each of `rect`'s rows.
fn clear_spill(frame: &mut Frame, rect: Rect) {
    let Some(x) = rect.x.checked_sub(1) else { return };
    let buffer = frame.buffer_mut();
    for y in rect.top()..rect.bottom() {
        let cell = &mut buffer[(x, y)];
        if width(cell.symbol()) > 1 {
            cell.set_symbol(" ");
        }
    }
}

/// The Keep a copy window, drawn (and registered) over everything — or
/// Where copies land in its place while that is open (clause 142). `room`
/// is the room's content rect: the window centres in it and clears its
/// width (the band).
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect, room: Rect) {
    // A picker whose view went (the gate's ask failed), or whose place is
    // nowhere any more (a re-read's `null`: nothing to edit, as the
    // webapp's `dmPickerSync` closes it), gives way to Keep a copy:
    // nothing would draw it, and it would keep every key.
    if gui.discover.keep.dest.is_some() && gui.app.keep_dest_view().is_none_or(|view| view.destination.is_none()) {
        close_dest(gui);
    }
    if gui.discover.keep.dest.is_some() {
        draw_dest(frame, gui, area);
        return;
    }
    // Retry's outcome holds only while its start is out.
    if !gui.app.keep_view().is_some_and(|view| matches!(view.stage, KeepStage::Starting)) {
        gui.discover.keep.frozen = None;
    }
    let Some((view, shown)) = shown_of(gui) else { return };
    // Its subject's cover, asked through its own server once.
    let fetch = gui.app.keep_cover();
    gui.pend(fetch.into_iter().collect());
    let w = window_width(area, room, KEEP_W);
    let text = usize::from(w.saturating_sub(4));
    let low = fitted(&view, shown, text, rows_anchored(area, room), rows_risen(area, room));
    let (rect, band) = window_rect(area, room, KEEP_W, HEAD_ROWS + 2 + low.len() as u16);
    let (inner, own) = open_window(frame, gui, area, rect, band);
    let footer = if going(&view, shown) { t!("gui.discover.keep.esc_close_going") } else { t!("gui.discover.keep.esc_close") };
    draw_borders(frame, gui, rect, &t!("gui.discover.keep.title"), &footer, Act::Disc(DiscAct::Keep(KeepAct::Close)));
    draw_head(frame, gui, &view, inner, own);
    gui.discover.keep.last_own = Some(own);
    let starting = matches!(view.stage, KeepStage::Starting);
    for (y, line) in (inner.y + HEAD_ROWS..inner.bottom()).zip(low.iter()) {
        draw_low(frame, gui, &view, inner, y, line, starting);
    }
    let keys = key_words(&low, &view);
    gui.discover.keep.drawn = Some((view, shown, keys));
}

/// The title on the top border, TEXT BOLD after one `─`; the way out on
/// the bottom border, DIM, ending one `─` before the corner — "Esc close"
/// a text button doing what Esc does (`close`), its tail ("· the copy
/// keeps going") plain. Keep a copy's, and Where copies land's.
fn draw_borders(frame: &mut Frame, gui: &mut Gui, rect: Rect, title: &str, footer: &str, close: Act) {
    border_title(frame, rect, title);
    let bottom = rect.bottom().saturating_sub(1);
    let end = rect.right().saturating_sub(3);
    let start = end.saturating_sub(width(footer) as u16);
    put(frame, start.saturating_sub(1), bottom, &format!(" {footer} "), dim());
    let esc = t!("gui.discover.keep.esc_close").to_string();
    let button = if footer.starts_with(esc.as_str()) { esc } else { footer.to_string() };
    let at = Rect { x: start, y: bottom, width: width(&button) as u16, height: 1 };
    if gui.ui.hovers(at) {
        put(frame, start, bottom, &button, bright_bold());
    }
    gui.ui.click(at, close);
}

/// The title on a window's top border, ` {title} ` from two cells in —
/// after the corner and one `─` — TEXT BOLD: Keep a copy's, Where copies
/// land's and the mood Filters' (library-rooms clause 32).
pub(crate) fn border_title(frame: &mut Frame, rect: Rect, title: &str) {
    // TEXT, never the border's ACCENT carried over: the window's own text
    // colour — the scheme's where it paints the ground, else the terminal's.
    let owned = th().ground.is_some() && crate::kit::theme::ground_owned();
    let text = if owned { th().text } else { ratatui::style::Color::Reset };
    put(frame, rect.x + 2, rect.y, &format!(" {title} "), Style::default().fg(text).add_modifier(Modifier::BOLD));
}

/// The head (§4.2): the item's title TEXT BOLD, the byline DIM — and the
/// cover, 4 × 2 cells of real pixels at the head's right end, only where
/// the art is loaded and the terminal draws pixels (the kit's one-frame
/// mosaic under an overlay passing over it); otherwise nothing, and the
/// words take the whole row.
fn draw_head(frame: &mut Frame, gui: &mut Gui, view: &KeepView, inner: Rect, own: Rect) {
    let s = &view.subject;
    let x = inner.x + 1;
    let mut room = usize::from(inner.width.saturating_sub(2));
    {
        let Gui { app, discover, ui, hot, .. } = &mut *gui;
        let pixels = !legacy_conhost() && (app.graphics.protocol().is_some() || app.graphics.is_hosted());
        let art = s.art.as_deref().and_then(|file| app.art.get(file)).and_then(|a| a.as_ref()).filter(|_| pixels);
        if let Some(art) = art {
            let cover = Rect { x: x + room as u16 - 4, y: inner.y, width: 4, height: 2 };
            // Its own footprint, this frame's and last frame's — the
            // window grows and shrinks with its rows — is never another.
            let owns: Vec<Rect> = [Some(own), discover.keep.last_own].into_iter().flatten().collect();
            let slot = discover.keep.slot.get_or_insert_with(|| Slot::new(app.graphics.fork()));
            if ui.covered_last_frame_by_other_than(cover, &owns) {
                slot.draw_mosaic(frame, cover, art);
            } else {
                let pace = Pace::frame();
                slot.draw_paced(frame, cover, art, &pace);
                if pace.starved() {
                    *hot = true;
                }
            }
            room = room.saturating_sub(5);
        }
    }
    let title = match &s.title {
        Some(title) => title.clone(),
        None => s.album.clone().unwrap_or_default(),
    };
    put(frame, x, inner.y, &super::bar::clip(&title, room), Style::default().add_modifier(Modifier::BOLD));
    let from = t!("gui.act.from_server", server = s.server).to_string();
    let parts: Vec<String> = if s.album_focus() {
        let year = s.year.filter(|y| *y > 0).map(|y| y.to_string());
        [s.album_credit().map(str::to_string), year, Some(from)].into_iter().flatten().collect()
    } else {
        [s.track_artist.clone(), s.album.clone(), Some(from)].into_iter().flatten().filter(|p| !p.trim().is_empty()).collect()
    };
    put(frame, x, inner.y + 1, &super::bar::clip(&parts.join(" · "), room), dim());
}

/// Runs drawn from `x`, clipped at `end`; the cell after the last.
fn put_runs(frame: &mut Frame, x: u16, y: u16, runs: &[(String, Style)], end: u16) -> u16 {
    let mut at = x;
    for (text, style) in runs {
        let left = usize::from(end.saturating_sub(at));
        if left == 0 {
            break;
        }
        let shown = super::bar::clip(text, left).into_owned();
        put(frame, at, y, &shown, *style);
        at += width(&shown) as u16;
    }
    at
}

/// A key word at `x` (§4.8): DIM, BRIGHT BOLD under the pointer, a click
/// doing what its key does — none of it while the window is starting
/// (`live` false). Its width.
fn draw_key(frame: &mut Frame, gui: &mut Gui, word: KeyWord, x: u16, y: u16, live: bool) -> u16 {
    let words = word.words();
    let at = Rect { x, y, width: width(&words) as u16, height: 1 };
    let style = if live && gui.ui.hovers(at) { bright_bold() } else { dim() };
    put(frame, x, y, &words, style);
    if live {
        gui.ui.click(at, Act::Disc(DiscAct::Keep(KeepAct::Key(word))));
    }
    at.width
}

/// One row of the table. Nothing in it takes the pointer while a start is
/// out (`starting`): the room under it is inert as ever.
fn draw_low(frame: &mut Frame, gui: &mut Gui, view: &KeepView, inner: Rect, y: u16, line: &Low, starting: bool) {
    let x = inner.x + 1;
    let room = inner.width.saturating_sub(2);
    let end = x + room;
    let bold = Style::default().add_modifier(Modifier::BOLD);
    match line {
        Low::Blank(_) => {}
        Low::Choice(choice) => {
            // The keyboard's place is the chosen dot: ACCENT, its label
            // BOLD; the pointer lights a label, never the dot.
            let row = Rect { x, y, width: room, height: 1 };
            let chosen = view.choice == *choice;
            let hover = !starting && gui.ui.hovers(row);
            let (on, off) = if legacy_conhost() { ("(*)", "( )") } else { ("(•)", "( )") };
            put(frame, x, y, if chosen { on } else { off }, if chosen { accent() } else { dim() });
            let style = match (hover, chosen) {
                (true, _) => bright_bold(),
                (false, true) => bold,
                (false, false) => Style::default(),
            };
            let name = choice_name(*choice, view);
            put(frame, x + LABEL_X, y, &super::bar::clip(&name, usize::from(room - LABEL_X)), style);
            if !starting {
                gui.ui.click(row, Act::Disc(DiscAct::Keep(KeepAct::Pick(*choice))));
            }
        }
        Low::Missing { on, count } => {
            // The choice's fourth stop: ticked ACCENT, its label BOLD — its
            // count where one is known: a copy of it that still wants the
            // user keeps it with none missing by name, and says none.
            let (check_on, check_off) = super::check_glyphs();
            let words = match count {
                0 => t!("gui.discover.keep.only_missing").to_string(),
                n => plural("gui.discover.keep.only_missing", *n as u64),
            };
            let words = super::bar::clip(&words, usize::from(room - BOX_LABEL_X)).into_owned();
            let at = Rect { x: x + LABEL_X, y, width: BOX_LABEL_X - LABEL_X + width(&words) as u16, height: 1 };
            let hover = !starting && gui.ui.hovers(at);
            put(frame, at.x, y, if *on { check_on } else { check_off }, if *on { accent() } else { dim() });
            let style = match (hover, *on) {
                (true, _) => bright_bold(),
                (false, true) => bold,
                (false, false) => Style::default(),
            };
            put(frame, x + BOX_LABEL_X, y, &words, style);
            if !starting {
                gui.ui.click(at, Act::Disc(DiscAct::Keep(KeepAct::Missing)));
            }
        }
        Low::Place { runs, cont, change, tip } => {
            let at = if *cont { x + CONT_X } else { x };
            let words = KeyWord::Change.words();
            let text_end = if *change { end.saturating_sub(width(&words) as u16 + CHANGE_GAP as u16) } else { end };
            put_runs(frame, at, y, runs, text_end);
            if let Some(tip) = tip {
                gui.ui.tip(Rect { x: at, y, width: text_end.saturating_sub(at), height: 1 }, tip.clone());
            }
            if *change {
                draw_key(frame, gui, KeyWord::Change, end.saturating_sub(width(&words) as u16), y, !starting);
            }
        }
        Low::Text { runs, cont, tip } => {
            let at = if *cont { x + CONT_X } else { x };
            put_runs(frame, at, y, runs, end);
            if let Some(tip) = tip {
                gui.ui.tip(Rect { x: at, y, width: end.saturating_sub(at), height: 1 }, tip.clone());
            }
        }
        Low::Bar { progress, tail } => draw_progress(frame, x, y, *progress, tail.as_deref()),
        Low::Keys(keys) => {
            let mut at = x;
            for word in keys {
                at += draw_key(frame, gui, *word, at, y, true) + KEY_GAP as u16;
            }
        }
        Low::Action { keys, primary } => {
            let mut at = x;
            for word in keys {
                at += draw_key(frame, gui, *word, at, y, true) + KEY_GAP as u16;
            }
            // The one primary, ACCENT BOLD — or off, DIM and inert, while
            // a start it sent is out.
            let label = primary.label();
            let rect = Rect { x: end.saturating_sub(width(&label) as u16), y, width: width(&label) as u16, height: 1 };
            match primary.press {
                Some(_) => {
                    let style = if gui.ui.hovers(rect) { bright_bold() } else { accent().add_modifier(Modifier::BOLD) };
                    put(frame, rect.x, y, &label, style);
                    gui.ui.click(rect, Act::Disc(DiscAct::Keep(KeepAct::Primary)));
                }
                None => put(frame, rect.x, y, &label, dim()),
            }
        }
    }
}

/// The copy's bar (the kit's Status, progress): ten cells, `▰` OK filled
/// and `▱` DIM, the percentage TEXT beside them; all DIM with no
/// percentage while there is no estimate — never a fake 0% — and `tail`
/// DIM after it. The legacy console says it in words.
fn draw_progress(frame: &mut Frame, x: u16, y: u16, progress: Option<f64>, tail: Option<&str>) {
    if legacy_conhost() {
        let (words, style) = match progress {
            Some(p) => (t!("gui.discover.keep.pct_done", pct = (p * 100.0).round() as u32).to_string(), Style::default()),
            None => (t!("gui.discover.keep.no_estimate").to_string(), dim()),
        };
        put(frame, x, y, &words, style);
        if let Some(tail) = tail {
            put(frame, x + width(&words) as u16, y, &format!(" · {tail}"), dim());
        }
        return;
    }
    let filled = progress.map_or(0, |p| ((p * 10.0).round() as usize).min(10));
    put(frame, x, y, &"▰".repeat(filled), Style::default().fg(th().ok));
    put(frame, x + filled as u16, y, &"▱".repeat(10 - filled), dim());
    match (progress, tail) {
        (Some(p), _) => put(frame, x + 11, y, &format!("{}%", (p * 100.0).round() as u32), Style::default()),
        (None, Some(tail)) => put(frame, x + 11, y, tail, dim()),
        (None, None) => {}
    }
}

/// The Copies list's bar: [`draw_progress`]'s, its percentage DIM.
fn draw_bar(frame: &mut Frame, x: u16, y: u16, progress: Option<f64>) {
    if legacy_conhost() {
        let words = match progress {
            Some(p) => t!("gui.discover.keep.pct_done", pct = (p * 100.0).round() as u32).to_string(),
            None => t!("gui.discover.keep.no_estimate").to_string(),
        };
        put(frame, x, y, &words, dim());
        return;
    }
    let filled = progress.map_or(0, |p| ((p * 10.0).round() as usize).min(10));
    put(frame, x, y, &"▰".repeat(filled), Style::default().fg(th().ok));
    put(frame, x + filled as u16, y, &"▱".repeat(10 - filled), dim());
    if let Some(p) = progress {
        put(frame, x + 11, y, &format!("{}%", (p * 100.0).round() as u32), dim());
    }
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// Open Keep a copy on a target (clauses 102–103): the App's flow, the dot
/// on the preselected choice.
pub(crate) fn open(gui: &mut Gui, target: crate::tui::app::keep::KeepTarget) {
    let effects = gui.app.keep_open(target);
    gui.pend(effects);
    gui.discover.keep.frozen = None;
    gui.discover.keep.dest = None;
    gui.discover.keep.last_own = None;
    gui.discover.keep.drawn = None;
}

/// Close the window; a copy still moving says so on the status line, since
/// the window that spoke for it is gone (clause 118).
pub(crate) fn close(gui: &mut Gui) {
    if let Some(view) = gui.app.keep_view()
        && let KeepStage::Job { job, .. } = &view.stage
        && job.live()
    {
        let note = busy_note(job, note_room(gui));
        set_note(gui, note);
    }
    gui.app.keep_close();
    gui.discover.keep.frozen = None;
    gui.discover.keep.dest = None;
    gui.discover.keep.last_own = None;
    gui.discover.keep.drawn = None;
}

/// The window's acts — a click, or the key that does the same.
pub(crate) fn act(gui: &mut Gui, act: KeepAct) {
    let Some((view, shown)) = shown_of(gui) else { return };
    match act {
        KeepAct::Close => close(gui),
        KeepAct::Dest(act) => dest_act(gui, act),
        KeepAct::Pick(choice) => gui.app.keep_pick(choice),
        KeepAct::Missing => {
            let on = view.missing.is_some_and(|(on, _)| on);
            gui.app.keep_set_missing(!on);
        }
        KeepAct::Primary => press(gui, &view, shown),
        KeepAct::Key(word) => {
            // A key word works only where it is drawn.
            if !acting_keys(gui, &view, shown).contains(&word) {
                return;
            }
            match word {
                KeyWord::Change => open_dest(gui),
                KeyWord::Queue => {
                    if let KeepStage::Job { job, .. } = &view.stage {
                        let effects = gui.app.keep_play_copy(job, false);
                        gui.pend(effects);
                        close(gui);
                    }
                }
                KeyWord::Choose => gui.app.keep_choose_again(),
                KeyWord::Cancel => {
                    let effects = gui.app.keep_cancel();
                    gui.pend(effects);
                }
                KeyWord::CancelOther => {
                    let effects = gui.app.keep_cancel_other();
                    gui.pend(effects);
                }
            }
        }
    }
}

/// The primary, pressed (Enter, or a click): Add starts the chosen copy;
/// Retry starts it again — its outcome held, the primary off, until the
/// server answers; Add again brings the choice back; Play your copy plays
/// it and closes; Close closes. Off, it does nothing.
fn press(gui: &mut Gui, view: &KeepView, shown: Shown) {
    let low = lower(view, shown, usize::from(KEEP_W - 4), Fit::default());
    let Some(press) = primary_of(&low).and_then(|p| p.press) else { return };
    match press {
        Press::Close => close(gui),
        Press::Add => {
            let effects = gui.app.keep_start();
            gui.pend(effects);
        }
        Press::Again(again) => {
            let unpaired = matches!(&view.stage, KeepStage::Job { job, .. } if job.unpaired());
            let effects = gui.app.keep_again();
            gui.pend(effects);
            match again {
                // Played: the window has said what it had to.
                JobAction::Play => close(gui),
                // A server no longer paired: the room is asked again and
                // its rows carry the new id (clause 128) — this window's
                // subject is gone with them.
                _ if unpaired => close(gui),
                // Its start is out: the outcome holds until it answers.
                JobAction::Retry | JobAction::CopyAgain
                    if gui.app.keep_view().is_some_and(|v| matches!(v.stage, KeepStage::Starting)) =>
                {
                    gui.discover.keep.frozen = Some(view.clone());
                }
                _ => {}
            }
        }
    }
}

/// ↑ ↓ (§6.1): the choice itself moves — This song › Album › Every album
/// by › the box, its fourth stop — clamped at both ends. The box keeps its
/// own tick (ticked from the start), so Every album by is reached on the
/// box while it is ticked; ↑ from the box unticks it (the dot stays), ↓
/// from Every album by ticks it again.
fn walk(gui: &mut Gui, view: &KeepView, delta: i64) {
    #[derive(Clone, Copy, PartialEq)]
    enum Stop {
        Choice(KeepChoice),
        Box,
    }
    let mut stops = Vec::new();
    for (choice, _) in &view.choices {
        stops.push(Stop::Choice(*choice));
        if *choice == KeepChoice::Artist && view.missing.is_some() {
            stops.push(Stop::Box);
        }
    }
    let on_box = view.choice == KeepChoice::Artist && view.missing.is_some_and(|(on, _)| on);
    let here = if on_box { Stop::Box } else { Stop::Choice(view.choice) };
    let Some(at) = stops.iter().position(|s| *s == here) else { return };
    let to = (at as i64 + delta).clamp(0, stops.len() as i64 - 1) as usize;
    match (here, stops[to]) {
        (_, Stop::Box) => gui.app.keep_set_missing(true),
        (Stop::Box, Stop::Choice(KeepChoice::Artist)) => gui.app.keep_set_missing(false),
        (_, Stop::Choice(choice)) => gui.app.keep_pick(choice),
    }
}

/// The window's keys (§6.2): ↑ ↓ move the choice, Enter presses the
/// primary, `d` `x` `a` `c` only where their words are drawn, Esc closes —
/// the copy keeps running. Everything else — Space, Tab, ← →, other
/// letters, `C` — does nothing.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) {
    // Where copies land, while it is open over the window, has every key.
    if gui.discover.keep.dest.is_some() {
        dest_key(gui, key);
        return;
    }
    let Some((view, shown)) = shown_of(gui) else { return };
    let choosing = matches!(view.stage, KeepStage::Choose { .. } | KeepStage::Refused { .. }) && view.choices.len() >= 2;
    match key.code {
        KeyCode::Esc => close(gui),
        KeyCode::Down if choosing => walk(gui, &view, 1),
        KeyCode::Up if choosing => walk(gui, &view, -1),
        KeyCode::Enter => press(gui, &view, shown),
        KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
            if let Some(word) = acting_keys(gui, &view, shown).into_iter().find(|w| w.key() == c) {
                act(gui, KeepAct::Key(word));
            }
        }
        _ => {}
    }
}

/// The tips line's cells (the footer's 99 at 100 columns): a line that
/// outgrows them gives up its `drops`, in order (clause 119).
const TIPS_CELLS: usize = 99;

fn fit_tips(line: String, drops: &[String]) -> String {
    let mut line = line;
    for drop in drops {
        if width(&line) <= TIPS_CELLS {
            break;
        }
        line = line.replace(&format!(" · {drop}"), "");
    }
    line
}

/// The tips line while the window is up (§6.4, clause 119): the keys that
/// work in its state, and nothing else.
pub(crate) fn tips(gui: &Gui) -> String {
    if let Some(ui) = &gui.discover.keep.dest {
        return dest_tips(gui, ui);
    }
    let Some((view, shown)) = shown_of(gui) else { return String::new() };
    let d = t!("gui.tips.discover_keep_d").to_string();
    let one = view.choices.len() < 2;
    match &view.stage {
        KeepStage::Choose { other: Some(other), .. } => {
            let mut line = t!("gui.tips.discover_keep_other").to_string();
            if other.cancelling {
                line = line.replace(&format!(" · {}", KeyWord::CancelOther.words()), "");
            }
            if one {
                line = line.replace(&format!("{} · ", t!("gui.tips.discover_keep_arrows")), "");
            }
            fit_tips(line, &[d])
        }
        KeepStage::Choose { .. } | KeepStage::Refused { .. } => {
            let key = if one { "gui.tips.discover_keep_one" } else { "gui.tips.discover_keep_choose" };
            fit_tips(t!(key).to_string(), &[d])
        }
        KeepStage::Starting => t!("gui.tips.discover_keep_close").to_string(),
        KeepStage::Off(_) => t!("gui.tips.discover_keep_yours").to_string(),
        KeepStage::Job { .. } if shown.retrying => t!("gui.tips.discover_keep_close").to_string(),
        KeepStage::Job { outcome, .. } if outcome.live() => {
            let key = if stopping(outcome, shown) { "gui.tips.discover_keep_yours" } else { "gui.tips.discover_keep_live" };
            t!(key).to_string()
        }
        KeepStage::Job { action, .. } => {
            let verb = match action {
                Some(JobAction::Play) => t!("gui.discover.keep.tip_play"),
                Some(JobAction::Retry | JobAction::CopyAgain) => t!("gui.discover.keep.tip_retry"),
                Some(JobAction::StartAgain) => t!("gui.discover.keep.tip_add_again"),
                Some(JobAction::Cancel) | None => t!("gui.discover.keep.tip_close"),
            };
            ended_tips(&verb, &acting_keys(gui, &view, shown))
        }
    }
}

/// An ended copy's tips line (§6.4): Enter's verb, the key words as drawn,
/// Esc — where it outgrows the footer, "d where files land" gives way
/// first, then "c choose again".
fn ended_tips(verb: &str, keys: &[KeyWord]) -> String {
    let mut parts = vec![t!("gui.tips.discover_keep_enter", verb = verb).to_string()];
    parts.extend(keys.iter().map(|k| k.tip()));
    parts.push(t!("gui.tips.discover_keep_esc").to_string());
    fit_tips(parts.join(" · "), &[KeyWord::Change.tip(), KeyWord::Choose.tip()])
}

// ── Where copies land (clauses 142–148) ─────────────────────────────────────

/// The picker's width (at most the area less 4, the kit's) and the height
/// it anchors as: 22 rows fit the 100 × 24 floor with the tab bar above
/// and the tips line below — its tallest states give rows up to hold it
/// ([`give_way`]).
const DEST_W: u16 = 80;
const DEST_ANCHOR: u16 = 22;
/// Its columns from the left border — no labels column, the names saying
/// what they are: the text (the preview and its notes, the libraries, the
/// radios' dots, the folders button, a refusal under a preset, Use the
/// library default), the open folder list's two-cell hang, the radios'
/// labels and Custom's field (its GOLD lines and its variables), the last
/// content cell.
const DEST_TEXT: u16 = 2;
const DEST_TREE: u16 = 4;
const DEST_FIELD: u16 = 6;
const DEST_LAST: u16 = 77;
/// The rows the folder list shows at once; more scroll.
const DEST_LIST: usize = 3;
/// A refusal's, or the validator's, GOLD rows at most.
const DEST_ERROR_ROWS: usize = 3;

/// A layout radio (clause 143), in the webapp's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Radio {
    /// "Artist / Album": the server's default layout — the album's credit
    /// ahead of the album, so a compilation stays in one folder.
    ArtistAlbum,
    /// "Server / Artist / Album" (`{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}`).
    Server,
    /// "Same folders as the server" (`{{SERVERFOLDER}}`).
    ServerFolder,
    /// "Library layout": the chosen library's own template, where it is
    /// none of the three.
    Template,
    Custom,
}

/// The presets this view offers for `vpath`'s library (the webapp's
/// `dmPickerPresets`), each with its words' key and the layout it writes:
/// Artist / Album (the view's default layout, else the server's), Server
/// / Artist / Album unless that is the default, Same folders as the
/// server only where the view lists its variable — a server from before
/// 2026-10-22 refuses it at Save, so it is never offered there — and
/// unless that is the default, then the library's own template where it
/// is none of them. Any other layout is Custom's.
fn presets(view: &CollectionDestinationView, vpath: &str) -> Vec<(Radio, &'static str, String)> {
    let first = view.default_layout.clone().filter(|l| !l.trim().is_empty()).unwrap_or_else(|| DEFAULT_LAYOUT.to_string());
    let mut out = vec![(Radio::ArtistAlbum, "gui.discover.dest.preset_artist_album", first.clone())];
    if first != PEER_LAYOUT {
        out.push((Radio::Server, "gui.discover.dest.preset_server", PEER_LAYOUT.to_string()));
    }
    if first != SERVERFOLDER_LAYOUT && view.knows(SERVER_FOLDER) {
        out.push((Radio::ServerFolder, "gui.discover.dest.preset_serverfolder", SERVERFOLDER_LAYOUT.to_string()));
    }
    let template = view.libraries.iter().find(|l| l.vpath == vpath).and_then(|l| l.template.clone()).filter(|t| !t.trim().is_empty());
    if let Some(template) = template
        && ![first.as_str(), PEER_LAYOUT, SERVERFOLDER_LAYOUT].contains(&template.as_str())
    {
        out.push((Radio::Template, "gui.discover.dest.preset_library", template));
    }
    out
}

fn is_preset(view: &CollectionDestinationView, vpath: &str, layout: &str) -> bool {
    presets(view, vpath).iter().any(|(_, _, p)| p == layout)
}

/// One keyboard stop of the picker, in walking order — the rows' order,
/// the webapp's: the radios, the folders button, then Custom's own rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestStop {
    /// The libraries' row — a stop with a choice of libraries.
    Library,
    Radio(Radio),
    /// "Start from an existing folder…" — and its list, while it is open.
    Folders,
    /// Custom's field, and the variables under it: stops while Custom is
    /// chosen.
    Field,
    Vars,
    UseDefault,
    Save,
}

/// What the picker's pointer and keys do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestAct {
    /// Esc, "Esc close" on the bottom border: the open folder list
    /// closes, else back to Keep a copy, nothing saved.
    Back,
    /// ← → on the libraries' row: the library before, the one after.
    Step(i8),
    /// A library clicked, by its place in the row.
    Library(usize),
    /// A stop clicked: the hand goes there, a field takes the keys.
    Hand(DestStop),
    /// A radio clicked.
    Pick(Radio),
    /// A variable of the row, by its place: inserted at the caret.
    Var(usize),
    /// "Start from an existing folder…" / "Hide folders": the list opens,
    /// or closes.
    Folders,
    /// A row of the open list, by its place: up to the parent, or into a
    /// folder — the layout starts there.
    Folder(usize),
    UseDefault,
    Save,
    /// The folder list's scrollbar.
    Scroll(i32),
    Jump(usize),
}

/// Where copies land's own state — the editor's fields as the webapp's
/// `picker` holds them, the keyboard's hand, the folder list, a refusal
/// (clauses 142–144). What it draws from — the destination's view — and
/// its asks are the App's.
#[derive(Clone)]
pub(crate) struct DestUi {
    /// The library chosen, by its name: the view may be read again under it.
    vpath: String,
    /// The preset chosen — or, opened on none, the layout as it opened
    /// (the webapp's `layout`).
    layout: String,
    /// Custom is chosen: its field is the layout.
    custom: bool,
    /// Custom's field (the webapp's `text`): empty the first time, and what
    /// was typed stays in it while a preset is chosen.
    text: Input,
    /// Custom's text before a library's own layout moved into it (the
    /// webapp's `before`).
    kept: Option<String>,
    /// The place the picker opened on, base folder and layout apart as the
    /// server holds them (the webapp's `from`).
    from: CollectionDestination,
    /// The keyboard's stop; `None` while its save is out.
    hand: Option<DestStop>,
    /// The variables row's held variable, by its place.
    var: usize,
    /// The save this picker sent, by its seq, while it is out: "saving…",
    /// and its answer is this picker's to fold in (another picker's — one
    /// left with Esc while saving — is the status line's).
    sent: Option<u64>,
    /// Where the hand was when the save went out: back there on a refusal.
    before: Option<DestStop>,
    /// The folder list under "Start from an existing folder…", while open.
    list: Option<FolderList>,
    /// A refused save, its words GOLD until the next edit.
    error: Option<crate::api::PluginFailure>,
    /// Use the library default pressed while the picker is open: Save sends
    /// `null` — back to the default, nothing kept for the account — where
    /// the fields are the default then, whatever was picked in between
    /// (the webapp's `reset`).
    defaulted: bool,
}

/// The folder list (the webapp's `browse`): the folder listed, inside the
/// library (none: its root), the held row (the keyboard's) and the
/// viewport, and a folder pressed whose name has a brace, which the list
/// says instead of starting there.
#[derive(Clone, Default)]
struct FolderList {
    dir: Vec<String>,
    held: Option<usize>,
    view: ListView,
    braced: Option<String>,
}

impl DestUi {
    /// Opened on the place in force (the webapp's `dmPickerFill`): a base
    /// folder set elsewhere comes in front of the layout ([`fold_base`])
    /// and opens in Custom's field, as does a layout that is none of the
    /// presets — nothing is dirty until edited. The hand on the first stop.
    fn of(place: &CollectionDestination, view: &CollectionDestinationView) -> DestUi {
        let layout = fold_base(&place.base, &place.layout);
        let custom = layout != place.layout || !is_preset(view, &place.vpath, &layout);
        let mut ui = DestUi {
            vpath: place.vpath.clone(),
            layout: layout.clone(),
            custom,
            text: Input::new(if custom { layout } else { String::new() }),
            kept: None,
            from: CollectionDestination { source: None, ..place.clone() },
            hand: None,
            var: 0,
            sent: None,
            before: None,
            list: None,
            error: None,
            defaulted: false,
        };
        ui.hand = ui.stops(view).first().copied();
        ui
    }

    /// The layout the picker holds: Custom's text while Custom is chosen,
    /// else the preset. It is saved with no base folder.
    fn layout(&self) -> String {
        if self.custom { self.text.value().to_string() } else { self.layout.clone() }
    }

    /// Custom's field still holds the layout as the picker opened it.
    fn as_opened(&self) -> bool {
        self.custom && self.text.value() == fold_base(&self.from.base, &self.from.layout)
    }

    /// A folder of a base set elsewhere, held as it opened, whose name a
    /// layout reads as a variable: saved as it is, the files would land
    /// elsewhere.
    fn held_braces(&self) -> Option<String> {
        if self.as_opened() { braced_folder(&self.from.base) } else { None }
    }

    fn radios(&self, view: &CollectionDestinationView) -> Vec<Radio> {
        let mut out: Vec<Radio> = presets(view, &self.vpath).into_iter().map(|(radio, _, _)| radio).collect();
        out.push(Radio::Custom);
        out
    }

    /// The chosen radio: Custom once picked, else the preset the layout is
    /// — none where the held preset left the list (a re-read changed the
    /// library's template under an edit), as the webapp marks none.
    fn chosen(&self, view: &CollectionDestinationView) -> Option<Radio> {
        if self.custom {
            return Some(Radio::Custom);
        }
        presets(view, &self.vpath).into_iter().find(|(_, _, p)| *p == self.layout).map(|(radio, _, _)| radio)
    }

    /// The picker opened on `place` still shows it: nothing edited since
    /// (the webapp's `from` against the view's destination).
    fn opened_on(&self, place: &CollectionDestination) -> bool {
        self.from.vpath == place.vpath && self.from.base == place.base && self.from.layout == place.layout
    }

    /// The place as Save sends it: the library and the layout, no base
    /// folder.
    fn place(&self) -> CollectionDestination {
        CollectionDestination { vpath: self.vpath.clone(), base: String::new(), layout: self.layout(), source: None }
    }

    /// The fields say the library default (the webapp's `dmPickerIsDefault`).
    fn is_default(&self, view: &CollectionDestinationView) -> bool {
        view.library_default().is_some_and(|d| d.same_place(&self.place()))
    }

    /// The account's own place is in force: Use the library default
    /// stands.
    fn mine(view: &CollectionDestinationView) -> bool {
        view.destination.as_ref().is_some_and(|d| d.source.as_deref() == Some("user"))
    }

    /// Something to save (the webapp's `dmPickerDirty`): another library or
    /// layout than the one it opened on (a base folder set elsewhere,
    /// folded into the layout as it opened, is no edit; a preset of the
    /// same text over a folder a layout reads as a variable is), or Use the
    /// library default pressed over a place saved for the account.
    fn dirty(&self, view: &CollectionDestinationView) -> bool {
        self.vpath != self.from.vpath
            || self.layout() != fold_base(&self.from.base, &self.from.layout)
            || (!self.custom && braced_folder(&self.from.base).is_some())
            || (self.defaulted && DestUi::mine(view) && self.is_default(view))
    }

    /// Save is on: something to save, a layout the server takes, no folder
    /// a layout cannot hold (the webapp's `dmPickerCanSave`).
    fn can_save(&self, view: &CollectionDestinationView) -> bool {
        self.dirty(view)
            && validate_layout(&self.layout(), &|name| view.knows(name)).is_ok()
            && !(self.custom && self.held_braces().is_some())
    }

    /// Why Save is off, for its tooltip: nothing changed, else the layout.
    fn save_off(&self, view: &CollectionDestinationView) -> String {
        if self.dirty(view) { t!("gui.discover.dest.save_off_bad").to_string() } else { t!("gui.discover.dest.save_off_same").to_string() }
    }

    /// Custom's field's problem (the webapp's `dmPickerCustomProblem`): the
    /// validator's words as typed — none while it is empty: its hint
    /// stands — or the folder it holds as it opened that a layout reads as
    /// a variable.
    fn problem(&self, view: &CollectionDestinationView) -> Option<String> {
        if !self.custom || self.text.value().is_empty() {
            return None;
        }
        if let Err(problem) = validate_layout(self.text.value(), &|name| view.knows(name)) {
            return Some(problem_words(&problem));
        }
        self.held_braces().map(|folder| t!("gui.discover.dest.err_base_braces", folder = folder).to_string())
    }

    /// The stops in walking order, the rows': the libraries (with a choice
    /// of them — the row's, so one the server no longer lists counts),
    /// each radio, the folders button, Custom's field and its variables
    /// while Custom is chosen, the buttons.
    fn stops(&self, view: &CollectionDestinationView) -> Vec<DestStop> {
        let mut out = Vec::new();
        if self.chips(view).len() > 1 {
            out.push(DestStop::Library);
        }
        out.extend(self.radios(view).into_iter().map(DestStop::Radio));
        out.push(DestStop::Folders);
        if self.custom {
            out.extend([DestStop::Field, DestStop::Vars]);
        }
        if DestUi::mine(view) || !self.is_default(view) {
            out.push(DestStop::UseDefault);
        }
        out.push(DestStop::Save);
        out
    }

    /// The buttons standing, left to right.
    fn buttons(&self, view: &CollectionDestinationView) -> Vec<DestStop> {
        self.stops(view).into_iter().filter(|s| matches!(s, DestStop::UseDefault | DestStop::Save)).collect()
    }

    /// The libraries the row offers (the webapp's `dmPickerChips`): every
    /// one the account may add files to, and the chosen one when the
    /// server no longer lists it.
    fn chips(&self, view: &CollectionDestinationView) -> Vec<String> {
        let mut names: Vec<String> = view.libraries.iter().map(|l| l.vpath.clone()).collect();
        if !names.contains(&self.vpath) {
            names.push(self.vpath.clone());
        }
        names
    }

    /// The folder the layout starts in whatever the tags (the webapp's
    /// `dmPickerBase`): Custom's leading folders that name no variable,
    /// normalised — none for a preset, or for a lead the server refuses.
    fn base(&self) -> Vec<String> {
        if !self.custom {
            return Vec::new();
        }
        let (folders, _) = layout_lead(self.text.value());
        let lead = folders.join("/");
        if lead.is_empty() || validate_resolved_path(&lead).is_err() { Vec::new() } else { base_folders(&lead) }
    }

    /// A library chosen (the webapp's `dmPickerLibrary`): a library's own
    /// layout that leaves with it stays the layout, in Custom's field;
    /// back in that library with it untouched, its preset again and
    /// Custom's field as it was (never over a folder a layout reads as a
    /// variable).
    fn choose_library(&mut self, view: &CollectionDestinationView, vpath: &str) {
        let layout = self.layout.clone();
        self.vpath = vpath.to_string();
        if !self.custom && !is_preset(view, vpath, &layout) {
            self.kept = Some(self.text.value().to_string());
            self.custom = true;
            self.text = Input::new(layout);
        } else if self.custom && self.text.value() == layout && self.held_braces().is_none() && is_preset(view, vpath, &layout) {
            self.custom = false;
            if let Some(kept) = self.kept.take() {
                self.text = Input::new(kept);
            }
        }
        self.error = None;
        self.anchor_list();
    }

    /// A folder chosen in the list starts Custom's layout (the webapp's
    /// `dmPickerStartAt`): in place of the folders it starts with — or,
    /// from a preset, in front of it. Custom is chosen, the caret at the
    /// end. The library root from a preset changes nothing.
    fn start_at(&mut self, folder: &str) {
        if !self.custom && folder.is_empty() {
            return;
        }
        let text = if self.custom { start_layout_at(folder, self.text.value()) } else { fold_base(folder, &self.layout) };
        let at = text.chars().count();
        self.text = Input::new(text).with_cursor(at);
        self.custom = true;
        self.error = None;
    }

    /// The open list lists the chosen folder's parent (the library root for
    /// none), so that folder shows pressed among its neighbours.
    fn anchor_list(&mut self) {
        let mut dir = self.base();
        dir.pop();
        if let Some(list) = self.list.as_mut() {
            *list = FolderList { dir, ..FolderList::default() };
        }
    }

    /// An edit: the refusal's words go (a default pressed stands until
    /// Save, which decides by the fields).
    fn edited(&mut self) {
        self.error = None;
    }
}

/// A folder as the file explorer takes it: `/{library}/{folder}/…`.
fn folder_path(vpath: &str, folders: &[String]) -> String {
    let mut out = format!("/{vpath}");
    for folder in folders {
        out.push('/');
        out.push_str(folder);
    }
    out
}

/// A folder in words: `copies › from-peers`.
fn folder_words(vpath: &str, folders: &[String]) -> String {
    std::iter::once(vpath).chain(folders.iter().map(String::as_str)).collect::<Vec<_>>().join(crumb_sep())
}

/// The variables a layout may name, as the row offers them.
fn variables(view: &CollectionDestinationView) -> Vec<String> {
    if view.variables.is_empty() {
        crate::api::types::LAYOUT_VARS.iter().map(|v| v.to_string()).collect()
    } else {
        view.variables.clone()
    }
}

fn var_token(name: &str) -> String {
    format!("{{{{{name}}}}}")
}

/// The validator's refusal in words (the webapp's `dmPickerProblem`, the
/// phone's words): an unknown variable named as it was typed, in its
/// braces — the variables row right under the words lists the known ones.
fn problem_words(problem: &LayoutProblem) -> String {
    let example = var_token("ARTIST");
    match problem {
        LayoutProblem::UnknownVariable { token } => t!("gui.discover.dest.err_unknown_var", name = token).to_string(),
        LayoutProblem::UnbalancedBraces => t!("gui.discover.dest.err_braces", example = example).to_string(),
        LayoutProblem::Absolute => t!("gui.discover.dest.err_absolute").to_string(),
        LayoutProblem::Traversal => t!("gui.discover.dest.err_traversal").to_string(),
        LayoutProblem::EmptyPath => t!("gui.discover.dest.err_empty").to_string(),
        LayoutProblem::Home => t!("gui.discover.dest.err_home").to_string(),
        LayoutProblem::DriveLetter => t!("gui.discover.dest.err_drive").to_string(),
        LayoutProblem::TemplateTooLong => t!("gui.discover.dest.err_too_long", max = MAX_TEMPLATE_LEN).to_string(),
        LayoutProblem::InvalidChars => t!("gui.discover.dest.err_control").to_string(),
        LayoutProblem::EmptyTemplate | LayoutProblem::PathTooLong => t!("gui.discover.dest.err_generic").to_string(),
    }
}

/// What the preview row shows (the webapp's `dmPickerPreview` and
/// `dmPickerUnseen`): the path the layout held renders for the copy Keep a
/// copy has chosen — the layout as it opened drawn as the server holds it,
/// base folder and all, so a folder set elsewhere reads as Keep a copy's
/// place line reads it, whatever a layout would make of its name — or no
/// path.
enum Preview {
    /// Custom's field is empty: "Files go to —", its hint says the rest.
    Empty,
    /// The layout would be refused: "Files go to —", the GOLD words under
    /// the field say why.
    Invalid,
    /// This item's path is longer than the server takes — no fault of the
    /// layout's: Save takes it. Measured as the server measures it
    /// (`renderTarget`, the webapp's `previewTarget`): the layout alone,
    /// rendered whole with this copy's own values, never a base folder.
    TooLong,
    Path(Landing),
}

fn preview(ui: &DestUi, view: &KeepView, dview: &CollectionDestinationView) -> Preview {
    if ui.custom && ui.text.value().is_empty() {
        return Preview::Empty;
    }
    let known = |name: &str| dview.knows(name);
    let (base, layout) = if ui.as_opened() { (ui.from.base.clone(), ui.from.layout.clone()) } else { (String::new(), ui.layout()) };
    let folders = base_folders(&base).join("/");
    if (!folders.is_empty() && validate_resolved_path(&folders).is_err()) || validate_layout(&layout, &known).is_err() {
        return Preview::Invalid;
    }
    if view.rendered_len_for(&layout, &known) > MAX_RESOLVED_LEN {
        return Preview::TooLong;
    }
    Preview::Path(view.landing_for(&ui.vpath, &base, &layout, &known))
}

/// The preview's note for a variable with no value for this item, left
/// out of the path and named in its braces (the webapp's
/// `dmPickerMissing`): an item at the top of its server's library sits in
/// no folder there. `None` with none.
fn missing_words(landing: &Landing) -> Option<String> {
    if landing.missing.is_empty() {
        return None;
    }
    let vars = landing.missing.iter().map(|v| var_token(v)).collect::<Vec<_>>().join(", ");
    let key = if landing.missing == [SERVER_FOLDER] { "gui.discover.dest.missing_server" } else { "gui.discover.dest.missing" };
    Some(t!(key, vars = vars).to_string())
}

/// A refusal's, or the validator's, GOLD rows: the words wrapped at `room`
/// cells, three rows at most — the third clipped with `…`.
fn error_lines(words: &str, room: usize) -> Vec<String> {
    let mut lines = crate::kit::wrap_words(words, room);
    if lines.len() > DEST_ERROR_ROWS {
        let rest = lines.split_off(DEST_ERROR_ROWS - 1).join(" ");
        lines.push(super::bar::clip(&rest, room).into_owned());
    }
    lines
}

/// One row of the open folder list.
#[derive(Debug, Clone, PartialEq)]
enum TreeRow {
    /// "Up to {parent}", first inside a folder.
    Up,
    Folder(String),
}

/// The open list's rows: Up inside a folder, then the folder's folders as
/// the App listed them (none until the listing answers).
fn tree_rows(app: &crate::tui::app::App, ui: &DestUi) -> Vec<TreeRow> {
    let Some(list) = ui.list.as_ref() else { return Vec::new() };
    let mut rows = Vec::new();
    if !list.dir.is_empty() {
        rows.push(TreeRow::Up);
    }
    if let Some(Folders::Listed(names)) = app.keep_dest_folders(&folder_path(&ui.vpath, &list.dir)) {
        rows.extend(names.iter().cloned().map(TreeRow::Folder));
    }
    rows
}

/// The one note under the open list, and whether it is GOLD: a folder
/// pressed whose name has a brace, "Looking up…" while it is asked, a
/// folder not there yet, by its name (the first file creates it; the
/// webapp's `dmBrowseMissing`) — the one the layout starts in, absent from
/// its parent's listing, or the folder listed, the server's 404 for it
/// (`Cannot read directory … (ENOENT)`: a typed lead two folders deep,
/// none of them there) — any other failure in the server's words (the
/// head above names the folder; a library's own 404 among them: no file
/// creates a library), or no folders in here. `None` with nothing to say.
fn tree_note(app: &crate::tui::app::App, ui: &DestUi) -> Option<(String, bool)> {
    let list = ui.list.as_ref()?;
    if let Some(folder) = &list.braced {
        return Some((t!("gui.discover.dest.folder_braces", folder = folder).to_string(), true));
    }
    let not_yet = |folder: &str| Some((t!("gui.discover.dest.will_create", folder = folder).to_string(), false));
    match app.keep_dest_folders(&folder_path(&ui.vpath, &list.dir)) {
        None | Some(Folders::Asking) => Some((t!("gui.discover.dest.looking_up").to_string(), false)),
        Some(Folders::Failed(failure)) => match (failure, list.dir.last()) {
            (crate::api::PluginFailure::Refused { status: 404, .. }, Some(folder)) => not_yet(folder),
            _ => Some((t!("gui.discover.dest.list_failed", words = failure_words(failure)).to_string(), true)),
        },
        Some(Folders::Listed(names)) => {
            let base = ui.base();
            if let Some((last, parent)) = base.split_last()
                && parent == list.dir.as_slice()
                && !names.contains(last)
            {
                return not_yet(last);
            }
            names.is_empty().then(|| (t!("gui.discover.dest.no_folders").to_string(), false))
        }
    }
}

/// One row of the picker under its top border, top to bottom.
#[derive(Debug, Clone, PartialEq)]
enum DestRow {
    /// "Files go to" and the path, "—" where none shows, or the GOLD
    /// words of a path the server would not take.
    Preview,
    /// A slot's note's rows (two at most), and the variables this item has
    /// no value for.
    Note(usize),
    Missing,
    /// B1 under the preview, B2 under the libraries, B3 above the buttons.
    Blank(Gap),
    /// The libraries, with a choice of them.
    Library,
    Radio(Radio),
    /// Custom's field.
    Field,
    /// A GOLD row: the validator's words, or a refusal's.
    Problem(String),
    Vars,
    /// "Start from an existing folder…" / "Hide folders".
    Folders,
    /// The open list's head, its rows by their place in the list's
    /// window, and its one note.
    TreeHead,
    Listed(usize),
    TreeNote(String, bool),
    Buttons,
}

/// The picker's rows (clause 143), before [`give_way`] fits them to the
/// floor, in the webapp's order: the preview and its notes, a blank, the
/// libraries and a blank where there is a choice of them, the radios — a
/// refusal's words under them with a preset — the folders button and its
/// open list, then Custom's own rows while it is chosen: its field, its
/// problem (or the refusal's words) and its variables; a blank, the
/// buttons.
fn dest_rows(ui: &DestUi, dview: &CollectionDestinationView, shown: &Preview, tree: usize, note: Option<&(String, bool)>) -> Vec<DestRow> {
    let wide = usize::from(DEST_LAST + 1 - DEST_TEXT);
    let mut rows = vec![DestRow::Preview];
    if let Preview::Path(landing) = shown {
        if let Some(words) = slot_note(landing) {
            let (lines, _) = wrap_rows(&words, wide, 0, &[], false);
            rows.extend((0..lines.len()).map(DestRow::Note));
        }
        if !landing.missing.is_empty() {
            rows.push(DestRow::Missing);
        }
    }
    rows.push(DestRow::Blank(Gap::B1));
    if ui.chips(dview).len() > 1 {
        rows.extend([DestRow::Library, DestRow::Blank(Gap::B2)]);
    }
    rows.extend(ui.radios(dview).into_iter().map(DestRow::Radio));
    let field_room = usize::from(DEST_LAST + 1 - DEST_FIELD);
    // The server's words as sent, as the webapp draws them.
    let refusal = ui.error.as_ref().map(failure_words);
    if !ui.custom && let Some(words) = &refusal {
        rows.extend(error_lines(words, wide).into_iter().map(DestRow::Problem));
    }
    rows.push(DestRow::Folders);
    if ui.list.is_some() {
        rows.push(DestRow::TreeHead);
        rows.extend((0..tree.min(DEST_LIST)).map(DestRow::Listed));
        if let Some((words, gold)) = note {
            rows.push(DestRow::TreeNote(words.clone(), *gold));
        }
    }
    if ui.custom {
        rows.push(DestRow::Field);
        if let Some(words) = refusal.or_else(|| ui.problem(dview)) {
            rows.extend(error_lines(&words, field_room).into_iter().map(DestRow::Problem));
        }
        rows.push(DestRow::Vars);
    }
    rows.extend([DestRow::Blank(Gap::B3), DestRow::Buttons]);
    rows
}

/// Where the rows would pass the 22 the 100 × 24 floor holds, they give
/// way in this order until they fit: the blank under the libraries (B2),
/// the one under the preview (B1), the one above the buttons (B3); the
/// missing-variables note; the slot note's second row (the first then
/// clipped, the whole note its tooltip), then the slot note (into the
/// path's tooltip, with the missing note); the variables row; the list's
/// rows from the last down to one; a GOLD block's rows down to one.
fn give_way(rows: &mut Vec<DestRow>) {
    let over = |rows: &[DestRow]| rows.len() + 2 > usize::from(DEST_ANCHOR);
    let steps = [
        DestRow::Blank(Gap::B2),
        DestRow::Blank(Gap::B1),
        DestRow::Blank(Gap::B3),
        DestRow::Missing,
        DestRow::Note(1),
        DestRow::Note(0),
        DestRow::Vars,
    ];
    for what in steps {
        if !over(rows) {
            return;
        }
        if let Some(at) = rows.iter().position(|r| *r == what) {
            rows.remove(at);
        }
    }
    let last = |rows: &[DestRow], pick: fn(&DestRow) -> bool| {
        let n = rows.iter().filter(|r| pick(r)).count();
        (n > 1).then(|| rows.iter().rposition(pick)).flatten()
    };
    while over(rows) {
        let Some(at) = last(rows, |r| matches!(r, DestRow::Listed(_))) else { break };
        rows.remove(at);
    }
    while over(rows) {
        let Some(at) = last(rows, |r| matches!(r, DestRow::Problem(_))) else { break };
        rows.remove(at);
    }
}

/// Runs of words in their tokens at `x`, in `room` cells: whole where they
/// fit, else their tail behind a leading `…` touching the words it cut
/// into (the picker's preview, so the folders that differ stay in view).
/// True when it was clipped.
fn put_tail(frame: &mut Frame, x: u16, y: u16, runs: &[(String, Style)], room: usize) -> bool {
    use unicode_segmentation::UnicodeSegmentation;
    let total: usize = runs.iter().map(|(text, _)| width(text)).sum();
    let mut skip = 0;
    let mut at = x;
    if total > room {
        let mark = if legacy_conhost() { "»" } else { "…" };
        put(frame, x, y, mark, dim());
        at += 1;
        skip = total - room.saturating_sub(1);
    }
    for (text, style) in runs {
        let mut shown = String::new();
        let mut cut = false;
        for grapheme in text.graphemes(true) {
            let w = crate::kit::grapheme_cells(grapheme);
            // A wide one the cut lands inside goes whole: a cell short,
            // never over.
            if skip > 0 {
                skip = skip.saturating_sub(w);
                cut = true;
                continue;
            }
            shown.push_str(grapheme);
        }
        // A blank the cut lands on inside a name goes too, so the `…`
        // touches the words — never a separator's blank before a whole
        // crumb.
        if cut && !shown.trim().is_empty() {
            shown = shown.trim_start().to_string();
        }
        put(frame, at, y, &shown, *style);
        at += width(&shown) as u16;
    }
    total > room
}

/// Open Where copies land (clause 142) on the place in force: `d change`
/// or `d`, from the choice or a file in the way. Keep a copy gives way
/// until it closes.
pub(crate) fn open_dest(gui: &mut Gui) {
    let Some(view) = gui.app.keep_dest_view() else { return };
    let Some(place) = view.destination.as_ref() else { return };
    gui.discover.keep.dest = Some(DestUi::of(place, view));
}

/// Ask the open list's folder of the server, once (clause 143).
fn dest_ask(gui: &mut Gui) {
    let Some(ui) = gui.discover.keep.dest.as_ref() else { return };
    let Some(list) = ui.list.as_ref() else { return };
    let path = folder_path(&ui.vpath, &list.dir);
    if gui.app.keep_dest_folders(&path).is_some() {
        return;
    }
    let effects = gui.app.keep_dest_list(&path);
    gui.pend(effects);
}

/// The open picker's own save is out: "saving…", no buttons, only Esc.
fn dest_saving(gui: &Gui) -> bool {
    let out = gui.app.keep_dest_saving();
    gui.discover.keep.dest.as_ref().is_some_and(|ui| ui.sent.is_some() && ui.sent == out)
}

/// Back to Keep a copy — Esc, "Esc close", or a save that landed — as it was,
/// the dot where it was; a save's place is its evidence (§7).
fn close_dest(gui: &mut Gui) {
    gui.discover.keep.dest = None;
    gui.app.keep_dest_forget();
}

/// A save's answer, folded in once a frame (clause 144) by the picker that
/// sent it: saved — its view replaced the gate's; Keep a copy shows the new
/// place — or the server's words in GOLD, the fields as they were and the
/// hand back where it was (on Save, where it was nowhere). A refusal whose
/// picker is gone (Esc while saving) is said on the status line, never
/// lost; an answer never closes or fills a picker opened since.
fn dest_landed(gui: &mut Gui) {
    let Some((seq, answer)) = gui.app.keep_dest_take_saved() else { return };
    let ours = gui.discover.keep.dest.as_ref().is_some_and(|ui| ui.sent == Some(seq));
    match answer {
        Ok(()) if ours => close_dest(gui),
        Ok(()) => {}
        Err(failure) if ours => {
            if let Some(ui) = gui.discover.keep.dest.as_mut() {
                ui.sent = None;
                ui.error = Some(failure);
                ui.hand = ui.before.take().or(Some(DestStop::Save));
            }
        }
        Err(failure) => {
            let words = t!("gui.discover.dest.save_failed", words = failure_words(&failure)).to_string();
            let room = note_room(gui);
            set_note(gui, (fit_note(&words, None, room), MessageKind::Error));
        }
    }
}

/// The picker's Save, while it is on: the library and the layout with no
/// base folder — or `null` where Use the library default was pressed and
/// the fields are the default.
fn dest_save(gui: &mut Gui) {
    let Some(view) = gui.app.keep_dest_view().cloned() else { return };
    let Some(ui) = gui.discover.keep.dest.as_mut() else { return };
    if !ui.can_save(&view) {
        return;
    }
    let place = ui.place();
    let back = ui.defaulted && ui.is_default(&view);
    let (effects, seq) = gui.app.keep_dest_save(if back { None } else { Some(place) });
    let Some(seq) = seq else { return };
    gui.pend(effects);
    if let Some(ui) = gui.discover.keep.dest.as_mut() {
        ui.sent = Some(seq);
        ui.before = ui.hand.take();
        ui.list = None;
        ui.error = None;
    }
}

/// The picker's acts (clauses 142–144).
pub(crate) fn dest_act(gui: &mut Gui, act: DestAct) {
    let Some(view) = gui.app.keep_dest_view().cloned() else { return };
    // What Esc does — while a save is out too: the open list closes, else
    // back to Keep a copy.
    if act == DestAct::Back {
        if gui.discover.keep.dest.as_ref().is_some_and(|ui| ui.list.is_some()) {
            if let Some(ui) = gui.discover.keep.dest.as_mut() {
                ui.list = None;
            }
        } else {
            close_dest(gui);
        }
        return;
    }
    if dest_saving(gui) {
        return;
    }
    let tree = gui.discover.keep.dest.as_ref().map(|ui| tree_rows(&gui.app, ui)).unwrap_or_default();
    let Some(ui) = gui.discover.keep.dest.as_mut() else { return };
    match act {
        DestAct::Back => {}
        DestAct::Step(delta) => {
            let chips = ui.chips(&view);
            if chips.len() > 1 {
                let at = chips.iter().position(|name| *name == ui.vpath).unwrap_or(0);
                let next = (at as i64 + i64::from(delta)).rem_euclid(chips.len() as i64) as usize;
                ui.choose_library(&view, &chips[next]);
            }
            ui.hand = Some(DestStop::Library);
        }
        DestAct::Library(at) => {
            // The name already chosen only takes the hand: a refusal's
            // words, the open list and the layout stay.
            if let Some(name) = ui.chips(&view).get(at).cloned()
                && name != ui.vpath
            {
                ui.choose_library(&view, &name);
            }
            ui.hand = Some(DestStop::Library);
        }
        DestAct::Hand(stop) => {
            if stop != DestStop::Folders {
                ui.list = None;
            }
            ui.hand = Some(stop);
        }
        DestAct::Pick(radio) => {
            match radio {
                // Chosen with the pointer, Custom's field takes the keyboard,
                // as the phone's does (dest_key keeps the hand on the radio).
                Radio::Custom => {
                    ui.custom = true;
                    ui.hand = Some(DestStop::Field);
                }
                preset => {
                    if let Some((_, _, layout)) = presets(&view, &ui.vpath).into_iter().find(|(r, _, _)| *r == preset) {
                        ui.layout = layout;
                    }
                    ui.custom = false;
                    ui.hand = Some(DestStop::Radio(radio));
                }
            }
            ui.list = None;
            ui.edited();
        }
        DestAct::Var(at) => {
            let Some(name) = variables(&view).get(at).cloned() else { return };
            let token = var_token(&name);
            // No room left for it in the layout: nothing goes in.
            if ui.text.value().chars().count() + token.chars().count() > MAX_TEMPLATE_LEN {
                return;
            }
            let cursor = ui.text.cursor();
            let mut value: Vec<char> = ui.text.value().chars().collect();
            let at = cursor.min(value.len());
            value.splice(at..at, token.chars());
            ui.text = Input::new(value.into_iter().collect()).with_cursor(at + token.chars().count());
            ui.custom = true;
            // The pointer's insert hands the keyboard back to the field; the
            // keyboard's stays on the row for the next one.
            if ui.hand != Some(DestStop::Vars) {
                ui.hand = Some(DestStop::Field);
            }
            ui.edited();
        }
        DestAct::Folders => {
            if ui.list.is_some() {
                ui.list = None;
            } else {
                ui.list = Some(FolderList::default());
                ui.anchor_list();
            }
            ui.hand = Some(DestStop::Folders);
        }
        DestAct::Folder(at) => {
            let Some(list) = ui.list.as_ref() else { return };
            let mut dir = list.dir.clone();
            let mut braced = None;
            match tree.get(at) {
                Some(TreeRow::Up) => {
                    dir.pop();
                }
                Some(TreeRow::Folder(name)) => {
                    let folder = sanitize_segment(Some(name));
                    if braced_folder(&folder).is_some() {
                        braced = Some(folder);
                    } else {
                        dir.push(folder);
                    }
                }
                None => return,
            }
            if let Some(folder) = braced {
                if let Some(list) = ui.list.as_mut() {
                    list.braced = Some(folder);
                }
            } else {
                ui.start_at(&dir.join("/"));
                ui.list = Some(FolderList { dir, ..FolderList::default() });
            }
            ui.hand = Some(DestStop::Folders);
        }
        DestAct::UseDefault => {
            if let Some(default) = view.library_default() {
                ui.vpath = default.vpath;
                ui.layout = default.layout.clone();
                ui.custom = !is_preset(&view, &ui.vpath, &default.layout);
                if ui.custom {
                    ui.text = Input::new(default.layout);
                }
                ui.defaulted = true;
                ui.edited();
                ui.anchor_list();
                ui.hand = ui.stops(&view).first().copied();
            }
        }
        DestAct::Save => dest_save(gui),
        DestAct::Scroll(delta) => {
            if let Some(list) = ui.list.as_mut() {
                list.view.step(delta);
            }
        }
        DestAct::Jump(first) => {
            if let Some(list) = ui.list.as_mut() {
                list.view.scroll = first;
            }
        }
    }
    dest_ask(gui);
}

/// The wheel over the picker: its folder list, a row a notch.
pub(crate) fn dest_wheel(gui: &mut Gui, delta: i32) {
    if let Some(list) = gui.discover.keep.dest.as_mut().and_then(|ui| ui.list.as_mut()) {
        list.view.step(delta);
    }
}

/// The picker's keys (clause 143): ↑ ↓ and Tab walk the stops — and the
/// folder list while it is open; ← → step the library, the variables and
/// the buttons; Space or Enter pick a radio, insert a variable, open a
/// folder or press a button; in Custom's field every printable key edits
/// and Enter saves while Save is on; Esc closes the list, else goes back.
fn dest_key(gui: &mut Gui, key: KeyEvent) {
    // No view to draw it from: back to Keep a copy, the key spent.
    let Some(view) = gui.app.keep_dest_view().cloned() else {
        close_dest(gui);
        return;
    };
    if dest_saving(gui) {
        if key.code == KeyCode::Esc {
            dest_act(gui, DestAct::Back);
        }
        return;
    }
    let tree = gui.discover.keep.dest.as_ref().map(|ui| tree_rows(&gui.app, ui)).unwrap_or_default();
    let Some(ui) = gui.discover.keep.dest.as_mut() else { return };
    let stops = ui.stops(&view);
    let walk = |hand: Option<DestStop>, delta: i64| -> Option<DestStop> {
        match hand.and_then(|h| stops.iter().position(|s| *s == h)) {
            None => stops.first().copied(),
            Some(at) => Some(stops[(at as i64 + delta).clamp(0, stops.len() as i64 - 1) as usize]),
        }
    };
    let buttons = ui.buttons(&view);
    let held = ui.list.as_ref().and_then(|l| l.held);
    let can_save = ui.can_save(&view);
    let vars = variables(&view).len();
    let var = ui.var.min(vars.saturating_sub(1));
    let shift_tab = key.code == KeyCode::BackTab || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT));
    match (ui.hand, key.code) {
        (_, KeyCode::Esc) => dest_act(gui, DestAct::Back),
        // The open list: ↓ into it and down it, ↑ up it and back to the
        // button from its top; Enter or Space on a held row goes there.
        (Some(DestStop::Folders), KeyCode::Down) if ui.list.is_some() && !tree.is_empty() => {
            if let Some(list) = ui.list.as_mut() {
                list.held = Some(held.map_or(0, |at| (at + 1).min(tree.len() - 1)));
                list.view.pick_up();
            }
        }
        (Some(DestStop::Folders), KeyCode::Up) if held.is_some() => {
            if let Some(list) = ui.list.as_mut() {
                list.held = held.and_then(|at| at.checked_sub(1));
                list.view.pick_up();
            }
        }
        (Some(DestStop::Folders), KeyCode::Enter | KeyCode::Char(' ')) if held.is_some() => {
            dest_act(gui, DestAct::Folder(held.unwrap_or(0)))
        }
        (Some(DestStop::Folders), KeyCode::Enter | KeyCode::Char(' ')) => dest_act(gui, DestAct::Folders),
        (hand, KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab) => {
            let forward = key.code == KeyCode::Down || (key.code == KeyCode::Tab && !shift_tab);
            let next = walk(hand, if forward { 1 } else { -1 });
            if next != Some(DestStop::Folders) {
                ui.list = None;
            }
            ui.hand = next;
        }
        (Some(DestStop::Library), KeyCode::Left) => dest_act(gui, DestAct::Step(-1)),
        (Some(DestStop::Library), KeyCode::Right) => dest_act(gui, DestAct::Step(1)),
        (Some(DestStop::Library | DestStop::Field), KeyCode::Enter) if can_save => dest_save(gui),
        // Chosen with the keyboard the hand stays in the radios — the field
        // comes after them — as the webapp's does.
        (Some(DestStop::Radio(radio)), KeyCode::Enter | KeyCode::Char(' ')) => {
            dest_act(gui, DestAct::Pick(radio));
            if let Some(ui) = gui.discover.keep.dest.as_mut() {
                ui.hand = Some(DestStop::Radio(radio));
            }
        }
        (Some(DestStop::Field), _) => {
            if ui.text.handle_event(&TermEvent::Key(key)).is_some_and(|change| change.value) {
                ui.edited();
            }
        }
        (Some(DestStop::Vars), KeyCode::Left) => ui.var = ui.var.saturating_sub(1),
        (Some(DestStop::Vars), KeyCode::Right) => ui.var = (ui.var + 1).min(vars.saturating_sub(1)),
        (Some(DestStop::Vars), KeyCode::Enter | KeyCode::Char(' ')) => dest_act(gui, DestAct::Var(var)),
        (Some(button @ (DestStop::UseDefault | DestStop::Save)), KeyCode::Left | KeyCode::Right) => {
            let at = buttons.iter().position(|b| *b == button).unwrap_or(0) as i64;
            let delta = if key.code == KeyCode::Right { 1 } else { -1 };
            ui.hand = buttons.get((at + delta).clamp(0, buttons.len() as i64 - 1) as usize).copied();
        }
        // A button presses on Space or Enter, as Keep a copy's do.
        (Some(DestStop::UseDefault), KeyCode::Enter | KeyCode::Char(' ')) => dest_act(gui, DestAct::UseDefault),
        (Some(DestStop::Save), KeyCode::Enter | KeyCode::Char(' ')) if can_save => dest_save(gui),
        _ => {}
    }
}

/// The tips line while the picker is up (clause 143), by the hand — the
/// keys that work and nothing else (clause 119): "Enter save" on the libraries,
/// in the field and on Save only while Save is on.
fn dest_tips(gui: &Gui, ui: &DestUi) -> String {
    if dest_saving(gui) {
        return t!("gui.tips.discover_dest_saving").to_string();
    }
    let can_save = gui.app.keep_dest_view().is_some_and(|view| ui.can_save(view));
    let verb = |stop: DestStop| match stop {
        DestStop::UseDefault => t!("gui.discover.dest.verb_default"),
        _ => t!("gui.discover.dest.verb_save"),
    };
    match ui.hand {
        Some(DestStop::Library) if can_save => t!("gui.tips.discover_dest_library_save").to_string(),
        Some(DestStop::Library) => t!("gui.tips.discover_dest_library").to_string(),
        Some(DestStop::Radio(_)) => t!("gui.tips.discover_dest_radio").to_string(),
        Some(DestStop::Field) if can_save => t!("gui.tips.discover_dest_custom_save").to_string(),
        Some(DestStop::Field) => t!("gui.tips.discover_dest_custom").to_string(),
        Some(DestStop::Vars) => t!("gui.tips.discover_dest_vars").to_string(),
        Some(DestStop::Folders) if ui.list.is_some() => t!("gui.tips.discover_dest_list").to_string(),
        Some(DestStop::Folders) => t!("gui.tips.discover_dest_folders").to_string(),
        Some(DestStop::Save) if !can_save => t!("gui.tips.discover_dest_button_off").to_string(),
        Some(stop @ (DestStop::UseDefault | DestStop::Save)) => t!("gui.tips.discover_dest_button", verb = verb(stop)).to_string(),
        // No hand (it never rests there for long): only Esc is sure.
        None => t!("gui.tips.discover_dest_saving").to_string(),
    }
}

/// Where copies land, drawn (and registered) over everything — Keep a copy
/// is not drawn while it is open (clause 142): the kit's modal, ACCENT, 80
/// wide, centred over the whole frame and anchored as if 22 rows, so its
/// top holds as the list, the variables and a refusal come and go; its
/// title on the top border, "Esc close" on the bottom (Keep a copy's
/// [`draw_borders`]). The room beneath is inert.
fn draw_dest(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let (Some(view), Some(dview)) = (gui.app.keep_view(), gui.app.keep_dest_view().cloned()) else { return };
    dest_ask(gui);
    let saving = dest_saving(gui);
    let tree = gui.discover.keep.dest.as_ref().map(|ui| tree_rows(&gui.app, ui)).unwrap_or_default();
    let note = gui.discover.keep.dest.as_ref().and_then(|ui| tree_note(&gui.app, ui));
    // The place read again under the open picker (another tab or device
    // saved one) is taken while nothing was edited and no save is out, the
    // hand, the list and the held variable kept (the webapp's
    // `dmPickerSync`); an edit is kept for Save or Esc to settle. Nowhere
    // any more, [`draw`] closed the picker before this.
    let Some(place) = dview.destination.as_ref() else { return };
    let Some(ui) = gui.discover.keep.dest.as_mut() else { return };
    if !saving && !ui.opened_on(place) && !ui.dirty(&dview) {
        let (hand, list, var) = (ui.hand, ui.list.take(), ui.var);
        *ui = DestUi::of(place, &dview);
        ui.hand = hand;
        ui.list = list;
        ui.var = var;
        ui.anchor_list();
    }
    // A hand on a stop that left (a button gone, the field under a
    // preset), or on none once its save came back, goes to the first stop
    // — never down where no key reaches.
    if !saving && ui.hand.is_none_or(|h| !ui.stops(&dview).contains(&h)) {
        ui.hand = ui.stops(&dview).first().copied();
    }
    let shown = preview(ui, &view, &dview);
    let mut rows = dest_rows(ui, &dview, &shown, tree.len(), note.as_ref());
    give_way(&mut rows);
    let listed = rows.iter().filter(|r| matches!(r, DestRow::Listed(_))).count();
    let window = ui.list.as_mut().map(|list| list.view.window(tree.len(), list.held, listed));
    let ui = ui.clone();
    // The room beneath inert, as under Keep a copy (the kit's Modal): what
    // it registered goes before the picker registers its own.
    gui.ui.clear_registries();
    gui.ui.click(area, Act::Guard);
    let h = rows.len() as u16 + 2;
    let rect = crate::kit::modal_rect(area, DEST_W, h, DEST_ANCHOR.max(h));
    gui.ui.overlay(rect);
    gui.ui.modal_over();
    let inner = modal_box(frame, rect);
    // No band: the room's nav reaches the box's left border.
    clear_spill(frame, rect);
    gui.ui.click(rect, Act::Guard);
    let dest = |act: DestAct| Act::Disc(DiscAct::Keep(KeepAct::Dest(act)));
    // "Esc close" does what Esc does: the open list closes first.
    draw_borders(frame, gui, rect, &t!("gui.discover.dest.title"), &t!("gui.discover.keep.esc_close"), dest(DestAct::Back));
    let text_x = rect.x + DEST_TEXT;
    let tree_x = rect.x + DEST_TREE;
    let field_x = rect.x + DEST_FIELD;
    let end = rect.x + DEST_LAST + 1;
    let wide = usize::from(end.saturating_sub(text_x));
    let tree_room = usize::from(end.saturating_sub(tree_x));
    let field_room = usize::from(end.saturating_sub(field_x));
    let gold = Style::default().fg(th().gold);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let chosen = ui.chosen(&dview);
    let names = variables(&dview);
    let can_save = ui.can_save(&dview);
    // The slot's note on the rows the give-way left it — two, one clipped
    // with its whole words the tooltip, or none: then into the path's.
    let slot = match &shown {
        Preview::Path(landing) => slot_note(landing),
        _ => None,
    };
    let note_rows = rows.iter().filter(|r| matches!(r, DestRow::Note(_))).count();
    let (note_lines, note_tip) = slot.as_deref().map(|words| wrap_rows(words, wide, 0, &[], note_rows < 2)).unwrap_or_default();
    let missing_shown = rows.contains(&DestRow::Missing);
    let (dot_on, dot_off) = if legacy_conhost() { ("(*)", "( )") } else { ("(•)", "( )") };
    // Words clipped at the border: whole in their tooltip, and only then.
    let tip_clipped = |gui: &mut Gui, x: u16, y: u16, room: usize, words: &str| {
        if width(words) > room {
            gui.ui.tip(Rect { x, y, width: room as u16, height: 1 }, words.to_string());
        }
    };
    for (i, row) in rows.iter().enumerate() {
        let y = inner.y + i as u16;
        match row {
            DestRow::Blank(_) => {}
            DestRow::Preview => {
                // A path past the server's 500: its GOLD words in the row's
                // place — no fault of the layout's, Save takes it.
                if matches!(shown, Preview::TooLong) {
                    let words = t!("gui.discover.dest.too_long", max = MAX_RESOLVED_LEN).to_string();
                    put(frame, text_x, y, &super::bar::clip(&words, wide), gold);
                    tip_clipped(gui, text_x, y, wide, &words);
                    continue;
                }
                // Keep a copy's words for the same line, DIM; then the path,
                // clipped from the left — or the kit's DIM `—` while the
                // field is empty or the layout would be refused, so the
                // field never moves under the caret.
                let lead = format!("{} ", t!("gui.discover.keep.goes_to"));
                put(frame, text_x, y, &super::bar::clip(&lead, wide), dim());
                let x = text_x + (width(&lead) as u16).min(wide as u16);
                let room = wide.saturating_sub(width(&lead));
                let Preview::Path(landing) = &shown else {
                    put(frame, x, y, &super::bar::clip(super::discover::dash(), room), dim());
                    continue;
                };
                let mut runs: Vec<(String, Style)> = Vec::new();
                for (i, crumb) in landing_crumbs(landing).into_iter().enumerate() {
                    if i > 0 {
                        runs.push((crumb_sep().to_string(), dim()));
                    }
                    runs.extend(crumb);
                }
                let clipped = put_tail(frame, x, y, &runs, room);
                // The tooltip: the whole path where the row clips it, and the
                // notes the rows had no room for.
                let mut tip: Vec<String> = Vec::new();
                if clipped {
                    tip.push(whole_path(landing, None));
                }
                if note_rows == 0 {
                    tip.extend(slot.clone());
                }
                if !missing_shown {
                    tip.extend(missing_words(landing));
                }
                if !tip.is_empty() {
                    gui.ui.tip(Rect { x: text_x, y, width: wide as u16, height: 1 }, tip.join(" — "));
                }
            }
            DestRow::Note(n) => {
                let Some(line) = note_lines.get(*n) else { continue };
                put(frame, text_x, y, line, dim());
                if let Some(tip) = &note_tip {
                    gui.ui.tip(Rect { x: text_x, y, width: wide as u16, height: 1 }, tip.clone());
                }
            }
            DestRow::Missing => {
                let Preview::Path(landing) = &shown else { continue };
                if let Some(words) = missing_words(landing) {
                    put(frame, text_x, y, &super::bar::clip(&words, wide), dim());
                    tip_clipped(gui, text_x, y, wide, &words);
                }
            }
            DestRow::Library => {
                // The hand on the row shows on the chosen name, ACCENT BOLD
                // — no label to take it.
                let hand = ui.hand == Some(DestStop::Library);
                let chips = ui.chips(&dview);
                // Every library in the server's order, the chosen one marked
                // — from the chosen one behind `…` where they would not all
                // fit (the mark steps to the one before), the last that has
                // room for a dot, a space and a cell of its name clipped at
                // the right, the rest reached by ← → alone (gui/dj.rs's
                // rule: never over the border).
                let cells = |name: &str| 4 + width(name) as u16;
                let total: u16 = chips.iter().map(|name| cells(name) + 3).sum::<u16>().saturating_sub(3);
                let at = chips.iter().position(|name| *name == ui.vpath).unwrap_or(0);
                let (first, mut x) = if usize::from(total) <= wide || at == 0 {
                    (0, text_x)
                } else {
                    put(frame, text_x, y, clip_mark(), dim());
                    gui.ui.click(Rect { x: text_x, y, width: 1, height: 1 }, dest(DestAct::Step(-1)));
                    (at, text_x + 2)
                };
                for (n, name) in chips.iter().enumerate().skip(first) {
                    if x + 5 > end {
                        break;
                    }
                    let on = n == at;
                    let w = cells(name).min(end - x);
                    let cell = Rect { x, y, width: w, height: 1 };
                    let hover = gui.ui.hovers(cell);
                    put(frame, x, y, if on { dot_on } else { dot_off }, if on { Style::default().fg(th().ok) } else { dim() });
                    let style = match (hover, on) {
                        (_, true) if hand => accent().add_modifier(Modifier::BOLD),
                        (true, _) => bright_bold(),
                        (false, true) => bold,
                        (false, false) => Style::default(),
                    };
                    put(frame, x + 4, y, &super::bar::clip(name, usize::from(w.saturating_sub(4))), style);
                    gui.ui.click(cell, dest(DestAct::Library(n)));
                    x += w + 3;
                }
            }
            DestRow::Radio(radio) => {
                let on = chosen == Some(*radio);
                put(frame, text_x, y, if on { dot_on } else { dot_off }, if on { Style::default().fg(th().ok) } else { dim() });
                // Its name alone: the path above shows what it makes, and
                // Custom's field the layout it holds.
                let words = match radio {
                    Radio::Custom => t!("gui.discover.dest.custom").to_string(),
                    preset => presets(&dview, &ui.vpath)
                        .into_iter()
                        .find(|(r, _, _)| r == preset)
                        .map(|(_, key, _)| t!(key).to_string())
                        .unwrap_or_default(),
                };
                let pick = Rect { x: text_x, y, width: end - text_x, height: 1 };
                let hover = gui.ui.hovers(pick);
                let style = match (ui.hand == Some(DestStop::Radio(*radio)), hover, on) {
                    (true, _, _) => accent().add_modifier(Modifier::BOLD),
                    (false, true, _) => bright_bold(),
                    (false, false, true) => bold,
                    (false, false, false) => Style::default(),
                };
                put(frame, field_x, y, &super::bar::clip(&words, field_room), style);
                gui.ui.click(pick, dest(DestAct::Pick(*radio)));
            }
            DestRow::Field => {
                let w = field_room as u16;
                let cell = Rect { x: field_x, y, width: w, height: 1 };
                let hand = ui.hand == Some(DestStop::Field);
                if hand {
                    super::text_field(frame, &mut gui.ui, field_x, y, ui.text.value(), ui.text.cursor(), w, Style::default());
                } else if !ui.text.value().is_empty() {
                    put(frame, field_x, y, &super::clip_lead(ui.text.value(), field_room), Style::default());
                }
                // Empty: the hint after the caret, DIM.
                if ui.text.value().is_empty() {
                    let hint = ell(&t!("gui.discover.dest.custom_hint", example = format!("{}/{}", var_token("ARTIST"), var_token("ALBUM"))));
                    let x = if hand { field_x + 1 } else { field_x };
                    put(frame, x, y, &super::bar::clip(&hint, usize::from(end.saturating_sub(x))), dim());
                }
                gui.ui.click(cell, dest(DestAct::Hand(DestStop::Field)));
            }
            DestRow::Problem(line) => {
                let x = if ui.custom { field_x } else { text_x };
                put(frame, x, y, line, gold);
            }
            DestRow::Vars => {
                // Under Custom's field where they fit, else right-aligned to
                // the last cell, as far left as the text's column: a server
                // that lists more never loses one.
                let total = names.iter().map(|n| width(n)).sum::<usize>() + 3 * names.len().saturating_sub(1);
                let mut x = field_x.min(end.saturating_sub(total as u16)).max(text_x);
                let held = (ui.hand == Some(DestStop::Vars)).then_some(ui.var.min(names.len().saturating_sub(1)));
                for (at, name) in names.iter().enumerate() {
                    let w = width(name) as u16;
                    let gap = if at > 0 { 3 } else { 0 };
                    if x + gap + w > end {
                        break;
                    }
                    if at > 0 {
                        put(frame, x, y, " · ", dim());
                        x += 3;
                    }
                    let cell = Rect { x, y, width: w, height: 1 };
                    let style = if held == Some(at) {
                        accent().add_modifier(Modifier::BOLD)
                    } else if gui.ui.hovers(cell) {
                        bright_bold()
                    } else {
                        dim()
                    };
                    put(frame, x, y, name, style);
                    gui.ui.click(cell, dest(DestAct::Var(at)));
                    x += w;
                }
            }
            DestRow::Folders => {
                let open = ui.list.is_some();
                let words = ell(&t!(if open { "gui.discover.dest.hide_folders" } else { "gui.discover.dest.start_from" }));
                let cell = Rect { x: text_x, y, width: (width(&words) as u16).min(end - text_x), height: 1 };
                let style = if ui.hand == Some(DestStop::Folders) {
                    accent().add_modifier(Modifier::BOLD)
                } else if gui.ui.hovers(cell) {
                    bright_bold()
                } else {
                    dim()
                };
                put(frame, text_x, y, &super::bar::clip(&words, wide), style);
                gui.ui.click(cell, dest(DestAct::Folders));
            }
            DestRow::TreeHead => {
                // Where the list is; its rows and its bar say how many.
                let Some(list) = ui.list.as_ref() else { continue };
                put(frame, tree_x, y, &super::bar::clip(&folder_words(&ui.vpath, &list.dir), tree_room), dim());
            }
            DestRow::Listed(n) => {
                let Some(list) = ui.list.as_ref() else { continue };
                let (first, visible) = window.unwrap_or((0, 0));
                let at = first + n;
                let Some(tree_row) = tree.get(at) else { continue };
                let base = ui.base();
                let (words, pressed) = match tree_row {
                    TreeRow::Up => {
                        let parent = list.dir.iter().rev().nth(1).map_or(ui.vpath.as_str(), String::as_str);
                        (t!("gui.discover.dest.up", name = parent).to_string(), false)
                    }
                    TreeRow::Folder(name) => {
                        let mut path = list.dir.clone();
                        path.push(name.clone());
                        (name.clone(), path == base)
                    }
                };
                let slab = Rect { x: tree_x - 1, y, width: end - (tree_x - 1), height: 1 };
                let held = list.held == Some(at);
                if held {
                    frame.buffer_mut().set_style(slab, sel());
                }
                let style = match (held, gui.ui.hovers(slab), pressed) {
                    (true, _, _) => sel(),
                    (false, true, _) => Style::default().fg(th().bright),
                    (false, false, true) => bold,
                    (false, false, false) => dim(),
                };
                put(frame, tree_x, y, &super::bar::clip(&words, tree_room), style);
                gui.ui.click(slab, dest(DestAct::Folder(at)));
                if *n == 0 && tree.len() > visible {
                    let bar = Rect { x: end, y, width: 1, height: visible as u16 };
                    scroll_list(frame, &mut gui.ui, bar, tree.len(), visible, first, dest(DestAct::Scroll(-1)), dest(DestAct::Scroll(1)), move |n| {
                        Act::Disc(DiscAct::Keep(KeepAct::Dest(DestAct::Jump(n))))
                    });
                }
            }
            DestRow::TreeNote(words, is_gold) => {
                let words = ell(words);
                put(frame, tree_x, y, &super::bar::clip(&words, tree_room), if *is_gold { gold } else { dim() });
                tip_clipped(gui, tree_x, y, tree_room, &words);
            }
            DestRow::Buttons => {
                // Saving: `[ Saving… ]` DIM where Save stands, no click — the
                // kit's modal primary that goes off — and Use the library
                // default not drawn.
                if saving {
                    let words = format!("[ {} ]", ell(&t!("gui.discover.dest.saving")));
                    put(frame, end.saturating_sub(width(&words) as u16), y, &words, dim());
                    continue;
                }
                // The webapp's footer: Use the library default at the left
                // edge (the text's column), Save at the right.
                for stop in ui.buttons(&dview) {
                    let text = match stop {
                        DestStop::UseDefault => format!("[ {} ]", t!("gui.discover.dest.use_default")),
                        _ => format!("[ {} ]", t!("gui.discover.dest.save")),
                    };
                    let w = width(&text) as u16;
                    let x = if stop == DestStop::UseDefault { text_x } else { end.saturating_sub(w) };
                    let cell = Rect { x, y, width: w, height: 1 };
                    let on = stop != DestStop::Save || can_save;
                    let style = if ui.hand == Some(stop) {
                        sel().add_modifier(Modifier::BOLD)
                    } else if on && gui.ui.hovers(cell) {
                        bright_bold()
                    } else if stop == DestStop::Save && on {
                        accent().add_modifier(Modifier::BOLD)
                    } else {
                        dim()
                    };
                    put(frame, x, y, &text, style);
                    if on {
                        let act = if stop == DestStop::UseDefault { DestAct::UseDefault } else { DestAct::Save };
                        gui.ui.click(cell, dest(act));
                    } else {
                        // Off: no click, no hover — its tooltip says why (the kit's rule).
                        gui.ui.tip(cell, ui.save_off(&dview));
                    }
                }
            }
        }
    }
}

// ── The Copies modal (clause 117) ───────────────────────────────────────────

/// The list's width — Keep a copy's, the two opening in the same place
/// ([`window_rect`]) — and the height it never grows past: past it, or past
/// the rows down to the room's last, its list scrolls.
const COPIES_W: u16 = KEEP_W;
const COPIES_MAX_H: u16 = 22;
/// The rows above the list: the title, a blank, the header, its rule.
const COPIES_HEAD: u16 = 4;
/// The kit table from its first cell: TITLE 31 · FROM 12 · STATE 20 ·
/// `[X]` 3, two cells between them — 72 cells, a cell of pad each side.
const COL_FROM: u16 = 33;
const COL_STATE: u16 = 47;
const COL_X: u16 = 69;
const TABLE_W: u16 = 72;

/// The Copies modal's own state — the jobs tray the indicator and `C`
/// open (clause 117). Everything it lists is the App's
/// ([`crate::tui::app::App::copies_rows`]).
#[derive(Debug, Default, Clone)]
pub(crate) struct CopiesUi {
    /// The held copy, by its job's id: the hand stays on its copy while the
    /// list reorders around it. `None` is stowed — no row lit (the kit's
    /// list-cursor law).
    pub held: Option<i64>,
    /// The list's viewport, counted in copies (each takes two rows).
    pub view: ListView,
    /// What the modal says once — a cancel that lost its race, a refused
    /// retry — gone on the next key.
    pub note: Option<KeepNote>,
}

/// What the list shows under its header.
enum CopiesBody {
    /// The first ask is out: "asking your server for copies…".
    Asking,
    /// The first ask failed: its words and `[ Try again ]`.
    LoadFailed(crate::api::PluginFailure),
    /// "Nothing copying." — before any copy, or after Clear finished.
    Empty,
    Rows(Vec<CopyRow>),
}

/// The list's body, and the failure of a refresh that kept its rows.
fn copies_body(app: &crate::tui::app::App) -> (CopiesBody, Option<crate::api::PluginFailure>) {
    let book = &app.taste.keep.book;
    let rows = app.copies_rows();
    if !rows.is_empty() {
        return (CopiesBody::Rows(rows), book.failed.clone());
    }
    match (&book.failed, book.listed) {
        (Some(failure), false) => (CopiesBody::LoadFailed(failure.clone()), None),
        (None, false) => (CopiesBody::Asking, None),
        (failed, true) => (CopiesBody::Empty, failed.clone()),
    }
}

/// The verbs line's indicator, or `C`: the Copies modal, its first ask now.
pub(crate) fn open_copies(gui: &mut Gui) {
    if !gui.app.keep_gate() {
        return;
    }
    let effects = gui.app.copies_open();
    gui.pend(effects);
    gui.discover.copies = Some(CopiesUi::default());
}

/// A copy's STATE cell (clause 117): the bar while it runs, else one word.
enum StateCell {
    Bar(Option<f64>),
    Word(String, Style),
}

fn state_cell(row: &CopyRow) -> StateCell {
    let gold = Style::default().fg(th().gold);
    let ok = Style::default().fg(th().ok);
    let glyphed = |mark: JobMark, key: &str| format!("{} {}", mark_glyph(mark), ell(&t!(key)));
    match &row.outcome {
        // A cancel asked — the server's flag, or this player's ask still
        // out: winding down, no `[X]` left to press.
        JobOutcome::Running { .. } if row.job.stopping() || row.cancelling => {
            StateCell::Word(ell(&t!("gui.discover.copies.stopping")), dim())
        }
        JobOutcome::Running { progress, .. } => StateCell::Bar(*progress),
        JobOutcome::Queued => StateCell::Word(glyphed(JobMark::Waiting, "gui.discover.mark.waiting"), dim()),
        // In your collection only when something was copied (clause 120).
        JobOutcome::Copied { .. } | JobOutcome::CopiedMany(_) => {
            StateCell::Word(format!("{} {}", tick(), t!("gui.discover.copies.kept")), ok)
        }
        JobOutcome::Owned { .. } => StateCell::Word(format!("{} {}", tick(), t!("gui.discover.copies.yours")), dim()),
        JobOutcome::Exists { .. } => StateCell::Word(t!("gui.discover.copies.skipped").to_string(), dim()),
        JobOutcome::Failed { .. } => StateCell::Word(glyphed(JobMark::Failed, "gui.discover.mark.failed"), gold),
        JobOutcome::Stopped { .. } => StateCell::Word(glyphed(JobMark::Stopped, "gui.discover.mark.stopped"), gold),
        JobOutcome::Refused { .. } => StateCell::Word(glyphed(JobMark::Refused, "gui.discover.mark.refused"), gold),
        JobOutcome::Cancelled { .. } => {
            StateCell::Word(glyphed(JobMark::Cancelled, "gui.discover.mark.cancelled"), dim())
        }
        JobOutcome::Nothing { .. } => StateCell::Word(t!("gui.discover.copies.nothing").to_string(), dim()),
        JobOutcome::Done => StateCell::Word(t!("gui.discover.copies.done").to_string(), dim()),
        JobOutcome::Unknown => StateCell::Word(String::new(), dim()),
    }
}

/// A Copies row's TITLE (clause 117): the song, the album, or "every album
/// by {artist}".
fn copy_title(row: &CopyRow) -> String {
    match &row.title {
        CopyTitle::Song(s) | CopyTitle::Album(s) => s.clone(),
        CopyTitle::Artist { artist, .. } => t!("gui.discover.copies.every_album", artist = artist).to_string(),
    }
}

/// The second row's opening word (clause 117): "song", "album", "only the
/// albums you don't have" — an artist copy opens with neither.
fn copy_kind(row: &CopyRow) -> Option<String> {
    match &row.title {
        CopyTitle::Song(_) => Some(t!("gui.discover.copies.kind_song").to_string()),
        CopyTitle::Album(_) => Some(t!("gui.discover.copies.kind_album").to_string()),
        CopyTitle::Artist { missing_only: true, .. } => Some(t!("gui.discover.copies.kind_missing").to_string()),
        CopyTitle::Artist { missing_only: false, .. } => None,
    }
}

/// Where a song landed, by its folder: "copies › Color Out › Peer
/// Sessions" — the file's own name left out.
fn landed_crumbs(filepath: &str) -> String {
    let parts: Vec<&str> = filepath.split('/').filter(|p| !p.is_empty()).collect();
    match parts.split_last() {
        Some((_, dirs)) if !dirs.is_empty() => dirs.join(crumb_sep()),
        _ => filepath.to_string(),
    }
}

/// The second row's words after its opening (clause 117): the step, the
/// reason in GOLD, or the outcome — numbers from `result.counts` only.
fn copy_detail(row: &CopyRow) -> Vec<(String, Style)> {
    let gold = Style::default().fg(th().gold);
    let server = row.from.clone().unwrap_or_default();
    let copied = |n: Option<ManyCounts>| -> Option<(String, Style)> {
        n.filter(|n| n.copied > 0).map(|n| (count("gui.discover.keep.copied", n.copied), dim()))
    };
    let mut runs: Vec<(String, Style)> = Vec::new();
    match &row.outcome {
        JobOutcome::Queued => runs.push((t!("gui.discover.copies.waiting").to_string(), dim())),
        JobOutcome::Running { step, stopping, .. } => {
            let words = match (step, *stopping || row.cancelling) {
                (Some(step), true) => t!("gui.discover.copies.stopping_after", step = step).to_string(),
                (None, true) => ell(&t!("gui.discover.copies.stopping")),
                (Some(step), false) => step.clone(),
                (None, false) => ell(&t!("gui.discover.copies.copying")),
            };
            runs.push((words, dim()));
        }
        JobOutcome::Copied { filepath, .. } => runs.push((landed_crumbs(filepath), dim())),
        JobOutcome::CopiedMany(n) => runs.extend(many_runs(n, dim()).into_iter().map(|(w, _)| (w, dim()))),
        JobOutcome::Owned { many: None, .. } => runs.push((t!("gui.discover.copies.had_it").to_string(), dim())),
        JobOutcome::Owned { many: Some(_), .. } => {
            runs.push((t!("gui.discover.copies.had_them").to_string(), dim()))
        }
        JobOutcome::Exists { .. } => runs.push((t!("gui.discover.copies.exists").to_string(), dim())),
        JobOutcome::Nothing { appearances, .. } => {
            let key = if *appearances > 0 { "gui.discover.copies.nothing_theirs" } else { "gui.discover.copies.all_yours" };
            runs.push((t!(key).to_string(), dim()));
        }
        JobOutcome::Failed { error: Some(error), many } => {
            runs.push((error.clone(), gold));
            runs.extend(copied(*many));
        }
        JobOutcome::Failed { error: None, many: Some(n) } if n.failed > 0 => {
            runs.push((plural("gui.discover.copies.failed_none", n.failed), gold));
        }
        JobOutcome::Failed { error: None, .. } => runs.push((t!("gui.discover.copies.failed_plain").to_string(), gold)),
        JobOutcome::Stopped { why, many } => {
            let reason = match why {
                StopReason::Quota => t!("gui.discover.copies.stopped_quota", server = server),
                StopReason::Busy => t!("gui.discover.copies.stopped_busy", server = server),
                StopReason::Peer | StopReason::Refused => t!("gui.discover.keep.stopped_peer", server = server),
                StopReason::Other => t!("gui.discover.copies.stopped_other"),
            };
            runs.push((reason.to_string(), gold));
            runs.extend(copied(Some(*many)));
        }
        JobOutcome::Refused { many } => {
            runs.push((t!("gui.discover.keep.refused", server = server).to_string(), gold));
            runs.extend(copied(*many));
        }
        JobOutcome::Cancelled { many } => {
            runs.push((t!("gui.discover.copies.cancelled").to_string(), dim()));
            runs.extend(copied(*many));
        }
        JobOutcome::Done => runs.push((t!("gui.discover.copies.finished").to_string(), dim())),
        JobOutcome::Unknown => {}
    }
    runs
}

/// A row's action, in the words its second row ends with (clause 117) —
/// a live copy's is its `[X]`, not words. A copy with gaps reads Retry (it
/// takes only the gaps), a cancelled one Add again — the window's words.
fn action_words(action: JobAction) -> Option<String> {
    match action {
        JobAction::Cancel => None,
        JobAction::Retry | JobAction::CopyAgain => Some(t!("gui.discover.keep.retry").to_string()),
        JobAction::StartAgain => Some(t!("gui.discover.keep.add_again").to_string()),
        JobAction::Play => Some(t!("gui.discover.keep.play_copy").to_string()),
    }
}

/// Whether a row offers its `[X]`: live, and no cancel asked yet.
fn cancellable(row: &CopyRow) -> bool {
    row.action == Some(JobAction::Cancel) && !row.cancelling
}

/// The note's words, and its token: the race's TEXT, a failure's GOLD.
fn copies_note_words(note: &KeepNote) -> (String, Style) {
    let gold = Style::default().fg(th().gold);
    match note {
        KeepNote::TooLate { title: Some(title) } => {
            (t!("gui.discover.copies.too_late", title = title).to_string(), Style::default())
        }
        KeepNote::TooLate { title: None } => (t!("gui.discover.copies.too_late_plain").to_string(), Style::default()),
        KeepNote::CancelFailed(f) => (t!("gui.discover.copies.cancel_failed", words = failure_words(f)).to_string(), gold),
        KeepNote::ClearFailed(f) => (t!("gui.discover.copies.clear_failed", words = failure_words(f)).to_string(), gold),
        KeepNote::StartFailed(f) => (t!("gui.discover.copies.refused_start", words = failure_words(f)).to_string(), gold),
    }
}

/// The Copies modal (clause 117; the canvas's TermCopies board): the kit's
/// neutral modal where Keep a copy opens ([`window_rect`]: 76 wide,
/// centred in the room with its band, anchored as if 17 rows), its height
/// following its rows down to the room's last row, then rising toward row
/// 1, never past 22 — the list scrolls past that, `▲█▼` on its right
/// border.
pub(crate) fn draw_copies(frame: &mut Frame, gui: &mut Gui, area: Rect, room: Rect) {
    if let Some(note) = gui.app.keep_take_note()
        && let Some(ui) = gui.discover.copies.as_mut()
    {
        ui.note = Some(note);
    }
    let (body, refresh) = copies_body(&gui.app);
    let cap = COPIES_MAX_H.min(rows_risen(area, room));
    // Room for the list: the head, the spacer (or the refresh's error line)
    // and the footer row, inside the border.
    let avail = cap.saturating_sub(COPIES_HEAD + 4);
    let n = match &body {
        CopiesBody::Rows(rows) => rows.len(),
        _ => 0,
    };
    let fits = usize::from(avail / 2).max(1);
    let list_h: u16 = match &body {
        CopiesBody::Asking | CopiesBody::LoadFailed(_) => 1,
        CopiesBody::Empty => 2,
        CopiesBody::Rows(_) => 2 * n.min(fits) as u16,
    };
    let h = 2 + COPIES_HEAD + list_h + 2;
    // The room beneath is inert (the kit's Modal; no scrim — the band).
    let (rect, band) = window_rect(area, room, COPIES_W, h);
    let (inner, _) = open_window(frame, gui, area, rect, band);
    put(frame, inner.x + 1, inner.y, &t!("gui.discover.copies.title"), accent().add_modifier(Modifier::BOLD));
    // Its `[X]` stands over the rows' `[X]` column, a cell in from the border.
    let close_in = Rect { width: inner.width.saturating_sub(1), ..inner };
    crate::kit::modal_close(frame, &mut gui.ui, close_in, Act::Disc(DiscAct::CopiesClose));
    let tx = inner.x + 1;
    let head = inner.y + 2;
    let label = |key: &str| t!(key).to_uppercase();
    put(frame, tx, head, &label("gui.discover.copies.col_title"), dim());
    put(frame, tx + COL_FROM, head, &label("gui.discover.copies.col_from"), dim());
    put(frame, tx + COL_STATE, head, &label("gui.discover.copies.col_state"), dim());
    let rule = if legacy_conhost() { "-" } else { "─" };
    let table_w = TABLE_W.min(inner.width.saturating_sub(2));
    put(frame, tx, head + 1, &rule.repeat(usize::from(table_w)), dim());
    let top = inner.y + COPIES_HEAD;
    let room = usize::from(table_w);
    let mut clearable = false;
    match &body {
        CopiesBody::Asking => {
            put(frame, tx, top, &super::bar::clip(&ell(&t!("gui.discover.copies.asking")), room), accent());
        }
        CopiesBody::LoadFailed(failure) => {
            let words = t!("gui.discover.copies.load_failed", words = failure_words(failure)).to_string();
            put(frame, tx, top, &super::bar::clip(&words, room), Style::default().fg(th().gold));
        }
        CopiesBody::Empty => {
            put(frame, tx, top, &super::bar::clip(&t!("gui.discover.copies.empty"), room), Style::default());
            put(frame, tx, top + 1, &super::bar::clip(&t!("gui.discover.copies.empty_sub"), room), dim());
        }
        CopiesBody::Rows(rows) => {
            clearable = rows.iter().any(|r| r.job.state.finished());
            let held_id = gui.discover.copies.as_ref().and_then(|ui| ui.held);
            let held_at = held_id.and_then(|id| rows.iter().position(|r| r.job.id == id));
            let (first, shown) = match gui.discover.copies.as_mut() {
                Some(ui) => ui.view.window(rows.len(), held_at, fits),
                None => (0, rows.len().min(fits)),
            };
            for (i, row) in rows.iter().enumerate().skip(first).take(shown) {
                let y = top + 2 * (i - first) as u16;
                draw_copy(frame, gui, tx, y, table_w, row, held_at == Some(i));
            }
            if rows.len() > shown {
                let bar = Rect { x: rect.right().saturating_sub(1), y: top, width: 1, height: 2 * shown as u16 };
                crate::kit::scroll_items(
                    frame,
                    &mut gui.ui,
                    bar,
                    rows.len(),
                    shown,
                    first,
                    Act::Disc(DiscAct::CopiesScroll(-1)),
                    Act::Disc(DiscAct::CopiesScroll(1)),
                    |i| Act::Disc(DiscAct::CopiesJump(i)),
                );
            }
        }
    }
    // The spacer row: a refresh that failed with its rows kept says so,
    // and when the poll tries again while a copy is live.
    let spacer = top + list_h;
    if let Some(failure) = &refresh {
        let words = t!("gui.discover.copies.refresh_failed", words = failure_words(failure)).to_string();
        let shown = super::bar::clip(&words, room).into_owned();
        put(frame, tx, spacer, &shown, Style::default().fg(th().gold));
        let live = matches!(&body, CopiesBody::Rows(rows) if rows.iter().any(|r| r.outcome.live()));
        if live && let Some(wait) = gui.app.keep_next_try(crate::clock::Instant::now()) {
            let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
            let at = tx + width(&shown) as u16;
            let left = usize::from((tx + table_w).saturating_sub(at));
            let tail = format!(" · {}", t!("gui.discover.copies.next_try", secs = secs));
            put(frame, at, spacer, &super::bar::clip(&tail, left), dim());
        }
    }
    // The footer row: the note at the left, Try again and Clear finished
    // at the right.
    let footer = spacer + 1;
    let mut right = tx + table_w;
    let mut text_button = |frame: &mut Frame, gui: &mut Gui, words: String, style: Style, act: DiscAct| {
        let w = width(&words) as u16;
        let x = right.saturating_sub(w);
        let at = Rect { x, y: footer, width: w, height: 1 };
        let style = if gui.ui.hovers(at) { bright_bold() } else { style };
        put(frame, x, footer, &words, style);
        gui.ui.click(at, Act::Disc(act));
        right = x.saturating_sub(3);
    };
    match &body {
        CopiesBody::LoadFailed(_) => {
            let words = format!("[ {} ]", t!("gui.discover.retry"));
            text_button(frame, gui, words, accent().add_modifier(Modifier::BOLD), DiscAct::CopiesTryAgain);
        }
        _ => {
            if clearable {
                text_button(frame, gui, t!("gui.discover.copies.clear").to_string(), dim(), DiscAct::CopiesClear);
            }
            if refresh.is_some() {
                text_button(frame, gui, t!("gui.discover.retry").to_string(), accent(), DiscAct::CopiesTryAgain);
            }
        }
    }
    if let Some(note) = gui.discover.copies.as_ref().and_then(|ui| ui.note.clone()) {
        let (words, style) = copies_note_words(&note);
        let left = usize::from(right.saturating_sub(tx));
        // Words the buttons would cut (a start refused for a moved
        // destination) take the spacer row above, whole, while it is free.
        if width(&words) > left && refresh.is_none() && !matches!(body, CopiesBody::LoadFailed(_)) {
            put(frame, tx, spacer, &super::bar::clip(&words, room), style);
        } else {
            put(frame, tx, footer, &super::bar::clip(&words, left), style);
        }
    }
}

/// One copy's two rows (clause 117): TITLE · FROM · STATE · `[X]`, then
/// its opening word, the step, reason or outcome, and its action.
fn draw_copy(frame: &mut Frame, gui: &mut Gui, tx: u16, y: u16, table_w: u16, row: &CopyRow, held: bool) {
    let slab = Rect { x: tx, y, width: table_w, height: 2 };
    if held {
        frame.render_widget(ratatui::widgets::Block::default().style(sel()), slab);
    }
    let paint = |style: Style| if held { sel() } else { style };
    let cell = |w: u16| usize::from(w.min(table_w));
    put(frame, tx, y, &super::bar::clip(&copy_title(row), cell(31)), paint(Style::default()));
    if let Some(from) = &row.from {
        put(frame, tx + COL_FROM, y, &super::bar::clip(from, cell(12)), paint(Style::default()));
    }
    match state_cell(row) {
        StateCell::Bar(progress) => draw_bar_in(frame, tx + COL_STATE, y, progress, held),
        StateCell::Word(words, style) => {
            put(frame, tx + COL_STATE, y, &super::bar::clip(&words, cell(20)), paint(style));
        }
    }
    if cancellable(row) {
        let at = Rect { x: tx + COL_X, y, width: 3, height: 1 };
        let style = if gui.ui.hovers(at) {
            Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
        } else {
            paint(dim())
        };
        put(frame, at.x, y, "[X]", style);
        gui.ui.click(at, Act::Disc(DiscAct::CopiesCancel(row.job.id)));
        gui.ui.tip_keyed(at, t!("gui.discover.copies.cancel_tip").to_string());
    }
    // The second row: the action right-aligned, the words before it.
    let y2 = y + 1;
    let end = tx + table_w;
    let mut room_end = end;
    if let Some(words) = row.action.and_then(action_words) {
        let w = width(&words) as u16;
        let at = Rect { x: end.saturating_sub(w), y: y2, width: w, height: 1 };
        // The held copy is one plain slab across both rows (the TermCopies
        // board): its action too, never bold.
        let style = if held {
            sel()
        } else if gui.ui.hovers(at) {
            bright_bold()
        } else {
            accent()
        };
        put(frame, at.x, y2, &words, style);
        gui.ui.click(at, Act::Disc(DiscAct::CopiesAct(row.job.id)));
        room_end = at.x.saturating_sub(1);
    }
    let mut at = tx + 2;
    let mut runs: Vec<(String, Style)> = copy_kind(row).map(|k| (k, dim())).into_iter().collect();
    runs.extend(copy_detail(row));
    for (i, (words, style)) in runs.iter().enumerate() {
        if i > 0 {
            let left = usize::from(room_end.saturating_sub(at));
            let sep = super::bar::clip(" · ", left).into_owned();
            put(frame, at, y2, &sep, paint(dim()));
            at += width(&sep) as u16;
        }
        let left = usize::from(room_end.saturating_sub(at));
        if left == 0 {
            break;
        }
        let shown = super::bar::clip(words, left).into_owned();
        put(frame, at, y2, &shown, paint(*style));
        at += width(&shown) as u16;
    }
}

/// [`draw_bar`] in a Copies row: the held row's bar in the selection's
/// colours.
fn draw_bar_in(frame: &mut Frame, x: u16, y: u16, progress: Option<f64>, held: bool) {
    if !held {
        draw_bar(frame, x, y, progress);
        return;
    }
    if legacy_conhost() {
        let words = match progress {
            Some(p) => t!("gui.discover.keep.pct_done", pct = (p * 100.0).round() as u32).to_string(),
            None => t!("gui.discover.keep.no_estimate").to_string(),
        };
        put(frame, x, y, &words, sel());
        return;
    }
    let filled = progress.map_or(0, |p| ((p * 10.0).round() as usize).min(10));
    put(frame, x, y, &format!("{}{}", "▰".repeat(filled), "▱".repeat(10 - filled)), sel());
    if let Some(p) = progress {
        put(frame, x + 11, y, &format!("{}%", (p * 100.0).round() as u32), sel());
    }
}

/// The Copies modal's rows as the keys walk them, and the held one.
fn copies_held(gui: &Gui) -> (Vec<CopyRow>, Option<usize>) {
    let rows = gui.app.copies_rows();
    let held = gui.discover.copies.as_ref().and_then(|ui| ui.held).and_then(|id| rows.iter().position(|r| r.job.id == id));
    (rows, held)
}

/// The Copies modal's acts (clause 117).
pub(crate) fn copies_act(gui: &mut Gui, act: &DiscAct) {
    let effects = match act {
        // A row's action: Retry, Copy again and Start again start the same
        // copy again; Play your copy plays it.
        DiscAct::CopiesAct(id) => gui.app.copies_act(*id),
        DiscAct::CopiesCancel(id) => gui.app.keep_cancel_job(*id),
        DiscAct::CopiesClear => gui.app.copies_clear(),
        DiscAct::CopiesTryAgain => gui.app.copies_try_again(),
        DiscAct::CopiesScroll(delta) => {
            if let Some(ui) = gui.discover.copies.as_mut() {
                ui.view.step(*delta);
            }
            Vec::new()
        }
        DiscAct::CopiesJump(first) => {
            if let Some(ui) = gui.discover.copies.as_mut() {
                ui.view.scroll = *first;
            }
            Vec::new()
        }
        _ => Vec::new(),
    };
    gui.pend(effects);
}

/// The wheel over the Copies modal: its list, a copy a notch.
pub(crate) fn copies_wheel(gui: &mut Gui, delta: i32) {
    if let Some(ui) = gui.discover.copies.as_mut() {
        ui.view.step(delta);
    }
}

/// The Copies modal's keys (clause 117): ↑ ↓ rows, Enter the held row's
/// action (Try again after a failed ask), `x` cancels the held live copy,
/// `X` clears the finished, Esc closes — copies keep running. A note goes
/// on the next key.
pub(crate) fn copies_key(gui: &mut Gui, key: KeyEvent) {
    if let Some(ui) = gui.discover.copies.as_mut() {
        ui.note = None;
    }
    let (rows, held) = copies_held(gui);
    let walk = |gui: &mut Gui, delta: i64| {
        if rows.is_empty() {
            return;
        }
        let at = match held {
            None => 0,
            Some(at) => (at as i64 + delta).clamp(0, rows.len() as i64 - 1) as usize,
        };
        if let Some(ui) = gui.discover.copies.as_mut() {
            ui.held = Some(rows[at].job.id);
            ui.view.pick_up();
        }
    };
    let row = held.and_then(|at| rows.get(at));
    match key.code {
        KeyCode::Esc => gui.discover.copies = None,
        KeyCode::Down => walk(gui, 1),
        KeyCode::Up => walk(gui, -1),
        KeyCode::Enter => match row.and_then(|r| r.action.map(|a| (r.job.id, a))) {
            Some((_, JobAction::Cancel)) => {}
            Some((id, _)) => copies_act(gui, &DiscAct::CopiesAct(id)),
            None if gui.app.taste.keep.book.failed.is_some() => copies_act(gui, &DiscAct::CopiesTryAgain),
            None => {}
        },
        KeyCode::Char('x') => {
            if let Some(row) = row.filter(|r| cancellable(r)) {
                copies_act(gui, &DiscAct::CopiesCancel(row.job.id));
            }
        }
        KeyCode::Char('X') if gui.app.copies_clearable() => copies_act(gui, &DiscAct::CopiesClear),
        _ => {}
    }
}

/// The tips line while the Copies modal is up (clause 119): by the held
/// row, a key only where it works — "Esc close" while it asks, after an
/// error, and empty.
pub(crate) fn copies_tips(gui: &Gui) -> String {
    let (body, _) = copies_body(&gui.app);
    let CopiesBody::Rows(rows) = body else { return t!("gui.tips.discover_copies_esc").to_string() };
    let held = gui.discover.copies.as_ref().and_then(|ui| ui.held).and_then(|id| rows.iter().find(|r| r.job.id == id));
    let mut parts = vec![t!("gui.tips.discover_copies_rows").to_string()];
    let verb = held.and_then(|row| match row.action {
        Some(JobAction::Cancel) if cancellable(row) => Some("gui.tips.discover_copies_cancel"),
        Some(JobAction::Retry | JobAction::CopyAgain) => Some("gui.tips.discover_copies_retry"),
        Some(JobAction::StartAgain) => Some("gui.tips.discover_copies_start_again"),
        Some(JobAction::Play) => Some("gui.tips.discover_copies_play"),
        _ => None,
    });
    if let Some(verb) = verb {
        parts.push(t!(verb).to_string());
    }
    if rows.iter().any(|r| r.job.state.finished()) {
        parts.push(t!("gui.tips.discover_copies_clear").to_string());
    }
    parts.push(t!("gui.tips.discover_copies_esc").to_string());
    parts.join(" · ")
}

// ── Open elsewhere (clauses 140–141) ────────────────────────────────────────

/// What a link's hand-off answers: its label, its URL and how it went.
type Opened = (String, String, Result<HandOff, String>);

/// Open elsewhere's list, hung from the track-actions sheet's row: the
/// keyboard's stop, the viewport, where its row stood last frame, and the
/// opener's answers on their way back from its thread.
pub(crate) struct LinksUi {
    /// The keyboard's stop — a link, or Try again; `None` while the
    /// pointer opened it (nothing lit).
    pub held: Option<usize>,
    pub view: ListView,
    /// The sheet's inner rect, the Open elsewhere… row's y and its label's
    /// last cell, as the sheet drew them this frame.
    pub anchor: Option<(Rect, u16, u16)>,
    /// Where the list stood last frame, for the wheel.
    pub at: Option<Rect>,
    tx: std::sync::mpsc::Sender<Opened>,
    rx: std::sync::mpsc::Receiver<Opened>,
}

impl Default for LinksUi {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        LinksUi { held: None, view: ListView::default(), anchor: None, at: None, tx, rx }
    }
}

impl LinksUi {
    /// A list (re)opened: nothing held, the view at its top.
    pub(crate) fn reset(&mut self) {
        self.held = None;
        self.view = ListView::default();
    }
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq)]
enum LinkLine {
    /// CANONICAL or SEARCH, DIM uppercase.
    Head(String),
    /// A link, by its place in [`web_links`].
    Link(usize, String),
    Words(String, Style),
    /// Try again, after a failure.
    Retry,
}

/// The https links of the list's answer, CANONICAL first (clause 140) —
/// nothing else reaches the opener.
fn web_links(app: &crate::tui::app::App) -> Vec<PluginLink> {
    let Some(LinksStatus::Ready(links)) = app.taste.keep.links.as_ref().map(|l| &l.status) else { return Vec::new() };
    let web: Vec<&PluginLink> = links.iter().filter(|l| l.is_web() && !l.label.trim().is_empty()).collect();
    web.iter().filter(|l| l.canonical()).chain(web.iter().filter(|l| !l.canonical())).map(|l| (*l).clone()).collect()
}

/// The list's lines for its state, wrapped to `room` cells.
fn link_lines(app: &crate::tui::app::App, room: usize) -> Vec<LinkLine> {
    let Some(state) = app.taste.keep.links.as_ref() else { return Vec::new() };
    match &state.status {
        LinksStatus::Asking => vec![LinkLine::Words(ell(&t!("gui.discover.asking")), accent())],
        LinksStatus::Failed(failure) => {
            let gold = Style::default().fg(th().gold);
            let mut lines = vec![LinkLine::Words(t!("gui.discover.links.failed").to_string(), gold)];
            for line in crate::kit::wrap_words(&failure_words(failure), room) {
                lines.push(LinkLine::Words(line, gold));
            }
            lines.push(LinkLine::Retry);
            lines
        }
        LinksStatus::Ready(_) => {
            let links = web_links(app);
            if links.is_empty() {
                return vec![LinkLine::Words(t!("gui.discover.links.none").to_string(), dim())];
            }
            let mut lines = Vec::new();
            let mut section: Option<bool> = None;
            for (i, link) in links.iter().enumerate() {
                if section != Some(link.canonical()) {
                    let key = if link.canonical() { "gui.discover.links.canonical" } else { "gui.discover.links.search" };
                    lines.push(LinkLine::Head(t!(key).to_uppercase()));
                    section = Some(link.canonical());
                }
                lines.push(LinkLine::Link(i, link.label.clone()));
            }
            lines
        }
    }
}

/// The lines a key or a click can hold: the links, or Try again.
fn link_stops(lines: &[LinkLine]) -> Vec<usize> {
    lines.iter().enumerate().filter(|(_, l)| matches!(l, LinkLine::Link(..) | LinkLine::Retry)).map(|(i, _)| i).collect()
}

/// The held stop, Try again held whenever it stands alone (clause 140:
/// "Try again is held, so Enter asks again").
fn links_held(gui: &Gui, lines: &[LinkLine]) -> Option<usize> {
    let stops = link_stops(lines);
    let retry = lines.contains(&LinkLine::Retry);
    let held = gui.discover.links.held.filter(|h| *h < stops.len());
    if retry { held.or(Some(0)) } else { held }
}

/// The list's inner width: the canvas's 30 cells, wider for a longer label.
const LINKS_INNER: u16 = 30;

/// Open elsewhere's list (clause 140): a frame, ACCENT, hung beside the
/// sheet's Open elsewhere… row — its last line level with the row, rising
/// over the rows above, clear of `[X]` and the sheet's borders.
pub(crate) fn draw_links(frame: &mut Frame, gui: &mut Gui) {
    gui.discover.links.at = None;
    let Some((sheet, row_y, label_end)) = gui.discover.links.anchor else { return };
    if gui.app.taste.keep.links.is_none() {
        return;
    }
    let right = sheet.right().saturating_sub(3);
    let max_w = right.saturating_sub(label_end + 2);
    let probe = link_lines(&gui.app, usize::from(LINKS_INNER - 2));
    let widest = probe
        .iter()
        .map(|l| match l {
            LinkLine::Head(w) => width(w) + 2,
            LinkLine::Link(_, w) => width(w) + 4,
            LinkLine::Words(..) | LinkLine::Retry => 0,
        })
        .max()
        .unwrap_or(0) as u16;
    let w = (LINKS_INNER.max(widest) + 2).min(max_w).max(12);
    let lines = link_lines(&gui.app, usize::from(w.saturating_sub(4)).max(1));
    let bottom = row_y + 1;
    let tall = (bottom + 1).saturating_sub(sheet.y);
    let h = (lines.len() as u16 + 2).min(tall).max(3);
    let rect = Rect { x: right.saturating_sub(w), y: (bottom + 1).saturating_sub(h), width: w, height: h };
    gui.ui.overlay(rect);
    let inner = modal_box(frame, rect);
    gui.ui.click(rect, Act::Guard);
    gui.discover.links.at = Some(rect);
    let held = links_held(gui, &lines);
    let stops = link_stops(&lines);
    let held_line = held.and_then(|h| stops.get(h).copied());
    let visible = usize::from(inner.height);
    let (first, shown) = gui.discover.links.view.window(lines.len(), held_line, visible);
    for (i, line) in lines.iter().enumerate().skip(first).take(shown) {
        let y = inner.y + (i - first) as u16;
        let row = Rect { x: inner.x, y, width: inner.width, height: 1 };
        let room = usize::from(inner.width.saturating_sub(2));
        match line {
            LinkLine::Head(words) => put(frame, inner.x + 1, y, &super::bar::clip(words, room), dim()),
            LinkLine::Words(words, style) => put(frame, inner.x + 1, y, &super::bar::clip(words, room), *style),
            LinkLine::Link(_, _) | LinkLine::Retry => {
                let on = held_line == Some(i);
                let hover = !on && gui.ui.hovers(row);
                if on {
                    frame.render_widget(ratatui::widgets::Block::default().style(sel()), row);
                }
                // The canvas's TermCopies G: the held link in the plain
                // selection colours, the hovered one BRIGHT — no bold.
                let style = match (on, hover) {
                    (true, _) => sel(),
                    (false, true) => Style::default().fg(th().bright),
                    (false, false) if *line == LinkLine::Retry => accent(),
                    (false, false) => Style::default(),
                };
                let (words, act) = match line {
                    LinkLine::Link(index, label) => (label.clone(), DiscAct::LinkOpen(*index)),
                    _ => (t!("gui.discover.retry").to_string(), DiscAct::LinksRetry),
                };
                put(frame, inner.x + 3, y, &super::bar::clip(&words, room.saturating_sub(2)), style);
                gui.ui.click(row, Act::Disc(act));
            }
        }
    }
    if lines.len() > shown {
        let bar = Rect { x: rect.right().saturating_sub(1), y: inner.y, width: 1, height: shown as u16 };
        scroll_list(
            frame,
            &mut gui.ui,
            bar,
            lines.len(),
            shown,
            first,
            Act::Disc(DiscAct::LinksScroll(-1)),
            Act::Disc(DiscAct::LinksScroll(1)),
            |i| Act::Disc(DiscAct::LinksJump(i)),
        );
    }
}

/// ↑ ↓ in the list: the next link (the hand picked up on the first).
pub(crate) fn links_walk(gui: &mut Gui, delta: i64) {
    let lines = link_lines(&gui.app, usize::from(LINKS_INNER - 2));
    let stops = link_stops(&lines);
    if stops.is_empty() {
        return;
    }
    let at = match links_held(gui, &lines) {
        None => 0,
        Some(at) => (at as i64 + delta).clamp(0, stops.len() as i64 - 1) as usize,
    };
    gui.discover.links.held = Some(at);
    gui.discover.links.view.pick_up();
}

/// Enter in the list: open the held link, or ask again after a failure;
/// with nothing held, the hand comes up on the first link.
pub(crate) fn links_enter(gui: &mut Gui) {
    let lines = link_lines(&gui.app, usize::from(LINKS_INNER - 2));
    let stops = link_stops(&lines);
    let Some(held) = links_held(gui, &lines) else {
        if !stops.is_empty() {
            gui.discover.links.held = Some(0);
            gui.discover.links.view.pick_up();
        }
        return;
    };
    match stops.get(held).and_then(|&i| lines.get(i)) {
        Some(LinkLine::Link(index, _)) => open_link(gui, *index),
        Some(LinkLine::Retry) => links_retry(gui),
        _ => {}
    }
}

/// Try again: the list asked again, for the same row — the one it was
/// first asked for, whatever answer has landed since.
pub(crate) fn links_retry(gui: &mut Gui) {
    let effects = gui.app.links_retry();
    gui.pend(effects);
    gui.discover.links.held = None;
}

/// Hand a link to the system's opener on a thread (clause 141): "opening
/// Deezer…" now, its answer on the status line once it lands.
pub(crate) fn open_link(gui: &mut Gui, index: usize) {
    let Some(link) = web_links(&gui.app).into_iter().nth(index) else { return };
    // Only https: the opener runs any scheme it is handed (clause 140).
    if !link.is_web() {
        return;
    }
    let room = note_room(gui);
    let words = ell(&t!("gui.discover.links.opening", label = link.label));
    set_note(gui, (fit_note(&words, None, room), MessageKind::Busy));
    let tx = gui.discover.links.tx.clone();
    std::thread::spawn(move || {
        let answer = open_target(&link.url);
        let _ = tx.send((link.label, link.url, answer));
    });
}

/// The opener's answers, worded (clause 141): "opened Deezer" (TEXT), or
/// "Deezer: no app took it" (GOLD). Under `MSTREAM_NO_OPEN` nothing opens,
/// and the note carries the link instead, the harness's seam.
fn links_landed(gui: &mut Gui) {
    let mut landed = Vec::new();
    while let Ok(answer) = gui.discover.links.rx.try_recv() {
        landed.push(answer);
    }
    let room = note_room(gui);
    for (label, url, answer) in landed {
        let note = match answer {
            Ok(HandOff::Launched) => (t!("gui.discover.links.opened", label = label).to_string(), MessageKind::Plain),
            Ok(HandOff::Headless(_)) => (format!("{label} · {url}"), MessageKind::Plain),
            Ok(HandOff::Nothing) | Err(_) => (t!("gui.discover.links.no_app", label = label).to_string(), MessageKind::Error),
        };
        set_note(gui, (fit_note(&note.0, None, room), note.1));
    }
}

/// The wheel over the list: its lines, while the pointer is on it.
pub(crate) fn links_wheel(gui: &mut Gui, delta: i32) -> bool {
    let over = gui.discover.links.at.zip(gui.ui.pointer).is_some_and(|(at, p)| at.contains(p));
    if over {
        gui.discover.links.view.step(delta);
    }
    over
}

/// The list's tips (clause 140): "↑ ↓ rows · Enter open · Esc back" over
/// links; Try again's after a failure; "Esc back" while it asks or has none.
pub(crate) fn links_tips(gui: &Gui) -> String {
    let lines = link_lines(&gui.app, usize::from(LINKS_INNER - 2));
    if lines.iter().any(|l| matches!(l, LinkLine::Link(..))) {
        t!("gui.tips.discover_links").to_string()
    } else if lines.contains(&LinkLine::Retry) {
        t!("gui.tips.discover_links_failed").to_string()
    } else {
        t!("gui.tips.discover_links_wait").to_string()
    }
}

/// The list's acts: a link, Try again, the scrollbar.
pub(crate) fn links_act(gui: &mut Gui, act: &DiscAct) {
    match act {
        // A click acts and stows the hand (the kit's list-cursor law).
        DiscAct::LinkOpen(index) => {
            gui.discover.links.held = None;
            open_link(gui, *index);
        }
        DiscAct::LinksRetry => links_retry(gui),
        DiscAct::LinksScroll(delta) => gui.discover.links.view.step(*delta),
        DiscAct::LinksJump(first) => gui.discover.links.view.scroll = *first,
        _ => {}
    }
}

// ── Status notes (clause 118) ───────────────────────────────────────────────

/// The note's cells at the window's width: the bar's note slot.
fn note_room(gui: &Gui) -> usize {
    usize::from(super::bar::note_width(gui.last_width.max(super::MIN_W)))
}

/// Words fitted to the note's cells: the head with its tail where the
/// whole fits, else the head — clipped with `…` (a trailing space trimmed
/// first) when even that overflows; the whole words stay in the modals.
pub(crate) fn fit_note(head: &str, wide: Option<&str>, room: usize) -> String {
    if let Some(wide) = wide
        && width(wide) <= room
    {
        return wide.to_string();
    }
    if width(head) <= room {
        return head.to_string();
    }
    let clipped = super::bar::clip(head, room).into_owned();
    let mark = if legacy_conhost() { '»' } else { '…' };
    match clipped.strip_suffix(mark) {
        Some(body) => format!("{}{mark}", body.trim_end()),
        None => clipped,
    }
}

/// What a job names: the song, the album, or the artist.
fn job_title(job: &PluginJob) -> String {
    let text = |k: &str| job.rec_text(k).unwrap_or_default();
    match job.scope() {
        Some(JobScope::Album) => text("album"),
        Some(JobScope::Artist | JobScope::ArtistMissing) => text("artist"),
        _ => job.rec_text("title").unwrap_or_else(|| text("album")),
    }
}

/// A start refused after its window closed: "couldn't start: {the server's
/// words}", as a start the Copies list sent says it — a 403's too: the gate
/// is asked again, and the room's ways in follow its answer (clause 147).
fn refused_note(failure: &crate::api::PluginFailure, room: usize) -> (String, MessageKind) {
    let words = t!("gui.discover.copies.refused_start", words = failure_words(failure)).to_string();
    (fit_note(&words, None, room), MessageKind::Error)
}

/// "copying Borrowed Tapes…" — a copy still moving.
fn busy_note(job: &PluginJob, room: usize) -> (String, MessageKind) {
    let title = job_title(job);
    let head = ell(&t!("gui.discover.note.copying", title = title));
    let wide = job.peer_name().map(|server| ell(&t!("gui.discover.note.copying_from", title = title, server = server)));
    (fit_note(&head, wide.as_deref(), room), MessageKind::Busy)
}

/// The note a copy that ended leaves (clause 118), `None` for none.
pub(crate) fn news_note(job: &PluginJob, room: usize) -> Option<(String, MessageKind)> {
    let title = job_title(job);
    let server = job.peer_name().unwrap_or_default();
    let tail = |head: String| format!("{head} · {title}");
    let (head, wide, kind) = match job.outcome() {
        JobOutcome::Copied { vpath, filepath } => {
            let library = vpath.or_else(|| filepath.split('/').next().map(str::to_string)).filter(|l| !l.is_empty());
            let head = t!("gui.discover.note.copied", title = title).to_string();
            let wide = library.map(|library| t!("gui.discover.note.copied_into", title = title, library = library).to_string());
            (head, wide, MessageKind::Success)
        }
        JobOutcome::CopiedMany(n) if n.failed > 0 => {
            let head = t!("gui.discover.note.album_failed").to_string();
            (head.clone(), Some(tail(head)), MessageKind::Error)
        }
        JobOutcome::CopiedMany(_) => {
            let head = t!("gui.discover.note.album_done").to_string();
            (head.clone(), Some(tail(head)), MessageKind::Success)
        }
        JobOutcome::Owned { .. } => {
            let head = t!("gui.discover.note.yours").to_string();
            (head.clone(), Some(tail(head)), MessageKind::Info)
        }
        JobOutcome::Exists { .. } => {
            let head = t!("gui.discover.note.exists").to_string();
            (head.clone(), Some(tail(head)), MessageKind::Error)
        }
        JobOutcome::Nothing { appearances, .. } => {
            let head = t!("gui.discover.note.nothing").to_string();
            let wide = if appearances > 0 { tail(head.clone()) } else { format!("{head} · {}", t!("gui.discover.copies.all_yours")) };
            (head, Some(wide), MessageKind::Info)
        }
        JobOutcome::Failed { error, .. } => {
            let words = error.unwrap_or_else(|| t!("gui.discover.copies.failed_plain").to_string());
            (t!("gui.discover.note.failed", words = words).to_string(), None, MessageKind::Error)
        }
        JobOutcome::Stopped { why: StopReason::Other, .. } => {
            // A reason this client does not know: neutral words, the title
            // where it fits.
            let head = t!("gui.discover.note.stopped", why = t!("gui.discover.note.why_other")).to_string();
            (head.clone(), Some(tail(head)), MessageKind::Error)
        }
        JobOutcome::Stopped { why, .. } => {
            let (short, long) = match why {
                StopReason::Quota => ("gui.discover.note.why_quota", "gui.discover.note.why_quota_long"),
                StopReason::Busy => ("gui.discover.note.why_busy", "gui.discover.note.why_busy_long"),
                StopReason::Peer | StopReason::Refused | StopReason::Other => {
                    ("gui.discover.note.why_peer", "gui.discover.note.why_peer_long")
                }
            };
            let head = t!("gui.discover.note.stopped", why = t!(short)).to_string();
            let wide = t!("gui.discover.note.stopped", why = t!(long, server = server)).to_string();
            (head, Some(wide), MessageKind::Error)
        }
        JobOutcome::Refused { .. } => {
            (t!("gui.discover.note.refuses", server = server).to_string(), None, MessageKind::Error)
        }
        JobOutcome::Cancelled { .. } => {
            let head = t!("gui.discover.note.cancelled").to_string();
            (head.clone(), Some(tail(head)), MessageKind::Info)
        }
        JobOutcome::Queued | JobOutcome::Running { .. } | JobOutcome::Done | JobOutcome::Unknown => return None,
    };
    Some((fit_note(&ell(&head), wide.map(|w| ell(&w)).as_deref(), room), kind))
}

/// A copy note on the status line — the App's message, so the next note
/// replaces it as every App message does; the shell's own note gives way.
fn set_note(gui: &mut Gui, (text, kind): (String, MessageKind)) {
    gui.note = None;
    gui.app.message = Some(crate::tui::app::Message { text, kind });
}

/// Once a frame (clauses 112, 118, 144): the poll's beat from what shows,
/// a save's answer, and the copies that ended since — worded on the status
/// line, except the one its own open modal speaks for.
pub(crate) fn frame_tick(gui: &mut Gui) {
    use crate::tui::app::keep::KeepWatch;
    let room = gui.screen == super::Screen::Library && gui.active == super::DISCOVER_NAV;
    // The quick beat only while a job's own surface shows it: the Copies
    // modal, or a Keep a copy modal whose own copy moves — one still
    // choosing over another copy keeps the room's (clause 112).
    let modal = gui.discover.copies.is_some() || gui.app.keep_flow_live();
    let watch = match (room, modal) {
        (true, true) => KeepWatch::Modal,
        (true, false) => KeepWatch::Room,
        (false, _) => KeepWatch::Away,
    };
    gui.app.keep_watch(watch);
    // Where copies land's save that answered since (clause 144) — before
    // the picker draws, and said even once its picker is gone.
    dest_landed(gui);
    // Open elsewhere's hand-offs that answered since (clause 141).
    links_landed(gui);
    let room = note_room(gui);
    // A start whose window closed before it answered (Esc on `[ Adding…
    // ]`): the copy it started, or why it did not — the window that would
    // have said so is gone.
    for landed in gui.app.keep_take_landed() {
        let note = match landed {
            Ok(job) if job.live() => Some(busy_note(&job, room)),
            Ok(job) => news_note(&job, room),
            Err(failure) => Some(refused_note(&failure, room)),
        };
        if let Some(note) = note {
            set_note(gui, note);
        }
    }
    let news = gui.app.keep_take_news();
    if news.is_empty() {
        return;
    }
    let followed = gui.app.taste.keep.flow.as_ref().and_then(|f| match f.phase {
        KeepPhase::Following(id) => Some(id),
        _ => None,
    });
    for job in news.iter().filter(|j| Some(j.id) != followed) {
        if let Some(note) = news_note(job, room) {
            set_note(gui, note);
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Position;
    use serde_json::{Value, json};

    use super::super::{Act, DISCOVER_NAV, Gui, render};
    use crate::api::types::{Capabilities, JobLookup, PluginJob, PluginJobList, TasteResponse, Track, TrackMetadata};
    use crate::tui::app::TasteTab;
    use crate::config::Config;
    use crate::tui::app::{App, Effect};
    use crate::tui::worker::{ApiCmd, Event, TastePart};

    pub(crate) const HOST: &str = "http://host:3000";
    pub(crate) const B: i64 = 1;
    pub(crate) const C: i64 = 4;

    pub(crate) fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    fn caps() -> Capabilities {
        Capabilities {
            discovery: true,
            discovery_path: true,
            discovery_p2p: true,
            federation_discovery: true,
            discovery_taste: 1,
            discovery_plugins: true,
            uploads: true,
            ..Capabilities::default()
        }
    }

    pub(crate) fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }

    pub(crate) fn buffer(gui: &mut Gui, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        terminal.backend().buffer().clone()
    }

    pub(crate) fn lines(buffer: &Buffer) -> Vec<String> {
        let area = *buffer.area();
        (0..area.height).map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect()
    }

    pub(crate) fn draw(gui: &mut Gui, width: u16, height: u16) -> Vec<String> {
        lines(&buffer(gui, width, height))
    }

    /// Where `text` stands on screen, in cells.
    pub(crate) fn at(rows: &[String], text: &str) -> Position {
        for (y, row) in rows.iter().enumerate() {
            if let Some(i) = row.find(text) {
                let x = row[..i].chars().count();
                return Position { x: x as u16, y: y as u16 };
            }
        }
        panic!("{text:?} is not on screen:\n{}", rows.join("\n"));
    }

    fn name(peer: i64) -> &'static str {
        if peer == B { "Stage Peer B" } else { "Stage Peer C" }
    }

    /// A connected server's row as the taste route sends it (the canvas's
    /// sample, the stage's): with the keys the plug-in schema strips.
    pub(crate) fn fed(peer: i64, title: &str, artist: &str, album: &str, similarity: f64, duration: Option<f64>) -> Value {
        json!({
            "artist": artist, "title": title, "album": album, "year": 2019, "duration": duration,
            "similarity": similarity, "seedHash": null, "albumArt": null,
            "filepath": format!("peerlib/{artist}/{album}/{title}.mp3"),
            "peer": { "id": peer, "name": name(peer) }, "source": "federation",
        })
    }

    /// The canvas's Connected servers tab: Stage Peer B's Glass Hours,
    /// Northern Signal (Peer Sessions) and Late Train Verse, Corner Store
    /// Loop (Borrowed Tapes); Stage Peer C's Rooftop Cypher (Borrowed
    /// Tapes), Brass District (Free Instrumentals Vol. 9) and Static
    /// Lullaby (First Transmission).
    pub(crate) fn peers_answer() -> TasteResponse {
        serde_json::from_value(json!({
            "seedSource": "history", "seeds": [], "library": null, "network": null,
            "peers": {
                "peers": [
                    { "id": B, "name": name(B), "reachable": true, "results": [
                        fed(B, "Glass Hours", "Color Out", "Peer Sessions", 0.95, Some(10.0)),
                        fed(B, "Northern Signal", "Color Out", "Peer Sessions", 0.9, Some(9.0)),
                        fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0)),
                        fed(B, "Corner Store Loop", "Boukmanflow", "Borrowed Tapes", 0.8, Some(9.0)),
                    ] },
                    { "id": C, "name": name(C), "reachable": true, "results": [
                        fed(C, "Rooftop Cypher", "Boukmanflow", "Borrowed Tapes", 0.6, Some(12.0)),
                        fed(C, "Brass District", "ALM", "Free Instrumentals Vol. 9", 0.85, Some(10.0)),
                        fed(C, "Static Lullaby", "Night Owl Radio", "First Transmission", 0.7, None),
                    ] }
                ],
                "searched": { "peers": 2, "unreachable": 0, "mismatched": 0 }
            }
        }))
        .unwrap()
    }

    fn main_answer() -> TasteResponse {
        serde_json::from_value(json!({
            "window": 30, "seedSource": "history", "fallback": false,
            "seeds": [ { "hash": "h1", "filepath": "music/host.mp3", "metadata": { "title": "Host", "artist": "Color Out" } } ],
            "library": { "filter": "fresh", "albums": [], "tracks": [
                { "filepath": "music/circles.mp3", "similarity": 0.8, "metadata": { "title": "Circles", "artist": "Color Out", "duration": 200 } }
            ] },
            "network": null
        }))
        .unwrap()
    }

    pub(crate) fn listing(allowed: bool) -> Value {
        json!({
            "plugins": [
                { "name": "links", "title": "Open elsewhere", "capabilities": ["links"], "scopes": ["song"] },
                { "name": "federation-copy", "capabilities": ["acquire"], "scopes": ["song", "album", "artist", "artist-missing", "folder"] }
            ],
            "jobs": { "allowed": allowed }
        })
    }

    pub(crate) fn destination() -> Value {
        json!({
            "destination": { "vpath": "copies", "base": "", "layout": "{{ALBUMARTIST}}/{{ALBUM}}", "source": "default" },
            "libraries": [{ "vpath": "copies" }], "defaultLayout": "{{ALBUMARTIST}}/{{ALBUM}}", "variables": ["ARTIST", "ALBUM"]
        })
    }

    fn fold(s: &str) -> String {
        s.trim().to_lowercase()
    }

    /// A row's keys, as the server builds them (enough for a test).
    pub(crate) fn keys_of(rec: &Value) -> (String, String, String) {
        let t = |k: &str| rec.get(k).and_then(Value::as_str).map(fold).unwrap_or_default();
        (
            format!("text:{}|{}|{}", t("artist"), t("title"), t("album")),
            format!("album:{}|{}", t("artist"), t("album")),
            format!("artist:{}", t("artist")),
        )
    }

    /// A copy job of the sample, pinned to the stage's destination as the
    /// server pins every job it starts (clause 145).
    pub(crate) fn job(id: i64, rec: &Value, scope: Option<&str>, state: &str, extra: Value) -> PluginJob {
        let (song, album, artist) = keys_of(rec);
        let key = match scope {
            Some("album") => album,
            Some("artist" | "artist-missing") => artist,
            _ => song,
        };
        let mut params = json!({ "destination": destination()["destination"] });
        if let Some(scope) = scope {
            params["scope"] = json!(scope);
        }
        let mut job = json!({
            "id": id, "plugin": "federation-copy", "key": key, "state": state,
            "params": params,
            "recommendation": rec,
        });
        if let (Some(map), Some(more)) = (job.as_object_mut(), extra.as_object()) {
            for (k, v) in more {
                map.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(job).unwrap()
    }

    /// The canvas's copies: Borrowed Tapes from Stage Peer B copying (song
    /// 2 of 3), Peer Sessions kept, Northern Signal kept, Brass District
    /// waiting, First Transmission failed.
    pub(crate) fn canvas_jobs() -> Vec<PluginJob> {
        let late = fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0));
        let glass = fed(B, "Glass Hours", "Color Out", "Peer Sessions", 0.95, Some(10.0));
        let north = fed(B, "Northern Signal", "Color Out", "Peer Sessions", 0.9, Some(9.0));
        let brass = fed(C, "Brass District", "ALM", "Free Instrumentals Vol. 9", 0.85, Some(10.0));
        let lullaby = fed(C, "Static Lullaby", "Night Owl Radio", "First Transmission", 0.7, None);
        vec![
            job(31, &late, Some("album"), "running", json!({ "progress": 0.52, "statusText": "song 2 of 3 · Late Train Verse",
                "result": { "scope": "album", "counts": { "total": 3, "copied": 1, "skipped": 0, "failed": 0 } } })),
            job(22, &glass, Some("album"), "done", json!({ "result": { "scope": "album", "counts": { "total": 3, "copied": 3, "skipped": 0, "failed": 0 }, "stopped": null } })),
            job(21, &north, None, "done", json!({ "result": { "copied": { "vpath": "copies", "filepath": "copies/Color Out/Peer Sessions/Northern Signal.mp3" } } })),
            job(33, &brass, None, "queued", json!({})),
            job(29, &lullaby, Some("album"), "failed", json!({ "error": "Stage Peer C is unreachable" })),
        ]
    }

    /// The session, the room's gate and the copy gate, the queue folded.
    pub(crate) fn plugin_gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(Some(HOST.into()), None, None));
        gui.app.connected = true;
        gui.app.session.server = HOST.into();
        gui.app.session.server_id = HOST.into();
        gui.app.capabilities = caps();
        gui.queue_open = false;
        gui
    }

    fn pending_taste(gui: &Gui, part: TastePart) -> (u64, u64) {
        gui.pending
            .iter()
            .rev()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::Taste { seq, generation, part: p, .. }) if *p == part => Some((*seq, *generation)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {part:?} ask in {:?}", gui.pending))
    }

    fn apply(gui: &mut Gui, event: Event) {
        let effects = gui.app.apply_event(event);
        gui.pend(effects);
    }

    /// Answer the last rows lookup out with `jobs`, by key.
    pub(crate) fn land_lookups(gui: &mut Gui, jobs: &[PluginJob]) {
        let asks: Vec<(u64, u64, Vec<Value>)> = gui
            .pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::JobsLookup { seq, generation, recs }) => Some((*seq, *generation, recs.clone())),
                _ => None,
            })
            .collect();
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::JobsLookup { .. })));
        for (seq, generation, recs) in asks {
            let answer: Vec<JobLookup> = recs
                .iter()
                .map(|rec| {
                    let (song, album, artist) = keys_of(rec);
                    let mut found: Vec<PluginJob> =
                        jobs.iter().filter(|j| [&song, &album, &artist].iter().any(|k| j.key.as_deref() == Some(k.as_str()))).cloned().collect();
                    found.sort_by_key(|j| std::cmp::Reverse(j.id));
                    let keys = [("song", &song), ("album", &album), ("artist", &artist), ("artist-missing", &artist)]
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), json!(v)))
                        .chain([("folder".to_string(), json!("folder:x"))])
                        .collect();
                    JobLookup { key: Some(song.clone()), keys, jobs: found }
                })
                .collect();
            apply(gui, Event::JobsLookup { seq, generation, answer: Ok(answer) });
        }
    }

    /// Answer the jobs list ask out with `jobs`.
    pub(crate) fn land_jobs(gui: &mut Gui, jobs: &[PluginJob]) {
        let asks: Vec<(u64, u64)> = gui
            .pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::Jobs { seq, generation }) => Some((*seq, *generation)),
                _ => None,
            })
            .collect();
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::Jobs { .. })));
        for (seq, generation) in asks {
            let list = PluginJobList { jobs: jobs.to_vec(), runner: None };
            apply(gui, Event::Jobs { seq, generation, answer: Ok(Box::new(list)) });
        }
    }

    /// The room on its Connected servers tab with the canvas's sample: the
    /// gate open, the rows looked up, the list answered; Glass Hours plays.
    pub(crate) fn canvas_gui(jobs: &[PluginJob]) -> Gui {
        let mut gui = plugin_gui();
        key(&mut gui, KeyCode::Char('G'));
        assert_eq!(gui.active, DISCOVER_NAV);
        let (seq, generation) = pending_taste(&gui, TastePart::Main);
        apply(&mut gui, Event::Taste { seq, generation, part: TastePart::Main, answer: Ok(Box::new(main_answer())) });
        let (seq, generation) = pending_taste(&gui, TastePart::Peers);
        apply(&mut gui, Event::Taste { seq, generation, part: TastePart::Peers, answer: Ok(Box::new(peers_answer())) });
        let generation = gui.app.session_gen();
        apply(
            &mut gui,
            Event::KeepGate {
                generation,
                plugins: Ok(Box::new(serde_json::from_value(listing(true)).unwrap())),
                destination: Some(Ok(Box::new(serde_json::from_value(destination()).unwrap()))),
            },
        );
        land_lookups(&mut gui, jobs);
        land_jobs(&mut gui, jobs);
        gui.act(Act::Disc(super::super::discover::DiscAct::Tab(TasteTab::Servers)));
        gui.app.now_playing = Some(Track {
            filepath: "peerlib/Color Out/Peer Sessions/Glass Hours.mp3".into(),
            metadata: TrackMetadata { title: Some("Glass Hours".into()), artist: Some("Color Out".into()), duration: Some(10.0), ..Default::default() },
        });
        gui.pending.clear();
        gui
    }

    /// The Keep modal's sizes, landed: the album's songs on Stage Peer B,
    /// the artist's albums there and in the library.
    pub(crate) fn land_facts(gui: &mut Gui, songs: Option<usize>, remote: &[&str], local: &[&str]) {
        let ask = gui.pending.iter().rev().find_map(|e| match e {
            Effect::Api(ApiCmd::KeepFacts { seq, generation, .. }) => Some((*seq, *generation)),
            _ => None,
        });
        let Some((seq, generation)) = ask else { panic!("no facts ask in {:?}", gui.pending) };
        let rows = |names: &[&str], artist: &str| -> Vec<crate::api::types::ArtistAlbumRow> {
            names
                .iter()
                .map(|n| serde_json::from_value(json!({ "name": n, "album_artist": artist, "artists": [artist] })).unwrap())
                .collect()
        };
        // The album's songs tagged as the stage's row is: Boukmanflow, 2019.
        let song = |i: usize| crate::tui::worker::KeepSong {
            filepath: format!("peerlib/Boukmanflow/Borrowed Tapes/{i:02}.mp3"),
            artist: Some("Boukmanflow".into()),
            year: Some(2019),
            genre: None,
        };
        let facts = crate::tui::worker::KeepFacts {
            album_songs: songs.map(|n| (1..=n).map(song).collect()),
            peer_albums: Some(rows(remote, "Boukmanflow")),
            local_albums: Some(rows(local, "Boukmanflow")),
        };
        apply(gui, Event::KeepFacts { seq, generation, facts: Box::new(facts) });
    }

    /// The stage's facts: Borrowed Tapes' three songs on Stage Peer B, the
    /// library's three other Boukmanflow albums.
    pub(crate) fn stage_facts(gui: &mut Gui) {
        land_facts(gui, Some(3), &["Borrowed Tapes"], &["Kite Days", "Night Bus", "Basement Tapes"]);
    }

    /// Press the primary and answer the start with `job` (or a refusal).
    pub(crate) fn start(gui: &mut Gui, answer: Result<PluginJob, crate::api::PluginFailure>) {
        gui.act(Act::Disc(super::super::discover::DiscAct::Keep(super::KeepAct::Primary)));
        let ticket = gui
            .pending
            .iter()
            .rev()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::JobStart { ticket, .. }) => Some(*ticket),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no start in {:?}", gui.pending));
        let generation = gui.app.session_gen();
        let answer = answer.map(|job| Box::new(crate::api::types::JobStarted { job, created: true }));
        apply(gui, Event::JobStarted { ticket, generation, answer });
    }

    /// The poll's answer with `jobs`.
    pub(crate) fn poll(gui: &mut Gui, jobs: &[PluginJob]) {
        let effects = gui.app.copies_try_again();
        gui.pend(effects);
        land_jobs(gui, jobs);
    }

    /// The window's own rows, 76 wide, from its top border (its title on
    /// it) to its bottom border (its way out on it). A row holds one
    /// character per cell, a wide character's second cell blank — so the
    /// title is looked for that way too.
    pub(crate) fn modal_rows(rows: &[String]) -> Vec<String> {
        let title: String = rust_i18n::t!("gui.discover.keep.title")
            .chars()
            .flat_map(|c| if crate::kit::width(&c.to_string()) == 2 { vec![c, ' '] } else { vec![c] })
            .collect();
        let top = rows
            .iter()
            .position(|r| r.contains(&format!("─ {title} ─")) || r.contains(&format!("- {title} -")))
            .expect("the window");
        let x = rows[top].chars().position(|c| c == '╭' || c == '+').expect("its corner");
        let bottom = (top + 1..rows.len()).find(|&y| matches!(rows[y].chars().nth(x), Some('╰' | '+'))).unwrap_or(rows.len() - 1);
        rows[top..=bottom].iter().map(|r| r.chars().skip(x).take(76).collect()).collect()
    }

    fn late() -> Value {
        fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0))
    }

    /// The boards' facts (SPEC §5): Borrowed Tapes' three songs on Stage
    /// Peer B, which holds Second Shift too; the library's three Boukmanflow
    /// albums, Second Shift among them — so Borrowed Tapes is the one album
    /// missing by name.
    fn board_facts(gui: &mut Gui) {
        land_facts(gui, Some(3), &["Borrowed Tapes", "Second Shift"], &["Kite Days", "Night Bus", "Second Shift"]);
    }

    /// Keep a copy open on `target` — Late Train Verse's row, Borrowed
    /// Tapes' tile — with the boards' facts.
    fn opened(target: KeepTarget) -> Gui {
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, target);
        board_facts(&mut gui);
        gui
    }

    /// The choice of `scope`, by the pointer.
    fn choose(gui: &mut Gui, scope: Option<&str>) {
        use crate::tui::app::keep::KeepChoice;
        let choice = match scope {
            Some("album") => KeepChoice::Album,
            Some("artist" | "artist-missing") => KeepChoice::Artist,
            _ => KeepChoice::Song,
        };
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Pick(choice))));
        // The box keeps its own tick (ticked from the start): the artist
        // scopes set it, where it is offered; the others leave it be.
        let want = match scope {
            Some("artist") => Some(false),
            Some("artist-missing") => Some(true),
            _ => None,
        };
        if let Some(want) = want
            && gui.app.keep_view().and_then(|v| v.missing).is_some_and(|(on, _)| on != want)
        {
            gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Missing)));
        }
    }

    /// The window on `target` with a copy of `scope` started, the server
    /// answering it with a job in `state`, then polled through `more`.
    fn copying(target: KeepTarget, scope: Option<&str>, state: &str, extra: Value, more: &[(&str, Value)]) -> Gui {
        let mut gui = opened(target);
        choose(&mut gui, scope);
        start(&mut gui, Ok(job(91, &late(), scope, state, extra)));
        for (state, extra) in more {
            poll(&mut gui, &[job(91, &late(), scope, state, extra.clone())]);
        }
        gui
    }

    const TILE: KeepTarget = KeepTarget::Tile(0);

    /// An album copy's result as federation-copy sends it: its counts out of
    /// three, the songs it copied (their landed files in copies) and found
    /// already in music, its stop word.
    fn album_result(copied: u64, owned: u64, failed: u64, stopped: Option<&str>) -> Value {
        let from = |i: u64| format!("peerlib/Boukmanflow/Borrowed Tapes/{i:02}.mp3");
        let copied_songs: Vec<Value> = (1..=copied)
            .map(|i| json!({ "from": from(i), "vpath": "copies", "filepath": format!("copies/Boukmanflow/Borrowed Tapes/{i:02}.mp3"), "title": format!("Song {i}") }))
            .collect();
        let owned_songs: Vec<Value> = (copied + 1..=copied + owned)
            .map(|i| json!({ "from": from(i), "why": "owned", "at": format!("music/Boukmanflow/Borrowed Tapes/{i:02}.mp3") }))
            .collect();
        json!({ "result": { "scope": "album", "songs": { "total": 3, "copied": copied_songs, "skipped": owned_songs, "failed": [] },
            "counts": { "total": 3, "copied": copied, "skipped": owned, "failed": failed }, "stopped": stopped } })
    }

    /// Late Train Verse copied, at its landed file.
    fn song_copied() -> Value {
        json!({ "result": { "copied": { "vpath": "copies", "filepath": "copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3" } } })
    }

    /// Stage Peer B's tile with an artist copy that walked its albums
    /// (`albums`, `left` out) — the boards' E17.
    fn walked(albums: Value, left: Value) -> Gui {
        let late = late();
        let songs: Vec<Value> = (1..=3)
            .map(|i| json!({ "from": format!("peerlib/Boukmanflow/Borrowed Tapes/{i:02}.mp3"), "vpath": "copies",
                "filepath": format!("copies/Boukmanflow/Borrowed Tapes/{i:02}.mp3"), "title": format!("Song {i}") }))
            .collect();
        let songs = json!({ "total": 3, "skipped": [], "failed": [], "copied": songs });
        let albums = albums.as_array().map(|a| a.iter().map(|album| { let mut album = album.clone(); album["songs"] = songs.clone(); album }).collect::<Vec<_>>());
        let walk = job(80, &late, Some("artist-missing"), "done", json!({ "result": { "scope": "artist-missing",
            "albums": albums, "skippedAlbums": left, "songs": songs, "counts": { "total": 3, "copied": 3, "skipped": 0, "failed": 0 },
            "stopped": null, "peer": { "id": B, "name": "Stage Peer B" } } }));
        let mut gui = canvas_gui(&[walk]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, TILE);
        board_facts(&mut gui);
        gui
    }

    /// A place saved for the account: `base` under copies, `layout`.
    fn place_view(base: &str, layout: &str) -> Value {
        let mut view = dest_view(base, "user");
        view["destination"]["layout"] = json!(layout);
        view["saved"]["layout"] = json!(layout);
        view
    }

    /// The window in one of the boards' states (SPEC §5, RecStates), by its
    /// id — what the dump draws and the tests read.
    fn keep_in(id: &str) -> Gui {
        let late = late();
        let quota = album_result(1, 0, 0, Some("quota"));
        let running = json!({ "progress": 0.52, "statusText": "song 2 of 3 · Late Train Verse",
            "result": { "scope": "album", "counts": { "total": 3, "copied": 1, "skipped": 0, "failed": 0 } } });
        let failed = json!({ "error": "Stage Peer B is unreachable" });
        match id {
            "C1b" => opened(TILE),
            "C2" => {
                let mut gui = canvas_gui(&[]);
                draw(&mut gui, 100, 30);
                super::open(&mut gui, TILE);
                gui
            }
            "C3" => {
                let mut gui = opened(TILE);
                choose(&mut gui, Some("artist"));
                gui
            }
            "C4" => {
                let mut gui = opened(TILE);
                choose(&mut gui, Some("artist-missing"));
                gui
            }
            "C5" => opened(LATE),
            "C6" => {
                let (mut gui, _) = mixtape_gui();
                draw(&mut gui, 100, 30);
                super::open(&mut gui, TILE);
                land_facts(&mut gui, Some(2), &[], &[]);
                gui
            }
            "C7" => {
                let (mut gui, _) = mixtape_gui();
                draw(&mut gui, 100, 30);
                super::open(&mut gui, KeepTarget::Row { group: 0, row: 0 });
                land_read(&mut gui, layout_view("From friends/{{ARTIST}}"));
                land_songs(&mut gui, &[("Kim Song", "Peer Kim"), ("Lou Song", "Peer Lou")]);
                choose(&mut gui, Some("album"));
                gui
            }
            "C8" | "C8b" => {
                let base = if id == "C8" { "from-peers/2025 archive" } else { "from-peers/2025 archive/the long weekend sessions" };
                let mut gui = opened(TILE);
                land_read(&mut gui, place_view(base, "{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}"));
                gui
            }
            "C9" => {
                let mut gui = opened(TILE);
                key(&mut gui, KeyCode::Enter);
                gui
            }
            "R1" => {
                let mut gui = opened(LATE);
                let words = "you have 20 jobs queued or running — wait for some to finish (the limit is 20)";
                start(&mut gui, Err(crate::api::PluginFailure::Refused { status: 429, words: words.into() }));
                gui
            }
            "R2" => {
                let mut gui = opened(TILE);
                let music: crate::api::types::CollectionDestination =
                    serde_json::from_value(json!({ "vpath": "music", "base": "", "layout": "{{ARTIST}}/{{YEAR}} - {{ALBUM}}", "source": "default" })).unwrap();
                start(&mut gui, Err(crate::api::PluginFailure::DestinationChanged(Some(Some(music)))));
                gui
            }
            "R3" => {
                let mut gui = opened(TILE);
                land_read(&mut gui, json!({ "destination": null, "libraries": [] }));
                gui
            }
            "R3b" | "R4" => {
                let words = if id == "R3b" { "Uploading Disabled" } else { "Not allowed for your account" };
                let mut gui = opened(TILE);
                start(&mut gui, Err(crate::api::PluginFailure::Refused { status: 403, words: words.into() }));
                gui
            }
            "R5" => {
                let mut gui = opened(TILE);
                let mut gone = listing(true);
                gone["plugins"].as_array_mut().expect("plugins").retain(|p| p["name"] != "federation-copy");
                let generation = gui.app.session_gen();
                apply(&mut gui, Event::KeepGate { generation, plugins: Ok(Box::new(serde_json::from_value(gone).unwrap())), destination: None });
                gui
            }
            "O1" | "O1d" | "O1b" => {
                let rooftop = fed(C, "Rooftop Cypher", "Boukmanflow", "Borrowed Tapes", 0.6, Some(12.0));
                let scope = if id == "O1b" { "artist" } else { "album" };
                let theirs = job(40, &rooftop, Some(scope), "running", json!({ "progress": 0.3 }));
                let mut gui = canvas_gui(&[theirs]);
                draw(&mut gui, 100, 30);
                super::open(&mut gui, TILE);
                board_facts(&mut gui);
                if id == "O1b" {
                    choose(&mut gui, Some("artist"));
                }
                if id == "O1d" {
                    key(&mut gui, KeyCode::Char('x'));
                }
                gui
            }
            "L1" => copying(TILE, Some("album"), "queued", json!({}), &[]),
            "L2" => copying(TILE, Some("album"), "queued", json!({}), &[("running", running)]),
            "L3" => copying(TILE, Some("artist"), "running", json!({ "progress": 0.38, "statusText": "album 1 of 2 · Borrowed Tapes · song 2 of 3",
                "result": { "scope": "artist", "counts": { "total": 6, "copied": 1, "skipped": 0, "failed": 0 } } }), &[]),
            "L4" => copying(LATE, None, "running", json!({}), &[]),
            "L5" => {
                let mut gui = copying(TILE, Some("album"), "running", running, &[]);
                key(&mut gui, KeyCode::Char('x'));
                gui
            }
            "E1" => copying(LATE, None, "done", song_copied(), &[]),
            "E2" => {
                let rooftop_late = fed(C, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.7, Some(8.0));
                let mut gui = canvas_gui(&[job(95, &rooftop_late, None, "done", song_copied())]);
                draw(&mut gui, 100, 30);
                super::open(&mut gui, LATE);
                board_facts(&mut gui);
                gui
            }
            "E3" => copying(TILE, Some("album"), "done", album_result(2, 1, 0, None), &[]),
            "E4" => {
                let songs: Vec<Value> = ["Borrowed Tapes", "Second Shift", "Second Shift"].iter().enumerate()
                    .map(|(i, album)| json!({ "from": format!("peerlib/Boukmanflow/{album}/{i:02}.mp3"), "vpath": "copies",
                        "filepath": format!("copies/Boukmanflow/{album}/{i:02}.mp3") }))
                    .collect();
                copying(TILE, Some("artist"), "done", json!({ "result": { "scope": "artist", "albums": [{ "name": "Borrowed Tapes" }, { "name": "Second Shift" }],
                    "songs": { "total": 6, "copied": songs, "skipped": [], "failed": [] },
                    "counts": { "total": 6, "copied": 3, "skipped": 3, "failed": 0 }, "stopped": null } }), &[])
            }
            "E5" => copying(TILE, Some("album"), "done", album_result(2, 0, 1, None), &[]),
            "E5b" => {
                let songs: Vec<Value> = (1..=3)
                    .map(|i| json!({ "from": format!("peerlib/Boukmanflow/Borrowed Tapes/{i:02}.mp3"), "vpath": "copies",
                        "filepath": format!("copies/Boukmanflow/Borrowed Tapes/{i:02}.mp3") }))
                    .collect();
                copying(TILE, Some("artist"), "done", json!({ "result": { "scope": "artist",
                    "albums": [{ "name": "Borrowed Tapes" }, { "name": "Second Shift", "error": "listing timed out" }],
                    "songs": { "total": 3, "copied": songs, "skipped": [], "failed": [] },
                    "counts": { "total": 3, "copied": 3, "skipped": 0, "failed": 0 }, "stopped": null } }), &[])
            }
            "E6" => copying(LATE, None, "done", json!({ "result": { "skipped": "owned",
                "existing": { "filepath": "music/Boukmanflow/Borrowed Tapes/02 Late Train Verse.mp3" } } }), &[]),
            "E7" | "E8" => {
                let mut gui = copying(LATE, None, "done", json!({ "result": { "skipped": "exists",
                    "filepath": "copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3" } }), &[]);
                if id == "E8" {
                    land_read(&mut gui, dest_view("from-peers", "user"));
                }
                gui
            }
            "E9" | "E9b" | "E20" => {
                let mut gui = copying(TILE, Some("album"), "failed", failed, &[]);
                if id == "E9b" {
                    land_read(&mut gui, dest_view("from-peers", "user"));
                }
                if id == "E20" {
                    key(&mut gui, KeyCode::Enter);
                }
                gui
            }
            "E10" => copying(TILE, Some("album"), "done", album_result(0, 0, 3, None), &[]),
            "E11" => copying(TILE, Some("album"), "done", quota, &[]),
            "E11b" => copying(TILE, Some("album"), "done", album_result(1, 0, 0, Some("busy")), &[]),
            "E11c" => copying(TILE, Some("album"), "done", album_result(1, 0, 0, Some("maintenance")), &[]),
            "E12" => copying(TILE, Some("album"), "done", album_result(0, 0, 0, Some("refused")), &[]),
            "E12b" => copying(TILE, Some("album"), "done", album_result(1, 0, 0, Some("refused")), &[]),
            "E13" => copying(TILE, Some("album"), "cancelled", album_result(1, 0, 0, None), &[]),
            "E14" => copying(TILE, Some("album"), "cancelled", album_result(0, 0, 0, None), &[]),
            "E15" => copying(TILE, Some("album"), "done", json!({ "result": { "scope": "album",
                "counts": { "total": 0, "copied": 0, "skipped": 0, "failed": 0 } } }), &[]),
            "E16" => copying(TILE, Some("artist"), "done", json!({ "result": { "scope": "artist", "albums": [],
                "skippedAlbums": [{ "name": "Comp", "why": "appearance" }], "counts": { "total": 0, "copied": 0, "skipped": 0, "failed": 0 }, "stopped": null } }), &[]),
            "E17" => walked(json!([{ "name": "Borrowed Tapes", "stopped": null }]), json!([])),
            "E17b" => walked(json!([]), json!([{ "name": "Borrowed Tapes", "why": "owned" }])),
            "E18" => {
                let songs = json!({ "total": 3, "skipped": [], "failed": [], "copied": [{ "from": late["filepath"], "vpath": "copies",
                    "filepath": "copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3", "title": "Late Train Verse" }] });
                let album = job(81, &late, Some("album"), "done", json!({ "result": { "scope": "album", "songs": songs,
                    "counts": { "total": 3, "copied": 3, "skipped": 0, "failed": 0 }, "stopped": null, "peer": { "id": B, "name": "Stage Peer B" } } }));
                let mut gui = canvas_gui(&[album]);
                draw(&mut gui, 100, 30);
                super::open(&mut gui, LATE);
                board_facts(&mut gui);
                gui
            }
            "E19" => {
                let mut gui = copying(TILE, Some("album"), "done", album_result(1, 0, 0, Some("peer")), &[]);
                land_read(&mut gui, dest_view("from-peers", "user"));
                gui
            }
            "E19b" => {
                let mut gui = copying(TILE, Some("album"), "done", album_result(2, 0, 1, None), &[]);
                land_read(&mut gui, place_view("", "{{GENRE}}/{{ALBUMARTIST}}/{{YEAR}} - {{ALBUM}}"));
                gui
            }
            "E21" => copying(TILE, Some("album"), "done", json!({ "result": {} }), &[]),
            "T1" => {
                let mut gui = opened(LATE);
                land_read(&mut gui, place_view("from-peers/the long weekend sessions", "{{PEER}}/{{GENRE}}/{{ALBUMARTIST}}/{{YEAR}} - {{ALBUM}}"));
                choose(&mut gui, Some("album"));
                let busy = "someone else on this server is already getting this — try again in a moment";
                start(&mut gui, Err(crate::api::PluginFailure::Refused { status: 409, words: busy.into() }));
                gui
            }
            other => panic!("no state {other}"),
        }
    }

    /// Every state the boards draw, in their order.
    const STATES: &[&str] = &[
        "C1b", "C2", "C3", "C4", "C5", "C6", "C7", "C8", "C8b", "C9", "R1", "R2", "R3", "R3b", "R4", "R5", "O1", "O1b", "O1d", "L1",
        "L2", "L3", "L4", "L5", "E1", "E2", "E3", "E4", "E5", "E5b", "E6", "E7", "E8", "E9", "E9b", "E10", "E11", "E11b", "E11c",
        "E12", "E12b", "E13", "E14", "E15", "E16", "E17", "E17b", "E18", "E19", "E19b", "E20", "E21", "T1",
    ];

    /// `cargo test dump_keep_states -- --ignored --nocapture`: every state
    /// at 100 × 30 — `KEEP_DUMP_ROWS=24` for the floor, `KEEP_DUMP_HINTS=1`
    /// with the tips line on, `KEEP_DUMP_LOCALE=de` in another language —
    /// then the room around two of them, and the legacy console's.
    #[test]
    #[ignore]
    fn dump_keep_states() {
        let locale = std::env::var("KEEP_DUMP_LOCALE").unwrap_or_else(|_| "en".to_string());
        rust_i18n::set_locale(&locale);
        crate::kit::theme::pin_modern_terminal();
        let height = std::env::var("KEEP_DUMP_ROWS").ok().and_then(|h| h.parse().ok()).unwrap_or(30);
        let hints = std::env::var("KEEP_DUMP_HINTS").is_ok_and(|h| h == "1");
        let show = |id: &str, gui: &mut Gui, hints: bool| {
            gui.config.gui.key_hints = hints;
            gui.ui.key_hints = hints;
            let rows = draw(gui, 100, height);
            let window = modal_rows(&rows);
            let top = rows.iter().position(|r| r.contains(&window[0])).unwrap_or(0);
            println!("── {id} · {} rows · rows {top}–{} · tips: {}", window.len(), top + window.len() - 1, super::super::discover::tips(gui));
            println!("{}", window.join("\n"));
        };
        let mut seen = std::collections::BTreeSet::new();
        for &id in STATES {
            if !seen.insert(id) {
                continue;
            }
            show(id, &mut keep_in(id), hints);
        }
        show("T1 · key hints on", &mut keep_in("T1"), true);
        println!("── the room, album tile idle, 100 × {height}");
        println!("{}", draw(&mut keep_in("C1b"), 100, height).join("\n"));
        println!("── the room, copying, 100 × {height}");
        println!("{}", draw(&mut keep_in("L2"), 100, height).join("\n"));
        let mut gui = keep_in("C1b");
        gui.queue_open = true;
        println!("── the queue open, 100 × {height}");
        println!("{}", draw(&mut gui, 100, height).join("\n"));
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        for id in ["C1b", "C9", "L1", "L2", "L5", "E1"] {
            show(&format!("{id} · legacy console"), &mut keep_in(id), hints);
        }
    }

    /// `cargo test dump_keep -- --ignored --nocapture` to eyeball the room,
    /// the sheet, the tile's list and the modal against the canvas.
    #[test]
    #[ignore]
    fn dump_keep() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        // The held copying row (↓ ↓ picks the third row up).
        println!("{}", draw(&mut gui, 100, 30).join("\n"));
        for _ in 0..3 {
            key(&mut gui, KeyCode::Down);
        }
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        println!("{}", draw(&mut gui, 100, 30).join("\n"));
        gui.queue_open = true;
        println!("{}", draw(&mut gui, 100, 30).join("\n"));
        gui.queue_open = false;
        key(&mut gui, KeyCode::Char('m'));
        println!("{}", draw(&mut gui, 100, 30).join("\n"));
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Char('c'));
        println!("{}", draw(&mut gui, 100, 30).join("\n"));
        key(&mut gui, KeyCode::Esc);
        gui.act(Act::Disc(super::super::discover::DiscAct::TileList(0)));
        println!("{}", draw(&mut gui, 100, 30).join("\n"));
        gui.discover.tile_list = None;
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        println!("{}", draw(&mut gui, 100, 24).join("\n"));
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        super::open(&mut gui, crate::tui::app::keep::KeepTarget::Row { group: 0, row: 2 });
        stage_facts(&mut gui);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char(' '));
        println!("{}", draw(&mut gui, 100, 30).join("\n"));
    }

    // ── Helpers for the assertions ──────────────────────────────────────

    use ratatui::style::{Color, Modifier};

    use super::super::discover::DiscAct;
    use crate::kit::theme::th;
    use crate::tui::app::keep::KeepTarget;

    fn fg(buffer: &Buffer, at: Position) -> Color {
        buffer[(at.x, at.y)].fg
    }

    fn bg(buffer: &Buffer, at: Position) -> Color {
        buffer[(at.x, at.y)].bg
    }

    fn bold(buffer: &Buffer, at: Position) -> bool {
        buffer[(at.x, at.y)].modifier.contains(Modifier::BOLD)
    }

    /// The room's row `y` from its first cell (the content column, x 17).
    fn room(rows: &[String], y: usize) -> String {
        rows[y].chars().skip(17).collect::<String>().trim_end().to_string()
    }

    const LATE: KeepTarget = KeepTarget::Row { group: 0, row: 2 };

    fn starts(gui: &Gui) -> Vec<(Value, crate::api::types::JobScope)> {
        gui.pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::JobStart { rec, scope, .. }) => Some(((**rec).clone(), *scope)),
                _ => None,
            })
            .collect()
    }

    // ── The room's marks (clauses 113–114) ──────────────────────────────

    #[test]
    fn a_rows_lead_and_a_tiles_artist_row_wear_the_copys_state_in_its_token() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let all = rows.join("\n");
        // The LEAD, row by row (the canvas's TermRoom a): the playing ▸ wins
        // over its kept album; Stage Peer C's Rooftop Cypher wears none of
        // Stage Peer B's album copy.
        for (lead, title, token) in [
            ("▸", "Glass Hours", th().ok),
            ("✓", "Northern Signal", th().ok),
            ("↓", "Late Train Verse", th().accent),
            ("↓", "Corner Store Loop", th().accent),
            ("◷", "Brass District", th().dim),
            ("!", "Static Lullaby", th().gold),
        ] {
            let at = at(&rows, title);
            let mark = Position { x: at.x - 2, y: at.y };
            assert_eq!(buffer[(mark.x, mark.y)].symbol(), lead, "{title}'s LEAD:\n{all}");
            assert_eq!((fg(&buffer, mark), bold(&buffer, mark)), (token, true), "{title}'s mark token");
        }
        let rooftop = at(&rows, "Rooftop Cypher");
        assert_eq!(buffer[(rooftop.x - 2, rooftop.y)].symbol(), " ", "another server's album copy marks nothing");
        // The tiles' artist rows: the state word in its token, the artist
        // where none speaks; never on the cover.
        assert!(room(&rows, 12).starts_with("copying…     ✓ kept       ALM          ! failed     Boukmanflow"), "{all}");
        for (word, token) in [("copying…", th().accent), ("✓ kept", th().ok), ("! failed", th().gold), ("ALM", th().dim)] {
            let at = at(&rows[12..13], word);
            assert_eq!(fg(&buffer, Position { x: at.x, y: 12 }), token, "{word}");
        }
        // No number on the page (clause 126): only m:ss lengths.
        let time = regex::Regex::new(r"\b\d+:\d\d\b").unwrap();
        for (y, row) in rows.iter().enumerate().take(24).skip(4) {
            let room: String = time.replace_all(&row.chars().skip(17).collect::<String>(), "").into_owned();
            assert!(!room.chars().any(|c| c.is_ascii_digit()) && !room.contains('%'), "a number on row {y}:\n{all}");
        }
        // The mark's words under the pointer: a mouse-only extra.
        let late = at(&rows, "Late Train Verse");
        assert!(gui.ui.tips.iter().any(|(r, t)| r.x == late.x - 2 && r.y == late.y && t == "copying…"), "the LEAD's tip");
    }

    #[test]
    fn the_held_row_keeps_its_glyph_in_the_selection_colours() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        draw(&mut gui, 100, 30);
        // ↓ lands on the strip, ↓ the first row, then two more.
        for _ in 0..4 {
            key(&mut gui, KeyCode::Down);
        }
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let late = at(&rows, "Late Train Verse");
        let mark = Position { x: late.x - 2, y: late.y };
        assert_eq!(buffer[(mark.x, mark.y)].symbol(), "↓");
        assert_eq!((fg(&buffer, mark), bg(&buffer, mark), bold(&buffer, mark)), (th().on_accent, th().accent, true));
        assert_eq!(
            super::super::discover::tips(&gui),
            "Enter play from here · a queue · N next · P play now · c keep a copy · m actions · Esc done",
            "the held connected server's row names `c` (clause 119)"
        );
    }

    // ── The indicator (clause 116) ──────────────────────────────────────

    #[test]
    fn the_indicator_says_copying_and_failed_in_words_and_squeezes_to_glyphs() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        gui.ui.key_hints = true;
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert!(room(&rows, 14).starts_with("▸ play · + queue all · ⇄ shuffle"), "{}", rows[14]);
        assert!(rows[14].trim_end().ends_with("copying… · ! failed"), "{}", rows[14]);
        let ind = at(&rows[14..15], "copying…");
        assert_eq!(ind.x, 78, "it ends at cell 96 (the canvas's TermRoom a)");
        assert_eq!(fg(&buffer, Position { x: 78, y: 14 }), th().accent);
        assert_eq!(fg(&buffer, Position { x: 87, y: 14 }), th().dim);
        assert_eq!(fg(&buffer, Position { x: 89, y: 14 }), th().gold);
        assert_eq!(gui.ui.hit(Position { x: 90, y: 14 }), Some(Act::Disc(DiscAct::Copies)));
        assert!(gui.ui.tips.iter().any(|(r, t)| r.x == 78 && r.y == 14 && t == "Copies — C"));
        // The 47-cell room squeezes it to glyphs, never clipped.
        gui.queue_open = true;
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(rows[14].chars().skip(59).take(5).collect::<String>(), "↓ · !", "{}", rows[14]);
        assert!(room(&rows, 14).starts_with("▸ play · + queue all · ⇄ shuffle"), "the verbs stand");
        assert!(gui.ui.tips.iter().any(|(r, t)| r.x == 59 && t == "copying… · ! failed — Copies — C"), "the squeeze's words in its tip");
        // A click opens the Copies modal and asks the list.
        gui.queue_open = false;
        draw(&mut gui, 100, 30);
        gui.act(Act::Disc(DiscAct::Copies));
        assert!(gui.discover.copies.is_some(), "the Copies modal");
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::Jobs { .. }))), "its first ask");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.copies.is_none(), "Esc closes it");
        // `C` too, anywhere in the room.
        key(&mut gui, KeyCode::Char('C'));
        assert!(gui.discover.copies.is_some(), "C opens it");
        key(&mut gui, KeyCode::Esc);
        // Nothing live and nothing failed: hidden, the line ends at shuffle.
        let kept: Vec<PluginJob> = canvas_jobs().into_iter().filter(|j| j.state == crate::api::types::JobState::Done).collect();
        let mut quiet = canvas_gui(&kept);
        let rows = draw(&mut quiet, 100, 30);
        assert_eq!(room(&rows, 14), "▸ play · + queue all · ⇄ shuffle", "hidden");
        assert!(!super::super::discover::tips(&quiet).contains("C copies"));
    }

    #[test]
    fn the_rest_tip_names_c_copies_only_while_the_indicator_stands() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        draw(&mut gui, 100, 30);
        assert_eq!(
            super::super::discover::tips(&gui),
            "↓ pick an album · ← → tabs · w window · e more options · G new seeds · C copies"
        );
    }

    #[test]
    fn the_legacy_console_draws_the_marks_and_the_indicator_in_its_own_glyphs() {
        let _en = english();
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let mut gui = canvas_gui(&canvas_jobs());
        let rows = draw(&mut gui, 100, 30);
        let all = rows.join("\n");
        for row in ["v Late Train Verse", "~ Brass District", "+ Northern Signal", "! Static Lullaby", "> Glass Hours"] {
            assert!(all.contains(row), "{row}:\n{all}");
        }
        assert!(room(&rows, 12).starts_with("copying...   + kept       ALM          ! failed"), "{all}");
        assert!(rows[14].trim_end().ends_with("copying... · ! failed"), "{}", rows[14]);
        gui.queue_open = true;
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(rows[14].chars().skip(59).take(5).collect::<String>(), "v · !");
    }

    // ── Ways in (clauses 101–103) ───────────────────────────────────────

    #[test]
    fn c_opens_keep_a_copy_on_the_held_row_else_the_hovered_row_or_tile() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        let rows = draw(&mut gui, 100, 30);
        // Nothing held, nothing under the pointer: `c` passes.
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.taste.keep.flow.is_none());
        // The hovered row.
        gui.ui.pointer = Some(at(&rows, "Corner Store Loop"));
        draw(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Char('c'));
        let flow = gui.app.taste.keep.flow.as_ref().expect("Keep a copy on the hovered row");
        assert_eq!(flow.subject.title.as_deref(), Some("Corner Store Loop"));
        key(&mut gui, KeyCode::Esc);
        // The hovered tile: album focus.
        gui.ui.pointer = Some(Position { x: 20, y: 8 });
        draw(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Char('c'));
        let flow = gui.app.taste.keep.flow.as_ref().expect("Keep a copy on the hovered tile");
        assert!(flow.subject.album_focus() && flow.subject.album.as_deref() == Some("Borrowed Tapes"));
        key(&mut gui, KeyCode::Esc);
        // The held row wins over the pointer.
        for _ in 0..4 {
            key(&mut gui, KeyCode::Down);
        }
        key(&mut gui, KeyCode::Char('c'));
        assert_eq!(gui.app.taste.keep.flow.as_ref().and_then(|f| f.subject.title.as_deref()), Some("Late Train Verse"));
        key(&mut gui, KeyCode::Esc);
        // The lit tile: `c` in album focus, `m` its list.
        gui.ui.pointer = None;
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Down);
        assert!(gui.discover.strip.on);
        assert_eq!(
            super::super::discover::tips(&gui),
            "← → albums · Enter play the album · + a queue · c keep a copy · m more · ↓ songs · Esc done"
        );
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.taste.keep.flow.as_ref().is_some_and(|f| f.subject.album_focus()));
    }

    #[test]
    fn with_the_copy_gate_closed_every_way_in_is_hidden_never_grey() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        // The account may not upload: the gate's fourth part (clause 101).
        gui.app.capabilities.uploads = false;
        let rows = draw(&mut gui, 100, 30);
        assert!(!rows[14].contains("copying…"), "no indicator: {}", rows[14]);
        gui.ui.pointer = Some(at(&rows, "Corner Store Loop"));
        draw(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.taste.keep.flow.is_none(), "c does nothing");
        key(&mut gui, KeyCode::Char('C'));
        assert!(gui.discover.copies.is_none(), "C does nothing");
        gui.act(Act::Disc(DiscAct::More(3)));
        let all = draw(&mut gui, 100, 30).join("\n");
        assert!(!all.contains("Keep a copy"), "no row:\n{all}");
        assert!(all.contains("Open elsewhere…"), "the links gate is its own:\n{all}");
        assert_eq!(super::super::actions::tips(&gui).as_deref(), Some("↑ ↓ rows · Enter choose · Esc close"));
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Esc);
        gui.act(Act::Disc(DiscAct::TileList(0)));
        let all = draw(&mut gui, 100, 30).join("\n");
        assert!(all.contains("Play album") && all.contains("Queue album") && !all.contains("Keep a copy"), "{all}");
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · Enter pick · a queue · Esc close");
    }

    #[test]
    fn a_connected_servers_song_sheet_gains_keep_a_copy_and_open_elsewhere() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        draw(&mut gui, 100, 30);
        gui.act(Act::Disc(DiscAct::More(2)));
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let all = rows.join("\n");
        // 66 × 14, centred: the rows of the canvas's TermEntryPoints (a).
        assert_eq!(rows[8].chars().nth(17), Some('╭'), "{all}");
        assert_eq!(rows[21].chars().nth(17), Some('╰'), "{all}");
        let labels: Vec<String> = (14..20).map(|y| rows[y].chars().skip(19).take(30).collect::<String>().trim_end().to_string()).collect();
        assert_eq!(labels, ["Play now", "Add next", "Add to end of queue", "Keep a copy…  c", "Song Info", "Open elsewhere…"]);
        let c = Position { x: 33, y: 17 };
        assert_eq!((buffer[(c.x, c.y)].symbol(), fg(&buffer, c)), ("c", th().dim), "the key DIM after the label");
        let badge = rows[12].chars().skip(19).collect::<String>();
        assert!(badge.starts_with("from Stage Peer B   • online"), "{badge}");
        assert_eq!(fg(&buffer, Position { x: 39, y: 12 }), th().ok, "the dot OK");
        assert_eq!(fg(&buffer, Position { x: 41, y: 12 }), th().dim, "the words DIM");
        assert!(!all.contains('★'), "no stars: a rating is your own server's");
        assert_eq!(super::super::actions::tips(&gui).as_deref(), Some("↑ ↓ rows · Enter choose · c keep a copy · Esc close"));
        // Open elsewhere: asked now, its row ACCENT while its list hangs;
        // Esc folds the list and keeps the sheet.
        gui.act(Act::SheetVerb(super::super::actions::SheetAction::OpenElsewhere));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::PluginLinks { plugin, .. }) if plugin == "links")));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        assert_eq!(fg(&buffer, Position { x: 19, y: 19 }), th().accent);
        key(&mut gui, KeyCode::Esc);
        assert!(gui.actions.sheet.as_ref().is_some_and(|s| !s.links) && gui.app.taste.keep.links.is_none());
        // `c` from anywhere in the sheet: the sheet gives way to the modal.
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.actions.sheet.is_none());
        assert_eq!(gui.app.taste.keep.flow.as_ref().and_then(|f| f.subject.title.as_deref()), Some("Late Train Verse"));
    }

    #[test]
    fn a_tiles_list_hangs_from_the_tile_with_its_verbs() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        draw(&mut gui, 100, 30);
        // A right click on a connected server's tile.
        assert_eq!(gui.ui.hit_context(Position { x: 20, y: 8 }), Some(Act::Disc(DiscAct::TileList(0))));
        gui.act(Act::Disc(DiscAct::TileList(0)));
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        // The canvas's TermEntryPoints (b): from the tile's first cell, one
        // row under it, 24 wide, ACCENT, no title, no [X].
        assert_eq!(rows[13].chars().skip(16).take(24).collect::<String>(), "╭──────────────────────╮");
        let shown: Vec<String> = (14..17).map(|y| rows[y].chars().skip(16).take(24).collect()).collect();
        assert_eq!(shown, ["│ Play album           │", "│ Queue album  a       │", "│ Keep a copy…  c      │"]);
        assert_eq!(rows[17].chars().skip(16).take(24).collect::<String>(), "╰──────────────────────╯", "no Open on: the server is not saved");
        assert_eq!(fg(&buffer, Position { x: 16, y: 13 }), th().accent);
        assert_eq!(fg(&buffer, Position { x: 31, y: 15 }), th().dim, "Queue album's key DIM");
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · Enter pick · a queue · c keep a copy · Esc close");
        // A click elsewhere closes and keeps; ↓ and Enter pick.
        assert_eq!(gui.ui.hit(Position { x: 60, y: 20 }), Some(Act::Disc(DiscAct::TileListClose)));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.discover.tile_list.is_none());
        assert!(gui.app.taste.keep.flow.as_ref().is_some_and(|f| f.subject.album_focus()), "Keep a copy in album focus");
        key(&mut gui, KeyCode::Esc);
        // `a` queues from inside it.
        gui.act(Act::Disc(DiscAct::TileList(0)));
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('a'));
        assert!(gui.discover.tile_list.is_none());
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::TasteAlbum { play: false, .. }))), "{:?}", gui.pending);
    }

    #[test]
    fn open_on_moves_to_the_saved_connected_server_and_opens_the_album_there() {
        let _en = english();
        // Moving to another server saves the settings.
        let _config = crate::config::testing::Scratch::new("keep-open-on");
        let mut gui = canvas_gui(&canvas_jobs());
        gui.config.servers = vec![
            crate::config::ServerEntry { url: HOST.into(), ..Default::default() },
            crate::config::ServerEntry {
                url: format!("mstream+peer://{B}@{HOST}"),
                peer: Some(crate::config::PeerEntry { parent: HOST.into(), id: B, name: "Stage Peer B".into(), missing: false, hidden: false }),
                ..Default::default()
            },
        ];
        draw(&mut gui, 100, 30);
        gui.act(Act::Disc(DiscAct::TileList(0)));
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(rows[17].chars().skip(16).take(24).collect::<String>(), "│ Open on Stage Peer B │");
        gui.act(Act::Disc(DiscAct::TileVerb(super::super::discover::TileVerb::OpenOn(1))));
        assert_eq!(gui.app.session.peer, Some((HOST.to_string(), B)), "the session moved to Stage Peer B");
        assert_eq!(gui.active, super::super::ALBUMS_NAV, "the Albums room");
        assert_eq!(
            gui.discover.open_on,
            Some(crate::tui::worker::LibraryNode::Album { name: "Borrowed Tapes".into(), artist: None }),
            "the album waits for that server's answer"
        );
    }

    // ── The Keep a copy window (clauses 106–110; SPEC "calmer") ─────────

    /// The window's rows from B1, the head's next row, down to its bottom
    /// border, 76 wide.
    fn lower_rows(rows: &[String]) -> Vec<String> {
        modal_rows(rows)[3..].to_vec()
    }

    fn canvas(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|r| r.to_string()).collect()
    }

    /// A board's frame as SPEC §5 prints it, a row a line.
    fn board(frame: &str) -> Vec<String> {
        frame.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
    }

    /// The pointer's act at a cell.
    fn hit_at(gui: &Gui, x: u16, y: u16) -> Option<Act> {
        gui.ui.hit(Position { x, y })
    }

    fn keep_act(act: super::KeepAct) -> Option<Act> {
        Some(Act::Disc(DiscAct::Keep(act)))
    }

    /// C5, the song row idle: its words, tokens, cells and controls, as
    /// the boards draw them (SPEC §5.1, §3.7, §6.3).
    #[test]
    fn the_modal_from_a_row_opens_on_the_song_where_the_canvas_draws_it() {
        let _en = english();
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        // The sizes still asked: no box, the primary without its count.
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(
            modal_rows(&rows),
            board(
                "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Late Train Verse                                                         │
                │ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
                │                                                                          │
                │ (•) This song                                                            │
                │ ( ) Album                                                                │
                │ ( ) Every album by Boukmanflow                                           │
                │                                                                          │
                │ Files go to copies › Boukmanflow › Borrowed Tapes               d change │
                │                                                                          │
                │                                                             [ Add song ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
                "
            )
        );
        board_facts(&mut gui);
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        // 76 wide at x 19–94, centred in the room; its top on row 6, 13 rows.
        assert_eq!((rows[6].chars().nth(19), rows[6].chars().nth(94)), (Some('╭'), Some('╮')), "{}", rows.join("\n"));
        assert_eq!(rows[18].chars().nth(19), Some('╰'));
        assert_eq!(
            modal_rows(&rows),
            board(
                "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Late Train Verse                                                         │
                │ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
                │                                                                          │
                │ (•) This song                                                            │
                │ ( ) Album                                                                │
                │ ( ) Every album by Boukmanflow                                           │
                │     [✓] Only the 1 album you don't have                                  │
                │                                                                          │
                │ Files go to copies › Boukmanflow › Borrowed Tapes               d change │
                │                                                                          │
                │                                                             [ Add song ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
                "
            )
        );
        // The runs (§5.1): the frame ACCENT, its title TEXT BOLD; the item's
        // title BOLD, the byline DIM; the chosen dot ACCENT and its label
        // BOLD, the others DIM and TEXT; the box DIM, its label TEXT; "Files
        // go to" DIM, the library BOLD, `›` DIM, the folders TEXT; `d change`
        // DIM; the one primary ACCENT BOLD; "Esc close" DIM.
        let cell = |x: u16, y: u16| Position { x, y };
        let text = |at: Position| (fg(&buffer, at), bold(&buffer, at));
        assert_eq!(fg(&buffer, cell(19, 6)), th().accent);
        assert_eq!(text(cell(22, 6)), (Color::Reset, true), "the title on the border, TEXT BOLD");
        assert_eq!(text(cell(21, 7)), (Color::Reset, true));
        assert_eq!(fg(&buffer, cell(21, 8)), th().dim);
        assert_eq!(text(cell(22, 10)), (th().accent, false), "the chosen dot ACCENT, never bold");
        assert_eq!(text(cell(25, 10)), (Color::Reset, true));
        assert_eq!(fg(&buffer, cell(22, 11)), th().dim);
        assert_eq!(text(cell(25, 11)), (Color::Reset, false));
        assert_eq!(fg(&buffer, cell(26, 13)), th().accent, "the box ticked from the start, ACCENT");
        assert_eq!(text(cell(29, 13)), (Color::Reset, true), "a ticked box's words BOLD");
        assert_eq!(fg(&buffer, cell(21, 15)), th().dim);
        assert_eq!(text(cell(33, 15)), (Color::Reset, true), "the library BOLD");
        assert_eq!(fg(&buffer, cell(40, 15)), th().dim, "› DIM");
        assert_eq!(text(cell(42, 15)), (Color::Reset, false));
        assert_eq!(at(&rows, "d change"), cell(85, 15));
        assert_eq!(text(cell(85, 15)), (th().dim, false));
        assert_eq!(at(&rows, "[ Add song ]"), cell(81, 17));
        assert_eq!(text(cell(81, 17)), (th().accent, true), "the one ACCENT BOLD element");
        assert_eq!(at(&rows, "Esc close"), cell(83, 18));
        assert_eq!(text(cell(83, 18)), (th().dim, false));
        // Nothing is filled: no slab anywhere in the window, the ground its
        // own.
        let ground = bg(&buffer, cell(40, 9));
        for y in 6..=18 {
            for x in 19..=94 {
                assert_eq!(bg(&buffer, cell(x, y)), ground, "a fill at ({x}, {y})");
            }
        }
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ choose · Enter starts · d where files land · Esc close");
        // The pointer: a choice's row picks it, the box toggles, `d change`
        // opens the picker, the primary presses, "Esc close" closes; every
        // other cell — the window's, the band's, the room's — is inert.
        use crate::tui::app::keep::KeepChoice;
        assert_eq!(hit_at(&gui, 21, 10), keep_act(super::KeepAct::Pick(KeepChoice::Song)));
        assert_eq!(hit_at(&gui, 60, 11), keep_act(super::KeepAct::Pick(KeepChoice::Album)));
        assert_eq!(hit_at(&gui, 92, 12), keep_act(super::KeepAct::Pick(KeepChoice::Artist)));
        assert_eq!(hit_at(&gui, 26, 13), keep_act(super::KeepAct::Missing));
        assert_eq!(hit_at(&gui, 59, 13), keep_act(super::KeepAct::Missing));
        assert_eq!(hit_at(&gui, 70, 13), Some(Act::Guard));
        assert_eq!(hit_at(&gui, 88, 15), keep_act(super::KeepAct::Key(super::KeyWord::Change)));
        assert_eq!(hit_at(&gui, 50, 15), Some(Act::Guard), "the place is words, not a control");
        assert_eq!(hit_at(&gui, 85, 17), keep_act(super::KeepAct::Primary));
        assert_eq!(hit_at(&gui, 40, 17), Some(Act::Guard));
        assert_eq!(hit_at(&gui, 87, 18), keep_act(super::KeepAct::Close));
        assert_eq!(hit_at(&gui, 93, 18), Some(Act::Guard));
        assert_eq!(hit_at(&gui, 18, 10), Some(Act::Guard), "the band");
        assert_eq!(hit_at(&gui, 40, 25), Some(Act::Guard), "the room beneath");
        // The pointer lights one target, BRIGHT BOLD: a choice's label (its
        // dot keeps DIM, the chosen keeps its own), the primary, `d change`,
        // "Esc close".
        for (pointer, lit) in [(cell(60, 11), cell(25, 11)), (cell(85, 17), cell(81, 17)), (cell(88, 15), cell(85, 15)), (cell(85, 18), cell(83, 18))] {
            gui.ui.pointer = Some(pointer);
            let buffer = super::tests::buffer(&mut gui, 100, 30);
            assert_eq!((fg(&buffer, lit), bold(&buffer, lit)), (th().bright, true), "lit at {lit:?}");
            assert_eq!(fg(&buffer, cell(22, 11)), th().dim);
            assert_eq!((fg(&buffer, cell(22, 10)), bold(&buffer, cell(25, 10))), (th().accent, true));
        }
        // Its footprint, the band, is registered: the room's covers draw as
        // the mosaic under it.
        assert!(gui.ui.covered_last_frame(ratatui::layout::Rect { x: 17, y: 6, width: 80, height: 13 }));
    }

    /// The window's rows, drawn.
    fn window_of(gui: &mut Gui) -> Vec<String> {
        modal_rows(&draw(gui, 100, 30))
    }

    #[test]
    fn the_keys_walk_the_choice_and_add_starts_with_the_row_unchanged() {
        let _en = english();
        let mut gui = opened(LATE);
        let shown = |gui: &mut Gui| {
            let low = lower_rows(&draw(gui, 100, 30));
            (low[1..5].to_vec(), low[6].clone(), low[8].clone())
        };
        // ↓ moves the choice itself: Album, its primary counting its songs.
        key(&mut gui, KeyCode::Down);
        let (choice, place, action) = shown(&mut gui);
        assert_eq!(choice[1], "│ (•) Album                                                                │");
        assert_eq!(place, "│ Files go to copies › Boukmanflow › Borrowed Tapes               d change │");
        assert_eq!(action, "│                                                  [ Add album · 3 songs ] │");
        // Every album by, the box ticked from the start: ↓ lands on the box,
        // the dot on Every album by — the one album it takes named where it
        // lands.
        key(&mut gui, KeyCode::Down);
        let (choice, place, action) = shown(&mut gui);
        assert_eq!(choice[2..4], ["│ (•) Every album by Boukmanflow                                           │", "│     [✓] Only the 1 album you don't have                                  │"]);
        assert_eq!(place, "│ Files go to copies › Boukmanflow › Borrowed Tapes               d change │");
        assert_eq!(action, "│                                                          [ Add 1 album ] │");
        // Clamped: ↓ again moves nothing.
        key(&mut gui, KeyCode::Down);
        assert_eq!(shown(&mut gui).0[3], "│     [✓] Only the 1 album you don't have                                  │");
        // ↑ unticks it, the dot still on Every album by: the server's albums
        // by the artist, album by album.
        key(&mut gui, KeyCode::Up);
        let (choice, place, action) = shown(&mut gui);
        assert_eq!(choice[2..4], ["│ (•) Every album by Boukmanflow                                           │", "│     [ ] Only the 1 album you don't have                                  │"]);
        assert_eq!(place, "│ Files go to copies › Boukmanflow › one folder per album         d change │");
        assert_eq!(action, "│                                                         [ Add 2 albums ] │");
        assert!(gui.app.keep_view().is_some_and(|v| v.missing == Some((false, 1)) && v.scope == crate::api::types::JobScope::Artist));
        // ↓ ticks it again; ↑ ↑ ↑ back to This song, clamped there.
        key(&mut gui, KeyCode::Down);
        assert!(gui.app.keep_view().is_some_and(|v| v.missing == Some((true, 1)) && v.scope == crate::api::types::JobScope::ArtistMissing));
        for _ in 0..4 {
            key(&mut gui, KeyCode::Up);
        }
        assert_eq!(shown(&mut gui).0[0], "│ (•) This song                                                            │");
        // Picking a radio leaves the box's tick as it was: it counts only
        // under Every album by.
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Pick(crate::tui::app::keep::KeepChoice::Song))));
        assert!(gui.app.keep_view().is_some_and(|v| v.missing == Some((false, 1)) && v.scope == crate::api::types::JobScope::Song));
        // Space, Tab, ← → and the letters no word names do nothing.
        let before = window_of(&mut gui);
        for code in [KeyCode::Char(' '), KeyCode::Tab, KeyCode::Left, KeyCode::Right, KeyCode::Char('x'), KeyCode::Char('a'), KeyCode::Char('c'), KeyCode::Char('C')] {
            key(&mut gui, code);
        }
        assert_eq!(window_of(&mut gui), before);
        assert!(starts(&gui).is_empty() && gui.discover.copies.is_none() && gui.discover.keep.dest.is_none(), "{:?}", gui.pending);
        // Enter starts at once — the row exactly as the taste route sent it
        // (clause 100) — and nothing moves but the primary, which goes off.
        key(&mut gui, KeyCode::Enter);
        assert_eq!(starts(&gui), vec![(late(), crate::api::types::JobScope::Song)]);
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(
            lower_rows(&rows)[8..],
            canvas(&[
                "│                                                              [ Adding… ] │",
                "╰────────────────────────────────────────────────────────────── Esc close ─╯",
            ])[..]
        );
        assert_eq!(fg(&buffer(&mut gui, 100, 30), at(&rows, "[ Adding… ]")), th().dim);
        assert_eq!(super::super::discover::tips(&gui), "Esc close");
        // ↑ ↓, `d`, Enter and the pointer wait for the server's answer.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('d'));
        key(&mut gui, KeyCode::Enter);
        assert_eq!(starts(&gui).len(), 1, "one start");
        assert!(gui.discover.keep.dest.is_none());
        assert_eq!(window_of(&mut gui)[4], "│ (•) This song                                                            │");
        assert_eq!(hit_at(&gui, 40, 11), Some(Act::Guard));
        assert_eq!(hit_at(&gui, 88, 15), Some(Act::Guard), "d change drawn, inert");
        assert_eq!(hit_at(&gui, 85, 17), Some(Act::Guard), "the primary off");
        assert_eq!(hit_at(&gui, 87, 18), keep_act(super::KeepAct::Close));
        // Esc closes; the start still lands.
        key(&mut gui, KeyCode::Esc);
        assert!(gui.app.taste.keep.flow.is_none());
    }

    #[test]
    fn every_state_reads_as_the_canvas_draws_it() {
        let _en = english();
        let cases: Vec<(&str, &str, &str)> = vec![
            ("L1", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ Waiting for a free slot                                                  │
                │ ▱▱▱▱▱▱▱▱▱▱ starts on its own                                             │
                │                                                                          │
                │ Into copies › Boukmanflow › Borrowed Tapes                               │
                │                                                                          │
                │ x cancel copy                                                  [ Close ] │
                ╰─────────────────────────────────────── Esc close · the copy keeps going ─╯
            ", "Enter close · x cancel the copy · Esc close — the copy keeps going"),
            ("L2", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ Copying · song 2 of 3 · Late Train Verse                                 │
                │ ▰▰▰▰▰▱▱▱▱▱ 52%                                                           │
                │                                                                          │
                │ Into copies › Boukmanflow › Borrowed Tapes                               │
                │                                                                          │
                │ x cancel copy                                                  [ Close ] │
                ╰─────────────────────────────────────── Esc close · the copy keeps going ─╯
            ", "Enter close · x cancel the copy · Esc close — the copy keeps going"),
            ("L3", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ Copying · album 1 of 2 · Borrowed Tapes · song 2 of 3                    │
                │ ▰▰▰▰▱▱▱▱▱▱ 38%                                                           │
                │                                                                          │
                │ Into copies › Boukmanflow › one folder per album                         │
                │                                                                          │
                │ x cancel copy                                                  [ Close ] │
                ╰─────────────────────────────────────── Esc close · the copy keeps going ─╯
            ", "Enter close · x cancel the copy · Esc close — the copy keeps going"),
            ("L4", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Late Train Verse                                                         │
                │ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
                │                                                                          │
                │ Copying                                                                  │
                │ ▱▱▱▱▱▱▱▱▱▱                                                               │
                │                                                                          │
                │ Into copies › Boukmanflow › Borrowed Tapes                               │
                │                                                                          │
                │ x cancel copy                                                  [ Close ] │
                ╰─────────────────────────────────────── Esc close · the copy keeps going ─╯
            ", "Enter close · x cancel the copy · Esc close — the copy keeps going"),
            ("L5", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ Stopping…                                                                │
                │ ▰▰▰▰▰▱▱▱▱▱ 52%                                                           │
                │                                                                          │
                │ Into copies › Boukmanflow › Borrowed Tapes                               │
                │                                                                          │
                │                                                                [ Close ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter close · Esc close"),
            ("E1", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Late Train Verse                                                         │
                │ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
                │                                                                          │
                │ ✓ In your collection                                                     │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes › 02 - Late Train Verse.mp3     │
                │                                                                          │
                │ a add to queue   c choose again                       [ Play your copy ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter play your copy · a add to queue · c choose again · Esc close"),
            ("E2", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Late Train Verse                                                         │
                │ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
                │                                                                          │
                │ ✓ In your collection                                                     │
                │ Copied from Stage Peer C                                                 │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes › 02 - Late Train Verse.mp3     │
                │                                                                          │
                │ a add to queue   c choose again                       [ Play your copy ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter play your copy · a add to queue · c choose again · Esc close"),
            ("E3", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ✓ In your collection                                                     │
                │ 2 copied · 1 already yours                                               │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes                                 │
                │                                                                          │
                │ c choose again                                        [ Play your copy ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter play your copy · c choose again · Esc close"),
            ("E4", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ✓ In your collection                                                     │
                │ 3 copied · 3 already yours                                               │
                │                                                                          │
                │ In copies › Boukmanflow                                                  │
                │                                                                          │
                │ c choose again                                                 [ Close ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter close · c choose again · Esc close"),
            ("E5", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! 1 song didn't copy                                                     │
                │ 2 copied                                                                 │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes                                 │
                │                                                                          │
                │ c choose again                                                 [ Retry ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter retry · c choose again · Esc close"),
            ("E5b", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! 1 album couldn't be listed                                             │
                │ 3 copied                                                                 │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes                                 │
                │                                                                          │
                │ c choose again                                                 [ Retry ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter retry · c choose again · Esc close"),
            ("E6", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Late Train Verse                                                         │
                │ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
                │                                                                          │
                │ ✓ Already yours                                                          │
                │ Your library already has this song                                       │
                │                                                                          │
                │ In music › Boukmanflow › Borrowed Tapes › 02 Late Train Verse.mp3        │
                │                                                                          │
                │ c choose again                                                 [ Close ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter close · c choose again · Esc close"),
            ("E7", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Late Train Verse                                                         │
                │ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
                │                                                                          │
                │ ! Skipped · a file is already there                                      │
                │ Nothing was overwritten                                                  │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes › 02 - Late Train Verse.mp3     │
                │                                                                          │
                │ d change   c choose again                                      [ Close ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter close · d where files land · c choose again · Esc close"),
            ("E8", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Late Train Verse                                                         │
                │ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
                │                                                                          │
                │ ! Skipped · a file is already there                                      │
                │ Nothing was overwritten                                                  │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes › 02 - Late Train Verse.mp3     │
                │ Retry adds to copies › from-peers › Boukmanflow › Borrowed Tapes         │
                │                                                                          │
                │ d change   c choose again                                      [ Retry ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter retry · d where files land · c choose again · Esc close"),
            ("E9", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! Copy failed: Stage Peer B is unreachable                               │
                │ Nothing was copied                                                       │
                │                                                                          │
                │ c choose again                                                 [ Retry ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter retry · c choose again · Esc close"),
            ("E9b", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! Copy failed: Stage Peer B is unreachable                               │
                │ Nothing was copied                                                       │
                │                                                                          │
                │ Retry adds to copies › from-peers › Boukmanflow › Borrowed Tapes         │
                │                                                                          │
                │ c choose again                                                 [ Retry ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter retry · c choose again · Esc close"),
            ("E10", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! Copy failed                                                            │
                │ Nothing was copied · 3 failed                                            │
                │                                                                          │
                │ c choose again                                                 [ Retry ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter retry · c choose again · Esc close"),
            ("E11", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! Stage Peer B's transfer limit stopped it                               │
                │ 1 copied · 2 not copied · try again later                                │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes                                 │
                │                                                                          │
                │ c choose again                                                 [ Retry ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter retry · c choose again · Esc close"),
            ("E11b", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! Stage Peer B is serving too many streams right now                     │
                │ 1 copied · 2 not copied · try again in a moment                          │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes                                 │
                │                                                                          │
                │ c choose again                                                 [ Retry ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter retry · c choose again · Esc close"),
            ("E11c", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! The copy stopped before it finished                                    │
                │ 1 copied · 2 not copied                                                  │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes                                 │
                │                                                                          │
                │ c choose again                                                 [ Retry ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter retry · c choose again · Esc close"),
            ("E12", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! Stage Peer B does not allow copies with your server's key              │
                │ Nothing was copied · playing it still works                              │
                │ Only Stage Peer B's admin can change this.                               │
                │                                                                          │
                │                                                                [ Close ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter close · Esc close"),
            ("E12b", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! Stage Peer B does not allow copies with your server's key              │
                │ 1 copied · playing it still works                                        │
                │ Only Stage Peer B's admin can change this.                               │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes                                 │
                │                                                                          │
                │                                                                [ Close ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter close · Esc close"),
            ("E13", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ × Cancelled                                                              │
                │ 1 copied · the other 2 stay on Stage Peer B                              │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes                                 │
                │                                                                          │
                │                                                            [ Add again ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter add again · Esc close"),
            ("E14", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ × Cancelled                                                              │
                │ Nothing was copied                                                       │
                │                                                                          │
                │                                                            [ Add again ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter add again · Esc close"),
            ("E15", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ Nothing to copy                                                          │
                │ You have it all                                                          │
                │                                                                          │
                │ c choose again                                                 [ Close ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter close · c choose again · Esc close"),
            ("E16", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ Nothing of theirs to copy                                                │
                │ Boukmanflow only appears on other artists' albums there                  │
                │                                                                          │
                │ c choose again                                                 [ Close ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter close · c choose again · Esc close"),
            ("E18", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Late Train Verse                                                         │
                │ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
                │                                                                          │
                │ ✓ In your collection                                                     │
                │ With your album copy                                                     │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes › 02 - Late Train Verse.mp3     │
                │                                                                          │
                │ a add to queue   c choose again                       [ Play your copy ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter play your copy · a add to queue · c choose again · Esc close"),
            ("E19", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! Stage Peer B went away                                                 │
                │ 1 copied · 2 not copied                                                  │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes                                 │
                │ Retry adds to copies › from-peers › Boukmanflow › Borrowed Tapes         │
                │                                                                          │
                │ c choose again                                                 [ Retry ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter retry · c choose again · Esc close"),
            ("E19b", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! 1 song didn't copy                                                     │
                │ 2 copied                                                                 │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes                                 │
                │ Retry adds to copies › ‹genre› › Boukmanflow › 2019 - Borrowed Tapes     │
                │ ‹genre› comes from each song's own tags, so these songs can land in more │
                │ than one folder (a song with none leaves that folder out).               │
                │                                                                          │
                │ c choose again                                                 [ Retry ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter retry · c choose again · Esc close"),
            ("E20", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ! Copy failed: Stage Peer B is unreachable                               │
                │ Nothing was copied                                                       │
                │                                                                          │
                │                                                            [ Retrying… ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Esc close"),
            ("E21", "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ The copy finished                                                        │
                │                                                                          │
                │                                                                [ Close ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
            ", "Enter close · Esc close"),
        ];
        for (id, frame, tips) in cases {
            let mut gui = keep_in(id);
            let rows = draw(&mut gui, 100, 30);
            assert_eq!(modal_rows(&rows), board(frame), "{id}:\n{}", modal_rows(&rows).join("\n"));
            assert_eq!(rows[6].chars().nth(19), Some('╭'), "{id}: the top holds on row 6");
            assert_eq!(super::super::discover::tips(&gui), tips, "{id}");
        }
        // The tokens (§4.7): a kept headline OK BOLD, glyph and words; a
        // trouble's GOLD BOLD, its sub DIM; a neutral end's glyph DIM, its
        // words TEXT BOLD; a glyph-less one TEXT BOLD; the step TEXT after a
        // DIM ` · `, the bar's ▰ OK and ▱ DIM, its % TEXT.
        let cell = |x: u16, y: u16| Position { x, y };
        let tokens = |id: &str, cells: &[(u16, u16, Color, bool)]| {
            let mut gui = keep_in(id);
            let buffer = buffer(&mut gui, 100, 30);
            for &(x, y, color, heavy) in cells {
                assert_eq!((fg(&buffer, cell(x, y)), bold(&buffer, cell(x, y))), (color, heavy), "{id} at ({x}, {y})");
            }
        };
        tokens("E1", &[(21, 10, th().ok, true), (23, 10, th().ok, true), (21, 12, th().dim, false), (24, 12, Color::Reset, true), (64, 12, th().dim, false), (21, 14, th().dim, false), (75, 14, th().accent, true)]);
        tokens("E5", &[(21, 10, th().gold, true), (23, 10, th().gold, true), (21, 11, th().dim, false)]);
        tokens("E6", &[(21, 10, th().dim, false), (23, 10, Color::Reset, true)]);
        tokens("E13", &[(21, 10, th().dim, false), (23, 10, Color::Reset, true)]);
        tokens("E15", &[(21, 10, Color::Reset, true)]);
        tokens("E12", &[(21, 12, th().dim, false)]);
        tokens("L2", &[(21, 10, Color::Reset, true), (29, 10, th().dim, false), (31, 10, Color::Reset, false), (21, 11, th().ok, false), (26, 11, th().dim, false), (32, 11, Color::Reset, false), (21, 13, th().dim, false)]);
        tokens("E20", &[(80, 13, th().dim, false)]);
        // The footer: only "Esc close" is a button; its tail is plain.
        let gui = {
            let mut gui = keep_in("L2");
            draw(&mut gui, 100, 30);
            gui
        };
        assert_eq!(hit_at(&gui, 61, 16), keep_act(super::KeepAct::Close));
        assert_eq!(hit_at(&gui, 75, 16), Some(Act::Guard));
        assert_eq!(hit_at(&gui, 23, 15), keep_act(super::KeepAct::Key(super::KeyWord::Cancel)));
    }

    #[test]
    fn a_refusal_brings_the_choice_back_with_the_servers_words_in_gold() {
        let _en = english();
        let mut gui = keep_in("R1");
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(
            modal_rows(&rows),
            board(
                "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Late Train Verse                                                         │
                │ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
                │                                                                          │
                │ (•) This song                                                            │
                │ ( ) Album                                                                │
                │ ( ) Every album by Boukmanflow                                           │
                │     [✓] Only the 1 album you don't have                                  │
                │                                                                          │
                │ Files go to copies › Boukmanflow › Borrowed Tapes               d change │
                │ ! Couldn't start: you have 20 jobs queued or running — wait for some to  │
                │   finish (the limit is 20)                                               │
                │                                                                          │
                │                                                             [ Add song ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
                "
            )
        );
        // GOLD, not bold, its second row at col 3; the dot where it was.
        let words = at(&rows, "! Couldn't start");
        assert_eq!((fg(&buffer, words), bold(&buffer, words)), (th().gold, false));
        assert_eq!(fg(&buffer, Position { x: 23, y: 17 }), th().gold);
        assert_eq!(fg(&buffer, at(&rows, "(•) This song")), th().accent);
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ choose · Enter starts · d where files land · Esc close");
        // A busy 409 and a server that did not answer, in their words.
        let busy = "someone else on this server is already getting this — try again in a moment";
        start(&mut gui, Err(crate::api::PluginFailure::Refused { status: 409, words: busy.into() }));
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[7..9], ["│ ! Couldn't start: someone else on this server is already getting this —  │", "│   try again in a moment                                                  │"]);
        start(&mut gui, Err(crate::api::PluginFailure::Unreachable("connection refused".into())));
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[7], "│ ! Couldn't start: your server didn't answer                              │");
        // The choice stays editable and Enter adds again.
        key(&mut gui, KeyCode::Down);
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert_eq!(starts(&gui), vec![(late(), crate::api::types::JobScope::Album)]);
    }

    #[test]
    fn the_tile_opens_album_focus_and_sends_its_best_row_with_scope_album() {
        let _en = english();
        let mut gui = keep_in("C2");
        assert_eq!(
            window_of(&mut gui),
            board(
                "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ (•) Album                                                                │
                │ ( ) Every album by Boukmanflow                                           │
                │                                                                          │
                │ Files go to copies › Boukmanflow › Borrowed Tapes               d change │
                │                                                                          │
                │                                                            [ Add album ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
                "
            )
        );
        board_facts(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(
            modal_rows(&rows),
            board(
                "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ (•) Album                                                                │
                │ ( ) Every album by Boukmanflow                                           │
                │     [✓] Only the 1 album you don't have                                  │
                │                                                                          │
                │ Files go to copies › Boukmanflow › Borrowed Tapes               d change │
                │                                                                          │
                │                                                  [ Add album · 3 songs ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
                "
            )
        );
        // §3.7's cells: rows 6–17, the primary at x 70–92 on row 16.
        assert_eq!((rows[6].chars().nth(19), rows[17].chars().nth(19)), (Some('╭'), Some('╰')));
        assert_eq!(at(&rows, "[ Add album · 3 songs ]"), Position { x: 70, y: 16 });
        assert_eq!(at(&rows, "Only the 1 album"), Position { x: 29, y: 12 });
        // Enter starts at once: the tile's best row with scope album, never a
        // song job — no ↓ first.
        key(&mut gui, KeyCode::Enter);
        assert_eq!(starts(&gui), vec![(late(), crate::api::types::JobScope::Album)]);
    }

    #[test]
    fn a_choice_an_only_missing_copy_already_kept_reads_its_state_never_add() {
        let _en = english();
        // The user's 2026-10-20 report: "Only the albums you don't have"
        // copied Borrowed Tapes; the tile reads that — and says whose copy.
        let mut gui = keep_in("E17");
        assert_eq!(
            window_of(&mut gui),
            board(
                "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ ✓ In your collection                                                     │
                │ 3 copied · with your copy of Boukmanflow's missing albums                │
                │                                                                          │
                │ In copies › Boukmanflow › Borrowed Tapes                                 │
                │                                                                          │
                │ c choose again                                        [ Play your copy ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
                "
            )
        );
        // Play your copy plays the album from the library, in its order, and
        // the window has said what it had to.
        key(&mut gui, KeyCode::Enter);
        let queued: Vec<&str> = gui.app.queue.items.iter().map(|q| q.filepath.as_str()).collect();
        assert_eq!(queued, ["copies/Boukmanflow/Borrowed Tapes/01.mp3", "copies/Boukmanflow/Borrowed Tapes/02.mp3", "copies/Boukmanflow/Borrowed Tapes/03.mp3"]);
        assert_eq!(gui.app.queue.current, Some(0));
        assert!(gui.app.taste.keep.flow.is_none());
        // Left out as already yours: the songs are yours; no In row — the
        // walk names the album, not its folder; Close.
        let mut gui = keep_in("E17b");
        assert_eq!(
            lower_rows(&draw(&mut gui, 100, 30)),
            canvas(&[
                "│                                                                          │",
                "│ ✓ Already yours                                                          │",
                "│ Your library already has these songs · with your copy of Boukmanflow's   │",
                "│ missing albums                                                           │",
                "│                                                                          │",
                "│ c choose again                                                 [ Close ] │",
                "╰────────────────────────────────────────────────────────────── Esc close ─╯",
            ])
        );
        // A guest spot claims nothing: the choice, Add album.
        let mut gui = walked(json!([]), json!([{ "name": "Borrowed Tapes", "why": "appearance" }]));
        assert_eq!(window_of(&mut gui)[10], "│                                                  [ Add album · 3 songs ] │");
    }

    #[test]
    fn a_compilation_tile_copies_the_whole_album_by_its_row_unchanged() {
        let _en = english();
        // The user's 2026-10-19 decision: a row carrying albumArtist groups
        // as one tile credited to it, offers Album alone — no radio, the
        // primary names it — and sends the row unchanged.
        let (mut gui, kim) = mixtape_gui();
        let rows = draw(&mut gui, 100, 30);
        assert!(room(&rows, 11).starts_with("Mixtape Vol…") && room(&rows, 12).starts_with("Various Art…"), "one tile, its credit:\n{}", rows.join("\n"));
        assert!(!room(&rows, 11).contains("Mixtape Vol… Mixtape"), "never one per track artist");
        let mut gui = keep_in("C6");
        assert_eq!(
            window_of(&mut gui),
            board(
                "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Mixtape Vol 1                                                            │
                │ Various Artists · 2019 · from Stage Peer B                               │
                │                                                                          │
                │ Files go to copies › Various Artists › Mixtape Vol 1            d change │
                │                                                                          │
                │                                                  [ Add album · 2 songs ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
                "
            )
        );
        assert_eq!(super::super::discover::tips(&gui), "Enter starts · d where files land · Esc close");
        // ↑ ↓ have no choice to move; Enter adds the album.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(starts(&gui), vec![(kim, crate::api::types::JobScope::Album)], "the best row, unchanged, scope album");
    }

    /// The words under the place: its note's rows joined, down to the next
    /// blank.
    fn under_place(low: &[String]) -> String {
        let place = low.iter().rposition(|r| r.contains("d change") || r.contains("Into ") || r.contains("adds to")).expect("the place");
        low[place + 1..]
            .iter()
            .take_while(|r| r.chars().skip(1).take(74).any(|c| c != ' '))
            .map(|r| r.chars().skip(1).take(74).collect::<String>().trim().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_compilations_album_draws_each_songs_artist_as_a_slot_with_its_note_on_its_own_rows() {
        let _en = english();
        // The 2026-10-22 bug: "copies › From friends › Boukmanflow" for an
        // album whose songs the server files under each one's own artist —
        // the slot, its note on its own DIM rows, never only in a tooltip.
        let note = "‹artist› comes from each song's own tags, so these songs can land in more than one folder.";
        let mut gui = keep_in("C7");
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(
            modal_rows(&rows),
            board(
                "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Kim Song                                                                 │
                │ Peer Kim · Mixtape Vol 1 · from Stage Peer B                             │
                │                                                                          │
                │ ( ) This song                                                            │
                │ (•) Album                                                                │
                │ ( ) Every album by Peer Kim                                              │
                │                                                                          │
                │ Files go to copies › From friends › ‹artist›                    d change │
                │ ‹artist› comes from each song's own tags, so these songs can land in     │
                │ more than one folder.                                                    │
                │                                                                          │
                │                                                  [ Add album · 2 songs ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
                "
            )
        );
        // The slot DIM, as "one folder per album" — a hint, never a folder's
        // name; the note DIM; nothing clipped, so no tooltip.
        assert_eq!(fg(&buffer, at(&rows, "‹artist›")), th().dim);
        assert_eq!(fg(&buffer, at(&rows, "comes from each")), th().dim);
        assert_ne!(fg(&buffer, at(&rows, "From friends")), th().dim);
        assert!(!gui.ui.tips.iter().any(|(_, tip)| tip.contains("own tags") || tip.contains("From friends")), "{:?}", gui.ui.tips);
        // From the tile, before the listing answers and once it names two
        // artists: the slot and the note.
        let (mut gui, kim) = mixtape_gui();
        draw(&mut gui, 100, 30);
        super::open(&mut gui, TILE);
        land_read(&mut gui, layout_view("From friends/{{ARTIST}}"));
        for listing in [None, Some(&[("Kim Song", "Peer Kim"), ("Lou Song", "Peer Lou")])] {
            if let Some(songs) = listing {
                land_songs(&mut gui, songs);
            }
            let low = lower_rows(&draw(&mut gui, 100, 30));
            assert_eq!(low[1], "│ Files go to copies › From friends › ‹artist›                    d change │", "{}", low.join("\n"));
            assert_eq!(under_place(&low), note);
        }
        // The picker's preview draws the same words, its note whole on its
        // own two rows under the path, as Keep a copy draws it — no tooltip.
        key(&mut gui, KeyCode::Char('d'));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        let path = picker.iter().position(|r| r.contains("Files go to copies › From friends › ‹artist›")).expect("the preview");
        let preview = [
            " Files go to copies › From friends › ‹artist›",
            " ‹artist› comes from each song's own tags, so these songs can land in more",
            " than one folder.",
        ];
        assert_eq!(picker[path..path + 3].to_vec(), boxed(&preview)[1..4].to_vec(), "{}", picker.join("\n"));
        let y = at_row(&rows, "‹artist› comes from");
        assert!(!gui.ui.tips.iter().any(|(rect, _)| rect.contains(Position { x: 40, y })), "{:?}", gui.ui.tips);
        key(&mut gui, KeyCode::Esc);
        // Starting and queued: the place sent, then the job's pin — "Into" —
        // the slot and its note stand.
        key(&mut gui, KeyCode::Enter);
        assert_eq!(lower_rows(&draw(&mut gui, 100, 30))[1], "│ Files go to copies › From friends › ‹artist›                    d change │");
        let ticket = gui.pending.iter().rev().find_map(|e| match e {
            Effect::Api(ApiCmd::JobStart { ticket, .. }) => Some(*ticket),
            _ => None,
        });
        let mut queued = job(92, &kim, Some("album"), "queued", json!({}));
        queued.params.destination = Some(Some(serde_json::from_value(layout_view("From friends/{{ARTIST}}")["destination"].clone()).unwrap()));
        let generation = gui.app.session_gen();
        let answer = Ok(Box::new(crate::api::types::JobStarted { job: queued, created: true }));
        apply(&mut gui, Event::JobStarted { ticket: ticket.expect("a start"), generation, answer });
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[4], "│ Into copies › From friends › ‹artist›                                    │", "{}", low.join("\n"));
        assert_eq!(under_place(&low), note);
        // Every song by one artist: that folder, no slot and no note.
        let (mut gui, _) = mixtape_gui();
        draw(&mut gui, 100, 30);
        super::open(&mut gui, TILE);
        land_read(&mut gui, layout_view("From friends/{{ARTIST}}"));
        land_songs(&mut gui, &[("Kim Song", "Peer Kim"), ("Kim Two", "Peer Kim")]);
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[1..3], ["│ Files go to copies › From friends › Peer Kim                    d change │", "│                                                                          │"]);
    }

    #[test]
    fn retry_adds_to_draws_a_genre_slot_and_its_note_names_the_folder_a_song_leaves_out() {
        let _en = english();
        // An album copy pinned to the library default failed — nothing
        // landed, so no In row; the account has saved a layout by genre since.
        let mut gui = copying(TILE, Some("album"), "failed", json!({ "error": "Stage Peer B is unreachable" }), &[]);
        land_read(&mut gui, layout_view("{{GENRE}}/{{ARTIST}} - {{YEAR}}"));
        let rows = draw(&mut gui, 100, 30);
        let low = lower_rows(&rows);
        assert_eq!(
            low[3..7].to_vec(),
            canvas(&[
                "│                                                                          │",
                "│ Retry adds to copies › ‹genre› › Boukmanflow - 2019                      │",
                "│ ‹genre› comes from each song's own tags, so these songs can land in more │",
                "│ than one folder (a song with none leaves that folder out).               │",
            ]),
            "{}",
            low.join("\n")
        );
        assert!(!gui.ui.tips.iter().any(|(_, tip)| tip.contains("own tags")), "the note is on screen, not in a tooltip: {:?}", gui.ui.tips);
        // The legacy console draws the slot in its own brackets.
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("Retry adds to copies > <genre> > Boukmanflow - 2019")), "{}", rows.join("\n"));
    }

    #[test]
    fn an_album_spread_over_the_servers_folders_draws_a_slot_with_its_own_note() {
        // English, but for a turn in Polish: a process of its own.
        let Some(_) = crate::setup::tests::in_own_process(&["en"]) else { return };
        // Mixtape Vol 1 on Stage Peer B sits in each artist's own folder.
        let keep = |layout: &str, songs: Option<&[(&str, &str)]>| {
            let (mut gui, _) = mixtape_gui();
            draw(&mut gui, 100, 30);
            super::open(&mut gui, TILE);
            land_read(&mut gui, layout_view(layout));
            if let Some(songs) = songs {
                land_songs(&mut gui, songs);
            }
            gui
        };
        let place = |text: &str| format!("│ {text:<62}  d change │");
        let lands = |gui: &mut Gui| {
            let low = lower_rows(&draw(gui, 100, 30));
            (low[1].clone(), under_place(&low))
        };
        let two = [("Kim Song", "Peer Kim"), ("Lou Song", "Peer Lou")];
        // Before the listing answers: the tile's row's own folders.
        let mut gui = keep("From friends/{{SERVERFOLDER}}", None);
        assert_eq!(lands(&mut gui), (place("Files go to copies › From friends › Peer Kim › Mixtape Vol 1"), String::new()));
        // The listing puts its songs in two folders: the slot, DIM, and its
        // note on its own rows.
        let mut gui = keep("From friends/{{SERVERFOLDER}}", Some(&two[..]));
        let note = "‹server folder› comes from where each song sits on the server, so these songs can land in more than one folder.";
        assert_eq!(lands(&mut gui), (place("Files go to copies › From friends › ‹server folder›"), note.to_string()));
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(fg(&buffer, at(&rows, "‹server folder›")), th().dim);
        // Beside the tags' slots one sentence names both: the tags first, in
        // the path's order, their number picking the verb, then the server
        // folder; with a genre, the folder a song with none leaves out.
        let mut gui = keep("{{SERVERFOLDER}}/{{ARTIST}}", Some(&two[..]));
        let note = "‹artist› comes from each song's own tags and ‹server folder› from where it sits on the server, so these songs can land in more than one folder.";
        assert_eq!(lands(&mut gui), (place("Files go to copies › ‹server folder› › ‹artist›"), note.to_string()));
        assert!(!gui.ui.tips.iter().any(|(_, tip)| tip == note), "two whole rows: no tooltip");
        // Longer: two rows at most, the second clipped, the whole note both
        // rows' tooltip.
        for (layout, path, note) in [
            ("{{GENRE}}/{{SERVERFOLDER}}", "Files go to copies › ‹genre› › ‹server folder›",
                "‹genre› comes from each song's own tags and ‹server folder› from where it sits on the server, so these songs can land in more than one folder (a song with none leaves that folder out)."),
            ("{{ARTIST}}/{{SERVERFOLDER}}/{{GENRE}}", "Files go to copies › ‹artist› › ‹server folder› › ‹genre›",
                "‹artist›, ‹genre› come from each song's own tags and ‹server folder› from where it sits on the server, so these songs can land in more than one folder (a song with none leaves that folder out)."),
        ] {
            let mut gui = keep(layout, Some(&two[..]));
            let rows = draw(&mut gui, 100, 30);
            let low = lower_rows(&rows);
            assert_eq!(low[1], place(path), "{layout}");
            assert!(low[3].ends_with("… │") && low[4] == format!("│{}│", " ".repeat(74)), "{layout}: {}", low.join("\n"));
            for y in [at_row(&rows, path) + 1, at_row(&rows, path) + 2] {
                assert_eq!(tip_at(&gui, Position { x: 40, y }).as_deref(), Some(note), "{layout}: row {y}");
            }
        }
        // The phone's words in another locale, its plural picked by the
        // tags — past two rows there: the note's own cap, the second row
        // clipped and the whole note both rows' tooltip (§4.5).
        {
            struct English;
            impl Drop for English {
                fn drop(&mut self) {
                    rust_i18n::set_locale("en");
                }
            }
            let _back = English;
            rust_i18n::set_locale("pl");
            let mut gui = keep("{{SERVERFOLDER}}/{{ARTIST}}", Some(&two[..]));
            let note = "‹wykonawca› pochodzi ze znaczników każdego utworu, a ‹folder z serwera› z miejsca, w którym leży on na serwerze, więc te utwory mogą trafić do więcej niż jednego folderu.";
            let rows = draw(&mut gui, 100, 30);
            let y = at_row(&rows, "‹wykonawca› pochodzi");
            let text = |y: u16| rows[usize::from(y)].chars().skip(20).take(74).collect::<String>().trim().to_string();
            assert!(text(y + 1).ends_with('…') && text(y + 2).is_empty(), "{}", rows.join("\n"));
            assert!(note.starts_with(&format!("{} {}", text(y), text(y + 1).trim_end_matches('…'))), "{}", rows.join("\n"));
            for y in [y, y + 1] {
                assert_eq!(tip_at(&gui, Position { x: 40, y }).as_deref(), Some(note), "row {y}");
            }
        }
        // Every song in one folder: that folder, no note.
        let mut gui = keep("From friends/{{SERVERFOLDER}}", Some(&[("Kim Song", "Peer Kim"), ("Kim Two", "Peer Kim")][..]));
        assert_eq!(lands(&mut gui), (place("Files go to copies › From friends › Peer Kim › Mixtape Vol 1"), String::new()));
        // The song: its own folders, inside the folder's words — never a slot.
        let mut gui = keep_on(layout_view("Copy of {{SERVERFOLDER}}"));
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("Files go to copies › Copy of Boukmanflow › Borrowed Tapes")), "{}", rows.join("\n"));
        assert!(!rows.iter().any(|r| r.contains("comes from")));
        // The legacy console's brackets.
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let mut gui = keep("{{SERVERFOLDER}}", Some(&two[..]));
        assert!(draw(&mut gui, 100, 30).iter().any(|r| r.contains("Files go to copies > <server folder>")));
    }

    #[test]
    fn another_servers_live_copy_hides_the_primary_never_greys_it() {
        let _en = english();
        // Stage Peer C copies Borrowed Tapes; Stage Peer B's tile reads so:
        // whose, when Add returns, and the cancel named for it.
        let mut gui = keep_in("O1");
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(
            modal_rows(&rows),
            board(
                "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ (•) Album                                                                │
                │ ( ) Every album by Boukmanflow                                           │
                │     [✓] Only the 1 album you don't have                                  │
                │                                                                          │
                │ Files go to copies › Boukmanflow › Borrowed Tapes               d change │
                │ Copying from Stage Peer C                                                │
                │ Add from Stage Peer B returns when that copy ends                        │
                │                                                                          │
                │ x cancel that copy                                             [ Close ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
                "
            )
        );
        let title = at(&rows, "Copying from");
        assert_eq!((fg(&buffer, title), bold(&buffer, title)), (Color::Reset, true));
        assert_eq!(fg(&buffer, at(&rows, "Add from Stage Peer B")), th().dim);
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ choose · x cancel that copy · d where files land · Esc close");
        // `x` cancels Stage Peer C's copy, once; the word hides while that
        // cancel is out (O1d) and `x` with it.
        key(&mut gui, KeyCode::Char('x'));
        key(&mut gui, KeyCode::Char('x'));
        let cancels: Vec<i64> = gui.pending.iter().filter_map(|e| match e {
            Effect::Api(ApiCmd::JobCancel { id, .. }) => Some(*id),
            _ => None,
        }).collect();
        assert_eq!(cancels, [40]);
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[9], "│                                                                [ Close ] │");
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ choose · d where files land · Esc close");
        assert!(starts(&gui).is_empty(), "no start while it covers the choice");
        // A scope it does not cover brings Add back, the two lines gone:
        // Every album by, the box ticked from the start, takes the missing one.
        key(&mut gui, KeyCode::Down);
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[5..8], [
            "│ Files go to copies › Boukmanflow › Borrowed Tapes               d change │",
            "│                                                                          │",
            "│                                                          [ Add 1 album ] │",
        ]);
        // Enter on Close closes; the tile itself wears none of Stage Peer C's
        // copy. (↑ unticks the box, ↑ again is Album.)
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.app.taste.keep.flow.is_none());
        let rows = draw(&mut gui, 100, 30);
        assert!(room(&rows, 12).starts_with("Boukmanflow"), "{}", rows[12]);
    }

    #[test]
    fn the_modal_at_the_floor_is_centred_with_every_row() {
        let _en = english();
        // F3: the album tile idle at the 100 × 24 floor — rows 3–14, the
        // player bar and its seek line untouched.
        let mut gui = keep_in("C1b");
        let rows = draw(&mut gui, 100, 24);
        assert_eq!((rows[3].chars().nth(19), rows[14].chars().nth(19)), (Some('╭'), Some('╰')), "{}", rows.join("\n"));
        assert_eq!(modal_rows(&rows), modal_rows(&draw(&mut gui, 100, 30)), "nothing gives way");
        assert!(rows[19].starts_with(" 0:00 "), "the seek line: {}", rows[19]);
        // T2: the tallest choosing state with key hints on — 15 rows to
        // have, so B2, B1 and B3 go; rows 3–17, the primary on row 16, the
        // tips line on row 23.
        let mut gui = keep_in("T1");
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        let rows = draw(&mut gui, 100, 24);
        assert_eq!(
            modal_rows(&rows),
            board(
                "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Late Train Verse                                                         │
                │ Boukmanflow · Borrowed Tapes · from Stage Peer B                         │
                │ ( ) This song                                                            │
                │ (•) Album                                                                │
                │ ( ) Every album by Boukmanflow                                           │
                │     [✓] Only the 1 album you don't have                                  │
                │ Files go to copies › from-peers › the long weekend sessions ›            │
                │   Stage Peer B › ‹genre› › Boukmanflow › 2019 - Borrowed Tapes  d change │
                │ ‹genre› comes from each song's own tags, so these songs can land in more │
                │ than one folder (a song with none leaves that folder out).               │
                │ ! Couldn't start: someone else on this server is already getting this —  │
                │   try again in a moment                                                  │
                │                                                  [ Add album · 3 songs ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
                "
            )
        );
        assert_eq!((rows[3].chars().nth(19), rows[17].chars().nth(19)), (Some('╭'), Some('╰')));
        assert_eq!(at(&rows, "[ Add album").y, 16);
        assert!(rows[18].starts_with(" 0:00 "), "the seek line whole: {}", rows[18]);
        assert!(rows[23].starts_with(" ↑ ↓ choose · Enter starts · d where files land · Esc close"), "the tips row: {}", rows[23]);
        // At 100 × 30 nothing gives way: rows 6–23, the blanks back.
        let rows = draw(&mut gui, 100, 30);
        assert_eq!((rows[6].chars().nth(19), rows[23].chars().nth(19)), (Some('╭'), Some('╰')), "{}", rows.join("\n"));
        assert_eq!(modal_rows(&rows).len(), 18);
    }

    #[test]
    fn the_give_way_caps_the_place_rises_then_caps_the_note_and_the_gold_line() {
        let _en = english();
        // T1's 18 rows in 10 under the anchored top, 12 risen (§3.6 steps
        // 2–5): the blanks go, the place keeps one row clipped before `d
        // change`, the top rises, the note and the GOLD line one row each —
        // each clipped row its whole words' tooltip.
        let gui = keep_in("T1");
        let view = gui.app.keep_view().expect("the window");
        let low = super::fitted(&view, super::Shown::default(), 72, 10, 12);
        assert_eq!(super::HEAD_ROWS + 2 + low.len() as u16, 12, "{low:#?}");
        let text = |line: &super::Low| match line {
            super::Low::Place { runs, tip, .. } | super::Low::Text { runs, tip, .. } => {
                (runs.iter().map(|(t, _)| t.as_str()).collect::<String>(), tip.clone())
            }
            other => panic!("not words: {other:?}"),
        };
        assert!(matches!(low[0], super::Low::Choice(_)) && matches!(low[3], super::Low::Missing { .. }), "{low:#?}");
        let (place, tip) = text(&low[4]);
        assert_eq!(place, "Files go to copies › from-peers › the long weekend sessions ›…");
        assert_eq!(tip.as_deref(), Some("copies/from-peers/the long weekend sessions/Stage Peer B/‹genre›/Boukmanflow/2019 - Borrowed Tapes"));
        assert!(matches!(&low[4], super::Low::Place { change: true, .. }), "d change stays on the row");
        let (note, tip) = text(&low[5]);
        assert_eq!(note, "‹genre› comes from each song's own tags, so these songs can land in mor…");
        assert!(tip.is_some_and(|t| t.ends_with("leaves that folder out).")));
        let (gold, tip) = text(&low[6]);
        assert_eq!(gold, "! Couldn't start: someone else on this server is already getting this —…");
        assert!(tip.is_some_and(|t| t.ends_with("try again in a moment")));
        assert!(matches!(low[7], super::Low::Action { .. }));
        // Rows to spare: the steps not taken — under the anchored top, the
        // blanks dropped only as far as needed: B2 alone.
        let low = super::fitted(&view, super::Shown::default(), 72, 17, 19);
        assert_eq!(low.len(), 13, "{low:#?}");
        assert!(matches!(low[0], super::Low::Blank(super::Gap::B1)) && matches!(low[low.len() - 2], super::Low::Blank(super::Gap::B3)));
        assert!(!low.iter().any(|l| matches!(l, super::Low::Blank(super::Gap::B2))));
        // Each step only where the one before it is not enough, in §3.6's
        // order — by the rows each part keeps: the place's, the note's (DIM)
        // and the GOLD line's.
        let kept = |low: &[super::Low]| {
            let places = low.iter().filter(|l| matches!(l, super::Low::Place { .. })).count();
            let words = |colour| {
                low.iter().filter(|l| matches!(l, super::Low::Text { runs, .. } if runs[0].1.fg == Some(colour))).count()
            };
            (places, words(th().dim), words(th().gold))
        };
        let rows = |low: &[super::Low]| super::HEAD_ROWS + 2 + low.len() as u16;
        // The place capped fits under the anchored top: nothing rises.
        let low = super::fitted(&view, super::Shown::default(), 72, 14, 15);
        assert_eq!((rows(&low), kept(&low)), (14, (1, 2, 2)), "{low:#?}");
        // Not even that: the place is capped before the top rises.
        let low = super::fitted(&view, super::Shown::default(), 72, 13, 15);
        assert_eq!((rows(&low), kept(&low)), (14, (1, 2, 2)), "{low:#?}");
        // Risen, the note gives way before the GOLD line.
        let low = super::fitted(&view, super::Shown::default(), 72, 10, 13);
        assert_eq!((rows(&low), kept(&low)), (13, (1, 1, 2)), "{low:#?}");
        // Too tall even at the tightest: the body's rows go from the bottom
        // up — never the action row, always the last.
        let low = super::fitted(&view, super::Shown::default(), 72, 6, 6);
        assert!(rows(&low) <= 6, "{low:#?}");
        assert!(matches!(low.last(), Some(super::Low::Action { .. })), "{low:#?}");
        assert!(matches!(low[0], super::Low::Choice(_)), "{low:#?}");
    }

    #[test]
    fn the_legacy_console_frames_the_modal_in_ascii_and_says_the_bar_in_words() {
        let _en = english();
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let cases = [
            ("C1b", "
                +- Keep a copy ------------------------------------------------------------+
                | Borrowed Tapes                                                           |
                | Boukmanflow · 2019 · from Stage Peer B                                   |
                |                                                                          |
                | (*) Album                                                                |
                | ( ) Every album by Boukmanflow                                           |
                |     [x] Only the 1 album you don't have                                  |
                |                                                                          |
                | Files go to copies > Boukmanflow > Borrowed Tapes               d change |
                |                                                                          |
                |                                                  [ Add album · 3 songs ] |
                +-------------------------------------------------------------- Esc close -+
            "),
            ("L2", "
                +- Keep a copy ------------------------------------------------------------+
                | Borrowed Tapes                                                           |
                | Boukmanflow · 2019 · from Stage Peer B                                   |
                |                                                                          |
                | Copying · song 2 of 3 · Late Train Verse                                 |
                | 52% done                                                                 |
                |                                                                          |
                | Into copies > Boukmanflow > Borrowed Tapes                               |
                |                                                                          |
                | x cancel copy                                                  [ Close ] |
                +--------------------------------------- Esc close · the copy keeps going -+
            "),
            ("L1", "
                +- Keep a copy ------------------------------------------------------------+
                | Borrowed Tapes                                                           |
                | Boukmanflow · 2019 · from Stage Peer B                                   |
                |                                                                          |
                | Waiting for a free slot                                                  |
                | no estimate yet · starts on its own                                      |
                |                                                                          |
                | Into copies > Boukmanflow > Borrowed Tapes                               |
                |                                                                          |
                | x cancel copy                                                  [ Close ] |
                +--------------------------------------- Esc close · the copy keeps going -+
            "),
            ("C9", "
                +- Keep a copy ------------------------------------------------------------+
                | Borrowed Tapes                                                           |
                | Boukmanflow · 2019 · from Stage Peer B                                   |
                |                                                                          |
                | (*) Album                                                                |
                | ( ) Every album by Boukmanflow                                           |
                |     [x] Only the 1 album you don't have                                  |
                |                                                                          |
                | Files go to copies > Boukmanflow > Borrowed Tapes               d change |
                |                                                                          |
                |                                                            [ Adding... ] |
                +-------------------------------------------------------------- Esc close -+
            "),
            ("L5", "
                +- Keep a copy ------------------------------------------------------------+
                | Borrowed Tapes                                                           |
                | Boukmanflow · 2019 · from Stage Peer B                                   |
                |                                                                          |
                | Stopping...                                                              |
                | 52% done                                                                 |
                |                                                                          |
                | Into copies > Boukmanflow > Borrowed Tapes                               |
                |                                                                          |
                |                                                                [ Close ] |
                +-------------------------------------------------------------- Esc close -+
            "),
        ];
        for (id, frame) in cases {
            let mut gui = keep_in(id);
            assert_eq!(window_of(&mut gui), board(frame), "{id}");
        }
        let mut gui = keep_in("C4");
        assert_eq!(window_of(&mut gui)[6], "|     [x] Only the 1 album you don't have                                  |");
        let mut gui = keep_in("E1");
        let shown = window_of(&mut gui);
        assert_eq!(shown[4], "| + In your collection                                                     |");
        assert!(shown[6].contains("In copies > Boukmanflow > Borrowed Tapes > 02 - Late Train Verse.mp3"));
    }

    #[test]
    fn a_start_sends_the_place_lands_in_drew_and_a_changed_one_starts_nothing() {
        let _en = english();
        let mut gui = opened(TILE);
        land_read(&mut gui, dest_view("", "user"));
        draw(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Enter);
        let (ticket, shown) = gui
            .pending
            .iter()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::JobStart { ticket, destination, .. }) => Some((*ticket, destination.clone())),
                _ => None,
            })
            .expect("a start");
        let shown = shown.flatten().expect("the place shown");
        assert_eq!((shown.vpath.as_str(), shown.base.as_str(), shown.layout.as_str()), ("copies", "", "{{ALBUMARTIST}}/{{ALBUM}}"));
        // 409 destination_changed (R2): the place redrawn from the answer,
        // one GOLD line, nothing started; the view is read again.
        gui.pending.clear();
        let music: crate::api::types::CollectionDestination =
            serde_json::from_value(json!({ "vpath": "music", "base": "", "layout": "{{ARTIST}}/{{YEAR}} - {{ALBUM}}", "source": "default" })).unwrap();
        let generation = gui.app.session_gen();
        apply(&mut gui, Event::JobStarted { ticket, generation, answer: Err(crate::api::PluginFailure::DestinationChanged(Some(Some(music)))) });
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::DestinationRead { .. }))), "read again");
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(
            modal_rows(&rows),
            board(
                "
                ╭─ Keep a copy ────────────────────────────────────────────────────────────╮
                │ Borrowed Tapes                                                           │
                │ Boukmanflow · 2019 · from Stage Peer B                                   │
                │                                                                          │
                │ (•) Album                                                                │
                │ ( ) Every album by Boukmanflow                                           │
                │     [✓] Only the 1 album you don't have                                  │
                │                                                                          │
                │ Files go to music › Boukmanflow › 2019 - Borrowed Tapes         d change │
                │ ! Where files land changed · check the path above and add again          │
                │                                                                          │
                │                                                  [ Add album · 3 songs ] │
                ╰────────────────────────────────────────────────────────────── Esc close ─╯
                "
            )
        );
        assert_eq!(fg(&buffer, at(&rows, "! Where files land changed")), th().gold);
        assert_eq!(fg(&buffer, at(&rows, "(•) Album")), th().accent, "the dot where it was");
        assert!(matches!(gui.app.keep_view().unwrap().stage, crate::tui::app::keep::KeepStage::Refused { .. }));
        // Enter adds to the place now drawn.
        key(&mut gui, KeyCode::Enter);
        let again = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::JobStart { destination, .. }) => destination.clone().flatten(),
            _ => None,
        });
        assert_eq!(again.map(|d| d.vpath), Some("music".into()));
    }

    /// The board's one-plain-line frame (R3–R5), its `line` the state's.
    fn right_gone(line: &str) -> Vec<String> {
        canvas(&[
            "╭─ Keep a copy ────────────────────────────────────────────────────────────╮",
            "│ Borrowed Tapes                                                           │",
            "│ Boukmanflow · 2019 · from Stage Peer B                                   │",
            "│                                                                          │",
            &format!("│ {line:<72} │"),
            "│                                                                          │",
            "│                                                                [ Close ] │",
            "╰────────────────────────────────────────────────────────────── Esc close ─╯",
        ])
    }

    #[test]
    fn nowhere_to_put_a_file_leaves_one_plain_line_and_close() {
        let _en = english();
        // R3: a re-read that names nowhere — one TEXT line and Close, 8 rows,
        // the top holding on row 6; every way in leaves the room.
        let mut gui = keep_in("R3");
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(modal_rows(&rows), right_gone("Your account can't add files to this server · copies are off"));
        assert_eq!(rows[6].chars().nth(19), Some('╭'), "the top holds");
        let line = at(&rows, "Your account");
        assert_eq!((fg(&buffer, line), bold(&buffer, line)), (Color::Reset, false));
        let close = at(&rows, "[ Close ]");
        assert_eq!((fg(&buffer, close), bold(&buffer, close)), (th().accent, true));
        assert_eq!(super::super::discover::tips(&gui), "Enter close · Esc close");
        assert!(!gui.app.keep_gate(), "the copy gate closed: every way in hidden");
        for code in [KeyCode::Char('d'), KeyCode::Char('x'), KeyCode::Char('c'), KeyCode::Up] {
            key(&mut gui, code);
        }
        assert!(gui.discover.keep.dest.is_none() && gui.pending.iter().all(|e| !matches!(e, Effect::Api(ApiCmd::JobCancel { .. }))));
        key(&mut gui, KeyCode::Enter);
        assert!(gui.app.taste.keep.flow.is_none());
        // A 403 at start is a right lost, never a refusal with Add still
        // offered: its words name uploading (R3), or the right to start
        // copies is gone (R4); a gate asked again that no longer lists the
        // plug-in (R5).
        for (id, line) in [
            ("R3b", "Your account can't add files to this server · copies are off"),
            ("R4", "Your account can't start copies on this server"),
            ("R5", "Copies from connected servers are off on this server right now"),
        ] {
            let mut gui = keep_in(id);
            assert_eq!(window_of(&mut gui), right_gone(line), "{id}");
            assert_eq!(super::super::discover::tips(&gui), "Enter close · Esc close", "{id}");
        }
        // A 404 first reads as a refusal in the server's words; the gate's
        // answer that the plug-in went turns it R5.
        let mut gui = opened(TILE);
        start(&mut gui, Err(crate::api::PluginFailure::Refused { status: 404, words: "no such plug-in: federation-copy".into() }));
        assert_eq!(lower_rows(&draw(&mut gui, 100, 30))[6], format!("│ {:<72} │", "! Couldn't start: no such plug-in: federation-copy"));
        let mut gone = listing(true);
        gone["plugins"].as_array_mut().expect("plugins").retain(|p| p["name"] != "federation-copy");
        let generation = gui.app.session_gen();
        apply(&mut gui, Event::KeepGate { generation, plugins: Ok(Box::new(serde_json::from_value(gone).unwrap())), destination: None });
        assert_eq!(window_of(&mut gui), right_gone("Copies from connected servers are off on this server right now"));
        // An ended copy whose Retry would start nothing now — the gate asked
        // again finds jobs not allowed: the plain line and Close, never a
        // dead Retry; Enter closes and nothing starts.
        let mut gui = keep_in("E9");
        let generation = gui.app.session_gen();
        apply(&mut gui, Event::KeepGate { generation, plugins: Ok(Box::new(serde_json::from_value(listing(false)).unwrap())), destination: None });
        assert_eq!(window_of(&mut gui), right_gone("Your account can't start copies on this server"));
        assert_eq!(super::super::discover::tips(&gui), "Enter close · Esc close");
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('c'));
        key(&mut gui, KeyCode::Enter);
        assert!(gui.app.taste.keep.flow.is_none() && starts(&gui).is_empty());
    }

    #[test]
    fn lands_in_wraps_a_long_place_at_a_crumb_the_library_always_showing() {
        let _en = english();
        // C8: the Server preset under a base folder of its own — broken
        // before a crumb, ` ›` ending the first row, the rest at col 3 with
        // `d change`; nothing cut, no tooltip.
        let mut gui = keep_in("C8");
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[5..7], [
            "│ Files go to copies › from-peers › 2025 archive › Stage Peer B ›          │",
            "│   Boukmanflow › Borrowed Tapes                                  d change │",
        ], "{}", low.join("\n"));
        assert!(!gui.ui.tips.iter().any(|(_, tip)| tip.contains("2025 archive")), "nothing cut: no tooltip");
        // C8b: longer still — the second row clipped at the right, never the
        // library, and the whole path both rows' tooltip.
        let mut gui = keep_in("C8b");
        let rows = draw(&mut gui, 100, 30);
        let low = lower_rows(&rows);
        assert_eq!(low[5..7], [
            "│ Files go to copies › from-peers › 2025 archive ›                         │",
            "│   the long weekend sessions › Stage Peer B › Boukmanflow › Bo…  d change │",
        ], "{}", low.join("\n"));
        let row = at_row(&rows, "the long weekend sessions");
        let whole = "copies/from-peers/2025 archive/the long weekend sessions/Stage Peer B/Boukmanflow/Borrowed Tapes";
        for y in [row - 1, row] {
            assert_eq!(tip_at(&gui, Position { x: 40, y }).as_deref(), Some(whole), "row {y}");
        }
        assert_eq!(tip_at(&gui, Position { x: 88, y: row }), None, "none on d change");
    }

    #[test]
    fn a_clipped_path_with_a_slot_tips_the_whole_path_and_its_note_has_its_rows() {
        let _en = english();
        // A long base folder and a genre in the layout: the second row is
        // clipped, so its tooltip is the whole path — the slot in its words;
        // the note stands on its own rows, on the place and the PREVIEW.
        let mut view = dest_view("from-peers/2025 archive/the long weekend sessions", "user");
        view["destination"]["layout"] = json!("{{PEER}}/{{GENRE}}/{{ALBUMARTIST}}/{{ALBUM}}");
        view["saved"]["layout"] = view["destination"]["layout"].clone();
        let mut gui = keep_on(view);
        let whole = "copies/from-peers/2025 archive/the long weekend sessions/Stage Peer B/‹genre›/Boukmanflow/Borrowed Tapes";
        for (choice, note) in [
            (crate::tui::app::keep::KeepChoice::Song, "‹genre› comes from the song's own tags (a song with none leaves that folder out)."),
            (
                crate::tui::app::keep::KeepChoice::Album,
                "‹genre› comes from each song's own tags, so these songs can land in more than one folder (a song with none leaves that folder out).",
            ),
        ] {
            gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Pick(choice))));
            let rows = draw(&mut gui, 100, 30);
            let low = lower_rows(&rows);
            let at = low.iter().position(|r| r.contains("Files go to")).expect("the place");
            assert_eq!(low[at..at + 2], [
                "│ Files go to copies › from-peers › 2025 archive ›                         │",
                "│   the long weekend sessions › Stage Peer B › ‹genre› › Boukma…  d change │",
            ], "{}", low.join("\n"));
            let row = at_row(&rows, "the long weekend sessions");
            for y in [row - 1, row] {
                assert_eq!(tip_at(&gui, Position { x: 40, y }).as_deref(), Some(whole), "row {y}");
            }
            assert_eq!(under_place(&low), note);
            // The picker's preview: the base folder folded into Custom's
            // field, the path drawn as the server holds it and clipped
            // from the left after "Files go to" — the whole path its
            // tooltip, no file — the note whole on its own rows, no tooltip.
            key(&mut gui, KeyCode::Char('d'));
            let rows = draw(&mut gui, 100, 30);
            let y = at_row(&rows, "‹genre› › Boukmanflow › Borrowed Tapes");
            assert!(rows[usize::from(y)].contains("│ Files go to …"), "the preview clipped after its lead: {}", rows[usize::from(y)]);
            assert!(gui.ui.tips.iter().any(|(rect, t)| rect.contains(Position { x: 40, y }) && t.as_str() == whole), "preview: {:?}", gui.ui.tips);
            let shown: Vec<String> = [y + 1, y + 2].iter().map(|&y| rows[usize::from(y)].chars().skip(12).take(76).collect::<String>().trim().to_string()).collect();
            assert_eq!(shown.join(" "), note, "{}", rows.join("\n"));
            assert!(!gui.ui.tips.iter().any(|(rect, _)| rect.contains(Position { x: 40, y: y + 1 })), "the note whole: {:?}", gui.ui.tips);
            key(&mut gui, KeyCode::Esc);
        }
    }

    #[test]
    fn a_moved_place_says_where_retry_adds_before_the_press_and_retry_sends_it() {
        let _en = english();
        // E9b: a copy pinned to the library root failed; the account has
        // saved copies › from-peers since (another tab).
        let mut gui = keep_in("E9b");
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(fg(&buffer, at(&rows, "Retry adds to")), th().dim);
        assert!(bold(&buffer, at(&rows, "copies › from-peers")), "the library BOLD");
        drop(buffer);
        // Retry sends the place it said — a stale one is the server's 409 —
        // and the outcome holds, its key words gone, the primary off (E20).
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        let sent = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::JobStart { destination, ticket, .. }) => Some((*ticket, destination.clone())),
            _ => None,
        });
        let (ticket, sent) = sent.expect("a retry");
        let sent = sent.flatten().expect("a place");
        assert_eq!((sent.vpath.as_str(), sent.base.as_str()), ("copies", "from-peers"));
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[1..8], [
            "│ ! Copy failed: Stage Peer B is unreachable                               │",
            "│ Nothing was copied                                                       │",
            "│                                                                          │",
            "│ Retry adds to copies › from-peers › Boukmanflow › Borrowed Tapes         │",
            "│                                                                          │",
            "│                                                            [ Retrying… ] │",
            "╰────────────────────────────────────────────────────────────── Esc close ─╯",
        ], "{}", low.join("\n"));
        assert_eq!(super::super::discover::tips(&gui), "Esc close");
        key(&mut gui, KeyCode::Char('c'));
        key(&mut gui, KeyCode::Enter);
        assert_eq!(starts(&gui).len(), 1, "inert while it is out");
        // The server answers: the copy, "Into" the place sent.
        let mut queued = job(93, &late(), Some("album"), "queued", json!({}));
        queued.params.destination = Some(Some(sent));
        let generation = gui.app.session_gen();
        apply(&mut gui, Event::JobStarted { ticket, generation, answer: Ok(Box::new(crate::api::types::JobStarted { job: queued, created: true })) });
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[4], "│ Into copies › from-peers › Boukmanflow › Borrowed Tapes                  │", "{}", low.join("\n"));
        assert!(!low.iter().any(|r| r.contains("Retry adds to")));
        // Where it has not moved, nothing more is said.
        let mut gui = keep_in("E9");
        land_read(&mut gui, destination());
        assert!(!draw(&mut gui, 100, 30).iter().any(|r| r.contains("adds to")));
        // An album copy with a gap says it the same way.
        let mut gui = keep_in("E5");
        land_read(&mut gui, dest_view("from-peers", "user"));
        assert!(draw(&mut gui, 100, 30).iter().any(|r| r.contains("│ Retry adds to copies › from-peers ›")));
    }

    #[test]
    fn one_folder_per_album_stands_dim_in_lands_in_the_library_bold_the_names_text() {
        let _en = english();
        // A running artist copy reads its pinned place, album by album.
        let mut gui = copying(TILE, Some("artist"), "running", json!({ "progress": 0.2 }), &[]);
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let crumbs = at(&rows, "copies › Boukmanflow › one folder per album");
        let cell = |dx: u16| Position { x: crumbs.x + dx, y: crumbs.y };
        assert!(bold(&buffer, cell(0)), "the library BOLD");
        assert_eq!(fg(&buffer, cell(7)), th().dim, "› DIM");
        assert!(fg(&buffer, cell(9)) != th().dim && !bold(&buffer, cell(9)), "the artist TEXT");
        for dx in 23..43 {
            assert_eq!(fg(&buffer, cell(dx)), th().dim, "one folder per album DIM at +{dx}");
        }
        // An album copy's real album stays TEXT.
        let mut gui = keep_in("E3");
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let crumbs = at(&lines(&buffer), "copies › Boukmanflow › Borrowed Tapes");
        let album = Position { x: crumbs.x + 23, y: crumbs.y };
        assert!(fg(&buffer, album) != th().dim && !bold(&buffer, album), "Borrowed Tapes TEXT");
    }

    #[test]
    fn the_band_clears_the_rooms_width_on_the_windows_rows() {
        let _en = english();
        // Nothing of the room shows beside the window: the band clears x
        // 17–96 on its rows; the rows above and below stay the room's.
        let mut gui = canvas_gui(&canvas_jobs());
        let before = draw(&mut gui, 100, 30);
        super::open(&mut gui, TILE);
        board_facts(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        let modal = modal_rows(&rows);
        for (i, y) in (6..6 + modal.len()).enumerate() {
            let band: String = rows[y].chars().skip(17).take(80).collect();
            assert_eq!(band, format!("  {}  ", modal[i]), "row {y}");
        }
        let below = 6 + modal.len();
        assert_eq!(rows[5], before[5], "the row above is the room's");
        assert_eq!(rows[below], before[below], "and the row below");
        assert_eq!(hit_at(&gui, 17, 10), Some(Act::Guard));
        assert_eq!(hit_at(&gui, 96, 10), Some(Act::Guard));
        // The queue open leaves the room too narrow: the window centres over
        // the frame, x 12–87, and no band.
        gui.queue_open = true;
        let rows = draw(&mut gui, 100, 30);
        assert_eq!((rows[6].chars().nth(12), rows[6].chars().nth(87)), (Some('╭'), Some('╮')), "{}", rows.join("\n"));
        assert!(rows[7].starts_with("   Artists  │"), "the nav beside it: {}", rows[7]);
    }

    #[test]
    fn the_indicator_is_not_drawn_while_keep_a_copy_or_the_copies_list_is_open() {
        let _en = english();
        // A tall window: the verbs line stands above the window's rows.
        let mut gui = canvas_gui(&canvas_jobs());
        gui.ui.key_hints = true;
        let rows = draw(&mut gui, 100, 60);
        let verbs = rows.iter().position(|r| r.trim_end().ends_with("copying… · ! failed")).expect("the indicator");
        let indicated = |gui: &Gui| gui.ui.tips.iter().any(|(_, tip)| tip == "Copies — C");
        assert!(indicated(&gui));
        // Keep a copy speaks for the copy alone.
        super::open(&mut gui, KeepTarget::Row { group: 0, row: 1 });
        let rows = draw(&mut gui, 100, 60);
        let top = rows.iter().position(|r| r.contains("─ Keep a copy ─")).expect("the window");
        assert!(top > verbs, "the window under the verbs line");
        assert!(!rows[verbs].contains("copying…") && !rows[verbs].contains("failed"), "{}", rows[verbs]);
        assert!(!indicated(&gui));
        // `C` does nothing while Keep a copy is open.
        key(&mut gui, KeyCode::Char('C'));
        assert!(gui.discover.copies.is_none());
        key(&mut gui, KeyCode::Esc);
        // Nor while the Copies list is.
        gui.act(Act::Disc(DiscAct::Copies));
        let rows = draw(&mut gui, 100, 60);
        assert!(!rows[verbs].contains("copying…"), "{}", rows[verbs]);
        assert!(!indicated(&gui));
        gui.act(Act::Disc(DiscAct::CopiesClose));
        assert!(draw(&mut gui, 100, 60)[verbs].trim_end().ends_with("copying… · ! failed"));
    }

    #[test]
    fn no_key_acts_without_its_word() {
        let _en = english();
        // Each state's letters: only the ones its words name do anything.
        for (id, dead) in [
            ("L2", "dac"),
            ("L5", "xdac"),
            ("E3", "adx"),
            ("E12", "cadx"),
            ("E13", "cadx"),
            ("E21", "cadx"),
            ("E9", "adx"),
            ("O1d", "xac"),
            ("R3", "dxac"),
        ] {
            let mut gui = keep_in(id);
            let before = window_of(&mut gui);
            gui.pending.clear();
            for c in dead.chars() {
                key(&mut gui, KeyCode::Char(c));
            }
            for code in [KeyCode::Char(' '), KeyCode::Tab, KeyCode::Left, KeyCode::Right, KeyCode::Char('C')] {
                key(&mut gui, code);
            }
            assert!(gui.app.taste.keep.flow.is_some(), "{id}: still open");
            assert_eq!(window_of(&mut gui), before, "{id}");
            let acted = gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::JobCancel { .. } | ApiCmd::JobStart { .. } | ApiCmd::DestinationSave { .. })));
            assert!(!acted && gui.discover.keep.dest.is_none() && gui.discover.copies.is_none(), "{id}: {:?}", gui.pending);
        }
        // And the ones they name do: `x` cancels the copy, `c` goes back to
        // the choice, `a` queues the song, `d` opens the picker.
        let mut gui = keep_in("L2");
        key(&mut gui, KeyCode::Char('x'));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::JobCancel { id: 91, .. }))));
        let mut gui = keep_in("E7");
        key(&mut gui, KeyCode::Char('d'));
        assert!(gui.discover.keep.dest.is_some());
        let mut gui = keep_in("E1");
        key(&mut gui, KeyCode::Char('a'));
        assert_eq!(gui.app.queue.items.last().map(|q| q.filepath.as_str()), Some("copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3"));
        assert!(gui.app.taste.keep.flow.is_none(), "queued, closed");
    }

    #[test]
    fn a_file_in_the_way_offers_close_then_retry_once_the_place_changed() {
        let _en = english();
        // E7: Retry would meet the same file — Close; `d change` on the action
        // row opens Where copies land.
        let mut gui = keep_in("E7");
        land_read(&mut gui, dest_view("", "user"));
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(modal_rows(&rows), modal_rows(&draw(&mut keep_in("E7"), 100, 30)), "the same place: still E7");
        assert_eq!(gui.ui.hit(at(&rows, "d change")), keep_act(super::KeepAct::Key(super::KeyWord::Change)));
        key(&mut gui, KeyCode::Char('d'));
        land_folders(&mut gui);
        // Saved elsewhere (the Server preset): E8 — Retry, and where it adds.
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Server));
        dest_act(&mut gui, super::DestAct::Save);
        land_save(&mut gui, Ok(place_view("", "{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}")));
        draw(&mut gui, 100, 30);
        assert!(gui.discover.keep.dest.is_none(), "saved: back in Keep a copy");
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[4..9], [
            "│ In copies › Boukmanflow › Borrowed Tapes › 02 - Late Train Verse.mp3     │",
            "│ Retry adds to copies › Stage Peer B › Boukmanflow › Borrowed Tapes       │",
            "│                                                                          │",
            "│ d change   c choose again                                      [ Retry ] │",
            "╰────────────────────────────────────────────────────────────── Esc close ─╯",
        ], "{}", low.join("\n"));
        // Retry sends the place it says.
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        let sent = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::JobStart { destination, .. }) => destination.clone().flatten(),
            _ => None,
        });
        assert_eq!(sent.map(|d| d.layout), Some("{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}".into()));
        // E7's Enter closes: nothing to retry into.
        let mut gui = keep_in("E7");
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert!(gui.app.taste.keep.flow.is_none() && starts(&gui).is_empty());
    }

    #[test]
    fn a_gap_is_the_headline_and_retry_takes_it() {
        let _en = english();
        // Done ≠ kept, made visible: GOLD BOLD "! 1 song didn't copy" over
        // DIM counts with no GOLD in them.
        let mut gui = keep_in("E5");
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        for x in 21..29 {
            assert_eq!(fg(&buffer, Position { x, y: 11 }), th().dim, "the counts DIM at {x}");
        }
        assert_eq!(at(&rows, "! 1 song didn't copy").y, 10);
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert_eq!(starts(&gui), vec![(late(), crate::api::types::JobScope::Album)], "Retry: the album's copy again");
    }

    #[test]
    fn play_your_copy_after_an_album_plays_the_album_from_the_library() {
        let _en = english();
        // E3: two copied, one already yours — the whole album from the
        // library, in the album's order, the first started; the window closes.
        let mut gui = keep_in("E3");
        key(&mut gui, KeyCode::Enter);
        let queued: Vec<&str> = gui.app.queue.items.iter().map(|q| q.filepath.as_str()).collect();
        assert_eq!(queued, ["copies/Boukmanflow/Borrowed Tapes/01.mp3", "copies/Boukmanflow/Borrowed Tapes/02.mp3", "music/Boukmanflow/Borrowed Tapes/03.mp3"]);
        assert_eq!(gui.app.queue.current, Some(0));
        assert!(gui.app.queue.items.iter().all(|q| q.origin.peer.is_none()), "the user's own library");
        assert!(gui.app.taste.keep.flow.is_none());
        // Every album by is no one queue: Close.
        let mut gui = keep_in("E4");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.app.queue.items.is_empty() && gui.app.taste.keep.flow.is_none());
    }

    #[test]
    fn c_choose_again_returns_to_the_choice_where_it_stood() {
        let _en = english();
        // E1 → `c`: the song's choice, the dot on This song, the place the
        // account's now — C5.
        let mut gui = keep_in("E1");
        key(&mut gui, KeyCode::Char('c'));
        assert_eq!(window_of(&mut gui), modal_rows(&draw(&mut keep_in("C5"), 100, 30)));
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ choose · Enter starts · d where files land · Esc close");
        // E3 → a click on the words: the tile's choice, the dot on Album.
        let mut gui = keep_in("E3");
        let rows = draw(&mut gui, 100, 30);
        let words = at(&rows, "c choose again");
        assert_eq!(gui.ui.hit(words), keep_act(super::KeepAct::Key(super::KeyWord::Choose)));
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Key(super::KeyWord::Choose))));
        assert_eq!(window_of(&mut gui), modal_rows(&draw(&mut keep_in("C1b"), 100, 30)));
        // Add again after a cancel brings the choice back too, and starts nothing.
        let mut gui = keep_in("E13");
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert_eq!(window_of(&mut gui), modal_rows(&draw(&mut keep_in("C1b"), 100, 30)));
        assert!(starts(&gui).is_empty());
    }

    #[test]
    fn the_cover_is_four_by_two_pixels_or_nothing() {
        use ratatui_image::picker::ProtocolType;
        let _en = english();
        let png = image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        png.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let art = crate::tui::art::decode(&bytes.into_inner()).unwrap();
        let with_art = |pixels: bool| {
            let mut gui = keep_in("C1b");
            if pixels {
                gui.app.graphics = crate::tui::graphics::Graphics::forced(ProtocolType::Kitty);
            }
            gui.app.art.insert("bt.jpg".into(), Some(art.clone()));
            if let Some(flow) = gui.app.taste.keep.flow.as_mut() {
                flow.subject.art = Some("bt.jpg".into());
            }
            gui
        };
        // Pixels where the terminal draws them: inner cols 69–72 of the
        // head's two rows (x 89–92, rows 7–8).
        let mut gui = with_art(true);
        let buffer = buffer(&mut gui, 100, 30);
        // Found by its top-left cell, which carries the picture's
        // placeholders (a kitty cell's symbol defeats string positions).
        let pixel = |x: u16, y: u16| buffer[(x, y)].symbol().contains('\u{10EEEE}');
        assert!(pixel(89, 7), "the cover is pixels: {:?}", buffer[(89, 7)].symbol());
        assert!(!pixel(88, 7) && !pixel(89, 9), "4 × 2, at the head's right end");
        assert_eq!(lines(&buffer)[7].chars().skip(21).take(14).collect::<String>(), "Borrowed Tapes");
        // Otherwise nothing: no mosaic, no empty frame — the words have the
        // whole row.
        let mut gui = with_art(false);
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(rows[7].chars().skip(84).take(11).collect::<String>(), "          │", "{}", rows[7]);
        assert_eq!(rows[8].chars().skip(84).take(11).collect::<String>(), "          │");
        assert!(gui.discover.keep.slot.is_none(), "no slot drawn at all");
    }

    #[test]
    fn key_words_that_would_crowd_the_primary_take_the_row_above() {
        let _en = english();
        // Not reached in English or German at 76 (§4.8); a narrower room
        // shows the rule.
        let mut low = Vec::new();
        let primary = super::Primary { words: "Play your copy".into(), press: Some(super::Press::Again(crate::api::types::JobAction::Play)) };
        super::action(&mut low, vec![super::KeyWord::Queue, super::KeyWord::Choose], primary.clone(), 50);
        assert_eq!(low, vec![
            super::Low::Keys(vec![super::KeyWord::Queue, super::KeyWord::Choose]),
            super::Low::Action { keys: Vec::new(), primary: primary.clone() },
        ]);
        let mut low = Vec::new();
        super::action(&mut low, vec![super::KeyWord::Queue, super::KeyWord::Choose], primary.clone(), 72);
        assert_eq!(low, vec![super::Low::Action { keys: vec![super::KeyWord::Queue, super::KeyWord::Choose], primary }]);
    }

    #[test]
    fn the_window_and_the_copies_list_open_in_the_same_place() {
        let _en = english();
        let late = late();
        let mut gui = copies_gui(&[job(40, &late, Some("album"), "running", json!({ "progress": 0.52 }))]);
        let rows = draw(&mut gui, 100, 30);
        let x = rows[6].chars().position(|c| c == '╭');
        assert_eq!(x, Some(19), "{}", rows.join("\n"));
        assert_eq!(rows[6].chars().nth(94), Some('╮'));
        let band: String = rows[8].chars().skip(17).take(2).chain(rows[8].chars().skip(95).take(2)).collect();
        assert_eq!(band, "    ", "the band: {}", rows[8]);
        assert_eq!(hit_at(&gui, 17, 8), Some(Act::Guard));
    }

    /// The tooltip that ripens with the pointer resting at `at`: the frame
    /// drawn, the dwell ticked and aged past the delay — the shell's own
    /// order, which takes the first tip registered under the pointer.
    fn ripe(gui: &mut Gui, at: Position) -> Option<String> {
        gui.ui.pointer = Some(at);
        draw(gui, 100, 30);
        gui.ui.dwell_tick();
        gui.ui.dwell_backdate(crate::kit::TIP_DELAY);
        let tip = gui.ui.ripe_tooltip().map(|(_, text)| text.to_string());
        gui.ui.pointer = None;
        tip
    }

    #[test]
    fn the_room_under_the_window_neither_lights_nor_tips_and_takes_no_hand() {
        let _en = english();
        // C8b: the pointer on the clipped place ripens the place's own
        // tooltip, the whole path — never a tip the room drew beneath it.
        let mut gui = keep_in("C8b");
        let rows = draw(&mut gui, 100, 30);
        let row = at_row(&rows, "Files go to copies › from-peers");
        let whole = "copies/from-peers/2025 archive/the long weekend sessions/Stage Peer B/Boukmanflow/Borrowed Tapes";
        for x in [25, 40, 60] {
            assert_eq!(ripe(&mut gui, Position { x, y: row }).as_deref(), Some(whole), "x {x}");
        }
        // The footer, a blank row, the band and the header above: none.
        let foot = at(&rows, "Esc close");
        for at in [Position { x: foot.x + 2, y: foot.y }, Position { x: 40, y: row + 2 }, Position { x: 18, y: row }, at(&rows, "Last 30 days")] {
            assert_eq!(ripe(&mut gui, at), None, "{at:?}");
        }
        // The header's dropdown, under the pointer, stays unlit.
        let header = at(&rows, "Last 30 days");
        gui.ui.pointer = Some(header);
        let buffer = buffer(&mut gui, 100, 30);
        assert_ne!(fg(&buffer, header), th().bright, "the room beneath sees no pointer");
        // C1b: a table row below the window keeps its columns under the
        // pointer — no hover verbs — and the hand is an arrow there (and on
        // the band), the hand on the window's own controls.
        let mut gui = canvas_gui(&[]);
        gui.ui.pointer = Some(Position { x: 40, y: 20 });
        let before = draw(&mut gui, 100, 30);
        assert!(before[20].contains("[⋯]"), "the row lights without the window: {}", before[20]);
        super::open(&mut gui, TILE);
        board_facts(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        assert!(!rows[20].contains("[⋯]") && !rows[20].contains("[▸]"), "{}", rows[20]);
        assert!(!super::super::over_control(&gui), "no hand over the room");
        gui.ui.pointer = Some(Position { x: 18, y: 10 });
        draw(&mut gui, 100, 30);
        assert!(!super::super::over_control(&gui), "no hand over the band");
        gui.ui.pointer = Some(at(&rows, "[ Add album"));
        draw(&mut gui, 100, 30);
        assert!(super::super::over_control(&gui), "the hand on the primary");
        // The Copies list: the same.
        let mut gui = copies_gui(&copies_jobs());
        gui.ui.pointer = Some(Position { x: 18, y: 10 });
        draw(&mut gui, 100, 30);
        assert!(!super::super::over_control(&gui));
        assert_eq!(ripe(&mut gui, Position { x: 18, y: 10 }), None);
    }

    #[test]
    fn the_box_kept_by_a_copy_alone_says_no_count() {
        let _en = english();
        // A failed only-missing copy of this server still wants the user, so
        // the box stays offered — not while the sizes are asked, and, with
        // nothing missing by name, with no count: never "the 0 albums".
        let failed = job(80, &late(), Some("artist-missing"), "failed", json!({ "error": "Stage Peer B is unreachable" }));
        let mut gui = canvas_gui(&[failed]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, TILE);
        let window = window_of(&mut gui);
        assert!(!window.iter().any(|r| r.contains("Only the")), "{}", window.join("\n"));
        land_facts(&mut gui, Some(3), &["Borrowed Tapes"], &["Borrowed Tapes", "Kite Days"]);
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[3], "│     [✓] Only the albums you don't have                                   │", "{}", low.join("\n"));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        let window = window_of(&mut gui);
        assert!(window.iter().any(|r| r.contains("[✓] Only the albums you don't have")), "{}", window.join("\n"));
        assert!(window.iter().any(|r| r.ends_with("[ Add the albums you don't have ] │")), "{}", window.join("\n"));
        assert!(!window.iter().any(|r| r.contains(" 0 ")), "{}", window.join("\n"));
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert_eq!(starts(&gui), vec![(late(), crate::api::types::JobScope::ArtistMissing)]);
    }

    #[test]
    fn a_clipped_step_tips_its_whole_words() {
        let _en = english();
        // A running copy's step is the server's words, on one row: clipped,
        // the row's tooltip is all of them — never the clipped row again.
        let step = "album 1 of 2 · The Extremely Long Album Title Of The Night Sessions · song 12 of 14 · Late Train Verse";
        let mut gui = copying(TILE, Some("artist"), "running", json!({ "progress": 0.3, "statusText": step }), &[]);
        let rows = draw(&mut gui, 100, 30);
        let y = at_row(&rows, "Copying · album 1 of 2");
        assert!(rows[usize::from(y)].contains("Night Se…"), "{}", rows[usize::from(y)]);
        assert_eq!(tip_at(&gui, Position { x: 40, y }), Some(format!("Copying · {step}")));
        assert_eq!(ripe(&mut gui, Position { x: 40, y }), Some(format!("Copying · {step}")));
    }

    #[test]
    fn a_japanese_or_chinese_note_fills_its_rows() {
        // Their sentences have no blank to break at: the note's first row
        // carries on after the slot, by cells, and the whole note stands
        // on its two rows — or, cut, is their tooltip.
        let Some(code) = crate::setup::tests::in_own_process(&["ja", "zh"]) else { return };
        let mut gui = keep_on(place_view("", "{{GENRE}}/{{ALBUMARTIST}}/{{ALBUM}}"));
        choose(&mut gui, Some("album"));
        let view = gui.app.keep_view().expect("the window");
        let note = super::slot_note(view.landing.as_ref().expect("a place")).expect("a slot's note");
        let low = super::lower(&view, super::Shown::default(), 72, super::Fit::default());
        let rows: Vec<(String, Option<String>)> = low
            .iter()
            .filter_map(|line| match line {
                super::Low::Text { runs, tip, .. } => Some((runs.iter().map(|(t, _)| t.as_str()).collect(), tip.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(rows.len(), 2, "{code}: {rows:?}");
        let slot = format!("‹{}›", rust_i18n::t!("gui.discover.dest.slot_genre"));
        assert!(crate::kit::width(&rows[0].0) > crate::kit::width(&slot) + 2, "{code}: the first row filled: {rows:?}");
        assert!(rows.iter().all(|(row, _)| crate::kit::width(row) <= 72), "{code}: {rows:?}");
        match &rows[0].1 {
            None => assert_eq!(format!("{}{}", rows[0].0, rows[1].0).replace(' ', ""), note.replace(' ', ""), "{code}"),
            Some(tip) => assert!(tip == &note && rows[1].0.ends_with('…'), "{code}: {rows:?}"),
        }
    }

    #[test]
    fn esc_while_adding_still_says_how_the_start_went() {
        let _en = english();
        // C9 → Esc: the start lands all the same, and the status line says
        // what came of it, the window that would have being gone (clause
        // 118).
        let answered = |answer: Result<PluginJob, crate::api::PluginFailure>| {
            let mut gui = keep_in("C9");
            let ticket = gui.pending.iter().rev().find_map(|e| match e {
                Effect::Api(ApiCmd::JobStart { ticket, .. }) => Some(*ticket),
                _ => None,
            });
            key(&mut gui, KeyCode::Esc);
            assert!(gui.app.taste.keep.flow.is_none(), "Esc closes while it is out");
            draw(&mut gui, 100, 30);
            assert!(gui.app.message.is_none(), "nothing yet: {:?}", gui.app.message);
            let generation = gui.app.session_gen();
            let answer = answer.map(|job| Box::new(crate::api::types::JobStarted { job, created: true }));
            apply(&mut gui, Event::JobStarted { ticket: ticket.expect("a start"), generation, answer });
            draw(&mut gui, 100, 30);
            gui.app.message.clone().map(|m| (m.text, m.kind))
        };
        use crate::tui::app::MessageKind;
        assert_eq!(answered(Ok(job(92, &late(), Some("album"), "queued", json!({})))), Some(("copying Borrowed Tapes…".into(), MessageKind::Busy)));
        // Refused — in the server's words, as a start from the Copies list
        // is; the note's cells clip them, the window being gone.
        assert_eq!(
            answered(Err(crate::api::PluginFailure::Refused { status: 429, words: "too many".into() })),
            Some(("couldn't start: too many".into(), MessageKind::Error))
        );
        assert_eq!(
            answered(Err(crate::api::PluginFailure::Refused { status: 403, words: "you have 20 jobs queued or running".into() })),
            Some(("couldn't start: you have 20 j…".into(), MessageKind::Error))
        );
    }

    #[test]
    fn the_cover_holds_its_pixels_while_the_window_grows_and_its_title_stops_short_of_it() {
        use ratatui_image::picker::ProtocolType;
        let _en = english();
        let png = image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        png.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let art = crate::tui::art::decode(&bytes.into_inner()).unwrap();
        // C2 → C1b: the sizes land and the window grows a row; its own
        // footprint, last frame's too, is never an overlay over the cover —
        // no ▀ frame between.
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, TILE);
        gui.app.graphics = crate::tui::graphics::Graphics::forced(ProtocolType::Kitty);
        gui.app.art.insert("bt.jpg".into(), Some(art));
        let long = "The Extremely Long Album Title Of The Night Sessions And The Morning After";
        if let Some(flow) = gui.app.taste.keep.flow.as_mut() {
            flow.subject.art = Some("bt.jpg".into());
            flow.subject.album = Some(long.into());
        }
        let pixel = |buffer: &Buffer| buffer[(89, 7)].symbol().contains('\u{10EEEE}');
        let first = buffer(&mut gui, 100, 30);
        assert!(pixel(&first), "pixels: {:?}", first[(89, 7)].symbol());
        let height = modal_rows(&lines(&first)).len();
        board_facts(&mut gui);
        let grown = buffer(&mut gui, 100, 30);
        assert_eq!(modal_rows(&lines(&grown)).len(), height + 1, "the box came in");
        assert!(pixel(&grown), "still pixels after the window grew: {:?}", grown[(89, 7)].symbol());
        // Beside the cover the title has 67 cells: clipped at x 87, a blank
        // before the picture.
        assert_eq!((grown[(87, 7)].symbol(), grown[(88, 7)].symbol()), ("…", " "));
        assert_eq!(grown[(21, 7)].symbol(), "T");
    }

    #[test]
    fn a_key_acts_only_where_the_frame_drew_its_word() {
        let _en = english();
        // E7 drew `d change` and `c choose again`. A frame that drew one of
        // them only (the give-way's last resort took the other's row) leaves
        // the other inert, its tip gone too.
        let mut gui = keep_in("E7");
        draw(&mut gui, 100, 30);
        let (view, shown, keys) = gui.discover.keep.drawn.clone().expect("the frame drew");
        assert_eq!(keys, [super::KeyWord::Change, super::KeyWord::Choose]);
        gui.discover.keep.drawn = Some((view, shown, vec![super::KeyWord::Choose]));
        assert_eq!(super::super::discover::tips(&gui), "Enter close · c choose again · Esc close");
        key(&mut gui, KeyCode::Char('d'));
        assert!(gui.discover.keep.dest.is_none(), "d's word was not drawn");
        key(&mut gui, KeyCode::Char('c'));
        assert!(matches!(gui.app.keep_view().unwrap().stage, super::KeepStage::Choose { .. }));
        // A view the frame did not draw (the state moved since): the table's
        // own words.
        let mut gui = keep_in("E7");
        draw(&mut gui, 100, 30);
        let stale = keep_in("E1").app.keep_view().unwrap();
        gui.discover.keep.drawn = Some((stale, super::Shown::default(), Vec::new()));
        key(&mut gui, KeyCode::Char('d'));
        assert!(gui.discover.keep.dest.is_some());
    }

    #[test]
    fn another_servers_copy_a_start_answered_with_leaves_the_choice_movable() {
        let _en = english();
        // Add album answered `200 created: false` with Stage Peer C's live
        // copy: O1, as a lookup that found it first draws it — and ↓ to a
        // scope it does not cover brings Add back, the two lines gone.
        let mut gui = opened(TILE);
        let rooftop = fed(C, "Rooftop Cypher", "Boukmanflow", "Borrowed Tapes", 0.6, Some(12.0));
        let theirs = job(40, &rooftop, Some("album"), "running", json!({ "progress": 0.3 }));
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Primary)));
        let ticket = gui.pending.iter().rev().find_map(|e| match e {
            Effect::Api(ApiCmd::JobStart { ticket, .. }) => Some(*ticket),
            _ => None,
        });
        let generation = gui.app.session_gen();
        let answer = Ok(Box::new(crate::api::types::JobStarted { job: theirs, created: false }));
        apply(&mut gui, Event::JobStarted { ticket: ticket.expect("a start"), generation, answer });
        assert_eq!(window_of(&mut gui), modal_rows(&draw(&mut keep_in("O1"), 100, 30)));
        key(&mut gui, KeyCode::Down);
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[5..8], [
            "│ Files go to copies › Boukmanflow › Borrowed Tapes               d change │",
            "│                                                                          │",
            "│                                                          [ Add 1 album ] │",
        ], "{}", low.join("\n"));
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Up);
        assert!(window_of(&mut gui).iter().any(|r| r.contains("Copying from Stage Peer C")));
    }

    #[test]
    fn under_where_copies_land_no_tooltip_of_the_rooms_stands() {
        let _en = english();
        // The picker in Keep a copy's place: every tooltip registered is
        // its own, inside its frame — none of the room's to ripen first.
        let mut gui = keep_in("C1b");
        key(&mut gui, KeyCode::Char('d'));
        land_folders(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        let top = rows.iter().position(|r| r.contains("─ Where copies land ─")).expect("the picker") as u16;
        let frame = ratatui::layout::Rect { x: 10, y: top, width: super::DEST_W, height: picker_rows(&rows).len() as u16 };
        assert!(gui.ui.tips.iter().all(|(rect, _)| frame.contains(Position { x: rect.x, y: rect.y })), "{:?}", gui.ui.tips);
        assert_eq!(ripe(&mut gui, Position { x: 2, y: 9 }), None, "the nav beneath");
    }

    #[test]
    fn whose_copy_spoke_follows_a_sub_lowercase_and_stands_alone_raised() {
        let _en = english();
        let mut view = keep_in("E18").app.keep_view().expect("the window");
        assert_eq!(view.borrowed, Some(crate::api::types::JobScope::Album));
        assert_eq!(super::with_borrowed(None, &view).as_deref(), Some("With your album copy"));
        assert_eq!(
            super::with_borrowed(Some("Your library already has this song".into()), &view).as_deref(),
            Some("Your library already has this song · with your album copy")
        );
        view.borrowed = Some(crate::api::types::JobScope::ArtistMissing);
        assert_eq!(super::with_borrowed(Some("3 copied".into()), &view).as_deref(), Some("3 copied · with your copy of Boukmanflow's missing albums"));
    }

    #[test]
    fn an_ended_tips_line_gives_d_way_before_c() {
        let _en = english();
        let keys = [super::KeyWord::Change, super::KeyWord::Choose];
        assert_eq!(super::ended_tips("close", &keys), "Enter close · d where files land · c choose again · Esc close");
        // Past the footer's 99 cells: d goes first…
        let line = super::ended_tips(&"x".repeat(60), &keys);
        assert!(line.contains("c choose again") && !line.contains("d where files land"), "{line}");
        // …then c.
        let line = super::ended_tips(&"x".repeat(75), &keys);
        assert!(!line.contains("c choose again") && !line.contains("d where files land"), "{line}");
    }

    fn counts(copied: u64, skipped: u64, failed: u64, stopped: Option<&str>) -> Value {
        json!({ "result": { "scope": "album", "counts": { "total": 3, "copied": copied, "skipped": skipped, "failed": failed }, "stopped": stopped } })
    }

    /// The room on Connected servers with one compilation, Mixtape Vol 1
    /// (credited to Various Artists): Peer Kim's song, its best row, and
    /// Peer Lou's.
    fn mixtape_gui() -> (Gui, Value) {
        let mut gui = plugin_gui();
        key(&mut gui, KeyCode::Char('G'));
        let (seq, generation) = pending_taste(&gui, TastePart::Main);
        apply(&mut gui, Event::Taste { seq, generation, part: TastePart::Main, answer: Ok(Box::new(main_answer())) });
        let mut kim = fed(B, "Kim Song", "Peer Kim", "Mixtape Vol 1", 0.9, Some(30.0));
        kim["albumArtist"] = json!("Various Artists");
        let mut lou = fed(B, "Lou Song", "Peer Lou", "Mixtape Vol 1", 0.8, Some(40.0));
        lou["albumArtist"] = json!("Various Artists");
        let answer: TasteResponse = serde_json::from_value(json!({
            "seedSource": "history", "seeds": [], "library": null, "network": null,
            "peers": { "peers": [ { "id": B, "name": name(B), "reachable": true, "results": [kim.clone(), lou] } ],
                       "searched": { "peers": 1 } }
        }))
        .unwrap();
        let (seq, generation) = pending_taste(&gui, TastePart::Peers);
        apply(&mut gui, Event::Taste { seq, generation, part: TastePart::Peers, answer: Ok(Box::new(answer)) });
        let generation = gui.app.session_gen();
        apply(
            &mut gui,
            Event::KeepGate {
                generation,
                plugins: Ok(Box::new(serde_json::from_value(listing(true)).unwrap())),
                destination: Some(Ok(Box::new(serde_json::from_value(destination()).unwrap()))),
            },
        );
        land_lookups(&mut gui, &[]);
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        (gui, kim)
    }

    /// The album's listing, landed with these songs' own tags.
    fn land_songs(gui: &mut Gui, songs: &[(&str, &str)]) {
        let ask = gui.pending.iter().rev().find_map(|e| match e {
            Effect::Api(ApiCmd::KeepFacts { seq, generation, .. }) => Some((*seq, *generation)),
            _ => None,
        });
        let Some((seq, generation)) = ask else { panic!("no facts ask in {:?}", gui.pending) };
        let songs = songs
            .iter()
            .map(|(title, artist)| crate::tui::worker::KeepSong {
                filepath: format!("peerlib/{artist}/Mixtape Vol 1/{title}.mp3"),
                artist: Some(artist.to_string()),
                year: Some(2019),
                genre: None,
            })
            .collect();
        let facts = crate::tui::worker::KeepFacts { album_songs: Some(songs), ..Default::default() };
        apply(gui, Event::KeepFacts { seq, generation, facts: Box::new(facts) });
    }

    /// A place saved for the account with `layout`.
    fn layout_view(layout: &str) -> Value {
        let mut view = dest_view("", "user");
        view["destination"]["layout"] = json!(layout);
        view["saved"]["layout"] = json!(layout);
        view
    }

    // ── Status notes (clause 118) ───────────────────────────────────────

    #[test]
    fn a_copy_that_ends_with_its_modal_closed_leaves_a_note_in_its_token() {
        let _en = english();
        let late = fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0));
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        start(&mut gui, Ok(job(90, &late, None, "running", json!({}))));
        // The open modal speaks for itself: no note.
        draw(&mut gui, 100, 30);
        assert!(gui.app.message.is_none());
        // Closed while it runs: the busy note, ACCENT.
        key(&mut gui, KeyCode::Esc);
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let note = at(&rows, "copying Late Train Verse…");
        assert_eq!(fg(&buffer, note), th().accent);
        // It lands: the note says so, OK, the tail where it fits.
        poll(&mut gui, &[job(90, &late, None, "done", json!({ "result": { "copied": { "vpath": "copies", "filepath": "copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3" } } }))]);
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let note = at(&rows, "copied Late Train Verse");
        assert_eq!(fg(&buffer, note), th().ok);
        assert!(!rows[note.y as usize].contains("into"), "the tail only where the whole note fits: {}", rows[note.y as usize]);
        // In a wider window the tail rides along.
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 120, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        start(&mut gui, Ok(job(90, &late, None, "running", json!({}))));
        key(&mut gui, KeyCode::Esc);
        let wide = draw(&mut gui, 120, 30);
        assert!(wide.iter().any(|r| r.contains("copying Late Train Verse from Stage Peer B…")), "{}", wide.join("\n"));
        poll(&mut gui, &[job(90, &late, None, "done", json!({ "result": { "copied": { "vpath": "copies", "filepath": "copies/x.mp3" } } }))]);
        let wide = draw(&mut gui, 120, 30);
        assert!(wide.iter().any(|r| r.contains("copied Late Train Verse into copies")), "{}", wide.join("\n"));
    }

    #[test]
    fn the_notes_words_fit_their_cells() {
        let _en = english();
        let late = fed(C, "Rooftop Cypher", "Boukmanflow", "Borrowed Tapes", 0.6, Some(12.0));
        let failed = job(5, &late, None, "failed", json!({ "error": "Stage Peer C is unreachable" }));
        assert_eq!(super::news_note(&failed, 30).map(|n| n.0).as_deref(), Some("copy failed: Stage Peer C is…"));
        let quota = job(6, &late, Some("album"), "done", counts(1, 0, 0, Some("quota")));
        assert_eq!(super::news_note(&quota, 30).map(|n| n.0).as_deref(), Some("copy stopped: transfer limit"));
        assert_eq!(
            super::news_note(&quota, 60).map(|n| n.0).as_deref(),
            Some("copy stopped: Stage Peer C reached its transfer limit")
        );
        let refused = job(7, &late, None, "failed", json!({ "error": "Stage Peer C does not allow copies with your server's key" }));
        assert_eq!(super::news_note(&refused, 30), Some(("Stage Peer C refuses copies".into(), crate::tui::app::MessageKind::Error)));
        let album = job(8, &late, Some("album"), "done", counts(3, 0, 0, None));
        assert_eq!(super::news_note(&album, 30).map(|n| n.0).as_deref(), Some("album copy finished"));
        assert_eq!(super::news_note(&album, 60).map(|n| n.0).as_deref(), Some("album copy finished · Borrowed Tapes"));
        let train = fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0));
        let yours = job(9, &train, None, "done", json!({ "result": { "skipped": "owned" } }));
        assert_eq!(super::news_note(&yours, 30), Some(("already yours".into(), crate::tui::app::MessageKind::Info)));
        assert_eq!(super::news_note(&yours, 40).map(|n| n.0).as_deref(), Some("already yours · Late Train Verse"));
        let cancelled = job(10, &late, Some("album"), "cancelled", json!({}));
        assert_eq!(super::news_note(&cancelled, 60).map(|n| n.0).as_deref(), Some("copy cancelled · Borrowed Tapes"));
    }

    #[test]
    fn a_server_name_stays_whole_when_the_words_wrap() {
        let words = "Stage Peer C is already copying this album — you can add Stage Peer B's once it ends";
        assert_eq!(
            super::wrap_keeping(words, 62, &["Stage Peer C", "Stage Peer B"]),
            vec!["Stage Peer C is already copying this album — you can add", "Stage Peer B's once it ends"]
        );
    }

    // ── Where copies land (clauses 142–148; the phone's sheet, the webapp's editor) ───

    /// The board's sample: the account may write to music (the library
    /// default, its template ARTIST / YEAR - ALBUM) and copies (saved,
    /// Album artist / Album).
    fn dest_view(base: &str, source: &str) -> Value {
        json!({
            "destination": { "vpath": "copies", "base": base, "layout": "{{ALBUMARTIST}}/{{ALBUM}}", "source": source },
            "saved": { "vpath": "copies", "base": base, "layout": "{{ALBUMARTIST}}/{{ALBUM}}" },
            "libraries": [{ "vpath": "music", "template": "{{ARTIST}}/{{YEAR}} - {{ALBUM}}" }, { "vpath": "copies", "template": null }],
            "defaultLayout": "{{ALBUMARTIST}}/{{ALBUM}}", "variables": ["ARTIST", "ALBUM", "YEAR", "GENRE", "ALBUMARTIST", "PEER"]
        })
    }

    /// The board's sample from a server that lists SERVERFOLDER too
    /// (mStream since 2026-10-22).
    fn dest_view_folders(base: &str, source: &str) -> Value {
        let mut view = dest_view(base, source);
        view["variables"].as_array_mut().expect("variables").push(json!("SERVERFOLDER"));
        view
    }

    /// Answer the destination read out (a Keep a copy that opened reads it
    /// again) with `view`.
    fn land_read(gui: &mut Gui, view: Value) {
        let ask = gui.pending.iter().rev().find_map(|e| match e {
            Effect::Api(ApiCmd::DestinationRead { seq, generation }) => Some((*seq, *generation)),
            _ => None,
        });
        let Some((seq, generation)) = ask else { panic!("no read in {:?}", gui.pending) };
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::DestinationRead { .. })));
        apply(gui, Event::DestinationRead { seq, generation, answer: Ok(Box::new(serde_json::from_value(view).unwrap())) });
    }

    /// Answer every folder listing the picker asks, a frame between (it
    /// asks one at a time): the libraries hold from-peers, and from-peers
    /// three servers' folders — `more` folders instead where given;
    /// anything else is empty.
    fn land_folders_with(gui: &mut Gui, more: Option<usize>) {
        loop {
            let ask = gui.pending.iter().find_map(|e| match e {
                Effect::Api(ApiCmd::DestinationFolders { seq, generation, path }) => Some((*seq, *generation, path.clone())),
                _ => None,
            });
            let Some((seq, generation, path)) = ask else { return };
            let asked = gui.pending.iter().filter(|e| matches!(e, Effect::Api(ApiCmd::DestinationFolders { .. }))).count();
            assert_eq!(asked, 1, "one listing at a time: {:?}", gui.pending);
            gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::DestinationFolders { .. })));
            let names: Vec<String> = match (path.as_str(), more) {
                ("/copies" | "/music", _) => vec!["from-peers".into()],
                ("/copies/from-peers", Some(n)) => (1..=n).map(|i| format!("folder {i}")).collect(),
                ("/copies/from-peers", None) => vec!["2025 archive".into(), "Stage Peer B".into(), "Stage Peer C".into()],
                _ => Vec::new(),
            };
            apply(gui, Event::DestinationFolders { seq, generation, path, answer: Ok(names) });
            draw(gui, 100, 30);
        }
    }

    fn land_folders(gui: &mut Gui) {
        land_folders_with(gui, None);
    }

    /// Keep a copy open on Late Train Verse, its place read again as the
    /// board's sample, the picker open with `d`.
    fn dest_gui(base: &str) -> Gui {
        dest_gui_on(dest_view(base, "user"))
    }

    /// The same, its place read again as `view`.
    fn dest_gui_on(view: Value) -> Gui {
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        land_read(&mut gui, view);
        draw(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Char('d'));
        land_folders(&mut gui);
        gui
    }

    /// The picker's own rows, 80 wide, from its top border — its title on
    /// it — to its bottom.
    fn picker_rows(rows: &[String]) -> Vec<String> {
        let top = rows.iter().position(|r| r.contains("─ Where copies land ─") || r.contains("- Where copies land -")).expect("the picker");
        let bottom = (top + 1..rows.len()).find(|&y| matches!(rows[y].chars().nth(10), Some('╰' | '+'))).expect("its bottom");
        rows[top..=bottom].iter().map(|r| r.chars().skip(10).take(80).collect()).collect()
    }

    fn saves(gui: &Gui) -> Vec<Option<crate::api::types::CollectionDestination>> {
        gui.pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::DestinationSave { destination, .. }) => Some(destination.clone()),
                _ => None,
            })
            .collect()
    }

    /// Answer the save out.
    fn land_save(gui: &mut Gui, answer: Result<Value, crate::api::PluginFailure>) {
        let ask = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::DestinationSave { seq, generation, .. }) => Some((*seq, *generation)),
            _ => None,
        });
        let Some((seq, generation)) = ask else { panic!("no save in {:?}", gui.pending) };
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::DestinationSave { .. })));
        let answer = answer.map(|view| Box::new(serde_json::from_value(view).unwrap()));
        apply(gui, Event::DestinationSaved { seq, generation, answer });
    }

    /// The picker's frame around `lines`, 80 wide: its title on the top
    /// border, "Esc close" on the bottom.
    fn boxed(lines: &[&str]) -> Vec<String> {
        let mut out = vec![format!("╭─ Where copies land {}╮", "─".repeat(58))];
        out.extend(lines.iter().map(|l| format!("│{l:<78}│")));
        out.push(format!("╰{} Esc close ─╯", "─".repeat(66)));
        out
    }

    fn dest_act(gui: &mut Gui, act: super::DestAct) {
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Dest(act))));
    }

    fn typed(gui: &mut Gui, text: &str) {
        for c in text.chars() {
            key(gui, KeyCode::Char(c));
        }
    }

    fn ctrl(gui: &mut Gui, c: char) {
        super::super::handle_key(gui, KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }

    #[test]
    fn a_copies_retry_sends_its_own_pin_and_a_moved_destination_is_its_note() {
        let _en = english();
        let late = fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0));
        let failed = job(70, &late, None, "failed", json!({ "error": "Stage Peer B is unreachable" }));
        let mut gui = copies_gui(std::slice::from_ref(&failed));
        gui.act(Act::Disc(DiscAct::CopiesAct(70)));
        let (ticket, sent) = gui
            .pending
            .iter()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::JobStart { ticket, destination, .. }) => Some((*ticket, destination.clone())),
                _ => None,
            })
            .expect("a retry");
        assert_eq!(sent, failed.params.destination, "the job's own pin, never today's place");
        let generation = gui.app.session_gen();
        apply(&mut gui, Event::JobStarted { ticket, generation, answer: Err(crate::api::PluginFailure::DestinationChanged(Some(None))) });
        let rows = draw(&mut gui, 100, 30);
        assert!(
            rows.iter().any(|r| r.contains("couldn't start: where files land changed — retry it from Keep a copy")),
            "{}",
            rows.join("\n")
        );
    }

    /// Keep a copy open on Late Train Verse, its place read again as `view`.
    fn keep_on(view: Value) -> Gui {
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        land_read(&mut gui, view);
        gui
    }

    /// The row a text stands on.
    fn at_row(rows: &[String], text: &str) -> u16 {
        at(rows, text).y
    }

    #[test]
    fn a_picker_whose_view_went_gives_way_to_keep_a_copy() {
        let _en = english();
        let mut gui = dest_gui("");
        let generation = gui.app.session_gen();
        let failed = crate::api::PluginFailure::Unreachable("down".into());
        apply(&mut gui, Event::KeepGate { generation, plugins: Err(failed), destination: None });
        assert!(gui.app.keep_dest_view().is_none());
        // A key first: Esc is never swallowed by a picker nothing draws.
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.keep.dest.is_none());
        let mut gui = dest_gui("");
        apply(&mut gui, Event::KeepGate { generation, plugins: Err(crate::api::PluginFailure::Unreachable("down".into())), destination: None });
        let rows = draw(&mut gui, 100, 30);
        assert!(gui.discover.keep.dest.is_none());
        assert!(rows.iter().any(|r| r.contains("Keep a copy")), "{}", rows.join("\n"));
        key(&mut gui, KeyCode::Esc);
        assert!(gui.app.taste.keep.flow.is_none(), "Esc closes it");
    }

    /// Answer one folder listing the picker asked, of `path`, with `names`.
    fn land_listing(gui: &mut Gui, path: &str, names: &[&str]) {
        let ask = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::DestinationFolders { seq, generation, path: asked }) if asked == path => Some((*seq, *generation)),
            _ => None,
        });
        let Some((seq, generation)) = ask else { panic!("no listing of {path} in {:?}", gui.pending) };
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::DestinationFolders { .. })));
        let names = names.iter().map(|n| n.to_string()).collect();
        apply(gui, Event::DestinationFolders { seq, generation, path: path.to_string(), answer: Ok(names) });
        draw(gui, 100, 30);
    }

    /// The picker's hit at a cell, as the pointer's act.
    fn dest_hit(gui: &Gui, at: Position) -> Option<super::DestAct> {
        match gui.ui.hit(at) {
            Some(Act::Disc(DiscAct::Keep(super::KeepAct::Dest(act)))) => Some(act),
            _ => None,
        }
    }

    /// The tooltip registered over a cell.
    fn tip_at(gui: &Gui, at: Position) -> Option<String> {
        gui.ui.tips.iter().rev().find(|(rect, _)| rect.contains(at)).map(|(_, tip)| tip.clone())
    }

    fn ui(gui: &Gui) -> &super::DestUi {
        gui.discover.keep.dest.as_ref().expect("the picker")
    }

    /// The picker on the saved place, the hand on the libraries (the
    /// editor idle on a preset): where this song lands, the choices, the
    /// buttons — no header, no labels, no templates.
    const IDLE: [&str; 10] = [
        " Files go to copies › Boukmanflow › Borrowed Tapes",
        "",
        " ( ) music   (•) copies",
        "",
        " (•) Artist / Album",
        " ( ) Server / Artist / Album",
        " ( ) Custom",
        " Start from an existing folder…",
        "",
        FOOT,
    ];

    /// The buttons' row: Use the library default at the text's column,
    /// Save at the right (the webapp's footer).
    const FOOT: &str = " [ Use the library default ]                                         [ Save ]";

    fn idle() -> Vec<String> {
        boxed(&IDLE)
    }

    /// The words the trim took out of the picker (2026-10-24): none of
    /// them stands in any of its states.
    const CUT: [&str; 21] = [
        "· yours",
        "· library default",
        "[X]",
        "One setting for your account",
        "lands at",
        "land at",
        "Type a path to see",
        "Fix the layout to see",
        "LIBRARY",
        "LAYOUT",
        "Type your own path",
        "Use / between folders",
        "a plain name puts every copy",
        ".mp3",
        "· 1 folder",
        "· 3 folders",
        "· 4 folders",
        "would pass",
        "so left out of the path",
        "does not exist yet",
        "Use ARTIST",
    ];

    /// The cut words absent from the picker's rows.
    fn none_cut(picker: &[String]) {
        for words in CUT {
            assert!(!picker.iter().any(|r| r.contains(words)), "{words:?} still stands:\n{}", picker.join("\n"));
        }
    }

    #[test]
    fn change_opens_where_copies_land_on_the_saved_place_as_the_editor_draws_it() {
        let _en = english();
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        // Opening read the destination again: the answer redraws the place
        // — and only the place: whose it is is said nowhere (§7).
        land_read(&mut gui, dest_view("", "user"));
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[6], "│ Files go to copies › Boukmanflow › Borrowed Tapes               d change │", "{}", low.join("\n"));
        assert!(!low.iter().any(|r| r.contains("saved for your account")));
        // No stop to walk to: `d` opens it — its first row the words of the
        // line it was opened from.
        key(&mut gui, KeyCode::Char('d'));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(picker_rows(&rows), idle(), "{}", rows.join("\n"));
        none_cut(&picker_rows(&rows));
        assert!(!rows.iter().any(|r| r.contains("{{ALBUMARTIST}}/{{ALBUM}}")), "no layout beside a preset: {}", rows.join("\n"));
        // 80 wide, centred over the whole frame, its top on row 4 (as if 22
        // rows); Keep a copy is not drawn while it is open.
        assert_eq!((rows[4].chars().nth(10), rows[4].chars().nth(89)), (Some('╭'), Some('╮')));
        assert!(!rows.iter().any(|r| r.contains("Keep a copy")), "{}", rows.join("\n"));
        assert_eq!(at(&rows, "Files go to copies › Boukmanflow › Borrowed Tapes"), Position { x: 12, y: 5 });
        // The tokens: the frame ACCENT, the title on it TEXT BOLD, "Esc
        // close" DIM; "Files go to" DIM, the library BOLD; the hand on the
        // libraries shows on the chosen name, ACCENT BOLD, its dot OK, the
        // other plain; the dots at the text's column, the chosen radio's
        // name BOLD at col 5; the folders button DIM, the quiet button DIM
        // — and Save DIM: nothing changed.
        let cell = |text: &str| at(&rows, text);
        let text = |at: Position| (fg(&buffer, at), bold(&buffer, at));
        assert_eq!(fg(&buffer, Position { x: 10, y: 4 }), th().accent, "the frame ACCENT");
        assert_eq!(cell("Where copies land"), Position { x: 13, y: 4 });
        assert_eq!(text(cell("Where copies land")), (Color::Reset, true), "the title on the border, TEXT BOLD");
        assert_eq!(cell("Esc close"), Position { x: 78, y: 15 });
        assert_eq!(text(cell("Esc close")), (th().dim, false));
        assert_eq!(fg(&buffer, cell("Files go to")), th().dim);
        assert_eq!(text(cell("copies › Boukmanflow")), (Color::Reset, true), "the library BOLD");
        let copies = cell("(•) copies");
        assert_eq!(fg(&buffer, copies), th().ok);
        assert_eq!(text(Position { x: copies.x + 4, y: copies.y }), (th().accent, true), "the hand on the chosen name");
        let music = cell("( ) music");
        assert_eq!(music.x, 12, "no labels column");
        assert_eq!(text(Position { x: music.x + 4, y: music.y }), (Color::Reset, false));
        let chosen = cell("(•) Artist / Album");
        assert_eq!(chosen.x, 12, "the dots at the text's column");
        assert_eq!(text(Position { x: 16, y: chosen.y }), (Color::Reset, true), "the names at col 5");
        assert_eq!(fg(&buffer, cell("Start from an existing")), th().dim);
        assert_eq!(fg(&buffer, cell("[ Use the library default ]")), th().dim);
        assert_eq!(text(cell("[ Save ]")), (th().dim, false));
        assert_eq!(super::super::discover::tips(&gui), "← → library · ↑ ↓ rows · Esc close");
        // The pointer: a library chooses, a radio's whole row picks, the
        // folders button opens the list, "Esc close" goes back; Save off
        // takes no click but says why; no `[X]`, the preview's row inert,
        // and no tooltip where nothing is clipped; the room beneath inert.
        assert_eq!(dest_hit(&gui, music), Some(super::DestAct::Library(0)));
        assert_eq!(dest_hit(&gui, copies), Some(super::DestAct::Library(1)));
        let server = cell("( ) Server / Artist");
        assert_eq!(dest_hit(&gui, server), Some(super::DestAct::Pick(super::Radio::Server)));
        assert_eq!(dest_hit(&gui, Position { x: 87, y: server.y }), Some(super::DestAct::Pick(super::Radio::Server)));
        assert_eq!(dest_hit(&gui, cell("( ) Custom")), Some(super::DestAct::Pick(super::Radio::Custom)));
        assert_eq!(dest_hit(&gui, cell("Start from an existing")), Some(super::DestAct::Folders));
        assert_eq!(dest_hit(&gui, cell("Esc close")), Some(super::DestAct::Back));
        assert_eq!(dest_hit(&gui, Position { x: 86, y: 15 }), Some(super::DestAct::Back));
        assert_eq!(gui.ui.hit(Position { x: 87, y: 15 }), Some(Act::Guard));
        assert_eq!(gui.ui.hit(Position { x: 87, y: 5 }), Some(Act::Guard), "no [X]");
        assert_eq!(gui.ui.hit(cell("Files go to")), Some(Act::Guard));
        assert_eq!(dest_hit(&gui, cell("[ Use the library default ]")), Some(super::DestAct::UseDefault));
        assert_eq!(gui.ui.hit(cell("[ Save ]")), Some(Act::Guard));
        assert_eq!(tip_at(&gui, cell("[ Save ]")).as_deref(), Some("nothing changed yet"));
        assert_eq!(tip_at(&gui, cell("Files go to")), None, "whole on its row: no tooltip");
        assert_eq!(tip_at(&gui, cell("Esc close")), None);
        assert_eq!(gui.ui.hit(Position { x: 5, y: 25 }), Some(Act::Guard));
        // Esc goes back, nothing saved: Keep a copy as it was, the dot where
        // it was.
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.keep.dest.is_none() && saves(&gui).is_empty());
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(lower_rows(&rows), low, "{}", rows.join("\n"));
        // A click on `d change` opens it too, and a click on "Esc close"
        // goes back as Esc does.
        let change = at(&rows, "d change");
        assert_eq!(gui.ui.hit(change), Some(Act::Disc(DiscAct::Keep(super::KeepAct::Key(super::KeyWord::Change)))));
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Key(super::KeyWord::Change))));
        assert!(gui.discover.keep.dest.is_some());
        let rows = draw(&mut gui, 100, 30);
        gui.act(gui.ui.hit(at(&rows, "Esc close")).expect("Esc close"));
        assert!(gui.discover.keep.dest.is_none() && saves(&gui).is_empty());
        // Whose place it is is said nowhere: on the library's default the
        // picker reads as on the account's own — Use the library default
        // stands only where pressing it would change something.
        let mut gui = dest_gui_on(dest_view("", "default"));
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(picker_rows(&rows), idle(), "{}", rows.join("\n"));
    }

    #[test]
    fn the_library_row_chooses_with_arrows_and_clicks_and_a_librarys_own_layout_follows_it() {
        let _en = english();
        let mut gui = dest_gui("");
        // ← to music: its own layout joins the radios by its name alone,
        // Save turns on.
        key(&mut gui, KeyCode::Left);
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(
            picker_rows(&rows),
            boxed(&[
                " Files go to music › Boukmanflow › Borrowed Tapes",
                "",
                " (•) music   ( ) copies",
                "",
                " (•) Artist / Album",
                " ( ) Server / Artist / Album",
                " ( ) Library layout",
                " ( ) Custom",
                " Start from an existing folder…",
                "",
                FOOT,
            ]),
            "{}",
            rows.join("\n")
        );
        assert!(!rows.iter().any(|r| r.contains("{{ARTIST}}/{{YEAR}} - {{ALBUM}}")), "no template beside it: {}", rows.join("\n"));
        let save = at(&rows, "[ Save ]");
        assert_eq!((fg(&buffer, save), bold(&buffer, save)), (th().accent, true));
        assert_eq!(dest_hit(&gui, save), Some(super::DestAct::Save));
        // Save on: the tips say Enter saves from the libraries (clause 119).
        assert_eq!(super::super::discover::tips(&gui), "← → library · Enter save · ↑ ↓ rows · Esc close");
        // Its layout picked: the preview follows; → back to copies takes
        // the layout along, into Custom's field — under "Start from an
        // existing folder…", the webapp's order — a library's own layout
        // that leaves with it stays the layout.
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Template));
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(picker_rows(&rows)[1], format!("│{:<78}│", " Files go to music › Boukmanflow › 2019 - Borrowed Tapes"));
        dest_act(&mut gui, super::DestAct::Step(1));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[7], format!("│{:<78}│", " (•) Custom"), "{}", picker.join("\n"));
        assert_eq!(picker[8], format!("│{:<78}│", " Start from an existing folder…"));
        assert_eq!(picker[9], format!("│{:<78}│", "     {{ARTIST}}/{{YEAR}} - {{ALBUM}}"));
        assert_eq!(picker[1], format!("│{:<78}│", " Files go to copies › Boukmanflow › 2019 - Borrowed Tapes"));
        // ← back to music with it untouched: its preset again, Custom's
        // field as it was — empty.
        dest_act(&mut gui, super::DestAct::Step(-1));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[7], format!("│{:<78}│", " (•) Library layout"), "{}", picker.join("\n"));
        assert!(!ui(&gui).custom && ui(&gui).text.value().is_empty());
        // A click chooses a library as the arrows do, and gives the row the
        // hand.
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Save));
        let rows = draw(&mut gui, 100, 30);
        gui.act(gui.ui.hit(at(&rows, "( ) copies")).expect("copies"));
        assert_eq!(ui(&gui).vpath, "copies");
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Library));
    }

    #[test]
    fn custom_opens_empty_with_its_hint_and_the_validator_speaks_as_typed() {
        let _en = english();
        let mut gui = dest_gui("");
        // Custom clicked: its field takes the keyboard, empty, the hint
        // after the caret; the preview holds its row with the kit's DIM `—`
        // so the field does not move on the first key; Save is off.
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Field));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let empty = boxed(&[
            " Files go to —",
            "",
            " ( ) music   (•) copies",
            "",
            " ( ) Artist / Album",
            " ( ) Server / Artist / Album",
            " (•) Custom",
            " Start from an existing folder…",
            "     ▏e.g. From friends or {{ARTIST}}/{{ALBUM}}",
            "     ARTIST · ALBUM · YEAR · GENRE · ALBUMARTIST · PEER",
            "",
            FOOT,
        ]);
        assert_eq!(picker_rows(&rows), empty, "{}", rows.join("\n"));
        none_cut(&picker_rows(&rows));
        assert_eq!(fg(&buffer, at(&rows, "Files go to —")), th().dim);
        assert_eq!(at(&rows, "Files go to —"), Position { x: 12, y: 5 });
        assert_eq!(fg(&buffer, Position { x: 24, y: 5 }), th().dim, "the kit's `—` DIM");
        assert_eq!(fg(&buffer, at(&rows, "e.g. From friends")), th().dim);
        assert_eq!(fg(&buffer, at(&rows, "ARTIST · ALBUM")), th().dim);
        assert_eq!(super::super::discover::tips(&gui), "type a layout · ↑ ↓ rows · Esc close");
        // An unknown variable: the preview's `—` stays DIM; the validator's
        // words under the field, GOLD — the typo named, the variables right
        // under them the known ones; Save off.
        typed(&mut gui, "{{ARTST}}/{{YEAR}} - {{ALBUM}}");
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], format!("│{:<78}│", " Files go to —"), "{}", picker.join("\n"));
        assert_eq!(picker[8], format!("│{:<78}│", " Start from an existing folder…"));
        assert_eq!(picker[9], format!("│{:<78}│", "     {{ARTST}}/{{YEAR}} - {{ALBUM}}▏"));
        assert_eq!(picker[10], format!("│{:<78}│", "     Unknown variable {{ARTST}}."));
        assert_eq!(picker[11], format!("│{:<78}│", "     ARTIST · ALBUM · YEAR · GENRE · ALBUMARTIST · PEER"));
        assert_eq!(fg(&buffer, at(&rows, "Files go to —")), th().dim);
        assert_eq!(fg(&buffer, at(&rows, "Unknown variable")), th().gold);
        assert_eq!(tip_at(&gui, at(&rows, "[ Save ]")).as_deref(), Some("fix the layout first"));
        none_cut(&picker);
        key(&mut gui, KeyCode::Enter);
        assert!(saves(&gui).is_empty(), "Enter saves nothing while Save is off");
        // Each refusal in the phone's words, as typed, on its one row.
        let problem = |gui: &mut Gui, text: &str| {
            ctrl(gui, 'u');
            typed(gui, text);
            let rows = draw(gui, 100, 30);
            let picker = picker_rows(&rows);
            picker[10..].iter().take_while(|r| !r.contains("ARTIST · ALBUM")).map(|r| r.trim_matches('│').trim().to_string()).collect::<Vec<_>>()
        };
        assert_eq!(problem(&mut gui, "/x"), ["A layout can't start with / — it begins inside the library."]);
        assert_eq!(problem(&mut gui, "a/../b"), ["A path can't contain .. folders."]);
        assert_eq!(problem(&mut gui, "~/x"), ["A path can't contain ~ or $HOME."]);
        assert_eq!(problem(&mut gui, "C:/x"), ["A path can't use a drive letter like C:."]);
        assert_eq!(problem(&mut gui, "{{ARTIST}"), ["A brace is out of place — variables are written like {{ARTIST}}."]);
        assert_eq!(problem(&mut gui, " "), ["That makes no folder — add a name or a variable."]);
        // A valid layout: the path, Save on, Enter saves it with no base folder.
        ctrl(&mut gui, 'u');
        typed(&mut gui, "From friends/{{ARTIST}}/{{ALBUM}}");
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], format!("│{:<78}│", " Files go to copies › From friends › Boukmanflow › Borrowed Tapes"), "{}", picker.join("\n"));
        assert_eq!(picker[9], format!("│{:<78}│", "     From friends/{{ARTIST}}/{{ALBUM}}▏"));
        assert_eq!(picker[10], format!("│{:<78}│", "     ARTIST · ALBUM · YEAR · GENRE · ALBUMARTIST · PEER"));
        assert_eq!(super::super::discover::tips(&gui), "type a layout · Enter save · ↑ ↓ rows · Esc close", "Save on: Enter saves from the field");
        key(&mut gui, KeyCode::Enter);
        let place = saves(&gui)[0].clone().expect("a place");
        assert_eq!((place.vpath.as_str(), place.base.as_str(), place.layout.as_str()), ("copies", "", "From friends/{{ARTIST}}/{{ALBUM}}"));
        // Chosen with the keyboard, the hand stays on the radio: the field
        // comes after it, past the folders button.
        let mut gui = dest_gui("");
        for _ in 0..3 {
            key(&mut gui, KeyCode::Down);
        }
        key(&mut gui, KeyCode::Char(' '));
        assert!(ui(&gui).custom);
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Radio(super::Radio::Custom)));
        key(&mut gui, KeyCode::Down);
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Folders));
        key(&mut gui, KeyCode::Down);
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Field));
        // A click on the field's row gives it the keys; a click on Custom's
        // radio from any cell of its row picks it.
        let mut gui = dest_gui("");
        let rows = draw(&mut gui, 100, 30);
        let custom = at(&rows, "( ) Custom");
        gui.act(gui.ui.hit(Position { x: 60, y: custom.y }).expect("Custom's row"));
        assert!(ui(&gui).custom && ui(&gui).hand == Some(super::DestStop::Field));
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Library));
        let rows = draw(&mut gui, 100, 30);
        gui.act(gui.ui.hit(Position { x: 30, y: at_row(&rows, "e.g. From friends") }).expect("the field"));
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Field));
    }

    #[test]
    fn the_variables_insert_at_the_caret_from_the_row_and_the_keyboard() {
        let _en = english();
        let mut gui = dest_gui("from-peers");
        // The caret after "from-peers/": a click on YEAR drops it there and
        // hands the keyboard back to the field.
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Field));
        key(&mut gui, KeyCode::Home);
        for _ in 0..11 {
            key(&mut gui, KeyCode::Right);
        }
        let rows = draw(&mut gui, 100, 30);
        gui.act(gui.ui.hit(at(&rows, "YEAR ·")).expect("the variable"));
        assert_eq!(ui(&gui).text.value(), "from-peers/{{YEAR}}{{ALBUMARTIST}}/{{ALBUM}}");
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Field));
        // ↓ to the row, → → to YEAR, Enter inserts it at the caret; the hand
        // stays on the row, its variable ACCENT BOLD.
        key(&mut gui, KeyCode::Down);
        assert_eq!(super::super::discover::tips(&gui), "← → variable · Enter insert · ↑ ↓ rows · Esc close");
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Right);
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let year = at(&rows, "YEAR ·");
        assert_eq!((fg(&buffer, year), bold(&buffer, year)), (th().accent, true));
        assert_eq!(fg(&buffer, at(&rows, "ALBUM ·")), th().dim);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(ui(&gui).text.value(), "from-peers/{{YEAR}}{{YEAR}}{{ALBUMARTIST}}/{{ALBUM}}");
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Vars));
        // Tab walks on as ↓ does — to the buttons: the folders button stands
        // above the field — Shift-Tab back.
        key(&mut gui, KeyCode::Tab);
        assert_eq!(ui(&gui).hand, Some(super::DestStop::UseDefault));
        key(&mut gui, KeyCode::BackTab);
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Vars));
        key(&mut gui, KeyCode::BackTab);
        key(&mut gui, KeyCode::BackTab);
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Folders));
    }

    #[test]
    fn start_from_an_existing_folder_lists_the_servers_folders_and_a_pick_starts_custom_there() {
        let _en = english();
        let mut gui = dest_gui("");
        for _ in 0..4 {
            key(&mut gui, KeyCode::Down);
        }
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Folders));
        assert_eq!(super::super::discover::tips(&gui), "Enter folders · ↑ ↓ rows · Esc close");
        // Enter opens the list on the library root, asked of the session's
        // own server once: "Looking up…" until it answers; the head and the
        // rows hang two cells in, the button's own.
        key(&mut gui, KeyCode::Enter);
        let asked = |gui: &Gui| -> Vec<String> {
            gui.pending
                .iter()
                .filter_map(|e| match e {
                    Effect::Api(ApiCmd::DestinationFolders { path, .. }) => Some(path.clone()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(asked(&gui), vec!["/copies".to_string()]);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[8], format!("│{:<78}│", " Hide folders"), "{}", picker.join("\n"));
        assert_eq!(picker[9], format!("│{:<78}│", "   copies"));
        assert_eq!(picker[10], format!("│{:<78}│", "   Looking up…"));
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ folders · Enter open · Esc close the list");
        // Answered: the head names where the list is, no count — its rows
        // say it.
        land_folders(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[9], format!("│{:<78}│", "   copies"), "{}", picker.join("\n"));
        assert_eq!(picker[10], format!("│{:<78}│", "   from-peers"));
        assert_eq!(picker.len(), 14);
        none_cut(&picker);
        // ↓ holds the folder (the slab across cols 13–87, not bold); Enter
        // starts Custom there and lists inside it: the list under the
        // button, then Custom's field and variables (the webapp's order).
        key(&mut gui, KeyCode::Down);
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let held = at(&rows, "from-peers");
        assert_eq!(held.x, 14);
        assert_eq!((bg(&buffer, held), bold(&buffer, held)), (th().accent, false));
        assert_eq!((bg(&buffer, Position { x: 13, y: held.y }), bg(&buffer, Position { x: 87, y: held.y })), (th().accent, th().accent));
        assert_ne!(bg(&buffer, Position { x: 12, y: held.y }), th().accent);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(asked(&gui), vec!["/copies/from-peers".to_string()]);
        land_folders(&mut gui);
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let bar = |text: &str, glyph: char| format!("{text:<77}{glyph}");
        let (up, archive, peer) = (bar("   Up to copies", '▲'), bar("   2025 archive", '█'), bar("   Stage Peer B", '▼'));
        assert_eq!(
            picker_rows(&rows),
            boxed(&[
                " Files go to copies › from-peers › Boukmanflow › Borrowed Tapes",
                "",
                " ( ) music   (•) copies",
                "",
                " ( ) Artist / Album",
                " ( ) Server / Artist / Album",
                " (•) Custom",
                " Hide folders",
                "   copies › from-peers",
                &up,
                &archive,
                &peer,
                "     from-peers/{{ALBUMARTIST}}/{{ALBUM}}",
                "     ARTIST · ALBUM · YEAR · GENRE · ALBUMARTIST · PEER",
                "",
                FOOT,
            ]),
            "{}",
            rows.join("\n")
        );
        assert!(ui(&gui).custom);
        assert_eq!(fg(&buffer, Position { x: 14, y: at_row(&rows, "Up to copies") - 1 }), th().dim, "the head DIM");
        // ↓ ↓ ↓ Enter goes into Stage Peer B: nothing in it, Up to its
        // parent; the layout starts there.
        for _ in 0..3 {
            key(&mut gui, KeyCode::Down);
        }
        key(&mut gui, KeyCode::Enter);
        land_folders(&mut gui);
        assert_eq!(ui(&gui).text.value(), "from-peers/Stage Peer B/{{ALBUMARTIST}}/{{ALBUM}}");
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[9], format!("│{:<78}│", "   copies › from-peers › Stage Peer B"), "{}", picker.join("\n"));
        assert_eq!(picker[10], format!("│{:<78}│", "   Up to from-peers"));
        assert_eq!(picker[11], format!("│{:<78}│", "   No folders in here"));
        // Up: the parent again, the layout starting there; the wheel scrolls
        // the list (four rows, three shown).
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(ui(&gui).text.value(), "from-peers/{{ALBUMARTIST}}/{{ALBUM}}");
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("│   copies › from-peers   ")), "{}", rows.join("\n"));
        super::super::discover::wheel(&mut gui, 1);
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("Stage Peer C")), "the wheel scrolls the list: {}", rows.join("\n"));
        // Esc closes the list, the hand on the button; opened again it
        // lists the folder the layout starts in among its neighbours, that
        // one BOLD; a click on a row goes in as Enter does.
        key(&mut gui, KeyCode::Esc);
        assert!(ui(&gui).list.is_none());
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Folders));
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("Start from an existing folder…")) && rows.iter().any(|r| r.contains("ARTIST · ALBUM")), "{}", rows.join("\n"));
        dest_act(&mut gui, super::DestAct::Folders);
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let y = at_row(&rows, "Hide folders") + 2;
        assert!(rows[usize::from(y)].contains("from-peers") && bold(&buffer, Position { x: 22, y }), "{}", rows.join("\n"));
        gui.act(gui.ui.hit(Position { x: 25, y }).expect("the row"));
        assert_eq!(ui(&gui).text.value(), "from-peers/{{ALBUMARTIST}}/{{ALBUM}}");
        assert_eq!(ui(&gui).list.as_ref().map(|l| l.dir.clone()), Some(vec!["from-peers".to_string()]));
        // Save sends the layout alone, no base folder.
        dest_act(&mut gui, super::DestAct::Save);
        let place = saves(&gui)[0].clone().expect("a place");
        assert_eq!((place.vpath.as_str(), place.base.as_str(), place.layout.as_str()), ("copies", "", "from-peers/{{ALBUMARTIST}}/{{ALBUM}}"));
    }

    #[test]
    fn the_list_says_a_folder_not_there_yet_and_one_a_layout_cannot_start_from() {
        let _en = english();
        // Custom typed with a folder the root does not list: the list opens
        // on the root, and says that folder, by its name, is created by the
        // first file — whole on its row, no tooltip.
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        typed(&mut gui, "From friends/{{ARTIST}}");
        dest_act(&mut gui, super::DestAct::Folders);
        land_folders(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[8], format!("│{:<78}│", " Hide folders"), "{}", picker.join("\n"));
        assert_eq!(picker[9], format!("│{:<78}│", "   copies"));
        assert_eq!(picker[10], format!("│{:<78}│", "   from-peers"));
        assert_eq!(picker[11], format!("│{:<78}│", "   “From friends” isn't there yet · the first file creates it"));
        assert_eq!(tip_at(&gui, at(&rows, "“From friends” isn't there yet")), None);
        none_cut(&picker);
        // Two folders deep, none of them there: the list anchors on the
        // first, the server answers 404 — that folder not there yet, DIM, as
        // the webapp's dmBrowseMissing reads a failed listing; any other
        // failure (403) keeps the server's words in GOLD, after the head
        // that names the folder.
        let typed_deep = || {
            let mut gui = dest_gui("");
            dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
            typed(&mut gui, "New/Sub/{{ARTIST}}");
            dest_act(&mut gui, super::DestAct::Folders);
            gui
        };
        let failed = |gui: &mut Gui, status: u16, words: &str| {
            let ask = gui.pending.iter().find_map(|e| match e {
                Effect::Api(ApiCmd::DestinationFolders { seq, generation, path }) => Some((*seq, *generation, path.clone())),
                _ => None,
            });
            let (seq, generation, path) = ask.expect("a listing asked");
            assert_eq!(path, "/copies/New");
            gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::DestinationFolders { .. })));
            let failure = crate::api::PluginFailure::Refused { status, words: words.into() };
            apply(gui, Event::DestinationFolders { seq, generation, path, answer: Err(failure) });
        };
        let mut gui = typed_deep();
        failed(&mut gui, 404, "Cannot read directory \"/copies/New\" (ENOENT)");
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let picker = picker_rows(&rows);
        assert_eq!(picker[9], format!("│{:<78}│", "   copies › New"), "{}", picker.join("\n"));
        assert_eq!(picker[10], format!("│{:<78}│", "   Up to copies"));
        assert_eq!(picker[11], format!("│{:<78}│", "   “New” isn't there yet · the first file creates it"));
        assert_eq!(fg(&buffer, at(&rows, "“New” isn't there yet")), th().dim);
        assert!(!rows.iter().any(|r| r.contains("Couldn't list")), "{}", rows.join("\n"));
        let mut gui = typed_deep();
        failed(&mut gui, 403, "Forbidden");
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let note = at(&rows, "Couldn't list this folder: Forbidden");
        assert_eq!(fg(&buffer, note), th().gold, "{}", rows.join("\n"));
        assert_eq!(tip_at(&gui, note), None, "whole: no tooltip");
        // The library's own root a 404 (one the server no longer lists): no
        // file creates a library — the server's words, GOLD.
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Folders);
        let ask = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::DestinationFolders { seq, generation, path }) => Some((*seq, *generation, path.clone())),
            _ => None,
        });
        let (seq, generation, path) = ask.expect("the root asked");
        assert_eq!(path, "/copies");
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::DestinationFolders { .. })));
        let failure = crate::api::PluginFailure::Refused { status: 404, words: "Cannot read directory \"/copies\" (ENOENT)".into() };
        apply(&mut gui, Event::DestinationFolders { seq, generation, path, answer: Err(failure) });
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let note = at(&rows, "Couldn't list this folder: Cannot read directory");
        assert_eq!(fg(&buffer, note), th().gold, "{}", rows.join("\n"));
        assert!(!rows.iter().any(|r| r.contains("isn't there yet")), "{}", rows.join("\n"));
        // Words past the row: clipped at the border, whole in the tooltip.
        let mut gui = typed_deep();
        let long = "EACCES: permission denied, scandir '/srv/copies/New' — the server's account may not read it";
        failed(&mut gui, 500, long);
        let rows = draw(&mut gui, 100, 30);
        let note = at(&rows, "Couldn't list this folder: EACCES");
        assert!(rows[usize::from(note.y)].chars().nth(87) == Some('…'), "{}", rows[usize::from(note.y)]);
        assert_eq!(tip_at(&gui, note), Some(format!("Couldn't list this folder: {long}")));
        // A folder whose name has a brace: pressed, the list says so, GOLD,
        // and nothing changes.
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Folders);
        land_listing(&mut gui, "/copies", &["{{PEER}}", "plain"]);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let note = at(&rows, "“{{PEER}}” has a brace, so a layout can't start in it");
        assert_eq!(fg(&buffer, note), th().gold);
        assert!(!ui(&gui).custom, "nothing started there");
        assert_eq!(tip_at(&gui, note), None, "whole on its row: no tooltip");
        // A listing that went unanswered says so.
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Folders);
        let ask = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::DestinationFolders { seq, generation, path }) => Some((*seq, *generation, path.clone())),
            _ => None,
        });
        let (seq, generation, path) = ask.expect("the root asked");
        apply(&mut gui, Event::DestinationFolders { seq, generation, path, answer: Err(crate::api::PluginFailure::Unreachable("down".into())) });
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(fg(&buffer, at(&rows, "Couldn't list this folder: your server didn't answer")), th().gold, "{}", rows.join("\n"));
    }

    #[test]
    fn a_destination_saved_with_a_base_folder_opens_folded_into_custom_and_nothing_is_dirty() {
        let _en = english();
        let mut gui = dest_gui("from-peers");
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(
            picker_rows(&rows),
            boxed(&[
                " Files go to copies › from-peers › Boukmanflow › Borrowed Tapes",
                "",
                " ( ) music   (•) copies",
                "",
                " ( ) Artist / Album",
                " ( ) Server / Artist / Album",
                " (•) Custom",
                " Start from an existing folder…",
                "     from-peers/{{ALBUMARTIST}}/{{ALBUM}}",
                "     ARTIST · ALBUM · YEAR · GENRE · ALBUMARTIST · PEER",
                "",
                FOOT,
            ]),
            "{}",
            rows.join("\n")
        );
        none_cut(&picker_rows(&rows));
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Library));
        assert_eq!(fg(&buffer, at(&rows, "[ Save ]")), th().dim, "nothing dirty until edited");
        assert_eq!(tip_at(&gui, at(&rows, "[ Save ]")).as_deref(), Some("nothing changed yet"));
        assert_eq!(super::super::discover::tips(&gui), "← → library · ↑ ↓ rows · Esc close", "Save off: no Enter save");
        // An edit: Save sends base '' and the layout — the same files in the
        // same place.
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Field));
        typed(&mut gui, "/{{YEAR}}");
        dest_act(&mut gui, super::DestAct::Save);
        let place = saves(&gui)[0].clone().expect("a place");
        assert_eq!((place.vpath.as_str(), place.base.as_str(), place.layout.as_str()), ("copies", "", "from-peers/{{ALBUMARTIST}}/{{ALBUM}}/{{YEAR}}"));
        // A base folder whose name a layout reads as a variable: the preview
        // draws it as the server holds it — a folder — the field's problem
        // names it on one row, Save stays off; a preset over it is an edit.
        let mut gui = dest_gui("{{PEER}}");
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], format!("│{:<78}│", " Files go to copies › {{PEER}} › Boukmanflow › Borrowed Tapes"), "{}", picker.join("\n"));
        assert_eq!(picker[9], format!("│{:<78}│", "     {{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}"));
        assert_eq!(picker[10], format!("│{:<78}│", "     “{{PEER}}” would read as a variable · rename it, or choose a layout"));
        assert_eq!(picker[11], format!("│{:<78}│", "     ARTIST · ALBUM · YEAR · GENRE · ALBUMARTIST · PEER"));
        assert_eq!(fg(&buffer, at(&rows, "“{{PEER}}” would read")), th().gold);
        assert_eq!(fg(&buffer, at(&rows, "[ Save ]")), th().dim);
        // Server / Artist / Album — the very text the field holds — over it
        // is the edit that proves the rule: the server's name, not that
        // folder; so is Artist / Album.
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Server));
        assert_eq!(ui(&gui).layout(), "{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}");
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(fg(&buffer, at(&rows, "[ Save ]")), th().accent, "the same text as a preset is an edit");
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::ArtistAlbum));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(fg(&buffer, at(&rows, "[ Save ]")), th().accent, "the server's name, not that folder: an edit");
    }

    #[test]
    fn use_the_library_default_refills_only_and_save_then_sends_null() {
        let _en = english();
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::UseDefault));
        key(&mut gui, KeyCode::Char(' '));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], format!("│{:<78}│", " Files go to music › Boukmanflow › 2019 - Borrowed Tapes"), "{}", picker.join("\n"));
        assert_eq!(picker[3], format!("│{:<78}│", " (•) music   ( ) copies"));
        assert_eq!(picker[7], format!("│{:<78}│", " (•) Library layout"));
        // The button stays while the account's own place is in force; the
        // hand moves to the first stop; nothing is saved yet.
        assert_eq!(picker[11], format!("│{:<78}│", FOOT));
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Library));
        assert!(saves(&gui).is_empty());
        // Save sends null: back to the library default, nothing kept.
        dest_act(&mut gui, super::DestAct::Save);
        assert_eq!(saves(&gui), vec![None]);
        // A preset picked after it: an explicit place.
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::UseDefault);
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Server));
        dest_act(&mut gui, super::DestAct::Save);
        let place = saves(&gui)[0].clone().expect("a place");
        assert_eq!((place.vpath.as_str(), place.base.as_str(), place.layout.as_str()), ("music", "", "{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}"));
        // Away and back, and the same radio picked again: the fields are the
        // default, so Save still goes back to it.
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::UseDefault);
        dest_act(&mut gui, super::DestAct::Step(1));
        dest_act(&mut gui, super::DestAct::Step(-1));
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Template));
        dest_act(&mut gui, super::DestAct::Save);
        assert_eq!(saves(&gui), vec![None], "null: back to the library default");
        // Under the library's default already, the button is not offered:
        // its absence is the one trace of whose place is in force.
        let mut view = dest_view("", "default");
        view["destination"] = json!({ "vpath": "music", "base": "", "layout": "{{ARTIST}}/{{YEAR}} - {{ALBUM}}", "source": "default" });
        view["saved"] = Value::Null;
        let mut gui = dest_gui_on(view);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[picker.len() - 2], format!("│{:>77} │", "[ Save ]"), "{}", picker.join("\n"));
        none_cut(&picker);
    }

    #[test]
    fn saving_then_saved_lands_back_in_keep_a_copy_and_a_refusal_keeps_the_fields_in_gold() {
        let _en = english();
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        typed(&mut gui, "From friends/{{ARTIST}}/{{ALBUM}}");
        key(&mut gui, KeyCode::Enter);
        // Saving: the field without its caret, `[ Saving… ]` DIM where Save
        // stands — the kit's modal primary that goes off, the phone's word —
        // Use the library default not drawn, the hand down; no key but Esc,
        // no click edits.
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let picker = picker_rows(&rows);
        assert_eq!(picker[9], format!("│{:<78}│", "     From friends/{{ARTIST}}/{{ALBUM}}"), "{}", picker.join("\n"));
        assert_eq!(picker[12], format!("│{:>77} │", "[ Saving… ]"));
        let busy = at(&rows, "[ Saving… ]");
        assert_eq!((busy.x, fg(&buffer, busy), bold(&buffer, busy)), (77, th().dim, false));
        assert_eq!(gui.ui.hit(busy), Some(Act::Guard), "no click");
        assert!(!picker.iter().any(|r| r.contains("Use the library default")), "{}", picker.join("\n"));
        assert_eq!(super::super::discover::tips(&gui), "Esc close");
        assert!(ui(&gui).hand.is_none());
        key(&mut gui, KeyCode::Down);
        assert!(ui(&gui).hand.is_none(), "no key but Esc while saving");
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Server));
        dest_act(&mut gui, super::DestAct::Library(0));
        dest_act(&mut gui, super::DestAct::Var(0));
        dest_act(&mut gui, super::DestAct::Folders);
        let held = ui(&gui);
        assert!(held.custom && held.vpath == "copies" && held.text.value() == "From friends/{{ARTIST}}/{{ALBUM}}" && held.list.is_none(), "nothing edits while the save is out");
        // Saved: Keep a copy, the place redrawn from the saved one — its
        // evidence, nothing else announces it — `d change` beside its last
        // crumb, which the row no longer holds with it.
        let mut saved = dest_view("", "user");
        saved["destination"]["layout"] = json!("From friends/{{ARTIST}}/{{ALBUM}}");
        land_save(&mut gui, Ok(saved));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let low = lower_rows(&lines(&buffer));
        assert_eq!(
            low[6..8].to_vec(),
            canvas(&[
                "│ Files go to copies › From friends › Boukmanflow ›                        │",
                "│   Borrowed Tapes                                                d change │",
            ]),
            "{}",
            low.join("\n")
        );
        assert!(!low.iter().any(|r| r.contains("saved for your account")));
        assert!(gui.discover.keep.dest.is_none());
        // Refused: the server's words as sent (no prefix, as the webapp
        // draws them) GOLD under the field, three rows at most, the fields
        // as they were, the hand back in the field; an edit clears them.
        // The same rows at the floor, from row 1.
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        typed(&mut gui, "From friends/{{ARTIST}}/{{ALBUM}}");
        key(&mut gui, KeyCode::Enter);
        let words = "destination layout: Unknown variable {{ARTST}}. Supported: {{ARTIST}}, {{ALBUM}}, {{YEAR}}, {{GENRE}}, {{ALBUMARTIST}}, {{PEER}}, {{SERVERFOLDER}}";
        land_save(&mut gui, Err(crate::api::PluginFailure::Refused { status: 400, words: words.into() }));
        let refused = boxed(&[
            " Files go to copies › From friends › Boukmanflow › Borrowed Tapes",
            "",
            " ( ) music   (•) copies",
            "",
            " ( ) Artist / Album",
            " ( ) Server / Artist / Album",
            " (•) Custom",
            " Start from an existing folder…",
            "     From friends/{{ARTIST}}/{{ALBUM}}▏",
            "     destination layout: Unknown variable {{ARTST}}. Supported: {{ARTIST}},",
            "     {{ALBUM}}, {{YEAR}}, {{GENRE}}, {{ALBUMARTIST}}, {{PEER}},",
            "     {{SERVERFOLDER}}",
            "     ARTIST · ALBUM · YEAR · GENRE · ALBUMARTIST · PEER",
            "",
            FOOT,
        ]);
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(picker_rows(&rows), refused, "{}", rows.join("\n"));
        assert_eq!(fg(&buffer, at(&rows, "destination layout: Unknown")), th().gold);
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Field));
        let rows = draw(&mut gui, 100, 24);
        assert_eq!(rows[1].chars().nth(10), Some('╭'), "{}", rows.join("\n"));
        assert_eq!(rows[17].chars().nth(10), Some('╰'), "{}", rows.join("\n"));
        assert_eq!(picker_rows(&rows), refused);
        key(&mut gui, KeyCode::Backspace);
        let rows = draw(&mut gui, 100, 30);
        assert!(!rows.iter().any(|r| r.contains("Unknown variable {{ARTST}}")));
        // A refusal under a preset stands under the radios, at the text's
        // column, above the folders button.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Left);
        dest_act(&mut gui, super::DestAct::Save);
        land_save(&mut gui, Err(crate::api::PluginFailure::Refused { status: 403, words: "destination: you cannot put files into library 'music'".into() }));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[8], format!("│{:<78}│", " ( ) Custom"), "{}", picker.join("\n"));
        assert_eq!(picker[9], format!("│{:<78}│", " destination: you cannot put files into library 'music'"));
        assert_eq!(picker[10], format!("│{:<78}│", " Start from an existing folder…"));
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Library));
        // A click on the name already chosen only gives the row the hand:
        // the words stand; the other name chosen, they go.
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Save));
        dest_act(&mut gui, super::DestAct::Library(0));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[9], format!("│{:<78}│", " destination: you cannot put files into library 'music'"), "{}", picker.join("\n"));
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Library));
        dest_act(&mut gui, super::DestAct::Library(1));
        let rows = draw(&mut gui, 100, 30);
        assert!(!rows.iter().any(|r| r.contains("you cannot put files")), "{}", rows.join("\n"));
    }

    #[test]
    fn the_tallest_picker_gives_its_blanks_way_at_the_floor_and_its_notes_into_the_tooltip() {
        let _en = english();
        // Five radios, Custom valid with a slot in it: the note whole on its
        // two rows (the phone's words, never clipped to one), no tooltip on
        // it or on the path; 18 rows, the same box at 100 × 24 (rows 1–18)
        // and 100 × 30 (from row 4).
        let mut gui = dest_gui_on(dest_view_folders("", "user"));
        key(&mut gui, KeyCode::Left);
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        typed(&mut gui, "{{ARTIST}}/{{GENRE}}");
        let tall = boxed(&[
            " Files go to music › Boukmanflow › ‹genre›",
            " ‹genre› comes from the song's own tags (a song with none leaves that folder",
            " out).",
            "",
            " (•) music   ( ) copies",
            "",
            " ( ) Artist / Album",
            " ( ) Server / Artist / Album",
            " ( ) Same folders as the server",
            " ( ) Library layout",
            " (•) Custom",
            " Start from an existing folder…",
            "     {{ARTIST}}/{{GENRE}}▏",
            "     ARTIST · ALBUM · YEAR · GENRE · ALBUMARTIST · PEER · SERVERFOLDER",
            "",
            FOOT,
        ]);
        let buffer = super::tests::buffer(&mut gui, 100, 24);
        let rows = lines(&buffer);
        assert_eq!(rows[1].chars().nth(10), Some('╭'), "{}", rows.join("\n"));
        assert_eq!(rows[18].chars().nth(10), Some('╰'), "{}", rows.join("\n"));
        assert_eq!(picker_rows(&rows), tall, "{}", rows.join("\n"));
        none_cut(&picker_rows(&rows));
        assert_eq!(fg(&buffer, at(&rows, "‹genre›")), th().dim, "the slot DIM");
        assert_eq!(fg(&buffer, at(&rows, "‹genre› comes from")), th().dim);
        assert_eq!(tip_at(&gui, at(&rows, "‹genre› comes from")), None, "whole: no tooltip");
        assert_eq!(tip_at(&gui, at(&rows, "out).")), None);
        assert_eq!(tip_at(&gui, at(&rows, "music › Boukmanflow › ‹genre›")), None, "nothing clipped, the note on its rows");
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(rows[4].chars().nth(10), Some('╭'));
        assert_eq!(picker_rows(&rows), tall);
        // A three-row refusal on top: 21 rows, every blank in place.
        dest_act(&mut gui, super::DestAct::Save);
        let words = "destination layout: Unknown variable {{ARTST}}. Supported: {{ARTIST}}, {{ALBUM}}, {{YEAR}}, {{GENRE}}, {{ALBUMARTIST}}, {{PEER}}, {{SERVERFOLDER}}";
        land_save(&mut gui, Err(crate::api::PluginFailure::Refused { status: 400, words: words.into() }));
        let blank = format!("│{:<78}│", "");
        let rows = draw(&mut gui, 100, 24);
        let picker = picker_rows(&rows);
        assert_eq!(picker.len(), 21, "{}", picker.join("\n"));
        assert_eq!(picker.iter().filter(|r| **r == blank).count(), 3, "{}", picker.join("\n"));
        // The list opened on the library's root: 22 rows would pass — the
        // blank under the libraries gives way, the one under the preview and
        // the one above the buttons stand, and so do the note's two rows.
        dest_act(&mut gui, super::DestAct::Folders);
        land_folders(&mut gui);
        for h in [24, 30] {
            let rows = draw(&mut gui, 100, h);
            let picker = picker_rows(&rows);
            assert_eq!(picker.len(), 22, "{}", picker.join("\n"));
            assert_eq!((&picker[4], &picker[5], &picker[6]), (&blank, &format!("│{:<78}│", " (•) music   ( ) copies"), &format!("│{:<78}│", " ( ) Artist / Album")), "{}", picker.join("\n"));
            assert_eq!(picker[picker.len() - 3], blank);
            assert!(picker.iter().any(|r| r.contains("out).")) && picker.iter().any(|r| r.contains("Unknown variable {{ARTST}}")), "{}", picker.join("\n"));
        }
        // A folder picked: the pick is an edit, the refusal goes, the
        // blanks come back — the list under the five radios, above the
        // field and the variables.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        land_folders(&mut gui);
        let rows = draw(&mut gui, 100, 24);
        let picker = picker_rows(&rows);
        assert_eq!(picker.len(), 21, "{}", picker.join("\n"));
        assert!(!picker.iter().any(|r| r.contains("Unknown variable {{ARTST}}")), "{}", picker.join("\n"));
        assert_eq!(ui(&gui).text.value(), "from-peers/{{ARTIST}}/{{GENRE}}");
        let up = at_row(&picker, "Up to music");
        assert!(picker[usize::from(up) + 1].contains("No folders in here") && picker[usize::from(up) + 2].contains("from-peers/{{ARTIST}}/{{GENRE}}"), "{}", picker.join("\n"));
        // Back in copies the open list re-anchors on its root; into
        // from-peers (three folders): the three list rows, the field and
        // the variables stand together with every blank, whatever the
        // terminal's height.
        dest_act(&mut gui, super::DestAct::Library(1));
        land_folders(&mut gui);
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Folders));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        land_folders(&mut gui);
        for h in [24, 30] {
            let rows = draw(&mut gui, 100, h);
            let picker = picker_rows(&rows);
            assert_eq!(picker.len(), 21, "{}", picker.join("\n"));
            assert_eq!(picker.iter().filter(|r| **r == blank).count(), 3, "{}", picker.join("\n"));
            assert!(picker.iter().any(|r| r.contains("ARTIST · ALBUM")), "{}", picker.join("\n"));
            assert_eq!(picker.iter().filter(|r| r.contains("2025 archive") || r.contains("Stage Peer B") || r.contains("Up to copies")).count(), 3, "{}", picker.join("\n"));
        }
    }

    #[test]
    fn the_notes_give_way_before_the_variables_and_the_list_at_the_floor() {
        let _en = english();
        // The tallest picker there is: music's five radios, a slot's note
        // and a missing-variables note, a three-row refusal, the list open
        // inside a folder with three folders and the folder the layout
        // starts in not there yet. The blanks, the missing note, then the
        // slot note's second row go — 22 rows: the note's first row clipped
        // (its whole words its tooltip), the missing note in the path's
        // tooltip; the variables, the list's three rows, the refusal and
        // the list's note stand.
        let mut gui = undated_gui(dest_view_folders("", "user"));
        dest_act(&mut gui, super::DestAct::Library(0));
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        typed(&mut gui, "from-peers/Nope/{{ARTIST}}/{{GENRE}}/{{YEAR}}");
        dest_act(&mut gui, super::DestAct::Save);
        let words = "destination layout: Unknown variable {{ARTST}}. Supported: {{ARTIST}}, {{ALBUM}}, {{YEAR}}, {{GENRE}}, {{ALBUMARTIST}}, {{PEER}}, {{SERVERFOLDER}}";
        land_save(&mut gui, Err(crate::api::PluginFailure::Refused { status: 400, words: words.into() }));
        dest_act(&mut gui, super::DestAct::Folders);
        land_listing(&mut gui, "/music/from-peers", &["a", "b", "c"]);
        let rows = draw(&mut gui, 100, 24);
        let picker = picker_rows(&rows);
        assert_eq!(picker.len(), 22, "{}", picker.join("\n"));
        assert_eq!(rows[1].chars().nth(10), Some('╭'), "{}", rows.join("\n"));
        assert_eq!(rows[22].chars().nth(10), Some('╰'), "{}", rows.join("\n"));
        let has = |text: &str| picker.iter().filter(|r| r.contains(text)).count();
        assert_eq!(picker[1], format!("│{:<78}│", " Files go to music › from-peers › Nope › Peer Kim › ‹genre›"), "{}", picker.join("\n"));
        let note = "‹genre› comes from the song's own tags (a song with none leaves that folder out).";
        assert_eq!(picker[2], format!("│{:<78}│", " ‹genre› comes from the song's own tags (a song with none leaves that folder…"));
        assert_eq!(tip_at(&gui, at(&rows, "‹genre› comes from")).as_deref(), Some(note));
        assert_eq!((has("out)."), has("No value for")), (0, 0), "{}", picker.join("\n"));
        assert_eq!(tip_at(&gui, at(&rows, "music › from-peers › Nope")).as_deref(), Some("No value for {{YEAR}}"));
        assert_eq!(has("ARTIST · ALBUM"), 1, "{}", picker.join("\n"));
        assert_eq!((has("Up to music"), has("│   a "), has("│   b "), has("│   c ")), (1, 1, 1, 0), "{}", picker.join("\n"));
        assert_eq!((has("Unknown variable {{ARTST}}"), has("{{SERVERFOLDER}}"), has("“Nope” isn't there yet")), (1, 1, 1), "{}", picker.join("\n"));
        assert!(!picker.iter().any(|r| r == &format!("│{:<78}│", "")), "no blank left: {}", picker.join("\n"));
    }

    #[test]
    fn the_rows_give_way_in_order_until_the_picker_fits_the_floor() {
        use super::DestRow::*;
        use super::Gap::{B1, B2, B3};
        let base = |listed: usize, problems: usize| -> Vec<super::DestRow> {
            let mut rows = vec![Preview, Note(0), Note(1), Missing, Blank(B1), Library, Blank(B2)];
            rows.extend([Radio(super::Radio::ArtistAlbum), Radio(super::Radio::Custom), Folders, TreeHead]);
            rows.extend((0..listed).map(Listed));
            rows.push(Field);
            rows.extend((0..problems).map(|i| Problem(format!("p{i}"))));
            rows.extend([Vars, Blank(B3), Buttons]);
            rows
        };
        let count = |rows: &[super::DestRow], pick: fn(&super::DestRow) -> bool| rows.iter().filter(|r| pick(r)).count();
        // 20 rows fit as they are, the borders making 22.
        let mut rows = base(3, 2);
        assert_eq!(rows.len(), 20);
        let kept = rows.clone();
        super::give_way(&mut rows);
        assert_eq!(rows, kept);
        // One over: the blank under the libraries alone; two: the one under
        // the preview too; three: the one above the buttons.
        let mut rows = base(3, 3);
        super::give_way(&mut rows);
        assert_eq!(rows.len(), 20);
        assert_eq!(rows[3..7], [Missing, Blank(B1), Library, Radio(super::Radio::ArtistAlbum)]);
        let mut rows = base(3, 4);
        super::give_way(&mut rows);
        assert_eq!(rows[3..6], [Missing, Library, Radio(super::Radio::ArtistAlbum)]);
        assert!(rows.contains(&Blank(B3)));
        let mut rows = base(3, 5);
        super::give_way(&mut rows);
        assert_eq!(rows.len(), 20);
        assert!(!rows.iter().any(|r| matches!(r, Blank(_))) && rows.contains(&Missing));
        // Then the missing note, then the slot note's second row, then its
        // first — the variables and the list stand.
        let mut rows = base(3, 6);
        super::give_way(&mut rows);
        assert_eq!(rows.len(), 20);
        assert!(!rows.contains(&Missing) && rows.contains(&Note(0)) && rows.contains(&Note(1)));
        let mut rows = base(3, 7);
        super::give_way(&mut rows);
        assert_eq!(rows.len(), 20);
        assert_eq!(rows[..2], [Preview, Note(0)]);
        let mut rows = base(3, 8);
        super::give_way(&mut rows);
        assert_eq!(rows.len(), 20);
        assert!(!rows.iter().any(|r| matches!(r, Note(_))));
        assert_eq!((rows.contains(&Vars), count(&rows, |r| matches!(r, Listed(_)))), (true, 3));
        // Then the variables, then the list from its last row down to one,
        // then a GOLD block's rows from the last down to one.
        let mut rows = base(3, 9);
        super::give_way(&mut rows);
        assert_eq!(rows.len(), 20);
        assert_eq!((rows.contains(&Vars), count(&rows, |r| matches!(r, Listed(_)))), (false, 3));
        let mut rows = base(3, 11);
        super::give_way(&mut rows);
        assert_eq!(rows.len(), 20);
        assert_eq!((count(&rows, |r| matches!(r, Listed(_))), count(&rows, |r| matches!(r, Problem(_)))), (1, 11));
        assert!(rows.contains(&Listed(0)));
        let mut rows = base(3, 13);
        super::give_way(&mut rows);
        assert_eq!(rows.len(), 20);
        assert_eq!((count(&rows, |r| matches!(r, Listed(_))), count(&rows, |r| matches!(r, Problem(_)))), (1, 11));
        assert!(rows.contains(&Problem("p0".into())) && rows.contains(&Problem("p10".into())) && !rows.contains(&Problem("p11".into())));
        // The preview, the field and the buttons never give way.
        assert!(rows.contains(&Preview) && rows.contains(&Field) && rows.contains(&Buttons));
    }

    /// Keep a copy on a song with no year, at the top of its server's
    /// library, its place read again as `view`, the picker open.
    fn undated_gui(view: Value) -> Gui {
        let mut gui = plugin_gui();
        key(&mut gui, KeyCode::Char('G'));
        let (seq, generation) = pending_taste(&gui, TastePart::Main);
        apply(&mut gui, Event::Taste { seq, generation, part: TastePart::Main, answer: Ok(Box::new(main_answer())) });
        let mut bare = fed(B, "Bare Song", "Peer Kim", "Loose Ends", 0.9, Some(30.0));
        bare["year"] = Value::Null;
        bare["filepath"] = json!("peerlib/Bare Song.mp3");
        let answer: TasteResponse = serde_json::from_value(json!({
            "seedSource": "history", "seeds": [], "library": null, "network": null,
            "peers": { "peers": [ { "id": B, "name": name(B), "reachable": true, "results": [bare] } ], "searched": { "peers": 1 } }
        }))
        .unwrap();
        let (seq, generation) = pending_taste(&gui, TastePart::Peers);
        apply(&mut gui, Event::Taste { seq, generation, part: TastePart::Peers, answer: Ok(Box::new(answer)) });
        let generation = gui.app.session_gen();
        apply(
            &mut gui,
            Event::KeepGate {
                generation,
                plugins: Ok(Box::new(serde_json::from_value(listing(true)).unwrap())),
                destination: Some(Ok(Box::new(serde_json::from_value(destination()).unwrap()))),
            },
        );
        land_lookups(&mut gui, &[]);
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Row { group: 0, row: 0 });
        land_read(&mut gui, view);
        key(&mut gui, KeyCode::Char('d'));
        gui
    }

    #[test]
    fn the_preview_names_the_variables_this_song_has_no_value_for_and_gives_the_row_way_last() {
        let _en = english();
        let mut gui = undated_gui(dest_view_folders("", "user"));
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        typed(&mut gui, "{{ARTIST}}/{{YEAR}}/{{ALBUM}}");
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], format!("│{:<78}│", " Files go to copies › Peer Kim › Loose Ends"), "{}", picker.join("\n"));
        assert_eq!(picker[2], format!("│{:<78}│", " No value for {{YEAR}}"));
        assert_eq!(fg(&buffer, at(&rows, "No value for")), th().dim);
        assert_eq!(tip_at(&gui, at(&rows, "No value for")), None, "whole: no tooltip");
        none_cut(&picker);
        // A song at the top of its server's library sits in no folder there.
        ctrl(&mut gui, 'u');
        typed(&mut gui, "{{SERVERFOLDER}}");
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], format!("│{:<78}│", " Files go to copies"), "{}", picker.join("\n"));
        assert_eq!(picker[2], format!("│{:<78}│", " No value for {{SERVERFOLDER}} · it sits in no folder on its server"));
        assert_eq!(tip_at(&gui, at(&rows, "No value for")), None);
        // Both notes under music's five radios and a three-row refusal: 22
        // rows exactly, nothing gives way; the list opened on top, the
        // blanks under the libraries and the preview go — both notes stand.
        dest_act(&mut gui, super::DestAct::Library(0));
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Field));
        ctrl(&mut gui, 'u');
        typed(&mut gui, "{{ARTIST}}/{{GENRE}}/{{YEAR}}");
        dest_act(&mut gui, super::DestAct::Save);
        let words = "destination layout: Unknown variable {{ARTST}}. Supported: {{ARTIST}}, {{ALBUM}}, {{YEAR}}, {{GENRE}}, {{ALBUMARTIST}}, {{PEER}}, {{SERVERFOLDER}}";
        land_save(&mut gui, Err(crate::api::PluginFailure::Refused { status: 400, words: words.into() }));
        let blank = format!("│{:<78}│", "");
        let rows = draw(&mut gui, 100, 24);
        let picker = picker_rows(&rows);
        assert_eq!(picker.len(), 22, "{}", picker.join("\n"));
        assert_eq!(picker.iter().filter(|r| **r == blank).count(), 3, "{}", picker.join("\n"));
        assert_eq!(picker[4], format!("│{:<78}│", " No value for {{YEAR}}"), "{}", picker.join("\n"));
        dest_act(&mut gui, super::DestAct::Folders);
        land_folders(&mut gui);
        let rows = draw(&mut gui, 100, 24);
        let picker = picker_rows(&rows);
        assert_eq!(picker.len(), 22, "{}", picker.join("\n"));
        assert_eq!(picker.iter().filter(|r| **r == blank).count(), 1, "{}", picker.join("\n"));
        assert!(picker.iter().any(|r| r.contains("‹genre› comes from")) && picker.iter().any(|r| r.contains("No value for {{YEAR}}")), "{}", picker.join("\n"));
        assert_eq!(tip_at(&gui, at(&rows, "music › Peer Kim › ‹genre›")), None, "{:?}", gui.ui.tips);
    }

    #[test]
    fn the_legacy_console_draws_where_copies_land_in_its_own_glyphs() {
        let _en = english();
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let mut gui = dest_gui("");
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[0], format!("+- Where copies land {}+", "-".repeat(58)));
        assert_eq!(picker[1], format!("|{:<78}|", " Files go to copies > Boukmanflow > Borrowed Tapes"));
        assert_eq!(picker[3], format!("|{:<78}|", " ( ) music   (*) copies"));
        assert_eq!(picker[5], format!("|{:<78}|", " (*) Artist / Album"));
        assert_eq!(picker[8], format!("|{:<78}|", " Start from an existing folder..."));
        assert_eq!(picker[11], format!("+{} Esc close -+", "-".repeat(66)));
        // Custom chosen: the preview's missing path the kit's `-`, the
        // hint's dots and the open list's "Looking up..." in the console's
        // own.
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], format!("|{:<78}|", " Files go to -"), "{}", picker.join("\n"));
        assert!(!picker.iter().any(|r| r.contains('…')), "{}", picker.join("\n"));
        dest_act(&mut gui, super::DestAct::Folders);
        land_folders(&mut gui);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert!(picker.iter().any(|r| r.starts_with("|   Looking up...")), "{}", picker.join("\n"));
        assert!(!picker.iter().any(|r| r.contains('…')), "{}", picker.join("\n"));
        key(&mut gui, KeyCode::Esc);
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Field));
        // The caret and Saving's dots in the console's own.
        typed(&mut gui, "x");
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("|     from-peersx│")), "{}", rows.join("\n"));
        key(&mut gui, KeyCode::Enter);
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("[ Saving... ] |")), "{}", rows.join("\n"));
    }

    #[test]
    fn a_wide_glyph_left_of_the_picker_never_hides_its_left_border() {
        // The room's nav beside the picker in a wide locale: a CJK glyph
        // across cols 9–10 would hide the border at col 10 (a terminal
        // never draws the cell a wide glyph spans) — col 9 is a blank there.
        let Some(code) = crate::setup::tests::in_own_process(&["ja", "zh"]) else { return };
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let all = lines(&buffer).join("\n");
        let top = (0..30).find(|&y| buffer[(10, y)].symbol() == "╭").unwrap_or_else(|| panic!("{code}: the picker\n{all}"));
        let bottom = (top..30).find(|&y| buffer[(10, y)].symbol() == "╰").unwrap_or_else(|| panic!("{code}: its bottom\n{all}"));
        for y in top..=bottom {
            assert!(crate::kit::width(buffer[(9, y)].symbol()) <= 1, "{code}: row {y} spills onto the border\n{all}");
            assert!(matches!(buffer[(10, y)].symbol(), "╭" | "│" | "╰"), "{code}: row {y}\n{all}");
        }
    }

    #[test]
    fn from_a_tile_every_album_by_previews_one_folder_per_album_and_no_file() {
        let _en = english();
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        stage_facts(&mut gui);
        land_read(&mut gui, dest_view("", "user"));
        choose(&mut gui, Some("artist"));
        key(&mut gui, KeyCode::Char('d'));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], format!("│{:<78}│", " Files go to copies › Boukmanflow › one folder per album"), "{}", picker.join("\n"));
        none_cut(&picker);
        // No answer to a save: the words GOLD under the radios.
        key(&mut gui, KeyCode::Left);
        dest_act(&mut gui, super::DestAct::Save);
        land_save(&mut gui, Err(crate::api::PluginFailure::Unreachable("connection refused".into())));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[9], format!("│{:<78}│", " your server didn't answer"), "{}", picker.join("\n"));
    }

    #[test]
    fn the_variables_row_draws_every_one_inside_the_border_in_every_locale() {
        let codes = ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"];
        let Some(code) = crate::setup::tests::in_own_process(&codes) else { return };
        let six = ["ARTIST", "ALBUM", "YEAR", "GENRE", "ALBUMARTIST", "PEER"];
        let seven = ["ARTIST", "ALBUM", "YEAR", "GENRE", "ALBUMARTIST", "PEER", "SERVERFOLDER"];
        // The six, and the seven of a server that lists SERVERFOLDER (65
        // cells): under Custom's field from col 5, never one lost.
        for (view, names) in [(dest_view("", "user"), &six[..]), (dest_view_folders("", "user"), &seven[..])] {
            let mut gui = dest_gui_on(view.clone());
            dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
            let buffer = super::tests::buffer(&mut gui, 100, 30);
            // The picker's content cells, cols 11–87 (its border at 10 and 89).
            let row = |y: u16| (11..88).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>();
            let y = (0..30).find(|&y| row(y).contains("ARTIST · ALBUM")).unwrap_or_else(|| panic!("{code}: no variables row"));
            let cells = row(y);
            for (i, name) in names.iter().enumerate() {
                let token = if *name == "ALBUM" { "ALBUM ·" } else { name };
                let byte = cells.find(token).unwrap_or_else(|| panic!("{code}: {name} missing from {cells:?}"));
                let at = cells[..byte].chars().count() as u16 + 11;
                assert_eq!(dest_hit(&gui, Position { x: at, y }), Some(super::DestAct::Var(i)), "{code}: {name} at {at}");
                assert!(at + name.len() as u16 - 1 <= 87, "{code}: {name} past the last cell");
            }
            assert!(!cells.trim_end().ends_with('·'), "{code}: no stray separator: {cells:?}");
            assert!(cells.starts_with("     ARTIST · "), "{code}: under the field: {cells:?}");
            // Every radio's words whole at col 5 and nothing after them:
            // no layout beside a preset (a wide locale's cells hold a
            // blank after each CJK glyph in the buffer: compared without).
            for (_, key, _) in super::presets(&serde_json::from_value(view.clone()).unwrap(), "copies") {
                let words = rust_i18n::t!(key).to_string();
                let packed = words.replace(' ', "");
                let y = (0..30).find(|&y| row(y).replace(' ', "").contains(&packed)).unwrap_or_else(|| panic!("{code}: {words} missing"));
                let shown = row(y);
                assert!(shown.starts_with(" ( ) "), "{code}: {shown:?}");
                assert!(shown.trim_end().replace(' ', "").ends_with(&packed) && !shown.contains("{{"), "{code}: {shown:?}");
            }
        }
    }

    #[test]
    fn space_presses_the_pickers_buttons_as_enter_does_and_tab_walks() {
        let _en = english();
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Left);
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Save));
        key(&mut gui, KeyCode::Char(' '));
        assert_eq!(saves(&gui).len(), 1, "Save on Space");
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::UseDefault));
        key(&mut gui, KeyCode::Char(' '));
        assert_eq!(ui(&gui).vpath, "music", "the default on Space");
        // ← → move between the buttons — the tips say Enter saves only while
        // Save is on (clause 119); Tab walks every stop to Save and
        // Shift-Tab back to LIBRARY.
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Save));
        key(&mut gui, KeyCode::Left);
        assert_eq!(ui(&gui).hand, Some(super::DestStop::UseDefault));
        assert_eq!(super::super::discover::tips(&gui), "← → buttons · Enter use the library default · Esc close");
        key(&mut gui, KeyCode::Right);
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Save));
        assert_eq!(super::super::discover::tips(&gui), "← → buttons · Esc close", "Save off: Enter does nothing");
        key(&mut gui, KeyCode::Enter);
        assert!(saves(&gui).is_empty());
        dest_act(&mut gui, super::DestAct::Library(0));
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Save));
        assert_eq!(super::super::discover::tips(&gui), "← → buttons · Enter save · Esc close");
        let mut gui = dest_gui("");
        for _ in 0..10 {
            key(&mut gui, KeyCode::Tab);
        }
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Save));
        for _ in 0..10 {
            key(&mut gui, KeyCode::BackTab);
        }
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Library));
    }

    #[test]
    fn with_one_library_its_row_is_no_stop_and_a_hand_whose_stop_left_lands_on_the_first() {
        let _en = english();
        // One library to add files to: no row for it — the path names it —
        // and no blank under it; the hand on the first radio.
        let mut view = dest_view("", "user");
        view["libraries"] = json!([{ "vpath": "copies", "template": null }]);
        let mut gui = keep_on(view);
        key(&mut gui, KeyCode::Char('d'));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(
            picker,
            boxed(&[
                " Files go to copies › Boukmanflow › Borrowed Tapes",
                "",
                " (•) Artist / Album",
                " ( ) Server / Artist / Album",
                " ( ) Custom",
                " Start from an existing folder…",
                "",
                FOOT,
            ]),
            "{}",
            picker.join("\n")
        );
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Radio(super::Radio::ArtistAlbum)), "the first radio instead");
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · Space or Enter pick · Esc close");
        // One listed library and a chosen one the server no longer lists:
        // two choices drawn, so the row is a stop — ← → reach both, a click
        // on either takes the hand.
        let mut view = dest_view("", "user");
        view["libraries"] = json!([{ "vpath": "music", "template": null }]);
        let mut gui = keep_on(view);
        key(&mut gui, KeyCode::Char('d'));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[3], format!("│{:<78}│", " ( ) music   (•) copies"), "{}", picker.join("\n"));
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Library));
        assert_eq!(dest_hit(&gui, at(&rows, "(•) copies")), Some(super::DestAct::Library(1)));
        key(&mut gui, KeyCode::Left);
        assert_eq!(ui(&gui).vpath, "music");
        // Left behind, the unlisted library is gone from the row (the
        // webapp's chips): one choice, no row, the hand on the first radio.
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], format!("│{:<78}│", " Files go to music › Boukmanflow › Borrowed Tapes"), "{}", picker.join("\n"));
        assert_eq!(picker[3], format!("│{:<78}│", " (•) Artist / Album"), "{}", picker.join("\n"));
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Radio(super::Radio::ArtistAlbum)));
        // music's own layout held, then a read without it: the hand goes
        // to the first stop, and the tips say its keys — Enter saves, the
        // layout being an edit.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Left);
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Template));
        let mut plain = dest_view("", "user");
        plain["libraries"] = json!([{ "vpath": "music", "template": null }, { "vpath": "copies", "template": null }]);
        let generation = gui.app.session_gen();
        let seq = gui.app.keep_dest_read().iter().find_map(|e| match e {
            Effect::Api(ApiCmd::DestinationRead { seq, .. }) => Some(*seq),
            _ => None,
        });
        apply(&mut gui, Event::DestinationRead { seq: seq.unwrap(), generation, answer: Ok(Box::new(serde_json::from_value(plain).unwrap())) });
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Library));
        assert_eq!(super::super::discover::tips(&gui), "← → library · Enter save · ↑ ↓ rows · Esc close");
        // The held layout is no preset any more and Custom was never picked:
        // no radio is marked, as the webapp marks none.
        let picker = picker_rows(&rows);
        assert_eq!(picker[3], format!("│{:<78}│", " (•) music   ( ) copies"), "{}", picker.join("\n"));
        assert!(!picker.iter().skip(4).any(|r| r.contains("(•)")), "{}", picker.join("\n"));
    }

    #[test]
    fn a_saves_answer_is_its_own_pickers_and_a_refusal_after_it_closed_is_a_note() {
        let _en = english();
        let refusal = || crate::api::PluginFailure::Refused { status: 403, words: "destination: you cannot put files into library 'music'".into() };
        // Esc while saving, then `d` again: the new picker is no "saving…",
        // its keys work, and the old refusal is the status line's.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Left);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(saves(&gui).len(), 1, "Enter on LIBRARY saves while Save is on");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.keep.dest.is_none(), "Esc still goes back");
        key(&mut gui, KeyCode::Char('d'));
        let rows = draw(&mut gui, 100, 30);
        assert!(!rows.iter().any(|r| r.contains("saving")), "{}", rows.join("\n"));
        assert_eq!(super::super::discover::tips(&gui), "← → library · ↑ ↓ rows · Esc close");
        key(&mut gui, KeyCode::Down);
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Radio(super::Radio::ArtistAlbum)));
        land_save(&mut gui, Err(refusal()));
        let rows = draw(&mut gui, 100, 30);
        assert!(ui(&gui).error.is_none(), "the new picker keeps its own fields");
        assert!(!picker_rows(&rows).iter().any(|r| r.contains("couldn't save")));
        let note = gui.app.message.as_ref().map(|m| (m.text.clone(), m.kind.clone())).expect("a note");
        assert!(note.0.starts_with("couldn't save: destination"), "{note:?}");
        assert_eq!(note.1, crate::tui::app::MessageKind::Error);
        // Saved after its picker went: the new picker stays open, the place
        // is the answer's — Keep a copy reads it once the picker closes.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Left);
        key(&mut gui, KeyCode::Enter);
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Char('d'));
        let mut music = dest_view("", "user");
        music["destination"] = json!({ "vpath": "music", "base": "", "layout": "{{ALBUMARTIST}}/{{ALBUM}}", "source": "user" });
        land_save(&mut gui, Ok(music));
        draw(&mut gui, 100, 30);
        assert!(gui.discover.keep.dest.is_some(), "never closed by another picker's answer");
        key(&mut gui, KeyCode::Esc);
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("Files go to music › Boukmanflow ›")), "{}", rows.join("\n"));
    }

    /// The destination read again (another tab saved one), answered as
    /// `view`.
    fn read_again(gui: &mut Gui, view: Value) {
        let generation = gui.app.session_gen();
        let effects = gui.app.keep_dest_read();
        let seq = effects
            .iter()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::DestinationRead { seq, .. }) => Some(*seq),
                _ => None,
            })
            .expect("a read");
        apply(gui, Event::DestinationRead { seq, generation, answer: Ok(Box::new(serde_json::from_value(view).unwrap())) });
    }

    #[test]
    fn the_library_row_never_draws_past_the_border_and_its_mark_steps_left() {
        let _en = english();
        let on = |long: &str| {
            let mut view = dest_view("", "user");
            view["libraries"] = json!([{ "vpath": long, "template": null }, { "vpath": "copies", "template": null }]);
            view["destination"]["vpath"] = json!(long);
            view["saved"]["vpath"] = json!(long);
            let mut gui = keep_on(view);
            key(&mut gui, KeyCode::Char('d'));
            gui
        };
        // The chosen library 67 cells wide: the other has no room for a dot,
        // a space and a cell of its name, so it is not drawn — the border
        // stands, nothing lands on the room beneath — and ← still reaches
        // it; from there the row starts at the chosen one behind `…`, whose
        // cell steps back to the one before.
        let long = "a".repeat(67);
        let mut gui = on(&long);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[3], format!("│{:<78}│", format!(" (•) {long}")), "{}", picker.join("\n"));
        let y = usize::from(at_row(&rows, "(•) aaa"));
        assert_eq!(rows[y].chars().nth(89), Some('│'));
        assert!(!rows[y].contains("( )") && !rows[y].chars().skip(90).any(|c| c == '…'), "{}", rows[y]);
        key(&mut gui, KeyCode::Left);
        assert_eq!(ui(&gui).vpath, "copies");
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[3], format!("│{:<78}│", " … (•) copies"), "{}", picker.join("\n"));
        let mark = at(&rows, "… (•) copies");
        assert_eq!(mark.x, 12);
        assert_eq!(dest_hit(&gui, mark), Some(super::DestAct::Step(-1)));
        gui.act(gui.ui.hit(mark).expect("the mark"));
        assert_eq!(ui(&gui).vpath, long);
        // 60 cells: the second has room for its dot and four cells of its
        // name, clipped at the last cell — never over the border.
        let long = "a".repeat(60);
        let mut gui = on(&long);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[3], format!("│{:<78}│", format!(" (•) {long}   ( ) copi…")), "{}", picker.join("\n"));
        assert_eq!(dest_hit(&gui, at(&rows, "( ) copi…")), Some(super::DestAct::Library(1)));
    }

    #[test]
    fn a_path_longer_than_the_server_takes_is_said_for_the_song_the_album_and_the_artist() {
        let _en = english();
        // A 490-character folder and the album: the layout passes the
        // validator (500 at most), the song's path would not — GOLD words in
        // the preview row's place, no path; Save on: the server takes the
        // layout.
        let too_long = format!("│{:<78}│", " Too long for the server · over 500 characters");
        let long = "x".repeat(490);
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        typed(&mut gui, &format!("{long}/{{{{ALBUM}}}}"));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], too_long, "{}", picker.join("\n"));
        assert_eq!(picker[2], format!("│{:<78}│", ""));
        assert_eq!(fg(&buffer, at(&rows, "Too long for the server")), th().gold);
        assert_eq!(fg(&buffer, at(&rows, "[ Save ]")), th().accent);
        assert_eq!(tip_at(&gui, at(&rows, "Too long for the server")), None, "whole on its row: no tooltip");
        none_cut(&picker);
        // An album's copy, and every album of an artist — rendered whole,
        // never cut at the album folder, as the server measures it: the same
        // words for every copy.
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        stage_facts(&mut gui);
        land_read(&mut gui, dest_view("", "user"));
        key(&mut gui, KeyCode::Char('d'));
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        typed(&mut gui, &format!("{long}/{{{{ALBUM}}}}"));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], too_long, "{}", picker.join("\n"));
        // Every album by the artist: the album's folder is each album's
        // own and drops from the measure (no value, as the webapp's
        // `previewTarget` renders it), so 488 cells after the artist's 11
        // are the 500 exactly, and one more passes them.
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        stage_facts(&mut gui);
        land_read(&mut gui, dest_view("", "user"));
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Pick(crate::tui::app::keep::KeepChoice::Artist))));
        key(&mut gui, KeyCode::Char('d'));
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        typed(&mut gui, &format!("{{{{ARTIST}}}}/{}", "x".repeat(488)));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert!(picker[1].starts_with("│ Files go to …xxxx"), "the lead stands, the path clipped after it: {}", picker.join("\n"));
        typed(&mut gui, "x");
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], too_long, "{}", picker.join("\n"));
        // A slot has no value of its own and drops from the measure, as
        // the webapp's previewTarget renders it: the song's artist and
        // album, its genre's slot and 469 cells are 496 — a path (the
        // slot's own words would pass the 500), its note whole under it.
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        typed(&mut gui, &format!("{{{{ARTIST}}}}/{{{{ALBUM}}}}/{{{{GENRE}}}}/{}", "x".repeat(469)));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert!(picker[1].starts_with("│ Files go to …xxxx"), "{}", picker.join("\n"));
        assert_eq!(picker[2], format!("│{:<78}│", " ‹genre› comes from the song's own tags (a song with none leaves that folder"));
        assert_eq!(picker[3], format!("│{:<78}│", " out)."));
        // A base folder set elsewhere is not measured (the server never
        // files by it): 29 cells of it and a layout whose path is 477 — a
        // path, clipped from the left, whole in its tooltip.
        let layout = format!("{}/{{{{ARTIST}}}}/{{{{ALBUM}}}}", "y".repeat(450));
        let mut view = dest_view(&"b".repeat(29), "user");
        view["destination"]["layout"] = json!(layout);
        view["saved"]["layout"] = json!(layout);
        let mut gui = dest_gui_on(view);
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let picker = picker_rows(&rows);
        assert!(picker[1].starts_with("│ Files go to …") && picker[1].ends_with("› Boukmanflow › Borrowed Tapes │"), "{}", picker.join("\n"));
        assert_eq!(
            tip_at(&gui, at(&rows, "Files go to …")),
            Some(format!("copies/{}/{}/Boukmanflow/Borrowed Tapes", "b".repeat(29), "y".repeat(450)))
        );
        assert_eq!(fg(&buffer, at(&rows, "[ Save ]")), th().dim, "nothing dirty");
        // Every locale's words hold the row whole.
        for code in ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
            let words = rust_i18n::t!("gui.discover.dest.too_long", locale = code, max = 500).to_string();
            assert!(crate::kit::width(&words) <= 76, "{code}: {words}");
            let lead = rust_i18n::t!("gui.discover.keep.goes_to", locale = code).to_string();
            assert!(crate::kit::width(&lead) <= 24, "{code}: the lead leaves the path its cells: {lead}");
        }
    }

    #[test]
    fn the_previews_clip_touches_the_words_it_cut_into() {
        let _en = english();
        let shown = |parts: &[&str], room: usize| {
            use ratatui::{Terminal, backend::TestBackend};
            let runs: Vec<(String, ratatui::style::Style)> = parts.iter().map(|p| (p.to_string(), ratatui::style::Style::default())).collect();
            let mut terminal = Terminal::new(TestBackend::new(40, 1)).unwrap();
            terminal
                .draw(|frame| {
                    super::put_tail(frame, 0, 0, &runs, room);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            (0..40).map(|x| buffer[(x, 0)].symbol()).collect::<String>().trim_end().to_string()
        };
        // A blank inside a name the cut lands on goes, a cell short.
        assert_eq!(shown(&["copies", " › ", "that keeps going"], 13), "…keeps going");
        // A separator's blank before a whole crumb stays, as the cut at
        // the separator itself does.
        assert_eq!(shown(&["copies", " › ", "Lanterns"], 10), "… Lanterns");
        assert_eq!(shown(&["copies", " › ", "Lanterns"], 12), "… › Lanterns");
        // Whole where it fits.
        assert_eq!(shown(&["copies", " › ", "Lanterns"], 17), "copies › Lanterns");
    }

    #[test]
    fn the_pickers_one_row_notes_fit_their_rows_in_every_locale() {
        let _en = english();
        // The rows' cells: the text's from col 1, the open list's note's
        // from col 3, the GOLD lines' under Custom's field from col 5 — all
        // to col 76.
        let wide = usize::from(super::DEST_LAST + 1 - super::DEST_TEXT);
        let tree = usize::from(super::DEST_LAST + 1 - super::DEST_TREE);
        let field = usize::from(super::DEST_LAST + 1 - super::DEST_FIELD);
        for code in ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
            // An item at its server's top under Same folders as the server.
            let words = rust_i18n::t!("gui.discover.dest.missing_server", locale = code, vars = "{{SERVERFOLDER}}").to_string();
            assert!(crate::kit::width(&words) <= wide, "{code}: {words}");
            // A folder of 16 characters with a brace, pressed in the list.
            let words = rust_i18n::t!("gui.discover.dest.folder_braces", locale = code, folder = "{{odd}} mixtapes").to_string();
            assert!(crate::kit::width(&words) <= tree, "{code}: {words}");
            // A base folder set elsewhere that reads as a variable: one row.
            let words = rust_i18n::t!("gui.discover.dest.err_base_braces", locale = code, folder = "{{PEER}}").to_string();
            assert!(crate::kit::width(&words) <= field, "{code}: {words}");
        }
    }

    #[test]
    fn the_picker_takes_a_place_read_again_while_nothing_is_edited_and_goes_with_nowhere() {
        let _en = english();
        // Another tab saved a place: opened on the old one with nothing
        // edited, the picker takes it — Custom with its text, the hand
        // kept, nothing dirty.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Down);
        let mut moved = dest_view("", "user");
        moved["destination"]["layout"] = json!("From friends/{{ARTIST}}");
        moved["saved"]["layout"] = json!("From friends/{{ARTIST}}");
        read_again(&mut gui, moved);
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let picker = picker_rows(&rows);
        assert_eq!(picker[1], format!("│{:<78}│", " Files go to copies › From friends › Boukmanflow"), "{}", picker.join("\n"));
        assert!(picker[7].contains("(•) Custom") && picker[9].contains("From friends/{{ARTIST}}"), "{}", picker.join("\n"));
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Radio(super::Radio::ArtistAlbum)));
        assert_eq!(fg(&buffer, at(&rows, "[ Save ]")), th().dim, "nothing dirty: the place is the new one");
        // An edit is kept for Save or Esc to settle: music chosen, then the
        // old place read again — music stays.
        dest_act(&mut gui, super::DestAct::Library(0));
        read_again(&mut gui, dest_view("", "user"));
        draw(&mut gui, 100, 30);
        assert_eq!((ui(&gui).vpath.as_str(), ui(&gui).text.value()), ("music", "From friends/{{ARTIST}}"));
        // The open list and the held variable ride through a sync.
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Folders);
        land_folders(&mut gui);
        let mut moved = dest_view("", "user");
        moved["destination"]["layout"] = json!("{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}");
        moved["saved"]["layout"] = json!("{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}");
        read_again(&mut gui, moved);
        let rows = draw(&mut gui, 100, 30);
        assert!(ui(&gui).list.is_some() && ui(&gui).hand == Some(super::DestStop::Folders));
        assert!(rows.iter().any(|r| r.contains("(•) Server / Artist / Album")) && rows.iter().any(|r| r.contains("Hide folders")), "{}", rows.join("\n"));
        // Nowhere any more: nothing to edit — the picker goes, Keep a copy
        // says copies are off.
        let mut gui = dest_gui("");
        let mut gone = dest_view("", "user");
        gone["destination"] = Value::Null;
        gone["saved"] = Value::Null;
        read_again(&mut gui, gone);
        let rows = draw(&mut gui, 100, 30);
        assert!(gui.discover.keep.dest.is_none());
        assert!(rows.iter().any(|r| r.contains("copies are off")), "{}", rows.join("\n"));
    }

    #[test]
    fn a_click_elsewhere_closes_the_list_up_leaves_it_and_a_library_switch_re_anchors_it() {
        let _en = english();
        let listed = |gui: &Gui| -> Vec<String> {
            gui.pending
                .iter()
                .filter_map(|e| match e {
                    Effect::Api(ApiCmd::DestinationFolders { path, .. }) => Some(path.clone()),
                    _ => None,
                })
                .collect()
        };
        // A radio clicked with the list open closes it; so does a click on
        // the field's row (the hand moves), and one on "Esc close", which
        // closes the list alone — a library chosen, a variable or Use the
        // library default keep it open.
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Folders);
        land_folders(&mut gui);
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Server));
        assert!(ui(&gui).list.is_none());
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        dest_act(&mut gui, super::DestAct::Folders);
        dest_act(&mut gui, super::DestAct::Library(0));
        dest_act(&mut gui, super::DestAct::Var(0));
        dest_act(&mut gui, super::DestAct::UseDefault);
        assert!(ui(&gui).list.is_some(), "a library, a variable and the default keep it open");
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Field));
        assert!(ui(&gui).list.is_none(), "a click on the field's row closes it");
        dest_act(&mut gui, super::DestAct::Folders);
        let rows = draw(&mut gui, 100, 30);
        gui.act(gui.ui.hit(at(&rows, "Esc close")).expect("Esc close"));
        assert!(gui.discover.keep.dest.is_some() && ui(&gui).list.is_none(), "\"Esc close\" closes the list first");
        let rows = draw(&mut gui, 100, 30);
        gui.act(gui.ui.hit(at(&rows, "Esc close")).expect("Esc close"));
        assert!(gui.discover.keep.dest.is_none() && saves(&gui).is_empty(), "then goes back, nothing saved");
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Folders);
        land_folders(&mut gui);
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Server));
        // ↓ into the list, ↑ from its first row back to the button.
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Folders));
        dest_act(&mut gui, super::DestAct::Folders);
        land_folders(&mut gui);
        key(&mut gui, KeyCode::Down);
        assert_eq!(ui(&gui).list.as_ref().and_then(|l| l.held), Some(0));
        key(&mut gui, KeyCode::Up);
        assert_eq!((ui(&gui).list.as_ref().and_then(|l| l.held), ui(&gui).hand), (None, Some(super::DestStop::Folders)));
        // Into from-peers, then another library: the list re-anchors on
        // the folder the layout starts in there — its root, asked once.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        land_folders(&mut gui);
        assert_eq!(ui(&gui).list.as_ref().map(|l| l.dir.clone()), Some(vec!["from-peers".to_string()]));
        assert_eq!(ui(&gui).text.value(), "from-peers/{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}");
        dest_act(&mut gui, super::DestAct::Library(0));
        assert_eq!(ui(&gui).list.as_ref().map(|l| l.dir.clone()), Some(Vec::new()), "re-anchored on music's root");
        assert_eq!(listed(&gui), vec!["/music".to_string()]);
        land_folders(&mut gui);
        // Back to Keep a copy and in again: the folders are asked afresh.
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.keep.dest.is_none());
        key(&mut gui, KeyCode::Char('d'));
        dest_act(&mut gui, super::DestAct::Folders);
        assert_eq!(listed(&gui), vec!["/copies".to_string()], "a reopened picker asks again");
    }

    #[test]
    fn a_variable_with_no_room_left_in_the_layout_goes_nowhere() {
        let _en = english();
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        typed(&mut gui, &"x".repeat(495));
        dest_act(&mut gui, super::DestAct::Var(0));
        assert_eq!(ui(&gui).text.value().chars().count(), 495, "{{{{ARTIST}}}} would pass 500");
        // One that fits goes in at the caret.
        ctrl(&mut gui, 'u');
        typed(&mut gui, &"x".repeat(490));
        dest_act(&mut gui, super::DestAct::Var(5));
        assert_eq!(ui(&gui).text.value(), format!("{}{{{{PEER}}}}", "x".repeat(490)));
    }

    #[test]
    fn every_control_the_trim_kept_works_by_its_click_and_by_its_key() {
        let _en = english();
        let click = |gui: &mut Gui, text: &str| {
            let rows = draw(gui, 100, 30);
            let act = gui.ui.hit(at(&rows, text)).unwrap_or_else(|| panic!("{text:?}: nothing to click"));
            assert_ne!(act, Act::Guard, "{text:?} takes no click");
            gui.act(act);
        };
        // By the pointer: a library by its name, a radio by its name, Custom
        // by its row — the field takes the keys — every variable by its
        // name at the caret, the folders button and a folder's row, "Hide
        // folders", Use the library default and Save.
        let mut gui = dest_gui("");
        click(&mut gui, "( ) music");
        assert_eq!((ui(&gui).vpath.as_str(), ui(&gui).hand), ("music", Some(super::DestStop::Library)));
        click(&mut gui, "( ) Server / Artist / Album");
        assert_eq!((ui(&gui).custom, ui(&gui).layout().as_str()), (false, "{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}"));
        click(&mut gui, "( ) Library layout");
        assert_eq!(ui(&gui).layout(), "{{ARTIST}}/{{YEAR}} - {{ALBUM}}");
        click(&mut gui, "( ) Custom");
        assert!(ui(&gui).custom && ui(&gui).hand == Some(super::DestStop::Field));
        // (A genre's slot brings its note: the rows below move down.)
        let mut x = 16;
        for (i, name) in ["ARTIST", "ALBUM", "YEAR", "GENRE", "ALBUMARTIST", "PEER"].iter().enumerate() {
            let rows = draw(&mut gui, 100, 30);
            let y = at_row(&rows, "ARTIST · ALBUM");
            let act = gui.ui.hit(Position { x, y }).expect("a variable");
            assert_eq!(act, Act::Disc(DiscAct::Keep(super::KeepAct::Dest(super::DestAct::Var(i)))), "{name} at {x}");
            gui.act(act);
            x += name.len() as u16 + 3;
        }
        assert_eq!(ui(&gui).text.value(), "{{ARTIST}}{{ALBUM}}{{YEAR}}{{GENRE}}{{ALBUMARTIST}}{{PEER}}");
        assert_eq!(ui(&gui).hand, Some(super::DestStop::Field), "the keys back in the field");
        ctrl(&mut gui, 'u');
        click(&mut gui, "Start from an existing folder…");
        land_folders(&mut gui);
        assert!(ui(&gui).list.is_some());
        click(&mut gui, "from-peers");
        land_folders(&mut gui);
        assert_eq!(ui(&gui).list.as_ref().map(|l| l.dir.clone()), Some(vec!["from-peers".to_string()]));
        assert!(ui(&gui).text.value().starts_with("from-peers"), "{}", ui(&gui).text.value());
        click(&mut gui, "Hide folders");
        assert!(ui(&gui).list.is_none());
        click(&mut gui, "[ Use the library default ]");
        assert_eq!((ui(&gui).vpath.as_str(), ui(&gui).custom), ("music", false));
        click(&mut gui, "[ Save ]");
        assert_eq!(saves(&gui), vec![None]);
        // By the keyboard, the same stops in the same order: ← → the
        // libraries, ↓ the radios and Space, the folders button and Enter,
        // the field, the variables and Enter, the buttons and Enter.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Left);
        assert_eq!(ui(&gui).vpath, "music");
        key(&mut gui, KeyCode::Right);
        let walked: Vec<_> = (0..4)
            .map(|_| {
                key(&mut gui, KeyCode::Down);
                ui(&gui).hand
            })
            .collect();
        use super::{DestStop, Radio};
        assert_eq!(walked, [Some(DestStop::Radio(Radio::ArtistAlbum)), Some(DestStop::Radio(Radio::Server)), Some(DestStop::Radio(Radio::Custom)), Some(DestStop::Folders)]);
        key(&mut gui, KeyCode::Enter);
        land_folders(&mut gui);
        assert!(ui(&gui).list.is_some());
        key(&mut gui, KeyCode::Esc);
        assert!(ui(&gui).list.is_none() && gui.discover.keep.dest.is_some(), "Esc closes the list first");
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Char(' '));
        assert!(ui(&gui).custom);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        typed(&mut gui, "Mine/");
        key(&mut gui, KeyCode::Down);
        assert_eq!(ui(&gui).hand, Some(DestStop::Vars));
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(ui(&gui).text.value(), "Mine/{{ALBUM}}");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        assert_eq!(ui(&gui).hand, Some(DestStop::Save));
        key(&mut gui, KeyCode::Enter);
        let place = saves(&gui)[0].clone().expect("a place");
        assert_eq!((place.vpath.as_str(), place.layout.as_str()), ("copies", "Mine/{{ALBUM}}"));
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.keep.dest.is_none(), "Esc goes back while saving");
    }

    /// `cargo test dump_dest -- --ignored --nocapture`: the picker's states.
    #[test]
    #[ignore]
    fn dump_dest() {
        let _en = english();
        let show = |gui: &mut Gui, what: &str| {
            for h in [30, 24] {
                let rows = draw(gui, 100, h);
                println!("── {what} · 100 × {h} · tips: {}", super::super::discover::tips(gui));
                println!("{}", picker_rows(&rows).join("\n"));
            }
        };
        let mut gui = dest_gui("");
        show(&mut gui, "idle on the saved place");
        key(&mut gui, KeyCode::Left);
        show(&mut gui, "music: its own layout");
        key(&mut gui, KeyCode::Right);
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        show(&mut gui, "custom empty");
        typed(&mut gui, "{{ARTST}}/{{YEAR}} - {{ALBUM}}");
        show(&mut gui, "custom invalid");
        ctrl(&mut gui, 'u');
        typed(&mut gui, "From friends/{{ARTIST}}/{{ALBUM}}");
        show(&mut gui, "custom typed");
        dest_act(&mut gui, super::DestAct::Folders);
        land_folders(&mut gui);
        show(&mut gui, "the list open");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        land_folders(&mut gui);
        show(&mut gui, "into from-peers");
        key(&mut gui, KeyCode::Esc);
        dest_act(&mut gui, super::DestAct::Save);
        show(&mut gui, "saving");
        let words = "destination layout: Unknown variable {{ARTST}}. Supported: {{ARTIST}}, {{ALBUM}}, {{YEAR}}, {{GENRE}}, {{ALBUMARTIST}}, {{PEER}}, {{SERVERFOLDER}}";
        land_save(&mut gui, Err(crate::api::PluginFailure::Refused { status: 400, words: words.into() }));
        show(&mut gui, "refused");
        let mut gui = dest_gui("from-peers");
        show(&mut gui, "a folded base");
        dest_act(&mut gui, super::DestAct::UseDefault);
        show(&mut gui, "use the library default");
    }

    // ── Words in every locale (clause 119) ──────────────────────────────

    #[test]
    fn the_new_tips_lines_fit_the_footer_in_every_locale() {
        // Read with the locale named, never switched process-wide: the
        // other tests read English meanwhile.
        let keys = [
            "gui.tips.discover_row_keep",
            "gui.tips.discover_peer_tile_keep",
            "gui.tips.discover_peer_tile_more",
            "gui.tips.discover_sheet",
            "gui.tips.discover_sheet_keep",
            "gui.tips.discover_tile_list",
            "gui.tips.discover_tile_list_keep",
            "gui.tips.discover_keep_close",
            "gui.tips.discover_keep_live",
            "gui.tips.discover_keep_yours",
            "gui.tips.discover_keep_choose",
            "gui.tips.discover_keep_one",
            "gui.tips.discover_keep_other",
            "gui.tips.discover_dest_library",
            "gui.tips.discover_dest_library_save",
            "gui.tips.discover_dest_list",
            "gui.tips.discover_dest_radio",
            "gui.tips.discover_dest_custom",
            "gui.tips.discover_dest_custom_save",
            "gui.tips.discover_dest_vars",
            "gui.tips.discover_dest_folders",
            "gui.tips.discover_dest_button_off",
            "gui.tips.discover_dest_saving",
        ];
        // Every key named here exists: rust-i18n hands back the key path for
        // a missing one, which would pass the width check vacuously.
        for key in keys {
            assert_ne!(rust_i18n::t!(key, locale = "en").to_string(), key, "{key} is missing");
        }
        for code in ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
            for key in keys {
                let line = rust_i18n::t!(key, locale = code).to_string();
                assert!(crate::kit::width(&line) <= 99, "{code}: {key} is {} cells: {line}", crate::kit::width(&line));
            }
            // The choice's lines carry their fragments whole in every locale,
            // for the code finds them by their words — "d where files land"
            // gives way first where a line outgrows the footer (never in
            // English), `x cancel that copy` while its cancel is out, "↑ ↓
            // choose" with one choice.
            let d = rust_i18n::t!("gui.tips.discover_keep_d", locale = code).to_string();
            let arrows = rust_i18n::t!("gui.tips.discover_keep_arrows", locale = code).to_string();
            let x = rust_i18n::t!("gui.discover.keep.key_cancel_other", locale = code).to_string();
            for key in ["gui.tips.discover_keep_choose", "gui.tips.discover_keep_one", "gui.tips.discover_keep_other"] {
                let whole = rust_i18n::t!(key, locale = code).to_string();
                assert!(whole.contains(&format!(" · {d} · ")), "{code}: d stands whole: {whole}");
                let line = super::fit_tips(whole.clone(), std::slice::from_ref(&d));
                assert!(crate::kit::width(&line) <= 99, "{code}: {} cells: {line}", crate::kit::width(&line));
                assert!(code != "en" || line == whole, "English keeps d: {line}");
            }
            let other = rust_i18n::t!("gui.tips.discover_keep_other", locale = code).to_string();
            assert!(other.starts_with(&format!("{arrows} · ")) && other.contains(&format!(" · {x} · ")), "{code}: {other}");
            // An ended copy's composed line, its widest: Enter's verb, every
            // key word an outcome draws, Esc.
            let keys = ["gui.tips.discover_keep_d", "gui.discover.keep.key_queue", "gui.discover.keep.key_choose"];
            for verb in ["tip_play", "tip_retry", "tip_close", "tip_add_again"] {
                let key = format!("gui.discover.keep.{verb}");
                let verb = rust_i18n::t!(key.as_str(), locale = code).to_string();
                let mut parts = vec![rust_i18n::t!("gui.tips.discover_keep_enter", locale = code, verb = verb).to_string()];
                parts.extend(keys.iter().map(|k| rust_i18n::t!(*k, locale = code).to_string()));
                parts.push(rust_i18n::t!("gui.tips.discover_keep_esc", locale = code).to_string());
                let drops = [d.clone(), rust_i18n::t!("gui.discover.keep.key_choose", locale = code).to_string()];
                let line = super::fit_tips(parts.join(" · "), &drops);
                assert!(crate::kit::width(&line) <= 99, "{code}: {line}");
            }
            for verb in ["verb_save", "verb_default"] {
                let key = format!("gui.discover.dest.{verb}");
                let verb = rust_i18n::t!(key.as_str(), locale = code).to_string();
                assert_ne!(verb, key, "{code}: {key} is missing");
                let line = rust_i18n::t!("gui.tips.discover_dest_button", locale = code, verb = verb).to_string();
                assert!(crate::kit::width(&line) <= 99, "{code}: {line}");
            }
            // The tile's state words with their glyph and a space: a
            // 12-cell tile holds them whole or clips them like any text.
            for word in ["waiting", "copying", "stopping", "kept", "yours", "failed", "stopped", "refused"] {
                let key = format!("gui.discover.mark.{word}");
                let shown = format!("✓ {}", rust_i18n::t!(key.as_str(), locale = code));
                assert!(crate::kit::width(&shown) <= 12, "{code}: {shown} is wider than a tile");
            }
            // The rest tip with `C copies` fits where the strip stands.
            let rest = format!(
                "{} · {}",
                rust_i18n::t!("gui.tips.discover_strip", locale = code),
                rust_i18n::t!("gui.tips.discover_copies", locale = code)
            );
            assert!(crate::kit::width(&rest) <= 99, "{code}: {rest}");
        }
    }

    #[test]
    fn every_locales_primary_and_action_row_fit_the_window() {
        // §9: every primary at most 34 cells with a 2-digit count (past that
        // the album's count would drop), every action row the window draws
        // in the text's 72 (past that its key words would take the row
        // above).
        for code in ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
            let t = |key: &str| rust_i18n::t!(key, locale = code).to_string();
            let keep = |key: &str| t(&format!("gui.discover.keep.{key}"));
            let cells = |text: &str| crate::kit::width(text);
            let mut primaries: Vec<String> =
                ["add_song", "add_every", "add_missing", "adding", "retrying", "retry", "play_copy", "add_again", "close"].map(keep).to_vec();
            for form in ["one", "few", "many", "other"] {
                let key = format!("gui.discover.keep.songs_{form}");
                let songs = rust_i18n::t!(key.as_str(), locale = code, count = 12).to_string();
                primaries.push(format!("{} · {songs}", keep("add_album")));
                let key = format!("gui.discover.keep.add_albums_{form}");
                primaries.push(rust_i18n::t!(key.as_str(), locale = code, count = 12).to_string());
            }
            for words in &primaries {
                let label = format!("[ {words} ]");
                assert!(cells(&label) <= super::PRIMARY_MAX, "{code}: {label} is {} cells", cells(&label));
            }
            // §5.4's widest: a kept song, a gap, a file in the way before and
            // after the place moved, a copy running here or on another server.
            for (keys, primary) in [
                (&["key_queue", "key_choose"][..], "play_copy"),
                (&["key_change", "key_choose"], "retry"),
                (&["key_change", "key_choose"], "close"),
                (&["key_cancel"], "close"),
                (&["key_cancel_other"], "close"),
            ] {
                let mut parts: Vec<String> = keys.iter().map(|key| keep(key)).collect();
                parts.push(format!("[ {} ]", keep(primary)));
                let row = parts.join("   ");
                assert!(cells(&row) <= 72, "{code}: {row} is {} cells", cells(&row));
            }
            // The key words open with their key, never translated.
            for (key, letter) in [("key_change", "d "), ("key_queue", "a "), ("key_choose", "c "), ("key_cancel", "x "), ("key_cancel_other", "x ")] {
                assert!(keep(key).starts_with(letter), "{code}: {key} is {}", keep(key));
            }
            for key in ["gui.discover.keep.esc_close", "gui.discover.keep.esc_close_going"] {
                assert!(cells(&t(key)) <= 50, "{code}: {key}");
                assert!(t(key).starts_with(&t("gui.discover.keep.esc_close")), "{code}: {key} opens with the button's words");
            }
            assert!(cells(&t("gui.discover.keep.title")) <= 40, "{code}");
        }
    }

    #[test]
    fn a_slot_note_of_one_variable_keeps_its_two_rows_in_every_locale() {
        // §4.5: the note stands on its own two rows in clause 105's words —
        // the phone's — and past them the second is clipped with `…`, the
        // whole note both rows' tooltip; no row passes the text's 72 cells.
        // English keeps every note of one tag variable, of the server
        // folder, or of one of each whole (three variables, or the genre
        // with the server folder, are cut in English too). Japanese and
        // Chinese, whose sentences have no blank to break at, fill their
        // rows grapheme by grapheme: their plain note stands whole.
        for code in ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
            let t = |key: &str| rust_i18n::t!(key, locale = code).to_string();
            let slot = |name: &str| format!("‹{}›", t(&format!("gui.discover.dest.slot_{name}")));
            let note = |key: &str, slots: &str| {
                let key = format!("gui.discover.dest.{key}");
                rust_i18n::t!(key.as_str(), locale = code, slots = slots, server = slot("server")).to_string()
            };
            for (key, slots) in [
                ("slot_note_one", slot("artist")),
                ("slot_note_one", slot("year")),
                ("slot_note_genre_one", slot("genre")),
                ("slot_note_song", slot("genre")),
                ("slot_note_server", String::new()),
                ("slot_note_mixed_one", slot("artist")),
            ] {
                let words = note(key, &slots);
                let (rows, cut) = super::wrap_rows(&words, 72, 0, &[], false);
                assert!(rows.len() <= 2, "{code}: {key}: {rows:?}");
                assert!(rows.iter().all(|row| crate::kit::width(row) <= 72), "{code}: {key} outgrows its row: {rows:?}");
                let bare = |text: &str| text.replace(' ', "");
                match &cut {
                    // Whole: every word of it on screen.
                    None => assert_eq!(bare(&rows.concat()), bare(&words), "{code}: {key}"),
                    // Cut: the second row's `…`, the whole words the tooltip.
                    Some(tip) => {
                        assert_eq!(tip, &words, "{code}: {key}");
                        assert!(rows.len() == 2 && rows[1].ends_with('…'), "{code}: {key}: {rows:?}");
                    }
                }
                if code == "en" || (matches!(code, "ja" | "zh") && key == "slot_note_one") {
                    assert!(cut.is_none(), "{code}: {key} needs more than two rows: {rows:?}");
                }
            }
        }
    }

    // ── The Copies modal (clause 117; the canvas's TermCopies board) ─────

    /// A recommendation of the sample on a server, its album credited.
    fn rec_on(peer: i64, title: &str, artist: &str, album: &str) -> Value {
        fed(peer, title, artist, album, 0.5, Some(10.0))
    }

    /// The canvas's copies (TermCopies F): Borrowed Tapes copying (song 2
    /// of 3), every album by Boukmanflow waiting, Northern Signal failed,
    /// Rooftop Cypher refused, Afterimage kept, Peer Sessions kept, Free
    /// Instrumentals Vol. 9 cancelled — and an eighth, older, below the
    /// fold.
    fn copies_jobs() -> Vec<PluginJob> {
        let counts = |total: u64, copied: u64, skipped: u64, failed: u64| {
            json!({ "total": total, "copied": copied, "skipped": skipped, "failed": failed })
        };
        vec![
            job(40, &rec_on(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes"), Some("album"), "running",
                json!({ "progress": 0.52, "statusText": "song 2 of 3 · Late Train Verse",
                    "result": { "scope": "album", "counts": counts(3, 1, 0, 0) } })),
            job(41, &rec_on(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes"), Some("artist"), "queued", json!({})),
            job(39, &rec_on(B, "Northern Signal", "Color Out", "Peer Sessions"), None, "failed",
                json!({ "error": "Stage Peer B is unreachable" })),
            job(38, &rec_on(C, "Rooftop Cypher", "Boukmanflow", "Borrowed Tapes"), None, "failed",
                json!({ "error": "Stage Peer C does not allow copies with your server's key" })),
            job(37, &rec_on(B, "Afterimage", "Color Out", "Peer Sessions"), None, "done",
                json!({ "result": { "copied": { "vpath": "copies", "filepath": "copies/Color Out/Peer Sessions/Afterimage.mp3" } } })),
            job(36, &rec_on(B, "Glass Hours", "Color Out", "Peer Sessions"), Some("album"), "done",
                json!({ "result": { "scope": "album", "counts": counts(3, 2, 1, 0), "stopped": null } })),
            job(35, &rec_on(B, "Brass District", "ALM", "Free Instrumentals Vol. 9"), Some("album"), "cancelled",
                json!({ "result": { "scope": "album", "counts": counts(2, 1, 0, 0) } })),
            job(34, &rec_on(B, "Static Lullaby", "Night Owl Radio", "First Transmission"), None, "done",
                json!({ "result": { "skipped": "owned", "existing": { "filepath": "music/x.mp3" } } })),
        ]
    }

    /// The room with `jobs`, the Copies modal open on their list.
    fn copies_gui(jobs: &[PluginJob]) -> Gui {
        let mut gui = canvas_gui(jobs);
        gui.act(Act::Disc(DiscAct::Copies));
        land_jobs(&mut gui, jobs);
        gui.pending.clear();
        gui
    }

    /// The Copies modal's own 76 cells per row, from its top border down.
    fn copies_rows_on(rows: &[String]) -> Vec<String> {
        let top = rows.iter().position(|r| r.contains("Copies") && r.contains("[X]")).expect("the Copies modal") - 1;
        let x = rows[top].chars().position(|c| c == '╭' || c == '+').expect("its corner");
        let bottom = (top + 1..rows.len())
            .find(|&y| matches!(rows[y].chars().nth(x), Some('╰') | Some('+')))
            .expect("its bottom");
        rows[top..=bottom].iter().map(|r| r.chars().skip(x).take(76).collect()).collect()
    }

    /// `cargo test dump_copies -- --ignored --nocapture`: the Copies modal
    /// and Open elsewhere's list, every state.
    #[test]
    #[ignore]
    fn dump_copies() {
        let _en = english();
        let mut gui = copies_gui(&copies_jobs());
        for _ in 0..3 {
            key(&mut gui, KeyCode::Down);
        }
        println!("{}", draw(&mut gui, 100, 30).join("\n"));
        println!("tips: {}", super::super::discover::tips(&gui));
        println!("{}", draw(&mut gui, 100, 24).join("\n"));
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        println!("{}", draw(&mut gui, 100, 30).join("\n"));
        drop(_legacy);
        crate::kit::theme::pin_modern_terminal();
        let mut gui = links_gui();
        land_links(&mut gui, Ok(links_answer()));
        for _ in 0..3 {
            key(&mut gui, KeyCode::Down);
        }
        println!("{}", draw(&mut gui, 100, 30).join("\n"));
        println!("tips: {:?}", super::super::actions::tips(&gui));
    }

    /// Answer the jobs list ask out with a failure.
    fn fail_jobs(gui: &mut Gui, failure: crate::api::PluginFailure) {
        let asks: Vec<(u64, u64)> = gui
            .pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::Jobs { seq, generation }) => Some((*seq, *generation)),
                _ => None,
            })
            .collect();
        assert!(!asks.is_empty(), "no list ask out in {:?}", gui.pending);
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::Jobs { .. })));
        for (seq, generation) in asks {
            apply(gui, Event::Jobs { seq, generation, answer: Err(failure.clone()) });
        }
    }

    fn unreachable() -> crate::api::PluginFailure {
        crate::api::PluginFailure::Unreachable("connection refused".into())
    }

    /// The canvas's 76-cell rows, as drawn.
    fn rows76(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|r| r.to_string()).collect()
    }

    #[test]
    fn the_copies_modal_lists_every_copy_as_the_canvas_draws_it() {
        let _en = english();
        let mut gui = copies_gui(&copies_jobs());
        // ↓ ↓ ↓ holds the third copy, Northern Signal (the canvas's frame).
        for _ in 0..3 {
            key(&mut gui, KeyCode::Down);
        }
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let all = rows.join("\n");
        // 76 wide where Keep a copy opens — x 19–94, centred in the room —
        // 22 tall: anchored as if 17 rows, it grows to the room's last row
        // and rises to row 3. Seven copies of eight, the list's bar on the
        // right border.
        let shown: Vec<String> = (3..25).map(|y| rows[y].chars().skip(19).take(76).collect()).collect();
        assert_eq!(
            shown,
            rows76(&[
                "╭──────────────────────────────────────────────────────────────────────────╮",
                "│ Copies                                                               [X] │",
                "│                                                                          │",
                "│ TITLE                            FROM          STATE                     │",
                "│ ──────────────────────────────────────────────────────────────────────── │",
                "│ Borrowed Tapes                   Stage Peer B  ▰▰▰▰▰▱▱▱▱▱ 52%        [X] ▲",
                "│   album · song 2 of 3 · Late Train Verse                                 █",
                "│ every album by Boukmanflow       Stage Peer B  ◷ waiting             [X] █",
                "│   waiting for a free slot                                                █",
                "│ Northern Signal                  Stage Peer B  ! failed                  █",
                "│   song · Stage Peer B is unreachable                               Retry █",
                "│ Rooftop Cypher                   Stage Peer C  ! refused                 █",
                "│   song · Stage Peer C does not allow copies with your server's key       █",
                "│ Afterimage                       Stage Peer B  ✓ in your collection      █",
                "│   song · copies › Color Out › Peer Sessions               Play your copy █",
                "│ Peer Sessions                    Stage Peer B  ✓ in your collection      █",
                "│   album · 2 copied · 1 already yours                                     █",
                "│ Free Instrumentals Vol. 9        Stage Peer B  × cancelled               │",
                "│   album · cancelled · 1 copied                                 Add again ▼",
                "│                                                                          │",
                "│                                                           Clear finished │",
                "╰──────────────────────────────────────────────────────────────────────────╯",
            ]),
            "{all}"
        );
        // The tokens, run by run (the canvas's titles).
        let at = |x: u16, y: u16| Position { x, y };
        assert_eq!((fg(&buffer, at(19, 3)), fg(&buffer, at(21, 4)), bold(&buffer, at(21, 4))), (th().accent, th().accent, true));
        assert_eq!(fg(&buffer, at(90, 4)), th().dim, "the close DIM");
        assert_eq!((fg(&buffer, at(21, 6)), fg(&buffer, at(21, 7))), (th().dim, th().dim), "the header and its rule DIM");
        assert_eq!((fg(&buffer, at(68, 8)), fg(&buffer, at(73, 8)), fg(&buffer, at(79, 8))), (th().ok, th().dim, th().dim), "▰ OK, ▱ and % DIM");
        assert_eq!(fg(&buffer, at(90, 8)), th().dim, "a live row's [X] DIM");
        assert_eq!(fg(&buffer, at(23, 9)), th().dim, "the second row DIM");
        assert_eq!((fg(&buffer, at(68, 10)), fg(&buffer, at(69, 10))), (th().dim, th().dim), "◷ waiting DIM");
        // The held row: both rows in the selection's colours.
        for x in [21, 47, 68, 92] {
            assert_eq!((bg(&buffer, at(x, 12)), bg(&buffer, at(x, 13))), (th().accent, th().accent), "held at {x}");
        }
        assert_eq!(
            (fg(&buffer, at(90, 13)), bg(&buffer, at(90, 13)), bold(&buffer, at(90, 13))),
            (th().on_accent, th().accent, false),
            "the held copy's Retry: the plain slab, never bold"
        );
        assert_eq!(fg(&buffer, at(68, 14)), th().gold, "! refused GOLD");
        assert_eq!((fg(&buffer, at(23, 15)), fg(&buffer, at(30, 15))), (th().dim, th().gold), "song · DIM, the server's words GOLD");
        assert_eq!((fg(&buffer, at(68, 16)), fg(&buffer, at(70, 16))), (th().ok, th().ok), "✓ in your collection OK");
        assert_eq!(fg(&buffer, at(79, 17)), th().accent, "Play your copy ACCENT");
        assert_eq!(fg(&buffer, at(68, 20)), th().dim, "× cancelled DIM");
        assert_eq!(fg(&buffer, at(84, 21)), th().accent, "Add again ACCENT");
        assert_eq!(fg(&buffer, at(79, 23)), th().dim, "Clear finished DIM");
        // No numbers outside the modal: the room's indicator says words.
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · Enter retry · X clear finished · Esc close");
        // The pointer on a live [X]: DANGER, its tooltip names `x`.
        gui.ui.pointer = Some(at(91, 10));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        assert_eq!((fg(&buffer, at(90, 10)), bold(&buffer, at(90, 10))), (th().danger, true));
        assert_eq!(gui.ui.hit(at(91, 10)), Some(Act::Disc(DiscAct::CopiesCancel(41))));
        assert_eq!(gui.ui.hit(at(89, 13)), Some(Act::Disc(DiscAct::CopiesAct(39))), "Retry");
        assert_eq!(gui.ui.hit(at(87, 23)), Some(Act::Disc(DiscAct::CopiesClear)));
        assert_eq!(gui.ui.hit(at(91, 4)), Some(Act::Disc(DiscAct::CopiesClose)));
        assert_eq!(gui.ui.hit(at(12, 14)), Some(Act::Guard), "the room beneath is inert");
        assert_eq!(gui.ui.hit(at(17, 14)), Some(Act::Guard), "and the band");
    }

    #[test]
    fn the_copies_list_scrolls_at_the_floor_its_head_and_footer_standing() {
        let _en = english();
        let mut gui = copies_gui(&copies_jobs());
        let rows = draw(&mut gui, 100, 24);
        // The rows down to the room's last, risen to row 1: 18 at the floor
        // — five copies — the tab bar above and the seek line below whole.
        assert_eq!(rows[1].chars().nth(19), Some('╭'), "{}", rows.join("\n"));
        assert_eq!(rows[18].chars().nth(19), Some('╰'));
        assert!(rows[19].starts_with(" 0:00 "), "{}", rows[19]);
        assert_eq!((rows[6].chars().nth(94), rows[15].chars().nth(94)), (Some('▲'), Some('▼')));
        assert!(rows[14].contains("Afterimage"), "{}", rows.join("\n"));
        assert!(!rows.iter().any(|r| r.contains("│ Peer Sessions ")), "the sixth below the fold");
        // The wheel scrolls a copy a notch; the head and Clear finished stay.
        gui.ui.pointer = Some(Position { x: 40, y: 10 });
        super::super::discover::wheel(&mut gui, 1);
        let rows = draw(&mut gui, 100, 24);
        assert!(rows[6].contains("every album by Boukmanflow"), "{}", rows.join("\n"));
        assert!(rows[4].contains("TITLE") && rows[17].contains("Clear finished"));
        // The eighth copy: the owned song — "✓ already yours", DIM.
        super::super::discover::wheel(&mut gui, 1);
        super::super::discover::wheel(&mut gui, 1);
        let rows = draw(&mut gui, 100, 24);
        let at = at(&rows, "Static Lullaby");
        assert!(rows[at.y as usize].contains("✓ already yours"));
        assert!(rows[at.y as usize + 1].contains("song · you had it, nothing copied"));
        // ↓ walks and reveals: the hand back on the first copy scrolls up.
        key(&mut gui, KeyCode::Down);
        let rows = draw(&mut gui, 100, 24);
        assert!(rows[6].contains("Borrowed Tapes"), "{}", rows.join("\n"));
    }

    #[test]
    fn the_copies_keys_cancel_retry_play_clear_and_close() {
        let _en = english();
        let mut gui = copies_gui(&copies_jobs());
        draw(&mut gui, 100, 30);
        // Stowed: Enter and `x` do nothing; the tips name the walk.
        key(&mut gui, KeyCode::Char('x'));
        key(&mut gui, KeyCode::Enter);
        assert!(gui.pending.is_empty(), "{:?}", gui.pending);
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · X clear finished · Esc close");
        // ↓ holds the running copy: `x` cancels it, once.
        key(&mut gui, KeyCode::Down);
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · x cancel · X clear finished · Esc close");
        key(&mut gui, KeyCode::Char('x'));
        key(&mut gui, KeyCode::Char('x'));
        let cancels: Vec<i64> = gui.pending.iter().filter_map(|e| match e {
            Effect::Api(ApiCmd::JobCancel { id, .. }) => Some(*id),
            _ => None,
        }).collect();
        assert_eq!(cancels, [40]);
        // Its cancel out: stopping…, no [X] left.
        let rows = draw(&mut gui, 100, 30);
        let row = &rows[at(&rows, "Borrowed Tapes").y as usize];
        assert!(row.contains("stopping…") && !row.contains("[X]"), "{row}");
        assert!(rows[at(&rows, "Borrowed Tapes").y as usize + 1].contains("album · stopping after song 2 of 3 · Late Train Verse"));
        gui.pending.clear();
        // ↓ ↓ on Northern Signal: Enter retries the same copy, its
        // recommendation as the server keeps it, no scope (a song).
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        let north = copies_jobs().into_iter().find(|j| j.id == 39).unwrap();
        assert_eq!(starts(&gui), vec![(north.recommendation.clone(), crate::api::types::JobScope::Song)]);
        gui.pending.clear();
        // ↓ ↓ on Afterimage: Enter plays your copy from your own server.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · Enter play your copy · X clear finished · Esc close");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.app.queue.items.iter().any(|q| q.filepath == "copies/Color Out/Peer Sessions/Afterimage.mp3" && q.origin.peer.is_none()));
        // The refused row offers nothing: the server would refuse again.
        key(&mut gui, KeyCode::Up);
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · X clear finished · Esc close");
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert!(starts(&gui).is_empty());
        // X clears the finished; Esc closes, the copies keep running.
        key(&mut gui, KeyCode::Char('X'));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::JobsClear { .. }))));
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.copies.is_none());
        assert!(gui.pending.iter().all(|e| !matches!(e, Effect::Api(ApiCmd::JobCancel { .. }))));
    }

    /// The modal's rows once `gui` drew at 100 × 30.
    fn copies_on(gui: &mut Gui) -> Vec<String> {
        copies_rows_on(&draw(gui, 100, 30))
    }

    #[test]
    fn every_copies_state_reads_as_the_canvas_draws_it() {
        let _en = english();
        let head = [
            "╭──────────────────────────────────────────────────────────────────────────╮",
            "│ Copies                                                               [X] │",
            "│                                                                          │",
            "│ TITLE                            FROM          STATE                     │",
            "│ ──────────────────────────────────────────────────────────────────────── │",
        ];
        let blank = "│                                                                          │";
        let bottom = "╰──────────────────────────────────────────────────────────────────────────╯";
        let modal = |body: &[&str]| -> Vec<String> {
            head.iter().chain(body.iter()).chain([&bottom]).map(|r| r.to_string()).collect()
        };
        // Loading: the first ask out, ACCENT.
        let mut gui = canvas_gui(&[]);
        gui.app.taste.keep.book = Default::default();
        gui.act(Act::Disc(DiscAct::Copies));
        assert_eq!(copies_on(&mut gui), modal(&[
            "│ asking your server for copies…                                           │", blank, blank,
        ]));
        assert_eq!(super::super::discover::tips(&gui), "Esc close");
        // The first ask failed: GOLD words, [ Try again ] — Enter asks again.
        fail_jobs(&mut gui, unreachable());
        assert_eq!(copies_on(&mut gui), modal(&[
            "│ couldn't load copies: your server didn't answer                          │", blank,
            "│                                                            [ Try again ] │",
        ]));
        let buffer = buffer(&mut gui, 100, 30);
        let words = at(&lines(&buffer), "couldn't load");
        assert_eq!(fg(&buffer, words), th().gold);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::Jobs { .. }))));
        // Empty: nothing to clear, so no Clear finished.
        land_jobs(&mut gui, &[]);
        assert_eq!(copies_on(&mut gui), modal(&[
            "│ Nothing copying.                                                         │",
            "│ Start one with c on a connected server's song or album.                  │", blank, blank,
        ]));
        // Running, then no estimate yet: the bar all DIM, no %.
        let late = rec_on(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes");
        let running = job(40, &late, Some("album"), "running", json!({ "progress": 0.52, "statusText": "song 2 of 3 · Late Train Verse",
            "result": { "scope": "album", "counts": { "total": 3, "copied": 1, "skipped": 0, "failed": 0 } } }));
        let mut gui = copies_gui(std::slice::from_ref(&running));
        assert_eq!(copies_on(&mut gui), modal(&[
            "│ Borrowed Tapes                   Stage Peer B  ▰▰▰▰▰▱▱▱▱▱ 52%        [X] │",
            "│   album · song 2 of 3 · Late Train Verse                                 │", blank, blank,
        ]));
        let rooftop = rec_on(C, "Rooftop Cypher", "Boukmanflow", "Borrowed Tapes");
        let mut gui = copies_gui(&[job(42, &rooftop, None, "running", json!({ "statusText": "asking Stage Peer C for the file" }))]);
        assert_eq!(copies_on(&mut gui)[5..7], [
            "│ Rooftop Cypher                   Stage Peer C  ▱▱▱▱▱▱▱▱▱▱            [X] │".to_string(),
            "│   song · asking Stage Peer C for the file                                │".to_string(),
        ]);
        // Waiting, the held row in the selection's colours.
        let color = rec_on(B, "Glass Hours", "Color Out", "Peer Sessions");
        let mut gui = copies_gui(&[
            running.clone(),
            job(43, &color, Some("artist-missing"), "queued", json!({})),
            job(44, &rooftop, None, "queued", json!({})),
        ]);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        assert_eq!(copies_on(&mut gui), modal(&[
            "│ Borrowed Tapes                   Stage Peer B  ▰▰▰▰▰▱▱▱▱▱ 52%        [X] │",
            "│   album · song 2 of 3 · Late Train Verse                                 │",
            "│ every album by Color Out         Stage Peer B  ◷ waiting             [X] │",
            "│   only the albums you don't have · waiting for a free slot               │",
            "│ Rooftop Cypher                   Stage Peer C  ◷ waiting             [X] │",
            "│   song · waiting for a free slot                                         │", blank, blank,
        ]));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let held = at(&lines(&buffer), "every album by Color Out");
        assert_eq!((bg(&buffer, held), bg(&buffer, Position { x: held.x + 71, y: held.y + 1 })), (th().accent, th().accent));
        // Failed, stopped and refused: GOLD carries the server's words.
        let counts = |copied: u64, failed: u64| json!({ "total": 3, "copied": copied, "skipped": 0, "failed": failed });
        let north = rec_on(B, "Northern Signal", "Color Out", "Peer Sessions");
        let brass = rec_on(B, "Brass District", "ALM", "Free Instrumentals Vol. 9");
        let late_c = rec_on(C, "Rooftop Cypher", "Boukmanflow", "Borrowed Tapes");
        let gui_jobs = vec![
            job(57, &north, None, "failed", json!({ "error": "Stage Peer B is unreachable" })),
            job(56, &rooftop, None, "failed", json!({ "error": "Stage Peer C does not allow copies with your server's key" })),
            job(55, &color, Some("album"), "done", json!({ "result": { "scope": "album", "counts": counts(0, 3), "stopped": null } })),
            job(54, &late, Some("album"), "done", json!({ "result": { "scope": "album", "counts": counts(1, 0), "stopped": "quota" } })),
            job(53, &brass, Some("album"), "done", json!({ "result": { "scope": "album", "counts": counts(1, 0), "stopped": "busy" } })),
            job(52, &color, Some("artist"), "done", json!({ "result": { "scope": "artist", "counts": counts(1, 0), "stopped": "peer" } })),
            job(51, &late_c, Some("album"), "done", json!({ "result": { "scope": "album", "counts": counts(0, 0), "stopped": "refused" } })),
        ];
        let mut gui = copies_gui(&gui_jobs);
        assert_eq!(copies_on(&mut gui), modal(&[
            "│ Northern Signal                  Stage Peer B  ! failed                  │",
            "│   song · Stage Peer B is unreachable                               Retry │",
            "│ Rooftop Cypher                   Stage Peer C  ! refused                 │",
            "│   song · Stage Peer C does not allow copies with your server's key       │",
            "│ Peer Sessions                    Stage Peer B  ! failed                  │",
            "│   album · 3 songs failed, none copied                              Retry │",
            "│ Borrowed Tapes                   Stage Peer B  ! stopped                 │",
            "│   album · Stage Peer B's transfer limit stopped it · 1 copied      Retry │",
            "│ Free Instrumentals Vol. 9        Stage Peer B  ! stopped                 │",
            "│   album · Stage Peer B was too busy to send more · 1 copied        Retry │",
            "│ every album by Color Out         Stage Peer B  ! stopped                 │",
            "│   Stage Peer B went away · 1 copied                                Retry │",
            "│ Borrowed Tapes                   Stage Peer C  ! refused                 │",
            "│   album · Stage Peer C does not allow copies with your server's key      │", blank,
            "│                                                           Clear finished │",
        ]));
        // Finished: Retry only where gaps are left — Copy again no more — and
        // none for a file in the way (Keep a copy is where the place
        // changes); Add again after a cancel.
        let glass = rec_on(B, "Glass Hours", "Color Out", "Peer Sessions");
        let after = rec_on(B, "Afterimage", "Color Out", "Peer Sessions");
        let owl = rec_on(B, "Static Lullaby", "Night Owl Radio", "First Transmission");
        let gui_jobs = vec![
            job(67, &after, None, "done", json!({ "result": { "copied": { "vpath": "copies", "filepath": "copies/Color Out/Peer Sessions/Afterimage.mp3" } } })),
            job(66, &glass, Some("album"), "done", json!({ "result": { "scope": "album", "counts": { "total": 3, "copied": 2, "skipped": 1, "failed": 0 } } })),
            job(65, &late, Some("artist"), "done", json!({ "result": { "scope": "artist", "counts": { "total": 4, "copied": 2, "skipped": 1, "failed": 1 },
                "albums": [{ "name": "Borrowed Tapes" }], "skippedAlbums": [{ "name": "Kite Days", "why": "owned" }, { "name": "Night Bus", "why": "owned" }] } })),
            job(64, &rooftop, None, "done", json!({ "result": { "skipped": "owned", "existing": { "filepath": "music/rooftop.mp3" } } })),
            job(63, &glass, None, "done", json!({ "result": { "skipped": "exists", "filepath": "copies/Color Out/Peer Sessions/Glass Hours.mp3" } })),
            job(62, &owl, Some("artist"), "done", json!({ "result": { "scope": "artist", "counts": { "total": 0, "copied": 0, "skipped": 0, "failed": 0 },
                "albums": [], "skippedAlbums": [{ "name": "Split 7in", "why": "appearance" }] } })),
            job(61, &brass, Some("album"), "cancelled", json!({ "result": { "scope": "album", "counts": { "total": 2, "copied": 1, "skipped": 0, "failed": 0 } } })),
        ];
        let mut gui = copies_gui(&gui_jobs);
        assert_eq!(copies_on(&mut gui), modal(&[
            "│ Afterimage                       Stage Peer B  ✓ in your collection      │",
            "│   song · copies › Color Out › Peer Sessions               Play your copy │",
            "│ Peer Sessions                    Stage Peer B  ✓ in your collection      │",
            "│   album · 2 copied · 1 already yours                                     │",
            "│ every album by Boukmanflow       Stage Peer B  ✓ in your collection      │",
            "│   2 copied · 1 already yours · 1 failed · 2 albums left out        Retry │",
            "│ Rooftop Cypher                   Stage Peer C  ✓ already yours           │",
            "│   song · you had it, nothing copied                                      │",
            "│ Glass Hours                      Stage Peer B  skipped                   │",
            "│   song · a file already sits at that path                                │",
            "│ every album by Night Owl Radio   Stage Peer B  nothing to copy           │",
            "│   nothing of theirs to copy                                              │",
            "│ Free Instrumentals Vol. 9        Stage Peer B  × cancelled               │",
            "│   album · cancelled · 1 copied                                 Add again │", blank,
            "│                                                           Clear finished │",
        ]));
        // A refresh that failed keeps its rows: GOLD, when the poll tries
        // again, Try again beside Clear finished.
        let after_done = gui_jobs[0].clone();
        let mut gui = copies_gui(&[running.clone(), after_done.clone()]);
        let effects = gui.app.copies_try_again();
        gui.pend(effects);
        fail_jobs(&mut gui, unreachable());
        let shown = copies_on(&mut gui);
        assert_eq!(shown[5..9], modal(&[
            "│ Borrowed Tapes                   Stage Peer B  ▰▰▰▰▰▱▱▱▱▱ 52%        [X] │",
            "│   album · song 2 of 3 · Late Train Verse                                 │",
            "│ Afterimage                       Stage Peer B  ✓ in your collection      │",
            "│   song · copies › Color Out › Peer Sessions               Play your copy │",
        ])[5..9]);
        assert!(shown[9].starts_with("│ couldn't refresh: your server didn't answer · next try in "), "{}", shown[9]);
        assert_eq!(shown[10], "│                                               Try again   Clear finished │");
        assert_eq!(gui.ui.hit(Position { x: 19 + 50, y: 6 + 10 }), Some(Act::Disc(DiscAct::CopiesTryAgain)));
        // A cancel that lost its race: the list asked again, the outcome,
        // and a TEXT note bottom-left — gone on the next key.
        let mut gui = copies_gui(std::slice::from_ref(&running));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('x'));
        let generation = gui.app.session_gen();
        apply(&mut gui, Event::JobCancelled { id: 40, generation, answer: Ok(crate::api::types::CancelAnswer::AlreadyFinished) });
        let done = job(40, &late, Some("album"), "done", json!({ "result": { "scope": "album", "counts": { "total": 3, "copied": 3, "skipped": 0, "failed": 0 } } }));
        land_jobs(&mut gui, std::slice::from_ref(&done));
        let shown = copies_on(&mut gui);
        assert_eq!(shown[5..8], [
            "│ Borrowed Tapes                   Stage Peer B  ✓ in your collection      │".to_string(),
            "│   album · 3 copied                                                       │".to_string(),
            blank.to_string(),
        ]);
        assert_eq!(shown[8], "│ Borrowed Tapes had already finished, nothing to cancel    Clear finished │");
        key(&mut gui, KeyCode::Down);
        assert!(!copies_on(&mut gui)[8].contains("had already finished"));
    }

    #[test]
    fn the_legacy_console_draws_the_copies_in_its_own_glyphs() {
        let _en = english();
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let mut gui = copies_gui(&copies_jobs());
        let shown = copies_on(&mut gui);
        assert_eq!(shown[0], format!("+{}+", "-".repeat(74)));
        assert_eq!(shown[4], format!("| {} |", "-".repeat(72)));
        assert!(shown[5].contains("Stage Peer B  52% done"), "{}", shown[5]);
        assert!(shown[7].contains("~ waiting") && shown[17].contains("x cancelled"), "{}", shown.join("\n"));
        assert!(shown[13].contains("+ in your collection") && shown[14].contains("copies > Color Out > Peer Sessions"));
        let joined = shown.join("");
        for glyph in ['◷', '✓', '×', '▰', '▱', '›', '…', '╭', '│'] {
            if glyph == '│' {
                continue; // the scrollbar's track is the kit's own
            }
            assert!(!joined.contains(glyph), "{glyph} on the legacy console");
        }
    }

    // ── Open elsewhere (clauses 140–141) ────────────────────────────────

    /// The links plug-in's answer: the recording's ids, then the searches
    /// — and one the opener must never see.
    fn links_answer() -> Vec<crate::api::types::PluginLink> {
        let link = |id: &str, label: &str, url: &str, kind: &str| crate::api::types::PluginLink {
            id: id.into(),
            label: label.into(),
            url: url.into(),
            kind: kind.into(),
        };
        vec![
            link("mb-search", "MusicBrainz", "https://musicbrainz.org/search?q=x", "search"),
            link("mb-recording", "MusicBrainz recording", "https://musicbrainz.org/recording/1", "canonical"),
            link("mb-release", "MusicBrainz release", "https://musicbrainz.org/release/2", "canonical"),
            link("deezer", "Deezer", "https://www.deezer.com/search/x", "search"),
            link("apple", "Apple Music", "https://music.apple.com/search?term=x", "search"),
            link("odd", "Odd", "javascript:alert(1)", "search"),
            link("youtube", "YouTube", "https://www.youtube.com/results?search_query=x", "search"),
            link("bandcamp", "Bandcamp", "https://bandcamp.com/search?q=x", "search"),
            link("discogs", "Discogs", "https://www.discogs.com/search?q=x", "search"),
        ]
    }

    /// The sheet on Late Train Verse with Open elsewhere's list asked.
    fn links_gui() -> Gui {
        let mut gui = canvas_gui(&canvas_jobs());
        draw(&mut gui, 100, 30);
        gui.act(Act::Disc(DiscAct::More(2)));
        draw(&mut gui, 100, 30);
        // ↓ to the row, Enter opens its list.
        for _ in 0..5 {
            key(&mut gui, KeyCode::Down);
        }
        key(&mut gui, KeyCode::Enter);
        gui
    }

    fn land_links(gui: &mut Gui, answer: Result<Vec<crate::api::types::PluginLink>, crate::api::PluginFailure>) {
        let (seq, generation) = gui
            .pending
            .iter()
            .rev()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::PluginLinks { seq, generation, .. }) => Some((*seq, *generation)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no links ask in {:?}", gui.pending));
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::PluginLinks { .. })));
        apply(gui, Event::PluginLinks { seq, generation, answer });
    }

    /// The sheet's 66 cells from row 8 at 100 × 30.
    fn sheet_rows(rows: &[String]) -> Vec<String> {
        rows[8..22].iter().map(|r| r.chars().skip(17).take(66).collect()).collect()
    }

    #[test]
    fn open_elsewhere_hangs_its_list_from_the_sheets_row_as_the_canvas_draws_it() {
        let _en = english();
        let mut gui = links_gui();
        // Asking: ACCENT, one line level with the row.
        let buffer = buffer(&mut gui, 100, 30);
        let shown = sheet_rows(&lines(&buffer));
        assert_eq!(shown[10..13], [
            "│ Song Info                   ╭──────────────────────────────╮   │".to_string(),
            "│ Open elsewhere…             │ asking…                      │   │".to_string(),
            "│                             ╰──────────────────────────────╯   │".to_string(),
        ], "{}", shown.join("\n"));
        assert_eq!((fg(&buffer, Position { x: 49, y: 19 }), fg(&buffer, Position { x: 19, y: 19 })), (th().accent, th().accent));
        assert!(bold(&buffer, Position { x: 19, y: 19 }), "the row's label ACCENT BOLD while its list is open");
        assert_eq!(super::super::actions::tips(&gui).as_deref(), Some("Esc back"));
        // The list: CANONICAL then SEARCH, DIM heads, the labels never a
        // URL, https only — the first link held (opened by the keys).
        land_links(&mut gui, Ok(links_answer()));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let shown = sheet_rows(&rows);
        // The list's cells (the sheet's header beside it is the fixture's:
        // no cover, no spec line).
        let list: Vec<String> = shown[1..13].iter().map(|r| r.chars().skip(30).collect()).collect();
        assert_eq!(list, [
            "╭──────────────────────────────╮[X]│",
            "│ CANONICAL                    │   │",
            "│   MusicBrainz recording      │   │",
            "│   MusicBrainz release        │   │",
            "│ SEARCH                       │   │",
            "│   MusicBrainz                │   │",
            "│   Deezer                     │   │",
            "│   Apple Music                │   │",
            "│   YouTube                    │   │",
            "│   Bandcamp                   │   │",
            "│   Discogs                    │   │",
            "╰──────────────────────────────╯   │",
        ].map(str::to_string), "{}", shown.join("\n"));
        let labels: Vec<String> = shown[6..12].iter().map(|r| r.chars().take(30).collect::<String>().trim_end().to_string()).collect();
        assert_eq!(labels, ["│ Play now", "│ Add next", "│ Add to end of queue", "│ Keep a copy…  c", "│ Song Info", "│ Open elsewhere…"]);
        assert!(!rows.join("\n").contains("https") && !rows.join("\n").contains("Odd"));
        assert_eq!(fg(&buffer, Position { x: 49, y: 10 }), th().dim, "CANONICAL DIM");
        let slab: Vec<_> = (47..80).map(|x| bg(&buffer, Position { x, y: 11 })).collect();
        assert_eq!((bg(&buffer, Position { x: 48, y: 11 }), bg(&buffer, Position { x: 77, y: 11 })), (th().accent, th().accent), "the held link: {slab:?}");
        assert_eq!(super::super::actions::tips(&gui).as_deref(), Some("↑ ↓ rows · Enter open · Esc back"));
        // ↓ ↓ ↓ to Deezer; hover brightens Apple Music.
        for _ in 0..3 {
            key(&mut gui, KeyCode::Down);
        }
        gui.ui.pointer = Some(Position { x: 55, y: 16 });
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        assert_eq!(bg(&buffer, Position { x: 51, y: 15 }), th().accent, "Deezer held");
        assert_eq!(
            (fg(&buffer, Position { x: 51, y: 15 }), bold(&buffer, Position { x: 51, y: 15 })),
            (th().on_accent, false),
            "the held link in the plain selection colours"
        );
        assert_eq!(fg(&buffer, Position { x: 51, y: 16 }), th().bright, "Apple Music under the pointer");
        assert!(!bold(&buffer, Position { x: 51, y: 16 }), "the hover BRIGHT, not bold");
        assert_eq!(gui.ui.hit(Position { x: 55, y: 16 }), Some(Act::Disc(DiscAct::LinkOpen(4))));
        // Enter opens Deezer: "opening Deezer…" ACCENT now; the opener's
        // answer once it lands (under test the harness's seam).
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.app.message.as_ref().map(|m| (m.text.as_str(), m.kind)), Some(("opening Deezer…", crate::tui::app::MessageKind::Busy)));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while gui.app.message.as_ref().is_some_and(|m| m.kind == crate::tui::app::MessageKind::Busy) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
            draw(&mut gui, 100, 30);
        }
        let message = gui.app.message.clone().expect("a note");
        assert_eq!(message.kind, crate::tui::app::MessageKind::Plain);
        assert!(message.text.starts_with("Deezer · https://www.deezer."), "{}", message.text);
        assert!(gui.actions.sheet.as_ref().is_some_and(|s| s.links), "the list stays open");
        // Esc folds it, the sheet stays.
        key(&mut gui, KeyCode::Esc);
        assert!(gui.actions.sheet.as_ref().is_some_and(|s| !s.links));
    }

    #[test]
    fn open_elsewheres_list_says_none_and_failed_and_asks_again() {
        let _en = english();
        let mut gui = links_gui();
        land_links(&mut gui, Ok(vec![]));
        let shown = sheet_rows(&draw(&mut gui, 100, 30));
        assert_eq!(shown[11], "│ Open elsewhere…             │ no links for this song       │   │");
        // Only links the opener may take count: a list of none of them is
        // "no links".
        let mut gui = links_gui();
        land_links(&mut gui, Ok(links_answer().into_iter().filter(|l| !l.is_web()).collect()));
        assert!(sheet_rows(&draw(&mut gui, 100, 30))[11].contains("no links for this song"));
        // Failed: the server's words GOLD, Try again held — Enter asks again.
        let mut gui = links_gui();
        land_links(&mut gui, Err(crate::api::PluginFailure::Refused { status: 504, words: "the links plug-in timed out".into() }));
        let buffer = buffer(&mut gui, 100, 30);
        let shown = sheet_rows(&lines(&buffer));
        assert_eq!(shown[8..13], [
            "│ Add to end of queue         ╭──────────────────────────────╮   │",
            "│ Keep a copy…  c             │ couldn't get links:          │   │",
            "│ Song Info                   │ the links plug-in timed out  │   │",
            "│ Open elsewhere…             │   Try again                  │   │",
            "│                             ╰──────────────────────────────╯   │",
        ].map(str::to_string), "{}", shown.join("\n"));
        assert_eq!(fg(&buffer, Position { x: 49, y: 17 }), th().gold);
        assert_eq!(bg(&buffer, Position { x: 48, y: 19 }), th().accent, "Try again held");
        assert_eq!(super::super::actions::tips(&gui).as_deref(), Some("Enter try again · Esc back"));
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::PluginLinks { .. }))), "{:?}", gui.pending);
        let shown = sheet_rows(&draw(&mut gui, 100, 30));
        assert!(shown[11].contains("asking…"));
    }

    #[test]
    fn the_copies_and_links_words_fit_their_cells_in_every_locale() {
        for code in ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
            let t = |key: &str| rust_i18n::t!(key, locale = code).to_string();
            // The STATE column's words with their glyph: 20 cells.
            for key in [
                "gui.discover.copies.kept",
                "gui.discover.copies.yours",
                "gui.discover.copies.skipped",
                "gui.discover.copies.nothing",
                "gui.discover.copies.done",
                "gui.discover.copies.stopping",
                "gui.discover.mark.waiting",
                "gui.discover.mark.failed",
                "gui.discover.mark.stopped",
                "gui.discover.mark.refused",
                "gui.discover.mark.cancelled",
            ] {
                let shown = format!("✓ {}", t(key));
                assert!(crate::kit::width(&shown) <= 20, "{code}: {key} is {} cells: {shown}", crate::kit::width(&shown));
            }
            // The header's labels, in their columns.
            assert!(crate::kit::width(&t("gui.discover.copies.col_title")) <= 31);
            assert!(crate::kit::width(&t("gui.discover.copies.col_from")) <= 12, "{code}");
            assert!(crate::kit::width(&t("gui.discover.copies.col_state")) <= 20);
            // The modal's lines: the table's 72 cells.
            for key in ["gui.discover.copies.asking", "gui.discover.copies.empty", "gui.discover.copies.empty_sub"] {
                assert!(crate::kit::width(&t(key)) <= 72, "{code}: {key}: {}", t(key));
            }
            let no_answer = t("gui.discover.keep.no_answer");
            let load = rust_i18n::t!("gui.discover.copies.load_failed", locale = code, words = no_answer).to_string();
            assert!(crate::kit::width(&load) <= 72, "{code}: {load}");
            // The links list's heads and its words: its 30 cells.
            for key in ["gui.discover.links.canonical", "gui.discover.links.search", "gui.discover.links.none", "gui.discover.links.failed"] {
                assert!(crate::kit::width(&t(key)) <= 28, "{code}: {key}: {}", t(key));
            }
            // Every tips line the Copies modal composes, and the links'.
            let rows = t("gui.tips.discover_copies_rows");
            let clear = t("gui.tips.discover_copies_clear");
            let esc = t("gui.tips.discover_copies_esc");
            for verb in ["cancel", "retry", "start_again", "play"] {
                let key = format!("gui.tips.discover_copies_{verb}");
                let line = [rows.clone(), t(&key), clear.clone(), esc.clone()].join(" · ");
                assert!(crate::kit::width(&line) <= 99, "{code}: {line}");
            }
            for key in ["gui.tips.discover_links", "gui.tips.discover_links_failed", "gui.tips.discover_links_wait"] {
                assert!(crate::kit::width(&t(key)) <= 99, "{code}: {key}");
            }
        }
    }

    #[test]
    fn a_links_hand_off_is_worded_by_how_it_went_and_the_pointer_opens_the_list_unheld() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        draw(&mut gui, 100, 30);
        gui.act(Act::Disc(DiscAct::More(2)));
        draw(&mut gui, 100, 30);
        // A click on the row: the list asked, nothing held.
        assert_eq!(gui.ui.hit(Position { x: 25, y: 19 }), Some(Act::SheetVerb(super::super::actions::SheetAction::OpenElsewhere)));
        gui.act(Act::SheetVerb(super::super::actions::SheetAction::OpenElsewhere));
        land_links(&mut gui, Ok(links_answer()));
        let buffer = buffer(&mut gui, 100, 30);
        assert!((10..21).all(|y| bg(&buffer, Position { x: 60, y }) != th().accent), "nothing held");
        // A click on a link opens it and keeps the list.
        let rows = lines(&buffer);
        let youtube = at(&rows, "YouTube");
        assert_eq!(gui.ui.hit(youtube), Some(Act::Disc(DiscAct::LinkOpen(5))));
        gui.act(Act::Disc(DiscAct::LinkOpen(5)));
        assert_eq!(gui.app.message.as_ref().map(|m| m.text.as_str()), Some("opening YouTube…"));
        // How a hand-off went: the system took it (TEXT), or nothing did
        // (GOLD).
        let send = |gui: &Gui, answer: Result<super::HandOff, String>| {
            gui.discover.links.tx.send(("Deezer".into(), "https://www.deezer.com/x".into(), answer)).unwrap();
        };
        let settle = |gui: &mut Gui| {
            for _ in 0..200 {
                draw(gui, 100, 30);
                if gui.app.message.as_ref().is_some_and(|m| m.kind != crate::tui::app::MessageKind::Busy) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            // The test's own YouTube hand-off may land after: wait it out.
            std::thread::sleep(std::time::Duration::from_millis(50));
            draw(gui, 100, 30);
        };
        settle(&mut gui);
        send(&gui, Ok(super::HandOff::Launched));
        draw(&mut gui, 100, 30);
        let note = gui.app.message.clone().unwrap();
        assert_eq!((note.text.as_str(), note.kind), ("opened Deezer", crate::tui::app::MessageKind::Plain));
        send(&gui, Ok(super::HandOff::Nothing));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let note = gui.app.message.clone().unwrap();
        assert_eq!((note.text.as_str(), note.kind), ("Deezer: no app took it", crate::tui::app::MessageKind::Error));
        let words = at(&lines(&buffer), "Deezer: no app took it");
        assert_eq!(fg(&buffer, words), th().gold);
        send(&gui, Err("No such file or directory".into()));
        draw(&mut gui, 100, 30);
        assert_eq!(gui.app.message.as_ref().map(|m| m.text.as_str()), Some("Deezer: no app took it"));
        // Only https reaches the opener: the javascript: link never shows.
        assert!(super::web_links(&gui.app).iter().all(|l| l.url.starts_with("https://")));
    }

    #[test]
    fn the_legacy_console_frames_the_links_list_in_ascii() {
        let _en = english();
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let mut gui = links_gui();
        let shown = sheet_rows(&draw(&mut gui, 100, 30));
        assert_eq!(shown[10..13].iter().map(|r| r.chars().skip(30).take(32).collect::<String>()).collect::<Vec<_>>(), [
            "+------------------------------+",
            "| asking...                    |",
            "+------------------------------+",
        ]);
        assert!(shown[11].contains(" Open elsewhere...   "), "{}", shown[11]);
    }

    // ── Review fixes (2026-10-20) ───────────────────────────────────────

    #[test]
    fn the_narrow_room_squeezes_the_indicator_to_glyphs_in_every_state() {
        let _en = english();
        let jobs = canvas_jobs();
        let live: Vec<PluginJob> = jobs.iter().filter(|j| j.state != crate::api::types::JobState::Failed).cloned().collect();
        let failed: Vec<PluginJob> = jobs.iter().filter(|j| !j.live()).cloned().collect();
        // The TermRoom table: the 80-cell room says words, the 47-cell room
        // (the queue open) its glyphs whatever the words would fit.
        for (jobs, words, glyph, token) in [(live, "copying…", "↓", th().accent), (failed, "! failed", "!", th().gold)] {
            let mut gui = canvas_gui(&jobs);
            gui.ui.key_hints = true;
            let rows = draw(&mut gui, 100, 30);
            assert!(rows[14].trim_end().ends_with(words), "{}", rows[14]);
            gui.queue_open = true;
            let buffer = buffer(&mut gui, 100, 30);
            let rows = lines(&buffer);
            assert!(!rows[14].contains(words), "glyphs only in the narrow room: {}", rows[14]);
            assert!(room(&rows, 14).starts_with("▸ play · + queue all · ⇄ shuffle"), "the verbs stand: {}", rows[14]);
            assert_eq!(buffer[(63, 14)].symbol(), glyph, "flush with the room's right: {}", rows[14]);
            assert_eq!(buffer[(62, 14)].symbol(), " ");
            assert_eq!(fg(&buffer, Position { x: 63, y: 14 }), token);
            assert!(gui.ui.tips.iter().any(|(r, t)| r.x == 63 && *t == format!("{words} — Copies — C")), "the words in its tip");
            // The pointer brightens it whole.
            gui.ui.pointer = Some(Position { x: 63, y: 14 });
            let buffer = super::tests::buffer(&mut gui, 100, 30);
            assert_eq!((fg(&buffer, Position { x: 63, y: 14 }), bold(&buffer, Position { x: 63, y: 14 })), (th().bright, true));
        }
    }

    /// A peers answer: Stage Peer B's rows (title, artist, album, match),
    /// Stage Peer C's Rooftop Cypher.
    fn peers_with(b_rows: &[(&str, &str, &str, f64)]) -> TasteResponse {
        let b: Vec<Value> = b_rows.iter().map(|(t, a, al, s)| fed(B, t, a, al, *s, Some(9.0))).collect();
        serde_json::from_value(json!({
            "seedSource": "history", "seeds": [], "library": null, "network": null,
            "peers": {
                "peers": [
                    { "id": B, "name": name(B), "reachable": true, "results": b },
                    { "id": C, "name": name(C), "reachable": true, "results": [
                        fed(C, "Rooftop Cypher", "Boukmanflow", "Borrowed Tapes", 0.6, Some(12.0)),
                    ] }
                ],
                "searched": { "peers": 2, "unreachable": 0, "mismatched": 0 }
            }
        }))
        .unwrap()
    }

    /// `G` asked again; its answer lands — the main part as before, the
    /// peers part `peers`.
    fn land_again(gui: &mut Gui, peers: TasteResponse) {
        let (seq, generation) = pending_taste(gui, TastePart::Main);
        apply(gui, Event::Taste { seq, generation, part: TastePart::Main, answer: Ok(Box::new(main_answer())) });
        let (seq, generation) = pending_taste(gui, TastePart::Peers);
        apply(gui, Event::Taste { seq, generation, part: TastePart::Peers, answer: Ok(Box::new(peers)) });
    }

    fn links_rec(gui: &Gui) -> Option<String> {
        gui.pending.iter().rev().find_map(|e| match e {
            Effect::Api(ApiCmd::PluginLinks { rec, .. }) => rec.get("title").and_then(Value::as_str).map(str::to_string),
            _ => None,
        })
    }

    #[test]
    fn the_sheet_acts_on_its_own_row_when_a_new_answer_lands_under_it() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        // `G` asks again; the old answer stands until the new lands.
        key(&mut gui, KeyCode::Char('G'));
        draw(&mut gui, 100, 30);
        gui.act(Act::Disc(DiscAct::More(2)));
        assert_eq!(gui.actions.sheet.as_ref().and_then(|s| s.track.metadata.title.as_deref()), Some("Late Train Verse"));
        // The new answer puts another song where the sheet's row stood.
        land_again(&mut gui, peers_with(&[
            ("Glass Hours", "Color Out", "Peer Sessions", 0.95),
            ("Northern Signal", "Color Out", "Peer Sessions", 0.9),
            ("Fresh Song", "Color Out", "New Light", 0.97),
            ("Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99),
        ]));
        assert_eq!(gui.app.taste.peer_row(0, 2).and_then(|(_, l)| l.title.as_deref()), Some("Fresh Song"));
        let all = draw(&mut gui, 100, 30).join("\n");
        assert!(all.contains("from Stage Peer B") && all.contains("Keep a copy…  c") && all.contains("Open elsewhere…"), "{all}");
        // Open elsewhere asks for the sheet's song.
        gui.pending.clear();
        gui.act(Act::SheetVerb(super::super::actions::SheetAction::OpenElsewhere));
        assert_eq!(links_rec(&gui).as_deref(), Some("Late Train Verse"));
        key(&mut gui, KeyCode::Esc);
        // Keep a copy opens on it, its row sent unchanged.
        key(&mut gui, KeyCode::Char('c'));
        let flow = gui.app.taste.keep.flow.as_ref().expect("Keep a copy");
        assert_eq!(flow.subject.title.as_deref(), Some("Late Train Verse"));
        assert_eq!(flow.subject.rec, fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(9.0)));
        key(&mut gui, KeyCode::Esc);
    }

    #[test]
    fn the_sheet_offers_no_copy_or_links_once_a_new_answer_left_its_row_out() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        key(&mut gui, KeyCode::Char('G'));
        draw(&mut gui, 100, 30);
        gui.act(Act::Disc(DiscAct::More(2)));
        // Open elsewhere's list asked and failed, then the row leaves.
        draw(&mut gui, 100, 30);
        for _ in 0..5 {
            key(&mut gui, KeyCode::Down);
        }
        key(&mut gui, KeyCode::Enter);
        land_links(&mut gui, Err(crate::api::PluginFailure::Refused { status: 504, words: "timed out".into() }));
        land_again(&mut gui, peers_with(&[
            ("Glass Hours", "Color Out", "Peer Sessions", 0.95),
            ("Northern Signal", "Color Out", "Peer Sessions", 0.9),
            ("Fresh Song", "Color Out", "New Light", 0.97),
        ]));
        // The list hangs on and Try again asks for the row it was asked for.
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert_eq!(links_rec(&gui).as_deref(), Some("Late Train Verse"), "{:?}", gui.pending);
        key(&mut gui, KeyCode::Esc);
        // Folded, the sheet offers neither: what they would send is gone.
        let rows = draw(&mut gui, 100, 30);
        let all = rows.join("\n");
        assert!(!all.contains("Keep a copy") && !all.contains("Open elsewhere"), "{all}");
        assert!(all.contains("from Stage Peer B"), "the badge names the sheet's server: {all}");
        assert_eq!(super::super::actions::tips(&gui).as_deref(), Some("↑ ↓ rows · Enter choose · Esc close"));
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.taste.keep.flow.is_none(), "c starts nothing");
        assert!(gui.actions.sheet.is_some());
    }

    #[test]
    fn the_tiles_list_follows_its_album_through_a_new_answer_or_folds() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        key(&mut gui, KeyCode::Char('G'));
        draw(&mut gui, 100, 30);
        gui.act(Act::Disc(DiscAct::TileList(0)));
        assert_eq!(gui.app.taste.peer_albums()[0].album, "Borrowed Tapes");
        // A new answer ranks another album first: Borrowed Tapes is the
        // second tile now, and the list hangs from it.
        land_again(&mut gui, peers_with(&[
            ("Fresh Song", "Color Out", "New Light", 1.0),
            ("Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99),
        ]));
        assert_eq!(gui.app.taste.peer_albums()[0].album, "New Light");
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(rows[13].chars().skip(29).take(24).collect::<String>(), "╭──────────────────────╮", "{}", rows.join("\n"));
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('a'));
        let queued = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::TasteAlbum { name, play: false, .. }) => Some(name.clone()),
            _ => None,
        });
        assert_eq!(queued.as_deref(), Some("Borrowed Tapes"), "{:?}", gui.pending);
        gui.act(Act::Disc(DiscAct::TileList(1)));
        key(&mut gui, KeyCode::Char('c'));
        let flow = gui.app.taste.keep.flow.as_ref().expect("Keep a copy");
        assert!(flow.subject.album_focus() && flow.subject.album.as_deref() == Some("Borrowed Tapes"));
        key(&mut gui, KeyCode::Esc);
        // A newer answer without it: the list folds, nothing acts.
        key(&mut gui, KeyCode::Char('G'));
        draw(&mut gui, 100, 30);
        gui.act(Act::Disc(DiscAct::TileList(1)));
        land_again(&mut gui, peers_with(&[("Fresh Song", "Color Out", "New Light", 1.0)]));
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.discover.tile_list.is_none() && gui.app.taste.keep.flow.is_none());
        // Opened anew, the second tile's list is that tile's: Stage Peer
        // C's Borrowed Tapes.
        gui.act(Act::Disc(DiscAct::TileList(1)));
        draw(&mut gui, 100, 30);
        assert_eq!(gui.discover.tile_list.as_ref().map(|l| l.pin.server()), Some("Stage Peer C"));
    }

    #[test]
    fn a_folder_copy_is_no_discover_copy_anywhere() {
        let _en = english();
        let rec = rec_on(B, "Northern Signal", "Color Out", "Peer Sessions");
        let folder = |state: &str, extra: Value| job(50, &rec, Some("folder"), state, extra);
        // Failed (the webapp's peer browser's): not listed, no indicator,
        // no inert Retry.
        let mut gui = copies_gui(&[folder("failed", json!({ "error": "Stage Peer B is unreachable" }))]);
        let all = draw(&mut gui, 100, 30).join("\n");
        assert!(all.contains("Nothing copying."), "{all}");
        assert!(gui.app.copies_rows().is_empty() && gui.app.keep_indicator().is_none());
        assert!(super::copies_tips(&gui).contains("Esc close") && !super::copies_tips(&gui).contains("retry"));
        // Live, then finished: no poll kept going for it, no note.
        key(&mut gui, KeyCode::Esc);
        let mut gui = canvas_gui(&[]);
        gui.act(Act::Disc(DiscAct::Copies));
        land_jobs(&mut gui, &[folder("running", json!({}))]);
        assert!(gui.app.keep_indicator().is_none());
        key(&mut gui, KeyCode::Esc);
        draw(&mut gui, 100, 30);
        gui.pending.clear();
        let later = crate::clock::Instant::now() + std::time::Duration::from_secs(30);
        let effects = gui.app.keep_tick(later);
        assert!(effects.is_empty(), "nothing Discover's is live: {effects:?}");
        gui.act(Act::Disc(DiscAct::Copies));
        land_jobs(&mut gui, &[folder("done", json!({ "result": { "scope": "folder" } }))]);
        draw(&mut gui, 100, 30);
        assert!(gui.app.message.is_none(), "{:?}", gui.app.message);
    }

    #[test]
    fn the_quick_beat_holds_only_while_a_copys_own_surface_shows_it() {
        use crate::tui::app::keep::KeepWatch;
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        draw(&mut gui, 100, 30);
        assert_eq!(gui.app.taste.keep.watch, KeepWatch::Room);
        // Keep a copy on Northern Signal, whose copies are done: the live
        // copies of others keep the room's beat.
        super::open(&mut gui, KeepTarget::Row { group: 0, row: 1 });
        assert_eq!(gui.app.taste.keep.flow.as_ref().and_then(|f| f.subject.title.as_deref()), Some("Northern Signal"));
        draw(&mut gui, 100, 30);
        assert_eq!(gui.app.taste.keep.watch, KeepWatch::Room);
        key(&mut gui, KeyCode::Esc);
        // On Late Train Verse, whose album is copying: the quick beat.
        super::open(&mut gui, LATE);
        draw(&mut gui, 100, 30);
        assert_eq!(gui.app.taste.keep.watch, KeepWatch::Modal);
        key(&mut gui, KeyCode::Esc);
        // The Copies modal: the quick beat.
        gui.act(Act::Disc(DiscAct::Copies));
        draw(&mut gui, 100, 30);
        assert_eq!(gui.app.taste.keep.watch, KeepWatch::Modal);
    }

    #[test]
    fn a_stop_this_client_does_not_know_is_worded_neutrally() {
        let _en = english();
        let rec = rec_on(B, "Glass Hours", "Color Out", "Peer Sessions");
        let stopped = job(60, &rec, Some("album"), "done",
            json!({ "result": { "scope": "album", "counts": { "total": 3, "copied": 2, "skipped": 0, "failed": 0 }, "stopped": "maintenance" } }));
        let (note, kind) = super::news_note(&stopped, 99).expect("a note");
        assert_eq!((note.as_str(), kind), ("copy stopped: ended early · Peer Sessions", crate::tui::app::MessageKind::Error));
        // Its short form fits the floor's 30 note cells in every locale.
        for code in ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
            let why = rust_i18n::t!("gui.discover.note.why_other", locale = code).to_string();
            let head = rust_i18n::t!("gui.discover.note.stopped", locale = code, why = why).to_string();
            assert!(crate::kit::width(&head) <= 30, "{code}: {head}");
        }
        let mut gui = copies_gui(&[stopped]);
        let rows = copies_rows_on(&draw(&mut gui, 100, 30));
        let all = rows.join("\n");
        assert!(all.contains("! stopped") && all.contains("the copy stopped before it finished") && all.contains("Retry"), "{all}");
        assert!(!all.contains("in your collection"), "{all}");
    }
}
