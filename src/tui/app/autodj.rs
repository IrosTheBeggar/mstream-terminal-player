//! Auto-DJ: the picks, and the tab that shapes them.
//!
//! Most of App's DJ fields are serviced from here and nowhere else
//! (audit #57). The rule that matters lives in [`App::consume_dj`]:
//! a DJ reply in hand means its request is no longer in flight,
//! decided in one place rather than by every arm on its own clock.
//!
//! The settings used to be a modal behind `D`, reachable from the browser
//! screen and invisible over the full-screen one. They are now the Auto-DJ
//! tab itself, which is the only screen that ever described them — and the
//! Sonic Path, which shared this file when it was an overlay, has moved to
//! its own tab in [`super::sonic`].

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

    // ── The Auto-DJ tab ─────────────────────────────────────────────────────

    /// Whether the Auto-DJ tab is the one in front of you — and so whether
    /// it gets the arrows and Enter. Its rows are values, so ←→ adjust them
    /// there and Tab / Shift+Tab do the tab switching instead.
    pub(super) fn on_dj_tab(&self) -> bool {
        self.fullscreen && self.now_tab() == NowTab::AutoDj
    }

    /// Enter on the highlighted row. The genre row opens the chooser and the
    /// sample row asks for one; everything else nudges its value, matching
    /// what the right arrow does.
    pub(super) fn activate_dj_row(&mut self) -> Vec<Effect> {
        match self.dj_panel.selected() {
            DjRow::Genres => self.open_genre_picker(),
            DjRow::Sample => self.sample_dj(),
            _ => self.adjust_dj_row(1),
        }
    }

    /// Move the highlighted setting by one step. Numbers move in useful
    /// increments rather than by one, so a slider crosses its range in a
    /// handful of presses.
    pub(super) fn adjust_dj_row(&mut self, delta: i32) -> Vec<Effect> {
        let panel = &self.dj_panel;
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
            // Not a value: ←→ have nothing to move here, and Enter is what
            // asks. Left alone rather than made to do something arbitrary.
            DjRow::Sample => {}
        }
        Vec::new()
    }

    fn open_genre_picker(&mut self) -> Vec<Effect> {
        self.dj_panel.genres = Some(GenrePicker { loading: true, ..Default::default() });
        vec![Effect::Api(ApiCmd::Genres)]
    }

    pub(super) fn handle_genre_action(&mut self, action: Action) -> Vec<Effect> {
        if action == Action::Quit {
            self.should_quit = true;
            return vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)];
        }
        let panel = &mut self.dj_panel;
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
        if self.dj_panel.sample_pending {
            return Vec::new();
        }
        self.dj_panel.sample_pending = true;
        self.dj_panel.sample.clear();
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
    /// the Auto-DJ tab of the full-screen view is for the rest of it.
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
            Event::AutoDjSample { .. } => self.dj_panel.sample_pending = false,
            Event::Genres(_) => {
                if let Some(picker) = self.dj_panel.genres.as_mut() {
                    picker.loading = false;
                }
            }
            _ => {}
        }

        match event {
            Event::AutoDjSample { tracks, pool, note } => {
                self.dj_panel.sample = tracks;
                // Keep the last pool size when this pick didn't report one:
                // it still describes the settings on screen.
                self.dj_panel.pool = pool.or(self.dj_panel.pool.take());
                if let Some(note) = note {
                    self.info(note);
                }
                Vec::new()
            }
            // The Sonic Path tab owns this one; it lands here only because
            // the four discovery replies come through one door.
            Event::Journey { stops, note, length, issue } => {
                self.consume_journey(stops, note, length, issue)
            }
            Event::Genres(genres) => {
                if let Some(picker) = self.dj_panel.genres.as_mut() {
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
