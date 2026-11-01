//! Presets compiled ahead, on a thread of their own (the performance
//! audit's #87). A preset's first sight used to compile its pipelines on the
//! window's thread — naga's front end, validator and writer, then the
//! driver's compiler, 26–292 ms a preset on an M3 Pro with a cold shader
//! cache — and the picture and the controls stood still for it. Now a
//! worker starts as soon as the device exists: the preset the window opens
//! on first, so its compile runs beside the window's own setup, then its
//! neighbours either way, then the rest in the dropdown's order.
//!
//! The window still knows whether this GPU draws a preset at the moment it
//! asks (contract clauses 7, 11): one the worker has in hand is waited for,
//! no longer than what is left of its compile; one it has not reached is
//! compiled where it is asked for, as before; and so is everything, if the
//! worker is gone.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use crate::shader::library::BUILTIN;
use crate::shader::preset::Preset;
use crate::shader::render::{Gpu, Scene};

/// A preset compiled, or the reason this GPU will not draw it.
pub type Compiled = Result<Scene, String>;

/// Built-in preset `i`, compiled for `gpu` — the same work on either thread.
pub fn compile(gpu: &Gpu, i: usize) -> Compiled {
    Preset::parse(BUILTIN[i].source).map_err(|e| e.to_string()).and_then(|preset| gpu.compile(&preset))
}

/// The order the worker compiles in: the preset the window opens on, the
/// ones `←` and `→` reach from it, then the rest as the dropdown lists them.
pub fn order(first: usize, count: usize) -> Vec<usize> {
    let mut order = vec![first];
    let neighbours = [(first + 1) % count, (first + count - 1) % count];
    for i in neighbours.into_iter().chain(0..count) {
        if !order.contains(&i) {
            order.push(i);
        }
    }
    order
}

pub struct Prefetch {
    /// Per preset, whether someone has taken it — the worker, or the window
    /// compiling it itself. Whoever takes it first compiles it, once.
    taken: Arc<[AtomicBool]>,
    compiled: Receiver<(usize, Compiled)>,
    /// What arrived while the window waited for another.
    early: Vec<(usize, Compiled)>,
}

impl Prefetch {
    /// The worker, compiling for `gpu` in `order`. `None` when no thread
    /// would start: the window then compiles every preset itself.
    pub fn start(gpu: Arc<Gpu>, order: Vec<usize>) -> Option<Prefetch> {
        Prefetch::spawn(order, move |i| compile(&gpu, i))
    }

    fn spawn(order: Vec<usize>, job: impl Fn(usize) -> Compiled + Send + 'static) -> Option<Prefetch> {
        let taken: Arc<[AtomicBool]> = (0..BUILTIN.len()).map(|_| AtomicBool::new(false)).collect();
        let (results, compiled) = mpsc::channel();
        let theirs = taken.clone();
        std::thread::Builder::new()
            .name("viz-window compile".into())
            // naga's front end recurses, and wgpu warns that making a
            // shader module can take a lot of stack: a thread gets 2 MiB
            // unless it asks, where the window's own has 8.
            .stack_size(16 << 20)
            .spawn(move || {
                for i in order {
                    if theirs[i].swap(true, Ordering::AcqRel) {
                        continue;
                    }
                    // A panic anywhere in the compile costs this preset, as
                    // it would on the window's thread — not the worker, and
                    // not a window waiting on it.
                    let result = catch_unwind(AssertUnwindSafe(|| job(i)))
                        .unwrap_or_else(|_| Err("the shader compiler panicked".into()));
                    if results.send((i, result)).is_err() {
                        return;
                    }
                }
            })
            .ok()?;
        Some(Prefetch { taken, compiled, early: Vec::new() })
    }

    /// What the worker has finished since the last look, without waiting.
    pub fn done(&mut self) -> Vec<(usize, Compiled)> {
        let mut done = std::mem::take(&mut self.early);
        done.extend(self.compiled.try_iter());
        done
    }

    /// Preset `i`, wanted now. The worker's result when it has taken it —
    /// waited for if it is still compiling — or `None` for the window to
    /// compile it itself: the worker has not reached it (and now never
    /// will), or the worker is gone.
    pub fn take(&mut self, i: usize) -> Option<Compiled> {
        if let Some(at) = self.early.iter().position(|(j, _)| *j == i) {
            return Some(self.early.remove(at).1);
        }
        if !self.taken[i].swap(true, Ordering::AcqRel) {
            return None;
        }
        loop {
            match self.compiled.recv() {
                Ok((j, result)) if j == i => return Some(result),
                Ok(other) => self.early.push(other),
                Err(_) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    /// A worker whose "compile" names the preset it was and is counted. It
    /// says when it starts one, holds preset 1 until `go` hears, and falls
    /// over on preset 2.
    fn worker(order: Vec<usize>) -> (Prefetch, Receiver<usize>, Sender<()>, Arc<Mutex<Vec<usize>>>) {
        let counts = Arc::new(Mutex::new(vec![0; BUILTIN.len()]));
        let theirs = counts.clone();
        let (started, starts) = mpsc::channel();
        let (go, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let prefetch = Prefetch::spawn(order, move |i| {
            started.send(i).unwrap();
            if i == 1 {
                gate.lock().unwrap().recv().unwrap();
            }
            std::thread::sleep(Duration::from_millis(5));
            theirs.lock().unwrap()[i] += 1;
            if i == 2 {
                panic!("a compiler that falls over");
            }
            Err(format!("worker {i}"))
        })
        .expect("a thread");
        (prefetch, starts, go, counts)
    }

    fn named(result: Option<Compiled>) -> Option<String> {
        result.map(|r| r.err().expect("the test's compiles all say who they were"))
    }

    #[test]
    fn the_opening_preset_first_then_its_neighbours_then_the_rest_in_order() {
        assert_eq!(order(0, 8), [0, 1, 7, 2, 3, 4, 5, 6]);
        assert_eq!(order(3, 8), [3, 4, 2, 0, 1, 5, 6, 7]);
        assert_eq!(order(7, 8), [7, 0, 6, 1, 2, 3, 4, 5]);
        assert_eq!(order(1, 2), [1, 0]);
        assert_eq!(order(0, 1), [0]);
    }

    #[test]
    fn a_preset_is_compiled_once_by_whoever_takes_it_first() {
        let (mut prefetch, starts, go, counts) = worker(order(0, 8));
        // The worker is on 0: asking for it waits for its result.
        assert_eq!(starts.recv().unwrap(), 0);
        assert_eq!(named(prefetch.take(0)), Some("worker 0".into()));
        // It is held on 1, and 6 is its last: the window takes 6 and
        // compiles it itself, and the worker will pass it by.
        assert_eq!(starts.recv().unwrap(), 1);
        assert!(prefetch.take(6).is_none(), "not reached: the window's to compile");
        go.send(()).unwrap();
        // Once it is on 5, waiting for 5 collects 7, 2, 3 and 4 on the way.
        assert_eq!(starts.iter().take(5).collect::<Vec<_>>(), [7, 2, 3, 4, 5]);
        assert_eq!(named(prefetch.take(5)), Some("worker 5".into()));
        // A panic in a compile is that preset refused, and the worker goes on.
        assert_eq!(named(prefetch.take(2)), Some("the shader compiler panicked".into()));
        let mut seen: Vec<usize> = prefetch.done().into_iter().map(|(i, _)| i).collect();
        seen.sort();
        assert_eq!(seen, [1, 3, 4, 7], "everything else, each once");
        std::thread::sleep(Duration::from_millis(50));
        assert!(prefetch.done().is_empty(), "and nothing more");
        assert_eq!(*counts.lock().unwrap(), [1, 1, 1, 1, 1, 1, 0, 1], "6 was the window's");
    }

    #[test]
    fn with_the_worker_gone_the_window_compiles_it_itself() {
        let (results, compiled) = mpsc::channel::<(usize, Compiled)>();
        drop(results);
        let taken: Arc<[AtomicBool]> = (0..BUILTIN.len()).map(|_| AtomicBool::new(true)).collect();
        let mut prefetch = Prefetch { taken, compiled, early: Vec::new() };
        assert!(prefetch.take(3).is_none());
        assert!(prefetch.done().is_empty());
    }
}
