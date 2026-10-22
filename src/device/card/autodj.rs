//! The Auto DJ index's writer: `/.mstream/autodj-<gen>.bin`, MPDJ v2 (the
//! firmware repo's docs/AUTODJ-INDEX.md, whose sections the numbers below
//! are; docs/METADATA.md 2.13, "M", ties it to the rest of the card), built
//! from mStream's embeddings of the music on the card.
//!
//! For each recording on the card that has an embedding (a row): the rows
//! nearest it by the exact rule of 5.2 — an f64 sum in index order,
//! quantised to a level before anything compares it — the rows of its own
//! song left out (2.4, 5.3), the list filled by the header's selection
//! (5.4; S84-55-169 by default) into a slot with its own CRC; beside the
//! slots each row's filters (DJRF), a map from every card file's path hash
//! to its row (DJPH), and the model's strings (STRS). Two conforming
//! writers make the same bytes (12.4): the player's reference writer is its
//! `tools/mpdj.py`, and the tests hold this one to that one's files.
//!
//! **Exact, not a matrix product.** The rule is a sequential sum per pair,
//! and a sum in any other order can land a pair a level away, which the
//! strided selection turns into a whole row's tail (12.1, 12.3). So the
//! rule itself runs here, on threads, sixteen pairs at a time with an
//! accumulator each: a product of two f32 values is exact in f64, so each
//! pair's sum is the reference's to the bit whatever the compiler does with
//! sixteen independent ones. 20,000 rows of 1,280 components take 35-45 s
//! on a 4-core laptop's 8 threads, 47-65 s on 4 (MEASURED: this file's
//! list code built on its own, optimised; the spread is the laptop's
//! clock), about the minute the transfer's plan allows.
//!
//! [`index`] is a run's whole step (7.3, 7.4): no index unless the card's
//! device.txt reads major 2; the file the root names now named again when
//! nothing in it would change; its slots carried into a new file when only
//! the filters, the paths, the strings or the card id would; else the
//! lists computed. The card id is this writer's addition to 7.3's table,
//! which names only the sections: every file carries its card's id
//! (M 2.5.2), so a copied card given a new one doesn't keep an index
//! stamped with the old.
//! The model's licence travels in the file (mStream's default model is CC
//! BY-NC-SA, 12.5): whether a card may carry an index at all — none for a
//! card made for someone else — is the caller's.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::contract::container::{self, FileMeta, MPDJ, REQUIRED, STRS, SectionOut, Strs};
use super::contract::device_txt::DeviceTxt;
use super::contract::mpdj::{self, DJPH, DJRF, DJSL, MAX_ROWS, SEL_STRIDED, SEL_TOP, Table, UNUSED, Uses};
use super::contract::mptg::{BPM_ANALYSED, Ledger, Record};
use super::contract::order::valid_rel_path;
use super::contract::prim::{fnv1a64, hex, path_hash};
use super::contract::text::{ARTIST, Run, TITLE};
use super::contract::{WriteError, Why};

/// The longest string a header field names (4.6).
const MAX_STRING: usize = 255;

// ── The model and the selection (4.1, 5.4) ──────────────────────────────────

/// The header's five strings (4.1, 4.6): the embedding model's id and
/// version as the embeddings route's `model` gives them, the metric, and
/// the licence and attribution of the data the lists derive from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Model {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) metric: String,
    pub(crate) license: String,
    pub(crate) attribution: String,
}

impl Model {
    /// The strings of a model 4.1 lists, by the route's `model.id` and
    /// `model.version`, byte for byte as every writer writes them (12.4).
    /// None for any other: the route doesn't carry a licence, so such a
    /// model gets no index until the index's text lists it.
    pub(crate) fn known(id: &str, version: &str) -> Option<Model> {
        match (id, version) {
            ("effnet-discogs", "1") => Some(Model {
                id: id.into(),
                version: version.into(),
                metric: "cosine".into(),
                license: "CC-BY-NC-SA-4.0".into(),
                attribution: "Discogs-EffNet by Music Technology Group, Universitat Pompeu Fabra \
                              (essentia.upf.edu/models)"
                    .into(),
            }),
            _ => None,
        }
    }

    /// In the header's order: modelId, modelVersion, metric, license,
    /// attribution.
    fn strings(&self) -> [&str; 5] {
        [&self.id, &self.version, &self.metric, &self.license, &self.attribution]
    }
}

/// Which candidates fill a list (5.4): the header's k, selKind, selHead
/// and selFloor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) k: u32,
    pub(crate) kind: u8,
    /// H, the exact head; kind 0's is k.
    pub(crate) head: u32,
    /// Lf, a level; kind 0's is 0.
    pub(crate) floor: u32,
}

impl Selection {
    /// The writer's default, S84-55-169: the exact top 84, then 85 entries
    /// at evenly spaced ranks down to the last candidate at level 140
    /// (cosine 0.547) or more, in 512-byte slots.
    pub(crate) const DEFAULT: Selection = Selection { k: 169, kind: SEL_STRIDED, head: 84, floor: 140 };

    /// The exact top k (selKind 0).
    pub(crate) const fn top(k: u32) -> Selection {
        Selection { k, kind: SEL_TOP, head: k, floor: 0 }
    }

    /// The slot this selection takes, or why a writer refuses it (4.1: what
    /// 8.2 would reject): k 2-169; kind 0 with H = k and Lf 0; kind 1 with
    /// 1 ≤ H ≤ k − 2 and Lf 1-255; no kind this writer doesn't know.
    fn slot_bytes(&self) -> Result<u32, WriteError> {
        let slot = mpdj::slot_bytes_for(self.k);
        if slot == 0 {
            return Err(WriteError("MPDJ: k must be 2-169"));
        }
        let fits = match self.kind {
            SEL_TOP => self.head == self.k && self.floor == 0,
            SEL_STRIDED => (1..=self.k - 2).contains(&self.head) && (1..=255).contains(&self.floor),
            _ => return Err(WriteError("MPDJ: a selKind this writer doesn't know")),
        };
        if !fits {
            return Err(WriteError("MPDJ: the selection's head or floor"));
        }
        Ok(slot)
    }

    /// The signature's `sel` line (6).
    fn line(&self) -> String {
        match self.kind {
            SEL_TOP => "sel top".into(),
            _ => format!("sel strided {} {}", self.head, self.floor),
        }
    }
}

// ── The keys (2.4; M 5.4) ───────────────────────────────────────────────────

/// A value is blank when it is empty or White_Space only: Unicode's
/// property, which `char::is_whitespace` (and `str::trim`) is.
fn blank(s: &str) -> bool {
    s.chars().all(char::is_whitespace)
}

/// nameKey: each run of White_Space one space and the ends trimmed; the
/// quotes `‘ ’ ‚ ‛ ′` to `'` and `“ ” „ ‟ ″` to `"`; the dashes `‐ ‑ ‒ – — ―
/// −` to `-`; then the default full lowercase with Final_Sigma
/// (`str::to_lowercase`: `ΣΟΦΙΑΣ` gives `σοφιας`). No accent folding, no
/// normalisation.
pub(crate) fn name_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            c => c,
        });
    }
    out.to_lowercase()
}

/// The artist display of a record's artist values: each trimmed of
/// White_Space, the blank ones dropped; one left as it is; several
/// deduplicated by nameKey (the first of equal keys kept) and joined with
/// `, `; none the empty string.
pub(crate) fn artist_display<S: AsRef<str>>(artists: &[S]) -> String {
    let values: Vec<&str> = artists.iter().map(|v| v.as_ref().trim()).filter(|v| !v.is_empty()).collect();
    if values.len() <= 1 {
        return values.first().map_or(String::new(), |v| (*v).to_owned());
    }
    let mut seen = HashSet::new();
    let kept: Vec<&str> = values.into_iter().filter(|v| seen.insert(name_key(v))).collect();
    kept.join(", ")
}

/// The nameKey of the first non-blank artist value; "" when there is none.
pub(crate) fn artist_name_key<S: AsRef<str>>(artists: &[S]) -> String {
    artists.iter().map(AsRef::as_ref).find(|v| !blank(v)).map_or(String::new(), name_key)
}

/// DJRF's artistKey: the low 32 bits of the FNV-1a 64 of that nameKey; 0
/// when there is none.
pub(crate) fn artist_key<S: AsRef<str>>(artists: &[S]) -> u32 {
    let key = artist_name_key(artists);
    if key.is_empty() { 0 } else { fnv1a64(key.as_bytes()) as u32 }
}

/// A row's song key: none for a blank title (never one from the file
/// name), so such a row is never "the same song" as another; else the
/// nameKey of the artist display, `|`, the nameKey of the title.
pub(crate) fn song_key<S: AsRef<str>>(artists: &[S], title: &str) -> Option<String> {
    if blank(title) {
        return None;
    }
    Some(format!("{}|{}", name_key(&artist_display(artists)), name_key(title)))
}

// ── The input ───────────────────────────────────────────────────────────────

/// A card file of the selection as the tags file of the same commit has
/// it: what its row takes from it when it is the row's primary file (5.3).
/// The title and the artist values are the record's as stored (M 2.3.6
/// already applied): the writer cuts, replaces and drops nothing in them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileIn {
    /// Relative to `/music`, every name as the card's directory stores it:
    /// the path hash is over these bytes (2.3).
    pub(crate) path: String,
    /// mStream's canonical hash, `audio-hash` else `hash`: the file's row.
    pub(crate) hash: [u8; 16],
    /// The run's field 0; "" for none.
    pub(crate) title: String,
    /// The run's field 1, its values in order.
    pub(crate) artists: Vec<String>,
    pub(crate) bpm10: u16,
    /// As stored: one above 24 is written as 0 (4.4).
    pub(crate) camelot: u8,
    /// BPM_ANALYSED: bpm10 and camelot are mStream's analysis.
    pub(crate) analysed: bool,
}

impl FileIn {
    /// A tags record's file, or None when its ledger row has no canonical
    /// hash, or it has no row (the listener's own files): nothing keys a
    /// row by it. Bad UTF-8 in the run is replaced, for the keys only.
    pub(crate) fn of(path: &str, rec: &Record, run: &Run, ledger: Option<&Ledger>) -> Option<FileIn> {
        let hash = ledger?.canonical_hash()?;
        Some(FileIn {
            path: path.to_owned(),
            hash,
            title: run.text(TITLE).into_owned(),
            artists: run.values(ARTIST).map(|v| String::from_utf8_lossy(v).into_owned()).collect(),
            bpm10: rec.bpm10,
            camelot: rec.camelot,
            analysed: rec.flags & BPM_ANALYSED != 0,
        })
    }
}

/// What the writer is given: every value a writer would otherwise choose
/// comes from its caller (12.4).
#[derive(Clone, Debug)]
pub(crate) struct IndexIn {
    /// The commit's (7.2): the file is named after it.
    pub(crate) generation: u32,
    pub(crate) card_id: u64,
    /// Unix seconds when the lists are computed; a file whose slots are
    /// carried keeps theirs (7.3).
    pub(crate) built_time: u32,
    /// The tags file the records come from; diagnostic.
    pub(crate) tags_generation: u32,
    pub(crate) model: Model,
    pub(crate) selection: Selection,
    /// Every card file of the selection, in any order.
    pub(crate) files: Vec<FileIn>,
    /// The embeddings by canonical hash, as the server sent them: never
    /// normalised again (5.2). A hash no file has is ignored.
    pub(crate) vectors: HashMap<[u8; 16], Vec<f32>>,
}

/// The rows a selection makes (5.1): the distinct canonical hashes among
/// its card files that have an embedding.
pub(crate) fn row_count(input: &IndexIn) -> usize {
    let hashes: HashSet<&[u8; 16]> =
        input.files.iter().map(|f| &f.hash).filter(|h| input.vectors.contains_key(*h)).collect();
    hashes.len()
}

// ── Everything but the lists (5.1, 4.4-4.6, 6) ──────────────────────────────

/// One row as the file keys it (5.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) hash: [u8; 16],
    /// Its primary file: the smallest path hash, then the smaller path.
    pub(crate) primary: String,
    /// The nameKey of the primary record's first non-blank artist value.
    pub(crate) artist_name_key: String,
    pub(crate) song_key: Option<String>,
}

/// Everything of a file but its lists: what a run works out before it
/// knows whether it needs them (7.3).
pub(crate) struct Prepared {
    generation: u32,
    card_id: u64,
    built_time: u32,
    tags_generation: u32,
    selection: Selection,
    slot_bytes: u32,
    /// In row order: by the canonical hash's bytes.
    pub(crate) rows: Vec<Row>,
    /// Each row's song: one id for the rows of one song key, the row's own
    /// index for a row with none (which nothing excludes).
    song: Vec<u32>,
    /// The same-song groups (6): members ascending, by their first.
    pub(crate) groups: Vec<Vec<u32>>,
    vectors: Panels,
    djrf: Vec<u8>,
    djph: Vec<u8>,
    path_count: u32,
    strs: Vec<u8>,
    string_at: [u32; 5],
    /// The selection signature's text, and the signature (6).
    pub(crate) sig_text: String,
    pub(crate) sig: [u8; 16],
}

/// DJPH's entries (4.5) from every card file of every row, (path hash,
/// row): sorted by hash, and a hash that two or more files share — a
/// 64-bit collision, in one row or across rows — left out entirely; a row
/// left with no entry stays in the table, a neighbour but never an anchor.
fn path_map(mut files: Vec<(u64, u32)>) -> Vec<(u64, u16)> {
    files.sort_unstable();
    let mut out = Vec::with_capacity(files.len());
    let mut i = 0;
    while i < files.len() {
        let j = files[i..].iter().position(|f| f.0 != files[i].0).map_or(files.len(), |d| i + d);
        if j == i + 1 {
            out.push((files[i].0, files[i].1 as u16));
        }
        i = j;
    }
    out
}

/// Validates the input (the writer's refusals) and works out every value
/// of the file but its lists. Refused: a selection or a string 8.2 or 4.6
/// would reject; a card file's path that isn't a relative path of names,
/// or a path twice; no row, or more than 65,535 ([`index`] says so before
/// it calls this); rows of mixed dims, or a value that isn't finite.
pub(crate) fn prepare(input: &IndexIn) -> Result<Prepared, WriteError> {
    let selection = input.selection;
    let slot_bytes = selection.slot_bytes()?;
    let model = input.model.strings();
    if model.iter().any(|s| s.len() > MAX_STRING || s.contains('\0')) {
        return Err(WriteError("MPDJ: a string over 255 bytes, or with a NUL"));
    }
    let mut paths = HashSet::new();
    for f in &input.files {
        if !valid_rel_path(f.path.as_bytes()) {
            return Err(WriteError("MPDJ: a card file's path that isn't a relative path of names"));
        }
        if !paths.insert(f.path.as_str()) {
            return Err(WriteError("MPDJ: a card file twice"));
        }
    }

    // The rows (5.1): a BTreeMap of byte arrays orders them as memcmp does.
    let mut by_row: BTreeMap<[u8; 16], Vec<&FileIn>> = BTreeMap::new();
    for f in input.files.iter().filter(|f| input.vectors.contains_key(&f.hash)) {
        by_row.entry(f.hash).or_default().push(f);
    }
    let n = by_row.len();
    if n == 0 || n > MAX_ROWS as usize {
        return Err(WriteError("MPDJ: rowCount must be 1-65,535"));
    }
    let vectors: Vec<&[f32]> = by_row.keys().map(|h| input.vectors[h].as_slice()).collect();
    let dim = vectors[0].len();
    if dim == 0 || vectors.iter().any(|v| v.len() != dim) {
        return Err(WriteError("MPDJ: the rows' vectors aren't all of one dim"));
    }
    if vectors.iter().any(|v| v.iter().any(|x| !x.is_finite())) {
        return Err(WriteError("MPDJ: a vector value that isn't finite"));
    }

    // Each row's primary file, its keys (2.4, 5.3) and its filters (4.4).
    let mut rows = Vec::with_capacity(n);
    let mut djrf = Vec::with_capacity(n * mpdj::FILTER_STRIDE as usize);
    let mut located = Vec::new();
    for (r, (hash, files)) in by_row.iter().enumerate() {
        let primary = files
            .iter()
            .min_by(|a, b| (path_hash(&a.path), a.path.as_bytes()).cmp(&(path_hash(&b.path), b.path.as_bytes())))
            .expect("a row has a file");
        located.extend(files.iter().map(|f| (path_hash(&f.path), r as u32)));
        djrf.extend_from_slice(&primary.bpm10.to_le_bytes());
        djrf.push(if primary.camelot <= 24 { primary.camelot } else { 0 });
        djrf.push(if primary.analysed { mpdj::FILTER_BPM_ANALYSED } else { 0 });
        djrf.extend_from_slice(&artist_key(&primary.artists).to_le_bytes());
        rows.push(Row {
            hash: *hash,
            primary: primary.path.clone(),
            artist_name_key: artist_name_key(&primary.artists),
            song_key: song_key(&primary.artists, &primary.title),
        });
    }
    let paths = path_map(located);
    let mut djph = Vec::with_capacity(paths.len() * mpdj::PATH_STRIDE as usize);
    for (h, r) in &paths {
        djph.extend_from_slice(&h.to_le_bytes());
        djph.extend_from_slice(&r.to_le_bytes());
    }

    // The songs, and the groups the signature names.
    let mut first: HashMap<&str, u32> = HashMap::new();
    let mut members: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut song = Vec::with_capacity(n);
    for (r, row) in rows.iter().enumerate() {
        let id = match &row.song_key {
            Some(key) => *first.entry(key.as_str()).or_insert(r as u32),
            None => r as u32,
        };
        song.push(id);
        members.entry(id).or_default().push(r as u32);
    }
    let groups: Vec<Vec<u32>> = members.into_values().filter(|g| g.len() > 1).collect();

    // STRS (4.6): a NUL, then each non-empty string once, in field order.
    let mut strs = Strs::new();
    let string_at = model.map(|s| strs.add(s.as_bytes()));

    // The signature (6): every line ended by one LF.
    let [id, version, metric, ..] = model;
    let mut sig_text = format!(
        "MPDJ-SEL 2\nmodel {id} {version}\nmetric {metric}\nk {}\n{}\nvec {dim} {}\n",
        selection.k,
        selection.line(),
        hex(&mpdj::vec_digest(vectors.iter().copied()))
    );
    for row in &rows {
        sig_text.push_str(&hex(&row.hash));
        sig_text.push('\n');
    }
    for g in &groups {
        let members: Vec<String> = g.iter().map(u32::to_string).collect();
        sig_text.push_str(&format!("same {}\n", members.join(" ")));
    }
    let sig = mpdj::signature(&sig_text);

    Ok(Prepared {
        generation: input.generation,
        card_id: input.card_id,
        built_time: input.built_time,
        tags_generation: input.tags_generation,
        selection,
        slot_bytes,
        rows,
        song,
        groups,
        vectors: Panels::new(&vectors, dim),
        djrf,
        djph,
        path_count: paths.len() as u32,
        strs: strs.bytes().to_vec(),
        string_at,
        sig_text,
        sig,
    })
}

// ── The lists (5.2-5.4, 4.3; 12.1-12.2) ─────────────────────────────────────

/// Rows a panel holds: the kernel sums W rows by W rows at once.
const W: usize = 4;
/// Panels a thread lists at a time: 64 rows, whose f64 copy stays in cache
/// while every panel of the table passes it once.
const BLOCK: usize = 16;

/// The rows' vectors in row order, W rows a panel and component-major
/// inside one — row r's component i is at `(r / W) W dim + i W + r % W` —
/// so the kernel reads W rows' i-th components together. The last panel's
/// rows past the table are zeros, whose levels nothing reads.
struct Panels {
    dim: usize,
    data: Vec<f32>,
}

impl Panels {
    fn new(rows: &[&[f32]], dim: usize) -> Panels {
        let mut data = vec![0f32; rows.len().div_ceil(W) * W * dim];
        for (r, row) in rows.iter().enumerate() {
            let base = (r / W) * W * dim + r % W;
            for (i, &v) in row.iter().enumerate() {
                data[base + i * W] = v;
            }
        }
        Panels { dim, data }
    }

    fn count(&self) -> usize {
        self.data.len() / (W * self.dim)
    }

    fn panel(&self, p: usize) -> &[f32] {
        &self.data[p * W * self.dim..(p + 1) * W * self.dim]
    }
}

/// The rule's sums (5.2) of W rows by W rows: each pair its own f64
/// accumulator, the products added in index order from component 0. `a` is
/// a panel widened to f64, `b` a panel as stored. Each product of two f32
/// values is exact in f64, so a pair's sum is the reference's to the bit.
#[inline]
fn dots(a: &[f64], b: &[f32], dim: usize) -> [[f64; W]; W] {
    let mut acc = [[0f64; W]; W];
    for (av, bv) in a.chunks_exact(W).zip(b.chunks_exact(W)).take(dim) {
        let bx = [bv[0] as f64, bv[1] as f64, bv[2] as f64, bv[3] as f64];
        for (x, sums) in acc.iter_mut().enumerate() {
            let ax = av[x];
            for (s, &by) in sums.iter_mut().zip(&bx) {
                *s += ax * by;
            }
        }
    }
    acc
}

/// How a run's lists are computed.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Work<'a> {
    /// Threads; 0 for as many as the machine runs at once.
    pub(crate) threads: usize,
    /// Set from another thread to stop at the next block of 64 rows: the
    /// build ends [`BuildError::Stopped`] and writes nothing.
    pub(crate) stop: Option<&'a AtomicBool>,
    /// Counts the rows whose lists are done, for a progress line.
    pub(crate) done: Option<&'a AtomicUsize>,
}

/// Why no file came of a build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BuildError {
    /// The input is one no writer may turn into a file.
    Refused(WriteError),
    /// [`Work::stop`] was set.
    Stopped,
}

impl From<WriteError> for BuildError {
    fn from(e: WriteError) -> BuildError {
        BuildError::Refused(e)
    }
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Refused(e) => e.fmt(f),
            BuildError::Stopped => f.write_str("MPDJ: stopped"),
        }
    }
}

impl std::error::Error for BuildError {}

impl Prepared {
    /// The rows the file has.
    // For the sync's progress (B1).
    #[allow(dead_code)]
    pub(crate) fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// DJSL's bytes: every row's slot in row order, the rows spread over
    /// threads 64 at a time, each block's slots written where they go.
    pub(crate) fn lists(&self, work: &Work) -> Result<Vec<u8>, BuildError> {
        let n = self.rows.len();
        let slot = self.slot_bytes as usize;
        let mut out = vec![0u8; n * slot];
        let blocks = self.vectors.count().div_ceil(BLOCK);
        let threads = match work.threads {
            0 => std::thread::available_parallelism().map_or(1, |t| t.get()),
            t => t,
        };
        let stopped = AtomicBool::new(false);
        let next = Mutex::new(out.chunks_mut(BLOCK * W * slot).enumerate());
        std::thread::scope(|scope| {
            for _ in 0..threads.clamp(1, blocks.max(1)) {
                scope.spawn(|| {
                    let mut a = vec![0f64; BLOCK * W * self.vectors.dim];
                    let mut levels = vec![0u8; BLOCK * W * n];
                    let mut order = vec![0u16; n];
                    loop {
                        if work.stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
                            stopped.store(true, Ordering::Relaxed);
                            return;
                        }
                        let Some((b, chunk)) = next.lock().ok().and_then(|mut it| it.next()) else { return };
                        self.block(b, chunk, &mut a, &mut levels, &mut order);
                        if let Some(done) = work.done {
                            done.fetch_add(chunk.len() / slot, Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        if stopped.into_inner() {
            return Err(BuildError::Stopped);
        }
        Ok(out)
    }

    /// Block `b`'s slots into `out`: its rows' levels to every row, by the
    /// kernel, then each row's slot from its levels.
    fn block(&self, b: usize, out: &mut [u8], a: &mut [f64], levels: &mut [u8], order: &mut [u16]) {
        let (n, dim) = (self.rows.len(), self.vectors.dim);
        let panel = W * dim;
        let p0 = b * BLOCK;
        let p1 = (p0 + BLOCK).min(self.vectors.count());
        for (d, &s) in a.iter_mut().zip(&self.vectors.data[p0 * panel..p1 * panel]) {
            *d = s as f64;
        }
        for q in 0..self.vectors.count() {
            let other = self.vectors.panel(q);
            for p in 0..p1 - p0 {
                let sums = dots(&a[p * panel..(p + 1) * panel], other, dim);
                for (x, row) in sums.iter().enumerate() {
                    let line = &mut levels[(p * W + x) * n..(p * W + x + 1) * n];
                    for (y, &s) in row.iter().enumerate() {
                        if let Some(l) = line.get_mut(q * W + y) {
                            *l = mpdj::level(s);
                        }
                    }
                }
            }
        }
        for (x, slot) in out.chunks_exact_mut(self.slot_bytes as usize).enumerate() {
            self.slot(p0 * W + x, &levels[x * n..(x + 1) * n], order, slot);
        }
    }

    /// Row r's slot (4.3) from its levels to every row (5.3-5.4), by 12.2's
    /// counting sort: its candidates are every row but those of its song
    /// (itself among them), by level descending and then row ascending, so
    /// the count of each level gives every rank's place, m included.
    fn slot(&self, r: usize, levels: &[u8], order: &mut [u16], out: &mut [u8]) {
        let song = self.song[r];
        let mut count = [0u32; 256];
        for (b, &l) in levels.iter().enumerate() {
            if self.song[b] != song {
                count[l as usize] += 1;
            }
        }
        // first[l]: the rank of level l's first candidate.
        let mut first = [0u32; 256];
        let mut total = 0;
        for l in (0..256).rev() {
            first[l] = total;
            total += count[l];
        }
        let sel = &self.selection;
        let m: u32 = if sel.kind == SEL_STRIDED { count[sel.floor as usize..].iter().sum() } else { 0 };
        let strided = sel.kind == SEL_STRIDED && m > sel.k;
        let used = if strided { sel.k } else { sel.k.min(total) };
        let k = sel.k as usize;
        out.fill(0);
        for j in 0..k {
            out[2 * j..2 * j + 2].copy_from_slice(&UNUSED.to_le_bytes());
        }
        if used > 0 {
            // The deepest rank a list takes; only the levels down to the
            // one that holds it are placed.
            let last = if strided { m - 1 } else { used - 1 };
            let mut at = first;
            for (b, &l) in levels.iter().enumerate() {
                let l = l as usize;
                if self.song[b] != song && first[l] <= last {
                    order[at[l] as usize] = b as u16;
                    at[l] += 1;
                }
            }
            for j in 0..used {
                let rank = if strided { mpdj::strided_rank(j, sel.k, sel.head, m) } else { j };
                let b = order[rank as usize];
                out[2 * j as usize..2 * j as usize + 2].copy_from_slice(&b.to_le_bytes());
                out[2 * k + j as usize] = levels[b as usize];
            }
        }
        let crc = mpdj::slot_crc(out, r as u32);
        let at = out.len() - 4;
        out[at..].copy_from_slice(&crc.to_le_bytes());
    }

    /// The file around DJSL's bytes (4.1-4.6): DJSL, DJRF, DJPH and STRS,
    /// headerBytes by the alignment rule (3.3), so DJSL starts on a slot.
    fn write(&self, djsl: &[u8], built_time: u32) -> Result<Vec<u8>, WriteError> {
        let n = self.rows.len() as u32;
        let sel = &self.selection;
        if djsl.len() != (n * self.slot_bytes) as usize {
            return Err(WriteError("MPDJ: DJSL isn't a slot a row"));
        }
        let mut th = Vec::with_capacity((mpdj::MIN_HEADER_BYTES - container::COMMON_HEADER) as usize);
        th.extend_from_slice(&n.to_le_bytes());
        th.extend_from_slice(&self.path_count.to_le_bytes());
        th.extend_from_slice(&(sel.k as u16).to_le_bytes());
        th.extend_from_slice(&(self.slot_bytes as u16).to_le_bytes());
        // indexBytes 2, scoreKind 1 (the level of 5.2).
        th.extend_from_slice(&[2, 1, sel.kind, sel.head as u8]);
        th.extend_from_slice(&built_time.to_le_bytes());
        th.extend_from_slice(&self.tags_generation.to_le_bytes());
        th.extend_from_slice(&self.sig);
        for at in self.string_at {
            th.extend_from_slice(&at.to_le_bytes());
        }
        th.extend_from_slice(&[sel.floor as u8, 0, 0, 0]);
        let array = |kind, count, stride, data| SectionOut { kind, flags: REQUIRED, count, stride, data };
        let sections = [
            array(DJSL, n, self.slot_bytes, djsl),
            array(DJRF, n, mpdj::FILTER_STRIDE, &self.djrf),
            array(DJPH, self.path_count, mpdj::PATH_STRIDE, &self.djph),
            array(STRS, 0, 0, &self.strs),
        ];
        let header_bytes = mpdj::header_bytes_for(4, self.slot_bytes, mpdj::MIN_HEADER_BYTES);
        let (generation, card_id) = (self.generation, self.card_id);
        let meta = FileMeta { magic: MPDJ, major: mpdj::MAJOR, minor: 0, generation, card_id };
        container::write(&meta, &th, header_bytes, &sections)
    }

    /// The file around these slots, checked whole before it's handed over:
    /// a file its own readers would treat as absent is a bug, never
    /// written.
    fn finish(&self, djsl: &[u8], built_time: u32) -> Result<Vec<u8>, BuildError> {
        let file = self.write(djsl, built_time)?;
        if check(&file).is_err() {
            return Err(BuildError::Refused(WriteError("MPDJ: the writer's own file reads as absent")));
        }
        Ok(file)
    }

    /// A new file, its lists computed.
    pub(crate) fn build(&self, work: &Work) -> Result<Vec<u8>, BuildError> {
        self.finish(&self.lists(work)?, self.built_time)
    }

    /// What 7.3's table makes of the file the current root names: the
    /// same signature and the same DJRF, DJPH, STRS and card id (the last
    /// not in the table: see the module's notes) — name it again; the same
    /// signature only — its slots carry, since they depend on nothing else
    /// and their CRCs on the row; anything else, or a file that reads as
    /// absent — compute.
    fn against<'o>(&self, old: Option<&'o [u8]>) -> Against<'o> {
        let Some(t) = old.and_then(|b| Table::open(b).ok()) else { return Against::Compute };
        if t.check(Uses::ALL).is_err() || t.sig != self.sig {
            return Against::Compute;
        }
        let sections = t.sections();
        let data = |kind| sections.iter().find(|s| s.0 == kind).map(|s| s.4);
        let Some(djsl) = data(DJSL) else { return Against::Compute };
        if djsl.len() != self.rows.len() * self.slot_bytes as usize {
            return Against::Compute;
        }
        let same = data(DJRF) == Some(&self.djrf[..])
            && data(DJPH) == Some(&self.djph[..])
            && data(STRS) == Some(&self.strs[..])
            && t.header().card_id == self.card_id;
        if same { Against::Same } else { Against::Carry { djsl, built_time: t.built_time } }
    }
}

enum Against<'o> {
    Same,
    Carry { djsl: &'o [u8], built_time: u32 },
    Compute,
}

// ── The file on the card (7.2-7.4, 8) ───────────────────────────────────────

/// A whole-file check (8.6's "all"): what the writer runs on its own file,
/// and a run's final verification of the file read back from the card.
pub(crate) fn check(file: &[u8]) -> Result<(), Why> {
    Table::open(file)?.check(Uses::ALL)
}

/// Why the new root names no index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoIndex {
    /// The card's device.txt doesn't list `read.mpdj=2`, or it has none
    /// (7.4): no reader of any other major exists.
    NotRead,
    /// No card file of the selection has an embedding (5.1).
    NoRows,
    /// More rows than u16 indexes hold (10.1): their count.
    TooManyRows(usize),
}

/// What a run does about its index (7.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Index {
    /// The new root names none.
    None(NoIndex),
    /// The new root names the file the current root names, again; its
    /// generation stays older than the root's.
    Same,
    /// A new file: written under `autodj-<gen>.bin` before the root that
    /// names it (7.2; `msmf::Comp::of` makes the root's entry). With
    /// `lists_kept`, its slots and their builtTime are the old file's.
    New { file: Vec<u8>, lists_kept: bool },
}

/// A run's step for the index (7.3, 7.4): `device` the card's device.txt
/// as read, `old` the index the card's current root names (its companion
/// entry already matched, M 2.5.3), if any. Only a new file needs the
/// lists, and only a new signature computes them.
pub(crate) fn index(
    device: &DeviceTxt,
    input: &IndexIn,
    old: Option<&[u8]>,
    work: &Work,
) -> Result<Index, BuildError> {
    if !device.writes_mpdj() {
        return Ok(Index::None(NoIndex::NotRead));
    }
    match row_count(input) {
        0 => return Ok(Index::None(NoIndex::NoRows)),
        n if n > MAX_ROWS as usize => return Ok(Index::None(NoIndex::TooManyRows(n))),
        _ => {}
    }
    let p = prepare(input)?;
    Ok(match p.against(old) {
        Against::Same => Index::Same,
        Against::Carry { djsl, built_time } => Index::New { file: p.finish(djsl, built_time)?, lists_kept: true },
        Against::Compute => Index::New { file: p.build(work)?, lists_kept: false },
    })
}

#[cfg(test)]
mod tests;
