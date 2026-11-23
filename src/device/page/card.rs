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
//!
//! This file is the card's model: what the page keeps of it, its keys, its
//! hints and what its gates ask. cells/card.rs draws it in character cells.

use std::cell::RefCell;
use std::rc::Rc;

use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::*;
use crate::device::card::cards::Cards as Known;
use crate::device::card::engine::{Answers, Removal};
use crate::device::card::plan::Fit;
use crate::device::card::state::Convert;
use crate::device::card::volume::Refusal;
use crate::device::card::worker::{
    self, Allow, CardView, Ended, Fold, Gate as Asked, Job, Link, LogItem, Next, Note, Pause, Planned, Read,
    Server, ServerLink, Want,
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
    /// When a gate opened as the answer to the one before it, or last
    /// swallowed an answer: until the keys have been quiet for
    /// [`CHAIN_GUARD`] it takes none, so a held key (however long its
    /// repeats go on) or a double-click that answered the first doesn't
    /// answer this one too.
    pub(crate) chained: Option<Instant>,
}

/// The daily budget saved on this computer for the card's peers, which the
/// sheet's one Budget row stands for (Apply writes it to each): the first
/// one saved. None: none saved, the default stands.
fn saved_budget(link: Option<&ServerLink>, peers: &[u32]) -> Option<u64> {
    use crate::device::card::budget::{Budget, PeerKey};
    if peers.is_empty() {
        return None;
    }
    let parent_key = link?.url_key()?;
    let book = Budget::system().ok()?.read().ok()?;
    peers.iter().find_map(|&peer_id| book.peers.get(&PeerKey { parent_key, peer_id }).and_then(|a| a.daily_mb))
}

/// A chained gate's guard is still up at `now`.
fn held(since: Instant, now: Instant) -> bool {
    now.saturating_duration_since(since) < CHAIN_GUARD
}

/// How long a gate opened by another's answer waits before it takes one:
/// past a key's repeat delay and a double-click's second click.
pub(crate) const CHAIN_GUARD: Duration = Duration::from_millis(500);

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
pub(super) fn now() -> u64 {
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
pub(super) fn server_name(card: &CardView) -> String {
    let kept = card.facts().map(|f| f.nickname.clone()).filter(|n| !n.is_empty());
    let shown = card.server.link().map(|l| l.name.clone());
    kept.or(shown).unwrap_or_else(|| "?".to_string())
}

/// A peer's name, as the card keeps it, else its id.
pub(super) fn peer_name(card: &CardView, id: u32) -> String {
    let peers = card.facts().and_then(|f| f.state.as_ref()).map(|s| s.settings.peers.clone()).unwrap_or_default();
    let named = peers.iter().find(|p| p.peer_id == id).map(|p| p.nickname.clone()).filter(|n| !n.is_empty());
    named.unwrap_or_else(|| format!("#{id}"))
}

/// The plan the card's last look made, if its server answered.
pub(super) fn plan_of(card: &CardView) -> Option<&Planned> {
    match &card.next {
        Next::Plan(plan) => Some(plan),
        _ => None,
    }
}

/// What a sync of the card would do: something waits.
pub(super) fn waits(card: &CardView) -> bool {
    let pending = card.facts().is_some_and(|f| f.pending);
    let something = |p: &Planned| p.new > 0 || !p.removals.is_empty() || p.plays > 0 || p.waves_missing;
    plan_of(card).is_some_and(something) || pending
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
    pub(super) fn word(self) -> String {
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
    if matches!(card.next, Next::SignIn { .. }) || matches!(card.paused(), Some(Pause::SignIn)) {
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

    /// The card at `key`, in view or not: a sheet's and a gate's card are
    /// the one they were opened for, whichever the view moved to since (it
    /// jumps to a sync that starts, clause 55).
    pub(super) fn card_of(&self, key: &str) -> Option<CardView> {
        let transfer = self.transfer.as_ref()?;
        let fold = transfer.fold.borrow();
        fold.cards.iter().find(|c| c.key == key).cloned()
    }

    /// The servers saved on this computer, as the worker was last told.
    pub(super) fn servers(&self) -> Vec<ServerLink> {
        self.transfer.as_ref().map(|t| t.fold.borrow().servers.clone()).unwrap_or_default()
    }

    pub(super) fn send_card(&mut self, cmd: worker::Cmd) {
        let sent = self.transfer.as_ref().is_none_or(|t| t.link.send(cmd));
        if !sent {
            // The worker is gone: nothing asked of a card is done.
            self.say(t!("dev.card.worker_gone").to_string(), true, None);
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
        self.follow_keys(&transfer.fold.borrow());
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
        // A sheet or a gate goes with its card: taken out, or another card
        // at its key now (a card with no serial has a reader's path for
        // one). Left open, it would hold the keys undrawn, or answer for
        // the card put in after.
        let gone = |key: &str, opened: &Opened| !self.card_of(key).is_some_and(|c| opened.still(&c));
        let gate_gone = self.card.gate.as_ref().is_some_and(|g| gone(&g.key, &g.card));
        let sheet_gone = self.card.sheet.as_ref().is_some_and(|s| gone(&s.key, &s.card));
        if gate_gone {
            self.card.gate = None;
        }
        if sheet_gone {
            self.card.sheet = None;
        }
        // A card put in takes the view only from the empty card (clause 25,
        // the pick of 2026-10-19).
        if self.card_open.is_none() && self.boards.is_empty() {
            self.card_open = keys.first().cloned();
        }
    }

    /// A card given its key with the serial (the worker's
    /// [`Event::Rekeyed`](worker::Event::Rekeyed)): the view, the row, the
    /// sheet and the gate follow it, the serial it has now theirs.
    fn follow_keys(&mut self, fold: &Fold) {
        let now = |key: &mut String| *key = fold.key_now(key);
        let serial = |key: &str| fold.cards.iter().find(|c| c.key == key).map(Opened::of);
        if let Some(open) = self.card_open.as_mut() {
            now(open);
        }
        self.card.row.iter_mut().for_each(now);
        if let Some(gate) = self.card.gate.as_mut() {
            now(&mut gate.key);
            gate.card.follow(serial(&gate.key));
        }
        if let Some(sheet) = self.card.sheet.as_mut() {
            now(&mut sheet.key);
            sheet.card.follow(serial(&sheet.key));
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
                    if let Some(end) = first_sentence(&words) {
                        words.truncate(end);
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
            Note::PlaysGift => (t!("dev.card.plays_gift").to_string(), false, false),
            Note::NotSaved { card } => (t!("dev.card.not_saved", card = card).to_string(), true, true),
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
        self.card_act_at(act, Instant::now());
    }

    /// [`Self::card_act`] at `now`: a chained gate's guard is judged by it.
    pub(super) fn card_act_at(&mut self, act: Act, now: Instant) {
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
            // A card taken out is its own until it is back (clause 53): its
            // settings, its plays and its sync wait for it.
            Act::CardSettings if !locked => {
                let may = |c: &CardView| c.facts().is_some_and(|f| f.set_up()) && c.run.is_none() && !c.out;
                if let Some(card) = card.filter(may) {
                    self.open_card_sheet(&card, false);
                }
            }
            // The plays wait for no sync: during one they are its own to send.
            Act::CardPlays if !locked => {
                if card.is_some_and(|c| c.run.is_none() && !c.out) {
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
                if let Some(card) = card.filter(|c| !c.out) {
                    self.sync_asked(&card);
                }
            }
            Act::CardOtherServer if !locked => {
                if let Some(card) = card {
                    self.open_card_sheet(&card, false);
                }
            }
            // A gate just opened by the answer to the one before it: the
            // answer is swallowed, and the guard waits for the keys to be
            // quiet from this one on.
            Act::CardKeep | Act::CardGo | Act::CardBox
                if self.card.chained.is_some_and(|t| held(t, now)) =>
            {
                self.card.chained = Some(now);
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
        if set_up && card.run.is_none() && !card.out {
            parts.push((t!("dev.card.hint_settings").to_string(), 2));
        }
        if plan_of(card).is_some_and(|p| p.plays > 0) && !card.out {
            parts.push((t!("dev.card.hint_plays").to_string(), 4));
        }
        let still = card.run.is_none() && !card.out;
        if card.found.accepted() && primary(card) != Some(CardPrimary::Eject) && still {
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

/// The plan's words for Next sync (clauses 44, 51a): `12 new · 2 to remove ·
/// 37 plays to send · waveforms to add`, or "nothing new on Home". `busy`: the
/// busy line's, its first piece naming the server.
pub(super) fn next_words(card: &CardView, plan: &Planned, busy: bool) -> String {
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
    if plan.waves_missing {
        parts.push(t!("dev.card.next_waves").to_string());
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

// ── The gates (clause 58) ───────────────────────────────────────────────────

/// A gate of a card's: what it asks, and what a yes sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CardGate {
    pub(crate) key: String,
    pub(crate) kind: GateKind,
    /// The answers already given, the next sync's with this gate's yes.
    pub(crate) allow: Allow,
    /// The card it was opened for: dropped once that card is gone.
    pub(crate) card: Opened,
}

/// The card a sheet or a gate was opened for, as its slot showed it then:
/// its volume serial, and its root's card id. A card at that slot's key
/// with another serial, or read with another id (or none), is another
/// card, whatever its key: two cards with no serial share one, a reader's
/// path. The sheet or the gate goes with the card it was opened for,
/// never answered for another.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Opened {
    serial: Option<String>,
    card_id: Option<u64>,
}

impl Opened {
    pub(crate) fn of(card: &CardView) -> Opened {
        let serial = card.found.volume.serial.as_ref().map(|s| s.to_string());
        Opened { serial, card_id: card.facts().and_then(|f| f.root).map(|r| r.0) }
    }

    /// Its card renamed with the serial the OS lists late: that serial is
    /// its own.
    fn follow(&mut self, now: Option<Opened>) {
        if let Some(now) = now.filter(|_| self.serial.is_none()) {
            self.serial = now.serial;
        }
    }

    /// `card`, as the row shows it now, is still the card opened for: in
    /// the reader, its serial, and its id once read (a read under way says
    /// nothing yet).
    pub(crate) fn still(&self, card: &CardView) -> bool {
        let now = Opened::of(card);
        let read = matches!(card.read, Read::Read(_));
        !card.out && now.serial == self.serial && (!read || now.card_id == self.card_id)
    }
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
    /// yes runs again taking it over. `plays`: the card's plays that wait
    /// on this computer for the server it leaves, as the stopped sync or
    /// the card's last look counted them; 0 says nothing of them.
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
    Some(CardGate { key, kind, allow, card: Opened::of(card) })
}

/// The card's plays that wait for its server, as its last look counted
/// them: the plan's, or a signed-out server's count off the card alone. 0
/// when the look has no count (the server didn't answer): nothing is said
/// of them then. Never the number of marks its state file holds, which
/// counts the players that logged plays, not the plays.
fn plays_waiting(card: &CardView) -> usize {
    match &card.next {
        Next::Plan(plan) => plan.plays,
        Next::SignIn { plays } => *plays,
        _ => 0,
    }
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
        Asked::Server { rows, plays } => {
            let new = card.server.link().map(|l| l.name.clone()).unwrap_or_default();
            let old = card.facts().map(|f| f.nickname.clone()).unwrap_or_default();
            GateKind::Server { card: name, old, new, rows: *rows, plays: *plays, want: None }
        }
        Asked::Proxy { peer } => GateKind::Proxy { peer: peer_name(card, *peer), id: *peer, server },
        Asked::DiskCheck => return None,
    })
}

/// Where the first of several sentences ends, its stop kept: at ". ", or
/// at the ideographic full stop Japanese and Chinese end one with. None for
/// one sentence alone.
pub(super) fn first_sentence(words: &str) -> Option<usize> {
    let latin = words.find(". ").map(|at| at + 1);
    let ideographic = words.find('。').map(|at| at + '。'.len_utf8()).filter(|&end| end < words.trim_end().len());
    match (latin, ideographic) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
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
            let opened = Opened::of(&card);
            self.card.gate = Some(CardGate { key: card.key.clone(), kind, allow: Allow::default(), card: opened });
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
        let card = self.card_of(&key);
        let next = card.as_ref().and_then(|c| gate_for(c, &plan_of(c).cloned().unwrap_or_default(), allow.clone()));
        match next {
            Some(next) => {
                self.card.gate = Some(next);
                self.card.chained = Some(Instant::now());
            }
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
    /// The card it was opened for: dropped once that card is gone.
    pub(crate) card: Opened,
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

    pub(super) fn row(&self) -> SRow {
        let rows = self.rows();
        rows.get(self.focus.min(rows.len().saturating_sub(1))).cloned().unwrap_or(SRow::Buttons)
    }

    pub(super) fn libraries(&self) -> Vec<String> {
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
        let peers: Vec<(u32, String, Vec<String>)> = facts
            .state
            .as_ref()
            .map(|s| s.settings.peers.iter().map(|p| (p.peer_id, p.nickname.clone(), p.libraries.clone())).collect())
            .unwrap_or_default();
        // The Budget row says the limit saved on this computer, not the
        // default; Apply with it unchanged writes nothing.
        let ids: Vec<u32> = peers.iter().map(|(id, ..)| *id).collect();
        want.budget_mb = saved_budget(card.server.link(), &ids);
        self.card.seq += 1;
        let sheet = CardSheet {
            key: card.key.clone(),
            card: Opened::of(card),
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
        let changed = sheet.want != sheet.was;
        // The Budget row shows the limit saved on this computer: Apply
        // writes one, to every peer of the card's, only when the listener
        // changed it.
        if sheet.want.budget_mb == sheet.was.budget_mb {
            sheet.want.budget_mb = None;
        }
        let key = sheet.key.clone();
        if sheet.setup {
            self.send_card(worker::Cmd::SetUp { key, want: Box::new(sheet.want) });
            return;
        }
        let card = self.card_of(&key);
        let current = card.as_ref().and_then(|c| c.server.link().map(|l| l.id.clone()));
        let filled = card.as_ref().and_then(|c| c.facts()).is_some_and(|f| f.tracks > 0);
        if filled && sheet.want.server.is_some() && sheet.want.server != current {
            let card = card.expect("the card");
            let opened = Opened::of(&card);
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
                plays: plays_waiting(&card),
                want: Some(Box::new(sheet.want)),
            };
            self.card.gate = Some(CardGate { key, kind, allow: Allow::default(), card: opened });
            return;
        }
        if changed {
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
pub(super) fn list_entries(sheet: &CardSheet, servers: &[ServerLink]) -> Vec<String> {
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
