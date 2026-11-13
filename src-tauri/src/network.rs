//! Which addresses this machine can be reached on, and whether Tailscale is one.
//!
//! A direct session needs a peer-reachable address, but a Mac usually has
//! several: `en0`, a few `utun*` interfaces, and whatever a VPN or Tailscale
//! created. Listing every usable IPv4 address lets the Host screen offer a
//! concrete choice instead of guessing. Recognising Tailscale is what makes a
//! single-string join code practical without port forwarding or a relay: both
//! peers are already on the same tailnet, so the host can advertise the
//! address Tailscale assigned it and the friend can reach it directly.
//!
//! Tailscale is detected through its own CLI rather than by interface name,
//! because the interface name (`utun*`) is not stable and is shared with other
//! tunnels. The CLI's JSON also names the tailnet and the local device, which
//! is what the UI wants to show. Every part degrades quietly: a missing CLI,
//! a stopped daemon, or malformed output simply means no Tailscale entries and
//! the plain interface list still works.

use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::process::Command;

/// A GUI app can start with a PATH that does not include the CLI, so try the
/// usual install locations after the plain command name. The macOS app bundle
/// carries the real binary; `/usr/local/bin/tailscale` is usually a shim.
const TAILSCALE_CLI_CANDIDATES: [&str; 4] = [
    "tailscale",
    "/usr/local/bin/tailscale",
    "/opt/homebrew/bin/tailscale",
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
];

/// One usable IPv4 address with the interface that carries it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkAddress {
    pub interface: String,
    pub address: String,
    pub label: String,
    pub tailscale: bool,
}

/// The Tailscale tailnet this machine is joined to, for display.
#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleInfo {
    /// Stable tailnet suffix, e.g. `tail98fbab.ts.net`.
    pub tailnet: String,
    /// Host name reported by Tailscale, e.g. `Srikant’s MacBook Air`.
    pub host_name: String,
    /// Backend state, e.g. `Running`.
    pub status: String,
    /// The Tailscale IPv4 address(es) assigned to this Mac.
    pub addresses: Vec<String>,
}

/// Everything the Host screen needs to describe how this Mac is reachable.
#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInfo {
    pub addresses: Vec<NetworkAddress>,
    pub tailscale: Option<TailscaleInfo>,
}

/// A parsed `tailscale status --json` result.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TailscaleStatus {
    tailnet: String,
    host_name: String,
    status: String,
    assigned: Vec<Ipv4Addr>,
}

/// Lists the usable IPv4 addresses and, when available, the Tailscale tailnet.
pub fn network_info() -> NetworkInfo {
    let addresses = interface_addresses();
    let status = tailscale_status();
    compose_network_info(addresses, status.as_ref())
}

/// Every non-loopback, non-link-local IPv4 address `if_addrs` reports.
fn interface_addresses() -> Vec<NetworkAddress> {
    let interfaces = if_addrs::get_if_addrs().unwrap_or_default();
    interfaces
        .into_iter()
        .filter_map(|interface| {
            let if_addrs::IfAddr::V4(ipv4) = interface.addr else {
                // IPv6 is not part of the direct-session path yet.
                return None;
            };
            let ip = ipv4.ip;
            if ip.is_loopback() || ip.is_unspecified() || ip.is_link_local() {
                return None;
            }
            Some(NetworkAddress {
                // Until Tailscale claims it, an address is labelled by its
                // interface, which is what a LAN user recognises.
                label: interface.name.clone(),
                interface: interface.name,
                address: ip.to_string(),
                tailscale: false,
            })
        })
        .collect()
}

/// Reads `tailscale status --json`, tolerating a missing or failing CLI.
fn tailscale_status() -> Option<TailscaleStatus> {
    for candidate in TAILSCALE_CLI_CANDIDATES {
        let Ok(output) = Command::new(candidate).args(["status", "--json"]).output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(status) = parse_tailscale_status(&stdout) {
            return Some(status);
        }
    }
    None
}

/// Parses `tailscale status --json` output.
///
/// Current clients expose the IPv4/IPv6 list both at the top level and under
/// `Self`; older clients only had the latter. IPv6 is ignored because the
/// direct-session path is IPv4, and a tailnet without an IPv4 address is not
/// useful here. Kept pure so the JSON shapes are testable without Tailscale.
fn parse_tailscale_status(json: &str) -> Option<TailscaleStatus> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let object = value.as_object()?;

    let assigned = object
        .get("TailscaleIPs")
        .and_then(|value| value.as_array())
        .or_else(|| {
            object
                .get("Self")
                .and_then(|value| value.as_object())
                .and_then(|self_object| self_object.get("TailscaleIPs"))
                .and_then(|value| value.as_array())
        })?
        .iter()
        .filter_map(|item| item.as_str()?.trim().parse::<Ipv4Addr>().ok())
        .collect::<Vec<_>>();

    if assigned.is_empty() {
        return None;
    }

    let self_object = object.get("Self").and_then(|value| value.as_object());
    let host_name = self_object
        .and_then(|self_object| text_field(self_object, "HostName"))
        .or_else(|| self_object.and_then(|self_object| text_field(self_object, "DNSName")))
        .unwrap_or_default();

    let current_tailnet = object
        .get("CurrentTailnet")
        .and_then(|value| value.as_object());
    let tailnet = current_tailnet
        .and_then(|tailnet| text_field(tailnet, "MagicDNSSuffix"))
        .or_else(|| text_field(object, "MagicDNSSuffix"))
        .or_else(|| current_tailnet.and_then(|tailnet| text_field(tailnet, "Name")))
        .unwrap_or_default();

    Some(TailscaleStatus {
        tailnet,
        host_name,
        status: text_field(object, "BackendState").unwrap_or_else(|| "Unknown".to_string()),
        assigned,
    })
}

/// A trimmed, non-empty string field from a JSON object.
fn text_field(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    let value = object.get(key)?.as_str()?.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.to_string())
}

/// Merges interface addresses with the Tailscale status: mark assigned
/// addresses, add any the interface list missed, then order and dedupe.
fn compose_network_info(
    addresses: Vec<NetworkAddress>,
    status: Option<&TailscaleStatus>,
) -> NetworkInfo {
    let mut addresses = addresses;
    let mut present: HashSet<String> = addresses
        .iter()
        .map(|entry| entry.address.clone())
        .collect();

    if let Some(status) = status {
        let label = if status.tailnet.is_empty() {
            "Tailscale".to_string()
        } else {
            format!("Tailscale {}", status.tailnet)
        };
        for assigned in &status.assigned {
            let address = assigned.to_string();
            match addresses.iter().position(|entry| entry.address == address) {
                Some(index) => {
                    if !addresses[index].tailscale {
                        addresses[index].tailscale = true;
                        addresses[index].label = label.clone();
                    }
                }
                None => {
                    if present.insert(address.clone()) {
                        // Assigned by Tailscale but not visible in if_addrs; a
                        // utun interface is the closest label we have.
                        addresses.push(NetworkAddress {
                            interface: "utun".to_string(),
                            address,
                            label: label.clone(),
                            tailscale: true,
                        });
                    }
                }
            }
        }
    }

    // Tailscale first (it is the remote-friendly address), then interface and
    // address so the list is stable between calls.
    addresses.sort_by(|left, right| {
        (
            !left.tailscale,
            left.interface.as_str(),
            left.address.as_str(),
        )
            .cmp(&(
                !right.tailscale,
                right.interface.as_str(),
                right.address.as_str(),
            ))
    });
    let mut seen = HashSet::new();
    addresses.retain(|entry| seen.insert(entry.address.clone()));

    NetworkInfo {
        addresses,
        tailscale: status.map(|status| TailscaleInfo {
            tailnet: status.tailnet.clone(),
            host_name: status.host_name.clone(),
            status: status.status.clone(),
            addresses: status.assigned.iter().map(ToString::to_string).collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "BackendState": "Running",
        "TailscaleIPs": ["100.125.242.28", "fd7a:115c:a1e0::bb38:f21d"],
        "MagicDNSSuffix": "tail98fbab.ts.net",
        "Self": {
            "HostName": "Srikant’s MacBook Air",
            "DNSName": "srikants-macbook-air.tail98fbab.ts.net.",
            "TailscaleIPs": ["100.125.242.28", "fd7a:115c:a1e0::bb38:f21d"]
        },
        "CurrentTailnet": {
            "Name": "srikanthamsa@gmail.com",
            "MagicDNSSuffix": "tail98fbab.ts.net"
        }
    }"#;

    fn address(interface: &str, ip: &str) -> NetworkAddress {
        NetworkAddress {
            interface: interface.to_string(),
            address: ip.to_string(),
            label: interface.to_string(),
            tailscale: false,
        }
    }

    fn status(assigned: &[&str], tailnet: &str) -> TailscaleStatus {
        TailscaleStatus {
            tailnet: tailnet.to_string(),
            host_name: "Test Mac".to_string(),
            status: "Running".to_string(),
            assigned: assigned
                .iter()
                .map(|ip| ip.parse::<Ipv4Addr>().expect("valid test address"))
                .collect(),
        }
    }

    #[test]
    fn parses_current_tailscale_status() {
        let status = parse_tailscale_status(FIXTURE).expect("status");
        assert_eq!(status.tailnet, "tail98fbab.ts.net");
        assert_eq!(status.host_name, "Srikant’s MacBook Air");
        assert_eq!(status.status, "Running");
        assert_eq!(
            status.assigned,
            vec!["100.125.242.28".parse::<Ipv4Addr>().expect("address")]
        );
    }

    #[test]
    fn falls_back_to_self_ips_and_hostname() {
        let json = r#"{
            "BackendState": "NeedsLogin",
            "Self": {
                "HostName": "Office Mac",
                "TailscaleIPs": ["100.64.0.9"]
            }
        }"#;
        let status = parse_tailscale_status(json).expect("status");
        assert_eq!(status.assigned, vec![Ipv4Addr::new(100, 64, 0, 9)]);
        assert_eq!(status.host_name, "Office Mac");
        assert_eq!(status.status, "NeedsLogin");
        assert_eq!(status.tailnet, "");
    }

    #[test]
    fn ignores_ipv6_only_and_malformed_statuses() {
        assert!(parse_tailscale_status(r#"{"TailscaleIPs":["fd7a::1"]}"#).is_none());
        assert!(parse_tailscale_status("not json").is_none());
        assert!(parse_tailscale_status("{}").is_none());
        assert!(parse_tailscale_status("[]").is_none());
    }

    #[test]
    fn marks_matching_addresses_and_adds_missing_ones() {
        let interfaces = vec![
            address("utun6", "100.125.242.28"),
            address("en0", "10.0.0.5"),
        ];
        let status = status(&["100.125.242.28", "100.125.242.29"], "tail98fbab.ts.net");
        let info = compose_network_info(interfaces, Some(&status));

        assert_eq!(info.addresses.len(), 3);
        // The added entry uses the synthetic "utun" interface, which sorts
        // before "utun6"; both are still Tailscale and come before the LAN.
        assert_eq!(info.addresses[0].address, "100.125.242.29");
        assert_eq!(info.addresses[0].interface, "utun");
        assert_eq!(info.addresses[0].label, "Tailscale tail98fbab.ts.net");
        assert!(info.addresses[0].tailscale);
        assert_eq!(info.addresses[1].address, "100.125.242.28");
        assert_eq!(info.addresses[1].interface, "utun6");
        assert!(info.addresses[1].tailscale);
        // The plain LAN address stays last.
        assert_eq!(info.addresses[2].address, "10.0.0.5");
        assert_eq!(info.addresses[2].label, "en0");
        assert!(!info.addresses[2].tailscale);

        let tailscale = info.tailscale.expect("tailscale info");
        assert_eq!(tailscale.tailnet, "tail98fbab.ts.net");
        assert_eq!(tailscale.status, "Running");
        assert_eq!(
            tailscale.addresses,
            vec!["100.125.242.28", "100.125.242.29"]
        );
    }

    #[test]
    fn sorts_tailscale_first_then_interface_then_address_and_dedupes() {
        let interfaces = vec![
            address("en0", "192.168.1.50"),
            address("en1", "192.168.1.60"),
            address("en0", "192.168.1.40"),
            address("utun6", "100.125.242.28"),
            address("en3", "192.168.1.50"),
        ];
        let tailscale = status(&["100.125.242.28"], "tail98fbab.ts.net");
        let info = compose_network_info(interfaces, Some(&tailscale));

        let ordered: Vec<&str> = info
            .addresses
            .iter()
            .map(|entry| entry.address.as_str())
            .collect();
        assert_eq!(
            ordered,
            vec![
                "100.125.242.28",
                "192.168.1.40",
                "192.168.1.50",
                "192.168.1.60"
            ]
        );
        assert_eq!(info.addresses.len(), 4);
    }

    #[test]
    #[ignore = "requires a running Tailscale client on this Mac"]
    fn reads_live_tailscale_status() {
        let status = tailscale_status().expect("Tailscale CLI should return a running status");
        assert_eq!(status.status, "Running");
        assert!(status
            .assigned
            .iter()
            .any(|ip| ip.is_private() || ip.octets()[0] == 100));
    }

    #[test]
    fn no_tailscale_means_plain_interfaces_and_no_info() {
        let interfaces = vec![address("en0", "10.0.0.5")];
        let info = compose_network_info(interfaces, None);
        assert!(info.tailscale.is_none());
        assert_eq!(info.addresses.len(), 1);
        assert_eq!(info.addresses[0].label, "en0");
    }
}
