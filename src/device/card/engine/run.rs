//! A run (docs/METADATA.md 2.12.1, steps 6-13, and the transfer plan's
//! 2.3, step 16): the plan written to `pending.bin` first, then the deletes,
//! then each file — fetched into `stage/<n>.tmp` (n its op's index in the
//! plan), checked, renamed into `/music` over the file it replaces (that one
//! deleted first: a FAT rename never replaces), and read back as the card
//! stores it, its record read from those bytes. A checkpoint every 500 files
//! or 5 minutes commits what is done; at the end the thumbnails, the Auto DJ
//! index and the final commit, the leftovers collected, and the card read
//! back as the device will read it.
//!
//! A file a later run may take up (2.12.8) is fetched into its partial in
//! `resume/` instead — its own listed one taken up, or a new one — flushed
//! and listed in `resume.json` as it grows and when its fetch stops short,
//! and renamed, finished and checked, straight into `/music`; the
//! partials no plan wants any more are collected at the end.
//!
//! Every write is preceded by the binding's cheap check, and every delete
//! batch, rename into `/music` and commit by its whole one; and every path
//! of `/music` the run deletes or replaces is one its plan owns (2.11): a
//! ledger file it matched, or a cut run's target — and goes only while it
//! is still the file the listing showed. A new name is never a delete: any
//! file found there is a collision, and that one file waits.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{self, Seek as _, SeekFrom, Write as _};
use std::time::Instant;

use super::super::contract::container::MPTG;
use super::super::contract::mpdj::{self, Table};
use super::super::contract::mptg::{
    self, BPM_ANALYSED, CONTAINER_NOT_AUDIO, FOLDER_OWNED, FOLDER_THUMB, FolderIn, Ledger, Meta,
    READ_RULES, RecordIn, SOURCE_SOFTWARE, UNREADABLE,
};
use super::super::contract::msmf::{
    self, Ack, Comp, FINAL, Manifest, ManifestIn, Pick, companion_name,
};
use super::super::contract::mspd::{self, OP_DELETE, OP_FOLDER, OP_WRITE, Op, PlanIn};
use super::super::contract::mpth;
use super::super::contract::order::{
    album_folder, compare_file_paths, compare_folder_paths, split_last, thumb_path, thumb_path_of,
};
use super::super::contract::prim::{Md5Sum, hash_sampled, path_hash};
use super::super::contract::MAX_FILE;
use super::super::contract::server::{Server, Servers};
use super::super::contract::text::Run;
use super::super::names::match_key;
use super::super::tags::{self, PARSER_VERSION};
use super::super::volume::{CardFile, CardFs, Item, next_seq};
use super::plan::{Plan, Replaced, Write, origin_flags};
use super::resume::{Entry, same_server, take_up};
use super::settle::{card_qfp, companion_generation, entries, ms};
use super::survey::{Music, join, key_path};
use super::{
    Album, AutoDj, Commit, Cover, Event, Fetcher, Have, Hooks, Outcome, Part, Report, Session, Stop,
    WantedFile,
};

/// A record of the next commit, by its path.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Owned {
    rec: mptg::Record,
    run: Run,
    ledger: Ledger,
    /// Its server, in the plan's numbering.
    server: usize,
}

/// How one file of step 8 ended.
enum Step {
    Done,
    /// Skipped, and reported; the run goes on.
    Skipped,
    /// The source stops the run: it parks.
    Park(String),
}

impl Session<'_> {
    /// Runs a plan made in this session, its guards all answered. The card
    /// is let go at the end (run.lock deleted), and when the run stops on a
    /// card that is still the bound one; a pulled or different card is
    /// never written again. A run that ends writes the card's settings
    /// first, when [`Hooks::state`] gives them: in this session, so no
    /// second one lists the card again for one private file.
    pub(crate) fn run(
        mut self,
        plan: Plan,
        fetcher: &mut dyn Fetcher,
        hooks: &mut dyn Hooks,
        events: &mut dyn FnMut(Event),
    ) -> Result<Outcome, Stop> {
        let refused = if plan.token != self.token {
            Some(Stop::StalePlan)
        } else if !plan.guards.is_empty() {
            Some(Stop::Unanswered(plan.guards.clone()))
        } else {
            None
        };
        if let Some(stop) = refused {
            self.bound.release();
            return Err(stop);
        }
        let result = Runner::new(&mut self, &plan).and_then(|mut r| r.go(fetcher, hooks, events));
        let mut gone = false;
        let result = result.map(|mut outcome| {
            let seq = next_seq(self.settled.state.seq);
            if let Some(bytes) = hooks.state(&outcome, seq) {
                let wrote = self.write_state(&bytes);
                gone = matches!(wrote, Err(Stop::DifferentCard | Stop::Pulled));
                outcome.stored = wrote.map_err(|stop| stop.to_string());
            }
            outcome
        });
        match result {
            Err(stop @ (Stop::DifferentCard | Stop::Pulled)) => Err(stop),
            Ok(outcome) if gone => Ok(outcome),
            other => {
                self.bound.release();
                other
            }
        }
    }
}

struct Runner<'r, 'c> {
    s: &'r mut Session<'c>,
    plan: &'r Plan,
    /// The next commit's records, by path.
    ledger: BTreeMap<String, Owned>,
    /// The folders the software owns, and the album folders with a
    /// thumbnail, by path as the card spells them.
    owned: BTreeSet<String>,
    thumbs: BTreeSet<String>,
    /// The root now: its generation (0: none), its tags file, its valid Auto
    /// DJ index (named again by checkpoints), its acknowledgements.
    generation: u32,
    tags_name: Option<String>,
    autodj: Option<Comp>,
    acks: Vec<Ack>,
    /// The next commit's generation.
    next: u32,
    card_id: u64,
    /// The paths of `/music` the run may delete or replace (2.11), by key:
    /// its deletes, the files its writes replace in place, the files it
    /// renames; and those of them it spared (step 7, step 8.3), the
    /// listener's files now.
    owns: HashSet<Vec<String>>,
    spared: HashSet<Vec<String>>,
    /// Each write's index in the plan on the card now (2.12.1, step 8.1).
    index: HashMap<String, u32>,
    /// The writes and folder ops done.
    done: HashSet<usize>,
    /// The card's servers as the run found them: what a partial's server
    /// is tested against (2.11), its peer's parent among them.
    servers: Servers,
    made: BTreeSet<String>,
    /// Files since the last commit, and when it was.
    since: usize,
    last: Instant,
    pending: bool,
    written: usize,
    renamed: usize,
    deleted: usize,
    report: Report,
}

impl<'r, 'c> Runner<'r, 'c> {
    fn new(s: &'r mut Session<'c>, plan: &'r Plan) -> Result<Runner<'r, 'c>, Stop> {
        let root = s.settled.root.as_ref();
        let generation = root.map_or(0, |r| r.manifest.header.generation);
        let next = if root.is_some() { generation + 1 } else { s.settled.top + 1 };
        // A card's first commit mints its id; so does a copied card's new
        // identity, a run of its own: a plan of the new id cut before its
        // first commit would be another card's to the next run (2.11).
        if plan.new_card_id && plan.touches_music() {
            return Err(Stop::Refused("a new card id is a run of its own".into()));
        }
        let old_id = root.map_or(0, |r| r.manifest.header.card_id);
        let card_id = if old_id == 0 || plan.new_card_id { s.env.id() } else { old_id };
        let tags_name = root.map(|r| r.manifest.tags.name());
        // An Auto DJ index carries the card id it was built for: under a new
        // one no commit names the old index again (AUTODJ-INDEX.md 7.3).
        let valid = s.settled.autodj.is_some() && card_id == old_id;
        let autodj = root.and_then(|r| r.manifest.autodj.clone()).filter(|_| valid);
        let acks = root.map(|r| r.manifest.acks.clone()).unwrap_or_default();
        let servers = root.map(|r| r.manifest.server_table()).unwrap_or_default();
        let mut owns: HashSet<Vec<String>> = HashSet::new();
        owns.extend(plan.deletes.iter().map(|d| key_path(&d.path)));
        for w in &plan.writes {
            if w.replaces.is_some() {
                owns.insert(key_path(&w.path));
            }
            if let Some(from) = &w.from {
                owns.insert(key_path(from));
            }
        }
        let mut ledger = BTreeMap::new();
        for k in &plan.keeps {
            let row = &s.survey.rows[k.row];
            let rec = mptg::Record { fat_time: k.fat_time, ..row.rec };
            let (run, server) = (row.run.clone(), k.server);
            ledger.insert(k.path.clone(), Owned { rec, run, ledger: k.ledger.clone(), server });
        }
        let (owned, thumbs) = (s.survey.owned.clone(), s.survey.thumbs.clone());
        let mut runner = Runner {
            s,
            plan,
            ledger,
            owned,
            thumbs,
            generation,
            tags_name,
            autodj,
            acks,
            next,
            card_id,
            owns,
            spared: HashSet::new(),
            index: HashMap::new(),
            done: HashSet::new(),
            servers,
            made: BTreeSet::new(),
            since: 0,
            last: Instant::now(),
            pending: false,
            written: 0,
            renamed: 0,
            deleted: 0,
            report: plan.report.clone(),
        };
        runner.carried()?;
        Ok(runner)
    }

    /// The rows read again when the tags file's reader or rules aren't this
    /// one's (2.6.1), and the adopted files' records (U6).
    fn carried(&mut self) -> Result<(), Stop> {
        if self.s.survey.reread {
            let paths: Vec<String> = self.ledger.keys().cloned().collect();
            for path in paths {
                let old = self.ledger[&path].clone();
                let (size, fat_time) = (old.rec.size as u64, old.rec.fat_time);
                let item = Item { name: String::new(), dir: false, size, fat_time };
                let (rec, run) = self.read_again(&path, &item, &old.rec)?;
                self.ledger.insert(path, Owned { rec, run, ..old });
            }
        }
        let plan = self.plan;
        for a in &plan.adopts {
            let file = &plan.wanted.files[a.file];
            let (size, fat_time) = (file.ledger.server_size, a.fat_time);
            let item = Item { name: String::new(), dir: false, size, fat_time };
            let owned = self.owned_of(&a.path, &item, file, origin_flags(file, false, true))?;
            self.ledger.insert(a.path.clone(), owned);
        }
        Ok(())
    }

    fn go(
        &mut self,
        fetcher: &mut dyn Fetcher,
        hooks: &mut dyn Hooks,
        events: &mut dyn FnMut(Event),
    ) -> Result<Outcome, Stop> {
        // Step 6: the plan on the card before anything is deleted or written,
        // the card checked again first: a commit made since the session
        // opened (another computer's run) stops this one.
        self.s.bound.check(true)?;
        if !self.plan.deletes.is_empty() || !self.plan.writes.is_empty() {
            self.write_pending()?;
        }

        // Step 7: the deletes, each only while its file is still what the
        // listing said.
        let plan = self.plan;
        if !plan.deletes.is_empty() {
            self.s.bound.check(true)?;
        }
        for d in &plan.deletes {
            let path = self.s.music.card_path(&d.path);
            let now = self.s.card.stat(&path).map_err(|e| self.s.bound.failed(e))?;
            let Some(item) = now else { continue };
            if !d.ours && (item.size != d.size || item.fat_time != d.fat_time) {
                self.report.spared.push(d.path.clone());
                self.spared.insert(key_path(&d.path));
                continue;
            }
            self.may(&d.path)?;
            self.s.bound.alive()?.delete(&path).map_err(|e| self.s.bound.failed(e))?;
            self.ledger.remove(&d.path);
            self.deleted += 1;
            events(Event::Deleted(d.path.clone()));
        }

        // Step 8: each file, in the plan's order; the server's own bytes of
        // a file its source says a later run may take up, into a partial.
        let mut order: Vec<usize> = (0..plan.writes.len()).collect();
        order.sort_by(|&a, &b| compare_file_paths(&plan.writes[a].path, &plan.writes[b].path));
        let resumable = |w: &Write| {
            let file = &plan.wanted.files[w.file];
            w.from.is_none() && file.original() && fetcher.resumable(file)
        };
        let resumable: HashSet<usize> =
            (0..plan.writes.len()).filter(|&k| resumable(&plan.writes[k])).collect();
        for k in order {
            if self.due() {
                self.commit(false, hooks, events)?;
            }
            let w = &plan.writes[k];
            let step = match &w.from {
                Some(from) => self.rename(w, from, events)?,
                None if resumable.contains(&k) => self.fetch_part(w, fetcher, hooks, events)?,
                None => self.fetch(w, fetcher, hooks, events)?,
            };
            // A parked write stays in the plan the checkpoint leaves on the
            // card (2.12.8): its target, if a cut run's, the software's.
            match step {
                Step::Done => {
                    self.done.insert(k);
                    self.since += 1;
                }
                Step::Skipped => {
                    self.done.insert(k);
                }
                Step::Park(why) => {
                    self.commit(false, hooks, events)?;
                    return Ok(self.outcome(Some(why), Ok(())));
                }
            }
        }

        // Steps 10-12: the thumbnails, the Auto DJ index, the final commit;
        // then the plan goes. The OWNED folders left empty go just before
        // the commit, so it lists no folder that is gone.
        self.thumbnails(hooks)?;
        self.prune()?;
        self.commit(true, hooks, events)?;
        if self.pending || self.s.settled.pending.is_some() {
            let card = self.s.bound.alive()?;
            if card.stat(&ms("pending.bin")).map_err(|e| self.s.bound.failed(e))?.is_some() {
                self.s.bound.alive()?.delete(&ms("pending.bin")).map_err(|e| self.s.bound.failed(e))?;
            }
        }
        // Step 13 and the last look.
        self.collect()?;
        let verified = self.verify();
        Ok(self.outcome(None, verified))
    }

    fn outcome(&self, parked: Option<String>, verified: Result<(), String>) -> Outcome {
        Outcome {
            generation: self.generation,
            card_id: self.card_id,
            commit_id: self.s.bound.id.commit_id,
            records: self.ledger.len(),
            written: self.written,
            renamed: self.renamed,
            deleted: self.deleted,
            parked,
            report: self.report.clone(),
            verified,
            stored: Ok(()),
        }
    }

    /// A checkpoint is due (2.12.1, step 9): 500 files, or 5 minutes.
    fn due(&self) -> bool {
        let k = &self.s.knobs;
        let late = self.last.elapsed().as_secs() >= k.checkpoint_secs;
        self.since > 0 && (self.since >= k.checkpoint_files || late)
    }

    /// A path of `/music` the run may delete or replace (2.11).
    fn may(&self, path: &str) -> Result<(), Stop> {
        match self.owns.contains(&key_path(path)) {
            true => Ok(()),
            false => Err(Stop::Refused(format!("{path} isn't the run's to change"))),
        }
    }

    fn failed(&mut self, e: io::Error) -> Stop {
        self.s.bound.failed(e)
    }

    /// A file of `/.mstream` written whole and flushed to the card.
    fn put(&mut self, path: &str, bytes: &[u8]) -> Result<(), Stop> {
        let card = self.s.bound.alive()?;
        let mut file = card.create(path).map_err(|e| self.failed(e))?;
        let wrote = file.write_all(bytes).and_then(|()| file.sync());
        drop(file);
        wrote.map_err(|e| self.failed(e))
    }

    /// `from` renamed to `to` inside `/.mstream`, the file there deleted
    /// first (2.12.1: a rename never replaces).
    fn replace(&mut self, from: &str, to: &str) -> Result<(), Stop> {
        let card = self.s.bound.alive()?;
        if card.stat(to).map_err(|e| self.failed(e))?.is_some() {
            self.s.bound.alive()?.delete(to).map_err(|e| self.failed(e))?;
        }
        self.s.bound.alive()?.rename(from, to).map_err(|e| self.failed(e))
    }

    /// The plan as it stands written to `pending.bin`, through `pending.tmp`
    /// (2.12.5): the deletes not made, the folders not made, the writes not
    /// done. Each write's index in it names its stage file.
    fn write_pending(&mut self) -> Result<(), Stop> {
        let mut ops: Vec<Op> = Vec::new();
        if !self.pending {
            let delete =
                |d: &super::plan::Delete| Op { op: OP_DELETE, path: d.path.clone(), expected_size: 0 };
            ops.extend(self.plan.deletes.iter().map(delete));
        }
        let folders = self.plan.folders.iter().filter(|f| !self.made.contains(*f));
        ops.extend(folders.map(|f| Op { op: OP_FOLDER, path: f.clone(), expected_size: 0 }));
        for (_, w) in self.plan.writes.iter().enumerate().filter(|(k, _)| !self.done.contains(k)) {
            let size = self.plan.wanted.files[w.file].size.min(u32::MAX as u64) as u32;
            ops.push(Op { op: OP_WRITE, path: w.path.clone(), expected_size: size });
        }
        let ops = pend_order(ops);
        let writes = ops.iter().enumerate().filter(|(_, o)| o.op == OP_WRITE);
        self.index = writes.map(|(n, o)| (o.path.clone(), n as u32)).collect();
        let plan = PlanIn {
            generation: self.next,
            card_id: self.card_id,
            minor: 0,
            run_id: self.s.run_id,
            base_generation: self.generation,
            ops,
        };
        let bytes = mspd::write(&plan).map_err(|e| Stop::Refused(e.0.into()))?;
        self.put(&ms("pending.tmp"), &bytes)?;
        self.replace(&ms("pending.tmp"), &ms("pending.bin"))?;
        self.pending = true;
        Ok(())
    }

    /// `/music` and every folder of `rel` that isn't there, made (2.8.1;
    /// OWNED from the next commit on).
    fn make_folders(&mut self, rel: &str) -> Result<(), Stop> {
        self.s.music_folder()?;
        let names: Vec<&str> = rel.split('/').collect();
        let mut dir = String::new();
        for name in &names[..names.len() - 1] {
            dir = join(&dir, name);
            if self.made.contains(&dir) || self.s.music.has_folder(&dir) {
                continue;
            }
            let path = self.s.music.card_path(&dir);
            self.s.bound.alive()?.mkdir(&path).map_err(|e| self.failed(e))?;
            self.made.insert(dir.clone());
            self.owned.insert(dir.clone());
        }
        Ok(())
    }

    /// A file just put in place, as the card stores it now (2.8.3, step 7;
    /// 2.3.4): its folder listed, the one entry of its name's key.
    fn read_back(&mut self, rel: &str) -> Result<(String, Item), Stop> {
        let (dir, name) = split_last(rel);
        let card = self.s.bound.alive()?;
        let items = card.list(&self.s.music.card_path(dir)).map_err(|e| self.failed(e))?;
        let key = match_key(name);
        let mut hits = items.into_iter().filter(|i| !i.dir && match_key(&i.name) == key);
        match (hits.next(), hits.next()) {
            (Some(item), None) => Ok((join(dir, &item.name), item)),
            _ => Err(Stop::Refused(format!("{rel} isn't on the card where it was put"))),
        }
    }

    /// A card file's record and run, read from its bytes (2.7), its size,
    /// stamp and fingerprint the card's.
    fn read_record(&mut self, rel: &str, item: &Item) -> Result<(mptg::Record, Run), Stop> {
        let path = self.s.music.card_path(rel);
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let ext = name.rsplit_once('.').map_or("", |(_, e)| e).to_string();
        let card = self.s.bound.alive()?;
        let read = |mut f: Box<dyn CardFile + '_>| tags::read_from(&mut *f, &ext);
        let read = card.open(&path).and_then(read);
        let reading = read.map_err(|e| self.failed(e))?;
        let qfp = card_qfp(card, &path, item.size).map_err(|e| self.failed(e))?;
        let (size, fat_time) = (item.size as u32, item.fat_time);
        let rec = mptg::Record { size, fat_time, qfp, ..reading.rec };
        Ok((rec, reading.run))
    }

    /// A carried record read again from its card file, mStream's analysis
    /// kept where the tags still have neither a BPM nor a key.
    fn read_again(
        &mut self,
        rel: &str,
        item: &Item,
        old: &mptg::Record,
    ) -> Result<(mptg::Record, Run), Stop> {
        let (mut rec, run) = self.read_record(rel, item)?;
        if rec.bpm10 == 0 && rec.camelot == 0 && old.flags & BPM_ANALYSED != 0 {
            (rec.bpm10, rec.camelot) = (old.bpm10, old.camelot);
            rec.flags |= BPM_ANALYSED;
        }
        Ok((rec, run))
    }

    /// A file of the selection's record and ledger row: the card's reading,
    /// mStream's analysis where the tags have neither a BPM nor a key
    /// (BPM_ANALYSED; none for an UNREADABLE file, whose fields are all
    /// absent: 2.6.4), the server's fields (2.6.6).
    fn owned_of(
        &mut self,
        rel: &str,
        item: &Item,
        file: &WantedFile,
        flags: u8,
    ) -> Result<Owned, Stop> {
        let (mut rec, run) = self.read_record(rel, item)?;
        let readable = rec.flags & UNREADABLE == 0;
        if readable && rec.bpm10 == 0 && rec.camelot == 0 && (file.bpm10 != 0 || file.camelot != 0) {
            (rec.bpm10, rec.camelot) = (file.bpm10, file.camelot);
            rec.flags |= BPM_ANALYSED;
        }
        let converted = file.converted_to();
        let kbps = if converted != 0 { file.kbps } else { 0 };
        let ledger = Ledger {
            origin_flags: flags,
            converted_to: converted,
            convert_kbps: kbps,
            server: 0,
            ..file.ledger.clone()
        };
        Ok(Owned { rec, run, ledger, server: self.plan.origin_at[file.origin] })
    }

    /// The ledger takes a file put at `planned` and read back as `rel`,
    /// over the record of the file it replaced there.
    fn take(&mut self, planned: &str, rel: String, owned: Owned) {
        self.ledger.remove(planned);
        self.ledger.insert(rel, owned);
    }

    /// Step 8 for a fetched file.
    fn fetch(
        &mut self,
        w: &Write,
        fetcher: &mut dyn Fetcher,
        hooks: &mut dyn Hooks,
        events: &mut dyn FnMut(Event),
    ) -> Result<Step, Stop> {
        let plan = self.plan;
        let file = &plan.wanted.files[w.file];
        if self.spared.contains(&key_path(&w.path)) {
            return self.skip(file, "a file the listener changed is where it goes".into(), events);
        }
        let n = self.index.get(&w.path).copied().unwrap_or(0);
        let stage = ms(&format!("stage/{n:08x}.tmp"));
        let card = self.s.bound.alive()?;
        if card.stat(&ms("stage")).map_err(|e| self.failed(e))?.is_none() {
            self.s.bound.alive()?.mkdir(&ms("stage")).map_err(|e| self.failed(e))?;
        }
        // 8.1: the bytes, with the MD5 when the server's hash is one of the
        // whole file: a `hash-v` 2 digest under 25 MiB (mStream's older
        // digests may not be one, and a false mismatch would skip a good
        // file).
        let l = &file.ledger;
        let full = file.original()
            && l.hash_v >= 2
            && !hash_sampled(l.hash_v, l.server_size)
            && l.file_hash != [0; 16];
        let mut f = self.s.bound.alive()?.create(&stage).map_err(|e| self.failed(e))?;
        let path = w.path.clone();
        let mut progress = |bytes: u64| events(Event::Progress { path: path.clone(), bytes });
        let md5 = full.then(Md5Sum::new);
        let mut sink = Sink { file: &mut *f, md5, bytes: 0, error: None, progress: &mut progress };
        let fetched = fetcher.fetch(file, &mut sink);
        let (md5, bytes, error) = (sink.md5.take(), sink.bytes, sink.error.take());
        if let Some(e) = error {
            drop(f);
            return Err(self.failed(e));
        }
        let fetched = match fetched {
            Ok(fetched) => fetched,
            Err(failed) => {
                drop(f);
                self.drop_stage(&stage)?;
                if failed.stop {
                    return Ok(Step::Park(failed.why));
                }
                self.drop_ours(w, events)?;
                return self.skip(file, failed.why, events);
            }
        };
        let synced = f.sync();
        drop(f);
        synced.map_err(|e| self.failed(e))?;

        // 8.2: what came is what was asked for.
        let mut verified = false;
        let checked = if file.original() && bytes != l.server_size {
            Err(format!("{bytes} bytes, where the server's file has {}", l.server_size))
        } else if let Some(sum) = md5 {
            verified = sum.finish() == l.file_hash;
            if verified { Ok(()) } else { Err("its MD5 isn't the server's".to_string()) }
        } else if !file.original() {
            hooks.conversion(file, &fetched, self.s.card, &stage)
        } else {
            Ok(())
        };
        let checked = checked.and_then(|()| hooks.before_record(file, self.s.card, &stage));
        if let Err(why) = checked {
            self.drop_stage(&stage)?;
            self.drop_ours(w, events)?;
            return self.skip(file, why, events);
        }

        // 8.3 and 8.4: into place, and read back.
        match self.place(w, file, &stage, verified, events)? {
            Some(step) => {
                self.drop_stage(&stage)?;
                Ok(step)
            }
            None => Ok(Step::Done),
        }
    }

    /// Steps 8.3 and 8.4 for bytes fetched to `from` (a stage file, or a
    /// partial): renamed into the write's `/music` path, the file it
    /// replaces deleted first, then read back and recorded. Some: they
    /// weren't put there — the file it was to replace changed since the
    /// listing, or the name is taken — and the step that comes of it; what
    /// becomes of `from` then is the caller's.
    fn place(
        &mut self,
        w: &Write,
        file: &WantedFile,
        from: &str,
        verified: bool,
        events: &mut dyn FnMut(Event),
    ) -> Result<Option<Step>, Stop> {
        // 8.3: into place, the file it replaces deleted first.
        self.s.bound.check(false)?;
        self.make_folders(&w.path)?;
        let target = self.s.music.card_path(&w.path);
        if let Some(step) = self.clear(w, file, events)? {
            return Ok(Some(step));
        }
        if let Err(e) = self.s.bound.alive()?.rename(from, &target) {
            // A name the card says is taken, where the listing showed none
            // (2.8.3, step 6: a collision): this file waits for the next
            // run, which names it around what is there.
            if e.kind() != io::ErrorKind::AlreadyExists {
                return Err(self.failed(e));
            }
            return self.skip(file, "the card has a file of that name".into(), events).map(Some);
        }

        // 8.4: read back, and the record read from the card's bytes.
        let (rel, item) = self.read_back(&w.path)?;
        let owned = self.owned_of(&rel, &item, file, origin_flags(file, verified, false))?;
        self.take(&w.path, rel.clone(), owned);
        self.written += 1;
        events(Event::Wrote { path: rel, bytes: item.size });
        Ok(None)
    }

    /// Step 8 for a file a later run may take up (2.12.8): the server's
    /// bytes into a partial of `resume/` — the file's own listed one, cut
    /// back to what was flushed and taken up under its validator, else a new
    /// one — flushed and listed as it grows and when the fetch stops short
    /// (a park keeps it for the next run); then checked as a stage file is,
    /// and renamed straight into `/music`. Never through `stage/`.
    fn fetch_part(
        &mut self,
        w: &Write,
        fetcher: &mut dyn Fetcher,
        hooks: &mut dyn Hooks,
        events: &mut dyn FnMut(Event),
    ) -> Result<Step, Stop> {
        let plan = self.plan;
        let file = &plan.wanted.files[w.file];
        if self.spared.contains(&key_path(&w.path)) {
            return self.skip(file, "a file the listener changed is where it goes".into(), events);
        }
        let (name, have) = self.take_part(file)?;
        let part = ms(&format!("resume/{name}"));
        let held = have.as_ref().map_or(0, |h| h.bytes);
        let card = self.s.bound.alive()?;
        let opened = if have.is_some() { card.open(&part) } else { card.create(&part) };
        let mut f = opened.map_err(|e| self.failed(e))?;
        // 8.1: its bytes past what was flushed are never trusted.
        if have.is_some() {
            let cut = f.seek(SeekFrom::End(0)).and_then(|len| match len > held {
                true => f.set_len(held),
                false => Ok(()),
            });
            cut.and_then(|()| f.seek(SeekFrom::Start(held))).map_err(|e| self.failed(e))?;
        }
        let etag = self.s.settled.parts.get(&name).and_then(|e| e.etag.clone()).unwrap_or_default();
        // One shorter than its listing (a disk check's cut of it, say) is
        // listed with what it holds before a byte lands past that: never a
        // listing that vouches for bytes not yet flushed.
        if have.is_some() && self.s.settled.parts.have(&name) > held {
            self.s.settled.parts.set(&name, held, &etag);
            self.s.write_resume()?;
        }
        let path = w.path.clone();
        let mut progress = |bytes: u64| events(Event::Progress { path: path.clone(), bytes });
        let mut sink = PartSink {
            run: &mut *self,
            file: f,
            name: name.clone(),
            len: held,
            etag,
            since: Instant::now(),
            stop: None,
            progress: &mut progress,
        };
        let fetched = fetcher.fetch_part(file, have.as_ref(), &mut sink);
        let len = sink.finish()?;
        if let Err(failed) = fetched {
            // What came stays for a later run, listed; a partial that holds
            // nothing goes.
            match len > 0 {
                true => self.list_parts()?,
                false => self.drop_part(&name)?,
            }
            if failed.stop {
                return Ok(Step::Park(failed.why));
            }
            self.drop_ours(w, events)?;
            return self.skip(file, failed.why, events);
        }

        // 8.2: what came is what was asked for, the whole file read back from
        // the card, the kept bytes too; the MD5 when the server's hash is
        // one of the whole file (as for a stage file).
        let l = &file.ledger;
        let full =
            l.hash_v >= 2 && !hash_sampled(l.hash_v, l.server_size) && l.file_hash != [0; 16];
        let mut verified = false;
        let checked = if len != l.server_size {
            Err(format!("{len} bytes, where the server's file has {}", l.server_size))
        } else if full {
            let sum = card_md5(self.s.card, &part, len).map_err(|e| self.failed(e))?;
            verified = sum == l.file_hash;
            if verified { Ok(()) } else { Err("its MD5 isn't the server's".to_string()) }
        } else {
            Ok(())
        };
        let checked = checked.and_then(|()| hooks.before_record(file, self.s.card, &part));
        if let Err(why) = checked {
            self.drop_part(&name)?;
            self.drop_ours(w, events)?;
            return self.skip(file, why, events);
        }

        // 8.3 and 8.4: renamed straight into place, its entry gone at the
        // next write of resume.json. One that can't be put there goes too:
        // the hook may have changed its bytes.
        match self.place(w, file, &part, verified, events)? {
            Some(step) => {
                self.drop_part(&name)?;
                Ok(step)
            }
            None => {
                self.s.settled.parts.remove(&name);
                Ok(Step::Done)
            }
        }
    }

    /// The partial a file's bytes go into (2.12.8): its own listed one — of
    /// the same server by 2.11's test, at the same server path — when the
    /// server's entry still names the file it was listed for, cut back to
    /// what was flushed ([`Have`]); else a new one, its entry claiming no
    /// byte yet. Any other partial of the file goes.
    fn take_part(&mut self, file: &WantedFile) -> Result<(String, Option<Have>), Stop> {
        let plan = self.plan;
        let origin = &plan.wanted.origins[file.origin];
        let l = &file.ledger;
        let theirs: Vec<Entry> = self
            .s
            .settled
            .parts
            .entries()
            .iter()
            .filter(|e| e.server_path.as_deref() == Some(l.server_path.as_str()))
            .filter(|e| e.server.is_some_and(|s| same_server(&self.servers, &s, &origin.talk)))
            .cloned()
            .collect();
        let mut best: Option<(String, Have)> = None;
        let mut gone = Vec::new();
        for e in theirs {
            let path = ms(&format!("resume/{}", e.name));
            let on_card = self.s.card.stat(&path).map_err(|err| self.failed(err))?;
            let size = on_card.filter(|i| !i.dir).map(|i| i.size);
            let taken = size.and_then(|size| take_up(&e, size, l.server_size, l.server_modified));
            match taken {
                Some((bytes, validator)) if best.as_ref().is_none_or(|(_, h)| bytes > h.bytes) => {
                    let had = best.replace((e.name, Have { bytes, validator }));
                    gone.extend(had.map(|(name, _)| name));
                }
                _ => gone.push(e.name),
            }
        }
        for name in gone {
            self.drop_part(&name)?;
        }
        if self.s.card.stat(&ms("resume")).map_err(|e| self.failed(e))?.is_none() {
            self.s.bound.alive()?.mkdir(&ms("resume")).map_err(|e| self.failed(e))?;
        }
        if let Some((name, have)) = best {
            return Ok((name, Some(have)));
        }
        let parts = &mut self.s.settled.parts;
        let name = parts.fresh_name();
        let (size, modified) = (l.server_size, l.server_modified);
        parts.add(Entry::new(&name, origin.server, &l.server_path, size, modified));
        Ok((name, None))
    }

    /// A partial and its entry gone: one that can't be taken up, or whose
    /// bytes came out wrong.
    fn drop_part(&mut self, name: &str) -> Result<(), Stop> {
        let path = ms(&format!("resume/{name}"));
        if self.s.card.stat(&path).map_err(|e| self.failed(e))?.is_some() {
            self.s.bound.alive()?.delete(&path).map_err(|e| self.failed(e))?;
        }
        self.s.settled.parts.remove(name);
        Ok(())
    }

    /// `resume.json` written when the run's table of partials moved.
    fn list_parts(&mut self) -> Result<(), Stop> {
        match self.s.settled.parts.dirty() {
            true => self.s.write_resume(),
            false => Ok(()),
        }
    }

    fn drop_stage(&mut self, stage: &str) -> Result<(), Stop> {
        let card = self.s.bound.alive()?;
        match card.stat(stage).map_err(|e| self.failed(e))? {
            Some(_) => self.s.bound.alive()?.delete(stage).map_err(|e| self.failed(e)),
            None => Ok(()),
        }
    }

    /// A write's target made ready for the rename into it (step 8.3): the
    /// file it replaces deleted, while it is still the one listed (a FAT
    /// rename never replaces). Some: the write doesn't happen — the file it
    /// was to replace changed since the listing (the listener's now), or a
    /// new name is taken (2.8.3, step 6: a collision — the listener's file,
    /// copied there since the listing, or another file's name to the OS,
    /// such as its 8.3 alias), which the next run names around.
    fn clear(
        &mut self,
        w: &Write,
        file: &WantedFile,
        events: &mut dyn FnMut(Event),
    ) -> Result<Option<Step>, Stop> {
        let target = self.s.music.card_path(&w.path);
        let Some(there) = self.s.card.stat(&target).map_err(|e| self.failed(e))? else {
            return Ok(None);
        };
        match w.replaces {
            Some(r) if listed(&there, r) => {
                self.may(&w.path)?;
                self.s.bound.alive()?.delete(&target).map_err(|e| self.failed(e))?;
                self.ledger.remove(&w.path);
                Ok(None)
            }
            Some(_) => Ok(Some(self.spare(&w.path))),
            None => self.skip(file, "the card has a file of that name".into(), events).map(Some),
        }
    }

    /// A ledger file a write was to replace or move that changed since the
    /// listing: kept as it is, its row dropped (2.11: the listener's now),
    /// and reported.
    fn spare(&mut self, path: &str) -> Step {
        self.report.spared.push(path.to_string());
        self.spared.insert(key_path(path));
        self.ledger.remove(path);
        Step::Skipped
    }

    /// A write that won't happen in this run, whose target is a cut run's
    /// half-finished work (2.12.3): deleted while it is still as listed.
    /// Once the final commit deletes `pending.bin` no plan names it, and a
    /// later run would take it for the listener's.
    fn drop_ours(&mut self, w: &Write, events: &mut dyn FnMut(Event)) -> Result<(), Stop> {
        let Some(r) = w.replaces.filter(|r| r.ours) else { return Ok(()) };
        let target = self.s.music.card_path(&w.path);
        let there = self.s.card.stat(&target).map_err(|e| self.failed(e))?;
        if there.is_some_and(|i| listed(&i, r)) {
            self.s.bound.check(false)?;
            self.may(&w.path)?;
            self.s.bound.alive()?.delete(&target).map_err(|e| self.failed(e))?;
            self.deleted += 1;
            events(Event::Deleted(w.path.clone()));
        }
        Ok(())
    }

    fn skip(
        &mut self,
        file: &WantedFile,
        why: String,
        events: &mut dyn FnMut(Event),
    ) -> Result<Step, Stop> {
        let server_path = file.ledger.server_path.clone();
        self.report.failed.push((server_path.clone(), why.clone()));
        events(Event::Failed { server_path, why });
        Ok(Step::Skipped)
    }

    /// Step 8 for a file the server moved: renamed on the card, its record
    /// carried, its row the server's entry now.
    fn rename(&mut self, w: &Write, from: &str, events: &mut dyn FnMut(Event)) -> Result<Step, Stop> {
        let plan = self.plan;
        let file = &plan.wanted.files[w.file];
        if self.spared.contains(&key_path(&w.path)) {
            return self.skip(file, "a file the listener changed is where it goes".into(), events);
        }
        let Some(old) = self.ledger.get(from).cloned() else { return Ok(Step::Skipped) };
        // The file it moves, still the one listed; else the listener's now
        // (2.11), left where it is.
        let source = self.s.music.card_path(from);
        let size = old.rec.size as u64;
        match self.s.card.stat(&source).map_err(|e| self.failed(e))? {
            Some(i) if listed(&i, Replaced { size, fat_time: old.rec.fat_time, ours: false }) => {}
            there => {
                self.drop_ours(w, events)?;
                if there.is_some() {
                    return Ok(self.spare(from));
                }
                self.ledger.remove(from);
                self.report.gone.push(from.to_string());
                return Ok(Step::Skipped);
            }
        }
        self.s.bound.check(false)?;
        self.make_folders(&w.path)?;
        let target = self.s.music.card_path(&w.path);
        if let Some(step) = self.clear(w, file, events)? {
            return Ok(step);
        }
        self.may(from)?;
        if let Err(e) = self.s.bound.alive()?.rename(&source, &target) {
            if e.kind() != io::ErrorKind::AlreadyExists {
                return Err(self.failed(e));
            }
            return self.skip(file, "the card has a file of that name".into(), events);
        }
        self.ledger.remove(from);
        let (rel, item) = self.read_back(&w.path)?;
        let l = &file.ledger;
        let ledger = Ledger {
            server_path: l.server_path.clone(),
            mstream_id: l.mstream_id,
            album_id: l.album_id,
            artist_id: l.artist_id,
            created_at: l.created_at,
            ..old.ledger.clone()
        };
        // The record carried, its stamp the card's; read again from the
        // bytes should its size have moved since the check above.
        let (rec, run) = match item.size == size {
            true => (mptg::Record { fat_time: item.fat_time, ..old.rec }, old.run.clone()),
            false => self.read_again(&rel, &item, &old.rec)?,
        };
        self.take(&w.path, rel.clone(), Owned { rec, run, ledger, ..old });
        self.renamed += 1;
        events(Event::Renamed { from: from.to_string(), to: rel });
        Ok(Step::Done)
    }

    /// Step 10: each album folder's thumbnail as the cover hook says, a clash
    /// of the 32-bit name going to the smaller full hash (2.14.1); a new one
    /// written as `.tmp`, the old `.565` deleted, then renamed.
    fn thumbnails(&mut self, hooks: &mut dyn Hooks) -> Result<(), Stop> {
        let roots = self.plan.roots.clone();
        let mut albums: BTreeMap<String, Vec<(String, Ledger)>> = BTreeMap::new();
        for (path, o) in self.ledger.iter().filter(|(_, o)| o.rec.container != CONTAINER_NOT_AUDIO) {
            let album = albums.entry(album_folder(path, &roots).to_string()).or_default();
            album.push((path.clone(), o.ledger.clone()));
        }
        let mut chosen: BTreeMap<u32, (u64, String, Option<(mpth::Pixels, u32)>)> = BTreeMap::new();
        for (folder, files) in &albums {
            let hash = path_hash(folder);
            let file = ms(&thumb_path_of(folder));
            let had = self.thumbs.contains(folder);
            let bytes = match had {
                true => self.s.card.read(&file, mpth::BYTES).map_err(|e| self.failed(e))?,
                false => None,
            };
            let on_card = bytes.is_some_and(|b| mpth::check(&b, hash));
            let files = files.iter().map(|(p, l)| (p.as_str(), l)).collect();
            let album = Album { folder, files, on_card };
            let new = match hooks.cover(&album) {
                Cover::Keep if on_card => None,
                Cover::New { pixels, source_bytes } => Some((pixels, source_bytes)),
                Cover::Keep | Cover::Gone => continue,
            };
            let name = (hash >> 32) as u32;
            if chosen.get(&name).is_none_or(|c| hash < c.0) {
                chosen.insert(name, (hash, folder.clone(), new));
            }
        }
        let mut thumbs = BTreeSet::new();
        for (hash, folder, new) in chosen.into_values() {
            if let Some((pixels, source_bytes)) = new {
                let bytes = mpth::write(hash, source_bytes, &pixels);
                let bytes = bytes.map_err(|e| Stop::Refused(e.0.into()))?;
                let file = ms(&thumb_path(hash));
                let (dir, _) = split_last(&file);
                if self.s.card.stat(dir).map_err(|e| self.failed(e))?.is_none() {
                    if self.s.card.stat(&ms("thumbs")).map_err(|e| self.failed(e))?.is_none() {
                        self.s.bound.alive()?.mkdir(&ms("thumbs")).map_err(|e| self.failed(e))?;
                    }
                    self.s.bound.alive()?.mkdir(dir).map_err(|e| self.failed(e))?;
                }
                let tmp = format!("{}.tmp", &file[..file.len() - 4]);
                self.put(&tmp, &bytes)?;
                self.replace(&tmp, &file)?;
            }
            thumbs.insert(folder);
        }
        self.thumbs = thumbs;
        Ok(())
    }

    /// The table of folders a commit records (2.6.3): every OWNED folder
    /// there is, and THUMB on the album folders with a thumbnail that hold
    /// a record.
    fn folder_table(&self, records: &[RecordIn]) -> Vec<FolderIn> {
        let mut ancestors: HashSet<&str> = HashSet::new();
        for r in records {
            let mut dir = split_last(&r.path).0;
            while !dir.is_empty() && ancestors.insert(dir) {
                dir = split_last(dir).0;
            }
        }
        let mut flags: BTreeMap<&str, u32> = BTreeMap::new();
        for f in &self.owned {
            *flags.entry(f).or_default() |= FOLDER_OWNED;
        }
        for f in self.thumbs.iter().filter(|f| f.is_empty() || ancestors.contains(f.as_str())) {
            *flags.entry(f).or_default() |= FOLDER_THUMB;
        }
        flags.into_iter().map(|(path, flags)| FolderIn { path: path.to_string(), flags }).collect()
    }

    /// A commit (2.12.1, steps 9 and 12): the tags file under its final
    /// name, the Auto DJ index of a final commit, then the root through
    /// `manifest.tmp` — `manifest.bin` deleted, then the rename — then the
    /// tags file before deleted, and at a checkpoint the plan's rest.
    fn commit(
        &mut self,
        last: bool,
        hooks: &mut dyn Hooks,
        events: &mut dyn FnMut(Event),
    ) -> Result<(), Stop> {
        let plan = self.plan;
        let table = &plan.table;
        let used: BTreeSet<usize> = self.ledger.values().map(|o| o.server).collect();
        let mut remap = vec![0u16; table.len()];
        let mut servers = Vec::new();
        for i in 1..table.len() {
            if used.contains(&i) {
                servers.push(table[i]);
                remap[i] = servers.len() as u16;
            }
        }
        let records: Vec<RecordIn> = self
            .ledger
            .iter()
            .map(|(path, o)| RecordIn {
                path: path.clone(),
                rec: o.rec,
                run: o.run.clone(),
                ledger: Some(Ledger { server: remap[o.server], ..o.ledger.clone() }),
            })
            .collect();
        let folders = self.folder_table(&records);
        let generation = self.next;
        let now = self.s.env.now();
        let root = self.s.settled.root.as_ref();

        // The Auto DJ index: a checkpoint names the old one again; the final
        // commit's is the hook's (AUTODJ-INDEX.md 7.3).
        let mut autodj = self.autodj.clone();
        let mut new_index: Option<Vec<u8>> = None;
        if last {
            let old = self.s.settled.autodj.as_deref().filter(|_| self.autodj.is_some());
            let commit = Commit {
                generation,
                card_id: self.card_id,
                now: now as u32,
                records: &records,
                device: &self.s.device,
                old,
            };
            autodj = match hooks.autodj(&commit) {
                AutoDj::None => None,
                AutoDj::Same => self.autodj.clone(),
                AutoDj::New(bytes) => {
                    let refused = |why| Stop::Refused(format!("an Auto DJ index: {why}"));
                    let table = Table::open(&bytes).map_err(refused)?;
                    table.check(mpdj::Uses::ALL).map_err(refused)?;
                    let h = table.header();
                    if h.generation != generation || h.card_id != self.card_id {
                        return Err(Stop::Refused("an Auto DJ index of another commit".into()));
                    }
                    let comp = Comp::of(&bytes);
                    new_index = Some(bytes);
                    comp
                }
            };
        }

        // The root's server 0 header: the server read, or as it was; its
        // revision only with the final commit, so a cut run's next run reads
        // the server again.
        let server0 = table[0];
        let reads_zero = plan.origin_at.contains(&0);
        let revision = match (last && reads_zero, root) {
            (true, _) => {
                let o = plan.origin_at.iter().position(|&i| i == 0).unwrap_or(0);
                plan.wanted.origins[o].revision.clone()
            }
            (false, Some(r)) => r.manifest.server_revision.clone(),
            (false, None) => String::new(),
        };
        let holds = |r: &&String| {
            let inside = |p: &str| p.strip_prefix(r.as_str()).is_some_and(|rest| rest.starts_with('/'));
            records.iter().any(|rec| inside(&rec.path))
        };
        let roots: Vec<String> = plan.roots.iter().filter(holds).cloned().collect();
        let acks = hooks.acks(&self.acks);

        // A run that changed nothing makes no commit: a new one would have
        // the device build its library again (3.2.2), which acknowledging
        // plays isn't worth (2.5.6): the plays' marks wait for a commit made
        // anyway.
        let same = self.unchanged(&records, &folders, &autodj, &servers, &roots, &revision, server0);
        if last && same {
            return Ok(());
        }

        // The card is checked before the commit's first write: once
        // manifest.tmp is whole it is the card's root (2.5.4), this run's.
        self.s.bound.check(true)?;
        let meta = Meta {
            source: SOURCE_SOFTWARE,
            parser_version: PARSER_VERSION,
            read_rules: READ_RULES,
            generation,
            card_id: self.card_id,
            minor: 0,
            producer: self.s.env.producer.clone(),
        };
        let tags = mptg::write(&meta, &records, &folders).map_err(|e| Stop::Refused(e.0.into()))?;
        let tags_name = companion_name(MPTG, generation);
        self.put(&ms(&tags_name), &tags)?;
        if let (Some(bytes), Some(comp)) = (&new_index, &autodj) {
            self.put(&ms(&comp.name()), bytes)?;
        }
        let comps: Vec<Comp> = Comp::of(&tags).into_iter().chain(autodj.clone()).collect();
        let commit_id = self.s.env.id();
        let manifest = ManifestIn {
            generation,
            card_id: self.card_id,
            minor: 0,
            commit_time: now.min(u32::MAX as u64) as u32,
            flags: if last { FINAL } else { 0 },
            commit_id,
            server_instance: server0.instance,
            server_url_key: server0.url_key,
            producer: self.s.env.producer.clone(),
            server_revision: revision,
            base_generation: self.generation,
            comps,
            roots,
            servers,
            acks: acks.clone(),
        };
        let manifest = msmf::write(&manifest).map_err(|e| Stop::Refused(e.0.into()))?;
        self.put(&ms("manifest.tmp"), &manifest)?;
        self.s.bound.committed(self.card_id, commit_id);
        self.replace(&ms("manifest.tmp"), &ms("manifest.bin"))?;

        // The tags file before goes; so, at the end, does an index no longer
        // named (the collection's).
        if let Some(old) = self.tags_name.replace(tags_name.clone())
            && old != tags_name
        {
            let card = self.s.bound.alive()?;
            if card.stat(&ms(&old)).map_err(|e| self.failed(e))?.is_some() {
                self.s.bound.alive()?.delete(&ms(&old)).map_err(|e| self.failed(e))?;
            }
        }
        self.generation = generation;
        self.next = generation + 1;
        self.autodj = autodj;
        self.acks = acks;
        self.since = 0;
        self.last = Instant::now();
        events(Event::Committed { generation, last });
        if !last {
            self.write_pending()?;
        }
        Ok(())
    }

    /// The final commit would say what the root says already: the same
    /// records, folders, servers, roots, revision and Auto DJ index, and the
    /// root a final one. Its acknowledgements aside: they ride on a commit,
    /// never make one.
    #[allow(clippy::too_many_arguments)]
    fn unchanged(
        &self,
        records: &[RecordIn],
        folders: &[FolderIn],
        autodj: &Option<Comp>,
        servers: &[Server],
        roots: &[String],
        revision: &str,
        server0: Server,
    ) -> bool {
        let Some(root) = self.s.settled.root.as_ref() else { return false };
        let Ok(tags) = root.tags.as_ref() else { return false };
        let m = &root.manifest;
        let new_id = m.header.card_id != self.card_id;
        if self.pending || !m.is_final() || self.s.survey.reread || !tags.has_ledger || new_id {
            return false;
        }
        let (_, old_records, old_folders) = tags.to_input();
        let sorted = |r: &[RecordIn]| {
            let mut v: Vec<RecordIn> = r.to_vec();
            v.sort_by(|a, b| a.path.cmp(&b.path));
            v
        };
        let folder_set =
            |f: &[FolderIn]| f.iter().map(|f| (f.path.clone(), f.flags)).collect::<BTreeSet<_>>();
        let mut old_roots = m.roots.clone();
        old_roots.sort();
        sorted(records) == sorted(&old_records)
            && folder_set(folders) == folder_set(&old_folders)
            && *autodj == m.autodj
            && servers == m.servers.as_slice()
            && roots == old_roots.as_slice()
            && revision == m.server_revision
            && (server0.instance, server0.url_key) == (m.server_instance, m.server_url_key)
    }

    /// Step 13's folders, taken before the final commit: each OWNED folder
    /// left empty (deepest first) removed, and left out of the commit. A cut
    /// between the two leaves the root before naming a folder that is gone,
    /// which the next run drops.
    fn prune(&mut self) -> Result<(), Stop> {
        let mut owned: Vec<String> = self.owned.iter().cloned().collect();
        owned.sort_by_key(|f| std::cmp::Reverse(f.matches('/').count()));
        for folder in owned {
            let path = self.s.music.card_path(&folder);
            let there = self.s.card.stat(&path).map_err(|e| self.failed(e))?.is_some_and(|i| i.dir);
            if !there {
                self.owned.remove(&folder);
                continue;
            }
            if entries(self.s.card, &path).map_err(|e| self.failed(e))?.is_empty() {
                self.s.bound.alive()?.rmdir(&path).map_err(|e| self.failed(e))?;
                self.owned.remove(&folder);
            }
        }
        Ok(())
    }

    /// Step 13: the companions the root doesn't name, the thumbnails whose
    /// folder lost THUMB.
    fn collect(&mut self) -> Result<(), Stop> {
        self.s.bound.check(true)?;
        let index = self.autodj.as_ref().map(Comp::name);
        let named: HashSet<String> = self.tags_name.iter().cloned().chain(index).collect();
        let top = entries(self.s.card, "/.mstream").map_err(|e| self.failed(e))?;
        for item in top.iter().filter(|i| !i.dir) {
            let lower = item.name.to_ascii_lowercase();
            if companion_generation(&lower).is_some() && !named.contains(&lower) {
                self.s.bound.alive()?.delete(&ms(&item.name)).map_err(|e| self.failed(e))?;
            }
        }
        let named = |f: &String| thumb_path_of(f).to_ascii_lowercase();
        let keep: HashSet<String> = self.thumbs.iter().map(named).collect();
        let folders = entries(self.s.card, &ms("thumbs")).map_err(|e| self.failed(e))?;
        for dir in folders.into_iter().filter(|i| i.dir) {
            let inner = ms(&format!("thumbs/{}", dir.name));
            let files = entries(self.s.card, &inner).map_err(|e| self.failed(e))?;
            for f in files.into_iter().filter(|i| !i.dir) {
                let rel = format!("thumbs/{}/{}", dir.name, f.name).to_ascii_lowercase();
                if !keep.contains(&rel) {
                    let path = format!("{inner}/{}", f.name);
                    self.s.bound.alive()?.delete(&path).map_err(|e| self.failed(e))?;
                }
            }
        }
        self.collect_parts()
    }

    /// Step 13's partials (2.12.8): one no plan wants any more — its file
    /// written, gone from the selection, changed on the server — deleted and
    /// its entry dropped. One whose file the selection still has, unchanged
    /// and not on the card, stays whether this run's plan wrote it or not (a
    /// file skipped, or one a later run is left to fetch); so does one of a
    /// server this run doesn't read, as that server's rows do. One whose
    /// server can't be told goes. `resume.json` is written when its list
    /// moved.
    fn collect_parts(&mut self) -> Result<(), Stop> {
        let files = entries(self.s.card, &ms("resume")).map_err(|e| self.failed(e))?;
        let plan = self.plan;
        let origins = &plan.wanted.origins;
        // The server's own bytes the card holds now: each record's server
        // (the run's numbering), server path, size and modified.
        let held: HashSet<(usize, &str, u64, u64)> = (self.ledger.values())
            .filter(|o| o.ledger.converted_to == 0)
            .map(|o| {
                let l = &o.ledger;
                (o.server, l.server_path.as_str(), l.server_size, l.server_modified)
            })
            .collect();
        let waiting: Vec<&WantedFile> = (plan.wanted.files.iter())
            .filter(|f| f.original())
            .filter(|f| {
                let (l, at) = (&f.ledger, plan.origin_at[f.origin]);
                !held.contains(&(at, l.server_path.as_str(), l.server_size, l.server_modified))
            })
            .collect();
        let servers = &self.servers;
        let wanted = |e: &Entry| {
            let Some(server) = e.server else { return false };
            if !origins.iter().any(|o| same_server(servers, &server, &o.talk)) {
                return true;
            }
            waiting.iter().any(|f| {
                let l = &f.ledger;
                same_server(servers, &server, &origins[f.origin].talk)
                    && e.server_path.as_deref() == Some(l.server_path.as_str())
                    && (e.size, e.modified) == (l.server_size, Some(l.server_modified))
            })
        };
        let (mut kept, mut gone) = (HashSet::new(), Vec::new());
        for item in files.into_iter().filter(|i| !i.dir) {
            if self.s.settled.parts.get(&item.name).is_some_and(wanted) {
                kept.insert(item.name);
            } else {
                gone.push(item.name);
            }
        }
        for name in gone {
            self.drop_part(&name)?;
        }
        let names = self.s.settled.parts.entries().iter().map(|e| e.name.clone());
        let names: Vec<String> = names.filter(|n| !kept.contains(n)).collect();
        for name in &names {
            self.s.settled.parts.remove(name);
        }
        self.list_parts()
    }

    /// The card read back as the device reads it (the transfer plan's 2.3,
    /// step 16; tools/synthtransfer.py's check is the model): the root that
    /// counts is this run's and final, its tags file and Auto DJ index valid
    /// and the ones it names, and every record's file there by the exact
    /// bytes of its path, with its size and stamp (and, deep, its
    /// fingerprint).
    fn verify(&mut self) -> Result<(), String> {
        let card = self.s.card;
        let read = |name: &str| card.read(&ms(name), MAX_FILE).map_err(|e| e.to_string());
        let valid = |name: &str| -> Result<Option<Manifest>, String> {
            Ok(read(name)?.and_then(|b| Manifest::read(&b, msmf::Uses::DEVICE).ok()))
        };
        let (bin, tmp) = (valid("manifest.bin")?, valid("manifest.tmp")?);
        let candidate = |m: &Option<Manifest>| m.as_ref().map(Manifest::candidate);
        let pick = msmf::elect(candidate(&bin), candidate(&tmp)).pick;
        let root = match pick {
            Pick::Bin => bin,
            Pick::Tmp => tmp,
            Pick::None => None,
        };
        let Some(root) = root else { return Err("no root reads".into()) };
        if root.commit_id != self.s.bound.id.commit_id || !root.is_final() {
            return Err("the root that counts isn't this run's final commit".into());
        }
        let tags_bytes = read(&root.tags.name())?.ok_or("the tags file is missing")?;
        let tags = mptg::read(&tags_bytes, mptg::Uses::DEVICE);
        let tags = tags.map_err(|why| format!("the tags file: {why}"))?;
        if !msmf::companion_matches(&root.tags, &tags.header, tags.source, None) {
            return Err("the tags file isn't the one the root names".into());
        }
        if let Some(entry) = &root.autodj {
            let bytes = read(&entry.name())?.ok_or("the Auto DJ index is missing")?;
            let table = Table::open(&bytes).map_err(|why| format!("the Auto DJ index: {why}"))?;
            table.check(mpdj::Uses::DEVICE).map_err(|why| format!("the Auto DJ index: {why}"))?;
            if !msmf::companion_matches(entry, table.header(), 0, Some(&table.sig)) {
                return Err("the Auto DJ index isn't the one the root names".into());
            }
        }
        let music = Music::list(card).map_err(|e| e.to_string())?;
        for e in &tags.records {
            let (dir, name) = split_last(&e.path);
            let listed = |items: &Vec<Item>| items.iter().find(|i| !i.dir && i.name == name).cloned();
            let item = music.dirs.get(dir).and_then(listed);
            let Some(item) = item else { return Err(format!("{} isn't on the card", e.path)) };
            if item.size != e.rec.size as u64 || item.fat_time != e.rec.fat_time {
                return Err(format!("{} isn't the file its record describes", e.path));
            }
            let print = || card_qfp(card, &music.card_path(&e.path), item.size).ok();
            if self.s.knobs.deep_check && print() != Some(e.rec.qfp) {
                return Err(format!("{}'s fingerprint isn't its record's", e.path));
            }
        }
        Ok(())
    }
}

/// A file as the plan's listing had it: its size and stamp.
fn listed(item: &Item, r: Replaced) -> bool {
    !item.dir && item.size == r.size && item.fat_time == r.fat_time
}

/// The ops of a plan in `pending.bin`'s order (2.12.5): the deletes, then
/// the folders in pre-order, then the writes, each group canonical — the
/// order mspd::write puts them in, so an op's place here is its index there.
fn pend_order(mut ops: Vec<Op>) -> Vec<Op> {
    let rank = |op: u8| match op {
        OP_DELETE => 0,
        OP_FOLDER => 1,
        _ => 2,
    };
    ops.sort_by(|a, b| {
        let paths = if a.op == OP_FOLDER {
            compare_folder_paths(&a.path, &b.path)
        } else {
            compare_file_paths(&a.path, &b.path)
        };
        rank(a.op).cmp(&rank(b.op)).then(paths)
    });
    ops
}

/// The stage file a fetch writes into: the MD5 of what passes, its count,
/// and a write the card refused, told apart from the source's own failure.
struct Sink<'a> {
    file: &'a mut dyn CardFile,
    md5: Option<Md5Sum>,
    bytes: u64,
    error: Option<io::Error>,
    progress: &'a mut dyn FnMut(u64),
}

impl io::Write for Sink<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.file.write(buf) {
            Ok(n) => {
                if let Some(md5) = &mut self.md5 {
                    md5.update(&buf[..n]);
                }
                self.bytes += n as u64;
                (self.progress)(self.bytes);
                Ok(n)
            }
            Err(e) => {
                self.error = Some(io::Error::new(e.kind(), e.to_string()));
                Err(e)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// The partial a fetch writes into (2.12.8): its bytes counted, flushed and
/// listed in `resume.json` as they grow ([`Knobs`]'s `part_bytes` and
/// `part_secs`), and cut back — the listing first, so it never claims bytes
/// that are gone — when the answer starts it over. A write the card refused,
/// or a listing that failed, stops the run: the stop is kept for it, the
/// fetch hearing only an error.
///
/// [`Knobs`]: super::Knobs
struct PartSink<'x, 'r, 'c> {
    run: &'x mut Runner<'r, 'c>,
    file: Box<dyn CardFile + 'c>,
    name: String,
    /// The partial's length now.
    len: u64,
    /// The ETag the bytes it holds came under (`""`: none).
    etag: String,
    /// Its last listing.
    since: Instant,
    stop: Option<Stop>,
    progress: &'x mut dyn FnMut(u64),
}

impl PartSink<'_, '_, '_> {
    /// The stop kept, as the fetch hears it.
    fn hold(&mut self, stop: Stop) -> io::Error {
        let e = io::Error::other(stop.to_string());
        self.stop = Some(stop);
        e
    }

    /// Flushed, and listed with what it holds.
    fn list(&mut self) -> Result<(), Stop> {
        self.file.sync().map_err(|e| self.run.failed(e))?;
        self.run.s.settled.parts.set(&self.name, self.len, &self.etag);
        self.run.s.write_resume()?;
        self.since = Instant::now();
        Ok(())
    }

    /// The fetch over: the partial flushed and closed, and the run's table
    /// told what it holds (a later write of `resume.json` lists it so). Its
    /// length; the run's stop instead, when the card stopped it.
    fn finish(mut self) -> Result<u64, Stop> {
        if let Some(stop) = self.stop.take() {
            return Err(stop);
        }
        self.file.sync().map_err(|e| self.run.failed(e))?;
        self.run.s.settled.parts.set(&self.name, self.len, &self.etag);
        Ok(self.len)
    }
}

impl io::Write for PartSink<'_, '_, '_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.stop.is_some() {
            return Err(io::Error::other("the run stopped"));
        }
        let n = match self.file.write(buf) {
            Ok(n) => n,
            Err(e) => {
                let stop = self.run.failed(e);
                return Err(self.hold(stop));
            }
        };
        self.len += n as u64;
        (self.progress)(self.len);
        let (bytes, secs) = (self.run.s.knobs.part_bytes, self.run.s.knobs.part_secs);
        let listed = self.run.s.settled.parts.have(&self.name);
        let late = self.since.elapsed().as_secs() >= secs;
        let due = self.len >= listed.saturating_add(bytes) || late;
        if self.len > listed
            && due
            && let Err(stop) = self.list()
        {
            return Err(self.hold(stop));
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Part for PartSink<'_, '_, '_> {
    fn answered(&mut self, from: u64, validator: Option<&str>) -> io::Result<()> {
        if self.stop.is_some() {
            return Err(io::Error::other("the run stopped"));
        }
        if from > self.len {
            let past = format!("an answer from byte {from}, past the {} held", self.len);
            return Err(io::Error::other(past));
        }
        // An ETag only: a listing with none asks by the date of `modified`.
        let etag = validator.filter(|v| v.contains('"')).unwrap_or_default().to_string();
        if from < self.len {
            // The bytes from `from` on go, the listing first claiming no
            // more than what stays.
            if self.run.s.settled.parts.have(&self.name) > from {
                self.run.s.settled.parts.set(&self.name, from, &etag);
                if let Err(stop) = self.run.s.write_resume() {
                    return Err(self.hold(stop));
                }
            }
            let cut = self.file.set_len(from).and_then(|()| self.file.seek(SeekFrom::Start(from)));
            if let Err(e) = cut {
                let stop = self.run.failed(e);
                return Err(self.hold(stop));
            }
            self.len = from;
        }
        self.etag = etag;
        Ok(())
    }
}

/// The MD5 of a card file's first `len` bytes, read back in pieces.
fn card_md5(card: &dyn CardFs, path: &str, len: u64) -> io::Result<[u8; 16]> {
    let mut md5 = Md5Sum::new();
    let mut at = 0;
    while at < len {
        let piece = card.read_at(path, at, (len - at).min(1 << 20) as usize)?;
        if piece.is_empty() {
            let short = "the file is shorter than read";
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, short));
        }
        md5.update(&piece);
        at += piece.len() as u64;
    }
    Ok(md5.finish())
}
