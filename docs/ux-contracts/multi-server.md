# Multi-server and the queue

| | |
|---|---|
| **Design of record** | `mstream_music` @ `origin/master` (`72501f4`, 2026-09-15) — `lib/objects/server.dart` (the record: identity, kinds, federation and transport), `lib/singletons/server_list.dart` (`ServerManager`: the list, the current and default server, switching, removal, the peer reconcile, tunnels-follow-the-queue), `lib/util/server_tree.dart` (display grouping), `lib/screens/manage_server.dart` (Manage Servers, the row menu, the remove dialog), `lib/screens/add_server.dart` (add / edit / Test Connection, Quick Connect, the repair sheet), `lib/main.dart` (the app-bar picker), `lib/singletons/browser_list.dart` (the home rows: the read-only note, the NETWORK group, the no-server row), `lib/util/queue_actions.dart` (the queue verbs), `lib/widgets/queue_list.dart` (the panel), `lib/media/audio_stuff.dart` (the handler: the removal sweep, the failure walk, restore), `lib/singletons/queue_store.dart` (`queue.json`), `lib/util/stream_url.dart` (the per-item URL), `lib/screens/share_playlist_dialog.dart` (the single-server rule). Where the record disagrees with itself, this contract says so. |
| **Server API** | `GET /api/` (capabilities — `federationBrowse`, `federationDirect`, transcode, discovery) with `GET /api/v1/ping` as the older fallback; `POST /api/v1/auth/login`; `GET /api/v1/federation/peers` (a parent's peer list, caller-scoped); the parent's proxies for a peer — `/api/v1/federation/peers/:id/api/*` (browse), `/api/v1/federation/peers/:id/stream/*path` (bytes, Range forwarded, no transcode), `/api/v1/federation/peers/:id/art/*path`; `GET /api/v1/federation/peers/:id/access` (a direct guest ticket, mStream #943); `GET /api/v1/iroh/code` (LAN Quick Connect). Checked against mStream 6.28.0 (`c791799a`). |
| **Already in this repo** | Config: `ServerEntry` (url, username, last_path, per-entry `self_signed`), `default_server` outranking most-recently-used, `Credentials` (tokens + Quick Connect pairing codes), `preferred_server` / `set_default_server` / `remove_server` (the ONE flow that drops a pairing code) / `touch_server`. Session: `Session { server, server_id, tunnel_code, token, self_signed }`, `App::adopt_server` (a switch keeps what is streaming and sheds the queue — see clause 11). The GUI's servers room (`src/gui/servers.rs`): the header dropdown, the add chooser (Standard connect · Quick Connect with mDNS rows), Manage Servers with switch · edit · make default · pair phone · remove, a one-shot validation client, sign-in-needed answers opening the form. Engine: per-host TLS trust. The queue: `Queue` of `Track { filepath, metadata }` — **no origin** — with `Queue` / `QueueAll` / `RemoveFromQueue` / `ClearQueue` / `ToggleRepeat` / `ToggleShuffle`, `play_listing` / `queue_listing`. **Missing**: federated peers as browsable entries, queue items that carry their server, cross-server playback and per-server tunnels, the failure walk, queue persistence across launches, and the bundled-server mode. |
| **Target surface** | the GUI player — the servers room and the queue panel — with the origin-carrying queue landing in the shared App so the TUI follows |
| **Status** | contract extracted 2026-09-17; **one addition beyond the record** (§ The bundled server, clauses 50–58) requested for the installers. **Implemented 2026-09-18** in the shared App and the GUI (six commits on the PR branch), open questions 1–5 settled on their leans; **clause 38 implemented 2026-09-19** on the shared `mstream-iroh-tunnel` crate (tunnels kept by identity, the grace, the ladder, the hold) — clause 27's direct sentence in progress, see the deviations log |

## Intent

Keep more than one mStream server, and treat the servers a friend shares
through federation as servers of your own — browsable from the same
picker, read-only where their owner keeps the writes. The queue is the
one place all of them meet: every queued track remembers which server it
came from, so a queue can mix servers, keeps playing across a switch, and
comes back on the next launch exactly where it was. Removing a server
takes its tracks with it; everything else about the queue is left alone.
A player shipped inside the server's own installer never lets the user
remove the server it shipped with.

## Entry points

1. **The header's server label** — a dropdown once more than one server
   is selectable (record: the app-bar picker, shown only then). Peers sit
   under their parent; the last row adds a server.
2. **Settings › Manage servers** — every saved server with its actions
   (record: Manage Servers from the drawer, with a ⋮ menu per row).
3. **Add a server** — from the dropdown's last row, from Manage Servers,
   and from the no-server screen (record: the browser's only row reads
   "Welcome To mStream · Click here to add server").
4. **Federated peers arrive by themselves** — once a saved server's
   capability payload says `federationBrowse`, its peers are fetched and
   filed under it (clause 20). Nobody adds a peer by hand.
5. **The queue panel** and the browse rows' verbs — play now, add next,
   add to the end, play from here, add all (clause 32).
6. **Launch** — opens on the default server (clause 10) and restores the
   saved queue, paused, where it was (clause 40).
7. **Bundled mode** — the installer's launcher starts the player with the
   packaged server named on the command line (clause 50).

## States & flows

**The server list** is stored in order; index 0 is the default. Federated
peers are appended as the reconcile finds them and *displayed* grouped
under their parent (the stored order is what actions key on). Each entry
is one of three kinds — a **standard** server (a URL, optional
credentials), a **Quick Connect** server (a pairing code is its identity;
its URL is a placeholder for the live loopback tunnel), or a **federated
peer** (another server's peer, reached through that parent; it has no URL,
credentials, transport or version of its own — every one of those is the
parent's). A peer is additionally **listed**, **missing** (the parent
stopped listing it) or **hidden** (the user parked it); only a listed,
unhidden peer is *selectable*.

**The browsed server** is runtime state, never persisted. At launch it is
the first selectable entry — the default. Switching changes what the
browser shows and nothing else.

**The queue** is independent of the browsed server: each item carries its
origin (the server's identity and the track's path) and resolves its own
URL, token and transport at play time. It persists — items, index,
position, shuffle, repeat — and comes back on launch.

What invalidates what: a **switch** resets only the browse stack;
**removing** a server sweeps its queued tracks and its peers; a peer going
**missing** or **hidden** keeps its queued tracks; a **rename** on the
parent changes a peer's label only.

## Behavior contract

### The server list and identity

1. **Identity is minted at add and never re-derived.** A standard server
   gets a stable local name from its host at save time; editing the URL,
   the credentials or the flags keeps it. Everything durable keys on that
   name — queue origins, peer-to-parent links, the persisted queue.
2. **A Quick Connect server's identity is its pairing code.** Its URL is a
   placeholder and reads as read-only in the edit form; there is no Test
   Connection for it. **Re-pairing** (a rotated code) swaps the code in
   place and persists only once the new code has actually connected; on
   failure the old code is restored and re-dialled. Identity survives a
   re-pair.
3. **A federated peer's identity is its parent plus the peer's row id on
   that parent.** Its display name is the peer's name as the parent
   reports it; a rename there is a new label, nothing more.
4. **Order.** Stored order is display order, except that each peer sits
   directly under its parent. Index 0 is the default and wears the star;
   the Make default action is absent on that row.
5. **Adding — the fields.** Server URL (needed; must parse to an origin;
   stored as scheme + host + port), a **Public access** switch (stands the
   username and password down and persists them empty), an **Accept a
   self-signed certificate** switch (per entry; trusted already while the
   test runs), Username, Password, **Test Connection**, Save. Two tabs:
   Server URL and Quick Connect (a pairing code, pasted or scanned, or an
   mDNS row that carries its address).
6. **Adding — the connect check, in order.** Ping the origin (5 s). A 200
   saves the entry with no token. Otherwise, public access on → "Could not
   reach server. If it requires login, turn off "Public access" and add
   credentials." and stop. Otherwise log in (6 s); success saves the entry
   with the token; failure says "Failed to Login". **Test Connection**
   walks the same ladder with its own words — "Connection Successful!"
   (with " — mStream v{version}" when `GET /api/` answers), "Connected —
   signed in successfully.", "Server reached, but sign-in failed — check
   your username and password.", "Connection timed out.", "Could not
   connect: {error}" — and editing the URL clears the result.
7. **Editing.** URL, credentials and both switches can change. The form
   opens pre-filled (a server with no credentials opens in public mode);
   a login that yields a token replaces the stored one, a public save
   leaves it alone. A peer has no edit form: its row opens Info instead.
8. **Sessions.** Credentials belong to the entry. The record never
   re-logs-in on a 401: an expired session surfaces as a failed request,
   and the fix is editing the server. *This surface re-prompts — see the
   deviations log.*
9. **Each entry shows its server's version** when known ("Server
   v{version}", else "Server version unknown"), and a Quick Connect entry
   that is current shows its path — Direct or Relay — while connected.

### Default, current, switching

10. **The browsed server at launch is the default** — index 0, or the first
    selectable entry when index 0 is a missing or hidden peer. Last-used
    is not remembered by the record. *This surface keeps its
    most-recently-used fallback for a config with no default — see the
    deviations log.*
11. **A switch changes the browsed server and nothing else.** The browse
    stack resets to the new server's home (and its caches go); a Quick
    Connect server's tunnel is brought up *before* the browser queries it,
    a standard server needs no wait. **Playback and the queue are
    untouched**: nothing is stopped, cleared, filtered or re-pointed —
    each queued track keeps playing from its own server. A tunnel the
    queue still needs stays up after the switch (clause 38).
12. **The picker** exists only while more than one server is selectable.
    It lists servers grouped (peers under their parent, a branch glyph and
    "via {parent}" beneath the peer's name), marks the current one, skips
    hidden and missing peers, and ends with Add server.
13. **A switch that cannot reach the server** says "Failed To Connect To
    Server" and leaves the selection standing — the choice was made, the
    server is the thing that failed.
14. **Adding a server.** The record switches to a newly added Quick
    Connect server and leaves a standard add browsing where it was, unless
    it was the first server. *Settled here as "a new server becomes the
    browsed server" — see the deviations log.*
15. **Make default** moves the entry to index 0, switches to it at once,
    and persists the order.

### Federated peers

20. **Discovery.** After a saved server's capability payload reports
    `federationBrowse`, its peer list (`GET /api/v1/federation/peers`,
    with that server's token) is fetched and reconciled into the list as
    peers of it. A server whose payload lacks the flag reconciles to an
    empty list — every peer it had goes missing. A peer never reconciles
    peers of its own.
21. **When.** At launch for every server, on every switch, after an add or
    an edit, when a tunnel the ping was waiting on comes up, and on the
    Federation screen's refresh. The reconcile writes nothing when nothing
    changed.
22. **Matching.** By peer id first; then by name, so a peer the admin
    removed and re-added under a fresh id adopts the old record (its
    queued tracks keep resolving). New peers are appended; a rename
    updates the label only. Every peer's capabilities are pinned off
    (clause 26) at mint and on every refresh, whatever the peer reports.
23. **Missing.** A peer the parent stops listing is **flagged, not
    deleted** — queued tracks point at it. It leaves the picker, its
    Manage row reads "No longer shared by {parent}", and **Forget** (the
    only removal a peer ever gets) drops the record. A missing peer that
    was being browsed hands the browser to its parent. Listed again → the
    flag clears.
24. **Hidden.** A user choice that survives every reconcile (a "remove"
    would only last until the parent's list was mirrored again). A hidden
    peer leaves the picker but stays addressable — "Hidden peers keep
    playing what you queued from them." Hiding the browsed peer moves the
    browser to its parent first. **Show** reverses it. Hidden reads as
    parked, not gone (a dimmed row).
25. **A peer's Manage row**: branch glyph, name, "via {parent}"; actions
    Info · Make default (if selectable) · Hide / Show (while listed) ·
    Forget (once missing). **No Edit, no Remove while listed** — a peer is
    the parent admin's data. Row activation is Info.
26. **A peer is read-only.** Its home leads with an inert note — "Read-only
    server" / "Playlists and ratings stay on your own server". Absent on a
    peer: Playlists (tile and add-to-playlist), Rated, ratings and lyrics
    badges, sharing ("Tracks on a shared server can't be shared from here —
    they live in someone else's library."), transcoding, similar-tracks
    and the sonic path, and any federation or network of its own. Present:
    Files, Albums, Artists, Recent, play and queue, Auto DJ (random-songs
    is on the allowlist).
27. **Transport.** A peer's requests ride its parent — the browse proxy,
    the byte proxy (Range forwarded, so seeking works; no transcode, so a
    peer's opus file cannot play — degrade with a clear message), the art
    proxy — authenticated with the **parent's** token. When the parent
    grants a direct guest ticket, the peer gets a tunnel of its own and
    serves `/media` itself with the guest token; the proxy stays the
    fallback (an older peer, a refused mint, a failed dial), and a 401 on
    the direct path refreshes the ticket once through the parent.
28. **Removing a parent removes its peers** in the same sweep, queued
    tracks included (clause 35). Editing a parent's URL needs no
    propagation — a peer resolves its parent live.

### The queue

30. **Every queued track carries its origin**: the server's identity and
    the track's path. Its stream URL, art URL, token and transport are
    built from **its own** server at play time — never from the browsed
    server. Three URL shapes: a peer through the parent's byte proxy; a
    plain `/media` for a direct peer, a Quick Connect or a standard server
    without transcoding; `/transcode` with codec and bitrate when the user
    asked for it and the server is not known to lack it (unknown reads as
    capable; a peer is pinned incapable). A queue mixing capable and
    incapable servers lands each track on the right endpoint.
31. **Mixed queues are ordinary.** No verb checks homogeneity — batch
    metadata is fetched one request per server, one tunnel is kept per
    referenced server, a rating lands on the track's source server. The
    single-server rule exists in exactly one place: sharing (Out of scope).
32. **The verbs.** Play now (insert after the current track, jump, play).
    Add next (after the current track; at the end when nothing is
    playing). Add to end (never plays). Play from here (replace the queue
    with the view's playable rows in order, jump to the row, play; the
    shuffle variant shuffles that order once and starts at the top — the
    mode is untouched). Add all (append; "{n} songs added to queue").
    Remove (a row leaves; a duplicate track removes the row acted on, not
    the first equal one). Reorder (a grip per row). Clear (no
    confirmation; ends playback; a DJ session ends with it). Shuffle and
    repeat (off · one · all) are absolute states, not toggles from the
    outside. All items are built before the queue is touched, so a failed
    build never leaves it half replaced; a failed row is dropped and the
    jump lands on the nearest earlier survivor.
33. **An empty queue starts playing** on Add all and on a plain
    add-to-queue activation; never on Add next or Add to end. A batch add
    onto an empty, idle queue opens on track 1, not on the last row
    appended.
34. **The panel** is headed QUEUE with "{n} tracks"; empty reads "Queue is
    empty"; the playing row is marked.
35. **Removing a server removes its queued tracks, silently.** Playback
    lands on the current track's new position when it survives, else the
    first survivor after it, else the last survivor; playing state,
    position (when the current track survived), shuffle and repeat are
    preserved. A queue that belonged entirely to the removed server ends
    as a Clear would. A DJ pointed at that server is disarmed.
36. **A missing or hidden peer keeps its tracks in the queue**; a missing
    peer's tracks simply fail at play time and walk clause 37.
37. **The failure walk** (a track that will not load): a local copy is
    tried first (n/a here); a tunnel that is not up is brought up and the
    spot re-seeded **without skipping**; a transient network error probes
    connectivity — offline → pause and hold, resuming when the network
    returns; online → retry the same track a bounded number of times,
    then skip. Each skip says "Skipping a track that won’t play."
    (debounced). Once every track has failed: offline → "Lost the
    connection — paused. Resumes when you’re back online." and pause;
    online → "Can't play these tracks — check the files or server." and
    stop. Reaching the end of the queue while walking stops or pauses
    silently. A skip never resumes audio over a deliberate pause.
38. **Tunnels follow the queue.** A Quick Connect server's or a direct
    peer's tunnel is kept up for as long as the queue references it,
    whichever server is browsed, and released only after a grace period
    once its last track leaves.
39. **Persistence.** The queue is saved — items (title, album, artist,
    genre, duration, origin, path, art), index, position, shuffle, repeat —
    debounced on every edit and track change, checkpointed every 10 s
    while playing, and flushed when the app backgrounds. An emptied queue
    deletes the snapshot, but only after a queue existed this session (a
    restore that whiffed must not destroy what it failed to read). The
    setting **Resume queue on launch** ("Save the play queue and your
    place, and restore them when you reopen the app.") defaults on;
    turning it off blocks restore and deletes the snapshot.
40. **Restore** runs after the server list has loaded. Items whose server
    is no longer configured are dropped; every surviving item gets a
    freshly built URL (the saved token and cache-buster are stale); the
    index is clamped; a position within a second of the track's end
    restarts it. The queue opens **paused at the spot — never auto-plays**.
    A restore whose load failed parks the intended spot so a later save
    cannot overwrite it with track 1 / 0:00. A snapshot from another
    schema version is ignored, not migrated.

### The bundled server *(addition — no record)*

The Windows and macOS installers ship this player beside the server (PLAN
Phase 8; mStream PR #911). A player launched from that install must not be
able to delete the server it was installed with.

50. **The mode is a launch property.** The installer's launcher — which
    already owns the command line (the Ghostty config's `command =`, the
    Windows shortcut) — starts the player with the packaged server named:
    `mstream-player gui --bundled-server <url>`. Nothing is persisted: the
    same binary launched without the flag behaves as before, and a
    config the mode touched is an ordinary config.
51. **Seeding.** On boot, an entry matching the flag's URL (as a normalized
    origin) is the bundled entry; when none exists one is created without
    credentials and made the default. A default the user chose later
    stands on later boots. Sign-in works as for any server: a public server
    needs nothing; one that asks opens the sign-in form.
52. **It cannot be removed.** Its Manage row has no Remove verb; the tips
    line drops the key; the row wears a mark ("Came with the installer")
    and Info says "This server was installed with the player, so it can't
    be removed from here. Other servers can be added and removed as
    usual." Nothing else about it is special: switch, edit credentials and
    the certificate switch, make default, pair a phone all work.
53. **Its URL is read-only** in the edit form (the launcher owns the
    address, the way the record's Quick Connect URL is read-only). If the
    launcher ever names a different URL, the old entry becomes an ordinary
    one — removable — and the new one is seeded.
54. **Its peers are ordinary peers**: hide, show and Forget apply to them.
55. **Other servers are unaffected**: any number can be added, switched to
    and removed; the bundled server need not stay the default.
56. **Unreachable at boot** is the normal failure path (the note, the
    retry); protection does not depend on reachability.
57. **`--same-machine` rides along.** The launcher passes both: the player
    is on the server's machine, so the admin rooms' OS dialogs apply.
58. **The classic TUI** accepts the flag on `tui` for the same seeding (it
    has no remove verb to guard); the App-level seeding is shared.

## Wording

English reference; the record ships all ten of this repo's locales
(de en es fr it ja pl pt ru zh), so translations carry over. Record keys in
the second column; this surface's existing key in the third where one
already exists (`gui.srv.*`).

| String | record key | here |
|---|---|---|
| Manage Servers | manageServersTitle | gui.srv.manage |
| Server Info | manageServerInfo | — |
| Make Default / Edit / Delete / Info | makeDefault / edit / delete / info | gui.srv.act_default / act_edit / act_remove / — |
| Confirm Remove Server / Go Back / Delete | confirmRemoveServerTitle / goBack / delete | gui.srv.remove_title / remove_keep / remove_yes |
| Add Server / Edit Server | addServerTitle / editServerTitle | gui.srv.form_add / form_edit |
| Server URL / Username / Password | fieldServerUrl / fieldUsername / fieldPassword | gui.srv.form_server / form_username / form_password |
| Public access — Server is publicly accessible — no username or password needed. | fieldPublicAccess / publicAccessSubtitle | gui.srv.public |
| Allow self-signed certificate — Skip TLS validation for this server. Only enable on a network you trust. | selfSignedTitle / selfSignedSubtitle | gui.srv.self_signed |
| Server URL is needed / Cannot parse URL | validatorUrlNeeded / validatorUrlParse | — |
| Test Connection / Testing… / Connecting… | testConnectionButton / testing / connecting | gui.srv.reaching |
| Connection Successful! | connectionSuccessful | — |
| Connected — signed in successfully. | testConnectedSignedIn | — |
| Server reached, but sign-in failed — check your username and password. | testSignInFailed | gui.srv.bad_login |
| Connection timed out. / Could not connect: {error} | testTimedOut / testConnectFailed | — |
| Could not reach server. If it requires login, turn off "Public access" and add credentials. | couldNotReachServer | gui.srv.public_auth (partial) |
| Failed to Login | failedToLogin | gui.srv.bad_login |
| Failed To Connect To Server | mainFailedToConnect | — |
| Server URL / Quick Connect (the add tabs) | addServerTabUrl / addServerTabQuickConnect | gui.srv.method_direct / method_qc |
| Show pairing code | irohShowPairingCode | gui.srv.act_qr |
| Direct / Relay | irohPathDirect / irohPathRelay | — |
| Server pairing changed — re-pair to reconnect. / Re-pair | irohBannerRepair / irohRepairAction | gui.srv.no_code (partial) |
| Server v{version} / Server version unknown | serverVersionLabel / serverVersionUnknown | (Manage shows a live version) |
| via {parent} | serverPickerVia | — |
| No longer shared by {parent} | federatedNoLongerListed | — |
| Forget / Hide from the picker / Show in the picker | federatedForget / federatedHide / federatedShow | — |
| Show in server picker — Hidden peers keep playing what you queued from them. | federationShowInPicker / federationShowInPickerNote | — |
| Unnamed server | federationUnnamedServer | — |
| Read-only server / Playlists and ratings stay on your own server | browserFederatedReadOnly / browserFederatedReadOnlyNote | — |
| Tracks on a shared server can't be shared from here — they live in someone else's library. | federatedShareUnavailable | — |
| Network / Federation (home group headers) | browserSectionNetwork / browserFederation | — |
| Welcome To mStream / Click here to add server | browserWelcomeTitle / browserWelcomeSubtitle | (the no-session screen) |
| Queue / {n} tracks / Queue is empty / Clear queue / Remove | tabQueue / mainQueueCount / mainQueueEmpty / mainClearQueue / mainRemove | gui.queue.title / — / gui.queue.empty / — / — |
| Play now / Add next / Add to end of queue / Add All / Shuffle | queuePlayNow / queueAddNext / queueAddToEnd / addAll / shuffle | — |
| {n} songs added to queue | browserSongsAdded | — |
| Resume queue on launch — Save the play queue and your place, and restore them when you reopen the app. | settingsResumeQueue / settingsResumeQueueSubtitle | — |
| Skipping a track that won’t play. | *(no key — an English literal in the record)* | — |
| Connecting to {server}… *(the row parked on a tunnel that is not up yet, clause 37)* | *(no key — the record parks silently and shows the strip)* | — |
| Lost the connection — paused. Resumes when you’re back online. | *(no key)* | — |
| Can't play these tracks — check the files or server. | *(no key)* | — |
| The queue mixes songs from {count} servers ({names}). Sharing only works when every song comes from a single server. | shareMultiServerBody | (kept for a share feature) |
| The server "{name}" is no longer in your server list. Re-add it to share its queue. | shareServerGoneBody | (kept for a share feature) |

New, written here (ten locales at implementation):

| String | key |
|---|---|
| Came with the installer | gui.srv.bundled |
| This server was installed with the player, so it can't be removed from here. Other servers can be added and removed as usual. | gui.srv.bundled_note |

## Out of scope here

- **Downloads and the offline queue** — this player has no download
  feature. The record's "Remove synced files from device?" checkbox on the
  remove dialog (which the record never reads — a bug there, not a
  clause), "Download queue", per-server storage modes, and the download
  folder in Server Info do not port. Clause 37's local-copy step is vacuous.
- **Sharing the queue as a link** — no share feature yet. The
  single-server rule and its strings are recorded above so a share feature
  inherits them rather than rediscovering them.
- **Federation administration** — tickets, requests, keys, peers: main's
  admin Federation room (`mstream-player admin federation`). This contract
  is the listener's side only.
- **Chromecast, Android Auto, the car UI, the playback-resumption chip.**
- **iOS transcode overrides** and the record's Play-flavor gating of the
  self-signed switch (this surface always offers it).
- **The record's tap-behaviour setting** (play from here / add and play /
  add to queue) — the App's activation semantics stand; see the
  deviations log.
- **Listening-history purge on removal** — applies once this player keeps
  a history.

## Translation notes (terminal GUI)

Idiom mapping, per the kit's conventions (docs/ui-kit.md):

| Record | Here |
|---|---|
| App-bar picker | The header dropdown — exists; gains grouped peers, "via {parent}", the branch glyph, and the Add server row |
| Manage Servers screen + ⋮ row menu | The Manage Servers room — exists; verbs are keys (Enter switch · e edit · d default · p pair · x remove) and hover verbs; peers add h hide/show and, once missing, x Forget |
| Remove dialog | The kit warning modal — exists ("REMOVE SERVER · Forget %{server}?" with the pairing-code note); no checkbox |
| Peer row (`└` branch glyph) | The same glyph — the record's `kPeerBranch` is a box-drawing character; hidden peers dim |
| Read-only note row | A note row at the top of the peer's home listing (the browser top bar's note line is per-action, so a row is the honest place) |
| Snackbars / toasts | The note line above the bar |
| Swipe verbs on queue rows | Hover verbs + keys (x remove · i info) |
| Drag-to-reorder grip | Keys (design in the canvas: a modifier + ↑↓) and a drag on the row's grip cell |
| Add next / Play now on browse rows | New hover verbs beside the existing [+] — the tips line has no room, so they ride dwell tooltips like the browse bar's verbs |
| Version chip / Direct–Relay chip | Text in the row's detail column |

Already shared under the GUI (the App the TUI drives): the server list and
its config, `default_server` outranking MRU, Quick Connect pairing and its
repair note, per-entry self-signed trust down to the engine's stream
client, `adopt_server`, the sign-in-needed form, the queue's verbs and its
repeat/shuffle, `play_listing` / `queue_listing`.

Gaps in the shared App that this contract adds (verify at
implementation):

- **Origin on the queue item** (clause 30) — `Track` gains the server's
  identity; `play_index` builds the URL from the item's server, not
  `session.server`; the api and audio workers need a client per referenced
  server (token, self-signed trust, tunnel). **Per-server tunnels** (clause
  38) are new: today one loopback bridge lives with the session.
- **Peers as entries** (clauses 20–28): a peer kind on `ServerEntry`
  (parent, peer id, name, missing, hidden); the reconcile after a server's
  `GET /api/` (the payload's `federationBrowse` key is not yet read into a
  capability field); browse, stream and art through the parent's proxy
  prefix; capabilities pinned off; the direct-access ticket later.
- **A switch stops shedding the queue** (clause 11) once items carry
  origin — `adopt_server`'s "queue cleared" note retires.
- **Removal sweeps the queue** (clause 35) and the removed server's peers.
- **The failure walk** (clause 37) — the engine today counts a run of
  backend stream errors as the device dying; skipping an unplayable track
  with the offline probe is new, and the three toasts get keys.
- **Queue persistence** (clauses 39–40): a `queue.json` beside the config,
  a `[player] resume_queue` pref, the restore after the server list loads.
- **Bundled mode** (clauses 50–58): the flag on `gui` and `tui`, the seed,
  the guard in the servers room, the read-only URL, two strings.

No design canvas planned: the servers room and the queue panel exist; the
peer row, the bundled mark and the two new hover verbs are row-shaped
additions to drawn idioms. Revisit if discussion disagrees.

## Open questions (to settle before implementation)

1. **The flag's shape** (clause 50): `--bundled-server <url>` on the
   command line, an environment variable, or a key the installer writes
   into the player's config. Lean: the flag — the launcher already
   composes the command line, `--same-machine` is the precedent, and a
   launch property cannot outlive the install the way a config key would.
2. **Credentials for the bundled server** (clause 51): seed without any
   and let the server ask, or have the installer provision a token. Lean:
   seed without — the sign-in form already rides the funnel, and a token
   on the command line is a secret in a process list.
3. **Direct access to peers** (clause 27): port the guest-ticket path with
   the proxy path, or proxy-only first. Lean: proxy first; direct when a
   federated peer is available to test against (PLAN B3b notes none is).
4. **Where the origin lives** (clause 30): a server id on `Track`, or a
   separate queue-item type wrapping `Track`. Lean: a queue-item type —
   `Track` is the API's shape and the listing rows should not carry a
   server they already know.
5. **Persistence and the TUI**: the classic TUI restores its session on
   launch; does the saved queue join that restore for both surfaces? Lean:
   yes, shared — one `queue.json`, one rule.

## Deviations log

- **2026-09-17 — Entries are keyed by URL / tunnel identity** (clause 1):
  the record appends on add with no duplicate check (two adds of the same
  address yield two entries; the same pairing code twice yields two
  records with one identity). This surface's config already upserts by
  URL and by tunnel id; re-adding edits.
- **2026-09-17 — The default and most-recently-used** (clause 10): the
  record's default is index 0 and last-used is not remembered; this
  surface keeps `default_server` outranking MRU and falls back to MRU when
  no default is set (a hand-edited config may name none).
- **2026-09-17 — Sign-in prompts** (clause 8): the record never re-logs-in
  on a 401; this surface already opens the sign-in form when a server
  asks, and keeps that.
- **2026-09-17 — A new server becomes the browsed server** (clause 14):
  the record switches only for Quick Connect adds; here every add switches
  — the user just typed its address.
- **2026-09-17 — The remove dialog keeps this surface's body** ("Forget
  %{server}?" plus the pairing-code note): the record's dialog has a title
  and a dead checkbox and no body; the pairing-code consequence is worth a
  sentence.
- **2026-09-17 — Playback-failure toasts are localized here**: the
  record's three are English literals; they get keys and ten locales.
- **2026-09-17 — The tap-behaviour setting is not ported**: the App's
  activation semantics stand (Enter plays, [+] and `a` queue); the
  record's three-way setting is a mobile ergonomics answer.
- **2026-09-17 — The bundled server is an addition** (clauses 50–58): no
  record; wording written fresh in all ten locales; the mechanism is open
  question 1.
- **2026-09-18 — Implemented on the leans** of the open questions: the
  flag is `--bundled-server <url>` on `gui` and `tui` (1); the bundled
  entry is seeded without credentials (2); peers ride the parent's proxies
  only — direct access (the guest ticket) is not ported (3); the origin is
  a queue-row type wrapping the API's track (4); one `queue.json` serves
  both surfaces (5).
- **2026-09-18 — Tunnels do not yet follow the queue** (clause 38): the
  api worker holds one Quick Connect bridge, so a queued row on another
  tunnel is skipped with a word until that server is dialled again. A
  standard server's rows and a peer's rows through its parent play from
  any session. Per-server bridges are the next piece of the queue work.
  **Closed 2026-09-19**: the worker keeps a registry of tunnels by identity
  on the shared `mstream-iroh-tunnel` crate (the mobile app's client);
  every queued row's tunnel server and the session's transport are its
  targets, a tunnel nothing references is released 10 s after its last row
  leaves, a dial that did not answer is retried on the record's ladder
  (5, 10, 20, 40, 60 s; five minutes past the tenth failure) and a refused
  code never on its own, and a row whose tunnel is not up yet is **held**
  with "Connecting to {server}…" until it is (clause 37's tunnel step) —
  skipped only when the server refused the code or no code is saved. Two
  readings of the record are settled here: the session's own tunnel, once
  it fails to come up, is re-dialled on the ladder but the browser is not
  reconnected on its own (the user sees the error and chooses); and a
  row's cover and shape are fetched from the row's own server, which
  clause 30 asked for and the first implementation did not do.
- **2026-09-18 — The skip toast names the track and keeps the reason**
  ("Skipping a track that won’t play — {track}: {reason}"): the record's
  literal, with the two facts a terminal user can act on.
- **2026-09-18 — Reorder is keys only** (`<` and `>` on the queue column,
  clause 32): the GUI's queue panel has no drag grip yet; its rows answer a
  click and a hover [x].
- **2026-09-18 — A restored spot shows on the GUI's bar** (clause 40): the
  card names the row and its position, paused, and the first play starts
  there. The TUI shows the row's marker and starts there on Space; its
  transport reads the engine, which is idle until then.
- **2026-09-18 — No Info verb in the servers room** (clause 25): a peer's
  parent is on its row ("via {parent}"), which is what Info would have
  said. The remove confirmation reads "Forget {name}?" for a missing peer.
