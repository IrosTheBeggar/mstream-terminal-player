# mstream-player

Terminal player and headless audio engine for [mStream](https://github.com/IrosTheBeggar/mStream).

<p align="center">
  <img src="docs/player.svg" alt="mstream-player: the Now Playing screen, with the Cover visualizer lighting the album art to the music" width="920">
</p>

*A real frame, not a mockup: the Cover visualizer plays the spectrum across the album art, drawn
in half-blocks at thirty frames a second. Captured with the built-in replay harness
(`mstream-player replay "…,html" --live`).*

**Status: Phase 4** — the interactive player works: browse your libraries, queue tracks, and
play them with seeking, all from the terminal. See [PLAN.md](PLAN.md) for the roadmap and the
control-API contract.

## Install

Homebrew (macOS, or Homebrew on Linux):

```bash
brew install irosthebeggar/mstream/mstream-player
```

Scoop (Windows):

```powershell
scoop bucket add mstream https://github.com/IrosTheBeggar/scoop-mstream
scoop install mstream-player
```

Or one line with no package manager. macOS / Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/IrosTheBeggar/mstream-terminal-player/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/IrosTheBeggar/mstream-terminal-player/main/install.ps1 | iex
```

Both fetch the binary for your machine from the latest release, verify its sha256 against the
release's `manifest.json`, and install it as `mstream-player` — into `~/.local/bin` on unix, or
`%LOCALAPPDATA%\Programs\mstream-player` (added to your user PATH) on Windows. Pin a version with
`MSTREAM_PLAYER_VERSION=v0.1.0`; change the destination with `MSTREAM_PLAYER_INSTALL_DIR`.

Or fetch a binary yourself from
[Releases](https://github.com/IrosTheBeggar/mstream-terminal-player/releases) — every release
carries a `manifest.json` with the sha256 of each file:

| Platform | Asset |
|---|---|
| Windows x64 | `mstream-player-win32-x64.exe` |
| macOS Intel / Apple Silicon | `mstream-player-darwin-x64` / `mstream-player-darwin-arm64` |
| Linux x64 / arm64 / armv7 | `mstream-player-linux-x64` / `…-arm64` / `…-arm` |
| Debian / Ubuntu / Pi OS | `mstream-player_<version>_{amd64,arm64,armhf}.deb` |
| Fedora / RHEL | `mstream-player-<version>.{x86_64,aarch64}.rpm` |

The `.deb` and `.rpm` declare the ALSA dependency, so `apt install ./mstream-player_*.deb` or
`dnf install ./mstream-player-*.rpm` brings everything along. The bare Linux binaries run on
glibc 2.31 or newer — Debian 11, Ubuntu 20.04, and the Pi and NAS images of that era — and need
ALSA at runtime (`sudo apt install libasound2`). macOS quarantines binaries downloaded in a
browser; the one-liner doesn't trip that, and the macOS binaries are signed and notarized. To
build from source instead:
`cargo install --git https://github.com/IrosTheBeggar/mstream-terminal-player`.

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

A pasted Quick Connect pairing code (`mstr1:…`) dials the server over its Iroh tunnel instead of
an address — the way in from networks that can't reach the server directly. It holds up on
corporate networks: the relay connection trusts the operating system's certificate store, so a
TLS-inspecting proxy (Netskope, Zscaler) passes once its CA is installed where everything else on
the machine already trusts it, and `HTTPS_PROXY` is honoured when the network insists on one. If
a dial still times out, `mstream-player quickconnect-probe '<code>'` narrates each stage and
names the relay it reached — and when the network is the problem, the concrete thing to ask IT
to allow is `*.relay.n0.iroh.link` on TCP 443.

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
| `1` … `6` | Files / Library / Playlists / Search / Discover / Settings · `/` search |
| `0` | full-screen now playing |
| `Space` | play or pause · `n` / `p` next / previous |
| `i` | jump to whatever is playing · `0` full-screen now playing |
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

### Colours

The player varies three colours: an accent for what's playing and what's
selected, a dim for labels and rules, and one for folders. By default they're
palette *names* — `cyan`, `dark-gray`, `blue` — so your terminal's own scheme
picks the hues and the player looks like the rest of your terminal rather than
imposing a look on it.

To pin them instead:

```toml
[theme]
accent = "#4fc3d6"     # exact, and needs a truecolor terminal
dim = "bright-black"   # or a palette name
folder = "33"          # or an index into the 256-colour cube
```

A value it can't read costs you that colour and nothing else — it says what was
wrong on the first screen and keeps the default. Errors stay red and aren't
configurable; an error that isn't red is a trap.

### Crossfade

```toml
[player]
crossfade_seconds = 6
```

Each track blends into the next as it ends — equal-power, with the coming track
opened ahead of time so the blend never waits on the network. Off by default,
and it only fires when a track ends *on its own*: `n` stays a cut (a short soft
one — nothing in this player clicks), a seek keeps the track you're on and drops
the one leaving, and pausing mid-blend freezes it exactly where it sits. Tracks
the server can't state a length for (a live transcode on its first play) change
over the plain way, since there is no known ending to fade toward.

Seeking toward the end plays fair with the blend: a forward seek stops just
short of the transition's own runway — the fade window plus a couple of
seconds for the open — so skipping to a track's last stretch still ends in a
crossfade instead of starving it (with no transition configured, seeking past
the end keeps its old skip-the-track meaning). Seek keys also chain: quick
presses of `}` add up a minute each rather than re-reading a stale position.

```toml
[player]
gapless = true
```

Gapless is **on by default**: with no crossfade set, the next track is fed
into the playing sink ahead of time and the boundary is crossed sample-tight —
no gap, no fade, exactly as the album was cut. The price is one track of
prefetch near each boundary; on a metered connection, turn it off. A
configured crossfade outranks it either way.

Everything lives in the **Settings** tab (`6`): Enter opens Crossfade, `←` `→`
walk the blend length, and Enter toggles the rest — **Gapless**, **Blend
skips** (a manual skip crosses in a second instead of cutting), and **Pause
fade** (pause and resume ride a short ramp instead of landing mid-note). What
you set there is what config.toml remembers. The tab's other room is
**Logs** — see Diagnostics below. The jukebox has the first pair as
`mstream-player serve --crossfade 6` / `--gapless`; the legacy spawn contract
keeps all of it off.

## Mouse

Clicking the progress bar seeks there. The bar lights up under the pointer
and marks the column a click would land on — a terminal will not change the
mouse cursor for us, so the affordance is on this side of the glass.
Everything else is keys.

Asking the terminal to report the mouse means it stops doing its own
click-drag selection — hold shift to select text as before. If you copy off
the screen more often than you scroll, turn it off:

```toml
[mouse]
enabled = false
```

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

## Now playing

`0` gives the whole terminal over to what's playing:

```
┌ Now Playing ───────────────────────────────────────────────────┐
│                                                                │
│              Bassnectar                                        │
│              Rewind The Track                                  │
│              Divergent Spectrum · 2011                         │
│              3:29 · 174 BPM · 8A (A minor) · 7/10 · 23 plays   │
│                                                                │
│              ▶ playing                                         │
│                                                                │
│████████████████████████▉          1:11 / 3:29                  │
│0 back                                       vol 100%  dj similar│
└────────────────────────────────────────────────────────────────┘
```

It's a view rather than a dialog, so every key still means what it means
everywhere else — `Space`, `n`, the seek keys and the volume keys all keep
working while you're looking at it, and `0` goes back. The key is the same
Camelot code the Auto-DJ panel matches on, with the tag's own spelling beside
it. Tags a track doesn't have are left out rather than shown empty.

## Now Playing

`0` gives the whole terminal to what's playing: the tags down the left as a
labelled column, a full-width transport along the foot, and a tabbed panel on
the right.

```
┌ Now Playing ────────────────────────────────────────────────────────────┐
│                              │ Queue  Lyrics  Discover  Auto-DJ  Visual… │
│ Rewind The Track             │ ──────────────────────────────────────────│
│ Bassnectar                   │ ▶ Bassnectar - Rewind The Track      3:29 │
│                              │   Bassnectar - Mic Check             4:02 │
│ Album   Divergent Spectrum   │   Bassnectar - Elastic               3:02 │
│ Year    2011                 │                                           │
│ Tempo   174 BPM              │                                           │
│ Key     8A  A minor          │                                           │
│──────────────────────────────┴───────────────────────────────────────────│
│████████████████████▍               1:11 / 3:29                           │
│←→ tab   ↑↓ list   Enter play   d remove   0 back        vol 100%  dj sim… │
└──────────────────────────────────────────────────────────────────────────┘
```

`←` `→` move between tabs, `↑` `↓` move within one. It's a view rather than a
dialog, so everything else keeps working — `Space`, `n`, `p`, seeking and
volume all behave as they do outside, including wherever `[keys]` moved them.
`Esc` or `0` leaves.

**Queue** shares the browser screen's selection, so `Enter` plays and `d`
removes the row under the cursor either way. **Auto-DJ** reports what it is set
to without offering to change it — `D` stays the one place that happens.
**Lyrics** appears only for a track the server has words for, so the strip
changes shape as the queue moves. **Discover** needs the server's index. Where
the strip won't fit, it shows the tab you're on between arrows rather than
truncating the last name.

Lyrics, Discover and the visualizer are stubs for now; the layout is what's
real.

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
which is RAM-backed on many Linux systems. Only the playing track is spooled — plus the next one
while a crossfade is being prepared, and for a few seconds the file of a preparation a queue edit
abandoned; each is deleted when its track stops or its download lets go, and leftovers from a
crash are swept at the next start. To put it elsewhere, set
`MSTREAM_PLAYER_CACHE_DIR` or add to config.toml:

```toml
[cache]
dir = "/mnt/scratch"   # spool files land in <dir>/spool
```

### Diagnostics

The Settings tab has a **Logs** room. The player is always capturing a session into a bounded
in-memory ring — the last couple of thousand lines, held for as long as it runs and gone when it
quits — so **View log** opens a real session at any time, written or not (`j`/`k` scroll, `G`
follows the end as new lines arrive, `q` closes). That is usually all a "what just happened?"
needs, and it leaves nothing behind on disk.

**Write log** (off by default) decides whether the same lines are also kept in a file. Turn it on
and the file opens *carrying what has already been captured*, so the thing you just watched
happen is in it — no restart, no environment variable. **Log level** (info by default) is how
much is captured at all: info is the player narrating its own decisions (what it played, seeked,
prepared, handed over, and what failed), while debug and trace add what iroh, the HTTP stack and
the spool have to say. Both switches are remembered in config.toml.

Underneath sit two switches, off by default, each writing to a file and never to the screen —
the TUI deliberately silences stderr while it draws, so files are how a session explains itself
afterwards.

- `MSTREAM_LOG=1` captures what the *dependencies* narrate — iroh's relay connects, holepunch
  attempts and path changes, HTTP retries — into `<cache>/logs/mstream-player.log`, keeping the
  last few runs beside it as `.1` through `.4`. `MSTREAM_LOG=/path/to/file` writes exactly there
  instead. `RUST_LOG` chooses how much is said (the usual tracing filter grammar; unset means
  `info`). The player prints `logging to …` at start and again at quit.
- `MSTREAM_ENGINE_TRACE=/path/to/file` is the player's own flight recorder: one timestamped line
  per transition decision — plays, announcements, seeks and their clamps, prepares, open failures
  with reasons, retries, handovers — plus every diagnostic the TUI kept off the screen. The file
  is truncated at each start, so it is always exactly one run.

The first is for "what did the network do"; the second for "what did the player decide". A bug
report with both attached usually answers itself.

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
- `--crossfade <seconds>`: blend each track into the next when one ends on its own (equal-power,
  prepared ahead so the blend never waits on the network). 0 — the default — keeps the original
  hard cut, and manual `/next` cuts either way. Needs track durations to find the fade point, so
  sources of unknown length (a live transcode) fall back to the plain cut
- `--gapless`: with no crossfade set, cross track boundaries sample-tight by feeding the next
  track into the playing sink ahead of time
- Soft cuts everywhere, flags or no flags: manual next fades out over 150 ms, stop over 80 ms,
  and seeks dip around the jump — where the original engine cut mid-waveform and clicked
- With a transition configured **and a next track queued**, `POST /seek` lands forward seeks no
  closer to the end than the transition needs (the fade window plus ~2s for the open); on the
  queue's last track, seeking past the end still ends the track, as it always did
- Bug fixes: volume persists across track changes, manual next is no longer trapped by
  loop-one, no panic when no audio device exists, removing a queue entry while stopped no
  longer starts playback

Requests must arrive under the address the server is bound to, and anything carrying a body
must say `Content-Type: application/json`. The routes and their JSON are unchanged; this is
about who is allowed to reach them. Without it, any page you happen to be visiting can drive
the jukebox — a `fetch` with a string body needs no preflight, so `/play`, `/stop` and
`/queue/*` are all reachable from a browser tab, and `/play` will open a URL of the caller's
choosing. A `POST` carrying an `Origin` header is refused outright for the same reason.

Addresses and `localhost` always pass; any other name has to be the one the server was bound
under, so if you reach a `--host 0.0.0.0` jukebox as `jukebox.local`, bind it under that name
instead. (A name is the only thing an attacker can point at your machine — an address can't be
rebound.) The other habit this breaks is `curl -d`, which sends form-encoded by default: pass
`-H 'Content-Type: application/json'`. Bodies are capped at 64 KB.

```bash
curl -sX POST -H 'Content-Type: application/json' -d '{"file":"/music/a.flac"}' http://127.0.0.1:3333/play
```

## Build

```
cargo build --release
```

Building on Linux needs ALSA headers (`libasound2-dev`); running needs `libasound`.

## License

GPL-3.0 — see [LICENSE](LICENSE). The engine originated in mStream as `rust-server-audio`.
