# The window spike — manual checklist

What a human at a real keyboard should run before `gui --window` is offered anywhere (PLAN.md,
Phase 14). Written by the spike's criteria judge on 2026-09-30 from the evidence of steps 1–5;
each item names its expected result, and the known gaps are marked as such so a failure there is
recorded, not a surprise. Expected numbers are from a release build on an M-series Mac.

1. macOS, launch: with a `--features desktop` build, run `mstream-player gui --window --server https://demo.mstream.io` from Finder-launched Terminal and from iTerm. Expected: a titled 'mStream Player' window appears in under 1 s, drawn at 100×30, with no terminal escape garbage in the launching shell.

2. macOS, keys: at a real keyboard, press 1–9, then Down/Up, Left/Right, Home/End, PageUp/PageDown, Tab and Shift+Tab, Enter, Esc, Backspace, Space and T. Expected: each behaves exactly as in `gui` in a terminal (same room, same focus movement; Shift+Tab moves focus backwards).

3. macOS, key repeat: hold Down in the Files list for 2 s, then hold Backspace in the search box. Expected: the selection and deletion repeat at the system repeat rate and stop on release, with no stuck key.

4. macOS, Cmd chords: press Cmd+V with text on the clipboard in the search box, then Cmd+Q, then Cmd+W. Expected: paste is known to be missing today, so record the result. Cmd+Q quits, and the config and session are written. Record whether Cmd+W closes the window and whether the instance-lock sidecar is removed.

5. macOS, dead keys: switch to US International or ABC Extended, type Option+e then e, and ` then a, in the search box. Expected: é and à land as single characters, with no stray accent or duplicate.

6. macOS, IME: enable Japanese Romaji and type 'nihon' then Space until 日本 is highlighted, then Enter in the search box, then repeat with Pinyin 'zhongguo' and Space. Expected: the committed 日本 / 中国 land once, and nothing submits twice. Kotoeri needs two Enters after a Space, the first closing its candidate list, exactly as a plain AppKit NSTextView does; 'nihon' then Enter with no Space commits on one. Note where the candidate window appears (known gap: top-left) and whether any inline preedit is visible (known gap: none).

7. macOS, CJK titles in en: with an English UI, browse to a folder whose track titles contain Japanese, Chinese or Korean. Expected (known gap): these draw as boxes today. Confirm, and confirm again after any CJK-fallback fix.

8. macOS, pointer: hover rows, click a folder then a track, right-click a row, drag a queue row, scroll with a mouse wheel and with a trackpad fling. Expected: the hand cursor over clickables, each click acts on the row under the pointer at the edges and on the bottom row, the sheet opens, the drag reorders, and scrolling is proportional without runaway momentum.

9. macOS, pointer leaving: press the mouse on a queue row, drag outside the window, release, then come back. Expected: no stuck drag or held button. This is a known gap, since CursorLeft does not clear held buttons.

10. macOS, resize and displays: drag the window corner slowly across many sizes, then drag the window between a Retina and a non-Retina display. Expected: no crash, the grid reflows, and text stays crisp. Note any glyph stretch, and any wrong cell size after the scale change (ScaleFactorChanged is unhandled).

11. macOS, colour: put the window beside `gui` in Terminal.app with the same theme, and compare the background and dim text with Digital Color Meter set to 'native'. Expected (known gap): the window's ground is visibly darker (#0a0b16 vs #12131c). It should match after the gamma fix.

12. macOS, playback and CPU: play a track, open Now Playing (0), then the Visualizer tab (4), and watch Activity Monitor for 60 s. Expected: about 3% CPU on Now Playing and about 15% in the Visualizer tab (2.7× the terminal). The audio has no glitches, and moving the window or covering it with another window does not stall playback.

13. macOS, occlusion: minimise the window and restore it, then switch Spaces and back. Expected: a full, correct repaint with no blank rows (the Occluded(false) repaint workaround).

14. Windows, launch: on Windows 11 at 100% and at 150% scaling, with a `--features desktop` build, run `mstream-player.exe gui --window --server https://demo.mstream.io` from PowerShell and from Windows Terminal. Expected: the window opens in under 1 s on DX12 (or Vulkan), the text is crisp at both scales, and no console escape garbage appears.

15. Windows, keys: repeat the key walk: digits, arrows, Home/End, PgUp/PgDn, Tab/Shift+Tab, Esc, Enter, Backspace, and Ctrl+C to quit. Expected: the same behaviour as the terminal GUI, and Ctrl+C quits cleanly with the config written.

16. Windows, AltGr: on a German layout type AltGr+Q (@) and AltGr+E (€) in the search box; on a Polish layout, AltGr+A (ą). Expected: the characters land as text and are not read as Ctrl+Alt chords that trigger shortcuts.

17. Windows, dead keys: on US International type ' then e, and ^ then o. Expected: é and ô land as single characters.

18. Windows, IME: use Microsoft Japanese IME to type 'nihon' and commit, and Microsoft Pinyin to type 'zhongguo' and commit. Expected: each commit lands once. Record the candidate window's position (known gap) and whether any preedit is shown.

19. Windows, clipboard: press Ctrl+V in the search box. Expected: record the behaviour (paste is not wired), and make sure it does not insert a literal 'v' or trigger an action.

20. Windows, pointer at 150%: click the first and last visible rows and the transport buttons, right-click a row, drag a queue row, and use the wheel and a precision touchpad. Expected: every action lands on the cell under the pointer; there is no off-by-one row at fractional scale.

21. Windows, close and exit: close with the title-bar X and with Alt+F4, then relaunch. Expected: a clean exit (code 0), the config and session persisted, no orphan audio thread or process in Task Manager, and the instance lock released so the relaunch works.

22. Windows, drag between monitors: move the window between a 100% and a 150% monitor. Expected: the cell grid adapts without a crash or mis-hit clicks (ScaleFactorChanged is unhandled today).

23. Windows, CPU and memory: idle on Library for 60 s, then 60 s in the Visualizer tab, watched in Task Manager. Expected: idle stays comparable to the terminal GUI (about 1–2%), and the Visualizer tab stays about 15% or less of one core, at roughly 110–130 MB.

24. Windows, native dialogs (added after the v0.12.0 smoke): in `setup --window` press b (Browse), and in the GUI's window open the Add-torrent room's file dialog. Expected: each dialog opens centred over the player's window, not at its last spot or the screen's corner; the player's window takes no clicks while it is up (GetWindow(dialog, GW_OWNER) is the player's window); and on closing, the focus goes back to the player's window. The unit test reads the owner rfd is handed; only this run shows Windows honouring it.

## Criteria the judge scored

- **1. Rendering fidelity: the window matches the terminal cell for cell, CJK included, no tofu; the colour-gamma finding** — partial. The cell-text comparison cannot see tofu, and the tofu gap is real. cjk_fallback (src/gui/window/mod.rs:746) loads a CJK face only when the UI locale is ja or zh. So CJK track titles in an en or other locale, and Korean anywhere, draw as boxes. On Linux, faces at a collection index above 0 are skipped (Noto CJK zh). The Visualizer tab's named ANSI colours go through ratatui-wgpu's SVG-keyword table, so Color::Blue draws as #0000ff, not a terminal palette. Gamma is unfixed. The options are to patch ratatui-wgpu's surface format, pre-convert colours in the Counted backend, or write a custom PostProcessor.

- **2. Plain input: every keymap key, repeat, Shift+Tab, Esc, digits and letters behave as in a terminal; clicks land on the intended cell at 2× (1× untested)** — partial. Every end-to-end key is injected through the script lever at the Raw-event level. With Accessibility not granted, no real OS keyboard event was ever delivered, so real key repeat is untested. Tab, BackTab, PageUp/Down, Home/End, Left/Right and F-keys were never driven end to end, only unit-tested. Cmd/Super chords are dropped by design (command_keys_stay_with_the_platform), so Cmd+V paste does not work. Clicks at 1× are untested: the Linux leg ran at scale 1 but issued no clicks. CursorLeft and focus loss do not clear held buttons.

- **3. Dead keys and AltGr** — untested. This needs a human at a physical keyboard on macOS (US-International / ABC Extended dead keys, German) and on Windows (AltGr on a German or Polish layout). There is a risk that AltGr, reported as Ctrl+Alt on Windows, gets routed as a Ctrl chord by the Ctrl-by-key-place rule.

- **4. IME: commits land as text (script lever); a real IME session untested; preedit and candidate-window gaps** — partial. The IME events were synthetic, so a real IME session (macOS Japanese/Pinyin, Windows Microsoft IME) is untested. The preedit string is tracked but never drawn, so the user sees no composition inline. The candidate window is pinned at the window origin because set_ime_cursor_area is not fed the caret cell. Also unverified: that IME is enabled only while a text field has focus, and that Enter during composition is not double-delivered.

- **5. Integration cost: changes outside the new module under roughly 200 lines, all at the seams** — pass. It is under 200 only when moved lines are counted as moves. A reviewer reading the raw stat will see 551 lines. The frame()/input() refactor does touch the terminal GUI's hot loop. That loop is behaviour-preserving on reading and in step 2's parity checks, but it is still the path the terminal ships on.

- **6. Performance: idle CPU comparable to the terminal, frame time under 16 ms, startup under a second to first frame (release)** — pass. Frame time:
- Single-frame maxima reached 16.12 ms (w4-nowplaying) and 17.82 ms (verifier's vw5-viz). These are over 16 ms, although every p95 is under 5 ms.

Startup:
- The cold first run after a build listed the window at 1.01 s, with first visible present 452 ms after run entry, so a cold first launch can exceed 1 s.
- No single spawn-to-first-pixel timer exists; this is a bound assembled from two measurements.

Cost beyond the criterion:
- In the Visualizer tab (30 fps) the window costs about 2.7× the terminal's CPU: 14.8–16.7% vs 5.4–5.7%.
- RSS is 111–116 MB vs 33–38 MB, so wgpu/Metal adds about 78 MB.

Measurement conditions:
- The measured binary predates a cosmetic stats.rs tidy.
- The Docker leg ran concurrently during the builds, though not during the measurements.

- **7. Footprint: binary grows a few MB at most; the Linux linkage set unchanged** — pass. NEEDED is unchanged, but the Linux window has a runtime dlopen dependency on libxkbcommon-x11.so. When it is absent, `gui --window` panics in xkbcommon-dl (x11.rs:59) with exit 101 before any window opens. This was reproduced by both the leg and the verifier. No pre-spike Linux binary size was measured: the aarch64 binary is 45,066,952 bytes, with no baseline. Build time is the bigger cost: about 14–17 minutes for a release build, and the test profile recompiles the wgpu/naga stack.

- **8. Platform: macOS Metal proven; Linux under Xvfb (adapter); Windows untested** — partial. Windows is entirely untested (DX12/Vulkan, DPI scaling, IME, AltGr). On Linux, only X11 under Xvfb with a CPU rasteriser was run. Still untested there: Wayland (winit prefers it when WAYLAND_DISPLAY is set), a real GPU driver, fractional scaling, and x86_64. Adapter choice appears only at wgpu_core=debug. On macOS only scale 2 was exercised. ScaleFactorChanged (dragging between displays) is unhandled.
