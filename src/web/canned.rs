//! The one answer the browser still makes up.
//!
//! The api worker is real now; what a browser can never do is mDNS, so the
//! Discover-servers view gets these. The library data this module used to
//! fake comes from the actual server the page fronts.

use crate::discovery::DiscoveredServer;

pub fn lan_servers() -> Vec<DiscoveredServer> {
    vec![
        DiscoveredServer {
            name: "Living Room".to_string(),
            base_url: "http://192.168.1.71:3000".to_string(),
            version: Some("5.13.0".to_string()),
            quick_connect: true,
        },
        DiscoveredServer {
            name: "Attic NAS".to_string(),
            base_url: "http://192.168.1.4:3000".to_string(),
            version: Some("5.12.2".to_string()),
            quick_connect: false,
        },
    ]
}
