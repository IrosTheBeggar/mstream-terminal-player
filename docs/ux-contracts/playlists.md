# Playlists

| | |
|---|---|
| **Design of record** | `mstream_music` @ `137dd27` — the browser tab's SERVER-playlists view: `_playlistsView` / `makePlaylistWidget` / the create·rename·delete flows in `lib/screens/browser.dart`, the tap route in `lib/util/browse_actions.dart`, `lib/widgets/playlist_name_dialog.dart`, the version gate in `lib/util/server_version.dart`. *Not* `lib/screens/playlists_screen.dart` — that screen manages device-local playlists, which this player doesn't have. |
| **Server API** | `GET /api/v1/playlist/getall` · `POST /api/v1/playlist/load` · `POST /api/v1/playlist/new` `{title}` (400 when the name exists) · `POST /api/v1/playlist/rename` `{oldName,newName}` (server ≥ 5.16.0; 404 before) · `POST /api/v1/playlist/delete` `{playlistname}` |
| **Already in this repo** | `getall`/`load`/`save` on the client; `LibraryNode::Playlists` → `Playlist(name)` drill with `Entry::Node` rows (the TUI browses them today); the sonic room's save-as-playlist writes; `consume_playlist_saved` already refreshes an open Playlists view |
| **Target surface** | the GUI player, the existing "Playlists" nav row (digit 6) made real |
| **Status** | implemented in the GUI, 2026-08-31; create/rename/delete are new App+API plumbing both surfaces share |

## Intent

The server's playlists as a browsable room: see them, open one and play
it, and manage the list itself — create, rename, delete — without leaving.
These are *server* playlists (what the sonic room's "Save as playlist"
writes); every change is visible to every client of that server.

## Entry points

Main navigation only. The record reaches the view through the browse tab's
"Playlists" action; here it is the existing nav row. No capability gate —
every mStream server has playlists.

## Behavior contract

### The list

1. **The create action leads the list** — the record's full-width "New
   playlist" button above the rows; an empty list keeps it (creating the
   first playlist is the empty state's whole point).
2. Rows are the playlist **names** (the getall route carries nothing
   else); activation opens the playlist (clause 40).
3. Each row offers **Rename** and **Delete** — the record's ⋮ menu, with
   Delete wearing the destructive color.
4. Empty state: "No playlists yet" where the rows would be, the record's
   words.
5. The list refreshes after every successful create, rename, delete, and
   after the sonic room's save (already the case).

### Create

10. The name dialog: title "New playlist", action **Create**; the name is
    trimmed; cancel or an empty name is a no-op.
11. Creating posts `playlist/new` — an **empty** playlist; the server
    answers 400 when the name is taken, and the failure says so (the
    record's one wording: "Couldn't save the playlist — the name may
    already be in use.").
12. Success refreshes the list; no toast — the new row appearing is the
    confirmation (the record refreshes silently too).

### Rename

20. The same dialog, title "Rename", action **Rename**, prefilled with the
    current name; cancel, empty, or the unchanged name is a no-op.
21. Rename needs server ≥ 5.16.0. The record *hides* the menu item on a
    server known older (a 404 surfaced as a generic error reads as an app
    bug); this surface instead **offers it always and words the 404
    honestly** — "this server can't rename playlists — it needs mStream
    5.16" (see Deviations: the session doesn't carry the server version,
    and a correctly-worded failure keeps the record's goal without new
    connect plumbing).
22. Success refreshes the list.

### Delete

30. Delete **confirms first**: a modal naming the playlist, the record's
    "Confirm Delete Playlist" over the name, Cancel / Delete with Delete
    in the destructive color. No [X] close — a warning gate forces an
    explicit choice (the kit's own law agrees).
31. Success refreshes the list; like create, the row vanishing is the
    confirmation.

### Inside a playlist

40. Activation drills into the playlist's tracks — the shared Library
    drill, so rows behave exactly as everywhere: activate plays, the
    hover [+] / `a` queues one, back returns to the list.
41. The tracks are the playlist in order; the playing marker and durations
    as in every pane.
42. No per-track management inside (the record has none for server
    playlists — no remove-song route is used); editing the contents is the
    save-flow's job (sonic's "Save as playlist", create-or-overwrite).

### Failures

50. Every management failure is one honest sentence in the note line —
    `<what failed>: <the server's words>` (this player's rule; the record
    collapses to one generic toast, and this surface is deliberately more
    specific — see Deviations).
51. A failed action changes nothing else: the dialog closes, the list
    stands, retry is typing it again.

## Wording

English reference; record keys in parentheses; all ten locales carried
over from the record's `.arb` files.

| String | (record key) |
|---|---|
| Playlists | playlistsTitle |
| New playlist | playlistsNew |
| Create / Rename / Delete / Cancel | create / rename / delete / cancel |
| Confirm Delete Playlist | browserConfirmDeletePlaylist |
| Couldn't save the playlist — the name may already be in use. | playlistActionFailed |
| No playlists yet | playlistsEmptyTitle |

## Out of scope here

- **Add-a-track-to-a-playlist** (`playlist/add-song`, the record's
  track-⋮ sheet): a track-level verb that belongs to every browse pane,
  not to this room — its own contract when the track-actions surface is
  ported.
- **Device-local playlists** (`playlists_screen.dart`): no local library
  on this surface.
- **Share playlist** (`share_playlist_dialog.dart`): rides the record's
  share subsystem, absent here.

## Translation notes (terminal GUI)

| Record | Here |
|---|---|
| Full-width "New playlist" button above the rows | The kit's **affirmative card** — its documented use is exactly "the add action above the table it feeds" |
| Row ⋮ menu (Rename / Delete) | Hover reveals `rename` and the `[X]` remove on the row (the servers-room idiom); keys `e` / `x` on the cursor row |
| Name dialog | Kit modal with the line input — the sonic save prompt's shape |
| Delete confirm dialog | The kit **warning modal**: gold border, no [X], explicit choice |
| Generic failure toast | The note line, worded per failure (player rule) |
| Tap a playlist | Click / Enter — the shared Library drill (the albums room's pattern) |

Already shared: the whole browse side — `LibraryNode::Playlists` /
`Playlist(name)`, `Entry::Node` rows, drill/back, track rows with playing
marker and durations, and the post-save refresh. New plumbing (both
surfaces gain it): `playlist_new` / `playlist_rename` / `playlist_delete`
on the client, their ApiCmds/Events, and the refresh-on-success
consumption.

## Deviations log

- **2026-08-31 — Rename is offered on every server** and the 404 is worded
  ("needs mStream 5.16") instead of the record's hide-the-item version
  gate: the session doesn't carry the server version, and the record's
  real goal — no mystery error — survives as wording. Revisit if the
  connect flow ever keeps `ServerInfo.version`.
- **2026-08-31 — Failures are specific**, not the record's one generic
  toast: `<what failed>: <the server's words>` is this player's standing
  rule and strictly more honest.
- **2026-08-31 — No design canvas for this feature**: every element maps
  to an existing, already-drawn idiom (albums list/drill, affirmative
  card, servers-room row verbs, sonic name modal, kit warning modal); the
  translation table above is the whole design.
