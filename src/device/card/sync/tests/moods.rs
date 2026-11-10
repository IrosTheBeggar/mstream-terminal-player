//! A sync's mood scores, end to end (the player's docs/MOOD-FILTERS.md 5.2,
//! 5.3): the fake mStream of [`super`] with its mood route, a host folder as
//! the card, and each record's tail read back by the kit's reader. Whatever
//! the route answers, a run ends well: the tails that came, or those the card
//! had. Every score here is made up, 5.7's worked example among them.

use std::collections::BTreeMap;

use super::*;
use crate::api::transfer::MoodScores;
use crate::device::card::contract::moods::{self, NO_ROW, Tail};
use crate::device::card::contract::prim::u32_at;
use crate::device::card::contract::{container, mptg};
use crate::device::card::sync::moods::{GIFT, NONE, REFUSED, Rule, Step, UNSERVED, said};
use crate::device::card::sync::peers::Ffmpeg;

/// Rock's tracks, as the card's records put them in order.
const ONE: &str = "Rock/Artist A/Album B/01.mp3";
const TWO: &str = "Rock/Artist A/Album B/02.flac";
const WAV: &str = "Rock/Artist C/Album D/01.wav";
const OPUS: &str = "Rock/Artist C/Album D/02.opus";
const ROCK: [&str; 4] = [ONE, TWO, WAV, OPUS];

/// 5.7's made-up track, in `MOOD_COLUMNS` order, and the tail it comes to:
/// sad beats happy, relaxed beats aggressive, a NULL party, and 0.29's 29
/// where a plain floor gives 28.
const EXAMPLE: MoodRow =
    [Some(0.7012), Some(0.3104), Some(0.5803), Some(0.7), Some(0.2999), None, Some(0.4999), Some(0.29)];
const EXAMPLE_TAIL: Tail = [0x03, 0x3A, 0x46, 0x1D, 0x31, 0xFF, 0x46, 0x00];

/// Each record's tail on the card, by its server path.
fn tails(sim: &FatSim) -> BTreeMap<String, Tail> {
    let seen = look(sim);
    let records = &seen.root.as_ref().unwrap().tags.as_ref().unwrap().records;
    let tail = |e: &Entry| (e.ledger.as_ref().unwrap().server_path.clone(), e.rec.moods);
    records.iter().map(tail).collect()
}

/// The tails a sync lays for these server paths: each its [`mood_row`]'s.
fn scored(paths: &[&str]) -> BTreeMap<String, Tail> {
    paths.iter().map(|p| (p.to_string(), moods::encode(Some(&mood_row(p))))).collect()
}

/// The paths of each call of the mood route since the last look, each
/// call's sorted.
fn calls_since(fake: &Fake) -> Vec<Vec<String>> {
    let calls = std::mem::take(&mut fake.lock().unwrap().mood_calls);
    calls.into_iter().map(|mut c| { c.sort(); c }).collect()
}

/// One call, for Rock's four.
fn rock_call() -> Vec<Vec<String>> {
    vec![ROCK.map(String::from).to_vec()]
}

/// The tags file's minor and its RECS stride: 1 and 80 with a tail, else 0
/// and 72, byte for byte what an older writer made.
fn tags_shape(sim: &FatSim) -> (u16, u32) {
    let root = look(sim).root.expect("a root");
    let name = format!("/.mstream/{}", root.manifest.tags.name());
    let bytes = sim.read(&name, usize::MAX).expect("it reads").expect("the file the root names");
    let h = container::Header::parse(&bytes).expect("a frame");
    let entry = |i: u32| &bytes[h.header_bytes as usize + 32 * i as usize..][..32];
    let recs = (0..h.section_count).map(entry).find(|e| u32_at(e, 0) == mptg::RECS);
    (h.minor, u32_at(recs.expect("RECS"), 20))
}

/// The note the run left of its mood scores, whatever it says.
fn note_of(synced: &Synced) -> Option<&str> {
    let moods = |n: &&String| n.contains("mood scores");
    let mut notes = synced.notes.iter().filter(moods);
    let note = notes.next();
    assert_eq!(notes.next(), None, "one note: {:?}", synced.notes);
    note.map(String::as_str)
}

/// An answer's line, `n` of the `of` paths asked for in rows.
fn brought(n: usize, of: usize) -> String {
    format!("mood scores for {n} of the card's {of} tracks from the server")
}

fn committed(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::Committed { .. })).count()
}

/// The card's files, the settings aside: a run that left them as they are.
fn card_files(sim: &FatSim) -> BTreeMap<String, (u64, u32, u64)> {
    let mut image = sim.image();
    image.retain(|p, _| !p.ends_with("state.json") && !p.ends_with("run.lock"));
    image
}

fn whole() -> Ask {
    Ask { whole: true, ..Ask::default() }
}

/// A card synced from a server with scores: every record has its row's.
fn scored_card(name: &str) -> (Scratch, FatSim, String, Fake, u32) {
    let scratch = Scratch::new(name);
    let sim = card(&scratch);
    let (base, fake) = server();
    with_moods(&fake);
    let first = sync_now(&sim, &base, &rock(), &mut envs()).0;
    let outcome = ran(&first);
    assert_eq!(outcome.verified, Ok(()), "{:?}", first.notes);
    assert_eq!(note_of(&first), Some(brought(4, 4).as_str()));
    assert_eq!(calls_since(&fake), rock_call(), "a first sync's whole read asks");
    assert_eq!(tails(&sim), scored(&ROCK));
    assert_eq!(tags_shape(&sim), (1, 80));
    let generation = outcome.generation;
    (scratch, sim, base, fake, generation)
}

// ── Who is asked ────────────────────────────────────────────────────────────

/// The kit reads a row slot by slot as the API gives it: two copies of
/// mStream's `MOOD_COLUMNS`, since the API may not lean on a card's kit.
#[test]
fn the_apis_heads_are_the_kits() {
    assert_eq!(MOOD_HEADS, moods::HEADS);
    let row: MoodRow = EXAMPLE;
    let scores: &moods::Scores = &row;
    assert_eq!(moods::encode(Some(scores)), EXAMPLE_TAIL);
}

/// 5.2's fetch rule, in its order: a card for someone else first, whatever
/// else holds; then the run's word and the listener's stop; then a card
/// with scores whose files are the root's; and only then the flag.
#[test]
fn the_fetch_rule_goes_in_its_order() {
    let served = Rule { served: true, ..Rule::default() };
    assert_eq!(served.step(false, false, false), Step::Ask);
    assert_eq!(served.step(false, true, false), Step::Ask, "files changed, or a whole read");
    assert_eq!(served.step(false, true, true), Step::Keep);
    assert_eq!(served.step(true, false, false), Step::Keep, "stopped");
    assert_eq!(Rule { left_out: true, ..served }.step(false, false, false), Step::Keep);
    let gift = Rule { someone_else: true, left_out: true, ..served };
    assert_eq!(gift.step(true, true, true), Step::Clear);
    assert_eq!(Rule::default().step(false, false, false), Step::Unserved);
    assert_eq!(Rule::default().step(false, true, true), Step::Keep, "nothing to say");
}

/// A server whose `/api/` doesn't say `moodScores` (an older mStream, or one
/// that says 0) is never asked, though its route would answer, nor on a
/// whole read: the card's tails stay as they are, here none, and a note
/// says why.
#[test]
fn a_server_without_the_flag_is_never_asked() {
    for flag in [None, Some(0)] {
        let scratch = Scratch::new("moods-unflagged");
        let sim = card(&scratch);
        let (base, fake) = server();
        with_moods(&fake);
        fake.lock().unwrap().moods_flag = flag;
        let mut envs = envs();
        let first = sync_now(&sim, &base, &rock(), &mut envs).0;
        assert_eq!(ran(&first).verified, Ok(()), "{:?}", first.notes);
        let again = sync_now(&sim, &base, &whole(), &mut envs).0;
        assert_eq!(again.summary.as_ref().unwrap().listed, Listed::Whole);
        assert_eq!(ran(&again).verified, Ok(()), "{:?}", again.notes);
        for synced in [&first, &again] {
            assert_eq!(note_of(synced), Some(UNSERVED), "{flag:?}");
        }
        assert_eq!(calls_since(&fake), Vec::<Vec<String>>::new(), "{flag:?}");
        assert!(tails(&sim).values().all(|t| *t == NO_ROW), "{flag:?}");
        assert_eq!(tags_shape(&sim), (0, 72), "{flag:?}");
    }
}

/// A card synced with no scores, its server's files the same: the next
/// sync's 304 asks for every row of the server's (the first sync after an
/// upgrade needs no whole read), and its one commit lays their tails, in
/// rows of 80 at minor 1, writing no file. 5.7's example comes to its bytes.
#[test]
fn a_304_on_a_card_with_none_asks_and_commits() {
    let scratch = Scratch::new("moods-upgrade");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    let first = sync_now(&sim, &base, &rock(), &mut envs).0;
    let generation = ran(&first).generation;
    assert_eq!(tags_shape(&sim), (0, 72));
    with_moods(&fake);
    fake.lock().unwrap().moods.insert(WAV.into(), json!(EXAMPLE));

    let (again, events) = sync_now(&sim, &base, &Ask::default(), &mut envs);
    assert_eq!(again.summary.as_ref().unwrap().listed, Listed::NotModified);
    let outcome = ran(&again);
    let done = (outcome.verified.clone(), outcome.written, outcome.generation);
    assert_eq!(done, (Ok(()), 0, generation + 1), "{:?}", again.notes);
    assert_eq!(committed(&events), 1, "{events:?}");
    assert_eq!(calls_since(&fake), rock_call());
    let mut want = scored(&[ONE, TWO, OPUS]);
    want.insert(WAV.into(), EXAMPLE_TAIL);
    assert_eq!(tails(&sim), want, "the converted file's record has its source's");
    assert_eq!(tags_shape(&sim), (1, 80));
    assert_eq!(note_of(&again), Some(brought(4, 4).as_str()));
}

/// A card with scores, its server's files the same: a 304 asks nothing and
/// makes no commit. A change of scores alone waits for the next whole read
/// (5.6).
#[test]
fn a_304_on_a_card_with_scores_asks_nothing() {
    let (_scratch, sim, base, fake, generation) = scored_card("moods-304");
    let image = card_files(&sim);
    fake.lock().unwrap().moods.insert(TWO.into(), json!(EXAMPLE));
    let (again, events) = sync_now(&sim, &base, &Ask::default(), &mut envs());
    assert_eq!(again.summary.as_ref().unwrap().listed, Listed::NotModified);
    assert_eq!((ran(&again).generation, committed(&events)), (generation, 0), "{events:?}");
    assert_eq!(calls_since(&fake), Vec::<Vec<String>>::new());
    assert_eq!(card_files(&sim), image, "the card as it was");
    assert_eq!(note_of(&again), None);
}

/// A card made for someone else gets no scores (the Auto DJ index's
/// licence gate): the route isn't asked, and the tails it had go in one
/// commit, back to rows of 72 at minor 0. Made for its listener again, the
/// next sync asks, the card having none.
#[test]
fn a_card_for_someone_else_asks_nothing_and_loses_what_it_had() {
    let (_scratch, sim, base, fake, generation) = scored_card("moods-gift");
    let mut envs = envs();
    let gift = Change { for_someone_else: Some(true), ..Change::default() };
    let (synced, events) = sync_now(&sim, &base, &Ask { change: gift, ..Ask::default() }, &mut envs);
    assert_eq!(synced.summary.as_ref().unwrap().listed, Listed::NotModified);
    let outcome = ran(&synced);
    assert_eq!((outcome.verified.clone(), outcome.generation), (Ok(()), generation + 1));
    assert_eq!(committed(&events), 1, "{events:?}");
    assert_eq!(calls_since(&fake), Vec::<Vec<String>>::new());
    assert!(tails(&sim).values().all(|t| *t == NO_ROW), "{:?}", tails(&sim));
    assert_eq!(tags_shape(&sim), (0, 72));
    assert_eq!(note_of(&synced), Some(GIFT));

    // A whole read of it asks nothing either, and has nothing to clear.
    let (again, events) = sync_now(&sim, &base, &whole(), &mut envs);
    assert_eq!((ran(&again).generation, committed(&events)), (generation + 1, 0), "{events:?}");
    assert_eq!((calls_since(&fake), note_of(&again)), (Vec::<Vec<String>>::new(), Some(GIFT)));

    let mine = Change { for_someone_else: Some(false), ..Change::default() };
    let back = sync_now(&sim, &base, &Ask { change: mine, ..Ask::default() }, &mut envs).0;
    assert_eq!(back.summary.as_ref().unwrap().listed, Listed::NotModified);
    assert_eq!(ran(&back).verified, Ok(()), "{:?}", back.notes);
    assert_eq!(calls_since(&fake), rock_call());
    assert_eq!(tails(&sim), scored(&ROCK));
}

/// The scores aren't the Auto DJ index's: a card with Auto DJ off has them
/// all the same.
#[test]
fn auto_dj_off_still_gets_scores() {
    let scratch = Scratch::new("moods-dj-off");
    let sim = card(&scratch);
    let (base, fake) = server();
    with_moods(&fake);
    let off = Change { auto_dj: Some(false), ..rock().change };
    let synced = sync_now(&sim, &base, &Ask { change: off, ..rock() }, &mut envs()).0;
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    assert!(look(&sim).root.as_ref().unwrap().manifest.autodj.is_none(), "no index");
    assert_eq!(calls_since(&fake), rock_call());
    assert_eq!(tails(&sim), scored(&ROCK));
    assert_eq!(note_of(&synced), Some(brought(4, 4).as_str()));
}

/// A peer's rows aren't asked for, of the server or of the peer: they stay
/// at 8 zero bytes in rows of 80, beside the server's own with their scores.
/// A card with a peer read reads whole every run, so its server is asked
/// every run (5.6), and equal scores make no commit.
#[test]
fn a_peers_rows_stay_unscored() {
    let scratch = Scratch::new("moods-peer");
    let sim = card(&scratch);
    let (base, fake, peer, dial) = peers::world(true, false, peers::folk());
    with_moods(&fake);
    let clock = Arc::new(AtomicU64::new(peers::NOW));
    let fed = peers::fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let synced = peers::sync_fed(&sim, &base, &peers::rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.verified.clone(), outcome.records), (Ok(()), 6), "{:?}", synced.notes);
    assert_eq!(note_of(&synced), Some(brought(4, 4).as_str()), "the peer's two aren't counted");
    assert_eq!(calls_since(&fake), rock_call(), "the server's own four, in one call");
    let theirs = peers::asked_of(&peer);
    assert!(!theirs.iter().any(|a| a.contains("mood")), "{theirs:?}");

    let mut want = scored(&ROCK);
    want.insert("Folk/Dana Duo/Album/01.mp3".into(), NO_ROW);
    want.insert("Folk/Dana Duo/Album/02.flac".into(), NO_ROW);
    assert_eq!(tails(&sim), want);
    assert_eq!(tags_shape(&sim), (1, 80));

    let again = peers::sync_fed(&sim, &base, &peers::rock_and_folk(), &fed, &mut envs);
    assert_eq!(ran(&again).generation, outcome.generation, "{:?}", again.notes);
    assert_eq!(calls_since(&fake), rock_call());
}

// ── The answer ──────────────────────────────────────────────────────────────

/// One track's scores moved on the server: `--moods false` asks nothing,
/// and the next whole read asks, its one commit changing that record's tail
/// alone; the same scores again make none (5.2: the tails are laid before
/// the commit is weighed).
#[test]
fn one_changed_score_commits_once() {
    let (_scratch, sim, base, fake, generation) = scored_card("moods-changed");
    let before = tails(&sim);
    fake.lock().unwrap().moods.insert(TWO.into(), json!(EXAMPLE));
    let mut envs = envs();

    let (off, events) = sync_now(&sim, &base, &Ask { no_moods: true, ..whole() }, &mut envs);
    assert_eq!((ran(&off).generation, committed(&events)), (generation, 0), "{events:?}");
    assert_eq!((calls_since(&fake), note_of(&off)), (Vec::<Vec<String>>::new(), None));

    let (changed, events) = sync_now(&sim, &base, &whole(), &mut envs);
    assert_eq!(changed.summary.as_ref().unwrap().listed, Listed::Whole);
    assert_eq!(calls_since(&fake), rock_call());
    let outcome = ran(&changed);
    let done = (outcome.verified.clone(), outcome.written, outcome.generation);
    assert_eq!(done, (Ok(()), 0, generation + 1), "{:?}", changed.notes);
    assert_eq!(committed(&events), 1, "{events:?}");
    let mut want = before;
    want.insert(TWO.into(), EXAMPLE_TAIL);
    assert_eq!(tails(&sim), want);

    let (same, events) = sync_now(&sim, &base, &whole(), &mut envs);
    assert_eq!(calls_since(&fake), rock_call(), "a whole read asks again");
    assert_eq!((ran(&same).generation, committed(&events)), (generation + 1, 0), "{events:?}");
    assert!(events.iter().any(|e| matches!(e, Event::Unchanged)), "{events:?}");
}

/// 5.2's table, a row at a time: a null clears its record's tail; a path
/// the answer leaves out, or whose row is of the wrong length, keeps its
/// own; a row of NULLs is a tail, analysed with nothing known. A server
/// that turns `analyzeMoods` off, every path null, clears them all in one
/// commit, back to rows of 72 at minor 0, and Details' log says it had none.
#[test]
fn a_null_row_clears_and_a_path_left_out_keeps() {
    let (_scratch, sim, base, fake, generation) = scored_card("moods-table");
    let before = tails(&sim);
    {
        let mut lib = fake.lock().unwrap();
        lib.moods.insert(ONE.into(), Value::Null);
        lib.moods_left_out.insert(TWO.into());
        let nulls: MoodRow = [None; 8];
        lib.moods.insert(WAV.into(), json!(nulls));
        lib.moods.insert(OPUS.into(), json!([0.5, 0.5, 0.5]));
    }
    let mut envs = envs();
    let (synced, events) = sync_now(&sim, &base, &whole(), &mut envs);
    assert_eq!((ran(&synced).generation, committed(&events)), (generation + 1, 1), "{events:?}");
    let mut want = before.clone();
    want.insert(ONE.into(), NO_ROW);
    want.insert(WAV.into(), [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    assert_eq!(tails(&sim), want);
    assert_eq!(want[TWO], before[TWO]);
    assert_eq!(want[OPUS], before[OPUS]);
    assert_eq!(tags_shape(&sim), (1, 80));
    assert_eq!(note_of(&synced), Some(brought(1, 4).as_str()), "the NULLs' row alone");

    {
        let mut lib = fake.lock().unwrap();
        lib.moods.clear();
        lib.moods_left_out.clear();
    }
    let (off, events) = sync_now(&sim, &base, &whole(), &mut envs);
    assert_eq!((ran(&off).generation, committed(&events)), (generation + 2, 1), "{events:?}");
    assert!(tails(&sim).values().all(|t| *t == NO_ROW), "{:?}", tails(&sim));
    assert_eq!(tags_shape(&sim), (0, 72));
    assert_eq!(note_of(&off), Some(NONE));
}

/// Details' log's line of an answer (clause 51b): the rows that came, a row
/// of NULLs among them, of the paths asked for, in the Card view's digits; a
/// null or a path left out is no row. With no row at all, that the server
/// has none.
#[test]
fn an_answers_line_counts_its_rows() {
    let paths: Vec<String> = (0..4_812).map(|i| format!("Rock/Artist/{i}.mp3")).collect();
    let mut found = MoodScores::default();
    for (i, path) in paths.iter().enumerate() {
        let row = match i % 200 {
            0 => None,
            1 => Some([None; 8]),
            2 => continue,
            _ => Some(EXAMPLE),
        };
        found.tracks.insert(path.clone(), row);
    }
    let want = "mood scores for 4,762 of the card's 4,812 tracks from the server";
    assert_eq!(said(&paths, &found), want);
    for row in found.tracks.values_mut() {
        *row = None;
    }
    assert_eq!(said(&paths, &found), NONE, "every path null");
    assert_eq!(said(&paths, &MoodScores::default()), NONE, "every path left out");
}

/// The mood route failing (a 500 here) costs only the new scores: the run
/// ends well with its other work committed, each record keeps the tail it
/// had (the new file has none), and a note says why. The scores it missed
/// come at the next whole read: a 304 asks nothing of a card with scores.
#[test]
fn a_failed_fetch_keeps_old_scores_and_the_run_ends_well() {
    let (_scratch, sim, base, fake, generation) = scored_card("moods-failed");
    let before = tails(&sim);
    let new = "Rock/Artist A/Album B/03 again.mp3";
    {
        let mut lib = fake.lock().unwrap();
        let mut copy = lib.tracks[0].clone();
        (copy.path, copy.id) = (new.into(), 6);
        lib.tracks.push(copy);
        lib.revision = "1:6:1790000000006:5".into();
        lib.moods.insert(new.into(), json!(mood_row(new)));
        lib.moods_say = Some("500 Internal Server Error");
    }
    let mut envs = envs();
    let synced = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let outcome = ran(&synced);
    let done = (outcome.verified.clone(), outcome.parked.clone(), outcome.written);
    assert_eq!(done, (Ok(()), None, 1), "{:?}", synced.notes);
    assert!(outcome.generation > generation, "committed");
    assert_eq!(calls_since(&fake).len(), 1);
    let mut want = before;
    want.insert(new.into(), NO_ROW);
    assert_eq!(tails(&sim), want);
    let note = note_of(&synced).expect("a note");
    assert!(note.starts_with("no new mood scores: they didn't come ("), "{note}");
    assert!(note.contains("500"), "{note}");

    fake.lock().unwrap().moods_say = None;
    let again = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(again.summary.as_ref().unwrap().listed, Listed::NotModified);
    assert_eq!(calls_since(&fake), Vec::<Vec<String>>::new());
    let filled = sync_now(&sim, &base, &whole(), &mut envs).0;
    assert_eq!(ran(&filled).verified, Ok(()), "{:?}", filled.notes);
    let mut all = ROCK.to_vec();
    all.push(new);
    assert_eq!(tails(&sim), scored(&all));
}

/// A file fetched again over its record (another recording on the server)
/// keeps the record's tail when no new scores come: the route failing (a
/// 500 here) or `--moods false`, as every other track does (clause 51b). The
/// 304 after it asks nothing of the scored card, the tail staying; the next
/// whole read lays the server's.
#[test]
fn a_file_fetched_again_keeps_its_scores_when_none_come() {
    let (_scratch, sim, base, fake, generation) = scored_card("moods-again");
    let before = tails(&sim);
    let size_of = |sim: &FatSim, path: &str| {
        let seen = look(sim);
        let records = &seen.root.as_ref().unwrap().tags.as_ref().unwrap().records;
        let of = |e: &&Entry| e.ledger.as_ref().is_some_and(|l| l.server_path == path);
        records.iter().find(of).expect("its record").rec.size as usize
    };
    let (one, two) = (corpus("mpeg2.mp3"), corpus("flac_text.flac"));
    {
        let mut lib = fake.lock().unwrap();
        lib.tracks[0].bytes = one.clone();
        lib.revision = "1:5:1790000000006:5".into();
        lib.moods.insert(ONE.into(), json!(EXAMPLE));
        lib.moods_say = Some("500 Internal Server Error");
    }
    let mut envs = envs();
    let failed = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let outcome = ran(&failed);
    let done = (outcome.verified.clone(), outcome.parked.clone(), outcome.written);
    assert_eq!(done, (Ok(()), None, 1), "{:?}", failed.notes);
    assert!(outcome.generation > generation, "committed");
    assert_eq!(size_of(&sim, ONE), one.len(), "ONE read from its new bytes");
    assert_eq!(calls_since(&fake).len(), 1);
    assert!(note_of(&failed).is_some_and(|n| n.contains("500")), "{:?}", failed.notes);
    assert_eq!(tails(&sim), before, "ONE's among them");

    fake.lock().unwrap().moods_say = None;
    let again = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(again.summary.as_ref().unwrap().listed, Listed::NotModified);
    assert_eq!((calls_since(&fake), note_of(&again)), (Vec::<Vec<String>>::new(), None));
    assert_eq!(tails(&sim), before);

    // `--moods false`, which the real-card checks keep a run's other work
    // apart with: TWO fetched again, every tail as the card had it.
    {
        let mut lib = fake.lock().unwrap();
        lib.tracks[1].bytes = two.clone();
        lib.revision = "1:5:1790000000007:6".into();
    }
    let off = sync_now(&sim, &base, &Ask { no_moods: true, ..Ask::default() }, &mut envs).0;
    assert_eq!((ran(&off).verified.clone(), ran(&off).written), (Ok(()), 1), "{:?}", off.notes);
    assert_eq!(size_of(&sim, TWO), two.len(), "TWO read from its new bytes");
    assert_eq!((calls_since(&fake), note_of(&off)), (Vec::<Vec<String>>::new(), None));
    assert_eq!(tails(&sim), before);

    let filled = sync_now(&sim, &base, &whole(), &mut envs).0;
    assert_eq!(ran(&filled).verified, Ok(()), "{:?}", filled.notes);
    assert_eq!(calls_since(&fake), rock_call());
    let mut want = before;
    want.insert(ONE.into(), EXAMPLE_TAIL);
    assert_eq!(tails(&sim), want);
}

/// A 403 (a key or a user the route refuses) or a 404 (no route) is no
/// scores from this server: asked once, never split, the card's tails kept,
/// no commit, and a note.
#[test]
fn a_403_or_404_reads_as_none() {
    let (_scratch, sim, base, fake, generation) = scored_card("moods-refused");
    let before = tails(&sim);
    let mut envs = envs();
    for words in ["403 Forbidden", "404 Not Found"] {
        {
            let mut lib = fake.lock().unwrap();
            lib.moods_say = Some(words);
            lib.moods.insert(ONE.into(), json!(EXAMPLE));
        }
        let (synced, events) = sync_now(&sim, &base, &whole(), &mut envs);
        assert_eq!(ran(&synced).verified, Ok(()), "{words}: {:?}", synced.notes);
        assert_eq!(calls_since(&fake), rock_call(), "{words}");
        assert_eq!((ran(&synced).generation, committed(&events)), (generation, 0), "{words}");
        assert_eq!(tails(&sim), before, "{words}");
        assert_eq!(note_of(&synced), Some(REFUSED), "{words}");
    }
}
