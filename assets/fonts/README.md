# Bundled fonts

`mStreamSymbols-Regular.ttf` (1,640 bytes) is the GUI window's symbol face: the glyphs the GUI
draws that Hack, the window's main face, lacks. The census test in `src/gui/window/mod.rs`
(`every_glyph_hack_lacks_has_a_fallback`) keeps that list, `BEYOND_HACK`: ★ ☆ ✓ today. The
face also has ✔ ✗ ✘, the heavy tick and the two crosses, for the next checkbox or failure
mark. The window embeds it (`include_bytes!`, only in builds with the `window` feature) ahead of
any system face, so the stars and the tick are the same on macOS, Windows and Linux, and are
drawn on a machine with no symbol font at all.

It is not a subset of another font: no subsetter was at hand (fontTools is not installed here),
and the one face on this machine under a fitting license that has all three glyphs, Kreative
Korp's Fairfax (OFL, shipped inside the ratatui-wgpu crate), is a 6×12 pixel face whose star is
five pixels across, which would sit badly beside Hack. So the glyphs are drawn by
`scripts/symbol-font.py` from plain geometry on Hack's metrics (2048 units to the em, advance
1233, ascender 1901, descender -483): a regular five-pointed star the size Menlo and DejaVu draw
theirs, the same star hollowed by a 60-unit outline, and the tick and crosses as two strokes at
Hack's stem weight (150 units) and a heavy weight (250). No outline comes from another font.

License: SIL Open Font License 1.1, in `OFL.txt` beside it (no Reserved Font Name).

To regenerate (Python 3, standard library only; the output is the same bytes every run):

    python3 scripts/symbol-font.py

To add a glyph, add its code point and contours to `glyphs()` in the script, regenerate, and
add it to `BEYOND_HACK` if the GUI draws it. To swap in a fuller face instead (Noto Sans
Symbols 2 or DejaVu Sans Mono, both under fitting licenses), subset it with fontTools'
`pyftsubset --unicodes=...` to the census list plus the Miscellaneous Symbols, Dingbats,
Arrows, Geometric Shapes and Box Drawing blocks, put it here with its license, and point
`SYMBOLS` in `src/gui/window/mod.rs` at it; the census test then checks it the same way.
