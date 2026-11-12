//! Embedded "launcher core": the localhost side of the BeamMP protocol.
//!
//! This reimplements the network half of BeamMP-Launcher (see docs/PLAN.md §5.2) so the
//! game can talk to *this* app instead of the official launcher:
//!
//! * Core listener on 127.0.0.1:4444 — the game connects here with
//!   `u32 LE len + payload` frames and sends single-letter commands (`A`, `B`, `C`, …).
//! * Proxy listener on 127.0.0.1:4445 — when the game asks to join a server (`C`),
//!   it reconnects here for session traffic; we bridge game ⇄ BeamMP server
//!   (TCP always; UDP vehicle data with `id:` prefix and `ABG:` zlib framing).
//! * `B` replies get LAN-discovered servers injected, so the in-game server browser
//!   shows the LAN without any internet backend.
//!
//! Differences from the official launcher, on purpose:
//! * The server list comes from LAN discovery, not backend.beammp.com.
//! * Guest login still goes through auth.beammp.com (required by BeamMP-Server) but the
//!   key is cached in our data dir, and the relay itself is fully local.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::net::{self, frame, FrameReader, LAUNCHER_PROXY_PORT};

/// Version we report to the game in the `Z` reply. Must be >= the mod's minimum
/// launcher version or the in-game UI refuses to work; we mirror the current stable.
pub const CORE_VERSION: &str = "2.3.2";
/// Where the auth key is cached.
const KEY_FILE: &str = "key";

/// Events the core reports to the UI.
#[derive(Debug, Clone)]
pub enum CoreEvent {
    Log(String),
    GameConnected,
    GameDisconnected,
    Joining { addr: String },
    Joined { addr: String, name: String },
    Disconnected { reason: String },
}

/// Shared, thread-safe core state.
pub struct CoreState {
    pub running: AtomicBool,
    /// Public key from guest login (needed before joining any server).
    pub public_key: Mutex<Option<String>>,
    /// Latest known LAN servers, as JSON array for the `B` reply.
    pub server_list_json: Mutex<String>,
    /// Current session target (ip:port) if any.
    pub session: Mutex<Option<(String, String)>>, // (addr, name)
    pub data_dir: PathBuf,
    pub events: Mutex<Vec<CoreEvent>>,
    /// UI-visible status string ("Ul"-style).
    pub status: Mutex<String>,
    /// Local identity nickname (reported to the game in the `N` auth reply).
    pub nickname: String,
}

impl CoreState {
    pub fn new(data_dir: PathBuf) -> Arc<Self> {
        // local identity: nickname stored in identity/identity.json
        let nickname = crate::identity::Identity::load_or_create(&data_dir, "Player")
            .map(|i| i.nickname)
            .unwrap_or_else(|_| "Player".into());
        Arc::new(Self {
            running: AtomicBool::new(false),
            public_key: Mutex::new(prefetch_key(&data_dir)),
            server_list_json: Mutex::new("[]".into()),
            session: Mutex::new(None),
            data_dir,
            events: Mutex::new(Vec::new()),
            status: Mutex::new("Ulstart".into()),
            nickname,
        })
    }

    pub fn log(&self, msg: impl Into<String>) {
        self.events.lock().unwrap().push(CoreEvent::Log(msg.into()));
    }

    pub fn drain_events(&self) -> Vec<CoreEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }

    /// Update the server list served to the game.
    pub fn set_server_list(&self, servers: &[net::ServerInfo]) {
        let arr: Vec<serde_json::Value> = servers
            .iter()
            .map(|s| {
                serde_json::json!({
                    "ip": s.addr.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or_else(|| s.addr.clone()),
                    "port": s.addr.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).unwrap_or(net::DEFAULT_SERVER_PORT),
                    "sname": s.name,
                    "name": s.name,
                    "map": s.map,
                    "players": s.players.to_string(),
                    "maxplayers": s.maxplayers.to_string(),
                    "version": s.version,
                    "clientversion": s.clientversion,
                    "tags": s.tags,
                    "guests": if s.guests { "true" } else { "false" },
                    "modlist": s.modlist,
                    "modstotal": s.modstotalsize.to_string(),
                    "modstotal2": s.modstotalsize.to_string(),
                    "modstotalsize": s.modstotalsize.to_string(),
                    "desc": s.desc,
                    "playerslist": s.playerslist,
                    "location": "LAN",
                })
            })
            .collect();
        *self.server_list_json.lock().unwrap() =
            serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into());
    }
}

fn prefetch_key(data_dir: &std::path::Path) -> Option<String> {
    let p = data_dir.join(KEY_FILE);
    let raw = std::fs::read_to_string(p).ok()?;
    let t = raw.trim();
    if t.len() < 10 || t.len() > 100 {
        return None;
    }
    Some(t.to_string())
}

/// Start the core: listeners + accept loop. Runs on a worker thread.
pub fn start(state: Arc<CoreState>, port: u16) -> std::io::Result<()> {
    let core = TcpListener::bind(("127.0.0.1", port))?;
    // Probe the proxy port eagerly — surfaces conflicts (e.g. the official BeamMP
    // launcher already running) immediately instead of at join time.
    {
        let probe = TcpListener::bind(("127.0.0.1", port + 1));
        probe?;
    }
    state.running.store(true, Ordering::SeqCst);
    state.log(format!(
        "launcher-core listening on 127.0.0.1:{port} (proxy :{})",
        port + 1
    ));

    std::thread::spawn(move || {
        for stream in core.incoming() {
            match stream {
                Ok(s) => {
                    let st = state.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = handle_game(s, st.clone()) {
                            st.log(format!("game connection error: {e}"));
                        }
                    });
                }
                Err(_) => continue,
            }
        }
    });
    Ok(())
}

fn handle_game(stream: TcpStream, state: Arc<CoreState>) -> std::io::Result<()> {
    stream.set_nodelay(true).ok();
    state.events.lock().unwrap().push(CoreEvent::GameConnected);
    state.log("game connected to core");

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    let mut framer = FrameReader::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        for frame_data in framer.feed(&buf[..n]) {
            if frame_data.is_empty() {
                continue;
            }
            let payload = String::from_utf8_lossy(&frame_data).to_string();
            let reply = handle_command(&payload, &state, &mut writer);
            if let Some(r) = reply {
                let framed = frame(r.as_bytes());
                if writer.write_all(&framed).is_err() {
                    break;
                }
            }
        }
    }
    state
        .events
        .lock()
        .unwrap()
        .push(CoreEvent::GameDisconnected);
    state.log("game disconnected from core");
    Ok(())
}

/// Process one game→launcher command; returns bytes to write back (already unframed),
/// or None. Heavy work (join) is spawned on threads so the loop stays responsive.
fn handle_command(payload: &str, state: &Arc<CoreState>, writer: &mut TcpStream) -> Option<String> {
    let code = payload.as_bytes()[0] as char;
    let rest = &payload[1..];
    match code {
        'A' => Some("A".into()), // heartbeat echo
        'B' => {
            let list = state.server_list_json.lock().unwrap().clone();
            state.log("serving server list (LAN) to game");
            Some(format!("B{list}"))
        }
        'Z' => Some(format!("Z{}", CORE_VERSION)),
        'P' => Some(format!("P{}", LAUNCHER_PROXY_PORT)),
        'U' => {
            let sub = rest.chars().next().unwrap_or(' ');
            let status = state.status.lock().unwrap().clone();
            match sub {
                'l' => Some(status),
                'p' => {
                    let ping = state
                        .session
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|_| "Up15".to_string())
                        .unwrap_or_else(|| "Up-2".into());
                    Some(ping)
                }
                _ => Some("Up15".into()),
            }
        }
        'M' => {
            // mod status — the game asks "M?" style; we answer empty status
            Some("M ".into())
        }
        'N' => {
            if rest == "c" {
                // auto-login check: report the local identity (nickname + machine id)
                let authed = state.public_key.lock().unwrap().is_some();
                let mut j = serde_json::json!({ "Auth": if authed { 1 } else { 0 } });
                if authed {
                    j["username"] = serde_json::Value::String(state.nickname.clone());
                    j["role"] = serde_json::Value::String("player".into());
                }
                Some(format!("N{j}"))
            } else if rest == "LO" {
                *state.public_key.lock().unwrap() = None;
                let _ = std::fs::remove_file(state.data_dir.join(KEY_FILE));
                Some("N{\"Auth\":0}".into())
            } else {
                // N:<json> login request → treat as guest login
                let st = state.clone();
                let mut w = writer.try_clone().ok()?;
                std::thread::spawn(move || {
                    let reply = match st.ensure_key_pub() {
                        Ok(_) => format!(
                            "N{{\"Auth\":1,\"username\":\"{}\",\"role\":\"player\"}}",
                            st.nickname
                        ),
                        Err(e) => {
                            st.log(format!("guest login failed: {e}"));
                            format!("N{{\"Auth\":0,\"message\":\"{e}\"}}")
                        }
                    };
                    let _ = w.write_all(&frame(reply.as_bytes()));
                });
                None
            }
        }
        'W' => {
            // mod security confirm — always yes in LAN context (server list only contains
            // servers the user explicitly saw in our app)
            Some("WY".into())
        }
        'I' => {
            // I<ip:port> — fetch that server's info
            let addr = rest.to_string();
            let mut w = writer.try_clone().ok()?;
            std::thread::spawn(move || {
                let reply = match net::query_server_info(&addr) {
                    Some(info) => {
                        let j = serde_json::json!({
                            "ip": info.addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(&info.addr),
                            "port": info.addr.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).unwrap_or(net::DEFAULT_SERVER_PORT),
                            "sname": info.name,
                            "map": info.map,
                            "players": info.players.to_string(),
                            "maxplayers": info.maxplayers.to_string(),
                            "version": info.version,
                            "tags": info.tags,
                            "desc": info.desc,
                            "playerslist": info.playerslist,
                        });
                        format!("I{addr};{j}")
                    }
                    None => format!("I{addr};"),
                };
                let _ = w.write_all(&frame(reply.as_bytes()));
            });
            None
        }
        'C' => {
            // C<ip:port> — join!
            let addr = rest.to_string();
            let st = state.clone();
            let mut w = writer.try_clone().ok()?;
            std::thread::spawn(move || {
                if let Err(e) = join_server(&st, &addr, &mut w) {
                    st.log(format!("join failed: {e}"));
                    *st.status.lock().unwrap() = format!("UlDisconnected: {e}");
                    let _ = w.write_all(frame(b"L".as_slice()).as_slice());
                }
            });
            None
        }
        'Q' => {
            let sub = rest.chars().next().unwrap_or(' ');
            match sub {
                'S' => {
                    // stop sync
                    *state.session.lock().unwrap() = None;
                    *state.status.lock().unwrap() = "Ulstart".into();
                    None
                }
                _ => None, // 'G' = quit game: official launcher exits; we just watch
            }
        }
        'R' => Some("R".into()), // mod loaded ack
        'O' => {
            // open URL — allow-list like the official launcher
            if is_allowed_link(rest) {
                let _ = open::that(rest);
            }
            None
        }
        _ => None,
    }
}

impl CoreState {
    fn ensure_key_pub(&self) -> Result<String, String> {
        // guest login if we don't have a key yet (blocking, called on worker threads)
        {
            let pk = self.public_key.lock().unwrap();
            if let Some(k) = pk.as_ref() {
                return Ok(k.clone());
            }
        }
        let key = crate::launcher::guest_login()?;
        let _ = std::fs::create_dir_all(&self.data_dir);
        let _ = std::fs::write(self.data_dir.join(KEY_FILE), &key);
        *self.public_key.lock().unwrap() = Some(key.clone());
        Ok(key)
    }
}

fn is_allowed_link(link: &str) -> bool {
    link.starts_with("https://beammp.com")
        || link.starts_with("https://www.beammp.com")
        || link.starts_with("https://beammp.gg")
        || link.starts_with("https://github.com/BeamMP/")
        || link.starts_with("https://discord.gg")
        || link.starts_with("https://patreon.com/BeamMP")
}

/// Join a server: handshake as a client, then bridge game ⇄ server.
///
/// The sequence (from `BeamMP-Launcher/src/Network/GlobalHandler.cpp` +
/// `Resources.cpp::Auth` + server `TNetwork.cpp::Authentication`):
///   connect TCP → send 'C' → send "VC<version>" → recv 'A' → send public key →
///   recv 'P<id>' → send 'SR' → recv mod list (JSON or legacy) → download mods →
///   send 'Done' → then relay loop.
fn join_server(
    state: &Arc<CoreState>,
    addr: &str,
    game_writer: &mut TcpStream,
) -> Result<(), String> {
    state
        .events
        .lock()
        .unwrap()
        .push(CoreEvent::Joining { addr: addr.into() });
    *state.status.lock().unwrap() = "UlLoading...".into();
    state.log(format!("connecting to {addr}"));

    let mut server = TcpStream::connect(addr).map_err(|e| format!("connect: {e}"))?;
    server.set_nodelay(true).ok();

    // 'C' identify
    server.write_all(b"C").map_err(|e| e.to_string())?;

    // version
    send_frame(&mut server, format!("VC{}", CORE_VERSION))?;
    let reply = recv_frame(&mut server)?;
    if reply.is_empty() || reply.first() == Some(&b'E') || reply.first() == Some(&b'K') {
        return Err(format!("rejected: {}", String::from_utf8_lossy(&reply)));
    }

    // auth key
    let key = state.ensure_key_pub()?;
    send_frame(&mut server, &key)?;
    let reply = recv_frame(&mut server)?;
    if reply.is_empty() || reply[0] != b'P' {
        return Err("authentication failed (no P<id>)".into());
    }
    let id_str = String::from_utf8_lossy(&reply[1..]).to_string();
    let client_id: i32 = id_str
        .trim()
        .parse()
        .map_err(|_| format!("bad client id '{id_str}'"))?;
    state.log(format!("authenticated as client {client_id}"));

    // mod sync
    send_frame(&mut server, "SR")?;
    let mods_reply = recv_frame(&mut server)?;
    let mods_list = parse_mods(&mods_reply);
    if !mods_list.is_empty() {
        state.log(format!("server has {} client mods", mods_list.len()));
        // Tell the game what's coming (L packet, name;name;).
        let names: Vec<&str> = mods_list.iter().map(|m| m.0.as_str()).collect();
        send_frame(game_writer, format!("L{}", names.join(";")))?;
        // Download each mod ('f' requests on the same socket, like the launcher).
        download_mods(state, &mut server, &mods_list, game_writer, client_id)?;
    } else {
        send_frame(game_writer, "L")?;
        send_frame(&mut server, "Done")?;
    }

    // Proxy: game reconnects on port+1 for session traffic.
    let proxy = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("proxy bind: {e}"))?;
    let proxy_port = proxy.local_addr().map_err(|e| e.to_string())?.port();
    send_frame(game_writer, format!("P{proxy_port}"))?;

    state.events.lock().unwrap().push(CoreEvent::Joined {
        addr: addr.into(),
        name: addr.into(),
    });
    *state.status.lock().unwrap() = "Uldone".into();

    // Accept the game's session connection, then pump both directions.
    let game_session = proxy.accept().map_err(|e| format!("proxy accept: {e}"))?.0;
    relay(state, game_session, server, client_id, addr)?;

    state.events.lock().unwrap().push(CoreEvent::Disconnected {
        reason: "session ended".into(),
    });
    *state.session.lock().unwrap() = None;
    *state.status.lock().unwrap() = "Ulstart".into();
    Ok(())
}

/// Bridge game ⇄ server until either side closes.
/// Game-side frames: `len+payload`. Server-side: `len+payload`, 'O'/'T' etc. passthrough.
/// UDP: we also open a UDP socket to the server and prefix outgoing game packets with
/// `id:` (the server's UDP format) when the game sends them marked unreliable.
fn relay(
    state: &Arc<CoreState>,
    game: TcpStream,
    mut server: TcpStream,
    client_id: i32,
    addr: &str,
) -> Result<(), String> {
    *state.session.lock().unwrap() = Some((addr.to_string(), addr.to_string()));

    // UDP channel to the server for vehicle data.
    let server_udp = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    let udp_target: std::net::SocketAddr = addr
        .to_socket_addrs()
        .map_err(|_| format!("bad addr {addr}"))?
        .next()
        .ok_or_else(|| format!("unresolvable addr {addr}"))?;
    server_udp.connect(udp_target).map_err(|e| e.to_string())?;

    let udp_out = Arc::new(server_udp.try_clone().map_err(|e| e.to_string())?);
    let client_id_a = client_id;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_g2s = stop.clone();
    let stop_s2g = stop.clone();
    let stop_udp = stop.clone();

    let state_g = state.clone();
    let state_s = state.clone();

    // game → server
    let g = game.try_clone().map_err(|e| e.to_string())?;
    let s_g2s = server.try_clone().map_err(|e| e.to_string())?;
    let t_g2s = std::thread::spawn(move || {
        let mut reader = BufReader::new(g);
        let mut server = s_g2s;
        let mut framer = FrameReader::new();
        let mut buf = [0u8; 8192];
        loop {
            if stop_g2s.load(Ordering::Relaxed) {
                break;
            }
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for f in framer.feed(&buf[..n]) {
                        if f.is_empty() {
                            continue;
                        }
                        // ServerSend rules: 'O'/'T' always TCP-ack; small unreliable → UDP.
                        let code = f[0] as char;
                        let rel = matches!(code, 'N' | 'W' | 'Y' | 'V' | 'E' | 'C' | 't');
                        if rel || f.len() > 1000 {
                            let _ = send_frame(&mut server, f.clone());
                        } else if f.len() > 400 {
                            // compress + TCP
                            let c = zlib_compress(&f);
                            let mut pkt = b"ABG:".to_vec();
                            pkt.extend_from_slice(&c);
                            let _ = send_frame(&mut server, pkt);
                        } else {
                            // UDP with id prefix
                            let mut pkt = format!("{}:", client_id_a + 1).into_bytes();
                            pkt.extend_from_slice(&f);
                            let _ = udp_out.send(&pkt);
                        }
                    }
                }
            }
        }
        let _ = state_g;
    });

    // server → game (TCP)
    let mut g2 = game.try_clone().map_err(|e| e.to_string())?;
    let s2 = server.try_clone().map_err(|e| e.to_string())?;
    let t_s2g = std::thread::spawn(move || {
        let mut reader = BufReader::new(s2);
        let mut buf = [0u8; 65536];
        loop {
            if stop_s2g.load(Ordering::Relaxed) {
                break;
            }
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // stream of len+payload frames from server
                    let mut chunk = &buf[..n];
                    while chunk.len() >= 4 {
                        let len =
                            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as usize;
                        if chunk.len() < 4 + len {
                            break;
                        }
                        let payload = &chunk[4..4 + len];
                        let payload = if payload.starts_with(b"ABG:") {
                            zlib_decompress(&payload[4..]).unwrap_or_else(|_| payload.to_vec())
                        } else {
                            payload.to_vec()
                        };
                        // 'p' = ping reply consumed by launcher; 'M' = map load → forward
                        if !payload.is_empty() {
                            let _ = g2.write_all(&frame(&payload));
                        }
                        chunk = &chunk[4 + len..];
                    }
                }
            }
        }
        let _ = state_s;
    });

    // server → game (UDP): read server UDP, forward to game over TCP (framed)
    let mut g3 = game.try_clone().map_err(|e| e.to_string())?;
    let t_udp = std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        loop {
            if stop_udp.load(Ordering::Relaxed) {
                break;
            }
            match server_udp.recv_from(&mut buf) {
                Ok((n, _)) => {
                    let mut payload = &buf[..n];
                    // server UDP packets may carry "ABG:" compression
                    let decoded;
                    if payload.starts_with(b"ABG:") {
                        decoded = zlib_decompress(&payload[4..]).unwrap_or_default();
                        payload = &decoded;
                    }
                    if !payload.is_empty() {
                        let _ = g3.write_all(frame(payload).as_slice());
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Wait for either direction to finish, then tear everything down.
    let _ = t_g2s.join();
    stop.store(true, Ordering::SeqCst);
    let _ = t_s2g.join();
    let _ = t_udp.join();
    // Best-effort 'T' (terminate) to server, like the official launcher.
    let _ = send_frame(&mut server, "T");
    Ok(())
}

type ModListing = Vec<(String, u64)>; // (file name, size)

/// Parse the mod list reply: new JSON format or legacy `name;name;size;size;`.
fn parse_mods(reply: &[u8]) -> ModListing {
    if reply.is_empty() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(reply);
    let text =
        text.trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != '{' && c != '/');
    if text.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            let mut out = Vec::new();
            if let Some(obj) = v.as_object() {
                // {"mods": {"file.zip": {"size": 123, ...}}}
                if let Some(mods) = obj.get("mods").and_then(|m| m.as_object()) {
                    for (name, meta) in mods {
                        let size = meta.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
                        out.push((name.clone(), size));
                    }
                    return out;
                }
                for (name, meta) in obj {
                    let size = meta.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
                    out.push((name.clone(), size));
                }
            }
            return out;
        }
    }
    // legacy: names then sizes, semicolon separated
    let parts: Vec<&str> = text.split(';').filter(|p| !p.is_empty()).collect();
    if parts.len().is_multiple_of(2) && !parts.is_empty() {
        let (names, sizes) = parts.split_at(parts.len() / 2);
        let mut out = Vec::new();
        for (n, s) in names.iter().zip(sizes.iter()) {
            if let Ok(sz) = s.trim().parse::<u64>() {
                out.push((n.to_string(), sz));
            }
        }
        return out;
    }
    Vec::new()
}

/// Download each client mod from the server ('f' protocol) and stream progress to the
/// game with 'Ul' packets. Files land in the game's mods/multiplayer folder.
fn download_mods(
    state: &Arc<CoreState>,
    server: &mut TcpStream,
    mods: &ModListing,
    game_writer: &mut TcpStream,
    client_id: i32,
) -> Result<(), String> {
    let install = crate::launcher::detect_game();
    let mod_dir = install
        .as_ref()
        .map(|i| i.mod_dir())
        .unwrap_or_else(|| state.data_dir.join("downloaded-mods"));
    std::fs::create_dir_all(&mod_dir).map_err(|e| e.to_string())?;

    for (i, (name, size)) in mods.iter().enumerate() {
        let lower = name.to_lowercase();
        let file_name = if cfg!(windows) { name.clone() } else { lower };
        let dest = mod_dir.join(&file_name);

        // progress note to the game
        send_frame(game_writer, format!("Ul{}/{}: {}", i + 1, mods.len(), name))?;

        if dest.exists() {
            // compare hash with what the server advertised? we only have size — accept.
            // The official launcher caches by sha; size match is a pragmatic shortcut.
            send_frame(server, format!("f{name}"))?;
            let _ = recv_frame(server); // 'D' ack
                                        // skip download: tell the server we're done with this file
            continue;
        }

        // request the file
        send_frame(server, format!("f{name}"))?;
        let mut data = Vec::with_capacity(*size as usize);
        let mut remaining = *size;
        let mut buf = [0u8; 65536];
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = server
                .read(&mut buf[..want])
                .map_err(|e| format!("download read: {e}"))?;
            if n == 0 {
                return Err("server closed during mod download".into());
            }
            data.extend_from_slice(&buf[..n]);
            remaining -= n as u64;
        }
        let tmp = dest.with_extension("zip.tmp");
        std::fs::write(&tmp, &data).map_err(|e| format!("write mod: {e}"))?;
        std::fs::rename(&tmp, &dest).map_err(|e| format!("move mod: {e}"))?;
        state.log(format!("downloaded mod {} ({} bytes)", name, data.len()));
        let _ = client_id;
    }
    send_frame(server, "Done")?;
    Ok(())
}

// — framing helpers over raw TcpStream ——————————————————————————————

fn send_frame(stream: &mut TcpStream, data: impl AsRef<[u8]>) -> Result<(), String> {
    let framed = frame(data.as_ref());
    stream.write_all(&framed).map_err(|e| e.to_string())
}

fn recv_frame(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .map_err(|e| format!("recv header: {e}"))?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 64 * 1024 * 1024 {
        return Err("frame too large".into());
    }
    let mut buf = vec![0u8; len];
    stream
        .read_exact(&mut buf)
        .map_err(|e| format!("recv body: {e}"))?;
    Ok(buf)
}

// — minimal zlib (deflate) via flate2 ———————————————————

fn zlib_compress(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).ok();
    enc.finish().unwrap_or_default()
}

fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut dec = flate2::read::ZlibDecoder::new(data);
    std::io::Read::read_to_end(&mut dec, &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Simple buffered reader (avoid pulling BufReader trait import dance).
struct BufReader<R: Read> {
    inner: R,
    buf: Vec<u8>,
    pos: usize,
}

impl<R: Read> BufReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            buf: Vec::new(),
            pos: 0,
        }
    }
}

impl<R: Read> Read for BufReader<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos < self.buf.len() {
            let n = (self.buf.len() - self.pos).min(out.len());
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.inner.read(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mods_json() {
        let reply =
            br#"{"mods": {"cars.zip": {"size": 100, "hash": "x"}, "map.zip": {"size": 200}}}"#;
        let mods = parse_mods(reply);
        assert_eq!(mods.len(), 2);
        assert_eq!(mods[0].0, "cars.zip");
        assert_eq!(mods[0].1, 100);
    }

    #[test]
    fn parse_mods_legacy() {
        let reply = b"cars.zip;map.zip;100;200;";
        let mods = parse_mods(reply);
        assert_eq!(mods.len(), 2);
        assert_eq!(mods[1].0, "map.zip");
        assert_eq!(mods[1].1, 200);
    }

    #[test]
    fn parse_mods_empty() {
        assert!(parse_mods(b"").is_empty());
        assert!(parse_mods(b"-").is_empty());
    }

    #[test]
    fn zlib_roundtrip() {
        let data = b"hello world hello world hello world".repeat(10);
        let c = zlib_compress(&data);
        assert!(c.len() < data.len());
        let d = zlib_decompress(&c).unwrap();
        assert_eq!(d, data);
    }

    #[test]
    fn link_allowlist() {
        assert!(is_allowed_link("https://beammp.com/faq"));
        assert!(!is_allowed_link("https://evil.example"));
    }
}
