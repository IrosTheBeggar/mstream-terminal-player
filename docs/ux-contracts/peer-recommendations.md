# Peer Recommendations — the network's leads

| | |
|---|---|
| **Design of record** | **For what is drawn:** the canvas "Peer Recommendations — terminal page", five boards drawn cell for cell at 100×30 and 100×24 — Page (`ALL PEERS · IDLE`, `PREVIEW PLAYING`, `LEAD SHEET`, `KITCHEN RADIO TAB`), Way in (`TOOLS · FIRST ASK`, `FLOOR · KEY HINTS`, `FLOOR · HINTS OFF`), Lead (`GET IT · DOWNLOADING`, `OPEN ELSEWHERE`, `OFFLINE · ASKING`, `NO PREVIEW`), States (`NO PEERS YET`, `NOTHING NEW`, `TRY AGAIN`, `SCAN PENDING`, `ADD STATES`, `PREVIEW PAUSED`) and Variants (`MEMBER ACCOUNT`, `NO PREVIEW PLUG-IN`, `QUEUE OPEN`, `NATIVE WINDOW`) — and its board spec of 2026-11-20 (two critic rounds), which wins over the boards where they differ. The spec's TEXT and STYLE mocks are in this repo verbatim (`src/kit/boards/peer_recs/*.txt`), and the room's tests match them cell for cell (`kit::boards::assert_frame`). The spec supersedes the approved board "B + C · Terminal" (2026-10-27) where §1.14 says. **For what it does:** mStream's plan `docs/peer-recommendations-plan.md` @ `9116d63b` (master, with #1086, #1088 and #1132) — §2 (items 2, 6, 10–19, 21 and 23 here), §3.1–§3.6 (the server contract and the client re-ask rules R1–R8), §4.3 (this surface; its anchors and nav rows were taken at the player's `12ee9a8` and are stale), §5.3–§5.7 (the lead key, preview pacing and lifetime, the strip states, copy); the webapp as shipped at that commit (`webapp/alpha/peer-recs.js`, `m.js` `peerRecs*`, `vp.js` `clip*` and `aside*`, with #1132's dead-clip rule); the phone as shipped (`mstream_music` origin/master `5a1c8c0d`: `lib/screens/discovery/peer_recs_controller.dart`, `peer_recs_previews.dart`, `preview_answers.dart`, `lib/util/peer_recs_rules.dart`, `lib/media/preview_player.dart`). Where the plan says "as each client's Discover does", this player's own Discover room (docs/ux-contracts/discovery-page.md) is the record. **Precedence:** the design wins for anything drawn (cells, words, glyphs, colours, which control exists); the plan's §3 and §5, then the web and the phone, win for behaviour; every place they disagree is in the log (S1–S37). |
| **Server API** | Every route is asked of the **user's own server** (`own_server()`), never a connected server's. **`POST /api/v1/discovery/taste`, generation 3** — the body keys `networkPeers` (1–50 ids of 64 hex; `[]` is a 400) and `networkOnRandomSeed` (walk the network on a random pick too); the answer's `network: {results: [lead], searched, reason}` with `reason` one of `null`, `empty-shelf`, `not-on-shelf`, `no-compatible`, `nothing-new`, `scan-pending`, `error`; a lead is `{artist, title, album, year, isrc, releaseGroupMbid, recordingMbid, duration, similarity, exportId, source: 'p2p', seedHash, peer: {endpointId, name, label}, tile: {key, hue, letter}}`; `network` is `null` with p2p off (on an unfiltered, non-random answer) and under any `moods` body. **`GET /api/v1/discovery/p2p/peers`** (member-readable) → `{canAdd, shelf: {held, searchable, running}, peers: [{endpointId, name, label, monogram, hue, online, onShelf, pinned, rowCount, add: {state: adding \| done \| failed, code, at} \| null}]}`, shelf rows first (online, then label), then the addable rows (admins only, at most 24); 403 when p2p is off or for a guest. **`POST /api/v1/admin/discovery/p2p/peer-dbs/add {endpointId}`** → 202 adding, 200 `done`, 200 `failed` with a code (`no-room`, `offline`, `bad-snapshot`, `no-disk`, `gone`, `failed`), 404 `not-in-catalog`, 409 `blocked` / `incompatible`, 403 / 405 the admin gates. **`POST /api/v1/discovery/plugins/{name}/resolve {recommendation}`** → `{result: {preview: null \| {provider, url, seconds, title, artist, album, link, artwork, attribution, match}}}`, 429 at the provider's limit, cached 10 minutes server-side. The routes Discover already speaks: `GET /api/v1/discovery/plugins`, `GET /api/v1/discovery/collection/destination`, `POST /api/v1/discovery/plugins/{name}/jobs {recommendation, destination}` (409 `destination_changed`), `POST /api/v1/discovery/plugin-jobs/lookup/batch`, `GET /api/v1/discovery/plugin-jobs/{id}`, and the `links` plug-in's resolve. **Gate:** the ping's `discovery && discoveryP2p && discoveryTaste >= 3 && discoveryP2pPeers >= 1` (flat keys, or under `/api`'s `features`). |
| **Already in this repo** | Reused as they are: Discover's `layout()` rhythm, its table rule, `scroll_list`, `ListView`, `sel()`, `bar::clip`, the bar's note, `kit::clipboard::copy` and `kit::width`; `text_button`'s look (through `text_button_in`, which takes the `Surface`); the track-actions sheet's frame and badge row; Keep's links list (`app.keep.links`, `draw_links`, `links_walk`, `links_enter`) and the job marks (`mark_glyph`, `mark_style`, `mark_word`, `ell`); `TasteResponse` and `TasteLead` (extended), `PluginFailure`, `send_plugin` / `own_server`, the job start and lookup calls, `name_key`; the mock-socket test kit, `App::tick_at`, the seq and `session_gen` fences, `opener::open_target`, and the e2e harness. **Built 2026-11-20** (one PR, "feat(gui): the Peer recs room (and the engine's aside playback)"): the generation-3 types, the five client calls and one worker door (`src/api/types/peer_recs.rs`, `src/api/peer_recs_door.rs`: `ApiCmd::PeerRecs` → `Event::PeerRecs`); the engine's aside clip (`src/engine/aside.rs`, the `AsideThen` / `AsideStatus` / `AsideEnd` types in `src/player.rs`, five `AudioCmd`s); the App room under `tui::app::taste` (`peer_recs.rs`, `rules.rs`, `previews.rs`); the GUI room (`src/gui/peer_recs.rs` with `tabs.rs`, `grid.rs`, `panes.rs`, `band.rs`); Discover's bar lent out through `src/gui/discover/taste_bar.rs`; the sheet's lead form (`src/gui/actions.rs`); the kit's `truecolor`, `swatch` and `busy_glyph` and its board matcher (`src/kit/boards.rs`); the strings; the fixtures `test/fixtures/peer_recs/*.json`; the e2e scenarios R and R2. |
| **Target surface** | The GUI player, both flavours (`mstream-player gui` and `gui --window`, which draw the same cells), as a TOOLS nav room. The TUI and the browser build compile the App half and never open the room. |
| **Status** | Contract written 2026-11-20 beside the build, from the design spec, the plan and the shipped web and phone. The live walk on the stage (`test/stage/peer-recs-stage.mjs`) records what it finds in the log. |

**How to cite.** Code cites this file as "peer-recs contract §1.n" for a
section, or "§1.n.m" for a clause. The section numbers mirror the design
spec's §1.1–§1.14, so a section here and the same-numbered section of the
spec talk about the same thing; §0.6 (colours) and §0.7 (glyphs) mirror
its two tables of the same numbers. The build's decisions keep the numbers
the build plan gave them (D10–D13, D15–D24, D27, D30–D32), so a code
comment that cites one finds it in the log.

## Intent

A second discovery room beside Discover, for songs you do not own. Your
server walks the shelf of peer snapshots it has fetched from the p2p
network, seeded by songs you finished, and answers with **leads**:
metadata only, with no stream and no cover. The room lays them out in
two columns, under a strip of tabs that narrows them to one peer. A lead
never plays: its slot plays a **30 s preview** through the server's
preview plug-in, and its sheet can **Get it**, **Open elsewhere…** or
**Copy title**. An admin can add a peer to the shelf from the strip. A
clip never plays out of sight of its attribution, and main playback
comes back where it stood when the clip is done.

## Entry points

1. **The nav row `Peer recs`**, in TOOLS directly under Discover, row
   **18** at 100×30 with every tool shown (§1.1). Drawn only while the
   gate holds.
2. **`R`** wherever the Library screen takes keys. While the room is up,
   `R` and a click on its row ask for new seeds (§1.4.5).
3. **Nothing else.** No sheet row and no hand-off from another screen
   leads here; the Now Playing screen's own Network tab is unchanged.

## States & flows

The room keeps one **visit**. Entering by the nav (or `R`) from another
room starts a fresh visit: All is selected, the cursor and the tabs stop
are stowed, and three asks go out together — the R1 batch ask (§1.4.4),
the first peers poll (§1.5.7) and the plug-in gate (§1.8.1, §1.9.4).

- **Loading** — no answer yet: the header reads `↻ asking…`, the strip
  is a skeleton until the first poll answers, and the grid draws skeleton
  rows.
- **Ready** — the verbs line (or the band, while a clip plays) and the
  grid.
- **A pane** — No peers yet, Nothing new, Try again or Scan pending, in
  the grid's place, under the strip (§1.10).
- **The lead sheet** — over the room, for one lead (§1.9). The room and
  the poll go on under it.
- **A clip** — the band takes the verbs line's row (§1.8).
- **Off** — not a state: the row goes and the room yields to Files
  (§1.1.6).

What persists, and what asks again:

| Kept | For how long | Asks again when |
|---|---|---|
| The batch (the first answer of a seed batch; its seeds are what the seeds modal lists) | the visit | R1: a fresh visit, New seeds, a window change, a seeds-modal edit |
| Each tab's answer for the batch | until the next batch, or an R4 re-ask | a tab with no kept answer; an `error` answer is never kept |
| The selected tab | the visit, through New seeds, the window and seed edits | — |
| Pins, dropped seeds, random picks passed over | the visit | — |
| The window | `[gui] discover_window`, shared with Discover: read on every enter, written on a pick | — |
| The strip (the last peers answer) | the visit | the poll, every 15 s, every 3 s while an Add runs |
| Local Add states | until a poll sent after them answers | — |
| Preview answers (found or none), per lead and provider | the visit; cleared on a provider change and on a resolve 403; a dead clip forgets its own | — |
| The plug-in gate (the provider, Get it, the links plug-in) | the visit | every enter; a resolve 404; a `destination_changed` |
| The lead sheet's job | while the sheet is open, held by the lead's key | the sheet's opening (one lookup); every 1.5 s while the job is live |

Nothing of the visit is the server's. Leaving the room by the nav ends
the visit; a server switch starts one on the new server or yields.

## Behavior contract

The design spec's §0.1–§0.5 (the boards, their scaffold and chrome, the
cell grid, how to read a mock) belong to the boards and travel with the
mocks in `src/kit/boards/peer_recs/`. Its colour and glyph tables are
contract, so they are kept here under their own numbers.

### §0.6 Colours

The truecolor tier's values (`kit/theme.rs`); every colour is a token,
and the swatch is the one computed per row (§1.12.1).

| token | hex | used here for |
|---|---|---|
| accent | `#7aabdf` | the active nav row, `↻ new seeds`, `▸ preview all`, `+ {peer}` tabs, the selected tab while the tabs stop holds another (BOLD), `‹ ›`, the slot's `▸`, the band's toggle, the sheet's and the links list's borders, Get it at rest and its `downloading…`, slabs, the volume's `▰`, the scrollbar's thumb |
| bright | `#8fd6e8` | hover only |
| dim | `#7a82a0` | nav rows at rest, the room's rule, heads, offline peers, `no room`, text buttons, the attribution, `[ get it ]`, `[X]`, `▸▸`, a muted slot, the skeleton's `░` |
| gold | `#e5c07b` | the seek rule, the play/pause frame, `↻ asking…`, `{peer} adding…`, the busy slot `◌`, a failed tab's name |
| ok | `#98c379` | the online dot `•`, the clip bar's `▰`, the playing queue row |
| danger | `#e06c75` | **not used** in this room: it is the destructive hover's, never decoration |
| text | `#d8dee9` | body text, titles, online peers' names |
| ground | `#12131c` | every cell's background |
| on_accent | `#0d1017` | text on a slab |

**Lead swatches** are `hsl(tile.hue, 45%, 60%)` of the server-stamped hue
(`kit::theme::swatch`). The stage's nine albums, which the tests pin:

| tile key (artist · album) | hue | swatch |
|---|---|---|
| Hollow Coast · Low Tide Hymns | 203 | `#6ba4c7` |
| Paper Lantern · Night Ferry | 39 | `#c7a76b` |
| Tidewater · Undertow | 4 | `#c7716b` |
| Marlowe Fields · Daylight Savings | 352 | `#c76b77` |
| Juniper Static · Neon Psalms | 78 | `#abc76b` |
| The Vesper Lines · Small Hours | 154 | `#6bc79f` |
| Okra Bloom · Porchlight | 353 | `#c76b76` |
| Kestrel & Vine (no album) | 53 | `#c7bc6b` |
| Ada Merrow · Cartography | 81 | `#a7c76b` |

Without truecolor (the 256- and 16-colour tiers) the swatch is DIM, and
so is a lead with no tile. The native window is always truecolor.

### §0.7 Glyphs

| glyph | role | in the GUI | legacy console |
|---|---|---|---|
| `▌` | the lead swatch | new | `▌` (CP437 has it) |
| `◌` | a slot resolving or waiting out a 429 | new here (Auto DJ's unpinned seed uses it too) | `~` |
| `‹` `›` | the peer tabs' overflow | a new use | `<` `>` |
| `▸` | a slot idle, muted or paused; `▸ preview all`; the active nav row | reused | `>` |
| `▮▮` | a slot and the band while a clip sounds | reused (pause) | `\|\|` |
| `▸▸` | the band's next preview | reused (next) | `>>` |
| `•` | the online dot | reused (`bullet_glyph`) | `*` |
| `▰` `▱` | the band's clip bar | reused | the words `NN%` |
| `░` | the skeleton | reused | `·` |
| `[X]` | closes the band's clip and the sheet | reused (the kit's close) | the same |
| `↻` | new seeds | reused | none (the words stand) |

**Never used:** `●` (the online mark is `•`), `✕` (Hack lacks it; the kit
closes with `[X]`), `↵` (tips spell Enter), and underline (the GUI draws
none). BOLD is the only modifier.

### §1.1 The row, its gate and the way in

1.1.1 **The gate** is `Capabilities::peer_recs()` — `discovery &&
discovery_p2p && discovery_taste >= 3 && discovery_p2p_peers >= 1`
— and `session.peer.is_none()`, and the room's off latch not set
(§1.10.6; cleared by the next `Connected`). `discovery_p2p_peers`
is the ping's `discoveryP2pPeers`; `discovery_p2p` is the config
flag, not whether the stack runs. The gate is read, never probed.
It has **no `connected` term**, as Discover's row has none: a short
disconnect keeps the row, and the room draws its name and the
disconnected text. Every ask still needs a connection (D31).

1.1.2 **Absent is absent.** Without the gate the row is not drawn, never
greyed; the rows under it move up; `R` does nothing; nothing is
asked. An older server (taste below 3, or no `discoveryP2pPeers`)
gets no entry and no reduced page.

1.1.3 **Its place.** `NavId::PeerRecs` is appended to the array as
`NAV[16]` (`PEER_RECS_NAV`): past the digits, it takes no digit,
and no other room renumbers. It is drawn in TOOLS under Discover:
`TOOLS = [DJ, SONIC, DISCOVER, PEER_RECS, TORRENT, YOUTUBE]`. The
label is `gui.nav.peer_recs` "Peer recs", at most 12 cells in every
locale. The group packs the rooms it shows.

Rows with every room shown (an owner session, Add YouTube
available):

| size | Files | Search | LIBRARY | Albums … Most played | TOOLS | Auto DJ · Sonic path · Discover · **Peer recs** · Add torrent · Add YouTube | Settings |
|---|---|---|---|---|---|---|---|
| 100×30, hints off or on | 2 | 3 | 5 | 6 … 13 (Rated 10) | 14 | 15 · 16 · 17 · **18** · 19 · 20 | 24 off, 23 on |
| 100×24, hints off (rise 3) | 2 | 3 | — | 4 … 11 | — | 12 · 13 · 14 · **15** · 16 · 17 | 18 |
| 100×24, hints on (rise 4) | **1** | **2** | — | 3 … 10 | — | 11 · 12 · 13 · **14** · 15 · 16 | 17 |

1.1.4 **The rise.** Where Settings would meet the last tool shown, the
block rises only as far as it must:
`rise = (15 + tools).saturating_sub(settings_y).min(4)` and
`lift = rise.min(1) + (rise >= 3) + (rise == 4)`. The steps are the
blank under Search, then the TOOLS heading, then the LIBRARY
heading, and — new here — **row 1**, the blank under the top tabs:
Files moves to `2 − (rise == 4)` and Search to `3 − (rise == 4)`.
The nav's rule `│` still starts on row 2. With six tools the rise
is 4 at 100×24 with hints on, 3 at 100×24 off and 100×25 on, 2 at
100×25 off and 100×26 on, 1 at 100×26 off and 100×27 on, and 0
from 100×27 off. Rise 4 happens only there, only with all six
tools; Settings stands alone on its row at every size.

1.1.5 **`R` opens it** wherever the Library screen takes keys. While the
room is up, `R` and a click on its row are New seeds (§1.4.5), as
`G` is for Discover. `R` is not added to `gui.tips.base`, which
uses 97 of its 99 cells; the room's own tips name it (D13).

1.1.6 **The gate going mid-visit** — an R7 re-ping that says off (after a
poll 403, a taste 403, a `network: null` or a resolve 403), a peer
session, a server without the room — closes the bar's modals and
yields to Files **in the frame that learns it**, before that frame
draws, with no `Connected` needed. A live clip ends and main is
restored (D31). There is no off card, and no Turn on form: the
webapp's admin form is the web's alone.

1.1.7 **Leaving and coming back.** Leaving by the nav (another room, a
digit, a letter key) ends the visit: the poll stops, a live clip
ends (main restored), and late answers are dropped. Coming back is
a fresh visit. **Not drawn, visit kept** — another screen (`0`,
`T`, `M`, `K`), the mini player below 100×24, the Files room under
a seeds-modal pick: a live clip ends (main restored), the poll
pauses, and the first frame the room is drawn again polls once at
once; the batch, the tab, the kept answers and the preview answers
stay. The lead sheet, the window list and the seeds modal are not
a cover: the poll runs under them.

1.1.8 **Server switch:** a fresh visit on the new server when the gate
holds there, else Files. A live clip ends, and main is restored
(D10; S30). **Quit:** the clip dies with the engine's shutdown;
nothing else is owed.

1.1.9 **The seed pick** (the seeds modal's "Pin a song…") arms the
capture from this room. While Files is up for it the room is
covered, not left. The pick lands back here as a seed edit (an R1
ask, §1.4.7); a refused pick is worded once, in this room, with
Discover's words, and Discover's own state is never written.
Re-entering the room by the nav while picking cancels the pick and
asks again, as Discover's does (D32).

### §1.2 Keys

None is drawn, except in the tips line with key hints on (§1.13).

1.2.1 **Stowed** (the kit's list-cursor law): `↓` picks the first lead and
`↑` the last (lead n − 1). When the tab has no rows (a pane), `↓`
and `↑` land on the tabs stop instead. `← →` step the selected tab
through All and the shelf tabs, skipping the add tabs, which are
never selected; the ask waits for 250 ms of rest (§1.5.6).

1.2.2 **The tabs stop** (Discover clause 47's top stop, with the
list-cursor law kept for stowed keys while rows exist).
- Reached by `↑` from either column's first row of the first
  block, or by a stowed `↓` or `↑` when the tab has no rows.
- Lands on the tab the stop last held, when `↑` comes from the
  grid and that tab is still in the strip; else on the selected
  tab; else on the first tab in view. The stop remembers that tab
  only within one trip into the grid: Esc, a click, a stowed
  `← →` step and leaving the room all forget it, so the next `↑`
  lands on the selected tab.
- On it, `← →` move the stop over every tab that acts (All, the
  shelf, `+`, try again; `adding…` and `no room` are skipped),
  scrolling the strip by whole tabs. Enter acts on the held tab:
  select, Add, or Add again. `↓` goes to the grid's first row when
  the tab has rows. Esc stows the stop.
- A tab that turns inert under the stop (its own Enter on `+`)
  keeps the stop until a key moves it, and Enter on it then does
  nothing (D23).
- There is no stop while the strip is not drawn (§1.5.2); `↑` from
  the first row then does nothing.

1.2.3 **The grid** (D22). `↑ ↓` walk the column, across blocks; `↑` from
a column's first row in the first block goes to the tabs stop.
`← →` go to the other column on the same visual row, or to that
column's last lead when it is shorter; they do nothing at the
edges. PgUp and PgDn move one screen of rows, clamped. The cursor
is held as a lead index, so a resize or the queue's toggle
re-blocks the grid under it, and a shorter answer clamps it.

1.2.4 **Enter or `m`** on a held row opens the lead sheet (§1.9) and
never plays; with no plug-ins at all it copies (§1.9.1). `m` is the
room's: stowed, it is swallowed, so it never opens the playing
card's sheet here. **`l`** previews the held row (§1.8.3).

1.2.5 **Anywhere in the room** (no sheet, list or modal up): `L` runs
Preview all, and pressed again stops it; `y` copies the list; `w`
opens the window list; `e` opens more options (the seeds modal);
`R` asks for new seeds.

1.2.6 **While a clip is live** (any state but idle): Space toggles the
clip, `n` plays the next preview, `x` ends the clip. Otherwise
Space and `n` are main's play/pause and next, as on every room.
`p` (main's previous) ends the clip as a main command (§1.8.8);
`s`, `r`, `A` and `-` / `+` keep their meanings and do not end it,
and the volume reaches the clip.

1.2.7 **Esc** closes the sheet (whose own Esc folds the links list
first), then stows the cursor or the tabs stop, then ends a live
clip, then falls through to the shell.

1.2.8 **Inside the sheet** the room's keys do nothing: the sheet swallows
keys as it does for every song, and its Preview row is the clip's
control there (D12).

1.2.9 **Mouse.**
- A row click, or a right-click on a row (the GUI's context
  click), opens the sheet, or copies (§1.9.1). A slot click
  previews that row.
- The click targets are the tabs, `‹ ›`, `▸ preview all`, `copy
  list`, the band's toggle, `▸▸`, attribution, `[ get it ]` and
  `[X]`, the panes' `↻ new seeds`, `Try again` and `Check again`,
  the header's controls, and every sheet row.
- A click acts and stows the cursor and the tabs stop (Discover
  clause 57). Apple Terminal's release echo is swallowed by the
  shell, as everywhere (docs/ui-kit.md).
- Hover is BRIGHT + BOLD. The only tooltips are `Add` on a `+` tab
  and the header's window and options tips; there is no refresh
  tooltip (Discover's names `G`).
- The wheel over the tabs row scrolls one tab a notch (D23); over
  the grid it scrolls the list.

### §1.3 The room's rhythm and its sizes

1.3.1 **Discover's `layout()`**, with no card and no album strip: the
heads sit right under the verbs line, and the grid starts on the
row after the rule.

| content height | header | tabs | verbs / band | heads | rule | grid |
|---|---|---|---|---|---|---|
| 23 (100×30, hints off) | 2 | 4 | 6 | 7 | 8 | 9–24 (16 rows, blocks of 32) |
| 22 (100×30, hints on) | 2 | 4 | 6 | 7 | 8 | 9–23 |
| 17 (100×24, hints off) | 2 | 4 | 5 | 6 | 7 | 8–18 (11 rows, blocks of 22) |
| 16 (100×24, hints on) | 2 | 3 | 4 | 5 | 6 | 7–17 |

1.3.2 **The content** runs from x 17 to the frame's width less 3: 80
cells at 100 columns, 47 with the queue open. Key hints are off by
default (`[gui] key_hints`), so there is no tips line and the bar
stands on rows 25–29 at 100×30.

1.3.3 **The queue open** (Tab): the room is 47 cells wide, the header
holds its second narrow step (§1.4.2), `›` stands at x 63, the
columns are 21 cells with no ARTIST (§1.7.1), and the band drops
its bar, then `[ get it ]` (§1.8.11). The keys are unchanged; a
queue row played is a main command (§1.8.8).

1.3.4 **The floor** is 100×24, where the nav takes its fourth rise with
hints on (§1.1.4). Below it the GUI is the mini player: the room is
not drawn, so a clip has ended and main is restored; the visit is
kept (§1.1.7).

1.3.5 **The native window** (`gui --window`) draws the same buffer, cell
for cell, with the same keys and mouse. Truecolor is pinned there,
so the swatches are always tinted; copy goes through the
pasteboard; the title is `mStream Player`. The new glyphs (`▌ ◌ ‹
›`) are in Hack, the window's font.

### §1.4 The header, and the batch asks

1.4.1 **Discover's bar**, lent through `taste_bar`. At 80 cells:
`Peer Recommendations` BOLD at x 17; `Last 30 days` (text) at 56–67
and `▾` dim at 69 — the window list, `w`; `more options` dim at
72–83 — the seeds modal, `e`; `↻ new seeds` accent at 86–96, which
reads `↻ asking…` gold while any ask of the room is out.

1.4.2 **Narrow steps**, in order: the refresh's words go (its glyph
stays); the name becomes `Peer recs` (the nav's word); `more
options` becomes `options`; the name drops. At 47 cells (the queue
open) step 2 holds: `Peer recs` 17–25, `Last 30 days` 31–42, `▾` 44,
`more options` 47–58, `↻` 61.

1.4.3 **No mood opener**, and no `moods` key is ever sent: a `moods` body
nulls the answer's `network`. The window list and the seeds modal
are Discover's, unchanged (their `gui.discover.*` keys and tips).

1.4.4 **R1 — a new batch.** The body:

```json
{ "window": 30,
  "pin": ["<user pin>", "…"],
  "exclude": ["<dropped seed hash>", "<passed random pick hash>", "…"],
  "sources": ["network"],
  "newArtistsOnly": false,
  "limit": { "seeds": 12, "network": 32 },
  "networkOnRandomSeed": true,
  "networkPeers": ["<64 lower-case hex>"] }
```

- `window` is always sent: 7, 30, 90 or 0 (default 30).
- `pin` holds only the user's pins from the seeds modal, at most 8
  (Discover's cap); it is omitted when there are none. **No
  auto-pin of the playing song** (D16).
- `exclude` is the visit's drops plus the passed random picks,
  de-duplicated, the newest 200; omitted when empty.
- `networkPeers` is `[selected]` while a peer tab is selected,
  lower-cased, 64 hex only, and the key is omitted for All. `[]` is
  never sent.
- `ignoreVPaths` and `moods` are never sent.
- **The golden body at open** (All, nothing pinned or dropped,
  window 30):
  `{"window":30,"sources":["network"],"newArtistsOnly":false,"limit":{"seeds":12,"network":32},"networkOnRandomSeed":true}`.
- R1 goes out on a fresh visit, New seeds, a window change and a
  seed edit. Sending it drops the batch and every kept tab answer;
  the answer that comes back is the new batch.

1.4.5 **New seeds** (`R`, `↻ new seeds`, the Nothing new pane's `↻ new
seeds`, the nav row while the room is up): R1, keeping the selected
tab's `networkPeers`. When the batch on screen came from a random
seed (`seedSource` random, or `fallback`), its pick's hash joins
the passed picks first, so it is in `exclude`.

1.4.6 **The window** (`w`, or the header's `Last 30 days ▾`): R1 with the
new window, keeping the tab, and the window is written to `[gui]
discover_window`. A failed ask keeps the new window, as Discover's
does. The room reads the setting on every enter; Discover's window
in memory is not live-synced (D15).

1.4.7 **Seed edits** in the seeds modal (drop or restore a seed, pin or
unpin, Pin the playing song, a landed Pin a song…) ask R1 when the
modal closes, keeping the tab. The modal lists **the batch's**
seeds, never the shown tab's: every tab is pinned to that batch.

1.4.8 **No pick left.** An unpinned ask with passed picks whose answer
has no seed and is random or not analyzed clears the passed picks
and asks once more, the old rows standing (Discover's rule).

1.4.9 **The window list and the seeds modal end a live clip** when they
open: both cover row 6, where the band stands (§1.8.10).

1.4.10 **The last ask wins.** Every ask takes a new seq; an answer whose
seq is not the newest, or whose session generation or visit has
passed, is dropped. A re-ask keeps the old rows drawn until the new
ones land, the header reading `↻ asking…`.

### §1.5 The peer tabs, the poll and Adds

1.5.1 **Items, in the server's order:** ` All ` (drawn by the room), then
the shelf rows (`onShelf`: online first, then by label), then —
only when the answer says `canAdd` — the addable rows. A shelf row
ignores any `add` stamped on it. Labels are drawn verbatim
(`mStream · 3f9a`). Every state is keyed on `endpointId`, which
must be exactly 64 lower-case hex; a bad id or a repeated one drops
its row.

1.5.2 **Look.** Each tab is ` {label} `, with one cell of padding either
side and 2 cells between tabs (Discover's `TAB_GAP`).

| item | when | drawn | Enter or click |
|---|---|---|---|
| All | always | ` All ` dim; the slab when selected | select (R2) |
| shelf, online | `online` | ` • {label} `: `•` ok, the label text | select (R2) |
| shelf, offline | not `online` | ` {label} ` dim | select (R2) |
| addable | no add state | ` + {label} ` accent | POST the Add |
| adding | the server's `adding` or `done`, or a local press | ` {label} adding… ` gold, inert | — |
| failed, `no-room` | server or local | ` {label} no room ` dim, inert | — |
| failed, any other code | server or local | ` {label} try again `: the label gold, `try again` accent BOLD | POST the Add again |
| before the first answer | no answer yet | `░` runs of 5, 16 and 15 cells at x 17, 24 and 42, dim | — |

**All alone is never drawn** (a member with an empty shelf, or a
404 route): the row stays blank, so the grid never jumps.

1.5.3 **Selection.** The selected tab is the slab (on-accent on accent,
BOLD) across its padded width. Only All and the shelf tabs are ever
selected. While the tabs stop holds another tab, the selected one
reads accent BOLD (its `•` stays ok); with the stop on the selected
tab, the strip reads as at rest. The held tab is the slab whatever
its kind.

1.5.4 **Overflow.** The tabs scroll by **whole tabs**: a tab that does not
fit whole is not drawn.
- `‹` accent at x 17 when tabs are hidden to the left (the first
  visible tab then starts at x 19); `›` accent at x 96 (x 63 with
  the queue open) when tabs are hidden to the right; tabs end by
  x 94. Each is drawn only when there is more that way.
- Each `‹ ›` click scrolls one tab and moves neither the selection
  nor the stop, so a click can scroll the selected tab out of view.
  A key that moves the selection or the stop scrolls that tab into
  view.
- A single label wider than the row is clipped with `…` and drawn
  alone: the one exception to "never cut a tab" (D23).
- Widths are cells (`kit::width`), never characters.
- The stop, the tab it last held and the strip's first visible tab
  are held by endpoint id, never by index, so a poll that reorders
  the rows moves none of them (D30).

1.5.5 **R2 — a tab** (and Try again, R4 and R5) re-asks on the batch on
screen:
- `pin` is every `seeds[].filepath` of the batch's first answer, in
  order, non-empty, de-duplicated, at most 12 — **the random pick
  included** (the page walks the network on it). The user's own
  pins are replaced by these.
- `limit.seeds` is the pin count, so the server adds no seed and
  the seeds modal stays true.
- `networkPeers` is `[id]` for a peer tab and absent for All.
- With no pinnable batch (none yet, the batch failed or was
  refused, or no seed has a path) R1 goes instead.
- **The golden body** (the Kitchen Radio tab on a history batch of
  two seeds):
  `{"window":30,"pin":["<s1>","<s2>"],"sources":["network"],"newArtistsOnly":false,"limit":{"seeds":2,"network":32},"networkOnRandomSeed":true,"networkPeers":["<kitchen id>"]}`.
- **A kept answer.** A tab whose answer is kept for this batch draws
  with no call, and the seq is bumped so an ask still out for
  another tab is dropped.

1.5.6 **Debounce (R8).** A stowed `← →` moves the selection at once (the
slab moves and scrolls into view), and the ask goes out 250 ms after
the last step; a kept answer draws at once with no call. Clicks,
and Enter on the tabs stop, ask at once.

1.5.7 **The poll (R6)** — `GET …/p2p/peers`, on enter, then every 15 s,
and every 3 s while any add item reads adding or one of this
visit's Add POSTs is out. Failed and no room do not hurry it. There
is one timer; each poll is numbered, and an answer older than the
newest one answered is dropped.
- **Paused** while the room is not drawn, with one poll on the
  first frame back; **stopped** on leave and off.
- **200:** R4 and R5 are worked out against the previous answer,
  the strip is rebuilt, and only the local Add states that still
  stand are kept. Then the room acts: R5 first, else R4 or a
  pending re-ask, unless it is holding (§1.5.9).
- **403:** the poll stops, and R7 re-pings (§1.10.6).
- **404:** the poll stops, and the strip is All alone (not drawn).
  The gate makes this defensive only.
- **Anything else** keeps the last strip and the cadence.
- `shelf.running`, `monogram`, `hue` and `rowCount` are read and
  never drawn. A row's `label` falls back to its trimmed `name`.

1.5.8 **Adds** (admins only: `canAdd` alone decides, never the account's
admin bit).
- Allowed on `+` and try again only, and not while this visit is
  already posting that id; a second press while posting does
  nothing. `adding…` and `no room` are inert.
- **The press:** a local `adding` stamped with the number of polls
  sent so far; the id joins the posted and posting sets; the strip
  redraws; the next poll comes in 3 s.
- **The answer:**

  | answer | outcome | local state until a later poll | next poll |
  |---|---|---|---|
  | 202 | adding | adding | cadence (3 s) |
  | 200 `done` | landed | adding | at once |
  | 200 `adding` | adding | adding | cadence |
  | 200 `failed` + `no-room` | no room | no room | cadence |
  | 200 `failed`, any other code or none | failed | try again | cadence |
  | 404 / 409 | gone | dropped | at once |
  | 403 / 405 | not allowed | dropped | at once |
  | anything else (400, 401, 5xx, no answer, an odd 200) | failed | try again | cadence |

- A local state stands until a poll **sent after it** answers; then
  the server's state rules. Local states are dropped for rows that
  are no longer addable (on the shelf, gone, or `canAdd` false).
- The posted set keeps an id only for adding and landed: only an
  Add still on its way can land.
- The server's words are never drawn; only its codes are read.
- A restart of the server drops its in-flight Adds; the row reads
  `+` again at the next poll.

1.5.9 **R4 — the shelf moved.** On each poll answer:
- **Landed:** an id this visit posted is now `onShelf` (or its
  `add.state` is `done`) and was not in the previous answer.
- **Grew under No peers yet:** the pane reads No peers yet and
  `shelf.searchable` went from 0 to more.
- **Emptied:** `shelf.searchable` went from more than 0 to 0.
- On any of the three, every kept tab answer is dropped and the
  current tab is re-asked with R2 (after an emptying, that draws No
  peers yet).
- **Holding:** while a clip is live or the lead sheet is open, the
  re-ask waits, and it runs **at once** when the holding ends (S18).
- An Add by another client, or by the server's rotation, only moves
  the strip. A tabs stop held through a landing stays on its tab.

1.5.10 **R5 — the selected peer is gone:** an answer whose
`network.reason` is `not-on-shelf` while a peer tab is selected, or
a poll in which the selected peer is no longer an `onShelf` row (an
offline peer is still on the shelf). All is selected and its kept
answer drawn with no call (the seq bumped), else R2 for All. The
`not-on-shelf` answer is never kept. R5 does **not** wait for
holding; a sheet whose lead has left the list closes (§1.9.9).

1.5.11 **No room.** After the shelf fills with pinned snapshots, a press
on `+` posts once and reads `no room`, dim and inert; no press
after it posts. The plan (§2 item 6) has the server clear every
stored failure, no-room included, on any shelf or pin change, so
the tab should read `+` again at the next poll after an unpin. The
live walk confirmed it (the log).

1.5.12 **A member** sees All and the shelf only: no `+`, `adding…`, `try
again` or `no room`, and never POSTs. A member with an empty shelf
sees no tabs row at all, and No peers yet.

### §1.6 The verbs line, and copy

1.6.1 **The line** (row 6): `▸ preview all` accent, ` · ` dim, `copy
list` dim, from x 17. Without a preview offer (§1.8.1), `copy list`
alone. Hidden while the tab has no rows (the row stays, and so do
the panes). While a clip is live, the band takes the row (§1.8.11).

1.6.2 **Copy text** is `Artist - Title`, both trimmed: the title alone
when the artist is empty, the artist alone when the title is empty,
nothing when both are.

1.6.3 **Copy list** (`y`, `copy list`) copies the shown tab's leads, all
of them, in **rank order** (not the column-major reading order),
one line each, empty lines dropped, joined by `\n`, with no
trailing newline. An empty list copies nothing and notes nothing.
It is offered without a provider too.

1.6.4 **Through `kit::clipboard::copy`.** `Copied::Clipboard` and
`Copied::Terminal` (OSC 52, which a terminal may ignore) both note
`Copied`; `Copied::Failed` notes `Couldn't copy`. Both notes are
dim at the bar's note (D20).

### §1.7 The grid

1.7.1 **Geometry.** The list is the content less 2 cells: x 17–94, with
the scrollbar lane at x 96, drawn only on overflow. The columns are
`cw = (list − 2) / 2` = **38** cells: column A at x 17–54, a 2-cell
gap, column B at x 57–94. With the queue open they are 21 cells, at
x 17–37 and x 40–60, and ARTIST drops.

1.7.2 **Order.** Column-major, in blocks of 2 × the visible rows: a block
holds `ceil(m / 2)` leads in column A and the rest in column B (the
web's `columnBlocks`). 12 leads read 6 | 6 at every size, and
Kitchen Radio's 5 read 3 | 2. At most 32 leads come
(`limit.network`), one block at 30 rows; at the floor the second
block scrolls with the kit's scrollbar. There is no Show more.

1.7.3 **Heads and rule.** DIM `TRACK` at +2 and `ARTIST` at +19 in each
column (the upper-cased `gui.discover.col_*`). On the row under
them, one dim `─` run spans the whole content, the lane included:
x 17–96 (x 17–63 with the queue open), Discover's table rule.

1.7.4 **A cell** (38 wide): `▌` swatch at +0, blank, TRACK at +2…+16
(15 cells, clipped with `…`), gap, ARTIST at +19…+34 (16, dim), gap,
the play slot at +36…+37. A narrow cell (21): `▌`, blank, TRACK at
+2…+17, gap, slot at +19…+20. With no slot (no preview offer),
ARTIST takes +19…+37.

1.7.5 **Row states.** At rest the title is text and the artist dim. The
slot reads the first that holds:

| state | look |
|---|---|
| the live clip, playing | title accent BOLD, slot `▮▮` accent BOLD |
| the live clip, paused | title accent BOLD, slot `▸` accent BOLD |
| the live clip, resolving, waiting out a 429, or opening | slot `◌` gold |
| no preview this visit (the shared answer says none) | slot `▸` dim, inert |
| otherwise (idle) | slot `▸` accent |

Over any of them, the row **held by the keyboard** is the slab
across the whole cell, the title on-accent BOLD, the swatch keeping
its hue on the accent; **hover** draws the title BRIGHT + BOLD.
The slot reads the room's one clip by **lead key**, so a list that
moves under a live clip moves its marks with the lead (D30). A
forgotten answer (§1.8.7) reads idle again, never muted.

1.7.6 **No job marks**, badges or counts on rows: a lead's job shows in
its sheet only.

### §1.8 Previews and the band

1.8.1 **Offered** only with an audio device (`audio_available`) and a
**provider**: the first plug-in in `GET …/discovery/plugins` order
that has `preview` (enabled, available, declaring it). There is no
fall-through to a second one: a miss is No preview. Without the
offer there is no slot, `l` and `L` do nothing, the verbs line is
`copy list` alone, there is no band, and the sheet has no Preview
row. The retry delay after a 429 is 60 s for `itunes`, 5 s for any
other provider.

1.8.2 **One clip at a time**, owned by the room; the grid and the sheet
drive the same session. It holds the lead (its raw JSON), its key,
its own copy of the list a chain walks, its index there, the chain
flag, a status — idle, resolving, waiting (a 429's wait), playing
or paused; live is any but idle — the preview, its length
(`preview.seconds`, else 30), and a generation fence that every
start, stop and switch bumps. The engine's opening moment draws as
busy.

1.8.3 **A press** (`l`, a slot click, the sheet's Preview, `n`):
- On a muted lead it changes nothing and notes `No preview`.
- On the live lead it toggles: playing pauses, paused resumes, and
  resolving or waiting stops the session. The status changes only
  when the engine took the pause or the resume.
- On another lead it **switches**: the old clip is silenced at once
  (main stays paused), then the new lead starts. The list is the
  selected tab's leads in rank order, and the index is the lead's
  place in it.
- **Start:** the status is resolving (`◌`, no bar, no attribution).
  A stored answer plays with no resolve; a stored none settles
  none; else the room asks (§1.8.4).

1.8.4 **Resolve pacing** (the plan's §5.5; D19).
- Asked only on a press; Preview all counts as the press. The body
  is `{recommendation: <the raw lead>}`, unchanged.
- One resolve in flight for the room, shared with the prefetch. A
  press made meanwhile is queued, a newer press replaces the queued
  one, and the queued press goes 300 ms after the flight ends, only
  if it is still current. The store is checked again before it
  goes.
- A 200 is stored for (lead key, provider), found or none, even when
  its press is no longer current; one answer per lead for the
  visit, at most 200 leads, the oldest dropped first. A preview
  counts as found only with a non-empty http(s) `url`.
- If still current: found plays (§1.8.5); `preview: null` settles
  none.
- A 429 on the first attempt: waiting (`◌`), the retry delay, then
  one more ask. A second 429, a transport failure, a timeout or any
  other status settles failed, for this press only: nothing is
  stored, and the next press asks again.
- A 403 clears the store, ends the session and re-pings (R7,
  §1.10.6). A 404 (the plug-in went) settles failed and reads the
  plug-in gate again.
- **A main command while a press only resolves or waits cancels
  that press**: nothing plays later. The player's own advance is
  not a main command for this rule (D19): a track's end, a
  crossfade's handover, a row that would not open walked past or
  tried again, and an Auto DJ top-up leave the press to play. An
  Auto DJ opener ("Surprise me" landing) is the listener's start
  and still cancels.

1.8.5 **Playing.**
- A found answer sends the clip to the engine with its URL
  verbatim — http(s) only, never through the stream-URL builder,
  never with a token. The status is playing once the engine says
  the clip sounds; the band then shows the bar and the attribution.
- **None:** the store says none (a muted slot, No preview in the
  sheet). A single press ends the session and notes `No preview`;
  a chain skips to the next lead.
- **Failed:** a single press ends the session and notes `No
  preview`; a chain ends there, quietly, rather than run into the
  provider's limiter.
- **Run-out:** a single clip that plays out ends the session, and
  main is restored at once; in a chain the next lead starts. A clip
  that played out keeps its stored answer.

1.8.6 **Preview all** (`L`, `▸ preview all`).
- Pressed while a chain is live, it ends the session.
- Otherwise (idle, or a single clip live, which it replaces) it
  starts a chain on the selected tab's leads from the top, at the
  first lead the store does not say none for; with none left, it
  does nothing.
- A run-out plays the next lead after the index that is not known
  none; none skips; failed ends the chain; the list's end ends the
  session, with main restored.
- **One look-ahead:** once the live clip passes 20 s, at most once
  per clip, the next lead (skipping known none) is resolved, but
  only when it has no stored answer, no resolve is in flight, none
  is queued and no 429 wait runs. It stores a 200, found or none,
  whatever the generation, never plays, never retries, and stores
  nothing on a 429, 403, 404 or transport failure. A press made
  while it is out queues behind it.
- **The list changes** (a tab, New seeds, a window or seed edit, an
  R4 or R5 re-ask drawn): the chain flag drops, the live clip plays
  out, and the session ends there.
- No pressed state is drawn: while a chain plays, the band holds
  the row.

1.8.7 **The shared answers, and the dead-clip rule.**
- One store per visit, keyed by (lead key, provider): found (the
  preview) or none, never busy or failed. The **lead key** is
  `endpointId + "\0" + exportId` (a string, or a finite number's
  decimal), else `endpointId + "\0" + nameKey(artist) + "|" +
  nameKey(title) + "|" + nameKey(album)`, with the player's
  `name_key` (the server's fold, byte for byte).
- The grid's slots, the sheet's Preview row and Preview all's skip
  test all read it, so a lead answered once is never asked again in
  the visit, across tabs.
- Cleared on a fresh visit, a provider change and a resolve 403;
  not on a tab change, New seeds or a window change.
- **A dead clip** (the engine's `Failed`: it would not open, the
  HTTP answer failed, it would not decode, it broke off part way,
  or it stalled) is No preview for that press: a single press ends
  and notes `No preview`, and Preview all ends the chain there, as
  after a 429. Its stored answer is **forgotten only while the store
  still holds that same URL** for the lead; a newer answer stored
  since stays. The slot and the sheet's Preview row read **idle**
  again, not muted, and the next press asks the plug-in again (the
  server's 10-minute cache may answer the same URL; that costs one
  cached resolve). A sheet opened later on the lead does not start
  from the dead URL. A clip that played out stays kept.
- **A refused clip** (the engine's `Refused`: no output device, not
  an http(s) URL, a backend with no clip player) is not the clip's
  fault: it is No preview for that press, and its answer stays
  (S29).

1.8.8 **Main under a clip** (the engine's aside, D10).
- The clip is a second player on the engine's own bus, so it
  follows every device move. Its open runs off the audio thread;
  the audio thread never waits on a CDN.
- Main is **paused in place when the session's first clip opens**,
  and only if main was audibly playing. While a press only resolves
  or waits, main plays on. Hand-offs (a switch, a chain step, `n`)
  keep main paused: the promise carries across clips.
- **Main always wins.** Every main transport command ends the clip
  first, as taken, and main is not resumed by the clip: play,
  pause, resume, stop, seek, next, previous, a queue row played, the
  queue cleared, and the branches of queue add and queue remove that
  start playback. A seek leaves main paused at its new place. In the room
  that is the bar's play frame, its seek line, `◂◂` and `▸▸`, `p`,
  and a queue row's play; Space and `n` are the clip's while it is
  live. The player's own advance (§1.8.4) is not one of them for a
  press still finding its clip; a clip the engine already opens or
  plays still ends as taken when main advances under it (D10).
- **Restore:** when the session ends any other way, main resumes in
  place, once, only if the session paused it and it is still that
  track, still paused. A single press restores at once on a
  run-out. Preview all holds main paused for 5 s after a run-out
  for the next clip, and the room stretches that hold at every
  step it waits on (each send, each 429 wait), up to 90 s, so a
  60 s iTunes wait never lets main slip back in (S28).
- **Never a play:** no scrobble, no play session, never queued,
  never now-playing, never in the player's status, and never fed to
  the visualizer. The volume reaches it. The device does not sleep
  while a clip opens, sounds or holds; a paused clip lets it sleep
  as a pause does.

1.8.9 **Server audio** (`gui --serve-port`). While a clip session is
live, a remote `/pause` pauses the engine unconditionally (today it
is dropped when main reads paused, which main always does under a
clip; D11). A remote play or seek ends the clip once, with no
double audio. The headless `mstream-player serve` never starts a
clip.

1.8.10 **What ends a clip.**
1. `x`, the band's `[X]`, the last Esc of the chain, or Space, `l`,
   a slot click or the sheet's Preview while it is busy.
2. A single clip playing out; a chain at its end or at a failed
   lead; `n` or `▸▸` at the list's end (S19).
3. A none or failed single press (with the note `No preview`).
4. A main command (§1.8.8), with no resume.
5. The room not drawn or left (another screen or room, the mini
   player, the Files room under a pick), a server switch, quit, or
   off: main is restored.
6. The window list (`w`) or the seeds modal (`e`) opening: both
   cover row 6 (S16).
7. A resolve 403 (R7); a provider that has gone at a gate re-read.
8. A remote play under server audio.

**What does not end it:** the lead sheet opening or closing (row 6
stays visible above the sheet at every size; S14); a tab change or
New seeds (the clip plays out, and a chain ends after it); the
queue panel opening (the band reflows); an attribution click (the
GUI stays drawn).

1.8.11 **The band** — row 6 while a clip is live (row 5 at 100×24 with
hints off, row 4 with them on), in the verbs line's place.
- **The head:** the toggle in a 2-cell slot at x 17 — `▮▮` accent
  BOLD (playing), `▸` accent BOLD (paused) or `◌` gold (resolving);
  `▸▸` dim at x 20, the next preview (`n`); then the title (text),
  ` · ` and the artist (dim) from x 23.
- **The right cluster**, ending at the content's last cell (x 96,
  or x 63 with the queue open): the clip bar — 10 cells, `▰` ok
  played and `▱` dim to come, over the clip's length (30 s when
  unknown) — then 2 cells; the **attribution**, verbatim from
  `preview.attribution`, dim and not underlined, a text button that
  opens `preview.link` only when it is https, then 1 cell; **`[ get
  it ]`** dim, only with the Get it gate (§1.9.5), which opens the
  lead's sheet with the cursor on Get it, then 1 cell; **`[X]`**
  dim, which ends the clip. The two bracketed buttons are BRIGHT +
  BOLD under the pointer.
- **Before a resolve answers** (`◌`) there is no bar and no
  attribution.
- **Fit.** The text runs from x 23 to 2 cells before the cluster,
  clipped with the kit's `clip` (the cells that fit less one, then
  `…`). When the clip would keep fewer than 4 cells of the artist,
  ` · artist` drops whole and the title stands alone (clipped only
  if it still overflows). When the text would get fewer than 16
  cells, the parts drop in this order: the bar, then `[ get it ]`,
  then the text clips down to 8 cells, then `▸▸`. The attribution
  and `[X]` never drop. At 80 cells with Deezer the whole band
  stands with 26 cells of text (`Signal Fires · Paper Lant…`); with
  iTunes' longer attribution the bar stays with 18 cells and the
  artist drops; at 47 cells the bar and `[ get it ]` drop, leaving
  16 cells, and the artist drops.
- **The legacy console** draws `||`, `>>` and `~` for the glyphs,
  and the words `NN%` for the bar.
- The band ends with the session; the verbs line comes back (hidden
  if the tab has no rows). It draws from the session alone, so it
  still draws when the clip's lead has left the shown list.

### §1.9 The lead sheet

1.9.1 **Opened by** Enter or `m` on a held row, a row click, a right-click
on a row, and the band's `[ get it ]` (with the cursor on Get it).
Opening never plays. **With no plug-ins at all** (the ping's
`discoveryPlugins` false) no sheet opens: Enter or a click copies
`Artist - Title` and notes `Copied` (or `Couldn't copy`).

1.9.2 **The frame is the track-actions sheet's** (track-actions clauses
23–31), in its lead form: `66.min(w − 2).max(44)` wide and 12 tall
with four rows, centred on the whole frame — x 17–82 and y 9–20 at
100×30, y 6–17 at 100×24 — with a rounded accent border, its inner
cells cleared to the ground, and `[X]` dim at x 79–81 on the first
inner row. The room beneath is inert and drawn without the pointer
(D24).

1.9.3 **The head** has no cover: the title BOLD at x 19, inner row 1; the
byline `Artist · Album` dim (either half may be missing); the
length alone, dim (a lead has no format); the badge row `from {peer
label}` dim, then 3 cells, then `•` ok and `online` dim **only while
the latest poll says that peer is online**; a blank row. A lead is
never "own": no stars are drawn, `← →` and `0`–`5` send nothing, and
there is no match line.

1.9.4 **The rows**, from inner row 6 at x 19, each hidden when its gate
fails, never greyed:
1. **Preview** — with a preview offer (§1.8.1). Its glyph at x 19
   follows the room's clip for this lead (`▸` accent idle, `▮▮`
   accent BOLD live, `▸` accent BOLD paused, `◌` gold busy), the
   word `Preview` at x 22. When the shared answer says none, it
   reads `▸` dim and `No preview` dim, inert. Enter: idle starts
   this lead's clip as `l` does (the same session and band; `n`
   walks the tab's list from this lead); live pauses or resumes;
   busy stops.
2. **Get it** — one row per acquire plug-in other than
   `federation-copy` (§1.9.5).
3. **Open elsewhere…** — the links plug-in (§1.9.6).
4. **Copy title** — always (§1.9.7).

Absent: Play now, Add next, Add to end, Add to playlist, Song info
and Keep a copy…, because a lead has no file.

1.9.5 **Get it.**
- The gate: `jobs.allowed`, an acquire plug-in other than
  `federation-copy`, and a non-null destination from `GET
  …/collection/destination` (read with the plug-in gate). Accent
  at rest, as Keep a copy… is; no ellipsis, because the row acts
  at once. With several plug-ins each row reads `Get it`, the
  plug-in's title dim 2 cells after it.
- Enter starts the job at once: `POST …/plugins/{name}/jobs
  {recommendation: <the raw lead>, destination: <the one shown>}`.
  A 409 `destination_changed` reads the destination again and puts
  the row back to idle, hiding it if the destination is now null.
- **The job word** stands at the row's right, ending at x 80, in its
  mark's token: `downloading…` accent (running), `◷ waiting` dim,
  `✓ kept` ok, `! failed` gold, and the marks' other words. It
  comes from one `lookup/batch` row when the sheet opens, then
  `GET …/plugin-jobs/{id}` every 1.5 s while the sheet is open and
  the job is live. It is not drawn while Open elsewhere's list
  hangs (the list would cut it).
- While the job is queued or running, Enter starts nothing more.
  After `kept` the row is inert; after `failed`, `stopped`,
  `cancelled` or `refused`, Enter starts again.
- Closing the sheet leaves the job running; rows never show it,
  and reopening looks it up again.

1.9.6 **Open elsewhere…** is the links plug-in's row and its list, as
they are for a connected server's row: Keep's links list, fed the
raw lead, with no network call of the room's own. The list hangs at
x 47–78; while it hangs, the row's label wears accent BOLD and the
list holds the slab, and the list's tips are its own. A pick opens
the link through the kit's opener, and the note says so in the links
list's own words (`gui.discover.links.*`).

1.9.7 **Copy title** copies `Artist - Title` (§1.6.2), closes the sheet
and notes `Copied` or `Couldn't copy`.

1.9.8 **The cursor** is the slab across the inner width (x 18–81), BOLD.
It starts on Get it, else Open elsewhere…, else Copy title — never on
Preview, so opening the sheet and pressing Enter never plays — and
`↑ ↓` skip a muted Preview. Esc or a click outside closes the sheet, and the grid's
cursor comes back to its row. The tips are `gui.tips.discover_sheet`.

1.9.9 **The sheet holds its lead by key** (D30). On every draw, key and
act it finds the lead again: at another index, the sheet follows it
(and so does the grid's cursor when the sheet closes); gone from the
list, the sheet closes. While the sheet is up, R4 re-asks wait
(holding), and the poll goes on.

### §1.10 The panes, and reading an answer

1.10.1 **Reading a taste answer**, in order:
1. a 403 → R7, from the taste ask;
2. a 200 whose seed is not random and whose `network` is null → R7,
   from the null network, on any tab (D18);
3. `reason` `not-on-shelf` with a peer tab selected → R5 (§1.5.10),
   never kept;
4. otherwise, the first answer of a batch becomes the batch, and the
   answer is kept for its tab — **unless** its `reason` is `error`,
   which is shown and never kept (a tab chosen again asks again);
5. no answer at all (a transport failure, a 5xx): the Try again
   pane, nothing kept.

Drawing a new answer while Preview all runs drops the chain flag
(§1.8.6). `network.reason` is read leniently: an unknown word is no
reason.

1.10.2 **The pane**, the first that holds:
1. **Scan pending** — the room's scan latch (a taste 403 whose
   re-ping said on), `notAnalyzed`, or `reason` `scan-pending`;
2. **Loading** — no answer yet, an ask out;
3. **Try again** — no answer (the ask failed), a `network` of null,
   or `reason` `error`;
4. **Ready** — the answer has leads: leads win over any reason;
5. **No peers yet** — `reason` `empty-shelf` or `no-compatible`, or
   the last poll's `shelf.searchable` is 0;
6. **Nothing new** — otherwise (`nothing-new`, or no reason).

1.10.3 **Drawn** in the list rect, at x 17 on the grid's first row, under
the heads and rule, with the strip above every pane and the verbs
line hidden:

| pane | drawn |
|---|---|
| Loading | the header's `↻ asking…` gold; the tabs skeleton; 4 `░` rows per column, TRACK runs of 9, 6, 10 and 7 cells at +2 and ARTIST runs of 11, 8, 9 and 12 at +19 |
| No peers yet | `No peers yet` dim |
| Nothing new | `Nothing new` dim, 2 cells, `↻ new seeds` accent BOLD (New seeds) |
| Try again | `Try again` accent BOLD, alone |
| Scan pending | `gui.discover.scan_pending` dim, wrapped at 78 cells, then `Check again` accent BOLD on the next row |

1.10.4 **Try again** asks R2 on the batch, else R1. **Check again** does
what Discover's does: on the 403 latch it re-pings, and otherwise it
asks again keeping the tab (D17).

1.10.5 **Never on this page:** Discover's "Nothing finished yet" card (a
random seed still walks the network, so a random answer is ready or
one of the network's own empty states), its off and not-asked panes,
its "couldn't reach this source" line, and the webapp's admin link
to its Enrichment settings.

1.10.6 **R7 — off.**
- **Triggers:** a taste 403, a peers 403 (§1.5.7), a resolve 403
  (§1.8.4), and a taste 200 whose seed is not random and whose
  `network` is null.
- **One re-ping at a time** (the door's `Probe`, answered `Probed {
  on }`); a second trigger while one is out sends no ping but still
  takes its own outcome now: a taste 403 holds the asks under Scan
  pending, and a null network draws Try again. The ping already out
  answers for both: off wins, and on turns a held Scan pending into
  the latched one. A re-ping that gets no answer counts as **off**.
- **Off** (the re-ping failed, or `peer_recs()` is false after it):
  the latch is set until the next `Connected`, the row hides, `R`
  goes dead, the poll stops, a live clip ends, and the room yields
  to Files (§1.1.6).
- **Still on**, by source: a taste 403 draws Scan pending (D17); a
  null network draws Try again; a peers 403 leaves the room as it
  is with the poll stopped; a resolve 403 does nothing more (the
  store was already cleared).

### §1.11 The bar under the room

1.11.1 **The player bar is the shell's**, unchanged. While a clip plays,
main's play frame reads `▸` and the seek line holds main's second;
a click on either is a main command (§1.8.8).

1.11.2 **The note** sits at x 24 on the bar's bottom row, dim: `Copied`,
`Couldn't copy`, `No preview`, and the links opener's words. The
shell's gold error note is not used by the room (D20).

1.11.3 **The boards' sample state**, which the tests reproduce: main
plays Glass Hours · Color Out (album Low Light, 3:52, at 1:24, `MP3
· 320 kbps · 44.1 kHz`), the volume at 80%, the server label
`http://127.0.0.1:3995`, and the stage's peers and twelve leads
(`test/fixtures/peer_recs/`).

### §1.12 Swatches, art and privacy

1.12.1 **A lead's tile is the 1-cell `▌` swatch** in `hsl(tile.hue, 45%,
60%)` of the server-stamped hue, with no letter. Without truecolor
(the 256- and 16-colour tiers), or for a lead with no tile, it is
drawn DIM. On the keyboard's slab the swatch keeps its hue on the
accent ground.

1.12.2 **Peers** get no monogram and no hue: labels only.

1.12.3 **No art, ever:** no lead cover, no earned art, no Cover Art
Archive, the native window included. The player's preview type has
no artwork field, so `preview.artwork` is never even kept.

1.12.4 **Privacy.** A third party is reached only on a press: the
server's resolve, then the clip URL, fetched straight from the
provider's host, verbatim, with no token. The network walk on a
random seed is the server's own.

## Wording

### §1.13 Strings

**New keys** (English below; all ten locales; nav and tab labels at most
12 cells; tips at most 99 cells):

| key | en | where |
|---|---|---|
| `gui.nav.peer_recs` | Peer recs | the nav row; the header's narrow name |
| `gui.peer_recs.name` | Peer Recommendations | the header |
| `gui.peer_recs.all` | All | the first tab |
| `gui.peer_recs.tab_adding` | %{peer} adding… | a gold, inert tab |
| `gui.peer_recs.tab_no_room` | %{peer} no room | a dim, inert tab |
| `gui.peer_recs.tab_try_again` | %{peer} try again | split at the placeholder: the label gold, the words accent BOLD, so a locale may put the label anywhere |
| `gui.peer_recs.add` | Add | the `+` tab's tooltip |
| `gui.peer_recs.no_peers` | No peers yet | the pane |
| `gui.peer_recs.nothing_new` | Nothing new | the pane, before `↻ new seeds` |
| `gui.peer_recs.preview` | Preview | the sheet's Preview row |
| `gui.peer_recs.no_preview` | No preview | the sheet's muted Preview row; the bar's note |
| `gui.peer_recs.preview_all` | preview all | the verbs line, after `▸ ` |
| `gui.peer_recs.copy_list` | copy list | the verbs line |
| `gui.peer_recs.copied` | Copied | the note after `y`, Copy title, a plug-in-less Enter |
| `gui.peer_recs.copy_failed` | Couldn't copy | the note when nothing took the copy |
| `gui.peer_recs.get_it` | Get it | the sheet's row |
| `gui.peer_recs.get_it_band` | get it | the band's `[ get it ]` |
| `gui.peer_recs.downloading` | downloading… | Get it's running job |
| `gui.peer_recs.copy` | Copy title | the sheet's last row |
| `gui.tips.peer_recs` | ↓ pick a row · ← → peers · w window · e more options · R new seeds | stowed, the tab has rows |
| `gui.tips.peer_recs_empty` | ↓ pick a peer · ← → peers · w window · e more options · R new seeds | stowed, the tab has no rows |
| `gui.tips.peer_recs_tabs` | ← → peers · Enter choose · Esc done | the tabs stop held |
| `gui.tips.peer_recs_row` | Enter open · l preview · L preview all · y copy list · Esc done | a row held, with a preview offer |
| `gui.tips.peer_recs_row_plain` | Enter open · y copy list · Esc done | a row held, with no preview offer (D21) |
| `gui.tips.peer_recs_clip` | Space play/pause · n next · x end · Esc done | a clip live, nothing held |

**Tips precedence**, the first that holds (hints on only): the sheet
(`gui.tips.discover_sheet`, or the links list's own while it hangs) · the
window list or the seeds modal (`gui.tips.discover_menu`,
`gui.tips.discover_seeds`) · the tabs stop held · a row held · a clip live
· stowed, with rows or without (D21).

**Reused keys.**
- The header: `gui.discover.window_days` "Last %{days} days",
  `window_all`, `more_options`, `options_short`, `new_seeds`, `asking`,
  and Discover's window and options tooltips.
- The heads: `gui.discover.col_track`, `col_artist`, drawn upper-cased.
- The panes: `gui.discover.retry` "Try again", `scan_pending`,
  `check_again`.
- The sheet: `gui.act.from_server` "from %{server}", `gui.act.online`,
  `gui.act.open_elsewhere` "Open elsewhere…", `gui.discover.links.*`,
  and the job marks' words `gui.discover.mark.*`.
- The seeds modal's and the window list's keys, as they are.

**Never keys:** peer labels, attributions, lead text, monograms. The
glyphs live in code, beside the words (docs/ui-kit.md, Glyphs).

## Out of scope here

- **The mini player** (below 100×24): the room is not drawn there, so a
  clip has ended; the mini player itself is unchanged.
- **Previews in the browser build**: its audio backend refuses a clip
  (the room then offers nothing it cannot do).
- **The visualizer tapping the clip**: the scope shows main, paused.
- **Casting**: the player has none.
- **The webapp's admin Turn on form** and its Setting up bar: p2p off
  hides the row here.
- **Earned art, lead covers, monograms, the match line, a tile per
  provider and the admin's Invite row** of the web's window: the terminal
  shows no percentages and fetches no art, and has no invite precedent.
- **More than 32 leads**: `limit.network` is 32, one block at 30 rows.
- **The 256- and 16-colour tiers, light themes and the legacy console**
  are drawn by rule (the swatch DIM; the glyphs' stand-ins in
  docs/ui-kit.md, Glyphs), not on boards.
- **`R` in the base tips line** (its budget), and **live window-memory
  sync** between this room and Discover (D27).
- **Discover's "Nothing finished yet" card** (never on this page).

## Translation notes (terminal GUI)

| Record (plan · web · phone) | Here |
|---|---|
| Web: the sidenav row under Discover; phone: the LISTEN card after Discover | `NavId::PeerRecs`, `NAV[16]`, TOOLS row 18 under Discover; `R`; `peer_recs::gate` in `nav_shown`; the fourth rise in `draw_nav` (§1.1.4) |
| T1/T3 of the plan: `TastePart::Network` on Discover's `TasteRequest` | Its own body type, `NetworkTasteRequest` (`src/api/types/peer_recs.rs`), sent through the room's door: Discover's types, golden bodies and matches stay untouched, and a room failure never becomes `Event::Error` (which would clear Discover's waits) |
| The web's and phone's per-call fetches | One door: `ApiCmd::PeerRecs { generation, cmd: PeerRecsCmd }` → `Event::PeerRecs { generation, event }`, with `Taste`, `Peers`, `Add`, `Gate`, `Preview`, `Lookup`, `Job`, `Start` and `Probe`, shaped once by `peer_recs()` in `src/api/peer_recs_door.rs` for the native and the browser worker |
| The re-ping (web `m.js`, phone D8) | The door's `Probe`, answered `Probed { on }`; a failed ping is off (Discover's probe would turn it into `Event::Error`) |
| The web's `PEERRECS` and `discovery-page.js` builders; the phone's `peer_recs_rules.dart` | Pure functions in `tui::app::taste::peer_recs::rules`: `r1_body`, `batch_pins`, `r2_body`, `read_answer`, `pane_of`, `strip_items`, `local_stands`, `add_outcome`, `landed`, `emptied`, `lead_key`, `copy_text`, `copy_list` |
| The phone's `peer_recs_controller.dart`; the web's `peerRecs*` | The App room, `tui::app::taste::peer_recs` (re-exported as `tui::app::peer_recs`), a child of Discover's taste module: its own `TasteState` holds the visit's steering and the batch (`taste.head`), so the seeds modal reads it as it reads Discover's, and Discover's state is never written |
| The web's `clip*` / `aside*`; the phone's `peer_recs_previews.dart`, `preview_answers.dart` | `tui::app::taste::peer_recs::previews`: the store, the pacing, the session, the hold, the dead-clip rule, and the facts the slot (`Slot`) and the band (`BandFacts`) draw |
| The web's aside `<audio>`; the phone's second `just_audio` player | The engine's aside (`src/engine/aside.rs`): `AudioCmd::AsidePlay { ticket, url, then: Restore \| Hold }`, `AsidePause`, `AsideResume`, `AsideStop { ticket, restore }`, `AsideHold { span }`; `Event::Aside(status)` each tick and one `Event::AsideEnded` per clip, whose end is `RanOut`, `Stopped`, `Superseded`, `Taken`, `Failed` or `Refused` — never `PlaybackFailed` or an error |
| The web's now-playing recommendation window; the phone's #237 sheet | The track-actions sheet's lead form (`actions::open_lead`; track-actions clauses 23–31) |
| The web's "Find elsewhere" | Keep's links list (`App::links_open_rec`, `draw_links`): Open elsewhere…, the terminal's word |
| Discover's config bar on the web and the phone | Discover's header, window list and seeds modal, lent through `gui::discover::taste_bar` (`BarRoom`), with a short-name step and no mood opener; Discover's drawing and steps unchanged |
| The web's strip; the phone's sideways strip | `gui::peer_recs::tabs`: whole-tab scrolling with `‹ ›`, the tabs stop held by endpoint id |
| The web's `columnBlocks` (blocks of 16, 8 \| 8) | `gui::peer_recs::grid`: blocks of 2 × the visible rows |
| The web's sticky band; the phone's band above the mini player | `gui::peer_recs::band`: row 6, in the verbs line's place |
| Toasts | The bar's note, dim |

### What the tests pin

The bodies as JSON values: R1 at open, R2 on a tab and on All, New seeds
on a random batch, and Discover's bodies with no generation-3 key. The
peers answer, Adds in every outcome, the poll's cadence and its fences,
R4, R5 and R7 with every source, the panes' order, the strip's states and
its whole-tab scroll, the grid's blocks and walks, the band's drop order,
the lead sheet's rows, cursor, badge and job word, the store, the pacing
and the dead-clip rule, the engine's aside with main always winning, and
the room's frames against the boards (`kit::boards`), through the App's
own event path from the fixtures. On a pty, e2e scenarios R (an admin, in
Apple Terminal's dialect) and R2 (a member) check the bodies the stage
cannot log, no art request, and no Discover body with a generation-3 key.

## Deviations log

### §1.14 Changes from the approved board (B + C · Terminal), 2026-11-20

1. **The nav** is re-placed against today's sixteen-room NAV: Peer recs is
   TOOLS row 18, under Discover and above Add torrent and Add YouTube,
   with Rated in LIBRARY, LIBRARY on row 5 and TOOLS on row 14. The
   100×24 hints-on floor takes the rise-4 rule (§1.1.4).
2. **The lead window** is the track-actions sheet's lead form (66×12,
   centred on the frame), not the 46-wide keep-window modal (§1.9).
3. **Hints off** on every frame but the floor's, because off is the
   default. There is no `↵`.
4. **The rhythm** is Discover's: heads right under the verbs line, the
   grid from row 9.
5. **Columns** are 38 cells, not 39, leaving the scrollbar lane free.
   ARTIST stays at +19.
6. **The online dot** is `•`, not `●`.
7. **Failed tabs** read `{label} try again` in gold and accent, not `!
   {label}` in danger (danger is reserved).
8. **No room** is drawn (ADD STATES).
9. **Label spacing:** `mStream · 3f9a` keeps its spaces.
10. **Attic Tapes** is on the strip, as the stage's shelf holds it.
11. **Computed hues:** the swatches use the server's tile hues, not the
    board's samples (reds included; the plan's Gate W).
12. **The band** gains `▸▸`, closes with `[X]` (Hack has no `✕`), writes
    `[get it]` as the kit's `[ get it ]`, drops the underline, and
    follows the artist-drop rule (§1.8.11).
13. **The live row** is accent BOLD; the muted, busy and paused slots
    are drawn.
14. **The bar** matches today's code: the Auto DJ split button, the
    transport at 21 / 27 / 34 / 41 / 48, the seek line from x 1 with 88
    cells.
15. **The frame-4 caption** had six words; every caption has 2–4.
16. **The sheet's words:** "Get it…" is "Get it" (it starts the job at
    once), "Copy" is "Copy title", and the running job reads
    `downloading…`, not `copying…`. **For the user to confirm**, as a
    visible wording change.
17. **The rule** under the heads is one 80-cell run across the content,
    not the board's two 39-cell runs.
18. **`‹ ›`** are accent, as the GUI's pager draws a live arrow, not dim.
19. **The tabs stop** is drawn (NO PEERS YET): the held tab is the slab
    and the selected tab reads accent BOLD. The board drew no keyboard
    stop on the strip.

### 2026-11-20 — The build's decisions

Numbered as the build plan numbered them; its other decisions are
architecture and process, not behaviour.

- **D10 — The engine's aside, adopted whole** (§1.8.8). A second player
  on the engine's bus, owned by its state; the open off the audio thread;
  main paused in place when the session's first clip *opens*, only if
  audibly playing; every main transport command ends the clip first as
  taken and restores nothing; a run-out restores at once, or holds for
  Preview all; `Failed` forgets the stored answer and `Refused` keeps it;
  never tapped, never in the player's status, never a play. Its open
  points, defaulted: a server switch restores main (the web does not
  resume; S30); the App does not defer its own auto-advance while a clip
  loads (a window of about a second, where a main advance ends the
  clip); the clip never feeds the visualizer.
- **D11 — The control face** (§1.8.9). While a clip session is live, a
  remote `/pause` pauses the engine unconditionally.
- **D12 — The lead window is the sheet's lead form** (§1.9): never
  "own"; Preview, Get it (one per plug-in), Open elsewhere… and Copy
  title, each hidden when its gate fails; the cursor starts on Get it,
  else Open elsewhere…, else Copy title, and skips a muted Preview; `•
  online` only while the poll says so; the room's keys do nothing inside
  the sheet, whose Preview row is the clip's control. Adding `l` there
  later is one arm in the sheet's key handler.
- **D13 — The nav** (§1.1.3–§1.1.5): `NAV[16]`, drawn in TOOLS under
  Discover, labelled `Peer recs`, with the key `R` (free in the GUI and
  the TUI), which asks for new seeds while the room is up; the rise-4
  rule at the floor; `R` kept out of `gui.tips.base`, which already uses
  97 of its 99 cells.
- **D15 — The window is shared** through `[gui] discover_window`, read on
  every enter, written on a pick; a failed ask keeps the new window, as
  Discover's `set_taste_window` never reverts (the phone reverts it);
  Discover's in-memory window is not live-synced (§1.4.6).
- **D16 — No auto-pin of the playing song at open** (§1.4.4), Discover's
  rule ("No auto-pin of the playing song", discovery-page.md's log).
  **For the user to confirm:** the web and the phone pin it, and the
  plan's R1 says "as each client's Discover does". The change, if wanted,
  is one line in the room's open and the R1 golden.
- **D17 — A taste 403 whose re-ping says on draws Scan pending**, and
  Check again does what Discover's does (§1.10.4, §1.10.6). The web draws
  Try again; the phone and Discover read it as scan pending.
- **D18 — R7's null-network re-ping** runs on any 200 whose seed is not
  random, tab or not (§1.10.1): the room always sends
  `networkOnRandomSeed` and never `moods`, so a null network can only
  mean p2p is off.
- **D19 — Previews follow the plan's §5.5 exactly** (§1.8.4–§1.8.7). The
  terminal adds one rule: a main transport command, while a press only
  resolves or waits, cancels that press. It is caught where every effect
  of the App passes (`App::note_pending`), with one way past it: the
  player's own advance, which is not the listener's command. That is the
  batch of a track's end, a crossfade's handover, a failed row's skip or
  retry (`PlaybackFailed`, and the probe answer `Reachable` that plays it
  again or resumes a stalled row), and an Auto DJ top-up pick
  (`apply_event` keeps its main commands as `own_advance`). An Auto DJ
  opener is the listener's "Surprise me" landing late, as a Listing's
  play-all does, and still cancels; so does every action and every
  control-face command. As the web (`asideMainPlaying` returns early when
  main played at the claim) and the phone (`_watchSong` only while the
  song was not playing) do, an advance never drops a press silently.
  Accepted edges: a tunnel's resume of a parked row still cancels (it is
  most often the listener's own deferred play); and until the next event
  or action, a control-face command whose batch equals that advance's
  (a Stop just after the queue's end Stop) passes too — main is already
  stopped, so the press playing is what the listener asked last.
- **D20 — Copy goes through `kit::clipboard::copy`** (§1.6.4). Both
  `Clipboard` and `Terminal` note `Copied` (not the kit's "if it allows
  it" wording); `Failed` notes `Couldn't copy`; both are dim, not the
  shell's gold error note.
- **D21 — Tips.** The design's five keys, plus one:
  `gui.tips.peer_recs_row_plain`, for a held row with no preview offer,
  because the GUI's tips name only keys that work. The precedence is
  §1.13's. A user who prefers the design's one string drops the key.
- **D22 — Grid walking** (§1.2.3), where the design says only "walk the
  cells": columns walk across blocks, `↑` at the top climbs to the tabs,
  `← →` go to the other column's same row or its last lead, PgUp/PgDn a
  screen, a stowed `↑` the last lead, the cursor held as a lead index.
- **D23 — The tabs stop's holes** (§1.2.2, §1.5.4): a tab that turns
  inert under the stop keeps it until a key moves it; a single label
  wider than the row is clipped and drawn alone; the wheel scrolls one tab
  a notch.
- **D24 — A sheet makes every room's base inert.** The pointer is blanked
  under any track-actions sheet, in every room, not only here: the kit's
  modal law already said so (docs/ui-kit.md, Modal), but before this
  change only Discover's modals, the mood filter and the Keep a copy and
  Add YouTube windows did it. If review
  objects, the rule narrows to this room with one condition.
- **D27 — Deferred, not built:** previews in the browser build; the
  visualizer tapping the clip; casting; the live check's server-audio
  steps (the stage has no server-audio engine); Terminal.app by hand (the
  user's); a trunk bundle build; `R` in the base tips; live window-memory
  sync between the two rooms.
- **D30 — State a poll or an answer can reshuffle is held by key, never by
  index.** The tabs stop, the tab it last held and the strip's first
  visible tab by endpoint id (`None` for All); the lead sheet by its
  lead's key, following it to a new index and closing when it leaves; the
  clip session by its lead's key, with its own copy of the list a chain
  walks, so the band draws on when its lead has left the shown list. The
  grid's cursor stays a lead index (D22), clamped when a new answer is
  shorter.
- **D31 — The gate, and the yield when it goes** (§1.1.1, §1.1.6). No
  `connected` term, as Discover's has none. The yield happens right after
  the frame's event fold and before its draw, every frame, because the
  shell's room re-check runs only on `Connected` and off the Library
  screen it returns early.
- **D32 — The seed pick's life** (§1.1.9). The pick is the room's only
  while its capture stands; a refusal stays in the room and is worded
  once; while picking the room is covered, not left; the nav back into
  the room while picking cancels the pick and asks again.

### 2026-11-20 — Where the sources disagree (S1–S37)

The build's behaviour digest tagged each disagreement among the design
(DS), the plan (P), the web (W), the phone (Ph) and Discover (TD); the
digest numbered them ⚑D1–⚑D37. The winner is in bold.

| # | topic | what each says | winner, and why |
|---|---|---|---|
| S1 | the nav row | DS: TOOLS under Discover, `NAV[16]`, row 18, rise 4. P: last in NAV, rows 4/12–16 | **DS**: P's anchors predate Rated and the Add rooms |
| S2 | the room's rhythm | DS: heads 7, rule 8, grid 9. P: heads 8–9, grid 10 | **DS**: Discover's grammar, and one more grid row |
| S3 | the lead window | DS: the sheet's lead form. P: a 46×12 keep-window modal. W: the now-playing window. Ph: #237's sheet | **DS**: P §2 item 15, every client reuses its existing lead window |
| S4 | the sheet's contents | DS: no match line, one Preview row, no invite, no cover. W: match line, ISRC, a tile per provider, invite. Ph: match line, cover | **DS**: no percentages, no art, no invite precedent |
| S5 | a failed tab | DS: `{label} try again`, gold and accent. P: `! {label}` in danger | **DS**: W and Ph show the words; danger is reserved |
| S6 | the muted slot | DS: `▸` dim, inert. P: `–` | **DS**: the shared muted-glyph decision |
| S7 | next in the band | DS: `▸▸` and `n`. P: none | **DS**: web and phone parity |
| S8 | the band's close | DS: `[X]`. P: `[✕]` | **DS**: Hack has no `✕` |
| S9 | the strip before the first poll | DS, P: the skeleton. W: nothing | **DS** |
| S10 | All alone, and a 404 route | DS: never drawn. W, Ph: drawn on a 404 | **DS**: a lone All carries nothing; the poll still stops on a 404 (P) |
| S11 | R1 pins now playing | P, W, Ph: yes | **TD** (no auto-pin), flagged for the user: D16 |
| S12 | a taste 403 with the flags still on | P: off only. W: Try again. Ph: scan pending | **Ph / TD**: D17 |
| S13 | Check again | W: an R2 re-ask. Ph: re-ping, then R1 | **TD**: Discover's latch and its clauses 81–82; D17 |
| S14 | the sheet opening during a clip | DS, W: keeps it. Ph: ends it | **DS / W**: row 6 stays visible above the sheet at every size |
| S15 | who owns the sheet's preview | DS: the room's clip. W: the window's own tile. Ph: the sheet's rows | **DS**: one session, one band, the attribution always in view |
| S16 | `w` / `e` during a clip | DS, Ph: end it. W: keeps it | **DS**: both cover row 6 |
| S17 | the provider pick | P: the first listed with `preview`. W: ignores `available`. Ph: filters it | **Ph**: the player's `has()` filters it already; still no fall-through |
| S18 | a held R4 re-ask | P: deferred while a clip is live. W: at the clip's end or the next poll. Ph: at once when holding ends | **Ph**: simpler, no extra wait |
| S19 | `n` at the list's end | W: ends the clip. Ph: disabled | **W**: the design draws no dead `▸▸` |
| S20 | `l` on a muted lead | DS: note `No preview`. W, Ph: inert | **DS**: the terminal has no tooltip on keys |
| S21 | copy outcomes | DS: `Copied` either way, `Couldn't copy`. W: a toast, or the text itself | **DS**: minimal text; D20 |
| S22 | the copy-failure note's colour | DS: dim | **DS** (the shell's error idiom is gold); D20 |
| S23 | Get it's word | DS: `Get it`, `[ get it ]`. P: `Get it…` | **DS**: the row acts at once |
| S24 | the running job's word | DS: `downloading…`. P: the marks' `copying…` | **DS**: Get it is never a copy |
| S25 | the copy row's word | DS, W, Ph: Copy title. P: Copy | **DS** |
| S26 | the links row's word | DS: Open elsewhere…. P, W: Find elsewhere | **DS**: the terminal's word for the same list |
| S27 | tips | DS: five keys, spelling Enter. P: one key with `↵` | **DS**: Discover's stowed / held split; D21 adds one |
| S28 | main's hold in a chain | P: a 5 s self-restore. W: no timer. Ph: `keepHeld` at every step | **Ph**: a 60 s iTunes wait outlives a 5 s hold |
| S29 | a refused clip | W: keeps the answer on `NotAllowedError` | **P / W in spirit**: forget on the clip's own failure, keep on an output failure |
| S30 | a server switch under a clip | P: main is restored. W: stops, no resume | **P**, flagged: the paused-in-place main is not torn down by a switch; D10 |
| S31 | a resolve 404 | W: failed. Ph: refused, clear and re-gate | **Builder default**: failed, and the gate read again; only a 403 runs R7 |
| S32 | the strip's keyboard | DS: a tabs stop (slab, selected accent BOLD). P: an outline | **DS**: the kit's cursor, Discover clause 47 |
| S33 | `ignoreVPaths` | W: sent | **TD**: Discover sends none |
| S34 | the user pins' cap | W: 12 | **TD**: 8, the shared seeds modal |
| S35 | the toolchain | P: one shared target dir, `+1.98.1` | **This repo's rules**: `cargo +stable`, never another worktree's target |
| S36 | the job start call | P: four arguments | **This repo**: `plugin_job_start_async` takes five (`landing`) |
| S37 | e2e names and rows | P: "Scenario P", nav rows 15 / 16 | **This repo**: Scenario P is the connected-server legs'; the room's are R and R2, on today's rows |

Also settled by the build's decisions: the phone puts the old window back
after a failed ask, and the terminal keeps the new one (D15).

### To record from the live walk

- **No room after an unpin** (§1.5.11). After `pin-all` and then an
  unpin on the stage: whether a standing `no room` clears, at which poll,
  and whether the next press posts and how it ends. Expected: the server
  drops the stored failure on the pin change, so the next poll reads `+`
  and the press posts. If the tab stays at `no room` with room to spare,
  that is a follow-up for the user, not a silent fix. **Walked
  2026-11-20:** as expected. `Rooftop FM no room` stood until the first
  peers poll after `unpin Basement` (the regular 15 s poll, 3 s after the
  unpin), which read `+ Rooftop FM`. The next press posted (202), drew
  `Rooftop FM adding…` and, on the 3 s poll, `Rooftop FM try again`
  (the stage's Rooftop FM always fails offline). Nothing stood at `no
  room` with room to spare; no follow-up.
- **Also seen on the walk** (not defects): `L` pressed again ends the
  live clip as well as the chain, as §1.8.6 says; R2's `pin` follows each
  answer's seed order, so two asks on one batch may list the same seeds
  in a different order; the stage's `noop-acquire` job is done by the
  first poll, so `downloading…` (L1) never shows there and the unit
  test covers that frame.
