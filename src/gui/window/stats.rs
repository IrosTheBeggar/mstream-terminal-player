//! `MSTREAM_WINDOW_STATS=<path>`: a spike lever, hidden like the others,
//! that writes what the window's frames cost as JSON when the window
//! closes. It is how the spike weighs the window against the terminal
//! without a profiler: how long the window took to put its first frame on
//! the GPU, how many frames it drew and how many of those changed no cell,
//! and the spread of three timings — the backend's flush (where
//! ratatui-wgpu shapes the dirty rows, encodes and presents), the loop's
//! whole `frame` half (the tick, the render into the buffer and that
//! flush), and the whole redraw the window handles (the frame plus the
//! script's step and any dump).
//!
//! The counting lives in [`Counted`], a pass-through backend the window
//! always draws through: ratatui hands a backend only the cells that
//! differ from the last frame, so the cells it is handed say whether a
//! frame changed anything, which nothing else outside ratatui-wgpu can.
//! It costs a counter per changed cell. The timings cost clock reads, so
//! without the lever there are none: no flush is timed and no redraw is
//! clocked (`Counted::new` is told, and the window holds no [`Stats`]).

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

/// A backend that says how many cells each draw handed it and, when it is
/// timing, how long its last flush took; everything else goes straight
/// through. Deref reaches the wrapped backend's own methods (`get_text`,
/// `resize`).
pub(super) struct Counted<B> {
    inner: B,
    /// Cells handed to `draw` since [`Counted::take_cells`] last asked.
    cells: u64,
    /// Whether flushes are timed: only while the stats lever is set.
    timed: bool,
    /// The last flush's time; zero when not timing.
    flushed: Duration,
}

impl<B> Counted<B> {
    pub(super) fn new(inner: B, timed: bool) -> Self {
        Self { inner, cells: 0, timed, flushed: Duration::ZERO }
    }

    /// The cells drawn since the last call, and the last flush's time
    /// (zero when not timing).
    pub(super) fn take_cells(&mut self) -> (u64, Duration) {
        (std::mem::take(&mut self.cells), self.flushed)
    }
}

impl<B> Deref for Counted<B> {
    type Target = B;
    fn deref(&self) -> &B {
        &self.inner
    }
}

impl<B> DerefMut for Counted<B> {
    fn deref_mut(&mut self) -> &mut B {
        &mut self.inner
    }
}

impl<B: Backend> Backend for Counted<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let cells = &mut self.cells;
        self.inner.draw(content.inspect(|_| *cells += 1))
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        if !self.timed {
            return self.inner.flush();
        }
        let started = Instant::now();
        let flushed = self.inner.flush();
        self.flushed = started.elapsed();
        flushed
    }
}

/// The timings, kept while the lever is set, and where they go at exit.
pub(super) struct Stats {
    path: PathBuf,
    /// `window::run`'s entry: the clock every "since" here is read on.
    started: Instant,
    /// The first frame that handed the backend any cell, which is the
    /// first that drew to the GPU and asked for a present.
    first_present: Option<Duration>,
    /// The first such frame after the window came into view: on macOS the
    /// very first present finds the window occluded and is dropped, so
    /// this is when the player could first be seen.
    first_visible: Option<Duration>,
    /// The window has said it is in view, and no present since.
    visible_pending: bool,
    frames: u64,
    unchanged: u64,
    cells: u64,
    flush_ms: Vec<f64>,
    /// Flushes of frames that changed something: the ones that encode and
    /// present, where the GPU's cost shows.
    present_flush_ms: Vec<f64>,
    frame_ms: Vec<f64>,
    redraw_ms: Vec<f64>,
}

impl Stats {
    /// The lever's stats when `MSTREAM_WINDOW_STATS` names a file, their
    /// clock started now (`window::run`'s entry); `None`, and no clock
    /// read, without it.
    pub(super) fn from_env() -> Option<Self> {
        let path = PathBuf::from(std::env::var_os("MSTREAM_WINDOW_STATS")?);
        Some(Self {
            path,
            started: Instant::now(),
            first_present: None,
            first_visible: None,
            visible_pending: false,
            frames: 0,
            unchanged: 0,
            cells: 0,
            flush_ms: Vec::new(),
            present_flush_ms: Vec::new(),
            frame_ms: Vec::new(),
            redraw_ms: Vec::new(),
        })
    }

    /// The window came into view: the next present is the first seen.
    pub(super) fn visible(&mut self) {
        if self.first_visible.is_none() {
            self.visible_pending = true;
        }
    }

    /// One frame: the cells it drew, its flush, the frame half's time.
    pub(super) fn frame(&mut self, cells: u64, flush: Duration, frame: Duration) {
        self.frames += 1;
        self.cells += cells;
        self.flush_ms.push(ms(flush));
        self.frame_ms.push(ms(frame));
        if cells == 0 {
            self.unchanged += 1;
            return;
        }
        self.present_flush_ms.push(ms(flush));
        let since = self.started.elapsed();
        self.first_present.get_or_insert(since);
        if self.visible_pending {
            self.visible_pending = false;
            self.first_visible = Some(since);
        }
    }

    /// The whole redraw the frame was part of.
    pub(super) fn redraw(&mut self, took: Duration) {
        self.redraw_ms.push(ms(took));
    }

    /// The report, to the lever's file; said on stderr either way.
    /// `covers` is the cover post-processor's own count: how many frames
    /// it composited, which with covers on screen is not only the frames
    /// that changed a cell.
    pub(super) fn write(&self, covers: Option<serde_json::Value>) {
        let wall = self.started.elapsed();
        let report = serde_json::json!({
            "wall_ms": ms(wall),
            "first_present_ms": self.first_present.map(ms),
            "first_visible_present_ms": self.first_visible.map(ms),
            "frames": self.frames,
            "frames_unchanged": self.unchanged,
            "frames_per_s": self.frames as f64 / wall.as_secs_f64().max(f64::EPSILON),
            "cells_drawn": self.cells,
            "frame_ms": spread(&self.frame_ms),
            "flush_ms": spread(&self.flush_ms),
            "present_flush_ms": spread(&self.present_flush_ms),
            "redraw_ms": spread(&self.redraw_ms),
            "covers": covers,
        });
        let text = serde_json::to_string_pretty(&report).unwrap_or_default() + "\n";
        match std::fs::write(&self.path, text) {
            Ok(()) => eprintln!("gui --window: stats to {}", self.path.display()),
            Err(e) => eprintln!("gui --window: stats to {}: {e}", self.path.display()),
        }
    }
}

fn ms(time: Duration) -> f64 {
    time.as_secs_f64() * 1000.0
}

/// p50, p95, max and mean of a set of timings, nearest-rank; nulls for
/// none.
fn spread(times: &[f64]) -> serde_json::Value {
    if times.is_empty() {
        return serde_json::json!({ "n": 0, "p50": null, "p95": null, "max": null, "mean": null });
    }
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let rank = |p: f64| sorted[((p * n as f64).ceil() as usize).clamp(1, n) - 1];
    serde_json::json!({
        "n": n,
        "p50": rank(0.50),
        "p95": rank(0.95),
        "max": sorted[n - 1],
        "mean": sorted.iter().sum::<f64>() / n as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_is_nearest_rank() {
        let times: Vec<f64> = (1..=100).map(f64::from).collect();
        let got = spread(&times);
        assert_eq!(got["p50"], 50.0);
        assert_eq!(got["p95"], 95.0);
        assert_eq!(got["max"], 100.0);
        assert_eq!(spread(&[7.0])["p95"], 7.0);
        assert_eq!(spread(&[])["n"], 0);
    }

    /// Only the cells ratatui hands over are counted, so a second draw of
    /// the same screen counts none.
    #[test]
    fn a_frame_that_changes_nothing_counts_no_cells() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::widgets::Paragraph;
        let mut terminal = Terminal::new(Counted::new(TestBackend::new(10, 2), false)).unwrap();
        let draw = |terminal: &mut Terminal<Counted<TestBackend>>, text: &str| {
            terminal.draw(|frame| frame.render_widget(Paragraph::new(text), frame.area())).unwrap();
            terminal.backend_mut().take_cells().0
        };
        assert!(draw(&mut terminal, "hello") > 0);
        assert_eq!(draw(&mut terminal, "hello"), 0);
        assert_eq!(draw(&mut terminal, "hellp"), 1);
        // Without the lever no flush is timed.
        assert_eq!(terminal.backend_mut().take_cells().1, Duration::ZERO);
    }
}
