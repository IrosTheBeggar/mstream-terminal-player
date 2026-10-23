//! The Discover room's Keep a copy modal (docs/ux-contracts/discovery-page.md,
//! clauses 106–110; the canvas's TermKeepModal and TermKeepStates boards),
//! the room's job marks' glyphs and words (clauses 113–114, 124), the
//! Copies modal and Open elsewhere's list (clauses 117, 140–141; the
//! canvas's TermCopies board), and the status line's copy and link notes
//! (clauses 118, 141) — the GUI half of the App's `tui::app::keep`, which
//! holds every fact drawn here.
//!
//! The modal is the kit's, ACCENT, 66 cells wide, anchored as if 17 rows
//! tall so its top holds while its lower region changes: the title and
//! `[X]`, a blank, the header (a 6 × 3 pixel cover, the title BOLD, a DIM
//! byline, "• online"), a blank, then the lower region — the choice (WHAT,
//! the radios and their sizes, "Only the albums you don't have"), LANDS IN,
//! whose place it is with `[ Change… ]`, and the buttons; once a job exists
//! the choice folds to one WHAT line and the kit's three-line scan widget
//! follows the copy to its outcome. `[ Change… ]` (or `d`) opens **Where
//! files land** in its place (clauses 142–148; the canvas's TermDestination
//! board): the account's destination, edited and saved on the server.

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::api::types::{
    CollectionDestination, CollectionDestinationView, JobAction, JobMark, JobOutcome, JobScope, Landing, ManyCounts,
    PluginJob, PluginLink, Run, SongTag, StopReason, base_folders,
};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, dim, scroll_list, width};
use crate::tui::app::keep::{
    ChoiceSize, CopyRow, CopyTitle, Fact, Folders, KeepChoice, KeepNote, KeepPhase, KeepStage, KeepVerb, KeepView,
    LinksStatus,
};
use crate::tui::app::MessageKind;

use super::cover::{Pace, Slot};
use super::discover::DiscAct;
use super::opener::{HandOff, open_target};
use super::{Act, Gui, accent, bright_bold, put, sel};

/// The modal's width (clause 106): at least 44, at most the area less 2.
pub(crate) const KEEP_W: u16 = 66;
/// The height it anchors as: its tallest common state, so the top holds.
const KEEP_ANCHOR: u16 = 17;
/// The header's rows above the lower region: the title, a blank, three
/// header rows and a blank.
const HEAD_ROWS: u16 = 6;
/// Where the radios (and the folded WHAT, the crumbs) and the sizes stand,
/// from the inner rect's left — the canvas's columns; a locale whose WHAT
/// or LANDS IN is wider moves the values right ([`value_col`]).
const RADIO_X: u16 = 11;
const SIZE_X: u16 = 43;

/// The values' column: the canvas's, or two cells past the wider label.
fn value_col() -> u16 {
    let label = |key: &str| width(&t!(key).to_uppercase()) as u16 + 2;
    RADIO_X.max(label("gui.discover.keep.what")).max(label("gui.discover.keep.lands_in"))
}

// ── State ───────────────────────────────────────────────────────────────────

/// The modal's own state: the keyboard's stop and the header's cover.
/// Everything it shows is the App's flow ([`crate::tui::app::keep::KeepFlow`]).
#[derive(Default)]
pub(crate) struct KeepUi {
    /// The keyboard's stop; `None` is stowed (Enter then means the
    /// primary, as the tips say).
    pub focus: Option<Stop>,
    /// A start was out last frame: when the choice comes back (a refusal,
    /// another server's copy) the hand comes back to its radio.
    starting: bool,
    /// The header cover's slot: pixels while the modal is the top layer.
    slot: Option<Slot>,
    /// "Where files land" while it is open over the modal (clause 142).
    pub dest: Option<DestUi>,
}

impl KeepUi {
    pub(crate) fn on_resize(&mut self) {
        if let Some(slot) = &mut self.slot {
            slot.on_resize();
        }
    }
}

/// One keyboard stop of the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    Choice(KeepChoice),
    /// "Only the albums you don't have".
    Missing,
    /// `[ Change… ]` on the SOURCE row: Where files land.
    Change,
    /// The left button: Cancel, Close, Add to queue or Cancel copy.
    Secondary,
    /// The right one, ACCENT BOLD: Add …, Retry, Copy again, Start again,
    /// Play your copy, or Close where nothing else is offered.
    Primary,
}

/// What the modal's pointer and keys do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeepAct {
    /// Esc, `[X]`, Cancel, Close: the copy keeps running (clause 109).
    Close,
    /// A radio, clicked.
    Pick(KeepChoice),
    /// The checkbox, clicked.
    Missing,
    /// `[ Change… ]`, clicked: Where files land.
    Change,
    Secondary,
    Primary,
    /// The picker's controls (clauses 142–144).
    Dest(DestAct),
}

/// A button of the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Cancel,
    Close,
    CancelCopy,
    AddToQueue,
    Add(KeepVerb),
    Again(JobAction),
}

impl Button {
    fn label(self) -> String {
        match self {
            Button::Cancel => t!("gui.discover.keep.cancel").to_string(),
            Button::Close => t!("gui.discover.keep.close").to_string(),
            Button::CancelCopy => t!("gui.discover.keep.cancel_copy").to_string(),
            Button::AddToQueue => t!("gui.discover.keep.add_to_queue").to_string(),
            Button::Add(verb) => verb_label(verb),
            Button::Again(JobAction::Retry) => t!("gui.discover.keep.retry").to_string(),
            Button::Again(JobAction::CopyAgain) => t!("gui.discover.keep.copy_again").to_string(),
            Button::Again(JobAction::StartAgain) => t!("gui.discover.keep.start_again").to_string(),
            Button::Again(JobAction::Play) => t!("gui.discover.keep.play_copy").to_string(),
            Button::Again(JobAction::Cancel) => t!("gui.discover.keep.cancel_copy").to_string(),
        }
    }
}

/// The primary's words, which follow the choice (clause 106).
fn verb_label(verb: KeepVerb) -> String {
    match verb {
        KeepVerb::Song => t!("gui.discover.keep.add_song").to_string(),
        KeepVerb::Album => t!("gui.discover.keep.add_album").to_string(),
        KeepVerb::Artist => t!("gui.discover.keep.add_artist").to_string(),
        KeepVerb::Missing(n) => plural("gui.discover.keep.add_missing", n as u64),
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

fn crumb_sep() -> &'static str {
    if legacy_conhost() { " > " } else { " › " }
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

/// A path's tooltip: the whole path where its rows clip it, the slot's
/// note where it shows one — the 66-cell LANDS IN and the one-row PREVIEW
/// have no row for the note, so it stands here.
fn path_tip(landing: &Landing, clipped: bool) -> Option<String> {
    let path = clipped.then(|| landing.words(&slot_words).join("/"));
    match (path, slot_note(landing)) {
        (Some(path), Some(note)) => Some(format!("{path} — {note}")),
        (path, note) => path.or(note),
    }
}

/// The cells LANDS IN's path has on a row: from the values' column to the
/// last content cell of the 66-cell modal.
fn lands_room() -> usize {
    usize::from((KEEP_W - 3).saturating_sub(value_col()))
}

/// The cells a row of runs takes.
fn cells_of(runs: &[(String, Style)]) -> usize {
    runs.iter().map(|(text, _)| width(text)).sum()
}

/// A path's runs in rows of `room` cells (clause 105): after `lead`, the
/// crumbs with `›` DIM between them; where they outgrow the first row
/// they break before a crumb, ` ›` ending it, and the rest stands on a
/// second — clipped at the right there, never cut from the left, so the
/// library always shows.
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

/// A landing as the modal's rows: LANDS IN's (no `lead`), or the "retry
/// adds to" words and the place, under it.
fn path_lows(lead: Option<String>, landing: &Landing) -> Vec<Low> {
    let room = lands_room();
    let first = lead.is_none();
    let lead = lead.map(|words| vec![(format!("{words} "), dim())]).unwrap_or_default();
    let rows = path_rows(lead, &landing_crumbs(landing), room);
    let tip = path_tip(landing, rows.iter().any(|row| cells_of(row) > room));
    rows.into_iter().enumerate().map(|(i, runs)| Low::Path { first: first && i == 0, runs, tip: tip.clone() }).collect()
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
        JobMark::Cancelled => {
            if legacy {
                "x"
            } else {
                "×"
            }
        }
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

/// A count's words in the modal: "3 songs", "1 copied".
fn count(stem: &str, n: u64) -> String {
    plural(stem, n)
}

/// A choice's name, as its radio and the folded WHAT line say it.
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

/// A choice's size, DIM beside its radio (clause 106): `…` while asked,
/// nothing when unknown.
fn size_words(size: &ChoiceSize) -> Option<String> {
    let dots = || ell("…");
    match size {
        ChoiceSize::Song { seconds, format } => {
            let parts: Vec<String> = seconds
                .filter(|s| *s > 0.0)
                .map(super::bar::fmt_time)
                .into_iter()
                .chain(format.clone())
                .collect();
            (!parts.is_empty()).then(|| parts.join(" · "))
        }
        ChoiceSize::Album { songs } => match songs {
            Fact::Asking => Some(dots()),
            Fact::Known(n) => Some(count("gui.discover.keep.songs", *n as u64)),
            Fact::Unknown => None,
        },
        ChoiceSize::Artist { albums, have } => match (albums, have) {
            (Fact::Asking, _) | (_, Fact::Asking) => Some(dots()),
            (Fact::Known(a), Fact::Known(h)) if *h > 0 => Some(format!(
                "{} · {}",
                count("gui.discover.keep.albums", *a as u64),
                count("gui.discover.keep.you_have", *h as u64)
            )),
            (Fact::Known(a), _) => Some(count("gui.discover.keep.albums", *a as u64)),
            (Fact::Unknown, Fact::Known(h)) if *h > 0 => Some(count("gui.discover.keep.you_have", *h as u64)),
            _ => None,
        },
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
/// whole on one line.
pub(crate) fn wrap_keeping(text: &str, width_cells: usize, keep: &[&str]) -> Vec<String> {
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

// ── The lower region, worked out ────────────────────────────────────────────

/// One row of the modal's lower region.
#[derive(Debug, Clone, PartialEq)]
enum Low {
    Blank,
    /// A radio: its choice, its size, whether it is the first (WHAT).
    Choice(KeepChoice, Option<String>, bool),
    /// "Only the albums you don't have", ticked or not.
    Missing(bool),
    /// The folded choice: WHAT and the copy's scope in words, its size.
    Folded(String, Option<String>),
    /// A row of where a copy lands (clause 105): LANDS IN's — the label on
    /// its `first` row — or "retry adds to …" under it: the library, its
    /// folders, an artist copy's "one folder per album" (a hint, not a
    /// folder, so DIM), wrapped at a crumb onto a second row where they
    /// outgrow one ([`path_rows`]); `tip` the whole path where even that
    /// is clipped, and a slot's note where one shows ([`path_tip`]).
    Path { first: bool, runs: Vec<(String, Style)>, tip: Option<String> },
    /// The SOURCE row under it: whose place it is (saved for the account,
    /// or the library's default) and `[ Change… ]` (clause 105).
    Source(bool),
    /// One row of words in a token.
    Text(String, Style),
    /// Runs of words in their tokens, ` · ` between them DIM.
    Runs(Vec<(String, Style)>),
    /// The scan widget's bar: the progress, `None` with no estimate.
    Bar(Option<f64>),
    /// The buttons, bottom-right: the secondary and the primary.
    Buttons(Option<Button>, Option<Button>),
}

/// The scope a folded WHAT line names, and its size.
fn folded(view: &KeepView, scope: JobScope) -> Low {
    let size_of = |choice: KeepChoice| {
        view.choices.iter().find(|(c, _)| *c == choice).and_then(|(_, size)| match size {
            ChoiceSize::Artist { .. } => None,
            other => size_words(other).filter(|w| w != &ell("…")),
        })
    };
    match scope {
        JobScope::Song => Low::Folded(choice_name(KeepChoice::Song, view), size_of(KeepChoice::Song)),
        JobScope::Album => Low::Folded(choice_name(KeepChoice::Album, view), size_of(KeepChoice::Album)),
        // Only the albums you don't have: how many it takes.
        JobScope::ArtistMissing => Low::Folded(
            choice_name(KeepChoice::Artist, view),
            view.missing.map(|(_, n)| count("gui.discover.keep.albums", n as u64)),
        ),
        _ => Low::Folded(choice_name(KeepChoice::Artist, view), None),
    }
}

/// A many-song copy's counts as runs (clause 110): copied, already yours,
/// failed (GOLD), albums left out — what the result's counts hold.
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

/// The copy's three scan rows and its buttons (clauses 109–110).
fn job_rows(view: &KeepView, job: &PluginJob, outcome: &JobOutcome, action: Option<JobAction>) -> (Vec<Low>, Option<Button>, Option<Button>) {
    let gold = Style::default().fg(th().gold);
    let ok = Style::default().fg(th().ok);
    let server = job.peer_name().unwrap_or_else(|| view.subject.server.clone());
    let wrap = |text: String, style: Style| -> Vec<Low> {
        wrap_keeping(&text, usize::from(KEEP_W - 4), &[server.as_str()]).into_iter().map(|line| Low::Text(line, style)).collect()
    };
    let mut rows: Vec<Low> = Vec::new();
    match outcome {
        JobOutcome::Queued => {
            rows.push(Low::Text(t!("gui.discover.keep.waiting").to_string(), Style::default()));
            rows.push(Low::Bar(None));
            rows.push(Low::Text(t!("gui.discover.keep.waiting_sub").to_string(), dim()));
        }
        JobOutcome::Running { progress, step, stopping } => {
            let step = if *stopping {
                ell(&t!("gui.discover.keep.stopping"))
            } else {
                step.clone().unwrap_or_else(|| ell(&t!("gui.discover.keep.copying")))
            };
            rows.push(Low::Text(step, Style::default()));
            rows.push(Low::Bar(*progress));
            match job.copy_result().many() {
                Some(n) => rows.push(Low::Runs(vec![
                    (count("gui.discover.keep.copied", n.copied), dim()),
                    (count("gui.discover.keep.failed", n.failed), dim()),
                ])),
                None => rows.push(Low::Blank),
            }
        }
        JobOutcome::Copied { filepath, .. } => {
            rows.push(Low::Runs(vec![(format!("{} {}", tick(), t!("gui.discover.keep.kept")), ok)]));
            rows.push(Low::Text(super::clip_lead(filepath, usize::from(KEEP_W - 4)), dim()));
        }
        JobOutcome::CopiedMany(n) => {
            rows.push(Low::Runs(vec![(format!("{} {}", tick(), t!("gui.discover.keep.kept")), ok)]));
            rows.push(Low::Runs(many_runs(n, ok)));
            rows.push(Low::Text(t!("gui.discover.keep.gaps").to_string(), dim()));
        }
        JobOutcome::Owned { many, .. } => {
            rows.push(Low::Runs(vec![(format!("{} {}", tick(), t!("gui.discover.keep.yours")), ok)]));
            let has = if many.is_some() { t!("gui.discover.keep.yours_many") } else { t!("gui.discover.keep.yours_song") };
            rows.push(Low::Text(format!("{has} · {}", t!("gui.discover.keep.nothing_copied")), dim()));
        }
        JobOutcome::Exists { filepath } => {
            rows.push(Low::Text(t!("gui.discover.keep.exists").to_string(), gold));
            if let Some(path) = filepath {
                rows.push(Low::Text(super::clip_lead(path, usize::from(KEEP_W - 4)), dim()));
            }
            rows.push(Low::Text(t!("gui.discover.keep.exists_sub").to_string(), dim()));
        }
        JobOutcome::Stopped { why, many } => {
            let reason = match why {
                StopReason::Quota => t!("gui.discover.keep.stopped_quota", server = server),
                StopReason::Busy => t!("gui.discover.keep.stopped_busy", server = server),
                StopReason::Peer | StopReason::Refused => t!("gui.discover.keep.stopped_peer", server = server),
                StopReason::Other => t!("gui.discover.keep.stopped_other"),
            };
            rows.extend(wrap(reason.to_string(), gold));
            let not = many.total.saturating_sub(many.copied + many.skipped);
            let mut runs = vec![(count("gui.discover.keep.copied", many.copied), dim())];
            if not > 0 {
                runs.push((count("gui.discover.keep.not_copied", not), dim()));
            }
            match why {
                StopReason::Quota => runs.push((t!("gui.discover.keep.later").to_string(), dim())),
                StopReason::Busy => runs.push((t!("gui.discover.keep.moment").to_string(), dim())),
                _ => {}
            }
            rows.push(Low::Runs(runs));
        }
        JobOutcome::Refused { many } => {
            rows.extend(wrap(t!("gui.discover.keep.refused", server = server).to_string(), gold));
            let copied = many.map(|n| n.copied).unwrap_or(0);
            let head = if copied > 0 {
                count("gui.discover.keep.copied", copied)
            } else {
                t!("gui.discover.keep.nothing_copied").to_string()
            };
            rows.push(Low::Text(format!("{head} · {}", t!("gui.discover.keep.refused_sub")), dim()));
        }
        JobOutcome::Failed { error, many } => {
            let words = error.clone().unwrap_or_else(|| t!("gui.discover.keep.failed_plain").to_string());
            let line = match error {
                Some(_) => t!("gui.discover.keep.failed", words = words).to_string(),
                None => words,
            };
            rows.extend(wrap(line, gold));
            let mut runs = vec![(t!("gui.discover.keep.nothing_copied").to_string(), dim())];
            if let Some(n) = many.filter(|n| n.failed > 0) {
                runs.push((count("gui.discover.keep.failed", n.failed), dim()));
            }
            rows.push(Low::Runs(runs));
        }
        JobOutcome::Cancelled { many } => {
            let mut head = t!("gui.discover.keep.cancelled").to_string();
            if let Some(n) = many.filter(|n| n.copied > 0) {
                head = format!("{head} · {}", count("gui.discover.keep.copied", n.copied));
            }
            rows.push(Low::Text(head, dim()));
            match many {
                Some(n) => {
                    let rest = n.total.saturating_sub(n.copied + n.skipped);
                    if rest > 0 {
                        let key = format!("gui.discover.keep.stay_{}", super::discover::plural_form(rest));
                        rows.push(Low::Text(t!(key.as_str(), count = rest, server = server).to_string(), dim()));
                    }
                }
                None => rows.push(Low::Text(t!("gui.discover.keep.nothing_copied").to_string(), dim())),
            }
        }
        JobOutcome::Nothing { appearances, .. } => {
            if *appearances > 0 {
                rows.push(Low::Text(t!("gui.discover.keep.nothing_theirs").to_string(), dim()));
                let artist = view.subject.track_artist.clone().unwrap_or_default();
                rows.push(Low::Text(t!("gui.discover.keep.appearances", artist = artist).to_string(), dim()));
            } else {
                let words = format!("{} {} · {}", tick(), t!("gui.discover.keep.nothing"), t!("gui.discover.keep.all_yours"));
                rows.push(Low::Runs(vec![(words, ok)]));
            }
        }
        JobOutcome::Done | JobOutcome::Unknown => {
            rows.push(Low::Text(t!("gui.discover.keep.finished").to_string(), dim()));
        }
    }
    // The buttons the outcome offers, the primary on the right.
    let (secondary, primary) = match (outcome, action) {
        (JobOutcome::Running { stopping: true, .. }, _) => (None, None),
        (_, Some(JobAction::Cancel)) => (Some(Button::CancelCopy), None),
        (_, Some(JobAction::Play)) => (Some(Button::AddToQueue), Some(Button::Again(JobAction::Play))),
        (_, Some(again)) => (Some(Button::Close), Some(Button::Again(again))),
        (_, None) => (None, Some(Button::Close)),
    };
    (rows, secondary, primary)
}

/// The whole lower region for a view (clauses 106–110, 147).
fn lower(view: &KeepView) -> Vec<Low> {
    let mut low: Vec<Low> = Vec::new();
    let lands = |low: &mut Vec<Low>| {
        if let Some(landing) = &view.landing {
            low.extend(path_lows(None, landing));
        }
        // Retry and Copy again land at the account's place now: where that
        // is not where this copy ran, the modal says so first (clause 145).
        if let (Some(again), KeepStage::Job { action: Some(action), .. }) = (&view.again, &view.stage) {
            let key = if *action == JobAction::Retry { "gui.discover.dest.retry_adds_to" } else { "gui.discover.dest.again_adds_to" };
            low.extend(path_lows(Some(t!(key).to_string()), again));
        }
    };
    let choice_block = |low: &mut Vec<Low>| {
        for (i, (choice, size)) in view.choices.iter().enumerate() {
            low.push(Low::Choice(*choice, size_words(size), i == 0));
            if *choice == KeepChoice::Artist
                && let Some((on, _)) = view.missing
            {
                low.push(Low::Missing(on));
            }
        }
        low.push(Low::Blank);
        lands(low);
        let saved = view.destination.as_ref().is_some_and(|d| d.source.as_deref() == Some("user"));
        low.push(Low::Source(saved));
    };
    match &view.stage {
        KeepStage::Choose { verb, other } => {
            choice_block(&mut low);
            match (verb, other) {
                (_, Some(other)) => {
                    low.push(Low::Blank);
                    let key = if matches!(view.scope, JobScope::Artist | JobScope::ArtistMissing) {
                        "gui.discover.keep.other_artist"
                    } else {
                        "gui.discover.keep.other_album"
                    };
                    let words = t!(key, other = other, server = view.subject.server).to_string();
                    let keep = [other.as_str(), view.subject.server.as_str()];
                    for line in wrap_keeping(&words, usize::from(KEEP_W - 4), &keep) {
                        low.push(Low::Text(line, accent()));
                    }
                }
                (Some(verb), None) => {
                    low.push(Low::Blank);
                    low.push(Low::Buttons(Some(Button::Cancel), Some(Button::Add(*verb))));
                }
                (None, None) => {}
            }
        }
        KeepStage::Refused { failure, verb } => {
            choice_block(&mut low);
            // A place no longer in force has its own line: LANDS IN above
            // already reads the one that is (clause 146).
            let words = match failure {
                crate::api::PluginFailure::DestinationChanged(_) => t!("gui.discover.dest.changed").to_string(),
                failure => t!("gui.discover.keep.refused_start", words = failure_words(failure)).to_string(),
            };
            for line in wrap_keeping(&words, usize::from(KEEP_W - 4), &[view.subject.server.as_str()]) {
                low.push(Low::Text(line, Style::default().fg(th().gold)));
            }
            low.push(Low::Buttons(Some(Button::Cancel), verb.map(Button::Add)));
        }
        // Nowhere to put a file: one plain line and Close (clause 147).
        KeepStage::Nowhere => {
            for line in wrap_keeping(&t!("gui.discover.dest.nowhere"), usize::from(KEEP_W - 4), &[]) {
                low.push(Low::Text(line, Style::default()));
            }
            low.push(Low::Blank);
            low.push(Low::Buttons(None, Some(Button::Close)));
        }
        KeepStage::Starting => {
            low.push(folded(view, view.scope));
            lands(&mut low);
            low.push(Low::Blank);
            low.push(Low::Text(ell(&t!("gui.discover.keep.starting")), accent()));
            low.extend([Low::Blank, Low::Blank, Low::Blank, Low::Blank]);
        }
        KeepStage::Job { job, outcome, action } => {
            low.push(folded(view, job.scope().unwrap_or(view.scope)));
            lands(&mut low);
            low.push(Low::Blank);
            let (rows, secondary, primary) = job_rows(view, job, outcome, *action);
            let n = rows.len();
            low.extend(rows);
            // The scan widget keeps its three rows, the frame its height.
            for _ in n..3 {
                low.push(Low::Blank);
            }
            low.push(Low::Blank);
            if secondary.is_some() || primary.is_some() {
                low.push(Low::Buttons(secondary, primary));
            } else {
                low.push(Low::Blank);
            }
        }
    }
    low
}

/// The keyboard's stops in this frame's lower region, in walking order.
fn stops_of(low: &[Low]) -> Vec<Stop> {
    let mut out = Vec::new();
    for line in low {
        match line {
            Low::Choice(choice, _, _) => out.push(Stop::Choice(*choice)),
            Low::Missing(_) => out.push(Stop::Missing),
            Low::Source(_) => out.push(Stop::Change),
            Low::Buttons(secondary, primary) => {
                if secondary.is_some() {
                    out.push(Stop::Secondary);
                }
                if primary.is_some() {
                    out.push(Stop::Primary);
                }
            }
            _ => {}
        }
    }
    out
}

fn buttons_of(low: &[Low]) -> (Option<Button>, Option<Button>) {
    low.iter()
        .find_map(|line| match line {
            Low::Buttons(s, p) => Some((*s, *p)),
            _ => None,
        })
        .unwrap_or((None, None))
}

/// The modal's height for a lower region.
fn height_of(low: &[Low]) -> u16 {
    HEAD_ROWS + low.len() as u16 + 2
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The legacy console's frame: `+ - |` (clause 124); elsewhere the kit's
/// rounded one. Clear beneath, the ground repainted, the footprint the
/// caller registered.
fn modal_box(frame: &mut Frame, rect: Rect) -> Rect {
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

/// The Keep a copy modal, drawn (and registered) over everything — or
/// Where files land in its place while that is open (clause 142).
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    // A picker whose view went (the gate's ask failed) gives way to Keep a
    // copy: nothing would draw it, and it would keep every key.
    if gui.discover.keep.dest.is_some() && gui.app.keep_dest_view().is_none() {
        close_dest(gui);
    }
    if gui.discover.keep.dest.is_some() {
        draw_dest(frame, gui, area);
        return;
    }
    let Some(view) = gui.app.keep_view() else { return };
    // Its subject's cover, asked through its own server once.
    let fetch = gui.app.keep_cover();
    gui.pend(fetch.into_iter().collect());
    let low = lower(&view);
    let stops = stops_of(&low);
    // A stop that left (a start answered, a copy ended) is the keyboard's
    // no longer: its hand goes down — and back on the chosen radio when a
    // start came back refused.
    if gui.discover.keep.focus.is_some_and(|f| !stops.contains(&f)) {
        gui.discover.keep.focus = None;
    }
    let choosing = matches!(view.stage, KeepStage::Choose { .. } | KeepStage::Refused { .. });
    if gui.discover.keep.starting && choosing && gui.discover.keep.focus.is_none() {
        gui.discover.keep.focus = Some(Stop::Choice(view.choice));
    }
    if !matches!(view.stage, KeepStage::Starting) {
        gui.discover.keep.starting = false;
    }
    let focus = gui.discover.keep.focus;
    // The room beneath is inert (the kit's Modal).
    gui.ui.click(area, Act::Guard);
    let h = height_of(&low);
    let w = KEEP_W.min(area.width.saturating_sub(2)).max(44);
    let rect = crate::kit::modal_rect(area, w, h, KEEP_ANCHOR.max(h));
    gui.ui.overlay(rect);
    gui.ui.modal_over();
    let inner = modal_box(frame, rect);
    gui.ui.click(rect, Act::Guard);
    put(frame, inner.x + 1, inner.y, &t!("gui.discover.keep.title"), accent().add_modifier(Modifier::BOLD));
    crate::kit::modal_close(frame, &mut gui.ui, inner, Act::Disc(DiscAct::Keep(KeepAct::Close)));

    draw_header(frame, gui, &view, inner, rect);

    for (y, line) in (inner.y + HEAD_ROWS..inner.bottom()).zip(low.iter()) {
        draw_low(frame, gui, &view, inner, y, line, focus);
    }
}

/// The header (clause 106): the cover, the title BOLD, the byline DIM and
/// "• online".
fn draw_header(frame: &mut Frame, gui: &mut Gui, view: &KeepView, inner: Rect, own: Rect) {
    let s = &view.subject;
    let cover = Rect { x: inner.x + 1, y: inner.y + 2, width: 6, height: 3 };
    {
        let Gui { app, discover, ui, hot, .. } = &mut *gui;
        let art = s.art.as_deref().and_then(|file| app.art.get(file)).and_then(|a| a.as_ref());
        match art {
            Some(art) => {
                let slot = discover.keep.slot.get_or_insert_with(|| Slot::new(app.graphics.fork()));
                // Pixels while the modal is the top layer; the mosaic
                // under a tooltip, and on the legacy console always.
                if legacy_conhost() || ui.covered_last_frame_by_another(cover, own) {
                    slot.draw_mosaic(frame, cover, art);
                } else {
                    let pace = Pace::frame();
                    slot.draw_paced(frame, cover, art, &pace);
                    if pace.starved() {
                        *hot = true;
                    }
                }
            }
            None => super::bar::cover_slot(frame, cover.x, cover.y, cover.width, cover.height),
        }
    }
    let tx = inner.x + 8;
    let room = usize::from(inner.right().saturating_sub(tx + 1));
    let title = match &s.title {
        Some(title) => title.clone(),
        None => s.album.clone().unwrap_or_default(),
    };
    put(frame, tx, inner.y + 2, &super::bar::clip(&title, room), Style::default().add_modifier(Modifier::BOLD));
    let year = s.year.filter(|y| *y > 0).map(|y| y.to_string());
    let from = t!("gui.act.from_server", server = s.server).to_string();
    let parts: Vec<String> = if s.album_focus() {
        [s.album_credit().map(str::to_string), year, Some(from)].into_iter().flatten().collect()
    } else {
        [s.track_artist.clone(), s.album.clone(), year, Some(from)].into_iter().flatten().filter(|p| !p.trim().is_empty()).collect()
    };
    put(frame, tx, inner.y + 3, &super::bar::clip(&parts.join(" · "), room), dim());
    put(frame, tx, inner.y + 4, super::bullet_glyph(), Style::default().fg(th().ok));
    put(frame, tx + 2, inner.y + 4, &t!("gui.act.online"), dim());
}

/// One row of the lower region.
#[allow(clippy::too_many_arguments)]
fn draw_low(frame: &mut Frame, gui: &mut Gui, view: &KeepView, inner: Rect, y: u16, line: &Low, focus: Option<Stop>) {
    let x = inner.x + 1;
    let room = usize::from(inner.width.saturating_sub(2));
    let label = |key: &str| t!(key).to_uppercase();
    match line {
        Low::Blank => {}
        Low::Choice(choice, size, first) => {
            if *first {
                put(frame, x, y, &label("gui.discover.keep.what"), dim());
            }
            let col = value_col();
            let label_x = col + 4;
            let row = Rect { x: inner.x + col, y, width: inner.width.saturating_sub(col + 1), height: 1 };
            let chosen = view.choice == *choice;
            let focused = focus == Some(Stop::Choice(*choice));
            let hover = gui.ui.hovers(row);
            let (on, off) = if legacy_conhost() { ("(*)", "( )") } else { ("(•)", "( )") };
            let glyph_style = if chosen { Style::default().fg(th().ok) } else { dim() };
            put(frame, inner.x + col, y, if chosen { on } else { off }, glyph_style);
            let name_style = match (focused, hover, chosen) {
                (true, _, _) => accent().add_modifier(Modifier::BOLD),
                (false, true, _) => bright_bold(),
                (false, false, true) => Style::default().add_modifier(Modifier::BOLD),
                (false, false, false) => Style::default(),
            };
            // The name keeps a cell before its size, or the row's rest.
            let name_room = match size {
                Some(_) => usize::from(SIZE_X.saturating_sub(label_x + 1)),
                None => room.saturating_sub(usize::from(label_x)),
            };
            put(frame, inner.x + label_x, y, &super::bar::clip(&choice_name(*choice, view), name_room), name_style);
            if let Some(size) = size {
                let size_room = usize::from(inner.width.saturating_sub(SIZE_X + 1));
                put(frame, inner.x + SIZE_X, y, &super::bar::clip(size, size_room), dim());
            }
            gui.ui.click(row, Act::Disc(DiscAct::Keep(KeepAct::Pick(*choice))));
        }
        Low::Missing(on) => {
            let (check_on, check_off) = super::check_glyphs();
            let label_x = value_col() + 4;
            let text_w = 4 + width(&t!("gui.discover.keep.only_missing")) as u16;
            let row = Rect { x: inner.x + label_x, y, width: text_w.min(inner.width.saturating_sub(label_x + 1)), height: 1 };
            let focused = focus == Some(Stop::Missing);
            let hover = gui.ui.hovers(row);
            put(frame, row.x, y, if *on { check_on } else { check_off }, if *on { Style::default().fg(th().ok) } else { dim() });
            let style = match (focused, hover) {
                (true, _) => accent().add_modifier(Modifier::BOLD),
                (false, true) => bright_bold(),
                (false, false) => Style::default(),
            };
            let words = super::bar::clip(&t!("gui.discover.keep.only_missing"), room.saturating_sub(usize::from(label_x + 4))).into_owned();
            put(frame, row.x + 4, y, &words, style);
            gui.ui.click(row, Act::Disc(DiscAct::Keep(KeepAct::Missing)));
        }
        Low::Folded(name, size) => {
            put(frame, x, y, &label("gui.discover.keep.what"), dim());
            let at = inner.x + value_col();
            let name_room = room.saturating_sub(usize::from(value_col() - 1));
            let shown = super::bar::clip(name, name_room).into_owned();
            put(frame, at, y, &shown, Style::default());
            if let Some(size) = size {
                let after = at + width(&shown) as u16;
                let left = usize::from(inner.right().saturating_sub(after + 1));
                put(frame, after, y, &super::bar::clip(&format!(" · {size}"), left), dim());
            }
        }
        Low::Path { first, runs, tip } => {
            if *first {
                put(frame, x, y, &label("gui.discover.keep.lands_in"), dim());
            }
            // The library BOLD, the names TEXT, `›` DIM; an artist copy's
            // "one folder per album" and a slot DIM — hints, never a
            // folder's name. A row that still overflows is clipped at the
            // right, the whole path in its tooltip; a slot's note stands
            // there too.
            let start = inner.x + value_col();
            let end = inner.right().saturating_sub(1);
            let mut at = start;
            for (text, style) in runs {
                let left = usize::from(end.saturating_sub(at));
                if left == 0 {
                    break;
                }
                let shown = super::bar::clip(text, left).into_owned();
                put(frame, at, y, &shown, *style);
                at += width(&shown) as u16;
            }
            if let Some(tip) = tip {
                gui.ui.tip(Rect { x: start, y, width: end.saturating_sub(start), height: 1 }, tip.clone());
            }
        }
        Low::Source(saved) => {
            let words = if *saved { t!("gui.discover.dest.source_user") } else { t!("gui.discover.dest.source_default") };
            let change = format!("[ {} ]", ell(&format!("{}…", t!("gui.discover.dest.change"))));
            let w = width(&change) as u16;
            let right = inner.right().saturating_sub(1);
            let rect = Rect { x: right.saturating_sub(w), y, width: w, height: 1 };
            let at = inner.x + value_col();
            put(frame, at, y, &super::bar::clip(&words, usize::from(rect.x.saturating_sub(at + 1))), dim());
            let style = if focus == Some(Stop::Change) {
                sel().add_modifier(Modifier::BOLD)
            } else if gui.ui.hovers(rect) {
                bright_bold()
            } else {
                dim()
            };
            put(frame, rect.x, y, &change, style);
            gui.ui.click(rect, Act::Disc(DiscAct::Keep(KeepAct::Change)));
            gui.ui.tip_keyed(rect, format!("{} — d", t!("gui.discover.dest.title")));
        }
        Low::Text(text, style) => {
            put(frame, x, y, &super::bar::clip(text, room), *style);
        }
        Low::Runs(runs) => {
            let mut at = x;
            let end = inner.right().saturating_sub(1);
            for (i, (text, style)) in runs.iter().enumerate() {
                let piece = if i == 0 { text.clone() } else { format!(" · {text}") };
                let left = usize::from(end.saturating_sub(at));
                if left == 0 {
                    break;
                }
                // The separator in the run's own token: the canvas's
                // "· 1 failed" in GOLD.
                let shown = super::bar::clip(&piece, left).into_owned();
                put(frame, at, y, &shown, *style);
                at += width(&shown) as u16;
            }
        }
        Low::Bar(progress) => draw_bar(frame, x, y, *progress),
        Low::Buttons(secondary, primary) => {
            draw_buttons(frame, gui, inner, y, *secondary, *primary, focus);
        }
    }
}

/// The scan widget's bar (the kit's Status, progress): ten cells, `▰` OK
/// filled and `▱` DIM, the percentage DIM beside them; all DIM with no
/// percentage while there is no estimate — never a fake 0%. The legacy
/// console says it in words.
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

/// The buttons, bottom-right (the kit's Modal): `[ label ]`, the primary
/// ACCENT BOLD, the others DIM; hover BRIGHT + BOLD; the keyboard's stop
/// the selection slab.
#[allow(clippy::too_many_arguments)]
fn draw_buttons(
    frame: &mut Frame,
    gui: &mut Gui,
    inner: Rect,
    y: u16,
    secondary: Option<Button>,
    primary: Option<Button>,
    focus: Option<Stop>,
) {
    let mut right = inner.right().saturating_sub(1);
    let mut place = |frame: &mut Frame, gui: &mut Gui, button: Button, stop: Stop, act: KeepAct, lead: bool| {
        let text = format!("[ {} ]", button.label());
        let w = width(&text) as u16;
        let x = right.saturating_sub(w);
        let rect = Rect { x, y, width: w, height: 1 };
        let hover = gui.ui.hovers(rect);
        let style = if focus == Some(stop) {
            sel().add_modifier(Modifier::BOLD)
        } else if hover {
            bright_bold()
        } else if lead {
            accent().add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        put(frame, x, y, &text, style);
        gui.ui.click(rect, Act::Disc(DiscAct::Keep(act)));
        right = x.saturating_sub(1);
    };
    // The primary is the rightmost; Close stands alone as the primary
    // where nothing else is offered.
    if let Some(button) = primary {
        place(frame, gui, button, Stop::Primary, KeepAct::Primary, true);
    }
    if let Some(button) = secondary {
        place(frame, gui, button, Stop::Secondary, KeepAct::Secondary, false);
    }
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// Open Keep a copy on a target (clauses 102–103): the App's flow, the
/// keyboard on the preselected radio.
pub(crate) fn open(gui: &mut Gui, target: crate::tui::app::keep::KeepTarget) {
    let effects = gui.app.keep_open(target);
    gui.pend(effects);
    gui.discover.keep.starting = false;
    gui.discover.keep.dest = None;
    gui.discover.keep.focus = gui.app.keep_view().map(|v| Stop::Choice(v.choice)).filter(|_| {
        gui.app.taste.keep.flow.as_ref().is_some_and(|f| matches!(f.phase, KeepPhase::Choose))
    });
}

/// Close the modal; a copy still moving says so on the status line, since
/// the modal that spoke for it is gone (clause 118).
pub(crate) fn close(gui: &mut Gui) {
    if let Some(view) = gui.app.keep_view()
        && let KeepStage::Job { job, .. } = &view.stage
        && job.live()
    {
        let note = busy_note(job, note_room(gui));
        set_note(gui, note);
    }
    gui.app.keep_close();
    gui.discover.keep.focus = None;
    gui.discover.keep.dest = None;
}

/// The modal's acts.
pub(crate) fn act(gui: &mut Gui, act: KeepAct) {
    let Some(view) = gui.app.keep_view() else { return };
    let low = lower(&view);
    let (secondary, primary) = buttons_of(&low);
    match act {
        KeepAct::Close => close(gui),
        KeepAct::Change => open_dest(gui),
        KeepAct::Dest(act) => dest_act(gui, act),
        KeepAct::Pick(choice) => {
            gui.app.keep_pick(choice);
            gui.discover.keep.focus = Some(Stop::Choice(choice));
        }
        KeepAct::Missing => {
            let on = view.missing.is_some_and(|(on, _)| on);
            gui.app.keep_set_missing(!on);
            gui.discover.keep.focus = Some(Stop::Missing);
        }
        KeepAct::Secondary => match secondary {
            Some(Button::Cancel | Button::Close) => close(gui),
            Some(Button::CancelCopy) => {
                let effects = gui.app.keep_cancel();
                gui.pend(effects);
            }
            Some(Button::AddToQueue) => {
                if let KeepStage::Job { job, .. } = &view.stage {
                    let effects = gui.app.keep_play_copy(job, false);
                    gui.pend(effects);
                    close(gui);
                }
            }
            _ => {}
        },
        KeepAct::Primary => match primary {
            Some(Button::Add(_)) => {
                let effects = gui.app.keep_start();
                gui.pend(effects);
                gui.discover.keep.focus = None;
                gui.discover.keep.starting = true;
            }
            Some(Button::Close) => close(gui),
            Some(Button::Again(again)) => {
                let unpaired = matches!(&view.stage, KeepStage::Job { job, .. } if job.unpaired());
                let effects = gui.app.keep_again();
                gui.pend(effects);
                match again {
                    // Played: the modal has said what it had to.
                    JobAction::Play => close(gui),
                    // The choice comes back, the keyboard on its radio.
                    JobAction::StartAgain => gui.discover.keep.focus = Some(Stop::Choice(view.choice)),
                    // A server no longer paired: the room is asked again
                    // and its rows carry the new id (clause 128) — this
                    // modal's subject is gone with them.
                    _ if unpaired => close(gui),
                    _ => {
                        gui.discover.keep.focus = None;
                        gui.discover.keep.starting = true;
                    }
                }
            }
            _ => {}
        },
    }
}

/// The modal's keys (clause 106): ↑ ↓ walk the stops, Space or Enter pick
/// a radio or press a button, Enter with nothing held presses the primary,
/// `x` cancels the copy, Esc closes — the copy keeps running.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) {
    // Where files land, while it is open over the modal, has every key.
    if gui.discover.keep.dest.is_some() {
        dest_key(gui, key);
        return;
    }
    let Some(view) = gui.app.keep_view() else { return };
    let low = lower(&view);
    let stops = stops_of(&low);
    let focus = gui.discover.keep.focus.filter(|f| stops.contains(f));
    let walk = |delta: i64| -> Option<Stop> {
        if stops.is_empty() {
            return None;
        }
        match focus.and_then(|f| stops.iter().position(|s| *s == f)) {
            // A stowed hand shows where it rests and moves nothing: the
            // chosen radio in the choice, else the primary.
            None => {
                let rest = stops
                    .iter()
                    .copied()
                    .find(|s| *s == Stop::Choice(view.choice))
                    .or_else(|| stops.iter().copied().find(|s| *s == Stop::Primary))
                    .unwrap_or(stops[0]);
                Some(rest)
            }
            Some(at) => Some(stops[(at as i64 + delta).clamp(0, stops.len() as i64 - 1) as usize]),
        }
    };
    match key.code {
        KeyCode::Esc => close(gui),
        KeyCode::Down if !stops.is_empty() => gui.discover.keep.focus = walk(1),
        KeyCode::Up if !stops.is_empty() => gui.discover.keep.focus = walk(-1),
        // ← → between the buttons only, once the hand is on them.
        KeyCode::Right if matches!(focus, Some(Stop::Secondary)) => gui.discover.keep.focus = walk(1),
        KeyCode::Left if matches!(focus, Some(Stop::Primary)) && stops.contains(&Stop::Secondary) => {
            gui.discover.keep.focus = walk(-1)
        }
        KeyCode::Char('x') => {
            let effects = gui.app.keep_cancel();
            gui.pend(effects);
        }
        // Where files land, from anywhere the choice stands (clause 142).
        KeyCode::Char('d') if stops.contains(&Stop::Change) => open_dest(gui),
        KeyCode::Char(' ') | KeyCode::Enter => match focus {
            Some(Stop::Choice(choice)) => act(gui, KeepAct::Pick(choice)),
            Some(Stop::Missing) => act(gui, KeepAct::Missing),
            Some(Stop::Change) => act(gui, KeepAct::Change),
            Some(Stop::Secondary) => act(gui, KeepAct::Secondary),
            Some(Stop::Primary) => act(gui, KeepAct::Primary),
            None if key.code == KeyCode::Enter && stops.contains(&Stop::Primary) => act(gui, KeepAct::Primary),
            None => {}
        },
        _ => {}
    }
}

/// The tips line's cells (the footer's 99 at 100 columns): a choice's line
/// that outgrows them gives up "d where files land" first (clause 119).
const TIPS_CELLS: usize = 99;

fn fit_tips(line: String, d: &str) -> String {
    if width(&line) <= TIPS_CELLS {
        return line;
    }
    line.replace(&format!(" · {d}"), "")
}

/// The tips line while the modal is up (clause 119): the keys that work in
/// its state, and nothing else.
pub(crate) fn tips(gui: &Gui) -> String {
    if let Some(ui) = &gui.discover.keep.dest {
        return dest_tips(gui, ui);
    }
    let Some(view) = gui.app.keep_view() else { return String::new() };
    let low = lower(&view);
    let (_, primary) = buttons_of(&low);
    let d = t!("gui.tips.discover_keep_d");
    match &view.stage {
        KeepStage::Choose { other: Some(_), .. } => fit_tips(t!("gui.tips.discover_keep_other").to_string(), &d),
        KeepStage::Choose { .. } | KeepStage::Refused { .. } => match primary {
            Some(button) => fit_tips(t!("gui.tips.discover_keep_choose", verb = button.label()).to_string(), &d),
            None => fit_tips(t!("gui.tips.discover_keep_other").to_string(), &d),
        },
        KeepStage::Nowhere => t!("gui.tips.discover_keep_yours").to_string(),
        KeepStage::Starting => t!("gui.tips.discover_keep_close").to_string(),
        KeepStage::Job { outcome, action, .. } => match (outcome, action) {
            (JobOutcome::Queued, _) => t!("gui.tips.discover_keep_queued").to_string(),
            (JobOutcome::Running { stopping: false, .. }, _) => t!("gui.tips.discover_keep_running").to_string(),
            (_, Some(JobAction::Play)) => t!("gui.tips.discover_keep_play").to_string(),
            (_, Some(JobAction::CopyAgain)) => t!("gui.tips.discover_keep_again").to_string(),
            (_, Some(JobAction::Retry)) => t!("gui.tips.discover_keep_retry").to_string(),
            (_, Some(JobAction::StartAgain)) => t!("gui.tips.discover_keep_start_again").to_string(),
            (JobOutcome::Owned { .. }, _) => t!("gui.tips.discover_keep_yours").to_string(),
            _ => t!("gui.tips.discover_keep_close").to_string(),
        },
    }
}

// ── Where files land (clauses 142–148) ──────────────────────────────────────

/// The picker's width (at most the area less 4, the kit's) and the height
/// it anchors as: its tallest state — the folder list and a three-row
/// refusal never stand together, nor the variables row and the list.
const DEST_W: u16 = 76;
const DEST_ANCHOR: u16 = 22;
/// Its columns from the left border: the labels, the values, Custom's
/// field and the variables row, the last content cell (the TermDestination
/// board's).
const DEST_LABEL: u16 = 2;
const DEST_VALUE: u16 = 12;
const DEST_FIELD: u16 = 24;
const DEST_LAST: u16 = 73;
/// The folders the list shows at once; more scroll.
const DEST_LIST: usize = 3;
/// A refusal's GOLD rows at most, wrapped at this many cells.
const DEST_ERROR_ROWS: usize = 3;
const DEST_WRAP: usize = 72;

/// The values' column: the board's, or two cells past the widest label
/// (a locale's "BIBLIOTHÈQUE").
fn dest_value_col() -> u16 {
    let widest = ["gui.discover.dest.library", "gui.discover.dest.folder", "gui.discover.dest.layout", "gui.discover.dest.preview"]
        .iter()
        .map(|key| width(&t!(*key).to_uppercase()) as u16)
        .max()
        .unwrap_or(0);
    DEST_VALUE.max(DEST_LABEL + widest + 2)
}

/// Custom's field: the board's column, or two cells past Custom's label.
fn dest_field_col() -> u16 {
    DEST_FIELD.max(dest_value_col() + 4 + width(&t!("gui.discover.dest.custom")) as u16 + 2)
}

/// The layout presets and the layout each writes (clause 143): the album
/// artist ahead of the album, so a compilation stays in one folder; the
/// last the connected server's own folders, offered only where the view
/// lists `SERVERFOLDER` ([`presets`]).
const PRESETS: [(Radio, &str, &str); 4] = [
    (Radio::AlbumArtist, "gui.discover.dest.preset_albumartist", "{{ALBUMARTIST}}/{{ALBUM}}"),
    (Radio::Artist, "gui.discover.dest.preset_artist", "{{ARTIST}}/{{ALBUM}}"),
    (Radio::Server, "gui.discover.dest.preset_server", "{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}"),
    (Radio::ServerFolder, "gui.discover.dest.preset_serverfolder", "{{SERVERFOLDER}}"),
];

/// The presets this server takes: "Same folders as the server" only where
/// the view lists its variable — a server from before it refuses it at
/// Save, so it is never offered there.
fn presets(view: &CollectionDestinationView) -> impl Iterator<Item = &'static (Radio, &'static str, &'static str)> + '_ {
    PRESETS.iter().filter(|(radio, _, _)| *radio != Radio::ServerFolder || view.knows(crate::api::types::SERVER_FOLDER))
}

/// A LAYOUT radio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Radio {
    AlbumArtist,
    Artist,
    Server,
    /// "Same folders as the server" (`{{SERVERFOLDER}}`).
    ServerFolder,
    /// The chosen library's own template, where it is none of the presets.
    Template,
    Custom,
}

/// One keyboard stop of the picker, in walking order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestStop {
    /// The library stepper — a stop with more than one library.
    Library,
    Folder,
    Radio(Radio),
    /// Custom's field, a stop while Custom is chosen.
    Layout,
    UseDefault,
    Cancel,
    Save,
}

/// What the picker's pointer and keys do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestAct {
    /// `[X]`, Cancel, Esc: back to Keep a copy, nothing saved.
    Back,
    /// ◂ ▸ and the library's name: the library before, the one after.
    Step(i8),
    /// A stop clicked: the hand goes there, a field takes the keys.
    Hand(DestStop),
    /// A radio clicked.
    Pick(Radio),
    /// A folder of the list clicked, by its place in the list: in it.
    Folder(usize),
    /// A variable of the row clicked, by its place: inserted at the caret.
    Var(usize),
    UseDefault,
    Save,
    /// The folder list's scrollbar.
    Scroll(i32),
    Jump(usize),
}

/// Where files land's own state: the fields as typed, the keyboard's hand,
/// the folder list, a refusal (clauses 142–144). What it draws from — the
/// destination's view — and its asks are the App's.
#[derive(Clone)]
pub(crate) struct DestUi {
    /// The library chosen, by its name: the view may be read again under it.
    vpath: String,
    folder: Input,
    layout: Input,
    /// Custom was picked: the layout reads Custom whatever it matches.
    custom: bool,
    /// The keyboard's stop; `None` while its save is out.
    hand: Option<DestStop>,
    /// The save this picker sent, by its seq, while it is out: "saving…",
    /// and its answer is this picker's to fold in (another picker's — one
    /// left with Esc while saving — is the status line's).
    sent: Option<u64>,
    /// Where the hand was when the save went out: back there on a refusal.
    before: Option<DestStop>,
    /// The folder list under FOLDER, while open.
    list: Option<FolderList>,
    /// A refused save, its words GOLD until the next edit.
    error: Option<crate::api::PluginFailure>,
    /// Use the library default pressed while the picker is open: Save sends
    /// `null` — back to the default, nothing kept for the account — where
    /// the fields are the default then, whatever was stepped or picked in
    /// between (the webapp's `reset`).
    defaulted: bool,
}

/// The folder list: its held folder (the keyboard's) and its viewport.
#[derive(Clone, Default)]
struct FolderList {
    held: Option<usize>,
    view: ListView,
}

impl DestUi {
    /// Opened on the place in force: the hand on its first stop.
    fn of(place: &CollectionDestination, view: &CollectionDestinationView) -> DestUi {
        DestUi {
            vpath: place.vpath.clone(),
            folder: Input::new(place.base.clone()),
            layout: Input::new(place.layout.clone()),
            custom: false,
            hand: Some(DestUi::first_stop(view)),
            sent: None,
            before: None,
            list: None,
            error: None,
            defaulted: false,
        }
    }

    /// The first stop: LIBRARY — FOLDER with one library, where LIBRARY is
    /// none.
    fn first_stop(view: &CollectionDestinationView) -> DestStop {
        if view.libraries.len() > 1 { DestStop::Library } else { DestStop::Folder }
    }

    /// The place as typed: the folder trimmed, the layout as it stands —
    /// no layout is checked here: the server's words decide.
    fn place(&self) -> CollectionDestination {
        CollectionDestination {
            vpath: self.vpath.clone(),
            base: self.folder.value().trim().to_string(),
            layout: self.layout.value().to_string(),
            source: None,
        }
    }

    /// The chosen library's own template, where it is none of the presets.
    fn template(&self, view: &CollectionDestinationView) -> Option<String> {
        let library = view.libraries.iter().find(|l| l.vpath == self.vpath)?;
        library.template.clone().filter(|t| !t.trim().is_empty() && !presets(view).any(|(_, _, p)| p == t))
    }

    fn radios(&self, view: &CollectionDestinationView) -> Vec<Radio> {
        let mut out: Vec<Radio> = presets(view).map(|(radio, _, _)| *radio).collect();
        if self.template(view).is_some() {
            out.push(Radio::Template);
        }
        out.push(Radio::Custom);
        out
    }

    /// The chosen radio: the preset (or the template) the layout is,
    /// character for character, else Custom — and Custom once picked.
    fn chosen(&self, view: &CollectionDestinationView) -> Radio {
        let layout = self.layout.value();
        if self.custom {
            return Radio::Custom;
        }
        if let Some((radio, _, _)) = presets(view).find(|(_, _, p)| *p == layout) {
            return *radio;
        }
        if self.template(view).is_some_and(|t| t == layout) {
            return Radio::Template;
        }
        Radio::Custom
    }

    /// The fields differ from the library default: "Use the library
    /// default" stands.
    fn off_default(&self, view: &CollectionDestinationView) -> bool {
        view.library_default().is_some_and(|d| !d.same_place(&self.place()))
    }

    /// The stops in walking order: LIBRARY (with a choice of libraries),
    /// FOLDER, each radio, Custom's field while Custom is chosen, the
    /// buttons.
    fn stops(&self, view: &CollectionDestinationView) -> Vec<DestStop> {
        let mut out = Vec::new();
        if view.libraries.len() > 1 {
            out.push(DestStop::Library);
        }
        out.push(DestStop::Folder);
        out.extend(self.radios(view).into_iter().map(DestStop::Radio));
        if self.chosen(view) == Radio::Custom {
            out.push(DestStop::Layout);
        }
        if self.off_default(view) {
            out.push(DestStop::UseDefault);
        }
        out.extend([DestStop::Cancel, DestStop::Save]);
        out
    }

    /// The buttons standing, left to right.
    fn buttons(&self, view: &CollectionDestinationView) -> Vec<DestStop> {
        self.stops(view).into_iter().filter(|s| matches!(s, DestStop::UseDefault | DestStop::Cancel | DestStop::Save)).collect()
    }

    /// The folder typed, split where completion works: the folder whose
    /// folders complete it, and the name being typed.
    fn completing(&self) -> (Vec<String>, String) {
        let text = self.folder.value();
        match text.rfind(['/', '\\']) {
            Some(at) => (base_folders(&text[..at]), text[at + 1..].to_string()),
            None => (Vec::new(), text.to_string()),
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

/// The list's folders: the completing folder's whose names begin with what
/// is typed (case folded) — `None` while they are asked or would not list.
fn suggestions(app: &crate::tui::app::App, ui: &DestUi) -> Option<Vec<String>> {
    let (dir, tail) = ui.completing();
    match app.keep_dest_folders(&folder_path(&ui.vpath, &dir))? {
        Folders::Listed(names) => {
            Some(names.iter().filter(|n| crate::setup::starts_with_fold(n, &tail)).cloned().collect())
        }
        _ => None,
    }
}

/// The folder typed, walked against its parents' listings: whether one of
/// its names is not there yet, and the first listing the walk still needs.
fn walk_folders(app: &crate::tui::app::App, ui: &DestUi) -> (bool, Option<String>) {
    let names = base_folders(ui.folder.value());
    for i in 0..names.len() {
        let path = folder_path(&ui.vpath, &names[..i]);
        match app.keep_dest_folders(&path) {
            None => return (false, Some(path)),
            Some(Folders::Listed(listed)) if !listed.contains(&names[i]) => return (true, None),
            Some(Folders::Listed(_)) => {}
            Some(_) => return (false, None),
        }
    }
    (false, None)
}

/// Ask what the picker draws next: the open list's folder, else the first
/// listing the typed folder's walk needs — one at a time, each once.
fn dest_ask(gui: &mut Gui) {
    let Some(ui) = gui.discover.keep.dest.as_ref() else { return };
    let path = match ui.list {
        Some(_) => Some(folder_path(&ui.vpath, &ui.completing().0)).filter(|p| gui.app.keep_dest_folders(p).is_none()),
        None => None,
    };
    let Some(path) = path.or_else(|| walk_folders(&gui.app, ui).1) else { return };
    let effects = gui.app.keep_dest_list(&path);
    gui.pend(effects);
}

/// The base folder's last name, where the layout's first folder renders
/// the same (case folded): files would land in X › X.
fn nested(ui: &DestUi, landing: &Landing) -> Option<String> {
    let base = base_folders(ui.folder.value());
    let last = base.last()?;
    let first: String = landing.folders.get(base.len())?.iter().map(|(text, _)| text.as_str()).collect();
    (first.to_lowercase() == last.to_lowercase()).then(|| last.clone())
}

/// The HINT row under FOLDER (clause 143): the open list's folder and how
/// many it holds; a base whose last folder the layout repeats (GOLD); a
/// folder not there yet; the completing folder's failed listing (GOLD);
/// else where it is.
fn folder_hint(app: &crate::tui::app::App, ui: &DestUi, landing: &Landing) -> (String, Style) {
    let gold = Style::default().fg(th().gold);
    let (dir, _) = ui.completing();
    if ui.list.is_some()
        && let Some(names) = suggestions(app, ui)
    {
        let key = format!("gui.discover.dest.listing_{}", super::discover::plural_form(names.len() as u64));
        let words = t!(key.as_str(), path = folder_words(&ui.vpath, &dir), count = names.len());
        return (words.to_string(), dim());
    }
    if let Some(name) = nested(ui, landing) {
        let path = format!("{name}{}{name}", crumb_sep());
        return (t!("gui.discover.dest.nested", path = path).to_string(), gold);
    }
    if walk_folders(app, ui).0 {
        return (t!("gui.discover.dest.not_there").to_string(), dim());
    }
    if let Some(Folders::Failed(failure)) = app.keep_dest_folders(&folder_path(&ui.vpath, &dir)) {
        let words = t!("gui.discover.dest.list_failed", path = folder_words(&ui.vpath, &dir), words = failure_words(failure));
        return (words.to_string(), gold);
    }
    (t!("gui.discover.dest.base_hint", library = ui.vpath).to_string(), dim())
}

/// A refusal's GOLD rows: the server's words after "couldn't save: ",
/// wrapped, three rows at most — the third clipped with `…`.
fn error_lines(failure: &crate::api::PluginFailure) -> Vec<String> {
    let words = t!("gui.discover.dest.save_failed", words = failure_words(failure)).to_string();
    let mut lines = crate::kit::wrap_words(&words, DEST_WRAP);
    if lines.len() > DEST_ERROR_ROWS {
        let rest = lines.split_off(DEST_ERROR_ROWS - 1).join(" ");
        lines.push(super::bar::clip(&rest, DEST_WRAP).into_owned());
    }
    lines
}

/// One row of the picker, top to bottom.
#[derive(Debug, Clone, PartialEq)]
enum DestRow {
    Title,
    Blank,
    Library,
    Folder,
    /// A row of the open folder list, by its place in the list's window.
    Listed(usize),
    Hint,
    Radio(Radio, bool),
    /// The variables, under Custom's field while it has the hand.
    Vars,
    Preview,
    File,
    Applies,
    /// A refusal's row.
    Error(String),
    Buttons,
}

/// The picker's rows (clause 142): 18, a row more for "Same folders as the
/// server", a library's template, the variables or each listed folder
/// (three at most), two more for a long refusal — and where that would
/// pass the 22 the 100 × 24 floor holds, the blank above PREVIEW gives way.
fn dest_rows(app: &crate::tui::app::App, ui: &DestUi, view: &CollectionDestinationView) -> Vec<DestRow> {
    let mut rows = vec![DestRow::Title, DestRow::Blank, DestRow::Library, DestRow::Folder];
    if ui.list.is_some() {
        let n = suggestions(app, ui).map_or(0, |names| names.len()).min(DEST_LIST);
        rows.extend((0..n).map(DestRow::Listed));
    }
    rows.extend([DestRow::Hint, DestRow::Blank]);
    rows.extend(ui.radios(view).into_iter().enumerate().map(|(i, r)| DestRow::Radio(r, i == 0)));
    if ui.hand == Some(DestStop::Layout) {
        rows.push(DestRow::Vars);
    }
    rows.extend([DestRow::Blank, DestRow::Preview, DestRow::File, DestRow::Blank]);
    match &ui.error {
        Some(failure) => rows.extend(error_lines(failure).into_iter().map(DestRow::Error)),
        None => rows.push(DestRow::Applies),
    }
    rows.push(DestRow::Buttons);
    if rows.len() as u16 + 2 > DEST_ANCHOR
        && let Some(at) = rows.iter().position(|row| *row == DestRow::Preview)
    {
        rows.remove(at - 1);
    }
    rows
}

/// The variables a layout may name, as the row offers them.
fn variables(view: &CollectionDestinationView) -> Vec<String> {
    if view.variables.is_empty() {
        crate::api::types::LAYOUT_VARS.iter().map(|v| v.to_string()).collect()
    } else {
        view.variables.clone()
    }
}

/// A radio's words.
fn radio_label(radio: Radio, vpath: &str) -> String {
    match radio {
        Radio::Template => t!("gui.discover.dest.preset_library", library = vpath).to_string(),
        Radio::Custom => t!("gui.discover.dest.custom").to_string(),
        preset => PRESETS.iter().find(|(r, _, _)| *r == preset).map(|(_, key, _)| t!(*key).to_string()).unwrap_or_default(),
    }
}

/// A button's words.
fn dest_button(stop: DestStop) -> String {
    match stop {
        DestStop::UseDefault => t!("gui.discover.dest.use_default").to_string(),
        DestStop::Cancel => t!("gui.discover.keep.cancel").to_string(),
        _ => t!("gui.discover.dest.save").to_string(),
    }
}

/// Runs of words in their tokens at `x`, in `room` cells: whole where they
/// fit, else their tail behind a leading `…` (the picker's PREVIEW, so the
/// folders that differ stay in view). True when it was clipped.
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
        for grapheme in text.graphemes(true) {
            let w = crate::kit::grapheme_cells(grapheme);
            // A wide one the cut lands inside goes whole: a cell short,
            // never over.
            if skip > 0 {
                skip = skip.saturating_sub(w);
                continue;
            }
            shown.push_str(grapheme);
        }
        put(frame, at, y, &shown, *style);
        at += width(&shown) as u16;
    }
    total > room
}

/// Open Where files land (clause 142) on the place in force: `[ Change… ]`,
/// `d`, from the choice. Keep a copy gives way until it closes.
pub(crate) fn open_dest(gui: &mut Gui) {
    let Some(view) = gui.app.keep_dest_view() else { return };
    let Some(place) = view.destination.as_ref() else { return };
    gui.discover.keep.dest = Some(DestUi::of(place, view));
    dest_ask(gui);
}

/// The open picker's own save is out: "saving…", no buttons, only Esc.
fn dest_saving(gui: &Gui) -> bool {
    let out = gui.app.keep_dest_saving();
    gui.discover.keep.dest.as_ref().is_some_and(|ui| ui.sent.is_some() && ui.sent == out)
}

/// Back to Keep a copy — Esc, `[X]`, Cancel, or a save that landed — the
/// hand on `[ Change… ]`.
fn close_dest(gui: &mut Gui) {
    gui.discover.keep.dest = None;
    gui.app.keep_dest_forget();
    gui.discover.keep.focus = Some(Stop::Change);
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

/// The picker's Save: the place as typed — or `null` where Use the library
/// default was pressed and the fields are the default.
fn dest_save(gui: &mut Gui) {
    let Some(view) = gui.app.keep_dest_view().cloned() else { return };
    let Some(ui) = gui.discover.keep.dest.as_mut() else { return };
    let place = ui.place();
    let back = ui.defaulted && view.library_default().is_some_and(|d| d.same_place(&place));
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
    let saving = dest_saving(gui);
    if act == DestAct::Back {
        close_dest(gui);
        return;
    }
    if saving {
        return;
    }
    let names = gui.discover.keep.dest.as_ref().and_then(|ui| suggestions(&gui.app, ui));
    let Some(ui) = gui.discover.keep.dest.as_mut() else { return };
    match act {
        DestAct::Back => {}
        DestAct::Step(delta) => {
            let n = view.libraries.len();
            if n > 1 {
                let at = view.libraries.iter().position(|l| l.vpath == ui.vpath).unwrap_or(0);
                let next = (at as i64 + i64::from(delta)).rem_euclid(n as i64) as usize;
                ui.vpath = view.libraries[next].vpath.clone();
                ui.list = None;
                ui.edited();
            }
            ui.hand = Some(DestStop::Library);
        }
        DestAct::Hand(stop) => {
            if stop != DestStop::Folder {
                ui.list = None;
            }
            ui.hand = Some(stop);
        }
        DestAct::Pick(radio) => {
            match radio {
                Radio::Custom => {
                    ui.custom = true;
                    ui.hand = Some(DestStop::Layout);
                }
                Radio::Template => {
                    if let Some(template) = ui.template(&view) {
                        ui.layout = Input::new(template);
                    }
                    ui.custom = false;
                    ui.hand = Some(DestStop::Radio(radio));
                }
                preset => {
                    if let Some((_, _, layout)) = presets(&view).find(|(r, _, _)| *r == preset) {
                        ui.layout = Input::new(layout.to_string());
                    }
                    ui.custom = false;
                    ui.hand = Some(DestStop::Radio(radio));
                }
            }
            ui.list = None;
            ui.edited();
        }
        DestAct::Folder(at) => {
            let Some(name) = names.and_then(|n| n.get(at).cloned()) else { return };
            let (mut dir, _) = ui.completing();
            dir.push(name);
            ui.folder = Input::new(format!("{}/", dir.join("/")));
            ui.list = Some(FolderList::default());
            ui.hand = Some(DestStop::Folder);
            ui.edited();
        }
        DestAct::Var(at) => {
            let Some(name) = variables(&view).get(at).cloned() else { return };
            let token = format!("{{{{{name}}}}}");
            let cursor = ui.layout.cursor();
            let mut value: Vec<char> = ui.layout.value().chars().collect();
            let at = cursor.min(value.len());
            value.splice(at..at, token.chars());
            ui.layout = Input::new(value.into_iter().collect()).with_cursor(at + token.chars().count());
            ui.custom = true;
            ui.hand = Some(DestStop::Layout);
            ui.edited();
        }
        DestAct::UseDefault => {
            if let Some(default) = view.library_default() {
                ui.vpath = default.vpath;
                ui.folder = Input::new(String::new());
                ui.layout = Input::new(default.layout);
                ui.custom = false;
                ui.list = None;
                ui.edited();
                ui.defaulted = true;
                ui.hand = Some(DestUi::first_stop(&view));
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

/// The wheel over the picker: its folder list, a folder a notch.
pub(crate) fn dest_wheel(gui: &mut Gui, delta: i32) {
    if let Some(list) = gui.discover.keep.dest.as_mut().and_then(|ui| ui.list.as_mut()) {
        list.view.step(delta);
    }
}

/// Tab and Shift-Tab in FOLDER (clause 143): the list opens on the
/// completing folder; the names it holds extend what is typed to their
/// common beginning, and once that gains nothing the held folder cycles.
fn dest_complete(gui: &mut Gui, forward: bool) {
    let names = gui.discover.keep.dest.as_ref().and_then(|ui| suggestions(&gui.app, ui));
    let Some(ui) = gui.discover.keep.dest.as_mut() else { return };
    ui.error = None;
    let (dir, tail) = ui.completing();
    let list = ui.list.get_or_insert_with(FolderList::default);
    let Some(names) = names.filter(|n| !n.is_empty()) else {
        dest_ask(gui);
        return;
    };
    let common = crate::setup::common_prefix(&names);
    if common.chars().count() > tail.chars().count() {
        list.held = None;
        let mut parts = dir;
        parts.push(common);
        ui.folder = Input::new(parts.join("/"));
    } else {
        let n = names.len();
        list.held = Some(match (list.held, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(at), true) => (at + 1) % n,
            (Some(at), false) => (at + n - 1) % n,
        });
        list.view.pick_up();
    }
    dest_ask(gui);
}

/// The picker's keys (clause 143): ↑ ↓ walk the stops — and the folder
/// list while it is open; Tab completes a folder; ← → step the library and
/// move between the buttons; Space or Enter pick a radio; in a field every
/// printable key edits; Enter saves from LIBRARY and the fields; Esc closes
/// the list, else goes back.
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
    let names = gui.discover.keep.dest.as_ref().and_then(|ui| suggestions(&gui.app, ui)).unwrap_or_default();
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
    let shift_tab = key.code == KeyCode::BackTab || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT));
    match (ui.hand, key.code) {
        (_, KeyCode::Esc) if ui.list.is_some() => ui.list = None,
        (_, KeyCode::Esc) => dest_act(gui, DestAct::Back),
        (Some(DestStop::Folder), KeyCode::Tab | KeyCode::BackTab) => dest_complete(gui, !shift_tab),
        // The open list: ↓ into it and down it, ↑ up it and back to the
        // field from its top; Enter on a held folder goes in.
        (Some(DestStop::Folder), KeyCode::Down) if ui.list.is_some() && !names.is_empty() => {
            if let Some(list) = ui.list.as_mut() {
                list.held = Some(held.map_or(0, |at| (at + 1).min(names.len() - 1)));
                list.view.pick_up();
            }
        }
        (Some(DestStop::Folder), KeyCode::Up) if held.is_some() => {
            if let Some(list) = ui.list.as_mut() {
                list.held = held.and_then(|at| at.checked_sub(1));
                list.view.pick_up();
            }
        }
        (Some(DestStop::Folder), KeyCode::Enter) if held.is_some() => dest_act(gui, DestAct::Folder(held.unwrap_or(0))),
        (hand, KeyCode::Up | KeyCode::Down) => {
            let next = walk(hand, if key.code == KeyCode::Down { 1 } else { -1 });
            if next != Some(DestStop::Folder) {
                ui.list = None;
            }
            ui.hand = next;
        }
        (Some(DestStop::Library), KeyCode::Left) => dest_act(gui, DestAct::Step(-1)),
        (Some(DestStop::Library), KeyCode::Right) => dest_act(gui, DestAct::Step(1)),
        (Some(DestStop::Library | DestStop::Folder | DestStop::Layout), KeyCode::Enter) => dest_save(gui),
        (Some(DestStop::Radio(radio)), KeyCode::Enter | KeyCode::Char(' ')) => dest_act(gui, DestAct::Pick(radio)),
        (Some(stop @ (DestStop::Folder | DestStop::Layout)), _) => {
            let input = if stop == DestStop::Folder { &mut ui.folder } else { &mut ui.layout };
            if input.handle_event(&TermEvent::Key(key)).is_some_and(|change| change.value) {
                ui.edited();
                if let Some(list) = ui.list.as_mut() {
                    list.held = None;
                    list.view = ListView::default();
                }
                dest_ask(gui);
            }
        }
        (Some(button @ (DestStop::UseDefault | DestStop::Cancel | DestStop::Save)), KeyCode::Left | KeyCode::Right) => {
            let at = buttons.iter().position(|b| *b == button).unwrap_or(0) as i64;
            let delta = if key.code == KeyCode::Right { 1 } else { -1 };
            ui.hand = buttons.get((at + delta).clamp(0, buttons.len() as i64 - 1) as usize).copied();
        }
        // A button presses on Space or Enter, as Keep a copy's do.
        (Some(DestStop::UseDefault), KeyCode::Enter | KeyCode::Char(' ')) => dest_act(gui, DestAct::UseDefault),
        (Some(DestStop::Cancel), KeyCode::Enter | KeyCode::Char(' ')) => dest_act(gui, DestAct::Back),
        (Some(DestStop::Save), KeyCode::Enter | KeyCode::Char(' ')) => dest_save(gui),
        _ => {}
    }
}

/// The tips line while the picker is up (clause 143), by the hand.
fn dest_tips(gui: &Gui, ui: &DestUi) -> String {
    if dest_saving(gui) {
        return t!("gui.tips.discover_dest_saving").to_string();
    }
    let verb = |stop: DestStop| match stop {
        DestStop::UseDefault => t!("gui.discover.dest.verb_default"),
        DestStop::Cancel => t!("gui.discover.dest.verb_cancel"),
        _ => t!("gui.discover.dest.verb_save"),
    };
    match ui.hand {
        Some(DestStop::Folder) if ui.list.is_some() => t!("gui.tips.discover_dest_list").to_string(),
        Some(DestStop::Folder) => t!("gui.tips.discover_dest_folder").to_string(),
        Some(DestStop::Radio(_)) => t!("gui.tips.discover_dest_radio").to_string(),
        Some(DestStop::Layout) => t!("gui.tips.discover_dest_custom").to_string(),
        Some(stop @ (DestStop::UseDefault | DestStop::Cancel | DestStop::Save)) => {
            t!("gui.tips.discover_dest_button", verb = verb(stop)).to_string()
        }
        Some(DestStop::Library) => t!("gui.tips.discover_dest_library").to_string(),
        // No hand (it never rests there for long): only Esc is sure.
        None => t!("gui.tips.discover_dest_saving").to_string(),
    }
}

/// Where files land, drawn (and registered) over everything — Keep a copy
/// is not drawn while it is open (clause 142; the TermDestination board):
/// the kit's modal, ACCENT, 76 wide, centred over the whole frame and
/// anchored as if 22 rows, so its top holds as the list, the variables and
/// a refusal come and go. The room beneath is inert.
fn draw_dest(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let (Some(view), Some(dview)) = (gui.app.keep_view(), gui.app.keep_dest_view().cloned()) else { return };
    dest_ask(gui);
    let saving = dest_saving(gui);
    let names = gui.discover.keep.dest.as_ref().and_then(|ui| suggestions(&gui.app, ui)).unwrap_or_default();
    let Some(ui) = gui.discover.keep.dest.as_mut() else { return };
    // A hand on a stop that left (a button gone), or on none once its save
    // came back, goes to the first stop — never down where no key reaches.
    if !saving && ui.hand.is_none_or(|h| !ui.stops(&dview).contains(&h)) {
        ui.hand = Some(DestUi::first_stop(&dview));
    }
    let window = ui.list.as_mut().map(|list| list.view.window(names.len(), list.held, DEST_LIST));
    let ui = ui.clone();
    let rows = dest_rows(&gui.app, &ui, &dview);
    gui.ui.click(area, Act::Guard);
    let h = rows.len() as u16 + 2;
    let rect = crate::kit::modal_rect(area, DEST_W, h, DEST_ANCHOR.max(h));
    gui.ui.overlay(rect);
    gui.ui.modal_over();
    let inner = modal_box(frame, rect);
    gui.ui.click(rect, Act::Guard);
    let dest = |act: DestAct| Act::Disc(DiscAct::Keep(KeepAct::Dest(act)));
    let label_x = rect.x + DEST_LABEL;
    let value_x = rect.x + dest_value_col();
    let field_x = rect.x + dest_field_col();
    let end = rect.x + DEST_LAST + 1;
    let room = usize::from(end.saturating_sub(value_x));
    let label = |key: &str, hand: bool| {
        let style = if hand { accent().add_modifier(Modifier::BOLD) } else { dim() };
        (t!(key).to_uppercase(), style)
    };
    let known = |name: &str| dview.knows(name);
    let landing = view.landing_for(&ui.vpath, ui.folder.value(), ui.layout.value(), &known);
    let chosen = ui.chosen(&dview);
    for (i, row) in rows.iter().enumerate() {
        let y = inner.y + i as u16;
        match row {
            DestRow::Title | DestRow::Blank => {
                if *row == DestRow::Title {
                    put(frame, label_x, y, &t!("gui.discover.dest.title"), accent().add_modifier(Modifier::BOLD));
                    let x = crate::kit::modal_close_plain(frame, &mut gui.ui, inner, dest(DestAct::Back));
                    gui.ui.tip_keyed(x, format!("{} — Esc", t!("gui.discover.dest.back")));
                }
            }
            DestRow::Library => {
                let (words, style) = label("gui.discover.dest.library", ui.hand == Some(DestStop::Library));
                put(frame, label_x, y, &words, style);
                // The label takes the hand where LIBRARY is a stop.
                if dview.libraries.len() > 1 {
                    gui.ui.click(Rect { x: label_x, y, width: value_x - label_x, height: 1 }, dest(DestAct::Hand(DestStop::Library)));
                }
                let name = super::bar::clip(&ui.vpath, room.saturating_sub(40)).into_owned();
                let name_w = width(&name) as u16;
                if dview.libraries.len() > 1 {
                    // ◂ name ▸: the arrows step, the name steps on.
                    let (l, r) = if legacy_conhost() { ("<", ">") } else { ("◂", "▸") };
                    let lrect = Rect { x: value_x, y, width: 1, height: 1 };
                    let nrect = Rect { x: value_x + 2, y, width: name_w, height: 1 };
                    let rrect = Rect { x: nrect.right() + 1, y, width: 1, height: 1 };
                    let bold = Style::default().add_modifier(Modifier::BOLD);
                    put(frame, lrect.x, y, l, if gui.ui.hovers(lrect) { bright_bold() } else { dim() });
                    put(frame, nrect.x, y, &name, if gui.ui.hovers(nrect) { bright_bold() } else { bold });
                    put(frame, rrect.x, y, r, if gui.ui.hovers(rrect) { bright_bold() } else { dim() });
                    gui.ui.click(lrect, dest(DestAct::Step(-1)));
                    gui.ui.click(nrect, dest(DestAct::Step(1)));
                    gui.ui.click(rrect, dest(DestAct::Step(1)));
                    let hint_x = rrect.right() + 3;
                    put(frame, hint_x, y, &super::bar::clip(&t!("gui.discover.dest.lib_hint"), usize::from(end.saturating_sub(hint_x))), dim());
                } else {
                    put(frame, value_x, y, &name, Style::default().add_modifier(Modifier::BOLD));
                    let hint_x = value_x + name_w + 3;
                    put(frame, hint_x, y, &super::bar::clip(&t!("gui.discover.dest.only_library"), usize::from(end.saturating_sub(hint_x))), dim());
                }
            }
            DestRow::Folder => {
                let hand = ui.hand == Some(DestStop::Folder);
                let (words, style) = label("gui.discover.dest.folder", hand);
                put(frame, label_x, y, &words, style);
                let w = room as u16;
                if hand {
                    super::text_field(frame, &mut gui.ui, value_x, y, ui.folder.value(), ui.folder.cursor(), w, Style::default());
                } else if ui.folder.value().is_empty() {
                    put(frame, value_x, y, &t!("gui.discover.dest.root"), dim());
                } else {
                    put(frame, value_x, y, &super::clip_lead(ui.folder.value(), room), Style::default());
                }
                gui.ui.click(Rect { x: label_x, y, width: end - label_x, height: 1 }, dest(DestAct::Hand(DestStop::Folder)));
            }
            DestRow::Listed(n) => {
                let (first, visible) = window.unwrap_or((0, 0));
                let at = first + n;
                let Some(name) = names.get(at) else { continue };
                let slab = Rect { x: value_x - 1, y, width: end - (value_x - 1), height: 1 };
                let held = ui.list.as_ref().and_then(|l| l.held) == Some(at);
                if held {
                    frame.buffer_mut().set_style(slab, sel());
                }
                let style = if held {
                    sel()
                } else if gui.ui.hovers(slab) {
                    Style::default().fg(th().bright)
                } else {
                    dim()
                };
                put(frame, value_x, y, &super::bar::clip(name, room), style);
                gui.ui.click(slab, dest(DestAct::Folder(at)));
                if *n == 0 && names.len() > visible {
                    let bar = Rect { x: end, y, width: 1, height: visible as u16 };
                    scroll_list(frame, &mut gui.ui, bar, names.len(), visible, first, dest(DestAct::Scroll(-1)), dest(DestAct::Scroll(1)), move |n| {
                        Act::Disc(DiscAct::Keep(KeepAct::Dest(DestAct::Jump(n))))
                    });
                }
            }
            DestRow::Hint => {
                let (words, style) = folder_hint(&gui.app, &ui, &landing);
                put(frame, value_x, y, &super::bar::clip(&words, room), style);
            }
            DestRow::Radio(radio, first) => {
                if *first {
                    put(frame, label_x, y, &t!("gui.discover.dest.layout").to_uppercase(), dim());
                }
                let on = chosen == *radio;
                let (dot_on, dot_off) = if legacy_conhost() { ("(*)", "( )") } else { ("(•)", "( )") };
                put(frame, value_x, y, if on { dot_on } else { dot_off }, if on { Style::default().fg(th().ok) } else { dim() });
                let words = radio_label(*radio, &ui.vpath);
                let row_end = if *radio == Radio::Custom { field_x - 1 } else { end };
                let pick = Rect { x: value_x, y, width: row_end - value_x, height: 1 };
                let hover = gui.ui.hovers(pick);
                let style = match (ui.hand == Some(DestStop::Radio(*radio)), hover, on) {
                    (true, _, _) => accent().add_modifier(Modifier::BOLD),
                    (false, true, _) => bright_bold(),
                    (false, false, true) => Style::default().add_modifier(Modifier::BOLD),
                    (false, false, false) => Style::default(),
                };
                put(frame, value_x + 4, y, &super::bar::clip(&words, usize::from(row_end - value_x - 4)), style);
                gui.ui.click(pick, dest(DestAct::Pick(*radio)));
                if *radio == Radio::Custom {
                    // The layout in force, always: DIM unless Custom is
                    // chosen, TEXT with the caret under the hand.
                    let w = end - field_x;
                    let field = Rect { x: field_x, y, width: w, height: 1 };
                    if ui.hand == Some(DestStop::Layout) {
                        super::text_field(frame, &mut gui.ui, field_x, y, ui.layout.value(), ui.layout.cursor(), w, Style::default());
                    } else {
                        let style = if on { Style::default() } else { dim() };
                        put(frame, field_x, y, &super::clip_lead(ui.layout.value(), usize::from(w)), style);
                    }
                    let act = if on { DestAct::Hand(DestStop::Layout) } else { DestAct::Pick(Radio::Custom) };
                    gui.ui.click(field, dest(act));
                }
            }
            DestRow::Vars => {
                // Under Custom's field where they fit, else right-aligned to
                // the last cell, as far left as the labels' column: a wider
                // locale's label, or the seventh (SERVERFOLDER), never
                // loses one.
                let names = variables(&dview);
                let total = names.iter().map(|n| width(n)).sum::<usize>() + 3 * names.len().saturating_sub(1);
                let mut x = field_x.min(end.saturating_sub(total as u16)).max(label_x);
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
                    put(frame, x, y, name, if gui.ui.hovers(cell) { bright_bold() } else { dim() });
                    gui.ui.click(cell, dest(DestAct::Var(at)));
                    x += w;
                }
            }
            DestRow::Preview => {
                put(frame, label_x, y, &t!("gui.discover.dest.preview").to_uppercase(), dim());
                let mut runs: Vec<(String, Style)> = Vec::new();
                for (i, crumb) in landing_crumbs(&landing).into_iter().enumerate() {
                    if i > 0 {
                        runs.push((crumb_sep().to_string(), dim()));
                    }
                    runs.extend(crumb);
                }
                let clipped = put_tail(frame, value_x, y, &runs, room);
                if let Some(tip) = path_tip(&landing, clipped) {
                    gui.ui.tip(Rect { x: value_x, y, width: room as u16, height: 1 }, tip);
                }
            }
            DestRow::File => {
                if let Some(file) = view.file_name() {
                    put(frame, value_x, y, &super::bar::clip(&file, room), dim());
                }
            }
            DestRow::Applies => {
                put(frame, label_x, y, &super::bar::clip(&t!("gui.discover.dest.applies"), DEST_WRAP), dim());
            }
            DestRow::Error(line) => put(frame, label_x, y, line, Style::default().fg(th().gold)),
            DestRow::Buttons => {
                if saving {
                    let words = ell(&format!("{}…", t!("gui.discover.dest.saving")));
                    put(frame, end.saturating_sub(width(&words) as u16), y, &words, accent());
                    continue;
                }
                let mut right = end;
                for stop in ui.buttons(&dview).into_iter().rev() {
                    let text = format!("[ {} ]", dest_button(stop));
                    let w = width(&text) as u16;
                    let x = right.saturating_sub(w);
                    let cell = Rect { x, y, width: w, height: 1 };
                    let style = if ui.hand == Some(stop) {
                        sel().add_modifier(Modifier::BOLD)
                    } else if gui.ui.hovers(cell) {
                        bright_bold()
                    } else if stop == DestStop::Save {
                        accent().add_modifier(Modifier::BOLD)
                    } else {
                        dim()
                    };
                    put(frame, x, y, &text, style);
                    let act = match stop {
                        DestStop::UseDefault => DestAct::UseDefault,
                        DestStop::Cancel => DestAct::Back,
                        _ => DestAct::Save,
                    };
                    gui.ui.click(cell, dest(act));
                    right = x.saturating_sub(1);
                }
            }
        }
    }
}

// ── The Copies modal (clause 117) ───────────────────────────────────────────

/// The modal's width, and the height it never grows past: 22 rows fit the
/// 100 × 24 floor with the tab bar above and the tips line below.
const COPIES_W: u16 = 76;
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
            StateCell::Word(ell(&t!("gui.discover.keep.stopping")), dim())
        }
        JobOutcome::Running { progress, .. } => StateCell::Bar(*progress),
        JobOutcome::Queued => StateCell::Word(glyphed(JobMark::Waiting, "gui.discover.mark.waiting"), dim()),
        // In your collection only when something was copied (clause 120).
        JobOutcome::Copied { .. } | JobOutcome::CopiedMany(_) => {
            StateCell::Word(format!("{} {}", tick(), t!("gui.discover.copies.kept")), ok)
        }
        JobOutcome::Owned { .. } => StateCell::Word(format!("{} {}", tick(), t!("gui.discover.keep.yours")), dim()),
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
        JobOutcome::Queued => runs.push((t!("gui.discover.keep.waiting").to_string(), dim())),
        JobOutcome::Running { step, stopping, .. } => {
            let words = match (step, *stopping || row.cancelling) {
                (Some(step), true) => t!("gui.discover.copies.stopping_after", step = step).to_string(),
                (None, true) => ell(&t!("gui.discover.keep.stopping")),
                (Some(step), false) => step.clone(),
                (None, false) => ell(&t!("gui.discover.keep.copying")),
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
            let key = if *appearances > 0 { "gui.discover.keep.nothing_theirs" } else { "gui.discover.keep.all_yours" };
            runs.push((t!(key).to_string(), dim()));
        }
        JobOutcome::Failed { error: Some(error), many } => {
            runs.push((error.clone(), gold));
            runs.extend(copied(*many));
        }
        JobOutcome::Failed { error: None, many: Some(n) } if n.failed > 0 => {
            runs.push((plural("gui.discover.copies.failed_none", n.failed), gold));
        }
        JobOutcome::Failed { error: None, .. } => runs.push((t!("gui.discover.keep.failed_plain").to_string(), gold)),
        JobOutcome::Stopped { why, many } => {
            let reason = match why {
                StopReason::Quota => t!("gui.discover.keep.stopped_quota", server = server),
                StopReason::Busy => t!("gui.discover.copies.stopped_busy", server = server),
                StopReason::Peer | StopReason::Refused => t!("gui.discover.keep.stopped_peer", server = server),
                StopReason::Other => t!("gui.discover.keep.stopped_other"),
            };
            runs.push((reason.to_string(), gold));
            runs.extend(copied(Some(*many)));
        }
        JobOutcome::Refused { many } => {
            runs.push((t!("gui.discover.keep.refused", server = server).to_string(), gold));
            runs.extend(copied(*many));
        }
        JobOutcome::Cancelled { many } => {
            runs.push((t!("gui.discover.keep.cancelled").to_string(), dim()));
            runs.extend(copied(*many));
        }
        JobOutcome::Done => runs.push((t!("gui.discover.keep.finished").to_string(), dim())),
        JobOutcome::Unknown => {}
    }
    runs
}

/// A row's action, in the words its second row ends with (clause 117) —
/// a live copy's is its `[X]`, not words.
fn action_words(action: JobAction) -> Option<String> {
    match action {
        JobAction::Cancel => None,
        JobAction::Retry => Some(t!("gui.discover.keep.retry").to_string()),
        JobAction::CopyAgain => Some(t!("gui.discover.keep.copy_again").to_string()),
        JobAction::StartAgain => Some(t!("gui.discover.keep.start_again").to_string()),
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
        KeepNote::StartFailed(f) => (t!("gui.discover.keep.refused_start", words = failure_words(f)).to_string(), gold),
    }
}

/// The Copies modal (clause 117; the canvas's TermCopies board): the kit's
/// neutral modal, 76 wide, centred, its height following its rows up to
/// 22 — the list scrolls past that, `▲█▼` on its right border.
pub(crate) fn draw_copies(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if let Some(note) = gui.app.keep_take_note()
        && let Some(ui) = gui.discover.copies.as_mut()
    {
        ui.note = Some(note);
    }
    let (body, refresh) = copies_body(&gui.app);
    let cap = COPIES_MAX_H.min(area.height.saturating_sub(2));
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
    // The room beneath is inert (the kit's Modal; no scrim).
    gui.ui.click(area, Act::Guard);
    let rect = crate::kit::modal_rect(area, COPIES_W, h, COPIES_MAX_H);
    gui.ui.overlay(rect);
    gui.ui.modal_over();
    let inner = modal_box(frame, rect);
    gui.ui.click(rect, Act::Guard);
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
        Some(JobAction::Retry) => Some("gui.tips.discover_copies_retry"),
        Some(JobAction::CopyAgain) => Some("gui.tips.discover_copies_again"),
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
            let wide = if appearances > 0 { tail(head.clone()) } else { format!("{head} · {}", t!("gui.discover.keep.all_yours")) };
            (head, Some(wide), MessageKind::Info)
        }
        JobOutcome::Failed { error, .. } => {
            let words = error.unwrap_or_else(|| t!("gui.discover.keep.failed_plain").to_string());
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
    // Where files land's save that answered since (clause 144) — before
    // the picker draws, and said even once its picker is gone.
    dest_landed(gui);
    // Open elsewhere's hand-offs that answered since (clause 141).
    links_landed(gui);
    let news = gui.app.keep_take_news();
    if news.is_empty() {
        return;
    }
    let followed = gui.app.taste.keep.flow.as_ref().and_then(|f| match f.phase {
        KeepPhase::Following(id) => Some(id),
        _ => None,
    });
    let room = note_room(gui);
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

    /// The Keep modal's own rows (16 rows from the 7th at 100×30).
    pub(crate) fn modal_rows(rows: &[String]) -> Vec<String> {
        let top = rows.iter().position(|r| r.contains("Keep a copy") && r.contains("[X]")).expect("the modal") - 1;
        let bottom = (top + 1..rows.len()).find(|&y| rows[y].chars().nth(17) == Some('╰') || rows[y].chars().nth(17) == Some('+')).unwrap_or(rows.len() - 1);
        rows[top..=bottom].iter().map(|r| r.chars().skip(17).take(66).collect()).collect()
    }

    /// `cargo test dump_keep_states -- --ignored --nocapture`: every state.
    #[test]
    #[ignore]
    fn dump_keep_states() {
        let _en = english();
        let late = fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0));
        let show = |gui: &mut Gui, what: &str| {
            let rows = draw(gui, 100, 30);
            println!("── {what} · tips: {}", super::super::discover::tips(gui));
            println!("{}", modal_rows(&rows).join("\n"));
        };
        let row = crate::tui::app::keep::KeepTarget::Row { group: 0, row: 2 };
        let mut gui = canvas_gui(&[]);
        super::open(&mut gui, row);
        show(&mut gui, "loading facts");
        stage_facts(&mut gui);
        show(&mut gui, "idle");
        let refused = crate::api::PluginFailure::Refused { status: 403, words: "Uploading Disabled".into() };
        start(&mut gui, Err(refused));
        show(&mut gui, "refused 403");
        let busy = crate::api::PluginFailure::Refused { status: 409, words: "someone else on this server is already getting this — try again in a moment".into() };
        start(&mut gui, Err(busy));
        show(&mut gui, "refused 409");
        let states: Vec<(&str, &str, Value)> = vec![
            ("done one song", "done", json!({ "result": { "copied": { "vpath": "copies", "filepath": "copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3" } } })),
            ("already yours", "done", json!({ "result": { "skipped": "owned", "existing": { "filepath": "music/x.mp3" } } })),
            ("exists", "done", json!({ "result": { "skipped": "exists", "filepath": "copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3" } })),
            ("failed", "failed", json!({ "error": "Stage Peer B is unreachable" })),
            ("refused", "failed", json!({ "error": "Stage Peer B does not allow copies with your server's key" })),
            ("cancelled", "cancelled", json!({})),
        ];
        for (what, state, extra) in states {
            let mut gui = canvas_gui(&[]);
            super::open(&mut gui, row);
            stage_facts(&mut gui);
            start(&mut gui, Ok(job(90, &late, None, state, extra)));
            show(&mut gui, what);
        }
        // From the tile: album focus, and the copy's life.
        let tile = crate::tui::app::keep::KeepTarget::Tile(0);
        let mut gui = canvas_gui(&[]);
        super::open(&mut gui, tile);
        stage_facts(&mut gui);
        show(&mut gui, "tile idle");
        let album = |state: &str, extra: Value| job(91, &late, Some("album"), state, extra);
        start(&mut gui, Ok(album("queued", json!({}))));
        show(&mut gui, "tile queued");
        poll(&mut gui, &[album("running", json!({ "progress": 0.52, "statusText": "song 2 of 3 · Late Train Verse",
            "result": { "scope": "album", "counts": { "total": 3, "copied": 1, "skipped": 0, "failed": 0 } } }))]);
        show(&mut gui, "tile running");
        poll(&mut gui, &[album("running", json!({ "progress": 0.52, "cancelRequested": true,
            "result": { "scope": "album", "counts": { "total": 3, "copied": 1, "skipped": 0, "failed": 0 } } }))]);
        show(&mut gui, "tile stopping");
        for (what, state, extra) in [
            ("tile quota", "done", json!({ "result": { "scope": "album", "counts": { "total": 3, "copied": 1, "skipped": 0, "failed": 0 }, "stopped": "quota" } })),
            ("tile cancelled", "cancelled", json!({ "result": { "scope": "album", "counts": { "total": 3, "copied": 1, "skipped": 0, "failed": 0 } } })),
            ("tile nothing", "done", json!({ "result": { "scope": "album", "counts": { "total": 0, "copied": 0, "skipped": 0, "failed": 0 } } })),
        ] {
            poll(&mut gui, &[album(state, extra)]);
            show(&mut gui, what);
        }
        let _ = C;
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

    // ── The Keep a copy modal (clauses 106–110) ─────────────────────────

    /// The modal's rows from WHAT down to its bottom border, 66 wide.
    fn lower_rows(rows: &[String]) -> Vec<String> {
        let all = modal_rows(rows);
        let what = all.iter().position(|r| r.contains("WHAT")).expect("WHAT");
        all[what..].to_vec()
    }

    fn canvas(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|r| r.to_string()).collect()
    }

    #[test]
    fn the_modal_from_a_row_opens_on_the_song_where_the_canvas_draws_it() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        // Loading facts: DIM … for the sizes, no checkbox yet.
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(
            lower_rows(&rows),
            canvas(&[
                "│ WHAT      (•) This song                   0:08 · mp3           │",
                "│           ( ) Album                       …                    │",
                "│           ( ) Every album by Boukmanflow  …                    │",
                "│                                                                │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│           the library's default                    [ Change… ] │",
                "│                                                                │",
                "│                                        [ Cancel ] [ Add song ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ])
        );
        stage_facts(&mut gui);
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        // 66 wide, centred, its top on row 6 (anchored as if 17 rows).
        assert_eq!(rows[6].chars().nth(17), Some('╭'));
        assert_eq!(rows[6].chars().nth(82), Some('╮'));
        let modal = modal_rows(&rows);
        assert_eq!(modal.len(), 17, "{}", modal.join("\n"));
        assert_eq!(modal[1], "│ Keep a copy                                                 [X]│");
        assert!(modal[3].ends_with("Late Train Verse                                        │"));
        assert!(modal[4].ends_with("Boukmanflow · Borrowed Tapes · 2019 · from Stage Peer B │"));
        assert!(modal[5].ends_with("• online                                                │"));
        assert_eq!(
            lower_rows(&rows),
            canvas(&[
                "│ WHAT      (•) This song                   0:08 · mp3           │",
                "│           ( ) Album                       3 songs              │",
                "│           ( ) Every album by Boukmanflow  1 album · you have 3 │",
                "│               [ ] Only the albums you don't have               │",
                "│                                                                │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│           the library's default                    [ Change… ] │",
                "│                                                                │",
                "│                                        [ Cancel ] [ Add song ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ])
        );
        // The tokens (the canvas's TermKeepModal): the frame and the title
        // ACCENT, [X] DIM, the title BOLD, the byline DIM, the dot OK; WHAT
        // DIM, the picked radio OK and its focused label ACCENT BOLD, the
        // sizes DIM; LANDS IN DIM, the library BOLD, `›` DIM; whose place
        // it is DIM and `[ Change… ]` DIM (TermDestination F1); Cancel DIM,
        // the primary ACCENT BOLD.
        let cell = |x: u16, y: u16| Position { x, y };
        assert_eq!(fg(&buffer, cell(17, 6)), th().accent);
        assert_eq!((fg(&buffer, cell(19, 7)), bold(&buffer, cell(19, 7))), (th().accent, true));
        assert_eq!(fg(&buffer, cell(79, 7)), th().dim);
        assert!(bold(&buffer, cell(26, 9)));
        assert_eq!(fg(&buffer, cell(26, 10)), th().dim);
        assert_eq!(fg(&buffer, cell(26, 11)), th().ok);
        assert_eq!(fg(&buffer, cell(19, 13)), th().dim);
        assert_eq!(fg(&buffer, cell(30, 13)), th().ok);
        assert_eq!((fg(&buffer, cell(33, 13)), bold(&buffer, cell(33, 13))), (th().accent, true));
        assert_eq!(fg(&buffer, cell(61, 13)), th().dim);
        assert_eq!(fg(&buffer, cell(29, 14)), th().dim);
        assert_eq!(fg(&buffer, cell(33, 16)), th().dim);
        assert_eq!(fg(&buffer, cell(19, 18)), th().dim);
        assert!(bold(&buffer, cell(29, 18)));
        assert_eq!(fg(&buffer, cell(36, 18)), th().dim);
        let change = at(&rows, "[ Change… ]");
        assert_eq!((change.x, change.y), (70, 19), "cols 53–63 of the modal");
        assert_eq!((fg(&buffer, change), bold(&buffer, change)), (th().dim, false));
        assert_eq!(fg(&buffer, at(&rows, "the library's default")), th().dim);
        assert_eq!(fg(&buffer, cell(58, 21)), th().dim);
        assert_eq!((fg(&buffer, cell(69, 21)), bold(&buffer, cell(69, 21))), (th().accent, true));
        assert_eq!(
            super::super::discover::tips(&gui),
            "↑ ↓ rows · Space or Enter pick · Enter on Add song starts · d where files land · Esc close"
        );
        // The room beneath is inert; the controls take their clicks.
        assert_eq!(gui.ui.hit(Position { x: 40, y: 25 }), Some(Act::Guard));
        assert_eq!(gui.ui.hit(Position { x: 80, y: 7 }), Some(Act::Disc(DiscAct::Keep(super::KeepAct::Close))));
        assert_eq!(gui.ui.hit(Position { x: 35, y: 14 }), Some(Act::Disc(DiscAct::Keep(super::KeepAct::Pick(crate::tui::app::keep::KeepChoice::Album)))));
        assert_eq!(gui.ui.hit(Position { x: 75, y: 19 }), Some(Act::Disc(DiscAct::Keep(super::KeepAct::Change))));
        assert_eq!(gui.ui.hit(Position { x: 72, y: 21 }), Some(Act::Disc(DiscAct::Keep(super::KeepAct::Primary))));
        // The strip's covers under the frame draw as the mosaic next frame
        // (Over pixels): its footprint is registered.
        assert!(gui.ui.overlays_moved() || gui.ui.covered_last_frame(ratatui::layout::Rect { x: 17, y: 6, width: 12, height: 5 }));
    }

    #[test]
    fn the_keys_walk_the_choice_and_add_starts_with_the_row_unchanged() {
        let _en = english();
        let mut gui = canvas_gui(&canvas_jobs());
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        draw(&mut gui, 100, 30);
        // ↓ walks to Album, Space picks it: the primary follows.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char(' '));
        let rows = draw(&mut gui, 100, 30);
        assert!(lower_rows(&rows)[8].ends_with("[ Cancel ] [ Add album ] │"), "{}", lower_rows(&rows).join("\n"));
        // Every album by lands album by album.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[5], "│ LANDS IN  copies › Boukmanflow › one folder per album          │");
        // Ticking "Only the albums you don't have" picks Every album by —
        // and the one album it takes is named where it lands.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        let rows = draw(&mut gui, 100, 30);
        let low = lower_rows(&rows);
        assert_eq!(low[2], "│           (•) Every album by Boukmanflow  1 album · you have 3 │");
        assert_eq!(low[3], "│               [✓] Only the albums you don't have               │");
        assert_eq!(low[5], "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │");
        assert!(low[8].ends_with("[ Cancel ] [ Add 1 album ] │"), "{}", low.join("\n"));
        // Picking a radio again unticks it.
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.app.keep_view().is_some_and(|v| v.missing == Some((false, 1))), "the album unticks it");
        // Enter with the hand on the primary starts — the row exactly as
        // the taste route sent it (clause 100); the hand walks the
        // checkbox, Change…, Cancel on the way.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.discover.keep.focus, Some(super::Stop::Change));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        let sent = starts(&gui);
        assert_eq!(sent.len(), 1, "{:?}", gui.pending);
        assert_eq!(sent[0].0, fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0)), "unchanged");
        assert_eq!(sent[0].1, crate::api::types::JobScope::ArtistMissing);
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(
            lower_rows(&rows),
            canvas(&[
                "│ WHAT      Every album by Boukmanflow · 1 album                 │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ starting…                                                      │",
                "│                                                                │",
                "│                                                                │",
                "│                                                                │",
                "│                                                                │",
                "╰────────────────────────────────────────────────────────────────╯",
            ])
        );
        assert_eq!(super::super::discover::tips(&gui), "Esc close");
        // Esc closes; nothing else goes out.
        key(&mut gui, KeyCode::Esc);
        assert!(gui.app.taste.keep.flow.is_none());
    }

    /// Open on Late Train Verse with the stage's facts, and start a copy
    /// the server answers with a job in `state`.
    fn row_in(state: &str, scope: Option<&str>, extra: Value) -> Gui {
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        let late = fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0));
        if let Some(scope) = scope {
            let choice = if scope == "album" { crate::tui::app::keep::KeepChoice::Album } else { crate::tui::app::keep::KeepChoice::Artist };
            gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Pick(choice))));
        }
        start(&mut gui, Ok(job(90, &late, scope, state, extra)));
        gui
    }

    /// Open on the tile of Borrowed Tapes from Stage Peer B and follow an
    /// album copy through `states`.
    fn tile_in(states: &[(&str, Value)]) -> Gui {
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        stage_facts(&mut gui);
        let late = fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0));
        let album = |state: &str, extra: &Value| job(91, &late, Some("album"), state, extra.clone());
        start(&mut gui, Ok(album(states[0].0, &states[0].1)));
        for (state, extra) in &states[1..] {
            poll(&mut gui, &[album(state, extra)]);
        }
        gui
    }

    fn counts(copied: u64, skipped: u64, failed: u64, stopped: Option<&str>) -> Value {
        json!({ "result": { "scope": "album", "counts": { "total": 3, "copied": copied, "skipped": skipped, "failed": failed }, "stopped": stopped } })
    }

    #[test]
    fn every_state_reads_as_the_canvas_draws_it() {
        let _en = english();
        let copied = json!({ "result": { "copied": { "vpath": "copies", "filepath": "copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3" } } });
        let running = json!({ "progress": 0.52, "statusText": "song 2 of 3 · Late Train Verse",
            "result": { "scope": "album", "counts": { "total": 3, "copied": 1, "skipped": 0, "failed": 0 } } });
        let stopping = json!({ "progress": 0.52, "cancelRequested": true,
            "result": { "scope": "album", "counts": { "total": 3, "copied": 1, "skipped": 0, "failed": 0 } } });
        let many = json!({ "result": { "scope": "artist", "albums": [{ "name": "Borrowed Tapes" }],
            "skippedAlbums": [{ "name": "A", "why": "owned" }, { "name": "B", "why": "owned" }],
            "counts": { "total": 3, "copied": 1, "skipped": 1, "failed": 1 }, "stopped": null } });
        let theirs = json!({ "result": { "scope": "artist", "albums": [], "skippedAlbums": [{ "name": "Comp", "why": "appearance" }],
            "counts": { "total": 0, "copied": 0, "skipped": 0, "failed": 0 }, "stopped": null } });
        let cases: Vec<(&str, Gui, Vec<&str>, &str)> = vec![
            ("done, one song", row_in("done", None, copied), vec![
                "│ WHAT      This song · 0:08 · mp3                               │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ ✓ in your collection                                           │",
                "│ copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3    │",
                "│                                                                │",
                "│                                                                │",
                "│                            [ Add to queue ] [ Play your copy ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Enter play your copy · ↑ ↓ buttons · Esc close"),
            ("done, many", row_in("done", Some("artist"), many), vec![
                "│ WHAT      Every album by Boukmanflow                           │",
                "│ LANDS IN  copies › Boukmanflow › one folder per album          │",
                "│                                                                │",
                "│ ✓ in your collection                                           │",
                "│ 1 copied · 1 already yours · 1 failed · 2 albums left out      │",
                "│ Copy again takes only the gaps                                 │",
                "│                                                                │",
                "│                                       [ Close ] [ Copy again ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Enter copy again · ↑ ↓ buttons · Esc close"),
            // Already yours: LANDS IN reads where the library has it.
            ("already yours", row_in("done", None, json!({ "result": { "skipped": "owned", "existing": { "filepath": "music/Boukmanflow/Borrowed Tapes/02.mp3" } } })), vec![
                "│ WHAT      This song · 0:08 · mp3                               │",
                "│ LANDS IN  music › Boukmanflow › Borrowed Tapes                 │",
                "│                                                                │",
                "│ ✓ already yours                                                │",
                "│ your library already has this song · nothing was copied        │",
                "│                                                                │",
                "│                                                                │",
                "│                                                      [ Close ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Enter close · Esc close"),
            ("a file exists", row_in("done", None, json!({ "result": { "skipped": "exists", "filepath": "copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3" } })), vec![
                "│ WHAT      This song · 0:08 · mp3                               │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ skipped · a file is already at that path                       │",
                "│ copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3    │",
                "│ nothing was overwritten                                        │",
                "│                                                                │",
                "│                                            [ Close ] [ Retry ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Enter retry · ↑ ↓ buttons · Esc close"),
            ("failed", row_in("failed", None, json!({ "error": "Stage Peer B is unreachable" })), vec![
                "│ WHAT      This song · 0:08 · mp3                               │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ copy failed: Stage Peer B is unreachable                       │",
                "│ nothing was copied                                             │",
                "│                                                                │",
                "│                                                                │",
                "│                                            [ Close ] [ Retry ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Enter retry · ↑ ↓ buttons · Esc close"),
            ("nothing of theirs", row_in("done", Some("artist"), theirs), vec![
                "│ WHAT      Every album by Boukmanflow                           │",
                "│ LANDS IN  copies › Boukmanflow › one folder per album          │",
                "│                                                                │",
                "│ nothing of theirs to copy                                      │",
                "│ Boukmanflow only appears on other artists' albums there        │",
                "│                                                                │",
                "│                                                                │",
                "│                                                      [ Close ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Esc close"),
            ("queued", tile_in(&[("queued", json!({}))]), vec![
                "│ WHAT      Album · 3 songs                                      │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ waiting for a free slot                                        │",
                "│ ▱▱▱▱▱▱▱▱▱▱                                                     │",
                "│ starts on its own when the copy ahead ends                     │",
                "│                                                                │",
                "│                                                [ Cancel copy ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "x cancel the copy · Esc close — it stays queued"),
            ("running", tile_in(&[("queued", json!({})), ("running", running.clone())]), vec![
                "│ WHAT      Album · 3 songs                                      │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ song 2 of 3 · Late Train Verse                                 │",
                "│ ▰▰▰▰▰▱▱▱▱▱ 52%                                                 │",
                "│ 1 copied · 0 failed                                            │",
                "│                                                                │",
                "│                                                [ Cancel copy ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "x cancel the copy · Esc close — the copy keeps running"),
            ("stopping", tile_in(&[("running", running.clone()), ("running", stopping)]), vec![
                "│ WHAT      Album · 3 songs                                      │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ stopping…                                                      │",
                "│ ▰▰▰▰▰▱▱▱▱▱ 52%                                                 │",
                "│ 1 copied · 0 failed                                            │",
                "│                                                                │",
                "│                                                                │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Esc close"),
            ("stopped, transfer limit", tile_in(&[("running", running.clone()), ("done", counts(1, 0, 0, Some("quota")))]), vec![
                "│ WHAT      Album · 3 songs                                      │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ Stage Peer B's transfer limit stopped it                       │",
                "│ 1 copied · 2 not copied · try again later                      │",
                "│                                                                │",
                "│                                                                │",
                "│                                            [ Close ] [ Retry ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Enter retry · ↑ ↓ buttons · Esc close"),
            ("stopped, busy", tile_in(&[("running", running.clone()), ("done", counts(1, 0, 0, Some("busy")))]), vec![
                "│ WHAT      Album · 3 songs                                      │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ Stage Peer B is serving too many streams right now             │",
                "│ 1 copied · 2 not copied · try again in a moment                │",
                "│                                                                │",
                "│                                                                │",
                "│                                            [ Close ] [ Retry ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Enter retry · ↑ ↓ buttons · Esc close"),
            ("stopped, went away", tile_in(&[("running", running.clone()), ("done", counts(1, 0, 0, Some("peer")))]), vec![
                "│ WHAT      Album · 3 songs                                      │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ Stage Peer B went away                                         │",
                "│ 1 copied · 2 not copied                                        │",
                "│                                                                │",
                "│                                                                │",
                "│                                            [ Close ] [ Retry ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Enter retry · ↑ ↓ buttons · Esc close"),
            ("stopped, refused", tile_in(&[("running", running.clone()), ("done", counts(0, 0, 0, Some("refused")))]), vec![
                "│ WHAT      Album · 3 songs                                      │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ Stage Peer B does not allow copies with your server's key      │",
                "│ nothing was copied · playing it still works                    │",
                "│                                                                │",
                "│                                                                │",
                "│                                                      [ Close ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Esc close"),
            ("cancelled", tile_in(&[("running", running.clone()), ("cancelled", counts(1, 0, 0, None))]), vec![
                "│ WHAT      Album · 3 songs                                      │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ cancelled · 1 copied                                           │",
                "│ the other 2 stay on Stage Peer B                               │",
                "│                                                                │",
                "│                                                                │",
                "│                                      [ Close ] [ Start again ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Enter start again · ↑ ↓ buttons · Esc close"),
            ("nothing to copy", tile_in(&[("running", running), ("done", json!({ "result": { "scope": "album", "counts": { "total": 0, "copied": 0, "skipped": 0, "failed": 0 } } }))]), vec![
                "│ WHAT      Album · 3 songs                                      │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ ✓ nothing to copy · you have it all                            │",
                "│                                                                │",
                "│                                                                │",
                "│                                                                │",
                "│                                                      [ Close ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ], "Esc close"),
        ];
        for (what, mut gui, want, tips) in cases {
            let rows = draw(&mut gui, 100, 30);
            assert_eq!(lower_rows(&rows), canvas(&want), "{what}:\n{}", modal_rows(&rows).join("\n"));
            assert_eq!(super::super::discover::tips(&gui), tips, "{what}");
        }
    }

    #[test]
    fn a_refusal_brings_the_choice_back_with_the_servers_words_in_gold() {
        let _en = english();
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        start(&mut gui, Err(crate::api::PluginFailure::Refused { status: 403, words: "Uploading Disabled".into() }));
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let low = lower_rows(&rows);
        assert_eq!(low[6], "│           the library's default                    [ Change… ] │");
        assert_eq!(low[7], "│ couldn't start: Uploading Disabled                             │");
        assert_eq!(low[8], "│                                        [ Cancel ] [ Add song ] │");
        let words = at(&rows, "couldn't start");
        assert_eq!(fg(&buffer, words), th().gold);
        // The hand is back on the chosen radio (the canvas: ACCENT BOLD).
        let song = at(&rows, "This song");
        assert_eq!((fg(&buffer, song), bold(&buffer, song)), (th().accent, true));
        // 409 wraps to a second row; the choice stays editable, Add stays.
        let busy = "someone else on this server is already getting this — try again in a moment";
        start(&mut gui, Err(crate::api::PluginFailure::Refused { status: 409, words: busy.into() }));
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[7], "│ couldn't start: someone else on this server is already getting │");
        assert_eq!(low[8], "│ this — try again in a moment                                   │");
        assert_eq!(low[9], "│                                        [ Cancel ] [ Add song ] │");
        // A server that did not answer reads so, never a bare error.
        start(&mut gui, Err(crate::api::PluginFailure::Unreachable("connection refused".into())));
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[7], "│ couldn't start: your server didn't answer                      │");
    }

    #[test]
    fn the_tile_opens_album_focus_and_sends_its_best_row_with_scope_album() {
        let _en = english();
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        stage_facts(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        let modal = modal_rows(&rows);
        assert_eq!(modal.len(), 16, "16 rows, anchored as if 17: {}", modal.join("\n"));
        assert_eq!(rows[6].chars().nth(17), Some('╭'), "the top holds on row 6");
        assert!(modal[3].ends_with("Borrowed Tapes                                          │"));
        assert!(modal[4].ends_with("Boukmanflow · 2019 · from Stage Peer B                  │"));
        assert_eq!(
            lower_rows(&rows),
            canvas(&[
                "│ WHAT      (•) Album                       3 songs              │",
                "│           ( ) Every album by Boukmanflow  1 album · you have 3 │",
                "│               [ ] Only the albums you don't have               │",
                "│                                                                │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│           the library's default                    [ Change… ] │",
                "│                                                                │",
                "│                                       [ Cancel ] [ Add album ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ])
        );
        // Enter on the held radio picks it; the primary starts — the
        // tile's best row with scope album, never a song job.
        key(&mut gui, KeyCode::Enter);
        assert!(starts(&gui).is_empty(), "Enter on a radio picks, it never starts");
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Primary)));
        let sent = starts(&gui);
        assert_eq!(
            sent,
            vec![(fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0)), crate::api::types::JobScope::Album)]
        );
    }

    #[test]
    fn a_choice_an_only_missing_copy_already_kept_reads_its_state_never_add() {
        let _en = english();
        // The user's 2026-10-20 report: "Only the albums you don't have"
        // copied Borrowed Tapes; the tile and the row then read that.
        let late = fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0));
        let landed = "copies/Boukmanflow/Borrowed Tapes/Late Train Verse.mp3";
        let songs = json!({ "total": 1, "skipped": [], "failed": [],
            "copied": [{ "from": late["filepath"], "vpath": "copies", "filepath": landed, "title": "Late Train Verse" }] });
        let walk = |albums: Value, left: Value| {
            job(80, &late, Some("artist-missing"), "done", json!({ "result": { "scope": "artist-missing",
                "albums": albums, "skippedAlbums": left, "songs": songs, "counts": { "total": 1, "copied": 1, "skipped": 0, "failed": 0 },
                "stopped": null, "peer": { "id": B, "name": "Stage Peer B" } } }))
        };
        let kept = walk(json!([{ "name": "Borrowed Tapes", "songs": songs, "stopped": null }]), json!([]));
        let mut gui = canvas_gui(&[kept]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        stage_facts(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(
            lower_rows(&rows),
            canvas(&[
                "│ WHAT      Album · 3 songs                                      │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ ✓ in your collection                                           │",
                "│ 1 copied                                                       │",
                "│ Copy again takes only the gaps                                 │",
                "│                                                                │",
                "│                                       [ Close ] [ Copy again ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ])
        );
        // Copy again: the album's copy of the tile's row, never the artist copy again.
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Primary)));
        assert_eq!(starts(&gui), vec![(late.clone(), crate::api::types::JobScope::Album)]);
        // The row: This song in your collection, its landed file.
        super::close(&mut gui);
        gui.pending.clear();
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(
            lower_rows(&rows),
            canvas(&[
                "│ WHAT      This song · 0:08 · mp3                               │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│                                                                │",
                "│ ✓ in your collection                                           │",
                "│ copies/Boukmanflow/Borrowed Tapes/Late Train Verse.mp3         │",
                "│                                                                │",
                "│                                                                │",
                "│                            [ Add to queue ] [ Play your copy ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ])
        );
        // Add to queue queues the user's own file.
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Secondary)));
        assert_eq!(gui.app.queue.items.last().map(|q| q.filepath.as_str()), Some(landed));
        // Left out as already here: already yours, Close alone.
        let owned = walk(json!([]), json!([{ "name": "Borrowed Tapes", "why": "owned" }]));
        let mut gui = canvas_gui(&[owned]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        stage_facts(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        let lower = lower_rows(&rows);
        assert_eq!(lower[3], "│ ✓ already yours                                                │", "{}", lower.join("\n"));
        assert_eq!(lower[lower.len() - 2], "│                                                      [ Close ] │");
        // A guest spot: nothing claimed — Add album.
        let guest = walk(json!([]), json!([{ "name": "Borrowed Tapes", "why": "appearance" }]));
        let mut gui = canvas_gui(&[guest]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        stage_facts(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        assert!(lower_rows(&rows).iter().any(|r| r.contains("[ Cancel ] [ Add album ]")), "{}", rows.join("\n"));
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

    #[test]
    fn a_compilation_tile_copies_the_whole_album_by_its_row_unchanged() {
        let _en = english();
        // The user's 2026-10-19 decision: a row carrying albumArtist groups
        // as one tile credited to it, offers Album alone (never "Every album
        // by" the track artist) and sends the row unchanged.
        let (mut gui, kim) = mixtape_gui();
        let rows = draw(&mut gui, 100, 30);
        assert!(room(&rows, 11).starts_with("Mixtape Vol…") && room(&rows, 12).starts_with("Various Art…"), "one tile, its credit:\n{}", rows.join("\n"));
        assert!(!room(&rows, 11).contains("Mixtape Vol… Mixtape"), "never one per track artist");
        super::open(&mut gui, KeepTarget::Tile(0));
        land_facts(&mut gui, Some(2), &[], &[]);
        let rows = draw(&mut gui, 100, 30);
        let modal = modal_rows(&rows);
        assert!(modal[4].contains("Various Artists · 2019 · from Stage Peer B"), "{}", modal.join("\n"));
        let low = lower_rows(&rows);
        assert_eq!(low[0], "│ WHAT      (•) Album                       2 songs              │");
        assert!(!low.iter().any(|r| r.contains("Every album by")), "no artist copy from a compilation's tile");
        // The server's default layout files it by the album's credit: the
        // compilation stays in one folder.
        assert_eq!(low[2], "│ LANDS IN  copies › Various Artists › Mixtape Vol 1             │");
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Primary)));
        let sent = starts(&gui);
        assert_eq!(sent, vec![(kim, crate::api::types::JobScope::Album)], "the best row, unchanged, scope album");
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

    #[test]
    fn a_compilations_album_draws_each_songs_artist_as_a_slot_with_its_note_in_the_tooltip() {
        let _en = english();
        // The 2026-10-22 bug: "copies › From friends › Boukmanflow" for an
        // album whose songs the server files under each one's own artist.
        let note = "‹artist› comes from each song's own tags, so these songs can land in more than one folder.";
        let (mut gui, kim) = mixtape_gui();
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        land_read(&mut gui, layout_view("From friends/{{ARTIST}}"));
        // Before the listing answers, and once it names two artists: the slot.
        for listing in [None, Some(&[("Kim Song", "Peer Kim"), ("Lou Song", "Peer Lou")])] {
            if let Some(songs) = listing {
                land_songs(&mut gui, songs);
            }
            let buffer = buffer(&mut gui, 100, 30);
            let rows = lines(&buffer);
            let low = lower_rows(&rows);
            assert_eq!(
                low[1..4].to_vec(),
                canvas(&[
                    "│                                                                │",
                    "│ LANDS IN  copies › From friends › ‹artist›                     │",
                    "│           saved for your account                   [ Change… ] │",
                ]),
                "{}",
                low.join("\n")
            );
            // DIM, as "one folder per album" — a hint, never a folder's name.
            assert_eq!(fg(&buffer, at(&rows, "‹artist›")), th().dim);
            assert_ne!(fg(&buffer, at(&rows, "From friends")), th().dim);
            let y = at_row(&rows, "‹artist›");
            assert!(gui.ui.tips.iter().any(|(rect, tip)| rect.contains(Position { x: 40, y }) && tip == note), "{:?}", gui.ui.tips);
        }
        // The picker's PREVIEW draws the same, its note in its tooltip.
        key(&mut gui, KeyCode::Char('d'));
        land_folders(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        let preview = picker.iter().position(|r| r.contains("PREVIEW")).expect("PREVIEW");
        assert_eq!(picker[preview..preview + 2].to_vec(), boxed(&["", " PREVIEW   copies › From friends › ‹artist›", ""])[2..4].to_vec());
        let y = at_row(&rows, "PREVIEW");
        assert!(gui.ui.tips.iter().any(|(rect, tip)| rect.contains(Position { x: 40, y }) && tip == note), "{:?}", gui.ui.tips);
        key(&mut gui, KeyCode::Esc);
        // Starting and queued: the place sent, then the job's pin — the slot stands.
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Primary)));
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(lower_rows(&rows)[1], "│ LANDS IN  copies › From friends › ‹artist›                     │");
        let ticket = gui.pending.iter().rev().find_map(|e| match e {
            Effect::Api(ApiCmd::JobStart { ticket, .. }) => Some(*ticket),
            _ => None,
        });
        let mut queued = job(92, &kim, Some("album"), "queued", json!({}));
        queued.params.destination = Some(Some(serde_json::from_value(layout_view("From friends/{{ARTIST}}")["destination"].clone()).unwrap()));
        let generation = gui.app.session_gen();
        let answer = Ok(Box::new(crate::api::types::JobStarted { job: queued, created: true }));
        apply(&mut gui, Event::JobStarted { ticket: ticket.expect("a start"), generation, answer });
        let rows = draw(&mut gui, 100, 30);
        let low = lower_rows(&rows);
        assert_eq!(low[1], "│ LANDS IN  copies › From friends › ‹artist›                     │", "{}", low.join("\n"));
        assert!(low.iter().any(|r| r.contains("waiting")), "{}", low.join("\n"));
        // Every song by one artist: that folder, no slot and no note.
        let (mut gui, _) = mixtape_gui();
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        land_read(&mut gui, layout_view("From friends/{{ARTIST}}"));
        land_songs(&mut gui, &[("Kim Song", "Peer Kim"), ("Kim Two", "Peer Kim")]);
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(lower_rows(&rows)[2], "│ LANDS IN  copies › From friends › Peer Kim                     │");
        assert!(!gui.ui.tips.iter().any(|(_, tip)| tip.contains("own tags")), "{:?}", gui.ui.tips);
    }

    #[test]
    fn retry_adds_to_draws_a_genre_slot_and_its_note_names_the_folder_a_song_leaves_out() {
        let _en = english();
        // An album copy pinned to the library default failed; the account
        // has saved a layout by genre since — no song's genre is known here.
        let mut gui = row_in("failed", Some("album"), json!({ "error": "Stage Peer B is unreachable" }));
        land_read(&mut gui, layout_view("{{GENRE}}/{{ARTIST}} - {{YEAR}}"));
        let rows = draw(&mut gui, 100, 30);
        let low = lower_rows(&rows);
        assert_eq!(
            low[1..3].to_vec(),
            canvas(&[
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│           retry adds to copies › ‹genre› › Boukmanflow - 2019  │",
            ]),
            "{}",
            low.join("\n")
        );
        let note = "‹genre› comes from each song's own tags, so these songs can land in more than one folder (a song with none leaves that folder out).";
        let y = at_row(&rows, "retry adds to");
        assert!(gui.ui.tips.iter().any(|(rect, tip)| rect.contains(Position { x: 40, y }) && tip == note), "{:?}", gui.ui.tips);
        assert!(
            !gui.ui.tips.iter().any(|(rect, tip)| rect.contains(Position { x: 40, y: y - 1 }) && tip.contains("own tags")),
            "the pinned place has no slot, so no note: {:?}",
            gui.ui.tips
        );
        // The legacy console draws the slot in its own brackets.
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("retry adds to copies > <genre> > Boukmanflow - 2019")), "{}", rows.join("\n"));
    }

    #[test]
    fn an_album_spread_over_the_servers_folders_draws_a_slot_with_its_own_note() {
        let _en = english();
        // Mixtape Vol 1 on Stage Peer B sits in each artist's own folder.
        let keep = |layout: &str, songs: Option<&[(&str, &str)]>| {
            let (mut gui, _) = mixtape_gui();
            draw(&mut gui, 100, 30);
            super::open(&mut gui, KeepTarget::Tile(0));
            land_read(&mut gui, layout_view(layout));
            if let Some(songs) = songs {
                land_songs(&mut gui, songs);
            }
            gui
        };
        let lands = |gui: &mut Gui| {
            let rows = draw(gui, 100, 30);
            let low = lower_rows(&rows);
            low.iter().find(|r| r.contains("LANDS IN")).cloned().unwrap_or_else(|| panic!("no LANDS IN: {}", low.join("\n")))
        };
        let tip_at = |gui: &Gui, y: u16| gui.ui.tips.iter().find(|(rect, _)| rect.contains(Position { x: 40, y })).map(|(_, tip)| tip.clone());
        let two = [("Kim Song", "Peer Kim"), ("Lou Song", "Peer Lou")];
        // Before the listing answers: the tile's row's own folders.
        let mut gui = keep("From friends/{{SERVERFOLDER}}", None);
        assert_eq!(lands(&mut gui), format!("│{:<64}│", " LANDS IN  copies › From friends › Peer Kim › Mixtape Vol 1"));
        // The listing puts its songs in two folders: the slot, DIM, and its
        // note in the path's tooltip.
        let mut gui = keep("From friends/{{SERVERFOLDER}}", Some(&two[..]));
        assert_eq!(lands(&mut gui), format!("│{:<64}│", " LANDS IN  copies › From friends › ‹server folder›"));
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(fg(&buffer, at(&rows, "‹server folder›")), th().dim);
        let note = "‹server folder› comes from where each song sits on the server, so these songs can land in more than one folder.";
        assert_eq!(tip_at(&gui, at_row(&rows, "LANDS IN")).as_deref(), Some(note));
        // Beside the tags' slots one sentence names both: the tags first, in
        // the path's order, their number picking the verb, then the server
        // folder; with a genre, the folder a song with none leaves out.
        let mut gui = keep("{{SERVERFOLDER}}/{{ARTIST}}", Some(&two[..]));
        assert_eq!(lands(&mut gui), format!("│{:<64}│", " LANDS IN  copies › ‹server folder› › ‹artist›"));
        let rows = draw(&mut gui, 100, 30);
        let note = "‹artist› comes from each song's own tags and ‹server folder› from where it sits on the server, so these songs can land in more than one folder.";
        assert_eq!(tip_at(&gui, at_row(&rows, "LANDS IN")).as_deref(), Some(note));
        let mut gui = keep("{{GENRE}}/{{SERVERFOLDER}}", Some(&two[..]));
        let rows = draw(&mut gui, 100, 30);
        let note = "‹genre› comes from each song's own tags and ‹server folder› from where it sits on the server, so these songs can land in more than one folder (a song with none leaves that folder out).";
        assert_eq!(tip_at(&gui, at_row(&rows, "LANDS IN")).as_deref(), Some(note));
        let mut gui = keep("{{ARTIST}}/{{SERVERFOLDER}}/{{GENRE}}", Some(&two[..]));
        let rows = draw(&mut gui, 100, 30);
        let note = "‹artist›, ‹genre› come from each song's own tags and ‹server folder› from where it sits on the server, so these songs can land in more than one folder (a song with none leaves that folder out).";
        assert_eq!(tip_at(&gui, at_row(&rows, "LANDS IN")).as_deref(), Some(note));
        // The phone's words in another locale, its plural picked by the tags.
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
            let rows = draw(&mut gui, 100, 30);
            let note = "‹wykonawca› pochodzi ze znaczników każdego utworu, a ‹folder z serwera› z miejsca, w którym leży on na serwerze, więc te utwory mogą trafić do więcej niż jednego folderu.";
            assert_eq!(tip_at(&gui, at_row(&rows, "TRAFI DO")).as_deref(), Some(note));
        }
        // Every song in one folder: that folder, no note.
        let mut gui = keep("From friends/{{SERVERFOLDER}}", Some(&[("Kim Song", "Peer Kim"), ("Kim Two", "Peer Kim")][..]));
        assert_eq!(lands(&mut gui), format!("│{:<64}│", " LANDS IN  copies › From friends › Peer Kim › Mixtape Vol 1"));
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(tip_at(&gui, at_row(&rows, "LANDS IN")), None);
        // The song: its own folders, inside the folder's words — never a slot.
        let mut gui = keep_on(layout_view("Copy of {{SERVERFOLDER}}"));
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("LANDS IN  copies › Copy of Boukmanflow › Borrowed Tapes")), "{}", rows.join("\n"));
        assert_eq!(tip_at(&gui, at_row(&rows, "LANDS IN")), None);
        // The legacy console's brackets.
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let mut gui = keep("{{SERVERFOLDER}}", Some(&two[..]));
        assert!(draw(&mut gui, 100, 30).iter().any(|r| r.contains("LANDS IN  copies > <server folder>")));
    }

    #[test]
    fn another_servers_live_copy_hides_the_primary_never_greys_it() {
        let _en = english();
        // Stage Peer C copies Borrowed Tapes; Stage Peer B's tile reads so.
        let rooftop = fed(C, "Rooftop Cypher", "Boukmanflow", "Borrowed Tapes", 0.6, Some(12.0));
        let theirs = job(40, &rooftop, Some("album"), "running", json!({ "progress": 0.3 }));
        let mut gui = canvas_gui(&[theirs]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        stage_facts(&mut gui);
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(
            lower_rows(&rows),
            canvas(&[
                "│ WHAT      (•) Album                       3 songs              │",
                "│           ( ) Every album by Boukmanflow  1 album · you have 3 │",
                "│               [ ] Only the albums you don't have               │",
                "│                                                                │",
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│           the library's default                    [ Change… ] │",
                "│                                                                │",
                "│ Stage Peer C is already copying this album — you can add       │",
                "│ Stage Peer B's once it ends                                    │",
                "╰────────────────────────────────────────────────────────────────╯",
            ])
        );
        assert_eq!(fg(&buffer, at(&rows, "Stage Peer C is already")), th().accent);
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · Space or Enter pick · d where files land · Esc close");
        key(&mut gui, KeyCode::Enter);
        assert!(starts(&gui).is_empty(), "no primary, no start");
        // The tile itself wears none of Stage Peer C's copy.
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Close)));
        let rows = draw(&mut gui, 100, 30);
        assert!(room(&rows, 12).starts_with("Boukmanflow"), "{}", rows[12]);
    }

    #[test]
    fn the_modal_at_the_floor_is_centred_with_every_row() {
        let _en = english();
        let mut gui = canvas_gui(&[]);
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        draw(&mut gui, 100, 24);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        let rows = draw(&mut gui, 100, 24);
        // 17 rows, its top on row 3 (anchored as if 17: TermDestination's
        // floor), its bottom over the bar's seek row.
        assert_eq!(rows[3].chars().nth(17), Some('╭'), "{}", rows.join("\n"));
        assert_eq!(rows[19].chars().nth(17), Some('╰'), "{}", rows.join("\n"));
        assert!(rows[23].starts_with(" ↑ ↓ rows · Space or Enter pick · Enter on Add song starts · d where files land · Esc close"), "the tips row");
    }

    #[test]
    fn the_legacy_console_frames_the_modal_in_ascii_and_says_the_bar_in_words() {
        let _en = english();
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Missing)));
        let rows = draw(&mut gui, 100, 30);
        let modal: Vec<String> = rows[6..23].iter().map(|r| r.chars().skip(17).take(66).collect()).collect();
        assert_eq!(modal[0], "+----------------------------------------------------------------+");
        assert_eq!(modal[1], "| Keep a copy                                                 [X]|");
        assert!(modal[5].ends_with("* online                                                |"));
        assert_eq!(modal[7], "| WHAT      ( ) This song                   0:08 · mp3           |");
        assert_eq!(modal[9], "|           (*) Every album by Boukmanflow  1 album · you have 3 |");
        assert_eq!(modal[10], "|               [x] Only the albums you don't have               |");
        assert_eq!(modal[12], "| LANDS IN  copies > Boukmanflow > Borrowed Tapes                |");
        assert_eq!(modal[13], "|           the library's default                  [ Change... ] |");
        assert_eq!(modal[15], "|                                     [ Cancel ] [ Add 1 album ] |");
        assert_eq!(modal[16], "+----------------------------------------------------------------+");
        // The running copy's bar row says its percentage in words.
        let late = fed(B, "Late Train Verse", "Boukmanflow", "Borrowed Tapes", 0.99, Some(8.0));
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        stage_facts(&mut gui);
        start(&mut gui, Ok(job(91, &late, Some("album"), "running", json!({ "progress": 0.52, "statusText": "song 2 of 3 · Late Train Verse" }))));
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("| 52% done ")), "{}", rows.join("\n"));
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

    // ── Where files land (clauses 142–148; the TermDestination board) ───

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

    /// The picker's own rows, 76 wide, from its top border.
    fn picker_rows(rows: &[String]) -> Vec<String> {
        let top = rows.iter().position(|r| r.contains("Where files land") && r.contains("[X]")).expect("the picker") - 1;
        let bottom = (top + 1..rows.len()).find(|&y| matches!(rows[y].chars().nth(12), Some('╰' | '+'))).expect("its bottom");
        rows[top..=bottom].iter().map(|r| r.chars().skip(12).take(76).collect()).collect()
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

    /// The picker's frame around `lines`, 76 wide.
    fn boxed(lines: &[&str]) -> Vec<String> {
        let mut out = vec![format!("╭{}╮", "─".repeat(74))];
        out.extend(lines.iter().map(|l| format!("│{l:<74}│")));
        out.push(format!("╰{}╯", "─".repeat(74)));
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

    /// The picker on the saved place (TermDestination F2).
    const F2: [&str; 16] = [
        " Where files land                                                      [X]",
        "",
        " LIBRARY   ◂ copies ▸   only libraries you may upload to",
        " FOLDER    library root",
        "           inside copies · leave it empty for the library root",
        "",
        " LAYOUT    (•) Album artist / Album",
        "           ( ) Artist / Album",
        "           ( ) Server / Album artist / Album",
        "           ( ) Custom  {{ALBUMARTIST}}/{{ALBUM}}",
        "",
        " PREVIEW   copies › Boukmanflow › Borrowed Tapes",
        "           Late Train Verse.mp3",
        "",
        " Applies to every copy and download on your account",
        "                          [ Use the library default ] [ Cancel ] [ Save ]",
    ];

    #[test]
    fn change_opens_where_files_land_on_the_saved_place_as_the_board_draws_it() {
        let _en = english();
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        // Opening read the destination again: the answer redraws LANDS IN.
        land_read(&mut gui, dest_view("", "user"));
        let low = lower_rows(&draw(&mut gui, 100, 30));
        assert_eq!(low[6], "│           saved for your account                   [ Change… ] │", "{}", low.join("\n"));
        // The hand walks the radios, the checkbox, then Change… (the slab, BOLD).
        for _ in 0..4 {
            key(&mut gui, KeyCode::Down);
        }
        assert_eq!(gui.discover.keep.focus, Some(super::Stop::Change));
        let buffer = buffer(&mut gui, 100, 30);
        let change = at(&lines(&buffer), "[ Change… ]");
        assert_eq!((bg(&buffer, change), bold(&buffer, change)), (th().accent, true));
        key(&mut gui, KeyCode::Enter);
        land_folders(&mut gui);
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(picker_rows(&rows), boxed(&F2), "{}", rows.join("\n"));
        // 76 wide, centred over the whole frame, its top on row 4 (as if 22
        // rows); Keep a copy is not drawn while it is open.
        assert_eq!((rows[4].chars().nth(12), rows[4].chars().nth(87)), (Some('╭'), Some('╮')));
        assert!(!rows.iter().any(|r| r.contains("Keep a copy")), "{}", rows.join("\n"));
        // The tokens: the title ACCENT BOLD, [X] DIM, the hand's label ACCENT
        // BOLD, the library BOLD between DIM arrows, the chosen radio OK and
        // BOLD, Custom's layout DIM, the quiet buttons DIM, Save ACCENT BOLD.
        let cell = |text: &str| at(&rows, text);
        assert_eq!((fg(&buffer, cell("Where files land")), bold(&buffer, cell("Where files land"))), (th().accent, true));
        assert_eq!(fg(&buffer, Position { x: 84, y: 5 }), th().dim);
        assert_eq!((fg(&buffer, cell("LIBRARY   ◂")), bold(&buffer, cell("LIBRARY   ◂"))), (th().accent, true));
        assert_eq!(fg(&buffer, cell("◂ copies")), th().dim);
        assert!(bold(&buffer, cell("copies ▸")));
        assert_eq!(fg(&buffer, cell("only libraries")), th().dim);
        assert_eq!(fg(&buffer, cell("FOLDER")), th().dim);
        assert_eq!(fg(&buffer, cell("library root")), th().dim);
        assert_eq!(fg(&buffer, cell("(•)")), th().ok);
        assert!(bold(&buffer, cell("Album artist / Album")));
        assert_eq!(fg(&buffer, cell("{{ALBUMARTIST}}/{{ALBUM}}")), th().dim);
        assert!(bold(&buffer, cell("copies › Boukmanflow")));
        assert_eq!(fg(&buffer, cell("Late Train Verse.mp3")), th().dim);
        assert_eq!(fg(&buffer, cell("Applies")), th().dim);
        assert_eq!(fg(&buffer, cell("[ Use the library default ]")), th().dim);
        assert_eq!((fg(&buffer, cell("[ Save ]")), bold(&buffer, cell("[ Save ]"))), (th().accent, true));
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · ← → library · Enter save · Esc back");
        // The pointer: the arrows and the name step, [X] goes back; the
        // room beneath is inert.
        let hit = |x: u16, y: u16| gui.ui.hit(Position { x, y });
        let dest = |act: super::DestAct| Some(Act::Disc(DiscAct::Keep(super::KeepAct::Dest(act))));
        assert_eq!(hit(24, 7), dest(super::DestAct::Step(-1)));
        assert_eq!(hit(27, 7), dest(super::DestAct::Step(1)));
        assert_eq!(hit(33, 7), dest(super::DestAct::Step(1)));
        assert_eq!(hit(85, 5), dest(super::DestAct::Back));
        assert_eq!(hit(28, 12), dest(super::DestAct::Pick(super::Radio::Artist)));
        assert_eq!(hit(84, 20), dest(super::DestAct::Save));
        assert_eq!(hit(5, 25), Some(Act::Guard));
        // Esc goes back, nothing saved, the hand on Change….
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.keep.dest.is_none() && saves(&gui).is_empty());
        assert_eq!(gui.discover.keep.focus, Some(super::Stop::Change));
        assert!(lower_rows(&draw(&mut gui, 100, 30)).iter().any(|r| r.contains("saved for your account")));
        // `d` opens it from the choice too.
        key(&mut gui, KeyCode::Char('d'));
        assert!(gui.discover.keep.dest.is_some());
    }

    #[test]
    fn the_library_steps_its_template_joins_the_presets_and_the_default_sends_null() {
        let _en = english();
        let mut gui = dest_gui("");
        // ← to music: its own template, no preset, joins the radios (P1).
        key(&mut gui, KeyCode::Left);
        land_folders(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(
            picker_rows(&rows),
            boxed(&[
                " Where files land                                                      [X]",
                "",
                " LIBRARY   ◂ music ▸   only libraries you may upload to",
                " FOLDER    library root",
                "           inside music · leave it empty for the library root",
                "",
                " LAYOUT    (•) Album artist / Album",
                "           ( ) Artist / Album",
                "           ( ) Server / Album artist / Album",
                "           ( ) music's template",
                "           ( ) Custom  {{ALBUMARTIST}}/{{ALBUM}}",
                "",
                " PREVIEW   music › Boukmanflow › Borrowed Tapes",
                "           Late Train Verse.mp3",
                "",
                " Applies to every copy and download on your account",
                "                          [ Use the library default ] [ Cancel ] [ Save ]",
            ])
        );
        // Use the library default refills the fields only (P2): the button
        // goes, the hand moves to LIBRARY, nothing is saved yet.
        dest_act(&mut gui, super::DestAct::UseDefault);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[10], format!("│{:<74}│", "           (•) music's template"));
        assert_eq!(picker[11], format!("│{:<74}│", "           ( ) Custom  {{ARTIST}}/{{YEAR}} - {{ALBUM}}"));
        assert_eq!(picker[13], format!("│{:<74}│", " PREVIEW   music › Boukmanflow › 2019 - Borrowed Tapes"));
        assert_eq!(picker[17], format!("│{:<74}│", "                                                      [ Cancel ] [ Save ]"));
        assert!(saves(&gui).is_empty());
        // Save sends null: back to the library default, nothing kept.
        key(&mut gui, KeyCode::Enter);
        assert_eq!(saves(&gui), vec![None]);
        // A preset picked after it: an explicit place.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Left);
        dest_act(&mut gui, super::DestAct::UseDefault);
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Artist));
        dest_act(&mut gui, super::DestAct::Save);
        let sent = saves(&gui);
        assert_eq!(sent.len(), 1);
        let place = sent[0].clone().expect("a place");
        assert_eq!((place.vpath.as_str(), place.base.as_str(), place.layout.as_str()), ("music", "", "{{ARTIST}}/{{ALBUM}}"));
    }

    /// F3: Custom typed, the save refused in the server's words.
    const F3: [&str; 19] = [
        " Where files land                                                      [X]",
        "",
        " LIBRARY   ◂ copies ▸   only libraries you may upload to",
        " FOLDER    library root",
        "           inside copies · leave it empty for the library root",
        "",
        " LAYOUT    ( ) Album artist / Album",
        "           ( ) Artist / Album",
        "           ( ) Server / Album artist / Album",
        "           (•) Custom  {{ARTST}}/{{YEAR}} - {{ALBUM}}▏",
        "                       ARTIST · ALBUM · YEAR · GENRE · ALBUMARTIST · PEER",
        "",
        " PREVIEW   copies › {{ARTST}} › 2019 - Borrowed Tapes",
        "           Late Train Verse.mp3",
        "",
        " couldn't save: destination layout: Unknown variable {{ARTST}}.",
        " Supported: {{ARTIST}}, {{ALBUM}}, {{YEAR}}, {{GENRE}}, {{ALBUMARTIST}},",
        " {{PEER}}",
        "                          [ Use the library default ] [ Cancel ] [ Save ]",
    ];

    #[test]
    fn a_refused_save_keeps_the_fields_and_says_the_servers_words_in_gold() {
        let _en = english();
        let mut gui = dest_gui("");
        // Custom: the hand goes into its field; the variables stand under it.
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        ctrl(&mut gui, 'u');
        typed(&mut gui, "{{ARTST}}/{{YEAR}} - {{ALBUM}}");
        assert_eq!(super::super::discover::tips(&gui), "type a layout · ↑ ↓ rows · Enter save · Esc back");
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        // The variable the server would refuse is drawn as typed, GOLD.
        let refused = at(&rows, "{{ARTST}} › 2019");
        assert_eq!(fg(&buffer, refused), th().gold);
        assert!(fg(&buffer, at(&rows, "2019 - Borrowed")) != th().gold);
        drop(buffer);
        // Enter saves the place as typed; "saving…", no buttons, the hand down.
        key(&mut gui, KeyCode::Enter);
        let sent = saves(&gui);
        let place = sent[0].clone().expect("a place");
        assert_eq!((place.vpath.as_str(), place.base.as_str(), place.layout.as_str()), ("copies", "", "{{ARTST}}/{{YEAR}} - {{ALBUM}}"));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[picker.len() - 2], format!("│{:>73} │", "saving…"));
        assert_eq!(super::super::discover::tips(&gui), "Esc back");
        assert!(gui.discover.keep.dest.as_ref().is_some_and(|ui| ui.hand.is_none()));
        let words = "destination layout: Unknown variable {{ARTST}}. Supported: {{ARTIST}}, {{ALBUM}}, {{YEAR}}, {{GENRE}}, {{ALBUMARTIST}}, {{PEER}}";
        land_save(&mut gui, Err(crate::api::PluginFailure::Refused { status: 400, words: words.into() }));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(picker_rows(&rows), boxed(&F3), "{}", rows.join("\n"));
        assert_eq!(fg(&buffer, at(&rows, "couldn't save")), th().gold);
        assert_eq!(fg(&buffer, at(&rows, "ARTIST · ALBUM")), th().dim);
        // The hand is back in the field; any edit clears the words.
        key(&mut gui, KeyCode::Backspace);
        let rows = draw(&mut gui, 100, 30);
        assert!(!rows.iter().any(|r| r.contains("couldn't save")));
        assert!(rows.iter().any(|r| r.contains("Applies to every copy")));
        // At the 100 × 24 floor the tallest common state stands whole, rows 1–21 (FL).
        typed(&mut gui, "}");
        dest_act(&mut gui, super::DestAct::Save);
        land_save(&mut gui, Err(crate::api::PluginFailure::Refused { status: 400, words: words.into() }));
        let rows = draw(&mut gui, 100, 24);
        assert_eq!(rows[1].chars().nth(12), Some('╭'), "{}", rows.join("\n"));
        assert_eq!(rows[21].chars().nth(12), Some('╰'), "{}", rows.join("\n"));
        assert_eq!(picker_rows(&rows), boxed(&F3));
    }

    #[test]
    fn same_folders_as_the_server_is_a_preset_only_where_the_server_lists_its_variable() {
        let _en = english();
        // A server that lists SERVERFOLDER: the preset after the Server one,
        // before the template and Custom.
        let mut gui = dest_gui_on(dest_view_folders("", "user"));
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(
            picker_rows(&rows),
            boxed(&[
                " Where files land                                                      [X]",
                "",
                " LIBRARY   ◂ copies ▸   only libraries you may upload to",
                " FOLDER    library root",
                "           inside copies · leave it empty for the library root",
                "",
                " LAYOUT    (•) Album artist / Album",
                "           ( ) Artist / Album",
                "           ( ) Server / Album artist / Album",
                "           ( ) Same folders as the server",
                "           ( ) Custom  {{ALBUMARTIST}}/{{ALBUM}}",
                "",
                " PREVIEW   copies › Boukmanflow › Borrowed Tapes",
                "           Late Train Verse.mp3",
                "",
                " Applies to every copy and download on your account",
                "                          [ Use the library default ] [ Cancel ] [ Save ]",
            ]),
            "{}",
            rows.join("\n")
        );
        // ↓ walks onto it after the Server preset; Enter picks it and the
        // layout is the variable alone; a click picks it too.
        for _ in 0..5 {
            key(&mut gui, KeyCode::Down);
        }
        assert_eq!(gui.discover.keep.dest.as_ref().and_then(|ui| ui.hand), Some(super::DestStop::Radio(super::Radio::ServerFolder)));
        key(&mut gui, KeyCode::Enter);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[10], format!("│{:<74}│", "           (•) Same folders as the server"));
        assert_eq!(picker[11], format!("│{:<74}│", "           ( ) Custom  {{SERVERFOLDER}}"));
        // The song's own folders on the peer, under the library.
        assert_eq!(picker[13], format!("│{:<74}│", " PREVIEW   copies › Boukmanflow › Borrowed Tapes"));
        let radio = at(&rows, "Same folders as the server");
        assert_eq!(gui.ui.hit(radio), Some(Act::Disc(DiscAct::Keep(super::KeepAct::Dest(super::DestAct::Pick(super::Radio::ServerFolder))))));
        dest_act(&mut gui, super::DestAct::Save);
        let place = saves(&gui)[0].clone().expect("a place");
        assert_eq!((place.vpath.as_str(), place.base.as_str(), place.layout.as_str()), ("copies", "", "{{SERVERFOLDER}}"));
        // Inside a folder's words the server's folders follow them: the
        // variables row inserts it at the caret.
        let mut gui = dest_gui_on(dest_view_folders("", "user"));
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        ctrl(&mut gui, 'u');
        typed(&mut gui, "Copy of ");
        dest_act(&mut gui, super::DestAct::Var(6));
        assert_eq!(gui.discover.keep.dest.as_ref().unwrap().layout.value(), "Copy of {{SERVERFOLDER}}");
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("PREVIEW   copies › Copy of Boukmanflow › Borrowed Tapes")), "{}", rows.join("\n"));
        // A server from before it offers no such preset, and draws the
        // variable typed by hand as it draws any it would refuse: GOLD.
        let mut gui = dest_gui("");
        let rows = draw(&mut gui, 100, 30);
        assert!(!rows.iter().any(|r| r.contains("Same folders")), "{}", rows.join("\n"));
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        ctrl(&mut gui, 'u');
        typed(&mut gui, "{{SERVERFOLDER}}");
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let typed_at = at(&rows, "copies › {{SERVERFOLDER}}");
        assert_eq!(fg(&buffer, Position { x: typed_at.x + 9, ..typed_at }), th().gold, "{}", rows.join("\n"));
    }

    #[test]
    fn the_tallest_picker_gives_the_blank_above_preview_way_at_the_floor() {
        let _en = english();
        // music's template, the fourth preset, Custom's variables and a
        // three-row refusal: one row past 22, so the blank above PREVIEW
        // gives way and the picker stands whole between the tab bar and the
        // tips line.
        let mut gui = dest_gui_on(dest_view_folders("", "user"));
        key(&mut gui, KeyCode::Left);
        land_folders(&mut gui);
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        ctrl(&mut gui, 'u');
        typed(&mut gui, "{{ARTST}}/{{SERVERFOLDER}}");
        dest_act(&mut gui, super::DestAct::Save);
        let words = "destination layout: Unknown variable {{ARTST}}. Supported: {{ARTIST}}, {{ALBUM}}, {{YEAR}}, {{GENRE}}, {{ALBUMARTIST}}, {{PEER}}, {{SERVERFOLDER}}";
        land_save(&mut gui, Err(crate::api::PluginFailure::Refused { status: 400, words: words.into() }));
        let tallest = boxed(&[
            " Where files land                                                      [X]",
            "",
            " LIBRARY   ◂ music ▸   only libraries you may upload to",
            " FOLDER    library root",
            "           inside music · leave it empty for the library root",
            "",
            " LAYOUT    ( ) Album artist / Album",
            "           ( ) Artist / Album",
            "           ( ) Server / Album artist / Album",
            "           ( ) Same folders as the server",
            "           ( ) music's template",
            "           (•) Custom  {{ARTST}}/{{SERVERFOLDER}}▏",
            "        ARTIST · ALBUM · YEAR · GENRE · ALBUMARTIST · PEER · SERVERFOLDER",
            " PREVIEW   music › {{ARTST}} › Boukmanflow › Borrowed Tapes",
            "           Late Train Verse.mp3",
            "",
            " couldn't save: destination layout: Unknown variable {{ARTST}}.",
            " Supported: {{ARTIST}}, {{ALBUM}}, {{YEAR}}, {{GENRE}}, {{ALBUMARTIST}},",
            " {{PEER}}, {{SERVERFOLDER}}",
            "                          [ Use the library default ] [ Cancel ] [ Save ]",
        ]);
        let rows = draw(&mut gui, 100, 24);
        assert_eq!(rows[1].chars().nth(12), Some('╭'), "{}", rows.join("\n"));
        assert_eq!(rows[22].chars().nth(12), Some('╰'), "{}", rows.join("\n"));
        assert_eq!(picker_rows(&rows), tallest, "{}", rows.join("\n"));
        // At 100 × 30 the same rows, the top where it always is.
        let rows = draw(&mut gui, 100, 30);
        assert_eq!(rows[4].chars().nth(12), Some('╭'));
        assert_eq!(picker_rows(&rows), tallest);
        // An edit clears the refusal: room again, the blank comes back.
        key(&mut gui, KeyCode::Backspace);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        let preview = picker.iter().position(|r| r.contains("PREVIEW")).expect("PREVIEW");
        assert_eq!(picker[preview - 1], format!("│{:<74}│", ""), "{}", picker.join("\n"));
        assert_eq!(picker.len(), 21);
    }

    #[test]
    fn a_picker_exactly_22_tall_keeps_the_blank_above_preview() {
        let _en = english();
        let blank_above_preview = |rows: &[String]| {
            let picker = picker_rows(rows);
            let preview = picker.iter().position(|r| r.contains("PREVIEW")).expect("PREVIEW");
            assert_eq!(picker.len(), 22, "{}", picker.join("\n"));
            assert_eq!(picker[preview - 1], format!("│{:<74}│", ""), "{}", picker.join("\n"));
        };
        // The fourth preset and the folder list open with three folders:
        // 22 rows with the blank, the floor's whole height — it stays.
        let mut gui = dest_gui_on(dest_view_folders("", "user"));
        key(&mut gui, KeyCode::Down);
        typed(&mut gui, "from-peers/");
        land_folders(&mut gui);
        key(&mut gui, KeyCode::Tab);
        land_folders(&mut gui);
        let rows = draw(&mut gui, 100, 24);
        assert_eq!(rows[1].chars().nth(12), Some('╭'), "{}", rows.join("\n"));
        assert_eq!(rows[22].chars().nth(12), Some('╰'), "{}", rows.join("\n"));
        blank_above_preview(&rows);
        blank_above_preview(&draw(&mut gui, 100, 30));
        // A server from before it: music's template, Custom's variables and
        // a three-row refusal — the same 22, as tall as before the preset.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Left);
        land_folders(&mut gui);
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        ctrl(&mut gui, 'u');
        typed(&mut gui, "{{ARTST}}/{{ALBUM}}");
        dest_act(&mut gui, super::DestAct::Save);
        let words = "destination layout: Unknown variable {{ARTST}}. Supported: {{ARTIST}}, {{ALBUM}}, {{YEAR}}, {{GENRE}}, {{ALBUMARTIST}}, {{PEER}}";
        land_save(&mut gui, Err(crate::api::PluginFailure::Refused { status: 400, words: words.into() }));
        let rows = draw(&mut gui, 100, 24);
        assert!(rows.iter().any(|r| r.contains(" {{PEER}}") && !r.contains("ALBUMARTIST")), "a three-row refusal: {}", rows.join("\n"));
        blank_above_preview(&rows);
    }

    #[test]
    fn folder_completion_lists_the_librarys_own_folders_one_ask_at_a_time() {
        let _en = english();
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Down);
        assert_eq!(super::super::discover::tips(&gui), "type a folder · Tab folders · ↑ ↓ rows · Enter save · Esc back");
        typed(&mut gui, "from-peers/");
        land_folders(&mut gui);
        // Tab lists the folder typed so far, asked of the session's own
        // server once (its parent's listing told that it is there).
        key(&mut gui, KeyCode::Tab);
        let asked: Vec<String> = gui
            .pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::DestinationFolders { path, .. }) => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(asked, vec!["/copies/from-peers".to_string()]);
        land_folders(&mut gui);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(
            picker_rows(&rows),
            boxed(&[
                " Where files land                                                      [X]",
                "",
                " LIBRARY   ◂ copies ▸   only libraries you may upload to",
                " FOLDER    from-peers/▏",
                "           2025 archive",
                "           Stage Peer B",
                "           Stage Peer C",
                "           in copies › from-peers · 3 folders",
                "",
                " LAYOUT    (•) Album artist / Album",
                "           ( ) Artist / Album",
                "           ( ) Server / Album artist / Album",
                "           ( ) Custom  {{ALBUMARTIST}}/{{ALBUM}}",
                "",
                " PREVIEW   copies › from-peers › Boukmanflow › Borrowed Tapes",
                "           Late Train Verse.mp3",
                "",
                " Applies to every copy and download on your account",
                "                          [ Use the library default ] [ Cancel ] [ Save ]",
            ]),
            "{}",
            rows.join("\n")
        );
        // The held folder wears the slab across cols 11–73 (not bold); the others DIM.
        let held = at(&rows, "Stage Peer B");
        assert_eq!((bg(&buffer, held), bold(&buffer, held)), (th().accent, false));
        assert_eq!(bg(&buffer, Position { x: 23, y: held.y }), th().accent);
        assert_eq!(fg(&buffer, at(&rows, "Stage Peer C")), th().dim);
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ folders · Enter open · Tab complete · Esc close the list");
        // Enter goes in: the folder filled, its own listed.
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.discover.keep.dest.as_ref().unwrap().folder.value(), "from-peers/Stage Peer B/");
        land_folders(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("in copies › from-peers › Stage Peer B · 0 folders")), "{}", rows.join("\n"));
        // The typed tail filters, Tab extends it to the names' common start,
        // then cycles; Esc closes the list, a second Esc goes back.
        ctrl(&mut gui, 'u');
        typed(&mut gui, "from-peers/st");
        key(&mut gui, KeyCode::Tab);
        assert_eq!(gui.discover.keep.dest.as_ref().unwrap().folder.value(), "from-peers/Stage Peer ");
        key(&mut gui, KeyCode::Tab);
        assert_eq!(gui.discover.keep.dest.as_ref().unwrap().list.as_ref().and_then(|l| l.held), Some(0));
        key(&mut gui, KeyCode::BackTab);
        assert_eq!(gui.discover.keep.dest.as_ref().unwrap().list.as_ref().and_then(|l| l.held), Some(1));
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.keep.dest.as_ref().is_some_and(|ui| ui.list.is_none()));
        // A click on a folder goes in, as Enter does.
        ctrl(&mut gui, 'u');
        key(&mut gui, KeyCode::Tab);
        land_folders(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        let folder = at(&rows, "           from-peers");
        gui.act(gui.ui.hit(Position { x: folder.x + 11, y: folder.y }).expect("the folder"));
        assert_eq!(gui.discover.keep.dest.as_ref().unwrap().folder.value(), "from-peers/");
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.keep.dest.is_none());
    }

    #[test]
    fn one_library_starts_on_the_folder_and_a_new_folder_is_not_there_yet() {
        let _en = english();
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        let mut view = dest_view("", "user");
        view["libraries"] = json!([{ "vpath": "copies", "template": null }]);
        land_read(&mut gui, view);
        key(&mut gui, KeyCode::Char('d'));
        assert_eq!(gui.discover.keep.dest.as_ref().unwrap().hand, Some(super::DestStop::Folder), "no stepper to start on");
        typed(&mut gui, "from-peers/2026");
        land_folders(&mut gui);
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker.len(), 18, "{}", picker.join("\n"));
        assert_eq!(picker[3], format!("│{:<74}│", " LIBRARY   copies   your only library"));
        assert_eq!(picker[4], format!("│{:<74}│", " FOLDER    from-peers/2026▏"));
        assert_eq!(picker[5], format!("│{:<74}│", "           not there yet · the first file creates it"));
        assert_eq!(picker[12], format!("│{:<74}│", " PREVIEW   copies › from-peers › 2026 › Boukmanflow › Borrowed Tapes"));
        // A base the layout repeats: files would land in X › X, GOLD.
        ctrl(&mut gui, 'u');
        typed(&mut gui, "boukmanflow");
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let nested = at(&rows, "files would land in boukmanflow › boukmanflow");
        assert_eq!(fg(&buffer, nested), th().gold);
    }

    #[test]
    fn a_saved_place_lands_back_in_keep_a_copy_with_the_hand_on_change() {
        let _en = english();
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Down);
        typed(&mut gui, "from-peers");
        key(&mut gui, KeyCode::Enter);
        let place = saves(&gui)[0].clone().expect("a place");
        assert_eq!((place.vpath.as_str(), place.base.as_str()), ("copies", "from-peers"));
        // Saving (F5a): the field without its caret, no buttons, the frame's height kept.
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker.len(), 18);
        assert_eq!(picker[4], format!("│{:<74}│", " FOLDER    from-peers"));
        assert_eq!(picker[16], format!("│{:>73} │", "saving…"));
        // Saved (F5b): Keep a copy shows the new place, saved for the account.
        land_save(&mut gui, Ok(dest_view("from-peers", "user")));
        let buffer = buffer(&mut gui, 100, 30);
        let low = lower_rows(&lines(&buffer));
        assert_eq!(low[5], "│ LANDS IN  copies › from-peers › Boukmanflow › Borrowed Tapes   │");
        assert_eq!(low[6], "│           saved for your account                   [ Change… ] │");
        assert!(gui.discover.keep.dest.is_none());
        assert_eq!(gui.discover.keep.focus, Some(super::Stop::Change));
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · Space or Enter pick · Enter on Add song starts · d where files land · Esc close");
    }

    #[test]
    fn a_start_sends_the_place_lands_in_drew_and_a_changed_one_starts_nothing() {
        let _en = english();
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        land_read(&mut gui, dest_view("", "user"));
        draw(&mut gui, 100, 30);
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Primary)));
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
        // 409 destination_changed (F7): LANDS IN redrawn from the answer, one
        // GOLD line, nothing started; the view is read again.
        gui.pending.clear();
        let music: crate::api::types::CollectionDestination =
            serde_json::from_value(json!({ "vpath": "music", "base": "", "layout": "{{ARTIST}}/{{YEAR}} - {{ALBUM}}", "source": "default" })).unwrap();
        let generation = gui.app.session_gen();
        apply(&mut gui, Event::JobStarted { ticket, generation, answer: Err(crate::api::PluginFailure::DestinationChanged(Some(Some(music)))) });
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::DestinationRead { .. }))), "read again");
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(
            lower_rows(&rows)[4..],
            canvas(&[
                "│                                                                │",
                "│ LANDS IN  music › Boukmanflow › 2019 - Borrowed Tapes          │",
                "│           the library's default                    [ Change… ] │",
                "│ where files land changed · check the path above and add again  │",
                "│                                        [ Cancel ] [ Add song ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ])[..]
        );
        assert_eq!(fg(&buffer, at(&rows, "where files land changed")), th().gold);
        let song = at(&rows, "This song");
        assert_eq!((fg(&buffer, song), bold(&buffer, song)), (th().accent, true), "the hand back on the chosen radio");
        assert!(matches!(gui.app.keep_view().unwrap().stage, crate::tui::app::keep::KeepStage::Refused { .. }));
        // Add again sends the place now drawn.
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Primary)));
        let again = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::JobStart { destination, .. }) => destination.clone().flatten(),
            _ => None,
        });
        assert_eq!(again.map(|d| d.vpath), Some("music".into()));
    }

    #[test]
    fn nowhere_to_put_a_file_leaves_one_plain_line_and_close() {
        let _en = english();
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        land_read(&mut gui, json!({ "destination": null, "libraries": [] }));
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let modal = modal_rows(&rows);
        assert_eq!(modal.len(), 11, "{}", modal.join("\n"));
        assert_eq!(rows[6].chars().nth(17), Some('╭'), "the top holds");
        assert_eq!(
            modal[7..].to_vec(),
            canvas(&[
                "│ your account can't add files to this server · copies are off   │",
                "│                                                                │",
                "│                                                      [ Close ] │",
                "╰────────────────────────────────────────────────────────────────╯",
            ])
        );
        let close = at(&rows, "[ Close ]");
        assert_eq!((fg(&buffer, close), bold(&buffer, close)), (th().accent, true));
        assert_eq!(super::super::discover::tips(&gui), "Enter close · Esc close");
        assert!(!gui.app.keep_gate(), "the copy gate closed: every way in hidden");
        key(&mut gui, KeyCode::Char('d'));
        assert!(gui.discover.keep.dest.is_none(), "nowhere to change");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.app.taste.keep.flow.is_none());
    }

    #[test]
    fn the_legacy_console_draws_where_files_land_in_its_own_glyphs() {
        let _en = english();
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        let mut gui = dest_gui("");
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(picker[0], format!("+{}+", "-".repeat(74)));
        assert_eq!(picker[1], format!("|{:<74}|", " Where files land                                                      [X]"));
        assert_eq!(picker[3], format!("|{:<74}|", " LIBRARY   < copies >   only libraries you may upload to"));
        assert_eq!(picker[7], format!("|{:<74}|", " LAYOUT    (*) Album artist / Album"));
        assert_eq!(picker[12], format!("|{:<74}|", " PREVIEW   copies > Boukmanflow > Borrowed Tapes"));
        assert_eq!(picker[17], format!("+{}+", "-".repeat(74)));
        // The caret and saving's dots in the console's own.
        key(&mut gui, KeyCode::Down);
        typed(&mut gui, "x");
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("| FOLDER    x│")), "{}", rows.join("\n"));
        key(&mut gui, KeyCode::Enter);
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("saving... |")), "{}", rows.join("\n"));
    }

    #[test]
    fn from_a_tile_every_album_by_previews_one_folder_per_album_and_no_file() {
        let _en = english();
        let mut gui = canvas_gui(&[]);
        draw(&mut gui, 100, 30);
        super::open(&mut gui, KeepTarget::Tile(0));
        stage_facts(&mut gui);
        land_read(&mut gui, dest_view("", "user"));
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Pick(crate::tui::app::keep::KeepChoice::Artist))));
        key(&mut gui, KeyCode::Char('d'));
        land_folders(&mut gui);
        dest_act(&mut gui, super::DestAct::Save);
        land_save(&mut gui, Err(crate::api::PluginFailure::Unreachable("connection refused".into())));
        let rows = draw(&mut gui, 100, 30);
        let picker = picker_rows(&rows);
        assert_eq!(
            picker[11..].to_vec(),
            boxed(&[
                "",
                " PREVIEW   copies › Boukmanflow › one folder per album",
                "",
                "",
                " couldn't save: your server didn't answer",
                "                          [ Use the library default ] [ Cancel ] [ Save ]",
            ])[1..]
                .to_vec(),
            "{}",
            picker.join("\n")
        );
    }

    #[test]
    fn a_variable_clicked_goes_in_at_the_caret_and_the_wheel_scrolls_the_folders() {
        let _en = english();
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
        // The caret after {{ALBUMARTIST}}/: Home, then 16 steps right.
        key(&mut gui, KeyCode::Home);
        for _ in 0..16 {
            key(&mut gui, KeyCode::Right);
        }
        let rows = draw(&mut gui, 100, 30);
        let year = at(&rows, "YEAR ·");
        gui.act(gui.ui.hit(year).expect("the variable"));
        let ui = gui.discover.keep.dest.as_ref().unwrap();
        assert_eq!(ui.layout.value(), "{{ALBUMARTIST}}/{{YEAR}}{{ALBUM}}");
        assert_eq!(ui.hand, Some(super::DestStop::Layout));
        // More folders than the list shows: its bar, and the wheel steps them.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Down);
        typed(&mut gui, "from-peers/");
        key(&mut gui, KeyCode::Tab);
        land_folders_with(&mut gui, Some(6));
        let rows = draw(&mut gui, 100, 30);
        assert!(rows[9].contains("folder 1") && rows[9].chars().nth(86) == Some('▲'), "{}", rows.join("\n"));
        assert!(rows[11].contains("folder 3") && rows[11].chars().nth(86) == Some('▼'));
        assert!(rows[12].contains("in copies › from-peers · 6 folders"), "{}", rows.join("\n"));
        super::super::discover::wheel(&mut gui, 2);
        let rows = draw(&mut gui, 100, 30);
        assert!(rows[9].contains("folder 3"), "{}", rows.join("\n"));
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

    #[test]
    fn lands_in_wraps_a_long_place_at_a_crumb_the_library_always_showing() {
        let _en = english();
        // The Server preset under a base folder of its own: 80 cells of path
        // in a row of 52 — the second row takes the rest, ` ›` ending the first.
        let mut view = dest_view("from-peers/2025 archive", "user");
        view["destination"]["layout"] = json!("{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}");
        let mut gui = keep_on(view.clone());
        let rows = draw(&mut gui, 100, 30);
        let low = lower_rows(&rows);
        let at = low.iter().position(|r| r.contains("LANDS IN")).expect("LANDS IN");
        assert_eq!(
            low[at..at + 3].to_vec(),
            canvas(&[
                "│ LANDS IN  copies › from-peers › 2025 archive › Stage Peer B ›  │",
                "│           Boukmanflow › Borrowed Tapes                         │",
                "│           saved for your account                   [ Change… ] │",
            ]),
            "{}",
            low.join("\n")
        );
        assert!(!gui.ui.tips.iter().any(|(_, tip)| tip.contains("2025 archive")), "nothing cut: no tooltip");
        // Longer still: the second row is clipped at the right — never the
        // library — and the whole path is the rows' tooltip.
        view["destination"]["base"] = json!("from-peers/2025 archive/the long weekend sessions");
        super::close(&mut gui);
        super::open(&mut gui, LATE);
        stage_facts(&mut gui);
        land_read(&mut gui, view);
        let rows = draw(&mut gui, 100, 30);
        let low = lower_rows(&rows);
        let at = low.iter().position(|r| r.contains("LANDS IN")).expect("LANDS IN");
        assert_eq!(low[at], "│ LANDS IN  copies › from-peers › 2025 archive ›                 │");
        assert_eq!(low[at + 1], "│           the long weekend sessions › Stage Peer B › Boukmanf… │");
        let row = at_row(&rows, "the long weekend sessions");
        let whole = "copies/from-peers/2025 archive/the long weekend sessions/Stage Peer B/Boukmanflow/Borrowed Tapes";
        for y in [row - 1, row] {
            assert!(gui.ui.tips.iter().any(|(rect, tip)| rect.contains(Position { x: 40, y }) && tip == whole), "row {y}: {:?}", gui.ui.tips);
        }
    }

    #[test]
    fn a_clipped_path_with_a_slot_tips_the_whole_path_then_its_note() {
        let _en = english();
        // A long base folder and a genre in the layout: the second row is
        // clipped, so the tooltip is the whole path — the slot in its words —
        // and then the note, on LANDS IN and on the PREVIEW alike.
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
            let at = low.iter().position(|r| r.contains("LANDS IN")).expect("LANDS IN");
            assert_eq!(
                low[at..at + 2].to_vec(),
                canvas(&[
                    "│ LANDS IN  copies › from-peers › 2025 archive ›                 │",
                    "│           the long weekend sessions › Stage Peer B › ‹genre› … │",
                ]),
                "{}",
                low.join("\n")
            );
            let tip = format!("{whole} — {note}");
            let row = at_row(&rows, "the long weekend sessions");
            for y in [row - 1, row] {
                assert!(gui.ui.tips.iter().any(|(rect, t)| rect.contains(Position { x: 40, y }) && *t == tip), "row {y}: {:?}", gui.ui.tips);
            }
            key(&mut gui, KeyCode::Char('d'));
            land_folders(&mut gui);
            let rows = draw(&mut gui, 100, 30);
            let y = at_row(&rows, "PREVIEW");
            assert!(rows[usize::from(y)].contains('…'), "the PREVIEW clipped: {}", rows[usize::from(y)]);
            assert!(gui.ui.tips.iter().any(|(rect, t)| rect.contains(Position { x: 40, y }) && *t == tip), "PREVIEW: {:?}", gui.ui.tips);
            key(&mut gui, KeyCode::Esc);
        }
    }

    /// The row a text stands on.
    fn at_row(rows: &[String], text: &str) -> u16 {
        at(rows, text).y
    }

    #[test]
    fn a_moved_place_says_where_retry_adds_before_the_press_and_retry_sends_it() {
        let _en = english();
        // A copy pinned to the library root failed; the account has saved
        // copies › from-peers since (another tab).
        let mut gui = row_in("failed", None, json!({ "error": "Stage Peer B is unreachable" }));
        land_read(&mut gui, dest_view("from-peers", "user"));
        let rows = draw(&mut gui, 100, 30);
        let low = lower_rows(&rows);
        assert_eq!(
            low[1..4].to_vec(),
            canvas(&[
                "│ LANDS IN  copies › Boukmanflow › Borrowed Tapes                │",
                "│           retry adds to copies › from-peers › Boukmanflow ›    │",
                "│           Borrowed Tapes                                       │",
            ]),
            "{}",
            low.join("\n")
        );
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        assert_eq!(fg(&buffer, at(&rows, "retry adds to")), th().dim);
        assert!(bold(&buffer, at(&rows, "copies › from-peers")), "the library BOLD");
        drop(buffer);
        // Retry sends the place it said — a stale one is the server's 409.
        gui.pending.clear();
        gui.act(Act::Disc(DiscAct::Keep(super::KeepAct::Primary)));
        let sent = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::JobStart { destination, .. }) => Some(destination.clone()),
            _ => None,
        });
        let sent = sent.expect("a retry").flatten().expect("a place");
        assert_eq!((sent.vpath.as_str(), sent.base.as_str()), ("copies", "from-peers"));
        // Starting: LANDS IN reads that place, and the line has said its piece.
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("LANDS IN  copies › from-peers › Boukmanflow")), "{}", rows.join("\n"));
        assert!(!rows.iter().any(|r| r.contains("retry adds to")));
        // Where it has not moved, nothing more is said.
        let mut gui = row_in("failed", None, json!({ "error": "Stage Peer B is unreachable" }));
        land_read(&mut gui, destination());
        let rows = draw(&mut gui, 100, 30);
        assert!(!rows.iter().any(|r| r.contains("adds to")), "{}", rows.join("\n"));
        // An album copy with gaps: Copy again says it in its own words.
        let mut gui = row_in("done", Some("album"), counts(2, 0, 1, None));
        land_read(&mut gui, dest_view("from-peers", "user"));
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("copy again adds to copies ›")), "{}", rows.join("\n"));
    }

    #[test]
    fn the_variables_row_draws_every_one_inside_the_border_in_every_locale() {
        let _en = english();
        struct English;
        impl Drop for English {
            fn drop(&mut self) {
                rust_i18n::set_locale("en");
            }
        }
        let _back = English;
        let six = ["ARTIST", "ALBUM", "YEAR", "GENRE", "ALBUMARTIST", "PEER"];
        let seven = ["ARTIST", "ALBUM", "YEAR", "GENRE", "ALBUMARTIST", "PEER", "SERVERFOLDER"];
        // The six, and the seven of a server that lists SERVERFOLDER: 65
        // cells, right-aligned to the last cell, never one lost.
        for (view, names) in [(dest_view("", "user"), &six[..]), (dest_view_folders("", "user"), &seven[..])] {
            for code in ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
                rust_i18n::set_locale(code);
                let mut gui = dest_gui_on(view.clone());
                dest_act(&mut gui, super::DestAct::Pick(super::Radio::Custom));
                let buffer = buffer(&mut gui, 100, 30);
                // The picker's content cells, cols 13–85 (its border at 12 and 87).
                let row = |y: u16| (13..86).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>();
                let y = (0..30).find(|&y| row(y).contains("ARTIST · ALBUM")).unwrap_or_else(|| panic!("{code}: no variables row"));
                let cells = row(y);
                for (i, name) in names.iter().enumerate() {
                    let token = if *name == "ALBUM" { "ALBUM ·" } else { name };
                    let byte = cells.find(token).unwrap_or_else(|| panic!("{code}: {name} missing from {cells:?}"));
                    let at = cells[..byte].chars().count() as u16 + 13;
                    let hit = gui.ui.hit(Position { x: at, y });
                    assert_eq!(hit, Some(Act::Disc(DiscAct::Keep(super::KeepAct::Dest(super::DestAct::Var(i))))), "{code}: {name} at {at}");
                    assert!(at + name.len() as u16 - 1 <= 85, "{code}: {name} past the last cell");
                }
                assert!(!cells.trim_end().ends_with('·'), "{code}: no stray separator: {cells:?}");
            }
        }
    }

    #[test]
    fn space_presses_the_pickers_buttons_as_enter_does() {
        let _en = english();
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Save));
        key(&mut gui, KeyCode::Char(' '));
        assert_eq!(saves(&gui).len(), 1, "Save on Space");
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::Cancel));
        key(&mut gui, KeyCode::Char(' '));
        assert!(gui.discover.keep.dest.is_none() && saves(&gui).is_empty(), "Cancel on Space");
        let mut gui = dest_gui("");
        dest_act(&mut gui, super::DestAct::Hand(super::DestStop::UseDefault));
        key(&mut gui, KeyCode::Char(' '));
        assert_eq!(gui.discover.keep.dest.as_ref().map(|ui| ui.vpath.clone()), Some("music".into()), "the default on Space");
    }

    #[test]
    fn with_one_library_its_label_is_no_stop_and_a_hand_whose_stop_left_lands_on_the_first() {
        let _en = english();
        let mut view = dest_view("", "user");
        view["libraries"] = json!([{ "vpath": "copies", "template": null }]);
        let mut gui = keep_on(view);
        key(&mut gui, KeyCode::Char('d'));
        let rows = draw(&mut gui, 100, 30);
        let label = at(&rows, "LIBRARY");
        assert_eq!(gui.ui.hit(label), Some(Act::Guard), "no stop to take the hand");
        // music's template radio held, then a read without it: the hand
        // goes to the first stop, and the tips say its keys.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Left);
        land_folders(&mut gui);
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Template));
        let mut plain = dest_view("", "user");
        plain["libraries"] = json!([{ "vpath": "music", "template": null }, { "vpath": "copies", "template": null }]);
        let generation = gui.app.session_gen();
        let seq = gui.app.keep_dest_read().iter().find_map(|e| match e {
            Effect::Api(ApiCmd::DestinationRead { seq, .. }) => Some(*seq),
            _ => None,
        });
        apply(&mut gui, Event::DestinationRead { seq: seq.unwrap(), generation, answer: Ok(Box::new(serde_json::from_value(plain).unwrap())) });
        draw(&mut gui, 100, 30);
        assert_eq!(gui.discover.keep.dest.as_ref().and_then(|ui| ui.hand), Some(super::DestStop::Library));
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · ← → library · Enter save · Esc back");
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

    #[test]
    fn use_the_library_default_stands_until_save_whatever_is_stepped_between() {
        let _en = english();
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Left);
        dest_act(&mut gui, super::DestAct::UseDefault);
        // Away to copies and back, and the same radio picked again: the
        // fields are the default, so Save still goes back to it.
        dest_act(&mut gui, super::DestAct::Step(1));
        dest_act(&mut gui, super::DestAct::Step(-1));
        dest_act(&mut gui, super::DestAct::Pick(super::Radio::Template));
        dest_act(&mut gui, super::DestAct::Save);
        assert_eq!(saves(&gui), vec![None], "null: back to the library default");
    }

    #[test]
    fn a_saves_answer_is_its_own_pickers_and_a_refusal_after_it_closed_is_a_note() {
        let _en = english();
        let refusal = || crate::api::PluginFailure::Refused { status: 400, words: "destination base folder: Resolved path cannot contain .. segments".into() };
        // Esc while saving, then `d` again: the new picker is no "saving…",
        // its keys work, and the old refusal is the status line's.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Down);
        typed(&mut gui, "../up");
        key(&mut gui, KeyCode::Enter);
        assert_eq!(saves(&gui).len(), 1);
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.keep.dest.is_none(), "Esc still goes back");
        key(&mut gui, KeyCode::Char('d'));
        let rows = draw(&mut gui, 100, 30);
        assert!(!rows.iter().any(|r| r.contains("saving")), "{}", rows.join("\n"));
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · ← → library · Enter save · Esc back");
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.discover.keep.dest.as_ref().and_then(|ui| ui.hand), Some(super::DestStop::Folder));
        land_save(&mut gui, Err(refusal()));
        let rows = draw(&mut gui, 100, 30);
        assert!(gui.discover.keep.dest.as_ref().is_some_and(|ui| ui.error.is_none()), "the new picker keeps its own fields");
        assert!(!picker_rows(&rows).iter().any(|r| r.contains("couldn't save")));
        // The status line's note, fitted to its slot.
        let note = gui.app.message.as_ref().map(|m| (m.text.clone(), m.kind.clone())).expect("a note");
        assert!(note.0.starts_with("couldn't save: destination"), "{note:?}");
        assert_eq!(note.1, crate::tui::app::MessageKind::Error);
        // Saved after its picker went: the new picker stays open, the place
        // is the answer's — Keep a copy reads it once the picker closes.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Down);
        typed(&mut gui, "from-peers");
        key(&mut gui, KeyCode::Enter);
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Char('d'));
        land_save(&mut gui, Ok(dest_view("from-peers", "user")));
        draw(&mut gui, 100, 30);
        assert!(gui.discover.keep.dest.is_some(), "never closed by another picker's answer");
        key(&mut gui, KeyCode::Esc);
        let rows = draw(&mut gui, 100, 30);
        assert!(rows.iter().any(|r| r.contains("LANDS IN  copies › from-peers ›")), "{}", rows.join("\n"));
        // Its own refusal comes back to it: the base folder's words GOLD,
        // wrapped at 72, and the hand back in the field.
        let mut gui = dest_gui("");
        key(&mut gui, KeyCode::Down);
        typed(&mut gui, "../up");
        key(&mut gui, KeyCode::Enter);
        land_save(&mut gui, Err(refusal()));
        let buffer = buffer(&mut gui, 100, 30);
        let rows = lines(&buffer);
        let picker = picker_rows(&rows);
        let n = picker.len();
        assert_eq!(
            picker[n - 4..n - 1].to_vec(),
            boxed(&[
                " couldn't save: destination base folder: Resolved path cannot contain ..",
                " segments",
                "                          [ Use the library default ] [ Cancel ] [ Save ]",
            ])[1..4]
                .to_vec(),
            "{}",
            picker.join("\n")
        );
        assert_eq!(fg(&buffer, at(&rows, "couldn't save: destination base")), th().gold);
        assert_eq!(gui.discover.keep.dest.as_ref().and_then(|ui| ui.hand), Some(super::DestStop::Folder));
    }

    /// `cargo test dump_dest -- --ignored --nocapture`: the picker's states.
    #[test]
    #[ignore]
    fn dump_dest() {
        let _en = english();
        let show = |gui: &mut Gui, what: &str| {
            let rows = draw(gui, 100, 30);
            println!("── {what} · tips: {}", super::super::discover::tips(gui));
            println!("{}", rows.join("\n"));
        };
        let mut gui = dest_gui("");
        show(&mut gui, "F2 idle");
        for _ in 0..5 {
            key(&mut gui, KeyCode::Down);
        }
        key(&mut gui, KeyCode::Enter);
        for c in "{{ARTST}}/{{YEAR}} - ".chars() {
            key(&mut gui, KeyCode::Char(c));
        }
        show(&mut gui, "custom typing");
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
            "gui.tips.discover_keep_queued",
            "gui.tips.discover_keep_running",
            "gui.tips.discover_keep_play",
            "gui.tips.discover_keep_again",
            "gui.tips.discover_keep_retry",
            "gui.tips.discover_keep_start_again",
            "gui.tips.discover_keep_yours",
            "gui.tips.discover_keep_other",
            "gui.tips.discover_dest_library",
            "gui.tips.discover_dest_folder",
            "gui.tips.discover_dest_list",
            "gui.tips.discover_dest_radio",
            "gui.tips.discover_dest_custom",
            "gui.tips.discover_dest_saving",
        ];
        for code in ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
            for key in keys {
                let line = rust_i18n::t!(key, locale = code).to_string();
                assert!(crate::kit::width(&line) <= 99, "{code}: {key} is {} cells: {line}", crate::kit::width(&line));
            }
            // The choice's tip with each verb: "d where files land" gives
            // way first where the line would outgrow the footer — never in
            // English.
            let d = rust_i18n::t!("gui.tips.discover_keep_d", locale = code).to_string();
            for verb in ["add_song", "add_album", "add_artist"] {
                let key = format!("gui.discover.keep.{verb}");
                let verb = rust_i18n::t!(key.as_str(), locale = code).to_string();
                let whole = rust_i18n::t!("gui.tips.discover_keep_choose", locale = code, verb = verb).to_string();
                assert!(whole.contains(&format!(" · {d} · ")), "{code}: the fragment stands whole: {whole}");
                let line = super::fit_tips(whole.clone(), &d);
                assert!(crate::kit::width(&line) <= 99, "{code}: {} cells: {line}", crate::kit::width(&line));
                assert!(code != "en" || line == whole, "English keeps d: {line}");
            }
            for verb in ["verb_save", "verb_cancel", "verb_default"] {
                let key = format!("gui.discover.dest.{verb}");
                let verb = rust_i18n::t!(key.as_str(), locale = code).to_string();
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
        // 76 wide, centred over the frame, 22 tall from row 4: seven copies
        // of eight, the list's bar on the right border.
        let shown: Vec<String> = (4..26).map(|y| rows[y].chars().skip(12).take(76).collect()).collect();
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
                "│   album · cancelled · 1 copied                               Start again ▼",
                "│                                                                          │",
                "│                                                           Clear finished │",
                "╰──────────────────────────────────────────────────────────────────────────╯",
            ]),
            "{all}"
        );
        // The tokens, run by run (the canvas's titles).
        let at = |x: u16, y: u16| Position { x, y };
        assert_eq!((fg(&buffer, at(12, 4)), fg(&buffer, at(14, 5)), bold(&buffer, at(14, 5))), (th().accent, th().accent, true));
        assert_eq!(fg(&buffer, at(83, 5)), th().dim, "the close DIM");
        assert_eq!((fg(&buffer, at(14, 7)), fg(&buffer, at(14, 8))), (th().dim, th().dim), "the header and its rule DIM");
        assert_eq!((fg(&buffer, at(61, 9)), fg(&buffer, at(66, 9)), fg(&buffer, at(72, 9))), (th().ok, th().dim, th().dim), "▰ OK, ▱ and % DIM");
        assert_eq!(fg(&buffer, at(83, 9)), th().dim, "a live row's [X] DIM");
        assert_eq!(fg(&buffer, at(16, 10)), th().dim, "the second row DIM");
        assert_eq!((fg(&buffer, at(61, 11)), fg(&buffer, at(62, 11))), (th().dim, th().dim), "◷ waiting DIM");
        // The held row: both rows in the selection's colours.
        for x in [14, 40, 61, 85] {
            assert_eq!((bg(&buffer, at(x, 13)), bg(&buffer, at(x, 14))), (th().accent, th().accent), "held at {x}");
        }
        assert_eq!(
            (fg(&buffer, at(83, 14)), bg(&buffer, at(83, 14)), bold(&buffer, at(83, 14))),
            (th().on_accent, th().accent, false),
            "the held copy's Retry: the plain slab, never bold"
        );
        assert_eq!(fg(&buffer, at(61, 15)), th().gold, "! refused GOLD");
        assert_eq!((fg(&buffer, at(16, 16)), fg(&buffer, at(23, 16))), (th().dim, th().gold), "song · DIM, the server's words GOLD");
        assert_eq!((fg(&buffer, at(61, 17)), fg(&buffer, at(63, 17))), (th().ok, th().ok), "✓ in your collection OK");
        assert_eq!(fg(&buffer, at(72, 18)), th().accent, "Play your copy ACCENT");
        assert_eq!(fg(&buffer, at(61, 21)), th().dim, "× cancelled DIM");
        assert_eq!(fg(&buffer, at(75, 22)), th().accent, "Start again ACCENT");
        assert_eq!(fg(&buffer, at(72, 24)), th().dim, "Clear finished DIM");
        // No numbers outside the modal: the room's indicator says words.
        assert_eq!(super::super::discover::tips(&gui), "↑ ↓ rows · Enter retry · X clear finished · Esc close");
        // The pointer on a live [X]: DANGER, its tooltip names `x`.
        gui.ui.pointer = Some(at(84, 11));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        assert_eq!((fg(&buffer, at(83, 11)), bold(&buffer, at(83, 11))), (th().danger, true));
        assert_eq!(gui.ui.hit(at(84, 11)), Some(Act::Disc(DiscAct::CopiesCancel(41))));
        assert_eq!(gui.ui.hit(at(82, 14)), Some(Act::Disc(DiscAct::CopiesAct(39))), "Retry");
        assert_eq!(gui.ui.hit(at(80, 24)), Some(Act::Disc(DiscAct::CopiesClear)));
        assert_eq!(gui.ui.hit(at(84, 5)), Some(Act::Disc(DiscAct::CopiesClose)));
        assert_eq!(gui.ui.hit(at(5, 15)), Some(Act::Guard), "the room beneath is inert");
    }

    #[test]
    fn the_copies_list_scrolls_at_the_floor_its_head_and_footer_standing() {
        let _en = english();
        let mut gui = copies_gui(&copies_jobs());
        let rows = draw(&mut gui, 100, 24);
        // 22 rows, the cap: the tab bar above and the tips line below stay.
        assert_eq!(rows[1].chars().nth(12), Some('╭'), "{}", rows.join("\n"));
        assert_eq!(rows[22].chars().nth(12), Some('╰'));
        assert_eq!((rows[6].chars().nth(87), rows[19].chars().nth(87)), (Some('▲'), Some('▼')));
        assert!(rows[18].contains("Free Instrumentals Vol. 9"), "{}", rows.join("\n"));
        assert!(!rows.join("\n").contains("Static Lullaby"), "the eighth below the fold");
        // The wheel scrolls a copy a notch; the head and Clear finished stay.
        gui.ui.pointer = Some(Position { x: 40, y: 10 });
        super::super::discover::wheel(&mut gui, 1);
        let rows = draw(&mut gui, 100, 24);
        assert!(rows[6].contains("every album by Boukmanflow"), "{}", rows.join("\n"));
        assert!(rows[4].contains("TITLE") && rows[21].contains("Clear finished"));
        // The eighth copy: the owned song — "✓ already yours", DIM.
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
        // Finished: Copy again only where gaps are left.
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
            "│   2 copied · 1 already yours · 1 failed · 2 albums left out   Copy again │",
            "│ Rooftop Cypher                   Stage Peer C  ✓ already yours           │",
            "│   song · you had it, nothing copied                                      │",
            "│ Glass Hours                      Stage Peer B  skipped                   │",
            "│   song · a file already sits at that path                          Retry │",
            "│ every album by Night Owl Radio   Stage Peer B  nothing to copy           │",
            "│   nothing of theirs to copy                                              │",
            "│ Free Instrumentals Vol. 9        Stage Peer B  × cancelled               │",
            "│   album · cancelled · 1 copied                               Start again │", blank,
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
        assert_eq!(gui.ui.hit(Position { x: 12 + 50, y: 4 + 10 }), Some(Act::Disc(DiscAct::CopiesTryAgain)));
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
                "gui.discover.keep.yours",
                "gui.discover.copies.skipped",
                "gui.discover.copies.nothing",
                "gui.discover.copies.done",
                "gui.discover.keep.stopping",
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
            for verb in ["cancel", "retry", "again", "start_again", "play"] {
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

    #[test]
    fn one_folder_per_album_stands_dim_in_lands_in_the_library_bold_the_names_text() {
        let _en = english();
        // A running artist copy reads its pinned place, album by album.
        let mut gui = row_in("running", Some("artist"), json!({ "progress": 0.2 }));
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
        let mut gui = row_in("done", Some("album"), counts(3, 0, 0, None));
        let buffer = super::tests::buffer(&mut gui, 100, 30);
        let crumbs = at(&lines(&buffer), "copies › Boukmanflow › Borrowed Tapes");
        let album = Position { x: crumbs.x + 23, y: crumbs.y };
        assert!(fg(&buffer, album) != th().dim && !bold(&buffer, album), "Borrowed Tapes TEXT");
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
