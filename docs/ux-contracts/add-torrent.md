# Add torrent

| | |
|---|---|
| **Design of record** | `mstream_music` @ `origin/master` (`0f8b23f`, merged 2026-08-28) — `lib/screens/add_torrent_screen.dart` (the "smart" panel), `lib/native/torrent_channel.dart` + `MainActivity` (both directions of the OS hand-off), the intent chooser in `lib/main.dart`, the two settings rows in `lib/screens/settings_screen.dart`, `lib/util/torrent_meta.dart`. The screen is itself a port of the webapp's standalone panel (`webapp/alpha/m.js setupAddTorrentPanel`; `webapp/torrent/` is the reduced mobile-web sibling) — where the two disagree, this contract says so. |
| **Server API** | `GET /api/v1/torrent/preflight?path=` (active · clientType · displayName · noUpload · userAllowed · reason) · `GET /api/v1/torrent/path-templates` · `POST /api/v1/torrent/auto-detect` (multipart) · `POST /api/v1/torrent/seed-existing` (multipart) · `POST /api/v1/torrent/add` (multipart: vpath · subPath · directoryName · renameRoot · exactly one of magnet / torrentFile) |
| **Already in this repo** | nothing before this slice — no torrent client surface existed; all five client calls, the multipart body and the metadata parser are new (`src/api/mod.rs`, `src/gui/torrent_meta.rs`). The kit brought the modal, the line editor, the wizard's path-completion machinery and the servers room's thread-and-reply pattern |
| **Target surface** | the GUI player — a room off Settings, plus `mstream-player gui --torrent <file-or-magnet>` for torrents arriving from the OS (`src/gui/torrent.rs`) |
| **Status** | contract extracted 2026-08-31; the five open questions settled and the room implemented 2026-09-01 (decisions in the deviations log) |

## Intent

Hand the server a torrent — a `.torrent` file or a magnet link — to
download into a chosen library at a music-shaped path, with the client
doing the smart part: read the torrent's own name, guess artist/album/year,
resolve the library's path template, and check whether the files are
already on disk before downloading anything. And because a torrent can
turn out not to be for the server at all, the flow can hand it onward to a
real torrent client instead. Being the system's handler for `.torrent`
files and `magnet:` links is what makes the receiving half exist.

## Entry points

1. **In-app navigation** — the screen on its own. *Here: the Settings
   room's `Add a torrent ▸` doorway, the Manage-servers shape.*
2. **Opened WITH a torrent** — the OS delivers a `.torrent` (file manager,
   browser download, share) or a `magnet:` link to the app. *Here: the
   `--torrent` seam, which the installers' file associations will
   launch.* What happens next is the **ask-me setting**'s call:
   - *ask* (default): a chooser — "add it here, or hand it on?" — with a
     don't-ask-again checkbox (checking it is what flips the setting;
     the Settings switch is the way back).
   - *skip*: straight into the screen, source pre-filled.
   Delivery is one-shot (cold start and already-running both drain the
   same slot, so nothing double-delivers), and every arrival is logged —
   "I opened a torrent and nothing happened" is untriageable otherwise.

## States & flows

One screen, revealed in steps. **Availability is per-server and the screen
is the gate** — there is no ping flag for torrents; `/torrent/preflight`
answers, and a banner explains why when the server can't take one
(feature off, uploads off, user not allowed — the server's own `reason`).

Server picker (multi-server) → library (vpath) picker → **source** →
metadata → destination → options → submit. Everything below the source
stays hidden until a source is real: a picked file, or a magnet with a
valid infohash.

## Behavior contract

### The source

1. **Exactly one of file / magnet.** Picking a file clears the magnet
   field; typing a magnet drops the file. Submitting with neither (or
   both) is refused in words.
2. The **file picker is typed to torrents** (extension `.torrent`, MIME
   `application/x-bittorrent`) and starts in the platform's Downloads —
   a torrent almost always arrives through the browser.
3. Picked bytes pass a **structural gate** (`isTorrentFile`: a bencoded
   dict carrying an `info` dict) — enough to reject an mp3 picked by
   mistake, named per file: "*name* is not a torrent file".
4. The **magnet field validates live** (a v1/v2 infohash in `xt`); an
   invalid one is marked in place, not at submit.
5. Once a file is loaded, the magnet field's spot shows the **file chip
   and the hand-off** instead (clause 60) — the two sources never show
   together.

### Metadata and the destination

10. Metadata pre-fills from the best name available: the torrent's own
    `info.name` (a byte-level bencode walk; never the whole info dict),
    the magnet's `dn` parameter, or the filename — through the loose
    music parser (artist − album, year token, junk stripped). The parse
    only pre-fills; it gates nothing.
11. **Auto-detect** (file sources, when the server offers it) asks
    `/torrent/auto-detect` and applies the answer, saying how sure it is
    ("detected" vs "best guess"); a miss says the server found nothing.
12. The **destination path autofills** from the library's template
    (`{{ARTIST}}/{{ALBUM}}` and friends), resolved client-side with the
    server's own sanitizer mirrored — the preview must match what
    `/torrent/add` will accept. No template → legacy `Artist/Album`.
13. **Editing the path by hand is sticky**: metadata edits stop
    recomputing it. A fresh source resets the stickiness.
14. The **preview line** shows the real landing spot:
    `/<vpath>/<path>/<contents>`.
15. **Rename root folder** is ON by default — the whole point of the
    resolved path is that the folder matches it, and a scene-release name
    rarely does. A rename that fails after a successful add is a warning,
    not a failure.

### Submit and the seed-existing check

20. File submissions run the **seed-existing check first** (unless
    *force fresh download* is ticked; magnets skip it — no file list to
    hash). The outcomes:
    - **seeded** — already in the library; the client is now seeding it.
      Done, said in words.
    - **already_in_daemon** — the torrent client already has it. Done.
    - **invalid_torrent** — refused with the server's words.
    - **daemon_error** — the *check* failed, not the add: say the check
      was skipped and fall through to a fresh download.
    - **partial_match** — some files exist elsewhere: the **match
      picker** (clause 21).
    - **no_match** (or anything unrecognized) — fall through to add.
21. The **match picker** lists each candidate location
    (`vpath/relativePath`, matched-of-total, missing count) — choosing
    one adds the torrent AT that location (seed what's there, fetch only
    what's missing); the last row is always **download fresh** at the
    typed path.
22. `/torrent/add` answers with the torrent's name and where files will
    land; a duplicate is its own wording ("already added"), not an error.
23. *(Record split, settled here for the webapp's side: this surface also
    words `match_unmapped` — files all present but the daemon's path
    mapping unconfirmed, "ask your admin to run auto-detect, then retry"
    — and `pad_files_missing` — a hybrid torrent this client type can't
    seed without re-downloading boundary pieces. The mobile screen falls
    through and lets `/add`'s 412 speak. See the deviations log.)*

### Receiving, and being the default

50. The app **registers for** `.torrent` (view + share) and the
    `magnet:` scheme — on most systems nothing else claims them, so a
    tapped magnet just arrives. *Here: the installers' job (Phase 8);
    the app ships the seam they launch.*
51. The **ask-me switch** in Settings decides what an arrival does
    (chooser vs straight to the screen). Stored as "skip", shown as
    "ask" — the sheet's checkbox sets it, the switch is the only way
    back.
52. ~~The **default-app row** in Settings opens the OS's own
    open-by-default screen~~ — *not ported; see the deviations log.*

### The hand-off

60. **Open with…** hands the loaded `.torrent` to a real torrent client
    (the system chooser), falling back to the share surface when none is
    installed, and saying so when nothing will take it at all.
61. The hand-off **leaves the form standing** — nothing was submitted, so
    backing out or picking a different file both still work.

## Wording

The record's `torrent*` and `settingsTorrent*` keys were carried over
from all ten `.arb` files into `locales/*.yml` (`gui.tor.*`, `gui.set.*`);
the strings new to this surface — the gate's standing, the picker, the
hand-off's outcomes, the two webapp seed-check sentences, the tips —
were written here in the same ten languages. The parser, sanitizer and
validator wordings above quote the record's English.

## Out of scope here

- **The admin side** (torrent client setup, path-mapping auto-detect,
  per-vpath templates authoring) — the webapp admin page's own feature.
- **The `/torrent` mobile-web page** — the reduced sibling, not the
  record.

## Translation notes (terminal GUI)

| Record | Here |
|---|---|
| System file picker, typed, starting in Downloads | The **native file dialog** first — the wizard's own backends (`src/setup/picker.rs`) grown a file-typed sibling: osascript `choose file of type {"torrent"}` on macOS (an NSOpenPanel from a terminal process never fronts), rfd's IFileOpenDialog on Windows, the XDG portal through ashpd on Linux — each filtered to `.torrent` / `application/x-bittorrent` and started in `~/Downloads`, run on a thread so the loop stays live. Where no dialog can open (SSH, no session bus, a refused osascript) the room says so and falls back to the kit's **path modal for files** (the wizard's typed completion re-drawn): the line editor starts in `~/Downloads/`, suggestions are that folder's sub-folders and `.torrent` files only, Tab completes, Enter descends or loads — also reachable on purpose (`t`, "type a path"), and the place a dropped file's path lands. The structural gate stays (clause 3) whichever road the file took. |
| Magnet TextField | The kit line input as a 1-row field; live infohash mark at the row's right edge (clause 4) |
| Opened-with (intents) | The **CLI seam**: `mstream-player gui --torrent <file-or-magnet>` as the `IncomingTorrent` equivalent — the chooser / ask-me flow rides it unchanged, and the arrival is logged |
| OS registration (manifest) | The **Phase-8 installers'** job: macOS document types + `magnet:` URL scheme on the .app, Windows file association, Linux `.desktop` MimeType — all launching the seam above. The app ships the seam; the installers ship the claim. |
| Android "open by default" settings screen | **Not ported** — no OS screen to open from a terminal app; the row returns with the installers (deviations) |
| Hand-off chooser (ACTION_VIEW) | `open` / `xdg-open` / `start` on the file, **staged** into the player's own temp folder — which doubles as the loop guard: an arrival from that folder is our own file coming back, i.e. this player is the default app, and the room says so instead of offering the hand-off again |
| Preflight banner | The sonic room's pattern inverted: the doorway shows when connected (no ping flag exists); the room itself banners the server's reason in gold, and the primary is disabled with the reason on its tip |
| Chooser + match-picker sheets | Kit modals (the sonic menu's shape) |
| Snackbars | The note line |
| Material TextFields, 56 px each | **1-row fields** (label column + value, the caret marking the focused one — the Settings rows' grammar). The kit's 3-row input cards would need 24 rows for the form alone; at the 100×24 floor the whole room — gate, source, metadata, destination, options, the tall primary — fits its 14 content rows exactly, and breathes at 18 |

## Decisions on the open questions (2026-09-01)

1. **Seed-check taxonomy split** — the webapp's fuller wordings are
   adopted for `match_unmapped` / `pad_files_missing` (they exist
   because the fall-through error confused people); the mobile's
   fall-through stays the record for the other outcomes.
2. **Delivery to a running instance** — v1 opens a second instance; the
   local socket is its own slice if it is ever wanted.
3. **The defaults row** — dropped until the installers exist; noted in
   the Phase 8 plan, which owns the registration.
4. **The hand-off** — kept, as one text verb on the file chip (mid-flow
   is exactly when you learn the torrent isn't for the server).
5. **Where the room lives** — a Settings doorway plus the `--torrent`
   seam; the nav digits are spent, and arriving from outside is the
   record's main road anyway.

## Deviations log

- **2026-08-31 — No design canvas planned**: the screen is a form — kit
  inputs, modals, a banner — every element already drawn once.
- **2026-09-01 — Clause 52 not ported**: an app cannot open a terminal
  into "System Settings › Default apps"; with nothing registered yet
  there is nothing to change either. The Phase 8 installers own the
  registration and the row comes back with them.
- **2026-09-01 — 1-row fields, not the kit's 3-row cards** (translation
  notes): the room's height budget at the 24-row floor.
- **2026-09-01 — The hand-off says so on success**: the record stays
  quiet (the system chooser is up); a terminal that just spawned `open`
  has nothing visible to show, so the note says "handed to the system's
  torrent app — the form stays as it is". "Nothing will take it" and
  "could not hand it over" keep the record's words.
- **2026-09-01 — A staged hand-off doubles as the loop guard**: the
  record's system chooser never offers the app itself; `open` may, once
  the installers register us. An arrival from the staging folder is
  named ("mStream is the default app for torrents — choose another app
  in your system settings") and taken, never bounced again.
- **2026-09-01 — A typed path beside the native picker** (review): the
  record has only the system picker; this surface keeps the typed
  modal as a named second road, because the dialog cannot exist over
  SSH and a terminal is where paths get pasted and dropped.
- **2026-09-01 — Done resets the form in place**: the record pops its
  screen; the room has nowhere to pop to, so a success (added, seeded,
  already there) clears the source and keeps the library and toggles
  for the next one.
