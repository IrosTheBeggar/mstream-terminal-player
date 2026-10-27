# App icons

The desktop flavour's icons, all made from `../mstream-logo.ico` (the
Windows icon `build.rs` embeds in the exe, which stays the source):

- `hicolor/<n>x<n>/apps/io.mstream.player.png` at 16, 32, 48, 64, 128,
  256 and 512 px: the freedesktop icon-theme layout, named by the app id
  (`src/identity.rs`) so the desktop entry (`../linux/`) finds it as
  `Icon=io.mstream.player`. The window also embeds the 256 px file as its
  own icon, and as the Dock icon of a bare macOS binary
  (`src/gui/window/icon.rs`).
- `io.mstream.player.icns`: the macOS icon, for a later .app bundle.

The .ico's frames are 32-bit BMPs at 16, 32, 48, 64 and 128 px; each of
those sizes is the .ico's own frame, unscaled. It has nothing larger, so
256 and 512 are its 128 px frame upscaled by `sips` (soft at the edges),
and the .icns stops at 512 (no 512@2x). A sharper source would be a .ico
with a 256 px frame, or a vector logo of the same art; the mStream
server's `build/icon.png` (512 px) is a flat-colour variant of the logo,
not this gradient one.

To regenerate, on a Mac (`sips` and `iconutil` are part of macOS):

    python3 scripts/icons.py

The output is reproducible: rerunning it on the same .ico writes the same
bytes. The tests in `src/identity.rs` and `src/gui/window/icon.rs` check
the sizes and that the embedded icon is the 256 px file.
