//! The Auto DJ room's sheets (auto-dj contract, clauses 41–53): one per
//! row, each the shape the mood Filters' modal already has (library-rooms
//! clause 32) — 66 cells, the title on the top border, no `[X]`,
//! `[ Done ]` at the foot with `clear` beside it only while something is
//! set — and its law: the sheet edits a draft, and closing it any way
//! applies it. New here: the row's value rides the border after the title,
//! in its tone, and follows the draft; a sheet with a switch opens on
//! `[✓] On`, and switched off its controls rest; the old room's help line
//! lives inside, so the descriptions already translated keep a place.
//!
//! Smart filters is not here: it is the mood modal itself, retitled
//! ([`super::super::moods`]).

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Block;
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::api::types::GenreSource;
use crate::dj::{self, GenreMode, SonicAnchor};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, dim, scroll_list, width, wrap_whole};
use crate::tui::app::{DjEdit, DjRow};

use super::super::{Act, Gui, List, accent, bright_bold, put, sel};
use super::{PickServer, Tone};

/// The sheets' cells across, and the cells their rows take: inside the
/// border, a pad cell each side.
const SHEET_W: u16 = 66;
const TEXT_W: u16 = SHEET_W - 4;
/// The second and third columns of a row of choices, from the first.
const COL2: u16 = 26;
const COL3: u16 = 46;
/// The Genres list's rows.
const GENRE_ROWS: u16 = 11;
/// The most rows of words Skip words shows.
const CHIP_ROWS: usize = 4;
/// The cells Skip words' `+` takes beside its card, a cell clear of it.
const ADD_W: u16 = 5;
/// The most characters a skip word or a typed length takes.
const WORD_MAX: usize = 40;
const LENGTH_MAX: usize = 8;

/// The room's sheets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sheet {
    PlayFrom,
    Sonic,
    Genres,
    Rating,
    Length,
    Batch,
    Tempo,
    SkipWords,
    Cooldown,
    Preview,
}

/// One keyboard stop of a sheet — and what a click on it presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// `[✓] On`.
    Switch,
    /// A bar: ← → step it, a click on a cell sets it.
    Bar,
    /// The first row of choices: the anchor (Drift · Stay close), the way
    /// a genre list cuts (Only · Skip).
    Radio(u8),
    /// The kind of genre the list matches (Generated · Tags · Mixed).
    Kind(u8),
    /// A text field: Length's two bounds, Skip words' line.
    Field(u8),
    /// A checkbox by its place: Play from's libraries, Length's Include
    /// unknown.
    Check(usize),
    /// One of Play from's servers.
    Server(usize),
    /// One of Skip words' words.
    Chip(usize),
    /// Rating's stars.
    Stars,
    /// The Genres list; its search line takes the letters.
    List,
    Clear,
    Done,
}

/// What an open sheet is editing — the room's own changes only when it
/// closes.
#[derive(Debug, Clone)]
enum Draft {
    PlayFrom {
        servers: Vec<PickServer>,
        /// The server ticked, and the one the rows were about on opening.
        chosen: usize,
        home: usize,
        libraries: Vec<String>,
        off: Vec<String>,
    },
    Sonic {
        on: bool,
        /// Hundredths of cosine: 30–80.
        floor: u32,
        anchor: SonicAnchor,
    },
    Genres {
        mode: GenreMode,
        source: GenreSource,
        chosen: Vec<String>,
        /// The genres chosen when the list was laid out: they stand first,
        /// and a tick since moves nothing under the hand.
        pinned: Vec<String>,
        filter: Input,
        row: usize,
    },
    Rating {
        rating: u32,
    },
    Length {
        min: String,
        max: String,
        unknown: bool,
    },
    Batch {
        songs: u32,
    },
    Tempo {
        on: bool,
        tolerance: u32,
    },
    SkipWords {
        words: Vec<String>,
        input: Input,
    },
    Cooldown {
        artists: u32,
    },
    Preview,
}

impl Draft {
    /// Whether two drafts hold the same settings — a line still being
    /// typed aside, which is no setting yet.
    fn same(&self, other: &Draft) -> bool {
        match (self, other) {
            (Draft::PlayFrom { chosen: a, off: b, .. }, Draft::PlayFrom { chosen: x, off: y, .. }) => a == x && b == y,
            (Draft::Sonic { on: a, floor: b, anchor: c }, Draft::Sonic { on: x, floor: y, anchor: z }) => {
                (a, b, c) == (x, y, z)
            }
            (
                Draft::Genres { mode: a, source: b, chosen: c, .. },
                Draft::Genres { mode: x, source: y, chosen: z, .. },
            ) => a == x && b == y && c == z,
            (Draft::Rating { rating: a }, Draft::Rating { rating: x }) => a == x,
            (Draft::Length { min: a, max: b, unknown: c }, Draft::Length { min: x, max: y, unknown: z }) => {
                (a, b, c) == (x, y, z)
            }
            (Draft::Batch { songs: a }, Draft::Batch { songs: x }) => a == x,
            (Draft::Tempo { on: a, tolerance: b }, Draft::Tempo { on: x, tolerance: y }) => (a, b) == (x, y),
            (Draft::SkipWords { words: a, .. }, Draft::SkipWords { words: x, .. }) => a == x,
            (Draft::Cooldown { artists: a }, Draft::Cooldown { artists: x }) => a == x,
            (Draft::Preview, Draft::Preview) => true,
            _ => false,
        }
    }
}

/// The open sheet: which, its draft, and the keyboard's hand.
#[derive(Debug, Clone)]
pub(crate) struct Open {
    pub sheet: Sheet,
    draft: Draft,
    /// What the draft was on opening: closing unchanged writes nothing.
    was: Draft,
    hand: Stop,
    /// Whether the hand is drawn: the sheet opened by its key, or a walking
    /// key pressed since a click stowed it (the kit's list-cursor law).
    shown: bool,
}

/// The pointer's acts inside a sheet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SheetAct {
    /// A stop pressed.
    Press(Stop),
    /// A bar's `-` or `+`, or a length's.
    Step(Stop, i32),
    /// A bar's cell: the value it sets.
    Set(u32),
    /// A star, by its place.
    Star(u32),
    /// A genre's row: ticked or let go.
    Genre(String),
    /// Skip words' `+`: the word on the line added, as Enter adds it.
    AddWord,
}

// ── Opening and closing ─────────────────────────────────────────────────────

/// Open `sheet` on the room's own values — what its row says, so a filter
/// that is off opens empty. `key` says the keyboard opened it: the hand is
/// up on its first stop.
pub(crate) fn open(gui: &mut Gui, sheet: Sheet, key: bool) {
    let app = &gui.app;
    let s = &app.dj;
    let (draft, hand) = match sheet {
        Sheet::Sonic => (
            Draft::Sonic {
                on: s.sonic,
                floor: (s.sonic_min_similarity * 100.0).round() as u32,
                anchor: s.sonic_anchor,
            },
            Stop::Switch,
        ),
        Sheet::Batch => (Draft::Batch { songs: s.songs_per_fetch }, Stop::Bar),
        Sheet::Tempo => (Draft::Tempo { on: s.bpm, tolerance: s.bpm_tolerance }, Stop::Switch),
        Sheet::Cooldown => (Draft::Cooldown { artists: s.artist_cooldown }, Stop::Bar),
        Sheet::Rating => (Draft::Rating { rating: app.dj_library().min_rating }, Stop::Stars),
        Sheet::Length => {
            let (min, max) = s.length_bounds().unwrap_or((None, None));
            let words = |bound: Option<u32>| bound.map(dj::fmt_length).unwrap_or_default();
            (
                Draft::Length { min: words(min), max: words(max), unknown: s.allow_unknown_length },
                Stop::Field(0),
            )
        }
        Sheet::SkipWords => {
            let words = if s.keyword_filter { s.keywords.clone() } else { Vec::new() };
            (Draft::SkipWords { words, input: Input::default() }, Stop::Field(0))
        }
        Sheet::Genres => {
            let library = app.dj_library();
            let chosen = if library.genre_filter { library.genres.clone() } else { Vec::new() };
            let mut pinned = chosen.clone();
            pinned.sort_by_key(|g| g.to_lowercase());
            (
                Draft::Genres {
                    mode: library.genre_mode,
                    source: library.genre_source,
                    chosen,
                    pinned,
                    filter: Input::default(),
                    row: 0,
                },
                Stop::List,
            )
        }
        Sheet::PlayFrom => {
            let servers = super::pickable_servers(gui);
            let home = servers.iter().position(|server| server.current).unwrap_or(0);
            let hand = if servers.len() > 1 { Stop::Server(home) } else { Stop::Check(0) };
            (
                Draft::PlayFrom {
                    servers,
                    chosen: home,
                    home,
                    libraries: super::libraries(gui),
                    off: app.dj_sources_off(),
                },
                hand,
            )
        }
        Sheet::Preview => (Draft::Preview, Stop::Done),
    };
    gui.dj.list = ListView::default();
    if let Draft::Genres { source, .. } = &draft {
        let source = *source;
        let effects = gui.app.dj_genres_ask(source);
        gui.pend(effects);
    }
    gui.dj.quick = None;
    gui.dj.first = None;
    gui.dj.sheet = Some(Open { sheet, was: draft.clone(), draft, hand, shown: key });
}

/// The draft becomes the room's own, and the sheet closes — Done, Esc and
/// Enter alike. A sheet closed as it was opened writes nothing.
fn done(gui: &mut Gui) {
    let Some(open) = gui.dj.sheet.take() else { return };
    if matches!(open.draft, Draft::Genres { .. }) {
        gui.app.close_dj_picker();
    }
    let unchanged = open.draft.same(&open.was);
    match open.draft {
        // A word still on the line when the sheet closes counts: it was
        // typed to be added.
        Draft::SkipWords { mut words, input } => {
            let pending = input.value().trim().to_string();
            let fresh = !pending.is_empty() && !words.iter().any(|w| w.eq_ignore_ascii_case(&pending));
            if fresh && words.len() < dj::KEYWORDS_MAX {
                words.push(pending);
            } else if unchanged {
                return;
            }
            super::edit(gui, DjEdit::Keywords(words));
        }
        _ if unchanged => {}
        Draft::Sonic { on, floor, anchor } => super::edit(gui, DjEdit::Sonic { on, floor, anchor }),
        Draft::Batch { songs } => super::edit(gui, DjEdit::Set(DjRow::SongsPerFetch, songs)),
        Draft::Tempo { on, tolerance } => super::edit(gui, DjEdit::Tempo { on, tolerance }),
        Draft::Cooldown { artists } => super::edit(gui, DjEdit::Set(DjRow::Cooldown, artists)),
        Draft::Rating { rating } => super::edit(gui, DjEdit::Set(DjRow::Rating, rating)),
        Draft::Length { min, max, unknown } => {
            // A side that does not read as a length is left as it was.
            let (was_min, was_max) = gui.app.dj.length_bounds().unwrap_or((None, None));
            let read = |text: &str, was: Option<u32>| {
                if text.trim().is_empty() { None } else { dj::parse_length(text).or(was) }
            };
            let (min, max) = (read(&min, was_min), read(&max, was_max));
            super::edit(gui, DjEdit::LengthWindow { min, max, unknown });
        }
        Draft::Genres { mode, source, chosen, .. } => {
            super::edit(gui, DjEdit::GenreRule { mode, source, genres: chosen });
        }
        Draft::PlayFrom { servers, chosen, home, libraries, off } => {
            // The folders are the rows' server's: switched on first, so the
            // last-one rule never stands in a swap's way.
            let now = gui.app.dj_sources_off();
            let mut effects = Vec::new();
            for on in [true, false] {
                for library in &libraries {
                    let (was_off, is_off) = (now.contains(library), off.contains(library));
                    if was_off != is_off && is_off != on {
                        effects.extend(gui.app.toggle_dj_source(library));
                    }
                }
            }
            if chosen != home
                && let Some(server) = servers.get(chosen)
            {
                effects.extend(gui.app.dj_play_from_set(server.id.clone()));
            }
            gui.pend(effects);
            gui.save_now();
        }
        Draft::Preview => {}
    }
}

// ── Words ───────────────────────────────────────────────────────────────────

fn done_word() -> String {
    format!("[ {} ]", t!("gui.moods.done"))
}

fn clear_word() -> String {
    t!("gui.moods.clear").to_string()
}

/// The sheet's name and its draft's value, as the row would say it.
fn title_of(open: &Open, gui: &Gui) -> (String, (String, Tone)) {
    let plain = |words: String| (words, Tone::Neutral);
    match &open.draft {
        Draft::PlayFrom { servers, chosen, .. } => (
            t!("gui.dj.play_from").to_string(),
            plain(servers.get(*chosen).map(|server| server.label.clone()).unwrap_or_default()),
        ),
        Draft::Sonic { on, floor, .. } => {
            let value = match super::sonic_drop(gui).filter(|_| open.draft.same(&open.was)) {
                Some(why) => (format!("! {why}"), Tone::Warning),
                None => super::sonic_value(*on, *floor),
            };
            (t!("gui.dj.sonic").to_string(), value)
        }
        Draft::Genres { mode, chosen, .. } => (t!("gui.dj.genres").to_string(), super::genres_value(*mode, chosen)),
        Draft::Rating { rating } => (t!("gui.dj.rating_row").to_string(), super::rating_value(*rating)),
        Draft::Length { min, max, .. } => {
            let read = |text: &str| dj::parse_length(text).filter(|seconds| *seconds > 0);
            (t!("gui.dj.length_row").to_string(), super::length_value(read(min), read(max)))
        }
        Draft::Batch { songs } => (t!("gui.dj.batch").to_string(), plain(super::songs_words(*songs))),
        Draft::Tempo { on, tolerance } => (t!("gui.dj.beat").to_string(), super::tempo_value(*on, *tolerance)),
        Draft::SkipWords { words, .. } => (t!("gui.dj.skip_words").to_string(), super::words_value(words)),
        Draft::Cooldown { artists } => (t!("gui.dj.cooldown").to_string(), super::cooldown_value(*artists)),
        Draft::Preview => (t!("gui.dj.preview").to_string(), plain(String::new())),
    }
}

/// The sheet's help: the old room's line for the row, wrapped to the sheet.
fn help_of(sheet: Sheet) -> Vec<String> {
    let text = match sheet {
        Sheet::Sonic => t!("gui.dj.sonic_sub"),
        Sheet::Batch => t!("gui.dj.songs_per_fetch_sub"),
        Sheet::Tempo => t!("gui.dj.bpm_sub"),
        Sheet::Cooldown => t!("gui.dj.cooldown_sub"),
        Sheet::Rating => t!("gui.dj.rating_sub"),
        Sheet::Length => t!("gui.dj.length_help"),
        Sheet::SkipWords => t!("gui.dj.keyword_sub"),
        Sheet::PlayFrom | Sheet::Genres | Sheet::Preview => return Vec::new(),
    };
    // Whole in the sheet's width whatever the language: a sentence with
    // no blanks to break at breaks between its characters.
    wrap_whole(&text, TEXT_W as usize).into_iter().take(4).collect()
}

/// Whether the draft has something `clear` would let go of.
fn clear_shown(draft: &Draft) -> bool {
    match draft {
        Draft::Genres { chosen, .. } => !chosen.is_empty(),
        Draft::Rating { rating } => *rating > 0,
        Draft::Length { min, max, .. } => !min.trim().is_empty() || !max.trim().is_empty(),
        Draft::SkipWords { words, .. } => !words.is_empty(),
        _ => false,
    }
}

// ── Geometry ────────────────────────────────────────────────────────────────

/// A bar's cells, the value its first cell sets, and what a cell is worth.
struct BarSpec {
    cells: u16,
    lo: u32,
    hi: u32,
    /// The value cell `k` (from zero) sets: `first + k * per`.
    first: u32,
    per: u32,
    value: u32,
    words: String,
}

fn bar_of(draft: &Draft) -> Option<BarSpec> {
    Some(match draft {
        Draft::Batch { songs } => BarSpec {
            cells: dj::SONGS_PER_FETCH_MAX as u16,
            lo: 1,
            hi: dj::SONGS_PER_FETCH_MAX,
            first: 1,
            per: 1,
            value: *songs,
            words: super::songs_words(*songs),
        },
        Draft::Tempo { tolerance, .. } => BarSpec {
            cells: dj::BPM_TOLERANCE_MAX as u16,
            lo: dj::BPM_TOLERANCE_MIN,
            hi: dj::BPM_TOLERANCE_MAX,
            first: 1,
            per: 1,
            value: *tolerance,
            words: t!("gui.dj.tolerance_value", bpm = *tolerance).to_string(),
        },
        Draft::Cooldown { artists } => BarSpec {
            cells: dj::ARTIST_COOLDOWN_MAX as u16,
            lo: 0,
            hi: dj::ARTIST_COOLDOWN_MAX,
            first: 1,
            per: 1,
            value: *artists,
            words: super::cooldown_value(*artists).0,
        },
        // Two points a cell across the band, thirty to eighty.
        Draft::Sonic { floor, .. } => BarSpec {
            cells: 25,
            lo: 30,
            hi: 80,
            first: 32,
            per: 2,
            value: *floor,
            words: t!("gui.dj.percent", pct = *floor).to_string(),
        },
        _ => return None,
    })
}

impl BarSpec {
    fn filled(&self) -> u16 {
        if self.value < self.first.saturating_sub(self.per / 2) {
            return 0;
        }
        let k = (self.value + self.per - self.first + self.per / 2) / self.per.max(1);
        (k as u16).min(self.cells)
    }
}

/// The words of Skip words laid out in rows of the sheet's width: a word,
/// a space and `[X]`, three cells apart. Each entry is the word's index and
/// its first cell.
fn chip_rows(words: &[String]) -> Vec<Vec<(usize, u16)>> {
    let mut rows: Vec<Vec<(usize, u16)>> = vec![Vec::new()];
    let mut x = 0u16;
    for (i, word) in words.iter().enumerate() {
        let w = width(word) as u16 + 4;
        if x > 0 && x + w > TEXT_W {
            if rows.len() == CHIP_ROWS {
                break;
            }
            rows.push(Vec::new());
            x = 0;
        }
        rows.last_mut().expect("a row").push((i, x));
        x += w + 3;
    }
    rows
}

/// The Genres list's rows: the ones chosen when it was laid out first,
/// then the server's — narrowed by the search line.
fn genre_rows<'a>(all: &'a [String], pinned: &'a [String], query: &str) -> Vec<&'a str> {
    let query = query.trim().to_lowercase();
    let hit = |genre: &&str| query.is_empty() || genre.to_lowercase().contains(&query);
    let first: std::collections::HashSet<&str> = pinned.iter().map(String::as_str).collect();
    pinned
        .iter()
        .map(String::as_str)
        .filter(hit)
        .chain(all.iter().map(String::as_str).filter(|genre| !first.contains(genre)).filter(hit))
        .collect()
}

/// Whether the Genres sheet offers the kinds: the server said how many of
/// each it has, and it has both.
fn kinds_offered(gui: &Gui) -> bool {
    gui.app
        .dj_panel
        .genres
        .as_ref()
        .and_then(|picker| picker.sources)
        .is_some_and(|sources| sources.offers_choice())
}

const KINDS: [GenreSource; 3] = [GenreSource::Model, GenreSource::Tag, GenreSource::Both];

fn kind_word(kind: GenreSource) -> String {
    match kind {
        GenreSource::Model => t!("gui.dj.source_generated"),
        GenreSource::Tag => t!("gui.dj.source_tags"),
        GenreSource::Both => t!("gui.dj.source_mixed"),
    }
    .to_string()
}

/// How many rows the Genres list takes in a frame `height` tall.
fn genre_list_rows(height: u16, kinds: bool) -> u16 {
    // The frame's two rows, the way it cuts, the kinds, a blank, the search
    // line, a blank and the foot.
    let fixed = 2 + 1 + u16::from(kinds) + 1 + 1 + 1 + 1;
    GENRE_ROWS.min(height.saturating_sub(2 + fixed)).max(3)
}

/// Whether a Length bound is real: Include unknown only means something
/// beside one.
fn bound_real(draft: &Draft) -> bool {
    match draft {
        Draft::Length { min, max, .. } => {
            let real = |text: &str| dj::parse_length(text).is_some_and(|seconds| seconds > 0);
            real(min) || real(max)
        }
        _ => false,
    }
}

/// A line of Play from's inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayLine {
    Server(usize),
    Gap,
    /// The name of the server the folders are of.
    Name,
    Library(usize),
}

/// Play from's lines, top to bottom: the servers where there is a choice
/// of them, then — while the one ticked is the one the rows are about — a
/// blank, its name and its folders.
fn play_lines(draft: &Draft) -> Vec<PlayLine> {
    let Draft::PlayFrom { servers, chosen, home, libraries, .. } = draft else { return Vec::new() };
    let mut lines = Vec::new();
    if servers.len() > 1 {
        lines.extend((0..servers.len()).map(PlayLine::Server));
    }
    if libraries.len() > 1 && chosen == home {
        if !lines.is_empty() {
            lines.push(PlayLine::Gap);
        }
        lines.push(PlayLine::Name);
        lines.extend((0..libraries.len()).map(PlayLine::Library));
    }
    lines
}

/// The sheet's stops by row — what ↑ ↓ ← → walk — the foot's last.
fn stop_rows(open: &Open, gui: &Gui) -> Vec<Vec<Stop>> {
    let mut rows: Vec<Vec<Stop>> = Vec::new();
    match &open.draft {
        Draft::PlayFrom { servers, chosen, home, libraries, .. } => {
            if servers.len() > 1 {
                rows.extend((0..servers.len()).map(|i| vec![Stop::Server(i)]));
            }
            if libraries.len() > 1 && chosen == home {
                rows.extend((0..libraries.len()).map(|i| vec![Stop::Check(i)]));
            }
        }
        Draft::Sonic { on, .. } => {
            rows.push(vec![Stop::Switch]);
            if *on {
                rows.push(vec![Stop::Bar]);
                rows.push(vec![Stop::Radio(0), Stop::Radio(1)]);
            }
        }
        Draft::Genres { .. } => {
            rows.push(vec![Stop::Radio(0), Stop::Radio(1)]);
            if kinds_offered(gui) {
                rows.push(vec![Stop::Kind(0), Stop::Kind(1), Stop::Kind(2)]);
            }
            rows.push(vec![Stop::List]);
        }
        Draft::Rating { .. } => rows.push(vec![Stop::Stars]),
        Draft::Length { .. } => {
            rows.push(vec![Stop::Field(0)]);
            rows.push(vec![Stop::Field(1)]);
            if bound_real(&open.draft) {
                rows.push(vec![Stop::Check(0)]);
            }
        }
        Draft::Batch { .. } | Draft::Cooldown { .. } => rows.push(vec![Stop::Bar]),
        Draft::Tempo { on, .. } => {
            rows.push(vec![Stop::Switch]);
            if *on {
                rows.push(vec![Stop::Bar]);
            }
        }
        Draft::SkipWords { words, .. } => {
            rows.push(vec![Stop::Field(0)]);
            for row in chip_rows(words) {
                if !row.is_empty() {
                    rows.push(row.into_iter().map(|(i, _)| Stop::Chip(i)).collect());
                }
            }
        }
        Draft::Preview => {}
    }
    let mut foot = Vec::new();
    if clear_shown(&open.draft) {
        foot.push(Stop::Clear);
    }
    foot.push(Stop::Done);
    rows.push(foot);
    rows
}

fn locate(rows: &[Vec<Stop>], stop: Stop) -> Option<(usize, usize)> {
    rows.iter().enumerate().find_map(|(r, row)| row.iter().position(|s| *s == stop).map(|c| (r, c)))
}

/// The hand off a stop that stopped being one: a `clear` gone to
/// `[ Done ]`, a control gone to rest to its switch, a word gone to the
/// line it was typed on.
fn settle(open: &mut Open, gui: &Gui) {
    let rows = stop_rows(open, gui);
    if locate(&rows, open.hand).is_some() {
        return;
    }
    open.hand = match open.hand {
        Stop::Clear => Stop::Done,
        Stop::Chip(i) if i > 0 && locate(&rows, Stop::Chip(i - 1)).is_some() => Stop::Chip(i - 1),
        Stop::Chip(_) => Stop::Field(0),
        Stop::Bar | Stop::Radio(_) => Stop::Switch,
        Stop::Check(_) if locate(&rows, Stop::Field(1)).is_some() => Stop::Field(1),
        _ => rows.first().and_then(|row| row.first()).copied().unwrap_or(Stop::Done),
    };
}

/// The rows the sheet's inside takes, the foot in.
fn body_rows(open: &Open, gui: &Gui, area: Rect) -> u16 {
    let help = help_of(open.sheet).len() as u16;
    // A help block is a blank and its lines; the foot a blank and its row.
    let tail = if help > 0 { 1 + help + 2 } else { 2 };
    match &open.draft {
        Draft::Batch { .. } | Draft::Cooldown { .. } | Draft::Rating { .. } => 1 + tail,
        Draft::Tempo { .. } => 3 + tail,
        Draft::Sonic { .. } => 5 + tail,
        Draft::Length { .. } => 4 + tail,
        // The line's card is three rows; the words stand right under it.
        Draft::SkipWords { words, .. } => 3 + chip_rows(words).len() as u16 + tail,
        Draft::Genres { .. } => {
            let kinds = kinds_offered(gui);
            1 + u16::from(kinds) + 2 + genre_list_rows(area.height, kinds) + 2
        }
        // Its lines, as many as the screen affords — the rest behind the
        // list's window.
        Draft::PlayFrom { .. } => {
            (play_lines(&open.draft).len() as u16).min(area.height.saturating_sub(8)) + 2
        }
        Draft::Preview => 3 + 2,
    }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// A sheet's title on its top border, ` {title} · {value} ` from two cells
/// in: the title TEXT BOLD, the dot DIM, the value in its tone.
pub(crate) fn border_title_value(frame: &mut Frame, rect: Rect, title: &str, value: &str, tone: Tone) {
    let owned = th().ground.is_some() && crate::kit::theme::ground_owned();
    let text = if owned { th().text } else { ratatui::style::Color::Reset };
    let x = rect.x + 2;
    let head = format!(" {title} ");
    put(frame, x, rect.y, &head, Style::default().fg(text).add_modifier(Modifier::BOLD));
    if value.is_empty() {
        return;
    }
    let used = width(&head) as u16;
    // Two cells of border stay at the far end.
    let room = rect.width.saturating_sub(used + 2 + 3 + 4) as usize;
    if room < 3 {
        return;
    }
    put(frame, x + used, rect.y, "· ", dim());
    let shown = super::super::bar::clip(value, room);
    put(frame, x + used + 2, rect.y, &format!("{shown} "), tone.style());
}

/// What a choice wears: the glyph in its state's colour whatever lights
/// the word; the word ACCENT BOLD under the keyboard's hand, BRIGHT BOLD
/// under the pointer, BOLD while chosen, DIM while it rests.
#[allow(clippy::too_many_arguments)]
fn draw_choice(
    frame: &mut Frame,
    gui: &mut Gui,
    x: u16,
    y: u16,
    glyphs: (&str, &str),
    on: bool,
    on_style: Style,
    word: &str,
    hand: bool,
    live: bool,
    act: SheetAct,
) {
    let rect = Rect { x, y, width: 4 + width(word) as u16, height: 1 };
    let hover = live && gui.ui.hovers(rect);
    let glyph_style = if on && live { on_style } else { dim() };
    put(frame, x, y, if on { glyphs.0 } else { glyphs.1 }, glyph_style);
    let word_style = match (live, hand, hover, on) {
        (false, _, _, _) => dim(),
        (true, true, _, _) => accent().add_modifier(Modifier::BOLD),
        (true, false, true, _) => bright_bold(),
        (true, false, false, true) => Style::default().add_modifier(Modifier::BOLD),
        (true, false, false, false) => Style::default(),
    };
    put(frame, x + 4, y, word, word_style);
    if live {
        gui.ui.click(rect, Act::DjSheet(act));
    }
}

fn radio_glyphs() -> (&'static str, &'static str) {
    if legacy_conhost() { ("(*)", "( )") } else { ("(•)", "( )") }
}

/// A bar in the volume widget's grammar: `-`, its cells, `+`, the value in
/// words. A click on an end steps, on a cell sets; at rest it is DIM whole
/// and takes no click.
fn draw_bar(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, spec: &BarSpec, hand: bool, live: bool) {
    let (full, empty) = if legacy_conhost() { ("■", "·") } else { ("▰", "▱") };
    let end_style = |gui: &Gui, rect: Rect| if live && gui.ui.hovers(rect) { bright_bold() } else { dim() };
    let minus = Rect { x, y, width: 1, height: 1 };
    put(frame, x, y, "-", end_style(gui, minus));
    let cells = Rect { x: x + 2, y, width: spec.cells, height: 1 };
    let lit = live && (hand || gui.ui.hovers(cells));
    let filled = spec.filled();
    for k in 0..spec.cells {
        let on = k < filled;
        let style = match (live, on) {
            (true, true) => Style::default().fg(if lit { th().bright } else { th().accent }),
            _ => dim(),
        };
        put(frame, cells.x + k, y, if on { full } else { empty }, style);
        if live {
            let value = spec.first + u32::from(k) * spec.per;
            gui.ui.click(Rect { x: cells.x + k, y, width: 1, height: 1 }, Act::DjSheet(SheetAct::Set(value)));
        }
    }
    let plus = Rect { x: cells.right() + 1, y, width: 1, height: 1 };
    put(frame, plus.x, y, "+", end_style(gui, plus));
    let words_style = match (live, hand) {
        (false, _) => dim(),
        (true, true) => accent().add_modifier(Modifier::BOLD),
        (true, false) => Style::default(),
    };
    put(frame, plus.x + 4, y, &spec.words, words_style);
    if live {
        gui.ui.click(minus, Act::DjSheet(SheetAct::Step(Stop::Bar, -1)));
        gui.ui.click(plus, Act::DjSheet(SheetAct::Step(Stop::Bar, 1)));
    }
}

/// The foot: `[ Done ]` ending three cells in from the right corner, and
/// `clear` three cells left of it while there is something to clear; a
/// note at the left where the sheet has one.
fn draw_foot(frame: &mut Frame, gui: &mut Gui, rect: Rect, open: &Open, note: Option<String>) {
    let y = rect.bottom().saturating_sub(2);
    let hand = |stop: Stop| open.shown && open.hand == stop;
    let done_word = done_word();
    let done_w = width(&done_word) as u16;
    let done = Rect { x: rect.right().saturating_sub(2 + done_w), y, width: done_w, height: 1 };
    let mut left_end = done.x;
    if clear_shown(&open.draft) {
        let word = clear_word();
        let w = width(&word) as u16;
        let clear = Rect { x: done.x.saturating_sub(3 + w), y, width: w, height: 1 };
        let style = if hand(Stop::Clear) {
            accent().add_modifier(Modifier::BOLD)
        } else if gui.ui.hovers(clear) {
            bright_bold()
        } else {
            dim()
        };
        put(frame, clear.x, y, &word, style);
        gui.ui.click(clear, Act::DjSheet(SheetAct::Press(Stop::Clear)));
        left_end = clear.x;
    }
    let style = if hand(Stop::Done) {
        sel().add_modifier(Modifier::BOLD)
    } else if gui.ui.hovers(done) {
        bright_bold()
    } else {
        accent().add_modifier(Modifier::BOLD)
    };
    put(frame, done.x, y, &done_word, style);
    gui.ui.click(done, Act::DjSheet(SheetAct::Press(Stop::Done)));
    if let Some(note) = note {
        let room = left_end.saturating_sub(rect.x + 2 + 3) as usize;
        put(frame, rect.x + 2, y, &super::super::bar::clip(&note, room), dim());
    }
}

/// The sheet over the room, drawn and registered over everything beneath:
/// the room's registries dropped, the whole frame inert, the band cleared
/// where it has one; the title and the draft's value on the top border;
/// the sheet's controls; its help; the foot.
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(mut open) = gui.dj.sheet.clone() else { return };
    settle(&mut open, gui);
    let h = body_rows(&open, gui, area) + 2;
    let (rect, band) = super::super::moods::place(area, gui.room_rect(), h);
    gui.ui.clear_registries();
    gui.ui.click(area, Act::Guard);
    let own = band.unwrap_or(rect);
    gui.ui.overlay(own);
    gui.ui.modal_over();
    if let Some(band) = band {
        crate::kit::blank(frame, band);
    }
    super::super::keep::modal_box(frame, rect);
    gui.ui.click(own, Act::Guard);
    let (title, (value, tone)) = title_of(&open, gui);
    border_title_value(frame, rect, &title, &value, tone);

    let (x0, top) = (rect.x + 2, rect.y + 1);
    let hand = |stop: Stop| open.shown && open.hand == stop;
    let (check_on, check_off) = super::super::check_glyphs();
    let checks = (check_on, check_off);
    let ok = Style::default().fg(th().ok);
    let mut note = None;
    // Where the controls end and the help begins.
    let mut y = top;
    match &open.draft {
        Draft::Batch { .. } | Draft::Cooldown { .. } => {
            let spec = bar_of(&open.draft).expect("a bar");
            draw_bar(frame, gui, x0, y, &spec, hand(Stop::Bar), true);
            y += 1;
        }
        Draft::Tempo { on, .. } | Draft::Sonic { on, .. } => {
            let on = *on;
            let word = t!("gui.dj.switch_on").to_string();
            draw_choice(frame, gui, x0, y, checks, on, ok, &word, hand(Stop::Switch), true, SheetAct::Press(Stop::Switch));
            let spec = bar_of(&open.draft).expect("a bar");
            draw_bar(frame, gui, x0, y + 2, &spec, hand(Stop::Bar), on);
            y += 3;
            if let Draft::Sonic { anchor, .. } = &open.draft {
                let radios = radio_glyphs();
                let words = [t!("gui.dj.drift").to_string(), t!("gui.dj.stay_close").to_string()];
                let x2 = x0 + COL2.max(4 + width(&words[0]) as u16 + 3);
                for (i, (kind, word)) in [SonicAnchor::Rolling, SonicAnchor::Locked].into_iter().zip(&words).enumerate() {
                    let stop = Stop::Radio(i as u8);
                    let x = if i == 0 { x0 } else { x2 };
                    draw_choice(frame, gui, x, y + 1, radios, *anchor == kind, accent(), word, hand(stop), on, SheetAct::Press(stop));
                }
                y += 2;
            }
        }
        Draft::Rating { rating } => {
            draw_stars(frame, gui, x0, y, *rating, hand(Stop::Stars));
            y += 1;
        }
        Draft::Length { min, max, unknown } => {
            let labels = [t!("gui.dj.shortest").to_string(), t!("gui.dj.longest").to_string()];
            for (i, (label, text)) in labels.iter().zip([min, max]).enumerate() {
                draw_length(frame, gui, x0, y + i as u16, label, text, i as u8, open.hand == Stop::Field(i as u8));
            }
            let word = t!("gui.dj.include_unknown").to_string();
            let stop = Stop::Check(0);
            let live = bound_real(&open.draft);
            draw_choice(frame, gui, x0, y + 3, checks, *unknown, ok, &word, hand(stop), live, SheetAct::Press(stop));
            y += 4;
        }
        Draft::SkipWords { words, input } => {
            draw_word_card(frame, gui, x0, y, input, open.hand == Stop::Field(0));
            for (r, row) in chip_rows(words).iter().enumerate() {
                for &(i, cx) in row {
                    draw_chip(frame, gui, x0 + cx, y + 3 + r as u16, &words[i], i, hand(Stop::Chip(i)));
                }
            }
            if words.len() >= dj::KEYWORDS_MAX {
                note = Some(t!("gui.dj.max", count = dj::KEYWORDS_MAX).to_string());
            }
            y += 3 + chip_rows(words).len() as u16;
        }
        Draft::Genres { .. } => {
            note = draw_genres(frame, gui, rect, &open);
            y = rect.bottom();
        }
        Draft::PlayFrom { .. } => {
            draw_play_from(frame, gui, rect, &open);
            y = rect.bottom();
        }
        Draft::Preview => {
            note = draw_preview(frame, gui, rect);
            y = rect.bottom();
        }
    }
    let help = help_of(open.sheet);
    for (i, line) in help.iter().enumerate() {
        let hy = y + 1 + i as u16;
        if hy + 2 < rect.bottom() {
            put(frame, x0, hy, line, dim());
        }
    }
    draw_foot(frame, gui, rect, &open, note);
    gui.dj.sheet = Some(open);
}

/// Rating's stars: five, a cell apart — a click on one sets it whole, a
/// second click on it halves it; ← → step by halves.
fn draw_stars(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, rating: u32, hand: bool) {
    let glyphs = super::super::actions::stars(Some(rating));
    for (i, glyph) in glyphs.chars().enumerate() {
        let cell = Rect { x: x + 2 * i as u16, y, width: 1, height: 1 };
        let full = 2 * (i as u32 + 1);
        let lit = rating + 1 >= full;
        let style = match (gui.ui.hovers(cell), hand, lit) {
            (true, _, _) => bright_bold(),
            (false, true, true) => accent().add_modifier(Modifier::BOLD),
            (false, false, true) => Style::default().fg(th().ok),
            (false, _, false) => dim(),
        };
        put(frame, cell.x, y, &glyph.to_string(), style);
        gui.ui.click(cell, Act::DjSheet(SheetAct::Star(i as u32)));
    }
}

/// A Length bound: its name, `-`, the typed length ending at one column
/// with the caret after it while it has the keyboard, `+`. What does not
/// read as a length shows GOLD.
#[allow(clippy::too_many_arguments)]
fn draw_length(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, label: &str, text: &str, index: u8, held: bool) {
    let stop = Stop::Field(index);
    let row = Rect { x, y, width: 37, height: 1 };
    let label_style = if held { accent().add_modifier(Modifier::BOLD) } else { Style::default() };
    put(frame, x, y, &super::super::bar::clip(label, 22), label_style);
    let minus = Rect { x: x + 23, y, width: 1, height: 1 };
    let plus = Rect { x: x + 35, y, width: 1, height: 1 };
    gui.ui.click(row, Act::DjSheet(SheetAct::Press(stop)));
    for (rect, glyph, delta) in [(minus, "-", -1), (plus, "+", 1)] {
        put(frame, rect.x, y, glyph, if gui.ui.hovers(rect) { bright_bold() } else { dim() });
        gui.ui.click(rect, Act::DjSheet(SheetAct::Step(stop, delta)));
    }
    let valid = text.trim().is_empty() || dj::parse_length(text).is_some();
    let style = if valid { Style::default() } else { Style::default().fg(th().gold) };
    let end = x + 33;
    put(frame, end.saturating_sub(width(text) as u16), y, text, style);
    if held {
        // The caret sits after the last character: the field types at its
        // end, and ← → are its steps.
        let on = gui.ui.caret();
        if on {
            put(frame, end, y, if legacy_conhost() { "_" } else { "▏" }, accent());
        }
        gui.ui.note_caret(Position { x: end, y });
    }
}

/// Skip words' line: the kit's text input — a 3-row card, its border the
/// accent while the line has the keyboard, BRIGHT under the pointer and DIM
/// at rest, the hint inside while it is empty — and `+` framed beside it
/// for the pointer: lit while there is a word on the line, a click there
/// adds it as Enter does. (The owner's call, 2026-11-10: the line was a
/// bare row, and Enter the only way to add.)
fn draw_word_card(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, input: &Input, held: bool) {
    let card = Rect { x, y, width: TEXT_W - ADD_W - 1, height: 3 };
    let tone = if held {
        th().accent
    } else if gui.ui.hovers(card) {
        th().bright
    } else {
        th().dim
    };
    crate::kit::frame_at(frame, card, tone);
    gui.ui.click(card, Act::DjSheet(SheetAct::Press(Stop::Field(0))));
    let (fx, fy) = (card.x + 2, card.y + 1);
    let room = card.width.saturating_sub(4);
    let value = input.value();
    let hint = t!("gui.dj.keyword_hint");
    if held {
        super::super::text_field(frame, &mut gui.ui, fx, fy, value, input.cursor(), room, Style::default());
        if value.is_empty() && gui.ui.composition().is_empty() {
            put(frame, fx + 1, fy, &super::super::bar::clip(&hint, room.saturating_sub(1) as usize), dim());
        }
    } else if value.is_empty() {
        put(frame, fx, fy, &super::super::bar::clip(&hint, room as usize), dim());
    } else {
        put(frame, fx, fy, &super::super::bar::clip(value, room as usize), Style::default());
    }
    // `+`: the card's height, a cell clear of it. With nothing typed it
    // rests — DIM, no click — as a tall control with nothing to do does.
    let at = Rect { x: card.right() + 1, y, width: ADD_W, height: 3 };
    if value.trim().is_empty() {
        crate::kit::tall_frame(frame, &mut gui.ui, at, "+", 1, |_| (th().dim, false), None);
    } else {
        let tone = |hovered: bool| (if hovered { th().bright } else { th().accent }, true);
        crate::kit::tall_frame(frame, &mut gui.ui, at, "+", 1, tone, Some(Act::DjSheet(SheetAct::AddWord)));
    }
}

/// One skip word and its `[X]`: a click lets it go.
fn draw_chip(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, word: &str, index: usize, hand: bool) {
    let w = width(word) as u16;
    let rect = Rect { x, y, width: w + 4, height: 1 };
    let hover = gui.ui.hovers(rect);
    let style = match (hand, hover) {
        (true, _) => accent().add_modifier(Modifier::BOLD),
        (false, true) => bright_bold(),
        (false, false) => Style::default(),
    };
    put(frame, x, y, word, style);
    let x_style = if hover || hand { Style::default().fg(th().danger).add_modifier(Modifier::BOLD) } else { dim() };
    put(frame, x + w + 1, y, "[X]", x_style);
    gui.ui.click(rect, Act::DjSheet(SheetAct::Press(Stop::Chip(index))));
}

/// The Genres sheet's inside: the way the list cuts, the kind of genre it
/// matches where the server has both, the search line, and the list — the
/// chosen first, checkboxes toggling the draft. Returns the foot's note.
fn draw_genres(frame: &mut Frame, gui: &mut Gui, rect: Rect, open: &Open) -> Option<String> {
    let Draft::Genres { mode, source, chosen, pinned, filter, row } = &open.draft else { return None };
    let hand = |stop: Stop| open.shown && open.hand == stop;
    let (x0, mut y) = (rect.x + 2, rect.y + 1);
    let radios = radio_glyphs();
    let modes = [
        (GenreMode::Whitelist, t!("gui.dj.genres_only").to_string()),
        (GenreMode::Blacklist, t!("gui.dj.genres_skip").to_string()),
    ];
    for (i, (kind, word)) in modes.iter().enumerate() {
        let stop = Stop::Radio(i as u8);
        let x = if i == 0 { x0 } else { x0 + COL2 };
        draw_choice(frame, gui, x, y, radios, mode == kind, accent(), word, hand(stop), true, SheetAct::Press(stop));
    }
    y += 1;
    let kinds = kinds_offered(gui);
    if kinds {
        for (i, kind) in KINDS.iter().enumerate() {
            let stop = Stop::Kind(i as u8);
            let x = x0 + [0, COL2, COL3][i];
            let word = kind_word(*kind);
            draw_choice(frame, gui, x, y, radios, source == kind, accent(), &word, hand(stop), true, SheetAct::Press(stop));
        }
        y += 1;
    }
    y += 1;
    // The search line: empty, its placeholder and no caret, yet every key
    // the sheet does not claim is its own — it says it has the keyboard
    // all the same, so the window's input method is on for its first key.
    let query = filter.value().to_string();
    let field = Rect { x: x0, y, width: TEXT_W, height: 1 };
    if query.is_empty() && gui.ui.composition().is_empty() {
        put(frame, x0, y, &super::super::bar::clip(&t!("gui.dj.search_genres"), TEXT_W as usize), dim());
        gui.ui.note_caret(Position { x: x0, y });
    } else {
        super::super::text_field(frame, &mut gui.ui, x0, y, &query, filter.cursor(), TEXT_W, Style::default());
    }
    gui.ui.click(field, Act::DjSheet(SheetAct::Press(Stop::List)));
    y += 1;

    let list_h = genre_list_rows(frame.area().height, kinds) as usize;
    let note = Some(t!("gui.dj.selected_count", count = chosen.len()).to_string());
    let Gui { app, dj, ui, .. } = gui;
    let Some(picker) = app.dj_panel.genres.as_ref() else { return note };
    if picker.loading && picker.all.is_empty() && pinned.is_empty() {
        put(frame, x0, y, &t!("gui.dj.genres_loading"), dim());
        return note;
    }
    if let Some(err) = &picker.failed {
        put(frame, x0, y, &t!("gui.dj.genres_failed"), Style::default().fg(th().gold));
        put(frame, x0, y + 1, &super::super::bar::clip(err, TEXT_W as usize), dim());
        return note;
    }
    if picker.all.is_empty() && pinned.is_empty() {
        put(frame, x0, y, &t!("gui.dj.no_genres"), dim());
        return note;
    }
    let names = genre_rows(&picker.all, pinned, &query);
    if names.is_empty() {
        let words = t!("gui.dj.no_genre_match", query = query.trim()).to_string();
        put(frame, x0, y, &super::super::bar::clip(&words, TEXT_W as usize), dim());
        return note;
    }
    let cursor = (*row).min(names.len() - 1);
    let (first, _) = dj.list.window(names.len(), Some(cursor), list_h);
    let overflow = names.len() > list_h;
    let row_w = if overflow { TEXT_W - 2 } else { TEXT_W };
    let (check_on, check_off) = super::super::check_glyphs();
    let held = open.shown && open.hand == Stop::List;
    for (i, name) in names.iter().enumerate().skip(first).take(list_h) {
        let ry = y + (i - first) as u16;
        let line = Rect { x: x0, y: ry, width: row_w, height: 1 };
        let on = chosen.iter().any(|g| g == name);
        let is_sel = held && i == cursor;
        let hover = ui.hovers(line);
        if is_sel {
            frame.render_widget(Block::default().style(sel()), line);
        }
        let glyph_style = match (is_sel, on) {
            (true, _) => sel(),
            (false, true) => Style::default().fg(th().ok),
            (false, false) => dim(),
        };
        put(frame, x0, ry, if on { check_on } else { check_off }, glyph_style);
        let style = match (is_sel, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        put(frame, x0 + 4, ry, &super::super::bar::clip(name, row_w.saturating_sub(5) as usize), style);
        ui.click(line, Act::DjSheet(SheetAct::Genre(name.to_string())));
    }
    if overflow {
        let bar = Rect { x: x0 + TEXT_W - 1, y, width: 1, height: list_h as u16 };
        scroll_list(
            frame,
            ui,
            bar,
            names.len(),
            list_h,
            first,
            Act::ScrollBy(List::DjSheet, -1),
            Act::ScrollBy(List::DjSheet, 1),
            |first| Act::ScrollTo(List::DjSheet, first),
        );
    }
    note
}

/// Play from's inside: the servers — one is ticked, a peer under its
/// parent — and, under them, the folders of the one the rows are about.
/// More lines than the sheet affords stand behind the list's window: the
/// wheel and the kit's scrollbar move it, and a walking key brings the
/// hand's line into view.
fn draw_play_from(frame: &mut Frame, gui: &mut Gui, rect: Rect, open: &Open) {
    let Draft::PlayFrom { servers, chosen, home, libraries, off } = &open.draft else { return };
    let hand = |stop: Stop| open.shown && open.hand == stop;
    let (x0, top) = (rect.x + 2, rect.y + 1);
    let lines = play_lines(&open.draft);
    // The frame's two rows and the foot's two are not the list's.
    let visible = rect.height.saturating_sub(4) as usize;
    let hand_line = lines.iter().position(|line| match (*line, open.hand) {
        (PlayLine::Server(i), Stop::Server(j)) | (PlayLine::Library(i), Stop::Check(j)) => i == j,
        _ => false,
    });
    let (first, _) = gui.dj.list.window(lines.len(), hand_line, visible);
    let overflow = lines.len() > visible;
    let radios = radio_glyphs();
    let checks = super::super::check_glyphs();
    let ok = Style::default().fg(th().ok);
    let branch = if legacy_conhost() { "+" } else { "└" };
    let room = TEXT_W as usize - 4 - if overflow { 2 } else { 0 };
    for (n, line) in lines.iter().enumerate().skip(first).take(visible) {
        let y = top + (n - first) as u16;
        match *line {
            PlayLine::Gap => {}
            PlayLine::Server(i) => {
                let Some(server) = servers.get(i) else { continue };
                let stop = Stop::Server(i);
                let word = if server.peer { format!("{branch} {}", server.label) } else { server.label.clone() };
                let word = super::super::bar::clip(&word, room).into_owned();
                draw_choice(frame, gui, x0, y, radios, i == *chosen, accent(), &word, hand(stop), true, SheetAct::Press(stop));
            }
            PlayLine::Name => {
                let name =
                    servers.get(*home).map(|server| server.label.clone()).unwrap_or_else(|| super::target_name(gui));
                put(frame, x0, y, &super::super::bar::clip(&name, room + 4), dim());
            }
            PlayLine::Library(i) => {
                let Some(library) = libraries.get(i) else { continue };
                let stop = Stop::Check(i);
                let word = super::super::bar::clip(library, room).into_owned();
                let on = !off.contains(library);
                draw_choice(frame, gui, x0, y, checks, on, ok, &word, hand(stop), true, SheetAct::Press(stop));
            }
        }
    }
    if overflow {
        let bar = Rect { x: x0 + TEXT_W - 1, y: top, width: 1, height: visible as u16 };
        scroll_list(
            frame,
            &mut gui.ui,
            bar,
            lines.len(),
            visible,
            first,
            Act::ScrollBy(List::DjSheet, -1),
            Act::ScrollBy(List::DjSheet, 1),
            |first| Act::ScrollTo(List::DjSheet, first),
        );
    }
}

/// Preview's inside (clause 53): three picks with the current settings,
/// none of them queued — the badge, the title, the artist, the length —
/// and the pool they were drawn from as the foot's note.
fn draw_preview(frame: &mut Frame, gui: &mut Gui, rect: Rect) -> Option<String> {
    let (x0, y) = (rect.x + 2, rect.y + 1);
    let panel = &gui.app.dj_panel;
    if panel.sample_pending {
        put(frame, x0, y, &t!("gui.dj.picking"), accent());
        return None;
    }
    let sonic = gui.app.dj_sonic_usable() && panel.pool.is_some();
    let badge = match (legacy_conhost(), sonic) {
        (true, _) => "~",
        (false, true) => "≈",
        (false, false) => "∞",
    };
    for (i, track) in panel.sample.iter().take(3).enumerate() {
        let ry = y + i as u16;
        put(frame, x0, ry, badge, dim());
        put(frame, x0 + 2, ry, &super::super::bar::clip(track.title_or_file(), 27), Style::default());
        let artist = track.metadata.artist.as_deref().unwrap_or_default();
        put(frame, x0 + 30, ry, &super::super::bar::clip(artist, 24), dim());
        if let Some(seconds) = track.metadata.duration.filter(|d| d.is_finite() && *d > 0.0) {
            let length = dj::fmt_length(seconds.round() as u32);
            put(frame, (x0 + TEXT_W).saturating_sub(width(&length) as u16), ry, &length, dim());
        }
    }
    panel.pool.as_ref().map(|pool| t!("gui.dj.pool", n = pool.pool_size).to_string())
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// A bar stepped, or set: held inside its range.
fn bar_set(draft: &mut Draft, value: u32) {
    let Some(spec) = bar_of(draft) else { return };
    let value = value.clamp(spec.lo, spec.hi);
    match draft {
        Draft::Batch { songs } => *songs = value,
        Draft::Tempo { tolerance, .. } => *tolerance = value,
        Draft::Cooldown { artists } => *artists = value,
        Draft::Sonic { floor, .. } => *floor = value,
        _ => {}
    }
}

fn bar_step(draft: &mut Draft, delta: i32) {
    let Some(spec) = bar_of(draft) else { return };
    bar_set(draft, (spec.value as i32 + delta).max(0) as u32);
}

/// A typed length stepped by fifteen seconds: an empty one counts from
/// nothing, and one stepped down to nothing is empty again.
fn length_step(text: &mut String, delta: i32) {
    let now = match dj::parse_length(text) {
        Some(seconds) => seconds,
        None if text.trim().is_empty() => 0,
        None => return,
    };
    let step = dj::LENGTH_STEP_SECONDS as i32;
    let next = (now as i32 + delta * step).clamp(0, dj::LENGTH_RAIL_SECONDS as i32) as u32;
    *text = if next == 0 { String::new() } else { dj::fmt_length(next) };
}

/// The typed line becomes a word: trimmed, once whatever its case, fifty
/// at most.
fn add_word(draft: &mut Draft) {
    let Draft::SkipWords { words, input } = draft else { return };
    let word = input.value().trim().to_string();
    if word.is_empty() {
        return;
    }
    if !words.iter().any(|w| w.eq_ignore_ascii_case(&word)) && words.len() < dj::KEYWORDS_MAX {
        words.push(word);
    }
    *input = Input::default();
}

/// A genre ticked or let go, two hundred at most.
fn toggle_genre(draft: &mut Draft, name: &str) {
    let Draft::Genres { chosen, .. } = draft else { return };
    match chosen.iter().position(|g| g == name) {
        Some(at) => {
            chosen.remove(at);
        }
        None if chosen.len() < dj::GENRES_MAX => chosen.push(name.to_string()),
        None => {}
    }
}

/// The genre under the list's cursor.
fn genre_under(gui: &Gui, draft: &Draft) -> Option<String> {
    let Draft::Genres { pinned, filter, row, .. } = draft else { return None };
    let picker = gui.app.dj_panel.genres.as_ref()?;
    let names = genre_rows(&picker.all, pinned, filter.value());
    names.get((*row).min(names.len().saturating_sub(1))).map(|name| name.to_string())
}

fn genre_count(gui: &Gui, draft: &Draft) -> usize {
    let Draft::Genres { pinned, filter, .. } = draft else { return 0 };
    gui.app.dj_panel.genres.as_ref().map_or(0, |picker| genre_rows(&picker.all, pinned, filter.value()).len())
}

/// `stop` pressed on the draft. True when the press closes the sheet.
fn press(gui: &mut Gui, open: &mut Open, stop: Stop) -> bool {
    match (&mut open.draft, stop) {
        (_, Stop::Done) => return true,
        (Draft::Sonic { on, .. } | Draft::Tempo { on, .. }, Stop::Switch) => *on = !*on,
        (Draft::Sonic { anchor, .. }, Stop::Radio(i)) => {
            *anchor = if i == 0 { SonicAnchor::Rolling } else { SonicAnchor::Locked };
        }
        (Draft::Genres { mode, .. }, Stop::Radio(i)) => {
            *mode = if i == 0 { GenreMode::Whitelist } else { GenreMode::Blacklist };
        }
        (Draft::Genres { source, pinned, chosen, row, .. }, Stop::Kind(i)) => {
            let kind = KINDS[usize::from(i).min(KINDS.len() - 1)];
            if *source != kind {
                *source = kind;
                // The list is another one now: what is chosen stands first.
                *pinned = chosen.clone();
                pinned.sort_by_key(|g| g.to_lowercase());
                *row = 0;
                gui.dj.list = ListView::default();
                let effects = gui.app.dj_genres_ask(kind);
                gui.pend(effects);
            }
        }
        (Draft::Genres { .. }, Stop::List) => {
            if let Some(name) = genre_under(gui, &open.draft) {
                toggle_genre(&mut open.draft, &name);
            }
        }
        (Draft::Genres { chosen, .. }, Stop::Clear) => chosen.clear(),
        (Draft::Rating { rating }, Stop::Clear) => *rating = 0,
        (Draft::Length { min, max, unknown }, Stop::Clear) => {
            min.clear();
            max.clear();
            *unknown = false;
        }
        (Draft::Length { unknown, .. }, Stop::Check(_)) => *unknown = !*unknown,
        (Draft::SkipWords { words, .. }, Stop::Clear) => words.clear(),
        (Draft::SkipWords { words, .. }, Stop::Chip(i)) => {
            if i < words.len() {
                words.remove(i);
            }
        }
        (Draft::PlayFrom { chosen, .. }, Stop::Server(i)) => *chosen = i,
        (Draft::PlayFrom { libraries, off, .. }, Stop::Check(i)) => {
            let Some(library) = libraries.get(i) else { return false };
            match off.iter().position(|name| name == library) {
                Some(at) => {
                    off.remove(at);
                }
                // The last one on stays on (clause 42).
                None if off.len() + 1 >= libraries.len() => {
                    gui.note = Some((t!("dj.one_source").to_string(), true));
                }
                None => off.push(library.clone()),
            }
        }
        _ => {}
    }
    false
}

/// Where a walking key takes the hand: ↑ ↓ the row above or below, the
/// place in the row kept where it can be; ← → the stop beside; Tab and
/// Shift-Tab the next or previous stop in reading order, wrapping.
fn walk(rows: &[Vec<Stop>], hand: Stop, key: KeyCode) -> Stop {
    let Some((r, c)) = locate(rows, hand) else {
        return rows.first().and_then(|row| row.first()).copied().unwrap_or(Stop::Done);
    };
    let land = |row: &Vec<Stop>| row[c.min(row.len() - 1)];
    match key {
        KeyCode::Up if r > 0 => land(&rows[r - 1]),
        KeyCode::Down if r + 1 < rows.len() => land(&rows[r + 1]),
        KeyCode::Left if c > 0 => rows[r][c - 1],
        KeyCode::Right if c + 1 < rows[r].len() => rows[r][c + 1],
        KeyCode::Tab | KeyCode::BackTab => {
            let order: Vec<Stop> = rows.iter().flatten().copied().collect();
            let at = order.iter().position(|s| *s == hand).unwrap_or(0);
            let n = order.len();
            if key == KeyCode::Tab { order[(at + 1) % n] } else { order[(at + n - 1) % n] }
        }
        _ => hand,
    }
}

/// The sheet's keys while it is up — every key but Ctrl-C, which quits
/// first: the room beneath is inert. `None` while no sheet is up.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    let mut open = gui.dj.sheet.clone()?;
    let rows = stop_rows(&open, gui);
    let code = match key.code {
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
        code => code,
    };
    let typing = !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let mut close = false;
    match (code, open.hand) {
        (KeyCode::Esc, _) => close = true,
        // `clear` while the hand shows on it; a typed word added; Done from
        // anywhere else.
        (KeyCode::Enter, Stop::Clear) if open.shown => {
            press(gui, &mut open, Stop::Clear);
        }
        (KeyCode::Enter, _) => match &open.draft {
            Draft::SkipWords { input, .. } if !input.value().trim().is_empty() => {
                add_word(&mut open.draft);
                open.hand = Stop::Field(0);
            }
            _ => close = true,
        },
        // The Genres list: its own rows first, then the stops around it.
        (KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown, Stop::List) if open.shown => {
            let count = genre_count(gui, &open.draft);
            let page = genre_list_rows(gui.last_height, kinds_offered(gui)) as usize;
            if let Draft::Genres { row, .. } = &mut open.draft {
                let last = count.saturating_sub(1);
                let at = (*row).min(last);
                match code {
                    KeyCode::Up if at == 0 => open.hand = walk(&rows, Stop::List, KeyCode::Up),
                    KeyCode::Down if at >= last => open.hand = walk(&rows, Stop::List, KeyCode::Down),
                    KeyCode::Up => *row = at - 1,
                    KeyCode::Down => *row = at + 1,
                    KeyCode::PageUp => *row = at.saturating_sub(page),
                    _ => *row = (at + page).min(last),
                }
                gui.dj.list.reveal = true;
            }
        }
        // A bar, the stars and a typed length move under ← →.
        (KeyCode::Left | KeyCode::Right, Stop::Bar) if open.shown => {
            bar_step(&mut open.draft, if code == KeyCode::Left { -1 } else { 1 });
        }
        (KeyCode::Left | KeyCode::Right, Stop::Stars) if open.shown => {
            if let Draft::Rating { rating } = &mut open.draft {
                let delta = if code == KeyCode::Left { -1 } else { 1 };
                *rating = (*rating as i32 + delta).clamp(0, dj::RATING_MAX as i32) as u32;
            }
        }
        (KeyCode::Left | KeyCode::Right, Stop::Field(i)) if matches!(open.draft, Draft::Length { .. }) => {
            if let Draft::Length { min, max, .. } = &mut open.draft {
                length_step(if i == 0 { min } else { max }, if code == KeyCode::Left { -1 } else { 1 });
            }
        }
        // Skip words' line edits itself: ← → and the rest are its own.
        (KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End | KeyCode::Backspace | KeyCode::Delete, Stop::Field(_))
            if matches!(open.draft, Draft::SkipWords { .. }) =>
        {
            if let Draft::SkipWords { input, .. } = &mut open.draft {
                input.handle_event(&TermEvent::Key(key));
            }
        }
        (KeyCode::Backspace, Stop::Field(i)) => {
            if let Draft::Length { min, max, .. } = &mut open.draft {
                (if i == 0 { min } else { max }).pop();
            }
        }
        (KeyCode::Delete, Stop::Field(i)) => {
            if let Draft::Length { min, max, .. } = &mut open.draft {
                (if i == 0 { min } else { max }).clear();
            }
        }
        (KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace, Stop::Chip(_)) if open.shown => {
            let held = open.hand;
            press(gui, &mut open, held);
        }
        (KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab, _) => {
            // A stowed hand is only shown by the first walking key.
            if open.shown {
                open.hand = walk(&rows, open.hand, code);
            }
            open.shown = true;
            // Play from's lines may stand behind its window: the hand's
            // comes into view.
            if matches!(open.draft, Draft::PlayFrom { .. }) {
                gui.dj.list.reveal = true;
            }
        }
        (KeyCode::Char(c), hand) if typing => match (&mut open.draft, hand) {
            // A length is typed at its end: digits and the colon.
            (Draft::Length { min, max, .. }, Stop::Field(i)) if c.is_ascii_digit() || c == ':' => {
                let text = if i == 0 { min } else { max };
                if text.chars().count() < LENGTH_MAX {
                    text.push(c);
                }
            }
            // Every letter is Skip words' line's, whatever the hand is on.
            (Draft::SkipWords { input, .. }, hand) if c != ' ' || hand == Stop::Field(0) => {
                if input.value().chars().count() < WORD_MAX {
                    input.handle_event(&TermEvent::Key(key));
                }
                open.hand = Stop::Field(0);
            }
            // And the Genres list's search line's — Space ticks the row.
            (Draft::Genres { filter, row, .. }, _) if c != ' ' => {
                if filter.handle_event(&TermEvent::Key(key)).is_some_and(|change| change.value) {
                    *row = 0;
                    gui.dj.list.scroll = 0;
                }
                open.hand = Stop::List;
                open.shown = true;
            }
            (Draft::Rating { rating }, Stop::Stars) if c.is_ascii_digit() && c <= '5' => {
                *rating = 2 * c.to_digit(10).unwrap_or(0);
            }
            (_, hand) if c == ' ' => {
                if open.shown {
                    close = press(gui, &mut open, hand);
                }
                open.shown = true;
            }
            _ => {}
        },
        (KeyCode::Backspace, _) => {
            if let Draft::Genres { filter, row, .. } = &mut open.draft
                && filter.handle_event(&TermEvent::Key(key)).is_some_and(|change| change.value)
            {
                *row = 0;
                gui.dj.list.scroll = 0;
            }
        }
        _ => {}
    }
    settle(&mut open, gui);
    gui.dj.sheet = Some(open);
    if close {
        done(gui);
    }
    Some(false)
}

/// The pointer's acts.
pub(crate) fn act(gui: &mut Gui, act: SheetAct) {
    let Some(mut open) = gui.dj.sheet.clone() else { return };
    let mut close = false;
    // Pressed, and the hand stowed there: the next walking key shows it on
    // this stop. A field keeps the keyboard either way.
    open.shown = false;
    match act {
        SheetAct::Press(stop) => {
            open.hand = stop;
            if !matches!(stop, Stop::Field(_)) {
                close = press(gui, &mut open, stop);
            }
        }
        SheetAct::Step(Stop::Field(i), delta) => {
            open.hand = Stop::Field(i);
            if let Draft::Length { min, max, .. } = &mut open.draft {
                length_step(if i == 0 { min } else { max }, delta);
            }
        }
        SheetAct::Step(_, delta) => {
            open.hand = Stop::Bar;
            bar_step(&mut open.draft, delta);
        }
        SheetAct::Set(value) => {
            open.hand = Stop::Bar;
            bar_set(&mut open.draft, value);
        }
        SheetAct::Star(i) => {
            open.hand = Stop::Stars;
            if let Draft::Rating { rating } = &mut open.draft {
                let full = 2 * (i + 1);
                *rating = if *rating == full { full - 1 } else { full };
            }
        }
        SheetAct::Genre(name) => {
            open.hand = Stop::List;
            toggle_genre(&mut open.draft, &name);
        }
        // The line keeps the keyboard, cleared for the next word.
        SheetAct::AddWord => {
            open.hand = Stop::Field(0);
            add_word(&mut open.draft);
        }
    }
    settle(&mut open, gui);
    gui.dj.sheet = Some(open);
    if close {
        done(gui);
    }
}

/// The wheel over a sheet: its list's — Genres' genres, Play from's
/// servers and folders (the next frame clamps the step). True while a sheet
/// is up.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) -> bool {
    let Some(open) = gui.dj.sheet.as_ref() else { return false };
    if matches!(open.sheet, Sheet::Genres | Sheet::PlayFrom) {
        gui.dj.list.step(delta);
    }
    true
}

/// The tips line while a sheet is up.
pub(crate) fn tips(gui: &Gui) -> Option<String> {
    let open = gui.dj.sheet.as_ref()?;
    let tip = match open.sheet {
        Sheet::Genres => t!("gui.tips.dj_genres"),
        Sheet::SkipWords => t!("gui.tips.dj_words"),
        Sheet::Length => t!("gui.tips.dj_length"),
        _ => t!("gui.tips.dj_sheet"),
    };
    Some(tip.to_string())
}
