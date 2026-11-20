//! The Peer recs room's wire (peer-recommendations plan §3.1–3.6, taste
//! generation 3): the network taste body the room sends, the peers route's
//! strip, an admin's async Add, and a preview plug-in's clip.
//!
//! Discover's own taste types (`TasteRequest`, `TasteLimit`) are not
//! touched: the room's ask has a body of its own (peer-recs contract
//! §1.4), so Discover's golden bodies cannot change by construction.
//!
//! The parsers follow the phone's rules (mstream_music
//! `lib/objects/peer_recs.dart`, `discovery_plugins.dart` `PluginPreview`):
//! a bad row is dropped, never fatal, and what the server stamps (a label,
//! a hue) is kept as it came, never computed here.

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde_json::Value;

/// The most seeds the room's taste ask names (the server's cap).
pub const SEEDS_CAP: u32 = 12;
/// The network rows the room asks for (peer-recommendations plan §3.5 R1).
pub const NETWORK_LIMIT: u32 = 32;

/// A p2p endpoint id as the wire carries it: exactly 64 lower-case hex.
pub fn is_endpoint_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// ── The body ────────────────────────────────────────────────────────────────

/// `limit` on the room's taste ask: both caps always sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NetworkLimit {
    pub seeds: u32,
    pub network: u32,
}

/// `POST /api/v1/discovery/taste` as the Peer recs room asks it
/// (peer-recommendations plan §3.5 R1–R3): the network walk alone, never
/// moods, and the random pick walked too.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkTasteRequest {
    pub window: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pin: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Always `["network"]`.
    pub sources: Vec<String>,
    pub new_artists_only: bool,
    pub limit: NetworkLimit,
    /// Always true: a new account's random pick walks the shelf too.
    pub network_on_random_seed: bool,
    /// The tab's peer. Empty for All, and then left out: the route answers
    /// 400 to `[]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub network_peers: Vec<String>,
}

impl NetworkTasteRequest {
    /// The body for `window` days, the visit's `pin` paths and `exclude`
    /// hashes, at most `seeds_limit` seeds (capped at [`SEEDS_CAP`]), and
    /// the tab's peer. The tab is lower-cased and kept only when it is an
    /// endpoint id, so a bad one asks All rather than earning a 400.
    pub fn new(
        window: u32,
        pin: Vec<String>,
        exclude: Vec<String>,
        seeds_limit: u32,
        tab: Option<&str>,
    ) -> NetworkTasteRequest {
        let network_peers = tab
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| is_endpoint_id(t))
            .into_iter()
            .collect();
        NetworkTasteRequest {
            window,
            pin,
            exclude,
            sources: vec!["network".to_string()],
            new_artists_only: false,
            limit: NetworkLimit { seeds: seeds_limit.min(SEEDS_CAP), network: NETWORK_LIMIT },
            network_on_random_seed: true,
            network_peers,
        }
    }
}

// ── Small readers ───────────────────────────────────────────────────────────

/// A count: a whole number ≥ 0, else 0 (the phone's `_whole`).
fn whole(v: Option<&Value>) -> u32 {
    v.and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.0 && n.fract() == 0.0)
        .map_or(0, |n| n.min(f64::from(u32::MAX)) as u32)
}

/// A server-stamped hue, 0–359: a finite number rounded half up and
/// folded; anything else 0 (the phone's `_hue`).
fn hue(v: Option<&Value>) -> u16 {
    v.and_then(Value::as_f64)
        .filter(|h| h.is_finite())
        .map_or(0, |h| ((h + 0.5).floor() as i64).rem_euclid(360) as u16)
}

fn string(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_string)
}

/// A non-blank string, at most `max` bytes (cut on a char boundary).
fn words(v: Option<&Value>, max: usize) -> Option<String> {
    let s = v.and_then(Value::as_str)?.trim();
    if s.is_empty() {
        return None;
    }
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(s[..end].to_string())
}

fn flag(v: Option<&Value>) -> bool {
    v.and_then(Value::as_bool).unwrap_or(false)
}

/// A URL a client may fetch or play: http or https, nothing else.
fn web_url(v: Option<&Value>) -> Option<String> {
    let s = v.and_then(Value::as_str)?.trim();
    let lower = s.get(..8).unwrap_or(s).to_ascii_lowercase();
    let ok = (lower.starts_with("http://") && s.len() > 7) || (lower.starts_with("https://") && s.len() > 8);
    ok.then(|| s.to_string())
}

// ── The peers answer ────────────────────────────────────────────────────────

/// `GET /api/v1/discovery/p2p/peers` (peer-recommendations plan §3.2): the
/// strip's rows in the server's order, shelf peers first (online first,
/// then by label), then an admin's addable peers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct P2pPeersAnswer {
    /// This caller may Add (an admin, not locked out, from an allowed
    /// address). The route's word alone, never the account's admin flag.
    pub can_add: bool,
    pub shelf: PeerShelf,
    pub peers: Vec<P2pPeer>,
}

/// The shelf's counts: unblocked snapshots held, and those the room can
/// search.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeerShelf {
    pub held: u32,
    pub searchable: u32,
    /// The p2p stack is running (false on the stage, which never starts it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub running: bool,
}

/// One row of the strip: a shelf peer or, for an admin, an addable one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct P2pPeer {
    pub endpoint_id: String,
    /// The peer's announced name; empty when absent.
    pub name: String,
    /// What the tab draws: the row's label when not blank, else the name,
    /// trimmed.
    pub label: String,
    /// Parsed for parity; the terminal draws no monogram (peer-recs
    /// contract §1.12).
    #[cfg_attr(not(test), allow(dead_code))]
    pub monogram: String,
    /// Parsed for parity; the terminal draws no peer hue.
    #[cfg_attr(not(test), allow(dead_code))]
    pub hue: u16,
    pub online: bool,
    pub on_shelf: bool,
    pub pinned: bool,
    /// The snapshot's track count; never drawn.
    #[cfg_attr(not(test), allow(dead_code))]
    pub row_count: Option<u64>,
    /// The Add the server holds for this peer (admins only).
    pub add: Option<PeerAddState>,
}

impl P2pPeersAnswer {
    /// The answer off any JSON: a missing `shelf` reads zeros, a row whose
    /// id is not an endpoint id or repeats an earlier one is dropped.
    pub fn from_value(v: &Value) -> P2pPeersAnswer {
        let shelf = v.get("shelf");
        let mut peers: Vec<P2pPeer> = Vec::new();
        for row in v.get("peers").and_then(Value::as_array).into_iter().flatten() {
            if let Some(row) = P2pPeer::from_value(row)
                && !peers.iter().any(|p| p.endpoint_id == row.endpoint_id)
            {
                peers.push(row);
            }
        }
        P2pPeersAnswer {
            can_add: flag(v.get("canAdd")),
            shelf: PeerShelf {
                held: whole(shelf.and_then(|s| s.get("held"))),
                searchable: whole(shelf.and_then(|s| s.get("searchable"))),
                running: flag(shelf.and_then(|s| s.get("running"))),
            },
            peers,
        }
    }

    /// The row of `id`, or `None`.
    pub fn row(&self, id: &str) -> Option<&P2pPeer> {
        self.peers.iter().find(|p| p.endpoint_id == id)
    }
}

impl<'de> Deserialize<'de> for P2pPeersAnswer {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(P2pPeersAnswer::from_value(&Value::deserialize(d)?))
    }
}

impl P2pPeer {
    /// One row, or `None` for anything but an object with a valid id. The
    /// id is not folded: the wire sends lower case, and anything else is
    /// not a row (the webapp's `isEndpointId`).
    pub fn from_value(v: &Value) -> Option<P2pPeer> {
        let endpoint_id = v.get("endpointId").and_then(Value::as_str).filter(|id| is_endpoint_id(id))?;
        let name = string(v.get("name")).unwrap_or_default();
        let label = v
            .get("label")
            .and_then(Value::as_str)
            .filter(|l| !l.trim().is_empty())
            .map_or_else(|| name.trim().to_string(), str::to_string);
        Some(P2pPeer {
            endpoint_id: endpoint_id.to_string(),
            label,
            monogram: string(v.get("monogram")).unwrap_or_default(),
            hue: hue(v.get("hue")),
            online: flag(v.get("online")),
            on_shelf: flag(v.get("onShelf")),
            pinned: flag(v.get("pinned")),
            row_count: v
                .get("rowCount")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && *n >= 0.0)
                .map(|n| n as u64),
            add: v.get("add").and_then(PeerAddState::from_value),
            name,
        })
    }
}

// ── Adds ────────────────────────────────────────────────────────────────────

/// An Add's state on the wire (mStream's `discovery-peer-add.js`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerAddKind {
    Adding,
    Done,
    Failed,
}

/// One Add state. The failure `code` (`no-room`, `no-disk`, `offline`,
/// `bad-snapshot`, `gone`, `failed`) is never shown as words: only "no
/// room" or "try again" (peer-recs contract §1.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAddState {
    pub state: PeerAddKind,
    pub code: Option<String>,
    /// When the server set it, an ISO string as it came.
    #[cfg_attr(not(test), allow(dead_code))]
    pub at: Option<String>,
}

impl PeerAddState {
    /// `None` for anything but an object with a known `state`.
    pub fn from_value(v: &Value) -> Option<PeerAddState> {
        let state = match v.get("state")?.as_str()? {
            "adding" => PeerAddKind::Adding,
            "done" => PeerAddKind::Done,
            "failed" => PeerAddKind::Failed,
            _ => return None,
        };
        Some(PeerAddState {
            state,
            code: string(v.get("code")),
            at: string(v.get("at")),
        })
    }

    /// A failed Add that found no room on the shelf: drawn "no room",
    /// inert, never posted again.
    pub fn no_room(&self) -> bool {
        self.state == PeerAddKind::Failed && self.code.as_deref() == Some("no-room")
    }
}

/// `POST /api/v1/admin/discovery/p2p/peer-dbs/add`'s outcome: the HTTP
/// status of any answer but a 401 (0 = no answer at all), and the state a
/// 200 or 202 body carries. The server's words are never kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAddAnswer {
    pub status: u16,
    pub state: Option<PeerAddState>,
}

impl PeerAddAnswer {
    /// The outcome of a reply: the state read only off a 200 or 202 body.
    pub fn of_reply(status: u16, body: &str) -> PeerAddAnswer {
        let state = if status == 200 || status == 202 {
            serde_json::from_str::<Value>(body).ok().as_ref().and_then(PeerAddState::from_value)
        } else {
            None
        };
        PeerAddAnswer { status, state }
    }
}

// ── Previews ────────────────────────────────────────────────────────────────

/// A preview plug-in's clip for a lead (`result.preview` of
/// `POST /api/v1/discovery/plugins/:name/resolve`; mStream's `deezer.js`
/// and `itunes.js` `choose`). It has no artwork field: the terminal never
/// asks for art (peer-recommendations plan §2 item 10).
#[derive(Debug, Clone, PartialEq)]
pub struct PluginPreview {
    /// The plug-in that found it (`deezer`, `itunes`).
    pub provider: String,
    /// The clip: http or https, played verbatim.
    pub url: String,
    /// The clip's length as the catalogue states it; 30 when it does not.
    pub seconds: u32,
    /// What the catalogue matched.
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// The words the catalogue asks to be shown beside the clip, as sent.
    pub attribution: Option<String>,
    /// The store page, kept only when it is https (the attribution opens it).
    pub link: Option<String>,
    /// How closely it matched the lead, 0–1.
    pub match_score: Option<f64>,
}

impl PluginPreview {
    /// One `preview` object, the answer's `plugin` standing in for a
    /// missing provider. `None` when there is no playable clip.
    pub fn from_value(preview: &Value, plugin: Option<&str>) -> Option<PluginPreview> {
        if !preview.is_object() {
            return None;
        }
        let url = web_url(preview.get("url"))?;
        let provider = words(preview.get("provider"), 64).or_else(|| {
            plugin.map(str::trim).filter(|p| !p.is_empty()).map(str::to_string)
        })?;
        let seconds = preview
            .get("seconds")
            .and_then(Value::as_f64)
            .filter(|s| s.is_finite() && *s >= 1.0)
            .map_or(30, |s| s.min(f64::from(u32::MAX)) as u32);
        let link = web_url(preview.get("link")).filter(|l| l.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://")));
        Some(PluginPreview {
            provider,
            url,
            seconds,
            title: words(preview.get("title"), 512),
            artist: words(preview.get("artist"), 512),
            album: words(preview.get("album"), 512),
            attribution: words(preview.get("attribution"), 256),
            link,
            match_score: preview
                .get("match")
                .and_then(Value::as_f64)
                .filter(|m| m.is_finite())
                .map(|m| m.clamp(0.0, 1.0)),
        })
    }
}

/// `POST /api/v1/discovery/plugins/:name/resolve`, as a preview plug-in
/// answers it: `{plugin, capabilities, key, recommendation, result}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PreviewResolveResponse {
    pub plugin: Option<String>,
    pub result: Option<PreviewResult>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PreviewResult {
    pub preview: Option<Value>,
}

impl PreviewResolveResponse {
    /// The clip, or `None` for `preview: null` and for anything that is no
    /// playable clip.
    pub fn into_preview(self) -> Option<PluginPreview> {
        let preview = self.result?.preview?;
        PluginPreview::from_value(&preview, self.plugin.as_deref())
    }
}

/// The stage's answers (mStream `test/stage/peer-recs-stage.mjs`), shared
/// with the e2e fake through `test/fixtures/peer_recs/` (peer-recs plan
/// D25).
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::api::types::TasteResponse;

    /// The stage's ids: its `endpointId(prefix, name)`, which is the prefix
    /// and the rest of sha256("peer-recs-stage:<name>") in hex.
    pub const BASEMENT: &str = "ba5eaa206eb20c70adb0cf0ad30db63b8920232f5da363f97a63e83de67cbfa0";
    pub const KITCHEN: &str = "cafea9ed902dcc07c36cadfab1be6f48860e86c105e73497e890998d6305e2dc";
    /// The peer labelled `mStream · 3f9a`.
    pub const F3A: &str = "3f9a1c7cbd00116d0312b285b279e969ca58077702b933169bdb017e0180a4c8";
    pub const ATTIC: &str = "a771c019b58e7329697be5d4bc8d2b14ddd82e4738bb019bca19ae733f53d2a7";
    pub const LIGHTHOUSE: &str = "11fe908bbf9c90af43356bae07fe59911a2e2fa2f9e620d429bb5fe36d05959c";
    pub const STATIC: &str = "57a76a82389134bb7ecdb2db349e629901a5b586b0ae8794d56c4f208d18e557";
    pub const ROOFTOP: &str = "f00f709723df9278bcb2b7f62724bb0dee17a42b362970ab516a14a51ba72cff";

    /// Every fixture's name, in the plan's order.
    pub const PEERS: [&str; 4] =
        ["peers_admin", "peers_member", "peers_admin_empty_shelf", "peers_admin_add_states"];
    pub const TASTES: [&str; 11] = [
        "taste_all",
        "taste_kitchen",
        "taste_basement",
        "taste_3f9a",
        "taste_attic",
        "taste_empty_shelf",
        "taste_not_on_shelf",
        "taste_random",
        "taste_scan_pending",
        "taste_network_null",
        "taste_error",
    ];
    pub const PREVIEWS: [&str; 3] = ["preview_deezer", "preview_itunes", "preview_null"];

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!("../../../test/fixtures/peer_recs/", $name, ".json"))
        };
    }

    /// A fixture's text, by its file name without `.json`.
    pub fn text(name: &str) -> &'static str {
        match name {
            "peers_admin" => fixture!("peers_admin"),
            "peers_member" => fixture!("peers_member"),
            "peers_admin_empty_shelf" => fixture!("peers_admin_empty_shelf"),
            "peers_admin_add_states" => fixture!("peers_admin_add_states"),
            "taste_all" => fixture!("taste_all"),
            "taste_kitchen" => fixture!("taste_kitchen"),
            "taste_basement" => fixture!("taste_basement"),
            "taste_3f9a" => fixture!("taste_3f9a"),
            "taste_attic" => fixture!("taste_attic"),
            "taste_empty_shelf" => fixture!("taste_empty_shelf"),
            "taste_not_on_shelf" => fixture!("taste_not_on_shelf"),
            "taste_random" => fixture!("taste_random"),
            "taste_scan_pending" => fixture!("taste_scan_pending"),
            "taste_network_null" => fixture!("taste_network_null"),
            "taste_error" => fixture!("taste_error"),
            "preview_deezer" => fixture!("preview_deezer"),
            "preview_itunes" => fixture!("preview_itunes"),
            "preview_null" => fixture!("preview_null"),
            other => panic!("no peer-recs fixture named {other}"),
        }
    }

    /// A fixture as JSON.
    pub fn json(name: &str) -> Value {
        serde_json::from_str(text(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// A peers answer (`peers_*`).
    pub fn peers(name: &str) -> P2pPeersAnswer {
        P2pPeersAnswer::from_value(&json(name))
    }

    /// A taste answer (`taste_*`).
    pub fn taste(name: &str) -> TasteResponse {
        serde_json::from_value(json(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// A resolve's whole answer (`preview_*`), as the route sends it.
    pub fn preview(name: &str) -> Value {
        json(name)
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::api::types::{NetworkReason, SeedSource};

    fn id(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }

    #[test]
    fn the_network_body_spells_r1() {
        let body = NetworkTasteRequest::new(30, Vec::new(), Vec::new(), SEEDS_CAP, None);
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({
                "window": 30, "sources": ["network"], "newArtistsOnly": false,
                "limit": { "seeds": 12, "network": 32 }, "networkOnRandomSeed": true
            })
        );
        // The seeds cap holds whatever is asked.
        assert_eq!(NetworkTasteRequest::new(30, Vec::new(), Vec::new(), 40, None).limit.seeds, 12);
    }

    #[test]
    fn the_network_body_adds_a_tab_and_never_an_empty_list() {
        let pins = vec!["a.mp3".to_string(), "b.mp3".to_string()];
        let body = NetworkTasteRequest::new(7, pins, vec!["h1".into()], 2, Some(KITCHEN));
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({
                "window": 7, "pin": ["a.mp3", "b.mp3"], "exclude": ["h1"], "sources": ["network"],
                "newArtistsOnly": false, "limit": { "seeds": 2, "network": 32 },
                "networkOnRandomSeed": true, "networkPeers": [KITCHEN]
            })
        );
        // An upper-case id is folded; a bad one is dropped, never sent empty.
        let upper = KITCHEN.to_ascii_uppercase();
        let folded = NetworkTasteRequest::new(30, Vec::new(), Vec::new(), 12, Some(&upper));
        assert_eq!(folded.network_peers, [KITCHEN]);
        for bad in [&KITCHEN[..63], "zz", "", &format!("{KITCHEN}0")] {
            let body = NetworkTasteRequest::new(30, Vec::new(), Vec::new(), 12, Some(bad));
            let wire = serde_json::to_value(&body).unwrap();
            assert!(wire.get("networkPeers").is_none(), "{bad}: {wire}");
        }
        assert!(is_endpoint_id(&id('a')) && !is_endpoint_id(&id('A')) && !is_endpoint_id(&id('g')));
    }

    #[test]
    fn the_peers_answer_parses_the_server_plans_sample() {
        let (a, b, c) = (id('a'), id('b'), id('c'));
        let json = serde_json::json!({
            "canAdd": true,
            "shelf": { "held": 3, "searchable": 2, "running": true },
            "peers": [
                { "endpointId": a, "name": "Basement Hi-Fi", "label": "Basement Hi-Fi", "monogram": "BH",
                  "hue": 210, "online": true, "onShelf": true, "pinned": true, "rowCount": 5, "add": null },
                { "endpointId": b, "name": "  mStream ", "label": "  ", "monogram": "mS", "hue": 140.5,
                  "online": false, "onShelf": true, "pinned": true, "rowCount": 2,
                  "add": { "state": "parked", "code": null, "at": null } },
                { "endpointId": &a[..63], "name": "short" },
                { "endpointId": a, "name": "twin" },
                { "endpointId": c, "name": "Rooftop FM", "label": "Rooftop FM", "online": true,
                  "onShelf": false, "rowCount": 3100,
                  "add": { "state": "failed", "code": "offline", "at": "2026-11-20T06:09:06.324Z" } },
                "not a row"
            ]
        });
        let answer: P2pPeersAnswer = serde_json::from_value(json).unwrap();
        assert!(answer.can_add);
        assert_eq!(answer.shelf, PeerShelf { held: 3, searchable: 2, running: true });
        let ids: Vec<&str> = answer.peers.iter().map(|p| p.endpoint_id.as_str()).collect();
        assert_eq!(ids, [a.as_str(), b.as_str(), c.as_str()]);
        assert_eq!(answer.row(&a).unwrap().label, "Basement Hi-Fi");
        // A blank label falls back to the trimmed name; an unknown add state
        // is no state.
        let row = answer.row(&b).unwrap();
        assert_eq!((row.label.as_str(), row.hue, row.add.as_ref()), ("mStream", 141, None));
        let rooftop = answer.row(&c).unwrap().add.clone().unwrap();
        assert_eq!(rooftop.state, PeerAddKind::Failed);
        assert!(!rooftop.no_room());
        let no_room = PeerAddState { state: PeerAddKind::Failed, code: Some("no-room".into()), at: None };
        assert!(no_room.no_room());
        assert!(!PeerAddState { state: PeerAddKind::Adding, ..no_room.clone() }.no_room());
        // Odd counts read 0; a body that is not an object is an empty strip.
        let odd = P2pPeersAnswer::from_value(&serde_json::json!({
            "canAdd": "yes", "shelf": { "held": -1, "searchable": 2.5 }
        }));
        assert_eq!((odd.can_add, odd.shelf.held, odd.shelf.searchable), (false, 0, 0));
        assert_eq!(P2pPeersAnswer::from_value(&serde_json::json!([1, 2])), P2pPeersAnswer::default());
    }

    #[test]
    fn an_add_answer_reads_its_state_only_from_a_200_or_202() {
        let adding = r#"{"endpointId":"x","state":"adding","code":null,"at":"2026-11-20T06:09:06Z"}"#;
        let answer = PeerAddAnswer::of_reply(202, adding);
        assert_eq!(answer.state.as_ref().map(|s| s.state), Some(PeerAddKind::Adding));
        assert_eq!(PeerAddAnswer::of_reply(409, r#"{"state":"failed"}"#).state, None);
        assert_eq!(PeerAddAnswer::of_reply(200, "not json"), PeerAddAnswer { status: 200, state: None });
    }

    fn resolve(preview: Value) -> Option<PluginPreview> {
        serde_json::from_value::<PreviewResolveResponse>(serde_json::json!({
            "plugin": "deezer", "capabilities": ["preview"], "result": { "preview": preview }
        }))
        .unwrap()
        .into_preview()
    }

    #[test]
    fn a_preview_answer_parses_deezer_itunes_and_null() {
        let deezer: PreviewResolveResponse = serde_json::from_value(preview("preview_deezer")).unwrap();
        let deezer = deezer.into_preview().unwrap();
        assert_eq!(deezer.provider, "deezer");
        assert_eq!(deezer.url, "http://127.0.0.1:3996/preview/3.mp3");
        assert_eq!(deezer.attribution.as_deref(), Some("Preview from Deezer"));
        assert_eq!((deezer.seconds, deezer.match_score), (30, Some(1.0)));
        // The stage's store link is http: dropped, as every non-https link.
        assert_eq!(deezer.link, None);
        let itunes: PreviewResolveResponse = serde_json::from_value(preview("preview_itunes")).unwrap();
        let itunes = itunes.into_preview().unwrap();
        assert_eq!(itunes.provider, "itunes");
        assert_eq!(itunes.attribution.as_deref(), Some("Provided courtesy of iTunes"));
        assert!(itunes.link.as_deref().is_some_and(|l| l.starts_with("https://")));
        let null: PreviewResolveResponse = serde_json::from_value(preview("preview_null")).unwrap();
        assert_eq!(null.into_preview(), None);
        // Only http(s) plays; an http clip is kept.
        for url in ["ftp://x/1.mp3", "javascript:alert(1)", "", "http://"] {
            assert_eq!(resolve(serde_json::json!({ "url": url })), None, "{url}");
        }
        let plain = resolve(serde_json::json!({ "url": "http://127.0.0.1:3996/preview/1.mp3",
                                                "link": "http://x/track/1", "match": 4 }))
        .unwrap();
        // No provider: the answer's plug-in; no seconds: 30; match clamped.
        assert_eq!(plain.provider, "deezer");
        assert_eq!((plain.seconds, plain.link, plain.match_score), (30, None, Some(1.0)));
        let https = resolve(serde_json::json!({ "url": "https://cdn/1.mp3", "link": "https://x/t/1",
                                                "seconds": 29.6, "match": -1 }))
        .unwrap();
        assert_eq!((https.link.as_deref(), https.seconds, https.match_score), (Some("https://x/t/1"), 29, Some(0.0)));
        assert_eq!(serde_json::from_value::<PreviewResolveResponse>(serde_json::json!({}))
            .unwrap()
            .into_preview(), None);
    }

    #[test]
    fn a_preview_never_keeps_artwork() {
        let answer = preview("preview_deezer");
        let art = answer["result"]["preview"]["artwork"].as_str().unwrap().to_string();
        assert!(!art.is_empty());
        let clip = serde_json::from_value::<PreviewResolveResponse>(answer).unwrap().into_preview().unwrap();
        let dump = format!("{clip:?}");
        assert!(!dump.contains(&art) && !dump.contains("cover"), "{dump}");
    }

    #[test]
    fn every_fixture_parses() {
        // The batch: twelve leads in DECISIONS §2's order, each with its
        // label, tile and export id, from the four shelf peers.
        let all = taste("taste_all");
        assert_eq!(all.seed_source, SeedSource::History);
        let leads = &all.network.as_ref().unwrap().results;
        let titles: Vec<&str> = leads.iter().filter_map(|l| l.title.as_deref()).collect();
        assert_eq!(
            titles,
            [
                "Salt Road", "Breakwater", "Signal Fires", "Harbor Lights", "Glass Season", "Quiet Engine",
                "Long Exposure", "Arcade Rain", "Paper Moons", "Sunday Wire", "Copper Dawn", "Field Notes",
            ]
        );
        let shelf = [BASEMENT, KITCHEN, F3A];
        for lead in leads {
            let peer = lead.peer.endpoint_id.as_deref().unwrap();
            assert!(shelf.contains(&peer), "{peer}");
            assert!(lead.peer.label.is_some() && lead.tile.is_some() && lead.export_id.is_some(), "{lead:?}");
        }
        assert_eq!(leads[5].peer.label.as_deref(), Some("mStream · 3f9a"));
        assert_eq!(leads[10].album, None);
        let tab = |name: &str| -> Vec<String> {
            let answer = taste(name);
            answer.network.unwrap().results.iter().filter_map(|l| l.title.clone()).collect()
        };
        assert_eq!(tab("taste_kitchen"), ["Signal Fires", "Harbor Lights", "Long Exposure", "Sunday Wire", "Field Notes"]);
        assert_eq!(tab("taste_basement"), ["Salt Road", "Breakwater", "Glass Season", "Arcade Rain", "Copper Dawn"]);
        assert_eq!(tab("taste_3f9a"), ["Quiet Engine", "Paper Moons"]);
        let reason = |name: &str| taste(name).network.and_then(|n| n.reason);
        assert_eq!(reason("taste_attic"), Some(NetworkReason::NothingNew));
        assert_eq!(reason("taste_empty_shelf"), Some(NetworkReason::EmptyShelf));
        assert_eq!(reason("taste_not_on_shelf"), Some(NetworkReason::NotOnShelf));
        assert_eq!(reason("taste_scan_pending"), Some(NetworkReason::ScanPending));
        assert_eq!(reason("taste_error"), Some(NetworkReason::Error));
        assert!(taste("taste_scan_pending").not_analyzed);
        assert_eq!(taste("taste_network_null").network, None);
        let random = taste("taste_random");
        assert!(random.fallback && random.seed_source == SeedSource::Random);
        assert_eq!(random.network.unwrap().results.len(), 12);
        for name in TASTES {
            let _ = taste(name);
        }
        // The strips: ids valid, labels as DECISIONS §2 draws them.
        let labels = |name: &str| -> Vec<String> { peers(name).peers.into_iter().map(|p| p.label).collect() };
        let shelf_labels = ["Basement Hi-Fi", "Kitchen Radio", "Attic Tapes", "mStream · 3f9a"];
        let admin = peers("peers_admin");
        assert!(admin.can_add);
        assert_eq!(labels("peers_admin"), [&shelf_labels[..], &["Lighthouse Tapes", "Static Garden", "Rooftop FM"]].concat());
        let member = peers("peers_member");
        assert!(!member.can_add && member.peers.iter().all(|p| p.on_shelf && p.add.is_none()));
        assert_eq!(labels("peers_member"), shelf_labels);
        let empty = peers("peers_admin_empty_shelf");
        assert_eq!((empty.shelf.searchable, empty.peers.iter().any(|p| p.on_shelf)), (0, false));
        let adds = peers("peers_admin_add_states");
        let add = |id: &str| adds.row(id).and_then(|p| p.add.clone()).unwrap();
        assert!(add(LIGHTHOUSE).no_room());
        assert_eq!(add(STATIC).state, PeerAddKind::Adding);
        assert_eq!((add(ROOFTOP).state, add(ROOFTOP).code.as_deref()), (PeerAddKind::Failed, Some("offline")));
        for name in PEERS {
            let answer = peers(name);
            let raw = json(name)["peers"].as_array().map_or(0, Vec::len);
            assert_eq!(answer.peers.len(), raw, "{name}: a row was dropped");
            assert!(answer.peers.iter().all(|p| is_endpoint_id(&p.endpoint_id)));
        }
        for name in PREVIEWS {
            let _: PreviewResolveResponse = serde_json::from_value(preview(name)).unwrap();
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_fixture_ids_are_the_stages() {
        use sha2::Digest;
        for (prefix, name, id) in [
            ("ba5eaa", "Basement Hi-Fi", BASEMENT),
            ("cafea9", "Kitchen Radio", KITCHEN),
            ("3f9a1c", "mStream", F3A),
            ("a771c0", "Attic Tapes", ATTIC),
            ("11fe90", "Lighthouse Tapes", LIGHTHOUSE),
            ("57a76a", "Static Garden", STATIC),
            ("f00f70", "Rooftop FM", ROOFTOP),
        ] {
            let digest = sha2::Sha256::digest(format!("peer-recs-stage:{name}").as_bytes());
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(format!("{prefix}{}", &hex[prefix.len()..]), id, "{name}");
        }
    }
}
