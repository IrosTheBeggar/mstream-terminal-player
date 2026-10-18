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
//! card, and planned against what the card and its ledger hold. [`source`]
//! is where a run's bytes come from: the server's own folders on this
//! computer's disk when it runs beside the server, else the network.
//! [`setup`] makes a FAT card one the transfer syncs — `/music`, the empty
//! first commit that mints the card's id, and its state file ([`state`]:
//! the card's identity and settings) — and says when a card is a copy of
//! another, with this computer's cache of the cards it has seen
//! ([`cards`]). [`sync`] fills a set-up card from the user's own server:
//! the wanted card of its selection, its conversions and its fit
//! ([`plan`]), the albums' covers ([`covers`]), and a dry run that writes
//! nothing ([`dry`]). [`history`] is the player's play log the other way:
//! each play read from the card, mapped through its ledger, and posted to
//! the server the file came from, which the card's next commit acknowledges.

// The kit and the engine built on it wait for their callers (the Card view,
// the public command line): until they land only the tests and a hidden
// command call them, so the tests' build is the one that holds every item
// to a caller.
#![cfg_attr(not(test), allow(dead_code))]

pub(crate) mod autodj;
pub(crate) mod cards;
pub(crate) mod contract;
pub(crate) mod covers;
pub(crate) mod dry;
pub(crate) mod engine;
pub(crate) mod history;
pub(crate) mod names;
pub(crate) mod plan;
pub(crate) mod setup;
pub(crate) mod source;
pub(crate) mod state;
pub(crate) mod sync;
pub(crate) mod tags;
pub(crate) mod volume;
