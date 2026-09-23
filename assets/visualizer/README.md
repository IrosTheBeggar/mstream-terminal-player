# Visualizer presets

The ShaderToy-convention presets the mobile app ships, vendored so the desktop
visualizer runs the same files (PLAN.md, Phase 10). They are data, not code:
each file is a complete preset in the single-file format Android's ShaderEngine
parses — `// === pass: <name> ===` sections, `// === channel` and `// === size`
routing in the header, `// param:` tunables, and title/author/license lines.

**Source:** [`IrosTheBeggar/mstream_music`](https://github.com/IrosTheBeggar/mstream_music)
`assets/shaders/`, at `4ae3dec` — the last commit to touch that directory on
`master` (2026-06-17). Re-vendor by copying the directory over and re-applying
the local edit below; `cmp` against the source should then show only that file.

## Local edits

| File | Edit | Why |
|---|---|---|
| `06-4d-beats.glsl` | `mat2(vec4)` → `mat2` from four scalars (one line) | naga builds an invalid Compose for a `mat2` from a single `vec4`. Valid GLSL ES either way, and the same edit the Flutter port made — it belongs upstream, and this row goes when it lands there. |

Everything else naga needs is done at load time by `src/shader/glsl.rs`, so the
files stay the mobile app's.

## Licenses and attribution

| File | Title | Author | License |
|---|---|---|---|
| `01-spectrum-bars.glsl` | Spectrum Bars | mstream_music | MIT |
| `02-audio-tunnel.glsl` | Audio Tunnel | mstream_music | MIT |
| `03-plasma-pulse.glsl` | Plasma Pulse | mstream_music | MIT |
| `04-cyber-fuji.glsl` | Cyber Fuji 2020 | Jan Mróz (jaszunio15), uploaded to Shadertoy by kaiware007 — [Wt33Wf](https://www.shadertoy.com/view/Wt33Wf) | [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) |
| `05-hex-marching.glsl` | Hex marching | mrange — [NdKyDw](https://www.shadertoy.com/view/NdKyDw) | CC0 |
| `06-4d-beats.glsl` | 4D Beats | mrange (Mårten Rånge) — [tfK3Dy](https://www.shadertoy.com/view/tfK3Dy) | CC0 |
| `07-neonwave-sunrise.glsl` | Neonwave sunrise | mrange (Mårten Rånge) — [7dyyRy](https://www.shadertoy.com/view/7dyyRy) | CC0 |
| `08-neonwave-sunset.glsl` | Neonwave Sunset | mrange (Mårten Rånge) — [7dtcRj](https://www.shadertoy.com/view/7dtcRj) | CC0 |
| `09-mountainbytes.glsl` | MountainBytes — Phosphorescent Purple Pixel Peaks | mrange (Mårten Rånge); music by Virgill — [lX2GzD](https://www.shadertoy.com/view/lX2GzD) | CC0 |

`04-cyber-fuji.glsl` is the one preset that is not MIT or CC0, and it is kept
out of the binary until that is settled (PLAN.md, Phase 10 watch items). It is
here, attributed, so the tests can hold it to the same bar as the rest.
