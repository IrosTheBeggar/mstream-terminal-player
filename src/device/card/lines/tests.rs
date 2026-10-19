//! The cards' command line: its words on the Card view's fixtures, its
//! gates' flags, and whole commands on the transfer worker over cards in
//! host folders and the sync tests' loopback mStream. No test touches a
//! real volume, a real server or a serial port.

use std::sync::Arc;

use super::*;
use crate::device::card::engine::Guard;
use crate::device::card::engine::fatsim::FatSim;
use crate::device::card::sync::tests::{Fake, SERIAL, Scratch, card as set_up_card, server};
use crate::device::card::volume::{Fs, Table};
use crate::device::card::worker::fixtures as f;
use crate::device::card::worker::tests::{Here, Shelf, home_server, volume_at};

fn now() -> u64 {
    super::now()
}

/// The ready card, its plan changed.
fn planned(change: impl FnOnce(&mut Planned)) -> CardView {
    f::with(f::ready("E:", now()), |c| {
        if let Next::Plan(plan) = &mut c.next {
            change(plan);
        }
    })
}

fn plan_of(card: &CardView) -> Planned {
    match &card.next {
        Next::Plan(plan) => plan.as_ref().clone(),
        _ => panic!("a plan"),
    }
}

#[test]
fn to_takes_the_into_lists_codecs_and_no_other() {
    assert_eq!(codec_of("opus"), Some((TransferCodec::Opus, None)));
    assert_eq!(codec_of("opus-192"), Some((TransferCodec::Opus, Some(TransferBitrate::K192))));
    assert_eq!(codec_of("MP3-320"), Some((TransferCodec::Mp3, Some(TransferBitrate::K320))));
    assert_eq!(codec_of("mp3:128k"), Some((TransferCodec::Mp3, Some(TransferBitrate::K128))));
    assert_eq!(codec_of("opus-256"), None, "Opus goes up to 192k");
    assert_eq!(codec_of("mp3-160"), None);
    assert_eq!(codec_of("flac"), None);
}

#[test]
fn a_cards_line_says_its_drive_name_volume_server_and_next_sync_or_why_not() {
    let line = card_line(&f::ready("E:", now()));
    let want = "E: · Car music · 63.8 GB FAT32 · Home · 4,812 tracks · last sync ";
    assert!(line.starts_with(want), "{line}");
    let next = "(3 days ago) · next sync 12 new (1.9 GB), 2 to remove, 37 plays to send";
    assert!(line.contains(next), "{line}");
    let line = card_line(&f::refused("F:", Fs::ExFat, Table::Mbr));
    let why = "exFAT: the player reads FAT32 only (device format formats it in the player)";
    assert_eq!(line, format!("F: · 63.8 GB exFAT · can't be used: {why}"));
    let line = card_line(&f::new_card("G:"));
    assert_eq!(line, "G: · 63.8 GB FAT32 · new card, not set up: setup sets it up");
    let unknown = f::with(f::ready("E:", now()), |c| {
        c.server = Server::Unknown("Work".into());
        c.next = Next::Unasked;
    });
    let line = card_line(&unknown);
    assert!(line.contains("filled from Work, which this computer has no connection to"), "{line}");
    assert!(line.ends_with("next sync not asked: no server to ask"), "{line}");
    let copied = f::with(f::ready("E:", now()), |c| f::facts(c).copied = true);
    assert!(card_line(&copied).ends_with("a copy of another card: setup --new-identity gives it its own"));
}

#[test]
fn status_prints_the_card_views_facts_as_key_value_rows() {
    let rows = facts_rows(&f::ready("E:", now()));
    let get = |key: &str| rows.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()).unwrap_or_default();
    assert_eq!(get("drive"), "E:");
    assert_eq!(get("volume"), "63.8 GB FAT32 · 22.6 GB free · 32 KB clusters");
    assert_eq!(get("name"), "Car music");
    assert_eq!(get("card"), "id 5eedc0de0a1b2c3d · revision 42 · mstream-terminal 0.14.0");
    assert_eq!(get("server"), "Home");
    assert_eq!(get("holds"), "Rock, Jazz");
    assert_eq!(get("tracks"), "4,812");
    assert_eq!(get("next sync"), "12 new (1.9 GB), 2 to remove, 37 plays to send");
    assert_eq!(get("fit"), "43.2 of 63.8 GB after this sync · 20.6 GB spare");
    assert_eq!(get("removes"), "Velvet Static/Harbour Songs/04 - Low Tide.flac and 1 more");
    assert_eq!(get("converts"), "only what the player can't play, to Opus 192k");
    assert_eq!(get("removals"), "follow Home");
    assert_eq!(get("player"), "Core2 0.12.0: mp3,flac,opus (its device.txt)");
    // Every key fits its column: one space at least before its value.
    assert!(rows.iter().all(|(k, _)| k.len() < KEY_W), "{rows:?}");
}

#[test]
fn a_drive_is_its_letter_its_path_or_its_cards_name_when_one_card_has_it() {
    let cards = vec![f::ready("E:", now()), f::new_card("G:")];
    for asked in ["E", "e:", "E:\\", "E:/", "Car music", "car MUSIC"] {
        assert_eq!(pick(&cards, asked), Ok("E:\\".to_string()), "{asked}");
    }
    assert_eq!(pick(&cards, "g:"), Ok("G:\\".to_string()));
    let missing = pick(&cards, "H:").unwrap_err();
    assert!(missing.contains("isn't a card in a reader here (these are: E:, G:)"), "{missing}");
    let twins = vec![f::ready("E:", now()), f::ready("F:", now())];
    let both = pick(&twins, "Car music").unwrap_err();
    assert!(both.contains("2 cards are called Car music: name one by its drive (E:, F:)"), "{both}");
    assert!(pick(&[], "E:").unwrap_err().contains("no card in a reader"));
}

#[test]
fn each_gate_is_a_flag_that_names_what_it_lets_through() {
    let many: Vec<String> = (0..214).map(|i| format!("Velvet Static/Harbour Songs/{i:03}.flac")).collect();
    // More than 50 removals: the plan's exact count, and no other.
    let card = planned(|p| p.removals = many[..60].to_vec());
    let plan = plan_of(&card);
    let refused = pass_gates(&card, &plan, &Flags::default()).unwrap_err().join("\n");
    assert!(refused.contains("Home no longer has 60 of Car music's tracks"), "{refused}");
    assert!(refused.contains("--allow-removals 60 lets them go (--list-removals lists them)"), "{refused}");
    let wrong = Flags { allow_removals: Some(59), ..Flags::default() };
    let refused = pass_gates(&card, &plan, &wrong).unwrap_err().join("\n");
    assert!(refused.contains("--allow-removals 59 isn't this plan's count (60)"), "{refused}");
    let right = Flags { allow_removals: Some(60), ..Flags::default() };
    let allow = pass_gates(&card, &plan, &right).expect("its count lets them go");
    assert!(allow.answers.removals && !allow.answers.mass_removals);

    // Half the card: the same flag, the stronger gate's words.
    let card = planned(|p| {
        p.removals = many.clone();
        p.guards = vec![Guard::Removals { paths: many.clone(), strong: true }];
    });
    let plan = plan_of(&card);
    let refused = pass_gates(&card, &plan, &Flags::default()).unwrap_err().join("\n");
    assert!(refused.contains("removes 214 of Car music's 4,812 tracks: half the card or more"), "{refused}");
    assert!(refused.contains("check Home first"), "{refused}");
    let right = Flags { allow_removals: Some(214), ..Flags::default() };
    let allow = pass_gates(&card, &plan, &right).expect("the stronger gate's count");
    assert!(allow.answers.removals && allow.answers.mass_removals);

    // Adopting: --adopt or --keep-mine, never a bare yes.
    let card = planned(|p| p.guards = vec![Guard::Adopt(vec!["a.flac".into(); 3])]);
    let plan = plan_of(&card);
    let refused = pass_gates(&card, &plan, &Flags { yes: true, ..Flags::default() }).unwrap_err().join("\n");
    assert!(refused.contains("--adopt adopts the 3, --keep-mine keeps them all yours"), "{refused}");
    let adopt = pass_gates(&card, &plan, &Flags { adopt: true, ..Flags::default() }).unwrap();
    assert_eq!(adopt.answers.adopt, Some(true));
    let mine = pass_gates(&card, &plan, &Flags { keep_mine: true, ..Flags::default() }).unwrap();
    assert_eq!(mine.answers.adopt, Some(false));

    // A peer only its parent reaches: --through-parent.
    let card = planned(|p| p.ask_proxy = vec![4]);
    let plan = plan_of(&card);
    let refused = pass_gates(&card, &plan, &Flags::default()).unwrap_err().join("\n");
    assert!(refused.contains("--through-parent copies through the parent"), "{refused}");
    let through = pass_gates(&card, &plan, &Flags { through_parent: true, ..Flags::default() }).unwrap();
    assert_eq!(through.proxy_copies, vec![4]);

    // Two removals ask nothing; a count given must still be the plan's.
    let card = f::ready("E:", now());
    let plan = plan_of(&card);
    assert_eq!(pass_gates(&card, &plan, &Flags::default()), Ok(Allow::default()));
    let off = Flags { allow_removals: Some(5), ..Flags::default() };
    assert!(pass_gates(&card, &plan, &off).unwrap_err()[0].contains("this sync removes 2, not 5"));
}

// ── Whole commands, on the worker ───────────────────────────────────────────

/// The terminal of a test: what was printed, and no one to ask unless
/// `answer` says.
struct Term {
    out: Vec<u8>,
    err: Vec<u8>,
    ctrl_c: AtomicU8,
}

impl Term {
    fn new() -> Term {
        Term { out: Vec::new(), err: Vec::new(), ctrl_c: AtomicU8::new(0) }
    }

    fn said(&self) -> String {
        String::from_utf8_lossy(&self.out).to_string() + &String::from_utf8_lossy(&self.err)
    }
}

/// A shelf with the sync tests' set-up card ("Car") and, beside it, a
/// fresh FAT card; this computer, the fake server as its saved Home.
struct Bench {
    shelf: Arc<Shelf>,
    here: Arc<Here>,
    home: ServerLink,
    /// The set-up card's key, and the fresh one's.
    key: String,
    fresh: String,
    sim: FatSim,
    _server: Fake,
}

fn bench(scratch: &Scratch) -> Bench {
    let sim = set_up_card(scratch);
    let (base, fake) = server();
    let shelf = Shelf::default();
    let found = volume_at(&scratch.0.join("card"), Fs::Fat32, SERIAL);
    let key = worker::key_of(&found);
    shelf.put(found, SERIAL);
    let fresh_dir = scratch.0.join("fresh");
    std::fs::create_dir_all(&fresh_dir).unwrap();
    let _ = FatSim::new(&fresh_dir, 0x0BAD_F00D);
    let fresh = volume_at(&fresh_dir, Fs::Fat32, 0x0BAD_F00D);
    let fresh_key = worker::key_of(&fresh);
    shelf.put(fresh, 0x0BAD_F00D);
    let (shelf, here) = (Arc::new(shelf), Arc::new(Here::default()));
    Bench { shelf, here, home: home_server(&base), key, fresh: fresh_key, sim, _server: fake }
}

impl Bench {
    /// What a command's worker runs on.
    fn setup(&self) -> Setup {
        Setup { volumes: self.shelf.clone(), home: self.here.clone(), every: Duration::from_millis(40) }
    }

    /// One command, its exit code; `answer` the listener's to the question
    /// (None: no terminal).
    fn run(
        &self,
        drive: Option<&str>,
        what: What,
        flags: &Flags,
        answer: Option<bool>,
        term: &mut Term,
    ) -> i32 {
        command(self.setup(), &self.home, drive, what, flags, answer, term)
    }
}

/// One command on the shelf, its exit code and its lines; `answer` the
/// listener's to the question (None: no terminal).
fn command(
    setup: Setup,
    home: &ServerLink,
    drive: Option<&str>,
    what: What,
    flags: &Flags,
    answer: Option<bool>,
    term: &mut Term,
) -> i32 {
    let mut ask = |_: &str| answer;
    let ctrl_c = AtomicU8::new(term.ctrl_c.load(Ordering::SeqCst));
    let mut io = Io { out: &mut term.out, err: &mut term.err, ask: &mut ask, ctrl_c: &ctrl_c };
    match drive {
        None => cards(setup, vec![home.clone()], &mut io),
        Some(drive) => card(setup, vec![home.clone()], drive, &what, flags, &mut io),
    }
}

#[test]
fn cards_lists_every_card_status_reads_and_sync_asks_before_it_writes() {
    let scratch = Scratch::new("lines-sync");
    let b = bench(&scratch);
    let (key, fresh_key) = (b.key.clone(), b.fresh.clone());
    let mut term = Term::new();
    let code = b.run(None, What::Status, &Flags::default(), None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    let line = " · Home · no tracks · last sync never · next sync ";
    assert!(said.contains(" · Car · ") && said.contains(line), "{said}");
    assert!(said.contains("new card, not set up: setup sets it up"), "{said}");

    // status writes nothing, and names its rows.
    let before = b.sim.image();
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Status, &Flags::default(), None, &mut term);
    assert_eq!(code, 0, "{}", term.said());
    assert!(term.said().contains("name        Car\n"), "{}", term.said());
    assert!(term.said().contains("server      Home\n"), "{}", term.said());

    // sync off a terminal without --yes: the plan, then nothing written.
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Sync, &Flags::default(), None, &mut term);
    let said = term.said();
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("from Home: ") && said.contains("Nothing written: add --yes to sync."), "{said}");
    // And on a terminal, its no.
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Sync, &Flags::default(), Some(false), &mut term);
    assert_eq!(code, 1, "{}", term.said());
    assert_eq!(b.sim.image(), before, "nothing written");

    // --yes: the run's steps, then done.
    let mut term = Term::new();
    let yes = Flags { yes: true, ..Flags::default() };
    let code = b.run(Some(&key), What::Sync, &yes, None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("listing Home's libraries…"), "{said}");
    let done = "verified. Eject it before you take it out.";
    assert!(said.contains("Car synced in ") && said.contains(done), "{said}");
    assert_ne!(b.sim.image(), before, "the card was written");

    // eject: the OS asked.
    let mut term = Term::new();
    let code = b.run(Some("Car"), What::Eject, &Flags::default(), None, &mut term);
    assert_eq!(code, 0, "{}", term.said());
    assert!(term.said().contains("(Car) can be taken out."), "{}", term.said());
    assert_eq!(b.shelf.ejected.lock().unwrap().clone(), vec![key.clone()]);
    let _ = fresh_key;
}

#[test]
fn setup_names_a_new_card_and_a_set_up_cards_flags_change_its_settings() {
    let scratch = Scratch::new("lines-setup");
    let b = bench(&scratch);
    let (key, fresh_key) = (b.key.clone(), b.fresh.clone());
    // A new card needs a name; its volume has one ("CARD") here.
    let mut term = Term::new();
    let flags = Flags { yes: true, name: Some("Gym".into()), ..Flags::default() };
    let code = b.run(Some(&fresh_key), What::Setup, &flags, None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("as \"Gym\": /music, its id and name; nothing else on it is touched."), "{said}");
    assert!(said.contains("  from        Home: every library it lists"), "{said}");
    assert!(said.contains("is set up: device card "), "{said}");
    let mut term = Term::new();
    let code = b.run(Some("Gym"), What::Status, &Flags::default(), None, &mut term);
    assert_eq!(code, 0, "{}", term.said());
    assert!(term.said().contains("name        Gym\n"), "{}", term.said());

    // A set-up card: no flag, nothing to change; a flag, its settings.
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Setup, &Flags::default(), None, &mut term);
    assert_eq!(code, 0);
    assert!(term.said().contains("is set up already"), "{}", term.said());
    let mut term = Term::new();
    let flags = Flags { yes: true, removals: Some(Removal::Never), covers: Some(false), ..Flags::default() };
    let code = b.run(Some(&key), What::Setup, &flags, None, &mut term);
    assert_eq!(code, 0, "{}", term.said());
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Status, &Flags::default(), None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("removals    never\n") && said.contains("covers      off\n"), "{said}");

    // A name that is an address is refused, as the sheet refuses it.
    let mut term = Term::new();
    let flags = Flags { yes: true, name: Some("music.example.com".into()), ..Flags::default() };
    let code = b.run(Some(&key), What::Setup, &flags, None, &mut term);
    assert_eq!(code, 1, "{}", term.said());
    assert!(term.said().contains("a name, never an address"), "{}", term.said());
}

#[test]
fn ctrl_c_stops_the_sync_at_its_next_save_and_says_run_it_again() {
    let scratch = Scratch::new("lines-stop");
    let b = bench(&scratch);
    let key = b.key.clone();
    let mut term = Term::new();
    term.ctrl_c.store(1, Ordering::SeqCst);
    let yes = Flags { yes: true, ..Flags::default() };
    let code = b.run(Some(&key), What::Sync, &yes, None, &mut term);
    let said = term.said();
    assert!(said.contains("stopping Car at the next save…"), "{said}");
    // Stopped before it wrote, or at a save: 3, run it again; a run quick
    // enough to finish first is done.
    assert!(code == 3 || code == 0, "{code}: {said}");
}

#[test]
fn a_drive_that_is_no_card_is_exit_two() {
    let scratch = Scratch::new("lines-none");
    let b = bench(&scratch);
    let mut term = Term::new();
    let code = b.run(Some("Q:"), What::Status, &Flags::default(), None, &mut term);
    assert_eq!(code, 2);
    assert!(term.said().contains("Q: isn't a card in a reader here"), "{}", term.said());
    // No card at all: `cards` says so, exit 1.
    let every = Duration::from_millis(40);
    let empty = Setup { volumes: Arc::new(Shelf::default()), home: b.here.clone(), every };
    let mut term = Term::new();
    let code = command(empty, &b.home, None, What::Status, &Flags::default(), None, &mut term);
    assert_eq!((code, term.said()), (1, "No card in a reader.\n".to_string()));
}

/// One `device card` command with these servers saved; `answer` the
/// listener's to the question (None: no terminal).
fn command_on(
    setup: Setup,
    servers: Vec<ServerLink>,
    drive: &str,
    flags: &Flags,
    answer: Option<bool>,
    term: &mut Term,
) -> i32 {
    let mut ask = |_: &str| answer;
    let ctrl_c = AtomicU8::new(0);
    let mut io = Io { out: &mut term.out, err: &mut term.err, ask: &mut ask, ctrl_c: &ctrl_c };
    card(setup, servers, drive, &What::Sync, flags, &mut io)
}

#[test]
fn a_take_over_is_planned_with_nothing_written_until_the_listener_goes_on() {
    let scratch = Scratch::new("lines-take-over");
    let b = bench(&scratch);
    let key = b.key.clone();
    let (base, _studio_server) = server();
    let studio = ServerLink { name: "Studio".into(), session: false, ..home_server(&base) };
    let both = || vec![b.home.clone(), studio.clone()];
    let to_studio = Flags { server: Some("Studio".into()), take_over: true, ..Flags::default() };

    // A new card: Studio's plan, then nothing written without --yes, nor
    // on a terminal's no.
    let before = b.sim.image();
    let mut term = Term::new();
    let code = command_on(b.setup(), both(), &key, &to_studio, None, &mut term);
    let said = term.said();
    assert_eq!(code, 1, "{said}");
    let nothing = "Nothing written: add --yes to sync.";
    assert!(said.contains("from Studio: ") && said.contains(nothing), "{said}");
    let mut term = Term::new();
    let code = command_on(b.setup(), both(), &key, &to_studio, Some(false), &mut term);
    assert_eq!(code, 1, "{}", term.said());
    assert_eq!(b.sim.image(), before, "state.json untouched: nothing written");

    // A filled card: the gate's flag, then its plan, still nothing written.
    let mut term = Term::new();
    let yes = Flags { yes: true, ..Flags::default() };
    let code = command_on(b.setup(), both(), &key, &yes, None, &mut term);
    assert_eq!(code, 0, "filled from Home: {}", term.said());
    let filled = b.sim.image();
    let mut term = Term::new();
    let bare = Flags { server: Some("Studio".into()), ..Flags::default() };
    let code = command_on(b.setup(), both(), &key, &bare, None, &mut term);
    assert_eq!(code, 1, "{}", term.said());
    let flag = "--server Studio --take-over makes Studio its server";
    assert!(term.said().contains(flag), "{}", term.said());
    let mut term = Term::new();
    let code = command_on(b.setup(), both(), &key, &to_studio, None, &mut term);
    let said = term.said();
    assert_eq!(code, 1, "{said}");
    assert!(said.contains("Nothing written"), "{said}");
    assert_eq!(b.sim.image(), filled, "the take-over waits for --yes");
}

#[test]
fn a_new_cards_server_chosen_on_the_command_line_is_written_then_synced_from() {
    let scratch = Scratch::new("lines-other-server");
    let b = bench(&scratch);
    let key = b.key.clone();
    let (base, _studio_server) = server();
    let studio = ServerLink { name: "Studio".into(), session: false, ..home_server(&base) };
    let both = || vec![b.home.clone(), studio.clone()];
    // No tracks yet: no gate, but the same order: the plan, then the yes,
    // then the settings, then the sync.
    let mut term = Term::new();
    let flags = Flags { yes: true, server: Some("Studio".into()), ..Flags::default() };
    let code = command_on(b.setup(), both(), &key, &flags, None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("from Studio: ") && said.contains("listing Studio's libraries…"), "{said}");
    assert!(!said.contains("waiting for the sync under way"), "{said}");
    let mut term = Term::new();
    let mut ask = |_: &str| None;
    let ctrl_c = AtomicU8::new(0);
    let mut io = Io { out: &mut term.out, err: &mut term.err, ask: &mut ask, ctrl_c: &ctrl_c };
    let code = card(b.setup(), both(), &key, &What::Status, &Flags::default(), &mut io);
    assert_eq!(code, 0, "{}", term.said());
    assert!(term.said().contains("server      Studio\n"), "{}", term.said());
}
