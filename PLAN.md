# mstream-player — Plan

A terminal player for [mStream](https://github.com/IrosTheBeggar/mStream), grown out of mStream's
`rust-server-audio` jukebox engine. **One binary, two faces:**

- `mstream-player` — interactive terminal (TUI) client that streams from any mStream server
- `mstream-player serve` — headless audio engine speaking the JSON control API that
  mStream's server-audio feature uses today (drop-in successor to `rust-server-audio`)

mStream stops vendoring the Rust source and checked-in binaries; instead it downloads the
pinned release of this binary when server-side audio is enabled.

## Locked decisions

| Decision | Choice |
|---|---|
| Binary / crate name | `mstream-player` |
| License | GPL-3.0 (matches mStream) |
| Control API | v1 is byte-compatible with rust-server-audio's routes, plus additive `GET /version` returning `{name, version, apiVersion}`. Breaking changes bump `apiVersion`. |
| Version coupling | mStream vendors an exact player version + sha256 manifest per release. Never "latest". |
| Rust layout | Single crate with modules (`engine/`, `serve/`, later `api/`, `tui/`). Split into workspace crates only if another consumer appears. |
| Release targets | win32-x64, darwin-x64, darwin-arm64, linux-x64, linux-arm64, linux-arm (armv7) — same six as today |

## Control API v1 (the compatibility contract)

Routes preserved exactly as rust-server-audio served them:

```
POST /play {file}          POST /queue/add {file}
POST /pause                POST /queue/add-many {files}
POST /resume               POST /queue/play-index {index}
POST /stop                 POST /queue/remove {index}
POST /next                 POST /queue/clear
POST /previous             GET  /queue
POST /seek {position}      GET  /status
POST /volume {volume}
POST /shuffle {value}
POST /loop
```

Additions (v1, additive only):

- `GET /version` → `{"name": "mstream-player", "version": "x.y.z", "apiVersion": 1}` — never auth-gated
- `--host` (default `127.0.0.1`; pass `0.0.0.0` to restore the old LAN-exposed bind)
- `--auth-token <t>` / env `MSTREAM_AUDIO_TOKEN` — when set, every route except `GET /version` requires the `x-auth-token` header
- `--exit-with-parent` — engine exits when stdin hits EOF. Only pass it when the parent
  holds stdin open (Node must spawn with `stdio: ['pipe', ...]`; today it uses `'ignore'`,
  which would EOF instantly)
- Legacy alias: `mstream-player --port N` ≡ `mstream-player serve --port N`, so the binary
  is a rename-and-drop-in replacement under mStream's existing spawn contract

## Phases

### Phase 1 — Port the engine (this repo, behavior-compatible)
Copy `rust-server-audio` in (3 commits of history; provenance noted as mStream@bec11154) and split:
`src/engine/` (state, queue, transport — no HTTP), `src/serve/` (tiny_http API), `src/cli`.
Hardening: loopback bind default, optional auth token, `/version`, stdin watchdog, real RNG shuffle.
Audit fixes folded in (see appendix). CI: adapt mStream's 6-target build workflow into
tag-driven GitHub Releases with a sha256 `manifest.json` instead of committing binaries.
**Done when:** the new binary, dropped into mStream's `bin/` path under the old name, passes a
jukebox + `/server-remote` smoke test unchanged.

### Phase 2 — HTTP streaming source (the risk phase, done early) ✅ DONE 2026-07-31
`Source::Local | Source::Http` via `stream-download` + reqwest (buffered `Read + Seek` over range
requests) into the same rodio decoder. Duration hint accepted from callers (remote probing costs a
fetch; the TUI knows durations from the mStream API). Smoke command:
`mstream-player play [--url <url-or-path> | --server <url> --token <jwt> <vpath>] [--seek-to N]`.

**DoD met** — verified against a live mStream (throwaway instance, Windows):
- MP3 and FLAC stream and **seek** over HTTP (`/media/...?token=...`); required the rodio 0.22
  upgrade to pass `byte_len` through to symphonia (audit finding #13).
- `/transcode` characterized: first hit is chunked (no Content-Length) — streams fine, duration
  unknown, and seeking *works* as long as ffmpeg outruns playback (symphonia's forward scan just
  waits for bytes); cached hits serve full length and seek cleanly. Server-default codec is opus,
  which symphonia can't decode → clients must pin `codec=mp3`/`aac` (finding #14).
- Engine open path holds the state lock with a 5s connect timeout on the HTTP client — a dead
  server stalls the control API for at most ~5s (documented tradeoff; revisit if it bites).

### Phase 3 — mStream API client ✅ DONE 2026-07-31
`src/api/`: `mod.rs` (client, auth, error mapping), `types.rs`, `urls.rs`, `session.rs`.
Endpoints: login, ping, file-explorer, db/artists, db/artists-albums, db/albums, db/album-songs,
db/metadata, db/search, playlist/getall, playlist/load. Types hand-written against
`docs/openapi.yaml` and pinned to live responses by unit tests.

Design points that matter downstream:
- **Auth split.** The JWT goes in an `x-access-token` header for API calls (never the query
  string, so it stays out of server logs); only *stream* URLs carry `?token=`, which is what
  makes them self-contained enough to hand to the engine.
- **Public mode is a first-class case.** Servers with no users authenticate everything, so a
  token-less client is valid — not an error path.
- **Codec pinning is type-level.** `TranscodeCodec` has no `opus` variant, so finding #14 cannot
  be violated by construction; `--transcode opus` is rejected at argument-parse time.
- **Tolerant deserialization.** Every struct is `#[serde(default)]` with unknown fields ignored
  and `null` accepted for documented-nullable objects; only `filepath` is load-bearing. A client
  built against one server version keeps working against another.
- **Session token reuse is server-scoped** — a saved token is never sent to a different server URL.
- Shared `src/runtime.rs`: one process-lifetime tokio runtime for both the API client and the
  streaming source, still lazily built so local-only serve mode never starts tokio.

Debug CLI over the client (the test harness, as `play` was for Phase 2): `login` (password via
`MSTREAM_PASSWORD` or `--password-stdin`, never persisted — only the JWT is), `logout`, `info`,
`ls`, `browse`, `search`, `playlists`. `play` now takes a library path and resolves the server,
token, stream URL, and duration hint from the saved session.

**Verified against a live mStream** (throwaway instance, Windows) in both public and
authenticated mode: login success/failure exit codes, auth enforcement, session persistence and
token scoping, file-explorer, tag browsing, search, playlists, and end-to-end playback of a
session-resolved FLAC with a server-supplied duration hint plus a mid-track seek — direct and
transcoded.

### Phase 4 — TUI ✅ DONE 2026-07-31
ratatui 0.30 + crossterm. Screens: connect/login (with a separate "reconnecting" view so a saved
session never flashes a password prompt), **file explorer**, playlists, search, queue pane, and a
transport bar with a live position gauge. Vim keys and arrows, help overlay on `?`.

Architecture:
- **Three threads.** The UI thread only renders and reads keys. An *audio* thread owns the engine
  (created on its own thread, since audio handles aren't portable across threads) and ticks it;
  an *api* thread owns the client. Neither the network nor the audio device can stall a redraw.
- **Track-end detection lives in the audio thread**, which sees every status transition and can
  tell "the track finished" from "the user pressed stop" — the UI would have to guess by polling.
- **`app.rs` is I/O-free.** Actions and worker events go in; state changes and `Effect`s come out,
  and only the run loop touches channels. That is what makes navigation, queue advancement and
  repeat/shuffle testable without a terminal or a server (30+ tests do exactly that).
- **The queue lives in the TUI**, not in the engine, so rows can carry real metadata and ordering;
  the engine plays one track at a time. Repeat/shuffle reuse the engine's rule that a *manual*
  skip is never trapped by repeat-one.
- **`PlayerCtl`** (`src/player.rs`) is the seam an mpv backend would slot into.
- Rendering is covered by ratatui's `TestBackend`: every screen is drawn to a buffer and asserted
  on, including a password-masking check and a "doesn't panic in a 20×8 terminal" case.

Verified against a live mStream on Windows: the binary connects from a saved session, lists
libraries, and draws the full layout.

**Library tab** (added in the first iteration pass): a second tab browsing by tags, entered from a
static mode menu that costs no request — **Artists** → an artist's albums → tracks, **Albums** →
tracks, **Genres** → tracks, **Recently Added** → tracks. Navigation is a `LibraryNode` stack, and
because every response is tagged with the node it belongs to, a slow reply for a screen the user
already left is discarded instead of overwriting the current one. Genres drill straight to tracks
rather than albums because the albums-in-a-genre route is velvet-only (finding #15).

**Auto-DJ** (second iteration pass): `A` cycles off → similar → tempo+key, and when the queue has
nothing after the current track the player quietly requests one more, seeded on what's playing.
- *similar* uses `POST /api/v1/discovery/local/similar/tracks` (audio embeddings). Two states are
  not failures and are handled as fallbacks with an explanation on screen: the server answers 403
  when discovery collection is off, and `notAnalyzed: true` when the seed has no embedding yet.
- *tempo+key* uses `POST /api/v1/db/random-songs`. `src/dj.rs` converts whatever the tagger wrote
  ("A minor", "Am", "8A", "Gbm", "F♯m") into a Camelot code, then asks for the wheel neighbours
  and the relative major/minor; tempo windows are built at the same, half and double time, since
  a 140 BPM track mixes into 70. Implausible centres (outside 40–220) are dropped.
- The `ignoreList` cursor is round-tripped so a session doesn't repeat itself, and picks are
  deduped against the queue before being appended.
- Verified live: a 128 BPM / Am seed produced windows `120.3–135.7` + `60.2–67.8` and keys
  `8A, 7A, 9A, 8B`, matched the 126 BPM Em and 124 BPM C tracks, and correctly excluded the
  90 BPM F♯m one. Similarity ranked three sine tones above pink noise.

Splitting `ApiError::Forbidden` out of `Unauthorized` was a prerequisite: mStream uses 403 for
"feature disabled", per-user permissions, and request-validation failures, none of which should
bounce a user to a login screen.

**Deferred from this phase:** the "TOML config with multiple saved servers" item. Single-server
`session.json` (Phase 3) covers the common case; a server picker is a self-contained follow-up
and was not worth half-building here.

### Phase A — Configuration pass (state, first run, Quick Connect)

Inserted after Phase 4 and before release: shipping a binary with a "type a URL" first run and no
persisted settings would be the wrong first impression, and the storage layout is cheapest to fix
before anyone has files to migrate.

#### A1 — State storage

Audit of what exists today (2026-08-02). One file is ever written:
`<config-dir>/mstream-player/session.json` holding `{server, username?, token?}`, `0600` on unix,
inheriting the profile ACL on Windows. Everything else — volume, repeat, shuffle, Auto-DJ mode,
the queue, position, current tab and browse path — is in-memory and dies with the process, and
there is no library cache. Separately, `stream-download` spools each track to a `NamedTempFile`
in the OS temp dir (deleted on drop, leaked on hard kill).

Work:
- **Atomic session writes.** `fs::write` truncates then writes, so a crash mid-write leaves a
  truncated file — which the loader treats as a hard error telling the user to run `logout`.
  Write to a sibling temp file and `rename` (atomic on both platforms).
- **Split settings from secrets.** `config.toml` (servers, preferences — safe to sync, back up,
  or check into dotfiles) and a separate credential store. This is also what unblocks the
  multi-server item deferred from Phase 4. OS keychain stays a later option, not a prerequisite.
- **Schema version** on both files from the start.
- **Configurable stream cache.** Whole tracks land in the OS temp dir, and `/tmp` is tmpfs on
  many Linux distros — a 400 MB FLAC silently costs 400 MB of RAM. Expose the directory
  (`TempStorageProvider::new_in`) and offer `BoundedStorageProvider` for constrained boxes.
- **Persist what a player is expected to remember**: volume, repeat/shuffle/Auto-DJ mode, last
  server, last browse path. Restoring the queue and position is a separate decision — nice, but
  it interacts with Auto-DJ and needs a "resume?" affordance rather than silently replaying.
- Document `MSTREAM_PLAYER_CONFIG_DIR` (currently only tests use it; it's how portable installs
  would work).

#### A2 — First run

Today the first screen is three empty fields. mStream advertises itself over mDNS
(`_mstream._tcp`, **enabled by default**) with TXT records carrying `name`, `scheme`, `port`,
`path` (so reverse-proxy subpaths resolve), `v`, `auth=apikey,jwt`, and `iroh=1` when the tunnel
is available. That is enough to replace typing with picking.

- ✅ Browse `_mstream._tcp` (crate: `mdns-sd`) and present found servers with their friendly
  names. Landed on the Quick Connect screen first, which is where it earns the most: that screen
  now lists servers found on the network — picking one connects directly, no code needed — above
  the paste-a-code row for reaching a server anywhere. Servers are labelled "pairing available"
  only when they advertise `iroh=1`. Still to do: offer the same list on the Direct branch.
- Build the base URL from the TXT record rather than guessing `http://host:3000`.
- Normalise hand-typed input (accept `host`, `host:3000`, bare paths) instead of rejecting it.
- Public-mode servers should connect with no credentials at all — detect and skip the password.
- Show the `iroh=1` capability so Quick Connect is only offered where it can work.

#### A3 — Quick Connect (Iroh)

**What it actually is** (verified in `src/api/iroh.js`, `src/state/iroh.js`,
`docs/iroh-pairing-code.md`): a long opaque string `mstr1:<base64url>` whose payload is
`{t: <EndpointTicket>, s: <base64 32-byte connectSecret>}`. Not a short code, not a PIN.

- `GET /api/v1/iroh/code` is **core**, but returns the code only when `iroh.enabled` **and**
  (`iroh.shareCodePublic` **or** the caller is admin). `iroh.enabled` defaults to **false**.
- Dial: parse `t` → `EndpointAddr`, connect with ALPN `mstream/tunnel/2`, then on the **first**
  bi-stream write the raw 32 secret bytes, finish the send side, and read `"OK"`. Rejection can
  surface as a non-`OK` read *or* as a thrown QUIC error depending on platform — handle both.
- After that, **one bi-stream == one TCP connection**, and ordinary HTTP rides it.

**The design that makes this cheap:** run a loopback TCP listener that opens a bi-stream per
inbound connection, and point the existing `api::Client` at `http://127.0.0.1:<port>`. Every
Phase 3 endpoint, range requests and seeking included, then works unchanged.

Three things to be honest about in the UI:
- **Pairing is not login.** The secret gates the pipe, not the API — the user still logs in
  normally over the tunnel. The `/api/v1/auth/pair` one-time-token exchange is specified in
  `docs/iroh-pairing-code.md` as v2 but **is not built**.
- **Pair on the LAN, roam later.** The code can only be fetched over an existing connection, by
  an admin. So the flow is: connect at home, save the code, use it from anywhere afterwards.
- Rotating the secret (`POST /api/v1/admin/iroh/rotate-secret`) invalidates every code ever
  issued — it is the only revocation, and it is all-or-nothing.

Needs the `iroh` (1.x) and `iroh-tickets` crates. **Risk:** nothing in the mStream repo tests a
Rust client against the Node NAPI tunnel; the only handshake test is Node-to-Node. Prove the
handshake against a live tunnel before building UI on top.

### Phase B — UI iterations (Auto-DJ, journeys, discovery, admin)

Feature-detect everything here from `GET /api/v1/ping` — it reports `discovery`, `discoveryPath`,
`discoveryP2p` and `federationDiscovery`, and the in-code rule is "no flag, no probes". Absent
fields mean an older server: treat as false. (Our `Ping` type needs extending to carry them.)

#### B1 — Auto-DJ panel
Promote the footer toggle to a real panel: mode, tempo tolerance, key strictness, genre
whitelist/blacklist, rating floor, artist cooldown, and a preview of the next few picks.
The big win available from `random-songs` and not yet used: `similarTo` (1–8 paths, averaged into
a **session centroid** — send the recent picks as a rolling anchor) paired with `minSimilarity`.
Joi enforces both-or-neither. Real calibration from the server source: same-artist ≈ .6–.9,
cross-artist ≈ .3–.7, so map a perceptual slider onto that range. The sonic pool is a hard
constraint — the BPM/key waterfall never widens outside it and fails loudly instead.

#### B2 — Sonic Journey
`POST /api/v1/discovery/local/path` with `{startFilePath, endFilePath, length}` (4–32, default 14)
returns waypoints along the great-circle arc between two tracks' embeddings, each carrying `t`
(0→1 arc position). `length` counts **total rows including both seeds**, so the result is the
queue. UI: pick a start (usually what's playing) and an end, choose a length, preview the arc,
queue it. Handle `notAnalyzed` as the **per-end object** `{start, end}` it is, so the message can
name which end is still waiting, and the same-audio short circuit that returns just two rows.

#### B3 — Discovery browsing
- Local: similar tracks, and similar artists with their `entryPoints` (up to 2 playable doorways
  per artist) — a natural "more like this" and "artist radio".
- P2P and federation similarity, clearly labelled with provenance (`peer.name`) and with the
  privacy difference surfaced: p2p queries run against local snapshot copies and never leave the
  machine, while federation sends the seed vector to peers the admin paired with.
- P2P results carry no `filepath` (you don't have those tracks) — present them as discovery, not
  as something to queue. Federation results do carry one and are streamable.

#### B4 — Admin panel
Admin auth is the **same JWT** — `admin` comes from the users table, and the token is byte
identical, so the client must probe rather than inspect it. Two gates will bite a terminal client:
`lockAdmin` returns 405, and `adminAccess.mode` restricts by IP, so a panel run from another
machine gets 403 under hardened configs. Say that plainly rather than showing a bare error.

Build, in order of fit: **logs** (`/api/v1/admin/logs/recent?since=<seq>` is a purpose-built
tail-poll API with a cursor), **scan progress** (use the *non-admin* `/api/v1/scan/progress` and
`/api/v1/scan/status` — no admin rights, no IP gate), **users & access**, **server audio**
(pairs with `/api/v1/server-playback/*`), **scan params and server config** (uniform scalar
toggles), then transcode, federation and p2p peer management.

Skip: log/export zip downloads, the SSL cert upload, the admin file-explorer tree, and backup
destination creation — all genuinely browser-shaped. Guard with confirmation:
`config/secret` (logs everyone out), `config/ui` (reboots and changes which routes exist),
`iroh/rotate-secret`, and `config/admin-access` (can lock you out from your own IP).
Note `users/access` is a **full replace, not a patch** — read-modify-write or you'll silently
clear flags.

### Phase 5 — Release & install
Tag-driven releases (binaries + `manifest.json` with per-file sha256). README install matrix.
brew tap / scoop manifest later.

### Phase 6 — mStream flip (cleanup lands here, in the mStream repo)
- Binary-manager module: platform/arch → asset name (same `-${platform}-${arch}` scheme),
  download pinned version, verify sha256 against a manifest **vendored in mStream**, store in a
  writable data dir (retires the chmod-on-boot hack), `serverAudioBinaryPath` override for
  air-gapped installs.
- `server-playback.js`: resolve via the manager; spawn with `--host 127.0.0.1 --auth-token` and
  piped stdin + `--exit-with-parent`; check `/version` compatibility; surface download
  progress/errors via `/api/v1/admin/server-audio/info`.
- Delete: `rust-server-audio/`, `bin/rust-server-audio/` (~17 MB + recurring CI-commit churn),
  `build-rust-server-audio.yml`.
- Keep `cli-audio/` fallback (MPD/mpv/VLC/mplayer) — safety net for failed downloads and the
  MPD-on-NAS crowd. Separate audit later.
- Opportunistic fixes while in the file: rename `bootRustPlayer` → `bootServerAudio`
  (call sites: `src/server.js`, `src/api/admin.js`); fix `absoluteToVpath` prefix bug
  (`startsWith` without a path-separator guard — `C:\Music` matches `C:\MusicVideos`);
  consider replacing `/server-remote` regex page surgery with a template marker.

### Phase 7 — Backlog (deliberately deferred)
Gapless (append-to-sink redesign of the advance loop), TUI as remote for server-side audio,
album art (ratatui-image), media keys (MPRIS/SMTC), scrobbling hooks, brew/scoop/AUR packaging.

## Known risks (accepted)

- Linux binaries link ALSA dynamically (`libasound` required at runtime — already true today).
- Download-on-enable adds a failure mode → covered by `serverAudioBinaryPath` + CLI fallback.
- Two-repo version skew → covered by pinning + `apiVersion` check.
- rodio device-hotplug behavior and gapless are deferred, not solved.
- **Iroh tunnel wire compatibility is unproven** (Phase A3): mStream's tunnel runs in-process via
  the `@number0/iroh` NAPI addon, and nothing in that repo tests a Rust `iroh` client against it —
  the only handshake test is Node-to-Node. Both sides are on the iroh 1.x line, which is
  suggestive, not evidence. Spike the handshake first; the phase is scoped so the rest of Quick
  Connect is worthless without it.
- Phase B depends on server-side features that are off by default (`collectDiscoveryData`,
  `discoveryP2p.enabled`, `federation.enabled`, `iroh.enabled`). Every one is feature-detected
  from `/api/v1/ping`, so the client degrades rather than erroring — but it does mean most of
  Phase B is invisible on a default install.

## Appendix — Phase 1 audit findings (of rust-server-audio @ mStream bec11154)

| # | Finding | Severity | Action |
|---|---|---|---|
| 1 | Volume resets to 100% on every track change (`play_current` builds a fresh `Sink`, never re-applies volume) | user-facing bug | fixed in port: desired volume stored in state, applied to each new sink |
| 2 | Manual `/next` under loop-one replays the same track forever (`pick_next_index` applies `LoopMode::One` to manual skips) | user-facing bug | fixed in port: manual next/previous bypass loop-one; auto-advance still honors it |
| 3 | Missing/vanished audio device panics (`expect` on `OutputStream::try_default` / `Sink::try_new`) — cryptic crash on headless boxes | robustness | fixed in port: clean error + exit(1) at boot, surfaced error mid-session instead of panic |
| 4 | `POST /queue/remove` of the current index **while stopped** starts playback as a side effect | bug | fixed in port: replay-on-remove only when not stopped |
| 5 | Shuffle "randomness" is a hash of the system clock; biased and predictable | quality | fixed in port: `fastrand` |
| 6 | Binds `0.0.0.0` with no auth; any LAN peer gets full transport control + local-path playback (file-existence oracle) | security | fixed in port: default `127.0.0.1`, `--host` opt-out, optional `--auth-token` |
| 7 | `get_file_duration` re-opens and re-probes the file already opened for decode | perf (matters for Phase 2 HTTP) | deferred to Phase 2: duration-hint parameter |
| 8 | Between-tracks the 250 ms advance poll reports `playing: false` transiently; inter-track gap is audible (no gapless) | known limitation | deferred (Phase 7 gapless) |
| 9 | Shuffle has no history: `previous` can't retrace shuffled order; shuffle never ends under loop=none | semantics quirk | deferred to Phase 4 (queue UX pass) |
| 10 | mp3 without duration metadata (no Xing header) reports `duration: 0` | known limitation | documented |
| 11 | Negative or non-finite `/seek` position reaches `Duration::from_secs_f64`, which panics — and a panic while holding the state mutex poisons it, wedging every later request | crash bug (found during port) | fixed in port: positions validated before conversion |
| 12 | `/status` reports `playing: true` for a few ms after `/stop` — `sink.empty()` only flips on the next audio callback, so the old `playing` expression raced the audio thread | cosmetic race (found in Phase 2 testing; present in original) | fixed: `playing` also consults the engine's own synchronously-set `stopped` flag |
| 13 | FLAC files without a SEEKTABLE block (typical for ffmpeg-encoded FLACs) are **unseekable** — rodio 0.20's decoder wrapper hardcoded `byte_len: None`, so symphonia couldn't binary-search. Applies to the shipped jukebox with local files too, not just HTTP | seek bug, latent in original (found in Phase 2 testing) | fixed: upgraded to rodio 0.22 and its `DecoderBuilder` — engine now passes `byte_len` from file metadata / HTTP Content-Length + `with_seekable(true)`. Also fixed wrong duration estimates over HTTP (was reporting 64.29s for a 60s file) |
| 14 | mStream's default transcode codec is **opus**, which symphonia cannot decode — a client naively requesting `/transcode/...` with server defaults gets an unplayable stream | client-design constraint (found in Phase 2 testing) | Phase 3 requirement: the API client must always pin `codec=mp3` (or `aac`) in transcode URLs, never rely on the server default |
| 15 | Several library routes documented in `docs/openapi.yaml` — `db/genre/albums`, `db/genre/songs`, `db/genre-groups`, `db/decades`, `db/decade/albums`, `db/decade/songs` — live in `src/api/velvet-stubs.js` and are **only mounted when `config.ui === 'velvet'`**. On a default-UI server they 404, and `/api/v1/ping` gives a client no way to tell which UI mode the server runs | client-design constraint (found in Phase 4 testing) | Library browsing uses only core routes: `db/genres` + `db/genre-songs` (so Genres drills straight to tracks, not to albums), `db/artists`, `db/artists-albums`, `db/albums`, `db/album-songs`, `db/recent/added`. Decade browsing is not offered at all |
