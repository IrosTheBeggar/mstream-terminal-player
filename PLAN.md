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

**Libraries room ✅ 2026-09-06** — `mstream-player admin` (`src/admin/`), the kit surface the
"Admin Libraries" canvas settled on, barebones by choice: a NAME · FOLDER · SYMLINKS table over
`admin/directories` (` [X]` outside the selection, tips on the bottom edge, no bottom bar or gold
rule), adding by the OS picker (`--same-machine`, the wizard's `picker.rs` on the worker), the
admin file explorer, or a typed path with server-fed completion — every route ending in the Name
modal, because the vpath is permanent (no rename API) — the remove gate, and the per-library
follow-symlinks toggle. The two gates above land as sentences (405 → "locked", 403 → "not an
admin here, or restricted by address"). Not built: the Overview (rooms + network stats,
canvas "Admin Overview", pinned), counts and scan columns (no per-library source since the
velvet removal; `/scan/progress` rows exist only mid-scan), rename (no API).

**Discovery room ✅ 2026-09-06** — `mstream-player admin discovery` (`src/admin/discovery.rs`;
`src/admin/mod.rs` is now the hub: the `Screen` trait, the shared terminal session, header and
bottom lines, the gate sentences). The webapp's Discovery page in the kit, per the "Admin P2P
Discovery" canvas: the state line (connected / joined, waiting / reconnecting, attempt N / not
joined / binary missing) with the all-dim "searching" bar, the four tabs in an eight-row pane
(Stats tiles, the Activity ring delta-polled by seq, Invite with the endpoint + ticket + the
befriend box as an inline kit input, Config with the identity and the five numeric settings on
digits), the catalog as a table whose SNAPSHOT and FEDERATION columns replace the card chips
(relations derived from `/admin/federation/requests`), the Enter sheet as a list picker, `f`
compose (message + offered vpaths), block and leave as gold gates, `h` incompatible, `u`
blocked, `/` filter, a quiet ten-second poll. Off the network: the pitch, one join card that
posts `enabled` at once (no gate, by the user's call) and the federation-requests opt-in on by
default; the identity modal opens after the join, as the webapp does. `y` copies the ticket
through the kit's clipboard (since 2026-10-02: the pasteboard or the platform's tool, OSC 52 as
the terminal's last resort and alone over SSH, never in the window). Kit additions documented: tabs (active
= 1-row filled slab), the state line, an inline non-modal input.

**Federation room ✅ 2026-09-07** — `mstream-player admin federation` (`src/admin/federation.rs`),
per the "Admin Federation" canvas: the state line (on · relay · endpoint · inbox), three tabs
Requests · Tickets · Peers, each a full-height table with its affordance on top (the inbox
checkbox card, the mint card, the paste box with the `mstrfed1:` ticket decoded client-side as
the webapp does), the webapp's request-state vocabulary as kit colors with the retry ladder
off the engine's state, one `Form` shape for mint / accept / limits (Tab-cycled fields, digits
only in the caps, the tri-state expiry), the minted ticket shown whole, gold gates for decline
/ revoke / forget / turn off, a quiet 30 s poll. What a peer shares is kept in memory from the
last `t` (the server stores no list). Not built: the Overview (pinned), a "view peer" jump.

**Backups room ✅ 2026-09-07** — `mstream-player admin backups` (`src/admin/backups.rs`), per the
"Admin Backups" canvas: the page's beta banner as a chip, the live-progress card and queue
notice as one state line (the bar's denominator is the previous run's total; a first run gets
the all-dim bar), the add card over a LIBRARY · DESTINATION · TRIGGER · RETENTION · LAST RUN · ON
table with throttle and excludes on the note line, one form for add and edit (radio row for the
library, the kit's radio group for the trigger, the path with `^B` browse — server browser or the
OS dialog with `--same-machine` — and the server's check-path verdict beneath, digit fields, the
exclude list prefilled from `/backup/platform` and OMITTED when untouched; edit sends only what
changed and `^P` sends `excludeGlobs: null`), the history modal with the selected run's notes
wrapped beneath, the remove gate ("the files on disk stay"), a 2 s poll in flight and 5 s idle.
`AdminDirEntry` gained the library `id` the backup routes address. Live-checked against the
scratch server: add (manual) → run now → history → remove.
Admin auth is the **same JWT** — `admin` comes from the users table, and the token is byte
identical, so the client must probe rather than inspect it. Two gates will bite a terminal client:
`lockAdmin` returns 405, and `adminAccess.mode` restricts by IP, so a panel run from another
machine gets 403 under hardened configs. Say that plainly rather than showing a bare error.

**Torrents room ✅ 2026-09-08** — `mstream-player admin torrents` (`src/admin/torrents.rs`), per the
"Admin Torrents" canvas: two phases — the client page (radio group Disabled / Transmission /
qBittorrent / Deluge with each daemon's port and quirk; the policy radio beneath it only when the
server has more than one user; Enter posts `/admin/torrent/client` and opens Connect) and the
connect page (HOST · PORT · USERNAME · PASSWORD · RPC PATH · HTTPS per client; `^T` posts
`/<client>/test`, Enter `/connect`, the daemon's answer under the form, qBittorrent's CSRF caveat)
— then the state line (connected / disconnected with the reason / asking) and five tabs: Torrents
(NAME · STATUS · PROGRESS · DOWN · SIZE · ADDED BY, `/` filter, `r` remove behind the files-stay
gate, the daemon's list error inline), Libraries (ACCESS ladder · PATH AS SEEN BY <client> ·
TEMPLATE; `d`/`D` auto-detect, `m` manual mapping with the 422 reason in the modal, `t` template
modal with the sample preview, `^S` suggested, `^X` clear), Seeding (`a` a `.torrent` — the OS
file dialog with `--same-machine` (picker gained `pick_file`), else a path modal with local Tab
completion; SEARCH IN ticks on digits; one multipart POST per file, queued; outcomes in the
webapp's words), Access (policy radio posts at once; USER · ADMIN · TORRENTS toggles; hidden with
one user), Client (status, saved fields, other saved clients, `t` test, `c` switch, `x`
disconnect gate). Polls: the list every 5 s on the Torrents tab, everything every 30 s. API:
the torrent types, twenty calls, a raw multipart `send_bytes`, `admin_users`;
`extract_error` now prefers a body's `message` sentence over its `error` code. Locales `tor:`
(240 keys × 10). Live-checked on the scratch server: choose Transmission → the connect form → a
probe against nothing ("connection failed: connect ECONNREFUSED") → back to Disabled. Cleanup
2026-09-17, once the GUI's Add-torrent room was rebased in: the seeding tab's dialog is the shared
typed `picker::pick_torrent` (`pick_file`/`FilePick` gone), and its POST rides the shared
`Multipart` + `post_multipart` under the torrent routes' 45 s ceiling (`send_bytes` and the
duplicate `extract_message` gone).

**Users room ✅ 2026-09-13** — `mstream-player admin users` (`src/admin/users.rs`), per the "Admin
Users" canvas: one table USER · ADMIN · LIBRARIES · FOLDERS · UPLOAD · AUDIO · MODIFY with checkbox
cells (`1`–`5` or a click flips a flag through `POST /admin/users/access`, echoing all five since the
route defaults the ones it does not see), `l` the library grant as a checkbox modal (`POST
/admin/users/vpaths` with the whole list), `p` a new password typed twice, `r` a gold gate naming the
server's CASCADE (playlists, play history, grants; files stay) with an extra line for the last admin;
`a` the add form with the webapp's defaults (`PUT /admin/users`; empty and taken names refused before
the server, which answers a taken name with a bare 500). Public mode (zero users) is the gold state
line plus the webapp's two warnings; after the first user the room signs in as that user
(`/auth/login`), keeps the session the way the hub's sign-in page does, and reloads. MODIFY
(`allowFileModify`) is shown although the webapp dropped it. Quiet 30 s reload, never under a modal.

**Stats page ✅ 2026-09-14** — `mstream-player stats` (`src/admin/stats.rs`, `src/admin/tz.rs`), per
the "Player Stats" canvas: the server's `/stats` page (Stats API v2, mStream 6.27) as a standalone
page on the hub's session and sign-in — any account, since the log is per account, so `run_stats`
shares the preflight (`ensure_session`) with `admin::run` but is not a room. Three tabs (the state
line that carried the period's totals went 2026-09-27 as redundant with the tiles; "times in UTC" ends
the Recent tab's note when the zone is unknown): Overview (the webapp's six
tiles as rounded cards with the delta against the previous period — a second `/stats/summary` at offset−1 — plays
per day/week/month as eighth-block columns from `/stats/timeseries` — on a whole-number scale with a
ticked baseline, three to eight rows as the screen allows (2026-09-27) — the hourOfDay profile, the
local/peer split with the ▰▱ bar while peer plays exist), Top (`/stats/top`, entity on `t`, metric on
`m`, the share bar following the metric, `via <peer>` from peerName), Recent (`/stats/history`
paged by the cursor as the selection nears the end; outcome, listened and client words as the
webapp's stats-view.js spells them; `x` → gold gate → `DELETE /stats/plays/:id`, then a quiet
reload). Periods from `/stats/periods` in the webapp's `periodOptions` order, stepped on `[` `]`,
dropped as a list under the `This month ▾` control on `p` or a click (2026-09-27; before, a centred
picker); the default period with no data falls to the first with data, as the webapp does. A change
keeps the page drawn as it stands until the new numbers land (the page tracks the period its numbers
belong to), so nothing blinks. Every read carries a `seq`; a stale answer is dropped. Time zone without a crate: `tz.rs` reads the
TZif file `/etc/localtime` points at (or `TZ` names) — transitions, then the POSIX footer rule — for
the IANA name the server wants and the offsets the local clock needs; no file → UTC. Empty log,
empty period, and a pre-6.27 server (404) are their own states. Not built from the canvas: the
"no account" board — the server pins public mode to its sentinel account, so a public server logs
plays too (a 403 there only means a federation key or guest token, said in one sentence). Deferred:
the player reporting its own plays (`POST /stats/plays`) — the log shows other clients' listening
until then. Locales `sta:` (177 keys × 10). Live-checked against a 6.27 scratch server seeded with
252 plays. 2026-09-27, once the page met the player's own reporting on main: the client column
knows the player's reports by the shared `CLIENT_NAME` (they had drifted — `mstream-player` vs
`mstream-terminal-player`, so the player's own plays showed a clipped name instead of "this
player"), and totals under a minute read in seconds (`7 s`) where the webapp rounds to `0 minutes`
— a Top row ranked by seconds of listening read as nothing.

**Admin tab in the GUI ✅ 2026-10-02** — the GUI's third tab (`M`), per the "Admin GUI Hybrid"
canvas and docs/ux-contracts/admin-screen.md: a hallway of the six rooms (SERVER: Libraries,
Users, Backups; NETWORK: Discovery, Federation, Torrents; WATCH: Log) beside the room, hosted
whole on the GUI's session through the hub's new hosting seams in `src/admin/mod.rs` (an area
to draw in, `body_column`/`draw_foot`/`draw_chip`, `modal_rect` centred in the area, the room's
`Claim` on the keyboard, one shared pointer routine, the `HostedRoom` face the GUI holds), and
the server's log (`GET /api/v1/admin/logs/recent?since=`, `src/gui/server_log.rs`) on a worker
thread of its own — docked as a column at 160 columns, as a band under the room at 48 rows,
otherwise a room of its own on the hallway's Log row, its lines wrapped whole under their clocks
(below); `L` hides or opens it. The host takes only Tab, BackTab and `L` from a focused room,
never a digit, `q` or Esc; Esc or `q` at a room's base hands the focus back to the hallway. Ten
locales (35 keys, `gui.tips.base` names `M`), tests for the footer's 99 cells and the hallway's
label widths in every locale, and an e2e leg with no server. Not built: the Overview row, the
rooms' one-line summaries, server-side log levels (the player's own level menu went on
2026-10-02, below: the log shows every line the ring holds).
Merged after v0.10.0, so the desktop product's window (Phase 14) hosts the tab too: the window
drives the same two loop halves as the terminal (`gui::frame`, `gui::input`), so the tab's keys,
pointer, wheel and hand cursor need nothing of their own there. One gap showed only in the
window, closed 2026-10-02 (contract clause 28): the hosted rooms drew their fields on their own
surfaces and noted no caret on the GUI's, so the window counted no field as having the keyboard
— Cmd+V pasted nothing into a room's field (a Federation or Discovery ticket included; Ctrl+V
off a Mac reached the field as a key it ignores), and the input method stayed off there. Every
room's focused field now draws through `kit::field_display`, which notes its caret and draws
the composition; `HostedRoom` carries the composition down and the caret up to the GUI's
surface, where the window reads it. The follow-up: a hosted room's caret holds steady rather
than blink, since the GUI times its frames by its own surface's blink clock.

> **Shipped as v0.11.0 on 2026-10-03 (release run 37092102068: both families, the three packages, the app notarized and stapled, Homebrew and Scoop bumped, 21 assets), with the log's wrapped lines, scroll bar, highlight, copy and download, and the hosted rooms' fields taking paste and the input method in the window.**

**Log copy, download and highlight ✅ 2026-10-02** — the Admin tab's server log gains its ways
out (contract clauses 29-34). A press-drag across its lines highlights whole lines, held by
sequence number so arrivals and the ring's drops never move it, through a new kit element, the
drag region (`Surface::drag_region`: the press, every move and the release wherever it lands,
with the scrollbar's hard capture, so `gui::input` skips the hosted room and the Stats page
while a grip stands). `y` copies the highlight or every line shown — the clock, winston's
`warn`/`error`, the whole message with its later lines indented — through the new
`kit::clipboard`, whose route follows where the player runs: the pasteboard (arboard, the
desktop flavour only) and the platform's tool in the window, never OSC 52 there; OSC 52 alone
over SSH; pasteboard, tool, then OSC 52 in a local terminal. Discovery's and Federation's `y`
moved onto it, so a ticket copies in the window too. `d` fetches `GET
/api/v1/admin/logs/download` on a one-shot thread with the log's client (never the poll's,
Known risk 63), checks the zip by its end record, and saves `mstream-logs-<server>-<time>.zip` in
the Downloads folder without ever replacing a file; a server with no log files or no route
leaves the lines shown as `.txt`, and a zip cut short is never saved. `o` shows the file in
Finder or Explorer (the folder elsewhere) rather than opening it, through `src/gui/opener.rs`,
which the torrent room's opener moved into. The header carries `copy` and `download` for the
pointer, whole or not at all, with `downloading…` while one runs. In the window Cmd+C
(Ctrl+Shift+C or Ctrl+Insert elsewhere) is the log's `y`, and the window always takes the
chord, so Ctrl+Shift+C off a Mac no longer quits. Ten locales (22 log keys, the two rooms'
`copied_clipboard`, the log's footer re-cut to 99 cells). Not built: part-line or keyboard
highlighting, edge auto-scroll, a drag in Apple Terminal (it reports no held button), choosing
the save folder.

**Wrapped log lines ✅ 2026-10-02** — a log line was one row, the message's first line clipped
at the log's edge with ` …` after a stack trace; now the whole message wraps under its clock
(contract clauses 23, 24, 29). Each of its lines wraps by words in cells at the log's width less
the clock's ten, keeping a stack trace's indentation; a word wider than a row (a path, CJK with
no spaces) breaks at the row's last cell; the later rows are set in under the message's first
cell, every row in the line's colour. The view still scrolls by lines with the bottom line whole
and the top one showing its tail, but a line taller than the view goes by rows and Home shows
the oldest line from its clock, so no row is out of reach. Every drawn row maps to its line for
the drag region, so a press on a later row means that line. ` …` and the first-line text it
marked are gone; the copy is unchanged.

**A taller band, a scroll bar, every level ✅ 2026-10-02** — three changes asked for together
(contract clauses 21-25 and the deviations log). The band under the room is twelve rows, ten
of them lines where seven were, the room three rows shorter and the 48-row threshold as it was.
The log has the kit's scroll bar on its last column while its lines do not all fit whole (the
lines a cell narrower beside it, none and the whole width when they fit), counted in lines
rather than rows so no frame wraps the whole ring: the thumb is where the oldest line shown
from its clock stands among them all, its length the share shown whole; the endcaps are ↑ and
↓ with the kit's hold-repeat, a track press moves the bottom line there (top of the log at one
end, following at the other) and arms the thumb's drag. The kit gained `scroll_items` (the bar
sized by items) and `Surface::holding_bar`, which `gui::input` now honours as it does a drag
region's grip, so a held arrow or thumb keeps the pointer from the hosted room until its
release. The drag region for the highlight leaves the bar's column out. The level filter and
its menu are gone: every line shows (http, verbose, debug and silly too) in the colours it had,
the Log row counts every line, Enter does nothing in the log, and the four level labels and the
menu's footer went from all ten locales, `tips_log` lost `Enter level`, and `empty` now says
the server's log has no lines yet.

Build, in order of fit: **logs** ✅ 2026-10-02 (`/api/v1/admin/logs/recent?since=<seq>` is a
purpose-built tail-poll API with a cursor; the GUI's Admin tab tails it, above), **scan progress** (use the *non-admin* `/api/v1/scan/progress` and
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

**File associations — the Add-torrent seam (2026-09-01).** The GUI's
Add-torrent room (Phase 10) ships `mstream-player gui --torrent
<file-or-magnet>`: the OS-hand-off entry point of the mobile app's
feature, waiting on a claim. Each installer registers the app for
`.torrent` files and the `magnet:` scheme and launches that seam — macOS
document types + URL scheme on the .app (through the bundled Ghostty's
config command), a Windows file association, a Linux `.desktop`
`MimeType=application/x-bittorrent;x-scheme-handler/magnet;`. Two riders
come with the claim: the Settings "default app for torrents" row the
contract dropped for now (clause 52), and a hand-off that must not loop —
the room already recognizes its own staged file coming back and says so,
but a registered app should offer "open in another app" the way the OS
does it (a chooser, not `open`).

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
   `update-tap.yml` refreshes the Homebrew tap. (The notify-mstream.yml
   trigger never fired — see Phase 14, 2026-10-03.)

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
- **Playlists room ✅ 2026-08-31** — contract-first again
  (`docs/ux-contracts/playlists.md`; the record is the mobile browser
  tab's SERVER-playlists view, not its local-playlists screen), and the
  first feature to skip the canvas deliberately: every element maps to
  an idiom already drawn (the albums drill, the kit affirmative card,
  the servers-room hover verbs, the sonic name modal, the kit warning
  modal), so the contract's translation table is the whole design. The
  list rides `LibraryNode::Playlists` with the New card above it;
  hover (or `e`/`x` on the cursor row) reveals rename and the [X];
  create/rename share one name dialog with the record's no-op contract;
  delete confirms through a warning gate where `x` answers and Enter
  stays the safe way out. Activation drills to the shared track rows.
  New plumbing both surfaces share: `playlist_new/rename/delete` on the
  client and their ApiCmds — silent on success (the row changing IS the
  confirmation; an open Playlists view re-asks), worded per failure,
  with rename's 404 named as the server's age (the route is 5.16.0+;
  the session carries no version, so the record's hide-the-item gate
  became honest wording — see the contract's deviations). 7 new tests;
  read-only smoke against demo.mstream.io (list · drill · wheel · back
  — no writes to a shared public server).
- **Browser top bar ✅ 2026-08-31** — contract-first
  (`docs/ux-contracts/browser-top-bar.md`; the record is the mobile
  toolbar's context-aware AppBar bottom slot), canvas skipped again: one
  bar row of existing kit vocabulary. The bar is two lines: the
  crumb row stays what it was (back ◂ when there's somewhere to go, the
  path, the count), and the controls line beneath carries —
  gated on the list holding playable rows, the record's no-dead-chrome
  law — play / queue all / shuffle as text verbs, the filter affordance
  always, and the honest count (`n items`, `n of m` under a filter).
  The filter IS the App's own prompt (StartFilter/filtering/apply_filter
  — live narrowing, Enter keeps, Esc clears, backspace-past-empty
  leaves, all pre-existing and TUI-shared); the three verbs are new App
  helpers on the pane (`play_listing`/`queue_listing`) both surfaces
  share — shuffle reorders ONCE, the record's semantic, mode untouched;
  verbs act on the narrowed view (what you see is what plays — a logged
  deviation from the record, which reads the unfiltered list). Keys
  f/p/A/S; verb keys ride dwell tooltips, the close-control's pattern.
  4 new tests; read-only smoke against demo.mstream.io proved the
  gating live (folders-only root: no verbs; drilled into tracks: verbs
  appear; filter narrows with the `of` count).
  Same-day follow-ups from review: the bar became TWO lines (the header
  row each room always drew, the controls beneath — filter leading on
  the left, verbs right), and it now serves every browse page — Files,
  the albums wall and drilled album, the playlists list and drilled
  playlist. The drilled views reuse the pane machinery wholesale; the
  wall filters through an index map (pages, cells, cursor and cover
  prefetch all follow the narrowed view, and a filtered cell opens the
  album it shows). The keys ride one shared gate that outranks the room
  handlers, so a typed letter can never queue a row.
- **Add torrent ✅ 2026-09-01** — contract-first
  (`docs/ux-contracts/add-torrent.md`; the record is the mobile app's
  smart Add Torrent panel, itself a port of the webapp's, plus both
  directions of its OS hand-off). The contract went to discussion with
  five open questions and came back with five decisions, all logged:
  the webapp's fuller `match_unmapped` / `pad_files_missing` wordings,
  a second instance for a second `--torrent`, no OS-defaults row until
  the installers exist, the hand-off kept as one verb on the file chip,
  and the room housed as a Settings doorway (the Manage-servers shape)
  plus the `mstream-player gui --torrent <file-or-magnet>` seam that
  stands in for the OS hand-off. `gui/torrent.rs` is the room: the
  `/torrent/preflight` gate as a gold banner with the server's own
  reason (no ping flag exists), the native file dialog (the
  wizard's picker backends grown a file-typed sibling, filtered to
  `.torrent`, started in Downloads, on a thread) with a typed `.torrent`
  picker as the fallback and the `t` road (the wizard's completion
  re-drawn for files; listings and reads on threads), a live-validated magnet row, metadata pre-filled
  from `info.name` / the magnet's `dn` through `torrent_meta.rs` (a
  port of the record's parser and the server's sanitizer, `regex` now a
  direct dep), the library's path template resolved with sticky hand
  edits, rename-root on by default, the seed-existing check before any
  add with every outcome worded and the partial-match picker as a kit
  modal, the arrival chooser with its don't-ask box persisted under
  `[torrent] ask`, and a hand-off that stages the file into the player's
  temp folder — which doubles as the loop guard once an installer makes
  us the default app. `api/mod.rs` grew a hand-rolled multipart body
  (the API's only multipart; no reqwest feature) and the five calls,
  shared by the wasm build. 1-row fields rather than the kit's 3-row
  cards, so the whole form fits the 100×24 floor. 12 new room tests + 10
  parser tests + 2 multipart tests; smoked read-only against
  demo.mstream.io (the gate answers "No torrent client is selected" and
  the primary stays disabled with that reason on its tip).
- **Multi-server and the queue ✅ 2026-09-18** — contract-first
  (docs/ux-contracts/multi-server.md, contract 06, with the bundled-server
  addition). Six commits: every queued row carries its server and plays
  from it, so a switch keeps the queue and a removed server sweeps its
  rows; `--bundled-server` seeds the installer's server as the default
  and never offers to remove it; the queue and the place in it come back
  on launch, paused (`queue.json`, `[player] resume_queue`); Add next /
  Play now / row moves and a queue panel whose rows answer clicks;
  federated peers browsed through their parent, read-only, seated under
  it in the room and the dropdown, hidden or forgotten from there; the
  failure walk probes a row's server, retries, holds and resumes.
  Per-server tunnels (clause 38) and the guest-ticket direct path to a
  peer landed 2026-09-19 on the shared tunnel crate — T1–T3 below — and
  T4's two-server rig proved them live on 2026-09-20. A drag grip for
  reorder landed on 2026-09-21 with the track actions (the grip, `<`/`>`). **Offline switch (2026-09-20)**: a switch to a
  server that will not answer used to hide the header's picker and route
  every action into the TUI's connect screen (the GUI's transport died
  while queue clicks still played); now the label and picker stay, the
  rooms say the server is offline with "Try again", and the App gates on
  a `connect_screen` flag the GUI switches off — contract clause 13's
  deviation entry has the details.
- **Queue rows three cells tall — an experiment (2026-09-20)**: the
  panel's rows carry a 6×3 cover on the left — the card's own cover size;
  the wall's per-slot pixel/mosaic machinery, moved to `src/gui/cover.rs`
  and shared — the title over the artist over the album, the length on
  the last line where the hover [x] lands, and the kit's live scrollbar
  (`Act::QScrollBy`/`QScrollTo`, the wheel riding the same act) down the
  screen's last column; the ▸ is gone and the playing row is told by
  colour and weight. Covers are claimed through `App::fetch_queue_art` —
  once, from the row's own server, a row on a closed tunnel waiting
  unclaimed — and pixels stand down under any overlay (the dropdown, a
  modal). Began as two lines with a 4×2 cover, grown the same day for a
  bigger picture. `src/gui/queue.rs` with thirteen tests,
  `smoke/gui/scenario_queue.py` against Rig B. Costs to weigh: a third of
  the rows fit (five at 30 lines). A drag grip would take the cover's
  column or a cell of the air; still deferred.
- **A scroll no longer re-encodes the covers (2026-09-20)**: the first cut
  keyed the panel's slots by row position, so every wheel tick re-encoded
  every visible cover — and each encode decoded the full source jpeg,
  tens of milliseconds apiece in a debug build, paced one per frame: a
  second of mosaic before the pictures returned. Two fixes. The slots
  are keyed by the cover (`QueueUi.slots: HashMap<art id, Slot>`, a
  slack of eight past the view): an encoded picture draws anywhere for
  free — kitty by reference, sixel and iTerm2 by re-emitting their bytes
  — so a row's cover moves with it, an album shares one transmission,
  and only a newly revealed cover encodes. And `Graphics::draw` fits the
  box from the 128 px thumbnail first and encodes from it when it has
  every pixel the box can show (a 6×3 cover at 10×20 is 60 px; a 12×6
  wall cell 120), decoding the source only for a bigger box — which also
  gives huge covers that kept no source bytes their pixels at small
  sizes. Pinned by `a_scroll_moves_the_covers_with_their_rows_and_encodes_only_the_new_one`
  and `a_small_box_draws_from_the_thumbnail_and_a_moved_box_re_encodes_nothing`;
  **Overlays no longer blur the whole panel (2026-09-20)**: the first
  gate stood every cover down to the mosaic while any dropdown or modal
  was open (reported: the covers "turn blurry" and the picture's extent
  shifts). Overlays now register their footprint with the kit's
  `Surface` (`modal_frame_on`, the dropdown, the tooltip), and only a
  cover a footprint touched last frame draws as text — for that frame and
  the one after the overlay leaves, which is what repaints its cells.
  `cover_encode_costs` (ignored) prints the numbers: a 400 px jpeg's
  decode alone was 35 ms in debug and 2.8 ms in release; the 6×3 encode
  from the thumbnail is 4 / 13 / 5 ms in debug (kitty / sixel / iTerm2)
  and 0.28 / 0.56 / 0.22 ms in release, and a redraw microseconds — so a
  wheel tick now costs one of those, not seven of the old.
- **Auto DJ — contract settled 2026-09-20, implemented 2026-09-20/21 (slices A1–A5)**
  (docs/ux-contracts/auto-dj.md, re-extracted against the mobile record's
  `439b4de4`: songs per fetch, a peer hosting the DJ, readiness, the
  empty-queue openers; multi-server sessions out of scope; decisions 1–12
  recorded and confirmed 2026-09-20 — decision 10 rewritten to the
  record's model: the DJ is armed FOR a server and the session browses
  freely). Slices, each citing its clauses:
  - **A1 — the shared model ✅ 2026-09-20**: `player.autodj` becomes `autodj_server`
    (the identity the DJ is armed for) and `[player.dj]` the record's
    toggles (the migration table in the contract); `Similar` retired; the
    DJ's server apart from the session with its probe (clause 19), the
    record's toggle semantics (entry point 1), the lane with its epoch,
    the top-up guards (13), batches in `consume_dj` (14, 27), the request
    builder grown sources / length / keywords / `require*` / `limit` and
    the peer rule (20–27), the seed rule by origin (23), the opener (3),
    the one-shot seed (5), the empty-queue answer and `Capture::DjSeed`
    (2, 4, 16), disarm on removal. Pure tests on the request JSON and the
    lane.
  - **A2 — the worker ✅ 2026-09-20** (landed with A1): every pick through `client_for(reach)` for the
    DJ's server, the tunnel-target set grown by it while armed (19), the
    capability learner keyed by server identity with the version
    pre-filter from the probe (25, 50), the failure taxonomy (30–34), the
    tunnel defer and owed pick (33, 35), readiness from
    `features.discoveryReady` (36), the `[dj]` log lines (63).
  - **A3 — the GUI ✅ 2026-09-21** (two commits): the start chooser as a
    kit modal, the opening-song banner, the queue panel's badge and
    empty-state openers (2, 4, 16, 60, 62); then the Settings doorway
    (`Auto DJ ▸`, a LISTEN group) and the room (40–53) — the state line
    and Start/Stop, QUEUE · CONTINUITY · FILTERS · SOURCES as `[✓]` rows,
    ten-cell bars that set by cell and step by ←→, `(•)` radio rows, chips
    with a remove, the keyword field, the genre picker with its search
    line, the server picker that moves the DJ (41), the version gates and
    the sonic reasons (43, 50), Preview (53); a scrolling body with the kit
    scrollbar. One `DjEdit` on the App that both shells edit through;
    rating and genres written to the target server's own entry (51); the
    room probes the session's server when opened with the DJ off. Ten
    locales of `gui.dj.*` carried from the record's `autoDj*` (the
    `mstream_music` worktree's ARB files), the terminal's own strings
    beside them. 11 GUI tests; `dump_dj` (ignored) renders it all.
  - **A4 — the TUI and the web shell follow ✅ 2026-09-21** (mostly with
    A1: `A` arms / switches off / moves, the tab's rows are the contract's,
    the choosers draw over both screens, the wasm check passes; the tab's
    Keywords row now types a word on Enter — Enter adds it and clears for
    the next, Esc or Enter on nothing leaves, `x` on the row takes the
    last word back — through the same `DjEdit` as the room).
  - **A5 — the rig ✅ 2026-09-21** (`smoke/gui/scenario_dj.py`): the room
    off with the probe's sonic reason (no discovery data on the rig), Start
    on a one-row queue landing a batch of four badged rows, Preview's three
    picks, a bar set by a cell click live, the server picker moving the DJ
    to Rig A through B's proxy with the next turn picked from there, Stop
    leaving the rows. Not exercised on this rig: a direct (tunnel) peer
    lane and the `discovery is disabled` degrade — the rig has no
    discovery data at all, so sonic is never asked; both remain a manual
    check against a server with discovery on.
- **The Library rooms ✅ 2026-09-21** (docs/ux-contracts/library-rooms.md,
  extracted from the mobile browser and the webapp's genres, then built):
  Artists · Genres · Recent as rooms over the shared App's Library pane
  under the browse bar (`src/gui/library.rs`); an artist opens as the
  wall — `albums.rs` generalized over a `WallState` per room and the App's
  `artist_albums` beside `albums` — with the singles bucket as a card; the
  drilled album's Back names the artist; the kit's `letter_strip` (`#`
  A–Z, 25 rows, snapping, following the filter) on Artists, Genres and the
  Albums wall; a wall's card opens through the pane's own row so Back
  restores from the trail. Five GUI tests.
- **Track actions & the queue ✅ 2026-09-21** (docs/ux-contracts/
  track-actions.md, extracted from the mobile sheet, its queue and its
  rating, then built in two commits): the shared App gained rate-song and
  add-song with a latest-wins revert, the full block and playlist names
  through the track's own reach (`src/tui/app/track.rs`); the GUI gained
  the one sheet for a song wherever it is met — a row's `[⋯]`, a right
  click (the kit's context registry), `m`, the playing card — with its
  badge row (stars, key, tempo, lyrics), the picker with New playlist,
  Song Info, and the queue panel's grip drag, `clear` and keyboard reach
  through the App's focus (`src/gui/actions.rs`). Twelve tests across the
  App and the GUI.
- **Cleanup pass over the PR ✅ 2026-09-22** — four review lenses (reuse,
  simplification, efficiency, altitude) run over `main...HEAD`, then seven
  commits, each green on the suite, the wasm check and the rig: the
  placeholder remnants, stale allows and alias acts dropped; the frame's
  per-row copies removed (`put` writes cells, `clip` borrows, the sheet,
  the torrent room and the playlists list read in place, one client per
  reach kept in the worker); the shell's idioms as kit helpers
  (`Surface::hovers`, `wrap_words`, `letter_index`, `tall_frame`,
  `cursor_ring`, the glyph pairs, a track's facts on the types); one
  `ListView` + `PaneRow(List, row, verb)` + `ScrollBy/ScrollTo(List)` for
  every list; the App naming its states (`PlaylistNames`,
  `adopt_server(Session, ..)`, an exhaustive `ApiCmd::reach`,
  `for_each_copy`, the library door keeping its trail); one
  `SettingsRoom`, `bar_now` on demand, the queue's key gate ahead of every
  room, the path tidy shared with the wizard; the reply shapers shared by
  both workers. Behaviour changed only where a symptom had been patched:
  a focused queue no longer loses Esc/Enter to the album or library room,
  a card opens through the App's door (Activate resolved by focus and
  played the queue row), a zero bitrate no longer prints as 0 kbps.
  Follow-ups the review named and this pass left: Arm/Disarm/Move as App
  actions with prefs persisted as an effect (`autodj_server` from the bar
  toggle is saved at quit, the room's doors save at once); a note lifetime
  in place of nineteen hand-placed clears; the connect flow's session
  writes moving from `servers::observe` into the App; the sonic rebuild
  after a random pick moving into the App; the servers form on
  `tui_input::Input`; the DJ room's per-frame line model; the per-room
  offline gate; a `menu_row` kit widget for the nine hand-rolled
  selectable rows.
- **Keyboard hints as a setting ✅ 2026-09-22** — this surface is the
  pointer's, the classic TUI the keyboard's, so the footer of keys and
  the ` — key` tails on tooltips are off by default: `[gui] key_hints`,
  a DISPLAY row in Settings, the kit's `Surface::tip_keyed` cutting a
  keyed tip at its dash when the surface does not name keys (the wizard
  and the admin rooms always do). The keys keep working either way.
- **The player bar, A′ ✅ 2026-09-22** — drawn first on the "Player bar
  options" canvas (six ways to spend the bottom ten rows; A chosen, then
  the four asks: repeat at the left edge, the transport centred and bold,
  the card four rows tall). Five rows where there were eight: the seek
  line, three rows of tall frames — repeat alone at the left, the
  transport (prev · play · next in the text colour, bold) with shuffle and
  auto-dj centred in the span before the card — and a bottom row with the
  volume, the screen's note and the card's fourth line. The card's cover
  is eight by four cells (square), with title, artist · year, the spec
  line and the stars with key and tempo beside it — the sheet's facts, so
  a song reads the same wherever it is met. The content and the queue
  panel take the three rows back (two with keyboard hints on: the tips
  line sits under the bar and shifts it up). **Then H, the same day**, after
  a fat-filled play (F) had a turn on the canvas and the bar: auto-dj
  framed at the left edge; prev · play · next as one centred group, prev
  and next in the rounded frame and play in the THICK one
  (`kit::tall_frame_bordered`), all bold; shuffle and repeat worn bare
  after them — the glyph alone on the middle row, a frame-high click
  target. The fat fill went back out of the kit with nothing wearing it.
  **Then I**: the toggles framed again, repeat moved to the left of prev
  (repeat · prev · play · next · shuffle, auto-dj still at the edge), the
  play frame in GOLD, and the card's hover `[⋯]` removed — a right click
  or `m` is the playing track's sheet (track-actions deviation logged);
  auto-dj then padded three columns in from the edge.
- **The top bar and a first Now Playing screen ✅ 2026-09-22** — the
  wordmark gives way to two tabs, Library and Now Playing (`Screen`,
  `Act::Screen`, `0`/Esc between them; a nav digit is the Library's; the
  Now Playing tab was hidden 2026-09-27 — `0` alone opens the screen, the
  bar's tabs are Library and Stats). Now
  Playing (`src/gui/now.rs`) is the slot the contract's tabs will fill: the
  playing track's cover as large as the stage allows through the cover
  slot machinery, its title, byline, spec and facts beneath; the queue
  panel and the bar stand under it. Ahead of its contract (the mobile
  record's player panel, lyrics screen, visualizer and More sheet) — see
  the next slice.
- **The Auto DJ room moves to the Library nav ✅ 2026-09-23** — the
  record keeps Auto DJ among its browse root's feature cards and in the
  desktop rail's TOOLS group, never under Settings; the room's Settings
  doorway (`Auto DJ ▸`, the LISTEN group) was this surface's own
  translation, and it is retired with the room's `◂` back. The nav grows
  a TOOLS group under Search — Auto DJ, then Sonic path — and the Auto DJ
  row wears a `•` in the ok colour while the DJ is armed (the record's
  card line). The tenth room has no digit; `D` opens it (auto-dj
  contract: decision 5 amended, entry point 2, the deviations log;
  `scenario_dj` opens the room from the nav).
- **Widget polish — the text caret blinks ✅ 2026-09-23**: the GUI's
  eleven text fields (the filter line, the search box, the DJ keyword
  field and genre filter, the pairing code and the server form, the
  playlist and sonic-save names, the torrent form and its picker) blink
  their caret on the kit's own clock — `Surface::caret_touch` on every key
  and click, `Surface::caret` for the phase as a field draws,
  `caret_next_flip` timing the loop's next frame to the flip,
  `kit::input_display_blink` withholding the `▏` with its cell kept. A
  first cut lent the caret to the terminal's cursor as a blinking bar
  (DECSCUSR 5) and was replaced the same day: a terminal profile can veto
  that blink, and the ask was a caret that always blinks. The wizard,
  admin rooms and web shell keep the steady `▏` for now.
- **Bug: the genre switch stepped its mode ✅ 2026-09-23** — in the Auto
  DJ room a click on the checked Genre filter with Whitelist chosen landed
  on Blacklist, and only a second click switched it off, because the
  switch and the radios shared one three-way `genre_mode`. Decoupled the
  record's way: `genre_filter` (the switch) beside a two-way `genre_mode`,
  in `[player.dj]` and on a server entry (`dj_genre_filter`), old "off"
  values reading as the switch off; `DjEdit::GenreCycle`
  on the App, the TUI row keeping its three-state ←→ walk, its picker
  reading the DJ library's genres. Tests on the model, the App, the GUI.
- **The Auto DJ room re-laid ✅ 2026-09-23** — the canvas "Auto DJ room
  options" drew today's dense form beside six directions (descriptions
  wrapped, two columns, cards, tabs, quiet aligned columns, trailing
  controls); E "Quiet" chosen with two changes (a blank row between
  settings, the volume widget's bars). Built as drawn: every control at a
  value column, `(•)` choices side by side, `- ▰▰▱ +` bars with a step at
  each end, the status as one line with Start / Stop at its right, and a
  help line under the body for the row under the pointer or the cursor in
  place of the clipped descriptions. The room scrolls above the help line.
- **The pick banner moves into the browser ✅ 2026-09-23** — a sonic
  endpoint or the DJ's opening song being chosen showed its banner on the
  bar's note row, below where the choosing happened. Now a banner row in
  the accent stands over the room (or the Now Playing stage) with `[X]`
  to let the pick go, and the room steps down a row while the pick is
  armed; the "· Esc cancels" tail is cut when keyboard hints are off.
- **Last played and Most played ✅ 2026-09-23** — the webapp's two stats
  lists as two more LIBRARY rooms over the shared pane (library-rooms
  clauses 21–24): `LibraryNode::{RecentlyPlayed, MostPlayed}` through
  `stats/recently-played` and `stats/most-played` with the hundred, the
  TUI's root menu with them too, hidden for a peer, no strip, no digit.
  The nav grew to fourteen rows (Files and Search together at the top).
  The reporting that feeds them landed the same day (the next bullet).
- **Play reporting ✅ 2026-09-23** (docs/ux-contracts/play-reporting.md,
  the webapp's play sessions as the record): one session per song start
  folded from the engine's status — forward steps under three seconds
  count, a pause is counted, the end of a known length is a completion —
  posted to the plays' own servers (a peer's to its parent, with the peer
  id and a snapshot) from an outbox that persists in `stats.json` with
  the open session checkpointed, retried a minute after a kept batch and
  on every connect; a server without the Stats API gets the legacy
  thirty-second scrobble. `src/tui/app/stats.rs`, the worker's
  `ReportPlays` / `Scrobble` and its `[stats]` lines, the queue saver
  grown the second file. Last played and Most played now show this
  player's own listening (`scenario_stats.py` on the rig).
- **The highlight is the keyboard's ✅ 2026-09-26** — every GUI list lit
  its first row on arrival and a click lit the row it acted on, because
  the shared pane's cursor (the TUI's, always visible there) was drawn as
  is. The kit's list-cursor law (docs/ui-kit.md, "List cursor") now holds
  everywhere: `ListView.held` gates the highlight, the first walking key
  shows the cursor where the list rests it and the next moves it, a row
  click or Esc stows it, typing a filter or a query picks it up, and the
  row verbs (Enter `a N P d < > e x`) wait for it so no key acts on a row
  nobody can see. The queue, Files, Search, the Library rooms and their
  wall, Playlists and the sonic results follow (the saved-servers room
  keeps its lit row: its click is a choice its verbs line acts on);
  the tips line names how to pick the cursor up while it is stowed.
- **The Now Playing screen is the TUI's view ✅ 2026-09-26**
  (docs/ux-contracts/now-playing.md, the record turned from the mobile
  player panel to this repo's own full-screen view on direction): the
  facts column with the cover beneath, the tabbed panel (Queue · Lyrics ·
  Discover · Auto-DJ · Visualizer as the session offers them), the rule,
  the mirrored waveform band and the modes readout, drawn under the GUI's
  top bar by the shared `render_now_view` (`NowExtras` for the rows kept
  under the cover, the mosaic rule and the hints; `NowLayout` for where
  the tabs, the band and the cover landed). The one addition: prev · play
  · next in the bar's frames under the cover. The GUI's queue panel and
  bar stand down on the screen, the band seeks and lights under the
  pointer, the tabs click, the App's `fullscreen` flag follows the screen
  so the TUI's keys mean the same here (digits pick tabs, `0`/Esc back),
  and the visualizer gets its 33 ms cadence.
- **The adversarial review ✅ 2026-09-26** — eight reviewers, one per
  risk area, before the merge. Fixed at the root: room keys and the nav
  seat the App's pane and focus (the Files keys drove the last room's
  pane; a folded queue panel kept the keys); Settings sub-rooms and
  dialogs are gated by screen and outrank the queue panel's keys; the Now
  Playing screen serves the App's input modes; `put`/`clip` measure cells
  and the GUI boots the system language; the unplayable-row walk is a
  loop, not a recursion; Auto DJ tops up on a play's first status and at
  the queue's end; a session knows its stream, one kept server backs off
  alone, the browser flushes its outbox, repeat-one's laps are plays; the
  resume spot follows its track; a pasted code's Connected is a fresh
  session (no token carried over), a direct peer's 401 opens no sign-in,
  an unreadable credentials file is never written over, a peer launch
  entry resolves through its parent, the Windows hand-off skips cmd.exe.
  Logged for later: owed plays stamped with their account, URL-edit
  propagation to peers and rows, a peer re-login keeping the peer, the
  rating/details race, version-floor stripping, the saved-servers list
  scrolling, a GUI e2e leg.
- **The review's follow-ups ✅ 2026-09-26** — the second PR, everything
  the review logged for later: a session generation on the library and
  search asks (a stale reply fills nothing); a peer re-login signs in at
  the parent and keeps the peer; a URL edit carries the peers, the rows,
  the owed plays and the DJ along; owed plays stamped with their
  account; the rating/details race; the DJ's version floors trim the
  ask, the badge reads the body sent, a Preview's lane is its own, the
  migration reads the old mode; the Manage-servers list and dropdown
  scroll; footer hints under a hundred cells in every locale; glyphs out
  of the strings; the playback toasts keyed; the transport's widths; a
  Create button in the name box; the keyword row names its key; an art
  cache that evicts its oldest unpinned cover; a GUI e2e leg with the
  demo seat; the replay harness redacts secrets.
- **The Stats screen ✅ 2026-09-27** — the top bar's third tab hosts the
  stats page whole (docs/ux-contracts/stats-screen.md): the page grew a
  hosted mode (no header, no tips row — the bar and the footer carry
  those), the GUI builds its client from the App's reach (a tunnel's
  bridge, a peer's parent) and routes the keys and the pointer below the
  bar to the page's own surface; `T` from anywhere, Esc back. A new
  terminal window for the page was weighed and refused (the contract's
  log says why).
- **The mini player ✅ 2026-09-27** (docs/ux-contracts/mini-player.md) —
  below the 100×24 the screens need, the GUI no longer draws one dim line:
  it draws the playing track's cover (pixels or mosaic, the card's two
  paths), the song's words the card's way (title, artist, album · year,
  spec, stars · key · tempo), the bar's own seek line on top of prev · play
  · next in the bar's frames, and a line asking for room. The parts rank
  title, seek line, artist, a cover at all, album, the cover's size, then
  the rest, and the layout keeps the best-ranked arrangement that fits. The cover is stacked over the frames in a tall or narrow window and
  beside them in a short wide one, whichever draws it larger, never under
  the card's 8×4; below the frames' 22 columns the line stands alone, as
  before. Growing back returns the screen untouched. `src/gui/mini.rs`,
  ten locales, layout and render tests, and a pty run resized through five
  shapes with the play frame clicked.
- **Resizing at the terminal's pace ✅ 2026-09-27** — every page's frames
  (the GUI, the TUI, the wizard, the admin rooms) are encoded into memory
  and written in one go once the frame is done, fenced in DEC mode 2026
  so a terminal that knows the mode shows the frame whole: the clear a
  resize opens with is never painted on its own, and a frame that changes
  nothing writes nothing (`src/kit/frames.rs`). In kitty itself, with no
  multiplexer between, a resize no longer sends every cover again: kitty
  keeps a transmitted picture through a resize and the clear after it
  (graphics.c spares virtual placements), so the cache stays
  (`Graphics::refresh`). Measured on a pty, the Albums wall in truecolor,
  from the resize to the last byte: the mosaic 1.7–2.8 ms → 0.8–1.2 ms;
  kitty 15–18 ms and ~870 KB → 2–3.5 ms and 17–30 KB; iTerm2 and sixel
  as before, 5–16 ms (their pictures live in the cells, and a clear takes
  them). After a walk through six sizes the screen matched a fresh start
  cell for cell on every protocol.
- **The Admin tab ✅ 2026-10-02** — the top bar's third tab hosts the
  admin panel's six rooms (docs/ux-contracts/admin-screen.md, B4 above):
  a hallway at the left, the room beside it on the App's reach as the
  Stats tab builds it, and the server's log docked as a column, as a band
  under the room, or as a room of its own, as the window allows. `M` from
  the Library, Stats and Now Playing; Esc back.
- **The MP3 Player tab ✅ 2026-10-04** — the top bar's fourth tab hosts
  the firmware page of `mstream-player device flash` with no flags
  (docs/ux-contracts/mp3-player-screen.md; Phase 13), in place of the
  tray launcher's dropped menu item: the page grew a hosted mode (no
  header, no tips row, nothing drawn past its area, Done's extra words
  giving way before its Close at the floor), the tab builds it on entry
  and lets it go on leaving — its worker told to let the board go, the
  page kept aside until it has, a quick return waiting for it — except
  while it writes, when every key but Ctrl+C is the page's, the top bar
  is inert and no screen change is honoured; a quit with the board held,
  or being reached, waits up to eight seconds for its restart. The worker
  hears Quit early (before a port, between baud rungs, once the board
  answers) and lets a board go with no boot-line listen, which the
  command's Esc gains too. The footer is the page's whatever the key
  hints say; the mini player hides the page's keys and says when it
  writes. `K` from the Library, Now Playing, Stats and the Admin hallway,
  `K` or Esc back (`F` was first; Caps Lock on the filter key's `f` reset
  a plugged-in Core2). The server label is cut to keep off the strip,
  tested at 100 columns in every locale.
- Next slices, in rough order: the shared view's Lyrics tab (the TUI's
  placeholder today), Discover's room and its "Play a path to…" entry (revisits the
  sonic contract's §5 search-skip; Find similar re-enters the sheet with
  it), the Rated list (ratings exist now), then e2e legs (fake server
  needs player endpoints).

#### Per-server tunnels and guest tickets — the plan (2026-09-18)

The two deferrals the multi-server contract logged, taken together because
they are one mechanism: **a tunnel registry that outlives the session**.
Clause 38 (a Quick Connect server's tunnel stays up while the queue
references it) needs bridges keyed by server instead of the api thread's
single slot; clause 27's direct sentence (a peer with a guest ticket gets a
tunnel of its own) is a second kind of credential dialled into the same
registry. Record: `ServerManager` in `mstream_music`
(`lib/singletons/server_list.dart` — `_tunnelTargets`, `setQueueIrohServers`,
`ensureTunnels`, `_ensureHandle`, `_refreshDirectAccess`, `_maintainDirect`,
`onDirectAuthRejected`), `lib/singletons/tunnel_policy.dart` (the
constants), `lib/objects/direct_access.dart` (the payload),
`lib/util/stream_url.dart` (the URL shapes). Server side, read at mStream
`c791799a` (6.28.0): `docs/federation-guest-ticket.md`,
`src/api/federation-browse.js` (the access route), `src/state/federation.js`
(the federation endpoint's accept loop), `src/api/federation-auth.js` (what
a guest may call), `src/api/server-info.js` (`federationDirect`).

**Where we stand.** `api_loop` (`src/tui/worker.rs`) owns one
`TunnelBridge` and one `(local_url, id)` pair; `ApiCmd::QuickConnect` dials
and connects in one step and keeps the old bridge until the new one answers
(`tunnel_answered`); `resolve_target` maps the loopback address back to the
identity for a login. The App mirrors the single slot as `open_tunnel`, and
`reach()` (`src/tui/app.rs`) answers a queued row on any other tunnel with
"is not connected — its tunnel is closed", which the failure walk turns
into a skip with a word. A peer always rides its parent's proxies with the
parent's token. Nothing dials for a row; only a switch dials. Two things
stay as they are: every bridge binds its own iroh `Endpoint`
(`Tunnel::open` → `bind_endpoint`), and the `Redialer` inside a bridge
re-dials a dead QUIC connection on the same loopback port, so a URL built
on that port survives a network blip. What changes is who owns bridges and
how many. One more fact that shapes the design: `Event::Connected` is a
full re-open (the browser goes to the opening path, art and the sonic path
clear), so a transport swap under a live session needs a command of its
own, not a second connect.

**The tunnel core is shared** (decided 2026-09-18). The mobile app's
`rust/iroh_tunnel` crate (`crate-type = ["cdylib", "rlib"]`, ~1.3k lines
of core under a thin C ABI, iroh 1.1.0) already does what T1 and T3 would
have grown here: both credential kinds (`PairingKind` — a `mstr1:` code, a
`mstrfedg1:` guest ticket dialled on the federation ALPN with the JWT on
the first bi-stream), `Tunnel::set_credential` in place on the same port,
a reconnect supervisor with `STATUS_{CONNECTING,CONNECTED,RECONNECTING,
REJECTED,DOWN}` on a watch channel, `path_kind`, `force_reconnect`,
`nudge_network`, an events ring, and a per-tunnel loopback token every
local request must carry as `__lt=<token>` so another process on the
machine cannot use the bridge as a proxy. Its interop harness
(`interop/harness.mjs`, a replica of the server's endpoint on
`@number0/iroh` 1.1.0) covers JSON, Range, concurrency, the kick and the
GUEST phase (dial, refusal, swap), and the mobile federation rig proved
the guest path live on 2026-09-05. One wire implementation for every
client — the mobile app, this player, and third parties through the C
ABI and the dev CLI — is the decision; the crate moves to its own
repository first (`IROH_TUNNEL_CRATE_PLAN.md` in the mobile repo), and
this player consumes it as a Rust dependency, never through the C ABI.
The player keeps what is its own: the identity conventions
(`mstream+iroh://`, `display_server`), the book, the App-side policy, and
the staged `quickconnect-probe` until the crate exposes its stages.

**The shape.**

1. **The keeper** (worker side, mechanical). A registry of the crate's
   `Tunnel`s keyed by identity — `mstream+iroh://<endpoint>` for a Quick Connect server,
   `mstream+peer://<id>@<parent>` for a direct peer — shared between the
   api thread and one thread per dial (`Arc<Mutex<HashMap<String, Slot>>>`;
   a cold dial can take ONLINE 8 s + DIAL 25 s + HANDSHAKE 15 s and must
   never sit on the api thread, where connection commands are serialized).
   Three commands, four events:
   - `ApiCmd::TunnelOpen { id, credential }` — a no-op when the id is up or
     dialling; else a dial thread that installs the bridge and answers
     `Event::TunnelUp { id, local_url }` or
     `Event::TunnelFailed { id, rejected, why }` (`rejected` = the server
     said NO — a rotated code or a refused guest token; everything else is
     unreachable).
   - `ApiCmd::TunnelClose { id }` — drop the bridge (its `Drop` closes the
     listener and the connection); `Event::TunnelClosed { id }`.
   - `ApiCmd::TunnelCredential { id, credential }` — the crate's
     `set_credential`: same port, same URLs; applies at the next re-dial,
     at once for a supervisor that gave up on a refused token.
   - `Event::TunnelPath { id, path }` and `Event::TunnelStatus { id, status }`
     from one sampler over the registry, polling `status()` and
     `path_kind()` on today's cadence and reporting changes — so the App
     sees a live tunnel go reconnecting, rejected or down without waiting
     for a request to fail.
   The api thread's `bridge`, `tunnel`, `resolve_target` and
   `tunnel_answered` retire. `ApiCmd::Connect` and `Login` gain
   `identity: String` (what the session is filed under — today derived from
   the loopback address), and `ApiCmd::QuickConnect` and
   `Event::TunnelReady` retire: the App composes *open, then connect at the
   loopback*. A fourth command, `ApiCmd::Retarget { identity, server, token }`
   → `Event::Retargeted { identity, server }`, swaps the session's client
   for the same identity after a `GET /api` through the new base answers;
   the App updates `session.server` and `session.token` and nothing else.
2. **The policy** (App side, pure, on the fake clock — the `Stall` probe's
   pattern). `App.tunnels: BTreeMap<String, TunnelState>` replaces
   `open_tunnel`; `TunnelState` is `Dialling { since } | Up { local_url,
   path } | Down { failed_at, attempts } | Rejected`. `tunnel_targets()` is
   recomputed after every queue edit, switch, restore, removal and tick:
   the session's transport plus every queued row's transport (a Quick
   Connect row → its own id; a peer row → its parent when the parent is a
   tunnel server, and its own id once T3 makes it direct).
   `reconcile_tunnels(now)` (from `tick_at`) opens what is wanted and
   absent — on the cold-dial ladder — and releases what is unwanted **after
   a 10 s grace** (a release deadline per id, cancelled when the id is
   wanted again: a restore, a clear-then-refill and the launch's empty
   queue all pass through "nothing queued" for a moment; the record tore a
   launch tunnel down mid-dial before it had the grace). A `Rejected` id is
   never re-dialled automatically; a user switch or a re-pair is what tries
   again.
3. **Identity on the wire.** The credential a target dials with comes from
   the book: `KnownServer` gains `pairing: Option<String>` (seeded from
   `Credentials.pairings` by `known_servers`), so `Session.tunnel_code` and
   the GUI's `pairing_for` lookup in `switch_to` collapse into it. A guest
   ticket is runtime-only — the record never persists one, and nothing
   here should either.
4. **Direct peers.** A parent whose capability payload says
   `federationDirect` is asked for each peer of it that is a target:
   `GET /api/v1/federation/peers/:id/access` with the parent's token, on the
   parent's plain client (never through `with_peer`'s rewrite). Granted →
   the peer's identity is opened with the `mstrfedg1:` ticket — ALPN
   `mstream/federation/1`, the JWT as the first bi-stream, `OK` back — and
   from then on the peer answers plain `/media`, `/album-art` and
   `/api/...` at its loopback with the guest token in the ordinary slots.
   Denied (`direct: false`) → the proxy for the rest of the session.
   Failed (a 502, a malformed 200) → the ladder. The ticket is asked for
   again at three quarters of its life and swapped in place; a 401 on the
   direct path asks once more with `?refresh=1`, then falls back.

**Wire facts** (read from the sources above; the shared crate's unit tests
and harness pin the credential, ALPN and handshake rows — this player's
tests pin the rest):

| | Quick Connect | Direct peer |
|---|---|---|
| Identity | `mstream+iroh://<endpoint>` | `mstream+peer://<id>@<parent>` (in the config since the peer work) |
| Credential | `mstr1:` pairing code — `{t: EndpointTicket, s: 32-byte secret}` | `mstrfedg1:` guest ticket — `{t: the PEER's federation EndpointTicket, g: a JWT}`; unknown fields ignored, a missing `t`/`g` rejected, a newer version says "update the player" |
| ALPN | `mstream/tunnel/2` | `mstream/federation/1` |
| First bi-stream | the secret bytes, `finish`, read `OK` | the token's UTF-8 bytes, `finish`, read `OK`; a refusal is `NO` or a close with reason `unauthorized`/`backoff` |
| Later bi-streams | one TCP connection each into the server's HTTP port | the same, into the peer's HTTP port |
| Auth per request | the user's JWT (`x-access-token` / `?token=`) | the guest JWT in the same slots; claims `{federationGuest, federationKeyId, iat, exp}`, no `username` |
| What answers | everything | the key's allowlist (`federation-auth.js`): `GET /api`, `GET /api/`, the db reads, `POST file-explorer`, `…/recursive`, `…/m3u`, `random-songs`, `federation/health`, `discovery/similar`, and the `/media/`, `/album-art/` GET prefixes — no ping, playlists, transcode or waveform |
| Lifetime | until re-paired | the token's `exp` (24 h by default); the parent re-mints past 75 % of it, or on `?refresh=1` (served from cache within 5 s of a mint) |
| Revocation | rotating the connect secret | deleting or expiring the parent's key on the peer — every guest of it fails at its next handshake and its next request |

The access route in full: `GET /api/v1/federation/peers/:id/access[?refresh=1]`
→ `200 {direct: true, endpointTicket, endpointId, guestToken, expiresAt,
directTicket}` · `200 {direct: false, reason}` · `502` when the peer cannot
be reached for the mint. `federationDirect` beside `federationBrowse` in
`GET /api/` is the key's presence (the build has the route) and its value
(there is a peer to reach); whether a given peer cooperates is only known
from the access route. Both flags are caller-scoped, so they are read from
a ping made with the parent's token — the session's own, or a one-shot
`GET /api/` with the token for a saved parent that is not browsed (the
`Probe` command grown a token and the flag).

**The constants** (the record's `TunnelTiming`, kept as named consts in
`app.rs` so the tests can read them): queue release grace **10 s**;
cold-dial retry ladder **5, 10, 20, 40, 60 s**, then **5 min** after the
tenth failure; direct ticket refresh at **0.75** of its life; **5 min**
between failed refreshes of a stale ticket, **60 s** after a refusal, at
once when the ticket has already expired; the switch spinner's bound
**12 s** before the header says it is still connecting (the record's
`awaitTunnelReady`). Not ported: the record's network-change hooks
(`retryAfterNetworkReturn`, the probes, the watchdogs) — this player has
no connectivity events; the App's ladder covers cold dials and the crate's
supervisor covers a live tunnel that drops (its own backoff, cut short by
`force_reconnect` or the relay coming back).

**Slices** (each a commit; `cargo test`,
`cargo check --target wasm32-unknown-unknown`, clippy on the new code).
The shared crate exists: <https://github.com/IrosTheBeggar/mstream-iroh-tunnel>,
tag `v0.1.0` (2026-09-18 — the crate as it left the mobile repo: ABI 2,
iroh 1.1.0, MIT). T1 depends on it by git tag until it is on crates.io:
`mstream-iroh-tunnel = { git = "https://github.com/IrosTheBeggar/mstream-iroh-tunnel", tag = "v0.1.0" }`
(the `c-abi` / `os-trust` features arrive with E3, so at v0.1.0 the C
symbols are compiled in — harmless, and the OS trust store is not yet
selectable through the crate).

- **T1 — the keeper, on the shared crate ✅ 2026-09-18.** As planned, with
  two settlements: iroh is pinned at 1.1.0 (the crate's tested line —
  `cargo update` had reached for 1.2.0), and an open request for a tunnel
  the worker already holds re-reports it rather than staying silent, so
  the App's picture can never lag behind a dial that will not happen. The
  code is parsed for its identity before anything is dialled, which is why
  the tests type a real-shaped code (`quickconnect::testing::sample_code`).
  789 tests; the wasm build checks. **E3 followed on 2026-09-19** (crate
  v0.2.0): the probe rides `connect_tunnel_staged` and prints the crate's
  stages, a refused code is `DialError::is_rejected()` rather than a word in
  the error text, `parse_code` is `iroh_tunnel::inspect` (a guest ticket is
  refused as not a pairing code), the crate is pulled without its C ABI and
  with `os-trust`, and `iroh` itself is a dev-dependency only — for the
  tests' fake server endpoint. `Cargo.toml`: the crate under
  the non-wasm target dependencies — at v0.1.0 as is; from E3 with
  `default-features = false` (no C symbols in the player binary) and its
  OS-trust feature on (the `platform-verifier` pin this player carries
  for the corporate trust store, which the player's own `iroh` feature
  line keeps enabling meanwhile through feature unification); iroh moves
  from 1.0.3 to 1.1.0 with it. `src/quickconnect.rs`
  shrinks to what is the player's own — `TUNNEL_ID_PREFIX`, `is_tunnel_id`,
  `display_server`, the identity read off the ticket (`server_id`,
  `endpoint_label`), and `probe`, which keeps the old staged dial until the
  crate exposes its stages (E3) and then moves over; `Tunnel`, `Redialer`,
  `TunnelBridge`, `open_bridge`, `handshake` and the accept loop go.
  `src/tui/worker.rs`: the registry holds `iroh_tunnel::Tunnel`s;
  `TunnelOpen { id, credential }` runs `connect_tunnel(&credential, 0)` on a
  dial thread over the player's runtime (`runtime::runtime()` made
  `pub(crate)`, since `set_credential`, `force_reconnect` and
  `begin_shutdown` take a `&Runtime`) and answers `TunnelUp { id, local_url,
  local_token }` or `TunnelFailed { id, rejected, why }` (`rejected` read off
  the error text, the harness's contract, until E3 types it); `TunnelClose`
  → `begin_shutdown`; `TunnelCredential` → `set_credential`; the sampler as
  above, and the crate's events ring drained into the player's log ring at
  info. `Connect` / `Login` gain `identity`; `QuickConnect`, `TunnelReady`,
  `resolve_target`, `tunnel_answered`, the api thread's `bridge` and
  `tunnel` retire. **The loopback token**: `Reach` gains
  `local_token: Option<String>`; the builders in `src/api/urls.rs` append
  `__lt=<token>` to every URL shape (the record's `localTokenQuery`), and
  `Client` gains a query pair it adds to every request, set when a session's
  base is a bridge — the engine's stream client needs nothing, the token is
  in the URL. `src/tui/app.rs` + `app/session.rs`: `tunnels` replaces
  `open_tunnel`; `begin()` for a tunnel server connects at the loopback when
  the tunnel is up and opens it first when not (`connecting` shows
  meanwhile); `TunnelUp` for the identity the session waits on → `Connect`;
  `NeedsLogin` at a loopback address → the sign-in form as `TunnelReady` did;
  `TunnelFailed { rejected: true }` → the "rejected — it may have been
  rotated" line; `tunnel_path` only from the session transport's id.
  `src/tui/mod.rs`: `known_servers` seeds `pairing`. `src/gui/servers.rs`:
  `switch_to` stops looking the code up. `src/web/api_worker.rs`,
  `src/web/mod.rs`: stubs (`TunnelOpen` → `TunnelFailed { rejected: false,
  why: "tunnels need the native player" }`). Tests: the three
  tunnel-session tests in `app/tests.rs` re-anchored on the composition;
  the worker's `resolve_target` / `tunnel_answered` tests retire; new:
  open-then-connect, a rejected dial's wording, the token on every URL
  shape and on the api client, a status change reaching the App. The
  envelope, ALPN, handshake and swap tests live in the crate. Size S–M: the
  crate carries the tunnel; what is left is plumbing and the token.
- **T2 — tunnels follow the queue ✅ 2026-09-19** (clause 38, and clause 37's
  tunnel step). As planned; the policy lives in `tick_at` (`reconcile_tunnels`
  every tick, the stall probe after it), the ladder's bookkeeping sits beside
  the registry (`tunnel_retry`, `tunnel_release`), and a held row keeps its
  restored spot for when the tunnel comes up. One settlement: the session's
  own tunnel is re-dialled on the ladder after a failure, but the browser is
  not reconnected on its own — the user saw the error. 798 tests. Plan text: `tunnel_targets()` and `reconcile_tunnels(now)` as above;
  `TunnelState::Down` carries the ladder. The failure walk gains a hold
  beside `Stall`: `play_index` on a row whose transport is a tunnel that is
  wanted and not `Rejected` does not skip — it parks on the row, paused,
  with the line **"Connecting to %{server}…"**, and `TunnelUp` for that id
  re-runs `play_index` (the record: recover in place — ensure that server's
  tunnel, re-seed, resume; don't skip). `TunnelFailed { rejected: true }`,
  a server with no pairing code and a server gone from the book skip with a
  word as today. Reads that belong to a row ride the row's reach:
  `ApiCmd::AlbumArt { file, reach: Option<Reach> }` and
  `Waveform { filepath, reach }` — `answer()` builds a one-shot client from
  a given reach (the `Probe` pattern), so a mixed queue's now-playing cover
  comes from its own server instead of the browsed one (clause 30's art
  sentence, unmet today). The crossfade announcement re-derives on the
  trailing refresh after `TunnelUp`, so a next row whose tunnel came up
  late still prefetches. Launch: the restored queue's tunnel servers are
  targets as soon as `restore_queue` runs, so the restored spot's server is
  dialling while the user reads the screen (the record pre-warms the resume
  server the same way). Removal and Forget: the sweep drops the rows, the
  target leaves, the release runs after the grace. Contract: clause 38's
  deviation closes; the status row updates. Tests (fake clock): targets
  from the session and the rows; a row added inside the grace cancels the
  release; the ladder's delays; a hold resumes on `TunnelUp`; `Rejected`
  skips with the word; a switch away keeps the queue's tunnel; the last row
  leaving releases after 10 s and not before; art asked with the row's
  reach. Size M.
- **T3 — guest tickets ✅ 2026-09-19** (clause 27's direct sentence). As planned,
  with the crate carrying the handshake. Settlements: the ticket's times are
  read off the guest JWT's own `iat`/`exp` (no date parsing; the peer's wall
  judges by the same claims); `Retarget` is a fourth connection command,
  since a `Connected` re-opens the browser; the record's `federationMissing`
  gate is not ported (a missing peer's access call fails at the parent and
  waits for the gap); a peer session that goes direct keeps `session.peer`,
  so the read-only rules and pinned capabilities stand. The GUI's mark is
  "· direct" beside "via {parent}" (`gui.srv.direct`, ten locales). Unverified
  live until T4's rig: the whole path ships on the crate's harness and the
  App's tests. Plan text: `src/api/types.rs`:
  `Ping.federation_direct` (`federationDirect`),
  `Capabilities.federation_direct`, `DirectAccessAnswer` (the three
  shapes; a 200 missing fields is transient, not a refusal).
  `src/api/mod.rs`: `federation_access_async(id, refresh)` on the plain
  client. `src/tui/worker.rs`: `ApiCmd::DirectAccess { parent, id, reach,
  refresh }` → `Event::DirectAccess { parent, id, answer: Granted(DirectTicket)
  | Denied(reason) | Failed(why) }`. `src/tui/app.rs`: `direct: HashMap<peer
  identity, DirectState { ticket, token, endpoint_id, expires_at,
  fetched_at, denied, last_ask, last_failure, refused: Option<String> }>`
  and `direct_offered: HashSet<parent>`; a peer target whose parent offers
  direct, not denied, not missing, asks when it holds no ticket, a stale
  one (≥ 0.75 of its life), or the one the peer refused — rate-limited as
  above; `Granted` → `TunnelOpen` with the `mstrfedg1:` ticket as the
  credential (the crate parses it and dials the federation ALPN), or
  `TunnelCredential` when the tunnel is up (the crate's in-place swap); the parent stays a target while
  a peer's ticket is due and the parent is itself a tunnel server (the
  access call rides it). `reach()`: a peer whose own bridge is `Up` →
  `Reach { base: local_url, token: guest, self_signed: false, peer: None }`
  → `media_url` / `album_art_url`; else the parent proxy as today.
  Session: a switch to (or a launch on) a peer whose bridge is up connects
  at its loopback with the guest token and `identity` = the peer id,
  `session.peer` kept so the read-only rules and the pinned capabilities
  still apply; `TunnelUp` for the *browsed* peer → `Retarget` to the
  loopback (the record flips a peer's URL shape exactly when its tunnel
  does; the browse stack stays); `TunnelClosed` or a denial → `Retarget`
  back to the parent's proxy base with the parent's token. A 401 on the
  direct path (an `Unauthorized` from the session client, or a 401 open
  failure on a row whose reach was direct — `transient_failure` must not
  eat it) asks the parent once with `refresh=1`, swaps the credential and
  retries the row; a second refusal falls back to the proxy for the
  session. A refused guest handshake surfaces as `TunnelFailed { rejected:
  true }` or a `Rejected` status on a running tunnel, and both mean
  "refresh through the parent", never "re-pair". Wording: the servers
  room's peer row gains **"· direct"** while its own tunnel is up; the hold line from T2 is reused; the ten locales.
  Contract: clause 27's direct sentence and the "direct access not ported
  (3)" deviation close; the wording table gains the rows; the Server API
  row already lists the route. Tests: the three answers parse; one ask per
  target, not per tick; stale → refresh, in place; denied → no re-ask this
  session; refused → one refresh then the proxy; the two reach shapes;
  `Retarget` keeps the browse stack; the 401 path. Size L — most of it
  policy, all of it testable offline.
- **T4 — the rig and the docs ✅ 2026-09-20.** Run as planned on two scratch
  servers (:3040 Rig A, :3041 Rig B; iroh and federation on; paired with
  two curls in public mode — the recipe is in the memory note), four legs:
  the shared crate's dev client dialled A's federation endpoint with B's
  guest ticket (path direct, mode guest; playlists 403 off the allowlist; a
  request without `__lt` dropped); `quickconnect-probe` passed against B's
  code through the crate; the `#[ignore]` worker test
  `rig_a_peer_is_reached_directly_with_a_guest_ticket`
  (`MSTREAM_RIG_PARENT=… cargo test -- --ignored rig`) ran the real api
  thread from connect to a 206 byte range off A's loopback with the guest
  token; and the GUI on a pty (`scratchpad/ptygui.py`, two scenarios)
  switched to Rig A, went direct within a tick or two ("Rig A · direct ·
  via …", A's log `authorized (guest, key 'rig-b')`), played "6AM" from A's
  own tunnel, paired B a second time over Quick Connect, queued rows from
  both servers, kept the tunnel-server row playing across two switches,
  kept both tunnels up on the standard server, and released each ~10 s
  after its last row left — `tunnel …: closing — nothing references it` in
  the ring, `guest connection closed` on A, `tunnel connection closed` on
  B, the room's mark gone. Two bugs the rig found, fixed in the same
  commit: the App's book of servers was not refreshed when the peer
  reconcile wrote the config (the header showed the peer's raw identity),
  and a peer's row asked its direct tunnel for a waveform, which the
  allowlist refuses (the rule now keys on the row's origin, not its
  transport). Cosmetic: the dropdown clipped the mark at 44 columns (now
  60, mark first); the "reaching…" note outlived the switch; the log
  scrubber redacted `id@parent` in a peer identity as if it were userinfo
  (spelled out now). Not run: a Netskope-shaped network. Plan text: Two scratch servers from the
  `local-mstream-scratch-server` recipe with `iroh.enabled` and
  `federation.enabled` on both (the secrets self-generate;
  `federation.serverName` for labels); pair them with the player's own
  room — `mstream-player admin federation` mints on A and pastes on B —
  then: the player on B lists A as a peer with `federationBrowse` and
  `federationDirect` true; the access route mints; the player dials A's
  federation endpoint and a queued row from A plays from
  `127.0.0.1:<port>/media/...?token=<guest>`; B's own Quick Connect code
  (`GET /api/v1/iroh/code`, admin) pairs B as a tunnel server, rows from
  both are queued, a switch to a standard server keeps both bridges, and
  removing the last row of each releases it 10 s later (the ring says so
  at info). `quickconnect-probe` stays the diagnostic for a code; a
  `federation-probe <ticket>` sibling is cheap if the guest handshake needs
  staging. PLAN and the contract's status row. Size S, plus whatever the
  rig teaches.

**Open questions (leans).**

1. *Where the policy lives* — the App, on the fake clock (lean), or the
   worker with timers. The App: the ladder, the grace and the ticket
   schedule are then unit-tested like the `Stall` cadence, and the worker
   stays a dialler.
2. *A hold that never ends* — a row waiting on a tunnel whose ladder keeps
   failing: hold indefinitely (lean; the record's playback path "would
   rather wait than fail", the line says what it waits on, and a skip is
   one key) or time out into the skip after the ladder's first long delay.
3. *The browsed peer going direct mid-session* — `Retarget` when its tunnel
   comes up (lean; the record's behaviour) or only at the next switch.
4. *One iroh endpoint per tunnel* — settled by the crate, which binds one
   per `connect_tunnel`; N relay connections for N tunnels is fine for a
   handful, and a shared endpoint would be the crate's optimisation, not
   this player's.
5. *Dial a peer direct when its parent is a standard server* — yes (lean;
   the record dials whenever the parent offers it and the peer has not
   declined: the bytes stop crossing the parent's link twice either way).
6. *A per-row tunnel mark in the queue panel* — no (lean); the hold line
   and the servers room's "· direct" are enough until a listener asks.

**Out of scope.** DJ fan-out over tunnels (this player's DJ is pointed at
one server, clause 35); downloads and the offline copy; the record's status
strip and Repair sheet (the servers room's re-pair path stands); the wasm
build (no iroh — the stubs say so).

**Risks.** The guest handshake is no longer this player's to prove: the
crate's harness GUEST phase covers it against `@number0/iroh`, the mobile
rig covered it live, and T4's rig (2026-09-20) proved the whole path from
this player. What the switch does put at risk: the crate's supervisor replaces
the `Redialer` whose single-flight re-dial and 4 s cooldown fixed requests
stacking up behind a dead link — read `supervise` and the bridges' wait on
the status watch before deleting the old code, and keep the "tunnel is
flakey" listening test in the smoke round. iroh 1.0.3 → 1.1.0 is a
lockfile bump the mobile side already made (it cleared four audit
advisories) but it is new to the six release targets here. Two endpoints on
one Mac should connect over the ticket's direct addresses without a relay;
on the Netskope network the relay wait costs 8 s per dial and nothing more
(a Known-risks line until seen). `runtime::block_on` runs dials on the
shared multi-thread runtime, so concurrent dial threads are fine.

### Phase 11 — The visualizer window: the mobile app's presets, on the desktop

> **Status 2026-09-22: 11.0 is done** — the presets parse, translate, compile for every
> backend and draw on a real GPU, and the audio texture matches Android's to the byte. 11.1,
> the window, is next. Feasibility was probed before a line was written: a winit window
> opened from a terminal-launched process, fronted and drew on macOS 26.6, and every pass of
> the mobile app's nine presets compiled through naga for Metal, HLSL and GL. Those scratch
> prototypes are not in the tree; what they proved is below.

The terminal visualizer (`tui::viz`) tops out at half-block resolution, and a picture that
fills the panel defeats `Canvas::into_lines`' run merging — every cell its own fg/bg pair,
~6 MB/s of escape bytes at 30 fps on a 120×40 panel. The mobile app's visualizers are GPU
fragment shaders, and none of that is a terminal's job. A terminal program can open a window
the way games do: winit for the window and its input, wgpu for the pixels.

**What the mobile app has** (`IrosTheBeggar/mstream_music`, public, GPL-3.0). Android runs
two engines: projectM v4 over JNI with 120 bundled `.milk` presets, and a ShaderToy-convention
ShaderEngine (`android/app/src/main/cpp/shader_engine.cpp`) running nine presets from
`assets/shaders/*.glsl`. iOS and the Flutter desktop build run SkSL ports of eight of them
(`shaders/visualizer/*.frag`); 09 is SkSL-infeasible — it passes samplers to functions and
loops on runtime bounds. The single-file `.glsl` is the canonical format: `// === pass:
<name> ===` sections (buffers A–D run before `image`; a `common` pass is shared code), `//
=== channel <pass>.<n> = <source>` and `// === size <pass> = WxH` lines in the header, `//
param: <name> <min> <max> <default>` tunables (the i-th line is `iParams[i]`, at most eight),
and title/author/license/description lines. The audio contract is `audio_texture.cpp`: a
512×2 R8 texture, row 0 the spectrum and row 1 the waveform — mono mix, Hann-1024 normalized
by 2/Σw, a linear-domain EMA of 0.27, and a dB window of −69.7…−20.7 mapped onto 0..255.

**Decisions.**
- **The presets are consumed verbatim, not ported.** Parity with mobile means running the same
  files: vendored, pinned by commit, edited only where a fix belongs upstream anyway.
- **wgpu, not a CPU framebuffer.** 07/08 are 11 KB raymarchers and 09 is 766 lines. A
  softbuffer prototype drew 68 fps at 1280×960 — one cheap shader in an unpaced loop pinning
  a core; it proved the window, not the renderer.
- **The window is a child process:** `mstream-player viz-window`, a hidden subcommand of the
  same binary. AppKit demands the process's first thread for the event loop — winit has no
  `with_any_thread` on macOS — and the TUI owns that thread. The child is also the crash
  boundary: naga panics are real (below), and one that escapes must cost the window, not the
  player.
- **The parent computes the texture and pipes it.** The tap feeds `shader::audio` in the
  parent; 1 KB frames plus control messages go down the child's stdin, and the child only
  renders. EOF on stdin means quit, so a parent crash closes the window for free. The child's
  stderr goes to the log, never to the TUI's terminal (it would scribble over the alternate
  screen); Windows spawns it with `CREATE_NO_WINDOW`; the feeder drops frames rather than
  block. `serve` reuses the same host side (11.3).
- **projectM is not in this binary.** If it comes, it comes as an optional companion speaking
  the same pipe; mobile has already built `projectM-4.dll` v4.1.6 + GLEW + a WGL render shim
  on Windows.

**Proven 2026-09-20..22** (scratch prototypes on the dev Mac):
- **The window.** winit 0.30 + softbuffer from a terminal-launched process: 47 crates, a
  5.9 s build, 1.0 MB, system frameworks only on macOS. Without `focus_window()` the window
  opens behind the terminal; with it the process is frontmost on macOS 26.6. The NSOpenPanel
  lesson in `Cargo.toml`'s picker note does not transfer to a window the process owns.
- **Linux linkage, on paper.** winit's defaults load everything at runtime: `wayland-dlopen`
  is a default feature, and the X11 path is `x11-dl` (dlopen) plus `x11rb` (pure-Rust
  protocol). Unverified on a real ELF — there is no Linux build host here — which is why 11.0
  puts a NEEDED guard in CI.
- **naga 30, the compile matrix** — all 15 passes of the nine presets through the frontend,
  the validator and four backends: **Metal 15/15, HLSL 15/15, GL 15/15, SPIR-V 11/15.** What
  it took:
  - Combined `sampler2D` uniforms are rejected outright ("Not implemented: variable
    qualifier"). Bind separate `texture2D`s and one `sampler`, with `#define iChannelN
    sampler2D(iChannelN_t, iChannel_s)`.
  - `sampler2D` function parameters are rejected (09's `hf`, `rayMarch`, `normal`), but
    separate `texture2D, sampler` parameters work on every backend: an 11-line mechanical
    split. SkSL's other blocker for 09, runtime-bounded loops, is no blocker here.
  - `mat2(vec4)` builds an invalid Compose (06). One line, `mat2` from scalars — the same
    edit the Flutter port made.
  - The four SPIR-V failures are one naga 30 bug: a swizzle passed to an `inout` parameter
    (hg_sdf's `mod1(p.x, …)`, in 05/07/08/09) panics the SPIR-V writer with "Expression [N]
    is not cached!". A plain local works; so does hoisting the swizzle into a temp, which is
    what `inout` means anyway. wgpu-like robustness policies plus compaction don't avoid it.
    Related to gfx-rs/naga#1621; the three-line repro is worth filing.
  - naga 30.0.1 with `glsl-in` alone does not compile — its interpolator module is gated on
    `spv-in`/`wgsl-in`. wgpu always brings `wgsl-in`, so it never bites here.

**Watch items.**
- **Mobile has two response curves.** The Dart one (`lib/visualizer/spectrum_source.dart`,
  iOS and desktop) is a sqrt curve with auto-gain and no smoothing; the C++ one (Android) is
  the calibrated dB window and EMA, "the convention the bundled shaders were authored
  against". The same preset reacts differently on an iPhone than on a Pixel. We follow the
  C++ one; the divergence is mobile's to settle.
- **Smoothing is per tick there, per second here.** Android's EMA advances once per PCM batch
  (~30 Hz); `viz.rs`'s rule is rates per second. So α = 0.27^(30·dt), which is exactly 0.27
  at 30 Hz.
- **Licenses.** 01–03 MIT, 05–09 CC0, 04 Cyber Fuji CC BY 3.0 (attribution required). The FSF
  has declared CC BY 4.0 GPLv3-compatible; 3.0 is unconfirmed. mstream_music, also GPL-3.0,
  already bundles it. Operator call before 04 is embedded in the binary; until then it is
  vendored as an attributed file and exercised by tests only.
- **The terminal is the best host in the family.** Flutter desktop captures WASAPI loopback
  on Windows (other apps' audio mixes in), re-decodes the track keyed to the playback position
  on macOS and iOS, and synthesizes a signal on Linux. The tap is the signal, everywhere.

**11.0 — Everything risky, no window.**
1. Vendor the presets into `assets/visualizer/`, pinned to `mstream_music@4ae3dec` (the last
   commit to touch `assets/shaders/` on master), with an attribution table and 06's one-line
   edit recorded as local until it lands upstream.
2. `shader::preset` — the format, a port of Android's `parseShader` (routing lines only
   before the first marker, unknown passes discarded, sizes clamped to 8192) plus the
   metadata and `// param:` lines under Dart's `parseShaderParams` rules.
3. `shader::glsl` — the translation to what naga accepts: the ShaderToy preamble with
   separate samplers, the `sampler2D`-parameter split, the `inout`-swizzle hoist, and a
   y-flip on the image pass only (buffer passes keep GL's memory layout, so feedback reads
   land where they were written). Token-based, not regex: mobile imports user shaders.
4. The compile matrix as a `cargo test`: every vendored pass × Metal/HLSL/GL/SPIR-V, each
   backend behind `catch_unwind`. It needs no GPU, so the existing Ubuntu and Windows jobs
   run it, and a naga bump that regresses a preset fails CI.
5. `shader::audio` — the texture, pinned by golden vectors from the C++ reference built with
   the kissfft it ships (±1 LSB, for FFT rounding).
6. wgpu enters the build with its first real use: `mstream-player viz-probe`, the sibling of
   `graphics-probe` — list the adapters, build every pass's pipeline on this machine's GPU,
   render offscreen, report.
7. CI guards the linkage — no `libvulkan`, `libEGL`, `libGL`, `libX11`, `libxcb`,
   `libwayland-*` or `libxkbcommon` in NEEDED on the Linux builds — and the binary-size delta
   against today's is recorded here.
**Done when:** all seven are green and their numbers are in this section.

**11.0 — done 2026-09-22.** What landed, and what it measured:
- `assets/visualizer/`: the nine presets at `mstream_music@4ae3dec`, an attribution table and
  06's one local line. `shader::library` compiles eight of them in; 04 stays out while its
  license is settled.
- `shader::preset`: Android's three line patterns ported as scanners that accept exactly what
  its regexes do — a marker without its closing `===` is a comment there, so it is here — plus
  the Dart side's `// param:` and title rules.
- `shader::glsl`: the preamble, the `sampler2D` split, the `inout` hoist and the image-pass
  flip, over a token stream. Each refuses by name where it cannot be exact.
- `shader::matrix`: **60 of 60** — all fifteen passes through Metal, HLSL, SPIR-V and GLSL.
  SPIR-V went from 11/15 to 15/15 with the hoist, and MountainBytes needs no hand edit at all.
  Four canaries pin the naga bugs the rewrites answer; a layout test reads the uniform offsets
  and bindings back out of naga.
- `shader::audio`: golden vectors from the unmodified `audio_texture.cpp`, with the generator
  and GL stubs to rebuild them (`test/golden/audio_texture/`; its README's recipe reproduces
  the file byte for byte). **None of the 10,240 bytes differ** on this Mac; the test allows
  ±1 for another platform's libm.
- `mstream-player viz-probe`: on an M3 Pro (Metal), release build, all eight built-in presets
  compiled in 26–292 ms and drew at 640×360 in 0.3–1.1 ms a frame. The frames were checked by
  eye — upright, 05's feedback trails, MountainBytes' terrain from its two buffers — and a
  debug and a release run drew them byte for byte alike: the probe is deterministic.
- Linkage: every GPU and display library in the Linux graph is opened at run time — ash
  `loaded`, khronos-egl `dynamic`, wayland-sys `dlopen` (declared so by wgpu-hal itself),
  renderdoc through libloading, no drm. `test/linkage.sh`'s first run, on PR #29's CI, read
  the x86_64 binary's NEEDED as libasound, libgcc_s, libm, libc and the loader — nothing the
  visualizer brought. The release workflow holds arm64 and armv7 to the same.
- Size: 24.7 MB → 28.9 MB for aarch64-apple-darwin in the release profile (+4.2 MB, +17%),
  nearly all of it wgpu and naga — the eight embedded presets are 67 KB of it.
- Unmeasured: any driver but this Mac's — DX12, Vulkan, a Mesa GL, a Pi. `viz-probe` is the
  tool, and `WGPU_BACKEND` picks the backend it asks.

**11.1 — The window.** `viz-window` with the six single-pass presets (01, 02, 03, 06, 07, 08):
winit + a wgpu surface; `PresentMode::Fifo`, never a busy loop; no rendering while occluded or
minimized; passes at logical resolution and upscaled (mobile's `pixelScale` exists because
per-device-pixel shading on a 3× phone was 9× the work); borderless fullscreen; the display
kept awake while fullscreen and playing; `focus_window()` on open; ←/→ between presets like
mobile's ‹ ›. The Visualizer tab gets the key that opens it. **Done when:** a track playing in
the TUI moves its window on macOS, Windows and Linux.

**11.1 — landed 2026-09-27** for the GUI shell (docs/ux-contracts/visualizer-window.md):
`mstream-player viz-window`, the hidden child — winit 0.30 and a wgpu surface on the same
`Gpu`, the preset drawn at logical size into an RGBA8 target and blitted, upscaled, onto a
non-sRGB surface (an sRGB-only one gets the blit that undoes the second encoding); Fifo,
no drawing while occluded, `focus_window()` on open, `f` borderless fullscreen, ←/→ through
every built-in preset (the multipass ones too — the renderer already drew them), Esc/q
closes. The parent (`src/gui/vizwin.rs`) spawns it (Windows: no console window), feeds the
audio texture from the tap thirty times a second down its stdin through a two-deep writer
that drops rather than blocks, sends silence while paused, forwards its stderr to the log,
and closes it with the player. The top bar's Visualizer item and `V` open or raise it.
Left: the display kept awake while fullscreen, and the TUI's own key for it.

**11.2 — Multipass and tunables.** 04, 05 and 09 (ping-pong feedback, 1×1 state buffers);
`// param:` defaults with config overrides; Android's crossfade between presets
(`uCurrent`/`uOld`/`uMixT`).

**11.2 — the window's controls and tunables landed 2026-09-27** (visualizer-window contract,
clauses 9–15; 05 and 09 were already in with 11.1): over the picture, a bar that shows while the
pointer moves — `‹` `›`, a dropdown of every built-in preset, the tuning toggle, fullscreen — and
the mobile app's tuning panel as a sheet down the right edge: the response curve (dB floor, dB
ceiling, smoothing, the ranges mobile's panel uses) and a slider per `// param:` knob, with Reset.
Drawn by egui 0.36 on the window's own wgpu device, after the blit, in the same submission. The
knobs reach `iParams[]` in the window; the curve is the texture's, so the window reports it up its
stdout (`preset` / `curve` / `knobs` lines) and the player builds the next texture with it. The
player keeps all three in `[visualizer]` in `config.toml` — written only by the player, a second
after the last change and when the window closes; read by both at open through one function — and
the window reopens on its last preset. Tested without a display: the controls' egui pass is pure
CPU, so the tests click the arrows and the dropdown rows and drag the sliders; an ignored GPU test
paints the bar, the open dropdown and the panel (in English, Russian, Japanese, Chinese) to PNGs.
Size: 32.5 MB → 36.1 MB for aarch64-apple-darwin in the release profile (+3.6 MB, +11%), main at
`0b5d2e7` against this, both on rustc 1.98.1 — egui's code and its two text faces; its two emoji
faces (0.7 MB) are left out. egui 0.36 wants rustc 1.95; CI's `stable` is 1.98.
Left: the crossfade; 04 (its license).

**11.3 — `serve --visualizer`.** The jukebox on a TV. `Engine::attach_tap` already exists, so
`serve` feeds the same host side and the window opens fullscreen on the jukebox's display.

**11.4 — (optional) Milkdrop.** The projectM companion, only if 11.1–11.3 leave appetite.

**Upstream.** The naga bug (three-line repro); 06's `mat2` fix into the canonical file; the
mobile response-curve divergence. And two in Android's engine, found porting it: a channel line
with an index past 2³¹ (`// === channel image.9999999999 = music`) throws out of `std::stoi` on
the compile worker's bare thread, which ends the app (reproduced off-device with the parser
copied verbatim; the size line beside it was hardened against exactly this); and a buffer that
reads a later buffer gets the frame before last, not the last frame. Both fixed, with the
curve divergence, in mstream_music#207 (2026-09-22) — reproduced on the Android emulator
before the fix and gone after it, and the Dart curve held to the same golden bytes as ours.

### Phase 12 — The control face: the desktop player as the server-audio engine

> **Status 2026-09-28: the player half is built** — `gui --serve-port <port>` hosts the control
> API from the GUI's own queue and worker, the sidecar publishes the port and a token, and the
> serve module's parser is shared by both faces. Verified: 1037 unit tests, the wasm check, and a
> live run through the mStream launcher's bundled Ghostty (the real GUI, every route over
> loopback, the port closing with the player). The mStream half — the server adopting the face,
> the launcher passing the port — is the next slice there (mStream Phase C).

> **Decided 2026-09-30: the face is always on.** The launcher passes `--serve-port` on every
> open, with the server's configured player port (`rustPlayerPort`, default 3333): that port is
> mStream's player port by configuration whichever engine holds it, so the GUI needs no
> ephemeral-port mode, and `autoBootServerAudio` decides only whether the server takes the offer
> up. Flipping the switch never needs the player restarted, and the torrent hand-off (a second
> `--torrent` while the player is open) gets its carrier for free: a route on this face, in a
> later slice, instead of an inbox file. The cost is a token-guarded loopback port whenever the
> GUI runs, so the sidecar that carries the token is written owner-only (instance.rs).

mStream's server-audio feature spawns `mstream-player serve` and proxies the web remote to it.
On a desktop where the GUI is open that made two players on one machine's speakers: the jukebox
the remote drove, and the player the person sat at. The one-engine rule is that the GUI IS the
engine while it is open.

- **One parser, two faces.** `serve::vet` owns every rule the wire has — the hygiene of #28/#30
  (Length required, Host, Origin, Content-Type, the body cap and the socket-reading discipline),
  the token gate that spares only `GET /version`, and the route table — and yields a `Command`.
  The headless engine executes it as before, route-for-route what rust-server-audio answered;
  `gui::control` executes it against the App through the same funnel a click uses (`forward`,
  `play_index`, `remove_queue_row`, `seek_to`, the bar's volume clamp). Neither can drift from
  the other, and the table is pinned by a test.
- **The listener is a thread with a channel.** It owns the socket, vets, and hands each Command
  to the GUI loop, which drains the channel once a tick (`control::pump`) and answers through the
  request's reply slot; three seconds of silence is a 503, never a hung socket. Loopback only.
  The bind retries for fifteen seconds: the server stops its headless engine when it sees the
  claim, and that engine may hold the port for its stop wait.
- **Paths are the player's.** `/play`, `/queue/add` and `/queue/add-many` take the server's
  library paths (vpaths) — what the GUI queues from a listing — and `/status`'s `file` and
  `/queue`'s rows answer with the same; mStream passes its library paths through untranslated
  for this face. A queued row starts with no tags and is filled the way the Song Info sheet's are:
  `fetch_track_info` → `consume_track_info` patches every copy of the path.
- **The wire's semantics kept, the App's honored.** `/pause` and `/resume` are explicit on the
  wire and a toggle in the App, so each flips only from the other state; `/stop` keeps the queue
  (ClearQueue's bookkeeping without the clearing); `/loop` cycles the App's repeat (off → all →
  one) and answers the engine's words (`none`, `all`, `one`); `/play` is ClearQueue + add + play.
  `GET /version` grew a `face` field (`serve` | `gui`), additive under apiVersion 1.
- **The claim rides the instance lock's sidecar** (`port`, `token`; schema still 1, the fields
  optional): the launcher hands the player `--serve-port` beside `--instance-lock`, and mStream
  reads the sidecar behind its own liveness check — pid alive, `/version` answering as `gui`.
  The token is 128 random bits minted at launch, in the sidecar and in the server's requests.

**Left for the mStream half:** the server's external-engine mode (stop the headless engine on a
live claim, proxy to the face with the token, respawn when the claim dies), the proxy layer's
pass-through of library paths for this backend, the launcher passing `rustPlayerPort` as the port
(always — see the decision above), and the pin bump. The first-version handoff carries no queue
across: opening the GUI ends the headless engine's queue, closing it hands back an empty engine
(the GUI's own queue persists in queue.json as ever).

### Phase 13 — The MP3 player's flasher (`mstream-player device flash`)

> **Status 2026-10-04: shipped in v0.10.0** (built 2026-10-01) — the page, the `--yes` line
> mode, `device list`, the fake board and its e2e leg; then the page's UX from the design cards
> (mStream `docs/designs/firmware-flash`, alternate A): the step line, the SD card row, the time
> left, what to do next, the port watch, and the log behind `l`. The pin in
> `src/device/firmware.rs` is firmware v0.8.0 (10 Oct 2026; before it v0.7.0 from 7 Oct,
> v0.6.0 from 4 Oct, and the first release, v0.5.0) — `device flash` with no flags writes it;
> `--firmware` / `--release` still override.
> The mStream launcher's menu item is dropped (Paul, 4 Oct 2026): **the GUI's MP3 Player tab
> replaces it** (built 2026-10-04, docs/ux-contracts/mp3-player-screen.md) — the same page with
> no flags, hosted under the GUI's top bar, `K` from the Library, Now Playing, Stats and the
> Admin hallway.

The mStream MP3 player (IrosTheBeggar/mstream-mp3-player) is an M5Stack Core2; its firmware
reaches the board over USB through the ESP32's serial bootloader. The flasher lives in this
player rather than in the tray launcher (decided 2026-09-30): flashing is a conversation —
which board, what is on it, erase or not, progress, what went wrong — and this is where the
pages are. The launcher was to open this page the way it opens the admin rooms; since
2026-10-04 the way in is the GUI's own tab instead, and the launcher has no item for it.

- **espflash as a library** (`default-features = false`, `serialport`): the reset dance, the
  RAM stub, compressed writes, the MD5 skip-and-verify. serialport with libudev off, because
  test/linkage.sh refuses a binary that needs it. About 2 MB on each binary.
- **The firmware's contracts.** The merged `*-full.bin` written at 0x0 keeps the settings (NVS
  sits above the image in the firmware's layout); ESP-IDF's app description at 0x10020 names
  the firmware and its version, so what is on a board is read from its flash — no console
  command, and a board whose firmware does not boot still answers.
- **Trust.** The pin is a tag plus the sha256 of its `-full.bin` (release assets are mutable);
  `--release` trusts the release's own `SHA256SUMS`; every local file is vetted by its
  description before anything is written.
- **The worker** (src/device/flow.rs) holds the board in its bootloader from the probe to the
  write, so one reset serves both, and restarts it on every way out before the write; the page
  cannot leave a write. The baud ladder (921600 → 460800 → 115200) covers bridges and cables
  that cannot hold the fast rate, on connect and again on a write that dies.
- **Left:** a way to the pinned release's `-dio-full.bin` from the page (today `--firmware`,
  for a Core2 that keeps restarting on the QIO image); Wi-Fi and pairing over the same port
  (the hidden `--server` flag is accepted for it); a udev rule in mStream's deb/rpm.

### Phase 14 — The window spike: the GUI in a window of its own (2026-09-29/30)

> **Status (2026-10-02): spike done, decision GO WITH CONDITIONS, the conditions and the
> Windows run worked (below); the window is the desktop product's default and compiled out of
> the terminal product (lanes 7–9), the desktop packages went through the v0.10.0-rc.1
> pre-release, and PR #41 is for merge.** The vendored crates (vendor/ratatui-wgpu with its
> 20 recorded changes, vendor/winit with two backports) ship as they are, by the owner's
> decision of 2026-10-02: their VENDORED.md files are the record, and filing the changes
> upstream is a separate task, not a gate. What remains before the stable tag is under
> **Shipping — shipped as v0.10.0 on 2026-10-02 (release run 37048300547: both families, the three packages, the app notarized and stapled, Homebrew and Scoop bumped, 21 assets; the published app prints 0.10.0 with `features: window`).**, near the end of this phase. Five
> commits on `claude/desktop-app-packaging-8ee4ff` (4e10926 render, 4685214 loop, de04c35
> input, 4d6b4df stats, c59cbdb art) plus the keyboard check's fix, on top of v0.8.0, pushed
> as PR #41; the condition lanes and the Windows machine's two fixes followed
> on the same branch. Steps 1–3 and 5 were each
> built by one implementer and judged by three adversarial reviewers with lenses that varied
> by step (fidelity, liveness, behaviour or visual; terminal parity or footprint; code), with
> fix rounds only for blocking findings; step 4 was two measurement legs (this Mac, and Linux
> in Docker), two skeptics who re-measured, and a criteria judge. Five blocking findings were
> raised, four distinct issues (the en stars as tofu, the pointer grid, Ctrl on non-Latin
> layouts, a crash on absurd script numbers), all in steps 1 and 3 and each fixed at the root;
> the implementer found and fixed a fifth crash on its own (a resize below one cell aborting
> inside the crate). The evidence is in `docs/window-spike/` (screenshots, stats, CPU
> samples, the Linux linkage list, the full manual checklist). The spike's sections are a
> record of what happened; the Shipping paragraph is the plan to ship.

**The question.** The desktop-app packaging research (2026-09-28) found that a "desktop
app" of this player is a terminal emulator plus the player, which is why the mStream bundle
carries Ghostty on macOS, launches through Windows Terminal on Windows and uses the user's
own terminal on Linux. The alternative is to draw the same cell buffer ourselves: ratatui
hands a `Backend` the cells that changed, and `ratatui-wgpu` is a backend whose surface is a
wgpu texture in a window we own — the same winit and wgpu the visualizer window already
links. The spike asked three things: does the GUI render faithfully that way, does input hold
up, and how much of the GUI has to change.

**What it is.** `mstream-player gui --window` (a hidden flag) runs the real player — the
same `App`, the same audio and api workers, the same `render` — in a winit window through
ratatui-wgpu 0.6 (`src/gui/window/`: `mod.rs` the window and its loop, `input.rs` the
translator, `script.rs` a scripted-input lever, `stats.rs` a timing lever, `covers.rs` the
art). The GUI loop in `src/gui/mod.rs` is split into two shared halves — `frame` (dispatch,
tick, draw, the worker drain, the polls, the pointer through a one-method `Host` trait, the
wait) and `input` (one crossterm-shaped event → Continue or Quit) — which the terminal loop
calls in the old order and the window calls from its event loop at the terminal's own
cadence (10 ms hot, 33 ms while the visualizer draws, the caret's flip, else 100 ms).
Startup and teardown that need no terminal moved verbatim into `start` and `finish`. That
split restructures the loop the terminal GUI ships on; the parity reviewers read it
statement by statement and ran the GUI tests and the expect leg, and found no behaviour
change, but it is the one part of the spike that touches the shipping path.

**Steps and evidence.**
- *1 — Render (4e10926).* Hack is the face, plus a system symbol face for the three glyphs
  Hack lacks (★ ☆ ✓; a census test over the GUI's literals and the locales guards the list;
  Menlo on macOS, DejaVu Sans Mono on Linux, the Windows candidates never run — on a box with
  none of them the stars are boxes again, which a bundled face would end) and the visualizer
  overlay's CJK face for ja and zh. The backend's `get_text()` against the same Gui drawn into
  a `TestBackend` reads `100×30 EQUAL` in en, ja and zh: a text-level check that proves every
  cell reached the backend, not what the pixels look like — that rests on the macOS
  screenshots, which show rounded frames, the seek thumb, the transport glyphs, kana and
  kanji from Hiragino, no tofu.
- *2 — The shell (4685214).* Against demo.mstream.io the Files room lists the server's
  library; at 70×20 the mini player shows; Cmd-Q reaches the same teardown and writes the
  config; a resize feeds the GUI's own Resize bookkeeping after a full repaint;
  `MSTREAM_WINDOW_SIZE=<cols>,<rows>` sets the opening grid.
- *3 — Input (de04c35).* winit keys, text, IME commits, pointer, buttons and wheel become
  crossterm events: named keys by name; characters from the text the OS composed; Ctrl from
  the layout's Latin letter, else a control character, else the bare Latin key, else the
  key's place (so a non-Latin layout still quits on Ctrl+c); Shift+Tab as BackTab; pixels to
  cells against the backend's stretched grid; wheel turns accumulated to whole lines; held
  buttons for drags. Accessibility is not granted on the dev Mac, so the proof is
  `MSTREAM_WINDOW_SCRIPT=<file>` (wait/key/text/ctrl/ime/move/click/rclick/drag/wheel/
  resize/dump/quit), which injects the raw events the winit handlers would build — one step
  below winit, so the OS-to-winit-to-raw conversion itself is proven only by reading winit's
  source. Through it: rooms by digit, drill, a typed query character by character, Tab and
  Shift+Tab, an IME commit landing as text (a synthetic preedit is tracked and logged, and
  types nothing), clicks acting on the row they land on down to the bottom row, hover verbs
  and the hand cursor following the pointer, the wheel both ways, a right-click sheet, a
  queue-row drag, Ctrl+c and `q` quitting through the same teardown with the config written.
  Home/End, PageUp/Down, Left/Right and the F-keys are unit-tested only, and the winit-to-raw
  half has no unit tests at all. 18 translator tests.
- *4 — The numbers (4d6b4df).* `MSTREAM_WINDOW_STATS=<path>` counts, through a pass-through
  backend, which cells ratatui handed on and times `frame`, the flush and the whole redraw.
- *5 — Art (c59cbdb).* `Graphics::hosted`: instead of encoding a picture for a terminal
  protocol, `draw` blanks the cover's cells and records the cell rect and the art on a
  per-frame board; a post-processor runs ratatui-wgpu's own text blit unchanged and then
  draws one textured quad per cover (the cell rect as a fraction of the grid, the picture
  fitted whole and centred, `Rgba8UnormSrgb` on the sRGB surface, the 128 px thumbnail first
  and a worker's full decode scaled to the box when it lands, at most 48 textures held).
  The Albums wall, the bar's card, the queue rows and Now Playing show pictures; a page turn
  and a resize keep them aligned; a queue row's sheet over the wall reads over mosaics — the
  terminal's overlay rule, lagging one frame, so a texture paints over a freshly opened modal
  for up to 100 ms. Slot::draw_paced in `src/gui/cover.rs`, the shared slot every room draws
  covers through, gained the one hosted branch. Art ran on macOS Metal only.

**The numbers** (release profile at step 4, before the art; this Mac; against
demo.mstream.io; ~25 s a run; CPU as cputime over wall time from t ≥ 10 s, the steady state;
the terminal GUI on a pty with the mosaic persona as the baseline; "first frame" counted from
`window::run` entry, not from spawn):

| Scenario | Window CPU | Terminal CPU | `frame` p50 / p95 ms | First frame ms |
|---|---|---|---|---|
| Idle, Library | 1.2% | 1.3% | 0.42 / 0.46 | 142 warm, 415 cold |
| Albums wall | 1.5% | 1.4% | 0.76 / 0.86 | 175 |
| Playback, Library | 2.9% | 2.3% | 0.54 / 2.2 | 190 |
| Now Playing + waveform | 3.2% | 2.6% | 0.55 / 2.6 | 181 |
| Now Playing, Visualizer tab (33 ms) | 14.8–16.7% | 5.4–5.7% | 3.1 / 4.7 | 170 |

Whole-run CPU (start-up included) is higher for the window: idle 2.0% against 1.5%, the wall
2.5 against 1.7, playback 4.2 against 3.2, Now Playing 4.6 against 3.8, the visualizer tab
10.6 against 4.6. A frame that changes nothing costs about 3 µs to flush (the backend skips
encode and present); a presenting flush is 1.2–2.8 ms p50, under 5 ms p95; single-frame
maxima reached 16–18 ms, over the 16 ms the criterion named. The first cold launch after a
build listed its window 1.0 s after spawn; there is no single spawn-to-pixel timer. RSS
111–116 MB against the terminal's 33–38 MB — the wgpu device, the atlas and the fonts. The
macOS binary 36,132,480 → 37,261,296 bytes at step 4 (+1.1 MB, +3.1%; the baseline is the
fd0f13b release of 2026-09-27, one small change before v0.8.0; step 5's art was not
re-measured in release) with no new framework or dylib (Metal, QuartzCore and AppKit were
already linked for the visualizer). Art is measured in the DEBUG build only: 20 s on a still
wall, `frame` p50 3.3 ms and p95 4.3 ms, but the seven frames that presented flushed at p50
15.9 ms and p95 34 ms (uploads and shaping included), and the visual reviewer's run with four
page turns had `frame` p95 7.6 ms and max 57 ms. Note that playback does NOT put the loop
on 33 ms — `drawing_audio()` is the visualizer tab only — so four of the five rows run at the
100 ms poll. A release build takes 14–17 min here, and the test profile recompiles the wgpu
and naga stack.

**The Linux leg** (Docker, `rust:1-slim-bookworm`, aarch64, the step-3 tree de04c35): the
release build in 17.5 min cold; `test/linkage.sh` passes with NEEDED unchanged (libasound,
libgcc_s, libm, libc — every crate ratatui-wgpu brings is pure Rust); the binary is
45,066,952 bytes with no pre-spike baseline to set against; under Xvfb the window opens and
quits clean in ~7 s on lavapipe (Vulkan, Mesa 22.3.6, a CPU device) and again with
`WGPU_BACKEND=gl` on llvmpipe, the text dumps identical and `100×30 EQUAL` — text dumps
only, en only, no screenshot, and neither the art path nor the stats lever ran there. One
fact for a Linux desktop build: without `libxkbcommon-x11.so` the window panics before it
opens (winit's X11 path through xkbcommon-dl, exit 101 — needs a clean error or a fall back
to the terminal GUI, and the dependency listed). From reading the crate, not observed:
`Font::new` reads only face 0 of a collection, so Noto CJK SC at index 2 would be skipped
for Chinese on Linux.

**The scorecard** (the criteria set before the spike started; the judge's verdicts, with the
reviewers' caveats restored):

| Criterion | Verdict | Why |
|---|---|---|
| Rendering fidelity | partial | Every cell reaches the backend (EQUAL on both platforms); pixels judged on macOS screenshots only, no tofu in en/ja/zh. Gaps: an en locale loads no CJK face, so CJK titles draw as boxes and Korean is tofu everywhere; after a wide glyph the rest of the row shifts left (the crate shapes a row as one string and the cell after a wide glyph is empty — a cover-box corner landed two cells left in the IME scenario), a crate defect needing a patch or a workaround; the darks come out darker (below). |
| Plain input | pass, with caveats | Driven end to end at 2× scale through the lever: Enter, Esc, Backspace, Down, Tab and Shift+Tab, digits and letters as text, Ctrl, clicks to the bottom row, wheel, drag; then at a real keyboard (the operator) and as real OS key events: every standard key, key repeat, Option chords, Cmd chords, Ctrl on a Russian layout. Unit-tested only: Home/End, Page keys, Left/Right, F-keys; 1× scale untested. |
| Dead keys and AltGr | partial | Dead keys pass on macOS at a real keyboard; the German layout's Option chords pass as real key events. Windows AltGr untested, and confirmed from the code: a left Ctrl+Alt standing in for AltGr there falls through to the bare Latin key (German `@` becomes Ctrl+q). The Dvorak Ctrl+punctuation remap was found at the keyboard and fixed. |
| IME | partial | Real sessions at last: Pinyin composes and Space commits `中国` (attachment flaky in two of five runs); Japanese composes, one Enter commits without Space, and after Space the candidate list takes an Enter of its own — native Kotoeri, matched by an AppKit control. Synthetic commits land as text; preedit is not drawn; the candidate window sits at the origin; unverified that IME is enabled only while a text field has focus and that Enter during a composition is not delivered twice. |
| Integration cost | partial | About 186 lines across 6 files at step 4 (all seams: the flag, the loop halves, `start`/`finish`, the palette pin, two visibilities), within the ~200 the criterion named; step 5 added graphics.rs (+61) and cover.rs (+10, the shared slot's hosted branch), about 257 lines across 8 files, over it. The raw stat reads +409/−213 because the loop body moved into `frame`/`input`. No room or widget file touched. |
| Performance | pass, with caveats | Steady idle CPU equal to the terminal's; p95 under 5 ms; a warm first frame under 200 ms. Caveats: maxima 16–18 ms; a cold launch about 1 s; the visualizer tab costs 2.7–2.9× the terminal; art unmeasured in release. |
| Footprint | pass | +1.1 MB, no new frameworks (before the art); Linux NEEDED unchanged, plus the libxkbcommon-x11 runtime load; the build and test times above are the larger cost. |
| Platform | partial | macOS Metal proven end to end; Linux X11 under Xvfb on a CPU rasteriser, text only, on the pre-art tree (no Wayland, no real driver, no x86_64); Windows untouched at the spike's end (the Windows 10 run and retest below came with the conditions). |

**What the crates taught.** ratatui-wgpu 0.6: `RenderSurface` is sealed (its `Sealed` bound
in `src/backend/mod.rs`; the headless surface is test-only), so a real window is required;
rows are marked clean after a failed present (the first present on macOS is occluded — a
full repaint on `Occluded(false)` is the workaround, and any other failed present, a Timeout
or an Outdated surface during a live resize, would still leave stale pixels until a cell
changes); the default post-processor stretches the cell grid across the whole surface, so
pixel→cell must use the drawn grid; a 0×0 grid panics inside the crate, so the surface is
held to one cell; `default-features = false` drops ahash and png (otherwise ahash's
compile-time-rng leaks into egui's tree). wgpu-core 30 sorts sRGB surface formats first, so
the surface is `Bgra8UnormSrgb`, and the crate's blit decodes with pow 2.2 while the sRGB
store re-encodes with the exact curve: the darks come out darker (ground #12131c → #0a0b16,
gold and text within one level), while the cover textures go through the exact curve, so
the ground beside a cover may differ by a level. The fix is not in our tree: a one-line
`remove_srgb_suffix()` patch carried on the crate, a colour pre-conversion in our own backend
wrapper, or a custom blit. The visualizer tab's named ANSI colours map through the crate's
SVG table (Blue = #0000ff) — `Builder::with_color_table` fixes that on our side. winit 0.30:
Cmd-Q exits inside AppKit, so main's exit code and the instance-lock sidecar cleanup are
skipped; `set_min_inner_size` is ignored by a scripted `setContentSize`; Super chords are
dropped today, so there is no Cmd+V paste (a terminal types pasted text as keys); a platform
that holds back redraws for a hidden window (a hidden Wayland window, for one) would stop
`frame`, and with it the worker drain and track advance — occlusion and minimise were never
exercised, even on macOS.

**Conditions before this could replace a terminal host** (the judge's list, plus the
reviewers' minors worth carrying): the gamma fix and the colour table; an always-on CJK
fallback with collection indices, and the wide-glyph row shift patched or worked around; the
libxkbcommon-x11 panic turned into a clean error and the dependency documented; a panic hook
of the window's own and Cmd-Q routed through `finish`; paste; the IME candidate window at the
caret, preedit drawn, IME gated to focused text fields (winit is now vendored with the #4478 Korean backport,
untested here for want of a Korean source); held buttons cleared on `CursorLeft`
and focus loss; `ScaleFactorChanged`; a periodic or resize-triggered full repaint for failed
presents; playback and queue advance proven while hidden or minimised; the overlay rule's
one-frame lag; nearest-neighbour sampling for the QR code; mipmaps or a sharper resample for
shrunk covers; resize increments so glyphs do not stretch between whole-cell sizes; a
byte-bounded texture cache; a bundled symbol face; the timing probe gated off without its
lever; and the manual checklist passed. The judge's gate: keep the flag hidden until the
gamma, CJK, libxkbcommon and panic-hook conditions and the Windows run are done.

**The conditions, worked (2026-09-30 to 10-01).** Five lanes on the same branch, each one
implementer and three adversarial reviewers as the spike was, each committed on its own:
- *Lane 1 (e8fe96e)* — ratatui-wgpu and winit vendored under `[patch.crates-io]`
  (`vendor/`, each with a VENDORED.md naming every change); winit carries Warp's 0.30
  backport of rust-windowing/winit#4478 (Korean: a key dropped after a commit, a doubled
  Space), untested here for want of a Korean source. The lane also showed the Japanese
  "swallowed Enter" was native Kotoeri (above).
- *Lane 2 (b5384fc)* — the renderer: headless rendering made public with a pixel readback and
  offscreen tests of our own; the surface takes the non-sRGB twin of the format wgpu offered,
  so bytes pass through exactly (ground #12131c and gold #e5c07b measure exact), with the
  exact sRGB curve where only sRGB exists; faces open by collection index; a failed present
  is owed, not lost; a font's id hashes a bounded prefix; a glyph narrower than its box is
  centred rather than enlarged, which was clipping Hangul; the wide-glyph "shift" was the
  text dump's spelling, shown by a test that draws AB日本CD. The window: a colour table for
  the named ANSI colours; CJK and Hangul faces for every locale through memory-mapped files
  at unchanged RSS; the QR code sampled nearest; covers resampled when a box shrinks; a byte
  bound on the texture cache; the timing probe idle without its lever.
- *Lane 3 (689df49)* — a composition drawn inline at the caret (the kit splices it, inert
  without one); IME allowed only while a field has the keyboard, placed at the caret's cell,
  with a modal taking the keyboard from the field beneath it; paste (Cmd+V, Ctrl+V,
  Ctrl+Shift+V) into a field only, the clipboard's first line typed as keys; the pointer
  leaving or focus lost releases held buttons; a scale-factor change rebuilds the fonts and
  re-grids; `scale`, `leave`, `alt`, `ctrlalt`, `minimise`, `frame`, `press`, `release`
  joined the lever.
- *Lane 4 (dc0ad19)* — a clean line and exit 1 when libxkbcommon-x11 is missing on an X11
  session, the package recommended by the deb and rpm; a panic hook of the window's own; the
  instance lock dropped by the window on Cmd-Q; the kit reports moved overlays and the next
  frame runs hot, so a cover paints over a fresh modal for ~20 ms, not ~100; a minimised
  window on macOS keeps its frames, clock and queue (measured over 20 s, so no hidden-window
  timer); resize increments; the field's window counts cells, so wide text fits (a shared
  correctness fix); a masked composition; paste capped at 4096 and stripped of stray format
  characters; a narrower cell replacing a wide one clears the continuation cell (a residue
  the window had shown).
- *Lane 5 (dde0aa4)* — from the Windows report: the glyph tier pinned modern under the
  window; Warp's backport of rust-windowing/winit#4582 against the layout-switch freeze; an
  Alt chord delivers only what the OS composed (AltGr's @ and €; Ctrl+Alt+q nothing, where
  it had quit the player); startup stages timed and the face discovery and the wgpu adapter
  and device moved to threads started before the window exists (warm first frame here
  ~180 → ~150 ms); every quit path exits within 50 ms here, with a "way out" timing line
  for Windows' 11 s Ctrl+Q; the dump names the last press and what it hit, for the queue
  drag that never starts there.

**The Windows run (2026-10-01, Windows 10 22H2, one display at 200%, NVIDIA via Vulkan,
rustc 1.98.1).** Build and tests green; the automated pass green (keys, mouse to the bottom
row, resize to a cell without a crash, covers as pictures, 1× through a DPI-unaware launch);
real input green for the key walk, clicks, the transport, close by X and Alt+F4 with the
sidecar removed, minimise with playback continuing; CPU idle 0.5% and the visualizer tab
15.9% in release. Defects: the conhost glyph set from a plain console (fixed, lane 5); a hang
on every keyboard-layout switch (the winit freeze, backported, untested until the retest);
the queue drag never starting (not reproducible here; evidence added for the retest);
Ctrl+V typing a `v` (paste, lane 3); Ctrl+Alt+q quitting (lane 5); the first frame 2–3 s
after the window appeared (stages instrumented; the retest names the slow one); Ctrl+Q
11 s to exit (timings added). Untestable there: AltGr, dead keys and IMEs (no second layout
without elevation, and the hang), 150%, a scale move.

**The Windows retest and the hand-off (2026-10-01, the same machine, c2c81bb then de9084a).**
The retest passed the glyph tier, Ctrl+V, Ctrl+Alt+q, the quit paths, minimise, AltGr and
the dead keys (later also the dead-key Space fix: `'` `e` → `é`, `'` Space → `'`, `^` Space
→ `^`); startup there was 1.4–1.7 s warm, the Vulkan instance alone 0.5 s; the posted
layout change still froze inside `DefWindowProcW`, an opengl32 hook sitting in the
window-procedure chain; and the queue drag read as a one-column miss. Three items went back
to the Windows agent with ownership of the files involved, and all three came back:
- *C, not a bug.* The grip is at window width − 7 (column 93 at 100 columns), `[⋯]` at
  95–97, and the earlier presses were at 94, a cell only the row covers. A press on the grip
  through the lever reads `QueueGrip`, the drag begins, and the release reorders the queue.
- *B, fixed by the backend mask (d50bd5d).* The repro on `WGPU_BACKEND=dx12` alone and on
  `vulkan` alone did not freeze, so it was the GL backend's presence, not winit, and the
  vendored winit is untouched. On Windows the window now builds a DX12-only instance first
  and a Vulkan-only one only when DX12 finds no adapter, never GL; `WGPU_BACKEND` still
  overrides. Three posted layout changes with the window focused: no hang, dumps continuing,
  AltGr+Q on the switched layout typing `@`. First present 1.0–1.2 s warm against
  1.6–1.8 s: the instance 22–39 ms where Vulkan's was 575–893, and DX12's adapter request,
  ~0.5 s for two GPUs, the long pole now.
- *F, the window hidden until its first present (ec1ab37).* Windows only: created with
  `with_visible(false)` and shown the moment a frame hands the backend cells, or at a 4 s
  deadline regardless. A hidden window gets no `WM_PAINT`, so until it is shown the frames
  run from `about_to_wait`. The window appears already drawn about 1 s after spawn warm,
  where it had stood blank from ~300 ms for 1.3–2.4 s; `first_visible_present_ms` fills on
  Windows. Under a compile-like load the baseline went "Not Responding" with a DWM ghost in
  its place from ~8 s until its first present at ~20 s; after, no ghost at all, since DWM
  does not ghost a hidden window. But the deadline is checked only once `open()` has
  returned from joining the GPU thread, so under that load it fired 17 s in, and the main
  thread sat hung through the join — the follow-up lane below.
- *Found there, outside its files:* the six offscreen render tests lock up when the harness
  runs them concurrently on NVIDIA — one test's Vulkan device going down inside nvoglv64.dll
  while another's all-backend instance brings a WGL context up in the same DLL, neither
  returning. Fixed here by taking the GPU one at a time (7b4399c). CI's Windows runner has
  no NVIDIA driver, which is why its leg stayed green.

**Lane 6 (f57faec), the review of the Windows commits, worked.** A reviewer read d50bd5d
and ec1ab37 against wgpu-hal 30.0.1 and found six minors, none blocking, and this lane did
them with the same shape (one implementer, a code lens and a macOS lens, no fix round
needed):
- One module, `src/gpu_pick.rs`, now chooses the instance and adapter for the GUI window,
  the visualizer's own process and the headless `viz-probe`: on Windows a DX12-only instance
  first, Vulkan alone only when DX12 has no hardware adapter, never GL, so no process of ours
  brings up the backend that froze the window unless `WGPU_BACKEND` asks. DX12 answers even
  without D3D12 hardware, with WARP, Microsoft's software rasteriser, which had made the
  Vulkan fallback all but dead and would have drawn such a machine's frames on the CPU: a
  software adapter is now held while Vulkan is tried and taken only as the last resort (the
  rule is a pure function over lazy tries, with tests that the walk stops at hardware).
- `open()` no longer joins the early threads. When they are not done, the loop polls them
  every 4 ms, builds the backend on the turn they finish and answers window events meanwhile;
  a hidden window shown at the deadline draws one frame first, so a late window is not blank
  (the Windows run under load had the main thread hung through the join and the deadline
  firing 17 s in). On this Mac the threads finish before the window exists, so nothing
  changes: first present 165 vs 167 ms warm medians, A/B; a 1.5 s delay planted on the GPU
  thread put the first present at 1.65 s with the window answering throughout.
- The "drawing with … through …" line is printed once, after the backend is built, naming
  the adapter the backend holds (the vendored builder now reports it, change 13), with ", in
  software" for a CPU adapter; the main-thread fallback hands its adapter and device to the
  builder instead of dropping them; on Windows `first_visible_present_ms` records the frame
  that showed the window, not the repaint after it; a `threads.wait` stage joins the stats.
Left as minors: the offscreen render tests still build the crate's default instance (GL in a
test process on Windows; the mutex covers the hang); the visualizer child prints no adapter
line; input that arrives before the backend exists is dropped, not held (a quit key in that
gap does nothing, the close button still works); and the backend build itself (surface,
configure, pipelines) still runs on the loop thread. The Windows paths of this lane are
reasoned through and compiled by CI's Windows leg, not run.

**Still open after the Windows run.** The WARP rule, the polled startup under load and the
visualizer's window on a layout switch, unrun on Windows; Korean on macOS (no source
enabled); a hidden window
on Wayland (frames measured on macOS, minimise with playback proven on Windows); Wayland
paste without XWayland; the cell after a modal opens still paints a cover for one short
frame — gone since lane 15 (the emoji faces, the Hangul height and the tag characters
are lane 14's, below); on
Windows, 150%, a move between scales and the IMEs, untestable there
without elevation.

**Two products from one crate (2026-10-01).** The owner's decision: the codebase is dual
purpose, a terminal player and a desktop player. The DESKTOP releases, a new asset family
`mstream-player-desktop-*` (win32-x64, darwin-x64, darwin-arm64, linux-x64; the bare files
carry a `-raw` suffix since 2026-10-02, after a download of one by hand; proven by
v0.10.1-rc.1, run 37056273001: the renamed binaries beside the packages, the stub inside
the Windows zip and absent from the release, latest and the channels untouched) with per-OS app
packages to follow, open the GUI in its own window; the TERMINAL releases (the unsuffixed
binaries, deb and rpm, the Homebrew formula, Scoop, the one-line installers, `cargo install`)
are the terminal player with the window compiled out. mStream's binary bundles ship the
desktop binary; every other mStream install gets the terminal one. On Windows no console
flash where it can be avoided. Three lanes put it in, the same shape as the condition lanes:
- *Lane 7 (4e172c3)* — Cargo features: `window` pulls ratatui-wgpu, memmap2 and arboard,
  the only crates the window alone uses; `desktop` enables it and carries the desktop
  defaults; `default = []`, so a plain build, `cargo install` and today's release legs are
  the terminal player, whose `gui --window` is clap's usage error as on v0.9.0 (a test in
  both flavours). The patch section cannot follow a feature, so the vendored winit reaches
  both flavours through the visualizer; the vendored ratatui-wgpu reaches the desktop only,
  and it had pulled wgpu's default features (webgpu, naga's wgsl-out) back in — change 14
  trims it to std and wgsl, and the terminal flavour's wgpu and naga feature sets equal
  main's on all three targets. CI tests both flavours on ubuntu and windows, adds a macOS
  leg for the desktop flavour, lavapipe for the render tests on ubuntu, the linkage guard on
  both flavours' Linux release builds, a guard that the terminal flavour pulls none of the
  window's crates, and the wasm check with the feature on.
- *Lane 8 (2424a3d)* — the desktop flavour's contract. The rule: an explicit argv means the
  same in both flavours; only an empty argv differs, the window in the desktop flavour and
  the TUI in the terminal one, with the TUI as the fallback where no window can be
  expected (no DISPLAY, WAYLAND_DISPLAY or WAYLAND_SOCKET on a Unix other than macOS; SSH
  with a tty) and Finder's `-psn_*` arguments counted as none. A first run with no saved
  server shows the Library screen's "no server saved" line and the Add a server button.
  The empty-argv launch takes `<config dir>/desktop-player.lock` with the launcher's
  sidecar (sharing the tray's path is open); the sidecar's host reads "window"; `--version`
  adds a `features: window` line under the unchanged first one; exit 3 wherever no window
  can open at all, for a launcher to fall back on; on Windows FreeConsole when the console
  is ours alone, and a GUI-subsystem stub bin, mstream-player-launch (packaged as "mStream
  Player.exe"), that starts the player with CREATE_NO_WINDOW so a shortcut opens the window
  and nothing else. Terminal flavour unchanged (1056 tests, the expect leg, a bare launch
  shows the wizard); desktop 1120 tests, the window on an empty argv in ~210 ms warm.
- *Lane 9 (a4328e9)* — release.yml: the leg steps once in build-binary.yml (workflow_call);
  `build` is the six terminal legs unchanged in effect; `build-desktop` the four desktop
  legs with `--features desktop`, signed and notarized alike, held back from stable tags
  until the repository variable DESKTOP_RELEASES is 'true'; a tag with a '-' is a
  pre-release, never latest, and `channels` runs only for a stable tag (before this, an rc
  tag would have bumped the formula and Scoop and been served by the installers' latest);
  the desktop binaries join manifest.json through the existing loop, apiVersion unchanged.
  Only an rc-tag run proves the reusable call, the secrets reaching the sign step, the
  cross container and the Windows desktop leg.
- *Lane 10 (3504cf9)* — identity: `scripts/icons.py` makes the hicolor PNG set and the
  macOS .icns from the Windows .ico (256 and 512 px upscaled from 128), with a Linux
  desktop entry under assets/linux (not packaged yet); `src/identity.rs` holds APP_ID
  "io.mstream.player" (app_id and WM_CLASS, the entry's name, the codesign identifier) and
  the AUMID "mStream.Player" shared with the stub; the window carries the logo as its icon,
  names itself on X11 and Wayland, and on macOS sets the Dock image, so the bare binary
  shows the logo (first present unchanged); a refused second launch raises the holder's
  window on macOS and Windows (never a call that waits on it), the line still printed.
- *Lane 11* — the packages: a package-desktop job (skipped with build-desktop) assembles
  "mStream Player.app" from the signed binary (bundle id io.mstream.player, the Bonjour
  and local-network keys, the minimum OS from the binary's own load command), signs it
  inside-out, notarizes, staples and ships it as a .app.zip; a Windows zip with the player,
  the launcher stub as "mStream Player.exe", the icon and an unsigned-build note; a
  deterministic Linux tarball with the desktop entry and icons. Scripts under scripts/
  run the same code here and in CI; the .app assembled from the debug binary opened from
  Finder with its id, name and logo. Only the rc-tag run proves the signing and the plumbing.

**The rc run (v0.10.0-rc.1, run 36948308093, 2026-10-02).** The tag at b760349. Proven:
the reusable build workflow is called for both families; all three terminal Linux legs and
the desktop linux-x64 leg built inside the cross container and passed the linkage guard
(the window build keeps NEEDED clean); both Windows legs built, the desktop one compiling
the Windows-only code (FreeConsole, the launcher stub, the FileDescription) for the first
time and staging the stub beside the player in its artifact; the gate held: with the
macOS legs failed, package-desktop, release and channels were skipped, no release object
exists and latest still names v0.9.0. The four macOS legs signed and then notarization
answered HTTP 403, "A required agreement is missing or has expired" — Apple's Developer
Program agreement awaiting the account holder's acceptance (v0.9.0 had notarized 25 hours
earlier). Once accepted, a rerun of the failed jobs went green end to end: the four macOS
legs notarized, the four package legs ran (the arm64 and x64 bundles "Accepted", stapled,
"The validate action worked", spctl "accepted, source=Notarized Developer ID"), the
release was published as a pre-release with 21 assets (both families, the three packages
and the two app zips, the stub dropped from dist), latest still v0.9.0, and channels
skipped — the Homebrew and Scoop repos untouched. On this Mac the downloaded arm64 app zip
verifies (codesign --deep --strict, stapler validate, spctl Notarized Developer ID, bundle
id io.mstream.player, version 0.10.0), opens from Finder's `open` with its window and the
default lock, and quits clean; the Windows zip holds "mStream Player.exe", the player, the
icon and the README; the tarball its layout. The rc's binaries print 0.9.0 because
Cargo.toml was not bumped; a stable tag bumps it first.

**Lane 14, the text and glyph conditions (2026-10-02).** A bundled six-glyph symbol face of
our own (★ ☆ ✓ ✔ ✗ ✘, 1.6 KB, drawn by scripts/symbol-font.py under the OFL, since no
subsetter or fitting OFL face was on this Mac) draws the GUI's own symbols ahead of any
system face; the platform's colour emoji face is mapped like the CJK faces with the
renderer's png feature on for window builds, and a VS16 heart, a regional-indicator flag, a
tag-sequence flag, a ZWJ family and a skin tone each draw as one coloured picture over the
two cells they claim (verified on Apple Color Emoji and Noto Color Emoji's bitmaps; Segoe's
COLR layers unrun); fallback faces draw at the primary face's pixels-per-em, so 한 か 日
ink at one height; a pasted subdivision flag keeps its tag characters and a clipped field
shows a cluster whole or not at all; a face that has the cell's first character wins it, so
a stray tag run is never a box (vendored changes 15–18). The lane found that the kit and
the GUI chrome measured text per character while ratatui paints per grapheme (❤️ and a
keycap spilled a cell past a field, a ZWJ family wasted four): kit::width now applies
ratatui-core's own rule in both flavours, with parity tests against a ratatui Buffer. Left:
src/tui/ui.rs's own per-character fit and tail, the admin rooms' clips by character count,
the caret stepping per character (tui-input), the renderer's one-cell halfwidth dakuten
against ratatui's two, a wider symbol face (Miscellaneous Symbols, Dingbats) once a
subsetter is available, and the emoji faces unrun on Windows and Linux.

**Lane 15, the behaviour conditions (2026-10-02; 4b409ad and 3f1e137).** Input that arrives
before the first frame is held (a bounded queue of the window's own raw events; a move
replacing the move before it, only the last resize and scale kept) and replayed one act per
frame after it, so a `/` typed into a blank window opens the search and the letters after it
land in the field — which needed the player's first effects, the connect among them, sent
before the first frame, since `/` opens the query only once connected. A cover is never
painted under a modal on the modal's first frame: the kit's Surface reports each overlay as
it is registered, the Board marks the covers placed before it that it touches, and
CoverPost skips them (order matters: a cover inside its own modal lies under that modal's
footprint); on the opening frame none of the modal's pixels differ from the settled frame; a
cover straddling the modal is left out whole for that frame. The renderer is built off the
loop thread: the vendored builder splits into create_surface on the loop thread and a Send
build of everything else (change 19), polled with the early threads; with 1.5 s planted in
the pipelines step the loop kept answering and a scripted resize during the build was kept;
first present unchanged within noise. Found on the way: the frame that made the player
visible on macOS was a 40 ms full repaint from a same-size Resized and a same-scale
ScaleFactorChanged as the window came on screen — both skipped (the resize still asks for a
redraw), and Occluded(false) presents what the backend holds (change 20), so the player is
seen ~30 ms sooner. CI's first run of the glyph tests on ubuntu and windows corrected three
expectations that held only on this Mac's faces (Noto's grey family silhouettes, Segoe's
pictureless England flag, Malgun Gothic's taller Hangul; the test's tolerance is 20% with
the reason). Left: a `/` typed after the first frame but before the connection still opens
an empty search (older than this lane); the lever's own `text` step is not paced; whether
Segoe's layered glyphs paint at all (the Windows machine); a wheel held before a click
rides along with it, so the click resolves against the pre-scroll layout.

**Shipping (the owner's decisions, 2026-10-02).** PR #41 is for merge. The vendored crates
ship as they are, under `[patch.crates-io]` (winit in both products, ratatui-wgpu in the
desktop one); vendor/ratatui-wgpu/VENDORED.md and vendor/winit/VENDORED.md are the record of
every change, and filing those changes upstream is a separate task that gates nothing. The
steps to the stable release, in order: merge PR #41; bump the crate version to 0.10.0 (the
rc's binaries print 0.9.0); set the repository variable DESKTOP_RELEASES to 'true' (until
then a stable tag builds only the terminal family); tag v0.10.0. Owed before that tag, as
proof rather than code: the Windows run of the published zip (the stub, the console, taskbar
grouping, the emoji and Hangul faces, at 100% and 150%); Linux on real hardware (the
tarball, X11 and Wayland, the desktop entry's app_id); the few macOS checklist rows still
unrun; Korean. After the first desktop release: a Windows code-signing certificate, a
Homebrew cask and a Scoop manifest for the desktop zip, an updater for the hand-downloaded
packages, and the wider symbol face (Miscellaneous Symbols, Dingbats) once a subsetter is
available. The mStream integration, parked behind other work at the time, landed on
2026-10-03 (under From here).

**From here, in order.** Done since the list was first written: the vendored-crate review
(they ship, above), the window compiled out of the terminal flavour, the open window
conditions (lanes 14 and 15), the hold-back (lane 9) and its rc run, and the README split into
two products (2026-10-02). The release gate and what follows it are under Shipping. On
mStream's side, merged 2026-10-03 after being parked a day behind other work: the manifest
updater taking the desktop names and refusing pre-release tags, and the bundler staging the
desktop entry under the terminal file name while the runtime fetch keeps the terminal one
(mStream PR #1049); the launcher probing `features: window`, starting the window directly
and falling back to the terminal route on exit 3 (mStream PR #1050); and the pins bumped to
v0.11.0, the first stable tag with the `-raw` names (mStream PR #1052, whose bundle legs
staged the desktop binary on macOS arm64 and x64, Windows x64 and Linux x64, and the
terminal one on Linux arm64). The notify path that fires is the mStream notice below. Left
on that side: the three-way coexistence check and the mStream release. Still open for the owner: a
shared lock with the tray, close as quit or keep playing, Ghostty's role for the setup and
admin faces (the research of 2026-10-02 found all three faces can open in a window behind a
Face trait, about four lanes; parked with the mStream work). Settled on 2026-10-02: vendored
winit in terminal releases (it ships, above), the Windows certificate (after the first
desktop release), the .app bundle id (io.mstream.player, the codesign identifier already in
use, shipped by the rc) and the packaging tool (our own scripts under scripts/, no
cargo-packager or Velopack; an updater comes after the first release).

**The mStream notice (2026-10-03).** The "notify path that fires" above: notify-mstream.yml
never fired. It listened for `release: published`, and the Release is created inside
release.yml with the workflow's own GITHUB_TOKEN, whose events, a workflow_dispatch or
repository_dispatch aside, start no workflows — zero runs since 2026-08-20, and every pin bump
to date was done by hand on the mStream side (the updater script run locally through v0.5.0, a
workflow_dispatch of update-mstream-player-manifest from v0.6.0 on). The dispatch now runs as
release.yml's `notify` job, beside `channels` and gated the same way, on stable tags only (a
pre-release is not an adoption candidate, and mStream's updater refuses it anyway);
notify-mstream.yml is the hand crank, and both run scripts/notify-mstream.sh, which validates
the tag the way mStream does and fails loud with GitHub's answer when the token is refused.
The proof: hand-crank the tag mStream currently pins (an older one would open a downgrade PR
there), and its workflow answers "pins already current — nothing to do".

**The manual checklist** (`docs/window-spike/checklist.md` has every item with its expected
result; the shape): macOS — launch from Terminal and iTerm; the key walk (digits, arrows,
Home/End, PageUp/Down, Tab and Shift+Tab, Enter, Esc, Backspace, Space, T); key repeat on
Down and Backspace; Cmd+V, Cmd+Q, Cmd+W; dead keys on ABC Extended (Option+e e → é, ` a →
à); Japanese Romaji and Pinyin in the search box; CJK titles in an en library; hover, click,
right-click, queue drag, wheel and trackpad fling; a drag released outside the window; a
slow corner resize and a move between Retina and non-Retina displays; the ground colour
beside Terminal.app with Digital Color Meter; CPU in the visualizer tab; playback continuing
while the window is covered, minimised and behind a Space switch. Windows (11, 100% and
150%) — launch from PowerShell and Windows Terminal; the key walk plus Ctrl+C; AltGr+Q/E on
German and AltGr+A on Polish; US International dead keys; Microsoft Japanese and Pinyin
IMEs; Ctrl+V; clicks on the first and last rows and the transport at 150%; close with X and
Alt+F4, then relaunch; a move between 100% and 150% monitors; CPU and memory idle and in the
visualizer tab.

**The keyboard check (2026-09-30).** The operator ran the macOS half at a real keyboard:
dead keys (Option+e e → é and the rest) and every standard key passed. Accessibility was then
granted to the Claude app, and the rest ran as real OS key events posted through a CGEvent
helper (`keys.swift` in the session scratchpad: key down and up with the autorepeat flag,
modifier flags, layouts switched through the Text Input Sources API), which also proved the
OS-to-winit-to-raw path the script lever had skipped: `/` opened the search card and `cass`
arrived through it; key repeat works (Down held into a list, Backspace held for one press and
five repeats took six characters); on the German layout Option+l, Option+e and the y key typed
`@€z`; Cmd+V pasted nothing and typed nothing (the known gap, safely), Cmd+W did nothing,
Cmd+Q quit and wrote the config; on the Russian layout Ctrl+c quit. One bug: on Dvorak,
Ctrl+' reached the GUI as `q` — the key's place stood in for every key the layout named
with something other than a Latin letter, and the GUI quits on `q` whatever the modifier —
so the player closed. Fixed the same day (the place stands in only for a letter of another
script; a unit test pins it; re-run at the keyboard: Ctrl+' does nothing, Ctrl+c on Russian
still quits). Also learned: the GUI's `q` quit ignores modifiers in both the terminal and the
window, so Ctrl+q quits too — pre-existing, and the same on both paths. Still untested: the
IMEs. This Mac has Japanese Romaji installed but no second input source added under Keyboard
settings, so the menu bar offers no switch, and the Text Input Sources API refuses to select
an input method programmatically (paramErr −50, even from a process with a key window),
while it switches keyboard layouts freely. Once the operator added Japanese and Pinyin under Keyboard settings, the API selected them (the
earlier refusal was a source outside the enabled set), and the IME rows ran the same way, judged
from screen dumps every two seconds and the window's preedit log. **Pinyin passes**: typing
`zhongguo` composes (`zhong guo` in the log, nothing typed into the box), Space commits `中国`
into the search box, twice out of two runs — though in two earlier runs the input method never
attached and the letters arrived as Latin text, so attachment is flaky. **Japanese behaves as the system does, which the first reading
got wrong**: `nihon` composes (`にほん`), and then what Enter does depends on whether Space was
pressed. Without Space, one Enter commits and the next submits the search. After Space, which
opens Kotoeri's candidate list, the first Enter only closes the list and the second commits —
and a plain AppKit text view, built as a control and fed the same posted keys, does exactly the
same, as does stock winit against the backported one. The first write-up called this a winit
0.30.13 defect of the rust-windowing/winit#4478 class; it is not, and #4478 touches only the
Korean paths. That backport is carried anyway (lane 1 of the conditions vendored winit for it):
it fixes documented Korean losses — an ASCII key typed right after a commit, a doubled Space —
that this Mac cannot test because no Korean source is enabled. Esc discards a composition on
its second press, also native. Also observed: with the Japanese IME
on and no text field focused, the digit `2` went into a full-width composition (`２`) instead
of switching rooms — a terminal composes the same way, but it is the case for gating IME to
focused text fields. Windows remained unrun at the time; the Windows run and retest
above came with the conditions.

**Driving it.** The levers above plus `MSTREAM_WINDOW_DUMP=<dir>` (the backend's text against
a `TestBackend` render of the same Gui — reliable only with no server, since a live listing
can land between the two draws). Screenshots of a background window: a swiftc
`CGWindowListCopyWindowInfo` lister for the id, then `screencapture -l <id> -x -o`; quit
through `NSRunningApplication.terminate` for the Cmd-Q path. Every cargo call on this Mac is
`cargo +1.98.1` (stable is 1.94.1; egui 0.36 wants 1.95). The scratch drivers do not survive
a session; the levers and `docs/window-spike/` do. Only a `--features desktop` build has the
window (`cargo +1.98.1 build --features desktop`); a plain build has no `gui --window`.

**Recommendation** (the judge's verdict; the framing that follows it is the author's). Go,
with the conditions above and the flag hidden until the gate clears. The budget of going:
about 2.7–2.9× the terminal's CPU in the visualizer tab and about 78 MB more resident
memory, in exchange for one executable per platform with no bundled terminal and pixel art
wherever a GPU rasterises — a claim proven on macOS, run once on Linux under a CPU
rasteriser, and (at the time of the verdict) not yet run on Windows; the Windows 10 run and
retest above came later and passed. The terminal-host route (Ghostty on macOS, Windows
Terminal on Windows, the user's terminal on Linux) ships today and does not depend on this,
so the author's proposal is a phase of its own rather than a rider on the packaging work,
starting with the manual checklist on macOS and Windows; if dead keys or IME fail there, the
author's fallback to try first is egui text fields over the surface (egui and its IME
plumbing are already linked for the visualizer's controls) — a proposal, not something the
spike tested.

### Phase 15 — Faces in the window: the wizard and Quick Connect leave the terminal, Ghostty leaves the mac bundle (2026-10-03)

> **Status (2026-10-03): the four player lanes are on `claude/faces-in-window` (cut from main at 6fb86e8, v0.11.0; player PR #48): the Face trait, the wizard's halves, the wizard's face and the capability word, each recorded below; on mStream's side the launcher's page route is PR #1061, inert until the pin carries the word; the v0.12.0 release shipped on 2026-10-03 (below), and on mStream's side the launcher's page route (#1061), the pin at v0.12.0 (#1062) and Ghostty's removal (#1063) all merged on 2026-10-04, so the next mStream release ships no bundled terminal; the owner opened `setup --window` on a Mac against the e2e fake server and it worked.** The research of 2026-10-02 (parked under Phase 14's open items) held against the tree and narrowed: PR #45 hosts the admin rooms inside the GUI's window and the mStream tray dropped its Manage-server rooms, so only two faces still need a window of their own for Ghostty to leave the mac bundle, the setup wizard and the Quick Connect page, and both are `src/setup/mod.rs`'s one `Wizard` in one loop (`run` and `run_qr` end in the same `run_tui`), so winit's one-loop-per-process rule never bites. The window host (`src/gui/window/mod.rs`) is face-agnostic everywhere but the call sites that name `Gui` and `Ctx` (the imports, the App fields, the no-display finish, the hosted Graphics and overlay watch, send_early, the title, the frame and input calls, the caret and composition reads, the copy chord, the script probes, flush and finish, the fidelity dump), so the plan replaces them one for one with a `Box<dyn Face>` behind a small object-safe trait in `src/gui/window/face.rs` (title, grid, frame, input, caret_at, set_composition, host_pictures, send_early, copy, flush, finish, exit_code, hit_debug, drag_began, render_test, the optional ones defaulted) and makes the GUI its first face (`GuiFace` in `src/gui/mod.rs`, wrapping today's calls verbatim; `gui::frame` takes `&mut dyn Host`); everything else in the host (build, show, held-input replay, IME placement, script, stats, the lock) stays as it is. The wizard's loop is already two halves either side of `event::poll` with one terminal write inside (the OSC 22 pointer), and the split into `setup::frame` and `setup::input` is terminal-neutral, as are the three field sites moving from `kit::input_display` to `kit::field_display` (`src/kit/mod.rs`, from the admin hosted-fields commit; the line is byte-identical while the composition is empty, and the note is what the window reads to turn its paste and input method on) and the two picture gates accepting a hosted surface (`done_two_column` and the folders wordmark gain `|| graphics.is_hosted()`, as `src/gui/cover.rs` does): a hosted `Graphics` answers `protocol()` None, so without the gate the window's 100×30 grid would show the Done page's `done.too_short` and no QR at all, where the hosted two-column page draws the raster code in its left column at any size. `setup --window` and `qr --window` (hidden flags, compiled only with the window) fork before `run_tui`'s first terminal write into `run_window`, which pins truecolor and modern glyphs like `gui::run`'s arm and hands a `WizardFace` (`src/setup/face.rs`) to `window::run` with no instance lock, so a windowed wizard opens beside an open player as `qr` always has; the host answers 3 wherever no window can open, 0 for every way out of the wizard and the close button, 1 for a frame error. The desktop flavour's `--version` second line becomes exactly `features: window window-pages` (`src/main.rs`; the launcher reads only that line and splits on whitespace, so `window` still matches), pinned by the main.rs test and by ci.yml asserting the line, the no-display exit 3 of `setup --window` and `qr --window`, and the terminal binary's exit 2, each right after its flavour's release build on Linux; the hosted Done page is proven headless twice, by a RecordingHost under a TestBackend on every CI host and by a ratatui-wgpu render through the Board's post-processor in `src/gui/window/render_tests.rs` on lavapipe and Metal. On mStream's side, in order: the launcher's page route (`parse_player_features` and `PlayerProbe.pages` in rust-launcher/src/paths.rs; `open_player_page`, a pure `page_verdict` that never says Refused because a page holds no lock, exit 3 at any time to Terminal.app's `.command` then the browser, exit 1 early to the browser, per-page logs beside desktop-player.log in tray_app.rs; `player_words` already puts `--window` after `setup`/`qr`), merged inert against the v0.11.0 pin and proven by stub legs in test/smoke/player-open-smoke.sh under xvfb including the gate leg; the pin bump to v0.12.0 with build-bun.yml asserting the staged player carries the word; then Ghostty's removal in one PR (the console block of scripts/build-bun.mjs, the io.mstream.console component and its smoke in build-bun.yml, bin/ghostty/, ConsoleLaunch and find_console_app in paths.rs, spawn_ghostty_page, ghostty_page_config, console_running and MacFocus::Console in platform.rs, the console parameter through tray_app.rs, the smoke's console lever, docs/install.md) with build/pkg-postinstall.sh removing the old `/Library/Application Support/mStream/console` and forgetting the receipt before its launcher guard, the pkg smoke seeding and asserting it; the Linux chain's `ghostty` candidate and the sidecar host word stay, since they are a user's own Ghostty. Lane order: the trait and the wizard split in parallel, the face on both, the release; the launcher route any time, the pin after the release, the removal after the pin, so the mac bundle never loses a pixel-capable route for the pages. Settled by default unless the owner says otherwise: the word over a numeric gate, both wizard exits 0 (Face::exit_code and a reserved verdict row keep the completed-vs-abandoned signal a later one-row change), Terminal.app then the browser on exit 3, duplicate page windows allowed, osascript `choose folder` kept, v0.12.0 stable directly, the removal one mStream release after the pin, 100×30 for the faces, per-page logs. Still owed live on a Mac, not headless: the first `qr --window` and `setup --window` runs (two-column page, flattened wordmark, one Dock tile, Cmd+V and the input method in the fields, Esc exits 0, a second page beside the player) and the osascript folder chooser fronting over, and handing focus back to, the winit window, which no recorded run has tested.

**The lanes, in dependency order.** P1-face-trait (player: a Face trait in the window host; the GUI as its first face (pure refactor)), P2a-wizard-halves (player: the wizard's loop in two halves, hosted-aware picture gates, fields that note their caret (terminal-neutral, no window feature)), P2b-wizard-face (player: WizardFace — `setup --window` and `qr --window` in the player's own window, exit 3 when none opens, proven headless), P3-capability-release (player: the word `window-pages` on `--version`'s second line, its CI gate, the records, and the v0.12.0 desktop release), M1-launcher-page-route (mstream: Setup and Quick Connect open in the player's own window when the probe says `window-pages`, with page verdicts; inert against the v0.11.0 pin), M2-pin-bump (mstream: pin the player at v0.12.0 and assert the word at bundle time), M3-ghostty-removal (mstream: Ghostty out of the mac bundle — bundler, pkg component, launcher console code, smoke lever, docs, and a postinstall that removes the old console (one PR)). The player lanes land on this branch; the mStream lanes on branches of the mStream repo. Each lane's record follows here as it lands.

**P1, the Face trait (2026-10-03).** The window host shows a `Box<dyn Face>` behind the object-safe trait in `src/gui/window/face.rs` and the GUI is its first face (`GuiFace` in `src/gui/mod.rs`, each method the call the host made into the Gui by name before, `send_early` moved into it whole and `gui::frame` taking `&mut dyn Host`), with two seams the plan did not list closed the same way: the script's dumps read the cursor shape the window was last handed (`WindowHost` keeps it) where they read the GUI's `Ctx`, and `Counted`, `CoverPost` and `Board` are `pub(crate)` because a face's frame half names the window's terminal; it is a pure move, proven by the desktop suite (1398 passed, the 15 GPU render tests among them) and the terminal suite (1302 passed) with three new tests (`the_faces_defaults_are_the_optional_surface`, `dump_is_generic_over_faces`, `gui_face_mirrors_the_guis_caret_and_title`), clippy finding nothing new in either flavour (1.98.1's `-D warnings` gate exits 101 in both flavours on the base commit and here alike, 42 errors in the binary and 73 in its tests at the same 90 sites in 24 files, none on a line this lane wrote, so the gate cannot pass on this tree until those are cleared), and the live `MSTREAM_WINDOW_DUMP` EQUAL run on a Mac still owed.

**P2a, the wizard's halves (2026-10-03).** The wizard's loop is `setup::frame` (the draw, the worker's answers, the dispatch, the pumps, generic over the backend with the pointer's hand handed to a callback and a zero wait when the scan's progress poll is due) and `setup::input` (the terminal loop's match, one event at a time, `Some(Outcome)` when it ends the wizard) around the terminal's own `event::poll`, its three fields (the rename chip, the login fields, the path modal) draw through `kit::field_line` and `kit::field_display` and note their caret, the line the same as before while nothing composes and a secret's composition drawn as its marks, the Done page's two columns and the folders wordmark accept a hosted `Graphics` beside a probed protocol (`Wizard::adopt_graphics` taking either, as `run_tui` now does with the probe), and `render`, `Outcome`, `spawn_worker`, `session_title` and `dispatch_queued` are `pub(crate)` for P2b's face, which carries `Op`, `Done`, `Job` and `ProgressReport` with them because `frame`'s signature names the channels (the `private_interfaces` lint), while the wizard's fields were the last callers of `kit::input_display` and its two helpers outside the tests, so those are marked test-only there rather than left to warn; it is proven by the terminal suite (1305 passed) and the desktop suite (1401 passed) with three new tests (`the_focused_field_notes_its_caret_and_a_modal_clears_it`, `the_loop_halves_compose`, `the_frame_half_asks_for_an_immediate_redraw_when_progress_is_due`) and the protocol arm added to `the_done_layout_is_decided_by_capability_not_data`, by `test/e2e/run.sh` passing all 24 of its checks against both flavours' debug binaries (the qr-pixel leg's stream still carrying `1337;File=`), and by clippy reporting the base's 73 diagnostics on the same source lines in both flavours and nothing else, with the hosted arm of the gates left to P2b, where `Graphics::hosted` exists.

**P2b, the wizard's face (2026-10-03).** `setup --window` and `qr --window` (hidden flags compiled only with the window, `pub(crate)` on `SetupArgs` and `QrArgs` so main's clap test can read them) fork after each entry has queued its first op and set its language, into `setup::run_window`, which is `gui::run`'s window arm (the X11 keyboard check on Linux, truecolour and modern glyphs pinned, the worker spawned) handing a `WizardFace` (`src/setup/face.rs`) to `window::run` with no instance lock; `run_tui` is unchanged, and so are main's dispatch and lock match, with a comment at the setup arm saying why a page takes no lock and writes no sidecar. The face answers each ask with the wizard's own call (the title is `session_title`, the frame and input halves are P2a's, the caret and the composition are the Surface's, `host_pictures` adopts `Graphics::hosted` and watches the overlays, `send_early` dispatches the queued op, `hit_debug` formats the hit, `render_test` is `render`), matches `Outcome` whole so that a later finished-or-abandoned outcome is decided beside `exit_code`, and leaves grid, copy, flush, exit_code and drag_began to the trait's defaults; its doc says why the window's input translation needs no wizard arm and which code each way out gives, the Cmd-Q that ends inside AppKit included. The wizard's four modals draw through `kit::modal_frame_on` and `kit::modal_frame_anchored_on`, so each registers its footprint, the watch hands it to the window's Board, and the Board stands down a picture placed beneath (the directory browser's top border crosses the folders wordmark's band at 100×30, and the language list does at 25 rows); in a window the folders wordmark draws as the figlet where an overlay stood last frame, and the frame half asks for the next frame at once when the overlays move, as the GUI's covers and its hot frame do, while a terminal draws exactly as before. The wizard's tooltip registers no overlay: the language chip's tip touches the wordmark's band at every width, so a registered tip would swap the wordmark for the figlet on each hover of the chip, and only a window narrowed below about 74 columns brings the tip and the picture's pixels together. ci.yml's Linux release steps check that the terminal binary's `qr --window` exits 2 and that the desktop binary's `setup --window` and `qr --window` exit 3 with no display, each right after its own build; the steps catch the code with `|| code=$?`, since Actions runs bash with `-e` and a bare `cmd; test $? -eq N` would end the step at the command. It is proven by the terminal suite (1305 passed, `gui_window_exists_only_where_the_window_does` extended to the pages) and the desktop suite (1410 passed: seven face tests, among them `the_hosted_quick_connect_page_is_two_column_at_the_windows_grid`, `every_wizard_modal_tells_the_window_its_footprint` and `a_modal_over_the_wordmark_stands_it_down_and_the_figlet_stands_in` on a recording `PictureHost`, and two in `src/gui/window/render_tests.rs`: `the_quick_connect_page_draws_its_code_in_the_window`, which reads the code's white quiet zone, a dark module and the ground in the band's slack off a Metal frame, and `the_wordmark_is_not_painted_over_a_wizard_modal`, which reads only the ground in the band's rows above the browser on the frame it opens and the figlet on the next), by `test/e2e/run.sh` passing its 24 checks against both flavours' debug binaries, by clippy reporting the base's sites and no new one in either flavour, and by three scripted runs in a real window on this Mac against the e2e fake server: `qr --window` was EQUAL at frame 5 with its pictures at 2,2 30×4 and 2,7 30×15, a click on the Android button hit `OpenPlayStore` and noted the store's URL, and Esc exited 0; `setup --window` placed the wordmark at 13,2 74×5 with no figlet, the path modal turned the input method on, drew a preedit in its field and took the commit, and `q` exited 0; a third, `setup --window` resized to 100×25 with the language list opened over the wordmark, dumped the picture, then on the frame the list opened no cover painted and one under an overlay, then the figlet around the list, then after Esc the picture again, and exited 0. Two things differ from the plan. The two-column page has no Quick Connect heading (the wordmark is its header, as `draw_done` says), so the GPU test reads the right column's buttons and the absent `done.too_short` instead. And a test binary may resolve the palette from a bare environment before a test can pin truecolour, so the tests' hand-over asserts that the wordmark exists exactly where the palette has a ground and builds it on the window's ground otherwise; the window itself pins before anything asks. Still owed: the exit-3 steps run only on CI's ubuntu, since a Mac always has a display; Cmd+V into the fields, the osascript folder chooser fronting over the window, the Dock tile and a page beside an open player, by hand on a Mac; and the host's no-display and loop-failure lines still begin `gui --window:` for a page. The admin rooms as faces of their own stay out of scope: they are hosted inside the GUI's window since PR #45.

**P3, the capability word (2026-10-03).** The desktop flavour's `--version` second line is exactly `features: window window-pages` (`const VERSION` in `src/main.rs`): one line of space-separated words and never a third, because mStream's launcher reads only line 2, strips `features:` and splits on commas or whitespace, so `window` still names the desktop build to every launcher that ever read it (the pin at v0.11.0 included) and `window-pages` is the word a launcher that knows it gates the pages' window route on; the terminal flavour keeps its one line. The main.rs test pins the exact line, its `features: window` prefix and the absence of a third line in both flavours, and ci.yml's Linux release step asserts the line on each flavour's release binary right after its build, beside P2b's exit checks (`wc -l` is 1 for the terminal build; `sed -n 2p` is the exact string for the desktop build), since release.yml never reads `--version`. The README and docs/window-spike/README.md quote the new line. **Shipped as v0.12.0 on 2026-10-03** (release commit 5982133 on main, tag at it, release run 37160913037 green end to end: both families, the three desktop packages notarized, Homebrew and Scoop bumped, 21 assets, not a pre-release; the published darwin-arm64 desktop binary prints `0.12.0` and `features: window window-pages`, the terminal one prints one line and answers `qr --window` with exit 2). The release's Notify mStream job fired its dispatch (`notified mStream: v0.12.0`), the first automatic run of that path since it moved into release.yml, and mStream's update-mstream-player-manifest ran from it (run 37162124631) and opened the pin PR #1062 (`bump pins to v0.12.0`, the same ten binaries at their new hashes). Release notes name the two pages, the version line and the known limits.

**The Windows smoke of v0.12.0 and its fixes (2026-10-03).** v0.12.0 was smoked on Windows 10 22H2 (a GTX 1060 beside an Intel UHD 620, one 3240×2160 display at 200 %, rustc 1.98.1) in both flavours: the terminal player with its visualizer child and `viz-probe`, and the desktop player's window faces (`gui --window` and the empty-argv launch, `setup --window`, `qr --window`, `viz-window`) driven by `MSTREAM_WINDOW_SCRIPT` and checked by `MSTREAM_WINDOW_DUMP`, the window's faces over CJK, emoji and flag titles, a second launch against a minimised player, and mStream's launcher routes to the player and to the two pages. What held: the queue drew every emoji title whole, ❤️, the bundled stars and tick drew right, seven of the eight built-in presets drew, and the player's own window was a plain app window (extended style 0x00040110). What did not is fixed on `claude/windows-smoke-fixes`, each fix with a sentence-named test, and all but the last with a reviewed follow-up: "desktop: a crash in the graphics driver exits instead of hanging behind WER" and "desktop: the terminal flavour's visualizer exits on a driver crash too"; "window: a failed opening is said once, not three times" and "window: a failed opening polls from wherever it failed"; "window: the native dialogs belong to the player's window on Windows" and "setup: the owner test reads the dialogs the pickers actually open"; "window: an emoji draws from the colour face, an unjoined flag as its letters" and "window: a composed family drawn whole, and a flag letter kept when its cell changes" (VENDORED.md changes 21 and 22); "desktop: raise the holder's real window, not winit's helper, when it is minimised" and "desktop: a console that reports the holder's pid is not the window to raise"; "window: a wide cell erases every glyph it lands on, so the bar keeps the space after an emoji" and "window: say truly why draw blanks a stranded continuation, and pin the bar test's glyphs" (change 23, 21 on its own branch, renumbered at the merge); "setup: the wizard measures its labels in cells, and the Done message gets the rows it wraps to", "admin: the gates and the right-anchored buttons measure in cells, as the kit now draws" and "setup: the Done page keeps its code before the scan status, and keeps scan rows only where a scan can be"; and "viz: 09 MountainBytes samples its terrain at level 0, so FXC builds it on Windows", a single commit with no follow-up, whose test is the existing `every_pass_of_every_preset_compiles_for_every_backend` in `src/shader/matrix.rs`, extended with an FXC (ps_5_1) arm that runs every pass through FXC on Windows, rather than a new one. The driver fact behind the first pair: NVIDIA 31.0.15.3640 fast-fails (0xC0000409) in nvwgf2umx.dll when the executable's path is 253 UTF-16 units or longer, under DX12 and Vulkan alike, so no backend choice avoids it, and through v0.12.0 Windows Error Reporting held the crashed process behind its "has stopped working" dialog, with no window and no exit, so mStream's launcher read a page as up. The player now sets `SEM_NOGPFAULTERRORBOX` before any device in both flavours and exits within a third of a second of the fault, and each of the three ways into a device (the window host, before its GPU thread starts, the visualizer's own process and `viz-probe`; `gpu_pick::long_path_warning`) says, the first two on stderr and the probe in its report, that a path of 250 units or more may crash some drivers and the player should move; mStream's launcher (its branch `claude/launcher-window-focus-and-crash-fallback`) sends a page that crashes at any time to the terminal and raises the player's real window, not winit's helper. The two groups' branches were integrated as one; on it the desktop suite passed 1423 with 50 ignored (the 25 GPU render tests among the passes) and the launcher binary's 4, the terminal suite 1307 with 49 ignored, the wasm check passed with and without `desktop`, the terminal flavour's tree held no window crate, and its release build printed one version line and answered `qr --window` with exit 2. macOS and Linux have these changes only through CI's suites. Some follow-ups' commit bodies name the commit they review by its SHA on the lanes' pre-integration branch (`claude/windows-smoke-fixes-t`), which this branch does not contain: 2eeaa5c there is 8439eae here, 6b85e8d is ba949d0, and f6615e5 is 0f5db1b. The bodies of 8439eae, ba949d0, e3e3b56 and 6002f7a also record the two England-flag render tests (`a_pasted_subdivision_flag_shows_in_a_field_as_one_picture`, `emoji_sequences_draw_as_one_picture_in_their_cells`) failing, as they did on their lane's base, which had no emoji fix; in this history those commits come after 4b9bbc0 and 48dbf30, and both tests pass at the tip. A review of the integrated branch then added: "gpu_pick: the visualizer and the probe say the long-path warning too"; "setup: the tall Done cards keep the row above the note, and one function draws every bar's buttons" (80×24 had lost its tall cards to the one-row list, and the CJK bar test now draws the bars' own `bar_buttons`); "window: a failed opening's end is tested through a stand-in loop, and the no-adapter run is a checklist item" (item 25); "desktop: say truly what the launcher's window search shares with the player's" (the launcher's search has the tool/no-activate rule, not the console rule); and these notes. After it the desktop suite passed 1426 with 50 ignored, the terminal suite 1310 with 49 ignored, and the wasm check with and without `desktop`. A last round settled the review's four disputed low findings, each with its test ("desktop: the helper programs the player starts keep the default error mode", so Explorer, the openers and clip.exe no longer inherit the crash quieting while the visualizer's window still does; "setup: the skip warning's confirmation stays inside the modal's border", which the French pair overflowed at any width and five more languages at the 58-column floor, CJK fitting; "admin: library ticks and radio rows measure their names in cells"; and "window: a keycap draws from the colour face, judged by its whole sequence", VENDORED.md change 24, latent on Windows 10, where no text face maps U+FE0F), after which the desktop suite passed 1432 with 50 ignored (26 GPU render tests among the passes) and the launcher binary's 4, the terminal suite 1315 with 49 ignored, and the wasm check with and without `desktop`. Two reviews of that round then added: "admin: a library name wider than its row is drawn clamped, not dropped or spilled" (measured in cells, one long CJK name blanked the add-user ticks and spilled the backup radio over its border); "setup: the skip warning's confirmation moves up whole instead of being cut" (to the blank row above the way back, so French and, at the floor, five more languages no longer read a word cut in half, and the test no longer names which translations overflow); and "admin: the tab rows and the rest of the drawn rects measure in cells" (the four rooms' tabs, Stats' axis, note, period list and origin split, the users table's flag columns, the BETA chips and the Seeding ticks, with a Japanese tab test per room and the fit checks and clip helpers still counted by characters left as follow-ups), after which the desktop suite passed 1440 with 50 ignored and the launcher binary's 4, the terminal suite 1323 with 49 ignored, and the wasm check with and without `desktop`. CI's `windows-latest` (now `windows-2025-vs2026`, with Windows 11's Segoe UI Emoji, which composes the family inside one emoji's square: 19 by 20 px against Windows 10's 18 by 16) then failed `a_family_the_face_composes_draws_whole_in_its_cells` on its wider-than-tall rule, a fact of Windows 10's design rather than of compositions; "window: the family test holds the drawn family to the face's own shape, not to a landscape" and "window: the family test holds the drawn size too, so a first picture shaped like the whole is told apart" hold the drawing to the face's own composition at the renderer's own scale instead, and print the shaping when they fail. The branch merged as PR #49 (93fa260) and shipped as v0.12.1 on 2026-10-04 (release run 37203820867: all 19 jobs green, 21 assets, Homebrew and Scoop bumped); its `notify` job opened mStream's pin PR #1066 by itself, the first stable tag to do so, and mStream's launcher half is PR #1065.

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
| 63 | The API worker is strictly serial (`worker.rs:425`): one 20 s timeout head-of-line blocks every pane, and a Quick Connect dial can hold it ~48 s. The code concedes this once already — mDNS discovery got its own thread so a browse "shouldn't hold up a pairing attempt queued behind it". The admin panel's log tailing arrives on this seam next (2026-10-02: it did not — the GUI's server log polls on a worker thread of its own, `src/gui/server_log.rs`, so the serial worker is still the open question for everything else) | decide the concurrency story before B4: dispatch onto the shared runtime with a generation token, or formalise the `spawn_discovery` escape hatch |
| 64 | Reply routing is encoded in the variant name — `ApiCmd::SearchDrill` exists only so a Library reply lands in a different pane, duplicating the Library command and event wholesale. Each new operation costs four compiler-checked places plus `note_pending`'s silent `_ => continue` | carry the destination in the command and echo it back on the event; derive the spinner's tab from the same field |
| 65 | `mstream-player dj` documents itself as "the scriptable view of what the player does" but hand-assembles the request instead of calling the tested `dj::build_random_request`, so it ignores artist cooldown, min rating, genre filters and the sonic pool — and already diverges today for any non-default key matching or rating (`cmd_library.rs:183`) | load the saved prefs and call the builder; collapse `BpmRange`/`BpmWindow` into one struct |
| 66 | `ui.rs` imports `fmt_duration` from `cmd_library.rs`, so the render layer can't compile without the CLI smoke-test harness. Every other shared formatter lives in `api/types.rs` | move it next to `display_name`/`metadata_display` |
| 67 | `worker::connect` and `worker::login` share the ping→`Connected` tail and both take an `_events` parameter neither uses | extract `establish()`, drop the dead parameters |
| 68 | `Engine::queue_add_entry` exists to carry a duration hint, but its only caller always passes `None` and the serve routes have no way to supply one — so the module header's promise that hints save a second fetch is dead in the only mode that uses the engine queue | inline it, or thread hints through the serve queue routes |
| 69 | `tokio::time::timeout` is used in three places in `quickconnect.rs` — the timeouts that keep a dead tunnel from hanging the api thread — but `time` isn't in our tokio feature list. It builds only because iroh and reqwest happen to enable it and Cargo unifies features; a dependency diet breaks the build pointing at the wrong file | add `"time"` |
| 70 | `base64` is pinned at 0.23 while every other consumer in the tree (reqwest, iroh, portmapper, tokio-websockets) is on 0.22.1, so both compile | drop our pin to `"0.22"` |
