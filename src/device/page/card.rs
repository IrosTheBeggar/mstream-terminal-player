//! The Card view: an SD card in this computer's reader, on the MP3 Player
//! tab beside the boards (docs/ux-contracts/mp3-player-screen.md, clauses
//! 3a, 20a, 25, 25a and 44–63; the twelfth design set, mStream
//! docs/designs/card-transfer, cards 02–07, with the user's picks of
//! 2026-10-19).
//!
//! A card is a subject like a board: a tab on the same row, one dim
//! rounded card in the board card's grammar — the head, Holds, Space, Last
//! sync, Next sync, Converts, one primary — and Details. Its words follow
//! what the transfer worker reads off it and its server answered
//! (card::worker): the view keeps only which card is in view, Details,
//! its Settings sheet, its gate and the notes. Every change that removes
//! music or rewrites the card passes a gold gate first (clause 58); the
//! sync itself is the worker's, which the GUI owns (clause 55), so leaving
//! the tab never stops it.

use std::cell::RefCell;
use std::rc::Rc;

use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::*;
use crate::device::card::cards::Cards as Known;
use crate::device::card::engine::{Answers, Removal};
use crate::device::card::plan::{Fit, TRACK_CAP};
use crate::device::card::state::Convert;
use crate::device::card::volume::{Fs, Refusal, Table};
use crate::device::card::worker::{
    self, Allow, CardView, Ended, Fold, Gate as Asked, Job, Link, LogItem, Next, Note, Pause, Phase,
    Planned, Read, Server, ServerLink, Want,
};
use crate::api::urls::{TransferBitrate, TransferCodec};

/// The transfer worker as a page borrows it for its visit: the way to it,
/// and the GUI's fold of its reports (clause 55: the worker is the GUI's).
#[derive(Clone)]
pub(crate) struct Transfer {
    pub(crate) link: Link,
    pub(crate) fold: Rc<RefCell<Fold>>,
}

/// The cards' own state on the page: the gate, the sheet, this computer's
/// cards.json for the board's card line, the fit request's number.
#[derive(Default)]
pub(crate) struct CardUi {
    pub(crate) gate: Option<CardGate>,
    pub(crate) sheet: Option<CardSheet>,
    /// `cards.json` as this visit read it (clause 20a).
    pub(crate) known: Option<Known>,
    /// The last fit asked for, so an older answer is never shown.
    pub(crate) seq: u64,
    /// The card a sync was last seen on, for the busy line's right edge.
    pub(crate) syncing: Option<String>,
    /// The cards on the row as the last look had them, in order: where a
    /// card taken out stood (clause 29).
    pub(crate) row: Vec<String>,
}

// ── The words ───────────────────────────────────────────────────────────────

/// `3 days ago`, `yesterday`, `5 min ago`, `just now`: `then` from `now`
/// (Unix seconds).
pub(super) fn ago(then: u64, now: u64) -> String {
    let secs = now.saturating_sub(then);
    match secs {
        0..60 => t!("dev.card.ago_now").to_string(),
        60..3_600 => t!("dev.card.ago_min", n = secs / 60).to_string(),
        3_600..86_400 => t!("dev.card.ago_hours", n = secs / 3_600).to_string(),
        86_400..172_800 => t!("dev.card.ago_yesterday").to_string(),
        _ => t!("dev.card.ago_days", n = secs / 86_400).to_string(),
    }
}

/// `25 min`, `5 h 12 min`, `40 s`.
pub(super) fn span(secs: u64) -> String {
    match secs {
        0..60 => t!("dev.card.dur_s", n = secs).to_string(),
        60..3_600 => t!("dev.card.dur_min", n = secs / 60).to_string(),
        _ => t!("dev.card.dur_h_min", h = secs / 3_600, m = secs % 3_600 / 60).to_string(),
    }
}

/// This computer's clock, in Unix seconds.
fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A conversion's words: `Opus 192k`.
pub(super) fn codec_words(codec: TransferCodec, bitrate: TransferBitrate) -> String {
    let name = match codec {
        TransferCodec::Opus => "Opus",
        TransferCodec::Mp3 => "MP3",
    };
    format!("{name} {}", bitrate.as_str())
}

/// The codecs the Into list offers (clause 48).
pub(super) const CODECS: [(TransferCodec, TransferBitrate); 8] = [
    (TransferCodec::Opus, TransferBitrate::K64),
    (TransferCodec::Opus, TransferBitrate::K96),
    (TransferCodec::Opus, TransferBitrate::K128),
    (TransferCodec::Opus, TransferBitrate::K192),
    (TransferCodec::Mp3, TransferBitrate::K128),
    (TransferCodec::Mp3, TransferBitrate::K192),
    (TransferCodec::Mp3, TransferBitrate::K256),
    (TransferCodec::Mp3, TransferBitrate::K320),
];

/// A card's name for a tab or a sentence: its own, else "Card in E:".
pub(super) fn card_name(card: &CardView) -> String {
    card.name().unwrap_or_else(|| t!("dev.card.name_in", drive = card.drive()).to_string())
}

/// The card's server's nickname: as the card keeps it, else as this
/// computer shows the server.
fn server_name(card: &CardView) -> String {
    let kept = card.facts().map(|f| f.nickname.clone()).filter(|n| !n.is_empty());
    let shown = card.server.link().map(|l| l.name.clone());
    kept.or(shown).unwrap_or_else(|| "?".to_string())
}

/// The plan the card's last look made, if its server answered.
fn plan_of(card: &CardView) -> Option<&Planned> {
    match &card.next {
        Next::Plan(plan) => Some(plan),
        _ => None,
    }
}

/// What a sync of the card would do: something waits.
fn waits(card: &CardView) -> bool {
    let pending = card.facts().is_some_and(|f| f.pending);
    plan_of(card).is_some_and(|p| p.new > 0 || !p.removals.is_empty() || p.plays > 0) || pending
}

/// The fit's verdict blocks a sync: it doesn't fit, or passes the cap.
fn blocked(fit: Option<&Fit>) -> bool {
    fit.is_some_and(|f| !f.fits() || f.over_cap())
}

/// The card's primary (clause 44): one or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CardPrimary {
    Sync,
    SetUp,
    Eject,
    AddServer,
    SignIn,
    CheckAgain,
    ChooseFits,
    NewIdentity,
}

impl CardPrimary {
    fn word(self) -> String {
        t!(match self {
            CardPrimary::Sync => "dev.card.btn_sync",
            CardPrimary::SetUp => "dev.card.btn_setup",
            CardPrimary::Eject => "dev.card.btn_eject",
            CardPrimary::AddServer => "dev.card.btn_add",
            CardPrimary::SignIn => "dev.card.btn_signin",
            CardPrimary::CheckAgain => "dev.card.btn_check",
            CardPrimary::ChooseFits => "dev.card.btn_fit",
            CardPrimary::NewIdentity => "dev.card.btn_new_id",
        })
        .to_string()
    }

    /// Its label on the card: its word and `▸` (the floor tests' word).
    #[cfg(test)]
    pub(crate) fn label(self) -> String {
        format!("{} {}", self.word(), glyphs().forward)
    }

    fn hint(self) -> String {
        t!(match self {
            CardPrimary::Sync => "dev.card.hint_enter_sync",
            CardPrimary::SetUp => "dev.card.hint_enter_setup",
            CardPrimary::Eject => "dev.card.hint_enter_eject",
            CardPrimary::AddServer => "dev.card.hint_enter_add",
            CardPrimary::SignIn => "dev.card.hint_enter_signin",
            CardPrimary::CheckAgain => "dev.card.hint_enter_check",
            CardPrimary::ChooseFits => "dev.card.hint_enter_fit",
            CardPrimary::NewIdentity => "dev.card.hint_enter_new_id",
        })
        .to_string()
    }
}

/// The card's primary, for what it is now.
pub(crate) fn primary(card: &CardView) -> Option<CardPrimary> {
    let busy = card.busy.is_some_and(|j| j != Job::Look);
    if card.run.is_some() || busy || card.out || card.paused().is_some() {
        return match card.paused() {
            Some(Pause::SignIn) => Some(CardPrimary::SignIn),
            _ => None,
        };
    }
    match &card.read {
        Read::Refused => {
            let unusable = card.found.refusals.iter().any(|r| matches!(r, Refusal::Filesystem(_) | Refusal::Table(_)));
            let shared = card.found.refusals.iter().any(|r| matches!(r, Refusal::Shared(_)));
            return (unusable && !shared).then_some(CardPrimary::Eject);
        }
        Read::Reading | Read::Failed(_) => return None,
        Read::Read(_) => {}
    }
    let facts = card.facts()?;
    if !facts.newer.is_empty() {
        return None;
    }
    if facts.check || card.gate.as_ref().is_some_and(|g| g.contains(&Asked::DiskCheck)) {
        return Some(CardPrimary::CheckAgain);
    }
    if facts.copied {
        return Some(CardPrimary::NewIdentity);
    }
    if !facts.set_up() {
        return Some(CardPrimary::SetUp);
    }
    match &card.server {
        Server::Unknown(_) | Server::New(None) => return Some(CardPrimary::AddServer),
        _ => {}
    }
    if matches!(card.next, Next::SignIn) || matches!(card.paused(), Some(Pause::SignIn)) {
        return Some(CardPrimary::SignIn);
    }
    if matches!(card.ended, Some(Ended::Fit(_)) | Some(Ended::Full { .. })) {
        return Some(CardPrimary::ChooseFits);
    }
    if blocked(plan_of(card).and_then(|p| p.fit.as_ref())) {
        return Some(CardPrimary::ChooseFits);
    }
    if matches!(card.ended, Some(Ended::Done { .. }) | Some(Ended::Refused { .. })) && !waits(card) {
        return Some(CardPrimary::Eject);
    }
    waits(card).then_some(CardPrimary::Sync)
}

/// A card's mark on its tab (clause 25a).
pub(super) fn mark(card: &CardView) -> (String, Color) {
    let g = glyphs();
    if card.out {
        return (t!("dev.card.tab_out").to_string(), th().dim);
    }
    if let Some(pct) = card.percent() {
        return (format!("{pct}%"), th().accent);
    }
    if let Some(Ended::Paused { bytes: (done, all), .. }) = &card.ended {
        let pct = if *all > 0 { done * 100 / all } else { 0 };
        return (format!("{}%", pct.min(100)), th().dim);
    }
    if card.run.is_some() || card.busy.is_some() || matches!(card.read, Read::Reading) {
        return (g.busy.to_string(), th().dim);
    }
    let cant = match &card.read {
        Read::Refused | Read::Failed(_) => true,
        Read::Read(f) => !f.newer.is_empty() || f.check,
        Read::Reading => false,
    };
    if cant {
        return (g.no.to_string(), th().gold);
    }
    match &card.next {
        Next::Asking => return (g.busy.to_string(), th().dim),
        Next::Unreached { .. } => return (g.unknown.to_string(), th().dim),
        _ => {}
    }
    match primary(card) {
        Some(CardPrimary::Eject) | None if !waits(card) => (g.ok.to_string(), th().ok),
        _ => (g.warn.to_string(), th().gold),
    }
}

/// A card's tab's name (clause 25): on Windows its drive, then its name;
/// elsewhere its name alone. `room` cells at most for the name.
pub(super) fn tab_name(card: &CardView, room: usize) -> String {
    let name = card.name().unwrap_or_else(|| t!("dev.card.tab_new").to_string());
    let name = if room < kit::width(&name) { fit(&name, room.max(6)) } else { name };
    if cfg!(windows) { format!("{} {name}", card.drive()) } else { name }
}

// ── The page's side ─────────────────────────────────────────────────────────

impl Page {
    /// The cards the worker knows, as the fold has them now.
    pub(super) fn cards(&self) -> Vec<CardView> {
        self.transfer.as_ref().map(|t| t.fold.borrow().cards.clone()).unwrap_or_default()
    }

    /// How many cards are on the row.
    pub(super) fn card_count(&self) -> usize {
        self.transfer.as_ref().map_or(0, |t| t.fold.borrow().cards.len())
    }

    /// The card in view, if the subject is a card.
    pub(super) fn open_card(&self) -> Option<CardView> {
        let key = self.card_open.as_deref()?;
        let transfer = self.transfer.as_ref()?;
        let fold = transfer.fold.borrow();
        fold.cards.iter().find(|c| c.key == key).cloned()
    }

    /// The servers saved on this computer, as the worker was last told.
    fn servers(&self) -> Vec<ServerLink> {
        self.transfer.as_ref().map(|t| t.fold.borrow().servers.clone()).unwrap_or_default()
    }

    pub(super) fn send_card(&mut self, cmd: worker::Cmd) {
        if let Some(t) = &self.transfer {
            t.link.send(cmd);
        }
    }

    /// The worker's notes and log lines since the last look, and the
    /// subject in view kept to what is on the row (clauses 25, 29, 55).
    pub(super) fn pump_cards(&mut self, now: Instant) {
        let Some(transfer) = self.transfer.clone() else { return };
        let (notes, logs, keys, syncing) = {
            let mut fold = transfer.fold.borrow_mut();
            let notes: Vec<Note> = fold.notes.drain(..).collect();
            let logs: Vec<(String, LogItem)> = fold.logs.drain(..).collect();
            let keys: Vec<String> = fold.cards.iter().map(|c| c.key.clone()).collect();
            (notes, logs, keys, fold.syncing().map(|c| c.key.clone()))
        };
        let drives: Vec<(String, String)> =
            self.cards().iter().map(|c| (c.key.clone(), c.drive())).collect();
        for (key, item) in logs {
            let drive = drives.iter().find(|(k, _)| *k == key).map(|(_, d)| d.clone());
            let text = log_words(&item, &drive.unwrap_or_default());
            let kind = if matches!(item, LogItem::Skipped { .. }) { LogKind::Fail } else { LogKind::Fact };
            self.log_at(now, Some(key.clone()), text, kind);
            // The files left out: a few of their places, a line each under
            // their count (a line is cut at the log's width, never wrapped).
            if let LogItem::Hidden(hidden) = &item {
                for place in hidden.places() {
                    self.log_at(now, Some(key.clone()), format!("  {place}"), LogKind::Quiet);
                }
            }
        }
        for note in notes {
            self.card_note(note, now);
        }
        // Coming back opens the view on the card being synced (clause 55).
        if self.card.syncing.is_none() && syncing.is_some() && self.boards.is_empty() {
            self.card_open = syncing.clone();
        }
        self.card.syncing = syncing;
        // A card gone takes its view with it to the next subject on the
        // row, right else left, as a board unplugged does (clause 29): left
        // of the first card is the last board.
        if let Some(open) = self.card_open.clone()
            && !keys.contains(&open)
        {
            self.card.gate = None;
            self.card.sheet = None;
            let row = std::mem::take(&mut self.card.row);
            let at = row.iter().position(|k| *k == open).unwrap_or(row.len());
            let right = row.iter().skip(at + 1).find(|k| keys.contains(k));
            let left = row.iter().take(at).rev().find(|k| keys.contains(k));
            self.card_open = right.or(left).cloned();
            if self.card_open.is_none()
                && let Some(board) = self.boards.last()
            {
                self.open = Some(board.port().to_string());
            }
        }
        self.card.row = keys.clone();
        // A card put in takes the view only from the empty card (clause 25,
        // the pick of 2026-10-19).
        if self.card_open.is_none() && self.boards.is_empty() {
            self.card_open = keys.first().cloned();
        }
    }

    /// A worker's note on the busy line (clauses 29, 54).
    fn card_note(&mut self, note: Note, now: Instant) {
        let cards = self.cards();
        let (text, gold, stays) = match note {
            Note::PutIn { drive } => (t!("dev.card.busy_put_in", drive = drive).to_string(), false, false),
            Note::Read { key } => {
                let Some(card) = cards.iter().find(|c| c.key == key) else { return };
                let Some(plan) = plan_of(card) else { return };
                let next = next_words(card, plan, true);
                let words = t!("dev.card.busy_read", drive = card.drive(), card = card_name(card), next = next);
                (words.to_string(), false, false)
            }
            Note::Out { drive, card } => {
                (t!("dev.card.busy_out", card = card, drive = drive).to_string(), false, false)
            }
            Note::OutRun { drive, card, at, of } => {
                let other = cards.iter().find(|c| c.drive() == drive && !c.out).map(card_name);
                let (i, n) = (fmt_count(at as u64), fmt_count(of as u64));
                let named = other.clone().unwrap_or_default();
                let mut words =
                    t!("dev.card.busy_out_run", card = card, drive = drive, i = i, n = n, other = named)
                        .to_string();
                if other.is_none() {
                    // Nothing in that reader now: the sentence ends at the count.
                    if let Some(cut) = words.find(". ") {
                        words.truncate(cut + 1);
                    }
                }
                (words, true, true)
            }
            Note::Synced { card, took, tracks } => {
                let words = t!("dev.card.done", card = card, t = span(took), n = fmt_count(tracks as u64));
                (words.to_string(), false, true)
            }
            Note::Ejected { card } => (t!("dev.card.ejected", card = card).to_string(), false, true),
            Note::EjectFailed { card, why } => (format!("{card}: {why}"), true, true),
            Note::PlaysSent { n, server } => {
                (t!("dev.card.done_plays", n = fmt_count(n as u64), server = server).to_string(), false, false)
            }
            Note::Failed { why } => (why, true, true),
        };
        self.say(text, gold, (!stays).then_some(now + NOTE_FOR));
    }

    /// `←` `→` over every subject, boards then cards, wrapping (clause 12).
    pub(super) fn switch_subject(&mut self, step: isize) {
        let cards = self.cards();
        let n = self.boards.len() + cards.len();
        if n < 2 {
            return;
        }
        let at = match &self.card_open {
            Some(key) => self.boards.len() + cards.iter().position(|c| &c.key == key).unwrap_or(0),
            None => self.shown_index().unwrap_or(0),
        };
        let next = (at as isize + step).rem_euclid(n as isize) as usize;
        if next < self.boards.len() {
            self.card_open = None;
            self.act(Act::Board(next));
        } else {
            self.act(Act::Card(next - self.boards.len()));
        }
    }

    /// The card's primary (clause 44), through its gate where it has one.
    pub(super) fn card_primary(&mut self) {
        let Some(card) = self.open_card() else { return };
        if self.lock.is_some() {
            return;
        }
        match primary(&card) {
            Some(CardPrimary::Sync) => self.sync_asked(&card),
            Some(CardPrimary::SetUp) => self.open_card_sheet(&card, true),
            Some(CardPrimary::ChooseFits) => self.open_card_sheet(&card, false),
            Some(CardPrimary::Eject) => self.send_card(worker::Cmd::Eject { key: card.key.clone() }),
            Some(CardPrimary::CheckAgain) => {
                let answers = Answers { disk_checked: true, ..Answers::default() };
                let allow = Box::new(Allow { answers, ..Allow::default() });
                self.send_card(worker::Cmd::Sync { key: card.key.clone(), allow });
            }
            Some(CardPrimary::NewIdentity) => self.send_card(worker::Cmd::NewIdentity { key: card.key.clone() }),
            // The GUI's own: its add-server form and its sign-in (clause 3a).
            Some(CardPrimary::AddServer) => self.wants = Some(Wants::AddServer),
            Some(CardPrimary::SignIn) => {
                let id = card.server.link().map(|l| l.id.clone());
                self.wants = Some(Wants::SignIn(id));
            }
            None => {}
        }
    }

    /// Sync ▸: its gates first, from the last look's plan (clause 58).
    fn sync_asked(&mut self, card: &CardView) {
        let plan = plan_of(card).cloned().unwrap_or_default();
        let gate = gate_for(card, &plan, Allow::default());
        match gate {
            Some(gate) => self.card.gate = Some(gate),
            None => self.send_card(worker::Cmd::Sync { key: card.key.clone(), allow: Box::default() }),
        }
    }

    /// A card's act: its tab, its links, its gate's and its sheet's.
    pub(super) fn card_act(&mut self, act: Act) {
        let card = self.open_card();
        let key = card.as_ref().map(|c| c.key.clone()).unwrap_or_default();
        let locked = self.lock.is_some();
        match act {
            Act::Card(i) => {
                let cards = self.cards();
                if let Some(card) = cards.get(i) {
                    self.card_open = Some(card.key.clone());
                    self.log_scroll = 0;
                }
            }
            Act::CardSettings if !locked => {
                if let Some(card) = card.filter(|c| c.facts().is_some_and(|f| f.set_up()) && c.run.is_none()) {
                    self.open_card_sheet(&card, false);
                }
            }
            // The plays wait for no sync: during one they are its own to send.
            Act::CardPlays if !locked => {
                if card.is_some_and(|c| c.run.is_none()) {
                    self.send_card(worker::Cmd::Plays { key });
                }
            }
            // `e` where Enter's Eject ▸ is, too: a card the player can't read
            // goes out on its way to the player's Format (clause 45).
            Act::CardEject if !locked => {
                let may = |c: &CardView| c.found.accepted() || primary(c) == Some(CardPrimary::Eject);
                if card.is_some_and(|c| c.run.is_none() && !c.out && may(&c)) {
                    self.send_card(worker::Cmd::Eject { key });
                }
            }
            // `x` on a card whose sync runs is honoured during a write too
            // (clause 9): the stop touches no board.
            Act::CardStop => {
                if card.is_some_and(|c| c.syncing()) {
                    self.send_card(worker::Cmd::Stop { key });
                }
            }
            Act::CardHide if !locked => {
                if card.is_some_and(|c| c.read == Read::Refused) {
                    self.send_card(worker::Cmd::Hide { key });
                }
            }
            Act::CardTryNow if !locked => {
                let allow = Box::default();
                match card.as_ref().and_then(|c| c.paused()) {
                    Some(_) => self.send_card(worker::Cmd::Sync { key, allow }),
                    None => self.send_card(worker::Cmd::Look { key }),
                }
            }
            Act::CardStopPause => self.send_card(worker::Cmd::StopPause { key }),
            Act::CardDontWait => self.send_card(worker::Cmd::DontWait { key }),
            Act::CardGiveUp => self.send_card(worker::Cmd::GiveUp { key }),
            Act::CardSyncAgain if !locked => {
                if let Some(card) = card {
                    self.sync_asked(&card);
                }
            }
            Act::CardOtherServer if !locked => {
                if let Some(card) = card {
                    self.open_card_sheet(&card, false);
                }
            }
            Act::CardKeep => self.gate_keep(),
            Act::CardGo => self.gate_go(),
            Act::CardBox => {
                if let Some(CardGate { kind: GateKind::Half { ticked, .. }, .. }) = self.card.gate.as_mut() {
                    *ticked = !*ticked;
                }
            }
            act @ (Act::CardSheetClose
            | Act::CardSheetTab(_)
            | Act::CardSheetRow(_)
            | Act::CardSheetPick(_)
            | Act::CardDefaults
            | Act::CardApply) => self.card_sheet_act(act),
            _ => {}
        }
    }

    /// The card's keys (clause 12): Enter, `d` `l`, `o`, `e`, `p`, `x`,
    /// `h`, `←` `→`, Esc. During a write only Details, the switch keys and
    /// `x` on a syncing card do anything (clause 9).
    pub(super) fn card_key(&mut self, key: KeyEvent) -> bool {
        let locked = self.lock.is_some();
        let act = match key.code {
            KeyCode::Enter if !locked => Some(Act::Primary),
            KeyCode::Char('d' | 'l') => Some(Act::Details),
            KeyCode::Char('o') if !locked => Some(Act::CardSettings),
            KeyCode::Char('e') if !locked => Some(Act::CardEject),
            KeyCode::Char('p') if !locked => Some(Act::CardPlays),
            KeyCode::Char('x') => Some(Act::CardStop),
            KeyCode::Char('h') if !locked => Some(Act::CardHide),
            KeyCode::Left => {
                self.switch_subject(-1);
                None
            }
            KeyCode::Right => {
                self.switch_subject(1);
                None
            }
            _ => return false,
        };
        if let Some(act) = act {
            self.act(act);
        }
        true
    }

    /// The footer's hint on a card (clause 15), in pieces and their place
    /// in the order they give way: `p` first, then `h`, `o` and `e`.
    pub(super) fn card_hint_parts(&self, card: &CardView, hosted: bool) -> Vec<(String, u8)> {
        let mut parts: Vec<(String, u8)> = Vec::new();
        if let Some(gate) = &self.card.gate {
            return gate_hint(gate);
        }
        if let Some(sheet) = &self.card.sheet {
            return sheet_hint(sheet);
        }
        let back = t!(if hosted { "dev.hint_library" } else { "dev.hint_leave" }).to_string();
        if self.boards.len() + self.card_count() >= 2 {
            parts.push((format!("{} {}", glyphs().switch, t!("dev.card.hint_switch")), 0));
        }
        if card.syncing() {
            parts.push((t!("dev.card.hint_stop").to_string(), 0));
            parts.push((t!("dev.hint_details").to_string(), 0));
            parts.push((back, 0));
            return parts;
        }
        if let Some(p) = primary(card).filter(|_| self.lock.is_none()) {
            parts.push((p.hint(), 0));
        }
        let set_up = card.facts().is_some_and(|f| f.set_up());
        if set_up && card.run.is_none() {
            parts.push((t!("dev.card.hint_settings").to_string(), 2));
        }
        if plan_of(card).is_some_and(|p| p.plays > 0) {
            parts.push((t!("dev.card.hint_plays").to_string(), 4));
        }
        if card.found.accepted() && primary(card) != Some(CardPrimary::Eject) && card.run.is_none() {
            parts.push((t!("dev.card.hint_eject").to_string(), 1));
        }
        if card.read == Read::Refused {
            parts.push((t!("dev.card.hint_hide").to_string(), 3));
        }
        parts.push((t!("dev.hint_details").to_string(), 0));
        parts.push((back, 0));
        parts
    }
}

/// What the GUI is asked to do for a card: its own add-server form, or a
/// sign-in to the card's server (clause 3a).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Wants {
    AddServer,
    SignIn(Option<String>),
}

/// The plan's words for Next sync (clause 44): `12 new · 2 to remove · 37
/// plays to send`, or "nothing new on Home". `busy`: the busy line's, its
/// first piece naming the server.
fn next_words(card: &CardView, plan: &Planned, busy: bool) -> String {
    let server = server_name(card);
    let mut parts = Vec::new();
    if plan.new > 0 {
        let n = fmt_count(plan.new as u64);
        parts.push(if busy {
            t!("dev.card.next_new_on", n = n, server = server).to_string()
        } else {
            t!("dev.card.next_new", n = n).to_string()
        });
    }
    if !plan.removals.is_empty() {
        parts.push(t!("dev.card.next_remove", n = fmt_count(plan.removals.len() as u64)).to_string());
    }
    if plan.plays > 0 {
        parts.push(t!("dev.card.next_plays", n = fmt_count(plan.plays as u64)).to_string());
    }
    if parts.is_empty() {
        let mut words = t!("dev.card.next_none", server = server).to_string();
        if !busy {
            words.push_str(&format!(" · {}", t!("dev.card.next_asked", when = ago(plan.asked, now()))));
        }
        return words;
    }
    parts.join(" · ")
}

/// A log line's words (Details, clause 44), on the card in `drive`.
fn log_words(item: &LogItem, drive: &str) -> String {
    match item {
        LogItem::PutIn { card, revision, tracks } => {
            let n = fmt_count(*tracks as u64);
            t!("dev.card.log_put_in", drive = drive, card = card, rev = revision, n = n).to_string()
        }
        LogItem::Plays { new, send } => {
            let (new, send) = (fmt_count(*new as u64), fmt_count(*send as u64));
            t!("dev.card.log_plays", n = new, send = send).to_string()
        }
        LogItem::Asked { server, new, removed, unchanged } => {
            let (n, m) = (fmt_count(*new as u64), fmt_count(*removed as u64));
            let key = if *unchanged { "dev.card.log_asked_same" } else { "dev.card.log_asked" };
            t!(key, server = server, n = n, m = m).to_string()
        }
        LogItem::KeptYours { file, server, copy } => {
            t!("dev.card.log_kept_yours", file = file, server = server, copy = copy).to_string()
        }
        LogItem::Skipped { path, why } => format!("{path} · {why}"),
        LogItem::Hidden(hidden) if hidden.n == 1 => t!("dev.card.log_hidden_one").to_string(),
        LogItem::Hidden(hidden) => {
            t!("dev.card.log_hidden", n = fmt_count(hidden.n as u64)).to_string()
        }
        LogItem::Said(text) => text.clone(),
    }
}

// ── The Card view ───────────────────────────────────────────────────────────

/// A card's or a library's size in the locale's unit: `127.9 GB`, `127.9 Go`.
fn size_words(bytes: u64) -> String {
    t!("dev.card_size", size = gb(bytes)).to_string()
}

/// The head's right edge: `E: · FAT32`, `G: · FAT32 on GPT`, `E: · FAT32
/// · read only`, `G: · exFAT · 127.9 GB`, or `not in a reader`.
fn head_right(card: &CardView) -> String {
    if card.out {
        return t!("dev.card.head_out").to_string();
    }
    let v = &card.found.volume;
    let mut fs = v.fs.name().to_string();
    if v.table == Table::Gpt {
        fs = t!("dev.card.head_on", fs = fs, table = "GPT").to_string();
    }
    let mut parts = vec![card.drive(), fs];
    let unusable = matches!(v.fs, Fs::ExFat | Fs::Ntfs | Fs::Fat12 | Fs::Other(_));
    if let Some(space) = v.space.filter(|_| unusable) {
        parts.push(size_words(space.size));
    }
    if v.read_only || card.found.disk.read_only {
        parts.push(t!("dev.card.head_read_only").to_string());
    }
    parts.join(" · ")
}

/// A gold problem row: `! words` bold, then dim lines under it.
fn problem(label: &str, glyph: &str, words: &str, lines: &[String], w: u16) -> Vec<Row> {
    let gold = Style::default().fg(th().gold);
    let pieces = [(format!("{glyph} "), gold), (words.to_string(), gold.add_modifier(Modifier::BOLD))];
    let mut rows = value_rows(Some(label.to_string()), &pieces, w);
    for line in lines {
        rows.extend(dim_rows(None, line, w));
    }
    rows
}

/// The Space row: the bar, and its numbers (clause 44).
fn space_rows(card: &CardView, bar: bool) -> Vec<Row> {
    let label = Some(t!("dev.card.label_space").to_string());
    let Some(space) = card.found.volume.space else { return Vec::new() };
    let used = space.size.saturating_sub(space.free);
    let mut left = t!("dev.card_used", used = gb(used), size = gb(space.size)).to_string();
    if let Some(facts) = card.facts().filter(|f| f.tracks > 0) {
        left.push_str(&format!(" · {}", t!("dev.card_tracks", n = fmt_count(facts.tracks as u64))));
    }
    let free = t!("dev.card_free", free = gb(space.free)).to_string();
    let full = space.size > 0 && (used * 100 / space.size >= 98 || space.free < 1_000_000_000);
    let right = if full {
        Line::from(Span::styled(format!("{} {free}", glyphs().warn), Style::default().fg(th().gold)))
    } else {
        Line::from(Span::raw(free))
    };
    let mut rows = Vec::new();
    if bar {
        let mut permille = (u128::from(used) * 1000 / u128::from(space.size.max(1))) as u32;
        if used > 0 {
            permille = permille.max(1);
        }
        let color = if full { th().gold } else { th().accent };
        rows.push(Row::Bar { label, permille, color });
        rows.push(Row::Value { label: None, left: Line::from(left), right: Some(right) });
    } else {
        rows.push(Row::Value { label, left: Line::from(left), right: Some(right) });
    }
    rows
}

/// The Last sync row (clause 44).
fn last_rows(card: &CardView, w: u16) -> Vec<Row> {
    let label = Some(t!("dev.card.label_last").to_string());
    let Some(facts) = card.facts() else { return Vec::new() };
    let gold = Style::default().fg(th().gold);
    let mut pieces: Vec<(String, Style)> = Vec::new();
    match &card.ended {
        Some(Ended::Done { skipped, plays, .. }) => {
            pieces.push((format!("{} · {}", t!("dev.card.ago_now"), t!("dev.card.verified")), Style::default()));
            if *plays > 0 {
                let words = t!("dev.card.done_plays", n = fmt_count(*plays as u64), server = server_name(card));
                pieces.push((format!(" · {words}"), Style::default()));
            }
            if !skipped.is_empty() {
                let words = t!("dev.card.skipped", n = fmt_count(skipped.len() as u64));
                pieces.push((format!(" · {} {words}", glyphs().warn), gold));
            }
        }
        _ if facts.tracks == 0 => {
            let words = match (facts.set_up(), facts.committed) {
                (true, Some(at)) => t!("dev.card.last_setup", when = ago(at, now())),
                _ => t!("dev.card.last_never"),
            };
            pieces.push((words.to_string(), Style::default()));
        }
        _ => {
            let when = facts.committed.map_or_else(|| "?".to_string(), |at| ago(at, now()));
            pieces.push((when, Style::default()));
        }
    }
    let mut rows = value_rows(label, &pieces, w);
    if let Some(Ended::Refused { peer, .. }) = &card.ended {
        let peer = peer_name(card, *peer);
        let mut words = t!("dev.card.err_refused", peer = peer.clone()).to_string();
        let libs = card
            .facts()
            .and_then(|f| f.state.as_ref())
            .and_then(|s| s.settings.peers.iter().find(|p| p.nickname == peer).map(|p| p.libraries.join(", ")))
            .unwrap_or_default();
        words.push_str(&format!(": {}", t!("dev.card.err_refused_2", libs = libs)));
        rows.extend(value_rows(None, &[(format!("{} {words}", glyphs().warn), gold)], w));
    }
    rows
}

/// A peer's name, as the card keeps it, else its id.
fn peer_name(card: &CardView, id: u32) -> String {
    let peers = card.facts().and_then(|f| f.state.as_ref()).map(|s| s.settings.peers.clone()).unwrap_or_default();
    let named = peers.iter().find(|p| p.peer_id == id).map(|p| p.nickname.clone()).filter(|n| !n.is_empty());
    named.unwrap_or_else(|| format!("#{id}"))
}

/// The Next sync row (clause 44): what a sync would do now, the server
/// not answering, or the fit's verdict.
fn next_rows(page: &Page, card: &CardView, w: u16) -> Vec<Row> {
    let label = Some(t!("dev.card.label_next").to_string());
    let g = glyphs();
    let gold = Style::default().fg(th().gold);
    let server = server_name(card);
    if card.run.as_ref().is_some_and(|r| r.phase == Phase::Waiting) {
        let running = page.cards().into_iter().find(|c| c.syncing()).map(|c| card_name(&c)).unwrap_or_default();
        let mut words = Vec::new();
        if let Some(plan) = plan_of(card).filter(|p| p.new > 0) {
            words.push(t!("dev.card.next_new", n = fmt_count(plan.new as u64)).to_string());
        }
        words.push(t!("dev.card.waits_for", card = running).to_string());
        return dim_rows(label, &words.join(" · "), w).into_iter().map(undim).collect();
    }
    match &card.next {
        Next::Unasked => Vec::new(),
        Next::Asking => value_rows(label, &scan(None, &t!("dev.card.next_asking", server = server)), w),
        Next::SignIn => {
            let plays = card.facts().and_then(|f| f.state.as_ref()).map_or(0, |s| s.plays.len());
            if plays == 0 {
                return Vec::new();
            }
            let words = t!("dev.card.next_signin", n = plays, server = server).to_string();
            value_rows(label, &[(words, Style::default())], w)
        }
        Next::Unreached { asked, .. } => {
            let words = t!("dev.card.next_unreached", server = server, when = ago(*asked, now()));
            value_rows(label, &[(words.to_string(), Style::default())], w)
        }
        Next::Refused(why) => value_rows(label, &[(why.clone(), dim())], w),
        Next::Plan(plan) => {
            if let Some(fit) = plan.fit.filter(|f| f.over_cap()) {
                let n = fmt_count(fit.tracks as u64);
                let head = t!("dev.card.fit_cap", n = n, max = fmt_count(TRACK_CAP as u64));
                let mut rows = value_rows(label, &[(format!("{} ", g.no), gold), (head.to_string(), gold)], w);
                let cards = fit.tracks.div_ceil(TRACK_CAP).max(2);
                rows.extend(dim_rows(None, &t!("dev.card.fit_cap_2", n = cards), w));
                return rows;
            }
            if let Some(fit) = plan.fit.filter(|f| !f.fits()) {
                let words = t!("dev.card.fit_no", gb = gb(fit.short()));
                return value_rows(label, &[(format!("{} ", g.no), gold), (words.to_string(), gold)], w);
            }
            value_rows(label, &[(next_words(card, plan, false), Style::default())], w)
        }
    }
}

/// A dim row made plain.
fn undim(row: Row) -> Row {
    match row {
        Row::Value { label, left, right } => {
            let spans: Vec<Span> = left.spans.into_iter().map(|s| Span::raw(s.content.to_string())).collect();
            Row::Value { label, left: Line::from(spans), right }
        }
        other => other,
    }
}

/// The Converts row (clause 44): the rule, and under it, dim, the player
/// assumed or a card for someone else.
fn converts_rows(card: &CardView, w: u16) -> Vec<Row> {
    let label = Some(t!("dev.card.label_converts").to_string());
    let Some(facts) = card.facts() else { return Vec::new() };
    let Some(state) = &facts.state else { return Vec::new() };
    let c = state.settings.conversion;
    let codec = codec_words(c.codec, c.bitrate);
    let words = match c.convert {
        Convert::Unplayable => t!("dev.card.conv_needed", codec = codec),
        Convert::Everything => t!("dev.card.conv_all", codec = codec),
    };
    let mut rows = value_rows(label, &[(words.to_string(), Style::default())], w);
    if !facts.device.present {
        rows.extend(dim_rows(None, &t!("dev.card.conv_default"), w));
    }
    if state.settings.for_someone_else {
        rows.extend(dim_rows(None, &t!("dev.card.conv_gift"), w));
    }
    rows
}

/// The rows of a sync under way, paused, or ended in an error (clauses
/// 51–53).
fn run_rows(page: &Page, card: &CardView, w: u16) -> Option<Vec<Row>> {
    let label = Some(t!("dev.card.label_syncing").to_string());
    let g = glyphs();
    let gold = Style::default().fg(th().gold);
    let server = server_name(card);
    if let Some(run) = card.run.as_ref().filter(|r| r.phase != Phase::Waiting) {
        let mut rows = Vec::new();
        match run.phase {
            Phase::Listing => {
                rows.extend(value_rows(label, &scan(None, &t!("dev.card.next_asking", server = server)), w));
            }
            Phase::Planning => rows.extend(value_rows(label, &scan(None, &t!("dev.card.sync_planning")), w)),
            Phase::Index | Phase::Verifying if run.files == 0 || run.phase == Phase::Verifying => {
                let key = if run.phase == Phase::Index { "dev.card.sync_index" } else { "dev.card.sync_verifying" };
                let words = t!(key);
                rows.extend(value_rows(label, &scan(None, &words), w));
            }
            _ => {
                let permille = card.percent().map_or(0, |p| u32::from(p) * 10);
                rows.push(Row::Bar { label, permille, color: th().accent });
                let count = t!(
                    "dev.card.sync_count",
                    i = fmt_count(run.done_files as u64),
                    n = fmt_count(run.files as u64),
                    done = gb(run.done_bytes),
                    total = gb(run.bytes)
                );
                let left_bytes = run.bytes.saturating_sub(run.done_bytes);
                let right = run.rate.filter(|r| *r > 0.0).map(|rate| {
                    let secs = (left_bytes as f64 / rate) as u64;
                    Line::from(t!("dev.card.sync_left", t = span(secs.max(60))).to_string())
                });
                rows.push(Row::Value { label: None, left: Line::from(count.to_string()), right });
                if let Some(rate) = run.rate.filter(|r| *r > 0.0) {
                    let saved = run.saved.map_or_else(|| "?".to_string(), |at| ago(at, now()));
                    let words = t!("dev.card.sync_rate", rate = format!("{:.1}", rate / 1e6), when = saved);
                    rows.extend(dim_rows(None, &words, w));
                }
                if run.phase == Phase::Index {
                    rows.extend(value_rows(None, &scan(None, &t!("dev.card.sync_index")), w));
                }
            }
        }
        if let Some(now_path) = &run.now {
            let label = Some(t!("dev.card.label_now").to_string());
            rows.extend(value_rows(label, &[(now_path.clone(), Style::default())], w));
        }
        return Some(rows);
    }
    let ended = card.ended.as_ref()?;
    let bar = |at: usize, of: usize| -> Vec<Row> {
        let permille = if of > 0 { (at * 1000 / of) as u32 } else { 0 };
        vec![Row::Bar { label: label.clone(), permille, color: th().dim }]
    };
    let count = |at: usize, of: usize| -> String {
        t!("dev.card.sync_of", i = fmt_count(at as u64), n = fmt_count(of as u64)).to_string()
    };
    let rows = match ended {
        Ended::Paused { pause, at, of, .. } => {
            let mut rows = bar(*at, *of);
            rows.extend(value_rows(None, &[(count(*at, *of), Style::default())], w));
            let (why, more) = match pause {
                Pause::Budget { until, peer, days, mb } => {
                    let peer = peer.parse().map(|id| peer_name(card, id)).unwrap_or_else(|_| peer.clone());
                    let mb = fmt_count(*mb);
                    let why = t!("dev.card.paused_budget", peer = peer, mb = mb);
                    let left = span(until.saturating_sub(now()));
                    let more = t!("dev.card.paused_budget_2", t = left, days = days);
                    (why.to_string(), Some(more.to_string()))
                }
                Pause::Server { until, tries } => {
                    let left = span(until.saturating_sub(now()));
                    (t!("dev.card.paused_server", server = server, t = left, n = tries).to_string(), None)
                }
                Pause::SignIn => (
                    t!("dev.card.paused_signin", server = server).to_string(),
                    Some(t!("dev.card.paused_signin_2").to_string()),
                ),
            };
            let pieces = [
                (format!("{} ", g.warn), gold),
                (t!("dev.card.paused").to_string(), gold.add_modifier(Modifier::BOLD)),
                (format!(" · {why}"), gold),
            ];
            rows.extend(value_rows(None, &pieces, w));
            if let Some(more) = more {
                rows.extend(dim_rows(None, &more, w));
            }
            rows
        }
        Ended::Full { at, of } => {
            let mut rows = bar(*at, *of);
            let words = t!("dev.card.err_full_2", i = fmt_count(*at as u64), n = fmt_count(*of as u64));
            rows.extend(problem_lines(g.no, &t!("dev.card.err_full"), &words, w));
            rows.extend(dim_rows(None, &t!("dev.card.err_full_why"), w));
            rows
        }
        Ended::Locked { at, of } => {
            let mut rows = bar(*at, *of);
            rows.extend(problem_lines(g.no, &t!("dev.card.err_locked"), &t!("dev.card.err_locked_why"), w));
            rows.extend(dim_rows(None, &t!("dev.card.err_locked_2"), w));
            rows
        }
        Ended::Out { at, of } => {
            let words = t!("dev.card.err_out_2", i = fmt_count(*at as u64), n = fmt_count(*of as u64));
            let pieces = [
                (format!("{} ", g.no), gold),
                (t!("dev.card.err_out").to_string(), gold.add_modifier(Modifier::BOLD)),
                (format!(" · {words}"), gold),
            ];
            let mut rows = value_rows(label, &pieces, w);
            let other = page.cards().into_iter().find(|c| c.drive() == card.drive() && c.key != card.key);
            if let Some(other) = other {
                let words = t!("dev.card.err_other_card", drive = card.drive(), other = card_name(&other));
                rows.extend(dim_rows(None, &words, w));
            }
            return Some(rows);
        }
        Ended::OtherCard => {
            let words = t!("dev.card.err_other_card", drive = card.drive(), other = "?");
            problem(&t!("dev.card.label_syncing"), g.no, &words, &[], w)
        }
        Ended::Failed(why) => problem(&t!("dev.card.label_syncing"), g.no, why, &[], w),
        Ended::Stopped { .. } | Ended::Done { .. } | Ended::Fit(_) | Ended::Refused { .. } => return None,
    };
    Some(rows)
}

/// A gold line in the value column: the glyph, the words bold, the rest
/// after ` · `.
fn problem_lines(glyph: &str, words: &str, rest: &str, w: u16) -> Vec<Row> {
    let gold = Style::default().fg(th().gold);
    let pieces = [
        (format!("{glyph} "), gold),
        (words.to_string(), gold.add_modifier(Modifier::BOLD)),
        (format!(" · {rest}"), gold),
    ];
    value_rows(None, &pieces, w)
}

/// Every row of `card`'s view, top to bottom (clause 44).
pub(super) fn card_layout(page: &Page, card: &CardView, content_w: u16) -> Vec<Row> {
    let w = content_w.saturating_sub(LABEL_W);
    let g = glyphs();
    let mut rows = vec![Row::Blank];
    let name = card_name(card);
    let right = Line::from(Span::styled(head_right(card), dim()));
    rows.push(Row::Wide { left: Line::from(Span::styled(name, bold())), right: Some(right) });
    rows.push(Row::Blank);
    let holds = t!("dev.card.label_holds").to_string();
    let mut links: Vec<(String, Act)> = Vec::new();
    let details = format!("{} {}", if page.details { g.open } else { g.forward }, t!("dev.details"));
    links.push((details, Act::Details));
    let locked = page.lock.is_some();
    match &card.read {
        Read::Reading => rows.extend(value_rows(Some(holds), &scan(None, &t!("dev.card.reading")), w)),
        Read::Failed(why) => rows.extend(problem(&holds, g.no, why, &[], w)),
        Read::Refused => {
            let refusals = &card.found.refusals;
            if let Some(Refusal::Shared(others)) = refusals.iter().find(|r| matches!(r, Refusal::Shared(_))) {
                let fs = others.join(", ");
                let lines = [t!("dev.card.shared", fs = fs).to_string(), t!("dev.card.shared_2").to_string()];
                rows.extend(problem(&holds, g.warn, &t!("dev.card.shared_title"), &lines, w));
            } else if let Some(r) = refusals.iter().find(|r| matches!(r, Refusal::Filesystem(_) | Refusal::Table(_))) {
                let (what, why) = match r {
                    Refusal::Table(_) => (t!("dev.card_gpt"), t!("dev.card_mbr_only")),
                    Refusal::Filesystem(Fs::ExFat) => (t!("dev.card_exfat"), t!("dev.card_fat32_only")),
                    Refusal::Filesystem(Fs::Ntfs) => (t!("dev.card_ntfs"), t!("dev.card_fat32_only")),
                    _ => (t!("dev.card.not_fat_card", fs = card.found.volume.fs.name()), t!("dev.card_fat32_only")),
                };
                let route = match page.boards.iter().find(|b| b.ours()) {
                    Some(board) => t!("dev.card.route_board", port = short(board.port())),
                    None => t!("dev.card.route_none"),
                };
                let label = t!("dev.card.label_format").to_string();
                let pieces = [
                    (format!("{} ", g.warn), Style::default().fg(th().gold)),
                    (what.to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD)),
                    (format!(" · {why}"), Style::default().fg(th().gold)),
                ];
                rows.extend(value_rows(Some(label), &pieces, w));
                rows.extend(dim_rows(None, &route, w));
            } else if refusals.iter().any(|r| matches!(r, Refusal::ReadOnly)) {
                let lines = [t!("dev.card.err_locked_2").to_string()];
                rows.extend(problem(&holds, g.warn, &t!("dev.card.locked"), &lines, w));
                rows.extend(space_rows(card, false));
            } else if refusals.iter().any(|r| matches!(r, Refusal::NotUtf8)) {
                let lines = [t!("dev.card.not_utf8_2").to_string()];
                rows.extend(problem(&holds, g.warn, &t!("dev.card.not_utf8"), &lines, w));
            }
            links.push((t!("dev.card.link_hide").to_string(), Act::CardHide));
        }
        Read::Read(facts) => {
            let set_up = facts.set_up();
            if !facts.newer.is_empty() {
                let lines = [t!("dev.card.newer_2").to_string()];
                let words = match facts.newer_major {
                    Some(major) => t!("dev.card.newer", major = major),
                    None => t!("dev.card.newer_any"),
                };
                rows.extend(problem(&holds, g.unknown, &words, &lines, w));
                rows.extend(space_rows(card, false));
            } else if facts.check || card.gate.as_ref().is_some_and(|g| g.contains(&Asked::DiskCheck)) {
                let label = t!("dev.card.label_check").to_string();
                let lines = [
                    t!("dev.card.check_2").to_string(),
                    t!("dev.card.check_win", drive = card.drive()).to_string(),
                    t!("dev.card.check_mac").to_string(),
                ];
                rows.extend(problem(&label, g.no, &t!("dev.card.check"), &lines, w));
            } else if facts.copied {
                let lines = [t!("dev.card.copied_2").to_string()];
                rows.extend(problem(&holds, g.warn, &t!("dev.card.copied"), &lines, w));
                rows.extend(space_rows(card, true));
            } else if !set_up {
                if facts.music.unlisted > 0 {
                    let n = fmt_count(u64::from(facts.music.unlisted));
                    rows.extend(problem(&holds, g.warn, &t!("dev.card.foreign", n = n), &[], w));
                } else {
                    let lines = [t!("dev.card.new_body_2").to_string()];
                    rows.extend(problem(&holds, g.warn, &t!("dev.card.new_body"), &lines, w));
                }
                rows.extend(space_rows(card, true));
                rows.extend(last_rows(card, w));
            } else {
                let libs = facts.libraries().join(", ");
                let server = server_name(card);
                let holds_words = if libs.is_empty() {
                    t!("dev.card.holds_none").to_string()
                } else {
                    t!("dev.card.holds_from", libs = libs, server = server).to_string()
                };
                rows.extend(value_rows(Some(holds.clone()), &[(holds_words, Style::default())], w));
                if let Some((i, n)) = facts.set {
                    let words = t!("dev.card.set_of", i = i, n = n);
                    let label = Some(t!("dev.card.label_set").to_string());
                    rows.extend(value_rows(label, &[(words.to_string(), Style::default())], w));
                }
                let label = t!("dev.card.label_server").to_string();
                match &card.server {
                    Server::Unknown(nick) => {
                        let nick = if nick.is_empty() { "?".to_string() } else { nick.clone() };
                        let lines = [t!("dev.card.unknown_server_2").to_string()];
                        rows.extend(problem(&label, g.warn, &t!("dev.card.unknown_server", server = nick), &lines, w));
                    }
                    Server::New(None) => {
                        let lines = [t!("dev.card.no_server_2").to_string()];
                        rows.extend(problem(&label, g.warn, &t!("dev.card.no_server"), &lines, w));
                    }
                    _ if matches!(card.next, Next::SignIn) || matches!(card.paused(), Some(Pause::SignIn)) => {
                        rows.extend(problem(&label, g.warn, &t!("dev.card.signin", server = server), &[], w));
                    }
                    _ => {}
                }
                match run_rows(page, card, w) {
                    Some(run) => rows.extend(run),
                    None => {
                        rows.extend(space_rows(card, true));
                        rows.extend(last_rows(card, w));
                        // With no server to ask, nothing to say of the next
                        // sync or what it converts (card 03).
                        if card.server.link().is_some() {
                            rows.extend(next_rows(page, card, w));
                            rows.extend(converts_rows(card, w));
                        }
                    }
                }
                let syncing = card.syncing();
                if syncing {
                    let run = card.run.as_ref().expect("a run");
                    if run.stopping {
                        // The stop is under way: no link, the words say so.
                        // The run fetches one file at a time: that one
                        // finishes, then the save, with no count to give.
                        links.push((t!("dev.card.stopping").to_string(), Act::Details));
                    } else if matches!(run.phase, Phase::Listing | Phase::Planning) {
                        links.push((t!("dev.card.link_stop_plain").to_string(), Act::CardStop));
                    } else if run.phase != Phase::Verifying {
                        links.push((t!("dev.card.link_stop").to_string(), Act::CardStop));
                    }
                } else if card.run.is_some() {
                    links.push((t!("dev.card.link_dont_wait").to_string(), Act::CardDontWait));
                } else if card.out {
                    links.push((t!("dev.card.link_give_up").to_string(), Act::CardGiveUp));
                } else if let Some(pause) = card.paused() {
                    if matches!(pause, Pause::Server { .. }) {
                        links.push((t!("dev.card.link_try_now").to_string(), Act::CardTryNow));
                    }
                    links.push((t!("dev.card.link_stop_now").to_string(), Act::CardStopPause));
                } else {
                    if matches!(card.next, Next::Unreached { .. }) {
                        links.push((t!("dev.card.link_try_now").to_string(), Act::CardTryNow));
                    }
                    if matches!(card.server, Server::Unknown(_)) {
                        let others = page.servers();
                        if let Some(other) = others.first() {
                            let words = t!("dev.card.link_other_server", server = other.name.clone());
                            links.push((words.to_string(), Act::CardOtherServer));
                        }
                    } else {
                        links.push((t!("dev.card.link_settings").to_string(), Act::CardSettings));
                    }
                    if plan_of(card).is_some_and(|p| p.plays > 0) {
                        links.push((t!("dev.card.link_plays").to_string(), Act::CardPlays));
                    }
                }
            }
        }
    }
    // Eject beside the links, but where it is the primary.
    let primary = primary(card);
    let accepted = card.found.accepted();
    let still = card.run.is_none() && !card.out && card.paused().is_none();
    if accepted && primary != Some(CardPrimary::Eject) && still {
        links.push((t!("dev.card.link_eject").to_string(), Act::CardEject));
    }
    rows.push(Row::Blank);
    // Under its gate or its sheet the card keeps its rows, as the design
    // draws it, and its primary keeps its place undrawn: a modal shorter
    // than the card leaves no half a button showing.
    let covered = page.card.gate.is_some() || page.card.sheet.is_some();
    let primary = primary.map(|p| {
        let word = p.word();
        match page.lock.as_ref() {
            Some(_) if !locked => (word, false, None),
            Some(_) => {
                let port = page.writing_port().map(short).unwrap_or_default();
                (word, false, Some(t!("dev.fw_one_at_a_time", port = port).to_string()))
            }
            None => (format!("{word} {}", g.forward), true, None),
        }
    });
    rows.push(Row::Actions { links: give_way(links, primary.as_ref(), content_w), primary, covered });
    if page.details {
        rows.extend(details_rows(card, w));
    }
    rows.push(Row::Blank);
    rows
}

/// The rows in `inside` rows at most (clause 4): short of rows the Card
/// view gives way its dim explanation lines first, the last first, then the
/// Converts row; never the head, Holds, Space or the bottom row.
pub(super) fn give_way_rows(rows: &mut Vec<Row>, inside: u16) {
    let height = |rows: &[Row]| rows.iter().map(Row::height).sum::<u16>();
    let dim_line = |row: &Row| match row {
        Row::Value { label: None, left, right: None } => {
            !left.spans.is_empty() && left.spans.iter().all(|s| s.style == dim())
        }
        _ => false,
    };
    let converts = t!("dev.card.label_converts").to_string();
    while height(rows) > inside {
        if let Some(at) = rows.iter().rposition(dim_line) {
            rows.remove(at);
            continue;
        }
        let at = rows.iter().position(|r| matches!(r, Row::Value { label: Some(l), .. } if *l == converts));
        let Some(at) = at else { break };
        rows.remove(at);
        while matches!(rows.get(at), Some(Row::Value { label: None, right: None, .. })) {
            rows.remove(at);
        }
    }
}

/// The bottom row's links, short of room before the primary: Send the
/// plays gives way first, then the links from the end (clause 44).
fn give_way(
    mut links: Vec<(String, Act)>,
    primary: Option<&(String, bool, Option<String>)>,
    w: u16,
) -> Vec<(String, Act)> {
    let room = usize::from(w).saturating_sub(primary.map_or(0, |(label, ..)| usize::from(kit::tall_width(label)) + 2));
    let width = |links: &[(String, Act)]| -> usize {
        links.iter().map(|(t, _)| kit::width(t)).sum::<usize>() + 3 * links.len().saturating_sub(1)
    };
    if width(&links) > room
        && let Some(at) = links.iter().position(|(_, a)| *a == Act::CardPlays)
    {
        links.remove(at);
    }
    links
}

/// Details (clause 44): the facts, a line each in the value column's `w`
/// cells, then the transfer's log.
fn details_rows(card: &CardView, w: u16) -> Vec<Row> {
    let mut rows = vec![Row::Rule];
    let fact = |label: String, value: String| Row::Fact { label, value, style: Style::default() };
    rows.push(fact(t!("dev.card.fact_drive").to_string(), drive_fact(card, w)));
    if let Some(facts) = card.facts() {
        if let Some((id, generation, _)) = facts.root {
            let version = facts.producer.rsplit(' ').next().unwrap_or_default().to_string();
            let words = t!("dev.card.fact_card_v", id = format!("{id:016x}"), rev = generation, version = version);
            rows.push(fact(t!("dev.label_card_fact").to_string(), words.to_string()));
        }
        let mut server = server_name(card);
        if card.server.link().is_some() {
            server.push_str(&format!(" · {}", t!("dev.card.fact_signed_in")));
        }
        rows.push(fact(t!("dev.card.label_server").to_string(), server));
        if let Some(plan) = plan_of(card).filter(|p| !p.removals.is_empty()) {
            let more = plan.removals.len() - 1;
            let words = if more == 0 { plan.removals[0].clone() } else { removes_fact(&plan.removals[0], more, w) };
            rows.push(fact(t!("dev.card.fact_removes").to_string(), words));
        }
        if let Some(tally) = plan_of(card).and_then(|p| p.tally.as_ref()) {
            let words = t!(
                "dev.card.fact_plays_v",
                n = fmt_count(tally.owed.max(tally.due) as u64),
                old = fmt_count(tally.clock as u64),
                mine = fmt_count(tally.own as u64)
            );
            rows.push(fact(t!("dev.card.label_plays").to_string(), words.to_string()));
        }
        let d = &facts.device;
        if d.present {
            let adj = if d.read_mpdj > 0 { d.read_mpdj.to_string() } else { "-".to_string() };
            let (version, codecs) = (d.firmware.clone(), d.codecs.clone());
            let words =
                t!("dev.card.fact_player_v", name = "Core2", version = version, codecs = codecs, adj = adj);
            rows.push(fact(t!("dev.card.row_player").to_string(), words.to_string()));
        }
    }
    if let Some(Ended::Done { skipped, .. }) = &card.ended {
        for (i, (path, why)) in skipped.iter().enumerate() {
            let label = if i == 0 { t!("dev.card.fact_skipped").to_string() } else { String::new() };
            rows.push(Row::Fact { label, value: format!("{path} · {why}"), style: Style::default() });
        }
    }
    rows.push(Row::Log);
    if card.run.is_none() && card.facts().is_some_and(|f| f.set_up()) && card.found.accepted() {
        rows.push(Row::Blank);
        let text = t!("dev.card.link_sync_again").to_string();
        rows.push(Row::Link { wide: true, text, act: Act::CardSyncAgain, note: None });
    }
    rows
}

/// The Drive fact in `w` cells, in the locale's words a piece at a time (a
/// card may lack its size, its clusters or its serial). Short of cells (a
/// volume's long name, on Linux or macOS) its serial gives way first, then
/// its clusters, each whole, never cut (clause 44).
fn drive_fact(card: &CardView, w: u16) -> String {
    let v = &card.found.volume;
    let table = v.table.name().to_uppercase();
    let fs = t!("dev.card.head_on", fs = v.fs.name(), table = table).to_string();
    let mut drive = vec![card.drive(), fs];
    // The pieces that give way, the last first.
    let mut spare = Vec::new();
    if let Some(space) = v.space {
        drive.push(size_words(space.size));
        if space.cluster > 0 {
            spare.push(t!("dev.card.fact_drive_kb", kb = space.cluster / 1024).to_string());
        }
    }
    if let Some(serial) = &v.serial {
        spare.push(serial.to_string());
    }
    loop {
        let words = drive.iter().chain(&spare).map(String::as_str).collect::<Vec<_>>().join(" · ");
        if kit::width(&words) <= usize::from(w) || spare.pop().is_none() {
            return words;
        }
    }
}

/// The Removes fact in `w` cells: the first path and how many more. Short
/// of cells the path gives way, cut with an ellipsis, never the count
/// (clause 44).
fn removes_fact(path: &str, more: usize, w: u16) -> String {
    let n = fmt_count(more as u64);
    let words = |path: &str| t!("dev.card.fact_removes_v", path = path, n = n.as_str()).to_string();
    let whole = words(path);
    let over = kit::width(&whole).saturating_sub(usize::from(w));
    if over == 0 {
        return whole;
    }
    words(&fit(path, kit::width(path).saturating_sub(over)))
}

// ── The board's card line (clause 20a) ──────────────────────────────────────

/// The line under a board's SD card row naming its card, from `cid=`.
pub(super) fn board_lines(page: &Page, board: &Board) -> Vec<String> {
    use crate::device::board::CardRoot;
    let Some(status) = board.status() else { return Vec::new() };
    match status.cid {
        CardRoot::Unknown => Vec::new(),
        CardRoot::None => match status.card {
            CardKind::Fat32 | CardKind::Fat16 => vec![t!("dev.card.board_unfilled").to_string()],
            _ => Vec::new(),
        },
        CardRoot::Loaded(r) => {
            let known = page.card.known.as_ref().and_then(|k| k.cards.get(&r.card_id));
            match known {
                Some(seen) if seen.synced > 0 => {
                    let server = if seen.server.is_empty() { "?".to_string() } else { seen.server.clone() };
                    let label = if seen.label.is_empty() { "?".to_string() } else { seen.label.clone() };
                    let when = ago(seen.synced, now());
                    let is = t!("dev.card.board_is", card = label, when = when, server = server);
                    let mut lines = vec![is.to_string()];
                    if r.generation < seen.generation {
                        lines.push(t!("dev.card.board_behind").to_string());
                    }
                    lines
                }
                _ => vec![t!("dev.card.board_elsewhere").to_string()],
            }
        }
    }
}

// ── The gates (clause 58) ───────────────────────────────────────────────────

/// A gate of a card's: what it asks, and what a yes sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CardGate {
    pub(crate) key: String,
    pub(crate) kind: GateKind,
    /// The answers already given, the next sync's with this gate's yes.
    pub(crate) allow: Allow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GateKind {
    /// More than 50 removals.
    Removals { card: String, server: String, n: usize, first: Vec<String> },
    /// More than half the card: the box, its counts per library.
    Half { card: String, server: String, n: usize, total: usize, libs: Vec<(String, usize, usize)>, ticked: bool },
    /// The listener's files the server also has.
    Adopt { drive: String, server: String, n: usize, all: usize },
    /// A sync over a relay, its bytes.
    Relay { server: String, bytes: u64 },
    /// A peer reached only through its parent's proxy.
    Proxy { peer: String, id: u32, server: String },
    /// The card's server becomes another. `want`: the Settings sheet's
    /// choice, which the yes writes (Apply, clause 48), the next sync
    /// taking the card over; None, a sync that stopped for it, which the
    /// yes runs again taking it over.
    Server { card: String, old: String, new: String, rows: usize, plays: usize, want: Option<Box<Want>> },
}

/// The gate a Sync ▸ passes first, if any: the worker's, from the plan's
/// guards, in their order, then the relay's (clause 58). The command line
/// asks the same ones (clause 67).
fn gate_for(card: &CardView, plan: &Planned, allow: Allow) -> Option<CardGate> {
    let key = card.key.clone();
    let kind = match worker::gate_ahead(card, plan, &allow) {
        Some(ahead) => gate_of_asked(card, &ahead)?,
        None if worker::relay_ahead(card, plan) && !allow.proxy_copies.contains(&0) => {
            GateKind::Relay { server: server_name(card), bytes: plan.bytes }
        }
        None => return None,
    };
    Some(CardGate { key, kind, allow })
}

/// A gate from the worker's ask, after a sync stopped for it.
fn gate_of_asked(card: &CardView, asked: &Asked) -> Option<GateKind> {
    let server = server_name(card);
    let name = card_name(card);
    Some(match asked {
        Asked::Removals { n, first } => GateKind::Removals { card: name, server, n: *n, first: first.clone() },
        Asked::Half { n, total, by_library } => {
            GateKind::Half { card: name, server, n: *n, total: *total, libs: by_library.clone(), ticked: false }
        }
        Asked::Adopt { n, all } => GateKind::Adopt { drive: card.drive(), server, n: *n, all: *all },
        Asked::Server { rows } => {
            let new = card.server.link().map(|l| l.name.clone()).unwrap_or_default();
            let old = card.facts().map(|f| f.nickname.clone()).unwrap_or_default();
            let plays = card.facts().and_then(|f| f.state.as_ref()).map_or(0, |s| s.plays.len());
            GateKind::Server { card: name, old, new, rows: *rows, plays, want: None }
        }
        Asked::Proxy { peer } => GateKind::Proxy { peer: peer_name(card, *peer), id: *peer, server },
        Asked::DiskCheck => return None,
    })
}

impl Page {
    /// A gate the worker asked after a sync stopped for it opens on the
    /// card in view (the sync was the listener's own Sync ▸).
    pub(super) fn card_asks(&mut self) {
        if self.card.gate.is_some() || self.card.sheet.is_some() || self.modal_board() {
            return;
        }
        let Some(card) = self.open_card() else { return };
        let Some(asks) = card.gate.as_ref() else { return };
        if let Some(kind) = asks.iter().find_map(|a| gate_of_asked(&card, a)) {
            self.card.gate = Some(CardGate { key: card.key.clone(), kind, allow: Allow::default() });
        }
    }

    /// A board's modal is up.
    fn modal_board(&self) -> bool {
        self.gate.is_some() || self.sheet.is_some()
    }

    /// The gate's keep (Enter, Esc, `n`): what stays stays, and nothing is
    /// written. Two keeps are answers of their own (the set's card 06):
    /// *Leave them all as mine* syncs without adopting, as `--keep-mine`
    /// does; *Leave Lisbon out* keeps that peer out of this card's syncs
    /// here, never asked again (clause 58: once per peer).
    fn gate_keep(&mut self) {
        let Some(gate) = self.card.gate.take() else { return };
        match gate.kind {
            GateKind::Adopt { .. } => {
                let mut allow = gate.allow;
                allow.answers.adopt = Some(false);
                self.gate_next(gate.key, allow);
            }
            GateKind::Proxy { id, .. } => self.send_card(worker::Cmd::LeaveOut { key: gate.key, peer: id }),
            // The sheet's choice, unwritten: there is no ask to forget.
            GateKind::Server { want: Some(_), .. } => {}
            _ => self.send_card(worker::Cmd::Dismiss { key: gate.key }),
        }
    }

    /// The gate's go: the sync, with this gate's yes among its answers —
    /// or the next gate, if the plan has one more. A removals gate's yes
    /// names its count: the run removes that many, or asks again.
    fn gate_go(&mut self) {
        let Some(gate) = self.card.gate.clone() else { return };
        let mut allow = gate.allow.clone();
        match &gate.kind {
            GateKind::Half { ticked: false, .. } => return,
            GateKind::Half { n, .. } => {
                allow.answers.removals = true;
                allow.answers.mass_removals = true;
                allow.answers.removals_seen = Some(*n);
            }
            GateKind::Removals { n, .. } => {
                allow.answers.removals = true;
                allow.answers.removals_seen = Some(*n);
            }
            GateKind::Adopt { .. } => allow.answers.adopt = Some(true),
            GateKind::Relay { .. } => allow.proxy_copies.push(0),
            GateKind::Proxy { id, .. } => allow.proxy_copies.push(*id),
            // From Settings: the yes writes the settings, and only them
            // (Apply never syncs, clause 48); the next sync takes it over.
            GateKind::Server { want: Some(want), .. } => {
                self.card.gate = None;
                self.send_card(worker::Cmd::Apply { key: gate.key, want: want.clone() });
                return;
            }
            GateKind::Server { want: None, .. } => allow.take_over = true,
        }
        self.card.gate = None;
        self.gate_next(gate.key, allow);
    }

    /// After a gate's answer: the plan's next gate, else the sync with
    /// every answer given.
    fn gate_next(&mut self, key: String, mut allow: Allow) {
        let card = self.open_card().filter(|c| c.key == key);
        let next = card.as_ref().and_then(|c| gate_for(c, &plan_of(c).cloned().unwrap_or_default(), allow.clone()));
        match next {
            Some(next) => self.card.gate = Some(next),
            None => {
                allow.proxy_copies.retain(|id| *id != 0);
                self.send_card(worker::Cmd::Sync { key, allow: Box::new(allow) });
            }
        }
    }

    /// The gate's keys (clause 23's): `y` goes, Enter Esc `n` keep, `e`
    /// flips the stronger gate's box.
    pub(super) fn card_gate_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') => self.card_act(Act::CardGo),
            KeyCode::Char('e') => self.card_act(Act::CardBox),
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('n') => self.card_act(Act::CardKeep),
            _ => {}
        }
    }
}

/// A gate line: text, a blank that gives way, or the box.
enum GLine {
    Text(Line<'static>),
    Blank,
    Box(Line<'static>),
}

/// The gate's words, wrapped in `width` cells.
fn gate_body(gate: &CardGate, width: usize) -> Vec<GLine> {
    let gold = Style::default().fg(th().gold);
    let green = Style::default().fg(th().ok);
    let plain = Style::default();
    let mut lines = Vec::new();
    let text = |lines: &mut Vec<GLine>, words: String, style: Style| {
        for line in wrap_spans(&[(words, style)], width) {
            lines.push(GLine::Text(line));
        }
    };
    let title = |words: String| (words, gold.add_modifier(Modifier::BOLD));
    match &gate.kind {
        GateKind::Removals { card, server, n, first } => {
            let words = t!("dev.card.gate_remove_title", n = fmt_count(*n as u64), card = card.clone());
            let (t, s) = title(words.to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_remove_why", server = server.clone()).to_string(), plain);
            for path in first {
                lines.push(GLine::Text(Line::from(fit(&format!("  {path}"), width))));
            }
            if *n > first.len() {
                let more = t!("dev.card.gate_remove_more", n = fmt_count((n - first.len()) as u64));
                lines.push(GLine::Text(Line::from(Span::styled(format!("  {more}"), dim()))));
            }
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_remove_stays", server = server.clone()).to_string(), green);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_remove_never").to_string(), plain);
        }
        GateKind::Half { card, server, n, total, libs, ticked } => {
            let (n, total) = (fmt_count(*n as u64), fmt_count(*total as u64));
            let words = t!("dev.card.gate_half_title", n = n, card = card.clone(), total = total);
            let (t, s) = title(words.to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_half_why", server = server.clone()).to_string(), plain);
            if !libs.is_empty() {
                lines.push(GLine::Blank);
                for (lib, gone, of) in libs {
                    let (n, total) = (fmt_count(*gone as u64), fmt_count(*of as u64));
                    let words = t!("dev.card.gate_half_lib", n = n, total = total);
                    lines.push(GLine::Text(Line::from(fit(&format!("  {lib}  {words}"), width))));
                }
            }
            lines.push(GLine::Blank);
            let mark = if *ticked { glyphs().checked } else { "[ ]" };
            let words = t!("dev.card.gate_half_box", server = server.clone());
            lines.push(GLine::Box(Line::from(format!("{mark} {words}"))));
            text(&mut lines, t!("dev.card.gate_half_stays").to_string(), plain);
        }
        GateKind::Adopt { drive, server, n, all } => {
            let words = t!("dev.card.gate_adopt_title", n = fmt_count(*n as u64), server = server.clone());
            let (t, s) = title(words.to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            let mb = (*n as u64 * 8 * 1024).div_ceil(1_000_000);
            let (every, some, size) = (fmt_count(*all as u64), fmt_count(*n as u64), fmt_count(mb));
            let words = t!(
                "dev.card.gate_adopt_body",
                drive = drive.clone(),
                all = every,
                n = some,
                server = server.clone(),
                mb = size
            );
            text(&mut lines, words.to_string(), plain);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_adopt_then").to_string(), plain);
            if all > n {
                text(&mut lines, t!("dev.card.gate_adopt_rest", n = fmt_count((all - n) as u64)).to_string(), green);
            }
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_adopt_nothing").to_string(), plain);
        }
        GateKind::Relay { server, bytes } => {
            let (t, s) = title(t!("dev.card.gate_relay_title", gb = gb(*bytes)).to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            // About 1.5 MB/s over the relay, 20 MB/s on the same network.
            let (slow, fast) = (span(bytes / 1_500_000), span(bytes / 20_000_000));
            let words = t!("dev.card.gate_relay_body", server = server.clone(), t = slow, t2 = fast);
            text(&mut lines, words.to_string(), plain);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_relay_ok").to_string(), green);
        }
        GateKind::Proxy { peer, server, .. } => {
            let words = t!("dev.card.gate_proxy_title", peer = peer.clone(), server = server.clone());
            let (t, s) = title(words.to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            let words = t!("dev.card.gate_proxy_body", peer = peer.clone(), server = server.clone());
            text(&mut lines, words.to_string(), plain);
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_proxy_once").to_string(), plain);
        }
        GateKind::Server { card, old, new, rows, plays, .. } => {
            let (t, s) = title(t!("dev.card.gate_server_title", new = new.clone()).to_string());
            text(&mut lines, t, s);
            lines.push(GLine::Blank);
            let words = t!("dev.card.gate_server_body", card = card.clone(), old = old.clone(), new = new.clone());
            text(&mut lines, words.to_string(), plain);
            let drop = t!("dev.card.gate_server_drop", n = fmt_count(*rows as u64), old = old.clone());
            lines.push(GLine::Text(Line::from(fit(&format!("  {drop}"), width))));
            if *plays > 0 {
                lines.push(GLine::Blank);
                let words = t!("dev.card.gate_server_plays", n = fmt_count(*plays as u64), old = old.clone());
                text(&mut lines, words.to_string(), plain);
            }
            lines.push(GLine::Blank);
            text(&mut lines, t!("dev.card.gate_server_old", old = old.clone()).to_string(), plain);
        }
    }
    lines.push(GLine::Blank);
    lines
}

/// The gate's buttons: the keep, named for what stays, then the go; the
/// stronger gate's go only once its box is ticked.
pub(super) fn gate_buttons_of(gate: &CardGate) -> (String, Option<String>) {
    let back = glyphs().back;
    let (keep, go) = match &gate.kind {
        GateKind::Removals { n, .. } => {
            let go = t!("dev.card.gate_remove_do", n = fmt_count(*n as u64)).to_string();
            (t!("dev.card.gate_not_now").to_string(), Some(go))
        }
        GateKind::Half { n, ticked, .. } => (
            t!("dev.card.gate_not_now").to_string(),
            ticked.then(|| t!("dev.card.gate_remove_do", n = fmt_count(*n as u64)).to_string()),
        ),
        GateKind::Adopt { n, .. } => (
            t!("dev.card.gate_adopt_keep").to_string(),
            Some(t!("dev.card.gate_adopt_do", n = fmt_count(*n as u64)).to_string()),
        ),
        GateKind::Relay { .. } => (
            t!("dev.card.gate_not_now").to_string(),
            Some(t!("dev.card.gate_relay_do").to_string()),
        ),
        GateKind::Proxy { peer, server, .. } => (
            t!("dev.card.gate_proxy_keep", peer = peer.clone()).to_string(),
            Some(t!("dev.card.gate_proxy_do", server = server.clone()).to_string()),
        ),
        GateKind::Server { old, new, .. } => (
            t!("dev.card.gate_server_keep_btn", old = old.clone()).to_string(),
            Some(t!("dev.card.gate_server_do", new = new.clone()).to_string()),
        ),
    };
    (format!("{back} {keep}"), go)
}

/// The gate's footer (clause 58): the go's key first, the keep's after;
/// the stronger gate says how to tick its box until it is ticked.
fn gate_hint(gate: &CardGate) -> Vec<(String, u8)> {
    let mut parts = Vec::new();
    let keep = match &gate.kind {
        GateKind::Adopt { .. } => t!("dev.card.hint_gate_adopt_keep").to_string(),
        GateKind::Server { old, .. } => t!("dev.card.hint_gate_server_keep", old = old.clone()).to_string(),
        GateKind::Proxy { .. } => t!("dev.hint_gate_cancel").to_string(),
        _ => t!("dev.card.hint_gate_keep").to_string(),
    };
    match &gate.kind {
        GateKind::Half { ticked: false, .. } => parts.push((t!("dev.card.hint_gate_box").to_string(), 0)),
        GateKind::Half { .. } | GateKind::Removals { .. } => {
            parts.push((t!("dev.card.hint_gate_remove").to_string(), 0));
        }
        GateKind::Adopt { .. } => parts.push((t!("dev.card.hint_gate_adopt").to_string(), 0)),
        GateKind::Relay { .. } => parts.push((t!("dev.card.hint_gate_relay").to_string(), 0)),
        GateKind::Proxy { server, .. } => {
            parts.push((format!("y {}", t!("dev.card.gate_proxy_do", server = server.clone()).to_lowercase()), 0));
        }
        GateKind::Server { new, .. } => parts.push((t!("dev.card.hint_gate_server", new = new.clone()).to_string(), 0)),
    }
    parts.push((keep, 0));
    parts
}

/// The kit's gold modal over the card (clause 58's grammar: clause 23's).
pub(super) fn draw_card_gate(frame: &mut Frame, page: &mut Page, area: Rect) {
    let Some(gate) = page.card.gate.clone() else { return };
    let width = GATE_W.min(area.width.saturating_sub(4));
    if width < 12 || area.height < 5 {
        return;
    }
    let text_w = usize::from(width.saturating_sub(4));
    let mut lines = gate_body(&gate, text_w);
    let most = usize::from(area.height.saturating_sub(2)).saturating_sub(3);
    while lines.len() > most {
        let Some(at) = lines.iter().rposition(|l| matches!(l, GLine::Blank)) else { break };
        lines.remove(at);
    }
    let height = (lines.len() + 3) as u16;
    let inner = kit::modal_frame_on(frame, &mut page.ui, area, width, height, th().gold);
    if inner.height == 0 {
        return;
    }
    let body = Rect {
        x: inner.x + 1,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(1),
    };
    for (i, line) in lines.iter().enumerate() {
        let y = body.y + i as u16;
        if y >= body.bottom() {
            break;
        }
        let at = Rect { y, height: 1, ..body };
        match line {
            GLine::Blank => {}
            GLine::Text(line) => frame.render_widget(Paragraph::new(line.clone()), at),
            GLine::Box(line) => {
                let rect = Rect { width: (line.width() as u16).min(at.width), ..at };
                let style = if page.ui.hovers(rect) { Style::default().fg(th().bright) } else { Style::default() };
                frame.render_widget(Paragraph::new(line.clone().style(style)), rect);
                page.ui.click(rect, Act::CardBox);
            }
        }
    }
    let y = inner.bottom().saturating_sub(1);
    let (keep, go) = gate_buttons_of(&gate);
    let keep_w = kit::width(&keep) as u16 + 4;
    let go_w = go.as_ref().map_or(0, |g| kit::width(g) as u16 + 4 + 2);
    let keep_x = inner.right().saturating_sub(go_w + keep_w).max(inner.x);
    let at = Rect { x: keep_x, y, width: inner.right() - keep_x, height: 1 };
    let keep_rect = kit::button(frame, &mut page.ui, at, &keep, true, Act::CardKeep);
    if let Some(go) = go {
        let go_x = keep_rect.right() + 2;
        if go_x < inner.right() {
            let at = Rect { x: go_x, y, width: inner.right() - go_x, height: 1 };
            kit::button(frame, &mut page.ui, at, &go, false, Act::CardGo);
        }
    }
}

// ── The card's Settings sheet (clauses 48–50) ───────────────────────────────

/// The sheet's two tabs (the pick of 2026-10-19: card 05's alternate A).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SheetTab {
    Music,
    Writing,
}

/// A row of the sheet, in Tab's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SRow {
    Name,
    From,
    Library(String),
    PeerLibrary(u32, String),
    Convert,
    Into,
    Removals,
    Covers,
    AutoDj,
    For,
    Player,
    Budget,
    Buttons,
}

/// The sheet while it is up: for one card, its choice, where the keyboard is.
#[derive(Clone, Debug)]
pub(crate) struct CardSheet {
    pub(crate) key: String,
    /// Set up (a new card) rather than its settings.
    pub(crate) setup: bool,
    pub(crate) tab: SheetTab,
    pub(crate) want: Want,
    /// What it opened on: Apply with nothing changed writes nothing.
    pub(crate) was: Want,
    pub(crate) focus: usize,
    /// The Name field while it has the keyboard.
    pub(crate) field: Option<Input>,
    /// A dropdown hanging open (From, Into), its cursor.
    pub(crate) list: Option<usize>,
    /// The buttons' cursor: Apply, else the other.
    pub(crate) on_apply: bool,
    /// The fit asked for, by its request's number.
    pub(crate) seq: u64,
    /// The libraries the server offers, and their sizes where known.
    pub(crate) offered: Vec<(String, Option<(usize, u64)>)>,
    pub(crate) peers: Vec<(u32, String, Vec<String>)>,
}

impl CardSheet {
    /// The rows of the tab in view, in order.
    pub(crate) fn rows(&self) -> Vec<SRow> {
        let mut rows = Vec::new();
        match self.tab {
            SheetTab::Music => {
                rows.push(SRow::Name);
                rows.push(SRow::From);
                rows.extend(self.offered.iter().map(|(lib, _)| SRow::Library(lib.clone())));
                for (id, _, libs) in &self.peers {
                    rows.extend(libs.iter().map(|lib| SRow::PeerLibrary(*id, lib.clone())));
                }
            }
            SheetTab::Writing => {
                rows.extend([SRow::Convert, SRow::Into, SRow::Removals, SRow::Covers]);
                rows.extend([SRow::AutoDj, SRow::For, SRow::Player]);
                if !self.peers.is_empty() {
                    rows.push(SRow::Budget);
                }
            }
        }
        rows.push(SRow::Buttons);
        rows
    }

    fn row(&self) -> SRow {
        let rows = self.rows();
        rows.get(self.focus.min(rows.len().saturating_sub(1))).cloned().unwrap_or(SRow::Buttons)
    }

    fn libraries(&self) -> Vec<String> {
        self.want.libraries.clone().unwrap_or_default()
    }
}

impl Page {
    /// `o`, *Settings…*, *Set up ▸*, *Choose what fits ▸*: the sheet for the
    /// card in view (clause 48), on Music.
    fn open_card_sheet(&mut self, card: &CardView, setup: bool) {
        let Some(facts) = card.facts() else { return };
        let mut want = Want::of(facts, card.server.link());
        if setup && want.name.is_empty() {
            want.name = card.found.volume.label.trim().to_string();
        }
        let plan = plan_of(card);
        let mut offered: Vec<(String, Option<(usize, u64)>)> = Vec::new();
        let libs = plan.map(|p| p.offered.clone()).filter(|o| !o.is_empty()).unwrap_or_else(|| facts.libraries());
        for lib in libs {
            let size = plan.and_then(|p| p.totals.iter().find(|(l, ..)| *l == lib)).map(|(_, n, b)| (*n, *b));
            offered.push((lib, size));
        }
        if want.libraries.is_none() {
            want.libraries = Some(offered.iter().map(|(l, _)| l.clone()).collect());
        }
        let peers = facts
            .state
            .as_ref()
            .map(|s| s.settings.peers.iter().map(|p| (p.peer_id, p.nickname.clone(), p.libraries.clone())).collect())
            .unwrap_or_default();
        self.card.seq += 1;
        let sheet = CardSheet {
            key: card.key.clone(),
            setup,
            tab: SheetTab::Music,
            was: want.clone(),
            want,
            focus: 0,
            field: None,
            list: None,
            on_apply: true,
            seq: self.card.seq,
            offered,
            peers,
        };
        self.card.sheet = Some(sheet);
    }

    /// The sheet's choice changed: its fit is asked for again (clause 49).
    fn sheet_changed(&mut self) {
        let Some(sheet) = self.card.sheet.as_mut() else { return };
        if sheet.setup {
            return;
        }
        self.card.seq += 1;
        sheet.seq = self.card.seq;
        let cmd = worker::Cmd::Fit { key: sheet.key.clone(), want: Box::new(sheet.want.clone()), seq: sheet.seq };
        self.send_card(cmd);
    }

    fn card_sheet_act(&mut self, act: Act) {
        match act {
            Act::CardSheetClose => self.card.sheet = None,
            Act::CardSheetTab(i) => {
                if let Some(sheet) = self.card.sheet.as_mut() {
                    sheet.tab = if i == 0 { SheetTab::Music } else { SheetTab::Writing };
                    sheet.focus = 0;
                    sheet.list = None;
                    sheet.field = None;
                }
            }
            Act::CardSheetRow(i) => {
                if let Some(sheet) = self.card.sheet.as_mut() {
                    sheet.focus = i;
                }
                self.sheet_toggle(1);
            }
            Act::CardSheetPick(i) => self.sheet_pick(i),
            Act::CardDefaults => {
                if let Some(sheet) = self.card.sheet.as_mut() {
                    if sheet.setup {
                        self.card.sheet = None;
                        return;
                    }
                    let keep = (sheet.want.name.clone(), sheet.want.server.clone(), sheet.want.libraries.clone());
                    let defaults = crate::device::card::state::Settings::defaults();
                    let c = defaults.conversion;
                    sheet.want = Want {
                        name: keep.0,
                        server: keep.1,
                        libraries: keep.2,
                        peers: sheet.want.peers.clone(),
                        convert: c.convert,
                        codec: c.codec,
                        bitrate: c.bitrate,
                        removal: defaults.removal,
                        covers: defaults.covers,
                        auto_dj: defaults.auto_dj,
                        for_someone_else: defaults.for_someone_else,
                        budget_mb: None,
                    };
                }
                self.sheet_changed();
            }
            Act::CardApply => self.sheet_apply_card(),
            _ => {}
        }
    }

    /// Apply, or Set up: the card's settings written, nothing synced; the
    /// another-server gate first where From names another (clause 58).
    fn sheet_apply_card(&mut self) {
        let Some(mut sheet) = self.card.sheet.take() else { return };
        if let Some(field) = sheet.field.take() {
            sheet.want.name = field.value().trim().to_string();
        }
        let key = sheet.key.clone();
        if sheet.setup {
            self.send_card(worker::Cmd::SetUp { key, want: Box::new(sheet.want) });
            return;
        }
        let card = self.open_card();
        let current = card.as_ref().and_then(|c| c.server.link().map(|l| l.id.clone()));
        let filled = card.as_ref().and_then(|c| c.facts()).is_some_and(|f| f.tracks > 0);
        if filled && sheet.want.server.is_some() && sheet.want.server != current {
            let card = card.expect("the card");
            let servers = self.servers();
            let new = servers.iter().find(|s| Some(&s.id) == sheet.want.server.as_ref()).map(|s| s.name.clone());
            // The gate first, nothing written (clause 58): its yes writes the
            // settings, and the next sync takes the card over; its keep
            // writes nothing at all.
            let kind = GateKind::Server {
                card: card_name(&card),
                old: server_name(&card),
                new: new.unwrap_or_default(),
                rows: card.facts().map_or(0, |f| f.tracks),
                plays: card.facts().and_then(|f| f.state.as_ref()).map_or(0, |s| s.plays.len()),
                want: Some(Box::new(sheet.want)),
            };
            self.card.gate = Some(CardGate { key, kind, allow: Allow::default() });
            return;
        }
        if sheet.want != sheet.was {
            self.send_card(worker::Cmd::Apply { key, want: Box::new(sheet.want) });
        }
    }

    /// The sheet's keys (clause 48, the Advanced sheet's: clause 34): Tab
    /// ⇧Tab and ↑ ↓ walk the rows, Space ticks or opens, ← → move between
    /// the tabs (or the buttons, or a radio's choices), Enter applies, Esc
    /// closes and changes nothing. The Name field takes every key while it
    /// has the keyboard.
    pub(super) fn card_sheet_key(&mut self, key: KeyEvent) {
        let servers = self.servers();
        let Some(sheet) = self.card.sheet.as_mut() else { return };
        if let Some(field) = sheet.field.as_mut() {
            match key.code {
                KeyCode::Enter | KeyCode::Tab | KeyCode::Esc => {
                    let value = field.value().trim().to_string();
                    if key.code != KeyCode::Esc {
                        sheet.want.name = value;
                    }
                    sheet.field = None;
                    if key.code == KeyCode::Tab {
                        sheet.focus += 1;
                    }
                }
                _ => {
                    field.handle_event(&ratatui::crossterm::event::Event::Key(key));
                }
            }
            return;
        }
        if let Some(cursor) = sheet.list {
            let count = list_entries(sheet, &servers).len();
            match key.code {
                KeyCode::Esc => sheet.list = None,
                KeyCode::Up => sheet.list = Some(cursor.saturating_sub(1)),
                KeyCode::Down => sheet.list = Some((cursor + 1).min(count.saturating_sub(1))),
                KeyCode::Enter | KeyCode::Char(' ') => self.sheet_pick(cursor),
                _ => {}
            }
            return;
        }
        let rows = sheet.rows();
        let n = rows.len();
        match key.code {
            KeyCode::Esc => self.card.sheet = None,
            KeyCode::Tab | KeyCode::Down => sheet.focus = (sheet.focus + 1).min(n - 1),
            KeyCode::BackTab | KeyCode::Up => sheet.focus = sheet.focus.saturating_sub(1),
            KeyCode::Left | KeyCode::Right => {
                let step = if key.code == KeyCode::Left { -1 } else { 1 };
                match sheet.row() {
                    SRow::Buttons => sheet.on_apply = step > 0,
                    SRow::Convert | SRow::Removals | SRow::For | SRow::Budget => self.sheet_toggle(step),
                    _ if !sheet.setup => {
                        sheet.tab = if step < 0 { SheetTab::Music } else { SheetTab::Writing };
                        sheet.focus = 0;
                    }
                    _ => {}
                }
            }
            KeyCode::Char(' ') => self.sheet_toggle(1),
            KeyCode::Enter => match sheet.row() {
                SRow::Name | SRow::From | SRow::Into => self.sheet_toggle(1),
                SRow::Buttons if !sheet.on_apply => self.card_sheet_act(Act::CardDefaults),
                _ => self.sheet_apply_card(),
            },
            _ => {}
        }
    }

    /// Space, or a click: the focused row's choice changes.
    fn sheet_toggle(&mut self, step: isize) {
        let Some(sheet) = self.card.sheet.as_mut() else { return };
        let mut changed = true;
        match sheet.row() {
            SRow::Name => {
                let value = sheet.want.name.clone();
                let cursor = value.chars().count();
                sheet.field = Some(Input::new(value).with_cursor(cursor));
                changed = false;
            }
            SRow::From | SRow::Into => {
                sheet.list = Some(0);
                changed = false;
            }
            SRow::Library(lib) => {
                let mut libs = sheet.libraries();
                match libs.iter().position(|l| *l == lib) {
                    Some(at) => {
                        libs.remove(at);
                    }
                    None => libs.push(lib),
                }
                sheet.want.libraries = Some(libs);
            }
            SRow::PeerLibrary(id, lib) => {
                if let Some((_, libs)) = sheet.want.peers.iter_mut().find(|(p, _)| *p == id) {
                    match libs.iter().position(|l| *l == lib) {
                        Some(at) => {
                            libs.remove(at);
                        }
                        None => libs.push(lib),
                    }
                }
            }
            SRow::Convert => {
                sheet.want.convert = match sheet.want.convert {
                    Convert::Unplayable => Convert::Everything,
                    Convert::Everything => Convert::Unplayable,
                };
            }
            SRow::Removals => {
                sheet.want.removal = match sheet.want.removal {
                    Removal::Mirror => Removal::Never,
                    Removal::Never => Removal::Mirror,
                };
            }
            SRow::Covers => sheet.want.covers = !sheet.want.covers,
            SRow::AutoDj => sheet.want.auto_dj = !sheet.want.auto_dj,
            SRow::For => sheet.want.for_someone_else = !sheet.want.for_someone_else,
            SRow::Budget => {
                let mb = sheet.want.budget_mb.unwrap_or(crate::device::card::budget::DEFAULT_MB) as i64;
                let mb = (mb + step as i64 * 250).clamp(250, 3_000) as u64;
                sheet.want.budget_mb = Some(mb);
            }
            SRow::Player => changed = false,
            SRow::Buttons => {
                changed = false;
                if sheet.on_apply {
                    self.sheet_apply_card();
                } else {
                    self.card_sheet_act(Act::CardDefaults);
                }
            }
        }
        if changed {
            self.sheet_changed();
        }
    }

    /// A row of the hanging list picked: a server for From, a codec for
    /// Into.
    fn sheet_pick(&mut self, i: usize) {
        let servers = self.servers();
        let Some(sheet) = self.card.sheet.as_mut() else { return };
        match sheet.row() {
            SRow::From => {
                if let Some(link) = servers.get(i) {
                    sheet.want.server = Some(link.id.clone());
                }
            }
            SRow::Into => {
                if let Some((codec, bitrate)) = CODECS.get(i) {
                    (sheet.want.codec, sheet.want.bitrate) = (*codec, *bitrate);
                }
            }
            _ => {}
        }
        sheet.list = None;
        self.sheet_changed();
    }
}

/// The hanging list's rows for the focused row.
fn list_entries(sheet: &CardSheet, servers: &[ServerLink]) -> Vec<String> {
    match sheet.row() {
        SRow::From => servers.iter().map(|s| s.name.clone()).collect(),
        SRow::Into => CODECS.iter().map(|(c, b)| codec_words(*c, *b)).collect(),
        _ => Vec::new(),
    }
}

/// The sheet's footer (clause 48).
fn sheet_hint(sheet: &CardSheet) -> Vec<(String, u8)> {
    if sheet.list.is_some() {
        return vec![
            (format!("{} {}", glyphs().updown, t!("dev.hint_sheet_choose")), 0),
            (t!("dev.hint_list_pick").to_string(), 0),
            (t!("dev.hint_list_close").to_string(), 0),
        ];
    }
    let mut parts = vec![(t!("dev.card.hint_sheet_row").to_string(), 0)];
    if !sheet.setup {
        parts.push((format!("{} {}", glyphs().updown, t!("dev.hint_sheet_choose")), 2));
        parts.push((t!("dev.hint_sheet_tick").to_string(), 2));
        parts.push((format!("{} {}", glyphs().switch, t!("dev.card.hint_sheet_tabs")), 1));
        parts.push((t!("dev.hint_sheet_apply").to_string(), 0));
    } else {
        parts.push((t!("dev.card.hint_enter_setup").to_string(), 0));
    }
    parts.push((t!("dev.hint_sheet_close").to_string(), 0));
    parts
}

/// The fit line's words (clause 49) from a plan's fit and the card's size.
fn fit_line(fit: &Fit, size: u64) -> (Vec<(String, Style)>, Option<String>) {
    let g = glyphs();
    let gold = Style::default().fg(th().gold);
    let used = size.saturating_sub(fit.room).saturating_add(fit.needed);
    if fit.over_cap() {
        let words = t!("dev.card.fit_cap", n = fmt_count(fit.tracks as u64), max = fmt_count(TRACK_CAP as u64));
        return (vec![(format!("{} ", g.no), gold), (words.to_string(), gold)], None);
    }
    if !fit.fits() {
        let head = t!("dev.card.fit_no", gb = gb(fit.short()));
        let over = t!("dev.card.fit_over", used = gb(used), size = gb(size));
        return (vec![(format!("{} ", g.no), gold), (format!("{head} · {over}"), gold)], None);
    }
    let spare = fit.room.saturating_sub(fit.needed);
    let after = t!("dev.card.fit_after", used = gb(used), size = gb(size), spare = gb(spare));
    if size > 0 && spare * 20 < size {
        let pieces = vec![(format!("{} ", g.warn), gold), (format!("{} · {after}", t!("dev.card.fit_tight")), gold)];
        return (pieces, Some(t!("dev.card.fit_tight_2").to_string()));
    }
    let ok = Style::default().fg(th().ok);
    (vec![(format!("{} ", g.ok), ok), (format!("{} · {after}", t!("dev.card.fit_ok")), Style::default())], None)
}

/// The help line's words, about the focused row (clause 48).
fn sheet_help(sheet: &CardSheet, server: &str) -> String {
    row_help(&sheet.row(), &sheet.offered, server)
}

/// The help about `row`, its library's size from what the server `offered`.
pub(super) fn row_help(row: &SRow, offered: &[(String, Option<(usize, u64)>)], server: &str) -> String {
    match row {
        SRow::Name => t!("dev.card.help_name").to_string(),
        SRow::Removals => t!("dev.card.help_removals", server = server).to_string(),
        SRow::Into => t!("dev.card.help_into", server = server).to_string(),
        SRow::Player => t!("dev.card.help_player").to_string(),
        SRow::Library(lib) => {
            let size = offered.iter().find(|(l, _)| l == lib).and_then(|(_, s)| *s);
            match size {
                Some((n, _)) => {
                    t!("dev.card.help_lib_short", lib = lib, server = server, n = fmt_count(n as u64)).to_string()
                }
                None => t!("dev.card.help_lib_unknown", lib = lib, server = server).to_string(),
            }
        }
        SRow::Budget => t!("dev.card.help_budget").to_string(),
        SRow::Convert | SRow::Covers | SRow::AutoDj | SRow::For => t!("dev.card.help_writing").to_string(),
        _ => t!("dev.card.help_from").to_string(),
    }
}

/// The help in two lines of `tw` cells, an ellipsis past them (clause 48).
pub(super) fn help_lines(text: &str, tw: usize) -> Vec<Line<'static>> {
    let help = wrap_spans(&[(text.to_string(), dim())], tw);
    let more = help.len() > 2;
    let cut = |line: Line<'static>| Line::from(Span::styled(fit(&format!("{line} …"), tw), dim()));
    help.into_iter().take(2).enumerate().map(|(i, line)| if more && i == 1 { cut(line) } else { line }).collect()
}

/// The sheet's width in an area `area_w` cells wide (the kit's modal: 74,
/// the area's less four), and its text's inside its border and padding.
pub(super) fn sheet_widths(area_w: u16) -> (u16, u16) {
    let width = GATE_W.min(area_w.saturating_sub(4));
    (width, width.saturating_sub(4))
}

/// The sheet's rows' height, its tallest state: its top is held there.
const CARD_SHEET_MAX: u16 = 22;

/// The sheet over the card (clause 48): the kit's neutral modal, two tabs,
/// the fit line under both, the help, the buttons.
pub(super) fn draw_card_sheet(frame: &mut Frame, page: &mut Page, area: Rect) {
    let Some(sheet) = page.card.sheet.clone() else { return };
    let Some(card) = page.open_card() else { return };
    let (width, text_w) = sheet_widths(area.width);
    if width < 24 || area.height < 8 {
        return;
    }
    let servers = page.servers();
    let server = sheet
        .want
        .server
        .as_ref()
        .and_then(|id| servers.iter().find(|s| &s.id == id))
        .map(|s| s.name.clone())
        .unwrap_or_else(|| server_name(&card));
    let tw = usize::from(text_w);
    let g = glyphs();
    // The lines: (text pieces, its row's act, whether it is the keyboard's).
    enum L {
        Title,
        Blank,
        Tabs,
        Row(usize, Line<'static>),
        Plain(Line<'static>),
        Rule,
        Help(Line<'static>),
        Buttons,
    }
    let mut lines: Vec<L> = vec![L::Title, L::Blank];
    if !sheet.setup {
        lines.push(L::Tabs);
        lines.push(L::Blank);
    }
    let focus = sheet.focus.min(sheet.rows().len() - 1);
    let label = |key: &str| format!("{:<14}", fit(&t!(key), 13));
    let on = |b: bool| if b { g.radio_on } else { "( )" };
    let tick = |b: bool| if b { g.checked } else { "[ ]" };
    let mut peer_head: Option<u32> = None;
    for (i, row) in sheet.rows().iter().enumerate() {
        let lit = i == focus && sheet.field.is_none() && sheet.list.is_none();
        let style = if lit { accent().add_modifier(Modifier::BOLD) } else { Style::default() };
        let line: Line<'static> = match row {
            SRow::Name => {
                let value = if sheet.want.name.is_empty() { String::new() } else { sheet.want.name.clone() };
                Line::from(vec![Span::styled(label("dev.card.row_name"), dim()), Span::styled(value, style)])
            }
            SRow::From => {
                let picked = sheet.want.server.as_ref().and_then(|id| servers.iter().find(|s| &s.id == id));
                let name = picked.map_or_else(|| "—".to_string(), |s| s.name.clone());
                let value = Span::styled(format!("{name} {}", g.open), style);
                Line::from(vec![Span::styled(label("dev.card.grp_from"), dim()), value])
            }
            SRow::Library(lib) => {
                let ticked = sheet.libraries().contains(lib);
                let size = sheet.offered.iter().find(|(l, _)| l == lib).and_then(|(_, s)| *s);
                let left = format!("  {} {lib}", tick(ticked));
                let right = size
                    .map(|(n, b)| {
                        let tracks = t!("dev.card_tracks", n = fmt_count(n as u64));
                        format!("{} · {tracks}", size_words(b))
                    })
                    .unwrap_or_default();
                let pad = tw.saturating_sub(kit::width(&left) + kit::width(&right));
                Line::from(vec![Span::styled(left, style), Span::raw(" ".repeat(pad)), Span::styled(right, dim())])
            }
            SRow::PeerLibrary(id, lib) => {
                if peer_head != Some(*id) {
                    peer_head = Some(*id);
                    let peer = sheet.peers.iter().find(|(p, ..)| p == id);
                    let name = peer.map(|(_, n, _)| n.clone()).unwrap_or_default();
                    let mb = fmt_count(sheet.want.budget_mb.unwrap_or(crate::device::card::budget::DEFAULT_MB));
                    let head = t!("dev.card.grp_peer", peer = name, server = server.clone(), mb = mb).to_string();
                    lines.push(L::Plain(Line::from(Span::styled(fit(&head, tw), dim()))));
                }
                let ticked = sheet.want.peers.iter().any(|(p, libs)| p == id && libs.contains(lib));
                Line::from(Span::styled(format!("  {} {lib}", tick(ticked)), style))
            }
            SRow::Convert => {
                let needed = sheet.want.convert == Convert::Unplayable;
                let (one, all) = (t!("dev.card.opt_needed"), t!("dev.card.opt_all"));
                let words = format!("{} {one}   {} {all}", on(needed), on(!needed));
                Line::from(vec![Span::styled(label("dev.card.row_convert"), dim()), Span::styled(words, style)])
            }
            SRow::Into => {
                let words = format!("{} {}", codec_words(sheet.want.codec, sheet.want.bitrate), g.open);
                Line::from(vec![Span::styled(label("dev.card.row_into"), dim()), Span::styled(words, style)])
            }
            SRow::Removals => {
                let follow = sheet.want.removal == Removal::Mirror;
                let (mirror, never) = (t!("dev.card.opt_follow", server = server.clone()), t!("dev.card.opt_never"));
                let words = format!("{} {mirror}   {} {never}", on(follow), on(!follow));
                Line::from(vec![Span::styled(label("dev.card.row_removals"), dim()), Span::styled(words, style)])
            }
            SRow::Covers => {
                let words = format!("{} {}", tick(sheet.want.covers), t!("dev.card.opt_covers"));
                Line::from(vec![Span::styled(label("dev.card.row_covers"), dim()), Span::styled(words, style)])
            }
            SRow::AutoDj => {
                let words = format!("{} {}", tick(sheet.want.auto_dj), t!("dev.card.opt_index"));
                Line::from(vec![Span::styled(label("dev.card.row_autodj"), dim()), Span::styled(words, style)])
            }
            SRow::For => {
                let me = !sheet.want.for_someone_else;
                let words = format!("{} {}   {} {}", on(me), t!("dev.card.opt_me"), on(!me), t!("dev.card.opt_else"));
                Line::from(vec![Span::styled(label("dev.card.row_for"), dim()), Span::styled(words, style)])
            }
            SRow::Player => {
                let d = card.facts().map(|f| f.device.clone());
                let words = match d.filter(|d| d.present) {
                    Some(d) => t!("dev.card.player_txt", name = "M5Stack Core2", version = d.firmware).to_string(),
                    None => t!("dev.card.player_none").to_string(),
                };
                Line::from(vec![Span::styled(label("dev.card.row_player"), dim()), Span::raw(words)])
            }
            SRow::Budget => {
                let mb = sheet.want.budget_mb.unwrap_or(crate::device::card::budget::DEFAULT_MB);
                let filled = (mb / 300).min(10) as usize;
                let bar = format!("{}{}", g.filled.repeat(filled), g.empty.repeat(10 - filled));
                let words = format!("- {bar} +  {}", t!("dev.card.budget_mb", mb = fmt_count(mb)));
                Line::from(vec![Span::styled(label("dev.card.row_budget"), dim()), Span::styled(words, style)])
            }
            SRow::Buttons => continue,
        };
        lines.push(L::Row(i, line));
    }
    lines.push(L::Blank);
    // The fit line: the sheet's own answer, else the card's last look.
    let fit_next = card.fit.as_ref().filter(|(seq, _)| *seq == sheet.seq).map(|(_, n)| n.clone());
    let size = card.found.volume.space.map_or(0, |s| s.size);
    match fit_next.as_ref().or(Some(&card.next)) {
        Some(Next::Asking) => {
            let words = t!("dev.card.sync_planning").to_string();
            let spans: Vec<Span<'static>> = scan(None, &words).into_iter().map(|(t, s)| Span::styled(t, s)).collect();
            lines.push(L::Plain(Line::from(spans)));
        }
        Some(Next::Plan(plan)) => {
            if let Some(fit) = plan.fit {
                let (pieces, more) = fit_line(&fit, size);
                for line in wrap_spans(&pieces, tw) {
                    lines.push(L::Plain(line));
                }
                if let Some(more) = more {
                    lines.push(L::Plain(Line::from(Span::styled(format!("  {more}"), dim()))));
                }
            }
        }
        _ => {}
    }
    lines.push(L::Rule);
    lines.extend(help_lines(&sheet_help(&sheet, &server), tw).into_iter().map(L::Help));
    lines.push(L::Buttons);
    // The frame takes at most two rows short of the area (kit::modal_rect), and
    // its own two borders.
    let most = usize::from(area.height.saturating_sub(4));
    if lines.len() > most {
        lines.retain(|l| !matches!(l, L::Rule | L::Help(_)));
    }
    while lines.len() > most {
        let Some(at) = lines.iter().rposition(|l| matches!(l, L::Blank)) else { break };
        lines.remove(at);
    }
    if lines.len() > most {
        let buttons = lines.pop();
        lines.truncate(most.saturating_sub(1));
        lines.extend(buttons);
    }
    let height = (lines.len() + 2) as u16;
    let inner = kit::modal_frame_anchored_on(frame, &mut page.ui, area, width, height, CARD_SHEET_MAX, th().accent);
    if inner.height == 0 || inner.width < 4 {
        return;
    }
    let content = Rect { x: inner.x + 1, y: inner.y, width: inner.width - 2, height: inner.height };
    let mut list_at: Option<Rect> = None;
    for (i, line) in lines.iter().enumerate() {
        let y = content.y + i as u16;
        if y >= content.bottom() {
            break;
        }
        let row = Rect { y, height: 1, ..content };
        match line {
            L::Title => {
                let title = if sheet.setup {
                    t!("dev.card.setup_title", drive = card.drive()).to_string()
                } else {
                    t!("dev.card.set_title", card = card_name(&card)).to_string()
                };
                let at = Rect { width: row.width.saturating_sub(4), ..row };
                let title = Span::styled(fit(&title, usize::from(at.width)), accent().add_modifier(Modifier::BOLD));
                frame.render_widget(Paragraph::new(title), at);
                kit::modal_close(frame, &mut page.ui, inner, Act::CardSheetClose);
            }
            L::Blank => {}
            L::Tabs => {
                let mut x = row.x + 1;
                let tabs = [("dev.card.tab_music", SheetTab::Music), ("dev.card.tab_writing", SheetTab::Writing)];
                for (i, (key, tab)) in tabs.iter().enumerate() {
                    let words = format!(" {} ", t!(*key));
                    let w = kit::width(&words) as u16;
                    let rect = Rect { x, y, width: w.min(row.right().saturating_sub(x)), height: 1 };
                    let style = if sheet.tab == *tab {
                        Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD)
                    } else if page.ui.hovers(rect) {
                        hover()
                    } else {
                        dim()
                    };
                    frame.render_widget(Paragraph::new(Span::styled(words, style)), rect);
                    page.ui.click(rect, Act::CardSheetTab(i));
                    x += w + 2;
                }
                if let Some(space) = card.found.volume.space {
                    let right = format!("{} · {}", card.drive(), size_words(space.size));
                    let w = kit::width(&right) as u16;
                    if x + 2 + w <= row.right() {
                        let at = Rect { x: row.right() - w, y, width: w, height: 1 };
                        frame.render_widget(Paragraph::new(Span::styled(right, dim())), at);
                    }
                }
            }
            L::Row(i, line) => {
                if sheet.field.is_some() && sheet.rows().get(*i) == Some(&SRow::Name) {
                    let at = Rect { x: row.x + 14, width: row.width.saturating_sub(14), ..row };
                    frame.render_widget(Paragraph::new(Span::styled(label("dev.card.row_name"), dim())), row);
                    if let Some(field) = &sheet.field {
                        let (value, cursor) = (field.value(), field.cursor());
                        let shown = kit::field_display(&mut page.ui, at.x, at.y, value, cursor, at.width, None);
                        frame.render_widget(Paragraph::new(Span::styled(shown, accent())), at);
                    }
                } else {
                    let cut = Line::from(line.spans.clone());
                    frame.render_widget(Paragraph::new(cut), row);
                }
                page.ui.click(row, Act::CardSheetRow(*i));
                if sheet.list.is_some() && *i == focus {
                    list_at = Some(Rect { x: row.x + 14, y: row.y + 1, ..row });
                }
            }
            L::Plain(line) => frame.render_widget(Paragraph::new(line.clone()), row),
            L::Rule => {
                let rule = Span::styled(g.rule.repeat(usize::from(row.width)), dim());
                frame.render_widget(Paragraph::new(rule), row)
            }
            L::Help(line) => frame.render_widget(Paragraph::new(line.clone()), row),
            L::Buttons => {
                let (other, go) = if sheet.setup {
                    (t!("dev.card.btn_cancel").to_string(), t!("dev.card.btn_do_setup").to_string())
                } else {
                    (t!("dev.card.btn_defaults").to_string(), t!("dev.card.btn_apply").to_string())
                };
                let ow = kit::width(&other) as u16 + 4;
                let gw = kit::width(&go) as u16 + 4;
                let x = row.right().saturating_sub(ow + 2 + gw).max(row.x);
                let at = Rect { x, width: row.right() - x, ..row };
                let focused = sheet.row() == SRow::Buttons;
                let other_act = if sheet.setup { Act::CardSheetClose } else { Act::CardDefaults };
                let r = kit::button(frame, &mut page.ui, at, &other, focused && !sheet.on_apply, other_act);
                let gx = r.right() + 2;
                if gx < row.right() {
                    let at = Rect { x: gx, width: row.right() - gx, ..row };
                    kit::button(frame, &mut page.ui, at, &go, true, Act::CardApply);
                }
            }
        }
    }
    // The hanging list over the sheet's lower rows.
    if let (Some(cursor), Some(at)) = (sheet.list, list_at) {
        let entries = list_entries(&sheet, &servers);
        let w = entries.iter().map(|e| kit::width(e)).max().unwrap_or(4) as u16 + 6;
        let h = (entries.len() as u16 + 2).min(area.bottom().saturating_sub(at.y));
        if h > 2 {
            let width = w.min(area.right().saturating_sub(at.x));
            let rect = Rect { x: at.x.saturating_sub(1), y: at.y, width, height: h };
            kit::blank(frame, rect);
            let inner = kit::frame_at(frame, rect, th().accent);
            for (i, entry) in entries.iter().enumerate() {
                let y = inner.y + i as u16;
                if y >= inner.bottom() {
                    break;
                }
                let r = Rect { y, height: 1, ..inner };
                let lit = Style::default().fg(th().on_accent).bg(th().accent);
                let style = if i == cursor { lit } else { Style::default() };
                let words = fit(&format!(" {entry}"), usize::from(r.width));
                frame.render_widget(Paragraph::new(Span::styled(words, style)), r);
                page.ui.click(r, Act::CardSheetPick(i));
            }
        }
    }
}
