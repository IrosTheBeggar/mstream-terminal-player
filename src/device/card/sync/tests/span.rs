//! Multi-card sets end to end (B7): the fake server of [`super`] holding a
//! shelf of made-up artists, several host folders as the cards — each of the
//! size the test says, the player's own room taken on it once it holds music
//! — and this computer's cache of cards in memory, kept between syncs as the
//! command keeps `cards.json`.

use std::collections::BTreeSet;

use super::*;
use crate::device::card::engine::fatsim::Cut;
use crate::device::card::names::match_key;
use crate::device::card::span::{self, Decision, Key, Offer, Set};

/// Artist folders under Rock, each with albums of so many tracks.
type Shelf = [(&'static str, &'static [usize])];

/// 47 tracks of about 12 KB of load each: three cards of [`ROOM`].
const SHELF: &Shelf = &[
    ("Artist A", &[4, 4]),
    ("Artist B", &[5]),
    ("Artist C", &[4, 3]),
    ("Artist D", &[6]),
    ("Artist E", &[4, 4]),
    ("Artist F", &[5]),
    ("Artist G", &[4, 4]),
];

/// A card's usable room; the second card's is bigger.
const ROOM: u64 = 300_000;
const ROOM_TWO: u64 = 450_000;

/// A track: an MP3 of `frames` silence-shaped frames, its id in its first
/// frame's body so no two are alike.
fn mp3(id: i64, frames: usize) -> Vec<u8> {
    let mut bytes = mp3_bytes(frames, false);
    bytes[320..328].copy_from_slice(&id.to_le_bytes());
    bytes
}

/// The shelf's tracks from id `first`, as `Rock/<artist>/Album <n>/<t>.mp3`.
fn shelved(shelf: &Shelf, first: i64, frames: usize) -> Vec<Track> {
    let mut tracks = Vec::new();
    for (artist, albums) in shelf {
        for (a, n) in albums.iter().enumerate() {
            for t in 1..=*n {
                let id = first + tracks.len() as i64;
                tracks.push(Track {
                    path: format!("Rock/{artist}/Album {}/{t:02}.mp3", a + 1),
                    id,
                    bytes: mp3(id, frames),
                    seconds: 0.6,
                    art: None,
                    full: json!({"sample-rate": 44100, "channels": 2, "bitrate": 128_000}),
                });
            }
        }
    }
    tracks
}

/// The card's size and free space as its volume would say them: room for
/// the transfer's files of `room` bytes beside the margin and the player's
/// own room, less the files on it now, each to its 512-byte cluster, and
/// the player's own room once the card holds music (it has booted with it).
fn space(sim: &FatSim, room: u64) -> Option<Space> {
    let big = Space { size: 1 << 30, free: 1 << 30, cluster: 512, clusters: 0 };
    let kept = (1 << 30) - plan::usable_room(&big, 0);
    let player = kept - (256 << 20);
    let files = sim.image();
    let mut used: u64 = files.values().map(|(len, _, _)| len.div_ceil(512) * 512).sum();
    if files.keys().any(|p| p.starts_with("/music/")) {
        used += player;
    }
    let size = kept + room;
    Some(Space { size, free: size.saturating_sub(used), cluster: 512, clusters: 0 })
}

/// A server, its cards, and this computer's cache of them.
struct World {
    scratch: Scratch,
    base: String,
    fake: Fake,
    cards: Cards,
    envs: Box<dyn FnMut() -> Env>,
}

impl World {
    fn new(name: &str, shelf: &Shelf, frames: usize) -> World {
        let tracks = shelved(shelf, 1, frames);
        let (vpaths, revision) = (vec!["Rock".into()], "1:1".into());
        let lib = Library { vpaths, tracks, revision, ..Library::default() };
        let fake = Arc::new(Mutex::new(lib));
        let served = fake.clone();
        let base = routed(move |asked| answer(&served, asked));
        let (scratch, cards) = (Scratch::new(name), Cards::default());
        World { scratch, base, fake, cards, envs: Box::new(envs()) }
    }

    /// More music on the server, under a new revision.
    fn add(&self, shelf: &Shelf, frames: usize) {
        let mut lib = self.fake.lock().unwrap();
        let first = lib.tracks.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        lib.tracks.extend(shelved(shelf, first, frames));
        lib.revision = format!("1:{first}");
    }

    /// A card the transfer set up, in a player that reads Opus and Auto DJ.
    fn card(&mut self, name: &str, serial: u32) -> FatSim {
        let root = self.scratch.0.join(name);
        let sim = FatSim::new(&root, serial);
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card");
        let golden = fixtures.join("golden/device.txt");
        std::fs::create_dir_all(root.join(".player")).unwrap();
        std::fs::copy(golden, root.join(".player/device.txt")).unwrap();
        let ask = setup::Ask { label: name.into(), new_identity: false };
        let set_up = setup::set_up(&sim, &ask, &mut *self.envs, &mut self.cards, &mut |_| {});
        set_up.expect("a set-up card");
        sim
    }

    fn sync(&mut self, sim: &FatSim, room: u64, ask: &Ask) -> Synced {
        let client = Arc::new(Client::new(&self.base).unwrap().with_token(Some("tok".into())));
        let plays: Rc<dyn history::Store> = Rc::new(MemStore::default());
        let (fed, space) = (no_peers(), space(sim, room));
        let (cards, envs) = (&mut self.cards, &mut *self.envs);
        sync(sim, space, client, Reached::Elsewhere, ask, cards, plays, &fed, envs, &mut |_| {})
    }

    fn join(&mut self, sim: &FatSim, room: u64, set: u64, index: usize) -> span::Joined {
        let (space, envs) = (space(sim, room), &mut *self.envs);
        let joined = span::join(sim, space, set, index, false, &mut self.cards, envs, &mut |_| {});
        joined.expect("joined")
    }

    /// The server's files whose key `pick` takes.
    fn paths(&self, pick: impl Fn(&Key) -> bool) -> BTreeSet<String> {
        let lib = self.fake.lock().unwrap();
        lib.tracks.iter().map(|t| t.path.clone()).filter(|p| pick(&Key::of(p))).collect()
    }
}

/// The selection, each card's files of it: no cover, no Auto DJ index
/// (fewer requests), the server called Home.
fn plain() -> Ask {
    let change = Change {
        covers: Some(false),
        auto_dj: Some(false),
        nickname: Some("Home".into()),
        ..Change::default()
    };
    Ask { libraries: Some(vec!["Rock".into()]), change, ..Ask::default() }
}

fn decide(d: Decision) -> Ask {
    Ask { decide: Some(d), ..plain() }
}

fn artist(name: &str) -> Key {
    Key { artist: match_key(name), album: String::new() }
}

/// An artist's files, by its folder's name.
fn of(names: &[&str]) -> impl Fn(&Key) -> bool {
    let keys: Vec<String> = names.iter().map(|n| match_key(n)).collect();
    move |k| keys.contains(&k.artist)
}

/// The server paths the card's ledger lists.
fn ledger(sim: &FatSim) -> BTreeSet<String> {
    let seen = look(sim);
    let records = seen.root.unwrap().tags.unwrap().records;
    records.into_iter().filter_map(|e| e.ledger).map(|l| l.server_path).collect()
}

/// The card's set, as its state file holds it.
fn set_on(sim: &FatSim) -> Set {
    State::of(&look(sim).state).and_then(|s| s.set).expect("a card of a set")
}

fn halted_fit(synced: &Synced) -> bool {
    matches!(synced.end, End::Halted(Halt::Fit(_)))
}

fn summary_set(synced: &Synced) -> &SetSummary {
    synced.summary.as_ref().and_then(|s| s.set.as_ref()).expect("a set's summary")
}

fn without_lock(sim: &FatSim) -> BTreeMap<String, (u64, u32, u64)> {
    let mut image = sim.image();
    image.retain(|p, _| !p.ends_with("run.lock"));
    image
}

/// The three cards of a set made of the shelf: card 1 (A to C) made it,
/// card 2 (D to F, a bigger card) and card 3 (G on) joined it; each synced.
fn full_set(name: &str) -> (World, Vec<FatSim>, u64) {
    let mut w = World::new(name, SHELF, 24);
    let one = w.card("one", 0x1111_0001);
    let made = w.sync(&one, ROOM, &decide(Decision::Spread));
    assert_eq!(ran(&made).verified, Ok(()), "{:?}", made.notes);
    let id = set_on(&one).id;
    let two = w.card("two", 0x2222_0002);
    w.join(&two, ROOM_TWO, id, 2);
    let synced = w.sync(&two, ROOM_TWO, &plain());
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    let three = w.card("three", 0x3333_0003);
    w.join(&three, ROOM, id, 3);
    let synced = w.sync(&three, ROOM, &plain());
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    (w, vec![one, two, three], id)
}

/// Every server path on any of `cards`.
fn anywhere(cards: &[&FatSim]) -> BTreeSet<String> {
    cards.iter().flat_map(|c| ledger(c)).collect()
}

// ── The tests ───────────────────────────────────────────────────────────────

#[test]
fn a_card_that_cant_hold_its_selection_spreads_it_over_a_set_and_takes_its_first_range() {
    let mut w = World::new("spread", SHELF, 24);
    let one = w.card("one", 0x1111_0001);
    let image = without_lock(&one);
    // The selection doesn't fit: the plan says what a set of it would be.
    let full = w.sync(&one, ROOM, &plain());
    assert!(halted_fit(&full), "{:?}", full.end);
    let spread: Vec<&Key> = full.summary.as_ref().unwrap().spread.iter().map(|p| &p.from).collect();
    assert_eq!(spread, [&Key::default(), &artist("Artist D"), &artist("Artist G")]);
    assert_eq!(without_lock(&one), image);
    // A dry run makes it, and writes nothing.
    let dry = w.sync(&one, ROOM, &Ask { dry_run: true, ..decide(Decision::Spread) });
    assert!(matches!(dry.end, End::Dry), "{:?}", dry.end);
    let made = summary_set(&dry);
    assert!(made.created && made.set.cards.len() == 3, "{made:?}");
    assert_eq!((made.mine, made.theirs), (20, 27));
    assert_eq!(without_lock(&one), image);
    assert!(w.cards.sets.is_empty());

    // Made: card 1 takes A to C, each artist whole; the others wait for
    // their cards.
    let synced = w.sync(&one, ROOM, &decide(Decision::Spread));
    let outcome = ran(&synced);
    assert_eq!((outcome.verified.clone(), outcome.written), (Ok(()), 20), "{:?}", synced.notes);
    assert_eq!(ledger(&one), w.paths(of(&["Artist A", "Artist B", "Artist C"])));
    let set = set_on(&one);
    let card_id = outcome.card_id;
    assert_eq!((set.rev, set.libraries.clone(), set.cards.len()), (1, vec!["Rock".to_string()], 3));
    assert_eq!(set.cards[0].card_id, Some(card_id));
    assert!(set.cards[1..].iter().all(|s| s.card_id.is_none() && s.tracks > 0));
    assert!(set.holds(0, set.asks), "it holds its whole range");
    assert_eq!((set.cards[0].tracks, set.cards[0].others), (20, 0));
    let text = std::fs::read_to_string(w.scratch.0.join("one/.mstream/state.json")).unwrap();
    let state: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(state["set"]["index"], 1);
    // This computer's cache mirrors it.
    assert_eq!(w.cards.sets[&set.id], set);
    assert_eq!(w.cards.cards[&card_id].set, Some(set.id));
    let printed = synced_json(&synced, false);
    assert_eq!(printed["plan"]["set"]["set"]["count"], 3);
    assert_eq!(printed["plan"]["set"]["created"], true);
    // A card of a set can't be spread again.
    let again = w.sync(&one, ROOM, &decide(Decision::Spread));
    assert!(matches!(&again.end, End::Halted(Halt::Refused(why)) if why.contains("set already")));
}

#[test]
fn the_next_cards_join_the_set_and_each_synced_alone_takes_its_range() {
    let mut w = World::new("join", SHELF, 24);
    let one = w.card("one", 0x1111_0001);
    ran(&w.sync(&one, ROOM, &decide(Decision::Spread)));
    let id = set_on(&one).id;
    let two = w.card("two", 0x2222_0002);
    let image_two = two.image();
    let cards = w.cards.clone();
    let refused = |index: usize, set: u64| {
        let (mut cards, space) = (cards.clone(), space(&two, ROOM_TWO));
        let mut envs = envs();
        match span::join(&two, space, set, index, false, &mut cards, &mut envs, &mut |_| {}) {
            Err(Stop::Refused(why)) => why,
            other => panic!("{other:?}"),
        }
    };
    assert!(refused(1, id).contains("card 1 is another card"));
    assert!(refused(4, id).contains("no card 4"));
    assert!(refused(2, 0x77).contains("knows no set"));
    assert_eq!(two.image(), image_two, "nothing written");

    // Card 2 joins: the set's table, and its selection, are its own now.
    let joined = w.join(&two, ROOM_TWO, id, 2);
    assert!(joined.wrote && !joined.already);
    let state = State::of(&look(&two).state).unwrap();
    let set = state.set.as_ref().unwrap();
    assert_eq!((set.slot_of(joined.card_id), set.cards[1].room), (Some(1), ROOM_TWO));
    assert_eq!(state.settings.sources[0].libraries, ["Rock"]);
    assert!(w.join(&two, ROOM_TWO, id, 2).already);
    // Synced alone, it takes its range; card 1 isn't in a reader, and is
    // never touched.
    let image_one = one.image();
    let synced = w.sync(&two, ROOM_TWO, &plain());
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    assert_eq!(ledger(&two), w.paths(of(&["Artist D", "Artist E", "Artist F"])));
    assert_eq!(one.image(), image_one);
    let set = set_on(&two);
    assert!(set.holds(1, set.asks));
    assert_eq!(summary_set(&synced).theirs, 28, "A to C on card 1, G on card 3 to come");
    // A card of a set syncs the set's libraries, no others.
    let jazz = Ask { libraries: Some(vec!["Jazz".into()]), ..plain() };
    let other = w.sync(&two, ROOM_TWO, &jazz);
    let End::Halted(Halt::Refused(why)) = &other.end else { panic!("{:?}", other.end) };
    assert!(why.contains("set's libraries"), "{why}");

    // Card 1 again, card 3 still to come: its range as it was, nothing done;
    // its state file now names card 2 too, from this computer's cache.
    let generation = look(&one).root.unwrap().manifest.header.generation;
    let again = w.sync(&one, ROOM, &plain());
    let outcome = ran(&again);
    assert_eq!((outcome.generation, outcome.written, outcome.deleted), (generation, 0, 0));
    let set = set_on(&one);
    assert_eq!(set.cards[1].card_id, Some(joined.card_id));
    assert_eq!(set.cards[2].card_id, None);
}

#[test]
fn new_music_lands_on_its_ranges_card_and_the_edges_stay_where_they_were() {
    let (mut w, cards, _) = full_set("grow");
    let (one, two, three) = (&cards[0], &cards[1], &cards[2]);
    let edges = |sim: &FatSim| {
        let set = set_on(sim);
        (set.rev, set.cards.iter().map(|s| s.from.clone()).collect::<Vec<_>>())
    };
    let before = edges(one);
    assert_eq!(before.1, [Key::default(), artist("Artist D"), artist("Artist G")]);
    // A new album of E (card 2's range), a new artist between A and B (card
    // 1's).
    w.add(&[("Artist E", &[0, 0, 3]), ("Artist Ab", &[2])], 24);
    let three_was = ledger(three);

    let synced = w.sync(one, ROOM, &plain());
    assert_eq!(ran(&synced).written, 2, "{:?}", synced.notes);
    assert_eq!(ledger(one), w.paths(of(&["Artist A", "Artist Ab", "Artist B", "Artist C"])));
    let synced = w.sync(two, ROOM_TWO, &plain());
    assert_eq!(ran(&synced).written, 3, "{:?}", synced.notes);
    assert!(ledger(two).contains("Rock/Artist E/Album 3/01.mp3"));
    // Card 3's range has nothing new: its commit records only the server's
    // revision.
    let synced = w.sync(three, ROOM, &plain());
    assert_eq!((ran(&synced).written, ran(&synced).deleted), (0, 0));
    assert_eq!(ledger(three), three_was);
    // The edges are where they were, on every card and on this computer.
    for sim in [one, two, three] {
        assert_eq!(edges(sim), before);
    }
    let mirror = w.cards.sets.values().next().unwrap();
    assert_eq!(mirror.rev, 1);
    assert!((0..3).all(|i| mirror.holds(i, mirror.asks)), "{mirror:?}");
}

#[test]
fn a_full_card_moves_its_edge_in_two_phases_and_a_cut_between_loses_no_music() {
    let (mut w, cards, _) = full_set("move");
    let (one, two, three) = (&cards[0], &cards[1], &cards[2]);
    // Eight new tracks of card 1's range: it can't take them.
    w.add(&[("Artist Bb", &[4, 4])], 24);
    let image = without_lock(one);
    let full = w.sync(one, ROOM, &plain());
    assert!(halted_fit(&full), "{:?}", full.end);
    let offers = &summary_set(&full).offers;
    let moved = w.paths(of(&["Artist C"]));
    let [Offer::MoveEdge { to: 1, from, fits: true, load }, Offer::AddCard { at: 1, .. }] = &offers[..]
    else {
        panic!("{offers:?}")
    };
    assert_eq!((from, load.tracks), (&artist("Artist C"), 7));
    assert_eq!(without_lock(one), image);

    // The edge moved: card 1 keeps C until card 2 holds it, so it still
    // can't take Bb — the answer stays on the card all the same.
    let decided = w.sync(one, ROOM, &decide(Decision::MoveEdge));
    assert!(halted_fit(&decided), "{:?}", decided.end);
    let set = summary_set(&decided);
    let said = (set.decided, set.written, set.held.get(&1).copied());
    assert_eq!(said, (Some(Decision::MoveEdge), true, Some(7)));
    let table = set_on(one);
    assert_eq!((table.rev, &table.cards[1].from), (2, &artist("Artist C")));
    assert_eq!(w.cards.sets[&table.id].rev, 2, "mirrored");
    let everywhere = anywhere(&[one, two, three]);
    assert!(everywhere.is_superset(&moved));

    // Card 2's sync is cut partway: it doesn't hold its range, and card 1
    // lets nothing go.
    two.cut_at(12, Cut::Fail);
    let cut = w.sync(two, ROOM_TWO, &plain());
    assert!(matches!(cut.end, End::Halted(Halt::Card(Stop::Pulled))), "{:?}", cut.end);
    two.replug();
    assert!(!w.cards.sets[&table.id].holds(1, 0));
    let kept = w.sync(one, ROOM, &plain());
    assert!(halted_fit(&kept), "{:?}", kept.end);
    let set = summary_set(&kept);
    assert_eq!((set.held.get(&1).copied(), set.written), (Some(7), false));
    assert!(ledger(one).is_superset(&moved));
    assert!(anywhere(&[one, two, three]).is_superset(&everywhere));

    // Card 2 again: it takes C, and holds its range.
    let synced = w.sync(two, ROOM_TWO, &plain());
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    assert!(ledger(two).is_superset(&moved));
    let mirror = &w.cards.sets[&table.id];
    assert!(mirror.holds(1, mirror.cards[0].asked), "{mirror:?}");
    // Now card 1 lets C go, and takes Bb in its room.
    let synced = w.sync(one, ROOM, &plain());
    let outcome = ran(&synced);
    assert_eq!((outcome.deleted, outcome.written, outcome.verified.clone()), (7, 8, Ok(())));
    assert_eq!(summary_set(&synced).dropped.get(&1).copied(), Some(7));
    assert_eq!(ledger(one), w.paths(of(&["Artist A", "Artist B", "Artist Bb"])));
    assert!(anywhere(&[one, two, three]).is_superset(&everywhere));
    let set = set_on(one);
    assert_eq!((set.cards[0].others, set.cards[0].tracks), (0, 21));
    // Each card's range, and nothing twice.
    let (a, b, c) = (ledger(one), ledger(two), ledger(three));
    assert!(a.is_disjoint(&b) && b.is_disjoint(&c) && a.is_disjoint(&c));
    assert_eq!(&(&(&a | &b) | &c), &w.paths(|_| true));
}

/// Card 2 cut at any write of the sync that takes the moved artist: it says
/// it holds its range only once its committed ledger has every file of it,
/// and its next sync gets there.
#[test]
fn a_move_cut_at_any_write_of_the_target_card_claims_nothing_it_doesnt_hold() {
    let (mut w, cards, _) = full_set("sweep");
    let (one, two) = (&cards[0], &cards[1]);
    w.add(&[("Artist Bb", &[4, 4])], 24);
    assert!(halted_fit(&w.sync(one, ROOM, &decide(Decision::MoveEdge))));
    let moved = w.paths(of(&["Artist C"]));
    let id = set_on(one).id;
    let was = w.cards.clone();
    // A copy of card 2 as it is now, for each cut.
    let at = w.scratch.0.clone();
    let copy = |name: &str| -> FatSim {
        copy_dir(&two.root(), &at.join(name));
        FatSim::new(&at.join(name), 0x2222_0002)
    };
    let whole = copy("whole");
    let before = whole.log().len();
    ran(&w.sync(&whole, ROOM_TWO, &plain()));
    let ops = whole.log()[before..].to_vec();
    let writes = ops.len() as u64;
    assert!(writes > 20, "{writes}");
    // What card 1 reads (this computer's cache), and what card 2's next
    // sync would put there (its own table merged with the cache): card 2
    // holds its range of the new ranges after card 1's ask.
    let holds = |sim: &FatSim, cards: &Cards| {
        let mirror = &cards.sets[&id];
        let merged = span::merge(&set_on(sim), mirror);
        assert_eq!(merged.rev, 2);
        (merged.holds(1, merged.cards[0].asked), mirror.holds(1, mirror.cards[0].asked))
    };
    // Every write cut, but the bytes of a stage file after the first file's
    // (cut there, a run stops as at the stage file's making); a few of the
    // cuts taken up by the next sync, which the engine's own tests take up
    // at every write.
    let first = ops.iter().position(|op| op.starts_with("rename /.mstream/stage"));
    let first = first.expect("a file put in place") as u64;
    let staged = |op: &str| {
        op.starts_with("write /.mstream/stage") || op.starts_with("sync /.mstream/stage")
    };
    let cuts = (1..=writes).filter(|&k| k <= first + 1 || !staged(&ops[k as usize - 1]));
    let recover = [12, writes - 9, writes - 1, writes];
    let (mut on_card_only, mut mirrored) = (0, 0);
    for k in cuts {
        w.cards = was.clone();
        let sim = copy(&format!("cut-{k}"));
        sim.cut_at(k, Cut::Fail);
        let _ = w.sync(&sim, ROOM_TWO, &plain());
        sim.replug();
        let (on_card, mirror) = holds(&sim, &w.cards);
        if on_card || mirror {
            assert!(ledger(&sim).is_superset(&moved), "cut at {k}: said so, and doesn't hold it");
        }
        on_card_only += (on_card && !mirror) as usize;
        mirrored += mirror as usize;
        if recover.contains(&k) {
            let again = w.sync(&sim, ROOM_TWO, &plain());
            assert_eq!(ran(&again).verified, Ok(()), "cut at {k}: {:?}", again.notes);
            assert!(ledger(&sim).is_superset(&moved), "cut at {k}");
            assert_eq!(holds(&sim, &w.cards), (true, true), "cut at {k}");
        }
        let _ = std::fs::remove_dir_all(sim.root());
    }
    // Cut in its state file's write: the card says so, this computer not
    // yet; cut once it let the card go: both.
    assert!(on_card_only > 0 && mirrored > 0, "{on_card_only} {mirrored}");
}

#[test]
fn a_card_added_after_a_full_one_takes_its_last_artists_once_it_joins() {
    let (mut w, cards, id) = full_set("add");
    let (one, two) = (&cards[0], &cards[1]);
    w.add(&[("Artist Bb", &[4, 4])], 24);
    let decided = w.sync(one, ROOM, &decide(Decision::AddCard));
    assert!(halted_fit(&decided), "{:?}", decided.end);
    let table = set_on(one);
    assert_eq!(table.cards.len(), 4);
    let new = &table.cards[1];
    assert_eq!((new.card_id, &new.from, new.tracks), (None, &artist("Artist C"), 7));
    // Card 2's range is as it was, under the new ranges: it says so again.
    let synced = w.sync(two, ROOM_TWO, &plain());
    assert_eq!((ran(&synced).written, ran(&synced).deleted), (0, 0));
    assert_eq!(set_on(two).cards.len(), 4);
    assert!(set_on(two).holds(2, 0));
    // Card 1 holds C for a card not set up yet.
    let held = w.sync(one, ROOM, &plain());
    assert_eq!(summary_set(&held).held.get(&1).copied(), Some(7));
    // The new card joins as card 2, and takes C; then card 1 lets C go.
    let four = w.card("four", 0x4444_0004);
    w.join(&four, ROOM, id, 2);
    let synced = w.sync(&four, ROOM, &plain());
    assert_eq!(ran(&synced).written, 7, "{:?}", synced.notes);
    let synced = w.sync(one, ROOM, &plain());
    let outcome = ran(&synced);
    assert_eq!((outcome.deleted, outcome.written), (7, 8), "{:?}", synced.notes);
    assert_eq!(ledger(&four), w.paths(of(&["Artist C"])));
}

/// A set made of a card that holds more than its range: the files of the
/// other card's range go once that card holds them, past the stronger gate
/// without asking again (they are the set's); a file the server dropped in
/// the same sync is asked about as ever.
#[test]
fn a_set_made_of_a_card_holding_more_lets_the_rest_go_and_asks_only_of_what_isnt_the_sets() {
    const SMALL: u64 = 150_000;
    let shelf: &Shelf = &[("Artist A", &[20]), ("Artist B", &[20, 20])];
    let mut w = World::new("gate", shelf, 2);
    let one = w.card("one", 0x1111_0001);
    let first = w.sync(&one, SMALL, &plain());
    assert_eq!(ran(&first).written, 60, "{:?}", first.notes);
    // Spread: card 1 keeps A, and B (two-thirds of its ledger) for card 2.
    let spread = w.sync(&one, SMALL, &decide(Decision::Spread));
    assert_eq!((ran(&spread).written, ran(&spread).deleted), (0, 0));
    assert_eq!(summary_set(&spread).held.get(&1).copied(), Some(40));
    let id = set_on(&one).id;
    let two = w.card("two", 0x2222_0002);
    w.join(&two, SMALL, id, 2);
    let synced = w.sync(&two, SMALL, &plain());
    assert_eq!(ran(&synced).written, 40, "{:?}", synced.notes);

    // The server drops a file of A: the sync that would let B go asks first,
    // the whole of what it takes off listed.
    let a1 = "Rock/Artist A/Album 1/01.mp3";
    let restore = {
        let mut lib = w.fake.lock().unwrap();
        let at = lib.tracks.iter().position(|t| t.path == a1).unwrap();
        lib.revision = "2:1".into();
        lib.tracks.remove(at)
    };
    let image = without_lock(&one);
    let asked = w.sync(&one, SMALL, &plain());
    let End::Halted(Halt::Card(Stop::Unanswered(guards))) = &asked.end else {
        panic!("{:?}", asked.end)
    };
    let strong = matches!(&guards[..], [Guard::Removals { strong: true, paths }] if paths.len() == 41);
    assert!(strong, "{guards:?}");
    assert_eq!(without_lock(&one), image);
    // The file back: B goes with no question.
    {
        let mut lib = w.fake.lock().unwrap();
        lib.tracks.push(restore);
        lib.revision = "3:1".into();
    }
    let synced = w.sync(&one, SMALL, &plain());
    let outcome = ran(&synced);
    assert_eq!((outcome.deleted, outcome.records, outcome.verified.clone()), (40, 20, Ok(())));
    assert_eq!(ledger(&one), w.paths(of(&["Artist A"])));
    assert_eq!(ledger(&two), w.paths(of(&["Artist B"])));
}

/// A folder copied whole: a card in another reader, as it was.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
            let stamp = std::fs::metadata(&src).unwrap().modified().unwrap();
            File::options().write(true).open(&dst).unwrap().set_modified(stamp).unwrap();
        }
    }
}
