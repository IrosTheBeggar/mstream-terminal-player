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

### Phase 2 — HTTP streaming source (the risk phase, done early)
`Source::Local | Source::Http` via `stream-download` + reqwest (buffered `Read + Seek` over range
requests) into the same rodio decoder. Duration hint accepted from callers (remote probing costs a
fetch; the TUI knows durations from the mStream API). Throwaway smoke command:
`mstream-player play --server <url> --token <jwt> <vpath>`.
**Done when:** FLAC and MP3 stream *and seek* from a live mStream server on Windows, and
`/transcode` seek behavior is characterized (fallback: seek disabled on transcodes in v1).

### Phase 3 — mStream API client
`src/api/`: login + token cache (OS config dir), `/api/v1/ping` bootstrap, file-explorer, DB
browse (artists/albums/genres/search/random), playlists, stream + transcode URL builders.
Types hand-written against mStream `docs/openapi.yaml`.

### Phase 4 — TUI
ratatui + crossterm. v1 screens: connect/login, **file explorer** (the mStream-identity feature),
playlists, search, queue pane, transport bar with position ticker. Vim keys + arrows, help overlay,
TOML config with multiple saved servers. `PlayerCtl` trait in front of the engine so a different
backend (e.g. mpv) could swap in if engine gaps ever bite.

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
