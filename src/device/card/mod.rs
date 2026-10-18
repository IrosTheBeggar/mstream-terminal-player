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
//! that says which volumes the transfer may write, and a card read as the
//! device reads it; it writes nothing.

// The kit and the pieces built on it wait for their caller, the engine;
// until it lands only the tests call them. The volume layer's probes for
// the other OSes are compiled everywhere, so their rules run on fakes in
// every OS's tests.
#![allow(dead_code)]

pub(crate) mod autodj;
pub(crate) mod contract;
pub(crate) mod names;
pub(crate) mod tags;
pub(crate) mod volume;
