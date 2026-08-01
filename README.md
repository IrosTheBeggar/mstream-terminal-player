# mstream-player

Terminal player and headless audio engine for [mStream](https://github.com/IrosTheBeggar/mStream).

**Status: Phase 4** — the interactive player works: browse your libraries, queue tracks, and
play them with seeking, all from the terminal. See [PLAN.md](PLAN.md) for the roadmap and the
control-API contract.

## The player

```
mstream-player
```

Launches the TUI. It reconnects from a saved session, or shows a connect screen (leave the
username empty for a server in public mode).

**Files** browses folders as they sit on disk. **Library** browses by tags — artists, albums,
genres, and recently added — and **Search** and **Playlists** do what they say. `Enter` on a
track queues everything visible and starts there; `a` queues just that one.

| Key | |
|---|---|
| `j` `k` / `↓` `↑` | move · `g` / `G` first / last |
| `Enter` / `l` | open a folder, or play from the highlighted track |
| `h` | go back |
| `a` | add the highlighted track to the queue |
| `Tab` | switch between browser and queue |
| `1` `2` `3` `4` | Files / Library / Playlists / Search · `/` search |
| `Space` | play or pause · `n` / `p` next / previous |
| `[` `]` | seek 5s · `-` `+` volume |
| `d` / `C` | remove from queue / clear queue |
| `r` / `s` | repeat / shuffle |
| `?` | help · `q` quit |

## Library commands

```
# log in once (password via env or stdin — only the token is stored)
echo "$PASSWORD" | mstream-player login --server http://localhost:3000 --user alice --password-stdin

mstream-player info                          # server capabilities + session
mstream-player ls "music/Artist/Album"       # browse folders
mstream-player browse                        # artists · --albums · --genres
mstream-player browse "Artist" "Album"       # an artist's albums, then its tracks
mstream-player browse --genre Ambient        # tracks in a genre
mstream-player browse --recent --limit 25    # recently added
mstream-player search "moon"
mstream-player playlists [name]
mstream-player logout
```

Servers running in public mode (no users configured) need no login — just pass
`--server <url>`.

## Playback

```
mstream-player play "music/Artist/Album/01.flac" --seek-to 45   # via the saved session
mstream-player play "music/Artist/Album/01.flac" --transcode mp3 --bitrate 192k
mstream-player play --url C:\music\song.mp3                     # local file or raw URL
```

`play` is a one-shot smoke test with PASS/FAIL exit codes, useful for checking that a server,
codec, or seek path actually works. Transcoding accepts `mp3` or `aac`; the server's default
codec is often opus, which this player cannot decode, so a codec is always requested explicitly.

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
