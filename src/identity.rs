//! Local identity & roles: nickname + machine ID, no passwords, no internet.
//!
//! Replaces the txt plan's "accounts.db" with a human-readable, git-diffable
//! `accounts.json` (same capability, zero binary dependency — an app that promises
//! "no runtime dependencies" shouldn't need SQLite just for 8 LAN players).
//!
//! * Players identify as **nickname + local hardware ID** (UUID generated once per
//!   machine, stored next to the config). No passwords, no email.
//! * The host keeps an optional **role table**: `Owner` → `Admin` → `Moderator` →
//!   `Player`, assigned by machine ID. Unknown IDs join as `Player` (or `Guest`
//!   when guest access is on and no identity is set).
//! * On stock BeamMP-Server instances the roles map to the server's own permission
//!   groups only where BeamMP exposes them; the full role set is enforced by the
//!   embedded relay (see `relay.rs`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// LAN roles. Derive order = privilege order: Guest < Player < Moderator < Admin < Owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Role {
    Guest,
    Player,
    Moderator,
    Admin,
    Owner,
}

impl Role {
    pub fn label(&self) -> &'static str {
        match self {
            Role::Owner => "Owner",
            Role::Admin => "Admin",
            Role::Moderator => "Moderator",
            Role::Player => "Player",
            Role::Guest => "Guest",
        }
    }

    /// The crown shows for the session owner on the players panel.
    pub fn has_crown(&self) -> bool {
        matches!(self, Role::Owner)
    }

    #[allow(dead_code)] // enforced by relay kick commands in the next milestone
    pub fn can_kick(&self) -> bool {
        matches!(self, Role::Owner | Role::Admin | Role::Moderator)
    }

    #[allow(dead_code)] // enforced by relay mod commands in the next milestone
    pub fn can_manage_mods(&self) -> bool {
        matches!(self, Role::Owner | Role::Admin)
    }
}

/// A player identity: nickname + stable machine id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Identity {
    pub nickname: String,
    /// 16 hex chars derived from a one-time generated UUID (this machine).
    pub machine_id: String,
}

impl Identity {
    /// Load from `identity.json`, or create with a given/default nickname.
    pub fn load_or_create(dir: &Path, nickname: &str) -> std::io::Result<Identity> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("identity.json");
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(id) = serde_json::from_str::<Identity>(&raw) {
                if id.is_valid() {
                    return Ok(id);
                }
            }
        }
        let id = Identity {
            nickname: sanitize_nickname(nickname),
            machine_id: new_machine_id(),
        };
        let json = serde_json::to_string_pretty(&id)?;
        std::fs::write(&path, json)?;
        Ok(id)
    }

    pub fn is_valid(&self) -> bool {
        !self.nickname.trim().is_empty()
            && self.nickname.chars().count() <= 16
            && self.machine_id.len() == 16
            && self.machine_id.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Persist a nickname change.
    pub fn set_nickname(&mut self, dir: &Path, nickname: &str) -> std::io::Result<()> {
        self.nickname = sanitize_nickname(nickname);
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("identity.json"),
            serde_json::to_string_pretty(self)?,
        )
    }
}

/// Trim, strip control chars, cap at 16 chars, default "Player".
pub fn sanitize_nickname(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(16)
        .collect();
    if cleaned.is_empty() {
        "Player".into()
    } else {
        cleaned
    }
}

fn new_machine_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..16].to_string()
}

/// The host's role table (accounts.json).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Accounts {
    /// machine_id → (nickname, role)
    #[serde(flatten)]
    pub entries: BTreeMap<String, AccountEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountEntry {
    pub nickname: String,
    pub role: Role,
}

impl Accounts {
    pub fn load(dir: &Path) -> Accounts {
        std::fs::read_to_string(dir.join("accounts.json"))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("accounts.json"),
            serde_json::to_string_pretty(self)?,
        )
    }

    pub fn role_of(&self, identity: &Identity) -> Role {
        self.entries
            .get(&identity.machine_id)
            .map(|e| e.role)
            .unwrap_or(Role::Player)
    }

    pub fn upsert(&mut self, identity: &Identity, role: Role) {
        self.entries.insert(
            identity.machine_id.clone(),
            AccountEntry {
                nickname: identity.nickname.clone(),
                role,
            },
        );
    }

    /// Remove a machine from the role table (called from the Settings roles editor).
    #[allow(dead_code)] // exposed for the roles editor's remove action
    pub fn remove(&mut self, machine_id: &str) -> bool {
        self.entries.remove(machine_id).is_some()
    }

    /// Ensure the first-ever identity becomes Owner automatically (zero-config).
    pub fn ensure_owner(&mut self, identity: &Identity) {
        if self.entries.is_empty() {
            self.upsert(identity, Role::Owner);
        }
    }
}

/// Where identity files live (app data dir).
pub fn identity_dir(base: &Path) -> PathBuf {
    base.join("identity")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_roundtrip_and_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let id = Identity::load_or_create(tmp.path(), "  Racer 7 ").unwrap();
        assert_eq!(id.nickname, "Racer 7");
        assert_eq!(id.machine_id.len(), 16);
        assert!(id.is_valid());

        // reload keeps machine id
        let id2 = Identity::load_or_create(tmp.path(), "ignored").unwrap();
        assert_eq!(id2.machine_id, id.machine_id);
        assert_eq!(id2.nickname, "Racer 7"); // persisted nickname wins

        // nickname change persists
        let mut id3 = id2;
        id3.set_nickname(tmp.path(), "Bob").unwrap();
        let id4 = Identity::load_or_create(tmp.path(), "?").unwrap();
        assert_eq!(id4.nickname, "Bob");
        assert_eq!(id4.machine_id, id3.machine_id);
    }

    #[test]
    fn nickname_sanitizing() {
        assert_eq!(sanitize_nickname(""), "Player");
        assert_eq!(sanitize_nickname("   "), "Player");
        assert_eq!(sanitize_nickname("Ann\u{1}a"), "Anna");
        assert_eq!(
            sanitize_nickname("01234567890123456789"),
            "0123456789012345"
        );
    }

    #[test]
    fn roles_order_and_permissions() {
        assert!(Role::Owner > Role::Admin);
        assert!(Role::Admin > Role::Moderator);
        assert!(Role::Moderator > Role::Player);
        assert!(Role::Player > Role::Guest);
        assert!(Role::Owner.can_kick());
        assert!(Role::Moderator.can_kick());
        assert!(!Role::Player.can_kick());
        assert!(Role::Owner.has_crown());
        assert!(!Role::Admin.has_crown());
        assert!(Role::Admin.can_manage_mods());
        assert!(!Role::Moderator.can_manage_mods());
    }

    #[test]
    fn accounts_zero_config_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let id = Identity {
            nickname: "First".into(),
            machine_id: "0123456789abcdef".into(),
        };
        let mut acc = Accounts::load(tmp.path());
        acc.ensure_owner(&id);
        assert_eq!(acc.role_of(&id), Role::Owner);

        // second player defaults to Player
        let other = Identity {
            nickname: "Second".into(),
            machine_id: "fedcba9876543210".into(),
        };
        assert_eq!(acc.role_of(&other), Role::Player);

        // promote and persist
        acc.upsert(&other, Role::Moderator);
        acc.save(tmp.path()).unwrap();
        let mut acc2 = Accounts::load(tmp.path());
        assert_eq!(acc2.role_of(&other), Role::Moderator);
        assert_eq!(acc2.role_of(&id), Role::Owner);

        assert!(acc2.remove("fedcba9876543210"));
        assert!(!acc2.remove("nonexistent"));
    }
}
