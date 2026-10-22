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
    assert!(frame.contains("Card          id 5eedc0de0a1b2c3d · rev. 42 · mStream Player 0.14.0"), "{frame}");
    assert!(frame.contains("Server        Home · signed in on this computer"), "{frame}");
    assert!(frame.contains("Removes       Velvet Static/Harbour Songs/04 - Low Tide.flac and 1 more"), "{frame}");
    assert!(frame.contains("Player        Core2 0.12.0: mp3,flac,opus · Auto DJ 2 · play log"), "{frame}");
    assert!(frame.contains("Sync again"), "{frame}");
}

#[test]
fn details_drive_row_is_in_the_locales_words() {
    // Its file system, table, size and clusters were English in every
    // locale; a size is in the locale's unit (Go in French, whose row is
    // whole now, under a label that is no longer the player's, Lecteur).
    let rows = [
        ("de", "Laufwerk      E: · FAT32 auf MBR · 63.8 GB · 32-KB-Cluster · 1A2B-3C4D"),
        ("fr", "Volume        E: · FAT32 sur MBR · 63.8 Go · cluster 32 Ko · 1A2B-3C4D"),
    ];
    for (code, drive) in rows {
        let _in = crate::setup::tests::in_locale(code);
        let (mut page, _ends, _inbox, _fold) = with_cards(|_| {}, vec![cards::ready("E:", now())]);
        press(&mut page, KeyCode::Char('d'));
        let frame = window(&mut page);
        assert!(frame.contains(drive), "{code}:\n{frame}");
    }
}

/// The longest names a server realistically goes by here (the config's
/// `display_name`): an address with its scheme and port, one word a line
/// can't break, and Quick Connect's, its id's first 12 characters.
const LONG_SERVERS: [&str; 2] = ["https://192.168.100.200:3000", "quick connect · 0123456789ab"];

#[test]
fn every_locales_help_keeps_to_its_two_lines_at_the_floor_and_on_the_console() {
    // Clause 48: the help under the rule is two lines, an ellipsis past
    // them. With the longest server names, a library's name of 23 cells and
    // six digits of tracks, no row's help needs the ellipsis in any
    // language, in the sheet at the GUI's floor (70 cells of text) or in
    // the console's narrower sheet (64: only the GUI shows a Card view
    // today, so this width keeps the words ready for a page that would);
    // nor does a line start with a mark a space sets apart (French's
    // colon, Russian's dash).
    let lib = "Classical Music Archive";
    let offered = vec![(lib.to_string(), Some((123_456, 1_234_500_000_000))), ("Live".to_string(), None)];
    let rows = [
        card::SRow::Name,
        card::SRow::From,
        card::SRow::Library(lib.into()),
        card::SRow::Library("Live".into()),
        card::SRow::PeerLibrary(4, lib.into()),
        card::SRow::Convert,
        card::SRow::Into,
        card::SRow::Removals,
        card::SRow::Covers,
        card::SRow::AutoDj,
        card::SRow::For,
        card::SRow::Player,
        card::SRow::Budget,
        card::SRow::Buttons,
    ];
    for (code, _) in crate::setup::LANGS {
        let _in = crate::setup::tests::in_locale(code);
        for (area_w, tw) in [(100, 70), (MIN_W, 64)] {
            assert_eq!(usize::from(card::sheet_widths(area_w).1), tw, "the sheet's text at {area_w} columns");
            for server in LONG_SERVERS {
                for row in &rows {
                    let text = card::row_help(row, &offered, server);
                    assert!(!text.contains("dev."), "{code}: {row:?} has no words");
                    let lines: Vec<String> = card::help_lines(&text, tw).iter().map(|l| l.to_string()).collect();
                    let whole = wrap_spans(&[(text.clone(), dim())], tw).len() <= 2;
                    assert!(whole, "{code}, {row:?} in {tw} cells runs past its two lines: {lines:?}");
                    for line in &lines {
                        assert!(kit::width(line) <= tw, "{code}, {row:?} in {tw} cells: {line:?}");
                        let first = line.chars().next();
                        let set_apart = matches!(first, Some(':' | ';' | '!' | '?' | '—' | '–'));
                        assert!(!set_apart, "{code}, {row:?} in {tw} cells starts a line with a mark: {lines:?}");
                    }
                }
            }
        }
    }
}

/// The ready card on `drive` with the longest values a card realistically
/// shows in Details: a 1 TB card in 64 KB clusters, its revision in four
/// digits, filled from `server`, a month of plays to send, the player's
/// every codec, and a removal's long path among 1,235.
fn longest(drive: &str, server: &str, now: u64) -> CardView {
    cards::with(cards::ready(drive, now), |v| {
        if let Some(space) = v.found.volume.space.as_mut() {
            space.size = 1_000_200_000_000;
            space.cluster = 64 * 1024;
        }
        let facts = cards::facts(v);
        facts.root = Some((0x5EED_C0DE_0A1B_2C3D, 4_821, 7));
        facts.nickname = server.into();
        let txt = b"contract=1\nfirmware=0.12.0\nread.mpdj=2\ncodecs=mp3,flac,opus,wav\nhistory=1\n";
        facts.device = crate::device::card::contract::device_txt::DeviceTxt::read(Some(txt));
        if let Next::Plan(plan) = &mut v.next {
            let long = "Velvet Static/Harbour Songs (Deluxe Edition)/04 - Low Tide (Extended Mix).flac".to_string();
            plan.removals = std::iter::once(long).chain((0..1_234).map(|i| format!("x/{i}.flac"))).collect();
            let tally = crate::device::card::history::Tally { owed: 1_234, clock: 99, own: 1_234, ..Default::default() };
            plan.tally = Some(tally);
        }
    })
}

#[test]
fn every_locales_facts_keep_to_their_line_with_the_longest_values() {
    // Details' facts are a line each in the value column (clause 44): with
    // the longest values, every fact is whole in every language, but where
    // a value of its own runs long: Removes' path gives way before its
    // count, and a volume's long name (Linux, macOS) takes the Drive fact's
    // serial, then its clusters, never cutting a piece.
    let now = now();
    let content_w = CARD_W - inset(CARD_W) * 2;
    let w = usize::from(content_w - LABEL_W);
    assert_eq!(w, 58, "the value column at the GUI's floor");
    // A drive letter, a FAT32 label's 11 cells (a mount point's name on
    // Linux or macOS), and a name longer than a label holds, for the rule
    // alone: it takes the clusters too.
    let drives = ["E:", "MSTREAM_CAR", "MSTREAM_CAR_MUSIC_BACKUP"];
    for (code, _) in crate::setup::LANGS {
        let _in = crate::setup::tests::in_locale(code);
        // The Drive fact's pieces, the last of them the first to give way.
        let pieces = |drive: &str| {
            let fs = t!("dev.card.head_on", fs = "FAT32", table = "MBR").to_string();
            let size = t!("dev.card_size", size = "1000.2").to_string();
            let kb = t!("dev.card.fact_drive_kb", kb = 64).to_string();
            vec![drive.to_string(), fs, size, kb, "1A2B-3C4D".to_string()]
        };
        for drive in drives {
            for server in LONG_SERVERS {
                let (mut page, _ends, _inbox, _fold) = with_cards(|_| {}, vec![longest(drive, server, now)]);
                press(&mut page, KeyCode::Char('d'));
                let view = page.open_card().expect("the card in view");
                let facts: Vec<(String, String)> = card::card_layout(&page, &view, content_w)
                    .into_iter()
                    .filter_map(|row| match row {
                        Row::Fact { label, value, .. } => Some((label, value)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(facts.len(), 6, "{code}: Drive, Card, Server, Removes, Plays, Player: {facts:?}");
                for (label, value) in &facts {
                    assert!(kit::width(label) < usize::from(LABEL_W), "{code}: the label {label:?}");
                    let cells = kit::width(value);
                    assert!(cells <= w, "{code}, {drive}, {server}: {label} is {cells} cells: {value}");
                }
                let [drive_f, card_f, server_f, removes, plays, player] = &facts[..] else { unreachable!() };
                // The Drive fact keeps the most pieces that fit, each whole:
                // the serial gives way first, then the clusters; the drive,
                // its file system and its size stay.
                let pieces = pieces(drive);
                let joined = |k: usize| pieces[..k].join(" · ");
                let kept = (3..=5).rev().find(|&k| kit::width(&joined(k)) <= w).unwrap_or(3);
                assert_eq!(drive_f.1, joined(kept), "{code}: the Drive fact gives way a piece at a time");
                assert!(!drive_f.1.contains('…'), "{code}: {drive_f:?}");
                match drive {
                    "E:" => assert_eq!(kept, 5, "{code}: whole on a drive letter: {drive_f:?}"),
                    "MSTREAM_CAR" => assert!(kept >= 4, "{code}: a label keeps the clusters: {drive_f:?}"),
                    _ => assert_eq!(kept, 3, "{code}: the clusters gone too: {drive_f:?}"),
                }
                assert!(card_f.1.ends_with(" 0.14.0") && card_f.1.contains("4821"), "{code}: {card_f:?}");
                let more = t!("dev.card.fact_removes_v", path = "", n = "1,234").to_string();
                assert!(removes.1.ends_with(&more) && removes.1.starts_with("Velvet Static/"), "{code}: {removes:?}");
                for whole in [server_f, plays, player] {
                    assert!(!whole.1.contains('…'), "{code}: {whole:?}");
                }
                assert!(server_f.1.starts_with(server), "{code}: {server_f:?}");
                assert!(player.1.contains("mp3,flac,opus,wav"), "{code}: {player:?}");
            }
        }
    }
}

#[test]
fn under_its_gate_or_its_sheet_a_card_keeps_its_rows_and_hides_its_primary() {
    // The design draws the Card view beneath a modal at its own height
    // (cards 05 and 06): the modal covers the card but the two cells at
    // each side, where a taller modal shows the card's corners beside it.
    // The card's primary keeps its rows and isn't drawn, so a modal shorter
    // than the card leaves no half a button below it.
    let _en = english();
    let now = now();
    let many: Vec<String> = (0..214).map(|i| format!("Velvet Static/Harbour Songs/{i:03} - Track.flac")).collect();
    let planned = |change: &dyn Fn(&mut worker::Planned)| {
        cards::with(cards::ready("E:", now), |v| {
            if let Next::Plan(plan) = &mut v.next {
                change(plan);
            }
        })
    };
    let removals = planned(&|p| p.removals = many.clone());
    let half = planned(&|p| {
        p.removals = many.clone();
        p.guards = vec![Guard::Removals { paths: many.clone(), strong: true }];
    });
    let adopt = planned(&|p| p.guards = vec![Guard::Adopt(vec!["a.flac".into(); 1_190])]);
    // Each with the primary at rest. A new card's Set up sheet, no library
    // offered, is shorter than the card: its button's rows are below it.
    let states: [(&str, CardView, &[KeyCode], &str); 6] = [
        ("settings", cards::ready("E:", now), &[KeyCode::Char('o')], "Sync ▸"),
        ("settings, writing", cards::ready("E:", now), &[KeyCode::Char('o'), KeyCode::Right], "Sync ▸"),
        ("removals gate", removals, &[KeyCode::Enter], "Sync ▸"),
        ("half gate", half, &[KeyCode::Enter], "Sync ▸"),
        ("adopt gate", adopt, &[KeyCode::Enter], "Sync ▸"),
        ("set up", cards::new_card("F:"), &[KeyCode::Enter], "Set up ▸"),
    ];
    // The card's left edge: 78 cells centred in 100.
    let left = (100 - CARD_W) / 2;
    let corner = |buf: &Buffer, glyph: &str| (0..buf.area.height).find(|&y| buf[(left, y)].symbol() == glyph);
    // The modal's bottom edge: a ╰ inside the card and a long rule after
    // it, where a button's frame is a dozen cells.
    let modal_bottom = |buf: &Buffer| {
        (0..buf.area.height).rev().find(|&y| {
            (left + 1..left + CARD_W - 1).any(|x| {
                let rule = |d: u16| x + d < buf.area.width && buf[(x + d, y)].symbol() == "─";
                buf[(x, y)].symbol() == "╰" && (1..=30).all(rule)
            })
        })
    };
    for (name, card, keys, primary) in states {
        for with_board in [false, true] {
            for area in [FLOOR, WINDOW] {
                let (mut page, _ends, _inbox, _fold) = if with_board {
                    with_cards(Ends::up_to_date, vec![card.clone()])
                } else {
                    with_cards(|_| {}, vec![card.clone()])
                };
                if with_board {
                    press(&mut page, KeyCode::Right);
                }
                let h = area.bottom() + 1;
                let rest = hosted_at(&mut page, 100, h, area);
                for code in keys {
                    press(&mut page, *code);
                }
                assert!(page.card.gate.is_some() || page.card.sheet.is_some(), "{name}: its modal is up");
                let modal = hosted_at(&mut page, 100, h, area);
                let all = text_of(&modal);
                assert_eq!(corner(&modal, "╭"), corner(&rest, "╭"), "{name}, {area:?}: the card's top:\n{all}");
                assert_eq!(corner(&modal, "╰"), corner(&rest, "╰"), "{name}, {area:?}: the card's bottom:\n{all}");
                assert!(text_of(&rest).contains(primary), "{name}: the primary at rest");
                assert!(!all.contains(primary), "{name}, {area:?}: no primary under the modal:\n{all}");
                // Below a modal shorter than the card, its rows show no part
                // of the button's frame.
                let end = modal_bottom(&modal).expect("the modal's bottom edge");
                let card_end = corner(&modal, "╰").expect("the card's bottom");
                for y in end + 1..card_end {
                    let row: String = (left + 1..left + CARD_W - 1).map(|x| modal[(x, y)].symbol()).collect();
                    let framed = row.chars().any(|c| matches!(c, '╭' | '╮' | '╰' | '╯'));
                    assert!(!framed, "{name}, {area:?}: half a button below the modal, row {y}:\n{all}");
                }
            }
        }
    }
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

/// The files a look left out, which no server serves (clause 44's log):
/// their count in the locale's words, then a few of their places a quiet
/// line each, the rest an ellipsis.
#[test]
fn details_log_says_the_files_left_out_then_their_places_in_every_locale() {
    use crate::device::card::plan::Hidden;
    let twin = "Rock/Artist A/Album B/._01 Song.mp3";
    let many = Hidden { n: 1_312, sample: vec!["Metal/.38 Special/".into(), twin.into()], more: true };
    let one = Hidden { n: 1, sample: vec![twin.into()], more: false };
    for (code, _) in crate::setup::LANGS {
        let _in = crate::setup::tests::in_locale(code);
        let (mut page, _ends, _inbox, fold) = with_cards(|_| {}, vec![cards::ready("E:", now())]);
        let key = fold.borrow().cards[0].key.clone();
        for hidden in [&many, &one] {
            let item = worker::LogItem::Hidden(hidden.clone());
            fold.borrow_mut().apply(worker::Event::Log { key: key.clone(), item });
        }
        page.pump();
        let lines: Vec<(String, LogKind)> =
            page.log_for(&key).iter().map(|l| (l.text.clone(), l.kind)).collect();
        let count = t!("dev.card.log_hidden", n = "1,312").to_string();
        let lone = t!("dev.card.log_hidden_one").to_string();
        assert!(count.contains("1,312") && !count.contains("dev.") && !lone.contains("dev."), "{code}");
        let place = |text: &str| (format!("  {text}"), LogKind::Quiet);
        let want = vec![
            (count, LogKind::Fact),
            place("Metal/.38 Special/"),
            place(twin),
            place("…"),
            (lone, LogKind::Fact),
            place(twin),
        ];
        assert_eq!(lines[lines.len().saturating_sub(6)..], want[..], "{code}");
    }
    // In English, in Details' log, in a window tall enough to show it whole.
    let _en = english();
    let (mut page, _ends, _inbox, fold) = with_cards(|_| {}, vec![cards::ready("E:", now())]);
    let key = fold.borrow().cards[0].key.clone();
    let item = worker::LogItem::Hidden(many);
    fold.borrow_mut().apply(worker::Event::Log { key, item });
    page.pump();
    press(&mut page, KeyCode::Char('d'));
    let tall = Rect { x: 0, y: 1, width: 100, height: 46 };
    let frame = text_of(&hosted_at(&mut page, 100, 48, tall));
    assert!(frame.contains("1,312 hidden files left out: the server can't serve them"), "{frame}");
    assert!(frame.contains("  Metal/.38 Special/"), "{frame}");
}
