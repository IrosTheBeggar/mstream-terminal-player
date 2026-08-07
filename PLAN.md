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
