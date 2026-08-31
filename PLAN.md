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

**Keys** (revisited 2026-08-03 after reading how cmus, ncmpcpp, musikcube, rmpc and termusic
handle navigation). Bindings live in one table per mode in `app.rs`, and `?` renders the help
*from that table* — the hand-written copy had already drifted, still advertising four tabs a day
after the fifth landed. A key press now carries its modifier, which is what stopped `Ctrl+D`
being read as plain `d` and silently removing a queue entry.

Three things the survey said we were missing, each with independent precedent:
- **Jump to what's playing** (`i`) — cmus `i`, ncmpcpp `o`, musikcube `x` all have it, and this
  player made the need worse: browsing now goes five tabs and two drill-downs deep.
- **A coarse seek** (`{` `}`, one minute) — cmus carries two granularities for a reason; five
  seconds is the wrong unit for a long mix.
- **Half-page scroll** (`Ctrl+u` / `Ctrl+d`) — vim-standard, and reachable without a Fn key.

Deliberately not adopted: `p` stays "previous track" though cmus, ncmpcpp and termusic use it for
pause (our `Space` is unambiguous, and `n`/`p` is equally standard elsewhere); `Space` stays
play/pause rather than ncmpcpp's select/add. Noted for later: `n`/`N` are spoken for everywhere
else as search-match navigation, so an in-list find would need different keys, and bare `g`
forecloses the `g`-prefix namespace rmpc uses for `gt`/`gT`.

**Configurable bindings** ✅ (2026-08-03) — the last thing all five of those players had and this
one didn't. A `[keys]` section in config.toml maps action names to key lists, layered over the
defaults; `mstream-player keys` prints the current map in exactly that format, and a round-trip
test proves pasting its output changes nothing. Naming an action *replaces* its keys, so a
binding can be moved or removed (`[]`); a claimed key is taken off whatever held it, so moving
one never means unbinding another by hand. Three rules came from things that went wrong while
building it: two actions claiming one key would have cancelled *both* (first claim now wins, and
the loser keeps its old key); a line whose every key was unreadable would have silently unbound
the action (a typo now costs the line, not the binding); and `ctrl+D` parsed to a key no terminal
sends (Ctrl folds case, bare letters don't). `Ctrl+C` is not rebindable — it is the way out when
everything else is confusing. Panel keys stay fixed, since panels draw their own hints.

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

Work (✅ all done 2026-08-02 — atomic writes, schema versions, the config/credentials split and
persisted preferences landed together; the spool relocation followed the same day):
- ✅ **Atomic session writes.** `fs::write` truncates then writes, so a crash mid-write leaves a
  truncated file — which the loader treats as a hard error telling the user to run `logout`.
  Write to a sibling temp file and `rename` (atomic on both platforms).
- ✅ **Split settings from secrets.** `config.toml` (servers, preferences — safe to sync, back up,
  or check into dotfiles) and a separate credential store. This is also what unblocks the
  multi-server item deferred from Phase 4. OS keychain stays a later option, not a prerequisite.
- ✅ **Schema version** on both files from the start.
- ✅ **Streaming scratch space** (was "configurable stream cache" — renamed because it is a
  *spool*, not a cache: only the playing track has a file, nothing is prefetched, nothing
  persists — Phase C loosens the one-file rule: while a crossfade prepares and blends, the
  upcoming track spools alongside the playing one, and a prepare cancelled by a queue edit
  holds its spool until its open completes or times out, so churn can briefly hold more). The problem stands as stated: whole tracks landed in the OS temp dir, and `/tmp` is
  tmpfs on many Linux distros — a 400 MB FLAC silently cost 400 MB of RAM. Spool files now go to
  `<platform cache dir>/mstream-player/spool` (`%LOCALAPPDATA%`, `~/Library/Caches`,
  `$XDG_CACHE_HOME`/`~/.cache`), overridable with `[cache] dir` in config.toml or
  `MSTREAM_PLAYER_CACHE_DIR` (`~` expands). They carry an `mstream-spool-` prefix so startup can
  sweep leftovers from killed runs without touching anything else — safe against a concurrently
  running instance, because unlink-while-open is harmless on unix and the Windows handles hold
  delete sharing (worst case the delete is refused and skipped). An unusable configured dir falls
  back to OS temp with a one-time warning instead of failing playback. Two scope decisions:
  `BoundedStorageProvider` ("offer it for constrained boxes") was **rejected** — it errors on any
  seek outside its window, which would regress exactly what finding #13 fixed; and queue
  prefetch / a persistent track cache are real features, not knobs on this one — moved to
  Phase 7. Verified live against demo.mstream.io: spool file appears in the new dir during
  playback, `%TEMP%` stays clean, the file self-deletes on exit, and a planted orphan was swept
  while a foreign file in the same dir was left alone.
- ✅ **Persist what a player is expected to remember**: volume, repeat/shuffle/Auto-DJ mode, last
  server, last browse path. Restoring the queue and position is a separate decision — nice, but
  it interacts with Auto-DJ and needs a "resume?" affordance rather than silently replaying.
- ✅ Document `MSTREAM_PLAYER_CONFIG_DIR` (currently only tests use it; it's how portable installs
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
  only when they advertise `iroh=1`.
  **Not extending the list to the Direct branch** (decided 2026-08-02): anyone who has registered
  a domain for their server wants to type it, so mDNS names would be the wrong default in the
  place people go to type an address.
- ✅ Build the base URL from the TXT record rather than guessing `http://host:3000` — scheme,
  port and the `path` prefix all come from the advert, so reverse-proxy subpaths resolve.
- ✅ **Normalise hand-typed input.** `src/api/server_url.rs` completes what a person types before
  anything else sees it: `nas:3000`, `demo.mstream.io`, `::1`, a pasted address bar with a
  `#!/artists` fragment, embedded credentials, a trailing slash. Every entry point runs through
  it — the connect screen and every `--server` flag — because the normalised string is what
  lands in `config.toml`, and storing what was typed would leave `nas:3000` and `http://nas:3000`
  looking like two servers, each holding half a session.
  - **The missing scheme is guessed from where the server lives:** loopback, RFC1918, link-local,
    `.local`, and single-label hostnames get `http` (how a LAN mStream is actually served);
    everything else gets `https`, because guessing `http` for an address that might be public
    would silently downgrade every later request. A typed scheme is always obeyed.
  - This kills the worst first-run message in the app. `Url::parse` reads `nas:3000` as *scheme*
    `nas`, so the old error accused the user of typing a protocol they hadn't typed; a bare
    hostname got "relative URL without a base".
- ✅ Public-mode servers connect with no credentials at all — an empty username sends `Connect`
  rather than `Login`.
- ✅ Show the `iroh=1` capability so Quick Connect is only offered where it can work.
- ✅ **Validate before dispatching.** A mistyped address, or a username with no password, is
  answered immediately instead of after a round trip. The completed URL is written back into the
  field, so what was assumed is visible when it turns out to be wrong.
- ✅ **Warn before a password crosses the internet in the clear.** The check existed
  (`is_insecure_remote`) but only the CLI used it; the TUI sent passwords over plaintext http
  without comment. Now it asks once and takes a second Enter as consent, keyed to the URL so
  editing the address asks again. Its definition of "remote" was also widened to exclude private
  and `.local` addresses — the same host classification the scheme guess uses — because a warning
  on every LAN sign-in is noise, and noise is what gets clicked through.
- ✅ Connect-screen messages wrap instead of being cut at the terminal edge. They end in the
  instruction ("Enter again to send it anyway"), so a hard cut removed the only actionable part.

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

**Identity is not the endpoint** (fixed 2026-08-02, after the loopback URL was found being saved
as the server). A tunnel session has two names and they must not be confused: the loopback bridge
it sends bytes to *this run*, and `mstream+iroh://<endpoint-id>` — the iroh public key — which is
what it is remembered as. The first is an ephemeral port; saving it meant the next launch dialled
a port that no longer existed, and the JWT was filed under a URL that could never match again.
The endpoint id holds still across ports, networks and a re-issued code for the same server.
The pairing code goes in `credentials.toml` beside the token, since it carries the tunnel secret
and is the only thing that turns a remembered identity back into a connection; `config.toml` gets
only the identity, which is a public key and safe to sync. Sign-out keeps the code deliberately —
fetching a new one needs admin access over an existing connection, so dropping it could strand
someone away from home. The CLI subcommands can't dial a tunnel and say so instead of failing on
a URL parse.

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

#### B0 — Feature detection ✅ DONE 2026-08-02

Everything in this phase is gated on `GET /api/v1/ping`, which reports `discovery`,
`discoveryPath`, `discoveryP2p` and `federationDiscovery`. The rule is **no flag, no probe**:
each of these is off by default server-side, and asking anyway earns a 403 that reads like a
failure but isn't. Absent fields mean an older server and are treated as false — the same answer
for the same reason. Each feature is gated on its own flag rather than inferred from another,
since the server reports them separately.

`Ping` carries the four flags; `Capabilities` (a small `Copy` value in `api/types.rs`) lifts them
out for anything deciding whether to offer a feature. The api thread refreshes it from one place —
where `Event::Connected` is sent — so a new way of connecting cannot forget to ask, and the app
stores its own copy from the same event.

First use, and the pattern the rest of Phase B follows: Auto-DJ no longer probes similarity on a
server without an index, `A` cycles straight past a mode that can't work, and a *remembered*
`similar` mode is dropped with an explanation when reconnecting somewhere that lacks it —
preferences are global, capabilities are per-server. `mstream-player info` prints the enabled
features, where "none enabled" is the ordinary answer on a default install.

#### B1 — Auto-DJ panel ✅ DONE 2026-08-02

`D` opens a panel over the player: mode, sonic pool + anchor, tempo window, key matching, rating
floor, artist cooldown, and genre whitelist/blacklist with a chooser fed by `db/genres`. `p`
samples three picks from the current settings without queueing any, which is how a setting gets
judged before it is committed to. Everything persists under `[player.dj]`.

`similarTo` + `minSimilarity` are now used, and are the reason the panel is worth having.
Request building lives in `dj::build_random_request` — pure, so what reaches the server is
pinned by tests reading the actual JSON, which is the only place absent-vs-empty is visible
(an empty array is not "no filter": it puts the request into continuity mode).

- **Both-or-neither is structural.** `RandomSongRequest::with_sonic_pool` takes seeds and
  threshold together and drops both unless both are present, so the `.and('similarTo',
  'minSimilarity')` 400 is unreachable rather than merely avoided. It bites on the first pick of
  a session, where tightness is set but nothing has played yet.
- **The slider is perceptual.** Raw cosine 0..1 is nearly all dead travel — the server's own
  calibration puts same-artist around .6–.9 and cross-artist .3–.7 — so 1–100 maps onto
  .30–.85, and the panel shows the raw value it lands on plus the pool size the server reports.
- **A hard constraint stays hard.** Tempo, key and artist relax inside the pool; the pool never
  widens. When it empties, the 400 is caught and retried once *without* the pool, saying so,
  rather than stalling the queue.
- **Feature-gated** on `discovery` from B0: no index, no sonic rows, no `similarTo` on the wire.

Two things live testing caught that reasoning had not: a modal overlay borrowing the player's
keymap meant `p` arrived as "previous track" (fixed with an `InputMode::Panel` binding set), and
a pick confined to 37 sonically-close tracks was being described as "picking at random".

**Deferred:** `artists` (similar-artist scope) needs a Last.fm proxy that isn't built; the
waterfall's step 1–5 branch stays unused until then.

#### B2 — Sonic Journey ✅ DONE 2026-08-02

`J` on a highlighted track plots a route to it from whatever is playing — `POST
/api/v1/discovery/local/path`, waypoints along the great-circle arc between the two embeddings.
With nothing playing it takes two presses: one to mark where to set off from, one to say where to
end up. The panel lists each stop with its arc position, `←→` changes the length (4–32), `Enter`
makes it the queue and starts it, `Esc` walks away.

- **Length is a replot, not a trim.** `length` counts total rows including both seeds, so the arc
  is *resampled* when it changes — verified live: at 14 stops the waypoints sit 0.091 apart, at 10
  they sit 0.111 apart and the same tracks come back with different similarities.
- **Three non-failures, each with its own sentence,** decided by a pure `journey_note`: an
  unanalysed end (per-end, so the message names *which* one is waiting), two seeds that are the
  same recording (the route short-circuits to just the ends), and an arc that came up short
  because waypoints snap to *visible* tracks and a small library runs out.
- Gated on `discoveryPath` from B0 — mStream's own comment says that flag's real payload is
  "this server version has the route".

Verified live against demo.mstream.io: a 14-stop journey ran from one track to another through
evenly-spaced waypoints, drifting through a second artist mid-arc and arriving where asked;
shortening it twice replotted each time; queueing replaced the queue with the 10 stops and
started playing. The same-track short circuit was hit by accident first and reported correctly.

#### B3a — Discover tab, local ✅ DONE 2026-08-02

A fifth tab, built the way the Library tab is: a static mode menu that costs no request, then
drill-down. Two modes — **Similar tracks** (ordinary playable rows, so Enter and `a` mean what
they always mean) and **Similar artists** (rows carrying how close, how many ways in, and what it
sounds like; Enter opens that artist's `entryPoints`).

The design question was how a terminal handles four sources whose rows mean different things.
The answer is **don't mix them**: one mode per source, homogeneous rows inside each, so Enter
never changes meaning within a list.

- **The seed is what's playing, or failing that what's highlighted** — the same rule Sonic Journey
  uses. Captured *before* the tab changes, since switching tabs moves the cursor; getting that
  ordering wrong made the seed always empty, which the tests caught and reading did not.
- **Entry points arrive with the artist list**, so drilling into one costs nothing — the reason
  the server inlines them.
- **The tab is hidden where the server has no index.** Tab numbers are positions in the *visible*
  list, so they stay 1..n with no gaps and no key points at a dead tab.
- Model tags are hierarchical (`Electronic---Dubstep`); only the leaf is shown. Live, the shared
  prefix filled the column on every row and said nothing.

Verified against a live personal server with a real index: Library → highlight → Discover carried
the seed across, similar tracks returned 40 neighbours, similar artists returned a ranked list
with tags, and drilling into one played a doorway.

#### B3b — Discovery browsing, network (todo)
- P2P and federation similarity, clearly labelled with provenance (`peer.name`) and with the
  privacy difference surfaced: p2p queries run against local snapshot copies and never leave the
  machine, while federation sends the seed vector to peers the admin paired with.
- P2P results carry no `filepath` at all — leads, not files. Present them read-only rather than
  inventing an action; `searched.{peers,tracks}` is worth showing.
- Federation results **are** streamable: `GET /api/v1/federation/peers/:id/stream/<path>` proxies
  over the bridge with Range forwarded, so seeking works. It deliberately has no transcode, so a
  peer's opus file cannot play — degrade with a clear message. (The comment at
  `discovery-federation.js:147` still calls the proxy "future"; it exists.)
- **Cost:** the queue assumes every track is local (`play_index` builds `media_url` from
  `self.server`), so queue items need to carry their origin before a peer's track can play.
- **Verifiability:** the personal server has `discoveryP2p` on, so the p2p half can be tested
  live. No federated peers are available, so that half would ship unverified.

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

### Phase C — Crossfade & prefetch (pulls Phase 7's prefetch forward)

Blend the end of each track into the start of the next, the way DJ software and the streaming
players do it. This is Phase 7's "next-track prefetch" with a fade on top: the prefetch —
opening the upcoming track's reader before the current one ends — is most of the work either
way, and it is also what removes the audible inter-track gap (finding #8) for anyone who turns
the fade on. Off by default everywhere: with `crossfade_seconds = 0` (and `gapless` off) the
player prefetches nothing and transitions as it always did — C4's soft cuts on manual
skip/stop/seek are the one deliberate global change, replacing clicks with short breaths.

**Why rodio suffices** (checked against rodio 0.22.2 source, not docs): every track already
plays as a `Player` connected to the one shared `Mixer` inside `MixerDeviceSink`, and that
mixer's whole job is summing simultaneous sources — it resamples each one to the device shape
through `UniformSourceIterator`, so a 44.1k track fading into a 48k track needs nothing from
us. Crossfade is "let two Players overlap with opposing gain ramps" rather than today's
stop-then-start. rodio's own `crossfade()` helper is *not* usable — it returns only the
overlapped portion and severs seek/pause/position — and `Player::set_volume` ramped from a
tick loop would staircase at 120 ms steps, so the fade is a source adapter of our own.

The pieces, in landing order:

#### C1 — The fade adapter ✅ DONE 2026-08-06
`engine/fade.rs`: a `Faded<S>` source wrapper (same shape as `tap::Tapped`) applying an
equal-power gain ramp — gain = sin(p·π/2), p stepped linearly per *frame* (per-sample stepping
would give each channel of a frame a slightly different gain). Commanded through a shared
`FadeHandle` (atomics, no locks on the audio path): `ramp_to(target, over)`, and it reports its
current position back so the engine can tell when an outgoing sink has gone silent. The ramp
advances per sample consumed, not per wall-clock second — so pause freezes a half-finished
blend exactly where it sits, and resumes it intact, for free. Every source gets wrapped whether
or not a fade is configured (gain 1.0 is a multiply and a check per sample); that keeps one
code path and buys later polish — click-killing micro-fades on stop and seek — for the price
of a `ramp_to` call. Pure unit tests against a counting source; no audio device.

#### C2 — Engine overlap machinery + serve flag ✅ DONE 2026-08-06
The engine gains a prepared-next slot and a retirement queue:

- **Prepare**: at `remaining ≤ fade + margin` (margin generous enough to cover `OPEN_TIMEOUT`
  plus the decode probe), commit the next pick — under shuffle the pick is committed *now*, not
  re-rolled at handover — and open it on a short-lived thread. Never under the state lock: the
  open blocks on the network for up to the timeout, and findings #48–#50 are the map of that
  minefield. The thread hands back the built decoder through a channel; dropping the receiver
  is how a stale prepare gets cancelled (the thread's send fails, the reader drops, its spool
  file deletes itself). A failed open is remembered so the tick doesn't re-open a doomed URL
  every 120 ms; the track's natural end then takes today's advance path, which retries once.
- **Handover**: at `remaining ≤ fade` with a prepared decoder ready — new `Player` on the same
  mixer, wrapped source starting at gain 0 ramping up, old sink commanded down over the same
  window, `self.sink` swapped, the old sink parked in an `outgoing` slot until it empties or
  its deadline passes. Status flips to the incoming track at fade start (file, duration,
  position-from-zero) — the same convention as the streaming players, and the serve wire
  format doesn't change shape.
- **Policies**, each small, all deliberate: no fade when duration is unknown (live transcode);
  effective fade clamps to half the track; loop-one never fades into itself; manual
  next/previous/play/stop/clear cancel the overlap by stopping the outgoing sink at once; seek
  cancels the blend and snaps the survivor to full gain; pause, resume and volume apply to both
  sinks (user volume stays `Player` volume on both — the fade gain lives inside the source, so
  the two never need multiplying together). The visualizer tap follows the handover: the
  outgoing source's tap goes quiet at fade start (a kill switch on `Tapped`), because two
  sources pushing one ring interleave garbage.
- **Serve**: `--crossfade <seconds>` on the serve subcommand, default 0. The legacy `--port N`
  spawn contract can't pass it, which is the point — mStream never sees a behavior change.
- The spool contract loosens by one file: during prepare-plus-overlap the upcoming track spools
  alongside the playing one (A1's "only the playing track has a file" note is amended). The
  startup sweep never cared how many there were.

Device tests ride the existing `#[ignore]` + `wav_bytes` pattern: a two-track queue whose
handover must arrive early and whose status must never report an empty file mid-run.

#### C3 — TUI wiring ✅ DONE 2026-08-06
The TUI keeps its own queue and feeds the engine one URL at a time, so it must say what comes
next: `AudioCmd::PrepareNext`/`ClearNext` (with a `collapse()` rule — a later Play or Stop
makes a pending prepare moot), re-sent when queue edits change the answer. The delicate part is
the cursor: an engine-initiated handover means `status.source` changes with no `TrackEnded`,
and without reconciliation the App still points at the old entry and ignores the *next* end as
stale. A new worker event (`EndWatch` learns that non-empty → different non-empty is a
handover) lets the App advance its cursor through the `play_index` path minus the Play itself —
falling back to a real Play when the queue was edited under a stale prepare. Config:
`[player] crossfade_seconds` (0 default, clamped sane), through `PlayerPrefs::adopt`, delivered
at session start — and the round-trip test that uses `crossfade_seconds` as its example
*unknown* key gets a new example, since the premise expires the moment the key means something.

#### C4 — Polish ✅ DONE 2026-08-06
Each cheap once C1–C3 existed, as promised. A manual skip breathes out over 150 ms instead of
clicking (the leaving sink retires softly through the same outgoing slot as a blend); a stop
gets 80 ms of the same mercy with the bookkeeping clearing instantly; a seek dips — 10 ms down,
jump, 30 ms up — so neither side of the jump clicks. True gapless landed as `[player] gapless`
(and serve `--gapless`), not as `crossfade_seconds = 0`: gapless prefetches, and prefetch is a
behavior the pre-Phase-C engine never showed unasked, so it stays opt-in like the blend. When
on with no crossfade set, the prepared next is appended to the playing sink at APPEND_LEAD
(late, because an append cannot be taken back) and rodio crosses the boundary sample-tight;
the bookkeeping follows at the boundary, and the TUI's HandedOver reconciliation covers both
transitions unchanged. Crossfade and Gapless first landed as Auto-DJ panel rows — playback settings
rather than picking ones, lodging where the adjustable settings then gathered — and moved to
their real home when C5 built one. The round-trip test's future key came true a second time
(`gapless`); `replaygain` carries the torch.

**Costs accepted**: two decoders and two spool files for a bounded window per transition;
summed peaks can transiently exceed full scale on loud masters even at equal power (rodio has a
`limit` source if that ever proves audible in practice).

#### C5 — The Settings tab ✅ DONE 2026-08-06
`6` opens Settings, a real home for the player's own knobs: a menu of groups (one group so
far — Crossfade) drilling into live-value rows. Enter and `→` step a value up, `←` steps it
down (the Auto-DJ panel's own convention for settings rows; Esc and `..` are the ways out),
the details read the values back as they change, and the engine hears every nudge in the
keystroke that made it. The panel's Crossfade/Gapless rows moved here whole, the panel went
back to being about picking, and on a server without Discover the tab slides onto `5` — the
strip numbers by position, and the strip is the truth.

#### C6 — Blend skips, pause fade, gapless by default ✅ DONE 2026-08-06
The two settings worth adding, added: **Blend skips** retires a manually skipped track over a
fixed second (not `crossfade_seconds` — an eight-second blend is lovely at a natural end and
treacle on a keystroke) with the incoming rising through the same window, reusing the drainer
fleet whole; **Pause fade** ramps down before the pause lands and back up as the resume begins,
with the tick performing the landing since nothing in the engine may sleep — the last hard
edges in the transport, both off by default. And **gapless went on by default**: the opt-in
stance guarded a shipped behavior that never shipped (v0.1.2 predates all of Phase C), the
legacy `--port` contract keeps its own defaults, and albums playing as they were cut is the
better first impression. The cost — one track of prefetch near each boundary — is two
keystrokes to decline in Settings. Curve selection and prefetch-lead knobs were considered
and refused: the first is inaudible preference, the second invites the misconfiguration where
blends silently stop firing. The genuinely valuable next steps are written down instead:
don't-blend-album-segues (needs trailing-silence detection) and ReplayGain (its own Settings
group, and the config test's torch key finally come true) — both Phase 7 material.

#### Phase C review ✅ 2026-08-06

A seven-lens adversarial review of C1–C3 (concurrency, audio, state machine, app
reconciliation, edge cases, performance, compat/tests; every finding re-verified by
independent refuters, then a completeness critic over the survivors): 25 raw findings, 19
confirmed, 2 more from the critic. Fixed the same day, each pinned by a test:

- **Pause/resume popped the blend** — the outgoing's wall-clock deadline ran through a pause;
  resume now re-arms it. Found independently by five of the seven lenses.
- **A prepare-thread panic tore the terminal down** under a live TUI (audit #32's failure mode
  on a new thread). Caught at the spawn *and* the panic hook stands back for the thread by
  name — the catch alone was not enough, since the hook runs at the panic site.
- **The fade window ignored the incoming track's length** — a short next track lived its whole
  life at partial gain and its end hard-cut the outgoing. `blend_window` now applies the
  half-track rule to both ends.
- **A push behind the repeat-all wrap was skipped** — linear announcements now re-ask
  `next_index` (deterministic, no dice); shuffle keeps the held roll, which is the point of it.
- **Restarting the playing track silently cost the next blend** — every Play now drops the
  app-side announcement so the refresh re-announces into the engine it just wiped.
- **Duplicate tracks blended invisibly and played thrice** — a track never blends into itself:
  refused app-side at announcement, refused engine-side as the belt.
- **The handover fallback could rewind onto an earlier duplicate** — the scan now prefers rows
  ahead of the cursor.
- **A failed manual jump kept a stale committed pick** while `queue_index` moved; the
  index-moving paths invalidate on error now — including `play_source` and the auto-advance
  arm, two siblings the *fix-verification* pass (a second adversarial round over the fix diff)
  caught after the first round missed them. And **`Failed` was a one-way latch** — a seek
  back past the prepare window resets it, so a healed network blip gets its retry.
- Seven test gaps closed: the new `collapse()` rules, invalidate-on-mutation (device),
  `prepare_next` idempotency (re-announce loop in the device test), the `handle_action`
  refresh funnel, `Faded` under a mid-stream rate change, the `HandedOver` staleness guards,
  and integer `crossfade_seconds` in hand-written TOML.

Deferred with eyes open: keeping a matching in-flight open across queue mutations and reusing
a `Ready` decoder when the blend misses (both are C4-adjacent — the second *is* the gapless
attach path); the remaining per-row URL builds in the fallback scan; and the no-headroom
summation, which stays in Costs above until someone hears it.

#### Phase C listening-session fix ✅ 2026-08-07

The first human listening session found what four adversarial rounds had not: *seeking to a
track's last stretch killed the crossfade* — the track played out and the seam went as a hard
cut. Reproduced end-to-end (serve engine on local WAVs, serve on real demo-server MP3s, and
the actual TUI driven over a pty), all of which **blended correctly** — the failures live at
the edges of the seek itself, three of them:

- **A seek past the end ended the track mid-keystroke.** rodio accepts the position, the
  decoder runs dry, hard advance — and `}` (a minute forward) makes overshoot routine.
  Forward seeks now stop at `seek_ceiling`: duration minus the fade window (or the gapless
  append lead) minus `OPEN_RUNWAY`, only when a transition is configured — with both off,
  past-the-end keeps its legacy skip-the-track meaning. Pinned by
  `a_seek_toward_the_end_stops_short_and_the_blend_still_fires` (device).
- **`Failed` was a latch, not a limit.** One starved open late in the track (the reported
  session rides a Quick Connect tunnel, where a seek's spool catch-up hogs the link and the
  next track's open times out) silenced the seam for the rest of the track. `Failed` now
  carries its timestamp and retries every `FAILED_RETRY` while `remaining > fade +
  OPEN_RUNWAY` — the dead-URL property the latch guarded costs a handful of opens per tail,
  not one per tick. A seek also resets it outright: a user move is fresh runway. Pinned by
  `a_failed_open_gets_its_retry_and_the_blend_still_fires` (device, flaky server).
- **Seek keys re-read a stale status.** Position refreshes ~4×/s, so a quick `}}}` computed
  the same base thrice and moved one minute. The app now chains: each press builds on the
  in-flight target (`seek_goal`, trusted for 2.5s or until status catches up / the source
  changes), capped at the bar's end so nothing banks minutes that don't exist. Pinned by
  `fast_seek_presses_build_on_each_other_not_the_stale_status`.

Considered and kept: `snap_out_of_blend` on seeks (C4's "keep the track being seeked" rule) —
a seek during a running blend still collapses it, by design; the ceiling makes it much harder
to land there by accident.

#### Phase C listening-session fix, round two ✅ 2026-08-07

The first round's fixes verified clean in every reproduction — including the reporter's exact
gesture (a pty-driven TUI, a real mouse click on the bar, a throttled tunnel-shaped link in
both the static and transcode response shapes) — and the bug survived anyway. What settled it
was evidence, not another theory: the TUI silences stderr, so the engine grew a **flight
recorder** (`MSTREAM_ENGINE_TRACE=<file>`, `engine/trace.rs`, one line per transition
decision, off unless named). One reproduced session then read:

```
preparing 04 Angel Palace (41.9s remaining, fade 30.0s)
open FAILED: no answer from the server after 10s
track ran out (next=failed, announced=true)
```

Three causes, each now fixed and pinned:

- **The Quick Connect bridge kept the client-side TCP open after the server side ended** —
  `bridge_one` waited for *both* copy directions. reqwest's pool read the held-open socket as
  a healthy idle connection, offered the corpse to the prepare's open, and the request waited
  out OPEN_TIMEOUT on a stream nothing was answering. Every transition whose prepare fired
  within the pool's ~90s idle window of the previous download finishing went out as a cut —
  which is why long tracks played naturally were fine and *any* seek toward the end was not.
  The bridge now ends when either direction ends (a server FIN reaches the client, as a
  direct connection would), and the engine's streaming client no longer pools at all
  (`pool_max_idle_per_host(0)` — an open per track, a connection per open; pinned by
  `a_second_open_never_reuses_the_first_connection` against a serve-one-then-hold-silent
  server).
- **The round-one retry gate was unsatisfiable under a long fade**: it demanded
  `remaining > fade + OPEN_RUNWAY`, but a 30s fade opens its window 42s out and the first
  open can only fail 32s out — 32 is never greater than 32, so no retry ever ran. The gate
  now asks only whether a retry could still be *heard* (`remaining > OPEN_RUNWAY`);
  blend_window already caps at what remains, and a shortened blend beats a cut. Pinned by
  `a_failed_open_inside_the_window_still_retries` (device, the trace's exact geometry).
- The recorder itself stays: it is how the next report gets read instead of guessed at.

#### Quick Connect tunnel audit ✅ 2026-08-07

Prompted by "API calls regularly fail" from the same listening sessions. Findings, worst
first:

- **The tunnel could never re-dial.** One QUIC connection was dialled at connect and held
  forever; iroh's own transport (5s heartbeats, 15s path idle / 30s relay-path idle) declares
  a connection dead after any real network interruption — a closed lid, a VPN re-auth, WiFi
  wandering — and a dead QUIC connection is one-shot. Every later `open_bi` failed, so every
  API call and every prepare failed for the rest of the session. The bridge now holds the
  pairing code as the standing capability it is (`Redialer`): the first caller to find the
  tunnel dead re-dials for everyone (single-flight behind a Mutex held across the dial),
  the handshake re-proves the secret, and the loopback URL never changes so live sessions
  ride through.
- **`open_bi` had no timeout**: on a half-dead connection it hangs until QUIC gives the path
  up. `STREAM_TIMEOUT` (8s — above the 5s heartbeat, below the 15–30s idle verdicts) now
  bounds it, and a timeout is treated as death → re-dial.
- **One accept error killed the whole bridge** (`Err(_) => break` in the accept loop);
  transient accept failures now log, breathe 250ms, and continue — shutdown stays the only
  exit.
- Round two's bridge fix (either-direction-end closes the client conn) is what fixed the
  API flakiness's steady-state form: the api client's reqwest pool was being handed
  held-open corpses after the server's keep-alive idle closed streams server-side.
  Confirmed benign now on both clients; the engine's additionally never pools.
- Clean bills: pairing-code parsing (four base64 shapes, version gate, secret length),
  handshake bounds (256-byte read limit, 15s timeout, rejection-vs-transport conflation is
  deliberate), eager first dial (bad codes fail at connect, not at first use), loopback-only
  listener, keep-alive defaults (iroh sets them; nothing to add).

The header now also says how a tunnel session is reached — `quick connect · ab12cd ·
direct` / `· relay` / `· reconnecting…` — fed by a sampler thread that lives exactly as
long as the bridge (Weak-held, 2s cadence, change-only events) reading the selected QUIC
path. Relay and direct sound different; the listener deserves to know which one they are
hearing.

**Research round (what other iroh users taught us).** Surveyed the ecosystem — dumbpipe,
sendme, iroh-ssh, pai-sho ("dumbpipe, but it reconnects" — validating the Redialer), Delta
Chat's peer_channels (the one production user at hostile-network scale; their lesson is
`endpoint.network_change()`, which macOS handles natively) — and the iroh source itself.
Two things applied directly to the corporate-network (Netskope) sessions:

- **The relay is plain HTTPS on outbound TCP 443** — precisely what corporate TLS
  inspection intercepts and re-signs. iroh's default trust is a compiled-in Mozilla bundle
  (`CaTlsConfig::EmbeddedWebPki`), which calls the corporate CA an UnknownIssuer and fails
  the relay — the only road a UDP-blocked network has left (iroh #2257 and Dioxus #5564 are
  the same failure in the wild). Now built with the `platform-verifier` feature and
  `CaTlsConfig::system()`: the tunnel trusts what the OS keychain trusts, like every
  browser on the same machine.
- **`proxy_from_env()`**: a network that declares its proxy (HTTP_PROXY/HTTPS_PROXY) gets
  the relay's HTTPS routed through it rather than around it. No-op elsewhere.

Noted, not needed: relay-only transport modes (holepunch attempts are harmless),
`network_change()` nudges (macOS detects natively; the Redialer covers connection death),
custom relay maps (an mStream-hosted relay is a server-side feature first).

And one UI repair from the same weather: **a browse the tunnel ate left its navigation
standing** — path one level deep, a phantom miller column of the unchanged listing beside
the pane, another copy stacked per retry click, and unclosable at the root because Back
refuses to pop with nowhere to step out to. Fixed at both ends: `browse_undo` walks the
path and the pushed column back when the error arrives (the listing handler's path guard
already drops any late success for an undone path), and Back at the root now drains
orphaned trail columns instead of stranding them. Pinned by
`a_failed_browse_takes_its_column_back_with_it` and
`back_at_the_root_drains_an_orphaned_column`.

#### Pre-merge once-over ✅ 2026-08-07

A fresh-eyes pass over everything since the last adversarial round — the seek fixes, the
flight recorder, the tunnel rounds, the marker — before merging `crossfade`. Four findings,
all fixed same-day:

- **The seek ceiling outlived its reason on the last track**: it clamped whenever crossfade
  was configured, even with nothing lined up to follow — on the queue's final track under a
  30s fade, the last 32 seconds were unreachable by seeking, protecting a transition that
  didn't exist. The clamp now also asks whether anything follows (`pending_next` or a queue
  candidate; pick_next is pure, so asking commits nothing). Pinned by
  `the_last_track_seeks_free_of_the_ceiling` (device).
- **Relayed error text could carry `?token=` into the flight recorder**: our own messages
  redact URLs, but "request failed: {e}" interpolates reqwest's text, which prints the full
  URL it failed on. `redact_queries` now scrubs query strings from relayed third-party error
  text (unit-pinned); the price is a few characters of anyone's prose that contains a `?`.
- **Dead-network dial pile-up**: a failed re-dial left the slot empty, and every caller
  queued behind it took its own 25-second turn at a network that just said no.
  `DIAL_COOLDOWN` (4s) makes callers arriving during a dead spell fail fast; the tunnel
  still re-dials on the next request after the cooldown.
- **The tunnel badge could dress a direct session**: the old bridge deliberately outlives a
  server switch (dropping it would cut a session mid-handover), so its sampler kept
  reporting — and a direct-URL session would wear the stale tunnel's `· relay`. TunnelPath
  events are now refused unless the current session is a tunnel (test updated to pin both
  directions).

Noted without code: a seek within the 150ms soft-pause ramp briefly restores volume before
the pause lands (cosmetic, needs a deliberate ~100ms gesture); a second folder click during
an unanswered browse can leave path one level shallower than the pane until the next
navigation (self-heals; both failures' undos fire); the flight-recorder file grows without
bound across sessions (it is a debug facility you opt into per-run).

#### D1 — Sonic Path takes the library's slot ✅ 2026-08-07

Auto-DJ had two homes: the modal behind `D`, reachable from the browser, and a read-only
summary on the full-screen view's Auto-DJ tab that pointed at it. Sonic Journey (B2) was a
third overlay plotting the same arc the webapp gives a whole panel to. Both moved to where
the thing they describe already lives, and the guiding rule was **every screen behind a
number** — nothing worth finding hidden behind a letter you would have to already know.

- **Auto-DJ is now the tab, not a modal over it.** The summary became the panel: `↑↓` walk
  the rows, `←→` adjust, `Enter` sets, and the sample is a row rather than a key (`p` is
  "previous track" in that view and the overlay used to steal it). The one tab whose rows are
  values, so it claims `←→` — `Tab` / `Shift+Tab` switch tabs there, which is why
  `Action::NowLeft`/`NowRight` exist separately from `NowTabPrev`/`NowTabNext`.
- **The modal was invisible where it mattered.** `render()` returns early in fullscreen, so
  `D` inside the now-playing view opened a panel that never drew and ate the keyboard. Gone
  with the modal; the genre chooser (the one overlay left) is now drawn in both branches.
- **Sonic Path is a tab — `6`, beside Discover, gated on `discoveryPath`.** Its rows are
  ordinary `Entry` values, so the tab costs no new navigation: `↑↓`, `Enter`, `a`, the
  filter, the queue column and the trail all keep meaning what they mean, and the stops come
  back as real `Entry::Track` rows. `Enter` on a stop plays the path from there; `a` queues
  just that stop.
- **The webapp's capture flow, keyed.** "Pick from library" arms `App::sonic_capture` and
  drops the user in the file browser with a banner along the foot; the next track they open
  *anywhere* fills the field instead of playing. It lives on the App rather than on
  `SonicPath` for the same reason `VUEPLAYERCORE.songCapture` does — it is answered from
  whichever tab they wander into.
- **Length is still a replot, and now says so.** The old overlay refetched on every `←→`;
  the tab moves the slider locally and leaves the asking to Build / Regenerate, which is
  both the webapp's shape and one request instead of twenty.
- **Full parity, including save-as-playlist** — `POST /api/v1/playlist/save`, a new
  `ApiCmd::SavePlaylist`, and a name prompt pre-filled with `start → end`.
- `J` survives as the shortcut in: it opens the tab aimed at the highlighted track, with
  whatever is playing as the start, and plots straight away. Two presses are no longer a
  thing — the tab is where the second end is chosen.

Verified live against demo.mstream.io through `replay --live`: the tab strip numbers to
`7:Settings`, arming from an empty Start field walked into the browser and came back with
`ALM - 3rd Dimension` in the field, and a 14-stop path plotted, listed its arc positions and
offered Play / Queue all / Save as playlist / Regenerate / Start over.

#### D2 — The waveform bar ✅ 2026-08-08

First item off the terminal-player audit (cmus, ncmpcpp, musikcube, termusic,
rmpc, and the closer analogues jellyfin-tui and sonic-tui). Nothing in that
survey draws a waveform under the progress bar; mStream has had
`GET /api/v1/db/waveform` all along and no client here ever called it.

800 peak magnitudes per track, whatever its length. The bar keeps its three
channels — accent behind the playhead, dim ahead, marker under the pointer —
and only the glyph changes, so it goes on being the same control whether or
not a shape arrived.

- **No ping flag, so probe once and latch.** Waveforms are the only optional
  feature the ping does not advertise, so "no flag, no probe" has nothing to
  read. `Client::no_waveforms` is the next best thing, and the same shape as
  `plain_listings`: a 503 means the *server* has no ffmpeg, which is a fact
  about the server, so the session costs one wasted request rather than one
  per track. A 500 is ffmpeg's settled verdict on that content (the server
  writes a failure marker), and a 404 covers both "not scanned" and "lives
  on a federated peer" — `federation-stream.js` puts waveforms out of scope
  deliberately. All four fold into `None`, and `None` draws the plain bar.
- **Energy, not peaks — measured, not reasoned.** The scoping said peak, on
  the grounds that the server bins by peak-of-peaks. Live against
  demo.mstream.io that drew **83 of 86 columns at full height**: peak over
  the ~10 bars sharing a column asks "was anything loud in these three
  seconds", and on a modern master it is yes everywhere. RMS asks how much
  energy was in them, which is the question whose answer is the shape.
- **Then stretched onto the band the track uses.** RMS alone still drew a
  brick: a dense body sits between 230 and 250, and eight heights cannot
  show a twenty-wide band. Mapping [10th percentile, loudest] onto the full
  height spends all eight glyphs where the variation is. The floor is a
  percentile rather than the minimum because nearly every track fades in and
  out, and one near-silent column at each end would anchor the bottom at
  zero and undo the stretch. The honest cost: this shows shape, not level.
- **Prefetched, and deliberately not off the announcement.** A cold waveform
  costs an ffmpeg decode, so one fetched when the track starts can arrive
  well into it. The obvious hook was `announced` — but that only exists when
  a blend or gapless seam needs it, and someone with crossfade off is
  exactly who would notice a late bar. So it falls back to the plain next
  row, except under shuffle, where the pick re-rolls on every call and
  prefetching would fetch the library one dispatch at a time.

- **Mirrored in the full-screen view.** One row of block elements is eight
  heights, which reads as a bar chart; the shape needs a centre line to grow
  out of. The full-screen band is now two rows — the half above drawn with
  ordinary lower blocks (the centre is the bottom of that row, where a lower
  block already starts), the half below drawn with the *same* glyph
  `REVERSED`, since Unicode has one-eighth, one-half and full for the upward
  direction and nothing between. Reversing swaps which part of the cell is
  inked without this code ever knowing the terminal's background. The whole
  band is the click target: `progress_area` grew a row and the handler was
  already row-agnostic, so it is a bigger target and nothing to special-case.
  The compact transport keeps its single row — those two rows are an eighth
  of the list on an 80x24 terminal.
- **Considered and deferred: `ratatui-image`.** Real pixel resolution via
  Sixel/Kitty/iTerm2 would beat eight heights, but two things rule it out
  *here*: the crate's compatibility matrix has no Windows row at all
  (conhost has no graphics protocol; the half-block fallback is two levels,
  worse than what we have), and the played/unplayed split moves continuously,
  so a raster bar means re-encoding a Sixel blob every frame — the crate
  itself says encoding is blocking and should be offloaded. It is designed
  for images that are drawn once. Kept on the audit list for **album art**
  in the now-playing pane, which is exactly that.

Verified live: intro ramp, body variation and outro fade all legible on two
dense hip-hop masters, on both the browser transport and the full-screen
gauge. Pinned by `the_waveform_shows_energy_rather_than_saturating_on_peaks`,
`the_full_screen_band_mirrors_the_shape_above_the_scrubber`,
`the_waveform_decorates_the_bar_without_taking_it_over`,
`the_hover_marker_survives_the_waveform`, and three app-level tests for the
one-ask-per-track rule and the prefetch.

#### D3 — Glyph sets, and what a console font actually has ✅ 2026-08-08

Reported live: the waveform rendered as `?` boxes. Not a codepage problem —
the console was already UTF-8. **Consolas has no glyphs for most of what the
UI draws.** Probed rather than recalled, by walking every non-ASCII character
in `src/tui` and `src/api` against the font's `CharacterToGlyphMap`:

| Missing from Consolas | Used by |
|---|---|
| `▁▂▃▅▆▇` (six of eight) | the waveform |
| `▏` | the hover marker, the filter caret |
| `▶` `◆` `⏸` | queue marker, Sonic Path ends, transport |
| braille `U+2800`+ | the spinner, and the whole visualizer |

Present: `█ ░ ▒ ▓ ▄ ─ │ ■` — the CP437 set, which is exactly why the bar
looked right until it learned to draw a waveform. It only ever used `█` and
`░`. Cascadia Mono (Windows Terminal's default) has everything; so does every
ordinary Linux and macOS terminal font.

`Glyphs` resolves once at startup, the same shape as `Theme`, from
`[display] glyphs = auto | full | legacy`. `auto` reads `WT_SESSION` —
Windows Terminal sets it, conhost does not — and assumes anything not Windows
is fine, which covers the browser build too.

- **The fallback is density, not height, and that was forced.** The first
  attempt quantised the eighths onto the three heights CP437 has (empty, `▄`,
  `█`). It drew a flat wall of `▄`, and the reason is arithmetic rather than
  taste: a mirrored half is drawn by indexing the complement and reversing,
  so it needs `ink(n) + ink(8-n)` to come to a full cell — and with only
  `{0, 4, 8}` available the only quantiser satisfying that sends everything
  between to 4. Density has four steps (`░▒▓█`), needs no symmetry, and fits
  the single row the transport had before waveforms existed. `Glyphs::
  mirrored` carries that, and the full-screen band claims no row for a half
  it cannot draw.
- **The tests now run both sets.** They assert against `glyphs()` rather than
  literals, so a Linux runner exercises the rich path and a Windows one
  exercises the fallback — which is how this was caught in the first place.
  The symmetry invariant is pinned for any set claiming to mirror, and the
  legacy set is checked against a hard-coded CP437 list.

**Still outstanding:** the visualizer is braille sub-cell plotting with no
fallback, so a legacy console still shows boxes there. Deliberately deferred
— the canvas is built on braille and a block-based renderer is its own piece
of work.

#### D4 — Numbers all the way down ✅ 2026-08-08

Reported live: "once the user goes to Auto-DJ, they can't go back."

`Tab` did work, and the footer said so. That is not the same as it being
findable — and the shape of the trap is the lesson. D1 gave the full-screen
view `←→` for navigation *except* on Auto-DJ, whose rows are values and
wanted the arrows for adjustment. So the one tab a user could get stuck on
was the one tab whose way out was different from every other, and the
exception fired exactly where the rule was most needed.

`1`–`5` now pick a tab in the full-screen view, indexing the *visible* ones
the way the browser's numbers do, so they stay 1..n with no gaps whatever
this track and this server allow. The strip wears the numbers — a key nobody
can see is a key nobody presses — and keeps them in the narrow fallback,
where the way out matters most. `←→` are handed to whichever tab is in
front: Auto-DJ adjusts with them, nothing else wants them. `Tab` and
`Shift+Tab` stay for anyone who would rather not count.

The digits already did nothing useful here: outside the view they pick
browser tabs, and in fullscreen that changed a screen nobody could see.

Two general points banked from this, since the same shape will recur:

- **A key that means one thing everywhere beats a key that means the right
  thing four times out of five.** The arrows were locally better on four
  tabs and catastrophic on the fifth, and the fifth is where the user was.
- **Discoverability is a rendering problem as much as a binding one.** The
  fix was not only rebinding but putting the number on the tab.

#### D5 — Miller columns take the width they are given ✅ 2026-08-08

Reported live: browsing by artist on a wide terminal, the first column fell
off after a couple of levels with plenty of room left. `column_widths` had a
`TRAIL_MAX: usize = 2` that no amount of width could move.

The count is now `[display] miller_columns` (default 4, counting the column
you are in) and it is a *ceiling*, not a promise — width still decides, the
trail still fills innermost-first, and the current column still keeps its
floor. The queue is deliberately outside the count: it is the end of the
chain rather than a step along it.

Two things came out of looking at the result rather than the diff:

- **Surplus width had nowhere to go but the current column.** At 160 columns
  that made a 100-wide list of tracks beside three 20-wide context columns
  clipping every album name to "Stim Pack Volume o". Half the surplus now
  widens the trail (to a 32 ceiling — past that it is spending width on a
  listing nobody is reading) and the rest stays with the column being read.
- **Three `[display]` knobs was one OnceLock too many.** `mirror_min_height`
  and `miller_columns` folded into a `Sizing` value set once at startup,
  beside `Theme` and `Glyphs`. The layout has to be a pure function of the
  area plus settings that cannot move mid-frame — `progress_area` re-derives
  it after the draw to answer a click — so "resolved once at startup" is
  load-bearing rather than tidiness.

#### D6 — Playlists move into the Library ✅ 2026-08-08

A tab of its own, for something that is a way of cutting the library like
artists or genres are. It came with a parallel copy of machinery the Library
tab already had: its own `Pane`, its own `playlist_open` cursor doing by hand
what `Drill::wants` does for every other tab, two `ApiCmd`s, two `Event`s and
an `Entry::Playlist` variant.

They are now two `LibraryNode`s — `Playlists` and `Playlist(name)` — and the
rows are ordinary `Entry::Node`s, so the drill, the trail columns, the
stale-reply guard, the spinner and the Miller columns all apply without a
line of their own. Net: **six tabs instead of seven**, and a `Pane`, a
cursor, two commands, two events and an `Entry` variant deleted.

Two things fell out of the move rather than being designed in:

- **The list refreshes now.** The tab cached its playlists and only fetched
  on first visit, so one made anywhere else never appeared. A library node is
  asked for on the way in like every other.
- **`step_out`'s catch-all became unreachable.** Playlists was the last tab
  not handled explicitly, so the match is exhaustive and the compiler now
  checks it — the audit-#59 property, arrived at by subtraction.

`Tab::ALL` is `[Files, Library, Search, Discover, SonicPath, Settings]`, and
the digits move with it: Search is 3, Discover 4, Sonic Path 5, Settings 6.

#### D7 — The now-playing Discover panel ✅ 2026-08-08

The second of the two `"not wired up yet"` placeholders. It shows the
server's nearest neighbours to whatever is on the speakers, in order, and
re-asks when the track changes.

- **A destination on the command, not a second command.** Both surfaces ask
  for `DiscoverNode::Tracks` about different seeds, so their replies had to
  be told apart. `ApiCmd::Discover` and `Event::Discover` carry a
  `DiscoverDest` — exactly what audit #64 asks for, against the alternative
  of a variant whose only job is to be a different name.
- **And a seed on the reply.** The browser tab tells a stale answer by its
  node, because walking around changes the node. This panel's node never
  changes — it is always "tracks like the seed" — so the seed is the only
  thing that can say which track an answer is about. Without it, a reply for
  the track that just ended would land under the name of the one now
  playing.
- **The refresh rides the dispatch funnel**, like the waveform prefetch:
  "the track changed" and "the tab opened" are two events with one answer,
  and a panel that asks whenever what it holds disagrees with what is
  sounding cannot be left describing the wrong song. It asks nothing while
  the tab is not the one being looked at.
- **`Enter` queues and plays; `a` queues.** Neither replaces the queue,
  which is what the browser's `Enter` on a track row does — reasonable while
  browsing, wrong in the middle of listening to the thing you are getting
  recommendations from.

Lyrics is now the last placeholder on that strip.

#### D8 — Discover asks what to look around from ✅ 2026-08-08

D7 made the full-screen panel follow the speakers, which answers one
question well and another not at all: **what does *this* track sound like,
without playing it?** The browser's Discover tab could not answer it either
— it anchored on whatever was highlighted when you opened it, which meant
the seed was something you never said out loud and could not change. So the
tab grew the step in front:

```
Discover · look around from…   →  Songs / Artists  →  the list
  What's playing                                       83%  ALM - Manor
  Choose a song…                                       81%  ALM - Hip Hop Factory
```

**The two Discover views are different things, and the split is the point.**
The panel is glanced at while music plays: one question, re-asked when the
answer would change, no steering. The tab is somewhere you *go* to look
something up, so it asks. Putting the drill on the panel — which is where it
was first built — made the glance into a menu and still left the tab
guessing.

- **The seed is resolved when it is picked, not read live.** "What's
  playing" means the track that *was* playing when you said so. A list that
  re-aimed itself every time a song ended is exactly what the panel is, and
  that is unusable for looking something up.
- **`Choose a song…` reuses the Sonic Path capture**, which is what turned
  two armed-picker fields into one `Capture` enum carrying who is waiting.
  Arming also drops out of the full-screen view — the picking happens in the
  browser, and arming from fullscreen left the user choosing from a listing
  they could not see.
- **Nothing seeds the tab implicitly any more.** `set_discover_seed` took
  the cursor's track on every tab switch and on every step back to the root;
  both are gone. `discover_seed` now only ever holds what a row named, so
  the title can be trusted.
- **Percentages instead of ranks.** The rows arrive in order, so a number
  counting them says nothing their position doesn't. The cosine says how
  much of a neighbour each one actually is — and it was being thrown away:
  `DiscoverData::Tracks` carried `Vec<Track>`, built by `SimilarTrack::
  into_track()`, which drops the similarity. It carries `Vec<SimilarTrack>`
  now, and both views show the number.
- **The stale guard needed both halves, on both arms.** Seed *and* node: the
  two lists are asked for separately about the same seed, so an artists
  reply arriving after the user asked for songs is as wrong as a reply about
  another track — and "similar tracks" is the same node whatever it is
  similar *to*, so the node alone cannot catch a re-seeded tab. The panel's
  arm had both from the start; the browser's was written with `..` and
  guarded on the node until the review found it, which is exactly the shape
  of bug a doc claiming otherwise helps hide.
#### Logging round ✅ 2026-08-07 (branch `logging`)

The four improvements the post-release overview named, in one coherent shape:

- **`stderrln!` tees into the flight recorder.** The TUI's silence rule stands — the screen is
  never smeared — but the silenced lines (tunnel re-dials, accept failures, open errors) now
  land in `MSTREAM_ENGINE_TRACE` when it is on, instead of vanishing with the alternate screen.
- **`MSTREAM_LOG` installs a `tracing` subscriber** (new `logging.rs`, tracing-subscriber with
  env-filter): everything iroh, reqwest, hyper and stream-download narrate — relay connects,
  holepunch attempts, path upgrades, reconnects — written to a file, never the terminal, filtered
  by `RUST_LOG` (default `info`). The blindness that cost a day of tunnel archaeology, ended.
- **A standard location with rotation**: `MSTREAM_LOG=1` means `<cache>/logs/mstream-player.log`
  with the last four runs kept beside it (`.1`–`.4`, logrotate-shift, no date dependency);
  an explicit path means exactly that file. `logging to …` prints at boot, and the TUI says it
  again at quit once the terminal is a terminal — both moments a person can actually read.
- **Recorder hygiene**: the trace file truncates at start and opens with a version header —
  one run per file, bounded even when the variable lives in a shell profile.

Kept deliberately: two files, two questions. `MSTREAM_LOG` answers "what did the network do",
the recorder answers "what did the player decide" — merging them would bury the forty decision
lines under ten thousand of hyper's.

#### PR #5 review round ✅ 2026-08-08 (branch `logs`)

Seven review lenses over the diagnostics diff, each finding put to two skeptics, then a
completeness critic: 39 raw findings, 20 survived, 13 distinct after dedup. Six fixed here — the
four that blocked, plus the two the review called out as merge-blocking in their own right:

- **The auth token was captured.** Our own lines were redacted, but stream-download names the URL
  it is fetching in a span field and reqwest logs request URLs at debug — and an mStream media
  URL carries `?token=<jwt>`. It reached the ring, the viewer and the file the README invites
  people to attach to bug reports. Scrubbing now happens in the writer, where both destinations
  meet (query strings and URL userinfo → `<redacted>`), and the file is created `0600`. Pinned by
  a unit test on the exact leaked shape and a live trace-level smoke that greps for the secret.
- **A paused player flooded the ring.** `etrace!` became an always-on tracing event, and two
  `crossfade_step` gates log on every tick — so a pause near a boundary wrote ~8 lines a second
  and evicted the whole 2 000-line ring in four minutes, destroying the session the feature
  exists to hold. The gates are edge-triggered now (`State::gate_noted`): once per stretch, and
  the next closure still speaks. Pinned by a smoke that pauses for 20 s and counts one line.
- **Re-enabling Write log truncated the file.** `File::create` on a path the session had already
  written destroyed everything past the ring. Opening now distinguishes `Fresh` from `Resume`:
  a resume appends, and pours only the stretch captured while writing was off, tracked by a
  monotonic line counter on the ring. Pinned by unit tests and a UI toggle smoke.
- **The log viewer panicked in the browser build.** `std::time::Instant::now()` panics on
  wasm32-unknown-unknown — the reason `crate::clock` exists — and `cargo check` cannot see it.
  All shared sites now use the shim, including a pre-existing one in `seek_goal`.
- **One `log_touched` flag persisted both switches**, so walking the level in a session started
  by `MSTREAM_LOG=1` also wrote `write = true` and turned on permanent disk logging. Split.
- **The flight recorder had lost `O_APPEND`** in the earlier hygiene change: two players sharing
  one `MSTREAM_ENGINE_TRACE` truncated each other and wrote at their own offsets, NUL-filling the
  overlap. It appends and signs each run again. Pinned by a two-player smoke (2 banners, 0 NULs).

The five follow-ups, closed the same day:

- **No size cap on a long session's file.** The sink now carries its own path and byte count, so
  it can roll itself at 8 MiB to a `.1` sibling and continue — bounded without reaching for a
  second lock (the writer's order stays RING, then SINK).
- **One-shot subcommands disturbed the session log.** `init` takes a `Run`: a config `write =
  true` is the player's own setting and no longer applies to `keys` or `ls`, which used to sweep
  and rotate the directory on every invocation. An explicit `MSTREAM_LOG` still works anywhere.
- **Cross-process rotation races.** The rename chain is gone. Every run writes
  `mstream-player-<pid>.log` and the directory is bounded by *sweeping* the oldest instead —
  skipping anything touched in the last ten minutes, so a running player's file is never a
  candidate. Nobody renames anybody.
- **The ring's memory ceiling** is now real: a byte budget (2 MiB) beside the line count, since
  2 000 lines at the 64 KiB line cap was 128 MiB, not the "few hundred kilobytes" the comment
  claimed.
- **Tests racing on the global ring**: the tail test works on a local `Ring` now, so the app
  tests' "nothing captured yet" assertion is no longer a coin toss under parallel execution.

#### The ring: a log you can read without keeping ✅ 2026-08-07 (branch `logs`)

"Is there any way to view logs without writing them?" — there is now. Every event the level
admits goes into a bounded in-memory ring (2 000 lines, whole-line assembly since the fmt layer
hands an event over in pieces, oldest dropped, a 64 KiB guard on a line that never ends), and
**View log** reads the ring rather than the file. So a session can be inspected at any moment
with nothing on disk, and the viewer's title says which it is — a path, or "in memory · not
written to disk".

Two consequences worth naming. The filter no longer consults the write switch: the level is what
gets *captured*, not merely what gets persisted, which is what makes viewing-without-writing
possible at all. And turning Write log on now pours the ring into the file first (`── N lines
captured before writing began ──`), because the reason anyone turns writing on is the thing they
just watched happen; without it the file would begin at the keystroke and miss exactly that.

The ring also exposed a product gap: at the default `info` level the dependencies are nearly
silent (iroh and reqwest keep their detail at debug and trace), so a default session had nothing
to show. The engine's flight-recorder lines are now emitted as `tracing` events too
(`target: "mstream"`), which puts the player's own voice — plays, announcements, seeks and their
clamps, prepares, open failures, handovers, tunnel re-dials — in the ring at info. The default
log is now the story of what the player decided, and raising the level adds what the network was
doing underneath.

#### Logs in Settings, split in two ✅ 2026-08-07 (branch `logging`)

Follow-up shaped by use: one row conflated whether logs are written with how loud they are.
Now **Write log** (bool, default off) owns the file, and **Log level** (info · debug · trace,
default info) owns the loudness — a write-time threshold, adjustable live while writing and
waiting patiently while not. Old one-field configs keep their meaning (`level = "trace"`
alone still means writing at trace; `"off"` means off — `settled_from_config`, unit-pinned),
both switches persist through both save paths, and the viewer shows whatever was captured
even after the switch goes back off. The always-installed-subscriber architecture is what
made the split free. Re-proved live in the pty drive: switch on → toast, level to trace →
26 KB in two seconds, viewer, quit, `write = true` and `level = "trace"` in config.

#### Logs in Settings ✅ 2026-08-07 (branch `logging`)

The mobile app's debugging room, grown here: **Settings → Logs** holds a **Log level** row
(off · info · debug · trace, the same ←→ grammar as Blend length) and a **View log** row that
opens the tail of the file inside the player — modal, `j`/`k` scrolling, `G` following the end
on a one-second refresh, `q` out. Turning the level up mid-session works because the round
before this one left the subscriber always installed behind a reloadable filter and a
late-binding writer: the first step up opens the rotated default file on the spot, and a
chosen level (only a *chosen* one — environment-forced sessions don't count) persists as
`[log] level` in config.toml through both save paths. Sessions with no subscriber (unit
tests, embedding) degrade honestly: the row reports that no file could open and snaps back
to off. Pinned by the settings-room app test, the config round-trip, and a pty drive of the
real TUI: level to trace, 26 KB of telemetry inside two seconds, viewer in and out, goodbye
line, config remembering `trace`.

### Phase 5 — Release & install ✅ DONE 2026-08-06 (v0.1.0 → v0.1.2)
Tag-driven releases (binaries + `manifest.json` with per-file sha256) and the README install
matrix, then the "later" items inside the same three days: one-line installers for sh and
PowerShell, a Homebrew tap the release workflow bumps itself, a scoop bucket, `.deb`/`.rpm`
that declare the ALSA dependency, and darwin binaries that leave signed and notarized.

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
The gapless/prefetch thread is gone from this list entirely: prefetch and crossfade moved
forward to Phase C, and true gapless — the append-to-sink redesign — landed as C4.
A persistent track cache (replay without re-downloading, offline listening) is the step after
that and a genuinely bigger one: eviction policy, a size budget, an index keyed by server +
filepath — this is where the SQLite question from A1 returns with an actual job to do. Also:
TUI as remote for server-side audio, media keys (MPRIS/SMTC), scrobbling hooks, AUR packaging,
don't-blend-album-segues (trailing-silence detection so continuous mixes keep their seams), and
ReplayGain (the crossfade flaw you can actually hear — lopsided blends between loud and quiet
masters — and a Settings group of its own).
Two entries left this list by other roads: album art was written down as "ratatui-image" and
landed instead as the half-block canvas the Cover visualizer draws through (0.1.1), and
brew/scoop shipped with Phase 5.

### Phase 8 — One terminal everywhere (post PR #10)

> **Status 2026-08-25: the macOS slice is implemented — mStream PR #911.**
> Ghostty 1.3.1 pinned (bin/ghostty/manifest.json: dmg sha + TeamIdentifier
> asserted), staged untouched at console/ beside mStream.app, launcher
> prefers it via generated config (never `-e`; `command = shell:…` so
> /bin/sh -c absorbs the "Application Support" spaces — a wrinkle the
> original plan missed; `quit-after-last-window-closed` so no windowless
> Dock ghost; `macos-icon = custom` → mStream.icns). Live-proven from the
> staged bundle end to end. Known v1 limits: the .pkg payload ships only
> mStream.app (pkg installs fall back to Terminal.app), and the win-icon /
> linux-.desktop branding riders are a separate small PR. Below stands as
> the design record.

The setup wizard and QR page (PR #10) degrade gracefully, but the probes that built the
ladder also mapped the ceiling: Apple's Terminal has NO pixel protocol (the kitty query is
reflected into the screen as literal text, DA1 has no sixel bit), no OSC 22, tofu
sextants/octants, and an invisible-hold mouse dialect — the weakest surface is also the mac
default. The fix is not more degradation; it is controlling the terminal on the platforms
where we ship a GUI at all. Feasibility is PROVEN, not projected: the unchanged
`mstream-player qr` binary inside Ghostty 1.3.1 draws the pairing code as a real scaled
pixel image (kitty graphics), live-demoed 2026-08-24.

**macOS — bundle Ghostty.** MIT license, 33.8 MB dmg, mac-native, signed + notarized by its
own team (TeamIdentifier 24VZTF6M5V), 60k stars and commits daily. Rivals disqualified:
Alacritty has no image protocol at all, kitty is GPLv3 + a Python runtime, WezTerm is ~90 MB
with slowed development.
- Pin by version + sha256 in a committed manifest, fetched at build time — the mStream
  repo's `bin/p2p-sidecar/manifest.json` pattern verbatim. Release host:
  `https://release.files.ghostty.org/<ver>/Ghostty.dmg`.
- Place `Ghostty.app` BESIDE `mStream.app` in the versioned bundle dir
  (`mStream-<ver>/console/`), never nested inside it: both notarization seals stay
  independent, and their app ships byte-identical (modifying it would void its ticket).
- The tray launcher grows Setup / Terminal Player items that spawn the bundled
  `Ghostty.app/Contents/MacOS/ghostty --config-file=<ours>`.
- **The command MUST come from the shipped config file, never `-e`** (tested, 2026-08-24):
  Ghostty confirms commands passed as launch arguments with an "Allow Ghostty to execute…"
  dialog — its anti-injection guard against LaunchServices — but treats config-declared
  commands as user-trusted and prompts for nothing. Shipped config: `command = <bundled
  player path + args>`, `title = mStream`, `window-width/height`,
  `confirm-close-surface = false`, and `auto-update = off` so the version stays ours (two
  config files if Setup and Player want different commands).
- **Dock icon = the mStream logo, also config-only** (proven live 2026-08-25 on the pristine
  signed app): `macos-icon = custom` + `macos-custom-icon = <abs path to our .icns>` sets
  NSApplication.icon at runtime — Dock and app switcher show the mStream mark, no bundle
  surgery, both notarization seals untouched. Ghostty's own caveats, both moot for us: the
  Finder icon of Ghostty.app stays the ghost (hardcoded in the signed bundle — ours is
  buried in `console/`, nobody browses it), and the Sparkle update dialog would show the
  ghost (`auto-update = off` means it never appears). The menu-bar NAME still reads
  "Ghostty"; if full naming ever matters, the rebrand tier (icns + CFBundleName + new
  bundle id + re-sign with our Developer ID, MIT-permitted) was also demoed working —
  note the icns swap alone is NOT enough, `CFBundleIconName`/Assets.car and the runtime
  icon setting both override it, which is exactly why the config route wins.
- Accepted first-run noise: macOS shows a one-time "Dock Tile Extension Added" notice for
  Ghostty's dock-tile plugin. Not removable without modifying their bundle; the escalation
  (strip + re-sign under our own identity, MIT permits) is documented, not v1.

**Windows — no Ghostty exists (macOS + Linux only), and none is needed.** Windows Terminal,
the default console on Win11, has shipped sixel since 1.22 — a protocol ratatui-image
already speaks. Launch the player through `wt.exe` when present (always Win11, common Store
install on Win10) from the installer's shortcuts and the tray; legacy conhost falls back to
the character ladder. Validation item for the next Windows smoke: confirm the graphics
probe lands on sixel there (DA1 advertisement).

**Linux — never bundle.** The Ghostty team ships no official Linux binaries (package
manager or source only), a build drags GTK4/libadwaita across distros, and Linux installs
skew headless/docker/SSH where a bundled GUI terminal cannot reach. When launching from a
GUI context, prefer a detected capable terminal (ghostty, kitty, wezterm, foot) and fall
back to the ladder + `v`-picture everywhere else.

**Console branding — the icon matrix** (assessed 2026-08-25; the principle: the icon
follows whoever owns the window, so brand what we own and don't fight what we don't).
- **macOS**: solved config-only via the bundled Ghostty (the dock-icon bullet above);
  Apple's Terminal.app on the pre-Phase-8 path keeps its own icon — not changeable, not
  fought.
- **Windows, player exe icon — do this regardless of terminals.** The player binary
  carries no icon resource, so Explorer, shortcuts, pinned taskbar entries AND the conhost
  fallback window (a classic console shows the launched exe's embedded icon) are all
  generic today. Two-step fix: (1) upstream, THIS repo — embed `mstream-logo-cut.ico` at
  link time via the `winresource` crate, so the released bytes carry the icon everywhere
  (next release, 0.5.0) — **REVIVED same day (operator decision): shipped via build.rs +
  winresource in this repo**, which also makes the mStream-side icon stamp below
  redundant for the player exe; (2) immediately, mStream repo — extend the bundler's existing
  resedit VersionInfo stamp (`scripts/win-versioninfo.mjs`, already runs post-sha-verify
  by design) with an icon group, so v0.4.x-pin bundles get it now. Rides in the branding
  riders PR (the mStream-side stamp fully covers bundle users on its own).
- **Windows Terminal taskbar: not possible, accepted.** WT is a packaged Microsoft app —
  taskbar identity is its own, no custom-icon config (the Ghostty contrast). A profile
  fragment could brand the TAB (icon + "mStream Setup" title) but means writing
  persistent config into the user's WT (`Fragments/` dir) for tab-only branding —
  declined unless someone feels strongly.
- **Linux: no reliable per-window icon, accepted.** The dock icon follows the emulator's
  desktop identity; the WM_CLASS + shipped-.desktop trick dies on the most common target
  (modern gnome-terminal is a single factory process — every window is
  `org.gnome.Terminal` no matter what's asked). Brand the LAUNCH POINT instead: the
  deb/rpm desktop packaging ships a "Set up mStream" `.desktop` entry (`Icon=` ours,
  `Terminal=true`) so the menu carries the logo even though the running window doesn't.
- **All platforms, cheap universal win**: the wizard sets the terminal title to "mStream
  Setup" via OSC 0/2 (this repo; the kit already does OSC ground-leasing, same pattern).
  Every popped-up window is at least NAMED ours, whatever its icon. **REVIVED same day
  (operator decision) and shipped**: run_tui claims a WindowTitle guard (the player's
  existing XTWINOPS 22/23 + OSC discipline, factored into tui::WindowTitle) — "mStream
  Setup" for setup, "mStream Quick Connect" for the standalone qr page, restored on exit
  and on panic.

Long game, noted not planned: `libghostty` — the embeddable zero-dependency terminal core —
is what "a terminal inside our own binary" would actually mean one day: an mStream console
app hosting the player directly, no third-party .app at all. **Pinned (assessed
2026-08-25):** works-in-principle on all three platforms (the core is toolkit-agnostic;
their GTK app is just another embedder), but revisit only on a trigger — libghostty ships
a stable versioned C API; the bundled-Ghostty approach starts hurting (size, upstream
config churn, notarization friction); or the library gains Windows rendering (the biggest
unlock — it would replace the wt.exe dependency). Linux is last in line either way:
headless installs need no shell, and desktop users mostly hold capable terminals already —
the only upgraded population is stock GNOME Terminal users, who still get a working
character-art wizard today. Also pinned: the wizard's scrollbar-hold soft-capture
refinement for Apple Terminal — mac users now get the bundled console instead, which
mooted the surface it was polishing.

### Phase 9 — Ship the wizard (merge PR #10 → release → mStream integration)

The order is fixed by the machinery: the mStream server fetches this
repo's binaries through a sha-pinned manifest (`bin/mstream-player/
manifest.json`, currently `v0.3.0`), and that manifest only moves when a
RELEASE exists — so it's merge, then tag, then integrate.

**9a — Merge PR #10.** (1–3 ✅ 2026-08-25; merge = operator)
1. ✅ Move the e2e harness INTO the repo (`test/e2e/`): `fake_mstream.py`,
   `replay.py`, `check_scan.py`, and the `.exp` legs (drive en/de,
   reopen, qr ×3, skip, rename, scan, spawn-stub) plus a runner script.
   Today they live only in the session scratchpad — the /tmp reaper
   already ate them once, and CI can't run what isn't committed.
2. ✅ (run 32873190145: test ubuntu+windows, wasm, e2e ubuntu+macos all green) The whole branch was `[skip ci]` by design (fast-loop policy). Before
   merging: full local battery (unit + wasm + clippy + every e2e leg),
   then one NON-skip push and watch every CI leg go green — the first
   real CI run this branch gets.
3. ✅ Pre-merge once-over ritual (the Phase C/D precedent): PLAN.md phase
   statuses, docs/ui-kit.md ↔ canvas ↔ code agreement, kill dead code,
   read the diff top to bottom once.
4. Merge (operator action). Prefer a merge commit — the branch's commits
   are structured and each carries its verification story.

**9b — Release v0.4.0.** (tagged 2026-08-25)
1. On master: bump `Cargo.toml` 0.3.0 → 0.4.0 (the wizard, the kit,
   i18n, the QR pages — a feature release), note it in PLAN.md.
2. Tag `v0.4.0` → `release.yml` (on `v*`) builds and attaches the
   platform binaries, Developer-ID-signed darwin included.
3. Before PUBLISHING the release: pull the darwin-arm64 asset and smoke
   it on this Mac — `setup` against the demo server, `qr` standalone,
   one language switch — the released-bytes sanity pass.
4. Publish (non-draft, non-prerelease) — that's the trigger
   `notify-mstream.yml` requires to signal the mStream repo, and
   `update-tap.yml` refreshes the Homebrew tap.

**9c — Integrate into the mStream binaries** (mStream repo).
1. ✅ Manifest bump (mStream PR #907, 2026-08-25): pinned `v0.4.0`, all
   six binaries downloaded and hash-verified by the update script; the
   bundler then re-verified the same pins live while staging darwin,
   linux and windows bundles. Musl keys find no entry and degrade as
   designed (asserted in alpine — see 4).
2. ✅ First-run wiring (mStream PR #908, 2026-08-25):
   - **Tray launcher**: "Set up mStream" menu item (all desktop
     platforms, not just mac+win — linux got it for free via the
     View-logs emulator walk). Always offered — the wizard reopens and
     seeds from server state — greyed when the install has no player
     binary. macOS launches an executable `.command` (no AppleEvents
     consent) with a CSI 8 resize to 120×42; Windows prefers `wt.exe`
     (sixel) with a conhost fallback; Phase 8's bundled Ghostty
     upgrades this later.
   - **Headless boot log**: with zero folders AND zero users, the
     server prints the invitation with the exact runnable command. The
     wizard line is gated on the binary being present (musl/docker
     hosts get the browser line alone) and, on linux, on
     `libasound.so.2` actually loading (the player links ALSA at load
     time — caught in the debian-slim smoke).
3. ✅ Compatibility asserted in code review: the wizard's progress arm
   swallows status errors (`Err(_) => {}` — "progress is garnish"), so
   a pre-`/scan/status` server just misses the final complete flip.
4. Bundle smoke matrix — darwin + linux legs done (2026-08-25):
   darwin-arm64 bundle built from both PRs: player v0.4.0 staged
   sha-identical to the release, launcher booted the bundle isolated,
   invitation printed the in-app player path, and the printed command
   drew the wizard in Terminal.app; debian container without ALSA
   suppresses the wizard line, with ALSA prints it and the binary runs;
   alpine/musl prints the browser line alone. Windows: static leg done
   (staged exe sha == pin); the behavioral pass (tray item → `wt.exe` →
   graphics probe lands on sixel) needs a real Windows machine after
   the PRs merge and CI rebuilds the committed launcher binaries — it
   doubles as the Phase 8 validation item.
5. ~~After the flip is proven, the deletions Phase 6 already lists
   (rust-server-audio tree and its CI) proceed on their own schedule.~~
   **SKIPPED (operator decision 2026-08-25) — and on inspection, already
   ~done:** the rust-server-audio tree, its bin/ folder, and its workflow
   are ALREADY GONE from mStream master (the flip's deletions rode along
   with the player-bootstrap work), the absoluteToVpath prefix bug is
   fixed (isWithin guard), and cli-audio/ is kept as planned. What the
   skip actually descopes is cosmetic residue only: the bootRustPlayer →
   bootServerAudio rename, two stale comments, and the template-marker
   "consider". Nothing to revive.

Phase 8 (the bundled console) stays sequenced AFTER 9c: it upgrades the
launcher's terminal choice, not the wizard itself.

### Phase 10 — The GUI player (`mstream-player gui`)

The mouse-first player surface the Win/macOS installers will launch in the
Phase-8 branded window — an ALTERNATIVE interface beside the classic TUI,
built on `src/kit/` the wizard's way (fixed palette, OSC 11 ground, every
action clickable AND keyed). Design of record: the "mStream Player GUI"
canvas, <https://claude.ai/code/artifact/e0a92aec-c63e-4156-84e2-15a05a6167f0>
(cell-exact 100×30 mockups + normative widget/limit boards), plus
docs/ui-kit.md. Direction approved 2026-08-27; the Jukebox sketch is kept as
a LATER secondary screen (party view), Columns retired.

- **Slice 1 — the shell ✅ 2026-08-27**: left nav (Settings live; the rest
  named and honest about arriving with browse), the Settings room (bottom-bar
  choice + the crossfade group, wired to the real audio worker and persisted
  per change), and BOTH bottom bars behind `[gui] bar` — `wave` (waveform +
  reflection over the compact control row; classic bar is the loading state
  AND the conhost floor) and `gold-line` (the gold rule IS the seek bar; song
  info left toggles the queue; tall 3-row controls). `src/gui/{mod,bar}.rs`,
  ten-locale `gui.*` keys, 18 unit/render tests + the ignored `dump_frames`
  eyeball. `MSTREAM_GUI_DEMO=1` seats a fixed track so the bars can be seen
  and the seek ridden before playback exists.
- **Slice 2 — Files + playback ✅ 2026-08-28** (bar settled the same day:
  controls above the rule, edge-to-card waveform, tips on the last row;
  GoldLine card right + volume on the tall controls' line). The GUI now
  embeds the REAL `App` + audio/api workers — the same state machine the
  TUI, wasm shell and replay harness drive — so session restore
  (`tui::startup`/`app_from`/`dispatch`/`remember`), queueing, crossfade
  announcements, track-end advance and waveform prefetch are shared, not
  re-implemented. Files leads the nav: browse the saved session's server
  (`gui --server <url>` overrides), click a row = select + `Activate`
  through the App's funnel, hover reveals [+] queue-add, kit scrollbar,
  wheel scrolls the view; the queue panel shows the real queue; the bar
  reads real timestamps/waveforms and seeks/pauses the engine. Settings'
  crossfade group now reads/writes the App's own knobs. Mouse verbs that
  have no honest keymap name (the volume cells) set the field and emit the
  `Effect` directly — documented, funnel-free by design.
- **Album art in the card ✅ 2026-08-28**: the graphics probe runs on boot
  (after init, the player's ordering; Resize refreshes the encode cache);
  the card paints real pixels where the terminal can and the ▀-mosaic
  elsewhere, the empty slot frame yielding once the art is decoded. The
  fetch was already free — `fetch_art` rides the App funnel. One cover per
  frame, so the single-slot encode cache holds; widening it comes with the
  Now Playing screen.
- **Search ✅ 2026-08-28**: `/` (or the nav item) opens the kit query card,
  which owns the keyboard while it takes text and drives the App's own
  query via `StartSearch`/`Input`/`Submit` — so `search_submitted`'s
  stale-reply guard keeps working. Five class chips (Artists · Albums ·
  Titles · Files · Lyrics) are the search params: state-colored toggle
  words filtering the class MENU (the API answers every class in one
  reply, so the choice is instant); ←/→ + `t` is the keyboard path. The
  results are the App's Search pane rendered by the same row renderer as
  Files (`draw_pane_rows`, with the dim detail column for drill rows) —
  class → listing → artist → albums all drill through the shared funnel,
  and clicks map through the filter to true pane indexes.
- **Servers ✅ 2026-08-29** (the Connect-screen slice, grown into
  multi-server): the header's server label becomes a dropdown once a
  second server is saved (current marked, default starred, "+ add" last);
  the [+] beside it — and the no-session screen's button — opens the add
  flow, a chooser first (2026-08-29 pass two): "Standard connect" is the
  SERVER/USERNAME/PASSWORD cards plus two checkboxes, "Accept a
  self-signed certificate" (a per-entry `danger_accept_invalid_certs`
  client, never process-wide) and "Public server" (stands the credential
  fields down; connect verifies with the auth-free ping); "Quick
  Connect" is where the design board's "ON THIS NETWORK" list moved —
  mDNS rows that carry their address to the standard page — plus the
  pairing-code paste, dialled through the funnel with the code held
  GUI-side until the tunnel answers (a bad code costs an error line,
  never the session that was playing; on Connected the code is seated
  for the save and the old server's state shed). The board's "Sign in
  once" line is dropped, and so is the switcher label's dwell tooltip —
  it matured exactly where the dropdown opens. Settings grew
  a SERVERS group whose row opens the Manage Servers room: every saved
  entry with username, live version (probed over the public `GET /api/`
  — ping carries no version; `Ping` still reads one tolerantly if it
  ever grows one) and the default star; per-row actions switch · edit ·
  make default (`default_server` config key, outranks MRU at startup) ·
  pair phone (the wizard's own QR renderers, pixels or half-blocks, over
  the stored pairing code — card cover stands down while it shows, the
  one-slot encode cache's rule) · remove (confirm modal; removal is the
  ONE flow that drops a pairing code). Adding/editing validates on a
  one-shot client so the live session is never touched until the server
  answers; switching goes through the App's own funnel
  (`App::adopt_server` → `begin()`), keeps what is already streaming,
  and clears the queue — queued tracks are filepaths resolved against
  the session's server at play time, so they cannot follow (true
  multi-server queues need tracks to carry their origin; deferred).
  Sign-in-needed answers (switch, expired session, fresh tunnel) open
  the same form in its funnel-riding flavour. The GUI now also renders
  the kit's dwell tooltips it had only been registering.
- **Albums wall ✅ 2026-08-29**: every album's cover, name and year in a
  grid, paged with ◂ ▸ arrows (and ←/→, PgUp/PgDn, the wheel) instead of
  scrolling — the webapp's album wall. Feasibility was measured first
  (the tests at the bottom of `gui/albums.rs` keep the numbers honest):
  a page of mosaic covers costs ~10 ms a frame with a `CoverPane` per
  slot, and the pixel path forks `Graphics` per slot with encodes PACED
  — each frame spends at most 40 ms starting encodes, the mosaic stands
  in, and a page upgrades over a few frames (debug-build sixel: worst
  frame 137 ms where the unpaced page cost two full seconds). Covers
  ride the App's own art claim (`fetch_art_file`, the playing cover's
  discipline — page-at-a-time, so the wholesale cap wipe self-heals);
  the list itself is the Library drill's Albums node, kept whole on
  `App.albums` beside the pane rows (the GUI door is
  `open_library_node`, seating the drill so the stale-reply guard keeps
  working). Clicking an album drills through the same funnel to its
  track list — `draw_pane_rows`, durations, hover [+], play on Enter —
  and the Parent row or `h` walks back, refetching the wall the drill's
  own way. Cell-exact: cover 12×6 (square at 10×20), name, `year ·
  artist` dim beneath.
  Performance pass (same day): `draw_pane_rows` takes BORROWED rows (the
  per-frame clone of every visible entry — and Search cloned its whole
  pane twice a frame — was drawing time spent on nothing); the wall
  draws through split field borrows instead of cloning a page of albums
  and their decoded covers (~50 KB of pixels apiece, ten times a
  second); the art-fetch scan allocates only for files still missing;
  `Graphics::refresh_font` re-reads the window-size ioctl at most every
  500 ms (one cover a frame was its design point — the wall made it
  fifteen syscalls a frame); and a frame whose encode budget turned
  slots away marks the Gui HOT, shortening the event loop's idle wait to
  10 ms so a page turn finishes upgrading in real tens of milliseconds
  instead of one encode per 100 ms poll tick.
- **Bar focused ✅ 2026-08-29**: the waveform bar is retired — the gold
  line is THE bar. With it went the `[gui] bar` config key (a leftover
  rides `GuiPrefs.extra` harmlessly), the Settings radio pair (PLAYBACK
  leads the room now), `Now.wave` and the wave/reflection renderer. The
  bar's dwell tooltips went too: the tips line already names every key,
  and a bar is hovered too often for tooltips to earn their draw.
- **Sonic path room ✅ 2026-08-31** — the first feature built the
  contract-first way: `docs/ux-contracts/sonic-path.md` extracted from
  the mobile app (the design of record, `mstream_music @ 137dd27`), the
  "Sonic Path Room" canvas drawn from the contract, then `gui/sonic.rs`
  implemented against both. The room rides the App's OWN sonic state
  machine (the TUI tab's) — setup cards with the three pick methods
  (playing · random · browse-capture), the ten-cell length bar, Build as
  the kit primary, results with seed tags + one-cell eighth-block match
  meters + Play/Queue all/Save as playlist, the save prompt on the App's
  line. Nav row is capability-gated on `discoveryPath` and takes digit 9
  (existing digits never renumber; absent is absent). New App plumbing
  both surfaces share: `ApiCmd::SonicRandom` (the record's random pick),
  and the clause-40 probe — a 403'd build re-pings before naming a
  reason (`JourneyIssue`/`SonicEmpty` carry the taxonomy typed, so Retry
  appears exactly where a retry can change the answer). Armed picks
  banner the note line, suppress the hover [+] (an armed click must only
  pick), and Esc/answer both return to the room. 18 new tests; smoked
  end-to-end against demo.mstream.io (random ends → build → meters on
  screen).
- Next slices, in rough order: the Now Playing screen (big art; the
  per-slot fork pattern from the wall applies), queue clicks + the rest
  of the Library tab views (Artists/Genres/Recent/Playlists — the wall's
  drill door generalizes), Discover's "Play a path to…" entry (revisits
  the contract's §5 search-skip), then e2e legs (fake server needs
  player endpoints).

## Smoke testing

`mstream-player replay "<script>"` drives the TUI from a script. Keys go through exactly the path
a real key press takes — `map_key(key, app.input_mode())` → `App::handle_action` → `ui::render`
against a `TestBackend` — so it exercises the real state machine and the real drawing code, and
prints the effects each step produced plus the resulting screen.

```
# offline and deterministic: worker replies come from @event steps. CI-safe.
mstream-player replay "Down, Enter, @servers, Enter, @needs-login"

# live: real workers, real server, real replies (and real surprises)
mstream-player replay "Down, Enter, Enter" --live --wait-ms 4500
```

Steps are keys by name (`Down`, `Enter`, `Esc`, `Tab`, `ctrl+c`), single characters, `'quoted
text'` to type, `@event` to inject a worker reply (`@servers`, `@connected`, `@needs-login`,
`@tunnel`, `@unauthorized`, `@error:msg`), `wait:500`, and `frame` to print the screen mid-run.

This exists because two bug classes were escaping: layout problems that a piped terminal capture
misreports (ratatui only redraws changed cells), and state-machine transitions that only appear
when keys are pressed in sequence against real replies. The first live run found one of each.

## Known risks (accepted)

- Linux binaries link ALSA dynamically (`libasound` required at runtime — already true today).
- Download-on-enable adds a failure mode → covered by `serverAudioBinaryPath` + CLI fallback.
- Two-repo version skew → covered by pinning + `apiVersion` check.
- rodio device-hotplug behavior is deferred, not solved. Gapless is deferred no longer: Phase C
  landed crossfade and true gapless, both opt-in — the default remains the compatible hard cut,
  so finding #8's gap stands only where nobody asked for better.
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
| 8 | Between-tracks the 250 ms advance poll reports `playing: false` transiently; inter-track gap is audible (no gapless) | known limitation | closed by Phase C where enabled (`--crossfade` / `--gapless`); the default stays the compatible hard cut |
| 9 | Shuffle has no history: `previous` can't retrace shuffled order; shuffle never ends under loop=none | semantics quirk | deferred to Phase 4 (queue UX pass) |
| 10 | mp3 without duration metadata (no Xing header) reports `duration: 0` | known limitation | documented |
| 11 | Negative or non-finite `/seek` position reaches `Duration::from_secs_f64`, which panics — and a panic while holding the state mutex poisons it, wedging every later request | crash bug (found during port) | fixed in port: positions validated before conversion |
| 12 | `/status` reports `playing: true` for a few ms after `/stop` — `sink.empty()` only flips on the next audio callback, so the old `playing` expression raced the audio thread | cosmetic race (found in Phase 2 testing; present in original) | fixed: `playing` also consults the engine's own synchronously-set `stopped` flag |
| 13 | FLAC files without a SEEKTABLE block (typical for ffmpeg-encoded FLACs) are **unseekable** — rodio 0.20's decoder wrapper hardcoded `byte_len: None`, so symphonia couldn't binary-search. Applies to the shipped jukebox with local files too, not just HTTP | seek bug, latent in original (found in Phase 2 testing) | fixed: upgraded to rodio 0.22 and its `DecoderBuilder` — engine now passes `byte_len` from file metadata / HTTP Content-Length + `with_seekable(true)`. Also fixed wrong duration estimates over HTTP (was reporting 64.29s for a 60s file) |
| 14 | mStream's default transcode codec is **opus**, which symphonia cannot decode — a client naively requesting `/transcode/...` with server defaults gets an unplayable stream | client-design constraint (found in Phase 2 testing) | Phase 3 requirement: the API client must always pin `codec=mp3` (or `aac`) in transcode URLs, never rely on the server default |
| 16 | mStream indexes `.m3u` playlist files as tracks, and the file explorer listed them alongside audio. `Enter` queues everything on screen, so a playlist file entered the queue and the decoder rejected it — and a failed track stopped the queue dead, so an album with a playlist file in it played nothing at all | user-facing bug (found by the author using the player against his own library) | fixed: the file browser offers only audio (unrecognised kinds still count as audio, so an undecodable format fails loudly rather than vanishing), and a source that won't play is skipped with a message naming it instead of ending the session. A run of failures as long as the queue stops rather than looping, since repeat would otherwise walk a queue of broken files forever. mStream's own Auto-DJ picker excludes `m3u` for exactly this reason — the file browser was the one path left |
| 15 | Several library routes documented in `docs/openapi.yaml` — `db/genre/albums`, `db/genre/songs`, `db/genre-groups`, `db/decades`, `db/decade/albums`, `db/decade/songs` — live in `src/api/velvet-stubs.js` and are **only mounted when `config.ui === 'velvet'`**. On a default-UI server they 404, and `/api/v1/ping` gives a client no way to tell which UI mode the server runs | client-design constraint (found in Phase 4 testing) | Library browsing uses only core routes: `db/genres` + `db/genre-songs` (so Genres drills straight to tracks, not to albums), `db/artists`, `db/artists-albums`, `db/albums`, `db/album-songs`, `db/recent/added`. Decade browsing is not offered at all |

## Appendix — Phase 4 audit findings (of this repo @ 9198e68)

A ten-lens sweep of the whole tree after the visualizer and mouse work landed. Every finding below
was re-checked against the code by a second reader before being written down; two candidates were
dropped as misreadings and six were downgraded. Numbering continues the Phase 1 table above.

### Correctness and robustness

| # | Finding | Severity | Fix |
|---|---|---|---|
| 17 | `remember()` (`src/tui/mod.rs:235`) does `config::load().unwrap_or_default()` on the way out. `startup()` already tolerates an unreadable config by running on defaults, so a TOML typo — which the file header invites, since `[keys]` and `[theme]` are meant to be hand-edited — means one launch-and-quit rewrites config.toml as defaults: server list, keys, theme and cache sections all gone. The warning `startup()` prints is hidden behind the alternate screen. `save_login()` at line 341 gets this right with `config::load()?` | data loss | skip the save when the load failed, and print the warning after `ratatui::restore()` |
| 18 | `src/engine/http.rs:101` sets only `connect_timeout`; there is no read or total timeout, and `open()` awaits header arrival inside the audio thread's state lock. The comment above it states the intent — "a dead server must fail fast, not hang the control API" — which `connect_timeout` does not deliver. Quick Connect makes it worse: requests go to the loopback bridge, so TCP connect always succeeds instantly and the only timeout present can never fire. A stalled tunnel or a half-open direct connection blocks the audio thread forever, so every later Pause/Stop/Seek queues unread and the session's playback controls are dead until restart | hang | add `.read_timeout()`/`.timeout()`, or wrap the `block_on` future in `tokio::time::timeout` so the stall becomes `PlaybackFailed` |
| 19 | `mstream-player login` (`src/cmd_library.rs:312`) aborts cleanly when config.toml is unreadable, but an unreadable credentials.toml silently becomes `Credentials::default()` and is then rename-replaced with only the new token — deleting every other server's token and every Quick Connect pairing code. `config.rs:504` documents why that is severe: a pairing code can only be re-fetched over an existing connection by an admin | data loss | treat the credentials load failure exactly like the config load failure nine lines above |
| 20 | `src/tui/worker.rs:444` installs the new `TunnelBridge` before the replacement connection is validated. Dropping the old bridge closes the loopback listener the *current* session is streaming through, so if the new code handshakes but the ping fails, the UI reports one error, keeps the old session — and every browse and play against the now-dead loopback port fails | bug | keep the old bridge and tunnel mapping until `connect()` confirms the new endpoint answers |
| 21 | The `starting` guard filters `Event::Status` by URL, but `Event::TrackEnded` and `Event::PlaybackFailed` (`src/tui/app.rs:2547`) carry no source and apply unconditionally. While a play blocks on a slow open, the user picks track B; the late failure for A blames B by name and skips from B to B+1 | race | tag both events with the source the audio thread already knows, and drop mismatches like `Status` does |
| 22 | `Event::Listing` (`src/tui/app.rs:2660`) and `Event::PlaylistTracks` (2781) apply unconditionally, while `Library`, `Discover` and `SearchDrill` all drop replies for a view the user left. Enter a directory on a slow link, press Back, and the late reply teleports the view and `self.path` back in — with `path` and the trail now disagreeing | race | drop a `Listing` whose path isn't the one being waited on, same rule the other three already follow |
| 23 | `jump_to_playing` (`src/tui/app.rs:2346`) sets `focus = Queue` without setting `queue_column`, which is the invariant `CycleFocus` maintains. With the column hidden (the default), `i` leaves the keyboard driving an invisible list: arrows move the unseen queue, Enter restarts the current track, `d` deletes a row the user can't see | bug | set `queue_column = true` when not fullscreen |
| 24 | `ListState` is `Copy` in ratatui-widgets 0.3.2, so `let mut state = app.queue.state;` (`src/tui/ui.rs:1142`) copies. `List::render` writes the corrected scroll offset back into that copy and it is thrown away every frame, so the fullscreen queue re-derives its window from a stale offset — the selection sticks to the bottom edge and the list slides under it | bug | take `&mut App` and pass `&mut app.queue.state`, matching `render_queue_column` |
| 25 | `spectrum()` (`src/tui/viz.rs:208`) windows with `hann(i / WINDOW)` but fills only `min(len, WINDOW)` slots. The tap ring is 8192 *interleaved* samples, so a 5.1 source yields 1365 mono frames and 7.1 yields 1024 — permanently under WINDOW. The data then ends at 0.75 (6ch) or 1.0 (8ch) window weight: a step discontinuity every frame, producing exactly the leakage the Hann exists to prevent, which the 0.85 tilt then amplifies in the top bands | bug | window over `take`, or size the ring in frames rather than interleaved samples |
| 26 | `Config` and `Credentials` deserialize without a catch-all, so `save()` (`src/config.rs:423`) drops any key it doesn't model. The `SCHEMA_VERSION` comment promises not to bump for added optional fields, which guarantees the collision: a newer player's settings pass the version gate on an older binary and are deleted the first time it saves — which happens on every exit | bug | `#[serde(flatten)] extra: toml::Table`, or drop the "optional fields don't bump the version" policy |
| 27 | `POST /seek` (`src/serve/mod.rs:224`) passes an arbitrary `f64` to `Engine::seek`, which rejects non-finite and negative but not magnitude, then calls `Duration::from_secs_f64` — which panics on `1e300`. The process dies (exit 101) and takes playback with it. This is finding #11 again, in the one dimension that check didn't cover | crash | reject out-of-range magnitudes alongside the existing finite/negative check |
| 28 | `read_body` (`src/serve/mod.rs:92`) has no size cap and no deadline, on the same loop that accepts requests and runs `advance_tick`. tiny_http hands over bodies above 1024 bytes as a lazy socket reader, so a client that promises 100 KB and stops writing wedges the server permanently — auto-advance included. The 401 path also drops the request with bytes outstanding, and tiny_http's `EqualReader::drop` allocates `vec![0; remaining]` to discard them | hang | cap Content-Length before touching the reader, read through `take()`, set a read deadline |
| 29 | `src/discovery.rs:73` interpolates `scheme` and `path` verbatim from an mDNS TXT record — publishable by anyone on the LAN — and never validates them. `scheme=https://evil.example/#` yields a base_url whose real host is the attacker's; the fragment is discarded on `join`, the scheme check passes, and the user is shown an ordinary login form whose POST goes to that host. The displayed URL still reads as a LAN address because the true host hides before the `#` | security | allowlist `http`/`https`, reject a path that isn't path segments; note that `normalize()` alone does **not** catch this |
| 30 | The serve control API defaults to no auth (`auth_token: Option`), and nothing checks Origin, Host or Content-Type. A `fetch` with a string body is a CORS simple request — no preflight — so any page the user visits while the jukebox runs can drive `/play`, `/queue/*`, `/stop`, and make the engine open a URL of its choosing. This is finding #6's LAN exposure reappearing as a same-machine one | security | require the bound Host and `application/json` on mutating routes; consider generating a token by default |
| 31 | Mouse capture is enabled outside ratatui's init/restore pair (`src/tui/mod.rs:181`) and disabled only on the normal return. ratatui's panic hook restores raw mode and the alternate screen and nothing else, so a panic drops the user into a shell still reporting mouse motion — which is the exact failure the comment at line 189 says the code exists to prevent. The pushed window title leaks the same way | robustness | chain a panic hook that emits `DisableMouseCapture` and `\x1b[23;2t` before delegating |
| 32 | `audio_loop` has no unwind guard, and symphonia has known panics on malformed files. A decoder panic kills the audio thread; the process-global ratatui hook then restores the terminal *under the still-running UI*, and `dispatch()`'s `let _ = audio_tx.send(cmd)` silently discards every later command. `AudioFailed` is only ever sent for an init failure | robustness | `catch_unwind` the loop body and emit `AudioFailed`; treat `SendError` in `dispatch` as "audio gone" |
| 33 | `Action::First`/`Last` (`src/tui/app.rs:1298`) dispatch on `focus` alone while `move_selection` special-cases fullscreen. In fullscreen, Tab is `NowTabNext`, so `CycleFocus` is unreachable and focus stays on the browser: `G` on the fullscreen Queue tab appears dead and silently moves the hidden browser cursor | bug | mirror `move_selection`'s fullscreen branch |
| 34 | The DJ panel's Mode row steps backward by calling `next_available` twice — correct only for a 3-cycle. Without `capabilities.discovery` the cycle is Off↔BpmKey, so on a default mStream install left/`h`/`[` does nothing while right works (`src/tui/app.rs:1785`) | bug | walk forward until the predecessor is found |
| 35 | `next_index` returns `Some` unconditionally in the shuffle branch (`src/tui/app.rs:498`), so `Repeat::Off` cannot be honoured while shuffle is on — the queue plays forever, contradicting the indicator the UI shows. Finding #9 called this out in the engine; the TUI copy inherited it | semantics | track the pass, or make the endless behaviour explicit in the indicator |
| 36 | `Vu`'s derived `Default` starts `peak_db` at 0.0, which is full scale: entering VU mode paints the peak marker pinned at the right edge for ~0.9 s and up to ~2.9 s on quiet material. `viz.forget()` runs on every mode cycle, so it happens every time | bug | hand-write `Default` with `peak_db: [VU_FLOOR_DB; 2]` |
| 37 | `rule_with_junction` puts the `┴` at `rule.x + left_width`, but the facts column's right border is at `+ left_width - 1`. Verified on a 90×26 render: divider in column 45, junction in 46 — the dangling join the function exists to prevent (`src/tui/ui.rs:911`) | cosmetic | pass `left_width.saturating_sub(1)` |
| 38 | `wrap()` (`src/tui/ui.rs:205`) budgets by `chars().count()` while the rest of the file measures display columns via `width_of()`. A CJK connect-screen error is sized at up to twice its budget and truncated by the un-wrapped Paragraph — losing the tail, which the comment at 179 says carries the instruction. Quick Connect's name column pads by chars too, so wide names shift the URL column | cosmetic | use `width_of()` in `wrap()` and pad explicitly |
| 39 | `centered_rect` (`src/tui/ui.rs:2191`) multiplies `u16` by percent: at 937+ columns the 70% journey panel overflows — panic in debug, nonsense width in release. Every other size calculation in the file saturates | robustness | compute in `u32` |
| 40 | `write_atomic` (`src/config.rs:368`) uses one fixed temp name and no fsync. A CLI `login` running while the TUI saves can rename the other process's truncated temp into place — corruption produced by the crash-safety machinery itself | race | unique temp name per writer, `sync_all` before rename |
| 41 | `version` is required on both `Config` and `Credentials` with no serde default, so a hand-written config without it fails to parse — and then `remember()` overwrites the hand-edited file with defaults on exit (see #17). The header pitches the file as editable | robustness | default `version` to `SCHEMA_VERSION` |
| 42 | `Client::resolve` (`src/api/mod.rs:123`) loads config before looking at its arguments, so a broken config defeats `--server` + `--token` — the flags that exist to be the escape hatch. `cache_root()` already tolerates a broken config for this reason | ergonomics | load only in the branches that need it |
| 43 | `start_current`'s failure paths `eprintln!` (`src/engine/mod.rs:232`, 236-240, 246, 256, 266) at the moment a track fails, which in TUI mode is mid-session in raw mode. The error already reaches the user as `PlaybackFailed`; `http.rs:54` limits its own warning to once per process for exactly this reason | cosmetic | drop them, or gate on a mode flag so serve and CLI keep the lines |
| 44 | Same class, worse cadence: the tunnel's per-connection tasks `eprintln!` on teardown (`src/quickconnect.rs:274`), and the comment above admits the trigger is routine keep-alive idle — so a healthy tunneled session smears text across the UI repeatedly | cosmetic | drop or gate while the TUI owns the terminal |
| 45 | `discover --seconds=-1` (or `nan`, or `1e300`) reaches `Duration::from_secs_f64` unvalidated and panics with a backtrace prompt — the one flag in this layer where a typo crashes instead of printing a sentence (`src/discovery.rs:152`) | robustness | a clap `value_parser` range |

### Performance

| # | Finding | Severity | Fix |
|---|---|---|---|
| 46 | `render_current_column` (`src/tui/ui.rs:594`, and the trail/queue/now-queue equivalents) builds an owned `ListItem` for **every** entry each frame, 2-4 `String`s apiece, while `List` draws only the ~30 that fit. Measured with a throwaway release benchmark: 8.4 ms/frame at 10,000 entries vs 0.27 ms at 100 — about 8% of a core burned continuously at the 10 Hz idle redraw, scaling linearly with folder size | medium | build only the visible window, or cache rows against a pane revision counter |
| 47 | The event loop draws a full frame per input event (`src/tui/mod.rs:277`) and never drains the input queue — only the worker channel. Mouse capture arms any-motion tracking, so sweeping the pointer emits one event per cell crossed and the loop services 100-200 consecutive full frames with keystrokes queued behind them. Compounds #46 exactly | medium | drain with `poll(ZERO)` before drawing, collapsing consecutive `Moved` to the last position |
| 48 | `Engine::seek` (`src/engine/mod.rs:363`) holds the state mutex across `try_seek`, which blocks on rodio's feedback channel until the *audio device callback* performs the seek — and for a target past the downloaded range that seek waits on the network. So a seek into un-downloaded audio stalls the device callback (audible dropout) while pause/stop/status all block on the held lock | medium | drop the guard before `try_seek`; the callback stall is inherent to rodio's pull model, the control-plane freeze is not |
| 49 | `advance_tick`'s skip-unplayable loop (`src/engine/mod.rs:519`) tries up to `queue.len()` sources in one call with the lock held. Ported from local files where an open failed in microseconds; entries are now HTTP URLs, so a 30-track queue against a downed server freezes the serve control API for ~150 s. TUI mode is spared — it keeps one entry | medium | attempt one source per tick, carry the counter in `State` |
| 50 | The audio loop takes one command per iteration and each `Play` blocks through a full open + format probe (`src/tui/worker.rs:299`). Hammering next through remote tracks pays every abandoned open serially, with Pause/Stop queued behind the doomed fetches and no status flowing | medium | drain with `try_recv` and collapse runs, keeping only the last Play/Seek/SetVolume while still honouring any Stop |
| 51 | `drawing_audio()` gates the 33 ms poll on "fullscreen && Visualizer" with no check on whether audio is sounding (`src/tui/app.rs:1150`). Paused, every value reaches zero within ~2 s and every frame is identical — yet the app keeps waking 30×/s to copy the ring, allocate, and run a 2048-point FFT over silence, indefinitely | low | fall back to the 100 ms poll once nothing sounds and the picture has settled |
| 52 | The paused path's `..heard.clone()` (`src/tui/viz.rs:111`) fully evaluates the clone — copying the 32 KB sample vec — then drops it because `samples` is overridden. Only `rate` and `channels` were wanted | low | name the two fields, or keep one reusable zeroed frame |
| 53 | `draw_scope` collects `chunks_exact` into a `Vec<&[f32]>` (~64 KB) purely for random access that indexing gives free, and `spectrum()` allocates its `re`/`im`/magnitude scratch per call (`src/tui/viz.rs:419`, 204). ~100 KB across ~10 allocations per frame, when `Bars` already persists its state vectors and could own the scratch too | low | index arithmetically; move the FFT scratch into `Bars` |

### Structure — worth doing before the codebase grows

| # | Finding | Fix |
|---|---|---|
| 54 | `app.rs` is 6,238 lines and 42% of it (3608-6238) is one `#[cfg(test)] mod tests`. The clusters inside it already sort by domain, so moving it to `app/tests.rs` halves the file with near-zero churn and turns every later extraction into a small diff | do this first; it is the enabler for 55-57 |
| 55 | The keymap subsystem (3061-3605 plus `Action::name`/`from_name`) reads no `App` state and is reached only through `Keymap::action`. ~545 lines of code plus its tests, and the part most likely to keep growing | move to `src/tui/keymap.rs` — almost pure text motion |
| 56 | The connect/session flow — `ConnectStage`/`ConnectForm`, `handle_connect_action`, the two submits and five `consume` arms — is ~390 lines plus ~560 lines of tests, and owns the first 8 `App` fields that nothing else writes. It already has three ways in and keeps attracting fixes | `src/tui/app/connect.rs`, and a `Session` struct so "which server is this" travels as one value |
| 57 | Auto-DJ/journey state is 8 fields serviced across five separate regions, and `autodj_pending` — one invariant — is cleared independently in three of them | `src/tui/app/dj.rs`, with the four DJ events routed through one `consume` |
| 58 | Library, Search and Discover each hand-roll the same drill-down: a `Vec<Node>` stack, an activate arm, a `step_out` arm, and a stale-reply guard. Nine blocks implementing one concept, and the stale-reply rule — the one that matters — is maintained by hand in three places (see #22, which is the fourth copy that was never written) | a generic `Drill<N>` owning `enter`/`back`/`accept`, then build `app/nav.rs` around it |
| 59 | The Tab→Pane mapping appears five times (`pane`, `pane_for_mut`, `note_pending`'s table, `clear_pending`'s array, and `ui.rs:609`). Two aren't compiler-checked: a missed `note_pending` arm is a spinner that never lights, a missed `clear_pending` slot is one that never stops. `app.rs:3064` already documents this drift happening once | one `panes: [Pane; 5]` indexed by `Tab` |
| 60 | `consume()` is a single 332-line match over 21 event variants spanning every domain | when 56-58 land, make it a dispatcher to per-module `consume`s — cheap then, expensive later |
| 61 | The entry builders (2860-3059 plus the search/discover ones) are pure data→`Vec<Entry>` functions carrying their own history of server quirks, interleaved between the event handler and the keymap | `src/tui/app/entries.rs` |
| 62 | `app::Queue::next_index`/`prev_index`/`remove` re-implement `engine::pick_next`/`apply_remove` verbatim — same shuffle formula, same loop-one rule, same remove fixups — and the app.rs comment admits they're synced by hand. They never cross-check because the TUI plays via `play_source`, which keeps the engine queue at one entry. Both copies carry parallel test suites, and they have already drifted on remove-current | extract the pure `(len, index, shuffle, loop, manual)` functions into one module both call |
| 63 | The API worker is strictly serial (`worker.rs:425`): one 20 s timeout head-of-line blocks every pane, and a Quick Connect dial can hold it ~48 s. The code concedes this once already — mDNS discovery got its own thread so a browse "shouldn't hold up a pairing attempt queued behind it". The admin panel's log tailing arrives on this seam next | decide the concurrency story before B4: dispatch onto the shared runtime with a generation token, or formalise the `spawn_discovery` escape hatch |
| 64 | Reply routing is encoded in the variant name — `ApiCmd::SearchDrill` exists only so a Library reply lands in a different pane, duplicating the Library command and event wholesale. Each new operation costs four compiler-checked places plus `note_pending`'s silent `_ => continue` | carry the destination in the command and echo it back on the event; derive the spinner's tab from the same field |
| 65 | `mstream-player dj` documents itself as "the scriptable view of what the player does" but hand-assembles the request instead of calling the tested `dj::build_random_request`, so it ignores artist cooldown, min rating, genre filters and the sonic pool — and already diverges today for any non-default key matching or rating (`cmd_library.rs:183`) | load the saved prefs and call the builder; collapse `BpmRange`/`BpmWindow` into one struct |
| 66 | `ui.rs` imports `fmt_duration` from `cmd_library.rs`, so the render layer can't compile without the CLI smoke-test harness. Every other shared formatter lives in `api/types.rs` | move it next to `display_name`/`metadata_display` |
| 67 | `worker::connect` and `worker::login` share the ping→`Connected` tail and both take an `_events` parameter neither uses | extract `establish()`, drop the dead parameters |
| 68 | `Engine::queue_add_entry` exists to carry a duration hint, but its only caller always passes `None` and the serve routes have no way to supply one — so the module header's promise that hints save a second fetch is dead in the only mode that uses the engine queue | inline it, or thread hints through the serve queue routes |
| 69 | `tokio::time::timeout` is used in three places in `quickconnect.rs` — the timeouts that keep a dead tunnel from hanging the api thread — but `time` isn't in our tokio feature list. It builds only because iroh and reqwest happen to enable it and Cargo unifies features; a dependency diet breaks the build pointing at the wrong file | add `"time"` |
| 70 | `base64` is pinned at 0.23 while every other consumer in the tree (reqwest, iroh, portmapper, tokio-websockets) is on 0.22.1, so both compile | drop our pin to `"0.22"` |
