//! A set's pure parts: the keys, the split, the offers, the table read and
//! written, two copies merged, and where a sync's files stand; with the
//! state file and `cards.json` that hold the table.

use super::*;

fn key(artist: &str, album: &str) -> Key {
    Key { artist: artist.into(), album: album.into() }
}

/// A group of `thousands` thousand bytes of load.
fn group(artist: &str, album: &str, thousands: u64, tracks: u64) -> Group {
    Group { key: key(artist, album), load: Load { bytes: thousands * 1000, tracks } }
}

/// A card's usable room: filled to 900,000 bytes ([`target`]).
const ROOM: u64 = 1_000_000;

#[test]
fn a_files_key_is_its_artist_then_its_album_folder_as_fat_matches_them() {
    assert_eq!(Key::of("Rock/Artist A/Album B/01.mp3"), key("ARTIST A", "ALBUM B"));
    let disc = Key::of("Rock/Artist A/Album B/CD 2/01.flac");
    assert_eq!(disc, key("ARTIST A", "ALBUM B"), "a disc is its album's");
    assert_eq!(Key::of("Rock/Artist A/loose.mp3"), key("ARTIST A", ""));
    assert_eq!(Key::of("Rock/top.mp3"), Key::default());
    // One name to FAT is one key: case, and NFC against NFD.
    assert_eq!(Key::of("Jazz/caf\u{E9}/x/1.mp3"), Key::of("Rock/CAFE\u{301}/X/2.mp3"));
    // A to Z, the artist first; an artist's loose files before its albums.
    let mut keys = [
        Key::of("R/b/a/1.mp3"),
        Key::of("R/a/z/1.mp3"),
        Key::of("R/a/1.mp3"),
        Key::of("R/1.mp3"),
    ];
    keys.sort();
    assert_eq!(keys, [Key::default(), key("A", ""), key("A", "Z"), key("B", "A")]);
}

#[test]
fn a_split_fills_each_card_a_to_z_with_whole_artists_to_about_90_percent() {
    assert_eq!(target(ROOM), Load { bytes: 900_000, tracks: 18_000 });
    // Artists of 400, 300, 500, 200 and 600 thousand bytes.
    let groups = [
        group("A", "1", 250, 2),
        group("A", "2", 150, 1),
        group("B", "1", 300, 3),
        group("C", "1", 500, 4),
        group("D", "1", 200, 1),
        group("E", "1", 600, 5),
    ];
    let parts = split(&groups, ROOM, ROOM);
    let froms: Vec<&Key> = parts.iter().map(|p| &p.from).collect();
    // A and B take the first card, C would make it 1,200: C goes whole to
    // the next, which D fills to 700; E to the third.
    assert_eq!(froms, [&Key::default(), &key("C", ""), &key("E", "")]);
    let loads: Vec<(u64, u64)> = parts.iter().map(|p| (p.load.bytes, p.load.tracks)).collect();
    assert_eq!(loads, [(700_000, 6), (700_000, 5), (600_000, 5)]);
    // Each edge is an artist's first key: its albums to come land with it.
    assert!(parts.iter().all(|p| p.from.album.is_empty()));
    // A bigger first card takes more; the cap on tracks counts as bytes do.
    assert_eq!(split(&groups, 2 * ROOM, ROOM).len(), 2);
    let many = [group("A", "1", 1, 10_000), group("B", "1", 1, 10_000)];
    assert_eq!(split(&many, 100 * ROOM, 100 * ROOM).len(), 2, "18,000 tracks a card");
    // Nothing to split: one card, empty.
    let none = split(&[], ROOM, ROOM);
    assert_eq!(none, [Part { from: Key::default(), load: Load::default() }]);
}

#[test]
fn an_artist_no_card_holds_splits_at_its_albums_and_an_album_never() {
    // B is 1,400 thousand bytes, in albums of 500, 400 and 500.
    let groups = [
        group("A", "1", 300, 1),
        group("B", "1", 500, 1),
        group("B", "2", 400, 1),
        group("B", "3", 500, 1),
        group("C", "1", 1200, 1),
        group("D", "1", 100, 1),
    ];
    let parts = split(&groups, ROOM, ROOM);
    let froms: Vec<&Key> = parts.iter().map(|p| &p.from).collect();
    // B begins where A left room (300 + 500), goes on at its second album;
    // C, one album bigger than a card, goes alone and whole; D after it.
    assert_eq!(froms, [&Key::default(), &key("B", "2"), &key("C", ""), &key("D", "")]);
    let bytes: Vec<u64> = parts.iter().map(|p| p.load.bytes / 1000).collect();
    assert_eq!(bytes, [800, 900, 1200, 100]);
}

/// A set of three cards: from the least key, from C, from E; card 2 not set
/// up yet.
fn three() -> Set {
    let card = |id: Option<u64>, from: Key, seq: u64| Slot {
        card_id: id,
        label: id.map_or_else(String::new, |id| format!("Card {id}")),
        from,
        room: ROOM,
        seq,
        ..Slot::default()
    };
    let cards = vec![
        card(Some(11), Key::default(), 1),
        card(None, key("C", ""), 0),
        card(Some(33), key("E", ""), 1),
    ];
    Set::new(0x5E7, 0xED90, &["Rock".to_string(), "Jazz".to_string()], cards)
}

#[test]
fn every_key_has_one_card_and_the_split_stays_where_it_was_as_the_library_grows() {
    let set = three();
    assert_eq!(set.libraries, ["Jazz", "Rock"]);
    assert_eq!(set.owner(&Key::default()), 0);
    assert_eq!(set.owner(&key("B", "Z")), 0);
    assert_eq!(set.owner(&key("C", "")), 1);
    assert_eq!(set.owner(&key("D", "1")), 1);
    assert_eq!(set.owner(&key("E", "0")), 2);
    assert_eq!(set.owner(&key("ZZ", "")), 2);
    // New music — a new artist between two, a new album that sorts before
    // an artist's first, a new loose file — each on its range's card, the
    // edges as they were.
    assert_eq!(set.owner(&Key::of("Rock/Bb/New/1.mp3")), 0);
    assert_eq!(set.owner(&Key::of("Rock/C/0 First/1.mp3")), 1);
    assert_eq!(set.owner(&Key::of("Rock/E/x.mp3")), 2);
    assert_eq!((set.until(0), set.until(2)), (Some(&key("C", "")), None));
    assert_eq!(set.slot_of(33), Some(2));
    assert!(set.range_text(1).contains("C\u{1f} .. E\u{1f}"), "{}", set.range_text(1));
}

#[test]
fn a_table_reads_back_as_written_and_keeps_what_it_doesnt_know() {
    let mut set = three();
    let commit_id = 0xC0FF_EE00_0000_0001;
    set.cards[0].holds = Some(Holds { rev: 1, asks: 2, generation: 7, commit_id });
    (set.cards[0].others, set.cards[0].others_key, set.cards[0].asked) = (3, 0xABCD, 2);
    set.asks = 2;
    let value = set.value(Some(1));
    assert_eq!(value["index"], 1);
    assert_eq!(value["cards"][1]["cardId"], Value::Null, "a card to come");
    assert_eq!(value["cards"][0]["holds"]["commitId"], "0xC0FFEE0000000001");
    assert_eq!(Set::read(&value), Some(set.clone()));

    // A newer player's members, in the table and in a card, stay.
    let mut newer = value.clone();
    newer["name"] = json!("Everything");
    newer["cards"][2]["colour"] = json!("blue");
    let read = Set::read(&newer).unwrap();
    assert_eq!(read.value(None)["name"], "Everything");
    assert_eq!(read.value(None)["cards"][2]["colour"], "blue");

    // What isn't a table: another version, ranges that don't cover every key
    // once each, a card in it twice, no card, no id.
    let broken = |f: &dyn Fn(&mut Value)| {
        let mut v = value.clone();
        f(&mut v);
        Set::read(&v)
    };
    assert!(broken(&|v| v["version"] = json!(2)).is_none());
    assert!(broken(&|v| v["cards"][0]["from"] = json!(["A", ""])).is_none(), "the least key first");
    assert!(broken(&|v| v["cards"][2]["from"] = json!(["B", ""])).is_none(), "rising");
    assert!(broken(&|v| v["cards"][2]["cardId"] = json!("0x000000000000000B")).is_none(), "twice");
    assert!(broken(&|v| v["cards"] = json!([])).is_none());
    assert!(broken(&|v| v["id"] = json!("0x0")).is_none());
}

#[test]
fn two_copies_merge_by_the_newer_ranges_and_each_cards_own_newest_facts() {
    let own = three();
    // This computer's copy: card 3 synced since (its seq) and holds its
    // range; card 2 joined; the asks went on.
    let mut mirror = own.clone();
    mirror.cards[2].seq = 2;
    mirror.cards[2].holds = Some(Holds { rev: 1, asks: 4, generation: 3, commit_id: 9 });
    mirror.cards[1].card_id = Some(22);
    mirror.cards[1].seq = 1;
    mirror.asks = 4;
    let merged = merge(&own, &mirror);
    assert_eq!(merged.cards[2].holds, mirror.cards[2].holds);
    assert_eq!(merged.cards[1].card_id, Some(22), "a join under the same ranges");
    assert_eq!(merged.asks, 4);
    assert!(merged.holds(2, 4) && !merged.holds(2, 5));
    // The card's own facts are newer on the card: they win there.
    let mut card = own.clone();
    card.cards[0].seq = 5;
    card.cards[0].label = "Car".into();
    assert_eq!(merge(&mirror, &card).cards[0].label, "Car");
    assert_eq!(merge(&card, &mirror).cards[0].label, "Car");

    // Newer ranges win whatever the facts: the old word on a range is void.
    let mut moved = own.clone();
    let edge = |to: usize, from: Key| Offer::MoveEdge { to, from, load: Load::default(), fits: true };
    moved.apply(&edge(1, key("B", "")), 0);
    let merged = merge(&mirror, &moved);
    assert_eq!((merged.rev, &merged.cards[1].from), (2, &key("B", "")));
    assert_eq!(merged.cards[2].holds, mirror.cards[2].holds, "card 3's newest facts");
    assert!(!merged.holds(2, 0), "said under the ranges before");
    assert_eq!(merged.cards[1].card_id, None, "a join under other ranges isn't taken");
    // Another set's copy changes nothing.
    let other = Set { id: 0x5E8, ..mirror.clone() };
    assert_eq!(merge(&own, &other), own);
}

#[test]
fn a_full_card_is_offered_its_edge_moved_or_a_card_added_and_the_answer_is_a_new_revision() {
    let mut set = three();
    // Card 1's range: A (400) and B (300, 500), past its 900.
    let groups = [group("A", "1", 400, 4), group("B", "1", 300, 3), group("B", "2", 500, 5)];
    let offered = offers(&set, 0, &groups, ROOM);
    let load = Load { bytes: 800_000, tracks: 8 };
    // Card 2 has no card yet: its planned load (none) and room take B.
    assert_eq!(
        offered,
        [
            Offer::MoveEdge { to: 1, from: key("B", ""), load, fits: true },
            Offer::AddCard { at: 1, from: key("B", ""), load },
        ]
    );
    // A range that fits its target has nothing to offer; the last card can
    // only add one; a next card too full is said so.
    assert!(offers(&set, 0, &groups[..2], ROOM).is_empty());
    let last = offers(&set, 2, &groups, ROOM);
    assert!(matches!(last[..], [Offer::AddCard { at: 3, .. }]), "{last:?}");
    let mut full = set.clone();
    full.cards[1].bytes = 200_000;
    assert!(matches!(offers(&full, 0, &groups, ROOM)[0], Offer::MoveEdge { fits: false, .. }));

    // Moved: card 2 begins at B, its planned load grows, the ranges' revision.
    set.apply(&offered[0], ROOM);
    assert_eq!((set.rev, &set.cards[1].from, set.cards[1].bytes), (2, &key("B", ""), 800_000));
    assert_eq!(set.owner(&key("B", "2")), 1);
    // Added: a card to come after card 1, its range B..C.
    let mut added = three();
    added.apply(&offered[1], ROOM);
    assert_eq!(added.cards.len(), 4);
    let new = &added.cards[1];
    assert_eq!((new.card_id, &new.from, new.room, new.tracks), (None, &key("B", ""), ROOM, 8));
    assert_eq!((added.owner(&key("B", "1")), added.owner(&key("C", ""))), (1, 2));
    assert_eq!(Decision::MoveEdge.pick(&offered), Some(&offered[0]));
    assert_eq!(Decision::AddCard.pick(&offered[..1]), None);
}

#[test]
fn a_card_lets_another_ranges_files_go_only_once_that_card_holds_its_range_after_the_ask() {
    let mut set = three();
    // Card 1 holds two files of card 3's range and one of its own; a file
    // of card 2's range isn't on it.
    let id = |path: &str| Some(format!("0\u{1f}{path}"));
    let files = [
        (key("A", "1"), id("Rock/A/1/1.mp3")),
        (key("E", "1"), id("Rock/E/1/1.mp3")),
        (key("F", "1"), id("Rock/F/1/1.mp3")),
        (key("C", "1"), None),
    ];
    // The first look asks for them: none goes.
    let (asked, now) = stands(&mut set, 0, &files);
    assert_eq!(now, [Stand::Mine, Stand::Held(2), Stand::Held(2), Stand::Theirs(1)]);
    assert_eq!((asked.seen, asked.asked, asked.others, set.asks), (0, 1, 2, 1));
    set.note(0, "Car", Some(900), &asked);
    let me = &set.cards[0];
    assert_eq!((me.asked, me.others, me.seq, me.room, me.label.as_str()), (1, 2, 2, 900, "Car"));

    // Card 3 holds its range, but said so before the ask: they stay.
    set.cards[2].holds = Some(Holds { rev: 1, asks: 0, generation: 2, commit_id: 5 });
    let (again, now) = stands(&mut set, 0, &files);
    assert_eq!(now[1..3], [Stand::Held(2), Stand::Held(2)]);
    assert_eq!((again.asked, set.asks), (1, 1), "the same files: no new ask");
    // Said after it: they go, and the card keeps nothing for others.
    set.cards[2].holds = Some(Holds { rev: 1, asks: 1, generation: 3, commit_id: 6 });
    let (gone, now) = stands(&mut set, 0, &files);
    assert_eq!(now[1..3], [Stand::Dropped(2), Stand::Dropped(2)]);
    assert_eq!((gone.others, gone.others_key), (0, 0));
    // A file of card 3's held since: a new ask, and all of them stay.
    let mut more = files.to_vec();
    more.push((key("G", "1"), id("Rock/G/1/1.mp3")));
    let (anew, now) = stands(&mut set, 0, &more);
    assert_eq!(now[1..3], [Stand::Held(2), Stand::Held(2)]);
    assert_eq!((anew.seen, anew.asked), (1, 2));
    // Under ranges moved since, the word counts for nothing.
    let mut moved = three();
    moved.cards[2].holds = Some(Holds { rev: 1, asks: 9, generation: 3, commit_id: 6 });
    moved.cards[0].asked = 1;
    let held = ["0\u{1f}Rock/E/1/1.mp3", "0\u{1f}Rock/F/1/1.mp3"];
    moved.cards[0].others_key = fingerprint(&mut held.clone());
    let edge = Offer::MoveEdge { to: 2, from: key("D", ""), load: Load::default(), fits: true };
    moved.apply(&edge, 0);
    let (_, now) = stands(&mut moved, 0, &files);
    assert_eq!(now[1..3], [Stand::Held(2), Stand::Held(2)]);
}

#[test]
fn the_state_file_and_this_computers_cache_hold_the_table() {
    let set = three();
    let fat = volume::Serial::Fat(0x1A2B_3C4D);
    let mut state = State::new(33, "Card 33", Some(&fat), 1);
    state.set = Some(set.clone());
    let text = String::from_utf8(state.text(2)).unwrap();
    let read = |text: &str| {
        let bytes = Some(text.as_bytes());
        State::of(&volume::settle_private(bytes, None, "state", volume::STATE_VERSION)).unwrap()
    };
    let back = read(&text);
    assert_eq!(back.set, Some(set.clone()));
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["set"]["index"], 3, "this card's place, from 1");
    // A table this software can't read stays as it was, and says so.
    let newer = text.replace("\"version\":1}", "\"version\":2}");
    assert_ne!(newer, text);
    let unread = read(&newer);
    assert!(unread.set.is_none() && unread.set_unread);
    let again: Value = serde_json::from_slice(&unread.text(3)).unwrap();
    assert_eq!(again["set"]["version"], 2, "kept");

    // cards.json: the newest table, each card of it marked as the set's.
    let mut cards = Cards::default();
    cards.saw(33, "Card 33", 1, 1, 100);
    cards.mirror(&set);
    let mut later = set.clone();
    later.cards[2].seq = 9;
    later.cards[2].tracks = 42;
    cards.mirror(&later);
    let text = cards.text();
    let parsed = Cards::parse(&text);
    assert_eq!(parsed.sets[&0x5E7].cards[2].tracks, 42);
    assert_eq!(parsed.cards[&33].set, Some(0x5E7));
    // An older copy mirrored after changes nothing it knows newer.
    cards.mirror(&set);
    assert_eq!(cards.sets[&0x5E7].cards[2].tracks, 42);
    assert_eq!(cards.set_of(&set).cards[2].tracks, 42);
    // A table it can't read is kept as it was.
    let mut odd: Value = serde_json::from_str(&text).unwrap();
    odd["sets"]["0x00000000000005E7"]["version"] = json!(9);
    let odd = Cards::parse(&odd.to_string());
    assert!(odd.sets.is_empty());
    let kept: Value = serde_json::from_str(&odd.text()).unwrap();
    assert_eq!(kept["sets"]["0x00000000000005E7"]["version"], 9);
}

/// A folder of its own under the system's temp folder, gone when dropped.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("mstream-span-{pid}-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch folder");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Each session's clock and ids: a fixed time, ids that never repeat.
fn envs() -> impl FnMut() -> Env {
    let mut seed = 0x5E7_u64;
    move || {
        seed += 1;
        let mut n = seed * 1000;
        let id = move || {
            n += 1;
            n
        };
        let producer = "mstream-terminal test".into();
        Env { now: Box::new(|| 1_791_900_000), id: Box::new(id), producer }
    }
}

#[test]
fn the_hidden_command_shows_a_cards_set_and_joins_only_a_volume_the_rule_accepts() {
    use crate::device::card::setup;
    use crate::device::card::volume::{Bus, Clock, Disk, Fs, Table, Volume};
    let scratch = Scratch::new("command");
    let at = |dir: &str, device: &str, fs: Fs| {
        let path = scratch.0.join(dir);
        std::fs::create_dir_all(&path).unwrap();
        let volume = Volume {
            path,
            device: device.into(),
            fs,
            table: Table::Mbr,
            label: "GYM".into(),
            serial: Some(volume::Serial::Fat(0x5E7_0001)),
            space: None,
            read_only: false,
            utf8: Some(true),
            clock: Clock::Offset(0),
        };
        let (system, others, id) = (Vec::new(), Vec::new(), "sdz".into());
        let disk = Disk { id, removable: true, bus: Bus::Usb, read_only: false, system, others };
        Found::new(volume, disk)
    };
    let found = [at("bad", "sdy1", Fs::ExFat), at("good", "sdz1", Fs::Fat32)];
    let mut envs = envs();
    let card = Mounted::of(&found[1]).expect("a volume the rule takes");
    let ask = setup::Ask { label: "Gym".into(), new_identity: false };
    let set_up = setup::set_up(&card, &ask, &mut envs, &mut Cards::default(), &mut |_| {});
    let set_up = set_up.expect("a set-up card");
    // This computer knows a set that waits for its card 2.
    let mut cards = Cards::default();
    cards.mirror(&three());
    let mut call = |named: &str, join: Option<(u64, usize)>, dry_run: bool| {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = on(&found, named, join, dry_run, &mut cards, &mut envs, &mut out, &mut err);
        let json = serde_json::from_slice::<Value>(&out).unwrap_or(Value::Null);
        (code, json, String::from_utf8(err).unwrap())
    };
    let state = || State::of(&volume::inspect(&card).unwrap().state).unwrap();

    // A volume the rule refuses is never opened; one it doesn't list, never
    // looked for.
    let (code, _, err) = call("sdy1", Some((0x5E7, 2)), false);
    assert_eq!(code, 2);
    assert!(err.contains("exfat"), "{err}");
    assert!(std::fs::read_dir(scratch.0.join("bad")).unwrap().next().is_none());
    let (code, _, err) = call("nowhere", None, false);
    assert_eq!(code, 2);
    assert!(err.contains("isn't a volume"), "{err}");
    // Looked at: no set of its own; the set that waits for a card.
    let (code, json, _) = call("sdz1", None, false);
    assert_eq!((code, &json["set"]), (0, &Value::Null));
    assert_eq!(json["waiting"][0]["cards"][1]["cardId"], Value::Null);
    // Another card's place: refused, nothing written.
    let before = state();
    let (code, _, err) = call("sdz1", Some((0x5E7, 1)), false);
    assert_eq!(code, 1);
    assert!(err.contains("card 1 is another card"), "{err}");
    // A dry run says the join, and writes nothing.
    let (code, json, _) = call("sdz1", Some((0x5E7, 2)), true);
    assert_eq!((code, json["wrote"].as_bool(), &json["set"]["index"]), (0, Some(false), &json!(2)));
    assert_eq!(state(), before);
    // Joined: the card's state file holds the set, its label as it was.
    let (code, json, _) = call("sdz1", Some((0x5E7, 2)), false);
    let said = (code, json["wrote"].as_bool(), json["already"].as_bool());
    assert_eq!(said, (0, Some(true), Some(false)));
    let after = state();
    assert_eq!(after.set.as_ref().map(|s| s.cards[1].card_id), Some(Some(set_up.card_id)));
    assert_eq!((after.label.as_str(), after.settings.sources[0].libraries.len()), ("Gym", 2));
    let (code, json, _) = call("sdz1", Some((0x5E7, 2)), false);
    assert_eq!((code, json["already"].as_bool()), (0, Some(true)));
    let (_, json, _) = call("sdz1", None, false);
    assert_eq!((&json["set"]["index"], &json["waiting"]), (&json!(2), &json!([])));
    assert_eq!(cards.sets[&0x5E7].cards[1].card_id, Some(set_up.card_id));
    assert_eq!(cards.cards[&set_up.card_id].set, Some(0x5E7));
}
