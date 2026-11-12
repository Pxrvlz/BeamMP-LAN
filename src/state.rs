//! Central application state: persistent config, worker-runtime handles, event pump.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::discovery::{DiscoveredServer, InterfaceInfo, ScanConfig};
use crate::launcher_core::{self, CoreState};
use crate::net::ServerInfo;
use crate::server_mgr::{ConsoleEvent, RunState, RunningServer, ServerInstance};

/// Which nav page is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Servers,
    Host,
    Mods,
    Launcher,
    Settings,
}

/// Persistent app configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Manual ("Add server") entries: "ip:port" → label.
    pub manual_servers: Vec<ManualServer>,
    /// Favorite endpoints "ip:port".
    pub favorites: Vec<String>,
    /// Recent endpoints, most recent last.
    pub recents: Vec<String>,
    /// Last selected interface names to scan (empty = all).
    pub selected_interfaces: Vec<String>,
    /// Ports probed during discovery.
    pub scan_ports: Vec<u16>,
    /// Launcher core port (game connects here).
    pub core_port: u16,
    /// Server binary override (auto-download if empty).
    pub server_binary_override: String,
    /// Auto-refresh the LAN list every N seconds (0 = off).
    pub auto_refresh_secs: u64,
    /// Start the launcher core automatically.
    pub auto_start_core: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualServer {
    pub endpoint: String,
    pub label: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            manual_servers: Vec::new(),
            favorites: Vec::new(),
            recents: Vec::new(),
            selected_interfaces: Vec::new(),
            scan_ports: vec![crate::net::DEFAULT_SERVER_PORT],
            core_port: crate::net::LAUNCHER_CORE_PORT,
            server_binary_override: String::new(),
            auto_refresh_secs: 5,
            auto_start_core: true,
        }
    }
}

impl AppConfig {
    pub fn save_to(&self, dir: &std::path::Path) {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(dir.join("config.json"), json);
        }
    }
}

/// Where the app keeps its data (instances, cache, key).
pub fn data_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("BEAMMP_LAN_DATA") {
        return PathBuf::from(custom);
    }
    let base = config_base();
    base.join("BeamMP-LAN")
}

fn config_base() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(roaming) = std::env::var("APPDATA") {
            return PathBuf::from(roaming);
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join("Library/Application Support");
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg);
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".config");
        }
    }
    PathBuf::from(".")
}

/// Events flowing from workers → UI thread.
#[derive(Debug)]
pub enum AppEvent {
    /// LAN scan finished.
    ScanDone(Vec<DiscoveredServer>),
    /// Scan progress tick.
    ScanProgress { scanned: usize, total: usize },
    /// One server's info refreshed (auto-refresh cycle).
    ServerInfo(Box<ServerInfo>),
    /// Generic toast/status message for the UI.
    Toast(String),
    /// A download finished (mod or server binary).
    DownloadDone { what: String, path: String },
    /// A download failed.
    DownloadError { what: String, error: String },
    /// Game install detected (or not).
    GameDetected(Option<crate::launcher::GameInstall>),
}

/// Runtime state for the host page (per instance).
pub struct InstanceRuntime {
    pub state: RunState,
    pub running: Option<RunningServer>,
    pub console: Vec<String>,
    pub console_scroll_to_bottom: bool,
    pub command_input: String,
    /// Players parsed from `list` output.
    pub players: Vec<(String, u32)>,
}

impl Default for InstanceRuntime {
    fn default() -> Self {
        Self {
            state: RunState::Stopped,
            running: None,
            console: Vec::new(),
            console_scroll_to_bottom: true,
            command_input: String::new(),
            players: Vec::new(),
        }
    }
}

impl InstanceRuntime {
    pub fn push_line(&mut self, line: &str) {
        self.console.push(line.to_string());
        if self.console.len() > 2000 {
            let drop = self.console.len() - 2000;
            self.console.drain(..drop);
        }
        self.console_scroll_to_bottom = true;
        // Parse player list updates from the official `list` output (simple heuristic:
        // lines like "0 Bob" / "- 2 | Bob").
        if line.starts_with("[ID]") || line.starts_with("ID  ") {
            self.players.clear();
        }
        let t = line.trim();
        if let Some((id, name)) = t.split_once(' ') {
            if let Ok(pid) = id.parse::<u32>() {
                if !name.is_empty() && name.chars().all(|c| !c.is_control()) && name.len() < 24 {
                    let name = name.split_whitespace().next().unwrap_or(name);
                    self.players.push((name.to_string(), pid));
                }
            }
        }
    }
}

/// The whole app state, owned by the egui App.
pub struct AppState {
    pub cfg: AppConfig,
    pub page: Page,
    pub instances: Vec<ServerInstance>,
    pub runtimes: std::collections::HashMap<String, InstanceRuntime>,
    pub selected_instance: Option<usize>,

    // discovery
    pub interfaces: Vec<InterfaceInfo>,
    pub scanning: bool,
    pub scan_progress: (usize, usize),
    pub discovered: Vec<DiscoveredServer>,
    pub manual_infos: Vec<ServerInfo>,
    pub last_scan: Option<std::time::Instant>,

    // launcher
    pub game: Option<crate::launcher::GameInstall>,
    pub mod_state: Option<crate::launcher::ModState>,
    pub core: Arc<CoreState>,
    pub core_running: bool,
    pub core_logs: Vec<String>,

    // server binary
    pub server_binary: Option<PathBuf>,
    pub downloading_server: bool,

    // ui misc
    pub toasts: Vec<(String, std::time::Instant)>,
    pub event_tx: Sender<AppEvent>,
    pub event_rx: Receiver<AppEvent>,
    pub console_tx: Sender<ConsoleEvent>,
    pub console_rx: Receiver<ConsoleEvent>,
}

impl AppState {
    pub fn new() -> Self {
        let (event_tx, event_rx) = channel();
        let (console_tx, console_rx) = channel();
        let dir = data_dir();
        let cfg = load_config(&dir);
        let core = CoreState::new(dir.join("core"));

        // restore instances from disk
        let instances = load_instances(&dir);

        let mut runtimes = std::collections::HashMap::new();
        for inst in &instances {
            runtimes.insert(inst.id.clone(), InstanceRuntime::default());
        }

        Self {
            page: Page::Servers,
            interfaces: Vec::new(),
            scanning: false,
            scan_progress: (0, 0),
            discovered: Vec::new(),
            manual_infos: Vec::new(),
            last_scan: None,
            game: None,
            mod_state: None,
            core_running: false,
            core_logs: Vec::new(),
            server_binary: None,
            downloading_server: false,
            toasts: Vec::new(),
            event_tx,
            event_rx,
            console_tx,
            console_rx,
            cfg,
            instances,
            runtimes,
            selected_instance: None,
            core,
        }
    }

    pub fn save_config(&self) {
        self.cfg.save_to(&data_dir());
    }

    /// Kick off a LAN scan on a worker thread.
    pub fn start_scan(&mut self) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        self.scan_progress = (0, 0);
        let cfg = ScanConfig {
            only_interfaces: self.cfg.selected_interfaces.clone(),
            ports: self.cfg.scan_ports.clone(),
            ..Default::default()
        };
        let tx = self.event_tx.clone();
        let tx_done = tx.clone();
        std::thread::spawn(move || {
            let progress_cb = move |scanned: usize, total: usize, _found: usize| {
                let _ = tx.send(AppEvent::ScanProgress { scanned, total });
            };
            let found = crate::discovery::scan_lan(&cfg, &progress_cb);
            let _ = tx_done.send(AppEvent::ScanDone(found));
        });
    }

    /// Refresh manual servers' info in the background.
    pub fn refresh_manual(&mut self) {
        let entries: Vec<(String, String)> = self
            .cfg
            .manual_servers
            .iter()
            .map(|m| (m.endpoint.clone(), m.label.clone()))
            .collect();
        let tx = self.event_tx.clone();
        std::thread::spawn(move || {
            for (endpoint, _label) in entries {
                let addr = if endpoint.contains(':') {
                    endpoint.clone()
                } else {
                    format!("{endpoint}:{}", crate::net::DEFAULT_SERVER_PORT)
                };
                if let Some(info) = crate::net::query_server_info(&addr) {
                    let _ = tx.send(AppEvent::ServerInfo(Box::new(info)));
                }
            }
        });
    }

    /// All servers currently visible (discovered + manual, deduped by addr).
    pub fn all_servers(&self) -> Vec<ServerInfo> {
        let mut out: Vec<ServerInfo> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for d in &self.discovered {
            if seen.insert(d.info.addr.clone()) {
                out.push(d.info.clone());
            }
        }
        for i in &self.manual_infos {
            if seen.insert(i.addr.clone()) {
                out.push(i.clone());
            }
        }
        out
    }

    /// Start the embedded launcher core.
    pub fn start_core(&mut self) {
        if self.core_running {
            return;
        }
        match launcher_core::start(self.core.clone(), self.cfg.core_port) {
            Ok(()) => {
                self.core_running = true;
                self.core_logs.push("core started".into());
            }
            Err(e) => {
                self.toasts.push((
                    format!("Could not start launcher core: {e}. Is the official BeamMP launcher running?"),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    /// Push LAN servers into the core so the in-game browser sees them.
    pub fn push_server_list_to_core(&self) {
        let servers = self.all_servers();
        self.core.set_server_list(&servers);
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toasts.push((msg.into(), std::time::Instant::now()));
    }

    /// Spawn one instance's server process.
    pub fn start_instance(&mut self, idx: usize) {
        let Some(inst) = self.instances.get(idx).cloned() else {
            return;
        };
        if inst.config.auth_key.trim().is_empty() {
            self.toast(format!(
                "'{}' has no AuthKey — get one at keymaster.beammp.com and paste it in the config editor.",
                inst.name
            ));
            return;
        }
        let Some(binary) = self.resolve_server_binary() else {
            self.toast("No BeamMP-Server binary — download it on the Host page first.".to_string());
            return;
        };
        let rt = self.runtimes.entry(inst.id.clone()).or_default();
        if rt.state == RunState::Running || rt.state == RunState::Starting {
            return;
        }
        rt.state = RunState::Starting;
        rt.console.clear();
        rt.console.push(format!("Starting {} …", inst.name));

        let tx = self.console_tx.clone();
        match inst.start(&binary, tx) {
            Ok(running) => {
                let rt = self.runtimes.get_mut(&inst.id).unwrap();
                rt.state = RunState::Running;
                rt.running = Some(running);
                self.toast(format!("'{}' is starting", inst.name));
            }
            Err(e) => {
                let rt = self.runtimes.get_mut(&inst.id).unwrap();
                rt.state = RunState::Crashed;
                rt.console.push(format!("Failed to start: {e}"));
            }
        }
    }

    /// Stop one instance's server process.
    pub fn stop_instance(&mut self, idx: usize) {
        let Some(inst) = self.instances.get(idx).cloned() else {
            return;
        };
        let rt = self.runtimes.entry(inst.id.clone()).or_default();
        if let Some(mut r) = rt.running.take() {
            let _ = crate::server_mgr::stop(&mut r);
            rt.state = RunState::Stopped;
            rt.console.push("Stopped by user".into());
            // reap threads
            if let Some(t) = r.stdout_task.take() {
                let _ = t.join();
            }
            if let Some(t) = r.stderr_task.take() {
                let _ = t.join();
            }
        }
    }

    /// Resolve the BeamMP-Server binary: override → downloaded → None.
    pub fn resolve_server_binary(&self) -> Option<PathBuf> {
        if !self.cfg.server_binary_override.is_empty() {
            let p = PathBuf::from(&self.cfg.server_binary_override);
            if p.exists() {
                return Some(p);
            }
        }
        let dir = data_dir().join("server-bin");
        find_binary_in(&dir)
    }
}

/// Scan `dir` for a BeamMP-Server executable.
pub fn find_binary_in(dir: &std::path::Path) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p.clone());
            } else {
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.starts_with("beammp-server") {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Ok(md) = e.metadata() {
                            if md.permissions().mode() & 0o111 == 0 {
                                continue;
                            }
                        }
                    }
                    return Some(p);
                }
            }
        }
    }
    None
}

pub fn load_config(dir: &std::path::Path) -> AppConfig {
    let p = dir.join("config.json");
    match std::fs::read_to_string(&p) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

pub fn load_instances(dir: &std::path::Path) -> Vec<ServerInstance> {
    let base = dir.join("servers");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&base) else {
        return out;
    };
    for e in entries.flatten() {
        let dirpath = e.path();
        if !dirpath.is_dir() {
            continue;
        }
        let cfg_path = dirpath.join("ServerConfig.toml");
        if !cfg_path.exists() {
            continue;
        }
        if let Ok(config) = load_instance_config(&cfg_path) {
            let name = dirpath
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            // id is the trailing 8 hex chars we created
            let id = name
                .rsplit('-')
                .next()
                .filter(|s| s.len() == 8 && s.chars().all(|c| c.is_ascii_hexdigit()))
                .map(|s| s.to_string())
                .unwrap_or_else(|| name.clone());
            // display name from config
            let display = if config.name.is_empty() {
                name
            } else {
                config.name.clone()
            };
            out.push(ServerInstance {
                id,
                name: display,
                dir: dirpath,
                config,
                created: SystemTime::now(),
            });
        }
    }
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

fn load_instance_config(path: &std::path::Path) -> Result<crate::server_mgr::ServerConfig, String> {
    crate::server_mgr::ServerConfig::load(path).map_err(|e| e.to_string())
}

/// Save raw RGBA pixels as a PNG file (minimal encoder using flate2 for the IDAT stream).
pub fn save_png(rgba: &[u8], width: u32, height: u32, path: &str) -> std::io::Result<()> {
    use std::io::Write;

    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (n, slot) in table.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        let mut crc = 0xFFFF_FFFFu32;
        for b in data {
            crc = table[((crc ^ *b as u32) & 0xFF) as usize] ^ (crc >> 8);
        }
        crc ^ 0xFFFF_FFFF
    }

    // filter 0 rows
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0u8);
        let start = y * width as usize * 4;
        raw.extend_from_slice(&rgba[start..start + width as usize * 4]);
    }

    let mut idat = Vec::new();
    {
        let mut enc = flate2::write::ZlibEncoder::new(&mut idat, flate2::Compression::fast());
        enc.write_all(&raw)?;
        enc.finish()?;
    }

    let mut png = Vec::new();
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let chunk = |tag: &[u8; 4], data: &[u8], out: &mut Vec<u8>| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc_input = Vec::new();
        crc_input.extend_from_slice(tag);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(tag);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(b"IHDR", &ihdr, &mut png);
    chunk(b"IDAT", &idat, &mut png);
    chunk(b"IEND", &[], &mut png);

    std::fs::write(path, png)
}
