# Auto DJ index cross-check

SPDX-License-Identifier: CC0-1.0 for the data files here (`wide.json`,
`wide.f32`, `expected.json`). To the extent possible under law, the authors
have waived all copyright and related or neighboring rights to them (the
CC0 1.0 Universal dedication,
https://creativecommons.org/publicdomain/zero/1.0/), as the player's own
card fixtures do, so the player's repo can take them too.

The Auto DJ index writer's second fixture set (`src/device/card/autodj`),
beside the player's (`test/fixtures/card`, whose Auto DJ goldens hold at
most 200 rows): a synthetic library of 517 rows of 40 components, and what
the player's reference writer makes of it at four selections. The test
builds each and compares its length, SHA-256, header CRC and selection
signature, and names the first row whose slot CRC differs.

- `wide.json`: an MPDJ description (the player's docs/AUTODJ-INDEX.md
  13.2), one row a line, with each row's primaryPath, artistNameKey and
  songKey given for checking. Its own selection is the writer's default.
- `wide.f32`: its vectors, raw little-endian f32, 40 a row, in the
  description's row order.
- `expected.json`: each build's selection (replacing the description's)
  and its facts, from `mpdj.build` with the rule as written (no fast path).

`make_wide.py` makes all three from a fixed seed (standard library only):

    python test/fixtures/autodj/make_wide.py <the player's repo>/tools/mpdj.py

They were made with the player's main at b78002e, where tools/mpdj.py last
changed in 85c752f (AUTODJ-INDEX.md as of 2026-10-13). Run it again only
when the index's rules change; never edit the files by hand. Names, hashes
and vectors are made up.
