# stream-download, vendored

> This copy ships in releases of both products; this file is the record of what differs.
> Filing the changes upstream is a separate task.

Upstream: [stream-download 0.24.2](https://crates.io/crates/stream-download/0.24.2) from
crates.io (<https://github.com/aschey/stream-download-rs>), with its licenses as published. The
root `Cargo.toml` swaps it in with `[patch.crates-io]`, so the dependency line there still reads
`stream-download = { version = "0.24.2", ... }`.

## Why it is vendored

Its range bookkeeping loses a waiting reader (PLAN.md, performance audit #73). The engine
streams every remote track through it, and on a slow link the losses were stalls of tens of
seconds after a seek:

- a Range response shorter than the 256 KiB prefetch never woke the reader parked on it (a 1 s
  nudge back stalled 14 s at 8 Mbit/s, a seek into a file's last 154 KB 29 s, a 90% seek in a
  FLAC without a SEEKTABLE 112 s at 2 Mbit/s);
- the loader then refilled the file's lowest gap rather than the one ahead of the reader;
- a seek past an already-downloaded island asked the server for an inverted range (a 416 and a
  dead track);
- a read that ran off the end of an island never asked for what came next;
- a chunk landing above a reader that had sought back into a gap woke it onto bytes never
  written;
- a seek made while the last one still sat unread in the one-slot seek queue was dropped. The
  download can reach a seek's position by itself before it reads the seek (a probe's seek to
  0 while the prefetch lands), so the reader's next seek parked it until the download
  happened by (a seek into a file's last stretch waited out a stalled first request);
- a read that found too few bytes named the position it would wait for only after looking, so
  when the rest landed in between, the download found no position named and woke nobody: the
  reader slept through its own bytes until the download ended.

Neither 0.24.3, 0.24.4 nor upstream main (checked 2026-09-28) changes those files.

## What differs from 0.24.2

Every change is marked in the source with a comment beginning `mstream-player patch`:

- `src/source/handle.rs`: `SourceHandle::want`, which a read about to wait calls with the
  position where its bytes run out. It is a `tokio::sync::watch`, so a want can be replaced by
  a newer one but never dropped. `request_seek` sends on a `watch` too, in place of the
  one-slot `mpsc`: the latest seek replaces one the download has not looked at yet. A wait
  (`wait_until_reached`) checks the downloaded runs for its position itself, under the lock
  the download's wake takes, and the wake names the position it is for, so a late one wakes
  no later wait.
- `src/source/mod.rs`: the download takes the latest seek from its `watch`; a prefetch a seek
  ends early wakes a reader parked in what it wrote; it wakes on a want and asks for the gap
  at that position; it
  records where the reader last sought; a seek's gap that holds the seek position comes first;
  a reader parked on `requested_position` is woken once the run of written bytes reaches it,
  whatever path wrote them (the prefetch included); a refill goes forward from the reader's
  last seek before wrapping to the file's start.
- `src/lib.rs`: the read path calls `want` before it parks.

## When to drop it

Once an upstream release carries equivalent fixes. The range tests in `src/engine/http.rs`
(throttled Range server, nudges, seeks into an island's end) say whether it did: point the
patch away and run them.
