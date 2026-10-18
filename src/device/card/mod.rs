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
//! Auto DJ index, the volumes, the engine — is built on it. [`autodj`] is
//! the Auto DJ index's writer: the neighbour lists from mStream's
//! embeddings, and whether a run writes a new index at all.

// The kit's callers are the transfer's next pieces (the names, the tag
// reader, the engine); until they land only the tests call it.
#![allow(dead_code)]

pub(crate) mod autodj;
pub(crate) mod contract;
