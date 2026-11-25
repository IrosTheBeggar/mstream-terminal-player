//! The Advanced options sheet drawn by egui (contract clauses 33–43; its
//! cells' face is cells/sheet.rs), after the canvas's Advanced.dc.html: the
//! kit's Dialog over the page's area and its scrim, the page inert beneath,
//! for the board in view. Its three groups — the firmware's choices with
//! what was picked under them, the flash mode with its marks by source, the
//! erase box — then the help and the foot, each in the words the cells give
//! it under the same conditions (their `sheet_lines`, `mode_lines`,
//! `release_facts`, `help_text` and `list_rows`, said again here for the
//! rows in points), and over its lower rows the release list while it
//! hangs open.
//!
//! The keyboard is the model's: Tab, the arrows, Space, Enter and Esc still
//! walk the sheet's stops, move the list's cursor and edit the path field
//! (page/sheet.rs), and egui draws where they are — the chosen mark in the
//! accent in the group with the keyboard, the release control and the
//! keyboard's chooser in the accent, the erase line and the keyboard's
//! button ringed, the list's cursor lit. A click asks for the act the cells
//! register for the same control, and only while they register it: under
//! the hanging list a click anywhere but on its rows only closes it, as the
//! cells' close is registered over the whole area. A chooser that opens a
//! system dialog asks the model, whose dialogs run on a thread of their own.
//!
//! The window is held where its tallest state would stand (`Dialog::hold`),
//! so rows that come and go under a choice move only its foot. Where the
//! window is too short for the boards' measures the rows close up together,
//! toward a tighter set, as far as the tallest state needs and no further,
//! so no row moves between one state and the next; the help gives way in a
//! state with no room left for it, as the cells' gives way first; and short
//! of even that, the body scrolls, the keyboard's stop brought into view.

use std::path::Path;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, Id, Pos2, Rect, Sense, Ui, pos2, vec2};

use super::super::sheet::{Chooser, Entry, Local, Picked, Source, Stop};
use super::super::*;
use super::{Drawn, Face, close_tip};
use crate::device::firmware::{Images, Release, clock_at, header_mhz};
use crate::gui::window::studio::{self, Dialog, Pal, Para, body, mono, strong};

/// The sheet's window: the board's 640 points.
const SHEET_W: f32 = 640.0;
/// A group's label row (the kit's small capitals), and a dim line's row: a
/// choice's facts, the mode's note, the help's.
const LABEL_H: f32 = 14.0;
const LINE_H: f32 = 18.0;
/// Where a choice's name and its lines start: the kit's choice's words,
/// past its 14-point mark and the 10 after it.
const NAME_IN: f32 = studio::RADIO_IN;
/// The room between a choice's name and what follows it, its detail or the
/// release control.
const DETAIL_GAP: f32 = 10.0;
/// The choosers, the boards' text buttons 24 points apart.
const APART: f32 = 24.0;
/// Where a refusal's words start: past the cross and the 8 after it.
const MARK_IN: f32 = 20.0;
/// The release control's height at most: the kit's closed list's box, in a
/// radio row 4 points taller.
const CONTROL_H: f32 = 26.0;
/// The release control's words: a choice's detail's 13 points.
const CONTROL_SIZE: f32 = 13.0;
/// The release list's lead column (a picked release's LED and the 8 after
/// it), and the room between its columns.
const LEAD_W: f32 = 14.0;
const COLUMN_GAP: f32 = 16.0;

// ── The measures ──────────────────────────────────────────────────────────

/// The sheet's vertical measures: the boards' where the window has the room
/// for its tallest state, closed up toward [`TIGHT`] where it has not.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Measures {
    /// Between the groups, the help and the foot.
    gap: f32,
    /// Between a group's label and its rows.
    label_gap: f32,
    /// A radio row, and between two.
    row: f32,
    row_gap: f32,
    /// Between a choice's lines and its choosers or its field.
    sub_gap: f32,
    chooser: f32,
    field: f32,
    /// Between the mode's rows and their note.
    note_gap: f32,
    /// The erase line.
    check: f32,
    /// Between the help's rule and its words.
    help_gap: f32,
}

/// The board's measures (Advanced.dc.html): groups 20 points apart, a
/// label 6 over its rows, rows 30 tall and 2 apart, the choosers 4 under
/// the build's facts, the field the kit's 34, the erase line the kit's 28,
/// the help 10 under its rule.
const ROOMY: Measures = Measures {
    gap: 20.0,
    label_gap: 6.0,
    row: 30.0,
    row_gap: 2.0,
    sub_gap: 4.0,
    chooser: 28.0,
    field: 34.0,
    note_gap: 4.0,
    check: 28.0,
    help_gap: 10.0,
};

/// The closest the rows come: the GUI's floor.
const TIGHT: Measures = Measures {
    gap: 6.0,
    label_gap: 2.0,
    row: 22.0,
    row_gap: 0.0,
    sub_gap: 2.0,
    chooser: 22.0,
    field: 26.0,
    note_gap: 2.0,
    check: 22.0,
    help_gap: 6.0,
};

impl Measures {
    /// `t` of the way from [`ROOMY`] to [`TIGHT`], each measure a whole
    /// number of points (no hairline falls between pixels), never taller
    /// than the share asked for.
    fn at(t: f32) -> Measures {
        let t = t.clamp(0.0, 1.0);
        let k = |a: f32, b: f32| (a + (b - a) * t).floor();
        let (r, s) = (ROOMY, TIGHT);
        Measures {
            gap: k(r.gap, s.gap),
            label_gap: k(r.label_gap, s.label_gap),
            row: k(r.row, s.row),
            row_gap: k(r.row_gap, s.row_gap),
            sub_gap: k(r.sub_gap, s.sub_gap),
            chooser: k(r.chooser, s.chooser),
            field: k(r.field, s.field),
            note_gap: k(r.note_gap, s.note_gap),
            check: k(r.check, s.check),
            help_gap: k(r.help_gap, s.help_gap),
        }
    }

    /// The measures for a sheet whose tallest state, its help aside, is
    /// `tallest` with `budget` points of room: the boards' where it fits,
    /// else closed up just as far as it needs, else the tightest.
    fn for_room(tallest: &Shape, budget: f32) -> Measures {
        let (a, c) = (tallest.height(&ROOMY), tallest.height(&TIGHT));
        if a <= budget + 0.01 {
            return ROOMY;
        }
        if c >= budget - 0.01 || a - c < 0.01 {
            return TIGHT;
        }
        Measures::at((a - budget) / (a - c))
    }
}

/// What stands under the local build's row: nothing, its choosers, or the
/// path field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Under {
    Nothing,
    Choosers,
    Field,
}

/// The rows that come and go in the sheet, which its height follows.
#[derive(Clone, Copy, Debug)]
struct Shape {
    /// The picked release's facts under Another release.
    release_line: bool,
    /// What was read of a local build, under its row.
    local_lines: usize,
    under: Under,
    /// The mode's note under its rows.
    mode_note: bool,
    /// The help's lines; none, and the help with its rule gives way.
    help_lines: usize,
    foot_h: f32,
}

impl Shape {
    /// The tallest the sheet stands, which its window is held for: a local
    /// build read in two lines with the path field open under them and the
    /// mode's note, the help in its two lines.
    fn tallest(foot_h: f32) -> Shape {
        Shape { release_line: false, local_lines: 2, under: Under::Field, mode_note: true, help_lines: 2, foot_h }
    }

    fn firmware(&self, m: &Measures) -> f32 {
        let mut h = LABEL_H + m.label_gap + 3.0 * m.row + 2.0 * m.row_gap;
        if self.release_line {
            h += LINE_H;
        }
        h += self.local_lines as f32 * LINE_H;
        let under = match self.under {
            Under::Nothing => 0.0,
            Under::Choosers => m.chooser,
            Under::Field => m.field,
        };
        if under > 0.0 && self.local_lines > 0 {
            h += m.sub_gap;
        }
        h + under
    }

    fn mode(&self, m: &Measures) -> f32 {
        let note = if self.mode_note { m.note_gap + LINE_H } else { 0.0 };
        LABEL_H + m.label_gap + 2.0 * m.row + m.row_gap + note
    }

    fn erase(&self, m: &Measures) -> f32 {
        LABEL_H + m.label_gap + m.check
    }

    fn help(&self, m: &Measures) -> f32 {
        if self.help_lines == 0 { 0.0 } else { 1.0 + m.help_gap + self.help_lines as f32 * LINE_H }
    }

    /// The body's height: the three groups, the help where it stands, the
    /// foot.
    fn height(&self, m: &Measures) -> f32 {
        let help = if self.help_lines == 0 { 0.0 } else { m.gap + self.help(m) };
        self.firmware(m) + m.gap + self.mode(m) + m.gap + self.erase(m) + help + m.gap + self.foot_h
    }
}

// ── What the sheet says ───────────────────────────────────────────────────

/// A radio row as the cells lay one out (cells/sheet.rs's `Radio`).
struct Radio {
    on: bool,
    /// Its group has the keyboard: its chosen mark in the accent.
    focused: bool,
    /// A choice the source has; dim when not.
    usable: bool,
    name: String,
    detail: Detail,
    act: Option<Act>,
}

/// What follows a radio's name.
enum Detail {
    /// Its description, dim, after a dash.
    Words(String),
    /// A picked path, in the text colour after a dim dash, cut at its front.
    Path(String),
    /// The release control: its words, and whether the keyboard is on it.
    Control(String, bool),
}

/// What was read of a local build, under its row.
enum Read {
    Nothing,
    Reading(String),
    /// Its two facts lines, and its file's name, a figure in them.
    Facts(String, String, String),
    /// Why it is not one the tab writes, in gold behind the cross.
    Refused(String),
}

/// A row of the release list, as the cells list them (`list_rows`).
struct ListRow {
    said: Said,
    /// The pickable entry it is, by its place among the page's entries.
    pick: Option<usize>,
    /// The release the sheet has picked: its lead's LED.
    picked: bool,
}

enum Said {
    /// GitHub being asked: the kit's searching shape and its words.
    Asking(String),
    /// Why the list did not come, in gold behind the cross.
    Failed(String),
    /// A release: its tag, its date and its builds, in their columns.
    Release { tag: String, date: String, words: String },
    /// A line in the rows' column: the pre-releases' switch, a release on
    /// this computer, Try again, or (dim) none and the time it was asked.
    Words { words: String, dim: bool },
}

impl Said {
    /// It stands in the rows' column, after the lead's.
    fn in_column(&self) -> bool {
        matches!(self, Said::Release { .. } | Said::Words { .. })
    }
}

/// The release list while it hangs open: its rows, the keyboard's cursor
/// among the picks, and every release's tag and date, which its columns
/// are as wide as, shown or not.
struct List {
    rows: Vec<ListRow>,
    cursor: usize,
    tags: Vec<String>,
    dates: Vec<String>,
}

/// The sheet's words and states for a pass, read off the model as the
/// cells read them (cells/sheet.rs's `sheet_lines`).
struct View {
    title: String,
    groups: [String; 3],
    firmware: [Radio; 3],
    release_line: Option<String>,
    read: Read,
    under: Under,
    /// The choosers on offer, the keyboard's one while the row has it, a
    /// system dialog up, and why none can open here.
    choosers: Vec<Chooser>,
    chooser_focus: Option<Chooser>,
    dialog: bool,
    no_dialog: Option<String>,
    modes: [Radio; 2],
    mode_note: Option<String>,
    erase: bool,
    erase_focus: bool,
    help: String,
    can_apply: bool,
    /// The buttons have the keyboard: whether on Apply.
    buttons_focus: Option<bool>,
    list: Option<List>,
    /// Where the keyboard is, as [`studio::reveal`] numbers it.
    stop: usize,
}

impl View {
    fn of(page: &Page, sheet: &Sheet, board: &Board) -> View {
        let pin = page.target.as_ref().map_or("?", |t| t.version.as_str()).to_string();
        let in_list = sheet.list.is_some();
        let typing = sheet.field.is_some();
        let focus = |stop: Stop| sheet.focus == stop && !in_list && !typing;
        let source = sheet.source;
        let pin_row = Radio {
            on: source == Source::Pin,
            focused: focus(Stop::Firmware),
            usable: true,
            name: pin,
            detail: Detail::Words(t!("dev.adv_pin_desc").to_string()),
            act: Some(Act::SheetSource(0)),
        };
        let release_detail = if source == Source::Release {
            let value = sheet.picked.as_ref().map_or_else(|| t!("dev.adv_release_pick").to_string(), |p| p.tag.clone());
            Detail::Control(value, focus(Stop::Sub))
        } else {
            Detail::Words(t!("dev.adv_release_desc").to_string())
        };
        let release_row = Radio {
            on: source == Source::Release,
            focused: focus(Stop::Firmware),
            usable: true,
            name: t!("dev.adv_release").to_string(),
            detail: release_detail,
            act: Some(Act::SheetSource(1)),
        };
        let release_line = match (&sheet.picked, source) {
            (Some(picked), Source::Release) => Some(release_facts(page, board, picked)),
            _ => None,
        };
        let path = match &sheet.local {
            Local::Reading(path) | Local::Vetted(path, _) | Local::Refused(path, _) if source == Source::Local => {
                Some(path.display().to_string())
            }
            _ => None,
        };
        let local_row = Radio {
            on: source == Source::Local,
            focused: focus(Stop::Firmware),
            usable: true,
            name: t!("dev.adv_local").to_string(),
            detail: path.map_or_else(|| Detail::Words(t!("dev.adv_local_desc").to_string()), Detail::Path),
            act: Some(Act::SheetSource(2)),
        };
        let read = match &sheet.local {
            _ if source != Source::Local => Read::Nothing,
            Local::Vetted(_, facts) => {
                let file = facts.file.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().to_string());
                let file = file.unwrap_or_default();
                let bytes = fmt_count(facts.bytes as u64);
                let first = t!("dev.adv_local_facts", file = file, version = facts.version, bytes = bytes).to_string();
                let mode = facts.mode.map_or_else(|| t!("dev.mode_unknown").to_string(), |m| m.word().to_string());
                Read::Facts(first, t!("dev.adv_local_kind", mode = mode).to_string(), file)
            }
            Local::Refused(_, why) => Read::Refused(why.clone()),
            Local::Reading(_) => Read::Reading(t!("dev.adv_local_reading").to_string()),
            Local::Nothing => Read::Nothing,
        };
        // The field is drawn wherever it holds the keys, so its caret has a
        // home: a click on another source leaves it open, and typing.
        let under = if typing {
            Under::Field
        } else if source == Source::Local {
            Under::Choosers
        } else {
            Under::Nothing
        };
        let (modes, mode_note) = mode_rows(page, sheet, board, focus(Stop::Mode));
        let stop = if typing {
            5
        } else {
            match sheet.focus {
                Stop::Firmware => 0,
                Stop::Sub => 1,
                Stop::Mode => 2,
                Stop::Erase => 3,
                Stop::Buttons => 4,
            }
        };
        View {
            title: t!("dev.adv_title", port = short(&sheet.port)).to_string(),
            groups: [t!("dev.adv_group_firmware"), t!("dev.adv_group_mode"), t!("dev.adv_group_erase")].map(|s| s.to_string()),
            firmware: [pin_row, release_row, local_row],
            release_line,
            read,
            under,
            choosers: sheet.choosers(),
            chooser_focus: (sheet.focus == Stop::Sub && !in_list).then_some(sheet.chooser),
            dialog: sheet.dialog,
            no_dialog: sheet.no_dialog.clone(),
            modes,
            mode_note,
            erase: sheet.erase,
            erase_focus: focus(Stop::Erase),
            help: help_text(page, sheet),
            can_apply: sheet.image().is_some(),
            buttons_focus: focus(Stop::Buttons).then_some(sheet.on_apply),
            list: sheet.list.map(|cursor| list_of(page, sheet, cursor)),
            stop,
        }
    }

    /// The shape it stands in, with what was read of a local build in
    /// `read_lines` and the help in `help_lines`.
    fn shape(&self, read_lines: usize, help_lines: usize, foot_h: f32) -> Shape {
        Shape {
            release_line: self.release_line.is_some(),
            local_lines: read_lines,
            under: self.under,
            mode_note: self.mode_note.is_some(),
            help_lines,
            foot_h,
        }
    }
}

/// The Flash mode group's rows and their note (the cells' `mode_lines`):
/// the pin and a release offer the builds they have, the board's own or
/// else its default marked; a local build's is its own, and takes no click.
fn mode_rows(page: &Page, sheet: &Sheet, board: &Board, focused: bool) -> ([Radio; 2], Option<String>) {
    let port = short(board.port());
    let runs = board.mode().map(|m| m.mode).filter(|_| board.ours());
    let mut note = None;
    let rows = [Mode::Qio, Mode::Dio].map(|mode| {
        let base = t!(if mode == Mode::Qio { "dev.adv_qio_desc" } else { "dev.adv_dio_desc" }).to_string();
        let (on, usable, desc) = match sheet.source {
            Source::Pin | Source::Release => {
                let usable = sheet.offers(mode);
                let desc = if usable {
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
        Radio { on, focused, usable, name: mode.word().to_string(), detail: Detail::Words(desc), act }
    });
    (rows, note)
}

/// The line under a picked release: its date, its builds, and its
/// direction against this board (the cells' `release_facts`).
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

/// The help's words, about the group with the keyboard (the cells'
/// `help_text`).
fn help_text(page: &Page, sheet: &Sheet) -> String {
    let pin = page.target.as_ref().map_or("?", |t| t.version.as_str()).to_string();
    match (sheet.focus, sheet.source) {
        (Stop::Mode, _) if sheet.effective_mode() == Some(Mode::Dio) => t!("dev.adv_help_dio").to_string(),
        (Stop::Mode, _) => t!("dev.adv_help_qio").to_string(),
        (Stop::Erase, _) => t!("dev.adv_help_erase").to_string(),
        (_, Source::Pin) => t!("dev.adv_help_pin", pin = pin).to_string(),
        (_, Source::Release) => match &sheet.picked {
            Some(picked) if sheet.list.is_none() => t!("dev.adv_help_picked", tag = picked.tag, pin = pin).to_string(),
            _ => t!("dev.adv_help_release").to_string(),
        },
        (_, Source::Local) => match &sheet.local {
            Local::Vetted(..) => t!("dev.adv_help_local_picked").to_string(),
            Local::Refused(..) => t!("dev.adv_help_refused").to_string(),
            _ => t!("dev.adv_help_local").to_string(),
        },
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

/// `2026-10-07` for a moment, by the calendar (UTC, as GitHub's dates).
fn date_of(at: std::time::SystemTime) -> Option<String> {
    let secs = at.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    let (y, m, d) = crate::admin::tz::civil_from_days(secs.div_euclid(86_400));
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// The release list's rows, as the cells list them (`list_rows`), with the
/// keyboard's `cursor` among its picks.
fn list_of(page: &Page, sheet: &Sheet, cursor: usize) -> List {
    let entries = page.entries();
    let picked = sheet.picked.as_ref().map(|p| p.tag.as_str());
    let is_picked = |entry: &Entry| match entry {
        Entry::Release(r) => Some(r.tag.as_str()) == picked,
        Entry::Cached(c) => Some(c.tag.as_str()) == picked,
        _ => false,
    };
    let row = |said: Said, pick: Option<usize>| ListRow { picked: pick.and_then(|i| entries.get(i)).is_some_and(is_picked), said, pick };
    let mut rows = Vec::new();
    let (mut tags, mut dates) = (Vec::new(), Vec::new());
    match &page.listing {
        Listing::NotAsked | Listing::Asking => rows.push(row(Said::Asking(t!("dev.adv_list_asking").to_string()), None)),
        Listing::Listed(list) => {
            tags = list.releases.iter().map(|r| r.tag.clone()).collect();
            dates = list.releases.iter().map(|r| r.date.clone()).collect();
            for (i, entry) in entries.iter().enumerate() {
                match entry {
                    Entry::Release(release) => {
                        let mut words = images_words(release.images);
                        if release.pre {
                            words = format!("{}, {words}", t!("dev.adv_pre"));
                        }
                        if page.pin_tag().is_some_and(|pin| release.newer_than(pin)) {
                            words = format!("{words} · {}", t!("dev.adv_newer"));
                        }
                        let said = Said::Release { tag: release.tag.clone(), date: release.date.clone(), words };
                        rows.push(row(said, Some(i)));
                    }
                    Entry::Toggle => {
                        let not_pin = |r: &&Release| page.pin_tag() != Some(r.tag.as_str());
                        let pre = list.releases.iter().filter(|r| r.pre).filter(not_pin).count();
                        let words = match (page.show_pre, pre) {
                            (true, _) => t!("dev.adv_list_pre_hide").to_string(),
                            (false, 1) => t!("dev.adv_list_pre_show_one").to_string(),
                            (false, n) => t!("dev.adv_list_pre_show", n = n).to_string(),
                        };
                        rows.push(row(Said::Words { words, dim: false }, Some(i)));
                    }
                    _ => {}
                }
            }
            if !entries.iter().any(|e| matches!(e, Entry::Release(_))) {
                rows.push(row(Said::Words { words: t!("dev.adv_list_none").to_string(), dim: true }, None));
            }
            let asked = list.asked.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let words = t!("dev.adv_list_asked", time = clock_at(asked)).to_string();
            rows.push(row(Said::Words { words, dim: true }, None));
        }
        Listing::Failed(failed) => {
            rows.push(row(Said::Failed(failed.why.text()), None));
            for (i, entry) in entries.iter().enumerate() {
                let words = match entry {
                    Entry::Cached(cached) => match cached.since.and_then(date_of) {
                        Some(date) => t!("dev.adv_list_cached", tag = cached.tag, date = date).to_string(),
                        None => t!("dev.adv_list_cached_plain", tag = cached.tag).to_string(),
                    },
                    Entry::Retry => t!("dev.adv_list_retry").to_string(),
                    _ => continue,
                };
                rows.push(row(Said::Words { words, dim: false }, Some(i)));
            }
        }
    }
    List { rows, cursor, tags, dates }
}

// ── The sheet ─────────────────────────────────────────────────────────────

/// The Advanced options sheet over `face.full`, held at its tallest
/// state's top. Its clicks ask for the cells' acts — a source
/// ([`Act::SheetSource`]), a build ([`Act::SheetMode`]), the erase box
/// ([`Act::SheetErase`]), the release control ([`Act::ListOpen`]), a row of
/// its list ([`Act::ListPick`]) or anywhere else while it hangs
/// ([`Act::ListClose`]), a chooser ([`Act::SheetChooser`]), Use defaults
/// and Apply, the close ([`Act::SheetClose`]) — and the path field, while
/// it has the keyboard, draws the input method's composition and gives back
/// its caret.
pub(super) fn show(ui: &mut Ui, page: &mut Page, face: &Face) -> Drawn {
    let mut drawn = Drawn::default();
    let Some(sheet) = page.sheet.clone() else { return drawn };
    // The cells draw nothing for a board that is gone: the sheet waits on
    // the model to close.
    let Some(board) = page.boards.iter().find(|b| b.port() == sheet.port).cloned() else { return drawn };
    let view = View::of(page, &sheet, &board);
    let id = Id::new("mp3-sheet");
    let dialog = Dialog::new(id, face.full, SHEET_W, &view.title);
    let room = dialog.body_room(ui);
    let foot_h = foot(ui, room.x).1;
    let measures = Measures::for_room(&Shape { help_lines: 0, ..Shape::tallest(foot_h) }, room.y);
    let hold = Shape::tallest(foot_h).height(&measures);
    let mut acts = Vec::new();
    let shown = dialog.hold(hold).show(ui, face.pal, |ui| sheet_body(ui, id, &view, &sheet, &measures, face, &mut acts));
    // Under the hanging list the close, as every click off its rows, only
    // closes the list (the body's catch says so).
    if shown.closed && view.list.is_none() {
        acts.push(Act::SheetClose);
    }
    // The cells' keyed tooltip on their close, which the kit's has not.
    close_tip(ui, id, face);
    drawn.acts = acts;
    drawn.caret = shown.inner;
    drawn
}

/// The foot's two buttons, Use defaults and Apply, ending at the right of
/// a foot `w` wide as the kit's foot lays them: where each stands from its
/// top left, and its height.
fn foot(ui: &Ui, w: f32) -> (Vec<Rect>, f32) {
    let (defaults, apply) = (t!("dev.adv_defaults").to_string(), t!("dev.adv_apply").to_string());
    studio::foot_rects(ui, &[], &[(&defaults, false), (&apply, true)], w)
}

/// How many lines `words` take wrapped at `w` in the UI face at `size`, two
/// at most: a refusal's, the help's.
fn lines_of(ui: &Ui, words: &str, size: f32, w: f32) -> usize {
    let format = TextFormat { font_id: body(size), ..Default::default() };
    let mut job = LayoutJob::single_section(words.to_string(), format);
    job.wrap = TextWrapping { max_width: w.max(1.0), max_rows: 2, break_anywhere: false, overflow_character: Some('…') };
    ui.painter().layout_job(job).rows.len().clamp(1, 2)
}

/// The sheet's body in the Dialog's `ui`, top to bottom in `m`: the three
/// groups, the help where this state leaves it room, the foot, and over
/// them the release list while it hangs. The caret of the path field while
/// it has the keyboard.
#[allow(clippy::too_many_arguments)]
fn sheet_body(ui: &mut Ui, id: Id, view: &View, sheet: &Sheet, m: &Measures, face: &Face, acts: &mut Vec<Act>) -> Option<Pos2> {
    let pal = face.pal;
    let w = ui.available_width();
    let budget = ui.max_rect().height();
    let read_lines = match &view.read {
        Read::Nothing => 0,
        Read::Reading(_) => 1,
        Read::Facts(..) => 2,
        Read::Refused(why) => lines_of(ui, why, 13.0, w - NAME_IN - MARK_IN),
    };
    let (laid, foot_h) = foot(ui, w);
    let mut shape = view.shape(read_lines, lines_of(ui, &view.help, 13.0, w), foot_h);
    if shape.height(m) > budget + 0.01 {
        shape.help_lines = 0;
    }
    let (area, _) = ui.allocate_exact_size(vec2(w, shape.height(m)), Sense::hover());
    let col = area;
    let live = view.list.is_none();
    let mut stops: [Option<Rect>; 6] = [None; 6];
    let mut caret = None;
    let mut anchor = None;
    let mut y = col.min.y;
    // Each row the body's view ends over rather than through.
    let row_at = |top: f32, h: f32| studio::body_row(ui, top, top + h);

    // The firmware: the pin, another release, a local build, each with its
    // lines under it.
    group_label(ui, col, y, &view.groups[0], pal);
    row_at(y, LABEL_H);
    y += LABEL_H + m.label_gap;
    let rows_top = y;
    for (k, radio) in view.firmware.iter().enumerate() {
        if k > 0 {
            y += m.row_gap;
        }
        let row = Rect::from_min_size(pos2(col.min.x, y), vec2(col.width(), m.row));
        row_at(y, m.row);
        if let Some(control) = radio_row(ui, id.with("firmware").with(k), row, radio, m, live, face, acts) {
            anchor = Some(control);
            stops[1] = Some(control);
        }
        y += m.row;
        let sub = pos2(col.min.x + NAME_IN, y);
        let sub_w = col.width() - NAME_IN;
        if k == 0 {
            continue;
        }
        if k == 1 {
            if let Some(line) = &view.release_line {
                studio::dim_line(ui, sub, sub_w, line, face.ports, pal);
                row_at(y, LINE_H);
                y += LINE_H;
            }
            continue;
        }
        stops[0] = Some(Rect::from_min_max(pos2(col.min.x, rows_top), pos2(col.max.x, y)));
        match &view.read {
            Read::Nothing => {}
            Read::Reading(words) => {
                studio::dim_line(ui, sub, sub_w, words, face.ports, pal);
            }
            Read::Facts(first, second, file) => {
                let also: Vec<&str> = std::iter::once(file.as_str()).chain(face.ports.iter().copied()).collect();
                studio::dim_line(ui, sub, sub_w, first, &also, pal);
                studio::dim_line(ui, sub + vec2(0.0, LINE_H), sub_w, second, face.ports, pal);
            }
            Read::Refused(why) => refusal(ui, sub, sub_w, why, pal),
        }
        for line in 0..read_lines {
            row_at(y + line as f32 * LINE_H, LINE_H);
        }
        y += read_lines as f32 * LINE_H;
        if read_lines > 0 && view.under != Under::Nothing {
            y += m.sub_gap;
        }
        match view.under {
            Under::Nothing => {}
            Under::Choosers => {
                let row = Rect::from_min_size(pos2(col.min.x + NAME_IN, y), vec2(sub_w, m.chooser));
                choosers(ui, id, row, view, face, live, acts);
                row_at(y, m.chooser);
                stops[1] = Some(row);
                y += m.chooser;
            }
            Under::Field => {
                let rect = Rect::from_min_size(pos2(col.min.x + NAME_IN, y), vec2(sub_w, m.field));
                if let Some(field) = &sheet.field {
                    let input = studio::Input::of(field).mono().lit().focused(true, face.composition);
                    caret = studio::field_box(ui, id.with("path"), rect, &input, pal).caret;
                }
                row_at(y, m.field);
                stops[5] = Some(rect);
                y += m.field;
            }
        }
    }

    // The flash mode: the two builds and their note.
    y += m.gap;
    group_label(ui, col, y, &view.groups[1], pal);
    row_at(y, LABEL_H);
    y += LABEL_H + m.label_gap;
    let modes_top = y;
    for (k, radio) in view.modes.iter().enumerate() {
        if k > 0 {
            y += m.row_gap;
        }
        let row = Rect::from_min_size(pos2(col.min.x, y), vec2(col.width(), m.row));
        row_at(y, m.row);
        radio_row(ui, id.with("mode").with(k), row, radio, m, live, face, acts);
        y += m.row;
    }
    stops[2] = Some(Rect::from_min_max(pos2(col.min.x, modes_top), pos2(col.max.x, y)));
    if let Some(note) = &view.mode_note {
        y += m.note_gap;
        studio::dim_line(ui, pos2(col.min.x + NAME_IN, y), col.width() - NAME_IN, note, face.ports, pal);
        row_at(y, LINE_H);
        y += LINE_H;
    }

    // The erase box.
    y += m.gap;
    group_label(ui, col, y, &view.groups[2], pal);
    row_at(y, LABEL_H);
    y += LABEL_H + m.label_gap;
    let row = Rect::from_min_size(pos2(col.min.x, y), vec2(col.width(), m.check));
    let (label, detail) = (t!("dev.adv_erase").to_string(), format!(" — {}", t!("dev.adv_erase_desc")));
    if studio::check_line_at(ui, id.with("erase"), row, view.erase, &label, &detail, view.erase_focus, pal).clicked() && live {
        acts.push(Act::SheetErase);
    }
    row_at(y, m.check);
    stops[3] = Some(row);
    y += m.check;

    // The help: a rule, and its words about the group with the keyboard.
    if shape.help_lines > 0 {
        y += m.gap;
        studio::rule(ui, col.min.x, col.max.x, y, pal);
        let at = pos2(col.min.x, y + 1.0 + m.help_gap);
        Para::new(&view.help, body(13.0), pal.dim).spaced(LINE_H).rows(2).paint(ui, at, col.width());
        row_at(y, shape.help(m));
        y += shape.help(m);
    }

    // The foot: Use defaults, and Apply where an image is whole.
    y += m.gap;
    let at = vec2(col.min.x, y);
    let (defaults, apply) = (t!("dev.adv_defaults").to_string(), t!("dev.adv_apply").to_string());
    let rects = [laid[0].translate(at), laid[1].translate(at)];
    row_at(y, foot_h);
    if studio::form_button(ui, id.with("foot").with(0usize), rects[0], &defaults, false, pal).clicked() && live {
        acts.push(Act::Defaults);
    }
    if view.can_apply {
        if studio::form_button(ui, id.with("foot").with(1usize), rects[1], &apply, true, pal).clicked() && live {
            acts.push(Act::Apply);
        }
    } else {
        let _ = studio::form_button_off(ui, id.with("foot").with(1usize), rects[1], &apply, pal);
    }
    // The keyboard's button: ringed, as the cells put it on their slab; an
    // Apply that cannot apply wears no ring, as it wears no slab.
    if let Some(on_apply) = view.buttons_focus
        && (view.can_apply || !on_apply)
    {
        studio::button_ring(ui, rects[usize::from(on_apply)], pal);
    }
    stops[4] = Some(rects[0].union(rects[1]));

    match &view.list {
        Some(list) => {
            let anchor = anchor.unwrap_or_else(|| Rect::from_min_size(pos2(col.min.x + NAME_IN, rows_top + m.row), vec2(1.0, 1.0)));
            release_list(ui, id, list, anchor, col, face, acts);
        }
        None => {
            if let Some(rect) = stops[view.stop] {
                studio::reveal(ui, id, view.stop, rect);
            }
        }
    }
    caret
}

/// A group's label at `y`: the kit's small capitals, dim.
fn group_label(ui: &Ui, col: Rect, y: f32, words: &str, pal: &Pal) {
    studio::small_caps(ui, pos2(col.min.x, y + LABEL_H / 2.0), words, pal.dim, col.width(), Align2::LEFT_CENTER);
}

/// A refusal from `at`, `w` wide: the kit's cross in gold on its first
/// line, and the words in gold beside it, in two lines at most.
fn refusal(ui: &Ui, at: Pos2, w: f32, why: &str, pal: &Pal) {
    studio::cross(ui, pos2(at.x + 6.0, at.y + LINE_H / 2.0), pal.gold);
    let words = at + vec2(MARK_IN, 0.0);
    Para::new(why, body(13.0), pal.gold).spaced(LINE_H).rows(2).paint(ui, words, (w - MARK_IN).max(1.0));
}

/// A choice of a group in `row`, the column's width, as the board draws
/// one: the kit's round mark (its chosen one in the accent in the group
/// with the keyboard, else the text colour), its name (semibold when
/// chosen, dim when the source has it not, a version in the monospace) and
/// after it its detail — its dim description, a picked path cut at its
/// front with the whole in a tooltip, or the release control. A row with
/// an act takes the click across its look, which reaches 8 points past the
/// column, raised under the pointer, as the kit's choice's; one without
/// takes nothing. Its release control's rect.
#[allow(clippy::too_many_arguments)]
fn radio_row(ui: &Ui, id: Id, row: Rect, radio: &Radio, m: &Measures, live: bool, face: &Face, acts: &mut Vec<Act>) -> Option<Rect> {
    let pal = face.pal;
    let hit = row.expand2(vec2(studio::RADIO_REACH, 0.0));
    let mut response = radio.act.as_ref().map(|_| studio::hand(ui.interact(hit, id, Sense::click())));
    if response.as_ref().is_some_and(|r| r.hovered()) {
        ui.painter().rect_filled(hit, studio::square(), pal.raised);
    }
    let mid = row.center().y;
    let lit = if radio.focused { pal.accent } else { pal.text };
    studio::radio_mark(ui, pos2(row.min.x + 7.0, mid), radio.on, lit, pal);
    let x = row.min.x + NAME_IN;
    let ink = if radio.usable { pal.text } else { pal.dim };
    let font = if radio.on { strong(13.5) } else { body(13.5) };
    let name = studio::figured(&radio.name, face.ports, font, mono(12.5), ink);
    let drawn = studio::runs(ui, pos2(x, mid), &name, (row.max.x - x).max(1.0), Align2::LEFT_CENTER);
    let from = drawn.max.x + DETAIL_GAP;
    let room = row.max.x - from;
    let mut control = None;
    match &radio.detail {
        Detail::Words(words) if room > 16.0 => {
            let runs = studio::figured(&format!("— {words}"), face.ports, body(13.0), mono(12.5), pal.dim);
            studio::runs(ui, pos2(from, mid), &runs, room, Align2::LEFT_CENTER);
        }
        Detail::Path(path) if room > 16.0 => {
            let dash = studio::text(ui, pos2(from, mid), "— ", body(13.0), pal.dim, room, Align2::LEFT_CENTER);
            let (shown, cut) = studio::cut_start(ui, path, mono(12.5), (row.max.x - dash.max.x).max(1.0));
            studio::text(ui, pos2(dash.max.x, mid), &shown, mono(12.5), pal.text, (row.max.x - dash.max.x).max(1.0), Align2::LEFT_CENTER);
            if cut {
                response = response.map(|r| r.on_hover_text(egui::RichText::new(path).font(mono(12.5))));
            }
        }
        Detail::Control(words, focused) => {
            // The kit's closed list, its words a choice's detail's size,
            // standing in the row 2 points clear of its look.
            let h = (m.row - 4.0).min(CONTROL_H);
            let x = from + 2.0;
            let w = studio::select_box_width(ui, words, face.ports, CONTROL_SIZE).min((row.max.x - x).max(1.0));
            let rect = Rect::from_min_size(pos2(x, mid - h / 2.0), vec2(w, h));
            let clicked = studio::select_button(ui, id.with("control"), rect, words, face.ports, CONTROL_SIZE, *focused, pal).clicked();
            if clicked && live {
                acts.push(Act::ListOpen);
            }
            control = Some(rect);
        }
        _ => {}
    }
    if live
        && response.is_some_and(|r| r.clicked())
        && let Some(act) = &radio.act
    {
        acts.push(act.clone());
    }
    control
}

/// The local build's choosers across `row` (the cells' `draw_choosers`):
/// the system's dialog up, said dim; else each chooser on offer as the
/// kit's text button, 24 points apart, one that would not fit left out —
/// the keyboard's one in the accent while the row has it — and, where no
/// dialog can open here, why, dim after them.
fn choosers(ui: &Ui, id: Id, row: Rect, view: &View, face: &Face, live: bool, acts: &mut Vec<Act>) {
    let pal = face.pal;
    let mid = row.center().y;
    if view.dialog {
        let words = t!("dev.adv_local_dialog").to_string();
        studio::text(ui, pos2(row.min.x, mid), &words, body(13.0), pal.dim, row.width(), Align2::LEFT_CENTER);
        return;
    }
    let mut x = row.min.x;
    for chooser in &view.choosers {
        let words = t!(match chooser {
            Chooser::File => "dev.adv_choose_file",
            Chooser::Folder => "dev.adv_choose_folder",
            Chooser::Type => "dev.adv_type_path",
        })
        .to_string();
        let w = studio::text_button_width(ui, &words, None, face.ports);
        if x + w > row.max.x + 0.5 {
            break;
        }
        let at = Chooser::ALL.iter().position(|c| c == chooser).unwrap_or(0);
        let focused = view.chooser_focus == Some(*chooser);
        // A point over, so the words' own width never reads as a cut.
        let button = studio::text_button_keyed(ui, id.with("chooser").with(at), pos2(x, mid), &words, face.ports, w + 1.0, focused, pal);
        if button.clicked() && live {
            acts.push(Act::SheetChooser(at));
        }
        x += w + APART;
    }
    if let Some(why) = &view.no_dialog
        && x + 16.0 < row.max.x
    {
        let words = t!("dev.adv_no_dialog", why = why).to_string();
        studio::text(ui, pos2(x, mid), &words, body(13.0), pal.dim, row.max.x - x, Align2::LEFT_CENTER);
    }
}

// ── The release list ─────────────────────────────────────────────────────

/// The release list hanging from `anchor`, its control, over the sheet's
/// lower rows (the cells' `draw_list`): the kit's list that hangs
/// (`studio::hung_list`), as the Settings board draws one, its entries —
/// the release a release row is for, its date and its builds in columns as
/// wide as every release's, the picked one's LED before it; the asking
/// line, the failure, the switch, what is on this computer and Try again —
/// the keyboard's cursor lit, an entry raised under the pointer. Its
/// column's words stand under the control's; it is as wide as its widest
/// entry, no wider than the column, and keeps inside it; as tall as its
/// entries, no lower than the body's foot or its view, its cursor kept in
/// sight. With
/// not one entry of room it is not drawn and still holds the keys.
///
/// A click on an entry that picks asks for [`Act::ListPick`]; one anywhere
/// else in the page's area, the sheet's controls under it, its close and
/// the scrim beside it, only [`Act::ListClose`], as the cells register
/// their close over the whole area under the list's rows.
fn release_list(ui: &Ui, id: Id, list: &List, anchor: Rect, col: Rect, face: &Face, acts: &mut Vec<Act>) {
    let pal = face.pal;
    let view = ui.clip_rect();
    // Every click in the body but on an entry lands here first: the
    // sheet's controls under it take none, show no hover and no hand.
    let catch = ui.interact(view, id.with("list-close"), Sense::click());
    if catch.clicked() {
        acts.push(Act::ListClose);
    }
    // And one off the body, on the title, the close or the scrim.
    let elsewhere = ui.input(|i| i.pointer.primary_clicked() && i.pointer.interact_pos().is_some_and(|p| face.full.contains(p) && !view.contains(p)));
    if elsewhere {
        acts.push(Act::ListClose);
    }
    let tag_w = list.tags.iter().map(|t| studio::width_of(ui, t, mono(12.5))).fold(0.0, f32::max) + COLUMN_GAP;
    let date_w = list.dates.iter().map(|d| studio::width_of(ui, d, mono(12.5))).fold(0.0, f32::max) + COLUMN_GAP;
    let words_w = |words: &str| studio::runs_width(ui, &studio::figured(words, face.ports, body(13.5), mono(12.5), pal.text));
    let natural = |row: &ListRow| -> f32 {
        let lead = if row.said.in_column() { LEAD_W } else { 0.0 };
        let said = match &row.said {
            Said::Asking(words) => studio::searching_width(ui, words),
            Said::Failed(why) => MARK_IN + studio::width_of(ui, why, body(13.5)),
            Said::Release { words, .. } => tag_w + date_w + words_w(words),
            Said::Words { words, .. } => words_w(words),
        };
        lead + said
    };
    let widest = list.rows.iter().map(natural).fold(0.0, f32::max);
    let top = anchor.max.y + 2.0;
    // Inside the sheet's body: its view's foot, which is the room the
    // Dialog gave it, stands under the body's own foot where the window
    // has shrunk to the body, and the list would hang past the frame.
    let bottom = view.max.y.min(col.max.y).min(face.full.max.y);
    let fits = studio::hung_fits(top, bottom).min(list.rows.len());
    if fits == 0 {
        return;
    }
    // As wide as its widest entry and the thumb's gutter while it holds
    // fewer than its all, no wider than the column.
    let w = studio::hung_width(widest, fits, list.rows.len()).min(col.width());
    // Its column's words under the control's, as the cells hang theirs and
    // the Settings board lines its entries up with its box's words: the
    // lead's room stands left of them.
    let left = (studio::hung_left(anchor.min.x) - LEAD_W).min(col.max.x - w).max(col.min.x);
    let cursor = list.rows.iter().position(|r| r.pick == Some(list.cursor));
    let start = studio::hung_first(cursor, fits, list.rows.len());
    let frame = studio::hung_frame(pos2(left, top), w, fits);
    let id_of = |k: usize| list.rows[k].pick.map(|pick| id.with("entry").with(pick));
    let words = |ui: &Ui, k: usize, room: Rect, on: bool| entry(ui, room, &list.rows[k], on, tag_w, date_w, face);
    if let Some(k) = studio::hung_list(ui, frame, start, fits, list.rows.len(), cursor, id_of, pal, words)
        && let Some(pick) = list.rows[k].pick
    {
        acts.push(Act::ListPick(pick));
    }
}

/// One entry's words in `room`, the selection's ink on the cursor's.
#[allow(clippy::too_many_arguments)]
fn entry(ui: &Ui, room: Rect, row: &ListRow, selected: bool, tag_w: f32, date_w: f32, face: &Face) {
    let pal = face.pal;
    let ink = |own: Color32| if selected { pal.on_accent } else { own };
    let mid = room.center().y;
    let mut x = room.min.x;
    let end = room.max.x;
    if row.said.in_column() {
        if row.picked {
            studio::led(ui, pos2(x + 3.0, studio::led_mid(ui, mid, &body(13.5), false)), ink(pal.accent));
        }
        x += LEAD_W;
    }
    let room = |x: f32| (end - x).max(1.0);
    match &row.said {
        Said::Asking(words) => {
            studio::searching_at(ui, pos2(x, mid), words, room(x), pal);
        }
        Said::Failed(why) => {
            studio::cross(ui, pos2(x + 6.0, mid), pal.gold);
            studio::text(ui, pos2(x + MARK_IN, mid), why, body(13.5), pal.gold, room(x + MARK_IN), Align2::LEFT_CENTER);
        }
        Said::Release { tag, date, words } => {
            studio::text(ui, pos2(x, mid), tag, mono(12.5), ink(pal.text), room(x), Align2::LEFT_CENTER);
            let x = x + tag_w;
            if x < end {
                studio::text(ui, pos2(x, mid), date, mono(12.5), ink(pal.dim), room(x), Align2::LEFT_CENTER);
            }
            let x = x + date_w;
            if x < end {
                let runs = studio::figured(words, face.ports, body(13.5), mono(12.5), ink(pal.text));
                studio::runs(ui, pos2(x, mid), &runs, room(x), Align2::LEFT_CENTER);
            }
        }
        Said::Words { words, dim } => {
            let own = if *dim { pal.dim } else { pal.text };
            let runs = studio::figured(words, face.ports, body(13.5), mono(12.5), ink(own));
            studio::runs(ui, pos2(x, mid), &runs, room(x), Align2::LEFT_CENTER);
        }
    }
}

#[cfg(test)]
mod tests {
    //! The sheet's egui face run headless, as the window's panel runs it,
    //! on the page's own fixtures: boards told on quiet channels, the
    //! commands the page sent read back, the keys pressed as the GUI hands
    //! them to the page. No worker runs, no port or card is opened, and no
    //! system dialog comes up: the page's dialogs are the test's stand-in.

    use egui::{Event, FullOutput, PointerButton, Vec2};
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;
    use crate::admin::EguiShown;
    use crate::device::desk::{Choice, Event as Told};
    use crate::device::page::{Ends, fixtures};
    use crate::setup::picker::Pick;

    fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    /// The page's area in points: in the GUI's default 100×30 window, at
    /// the GUI's floor (100×24), and in the boards' 1440 by 900 window.
    const WINDOW: Vec2 = vec2(800.0, 448.0);
    const FLOOR: Vec2 = vec2(800.0, 352.0);
    const BOARD: Vec2 = vec2(1440.0, 824.0);
    /// The foot's one row: the kit's form button.
    const FORM_H: f32 = studio::FORM_BUTTON_H;
    /// The kit's modal: its padding, its title's row and the room under
    /// it; a choice's look past the column; the closed list's least width.
    const MODAL_PAD: f32 = 24.0;
    const TITLE_H: f32 = 20.0;
    const MODAL_GAP: f32 = 20.0;
    const REACH: f32 = studio::RADIO_REACH;
    const CONTROL_LEAST: f32 = studio::SELECT_LEAST;

    /// A local build's folder, and a file that is not one, of the
    /// platform's own: the page shortens a path at its last separator.
    #[cfg(windows)]
    const BUILD: &str = "C:\\code\\mstream-mp3-player\\.pio\\build\\core2";
    #[cfg(not(windows))]
    const BUILD: &str = "/code/mstream-mp3-player/.pio/build/core2";
    #[cfg(windows)]
    const REFUSED: &str = "C:\\Downloads\\core2-factory.bin";
    #[cfg(not(windows))]
    const REFUSED: &str = "/Downloads/core2-factory.bin";

    /// A context styled as the host's, its passes run over a page's area.
    struct Probe {
        ctx: egui::Context,
        size: Vec2,
    }

    impl Probe {
        fn new(size: Vec2) -> Probe {
            let ctx = egui::Context::default();
            crate::gui::window::panel::fonts(&ctx);
            studio::style_once(&ctx, &Pal::now());
            Probe { ctx, size }
        }

        /// One pass with `events`, the input method composing
        /// `composition` and tooltips naming their keys where `key_hints`:
        /// what egui drew, and what the page told its host.
        fn run(&self, page: &mut Page, events: Vec<Event>, composition: &str, key_hints: bool) -> (FullOutput, EguiShown) {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), events, ..Default::default() };
            let mut told: Option<EguiShown> = None;
            let mut out = self.ctx.run_ui(input, |ui| {
                let next = super::super::show(ui, page, composition, key_hints);
                told = Some(match told.take() {
                    Some(last) => EguiShown { outcome: last.outcome.or(next.outcome), caret: next.caret },
                    None => next,
                });
            });
            // No GPU here to take the font atlas: let go of it on purpose.
            out.textures_delta.clear();
            (out, told.expect("the pass ran"))
        }

        fn pass(&self, page: &mut Page, events: Vec<Event>) -> (FullOutput, EguiShown) {
            self.run(page, events, "", false)
        }

        /// Two passes (egui sizes a modal on the first), the second's.
        fn settle(&self, page: &mut Page) -> FullOutput {
            let _ = self.pass(page, Vec::new());
            self.pass(page, Vec::new()).0
        }

        /// A press and a release at `at`, a pass each after the move.
        fn click(&self, page: &mut Page, at: Pos2) {
            for events in click_events(at) {
                let _ = self.pass(page, events);
            }
        }

        /// The pointer moved to `at` and left there: the passes after.
        fn hover(&self, page: &mut Page, at: Pos2, key_hints: bool) -> FullOutput {
            let _ = self.run(page, vec![Event::PointerMoved(at)], "", key_hints);
            let _ = self.run(page, Vec::new(), "", key_hints);
            self.run(page, Vec::new(), "", key_hints).0
        }

        /// Where what `id` drew on the last pass stands.
        fn rect_of(&self, id: Id) -> Rect {
            self.ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} was drawn")).rect
        }
    }

    fn click_events(at: Pos2) -> [Vec<Event>; 3] {
        let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        [vec![Event::PointerMoved(at)], vec![button(true)], vec![button(false)]]
    }

    /// Every text the sheet drew over its scrim, whole, with where and the
    /// rect it was clipped to: the page beneath, drawn first, left out.
    fn texts(out: &FullOutput) -> Vec<(String, Rect, Rect)> {
        let scrim = Color32::from_rgba_premultiplied(6, 6, 9, 158);
        let from = out.shapes.iter().position(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.fill == scrim)).expect("the sheet's scrim");
        out.shapes[from..]
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), Rect::from_min_size(t.pos, t.galley.size()), clipped.clip_rect)),
                _ => None,
            })
            .collect()
    }

    fn drew(out: &FullOutput, words: &str) -> bool {
        texts(out).iter().any(|(t, ..)| t == words)
    }

    /// Where the sheet drew `words`.
    fn find(out: &FullOutput, words: &str) -> Rect {
        let all = texts(out);
        let found = all.iter().find(|(t, ..)| t == words).map(|(_, r, _)| *r);
        found.unwrap_or_else(|| panic!("{words:?} drawn: {:?}", all.iter().map(|(t, ..)| t).collect::<Vec<_>>()))
    }

    fn assert_drew(out: &FullOutput, words: &[&str]) {
        for w in words {
            assert!(drew(out, w), "{w:?} drawn: {:?}", texts(out).into_iter().map(|(t, ..)| t).collect::<Vec<_>>());
        }
    }

    /// Whether the sheet drew `words` whole inside what it was clipped to:
    /// in the body's view, not scrolled out of it.
    fn in_view(out: &FullOutput, words: &str) -> bool {
        let (_, at, clip) = texts(out).into_iter().find(|(t, ..)| t == words).unwrap_or_else(|| panic!("{words:?} drawn"));
        clip.contains_rect(at.shrink(0.5))
    }

    /// The centres of the circles a pass filled in `color`, `r` points
    /// round: the chosen marks' dots.
    fn dots(out: &FullOutput, color: Color32, r: f32) -> Vec<Pos2> {
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Circle(circle) if circle.fill == color && circle.radius == r => Some(circle.center),
                _ => None,
            })
            .collect()
    }

    /// How many rects a pass stroked in `color` round `inside`.
    fn rings_round(out: &FullOutput, color: Color32, inside: Rect) -> usize {
        let ring = |c: &&egui::epaint::ClippedShape| match &c.shape {
            egui::Shape::Rect(r) => r.stroke.color == color && r.stroke.width > 0.0 && r.rect.contains_rect(inside),
            _ => false,
        };
        out.shapes.iter().filter(ring).count()
    }

    fn page_with(tell: impl FnOnce(&Ends)) -> (Page, Ends) {
        let (mut page, ends) = Page::quiet();
        tell(&ends);
        page.pump();
        (page, ends)
    }

    /// COM3 on v0.8.0's QIO build: the pin, up to date.
    fn on_pin_qio() -> Board {
        fixtures::old_on("COM3", "v0.8.0", "e127a6bf")
    }

    /// The sheet up over COM3 at its defaults.
    fn sheet_up() -> (Page, Ends) {
        let (mut page, ends) = page_with(|e| e.boards(vec![on_pin_qio()]));
        page.act(Act::Advanced);
        assert!(page.sheet.is_some());
        (page, ends)
    }

    fn press(page: &mut Page, code: KeyCode) {
        let _ = Screen::key(page, KeyEvent::new(code, KeyModifiers::NONE));
        page.pump();
    }

    fn type_in(page: &mut Page, text: &str) {
        for c in text.chars() {
            press(page, KeyCode::Char(c));
        }
    }

    /// The page's own events until `done`, the dialogs' thread answering in
    /// its own time.
    fn pump_until(page: &mut Page, what: &str, done: impl Fn(&Page) -> bool) {
        let t0 = std::time::Instant::now();
        while !done(page) {
            assert!(t0.elapsed() < std::time::Duration::from_secs(5), "never: {what}");
            std::thread::sleep(std::time::Duration::from_millis(5));
            page.pump();
        }
    }

    fn sheet(page: &Page) -> &Sheet {
        page.sheet.as_ref().expect("the sheet is up")
    }

    fn choose(image: Image, erase: bool) -> Cmd {
        Cmd::Choose { port: "COM3".into(), choice: Some(Choice { image, erase }) }
    }

    /// A local build chosen by a click, its path field opened by a click on
    /// Type a path, and `path` typed in it and sent to be read.
    fn typed(probe: &Probe, page: &mut Page, ends: &Ends, path: &str) {
        let out = probe.settle(page);
        probe.click(page, find(&out, "A local build").center());
        let out = probe.settle(page);
        probe.click(page, find(&out, "Type a path").center());
        assert!(page.typing(), "Type a path: the field has the keys");
        if let Some(field) = page.sheet.as_mut().and_then(|s| s.field.as_mut()) {
            *field = tui_input::Input::default();
        }
        type_in(page, path);
        press(page, KeyCode::Enter);
        assert_eq!(ends.sent(), [Cmd::Vet { path: path.into() }], "read on the worker at once");
    }

    // ── The sheet at its defaults, and its room ───────────────────────────

    #[test]
    fn at_its_defaults_the_sheet_says_what_the_cells_say_in_the_boards_measures() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, ends) = sheet_up();
        let out = probe.settle(&mut page);
        assert_drew(&out, &["Advanced options · COM3", "FIRMWARE", "FLASH MODE", "ERASE"]);
        assert_drew(&out, &["Erase the whole flash first", " — loses the settings", "USE DEFAULTS", "APPLY"]);
        // Every radio row the cells draw, in their words: its name, and its
        // description after the dash.
        let rows = crate::device::page::cells::sheet::radio_words(&page);
        assert_eq!(rows.len(), 5, "{rows:?}");
        for words in &rows {
            let (name, desc) = words.split_once(" — ").unwrap_or_else(|| panic!("{words}"));
            assert_drew(&out, &[name, &format!("— {desc}")]);
        }
        let help = "v0.8.0 is the release this player was made with, checked by its built-in checksum. “Up to date” here always means v0.8.0.";
        assert_drew(&out, &[help]);
        // The board's measures: rows 30 tall and 2 apart, the window held
        // where its tallest state stands centred.
        let row = |k: usize| probe.rect_of(Id::new("mp3-sheet").with("firmware").with(k));
        let (pin, release, local) = (row(0), row(1), row(2));
        assert_eq!((pin.height(), release.min.y - pin.min.y, local.min.y - release.min.y), (30.0, 32.0, 32.0), "{pin:?} {release:?} {local:?}");
        assert_eq!(pin.width(), 640.0 - 2.0 * MODAL_PAD + 2.0 * REACH, "a row's look reaches 8 points past the column");
        let close = probe.rect_of(Id::new("mp3-sheet").with("close"));
        let tallest = Shape::tallest(FORM_H).height(&ROOMY) + 2.0 * MODAL_PAD + TITLE_H + MODAL_GAP;
        let top = BOARD.y / 2.0 - tallest / 2.0;
        assert!((close.center().y - (top + MODAL_PAD + TITLE_H / 2.0)).abs() < 1.0, "the close at {close:?}, the top at {top}");
        assert!(ends.sent().is_empty(), "drawing asks the worker nothing");
    }

    #[test]
    fn the_window_stands_still_whatever_rows_come_and_go_under_a_choice() {
        let _en = english();
        for size in [BOARD, WINDOW, FLOOR] {
            let probe = Probe::new(size);
            let (mut page, ends) = sheet_up();
            let _ = probe.settle(&mut page);
            let close = Id::new("mp3-sheet").with("close");
            let local = Id::new("mp3-sheet").with("firmware").with(2usize);
            let (at_rest, row) = (probe.rect_of(close), probe.rect_of(local));
            typed(&probe, &mut page, &ends, BUILD);
            ends.tell([Told::Vetted { path: BUILD.into(), result: Ok(fixtures::local_facts(BUILD, "v0.8.0-5-g4e94418", Mode::Qio)) }]);
            page.pump();
            let _ = probe.settle(&mut page);
            assert_eq!(probe.rect_of(close), at_rest, "{size:?}: the top held");
            assert_eq!(probe.rect_of(local), row, "{size:?}: the row the pointer was on stays");
        }
    }

    #[test]
    fn in_a_short_window_the_rows_close_up_and_the_help_gives_way_before_the_foot_does() {
        let _en = english();
        // The GUI's default window: the rows closer than the boards', the
        // whole sheet at rest in view.
        let probe = Probe::new(WINDOW);
        let (mut page, _ends) = sheet_up();
        let out = probe.settle(&mut page);
        let row = |k: usize| probe.rect_of(Id::new("mp3-sheet").with("firmware").with(k));
        assert!(row(1).min.y - row(0).min.y < 32.0, "closed up: {:?} {:?}", row(0), row(1));
        for (words, ..) in texts(&out) {
            assert!(in_view(&out, &words), "{words:?} whole in view");
        }
        // The GUI's floor: no room for the help at rest, the foot whole.
        let probe = Probe::new(FLOOR);
        let out = probe.settle(&mut page);
        assert!(!texts(&out).iter().any(|(t, ..)| t.starts_with("v0.8.0 is the release")), "the help gave way");
        assert!(in_view(&out, "APPLY") && in_view(&out, "USE DEFAULTS"), "the foot whole in view");
    }

    #[test]
    fn at_the_floor_the_tallest_state_scrolls_and_the_keyboards_stop_is_brought_into_view() {
        let _en = english();
        let probe = Probe::new(FLOOR);
        let (mut page, ends) = sheet_up();
        // Refused in two lines, the field open again under them, the
        // mode's note: the tallest the sheet stands.
        typed(&probe, &mut page, &ends, REFUSED);
        let why = format!("{REFUSED} is not an mstream-mp3-player image — it says arduino-lib-builder and nothing else");
        ends.tell([Told::Vetted { path: REFUSED.into(), result: Err(DeviceError::Firmware(why)) }]);
        page.pump();
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "Type a path").center());
        assert!(page.typing());
        let out = probe.settle(&mut page);
        assert!(!in_view(&out, "APPLY"), "the foot under the fold");
        // The keys leave the field and walk to the buttons: the foot comes
        // into view on the pass the keyboard reaches it.
        press(&mut page, KeyCode::Esc);
        while sheet(&page).focus != Stop::Buttons {
            press(&mut page, KeyCode::Tab);
        }
        let out = probe.settle(&mut page);
        assert!(in_view(&out, "APPLY"), "the foot brought into view");
    }

    // ── The keyboard's walk ───────────────────────────────────────────────

    #[test]
    fn the_keyboards_stop_is_drawn_where_the_model_holds_it() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends) = sheet_up();
        let pal = Pal::now();
        let out = probe.settle(&mut page);
        // On the Firmware group: its chosen mark in the accent, Flash
        // mode's in the text colour.
        let (accent, text) = (dots(&out, pal.accent, 3.0), dots(&out, pal.text, 3.0));
        assert!(accent.len() == 1 && (accent[0].y - find(&out, "v0.8.0").center().y).abs() < 2.0, "{accent:?}");
        assert!(text.len() == 1 && (text[0].y - find(&out, "QIO").center().y).abs() < 2.0, "{text:?}");
        // Tab: Flash mode has it, and the help speaks of it.
        press(&mut page, KeyCode::Tab);
        assert_eq!(sheet(&page).focus, Stop::Mode);
        let out = probe.settle(&mut page);
        let accent = dots(&out, pal.accent, 3.0);
        assert!(accent.len() == 1 && (accent[0].y - find(&out, "QIO").center().y).abs() < 2.0, "{accent:?}");
        assert_drew(&out, &["QIO reads the flash on 4 lines: lists, the dance and MP3 decoding are faster. Most Core2s run it."]);
        // Erase: the keyboard's look round its line.
        press(&mut page, KeyCode::Tab);
        let out = probe.settle(&mut page);
        assert_eq!(rings_round(&out, pal.accent, find(&out, "Erase the whole flash first")), 1, "the erase line ringed");
        // The buttons: Apply, then Use defaults, ringed in turn.
        press(&mut page, KeyCode::Tab);
        let out = probe.settle(&mut page);
        assert_eq!(rings_round(&out, pal.accent, find(&out, "APPLY")), 1, "Apply has the keyboard");
        assert_eq!(rings_round(&out, pal.accent, find(&out, "USE DEFAULTS")), 0);
        press(&mut page, KeyCode::Left);
        let out = probe.settle(&mut page);
        assert_eq!(rings_round(&out, pal.accent, find(&out, "USE DEFAULTS")), 1, "Use defaults has it");
        assert_eq!(rings_round(&out, pal.accent, find(&out, "APPLY")), 0);
    }

    // ── Clicks ────────────────────────────────────────────────────────────

    #[test]
    fn a_build_clicked_is_the_cells_sheet_mode_and_apply_chooses_it() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, ends) = sheet_up();
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "DIO").center());
        assert_eq!((sheet(&page).mode, sheet(&page).focus), (Mode::Dio, Stop::Mode), "the cells' SheetMode(Dio)");
        let out = probe.settle(&mut page);
        let help = "DIO reads it on 2 lines, as M5Stack ships the Core2: a little slower, runs on every unit. For a Core2 that keeps restarting.";
        assert_drew(&out, &[help]);
        assert!(ends.sent().is_empty(), "nothing in the sheet writes, or asks");
        probe.click(&mut page, find(&out, "APPLY").center());
        assert!(page.sheet.is_none(), "Apply closes it");
        assert_eq!(ends.sent(), [choose(Image::Pin(Mode::Dio), false)]);
    }

    #[test]
    fn the_erase_box_flips_once_a_click_however_many_runs_egui_makes_of_its_pass() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends) = sheet_up();
        let out = probe.settle(&mut page);
        let [moved, pressed, released] = click_events(find(&out, "Erase the whole flash first").center());
        let _ = probe.pass(&mut page, moved);
        let _ = probe.pass(&mut page, pressed);
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, BOARD)), events: released, ..Default::default() };
        let mut runs = 0;
        let mut out = probe.ctx.run_ui(input, |ui| {
            runs += 1;
            let _ = super::super::show(ui, &mut page, "", false);
            if runs == 1 {
                ui.ctx().request_discard("the test's second run");
            }
        });
        out.textures_delta.clear();
        assert_eq!(runs, 2, "egui ran the release's pass twice");
        assert!(sheet(&page).erase && sheet(&page).focus == Stop::Erase, "the cells' SheetErase, once");
    }

    #[test]
    fn use_defaults_and_the_close_are_the_cells_and_the_page_beneath_takes_no_click() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let board = fixtures::chosen(on_pin_qio(), Image::release("v0.7.0", Mode::Qio), By::Sheet, None);
        let (mut page, ends) = page_with(|e| e.boards(vec![board]));
        let card = probe.settle(&mut page);
        let details = card.shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Text(t) if t.galley.text() == "Details" => Some(Rect::from_min_size(t.pos, t.galley.size()).center()),
            _ => None,
        });
        let details = details.expect("the card's Details");
        page.act(Act::Advanced);
        let out = probe.settle(&mut page);
        assert_drew(&out, &["v0.7.0"]);
        // Beneath the scrim the card is inert.
        probe.click(&mut page, details);
        assert!(page.sheet.is_some() && !page.details && ends.sent().is_empty(), "the page under the sheet is inert");
        // Use defaults: the cells' Defaults, the choice let go.
        let _ = probe.settle(&mut page);
        probe.click(&mut page, probe.rect_of(Id::new("mp3-sheet").with("foot").with(0usize)).center());
        assert!(page.sheet.is_none());
        assert_eq!(ends.sent(), [Cmd::Choose { port: "COM3".into(), choice: None }]);
        // The close: the cells' SheetClose, nothing changed.
        page.act(Act::Advanced);
        let _ = probe.settle(&mut page);
        probe.click(&mut page, probe.rect_of(Id::new("mp3-sheet").with("close")).center());
        assert!(page.sheet.is_none() && ends.sent().is_empty(), "its close changes nothing");
    }

    #[test]
    fn the_close_says_close_and_its_key_as_the_cells_tooltip_does() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends) = sheet_up();
        let _ = probe.settle(&mut page);
        let at = probe.rect_of(Id::new("mp3-sheet").with("close")).center();
        let said = |out: &FullOutput, words: &str| out.shapes.iter().any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == words));
        let out = probe.hover(&mut page, at, true);
        assert!(said(&out, "Close — Esc"), "with the keys named");
        let out = probe.hover(&mut page, at, false);
        assert!(said(&out, "Close") && !said(&out, "Close — Esc"), "and cut at its dash without");
    }

    // ── Another release ───────────────────────────────────────────────────

    #[test]
    fn another_release_hangs_its_list_and_a_pick_or_a_click_off_its_rows_closes_it() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, ends) = sheet_up();
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "Another release").center());
        assert!(sheet(&page).source == Source::Release && sheet(&page).list.is_some(), "the cells' SheetSource(1): the list hangs");
        assert_eq!(ends.sent(), [Cmd::Releases], "and GitHub is asked, once");
        let out = probe.settle(&mut page);
        assert_drew(&out, &["pick one", "asking GitHub for the releases…"]);
        let help = "Asks GitHub for the list when chosen: one request, kept for this visit. Each release is checked by its own SHA256SUMS.";
        assert_drew(&out, &[help]);
        // The list comes: the pin left out, the pre-releases behind their
        // switch, the time it was asked.
        ends.tell([fixtures::listed()]);
        page.pump();
        let out = probe.settle(&mut page);
        let asked = format!("asked GitHub at {}", clock_at(1_791_621_660));
        assert_drew(&out, &["v0.7.0", "2026-10-07", "QIO and DIO", "v0.6.0", "v0.5.0", "DIO only", "Show 1 pre-release", &asked]);
        assert!(!drew(&out, "2026-10-10") && !drew(&out, "v0.5.0-beta.1"), "{:?}", texts(&out));
        // The keyboard's cursor on the first, lit.
        let pal = Pal::now();
        let lit = |out: &FullOutput, words: &str| {
            let at = find(out, words);
            out.shapes.iter().any(|c| matches!(&c.shape, egui::Shape::Rect(r) if r.fill == pal.accent && r.rect.contains(at.center())))
        };
        assert!(lit(&out, "v0.7.0") && !lit(&out, "v0.6.0"));
        // Its tags stand under the control's words, and the control at the
        // Settings board's least width.
        let control = Id::new("mp3-sheet").with("firmware").with(1usize).with("control");
        let (tag, words) = (find(&out, "v0.7.0"), find(&out, "pick one"));
        assert!((tag.min.x - words.min.x).abs() < 1.0, "the tags under the control's words: {tag:?} {words:?}");
        assert_eq!(probe.rect_of(control).width(), CONTROL_LEAST);
        // The switch keeps it open, the pre-release shown.
        probe.click(&mut page, find(&out, "Show 1 pre-release").center());
        assert!(page.show_pre && sheet(&page).list.is_some(), "the cells' ListPick on the switch");
        let out = probe.settle(&mut page);
        assert_drew(&out, &["v0.5.0-beta.1", "pre-release, DIO only", "Hide pre-releases"]);
        // A click off its rows, on the scrim beside the window or on the
        // sheet's close, only closes the list.
        probe.click(&mut page, pos2(40.0, 400.0));
        assert!(page.sheet.is_some() && sheet(&page).list.is_none(), "the scrim: the cells' ListClose");
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "pick one").center());
        assert!(sheet(&page).list.is_some(), "the control: the cells' ListOpen");
        let _ = probe.settle(&mut page);
        probe.click(&mut page, probe.rect_of(Id::new("mp3-sheet").with("close")).center());
        assert!(page.sheet.is_some() && sheet(&page).list.is_none(), "the close, under the list: only the list");
        // A pick: v0.7.0, its facts under the control, its help.
        page.act(Act::ListOpen);
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "v0.7.0").center());
        assert_eq!(sheet(&page).picked.as_ref().map(|p| p.tag.as_str()), Some("v0.7.0"), "the cells' ListPick(0)");
        assert!(sheet(&page).list.is_none());
        let out = probe.settle(&mut page);
        assert_drew(&out, &["v0.7.0", "2026-10-07 · QIO and DIO · a step back from COM3's v0.8.0"]);
        assert_drew(&out, &["v0.7.0 downloads on Apply and is checked against its own SHA256SUMS. “Up to date” still means v0.8.0."]);
        assert_eq!(probe.rect_of(control).width(), CONTROL_LEAST, "the control kept its width from pick one to the tag");
        assert!(ends.sent().is_empty(), "the list in hand: GitHub is not asked again");
        probe.click(&mut page, find(&out, "APPLY").center());
        assert_eq!(ends.sent(), [choose(Image::release("v0.7.0", Mode::Qio), false)]);
    }

    #[test]
    fn under_the_hanging_list_a_click_on_the_sheets_own_controls_only_closes_it() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, ends) = sheet_up();
        page.act(Act::SheetSource(1));
        ends.tell([fixtures::listed()]);
        page.pump();
        let _ = ends.sent();
        let asked = format!("asked GitHub at {}", clock_at(1_791_621_660));
        // The pin's row over the list, the control it hangs from, its own
        // row that picks nothing, and the erase line and Use defaults clear
        // of it under the body's view: the cells' close over the whole area
        // takes each click, never the control's own act (the pin chosen, the
        // list kept, the box ticked, the sheet let go).
        let below = [("v0.8.0", false), ("pick one", false), (asked.as_str(), false), ("Erase the whole flash first", true), ("USE DEFAULTS", true)];
        for (words, below) in below {
            page.act(Act::ListOpen);
            let out = probe.settle(&mut page);
            let at = find(&out, words);
            if below {
                assert!(at.min.y > find(&out, &asked).max.y, "{words:?} clear of the list's rows: {at:?}");
            }
            probe.click(&mut page, at.center());
            assert!(page.sheet.is_some(), "{words:?}: the sheet stays up");
            let sheet = sheet(&page);
            assert!(sheet.list.is_none(), "{words:?}: the cells' ListClose");
            assert_eq!((sheet.source, sheet.focus, sheet.erase), (Source::Release, Stop::Sub, false), "{words:?}: nothing else");
            assert!(ends.sent().is_empty(), "{words:?}: nothing asked of the worker");
        }
    }

    #[test]
    fn at_the_guis_default_size_the_list_hangs_inside_the_sheets_frame() {
        let _en = english();
        let probe = Probe::new(WINDOW);
        let (mut page, ends) = sheet_up();
        page.act(Act::SheetSource(1));
        ends.tell([fixtures::listed()]);
        page.pump();
        let _ = ends.sent();
        let out = probe.settle(&mut page);
        // The window's frame: the strong edge round the sheet, as tall as
        // its body.
        let pal = Pal::now();
        let frame = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.stroke.color == pal.edge_strong && r.rect.width() > 400.0 => Some(r.rect),
                _ => None,
            })
            .max_by(|a, b| a.area().total_cmp(&b.area()))
            .expect("the sheet's window");
        // The list's frame: the tallest hairline in the accent.
        let list = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.stroke.color == pal.accent && r.fill == Color32::TRANSPARENT => Some(r.rect),
                _ => None,
            })
            .max_by(|a, b| a.height().total_cmp(&b.height()))
            .expect("the list's frame");
        assert!(list.height() > 2.0 * studio::HUNG_ROW_H, "{list:?}");
        assert!(list.max.y <= frame.max.y - MODAL_PAD + 0.5, "the list inside the sheet's body: {list:?} in {frame:?}");
    }

    #[test]
    fn a_list_holding_fewer_than_its_all_stands_its_thumb_in_a_gutter_clear_of_its_cursor() {
        let _en = english();
        let pal = Pal::now();
        let thumb_ink = pal.dim.gamma_multiply(studio::THUMB_REST);
        let mut thumbs = 0;
        for h in [352.0, 320.0, 300.0] {
            let probe = Probe::new(vec2(WINDOW.x, h));
            let (mut page, ends) = sheet_up();
            page.act(Act::SheetSource(1));
            ends.tell([fixtures::listed()]);
            page.pump();
            let _ = ends.sent();
            let out = probe.settle(&mut page);
            let fills = |c: Color32| out.shapes.iter().filter_map(|s| match &s.shape { egui::Shape::Rect(r) if r.fill == c => Some(r.rect), _ => None }).collect::<Vec<_>>();
            let Some(thumb) = fills(thumb_ink).into_iter().find(|r| (r.width() - studio::THUMB_W).abs() < 0.01) else { continue };
            thumbs += 1;
            // The cursor's entry lit in the accent, beside the thumb's row.
            let cursor = fills(pal.accent).into_iter().find(|r| (r.height() - studio::HUNG_ROW_H).abs() < 0.01).expect("the cursor lit");
            assert!(cursor.max.x + 1.0 <= thumb.min.x, "{h}: the cursor stops short of the thumb: {cursor:?} {thumb:?}");
        }
        assert!(thumbs > 0, "somewhere the list held fewer than its all");
    }

    #[test]
    fn the_path_fields_words_are_the_accents_as_its_caret_and_its_edge_are() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, _ends) = sheet_up();
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "A local build").center());
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "Type a path").center());
        if let Some(field) = page.sheet.as_mut().and_then(|s| s.field.as_mut()) {
            *field = tui_input::Input::default();
        }
        type_in(&mut page, "qK1");
        let out = probe.settle(&mut page);
        let accent = Pal::now().accent;
        let colors: Vec<Color32> = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == "qK1" => Some(t.galley.job.sections.iter().map(|s| s.format.color).collect()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(!colors.is_empty() && colors.iter().all(|c| *c == accent), "the typed path in the accent: {colors:?}");
    }

    #[test]
    fn with_nothing_picked_apply_waits_and_takes_no_click() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, ends) = sheet_up();
        page.act(Act::SheetSource(1));
        page.act(Act::ListClose);
        let _ = ends.sent();
        let out = probe.settle(&mut page);
        assert_drew(&out, &["pick one", "APPLY"]);
        probe.click(&mut page, find(&out, "APPLY").center());
        assert!(page.sheet.is_some() && ends.sent().is_empty(), "nothing to apply: no click");
    }

    #[test]
    fn offline_the_list_says_why_in_gold_and_try_again_asks_again() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, ends) = sheet_up();
        page.act(Act::SheetSource(1));
        ends.tell([fixtures::offline()]);
        page.pump();
        let _ = ends.sent();
        let out = probe.settle(&mut page);
        assert_drew(&out, &["GitHub did not answer: offline?", "v0.7.0 · on this computer since 2026-10-07", "Try again"]);
        let gold = Pal::now().gold;
        let cross = out.shapes.iter().filter(|c| matches!(&c.shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == gold && stroke.width == 1.6));
        assert_eq!(cross.count(), 2, "the failure behind the kit's cross, in gold");
        probe.click(&mut page, find(&out, "Try again").center());
        assert_eq!(ends.sent(), [Cmd::Releases], "a failure is never kept: asked again");
        assert!(sheet(&page).list.is_some() && page.listing == Listing::Asking);
        let out = probe.settle(&mut page);
        assert_drew(&out, &["asking GitHub for the releases…"]);
        // What is on this computer, picked: its one build offered.
        ends.tell([fixtures::offline()]);
        page.pump();
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "v0.7.0 · on this computer since 2026-10-07").center());
        let out = probe.settle(&mut page);
        assert_drew(&out, &["— not in v0.7.0, which has one image", "QIO only · a step back from COM3's v0.8.0"]);
    }

    // ── A local build ─────────────────────────────────────────────────────

    #[test]
    fn a_local_build_is_typed_in_its_field_read_and_vetted_and_its_caret_goes_to_the_host() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, ends) = sheet_up();
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "A local build").center());
        assert_eq!(sheet(&page).source, Source::Local, "the cells' SheetSource(2)");
        let (out, shown) = probe.pass(&mut page, Vec::new());
        assert_drew(&out, &["Choose a file…", "Choose a folder…", "Type a path", "the image's own: read from it once you choose one"]);
        let help = "Choose… opens your system's own dialog. A build folder means its firmware.factory.bin (PlatformIO: .pio/build/core2/).";
        assert_drew(&out, &[help]);
        assert!(shown.caret.is_none(), "no field has the keyboard: no caret");
        // Type a path: the field takes the keys, its caret the host's.
        probe.click(&mut page, find(&out, "Type a path").center());
        assert!(page.typing() && Screen::claim(&page) == Claim::All, "the cells' SheetChooser(2)");
        if let Some(field) = page.sheet.as_mut().and_then(|s| s.field.as_mut()) {
            *field = tui_input::Input::default();
        }
        type_in(&mut page, "qK1");
        let _ = probe.settle(&mut page);
        let (out, shown) = probe.run(&mut page, Vec::new(), "かな", false);
        let field = probe.rect_of(Id::new("mp3-sheet").with("path"));
        let caret = shown.caret.expect("the field with the keyboard: its caret");
        assert!(field.contains(caret - vec2(0.0, 0.5)), "the caret's foot in the field: {caret:?} {field:?}");
        assert!(drew(&out, "qK1かな"), "the composition at the caret: {:?}", texts(&out));
        let _ = probe.settle(&mut page);
        for _ in 0..3 {
            press(&mut page, KeyCode::Backspace);
        }
        type_in(&mut page, BUILD);
        press(&mut page, KeyCode::Enter);
        assert_eq!(ends.sent(), [Cmd::Vet { path: BUILD.into() }]);
        let (out, shown) = probe.pass(&mut page, Vec::new());
        assert!(shown.caret.is_none(), "the field closed: no caret");
        assert_drew(&out, &["reading it…", BUILD]);
        // Vetted: its facts, its own build, the other's why.
        ends.tell([Told::Vetted { path: BUILD.into(), result: Ok(fixtures::local_facts(BUILD, "v0.8.0-5-g4e94418", Mode::Qio)) }]);
        page.pump();
        let out = probe.settle(&mut page);
        assert_drew(&out, &["firmware.factory.bin · v0.8.0-5-g4e94418 · 2,701,840 B", "QIO, from its header · a local build, not a release"]);
        assert_drew(&out, &["— the image's own (80 MHz in its header)", "— not this image's: build the DIO variant for it"]);
        assert_drew(&out, &["Checked only for being mStream firmware, not for working. Read again at the gate, in case the folder was rebuilt."]);
        for words in crate::device::page::cells::sheet::radio_words(&page) {
            let (name, desc) = words.split_once(" — ").unwrap_or_else(|| panic!("{words}"));
            assert_drew(&out, &[name, &format!("— {desc}")]);
        }
        // Its build is its own: the mode rows take no click.
        let before = (sheet(&page).focus, sheet(&page).mode);
        probe.click(&mut page, find(&out, "DIO").center());
        assert_eq!((sheet(&page).focus, sheet(&page).mode), before, "no act for a local build's mode");
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "APPLY").center());
        assert_eq!(ends.sent(), [choose(Image::Local(BUILD.into()), false)]);
    }

    #[test]
    fn a_refused_build_says_why_in_gold_and_apply_waits() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, ends) = sheet_up();
        typed(&probe, &mut page, &ends, REFUSED);
        let why = format!("{REFUSED} is not an mstream-mp3-player image — it says arduino-lib-builder");
        ends.tell([Told::Vetted { path: REFUSED.into(), result: Err(DeviceError::Firmware(why)) }]);
        page.pump();
        let out = probe.settle(&mut page);
        let sep = std::path::MAIN_SEPARATOR;
        assert_drew(&out, &[&format!("…{sep}core2-factory.bin is not an mstream-mp3-player image — it says arduino-lib-builder")]);
        assert_drew(&out, &["Only builds of mstream-mp3-player go on from here: their app description has to say so.", REFUSED]);
        assert_drew(&out, &["the image's own: read from it once you choose one"]);
        probe.click(&mut page, find(&out, "APPLY").center());
        assert!(page.sheet.is_some() && ends.sent().is_empty(), "Apply waits for a vetted pick");
    }

    #[test]
    fn a_long_path_is_cut_at_its_front_and_said_whole_in_a_tooltip() {
        let _en = english();
        let probe = Probe::new(WINDOW);
        let sep = std::path::MAIN_SEPARATOR;
        let parts = ["builds", "a folder with a long name for the player's builds", "another one", "mstream-mp3-player", ".pio", "build", "core2"];
        let path = parts.iter().fold(String::new(), |all, part| format!("{all}{sep}{part}"));
        let (mut page, ends) = sheet_up();
        typed(&probe, &mut page, &ends, &path);
        ends.tell([Told::Vetted { path: path.clone().into(), result: Ok(fixtures::local_facts(&path, "v0.8.0-5-g4e94418", Mode::Qio)) }]);
        page.pump();
        let out = probe.settle(&mut page);
        let shown = texts(&out).into_iter().map(|(t, ..)| t).find(|t| t.starts_with('…') && t.ends_with("core2"));
        let shown = shown.unwrap_or_else(|| panic!("the path cut at its front: {:?}", texts(&out)));
        assert!(path.ends_with(shown.trim_start_matches('…')) && shown.len() < path.len(), "{shown}");
        assert!(!drew(&out, &path), "not whole on the row");
        // egui shows no tooltip just after a click: a hand resting on the
        // row, in a context with no clicks behind it.
        let probe = Probe::new(WINDOW);
        let _ = probe.settle(&mut page);
        let row = probe.rect_of(Id::new("mp3-sheet").with("firmware").with(2usize));
        let out = probe.hover(&mut page, row.center(), false);
        let whole = out.shapes.iter().any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text() == path));
        assert!(whole, "whole in the row's tooltip");
    }

    #[test]
    fn the_choosers_ask_the_models_dialogs_and_where_none_can_open_the_field_takes_the_keys() {
        let _en = english();
        let probe = Probe::new(BOARD);
        let (mut page, ends) = sheet_up();
        page.dialogs.stub = Some(std::sync::Arc::new(|chooser| match chooser {
            Chooser::Folder => Pick::Folder(BUILD.into()),
            _ => Pick::Cancelled,
        }));
        page.act(Act::SheetSource(2));
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "Choose a folder…").center());
        assert!(sheet(&page).dialog, "the cells' SheetChooser(1): the model's dialog, on its thread");
        let out = probe.settle(&mut page);
        assert_drew(&out, &["the dialog is open…"]);
        assert!(!drew(&out, "Choose a file…"));
        pump_until(&mut page, "the folder's dialog answered", |p| p.sheet.as_ref().is_some_and(|s| !s.dialog));
        assert_eq!(ends.sent(), [Cmd::Vet { path: BUILD.into() }]);
        // No dialog here: the field takes the keys, and after Esc the row
        // says why.
        page.dialogs.stub = Some(std::sync::Arc::new(|_| Pick::Unavailable("no session bus".into())));
        let out = probe.settle(&mut page);
        probe.click(&mut page, find(&out, "Choose a file…").center());
        pump_until(&mut page, "the field opened", |p| p.typing());
        press(&mut page, KeyCode::Esc);
        let out = probe.settle(&mut page);
        assert_drew(&out, &["Type a path", "no dialog here: no session bus"]);
        assert!(!drew(&out, "Choose a file…") && !drew(&out, "Choose a folder…"));
        // The keyboard's chooser, the one the field came from, in the
        // accent while the row has the keyboard. The pointer still rests
        // where the click left it, on Type a path now, and under it the
        // pointer's look wins, as the cells' hover does; gone, the
        // keyboard's shows.
        assert_eq!((sheet(&page).focus, sheet(&page).chooser), (Stop::Sub, Chooser::Type));
        let pal = Pal::now();
        let ink = |out: &FullOutput| {
            out.shapes.iter().find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == "Type a path" => t.galley.job.sections.first().map(|s| s.format.color),
                _ => None,
            })
        };
        assert_eq!(ink(&out), Some(pal.text), "under the pointer: its look");
        let _ = probe.pass(&mut page, vec![Event::PointerGone]);
        let out = probe.settle(&mut page);
        assert_eq!(ink(&out), Some(pal.accent), "the keyboard's chooser lit");
    }
}
