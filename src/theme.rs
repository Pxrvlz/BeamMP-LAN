//! BeamNG.drive-inspired dark theme for BeamMP-LAN.
//!
//! Color tokens documented in docs/PLAN.md §3.

use eframe::egui;

pub const BG: egui::Color32 = egui::Color32::from_rgb(0x15, 0x18, 0x1d);
pub const BG_DEEP: egui::Color32 = egui::Color32::from_rgb(0x10, 0x13, 0x17);
pub const PANEL: egui::Color32 = egui::Color32::from_rgb(0x1b, 0x1f, 0x26);
pub const PANEL_RAISED: egui::Color32 = egui::Color32::from_rgb(0x23, 0x29, 0x33);
pub const PANEL_HOVER: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x31, 0x3d);
pub const BORDER: egui::Color32 = egui::Color32::from_rgb(0x2e, 0x35, 0x40);
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(0xe8, 0xea, 0xed);
pub const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0x9a, 0xa3, 0xad);
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0xff, 0x7d, 0x1a);
pub const ACCENT_DIM: egui::Color32 = egui::Color32::from_rgb(0xa8, 0x54, 0x16);
pub const SELECTION: egui::Color32 = egui::Color32::from_rgb(0x66, 0xd9, 0xff);
pub const GREEN: egui::Color32 = egui::Color32::from_rgb(0x67, 0xd1, 0x5b);
pub const RED: egui::Color32 = egui::Color32::from_rgb(0xe5, 0x48, 0x4d);
pub const WARN: egui::Color32 = egui::Color32::from_rgb(0xf2, 0xc7, 0x44);

pub const HEADING_SIZE: f32 = 15.0;

/// Install fonts (Noto Sans via system, DejaVu fallback) and apply the dark theme.
pub fn install_fonts(cc: &eframe::CreationContext<'_>) {
    let mut fonts = egui::FontDefinitions::default();

    // Prefer a real Noto Sans from disk if the OS ships it.
    let candidates: &[&str] = &[
        "/usr/share/fonts/truetype/chinese/NotoSansSC-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    for (i, path) in candidates.iter().enumerate() {
        if let Ok(bytes) = std::fs::read(path) {
            let family = format!("bmlan-sans-{i}");
            fonts
                .font_data
                .insert(family.clone(), egui::FontData::from_owned(bytes).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, family);
        }
    }

    cc.egui_ctx.set_fonts(fonts);
    apply_theme(&cc.egui_ctx);
}

/// Apply BeamNG-style styling to the context.
pub fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    // Widgets: dark chips with subtle borders, orange accent on hover/selection.
    for w in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        w.bg_fill = PANEL_RAISED;
        w.fg_stroke = egui::Stroke::new(1.0f32, TEXT);
        w.corner_radius = egui::CornerRadius::same(6);
        w.bg_stroke = egui::Stroke::new(1.0f32, BORDER);
    }
    style.visuals.widgets.hovered.bg_fill = PANEL_HOVER;
    style.visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0f32, ACCENT);
    style.visuals.widgets.active.bg_fill = PANEL_HOVER;
    style.visuals.widgets.active.fg_stroke = egui::Stroke::new(1.4f32, ACCENT);

    style.visuals.dark_mode = true;
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BG_DEEP;
    style.visuals.faint_bg_color = PANEL;
    style.visuals.code_bg_color = BG_DEEP;
    style.visuals.hyperlink_color = SELECTION;
    style.visuals.selection.bg_fill = ACCENT_DIM;
    style.visuals.selection.stroke = egui::Stroke::new(1.0f32, ACCENT);
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(0.5f32, BORDER);
    style.visuals.window_stroke = egui::Stroke::new(1.0f32, BORDER);

    // Spacing: roomy, card-like.
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    style.spacing.interact_size = egui::vec2(24.0, 30.0);

    // No drop shadows (flat BeamNG look).
    style.visuals.window_shadow = egui::Shadow::NONE;
    style.visuals.popup_shadow = egui::Shadow::NONE;

    ctx.set_style(style);
}

/// Small status pill (e.g. "ONLINE", "3/8 PLAYERS").
pub fn pill(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    let rich = egui::RichText::new(text).color(color).small().strong();
    let frame = egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(egui::Stroke::new(1.0f32, color.gamma_multiply(0.7)))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 2));
    frame.show(ui, |ui| ui.label(rich)).inner
}

/// Primary orange action button; returns true when clicked.
pub fn action_button(ui: &mut egui::Ui, label: &str) -> bool {
    let btn = egui::Button::new(
        egui::RichText::new(label)
            .color(egui::Color32::BLACK)
            .strong(),
    )
    .fill(ACCENT)
    .stroke(egui::Stroke::NONE)
    .corner_radius(4);
    ui.add(btn).clicked()
}

/// Secondary ghost button (dark chip, light border).
pub fn ghost_button(ui: &mut egui::Ui, label: &str) -> bool {
    let btn = egui::Button::new(egui::RichText::new(label).color(TEXT))
        .fill(PANEL_RAISED)
        .stroke(egui::Stroke::new(1.0f32, BORDER))
        .corner_radius(4);
    ui.add(btn).clicked()
}

/// Danger button (red).
pub fn danger_button(ui: &mut egui::Ui, label: &str) -> bool {
    let btn = egui::Button::new(
        egui::RichText::new(label)
            .color(egui::Color32::WHITE)
            .strong(),
    )
    .fill(RED)
    .stroke(egui::Stroke::NONE)
    .corner_radius(4);
    ui.add(btn).clicked()
}

/// BeamNG-style card frame.
pub fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(egui::Stroke::new(1.0f32, BORDER))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(12))
        .show(ui, add_contents);
}

/// Pretty-print a BeamMP map path like `/levels/gridmap_v2/info.json` → `Gridmap V2`.
pub fn pretty_map(map: &str) -> String {
    let m = map.trim().trim_start_matches('/');
    let m = m.strip_prefix("levels/").unwrap_or(m);
    let m = m.split('/').next().unwrap_or(m);
    let m = m.trim_end_matches(".json").trim_end_matches(".mis");
    let m = m.replace(['_', '-'], " ");
    let mut out = String::new();
    for (i, ch) in m.chars().enumerate() {
        if i == 0 || m.chars().nth(i - 1).is_some_and(|p| p == ' ') {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
    }
    out.trim().to_string()
}

/// Format bytes the human way.
pub fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {UNITS}", v = v, UNITS = UNITS[u])
    }
}
