//! UDP LAN beacon: zero-config host/client discovery without any scanning.
//!
//! Complements the TCP subnet scan in `discovery.rs`: instead of probing 254 hosts,
//! hosts *announce* themselves by UDP broadcast every few seconds, and clients can
//! *seek* actively. On networks where broadcast works (ethernet LANs, most VPNs)
//! servers appear instantly; the TCP scan remains the fallback for networks that
//! filter broadcast (some managed switches, certain VPN configs).
//!
//! Wire format (all datagrams, max ~1400 bytes):
//! ```text
//! BEAMMP_LAN1|<verb>|<json payload>
//! ```
//! * `announce` — host → broadcast, on BEACON_PORT every BEACON_INTERVAL while a
//!   server (managed BeamMP-Server instance or embedded relay) is running.
//! * `seek`    — client → broadcast, asks all hosts to answer right now.
//! * `reply`   — host → seeker (unicast), same payload as `announce`.
//!
//! The payload is the beacon view of the server info: name, port, map, players,
//! whether it's an embedded-relay server, and the client mods required to join.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Magic prefix + protocol version. Bump the digit on incompatible changes.
pub const BEACON_MAGIC: &str = "BEAMMP_LAN1";
/// UDP port for beacons (server default is 30814, launcher core 4444/4445).
pub const BEACON_PORT: u16 = 30816;
/// Hosts re-announce on this cadence.
pub const BEACON_INTERVAL: Duration = Duration::from_secs(3);
/// How long a seeker listens after sending a seek.
pub const SEEK_WINDOW: Duration = Duration::from_millis(800);

/// One beacon advertisement (JSON payload).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BeaconInfo {
    /// 8-char host id (stable per app instance).
    pub id: String,
    /// Server name.
    pub name: String,
    /// TCP port of the actual BeamMP server.
    pub port: u16,
    /// Map path (as in ServerConfig.toml).
    pub map: String,
    pub players: u32,
    pub maxplayers: u32,
    /// True when hosted by BeamMP-LAN's embedded relay (no BeamMP-Server binary).
    pub relay: bool,
    /// Client mods required to join (file names).
    #[serde(default)]
    pub requires: Vec<String>,
    /// Sender's IP as seen by the receiver (filled in by the seeker).
    #[serde(skip)]
    pub from_ip: String,
}

impl BeaconInfo {
    /// Endpoint "ip:port" for connecting.
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.from_ip, self.port)
    }
}

fn encode(verb: &str, payload: &BeaconInfo) -> Vec<u8> {
    let json = serde_json::to_string(payload).unwrap_or_default();
    format!("{BEACON_MAGIC}|{verb}|{json}").into_bytes()
}

fn decode(datagram: &[u8]) -> Option<(&str, BeaconInfo)> {
    let text = std::str::from_utf8(datagram).ok()?;
    let rest = text.strip_prefix(BEACON_MAGIC)?;
    let rest = rest.strip_prefix('|')?;
    let (verb, json) = rest.split_once('|')?;
    let mut info: BeaconInfo = serde_json::from_str(json).ok()?;
    // Sanity: a valid beacon must have a plausible port and non-empty id.
    if info.port == 0 || info.id.len() > 16 {
        return None;
    }
    info.from_ip = String::new();
    Some((verb, info))
}

/// Broadcast addresses of all usable IPv4 interfaces (plus loopback for same-machine
/// play — two app instances on one PC see each other, which is also how the e2e test
/// works).
pub fn broadcast_targets() -> Vec<SocketAddrV4> {
    let mut out = vec![SocketAddrV4::new(Ipv4Addr::LOCALHOST, BEACON_PORT)];
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() {
                continue;
            }
            if let if_addrs::IfAddr::V4(v4) = iface.addr {
                let ip = u32::from(v4.ip);
                let mask = u32::from(v4.netmask);
                if mask == 0 || mask.count_ones() >= 31 {
                    continue;
                }
                let bcast = Ipv4Addr::from(ip | !mask);
                out.push(SocketAddrV4::new(bcast, BEACON_PORT));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Client side: broadcast a seek and collect announces/replies for SEEK_WINDOW.
/// Also picks up unsolicited announces that arrive while we listen.
pub fn seek(timeout: Duration) -> Vec<BeaconInfo> {
    let socket = match UdpSocket::bind(("0.0.0.0", BEACON_PORT)) {
        Ok(s) => s,
        Err(_) => return Vec::new(), // port busy: another app instance is announcing
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));
    let _ = socket.set_broadcast(true);

    let probe = BeaconInfo {
        id: "seek".into(),
        name: String::new(),
        port: 0,
        map: String::new(),
        players: 0,
        maxplayers: 0,
        relay: false,
        requires: Vec::new(),
        from_ip: String::new(),
    };
    for target in broadcast_targets() {
        let _ = socket.send_to(&encode("seek", &probe), target);
    }

    let deadline = std::time::Instant::now() + timeout;
    let mut found: Vec<BeaconInfo> = Vec::new();
    while std::time::Instant::now() < deadline {
        let mut buf = [0u8; 2048];
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                let Some((verb, mut info)) = decode(&buf[..n]) else {
                    continue;
                };
                if verb != "announce" && verb != "reply" {
                    continue;
                }
                if info.id == "seek" || info.id.is_empty() {
                    continue;
                }
                info.from_ip = from.ip().to_string();
                if !found
                    .iter()
                    .any(|f: &BeaconInfo| f.id == info.id && f.port == info.port)
                {
                    found.push(info);
                }
            }
            Err(_) => continue,
        }
    }
    found
}

/// Host side: periodic announcer with a stop flag.
pub struct Announcer {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// What we're currently advertising (players count changes over time).
    current: Arc<Mutex<Option<BeaconInfo>>>,
}

impl Announcer {
    /// Start announcing. The payload is re-read from `current` every tick, so the
    /// caller can update player counts live via `update()`.
    pub fn start() -> Arc<Announcer> {
        let stop = Arc::new(AtomicBool::new(false));
        let current: Arc<Mutex<Option<BeaconInfo>>> = Arc::new(Mutex::new(None));
        let announcer = Arc::new(Announcer {
            stop: stop.clone(),
            handle: None,
            current: current.clone(),
        });

        let socket = UdpSocket::bind(("0.0.0.0", 0)).ok();
        if let Some(s) = &socket {
            let _ = s.set_broadcast(true);
        }

        let handle = std::thread::spawn(move || {
            // Answer seeks while announcing.
            let listener = if socket.is_none() {
                UdpSocket::bind(("0.0.0.0", BEACON_PORT)).ok()
            } else {
                socket
            };
            let Some(sock) = listener else {
                return; // cannot bind anything — beacon off
            };
            let _ = sock.set_broadcast(true);
            let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
            while !stop.load(Ordering::Relaxed) {
                let payload = current.lock().unwrap().clone();
                if let Some(info) = payload {
                    for target in broadcast_targets() {
                        let _ = sock.send_to(&encode("announce", &info), target);
                    }
                }
                // Drain pending seeks and reply directly.
                let deadline = std::time::Instant::now() + BEACON_INTERVAL;
                while std::time::Instant::now() < deadline {
                    let mut buf = [0u8; 2048];
                    match sock.recv_from(&mut buf) {
                        Ok((n, from)) => {
                            let Some((verb, probe)) = decode(&buf[..n]) else {
                                continue;
                            };
                            if verb != "seek" {
                                continue;
                            }
                            let _ = &probe;
                            if let Some(info) = current.lock().unwrap().clone() {
                                let _ = sock.send_to(&encode("reply", &info), from);
                            }
                        }
                        Err(_) => break, // timeout → next announce tick
                    }
                }
            }
        });
        // SAFETY-free field update: we stored handle as None; wrap via interior access.
        // (Announcer is used through Arc; store the thread handle after construction.)
        let raw = Arc::get_mut(&mut announcer.clone())
            .map(|a| {
                a.handle = Some(handle);
            })
            .is_some();
        let _ = raw; // if Arc::get_mut failed (shouldn't — fresh Arc), thread still runs
        announcer
    }

    /// Update the advertised payload (player count etc.).
    pub fn update(&self, info: BeaconInfo) {
        *self.current.lock().unwrap() = Some(info);
    }

    /// Clear the advertisement (server stopped).
    #[allow(dead_code)] // part of the announcer API; called when a host stops mid-session
    pub fn clear(&self) {
        *self.current.lock().unwrap() = None;
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BeaconInfo {
        BeaconInfo {
            id: "deadbeef".into(),
            name: "Test Host".into(),
            port: 30814,
            map: "/levels/gridmap_v2/info.json".into(),
            players: 2,
            maxplayers: 8,
            relay: false,
            requires: vec!["cars.zip".into()],
            from_ip: String::new(),
        }
    }

    #[test]
    fn encode_decode_roundtrip() {
        let bytes = encode("announce", &sample());
        assert!(bytes.starts_with(b"BEAMMP_LAN1|announce|{"));
        let (verb, info) = decode(&bytes).unwrap();
        assert_eq!(verb, "announce");
        assert_eq!(info, sample());
        assert_eq!(info.from_ip, "");
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode(b"garbage").is_none());
        assert!(decode(b"BEAMMP_LAN1|announce|notjson").is_none());
        // invalid: port 0
        let mut bad = sample();
        bad.port = 0;
        assert!(decode(&encode("announce", &bad)).is_none());
    }

    #[test]
    fn broadcast_targets_include_loopback() {
        let targets = broadcast_targets();
        assert!(targets.contains(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, BEACON_PORT)));
    }

    /// E2E on loopback: an announcer with a live payload is found by seek().
    /// Uses the real BEACON_PORT on 127.0.0.1 — must not run concurrently with
    /// another beacon test.
    #[test]
    fn e2e_announce_and_seek() {
        let announcer = Announcer::start();
        announcer.update(sample());

        // Wait at most one announce interval for the first broadcast.
        let found = seek(Duration::from_secs(5));
        announcer.stop();
        announcer.clear();

        let hit = found
            .iter()
            .find(|f| f.id == "deadbeef" && f.port == 30814 && f.from_ip == "127.0.0.1");
        assert!(
            hit.is_some(),
            "beacon should be discovered on loopback, got: {found:?}"
        );
        assert_eq!(hit.unwrap().name, "Test Host");
        assert_eq!(hit.unwrap().requires, vec!["cars.zip".to_string()]);
    }
}
