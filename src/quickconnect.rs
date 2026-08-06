//! Quick Connect: reach an mStream server over its Iroh tunnel instead of a
//! reachable URL.
//!
//! A pairing code is `mstr<V>:<base64url(JSON)>` carrying an endpoint ticket
//! and the 32-byte secret that gates the tunnel. Dialling it gives a QUIC
//! connection where **one bi-stream is one TCP connection** to the server's
//! local HTTP port — so ordinary HTTP, range requests and all, rides over it
//! unchanged.
//!
//! Two things this is *not*. The secret gates the pipe, not the API: after the
//! tunnel is up the client still logs in normally. And the code itself is
//! fetched over an existing connection by an admin, so the flow is pair on the
//! LAN, then roam.

use std::str::FromStr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, RelayUrl, TransportAddr};
use iroh_relay::tls::CaTlsConfig;
use iroh_tickets::endpoint::EndpointTicket;
use serde::Deserialize;

/// Wire protocol version. Note this is a *separate* axis from the `mstr<V>`
/// code version: a v1 code dials ALPN v2 today.
pub const TUNNEL_ALPN: &[u8] = b"mstream/tunnel/2";

const PAIRING_PREFIX: &str = "mstr";
/// Highest pairing-code version we understand.
const MAX_PAIRING_VERSION: u32 = 1;
const SECRET_LEN: usize = 32;

/// The server bounds its handshake read; match it so a hostile peer can't make
/// us buffer.
const HANDSHAKE_LIMIT: usize = 256;
/// Waiting for our own home relay before dialling. Cross-network, the first
/// stream can reset on a path that isn't ready yet.
const ONLINE_TIMEOUT: Duration = Duration::from_secs(8);
const DIAL_TIMEOUT: Duration = Duration::from_secs(25);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Marks a remembered server as one reached through a tunnel rather than at a
/// URL. Deliberately not a real scheme: nothing may hand it to an HTTP client.
pub const TUNNEL_ID_PREFIX: &str = "mstream+iroh://";

#[derive(Debug, Clone)]
pub struct PairingCode {
    pub addr: EndpointAddr,
    secret: Vec<u8>,
}

impl PairingCode {
    /// Short form of the endpoint id, for display. Never shows the secret.
    pub fn endpoint_label(&self) -> String {
        let id = self.addr.id.to_string();
        id.chars().take(12).collect()
    }

    /// Stable identity of the server this code reaches.
    ///
    /// The endpoint id is a public key, so it holds still across new loopback
    /// ports, new networks, and a re-issued code for the same server — which
    /// is what makes it the right thing to file a saved session under. The
    /// loopback URL a session happens to use is none of those things.
    pub fn server_id(&self) -> String {
        format!("{TUNNEL_ID_PREFIX}{}", self.addr.id)
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

#[derive(Deserialize)]
struct PairingPayload {
    t: String,
    s: String,
}

/// Decode base64 in any of the four shapes the server's own parser accepts:
/// url-safe or standard alphabet, padded or not.
fn decode_b64(raw: &str) -> Result<Vec<u8>, String> {
    let normalised: String = raw
        .trim()
        .chars()
        .filter(|c| *c != '=')
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    STANDARD_NO_PAD.decode(normalised).map_err(|e| format!("not valid base64: {e}"))
}

/// Parse a pairing code.
///
/// A body with no `mstr<V>:` prefix is a legacy code and treated as v1, which
/// is what the server's parser does.
pub fn parse_code(raw: &str) -> Result<PairingCode, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("no pairing code given".to_string());
    }

    let body = match raw.split_once(':') {
        Some((head, body)) if head.starts_with(PAIRING_PREFIX) => {
            let version: u32 = head[PAIRING_PREFIX.len()..]
                .parse()
                .map_err(|_| format!("unrecognised pairing code prefix '{head}'"))?;
            if version > MAX_PAIRING_VERSION {
                return Err(format!(
                    "this code is version {version}; this player understands up to \
                     {MAX_PAIRING_VERSION} — update the player"
                ));
            }
            body
        }
        // No recognised prefix: legacy v1 body.
        _ => raw,
    };

    let payload: PairingPayload = serde_json::from_slice(&decode_b64(body)?)
        .map_err(|e| format!("pairing code payload is not valid JSON: {e}"))?;

    let ticket = EndpointTicket::from_str(payload.t.trim())
        .map_err(|e| format!("pairing code contains an invalid endpoint ticket: {e}"))?;
    let secret = decode_b64(&payload.s)?;
    if secret.len() != SECRET_LEN {
        return Err(format!(
            "pairing secret should be {SECRET_LEN} bytes, got {}",
            secret.len()
        ));
    }

    Ok(PairingCode { addr: ticket.into(), secret })
}

/// A live tunnel. Each [`Tunnel::open_stream`] is one TCP connection's worth of
/// traffic to the server's HTTP port.
pub struct Tunnel {
    connection: Connection,
    // Held because dropping the endpoint tears down the connection.
    _endpoint: Endpoint,
}

impl Tunnel {
    /// Dial the server and complete the secret handshake.
    pub async fn open(code: &PairingCode) -> Result<Self, String> {
        let endpoint = bind_endpoint().await?;
        let relay_online = wait_for_relay(&endpoint).await;
        let connection = dial(&endpoint, &code.addr, relay_online).await?;
        handshake(&connection, &code.secret).await?;
        Ok(Tunnel { connection, _endpoint: endpoint })
    }

    /// Open one tunnelled TCP connection to the server's HTTP port.
    pub async fn open_stream(
        &self,
    ) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream), String> {
        self.connection
            .open_bi()
            .await
            .map_err(|e| format!("tunnel stream failed: {e}"))
    }
}

/// Bind the local endpoint the way every tunnel user must: n0 defaults, plus
/// the two settings the defaults leave off that decide whether a corporate
/// network works at all. The OS trust store, because TLS inspection (Netskope,
/// Zscaler) re-signs the relay connection with a CA only the system keychain
/// knows — iroh's embedded Mozilla roots would reject it. And the
/// environment's proxy, because a proxy-only network drops direct dials.
async fn bind_endpoint() -> Result<Endpoint, String> {
    Endpoint::builder(presets::N0)
        .ca_tls_config(CaTlsConfig::system())
        .proxy_from_env()
        .bind()
        .await
        .map_err(|e| format!("could not start the local endpoint: {e}"))
}

/// Best effort — if the relay isn't ready in time, dial anyway rather than
/// failing outright. The outcome is returned rather than discarded because a
/// dial that then times out means opposite things on a machine that reached
/// the relay network and one that never could.
async fn wait_for_relay(endpoint: &Endpoint) -> bool {
    tokio::time::timeout(ONLINE_TIMEOUT, endpoint.online()).await.is_ok()
}

/// Reach the server by whatever path iroh finds — direct, or relayed for a
/// network that blocks UDP.
async fn dial(
    endpoint: &Endpoint,
    addr: &EndpointAddr,
    relay_online: bool,
) -> Result<Connection, String> {
    tokio::time::timeout(DIAL_TIMEOUT, endpoint.connect(addr.clone(), TUNNEL_ALPN))
        .await
        .map_err(|_| dial_timeout_message(relay_online))?
        .map_err(|e| format!("could not reach the server: {e}"))
}

/// The same silent 25 seconds point at opposite culprits depending on whether
/// this machine ever reached the relay network itself.
fn dial_timeout_message(relay_online: bool) -> String {
    if relay_online {
        "timed out reaching the server through the tunnel — the relay network \
         is reachable from here, so the server may be offline, or its pairing \
         code was issued while the server had no relay contact"
            .to_string()
    } else {
        "timed out reaching the server, and the iroh relay network was \
         unreachable too — this network may block or intercept it (corporate \
         networks often do); if it requires a proxy, set HTTPS_PROXY and retry"
            .to_string()
    }
}

/// The first bi-stream is the gate: write the secret, close our side, read the
/// verdict.
async fn handshake(connection: &Connection, secret: &[u8]) -> Result<(), String> {
    let attempt = async {
        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|e| format!("could not open the handshake stream: {e}"))?;
        send.write_all(secret).await.map_err(|e| format!("handshake write failed: {e}"))?;
        // The server reads to end, so it only proceeds once we finish.
        send.finish().map_err(|e| format!("handshake finish failed: {e}"))?;

        // A rejection can arrive as "NO", as an empty read, or as a transport
        // error when the server drops the connection — all three mean the same
        // thing, so treat any read failure as a rejection rather than an
        // infrastructure problem.
        Ok::<_, String>(recv.read_to_end(HANDSHAKE_LIMIT).await.ok())
    };

    let reply = tokio::time::timeout(HANDSHAKE_TIMEOUT, attempt)
        .await
        .map_err(|_| "the server never answered the pairing handshake".to_string())??;

    match reply.as_deref() {
        Some(b"OK") => Ok(()),
        _ => Err("the server rejected this pairing code — it may have been rotated".to_string()),
    }
}

/// A live tunnel exposed as a local HTTP endpoint.
///
/// This is the trick that keeps Quick Connect cheap: a loopback listener turns
/// each inbound TCP connection into one tunnel bi-stream, so the ordinary
/// `api::Client` — and the playback engine's range requests — can point at
/// `local_url` and work exactly as they do against a direct server.
pub struct TunnelBridge {
    pub local_url: String,
    /// Dropping this stops the accept loop and closes the tunnel.
    _shutdown: tokio::sync::oneshot::Sender<()>,
}

/// Dial a pairing code and publish it on loopback. Blocking; call from a worker
/// thread.
pub fn open_bridge(code: &PairingCode) -> Result<TunnelBridge, String> {
    let code = code.clone();
    crate::runtime::block_on(async move {
        let tunnel = std::sync::Arc::new(Tunnel::open(&code).await?);
        publish_on_loopback(tunnel).await
    })?
}

/// Put an open tunnel behind a loopback listener, one inbound TCP connection
/// per bi-stream.
async fn publish_on_loopback(tunnel: std::sync::Arc<Tunnel>) -> Result<TunnelBridge, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("could not open a local port for the tunnel: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("could not read the local tunnel port: {e}"))?
        .port();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(accept_loop(listener, tunnel, shutdown_rx));

    Ok(TunnelBridge {
        local_url: format!("http://127.0.0.1:{port}"),
        _shutdown: shutdown_tx,
    })
}

async fn accept_loop(
    listener: tokio::net::TcpListener,
    tunnel: std::sync::Arc<Tunnel>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    loop {
        let socket = tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((socket, _)) => socket,
                Err(_) => break,
            },
        };
        let tunnel = tunnel.clone();
        tokio::spawn(async move {
            if let Err(e) = bridge_one(socket, tunnel).await {
                // Individual connections failing is normal (the client hangs
                // up on keep-alive idle); only worth a line for diagnosis —
                // and not worth smearing across a session that is drawing,
                // where "normal" made it a repeat offender (audit #44).
                crate::stderrln!("[quickconnect] stream ended: {e}");
            }
        });
    }
}

/// Pump one TCP connection through one tunnel bi-stream, both directions.
async fn bridge_one(
    socket: tokio::net::TcpStream,
    tunnel: std::sync::Arc<Tunnel>,
) -> Result<(), String> {
    let (mut send, mut recv) = tunnel.open_stream().await?;
    let (mut client_read, mut client_write) = socket.into_split();

    let upstream = async {
        tokio::io::copy(&mut client_read, &mut send).await?;
        // Signal end-of-request so the server stops waiting for more.
        let _ = send.finish();
        Ok::<_, std::io::Error>(())
    };
    let downstream = async {
        tokio::io::copy(&mut recv, &mut client_write).await?;
        Ok::<_, std::io::Error>(())
    };

    tokio::try_join!(upstream, downstream)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The proxy variable iroh will read, if one is set — named but never printed,
/// since proxy URLs routinely carry credentials. Mirrors iroh's read order.
fn proxy_env_var() -> Option<&'static str> {
    ["http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY"]
        .into_iter()
        .find(|name| std::env::var(name).is_ok_and(|v| !v.is_empty()))
}

/// The relay the endpoint settled on, if any. The probe names it so a
/// corporate allowlist request can name it too.
fn home_relay(endpoint: &Endpoint) -> Option<RelayUrl> {
    endpoint.addr().addrs.into_iter().find_map(|addr| match addr {
        TransportAddr::Relay(url) => Some(url),
        _ => None,
    })
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

    // The same steps `Tunnel::open` composes, exercised the same way the
    // player uses them — then the loopback bridge and the ordinary API client.
    let started = std::time::Instant::now();
    let stage = move |what: &str| {
        println!("  {what} after {:.2}s", started.elapsed().as_secs_f64());
    };
    let opened = crate::runtime::block_on(async move {
        let endpoint = bind_endpoint().await?;
        stage("local endpoint up");
        let relay_online = wait_for_relay(&endpoint).await;
        match (relay_online, home_relay(&endpoint)) {
            (true, Some(url)) => stage(&format!("relay reached ({url})")),
            (true, None) => stage("relay reported online"),
            (false, _) => stage(
                "NO RELAY — dialling direct anyway; if that fails too, this \
                 network likely blocks or intercepts the iroh relay servers",
            ),
        }
        let connection = dial(&endpoint, &parsed.addr, relay_online).await?;
        stage("server accepted the connection");
        handshake(&connection, &parsed.secret).await?;
        stage("pairing handshake accepted");
        publish_on_loopback(std::sync::Arc::new(Tunnel { connection, _endpoint: endpoint })).await
    })
    .and_then(|opened| opened);

    let bridge = match opened {
        Ok(bridge) => bridge,
        Err(e) => {
            eprintln!("FAIL: {e}");
            return 1;
        }
    };
    println!(
        "tunnel up at {} after {:.2}s",
        bridge.local_url,
        started.elapsed().as_secs_f64()
    );

    let client = match crate::api::Client::new(&bridge.local_url) {
        Ok(client) => client,
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
mod tests {
    use super::*;

    /// Build a code the way the server does, so the parser is tested against
    /// the real shape rather than a guess.
    fn encode(version: Option<u32>, ticket: &str, secret: &[u8]) -> String {
        let payload = serde_json::json!({
            "t": ticket,
            "s": base64::engine::general_purpose::STANDARD.encode(secret),
        });
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string());
        match version {
            Some(v) => format!("{PAIRING_PREFIX}{v}:{body}"),
            None => body,
        }
    }

    // A real ticket captured from a running mStream tunnel.
    const TICKET: &str = "endpointabrraywtjw6g3m7gofwzvgif4t7p7b7olzxcske4lei7axhn53gmkbaaenuhi5dqom5c6l3vonstcljrfzzgk3dbpexg4mbonfzg62bonruw42zof4aqasj432dpvxydaeakyhaaah5n6aybadakqakh7lpqg";

    #[test]
    fn parses_a_v1_code() {
        let code = parse_code(&encode(Some(1), TICKET, &[7u8; 32])).unwrap();
        assert_eq!(code.secret, vec![7u8; 32]);
        assert!(!code.endpoint_label().is_empty());
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
        let code = parse_code(&encode(None, TICKET, &[1u8; 32])).unwrap();
        assert_eq!(code.secret.len(), SECRET_LEN);
    }

    #[test]
    fn rejects_a_future_version_with_advice() {
        let err = parse_code(&encode(Some(2), TICKET, &[1u8; 32])).unwrap_err();
        assert!(err.contains("update the player"), "got: {err}");
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
        let bridge = open_bridge(&parse_code(&code).expect("parse")).expect("open bridge");

        use std::io::{Read, Write};
        let target = bridge.local_url.strip_prefix("http://").expect("local url");
        let mut sock = std::net::TcpStream::connect(target).expect("connect bridge");
        sock.write_all(b"GET /api/v1/ping HTTP/1.1\r\nhost: tunnel\r\nconnection: close\r\n\r\n")
            .expect("send request");
        // Mirror what a real HTTP client does at end of request, and what the
        // bridge needs to forward end-of-stream: half-close the write side.
        sock.shutdown(std::net::Shutdown::Write).expect("half-close");
        let mut reply = String::new();
        let _ = sock.read_to_string(&mut reply);
        assert!(reply.starts_with("HTTP/1.1 200 OK"), "got: {reply}");
        assert!(reply.ends_with("hi"), "got: {reply}");
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
    fn a_dial_timeout_names_the_right_culprit() {
        let reached = dial_timeout_message(true);
        assert!(reached.contains("server may be offline"), "got: {reached}");
        let unreached = dial_timeout_message(false);
        assert!(unreached.contains("relay network was unreachable"), "got: {unreached}");
        assert!(unreached.contains("HTTPS_PROXY"), "got: {unreached}");
    }
}
