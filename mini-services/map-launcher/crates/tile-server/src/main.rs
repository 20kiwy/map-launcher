//! map-launcher tile-server — the desktop app twin of the headless server.
//!
//! One engine (tile-server-core), two binaries:
//! - `tile-server`      → this window: a Map tab (tile canvas + the layer
//!   engine panel, in the spirit of vectorcraft's Layers panel) and a
//!   Downloads tab (Tanzania region cards, in the spirit of craft-launcher),
//!   while the HTTP server keeps serving :3100 in the background.
//! - `tile-server-cli`  → the same engine without a window (`serve`/`extract`).
//!
//! Subcommands keep release scripts working: `tile-server serve`, `tile-server
//! extract <pbf> <outdir> [w s e n]`, `--version`; `--screenshot <dir>` renders
//! both tabs headlessly (xvfb) for the versioned screenshots folder.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use carto_math::{lonlat_to_merc, merc_to_lonlat, world_px};
use eframe::egui;
use egui::{Align2, Color32, ColorImage, CornerRadius, FontId, Frame, Layout, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use tile_server_core::layers::{layer_catalog, LayerDef, LayerVis};
use tile_server_core::download::{pbf_name_for, Manager, Phase, Region, RegionReq};
use tile_server_core::{load_stack, render_tile_pixmap, spawn_server, App, ServerHandle};

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("tile-server {}", tile_server_core::VERSION);
        return Ok(());
    }
    if let Some(pos) = args.iter().position(|a| a == "extract") {
        let rest: Vec<String> = args[pos + 1..].to_vec();
        std::process::exit(tile_server_core::run_extract(&rest));
    }
    if let Some(pos) = args.iter().position(|a| a == "serve" || a == "--headless") {
        let mut port: u16 = 3100;
        let mut data_flag: Option<String> = None;
        let mut i = pos + 1;
        while i < args.len() {
            match args[i].as_str() {
                "--port" if i + 1 < args.len() => {
                    port = args[i + 1].parse().unwrap_or(3100);
                    i += 2;
                }
                "--data-dir" if i + 1 < args.len() => {
                    data_flag = Some(args[i + 1].clone());
                    i += 2;
                }
                _ => i += 1,
            }
        }
        std::process::exit(tile_server_core::run_headless(port, data_flag));
    }

    let mut port: u16 = 3100;
    let mut data_flag: Option<String> = None;
    let mut screenshot: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" if i + 1 < args.len() => {
                port = args[i + 1].parse().unwrap_or(3100);
                i += 2;
            }
            "--data-dir" if i + 1 < args.len() => {
                data_flag = Some(args[i + 1].clone());
                i += 2;
            }
            "--screenshot" if i + 1 < args.len() => {
                screenshot = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            _ => i += 1,
        }
    }

    let stack = load_stack(data_flag);
    stack.log();
    let app = tile_server_core::app_from_stack(stack);
    // Screenshot mode: size the window to fill the Xvfb screen at 0,0 so a
    // root-window grab captures exactly the app.
    let shot_size = screenshot.as_ref().map(|_| {
        let s = std::env::var("MAP_LAUNCHER_SHOT_SIZE").unwrap_or_else(|_| "1600x1000".into());
        let mut it = s.split('x');
        (
            it.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1600.0),
            it.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1000.0),
        )
    });
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!("map-launcher tile-server {}", tile_server_core::VERSION))
        .with_min_inner_size([980.0, 620.0]);
    viewport = match shot_size {
        Some((w, h)) => viewport.with_inner_size([w, h]).with_position([0.0, 0.0]),
        None => viewport.with_inner_size([1280.0, 800.0]),
    };
    let options = eframe::NativeOptions {
        viewport,
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "map-launcher tile-server",
        options,
        Box::new(move |cc| {
            theme::install(cc);
            Ok(Box::new(Shell::new(cc, app.clone(), port, screenshot)))
        }),
    )
}

/* --------------------------------------------------------------------- */
/* Theme (craft-launcher tokens, adapted to the map's accent)              */
/* --------------------------------------------------------------------- */

mod theme {
    use super::*;

    pub const BG: Color32 = Color32::from_rgb(0x17, 0x18, 0x1a);
    pub const PANEL: Color32 = Color32::from_rgb(0x1f, 0x21, 0x24);
    pub const PANEL_2: Color32 = Color32::from_rgb(0x26, 0x29, 0x2d);
    pub const BORDER: Color32 = Color32::from_rgb(0x33, 0x36, 0x3a);
    pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe4, 0xe0);
    pub const DIM: Color32 = Color32::from_rgb(0x9a, 0x98, 0x91);
    pub const ACCENT: Color32 = Color32::from_rgb(0xdc, 0x2a, 0x67);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x8e, 0x1d, 0x43);
    pub const OK: Color32 = Color32::from_rgb(0x4e, 0x9d, 0x55);
    pub const WARN: Color32 = Color32::from_rgb(0xc7, 0x86, 0x2b);
    pub const DANGER: Color32 = Color32::from_rgb(0xc3, 0x3b, 0x3b);
    pub const ROW_SEL: Color32 = Color32::from_rgb(0x33, 0x21, 0x28);

    /// Register the map's font faces as the UI font (system Noto first, the
    /// embedded DejaVu as the guaranteed fallback) and paint the dark style.
    pub fn install(cc: &eframe::CreationContext<'_>) {
        let mut defs = egui::FontDefinitions::default();
        let reg = carto_style::load_map_font(false)
            .or_else(|| carto_style::find_font("dejavu", false))
            .and_then(|p| std::fs::read(p).ok())
            .or_else(|| carto_style::embedded_font("DejaVu Sans", false).map(|(_, b)| b.to_vec()));
        let bold = carto_style::load_map_font(true)
            .or_else(|| carto_style::find_font("dejavu", true))
            .and_then(|p| std::fs::read(p).ok())
            .or_else(|| carto_style::embedded_font("DejaVu Sans", true).map(|(_, b)| b.to_vec()));
        if let Some(bytes) = reg {
            defs.font_data.insert("map-ui".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            // After egui's defaults: variable system fonts (NotoSansSC[wght])
            // can rasterise empty glyphs, so they must not lead the family.
            defs.families.entry(egui::FontFamily::Proportional).or_default().push("map-ui".into());
        }
        if let Some(bytes) = bold {
            defs.font_data.insert("map-ui-bold".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
            defs.families.entry(egui::FontFamily::Proportional).or_default().push("map-ui-bold".into());
        }
        cc.egui_ctx.set_fonts(defs);

        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        let mut v = egui::Visuals::dark();
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.extreme_bg_color = BG;
        v.faint_bg_color = PANEL_2;
        v.widgets.noninteractive.bg_fill = PANEL_2;
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.inactive.bg_fill = PANEL_2;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x2e, 0x32, 0x37);
        v.widgets.hovered.fg_stroke = Stroke::new(1.2, TEXT);
        v.widgets.active.bg_fill = ACCENT_DIM;
        v.widgets.active.fg_stroke = Stroke::new(1.2, TEXT);
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.override_text_color = Some(TEXT);
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, v);
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = Vec2::new(8.0, 6.0);
            style.spacing.button_padding = Vec2::new(10.0, 5.0);
        });
    }

    /// A small status pill.
    pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) {
        let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(11.5), color);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(galley.size().x + 14.0, 18.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(9), color.gamma_multiply(0.22));
        ui.painter().rect_stroke(rect, CornerRadius::same(9), Stroke::new(1.0, color), egui::StrokeKind::Inside);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(11.5), color);
    }

    /// The dot used for live statuses.
    pub fn dot(painter: &egui::Painter, c: Pos2, color: Color32) {
        painter.circle_filled(c, 4.0, color);
    }
}

/* --------------------------------------------------------------------- */
/* Tabs                                                                    */
/* --------------------------------------------------------------------- */

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Map,
    Downloads,
}

fn fmt_bytes(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1 << 30) as f64)
    } else if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / (1 << 20) as f64)
    } else if b >= 1 << 10 {
        format!("{:.0} KB", b as f64 / (1 << 10) as f64)
    } else {
        format!("{b} B")
    }
}

/// The screenshot plan for headless captures (`--screenshot <dir>`).
struct Capture {
    dir: PathBuf,
    frames: u32,
    stage: u32,
}

struct Shell {
    app: Arc<App>,
    tab: Tab,
    server: Option<ServerHandle>,
    server_running: bool,
    port: u16,
    data_version: Arc<AtomicU64>,

    map: MapState,
    layers: PanelState,
    downloads: DownloadsState,

    toast: Option<(String, Instant)>,
    capture: Option<Capture>,
}

impl Shell {
    fn new(cc: &eframe::CreationContext<'_>, app: Arc<App>, port: u16, screenshot: Option<PathBuf>) -> Self {
        // Auto-start the server: the GUI *is* a tile-server.
        let (server, server_running) = match spawn_server(port, app.clone()) {
            Ok(h) => (Some(h), true),
            Err(e) => {
                eprintln!("server start failed: {e} (port {port} busy?) — start it from the toolbar");
                (None, false)
            }
        };
        let bbox = app.store.read().unwrap().bbox;
        let data_version = Arc::new(AtomicU64::new(1));
        let map = MapState::new(bbox);
        let mut shell = Shell {
            app,
            tab: Tab::Map,
            server,
            server_running,
            port,
            data_version: data_version.clone(),
            map,
            layers: PanelState::default(),
            downloads: DownloadsState::new(),
            toast: None,
            capture: screenshot.map(|dir| Capture { dir, frames: 0, stage: 0 }),
        };
        shell.downloads.data_version = data_version;
        let _ = cc;
        shell
    }

    fn set_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Live job polling + auto store swap on completion.
        self.downloads.poll(ctx, &self.app, &self.data_version);

        // Expire the toast.
        if let Some((_, at)) = &self.toast {
            if at.elapsed() >= Duration::from_secs(4) {
                self.toast = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }

        // Headless screenshot capture: ask eframe for a real screenshot of
        // the painted frame (the reply arrives as an Event::Screenshot).
        if let Some(capture) = &mut self.capture {
            capture.frames += 1;
            match capture.stage {
                0 if capture.frames >= 70 => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(1u64)));
                    capture.stage = 1;
                    capture.frames = 0;
                }
                1 => {
                    if let Some(img) = take_screenshot_event(ctx) {
                        save_tab_screenshot(&capture.dir, "01-map-tab-layers.png", Some(&img));
                        eprintln!("screenshot saved: 01-map-tab-layers.png");
                        self.tab = Tab::Downloads;
                        capture.stage = 2;
                        capture.frames = 0;
                    } else if capture.frames > 120 {
                        eprintln!("screenshot 1 timed out");
                        capture.stage = 4;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else {
                        ctx.request_repaint_after(Duration::from_millis(50));
                    }
                }
                2 if capture.frames >= 35 => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(2u64)));
                    capture.stage = 3;
                    capture.frames = 0;
                }
                3 => {
                    if let Some(img) = take_screenshot_event(ctx) {
                        save_tab_screenshot(&capture.dir, "02-downloads-tab.png", Some(&img));
                        eprintln!("screenshot saved: 02-downloads-tab.png");
                        capture.stage = 4;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else if capture.frames > 120 {
                        eprintln!("screenshot 2 timed out");
                        capture.stage = 4;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else {
                        ctx.request_repaint_after(Duration::from_millis(50));
                    }
                }
                _ => ctx.request_repaint_after(Duration::from_millis(50)),
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let running = self.server_running;
        egui::Panel::left("rail").exact_size(64.0).resizable(false).frame(Frame::new().fill(theme::PANEL).stroke(Stroke::new(1.0, theme::BORDER))).show(ui, |ui| {
            ui.add_space(10.0);
            // Logo: two crossing roads on a tile.
            let (logo_rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(logo_rect, CornerRadius::same(9), theme::ACCENT_DIM);
            p.line_segment([logo_rect.left_center(), logo_rect.right_center()], Stroke::new(3.5, theme::TEXT));
            p.line_segment([logo_rect.center_top(), logo_rect.center_bottom()], Stroke::new(2.0, theme::ACCENT));
            ui.add_space(14.0);
            for (tab, icon, label) in [(Tab::Map, "🗺", "Map"), (Tab::Downloads, "⬇", "Data")] {
                let selected = self.tab == tab;
                let btn = egui::Button::new(egui::RichText::new(format!("{icon}\n{label}")).size(10.5).color(if selected { theme::TEXT } else { theme::DIM }))
                    .min_size(Vec2::new(52.0, 52.0))
                    .fill(if selected { theme::ACCENT_DIM } else { Color32::TRANSPARENT });
                if ui.add(btn).clicked() {
                    self.tab = tab;
                }
                ui.add_space(6.0);
            }
            ui.with_layout(Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                let (r, _) = ui.allocate_exact_size(Vec2::new(40.0, 16.0), Sense::hover());
                theme::dot(ui.painter(), Pos2::new(r.center().x - 8.0, r.center().y), if running { theme::OK } else { theme::DIM });
                ui.painter().text(
                    Pos2::new(r.center().x + 4.0, r.center().y),
                    Align2::LEFT_CENTER,
                    if running { "live" } else { "off" },
                    FontId::proportional(10.0),
                    theme::DIM,
                );
                ui.label(egui::RichText::new(format!("v{}", tile_server_core::VERSION)).size(10.0).color(theme::DIM));
            });
        });

        if self.tab == Tab::Map {
            egui::Panel::right("layers")
                .exact_size(324.0)
                .resizable(false)
                .frame(Frame::new().fill(theme::PANEL).stroke(Stroke::new(1.0, theme::BORDER)))
                .show(ui, |ui| {
                    self.layers.panel_ui(ui, &self.app, &self.data_version);
                });
        }
        egui::CentralPanel::default().frame(Frame::new().fill(theme::BG)).show(ui, |ui| match self.tab {
            Tab::Map => self.map_tab(ui),
            Tab::Downloads => self.downloads.tab_ui(ui, &self.app),
        });

        // Toast (bottom center).
        if let Some((msg, _at)) = &self.toast {
            let screen = ctx.input(|i| i.viewport_rect());
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("toast")));
            let text_size = painter.layout_no_wrap(msg.clone(), FontId::proportional(13.0), theme::TEXT).size();
            let rect = Rect::from_center_size(Pos2::new(screen.center().x, screen.bottom() - 42.0), Vec2::new(text_size.x + 28.0, 30.0));
            painter.rect_filled(rect, CornerRadius::same(15), theme::PANEL_2);
            painter.rect_stroke(rect, CornerRadius::same(15), Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Outside);
            painter.text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(13.0), theme::TEXT);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0x17 as f32 / 255.0, 0x18 as f32 / 255.0, 0x1a as f32 / 255.0, 1.0]
    }
}

impl Shell {
    fn map_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        // Toolbar
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Map").size(17.0).strong());
            ui.separator();
            let label = if self.server_running { "Stop server" } else { "Start server" };
            let (col, text) = if self.server_running { (theme::PANEL_2, theme::TEXT) } else { (theme::OK, Color32::WHITE) };
            if ui.add(egui::Button::new(egui::RichText::new(label).color(text)).fill(col)).clicked() {
                self.toggle_server();
            }
            let (dot_r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            theme::dot(ui.painter(), dot_r.center(), if self.server_running { theme::OK } else { theme::DIM });
            let url = format!("http://127.0.0.1:{}", self.port);
            ui.label(egui::RichText::new(&url).size(12.5).color(theme::DIM).monospace());
            if ui.small_button("Copy").clicked() {
                ui.ctx().copy_text(url.clone());
                self.set_toast("Server URL copied");
            }
            ui.separator();
            let stats = self.app.stats();
            ui.label(egui::RichText::new(format!(
                "{} roads · {} buildings · {} POIs",
                thousands(stats.roads),
                thousands(stats.buildings),
                thousands(stats.pois)
            ))
            .size(12.5)
            .color(theme::DIM));
            let region = self.app.region_name.lock().unwrap().clone();
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(16.0);
                ui.label(egui::RichText::new(region).size(12.5).strong());
            });
        });
        ui.add_space(6.0);

        // Canvas fills the rest (the layers live in the right-side panel).
        egui::Frame::new().fill(theme::PANEL).corner_radius(CornerRadius::same(8)).stroke(Stroke::new(1.0, theme::BORDER)).show(ui, |ui| {
            self.map.canvas_ui(ui, &self.app, &self.data_version, self.port);
        });
    }

    fn toggle_server(&mut self) {
        if self.server_running {
            if let Some(h) = self.server.take() {
                h.stop();
            }
            self.server_running = false;
            self.set_toast(format!("Server stopped (was :{})", self.port));
        } else {
            match spawn_server(self.port, self.app.clone()) {
                Ok(h) => {
                    self.server = Some(h);
                    self.server_running = true;
                    self.set_toast(format!("Serving tiles on :{}", self.port));
                }
                Err(e) => {
                    self.server_running = false;
                    self.set_toast(format!("Start failed: {e}"));
                }
            }
        }
    }
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/* --------------------------------------------------------------------- */
/* Map canvas                                                              */
/* --------------------------------------------------------------------- */

struct MapState {
    /// Fractional zoom (4..=19).
    zoom: f32,
    /// Center lon/lat.
    center: (f64, f64),
    tex: HashMap<(u8, u32, u32), (TextureHandle, u64)>,
}

impl MapState {
    fn new(bbox: [f64; 4]) -> Self {
        Self {
            zoom: 12.0,
            center: ((bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0),
            tex: HashMap::new(),
        }
    }

    fn canvas_ui(&mut self, ui: &mut egui::Ui, app: &Arc<App>, data_version: &Arc<AtomicU64>, port: u16) {
        ui.add_space(8.0);
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());

        // --- interaction ---------------------------------------------------
        let z_int = self.zoom.round().clamp(4.0, 19.0) as u8;
        let scale = world_px(z_int) as f64;
        if resp.dragged() {
            let d = resp.drag_delta();
            let (mut mx, mut my) = lonlat_to_merc(self.center.0, self.center.1);
            mx -= d.x as f64 / scale;
            my += d.y as f64 / scale;
            self.center = merc_to_lonlat(mx.clamp(0.0, 1.0), my.clamp(0.0, 1.0));
        }
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if resp.hovered() && scroll.abs() > 0.5 {
            self.zoom = (self.zoom + scroll * 0.01).clamp(4.0, 19.0);
        }
        if resp.double_clicked() {
            self.zoom = (self.zoom + 1.0).clamp(4.0, 19.0);
        }

        // --- paint ----------------------------------------------------------
        let painter = ui.painter_at(rect);
        let land = app.palette.get("land-color").unwrap_or(carto_style::Rgba::rgb(0xf2, 0xef, 0xe9));
        painter.rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(land.r, land.g, land.b));

        let layer_version = app.layers.read().unwrap().version;
        let dv = data_version.load(Ordering::Relaxed);
        let stamp = (layer_version << 8) | (dv & 0xff).min(0xff);

        let (cx_m, cy_m) = lonlat_to_merc(self.center.0, self.center.1);
        let world = scale;
        let top_left_m = (cx_m * world - rect.width() as f64 / 2.0, cy_m * world - rect.height() as f64 / 2.0);
        let tiles = 1u32 << z_int;
        let t0x = (top_left_m.0 / 256.0).floor().max(0.0) as u32;
        let t0y = (top_left_m.1 / 256.0).floor().max(0.0) as u32;
        let t1x = (((top_left_m.0 + rect.width() as f64) / 256.0).floor() as u32).min(tiles - 1);
        let t1y = (((top_left_m.1 + rect.height() as f64) / 256.0).floor() as u32).min(tiles - 1);

        let mut budget = 3usize;
        for ty in t0y..=t1y {
            for tx in t0x..=t1x {
                let px = (tx as f64 * 256.0 - top_left_m.0) as f32 + rect.left();
                let py = (ty as f64 * 256.0 - top_left_m.1) as f32 + rect.top();
                let dst = Rect::from_min_size(Pos2::new(px, py), Vec2::splat(256.0));
                let key = (z_int, tx, ty);
                let fresh = self.tex.get(&key).is_some_and(|(_, v)| *v == stamp);
                if !fresh && budget > 0 {
                    if let Some(pm) = render_tile_pixmap(app, &app.store.read().unwrap(), &app.layers.read().unwrap(), z_int, tx, ty) {
                        let (w, h) = (pm.width() as usize, pm.height() as usize);
                        let mut rgba = pm.data().to_vec();
                        unpremultiply(&mut rgba);
                        let img = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                        let handle = ui.ctx().load_texture(format!("tile-{z_int}-{tx}-{ty}"), img, TextureOptions::LINEAR);
                        self.tex.insert(key, (handle, stamp));
                        budget -= 1;
                    }
                }
                if let Some((handle, v)) = self.tex.get(&key) {
                    if *v == stamp {
                        painter.image(handle.id(), dst, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                    }
                }
                // Evict stale textures when the cache grows.
                if self.tex.len() > 96 {
                    self.tex.retain(|_, (_, v)| *v == stamp);
                }
            }
        }

        // --- overlays --------------------------------------------------------
        painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
        let badge = |painter: &egui::Painter, pos: Pos2, text: String, col: Color32| {
            let size = painter.layout_no_wrap(text.clone(), FontId::monospace(11.0), col).size();
            let r = Rect::from_min_size(pos + Vec2::new(8.0, 8.0), Vec2::new(size.x + 14.0, 22.0));
            painter.rect_filled(r, CornerRadius::same(6), theme::BG.gamma_multiply(0.85));
            painter.text(r.center(), Align2::CENTER_CENTER, text, FontId::monospace(11.0), col);
        };
        let region = app.region_name.lock().unwrap().clone();
        badge(&painter, rect.left_top(), region, theme::TEXT);
        badge(&painter, rect.left_bottom() + Vec2::new(0.0, -30.0), format!("z{:.1}", self.zoom), theme::ACCENT);
        painter.text(
            rect.right_bottom() + Vec2::new(-8.0, -6.0),
            Align2::RIGHT_BOTTOM,
            format!("tiles http://127.0.0.1:{port}/tile/{{z}}/{{x}}/{{y}}.png · © OpenStreetMap contributors"),
            FontId::proportional(10.5),
            theme::DIM,
        );

        // Zoom controls (right-bottom).
        let mut zui = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(12.0)));
        zui.with_layout(Layout::right_to_left(egui::Align::BOTTOM), |ui| {
            ui.vertical(|ui| {
                if ui.add(egui::Button::new("+").min_size(Vec2::splat(30.0))).clicked() {
                    self.zoom = (self.zoom + 1.0).clamp(4.0, 19.0);
                }
                if ui.add(egui::Button::new("−").min_size(Vec2::splat(30.0))).clicked() {
                    self.zoom = (self.zoom - 1.0).clamp(4.0, 19.0);
                }
                if ui.add(egui::Button::new("⌂").min_size(Vec2::splat(30.0))).clicked() {
                    let b = app.store.read().unwrap().bbox;
                    self.center = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
                    self.zoom = 12.0;
                }
            });
        });

        if budget < 3 {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }
}

/// egui wants straight alpha; tiny_skia stores premultiplied.
fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 255 || a == 0 {
            continue;
        }
        px[0] = ((px[0] as u32 * 255 + a / 2) / a).min(255) as u8;
        px[1] = ((px[1] as u32 * 255 + a / 2) / a).min(255) as u8;
        px[2] = ((px[2] as u32 * 255 + a / 2) / a).min(255) as u8;
    }
}

/* --------------------------------------------------------------------- */
/* Layers panel (vectorcraft layer-engine style)                           */
/* --------------------------------------------------------------------- */

/// Panel geometry, straight from the reference: eye column, colour bar,
/// disclosure chevron, name, then the zoom badge on the right.
const COLUMN: f32 = 25.0;
const INDENT: f32 = 14.0;
const ROW_H: f32 = 26.0;

#[derive(Default)]
struct PanelState {
    search: String,
    open: std::collections::HashSet<String>,
    selected: Option<String>,
    /// Property editors for the selected layer.
    edit_zoom: [i32; 2],
}

impl PanelState {
    fn panel_ui(&mut self, ui: &mut egui::Ui, app: &Arc<App>, data_version: &Arc<AtomicU64>) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Layers").size(15.5).strong());
            ui.separator();
            if ui.small_button("Show all").clicked() {
                let mut vis = app.layers.write().unwrap();
                vis.visible.clear();
                vis.touch();
                app.invalidate();
                data_version.fetch_add(1, Ordering::Relaxed);
            }
            if ui.small_button("Hide all").clicked() {
                let mut vis = app.layers.write().unwrap();
                for d in layer_catalog() {
                    vis.visible.insert(d.id.to_string(), false);
                }
                vis.touch();
                app.invalidate();
                data_version.fetch_add(1, Ordering::Relaxed);
            }
        });
        ui.add_space(4.0);
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search layers…").desired_width(f32::INFINITY));
        ui.add_space(4.0);

        let defs = layer_catalog();
        let vis_snapshot = app.layers.read().unwrap().clone();
        let query = self.search.trim().to_lowercase();

        // Row list (groups first, children indented).
        let mut rows: Vec<(&LayerDef, usize)> = Vec::new();
        for d in &defs {
            if d.parent.is_empty() {
                if !query.is_empty() {
                    // Match groups with any child hit too.
                    let children_hit = defs.iter().any(|c| c.parent == d.id && (c.id.contains(&query) || c.name.to_lowercase().contains(&query)));
                    if !(d.id.contains(&query) || d.name.to_lowercase().contains(&query) || children_hit) {
                        continue;
                    }
                }
                rows.push((d, 0));
                if self.open.contains(d.id) || !query.is_empty() {
                    for c in &defs {
                        if c.parent == d.id {
                            if !query.is_empty() && !(c.id.contains(&query) || c.name.to_lowercase().contains(&query)) {
                                continue;
                            }
                            rows.push((c, 1));
                        }
                    }
                }
            }
        }

        // The properties editor takes the bottom half; the tree scrolls on top.
        let total_h = ui.available_height() - 6.0;
        let list_h = if self.selected.is_some() { (total_h * 0.52).max(160.0) } else { total_h };

        egui::ScrollArea::vertical().id_salt("layer-rows").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (d, depth) in rows.iter() {
                self.row_ui(ui, d, *depth, &vis_snapshot, app, data_version);
            }
        });

        // Engine layer properties for the selected layer.
        if let Some(sel) = self.selected.clone() {
            ui.add_space(6.0);
            ui.separator();
            if let Some(def) = defs.iter().find(|d| d.id == sel) {
                self.properties_ui(ui, def, app, data_version, &vis_snapshot);
            }
        }
    }

    fn row_ui(&mut self, ui: &mut egui::Ui, d: &LayerDef, depth: usize, vis: &LayerVis, app: &Arc<App>, data_version: &Arc<AtomicU64>) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
        let selected = self.selected.as_deref() == Some(d.id);
        if selected {
            ui.painter().rect_filled(rect, 0.0, theme::ROW_SEL);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 0.0, theme::PANEL_2.gamma_multiply(0.6));
        }
        ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, theme::BORDER));

        let p = ui.painter();
        let visible = vis.is_visible(d.id);
        let dim = if visible { theme::TEXT } else { theme::DIM };

        // Eye column (painted, not a glyph — survives any font set).
        let eye_c = Pos2::new(rect.left() + COLUMN / 2.0, rect.center().y);
        let eye_col = if visible { theme::OK } else { theme::DIM };
        p.circle_stroke(eye_c, 5.0, Stroke::new(1.4, eye_col));
        if visible {
            p.circle_filled(eye_c, 2.4, eye_col);
        }

        // Colour bar (group = wide, child = thin) — vectorcraft's marker.
        let mut x = rect.left() + COLUMN + 2.0;
        let bar_w = if d.kind == "group" { 4.0 } else { 2.0 };
        let col = layer_color(app, vis, d);
        p.rect_filled(Rect::from_min_size(Pos2::new(x - 1.0, rect.top() + 3.0), Vec2::new(bar_w, ROW_H - 6.0)), 0.0, col);
        x += 8.0 + depth as f32 * INDENT;

        // Disclosure chevron for groups.
        let is_group = d.kind == "group";
        if is_group {
            let open = self.open.contains(d.id) || !self.search.trim().is_empty();
            let dr = Rect::from_min_size(Pos2::new(x, rect.center().y - 8.0), Vec2::new(14.0, 16.0));
            let c = dr.center();
            let tri: Vec<Pos2> = if open {
                vec![Pos2::new(c.x - 4.0, c.y - 2.0), Pos2::new(c.x + 4.0, c.y - 2.0), Pos2::new(c.x, c.y + 3.0)]
            } else {
                vec![Pos2::new(c.x - 2.0, c.y - 4.0), Pos2::new(c.x - 2.0, c.y + 4.0), Pos2::new(c.x + 3.0, c.y)]
            };
            p.add(egui::Shape::convex_polygon(tri, theme::DIM, Stroke::NONE));
            let dresp = ui.interact(dr, ui.id().with(("disc", d.id)), Sense::click());
            if dresp.clicked() {
                if open {
                    self.open.remove(d.id);
                } else {
                    self.open.insert(d.id.to_string());
                }
            }
        }
        x += 16.0;

        // Name + kind chip.
        p.text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, d.name, FontId::proportional(12.5), dim);
        let kind_col = match d.kind {
            "engine" => theme::WARN,
            "label" => theme::ACCENT,
            _ => theme::DIM,
        };
        if d.kind != "group" {
            let label = d.kind.to_uppercase();
            let name_w = p.layout_no_wrap(d.name.to_string(), FontId::proportional(12.5), dim).size().x;
            p.text(Pos2::new(x + name_w + 8.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(8.5), kind_col);
        }

        // Zoom badge on the right.
        let (zmin, zmax) = vis.zoom.get(d.id).copied().unwrap_or((d.min_zoom, d.max_zoom));
        let badge = if zmax >= 19 { format!("z{zmin}+") } else { format!("z{zmin}–{zmax}") };
        p.text(Pos2::new(rect.right() - 6.0, rect.center().y), Align2::RIGHT_CENTER, badge, FontId::monospace(9.5), theme::DIM);

        // Interactions: click on the eye column toggles, click on the row selects.
        if resp.clicked() {
            let on_eye = resp.interact_pointer_pos().is_some_and(|p| p.x - rect.left() < COLUMN);
            if on_eye {
                self.toggle(app, d.id, !visible, data_version);
            } else {
                self.selected = Some(d.id.to_string());
                let (mn, mx) = vis.zoom.get(d.id).copied().unwrap_or((d.min_zoom, d.max_zoom));
                self.edit_zoom = [mn as i32, mx as i32];
            }
        }
    }

    fn toggle(&mut self, app: &Arc<App>, id: &str, value: bool, data_version: &Arc<AtomicU64>) {
        {
            let mut vis = app.layers.write().unwrap();
            vis.visible.insert(id.to_string(), value);
            vis.touch();
        }
        app.invalidate();
        data_version.fetch_add(1, Ordering::Relaxed);
    }

    fn properties_ui(&mut self, ui: &mut egui::Ui, def: &LayerDef, app: &Arc<App>, data_version: &Arc<AtomicU64>, vis: &LayerVis) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("ENGINE LAYER PROPERTIES").size(10.0).strong().color(theme::DIM));
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Reset").clicked() {
                    let mut v = app.layers.write().unwrap();
                    v.visible.remove(def.id);
                    v.opacity.remove(def.id);
                    v.zoom.remove(def.id);
                    v.color.remove(def.id);
                    v.touch();
                    app.invalidate();
                    data_version.fetch_add(1, Ordering::Relaxed);
                }
            });
        });
        egui::ScrollArea::vertical().id_salt("layer-props").auto_shrink([false, false]).show(ui, |ui| {
            // Vector path (the data this layer paints).
            ui.label(egui::RichText::new("Vector path").size(10.5).color(theme::DIM));
            egui::Frame::new().fill(theme::BG).corner_radius(CornerRadius::same(5)).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(def.vector_path).size(11.0).monospace().color(theme::TEXT));
            });

            // Paint: fill colour + swatch.
            ui.add_space(4.0);
            ui.label(egui::RichText::new(match def.kind { "fill" => "Fill", "stroke" => "Stroke", "label" => "Label paint", "engine" => "Engine paint", _ => "Paint" }).size(10.5).color(theme::DIM));
            ui.horizontal(|ui| {
                let col = layer_color(app, vis, def);
                let (sw, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
                ui.painter().rect_filled(sw, CornerRadius::same(4), col);
                ui.painter().rect_stroke(sw, CornerRadius::same(4), Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Outside);
                let mut hex = format!("#{:02X}{:02X}{:02X}", col.r(), col.g(), col.b());
                let resp = ui.add(egui::TextEdit::singleline(&mut hex).desired_width(86.0).font(FontId::monospace(12.0)));
                if resp.changed() {
                    if let Some(c) = parse_hex(&hex) {
                        let mut v = app.layers.write().unwrap();
                        v.color.insert(def.id.to_string(), c);
                        v.touch();
                        app.invalidate();
                        data_version.fetch_add(1, Ordering::Relaxed);
                    }
                }
                if vis.color_override(def.id).is_some() && ui.small_button("palette").clicked() {
                    let mut v = app.layers.write().unwrap();
                    v.color.remove(def.id);
                    v.touch();
                    app.invalidate();
                    data_version.fetch_add(1, Ordering::Relaxed);
                }
            });

            // Opacity.
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Opacity").size(10.5).color(theme::DIM));
            let mut op = vis.alpha(def.id);
            let slider = ui.add(egui::Slider::new(&mut op, 0.0..=1.0).show_value(true));
            if slider.changed() {
                let mut v = app.layers.write().unwrap();
                v.opacity.insert(def.id.to_string(), op);
                v.touch();
                app.invalidate();
                data_version.fetch_add(1, Ordering::Relaxed);
            }

            // Zoom range.
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Zoom range").size(10.5).color(theme::DIM));
            ui.horizontal(|ui| {
                let mut edited = false;
                let a = ui.add(egui::DragValue::new(&mut self.edit_zoom[0]).range(0..=19).prefix("z").speed(0.2));
                ui.label("–");
                let b = ui.add(egui::DragValue::new(&mut self.edit_zoom[1]).range(0..=19).prefix("z").speed(0.2));
                if a.changed() || b.changed() {
                    if self.edit_zoom[1] < self.edit_zoom[0] {
                        self.edit_zoom[1] = self.edit_zoom[0];
                    }
                    edited = true;
                }
                if edited {
                    let mut v = app.layers.write().unwrap();
                    v.zoom.insert(def.id.to_string(), (self.edit_zoom[0].clamp(0, 19) as u8, self.edit_zoom[1].clamp(0, 19) as u8));
                    v.touch();
                    app.invalidate();
                    data_version.fetch_add(1, Ordering::Relaxed);
                }
                if !vis.zoom.contains_key(def.id) {
                    ui.label(egui::RichText::new(format!("(default {}–{})", def.min_zoom, def.max_zoom)).size(9.5).color(theme::DIM));
                }
            });

            // Engine flags (roads only).
            if def.parent == "roads" || def.id == "roads" {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Engine").size(10.5).color(theme::DIM));
                ui.label(egui::RichText::new("Casing under fill · oneway arrows from z16 · name shields from z13 · lane widths from maxspeed/lanes tags").size(10.0).color(theme::DIM));
            }
        });
    }
}

fn layer_color(app: &Arc<App>, vis: &LayerVis, d: &LayerDef) -> Color32 {
    if let Some([r, g, b]) = vis.color_override(d.id) {
        return Color32::from_rgb(r, g, b);
    }
    let base = d.color_var.and_then(|v| app.palette.get(v)).unwrap_or(carto_style::Rgba::rgb(d.fallback[0], d.fallback[1], d.fallback[2]));
    Color32::from_rgb(base.r, base.g, base.b)
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    Some([u8::from_str_radix(&s[0..2], 16).ok()?, u8::from_str_radix(&s[2..4], 16).ok()?, u8::from_str_radix(&s[4..6], 16).ok()?])
}

/* --------------------------------------------------------------------- */
/* Downloads tab (craft-launcher region cards)                             */
/* --------------------------------------------------------------------- */

struct DownloadsState {
    manager: Arc<Manager>,
    jobs: HashMap<String, Phase>,
    last_jobs: HashMap<String, Phase>,
    custom_url: String,
    custom_bbox_world: [f64; 4],
    data_version: Arc<AtomicU64>,
}

impl DownloadsState {
    fn new() -> Self {
        Self { manager: Arc::new(Manager::new()), jobs: HashMap::new(), last_jobs: HashMap::new(), custom_url: String::new(), custom_bbox_world: [-180.0, -85.0, 180.0, 85.0], data_version: Arc::new(AtomicU64::new(1)) }
    }

    fn regions_dir(&self, _app: &Arc<App>) -> PathBuf {
        tile_server_core::abs_root().join("data").join("regions")
    }

    fn downloads_dir(&self, _app: &Arc<App>) -> PathBuf {
        tile_server_core::abs_root().join("data").join("downloads")
    }

    /// Poll the manager, swap in a freshly extracted store, keep the UI alive.
    fn poll(&mut self, ctx: &egui::Context, app: &Arc<App>, data_version: &Arc<AtomicU64>) {
        self.jobs = self.manager.all();
        let mut any_active = false;
        for (id, phase) in &self.jobs {
            if phase.is_active() {
                any_active = true;
            }
            let was = self.last_jobs.get(id);
            if matches!(phase, Phase::Installed { .. }) && !was.is_some_and(|p| matches!(p, Phase::Installed { .. })) {
                // Freshly installed: swap the store in and tell the user.
                let dir = self.regions_dir(app).join(id);
                match carto_core::Store::load(&dir) {
                    Ok(store) => {
                        let label = region_label(id);
                        *app.store.write().unwrap() = store;
                        *app.region_name.lock().unwrap() = label.clone();
                        app.invalidate();
                        data_version.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => eprintln!("store load failed for {id}: {e}"),
                }
            }
        }
        self.last_jobs = self.jobs.clone();
        if any_active {
            ctx.request_repaint_after(Duration::from_millis(400));
        }
    }

    fn start(&mut self, app: &Arc<App>, id: &str, name: &str, bbox: [f64; 4], url: &str) {
        self.manager.start(
            RegionReq { id: id.to_string(), name: name.to_string(), bbox, url: url.to_string() },
            self.downloads_dir(app),
            self.regions_dir(app),
        );
    }

    fn tab_ui(&mut self, ui: &mut egui::Ui, app: &Arc<App>) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Downloads").size(17.0).strong());
            ui.separator();
            ui.label(egui::RichText::new("Tanzania map parts — download, extract, serve. Same flow the launcher uses for its apps.").size(12.0).color(theme::DIM));
        });
        ui.add_space(4.0);

        // Custom PBF URL row.
        egui::Frame::new().fill(theme::PANEL).corner_radius(CornerRadius::same(8)).stroke(Stroke::new(1.0, theme::BORDER)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Custom URL").size(11.0).color(theme::DIM));
                let w = ui.available_width() - 250.0;
                ui.add(egui::TextEdit::singleline(&mut self.custom_url).hint_text("https://…/any-extract.osm.pbf").desired_width(w.max(120.0)).font(FontId::monospace(11.5)));
                let enabled = self.custom_url.trim().starts_with("http");
                if ui.add_enabled(enabled, egui::Button::new("Download & extract")).clicked() {
                    let url = self.custom_url.trim().to_string();
                    let stem = pbf_name_for(&url).trim_end_matches(".osm.pbf").trim_end_matches(".pbf").to_string();
                    let id = format!("custom-{}", sanitize(&stem));
                    self.start(app, &id, &stem, self.custom_bbox_world, &url);
                }
            });
        });
        ui.add_space(8.0);

        // Region cards (single column, full width — no cell sizing games).
        let regions: Vec<Region> = tile_server_core::download::regions();
        egui::ScrollArea::vertical().id_salt("region-cards").auto_shrink([false, false]).show(ui, |ui| {
            let card_w = ui.available_width();
            for r in &regions {
                self.card_ui(ui, app, r, card_w);
                ui.add_space(10.0);
            }
        });

        ui.add_space(6.0);
        ui.label(egui::RichText::new("Data © OpenStreetMap contributors (ODbL) · PBF extracts served by download.geofabrik.de · stores built with the same extractor as tile-server-cli extract").size(10.0).color(theme::DIM));
    }

    fn card_ui(&mut self, ui: &mut egui::Ui, app: &Arc<App>, r: &Region, w: f32) {
        let phase = self.jobs.get(r.id).cloned().unwrap_or(Phase::Idle);
        let active = phase.is_active();
        let installed = store_installed(&self.regions_dir(app).join(r.id)) || matches!(phase, Phase::Installed { .. });
        let active_region = app.region_name.lock().unwrap().clone();

        egui::Frame::new().fill(theme::PANEL).corner_radius(CornerRadius::same(8)).stroke(Stroke::new(1.0, if active { theme::ACCENT } else { theme::BORDER })).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
            ui.set_min_width(w - 24.0);
            ui.set_max_width(w - 24.0);

            // Title + status pill.
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(r.name).size(14.5).strong());
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    match &phase {
                        Phase::Idle if installed => pill_local(ui, if active_region == r.name { "In use" } else { "Installed" }, theme::OK),
                        Phase::Idle => pill_local(ui, "Not installed", theme::DIM),
                        Phase::Queued => pill_local(ui, "Queued", theme::WARN),
                        Phase::Downloading { .. } | Phase::Reconnecting { .. } => pill_local(ui, "Downloading", theme::WARN),
                        Phase::Extracting { .. } => pill_local(ui, "Extracting", theme::WARN),
                        Phase::Installed { .. } => pill_local(ui, "Installed", theme::OK),
                        Phase::Failed { .. } => pill_local(ui, "Failed", theme::DANGER),
                    }
                });
            });
            ui.label(egui::RichText::new(r.desc).size(11.5).color(theme::DIM).weak());

            // Meta: bbox + size.
            ui.label(
                egui::RichText::new(format!(
                    "bbox {:.1}, {:.1} → {:.1}, {:.1}   ·   {}   ·   {}",
                    r.bbox[0], r.bbox[1], r.bbox[2], r.bbox[3], r.size_hint,
                    if r.shared { "crops the shared Tanzania PBF" } else { "direct PBF download" }
                ))
                .size(10.5)
                .monospace()
                .color(theme::DIM),
            );

            // Source URL row (the launcher's release link).
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("source").size(10.0).color(theme::DIM));
                ui.hyperlink_to(egui::RichText::new(shorten_url(r.url, 58)).size(10.5).monospace(), r.url);
                if ui.small_button("Copy URL").clicked() {
                    ui.ctx().copy_text(r.url.to_string());
                }
            });

            // Progress.
            match &phase {
                Phase::Downloading { done, total } | Phase::Reconnecting { done, total } => {
                    let frac = if *total > 0 { *done as f32 / *total as f32 } else { 0.15 };
                    ui.add(egui::ProgressBar::new(frac.clamp(0.02, 1.0)).show_percentage());
                    ui.label(
                        egui::RichText::new(if matches!(phase, Phase::Reconnecting { .. }) {
                            format!("Reconnecting — {} of {} done", fmt_bytes(*done), if *total > 0 { fmt_bytes(*total) } else { "unknown size".into() })
                        } else {
                            format!("{} of {}", fmt_bytes(*done), if *total > 0 { fmt_bytes(*total) } else { "unknown size".into() })
                        })
                        .size(10.5)
                        .color(theme::DIM),
                    );
                }
                Phase::Extracting { message } => {
                    ui.add(egui::ProgressBar::new(0.75).animate(true));
                    ui.label(egui::RichText::new(message).size(10.5).color(theme::DIM));
                }
                Phase::Installed { roads, buildings, pois } => {
                    ui.label(egui::RichText::new(format!("{} roads · {} buildings · {} POIs", thousands(*roads), thousands(*buildings), thousands(*pois))).size(11.0).color(theme::OK));
                }
                Phase::Failed { message } => {
                    ui.label(egui::RichText::new(message).size(10.5).color(theme::DANGER));
                }
                Phase::Idle | Phase::Queued => {
                    if installed {
                        if let Ok(meta) = std::fs::metadata(self.regions_dir(app).join(r.id).join("store.bin")) {
                            ui.label(egui::RichText::new(format!("store.bin {} on disk", fmt_bytes(meta.len()))).size(10.5).color(theme::DIM));
                        }
                    }
                }
            }

            // Buttons.
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if active {
                    if ui.add(egui::Button::new(egui::RichText::new("Cancel").color(theme::DANGER))).clicked() {
                        self.manager.cancel(r.id);
                    }
                } else if ui.add(egui::Button::new(if installed { "Redownload" } else { "Download" }).fill(if installed { theme::PANEL_2 } else { theme::ACCENT_DIM })).clicked() {
                    self.start(app, r.id, r.name, r.bbox, r.url);
                }
                if installed && active_region != r.name && ui.add(egui::Button::new("Use this region")).clicked() {
                    let dir = self.regions_dir(app).join(r.id);
                    match carto_core::Store::load(&dir) {
                        Ok(store) => {
                            *app.store.write().unwrap() = store;
                            *app.region_name.lock().unwrap() = r.name.to_string();
                            app.invalidate();
                            self.data_version.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            // Leave the current store; surface the error inline next poll.
                            self.jobs.insert(r.id.to_string(), Phase::Failed { message: format!("store load failed: {e}") });
                        }
                    }
                }
                if installed && ui.small_button("Reveal").clicked() {
                    let dir = self.regions_dir(app).join(r.id);
                    ui.ctx().copy_text(dir.display().to_string());
                }
            });
        });
    }
}

fn pill_local(ui: &mut egui::Ui, text: &str, color: Color32) {
    theme::pill(ui, text, color);
}

fn region_label(id: &str) -> String {
    let regions = tile_server_core::download::regions();
    regions.iter().find(|r| r.id == id).map(|r| r.name.to_string()).unwrap_or_else(|| id.to_string())
}

fn store_installed(dir: &std::path::Path) -> bool {
    dir.join("store.bin").is_file()
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect::<String>().trim_matches('-').to_string()
}

fn shorten_url(u: &str, max: usize) -> String {
    if u.len() > max {
        format!("{}…", &u[..max.saturating_sub(1)])
    } else {
        u.to_string()
    }
}

/* --------------------------------------------------------------------- */
/* Screenshot capture (xvfb / --screenshot <dir>)                          */
/* --------------------------------------------------------------------- */

/// The screenshot eframe took of the previous painted frame, if any.
fn take_screenshot_event(ctx: &egui::Context) -> Option<egui::ColorImage> {
    ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Screenshot { image, .. } => Some((**image).clone()),
            _ => None,
        })
    })
}

/// Save the current tab: prefer a real X11 root-window grab (ffmpeg x11grab) —
/// under xvfb eframe's own screenshot can come back as a cleared buffer —
/// falling back to the in-process frame it captured.
fn save_tab_screenshot(
    dir: &std::path::Path,
    name: &str,
    fallback: Option<&egui::ColorImage>,
) {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(name);
    if x11_grab_png(&path).is_some() {
        return;
    }
    eprintln!("x11 grab unavailable, saving eframe frame buffer");
    if let Some(img) = fallback {
        save_color_image(img, &path);
    }
}

/// Grab the X11 root window (the whole virtual screen) to a PNG via ffmpeg.
fn x11_grab_png(path: &std::path::Path) -> Option<()> {
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let size = std::env::var("MAP_LAUNCHER_SHOT_SIZE").unwrap_or_else(|_| "1600x1000".into());
    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "x11grab", "-video_size"])
        .args([size.as_str(), "-i", display.as_str(), "-frames:v", "1"])
        .arg(path.as_os_str())
        .status()
        .ok()?;
    status.success().then_some(())
}

/// Save a straight-alpha ColorImage as a PNG.
fn save_color_image(img: &egui::ColorImage, path: &std::path::Path) {
    let (w, h) = (img.size[0] as usize, img.size[1] as usize);
    let mut premul = Vec::with_capacity(w * h * 4);
    for px in &img.pixels {
        let (r, g, b, a) = (px.r() as u32, px.g() as u32, px.b() as u32, px.a() as u32);
        premul.push(((r * a + 127) / 255) as u8);
        premul.push(((g * a + 127) / 255) as u8);
        premul.push(((b * a + 127) / 255) as u8);
        premul.push(a as u8);
    }
    let Some(size) = tiny_skia::IntSize::from_wh(w as u32, h as u32) else { return };
    let Some(pm) = tiny_skia::Pixmap::from_vec(premul, size) else { return };
    let _ = std::fs::create_dir_all(path.parent().unwrap_or_else(|| std::path::Path::new(".")));
    let _ = pm.save_png(path);
}
