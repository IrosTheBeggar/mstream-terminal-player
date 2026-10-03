# The window spike — evidence

> **Status (2026-10-02):** the spike became the desktop product; its record is PLAN.md's Phase 14,
> where the Shipping paragraph says what remains. The evidence below stays as the spike left it.

What PLAN.md's Phase 14 cites, kept beside it because the session scratchpad that produced it
does not persist. Screenshots are the real window on the dev Mac (Retina, scale 2), downscaled
to 1000 px wide; the text under each name says which step and scenario produced it.
The window is in `--features desktop` builds only (`cargo build --features desktop`).

- `1-render-ja.png` — step 1: the demo Library screen in Japanese, Hack plus Hiragino, no tofu.
- `2-live-files.png` — step 2: the real workers in the window, the Files room listing
  demo.mstream.io's library.
- `3-input-mouse.png` — step 3: the end of the mouse scenario driven through the script lever
  (hover verbs, [+] queueing, wheel, a right-click sheet, a queue-row drag).
- `5-art-wall.png` — step 5: the Albums wall with covers drawn as GPU textures.
- `5-art-sheet-over-wall.png` — step 5: a sheet open over the wall and the queue; the covers
  under it fall back to the ▀-mosaic and the sheet reads.
- `6-keys-german-option-chords.png` — the keyboard check (2026-09-30, real key events posted
  through the OS with Accessibility granted): on the German layout, Option+l, Option+e and
  the y key typed `@€z` into the search box.
- `6-keys-repeat-backspace.png` — the same check: `abcdefgh` typed, then Backspace held for
  one press and five auto-repeats, leaving `ab`.
- `6-keys-pinyin-commit.png` and `6-keys-pinyin-commit-dump.txt` — the Pinyin IME in the
  search box after `zhongguo` and Space: the dump holds `中国`; the screenshot shows the known
  gap, an en locale has no CJK face so the two glyphs draw as bars.
- `6-keys-japanese-preedit.log` — the window's stderr for the Japanese probe: the composition
  `n に にh にほ にほn にほん`, then no commit on the first Enter or a Right arrow, then `preedit ""`
  on Esc. Not a winit defect, as first thought: Space had opened Kotoeri's candidate list, and
  the first Enter only closes it. A plain `NSTextView` does the same; without the Space one
  Enter commits (vendor/winit/VENDORED.md).
- `stats-*.json` — step 4's `MSTREAM_WINDOW_STATS` reports, release build, ~25 s each against
  demo.mstream.io: idle, the Albums wall, playback on the Library screen, Now Playing, and
  Now Playing's Visualizer tab (the one 33 ms screen); `stats-wall-with-covers-debug.json` is
  step 5's 20 s on the wall in the DEBUG build.
- `cpu-*.tsv` — `ps` samples every 2 s (%cpu, cumulative CPU seconds, RSS) for the window and
  the terminal GUI on a pty, idle and in the Visualizer tab; CPU in the record is the cputime
  delta over wall time from t ≥ 10 s.
- `linux-linkage.txt` — the Linux leg's NEEDED list and guard exit (Docker, aarch64).
- `checklist.md` — the manual checklist and the criteria scorecard, from the step-4 judge.

## The script lever

`MSTREAM_WINDOW_SCRIPT=<file>` on a `--features desktop` binary plays a script of the raw events
the winit handlers build, one step per frame (`wait <ms>`, `key`, `text`, `ctrl`, `alt`, `ime`,
`preedit`, `move`, `click`, `drag`, `wheel`, `resize`, `scale`, `minimise <ms>`, `frame`,
`dump <path>`, `say`, `quit`; the full list is `src/gui/window/script.rs`'s header).
`freeze <ms>` holds the frame just drawn on screen that long, no frame after it, for a
screenshot of a frame the next would replace (a modal's opening frame, say). The script starts
as the window is made, not at its first frame, so a `wait` counts from the window's creation
and a slow GPU's blank second can be typed into: until the first frame only the steps that need
no frame run (`wait`, `say`, `resize`, `quit`, keys, text, and pointer steps in pixels), their
inputs held and replayed after it one act a frame as a person's are, and the first step that
needs a frame or the grid (`move`, `click`, `dump`, `frame`, `scale`, `minimise`, `freeze`)
waits for the first frame. The player's first effects (the connect among them) are sent as the
window is made too, not by its first frame, so a held key that needs the server (`/` opens the
search's field only once connected) finds it connected if the server answered before that frame.
`MSTREAM_WINDOW_STATS=<path>`, `MSTREAM_WINDOW_DUMP=<dir>` and
`MSTREAM_WINDOW_SIZE=<cols>,<rows>` are the other levers (PLAN.md, Phase 14).

## Faces

The window host (`src/gui/window/mod.rs`) shows a face: a `Box<dyn Face>` behind the small
object-safe trait in `src/gui/window/face.rs`, so a program other than the GUI can stand in a
window of its own without a second host (PLAN.md, Phase 15). The GUI is the first face
(`GuiFace` in `src/gui/mod.rs`), each method the call the host made into the Gui by name
before. What every face answers: its `title`, the `frame` half (one frame drawn into the
window's terminal and the wait until the next, with the window's cursor as its `Host`), the
`input` half (one translated event, `Flow::Quit` to leave), the caret of the field with the
keyboard (`caret_at`, which turns the input method and the paste chord on) and the input
method's composition (`set_composition`), `host_pictures` (the Board that draws pictures as
textures, and the overlay watch), `finish` (the way out, alone on the no-display path), and
`render_test`, what the fidelity dump draws into a `TestBackend`. The rest is optional and
defaulted to nothing: `grid` (100×30), `send_early` (work before the first frame), `copy` (the
copy chord, swallowed by default), `flush` (what a quit would have saved, for the close
button), `exit_code` (0 for a clean close; a failed frame and no window keep 1 and 3), and the
script lever's probes `hit_debug` and `drag_began`. Everything else (the renderer's build, the
held input and its replay, the input method's placement, the levers, the stats, the instance
lock) is the host's and no face's business.

## The two flavours

One codebase, two products, one CLI: an explicit argv means the same in both, and only an empty
argv differs. The terminal flavour (no features; the plain release binaries, deb/rpm, Homebrew,
Scoop, the install scripts, `cargo install`) opens the TUI, as v0.9.0 did. The desktop flavour
(`--features desktop`) opens the GUI in its own window, as `gui --window` does — unless no
window can be expected (a Unix other than macOS with none of `DISPLAY`, `WAYLAND_DISPLAY` or
`WAYLAND_SOCKET` set, or an SSH session
with a terminal on stdin), when it is the TUI as before; a macOS `-psn_…` argument alone
counts as empty. That launch takes a default instance lock, `desktop-player.lock` in the
player's config directory, so a second one prints the launcher's "already open" line (and logs
it) and leaves; whether to share the tray's lock instead is open. The sidecar's `host` reads
`window` for a player in its own window (`gui --window` or the desktop empty argv) and the
terminal's name otherwise. `mstream-player --version` (and `-V`) keeps its first line
`mstream-player X.Y.Z`; a build with the window adds `features: window` on a second. When the
window cannot open at all (no display, libxkbcommon-x11 missing, no GPU adapter or backend,
an event loop that will not start) the player exits 3, which a launcher takes as "use the
terminal route"; 1 stays every other failure. On Windows a double-clicked desktop binary
frees the console Explorer gave it (one still flashes as it starts), and the
`mstream-player-launch` stub, packaged as "mStream Player.exe", is a GUI-subsystem program
that starts `mstream-player.exe gui --window` beside it with no console at all.

## The desktop packages

Beside the bare `mstream-player-desktop-*` binaries, release.yml's `package-desktop` job wraps
each one the way its OS installs an app, with the layouts in `scripts/package-macos.sh`,
`scripts/package-windows.py` and `scripts/package-linux.sh` (the same scripts run on a dev Mac).
`mstream-player-desktop-darwin-{arm64,x64}.app.zip` holds `mStream Player.app`: bundle id
`io.mstream.player`, the executable `Contents/MacOS/mstream-player`, the icns, and an
Info.plist asking for the local network (`_mstream._tcp`, for discovery); when the signing
secrets exist the bundle is signed with the Developer ID, notarized and stapled, else it ships
unsigned and the run's summary says so. `mstream-player-desktop-win32-x64.zip` is portable:
`mStream Player.exe` (the launcher stub, to double-click), `mstream-player.exe` (the player,
also usable from a terminal), `mstream-player.ico` and a README.txt; it is **not code-signed**
for now (no Windows certificate yet), so SmartScreen warns on first run, which the README says.
`mstream-player-desktop-linux-x64.tar.gz` holds one directory, `mstream-player-desktop/`, with
the binary, `io.mstream.player.desktop`, the hicolor icons (16 to 512 px) under `icons/`, and a
README.txt with the per-user install into `~/.local`. The zip and the tarball are reproducible
(fixed order, the commit's time, owner 0). A pre-release's version, which Info.plist's
CFBundleShortVersionString cannot hold, keeps its numeric core there and the full string in
the bundle's `MStreamPlayerVersion` key.
