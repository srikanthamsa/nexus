//! Which addresses this machine can be reached on, and whether ZeroTier is one.
//!
//! A direct session needs a peer-reachable address, but a Mac usually has
//! several: `en0`, a few `utun*` interfaces, and whatever a VPN or ZeroTier
//! created. Listing every usable IPv4 address lets the Host screen offer a
//! concrete choice instead of guessing. Recognising ZeroTier is what makes a
//! single-string join code practical without port forwarding or a relay: both
//! peers are already on the same virtual network, so the host can advertise
//! the address ZeroTier assigned it and the friend can reach it directly.
//!
//! ZeroTier is detected through its own CLI rather than by interface name,
//! because the interface name (`feth2576` on this Mac) is generated per
//! install. The CLI's JSON also names the network and the local device, which
//! is what the UI wants to show. Every part degrades quietly: a missing CLI,
//! a stopped daemon, or malformed output simply means no ZeroTier entries and
//! the plain interface list still works.

use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::process::Command;

/// Preferred command name. The installer's location is tried as a fallback
/// because a GUI app can start with a PATH that does not include
/// `/usr/local/bin`, and the binary is a symlink whose name selects CLI mode.
const ZERO_TIER_CLI: &str = "zerotier-cli";
const ZERO_TIER_CLI_FALLBACK: &str = "/usr/local/bin/zerotier-cli";

/// One usable IPv4 address with the interface that carries it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkAddress {
    pub interface: String,
    pub address: String,
    pub label: String,
    pub zero_tier: bool,
}

/// The ZeroTier network this machine is joined to, for display.
#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ZeroTierInfo {
    pub network_id: String,
    pub name: String,
    pub status: String,
}

/// Everything the Host screen needs to describe how this Mac is reachable.
#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInfo {
    pub addresses: Vec<NetworkAddress>,
    pub zero_tier: Option<ZeroTierInfo>,
}

/// A ZeroTier network as reported by `zerotier-cli -j listnetworks`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ZeroTierNetwork {
    id: String,
    name: String,
    status: String,
    assigned: Vec<Ipv4Addr>,
    port_device: String,
}

/// Lists the usable IPv4 addresses and, when available, the ZeroTier network.
pub fn network_info() -> NetworkInfo {
    let addresses = interface_addresses();
    let networks = zero_tier_networks();
    compose_network_info(addresses, &networks)
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
                // Until ZeroTier claims it, an address is labelled by its
                // interface, which is what a LAN user recognises.
                label: interface.name.clone(),
                interface: interface.name,
                address: ip.to_string(),
                zero_tier: false,
            })
        })
        .collect()
}

/// Reads ZeroTier's joined networks, tolerating a missing or failing CLI.
fn zero_tier_networks() -> Vec<ZeroTierNetwork> {
    for candidate in [ZERO_TIER_CLI, ZERO_TIER_CLI_FALLBACK] {
        let Ok(output) = Command::new(candidate)
            .args(["-j", "listnetworks"])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        return parse_zero_tier_networks(&stdout);
    }
    Vec::new()
}

/// Parses `zerotier-cli -j listnetworks` output.
///
/// Kept pure so the interesting cases (id/nwid fallback, CIDR addresses,
/// malformed entries) are testable without a ZeroTier install. Anything that
/// does not look like a joined network is skipped rather than failing the
/// whole list: one bad entry must not hide a usable network.
fn parse_zero_tier_networks(json: &str) -> Vec<ZeroTierNetwork> {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };

    let mut networks = Vec::new();
    for entry in entries {
        let Some(object) = entry.as_object() else {
            continue;
        };
        // Modern clients report `id`; older ones only had `nwid`.
        let id = match text_field(object, "id").or_else(|| text_field(object, "nwid")) {
            Some(id) => id,
            None => continue,
        };
        let assigned = object
            .get("assignedAddresses")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let cidr = item.as_str()?.trim();
                        cidr.split('/').next()?.parse::<Ipv4Addr>().ok()
                    })
                    .collect()
            })
            .unwrap_or_default();
        networks.push(ZeroTierNetwork {
            id,
            name: text_field(object, "name").unwrap_or_default(),
            status: text_field(object, "status").unwrap_or_default(),
            assigned,
            port_device: text_field(object, "portDeviceName").unwrap_or_default(),
        });
    }
    networks
}

/// A trimmed, non-empty string field from a JSON object.
fn text_field(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    let value = object.get(key)?.as_str()?.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.to_string())
}

/// Merges interface addresses with ZeroTier networks: mark assigned
/// addresses, add any the interface list missed, then order and dedupe.
fn compose_network_info(
    addresses: Vec<NetworkAddress>,
    networks: &[ZeroTierNetwork],
) -> NetworkInfo {
    let mut addresses = addresses;
    let mut present: HashSet<String> = addresses
        .iter()
        .map(|entry| entry.address.clone())
        .collect();

    for network in networks {
        let label = format!("ZeroTier {}", network.id);
        for assigned in &network.assigned {
            let address = assigned.to_string();
            // Resolve the position first so the borrow ends before any push:
            // an address can be both reported and missing, and the two paths
            // must not borrow the list at once.
            match addresses.iter().position(|entry| entry.address == address) {
                Some(index) => {
                    // First network wins if two networks assign the same address.
                    if !addresses[index].zero_tier {
                        addresses[index].zero_tier = true;
                        addresses[index].label = label.clone();
                    }
                }
                None => {
                    if present.insert(address.clone()) {
                        // Assigned by ZeroTier but not visible in if_addrs; the
                        // network's device name is the closest interface we have.
                        addresses.push(NetworkAddress {
                            interface: network.port_device.clone(),
                            address,
                            label: label.clone(),
                            zero_tier: true,
                        });
                    }
                }
            }
        }
    }

    // ZeroTier first (it is the remote-friendly address), then interface and
    // address so the list is stable between calls.
    addresses.sort_by(|left, right| {
        (
            !left.zero_tier,
            left.interface.as_str(),
            left.address.as_str(),
        )
            .cmp(&(
                !right.zero_tier,
                right.interface.as_str(),
                right.address.as_str(),
            ))
    });
    let mut seen = HashSet::new();
    addresses.retain(|entry| seen.insert(entry.address.clone()));

    NetworkInfo {
        addresses,
        zero_tier: networks.first().map(|network| ZeroTierInfo {
            network_id: network.id.clone(),
            name: network.name.clone(),
            status: network.status.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
        {
            "id": "a09acf02332a5b30",
            "nwid": "a09acf02332a5b30",
            "name": "srikanthamsa",
            "status": "OK",
            "assignedAddresses": ["192.168.195.252/24"],
            "portDeviceName": "feth2576"
        },
        {
            "nwid": "8056c2e21c000001",
            "name": "fallback-nwid",
            "status": "REQUESTING_CONFIGURATION",
            "assignedAddresses": ["10.147.20.5/24", "not-an-address/24"],
            "portDeviceName": ""
        }
    ]"#;

    fn address(interface: &str, ip: &str) -> NetworkAddress {
        NetworkAddress {
            interface: interface.to_string(),
            address: ip.to_string(),
            label: interface.to_string(),
            zero_tier: false,
        }
    }

    fn network(id: &str, assigned: &[&str], device: &str) -> ZeroTierNetwork {
        ZeroTierNetwork {
            id: id.to_string(),
            name: "test".to_string(),
            status: "OK".to_string(),
            assigned: assigned
                .iter()
                .map(|ip| ip.parse::<Ipv4Addr>().expect("valid test address"))
                .collect(),
            port_device: device.to_string(),
        }
    }

    #[test]
    fn parses_networks_with_id_and_nwid_fallback() {
        let networks = parse_zero_tier_networks(FIXTURE);
        assert_eq!(networks.len(), 2);
        assert_eq!(networks[0].id, "a09acf02332a5b30");
        assert_eq!(networks[0].name, "srikanthamsa");
        assert_eq!(networks[0].status, "OK");
        assert_eq!(networks[1].id, "8056c2e21c000001");
        assert_eq!(networks[1].status, "REQUESTING_CONFIGURATION");
    }

    #[test]
    fn parses_assigned_cidrs_and_skips_bad_ones() {
        let networks = parse_zero_tier_networks(FIXTURE);
        assert_eq!(
            networks[0].assigned,
            vec!["192.168.195.252".parse::<Ipv4Addr>().expect("address")]
        );
        assert_eq!(
            networks[1].assigned,
            vec!["10.147.20.5".parse::<Ipv4Addr>().expect("address")]
        );
        assert_eq!(networks[0].port_device, "feth2576");
    }

    #[test]
    fn skips_malformed_entries_and_tolerates_garbage() {
        let json = r#"[
            null,
            42,
            {"name": "no id at all"},
            {"id": ""},
            {"id": "ok", "assignedAddresses": "not-an-array"},
            {"id": "ok2", "name": 7, "status": false}
        ]"#;
        let networks = parse_zero_tier_networks(json);
        assert_eq!(networks.len(), 2);
        assert_eq!(networks[0].id, "ok");
        assert!(networks[0].assigned.is_empty());
        assert_eq!(networks[1].id, "ok2");
        // Non-string name/status fall back to empty rather than failing.
        assert_eq!(networks[1].name, "");
        assert_eq!(networks[1].status, "");

        assert!(parse_zero_tier_networks("not json").is_empty());
        assert!(parse_zero_tier_networks("{}").is_empty());
    }

    #[test]
    fn marks_matching_addresses_and_adds_missing_ones() {
        let interfaces = vec![
            address("feth2576", "192.168.195.252"),
            address("en0", "10.0.0.5"),
        ];
        let networks = vec![network(
            "a09acf02332a5b30",
            &["192.168.195.252", "192.168.195.253"],
            "feth2576",
        )];
        let info = compose_network_info(interfaces, &networks);

        assert_eq!(info.addresses.len(), 3);
        // ZeroTier entries come first, sorted by interface then address. Both
        // live on feth2576 here, so the lower address leads.
        assert_eq!(info.addresses[0].address, "192.168.195.252");
        assert_eq!(info.addresses[0].interface, "feth2576");
        assert_eq!(info.addresses[0].label, "ZeroTier a09acf02332a5b30");
        assert!(info.addresses[0].zero_tier);
        // The assigned address the interface list missed is added with the
        // network's device name.
        assert_eq!(info.addresses[1].address, "192.168.195.253");
        assert_eq!(info.addresses[1].interface, "feth2576");
        assert!(info.addresses[1].zero_tier);
        // The plain LAN address stays last, labelled by its interface.
        assert_eq!(info.addresses[2].address, "10.0.0.5");
        assert_eq!(info.addresses[2].label, "en0");
        assert!(!info.addresses[2].zero_tier);

        let zero_tier = info.zero_tier.expect("network info");
        assert_eq!(zero_tier.network_id, "a09acf02332a5b30");
        assert_eq!(zero_tier.name, "test");
        assert_eq!(zero_tier.status, "OK");
    }

    #[test]
    fn sorts_zero_tier_first_then_interface_then_address_and_dedupes() {
        let interfaces = vec![
            address("en0", "192.168.1.50"),
            address("en1", "192.168.1.60"),
            address("en0", "192.168.1.40"),
            // The same address can appear on two interfaces with overlapping
            // subnets; the list must not show it twice.
            address("en2", "192.168.195.252"),
            address("en3", "192.168.1.50"),
        ];
        let networks = vec![network("zt", &["192.168.195.252"], "feth2576")];
        let info = compose_network_info(interfaces, &networks);

        let ordered: Vec<&str> = info
            .addresses
            .iter()
            .map(|entry| entry.address.as_str())
            .collect();
        assert_eq!(
            ordered,
            vec![
                "192.168.195.252",
                "192.168.1.40",
                "192.168.1.50",
                "192.168.1.60"
            ]
        );
        assert_eq!(info.addresses.len(), 4);
    }

    #[test]
    fn no_networks_means_plain_interfaces_and_no_info() {
        let interfaces = vec![address("en0", "10.0.0.5")];
        let info = compose_network_info(interfaces, &[]);
        assert!(info.zero_tier.is_none());
        assert_eq!(info.addresses.len(), 1);
        assert_eq!(info.addresses[0].label, "en0");
    }
}
