// BeamMP-LAN — all-in-one LAN manager for BeamMP.
// Copyright (C) 2026 BeamMP-LAN contributors.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published
// by the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

mod app;
mod discovery;
mod launcher;
mod launcher_core;
mod mod_manager;
mod net;
mod server_mgr;
mod state;
mod theme;

fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([980.0, 620.0])
            .with_title("BeamMP-LAN")
            .with_icon(load_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "BeamMP-LAN",
        native_options,
        Box::new(|cc| Ok(Box::new(app::BeamMpLanApp::new(cc)))),
    )
}

/// Generated app icon: orange disc with three white "LAN signal" bars.
fn load_icon() -> egui::IconData {
    const W: u32 = 64;
    const H: u32 = 64;
    let mut rgba = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            let fx = x as f32;
            let fy = y as f32;
            let in_circle = (fx - 31.5).hypot(fy - 31.5) <= 26.0;
            let is_bar = |bx: f32, w: f32, h: f32| -> bool {
                fx >= bx && fx < bx + w && fy >= 44.0 - h && fy < 44.0
            };
            let bars =
                is_bar(18.0, 7.0, 10.0) || is_bar(29.0, 7.0, 17.0) || is_bar(40.0, 7.0, 24.0);
            if !in_circle {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else if bars {
                rgba.extend_from_slice(&[232, 234, 237, 255]);
            } else {
                rgba.extend_from_slice(&[255, 125, 26, 255]);
            }
        }
    }
    egui::IconData {
        rgba,
        width: W,
        height: H,
    }
}
