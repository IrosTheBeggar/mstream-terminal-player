//! Auto-DJ and the Sonic Journey: the picks, the panel, and the trip.
//!
//! Eight of App's fields are serviced from here and nowhere else
//! (audit #57). The rule that matters lives in [`App::consume_dj`]:
//! a DJ reply in hand means its request is no longer in flight,
//! decided in one place rather than by every arm on its own clock.

use super::*;

impl App {
    /// Recent track paths, newest first — the sonic anchor.
    fn anchors(&self) -> Vec<String> {
        self.autodj_recent.iter().map(|t| t.filepath.clone()).collect()
    }

    /// Recently-played artist names, newest first and deduped, for the
    /// cooldown. Tracks with no artist tag contribute nothing rather than an
    /// empty name the server would match against everything.
    pub(super) fn recent_artists(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.autodj_recent
            .iter()
            .filter_map(|t| t.metadata.artist.as_deref())
            .filter(|a| !a.trim().is_empty())
            .filter(|a| seen.insert(a.to_ascii_lowercase()))
            .map(str::to_string)
            .collect()
    }

    /// Note a track as played, for anchoring and cooldown.
    pub(super) fn remember_played(&mut self, track: &Track) {
        self.autodj_recent.retain(|t| t.filepath != track.filepath);
        self.autodj_recent.insert(0, track.clone());
        self.autodj_recent.truncate(RECENT_MEMORY);
    }

    /// Everything the worker needs to ask for a pick.
    fn dj_request(&self) -> Box<DjRequest> {
        Box::new(DjRequest {
            mode: self.autodj,
            settings: self.dj.clone(),
            seed: self.now_playing.clone().map(Box::new),
            ignore_list: self.autodj_ignore.clone(),
            anchors: self.anchors(),
            recent_artists: self.recent_artists(),
            sonic_available: self.capabilities.discovery,
        })
    }

    // ── Sonic Journey ───────────────────────────────────────────────────────

    /// `J` on a track. What's playing is the natural place to set off from,
    /// so one press is usually enough; with nothing playing it takes two —
    /// one to mark where to start, one to say where to end up.
    pub(super) fn start_journey(&mut self) -> Vec<Effect> {
        if !self.capabilities.discovery_path {
            self.error("this server can't plot journeys — it has no discovery index");
            return Vec::new();
        }
        let Some(picked) = self.selected_track() else {
            self.error("highlight a track to travel to");
            return Vec::new();
        };

        let from = self.now_playing.clone().or_else(|| self.journey_from.take());
        let Some(from) = from else {
            self.journey_from = Some(picked.clone());
            self.info(format!(
                "journey starts at {} — now highlight where it should end and press J",
                picked.display_name()
            ));
            return Vec::new();
        };

        self.journey_from = None;
        self.open_journey(from, picked)
    }

    fn open_journey(&mut self, from: Track, to: Track) -> Vec<Effect> {
        let length = self
            .journey
            .as_ref()
            .map_or(crate::api::types::JOURNEY_DEFAULT_LENGTH, |j| j.length);
        self.journey = Some(Journey {
            from,
            to,
            length,
            stops: Vec::new(),
            pending: true,
            offset: 0,
        });
        self.message = None;
        self.fetch_journey()
    }

    fn fetch_journey(&mut self) -> Vec<Effect> {
        let Some(journey) = self.journey.as_mut() else { return Vec::new() };
        journey.pending = true;
        journey.offset = 0;
        vec![Effect::Api(ApiCmd::Journey {
            start: journey.from.filepath.clone(),
            end: journey.to.filepath.clone(),
            length: journey.length,
        })]
    }

    pub(super) fn handle_journey_action(&mut self, action: Action) -> Vec<Effect> {
        if action == Action::Quit {
            self.should_quit = true;
            return vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)];
        }
        let Some(journey) = self.journey.as_mut() else { return Vec::new() };
        match action {
            Action::Cancel | Action::Input('J') => {
                self.journey = None;
                Vec::new()
            }
            // Changing the length is a different arc, so it has to be asked
            // for again rather than trimmed locally.
            Action::SeekForward | Action::Back | Action::SeekBackward => {
                let delta: i32 = if action == Action::SeekForward { 2 } else { -2 };
                let length = (journey.length as i32 + delta).clamp(
                    crate::api::types::JOURNEY_MIN_LENGTH as i32,
                    crate::api::types::JOURNEY_MAX_LENGTH as i32,
                ) as u32;
                if length == journey.length {
                    return Vec::new();
                }
                journey.length = length;
                self.fetch_journey()
            }
            Action::Down => {
                journey.offset = (journey.offset + 1).min(journey.stops.len().saturating_sub(1));
                Vec::new()
            }
            Action::Up => {
                journey.offset = journey.offset.saturating_sub(1);
                Vec::new()
            }
            Action::First => {
                journey.offset = 0;
                Vec::new()
            }
            Action::Activate | Action::Submit => self.queue_journey(),
            _ => Vec::new(),
        }
    }

    /// Take the arc as the queue and start at its first stop.
    fn queue_journey(&mut self) -> Vec<Effect> {
        let Some(journey) = self.journey.take() else { return Vec::new() };
        if journey.stops.is_empty() {
            return Vec::new();
        }
        let count = journey.stops.len();
        let tracks: Vec<Track> = journey.stops.iter().map(|s| s.to_track()).collect();
        self.queue.replace(tracks);
        self.info(format!(
            "{count} stops from {} to {}",
            journey.from.display_name(),
            journey.to.display_name()
        ));
        self.play_index(0)
    }

    // ── Auto-DJ panel ───────────────────────────────────────────────────────

    /// Open the panel, built for what this server can do.
    pub(super) fn open_dj_panel(&mut self) -> Vec<Effect> {
        self.dj_panel = Some(DjPanel::new(self.capabilities));
        self.message = None;
        Vec::new()
    }

    /// Keys while the panel is open. Left/right adjust the highlighted row;
    /// everything else is navigation or one of the panel's own commands.
    pub(super) fn handle_dj_action(&mut self, action: Action) -> Vec<Effect> {
        if action == Action::Quit {
            self.should_quit = true;
            return vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)];
        }
        // The genre chooser sits over the panel and takes keys first.
        if self.dj_panel.as_ref().is_some_and(|p| p.genres.is_some()) {
            return self.handle_genre_action(action);
        }

        let Some(panel) = self.dj_panel.as_mut() else { return Vec::new() };
        match action {
            Action::Cancel | Action::ToggleAutoDj => {
                self.dj_panel = None;
                Vec::new()
            }
            Action::Up => {
                panel.row = panel.row.saturating_sub(1);
                Vec::new()
            }
            Action::Down => {
                panel.row = (panel.row + 1).min(panel.rows.len().saturating_sub(1));
                Vec::new()
            }
            Action::First => {
                panel.row = 0;
                Vec::new()
            }
            Action::Last => {
                panel.row = panel.rows.len().saturating_sub(1);
                Vec::new()
            }
            // `h`/`l` and the arrows both land here — `[`/`]` too, which is
            // the same left/right shape.
            Action::Back | Action::SeekBackward => self.adjust_dj_row(-1),
            Action::SeekForward => self.adjust_dj_row(1),
            Action::Activate | Action::Submit => match panel.selected() {
                // Enter on the genre row opens the chooser; elsewhere it
                // nudges the setting, matching what right-arrow does.
                DjRow::Genres => self.open_genre_picker(),
                _ => self.adjust_dj_row(1),
            },
            // `p` samples what these settings produce.
            Action::Input('p') => self.sample_dj(),
            _ => Vec::new(),
        }
    }

    /// Move the highlighted setting by one step. Numbers move in useful
    /// increments rather than by one, so a slider crosses its range in a
    /// handful of presses.
    fn adjust_dj_row(&mut self, delta: i32) -> Vec<Effect> {
        let Some(panel) = self.dj_panel.as_ref() else { return Vec::new() };
        let step = |value: u32, by: i32, max: u32| -> u32 {
            (value as i32 + by).clamp(0, max as i32) as u32
        };
        match panel.selected() {
            DjRow::Mode => {
                self.autodj = if delta > 0 {
                    self.autodj.next_available(self.capabilities)
                } else {
                    self.autodj.prev_available(self.capabilities)
                };
                if self.autodj == AutoDjMode::Off {
                    self.autodj_pending = false;
                    return Vec::new();
                }
                return self.maybe_autodj();
            }
            DjRow::Tightness => {
                self.dj.sonic_tightness = step(self.dj.sonic_tightness, delta * 5, 100);
            }
            DjRow::Anchor => self.dj.sonic_anchor = self.dj.sonic_anchor.next(),
            DjRow::Tempo => {
                self.dj.tempo_tolerance =
                    step(self.dj.tempo_tolerance, delta, dj::TEMPO_TOLERANCE_MAX);
            }
            DjRow::Key => {
                self.dj.key_matching = if delta > 0 {
                    self.dj.key_matching.next()
                } else {
                    self.dj.key_matching.next().next()
                };
            }
            DjRow::Rating => self.dj.min_rating = step(self.dj.min_rating, delta, dj::RATING_MAX),
            DjRow::Cooldown => {
                self.dj.artist_cooldown =
                    step(self.dj.artist_cooldown, delta, dj::ARTIST_COOLDOWN_MAX);
            }
            DjRow::Genres => self.dj.genre_mode = self.dj.genre_mode.next(),
        }
        Vec::new()
    }

    fn open_genre_picker(&mut self) -> Vec<Effect> {
        let Some(panel) = self.dj_panel.as_mut() else { return Vec::new() };
        panel.genres = Some(GenrePicker { loading: true, ..Default::default() });
        vec![Effect::Api(ApiCmd::Genres)]
    }

    fn handle_genre_action(&mut self, action: Action) -> Vec<Effect> {
        let Some(panel) = self.dj_panel.as_mut() else { return Vec::new() };
        let Some(picker) = panel.genres.as_mut() else { return Vec::new() };
        match action {
            Action::Cancel | Action::Activate | Action::Submit => {
                panel.genres = None;
            }
            Action::Up => picker.row = picker.row.saturating_sub(1),
            Action::Down => {
                picker.row = (picker.row + 1).min(picker.all.len().saturating_sub(1));
            }
            Action::First => picker.row = 0,
            Action::Last => picker.row = picker.all.len().saturating_sub(1),
            // Space toggles, which is why Enter closes rather than selects:
            // a chooser you leave with the same key you pick with is a
            // chooser you keep leaving by accident.
            Action::PlayPause => {
                if let Some(name) = picker.all.get(picker.row).cloned() {
                    if let Some(at) = self.dj.genres.iter().position(|g| *g == name) {
                        self.dj.genres.remove(at);
                    } else {
                        self.dj.genres.push(name);
                    }
                    // Choosing genres with the filter off is a dead end;
                    // switch it on rather than silently ignoring the choice.
                    if self.dj.genre_mode == dj::GenreMode::Off && !self.dj.genres.is_empty() {
                        self.dj.genre_mode = dj::GenreMode::Whitelist;
                    }
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn sample_dj(&mut self) -> Vec<Effect> {
        let Some(panel) = self.dj_panel.as_mut() else { return Vec::new() };
        if panel.sample_pending {
            return Vec::new();
        }
        panel.sample_pending = true;
        panel.sample.clear();
        vec![Effect::Api(ApiCmd::AutoDjSample {
            request: self.dj_request(),
            count: DJ_SAMPLE_COUNT,
        })]
    }

    /// Ask Auto-DJ for another track once the queue has nothing left after the
    /// one playing — early enough that it lands before the current track ends.
    pub(super) fn maybe_autodj(&mut self) -> Vec<Effect> {
        if self.autodj == AutoDjMode::Off || self.autodj_pending {
            return Vec::new();
        }
        let needs_more = match self.queue.current {
            Some(index) => index + 1 >= self.queue.items.len(),
            // Nothing playing: only step in if there's nothing queued either,
            // so switching it on doesn't jump a queue the user just built.
            None => self.queue.items.is_empty(),
        };
        if !needs_more {
            return Vec::new();
        }

        self.autodj_pending = true;
        vec![Effect::Api(ApiCmd::AutoDj(self.dj_request()))]
    }

    /// `A` cycles the mode, which is the whole interaction most of the time;
    /// the panel behind `D` is for the rest of it.
    pub(super) fn cycle_autodj(&mut self) -> Vec<Effect> {
        self.autodj = self.autodj.next_available(self.capabilities);
        self.info(format!("auto-dj: {}", self.autodj.label()));
        if self.autodj == AutoDjMode::Off {
            // Any reply still in flight is no longer wanted.
            self.autodj_pending = false;
            return Vec::new();
        }
        self.maybe_autodj()
    }

    /// The four DJ replies, through one door.
    ///
    /// Whatever else a reply says, the request it answers is no longer in
    /// flight — settled here, first, for all of them. Each arm used to
    /// clear its own flag on its own clock, and `autodj_pending` was being
    /// written in three unrelated places (audit #57).
    pub(super) fn consume_dj(&mut self, event: Event) -> Vec<Effect> {
        match &event {
            Event::AutoDjPick { .. } => self.autodj_pending = false,
            Event::AutoDjSample { .. } => {
                if let Some(panel) = self.dj_panel.as_mut() {
                    panel.sample_pending = false;
                }
            }
            Event::Journey { .. } => {
                if let Some(journey) = self.journey.as_mut() {
                    journey.pending = false;
                }
            }
            Event::Genres(_) => {
                if let Some(picker) =
                    self.dj_panel.as_mut().and_then(|panel| panel.genres.as_mut())
                {
                    picker.loading = false;
                }
            }
            _ => {}
        }

        match event {
            Event::AutoDjSample { tracks, pool, note } => {
                if let Some(panel) = self.dj_panel.as_mut() {
                    panel.sample = tracks;
                    // Keep the last pool size when this pick didn't report
                    // one: it still describes the settings on screen.
                    panel.pool = pool.or(panel.pool.take());
                }
                if let Some(note) = note {
                    self.info(note);
                }
                Vec::new()
            }
            Event::Journey { stops, note } => {
                // A journey the user closed while it was in flight stays
                // closed; the reply is no longer wanted.
                if let Some(journey) = self.journey.as_mut() {
                    journey.stops = stops;
                    journey.offset = 0;
                }
                if let Some(note) = note {
                    self.info(note);
                }
                Vec::new()
            }
            Event::Genres(genres) => {
                if let Some(picker) =
                    self.dj_panel.as_mut().and_then(|panel| panel.genres.as_mut())
                {
                    picker.all = genres.into_iter().map(|g| g.name).collect();
                    picker.row = picker.row.min(picker.all.len().saturating_sub(1));
                }
                Vec::new()
            }
            Event::AutoDjPick { candidates, ignore_list, note } => {
                self.autodj_ignore = ignore_list;
                let explained = note.is_some();
                if let Some(note) = note {
                    self.info(note);
                }
                if self.autodj == AutoDjMode::Off {
                    return Vec::new(); // switched off while the request was out
                }

                let queued: std::collections::HashSet<String> =
                    self.queue.items.iter().map(|t| t.filepath.clone()).collect();
                let Some(pick) = candidates.into_iter().find(|t| !queued.contains(&t.filepath))
                else {
                    if !explained {
                        self.info("auto-dj: nothing new to add");
                    }
                    return Vec::new();
                };

                let label = pick.display_name();
                // If the queue already ran dry, this pick should start playing
                // rather than sit there.
                let start_it = self.queue.current.is_none() && self.status.is_idle();
                self.queue.push(pick);
                if !explained {
                    self.info(format!("auto-dj: {label}"));
                }
                if start_it {
                    return self.play_index(self.queue.items.len() - 1);
                }
                Vec::new()
            }
            // The routing arm in `apply_event` sends exactly the four above.
            _ => Vec::new(),
        }
    }
}
