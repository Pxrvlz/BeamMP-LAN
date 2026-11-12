//! Game launcher: detect BeamNG.drive, install/update the BeamMP mod, launch the game.
//!
//! Detection logic mirrors `BeamMP-Launcher/src/Security/BeamNG.cpp` + `GameStart.cpp`:
//! Windows: registry HKCU\Software\BeamNG\BeamNG.drive (rootpath, userpath_override),
//! Steam "Steam App 284160", startup.ini, %LOCALAPPDATA%\BeamNG\BeamNG.Drive.ini.
//! Linux: ~/.local/share/BeamNG/BeamNG.drive/.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::net;

/// Endpoints used by the official launcher (Startup.cpp / Login.cpp).
pub const BACKEND_BASE: &str = "https://backend.beammp.com";
pub const AUTH_BASE: &str = "https://auth.beammp.com";
/// Latest BeamMP client mod download (branch from launcher Config.cpp; default "stable").
pub const MOD_BRANCH: &str = "stable";

/// Where the game's user folder was found.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GameInstall {
    /// Install dir (contains BeamNG.drive.exe / BinLinux).
    pub game_dir: PathBuf,
    /// User dir (.../BeamNG.drive/<version>).
    pub user_dir: PathBuf,
    /// How it was found.
    pub method: String,
    /// Parsed game version (from version.txt), e.g. "0.34".
    pub game_version: String,
}

impl GameInstall {
    pub fn mod_dir(&self) -> PathBuf {
        self.user_dir.join("mods").join("multiplayer")
    }

    pub fn beammp_zip(&self) -> PathBuf {
        self.mod_dir().join("beammp.zip")
    }

    pub fn db_json(&self) -> PathBuf {
        self.user_dir.join("mods").join("db.json")
    }
}

/// Attempt automatic detection, in the same order the official launcher uses.
pub fn detect_game() -> Option<GameInstall> {
    #[cfg(windows)]
    {
        if let Some(g) = detect_game_windows() {
            return Some(g);
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(g) = detect_game_linux() {
            return Some(g);
        }
    }
    None
}

#[cfg(windows)]
fn detect_game_windows() -> Option<GameInstall> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    // 1. HKCU\Software\BeamNG\BeamNG.drive : rootpath / userpath_override
    let mut game_dir: Option<PathBuf> = None;
    let mut user_override: Option<PathBuf> = None;
    if let Ok(key) = hkcu.open_subkey("Software\\BeamNG\\BeamNG.drive") {
        if let Ok(rp) = key.get_value::<String, _>("rootpath") {
            game_dir = Some(PathBuf::from(&rp));
        }
        if let Ok(up) = key.get_value::<String, _>("userpath_override") {
            user_override = Some(PathBuf::from(&up));
        }
    }

    // 2. Steam: HKCU\Software\Valve\Steam → install path via libraryfolders.vdf
    if game_dir.is_none() {
        if let Ok(steam) = hkcu.open_subkey("Software\\Valve\\Steam") {
            if let Ok(steampath) = steam.get_value::<String, _>("SteamPath") {
                let libs = parse_steam_libraries(
                    Path::new(&steampath).join("steamapps/libraryfolders.vdf"),
                );
                for lib in libs {
                    let candidate = lib.join("steamapps/common/BeamNG.drive");
                    if candidate.join("BeamNG.drive.exe").exists() {
                        game_dir = Some(candidate);
                        break;
                    }
                }
            }
        }
    }

    let game_dir = game_dir?;
    let user_dir = resolve_user_dir(&game_dir, user_override, true)?;
    let game_version = read_version(&user_dir);
    Some(GameInstall {
        game_dir,
        user_dir,
        method: "registry".into(),
        game_version,
    })
}

#[cfg(windows)]
fn parse_steam_libraries(vdf: PathBuf) -> Vec<PathBuf> {
    let Ok(raw) = std::fs::read_to_string(vdf) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in raw.lines() {
        if let Some(idx) = line.find("\"path\"") {
            let rest = &line[idx..];
            if let Some(q1) = rest.find('"') {
                if let Some(q2) = rest[q1 + 1..].find('"') {
                    let p = &rest[q1 + 1..q1 + 1 + q2];
                    let p = p.replace("\\\\", "\\");
                    out.push(PathBuf::from(p));
                }
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn detect_game_linux() -> Option<GameInstall> {
    let home = std::env::var("HOME").ok()?;
    let base = PathBuf::from(&home).join(".local/share/BeamNG/BeamNG.drive");
    // newest version dir
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&base)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    versions.sort();
    let user_dir = versions.into_iter().next_back()?;
    let game_version = read_version(&user_dir);
    let game_dir = base.parent()?.parent()?.to_path_buf();
    Some(GameInstall {
        game_dir,
        user_dir,
        method: "linux-default".into(),
        game_version,
    })
}

/// Resolve the user folder: override → startup.ini → BeamNG.Drive.ini → default.
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_user_dir(
    game_dir: &Path,
    user_override: Option<PathBuf>,
    windows: bool,
) -> Option<PathBuf> {
    if let Some(o) = user_override {
        if o.exists() {
            return Some(o);
        }
    }
    // startup.ini [filesystem] UserPath
    if let Ok(raw) = std::fs::read_to_string(game_dir.join("startup.ini")) {
        if let Some(v) = ini_get(&raw, "filesystem", "UserPath") {
            let p = expand_home(&v);
            if p.exists() {
                return Some(p);
            }
        }
    }
    if windows {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let beamng_dir = PathBuf::from(local).join("BeamNG");
            if let Ok(raw) = std::fs::read_to_string(beamng_dir.join("BeamNG.Drive.ini")) {
                if let Some(v) = ini_get(&raw, "userFolder", "") {
                    let p = expand_home(&v);
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
            let candidate = beamng_dir.join("BeamNG.drive");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    // fallback: game_dir/../ (portable installs)
    let next_to = game_dir.parent()?.join("BeamNG.drive");
    if next_to.exists() {
        return Some(next_to);
    }
    None
}

#[cfg_attr(not(windows), allow(dead_code))]
fn ini_get(raw: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = section.is_empty();
    for line in raw.lines() {
        let l = line.trim();
        if l.starts_with('[') && l.ends_with(']') {
            in_section = l[1..l.len() - 1].eq_ignore_ascii_case(section);
            continue;
        }
        if in_section {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim().eq_ignore_ascii_case(key) {
                    return Some(v.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    None
}

#[cfg_attr(not(windows), allow(dead_code))]
fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

/// The game stores version.txt next to the version dirs.
fn read_version(user_dir: &Path) -> String {
    std::fs::read_to_string(
        user_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| user_dir.to_path_buf())
            .join("version.txt"),
    )
    .map(|v| v.trim().to_string())
    .unwrap_or_default()
}

/// State of the installed BeamMP mod.
#[derive(Debug, Clone)]
pub struct ModState {
    pub installed: bool,
    pub zip_path: Option<PathBuf>,
    pub hash: Option<String>,
}

pub fn mod_state(install: &GameInstall) -> ModState {
    let zip = install.beammp_zip();
    if zip.exists() {
        ModState {
            installed: true,
            zip_path: Some(zip.clone()),
            hash: super::mod_manager::sha256_file(&zip).ok(),
        }
    } else {
        ModState {
            installed: false,
            zip_path: None,
            hash: None,
        }
    }
}

/// Guest login against auth.beammp.com (the launcher sends an empty `pk` for guests).
/// Returns the public key to use when joining servers.
pub fn guest_login() -> Result<String, String> {
    let resp = ureq::post(&format!("{AUTH_BASE}/userlogin"))
        .timeout(Duration::from_secs(10))
        .send_json(serde_json::json!({ "pk": "" }))
        .map_err(|e| format!("auth request failed: {e}"))?;
    let v: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("bad auth reply: {e}"))?;
    if v.get("success").and_then(|s| s.as_bool()).unwrap_or(false) {
        v.get("public_key")
            .and_then(|k| k.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "auth ok but no public_key".into())
    } else {
        Err(format!(
            "auth refused: {}",
            v.get("message").and_then(|m| m.as_str()).unwrap_or("?")
        ))
    }
}

/// Fetch the sha256 the backend expects for the current stable mod build.
#[allow(dead_code)]
pub fn fetch_mod_hash(public_key: &str) -> Result<String, String> {
    let url = format!("{BACKEND_BASE}/sha/mod?branch={MOD_BRANCH}&pk={public_key}");
    let resp = ureq::get(&url)
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| format!("hash request failed: {e}"))?;
    let h = resp
        .into_string()
        .map_err(|e| format!("bad hash reply: {e}"))?
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>();
    if h.len() == 64 {
        Ok(h)
    } else {
        Err("backend returned an invalid hash".into())
    }
}

/// Download the BeamMP mod zip into the game's mods/multiplayer folder.
/// Mirrors PreGame() in Startup.cpp (Windows name BeamMP.zip; Linux lowercase).
pub fn download_mod(install: &GameInstall, public_key: &str) -> Result<PathBuf, String> {
    let dir = install.mod_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create mods dir: {e}"))?;

    // enable in db.json (EnableMP in Startup.cpp)
    enable_in_db(install);

    let file_name = if cfg!(windows) {
        "BeamMP.zip"
    } else {
        "beammp.zip"
    };
    let dest = dir.join(file_name);
    let tmp = dir.join(format!("{file_name}.tmp"));

    let url =
        format!("{BACKEND_BASE}/builds/client?download=true&pk={public_key}&branch={MOD_BRANCH}");
    let resp = ureq::get(&url)
        .timeout(Duration::from_secs(120))
        .call()
        .map_err(|e| format!("mod download failed: {e}"))?;
    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf)
        .map_err(|e| format!("mod download read failed: {e}"))?;
    if buf.len() < 10_000 {
        return Err("mod download too small / not a zip".into());
    }
    std::fs::write(&tmp, &buf).map_err(|e| format!("cannot write mod: {e}"))?;
    std::fs::rename(&tmp, &dest).map_err(|e| format!("cannot move mod into place: {e}"))?;
    Ok(dest)
}

/// Set mods.multiplayerbeammp.active = true in db.json (launcher EnableMP()).
fn enable_in_db(install: &GameInstall) {
    let db = install.db_json();
    let Ok(raw) = std::fs::read_to_string(&db) else {
        return;
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let Some(mods) = v.get_mut("mods").and_then(|m| m.as_object_mut()) else {
        return;
    };
    if let Some(entry) = mods.get_mut("multiplayerbeammp") {
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("active".into(), serde_json::Value::Bool(true));
            let _ = std::fs::write(&db, serde_json::to_string(&v).unwrap_or_default());
        }
    }
}

/// Launch the game (detached). Returns the spawned child.
pub fn launch_game(
    install: &GameInstall,
    extra_args: &[String],
) -> std::io::Result<std::process::Child> {
    #[cfg(windows)]
    let exe = install.game_dir.join("BeamNG.drive.exe");
    #[cfg(not(windows))]
    let exe = install.game_dir.join("BinLinux/BeamNG.drive.x64");

    let mut cmd = std::process::Command::new(&exe);
    cmd.current_dir(&install.game_dir);
    for a in extra_args {
        cmd.arg(a);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        cmd.stdin(std::process::Stdio::null());
    }
    cmd.spawn()
}

/// Download the latest BeamMP-Server binary from GitHub releases into `dest_dir`.
/// Returns the path to the executable.
pub fn download_server_release(dest_dir: &Path) -> Result<PathBuf, String> {
    let api = "https://api.github.com/repos/BeamMP/BeamMP-Server/releases/latest";
    let resp = ureq::get(api)
        .timeout(Duration::from_secs(15))
        .set("User-Agent", "BeamMP-LAN")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("github api failed: {e}"))?;
    let v: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("bad github reply: {e}"))?;
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or("no tag_name in release")?
        .to_string();
    // Store under a version-tagged subdir so multiple releases can coexist.
    let dest_dir = &dest_dir.join(&tag);
    let assets = v
        .get("assets")
        .and_then(|a| a.as_array())
        .ok_or("no assets in release")?;

    let want = if cfg!(windows) { "windows" } else { "linux" };
    let mut chosen: Option<(String, String)> = None;
    for a in assets {
        let name = a.get("name").and_then(|n| n.as_str()).unwrap_or_default();
        let url = a
            .get("browser_download_url")
            .and_then(|u| u.as_str())
            .unwrap_or_default();
        let lower = name.to_lowercase();
        let is_server_bin =
            lower.contains(want) && !lower.contains("sha256") && !lower.contains(".txt");
        if is_server_bin && chosen.is_none() {
            chosen = Some((name.to_string(), url.to_string()));
        }
    }
    let Some((name, url)) = chosen else {
        return Err("no matching server asset in the latest release".into());
    };

    std::fs::create_dir_all(dest_dir).map_err(|e| format!("cannot create dir: {e}"))?;
    let zip_path = dest_dir.join(&name);
    let resp = ureq::get(&url)
        .timeout(Duration::from_secs(300))
        .call()
        .map_err(|e| format!("server download failed: {e}"))?;
    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf)
        .map_err(|e| format!("server download read failed: {e}"))?;
    std::fs::write(&zip_path, &buf).map_err(|e| format!("cannot write server zip: {e}"))?;

    // If it's a zip, extract; if it's a raw binary, chmod +x.
    if name.ends_with(".zip") {
        extract_zip(&zip_path, dest_dir)?;
        let bin = find_server_binary(dest_dir)
            .ok_or("extracted archive contains no BeamMP-Server binary")?;
        make_executable(&bin);
        Ok(bin)
    } else {
        make_executable(&zip_path);
        Ok(zip_path)
    }
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("bad zip: {e}"))?;
    archive
        .extract(dest)
        .map_err(|e| format!("zip extract failed: {e}"))?;
    Ok(())
}

fn find_server_binary(dir: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("beammp-server") {
            return Some(entry.path());
        }
    }
    // one level deep (zip may contain a folder)
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        if entry.path().is_dir() {
            if let Some(p) = find_server_binary(&entry.path()) {
                return Some(p);
            }
        }
    }
    None
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perm = meta.permissions();
            perm.set_mode(0o755);
            let _ = std::fs::set_permissions(path, perm);
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Convenience: probe a server's info to sanity-check reachability before join.
#[allow(dead_code)]
pub fn probe(addr: &str) -> Option<net::ServerInfo> {
    net::query_server_info(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_parsing() {
        let ini = "[filesystem]\nUserPath = C:/games/user\n";
        assert_eq!(
            ini_get(ini, "filesystem", "UserPath").as_deref(),
            Some("C:/games/user")
        );
        let ini2 = "[userFolder]\n  C:/other  \n";
        assert_eq!(ini_get(ini2, "userFolder", ""), None);
    }
}
