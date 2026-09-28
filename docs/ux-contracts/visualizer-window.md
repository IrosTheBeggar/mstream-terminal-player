# Visualizer window

| | |
|---|---|
| **Design of record** | PLAN.md, Phase 11 (the decisions of 2026-09-20..22 and the 11.1 slice): the mobile app's shader presets in a window of their own, a child process fed by the player. The presets, their translation, the audio texture and the GPU renderer are Phase 11.0's (`src/shader/`, merged in #29 at `492f362`, 2026-09-27); the window is `src/viz_window/`, the host `src/gui/vizwin.rs`. The controls (clauses 9–15) are the mobile app's visualizer screen and its tuning panel — mstream_music `lib/screens/visualizer_screen.dart` (`_tuningOverlay`, `_resetTuning`) and `lib/singletons/settings.dart` (`defaultGlobalParams`, `visualizerShaderParams`) at `bdb174e0` — drawn by egui in `src/viz_window/controls.rs`. |
| **Server API** | None. The window hears the player's own playback through the engine tap. |
| **Already in this repo** | Everything but the window: the eight built-in presets drawn offscreen by `viz-probe`, `shader::audio` matching Android's texture byte for byte and its `Curve` (the three values the mobile panel tunes), `Scene::draw` into any RGBA8 view, each preset's `// param:` knobs parsed by `shader::preset`. |
| **Status** | Implemented 2026-09-27, this document and the code in one commit; the controls (clauses 9–15) the same day. |

## Intent

The music, seen: the same shader presets the mobile app runs, on the
desktop's GPU, in a window the terminal cannot be — fullscreen on a
second display if you like — moving to what the player is playing right
now, and never in the terminal's way. Its controls are there when the
pointer is and gone when it rests: pick a preset, tune it the way the
phone tunes it, and find it as you left it next time.

## Entry points

1. The top bar's **Visualizer** item, after the Stats tab. It is not a
   screen: the terminal stays where it was, and a window opens beside it.
   While the window is open the item is lit, and a click brings the
   window to the front.
2. **`V`** from any screen, the same.

## States & flows

- **Closed**: the item dim. `V` or a click opens the window; it comes to
  the front on its own, on the preset it last showed, its controls
  showing for a moment so they are found.
- **Open**: the item lit in the accent. `V` or a click raises the window.
  The player goes on as before; the window's own keys are the window's.
- **The pointer rests**: the bar fades; fullscreen, the pointer goes too.
  Moving it brings both back.
- **Tuning**: the panel slides in from the right; what its sliders turn
  is heard at once, and kept a moment after the last turn.
- **Nothing playing, or paused**: the window is fed silence, so the
  presets settle to their quiet state at the smoothing's pace, and keep
  moving on their own time. Once the silence has settled it is not sent
  again: the window keeps the quiet texture, and the player goes back to
  its own pace until something plays.
- **The window closes** (Esc, `q`, its close control): the item goes dim.
  The player notices within a frame, and keeps what the window last
  reported.
- **The window cannot open** (no display, no GPU, a preset the driver
  refuses): the child says why on its stderr, which goes to the log, and
  the player's note says "the visualizer window could not open — …".
  The player is not otherwise touched.
- **The player quits**: it closes its end of the pipe, which is the
  window's cue to quit; a window that lingers past a moment is killed.

## Behavior contract

### The process

1. **The window is a child process** — `mstream-player viz-window`, a
   hidden subcommand of the same binary — because the window toolkit
   wants the process's first thread and the terminal owns the player's,
   and because a preset that panics the shader compiler must cost the
   window, not the player.
2. **The parent computes, the child draws.** The player reads its own tap,
   builds the audio texture (`shader::audio`, the mobile app's curve) about
   thirty times a second while anything plays or silence is still settling,
   and writes it down the child's stdin as one framed message; a preset
   choice and a raise go the same way. The child never reads the tap, never
   touches the network, and holds no state of the player's. EOF on stdin
   means quit.
3. **The feeder never blocks the player**: frames queue two deep and a
   third is dropped, so a stalled window costs frames, not the player's
   loop. The child's stderr goes to the log, never to the terminal.
   Windows spawns the child without a console window.

### The window

4. **One window, the presets' 16:9, 960×540 logical at open**, titled with
   the preset in front; `focus_window()` on open so it fronts a
   terminal-launched process. Borderless fullscreen on `f`, and back.
5. **The preset draws at the window's logical size** — a Retina window
   shades one fragment per point, not per pixel, as mobile's `pixelScale`
   does — into an RGBA8 target the window then samples, upscaled, onto its
   surface. The surface is a non-sRGB format where the driver offers one;
   on an sRGB-only surface the blit undoes the second encoding, so the
   preset's colours are the preset's.
6. **Paced by the display**: `PresentMode::Fifo`, a redraw requested after
   each present, no rendering while occluded or minimized, never a busy
   loop.
7. **Every built-in preset**, single-pass and multipass alike (the
   renderer already draws the feedback buffers, so 11.1's six became
   eight): `→` and `←` step through them, wrapping; a preset this GPU
   refuses is marked with a line on stderr, never tried again and stepped
   past; if it refuses them all the window exits 1.
8. **Keys in the window**: `←` `→` presets, `t` the tuning panel, `f`
   fullscreen, `Esc` closes the innermost thing open (clause 15), `q`
   closes the window. Nothing else: volume, play and pause stay the
   player's.

### The controls

9. **Drawn in the window, over the picture** — egui on the window's own
   device, painted after the blit in the same submission, at the display's
   own pixel density. The picture never shrinks or moves for them.
10. **The bar**, centred along the bottom: `‹`, a dropdown naming the
    preset in front, `›`, then the tuning toggle (lit while the panel is
    open) and fullscreen (its mark says which way it goes). It shows when
    the window opens, while the pointer moves over the window or rests on
    the bar, while a dropdown or the panel is open, and after a key
    changes the preset (so the new name is seen in fullscreen); it fades
    2.5 s after the pointer last moved. Fullscreen, the resting pointer
    hides with it. While the panel is open the bar centres itself on the
    width the panel leaves. `‹` `›` are clause 7's `←` `→`.
11. **The dropdown** lists every built-in preset as `NN Title` — the number
    both apps name presets by, and the file's own title — the one in front
    selected. A row puts that preset in front. A preset this GPU refused
    is listed but not offered: its row is disabled, and hovering it says
    why ("This GPU would not draw it"). A row the GPU refuses on the spot
    is marked so, and the picture stays.
12. **The tuning panel** is the mobile app's: a sheet down the right edge,
    the window's height, sliding in and out; a title, **Reset**, and a
    close mark. Its first section, **Response curve · all shaders**, is
    the texture's curve: *dB floor* (−120 … −50), *dB ceiling* (−60 … −5)
    and *Smoothing* (0 … 0.95), each name's tooltip saying which way to
    turn it; the floor and ceiling stop 1 dB short of each other, so the
    window the sliders make is never one the texture refuses. Its second,
    named for the preset in front, is a slider per `// param:` knob the
    file declares, named and ranged as the file says; a preset with none
    says "This shader exposes no knobs." Every value shows to two decimals.
    Reset returns the curve and the preset in front's knobs to their
    defaults — no further. The rows scroll where the window is short.
13. **What the controls change goes where it acts.** A knob reaches its
    preset's `iParams[]` the next frame. The curve is the texture's, and
    the texture is the player's (clause 2): the window reports it up its
    stdout, and the player builds the next texture with it. Every change —
    the preset in front, the curve, a preset's knobs — is reported as one
    line (`preset <file>`, `curve <floor> <ceiling> <smoothing>`,
    `knobs <file> [<name>=<value> …]`); a line the player does not read as
    one goes to the log and costs nothing else.
14. **The player keeps them**, in `[visualizer]` in `config.toml`: `preset`
    (the file name), `min_db` / `max_db` / `smoothing`, and
    `[visualizer.knobs."<file>"]` with `<knob> = <value>` — only what is
    away from the defaults; an untouched window writes nothing. It saves
    the way the GUI saves its settings (the file loaded fresh, the
    section's fields replaced, a newer player's keys there kept, nothing
    written over a file that would not load at start), a second after the
    last change and at once when the window closes. The window reads the
    section when it opens, each value held to its slider's range and a
    knob the file no longer declares dropped; the player sets its texture's
    curve from the same reading by the same function, so the two agree
    from the first frame. The window opens on the preset it last showed.
15. **Esc closes the innermost thing**: an open dropdown, then the tuning
    panel, then the window. Keys a focused widget uses — Tab moving
    between the controls, a focused slider's arrows — are the widget's.
    Tooltips name their key (` — ←`, ` — T`) only while the player shows
    key hints (`[gui] key_hints`), the GUI's rule for its own.

## Wording

`gui.top.viz` "Visualizer" (the item), `gui.top.viz_tip` its tooltip
naming `V`, `gui.viz.failed` the note with the child's own last words.
The window's title is `mStream Visualizer — <preset>`, not localized (the
preset titles are the files').

The controls, `gui.viz.*` — the ones marked ◆ carried over from the mobile
record (`visualizerPreviousPreset`, `visualizerNextPreset`,
`visualizerTuningTitle`, `reset`, `close`, `visualizerNoKnobs`, and the
panel's English section and slider labels, which the record hard-codes):

| Key | English |
|---|---|
| `prev_tip` ◆ | Previous preset — ← |
| `next_tip` ◆ | Next preset — → |
| `pick_tip` | Choose a preset |
| `refused` | This GPU would not draw it |
| `tuning_tip` ◆ | Tuning — T |
| `fullscreen_tip` | Fullscreen — F |
| `windowed_tip` | Leave fullscreen — F |
| `tuning` ◆ | Tuning |
| `reset` ◆ | Reset |
| `close_tip` ◆ | Close — Esc |
| `curve` ◆ | Response curve · all shaders |
| `floor` ◆ | dB floor |
| `floor_tip` | What reads as silence. Lower it to see quieter detail. |
| `ceiling` ◆ | dB ceiling |
| `ceiling_tip` | What reads as full. Raise it if loud passages turn white. |
| `smoothing` ◆ | Smoothing |
| `smoothing_tip` | Higher is calmer, and slower to follow the beat. |
| `no_knobs` ◆ | This shader exposes no knobs. |

Knob names are the files' (`contrast`, `bassSpeed`), not localized, as on
the phone.

## Translation notes

- **egui** draws the controls (`egui`, `egui-wgpu`, `egui-winit` 0.36, the
  line on wgpu 30 and winit 0.30.13), with its clipboard and link features
  off, so the Linux binary links nothing new (the NEEDED guard), and with
  two of its four embedded faces — its emoji faces are 0.7 MB the controls
  never draw. The pass
  is pure CPU until it is painted, which is how the tests click the bar,
  pick a row and drag the sliders with no display; an ignored test
  (`the_controls_drawn_over_a_preset`) paints the states a screenshot of
  the real window cannot reach to PNGs on a real GPU.
- **Faces.** egui's two text faces, Ubuntu Light and Hack, carry Latin,
  Greek and Cyrillic, and Hack the arrows the tooltips name keys with; each
  family falls back on the other. Japanese and Chinese borrow the system's
  CJK face (Hiragino on macOS, Yu
  Gothic or YaHei on Windows, Noto CJK or WenQuanYi on Linux), checked
  the way egui will read it first; where there is none, the window's
  controls speak English rather than boxes.
- **The language** is the player's: the child runs the same boot
  detection (`MSTREAM_SETUP_LANG`, then the system locale).

## Deviations log

- **2026-09-27 — Extracted and implemented** for the GUI shell first; the
  TUI has no key for it yet (11.1's "done when" named the TUI). Left for
  later: the display kept awake while fullscreen and playing (11.1); the
  crossfade between presets (11.2); `serve --visualizer` (11.3). The
  multipass presets are in, since the renderer already drew them.
- **2026-09-27 — The controls** (clauses 9–15; clause 7 now marks a refused
  preset, clause 8 gains `t` and the innermost-first Esc), from the mobile
  record, with these differences:
  - **Always there.** The phone hides its panel behind a setting
    (`showVisualizerKnobs`); here the bar offers it to anyone, and it
    stays out of the picture until asked for.
  - **No tap for the next preset.** The phone's tap on the picture is
    "next"; here a click on the picture does nothing, because the click
    that brings a desktop window to the front must not change it. The
    arrows and the dropdown are the way, and the phone has no dropdown.
  - **Opens on the last preset**, where the phone opens on a random one.
  - **Kept by file and knob name**, where the phone keeps each shader's
    values as a list by asset path: a hand-edited file reads, and a knob
    a preset gains or loses does not shift its neighbours.
  - **The floor and ceiling never cross.** The phone lets the sliders
    make an empty window and its engine refuses it silently; here the
    sliders stop 1 dB apart, so what the panel shows is what is drawn.
  - **Localized labels.** The phone hard-codes its section and curve
    labels in English; here they are keys in the ten locales, with the
    English kept as the phone has it.
- **2026-09-27 — The German Visualizer tooltip** used an en dash before
  its key (`– V`), so the GUI's key-hints rule could not strip it; it
  reads `— V` now, as every other tooltip does.
