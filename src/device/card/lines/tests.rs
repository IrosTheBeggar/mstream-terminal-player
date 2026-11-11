//! The cards' command line: its words on the Card view's fixtures, its
//! gates' flags, and whole commands on the transfer worker over cards in
//! host folders and the sync tests' loopback mStream. No test touches a
//! real volume, a real server or a serial port.

use std::sync::Arc;

use super::*;
use crate::device::card::engine::Guard;
use crate::device::card::engine::fatsim::FatSim;
use crate::device::card::sync::tests::{
    Fake, SERIAL, Scratch, Wave, card as set_up_card, envs, logged, peaks_of, rock, server, sync_now,
    with_hidden, with_moods,
};
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
    let _en = crate::setup::tests::in_locale("en");
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
fn a_plan_says_the_waveforms_it_adds_and_the_run_its_phase() {
    let alone = Planned { waves_missing: true, ..Planned::default() };
    assert_eq!(plan_words(&alone, "Home"), "waveforms to add");
    let more = plan_of(&planned(|p| p.waves_missing = true));
    let all = "12 new (1.9 GB), 2 to remove, 37 plays to send, waveforms to add";
    assert_eq!(plan_words(&more, "Home"), all);
    assert_eq!(plan_words(&Planned::default(), "Home"), "nothing new on Home");
    assert_eq!(phase_words(Phase::Waves, 0, 0, "Home"), "asking Home for waveforms…");
}

#[test]
fn status_prints_the_card_views_facts_as_key_value_rows() {
    // It reads a German row too, so in a process of its own.
    let Some(_) = crate::setup::tests::in_own_process(&["en"]) else { return };
    let en = crate::setup::tests::in_locale("en");
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
    assert_eq!(get("player"), "Core2 0.14.0: mp3,flac,opus (its device.txt)");
    // Every key fits its column: one space at least before its value.
    assert!(rows.iter().all(|(k, _)| k.len() < KEY_W), "{rows:?}");
    drop(en);
    // The card's sizes are the Card view's words, so a locale set would
    // reach them too; the command line sets none, and prints the English.
    let _de = crate::setup::tests::in_locale("de");
    let rows = facts_rows(&f::ready("E:", now()));
    let volume = rows.iter().find(|(k, _)| *k == "volume").map(|(_, v)| v.clone()).unwrap_or_default();
    assert_eq!(volume, "63.8 GB FAT32 · 22.6 GB frei · 32-KB-Cluster");
}

#[test]
fn a_drive_is_its_letter_its_path_or_its_cards_name_when_one_card_has_it() {
    let cards = vec![f::ready("E:", now()), f::new_card("G:")];
    for asked in ["E", "e:", "E:\\", "E:/", "Car music", "car MUSIC"] {
        assert_eq!(pick(&cards, asked), Ok(cards[0].key.clone()), "{asked}");
    }
    assert_eq!(pick(&cards, "g:"), Ok(cards[1].key.clone()));
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
    /// The fake server, its saved Home.
    server: Fake,
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
    Bench { shelf, here, home: home_server(&base), key, fresh: fresh_key, sim, server: fake }
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

/// `plays` on a card for someone else (clause 61, `--for someone-else`):
/// none is sent and the line says why, where the same card, the listener's
/// own, plans one. Made the listener's own again (`--for me`), the card has
/// nothing to send of what its log held: only what is played after goes.
#[test]
fn plays_on_a_card_for_someone_else_sends_none_and_says_why() {
    let scratch = Scratch::new("lines-gift");
    let b = bench(&scratch);
    let key = b.key.clone();
    let base = b.home.base.clone().expect("the fake server's address");
    let synced = sync_now(&b.sim, &base, &rock(), &mut envs()).0;
    assert!(matches!(synced.end, crate::device::card::sync::End::Ran(_)), "{:?}", synced.end);
    let play = logged(&b.sim, "Album B/01.mp3", 1, 1_791_950_000_000);
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), &play).unwrap();
    let yes = Flags { yes: true, ..Flags::default() };
    let posts = || b.server.lock().unwrap().plays.len();

    // The listener's own: a play to send, and off a terminal the plan alone.
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Plays, &Flags::default(), None, &mut term);
    assert_eq!(code, 1, "{}", term.said());
    assert!(term.said().contains("Send 1 plays from "), "{}", term.said());

    // For someone else: none goes, with --yes or without.
    let mut term = Term::new();
    let gift = Flags { for_someone_else: Some(true), ..yes.clone() };
    assert_eq!(b.run(Some(&key), What::Setup, &gift, None, &mut term), 0, "{}", term.said());
    for flags in [&yes, &Flags::default()] {
        let mut term = Term::new();
        let code = b.run(Some(&key), What::Plays, flags, None, &mut term);
        let said = term.said();
        assert_eq!(code, 0, "{said}");
        assert!(said.contains(" is a card for someone else: its plays aren't sent."), "{said}");
        assert!(!said.contains("plays sent to") && !said.contains("Send "), "{said}");
    }
    let mut term = Term::new();
    assert_eq!(b.run(Some(&key), What::Status, &Flags::default(), None, &mut term), 0);
    let said = term.said();
    assert!(said.contains("for         someone else\n") && !said.contains("plays to send"), "{said}");
    assert_eq!(posts(), 0, "nothing posted");

    // The listener's own again: what its log held was settled unsent as
    // the setting went, and nothing is left to send.
    let mut term = Term::new();
    let mine = Flags { for_someone_else: Some(false), ..yes.clone() };
    assert_eq!(b.run(Some(&key), What::Setup, &mine, None, &mut term), 0, "{}", term.said());
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Plays, &yes, None, &mut term);
    assert_eq!(code, 0, "{}", term.said());
    assert!(term.said().contains("No plays to send to Home."), "{}", term.said());
    assert_eq!(posts(), 0, "nothing of that time, ever");

    // A play of the listener's own, after that, goes.
    let more = [play, logged(&b.sim, "Album B/01.mp3", 2, 1_791_950_400_000)].concat();
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), more).unwrap();
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Plays, &yes, None, &mut term);
    assert_eq!(code, 0, "{}", term.said());
    assert!(term.said().contains("1 plays sent to Home."), "{}", term.said());
    assert_eq!(posts(), 1);
}

/// `plays` and a play the server doesn't know (clause 61): the play waits,
/// and the lines say so beside a count of none sent; once the server knows
/// the track, it goes.
#[test]
fn plays_says_which_wait_for_a_server_that_doesnt_know_their_tracks() {
    let scratch = Scratch::new("lines-unknown");
    let b = bench(&scratch);
    let key = b.key.clone();
    let base = b.home.base.clone().expect("the fake server's address");
    let synced = sync_now(&b.sim, &base, &rock(), &mut envs()).0;
    assert!(matches!(synced.end, crate::device::card::sync::End::Ran(_)), "{:?}", synced.end);
    let play = logged(&b.sim, "Album B/01.mp3", 1, 1_791_850_000_000);
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), play).unwrap();
    // The server's scan hasn't reached the track, and its listing says a
    // scan is running: the listing the command reads lacks the path, and
    // says nothing of the file being gone.
    {
        let mut lib = b.server.lock().unwrap();
        lib.unindexed.insert("Rock/Artist A/Album B/01.mp3".into());
        lib.scanning = true;
    }
    let yes = Flags { yes: true, ..Flags::default() };
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Plays, &yes, None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    let waits = "  1 play waits: the server doesn't know its track, and no listing says it is gone \
                 (a scan may be under way)\n";
    assert!(said.contains(waits), "{said}");
    assert!(said.contains("0 plays sent to Home."), "{said}");

    {
        let mut lib = b.server.lock().unwrap();
        lib.unindexed.clear();
        lib.scanning = false;
    }
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Plays, &yes, None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("1 plays sent to Home.") && !said.contains("waits"), "{said}");
}

/// `plays` to a server that takes none (no Stats API): the job's own words
/// say why none went, beside the count of none sent.
#[test]
fn plays_says_a_server_takes_none_when_it_has_no_stats_api() {
    let scratch = Scratch::new("lines-no-stats");
    let b = bench(&scratch);
    let key = b.key.clone();
    let base = b.home.base.clone().expect("the fake server's address");
    let synced = sync_now(&b.sim, &base, &rock(), &mut envs()).0;
    assert!(matches!(synced.end, crate::device::card::sync::End::Ran(_)), "{:?}", synced.end);
    let play = logged(&b.sim, "Album B/01.mp3", 1, 1_791_850_000_000);
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), play).unwrap();
    b.server.lock().unwrap().no_stats = true;
    let yes = Flags { yes: true, ..Flags::default() };
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Plays, &yes, None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    let why = "  the server takes no plays (it has no Stats API): they wait\n";
    assert!(said.contains(why) && said.contains("0 plays sent to Home."), "{said}");
    assert!(b.server.lock().unwrap().plays.is_empty());
}

/// The plays from before the player's clock was set are counted and never
/// sent (clause 61): `plays` and `sync` say how many the card holds, and
/// name `device time`, the command line's way to set the clock. A card
/// with none says neither line.
#[test]
fn plays_and_sync_count_the_undated_plays_and_name_the_command_that_sets_the_clock() {
    let scratch = Scratch::new("lines-undated");
    let b = bench(&scratch);
    let key = b.key.clone();
    let base = b.home.base.clone().expect("the fake server's address");
    let synced = sync_now(&b.sim, &base, &rock(), &mut envs()).0;
    assert!(matches!(synced.end, crate::device::card::sync::End::Ran(_)), "{:?}", synced.end);
    let count = "from before the player's clock was set: counted, not sent.";
    let how = "  device time <port> sets the clock of a player on USB (device list names its port).\n";
    let run = |what: What| {
        let mut term = Term::new();
        let code = b.run(Some(&key), what, &Flags::default(), None, &mut term);
        (code, term.said())
    };
    // Every play dated: neither line.
    let dated = logged(&b.sim, "Album B/01.mp3", 1, 1_791_850_000_000);
    let plays_file = scratch.0.join("card/.player/plays.jnl");
    std::fs::write(&plays_file, &dated).unwrap();
    for what in [What::Plays, What::Sync] {
        let (_, said) = run(what);
        assert!(!said.contains(count) && !said.contains("device time"), "{said}");
    }
    // Two with no start, and none after them in their boot to date them by.
    let undated = [logged(&b.sim, "Album B/01.mp3", 1, 0), logged(&b.sim, "Album B/02.flac", 2, 0)];
    std::fs::write(&plays_file, undated.concat()).unwrap();
    let (code, plays) = run(What::Plays);
    assert_eq!(code, 0, "{plays}");
    let two = format!("  2 plays are {count}\n{how}");
    assert!(plays.contains(&two) && plays.contains("No plays to send to Home."), "{plays}");
    let (code, sync) = run(What::Sync);
    assert_eq!(code, 1, "the plan alone, off a terminal: {sync}");
    assert!(sync.contains(&two), "{sync}");
    assert!(b.server.lock().unwrap().plays.is_empty(), "none sent");
}

/// The Card view's own words for the plays from before the clock was set
/// (`plays_old`) promise what happens (clause 61): counted, and not sent.
/// They said "kept", and nothing keeps them.
#[test]
fn the_card_views_words_for_undated_plays_say_counted_not_kept() {
    let _en = crate::setup::tests::in_locale("en");
    let words = t!("dev.card.plays_old", n = 2).to_string();
    assert_eq!(words, "2 from before the player's clock was set: counted, not sent");
}

/// The files the server lists and can't serve (a dot part past the vpath)
/// are left out of the sync: the plan says how many and where, once, before
/// the question (a look that writes nothing says it too); none says
/// skipped, nothing is asked of them, and the run ends clean.
#[test]
fn a_sync_leaves_the_files_the_server_cant_serve_out_and_says_so_once() {
    let scratch = Scratch::new("lines-hidden");
    let b = bench(&scratch);
    let hidden = with_hidden(&b.server);
    let key = b.key.clone();
    let told = "\n  2 hidden files left out: the server can't serve them \
                (a name or a folder starting with a dot)\n    Rock/Artist A/.hidden/\n    \
                Rock/Artist A/Album B/._01.mp3\n";

    // Off a terminal without --yes: the plan and its files left out.
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Sync, &Flags::default(), None, &mut term);
    let said = term.said();
    assert_eq!(code, 1, "{said}");
    let plan = said.find(" from Home: ").expect("the plan");
    let (at, nothing) = (said.find(told), said.find("Nothing written: add --yes to sync."));
    assert!(at.is_some_and(|at| at > plan) && nothing > at, "{said}");

    let mut term = Term::new();
    let yes = Flags { yes: true, ..Flags::default() };
    let code = b.run(Some(&key), What::Sync, &yes, None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("Car synced in ") && said.contains("verified."), "{said}");
    assert_eq!(said.matches(told).count(), 1, "{said}");
    assert_eq!(said.matches("hidden files").count(), 1, "said once: {said}");
    assert!(!said.contains("skipped"), "{said}");
    let asked = b.server.lock().unwrap().asked.clone();
    for path in hidden {
        assert!(!asked.iter().any(|a| a.contains(path)), "{path}: {asked:?}");
    }
}

/// A card in step that lacks its waveforms: a line says so, the plan is that
/// alone, `--yes` adds them with their phase and how far they have come, and
/// the card is in step after.
#[test]
fn a_sync_adds_the_waveforms_a_card_lacks_and_says_how_far_it_has_come() {
    let scratch = Scratch::new("lines-waves");
    let b = bench(&scratch);
    let key = b.key.clone();
    let base = b.home.base.clone().unwrap();
    let off = crate::device::card::sync::Ask { no_waves: true, ..Default::default() };
    // A card of its own, whose run.lock goes with it.
    let sim = FatSim::new(&scratch.0.join("card"), SERIAL);
    let synced = sync_now(&sim, &base, &off, &mut envs()).0;
    assert!(matches!(synced.end, crate::device::card::sync::End::Ran(_)), "{:?}", synced.end);
    drop(sim);
    {
        // Slow enough to be told: a third of a second each, three at once.
        let mut lib = b.server.lock().unwrap();
        let paths: Vec<String> = lib.tracks.iter().map(|t| t.path.clone()).collect();
        for path in paths {
            let peaks = Box::new(Wave::Peaks(peaks_of(&path)));
            lib.waves.insert(path, vec![Wave::Slow(Duration::from_millis(350), peaks)]);
        }
    }
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Status, &Flags::default(), None, &mut term);
    assert_eq!(code, 0, "{}", term.said());
    assert!(term.said().contains("next sync   waveforms to add\n"), "{}", term.said());

    let mut term = Term::new();
    let yes = Flags { yes: true, ..Flags::default() };
    let code = b.run(Some(&key), What::Sync, &yes, None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    assert!(said.contains(" from Home: waveforms to add.\n"), "{said}");
    assert!(said.contains("asking Home for waveforms…\n"), "{said}");
    assert!(said.contains(" of 5 waveforms\n"), "{said}");
    assert!(said.contains("Car synced in ") && said.contains("verified."), "{said}");

    // In step: the look the sync's worker left behind holds no real run's
    // place, and keeps no other look off the card.
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Status, &Flags::default(), None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("next sync   nothing new on Home\n"), "{said}");
}

/// The mood scores (clause 51b): a sync from a server that serves them asks
/// once and prints the line Details' log has of them, as it prints the
/// run's other notes.
#[test]
fn a_sync_says_the_mood_scores_it_brought() {
    let scratch = Scratch::new("lines-moods");
    let b = bench(&scratch);
    let key = b.key.clone();
    with_moods(&b.server);
    let yes = Flags { yes: true, ..Flags::default() };
    let mut term = Term::new();
    let code = b.run(Some(&key), What::Sync, &yes, None, &mut term);
    let said = term.said();
    assert_eq!(code, 0, "{said}");
    let line = "\n  mood scores for 5 of the card's 5 tracks from the server\n";
    assert!(said.contains(line), "{said}");
    assert_eq!(b.server.lock().unwrap().mood_calls.len(), 1, "one call");
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
