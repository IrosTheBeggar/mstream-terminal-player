//! The music on a Core2's SD card, put there by this player with the card in
//! a PC's reader: the Core2's USB is a serial bridge with no mass storage,
//! so the card never travels over the cable the flasher uses. The player's
//! firmware reads what the transfer leaves on the card by a contract of its
//! own (the firmware repo's docs/METADATA.md part 2: `/.mstream` is the
//! software's, `/.player` the device's, `/music` the listener's); this
//! module is the writer of that contract.
//!
//! [`contract`] is the kit: the files themselves, byte for byte, pure code
//! with no I/O. The rest of the transfer — the names, the tag reader, the
//! Auto DJ index, the volumes, the engine — is built on it. [`names`] gives
//! the server's files their paths on the card (2.8.3-2.8.6), pure code too.
//! [`tags`] is the tag reader: a file's record read from its bytes on the
//! card, the way mStream's scanner and the device's own scan read them.
//! [`autodj`] is the Auto DJ index's writer: the neighbour lists from
//! mStream's embeddings, and whether a run writes a new index at all.
//! [`volume`] is the cards in this computer's readers: the drive-safety rule
//! that says which volumes the transfer may write, a card read as the
//! device reads it, and the write half a run drives. [`engine`] is the run:
//! every write a sync makes, in the contract's order (2.12), bound to one
//! card, and planned against what the card and its ledger hold.

// The kit and the engine built on it wait for their callers (the sync of
// the user's server, the Card view): until they land only the tests call
// them, so the tests' build is the one that holds every item to a caller.
#![cfg_attr(not(test), allow(dead_code))]

pub(crate) mod autodj;
pub(crate) mod contract;
pub(crate) mod engine;
pub(crate) mod names;
pub(crate) mod tags;
pub(crate) mod volume;
