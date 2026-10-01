# winit, vendored

Upstream: [winit 0.30.13](https://crates.io/crates/winit/0.30.13) from crates.io
(<https://github.com/rust-windowing/winit>). The root `Cargo.toml` swaps it in with
`[patch.crates-io]`, so the dependency line there still reads `winit = "0.30"`, and
egui-winit, which also wants winit 0.30, resolves to this same copy: one winit in the tree.

## Why it is vendored

To carry one macOS IME fix that no 0.30 release has: rust-windowing/winit
[PR #4478](https://github.com/rust-windowing/winit/pull/4478), "macOS: Fix Korean composing
text commit key handling" (open against master at head
`dfb23bff7d1c945a580673a9977f2699e0234d91` when backported, 2026-09-30). On master the code
lives in `winit-appkit/src/view.rs`; in 0.30.13 it is
`src/platform_impl/macos/view.rs`, where the same two methods carry the same logic under the
older objc2 0.5 macros (`#[method(...)]`, `unsafe { self.hasMarkedText() }`), so the PR's
logic was carried over by hand rather than by `patch`.

## What differs from 0.30.13

`src/platform_impl/macos/view.rs`, two hunks, each marked in the source with a comment naming
the PR:

1. `insertText:replacementRange:` — when it commits (marked text present, IME enabled, not a
   control character) it now clears `marked_text` first, as NSTextInputClient requires, so a
   second `insertText` inside the same `interpretKeyEvents` no longer sees stale marked text
   and commits again (Korean: Space on an empty field typed two spaces). And a new
   `else if ime_state == Committed && !is_control` branch sets `forward_key_to_app`, so the
   character an IME inserts right after a commit reaches the app as a key event instead of
   being dropped or committed twice.
2. `doCommandBySelector:` — the early `return` while `ime_state == Committed` is gone. It
   swallowed the command key that follows a commit (Korean: `한` then Enter committed but lost
   the Enter); with hunk 1 clearing the marked text, the double input it guarded against no
   longer happens.

The PR's changelog entry was not carried over; the vendored copy keeps 0.30.13's own `src/changelog/unreleased.md` untouched.
The PR's changelog line was not carried over: the vendored copy keeps 0.30.13's own `src/changelog/unreleased.md` untouched.

Packaging trimmed: `examples/` and `tests/` dropped, with their `[[example]]` and `[[test]]`
sections, every `dev-dependencies` table and the two paths in `include`; also the crate's
`Cargo.lock`, `Cargo.toml.orig`, `.cargo_vcs_info.json` and `.cargo-ok`. Kept as published:
`src/` (including `platform_impl/web/web_sys/worker.min.js` and the X11 test data
`include_str!` reads), `build.rs`, `docs/`, `FEATURES.md`, `README.md`, `LICENSE`.

## What it does and does not fix here

Checked at a real keyboard (CGEvents posted through the OS) in the GUI window's search box,
2026-09-30, against a plain AppKit `NSTextView` driven by the same keys as the control:

- Pinyin (`zhongguo`, Space): `中国` commits on Space and Enter then submits once, as before.
- Japanese Romaji: unchanged by this backport, and the same with stock 0.30.13. `nihon` then
  Enter commits `にほん` on that one Enter, and the next Enter submits. `nihon`, Space, Enter
  takes two Enters to commit because Space opens Kotoeri's candidate list (the first Space
  highlights `にほん`, a second `日本`): the first Enter only closes the list, with no
  NSTextInputClient call, and the second commits. A plain AppKit `NSTextView`
  given the same keys does exactly the same, so that is the input method's UI, not a winit
  defect, and nothing here should change it.
- The post-commit paths the PR repairs are Korean's (`한` + Enter, `한` + Space); Korean is not
  an enabled input source on the machine the check ran on, so those were not exercised here.

Drop the patch entry once a winit release on the 0.30 line, or the one egui-winit moves to,
carries PR #4478.
