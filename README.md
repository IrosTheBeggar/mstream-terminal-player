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

Type the address however you say it — `nas:3000`, `demo.mstream.io`, `attic.local:3000`, or a
full URL. A missing `http://` or `https://` is filled in from where the server lives: addresses
that can only be on your network (loopback, private ranges, `.local`, plain hostnames) get
`http`, anything reachable from the internet gets `https`. The completed address is written back
into the field, so you can see what was assumed.

**Files** browses folders as they sit on disk, opening wherever the server says
is most useful — straight inside your library when there is only one, or the
list to choose from when there are several. `h` from there goes up to the list
either way. **Library** browses by tags — artists, albums,
genres, and recently added — and **Search** and **Playlists** do what they say. `Enter` on a
track queues everything visible and starts there; `a` queues just that one.

Where you are and what you are hearing are drawn separately: the cursor is a
highlighted background, the playing track is in colour, and a row that is both
shows both. Lengths are flushed to the right edge so a folder can be read down
the column, and the queue's title carries how long it runs as well as how many
tracks are in it. A pane still waiting on the server says so with a spinner
rather than claiming to be empty. The terminal's title bar follows the track,
which is the only part of the player still visible when it is behind another
window.

| Key | |
|---|---|
| `j` `k` / `↓` `↑` | move · `g` / `G` first / last |
| `Ctrl+d` / `Ctrl+u` | half a screen down / up · `PgDn` `PgUp` a whole one |
| `Enter` / `l` | open a folder, or play from the highlighted track |
| `h` | go back |
| `a` | add the highlighted track to the queue |
| `Tab` | switch between browser and queue |
| `1` … `5` | Files / Library / Playlists / Search / Discover · `/` search |
| `Space` | play or pause · `n` / `p` next / previous |
| `i` | jump to whatever is playing |
| `[` `]` | seek 5s · `{` `}` seek a minute · `-` `+` volume |
| `d` / `C` | remove from queue / clear queue |
| `r` / `s` | repeat / shuffle · `A` auto-dj · `D` auto-dj panel |
| `J` | sonic journey to the highlighted track |
| `?` | help · `q` quit |

`?` shows the same list in the app. It is generated from the key table
itself, so it cannot fall out of step with what the keys actually do.

### Rebinding

```bash
mstream-player keys
```

prints every binding as a `[keys]` section you can paste into `config.toml`
and edit:

```toml
[keys]
next-track = ["b"]              # move it
jump-to-playing = ["ctrl+o"]    # ctrl+x, ctrl-x and ^x all work
add-to-queue = ["a", "insert"]  # give it several
clear-queue = []                # or take it away
```

Naming an action **replaces** its keys; anything you don't name keeps its
default. A key you claim is taken off whatever held it before, so moving one
binding never means unbinding another by hand first. A line the player can't
read costs you that line and nothing else — it says what was wrong on the
first screen and carries on. `Ctrl+C` always quits, whatever the file says.

Overlays like the Auto-DJ panel keep their own keys, since they draw their
own hints along the bottom.

## Library commands

```
# log in once (password via env or stdin — only the token is stored)
echo "$PASSWORD" | mstream-player login --server http://localhost:3000 --user alice --password-stdin

mstream-player info                          # server capabilities + session
mstream-player ls                            # start wherever makes sense
mstream-player ls ""                         # the list of libraries
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
`--server <url>`. `--server` accepts the same shorthand as the connect screen.

## Auto-DJ

Press `A` to cycle Auto-DJ: **off → similar → tempo+key**. When the queue has
nothing left after the track that's playing, it quietly adds one more.
Press `D` for the panel, where the rest of it lives.

- **similar** uses the server's audio-embedding index (`discovery`), so picks
  sound like what you're listening to rather than merely sharing a tag.
- **tempo+key** matches harmonically: the seed's key is converted to its
  Camelot code and matched against its wheel neighbours and relative
  major/minor, with tempo windows at the same, half and double time.

**similar** only appears where the server has the index — `A` cycles straight
past it otherwise, and a remembered `similar` setting is dropped (with a note)
when you connect somewhere that lacks one. If the index is there but the track
hasn't been analysed yet, it falls back to tempo+key and says so instead of
going quiet. `mstream-player info` lists what a given server has enabled.

### The panel (`D`)

| Setting | |
|---|---|
| **Sonic pool** | only pick tracks that *sound* like the session, as a tightness slider. Shows the raw cosine it maps to, and how many tracks are inside it. |
| **Anchor** | `current` follows each track; `session` averages the recent picks, so a set drifts as a whole instead of walking away one song at a time. |
| **Tempo window** | how far either side of the seed's BPM, at the same, half and double time. The server widens to twice this before giving up on tempo. |
| **Key matching** | `compatible` is the Camelot neighbourhood; `strict` never leaves the seed's key. |
| **Rating floor** · **Artist cooldown** | skip anything below a rating; keep the last N artists out. |
| **Genres** | whitelist or blacklist. A whitelist also excludes untagged tracks — "only these" is the stricter promise. |

`↑↓` choose · `←→` adjust · `p` samples three picks so you can hear what a
setting does before committing to it · `Esc` closes. Settings persist.

The sonic pool is a **hard** constraint: tempo, key and artist all relax
inside it, but it never widens. If nothing is similar enough the player says
so and loosens for that one pick rather than quietly playing something else.
Rows that need the server's embedding index only appear where it exists.

## Discover

Press `5` with a track playing or highlighted, and the Discover tab opens
anchored on it:

```
┌ Discover · from Bassnectar - Rewind The Track ───────────┐
│> Similar tracks    in your library                       │
│  Similar artists   like Bassnectar                       │
└──────────────────────────────────────────────────────────┘
```

**Similar tracks** are ordinary rows — `Enter` plays from there, `a` queues
one, exactly as everywhere else. **Similar artists** shows how close each one
is, how many ways in it has, and what the model thinks it sounds like:

```
┌ Artists like Bassnectar ─────────────────────────────────┐
│> Barely Alive   0.96  2 ways in · Dubstep, Electro House │
│  ill Gates      0.95  2 ways in · Dubstep, Drum n Bass   │
│  NiT GriT       0.95  2 ways in · Dubstep, Glitch        │
```

`Enter` opens an artist's ways in — the two of their tracks closest to *what
you were already listening to*, rather than whatever they're best known for.

The tab only appears on servers that have the discovery index.

## Sonic Journey

Highlight a track and press `J`: the player plots a route to it from whatever
is playing, through tracks that sit between the two in the server's embedding
space. With nothing playing it takes two presses — one to mark where to set
off from, one to say where to end up.

```
┌ Sonic Journey ──────────────────────────────────┐
│  From    Boukmanflow - Bout2skate               │
│  To      Boukmanflow - The Hangout              │
│  Stops   14   ←→ to change                      │
│                                                 │
│   1.   0%  Boukmanflow - Bout2skate             │
│   2.   8%  Boukmanflow - NewRapCity             │
│   …                                             │
│   7.  46%  ALM - Goodbye 78                     │
│   …                                             │
│  14. 100%  Boukmanflow - The Hangout            │
└─────────────────────────────────────────────────┘
```

The percentage is how far along the arc each stop sits. Changing the length
re-plots rather than trimming — a shorter journey passes through different
places. `Enter` makes it the queue and starts it; `Esc` walks away.

Needs the server's discovery index; without it the key says so instead.

To see what Auto-DJ would pick without launching the player:

```
mstream-player dj "music/Artist/Album/01.flac" --mode similar
mstream-player dj "music/Artist/Album/01.flac" --mode tempo
```

## Where it keeps things

Two files, in `%APPDATA%\mstream-player` (Windows), `~/Library/Application Support/mstream-player`
(macOS) or `~/.config/mstream-player` (Linux). Set `MSTREAM_PLAYER_CONFIG_DIR` to put them
somewhere else — a portable install can keep them beside the binary.

- `config.toml` — servers, preferences and key bindings. Readable and editable; fine to keep in a
  dotfiles repo.
- `credentials.toml` — access tokens and Quick Connect pairing codes. Owner-only on unix, and the
  one to leave out of any sync.

A server reached with Quick Connect is remembered as `mstream+iroh://<endpoint-id>` rather than by
address — the tunnel gets a fresh local port every run, so the port is no use as a name. Its
pairing code is kept in `credentials.toml`, which is what lets the player re-dial the tunnel on
its own at the next launch. Signing out keeps the code: getting a new one takes admin access over
a connection you may not have from wherever you are.

Both are written by rename, so an interrupted write leaves the previous file intact.

While a track plays it is spooled to one scratch file (that's what makes seeking instant), which
goes in the platform cache directory — `%LOCALAPPDATA%\mstream-player\spool`,
`~/Library/Caches/mstream-player/spool` or `~/.cache/mstream-player/spool` — rather than `/tmp`,
which is RAM-backed on many Linux systems. Only the playing track is spooled; the file is deleted
when it stops, and leftovers from a crash are swept at the next start. To put it elsewhere, set
`MSTREAM_PLAYER_CACHE_DIR` or add to config.toml:

```toml
[cache]
dir = "/mnt/scratch"   # spool files land in <dir>/spool
```

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
