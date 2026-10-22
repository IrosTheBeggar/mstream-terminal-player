#!/usr/bin/env python3
"""Make the Auto DJ index writer's cross-check: a synthetic library, and
what the player's reference writer builds of it.

    python test/fixtures/autodj/make_wide.py <the player's repo>/tools/mpdj.py

Writes, beside this script:
- wide.json, an MPDJ description (the player's docs/AUTODJ-INDEX.md 13.2)
  of 517 rows, their primaryPath, artistNameKey and songKey given for
  checking;
- wide.f32, its vectors (raw little-endian f32, 40 a row, in the
  description's row order);
- expected.json, what the reference writer (`mpdj.build`, the rule as
  written, no fast path) makes of the description at four selections: each
  file's length, SHA-256, header CRC, selection signature and every slot's
  CRC, in row order, so a test that differs can say which rows.

The library is wider than the player's own fixtures (200 rows at most) and
built to stress what a fast writer gets wrong: a cone where most rows have
far more than 169 candidates at level 140, so the strided tail and its
remainders decide most lists; outliers whose lists reach below the floor;
exact copies of vectors, so whole runs of candidates tie at one level;
vectors at twice the unit length (levels clamped at 255) and opposite ones
(negative cosines, level 0); same-song groups by case, spaces, a no-break
space, the quotes and the dashes nameKey folds; titles that are blank or
absent; rows with no artist or several; recordings at two and three paths,
NFC beside NFD; filters with a camelot above 24. random.Random and integer
components only, so the bytes are the same on any Python 3. Names are
made up.
"""
import hashlib
import importlib.util
import json
import math
import random
import struct
import sys
import unicodedata
from pathlib import Path

HERE = Path(__file__).resolve().parent
SEED = 20261018
N, DIM = 517, 40
CONE, OUTLIERS, OPPOSITE, DOUBLE = 440, 60, 7, 5      # then 5 copies: 517 rows
SELECTIONS = [
    {"k": 169, "selKind": 1, "selHead": 84, "selFloor": 140, "note": "the writer's default, S84-55-169"},
    {"k": 50, "selKind": 0, "note": "a top-k table"},
    {"k": 24, "selKind": 1, "selHead": 8, "selFloor": 120, "note": "short strided lists, 256-byte slots"},
    {"k": 169, "selKind": 1, "selHead": 1, "selFloor": 1, "note": "a head of 1 and the lowest floor: nearly "
                                                                    "every row strided down its whole order"},
]


def load(path):
    spec = importlib.util.spec_from_file_location("mpdj", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def unit_f32(mpdj, v):
    """An integer vector normalised in f64 (an exact sum of squares, a
    correctly rounded square root and division), each value rounded to f32."""
    s = sum(x * x for x in v)
    assert 0 < s < 1 << 53
    norm = math.sqrt(float(s))
    return [mpdj.f32(x / norm) for x in v]


def library(mpdj):
    rng = random.Random(SEED)
    bits = rng.getrandbits
    centre = [600 + bits(10) for _ in range(DIM)]
    other = [bits(12) - 2048 for _ in range(DIM)]
    emb = []
    for g in range(CONE):
        emb.append(unit_f32(mpdj, [(4 * c + 3 * (bits(12) - 2048)) << 6 for c in centre]))
    for g in range(OUTLIERS):
        emb.append(unit_f32(mpdj, [(2 * o + bits(11) - 1024) << 6 for o in other]))
    for g in range(OPPOSITE):
        emb.append(unit_f32(mpdj, [(-4 * c + bits(10) - 512) << 6 for c in centre]))
    for g in range(DOUBLE):                                # twice a cone row: exact in f32
        emb.append([2.0 * x for x in emb[37 * g + 11]])
    for src in (5, 6, 77, 78, 300):                        # exact copies
        emb.append(list(emb[src]))
    assert len(emb) == N

    hashes, rows = set(), []
    for g in range(N):
        while True:
            h = "%032x" % bits(128)
            if h not in hashes:
                break
        hashes.add(h)
        artist = "Generated Artist %02d" % (g // 9 + 1)
        album = "Sample Album %03d" % (g // 4 + 1)
        title = "Sample Title %03d" % g
        artists = [artist]
        if g % 37 == 5:
            artists = []
        elif g % 29 == 3:
            artists = [artist, "Generated Artist 90", artist.upper()]
        f = {"path": "%s/%s/%02d - %s.mp3" % (artist, album, g % 4 + 1, title), "artists": artists,
             "title": title}
        if g % 41 == 7:
            del f["title"]                                 # no title field
        elif g % 53 == 11:
            f["title"] = "   "                             # blank
        bpm10 = 0 if g % 13 == 4 else 650 + (g * 53) % 1300
        camelot = 24 + g % 7 if g % 19 == 0 else g % 25    # 25-30 are written as 0
        if bpm10:
            f["bpm10"] = bpm10
        if camelot:
            f["camelot"] = camelot
        if g % 3 == 1:
            f["bpmAnalysed"] = True
        files = [f]
        if g % 31 == 2:                                    # a second path, another record
            files.append(dict(f, path="%s/Copy/%s.mp3" % (artist, title), bpm10=900, camelot=3))
        if g == 200:                                       # a third
            files.append(dict(f, path="%s/Live/%s.flac" % (artist, title), bpm10=1000))
        if g == 301:                                       # NFC beside NFD
            nfc = "Generated Artist 34/Sample Café/%02d - Sample Café %03d.mp3" % (1, g)
            files = [dict(f, path=nfc), dict(f, path=unicodedata.normalize("NFD", nfc), bpm10=1111)]
        rows.append({"hash": h, "files": files})

    # Same-song groups: case, spaces and a no-break space; nameKey's folds.
    def song(g, artists, title):
        for f in rows[g]["files"]:
            f["artists"], f["title"] = artists, title

    song(20, ["Generated Artist 03"], "Sample Title 020")
    song(21, ["GENERATED ARTIST 03"], "SAMPLE  TITLE 020")
    song(22, ["Generated Artist 03"], " sample title 020 ")
    song(150, ["Generated Artist 17", "Generated Artist 18"], "Sample – Title’s 150")
    song(151, ["generated artist 17", "Generated Artist 18 "], "sample - title's 150")
    for g in range(100, 105):
        song(g, ["Generated Artist 12"], "Sample Title 100")
    song(512, rows[5]["files"][0]["artists"], rows[5]["files"][0].get("title"))  # a copy, its source's song
    if "title" not in rows[5]["files"][0]:
        raise AssertionError("row 5 has a title")
    return rows, emb


def described(mpdj, rows, emb):
    d = {"format": "MPDJ", "file": "wide.bin", "generation": 77, "cardId": "0x5EEDC0DE0A1B2C3D",
         "builtTime": 1791000000, "tagsGeneration": 76, "modelId": "synthetic-embedding", "modelVersion": "1",
         "metric": "cosine", "license": "CC0-1.0", "attribution": "synthetic test vectors", "dim": DIM,
         "vectors": "wide.f32",
         "note": "made by make_wide.py beside it (random.Random(%d)): the Auto DJ index writer's cross-check, "
                 "built at expected.json's selections" % SEED,
         "rows": []}
    for r in rows:
        primary = min(r["files"], key=lambda f: (mpdj.path_hash(f["path"]), f["path"].encode("utf-8")))
        r = dict(r, primaryPath=primary["path"], artistNameKey=mpdj.artist_name_key(primary["artists"]),
                 songKey=mpdj.song_key(primary["artists"], primary.get("title")))
        d["rows"].append(r)
    raw = b"".join(struct.pack("<%df" % DIM, *e) for e in emb)
    return d, raw


def facts(mpdj, desc, raw, sel):
    """What a build at one selection holds, by the rule."""
    d = dict(desc, **{k: v for k, v in sel.items() if k != "note"})
    p = mpdj.prepare(d, sidecars={"wide.f32": raw})
    lv = mpdj.level_matrix(p.emb)
    cand = [mpdj.order_candidates(lv[a], a, p.keys) for a in range(N)]
    lists = [mpdj.select(c, p.kind, p.k, p.head, p.floor) for c in cand]
    m = [sum(1 for _, x in c if x >= p.floor) for c in cand] if p.kind else None
    return p, lists, m


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    mpdj = load(sys.argv[1])
    rows, emb = library(mpdj)
    desc, raw = described(mpdj, rows, emb)
    builds = []
    for sel in SELECTIONS:
        d = dict(desc, **{k: v for k, v in sel.items() if k != "note"})
        data = mpdj.build(d, sidecars={"wide.f32": raw}, fast=False)
        r = mpdj.Reader(data)
        p, lists, m = facts(mpdj, desc, raw, sel)
        if [r.slot(i)[2] for i in range(r.rows)] != lists:
            raise AssertionError("the lists read back aren't the rule's")
        if sel == SELECTIONS[0]:
            k, head = p.k, p.head
            strided = [a for a in range(N) if m[a] > k]
            if not (len(strided) > N // 2 and any((m[a] - 1 - head) % (k - 1 - head) for a in strided)):
                raise AssertionError("the default build: most rows strided, with stride remainders")
            if not any(m[a] <= k and any(x < p.floor for _, x in lists[a]) for a in range(N)):
                raise AssertionError("the default build: a list below the floor")
            if not any(lv == 255 for e in lists for _, lv in e) or not any(lv == 0 for e in lists for _, lv in e):
                raise AssertionError("the default build: levels 255 and 0")
            if len(p.groups) < 4 or len(p.paths) <= N:
                raise AssertionError("the groups and the paths")
        crcs = ["%08x" % struct.unpack_from("<I", r.slot_raw(i), r.slot_bytes - 4)[0] for i in range(r.rows)]
        builds.append(dict(sel, bytes=len(data), sha256=hashlib.sha256(data).hexdigest(),
                           headerCrc="0x%08X" % struct.unpack_from("<I", data, 32)[0],
                           selectionSig=data[64:80].hex(), slotCrcs=" ".join(crcs)))
    # One row a line.
    head = json.dumps({k: v for k, v in desc.items() if k != "rows"}, indent=1, ensure_ascii=False)
    lines = ",\n".join("  " + json.dumps(r, ensure_ascii=False, separators=(",", ":")) for r in desc["rows"])
    text = head[:-2] + ',\n "rows": [\n' + lines + "\n ]\n}\n"
    if json.loads(text) != desc:
        raise AssertionError("wide.json doesn't read back")
    (HERE / "wide.json").write_text(text, encoding="utf-8", newline="\n")
    (HERE / "wide.f32").write_bytes(raw)
    expected = {"note": "tools/mpdj.py's build of wide.json at each selection (the description's own k and "
                        "selection replaced): bytes, SHA-256, header CRC, selection signature, and each row's "
                        "slot CRC in row order",
                "vectorsSha256": hashlib.sha256(raw).hexdigest(), "builds": builds}
    (HERE / "expected.json").write_text(json.dumps(expected, indent=1) + "\n", encoding="utf-8", newline="\n")
    for b in builds:
        print("k %d selKind %d: %d bytes, sha256 %s" % (b["k"], b["selKind"], b["bytes"], b["sha256"]))


if __name__ == "__main__":
    main()
