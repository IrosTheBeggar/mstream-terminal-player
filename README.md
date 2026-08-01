# mstream-player

Terminal player and headless audio engine for [mStream](https://github.com/IrosTheBeggar/mStream).

**Status: Phase 1** — the playback engine (ported from mStream's `rust-server-audio`) with its
JSON control API. The interactive TUI lands in Phase 4. See [PLAN.md](PLAN.md) for the roadmap
and the control-API contract.

## Serve mode (jukebox engine)

```
mstream-player serve --port 3333
```

Drop-in successor to mStream's `rust-server-audio` — `mstream-player --port N` is accepted as a
legacy alias for the old spawn contract. Changes from the original engine:

- Binds `127.0.0.1` by default (`--host 0.0.0.0` restores the old LAN-exposed behavior)
- Optional auth: `--auth-token <t>` or env `MSTREAM_AUDIO_TOKEN` (checked as `x-auth-token`
  header on every route except `GET /version`)
- `GET /version` → `{"name", "version", "apiVersion"}`
- `--exit-with-parent`: exit when stdin closes (pass only when the parent holds stdin open)
- Bug fixes: volume persists across track changes, manual next is no longer trapped by
  loop-one, no panic when no audio device exists, removing a queue entry while stopped no
  longer starts playback

## Build

```
cargo build --release
```

Building on Linux needs ALSA headers (`libasound2-dev`); running needs `libasound`.

## License

GPL-3.0 — see [LICENSE](LICENSE). The engine originated in mStream as `rust-server-audio`.
