# Add torrent

| | |
|---|---|
| **Design of record** | `mstream_music` @ `origin/master` (`0f8b23f`, merged 2026-08-28) — `lib/screens/add_torrent_screen.dart` (the "smart" panel), `lib/native/torrent_channel.dart` + `MainActivity` (both directions of the OS hand-off), the intent chooser in `lib/main.dart`, the two settings rows in `lib/screens/settings_screen.dart`, `lib/util/torrent_meta.dart`. The screen is itself a port of the webapp's standalone panel (`webapp/alpha/m.js setupAddTorrentPanel`; `webapp/torrent/` is the reduced mobile-web sibling) — where the two disagree, this contract says so. |
| **Server API** | `GET /api/v1/torrent/preflight?path=` (active · clientType · noUpload · userAllowed · reason) · `GET /api/v1/torrent/path-templates` · `POST /api/v1/torrent/auto-detect` (multipart) · `POST /api/v1/torrent/seed-existing` (multipart) · `POST /api/v1/torrent/add` (multipart: vpath · subPath · directoryName · renameRoot · exactly one of magnet / torrentFile) |
| **Already in this repo** | nothing — no torrent client surface exists; all five client calls are new. The kit brings the local-path machinery (typed completion + the fallback browser, the wizard's), the modal, and the browse rooms' gating precedents |
| **Target surface** | the GUI player — a room (plus a CLI seam for torrents arriving from the OS) |
| **Status** | contract extracted 2026-08-31; **open questions below — discussion before implementation** |

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

1. **In-app navigation** — the screen on its own.
2. **Opened WITH a torrent** — the OS delivers a `.torrent` (file manager,
   browser download, share) or a `magnet:` link to the app. What happens
   next is the **ask-me setting**'s call:
   - *ask* (default): a chooser sheet — "add it here, or hand it on?" —
     with a don't-ask-again checkbox (checking it is what flips the
     setting; the Settings switch is the way back).
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
23. *(Record split: the webapp panel also words `match_unmapped` — files
    all present but the daemon's path mapping unconfirmed, "ask your
    admin to run auto-detect, then retry" — and `pad_files_missing` — a
    hybrid torrent this client type can't seed without re-downloading
    boundary pieces. The mobile screen falls through and lets `/add`'s
    412 speak. See Open questions.)*

### Receiving, and being the default

50. The app **registers for** `.torrent` (view + share) and the
    `magnet:` scheme — on most systems nothing else claims them, so a
    tapped magnet just arrives.
51. The **ask-me switch** in Settings decides what an arrival does
    (chooser vs straight to the screen). Stored as "skip", shown as
    "ask" — the sheet's checkbox sets it, the switch is the only way
    back.
52. The **default-app row** in Settings opens the OS's own
    open-by-default screen — an app cannot make itself the default; taking
    the user there is the most it may do. It is also the only in-app way
    back from having made some *other* app the default, which is
    otherwise a silent dead end. Both rows exist only on platforms where
    receiving works at all.

### The hand-off

60. **Open with…** hands the loaded `.torrent` to a real torrent client
    (the system chooser), falling back to the share surface when none is
    installed, and saying so when nothing will take it at all.
61. The hand-off **leaves the form standing** — nothing was submitted, so
    backing out or picking a different file both still work.

## Wording

The record ships `torrent*` keys in all ten locales — carried over at
implementation, not re-made. The parser, sanitizer and validator wordings
above quote the record's English.

## Out of scope here

- **The admin side** (torrent client setup, path-mapping auto-detect,
  per-vpath templates authoring) — the webapp admin page's own feature.
- **The `/torrent` mobile-web page** — the reduced sibling, not the
  record.

## Translation notes (terminal GUI) — for discussion

| Record | Here (proposed) |
|---|---|
| System file picker, typed, starting in Downloads | The kit's **local path machinery** — the wizard's typed completion + fallback browser (`docs/ui-kit.md`), started at `~/Downloads`, suggestions narrowed to `.torrent`; the structural gate stays (clause 3). Terminal bonus: dropping a file on the window pastes its path into the same input. |
| Magnet TextField | The kit line input; bracketed paste; live infohash mark (clause 4) |
| Opened-with (intents) | A **CLI seam**: `mstream-player gui --torrent <file-or-magnet>` as the `IncomingTorrent` equivalent — the chooser/ask-me flow rides it unchanged |
| OS registration (manifest) | The **Phase-8 installers'** job: macOS document types + `magnet:` URL scheme on the .app, Windows file association, Linux `.desktop` MimeType — all launching the seam above. The app ships the seam; the installers ship the claim. |
| Android "open by default" settings screen | **No terminal equivalent** — see Open questions |
| Hand-off chooser (ACTION_VIEW) | `open` / `xdg-open` / `start` on the staged file — with a guard for the case where *we* are the default (an open-with loop) |
| Preflight banner | The sonic room's pattern inverted: the nav row shows when connected (no ping flag exists), the room itself banners the reason |
| Chooser + match-picker sheets | Kit modals (the sonic menu's shape) |
| Snackbars | The note line |

## Open questions (to settle before implementation)

1. **The seed-check taxonomy split** (clause 23): adopt the webapp's
   fuller wordings for `match_unmapped` / `pad_files_missing`, or the
   mobile screen's fall-through-and-let-412-speak? The webapp's are
   actionable; the mobile's are simpler. My lean: the webapp's two
   sentences — they exist because the fall-through error confused people.
2. **Delivery to a running instance**: a second `mstream-player gui
   --torrent x` while one runs — hand the torrent to the running GUI
   (needs a local socket / file drop) or just run the flow in the new
   instance and exit? My lean: v1 opens a second instance; the socket is
   its own slice.
3. **The defaults row**: with no OS screen to open from a terminal app,
   does the row become guidance text ("your installer registered
   mStream for torrents; change it in System Settings → …"), or does it
   drop until the installers exist? My lean: drop it now, note it in the
   installer slice.
4. **Does the hand-off earn its place here?** On a desktop with a real
   torrent client installed, "open with the default app" is one
   `open` away — but the record keeps it because mid-flow is exactly
   when you realize the torrent isn't for the server. My lean: keep it,
   one text verb on the file chip.
5. **Where the room lives**: a nav row (digit 10 doesn't exist — the
   digits are spent), a Settings doorway like Manage servers, or a verb
   on the Files bar? My lean: a Settings doorway + the `--torrent` seam,
   since arriving-from-outside is the record's main road anyway.

## Deviations log

- **2026-08-31 — No design canvas planned**: the screen is a form — kit
  inputs, cards, modals, a banner — every element already drawn once.
  Revisit if discussion disagrees.
