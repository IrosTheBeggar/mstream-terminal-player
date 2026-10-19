//! The Card view on the page (clauses 3a, 12, 15, 20a, 25, 25a, 44–63): a
//! card in a reader beside the boards, its states, its gates, its sheet and
//! its sync, drawn from cards as the transfer worker tells them
//! (worker::fixtures). The page rides channels the test holds: no worker
//! runs, no volume or server is touched.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use super::*;
use crate::device::board::{CardRef, CardRoot};
use crate::device::card::cards::{Cards as Known, Seen};
use crate::device::card::engine::Guard;
use crate::device::card::volume::{Fs, Table};
use crate::device::card::worker::{self, CardView, Fold, In, Link, Next, fixtures as cards};

/// Unix seconds now: the cards' times are said from it.
fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A page with these boards and these cards, on channels the test holds:
/// the board worker's ends, the transfer worker's inbox, its fold.
pub(super) fn with_cards(
    tell: impl FnOnce(&Ends),
    list: Vec<CardView>,
) -> (Page, Ends, Receiver<In>, Rc<RefCell<Fold>>) {
    let (mut page, ends) = Page::quiet();
    let (link, inbox) = Link::pair();
    let fold = Rc::new(RefCell::new(Fold::default()));
    {
        let mut f = fold.borrow_mut();
        f.watched = true;
        f.servers = vec![cards::home()];
        for card in list {
            f.apply(worker::Event::Card(Box::new(card)));
        }
    }
    page.attach(Transfer { link, fold: fold.clone() });
    tell(&ends);
    page.pump();
    (page, ends, inbox, fold)
}

/// The commands the page sent the transfer worker.
fn sent(inbox: &Receiver<In>) -> Vec<worker::Cmd> {
    worker::sent(inbox)
}

/// A card's tab on the row: `E: Car music` on Windows, the name elsewhere.
fn tab(name: &str) -> String {
    if cfg!(windows) { format!("E: {name}") } else { name.to_string() }
}

#[test]
fn a_card_alone_is_its_view_card_03s_rows_with_no_tab_row() {
    let _en = english();
    let (mut page, _ends, inbox, _fold) = with_cards(|_| {}, vec![cards::ready("E:", now())]);
    let frame = window(&mut page);
    let card = [
        "           ╭────────────────────────────────────────────────────────────────────────────╮",
        "           │                                                                            │",
        "           │  Car music                                                     E: · FAT32  │",
        "           │                                                                            │",
        "           │  Holds         Rock, Jazz from Home                                        │",
        "           │  Space         █████████████████████████████████████░░░░░░░░░░░░░░░░░░░░░  │",
        "           │                41.2 of 63.8 GB used · 4,812 tracks           22.6 GB free  │",
        "           │  Last sync     3 days ago                                                  │",
        "           │  Next sync     12 new · 2 to remove · 37 plays to send                     │",
        "           │  Converts      only what the player can't play, to Opus 192k               │",
        "           │                                                                            │",
        "           │                                                              ╭──────────╮  │",
        "           │  ▸ Details   Settings…   Send the plays   Eject              │  Sync ▸  │  │",
        "           │                                                              ╰──────────╯  │",
        "           │                                                                            │",
        "           ╰────────────────────────────────────────────────────────────────────────────╯",
    ];
    assert_eq!(rows(&frame, 2, 16), card, "card 03's one-subject frame:\n{frame}");
    assert_eq!(page.hint(), "Enter sync · o settings · p send plays · e eject · d details · Esc library");
    // Two removals ask nothing: Enter is the sync.
    press(&mut page, KeyCode::Enter);
    let key = "E:\\".to_string();
    assert_eq!(sent(&inbox), [worker::Cmd::Sync { key: key.clone(), allow: Box::default() }]);
    // Its links' keys.
    for (code, cmd) in [
        ('p', worker::Cmd::Plays { key: key.clone() }),
        ('e', worker::Cmd::Eject { key: key.clone() }),
    ] {
        press(&mut page, KeyCode::Char(code));
        assert_eq!(sent(&inbox), [cmd]);
    }
    // Details: the facts and the log.
    press(&mut page, KeyCode::Char('d'));
    let frame = window(&mut page);
    assert!(frame.contains("Drive         E: · FAT32 on MBR · 63.8 GB · 32 KB clusters · 1A2B-3C4D"), "{frame}");
    assert!(frame.contains("Card          id 5eedc0de0a1b2c3d · revision 42 · mStream Player 0.14.0"), "{frame}");
    assert!(frame.contains("Server        Home · signed in on this computer"), "{frame}");
    assert!(frame.contains("Removes       Velvet Static/Harbour Songs/04 - Low Tide.flac and 1 more"), "{frame}");
    assert!(frame.contains("Player        Core2 0.12.0: mp3,flac,opus · Auto DJ 2 · play log"), "{frame}");
    assert!(frame.contains("Sync again"), "{frame}");
}

#[test]
fn a_board_and_a_card_share_one_row_and_the_arrows_walk_every_subject() {
    let _en = english();
    let (mut page, _ends, _inbox, _fold) = with_cards(Ends::up_to_date, vec![cards::ready("E:", now())]);
    let frame = window(&mut page);
    let row = &rows(&frame, 2, 1)[0];
    assert!(row.starts_with(&format!("            COM3 ✓   {} !", tab("Car music"))), "{row:?}");
    assert!(row.trim_end().ends_with("1 player · 1 card"), "{row:?}");
    // The board keeps the view: a card put in takes it only from the empty
    // card (clause 25, the pick of 2026-10-19).
    assert!(frame.contains("M5Stack Core2"), "{frame}");
    assert_eq!(page.hint(), "←→ switch · o advanced · d details · Esc library");
    press(&mut page, KeyCode::Right);
    let frame = window(&mut page);
    assert!(frame.contains("│  Car music") && frame.contains("Holds"), "{frame}");
    assert!(page.hint().starts_with("←→ switch · Enter sync"), "{}", page.hint());
    press(&mut page, KeyCode::Right);
    assert!(window(&mut page).contains("M5Stack Core2"), "wrapping, back to the board");
    // A click on the card's tab.
    let frame = window(&mut page);
    let row = rows(&frame, 2, 1)[0].clone();
    click(&mut page, col(&row, &tab("Car music")) + 1, 2);
    assert!(window(&mut page).contains("Holds"));
}

#[test]
fn the_card_put_in_takes_the_view_only_from_the_empty_card() {
    let _en = english();
    let (mut page, _ends, _inbox, fold) = with_cards(Ends::no_board, Vec::new());
    assert!(window(&mut page).contains("Nothing plugged in"));
    fold.borrow_mut().apply(worker::Event::Card(Box::new(cards::reading("E:"))));
    page.pump();
    let frame = window(&mut page);
    assert!(frame.contains("Card in E:") && frame.contains("▱▱▱▱▱▱▱▱▱▱ reading the card…"), "{frame}");
    // Taken out: the empty card again.
    fold.borrow_mut().apply(worker::Event::Gone { key: "E:\\".into() });
    page.pump();
    assert!(window(&mut page).contains("Nothing plugged in"));
}

#[test]
fn a_new_card_offers_set_up_and_its_sheet_sets_it_up() {
    let _en = english();
    let (mut page, _ends, inbox, _fold) = with_cards(|_| {}, vec![cards::new_card("F:")]);
    let frame = window(&mut page);
    assert!(frame.contains("│  Card in F:"), "{frame}");
    assert!(frame.contains("Holds         ! A FAT32 card with nothing from mStream on it yet."), "{frame}");
    assert!(frame.contains("Last sync     never: nothing from mStream on it yet"), "{frame}");
    assert!(frame.contains("│  Set up ▸  │") && frame.contains("▸ Details   Eject"), "{frame}");
    assert_eq!(page.hint(), "Enter set up · e eject · d details · Esc library");
    press(&mut page, KeyCode::Enter);
    let frame = window(&mut page);
    assert!(frame.contains("Set up F:"), "the sheet, titled for a set-up:\n{frame}");
    assert!(frame.contains("Cancel") && frame.contains("Set up"), "{frame}");
    assert!(!frame.contains("Writing"), "a set-up's sheet is Music alone:\n{frame}");
    // A name typed in its field, then Set up.
    press(&mut page, KeyCode::Enter);
    for c in "Gym".chars() {
        press(&mut page, KeyCode::Char(c));
    }
    press(&mut page, KeyCode::Enter);
    press(&mut page, KeyCode::Char('o'));
    assert!(sent(&inbox).is_empty(), "o is no key of the sheet's");
    press(&mut page, KeyCode::BackTab);
    page.card.sheet.as_mut().unwrap().focus = 99;
    press(&mut page, KeyCode::Enter);
    let sent = sent(&inbox);
    let [worker::Cmd::SetUp { key, want }] = sent.as_slice() else { panic!("{sent:?}") };
    assert_eq!((key.as_str(), want.name.as_str()), ("F:\\", "Gym"));
    assert_eq!(want.server.as_deref(), Some("http://127.0.0.1:3000"), "the session's server");
}

#[test]
fn removals_past_fifty_ask_first_and_half_the_card_waits_for_its_box() {
    let _en = english();
    let many: Vec<String> = (0..214).map(|i| format!("Velvet Static/Harbour Songs/{i:03} - Track.flac")).collect();
    let card = cards::with(cards::ready("E:", now()), |c| {
        if let Next::Plan(plan) = &mut c.next {
            plan.removals = many.clone();
        }
    });
    let (mut page, _ends, inbox, _fold) = with_cards(|_| {}, vec![card]);
    press(&mut page, KeyCode::Enter);
    assert!(sent(&inbox).is_empty(), "the gate first");
    let frame = window(&mut page);
    assert!(frame.contains("Remove 214 tracks from Car music?"), "{frame}");
    assert!(frame.contains("  Velvet Static/Harbour Songs/000 - Track.flac"), "{frame}");
    assert!(frame.contains("  and 209 more"), "{frame}");
    assert!(frame.contains("◂ Not now") && frame.contains("Remove 214 and sync"), "{frame}");
    assert_eq!(page.hint(), "y remove and sync · Enter or Esc not now");
    press(&mut page, KeyCode::Esc);
    assert!(page.card.gate.is_none());
    assert_eq!(sent(&inbox), [worker::Cmd::Dismiss { key: "E:\\".into() }]);
    press(&mut page, KeyCode::Enter);
    press(&mut page, KeyCode::Char('y'));
    let sent_now = sent(&inbox);
    let [worker::Cmd::Sync { allow, .. }] = sent_now.as_slice() else { panic!("{sent_now:?}") };
    assert!(allow.answers.removals && !allow.answers.mass_removals);
    assert_eq!(allow.answers.removals_seen, Some(214), "the yes names its count");

    // Half the card: no second button until the box is ticked; its count
    // per library from the look, as from a sync that stopped for it.
    let card = cards::with(cards::ready("E:", now()), |c| {
        if let Next::Plan(plan) = &mut c.next {
            plan.removals = many.clone();
            plan.guards = vec![Guard::Removals { paths: many.clone(), strong: true }];
            plan.removal_libs = (0..214).map(|i| if i < 200 { "Rock" } else { "Jazz" }.to_string()).collect();
        }
    });
    let (mut page, _ends, inbox, _fold) = with_cards(|_| {}, vec![card]);
    press(&mut page, KeyCode::Enter);
    let frame = window(&mut page);
    assert!(frame.contains("Remove 214 of Car music's 4,812 tracks?"), "{frame}");
    assert!(frame.contains("  Rock  200 of 2,904 gone"), "{frame}");
    assert!(frame.contains("  Jazz  14 of 1,908 gone"), "{frame}");
    assert!(frame.contains("[ ] Home really has dropped them"), "{frame}");
    assert!(!frame.contains("Remove 214 and sync"), "no second button yet:\n{frame}");
    assert_eq!(page.hint(), "e tick the box · Enter or Esc not now");
    press(&mut page, KeyCode::Char('y'));
    assert!(sent(&inbox).is_empty() && page.card.gate.is_some(), "y does nothing yet");
    press(&mut page, KeyCode::Char('e'));
    let frame = window(&mut page);
    assert!(frame.contains("[✓] Home really has dropped them") && frame.contains("Remove 214 and sync"), "{frame}");
    assert_eq!(page.hint(), "y remove and sync · Enter or Esc not now");
    press(&mut page, KeyCode::Char('e'));
    assert!(!window(&mut page).contains("Remove 214 and sync"), "unticked, the button goes");
    press(&mut page, KeyCode::Char('e'));
    press(&mut page, KeyCode::Char('y'));
    let sent_now = sent(&inbox);
    let [worker::Cmd::Sync { allow, .. }] = sent_now.as_slice() else { panic!("{sent_now:?}") };
    assert!(allow.answers.removals && allow.answers.mass_removals);
    assert_eq!(allow.answers.removals_seen, Some(214));
}

#[test]
fn a_sync_draws_its_progress_and_x_stops_it_at_the_next_save() {
    let _en = english();
    let (mut page, _ends, inbox, fold) = with_cards(Ends::up_to_date, vec![cards::syncing("E:", now())]);
    // Coming back opens the view on the card being synced... with a board
    // plugged in the board keeps it; the arrows reach the card.
    press(&mut page, KeyCode::Right);
    let frame = window(&mut page);
    let row = &rows(&frame, 2, 1)[0];
    assert!(row.contains(&format!("{} 28%", tab("Car music"))), "the tab's percentage: {row:?}");
    assert!(row.trim_end().ends_with("syncing Car music…"), "{row:?}");
    assert!(frame.contains("1,204 of 4,812 · 6.1 of 21.3 GB"), "{frame}");
    assert!(frame.contains("about 20 min left"), "{frame}");
    assert!(frame.contains("12.4 MB/s · saved 3 min ago, safe to stop"), "{frame}");
    assert!(frame.contains("Now           Orla Finch/Paper Boats/03 - Fold.flac"), "{frame}");
    assert!(frame.contains("▸ Details   Stop at the next save"), "{frame}");
    assert_eq!(page.hint(), "←→ switch · x stop at the next save · d details · Esc library");
    press(&mut page, KeyCode::Char('x'));
    assert_eq!(sent(&inbox), [worker::Cmd::Stop { key: "E:\\".into() }]);
    // Stopping: the words say so in the link's place.
    let mut card = fold.borrow().cards[0].clone();
    card.run.as_mut().unwrap().stopping = true;
    fold.borrow_mut().apply(worker::Event::Card(Box::new(card)));
    page.pump();
    let frame = window(&mut page);
    assert!(frame.contains("stopping at the next save…") && !frame.contains("files to it"), "{frame}");
    // Settings, Eject and the plays wait for the sync: nothing goes (the
    // sync sends the plays itself).
    press(&mut page, KeyCode::Char('o'));
    press(&mut page, KeyCode::Char('e'));
    press(&mut page, KeyCode::Char('p'));
    assert!(sent(&inbox).is_empty() && page.card.sheet.is_none());
}

#[test]
fn during_a_firmware_write_a_card_is_looked_at_only_and_x_still_stops_its_sync() {
    let _en = english();
    let (mut page, _ends, inbox, _fold) =
        with_cards(Ends::update_available, vec![cards::syncing("E:", now())]);
    press(&mut page, KeyCode::Enter);
    press(&mut page, KeyCode::Char('y'));
    assert!(page.writing());
    press(&mut page, KeyCode::Right);
    assert!(window(&mut page).contains("Car music"), "the arrows still look at the card");
    press(&mut page, KeyCode::Char('o'));
    press(&mut page, KeyCode::Enter);
    assert!(sent(&inbox).is_empty() && page.card.sheet.is_none(), "nothing starts during a write");
    press(&mut page, KeyCode::Char('x'));
    assert_eq!(sent(&inbox), [worker::Cmd::Stop { key: "E:\\".into() }], "the stop touches no board");
}

#[test]
fn the_board_names_the_card_it_holds_by_its_cid() {
    let _en = english();
    let id = 0x5EED_C0DE_0A1B_2C3D;
    let board = |cid: CardRoot| fixtures::answering_with("COM3", "v0.8.0", move |s| s.cid = cid);
    let loaded = |generation| CardRoot::Loaded(CardRef { card_id: id, generation });
    let mut known = Known::default();
    let synced = now() - 86_400;
    let seen = Seen { label: "Gym".into(), generation: 3, server: "Home".into(), synced, ..Seen::default() };
    known.cards.insert(id, seen);
    let (mut page, _ends) = page_with(|e| e.boards(vec![board(loaded(3))]));
    page.card.known = Some(known.clone());
    let frame = window(&mut page);
    assert!(frame.contains("This is Gym · synced yesterday from Home"), "{frame}");
    assert!(!frame.contains("restart it"), "{frame}");
    // A commit of this computer's the board hasn't loaded yet.
    let (mut page, _ends) = page_with(|e| e.boards(vec![board(loaded(2))]));
    page.card.known = Some(known.clone());
    let frame = window(&mut page);
    assert!(frame.contains("The player still plays the music from before the last"), "{frame}");
    // Another computer's card, a card not filled yet, firmware without the key.
    let theirs = CardRoot::Loaded(CardRef { card_id: 9, generation: 1 });
    let (mut page, _ends) = page_with(|e| e.boards(vec![board(theirs)]));
    page.card.known = Some(known.clone());
    assert!(window(&mut page).contains("Filled by mStream on another computer."));
    let (mut page, _ends) = page_with(|e| e.boards(vec![board(CardRoot::None)]));
    assert!(window(&mut page).contains("Nothing from mStream on it yet: put it in this computer's"));
    let (mut page, _ends) = page_with(Ends::up_to_date);
    let frame = window(&mut page);
    assert!(!frame.contains("This is") && !frame.contains("Filled by"), "no cid=, no line:\n{frame}");
}

#[test]
fn a_volume_the_player_cant_use_says_why_and_h_hides_it() {
    let _en = english();
    let exfat = cards::refused("G:", Fs::ExFat, Table::Mbr);
    let (mut page, _ends, inbox, _fold) = with_cards(|_| {}, vec![exfat]);
    let frame = window(&mut page);
    assert!(frame.contains("Card in G:") && frame.contains("G: · exFAT · 63.8 GB"), "{frame}");
    assert!(frame.contains("Format        ! exFAT card · the player reads FAT32 only"), "{frame}");
    assert!(frame.contains("Your Core2 can format it: eject it, put it in the Core2"), "{frame}");
    assert!(frame.contains("│  Eject ▸  │") && frame.contains("Not a card? Hide it"), "{frame}");
    assert_eq!(page.hint(), "Enter eject · h hide · d details · Esc library");
    // Eject ▸, the route's first step: Enter, and e (clause 12's key).
    for code in [KeyCode::Enter, KeyCode::Char('e')] {
        press(&mut page, code);
        assert_eq!(sent(&inbox), [worker::Cmd::Eject { key: "G:\\".into() }], "{code:?}");
    }
    press(&mut page, KeyCode::Char('h'));
    assert_eq!(sent(&inbox), [worker::Cmd::Hide { key: "G:\\".into() }]);
    // With a board plugged in, the route names its port.
    let gpt = cards::refused("G:", Fs::Fat32, Table::Gpt);
    let (mut page, _ends, _inbox, _fold) = with_cards(Ends::up_to_date, vec![gpt]);
    press(&mut page, KeyCode::Right);
    let frame = window(&mut page);
    assert!(frame.contains("G: · FAT32 on GPT"), "{frame}");
    assert!(frame.contains("The player can format it: eject it, put it in the Core2 on"), "{frame}");
    assert!(frame.contains("COM3, and choose Format as FAT32 on COM3's tab."), "{frame}");
}

#[test]
fn the_settings_sheet_has_two_tabs_and_asks_the_fit_at_every_change() {
    let _en = english();
    let (mut page, _ends, inbox, _fold) = with_cards(|_| {}, vec![cards::ready("E:", now())]);
    press(&mut page, KeyCode::Char('o'));
    let frame = window(&mut page);
    assert!(frame.contains("Settings · Car music"), "{frame}");
    assert!(frame.contains(" Music ") && frame.contains(" Writing "), "{frame}");
    assert!(frame.contains("Name          Car music"), "{frame}");
    assert!(frame.contains("From          Home ▾"), "{frame}");
    assert!(frame.contains("[✓] Rock") && frame.contains("27.9 GB · 2,904 tracks"), "{frame}");
    assert!(frame.contains("[ ] Audiobooks"), "{frame}");
    assert!(frame.contains("✓ Fits · "), "the fit line:\n{frame}");
    assert!(page.hint().contains("Music / Writing"), "{}", page.hint());
    // Tick Audiobooks: the fit is asked again, by number.
    for _ in 0..4 {
        press(&mut page, KeyCode::Down);
    }
    press(&mut page, KeyCode::Char(' '));
    let asked = sent(&inbox);
    let [worker::Cmd::Fit { want, .. }] = asked.as_slice() else { panic!("{asked:?}") };
    assert_eq!(want.libraries.as_deref(), Some(&["Rock".to_string(), "Jazz".into(), "Audiobooks".into()][..]));
    // The Writing tab.
    page.card.sheet.as_mut().unwrap().focus = 0;
    press(&mut page, KeyCode::Right);
    let frame = window(&mut page);
    assert!(frame.contains("Convert       (•) only what it can't play   ( ) everything"), "{frame}");
    assert!(frame.contains("Removals      (•) follow Home   ( ) never remove"), "{frame}");
    assert!(frame.contains("Covers        [✓] small covers for the player"), "{frame}");
    // Never remove, then Apply: the settings written, nothing synced.
    press(&mut page, KeyCode::Down);
    press(&mut page, KeyCode::Down);
    press(&mut page, KeyCode::Char(' '));
    let _ = sent(&inbox);
    press(&mut page, KeyCode::Enter);
    let applied = sent(&inbox);
    let [worker::Cmd::Apply { want, .. }] = applied.as_slice() else { panic!("{applied:?}") };
    assert_eq!(want.removal, crate::device::card::engine::Removal::Never);
    assert!(page.card.sheet.is_none());
    // Esc changes nothing.
    press(&mut page, KeyCode::Char('o'));
    press(&mut page, KeyCode::Esc);
    assert!(sent(&inbox).is_empty() && page.card.sheet.is_none());
}

#[test]
fn the_cards_states_at_rest_say_what_the_card_is_and_the_way_on() {
    let _en = english();
    let draw = |card: CardView| -> String {
        let (mut page, _ends, _inbox, _fold) = with_cards(|_| {}, vec![card]);
        window(&mut page)
    };
    let ready = || cards::ready("E:", now());
    let frame = draw(cards::with(ready(), |c| c.server = worker::Server::Unknown("Home".into())));
    assert!(frame.contains("Server        ! Filled from Home, which this computer has no sign-in for"), "{frame}");
    assert!(frame.contains("Add the server ▸"), "{frame}");
    let frame = draw(cards::with(ready(), |c| c.server = worker::Server::New(None)));
    assert!(frame.contains("! No server saved on this computer yet"), "{frame}");
    let frame = draw(cards::with(ready(), |c| c.next = Next::SignIn));
    assert!(frame.contains("! Home needs you to sign in again") && frame.contains("Sign in ▸"), "{frame}");
    let frame = draw(cards::with(ready(), |c| c.next = Next::Unreached { asked: now() - 120, why: "x".into() }));
    assert!(frame.contains("Next sync     Home didn't answer · asked 2 min ago"), "{frame}");
    assert!(frame.contains("Try now"), "{frame}");
    let frame = draw(cards::with(ready(), |c| {
        cards::facts(c).newer = vec!["manifest.bin".into()];
        cards::facts(c).newer_major = Some(3);
    }));
    assert!(frame.contains("? Written by a newer mStream Player (card format 3)"), "{frame}");
    // A private file's newer version names no card format.
    let frame = draw(cards::with(ready(), |c| cards::facts(c).newer = vec!["state.json".into()]));
    assert!(frame.contains("? Written by a newer mStream Player "), "{frame}");
    assert!(!frame.contains("card format"), "{frame}");
    let frame = draw(cards::with(ready(), |c| cards::facts(c).check = true));
    assert!(frame.contains("Check         ✗ Check this card on the computer first"), "{frame}");
    assert!(frame.contains("Windows: E: › Properties › Tools › Check.") && frame.contains("Check again ▸"), "{frame}");
    let frame = draw(cards::with(ready(), |c| cards::facts(c).copied = true));
    assert!(frame.contains("! A copy of another card") && frame.contains("Give it its own identity ▸"), "{frame}");
    let frame = draw(cards::with(ready(), |c| {
        if let Next::Plan(plan) = &mut c.next {
            let (needed, room) = (33_500_000_000, 22_600_000_000);
            plan.fit = Some(crate::device::card::plan::Fit { needed, room, tracks: 5_000 });
        }
    }));
    assert!(frame.contains("Next sync     ✗ Doesn't fit by 10.9 GB") && frame.contains("Choose what fits ▸"), "{frame}");
    let frame = draw(cards::with(ready(), |c| cards::facts(c).device.present = false));
    assert!(frame.contains("no device.txt yet: synced for a player's first defaults"), "{frame}");
    let frame = draw(cards::with(ready(), |c| {
        c.ended = Some(worker::Ended::Done { tracks: 4_822, took: 1_680, skipped: Vec::new(), plays: 37 });
        c.next = Next::Plan(Box::default());
    }));
    assert!(frame.contains("Last sync     just now · verified · 37 plays sent to Home"), "{frame}");
    assert!(frame.contains("│  Eject ▸  │"), "Done's primary is Eject (choice 12):\n{frame}");
    let frame = draw(cards::with(ready(), |c| {
        c.run = Some(worker::Run { phase: worker::Phase::Waiting, ..worker::Run::default() });
    }));
    assert!(frame.contains("(one sync at a time)"), "{frame}");
    assert!(frame.contains("Don't wait"), "{frame}");
    let frame = draw(cards::with(ready(), |c| {
        let pause = worker::Pause::Server { until: now() + 120, tries: 3 };
        let bytes = (8_600_000_000, 21_300_000_000);
        c.ended = Some(worker::Ended::Paused { pause, at: 1_930, of: 4_812, bytes });
    }));
    assert!(frame.contains("! Paused · Home didn't answer · trying again in 2 min (try"), "{frame}");
    assert!(frame.contains("▸ Details   Try now   Stop for now  "), "no primary, no Eject:\n{frame}");
    assert!(!frame.contains("Sync ▸"), "{frame}");
    let frame = draw(cards::with(ready(), |c| {
        c.out = true;
        c.ended = Some(worker::Ended::Out { at: 1_204, of: 4_812 });
    }));
    assert!(frame.contains("not in a reader") && frame.contains("✗ Taken out · stopped at 1,204 of 4,812"), "{frame}");
    assert!(frame.contains("Give up this run…"), "{frame}");
}

/// Studio, a second server saved on this computer.
fn studio() -> worker::ServerLink {
    worker::ServerLink {
        id: "http://127.0.0.1:4000".into(),
        name: "Studio".into(),
        base: Ok("http://127.0.0.1:4000".into()),
        ..worker::ServerLink::default()
    }
}

/// The sheet open on the card, From set to Studio, then Apply.
fn apply_studio(page: &mut Page, inbox: &Receiver<In>) {
    press(page, KeyCode::Char('o'));
    press(page, KeyCode::Down);
    press(page, KeyCode::Enter);
    press(page, KeyCode::Down);
    press(page, KeyCode::Enter);
    let asked = sent(inbox);
    assert!(matches!(asked.as_slice(), [worker::Cmd::Fit { .. }]), "the fit only: {asked:?}");
    page.card.sheet.as_mut().unwrap().focus = 99;
    press(page, KeyCode::Enter);
}

#[test]
fn another_server_in_settings_asks_first_and_writes_nothing_until_its_yes() {
    let _en = english();
    let (mut page, _ends, inbox, fold) = with_cards(|_| {}, vec![cards::ready("E:", now())]);
    fold.borrow_mut().servers.push(studio());
    apply_studio(&mut page, &inbox);
    // The gate first, and nothing sent: the card is as it was.
    assert!(sent(&inbox).is_empty(), "nothing written before the gate's yes");
    let frame = window(&mut page);
    assert!(frame.contains("Make Studio this card's server?"), "{frame}");
    assert!(frame.contains("◂ Keep Home") && frame.contains("Make Studio its server"), "{frame}");
    assert_eq!(page.hint(), "y make Studio its server · Enter or Esc keep Home");
    // Keep Home: nothing written, nothing to forget.
    press(&mut page, KeyCode::Esc);
    assert!(page.card.gate.is_none() && sent(&inbox).is_empty());

    // Its yes writes the settings, and only them: Apply never syncs (clause
    // 48); the next Sync takes the card over (the worker's slot).
    apply_studio(&mut page, &inbox);
    press(&mut page, KeyCode::Char('y'));
    let applied = sent(&inbox);
    let [worker::Cmd::Apply { key, want }] = applied.as_slice() else { panic!("{applied:?}") };
    assert_eq!((key.as_str(), want.server.as_deref()), ("E:\\", Some("http://127.0.0.1:4000")));
    assert!(page.card.gate.is_none());
}

#[test]
fn leave_them_all_as_mine_syncs_without_adopting_and_a_peer_left_out_is_kept_out() {
    let _en = english();
    let card = cards::with(cards::ready("E:", now()), |c| {
        if let Next::Plan(plan) = &mut c.next {
            plan.guards = vec![Guard::Adopt(vec!["a.flac".into(); 1_190])];
        }
    });
    let (mut page, _ends, inbox, _fold) = with_cards(|_| {}, vec![card]);
    press(&mut page, KeyCode::Enter);
    let frame = window(&mut page);
    assert!(frame.contains("Adopt 1,190 files Home also has?"), "{frame}");
    // The keep is an answer: the sync goes on, adopting nothing (as
    // --keep-mine does).
    press(&mut page, KeyCode::Esc);
    let synced = sent(&inbox);
    let [worker::Cmd::Sync { allow, .. }] = synced.as_slice() else { panic!("{synced:?}") };
    assert_eq!(allow.answers.adopt, Some(false));
    press(&mut page, KeyCode::Enter);
    press(&mut page, KeyCode::Char('y'));
    let synced = sent(&inbox);
    let [worker::Cmd::Sync { allow, .. }] = synced.as_slice() else { panic!("{synced:?}") };
    assert_eq!(allow.answers.adopt, Some(true));

    // A peer only its parent's proxy reaches, asked after a sync: its keep
    // leaves it out of this card's syncs, kept by the worker.
    let card = cards::with(cards::ready("E:", now()), |c| c.gate = Some(vec![worker::Gate::Proxy { peer: 4 }]));
    let (mut page, _ends, inbox, _fold) = with_cards(|_| {}, vec![card]);
    let frame = window(&mut page);
    assert!(frame.contains("Copy from #4 through Home?"), "{frame}");
    press(&mut page, KeyCode::Esc);
    assert_eq!(sent(&inbox), [worker::Cmd::LeaveOut { key: "E:\\".into(), peer: 4 }]);
}

#[test]
fn a_card_taken_out_with_its_view_open_moves_the_view_right_else_left() {
    let _en = english();
    let (mut page, _ends, _inbox, fold) =
        with_cards(Ends::up_to_date, vec![cards::ready("E:", now()), cards::ready("F:", now())]);
    // COM3, E:, F: on the row; E: in view.
    press(&mut page, KeyCode::Right);
    assert_eq!(page.card_open.as_deref(), Some("E:\\"));
    fold.borrow_mut().apply(worker::Event::Gone { key: "E:\\".into() });
    page.pump();
    assert_eq!(page.card_open.as_deref(), Some("F:\\"), "the next to its right");
    // E: put back beside it; F: taken out: to its left, E:.
    fold.borrow_mut().apply(worker::Event::Card(Box::new(cards::ready("E:", now()))));
    page.pump();
    assert_eq!(page.card_open.as_deref(), Some("F:\\"), "a card put in takes no view");
    fold.borrow_mut().apply(worker::Event::Gone { key: "F:\\".into() });
    page.pump();
    assert_eq!(page.card_open.as_deref(), Some("E:\\"), "none to its right: to its left");
    // The last card out: the board to its left.
    fold.borrow_mut().apply(worker::Event::Gone { key: "E:\\".into() });
    page.pump();
    assert_eq!(page.card_open, None);
    assert!(window(&mut page).contains("M5Stack Core2"), "the board's view");
}
