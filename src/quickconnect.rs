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
use iroh::{Endpoint, EndpointAddr};
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
        let endpoint = Endpoint::bind(presets::N0)
            .await
            .map_err(|e| format!("could not start the local endpoint: {e}"))?;

        // Best effort: if the relay isn't ready in time, dial anyway rather
        // than failing outright.
        let _ = tokio::time::timeout(ONLINE_TIMEOUT, endpoint.online()).await;

        let connection =
            tokio::time::timeout(DIAL_TIMEOUT, endpoint.connect(code.addr.clone(), TUNNEL_ALPN))
                .await
                .map_err(|_| "timed out reaching the server through the tunnel".to_string())?
                .map_err(|e| format!("could not reach the server: {e}"))?;

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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("could not open a local port for the tunnel: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("could not read the local tunnel port: {e}"))?
            .port();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(accept_loop(listener, tunnel, shutdown_rx));

        Ok::<_, String>(TunnelBridge {
            local_url: format!("http://127.0.0.1:{port}"),
            _shutdown: shutdown_tx,
        })
    })?
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
                // up on keep-alive idle); only worth a line for diagnosis.
                eprintln!("[quickconnect] stream ended: {e}");
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

/// Diagnostic: dial a pairing code and make one real HTTP request through the
/// tunnel, proving the whole path rather than just the handshake.
pub fn probe(code: &str) -> i32 {
    let parsed = match parse_code(code) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    println!("endpoint: {}…", parsed.endpoint_label());

    // Exercise the same path the player uses: bring the tunnel up on loopback,
    // then talk to it with the ordinary API client.
    let started = std::time::Instant::now();
    let bridge = match open_bridge(&parsed) {
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
}
