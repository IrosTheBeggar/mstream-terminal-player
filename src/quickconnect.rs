//! Quick Connect: reach an mStream server over its Iroh tunnel instead of a
//! reachable URL.
//!
//! A pairing code is `mstr<V>:<base64url(JSON)>` carrying an endpoint ticket
//! and the 32-byte secret that gates the tunnel. Dialling it gives a QUIC
//! connection where **one bi-stream is one TCP connection** to the server's
//! local HTTP port — so ordinary HTTP, range requests and all, rides over it
//! unchanged.
//!
//! The tunnel itself — the parse, the dial, the loopback bridge, the reconnect
//! supervisor, the in-place credential swap, the loopback token — is the
//! shared `mstream-iroh-tunnel` crate's (`iroh_tunnel`), the same client the
//! mobile app ships. What is the player's own lives here: the identity a
//! tunnel server is remembered by, the words for a tunnel's state, and the
//! `quickconnect-probe` diagnostic, which walks the crate's dial one stage at
//! a time so a hostile network's failure has a name.
//!
//! Two things this is *not*. The secret gates the pipe, not the API: after the
//! tunnel is up the client still logs in normally. And the code itself is
//! fetched over an existing connection by an admin, so the flow is pair on the
//! LAN, then roam.

use iroh_tunnel::{DialError, PairingKind, Stage};

/// Marks a remembered server as one reached through a tunnel rather than at a
/// URL. Deliberately not a real scheme: nothing may hand it to an HTTP client.
pub const TUNNEL_ID_PREFIX: &str = "mstream+iroh://";

/// How the tunnel is reaching the server right now. iroh starts a
/// connection on its relay path and holepunches toward a direct one, so
/// this can change moments after connecting — and change back when a
/// network path dies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelPath {
    /// A holepunched peer-to-peer path carries the traffic.
    Direct,
    /// Traffic bounces through an iroh relay server.
    Relay,
    /// Between tunnels: the last one died and a fresh dial has not won yet.
    Reconnecting,
}

impl TunnelPath {
    /// The word the UI shows for this state.
    pub fn label(self) -> &'static str {
        match self {
            TunnelPath::Direct => "direct",
            TunnelPath::Relay => "relay",
            TunnelPath::Reconnecting => "reconnecting…",
        }
    }

    /// From the shared tunnel client's `path_kind()`: unknown (0) is what it
    /// reports whenever the tunnel is not connected, so that reads as
    /// between tunnels here.
    pub fn from_kind(kind: u8) -> TunnelPath {
        match kind {
            1 => TunnelPath::Direct,
            2 => TunnelPath::Relay,
            _ => TunnelPath::Reconnecting,
        }
    }
}

/// What a tunnel's supervisor is doing, as the shared client reports it —
/// its `STATUS_*` codes, named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelStatus {
    Connecting,
    Connected,
    /// The connection died and the supervisor is re-dialling on the same
    /// loopback port; requests wait for it, bounded.
    Reconnecting,
    /// The server refused the credential and the supervisor gave up: a
    /// rotated pairing code, or an expired guest token. A new credential
    /// re-dials at once.
    Rejected,
    Down,
}

impl TunnelStatus {
    pub fn from_code(code: u8) -> TunnelStatus {
        match code {
            0 => TunnelStatus::Connecting,
            1 => TunnelStatus::Connected,
            2 => TunnelStatus::Reconnecting,
            3 => TunnelStatus::Rejected,
            _ => TunnelStatus::Down,
        }
    }
}

/// The base URL a tunnel serves at, from the loopback port the shared client
/// bound.
pub fn local_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// What a pairing code names: the server's iroh endpoint id, a public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    pub endpoint_id: String,
}

impl PairingCode {
    /// Short form of the endpoint id, for display. Never shows the secret.
    pub fn endpoint_label(&self) -> String {
        self.endpoint_id.chars().take(12).collect()
    }

    /// Stable identity of the server this code reaches.
    ///
    /// The endpoint id is a public key, so it holds still across new loopback
    /// ports, new networks, and a re-issued code for the same server — which
    /// is what makes it the right thing to file a saved session under. The
    /// loopback URL a session happens to use is none of those things.
    pub fn server_id(&self) -> String {
        format!("{TUNNEL_ID_PREFIX}{}", self.endpoint_id)
    }
}

/// Whether a remembered server is reached through a tunnel. Such an identity
/// is not an address: reaching it means dialling its pairing code first.
pub fn is_tunnel_id(server: &str) -> bool {
    server.starts_with(TUNNEL_ID_PREFIX)
}

/// A tunnel identity in a form worth showing someone, since the raw endpoint
/// id is a 52-character public key. Anything else is returned unchanged.
pub fn display_server(server: &str) -> String {
    match server.strip_prefix(TUNNEL_ID_PREFIX) {
        Some(id) => format!("quick connect · {}", id.chars().take(12).collect::<String>()),
        None => server.to_string(),
    }
}

/// Parse a pairing code for the identity it names — the shared crate's
/// parse, which is the server's own shape: a versioned envelope, a bare body
/// as legacy v1, either base64 alphabet. A federation guest ticket is not a
/// pairing code: it reaches a peer, which is dialled by its own row, never
/// pasted here.
pub fn parse_code(raw: &str) -> Result<PairingCode, String> {
    let credential = iroh_tunnel::inspect(raw).map_err(|e| e.to_string())?;
    match credential.kind {
        PairingKind::Tunnel => Ok(PairingCode { endpoint_id: credential.endpoint_id }),
        PairingKind::FederationGuest => Err(
            "this is a federation guest ticket, not a pairing code — a peer is reached through its parent"
                .to_string(),
        ),
    }
}

/// Which proxy variable is set, if any — named but never printed whole,
/// since proxy URLs routinely carry credentials. Mirrors iroh's read order.
fn proxy_env_var() -> Option<&'static str> {
    ["http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY"]
        .into_iter()
        .find(|name| std::env::var(name).is_ok_and(|v| !v.is_empty()))
}

/// The same silent failure points at opposite culprits depending on whether
/// this machine ever reached the relay network itself.
fn unreachable_advice(relay_online: bool) -> &'static str {
    if relay_online {
        "the relay network is reachable from here, so the server may be offline, or its \
         pairing code was issued while the server had no relay contact"
    } else {
        "the iroh relay network was unreachable too — this network may block or intercept \
         it (corporate networks often do); if it requires a proxy, set HTTPS_PROXY and retry"
    }
}

/// Diagnostic: dial a pairing code and make one real HTTP request through the
/// tunnel, proving the whole path rather than just the handshake. Each stage
/// is narrated as it completes, so on a hostile network the output names the
/// stage that died — the difference between an IT ticket about blocked relays
/// and a look at the server.
pub fn probe(code: &str) -> i32 {
    let parsed = match parse_code(code) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    println!("endpoint: {}…", parsed.endpoint_label());
    if let Some(name) = proxy_env_var() {
        println!("proxy: ${name} is set and will be used for relay dials");
    }

    // The shared client's dial, one stage at a time — then one real request
    // over the bridge it built, through the ordinary API client.
    let started = std::time::Instant::now();
    let mut narrate = |stage: Stage| {
        let what = match stage {
            Stage::Bound => "local endpoint up".to_string(),
            Stage::Relay { online: true, url: Some(url) } => format!("relay reached ({url})"),
            Stage::Relay { online: true, url: None } => "relay reported online".to_string(),
            Stage::Relay { online: false, .. } => "NO RELAY — dialling direct anyway; if that fails too, this \
                 network likely blocks or intercepts the iroh relay servers"
                .to_string(),
            Stage::Connected => "server accepted the connection".to_string(),
            Stage::Handshaken => "pairing handshake accepted".to_string(),
            Stage::Serving { local_port } => format!("tunnel up at {}", local_url(local_port)),
        };
        println!("  {what} after {:.2}s", started.elapsed().as_secs_f64());
    };
    let tunnel = match crate::runtime::block_on(iroh_tunnel::connect_tunnel_staged(code, 0, &mut narrate)) {
        Ok(Ok(tunnel)) => tunnel,
        Ok(Err(e)) => {
            eprintln!("FAIL: {e}");
            if let DialError::Unreachable { relay_online, .. } = e {
                eprintln!("      {}", unreachable_advice(relay_online));
            }
            return 1;
        }
        Err(e) => {
            eprintln!("FAIL: {e}");
            return 1;
        }
    };

    let client = match crate::api::Client::new(&tunnel.local_url()) {
        Ok(client) => client.with_local_token(Some(tunnel.local_token())),
        Err(e) => {
            eprintln!("FAIL: {e}");
            return 1;
        }
    };
    match client.ping() {
        Ok(ping) => {
            println!("PASS — public-mode server answered: {} libraries", ping.vpaths.len());
            0
        }
        // The expected answer on a server with users: the pipe works, the API
        // still wants credentials.
        Err(crate::api::ApiError::Unauthorized) => {
            println!("PASS — server answered over the tunnel and asked for a login");
            0
        }
        Err(e) => {
            eprintln!("FAIL: {e}");
            1
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use base64::Engine as _;

    /// Build a code the way the server does, so the parser is tested against
    /// the real shape rather than a guess.
    pub(crate) fn encode(version: Option<u32>, ticket: &str, secret: &[u8]) -> String {
        let payload = serde_json::json!({
            "t": ticket,
            "s": base64::engine::general_purpose::STANDARD.encode(secret),
        });
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string());
        match version {
            Some(v) => format!("mstr{v}:{body}"),
            None => body,
        }
    }

    // A real ticket captured from a running mStream tunnel.
    pub(crate) const TICKET: &str = "endpointabrraywtjw6g3m7gofwzvgif4t7p7b7olzxcske4lei7axhn53gmkbaaenuhi5dqom5c6l3vonstcljrfzzgk3dbpexg4mbonfzg62bonruw42zof4aqasj432dpvxydaeakyhaaah5n6aybadakqakh7lpqg";

    /// A valid v1 pairing code for [`TICKET`] with a fixed secret.
    pub(crate) fn sample_code() -> String {
        encode(Some(1), TICKET, &[9u8; 32])
    }

    /// The identity [`sample_code`] parses to.
    pub(crate) fn sample_id() -> String {
        super::parse_code(&sample_code()).expect("sample code parses").server_id()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{encode, TICKET};
    use super::*;
    use base64::Engine as _;
    use iroh::Endpoint;
    use iroh::endpoint::presets;
    use iroh_tickets::endpoint::EndpointTicket;
    use iroh_tunnel::TUNNEL_ALPN;

    #[test]
    fn parses_a_v1_code() {
        let code = parse_code(&encode(Some(1), TICKET, &[7u8; 32])).unwrap();
        assert!(!code.endpoint_label().is_empty());
        assert!(code.server_id().starts_with(TUNNEL_ID_PREFIX));
        assert!(code.endpoint_id.len() > 40, "an endpoint id is a public key: {}", code.endpoint_id);
    }

    #[test]
    fn the_server_id_comes_from_the_endpoint_not_the_secret() {
        // Two codes for the same server — different secrets, as a rotation
        // produces — must still name the same server, or a rotation would
        // strand the saved session it was meant to keep working.
        let a = parse_code(&encode(Some(1), TICKET, &[1u8; 32])).unwrap();
        let b = parse_code(&encode(Some(1), TICKET, &[2u8; 32])).unwrap();
        assert_eq!(a.server_id(), b.server_id());

        assert!(is_tunnel_id(&a.server_id()));
        assert!(!is_tunnel_id("http://host:3000"));
        // And the identity never carries the secret it was derived alongside.
        assert!(!a.server_id().contains(&base64::engine::general_purpose::STANDARD
            .encode([1u8; 32])));
    }

    #[test]
    fn a_tunnel_identity_is_shown_as_something_readable() {
        let code = parse_code(&encode(Some(1), TICKET, &[7u8; 32])).unwrap();
        let shown = display_server(&code.server_id());
        assert!(shown.starts_with("quick connect · "), "got: {shown}");
        assert!(!shown.contains(TUNNEL_ID_PREFIX), "the scheme is noise to a reader");
        assert!(shown.chars().count() < 32, "short enough for a header: {shown}");
        // Ordinary servers pass through untouched.
        assert_eq!(display_server("https://demo.mstream.io"), "https://demo.mstream.io");
    }

    #[test]
    fn treats_a_bare_body_as_legacy_v1() {
        // The server's own parser accepts an unprefixed body; so must we.
        assert!(parse_code(&encode(None, TICKET, &[1u8; 32])).is_ok());
    }

    #[test]
    fn rejects_a_future_version_with_advice() {
        let err = parse_code(&encode(Some(2), TICKET, &[1u8; 32])).unwrap_err();
        assert!(err.to_lowercase().contains("update"), "got: {err}");
    }

    #[test]
    fn a_guest_ticket_is_not_a_pairing_code() {
        let json = serde_json::json!({ "t": TICKET, "g": "guest-token" }).to_string();
        let ticket = format!("mstrfedg1:{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json));
        let err = parse_code(&ticket).unwrap_err();
        assert!(err.contains("guest ticket"), "got: {err}");
    }

    #[test]
    fn accepts_either_base64_alphabet_padded_or_not() {
        let payload = serde_json::json!({
            "t": TICKET,
            "s": base64::engine::general_purpose::STANDARD.encode([3u8; 32]),
        })
        .to_string();
        for encoded in [
            base64::engine::general_purpose::URL_SAFE.encode(&payload),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload),
            base64::engine::general_purpose::STANDARD.encode(&payload),
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(&payload),
        ] {
            assert!(parse_code(&format!("mstr1:{encoded}")).is_ok(), "failed on {encoded}");
        }
    }

    #[test]
    fn rejects_a_wrong_length_secret() {
        let err = parse_code(&encode(Some(1), TICKET, &[1u8; 16])).unwrap_err();
        assert!(err.contains("32 bytes"), "got: {err}");
    }

    #[test]
    fn rejects_junk() {
        assert!(parse_code("").is_err());
        assert!(parse_code("   ").is_err());
        assert!(parse_code("mstr1:not-base64!!").is_err());
        assert!(parse_code("mstrX:abcd").is_err());
    }

    #[test]
    fn trims_surrounding_whitespace_from_a_pasted_code() {
        let code = format!("  {}\n", encode(Some(1), TICKET, &[9u8; 32]));
        assert!(parse_code(&code).is_ok());
    }

    /// Stand up an endpoint speaking the server half of the tunnel protocol,
    /// as mStream implements it — the first bi-stream carries the secret and
    /// is answered OK, every later one is one TCP connection's worth of bytes
    /// to a local HTTP port that always answers `http_response` — and hand
    /// back its ticket. Relay-free, dialled by direct addresses: tests need
    /// no network beyond this machine.
    fn fake_mstream_endpoint(secret: [u8; 32], http_response: &'static [u8]) -> String {
        let http = std::net::TcpListener::bind("127.0.0.1:0").expect("bind http");
        let http_port = http.local_addr().expect("http addr").port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            while let Ok((mut sock, _)) = http.accept() {
                let mut head = Vec::new();
                let mut byte = [0u8; 256];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut byte) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&byte[..n]),
                    }
                }
                let _ = sock.write_all(http_response);
            }
        });

        let addr = crate::runtime::block_on(async move {
            let endpoint = Endpoint::builder(presets::Minimal)
                .alpns(vec![TUNNEL_ALPN.to_vec()])
                .bind()
                .await
                .expect("bind server endpoint");
            let addr = endpoint.addr();
            tokio::spawn(async move {
                while let Some(incoming) = endpoint.accept().await {
                    let Ok(connection) = incoming.await else { continue };
                    tokio::spawn(async move {
                        let Ok((mut send, mut recv)) = connection.accept_bi().await else {
                            return;
                        };
                        let got = recv.read_to_end(256).await.unwrap_or_default();
                        if got != secret {
                            let _ = send.write_all(b"NO").await;
                            return;
                        }
                        let _ = send.write_all(b"OK").await;
                        let _ = send.finish();
                        while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                            tokio::spawn(async move {
                                let Ok(tcp) =
                                    tokio::net::TcpStream::connect(("127.0.0.1", http_port)).await
                                else {
                                    return;
                                };
                                let (mut tcp_read, mut tcp_write) = tcp.into_split();
                                let up = async {
                                    let _ = tokio::io::copy(&mut recv, &mut tcp_write).await;
                                };
                                let down = async {
                                    let _ = tokio::io::copy(&mut tcp_read, &mut send).await;
                                    let _ = send.finish();
                                };
                                tokio::join!(up, down);
                            });
                        }
                    });
                }
            });
            addr
        })
        .expect("runtime");

        EndpointTicket::from(addr).to_string()
    }

    /// The whole client path against a live endpoint speaking the server's
    /// protocol — parse, dial, handshake, bridge, one HTTP round trip. The
    /// peer is relay-free and dialled by its direct addresses, so a pass
    /// means the client machinery is sound without any network beyond this
    /// machine — which is exactly the half a corporate firewall can't touch.
    #[test]
    fn dials_handshakes_and_bridges_http_end_to_end() {
        const SECRET: [u8; 32] = [42u8; 32];
        let ticket = fake_mstream_endpoint(
            SECRET,
            b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi",
        );
        let code = encode(Some(1), &ticket, &SECRET);
        assert!(parse_code(&code).is_ok(), "the code parses to an identity first");
        let tunnel = crate::runtime::block_on(iroh_tunnel::connect_tunnel(&code, 0))
            .expect("runtime")
            .expect("open tunnel");

        use std::io::{Read, Write};
        let target = format!("127.0.0.1:{}", tunnel.local_port);
        let mut sock = std::net::TcpStream::connect(&target).expect("connect bridge");
        // The loopback token on the request line: without it the shared
        // client drops the connection, so another process on this machine
        // cannot use the bridge as a proxy.
        let request = format!(
            "GET /api/v1/ping?__lt={} HTTP/1.1\r\nhost: tunnel\r\nconnection: close\r\n\r\n",
            tunnel.local_token()
        );
        sock.write_all(request.as_bytes()).expect("send request");
        // Mirror what a real HTTP client does at end of request, and what the
        // bridge needs to forward end-of-stream: half-close the write side.
        sock.shutdown(std::net::Shutdown::Write).expect("half-close");
        let mut reply = String::new();
        let _ = sock.read_to_string(&mut reply);
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "got: {reply}");
        assert!(reply.ends_with("hi"), "got: {reply}");

        // And the gate itself: the same request without the token gets no
        // answer at all.
        let mut bare = std::net::TcpStream::connect(&target).expect("connect bridge");
        bare.write_all(b"GET /api/v1/ping HTTP/1.1\r\nhost: tunnel\r\nconnection: close\r\n\r\n")
            .expect("send request");
        bare.shutdown(std::net::Shutdown::Write).expect("half-close");
        let mut nothing = String::new();
        let _ = bare.read_to_string(&mut nothing);
        assert!(nothing.is_empty(), "the gate let a tokenless request through: {nothing}");
    }

    /// The probe walks the same stages and comes back green against a healthy
    /// endpoint — including the final ping through the real API client, which
    /// needs the canned answer to be a ping-shaped JSON body.
    #[test]
    fn the_probe_passes_against_a_live_endpoint() {
        const SECRET: [u8; 32] = [7u8; 32];
        let ticket = fake_mstream_endpoint(
            SECRET,
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
              content-length: 13\r\nconnection: close\r\n\r\n{\"vpaths\":[]}",
        );
        let code = encode(Some(1), &ticket, &SECRET);
        assert_eq!(probe(&code), 0);
    }

    #[test]
    fn an_unreachable_server_names_the_right_culprit() {
        let reached = unreachable_advice(true);
        assert!(reached.contains("server may be offline"), "got: {reached}");
        let unreached = unreachable_advice(false);
        assert!(unreached.contains("relay network was unreachable"), "got: {unreached}");
        assert!(unreached.contains("HTTPS_PROXY"), "got: {unreached}");
    }
}
