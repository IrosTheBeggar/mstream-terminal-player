# Visualizer window

| | |
|---|---|
| **Design of record** | PLAN.md, Phase 11 (the decisions of 2026-09-20..22 and the 11.1 slice): the mobile app's shader presets in a window of their own, a child process fed by the player. The presets, their translation, the audio texture and the GPU renderer are Phase 11.0's (`src/shader/`, merged in #29 at `492f362`, 2026-09-27); the window is `src/viz_window/`, the host `src/gui/vizwin.rs`. |
| **Server API** | None. The window hears the player's own playback through the engine tap. |
| **Already in this repo** | Everything but the window: the eight built-in presets drawn offscreen by `viz-probe`, `shader::audio` matching Android's texture byte for byte, `Scene::draw` into any RGBA8 view. |
| **Status** | Implemented 2026-09-27, this document and the code in one commit. |

## Intent

The music, seen: the same shader presets the mobile app runs, on the
desktop's GPU, in a window the terminal cannot be — fullscreen on a
second display if you like — moving to what the player is playing right
now, and never in the terminal's way.

## Entry points

1. The top bar's **Visualizer** item, after the Stats tab. It is not a
   screen: the terminal stays where it was, and a window opens beside it.
   While the window is open the item is lit, and a click brings the
   window to the front.
2. **`V`** from any screen, the same.

## States & flows

- **Closed**: the item dim. `V` or a click opens the window; it comes to
  the front on its own.
- **Open**: the item lit in the accent. `V` or a click raises the window.
  The player goes on as before; the window's own keys are the window's.
- **Nothing playing, or paused**: the window is fed silence, so the
  presets settle to their quiet state at the smoothing's pace, and keep
  moving on their own time.
- **The window closes** (Esc, `q`, its close control): the item goes dim.
  The player notices within a frame.
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
   thirty times a second, and writes it down the child's stdin as one
   framed message; a preset choice and a raise go the same way. The child
   never reads the tap, never touches the network, and holds no state of
   the player's. EOF on stdin means quit.
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
   refuses is skipped with a line on stderr; if it refuses them all the
   window exits 1.
8. **Keys in the window**: `←` `→` presets, `f` fullscreen, `Esc` or `q`
   close. Nothing else: volume, play and pause stay the player's.

## Wording

`gui.top.viz` "Visualizer" (the item), `gui.top.viz_tip` its tooltip
naming `V`, `gui.viz.failed` the note with the child's own last words.
The window's title is `mStream Visualizer — <preset>`, not localized (the
preset titles are the files').

## Deviations log

- **2026-09-27 — Extracted and implemented** for the GUI shell first; the
  TUI has no key for it yet (11.1's "done when" named the TUI). Left for
  later: the display kept awake while fullscreen and playing (11.1); the
  `// param:` tunables, config overrides and the crossfade between
  presets (11.2); `serve --visualizer` (11.3). The multipass presets are
  in, since the renderer already drew them.
