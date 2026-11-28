# mstream-player

Terminal player and headless audio engine for [mStream](https://github.com/IrosTheBeggar/mStream).

<p align="center">
  <img src="docs/player.svg" alt="mstream-player: the Now Playing screen, with the Cover visualizer lighting the album art to the music" width="920">
</p>

*A real frame, not a mockup: the Cover visualizer plays the spectrum across the album art, drawn
in half-blocks at thirty frames a second.*

One codebase, two products: the **terminal player**, which runs in the terminal you already
have, and the **desktop player**, the same program with the GUI drawn in a window of its own. See
[PLAN.md](PLAN.md) for the roadmap and the control-API contract.

## Install

### Two flavours

The terminal player and the desktop player are one program built two ways, and they differ in two
places only. Started with no arguments, the desktop player opens in its own window, where the
terminal player opens in your terminal; and only the desktop player has the window at all
(`mstream-player gui --window`). Everything else — commands, keys, servers,
settings — is the same in both, and both keep their files in the same places.

### Install the desktop player

Download the package for your machine from
[Releases](https://github.com/IrosTheBeggar/mstream-terminal-player/releases).

| Platform | Package |
|---|---|
| macOS Intel / Apple Silicon | `mstream-player-desktop-darwin-x64.app.zip` / `…-arm64.app.zip` |
| Windows x64 | `mstream-player-desktop-win32-x64.zip` |
| Linux x64 | `mstream-player-desktop-linux-x64.tar.gz` |

**macOS.** Unzip and drag **mStream Player.app** to Applications. The app is signed, notarized
and stapled, so Gatekeeper accepts it.

**Windows.** Unzip it anywhere; nothing is installed. Double-click **mStream Player.exe**, or pin
it to the taskbar: it is a small launcher that starts the player beside it with no console
window. `mstream-player.exe` beside it is the player itself, for use from a terminal; the zip
also holds the icon and a `README.txt`. The build is not code-signed yet, so SmartScreen may warn
on first run: choose **More info**, then **Run anyway**. Some NVIDIA drivers (31.0.15.3640 among
them) crash when the player's path is 253 characters or longer; the player then exits rather
than hang, so if no window appears, unzip it to a shorter folder.

**Linux** (x64). The tarball unpacks to `mstream-player-desktop/`: the binary, the desktop entry
`io.mstream.player.desktop`, the icons under `icons/hicolor/` and a `README.txt`. Run it in
place with `./mstream-player`, or install it for your user so it shows in the app menu:

```bash
cd mstream-player-desktop
mkdir -p ~/.local/bin ~/.local/share/applications ~/.local/share/icons
cp mstream-player ~/.local/bin/
cp io.mstream.player.desktop ~/.local/share/applications/
cp -r icons/hicolor ~/.local/share/icons/
update-desktop-database ~/.local/share/applications
gtk-update-icon-cache ~/.local/share/icons/hicolor   # if your desktop uses it
```

The menu entry runs `mstream-player`, so `~/.local/bin` must be on your PATH; log out
and in once after the first install if it was not.

On X11 the window needs libxkbcommon-x11 (`libxkbcommon-x11-0` on Debian and Ubuntu); audio needs
ALSA, as below. There is no desktop build for arm Linux; the terminal player runs there.

The same release carries the bare desktop binaries, named `-raw` so they are not mistaken
for the packages — `mstream-player-desktop-darwin-arm64-raw`, `…-darwin-x64-raw`,
`…-linux-x64-raw` and `…-win32-x64-raw.exe` — for scripts and bundles. A browser download
leaves them without the execute bit; the packages above are the way in.

Started with no arguments (a double-click, the Dock, the app menu), the desktop player opens the
GUI in its own window. With no display, or over SSH, it falls back to the terminal UI. Every
command works as it does in the terminal player, `mstream-player gui --window` opens the window
explicitly, `setup --window` and `qr --window` open the setup wizard and the Quick Connect page in
windows of their own (beside an open player, and 0 every way they close), and exit code 3 means no
window could open. `mstream-player --version` prints a second line, `features: window window-pages`
(`window` says the player opens a window; `window-pages` that the setup wizard and Quick Connect do
too), so a script can tell the two flavours apart. It keeps its config, credentials and cache in the same places as the
terminal player — see [Where it keeps things](#where-it-keeps-things).

To build it from source (or use `--features window` for `gui --window` without the window on an
empty command line):

```bash
cargo install --locked --git https://github.com/IrosTheBeggar/mstream-terminal-player --features desktop
```

### Install the terminal player

This is what Homebrew, Scoop, the one-line installers, the `.deb` and `.rpm`, the unsuffixed
release binaries and `cargo install` deliver. It has no window of its own: `gui --window`,
`setup --window` and `qr --window` are usage errors there.

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
`cargo install --locked --git https://github.com/IrosTheBeggar/mstream-terminal-player`. Add
`--features desktop` to build the window into it — the desktop player above. `--locked` builds
the dependency versions the repository's `Cargo.lock` names, the ones CI tested; without it cargo
takes the newest of each, and one dependency's breaking minor release is enough to stop the build.

## The player

```
mstream-player
```

Launches the player: in your terminal with the terminal player, in its own window with the
desktop one. [The GUI player](#the-gui-player) below says what is in it. It reconnects from a
saved session, and the header's dropdown adds a server.

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

### Colours

Every full-screen page — the player, the setup wizard, the admin rooms — keeps
its palette even when `NO_COLOR` is set in the environment. That convention is
for programs that decorate their output, and the plain commands (`ls`,
`search`, `info`, …) honour it; a full-screen page's palette is the interface,
and without it the wizard reads as broken. Tool shells and some IDE terminals
export `NO_COLOR=1` without saying so — that, not the console, is why a page
used to come up white on black in a Command Prompt opened from one.

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
the end keeps its old skip-the-track meaning).

```toml
[player]
gapless = true
```

Gapless is **on by default**: with no crossfade set, the next track is fed
into the playing sink ahead of time and the boundary is crossed sample-tight —
no gap, no fade, exactly as the album was cut. The price is one track of
prefetch near each boundary; on a metered connection, turn it off. A
configured crossfade outranks it either way.

Everything lives in the **Settings** room: the blend length, **Gapless**,
**Blend skips** (a manual skip crosses in a second instead of cutting), and
**Pause fade** (pause and resume ride a short ramp instead of landing
mid-note). What you set there is what config.toml remembers. The jukebox has
the first pair as `mstream-player serve --crossfade 6` / `--gapless`; the
legacy spawn contract keeps all of it off.

### Output devices

Playback follows the machine. Plug in headphones, pair a Bluetooth speaker,
and the player moves to the new default within a second or so, the track
going on from the sample it had reached — paused stays paused, the volume
stays yours, a blend goes on blending. Nothing is fetched again for a move,
and nothing waits for one: the old device plays on until the new one has
opened, and the controls answer throughout. A default that is not ready
yet (a headset still connecting) is tried again for half a minute. Pull the
device it was playing on and it hops to whatever the system falls back to;
when there is nothing to fall back to (the only headset switched off), it
holds the moment and resumes as soon as any output returns. A device that
is still there and has stopped taking sound is opened afresh, and if that
does not start it the player says the audio is lost and waits — it is said
to be back only once sound is flowing again. Each move is one line in the
message bar, or on stderr in `serve` mode.

### When the server goes quiet

A track that is streaming can run out of sound before it runs out of track:
the server stops answering, the Wi-Fi drops. The track waits where it stood
— its position does not move for silence — and the rest of the player goes
on as if nothing had happened. The controls answer, another track plays the
moment you pick one, and a seek back into the part that has already arrived
plays it at once. A seek ahead of the download lands when the sound does,
and a track you leave lets go of its download.

## The progress bar

On a server that can draw them, Now Playing's bar *is* the track — its shape
under the playhead, so a breakdown or a quiet passage is something you can aim at
rather than hunt for:

```
▁▁▁▁▁▁▁▃▇▇██▇▆▇▆▅█▇▇▇█████▇▇▇█▇▆█▇▇▆▇▆▅▇▇▇▇▇▄▅▇▇██▇▆▇█▇▆▇▆▆▆█▇███▆▇█▆▇▇██▇▁▁  0:40 / 4:23
```

Mirrored about the line through the middle, which is what makes it read as a
recording rather than as a bar chart. The whole band is the scrubber — click
either row.

The colours are the same three channels as the plain bar — accent behind the
playhead, dim ahead of it, and a marker where a click would land — so it is
still the same control, just wearing the song.

Two things it deliberately isn't. It shows **energy**, not peaks: the server
already sends peaks, and taking the peak again over the three seconds a
column covers answers "was anything loud here", which on a modern master is
yes from end to end. And it shows a track's **shape, not its level** —
scaled to the band that track actually uses, because nothing here applies
ReplayGain and a quietly-mastered record would otherwise be a flat line. Two
tracks side by side say nothing about which is louder.

### Glyphs, and the Windows console

A font either has a character or it prints a box, and the Windows 10 default
console — conhost with Consolas — stops at the old CP437 repertoire. Of what
this player draws, Consolas is missing six of the eight eighth-height blocks,
`▏`, `▶`, `◆`, `⏸` and every braille pattern; it keeps `█ ░ ▒ ▓ ▄ ─ │`.

So the player picks a glyph set at startup. On a terminal that can take them
it draws the shapes above. On a legacy console it draws the waveform as
*density* rather than height — `░▒▓█`, one row, no mirror — which is coarser
but renders everywhere:

```
░░░░░░░░▓██▓▓▓▓▒▓▓▓▓▓████▓▓██▓▓▓█▓▒▓▓▒▓▓▓▓▓▒▒▓█▓█▓▓█▓▓█▓▒▒▓▓███▓▓▓█▒▓░░  0:30 / 4:23
```

It reads `WT_SESSION` — Windows Terminal sets it, conhost does not — and
assumes anything that isn't Windows is fine.

The visualizer needs no fallback: it draws in half blocks (`▀ ▄ █`), which
every console font has. The real fix for the rest is a terminal whose font
has more than CP437 in it: run the player from Windows Terminal.

It needs `ffmpeg` on the server. Without it — or for a track the scan hasn't
reached, or one living on a federated peer — the bar draws as it always did.
The player asks once, believes the answer, and stops asking for the session.
It also asks for the *next* track's shape while the current one plays, since
generating one the server hasn't seen costs an ffmpeg decode and a shape
that arrives thirty seconds in is a bar that changes under you.

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
`--server <url>`. `--server` accepts the same shorthand as the player's own server form.

## The GUI player

`mstream-player gui`, or `mstream-player` alone, opens the player — the
mouse-first surface the installers launch: everything is clickable and every
action keeps a key. In the terminal player it runs in your terminal and wants
at least 100×24 cells; smaller, it becomes a mini player — the cover, what
is playing, a seek line, and prev · play · next — until there is room again.
In the desktop player, started with no arguments or as `gui --window`, it
opens in its own window instead (see [Two flavours](#two-flavours)).

- **Five screens**: **Library** (the nav column and its rooms), **Stats**
  (`T`), **Admin** (`M`) and **MP3 Player** (`K`) are the top bar's tabs;
  **Now Playing** (`0`, no tab of its own) is a full-screen view — the
  facts, the cover, the tabbed panel and the waveform band — with
  prev · play · next under the cover; Stats is the listening log
  of `mstream-player stats` on the session's server, whole: the same tabs,
  periods and keys, Esc back to the Library. Admin is the admin panel below,
  its six rooms in a hallway at the left, with the server's own log beside
  them. MP3 Player is the page of `mstream-player device flash`
  ([below](#mp3-player-m5stack-core2)) with no flags, and needs no server:
  one card about the Core2 plugged in, read over USB without restarting it —
  whether its firmware is up to date, and what its SD card holds. A write
  passes a warning first and, once started, cannot be left — the tab holds
  every key but Ctrl+C until it is done, and its footer, drawn on this tab
  whatever the key-hints setting, warns against unplugging the board.
  Beside them, the
  **Visualizer** item (`V`) opens the mobile app's shader presets in a
  window of their own, moving to what is playing. Move the pointer over it
  for its controls: `‹` `›` and a dropdown of every preset, fullscreen,
  and the mobile app's tuning panel — the response curve every preset
  hears, and the knobs the preset in front declares — kept in
  `[visualizer]` in `config.toml` and the window reopens where you left
  it. The keys work too: `←` `→` presets, `t` tuning, `f` fullscreen, Esc
  closes the innermost thing and then the window.
- **Rooms**: Files, Search, then Albums (a wall of covers), Artists, Genres,
  Recent, Rated, Playlists, Last played and Most played, then the tools —
  Auto DJ, Sonic path, Discover, Add torrent and Add YouTube — and
  Settings. Digits `1`–`9`, `D` and `G` reach them; the
  browse bar above a listing filters it and plays, queues or shuffles the
  whole list. A row's hover verbs play it now, add it next, queue it, or
  open its sheet (rating, add to playlist, song info); a right click opens
  the sheet too.
- **The queue panel** at the right (Tab shows or hides it): rows with
  covers, a hover `[x]`, a grip to drag rows, `clear` in its header. Rows
  carry the server they came from, so a queue can span servers.
- **Servers**: the header's dropdown switches between saved servers and adds
  one — a standard address or a Quick Connect pairing code; Settings ›
  Manage servers edits, pairs a phone by QR, makes a default, removes.
  Federated peers a server lists are browsable (read-only).
- **Play reporting** is on: what you play is posted to your server's play
  history, so Last played and Most played show your own
  listening.
- **Add from YouTube**, on your own server where your account can add
  files: the nav's Add YouTube room takes a pasted link (in the player's
  own window a `Paste` button reads the clipboard) and shows the song it
  makes as a card — the video's cover, its tags tidied, every change
  undoable, `edit tags` a sheet of text boxes — with where it saves and
  as what on a row each, and one button that asks the server to add it;
  on a folder in Files, the browse bar's `add from YouTube` (or `y`) adds
  it right there. The list under the form follows each add to the end and
  keeps what came in. (The cover is YouTube's own thumbnail for the video,
  which the player asks YouTube's image host for itself.)
- **Add torrent** hands the server a `.torrent` or a magnet link: choose
  the file, or paste the link — or the path of a file dropped on the
  terminal — and the room shows what it holds (how many files, how big),
  the artist, album and year it read from the name, and where it will
  land. `Metadata` and `Destination` each open a sheet; `Add torrent` is
  under them. What you sent stays listed under the form, and a refusal
  stays on the page in the server's own words.
- **Settings**: crossfade, gapless, resume the queue at launch, torrents,
  and **keyboard hints** (off by default — the footer names the keys when
  it is on). The GUI speaks the ten locales the wizard does, from your
  system language.

Launch options: `--server`, `--token`, `--bundled-server` (the installer's
server, never removable), `--torrent` (open with a `.torrent` or a magnet).
`MSTREAM_GUI_DEMO=1` seats a fixed track so the bar can be seen without a
server; `MSTREAM_NO_OPEN=1` keeps hand-offs from popping other apps. The
pty scenarios under `smoke/gui/` drive it against a local two-server rig.

## Admin panel

```
mstream-player admin                         # manage the saved session's server
mstream-player admin --server http://nas:3000
mstream-player admin --same-machine          # this terminal runs on the server: add folders with the OS dialog
```

Six rooms. **Libraries** (the default) is the server's music folders — each folder's name,
its path as the server sees it, and whether the scanner follows symlinks (`s` flips it, from
the next scan on). `b` browses the server's own disk; with `--same-machine` it opens the OS
folder dialog instead, the same picker the setup wizard uses. `t` types a path, with
completion from the server; `~` is the server user's home, as it is for `b`. Every way of
adding ends at the naming step, because the name is
the library's vpath — its address in every link, playlist and user grant — and the server
cannot rename one, so it is asked for once. `r` removes behind a warning; the files never move.

The GUI player hosts the same six rooms in its **Admin** tab (`M`), on the session it already
holds, beside a live tail of the server's log — a column in a wide window, a band under the
room in a tall one, otherwise a room of its own (`L` moves it). Drag across the log's lines to
highlight them; `y` copies them (or every line shown), `d` saves the server's log files to
your Downloads folder.

```
mstream-player admin discovery               # the discovery network (P2P)
```

**Discovery** is the webapp's Discovery page: one state line (connected, joined and waiting,
reconnecting, not joined), the four tabs — Stats, Activity, Invite, Config — and the servers
you follow as a table, polled every ten seconds. `←`/`→` switch tabs. On a row, `Enter` opens
the server's sheet with everything the webapp's card offers; `d` downloads or updates its
snapshot, `p` pins it against rotation, `r` removes the snapshot, `f` asks the server to
federate (a message plus the libraries you would share back), `g` forgets an offline server,
`b` blocks behind a warning. `/` filters, `h` shows the servers hidden for an incompatible
embedding model, `u` lists the blocked ones. Invite holds the endpoint and the ticket a friend
pastes (`y` copies it where the terminal allows); `j` focuses the box a friend's ticket goes
into. Config edits the announced name and description (`e`) and the five numeric settings
(`1`–`5`). Off the network, the room is the webapp's pitch with one card that joins at once
and the federation-requests opt-in, on by default; `x` leaves again, behind a warning.

```
mstream-player admin federation              # requests, the tickets you minted, the peers you can read
```

**Federation** is the webapp's Federation tab as three tabs on `←`/`→`, each a table with its
own affordance on top. **Requests** holds the inbox switch (`s`) and every pairing ask in
either direction with the webapp's state words; on a row, `a` accepts (a form: the libraries
they may read, the bandwidth limits, an expiry, and whether to take what they offered), `n`
declines behind a warning, `c` withdraws your own ask, `d` dismisses a finished record.
**Tickets** is the mint card (`n`: who it is for, the libraries, the limits) and the keys you
minted; the ticket to hand over is shown whole and `y` copies it, `l` edits limits and the
expiry, `b` lets a claimed ticket be claimed again, `r` revokes behind a warning. **Peers**
has the paste box (`j`) with the ticket decoded beneath it before anything is sent, then the
servers you can read: `t` tests one and shows what it shares, `s` flips the discovery opt-out,
`r` forgets it behind a warning. `x` turns the endpoint off, behind a warning.

```
mstream-player admin backups                 # each library's copies on other drives
```

**Backups** is the webapp's Backups page, still marked beta there. One state line carries
the run in flight with the kit's scan bar, or the tasks waiting behind a scan, or idle. The
add card sits above the table of destinations — library, folder, trigger, retention, the last
run's status and age, on or off. On a row, `Enter` opens the last fifty runs with the chosen
run's notes beneath, `n` runs it now, `e` edits it, `s` flips it on or off, `r` removes it
behind a warning that the files on disk stay. Adding and editing share one form: the library
(fixed when editing), the trigger, the destination folder — typed (`~` is the server user's
home), or `^B` browses the server's disk, or the OS dialog with `--same-machine` — with the
server's own verdict on the path beneath it as you type, retention, throttle, the daily hour,
and the exclude patterns, prefilled from the server and sent only when changed so an untouched
list keeps following the server's defaults.

```
mstream-player admin torrents                # the torrent client and what it does for you
```

**Torrents** is the webapp's Torrent Client page, beta there too. Until a client is chosen and
connected, the page is the setup: the client as a radio group (Transmission, qBittorrent,
Deluge), who may add torrents beneath it when the server has more than one user, and the
connect form where `^T` probes without saving and Enter saves after a good probe. Connected,
one state line carries the daemon's status and five tabs follow. Torrents is the daemon's
list with a progress bar, a name-or-hash filter and `r` to drop an mStream-added torrent (the
files stay). Libraries shows the daemon-side path of each library with the verified /
inferred / probing / unconfirmed ladder and its destination template: `d` re-probes, `m` maps
a path by hand, `t` edits the template with a live preview. Seeding hands the server a
`.torrent` for content already on disk (`a`, a typed path or the OS dialog with
`--same-machine`). Access holds the policy and the per-user whitelist, and only exists with
more than one user. Client shows the connection: `t` tests, `c` switches client, `x`
disconnects behind a gate.

```
mstream-player admin users                   # who has an account, what each may do, which libraries they see
```

**Users** is the webapp's Users page as one table: USER · ADMIN · LIBRARIES · FOLDERS · UPLOAD ·
AUDIO · MODIFY, every permission a checkbox cell that `1`–`5` (or a click) flips at once. `l`
edits the library grant, `p` sets a new password (typed twice, since the terminal masks it),
`r` removes behind a gate that names what goes with the account — playlists, play history and
library grants; the music files stay — and warns when the row is the only admin, who also
cannot be un-admined. `a` adds a user with the webapp's defaults: every library ticked, admin
only on a server with no users yet, folders and uploads on, server audio off. A server with no
users runs without logins; the room says so in gold, and once the first user is created it
signs itself in as that user and keeps the session, so the panel stays open instead of
bouncing to the sign-in page. MODIFY is the file-change permission the API carries and the
webapp no longer shows.

Without a saved session for that server — or with one the server no longer accepts — the
hub opens a sign-in page first and keeps the session for next time, the way `login` does.
The account must be an admin's. Two server settings answer with errors the room
explains in words: `lockAdmin` (the panel is locked, HTTP 405) and an address-restricted admin
panel (403 from another machine).

## Stats

```
mstream-player stats                         # your listening on the saved session's server
mstream-player stats --server http://nas:3000
```

The server's `/stats` page (mStream 6.27 and up) as a page of its own, in the admin rooms'
chrome and behind the same sign-in — but for any account, not an admin's, because the
listening log is per account. `←`/`→` switch three tabs, and the period is a dropdown
(`This month ▾`) on the Overview's controls row — the other tabs carry it at the right of the
tab row. **Overview** is the webapp's six tiles as cards (plays, listening time, tracks,
skips, streak, sessions, each against the previous period), plays per day as a column
chart on a whole-number scale with the busiest day named, the 24-hour profile with the peak hour, and — while the
log holds federated peers' tracks — where the tracks live. **Top** ranks tracks, artists,
albums or genres (`t` cycles) by plays or by time (`m` flips), with the share bar, the
plays, the time and, for tracks, the last play; the cursor row's rest goes on the note line.
**Recent** is the log, newest first, paged as the cursor nears the end: the time and day,
the track, the outcome in the webapp's words (completed, skipped at 0:08, stopped at 5:40,
scrobbled at 0:30 for a play an older client sent), what was listened, and the client —
web player, this player, the mobile app — or *not counted* where the server did not count
the play. `x` forgets a play behind a gate; the log and the totals reload.

`[` and `]` step through the periods the log has data in — this and last week, month,
quarter, half-year and year, the older months and years, all time — and `p` or a click on
the period drops the list; `o` cycles the origin (all, this server, peers) while peer plays
exist. Everything loads on entry and on a change, and a change keeps the page as it stands
until the new numbers land; there is no poll. Times are in the machine's zone, read the way
`date` reads it (`TZ`, else `/etc/localtime`; on Windows, the zone Windows is set to), and the
server is asked for its periods and buckets in that zone; a machine with no zone at all gets
UTC, and the Recent tab says so.
A server older than 6.27 has no Stats API, and the page says that instead. The player
reports its own plays (see Play reporting below), and the log names them *this player*;
a total under a minute reads in seconds.

## MP3 player (M5Stack Core2)

```
mstream-player device list                        # the Core2-shaped boards on USB, each with its firmware and card
mstream-player device list --ports                # just the ports: nothing is opened
mstream-player device flash                       # install or update the player firmware on one
mstream-player device flash --firmware .pio/build/core2   # a build of your own (or its *-full.bin)
mstream-player device flash --release v0.5.0      # a particular release
mstream-player device flash --flash-mode dio      # this player's release, in DIO
mstream-player device releases --pre-releases     # the firmware's releases on GitHub
mstream-player device flash --yes --port COM3     # no screen, no questions
mstream-player device flash --yes --all           # update every board that runs an older release
mstream-player device cards                       # the SD cards in this computer's card readers
mstream-player device card E: status              # one card's facts, as key-value lines
mstream-player device card E: setup --name Car --yes     # make a new card one the player syncs
mstream-player device card E: sync                # print the plan and ask to go on; --yes runs it unasked
mstream-player device card E: eject               # let the card go
mstream-player device format --port COM3          # format the board's card as FAT32: the player asks first
mstream-player device time COM3                   # set the player's clock when it is unset or off: its plays are dated by it
```

The [mStream MP3 player](https://github.com/IrosTheBeggar/mstream-mp3-player) is an M5Stack
Core2 running its own firmware, and this is how you see what it runs and how the firmware gets
onto it, over the board's USB cable. The page finds the board by the USB serial chip M5Stack
ships (a CH9102 or a CP2104) and **asks the firmware running on it, without restarting it**:
the port is opened with DTR and RTS held low, so the board's screen stays on and its music
plays on. What comes back is one card. Its head is the board's name and port; its **Firmware**
row is one chip — ✓ *Up to date* (the board runs exactly the release this player carries, or
one ahead of it, which is never offered a step back), ! *Update available* (an older release,
or a development build based below it), ✗ *Not mStream firmware*, ? *Not answering*; its
**SD card** row is a bar of the space used with `21.4 of 59.6 GB used · 1,284 tracks` and the
free space under it, in decimal GB as the Core2's About shows them — or what the card is and
how to fix it (no card, exFAT, GPT), or why it cannot be shown (firmware too old to report it,
a write under way): the card comes from the firmware's `@status`, which firmware 0.9.0 and
later answer — v0.14.0, the release this player pins, among them, so a board on v0.8.0 or
older is told *Updating shows it.* Free space a card never counted is offered as one count
(`c`), never started by itself. One button follows the chip — *Update ▸*, *Install ▸*,
*Read the board ▸* (a board that does not answer: reading it restarts it, said on the card
first), *Try again ▸*, or none — and **every write passes a warning first**: what goes on, the
dark screen while it writes, what stays (settings, the paired headphones, the music on the SD
card), and that unplugging halfway leaves it half written; Enter, Esc and `n` keep the board
as it is, `y` writes, and the board is reset only then. The erase box is in the install's
warning alone, on by default over other firmware. When the board restarts, the card is the
board again — `✓ Up to date · v0.14.0 · just written`, its SD card as the new firmware reports
it — and one line says what was done. Everything technical — the port and bridge, the flash
size, the firmware's ELF, where this player's image came from, the card's bytes, the run's log
with a clock, and *Write again* — is behind **Details** (`d`, or
`l`), which opens by itself when a write fails. With several boards plugged in, each gets a
port tab wearing its mark (✓ ! ✗ ?, or a write's percent), `←` `→` switch, *Show on the player*
(`s`) puts "This one" on a board's own screen, and *Update all* (`a`) writes every board that
needs it, one after another, behind one warning that names them all; while one board is
written the others can be looked at, never written. With no board the card watches the USB
ports by itself (*Not showing up?* holds the cable, driver and dialout hints and the serial
ports seen). A port another program holds is that board's own state, *Ask again* (`r`) once
it is free. No mStream session is involved.

What a write puts on a board is chosen behind **Advanced…** (`o`), a sheet over the card:
the **firmware** — this player's release, another release (listed from GitHub only when you
choose it; pre-releases behind a switch), or a local build (a merged `.bin` or a PlatformIO
build folder, picked with your system's own file or folder dialog or typed with Tab
completing it, and read at once to check it is mStream firmware) — the **flash mode**, *QIO*
(faster) or *DIO* (runs on every Core2), and an optional **erase**. Apply makes that board's
next write: one line under its chip says it (`Next write: v0.7.0 in QIO (faster)`, with a
*Reset*, `x`), its button follows it (*Write ▸*, *Update ▸*, *Go back ▸*), the warning names
the source, the mode and the check it passed, and the write uses it up. Nothing in the sheet
writes anything, and up to date is still this player's release: after going back to v0.7.0 the
card offers the update again. A board running DIO says so under its chip, and its updates keep
it; one that keeps restarting after a write is offered its DIO image as its button.

The GUI player has the same page as its **MP3 Player** tab (`K`): `device flash` with no
flags — the pinned release, every board watched — under the top bar, its hint on the footer,
Esc (or `K`) back to the Library. Opening the tab resets nothing; leaving it lets the ports go
(coming straight back waits a moment for that), and quitting the player waits for a board
being read to restart. While a write runs the tab cannot be left; Ctrl+C, closing the window
and, on macOS, Cmd-Q still quit, and leave the board half written. The tab's Advanced
options are the flags' choices — `--firmware`, `--release`, `--flash-mode`, an erase — for the
board in view; `--port`, `--yes` and an app-only image are the command line's alone.
`device flash` on its own is the same page in a terminal of 72×24 or more, its flags filling
in each board's next write (the card says which flags set it, and `x` goes back to the pin).

The firmware comes from the firmware repository's GitHub Releases: the release this player
pins (its tag and the checksums of both its builds sit in `src/device/firmware.rs`, bumped with
player releases), `--release <tag>` for another one (checked against that release's
`SHA256SUMS`), or `--firmware` for a file — a release's `*-full.bin`, or a build folder with
its `firmware.factory.bin` (a PlatformIO `.pio/build/` with one environment in it will do).
`--flash-mode qio|dio` picks the build of the pin or of `--release`; without it each board keeps
the mode its ELF says, where the release has that build. A file has its own, read from its
header, and a flag that says otherwise is refused. A board is always measured against
the pin, whatever the flags choose: they are what the next write puts on it, and the `plan:`
line says so (`v0.7.0 in QIO, release v0.7.0 — not this player's release`). Downloads are kept
under the player's cache directory, each release's beside the `SHA256SUMS` it was checked
against, so a release downloaded once is written again without the network;
`MSTREAM_FIRMWARE_BASE` points the downloads at a mirror (`{tag}` in it becomes the release's
tag). `device releases` lists the firmware's releases — one request to GitHub's API, which
`MSTREAM_FIRMWARE_API` can point elsewhere — newest first, with each one's date, its builds and
what of it is on this computer; `--pre-releases` adds the pre-releases. `--erase` and
`--no-erase` set the install warning's erase box (and an update's erase); `--yes` prints each
step and writes without a screen, the flags answering the warning.

`device list` asks each board what it runs over USB, all at once and with no reset (the port is
opened with DTR and RTS held low, so the music on it plays on): the firmware's `@status` line —
its version and its SD card's size, free space and tracks — or, from firmware older than the
query, the console's `L` for the version alone. Each board's line is the port's as before, then
its firmware against the pin (`v0.7.0, update to v0.14.0`) and its card (`card 38.2 GB free of
59.6 GB, 1,284 tracks`); a board that says nothing is `not answering`, and a port another
program holds is `in use by another program`. With several boards, `--yes` still asks for
`--port` — its refusal lists each board's firmware now — or `--all`, which updates every board
running an older release of this firmware to the pin, one after another, each in the flash mode
it runs (or the one `--flash-mode` names), stopping at the first failure; it never installs
over other firmware, never erases and never goes back, and names the boards it left alone.

The pinned release is v0.14.0, whose image runs the board's flash in QIO, the faster mode. M5Stack
ships the Core2 in DIO and only one Core2 has been tried in QIO, so the release also carries a
`-dio-full.bin`, the same firmware in DIO: if a Core2 keeps restarting after an install,
`device flash --flash-mode dio` writes it, or the tab's Advanced… does (Flash mode, DIO). It
keeps the settings like any update. A board's
mode is read from the ELF id it reports, against every release's pair, so a Core2 on DIO stays
on DIO through every update with nothing remembered anywhere; `device list` says `· DIO` beside
its version. After a write, a board that prints the chip's reset banner again and again and
never its firmware's first line is said to keep restarting: the card goes gold, *✗ Keeps
restarting*, and its button is *Write the DIO image ▸* — through the warning, never written by
itself.

The board's SD card is filled with the card in **this computer's card reader**, never over the
cable. A card in a reader is a subject of the MP3 Player tab beside the boards, a tab of its
own (`E: Car music` on Windows): its **Card view** says what it holds and from which saved
server, its space, its last sync and what the next one would do (`12 new · 2 to remove · 37
plays to send`, as that server answered it), and *Sync ▸* fills it — converting what the
player can't play, writing the covers, the Auto DJ index and each track's waveform for the
progress bar, all of which the firmware reads, laying each track's mood scores for the
player's mood filters (none on a card for someone else), and sending the player's plays
back to the server they came from (none from a card for someone else, then or later, and none
made before the player's clock was ever set: the tab sets it whenever it sees the player on
USB, and `device time <port>` does from a terminal). A sync saves as it goes: `x` stops it
at the next save, a card taken out goes on from there in any reader, and it runs on while
you listen, the MP3 Player tab wearing its percentage. **Settings…** (`o`) holds the card's
choices on two tabs, Music (its server and libraries) and Writing (what is converted and to
what, removals, covers, Auto DJ, a card for someone else), with a line that says at every
change whether it fits. More than 50 removals, half the card gone, music on the card that
mStream didn't put there, another server, a relay and a peer reached only through its parent
each ask first, behind the gold warning. Quitting with `q` during a sync asks, then stops at
the next save; the window's close button and Cmd-Q stop it at the next save without asking.
`device cards` lists the cards, one line each, and `device card <drive>
status|setup|sync|plays|eject` does what the Card view does; each warning is a flag that names
what it lets through (`--allow-removals 214`, `--server Work --take-over`, `--adopt` or
`--keep-mine`, `--through-parent`), and `setup`'s flags are the Settings' (`--name`,
`--libraries`, `--convert`, `--to`, `--removals`, `--covers`, `--autodj`, `--for`,
`--budget`). Exit 3 means paused or stopped: run it again later. Only FAT32 cards on MBR are
the player's; a card it can't read is listed with why.

A card the player can't read (exFAT, NTFS, a GPT card) is **formatted in the player**, never on
the computer: with firmware 0.14.0 or later, the board's SD card row offers *Format as FAT32 ▸*
(`f` where something else comes first; for the player's own music card, in Details only), behind
a gold warning that says what is lost. The tab only asks: the player shows the question on its
own screen, *Format…* then its red button held 2 s, restarts and formats the card, and the row
follows it to the empty card, ready to fill in a card reader. A damaged card is sent to a
computer's disk check first. On older firmware the row says it can't format the card, and
that updating to v0.14.0, the pinned release, lets it. Leaving the tab never stops it. `device format` does the same from a
terminal; it has no `--yes`, since the player confirms.

Windows and macOS need the USB serial driver from M5Stack's download page; on Linux your
account needs the `dialout` (or `uucp`) group. A port held by a serial monitor is reported
as such. `MSTREAM_DEVICE_FAKE=<word>` runs everything against a scripted board, for tests:
`fresh`, `ours:<version>`, `other`, `nodevice`, `two`, `busy`, `nosync`, `failwrite`, and for
the running firmware's answers `status:<version>` (answers `@status`), `old:<version>` (`@err 7`,
then `L`), `silent:<version>` (ours, not running) and `chip` (a board that only shares the USB
chip). Several boards are words separated by commas, and options after slashes script the rest:
`status:v0.9.0/free=?,old:v0.7.0,chip/in=3` is three boards, the last plugged in three seconds
on (`/out=`, `/held=`, `/fail=write`, `/identify=ui`, and any `@status` field). A 0.14.0
board's `@format` is formatted by default, or `/format=no`, `late`, `silent`, a refusal
(`viz`, `card`, …, `second-size`), or `fail-write` and its kin. `/mode=dio`
runs a release's DIO build (its ELF id), `/elf=<hex>` any ELF, and `/loop=qio` is a Core2
whose flash cannot run QIO: a QIO image written to it keeps it restarting. src/device/engine.rs
has the whole grammar.

## Now Playing

`0` gives the whole screen to what's playing, under the top bar: the tags down
the left as a labelled column with the cover and prev · play · next beneath
them, a full-width waveform band along the foot, and a tabbed panel on the
right.

```
                          │ [1:Queue]  2:Discover   3:Auto-DJ   4:Visualizer
  6AM                     │ ───────────────────────────────────────────────
  Boukmanflow             │ > Boukmanflow - 6AM                        2:03
                          │   Boukmanflow - Been a While               2:07
  Year    2023            │   Boukmanflow - Being stuck                2:19
  Genre   rap, jazzrap    │   Boukmanflow - Better Days                2:41
  Tempo   93 BPM          │   Boukmanflow - BIG PICTURE                2:46
  Key     5A  C minor     │   Boukmanflow - Bout2skate                 2:16
  Format  MP3   192 kbps  │   Boukmanflow - Buckets                    1:55
 ─────────────────────────┴────────────────────────────────────────────────
 ▓▓▓▓▒▒▒▓▓▒▒▒▒▓▓░▒▒░▓▒░░░░▓░▒█░▒▓░▒▒▓▒▓▒░░▒▓▓▓▒▓▒█▓▒▒▒▒▒▓░▒█░  0:02 / 2:03
                                                                   vol 100%
```

The digits pick a tab, numbered on the strip in the order it draws them —
`1`–`4` above, because that session's track has no lyrics. `↑` `↓` move
within a tab, and `Tab` and `Shift+Tab` step along it for anyone who would
rather not count. It's a view rather than a dialog, so everything else keeps
working — `Space`, `n`, `p` and volume all behave as they do outside. `Esc`
or `0` leaves. Click a tab to pick it, and the band to seek.

The numbers rather than `←` `→` for a reason worth stating: **Auto-DJ's rows
are values**, so `←` `→` adjust them there. When the arrows were also the
navigation, that made the one tab you could get stuck on the one tab whose
way out was different — and people got stuck on it. The arrows now belong to
whichever tab is in front, and the way between tabs is the same key
everywhere.

On **Queue**, `Enter` plays and `d` removes the row under the cursor.
**Auto-DJ** is where Auto-DJ is *set*, not just reported. **Lyrics** appears only
for a track the server has words for, so the strip
changes shape as the queue moves. **Discover** needs the server's index. Where
the strip won't fit, it shows the tab you're on between arrows rather than
truncating the last name.

**Visualizer** cycles modes with `v`: spectrum, cover, scope, vectorscope, vu.

The facts column keeps the album cover under the track's details — as real
pixels on a terminal that can draw them (kitty, sixel, iTerm2), and as the
same half-block mosaic the cover visualizer uses on every other terminal. The
player asks the terminal itself at startup; anything that doesn't answer with
a pixel protocol — Terminal.app, a pipe, tmux without passthrough — gets the
mosaic, with no escape bytes ever written blind. Terminals lie in both
directions, so both lies have an override: `MSTREAM_NO_GRAPHICS=1` forces the
mosaic on a terminal that advertises more than it delivers,
`MSTREAM_GRAPHICS=kitty|sixel|iterm2` forces a protocol on one that hides
what it can draw, and `mstream-player graphics-probe` prints what your
terminal answered, then draws a test card both ways so one glance settles it.

Lyrics is the last stub; everything else on the strip does what it says.

The key is the same Camelot code Auto-DJ matches on, with the tag's own
spelling beside it. Tags a track doesn't have are left out rather than shown
empty.

### The Discover panel

The full-screen view's Discover tab follows the speakers. No seed to pick,
no menu — one question, asked again whenever the answer would change.

```
 Songs similar to what's playing

  83%  ALM - Manor                            4:11
  81%  ALM - Hip Hop Factory                  5:02
  80%  Color Out - Ghosts (Abludo Remix)      4:09
```

The number is how close the server thinks it is — these arrive in order, so
a rank would say nothing a position doesn't, where the percentage says how
much of a neighbour it actually is.

`Enter` queues one and starts it, `a` just queues it — neither throws away
the queue you are in the middle of. This is a panel you glance at, not one
you steer.

## Auto DJ

Auto DJ keeps the music going: when the queue has nothing left after the
track that is playing, it asks the server for the next songs and adds them.
It is **on or off**: click the bar's `auto-dj` button (or press `A`) to
start it, and again to stop it — Stop always stops, wherever the DJ is
running. Its picks wear a badge in the queue. Started on an empty queue it
asks for a **first song** — **Random** (a song under your filters),
**Choose** (pick one from the library) or **Recently played** (one of the
last twenty songs played on that server); **Remember** keeps the answer,
and Esc leaves the DJ off.

The settings live in the **Auto DJ room**, under TOOLS in the nav (or `D`). The room is one row per setting, each showing its value and
opening one small sheet; **Start** / **Stop** sits on top and is the status:

| Row | |
|---|---|
| **Play from** | which server the DJ picks from, and which of its folders. Start goes to this server whichever one you are browsing; it is remembered. (Shown once there is more than one server or folder to choose.) |
| **First song** | what Start does with nothing queued: Ask · Random · Choose · Recently played. |
| **Batch** | how many songs the DJ adds at a time (mStream 6.26 and later; one before). |
| **Sonic similarity** | only pick songs that *sound* like the session: how close a match, and whether to **Drift** with the mood or **Stay close** to the song you started from. Needs the server's discovery index. |
| **Smart filters** | the mood filter — happy · sad, energetic · relaxed, vocal · instrumental, tags — where the server has mood scores for its songs. |
| **Genres** | **Only** these genres, or **Skip** them; and, where the server has both, whether to match generated genres, the files' tags, or either (mStream 6.32.2 and later). |
| **Rating** · **Length** | skip anything rated below; keep to a length window, typed to the second (`2:00`, `90`, `1:02:30`). |
| **Beat matching** · **Harmonic mixing** | prefer the tempo near the current song's, within a tolerance; prefer a key that mixes well (the Camelot wheel's neighbours). |
| **Skip words** | words a title, artist, album or path must not contain. |
| **Artist cooldown** | keep the last few artists out of the next picks. |
| **Preview picks** | on the title row: three picks with the current settings, none of them queued. |

A row the server cannot do stays where it is, greyed, and says why — "Needs
server 6.25+", "Discovery off", "Still scanning", "Not on peers". The
chevron half of the bar's button (`▴`) opens **quick config**: the DJ's
server, sonic similarity and smart filters without leaving the room you
are in.

Now Playing's Auto-DJ tab (`0`, then across) keeps the same settings as a
list of rows; `A` there starts the DJ on the server you are on.

The rules are kept per server, with the global settings behind them. A
server that does not know a setting is told once: the DJ learns which keys
it rejects and drops them for the session, so an older mStream still plays.
`mstream-player info` lists what a given server has enabled.

## Sonic Path

The Sonic path room is the webapp's panel: pick a song to start from and one
to end on, choose a length, and the server fills the road between them with
tracks that sit along the arc joining the two in its embedding space.

Either end offers the two ways the webapp does: **use playing song**, or
**pick from library**. Picking from the library *arms* the browser — the next
track you open anywhere (files, library, search, a playlist) lands in the
field instead of on the speakers. `Esc` calls it off.

Build, and the setup gives way to the path: play it, which replaces the
queue, queue all of it onto the end of what is already there, or save it as a
playlist on the server. Changing the length plots again rather than trimming:
a shorter path passes through different places entirely.

The room needs a server with the discovery index.

To see what Auto-DJ would pick without launching the player:

```
mstream-player dj "music/Artist/Album/01.flac" --mode similar
mstream-player dj "music/Artist/Album/01.flac" --mode tempo
```

## Where it keeps things

Two files, in `%APPDATA%\mstream-player` (Windows), `~/Library/Application Support/mstream-player`
(macOS) or `~/.config/mstream-player` (Linux). Set `MSTREAM_PLAYER_CONFIG_DIR` to put them
somewhere else — a portable install can keep them beside the binary.

- `config.toml` — servers and preferences. Readable and editable; fine to keep in a
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

The player's debug log is off by default. To keep one, say so in config.toml:

```toml
[log]
write = true
level = "debug"    # info (the default), debug or trace
```

The file is `<cache>/logs/mstream-player.log`, with the last few runs kept beside it. info is
the player narrating its own decisions (what it played, seeked, prepared, handed over, and what
failed), while debug and trace add what iroh, the HTTP stack and the spool have to say.

Whatever the level, secrets are taken out before a line reaches the file: query strings (an
mStream media URL carries `?token=…`) and any credentials in a URL are replaced with
`<redacted>`, and the file is created readable only by you. The log is meant to be attachable to
a bug report.

Underneath sit two switches, off by default, each writing to a file and never to the screen —
the player silences stderr while it draws, so files are how a session explains itself afterwards.

- `MSTREAM_LOG=1` captures what the *dependencies* narrate — iroh's relay connects, holepunch
  attempts and path changes, HTTP retries — into `<cache>/logs/`, one file per run named for its
  process, with the last few runs kept and older ones swept away. `MSTREAM_LOG=/path/to/file`
  writes exactly there instead. `RUST_LOG` chooses how much is said (the usual tracing filter
  grammar; unset means `info`). The player prints `logging to …` at start and again at quit.
  A file that reaches 8 MiB rolls aside to `.1` and a fresh one continues, so a jukebox left
  running for a week is bounded. One-shot commands like `ls` and `info` never touch any of
  this unless `MSTREAM_LOG` names a file for them.
- `MSTREAM_ENGINE_TRACE=/path/to/file` is the player's own flight recorder: one timestamped line
  per transition decision — plays, announcements, seeks and their clamps, prepares, open failures
  with reasons, retries, handovers, and every output device opened, how long it took and what a
  device that refused said — plus every diagnostic the player kept off the screen. Each run
  appends and signs itself with a banner, so two players sharing one path interleave rather than
  overwrite each other.

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

It watches the output device as the player does. Change the output while it plays (`--test-seconds
120` gives the time) and the move prints between the status lines; the last line before the
verdict is the longest any tick of the engine took, which stays at a few milliseconds however
long the device takes to change.

A status line ends in `(waiting for sound)` while the track is playing with nothing decoded to
play: its download has stalled, or a seek has not landed yet. The position stands still until
there is.

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
  track into the playing sink ahead of time. The next track is opened ahead too (from 12 s before
  the end), so the boundary does not wait on its open, and neither do requests arriving then —
  without it, serve opens the next track at the boundary itself. If that open is still running
  when the track ends, the boundary opens it itself, as without the flag; like `--crossfade`, it
  needs track durations, so sources of unknown length (a live transcode) are not opened ahead
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
