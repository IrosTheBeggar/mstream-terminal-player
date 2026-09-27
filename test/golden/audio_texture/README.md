# Audio texture golden vectors

`golden.bin` is what the mobile app's audio texture uploads for a known
signal, and `src/shader/audio.rs` is held to it (`cargo test shader::audio`).
It was produced by the reference itself: `generate.cpp` links
[mstream_music](https://github.com/IrosTheBeggar/mstream_music)'s
`android/app/src/main/cpp/audio_texture.cpp`, unmodified, against the stub
GLES in `stubs/` — whose `glTexSubImage2D` keeps the bytes it is handed —
and the kissfft that file ships with, built the way Android's CMake builds it
(`kiss_fft_scalar=float`).

Reference: `audio_texture.cpp` at `4ba1130` on `master` (blob `32940a41`),
the dB-window curve with its defaults of −69.7 dB, −20.7 dB and 0.27.

## The run

Ten uploads, each after 735 stereo frames (a 60 Hz frame at 44.1 kHz):

- a 60 Hz triangle and noise on the left, 440 Hz and 3 kHz triangles and
  noise on the right, every sample from integer arithmetic and float
  operations that are exact or singly rounded — so the Rust test generates
  the same samples bit for bit, and the FNV-1a hash of them in the header
  proves it did;
- `setParams(-80, -30, 0.6)` before the sixth upload;
- silence from the ninth, so the smoothing's decay is pinned too.

The first upload reads a ring that is mostly still zeros, which pins the
start-up case: fewer samples than the transform wants.

## Format

| Bytes | |
|---|---|
| 8 | `MSATGLD1` |
| 4 | uploads, u32 LE (10) |
| 4 | stereo frames per upload, u32 LE (735) |
| 8 | FNV-1a 64 of every input float's bits, left then right, LE |
| 1024 × uploads | each upload: 512 spectrum bytes, then 512 waveform bytes |

## Regenerating

From this directory, with an mstream_music checkout at `$MOBILE`:

```bash
M="$MOBILE/android/app/src/main/cpp"
clang -c -O2 -ffp-contract=off -Dkiss_fft_scalar=float -I "$M/kissfft" "$M/kissfft/kiss_fft.c" "$M/kissfft/kiss_fftr.c"
clang++ -std=c++17 -O2 -ffp-contract=off -Dkiss_fft_scalar=float -I stubs -I "$M" -I "$M/kissfft" generate.cpp "$M/audio_texture.cpp" kiss_fft.o kiss_fftr.o -o generate
./generate golden.bin
```

`-ffp-contract=off` keeps the signal's arithmetic unfused, as Rust's is. The
comparison allows one step either way per byte: kissfft's real transform and
the Rust side's radix-2 round differently in the last bits, and a truncation
to 0..255 can land either side of a boundary.
