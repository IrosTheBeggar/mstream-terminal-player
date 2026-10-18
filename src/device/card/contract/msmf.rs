//! `manifest.bin` (MSMF): the commit point (docs/METADATA.md 2.5).
//!
//! A small file that names the companion files of the card's current state
//! — exactly one tags file, then at most one Auto DJ index — each by its
//! generation, length and header CRC. Writing it, through `manifest.tmp`, is
//! the moment a run's work becomes visible. Its header describes server 0,
//! the root's own server; `SRVS` lists the others (2.5.5), and `ACKS` the
//! plays of the device's log the software has (2.5.6). [`elect`] is which
//! of `manifest.bin` and `manifest.tmp` counts (2.5.4); the same rule picks
//! between `pending.bin` and `pending.tmp`.

use super::container::{self, Container, FileMeta, FormatSpec, Header, SectionOut, SectionSpec};
use super::container::{MPDJ, MPTG, MSMF, REQUIRED, STRS, StrCursor, Strs};
use super::order::{compare_names, valid_rel_path};
use super::prim::{bytes16_at, fourcc, u32_at, u64_at};
use super::server::{SRVS_STRIDE, Server, Servers, PEER, srvs_breaks};
use super::{WriteError, Why};

pub(crate) const COMP: u32 = fourcc(b"COMP");
pub(crate) const LIBR: u32 = fourcc(b"LIBR");
pub(crate) const SRVS: u32 = fourcc(b"SRVS");
pub(crate) const ACKS: u32 = fourcc(b"ACKS");

pub(crate) const MAJOR: u16 = 1;
pub(crate) const HEADER_BYTES: u32 = 96;
pub(crate) const COMP_STRIDE: u32 = 40;
pub(crate) const LIBR_STRIDE: u32 = 4;
pub(crate) const ACK_STRIDE: u32 = 24;
/// Header flag bit 0: this commit ended a run (clear: a checkpoint).
pub(crate) const FINAL: u32 = 1;

const SEC_COMP: usize = 0;
const SEC_LIBR: usize = 1;
const SEC_STRS: usize = 2;
const SEC_SRVS: usize = 3;

/// MSMF as the software reads it: the device's three sections, and SRVS,
/// which the device never reads (an optional section it skips). ACKS is
/// not among them: a reader takes it apart from these rules (2.5.6).
const SPEC: FormatSpec = FormatSpec {
    magic: MSMF,
    major: MAJOR,
    header_bytes: HEADER_BYTES,
    sections: &[
        SectionSpec { kind: COMP, stride: COMP_STRIDE, required: true },
        SectionSpec { kind: LIBR, stride: LIBR_STRIDE, required: false },
        SectionSpec { kind: STRS, stride: 0, required: true },
        SectionSpec { kind: SRVS, stride: SRVS_STRIDE, required: false },
    ],
};

/// MSMF as the device reads it (its `lib/core/CardManifest`): SRVS is an
/// unknown optional section to it, skipped unread.
const DEVICE_SPEC: FormatSpec = FormatSpec {
    magic: MSMF,
    major: MAJOR,
    header_bytes: HEADER_BYTES,
    sections: &[
        SectionSpec { kind: COMP, stride: COMP_STRIDE, required: true },
        SectionSpec { kind: LIBR, stride: LIBR_STRIDE, required: false },
        SectionSpec { kind: STRS, stride: 0, required: true },
    ],
};

/// Which sections a reader uses: the software reads SRVS and ACKS; the
/// device never reads SRVS, and before its play log (not built yet) skips
/// ACKS too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Uses {
    pub(crate) servers: bool,
    pub(crate) acks: bool,
}

impl Uses {
    pub(crate) const ALL: Uses = Uses { servers: true, acks: true };
    pub(crate) const DEVICE: Uses = Uses { servers: false, acks: false };
}

/// An ACKS entry (2.5.6): every play slot of this device and epoch with a
/// seq up to `seq` is settled (2.20.6), so the device may trim it. Ordered
/// as ACKS lists them: by the device id's bytes, then the epoch unsigned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Ack {
    /// The device's id as its play slots give it (2.20.4).
    pub(crate) device_id: [u8; 6],
    pub(crate) epoch: u64,
    pub(crate) seq: u32,
}

impl Ack {
    /// The entry's 24 bytes; its reserved bytes 0.
    pub(crate) fn encode(&self) -> [u8; ACK_STRIDE as usize] {
        let mut row = [0; ACK_STRIDE as usize];
        row[..6].copy_from_slice(&self.device_id);
        row[8..16].copy_from_slice(&self.epoch.to_le_bytes());
        row[16..20].copy_from_slice(&self.seq.to_le_bytes());
        row
    }

    /// An entry from the first 24 bytes of its row (a newer minor's longer
    /// row is read by its prefix); the reserved bytes aren't read.
    pub(crate) fn decode(row: &[u8]) -> Ack {
        let mut device_id = [0; 6];
        device_id.copy_from_slice(&row[..6]);
        Ack { device_id, epoch: u64_at(row, 8), seq: u32_at(row, 16) }
    }

    /// The device and epoch the entry is for: each once in an ACKS.
    pub(crate) fn key(&self) -> ([u8; 6], u64) {
        (self.device_id, self.epoch)
    }
}

/// A companion the root names (2.5.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Comp {
    /// `MPTG` or `MPDJ`.
    pub(crate) kind: u32,
    /// The companion's own header generation: older than the root's when a
    /// file is named again.
    pub(crate) generation: u32,
    pub(crate) file_bytes: u32,
    pub(crate) header_crc: u32,
    /// MPDJ: the selection signature; zeros for MPTG.
    pub(crate) key: [u8; 16],
}

impl Comp {
    /// Its file name inside `/.mstream`: `tags-0000002a.bin`,
    /// `autodj-00000029.bin`.
    pub(crate) fn name(&self) -> String {
        companion_name(self.kind, self.generation)
    }

    /// The entry naming a companion file, from its bytes: its generation,
    /// length and header CRC, and an MPDJ's selection signature (its header
    /// bytes 64-79). None when the bytes aren't an MPTG or an MPDJ frame.
    pub(crate) fn of(file: &[u8]) -> Option<Comp> {
        let h = Header::parse(file)?;
        let key = match h.magic {
            MPTG => [0; 16],
            MPDJ if file.len() >= 80 => bytes16_at(file, 64),
            _ => return None,
        };
        let (kind, generation, header_crc) = (h.magic, h.generation, h.header_crc);
        Some(Comp { kind, generation, file_bytes: file.len() as u32, header_crc, key })
    }
}

/// `tags-<gen>.bin` or `autodj-<gen>.bin`, the generation as 8 lowercase
/// hex digits (2.2).
pub(crate) fn companion_name(kind: u32, generation: u32) -> String {
    match kind {
        MPDJ => format!("autodj-{generation:08x}.bin"),
        _ => format!("tags-{generation:08x}.bin"),
    }
}

/// A root that passed 2.4.3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub(crate) header: Header,
    pub(crate) commit_time: u32,
    pub(crate) flags: u32,
    pub(crate) commit_id: u64,
    /// Server 0's instance (zeros: unknown) and URL key (0: unknown).
    pub(crate) server_instance: [u8; 16],
    pub(crate) server_url_key: u64,
    /// The writer and its version; informational, any bad UTF-8 replaced.
    pub(crate) producer: String,
    /// The sync manifest's `revision` when the selection was read; "" unknown.
    pub(crate) server_revision: String,
    pub(crate) base_generation: u32,
    /// The commit's tags file, and its Auto DJ index when it has one.
    pub(crate) tags: Comp,
    pub(crate) autodj: Option<Comp>,
    /// The vpath layout's library roots, relative to `/music`, in byte
    /// order (2.8.6); empty: one root, `/music`.
    pub(crate) roots: Vec<String>,
    /// SRVS's entries, server i being entry i − 1 (empty when SRVS is
    /// absent, or not read: [`Uses::DEVICE`]).
    pub(crate) servers: Vec<Server>,
    /// ACKS's entries, in its order: empty when the root has none, when it
    /// isn't read ([`Uses::DEVICE`]), and when it breaks one of 2.5.6's
    /// checks (the root stays valid; the software writes it again from its
    /// marks).
    pub(crate) acks: Vec<Ack>,
}

impl Manifest {
    pub(crate) fn is_final(&self) -> bool {
        self.flags & FINAL != 0
    }

    /// The root as a candidate of 2.5.4's election.
    pub(crate) fn candidate(&self) -> Candidate {
        let (generation, header_crc) = (self.header.generation, self.header.header_crc);
        Candidate { generation, id: self.commit_id, header_crc }
    }

    /// The card's servers: server 0 from the header, then SRVS's.
    pub(crate) fn server_table(&self) -> Servers {
        let root = Server::root(self.server_instance, self.server_url_key);
        Servers { root, entries: self.servers.clone() }
    }

    /// What [`write`] makes this root from again, and so the base of the
    /// next commit's, which sets its own generation, commit and companions:
    /// the same header values, companions, roots, servers and ACKS entries
    /// (SRVS's and ACKS's when the root was read with [`Uses::ALL`]: each
    /// commit carries the root before's acknowledgements forward, 2.5.6).
    /// The minor is this writer's, 0: a newer minor's additions are dropped
    /// (2.4.5). That is its longer header and rows, its sections, a COMP
    /// entry of a kind v1 doesn't know, and every flag bit v1 doesn't define
    /// (a writer sets only the bits it defines): the header's past FINAL,
    /// an SRVS entry's past PEER.
    /// An entry that breaks a writer rule of 2.5.5 is kept as it is, and
    /// [`write`] refuses it: what becomes of its rows is the run's to say.
    pub(crate) fn to_input(&self) -> ManifestIn {
        ManifestIn {
            generation: self.header.generation,
            card_id: self.header.card_id,
            minor: 0,
            commit_time: self.commit_time,
            flags: self.flags & FINAL,
            commit_id: self.commit_id,
            server_instance: self.server_instance,
            server_url_key: self.server_url_key,
            producer: self.producer.clone(),
            server_revision: self.server_revision.clone(),
            base_generation: self.base_generation,
            comps: std::iter::once(self.tags.clone()).chain(self.autodj.clone()).collect(),
            roots: self.roots.clone(),
            servers: self.servers.iter().map(|s| Server { flags: s.flags & PEER, ..*s }).collect(),
            acks: self.acks.clone(),
        }
    }

    /// Reads a root, in the player's order of checks: the frame (2.4.3);
    /// STRS's first byte and the header's two strings; each COMP entry
    /// (exactly one MPTG, then at most one MPDJ, each named for its
    /// generation; another kind is ignored where it stands); each LIBR root
    /// (a relative path of names, the roots strictly increasing); then the
    /// sections' CRCs. With [`Uses::ALL`] SRVS too: rows of at least 40
    /// bytes, and its CRC. An entry that breaks a writer rule is not damage
    /// (its rows' server is unknown: [`Servers::get`]). And ACKS, found by
    /// its type and checked on its own ([`read_acks`]): it never makes the
    /// root absent.
    pub(crate) fn read(bytes: &[u8], uses: Uses) -> Result<Manifest, Why> {
        let c = Container::open(bytes, if uses.servers { &SPEC } else { &DEVICE_SPEC })?;
        let th = c.type_header();
        let commit_time = u32_at(th, 0);
        let flags = u32_at(th, 4);
        let commit_id = u64_at(th, 8);
        let server_instance = bytes16_at(th, 16);
        let producer_at = u32_at(th, 32);
        let revision_at = u32_at(th, 36);
        let base_generation = u32_at(th, 40);
        let server_url_key = u64_at(th, 48);

        let comp = c.section(SEC_COMP).ok_or(Why::Missing)?;
        let strs = c.section(SEC_STRS).ok_or(Why::Missing)?;
        let sdata = c.data(&strs);
        let mut ss = StrCursor::new(sdata, 0, sdata.len());
        if ss.byte() != Some(0) {
            return Err(Why::String);
        }
        let producer = String::from_utf8_lossy(ss.ordered(producer_at)?).into_owned();
        let server_revision = String::from_utf8_lossy(ss.ordered(revision_at)?).into_owned();

        let (mut tags, mut autodj): (Option<Comp>, Option<Comp>) = (None, None);
        for i in 0..comp.count {
            let e = c.row(&comp, i, COMP_STRIDE as usize);
            let entry = Comp {
                kind: u32_at(e, 0),
                generation: u32_at(e, 4),
                file_bytes: u32_at(e, 8),
                header_crc: u32_at(e, 12),
                key: bytes16_at(e, 24),
            };
            let name = ss.ordered(u32_at(e, 16))?;
            match entry.kind {
                MPTG => {
                    if tags.is_some() || autodj.is_some() || name != entry.name().as_bytes() {
                        return Err(Why::Comp);
                    }
                    tags = Some(Comp { key: [0; 16], ..entry });
                }
                MPDJ => {
                    if tags.is_none() || autodj.is_some() || name != entry.name().as_bytes() {
                        return Err(Why::Comp);
                    }
                    autodj = Some(entry);
                }
                // Another kind: ignored (2.4.5), its name read in its place.
                _ => {}
            }
        }
        let Some(tags) = tags else { return Err(Why::Comp) };

        let mut roots: Vec<String> = Vec::new();
        if let Some(libr) = c.section(SEC_LIBR) {
            for i in 0..libr.count {
                let off = u32_at(c.row(&libr, i, LIBR_STRIDE as usize), 0);
                if off == 0 {
                    return Err(Why::Libr);
                }
                let root = ss.ordered(off)?;
                let after = roots.last().is_none_or(|p| compare_names(p.as_bytes(), root).is_lt());
                if !valid_rel_path(root) || !after {
                    return Err(Why::Libr);
                }
                // The software's own check: a root it can't spell can't be
                // matched on a PC's listing.
                let Ok(root) = std::str::from_utf8(root) else { return Err(Why::Libr) };
                roots.push(root.to_owned());
            }
        }
        if !c.crc_ok(&comp) || c.section(SEC_LIBR).is_some_and(|l| !c.crc_ok(&l)) || !c.crc_ok(&strs) {
            return Err(Why::SectionCrc);
        }
        let mut servers = Vec::new();
        if uses.servers && let Some(srvs) = c.section(SEC_SRVS) {
            if !c.crc_ok(&srvs) {
                return Err(Why::SectionCrc);
            }
            let row = |i| Server::decode(c.row(&srvs, i, SRVS_STRIDE as usize));
            servers = (0..srvs.count).map(row).collect();
        }
        let acks = if uses.acks { read_acks(&c) } else { Vec::new() };
        Ok(Manifest {
            header: *c.header(),
            commit_time,
            flags,
            commit_id,
            server_instance,
            server_url_key,
            producer,
            server_revision,
            base_generation,
            tags,
            autodj,
            roots,
            servers,
            acks,
        })
    }
}

/// ACKS as a reader that uses it takes it (2.5.6): found by its type in the
/// directory and checked apart from the container's rules for the sections
/// MSMF knows, whose stride and length checks would make the root absent.
/// Rows of at least 24 bytes (a longer one read by its prefix), bytes =
/// count × stride, its CRC, and the entries in order, each device and
/// epoch once. One that fails any of these is ignored: ACKS only lets the
/// device trim its log, so losing it only keeps more plays.
fn read_acks(c: &Container<'_>) -> Vec<Ack> {
    let Some(s) = c.find(ACKS) else { return Vec::new() };
    let shaped = s.stride >= ACK_STRIDE && s.count as u64 * s.stride as u64 == s.bytes as u64;
    if !shaped || !c.crc_ok(&s) {
        return Vec::new();
    }
    let acks: Vec<Ack> = (0..s.count).map(|i| Ack::decode(c.row(&s, i, ACK_STRIDE as usize))).collect();
    if acks.windows(2).any(|w| w[0].key() >= w[1].key()) {
        return Vec::new();
    }
    acks
}

/// What a root's writer is given: every value a writer would otherwise
/// choose (2.4.2) comes from its caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ManifestIn {
    pub(crate) generation: u32,
    pub(crate) card_id: u64,
    pub(crate) minor: u16,
    pub(crate) commit_time: u32,
    pub(crate) flags: u32,
    pub(crate) commit_id: u64,
    pub(crate) server_instance: [u8; 16],
    pub(crate) server_url_key: u64,
    pub(crate) producer: String,
    pub(crate) server_revision: String,
    pub(crate) base_generation: u32,
    /// The tags file, then the Auto DJ index when there is one.
    pub(crate) comps: Vec<Comp>,
    /// The vpath layout's roots, in any order: the writer sorts them.
    pub(crate) roots: Vec<String>,
    /// The servers other than the root's, server i being entry i − 1: the
    /// order is the caller's, which keeps each server's index from the
    /// commit before while it can (2.5.5).
    pub(crate) servers: Vec<Server>,
    /// The plays acknowledged, in any order: the writer sorts them. Only a
    /// commit a run makes anyway carries new ones (2.5.6); the caller
    /// raises each to its mark, never lowers one.
    pub(crate) acks: Vec<Ack>,
}

/// Writes a root (2.5): COMP; LIBR when a root other than `/music` is
/// given; STRS (the producer, the revision, the companions' names, the
/// roots); SRVS when other servers are; ACKS, after it, when plays are
/// acknowledged, by device id then epoch. Refused: COMP that isn't one MPTG
/// then at most one MPDJ; a root that isn't a relative path of names, or
/// twice; an SRVS entry that breaks a writer rule of 2.5.5; a header or SRVS
/// flag bit v1 doesn't define (2.4.5); a device and epoch acknowledged
/// twice. That SRVS lists exactly the servers the tags file's rows name is
/// the caller's to keep ([`servers_match_rows`]).
pub(crate) fn write(m: &ManifestIn) -> Result<Vec<u8>, WriteError> {
    let comps_ok = match m.comps.as_slice() {
        [t] => t.kind == MPTG,
        [t, d] => t.kind == MPTG && d.kind == MPDJ,
        _ => false,
    };
    if !comps_ok {
        return Err(WriteError("COMP: one MPTG, then at most one MPDJ"));
    }
    let mut roots: Vec<&str> = m.roots.iter().map(String::as_str).collect();
    if roots.iter().any(|r| !valid_rel_path(r.as_bytes())) {
        return Err(WriteError("LIBR: a root that isn't a relative path of names"));
    }
    roots.sort_by(|a, b| compare_names(a.as_bytes(), b.as_bytes()));
    if roots.windows(2).any(|w| w[0] == w[1]) {
        return Err(WriteError("LIBR: a root twice"));
    }
    if m.producer.contains('\0') || m.server_revision.contains('\0') {
        return Err(WriteError("a string with a NUL"));
    }
    let root = Server::root(m.server_instance, m.server_url_key);
    if !srvs_breaks(&root, &m.servers).is_empty() {
        return Err(WriteError("SRVS: an entry that breaks 2.5.5's writer rules"));
    }
    if m.flags & !FINAL != 0 || m.servers.iter().any(|s| s.flags & !PEER != 0) {
        return Err(WriteError("a flag bit v1 doesn't define"));
    }
    let mut acks = m.acks.clone();
    acks.sort();
    if acks.windows(2).any(|w| w[0].key() == w[1].key()) {
        return Err(WriteError("ACKS: a device and epoch twice"));
    }

    let mut strs = Strs::new();
    let producer = strs.add(m.producer.as_bytes());
    let revision = strs.add(m.server_revision.as_bytes());
    let mut comp = Vec::with_capacity(m.comps.len() * COMP_STRIDE as usize);
    for c in &m.comps {
        let name = strs.add(c.name().as_bytes());
        comp.extend_from_slice(&c.kind.to_le_bytes());
        comp.extend_from_slice(&c.generation.to_le_bytes());
        comp.extend_from_slice(&c.file_bytes.to_le_bytes());
        comp.extend_from_slice(&c.header_crc.to_le_bytes());
        comp.extend_from_slice(&name.to_le_bytes());
        comp.extend_from_slice(&[0; 4]);
        comp.extend_from_slice(if c.kind == MPDJ { &c.key } else { &[0; 16] });
    }
    let libr: Vec<u8> = roots.iter().flat_map(|r| strs.add(r.as_bytes()).to_le_bytes()).collect();
    let srvs: Vec<u8> = m.servers.iter().flat_map(|s| s.encode()).collect();
    let ack_rows: Vec<u8> = acks.iter().flat_map(Ack::encode).collect();

    let mut th = Vec::with_capacity((HEADER_BYTES - 40) as usize);
    th.extend_from_slice(&m.commit_time.to_le_bytes());
    th.extend_from_slice(&m.flags.to_le_bytes());
    th.extend_from_slice(&m.commit_id.to_le_bytes());
    th.extend_from_slice(&m.server_instance);
    th.extend_from_slice(&producer.to_le_bytes());
    th.extend_from_slice(&revision.to_le_bytes());
    th.extend_from_slice(&m.base_generation.to_le_bytes());
    th.extend_from_slice(&[0; 4]);
    th.extend_from_slice(&m.server_url_key.to_le_bytes());

    let count = m.comps.len() as u32;
    let mut sections = vec![SectionOut { kind: COMP, flags: REQUIRED, count, stride: COMP_STRIDE, data: &comp }];
    if !roots.is_empty() {
        let count = roots.len() as u32;
        sections.push(SectionOut { kind: LIBR, flags: 0, count, stride: LIBR_STRIDE, data: &libr });
    }
    sections.push(SectionOut { kind: STRS, flags: REQUIRED, count: 0, stride: 0, data: strs.bytes() });
    if !m.servers.is_empty() {
        // Optional, never REQUIRED: the device doesn't know it (2.5.5).
        let count = m.servers.len() as u32;
        sections.push(SectionOut { kind: SRVS, flags: 0, count, stride: SRVS_STRIDE, data: &srvs });
    }
    if !acks.is_empty() {
        // Optional, never REQUIRED, no strings: the device before its play
        // log doesn't know it (2.5.6).
        let count = acks.len() as u32;
        sections.push(SectionOut { kind: ACKS, flags: 0, count, stride: ACK_STRIDE, data: &ack_rows });
    }
    let (minor, generation, card_id) = (m.minor, m.generation, m.card_id);
    let meta = FileMeta { magic: MSMF, major: MAJOR, minor, generation, card_id };
    container::write(&meta, &th, HEADER_BYTES, &sections)
}

/// 2.5.5's rule between a root and its tags file: SRVS lists exactly the
/// servers the rows name besides the root's, and every row names 0 or an
/// entry. `rows` is each ledger row's `server`.
pub(crate) fn servers_match_rows(servers: usize, rows: impl IntoIterator<Item = u16>) -> bool {
    let mut used = vec![false; servers];
    for s in rows {
        match s as usize {
            0 => {}
            i if i <= servers => used[i - 1] = true,
            _ => return false,
        }
    }
    used.into_iter().all(|u| u)
}

/// A companion a root names is valid only when it validates (the caller
/// read it) and its header agrees with the entry: its magic, generation,
/// length and header CRC; a tags file of source 2 or 3; an Auto DJ index
/// whose selection signature is the entry's key (2.5.3). One that fails is
/// absent; the root stays valid.
pub(crate) fn companion_matches(
    entry: &Comp,
    companion: &Header,
    mptg_source: u8,
    mpdj_sig: Option<&[u8; 16]>,
) -> bool {
    if companion.magic != entry.kind
        || companion.generation != entry.generation
        || companion.file_bytes != entry.file_bytes
        || companion.header_crc != entry.header_crc
    {
        return false;
    }
    match entry.kind {
        MPTG => mptg_source == 2 || mptg_source == 3,
        MPDJ => mpdj_sig == Some(&entry.key),
        _ => false,
    }
}

/// One of two files that may hold the same thing — `manifest.bin` and
/// `manifest.tmp`, or `pending.bin` and `pending.tmp` — as the election
/// sees it: its generation, its id (commitId; a plan's runId) and its
/// header CRC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) generation: u32,
    pub(crate) id: u64,
    pub(crate) header_crc: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    None,
    Bin,
    Tmp,
}

/// 2.5.4's answer, and whether the two are one commit seen twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Election {
    pub(crate) pick: Pick,
    /// The same commitId and headerCrc: a rename cut between its two
    /// directory writes may have left both names on one cluster chain.
    /// Readers use either; the software deletes neither until a disk check
    /// has run (2.12.1, "Cut renames").
    pub(crate) same_commit: bool,
}

/// Which of the two counts (2.5.4): of two valid files the higher
/// generation, `.bin` on a tie; else the one valid file; else none (the
/// card has no transfer data). None for a file that didn't validate.
pub(crate) fn elect(bin: Option<Candidate>, tmp: Option<Candidate>) -> Election {
    match (bin, tmp) {
        (Some(b), Some(t)) => Election {
            pick: if t.generation > b.generation { Pick::Tmp } else { Pick::Bin },
            same_commit: b.id == t.id && b.header_crc == t.header_crc,
        },
        (Some(_), None) => Election { pick: Pick::Bin, same_commit: false },
        (None, Some(_)) => Election { pick: Pick::Tmp, same_commit: false },
        (None, None) => Election { pick: Pick::None, same_commit: false },
    }
}
