//! The card's side of a sync (docs/METADATA.md 2.10.2, step 2): `/music`
//! listed, names as the card stores them, and each row of the committed
//! ledger matched against the listing one path component at a time — the
//! same bytes, else the one entry FAT's case-insensitive rule makes the
//! same — then held to its (size, fatTime) with the uniform skew, else its
//! fingerprint. A row whose file is gone may have been moved by the
//! listener (the one unclaimed file of its size and fingerprint); one whose
//! file is a cut run's write target is the software's own half-finished
//! work (2.12.3).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io;

use super::super::contract::fat::{skew, time_matches};
use super::super::contract::mptg::{
    FLAGS_V1, FOLDER_OWNED, FOLDER_THUMB, KNOWN_RULES1, Ledger, ORIGIN_FLAGS_V1, READ_RULES, Record,
    SOURCE_SOFTWARE,
};
use super::super::contract::mspd::{OP_FOLDER, OP_WRITE};
use super::super::contract::text::Run;
use super::super::names::match_key;
use super::super::tags::PARSER_VERSION;
use super::super::volume::{CardFs, Item};
use super::settle::{Settled, card_qfp};
use super::Knobs;

/// The deepest folder a listing walks into: the device lists 8 levels
/// below `/music`; this only bounds a card made strangely.
const WALK_DEPTH: usize = 32;

/// `/music` as the card lists it.
pub(super) struct Music {
    /// The folder's own spelling at the volume's root (`music`, `Music`);
    /// None when the card has none.
    pub(super) top: Option<String>,
    /// Each folder's entries, by its path relative to `/music` (`""` is
    /// `/music` itself).
    pub(super) dirs: BTreeMap<String, Vec<Item>>,
}

/// A path below a folder (`""` for `/music`).
pub(super) fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

/// A path's names by their match keys: one entry to FAT.
pub(super) fn key_path(path: &str) -> Vec<String> {
    path.split('/').filter(|n| !n.is_empty()).map(match_key).collect()
}

impl Music {
    pub(super) fn list(card: &dyn CardFs) -> io::Result<Music> {
        let top = card.list("/")?.into_iter().find(|i| i.dir && i.name.eq_ignore_ascii_case("music"));
        let mut music = Music { top: top.map(|i| i.name), dirs: BTreeMap::new() };
        if music.top.is_none() {
            return Ok(music);
        }
        let mut stack = vec![(String::new(), 0usize)];
        while let Some((rel, depth)) = stack.pop() {
            // By name: what a run does never hangs on the order an OS lists.
            let mut items = card.list(&music.card_path(&rel))?;
            items.sort_by(|a, b| a.name.cmp(&b.name));
            for item in items.iter().filter(|i| i.dir && depth + 1 < WALK_DEPTH) {
                stack.push((join(&rel, &item.name), depth + 1));
            }
            music.dirs.insert(rel, items);
        }
        Ok(music)
    }

    /// The card path of a path relative to `/music`.
    pub(super) fn card_path(&self, rel: &str) -> String {
        let top = self.top.as_deref().unwrap_or("music");
        if rel.is_empty() { format!("/{top}") } else { format!("/{top}/{rel}") }
    }

    /// `rel` as the card lists it, a file or a folder, matched one name at a
    /// time from `/music` down: the entry spelled alike, else the one entry
    /// of the same match key. Several of that key: no match.
    pub(super) fn find(&self, rel: &str, dir: bool) -> Option<(String, Item)> {
        let names: Vec<&str> = rel.split('/').filter(|n| !n.is_empty()).collect();
        let mut at = String::new();
        let mut found: Option<Item> = None;
        for (k, name) in names.iter().enumerate() {
            let want_dir = dir || k + 1 < names.len();
            let items = self.dirs.get(&at)?;
            let kind = |i: &&Item| i.dir == want_dir;
            let item = match items.iter().filter(kind).find(|i| i.name == *name) {
                Some(item) => item,
                None => {
                    let key = match_key(name);
                    let mut same = items.iter().filter(kind).filter(|i| match_key(&i.name) == key);
                    match (same.next(), same.next()) {
                        (Some(item), None) => item,
                        _ => return None,
                    }
                }
            };
            at = join(&at, &item.name);
            found = Some(item.clone());
        }
        found.map(|item| (at, item))
    }

    /// Every file, with its path relative to `/music`.
    pub(super) fn files(&self) -> impl Iterator<Item = (String, &Item)> {
        self.dirs.iter().flat_map(|(dir, items)| {
            items.iter().filter(|i| !i.dir).map(move |i| (join(dir, &i.name), i))
        })
    }

    /// Every folder below `/music`, relative to it.
    pub(super) fn folders(&self) -> impl Iterator<Item = &String> {
        self.dirs.keys().filter(|k| !k.is_empty())
    }

    /// A folder is there, under any case.
    pub(super) fn has_folder(&self, rel: &str) -> bool {
        rel.is_empty() || self.find(rel, true).is_some()
    }
}

/// A ledger row's card side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Side {
    /// Its file, unchanged, at its listed spelling, with this run's stamp.
    Same { path: String, fat_time: u32 },
    /// Its file, moved by the listener (U4): found elsewhere by its size and
    /// fingerprint.
    Moved { path: String, fat_time: u32 },
    /// Changed by the listener: the listener's from now on (U5).
    Changed { path: String },
    /// Not on the card.
    Missing,
    /// A cut run's write target that no longer matches its row (2.12.3): the
    /// software's half-finished work, to replace in place or delete.
    Ours { path: String },
}

/// A row of the committed ledger and what the card has of it.
#[derive(Clone, Debug)]
pub(super) struct Row {
    /// Its path as the tags file has it.
    pub(super) path: String,
    /// The record and run as the tags file has them, every flag bit v1
    /// doesn't define dropped (2.4.5).
    pub(super) rec: Record,
    pub(super) run: Run,
    pub(super) ledger: Ledger,
    pub(super) side: Side,
}

/// What the listing and the matching found.
pub(super) struct Survey {
    pub(super) rows: Vec<Row>,
    /// The tags file's records aren't this reader's: another parserVersion
    /// or readRules, or the device's reader (the sync agent's file, source
    /// 3, whatever version it calls its own). Every carried record is read
    /// again from its card file (2.6.1). The device's records are no
    /// stand-in for this reader's under the same readRules: its MP3 lengths
    /// are trimmed, it reads the files lofty fails on, and it has known
    /// differences besides (2.17, item 3), so the device's turn costs the
    /// next run a reading of every file.
    pub(super) reread: bool,
    /// Files no row claims: the listener's, each with its listed path.
    pub(super) unclaimed: Vec<(String, Item)>,
    /// A cut run's write targets on the card that no row describes: its
    /// half-finished work (2.11, 2.12.3).
    pub(super) recover: Vec<String>,
    /// The folders the software owns (OWNED, or a cut run's folder op that
    /// was made), as listed.
    pub(super) owned: BTreeSet<String>,
    /// The album folders with a thumbnail (THUMB), as listed.
    pub(super) thumbs: BTreeSet<String>,
    /// The tags file's skew over this listing (2.3.4), in seconds; 0 none.
    pub(super) skew: i32,
}

pub(super) fn survey(
    card: &dyn CardFs,
    music: &Music,
    settled: &Settled,
    knobs: &Knobs,
) -> io::Result<Survey> {
    let root = settled.root.as_ref();
    // A tags file with no ledger (the empty first commit has none: no
    // records) gives no rows, but its OWNED folders are still the software's.
    let whole = root.and_then(|r| r.tags.as_ref().ok());
    let tags = whole.filter(|t| t.has_ledger);
    let targets: HashSet<Vec<String>> = settled
        .pending
        .iter()
        .flat_map(|p| p.ops.iter().filter(|o| o.op == OP_WRITE).map(|o| key_path(&o.path)))
        .collect();

    // Each row, matched by path.
    let mut rows = Vec::new();
    let mut found: Vec<Option<(String, Item)>> = Vec::new();
    for e in tags.iter().flat_map(|t| t.records.iter()) {
        let Some(ledger) = &e.ledger else { continue };
        let rec = Record { flags: e.rec.flags & FLAGS_V1, known: e.rec.known & KNOWN_RULES1, ..e.rec };
        let ledger = Ledger { origin_flags: ledger.origin_flags & ORIGIN_FLAGS_V1, ..ledger.clone() };
        found.push(music.find(&e.path, false));
        rows.push(Row { path: e.path.clone(), rec, run: e.run.clone(), ledger, side: Side::Missing });
    }
    let pairs = rows.iter().zip(&found).filter_map(|(row, f)| {
        let (_, item) = f.as_ref()?;
        (item.size == row.rec.size as u64).then_some((row.rec.fat_time, item.fat_time))
    });
    let skew = skew(pairs.collect::<Vec<_>>());

    let mut claimed: HashSet<String> = HashSet::new();
    for (row, f) in rows.iter_mut().zip(found) {
        let Some((path, item)) = f else { continue };
        claimed.insert(path.clone());
        let size = item.size == row.rec.size as u64;
        let printed = |card: &dyn CardFs| card_qfp(card, &music.card_path(&path), item.size);
        let same = size
            && (time_matches(row.rec.fat_time, item.fat_time, skew)
                || row.rec.qfp != 0 && printed(card)? == row.rec.qfp);
        row.side = match (same, targets.contains(&key_path(&path))) {
            (true, _) => Side::Same { path, fat_time: item.fat_time },
            (false, true) => Side::Ours { path },
            (false, false) => Side::Changed { path },
        };
    }

    // A file gone from its place may be one the listener moved: the one
    // unclaimed file of its size whose fingerprint is its own.
    let unclaimed = music.files().filter(|(p, _)| !claimed.contains(p));
    let mut loose: Vec<(String, Item)> = unclaimed.map(|(p, i)| (p, i.clone())).collect();
    if knobs.follow_moves {
        let mut prints: HashMap<String, u64> = HashMap::new();
        for row in rows.iter_mut().filter(|r| r.side == Side::Missing && r.rec.qfp != 0) {
            let mut hits = Vec::new();
            for (path, item) in loose.iter().filter(|(_, i)| i.size == row.rec.size as u64) {
                let print = match prints.get(path) {
                    Some(&p) => p,
                    None => {
                        let p = card_qfp(card, &music.card_path(path), item.size)?;
                        prints.insert(path.clone(), p);
                        p
                    }
                };
                if print == row.rec.qfp {
                    hits.push((path.clone(), item.fat_time));
                }
            }
            if let [(path, fat_time)] = hits.as_slice() {
                loose.retain(|(p, _)| p != path);
                row.side = Side::Moved { path: path.clone(), fat_time: *fat_time };
            }
        }
    }
    let (recover, unclaimed): (Vec<_>, Vec<_>) =
        loose.into_iter().partition(|(p, _)| targets.contains(&key_path(p)));
    let recover = recover.into_iter().map(|(p, _)| p).collect();

    // The folders the software owns, and the albums with a thumbnail, as
    // the card spells them now: a cut run's folder ops become OWNED.
    let mut owned = BTreeSet::new();
    let mut thumbs = BTreeSet::new();
    for f in whole.iter().flat_map(|t| t.folders.iter()).filter(|f| !f.path.is_empty()) {
        let Some((path, _)) = music.find(&f.path, true) else { continue };
        if f.flags & FOLDER_OWNED != 0 {
            owned.insert(path.clone());
        }
        if f.flags & FOLDER_THUMB != 0 {
            thumbs.insert(path);
        }
    }
    for op in settled.pending.iter().flat_map(|p| p.ops.iter()).filter(|o| o.op == OP_FOLDER) {
        if let Some((path, _)) = music.find(&op.path, true) {
            owned.insert(path);
        }
    }
    let reread = tags.is_some_and(|t| {
        let reader = (t.source, t.parser_version, t.read_rules);
        reader != (SOURCE_SOFTWARE, PARSER_VERSION, READ_RULES)
    });
    Ok(Survey { rows, reread, unclaimed, recover, owned, thumbs, skew })
}
