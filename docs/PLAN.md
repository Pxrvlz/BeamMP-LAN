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
| M8 | v0.2.0 "LAN Edition": UDP beacon, local identity + roles, Relay Mode, zero-config defaults | done |

---

# v0.2.0 — the "LAN Edition" upgrade

The v0.2.0 release is a planned, honest response to the *BeamMP LAN Edition* concept
document (a product pitch that promised full offline play, no accounts, no internet —
but was never implemented). Instead of shipping marketing claims, every promise from
that document was either **implemented verifiably**, **replaced with something that
actually works**, or **documented as not-yet-true**. This section is the mapping.

## 10. Claim → reality → implementation

| # | LAN Edition claim | Reality check | What v0.2.0 does |
| --- | --- | --- | --- |
| 1 | "Hosts and clients discover each other via UDP LAN beacon" | Not in v0.1.0 (TCP subnet scan only, ~3 s) | **Implemented**: `beacon.rs` — hosts broadcast `BEAMMP_LAN1\|announce\|{json}` on UDP 30816 every 3 s; clients `seek` and get `reply` in < 1 s. TCP scan remains the fallback for broadcast-filtering networks. |
| 2 | "No internet required" | Stock BeamMP-Server authenticates clients against auth.beammp.com — an internet dependency in the server itself | **Partially true, now honestly labeled**: *Relay Mode* (below) is fully offline. BeamMP-Server instances still need internet once for the AuthKey + client auth. The README has an explicit offline-capability matrix. |
| 3 | "Fully self-contained, offline-capable fork of BeamMP rewritten in Rust" | BeamMP itself is AGPL C++ — forking it into this Rust app is neither needed nor done; only the wire protocols are shared | **Clarified**: BeamMP-LAN is a *companion*, not a fork. The only BeamMP code path replaced is the launcher (embedded core, v0.1.0). The server-side equivalent is Relay Mode. |
| 4 | "Single ~9 MB executable" | True-ish goal; v0.1.0 release build was ~6–8 MB already | **Kept**: single static binary; CI measures and the README publishes the actual size per release. |
| 5 | "Players authenticate with nickname + local hardware ID — no passwords" | Not in v0.1.0 | **Implemented**: `identity.rs` — nickname + 16-hex machine ID generated once, persisted in `identity.json`. The embedded core reports it to the game (`N` auth reply). |
| 6 | "Optional role system: Owner/Admin/Moderator/Player (accounts.db)" | SQLite for 8 LAN players is overkill; "hardware ID" is just a random UUID (no actual hardware fingerprinting — that would be invasive) | **Implemented, simplified**: `accounts.json` (human-readable, diff-friendly) with the full role ladder, ordered `Guest < Player < Moderator < Admin < Owner`. First-ever host auto-becomes Owner. Role editor in Settings. "Hardware ID" honestly = per-machine random ID. |
| 7 | "Guest access supported" | — | **Kept**: `AllowGuests` on stock servers; relay accepts any identity and assigns `Player <n>`. |
| 8 | "In-game UI fully redesigned (7 screens: badges, scan buttons, teleport…)" | The BeamMP client mod is a closed-ish Lua/CEF codebase; shipping a redesigned in-game UI inside this app is not possible without forking the mod itself | **Replaced**: the app's own desktop UI (Servers/Host/Mods/Launcher/Settings) carries the LAN Edition features (beacon indicator, `[LAN]` tags, relay stats, roles, crown). The in-game browser gets the LAN server list via the core's `B` injection — same effect for discovery, without forking the mod. Documented as out-of-scope for the app. |
| 9 | "Bring-your-own maps — drop the .zip into Resources/Client" | Already how BeamMP works | **Kept + improved**: Mods page manages it with enable/disable, hashing, and a file picker; relay serves the same folder. |
| 10 | "Zero-config server: 8 players, 4 cars out of the box" | v0.1.0 defaults were MaxCars 1, tags "Freeroam" | **Implemented**: defaults now `MaxPlayers 8`, `MaxCars 4`, tags `Freeroam,LAN`; Relay Mode needs literally zero config (no AuthKey at all). |
| 11 | "CobaltEssentials server plugin included" | Bundling third-party binaries has license + update problems | **Implemented as 1-click install**: Host page button downloads the verified official release (prestonelam2003/CobaltEssentials v1.7.6, 60 KB) and extracts the plugin zips into the selected instance's `Resources/Server`. |
| 12 | "CEF keyboard focus fixed (chat)" | That's a BeamMP mod bug, fixed upstream | **Dropped**: not this app's code; upstream mod handles it. |

## 11. Relay Mode — the fully-offline path (experimental)

`relay.rs` embeds a BeamMP-speaking server in the app itself, removing the last hard
internet dependency for hosting:

```
client                              BeamMP-LAN (Relay Mode)
  │  TCP connect                           │
  │  'C' ────────────────────────────────► │  identify
  │  ◄frame► "VC2.3.2" ──────────────────► │  version  ──► framed "A"
  │  ◄frame► "<machine_id>|<nickname>" ───► │  local auth (any key accepted;
  │                                        │   identity from identity.rs)
  │  ◄frame► "P1"                          │  client id assigned
  │  ◄frame► "SR" ───────────────────────► │  mod list {"mods": {...}}
  │  ◄frame► "f<mymod.zip>" ─────────────► │  mod served from Resources/Client
  │  ◄frame► "Done"                        │  enter session
  │  ◄──── frames fan out between all connected clients ────► │
```

* **Local trust model**: any auth key is accepted; the identity payload
  `<machine_id>|<nickname>` is parsed when our own client side sends it. Unknown
  clients get `Player <id>`.
* **Mods**: the same `Resources/Client` folder as stock; `f<name>` file requests are
  served with length-prefixed raw bytes (stock protocol), path-traversal-proof.
* **Sessions**: every client's frames fan out to all others (no echo). Chat frames
  surface in the Host page relay log.
* **Why experimental**: the real BeamNG client exercises far more protocol surface
  (position/vehicle sync details, ping keepalives, spawn negotiation). The handshake,
  identity, mod sync and fan-out are verified by an e2e test with scripted clients;
  full real-game sessions are the next milestone. The UI labels it EXPERIMENTAL.

## 12. What's Included (completing the original document's cut-off section)

The LAN Edition document ended mid-sentence at "## 📦 What's Included". Here is what a
BeamMP-LAN release actually ships:

1. **`beammp-lan` (or `beammp-lan.exe`)** — the single native binary:
   * Servers page — beacon + TCP scan discovery, favorites, manual entries, join
   * Host page — Relay Mode (offline) + BeamMP-Server instance manager (full compat),
     one-click BeamMP-Server and CobaltEssentials downloads, live consoles
   * Mods page — Client/Server mod management for every instance
   * Launcher page — game detection, BeamMP mod install/update, embedded launcher-core
   * Settings — identity (nickname + machine ID), roles editor, discovery tuning
2. **`README.md`** — honest offline-capability matrix, setup, protocol pointers
3. **`LICENSE`** — AGPL-3.0 (matches BeamMP's own license)
4. **This plan** — architecture, protocol research, claim→implementation audit
5. **CI artifacts** (from GitHub Actions): Linux x86_64 + Windows x64 zips, each
   containing the binary, README and LICENSE

*Not* included (deliberately): no BeamMP-Server binary bundled (downloaded on demand
from the official repo), no CobaltEssentials bundled (1-click install), no modified
BeamMP client mod (the stock mod is installed from the official backend), no installer
(the zip is xcopy-deployable).

## 13. Offline-capability matrix (also in README)

| Capability | Relay Mode | BeamMP-Server instance |
| --- | --- | --- |
| Discover servers | ✅ offline (beacon + scan) | ✅ offline (beacon + scan) |
| Host a session | ✅ offline | ✅ offline (after one-time AuthKey fetch) |
| Join a session | ✅ offline | ✅ offline (client auth is server-side) |
| Serve client mods | ✅ offline | ✅ offline |
| Install BeamMP mod (first time) | ❌ needs internet once | ❌ needs internet once |
| Full game-session fidelity | ⚠️ experimental | ✅ production |

## 14. v0.2.0 milestone verification

* `cargo test` — 30 tests, including `beacon::tests::e2e_announce_and_seek` (real UDP
  loopback: announcer → seeker finds it) and `relay::tests::e2e_relay_two_clients`
  (two scripted clients: handshake, local auth, ids, frame fan-out both directions,
  stats, events, mod file serving, path-traversal rejection).
* `cargo clippy --all-targets` — 0 warnings. `cargo fmt` — clean.
* Windows paths type-checked via `cargo check --target x86_64-pc-windows-msvc`
  (ureq shimmed out in the sandbox; CI does the real build).

## 15. Licensing

BeamMP-LAN is licensed under **AGPL-3.0-or-later** (same as BeamMP itself). It
interoperates with BeamMP-Server, BeamMP-Launcher and the BeamMP mod over their
documented network protocols but does not include their source. BeamMP is © BeamMP Ltd.
— this is a community LAN tool, not an official BeamMP product. CobaltEssentials is ©
its authors (prestonelam2003 et al.), downloaded on demand from its official releases.
