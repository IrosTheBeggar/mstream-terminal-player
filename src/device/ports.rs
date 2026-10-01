//! Which serial ports could be a Core2. The board itself has no USB
//! identity: what the host sees is M5Stack's USB-to-serial bridge, and the
//! Core2 has shipped with two — a WCH CH9102 on the current units, a
//! Silicon Labs CP2104 on the early ones. Either match means "an ESP32
//! board of M5Stack's", not "a Core2": the flash page names the Core2 and
//! asks, and the engine pins the chip family before it writes.

use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};

use super::DeviceError;

/// (vendor, product, what to call it) — the bridges a Core2 can carry.
const BRIDGES: [(u16, u16, &str); 2] = [(0x1A86, 0x55D4, "CH9102"), (0x10C4, 0xEA60, "CP210x")];

/// A port that looks like a Core2: its name, the bridge that gave it away,
/// and the USB identity the engine's reset logic wants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub port: String,
    pub bridge: &'static str,
    pub usb: UsbPortInfo,
}

impl Candidate {
    /// `COM3 · CH9102 · serial 5B1F007751` — one line for a list.
    pub fn describe(&self) -> String {
        let mut line = format!("{} · {}", self.port, self.bridge);
        if let Some(serial) = self.usb.serial_number.as_deref().filter(|s| !s.is_empty()) {
            line.push_str(&format!(" · serial {serial}"));
        }
        line
    }

    /// A port named by hand (`--port`) that the bridge table did not list:
    /// opened all the same, with an empty USB identity (espflash's own
    /// shape for a port with no USB story — only the reset strategy reads
    /// it, and a zero PID means the classic DTR/RTS dance).
    pub fn bare(port: &str) -> Candidate {
        Candidate {
            port: port.to_string(),
            bridge: "?",
            usb: UsbPortInfo {
                vid: 0,
                pid: 0,
                serial_number: None,
                manufacturer: None,
                product: None,
            },
        }
    }
}

/// The boards plugged in right now.
pub(crate) fn candidates() -> Result<Vec<Candidate>, DeviceError> {
    let ports = serialport::available_ports().map_err(|e| DeviceError::List(e.to_string()))?;
    Ok(filter(ports))
}

/// The other serial ports — everything that is not a Core2's bridge, by
/// name — for the page to list when it finds no board: the usual answer
/// to "is the driver installed?" is a port with another bridge, or none.
pub(crate) fn others() -> Vec<String> {
    serialport::available_ports().map(others_of).unwrap_or_default()
}

pub(crate) fn others_of(ports: Vec<SerialPortInfo>) -> Vec<String> {
    let core2: Vec<String> = filter(ports.clone()).into_iter().map(|c| c.port).collect();
    ports
        .into_iter()
        .map(|p| p.port_name)
        .filter(|name| !core2.contains(name) && !name.starts_with("/dev/tty."))
        .collect()
}

/// The Core2-shaped ports among `ports`, in the order the OS listed them.
/// macOS lists every bridge twice — `/dev/cu.*` and `/dev/tty.*` — and the
/// tty side blocks on carrier detect, so only the cu side counts.
pub(crate) fn filter(ports: Vec<SerialPortInfo>) -> Vec<Candidate> {
    ports
        .into_iter()
        .filter_map(|port| {
            let SerialPortType::UsbPort(usb) = port.port_type else {
                return None;
            };
            if port.port_name.starts_with("/dev/tty.") {
                return None;
            }
            let bridge = BRIDGES.iter().find(|(vid, pid, _)| *vid == usb.vid && *pid == usb.pid)?.2;
            Some(Candidate { port: port.port_name, bridge, usb })
        })
        .collect()
}

/// What a scan found, against what was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    /// The board to use.
    One(Candidate),
    /// Nothing Core2-shaped is plugged in.
    None,
    /// More than one, and no `--port` to choose: the page asks.
    Several,
}

/// The board to use: the one named (listed or not), else the only one.
/// Port names are compared case-insensitively — `com3` and `COM3` are the
/// same port on Windows, and nothing else ever differs by case.
pub(crate) fn pick(found: &[Candidate], wanted: Option<&str>) -> Pick {
    if let Some(name) = wanted.map(str::trim).filter(|n| !n.is_empty()) {
        return Pick::One(
            found
                .iter()
                .find(|c| c.port.eq_ignore_ascii_case(name))
                .cloned()
                .unwrap_or_else(|| Candidate::bare(name)),
        );
    }
    match found {
        [] => Pick::None,
        [one] => Pick::One(one.clone()),
        _ => Pick::Several,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb(name: &str, vid: u16, pid: u16, serial: Option<&str>) -> SerialPortInfo {
        SerialPortInfo {
            port_name: name.to_string(),
            port_type: SerialPortType::UsbPort(UsbPortInfo {
                vid,
                pid,
                serial_number: serial.map(str::to_string),
                manufacturer: None,
                product: None,
            }),
        }
    }

    #[test]
    fn only_the_two_bridges_count_and_only_once_each_on_macos() {
        let ports = vec![
            usb("COM3", 0x1A86, 0x55D4, Some("5B1F007751")),
            usb("COM4", 0x10C4, 0xEA60, None),
            usb("COM5", 0x1A86, 0x7523, None), // a CH340: an ESP board, not a Core2's bridge
            SerialPortInfo { port_name: "COM1".into(), port_type: SerialPortType::PciPort },
            usb("/dev/tty.usbserial-5", 0x1A86, 0x55D4, Some("x")),
            usb("/dev/cu.usbserial-5", 0x1A86, 0x55D4, Some("x")),
        ];
        let others = others_of(ports.clone());
        assert_eq!(others, ["COM5", "COM1"], "the CH340 and the PCI port; never a Core2, never a tty twin");
        let found = filter(ports);
        let names: Vec<&str> = found.iter().map(|c| c.port.as_str()).collect();
        assert_eq!(names, ["COM3", "COM4", "/dev/cu.usbserial-5"]);
        assert_eq!(found[0].bridge, "CH9102");
        assert_eq!(found[1].bridge, "CP210x");
        assert_eq!(found[0].describe(), "COM3 · CH9102 · serial 5B1F007751");
        assert_eq!(found[1].describe(), "COM4 · CP210x", "no serial, no serial column");
    }

    #[test]
    fn a_named_port_wins_listed_or_not_else_the_only_board() {
        let found = filter(vec![usb("COM3", 0x1A86, 0x55D4, None), usb("COM7", 0x10C4, 0xEA60, None)]);
        assert_eq!(pick(&found, None), Pick::Several);
        assert!(matches!(pick(&found, Some("com7")), Pick::One(c) if c.port == "COM7" && c.bridge == "CP210x"));
        assert!(
            matches!(pick(&found, Some("COM9")), Pick::One(c) if c.port == "COM9" && c.bridge == "?"),
            "an unlisted port is still opened, as asked"
        );
        assert_eq!(pick(&found[..1], Some("  ")), Pick::One(found[0].clone()), "blank means not asked");
        assert_eq!(pick(&[], None), Pick::None);
    }
}
