//! Finding mStream servers on the local network.
//!
//! mStream advertises `_mstream._tcp` over mDNS by default, and its TXT
//! records carry everything needed to build a working base URL — scheme, port
//! and even the path prefix, so a server behind a reverse proxy resolves
//! correctly. It also advertises `iroh=1` when the Quick Connect tunnel is
//! available, which is how we can offer pairing only where it would work.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use mdns_sd::{ScopedIp, ServiceDaemon, ServiceEvent};

const SERVICE_TYPE: &str = "_mstream._tcp.local.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredServer {
    /// Friendly name from the advert, falling back to the instance label.
    pub name: String,
    pub base_url: String,
    pub version: Option<String>,
    /// The server advertises an Iroh tunnel, so a pairing code can be used.
    pub quick_connect: bool,
}

/// Browse for servers, collecting whatever answers within `window`.
///
/// Blocking: mDNS has no "that's everyone" signal, so this simply listens for
/// a fixed period. Call it from a worker thread.
pub fn browse(window: Duration) -> Result<Vec<DiscoveredServer>, String> {
    let daemon = ServiceDaemon::new().map_err(|e| format!("could not start mDNS: {e}"))?;
    let receiver = daemon
        .browse(SERVICE_TYPE)
        .map_err(|e| format!("could not browse for servers: {e}"))?;

    let deadline = Instant::now() + window;
    let mut found: Vec<DiscoveredServer> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                // The same instance can resolve more than once (one event per
                // interface); keep the first.
                if !seen.insert(info.fullname.clone()) {
                    continue;
                }
                if let Some(server) = to_server(&info) {
                    found.push(server);
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let _ = daemon.shutdown();
    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(found)
}

fn to_server(info: &mdns_sd::ResolvedService) -> Option<DiscoveredServer> {
    let address = pick_address(&info.addresses)?;
    let txt = |key: &str| info.get_property_val_str(key).map(str::to_string);

    let scheme = txt("scheme").unwrap_or_else(|| "http".to_string());
    // The advert's own port is authoritative when present; the SRV port is the
    // fallback.
    let port = txt("port").and_then(|p| p.parse::<u16>().ok()).unwrap_or(info.port);
    let path = txt("path").unwrap_or_default();
    let path = path.trim_matches('/');

    let mut base_url = format!("{scheme}://{address}:{port}");
    if !path.is_empty() {
        base_url.push('/');
        base_url.push_str(path);
    }

    let name = txt("name")
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| instance_label(&info.fullname));

    Some(DiscoveredServer {
        name,
        base_url,
        version: txt("v"),
        quick_connect: txt("iroh").as_deref() == Some("1"),
    })
}

/// Prefer IPv4 — it makes for a readable URL and avoids the scope-id quoting
/// that link-local IPv6 needs.
fn pick_address(addresses: &HashSet<ScopedIp>) -> Option<String> {
    let mut v6 = None;
    for address in addresses {
        match address {
            ScopedIp::V4(v4) => return Some(v4.addr().to_string()),
            ScopedIp::V6(candidate) => v6 = Some(format!("[{}]", candidate.addr())),
            // The enum is non-exhaustive; anything new is simply not a form
            // we know how to put in a URL.
            _ => {}
        }
    }
    v6
}

/// `"Living Room._mstream._tcp.local."` → `"Living Room"`.
fn instance_label(fullname: &str) -> String {
    fullname
        .split_once("._mstream.")
        .map(|(label, _)| label.to_string())
        .unwrap_or_else(|| fullname.to_string())
}

/// Diagnostic: list what's on the network right now.
pub fn print_found(seconds: f64) -> i32 {
    match browse(Duration::from_secs_f64(seconds)) {
        Ok(servers) if servers.is_empty() => {
            println!("(no mStream servers found on this network)");
            0
        }
        Ok(servers) => {
            for server in servers {
                println!(
                    "{:<24} {:<32} {}{}",
                    server.name,
                    server.base_url,
                    server.version.map(|v| format!("v{v}")).unwrap_or_default(),
                    if server.quick_connect { "  [quick connect]" } else { "" },
                );
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_service_suffix_from_an_instance_name() {
        assert_eq!(instance_label("Living Room._mstream._tcp.local."), "Living Room");
        // Anything unexpected is passed through rather than mangled.
        assert_eq!(instance_label("odd-name"), "odd-name");
    }
}
