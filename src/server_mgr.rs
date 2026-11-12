//! Server manager: BeamMP-Server instances, typed ServerConfig.toml, process control,
//! live console.
//!
//! Config keys & defaults mirror `BeamMP-Server/src/Settings.cpp` exactly.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Typed `[General]` + `[Misc]` of BeamMP-Server's ServerConfig.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    // [General]
    pub auth_key: String,
    pub log_chat: bool,
    pub debug: bool,
    pub private: bool,
    pub information_packet: bool,
    pub allow_guests: bool,
    pub ip: String,
    pub port: u16,
    pub name: String,
    pub tags: String,
    pub max_cars: u32,
    pub max_players: u32,
    pub map: String,
    pub description: String,
    pub resource_folder: String,
    // [Misc]
    pub im_scared_of_updates: bool,
    pub update_reminder_time: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            auth_key: String::new(),
            log_chat: true,
            debug: false,
            private: true,
            information_packet: true,
            allow_guests: true,
            ip: "::".into(),
            port: crate::net::DEFAULT_SERVER_PORT,
            name: "BeamMP Server".into(),
            tags: "Freeroam,LAN".into(),
            max_cars: 4,
            max_players: 8,
            map: "/levels/gridmap_v2/info.json".into(),
            description: "BeamMP Default Description".into(),
            resource_folder: "Resources".into(),
            im_scared_of_updates: true,
            update_reminder_time: "30s".into(),
        }
    }
}

impl ServerConfig {
    /// Parse from a ServerConfig.toml file (missing keys fall back to defaults).
    pub fn load(path: &Path) -> anyhow_result::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        Self::from_toml_str(&raw)
    }

    pub fn from_toml_str(raw: &str) -> anyhow_result::Result<Self> {
        let v: toml::Value = toml::from_str(raw)?;
        let g = v
            .get("General")
            .cloned()
            .unwrap_or(toml::Value::Table(Default::default()));
        let m = v
            .get("Misc")
            .cloned()
            .unwrap_or(toml::Value::Table(Default::default()));
        let get_str = |t: &toml::Value, k: &str, d: &str| -> String {
            t.get(k).and_then(|x| x.as_str()).unwrap_or(d).to_string()
        };
        let get_bool = |t: &toml::Value, k: &str, d: bool| -> bool {
            match t.get(k) {
                Some(toml::Value::Boolean(b)) => *b,
                _ => d,
            }
        };
        let get_int = |t: &toml::Value, k: &str, d: u32| -> u32 {
            t.get(k)
                .and_then(|x| x.as_integer())
                .map(|x| x as u32)
                .unwrap_or(d)
        };
        Ok(ServerConfig {
            auth_key: get_str(&g, "AuthKey", ""),
            log_chat: get_bool(&g, "LogChat", true),
            debug: get_bool(&g, "Debug", false),
            private: get_bool(&g, "Private", true),
            information_packet: get_bool(&g, "InformationPacket", true),
            allow_guests: get_bool(&g, "AllowGuests", true),
            ip: get_str(&g, "IP", "::"),
            port: get_int(&g, "Port", crate::net::DEFAULT_SERVER_PORT as u32) as u16,
            name: get_str(&g, "Name", "BeamMP Server"),
            tags: get_str(&g, "Tags", "Freeroam"),
            max_cars: get_int(&g, "MaxCars", 1),
            max_players: get_int(&g, "MaxPlayers", 8),
            map: get_str(&g, "Map", "/levels/gridmap_v2/info.json"),
            description: get_str(&g, "Description", "BeamMP Default Description"),
            resource_folder: get_str(&g, "ResourceFolder", "Resources"),
            im_scared_of_updates: get_bool(&m, "ImScaredOfUpdates", true),
            update_reminder_time: get_str(&m, "UpdateReminderTime", "30s"),
        })
    }

    /// Render as ServerConfig.toml, matching the C++ writer's key order & comments.
    pub fn to_toml_string(&self) -> String {
        let mut s = String::new();
        s.push_str("# This is the BeamMP-Server config file.\n");
        s.push_str("# Managed by BeamMP-LAN.\n");
        s.push_str("# IMPORTANT: Fill in the AuthKey with the key you got from https://keymaster.beammp.com/ on the left under \"Keys\"\n\n");
        s.push_str("[General]\n");
        s.push_str(&format!("AuthKey = \"{}\"\n", escape_toml(&self.auth_key)));
        s.push_str("# Whether to log chat messages in the console / log\n");
        s.push_str(&format!("LogChat = {}\n", self.log_chat));
        s.push_str(&format!("Debug = {}\n", self.debug));
        s.push_str(&format!("Private = {}\n", self.private));
        s.push_str("# Whether to allow unconnected clients to get the public server information without joining\n");
        s.push_str(&format!(
            "InformationPacket = {}\n",
            self.information_packet
        ));
        s.push_str("# Whether to allow guests\n");
        s.push_str(&format!("AllowGuests = {}\n", self.allow_guests));
        s.push_str(
            "# The IP address to bind the server to, this is NOT related to your public IP.\n",
        );
        s.push_str(&format!("IP = \"{}\"\n", escape_toml(&self.ip)));
        s.push_str(&format!("Port = {}\n", self.port));
        s.push_str(&format!("Name = \"{}\"\n", escape_toml(&self.name)));
        s.push_str("# Add custom identifying tags. Format: TagA,TagB,TagC\n");
        s.push_str(&format!("Tags = \"{}\"\n", escape_toml(&self.tags)));
        s.push_str(&format!("MaxCars = {}\n", self.max_cars));
        s.push_str(&format!("MaxPlayers = {}\n", self.max_players));
        s.push_str(&format!("Map = \"{}\"\n", escape_toml(&self.map)));
        s.push_str(&format!(
            "Description = \"{}\"\n",
            escape_toml(&self.description)
        ));
        s.push_str(&format!(
            "ResourceFolder = \"{}\"\n",
            escape_toml(&self.resource_folder)
        ));
        s.push_str("\n[Misc]\n");
        s.push_str(&format!(
            "ImScaredOfUpdates = {}\n",
            self.im_scared_of_updates
        ));
        s.push_str("# Time between update reminders (s, min, h, d)\n");
        s.push_str(&format!(
            "UpdateReminderTime = \"{}\"\n",
            escape_toml(&self.update_reminder_time)
        ));
        s
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.to_toml_string())
    }
}

fn escape_toml(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Minimal error alias (this crate avoids anyhow to keep deps light).
pub mod anyhow_result {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
}

/// One managed server instance (a folder under `<data>/servers/<name>/`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInstance {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    #[serde(flatten)]
    pub config: ServerConfig,
    #[serde(default = "now_marker")]
    pub created: SystemTime,
}

fn now_marker() -> SystemTime {
    SystemTime::now()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Stopped,
    Starting,
    Running,
    Crashed,
}

impl RunState {
    pub fn label(&self) -> &'static str {
        match self {
            RunState::Stopped => "STOPPED",
            RunState::Starting => "STARTING",
            RunState::Running => "RUNNING",
            RunState::Crashed => "CRASHED",
        }
    }
}

/// A console event forwarded to the UI.
#[derive(Debug, Clone)]
pub enum ConsoleEvent {
    Line(String),
}

/// Runtime handle for one running server process.
pub struct RunningServer {
    pub child: Child,
    pub stdout_task: Option<std::thread::JoinHandle<()>>,
    pub stderr_task: Option<std::thread::JoinHandle<()>>,
}

impl ServerInstance {
    pub fn config_path(&self) -> PathBuf {
        self.dir.join("ServerConfig.toml")
    }

    pub fn resources_dir(&self) -> PathBuf {
        self.dir.join(&self.config.resource_folder)
    }

    pub fn client_mods_dir(&self) -> PathBuf {
        self.resources_dir().join("Client")
    }

    pub fn server_mods_dir(&self) -> PathBuf {
        self.resources_dir().join("Server")
    }

    /// Create the instance folder structure + default config.
    pub fn create(base: &Path, name: &str, config: ServerConfig) -> std::io::Result<Self> {
        let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let dir = base.join(format!("{}-{}", sanitize(name), id));
        std::fs::create_dir_all(dir.join("Resources/Client"))?;
        std::fs::create_dir_all(dir.join("Resources/Server"))?;
        let inst = ServerInstance {
            id,
            name: name.to_string(),
            dir,
            config,
            created: SystemTime::now(),
        };
        inst.config.save(&inst.config_path())?;
        Ok(inst)
    }

    /// Write config changes to disk.
    pub fn save_config(&self) -> std::io::Result<()> {
        self.config.save(&self.config_path())
    }

    /// Spawn BeamMP-Server for this instance with piped stdio.
    pub fn start(
        &self,
        binary: &Path,
        event_tx: Sender<ConsoleEvent>,
    ) -> std::io::Result<RunningServer> {
        // Ensure resource dirs exist (the server expects them; it creates Client itself).
        std::fs::create_dir_all(self.client_mods_dir()).ok();
        std::fs::create_dir_all(self.server_mods_dir()).ok();
        self.save_config()?;

        let mut cmd = Command::new(binary);
        cmd.current_dir(&self.dir)
            .arg(format!("--config={}", self.config_path().display()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
        }

        let mut child = cmd.spawn()?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let txo = event_tx.clone();
        let stdout_task = stdout.map(|s| {
            std::thread::spawn(move || {
                let reader = BufReader::new(s);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            if txo.send(ConsoleEvent::Line(l)).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
        });
        let txe = event_tx;
        let stderr_task = stderr.map(|s| {
            std::thread::spawn(move || {
                let reader = BufReader::new(s);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            if txe.send(ConsoleEvent::Line(l)).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
        });

        Ok(RunningServer {
            child,
            stdout_task,
            stderr_task,
        })
    }
}

/// Stop a running server: try graceful `exit` via stdin first? The official server reads
/// commands from a console line — since we pipe stdin from null, we escalate to kill.
pub fn stop(running: &mut RunningServer) -> std::io::Result<()> {
    running.child.kill()
}

/// Curated list of official BeamNG maps for the instance wizard.
pub const OFFICIAL_MAPS: &[(&str, &str)] = &[
    ("/levels/gridmap_v2/info.json", "Gridmap V2"),
    ("/levels/gridmap/info.json", "Gridmap"),
    ("/levels/italy/info.json", "Italy"),
    ("/levels/utah/info.json", "Utah"),
    ("/levels/west_coast_usa/info.json", "West Coast USA"),
    ("/levels/east_coast_usa/info.json", "East Coast USA"),
    ("/levels/jungle_rock_island/info.json", "Jungle Rock Island"),
    ("/levels/industrial/info.json", "Industrial"),
    ("/levels/small_island/info.json", "Small Island"),
    ("/levels/smallgrid/info.json", "Small Grid"),
    ("/levels/desert/info.json", "Desert"),
    ("/levels/axample/info.json", "Axample"),
    ("/levels/driver_training/info.json", "Driver Training"),
];

/// Sanitize an instance name for use in a folder name.
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "server".into()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip() {
        let cfg = ServerConfig {
            name: "LAN Party \"Quote\"".to_string(),
            auth_key: "abcd-1234-efgh-5678".to_string(),
            port: 30815,
            max_players: 16,
            ..Default::default()
        };
        let s = cfg.to_toml_string();
        assert!(s.contains("[General]"));
        assert!(s.contains("AuthKey = \"abcd-1234-efgh-5678\""));
        assert!(s.contains("Name = \"LAN Party \\\"Quote\\\"\""));
        assert!(s.contains("Port = 30815"));
        let back = ServerConfig::from_toml_str(&s).unwrap();
        assert_eq!(back.name, cfg.name);
        assert_eq!(back.port, 30815);
        assert_eq!(back.max_players, 16);
        assert_eq!(back.auth_key, cfg.auth_key);
    }

    #[test]
    fn config_parses_real_beammp_file() {
        // Shape of the file BeamMP-Server writes (Settings.cpp defaults).
        let raw = r#"
[General]
AuthKey = "11111111-2222-3333-4444-555555555555"
LogChat = true
Debug = false
Private = true
InformationPacket = true
AllowGuests = true
IP = "::"
Port = 30814
Name = "My Server"
Tags = "Freeroam"
MaxCars = 2
MaxPlayers = 12
Map = "/levels/italy/info.json"
Description = "hi"
ResourceFolder = "Resources"

[Misc]
ImScaredOfUpdates = true
UpdateReminderTime = "30s"
"#;
        let cfg = ServerConfig::from_toml_str(raw).unwrap();
        assert_eq!(cfg.port, 30814);
        assert_eq!(cfg.name, "My Server");
        assert_eq!(cfg.max_cars, 2);
        assert_eq!(cfg.map, "/levels/italy/info.json");
        assert_eq!(cfg.auth_key.len(), 36);
    }

    #[test]
    fn instance_create_and_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let inst =
            ServerInstance::create(tmp.path(), "Fri&day Night", ServerConfig::default()).unwrap();
        assert!(inst.dir.join("Resources/Client").is_dir());
        assert!(inst.dir.join("Resources/Server").is_dir());
        assert!(inst.config_path().is_file());
        assert!(inst.dir.to_string_lossy().contains("Fri-day-Night"));
        // reload
        let loaded = ServerConfig::load(&inst.config_path()).unwrap();
        assert_eq!(loaded.port, 30814);
    }
}
