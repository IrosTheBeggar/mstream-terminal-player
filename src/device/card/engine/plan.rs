//! A run's plan (docs/METADATA.md 2.10.2, 2.11, 2.12.3): the server's side
//! paired with the ledger, each row given its action by the plan table,
//! the new files named, and the guards raised as decisions.
//!
//! **Pairing, per server** (2.5.5). Each server the run reads is one of the
//! card's by 2.11's test, or new. A row of it pairs with the entry of its
//! server path, else with the one unpaired entry of its audio hash (else
//! its file hash): a move on the server, carried out as a rename on the
//! card. A row of a server the run doesn't read is carried as it is.
//!
//! **The table** (2.10.2, step 4), with the card's settings:
//!
//! | Server | Card | Action |
//! |---|---|---|
//! | new in the selection | path free | write |
//! | changed | unchanged | replace (in place) |
//! | unchanged | unchanged | keep, restamped |
//! | any | changed by the listener | keep it, drop the row, report (U5) |
//! | in the selection | moved by the listener | follow (U4) |
//! | in the selection | missing | write again (U4) |
//! | left the selection | unchanged | delete (Q1's mirror) or keep |
//! | — | a cut run's target | replace in place, or delete (2.12.3) |
//!
//! **Names** (2.8.3): a namer seeded with the card's listing and the rows
//! that stay. A first pass leaves the listener's own files out, which says
//! where each file would go on its own: a server move whose file would keep
//! its name stays where it is, and a listener's file of the same path,
//! size and fingerprint is one to adopt (U6). The second pass names for
//! real, around every file that stays.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::super::contract::mptg::{ADOPTED, HASH_SAMPLED, Ledger, VERIFIED};
use super::super::contract::prim::{hash_sampled, qfp, qfp_ranges};
use super::super::contract::server::{NO_INSTANCE, Server, Verdict, srvs_breaks};
use super::super::names::{Bytes, Layout, Namer, Wanted};
use super::super::volume::Item;
use super::settle::card_qfp;
use super::survey::{Side, join, key_path};
use super::{Answers, Fetcher, Guard, Removal, Report, Session, Stop, WantedCard, WantedFile};

/// A row carried into the next commit, its file left where the card has it.
#[derive(Clone, Debug)]
pub(super) struct Keep {
    pub(super) row: usize,
    /// As listed: a spelling the listener changed, or a move followed.
    pub(super) path: String,
    pub(super) fat_time: u32,
    /// Its server, in the run's numbering.
    pub(super) server: usize,
    /// Its row, the ids refreshed from the server's entry when it was read.
    pub(super) ledger: Ledger,
}

/// A listener's file taken into the ledger (U6): ADOPTED, not VERIFIED.
#[derive(Clone, Debug)]
pub(super) struct Adopt {
    pub(super) file: usize,
    pub(super) path: String,
    pub(super) fat_time: u32,
}

/// A file leaving the card at step 7.
#[derive(Clone, Debug)]
pub(super) struct Delete {
    pub(super) path: String,
    /// What the listing said of it: it goes only while it still says so.
    pub(super) size: u64,
    pub(super) fat_time: u32,
    /// It left the selection (Q1's gates count these).
    pub(super) removal: bool,
    /// A cut run's half-finished work (2.12.3): no identity of its own.
    pub(super) ours: bool,
}

impl Delete {
    fn ours(path: &str) -> Delete {
        Delete { path: path.to_string(), size: 0, fat_time: 0, removal: false, ours: true }
    }
}

/// A file the run puts in place at step 8.
#[derive(Clone, Debug)]
pub(super) struct Write {
    /// Relative to `/music`, as the names gave it.
    pub(super) path: String,
    pub(super) file: usize,
    /// A rename of a file already on the card (a move on the server), from
    /// this path; None: the bytes are fetched.
    pub(super) from: Option<String>,
    /// The file it replaces in place, as listed; None: a new name, where
    /// any file at all is a collision (2.8.3, step 6), never deleted.
    pub(super) replaces: Option<Replaced>,
}

/// A file a write replaces in place (2.11): a ledger file, or a cut run's
/// half-finished work. It goes at step 8.3 only while it is still what the
/// listing said, as a delete of step 7 does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Replaced {
    pub(super) size: u64,
    pub(super) fat_time: u32,
    /// A cut run's half-finished work (2.12.3): no ledger file, and one a
    /// run that can't replace it deletes, or a later run takes it for the
    /// listener's.
    pub(super) ours: bool,
}

impl Replaced {
    fn of(item: &Item, ours: bool) -> Replaced {
        Replaced { size: item.size, fat_time: item.fat_time, ours }
    }
}

/// What a run will do, and what it needs answered first.
pub(crate) struct Plan {
    pub(super) token: u64,
    pub(super) wanted: WantedCard,
    /// The card's servers as the run numbers them: the root's, the old SRVS
    /// entries, then the servers it adds. Each commit renumbers the ones
    /// with rows (2.5.5).
    pub(super) table: Vec<Server>,
    /// Which of them each origin is.
    pub(super) origin_at: Vec<usize>,
    pub(super) keeps: Vec<Keep>,
    pub(super) adopts: Vec<Adopt>,
    pub(super) deletes: Vec<Delete>,
    pub(super) writes: Vec<Write>,
    /// The folders the writes need, relative to `/music`.
    pub(super) folders: Vec<String>,
    /// The vpath layout's library roots (LIBR, 2.8.6).
    pub(super) roots: Vec<String>,
    /// The decisions still to answer; the run refuses while any is left.
    pub(crate) guards: Vec<Guard>,
    pub(crate) report: Report,
    /// The commit mints a new card id ([`Answers::new_card_id`]).
    pub(super) new_card_id: bool,
}

impl Plan {
    /// The run would delete, write, rename or adopt a file of `/music`: a
    /// card's set-up, which never touches the listener's music, refuses
    /// such a plan.
    pub(crate) fn touches_music(&self) -> bool {
        !self.deletes.is_empty() || !self.writes.is_empty() || !self.adopts.is_empty()
    }

    /// The files that leave the card because they left the selection: the
    /// list shown before a sync (Q1).
    pub(crate) fn removals(&self) -> Vec<&str> {
        self.deletes.iter().filter(|d| d.removal).map(|d| d.path.as_str()).collect()
    }

    /// Files to fetch, and their bytes (estimates for conversions).
    pub(crate) fn fetches(&self) -> (usize, u64) {
        let fetched = self.writes.iter().filter(|w| w.from.is_none());
        fetched.fold((0, 0), |(n, b), w| (n + 1, b + self.wanted.files[w.file].size))
    }

    /// Server moves carried out as renames.
    pub(crate) fn renames(&self) -> usize {
        self.writes.iter().filter(|w| w.from.is_some()).count()
    }

    /// Rows kept as they are, and files adopted.
    pub(crate) fn kept(&self) -> (usize, usize) {
        (self.keeps.len(), self.adopts.len())
    }
}

/// The extension a file takes on the card (2.8.3, step 3), lowercase.
fn card_extension(file: &WantedFile) -> String {
    let server = file.ledger.server_path.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
    match file.bytes {
        Bytes::Original { ogg_opus: true } if server == "ogg" || server == "oga" => "opus".into(),
        Bytes::Converted(1) => "mp3".into(),
        Bytes::Converted(2) => "flac".into(),
        Bytes::Converted(3) => "opus".into(),
        _ => server,
    }
}

/// A paired row's file is the entry's bytes still (2.10.2): the same audio
/// hash, size and `modified`, the same file hash when both have one, and
/// the same conversion.
fn unchanged(row: &Ledger, file: &WantedFile) -> bool {
    let l = &file.ledger;
    let none = [0; 16];
    let file_hash = row.file_hash == l.file_hash || row.file_hash == none || l.file_hash == none;
    let kbps = row.converted_to == 0 || row.convert_kbps == file.kbps;
    let conversion = row.converted_to == file.converted_to() && kbps;
    row.audio_hash == l.audio_hash
        && row.server_size == l.server_size
        && row.server_modified == l.server_modified
        && file_hash
        && conversion
}

/// A row's ids refreshed from its entry (2.10.2: they change when the
/// server's database is rebuilt), its server path the entry's.
fn refreshed(row: &Ledger, file: &WantedFile) -> Ledger {
    let l = &file.ledger;
    Ledger {
        server_path: l.server_path.clone(),
        mstream_id: l.mstream_id,
        album_id: l.album_id,
        artist_id: l.artist_id,
        created_at: l.created_at,
        ..row.clone()
    }
}

/// The key a server move pairs by: the audio hash, else the file hash.
fn move_key(l: &Ledger) -> Option<(u8, [u8; 16])> {
    match (l.audio_hash, l.file_hash) {
        (a, _) if a != [0; 16] => Some((0, a)),
        (_, f) if f != [0; 16] => Some((1, f)),
        _ => None,
    }
}

/// What a namer is seeded with.
#[derive(Default)]
struct Seeds {
    /// Files that stay as rows: (path, server, server path).
    kept: Vec<(String, u16, String)>,
    /// Files that stay as they are, no row's.
    plain: Vec<String>,
}

fn namer(s: &Session, layout: &Layout, seeds: &Seeds) -> Result<Namer, Stop> {
    let mut n = Namer::new(layout.clone(), &s.device).map_err(|e| Stop::Refused(e.0.to_string()))?;
    for folder in s.music.folders() {
        n.seed(folder, true);
    }
    for path in &seeds.plain {
        n.seed(path, false);
    }
    for (path, server, server_path) in &seeds.kept {
        n.seed_kept(path, *server, server_path);
    }
    Ok(n)
}

fn wanted<'a>(files: &'a [WantedFile], at: &[usize], f: usize) -> Wanted<'a> {
    let file = &files[f];
    let size = if file.original() { file.ledger.server_size } else { file.size };
    let (id, server_path) = (file.ledger.mstream_id, &file.ledger.server_path);
    Wanted { server: at[file.origin] as u16, id, server_path, size, bytes: file.bytes }
}

pub(super) fn plan(
    s: &mut Session,
    wanted_card: &WantedCard,
    answers: &Answers,
    fetcher: &mut dyn Fetcher,
) -> Result<Plan, Stop> {
    let files = &wanted_card.files;
    let origins = &wanted_card.origins;
    if files.iter().any(|f| f.origin >= origins.len()) {
        return Err(Stop::Refused("a file of no origin".into()));
    }
    let rows = &s.survey.rows;
    let root = s.settled.root.as_ref();
    let old = root.map(|r| r.manifest.server_table()).unwrap_or_default();
    let broken = srvs_breaks(&old.root, &old.entries);
    let mut table: Vec<Server> = vec![old.root];
    table.extend(old.entries.iter().copied());
    let rows_of = |i: usize| rows.iter().filter(|r| r.ledger.server as usize == i).count();
    let mut guards = Vec::new();
    let mut report = Report::default();

    // ── Each origin is one of the card's servers, or a new one (2.11) ──
    let mut at: Vec<Option<usize>> = origins
        .iter()
        .map(|o| (0..table.len()).find(|&i| old.same_server(i, &o.talk) == Verdict::Same))
        .collect();
    let read: HashSet<usize> = at.iter().flatten().copied().collect();
    let mut zero_free = rows_of(0) == 0 && !read.contains(&0);
    for (o, origin) in origins.iter().enumerate() {
        let srv = origin.server;
        if let Some(i) = at[o] {
            let instance = if srv.instance != NO_INSTANCE { srv.instance } else { table[i].instance };
            table[i] = Server { instance, ..srv };
            continue;
        }
        // A server none of the card's is known to be, while the card holds
        // files of one that can't be told apart from it: the same server at
        // another address, perhaps. Asked (U7).
        let unsure: usize = (0..table.len())
            .filter(|&i| !read.contains(&i) && !broken.contains(&i))
            .filter(|&i| old.same_server(i, &origin.talk) == Verdict::Unknown)
            .map(rows_of)
            .sum();
        if unsure > 0 && !answers.new_servers {
            guards.push(Guard::Server { origin: o, rows: unsure });
        }
        // Server 0 with no rows (a card just set up, or one whose files all
        // went) becomes this server; a peer's parent stands there for it.
        if zero_free {
            zero_free = false;
            if !srv.is_peer() {
                table[0] = srv;
                at[o] = Some(0);
                continue;
            }
            table[0] = Server::root(NO_INSTANCE, srv.parent_key);
        }
        table.push(srv);
        at[o] = Some(table.len() - 1);
    }
    let at: Vec<usize> = at.into_iter().map(|i| i.unwrap_or(0)).collect();
    let reader: HashMap<usize, usize> = at.iter().enumerate().map(|(o, &i)| (i, o)).collect();

    // ── Pairing, per server read (2.10.2, step 1) ──
    let mut row_file: Vec<Option<usize>> = vec![None; rows.len()];
    let mut file_row: Vec<Option<usize>> = vec![None; files.len()];
    for (o, &i) in at.iter().enumerate() {
        let of_it = |&r: &usize| rows[r].ledger.server as usize == i;
        let mine: Vec<usize> = (0..rows.len()).filter(of_it).collect();
        let theirs: Vec<usize> = (0..files.len()).filter(|&f| files[f].origin == o).collect();
        let mut by_path: HashMap<&str, usize> = HashMap::new();
        for &f in &theirs {
            by_path.entry(files[f].ledger.server_path.as_str()).or_insert(f);
        }
        for &r in &mine {
            if let Some(&f) = by_path.get(rows[r].ledger.server_path.as_str())
                && file_row[f].is_none()
            {
                (row_file[r], file_row[f]) = (Some(f), Some(r));
            }
        }
        let mut by_key: HashMap<(u8, [u8; 16]), (Vec<usize>, Vec<usize>)> = HashMap::new();
        let unpaired = |&&r: &&usize| row_file[r].is_none() && !rows[r].ledger.server_path.is_empty();
        for &r in mine.iter().filter(unpaired) {
            if let Some(k) = move_key(&rows[r].ledger) {
                by_key.entry(k).or_default().0.push(r);
            }
        }
        for &f in theirs.iter().filter(|&&f| file_row[f].is_none()) {
            if let Some(k) = move_key(&files[f].ledger) {
                by_key.entry(k).or_default().1.push(f);
            }
        }
        for (rs, fs) in by_key.values() {
            if let ([r], [f]) = (rs.as_slice(), fs.as_slice()) {
                (row_file[*r], file_row[*f]) = (Some(*f), Some(*r));
            }
        }
    }

    // ── The table (2.10.2, step 4) ──
    let mut keeps: Vec<Keep> = Vec::new();
    let mut deletes: Vec<Delete> = Vec::new();
    let mut seeds = Seeds::default();
    // Files to name: (file, the row it is of) — new, written again, or
    // replacing a row under a new extension.
    let mut to_name: Vec<(usize, Option<usize>)> = Vec::new();
    // Server moves of a file in its place: (row, file, its path, stamp).
    let mut moves: Vec<(usize, usize, String, u32)> = Vec::new();
    let mut writes: Vec<Write> = Vec::new();
    let item_of = |path: &str| s.music.find(path, false).map(|(_, i)| i);
    for (r, row) in rows.iter().enumerate() {
        let srv = row.ledger.server as usize;
        if srv >= table.len() || broken.contains(&srv) {
            // A server that can't be told: the file is the listener's.
            match &row.side {
                Side::Same { path, .. }
                | Side::Moved { path, .. }
                | Side::Changed { path }
                | Side::Ours { path } => {
                    report.unknown.push(path.clone());
                    seeds.plain.push(path.clone());
                }
                Side::Missing => report.gone.push(row.path.clone()),
            }
            continue;
        }
        let paired = row_file[r];
        match &row.side {
            Side::Missing => match paired {
                Some(f) if s.knobs.rewrite_missing => to_name.push((f, Some(r))),
                _ => report.gone.push(row.path.clone()),
            },
            Side::Changed { path } => match paired {
                Some(f) if s.knobs.replace_edits && card_extension(&files[f]) == ext_of(path) => {
                    let replaces = item_of(path).map(|i| Replaced::of(&i, false));
                    writes.push(Write { path: path.clone(), file: f, from: None, replaces });
                    seeds.kept.push((path.clone(), srv as u16, files[f].ledger.server_path.clone()));
                }
                // The listener's now (U5); a file of the selection still,
                // which goes beside it as any new file whose name is taken
                // does — now, as the next run would.
                paired => {
                    report.edited.push(path.clone());
                    seeds.plain.push(path.clone());
                    to_name.extend(paired.map(|f| (f, None)));
                }
            },
            Side::Ours { path } => match paired {
                Some(f) if card_extension(&files[f]) == ext_of(path) => {
                    let replaces = item_of(path).map(|i| Replaced::of(&i, true));
                    writes.push(Write { path: path.clone(), file: f, from: None, replaces });
                    seeds.kept.push((path.clone(), srv as u16, files[f].ledger.server_path.clone()));
                }
                Some(f) => {
                    deletes.push(Delete::ours(path));
                    to_name.push((f, Some(r)));
                }
                None => deletes.push(Delete::ours(path)),
            },
            Side::Same { path, fat_time } | Side::Moved { path, fat_time } => {
                let in_place = matches!(row.side, Side::Moved { .. });
                let (path, fat_time) = (path.clone(), *fat_time);
                let keep =
                    |ledger: Ledger| Keep { row: r, path: path.clone(), fat_time, server: srv, ledger };
                let size = row.rec.size as u64;
                match (reader.get(&srv), paired) {
                    (None, _) => {
                        keeps.push(keep(row.ledger.clone()));
                        seeds.kept.push((path.clone(), srv as u16, row.ledger.server_path.clone()));
                    }
                    (Some(_), Some(f)) if unchanged(&row.ledger, &files[f]) => {
                        let server_path = files[f].ledger.server_path.clone();
                        if row.ledger.server_path != server_path && !in_place {
                            moves.push((r, f, path, fat_time));
                        } else {
                            keeps.push(keep(refreshed(&row.ledger, &files[f])));
                            seeds.kept.push((path, srv as u16, server_path));
                        }
                    }
                    (Some(_), Some(f)) => {
                        // Changed on the server: replaced in place while its
                        // extension stays, else deleted and written anew.
                        if card_extension(&files[f]) == ext_of(&path) {
                            keeps.push(keep(row.ledger.clone()));
                            let replaces = Some(Replaced { size, fat_time, ours: false });
                            writes.push(Write { path: path.clone(), file: f, from: None, replaces });
                            seeds.kept.push((path, srv as u16, files[f].ledger.server_path.clone()));
                        } else {
                            deletes.push(Delete { path, size, fat_time, removal: false, ours: false });
                            to_name.push((f, Some(r)));
                        }
                    }
                    (Some(&o), None) => {
                        let stays = origins[o].scanning || row.ledger.server_path.is_empty();
                        if stays || s.knobs.removal == Removal::Never {
                            if !stays {
                                report.kept.push(path.clone());
                            }
                            keeps.push(keep(row.ledger.clone()));
                            seeds.kept.push((path, srv as u16, row.ledger.server_path.clone()));
                        } else {
                            deletes.push(Delete { path, size, fat_time, removal: true, ours: false });
                        }
                    }
                }
            }
        }
    }
    for f in (0..files.len()).filter(|&f| file_row[f].is_none()) {
        to_name.push((f, None));
    }
    let recover_free: Vec<String> = s.survey.recover.clone();

    // ── Names: where each file would go on its own (pass A) ──
    let vpath_of = |f: &WantedFile| f.ledger.server_path.split('/').next().unwrap_or("").to_string();
    let libraries: BTreeSet<(u16, String)> =
        files.iter().map(|f| (at[f.origin] as u16, vpath_of(f))).collect();
    let old_roots: Vec<String> = root.map(|r| r.manifest.roots.clone()).unwrap_or_default();
    let several = rows.is_empty() && libraries.len() > 1;
    let vpath = s.knobs.vpath.unwrap_or(!old_roots.is_empty() || several);
    let layout = if vpath { Layout::Vpath(libraries.into_iter().collect()) } else { Layout::Default };
    let mut first = namer(s, &layout, &seeds)?;
    let mut list: Vec<usize> = moves.iter().map(|m| m.1).collect();
    list.extend(to_name.iter().map(|t| t.0));
    let alone: Vec<Wanted> = list.iter().map(|&f| wanted(files, &at, f)).collect();
    let natural: HashMap<usize, String> =
        list.iter().zip(first.place(&alone)).filter_map(|(&f, p)| p.ok().map(|p| (f, p))).collect();

    // A server move whose file would keep its name stays where it is; one
    // that is renamed keeps its row where it is until the rename (a cut
    // before it leaves the file the software's).
    let mut renames: Vec<(usize, usize, String)> = Vec::new();
    for (r, f, path, fat_time) in moves {
        let server = rows[r].ledger.server as usize;
        if natural.get(&f).is_some_and(|p| key_path(p) == key_path(&path)) {
            let ledger = refreshed(&rows[r].ledger, &files[f]);
            keeps.push(Keep { row: r, path: path.clone(), fat_time, server, ledger });
            seeds.kept.push((path, server as u16, files[f].ledger.server_path.clone()));
        } else {
            let ledger = rows[r].ledger.clone();
            keeps.push(Keep { row: r, path: path.clone(), fat_time, server, ledger });
            seeds.plain.push(path.clone());
            renames.push((r, f, path));
        }
    }

    // U6: a listener's file where a new original would go, of its size and
    // fingerprint, is one to adopt.
    let unclaimed: HashMap<Vec<String>, (String, u64, u32)> = s
        .survey
        .unclaimed
        .iter()
        .map(|(p, i)| (key_path(p), (p.clone(), i.size, i.fat_time)))
        .collect();
    let mut candidates: Vec<Adopt> = Vec::new();
    for &(f, _) in &to_name {
        let file = &files[f];
        let Some(path) = natural.get(&f).filter(|_| file.original()) else { continue };
        let Some((listed, size, fat_time)) = unclaimed.get(&key_path(path)) else { continue };
        if *size != file.ledger.server_size {
            continue;
        }
        let card = card_qfp(s.card, &s.music.card_path(listed), *size);
        let card = card.map_err(|e| s.bound.failed(e))?;
        if server_qfp(fetcher, file).is_some_and(|q| q == card) {
            candidates.push(Adopt { file: f, path: listed.clone(), fat_time: *fat_time });
        }
    }
    let adopts = match answers.adopt {
        Some(true) => candidates,
        None if !candidates.is_empty() => {
            guards.push(Guard::Adopt(candidates.iter().map(|a| a.path.clone()).collect()));
            Vec::new()
        }
        _ => Vec::new(),
    };
    let adopted: HashSet<usize> = adopts.iter().map(|a| a.file).collect();
    let adopted_paths: HashSet<Vec<String>> = adopts.iter().map(|a| key_path(&a.path)).collect();
    for a in &adopts {
        let file = &files[a.file];
        seeds.kept.push((a.path.clone(), at[file.origin] as u16, file.ledger.server_path.clone()));
    }
    for (path, _) in &s.survey.unclaimed {
        if !adopted_paths.contains(&key_path(path)) {
            seeds.plain.push(path.clone());
        }
    }

    // ── Names for real (pass B): around everything that stays ──
    let mut second = namer(s, &layout, &seeds)?;
    let mut named: Vec<(usize, Option<String>)> =
        renames.iter().map(|m| (m.1, Some(m.2.clone()))).collect();
    named.extend(to_name.iter().filter(|t| !adopted.contains(&t.0)).map(|t| (t.0, None)));
    let these: Vec<Wanted> = named.iter().map(|&(f, _)| wanted(files, &at, f)).collect();
    // A move the names refuse stays where it is, under its old row; a file
    // they refuse is reported. A cut run's target at the name a file takes
    // is its half-finished work, replaced in place (2.12.3).
    let recovering: HashMap<Vec<String>, &String> =
        recover_free.iter().map(|p| (key_path(p), p)).collect();
    for ((f, from), placed) in named.into_iter().zip(second.place(&these)) {
        if let Ok(path) = placed {
            let cut = recovering.get(&key_path(&path)).and_then(|p| item_of(p));
            let replaces = cut.map(|i| Replaced::of(&i, true));
            writes.push(Write { path, file: f, from, replaces });
        }
    }
    let names_report = second.report();
    report.refused = names_report.skipped;
    report.big_folders = names_report.big_folders;

    // A cut run's target no write takes again goes (2.12.3).
    let written: HashSet<Vec<String>> = writes.iter().map(|w| key_path(&w.path)).collect();
    for path in recover_free {
        if !written.contains(&key_path(&path)) {
            let (size, fat_time) = item_of(&path).map_or((0, 0), |i| (i.size, i.fat_time));
            deletes.push(Delete { path, size, fat_time, removal: false, ours: true });
        }
    }

    // The folders the writes need.
    let mut folders: BTreeSet<String> = BTreeSet::new();
    for w in &writes {
        let mut dir = String::new();
        let names: Vec<&str> = w.path.split('/').collect();
        for name in &names[..names.len() - 1] {
            dir = join(&dir, name);
            if !s.music.has_folder(&dir) {
                folders.insert(dir.clone());
            }
        }
    }

    // ── Q1's gates, and 2.10.2's mass-deletion guard ──
    // The strong gate lists every file the run would take off the card,
    // its own leftovers aside: each delete, and each file replaced in place
    // (deleted at step 8.3 before its new bytes go there).
    let removals: Vec<String> =
        deletes.iter().filter(|d| d.removal).map(|d| d.path.clone()).collect();
    let mut lost: Vec<String> =
        deletes.iter().filter(|d| !d.ours).map(|d| d.path.clone()).collect();
    let replaced = writes.iter().filter(|w| w.replaces.is_some_and(|r| !r.ours));
    lost.extend(replaced.map(|w| w.path.clone()));
    lost.sort();
    lost.dedup();
    let mut strong = !removals.is_empty() && removals.len() * 2 > rows.len();
    // Most of a server's rows gone from its whole manifest (a rebuilt or
    // emptied library, another server at its address): asked before the
    // run deletes or replaces anything, whether or not a file left the
    // selection — the same paths with new audio pair and are replaced.
    for (o, origin) in origins.iter().enumerate() {
        let Some(whole) = &origin.whole else { continue };
        let theirs = rows.iter().map(|r| &r.ledger).filter(|l| l.server as usize == at[o]);
        let hashed: Vec<&Ledger> = theirs.filter(|l| l.audio_hash != [0; 16]).collect();
        let gone = hashed.iter().filter(|l| !whole.contains(&l.audio_hash)).count();
        strong |= !lost.is_empty() && !hashed.is_empty() && gone * 2 > hashed.len();
    }
    if strong && !answers.mass_removals {
        guards.push(Guard::Removals { paths: lost, strong: true });
    } else if removals.len() > s.knobs.gate && !answers.removals && !answers.mass_removals {
        guards.push(Guard::Removals { paths: removals, strong: false });
    }

    // LIBR: the roots this naming gave, and the old ones still holding a
    // file that stays.
    let mut roots: BTreeSet<String> = second.roots().into_iter().collect();
    for old_root in old_roots {
        let inside = |p: &str| p.strip_prefix(&old_root).is_some_and(|rest| rest.starts_with('/'));
        if keeps.iter().any(|k| inside(&k.path)) {
            roots.insert(old_root);
        }
    }
    if !vpath {
        roots.clear();
    }

    let mut adopts_sorted = adopts;
    adopts_sorted.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Plan {
        token: s.token,
        wanted: wanted_card.clone(),
        table,
        origin_at: at,
        keeps,
        adopts: adopts_sorted,
        deletes,
        writes,
        folders: folders.into_iter().collect(),
        roots: roots.into_iter().collect(),
        guards,
        report,
        new_card_id: answers.new_card_id,
    })
}

/// A path's extension, lowercase.
fn ext_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_ascii_lowercase())
}

/// The server's side of a file's fingerprint: two ranged reads (U6). None
/// when the source can't say.
fn server_qfp(fetcher: &mut dyn Fetcher, file: &WantedFile) -> Option<u64> {
    let size = file.ledger.server_size;
    let r = qfp_ranges(size);
    let head = fetcher.read_range(file, 0, r.head as usize).ok()?;
    let tail = fetcher.read_range(file, r.tail_offset, r.tail as usize).ok()?;
    (head.len() as u64 == r.head && tail.len() as u64 == r.tail).then(|| qfp(size, &head, &tail))
}

/// ORIG's origin flags of a file this run puts on the card: HASH_SAMPLED
/// when the server's hash is a sampled digest; VERIFIED when the download's
/// MD5 matched it; ADOPTED for a listener's file taken in.
pub(super) fn origin_flags(file: &WantedFile, verified: bool, adopted: bool) -> u8 {
    let l = &file.ledger;
    let sampled = if hash_sampled(l.hash_v, l.server_size) { HASH_SAMPLED } else { 0 };
    let verified = if verified { VERIFIED } else { 0 };
    sampled | verified | if adopted { ADOPTED } else { 0 }
}
