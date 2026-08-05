//! The app's tests, moved out whole (audit #54): they had grown to 43%
//! of app.rs. `use super::*;` keeps them exactly the tests they were.

use super::*;
use crate::api::types::{DirEntry, FileEntry, FileMetadata, SearchResults, TrackMetadata};

fn track(path: &str) -> Track {
    Track { filepath: path.to_string(), metadata: TrackMetadata::default() }
}

#[test]
fn removing_the_only_track_while_it_plays_reports_it_was_current() {
    // The emptied-queue outcome has to count as removing the current row —
    // a one-track queue has nowhere else to point, and reporting false
    // here would leave playback running on a track no longer queued.
    let mut queue = Queue::default();
    queue.replace(vec![track("solo")]);
    queue.start(0);
    assert!(queue.remove(0), "the emptied queue took the playing row with it");
    assert_eq!(queue.current, None);
}

/// The source the app actually asked for. A status has to name it to be
/// about the track now playing, so tests cannot invent one.
fn played_url(effects: &[Effect]) -> String {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Audio(AudioCmd::Play { url, .. }) => Some(url.clone()),
            _ => None,
        })
        .expect("expected a play effect")
}

/// What the audio thread would send when the track those effects started
/// runs out. Built from the play rather than named by hand for the same
/// reason [`played_url`] exists: an event about a source nobody asked for
/// is one the app is entitled to ignore.
fn ended(effects: &[Effect]) -> Event {
    Event::TrackEnded { source: played_url(effects) }
}

fn failed(effects: &[Effect], error: &str) -> Event {
    Event::PlaybackFailed { source: played_url(effects), error: error.to_string() }
}

fn track_by(path: &str, artist: &str) -> Track {
    Track {
        filepath: path.to_string(),
        metadata: TrackMetadata { artist: Some(artist.to_string()), ..Default::default() },
    }
}

/// A session against a fully-featured server. Capabilities are set
/// explicitly because they change what the UI offers — a default (empty)
/// set would silently be testing the degraded path.
fn connected_app() -> App {
    let mut app = App::new(Some("http://host:3000".into()), Some("tok".into()), None);
    app.connected = true;
    app.capabilities = crate::api::types::Capabilities {
        discovery: true,
        discovery_path: true,
        discovery_p2p: false,
        federation_discovery: false,
    };
    app
}

fn listing(path: &str, dirs: &[&str], files: &[&str]) -> DirListing {
    DirListing {
        path: path.to_string(),
        directories: dirs.iter().map(|d| DirEntry { name: (*d).to_string() }).collect(),
        files: files
            .iter()
            .map(|f| FileEntry {
                name: (*f).to_string(),
                kind: Some("mp3".into()),
                ..Default::default()
            })
            .collect(),
    }
}

#[test]
fn listing_becomes_entries_with_qualified_paths() {
    let entries = entries_from_listing(&listing("/lib/Artist/", &["Album"], &["song.mp3"]), "");
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0], Entry::Parent);
    assert!(matches!(&entries[1], Entry::Dir { path, .. } if path == "lib/Artist/Album"));
    assert!(
        matches!(&entries[2], Entry::Track { track, .. } if track.filepath == "lib/Artist/song.mp3")
    );
}

/// Connect for real, so the library list comes from a ping rather than
/// being poked into the field the browser reads.
fn app_with_libraries(vpaths: &[&str]) -> App {
    let mut app = connected_app();
    app.apply_event(Event::Connected {
        server: "http://host:3000".into(),
        id: "http://host:3000".into(),
        username: None,
        token: None,
        ping: Box::new(crate::api::types::Ping {
            vpaths: vpaths.iter().map(|v| (*v).to_string()).collect(),
            ..Default::default()
        }),
    });
    app
}

#[test]
fn the_only_library_is_the_top_of_the_browser() {
    let mut app = app_with_libraries(&["library"]);
    app.path = "library".into();
    app.apply_event(Event::Listing(Box::new(listing("library", &["Artist"], &[]))));

    // Nothing above it: the list of libraries would be this one row.
    assert!(!app.files.entries.contains(&Entry::Parent), "{:?}", app.files.entries);
    assert!(app.handle_action(Action::Back).is_empty(), "back asks for nothing");
    assert_eq!(app.path, "library", "and stays put");

    // A folder inside it still has its way back.
    app.handle_action(Action::Activate);
    assert_eq!(app.path, "library/Artist");
    app.apply_event(Event::Listing(Box::new(listing(
        "library/Artist",
        &["Album"],
        &[],
    ))));
    assert!(app.files.entries.contains(&Entry::Parent));
    // No request: the trail already holds the listing we came through.
    assert!(app.handle_action(Action::Back).is_empty());
    assert_eq!(app.path, "library");
    assert!(!app.files.entries.contains(&Entry::Parent), "and the top has no way up");
}

#[test]
fn several_libraries_keep_the_list_of_them_reachable() {
    // With a choice to make, the top-level listing is worth going back to.
    let mut app = app_with_libraries(&["music", "podcasts"]);
    app.path = "music".into();
    app.apply_event(Event::Listing(Box::new(listing("music", &["Artist"], &[]))));

    assert!(app.files.entries.contains(&Entry::Parent));
    assert_eq!(
        app.handle_action(Action::Back),
        vec![Effect::Api(ApiCmd::Browse(String::new()))]
    );
}

fn type_filter(app: &mut App, text: &str) {
    app.handle_action(Action::StartFilter);
    for c in text.chars() {
        app.handle_action(Action::Input(c));
    }
}

fn labels(app: &App) -> Vec<&str> {
    app.pane().entries.iter().map(Entry::label).collect()
}

#[test]
fn a_filter_narrows_the_list_without_losing_the_way_out() {
    let mut app = connected_app();
    app.apply_event(Event::Listing(Box::new(listing(
        "/lib/",
        &["Bassnectar", "Basshunter", "Portishead"],
        &["bass solo.mp3"],
    ))));

    type_filter(&mut app, "BASS");
    // Case folds both ways, files and folders alike, and `..` is not a
    // result so it is never filtered away.
    assert_eq!(labels(&app), vec!["..", "Bassnectar", "Basshunter", "bass solo.mp3"]);
    assert_eq!(app.pane().counts(), (3, 4));
    assert_eq!(app.input_mode(), InputMode::Editing, "the prompt has the keys");

    // Narrowing further happens on the keystroke, with nothing to submit.
    app.handle_action(Action::Input('h'));
    assert_eq!(labels(&app), vec!["..", "Basshunter"]);

    // Backspacing widens again, from the list that was never thrown away.
    app.handle_action(Action::Backspace);
    assert_eq!(labels(&app).len(), 4);
}

#[test]
fn a_filter_survives_being_typed_but_not_a_new_listing() {
    let mut app = connected_app();
    app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Alpha", "Beta"], &[]))));

    type_filter(&mut app, "alp");
    app.handle_action(Action::Submit);
    assert!(!app.filtering, "the prompt closes");
    assert_eq!(app.pane().filter, "alp", "and what was typed stays");
    assert_eq!(app.input_mode(), InputMode::Normal, "the list has the keys back");
    assert_eq!(labels(&app), vec!["..", "Alpha"]);

    // A different list is not the list the filter was typed against.
    app.handle_action(Action::Down); // onto Alpha, the only row it left
    app.handle_action(Action::Activate);
    app.apply_event(Event::Listing(Box::new(listing("/lib/Alpha/", &["One", "Two"], &[]))));
    assert!(app.pane().filter.is_empty());
    assert_eq!(labels(&app), vec!["..", "One", "Two"]);
}

#[test]
fn escaping_a_filter_puts_the_whole_list_back() {
    let mut app = connected_app();
    app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Alpha", "Beta"], &[]))));

    type_filter(&mut app, "alp");
    app.handle_action(Action::Cancel);
    assert!(!app.filtering);
    assert!(app.pane().filter.is_empty());
    assert_eq!(labels(&app), vec!["..", "Alpha", "Beta"]);

    // Backspacing past the start leaves too, having changed nothing.
    type_filter(&mut app, "");
    app.handle_action(Action::Backspace);
    assert!(!app.filtering);
    assert_eq!(labels(&app).len(), 3);
}

#[test]
fn the_column_behind_a_filtered_pick_holds_the_whole_folder() {
    // The filter found the row; the folder it was in is the context worth
    // keeping. Coming back out has nothing hidden and no filter left to
    // explain why.
    let mut app = connected_app();
    app.apply_event(Event::Listing(Box::new(listing(
        "/lib/",
        &["Alpha", "Beta", "Betamax"],
        &[],
    ))));

    type_filter(&mut app, "betam");
    assert_eq!(labels(&app), vec!["..", "Betamax"]);
    app.handle_action(Action::Submit);
    app.handle_action(Action::Activate);
    assert_eq!(app.path, "lib/Betamax");

    let step = &app.files.trail[0];
    assert_eq!(
        step.entries.iter().map(Entry::label).collect::<Vec<_>>(),
        vec!["..", "Alpha", "Beta", "Betamax"]
    );
    assert_eq!(step.entries[step.chosen].label(), "Betamax", "marked where we went in");

    app.apply_event(Event::Listing(Box::new(listing("/lib/Betamax/", &["Tape"], &[]))));
    app.handle_action(Action::Back);
    assert_eq!(labels(&app), vec!["..", "Alpha", "Beta", "Betamax"]);
    assert!(app.pane().filter.is_empty(), "and no filter came back with it");
}

#[test]
fn every_tab_filters_its_own_list() {
    use crate::tui::worker::{LibraryData, LibraryNode};
    let mut app = connected_app();
    app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Alpha", "Beta"], &[]))));
    type_filter(&mut app, "alp");
    app.handle_action(Action::Submit);

    app.handle_action(Action::SelectTab(1));
    app.apply_event(Event::Library {
        dest: Tab::Library,
        node: LibraryNode::Root,
        data: LibraryData::Artists(vec!["Bassnectar".into(), "Portishead".into()]),
    });
    assert!(app.pane().filter.is_empty(), "a tab does not inherit another's filter");
    type_filter(&mut app, "port");
    app.handle_action(Action::Submit);
    assert_eq!(labels(&app), vec!["..", "Portishead"]);

    app.handle_action(Action::SelectTab(0));
    assert_eq!(app.pane().filter, "alp", "and keeps its own when you come back");
    assert_eq!(labels(&app), vec!["..", "Alpha"]);
}

#[test]
fn root_listing_has_no_parent_entry() {
    let entries = entries_from_listing(&listing("/", &["lib"], &[]), "");
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0], Entry::Dir { .. }));
}

#[test]
fn navigating_into_and_back_out_of_directories() {
    let mut app = connected_app();
    app.apply_event(Event::Listing(Box::new(listing("/", &["lib"], &[]))));

    let effects = app.handle_action(Action::Activate);
    assert_eq!(effects, vec![Effect::Api(ApiCmd::Browse("lib".into()))]);
    assert_eq!(app.path, "lib");

    app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Artist"], &[]))));
    app.handle_action(Action::Down); // onto "Artist"
    let effects = app.handle_action(Action::Activate);
    assert_eq!(effects, vec![Effect::Api(ApiCmd::Browse("lib/Artist".into()))]);

    // Back up one level. The listing is already in hand, so this costs
    // nothing and lands on the row we came through rather than the top.
    let effects = app.handle_action(Action::Back);
    assert!(effects.is_empty(), "going back asks the server for nothing");
    assert_eq!(app.path, "lib");
    assert_eq!(app.files.entries.len(), 2, "'..' and Artist, restored");
    assert_eq!(app.files.state.selected(), Some(1), "back on Artist");

    let effects = app.handle_action(Action::Back);
    assert!(effects.is_empty());
    assert_eq!(app.path, "");
    // At the root there is nowhere further up.
    assert!(app.handle_action(Action::Back).is_empty());
}

#[test]
fn a_listing_for_a_folder_already_left_does_not_teleport_the_view() {
    let mut app = connected_app();
    app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Alpha", "Beta"], &[]))));

    // Into Alpha on a slow link, and straight back out before it answers.
    app.handle_action(Action::Activate);
    assert_eq!(app.path, "lib/Alpha");
    app.handle_action(Action::Back);
    assert_eq!(app.path, "lib");

    // Alpha answers now. It used to be applied whatever had happened in
    // the meantime, putting its rows and its path back on a screen the
    // user had left — and the trail beside them still described the way
    // out of lib, so the two disagreed with nothing to settle it.
    app.apply_event(Event::Listing(Box::new(listing("/lib/Alpha/", &["One", "Two"], &[]))));
    assert_eq!(app.path, "lib");
    assert_eq!(labels(&app), vec!["..", "Alpha", "Beta"]);
}

#[test]
fn the_listing_that_opens_the_browser_is_taken_whatever_it_says() {
    // `~` is a question only the server can answer, and a remembered path
    // is a hope about how it spells things. Either way the first listing
    // is the one that says where we are, and there is nothing on screen
    // yet for it to overwrite.
    let mut app = connected_app();
    app.path = "music/Artist".into();
    app.apply_event(Event::Listing(Box::new(listing("/library/", &["Alpha"], &[]))));
    assert_eq!(app.path, "library");

    // And only the first: from here on the rule is the ordinary one.
    app.apply_event(Event::Listing(Box::new(listing("/elsewhere/", &["Beta"], &[]))));
    assert_eq!(app.path, "library");
    assert_eq!(labels(&app), vec!["..", "Alpha"]);
}

#[test]
fn enter_on_a_track_queues_the_directory_and_starts_there() {
    let mut app = connected_app();
    app.apply_event(Event::Listing(Box::new(listing(
        "/lib/",
        &["sub"],
        &["a.mp3", "b.mp3", "c.mp3"],
    ))));

    // Rows: [.., sub, a, b, c]. The cursor starts on "sub", not on "..".
    assert_eq!(app.files.state.selected(), Some(1));
    for _ in 0..2 {
        app.handle_action(Action::Down); // onto "b"
    }
    let effects = app.handle_action(Action::Activate);

    assert_eq!(app.queue.items.len(), 3, "only playable rows are queued");
    assert_eq!(app.queue.current, Some(1), "playback starts at the selected track");
    match &effects[0] {
        Effect::Audio(AudioCmd::Play { url, .. }) => {
            assert_eq!(url, "http://host:3000/media/lib/b.mp3?token=tok");
        }
        other => panic!("expected a play effect, got {other:?}"),
    }
}

#[test]
fn an_end_from_the_track_left_behind_does_not_walk_the_queue_on() {
    // The audio thread notices an end by polling, so an end and a skip
    // can cross in the post. Taking one for the other costs the user the
    // track they just chose: it is passed over without a note played.
    let mut app = connected_app();
    app.queue.replace(vec![track("a"), track("b"), track("c")]);
    let first = app.handle_action(Action::PlayPause);
    app.handle_action(Action::NextTrack);
    assert_eq!(app.queue.current, Some(1));

    let effects = app.apply_event(ended(&first));
    assert!(effects.is_empty(), "nothing is asked of the audio thread");
    assert_eq!(app.queue.current, Some(1), "b is still what we are on, not c");
}

#[test]
fn queue_advances_on_track_end_and_stops_at_the_end() {
    let mut app = connected_app();
    app.queue.replace(vec![track("lib/a.mp3"), track("lib/b.mp3")]);
    let effects = app.handle_action(Action::PlayPause);

    let effects = app.apply_event(ended(&effects));
    assert_eq!(app.queue.current, Some(1));
    assert!(matches!(effects[0], Effect::Audio(AudioCmd::Play { .. })));

    // End of the last track with repeat off: stop, don't wrap.
    let effects = app.apply_event(ended(&effects));
    assert_eq!(effects, vec![Effect::Audio(AudioCmd::Stop)]);
    assert_eq!(app.queue.current, None);
    assert!(app.now_playing.is_none());
}

#[test]
fn a_shuffled_queue_still_ends_when_repeat_is_off() {
    // Shuffle has no position to run out of, so the end has to be
    // counted: it used to draw another track forever while the
    // indicator said repeat was off.
    let mut app = connected_app();
    app.queue.replace(vec![track("a"), track("b"), track("c")]);
    app.queue.shuffle = true;

    let mut effects = app.handle_action(Action::PlayPause);
    for started in 1..3 {
        effects = app.apply_event(ended(&effects));
        assert!(
            matches!(effects.first(), Some(Effect::Audio(AudioCmd::Play { .. }))),
            "track {started} ended, the pass goes on"
        );
    }
    let effects = app.apply_event(ended(&effects));
    assert_eq!(effects, vec![Effect::Audio(AudioCmd::Stop)], "three starts was the pass");
    assert_eq!(app.queue.current, None);

    // Stopped is not stuck: the engine reports idle, and space deals a
    // fresh pass rather than the queue being spent for good.
    app.apply_event(Event::Status(PlayerStatus::default()));
    let effects = app.handle_action(Action::PlayPause);
    assert!(matches!(effects.first(), Some(Effect::Audio(AudioCmd::Play { .. }))));
    let effects = app.apply_event(ended(&effects));
    assert!(
        matches!(effects.first(), Some(Effect::Audio(AudioCmd::Play { .. }))),
        "the new pass is not billed for the old one's plays"
    );
}

#[test]
fn jump_to_playing_puts_the_queue_on_screen_before_handing_it_the_cursor() {
    // `i` used to move focus without showing the column, leaving the
    // arrows driving a list nobody could see, Enter restarting the
    // current track, and `d` deleting an unseen row.
    let mut app = connected_app();
    app.queue.replace(vec![track("a"), track("b")]);
    app.handle_action(Action::PlayPause);
    assert!(!app.queue_column, "the column starts hidden");

    app.handle_action(Action::JumpToPlaying);
    assert!(app.queue_column, "the cursor's list is put on screen with it");
    assert_eq!(app.focus, Focus::Queue);
    assert_eq!(app.queue.state.selected(), app.queue.current);
}

#[test]
fn jump_to_playing_in_the_fullscreen_view_turns_to_the_queue_tab() {
    // The full-screen view keeps its queue on a tab, where focus means
    // nothing — and a focus quietly parked on the queue would leave the
    // browser screen driving the hidden column after `0` back.
    let mut app = connected_app();
    app.queue.replace(vec![track("a")]);
    app.handle_action(Action::PlayPause);
    app.handle_action(Action::ToggleNowPlaying);
    app.now_tab = NowTab::Visualizer;

    app.handle_action(Action::JumpToPlaying);
    assert_eq!(app.now_tab(), NowTab::Queue, "the tab with the cursor comes up");
    assert_eq!(app.focus, Focus::Browser, "the other screen's focus is left alone");
    assert!(!app.queue_column, "and no hidden column is armed behind the view");
}

#[test]
fn the_jump_keys_follow_the_fullscreen_view_like_the_arrows_do() {
    // In the full-screen view Tab means "next panel tab", so focus can
    // never reach the queue there: G dispatched on focus alone looked
    // dead while it silently moved the browser nobody could see.
    let mut app = connected_app();
    app.apply_event(Event::Listing(Box::new(listing("/lib/", &["sub"], &["a.mp3"]))));
    app.queue.replace(vec![track("a"), track("b"), track("c")]);
    app.handle_action(Action::ToggleNowPlaying);
    app.now_tab = NowTab::Queue;

    let browser_at = app.files.state.selected();
    app.handle_action(Action::Last);
    assert_eq!(app.queue.state.selected(), Some(2), "G reaches the end of the queue");
    app.handle_action(Action::First);
    assert_eq!(app.queue.state.selected(), Some(0), "g comes back to the top");
    assert_eq!(app.files.state.selected(), browser_at, "the hidden browser never moved");
}

#[test]
fn the_dj_mode_row_steps_left_even_when_the_ring_is_two_long() {
    // Stepping back by going forward twice assumed all three modes were
    // on offer. Without a similarity index the ring is Off and BpmKey,
    // and two steps forward is a lap: left looked dead on a default
    // server while right worked.
    let mut app = connected_app();
    app.capabilities = crate::api::types::Capabilities::default();
    app.handle_action(Action::OpenDjPanel);

    app.handle_action(Action::Back); // left on the Mode row
    assert_eq!(app.autodj, AutoDjMode::BpmKey, "left from Off reaches the other mode");
    app.handle_action(Action::Back);
    assert_eq!(app.autodj, AutoDjMode::Off, "and left again comes back round");
}

#[test]
fn the_track_coming_up_never_wears_the_state_of_the_one_that_ended() {
    let mut app = connected_app();
    app.queue.replace(vec![track("a"), track("b")]);
    let effects = app.handle_action(Action::PlayPause);
    let first = played_url(&effects);
    app.apply_event(Event::Status(PlayerStatus {
        source: first.clone(),
        playing: true,
        position: 174.0,
        duration: 174.0,
        ..Default::default()
    }));
    assert!(app.status.playing);

    // It runs out and the queue moves on. Nothing is sounding yet, and
    // saying so under the next track's name is what read as the player
    // stopping between every song.
    let effects = app.apply_event(Event::TrackEnded { source: first.clone() });
    assert_eq!(app.queue.current, Some(1));
    assert!(app.is_starting());
    assert_eq!(app.status.position, 0.0, "the old position does not carry over");

    // The poll that comes next is the engine still describing the track
    // that ended -- an empty source, because it cleared it.
    app.apply_event(Event::Status(PlayerStatus::default()));
    assert!(app.is_starting(), "a status about nothing is not the answer");
    assert_eq!(app.status.source, played_url(&effects));

    // The answer names what was asked for.
    app.apply_event(Event::Status(PlayerStatus {
        source: played_url(&effects),
        playing: true,
        ..Default::default()
    }));
    assert!(!app.is_starting());
    assert!(app.status.playing);
}

#[test]
fn a_length_already_known_is_shown_before_the_engine_confirms_it() {
    // The bar had no total until the first status came back, so a track
    // whose length the library already told us started out as `--:--`.
    let mut app = connected_app();
    let mut long = track("a");
    long.metadata.duration = Some(221.0);
    app.queue.replace(vec![long]);
    app.handle_action(Action::PlayPause);
    assert_eq!(app.status.duration, 221.0);
}

#[test]
fn repeat_all_wraps_but_repeat_one_only_traps_automatic_advance() {
    let mut queue = Queue {
        items: vec![track("a"), track("b")],
        current: Some(1),
        ..Default::default()
    };

    assert_eq!(queue.next_index(false), None);
    queue.repeat = Repeat::All;
    assert_eq!(queue.next_index(false), Some(0));

    queue.repeat = Repeat::One;
    queue.current = Some(0);
    assert_eq!(queue.next_index(false), Some(0), "auto-advance repeats the track");
    assert_eq!(queue.next_index(true), Some(1), "a manual skip escapes repeat-one");
}

#[test]
fn previous_restarts_the_track_when_past_the_grace_window() {
    let mut app = connected_app();
    app.queue.replace(vec![track("a"), track("b")]);
    app.play_index(1);

    app.status.position = 10.0;
    assert_eq!(
        app.handle_action(Action::PrevTrack),
        vec![Effect::Audio(AudioCmd::Seek(0.0))]
    );

    app.status.position = 1.0;
    app.handle_action(Action::PrevTrack);
    assert_eq!(app.queue.current, Some(0));
}

#[test]
fn removing_the_playing_track_stops_playback() {
    let mut app = connected_app();
    app.queue.replace(vec![track("a"), track("b"), track("c")]);
    app.play_index(1);
    app.focus = Focus::Queue;
    app.queue.state.select(Some(1));

    let effects = app.handle_action(Action::RemoveFromQueue);
    assert_eq!(effects, vec![Effect::Audio(AudioCmd::Stop)]);
    assert_eq!(app.queue.items.len(), 2);
    assert_eq!(app.queue.current, None);
}

#[test]
fn removing_an_earlier_track_keeps_the_current_one_playing() {
    let mut queue = Queue {
        items: vec![track("a"), track("b"), track("c")],
        current: Some(2),
        ..Default::default()
    };
    assert!(!queue.remove(0));
    assert_eq!(queue.current, Some(1), "index follows the still-playing track");
}

#[test]
fn adding_to_an_empty_idle_queue_starts_playback() {
    let mut app = connected_app();
    app.apply_event(Event::Listing(Box::new(listing("/lib/", &[], &["a.mp3"]))));
    let effects = app.handle_action(Action::AddToQueue);
    assert_eq!(app.queue.items.len(), 1);
    assert!(matches!(effects[0], Effect::Audio(AudioCmd::Play { .. })));

    // A second add while something is loaded just queues.
    app.status.source = "http://host/media/lib/a.mp3".into();
    let effects = app.handle_action(Action::AddToQueue);
    assert_eq!(app.queue.items.len(), 2);
    assert!(effects.is_empty());
}

#[test]
fn volume_is_clamped() {
    let mut app = connected_app();
    for _ in 0..30 {
        app.handle_action(Action::VolumeUp);
    }
    assert_eq!(app.volume, 1.0);
    for _ in 0..40 {
        app.handle_action(Action::VolumeDown);
    }
    assert_eq!(app.volume, 0.0);
}

#[test]
fn seeking_backward_never_goes_negative() {
    let mut app = connected_app();
    app.status.source = "http://host/a.mp3".into();
    app.status.position = 2.0;
    assert_eq!(
        app.handle_action(Action::SeekBackward),
        vec![Effect::Audio(AudioCmd::Seek(0.0))]
    );
}

#[test]
fn seeking_while_idle_does_nothing() {
    let mut app = connected_app();
    assert!(app.handle_action(Action::SeekForward).is_empty());
}

#[test]
fn every_class_a_search_matched_is_reachable() {
    use crate::api::types::{SearchGroup, SearchTrack};
    let mut app = connected_app();
    app.handle_action(Action::SelectTab(3));
    let hit = |p: &str| SearchTrack {
        name: p.to_string(),
        filepath: p.to_string(),
        album_art_file: None,
        metadata: TrackMetadata::default(),
    };
    app.search_submitted = Some("moon".into());
    app.apply_event(Event::SearchResults {
        query: "moon".into(),
        results: Box::new(SearchResults {
            artists: vec![SearchGroup { name: "Moon Hooch".into(), album_art_file: None }],
            albums: vec![],
            title: vec![hit("lib/a.mp3")],
            files: vec![hit("lib/a.mp3"), hit("lib/b.mp3")],
            lyrics: vec![],
        }),
    });

    // The artist hit used to be counted into a sentence and thrown away.
    // Classes that matched nothing stay out of the menu.
    let menu: Vec<&str> = app
        .search
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Search { label, .. } => Some(label.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(menu, vec!["Artists", "Titles", "Filenames"]);
    assert_eq!(app.search_summary.as_deref(), Some("4 matches"));

    // Opening a class needs no request -- the whole reply is in hand.
    assert!(app.handle_action(Action::Activate).is_empty(), "no request to open a class");
    assert_eq!(app.search_node(), &SearchNode::Class(SearchClass::Artists));

    // And the artist opens the same place the Library tab would, with the
    // search tab named as the destination so the reply cannot land in the
    // wrong column.
    let effects = app.handle_action(Action::Activate);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Library {
            node: LibraryNode::Artist("Moon Hooch".into()),
            dest: Tab::Search,
        })]
    );

    // A track that matched on two classes is listed under both, which is
    // the point: it says the match was found two ways.
    app.handle_action(Action::Back);
    app.handle_action(Action::Back);
    app.handle_action(Action::Down);
    app.handle_action(Action::Down);
    app.handle_action(Action::Activate);
    assert_eq!(app.search_node(), &SearchNode::Class(SearchClass::Files));
    assert_eq!(app.search.entries.len(), 3, "'..' plus both filename hits");
}

#[test]
fn unauthorized_returns_to_the_connect_screen() {
    let mut app = connected_app();
    app.apply_event(Event::Unauthorized);
    assert!(!app.connected);
    assert!(app.session.token.is_none());
    assert_eq!(app.input_mode(), InputMode::Editing);
}

#[test]
fn a_public_mode_server_picked_from_the_network_connects_outright() {
    use crate::discovery::DiscoveredServer;
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::QuickConnect;
    app.apply_event(Event::ServersDiscovered(vec![DiscoveredServer {
        name: "Open Server".into(),
        base_url: "http://192.168.1.5:3000".into(),
        version: None,
        quick_connect: false,
    }]));
    app.handle_action(Action::Submit);

    app.apply_event(Event::Connected {
        server: "http://192.168.1.5:3000".into(),
        id: "http://192.168.1.5:3000".into(),
        username: None,
        token: None,
        ping: Box::new(Default::default()),
    });
    assert!(app.connected, "no login step when the server doesn't want one");
}

#[test]
fn the_first_screen_offers_both_ways_in() {
    let mut app = App::new(None, None, None);
    assert_eq!(app.connect.stage, ConnectStage::Choosing);
    assert_eq!(CONNECT_METHODS.len(), 2);

    // Direct is the default choice.
    app.handle_action(Action::Submit);
    assert_eq!(app.connect.stage, ConnectStage::Direct);

    // Esc returns to the chooser, where Down picks Quick Connect.
    app.handle_action(Action::Cancel);
    assert_eq!(app.connect.stage, ConnectStage::Choosing);
    app.handle_action(Action::Down);
    app.handle_action(Action::Submit);
    assert_eq!(app.connect.stage, ConnectStage::QuickConnect);
}

#[test]
fn the_chooser_selection_stays_in_range() {
    let mut app = App::new(None, None, None);
    for _ in 0..5 {
        app.handle_action(Action::Up);
    }
    assert_eq!(app.connect.choice, 0);
    for _ in 0..5 {
        app.handle_action(Action::Down);
    }
    assert_eq!(app.connect.choice, CONNECT_METHODS.len() - 1);
}

#[test]
fn opening_quick_connect_starts_a_network_search() {
    let mut app = App::new(None, None, None);
    app.handle_action(Action::Down); // onto Quick Connect
    let effects = app.handle_action(Action::Submit);
    assert_eq!(app.connect.stage, ConnectStage::QuickConnect);
    assert!(effects.contains(&Effect::Discover));
    assert!(app.connect.searching);
}

#[test]
fn choosing_a_discovered_server_connects_to_it_directly() {
    use crate::discovery::DiscoveredServer;
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::QuickConnect;
    app.apply_event(Event::ServersDiscovered(vec![DiscoveredServer {
        name: "Living Room".into(),
        base_url: "http://192.168.1.71:3999".into(),
        version: None,
        quick_connect: true,
    }]));
    assert!(!app.connect.searching);

    // A server on this network needs no tunnel and no code.
    let effects = app.handle_action(Action::Submit);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Connect {
            server: "http://192.168.1.71:3999".into(),
            token: None,
        })]
    );
}

#[test]
fn late_discovery_results_do_not_move_the_cursor_off_the_paste_row() {
    // Found live: the browse takes seconds, so a pasted code can be
    // submitted before it answers. Row 0 means "paste" with an empty list
    // and "first server" with a populated one, so the arriving results
    // used to retarget Enter at a server the user never chose.
    use crate::discovery::DiscoveredServer;
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::QuickConnect;
    for c in "mstr1:abc".chars() {
        app.handle_action(Action::Input(c));
    }
    assert!(app.connect.on_paste_row());

    app.apply_event(Event::ServersDiscovered(vec![DiscoveredServer {
        name: "Living Room".into(),
        base_url: "http://192.168.1.71:3999".into(),
        version: None,
        quick_connect: true,
    }]));
    assert!(app.connect.on_paste_row(), "still aimed at the code the user pasted");

    // …and Enter still dials the code rather than the newly-found server.
    let effects = app.handle_action(Action::Submit);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::QuickConnect { code: "mstr1:abc".into(), token: None })]
    );
}

#[test]
fn a_late_needs_login_cannot_unseat_a_live_session() {
    // Found live: a second connection attempt answered after the tunnel
    // had already connected, and dragged the connected UI to a login form
    // for a server the user had abandoned.
    let mut app = connected_app();
    app.apply_event(Event::NeedsLogin { server: "http://192.168.1.71:3999".into() });
    assert!(app.connected, "the live session survives");
    assert_eq!(app.session.server, "http://host:3000", "and stays on its own server");
}

#[test]
fn typing_a_code_jumps_past_the_discovered_servers() {
    use crate::discovery::DiscoveredServer;
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::QuickConnect;
    app.apply_event(Event::ServersDiscovered(vec![DiscoveredServer {
        name: "Living Room".into(),
        base_url: "http://host:3999".into(),
        version: None,
        quick_connect: true,
    }]));
    assert_eq!(app.connect.row, 0, "starts on the first server");

    app.handle_action(Action::Input('m'));
    assert!(app.connect.on_paste_row(), "typing means the user has a code");

    // …and Enter now dials rather than connecting to the highlighted server.
    for c in "str1:abc".chars() {
        app.handle_action(Action::Input(c));
    }
    let effects = app.handle_action(Action::Submit);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::QuickConnect { code: "mstr1:abc".into(), token: None })]
    );
}

#[test]
fn the_selection_cannot_run_past_the_paste_row() {
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::QuickConnect;
    for _ in 0..5 {
        app.handle_action(Action::Down);
    }
    assert_eq!(app.connect.row, app.connect.paste_row());
}

#[test]
fn pasting_a_pairing_code_dials_the_tunnel() {
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::QuickConnect;
    for c in "mstr1:abc".chars() {
        app.handle_action(Action::Input(c));
    }
    let effects = app.handle_action(Action::Submit);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::QuickConnect { code: "mstr1:abc".into(), token: None })]
    );
    assert!(app.connecting);
}

#[test]
fn a_tunnel_session_is_remembered_by_identity_not_by_its_loopback_port() {
    // The bug this pins: the loopback bridge got saved as the server, so
    // the next run dialled a port that no longer existed, and the token
    // was filed under a URL that could never match again.
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::QuickConnect;
    for c in "mstr1:abc".chars() {
        app.handle_action(Action::Input(c));
    }
    app.handle_action(Action::Submit);

    app.apply_event(Event::TunnelReady {
        local_url: "http://127.0.0.1:51234".into(),
        id: "mstream+iroh://endpointabc".into(),
    });
    app.connect.username = "alice".into();
    app.connect.password = "pw".into();
    app.handle_action(Action::Submit);
    app.apply_event(Event::Connected {
        server: "http://127.0.0.1:51234".into(),
        id: "mstream+iroh://endpointabc".into(),
        username: Some("alice".into()),
        token: Some("tok".into()),
        ping: Box::new(Default::default()),
    });

    // What gets written down is the identity...
    assert_eq!(app.session.server_id, "mstream+iroh://endpointabc");
    // ...while requests and stream URLs still go over the bridge.
    assert_eq!(app.session.server, "http://127.0.0.1:51234");
    let kept = app.session.tunnel_code.as_deref();
    assert_eq!(kept, Some("mstr1:abc"), "kept, or there's no way back");
}

#[test]
fn a_public_tunnel_server_is_still_worth_saving() {
    // No login means no token and no username — but without the pairing
    // code stored, the server is unreachable next time.
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::QuickConnect;
    for c in "mstr1:pub".chars() {
        app.handle_action(Action::Input(c));
    }
    app.handle_action(Action::Submit);

    let effects = app.apply_event(Event::Connected {
        server: "http://127.0.0.1:5000".into(),
        id: "mstream+iroh://pubserver".into(),
        username: None,
        token: None,
        ping: Box::new(Default::default()),
    });
    assert!(effects.contains(&Effect::SaveSession), "got {effects:?}");
}

#[test]
fn reconnecting_to_a_tunnel_server_dials_its_code_again() {
    let app = App::new(Some("mstream+iroh://endpointabc".into()), Some("tok".into()), None);
    // The identity is not an address, so it must not reach the form or
    // the endpoint — only the dialler.
    assert!(app.session.server.is_empty(), "nothing can be requested from an identity");
    assert!(app.connect.server.is_empty(), "and it cannot be typed or edited");

    let mut app = app.with_tunnel(Some("mstr1:saved".into()));
    let effects = app.start();
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::QuickConnect {
            code: "mstr1:saved".into(),
            token: Some("tok".into()),
        })],
        "the saved token rides the re-dialled tunnel"
    );
    assert!(app.connecting);
}

#[test]
fn a_remembered_tunnel_with_no_code_says_so_instead_of_hanging() {
    // Credentials deleted, or a config.toml copied to a new machine
    // without the file holding its secrets.
    let mut app = App::new(Some("mstream+iroh://endpointabc".into()), None, None);
    assert!(app.start().is_empty(), "nothing to dial, so nothing is attempted");
    assert!(!app.connecting, "and it doesn't sit on a connecting screen forever");
    assert!(app.session.server_id.is_empty(), "the unreachable server is let go");
    let message = &app.message.as_ref().unwrap().text;
    assert!(message.contains("pairing code"), "got: {message}");
}

#[test]
fn an_expired_tunnel_session_signs_back_in_over_the_open_bridge() {
    // The tunnel outlives the token: the bridge is still up in the worker,
    // so the login form must aim at it rather than at an identity no HTTP
    // client can dial.
    let mut app = App::new(None, None, None);
    app.connected = true;
    app.session.server = "http://127.0.0.1:51234".into();
    app.session.server_id = "mstream+iroh://endpointabc".into();
    app.session.token = Some("stale".into());

    app.apply_event(Event::Unauthorized);
    assert_eq!(app.connect.stage, ConnectStage::Direct);
    assert_eq!(app.connect.server, "http://127.0.0.1:51234", "aims at the live bridge");

    app.connect.username = "alice".into();
    app.connect.password = "pw".into();
    let effects = app.handle_action(Action::Submit);
    // Loopback, so no plaintext warning stands between the user and a
    // re-login they didn't ask for in the first place.
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Login {
            server: "http://127.0.0.1:51234".into(),
            username: "alice".into(),
            password: "pw".into(),
        })]
    );
}

#[test]
fn a_tunnel_is_shown_by_name_rather_than_by_port() {
    let mut app = App::new(None, None, None);
    app.session.server = "http://127.0.0.1:51234".into();
    app.session.server_id = "mstream+iroh://endpointabcdef123456".into();
    let shown = app.server_display();
    assert!(shown.starts_with("quick connect"), "got: {shown}");
    assert!(!shown.contains("127.0.0.1"), "the port is an implementation detail");

    // A direct server is shown as itself.
    let mut app = App::new(None, None, None);
    app.session.server = "https://demo.mstream.io".into();
    app.session.server_id = "https://demo.mstream.io".into();
    assert_eq!(app.server_display(), "https://demo.mstream.io");
}

#[test]
fn an_empty_pairing_code_is_refused_without_a_request() {
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::QuickConnect;
    assert!(app.handle_action(Action::Submit).is_empty());
    assert!(!app.connecting);
}

#[test]
fn an_open_tunnel_leads_to_the_login_form() {
    // The secret gates the pipe, not the API — so the tunnel coming up
    // means "now sign in", not "you're in".
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::QuickConnect;
    app.connecting = true;

    app.apply_event(Event::TunnelReady {
        local_url: "http://127.0.0.1:51234".into(),
        id: "mstream+iroh://abc123".into(),
    });
    assert_eq!(app.connect.stage, ConnectStage::Direct);
    assert_eq!(app.connect.server, "http://127.0.0.1:51234");
    assert_eq!(app.connect.field, 1, "focus lands on the username");
    assert!(!app.connecting);
    assert_eq!(app.session.server_id, "mstream+iroh://abc123", "already filed under its identity");
}

#[test]
fn a_picked_server_that_wants_credentials_opens_its_login_form() {
    // Regression: choosing a server found on the network bounced back to
    // "how do you want to connect?", losing the server that was picked —
    // the connect path was reporting "needs a sign-in" as an
    // authorization failure.
    use crate::discovery::DiscoveredServer;
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::QuickConnect;
    app.apply_event(Event::ServersDiscovered(vec![DiscoveredServer {
        name: "Living Room".into(),
        base_url: "http://192.168.1.71:3999".into(),
        version: None,
        quick_connect: true,
    }]));
    app.handle_action(Action::Submit);

    app.apply_event(Event::NeedsLogin { server: "http://192.168.1.71:3999".into() });
    assert_eq!(app.connect.stage, ConnectStage::Direct, "lands on the login form");
    assert_eq!(
        app.connect.server, "http://192.168.1.71:3999",
        "the chosen server is kept, not blanked"
    );
    assert_eq!(app.connect.field, 1, "focus is on the username");
    assert!(!app.connecting);
}

#[test]
fn an_expired_session_offers_a_login_for_the_same_server() {
    let mut app = connected_app();
    app.apply_event(Event::Unauthorized);
    assert!(!app.connected);
    assert_eq!(app.connect.stage, ConnectStage::Direct);
    assert_eq!(app.connect.server, "http://host:3000", "stays on the server in use");
    assert!(app.session.token.is_none());
}

#[test]
fn connecting_without_a_username_uses_public_mode() {
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::Direct;
    app.connect.server = "http://host:3000".into();
    let effects = app.handle_action(Action::Submit);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Connect { server: "http://host:3000".into(), token: None })]
    );
}

#[test]
fn the_server_field_starts_empty_for_a_new_user() {
    // It used to be prefilled with a guess at localhost, so the first
    // thing anyone had to do was delete two dozen characters.
    let app = App::new(None, None, None);
    assert!(app.connect.server.is_empty());

    // A server we actually know about is still offered.
    let app = App::new(Some("http://host:3000".into()), None, None);
    assert_eq!(app.connect.server, "http://host:3000");
}

#[test]
fn connect_form_edits_the_focused_field_only() {
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::Direct;
    app.handle_action(Action::Input('h'));
    app.handle_action(Action::CycleFocus);
    app.handle_action(Action::Input('u'));
    assert_eq!(app.connect.server, "h");
    assert_eq!(app.connect.username, "u");
    assert!(app.connect.password.is_empty());
}

#[test]
fn login_effect_carries_credentials_and_clears_the_password() {
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::Direct;
    app.connect.server = "http://host:3000".into();
    app.connect.username = "alice".into();
    app.connect.password = "secret".into();

    let effects = app.handle_action(Action::Submit);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Login {
            server: "http://host:3000".into(),
            username: "alice".into(),
            password: "secret".into(),
        })]
    );
    assert!(app.connect.password.is_empty(), "password is not kept in memory after use");
}

/// A connect screen sitting on `Direct` with the given server text.
fn at_direct(server: &str) -> App {
    let mut app = App::new(None, None, None);
    app.connect.stage = ConnectStage::Direct;
    app.connect.server = server.into();
    app
}

#[test]
fn a_typed_address_is_completed_before_it_is_used() {
    // What used to happen here: "relative URL without a base", after a
    // round trip, with the typed text still on screen.
    let mut app = at_direct("nas:3000");
    let effects = app.handle_action(Action::Submit);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Connect { server: "http://nas:3000".into(), token: None })]
    );
    assert_eq!(app.connect.server, "http://nas:3000", "the field shows what was assumed");

    let mut app = at_direct("music.example.com");
    app.handle_action(Action::Submit);
    assert_eq!(app.connect.server, "https://music.example.com");
}

#[test]
fn an_unusable_address_is_refused_without_a_round_trip() {
    let mut app = at_direct("ftp://host");
    assert!(app.handle_action(Action::Submit).is_empty(), "nothing is dispatched");
    assert!(!app.connecting, "and the screen doesn't pretend to be busy");
    let message = app.message.as_ref().unwrap();
    assert_eq!(message.kind, MessageKind::Error);
    assert!(message.text.contains("http://"), "it says what to type instead");

    let mut app = at_direct("   ");
    assert!(app.handle_action(Action::Submit).is_empty());
    assert!(app.message.as_ref().unwrap().text.contains("enter a server address"));
}

#[test]
fn a_username_without_a_password_is_caught_here_not_by_the_server() {
    let mut app = at_direct("http://host:3000");
    app.connect.username = "alice".into();
    assert!(app.handle_action(Action::Submit).is_empty());
    let text = &app.message.as_ref().unwrap().text;
    assert!(text.contains("password"), "got: {text}");
    // The way out is spelled out, since public mode is a real mode.
    assert!(text.contains("public"), "got: {text}");
}

#[test]
fn sending_a_password_over_plain_http_asks_first() {
    let mut app = at_direct("http://music.example.com");
    app.connect.username = "alice".into();
    app.connect.password = "secret".into();

    // First Enter: warned, nothing sent, password still typed.
    assert!(app.handle_action(Action::Submit).is_empty());
    assert!(app.message.as_ref().unwrap().text.contains("unencrypted"));
    assert_eq!(app.connect.password, "secret", "so the answer can just be yes");

    // Second Enter: taken as consent.
    let effects = app.handle_action(Action::Submit);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Login {
            server: "http://music.example.com".into(),
            username: "alice".into(),
            password: "secret".into(),
        })]
    );
}

#[test]
fn consent_to_plaintext_does_not_follow_you_to_another_server() {
    let mut app = at_direct("http://music.example.com");
    app.connect.username = "alice".into();
    app.connect.password = "secret".into();
    app.handle_action(Action::Submit); // warned

    app.connect.server = "http://other.example.com".into();
    assert!(app.handle_action(Action::Submit).is_empty(), "the new host warns on its own");
    assert!(app.message.as_ref().unwrap().text.contains("other.example.com"));
}

#[test]
fn signing_in_to_a_lan_server_is_not_interrupted() {
    // http on the LAN is how mStream is normally run: a warning every
    // time would be noise, and noise is what gets clicked through.
    for server in ["http://192.168.1.71:3999", "nas:3000", "http://localhost:3000"] {
        let mut app = at_direct(server);
        app.connect.username = "alice".into();
        app.connect.password = "secret".into();
        let effects = app.handle_action(Action::Submit);
        assert!(
            matches!(effects.as_slice(), [Effect::Api(ApiCmd::Login { .. })]),
            "{server} should sign in without ceremony, got {effects:?}"
        );
    }
}

#[test]
fn a_public_server_over_plain_http_needs_no_warning() {
    // Nothing secret is being sent, so there is nothing to warn about.
    let mut app = at_direct("http://music.example.com");
    let effects = app.handle_action(Action::Submit);
    assert!(matches!(effects.as_slice(), [Effect::Api(ApiCmd::Connect { .. })]));
}

#[test]
fn quitting_shuts_both_workers_down() {
    let mut app = connected_app();
    let effects = app.handle_action(Action::Quit);
    assert!(app.should_quit);
    assert!(effects.contains(&Effect::Audio(AudioCmd::Shutdown)));
    assert!(effects.contains(&Effect::Api(ApiCmd::Shutdown)));
}

#[test]
fn key_mapping_differs_between_normal_and_editing_modes() {
    let key = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);

    assert_eq!(map_key(key('j'), InputMode::Normal), Some(Action::Down));
    assert_eq!(map_key(key('j'), InputMode::Editing), Some(Action::Input('j')));
    assert_eq!(map_key(key('q'), InputMode::Normal), Some(Action::Quit));
    assert_eq!(map_key(key('q'), InputMode::Editing), Some(Action::Input('q')));

    // Ctrl+C always quits, even mid-typing.
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(map_key(ctrl_c, InputMode::Editing), Some(Action::Quit));

    assert_eq!(map_key(key('2'), InputMode::Normal), Some(Action::SelectTab(1)));
    assert_eq!(
        map_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), InputMode::Normal),
        Some(Action::Activate)
    );
}

#[test]
fn searching_from_the_query_box_emits_one_search() {
    let mut app = connected_app();
    app.handle_action(Action::StartSearch);
    assert_eq!(app.tab, Tab::Search);
    assert_eq!(app.input_mode(), InputMode::Editing);

    for c in "moon".chars() {
        app.handle_action(Action::Input(c));
    }
    let effects = app.handle_action(Action::Submit);
    assert_eq!(effects, vec![Effect::Api(ApiCmd::Search("moon".into()))]);
    assert!(!app.editing_query);
}

#[test]
fn opening_the_playlists_tab_loads_them_once() {
    let mut app = connected_app();
    let effects = app.handle_action(Action::SelectTab(2));
    assert_eq!(effects, vec![Effect::Api(ApiCmd::Playlists)]);

    app.apply_event(Event::Playlists(vec![crate::api::types::PlaylistSummary {
        name: "Roadtrip".into(),
    }]));
    // Already loaded: switching back doesn't refetch.
    app.handle_action(Action::SelectTab(0));
    assert!(app.handle_action(Action::SelectTab(2)).is_empty());
}

#[test]
fn a_pane_knows_when_its_contents_are_still_on_the_wire() {
    let mut app = connected_app();
    assert!(!app.playlists.loading);

    app.handle_action(Action::SelectTab(2));
    assert!(app.playlists.loading, "asking marks the pane, not the call site");
    // Only the pane that was asked for — leaving the tab mid-flight must
    // not leave a spinner turning somewhere it was never requested.
    assert!(!app.files.loading && !app.search.loading);

    app.apply_event(Event::Playlists(Vec::new()));
    assert!(!app.playlists.loading, "the reply lands through Pane::set");
}

#[test]
fn a_request_that_fails_is_no_longer_pending() {
    let mut app = connected_app();
    app.handle_action(Action::SelectTab(2));
    // Nothing calls `Pane::set` on the way out of an error, so this is the
    // one path that would otherwise spin forever.
    app.apply_event(Event::Error("nope".into()));
    assert!(!app.playlists.loading);
}

#[test]
fn playlist_tracks_open_and_close() {
    let mut app = connected_app();
    app.handle_action(Action::SelectTab(2));
    app.apply_event(Event::Playlists(vec![crate::api::types::PlaylistSummary {
        name: "Roadtrip".into(),
    }]));

    let effects = app.handle_action(Action::Activate);
    assert_eq!(effects, vec![Effect::Api(ApiCmd::LoadPlaylist("Roadtrip".into()))]);

    app.apply_event(Event::PlaylistTracks {
        name: "Roadtrip".into(),
        tracks: vec![track("lib/a.mp3")],
    });
    assert_eq!(app.playlists.entries.len(), 2); // ".." + one track
    assert_eq!(app.playlist_open.as_deref(), Some("Roadtrip"));

    let effects = app.handle_action(Action::Back);
    assert!(effects.is_empty(), "the playlist list came back off the trail");
    assert!(app.playlist_open.is_none());
    assert_eq!(app.playlists.entries.len(), 1, "the one playlist, restored");
}

#[test]
fn a_playlist_answering_after_it_was_left_does_not_open_over_the_top() {
    let mut app = connected_app();
    app.handle_action(Action::SelectTab(2));
    app.apply_event(Event::Playlists(
        ["Roadtrip", "Dinner"]
            .iter()
            .map(|name| crate::api::types::PlaylistSummary { name: (*name).to_string() })
            .collect(),
    ));

    // Open one and change your mind before it answers. Which playlist is
    // open is now decided on the way in rather than by whatever replied
    // last, so the way out has something to close.
    app.handle_action(Action::Activate);
    assert_eq!(app.playlist_open.as_deref(), Some("Roadtrip"));
    assert!(app.message.as_ref().unwrap().text.contains("loading playlist"));
    app.handle_action(Action::Back);
    assert!(app.playlist_open.is_none());
    assert!(app.message.is_none(), "and the note about loading it goes too");

    app.apply_event(Event::PlaylistTracks {
        name: "Roadtrip".into(),
        tracks: vec![track("lib/a.mp3")],
    });
    assert!(app.playlist_open.is_none(), "closed stays closed");
    assert_eq!(labels(&app), vec!["Roadtrip", "Dinner"], "and the list is still the list");

    // The same rule when the change of mind is another playlist: the one
    // on screen must be the one named at the top of it.
    app.handle_action(Action::Down);
    app.handle_action(Action::Activate);
    assert_eq!(app.playlist_open.as_deref(), Some("Dinner"));
    app.apply_event(Event::PlaylistTracks {
        name: "Roadtrip".into(),
        tracks: vec![track("lib/a.mp3")],
    });
    assert_eq!(app.playlist_open.as_deref(), Some("Dinner"));
    assert_eq!(labels(&app), vec!["Roadtrip", "Dinner"], "Roadtrip's tracks are not it");
}

#[test]
fn library_tab_opens_on_a_static_menu_without_a_request() {
    let mut app = connected_app();
    let effects = app.handle_action(Action::SelectTab(1));
    assert!(effects.is_empty(), "the mode menu costs no round-trip");
    assert_eq!(app.library.entries.len(), 4);
    assert_eq!(app.library_node(), &LibraryNode::Root);

    let labels: Vec<&str> = app
        .library
        .entries
        .iter()
        .map(|e| match e {
            Entry::Node { label, .. } => label.as_str(),
            _ => "?",
        })
        .collect();
    assert_eq!(labels, ["Artists", "Albums", "Genres", "Recently Added"]);
}

#[test]
fn drilling_from_artists_to_an_album_of_tracks() {
    let mut app = connected_app();
    app.handle_action(Action::SelectTab(1));

    // Artists
    let effects = app.handle_action(Action::Activate);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Library { node: LibraryNode::Artists, dest: Tab::Library })]
    );
    app.apply_event(Event::Library {
        dest: Tab::Library,
        node: LibraryNode::Artists,
        data: LibraryData::Artists(vec!["Signal Chain".into(), "Terminal Test".into()]),
    });
    assert_eq!(app.library.entries.len(), 3); // ".." + two artists

    // One artist's albums
    let effects = app.handle_action(Action::Activate);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Library {
            node: LibraryNode::Artist("Signal Chain".into()),
            dest: Tab::Library,
        })]
    );
    app.apply_event(Event::Library {
        dest: Tab::Library,
        node: LibraryNode::Artist("Signal Chain".into()),
        data: LibraryData::Albums(vec![Album {
            name: Some("Second Album".into()),
            artist: Some("Signal Chain".into()),
            year: Some(2025),
            album_art_file: None,
        }]),
    });

    // That album's tracks
    let effects = app.handle_action(Action::Activate);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Library {
            node: LibraryNode::Album {
                name: "Second Album".into(),
                artist: Some("Signal Chain".into()),
            },
            dest: Tab::Library,
        })]
    );
    app.apply_event(Event::Library {
        dest: Tab::Library,
        node: LibraryNode::Album {
            name: "Second Album".into(),
            artist: Some("Signal Chain".into()),
        },
        data: LibraryData::Tracks(vec![track("testlib/a.mp3"), track("testlib/b.mp3")]),
    });

    // Playing from here queues the album and starts at the selected track.
    let effects = app.handle_action(Action::Activate);
    assert_eq!(app.queue.items.len(), 2);
    assert!(matches!(effects[0], Effect::Audio(AudioCmd::Play { .. })));
}

#[test]
fn back_walks_the_library_stack_to_the_menu() {
    let mut app = connected_app();
    app.handle_action(Action::SelectTab(1));
    app.handle_action(Action::Activate); // → Artists
    app.apply_event(Event::Library {
        dest: Tab::Library,
        node: LibraryNode::Artists,
        data: LibraryData::Artists(vec!["Solo".into()]),
    });
    app.handle_action(Action::Activate); // → Artist("Solo")

    let effects = app.handle_action(Action::Back);
    assert!(effects.is_empty(), "the artist list came back off the trail");
    assert_eq!(app.library_node(), &LibraryNode::Artists);

    let effects = app.handle_action(Action::Back);
    assert!(effects.is_empty(), "returning to the static menu needs no request");
    assert_eq!(app.library_node(), &LibraryNode::Root);
    assert_eq!(app.library.entries.len(), 4);

    // Already at the top.
    assert!(app.handle_action(Action::Back).is_empty());
}

#[test]
fn a_reply_for_an_abandoned_view_is_discarded() {
    let mut app = connected_app();
    app.handle_action(Action::SelectTab(1));
    app.handle_action(Action::Activate); // asked for Artists
    app.handle_action(Action::Back); // …then changed our mind

    app.apply_event(Event::Library {
        dest: Tab::Library,
        node: LibraryNode::Artists,
        data: LibraryData::Artists(vec!["Ghost".into()]),
    });
    assert_eq!(app.library_node(), &LibraryNode::Root);
    assert_eq!(app.library.entries.len(), 4, "the menu is untouched by the late reply");
}

#[test]
fn genres_show_track_counts_and_lead_to_songs() {
    let mut app = connected_app();
    app.tab = Tab::Library;
    app.library_stack.restart();
    app.library_stack.enter(LibraryNode::Genres);
    app.apply_event(Event::Library {
        dest: Tab::Library,
        node: LibraryNode::Genres,
        data: LibraryData::Genres(vec![
            Genre { name: "Ambient".into(), track_count: Some(2) },
            Genre { name: "Electronic".into(), track_count: None },
        ]),
    });

    assert_eq!(
        app.library.entries[1],
        Entry::Node { label: "Ambient (2)".into(), node: LibraryNode::Genre("Ambient".into()) }
    );
    assert_eq!(
        app.library.entries[2],
        Entry::Node { label: "Electronic".into(), node: LibraryNode::Genre("Electronic".into()) }
    );

    let effects = app.handle_action(Action::Activate);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Library {
            node: LibraryNode::Genre("Ambient".into()),
            dest: Tab::Library,
        })]
    );
}

#[test]
fn albums_without_an_artist_still_resolve() {
    // The all-albums endpoint omits the artist field; the album name alone
    // has to be enough to fetch tracks.
    let mut app = connected_app();
    app.library_stack.restart();
    app.library_stack.enter(LibraryNode::Albums);
    app.apply_event(Event::Library {
        dest: Tab::Library,
        node: LibraryNode::Albums,
        data: LibraryData::Albums(vec![Album {
            name: Some("Phase Three".into()),
            artist: None,
            year: Some(2026),
            album_art_file: None,
        }]),
    });

    assert_eq!(
        app.library.entries[1],
        Entry::Node {
            label: "Phase Three (2026)".into(),
            node: LibraryNode::Album { name: "Phase Three".into(), artist: None },
        }
    );
}

#[test]
fn recently_added_lists_tracks_directly() {
    let mut app = connected_app();
    app.library_stack.restart();
    app.library_stack.enter(LibraryNode::Recent);
    app.apply_event(Event::Library {
        dest: Tab::Library,
        node: LibraryNode::Recent,
        data: LibraryData::Tracks(vec![track("testlib/new.mp3")]),
    });
    assert_eq!(app.library.entries.len(), 2); // ".." + the track
    assert!(matches!(app.library.entries[1], Entry::Track { .. }));
}

fn autodj_effect(effects: &[Effect]) -> Option<&ApiCmd> {
    effects.iter().find_map(|e| match e {
        Effect::Api(cmd @ ApiCmd::AutoDj { .. }) => Some(cmd),
        _ => None,
    })
}

#[test]
fn remembered_preferences_are_applied_and_handed_back() {
    let saved = crate::config::PlayerPrefs {
        volume: 0.35,
        repeat: "all".into(),
        shuffle: true,
        autodj: "tempo+key".into(),
        dj: Default::default(),
    };
    let app = App::new(None, None, None).with_prefs(&saved);
    assert_eq!(app.volume, 0.35);
    assert_eq!(app.queue.repeat, Repeat::All);
    assert!(app.queue.shuffle);
    assert_eq!(app.autodj, AutoDjMode::BpmKey);

    // What goes out matches what came in, so a restart is a no-op.
    assert_eq!(app.prefs(), saved);
}

#[test]
fn nonsense_preferences_fall_back_rather_than_refusing_to_start() {
    // A hand-edited config shouldn't be able to brick the player.
    let saved = crate::config::PlayerPrefs {
        volume: 9.0,
        repeat: "sideways".into(),
        shuffle: false,
        autodj: "disco".into(),
        dj: Default::default(),
    };
    let app = App::new(None, None, None).with_prefs(&saved);
    assert_eq!(app.volume, 1.0, "volume is clamped");
    assert_eq!(app.queue.repeat, Repeat::Off);
    assert_eq!(app.autodj, AutoDjMode::Off);
}

#[test]
fn autodj_cycles_through_its_modes() {
    let mut app = connected_app();
    assert_eq!(app.autodj, AutoDjMode::Off);
    app.handle_action(Action::ToggleAutoDj);
    assert_eq!(app.autodj, AutoDjMode::Similar);
    app.handle_action(Action::ToggleAutoDj);
    assert_eq!(app.autodj, AutoDjMode::BpmKey);
    app.handle_action(Action::ToggleAutoDj);
    assert_eq!(app.autodj, AutoDjMode::Off);
}

/// A browser pane holding tracks, with one highlighted.
fn browsing(app: &mut App, names: &[&str], selected: usize) {
    let entries: Vec<Entry> = names
        .iter()
        .map(|n| Entry::Track { label: (*n).to_string(), track: Box::new(track(n)) })
        .collect();
    app.files.set(entries);
    app.files.state.select(Some(selected));
}

fn stop(path: &str, t: f64) -> crate::api::types::JourneyStop {
    crate::api::types::JourneyStop {
        filepath: path.into(),
        t,
        similarity: 0.9,
        ..Default::default()
    }
}

fn similar_artist(name: &str, similarity: f64, ways: &[&str]) -> crate::api::types::SimilarArtist {
    crate::api::types::SimilarArtist {
        artist: name.into(),
        similarity,
        analyzed_count: 12,
        genre_tags: vec!["ambient".into()],
        entry_points: ways.iter().map(|p| track(p)).collect(),
    }
}

/// Index of the Discover tab among the visible ones.
fn discover_tab(app: &App) -> usize {
    app.tabs().iter().position(|t| *t == Tab::Discover).expect("discover is available")
}

#[test]
fn hierarchical_model_tags_are_shown_by_their_leaf() {
    // Live against a real index every row read "Electronic---Dubstep,
    // Electroni…" — the shared prefix filled the column and said nothing.
    assert_eq!(tidy_tag("Electronic---Dubstep"), "Dubstep");
    assert_eq!(tidy_tag("Hip Hop---Trap"), "Trap");
    assert_eq!(tidy_tag("Jazz"), "Jazz");
    assert_eq!(tidy_tag(""), "");
}

#[test]
fn the_discover_tab_is_absent_where_the_server_cannot_serve_it() {
    let app = connected_app();
    assert!(app.tabs().contains(&Tab::Discover));

    let mut plain = connected_app();
    plain.capabilities = Default::default();
    assert!(!plain.tabs().contains(&Tab::Discover));
    // And the numbers stay 1..n so no key points at a gap.
    assert_eq!(plain.tabs().len(), 4);
    assert!(plain.handle_action(Action::SelectTab(4)).is_empty(), "there is no fifth tab");
    assert_ne!(plain.tab, Tab::Discover);
}

#[test]
fn opening_discover_anchors_on_what_is_highlighted() {
    // The cursor was on a track in another tab, and that is the obvious
    // thing to mean by "more like this".
    let mut app = connected_app();
    browsing(&mut app, &["a", "b"], 1);
    app.handle_action(Action::SelectTab(discover_tab(&app)));

    assert_eq!(app.tab, Tab::Discover);
    assert_eq!(app.discover_seed.as_ref().unwrap().filepath, "b");
    // The mode menu costs no request.
    assert_eq!(app.discover.entries.len(), 2);
}

#[test]
fn what_is_playing_wins_over_what_is_merely_highlighted() {
    let mut app = connected_app();
    app.queue.replace(vec![track("playing")]);
    app.play_index(0);
    browsing(&mut app, &["highlighted"], 0);
    app.handle_action(Action::SelectTab(discover_tab(&app)));
    assert_eq!(app.discover_seed.as_ref().unwrap().filepath, "playing");
}

#[test]
fn similar_tracks_are_ordinary_playable_rows() {
    let mut app = connected_app();
    browsing(&mut app, &["seed"], 0);
    app.handle_action(Action::SelectTab(discover_tab(&app)));

    let effects = app.handle_action(Action::Activate);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Api(ApiCmd::Discover { node: DiscoverNode::Tracks, seed })]
            if seed.filepath == "seed"
    ));

    app.apply_event(Event::Discover {
        node: DiscoverNode::Tracks,
        data: DiscoverData::Tracks(vec![track("near-one"), track("near-two")]),
        note: None,
    });
    // Parent row plus the two neighbours, and Enter plays them like any
    // other list — no new concept to learn.
    assert_eq!(app.discover.entries.len(), 3);
    app.discover.state.select(Some(1));
    let effects = app.handle_action(Action::Activate);
    assert_eq!(app.queue.items.len(), 2);
    assert_eq!(app.queue.current, Some(0));
    assert!(effects.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { .. }))));
}

#[test]
fn an_artist_drills_into_its_ways_in_without_asking_again() {
    // The entry points arrived with the artist list, so going in costs
    // nothing — the whole reason the server sends them inline.
    let mut app = connected_app();
    browsing(&mut app, &["seed"], 0);
    app.handle_action(Action::SelectTab(discover_tab(&app)));
    app.discover.state.select(Some(1)); // "Similar artists"
    app.handle_action(Action::Activate);

    app.apply_event(Event::Discover {
        node: DiscoverNode::Artists,
        data: DiscoverData::Artists(vec![
            similar_artist("Near Artist", 0.91, &["in-one", "in-two"]),
            similar_artist("Other", 0.80, &[]),
        ]),
        note: None,
    });
    assert_eq!(app.discover.entries.len(), 3, "parent plus two artists");

    app.discover.state.select(Some(1));
    let effects = app.handle_action(Action::Activate);
    assert!(effects.is_empty(), "no request — the doorways were already here");
    assert_eq!(*app.discover_node(), DiscoverNode::Artist("Near Artist".into()));
    assert_eq!(app.discover.entries.len(), 3, "parent plus two ways in");

    // And back out again, still without asking.
    assert!(app.handle_action(Action::Back).is_empty());
    assert_eq!(*app.discover_node(), DiscoverNode::Artists);
    assert_eq!(app.discover.entries.len(), 3);
}

#[test]
fn a_discover_reply_for_a_view_already_left_is_dropped() {
    let mut app = connected_app();
    browsing(&mut app, &["seed"], 0);
    app.handle_action(Action::SelectTab(discover_tab(&app)));
    app.handle_action(Action::Activate); // into Tracks
    app.handle_action(Action::Back); // and straight back out

    app.apply_event(Event::Discover {
        node: DiscoverNode::Tracks,
        data: DiscoverData::Tracks(vec![track("late")]),
        note: None,
    });
    assert_eq!(*app.discover_node(), DiscoverNode::Root);
    assert_eq!(app.discover.entries.len(), 2, "still the mode menu");
}

#[test]
fn discovery_with_nothing_to_go_on_says_so() {
    let mut app = connected_app();
    app.handle_action(Action::SelectTab(discover_tab(&app)));
    assert!(app.discover_seed.is_none(), "nothing playing, nothing highlighted");

    assert!(app.handle_action(Action::Activate).is_empty());
    assert!(app.message.as_ref().unwrap().text.contains("play or highlight a track"));
}

#[test]
fn the_browser_opens_where_the_server_thinks_best() {
    // With one library the old opening screen was a list of exactly one
    // row, which everyone stepped through to reach any music.
    let mut app = App::new(Some("http://host:3000".into()), Some("t".into()), None);
    let effects = app.apply_event(Event::Connected {
        server: "http://host:3000".into(),
        id: "http://host:3000".into(),
        username: None,
        token: None,
        ping: Box::new(Default::default()),
    });
    assert!(
        effects.contains(&Effect::Api(ApiCmd::Browse(crate::api::BEST_START.into()))),
        "got {effects:?}"
    );

    // A remembered path still wins — being put back where you were beats
    // being taken somewhere tidy.
    let mut app = App::new(Some("http://host:3000".into()), Some("t".into()), None);
    app.path = "library/Artist".into();
    let effects = app.apply_event(Event::Connected {
        server: "http://host:3000".into(),
        id: "http://host:3000".into(),
        username: None,
        token: None,
        ping: Box::new(Default::default()),
    });
    assert!(
        effects.contains(&Effect::Api(ApiCmd::Browse("library/Artist".into()))),
        "got {effects:?}"
    );
}

#[test]
fn going_up_from_a_library_still_asks_for_the_list() {
    // `~` and `""` are different questions: the second is the only way to
    // see the other libraries, so navigating up must keep using it.
    let mut app = connected_app();
    app.apply_event(Event::Listing(Box::new(listing("/lib/", &[], &[]))));
    assert_eq!(app.path, "lib");
    let effects = app.handle_action(Action::Back);
    assert_eq!(effects, vec![Effect::Api(ApiCmd::Browse(String::new()))]);
}

#[test]
fn a_listing_carries_the_tags_the_server_sent_with_it() {
    let listing = DirListing {
        path: "/library/Artist/Album".into(),
        directories: Vec::new(),
        files: vec![
            FileEntry {
                name: "01 - Song.mp3".into(),
                kind: Some("mp3".into()),
                metadata: Some(FileMetadata {
                    filepath: "library/Artist/Album/01 - Song.mp3".into(),
                    metadata: Some(TrackMetadata {
                        title: Some("Song".into()),
                        artist: Some("Artist".into()),
                        duration: Some(238.655),
                        bpm: Some(130),
                        ..Default::default()
                    }),
                }),
            },
            // On disk but not in the database — scanned since, or never.
            // The row still has to work, just without the extras.
            FileEntry {
                name: "02 - Unscanned.mp3".into(),
                kind: Some("mp3".into()),
                metadata: Some(FileMetadata {
                    filepath: "library/Artist/Album/02 - Unscanned.mp3".into(),
                    metadata: None,
                }),
            },
        ],
    };

    let tracks: Vec<Track> = entries_from_listing(&listing, "")
        .into_iter()
        .filter_map(|e| match e {
            Entry::Track { track, .. } => Some(*track),
            _ => None,
        })
        .collect();

    assert_eq!(tracks[0].metadata.duration, Some(238.655));
    assert_eq!(tracks[0].metadata.bpm, Some(130));
    assert_eq!(tracks[0].display_name(), "Artist - Song");
    assert_eq!(tracks[0].filepath, "library/Artist/Album/01 - Song.mp3");

    assert_eq!(tracks[1].metadata, TrackMetadata::default());
    assert_eq!(tracks[1].display_name(), "02 - Unscanned.mp3", "falls back to the name");
}

#[test]
fn a_listing_without_metadata_still_builds_its_own_paths() {
    // What an older server answers, and what the fallback request gets:
    // no `metadata` key at all. The path has to come from the folder plus
    // the filename, exactly as it did before.
    let listing = DirListing {
        path: "/library/Artist".into(),
        directories: Vec::new(),
        files: vec![FileEntry {
            name: "loose.mp3".into(),
            kind: Some("mp3".into()),
            metadata: None,
        }],
    };
    match &entries_from_listing(&listing, "")[1] {
        Entry::Track { track, .. } => {
            assert_eq!(track.filepath, "library/Artist/loose.mp3");
            assert_eq!(track.metadata, TrackMetadata::default());
        }
        other => panic!("expected a track, got {other:?}"),
    }
}

#[test]
fn a_playlist_file_is_not_offered_as_a_track() {
    // Found live: an album folder held `001-2pac-greatest_hits.m3u`, and
    // Enter queues everything on screen, so the playlist file went into
    // the queue and the decoder rejected it — stopping the album on the
    // first row. mStream's own Auto-DJ excludes these for this reason.
    let listing = DirListing {
        path: "/library/2Pac/Greatest Hits".into(),
        directories: Vec::new(),
        files: vec![
            FileEntry {
                name: "001-2pac-greatest_hits.m3u".into(),
                kind: Some("m3u".into()),
                ..Default::default()
            },
            FileEntry {
                name: "201 - Keep Ya Head Up.mp3".into(),
                kind: Some("mp3".into()),
                ..Default::default()
            },
        ],
    };
    let entries = entries_from_listing(&listing, "");
    let labels: Vec<&str> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Track { label, .. } => Some(label.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(labels, vec!["201 - Keep Ya Head Up.mp3"]);

    // A format this player can't decode is still audio, and still listed:
    // it should fail loudly when played, not disappear from the library.
    assert!(is_audio(Some("opus")));
    assert!(is_audio(Some("mp3")));
    assert!(is_audio(None), "an unclassified file is given the benefit of the doubt");
    for playlist in ["m3u", "M3U", "m3u8", "pls", "cue"] {
        assert!(!is_audio(Some(playlist)), "{playlist} is a list, not a track");
    }
}

#[test]
fn a_track_that_will_not_play_is_skipped_rather_than_stopping_everything() {
    // The queue used to stop dead on the first unplayable file: the
    // message appeared and nothing moved. One bad file should cost one
    // track, not the session.
    let mut app = connected_app();
    app.queue.replace(vec![track("broken"), track("fine"), track("also-fine")]);
    let started = app.handle_action(Action::PlayPause);

    let effects = app.apply_event(failed(&started, "unrecognised format"));
    assert_eq!(app.queue.current, Some(1), "moved on to the next track");
    assert!(effects.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { .. }))));
    let message = &app.message.as_ref().unwrap().text;
    assert!(message.contains("skipping"), "got: {message}");
    assert!(message.contains("broken"), "and names the track: {message}");
}

#[test]
fn a_failure_from_a_track_already_left_is_not_pinned_on_this_one() {
    // An open can take as long as the network does to give up, and the
    // user does not wait. The answer used to arrive with no name on it
    // and be read as being about whatever was playing by then: it said
    // "skipping <the track they had just chosen>", stepped past it
    // unheard, and counted a failure against a track nothing had been
    // tried on.
    let mut app = connected_app();
    app.queue.replace(vec![track("stalled.mp3"), track("chosen.mp3"), track("after.mp3")]);
    let stalled = app.handle_action(Action::PlayPause);
    let chosen = app.handle_action(Action::NextTrack);

    let effects = app.apply_event(failed(&stalled, "connection timed out"));
    let message = app.message.as_ref().map(|m| m.text.as_str()).unwrap_or_default();
    assert!(!message.contains("chosen"), "the wrong track is blamed: {message}");
    assert!(effects.is_empty(), "and the queue does not move");
    assert_eq!(app.queue.current, Some(1), "it stays on the track that was chosen");
    assert_eq!(app.failures, 0, "which has not failed — nothing has been tried on it");
    assert!(app.is_starting(), "and is still starting, so its own status is still wanted");

    // The failure that really is about it still lands.
    app.apply_event(failed(&chosen, "unrecognised format"));
    assert_eq!(app.queue.current, Some(2));
    assert!(app.message.as_ref().unwrap().text.contains("chosen"));
}

#[test]
fn a_queue_where_nothing_plays_gives_up_instead_of_looping() {
    // With repeat on, skipping past every broken track would go round
    // forever, hammering the server and never making a sound.
    let mut app = connected_app();
    app.queue.repeat = Repeat::All;
    app.queue.replace(vec![track("a"), track("b")]);
    let started = app.handle_action(Action::PlayPause);

    let next = app.apply_event(failed(&started, "nope"));
    let effects = app.apply_event(failed(&next, "nope"));
    assert_eq!(effects, vec![Effect::Audio(AudioCmd::Stop)], "it stops rather than wrapping");
    assert_eq!(app.queue.current, None);
    assert!(app.message.as_ref().unwrap().text.contains("nor could the rest"));
}

#[test]
fn one_track_playing_forgives_the_failures_before_it() {
    // The give-up counter is about a *run* of failures. A queue with one
    // bad file every few tracks must keep going indefinitely.
    let mut app = connected_app();
    app.queue.replace(vec![track("a"), track("b")]);
    let started = app.handle_action(Action::PlayPause);
    let effects = app.apply_event(failed(&started, "nope"));
    assert_eq!(app.failures, 1);

    let url = played_url(&effects);
    app.apply_event(Event::Status(PlayerStatus { source: url, ..Default::default() }));
    assert_eq!(app.failures, 0, "a track that loaded clears the run");
}

#[test]
fn a_journey_sets_off_from_what_is_playing() {
    // One keypress when something is already playing: that track is the
    // obvious place to leave from.
    let mut app = connected_app();
    app.queue.replace(vec![track("playing")]);
    app.play_index(0);
    browsing(&mut app, &["far-away"], 0);

    let effects = app.handle_action(Action::StartJourney);
    assert_eq!(
        effects,
        vec![Effect::Api(ApiCmd::Journey {
            start: "playing".into(),
            end: "far-away".into(),
            length: 14,
        })]
    );
    let journey = app.journey.as_ref().unwrap();
    assert!(journey.pending);
    assert_eq!(journey.from.filepath, "playing");
    assert_eq!(journey.to.filepath, "far-away");
}

#[test]
fn with_nothing_playing_a_journey_takes_two_presses() {
    let mut app = connected_app();
    browsing(&mut app, &["a", "b"], 0);

    // First press marks where to set off from and says so.
    assert!(app.handle_action(Action::StartJourney).is_empty());
    assert!(app.journey.is_none(), "no request until there are two ends");
    assert_eq!(app.journey_from.as_ref().unwrap().filepath, "a");
    assert!(app.message.as_ref().unwrap().text.contains("press J"));

    // Second press, on a different track, plots it.
    app.files.state.select(Some(1));
    let effects = app.handle_action(Action::StartJourney);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Api(ApiCmd::Journey { start, end, .. })] if start == "a" && end == "b"
    ));
    assert!(app.journey_from.is_none(), "the pending start is consumed");
}

#[test]
fn a_journey_needs_a_server_that_can_plot_one() {
    let mut app = connected_app();
    app.capabilities.discovery_path = false;
    browsing(&mut app, &["a"], 0);

    assert!(app.handle_action(Action::StartJourney).is_empty());
    assert!(app.journey.is_none());
    assert!(app.message.as_ref().unwrap().text.contains("can't plot journeys"));
}

#[test]
fn a_journey_needs_a_track_not_a_folder() {
    let mut app = connected_app();
    app.files.set(vec![Entry::Dir {
        label: "an album".into(),
        path: "lib/an album".into(),
    }]);
    app.files.state.select(Some(0));

    assert!(app.handle_action(Action::StartJourney).is_empty());
    assert!(app.message.as_ref().unwrap().text.contains("highlight a track"));
}

#[test]
fn changing_the_length_asks_for_a_different_arc() {
    // The stops aren't a list to trim — a shorter journey is a different
    // set of waypoints, so it has to be replotted.
    let mut app = connected_app();
    app.queue.replace(vec![track("from")]);
    app.play_index(0);
    browsing(&mut app, &["to"], 0);
    app.handle_action(Action::StartJourney);
    let asked = app.journey.as_ref().unwrap().length;
    app.apply_event(Event::Journey {
        stops: vec![stop("from", 0.0), stop("mid", 0.5), stop("to", 1.0)],
        note: None,
        length: asked,
    });
    assert!(!app.journey.as_ref().unwrap().pending);

    let effects = app.handle_action(Action::SeekForward);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Api(ApiCmd::Journey { length: 16, .. })]
    ));
    assert!(app.journey.as_ref().unwrap().pending, "waiting on the new arc");

    // And it stops at the ends the server accepts rather than asking for
    // a length it would reject.
    for _ in 0..40 {
        app.handle_action(Action::Back);
    }
    assert_eq!(app.journey.as_ref().unwrap().length, 4);
    assert!(app.handle_action(Action::Back).is_empty(), "no request at the floor");
}

#[test]
fn queueing_a_journey_replaces_the_queue_and_starts_it() {
    let mut app = connected_app();
    app.queue.replace(vec![track("old")]);
    app.play_index(0);
    browsing(&mut app, &["to"], 0);
    app.handle_action(Action::StartJourney);
    let asked = app.journey.as_ref().unwrap().length;
    app.apply_event(Event::Journey {
        stops: vec![stop("from", 0.0), stop("mid", 0.5), stop("to", 1.0)],
        note: None,
        length: asked,
    });

    let effects = app.handle_action(Action::Submit);
    assert!(app.journey.is_none(), "the panel closes once it's the queue");
    assert_eq!(
        app.queue.items.iter().map(|t| t.filepath.as_str()).collect::<Vec<_>>(),
        vec!["from", "mid", "to"],
        "the arc is the queue, in order"
    );
    assert_eq!(app.queue.current, Some(0));
    assert!(effects.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { .. }))));
}

#[test]
fn search_replies_that_pass_each_other_cannot_swap_the_results() {
    // Reads answer on their own threads now, so the reply for an old
    // search can land after the reply for the current one. Only the
    // query last submitted is still wanted.
    let mut app = connected_app();
    app.handle_action(Action::SelectTab(3));
    for c in "one".chars() {
        app.handle_action(Action::Input(c));
    }
    app.handle_action(Action::Submit);
    app.handle_action(Action::StartSearch);
    for _ in 0.."one".len() {
        app.handle_action(Action::Backspace);
    }
    for c in "two".chars() {
        app.handle_action(Action::Input(c));
    }
    app.handle_action(Action::Submit);

    let stale = app.apply_event(Event::SearchResults {
        query: "one".into(),
        results: Box::default(),
    });
    assert!(stale.is_empty());
    assert_eq!(app.search_summary, None, "the overtaken search says nothing");

    app.apply_event(Event::SearchResults {
        query: "two".into(),
        results: Box::default(),
    });
    assert_eq!(app.search_summary.as_deref(), Some("0 matches"), "the current one lands");
}

#[test]
fn a_journey_reply_for_a_length_since_changed_keeps_waiting() {
    let mut app = connected_app();
    app.queue.replace(vec![track("from")]);
    app.play_index(0);
    browsing(&mut app, &["to"], 0);
    app.handle_action(Action::StartJourney);
    let first = app.journey.as_ref().unwrap().length;
    app.handle_action(Action::SeekForward); // ask for a longer arc

    // The reply to the original length answers a request nobody is
    // tracking any more: the stops stay empty and the panel keeps waiting.
    app.apply_event(Event::Journey {
        stops: vec![stop("stale", 0.0)],
        note: None,
        length: first,
    });
    let journey = app.journey.as_ref().unwrap();
    assert!(journey.stops.is_empty(), "an arc of the wrong length is not this arc");
    assert!(journey.pending, "still waiting on the length actually asked for");

    app.apply_event(Event::Journey {
        stops: vec![stop("fresh", 0.0)],
        note: None,
        length: first + 2,
    });
    let journey = app.journey.as_ref().unwrap();
    assert_eq!(journey.stops.len(), 1);
    assert!(!journey.pending);
}

#[test]
fn drilling_out_of_search_lights_the_search_tab_spinner() {
    // The destination now travels with the command, so note_pending can't
    // forget a case: the old SearchDrill command wasn't in its table at
    // all, and this spinner never lit.
    let mut app = connected_app();
    app.handle_action(Action::SelectTab(3));
    app.search_submitted = Some("moon".into());
    app.apply_event(Event::SearchResults {
        query: "moon".into(),
        results: Box::new(crate::api::types::SearchResults {
            artists: vec![crate::api::types::SearchGroup {
                name: "Moon Hooch".into(),
                album_art_file: None,
            }],
            ..Default::default()
        }),
    });
    app.handle_action(Action::Activate); // into the Artists class
    app.handle_action(Action::Activate); // into the artist — a request
    assert!(app.search.loading, "the tab that asked is the tab that spins");
}

#[test]
fn a_journey_reply_that_arrives_after_it_is_closed_is_dropped() {
    let mut app = connected_app();
    app.queue.replace(vec![track("from")]);
    app.play_index(0);
    browsing(&mut app, &["to"], 0);
    app.handle_action(Action::StartJourney);
    app.handle_action(Action::Cancel);

    app.apply_event(Event::Journey { stops: vec![stop("late", 0.0)], note: None, length: 14 });
    assert!(app.journey.is_none(), "it stays closed");
    assert_eq!(app.queue.items.len(), 1, "and nothing is queued behind the user's back");
}

#[test]
fn the_panel_only_offers_rows_the_server_can_honour() {
    let mut app = connected_app();
    app.handle_action(Action::OpenDjPanel);
    let rows = &app.dj_panel.as_ref().unwrap().rows;
    assert!(rows.contains(&DjRow::Tightness), "this server has the index");
    assert!(rows.contains(&DjRow::Anchor));

    // Without it, a row promising a sonic pool would be a lie.
    let mut app = connected_app();
    app.capabilities = Default::default();
    app.handle_action(Action::OpenDjPanel);
    let rows = &app.dj_panel.as_ref().unwrap().rows;
    assert!(!rows.contains(&DjRow::Tightness));
    assert!(!rows.contains(&DjRow::Anchor));
    assert!(rows.contains(&DjRow::Tempo), "the rest is still there");
}

/// A `[keys]` section from a config file.
fn keys(pairs: &[(&str, &[&str])]) -> std::collections::BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(name, specs)| {
            ((*name).to_string(), specs.iter().map(|s| (*s).to_string()).collect())
        })
        .collect()
}

fn press(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

#[test]
fn a_rebound_action_answers_to_its_new_key_and_not_the_old_one() {
    let (map, warnings) = Keymap::default()
        .with_overrides(&keys(&[("next-track", &["b"])]));
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(map.action(press('b'), InputMode::Normal), Some(Action::NextTrack));
    // Naming an action replaces its keys rather than adding to them, so
    // the default is gone — that is what makes a binding movable.
    assert_eq!(map.action(press('n'), InputMode::Normal), None);
    // Everything unmentioned is untouched.
    assert_eq!(map.action(press('p'), InputMode::Normal), Some(Action::PrevTrack));
}

#[test]
fn taking_a_key_from_another_action_just_works() {
    // Binding `d` to something new should not also require unbinding it
    // from remove-from-queue first.
    let (map, warnings) =
        Keymap::default().with_overrides(&keys(&[("next-track", &["d"])]));
    assert!(warnings.is_empty(), "no complaint about the old owner: {warnings:?}");
    assert_eq!(map.action(press('d'), InputMode::Normal), Some(Action::NextTrack));
    // And the action that lost it still works by its other keys, or not
    // at all if it had none.
    assert_eq!(map.action(press('C'), InputMode::Normal), Some(Action::ClearQueue));
}

#[test]
fn an_action_can_be_unbound_entirely() {
    let (map, _) = Keymap::default().with_overrides(&keys(&[("clear-queue", &[])]));
    assert_eq!(map.action(press('C'), InputMode::Normal), None);
    // And it drops off the help, rather than listing a key that is gone.
    assert!(!map.help_rows().iter().any(|(_, what)| *what == "clear the queue"));
}

#[test]
fn a_line_of_nothing_but_typos_leaves_the_binding_alone() {
    // An explicit `[]` means "unbind this". A line where every key was
    // unreadable means the line is wrong, and taking the key away would
    // punish a typo far harder than it deserves.
    let (map, warnings) =
        Keymap::default().with_overrides(&keys(&[("volume-up", &["not a key"])]));
    assert_eq!(map.action(press('+'), InputMode::Normal), Some(Action::VolumeUp));
    assert!(warnings.iter().any(|w| w.contains("left as it was")), "{warnings:?}");

    // But one good key among the bad still takes effect.
    let (map, _) = Keymap::default()
        .with_overrides(&keys(&[("volume-up", &["not a key", "V"])]));
    assert_eq!(map.action(press('V'), InputMode::Normal), Some(Action::VolumeUp));
    assert_eq!(map.action(press('+'), InputMode::Normal), None);
}

#[test]
fn the_help_follows_the_bindings_that_are_actually_in_force() {
    let (map, _) = Keymap::default().with_overrides(&keys(&[("quit", &["ctrl+q", "Z"])]));
    let quit = map
        .help_rows()
        .into_iter()
        .find(|(_, what)| *what == "quit")
        .expect("quit is still listed");
    assert_eq!(quit.0, "^q Z", "the help shows the new keys, in order");
}

#[test]
fn a_broken_keys_section_costs_only_the_broken_line() {
    let (map, warnings) = Keymap::default().with_overrides(&keys(&[
        ("next-track", &["b"]),
        ("teleport", &["t"]),
        ("previous-track", &["not a key"]),
    ]));
    assert!(warnings.iter().any(|w| w.contains("no action called 'teleport'")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("'not a key' is not a key")), "{warnings:?}");
    // The good line still took effect, and the player still works.
    assert_eq!(map.action(press('b'), InputMode::Normal), Some(Action::NextTrack));
    assert_eq!(map.action(press(' '), InputMode::Normal), Some(Action::PlayPause));
    // The line that was all typos kept its default rather than vanishing.
    assert_eq!(map.action(press('p'), InputMode::Normal), Some(Action::PrevTrack));
}

#[test]
fn two_actions_claiming_one_key_is_reported() {
    let (map, warnings) = Keymap::default()
        .with_overrides(&keys(&[("next-track", &["z"]), ("previous-track", &["z"])]));
    assert!(warnings.iter().any(|w| w.contains("bound to both")), "{warnings:?}");
    // The first claim wins outright — stripping the key from each in turn
    // would leave it doing nothing, which is nobody's intent.
    assert_eq!(map.action(press('z'), InputMode::Normal), Some(Action::NextTrack));
    // And the one that lost keeps the binding it already had, rather than
    // being left with none at all.
    assert_eq!(map.action(press('p'), InputMode::Normal), Some(Action::PrevTrack));
}

#[test]
fn ctrl_c_cannot_be_taken_away() {
    // The way out has to survive any config, including a hostile one.
    let (map, _) = Keymap::default().with_overrides(&keys(&[("quit", &["Z"])]));
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(map.action(ctrl_c, InputMode::Normal), Some(Action::Quit));
    assert_eq!(map.action(ctrl_c, InputMode::Panel), Some(Action::Quit));
    assert_eq!(map.action(ctrl_c, InputMode::Editing), Some(Action::Quit));
}

#[test]
fn key_specs_round_trip_and_forgive_the_obvious_variants() {
    for spec in ["j", "G", "space", "enter", "esc", "tab", "up", "pagedown", "ctrl+d", "?"] {
        let parsed = Key::parse(spec).unwrap_or_else(|| panic!("could not read {spec:?}"));
        // What it writes back must read as the same key — case included,
        // since `G` and `g` are different bindings.
        assert_eq!(Key::parse(&parsed.spec()), Some(parsed), "{spec} did not round-trip");
    }
    assert_eq!(Key::parse("G").unwrap().spec(), "G", "a capital stays capital");
    // Same key, several spellings — this is a hand-edited file.
    for spec in ["ctrl+d", "Ctrl+D", "ctrl-d", "^d"] {
        assert_eq!(Key::parse(spec), Some(ctrl('d')), "{spec}");
    }
    assert_eq!(Key::parse("PgDn"), Some(key(KeyCode::PageDown)));
    assert_eq!(Key::parse("Escape"), Some(key(KeyCode::Esc)));
    // With Ctrl the capital is folded away: the terminal sends the
    // lowercase form, so `ctrl+D` would otherwise never fire.
    assert_eq!(Key::parse("ctrl+D"), Some(ctrl('d')));
    for bad in ["", "  ", "nonsense", "ctrl+"] {
        assert_eq!(Key::parse(bad), None, "{bad:?} is not a key");
    }
}

#[test]
fn the_dumped_config_is_the_config_that_would_be_read_back() {
    // `mstream-player keys` is only useful if pasting its output changes
    // nothing — which means the writer and the parser have to agree.
    let dumped = Keymap::default().to_config_toml();
    let parsed: crate::config::Config =
        toml::from_str(&format!("version = 1\n{dumped}")).expect("valid TOML");
    let (map, warnings) = Keymap::default().with_overrides(&parsed.keys);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(map.help_rows(), Keymap::default().help_rows(), "round trip changed something");
}

#[test]
fn a_modifier_is_part_of_the_key_not_decoration() {
    // Ctrl was only ever checked for `c`, so Ctrl+D arrived as plain `d`
    // and quietly removed a queue entry when a vim user reached for
    // half-page-down.
    let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert_eq!(map_key(ctrl_d, InputMode::Normal), Some(Action::HalfPageDown));
    let plain_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
    assert_eq!(map_key(plain_d, InputMode::Normal), Some(Action::RemoveFromQueue));

    // And an unbound Ctrl combination does nothing at all rather than
    // falling through to the bare letter.
    let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(map_key(ctrl_r, InputMode::Normal), None);
    // Including inside a panel, where bare letters are passed through.
    assert_eq!(map_key(ctrl_r, InputMode::Panel), None);
    let plain_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
    assert_eq!(map_key(plain_p, InputMode::Panel), Some(Action::Input('p')));
}

#[test]
fn jumping_to_what_is_playing_goes_to_the_queue() {
    // Browsing takes you a long way from the music — several tabs and two
    // drill-downs — so one key has to lead back.
    let mut app = connected_app();
    let named = Track {
        filepath: "b".into(),
        metadata: TrackMetadata {
            artist: Some("Band".into()),
            title: Some("Song".into()),
            ..Default::default()
        },
    };
    app.queue.replace(vec![track("a"), named, track("c")]);
    app.play_index(1);
    // Wander off.
    app.focus = Focus::Browser;
    app.queue.state.select(Some(0));

    assert!(app.handle_action(Action::JumpToPlaying).is_empty());
    assert_eq!(app.focus, Focus::Queue);
    assert_eq!(app.queue.state.selected(), Some(1), "the playing row, not the first");
    assert!(app.message.as_ref().unwrap().text.contains("Band - Song"));
}

#[test]
fn jumping_with_nothing_playing_says_so() {
    let mut app = connected_app();
    app.queue.replace(vec![track("a")]);
    assert!(app.handle_action(Action::JumpToPlaying).is_empty());
    assert_eq!(app.focus, Focus::Browser, "the cursor stays where it was");
    assert!(app.message.as_ref().unwrap().text.contains("nothing is playing"));
}

#[test]
fn the_shifted_seek_keys_move_by_a_minute() {
    let mut app = connected_app();
    app.status.source = "http://host/a.mp3".into();
    app.status.position = 200.0;

    let effects = app.handle_action(Action::SeekForwardFar);
    assert_eq!(effects, vec![Effect::Audio(AudioCmd::Seek(260.0))]);
    let effects = app.handle_action(Action::SeekBackwardFar);
    assert_eq!(effects, vec![Effect::Audio(AudioCmd::Seek(140.0))]);
    // And the fine keys still move by five.
    let effects = app.handle_action(Action::SeekForward);
    assert_eq!(effects, vec![Effect::Audio(AudioCmd::Seek(205.0))]);
}

#[test]
fn half_a_page_is_half_of_a_page() {
    let mut app = connected_app();
    let names: Vec<String> = (0..40).map(|i| i.to_string()).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    browsing(&mut app, &refs, 0);

    app.handle_action(Action::PageDown);
    let after_page = app.files.state.selected().unwrap();
    app.handle_action(Action::First);
    app.handle_action(Action::HalfPageDown);
    assert_eq!(app.files.state.selected(), Some(after_page / 2));
}

#[test]
fn the_panel_binds_its_own_keys_rather_than_the_players() {
    // Found live: `p` reached the panel as "previous track", so the
    // sample key silently did nothing.
    let key = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    assert_eq!(map_key(key('p'), InputMode::Normal), Some(Action::PrevTrack));
    assert_eq!(map_key(key('p'), InputMode::Panel), Some(Action::Input('p')));

    // Left/right come from three shapes, all meaning "adjust".
    for c in ['h', '['] {
        assert_eq!(map_key(key(c), InputMode::Panel), Some(Action::Back));
    }
    for c in ['l', ']'] {
        assert_eq!(map_key(key(c), InputMode::Panel), Some(Action::SeekForward));
    }
    // And the panel's own key closes it, as does Esc.
    assert_eq!(map_key(key('D'), InputMode::Panel), Some(Action::Cancel));
    assert_eq!(
        map_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), InputMode::Panel),
        Some(Action::Cancel)
    );
}

#[test]
fn the_panel_owns_the_keyboard_while_it_is_open() {
    let mut app = connected_app();
    assert_eq!(app.input_mode(), InputMode::Normal);
    app.handle_action(Action::OpenDjPanel);
    assert_eq!(app.input_mode(), InputMode::Panel);
    app.handle_action(Action::Cancel);
    assert_eq!(app.input_mode(), InputMode::Normal);
}

#[test]
fn the_panel_takes_the_keys_the_player_would_otherwise_use() {
    // Space is the toggle inside the genre chooser and pause outside it;
    // arrows move settings, not the browser. Leaking either would make
    // editing settings play music.
    let mut app = connected_app();
    app.queue.replace(vec![track("a"), track("b")]);
    app.handle_action(Action::OpenDjPanel);

    let effects = app.handle_action(Action::PlayPause);
    assert!(effects.is_empty(), "no playback from inside the panel");
    app.handle_action(Action::Down);
    assert_eq!(app.dj_panel.as_ref().unwrap().row, 1, "moves the panel, not the queue");
    assert_eq!(app.queue.current, None);

    app.handle_action(Action::Cancel);
    assert!(app.dj_panel.is_none(), "Esc closes it");
}

#[test]
fn adjusting_a_row_changes_the_setting_it_names() {
    let mut app = connected_app();
    app.handle_action(Action::OpenDjPanel);

    // Row 0 is the mode; stepping right cycles it.
    app.handle_action(Action::SeekForward);
    assert_eq!(app.autodj, AutoDjMode::Similar);

    // Tightness moves in useful steps and stops at the ends rather than
    // wrapping — a slider that wraps loses your place.
    app.dj_panel.as_mut().unwrap().row = 1;
    assert_eq!(app.dj_panel.as_ref().unwrap().selected(), DjRow::Tightness);
    app.handle_action(Action::SeekForward);
    assert_eq!(app.dj.sonic_tightness, 5);
    for _ in 0..40 {
        app.handle_action(Action::SeekForward);
    }
    assert_eq!(app.dj.sonic_tightness, 100, "clamped at the top");
    for _ in 0..40 {
        app.handle_action(Action::Back);
    }
    assert_eq!(app.dj.sonic_tightness, 0, "and at the bottom, which is off");
}

#[test]
fn panel_settings_are_remembered() {
    let mut app = connected_app();
    app.handle_action(Action::OpenDjPanel);
    app.dj_panel.as_mut().unwrap().row = 1;
    app.handle_action(Action::SeekForward); // tightness 5

    let saved = app.prefs();
    assert_eq!(saved.dj.sonic_tightness, 5);
    let restored = App::new(None, None, None).with_prefs(&saved);
    assert_eq!(restored.dj, app.dj);
}

#[test]
fn g_and_shift_g_jump_to_the_ends_of_the_panel() {
    // Found live: both keys were bound in panel mode but the settings
    // list ignored them, so `G` silently did nothing.
    let mut app = connected_app();
    app.handle_action(Action::OpenDjPanel);
    app.handle_action(Action::Last);
    let panel = app.dj_panel.as_ref().unwrap();
    assert_eq!(panel.selected(), DjRow::Genres, "the last row");
    app.handle_action(Action::First);
    assert_eq!(app.dj_panel.as_ref().unwrap().selected(), DjRow::Mode);
}

#[test]
fn choosing_a_genre_switches_the_filter_on() {
    // Picking genres while the mode is off would do nothing at all, which
    // reads as the chooser being broken.
    let mut app = connected_app();
    app.handle_action(Action::OpenDjPanel);
    app.dj_panel.as_mut().unwrap().row = app.dj_panel.as_ref().unwrap().rows.len() - 1;
    assert_eq!(app.dj_panel.as_ref().unwrap().selected(), DjRow::Genres);

    let effects = app.handle_action(Action::Activate);
    assert_eq!(effects, vec![Effect::Api(ApiCmd::Genres)]);
    assert!(app.dj_panel.as_ref().unwrap().genres.as_ref().unwrap().loading);

    app.apply_event(Event::Genres(vec![
        Genre { name: "Ambient".into(), track_count: Some(4) },
        Genre { name: "Techno".into(), track_count: Some(9) },
    ]));
    let picker = app.dj_panel.as_ref().unwrap().genres.as_ref().unwrap();
    assert_eq!(picker.all, vec!["Ambient", "Techno"]);
    assert!(!picker.loading);

    app.handle_action(Action::PlayPause); // toggle "Ambient"
    assert_eq!(app.dj.genres, vec!["Ambient"]);
    assert_eq!(app.dj.genre_mode, dj::GenreMode::Whitelist, "switched on for you");

    // And toggling it back off leaves nothing selected.
    app.handle_action(Action::PlayPause);
    assert!(app.dj.genres.is_empty());

    app.handle_action(Action::Submit);
    assert!(app.dj_panel.as_ref().unwrap().genres.is_none(), "Enter closes the chooser");
    assert!(app.dj_panel.is_some(), "back to the panel, not out of it");
}

#[test]
fn sampling_asks_for_picks_without_queueing_any() {
    let mut app = connected_app();
    app.handle_action(Action::OpenDjPanel);

    let effects = app.handle_action(Action::Input('p'));
    match effects.as_slice() {
        [Effect::Api(ApiCmd::AutoDjSample { count, .. })] => assert_eq!(*count, 3),
        other => panic!("unexpected {other:?}"),
    }
    assert!(app.dj_panel.as_ref().unwrap().sample_pending);
    // A second press while one is out must not pile on.
    assert!(app.handle_action(Action::Input('p')).is_empty());

    app.apply_event(Event::AutoDjSample {
        tracks: vec![track("one"), track("two")],
        pool: Some(crate::api::types::SonicReport {
            similarity: Some(0.71),
            pool_size: 1247,
        }),
        note: None,
    });
    let panel = app.dj_panel.as_ref().unwrap();
    assert_eq!(panel.sample.len(), 2);
    assert_eq!(panel.pool.as_ref().unwrap().pool_size, 1247);
    assert!(!panel.sample_pending);
    assert!(app.queue.items.is_empty(), "a sample is not a queue");
}

#[test]
fn a_request_carries_the_session_the_panel_is_tuning() {
    let mut app = connected_app();
    app.autodj = AutoDjMode::BpmKey;
    app.dj.sonic_tightness = 50;
    app.dj.artist_cooldown = 2;

    // Two tracks played, newest first, with the artist of each.
    app.queue.replace(vec![
        track_by("a", "Alpha"),
        track_by("b", "Beta"),
        track_by("c", "Gamma"),
    ]);
    app.play_index(0);
    app.play_index(1);
    let effects = app.play_index(2);

    match autodj_effect(&effects).expect("the queue ran out") {
        ApiCmd::AutoDj(request) => {
            assert_eq!(request.anchors, vec!["c", "b", "a"], "newest first");
            assert_eq!(request.recent_artists, vec!["Gamma", "Beta", "Alpha"]);
            assert!(request.sonic_available, "this server has the index");
            assert_eq!(request.settings.sonic_tightness, 50);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn the_cooldown_list_does_not_repeat_an_artist() {
    // Three tracks by the same artist should not spend the whole cooldown.
    let mut app = connected_app();
    app.queue.replace(vec![
        track_by("a", "Alpha"),
        track_by("b", "Alpha"),
        track_by("c", "Beta"),
    ]);
    app.play_index(0);
    app.play_index(1);
    app.play_index(2);
    assert_eq!(app.recent_artists(), vec!["Beta", "Alpha"]);
}

#[test]
fn autodj_skips_a_mode_the_server_cannot_serve() {
    // Default install: no embedding index. Offering "similar" would spend
    // a keystroke and a round trip to land on tempo+key anyway.
    let mut app = connected_app();
    app.capabilities = Default::default();

    app.handle_action(Action::ToggleAutoDj);
    assert_eq!(app.autodj, AutoDjMode::BpmKey, "straight past similar");
    app.handle_action(Action::ToggleAutoDj);
    assert_eq!(app.autodj, AutoDjMode::Off, "and the cycle still closes");
}

#[test]
fn a_remembered_similar_mode_is_dropped_on_a_server_without_the_index() {
    // Preferences are global; capabilities are per-server. Reconnecting
    // elsewhere must not leave a mode selected that does something else.
    let saved = crate::config::PlayerPrefs {
        volume: 1.0,
        repeat: "off".into(),
        shuffle: false,
        autodj: "similar".into(),
        dj: Default::default(),
    };
    let mut app = App::new(None, None, None).with_prefs(&saved);
    assert_eq!(app.autodj, AutoDjMode::Similar);

    app.apply_event(Event::Connected {
        server: "http://plain:3000".into(),
        id: "http://plain:3000".into(),
        username: None,
        token: None,
        ping: Box::new(Default::default()),
    });
    assert_eq!(app.autodj, AutoDjMode::BpmKey);
    assert!(app.message.as_ref().unwrap().text.contains("similarity index"));

    // On a server that has one, the remembered mode is left alone.
    let mut app = App::new(None, None, None).with_prefs(&saved);
    app.apply_event(Event::Connected {
        server: "http://rich:3000".into(),
        id: "http://rich:3000".into(),
        username: None,
        token: None,
        ping: Box::new(crate::api::types::Ping {
            discovery: true,
            ..Default::default()
        }),
    });
    assert_eq!(app.autodj, AutoDjMode::Similar);
    assert!(app.capabilities.discovery);
}

#[test]
fn switching_autodj_on_with_an_empty_queue_starts_it() {
    let mut app = connected_app();
    let effects = app.handle_action(Action::ToggleAutoDj);
    match autodj_effect(&effects).expect("a request goes out") {
        ApiCmd::AutoDj(request) => {
            assert_eq!(request.mode, AutoDjMode::Similar);
            assert!(request.seed.is_none());
            assert!(request.ignore_list.is_empty());
        }
        other => panic!("unexpected command {other:?}"),
    }
}

#[test]
fn switching_autodj_on_does_not_jump_a_queue_the_user_built() {
    let mut app = connected_app();
    app.queue.replace(vec![track("a"), track("b")]);
    let effects = app.handle_action(Action::ToggleAutoDj);
    assert!(autodj_effect(&effects).is_none(), "there are tracks waiting already");
}

#[test]
fn autodj_requests_only_once_the_queue_has_nothing_after_the_current_track() {
    let mut app = connected_app();
    app.autodj = AutoDjMode::BpmKey;
    app.queue.replace(vec![track("a"), track("b")]);

    let effects = app.play_index(0);
    assert!(autodj_effect(&effects).is_none(), "one track still waiting");

    let effects = app.play_index(1);
    let cmd = autodj_effect(&effects).expect("the last track should pull in another");
    match cmd {
        ApiCmd::AutoDj(request) => {
            assert_eq!(request.mode, AutoDjMode::BpmKey);
            assert_eq!(
                request.seed.as_ref().unwrap().filepath,
                "b",
                "seeded on what's playing"
            );
        }
        other => panic!("unexpected command {other:?}"),
    }

    // A second trigger while the first is unanswered must not pile on.
    assert!(app.maybe_autodj().is_empty());
}

#[test]
fn autodj_picks_are_appended_and_deduped() {
    let mut app = connected_app();
    app.autodj = AutoDjMode::Similar;
    app.queue.replace(vec![track("a")]);
    app.play_index(0);

    app.apply_event(Event::AutoDjPick {
        // The first candidate is already queued, so the second wins.
        candidates: vec![track("a"), track("b")],
        ignore_list: vec![7],
        note: None,
    });
    assert_eq!(app.queue.items.len(), 2);
    assert_eq!(app.queue.items[1].filepath, "b");
    assert_eq!(app.autodj_ignore, vec![7], "the cursor is kept for the next request");
    assert!(!app.autodj_pending);
}

#[test]
fn an_autodj_pick_starts_playing_when_the_queue_ran_dry() {
    let mut app = connected_app();
    app.autodj = AutoDjMode::Similar;
    // Nothing playing, nothing queued.
    let effects = app.apply_event(Event::AutoDjPick {
        candidates: vec![track("fresh")],
        ignore_list: Vec::new(),
        note: None,
    });
    assert_eq!(app.queue.items.len(), 1);
    assert!(matches!(effects[0], Effect::Audio(AudioCmd::Play { .. })));
    assert_eq!(app.queue.current, Some(0));
}

#[test]
fn a_pick_arriving_after_autodj_is_switched_off_is_dropped() {
    let mut app = connected_app();
    app.autodj = AutoDjMode::Similar;
    app.autodj_pending = true;
    app.autodj = AutoDjMode::Off;

    let effects = app.apply_event(Event::AutoDjPick {
        candidates: vec![track("late")],
        ignore_list: Vec::new(),
        note: None,
    });
    assert!(app.queue.items.is_empty());
    assert!(effects.is_empty());
}

#[test]
fn a_fallback_note_is_surfaced_instead_of_the_track_name() {
    let mut app = connected_app();
    app.autodj = AutoDjMode::Similar;
    app.apply_event(Event::AutoDjPick {
        candidates: vec![track("x")],
        ignore_list: Vec::new(),
        note: Some("this track hasn't been analysed yet — matching tempo and key".into()),
    });
    let message = app.message.as_ref().unwrap();
    assert!(message.text.contains("analysed"), "the user learns why it fell back");
}

#[test]
fn selection_stays_in_bounds() {
    let mut pane = Pane::default();
    pane.set(vec![Entry::Parent, Entry::Playlist { name: "x".into() }]);
    pane.move_by(-5);
    assert_eq!(pane.state.selected(), Some(0));
    pane.move_by(50);
    assert_eq!(pane.state.selected(), Some(1));

    // An empty pane has nothing selected and must not panic.
    pane.set(Vec::new());
    pane.move_by(1);
    assert_eq!(pane.state.selected(), None);
}
