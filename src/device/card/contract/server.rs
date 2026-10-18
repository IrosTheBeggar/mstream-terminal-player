//! Which server filled each file (docs/METADATA.md 2.5.1, 2.5.5, 2.11).
//!
//! A card may hold files from several servers: the root's own (server 0,
//! described by `manifest.bin`'s header), another mStream server reached by
//! its own URL, or a federation peer reached through a server the software
//! signs in to. Each ledger row names its server (ORIG's `server`), and the
//! root's optional `SRVS` section lists the servers other than its own. On
//! the card a server is known by its instance UUID (once mStream gives one)
//! and by the FNV-1a 64 of its normalised base URL — only the hash, never
//! the URL or a token. The device reads none of this.

use std::collections::{BTreeSet, HashMap};

use super::prim::{fnv1a64, u32_at, u64_at, bytes16_at};

/// SRVS flag bit 0: a federation peer, reached through its parent.
pub(crate) const PEER: u32 = 1;
/// An SRVS entry's v1 length.
pub(crate) const SRVS_STRIDE: u32 = 40;
/// An unknown instance.
pub(crate) const NO_INSTANCE: [u8; 16] = [0; 16];

/// A server's base URL normalised (2.5.1): the scheme and the host in lower
/// case; no user name, password, query or fragment; an empty port, or the
/// scheme's default (80 for http, 443 for https), dropped, any other in
/// decimal without leading zeros; the path kept, every trailing slash
/// removed. `HTTP://Music.Example:3000/` is `http://music.example:3000`.
/// None for text with no `scheme://host`, or a port that isn't digits.
pub(crate) fn normalise_url(url: &str) -> Option<String> {
    let sep = url.find("://")?;
    if sep == 0 {
        return None;
    }
    let scheme = url[..sep].to_ascii_lowercase();
    let rest = &url[sep + 3..];
    // The authority runs to the first '/', '?' or '#'; the user name and
    // password end at its last '@'.
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let mut auth = &rest[..auth_end];
    if let Some(at) = auth.rfind('@') {
        auth = &auth[at + 1..];
    }
    // The port: after the last ':' that isn't inside an IPv6 literal's [].
    let (host, port) = match auth.rfind([':', ']']) {
        Some(k) if auth.as_bytes()[k] == b':' => (&auth[..k], Some(&auth[k + 1..])),
        _ => (auth, None),
    };
    if host.is_empty() {
        return None;
    }
    let mut out = format!("{scheme}://{}", host.to_ascii_lowercase());
    if let Some(port) = port {
        if !port.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        if !port.is_empty() {
            // Leading zeros go; a port past any real one keeps its digits'
            // value, capped as the player caps it.
            let digit = |v: u64, c: u8| if v < 1_000_000 { v * 10 + (c - b'0') as u64 } else { v };
            let value = port.bytes().fold(0u64, digit);
            let default = (scheme == "http" && value == 80) || (scheme == "https" && value == 443);
            if !default {
                out.push_str(&format!(":{value}"));
            }
        }
    }
    let path = &rest[auth_end..];
    let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
    out.push_str(path.trim_end_matches('/'));
    Some(out)
}

/// serverUrlKey (2.5.1): the FNV-1a 64 of the normalised base URL; 0
/// (unknown) when the URL has no host.
pub(crate) fn server_url_key(url: &str) -> u64 {
    normalise_url(url).map_or(0, |u| fnv1a64(u.as_bytes()))
}

/// One of the card's servers: server 0 (the root header's fields), or an
/// SRVS entry (2.5.5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Server {
    /// Its instance UUID in RFC 4122 order; zeros: unknown.
    pub(crate) instance: [u8; 16],
    /// The key of the URL the software reaches it at; 0 for a peer.
    pub(crate) url_key: u64,
    /// A peer's: its parent's serverUrlKey; 0 otherwise.
    pub(crate) parent_key: u64,
    /// A peer's: its `id` in its parent's federation peer list; 0 otherwise.
    pub(crate) peer_id: u32,
    pub(crate) flags: u32,
}

impl Server {
    /// Server 0, from the root's header.
    pub(crate) fn root(instance: [u8; 16], url_key: u64) -> Server {
        Server { instance, url_key, ..Server::default() }
    }

    /// A server reached by its own URL.
    pub(crate) fn direct(instance: [u8; 16], url_key: u64) -> Server {
        Server { instance, url_key, ..Server::default() }
    }

    /// Peer `peer_id` of the server whose key is `parent_key`.
    pub(crate) fn peer(instance: [u8; 16], parent_key: u64, peer_id: u32) -> Server {
        Server { instance, parent_key, peer_id, flags: PEER, ..Server::default() }
    }

    pub(crate) fn is_peer(&self) -> bool {
        self.flags & PEER != 0
    }

    /// An SRVS entry's 40 bytes.
    pub(crate) fn encode(&self) -> [u8; 40] {
        let mut out = [0u8; 40];
        out[..16].copy_from_slice(&self.instance);
        out[16..24].copy_from_slice(&self.url_key.to_le_bytes());
        out[24..32].copy_from_slice(&self.parent_key.to_le_bytes());
        out[32..36].copy_from_slice(&self.peer_id.to_le_bytes());
        out[36..40].copy_from_slice(&self.flags.to_le_bytes());
        out
    }

    /// An entry from its first 40 bytes (a later minor's longer row is read
    /// by its prefix).
    pub(crate) fn decode(row: &[u8]) -> Server {
        Server {
            instance: bytes16_at(row, 0),
            url_key: u64_at(row, 16),
            parent_key: u64_at(row, 24),
            peer_id: u32_at(row, 32),
            flags: u32_at(row, 36),
        }
    }
}

/// The servers (i ≥ 1, entry i − 1) whose SRVS entry breaks a writer rule
/// of 2.5.5, given server 0: an entry for the root's own server (its
/// instance, when known, or its key); each of two entries for one server
/// (by a non-zero instance, a non-zero key, or as one parent's peer); a
/// peer without parentKey or peerId, or with a key; an entry without PEER
/// that has no key, or has a parentKey or peerId. A reserved flag bit is a
/// newer minor's (2.4.5), not a broken rule. To a reader such an entry is
/// not damage: its rows' server is unknown.
pub(crate) fn srvs_breaks(root: &Server, entries: &[Server]) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for (i, e) in entries.iter().enumerate() {
        let fields = if e.is_peer() {
            e.url_key == 0 && e.parent_key != 0 && e.peer_id != 0
        } else {
            e.url_key != 0 && e.parent_key == 0 && e.peer_id == 0
        };
        let roots = (e.instance != NO_INSTANCE && e.instance == root.instance)
            || (e.url_key != 0 && e.url_key == root.url_key);
        if !fields || roots {
            out.insert(i + 1);
        }
    }
    // Each server once: both of two entries that name one server break it.
    let mut by_instance: HashMap<[u8; 16], Vec<usize>> = HashMap::new();
    let mut by_key: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut by_peer: HashMap<(u64, u32), Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if e.instance != NO_INSTANCE {
            by_instance.entry(e.instance).or_default().push(i + 1);
        }
        if e.url_key != 0 {
            by_key.entry(e.url_key).or_default().push(i + 1);
        }
        if e.is_peer() && (e.parent_key, e.peer_id) != (0, 0) {
            by_peer.entry((e.parent_key, e.peer_id)).or_default().push(i + 1);
        }
    }
    for same in by_instance.values().chain(by_key.values()).chain(by_peer.values()) {
        if same.len() > 1 {
            out.extend(same);
        }
    }
    out
}

/// What 2.11's test of a ledger row's server comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Same,
    Different,
    Unknown,
}

/// What the software reads now: the server at a URL (its key, its
/// instance when it gives one, and the keys the listener has called the
/// same server as this URL), or one of that server's federation peers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Talk {
    pub(crate) url_key: u64,
    pub(crate) instance: [u8; 16],
    pub(crate) confirmed: Vec<u64>,
    pub(crate) peer: Option<TalkPeer>,
}

/// A peer read through the server of a [`Talk`]: its id in that server's
/// peer list, and its own instance when known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TalkPeer {
    pub(crate) id: u32,
    pub(crate) instance: [u8; 16],
}

/// A card's servers: server 0, then SRVS's entries in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Servers {
    pub(crate) root: Server,
    pub(crate) entries: Vec<Server>,
}

impl Servers {
    /// Server `index` as a reader sees it: none when no entry has that
    /// index, or when its entry breaks a writer rule (2.5.5).
    pub(crate) fn get(&self, index: usize) -> Option<&Server> {
        if index == 0 {
            return Some(&self.root);
        }
        let entry = self.entries.get(index - 1)?;
        (!srvs_breaks(&self.root, &self.entries).contains(&index)).then_some(entry)
    }

    /// A peer's parent on the card: the server reached by its own URL (the
    /// root's, or an entry without PEER) whose key is `key`.
    pub(crate) fn parent(&self, key: u64) -> Option<usize> {
        if key == 0 {
            return None;
        }
        (0..=self.entries.len()).find(|&i| self.get(i).is_some_and(|s| !s.is_peer() && s.url_key == key))
    }

    /// 2.11's test of server `index` against what the software reads.
    ///
    /// Both instances known: the same when they are equal, different when
    /// not. Otherwise a server reached by its own URL is the same by the key
    /// rule (its key is this URL's, or one the listener confirmed), and a
    /// peer when the software reads it as that peer of its parent — the
    /// card's server of parentKey, tested by this rule, or, with no such
    /// server on the card, parentKey by the key rule alone. Anything else
    /// is unknown: reading a server never makes its peers' rows the same,
    /// nor reading a peer its parent's. Different or unknown: the software
    /// MUST NOT delete or replace that row's file without asking.
    pub(crate) fn same_server(&self, index: usize, talk: &Talk) -> Verdict {
        let Some(srv) = self.get(index) else { return Verdict::Unknown };
        let theirs = match &talk.peer {
            Some(p) => p.instance,
            None => talk.instance,
        };
        if srv.instance != NO_INSTANCE && theirs != NO_INSTANCE {
            return if srv.instance == theirs { Verdict::Same } else { Verdict::Different };
        }
        let Some(peer) = &talk.peer else {
            if srv.is_peer() {
                return Verdict::Unknown;
            }
            return if key_rule(srv.url_key, talk) { Verdict::Same } else { Verdict::Unknown };
        };
        if !srv.is_peer() {
            return Verdict::Unknown;
        }
        let via = Talk { peer: None, ..talk.clone() };
        let through = match self.parent(srv.parent_key) {
            Some(parent) => self.same_server(parent, &via) == Verdict::Same,
            None => key_rule(srv.parent_key, &via),
        };
        if through && srv.peer_id == peer.id { Verdict::Same } else { Verdict::Unknown }
    }

    /// Server 0's test alone: a card filled from one server.
    pub(crate) fn same_root(&self, talk: &Talk) -> Verdict {
        self.same_server(0, talk)
    }
}

/// The key rule: `key` (non-zero) is the key of the URL read, or one the
/// listener has called the same server as it.
fn key_rule(key: u64, talk: &Talk) -> bool {
    key != 0 && (key == talk.url_key || talk.confirmed.contains(&key))
}
