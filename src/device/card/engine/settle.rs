//! Steps 1-4 of a run (docs/METADATA.md 2.12.1), before it plans anything:
//! the root that counts settled (a `manifest.tmp` that won renamed into
//! place), the plan of a cut run settled and read (one of another card's
//! id dropped: it owns nothing here), the private files read by their
//! recovery rule (2.12.7), the cut renames looked for, and the leftovers
//! cleaned up — every stage file, every `*.tmp`, every companion the root
//! doesn't name, every partial `resume.json` doesn't keep (2.12.8). The
//! private files stay. [`newer`] is 2.4.5's check, made before the run
//! writes anything at all.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;

use serde_json::Value;

use super::super::contract::MAX_FILE;
use super::super::contract::container::unknown_major;
use super::super::contract::device_txt::DeviceTxt;
use super::super::contract::msmf::{self, Pick};
use super::super::contract::mptg::Tags;
use super::super::contract::mspd::{self, OP_WRITE};
use super::super::contract::prim::{qfp, qfp_ranges};
use super::super::volume::{self, CardFs, CardWrite, Item, Private, RESUME_VERSION, Root, STATE_VERSION};
use super::bind::Bound;
use super::survey::key_path;
use super::{Answers, Guard, Stop};

/// What steps 1 and 2 settled.
pub(super) struct Settled {
    /// The root that counts, renamed into place, with its tags file's
    /// verdict (2.5.3, 2.5.4).
    pub(super) root: Option<Root>,
    /// The Auto DJ index the root names, when it is valid: its bytes.
    pub(super) autodj: Option<Vec<u8>>,
    /// The highest generation the card shows, by its root or by a file's
    /// name (2.5.2: a name is never reused).
    pub(super) top: u32,
    /// A cut run's plan (2.12.5).
    pub(super) pending: Option<mspd::Plan>,
    /// The card id of a plan of another card, deleted at step 4.
    pub(super) foreign: Option<u64>,
    /// `state.json` (2.12.7): the card's settings, for the caller.
    pub(super) state: Private,
}

/// A path inside `/.mstream`.
pub(super) fn ms(name: &str) -> String {
    format!("/.mstream/{name}")
}

/// A companion's name, generation and all (2.4.3's COMP names): `tags-` or
/// `autodj-`, 8 lowercase hex digits, `.bin`. Its generation.
pub(super) fn companion_generation(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("tags-").or_else(|| name.strip_prefix("autodj-"))?;
    let hex = rest.strip_suffix(".bin")?;
    let lower = hex.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
    (hex.len() == 8 && lower).then(|| u32::from_str_radix(hex, 16).ok()).flatten()
}

/// The entries of a folder of the card, by name; none when it isn't there.
pub(super) fn entries(card: &dyn CardFs, dir: &str) -> io::Result<Vec<Item>> {
    match card.list(dir) {
        Ok(mut items) => {
            items.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(items)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// 2.4.5's writer guard, read only: every file in `/.mstream` of a contract
/// name whose major this software doesn't know (its first 6 bytes alone),
/// a plan with an op of a newer writer, a private file of a newer version
/// (2.12.7). Any of them: the software writes nothing to the card.
pub(super) fn newer(card: &dyn CardFs) -> io::Result<Vec<String>> {
    let mut out = Vec::new();
    let files: Vec<Item> = entries(card, "/.mstream")?.into_iter().filter(|i| !i.dir).collect();
    for item in &files {
        let lower = item.name.to_ascii_lowercase();
        if volume::contract_named(&lower) && unknown_major(&card.read_at(&ms(&item.name), 0, 6)?) {
            out.push(item.name.clone());
        }
    }
    for name in ["pending.bin", "pending.tmp"] {
        let plan = card.read(&ms(name), MAX_FILE)?.and_then(|b| mspd::Plan::read(&b).ok());
        if plan.is_some_and(|p| p.stops_software()) {
            out.push(name.to_string());
        }
    }
    for (x, known) in [("state", STATE_VERSION), ("resume", RESUME_VERSION)] {
        let json = card.read(&ms(&format!("{x}.json")), MAX_FILE)?;
        let new = card.read(&ms(&format!("{x}.new")), MAX_FILE)?;
        let p = volume::settle_private(json.as_deref(), new.as_deref(), x, known);
        if let Some(file) = p.file.filter(|_| p.version != known) {
            out.push(file);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// `/.player/device.txt`, or its no-file values (2.15). The software reads
/// it and nothing else there (2.11).
pub(super) fn device_txt(card: &dyn CardFs) -> DeviceTxt {
    DeviceTxt::read(card.read("/.player/device.txt", 64 * 1024).ok().flatten().as_deref())
}

/// The card's reads for the settling: a failure kept for after the
/// election, which takes a reader that can't fail.
struct Reads<'c> {
    card: &'c dyn CardWrite,
    error: RefCell<Option<io::Error>>,
}

impl Reads<'_> {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        match self.card.read(&ms(name), MAX_FILE) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.error.borrow_mut().get_or_insert(e);
                None
            }
        }
    }

    fn done(&self, bound: &mut Bound) -> Result<(), Stop> {
        match self.error.borrow_mut().take() {
            Some(e) => Err(bound.failed(e)),
            None => Ok(()),
        }
    }
}

/// Finishes a rename that a cut left half done (2.12.1, 2.12.7): the
/// file it replaces deleted, then the rename.
fn finish(bound: &mut Bound, from: &str, to: &str) -> Result<(), Stop> {
    let card = bound.alive()?;
    let there = card.stat(&ms(to)).map_err(|e| bound.failed(e))?;
    if there.is_some() {
        bound.alive()?.delete(&ms(to)).map_err(|e| bound.failed(e))?;
    }
    bound.alive()?.rename(&ms(from), &ms(to)).map_err(|e| bound.failed(e))
}

fn delete(bound: &mut Bound, path: &str) -> Result<(), Stop> {
    bound.alive()?.delete(path).map_err(|e| bound.failed(e))
}

/// Steps 1-4. A cut rename found stops with [`Guard::DiskCheck`] before
/// anything is deleted, unless the listener has run the disk check since.
pub(super) fn settle(card: &dyn CardWrite, bound: &mut Bound, answers: &Answers) -> Result<Settled, Stop> {
    let reads = Reads { card, error: RefCell::new(None) };
    let mut pairs: Vec<(String, String)> = Vec::new();

    // Step 1: the root. One commit seen twice is touched by no one until a
    // disk check has run; a manifest.tmp that won is renamed into place.
    let mut root = volume::settle_root(&|name| reads.read(name));
    reads.done(bound)?;
    if let Some(r) = root.as_mut() {
        if r.twice {
            pairs.push((ms("manifest.tmp"), ms("manifest.bin")));
        } else if r.file == "manifest.tmp" {
            finish(bound, "manifest.tmp", "manifest.bin")?;
            r.file = "manifest.bin";
        }
    }
    let autodj = root
        .as_ref()
        .filter(|r| r.autodj == Some(Ok(())))
        .and_then(|r| r.manifest.autodj.as_ref())
        .and_then(|c| reads.read(&c.name()));
    reads.done(bound)?;
    let names: Vec<Item> = entries(card, "/.mstream").map_err(|e| bound.failed(e))?;
    let named = names.iter().filter_map(|i| companion_generation(&i.name.to_ascii_lowercase()));
    let highest = named.chain(root.as_ref().map(|r| r.manifest.header.generation)).max().unwrap_or(0);

    // Step 2: the plan, read before step 4 deletes any *.tmp; then the
    // private files, each by its recovery rule.
    let (bin, tmp) = (reads.read("pending.bin"), reads.read("pending.tmp"));
    reads.done(bound)?;
    let plan_of = |b: &Option<Vec<u8>>| b.as_ref().and_then(|b| mspd::Plan::read(b).ok());
    let (bin_plan, tmp_plan) = (plan_of(&bin), plan_of(&tmp));
    let pending = if bin.is_some() && bin == tmp {
        pairs.push((ms("pending.tmp"), ms("pending.bin")));
        bin_plan
    } else {
        let candidate = |p: &Option<mspd::Plan>| p.as_ref().map(mspd::Plan::candidate);
        match msmf::elect(candidate(&bin_plan), candidate(&tmp_plan)).pick {
            Pick::Bin => bin_plan,
            Pick::Tmp => {
                finish(bound, "pending.tmp", "pending.bin")?;
                tmp_plan
            }
            Pick::None => None,
        }
    };
    // A plan of another card (a copied `/.mstream`, a restored image): no
    // target of it is this card's half-finished work (2.11), so it is no
    // plan here, and goes at step 4. One of a card with no root yet can't
    // be told apart, and is taken as this card's.
    let root_id = root.as_ref().map_or(0, |r| r.manifest.header.card_id);
    let other = |id: u64| id != 0 && root_id != 0 && id != root_id;
    let foreign = pending.as_ref().map(|p| p.header.card_id).filter(|&id| other(id));
    let pending = pending.filter(|_| foreign.is_none());
    let mut private = |x: &str, known: u64, pairs: &mut Vec<_>| -> Result<Private, Stop> {
        let (json_name, new_name) = (format!("{x}.json"), format!("{x}.new"));
        let (json, new) = (reads.read(&json_name), reads.read(&new_name));
        reads.done(bound)?;
        let mut p = volume::settle_private(json.as_deref(), new.as_deref(), x, known);
        if p.twice {
            pairs.push((ms(&new_name), ms(&json_name)));
        } else if p.file.as_deref() == Some(new_name.as_str()) {
            finish(bound, &new_name, &json_name)?;
            p.file = Some(json_name);
        } else if p.file.is_some() && new.is_some() {
            delete(bound, &ms(&new_name))?;
        }
        Ok(p)
    };
    let state = private("state", STATE_VERSION, &mut pairs)?;
    let resume = private("resume", RESUME_VERSION, &mut pairs)?;

    // Step 3: the cut renames.
    let tags = root.as_ref().and_then(|r| r.tags.as_ref().ok());
    pairs.extend(cut_renames(card, pending.as_ref(), tags).map_err(|e| bound.failed(e))?);
    if !pairs.is_empty() && !answers.disk_checked {
        return Err(Stop::Guard(Guard::DiskCheck(pairs)));
    }

    // Step 4: the clean-up.
    let manifest = root.as_ref().map(|r| &r.manifest);
    let tags_name = manifest.map(|m| m.tags.name());
    let autodj_name = manifest.and_then(|m| m.autodj.as_ref()).map(|c| c.name());
    let named: HashSet<String> = tags_name.into_iter().chain(autodj_name).collect();
    // Listed again: steps 1 and 2 may have renamed a .tmp into place.
    let top = entries(card, "/.mstream").map_err(|e| bound.failed(e))?;
    clean(card, bound, &top, &named, &resume_parts(resume.value.as_ref()))?;
    if foreign.is_some() && card.stat(&ms("pending.bin")).map_err(|e| bound.failed(e))?.is_some() {
        delete(bound, &ms("pending.bin"))?;
    }
    Ok(Settled { root, autodj, top: highest, pending, foreign, state })
}

/// Two files a cut rename may have left on one cluster chain: both there,
/// the same size and the same fingerprint (2.12.1, "Cut renames").
fn twins(card: &dyn CardFs, a: &str, b: &str) -> io::Result<bool> {
    let (Some(x), Some(y)) = (card.stat(a)?, card.stat(b)?) else { return Ok(false) };
    if x.dir || y.dir || x.size != y.size {
        return Ok(false);
    }
    Ok(card_qfp(card, a, x.size)? == card_qfp(card, b, y.size)?)
}

/// A card file's quick fingerprint (2.3.5): two reads.
pub(super) fn card_qfp(card: &dyn CardFs, path: &str, size: u64) -> io::Result<u64> {
    let r = qfp_ranges(size);
    let head = card.read_at(path, 0, r.head as usize)?;
    let tail = card.read_at(path, r.tail_offset, r.tail as usize)?;
    if head.len() as u64 != r.head || tail.len() as u64 != r.tail {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the file is shorter than listed"));
    }
    Ok(qfp(size, &head, &tail))
}

/// Step 3's pairs, besides the root's and the private files' (2.12.1,
/// 2.12.8): a plan's write op n whose `stage/<n>.tmp` and target are twins;
/// a partial in `resume/` and a write target of its size that are; a
/// thumbnail's `.tmp` and `.565` that are byte-equal. And one the contract
/// doesn't list, since only this writer makes the rename: a file the
/// server moved, renamed on the card from its ledger path to a write
/// target. A target that isn't a ledger path, with the size and the
/// fingerprint of a ledger file that is still there, may be that rename
/// cut short.
fn cut_renames(
    card: &dyn CardFs,
    pending: Option<&mspd::Plan>,
    tags: Option<&Tags>,
) -> io::Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    if let Some(plan) = pending {
        let ops = plan.ops.iter().enumerate();
        let writes: Vec<(usize, &mspd::Op)> = ops.filter(|(_, o)| o.op == OP_WRITE).collect();
        for (n, op) in &writes {
            let (stage, target) = (ms(&format!("stage/{n:08x}.tmp")), format!("/music/{}", op.path));
            if twins(card, &stage, &target)? {
                pairs.push((stage, target));
            }
        }
        let records = tags.map_or(&[][..], |t| t.records.as_slice());
        let listed: HashSet<Vec<String>> = records.iter().map(|e| key_path(&e.path)).collect();
        for (_, op) in writes.iter().filter(|(_, o)| !listed.contains(&key_path(&o.path))) {
            let target = format!("/music/{}", op.path);
            let Some(item) = card.stat(&target)?.filter(|i| !i.dir) else { continue };
            let same = records.iter().filter(|e| e.rec.size as u64 == item.size && e.rec.qfp != 0);
            let mut print = None;
            for e in same {
                let source = format!("/music/{}", e.path);
                if card.stat(&source)?.is_none_or(|i| i.dir || i.size != item.size) {
                    continue;
                }
                let target_print = match print {
                    Some(p) => p,
                    None => *print.insert(card_qfp(card, &target, item.size)?),
                };
                if target_print == e.rec.qfp && card_qfp(card, &source, item.size)? == e.rec.qfp {
                    pairs.push((source, target.clone()));
                }
            }
        }
        for part in entries(card, "/.mstream/resume")?.into_iter().filter(|i| !i.dir) {
            let path = ms(&format!("resume/{}", part.name));
            for (_, op) in writes.iter().filter(|(_, o)| o.expected_size as u64 == part.size) {
                let target = format!("/music/{}", op.path);
                if twins(card, &path, &target)? {
                    pairs.push((path.clone(), target));
                }
            }
        }
    }
    for folder in entries(card, "/.mstream/thumbs")?.into_iter().filter(|i| i.dir) {
        let dir = ms(&format!("thumbs/{}", folder.name));
        let files = entries(card, &dir)?;
        for tmp in files.iter().filter(|i| !i.dir && i.name.to_ascii_lowercase().ends_with(".tmp")) {
            let stem = &tmp.name[..tmp.name.len() - 4];
            let done = format!("{stem}.565");
            let Some(done) = files.iter().find(|i| !i.dir && i.name.eq_ignore_ascii_case(&done)) else {
                continue;
            };
            let (a, b) = (format!("{dir}/{}", tmp.name), format!("{dir}/{}", done.name));
            let equal = tmp.size == done.size && card.read(&a, MAX_FILE)? == card.read(&b, MAX_FILE)?;
            if equal {
                pairs.push((a, b));
            }
        }
    }
    Ok(pairs)
}

/// The partials a valid `resume.json` keeps (2.12.8): each listed name of a
/// partial (`<8 hex>.part`) with its size. An entry that isn't one keeps
/// nothing, and a name listed twice keeps nothing.
pub(super) fn resume_parts(value: Option<&Value>) -> HashMap<String, u64> {
    let mut out: HashMap<String, u64> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let parts = value.and_then(|v| v.get("parts")).and_then(Value::as_array);
    for p in parts.into_iter().flatten() {
        let name = p.get("name").and_then(Value::as_str).filter(|n| part_name(n));
        let size = p.get("size").and_then(Value::as_u64);
        let (Some(name), Some(size)) = (name, size) else { continue };
        if !seen.insert(name.to_string()) {
            out.remove(name);
            continue;
        }
        out.insert(name.to_string(), size);
    }
    out
}

/// A partial's name: 8 lowercase hex digits and `.part`.
fn part_name(name: &str) -> bool {
    let hex = name.strip_suffix(".part").unwrap_or("");
    hex.len() == 8 && hex.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Step 4 (vectors.json's privateFiles.cleanup): everything in `stage/`,
/// every `*.tmp` at `/.mstream`'s top and under `thumbs/`, every companion
/// the root doesn't name, and in `resume/` every file the settled
/// `resume.json` doesn't keep or that is longer than its entry, and every
/// folder with what it holds. The private files and folders stay.
fn clean(
    card: &dyn CardWrite,
    bound: &mut Bound,
    top: &[Item],
    named: &HashSet<String>,
    keep: &HashMap<String, u64>,
) -> Result<(), Stop> {
    for item in top {
        let (lower, path) = (item.name.to_ascii_lowercase(), ms(&item.name));
        if !item.dir {
            let companion = companion_generation(&lower).is_some() && !named.contains(&lower);
            if lower.ends_with(".tmp") || companion {
                delete(bound, &path)?;
            }
            continue;
        }
        match lower.as_str() {
            "stage" => empty(card, bound, &path, &|_| true)?,
            "thumbs" => {
                empty(card, bound, &path, &|i: &Item| i.name.to_ascii_lowercase().ends_with(".tmp"))?
            }
            "resume" => {
                for part in entries(card, &path).map_err(|e| bound.failed(e))? {
                    let inner = format!("{path}/{}", part.name);
                    let kept = keep.get(&part.name).is_some_and(|&size| part.size <= size);
                    if part.dir {
                        empty(card, bound, &inner, &|_| true)?;
                        bound.alive()?.rmdir(&inner).map_err(|e| bound.failed(e))?;
                    } else if !kept {
                        delete(bound, &inner)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Deletes the files of a folder, at every depth, that `gone` picks; a
/// subfolder left empty by a delete-everything goes too.
fn empty(
    card: &dyn CardWrite,
    bound: &mut Bound,
    dir: &str,
    gone: &dyn Fn(&Item) -> bool,
) -> Result<(), Stop> {
    for item in entries(card, dir).map_err(|e| bound.failed(e))? {
        let path = format!("{dir}/{}", item.name);
        if item.dir {
            empty(card, bound, &path, gone)?;
            let left = entries(card, &path).map_err(|e| bound.failed(e))?;
            if left.is_empty() && gone(&item) {
                bound.alive()?.rmdir(&path).map_err(|e| bound.failed(e))?;
            }
        } else if gone(&item) {
            delete(bound, &path)?;
        }
    }
    Ok(())
}
