//! The egui application: BeamNG-style dark UI with the five pages.

use eframe::egui;
use egui::{Align, Layout, RichText};

use crate::launcher_core::CoreEvent;
use crate::net::ServerInfo;
use crate::server_mgr::RunState;
use crate::state::{AppEvent, Page};
use crate::theme::{self, *};

pub struct BeamMpLanApp {
    pub state: crate::state::AppState,
    /// Cached "join in progress" address (Servers page → Join).
    join_addr: Option<String>,
    /// UI-only fields for the Host page wizard.
    wizard: InstanceWizard,
    /// UI-only fields for manual add.
    add_server: (String, String),
    /// Raw TOML editor state.
    raw_toml: Option<String>,
    /// mod manager: selected side + pending picker text.
    mods_side: crate::mod_manager::ModSide,
    /// screenshot automation plan (BEAMMP_LAN_SHOTS env), if any.
    shot_plan: Option<String>,
    shot_step: usize,
    /// screenshots actually saved so far (screenshot automation).
    shot_saved: usize,
}

struct InstanceWizard {
    name: String,
    port: u16,
    map: String,
    max_players: u32,
    max_cars: u32,
    tags: String,
    auth_key: String,
    open: bool,
}

impl Default for InstanceWizard {
    fn default() -> Self {
        Self {
            name: "Friday Night LAN".into(),
            port: crate::net::DEFAULT_SERVER_PORT,
            map: "/levels/gridmap_v2/info.json".into(),
            max_players: 8,
            max_cars: 4,
            tags: "Freeroam,LAN".into(),
            auth_key: String::new(),
            open: false,
        }
    }
}

impl BeamMpLanApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(cc);
        let mut state = crate::state::AppState::new();

        // Detect interfaces + game in the background.
        let tx = state.event_tx.clone();
        std::thread::spawn(move || {
            let g = crate::launcher::detect_game();
            let _ = tx.send(AppEvent::GameDetected(g));
        });

        state.interfaces = crate::discovery::list_interfaces();
        state.mod_state = state.game.as_ref().map(crate::launcher::mod_state);

        if state.cfg.auto_start_core {
            state.start_core();
        }
        state.start_scan();
        state.refresh_manual();

        let shot_plan = std::env::var("BEAMMP_LAN_SHOTS").ok();
        Self {
            state,
            join_addr: None,
            wizard: InstanceWizard::default(),
            add_server: (String::new(), String::new()),
            raw_toml: None,
            mods_side: crate::mod_manager::ModSide::Client,
            shot_plan,
            shot_step: 0,
            shot_saved: 0,
        }
    }

    /// If BEAMMP_LAN_SHOTS is set ("<outdir>,<pages like shml>"), advance one page per
    /// second, request a viewport screenshot, and save it when the Event arrives.
    /// Exits after the last page.
    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(spec) = self.shot_plan.clone() else {
            return;
        };
        let mut parts = spec.split(',');
        let outdir = parts.next().unwrap_or(".").to_string();
        let pages: Vec<char> = parts.next().unwrap_or("s").chars().collect();
        let step = self.shot_step;

        // Ask for a screenshot of the page currently on screen.
        if step > 0 && step <= pages.len() {
            let name = match pages[step - 1] {
                's' => "servers",
                'h' => "host",
                'm' => "mods",
                'l' => "launcher",
                'e' => "settings",
                _ => "page",
            };
            let path = format!("{outdir}/{name}.png");
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(path)));
        }

        if step < pages.len() {
            self.state.page = match pages[step] {
                's' => Page::Servers,
                'h' => Page::Host,
                'm' => Page::Mods,
                'l' => Page::Launcher,
                'e' => Page::Settings,
                _ => Page::Servers,
            };
            self.shot_step = step + 1;
        } else {
            self.shot_step = step + 1;
        }
    }

    /// Collect Event::Screenshot results (delivered a frame after the command).
    fn collect_screenshots(&mut self, ctx: &egui::Context) {
        if self.shot_plan.is_none() {
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            if let egui::Event::Screenshot {
                image, user_data, ..
            } = ev
            {
                if let Some(arc) = user_data.data.as_ref() {
                    if let Some(path) = arc.downcast_ref::<String>() {
                        let _ = crate::state::save_png(
                            image.as_raw(),
                            image.width() as u32,
                            image.height() as u32,
                            path,
                        );
                        // count saved shots; exit only once every requested page was
                        // actually captured (the event arrives a frame after the command)
                        self.shot_saved += 1;
                        let pages: Vec<char> = self
                            .shot_plan
                            .as_deref()
                            .unwrap_or("s")
                            .split(',')
                            .nth(1)
                            .unwrap_or("s")
                            .chars()
                            .collect();
                        if self.shot_saved >= pages.len() {
                            std::process::exit(0);
                        }
                    }
                }
            }
        }
    }
}

impl eframe::App for BeamMpLanApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_events(ctx);
        self.state.interfaces = crate::discovery::list_interfaces();
        self.state.pump_relay();

        // Auto-refresh: rescan periodically.
        if let Some(last) = self.state.last_scan {
            if self.state.cfg.auto_refresh_secs > 0
                && !self.state.scanning
                && last.elapsed().as_secs() >= self.state.cfg.auto_refresh_secs
            {
                self.state.start_scan();
                self.state.refresh_manual();
            }
        }

        egui::TopBottomPanel::top("topbar").show(ctx, |ui| {
            self.topbar(ui);
        });

        egui::SidePanel::left("nav")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(egui::Stroke::new(1.0f32, BORDER)),
            )
            .exact_width(190.0)
            .show(ctx, |ui| {
                self.nav(ui);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| match self.state.page {
                Page::Servers => self.page_servers(ui),
                Page::Host => self.page_host(ui),
                Page::Mods => self.page_mods(ui),
                Page::Launcher => self.page_launcher(ui),
                Page::Settings => self.page_settings(ui),
            });

        self.toasts(ctx);

        // Screenshot automation: advance one step per second.
        self.collect_screenshots(ctx);
        static SHOT_CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tick = SHOT_CLOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.shot_plan.is_some() && tick.is_multiple_of(2) {
            self.screenshot_step(ctx);
        }

        // Keep the UI alive while scans/relays run.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

impl BeamMpLanApp {
    // — chrome ——————————————————————————————————————————————————————————

    fn topbar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(PANEL)
            .stroke(egui::Stroke::new(1.0f32, BORDER))
            .inner_margin(egui::Margin::symmetric(16, 8))
            .show(ui, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new("BEAMMP").color(ACCENT).strong().size(20.0));
                    ui.label(RichText::new("-LAN").color(TEXT).strong().size(20.0));
                    ui.add_space(16.0);
                    let ifs = self.state.interfaces.len();
                    ui.label(
                        RichText::new(format!(
                            "{ifs} network{} detected",
                            if ifs == 1 { "" } else { "s" }
                        ))
                        .color(TEXT_DIM)
                        .small(),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.state.core_running {
                        pill(ui, "LAUNCHER CORE ONLINE", GREEN);
                    } else {
                        pill(ui, "LAUNCHER CORE OFFLINE", RED);
                    }
                    ui.add_space(8.0);
                    if self.state.relay.is_some() {
                        pill(ui, "RELAY MODE ON", ACCENT);
                        ui.add_space(8.0);
                    }
                    if self.state.scanning || self.state.beacon_scanning {
                        pill(ui, "SCANNING…", WARN);
                    } else {
                        let n = self.state.all_servers().len();
                        pill(
                            ui,
                            &format!(
                                "{n} SERVER{} FOUND{}",
                                if n == 1 { "" } else { "S" },
                                if self.state.beacons.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · {} VIA BEACON", self.state.beacons.len())
                                }
                            ),
                            SELECTION,
                        );
                    }
                });
            });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.vertical(|ui| {
            let items: [(Page, &str, &str); 5] = [
                (Page::Servers, "⌖", "SERVERS"),
                (Page::Host, "▣", "HOST"),
                (Page::Mods, "⛁", "MODS"),
                (Page::Launcher, "▶", "LAUNCHER"),
                (Page::Settings, "⚙", "SETTINGS"),
            ];
            for (page, icon, label) in items {
                let selected = self.state.page == page;
                let frame = egui::Frame::new()
                    .fill(if selected { ACCENT_DIM } else { PANEL })
                    .stroke(if selected {
                        egui::Stroke::new(1.0f32, ACCENT)
                    } else {
                        egui::Stroke::NONE
                    })
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(12, 8));
                let resp = frame
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(icon)
                                    .color(if selected { ACCENT } else { TEXT_DIM })
                                    .size(16.0),
                            );
                            ui.label(
                                RichText::new(label)
                                    .color(if selected { TEXT } else { TEXT_DIM })
                                    .strong()
                                    .size(14.0),
                            );
                        })
                        .response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                    })
                    .inner;
                if resp.clicked() {
                    self.state.page = page;
                }
            }

            ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("LAN edition · AGPL-3.0")
                        .color(TEXT_DIM)
                        .small(),
                );
                ui.label(
                    RichText::new(format!("core v{}", crate::launcher_core::CORE_VERSION))
                        .color(TEXT_DIM)
                        .small(),
                );
            });
        });
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        let now = std::time::Instant::now();
        self.state
            .toasts
            .retain(|(_, t)| now.duration_since(*t).as_secs() < 6);
        if self.state.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(PANEL_RAISED)
                    .stroke(egui::Stroke::new(1.0f32, ACCENT))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        for (msg, _) in self.state.toasts.clone() {
                            ui.label(RichText::new(msg).color(TEXT).small());
                        }
                    })
            });
    }

    // — event pump ——————————————————————————————————————————————————————

    fn pump_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.state.event_rx.try_recv() {
            match ev {
                AppEvent::ScanDone(found) => {
                    self.state.scanning = false;
                    self.state.discovered = found;
                    self.state.last_scan = Some(std::time::Instant::now());
                    self.state.push_server_list_to_core();
                }
                AppEvent::ScanProgress { scanned, total } => {
                    self.state.scan_progress = (scanned, total);
                }
                AppEvent::BeaconDone(found) => {
                    self.state.beacon_scanning = false;
                    self.state.beacons = found;
                    self.state.push_server_list_to_core();
                }
                AppEvent::ServerInfo(info) => {
                    if let Some(existing) = self
                        .state
                        .manual_infos
                        .iter_mut()
                        .find(|i| i.addr == info.addr)
                    {
                        *existing = *info;
                    } else {
                        self.state.manual_infos.push(*info);
                    }
                    self.state.push_server_list_to_core();
                }
                AppEvent::Toast(msg) => self.state.toast(msg),
                AppEvent::DownloadDone { what, path } => {
                    self.state.downloading_server = false;
                    if what == "server" {
                        self.state.server_binary = Some(std::path::PathBuf::from(&path));
                    }
                    self.state.toast(format!("Downloaded {what} → {path}"));
                }
                AppEvent::DownloadError { what, error } => {
                    self.state.downloading_server = false;
                    self.state.toast(format!("{what} download failed: {error}"));
                }
                AppEvent::GameDetected(game) => {
                    self.state.game = game;
                    self.state.mod_state = self.state.game.as_ref().map(crate::launcher::mod_state);
                }
            }
        }

        // console events from running server processes
        while let Ok(ev) = self.state.console_rx.try_recv() {
            match ev {
                crate::server_mgr::ConsoleEvent::Line(l) => {
                    // route to instance: we tag lines via the sender; simplest: broadcast to the
                    // currently running instance with matching console output.
                    if let Some((id, _)) = self
                        .state
                        .runtimes
                        .iter()
                        .find(|(_, rt)| rt.state == RunState::Running)
                        .map(|(id, rt)| (id.clone(), rt.console.len()))
                    {
                        if let Some(rt) = self.state.runtimes.get_mut(&id) {
                            rt.push_line(&l);
                        }
                    }
                }
            }
        }

        // core events
        for ev in self.state.core.drain_events() {
            match ev {
                CoreEvent::Log(l) => {
                    self.state.core_logs.push(l.clone());
                    if self.state.core_logs.len() > 500 {
                        let drop = self.state.core_logs.len() - 500;
                        self.state.core_logs.drain(..drop);
                    }
                }
                CoreEvent::Joined { addr, name } => {
                    self.state.toast(format!("Connected to {name} ({addr})"));
                }
                CoreEvent::Joining { addr } => {
                    self.state.toast(format!("Joining {addr} …"));
                }
                CoreEvent::Disconnected { reason } => {
                    self.state.toast(format!("Disconnected: {reason}"));
                }
                CoreEvent::GameConnected | CoreEvent::GameDisconnected => {}
            }
        }

        // poll running servers for exit (try_wait)
        let mut exits = Vec::new();
        for (id, rt) in self.state.runtimes.iter_mut() {
            if let Some(r) = rt.running.as_mut() {
                match r.child.try_wait() {
                    Ok(Some(status)) => {
                        exits.push((id.clone(), format!("Server exited: {status}")));
                    }
                    Ok(None) => {}
                    Err(e) => exits.push((id.clone(), format!("Server error: {e}"))),
                }
            }
        }
        for (id, msg) in exits {
            if let Some(rt) = self.state.runtimes.get_mut(&id) {
                rt.state = RunState::Stopped;
                rt.running = None;
                rt.console.push(msg.clone());
            }
            self.state.toast(msg);
        }

        ctx.request_repaint();
    }

    // — pages ————————————————————————————————————————————————————————————

    fn page_servers(&mut self, ui: &mut egui::Ui) {
        ui.heading(
            RichText::new("SERVERS ON YOUR LAN")
                .strong()
                .size(theme::HEADING_SIZE),
        );
        ui.label(
            RichText::new(
                "BeamMP servers on your networks — discovered instantly via UDP beacon, with a TCP subnet scan as fallback. No internet needed. Works with Radmin VPN, ZeroTier, Tailscale and physical LANs.",
            )
            .color(TEXT_DIM)
            .small(),
        );
        ui.add_space(10.0);

        // toolbar
        ui.horizontal(|ui| {
            if self.state.scanning {
                let (s, t) = self.state.scan_progress;
                let pct = if t > 0 { s as f32 / t as f32 } else { 0.0 };
                let bar = egui::ProgressBar::new(pct).show_percentage();
                ui.add(bar.desired_width(220.0));
                ui.label(
                    RichText::new(format!("{s}/{t} hosts"))
                        .color(TEXT_DIM)
                        .small(),
                );
            } else if action_button(ui, "RESCAN") {
                self.state.start_scan();
                self.state.refresh_manual();
            }
            ui.add_space(8.0);
            if ghost_button(ui, "+ ADD SERVER") {
                self.add_server = (String::new(), String::new());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "Your addresses: {}",
                        self.state
                            .interfaces
                            .iter()
                            .map(|i| i.ip.to_string())
                            .collect::<Vec<_>>()
                            .join("  ·  ")
                    ))
                    .color(TEXT_DIM)
                    .small(),
                );
            });
        });
        ui.add_space(4.0);

        // manual add popup
        ui.collapsing("Add a server manually", |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("IP:PORT").color(TEXT_DIM).small());
                let mut ep = self.add_server.0.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut ep)
                        .desired_width(200.0)
                        .hint_text("26.3.7.9:30814"),
                );
                if resp.changed() {
                    self.add_server.0 = ep;
                }
                if action_button(ui, "ADD") {
                    let ep = self.add_server.0.trim().to_string();
                    if !ep.is_empty() {
                        let full = if ep.contains(':') {
                            ep.clone()
                        } else {
                            format!("{ep}:{}", crate::net::DEFAULT_SERVER_PORT)
                        };
                        if !self
                            .state
                            .cfg
                            .manual_servers
                            .iter()
                            .any(|m| m.endpoint == full)
                        {
                            self.state
                                .cfg
                                .manual_servers
                                .push(crate::state::ManualServer {
                                    endpoint: full.clone(),
                                    label: "Manual".into(),
                                });
                            self.state.save_config();
                            self.state.refresh_manual();
                        }
                    }
                }
            });
        });

        // server cards
        let servers = self.state.all_servers();
        if servers.is_empty() && !self.state.scanning {
            card(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(10.0);
                    ui.label(RichText::new("No servers found (yet)").color(TEXT).strong());
                    ui.label(
                        RichText::new(
                            "Start one on the HOST page, or make sure the host's firewall allows TCP 30814.\n\
                             With Radmin VPN, add the server by its 26.x address if discovery can't see it.",
                        )
                        .color(TEXT_DIM)
                        .small(),
                    );
                    ui.add_space(10.0);
                });
            });
        }

        for srv in servers.iter() {
            let is_fav = self.state.cfg.favorites.contains(&srv.addr);
            let selected = self.join_addr.as_deref() == Some(srv.addr.as_str());
            egui::Frame::new()
                .fill(if selected { PANEL_HOVER } else { PANEL })
                .stroke(if selected {
                    egui::Stroke::new(1.0f32, SELECTION)
                } else {
                    egui::Stroke::new(1.0f32, BORDER)
                })
                .corner_radius(8)
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&srv.name).color(TEXT).strong().size(15.0));
                                if is_fav {
                                    ui.label(RichText::new("★").color(WARN));
                                }
                            });
                            ui.label(
                                RichText::new(format!(
                                    "{}  ·  {}  ·  {}",
                                    srv.addr,
                                    pretty_map(&srv.map),
                                    srv.version
                                ))
                                .color(TEXT_DIM)
                                .small(),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if action_button(ui, "JOIN") {
                                self.join_addr = Some(srv.addr.clone());
                                self.do_join(srv);
                            }
                            ui.add_space(6.0);
                            let fav_label = if is_fav {
                                "★ FAVORITE"
                            } else {
                                "☆ FAVORITE"
                            };
                            if ghost_button(ui, fav_label) {
                                if is_fav {
                                    self.state.cfg.favorites.retain(|f| f != &srv.addr);
                                } else {
                                    self.state.cfg.favorites.push(srv.addr.clone());
                                }
                                self.state.save_config();
                            }
                            ui.add_space(6.0);
                            pill(
                                ui,
                                &format!("{}/{} PLAYERS", srv.players, srv.maxplayers),
                                if srv.players > 0 { SELECTION } else { TEXT_DIM },
                            );
                        });
                    });
                });
        }
    }

    fn do_join(&mut self, srv: &ServerInfo) {
        // Ensure core is up, then send the join through our relay: the game is expected
        // to be running & connected to the core. If the game isn't running, launch it.
        if !self.state.core_running {
            self.state.start_core();
        }
        // record recent
        if !self.state.cfg.recents.contains(&srv.addr) {
            self.state.cfg.recents.push(srv.addr.clone());
            if self.state.cfg.recents.len() > 20 {
                self.state.cfg.recents.remove(0);
            }
            self.state.save_config();
        }
        // If game not yet detected/running, launch it — the mod auto-connects through the
        // core once it sees the server in its list.
        let game = self.state.game.clone();
        let addr = srv.addr.clone();
        let tx = self.state.event_tx.clone();
        std::thread::spawn(move || {
            if let Some(g) = game {
                let _ = tx.send(AppEvent::Toast(format!(
                    "Launching {} — pick “{}” in the in-game multiplayer server list (LAN section)",
                    g.game_version, addr
                )));
                let _ = crate::launcher::launch_game(&g, &[]);
            } else {
                let _ = tx.send(AppEvent::Toast(
                    "BeamNG.drive not detected — set the install path on the LAUNCHER page."
                        .to_string(),
                ));
            }
        });
    }

    fn page_host(&mut self, ui: &mut egui::Ui) {
        ui.heading(
            RichText::new("HOST A SERVER")
                .strong()
                .size(theme::HEADING_SIZE),
        );
        ui.label(
            RichText::new(
                "Two ways to host: Relay Mode (built-in, fully offline — no AuthKey, no internet) or a BeamMP-Server instance (full compatibility).",
            )
            .color(TEXT_DIM)
            .small(),
        );
        ui.add_space(10.0);

        // Relay Mode card
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("RELAY MODE").color(ACCENT).strong());
                if let Some(r) = &self.state.relay {
                    let stats = r.stats();
                    let max_players = r.config.max_players;
                    pill(ui, "RUNNING", GREEN);
                    pill(
                        ui,
                        &format!(
                            "{}/{} PLAYERS · {} FRAMES RELAYED",
                            stats.clients, max_players, stats.frames_relayed
                        ),
                        SELECTION,
                    );
                } else {
                    pill(ui, "OFF", TEXT_DIM);
                    pill(ui, "EXPERIMENTAL", WARN);
                }
            });
            ui.label(
                RichText::new(
                    "The embedded server: local identities (nickname + machine ID), zero config, no BeamMP-Server binary. \
                     Best for air-gapped LANs. The first person to host on this machine becomes Owner automatically.",
                )
                .color(TEXT_DIM)
                .small(),
            );
            ui.add_space(4.0);
            let instance_for_relay =
                self.state
                    .selected_instance
                    .or(if self.state.instances.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
            ui.horizontal(|ui| {
                if let Some(idx) = instance_for_relay {
                    if self.state.relay.is_some() {
                        if danger_button(ui, "STOP RELAY") {
                            self.state.stop_relay();
                        }
                    } else if action_button(ui, "START RELAY") {
                        self.state.start_relay(idx);
                    }
                } else {
                    ui.label(
                        RichText::new("Create an instance first (it holds the map/mods config).")
                            .color(TEXT_DIM)
                            .small(),
                    );
                }
                if ghost_button(ui, "ANNOUNCE VIA BEACON") {
                    self.state.start_announcer();
                }
            });
            if !self.state.relay_events.is_empty() {
                // players with roles + crown for the owner
                let stats = self.state.relay.as_ref().map(|r| r.stats());
                if let Some(st) = stats.filter(|s| s.clients > 0) {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("{} player(s) connected", st.clients))
                            .color(TEXT)
                            .small(),
                    );
                }
                let owner_crown = self
                    .state
                    .accounts
                    .role_of(&self.state.identity)
                    .has_crown();
                if owner_crown {
                    ui.label(
                        RichText::new("👑 you are the Owner of this host")
                            .color(WARN)
                            .small(),
                    );
                }
                egui::CollapsingHeader::new(RichText::new("Relay log").color(TEXT_DIM).small())
                    .default_open(false)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(180.0)
                            .show(ui, |ui| {
                                for l in self.state.relay_events.iter().rev().take(60).rev() {
                                    ui.label(
                                        RichText::new(l).monospace().size(11.0).color(TEXT_DIM),
                                    );
                                }
                            });
                    });
            }
        });
        ui.add_space(8.0);

        // server binary availability
        ui.horizontal(|ui| {
            if self.state.resolve_server_binary().is_some() {
                pill(ui, "SERVER BINARY READY", GREEN);
                if let Some(p) = self.state.resolve_server_binary() {
                    ui.label(
                        RichText::new(p.display().to_string())
                            .color(TEXT_DIM)
                            .small(),
                    );
                }
            } else if self.state.downloading_server {
                pill(ui, "DOWNLOADING SERVER…", WARN);
            } else {
                pill(ui, "NO SERVER BINARY", TEXT_DIM);
                if action_button(ui, "DOWNLOAD BEAMMP-SERVER") {
                    self.state.downloading_server = true;
                    let tx = self.state.event_tx.clone();
                    let dir = crate::state::data_dir().join("server-bin");
                    std::thread::spawn(move || {
                        match crate::launcher::download_server_release(&dir) {
                            Ok(p) => {
                                let _ = tx.send(AppEvent::DownloadDone {
                                    what: "server".into(),
                                    path: p.display().to_string(),
                                });
                            }
                            Err(e) => {
                                let _ = tx.send(AppEvent::DownloadError {
                                    what: "server".into(),
                                    error: e,
                                });
                            }
                        }
                    });
                }
            }
            if self.state.downloading_server && ghost_button(ui, "INSTALL COBALTESSENTIALS") {
                self.install_cobaltessentials();
            }
        });
        ui.add_space(6.0);

        // instance list
        if self.state.instances.is_empty() {
            card(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new("No instances yet").color(TEXT).strong());
                    ui.label(
                        RichText::new("Create your first LAN server instance below.")
                            .color(TEXT_DIM)
                            .small(),
                    );
                    ui.add_space(8.0);
                });
            });
        }

        for idx in 0..self.state.instances.len() {
            self.instance_row(ui, idx);
        }

        ui.add_space(10.0);
        if !self.wizard.open {
            if action_button(ui, "+ NEW INSTANCE") {
                // suggest the next free port
                let used: Vec<u16> = self.state.instances.iter().map(|i| i.config.port).collect();
                let mut p = crate::net::DEFAULT_SERVER_PORT;
                while used.contains(&p) {
                    p += 1;
                }
                self.wizard.port = p;
                self.wizard.open = true;
            }
        } else {
            self.instance_wizard(ui);
        }
    }

    fn instance_row(&mut self, ui: &mut egui::Ui, idx: usize) {
        let inst = self.state.instances[idx].clone();
        let (state, players, console_len) = {
            let rt = self.state.runtimes.entry(inst.id.clone()).or_default();
            (rt.state, rt.players.len(), rt.console.len())
        };

        let selected = self.state.selected_instance == Some(idx);
        egui::Frame::new()
            .fill(if selected { PANEL_HOVER } else { PANEL })
            .stroke(if selected {
                egui::Stroke::new(1.0f32, SELECTION)
            } else {
                egui::Stroke::new(1.0f32, BORDER)
            })
            .corner_radius(8)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&inst.name).color(TEXT).strong().size(15.0));
                        ui.label(
                            RichText::new(format!(
                                "port {}  ·  {}  ·  {} cars  ·  Resources at {}",
                                inst.config.port,
                                pretty_map(&inst.config.map),
                                inst.config.max_cars,
                                inst.dir.display()
                            ))
                            .color(TEXT_DIM)
                            .small(),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        match state {
                            RunState::Running => {
                                if danger_button(ui, "STOP") {
                                    self.state.stop_instance(idx);
                                }
                                ui.add_space(6.0);
                                if ghost_button(ui, "RESTART") {
                                    self.state.stop_instance(idx);
                                    self.state.start_instance(idx);
                                }
                            }
                            RunState::Starting => {
                                pill(ui, "STARTING…", WARN);
                            }
                            RunState::Crashed => {
                                if action_button(ui, "START") {
                                    self.state.start_instance(idx);
                                }
                                ui.add_space(6.0);
                                pill(ui, "CRASHED", RED);
                            }
                            RunState::Stopped => {
                                if action_button(ui, "START") {
                                    self.state.start_instance(idx);
                                }
                            }
                        }
                        ui.add_space(6.0);
                        pill(
                            ui,
                            state.label(),
                            match state {
                                RunState::Running => GREEN,
                                RunState::Crashed => RED,
                                _ => TEXT_DIM,
                            },
                        );
                        ui.add_space(6.0);
                        if players > 0 {
                            pill(ui, &format!("{players} PLAYERS"), SELECTION);
                        }
                        let _ = console_len;
                    });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ghost_button(ui, "CONFIG") {
                        self.state.selected_instance = Some(idx);
                        self.raw_toml = Some(inst.config.to_toml_string());
                    }
                    if ghost_button(ui, "MODS") {
                        self.state.selected_instance = Some(idx);
                        self.state.page = Page::Mods;
                    }
                    if ghost_button(ui, "CONSOLE") {
                        self.state.selected_instance = Some(idx);
                    }
                    if ghost_button(ui, "OPEN FOLDER") {
                        let _ = open::that(&inst.dir);
                    }
                    if danger_button(ui, "DELETE") {
                        // stop if running
                        self.state.stop_instance(idx);
                        let _ = std::fs::remove_dir_all(&inst.dir);
                        self.state.instances.remove(idx);
                        self.state.selected_instance = None;
                    }
                });
            });

        // console drawer for the selected instance
        if self.state.selected_instance == Some(idx) {
            self.console_drawer(ui, idx);
            self.config_drawer(ui, idx);
        }
    }

    fn console_drawer(&mut self, ui: &mut egui::Ui, idx: usize) {
        let inst = self.state.instances[idx].clone();
        let rt = self.state.runtimes.entry(inst.id.clone()).or_default();
        egui::CollapsingHeader::new(RichText::new("CONSOLE").color(TEXT_DIM).small())
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .fill(BG_DEEP)
                            .corner_radius(4)
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                for line in &rt.console {
                                    ui.label(
                                        RichText::new(line)
                                            .color(if line.contains("[ERROR]") {
                                                RED
                                            } else if line.contains("[WARN]") {
                                                WARN
                                            } else {
                                                TEXT_DIM
                                            })
                                            .monospace()
                                            .size(11.5),
                                    );
                                }
                            });
                    });
                ui.horizontal(|ui| {
                    let mut cmd = rt.command_input.clone();
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut cmd)
                            .desired_width(ui.available_width() - 90.0)
                            .hint_text("type a command (list, status, say …) — Enter to send"),
                    );
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        // The official server reads stdin; we currently spawn with stdin null,
                        // so commands are unsupported until a stdin pipe is added (see PLAN §6.2).
                        rt.console.push(format!("> {cmd}"));
                        rt.console
                            .push("(command input requires stdin pipe — coming soon)".into());
                        cmd.clear();
                    }
                    rt.command_input = cmd;
                });
            });
    }

    fn config_drawer(&mut self, ui: &mut egui::Ui, idx: usize) {
        let inst = self.state.instances[idx].clone();
        let mut cfg = inst.config.clone();
        egui::CollapsingHeader::new(
            RichText::new("CONFIG (ServerConfig.toml)")
                .color(TEXT_DIM)
                .small(),
        )
        .default_open(false)
        .show(ui, |ui| {
            egui::Grid::new("cfg")
                .num_columns(2)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").color(TEXT_DIM));
                    ui.text_edit_singleline(&mut cfg.name);
                    ui.end_row();
                    ui.label(RichText::new("AuthKey").color(TEXT_DIM));
                    ui.text_edit_singleline(&mut cfg.auth_key);
                    ui.end_row();
                    ui.label(RichText::new("Port").color(TEXT_DIM));
                    ui.add(egui::DragValue::new(&mut cfg.port).range(1024..=65535));
                    ui.end_row();
                    ui.label(RichText::new("Map").color(TEXT_DIM));
                    let maps = crate::server_mgr::OFFICIAL_MAPS.to_vec();
                    let selected = maps.iter().position(|(p, _)| *p == cfg.map).unwrap_or(0);
                    let mut choice = selected;
                    egui::ComboBox::from_id_salt("map_select")
                        .selected_text(maps.get(choice).map(|m| m.1).unwrap_or("Custom"))
                        .show_ui(ui, |ui| {
                            for (i, (_, label)) in maps.iter().enumerate() {
                                ui.selectable_value(&mut choice, i, *label);
                            }
                        });
                    if choice != selected
                        || (choice == 0 && !maps.iter().any(|(p, _)| *p == cfg.map))
                    {
                        cfg.map = maps[choice].0.to_string();
                    }
                    ui.end_row();
                    ui.label(RichText::new("Max players").color(TEXT_DIM));
                    ui.add(egui::DragValue::new(&mut cfg.max_players).range(1..=200));
                    ui.end_row();
                    ui.label(RichText::new("Max cars / player").color(TEXT_DIM));
                    ui.add(egui::DragValue::new(&mut cfg.max_cars).range(1..=20));
                    ui.end_row();
                    ui.label(RichText::new("Tags").color(TEXT_DIM));
                    ui.text_edit_singleline(&mut cfg.tags);
                    ui.end_row();
                    ui.label(RichText::new("Description").color(TEXT_DIM));
                    ui.text_edit_singleline(&mut cfg.description);
                    ui.end_row();
                    ui.label(RichText::new("Private (not listed)").color(TEXT_DIM));
                    ui.checkbox(&mut cfg.private, "");
                    ui.end_row();
                    ui.label(RichText::new("Allow guests").color(TEXT_DIM));
                    ui.checkbox(&mut cfg.allow_guests, "");
                    ui.end_row();
                    ui.label(RichText::new("Info packet").color(TEXT_DIM));
                    ui.checkbox(&mut cfg.information_packet, "");
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if action_button(ui, "SAVE CONFIG") {
                    self.state.instances[idx].config = cfg.clone();
                    let _ = self.state.instances[idx].save_config();
                    self.state.toast("Config saved".to_string());
                }
                if ghost_button(ui, "RAW TOML") {
                    self.raw_toml = Some(cfg.to_toml_string());
                }
            });
            if let Some(raw) = self.raw_toml.clone() {
                let mut r = raw;
                egui::CollapsingHeader::new(RichText::new("Raw TOML").color(TEXT_DIM).small())
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut r)
                                .desired_rows(16)
                                .code_editor(),
                        );
                        if action_button(ui, "APPLY RAW") {
                            match crate::server_mgr::ServerConfig::from_toml_str(&r) {
                                Ok(parsed) => {
                                    self.state.instances[idx].config = parsed;
                                    let _ = self.state.instances[idx].save_config();
                                    self.state.toast("Applied raw TOML".to_string());
                                }
                                Err(e) => self.state.toast(format!("Invalid TOML: {e}")),
                            }
                        }
                    });
            }
        });
    }

    fn instance_wizard(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(RichText::new("NEW SERVER INSTANCE").color(ACCENT).strong());
            egui::Grid::new("wizard")
                .num_columns(2)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").color(TEXT_DIM));
                    ui.text_edit_singleline(&mut self.wizard.name);
                    ui.end_row();
                    ui.label(RichText::new("Port").color(TEXT_DIM));
                    ui.add(egui::DragValue::new(&mut self.wizard.port).range(1024..=65535));
                    ui.end_row();
                    ui.label(RichText::new("Map").color(TEXT_DIM));
                    egui::ComboBox::from_id_salt("wizard_map")
                        .selected_text(
                            crate::server_mgr::OFFICIAL_MAPS
                                .iter()
                                .find(|(p, _)| *p == self.wizard.map)
                                .map(|m| m.1)
                                .unwrap_or("Custom"),
                        )
                        .show_ui(ui, |ui| {
                            for (path, label) in crate::server_mgr::OFFICIAL_MAPS {
                                ui.selectable_value(&mut self.wizard.map, path.to_string(), *label);
                            }
                        });
                    ui.end_row();
                    ui.label(RichText::new("Max players").color(TEXT_DIM));
                    ui.add(egui::DragValue::new(&mut self.wizard.max_players).range(1..=200));
                    ui.end_row();
                    ui.label(RichText::new("Max cars / player").color(TEXT_DIM));
                    ui.add(egui::DragValue::new(&mut self.wizard.max_cars).range(1..=20));
                    ui.end_row();
                    ui.label(RichText::new("Tags").color(TEXT_DIM));
                    ui.text_edit_singleline(&mut self.wizard.tags);
                    ui.end_row();
                    ui.label(RichText::new("AuthKey").color(TEXT_DIM));
                    ui.text_edit_singleline(&mut self.wizard.auth_key);
                    ui.end_row();
                });
            ui.label(
                RichText::new(
                    "AuthKey: get a free key at keymaster.beammp.com (needs internet once).",
                )
                .color(TEXT_DIM)
                .small(),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if action_button(ui, "CREATE") {
                    let cfg = crate::server_mgr::ServerConfig {
                        name: self.wizard.name.clone(),
                        port: self.wizard.port,
                        map: self.wizard.map.clone(),
                        max_players: self.wizard.max_players,
                        max_cars: self.wizard.max_cars,
                        tags: self.wizard.tags.clone(),
                        auth_key: self.wizard.auth_key.trim().to_string(),
                        ..Default::default()
                    };
                    let base = crate::state::data_dir().join("servers");
                    match crate::server_mgr::ServerInstance::create(&base, &self.wizard.name, cfg) {
                        Ok(inst) => {
                            self.state
                                .runtimes
                                .insert(inst.id.clone(), Default::default());
                            self.state.instances.push(inst);
                            self.wizard.open = false;
                            self.state.toast("Instance created".to_string());
                        }
                        Err(e) => self.state.toast(format!("Create failed: {e}")),
                    }
                }
                if ghost_button(ui, "CANCEL") {
                    self.wizard.open = false;
                }
            });
        });
    }

    fn page_mods(&mut self, ui: &mut egui::Ui) {
        ui.heading(
            RichText::new("MOD MANAGER")
                .strong()
                .size(theme::HEADING_SIZE),
        );
        if self.state.instances.is_empty() {
            ui.label(
                RichText::new(
                    "Create a server instance on the HOST page first — mods live in its Resources folder.",
                )
                .color(TEXT_DIM)
                .small(),
            );
            return;
        }
        // instance selector
        ui.horizontal(|ui| {
            ui.label(RichText::new("INSTANCE").color(TEXT_DIM).small());
            let sel = self
                .state
                .selected_instance
                .unwrap_or(0)
                .min(self.state.instances.len() - 1);
            let names: Vec<String> = self
                .state
                .instances
                .iter()
                .map(|i| i.name.clone())
                .collect();
            egui::ComboBox::from_id_salt("mods_instance")
                .selected_text(names[sel].clone())
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut self.state.selected_instance, Some(i), n);
                    }
                });
            self.state.selected_instance = Some(sel);
        });
        let idx = self
            .state
            .selected_instance
            .unwrap_or(0)
            .min(self.state.instances.len() - 1);
        let inst = self.state.instances[idx].clone();

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let client_sel = self.mods_side == crate::mod_manager::ModSide::Client;
            let server_sel = self.mods_side == crate::mod_manager::ModSide::Server;
            if ui
                .add(egui::SelectableLabel::new(
                    client_sel,
                    RichText::new("CLIENT MODS (Resources/Client)").color(if client_sel {
                        ACCENT
                    } else {
                        TEXT_DIM
                    }),
                ))
                .clicked()
            {
                self.mods_side = crate::mod_manager::ModSide::Client;
            }
            if ui
                .add(egui::SelectableLabel::new(
                    server_sel,
                    RichText::new("SERVER PLUGINS (Resources/Server)").color(if server_sel {
                        ACCENT
                    } else {
                        TEXT_DIM
                    }),
                ))
                .clicked()
            {
                self.mods_side = crate::mod_manager::ModSide::Server;
            }
        });

        let dir = match self.mods_side {
            crate::mod_manager::ModSide::Client => inst.client_mods_dir(),
            crate::mod_manager::ModSide::Server => inst.server_mods_dir(),
        };
        let mods = crate::mod_manager::list_mods(&dir, self.mods_side);

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if action_button(ui, "INSTALL MOD (.ZIP)…") {
                if let Some(path) = rfd_pick_zip() {
                    match crate::mod_manager::install_zip(&path, &dir) {
                        Ok(dest) => {
                            self.state.toast(format!("Installed {}", dest.display()));
                        }
                        Err(e) => self.state.toast(format!("Install failed: {e}")),
                    }
                }
            }
            if ghost_button(ui, "OPEN FOLDER") {
                let _ = open::that(&dir);
            }
        });

        if mods.is_empty() {
            card(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "No {} yet — install a .zip or drop one into\n{}",
                            self.mods_side.label(),
                            dir.display()
                        ))
                        .color(TEXT_DIM)
                        .small(),
                    );
                });
            });
        }

        for m in &mods {
            egui::Frame::new()
                .fill(PANEL)
                .stroke(egui::Stroke::new(1.0f32, BORDER))
                .corner_radius(6)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&m.file_name).color(TEXT).strong());
                            ui.label(
                                RichText::new(format!(
                                    "{}  ·  {}{}",
                                    fmt_bytes(m.size),
                                    if m.enabled { "enabled" } else { "disabled" },
                                    if m.protected { "  ·  protected" } else { "" }
                                ))
                                .color(if m.enabled { TEXT_DIM } else { RED })
                                .small(),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if m.enabled {
                                if danger_button(ui, "DISABLE") {
                                    match crate::mod_manager::set_enabled(m, false) {
                                        Ok(_) => {
                                            self.state.toast(format!("Disabled {}", m.file_name))
                                        }
                                        Err(e) => self.state.toast(format!("Failed: {e}")),
                                    }
                                }
                            } else if action_button(ui, "ENABLE") {
                                match crate::mod_manager::set_enabled(m, true) {
                                    Ok(_) => self.state.toast(format!("Enabled {}", m.file_name)),
                                    Err(e) => self.state.toast(format!("Failed: {e}")),
                                }
                            }
                            if ghost_button(ui, "DELETE") {
                                let _ = std::fs::remove_file(&m.path);
                                self.state.toast(format!("Deleted {}", m.file_name));
                            }
                        });
                    });
                });
        }

        // note about server-side resource load
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Changes to client mods require a server restart; players re-download changed mods automatically (hash-checked).",
            )
            .color(TEXT_DIM)
            .small(),
        );
    }

    fn page_launcher(&mut self, ui: &mut egui::Ui) {
        ui.heading(
            RichText::new("GAME LAUNCHER")
                .strong()
                .size(theme::HEADING_SIZE),
        );
        ui.label(
            RichText::new(
                "BeamNG.drive detection and the BeamMP multiplayer mod. The embedded launcher-core serves the in-game server browser from the LAN.",
            )
            .color(TEXT_DIM)
            .small(),
        );
        ui.add_space(10.0);

        // game detection card
        card(ui, |ui| {
            ui.label(RichText::new("BEAMNG.DRIVE").color(ACCENT).strong());
            match &self.state.game {
                Some(g) => {
                    ui.label(
                        RichText::new(format!("Install: {}", g.game_dir.display()))
                            .color(TEXT)
                            .small(),
                    );
                    ui.label(
                        RichText::new(format!("User folder: {}", g.user_dir.display()))
                            .color(TEXT)
                            .small(),
                    );
                    ui.label(
                        RichText::new(format!(
                            "Version: {}  ·  found via {}",
                            g.game_version, g.method
                        ))
                        .color(TEXT_DIM)
                        .small(),
                    );
                }
                None => {
                    ui.label(
                        RichText::new(
                            "Not detected. Launch the game once, or set the path in Settings.",
                        )
                        .color(WARN)
                        .small(),
                    );
                }
            }
            ui.add_space(6.0);
            if ghost_button(ui, "RE-DETECT") {
                let g = crate::launcher::detect_game();
                self.state.game = g;
                self.state.mod_state = self.state.game.as_ref().map(crate::launcher::mod_state);
            }
        });
        ui.add_space(8.0);

        // beammp mod card
        card(ui, |ui| {
            ui.label(
                RichText::new("BEAMMP MULTIPLAYER MOD")
                    .color(ACCENT)
                    .strong(),
            );
            match (&self.state.game, &self.state.mod_state) {
                (Some(_), Some(ms)) => {
                    if ms.installed {
                        pill(ui, "INSTALLED", GREEN);
                        ui.label(
                            RichText::new(format!(
                                "{}  ·  sha256 {}…",
                                ms.zip_path
                                    .as_ref()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default(),
                                ms.hash
                                    .as_deref()
                                    .unwrap_or("?")
                                    .chars()
                                    .take(12)
                                    .collect::<String>()
                            ))
                            .color(TEXT_DIM)
                            .small(),
                        );
                    } else {
                        pill(ui, "NOT INSTALLED", RED);
                    }
                }
                _ => {
                    pill(ui, "GAME NOT FOUND", RED);
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if action_button(ui, "INSTALL / UPDATE MOD") {
                    let game = self.state.game.clone();
                    let tx = self.state.event_tx.clone();
                    std::thread::spawn(move || match crate::launcher::guest_login() {
                        Ok(pk) => match game {
                            Some(g) => match crate::launcher::download_mod(&g, &pk) {
                                Ok(p) => {
                                    let _ = tx.send(AppEvent::DownloadDone {
                                        what: "BeamMP mod".into(),
                                        path: p.display().to_string(),
                                    });
                                }
                                Err(e) => {
                                    let _ = tx.send(AppEvent::DownloadError {
                                        what: "BeamMP mod".into(),
                                        error: e,
                                    });
                                }
                            },
                            None => {
                                let _ = tx.send(AppEvent::Toast(
                                    "Set up the game install first.".to_string(),
                                ));
                            }
                        },
                        Err(e) => {
                            let _ = tx.send(AppEvent::DownloadError {
                                what: "BeamMP mod".into(),
                                error: e,
                            });
                        }
                    });
                }
                if self.state.game.is_some() && ghost_button(ui, "LAUNCH GAME") {
                    let g = self.state.game.clone().unwrap();
                    let tx = self.state.event_tx.clone();
                    std::thread::spawn(move || match crate::launcher::launch_game(&g, &[]) {
                        Ok(_) => {
                            let _ = tx.send(AppEvent::Toast("Game launched".to_string()));
                        }
                        Err(e) => {
                            let _ = tx.send(AppEvent::Toast(format!("Launch failed: {e}")));
                        }
                    });
                }
            });
        });
        ui.add_space(8.0);

        // launcher core card
        card(ui, |ui| {
            ui.label(
                RichText::new("EMBEDDED LAUNCHER CORE")
                    .color(ACCENT)
                    .strong(),
            );
            ui.label(
                RichText::new(format!(
                    "Listens on 127.0.0.1:{} — the in-game multiplayer UI connects here and gets the LAN server list. Session relay on the next port.",
                    self.state.cfg.core_port
                ))
                .color(TEXT_DIM)
                .small(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if self.state.core_running {
                    pill(ui, "RUNNING", GREEN);
                    if danger_button(ui, "STOP CORE") {
                        // core stop = currently only via app exit (threads are daemon-ish)
                        self.state
                            .toast("Core stops when the app closes.".to_string());
                    }
                } else if action_button(ui, "START CORE") {
                    self.state.start_core();
                }
            });
            ui.add_space(4.0);
            egui::CollapsingHeader::new(RichText::new("Core log").color(TEXT_DIM).small())
                .default_open(false)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(200.0)
                        .show(ui, |ui| {
                            for l in &self.state.core_logs {
                                ui.label(RichText::new(l).monospace().size(11.0).color(TEXT_DIM));
                            }
                        });
                });
        });
    }

    fn page_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading(RichText::new("SETTINGS").strong().size(theme::HEADING_SIZE));
        ui.add_space(8.0);

        // Identity card (nickname + machine id — the whole "account" system)
        card(ui, |ui| {
            ui.label(RichText::new("PLAYER IDENTITY").color(ACCENT).strong());
            ui.label(
                RichText::new(
                    "No accounts, no passwords: you are a nickname plus this machine's ID. Hosts assign roles per machine ID.",
                )
                .color(TEXT_DIM)
                .small(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Nickname").color(TEXT_DIM));
                let mut nick = self.state.identity.nickname.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut nick)
                        .desired_width(180.0)
                        .hint_text("Player"),
                );
                if resp.changed() {
                    self.state.identity.nickname = crate::identity::sanitize_nickname(&nick);
                }
                if resp.lost_focus() {
                    let dir = crate::identity::identity_dir(&crate::state::data_dir());
                    if self.state.identity.set_nickname(&dir, &nick).is_ok() {
                        self.state.cfg.nickname = self.state.identity.nickname.clone();
                        self.state.save_config();
                        self.state.toast("Nickname saved".to_string());
                    }
                }
                ui.label(
                    RichText::new(format!(
                        "machine ID {}…",
                        &self.state.identity.machine_id[..8]
                    ))
                    .color(TEXT_DIM)
                    .small(),
                );
            });
        });
        ui.add_space(8.0);

        // Roles card (host-side, accounts.json)
        card(ui, |ui| {
            ui.label(RichText::new("ROLES (THIS HOST)").color(ACCENT).strong());
            ui.label(
                RichText::new(
                    "Machine IDs that joined this host. Edit roles for Relay Mode sessions; the first-ever host became Owner automatically.",
                )
                .color(TEXT_DIM)
                .small(),
            );
            ui.add_space(4.0);
            if self.state.accounts.entries.is_empty() {
                ui.label(
                    RichText::new("Nobody has hosted from this machine yet — start Relay Mode to become Owner.")
                        .color(TEXT_DIM)
                        .small(),
                );
            } else {
                egui::Grid::new("roles")
                    .num_columns(3)
                    .spacing([14.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("MACHINE").color(TEXT_DIM).small().strong());
                        ui.label(RichText::new("NICKNAME").color(TEXT_DIM).small().strong());
                        ui.label(RichText::new("ROLE").color(TEXT_DIM).small().strong());
                        ui.end_row();
                        let entries: Vec<(String, crate::identity::AccountEntry)> = self
                            .state
                            .accounts
                            .entries
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        for (mid, entry) in entries {
                            ui.label(
                                RichText::new(format!("{}…", &mid[..8.min(mid.len())]))
                                    .color(TEXT_DIM)
                                    .small(),
                            );
                            ui.label(RichText::new(&entry.nickname).color(TEXT).small());
                            let mut role = entry.role;
                            egui::ComboBox::from_id_salt(format!("role_{mid}"))
                                .selected_text(role.label())
                                .show_ui(ui, |ui| {
                                    for r in [
                                        crate::identity::Role::Owner,
                                        crate::identity::Role::Admin,
                                        crate::identity::Role::Moderator,
                                        crate::identity::Role::Player,
                                        crate::identity::Role::Guest,
                                    ] {
                                        ui.selectable_value(&mut role, r, r.label());
                                    }
                                });
                            if role != entry.role {
                                self.state.accounts.entries.insert(
                                    mid.clone(),
                                    crate::identity::AccountEntry {
                                        nickname: entry.nickname.clone(),
                                        role,
                                    },
                                );
                                let _ = self.state.accounts.save(&crate::state::data_dir());
                            }
                            ui.end_row();
                        }
                    });
            }
        });
        ui.add_space(8.0);

        card(ui, |ui| {
            ui.label(RichText::new("DISCOVERY").color(ACCENT).strong());
            ui.horizontal(|ui| {
                let mut use_beacon = self.state.cfg.use_beacon;
                if ui
                    .checkbox(
                        &mut use_beacon,
                        "UDP beacon discovery (instant, broadcast-based)",
                    )
                    .changed()
                {
                    self.state.cfg.use_beacon = use_beacon;
                    self.state.save_config();
                }
            });
            egui::Grid::new("set_disc")
                .num_columns(2)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Auto-refresh (seconds, 0 = off)").color(TEXT_DIM));
                    ui.add(
                        egui::DragValue::new(&mut self.state.cfg.auto_refresh_secs).range(0..=120),
                    );
                    ui.end_row();
                    ui.label(RichText::new("Scan ports").color(TEXT_DIM));
                    let mut ports = self
                        .state
                        .cfg
                        .scan_ports
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    if ui.text_edit_singleline(&mut ports).changed() {
                        self.state.cfg.scan_ports = ports
                            .split(',')
                            .filter_map(|p| p.trim().parse().ok())
                            .collect();
                    }
                    ui.end_row();
                });
            ui.label(
                RichText::new("Interfaces (all scanned when none selected):")
                    .color(TEXT_DIM)
                    .small(),
            );
            for iface in self.state.interfaces.clone() {
                let mut selected = self.state.cfg.selected_interfaces.contains(&iface.name);
                if ui
                    .checkbox(
                        &mut selected,
                        format!("{} — {} ({})", iface.name, iface.ip, iface.hint),
                    )
                    .changed()
                {
                    if selected {
                        self.state.cfg.selected_interfaces.push(iface.name.clone());
                    } else {
                        self.state
                            .cfg
                            .selected_interfaces
                            .retain(|n| *n != iface.name);
                    }
                }
            }
        });
        ui.add_space(8.0);

        card(ui, |ui| {
            ui.label(RichText::new("LAUNCHER CORE").color(ACCENT).strong());
            egui::Grid::new("set_core")
                .num_columns(2)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Core port (default 4444)").color(TEXT_DIM));
                    ui.add(egui::DragValue::new(&mut self.state.cfg.core_port).range(1024..=65535));
                    ui.end_row();
                    ui.label(RichText::new("Start core automatically").color(TEXT_DIM));
                    ui.checkbox(&mut self.state.cfg.auto_start_core, "");
                    ui.end_row();
                });
            ui.label(
                RichText::new(
                    "If the official BeamMP launcher is running it occupies 4444 — close it or change the port here and set the same value in the game's launcherPort setting.",
                )
                .color(TEXT_DIM)
                .small(),
            );
        });
        ui.add_space(8.0);

        card(ui, |ui| {
            ui.label(RichText::new("SERVERS").color(ACCENT).strong());
            egui::Grid::new("set_srv")
                .num_columns(2)
                .spacing([14.0, 6.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("BeamMP-Server binary override").color(TEXT_DIM));
                    let mut ov = self.state.cfg.server_binary_override.clone();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut ov)
                                .hint_text("(auto-downloaded when empty)")
                                .desired_width(360.0),
                        )
                        .changed()
                    {
                        self.state.cfg.server_binary_override = ov;
                    }
                    ui.end_row();
                });
            ui.label(
                RichText::new(format!(
                    "Data directory: {}",
                    crate::state::data_dir().display()
                ))
                .color(TEXT_DIM)
                .small(),
            );
        });
        ui.add_space(8.0);

        card(ui, |ui| {
            ui.label(RichText::new("ABOUT").color(ACCENT).strong());
            ui.label(
                RichText::new(
                    "BeamMP-LAN — all-in-one LAN manager for BeamMP.\nCommunity tool, not affiliated with BeamMP Ltd. or BeamNG.\nLicense: AGPL-3.0-or-later. BeamMP (AGPL-3.0) © BeamMP Ltd.",
                )
                .color(TEXT_DIM)
                .small(),
            );
        });

        ui.add_space(8.0);
        if action_button(ui, "SAVE SETTINGS") {
            self.state.save_config();
            self.state.push_server_list_to_core();
            self.state.toast("Settings saved".to_string());
        }
    }

    /// Download CobaltEssentials (community BeamMP server plugin: admin commands,
    /// permissions, vehicle management) into the selected instance's Server mods.
    fn install_cobaltessentials(&mut self) {
        let Some(idx) = self
            .state
            .selected_instance
            .or(if self.state.instances.is_empty() {
                None
            } else {
                Some(0)
            })
        else {
            self.state
                .toast("Create a server instance first.".to_string());
            return;
        };
        let dir = self.state.instances[idx].server_mods_dir();
        let tx = self.state.event_tx.clone();
        self.state
            .toast("Downloading CobaltEssentials v1.7.6 …".to_string());
        std::thread::spawn(move || {
            let url = "https://github.com/prestonelam2003/CobaltEssentials/releases/download/v1.7.6/CobaltEssentials.zip";
            match ureq::get(url)
                .timeout(std::time::Duration::from_secs(60))
                .call()
            {
                Ok(resp) => {
                    let mut buf = Vec::new();
                    if std::io::Read::read_to_end(&mut resp.into_reader(), &mut buf).is_err()
                        || buf.len() < 1000
                    {
                        let _ = tx.send(AppEvent::DownloadError {
                            what: "CobaltEssentials".into(),
                            error: "download failed or too small".into(),
                        });
                        return;
                    }
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        let _ = tx.send(AppEvent::DownloadError {
                            what: "CobaltEssentials".into(),
                            error: e.to_string(),
                        });
                        return;
                    }
                    // The release zip contains the plugin zip(s); extract .zip members
                    // into Resources/Server directly.
                    let tmp = dir.join(".cobalt-download.zip");
                    if std::fs::write(&tmp, &buf).is_err() {
                        let _ = tx.send(AppEvent::DownloadError {
                            what: "CobaltEssentials".into(),
                            error: "cannot write temp file".into(),
                        });
                        return;
                    }
                    let file = match std::fs::File::open(&tmp) {
                        Ok(f) => f,
                        Err(e) => {
                            let _ = tx.send(AppEvent::DownloadError {
                                what: "CobaltEssentials".into(),
                                error: e.to_string(),
                            });
                            return;
                        }
                    };
                    let mut zip = match zip::ZipArchive::new(file) {
                        Ok(z) => z,
                        Err(e) => {
                            let _ = tx.send(AppEvent::DownloadError {
                                what: "CobaltEssentials".into(),
                                error: e.to_string(),
                            });
                            return;
                        }
                    };
                    let mut installed = 0;
                    for i in 0..zip.len() {
                        let Ok(mut entry) = zip.by_index(i) else {
                            continue;
                        };
                        if !entry.is_file() {
                            continue;
                        }
                        let name = entry.name().to_string();
                        if name.to_lowercase().ends_with(".zip") {
                            let dest = dir.join(
                                std::path::Path::new(&name)
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                            );
                            if let Ok(mut f) = std::fs::File::create(&dest) {
                                if std::io::copy(&mut entry, &mut f).is_ok() {
                                    installed += 1;
                                }
                            }
                        }
                    }
                    let _ = std::fs::remove_file(&tmp);
                    let _ = tx.send(AppEvent::DownloadDone {
                        what: format!(
                            "CobaltEssentials ({installed} plugin file{})",
                            if installed == 1 { "" } else { "s" }
                        ),
                        path: dir.display().to_string(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::DownloadError {
                        what: "CobaltEssentials".into(),
                        error: e.to_string(),
                    });
                }
            }
        });
    }
}

/// Native file picker (rfd would be the natural choice; we shell out to keep deps small).
fn rfd_pick_zip() -> Option<std::path::PathBuf> {
    // zenity/kdialog on Linux, PowerShell on Windows.
    #[cfg(unix)]
    {
        let out = std::process::Command::new("zenity")
            .args(["--file-selection", "--file-filter=ZIP archives | *.zip"])
            .output()
            .or_else(|_| {
                std::process::Command::new("kdialog")
                    .args(["--getopenfilename", ".", "*.zip"])
                    .output()
            })
            .ok()?;
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                return Some(std::path::PathBuf::from(p));
            }
        }
        None
    }
    #[cfg(not(unix))]
    {
        None // Windows picker ships with the msvc build via rfd later; CLI fallback below
    }
}
