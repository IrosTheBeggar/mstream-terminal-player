# The window spike — evidence

What PLAN.md's Phase 12 cites, kept beside it because the session scratchpad that produced it
does not persist. Screenshots are the real window on the dev Mac (Retina, scale 2), downscaled
to 1000 px wide; the text under each name says which step and scenario produced it.

- `1-render-ja.png` — step 1: the demo Library screen in Japanese, Hack plus Hiragino, no tofu.
- `2-live-files.png` — step 2: the real workers in the window, the Files room listing
  demo.mstream.io's library.
- `3-input-mouse.png` — step 3: the end of the mouse scenario driven through the script lever
  (hover verbs, [+] queueing, wheel, a right-click sheet, a queue-row drag).
- `5-art-wall.png` — step 5: the Albums wall with covers drawn as GPU textures.
- `5-art-sheet-over-wall.png` — step 5: a sheet open over the wall and the queue; the covers
  under it fall back to the ▀-mosaic and the sheet reads.
- `6-keys-german-option-chords.png` — the keyboard check (2026-09-30, real key events posted
  through the OS with Accessibility granted): on the German layout, Option+l, Option+e and
  the y key typed `@€z` into the search box.
- `6-keys-repeat-backspace.png` — the same check: `abcdefgh` typed, then Backspace held for
  one press and five auto-repeats, leaving `ab`.
- `6-keys-pinyin-commit.png` and `6-keys-pinyin-commit-dump.txt` — the Pinyin IME in the
  search box after `zhongguo` and Space: the dump holds `中国`; the screenshot shows the known
  gap, an en locale has no CJK face so the two glyphs draw as bars.
- `6-keys-japanese-preedit.log` — the window's stderr for the Japanese probe: the composition
  `n に にh にほ にほn にほん`, then no commit on the first Enter or a Right arrow, then `preedit ""`
  on Esc — the winit 0.30.13 commit-key defect.
- `stats-*.json` — step 4's `MSTREAM_WINDOW_STATS` reports, release build, ~25 s each against
  demo.mstream.io: idle, the Albums wall, playback on the Library screen, Now Playing, and
  Now Playing's Visualizer tab (the one 33 ms screen); `stats-wall-with-covers-debug.json` is
  step 5's 20 s on the wall in the DEBUG build.
- `cpu-*.tsv` — `ps` samples every 2 s (%cpu, cumulative CPU seconds, RSS) for the window and
  the terminal GUI on a pty, idle and in the Visualizer tab; CPU in the record is the cputime
  delta over wall time from t ≥ 10 s.
- `linux-linkage.txt` — the Linux leg's NEEDED list and guard exit (Docker, aarch64).
- `checklist.md` — the manual checklist and the criteria scorecard, from the step-4 judge.
