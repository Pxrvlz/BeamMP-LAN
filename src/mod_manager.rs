//! Mod manager for a server instance's Resources folders.
//!
//! * `Resources/Client/*.zip` — mods pushed to joining players (hashed, `mods.json` cache).
//! * `Resources/Server/*.zip` — server-side Lua plugins.
//! * Disabled state = renamed to `.zip.disabled` (the server only globs `*.zip`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Where a mod lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ModSide {
    Client,
    Server,
}

impl ModSide {
    pub fn label(&self) -> &'static str {
        match self {
            ModSide::Client => "CLIENT MOD",
            ModSide::Server => "SERVER PLUGIN",
        }
    }
}

/// One mod file on disk.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ModEntry {
    pub side: ModSide,
    pub file_name: String,
    pub path: PathBuf,
    pub size: u64,
    pub enabled: bool,
    pub hash: Option<String>,
    /// From the server's `Resources/Client/mods.json`, if present.
    pub protected: bool,
}

/// sha256 of a file, hex lowercase.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = std::io::Read::read(&mut file, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let out = hasher.finalize();
    Ok(out.iter().map(|b| format!("{b:02x}")).collect())
}

/// Parse `Resources/Client/mods.json` → set of protected file names.
fn protected_set(dir: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(raw) = std::fs::read_to_string(dir.join("mods.json")) else {
        return out;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return out;
    };
    if let Some(obj) = v.as_object() {
        for (name, entry) in obj {
            if entry
                .get("protected")
                .and_then(|p| p.as_bool())
                .unwrap_or(false)
            {
                out.insert(name.clone());
            }
        }
    }
    out
}

/// List mods of one side.
pub fn list_mods(dir: &Path, side: ModSide) -> Vec<ModEntry> {
    let protected = protected_set(dir);
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if file_name == "mods.json" {
            continue;
        }
        let lower = file_name.to_lowercase();
        if lower.ends_with(".zip") {
            out.push(ModEntry {
                side,
                file_name: file_name.to_string(),
                size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                enabled: true,
                hash: None,
                protected: protected.contains(file_name),
                path,
            });
        } else if lower.ends_with(".zip.disabled") {
            out.push(ModEntry {
                side,
                file_name: file_name.trim_end_matches(".disabled").to_string(),
                size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                enabled: false,
                hash: None,
                protected: protected.contains(file_name.trim_end_matches(".disabled")),
                path,
            });
        }
    }
    out.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    out
}

/// Toggle a mod (rename to/from `.zip.disabled`).
pub fn set_enabled(entry: &ModEntry, enabled: bool) -> std::io::Result<PathBuf> {
    if entry.enabled == enabled {
        return Ok(entry.path.clone());
    }
    let target = if enabled {
        PathBuf::from(
            entry
                .path
                .to_string_lossy()
                .trim_end_matches(".disabled")
                .to_string(),
        )
    } else {
        PathBuf::from(format!("{}.disabled", entry.path.to_string_lossy()))
    };
    std::fs::rename(&entry.path, &target)?;
    Ok(target)
}

/// Install a zip into a side's folder. Returns the destination path.
pub fn install_zip(src: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "mod.zip".into());
    let dest = dir.join(sanitize_mod_name(&name));
    if dest.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("'{}' already exists in {}", name, dir.display()),
        ));
    }
    std::fs::copy(src, &dest)?;
    Ok(dest)
}

fn sanitize_mod_name(name: &str) -> String {
    // BeamNG can't handle uppercase names on Linux — keep names filesystem-safe.
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " .-_()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_and_toggle() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Resources/Client");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("car_pack.zip"), b"fake zip").unwrap();
        std::fs::write(dir.join("map_pack.zip.disabled"), b"fake zip").unwrap();
        std::fs::write(dir.join("notes.txt"), b"ignored").unwrap();
        std::fs::write(
            dir.join("mods.json"),
            r#"{"car_pack.zip": {"protected": true, "hash": "x"}}"#,
        )
        .unwrap();

        let mods = list_mods(&dir, ModSide::Client);
        assert_eq!(mods.len(), 2);
        let car = mods.iter().find(|m| m.file_name == "car_pack.zip").unwrap();
        assert!(car.enabled);
        assert!(car.protected);
        let map = mods.iter().find(|m| m.file_name == "map_pack.zip").unwrap();
        assert!(!map.enabled);

        // disable car
        let p = set_enabled(car, false).unwrap();
        assert!(p.to_string_lossy().ends_with("car_pack.zip.disabled"));
        let mods2 = list_mods(&dir, ModSide::Client);
        assert!(
            !mods2
                .iter()
                .find(|m| m.file_name == "car_pack.zip")
                .unwrap()
                .enabled
        );
        // re-enable
        let car2 = mods2
            .iter()
            .find(|m| m.file_name == "car_pack.zip")
            .unwrap();
        let p2 = set_enabled(car2, true).unwrap();
        assert!(p2.to_string_lossy().ends_with("car_pack.zip"));
    }

    #[test]
    fn install_rejects_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("newmod.zip");
        std::fs::write(&src, b"data").unwrap();
        let dir = tmp.path().join("Resources/Client");
        let installed = install_zip(&src, &dir).unwrap();
        assert!(installed.is_file());
        assert!(install_zip(&src, &dir).is_err());
    }

    #[test]
    fn hashes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("x.zip");
        std::fs::write(&f, b"hello").unwrap();
        let h = sha256_file(&f).unwrap();
        assert_eq!(h.len(), 64);
        // well-known sha256("hello")
        assert_eq!(
            h,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
