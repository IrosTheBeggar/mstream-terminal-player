//! The Peer recs room's one door (peer-recs contract §1.4, D5 of the
//! build plan): every ask the room makes of the server goes through
//! `ApiCmd::PeerRecs { generation, cmd }` and comes back on
//! `Event::PeerRecs { generation, event }`.
//!
//! The vocabulary lives here, beside the client, so the App compiles
//! against it and one shaper, [`peer_recs`], serves both the native worker
//! and the browser's. The worker only routes.
//!
//! The rule the shaper keeps: a room failure stays in the room's event. It
//! returns `Err` only for a 401, which is the session's problem and goes
//! out as `Event::Unauthorized`; anything else becoming `Event::Error`
//! would clear Discover's waits.

use serde_json::Value;

use super::types::{
    Capabilities, CollectionDestination, CollectionDestinationView, DiscoveryPluginsResponse, JobLookup,
    JobScope, JobStartAnswer, JobStarted, NetworkTasteRequest, P2pPeersAnswer, PeerAddAnswer, PluginJob,
    PluginPreview, TasteResponse,
};
use super::{ApiError, Client, PluginFailure};

/// One of the room's asks. Each names what its answer is fenced by: a
/// `seq` or `ticket` the App compares with its newest, or the poll count.
#[derive(Debug, Clone, PartialEq)]
pub enum PeerRecsCmd {
    /// The network taste ask (peer-recommendations plan §3.5 R1–R3).
    Taste { seq: u64, body: Box<NetworkTasteRequest> },
    /// The strip's poll (R6), numbered so a late answer can be dropped.
    Peers { poll: u64 },
    /// An admin's `+` or "try again" (R4).
    Add { endpoint_id: String },
    /// The plug-in listing, and the destination when Get it could use it.
    Gate { seq: u64 },
    /// A preview resolve for one lead, sent as it came (plan §5.5).
    Preview { ticket: u64, plugin: String, rec: Box<Value> },
    /// The lead sheet's one `lookup/batch` row.
    Lookup { seq: u64, rec: Box<Value> },
    /// One Get it job as it stands, polled while the sheet is open.
    Job { seq: u64, id: i64 },
    /// Get it: a song job on an acquire plug-in, at the place shown.
    Start { seq: u64, plugin: String, rec: Box<Value>, destination: Option<CollectionDestination> },
    /// R7's re-ping: is the room still on?
    Probe,
}

/// A command's answer, failures included.
#[derive(Debug, Clone, PartialEq)]
pub enum PeerRecsEvent {
    Taste { seq: u64, answer: Result<Box<TasteResponse>, RecsFailure> },
    Peers { poll: u64, answer: Result<Box<P2pPeersAnswer>, PeersFailure> },
    /// Every outcome is an answer: a transport failure is status 0.
    Added { endpoint_id: String, answer: PeerAddAnswer },
    Gate {
        seq: u64,
        plugins: Result<Box<DiscoveryPluginsResponse>, PluginFailure>,
        /// Read only when jobs are allowed and a Get it plug-in is listed.
        destination: Option<Result<Box<CollectionDestinationView>, PluginFailure>>,
    },
    /// `Ok(None)` is `preview: null`: no clip for this lead.
    Preview { ticket: u64, answer: Result<Option<Box<PluginPreview>>, PluginFailure> },
    Lookup { seq: u64, answer: Result<Box<JobLookup>, PluginFailure> },
    Job { seq: u64, answer: Result<Option<Box<PluginJob>>, PluginFailure> },
    /// A 409 `destination_changed` is `Err(PluginFailure::DestinationChanged)`.
    Started { seq: u64, answer: Result<Box<JobStarted>, PluginFailure> },
    /// The re-ping's word. A ping that fails is off (the web's and the
    /// phone's rule: a re-ping with no answer is off).
    Probed { on: bool },
}

/// Why a peers poll has no strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeersFailure {
    /// 403: p2p off, or no account (R7's re-ping follows).
    Forbidden,
    /// 404: a server without the route, so All stands alone.
    NoRoute,
    /// Anything else: the last strip stays.
    Failed,
}

/// Why a taste ask has no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecsFailure {
    /// 403: the index is off or its scan is pending (R7's re-ping follows).
    Forbidden,
    Failed,
}

/// A plug-in call's result, a 401 apart.
fn plugin<T>(result: Result<T, ApiError>) -> Result<Result<T, PluginFailure>, ApiError> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(ApiError::Unauthorized) => Err(ApiError::Unauthorized),
        Err(e) => Ok(Err(PluginFailure::of(&e))),
    }
}

/// Run one of the room's asks and shape its answer.
pub(crate) async fn peer_recs(c: &Client, cmd: PeerRecsCmd) -> Result<PeerRecsEvent, ApiError> {
    Ok(match cmd {
        PeerRecsCmd::Taste { seq, body } => {
            let answer = match c.taste_network_async(&body).await {
                Ok(response) => Ok(Box::new(response)),
                Err(ApiError::Unauthorized) => return Err(ApiError::Unauthorized),
                Err(ApiError::Forbidden(_)) => Err(RecsFailure::Forbidden),
                Err(_) => Err(RecsFailure::Failed),
            };
            PeerRecsEvent::Taste { seq, answer }
        }
        PeerRecsCmd::Peers { poll } => {
            let answer = match c.p2p_peers_async().await {
                Ok(view) => Ok(Box::new(view)),
                Err(ApiError::Unauthorized) => return Err(ApiError::Unauthorized),
                Err(ApiError::Forbidden(_)) => Err(PeersFailure::Forbidden),
                Err(ApiError::NotFound(_)) => Err(PeersFailure::NoRoute),
                Err(_) => Err(PeersFailure::Failed),
            };
            PeerRecsEvent::Peers { poll, answer }
        }
        PeerRecsCmd::Add { endpoint_id } => {
            let answer = match c.p2p_peer_add_async(&endpoint_id).await {
                Ok(answer) => answer,
                Err(ApiError::Unauthorized) => return Err(ApiError::Unauthorized),
                // A client that could not even build the ask: no answer.
                Err(_) => PeerAddAnswer { status: 0, state: None },
            };
            PeerRecsEvent::Added { endpoint_id, answer }
        }
        PeerRecsCmd::Gate { seq } => {
            let plugins = plugin(c.discovery_plugins_async().await)?;
            let destination = match &plugins {
                Ok(listing) if listing.jobs.allowed && !listing.get_it_plugins().is_empty() => {
                    Some(plugin(c.collection_destination_async().await)?.map(Box::new))
                }
                _ => None,
            };
            PeerRecsEvent::Gate { seq, plugins: plugins.map(Box::new), destination }
        }
        PeerRecsCmd::Preview { ticket, plugin: name, rec } => {
            let answer = plugin(c.plugin_preview_async(&name, &rec).await)?;
            PeerRecsEvent::Preview { ticket, answer: answer.map(|p| p.map(Box::new)) }
        }
        PeerRecsCmd::Lookup { seq, rec } => {
            let answer = plugin(c.plugin_jobs_lookup_batch_async(std::slice::from_ref(&*rec)).await)?;
            let answer = answer.map(|rows| Box::new(rows.into_iter().next().unwrap_or_default()));
            PeerRecsEvent::Lookup { seq, answer }
        }
        PeerRecsCmd::Job { seq, id } => {
            let answer = plugin(c.plugin_job_async(id).await)?;
            PeerRecsEvent::Job { seq, answer: answer.map(|j| j.map(Box::new)) }
        }
        PeerRecsCmd::Start { seq, plugin: name, rec, destination } => {
            let started = c
                .plugin_job_start_async(&name, &rec, Some(JobScope::Song), Some(destination.as_ref()), None)
                .await;
            let answer = plugin(started)?.and_then(|started| match started {
                JobStartAnswer::Started(started) => Ok(started),
                JobStartAnswer::DestinationChanged(now) => Err(PluginFailure::DestinationChanged(now)),
            });
            PeerRecsEvent::Started { seq, answer }
        }
        PeerRecsCmd::Probe => match c.ping_async().await {
            Ok(ping) => PeerRecsEvent::Probed { on: Capabilities::from(&ping).peer_recs() },
            Err(ApiError::Unauthorized) => return Err(ApiError::Unauthorized),
            Err(_) => PeerRecsEvent::Probed { on: false },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::tests::{canned_server_bodies, json_reply};
    use crate::api::types::fixtures;
    use crate::api::wait;

    fn client(port: u16) -> Client {
        Client::new(&format!("http://127.0.0.1:{port}")).unwrap().with_token(Some("tok".into()))
    }

    fn run(replies: Vec<Vec<u8>>, cmd: PeerRecsCmd) -> (Result<PeerRecsEvent, ApiError>, Vec<String>) {
        let (port, seen) = canned_server_bodies(replies);
        let event = wait(peer_recs(&client(port), cmd));
        (event, seen.try_iter().map(|(line, _)| line).collect())
    }

    #[test]
    fn the_door_keeps_every_failure_in_its_event() {
        let peers = |status: &str| {
            let (event, _) = run(vec![json_reply(status, serde_json::json!({ "error": "no" }))], PeerRecsCmd::Peers { poll: 4 });
            match event.unwrap() {
                PeerRecsEvent::Peers { poll: 4, answer } => answer.err(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(peers("403 Forbidden"), Some(PeersFailure::Forbidden));
        assert_eq!(peers("404 Not Found"), Some(PeersFailure::NoRoute));
        assert_eq!(peers("500 Internal Server Error"), Some(PeersFailure::Failed));
        let ok = run(vec![json_reply("200 OK", fixtures::json("peers_admin"))], PeerRecsCmd::Peers { poll: 5 }).0.unwrap();
        assert_eq!(ok, PeerRecsEvent::Peers { poll: 5, answer: Ok(Box::new(fixtures::peers("peers_admin"))) });

        let body = Box::new(NetworkTasteRequest::new(30, Vec::new(), Vec::new(), 12, None));
        let taste = |status: &str| {
            let cmd = PeerRecsCmd::Taste { seq: 9, body: body.clone() };
            run(vec![json_reply(status, serde_json::json!({ "error": "no" }))], cmd).0
        };
        assert_eq!(taste("403 Forbidden").unwrap(), PeerRecsEvent::Taste { seq: 9, answer: Err(RecsFailure::Forbidden) });
        assert_eq!(taste("502 Bad Gateway").unwrap(), PeerRecsEvent::Taste { seq: 9, answer: Err(RecsFailure::Failed) });
        // A 401 is the session's, whichever ask meets it.
        assert!(matches!(taste("401 Unauthorized"), Err(ApiError::Unauthorized)));
        let unauthorized = json_reply("401 Unauthorized", serde_json::json!({}));
        assert!(matches!(run(vec![unauthorized.clone()], PeerRecsCmd::Peers { poll: 1 }).0, Err(ApiError::Unauthorized)));
        let add = PeerRecsCmd::Add { endpoint_id: fixtures::ROOFTOP.into() };
        assert!(matches!(run(vec![unauthorized.clone()], add).0, Err(ApiError::Unauthorized)));
        assert!(matches!(run(vec![unauthorized], PeerRecsCmd::Probe).0, Err(ApiError::Unauthorized)));

        // The re-ping: a generation-3 ping is on, one short of a flag is
        // off, and one that fails is off.
        let on = serde_json::json!({
            "discovery": true, "discoveryP2p": true, "discoveryTaste": 3, "discoveryP2pPeers": 1
        });
        let probed = |reply| run(vec![reply], PeerRecsCmd::Probe).0.unwrap();
        assert_eq!(probed(json_reply("200 OK", on.clone())), PeerRecsEvent::Probed { on: true });
        let mut off = on.clone();
        off["discoveryP2p"] = serde_json::json!(false);
        assert_eq!(probed(json_reply("200 OK", off)), PeerRecsEvent::Probed { on: false });
        assert_eq!(probed(json_reply("500 Internal Server Error", serde_json::json!({}))), PeerRecsEvent::Probed { on: false });
        assert_eq!(probed(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nnot".to_vec()), PeerRecsEvent::Probed { on: false });

        // A preview's 429 stays a refusal with its status, for the wait.
        let rec = Box::new(fixtures::json("taste_all")["network"]["results"][5].clone());
        let cmd = PeerRecsCmd::Preview { ticket: 3, plugin: "deezer".into(), rec };
        let (event, lines) = run(vec![json_reply("429 Too Many Requests", serde_json::json!({ "error": "slow down" }))], cmd);
        assert!(lines[0].starts_with("POST /api/v1/discovery/plugins/deezer/resolve "), "{lines:?}");
        assert!(matches!(event.unwrap(), PeerRecsEvent::Preview { ticket: 3, answer: Err(PluginFailure::Refused { status: 429, .. }) }));
        // A start's changed destination comes back as that failure.
        let cmd = PeerRecsCmd::Start {
            seq: 2,
            plugin: "noop-acquire".into(),
            rec: Box::new(serde_json::json!({ "title": "Long Exposure" })),
            destination: None,
        };
        let changed = json_reply("409 Conflict", serde_json::json!({ "error": "destination_changed", "destination": null }));
        assert_eq!(
            run(vec![changed], cmd).0.unwrap(),
            PeerRecsEvent::Started { seq: 2, answer: Err(PluginFailure::DestinationChanged(Some(None))) }
        );
    }

    fn listing(plugins: serde_json::Value, allowed: bool) -> Vec<u8> {
        json_reply("200 OK", serde_json::json!({ "plugins": plugins, "jobs": { "allowed": allowed } }))
    }

    #[test]
    fn the_gate_reads_the_destination_only_for_get_it() {
        let deezer = serde_json::json!({ "name": "deezer", "capabilities": ["preview"] });
        let copy = serde_json::json!({ "name": "federation-copy", "capabilities": ["acquire"] });
        let noop = serde_json::json!({ "name": "noop-acquire", "capabilities": ["acquire"] });
        let view = json_reply("200 OK", serde_json::json!({ "destination": null, "libraries": [] }));
        // A Get it plug-in and jobs allowed: the listing, then the place.
        let (event, lines) =
            run(vec![listing(serde_json::json!([deezer, copy, noop]), true), view.clone()], PeerRecsCmd::Gate { seq: 6 });
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with("GET /api/v1/discovery/plugins "), "{lines:?}");
        assert!(lines[1].starts_with("GET /api/v1/discovery/collection/destination "), "{lines:?}");
        match event.unwrap() {
            PeerRecsEvent::Gate { seq: 6, plugins: Ok(plugins), destination: Some(Ok(_)) } => {
                assert_eq!(plugins.preview_plugin().map(|p| p.name.as_str()), Some("deezer"));
            }
            other => panic!("{other:?}"),
        }
        // The copy plug-in alone, or jobs refused: no destination read.
        for (plugins, allowed) in [(serde_json::json!([deezer, copy]), true), (serde_json::json!([noop]), false)] {
            let (event, lines) = run(vec![listing(plugins, allowed), view.clone()], PeerRecsCmd::Gate { seq: 7 });
            assert_eq!(lines.len(), 1, "{lines:?}");
            assert!(matches!(event.unwrap(), PeerRecsEvent::Gate { seq: 7, plugins: Ok(_), destination: None }));
        }
        // A listing that fails is the gate's failure, never an error.
        let (event, _) = run(vec![json_reply("500 Internal Server Error", serde_json::json!({}))], PeerRecsCmd::Gate { seq: 8 });
        assert!(matches!(event.unwrap(), PeerRecsEvent::Gate { seq: 8, plugins: Err(_), destination: None }));
    }

    #[test]
    fn the_sheet_reads_its_lookup_row_and_its_job() {
        let rec = Box::new(serde_json::json!({ "title": "Long Exposure", "artist": "Marlowe Fields" }));
        let lookup = json_reply("200 OK", serde_json::json!({ "results": [{ "key": "text:1", "keys": {}, "jobs": [] }] }));
        let (event, lines) = run(vec![lookup], PeerRecsCmd::Lookup { seq: 1, rec });
        assert!(lines[0].starts_with("POST /api/v1/discovery/plugin-jobs/lookup/batch "), "{lines:?}");
        assert!(matches!(event.unwrap(), PeerRecsEvent::Lookup { seq: 1, answer: Ok(_) }));
        let job = json_reply("200 OK", serde_json::json!({ "job": { "id": 7, "plugin": "noop-acquire", "state": "running" } }));
        match run(vec![job], PeerRecsCmd::Job { seq: 2, id: 7 }).0.unwrap() {
            PeerRecsEvent::Job { seq: 2, answer: Ok(Some(job)) } => assert_eq!(job.id, 7),
            other => panic!("{other:?}"),
        }
        let gone = json_reply("404 Not Found", serde_json::json!({ "error": "no such job" }));
        assert!(matches!(
            run(vec![gone], PeerRecsCmd::Job { seq: 3, id: 7 }).0.unwrap(),
            PeerRecsEvent::Job { seq: 3, answer: Err(PluginFailure::Refused { status: 404, .. }) }
        ));
    }
}
