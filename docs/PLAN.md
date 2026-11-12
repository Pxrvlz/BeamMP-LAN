# BeamMP-LAN — Design & Plan

An all-in-one **LAN server manager, mod manager, launcher and server browser** for BeamMP,
built as a single native Rust desktop app for use on LAN parties and VPN LANs
(Radmin VPN, ZeroTier, Tailscale, Hamachi, plain ethernet…).

This document is the master plan: goals, architecture, UI design, protocol research,
feature breakdown and milestones.

---

## 1. Why

BeamMP's official tooling assumes an internet-connected, publicly listed server:

* The **BeamMP-Server** heartbeats to `backend.beammp.com`, requires an AuthKey from
  keymaster.beammp.com, and is designed to be listed in the public server list.
* The **BeamMP-Launcher** updates the mod, launches the game, relays game ⇄ server traffic
  through localhost (port 4444 core / 4445+ proxy), and pulls the public server list from
  the backend.
* The **in-game mod UI** gets its server list from the launcher, which gets it from the
  internet backend.

On a LAN (or a VPN LAN like Radmin) you want none of that: no internet dependency for
discovery, no public listing, everything on `26.x.x.x` / `25.x.x.x` / `192.168.x.x` style
addresses. **BeamMP-LAN** replaces the whole toolchain with one app that:

1. **Discovers** BeamMP servers on the LAN by scanning the active network interface(s)
   and speaking the native BeamMP info protocol to each candidate.
2. **Manages server instances** — download BeamMP-Server binaries, create instances,
   edit `ServerConfig.toml` with a friendly UI, watch the live console, start/stop/restart.
3. **Manages mods** — the `Resources/Client` and `Resources/Server` folders of each
   instance, enable/disable mods, install from zip, with hashes matching what the server
   advertises.
4. **Launches the game** — finds the BeamNG.drive install, installs/updates the BeamMP
   mod into `mods/multiplayer/beammp.zip`, launches the game, and then **acts as the
   launcher itself**: it serves the local core socket on `127.0.0.1:4444`, feeds the
   in-game server browser with the discovered LAN servers, and relays game ⇄ LAN server
   traffic.

The result: install BeamMP-LAN once, share it on the LAN share, and anyone can host or
join a BeamMP session over Radmin VPN with a couple of clicks — no internet backend
involved.

> **Note on authentication:** the BeamMP server software authenticates joining clients
> through `auth.beammp.com` (the client sends its public key, the server asks the backend
> who owns it). This is part of BeamMP-Server itself and works fine as long as the
> *hosting machine* has internet. Purely-offline play is possible by using server
> resources/plugins that relax this, but is out of scope for this app. BeamMP-LAN's
> discovery, mod management and relay never touch the internet backend.

## 2. Goals / non-goals

**Goals**

* Single native Rust binary, no web view, no bundled browser — `eframe`/`egui` UI.
* Windows-first (BeamNG.drive is Windows/Linux; most LAN parties are Windows), Linux fully
  supported (the CI builds both).
* Works behind Radmin VPN / ZeroTier / Tailscale / Hamachi / physical LAN.
* Zero configuration to *join*: auto-detect network, auto-discover servers, one click.
* Zero terminal use to *host*: instance wizard, config editor, console, player list.
* Real BeamMP protocol compatibility (info packet, server list injection, relay) — verified
  against the actual BeamMP-Server/Launcher/mod source code.

**Non-goals**

* Replacing the in-game multiplayer UI.
* Hosting on the public BeamMP server list (this is the LAN app; the server can still be
  made public by its own means).
* Mod *editing* (we manage files, we don't author them).

## 3. UI design — BeamNG.drive look & feel

BeamNG.drive's UI language (v0.3x "Vue" era): dark graphite panels, orange `#f80` accent,
thin light-blue selection outlines, uppercase condensed headers, white value text on dark
chips, rounded corners, subtle gradients. We reproduce it with egui:

| Token | Value (BeamNG-inspired) |
| --- | --- |
| Background | `#15181d` (deep graphite) |
| Panel | `#1b1f26` |
| Panel raised / chips | `#232933` |
| Border | `#2e3540` |
| Text primary | `#e8eaed` |
| Text secondary | `#9aa3ad` |
| Accent (orange) | `#ff7d1a` |
| Selection outline | `#66d9ff` |
| Green (online/ok) | `#67d15b` |
| Red (error/stopped) | `#e5484d` |
| Warn | `#f2c744` |
| Font | `Noto Sans` (system), headings uppercase + letter-spaced |

Layout: left vertical navigation rail with icons + labels (Servers, Host, Mods, Launcher,
Settings), content area with the BeamNG-style "card rows" (dark chips on darker
background, orange action buttons, thin `#66d9ff` focus ring on the focused row). The
top of the content area carries a context header with the page title in uppercase
condensed style and status pills (network name, launcher-core running, server running).

## 4. Architecture

```
┌──────────────────────────── BeamMP-LAN (single Rust binary) ────────────────────────────┐
│                                                                                          │
│  ┌─────────────┐   ┌────────────────────── eframe/egui UI (BeamNG-style dark theme) ───┐ │
│  │ config.json │◄──┤ Servers │ Host │ Mods │ Launcher │ Settings                        │ │
│  └─────────────┘   └───────────────┬───────────────────────────────────────────────────┘ │
│                                    │ channel events                                     │
│  ┌─────────────────────────────────┴──────────────────────────────────────────────────┐ │
│  │                            app state (central AppState)                              │ │
│  └───┬──────────────┬───────────────┬──────────────┬───────────────────┬──────────────┘ │
│      │              │               │              │                   │                │
│  ┌───▼────┐   ┌─────▼─────┐   ┌─────▼──────┐  ┌────▼─────┐   ┌─────────▼───────┐      │
│  │discovery│   │server_mgr │   │ mod_manager │  │ launcher │   │ launcher-core   │      │
│  │(scan +  │   │ (instances,│   │ (Resources/ │  │ (game    │   │ (localhost:4444  │      │
│  │ info pkts)│ │  config,   │   │  Client +   │  │  detect, │   │  core + :4445+   │      │
│  │         │   │  console)  │   │  Server)    │  │  mod dl, │   │  proxy, guest    │      │
│  │         │   │            │   │             │  │  launch) │   │  login, relay)   │      │
│  └───┬─────┘   └─────┬──────┘   └─────┬───────┘  └────┬─────┘   └─────────┬───────┘      │
│      │               │                │               │                   │              │
│  ┌───▼───────────────▼────────────────▼───────────────▼───────────────────▼───────────┐ │
│  │                        worker threads + mpsc event pump                              │ │
│  └───┬───────────────────────────────┬────────────────────────────────────────────┬───┘ │
│      │ TcpStream (info pkt 'I')      │ process (BeamMP-Server)                     │     │
│      │ UDP/TCP relay                 │ reqwest/ureq HTTPS (mod + server download)  │     │
└──────┼───────────────────────────────┼────────────────────────────────────────────┼─────┘
       ▼                                ▼                                            ▼
   LAN peers                        BeamMP-Server                              backend.beammp.com
   (Radmin VPN etc.)                instances (per-app)                        (only for mod/server
                                                                                downloads & auth)
```

### Modules

| Module | File | Responsibility |
| --- | --- | --- |
| `theme` | `src/theme.rs` | BeamNG color tokens, fonts, widget helpers |
| `state` | `src/state.rs` | `AppState`, config load/save, event channels |
| `discovery` | `src/discovery.rs` | Interface enumeration (`if-addrs`), port scan of `:30814` on all interface subnets, `'I'` info-packet queries, ping |
| `server manager` | `src/server_mgr.rs` | Instance CRUD, ServerConfig.toml read/write (typed), process spawn/kill, stdout tail → console page, `list` parsing for players |
| `mod manager` | `src/mod_manager.rs` | List Resources/Client + Resources/Server, enable/disable (`.zip.disabled`), install from zip, SHA-256 hashing, mods.json awareness |
| `launcher` | `src/launcher.rs` | Find game (Windows registry / Linux paths / Steam library scan), download `beammp.zip` from backend, place into `mods/multiplayer/`, launch game, one-click join |
| `launcher-core` | `src/launcher_core.rs` | Implements the launcher's localhost protocol: core server `127.0.0.1:4444`, proxy `4445`, `B` server-list injection from LAN discovery, `C` connect relay, `I` info query, `N` guest login via auth.beammp.com, `Z` version, `U`/`M`/`p` status |
| `app` | `src/app.rs` | egui app, page routing, event pump |

### Threading model

* UI runs on the eframe main thread; **all** blocking work (scan, process IO, downloads,
  relay sockets) runs on dedicated worker threads, communicating with the UI through
  `std::sync::mpsc` channels drained each frame (egui repaints on demand +
  `ctx.request_repaint_after`).
* The server-manager spawns `BeamMP-Server` with piped stdout/stderr, a dedicated reader
  task, and `--config` pointing at the instance's `ServerConfig.toml`.

## 5. Protocol research (verified against source)

All findings verified by reading the current BeamMP source trees (server `master`,
launcher `master`, mod `development`):

### 5.1 BeamMP-Server

* Config file: **`ServerConfig.toml`** (TOML) in the working directory — the old
  `Server.cfg` is explicitly warned as obsolete by the server itself. Keys under
  `[General]`: `AuthKey` (string, required — server refuses to start without it),
  `LogChat`, `Debug`, `Private`, `InformationPacket`, `AllowGuests` (bools), `IP`
  (default `::`), `Port` (default **30814**), `Name`, `Tags`, `Description`, `Map`
  (default `/levels/gridmap_v2/info.json`), `MaxCars` (1), `MaxPlayers` (8),
  `ResourceFolder` (default `Resources`); under `[Misc]`: `ImScaredOfUpdates`,
  `UpdateReminderTime` (`"30s"`). Full defaults from `Settings.cpp`.
* CLI: `--port=`, `--config=`, `--working-directory=`, `--version`, `--help`.
* **Info packet**: on TCP connect, send 1 byte `'I'` → server replies `i32 LE length +
  JSON` (the same body it heartbeats to the backend, minus secrets): `players`,
  `maxplayers`, `port`, `map`, `private`, `version`, `clientversion`, `name`, `tags`,
  `guests`, `modlist`, `modstotalsize`, `modstotal`, `playerslist`, `desc`. If
  `InformationPacket = false`, reply is empty. Also `'P'` → server replies `'P'` (ping).
* Auth flow for joining clients: `C` → `VC<version>` → `A` → public key → backend
  `pkToUser` on `auth.beammp.com` (server side) → `P<id>` → `SR` → mod list (new format:
  JSON with sha256 hashes; old format `name;name;size;size`) → `'D' + id` opens a second
  download socket, `'f<name>'` requests a file, `Done` finishes.
* Console commands: `help`, `list`, `status`, `settings`, `clear`, `say`, `version`,
  `protectmod`, `reloadmods`, `nettest`, `lua`, `kick` (via `mCommandMap`).
* Resources: `Resources/Client/*.zip` are pushed to joining clients (hashed, listed via
  `mods.json` cache); `Resources/Server` holds server-side Lua plugins.

### 5.2 BeamMP-Launcher (what launcher-core reimplements)

* Core socket: listens on **`127.0.0.1:4444`** (`-p/--port` to change; the game's
  `launcherPort` setting must match). Game→launcher packets: `u32 LE length` + payload.
* Game → launcher codes: `A` (heartbeat/echo `A`), `B` (server list request → reply
  `B<json array>`), `C<ip:port>` (connect), `O<url>` (open browser, allow-listed),
  `P` (→ `P<proxyPort>`), `Ul/Up` (status), `M` (mod status), `QS` (stop sync), `QG`
  (quit game), `R<name>` (mod loaded confirm), `Z` (→ `Z<version>`), `Nc` (auth check),
  `N:<json>` (login), `WY/WN` (mod-security confirm), `I<ip:port>` (server info).
* Launcher → game: same framing; codes `A`, `B<list>`, `L<mods;list>`, `M<map>` (load
  level), `N<auth json>`, `P<port>`, `U<status>`, `W MODS_FOUND`, `Z<ver>`, `J<prompt>`,
  `I<ip:port>;<json>`.
* Proxy: on `C`, launcher opens a listener on `port+1` (**4445**) — the game connects
  there for session traffic; launcher bridges game ⇄ server over TCP (+ UDP for vehicle
  data; large or unreliable-flagged packets go TCP, with `ABG:` zlib compression when
  > 400/1024 bytes).
* Guest login: `POST https://auth.beammp.com/userlogin` with JSON `{"pk": "<key>"}`
  (empty for guest) → `{"success":true,"public_key":..., "private_key":...}`; keys cached
  in a `key` file next to the binary; the public key is what the server sees during join.
* Mod update: `GET https://backend.beammp.com/sha/mod?branch=<branch>&pk=<key>` → sha256;
  if the local `mods/multiplayer/beammp.zip` hash differs, download
  `https://backend.beammp.com/builds/client?download=true&pk=<key>&branch=<branch>`.
* Game detection (Windows): registry `HKCU\Software\BeamNG\BeamNG.drive` (`rootpath`,
  `userpath_override`), Steam registry `Steam App 284160`, `startup.ini`, and
  `%LOCALAPPDATA%\BeamNG\BeamNG.Drive.ini` → user folder `.../BeamNG.drive/<version>`.
  Linux: `~/.local/share/BeamNG/BeamNG.drive/`. Game exe: `BeamNG.drive.exe` (Win) /
  `BinLinux/BeamNG.drive.x64` (Linux).
* In-game server list JSON (fed via `B`): array of objects with at least `ip`, `port`,
  `sname`/`name`, `map`, `players`, `maxplayers`, `version`, `tags`, `modlist`…  (the mod
  normalizes with `normalizeServer`; `sname` may contain color formatting).

### 5.3 What BeamMP-LAN changes for LAN

* Discovery = scan every interface's subnet for open **30814** + `'I'` info query (no
  internet).
* The in-game server browser is fed from **LAN discovery** via the `B` reply.
* "Join" = `C<lan-ip>:30814` through our embedded launcher-core relay.
* Host = BeamMP-Server instance with `Private = true` (not listed publicly), started by
  the app with a generated AuthKey field the user fills from keymaster (one-time, needs
  internet) — everything else stays LAN-local.

## 6. Feature breakdown

### 6.1 Servers page (join side)

* Live LAN server list: name, map (pretty-printed), players/max, version, ping, tags.
* Manual "Add server" by `IP:port` for cases discovery can't see (e.g. different subnet
  mask).
* Favorites & recents (persisted in config).
* One-click **Join** → launcher-core `C` connect + game launch if not running.
* Refresh button + auto-refresh every 5 s; per-server info expansion (mods list, desc).

### 6.2 Host page (server manager)

* Instance list with status (Running / Stopped / Crashed), players online, uptime.
* **New instance wizard**: name, port (auto-suggest next free), map picker (curated list
  of official maps), max players/cars, tags, Radmin-VPN-aware hint showing the host's
  addresses to share.
* Config editor: friendly form for all `[General]`/`[Misc]` keys + raw TOML view.
* Console: live stdout/stderr tail with autoscroll, command input (supports `list`,
  `status`, `say …`, `kick …`), and a players panel parsed from `list` output.
* Start / Stop / Restart / Open folder; delete instance (with confirm).
* First-run: "Download server" button fetching the latest BeamMP-Server release from
  GitHub (asset per OS), unpacked into the app data dir.

### 6.3 Mods page

* Per-instance mod list, split **Client mods** (`Resources/Client`) vs **Server
  resources** (`Resources/Server`).
* Each entry: file name, size, sha256 (matches what the server advertises), protected
  flag from `mods.json`, enable/disable toggle (renames to `.zip.disabled` — the server
  only globs `*.zip`), delete, open in explorer.
* "Install mod…" file picker → copies zip into the right folder; warns on duplicates by
  hash.

### 6.4 Launcher page

* Game detection status: install path, user folder, game version, BeamMP mod state
  (installed / update available).
* "Install / Update BeamMP mod" (downloads from backend.beammp.com; shows hash check).
* "Launch game" (with or without launching BeamMP-LAN's core).
* One-click join from the Servers page does: ensure mod → launch game → core connects →
  in-game list shows LAN servers → auto-connect to the chosen server.

### 6.5 Settings page

* Data directory, server binary path override, discovery port range + timeout, interface
  selection (auto = all non-loopback), launcher core port (4444) + proxy base (4445),
  theme accent, language (EN only for v1).
* "About" panel with licenses (this app AGPL-3.0; BeamMP AGPL-3.0).

## 7. Data layout

```
<app data>/                       (config dir; e.g. %APPDATA%/BeamMP-LAN or ~/.config/beammp-lan)
  config.json                     app settings, favorites, manual servers
  servers/<instance>/ServerConfig.toml
  servers/<instance>/Resources/{Client,Server}/
  server-bin/<version>/BeamMP-Server[.exe]   downloaded server binaries
  core/key                        cached BeamMP guest key
```

## 8. Milestones

| # | Milestone | Status |
| --- | --- | --- |
| M0 | Protocol research & this plan | done |
| M1 | App skeleton: eframe + theme + nav + config | done |
| M2 | Discovery + Servers page (info packets, ping) | done |
| M3 | Server manager: instances, TOML editor, console | done |
| M4 | Mod manager | done |
| M5 | Launcher: game detect + mod download + launch | done |
| M6 | Launcher-core: 4444 core, 4445 proxy, B-list injection, guest login, relay | done |
| M7 | Settings page, packaging (CI), README, screenshots | done |

## 9. Licensing

BeamMP-LAN is licensed under **AGPL-3.0-or-later** (same as BeamMP itself). It
interoperates with BeamMP-Server, BeamMP-Launcher and the BeamMP mod over their documented
network protocols but does not include their source. BeamMP is © BeamMP Ltd. — this is a
community LAN tool, not an official BeamMP product.
