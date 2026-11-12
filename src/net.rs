//! BeamMP wire protocol helpers.
//!
//! Everything in this module was derived from reading the BeamMP-Server and
//! BeamMP-Launcher sources (see docs/PLAN.md §5):
//!
//! * Game ⇄ launcher framing on 127.0.0.1:4444: `u32 LE length` header + payload.
//! * Server info packet: TCP connect, send `'I'`, read `i32 LE length` + JSON body.
//! * Ping: send `'P'`, server replies `'P'`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Default BeamMP server port (server `Settings.cpp`).
pub const DEFAULT_SERVER_PORT: u16 = 30814;
/// Default launcher core port (launcher `Options.h`).
pub const LAUNCHER_CORE_PORT: u16 = 4444;
/// Launcher proxy port = core port + 1 (launcher `GlobalHandler.cpp::SetupListener`).
pub const LAUNCHER_PROXY_PORT: u16 = 4445;
/// How long to wait for a reply on the info/ping channel.
pub const INFO_TIMEOUT: Duration = Duration::from_millis(1200);

/// Read exactly `n` bytes from the stream.
pub fn read_exact_stream(stream: &mut TcpStream, n: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

/// Query a BeamMP server's information packet: connect, send 'I', read JSON.
///
/// Returns `None` if the server is unreachable, not a BeamMP server, or has
/// `InformationPacket = false` (empty reply).
pub fn query_server_info(addr: &str) -> Option<ServerInfo> {
    let mut stream = TcpStream::connect(addr).ok()?;
    stream.set_read_timeout(Some(INFO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(INFO_TIMEOUT)).ok()?;
    stream.write_all(b"I").ok()?;

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).ok()?;
    let len = i32::from_le_bytes(len_buf);
    if len <= 0 || len > 8 * 1024 * 1024 {
        return None;
    }
    let body = read_exact_stream(&mut stream, len as usize).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&body).ok()?;
    ServerInfo::from_json(&json, addr)
}

/// Measure round-trip to a BeamMP server using the 'P' ping code.
pub fn ping_server(addr: &str) -> Option<Duration> {
    let start = std::time::Instant::now();
    let mut stream = TcpStream::connect(addr).ok()?;
    stream.set_read_timeout(Some(INFO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(INFO_TIMEOUT)).ok()?;
    stream.write_all(b"P").ok()?;
    let mut buf = [0u8; 1];
    stream.read_exact(&mut buf).ok()?;
    if buf[0] == b'P' {
        Some(start.elapsed())
    } else {
        None
    }
}

/// Parsed server info (fields from `THeartbeatThread::GenerateCall`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServerInfo {
    pub addr: String,
    pub name: String,
    pub map: String,
    pub players: u32,
    pub maxplayers: u32,
    pub version: String,
    pub clientversion: String,
    pub tags: String,
    pub guests: bool,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub modlist: String,
    #[serde(default)]
    pub modstotal: u32,
    #[serde(default)]
    pub modstotalsize: u64,
    #[serde(default)]
    pub playerslist: Vec<String>,
}

impl ServerInfo {
    /// Build from the heartbeat JSON, tolerating string-typed numbers (the C++ server
    /// stringifies several fields) and missing keys.
    pub fn from_json(v: &serde_json::Value, addr: &str) -> Option<Self> {
        // The server sends some fields as strings ("8"), some as numbers.
        fn num(v: &serde_json::Value, key: &str) -> u32 {
            match v.get(key) {
                Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0) as u32,
                Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(0),
                _ => 0,
            }
        }
        fn num64(v: &serde_json::Value, key: &str) -> u64 {
            match v.get(key) {
                Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
                Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(0),
                _ => 0,
            }
        }
        fn string(v: &serde_json::Value, key: &str, default: &str) -> String {
            match v.get(key) {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Number(n)) => n.to_string(),
                _ => default.to_string(),
            }
        }
        fn boolean(v: &serde_json::Value, key: &str, default: bool) -> bool {
            match v.get(key) {
                Some(serde_json::Value::Bool(b)) => *b,
                Some(serde_json::Value::String(s)) => s == "true" || s == "1",
                _ => default,
            }
        }
        let playerslist = match v.get("playerslist") {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .map(|p| p.as_str().unwrap_or_default().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            Some(serde_json::Value::String(s)) => s
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        Some(ServerInfo {
            addr: addr.to_string(),
            name: string(v, "name", "BeamMP Server"),
            map: string(v, "map", ""),
            players: num(v, "players"),
            maxplayers: num(v, "maxplayers"),
            version: string(v, "version", ""),
            clientversion: string(v, "clientversion", ""),
            tags: string(v, "tags", ""),
            guests: boolean(v, "guests", true),
            desc: string(v, "desc", ""),
            modlist: string(v, "modlist", ""),
            modstotal: num(v, "modstotal"),
            modstotalsize: num64(v, "modstotalsize"),
            playerslist,
        })
    }
}

/// Frame a payload with the u32 LE length header used on the game ⇄ launcher link.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Incremental reader for length-prefixed frames over a `TcpStream`.
pub struct FrameReader {
    buf: Vec<u8>,
}

impl Default for FrameReader {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameReader {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Feed raw bytes; returns every complete frame that became available.
    pub fn feed(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(data);
        let mut frames = Vec::new();
        loop {
            if self.buf.len() < 4 {
                break;
            }
            let len =
                u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
            if len > 64 * 1024 * 1024 {
                // Corrupt stream — drop everything to resync.
                self.buf.clear();
                break;
            }
            if self.buf.len() < 4 + len {
                break;
            }
            let frame: Vec<u8> = self.buf[4..4 + len].to_vec();
            self.buf.drain(..4 + len);
            frames.push(frame);
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_roundtrip() {
        let mut r = FrameReader::new();
        let framed = frame(b"hello");
        assert_eq!(framed.len(), 9);
        // feed in two chunks
        let mut out = r.feed(&framed[..3]);
        assert!(out.is_empty());
        out.extend(r.feed(&framed[3..]));
        assert_eq!(out, vec![b"hello".to_vec()]);
    }

    #[test]
    fn framing_multiple() {
        let mut r = FrameReader::new();
        let mut stream = frame(b"a");
        stream.extend(frame(b"bb"));
        let out = r.feed(&stream);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], b"a".to_vec());
        assert_eq!(out[1], b"bb".to_vec());
    }

    #[test]
    fn server_info_parses_string_numbers() {
        let v: serde_json::Value = serde_json::json!({
            "players": "3",
            "maxplayers": 8,
            "name": "LAN Party",
            "map": "/levels/gridmap_v2/info.json",
            "playerslist": ["alice", "bob"],
        });
        let info = ServerInfo::from_json(&v, "10.1.2.3:30814").unwrap();
        assert_eq!(info.players, 3);
        assert_eq!(info.maxplayers, 8);
        assert_eq!(info.playerslist, vec!["alice", "bob"]);
    }

    /// E2E: driven by scripts/e2e_protocol_test.py which hosts a fake BeamMP server
    /// on 127.0.0.1:31814 implementing the 'I'/'P' protocol from TNetwork.cpp.
    #[test]
    #[ignore]
    fn e2e_info_protocol() {
        let addr = "127.0.0.1:31814";
        let info = query_server_info(addr).expect("server should answer the 'I' packet");
        assert_eq!(info.name, "E2E Test Server");
        assert_eq!(info.players, 2);
        assert_eq!(info.maxplayers, 8);
        assert_eq!(info.map, "/levels/gridmap_v2/info.json");
        assert_eq!(info.version, "3.4.1");
        assert_eq!(info.modstotal, 2);
        assert_eq!(info.playerslist, vec!["alice", "bob"]);
        let ping = ping_server(addr).expect("'P' ping should be answered");
        assert!(ping.as_millis() < 1000);
    }

    #[test]
    fn pretty_map_works() {
        assert_eq!(
            crate::theme::pretty_map("/levels/gridmap_v2/info.json"),
            "Gridmap V2"
        );
        assert_eq!(
            crate::theme::pretty_map("/levels/west_coast_usa/info.json"),
            "West Coast Usa"
        );
    }
}
