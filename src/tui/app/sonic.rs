//! The Sonic Path tab: pick two songs, and the server fills the road between
//! them.
//!
//! The webapp's panel, port for port — two ends, a length, Build, and then
//! the path with what can be done to it. Two things are done the terminal's
//! way rather than the browser's:
//!
//! * **The rows are ordinary [`Entry`] values**, so the tab costs no new
//!   navigation. `↑↓`, `Enter`, `a`, the filter, the queue column and the
//!   trail all go on meaning what they mean on every other tab, and the
//!   stops come back as real [`Entry::Track`] rows — `Enter` on one plays
//!   the path from there, `a` queues just that stop.
//! * **The armed picker is answered by `Enter`**, not by a click. Arming
//!   drops the user in the file browser with a banner along the foot; the
//!   next track they open lands in the field instead of on the speakers.
//!   [`App::capture`] holds the arming, which is why it lives on the
//!   App and not on [`SonicPath`]: it is answered from whichever tab they
//!   wander into looking for the song.

use super::*;

/// The band the length slider spans, and how many cells draw it.
const LENGTH_BAR: u32 = 10;

impl App {
    // ── Getting there ───────────────────────────────────────────────────────

    /// Show the tab, refusing politely on a server that cannot plot paths.
    ///
    /// Leaves the full-screen view on the way, because this is navigation
    /// and the tab it navigates to is drawn on the browser screen. Without
    /// that, `J` from the full-screen view rewrote the panel and could fire
    /// a build request with nothing on screen to show for either.
    pub(super) fn open_sonic_tab(&mut self) -> Vec<Effect> {
        if !self.capabilities.discovery_path {
            self.error("this server can't plot sonic paths — it has no discovery index");
            return Vec::new();
        }
        self.fullscreen = false;
        match self.tabs().iter().position(|tab| *tab == Tab::SonicPath) {
            Some(index) => self.select_tab(index),
            None => Vec::new(),
        }
    }

    /// `J` on a track: the fast way in. What's playing is the natural place
    /// to set off from, so this usually opens the tab with both ends filled
    /// and the path already being plotted.
    pub(super) fn start_journey(&mut self) -> Vec<Effect> {
        if !self.capabilities.discovery_path {
            self.error("this server can't plot sonic paths — it has no discovery index");
            return Vec::new();
        }
        // The full-screen Discover panel has a highlighted row of its own,
        // and it is the one on screen — `a` already prefers it for the same
        // reason. Without this, `J` there travelled to whatever the hidden
        // browser happened to be pointing at.
        let picked = self.now_discover_selected().or_else(|| self.selected_track());
        let Some(picked) = picked else {
            self.error("highlight a track to travel to");
            return Vec::new();
        };

        // Aiming somewhere new is a new path, not an edit of the last one.
        self.sonic.view = SonicView::Setup;
        self.sonic.stops.clear();
        self.sonic.fetched = false;
        self.sonic.note = None;
        self.sonic.end = Some(picked);
        if self.sonic.start.is_none() {
            self.sonic.start = self.now_playing.clone();
        }
        self.sonic_stack.restart();

        let mut effects = self.open_sonic_tab();
        // Both ends known means there is nothing left to ask, so ask.
        if self.sonic.ready() {
            effects.extend(self.build_sonic_path());
        } else {
            self.info("choose where to set off from, then build the path");
        }
        effects
    }

    /// Everything about the path forgotten — what a fresh connection gets,
    /// and what "Start over" does.
    pub(crate) fn reset_sonic_path(&mut self) {
        self.sonic = SonicPath::default();
        self.capture = None;
        self.sonic_playlist_name = None;
        self.sonic_stack.restart();
        self.refresh_sonic_rows();
    }

    // ── Rows ────────────────────────────────────────────────────────────────

    /// Rebuild the tab's rows in place: every detail on them is live, and
    /// [`Pane::set`] would throw the cursor away on every keystroke.
    pub(super) fn refresh_sonic_rows(&mut self) {
        let mut entries = self.sonic_entries();
        // Pad the labels to a common width so the values line up as a
        // column. `entry_line` lays one row out at a time and cannot know
        // what its neighbours are called, so the padding has to be built in
        // — the Auto-DJ tab, which draws its rows all at once, does the
        // same sum for itself.
        // The status row is a sentence rather than a label, so it neither
        // pads nor sets the width — left in, it would push every value on
        // the tab off to the right of "not enough analysed songs".
        let labelled = |entry: &Entry| match entry {
            Entry::Sonic { label, row, .. } if *row != SonicRow::Status => {
                Some(label.chars().count())
            }
            _ => None,
        };
        let widest = entries.iter().filter_map(labelled).max().unwrap_or(0);
        for entry in &mut entries {
            if let Entry::Sonic { label, row, .. } = entry
                && *row != SonicRow::Status
            {
                let pad = widest - label.chars().count();
                label.push_str(&" ".repeat(pad));
            }
        }
        let selected = match entries.is_empty() {
            true => None,
            false => Some(self.sonic_pane.state.selected().unwrap_or(0).min(entries.len() - 1)),
        };
        // A filter describes the list it was typed against, and these are
        // different rows — the same rule [`Pane::set`] follows, applied by
        // hand because the cursor has to survive, which `set` would not
        // allow.
        self.sonic_pane.clear_filter();
        self.sonic_pane.entries = entries;
        self.sonic_pane.state.select(selected);
        self.sonic_pane.loading = self.sonic.pending;
    }

    fn sonic_entries(&self) -> Vec<Entry> {
        match *self.sonic_stack.here() {
            SonicNode::Side(side) => self.sonic_side_entries(side),
            SonicNode::Root => match self.sonic.view {
                SonicView::Setup => self.sonic_setup_entries(),
                SonicView::Results => self.sonic_results_entries(),
            },
        }
    }

    /// How an end reads on its row: the track, or the invitation to choose
    /// one. Never a bare filename — [`Track::display_name`] falls back to it
    /// only when the tags have nothing better.
    fn sonic_end_detail(&self, side: SonicSide) -> String {
        match self.sonic.side(side) {
            Some(track) => track.display_name(),
            None => "not set — Enter to choose".to_string(),
        }
    }

    fn sonic_setup_entries(&self) -> Vec<Entry> {
        let build = if self.sonic.ready() {
            format!("{} stops, both ends included", self.sonic.length)
        } else {
            "choose both ends first".to_string()
        };
        vec![
            Entry::Sonic {
                label: SonicSide::Start.label().to_string(),
                detail: self.sonic_end_detail(SonicSide::Start),
                row: SonicRow::End(SonicSide::Start),
            },
            Entry::Sonic {
                label: SonicSide::End.label().to_string(),
                detail: self.sonic_end_detail(SonicSide::End),
                row: SonicRow::End(SonicSide::End),
            },
            self.sonic_length_entry(),
            Entry::Sonic {
                label: "Build the path".to_string(),
                detail: build,
                row: SonicRow::Build,
            },
        ]
    }

    /// The length row, drawn as the webapp's slider is — a bar you can read
    /// the position of at a glance, with the number beside it.
    fn sonic_length_entry(&self) -> Entry {
        use crate::api::types::{JOURNEY_MAX_LENGTH, JOURNEY_MIN_LENGTH};
        let span = JOURNEY_MAX_LENGTH - JOURNEY_MIN_LENGTH;
        let filled = ((self.sonic.length - JOURNEY_MIN_LENGTH) * LENGTH_BAR).div_ceil(span);
        let bar: String = "\u{2593}".repeat(filled as usize)
            + &"\u{2591}".repeat((LENGTH_BAR - filled) as usize);
        Entry::Sonic {
            label: "Length".to_string(),
            detail: format!("{bar}  {} stops   \u{2190}\u{2192} adjust", self.sonic.length),
            row: SonicRow::Length,
        }
    }

    /// One end's little menu. The two ways in the webapp puts on a card,
    /// as rows — and a Clear that only exists once there is something to
    /// clear.
    fn sonic_side_entries(&self, side: SonicSide) -> Vec<Entry> {
        let playing = match &self.now_playing {
            Some(track) => track.display_name(),
            None => "nothing playing".to_string(),
        };
        let mut entries = vec![
            Entry::Parent,
            Entry::Sonic {
                label: "Use playing song".to_string(),
                detail: playing,
                row: SonicRow::UsePlaying,
            },
            Entry::Sonic {
                label: "Pick from library\u{2026}".to_string(),
                detail: "the next track you open lands here".to_string(),
                row: SonicRow::PickFromLibrary,
            },
        ];
        if let Some(track) = self.sonic.side(side) {
            entries.push(Entry::Sonic {
                label: "Clear".to_string(),
                detail: track.display_name(),
                row: SonicRow::Clear,
            });
        }
        entries
    }

    /// What the results view needs to say about itself beyond the stops:
    /// that it is still waiting, why it came back empty, or the server's own
    /// note about a short or short-circuited arc.
    fn sonic_status(&self) -> Option<String> {
        if self.sonic.probe {
            return Some("asking the server why\u{2026}".to_string());
        }
        if self.sonic.pending {
            return Some("plotting the path\u{2026}".to_string());
        }
        if let Some(note) = &self.sonic.note {
            return Some(note.clone());
        }
        if self.sonic.fetched && self.sonic.stops.is_empty() {
            return Some(
                "no path found — not enough analysed songs between these two".to_string(),
            );
        }
        None
    }

    fn sonic_results_entries(&self) -> Vec<Entry> {
        let has_path = !self.sonic.pending && !self.sonic.stops.is_empty();
        let mut entries = Vec::new();
        if has_path {
            let total: f64 =
                self.sonic.stops.iter().filter_map(|stop| stop.metadata.duration).sum();
            let length = if total > 0.0 {
                format!(
                    "{} stops \u{b7} {}",
                    self.sonic.stops.len(),
                    crate::tui::ui::fmt_span(total)
                )
            } else {
                format!("{} stops", self.sonic.stops.len())
            };
            entries.extend([
                Entry::Sonic {
                    // Through the glyph set, not spelled out: U+25B6 is one
                    // of the characters a console font does not have, and
                    // this row is the tab's headline.
                    label: format!("{} Play the path", crate::tui::ui::glyphs().playing),
                    detail: format!("{length} \u{b7} replaces the queue"),
                    row: SonicRow::Play,
                },
                Entry::Sonic {
                    label: "Queue all".to_string(),
                    detail: "onto the end of what's already there".to_string(),
                    row: SonicRow::QueueAll,
                },
                Entry::Sonic {
                    label: "Save as playlist\u{2026}".to_string(),
                    detail: "name it and write it to the server".to_string(),
                    row: SonicRow::SavePlaylist,
                },
            ]);
        }
        entries.push(self.sonic_length_entry());
        entries.push(Entry::Sonic {
            label: "Regenerate".to_string(),
            // Changing the length re-plots rather than trimming: a shorter
            // path goes through different places entirely.
            detail: format!("plot {} stops again", self.sonic.length),
            row: SonicRow::Regenerate,
        });
        entries.push(Entry::Sonic {
            label: "Start over".to_string(),
            detail: "back to choosing two songs".to_string(),
            row: SonicRow::StartOver,
        });

        // How the plot went, when that needs saying. A waiting tab with no
        // rows under the controls and no word about why is the one state
        // this view must never be in.
        if let Some(status) = self.sonic_status() {
            entries.push(Entry::Sonic {
                label: status,
                detail: String::new(),
                row: SonicRow::Status,
            });
        }

        let last = self.sonic.stops.len().saturating_sub(1);
        entries.extend(self.sonic.stops.iter().enumerate().map(|(index, stop)| {
            // The ends are the tracks that were chosen; everything between
            // is the server's pick for that point along the arc, and the
            // percentage is how far along it sits.
            let marker = if index == 0 || index == last {
                crate::tui::ui::glyphs().seed
            } else {
                " "
            };
            Entry::Track {
                label: format!(
                    "{marker} {:>2}. {:>3.0}%  {}",
                    index + 1,
                    stop.t * 100.0,
                    stop.metadata_display()
                ),
                track: Box::new(stop.to_track()),
            }
        }));
        entries
    }

    // ── Doing things ────────────────────────────────────────────────────────

    pub(crate) fn activate_sonic_row(&mut self, row: SonicRow) -> Vec<Effect> {
        match row {
            SonicRow::End(side) => {
                self.sonic_stack.enter(SonicNode::Side(side));
                self.sonic_pane.state.select(Some(1));
                self.refresh_sonic_rows();
                Vec::new()
            }
            // Enter walks the value the same way → does; ← walks it back.
            SonicRow::Length => self.adjust_sonic_length(1),
            SonicRow::Build | SonicRow::Regenerate => {
                if !self.sonic.ready() {
                    self.error("choose both ends first");
                    return Vec::new();
                }
                self.build_sonic_path()
            }
            SonicRow::UsePlaying => {
                let side = self.open_side();
                let Some(track) = self.now_playing.clone() else {
                    self.error("nothing is playing right now");
                    return Vec::new();
                };
                self.capture_sonic_side(side, track)
            }
            SonicRow::PickFromLibrary => {
                let side = self.open_side();
                self.arm_capture(Capture::Sonic(side))
            }
            SonicRow::Clear => {
                let side = self.open_side();
                self.sonic.set_side(side, None);
                self.leave_side(side);
                Vec::new()
            }
            SonicRow::Play => {
                if self.sonic.stops.is_empty() {
                    return Vec::new();
                }
                self.queue.replace(self.sonic.tracks());
                self.play_index(0)
            }
            SonicRow::QueueAll => {
                let tracks = self.sonic.tracks();
                if tracks.is_empty() {
                    return Vec::new();
                }
                let count = tracks.len();
                let was_empty = self.queue.items.is_empty();
                for track in tracks {
                    self.queue.push(track);
                }
                self.info(format!("queued {count} stops"));
                if was_empty && self.status.is_idle() {
                    return self.play_index(0);
                }
                Vec::new()
            }
            SonicRow::SavePlaylist => {
                if self.sonic.stops.is_empty() {
                    return Vec::new();
                }
                // Offered pre-filled with where it goes and where it ends
                // up, because that is the name people would type anyway.
                self.sonic_playlist_name = Some(self.suggested_playlist_name());
                Vec::new()
            }
            SonicRow::StartOver => {
                self.reset_sonic_path();
                self.sonic_pane.state.select(Some(0));
                Vec::new()
            }
            // A statement of how the plot went. There is nothing to press it
            // for, and inventing something would be worse than nothing.
            SonicRow::Status => Vec::new(),
        }
    }

    /// Play the whole journey from stop `index` — the GUI's click on a row.
    /// The same replace-the-queue contract as the Play row; starting from a
    /// stop is starting the journey there, not auditioning one track.
    pub(crate) fn play_sonic_from(&mut self, index: usize) -> Vec<Effect> {
        if index >= self.sonic.stops.len() {
            return Vec::new();
        }
        self.queue.replace(self.sonic.tracks());
        self.play_index(index)
    }

    /// Queue just stop `index` — the GUI's hover [+], the TUI's `a`.
    pub(crate) fn queue_sonic_stop(&mut self, index: usize) -> Vec<Effect> {
        let Some(stop) = self.sonic.stops.get(index) else {
            return Vec::new();
        };
        let track = stop.to_track();
        self.info(format!("queued {}", track.display_name()));
        let was_empty = self.queue.items.is_empty();
        self.queue.push(track);
        if was_empty && self.status.is_idle() {
            return self.play_index(0);
        }
        Vec::new()
    }

    /// Which end the open menu belongs to. `Start` is the fallback rather
    /// than an error: the rows that ask this only exist inside a side menu,
    /// so there is no reachable state where the answer is nothing.
    fn open_side(&self) -> SonicSide {
        match *self.sonic_stack.here() {
            SonicNode::Side(side) => side,
            SonicNode::Root => SonicSide::Start,
        }
    }

    /// Back out to the tab's own rows with the cursor on the end that was
    /// just settled, so the answer is under the cursor that asked for it.
    fn leave_side(&mut self, side: SonicSide) {
        self.sonic_stack.restart();
        self.refresh_sonic_rows();
        if self.sonic.view == SonicView::Setup {
            self.sonic_pane.state.select(Some(match side {
                SonicSide::Start => 0,
                SonicSide::End => 1,
            }));
        }
    }

    /// The ← route into adjustment, the same shape [`App::settings_step`]
    /// has: `Some` only when the cursor stands on the length row, so Back
    /// keeps meaning back everywhere else on the tab.
    pub(super) fn sonic_step(&mut self, delta: i32) -> Option<Vec<Effect>> {
        if self.tab != Tab::SonicPath || self.focus != Focus::Browser {
            return None;
        }
        match self.sonic_pane.selected() {
            Some(Entry::Sonic { row: SonicRow::Length, .. }) => {
                Some(self.adjust_sonic_length(delta))
            }
            _ => None,
        }
    }

    pub(crate) fn adjust_sonic_length(&mut self, delta: i32) -> Vec<Effect> {
        use crate::api::types::{JOURNEY_MAX_LENGTH, JOURNEY_MIN_LENGTH};
        let length = (self.sonic.length as i32 + delta)
            .clamp(JOURNEY_MIN_LENGTH as i32, JOURNEY_MAX_LENGTH as i32) as u32;
        if length == self.sonic.length {
            return Vec::new();
        }
        self.sonic.length = length;
        // Deliberately not a refetch: sliding the length would otherwise put
        // a request on the wire per keystroke. Build (or Regenerate, which
        // is the same row by another name) is what asks.
        //
        // But a path already on the wire was plotted for the length that has
        // just changed, and `consume_journey` drops a reply whose length is
        // not the current one — so nothing is coming, and a wait kept for it
        // left the row saying "plotting…" for the rest of the session.
        self.sonic.pending = false;
        self.refresh_sonic_rows();
        Vec::new()
    }

    // ── Choosing the ends ───────────────────────────────────────────────────

    /// Arm the picker and put the user somewhere with songs in it.
    ///
    /// Shared by both panels that ask for a song: the capture is answered
    /// from anywhere — search, albums, a playlist — so the file browser is a
    /// starting point rather than a requirement, and the arming carries who
    /// is waiting rather than each caller keeping its own flag.
    pub(crate) fn arm_capture(&mut self, who: Capture) -> Vec<Effect> {
        self.capture = Some(who);
        self.message = None;
        // The picking happens in the browser, so the browser has to be on
        // screen — arming from the full-screen view otherwise left the user
        // choosing from a listing they could not see.
        self.fullscreen = false;
        if let Capture::Sonic(_) = who {
            self.sonic_stack.restart();
            self.refresh_sonic_rows();
        }
        match self.tabs().iter().position(|tab| *tab == Tab::Files) {
            Some(index) => self.select_tab(index),
            None => Vec::new(),
        }
    }

    /// The next track chosen anywhere, handed to whoever armed the picker.
    pub(super) fn answer_capture(&mut self, who: Capture, track: Track) -> Vec<Effect> {
        self.capture = None;
        match who {
            Capture::Sonic(side) => self.capture_sonic_side(side, track),
            // Back to the tab that asked, on the two ways of looking.
            Capture::Discover => self.seed_discover(track),
        }
    }

    /// A track chosen for one end, however it was chosen.
    pub(crate) fn capture_sonic_side(&mut self, side: SonicSide, track: Track) -> Vec<Effect> {
        let label = track.display_name();
        self.sonic.set_side(side, Some(track));
        self.capture = None;
        // A path drawn between the old ends is not a path between these.
        if self.sonic.view == SonicView::Results {
            self.sonic.view = SonicView::Setup;
            self.sonic.stops.clear();
            self.sonic.fetched = false;
            self.sonic.note = None;
        }
        self.sonic_stack.restart();
        let effects = self.open_sonic_tab();
        self.leave_side(side);
        self.info(format!("{}: {label}", side.label().to_lowercase()));
        effects
    }

    // ── Building it ─────────────────────────────────────────────────────────

    pub(crate) fn build_sonic_path(&mut self) -> Vec<Effect> {
        let (Some(start), Some(end)) = (&self.sonic.start, &self.sonic.end) else {
            return Vec::new();
        };
        let (start, end) = (start.filepath.clone(), end.filepath.clone());
        self.sonic.view = SonicView::Results;
        self.sonic.pending = true;
        self.sonic.probe = false;
        self.sonic.empty = SonicEmpty::Plain;
        self.sonic.stops.clear();
        self.sonic.note = None;
        self.message = None;
        // Back at the top of the actions: the rows under the cursor a moment
        // ago were the setup's, and they are gone.
        self.sonic_pane.state.select(Some(0));
        self.refresh_sonic_rows();
        vec![Effect::Api(ApiCmd::Journey { start, end, length: self.sonic.length })]
    }

    /// The server's answer. `length` names the request it belongs to: asking
    /// for a different one while this was in flight makes it an answer
    /// nobody is waiting for any more.
    pub(super) fn consume_journey(
        &mut self,
        stops: Vec<crate::api::types::JourneyStop>,
        note: Option<String>,
        length: u32,
        issue: crate::tui::worker::JourneyIssue,
    ) -> Vec<Effect> {
        use crate::tui::worker::JourneyIssue;
        if self.sonic.length != length || self.sonic.view != SonicView::Results {
            return Vec::new();
        }
        self.sonic.pending = false;
        self.sonic.fetched = true;
        self.sonic.stops = stops;
        self.sonic.note = note;
        self.sonic.empty = match issue {
            JourneyIssue::NotAnalyzed => SonicEmpty::NotAnalyzed,
            JourneyIssue::None | JourneyIssue::Disabled => SonicEmpty::Plain,
        };
        if issue == JourneyIssue::Disabled {
            // Ask before naming a reason: the ping's flag tells "switched
            // off" from "nothing scanned yet", and one cheap GET is nothing
            // next to telling the user the wrong thing and retracting it.
            self.sonic.probe = true;
            self.refresh_sonic_rows();
            return vec![Effect::Api(ApiCmd::DiscoveryProbe)];
        }
        self.sonic_pane.state.select(Some(0));
        self.refresh_sonic_rows();
        Vec::new()
    }

    /// The probe's verdict. A flag still on means the scan simply hasn't
    /// run — worth retrying later; a flag gone means the feature was
    /// switched off under us, and the next real ping takes the entry point
    /// with it.
    pub(super) fn consume_discovery_probe(&mut self, available: bool) -> Vec<Effect> {
        if !self.sonic.probe {
            return Vec::new();
        }
        self.sonic.probe = false;
        let (kind, note) = if available {
            (
                SonicEmpty::ScanPending,
                "the server hasn't analyzed any music yet — a path needs the discovery scan to have run",
            )
        } else {
            (SonicEmpty::TurnedOff, "sonic discovery has been switched off on this server")
        };
        self.sonic.empty = kind;
        self.sonic.note = Some(note.to_string());
        self.refresh_sonic_rows();
        Vec::new()
    }

    // ── Saving it ───────────────────────────────────────────────────────────

    /// "Artist — A → Artist — B", trimmed to something a playlist name can
    /// be. Titles alone: the artists are usually the same at both ends and
    /// the name is read in a list, not studied.
    fn suggested_playlist_name(&self) -> String {
        let title = |track: Option<&Track>| -> String {
            track
                .map(|track| {
                    track.metadata.title.clone().unwrap_or_else(|| track.display_name())
                })
                .unwrap_or_default()
        };
        format!(
            "{} \u{2192} {}",
            title(self.sonic.start.as_ref()),
            title(self.sonic.end.as_ref())
        )
    }

    /// The name prompt. Returns `None` for keys it does not claim, so the
    /// rest of the player keeps working underneath it.
    pub(super) fn handle_playlist_name_action(
        &mut self,
        action: &Action,
    ) -> Option<Vec<Effect>> {
        let mut name = self.sonic_playlist_name.clone()?;
        match action {
            Action::Input(c) => name.push(*c),
            Action::Backspace => {
                name.pop();
            }
            Action::Cancel => {
                self.sonic_playlist_name = None;
                return Some(Vec::new());
            }
            Action::Submit => {
                self.sonic_playlist_name = None;
                let name = name.trim().to_string();
                if name.is_empty() {
                    return Some(Vec::new());
                }
                let files: Vec<String> =
                    self.sonic.stops.iter().map(|stop| stop.filepath.clone()).collect();
                if files.is_empty() {
                    return Some(Vec::new());
                }
                self.info(format!("saving {name}\u{2026}"));
                return Some(vec![Effect::Api(ApiCmd::SavePlaylist { name, files })]);
            }
            _ => return None,
        }
        self.sonic_playlist_name = Some(name);
        Some(Vec::new())
    }

    /// The save landed. Anyone standing on the list of playlists is looking
    /// at one that no longer has everything in it, so that view — and only
    /// that view — is asked for again.
    pub(super) fn consume_playlist_saved(&mut self, name: String, count: usize) -> Vec<Effect> {
        self.info(format!("saved {name} — {count} tracks"));
        self.refresh_playlists_view()
    }

    /// A management verb landed (create, rename, delete). No message — the
    /// row appearing, renaming or vanishing IS the confirmation (the
    /// playlists contract, clauses 12 and 31) — but an open Playlists view
    /// is looking at a list that just changed, so it re-asks.
    pub(crate) fn consume_playlist_changed(&mut self) -> Vec<Effect> {
        self.refresh_playlists_view()
    }

    fn refresh_playlists_view(&mut self) -> Vec<Effect> {
        if *self.library_node() == LibraryNode::Playlists {
            return vec![Effect::Api(ApiCmd::Library {
                node: LibraryNode::Playlists,
                dest: Tab::Library,
            })];
        }
        Vec::new()
    }
}
