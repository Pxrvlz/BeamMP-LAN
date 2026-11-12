//! LAN discovery: find BeamMP servers without any internet backend.
//!
//! Strategy (docs/PLAN.md §6.1):
//! 1. Enumerate all non-loopback IPv4 interfaces with `if_addrs` (this automatically
//!    picks up Radmin VPN's `26.x/25.x`, ZeroTier, Tailscale `100.x`, Hamachi `25.x`,
//!    physical LAN subnets…).
//! 2. For each interface, scan its /24 (or the configured prefix) for open
//!    TCP :30814 with a short timeout, in parallel.
//! 3. For every open port, send the BeamMP `'I'` info packet; parse the reply.
//! 4. Report found servers with ping (via the `'P'` code).

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::net;

/// One scan result ready for the UI.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DiscoveredServer {
    pub info: net::ServerInfo,
    pub ping_ms: Option<u64>,
    pub interface: String,
}

/// Info about one usable local interface.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InterfaceInfo {
    pub name: String,
    pub ip: Ipv4Addr,
    pub prefix: u8,
    /// Human hint, e.g. "Radmin VPN (26.x)" for known VPN ranges.
    pub hint: String,
}

/// Guess a friendly hint from the address range.
pub fn interface_hint(ip: Ipv4Addr) -> String {
    let o = ip.octets();
    match o[0] {
        26 => "Radmin VPN".into(),
        25 => "Hamachi / Radmin VPN".into(),
        100 => "Tailscale / CGNAT".into(),
        172 if (16..=31).contains(&o[1]) => "Docker / private".into(),
        10 => "Private LAN".into(),
        192 if o[1] == 168 => "Private LAN".into(),
        169 if o[1] == 254 => "Link-local".into(),
        _ => "Network".into(),
    }
}

/// Enumerate usable interfaces (IPv4, non-loopback, with broadcast-capable subnets).
pub fn list_interfaces() -> Vec<InterfaceInfo> {
    let mut out = Vec::new();
    let ifaces = if_addrs::get_if_addrs().unwrap_or_default();
    for iface in ifaces {
        if !iface.is_loopback() {
            if let if_addrs::IfAddr::V4(v4) = iface.addr {
                // netmask → prefix length
                let mask = u32::from(v4.netmask);
                let prefix = mask.count_ones() as u8;
                if prefix <= 30 {
                    out.push(InterfaceInfo {
                        name: iface.name,
                        ip: v4.ip,
                        prefix,
                        hint: interface_hint(v4.ip),
                    });
                }
            }
        }
    }
    out.sort_by_key(|i| i.ip);
    out
}

/// All candidate host addresses of an interface's subnet (capped at /24 → 254 hosts).
pub fn subnet_hosts(ip: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    if prefix > 24 {
        // Tiny subnet: enumerate the whole thing.
        let net = u32::from(ip) & prefix_mask(prefix);
        let count = 1u32 << (32 - prefix);
        return (0..count.min(254))
            .map(|i| Ipv4Addr::from(net + i))
            .collect();
    }
    // Cap at the /24 containing the local IP to avoid scanning /8 Radmin networks in full.
    let net = u32::from(ip) & 0xFFFF_FF00;
    (1..=254u32).map(|i| Ipv4Addr::from(net | i)).collect()
}

fn prefix_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX.checked_shl(32 - prefix as u32).unwrap_or(0)
    }
}

/// Scan parameters.
#[derive(Debug, Clone)]
pub struct ScanConfig {
    /// Only scan these interfaces (empty = all).
    pub only_interfaces: Vec<String>,
    /// Ports to probe on each host.
    pub ports: Vec<u16>,
    /// Per-connect timeout.
    pub connect_timeout: Duration,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            only_interfaces: Vec::new(),
            ports: vec![net::DEFAULT_SERVER_PORT],
            connect_timeout: Duration::from_millis(350),
        }
    }
}

/// Blocking scan (run on a worker thread, not the UI thread).
/// `progress_cb` is called with (scanned, total, found).
pub fn scan_lan(
    cfg: &ScanConfig,
    progress_cb: &(dyn Fn(usize, usize, usize) + Send + Sync),
) -> Vec<DiscoveredServer> {
    let interfaces: Vec<InterfaceInfo> = list_interfaces()
        .into_iter()
        .filter(|i| {
            cfg.only_interfaces.is_empty()
                || cfg
                    .only_interfaces
                    .iter()
                    .any(|n| *n == i.name || n.parse::<Ipv4Addr>().is_ok_and(|ip| ip == i.ip))
        })
        .collect();

    // Dedup candidate hosts across interfaces (a host may be reachable via 2 NICs).
    let mut hosts: BTreeSet<Ipv4Addr> = BTreeSet::new();
    for iface in &interfaces {
        for h in subnet_hosts(iface.ip, iface.prefix) {
            if h != iface.ip {
                hosts.insert(h);
            }
        }
    }

    let total = hosts.len() * cfg.ports.len();
    let scanned = Arc::new(AtomicUsize::new(0));
    let found = Arc::new(AtomicUsize::new(0));
    let results: std::sync::Mutex<Vec<DiscoveredServer>> = std::sync::Mutex::new(Vec::new());
    let progress = progress_cb;

    // Simple thread pool sized to the machine; each worker pulls hosts off a shared queue.
    let queue = std::sync::Mutex::new(hosts.into_iter().collect::<Vec<_>>());
    let workers = std::thread::available_parallelism()
        .map(|n| n.get().clamp(8, 128))
        .unwrap_or(16);
    let ports = cfg.ports.clone();
    let connect_timeout = cfg.connect_timeout;

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let host = {
                    let mut q = queue.lock().unwrap();
                    q.pop()
                };
                let Some(host) = host else { break };
                for port in &ports {
                    let addr = SocketAddrV4::new(host, *port).to_string();
                    let probe = std::net::TcpStream::connect_timeout(
                        &SocketAddrV4::new(host, *port).into(),
                        connect_timeout,
                    );
                    if probe.is_ok() {
                        if let Some(info) = net::query_server_info(&addr) {
                            let ping = net::ping_server(&addr).map(|d| d.as_millis() as u64);
                            results.lock().unwrap().push(DiscoveredServer {
                                interface: format!("{host}"),
                                info,
                                ping_ms: ping,
                            });
                            found.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    let s = scanned.fetch_add(1, Ordering::Relaxed) + 1;
                    progress(s, total, found.load(Ordering::Relaxed));
                }
            });
        }
    });

    let mut out = results.into_inner().unwrap();
    out.sort_by_key(|d| d.info.name.clone());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnet_hosts_capped_at_254() {
        let hosts = subnet_hosts("192.168.1.42".parse().unwrap(), 24);
        assert_eq!(hosts.len(), 254);
        assert!(hosts.contains(&"192.168.1.1".parse().unwrap()));
        // note: self-exclusion is applied by the caller (scan_lan), not here
        assert!(hosts.contains(&"192.168.1.42".parse().unwrap()));
    }

    #[test]
    fn radmin_hint() {
        assert_eq!(interface_hint("26.10.0.5".parse().unwrap()), "Radmin VPN");
        assert_eq!(
            interface_hint("192.168.0.2".parse().unwrap()),
            "Private LAN"
        );
    }

    #[test]
    fn prefix_masks() {
        assert_eq!(prefix_mask(24), 0xFFFFFF00);
        assert_eq!(prefix_mask(8), 0xFF000000);
    }
}
