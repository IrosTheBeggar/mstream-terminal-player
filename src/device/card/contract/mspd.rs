//! `pending.bin` (MSPD): an unfinished run's plan (docs/METADATA.md
//! 2.12.5).
//!
//! The paths a run will delete, the folders it will create and the paths
//! it will write, each relative to `/music`: written through `pending.tmp`
//! before the run touches the card, rewritten at each checkpoint, deleted
//! after the final commit. A cut run's next run reads it before its
//! clean-up, so it replaces its own half-finished work in place (2.12.3).
//! The device reads only that a plan is there ("The last transfer didn't
//! finish").

use std::cmp::Ordering;

use super::container::{self, Container, FileMeta, FormatSpec, Header, SectionOut, SectionSpec};
use super::container::{MSPD, REQUIRED, STRS, StrCursor, Strs};
use super::msmf::Candidate;
use super::order::{MAX_REL_PATH, compare_file_paths, compare_folder_paths, valid_rel_path};
use super::prim::{fourcc, u32_at, u64_at};
use super::{WriteError, Why};

pub(crate) const PEND: u32 = fourcc(b"PEND");
pub(crate) const MAJOR: u16 = 1;
pub(crate) const HEADER_BYTES: u32 = 56;
pub(crate) const PEND_STRIDE: u32 = 16;

/// PEND `op`: a path the run will write, one it will delete, a folder it
/// will create.
pub(crate) const OP_WRITE: u8 = 1;
pub(crate) const OP_DELETE: u8 = 2;
pub(crate) const OP_FOLDER: u8 = 3;

const SPEC: FormatSpec = FormatSpec {
    magic: MSPD,
    major: MAJOR,
    header_bytes: HEADER_BYTES,
    sections: &[
        SectionSpec { kind: PEND, stride: PEND_STRIDE, required: true },
        SectionSpec { kind: STRS, stride: 0, required: true },
    ],
};

/// One op of a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Op {
    pub(crate) op: u8,
    /// Relative to `/music`.
    pub(crate) path: String,
    /// A write's expected size; 0 for the others.
    pub(crate) expected_size: u32,
}

/// The deletes, then the folders, then the writes.
fn rank(op: u8) -> u8 {
    match op {
        OP_DELETE => 0,
        OP_FOLDER => 1,
        _ => 2,
    }
}

/// Two ops of one group in the canonical order: folders in pre-order,
/// files in the walk's order (2.6.8).
fn compare_paths(op: u8, a: &str, b: &str) -> Ordering {
    if op == OP_FOLDER { compare_folder_paths(a, b) } else { compare_file_paths(a, b) }
}

/// A plan that passed 2.4.3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) header: Header,
    pub(crate) run_id: u64,
    pub(crate) base_generation: u32,
    /// In the file's order: the deletes, the folders, the writes; an op of
    /// a newer writer where it stands.
    pub(crate) ops: Vec<Op>,
}

impl Plan {
    /// An op above 3: a newer writer's. The plan reads, but the software
    /// MUST NOT write to the card, as for an unknown major (2.4.5).
    pub(crate) fn stops_software(&self) -> bool {
        self.ops.iter().any(|o| o.op > OP_FOLDER)
    }

    /// The plan as a candidate of 2.5.4's election (`pending.bin` against
    /// `pending.tmp`): its generation, its runId, its header CRC.
    pub(crate) fn candidate(&self) -> Candidate {
        Candidate { generation: self.header.generation, id: self.run_id, header_crc: self.header.header_crc }
    }

    /// Reads a plan, in the player's order of checks: the frame (2.4.3),
    /// STRS's first byte, then each op — not 0, a path that is a string in
    /// order, within 255 bytes with `/music/`, a relative path of names (so
    /// no plan can name a file outside `/music`), each group in the
    /// canonical order after the one before (an op above 3 is placed in no
    /// order) — then the CRCs.
    pub(crate) fn read(bytes: &[u8]) -> Result<Plan, Why> {
        let c = Container::open(bytes, &SPEC)?;
        let th = c.type_header();
        let (run_id, base_generation) = (u64_at(th, 0), u32_at(th, 8));
        let pend = c.section(0).ok_or(Why::Missing)?;
        let strs = c.section(1).ok_or(Why::Missing)?;
        let sdata = c.data(&strs);
        let mut ss = StrCursor::new(sdata, 0, sdata.len());
        if ss.byte() != Some(0) {
            return Err(Why::String);
        }
        let mut ops: Vec<Op> = Vec::with_capacity(pend.count as usize);
        let mut prev: Option<(u8, String)> = None;
        for i in 0..pend.count {
            let e = c.row(&pend, i, PEND_STRIDE as usize);
            let (op, off, expected_size) = (e[0], u32_at(e, 4), u32_at(e, 8));
            if op == 0 {
                return Err(Why::PendOp);
            }
            if off == 0 {
                return Err(Why::PendPath);
            }
            let path = ss.ordered(off)?;
            if path.len() > MAX_REL_PATH {
                return Err(Why::PathLength);
            }
            if !valid_rel_path(path) {
                return Err(Why::PendPath);
            }
            let Ok(path) = std::str::from_utf8(path) else { return Err(Why::PendPath) };
            if op <= OP_FOLDER {
                if let Some((p_op, p_path)) = &prev {
                    let (r, pr) = (rank(op), rank(*p_op));
                    if r < pr || (r == pr && compare_paths(op, p_path, path).is_ge()) {
                        return Err(Why::PendOrder);
                    }
                }
                prev = Some((op, path.to_owned()));
            }
            ops.push(Op { op, path: path.to_owned(), expected_size });
        }
        if !c.crc_ok(&pend) || !c.crc_ok(&strs) {
            return Err(Why::SectionCrc);
        }
        Ok(Plan { header: *c.header(), run_id, base_generation, ops })
    }
}

/// What a plan's writer is given.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlanIn {
    pub(crate) generation: u32,
    pub(crate) card_id: u64,
    pub(crate) minor: u16,
    pub(crate) run_id: u64,
    pub(crate) base_generation: u32,
    /// In any order: the writer sorts them.
    pub(crate) ops: Vec<Op>,
}

/// Writes a plan in 2.12.5's order: the deletes, then the folders, then the
/// writes, each group in the canonical order (folders in pre-order). A
/// write keeps its expected size; the others write 0. Refused: an op that
/// isn't 1-3, a path that isn't a relative path of names, an op twice.
pub(crate) fn write(p: &PlanIn) -> Result<Vec<u8>, WriteError> {
    let mut ops: Vec<&Op> = p.ops.iter().collect();
    if ops.iter().any(|o| !(OP_WRITE..=OP_FOLDER).contains(&o.op)) {
        return Err(WriteError("PEND: an op that isn't 1-3"));
    }
    if ops.iter().any(|o| !valid_rel_path(o.path.as_bytes())) {
        return Err(WriteError("PEND: a path that isn't a relative path of names"));
    }
    ops.sort_by(|a, b| rank(a.op).cmp(&rank(b.op)).then_with(|| compare_paths(a.op, &a.path, &b.path)));
    if ops.windows(2).any(|w| w[0].op == w[1].op && w[0].path == w[1].path) {
        return Err(WriteError("PEND: an op twice"));
    }
    let mut strs = Strs::new();
    let mut pend = Vec::with_capacity(ops.len() * PEND_STRIDE as usize);
    for o in &ops {
        pend.extend_from_slice(&[o.op, 0, 0, 0]);
        pend.extend_from_slice(&strs.add(o.path.as_bytes()).to_le_bytes());
        pend.extend_from_slice(&(if o.op == OP_WRITE { o.expected_size } else { 0 }).to_le_bytes());
        pend.extend_from_slice(&[0; 4]);
    }
    let mut th = Vec::with_capacity((HEADER_BYTES - 40) as usize);
    th.extend_from_slice(&p.run_id.to_le_bytes());
    th.extend_from_slice(&p.base_generation.to_le_bytes());
    th.extend_from_slice(&[0; 4]);
    let sections = [
        SectionOut { kind: PEND, flags: REQUIRED, count: ops.len() as u32, stride: PEND_STRIDE, data: &pend },
        SectionOut { kind: STRS, flags: REQUIRED, count: 0, stride: 0, data: strs.bytes() },
    ];
    let (minor, generation, card_id) = (p.minor, p.generation, p.card_id);
    let meta = FileMeta { magic: MSPD, major: MAJOR, minor, generation, card_id };
    container::write(&meta, &th, HEADER_BYTES, &sections)
}
