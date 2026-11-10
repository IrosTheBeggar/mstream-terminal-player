use std::fmt::Debug;
use std::ops::Range;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Instant;

use parking_lot::{Condvar, Mutex, RwLock};
use rangemap::RangeSet;
use tokio::sync::{Notify, watch};
use tracing::debug;

#[derive(Debug, Clone)]
pub(crate) struct SourceHandle {
    pub(super) downloaded: Downloaded,
    pub(super) download_status: DownloadStatus,
    pub(super) requested_position: RequestedPosition,
    pub(super) position_reached: PositionReached,
    pub(super) content_length: Option<u64>,
    // mstream-player patch: see `request_seek`.
    pub(super) seek_tx: watch::Sender<u64>,
    // mstream-player patch: see `want`.
    pub(super) want_tx: watch::Sender<u64>,
    pub(super) notify_read: NotifyRead,
}

impl SourceHandle {
    pub(crate) fn get_downloaded_at_position(&self, position: u64) -> Option<Range<u64>> {
        self.downloaded.get(position)
    }

    pub(crate) fn wait_for_position(&self, requested_position: u64) {
        self.request_position(requested_position);
        debug!(
            requested_position = requested_position,
            "waiting for requested position"
        );
        self.notify_read.notify_waiting();
        self.wait_until_reached(requested_position);
    }

    pub(crate) fn seek(&self, seek_position: u64) {
        self.request_position(seek_position);
        self.request_seek(seek_position);
        debug!(
            requested_position = seek_position,
            "waiting for requested position"
        );
        self.wait_until_reached(seek_position);
    }

    // mstream-player patch: the wait looks at the downloaded runs itself, as well as for the
    // download's word. A read finds too few bytes downloaded and only then names the position
    // it will wait for; a download writing the rest in between found no position named and
    // told nobody, and the reader slept through its own bytes until the download ended or
    // came by again (a seek into a file's last stretch waited out the stalled back-fill of
    // everything below it). The download's word now names the position it is for, so one
    // that lands after its reader has moved on wakes no later wait.
    fn wait_until_reached(&self, position: u64) {
        let downloaded = &self.downloaded;
        self.position_reached
            .wait_for_position_reached(position, || downloaded.reaches(position));
    }

    // mstream-player patch: tells the download that a read is about to wait at `position`,
    // where the reader's bytes run out. A seek re-targets the download; a read never did, so
    // a reader that played (or sought within the spool) past the end of an island waited
    // there until the download happened by, after everything else. The download decides
    // whether it is already on its way.
    //
    // Only the latest want counts, so it replaces one the download has not looked at yet. On
    // a one-slot queue it was dropped instead, and the one ahead of it was stale by then: the
    // write that woke its reader had landed while it waited. The download found those bytes
    // already there and did nothing, and the reader, off the end of another island, parked
    // until the back-fill came round (performance audit #73 follow-up).
    pub(crate) fn want(&self, position: u64) {
        self.want_tx.send_replace(position);
    }

    pub(crate) fn notify_read(&self) {
        self.notify_read.notify_read();
    }

    pub(crate) fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    pub(crate) fn is_failed(&self) -> bool {
        self.download_status.is_failed()
    }

    fn request_position(&self, position: u64) {
        self.requested_position.set(position);
    }

    // mstream-player patch: only the latest seek counts, as with `want`. A seek waits for its
    // position, but the download can reach that position by itself before it looks at the
    // seek (a probe's seek to 0 while the prefetch lands), and the reader, woken, seeks on. On
    // the one-slot queue the stale seek still held the slot and the new one was dropped
    // ("Sent multiple seek requests without waiting"), so the reader parked until the
    // download happened by: a seek into the file's last stretch waited out the whole first
    // request.
    fn request_seek(&self, position: u64) {
        self.seek_tx.send_replace(position);
    }
}

#[derive(Debug, Clone)]
pub(super) struct RequestedPosition(Arc<AtomicI64>);

impl Default for RequestedPosition {
    fn default() -> Self {
        Self(Arc::new(AtomicI64::new(-1)))
    }
}

// relaxed ordering as we are not using the atomic to
// lock something or to synchronize threads.
impl RequestedPosition {
    pub(super) fn clear(&self) {
        self.0.store(-1, Ordering::Relaxed);
    }

    pub(super) fn get(&self) -> Option<u64> {
        let val = self.0.load(Ordering::Relaxed);
        if val == -1 { None } else { Some(val as u64) }
    }

    fn set(&self, position: u64) {
        self.0.store(position as i64, Ordering::Relaxed);
    }
}

#[derive(Default, Debug)]
struct Waiter {
    // mstream-player patch: the position the download last said was reached, in place of a
    // bare flag (`SourceHandle::wait_until_reached`).
    position_reached: Option<u64>,
    stream_done: bool,
}

// parking_lot's synchronization primitive's aren't unwind safe: https://github.com/Amanieu/parking_lot/issues/32
// Libraries that use FFI sometimes require these traits, which can cause compilation failures for
// anything that tries to hold a reference to this struct.
// Here we can use AssertUnwindSafe as long as we ensure we don't panic while holding a lock.
#[derive(Default, Clone, Debug)]
pub(super) struct PositionReached(Arc<(AssertUnwindSafe<Mutex<Waiter>>, Condvar)>);

impl PositionReached {
    pub(super) fn notify_position_reached(&self, position: u64) {
        let (mutex, cvar) = self.0.as_ref();
        mutex.lock().position_reached = Some(position);
        cvar.notify_all();
    }

    pub(super) fn notify_stream_done(&self) {
        let (mutex, cvar) = self.0.as_ref();
        mutex.lock().stream_done = true;
        cvar.notify_all();
    }

    // mstream-player patch: see `SourceHandle::wait_until_reached`. `reached` is checked under
    // the lock, before the first sleep and after every wake, and a write's word takes the same
    // lock, so a word can't slip in between the check and the sleep.
    fn wait_for_position_reached(&self, position: u64, reached: impl Fn() -> bool) {
        let (mutex, cvar) = self.0.as_ref();
        let mut waiter = mutex.lock();
        if waiter.stream_done {
            return;
        }

        let wait_start = Instant::now();

        cvar.wait_while(&mut waiter, |waiter| {
            !waiter.stream_done && waiter.position_reached != Some(position) && !reached()
        });
        debug!(
            elapsed = format!("{:?}", wait_start.elapsed()),
            "position reached"
        );
        if !waiter.stream_done {
            waiter.position_reached = None;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct Downloaded(Arc<AssertUnwindSafe<RwLock<RangeSet<u64>>>>);

impl Downloaded {
    pub(super) fn add(&self, range: Range<u64>) {
        // explicit check here prevents panic
        if range.end > range.start {
            self.0.write().insert(range);
        }
    }

    pub(super) fn get(&self, position: u64) -> Option<Range<u64>> {
        self.0.read().get(&position).cloned()
    }

    // mstream-player patch: whether a run of downloaded bytes holds `position`, its end
    // included, which is what `Source::notify_if_reached` wakes a reader for.
    pub(super) fn reaches(&self, position: u64) -> bool {
        let runs = self.0.read();
        runs.contains(&position) || position.checked_sub(1).is_some_and(|last| runs.contains(&last))
    }

    pub(super) fn next_gap(&self, range: Range<u64>) -> Option<Range<u64>> {
        self.0.read().gaps(&range).next()
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct NotifyRead {
    notify: Arc<Notify>,
    write_requested: Arc<AtomicBool>,
}

impl NotifyRead {
    pub(super) fn request(&self) {
        self.write_requested.store(true, Ordering::SeqCst);
    }

    fn notify_read(&self) {
        if self.write_requested.swap(false, Ordering::SeqCst) {
            self.notify.notify_one();
        }
    }

    fn notify_waiting(&self) {
        if self.write_requested.load(Ordering::SeqCst) {
            self.notify.notify_one();
        }
    }

    pub(super) async fn wait_for_read(&self) {
        self.notify.notified().await;
    }
}

#[derive(Default, Clone, Debug)]
pub(super) struct DownloadStatus(Arc<AtomicBool>);

impl DownloadStatus {
    pub(super) fn set_failed(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn is_failed(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
