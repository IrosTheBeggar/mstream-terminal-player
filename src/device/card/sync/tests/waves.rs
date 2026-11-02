//! A sync's waveforms, end to end (WAVEFORM-INDEX.md 7.3, 12): the fake mStream
//! of [`super`] with its waveform route ([`Wave`]), a host folder as the card,
//! and the file read back by the kit's reader. Whatever the route answers, a
//! run ends well: what came, with the old rows that still hold, or the file as
//! it was. Every shape here is made up.

use std::collections::BTreeMap;
use std::time::Instant;

use super::*;
use crate::device::card::contract::mpwf::{self, Table, bars_from};
use crate::device::card::contract::prim::path_hash;
use crate::device::card::sync::waves::{Cut as WaveCut, NO_FFMPEG_RUNS, Pace, fetch, note_of as wave_note};
use crate::device::card::waves::Ask as WaveAsk;

/// Rock's tracks, as the card's records put them in order.
const ONE: &str = "Rock/Artist A/Album B/01.mp3";
const TWO: &str = "Rock/Artist A/Album B/02.flac";
const WAV: &str = "Rock/Artist C/Album D/01.wav";
const OPUS: &str = "Rock/Artist C/Album D/02.opus";
const ROCK: [&str; 4] = [ONE, TWO, WAV, OPUS];

/// The fake's script for one track.
fn say(fake: &Fake, path: &str, script: Vec<Wave>) {
    fake.lock().unwrap().waves.insert(path.to_string(), script);
}

/// What the route has been asked for since the last look at it.
fn asked_since(fake: &Fake) -> Vec<String> {
    std::mem::take(&mut fake.lock().unwrap().waved)
}

/// The file the card's root names: its generation and its slots, the whole
/// file checked; None when the root names none.
fn named(sim: &FatSim, scratch: &Scratch) -> Option<(u32, Vec<mpwf::Slot>)> {
    let seen = look(sim);
    let comp = seen.root.as_ref()?.manifest.waves.clone()?;
    assert_eq!(seen.root.as_ref()?.waves, Some(Ok(())), "the root's file is valid");
    let bytes = std::fs::read(scratch.0.join("card/.mstream").join(comp.name())).unwrap();
    let table = Table::open(&bytes).unwrap();
    table.check().unwrap();
    let slots = (0..table.rows).map(|r| table.slot(r).unwrap()).collect();
    Some((comp.generation, slots))
}

/// The slots a file holds for these server paths: each of its card file's hash
/// and length, with the bars of that server path's peaks; in row order.
fn slots_of(sim: &FatSim, paths: &[&str]) -> Vec<mpwf::Slot> {
    let seen = look(sim);
    let records = &seen.root.as_ref().unwrap().tags.as_ref().unwrap().records;
    let of_path = |e: &&Entry| {
        e.ledger.as_ref().is_some_and(|l| paths.contains(&l.server_path.as_str()))
    };
    let slot = |e: &Entry| mpwf::Slot {
        path_hash: path_hash(&e.path),
        length_ms: e.rec.duration_ms,
        bars: bars_from(&peaks_of(&e.ledger.as_ref().unwrap().server_path)),
    };
    let mut slots: Vec<mpwf::Slot> = records.iter().filter(of_path).map(slot).collect();
    slots.sort_by_key(|s| s.path_hash);
    slots
}

fn waves_told(events: &[Event]) -> Vec<(usize, usize)> {
    let told = events.iter().filter_map(|e| match e {
        Event::Waves { done, of } => Some((*done, *of)),
        _ => None,
    });
    told.collect()
}

/// The note the run left of its waveforms, whatever it says.
fn note_of(synced: &Synced) -> Option<&String> {
    let waves = |n: &&String| n.contains("waveform");
    let mut notes = synced.notes.iter().filter(waves);
    let note = notes.next();
    assert_eq!(notes.next(), None, "one note: {:?}", synced.notes);
    note
}

/// The card's files, the settings aside: a run that left them as they are.
fn files(sim: &FatSim) -> BTreeMap<String, (u64, u32, u64)> {
    let mut image = sim.image();
    image.retain(|p, _| !p.ends_with("state.json") && !p.ends_with("run.lock"));
    image
}

/// A dry run: whether the card is offered its waveforms.
fn offered(sim: &FatSim, base: &str) -> bool {
    let ask = Ask { dry_run: true, ..Ask::default() };
    let synced = sync_now(sim, base, &ask, &mut envs()).0;
    assert!(matches!(synced.end, End::Dry), "{:?}", synced.end);
    synced.summary.as_ref().unwrap().waves_missing
}

/// A sync of a card with this much room.
fn sync_with_room(sim: &FatSim, base: &str, ask: &Ask, free: u64) -> Synced {
    let client = Arc::new(Client::new(base).unwrap().with_token(Some("tok".into())));
    let (fed, plays) = (no_peers(), Rc::new(MemStore::default()));
    let mut cards = Cards::default();
    let (space, reached, quiet) = (space(free), Reached::Elsewhere, &mut |_| {});
    sync(sim, space, client, reached, ask, &mut cards, plays, &fed, &mut envs(), quiet)
}

// ── A sync's file ───────────────────────────────────────────────────────────

#[test]
fn a_sync_writes_each_tracks_bars_and_the_root_names_the_file() {
    let scratch = Scratch::new("waves-first");
    let sim = card(&scratch);
    let (base, fake) = server();
    with_waves(&fake);
    let (synced, events) = sync_now(&sim, &base, &rock(), &mut envs());
    let outcome = ran(&synced);
    assert_eq!(outcome.verified, Ok(()), "{:?}", synced.notes);
    assert_eq!(note_of(&synced), None);
    assert!(synced.summary.as_ref().unwrap().waves_missing, "the plan said so");

    // A row for each track: its card file's hash and record's length, and its
    // server path's bars, which for a converted file are its source's.
    let (generation, slots) = named(&sim, &scratch).expect("a file");
    assert_eq!(generation, outcome.generation, "written by the final commit");
    assert_eq!(slots.len(), 4);
    assert_eq!(slots, slots_of(&sim, &ROCK));
    let seen = look(&sim);
    let records = &seen.root.as_ref().unwrap().tags.as_ref().unwrap().records;
    let converted = records.iter().find(|e| e.path == "Artist C/Album D/01.opus").unwrap();
    assert_eq!(converted.ledger.as_ref().unwrap().server_path, WAV);
    let row = slots.iter().find(|s| s.path_hash == path_hash(&converted.path)).unwrap();
    assert_eq!(row.length_ms, converted.rec.duration_ms, "the card file's own length");
    assert_eq!(row.bars, bars_from(&peaks_of(WAV)), "its source's shape");

    // Asked once each, the deselected library never.
    let mut asked = asked_since(&fake);
    assert_eq!(asked.len(), 4, "{asked:?}");
    asked.sort();
    assert_eq!(asked, ROCK);

    // Told as they came, before the commit that names them.
    assert_eq!(waves_told(&events), [(0, 4), (1, 4), (2, 4), (3, 4), (4, 4)]);
    let last = |e: &Event| matches!(e, Event::Committed { last: true, .. });
    let told = |e: &Event| matches!(e, Event::Waves { .. });
    let (commit, first) = (events.iter().position(last), events.iter().position(told));
    assert!(first < commit, "{events:?}");
    assert_eq!(State::of(&look(&sim).state).unwrap().waves_none, None);
    assert!(!offered(&sim, &base), "the card has its file");
}

#[test]
fn a_card_for_someone_else_gets_them_too_and_an_older_player_none() {
    let scratch = Scratch::new("waves-gift");
    let sim = card(&scratch);
    let (base, fake) = server();
    with_waves(&fake);
    let gift = Change { for_someone_else: Some(true), ..rock().change };
    let synced = sync_now(&sim, &base, &Ask { change: gift, ..rock() }, &mut envs()).0;
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    assert!(look(&sim).root.as_ref().unwrap().manifest.autodj.is_none(), "no Auto DJ index for a gift");
    assert_eq!(named(&sim, &scratch).unwrap().1.len(), 4, "shapes hold nothing personal");

    // A firmware before it reads no such file: the route isn't asked, and the
    // file goes (7.3's first line), said once.
    let device_txt = scratch.0.join("card/.player/device.txt");
    let text = std::fs::read_to_string(&device_txt).unwrap();
    let older: Vec<&str> = text.lines().filter(|l| !l.starts_with("read.mpwf")).collect();
    std::fs::write(&device_txt, older.join("\n") + "\n").unwrap();
    asked_since(&fake);
    let plan = sync_now(&sim, &base, &Ask { dry_run: true, ..Ask::default() }, &mut envs()).0;
    assert!(!plan.summary.as_ref().unwrap().waves_missing);
    let synced = sync_now(&sim, &base, &Ask::default(), &mut envs()).0;
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    assert_eq!(asked_since(&fake), Vec::<String>::new());
    assert!(named(&sim, &scratch).is_none());
    let note = note_of(&synced).expect("a note");
    assert_eq!(note, "no waveform index: the card's player doesn't read one");
}

/// Two card files of one recording: asked once, by the first one's server
/// path, and a row each, with its own hash.
#[test]
fn a_recording_is_asked_for_once_and_each_of_its_files_has_a_row() {
    let scratch = Scratch::new("waves-twice");
    let sim = card(&scratch);
    let (base, fake) = server();
    let copy = "Rock/Artist A/Album B/03 again.mp3";
    {
        let mut lib = fake.lock().unwrap();
        let mut twin = lib.tracks[0].clone();
        (twin.path, twin.id) = (copy.into(), 6);
        lib.tracks.push(twin);
    }
    with_waves(&fake);
    let synced = sync_now(&sim, &base, &rock(), &mut envs()).0;
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    let asked = asked_since(&fake);
    assert_eq!((asked.len(), asked.iter().any(|a| a == copy)), (4, false), "{asked:?}");
    let (_, slots) = named(&sim, &scratch).unwrap();
    assert_eq!(slots.len(), 5);
    let hash = path_hash("Artist A/Album B/03 again.mp3");
    let row = slots.iter().find(|s| s.path_hash == hash).expect("the copy's own row");
    assert_eq!(row.bars, bars_from(&peaks_of(ONE)));
}

/// The route looks for the file first: a recording's file gone from one path
/// since the listing is a 404, with the peaks there under its other path. A 503
/// is the server's, of the recording: no other path is asked.
#[test]
fn a_path_with_nothing_for_it_gives_way_to_the_recordings_other() {
    let copy = "Rock/Artist A/Album B/03 again.mp3";
    let scene = |name: &str, first: Wave, second: Wave| {
        let scratch = Scratch::new(name);
        let sim = card(&scratch);
        let (base, fake) = server();
        {
            let mut lib = fake.lock().unwrap();
            let mut twin = lib.tracks[0].clone();
            (twin.path, twin.id) = (copy.into(), 6);
            lib.tracks.push(twin);
        }
        with_waves(&fake);
        say(&fake, ONE, vec![first]);
        say(&fake, copy, vec![second]);
        let synced = sync_now(&sim, &base, &rock(), &mut envs()).0;
        assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
        let (_, slots) = named(&sim, &scratch).expect("a file");
        (scratch, slots, asked_since(&fake), synced)
    };
    let times = |asked: &[String], path: &str| asked.iter().filter(|a| *a == path).count();
    let hash = path_hash("Artist A/Album B/03 again.mp3");

    let peaks = Wave::Peaks(peaks_of(ONE));
    let (_dir, slots, asked, _) = scene("waves-other-path", Wave::Says("404 Not Found"), peaks.clone());
    assert_eq!((times(&asked, ONE), times(&asked, copy), asked.len()), (1, 1, 5), "{asked:?}");
    assert_eq!(slots.len(), 5, "both its card files");
    let row = slots.iter().find(|s| s.path_hash == hash).expect("the copy's own row");
    assert_eq!(row.bars, bars_from(&peaks_of(ONE)));
    let row = slots.iter().find(|s| s.path_hash == path_hash("Artist A/Album B/01.mp3"));
    assert_eq!(row.expect("the other's").bars, bars_from(&peaks_of(ONE)));

    // Nothing for it under either: each asked once, and neither has a row.
    let none = Wave::Says("404 Not Found");
    let (_dir, slots, asked, _) = scene("waves-no-path", none.clone(), none);
    assert_eq!((times(&asked, ONE), times(&asked, copy), asked.len()), (1, 1, 5), "{asked:?}");
    assert_eq!(slots.len(), 3, "{slots:?}");

    // The first path's own answers end it there.
    let (_dir, slots, asked, _) = scene("waves-first-path", peaks.clone(), Wave::Dropped);
    assert_eq!((times(&asked, ONE), times(&asked, copy), asked.len()), (1, 0, 4), "{asked:?}");
    assert_eq!(slots.len(), 5);
    let busy = Wave::Says("503 Service Unavailable");
    let (_dir, slots, asked, synced) = scene("waves-busy-path", busy, peaks);
    assert_eq!((times(&asked, ONE), times(&asked, copy), asked.len()), (1, 0, 4), "{asked:?}");
    assert_eq!(slots.len(), 3, "{slots:?}");
    assert!(note_of(&synced).expect("a note").contains("no ffmpeg to make 1 of the 4"));
}

#[test]
fn one_tracks_answer_stops_no_other() {
    let scratch = Scratch::new("waves-answers");
    let sim = card(&scratch);
    let (base, fake) = server();
    // The first asked has no ffmpeg behind it, and the last still has peaks.
    say(&fake, ONE, vec![Wave::Says("503 Service Unavailable")]);
    say(&fake, TWO, vec![Wave::Says("500 Internal Server Error")]);
    say(&fake, WAV, vec![Wave::Says("403 Forbidden")]);
    say(&fake, OPUS, vec![Wave::Peaks(peaks_of(OPUS))]);
    let jazz = "Jazz/Artist E/Album F/01.mp3";
    say(&fake, jazz, vec![Wave::Says("404 Not Found")]);
    let (synced, events) = sync_now(&sim, &base, &Ask::default(), &mut envs());
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    let mut asked = asked_since(&fake);
    asked.sort();
    let mut all = vec![ONE, TWO, WAV, OPUS, jazz];
    all.sort();
    assert_eq!(asked, all, "each once: no answer latches, none is asked again");
    assert_eq!(waves_told(&events).last(), Some(&(5, 5)), "every one answered");
    assert_eq!(named(&sim, &scratch).unwrap().1, slots_of(&sim, &[OPUS]));
    let note = note_of(&synced).expect("what the server couldn't make");
    assert!(note.contains("no ffmpeg to make 1 of the 5 asked for"), "{note}");
}

#[test]
fn an_answer_that_isnt_peaks_is_no_shape_and_any_length_is_one() {
    let scratch = Scratch::new("waves-lengths");
    let sim = card(&scratch);
    let (base, fake) = server();
    // One value: every bar it. And a body with no peaks in it: none.
    say(&fake, ONE, vec![Wave::Peaks(vec![200])]);
    say(&fake, TWO, vec![Wave::Peaks(Vec::new())]);
    let synced = sync_now(&sim, &base, &rock(), &mut envs()).0;
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    let (_, slots) = named(&sim, &scratch).unwrap();
    assert_eq!(slots.len(), 1, "{slots:?}");
    assert_eq!(slots[0].bars, [200; mpwf::BAR_COUNT]);
}

// ── Soft failures ───────────────────────────────────────────────────────────

/// The listener's stop during the fetch is no stop of the run: it ends well,
/// what came written, what was on its way dropped; the next sync asks for the
/// rest.
#[test]
fn a_stop_keeps_what_came_and_the_next_sync_asks_for_the_rest() {
    let scratch = Scratch::new("waves-stop");
    let sim = card(&scratch);
    let (base, fake) = server();
    let slow = |path: &str| {
        vec![Wave::Slow(Duration::from_secs(30), Box::new(Wave::Peaks(peaks_of(path))))]
    };
    say(&fake, ONE, vec![Wave::Peaks(peaks_of(ONE))]);
    for path in [TWO, WAV, OPUS] {
        say(&fake, path, slow(path));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let ask = Ask { stop: Some(stop.clone()), ..rock() };
    let t0 = Instant::now();
    let mut said = |e: Event| {
        if matches!(e, Event::Waves { done: 1, .. }) {
            stop.store(true, Ordering::SeqCst);
        }
    };
    let synced = sync_saying(&sim, &base, &ask, &mut said);
    assert!(t0.elapsed() < Duration::from_secs(20), "it waited for answers it had given up");
    let outcome = ran(&synced);
    assert_eq!((outcome.parked.clone(), outcome.verified.clone()), (None, Ok(())));
    assert_eq!(outcome.written, 4, "the music's all in");
    assert!(volume::inspect(&sim).unwrap().pending.is_none(), "the run ended");
    assert_eq!(named(&sim, &scratch).unwrap().1, slots_of(&sim, &[ONE]));
    let note = note_of(&synced).expect("a note");
    assert!(note.contains("1 of 4 came: stopped at the listener's word"), "{note}");
    assert!(note.ends_with("the next sync asks for the rest"), "{note}");
    assert!(!offered(&sim, &base), "a file that came short doesn't count as lacking");

    // The next sync, for whatever else, asks for those and no other.
    with_waves(&fake);
    asked_since(&fake);
    let again = sync_saying(&sim, &base, &Ask::default(), &mut |_| {});
    assert_eq!(ran(&again).verified, Ok(()), "{:?}", again.notes);
    let mut asked = asked_since(&fake);
    asked.sort();
    assert_eq!(asked, [TWO, WAV, OPUS]);
    assert_eq!(named(&sim, &scratch).unwrap().1, slots_of(&sim, &ROCK));
}

/// A stop set before the fetch begins, on a run with nothing to park: nothing
/// asked, and the card still lacks its file.
#[test]
fn a_stop_ahead_of_the_fetch_asks_nothing_and_the_card_still_lacks_its_file() {
    let scratch = Scratch::new("waves-stop-first");
    let sim = card(&scratch);
    let (base, fake) = server();
    let off = Ask { no_waves: true, ..rock() };
    assert_eq!(ran(&sync_now(&sim, &base, &off, &mut envs()).0).verified, Ok(()));
    with_waves(&fake);
    assert!(offered(&sim, &base));
    let stop = Arc::new(AtomicBool::new(false));
    let ask = Ask { stop: Some(stop.clone()), ..Ask::default() };
    let mut said = |e: Event| {
        if matches!(e, Event::Planned { .. }) {
            stop.store(true, Ordering::SeqCst);
        }
    };
    let synced = sync_saying(&sim, &base, &ask, &mut said);
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    assert_eq!(asked_since(&fake), Vec::<String>::new());
    assert!(named(&sim, &scratch).is_none());
    assert_eq!(State::of(&look(&sim).state).unwrap().waves_none, None, "nothing was answered");
    assert!(offered(&sim, &base), "still lacking: nothing was asked");
}

#[test]
fn a_fetch_that_fails_keeps_the_old_file_and_the_next_asks_for_the_rest() {
    let scratch = Scratch::new("waves-failed");
    let sim = card(&scratch);
    let (base, fake) = server();
    with_waves(&fake);
    let mut envs = envs();
    let first = sync_now(&sim, &base, &rock(), &mut envs).0;
    let (old_generation, old_slots) = named(&sim, &scratch).expect("a file");
    assert_eq!(old_generation, ran(&first).generation);
    let old_name = format!("waves-{old_generation:08x}.bin");
    let old_file = scratch.0.join("card/.mstream").join(&old_name);

    // A new track, and a connection that drops each time it is asked for: its
    // three tries, a commit for the track, the old file named again.
    let new = "Rock/Artist A/Album B/03.mp3";
    {
        let mut lib = fake.lock().unwrap();
        let mut more = lib.tracks[0].clone();
        (more.path, more.id, more.bytes) = (new.into(), 6, corpus("latin1.mp3"));
        lib.tracks.push(more);
        lib.revision = "1:6:1790000000006:5".into();
        lib.waves.insert(new.into(), vec![Wave::Dropped]);
        lib.waved.clear();
    }
    let dropped = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let outcome = ran(&dropped);
    assert_eq!(outcome.verified, Ok(()), "{:?}", dropped.notes);
    assert_eq!(outcome.written, 1);
    assert!(outcome.generation > old_generation);
    assert_eq!(asked_since(&fake), [new, new, new]);
    let (generation, slots) = named(&sim, &scratch).expect("the old file");
    assert_eq!((generation, slots), (old_generation, old_slots.clone()));
    assert!(old_file.exists());
    let note = note_of(&dropped).expect("a note");
    assert!(note.contains("0 of 1 came: the server stopped answering"), "{note}");
    assert!(note.ends_with("the next sync asks for the rest"), "{note}");

    // A sign-in refused: once, no more tries, and no commit for nothing.
    say(&fake, new, vec![Wave::Says("401 Unauthorized")]);
    let image = files(&sim);
    let refused = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(ran(&refused).verified, Ok(()), "{:?}", refused.notes);
    assert_eq!(asked_since(&fake), [new]);
    assert_eq!(files(&sim), image, "the card as it was");
    let note = note_of(&refused).expect("a note");
    assert!(note.contains("the server no longer takes this sign-in"), "{note}");

    // Answered at last: a new file, the old one gone.
    say(&fake, new, vec![Wave::Peaks(peaks_of(new))]);
    let answered = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let outcome = ran(&answered);
    assert_eq!(outcome.verified, Ok(()), "{:?}", answered.notes);
    assert_eq!(asked_since(&fake), [new]);
    let (generation, slots) = named(&sim, &scratch).expect("a new file");
    assert_eq!(generation, outcome.generation);
    assert_eq!(slots, slots_of(&sim, &[ONE, TWO, WAV, OPUS, new]));
    assert!(!old_file.exists(), "the clean-up took it");
    assert_eq!(note_of(&answered), None);
}

#[test]
fn a_second_run_asks_only_for_what_is_missing_and_commits_nothing_if_nothing_changed() {
    let scratch = Scratch::new("waves-second");
    let sim = card(&scratch);
    let (base, fake) = server();
    with_waves(&fake);
    say(&fake, TWO, vec![Wave::Says("404 Not Found")]);
    let mut envs = envs();
    let first = sync_now(&sim, &base, &rock(), &mut envs).0;
    let generation = ran(&first).generation;
    assert_eq!(named(&sim, &scratch).unwrap().1, slots_of(&sim, &[ONE, WAV, OPUS]));
    assert_eq!(note_of(&first), None, "a track with no shape is a plain line, nothing to say");
    asked_since(&fake);

    // Nothing new: the one with no bars is asked again, and the same answer
    // makes no commit (a commit costs the device its library's rebuild).
    let image = files(&sim);
    let (second, events) = sync_now(&sim, &base, &Ask::default(), &mut envs);
    assert_eq!(second.summary.as_ref().unwrap().listed, Listed::NotModified);
    assert_eq!(ran(&second).generation, generation);
    assert_eq!(asked_since(&fake), [TWO]);
    assert_eq!(waves_told(&events), [(0, 1), (1, 1)]);
    assert_eq!(files(&sim), image, "no commit");
    assert!(!events.iter().any(|e| matches!(e, Event::Committed { .. })), "{events:?}");

    // Answered: a commit for its row alone.
    say(&fake, TWO, vec![Wave::Peaks(peaks_of(TWO))]);
    let third = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let outcome = ran(&third);
    assert_eq!((outcome.verified.clone(), outcome.written), (Ok(()), 0));
    assert_eq!(outcome.generation, generation + 1);
    assert_eq!(asked_since(&fake), [TWO]);
    assert_eq!(named(&sim, &scratch).unwrap(), (generation + 1, slots_of(&sim, &ROCK)));

    // All in: nothing to ask, no event, no commit.
    let image = files(&sim);
    let (fourth, events) = sync_now(&sim, &base, &Ask::default(), &mut envs);
    assert_eq!(ran(&fourth).generation, generation + 1);
    assert_eq!(asked_since(&fake), Vec::<String>::new());
    assert!(waves_told(&events).is_empty(), "{events:?}");
    assert_eq!(files(&sim), image);
}

// ── The server's pace ───────────────────────────────────────────────────────

#[test]
fn a_429_and_a_503_with_a_retry_after_are_waited_out() {
    let scratch = Scratch::new("waves-after");
    let sim = card(&scratch);
    let (base, fake) = server();
    with_waves(&fake);
    say(&fake, ONE, vec![Wave::After("429 Too Many Requests", 1), Wave::Peaks(peaks_of(ONE))]);
    say(&fake, TWO, vec![Wave::After("503 Service Unavailable", 1), Wave::Peaks(peaks_of(TWO))]);
    let t0 = Instant::now();
    let synced = sync_now(&sim, &base, &rock(), &mut envs()).0;
    assert!(t0.elapsed() >= Duration::from_secs(1), "the Retry-After, kept");
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    let asked = asked_since(&fake);
    let times = |path: &str| asked.iter().filter(|a| *a == path).count();
    assert_eq!((times(ONE), times(TWO), times(WAV), times(OPUS)), (2, 2, 1, 1), "{asked:?}");
    assert_eq!(named(&sim, &scratch).unwrap().1, slots_of(&sim, &ROCK));
    assert_eq!(note_of(&synced), None);
}

#[test]
fn a_server_that_keeps_asking_to_be_left_alone_is_and_a_long_wait_isnt_waited() {
    let scratch = Scratch::new("waves-alone");
    let sim = card(&scratch);
    let (base, fake) = server();
    // Three tries of its asking, then no more.
    for path in ROCK {
        say(&fake, path, vec![Wave::After("429 Too Many Requests", 0)]);
    }
    let synced = sync_now(&sim, &base, &rock(), &mut envs()).0;
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    let asked = asked_since(&fake);
    assert!((3..=9).contains(&asked.len()), "{asked:?}");
    assert!(named(&sim, &scratch).is_none());
    let note = note_of(&synced).expect("a note");
    assert!(note.contains("0 of 4 came: the server asks to be left alone"), "{note}");

    // An hour: the next run asks, with the card let go now.
    for path in ROCK {
        say(&fake, path, vec![Wave::After("429 Too Many Requests", 3600)]);
    }
    let t0 = Instant::now();
    let synced = sync_now(&sim, &base, &Ask::default(), &mut envs()).0;
    assert!(t0.elapsed() < Duration::from_secs(60), "it didn't wait an hour");
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    assert!(note_of(&synced).expect("a note").contains("left alone"));
}

/// The fetch alone: asks of the fake's tracks, hashed by their turn.
fn asks_of(paths: &[String]) -> Vec<WaveAsk> {
    let ask = |(i, path): (usize, &String)| WaveAsk {
        hash: [i as u8 + 1; 16],
        server_paths: vec![path.clone()],
        server: 0,
    };
    paths.iter().enumerate().map(ask).collect()
}

#[test]
fn past_its_budget_a_fetch_keeps_what_came_and_says_so() {
    let (base, fake) = server();
    let paths: Vec<String> = ROCK.map(str::to_string).to_vec();
    say(&fake, ONE, vec![Wave::Peaks(peaks_of(ONE))]);
    for path in [TWO, WAV, OPUS] {
        let late = Wave::Slow(Duration::from_secs(30), Box::new(Wave::Peaks(peaks_of(path))));
        say(&fake, path, vec![late]);
    }
    let client = Client::new(&base).unwrap();
    let pace = Pace { budget: Duration::from_millis(1500), decode: Duration::from_secs(60) };
    let mut told = Vec::new();
    let t0 = Instant::now();
    let fetched = fetch(&client, &asks_of(&paths), None, pace, &mut |e| told.push(e));
    assert!(t0.elapsed() < Duration::from_secs(20), "the budget ends what is on its way");
    assert_eq!(fetched.shapes.len(), 1);
    assert_eq!(fetched.shapes[&[1u8; 16]], bars_from(&peaks_of(ONE)));
    assert!(!fetched.whole, "the next run asks for the rest");
    let note = fetched.note.expect("a note");
    assert!(note.contains("1 of 4 came: a run gives them half an hour"), "{note}");
    assert_eq!(waves_told(&told), [(0, 4), (1, 4)]);
}

/// A stop, or the budget running out, with every one answered cut nothing: no
/// note says the next sync asks for the rest.
#[test]
fn a_stop_or_the_budget_past_the_last_answer_cuts_nothing() {
    let (base, fake) = server();
    let paths = vec![ONE.to_string()];
    let client = Client::new(&base).unwrap();

    // The stop as the last answer is told: the worker's next look comes after.
    say(&fake, ONE, vec![Wave::Peaks(peaks_of(ONE))]);
    let stop = AtomicBool::new(false);
    let pace = Pace { budget: Duration::from_secs(60), decode: Duration::from_secs(60) };
    let mut said = |e: Event| {
        if matches!(e, Event::Waves { done: 1, .. }) {
            stop.store(true, Ordering::SeqCst);
        }
    };
    let fetched = fetch(&client, &asks_of(&paths), Some(&stop), pace, &mut said);
    assert!(fetched.whole && fetched.shapes.len() == 1, "{:?}", fetched.note);
    assert_eq!(fetched.note, None);

    // The budget out with the answer on its way, between two of its looks at
    // the clock, each 100 ms: the answer lands all the same. A machine slow
    // enough for a look to come first drops it, and rightly says so.
    let late = Wave::Slow(Duration::from_millis(170), Box::new(Wave::Peaks(peaks_of(ONE))));
    say(&fake, ONE, vec![late]);
    let pace = Pace { budget: Duration::from_millis(140), decode: Duration::from_secs(60) };
    let fetched = fetch(&client, &asks_of(&paths), None, pace, &mut |_| {});
    assert_eq!(fetched.note.is_some(), !fetched.whole, "{:?}", fetched.note);

    // However it came about: a recording with none to give among them too, and
    // what the server had no ffmpeg to make still said.
    assert_eq!(wave_note(Some(&WaveCut::Budget), 4, 3, 0, 4), None);
    assert_eq!(wave_note(Some(&WaveCut::Stopped), 4, 3, 0, 4), None);
    let note = wave_note(Some(&WaveCut::Budget), 3, 3, 0, 4).expect("one left");
    assert!(note.contains("3 of 4 came"), "{note}");
    let note = wave_note(Some(&WaveCut::Budget), 4, 3, 1, 4).expect("one with no ffmpeg");
    assert!(note.contains("no ffmpeg to make 1 of the 4"), "{note}");
}

#[test]
fn three_ask_at_once_and_two_once_an_answer_was_a_decode() {
    let (base, fake) = server();
    let paths: Vec<String> = (0..12).map(|i| format!("Rock/made-up/{i:02}.mp3")).collect();
    let decode = |path: &str| {
        Wave::Slow(Duration::from_millis(80), Box::new(Wave::Peaks(peaks_of(path))))
    };
    for (i, path) in paths.iter().enumerate() {
        // The first three answer only with all three in flight.
        let wave = match i {
            0..3 => Wave::Together(3, Box::new(decode(path))),
            _ => decode(path),
        };
        say(&fake, path, vec![wave]);
    }
    let client = Client::new(&base).unwrap();
    let pace = Pace { budget: Duration::from_secs(60), decode: Duration::from_millis(50) };
    let t0 = Instant::now();
    let fetched = fetch(&client, &asks_of(&paths), None, pace, &mut |_| {});
    assert!(t0.elapsed() < Duration::from_secs(5), "three were in flight at once");
    assert!(fetched.whole && fetched.note.is_none(), "{:?}", fetched.note);
    assert_eq!(fetched.shapes.len(), 12);
    let seen = fake.lock().unwrap().wave_seen.clone();
    assert!(seen.iter().all(|&n| n <= 3), "{seen:?}");
    assert!(seen[seen.len() - 3..].iter().all(|&n| n <= 2), "one went after the decode: {seen:?}");
}

#[test]
fn nothing_to_ask_asks_nothing_and_says_nothing() {
    let (base, fake) = server();
    let client = Client::new(&base).unwrap();
    let mut told = Vec::new();
    let pace = Pace { budget: Duration::from_secs(60), decode: Duration::from_secs(2) };
    let fetched = fetch(&client, &[], None, pace, &mut |e| told.push(e));
    assert!(fetched.whole && fetched.shapes.is_empty() && fetched.note.is_none());
    assert!(told.is_empty(), "{told:?}");
    assert_eq!(asked_since(&fake), Vec::<String>::new());
}

#[test]
fn the_hidden_command_says_the_waveforms_now_and_then() {
    let line = |done, of| event_line(&Event::Waves { done, of });
    assert_eq!(line(0, 1200).as_deref(), Some("waveforms: 0 of 1200"));
    assert_eq!(line(1, 1200), None);
    assert_eq!(line(500, 1200).as_deref(), Some("waveforms: 500 of 1200"));
    assert_eq!(line(1200, 1200).as_deref(), Some("waveforms: 1200 of 1200"));
}

// ── The run around the fetch ────────────────────────────────────────────────

/// The fetch may take half an hour, with the final commit's first write behind
/// it: what a run copied is named by a root before the first ask. A run that
/// copied nothing has nothing to name.
#[test]
fn a_run_names_what_it_copied_before_it_asks_for_waveforms() {
    let scratch = Scratch::new("waves-named");
    let sim = card(&scratch);
    let (base, fake) = server();
    with_waves(&fake);
    let mut asking = None;
    let mut said = |e: Event| {
        if matches!(e, Event::Waves { done: 0, .. }) {
            asking = Some(look(&sim));
        }
    };
    let synced = sync_saying(&sim, &base, &rock(), &mut said);
    let outcome = ran(&synced);
    assert_eq!(outcome.verified, Ok(()), "{:?}", synced.notes);
    let seen = asking.expect("it asked");
    let root = seen.root.as_ref().expect("a root");
    assert!(!root.manifest.is_final(), "a checkpoint");
    assert_eq!(root.tags.as_ref().unwrap().records.len(), 4, "the run's files, all of them");
    assert_eq!(root.manifest.header.generation + 1, outcome.generation, "the final commit next");
    assert!(seen.pending.is_some(), "the run isn't over");
    assert!(volume::inspect(&sim).unwrap().pending.is_none(), "now it is");

    // Nothing copied, and one to ask for: the final commit alone.
    say(&fake, TWO, vec![Wave::Says("404 Not Found")]);
    let off = sync_now(&sim, &base, &Ask { no_waves: true, ..Ask::default() }, &mut envs()).0;
    assert_eq!(ran(&off).verified, Ok(()));
    let (filled, events) = sync_now(&sim, &base, &Ask::default(), &mut envs());
    assert_eq!(ran(&filled).verified, Ok(()), "{:?}", filled.notes);
    assert!(!waves_told(&events).is_empty());
    let checkpoint = |e: &Event| matches!(e, Event::Committed { last: false, .. });
    assert!(!events.iter().any(checkpoint), "{events:?}");
    assert_eq!(named(&sim, &scratch).unwrap().1, slots_of(&sim, &[ONE, WAV, OPUS]));
}

// ── What a look says ────────────────────────────────────────────────────────

/// A card with nothing else to do is offered its waveforms on a look, and a
/// sync adds them. One whose server has none to give isn't offered them again:
/// the card can't remember what was asked of whom in its file, but the look
/// doesn't come back to the same sync for ever.
#[test]
fn a_card_that_lacks_them_is_offered_them_until_its_server_has_answered_none() {
    let scratch = Scratch::new("waves-lacks");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    // In step with its server, no file (`--waves false`), the route not asked.
    let off = Ask { no_waves: true, ..rock() };
    let plan = sync_now(&sim, &base, &Ask { dry_run: true, ..off.clone() }, &mut envs).0;
    assert!(!plan.summary.as_ref().unwrap().waves_missing, "left out, not lacking");
    let synced = sync_now(&sim, &base, &off, &mut envs).0;
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    assert_eq!(asked_since(&fake), Vec::<String>::new());
    assert!(named(&sim, &scratch).is_none());
    assert_eq!(note_of(&synced), None, "as it was told");

    // A look: nothing new, but something waits, and the look writes nothing.
    let image = sim.image();
    let plan = sync_now(&sim, &base, &Ask { dry_run: true, ..Ask::default() }, &mut envs).0;
    let summary = plan.summary.as_ref().unwrap();
    assert_eq!((summary.fetches, summary.removals.len()), (0, 0));
    assert!(summary.waves_missing);
    assert_eq!(asked_since(&fake), Vec::<String>::new(), "a look asks the route nothing");
    let mut after = sim.image();
    after.retain(|p, _| !p.ends_with("run.lock"));
    let mut before = image;
    before.retain(|p, _| !p.ends_with("run.lock"));
    assert_eq!(after, before);
    let printed = synced_json(&plan, true);
    assert_eq!(printed["plan"]["wavesMissing"], json!(true), "{printed}");

    // A server with none to give: every one asked and none came, the file not
    // made, and the card in step from now on.
    let generation = look(&sim).root.as_ref().unwrap().manifest.header.generation;
    let image = files(&sim);
    let none = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(ran(&none).verified, Ok(()), "{:?}", none.notes);
    assert_eq!(asked_since(&fake).len(), 4);
    assert_eq!(files(&sim), image, "no commit");
    let note = note_of(&none).expect("a note");
    assert_eq!(note, "no waveform index: the server has no peaks for the card's files");
    assert_eq!(State::of(&look(&sim).state).unwrap().waves_none, Some(generation));
    assert!(!offered(&sim, &base));

    // Cut short with nothing gained, it stays offered: those weren't answers.
    for path in ROCK {
        say(&fake, path, vec![Wave::Dropped]);
    }
    let cut = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(ran(&cut).verified, Ok(()), "{:?}", cut.notes);
    assert!(note_of(&cut).expect("a note").contains("the server stopped answering"));
    assert_eq!(State::of(&look(&sim).state).unwrap().waves_none, None);
    assert!(offered(&sim, &base));
    asked_since(&fake);

    // A sync, for any reason, asks again, and takes what the server has now.
    with_waves(&fake);
    let filled = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let outcome = ran(&filled);
    assert_eq!((outcome.verified.clone(), outcome.generation), (Ok(()), generation + 1));
    assert_eq!(named(&sim, &scratch).unwrap().1, slots_of(&sim, &ROCK));
    assert_eq!(State::of(&look(&sim).state).unwrap().waves_none, None);
    assert!(!offered(&sim, &base));

    // `--waves false` takes the file away again.
    let synced = sync_now(&sim, &base, &Ask { no_waves: true, ..Ask::default() }, &mut envs).0;
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    assert!(named(&sim, &scratch).is_none());
    assert!(offered(&sim, &base));
}

/// A 503 for every track is no server with none to give: it may be one still
/// bringing its ffmpeg up, or back from a reboot, and a look goes on offering the
/// card its waveforms. A host that keeps saying so stops being offered them after
/// a few runs: the look doesn't come back to the same sync for ever.
#[test]
fn a_server_with_no_ffmpeg_yet_stays_offered_until_it_has_said_so_enough() {
    let in_step = |name: &str| {
        let scratch = Scratch::new(name);
        let sim = card(&scratch);
        let (base, fake) = server();
        let off = Ask { no_waves: true, ..rock() };
        assert_eq!(ran(&sync_now(&sim, &base, &off, &mut envs()).0).verified, Ok(()));
        for path in ROCK {
            say(&fake, path, vec![Wave::Says("503 Service Unavailable")]);
        }
        (scratch, sim, base, fake)
    };
    let sync_again = |sim: &FatSim, base: &str| {
        let synced = sync_now(sim, base, &Ask::default(), &mut envs()).0;
        assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
        synced
    };
    let state = |sim: &FatSim| {
        let state = State::of(&look(sim).state).unwrap();
        (state.waves_none, state.waves_no_ffmpeg)
    };

    // A minute later it has its ffmpeg: the look still offers, and a sync takes
    // them.
    let (scratch, sim, base, fake) = in_step("waves-ffmpeg-soon");
    let image = files(&sim);
    let synced = sync_again(&sim, &base);
    assert_eq!(files(&sim), image, "no commit, and no file");
    let note = note_of(&synced).expect("a note");
    assert!(note.contains("no ffmpeg to make 4 of the 4 asked for: no shape yet"), "{note}");
    assert_eq!(state(&sim), (None, 1));
    assert!(offered(&sim, &base), "nothing said it has none to give");
    with_waves(&fake);
    assert_eq!(note_of(&sync_again(&sim, &base)), None);
    assert_eq!(named(&sim, &scratch).unwrap().1, slots_of(&sim, &ROCK));
    assert_eq!(state(&sim), (None, 0), "over");
    assert!(!offered(&sim, &base));

    // One track with none to give among them says no more than the others do.
    let (_scratch, sim, base, fake) = in_step("waves-ffmpeg-some");
    say(&fake, ONE, vec![Wave::Says("404 Not Found")]);
    sync_again(&sim, &base);
    assert_eq!(state(&sim), (None, 1));
    assert!(offered(&sim, &base));

    // A host that has none: offered for that many runs in a row, then quiet.
    let (scratch, sim, base, fake) = in_step("waves-ffmpeg-never");
    let generation = look(&sim).root.as_ref().unwrap().manifest.header.generation;
    for runs in 1..NO_FFMPEG_RUNS {
        assert!(offered(&sim, &base), "run {runs}");
        sync_again(&sim, &base);
        assert_eq!(state(&sim), (None, runs));
    }
    assert!(offered(&sim, &base));
    sync_again(&sim, &base);
    assert_eq!(state(&sim), (Some(generation), NO_FFMPEG_RUNS));
    assert!(!offered(&sim, &base));

    // A sync, for any reason, still asks, and takes what the server has now.
    with_waves(&fake);
    sync_again(&sim, &base);
    assert_eq!(named(&sim, &scratch).unwrap().1, slots_of(&sim, &ROCK));
    assert_eq!(state(&sim), (None, 0));
    assert!(!offered(&sim, &base));
}

/// A card that has the room counts the file, and a card with nothing else to
/// do that hasn't goes without: a plain line is no reason to say it doesn't
/// fit, and a sync doesn't fill it to find that out.
#[test]
fn a_card_with_nothing_else_to_do_and_no_room_goes_without_and_fits() {
    let scratch = Scratch::new("waves-full");
    let sim = card(&scratch);
    let (base, fake) = server();
    with_waves(&fake);
    let off = Ask { no_waves: true, ..rock() };
    assert_eq!(ran(&sync_with_room(&sim, &base, &off, 32 << 30)).verified, Ok(()));
    assert!(named(&sim, &scratch).is_none());
    asked_since(&fake);

    let dry = Ask { dry_run: true, ..Ask::default() };
    let room = sync_with_room(&sim, &base, &dry, 32 << 30);
    let summary = room.summary.as_ref().unwrap();
    assert!(summary.waves_missing);
    assert!(summary.fit.is_some_and(|f| f.needed > 0 && f.fits()), "{:?}", summary.fit);

    let full = sync_with_room(&sim, &base, &dry, 512);
    let summary = full.summary.as_ref().unwrap();
    assert!(!summary.waves_missing, "nothing to offer");
    let fit = summary.fit.expect("a fit");
    assert_eq!((fit.needed, fit.fits()), (0, true), "{fit:?}");

    let image = files(&sim);
    let synced = sync_with_room(&sim, &base, &Ask::default(), 512);
    assert!(matches!(synced.end, End::Ran(_)), "{:?}", synced.end);
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    assert_eq!(asked_since(&fake), Vec::<String>::new(), "nothing to put them in");
    assert_eq!(files(&sim), image);
    assert_eq!(note_of(&synced).expect("a note"), "no waveform index: the card has no room for it");
    // With room again, they come.
    assert_eq!(ran(&sync_with_room(&sim, &base, &Ask::default(), 32 << 30)).verified, Ok(()));
    assert_eq!(named(&sim, &scratch).unwrap().1, slots_of(&sim, &ROCK));
}
