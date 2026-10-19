//! The transfer worker on cards in host folders ([`FatSim`], set up by the
//! set-up) and the sync tests' loopback mStream: what it reads and asks
//! when a card is put in, one sync at a time and its progress, the stop,
//! the gates, the settings, hiding and ejecting. No test touches a real
//! volume, a real server or a serial port.

use std::sync::Mutex;
use std::sync::mpsc::Receiver;

use super::*;
use crate::device::card::engine::fatsim::FatSim;
use crate::device::card::history::tests::MemStore;
use crate::device::card::sync::tests::{Fake, SERIAL, Scratch, card, no_peers, server};
use crate::device::card::volume::{Bus, Clock, Disk, Fs, Serial, Table, Volume};

/// The volumes of the test: host folders, each a FAT card at a serial.
#[derive(Default)]
pub(crate) struct Shelf {
    cards: Mutex<Vec<(Found, u32)>>,
    pub(crate) ejected: Mutex<Vec<String>>,
}

impl Shelf {
    pub(crate) fn put(&self, found: Found, serial: u32) {
        self.cards.lock().unwrap().push((found, serial));
    }

    fn take(&self, key: &str) {
        self.cards.lock().unwrap().retain(|(f, _)| key_of(f) != key);
    }
}

impl Volumes for Shelf {
    fn find(&self) -> io::Result<Vec<Found>> {
        Ok(self.cards.lock().unwrap().iter().map(|(f, _)| f.clone()).collect())
    }

    fn open(&self, found: &Found) -> Option<Box<dyn CardWrite>> {
        let cards = self.cards.lock().unwrap();
        let (_, serial) = cards.iter().find(|(f, _)| key_of(f) == key_of(found))?;
        let card = || Box::new(FatSim::new(&found.volume.path, *serial)) as Box<dyn CardWrite>;
        found.accepted().then(card)
    }

    fn eject(&self, found: &Found) -> Result<(), String> {
        self.ejected.lock().unwrap().push(key_of(found));
        Ok(())
    }
}

/// This computer, in memory: its cards.json, a fixed clock, and ids that
/// never repeat from one session to the next.
#[derive(Default)]
pub(crate) struct Here {
    cards: Mutex<Cards>,
    seed: std::sync::atomic::AtomicU64,
}

/// Each session's clock and ids from `seed`: a fixed time.
fn envs_from(seed: u64) -> impl FnMut() -> Env {
    let mut seed = seed;
    move || {
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut n = seed;
        let id = move || {
            n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            n
        };
        let producer = "mstream-terminal test".into();
        Env { now: Box::new(|| 1_791_900_000), id: Box::new(id), producer }
    }
}

impl Home for Here {
    fn cards(&self) -> Cards {
        self.cards.lock().unwrap().clone()
    }

    fn save_cards(&self, cards: &Cards) -> Result<(), String> {
        *self.cards.lock().unwrap() = cards.clone();
        Ok(())
    }

    fn plays(&self) -> Rc<dyn history::Store> {
        Rc::new(MemStore::default())
    }

    fn federation(&self, _cards: Vec<PathBuf>) -> Result<Federation, String> {
        Ok(no_peers())
    }

    fn envs(&self) -> Box<dyn FnMut() -> Env> {
        let n = self.seed.fetch_add(1, Ordering::SeqCst);
        Box::new(envs_from(0xC0FF_EE00 ^ n.wrapping_mul(0x1000_0001)))
    }

    fn now(&self) -> u64 {
        1_791_900_000
    }
}

/// A volume at `dir`, as the OS would list a card in a USB reader.
pub(crate) fn volume_at(dir: &std::path::Path, fs: Fs, serial: u32) -> Found {
    let volume = Volume {
        path: dir.to_path_buf(),
        device: "sdz1".into(),
        fs,
        table: Table::Mbr,
        label: "CARD".into(),
        serial: Some(Serial::Fat(serial)),
        space: Some(Space { size: 64 << 30, free: 32 << 30, cluster: 32 * 1024, clusters: 0 }),
        read_only: false,
        utf8: Some(true),
        clock: Clock::Offset(0),
    };
    let disk = Disk {
        id: "sdz".into(),
        removable: true,
        bus: Bus::Usb,
        read_only: false,
        system: Vec::new(),
        others: Vec::new(),
    };
    Found::new(volume, disk)
}

/// The fake server as a saved server, the session's.
pub(crate) fn home_server(base: &str) -> ServerLink {
    ServerLink {
        id: base.to_string(),
        name: "Home".into(),
        base: Ok(base.to_string()),
        token: Some("tok".into()),
        session: true,
        ..ServerLink::default()
    }
}

struct Rig {
    link: Link,
    events: Receiver<Event>,
    fold: Fold,
    shelf: Arc<Shelf>,
    here: Arc<Here>,
}

impl Rig {
    fn new(shelf: Shelf) -> Rig {
        let (shelf, here) = (Arc::new(shelf), Arc::new(Here::default()));
        let every = Duration::from_millis(40);
        let setup = Setup { volumes: shelf.clone(), home: here.clone(), every };
        let (link, events) = spawn(setup);
        Rig { link, events, fold: Fold::default(), shelf, here }
    }

    /// Folds the worker's reports until `done` holds of the fold, for at
    /// most 30 s.
    fn until(&mut self, what: &str, done: impl Fn(&Fold) -> bool) {
        let t0 = Instant::now();
        while !done(&self.fold) {
            let (cards, notes) = (&self.fold.cards, &self.fold.notes);
            assert!(t0.elapsed() < Duration::from_secs(30), "waited for {what}: {notes:?}
{cards:#?}");
            if let Ok(event) = self.events.recv_timeout(Duration::from_millis(50)) {
                self.fold.apply(event);
            }
        }
    }

    fn card(&self, key: &str) -> &CardView {
        self.fold.cards.iter().find(|c| c.key == key).expect("the card")
    }

    fn send(&self, cmd: Cmd) {
        assert!(self.link.send(cmd));
    }
}

fn planned(card: &CardView) -> Option<&Planned> {
    match &card.next {
        Next::Plan(plan) => Some(plan),
        _ => None,
    }
}

/// A set-up card in the shelf, and the server: the rig, the card's key, the
/// fake server.
fn with_card(scratch: &Scratch) -> (Rig, String, Fake, FatSim) {
    let sim = card(scratch);
    let (base, fake) = server();
    let found = volume_at(&scratch.0.join("card"), Fs::Fat32, SERIAL);
    let key = key_of(&found);
    let shelf = Shelf::default();
    shelf.put(found, SERIAL);
    let mut rig = Rig::new(shelf);
    rig.send(Cmd::Servers(vec![home_server(&base)]));
    let wanted = key.clone();
    rig.until("the card read and its server asked", |f| {
        f.cards.iter().any(|c| c.key == wanted && planned(c).is_some())
    });
    (rig, key, fake, sim)
}

fn without_lock(sim: &FatSim) -> std::collections::BTreeMap<String, (u64, u32, u64)> {
    let mut image = sim.image();
    image.retain(|p, _| !p.ends_with("run.lock"));
    image
}

// ── The tests ───────────────────────────────────────────────────────────────

#[test]
fn a_card_put_in_is_read_and_its_server_asked_with_nothing_written() {
    let scratch = Scratch::new("worker-look");
    let sim = card(&scratch);
    let before = without_lock(&sim);
    drop(sim);
    let (base, _fake) = server();
    let found = volume_at(&scratch.0.join("card"), Fs::Fat32, SERIAL);
    let key = key_of(&found);
    let shelf = Shelf::default();
    shelf.put(found, SERIAL);
    let mut rig = Rig::new(shelf);
    rig.send(Cmd::Servers(vec![home_server(&base)]));
    let wanted = key.clone();
    rig.until("the plan", |f| f.cards.iter().any(|c| c.key == wanted && planned(c).is_some()));
    let card = rig.card(&key);
    let facts = card.facts().expect("read");
    assert!(facts.set_up());
    assert_eq!(card.name().as_deref(), Some("Car"));
    // Set up and never synced: it takes the session's server.
    let server = &card.server;
    assert!(matches!(server, Server::New(Some(link)) if link.name == "Home"), "{server:?}");
    let plan = planned(card).unwrap();
    assert_eq!((plan.new, plan.removals.len()), (5, 0), "every library the account sees: {plan:?}");
    assert_eq!(plan.offered, ["Rock", "Jazz"]);
    assert!(plan.fit.is_some_and(|f| f.fits()), "a fit, from the volume's space");
    // Read and asked, and nothing written.
    let sim = FatSim::new(&scratch.0.join("card"), SERIAL);
    assert_eq!(without_lock(&sim), before);
    assert!(rig.fold.notes.contains(&Note::PutIn { drive: card.drive() }));
    assert!(rig.fold.notes.contains(&Note::Read { key: key.clone() }));
    let logs: Vec<&LogItem> =
        rig.fold.logs.iter().filter(|(k, _)| *k == key).map(|(_, l)| l).collect();
    assert!(matches!(logs[0], LogItem::PutIn { card, .. } if card == "Car"), "{logs:?}");
    assert!(logs.iter().any(|l| matches!(l, LogItem::Asked { new: 5, .. })), "{logs:?}");
}

#[test]
fn a_sync_runs_on_the_worker_its_progress_told_and_done_is_the_card_again() {
    let scratch = Scratch::new("worker-sync");
    let (mut rig, key, _fake, _sim) = with_card(&scratch);
    let mut phases: Vec<Phase> = Vec::new();
    rig.send(Cmd::Sync { key: key.clone(), allow: Box::default() });
    let t0 = Instant::now();
    loop {
        assert!(t0.elapsed() < Duration::from_secs(30), "the sync ended: {:#?}", rig.fold.cards);
        if let Ok(event) = rig.events.recv_timeout(Duration::from_millis(50)) {
            if let Event::Progress { run, .. } = &event
                && phases.last() != Some(&run.phase)
            {
                phases.push(run.phase);
            }
            rig.fold.apply(event);
        }
        let card = rig.card(&key);
        if card.run.is_none() && matches!(card.ended, Some(Ended::Done { .. })) {
            break;
        }
    }
    assert!(phases.contains(&Phase::Copying) && phases.contains(&Phase::Verifying), "{phases:?}");
    let ended = rig.card(&key).ended.clone();
    assert!(matches!(ended, Some(Ended::Done { tracks: 5, .. })), "{ended:?}");
    assert!(rig.fold.notes.iter().any(|n| matches!(n, Note::Synced { tracks: 5, .. })));
    // The next look says the card is in step, and this computer knows it
    // synced it from Home.
    let wanted = key.clone();
    rig.until("the look after", |f| {
        let in_step = |p: &Planned| p.new == 0 && p.unchanged;
        f.cards.iter().any(|c| c.key == wanted && planned(c).is_some_and(in_step))
    });
    let cards = rig.here.cards();
    let seen = cards.cards.values().next().expect("cards.json knows the card");
    assert_eq!((seen.server.as_str(), seen.synced), ("Home", 1_791_900_000));
}

#[test]
fn a_stop_before_anything_is_written_leaves_the_card_as_it_was() {
    let scratch = Scratch::new("worker-stop");
    let (mut rig, key, _fake, sim) = with_card(&scratch);
    let before = without_lock(&sim);
    rig.send(Cmd::Sync { key: key.clone(), allow: Box::default() });
    rig.send(Cmd::Stop { key: key.clone() });
    let wanted = key.clone();
    rig.until("the stop", |f| f.cards.iter().any(|c| c.key == wanted && c.ended.is_some()));
    let ended = rig.card(&key).ended.clone();
    assert!(matches!(ended, Some(Ended::Stopped { at: 0, .. })), "{ended:?}");
    assert_eq!(without_lock(&sim), before, "nothing reached the card");
}

#[test]
fn one_sync_at_a_time_the_second_waits_its_turn_and_dont_wait_forgets_it() {
    let scratch = Scratch::new("worker-two");
    let (mut rig, key, _fake, _sim) = with_card(&scratch);
    // A second card of its own: another serial, another id.
    let other = Scratch::new("worker-two-b");
    let root = other.0.join("card");
    let second = FatSim::new(&root, SERIAL + 1);
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures");
    let golden = fixtures.join("card/golden/device.txt");
    std::fs::create_dir_all(root.join(".player")).unwrap();
    std::fs::copy(golden, root.join(".player/device.txt")).unwrap();
    let ask = setup::Ask { label: "Gym".into(), new_identity: false };
    let mut cards = Cards::default();
    setup::set_up(&second, &ask, &mut envs_from(0xB0B), &mut cards, &mut |_| {}).unwrap();
    let found = volume_at(&root, Fs::Fat32, SERIAL + 1);
    let key_b = key_of(&found);
    rig.shelf.put(found, SERIAL + 1);
    let wanted = key_b.clone();
    rig.until("the second card", |f| {
        f.cards.iter().any(|c| c.key == wanted && planned(c).is_some())
    });

    rig.send(Cmd::Sync { key: key.clone(), allow: Box::default() });
    rig.send(Cmd::Sync { key: key_b.clone(), allow: Box::default() });
    let wanted = key_b.clone();
    rig.until("the second waiting", |f| {
        let waits = |r: &Run| r.phase == Phase::Waiting;
        f.cards.iter().any(|c| c.key == wanted && c.run.as_ref().is_some_and(waits))
    });
    assert!(!rig.card(&key_b).syncing(), "waiting is not syncing");
    // Both end, one after the other.
    let (a, b) = (key.clone(), key_b.clone());
    rig.until("both synced", |f| {
        let done = |k: &str| {
            f.cards.iter().any(|c| c.key == k && matches!(c.ended, Some(Ended::Done { .. })))
        };
        done(&a) && done(&b)
    });

    // A second one forgotten before its turn never runs.
    rig.send(Cmd::Sync { key: key.clone(), allow: Box::default() });
    rig.send(Cmd::Sync { key: key_b.clone(), allow: Box::default() });
    rig.send(Cmd::DontWait { key: key_b.clone() });
    let a = key.clone();
    rig.until("the first again", |f| {
        f.cards.iter().any(|c| c.key == a && c.run.is_none() && c.ended.is_some())
    });
    assert!(rig.card(&key_b).run.is_none());
}

#[test]
fn half_the_card_gone_from_the_server_is_a_gate_with_each_library_counted() {
    let scratch = Scratch::new("worker-half");
    let (mut rig, key, fake, _sim) = with_card(&scratch);
    rig.send(Cmd::Sync { key: key.clone(), allow: Box::default() });
    let wanted = key.clone();
    rig.until("the first sync", |f| {
        f.cards.iter().any(|c| c.key == wanted && matches!(c.ended, Some(Ended::Done { .. })))
    });
    {
        let mut lib = fake.lock().unwrap();
        lib.tracks.retain(|t| t.path.ends_with("01.mp3"));
        lib.revision = "1:2:1790000000001:1".into();
    }
    rig.send(Cmd::Sync { key: key.clone(), allow: Box::default() });
    let wanted = key.clone();
    rig.until("the gate", |f| f.cards.iter().any(|c| c.key == wanted && c.gate.is_some()));
    let gate = rig.card(&key).gate.clone().unwrap();
    let Some(Gate::Half { n, total, by_library }) = gate.first() else { panic!("{gate:?}") };
    assert_eq!((*n, *total), (3, 5));
    assert_eq!(by_library, &[("Rock".to_string(), 3, 4)], "Rock's 3 of its 4");

    // Answered, the sync removes them.
    let answers = Answers { removals: true, mass_removals: true, ..Answers::default() };
    let allow = Box::new(Allow { answers, ..Allow::default() });
    rig.send(Cmd::Sync { key: key.clone(), allow });
    let wanted = key.clone();
    rig.until("the removals", |f| {
        let two = |c: &CardView| matches!(c.ended, Some(Ended::Done { tracks: 2, .. }));
        f.cards.iter().any(|c| c.key == wanted && two(c))
    });
}

#[test]
fn set_up_then_apply_write_the_cards_settings_and_nothing_else() {
    let scratch = Scratch::new("worker-setup");
    let dir = scratch.0.join("new");
    std::fs::create_dir_all(dir.join(".player")).unwrap();
    let (base, _fake) = server();
    let found = volume_at(&dir, Fs::Fat32, SERIAL);
    let key = key_of(&found);
    let shelf = Shelf::default();
    shelf.put(found, SERIAL);
    let mut rig = Rig::new(shelf);
    rig.send(Cmd::Servers(vec![home_server(&base)]));
    let wanted = key.clone();
    rig.until("the new card read", |f| {
        f.cards.iter().any(|c| c.key == wanted && c.facts().is_some())
    });
    let card = rig.card(&key);
    assert!(!card.facts().unwrap().set_up());
    let server = &card.server;
    assert!(matches!(server, Server::New(Some(link)) if link.name == "Home"), "the session's");
    let mut want = Want::of(card.facts().unwrap(), card.server.link());
    (want.name, want.libraries) = ("Gym".into(), Some(vec!["Rock".into()]));
    rig.send(Cmd::SetUp { key: key.clone(), want: Box::new(want.clone()) });
    let wanted = key.clone();
    rig.until("set up, and asked", |f| {
        f.cards.iter().any(|c| c.key == wanted && planned(c).is_some_and(|p| p.new == 4))
    });
    let card = rig.card(&key);
    assert_eq!(card.name().as_deref(), Some("Gym"));
    assert_eq!(card.facts().unwrap().libraries(), ["Rock"]);
    let text = std::fs::read_to_string(dir.join(".mstream/state.json")).unwrap();
    assert!(text.contains("\"nickname\":\"Home\"") && !text.contains("127.0.0.1"), "{text}");

    // Apply writes the settings alone: never removes, and nothing in /music.
    want.removal = Removal::Never;
    let music = std::fs::read_dir(dir.join("music")).unwrap().count();
    rig.send(Cmd::Apply { key: key.clone(), want: Box::new(want) });
    let wanted = key.clone();
    rig.until("applied", |f| {
        f.cards.iter().any(|c| {
            c.key == wanted
                && c.facts()
                    .and_then(|f| f.state.as_ref())
                    .is_some_and(|s| s.settings.removal == Removal::Never)
        })
    });
    assert_eq!(std::fs::read_dir(dir.join("music")).unwrap().count(), music);
}

#[test]
fn the_fit_is_worked_out_for_the_sheets_choice_and_answers_its_own_request() {
    let scratch = Scratch::new("worker-fit");
    let (mut rig, key, _fake, _sim) = with_card(&scratch);
    let card = rig.card(&key).clone();
    let mut want = Want::of(card.facts().unwrap(), card.server.link());
    want.libraries = Some(vec!["Jazz".into()]);
    rig.send(Cmd::Fit { key: key.clone(), want: Box::new(want), seq: 7 });
    let wanted = key.clone();
    rig.until("the fit", |f| {
        f.cards.iter().any(|c| c.key == wanted && matches!(&c.fit, Some((7, Next::Plan(_)))))
    });
    let Some((_, Next::Plan(plan))) = &rig.card(&key).fit else { unreachable!() };
    assert_eq!(plan.new, 1, "Jazz's one file");
    let totals = &plan.totals;
    assert!(totals.iter().any(|(lib, files, _)| lib == "Jazz" && *files == 1), "{totals:?}");
}

#[test]
fn a_volume_the_player_cant_use_is_listed_and_hidden_until_shown_again() {
    let scratch = Scratch::new("worker-hide");
    let dir = scratch.0.join("exfat");
    std::fs::create_dir_all(&dir).unwrap();
    let found = volume_at(&dir, Fs::ExFat, 0x0BAD_F00D);
    let key = key_of(&found);
    let shelf = Shelf::default();
    shelf.put(found, 0x0BAD_F00D);
    let mut rig = Rig::new(shelf);
    let wanted = key.clone();
    rig.until("listed", |f| f.cards.iter().any(|c| c.key == wanted));
    assert_eq!(rig.card(&key).read, Read::Refused, "listed with why, never read");
    rig.send(Cmd::Hide { key: key.clone() });
    let wanted = key.clone();
    rig.until("hidden", |f| f.watched && !f.cards.iter().any(|c| c.key == wanted));
    assert!(rig.here.cards().is_hidden("0BAD-F00D"));
    // It stays off the row at the next looks, until shown again.
    for _ in 0..5 {
        if let Ok(event) = rig.events.recv_timeout(Duration::from_millis(60)) {
            rig.fold.apply(event);
        }
    }
    assert!(!rig.fold.cards.iter().any(|c| c.key == key));
    rig.send(Cmd::ShowHidden);
    let wanted = key.clone();
    rig.until("shown again", |f| f.cards.iter().any(|c| c.key == wanted));
    // The computer's own disks are never listed at all.
    let mut system = volume_at(&scratch.0.join("sys"), Fs::Fat32, 1);
    system.disk.system = vec!["/".into()];
    system.refusals = volume::judge(&system.disk, &system.volume);
    assert!(!listed(&system));
}

#[test]
fn eject_asks_the_os_and_a_card_taken_out_takes_its_tab_with_it() {
    let scratch = Scratch::new("worker-eject");
    let (mut rig, key, _fake, _sim) = with_card(&scratch);
    rig.send(Cmd::Eject { key: key.clone() });
    rig.until("ejected", |f| {
        f.notes.iter().any(|n| matches!(n, Note::Ejected { card } if card == "Car"))
    });
    assert_eq!(*rig.shelf.ejected.lock().unwrap(), [key.clone()]);
    rig.shelf.take(&key);
    let wanted = key.clone();
    rig.until("gone", |f| !f.cards.iter().any(|c| c.key == wanted));
    assert!(rig.fold.notes.iter().any(|n| matches!(n, Note::Out { card, .. } if card == "Car")));
}

#[test]
fn a_card_taken_out_in_its_sync_keeps_its_tab_marked_out() {
    // The watch on its own, with the sync's place held by hand.
    let scratch = Scratch::new("worker-out");
    let _sim = card(&scratch);
    let found = volume_at(&scratch.0.join("card"), Fs::Fat32, SERIAL);
    let key = key_of(&found);
    let shelf = Arc::new(Shelf::default());
    shelf.put(found.clone(), SERIAL);
    let (me, _inbox) = channel();
    let (out, events) = channel();
    let setup = Setup { volumes: shelf.clone(), home: Arc::new(Here::default()), every: EVERY };
    let mut worker = Worker::new(setup, me, out);
    let mut view = CardView::new(found);
    view.run = Some(Run { phase: Phase::Copying, files: 4812, done_files: 1204, ..Run::default() });
    let slot = Slot {
        view,
        look_again: false,
        told: None,
        retry: None,
        tries: 0,
        copies: Vec::new(),
        deferred: Vec::new(),
    };
    worker.slots.push(slot);
    worker.running = Some(Running { key: key.clone(), stop: Arc::new(AtomicBool::new(false)) });
    shelf.take(&key);
    worker.watch();
    assert!(worker.slots[0].view.out, "kept, marked out");
    let notes: Vec<Event> = events.try_iter().filter(|e| matches!(e, Event::Note(_))).collect();
    assert!(
        notes.iter().any(|e| matches!(e, Event::Note(Note::OutRun { at: 1204, of: 4812, .. }))),
        "{notes:?}"
    );
    // Given up, it goes.
    worker.running = None;
    worker.cmd(Cmd::GiveUp { key: key.clone() });
    assert!(worker.slots.is_empty());
}

#[test]
fn a_cards_server_is_the_saved_one_its_key_names_never_merely_the_sessions() {
    let home = home_server("http://127.0.0.1:3000");
    let studio = ServerLink {
        id: "http://127.0.0.1:4000".into(),
        name: "Studio".into(),
        base: Ok("http://127.0.0.1:4000".into()),
        session: false,
        ..ServerLink::default()
    };
    let key = home.url_key().unwrap();
    let mut facts = Facts { server: Some(key), nickname: "Home".into(), ..Facts::default() };
    let both = [studio.clone(), home.clone()];
    assert!(matches!(server_of(&facts, &both, false), Server::Saved(l) if l.name == "Home"));
    let alone = std::slice::from_ref(&studio);
    assert_eq!(server_of(&facts, alone, false), Server::Unknown("Home".into()));
    facts.server = None;
    let new = server_of(&facts, &both, false);
    assert!(matches!(new, Server::New(Some(l)) if l.name == "Home"), "a new card: the session's");
    assert_eq!(server_of(&facts, &[], false), Server::New(None), "no server saved at all");
    assert_eq!(home.reached(), crate::device::card::source::local::Reached::Loopback);
}

#[test]
fn the_fold_keeps_the_cards_by_drive_and_the_tabs_percentage_follows_the_sync() {
    let scratch = Scratch::new("worker-fold");
    let a = volume_at(&scratch.0.join("b"), Fs::Fat32, 1);
    let b = volume_at(&scratch.0.join("a"), Fs::Fat32, 2);
    let mut fold = Fold::default();
    fold.apply(Event::Card(Box::new(CardView::new(a.clone()))));
    fold.apply(Event::Card(Box::new(CardView::new(b.clone()))));
    assert_eq!(fold.cards[0].key, key_of(&b), "by drive");
    assert!(fold.sync_mark().is_none());
    let run = Run { phase: Phase::Copying, bytes: 400, done_bytes: 112, ..Run::default() };
    fold.apply(Event::Progress { key: key_of(&a), run });
    let (card, pct, paused) = fold.sync_mark().unwrap();
    assert_eq!((card.key.as_str(), pct, paused), (key_of(&a).as_str(), 28, false));
    let mut view = fold.cards[1].clone();
    view.run = None;
    view.ended = Some(Ended::Paused { pause: Pause::SignIn, at: 3, of: 9, bytes: (50, 100) });
    fold.apply(Event::Card(Box::new(view)));
    let (_, pct, paused) = fold.sync_mark().unwrap();
    assert_eq!((pct, paused), (50, true), "paused, dim");
    fold.apply(Event::Gone { key: key_of(&a) });
    assert_eq!(fold.cards.len(), 1);
    assert_eq!(drive_of(&volume_at(std::path::Path::new("E:\\"), Fs::Fat32, 1)), "E:");
}

#[test]
fn a_server_that_doesnt_answer_pauses_the_sync_until_its_retry_or_stop_for_now() {
    let scratch = Scratch::new("worker-pause");
    let (mut rig, key, _fake, _sim) = with_card(&scratch);
    // The same server, gone: nothing answers at its address now.
    let home = rig.card(&key).server.link().unwrap().clone();
    let gone = ServerLink { base: Ok("http://127.0.0.1:9".into()), ..home };
    rig.send(Cmd::Servers(vec![gone]));
    rig.send(Cmd::Sync { key: key.clone(), allow: Box::default() });
    let wanted = key.clone();
    rig.until("the pause", |f| f.cards.iter().any(|c| c.key == wanted && c.paused().is_some()));
    let pause = rig.card(&key).paused().cloned();
    let a_minute = 1_791_900_000 + 60;
    let waits = matches!(pause, Some(Pause::Server { tries: 1, until }) if until == a_minute);
    assert!(waits, "{pause:?}");
    assert!(rig.fold.sync_mark().is_some_and(|(_, _, paused)| paused), "the tab's mark, dim");
    rig.send(Cmd::StopPause { key: key.clone() });
    let wanted = key.clone();
    rig.until("stopped for now", |f| {
        f.cards.iter().any(|c| c.key == wanted && matches!(c.ended, Some(Ended::Stopped { .. })))
    });
    // Asked again, it reads the card and asks its server again.
    rig.fold.logs.clear();
    rig.send(Cmd::Look { key: key.clone() });
    let wanted = key.clone();
    rig.until("asked again", |f| {
        f.cards.iter().any(|c| c.key == wanted && matches!(c.next, Next::Unreached { .. }))
    });
}

#[test]
fn the_plays_a_new_identity_a_dismissed_gate_and_the_quit() {
    let scratch = Scratch::new("worker-misc");
    let (mut rig, key, _fake, _sim) = with_card(&scratch);
    rig.send(Cmd::Plays { key: key.clone() });
    rig.until("the plays", |f| f.notes.iter().any(|n| matches!(n, Note::PlaysSent { n: 0, .. })));

    let id = rig.card(&key).facts().and_then(|f| f.root).map(|r| r.0);
    rig.send(Cmd::NewIdentity { key: key.clone() });
    let wanted = key.clone();
    rig.until("a new identity", |f| {
        f.cards.iter().any(|c| c.key == wanted && c.facts().and_then(|f| f.root).map(|r| r.0) != id)
    });
    assert_eq!(rig.card(&key).name().as_deref(), Some("Car"), "its name kept");

    rig.send(Cmd::Dismiss { key: key.clone() });
    rig.send(Cmd::Quit);
    rig.until("idle", |f| f.idle);
    let (link, rx) = Link::pair();
    assert!(link.send(Cmd::ShowHidden));
    assert_eq!(sent(&rx), [Cmd::ShowHidden]);
    let setup = Setup::system();
    assert_eq!(setup.every, EVERY);
}
