//! map-launcher tile-server — the desktop app twin of the headless server.
//!
//! One engine (tile-server-core), two binaries:
//! - `tile-server`      → this window, styled after craft-launcher (frameless
//!   window, custom titlebar, hard edges, hairlines, mono HUD labels):
//!   a Map tab (tile canvas + the layer engine panel, in the spirit of
//!   vectorcraft's Layers panel) and a Downloads tab (Tanzania region table +
//!   a Local Data table + a data-folder browser, in the spirit of
//!   craft-launcher), while the HTTP server keeps serving :3100.
//! - `tile-server-cli`  → the same engine without a window (`serve`/`extract`).
//!
//! Subcommands keep release scripts working: `tile-server serve`, `tile-server
//! extract <pbf> <outdir> [w s e n]`, `--version`; `--screenshot <dir>` renders
//! both tabs headlessly (xvfb) for the versioned screenshots folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use carto_math::{lonlat_to_merc, merc_to_lonlat, world_px};
use eframe::egui;
use egui::epaint::text::VariationCoords;
use egui::text::LayoutJob;
use egui::viewport::ResizeDirection;
use egui::{Align, Align2, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Frame, Layout, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use tile_server_core::download::{pbf_name_for, Manager, Phase, Region, RegionReq};
use tile_server_core::layers::{children_of, descendants_of, layer_catalog, top_ancestor_of, LayerDef, LayerVis};
use tile_server_core::{bind_error_hint, load_stack, render_tile_pixmap, spawn_server, App, ServerHandle};

mod settings;
use settings::Settings;

mod annotations;
use annotations::Anno;

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

    // Remembered choices: port + data folder (per-user config dir).
    let settings = Settings::load();
    if data_flag.is_none() {
        port = settings.port.unwrap_or(port);
    }
    // A remembered data folder only kicks in without --data-dir / env override.
    if data_flag.is_none() && std::env::var("MAP_LAUNCHER_DATA").map(|v| v.trim().is_empty()).unwrap_or(true) {
        if let Some(d) = settings.data_dir.as_ref().filter(|d| d.join("store.bin").is_file()) {
            data_flag = Some(d.display().to_string());
        }
    }

    let stack = load_stack(data_flag);
    stack.log();
    let app = tile_server_core::app_from_stack(stack);

    // Screenshot mode / driver scripts: size the window to fill the Xvfb
    // screen at 0,0 so a root-window grab captures exactly the app
    // (MAP_LAUNCHER_SHOT_SIZE=WxH; also honoured without --screenshot so
    // automation tools see a deterministic layout).
    let shot_size = {
        let want = screenshot.is_some() || std::env::var("MAP_LAUNCHER_SHOT_SIZE").is_ok();
        want.then(|| {
            let s = std::env::var("MAP_LAUNCHER_SHOT_SIZE").unwrap_or_else(|_| "1600x1000".into());
            let mut it = s.split('x');
            (
                it.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1600.0),
                it.next().and_then(|v| v.parse::<f32>().ok()).unwrap_or(1000.0),
            )
        })
    };
    // The header is the title bar (craft-launcher): a frameless window.
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!("map-launcher tile-server {}", tile_server_core::VERSION))
        .with_decorations(false)
        .with_min_inner_size([720.0, 500.0]);
    viewport = match shot_size {
        Some((w, h)) => viewport.with_inner_size([w, h]).with_position([0.0, 0.0]),
        None => viewport.with_inner_size([1120.0, 760.0]),
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
            Ok(Box::new(Shell::new(cc, app.clone(), port, settings, screenshot)))
        }),
    )
}

/* --------------------------------------------------------------------- */
/* Theme — craft-launcher tokens, verbatim                                  */
/* --------------------------------------------------------------------- */

const HEADER_H: f32 = 48.0;
const FOOTER_H: f32 = 34.0;
/// Height of buttons, inputs and filter cells (the site's medium button).
pub const CONTROL_H: f32 = 34.0;
/// `.hud-label`: 11 px Geist Mono 500, upper case, 0.15 em apart.
pub const HUD_SIZE: f32 = 11.0;
pub const HUD_SPACING: f32 = HUD_SIZE * 0.15;
/// Buttons: 11 px Geist Mono 700, upper case, 0.12 em apart.
pub const BUTTON_SPACING: f32 = HUD_SIZE * 0.12;
/// How far in from the window's edges a custom title bar resizes.
const RESIZE_EDGE: f32 = 5.0;

mod theme {
    use super::*;

    #[derive(Clone, Copy)]
    pub struct Tokens {
        pub bg: Color32,
        pub sunken: Color32,
        pub ink: Color32,
        pub ink_strong: Color32,
        pub muted: Color32,
        pub faint: Color32,
        pub line: Color32,
        pub line_strong: Color32,
        pub accent: Color32,
        pub accent_ink: Color32,
        pub invert_bg: Color32,
        pub invert_fg: Color32,
        pub good: Color32,
        pub warn: Color32,
        pub bad: Color32,
    }

    const fn ink_alpha(rgb: u32, alpha: u8) -> Color32 {
        let [_, r, g, b] = rgb.to_be_bytes();
        Color32::from_rgba_unmultiplied_const(r, g, b, alpha)
    }

    const fn hex(rgb: u32) -> Color32 {
        let [_, r, g, b] = rgb.to_be_bytes();
        Color32::from_rgb(r, g, b)
    }

    impl Tokens {
        pub const DARK: Self = Self {
            bg: hex(0x121316),
            sunken: hex(0x060607),
            ink: hex(0xf2f1ee),
            ink_strong: hex(0xffffff),
            muted: ink_alpha(0xf2f1ee, 153),
            faint: ink_alpha(0xf2f1ee, 97),
            line: ink_alpha(0xf2f1ee, 38),
            line_strong: ink_alpha(0xf2f1ee, 102),
            accent: hex(0x2d81ff),
            accent_ink: hex(0x74aaff),
            invert_bg: hex(0xffffff),
            invert_fg: hex(0x0b0b0c),
            good: hex(0x3fd6b8),
            warn: hex(0xffc14d),
            bad: hex(0xff6259),
        };
    }

    pub fn t(_ctx: &egui::Context) -> Tokens {
        Tokens::DARK
    }

    pub fn hairline() -> Stroke {
        Stroke::new(1.0, Tokens::DARK.line)
    }

    /// Inter 500.
    pub fn medium() -> FontFamily {
        FontFamily::Name("medium".into())
    }
    /// Archivo 620 at 118 % width: headings and wordmarks.
    pub fn display() -> FontFamily {
        FontFamily::Name("display".into())
    }
    /// Geist Mono 500: labels and badges.
    pub fn mono() -> FontFamily {
        FontFamily::Name("mono".into())
    }
    /// Geist Mono 700: buttons.
    pub fn mono_bold() -> FontFamily {
        FontFamily::Name("mono-bold".into())
    }

    /// Registers the three variable fonts (OFL, vendored from craft-launcher),
    /// each at the weights the UI uses. egui's own fonts stay behind ours for
    /// symbols ours don't have.
    pub fn install(cc: &eframe::CreationContext<'_>) {
        const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Variable.ttf");
        const ARCHIVO: &[u8] = include_bytes!("../assets/fonts/Archivo-Variable.ttf");
        const GEIST_MONO: &[u8] = include_bytes!("../assets/fonts/GeistMono-Variable.ttf");
        let mut fonts = FontDefinitions::default();
        let fallback: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        let faces: [(&str, &[u8], &[([u8; 4], f32)], FontFamily); 7] = [
            ("Inter", INTER, &[(*b"wght", 400.0), (*b"opsz", 14.0)], FontFamily::Proportional),
            ("Inter-Medium", INTER, &[(*b"wght", 500.0), (*b"opsz", 14.0)], medium()),
            ("Inter-SemiBold", INTER, &[(*b"wght", 600.0), (*b"opsz", 14.0)], FontFamily::Name("semibold".into())),
            ("Archivo-Display", ARCHIVO, &[(*b"wght", 620.0), (*b"wdth", 118.0)], display()),
            ("GeistMono", GEIST_MONO, &[(*b"wght", 400.0)], FontFamily::Monospace),
            ("GeistMono-Medium", GEIST_MONO, &[(*b"wght", 500.0)], mono()),
            ("GeistMono-Bold", GEIST_MONO, &[(*b"wght", 700.0)], mono_bold()),
        ];
        for (name, bytes, coords, family) in faces {
            // No subpixel binning: the fonts carry no hinting, so fractional x smears stems.
            let tweak = FontTweak { coords: VariationCoords::new(coords.iter().copied()), subpixel_binning: Some(false), ..Default::default() };
            fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes).tweak(tweak)));
            let mut stack = vec![name.to_owned()];
            stack.extend(fallback.iter().cloned());
            fonts.families.insert(family, stack);
        }
        cc.egui_ctx.set_fonts(fonts);

        cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
        let tk = Tokens::DARK;
        let mut v = egui::Visuals::dark();
        v.panel_fill = tk.bg;
        v.window_fill = tk.bg;
        v.extreme_bg_color = tk.sunken;
        v.faint_bg_color = tk.sunken;
        v.code_bg_color = tk.sunken;
        v.window_stroke = Stroke::new(1.0, tk.line_strong);
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.override_text_color = Some(tk.ink);
        v.hyperlink_color = tk.accent_ink;
        v.selection.bg_fill = tk.accent;
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        v.widgets.noninteractive.bg_fill = tk.sunken;
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, tk.ink);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, tk.line);
        v.widgets.inactive.bg_fill = tk.bg;
        v.widgets.inactive.weak_bg_fill = tk.bg;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, tk.line_strong);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, tk.line);
        v.widgets.hovered.bg_fill = tk.sunken;
        v.widgets.hovered.weak_bg_fill = tk.sunken;
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, tk.ink_strong);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, tk.line_strong);
        v.widgets.active.bg_fill = tk.sunken;
        v.widgets.active.weak_bg_fill = tk.sunken;
        v.widgets.active.fg_stroke = Stroke::new(1.0, tk.ink_strong);
        v.widgets.active.bg_stroke = Stroke::new(1.0, tk.line_strong);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = CornerRadius::ZERO;
            w.expansion = 0.0;
        }
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, v);
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = Vec2::new(8.0, 6.0);
            style.spacing.icon_spacing = 10.0;
            style.spacing.button_padding = Vec2::new(12.0, 6.0);
            style.spacing.interact_size.y = CONTROL_H;
            style.spacing.menu_margin = egui::Margin::same(4);
            style.spacing.window_margin = egui::Margin::same(4);
            style.text_styles = [
                (egui::TextStyle::Heading, FontId::new(24.0, display())),
                (egui::TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
                (egui::TextStyle::Button, FontId::new(13.5, medium())),
                (egui::TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
                (egui::TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
            ]
            .into();
        });
    }
}

/* --------------------------------------------------------------------- */
/* Widgets — the craft-launcher vocabulary                                  */
/* --------------------------------------------------------------------- */

mod widgets {
    use super::*;

    /// `text` laid out upper case in `family`, `spacing` points between letters.
    pub fn caps(ui: &egui::Ui, text: &str, size: f32, family: FontFamily, spacing: f32, color: Color32) -> Arc<egui::Galley> {
        let format = egui::TextFormat { font_id: FontId::new(size, family), color, extra_letter_spacing: spacing, ..Default::default() };
        ui.painter().layout_job(LayoutJob::single_section(text.to_uppercase(), format))
    }

    fn paint_left(ui: &egui::Ui, rect: Rect, galley: Arc<egui::Galley>, color: Color32) {
        let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
        ui.painter().galley_with_override_text_color(pos, galley, color);
    }

    fn paint_centred(ui: &egui::Ui, rect: Rect, galley: Arc<egui::Galley>, color: Color32) {
        ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
    }

    /// A `.hud-label` mono label.
    pub fn hud(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
        let galley = caps(ui, text, HUD_SIZE, theme::mono(), HUD_SPACING, color);
        let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::hover());
        paint_left(ui, rect, galley, color);
        resp
    }

    /// A mono label cut to one line with "…" where it is wider than `max_width`.
    pub fn hud_elided(ui: &mut egui::Ui, text: &str, color: Color32, max_width: f32) -> egui::Response {
        let format = egui::TextFormat {
            font_id: FontId::new(HUD_SIZE, theme::mono()),
            color,
            extra_letter_spacing: HUD_SPACING,
            ..Default::default()
        };
        let upper = text.to_uppercase();
        let mut galley = ui.painter().layout_job(LayoutJob::single_section(upper.clone(), format.clone()));
        if galley.size().x > max_width && upper.len() > 4 {
            let mut cut = upper.clone();
            while cut.len() > 4 {
                cut.pop();
                galley = ui.painter().layout_job(LayoutJob::single_section(format!("{cut}…"), format.clone()));
                if galley.size().x <= max_width {
                    break;
                }
            }
        }
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x.min(max_width), galley.size().y), Sense::hover());
        let resp = resp.on_hover_text(text);
        paint_left(ui, rect, galley, color);
        resp
    }

    /// A mono link that opens in the browser (`text ↗`).
    pub fn hud_link(ui: &mut egui::Ui, text: &str, url: &str) -> egui::Response {
        let t = theme::t(ui.ctx());
        let label = format!("{text} ↗");
        let galley = caps(ui, &label, HUD_SIZE, theme::mono(), HUD_SPACING, t.muted);
        let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::click());
        let color = if resp.hovered() { t.ink_strong } else { t.muted };
        paint_left(ui, rect, galley, color);
        let resp = resp.on_hover_text(url).on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        resp
    }

    /// A small square status badge: 10 px mono on the sunken surface, in `color`.
    pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
        let t = theme::t(ui.ctx());
        let galley = caps(ui, text, 10.0, theme::mono(), 1.0, color);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 12.0, 20.0), Sense::hover());
        ui.painter().rect(rect, CornerRadius::ZERO, t.sunken, theme::hairline(), egui::StrokeKind::Inside);
        paint_centred(ui, rect, galley, color);
        resp
    }

    /// Which of the button variants.
    #[derive(Clone, Copy)]
    pub enum Kind {
        /// A solid block in the inverted colours; fades a little on hover.
        Primary,
        /// Outlined; inverts on hover.
        Secondary,
        /// Solid red; fades a little on hover.
        #[allow(dead_code)] // part of the vocabulary, kept for craft parity
        Danger,
    }

    /// A square button with a mono upper-case label, at least `width` wide.
    pub fn button(ui: &mut egui::Ui, text: &str, kind: Kind, width: f32) -> egui::Response {
        let t = theme::t(ui.ctx());
        let galley = caps(ui, text, HUD_SIZE, theme::mono_bold(), BUTTON_SPACING, t.ink);
        let size = Vec2::new((galley.size().x + 32.0).max(width), CONTROL_H);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let enabled = ui.is_enabled();
        let hovered = resp.hovered() && enabled;
        let (fill, stroke, color) = match kind {
            Kind::Primary => (t.invert_bg.gamma_multiply(if hovered { 0.8 } else { 1.0 }), Stroke::NONE, t.invert_fg),
            Kind::Danger => (t.bad.gamma_multiply(if hovered { 0.8 } else { 1.0 }), Stroke::NONE, Color32::WHITE),
            Kind::Secondary if hovered => (t.invert_bg, Stroke::NONE, t.invert_fg),
            Kind::Secondary => (t.bg, Stroke::new(1.0, t.line_strong), t.ink),
        };
        let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.5) };
        ui.painter().rect(rect, CornerRadius::ZERO, fade(fill), Stroke::new(stroke.width, fade(stroke.color)), egui::StrokeKind::Inside);
        paint_centred(ui, rect, galley, fade(color));
        if enabled {
            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            resp
        }
    }

    /// Which drawn glyph a [`glyph_button`] shows (drawn, so no font needs the symbol).
    #[derive(Clone, Copy, PartialEq)]
    pub enum Glyph {
        Close,
        Copy,
        Folder,
        Plus,
        Minus,
        Home,
    }

    impl Glyph {
        fn tip(&self) -> &'static str {
            match self {
                Glyph::Close => "Close",
                Glyph::Copy => "Copy",
                Glyph::Folder => "Open folder",
                Glyph::Plus => "Zoom in",
                Glyph::Minus => "Zoom out",
                Glyph::Home => "Reset view",
            }
        }
    }

    /// A 34 px square button with a painter-drawn glyph; hairline outline,
    /// inverts on hover.
    pub fn glyph_button(ui: &mut egui::Ui, glyph: Glyph) -> egui::Response {
        let t = theme::t(ui.ctx());
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(CONTROL_H), Sense::click());
        let (fill, stroke_color) = if resp.hovered() {
            (t.invert_bg, t.invert_fg)
        } else {
            (t.bg, t.line_strong)
        };
        ui.painter().rect(rect, CornerRadius::ZERO, fill, Stroke::new(1.0, stroke_color), egui::StrokeKind::Inside);
        let p = ui.painter();
        let c = rect.center();
        let ink = if resp.hovered() { t.invert_fg } else { t.muted };
        let s = Stroke::new(1.5, ink);
        match glyph {
            Glyph::Close => {
                p.line_segment([c - Vec2::new(4.5, 4.5), c + Vec2::new(4.5, 4.5)], s);
                p.line_segment([c - Vec2::new(4.5, -4.5), c + Vec2::new(4.5, -4.5)], s);
            }
            Glyph::Copy => {
                p.rect_stroke(Rect::from_min_size(c - Vec2::new(5.0, 5.0), Vec2::splat(7.0)), CornerRadius::ZERO, s, egui::StrokeKind::Inside);
                p.hline(c.x - 1.0..=c.x + 5.0, c.y - 5.0, s);
                p.vline(c.x + 5.0, c.y - 5.0..=c.y + 1.0, s);
                p.hline(c.x - 5.0..=c.x + 1.0, c.y + 5.0, s);
                p.vline(c.x - 5.0, c.y - 1.0..=c.y + 5.0, s);
            }
            Glyph::Folder => {
                p.hline(c.x - 6.0..=c.x - 1.0, c.y - 4.0, s);
                p.line_segment([Pos2::new(c.x - 6.0, c.y - 4.0), Pos2::new(c.x - 6.0, c.y + 4.0)], s);
                p.line_segment([Pos2::new(c.x - 6.0, c.y + 4.0), Pos2::new(c.x + 6.0, c.y + 4.0)], s);
                p.line_segment([Pos2::new(c.x + 6.0, c.y + 4.0), Pos2::new(c.x + 6.0, c.y - 1.0)], s);
                p.hline(c.x - 1.0..=c.x + 6.0, c.y - 1.0, s);
                p.line_segment([Pos2::new(c.x - 1.0, c.y - 1.0), Pos2::new(c.x + 1.0, c.y - 4.0)], s);
            }
            Glyph::Plus => {
                p.hline(c.x - 5.0..=c.x + 5.0, c.y, s);
                p.vline(c.x, c.y - 5.0..=c.y + 5.0, s);
            }
            Glyph::Minus => {
                p.hline(c.x - 5.0..=c.x + 5.0, c.y, s);
            }
            Glyph::Home => {
                p.line_segment([Pos2::new(c.x - 6.0, c.y), Pos2::new(c.x, c.y - 6.0)], s);
                p.line_segment([Pos2::new(c.x, c.y - 6.0), Pos2::new(c.x + 6.0, c.y)], s);
                p.rect_stroke(Rect::from_min_size(Pos2::new(c.x - 4.0, c.y), Vec2::new(8.0, 6.0)), CornerRadius::ZERO, s, egui::StrokeKind::Inside);
            }
        }
        resp.on_hover_text(glyph.tip()).on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    /// A slim square progress bar with a caption above it.
    pub fn progress(ui: &mut egui::Ui, fraction: Option<f32>, caption: &str, color: Color32, width: f32) {
        let t = theme::t(ui.ctx());
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, CONTROL_H), Sense::hover());
        let p = ui.painter();
        let bar = Rect::from_min_size(egui::pos2(rect.left(), rect.bottom() - 4.0), Vec2::new(width, 4.0));
        p.rect_filled(bar, CornerRadius::ZERO, t.line);
        match fraction {
            Some(f) => {
                p.rect_filled(Rect::from_min_size(bar.min, Vec2::new(bar.width() * f.clamp(0.0, 1.0), bar.height())), CornerRadius::ZERO, color);
            }
            None => {
                let width = bar.width();
                let segment = width * 0.3;
                let x = (ui.input(|i| i.time) * 0.8).fract() as f32 * width;
                for left in [x, x - width] {
                    let seg = Rect::from_min_size(egui::pos2(bar.left() + left, bar.top()), Vec2::new(segment, bar.height())).intersect(bar);
                    if seg.is_positive() {
                        p.rect_filled(seg, CornerRadius::ZERO, color);
                    }
                }
                ui.ctx().request_repaint();
            }
        }
        p.text(egui::pos2(rect.left(), rect.top() + 9.0), Align2::LEFT_CENTER, caption, FontId::new(12.0, theme::medium()), t.muted);
    }

    /// A 1 px vertical rule between header clusters.
    pub fn rule(ui: &mut egui::Ui, height: f32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, height), Sense::hover());
        ui.painter().vline(rect.center().x, rect.y_range(), theme::hairline());
    }

    /// Custom title bars: dragging the header's empty space moves the window,
    /// double-clicking maximizes/restores. Call before drawing the header's
    /// controls so they stay on top.
    pub fn title_bar_drag(ui: &mut egui::Ui) {
        let r = ui.max_rect();
        let rect = Rect::from_min_max(r.min + Vec2::splat(RESIZE_EDGE), r.max - Vec2::new(RESIZE_EDGE, 0.0));
        let resp = ui.interact(rect, ui.id().with("title-bar"), Sense::click_and_drag());
        if resp.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if resp.double_clicked() {
            let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
    }

    /// A window button in the custom title bar.
    #[derive(Clone, Copy, PartialEq)]
    enum WindowButton {
        Minimize,
        Maximize,
        Restore,
        Close,
    }

    /// Minimize, maximize (or restore) and close, `height` tall, for a
    /// right-to-left layout: close ends up at the right.
    pub fn window_buttons(ui: &mut egui::Ui, height: f32) {
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if window_button(ui, WindowButton::Close, height).clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let size = if maximized { WindowButton::Restore } else { WindowButton::Maximize };
        if window_button(ui, size, height).clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
        if window_button(ui, WindowButton::Minimize, height).clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        rule(ui, height);
    }

    fn window_button(ui: &mut egui::Ui, button: WindowButton, height: f32) -> egui::Response {
        let t = theme::t(ui.ctx());
        let (rect, _) = ui.allocate_exact_size(Vec2::new(46.0, height), Sense::hover());
        // Clickable below the resize band along the top (and, for close, the right edge).
        let right = if button == WindowButton::Close { RESIZE_EDGE } else { 0.0 };
        let hit = Rect::from_min_max(rect.min + Vec2::new(0.0, RESIZE_EDGE), rect.max - Vec2::new(right, 0.0));
        let resp = ui.interact(hit, ui.id().with(("window-button", button as u8)), Sense::click());
        let (fill, ink) = match (button, resp.hovered()) {
            (WindowButton::Close, true) => (Color32::from_rgb(0xc4, 0x2b, 0x1c), Color32::WHITE),
            (_, true) => (t.line, t.ink_strong),
            (_, false) => (Color32::TRANSPARENT, t.muted),
        };
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::ZERO, fill);
        let c = rect.center();
        let stroke = Stroke::new(1.0, ink);
        let s = 5.0; // half the glyph's size, like Windows' caption buttons
        match button {
            WindowButton::Minimize => {
                p.hline(c.x - s..=c.x + s, c.y, stroke);
            }
            WindowButton::Maximize => {
                p.rect_stroke(Rect::from_center_size(c, Vec2::splat(2.0 * s)), CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            }
            WindowButton::Restore => {
                let front = Rect::from_min_size(c + Vec2::new(-s, -s + 2.0), Vec2::splat(2.0 * s - 2.0));
                p.rect_stroke(front, CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
                p.hline(c.x - s + 2.0..=c.x + s, c.y - s, stroke);
                p.vline(c.x + s, c.y - s..=c.y + s - 2.0, stroke);
            }
            WindowButton::Close => {
                p.line_segment([c - Vec2::new(s, s), c + Vec2::new(s, s)], stroke);
                p.line_segment([c - Vec2::new(s, -s), c + Vec2::new(s, -s)], stroke);
            }
        }
        resp
    }

    /// Custom title bars: a hairline round the window, and resizing it from
    /// its edges and corners. Call last each frame.
    pub fn window_edges(ctx: &egui::Context) {
        use egui::CursorIcon as C;
        let (maximized, fullscreen) =
            ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
        if maximized || fullscreen {
            return;
        }
        let _t = theme::t(ctx);
        let r = ctx.content_rect();
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window-border"))).rect_stroke(
            r,
            CornerRadius::ZERO,
            theme::hairline(),
            egui::StrokeKind::Inside,
        );
        let Some(p) = ctx.input(|i| i.pointer.hover_pos()).filter(|p| r.contains(*p)) else { return };
        let near = |d: f32| d < RESIZE_EDGE;
        let corner = |d: f32| d < 3.0 * RESIZE_EDGE;
        let (left, right, top, bottom) = (p.x - r.left(), r.right() - p.x, p.y - r.top(), r.bottom() - p.y);
        let hit = if (near(top) && corner(left)) || (near(left) && corner(top)) {
            Some((ResizeDirection::NorthWest, C::ResizeNorthWest))
        } else if (near(top) && corner(right)) || (near(right) && corner(top)) {
            Some((ResizeDirection::NorthEast, C::ResizeNorthEast))
        } else if (near(bottom) && corner(left)) || (near(left) && corner(bottom)) {
            Some((ResizeDirection::SouthWest, C::ResizeSouthWest))
        } else if (near(bottom) && corner(right)) || (near(right) && corner(bottom)) {
            Some((ResizeDirection::SouthEast, C::ResizeSouthEast))
        } else if near(top) {
            Some((ResizeDirection::North, C::ResizeNorth))
        } else if near(bottom) {
            Some((ResizeDirection::South, C::ResizeSouth))
        } else if near(left) {
            Some((ResizeDirection::West, C::ResizeWest))
        } else if near(right) {
            Some((ResizeDirection::East, C::ResizeEast))
        } else {
            None
        };
        if let Some((direction, cursor)) = hit {
            ctx.set_cursor_icon(cursor);
            if ctx.input(|i| i.pointer.primary_pressed()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
            }
        }
    }
}

/* --------------------------------------------------------------------- */
/* Tabs                                                                     */
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
    /// Last server start failure (bind errors, Windows port-range hints…).
    server_error: Option<String>,
    port: u16,
    port_edit: String,
    settings: Settings,
    data_version: Arc<AtomicU64>,
    /// The active data folder: where Local Data reads from and new downloads
    /// install to. Changed from the Downloads tab (browse / type a path).
    workspace: PathBuf,
    folder_rx: Option<mpsc::Receiver<Option<PathBuf>>>,
    map: MapState,
    layers: PanelState,
    /// User-drawn polygons/paths, shown in the Layers tree and persisted in
    /// the config dir.
    annos: Vec<Anno>,
    downloads: DownloadsState,
    toast: Option<(String, Instant, Color32)>,
    capture: Option<Capture>,
}

impl Shell {
    fn new(cc: &eframe::CreationContext<'_>, app: Arc<App>, port: u16, settings: Settings, screenshot: Option<PathBuf>) -> Self {
        // Auto-start the server: the GUI *is* a tile-server.
        let (server, server_running, server_error) = match spawn_server(port, app.clone()) {
            Ok(h) => (Some(h), true, None),
            Err(e) => {
                let hint = bind_error_hint(&e, port);
                eprintln!("server start failed: {hint}");
                (None, false, Some(hint))
            }
        };
        let workspace = app.data_dir.lock().unwrap().clone();
        let data_version = Arc::new(AtomicU64::new(1));
        let map = MapState::new(app.store.read().unwrap().bbox);
        let mut shell = Shell {
            app,
            tab: Tab::Map,
            server,
            server_running,
            server_error,
            port,
            port_edit: port.to_string(),
            settings,
            data_version: data_version.clone(),
            workspace,
            folder_rx: None,
            map,
            layers: PanelState::default(),
            annos: annotations::load(),
            downloads: DownloadsState::new(),
            toast: None,
            capture: screenshot.map(|dir| Capture { dir, frames: 0, stage: 0 }),
        };
        shell.downloads.data_version = data_version;
        let _ = cc;
        shell
    }

    fn set_toast(&mut self, msg: impl Into<String>, color: Color32) {
        self.toast = Some((msg.into(), Instant::now(), color));
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Live job polling + auto store swap on completion.
        self.downloads.poll(ctx, &self.app, &self.data_version, &self.workspace);

        // A folder picked in the native dialog (Windows).
        if let Some(rx) = &self.folder_rx {
            match rx.try_recv() {
                Ok(Some(path)) => {
                    self.apply_workspace(path);
                    self.folder_rx = None;
                }
                Ok(None) => self.folder_rx = None,
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.folder_rx = None,
            }
        }

        // Expire the toast.
        if let Some((_, at, _)) = &self.toast {
            if at.elapsed() >= Duration::from_secs(5) {
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
        let t = theme::t(&ctx);

        // The header is the title bar (frameless window).
        egui::Panel::top("header")
            .exact_size(HEADER_H)
            .resizable(false)
            .frame(Frame::new().fill(t.bg))
            .show(ui, |ui| self.header_ui(ui));

        if self.tab == Tab::Map {
            egui::Panel::right("layers")
                .exact_size(324.0)
                .resizable(false)
                .frame(Frame::new().fill(t.bg))
                .show(ui, |ui| {
                    let r = ui.max_rect();
                    ui.painter().vline(r.left(), r.y_range(), theme::hairline());
                    let Shell { app, layers, annos, map, data_version, workspace, .. } = self;
                    layers.panel_ui(ui, app, data_version, annos, map, workspace);
                });
        }

        egui::Panel::bottom("footer")
            .exact_size(FOOTER_H)
            .resizable(false)
            .frame(Frame::new().fill(t.bg))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().hline(r.left()..=r.right(), r.top(), theme::hairline());
                ui.add_space(9.0);
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    let stats = self.app.stats();
                    widgets::hud_elided(
                        ui,
                        &format!(
                            "v{} · openstreetmap-carto palette · port {} · {}",
                            tile_server_core::VERSION,
                            self.port,
                            if stats.roads > 0 { format!("{} roads loaded", thousands(stats.roads)) } else { "no data loaded".to_string() }
                        ),
                        t.faint,
                        (r.width() * 0.55).max(240.0),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(20.0);
                        widgets::hud(ui, "© OpenStreetMap contributors", t.faint);
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(Frame::new().fill(t.bg).inner_margin(egui::Margin { left: 20, right: 20, top: 18, bottom: 12 }))
            .show(ui, |ui| match self.tab {
                Tab::Map => self.map_tab(ui),
                Tab::Downloads => self.downloads_tab(ui),
            });

        // Toast (bottom right, above the footer — the launcher's style).
        if let Some((msg, _at, color)) = &self.toast {
            let screen = ctx.input(|i| i.viewport_rect());
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("toast")));
            let galley = painter.layout(msg.clone(), FontId::new(12.5, theme::medium()), t.ink, 340.0);
            let size = Vec2::new(galley.size().x + 46.0, galley.size().y + 20.0);
            let rect = Rect::from_min_size(
                Pos2::new(screen.right() - 20.0 - size.x, screen.bottom() - FOOTER_H - 12.0 - size.y),
                size,
            );
            painter.rect_filled(rect.expand(2.0), CornerRadius::ZERO, Color32::from_black_alpha(140));
            painter.rect_filled(rect, CornerRadius::ZERO, t.bg);
            painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, t.line_strong), egui::StrokeKind::Inside);
            painter.rect_filled(Rect::from_min_size(rect.min + Vec2::new(14.0, rect.height() / 2.0 - 3.0), Vec2::splat(6.0)), CornerRadius::ZERO, *color);
            painter.galley(rect.min + Vec2::new(30.0, 10.0), galley.clone(), t.ink);
        }

        // The frameless window's hairline border + edge resizing, last.
        widgets::window_edges(&ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let t = theme::Tokens::DARK;
        [t.bg.r() as f32 / 255.0, t.bg.g() as f32 / 255.0, t.bg.b() as f32 / 255.0, 1.0]
    }
}

impl Shell {
    /* ---------------- header (the title bar) ---------------- */

    fn header_ui(&mut self, ui: &mut egui::Ui) {
        let t = theme::t(ui.ctx());
        widgets::title_bar_drag(ui);
        let mut switch: Option<Tab> = None;
        ui.horizontal(|ui| {
            ui.add_space(16.0);

            // Logo: two crossing roads on a square tile.
            let (logo, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(logo, CornerRadius::ZERO, t.accent);
            p.line_segment([logo.left_center(), logo.right_center()], Stroke::new(2.5, Color32::WHITE));
            p.line_segment([logo.center_top(), logo.center_bottom()], Stroke::new(1.5, t.invert_bg));
            ui.add_space(10.0);
            ui.label(RichText::new("map-launcher").font(FontId::new(15.0, theme::display())).color(t.ink_strong));
            ui.add_space(8.0);
            widgets::hud(ui, "tile-server", t.faint);
            ui.add_space(16.0);

            // Tabs (MAP | DOWNLOADS) — hard cells sharing one hairline box,
            // the selected one inverted.
            ui.spacing_mut().item_spacing.x = 0.0;
            for (label, tab) in [("Map", Tab::Map), ("Downloads", Tab::Downloads)] {
                let selected = self.tab == tab;
                let galley = widgets::caps(ui, label, HUD_SIZE, theme::mono_bold(), BUTTON_SPACING, t.ink);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 28.0, CONTROL_H), Sense::click());
                let (fill, ink, stroke) = if selected {
                    (t.invert_bg, t.invert_fg, Stroke::new(1.0, t.invert_bg))
                } else if resp.hovered() {
                    (t.sunken, t.ink_strong, theme::hairline())
                } else {
                    (t.bg, t.muted, theme::hairline())
                };
                ui.painter().rect(rect, CornerRadius::ZERO, fill, stroke, egui::StrokeKind::Inside);
                paint_cell_centred(ui, rect, galley, ink);
                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    switch = Some(tab);
                }
            }
            ui.spacing_mut().item_spacing.x = 8.0;

            // Right cluster: server status, port, window buttons.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                widgets::window_buttons(ui, HEADER_H);
                ui.add_space(12.0);

                // Port field (editable while the server is stopped).
                let port_w = 52.0;
                widgets::hud(ui, "port", t.faint);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.port_edit)
                        .desired_width(port_w)
                        .font(FontId::new(12.0, theme::mono()))
                        .vertical_align(Align::Center)
                        .margin(Vec2::new(8.0, 0.0))
                        .min_size(Vec2::new(port_w, 24.0)),
                );
                ui.add_space(4.0);
                if !self.server_running {
                    if resp.changed() {
                        if let Ok(p) = self.port_edit.trim().parse::<u16>() {
                            if (1..=65535).contains(&p) && p != self.port {
                                self.port = p;
                                self.settings.port = Some(p);
                                self.settings.save();
                            }
                        }
                    }
                } else if resp.hovered() {
                    resp.on_hover_text("Stop the server to change the port");
                }

                widgets::rule(ui, 22.0);
                ui.add_space(12.0);

                // Live dot + URL + copy.
                let (dot_r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                ui.painter().circle_filled(dot_r.center(), 4.0, if self.server_running { t.good } else { t.muted });
                let url = format!("http://127.0.0.1:{}", self.port);
                let label = if self.server_running { url.clone() } else { "server off".to_string() };
                widgets::hud(ui, &label, if self.server_running { t.ink } else { t.muted });
                ui.add_space(2.0);
                if widgets::glyph_button(ui, widgets::Glyph::Copy).clicked() {
                    ui.ctx().copy_text(url.clone());
                    self.set_toast("Server URL copied", t.accent_ink);
                }
            });
        });
        // Hairline under the header.
        let r = ui.max_rect();
        ui.painter().hline(r.left()..=r.right(), r.bottom(), theme::hairline());
        if let Some(tab) = switch {
            self.tab = tab;
        }
    }

    /// The "no data" / "server failed" banners, shown at the top of a tab.
    fn notices_ui(&mut self, ui: &mut egui::Ui) {
        let t = theme::t(ui.ctx());
        let empty = self.app.stats().roads == 0;
        if empty {
            let searched = self.app.data_dir.lock().unwrap().display().to_string();
            let exe_temp = std::env::current_exe().ok().and_then(|p| Some(p.display().to_string())).map(|p| p.contains("Rar$") || p.contains("\\Temp\\") || p.contains("/Temp/")).unwrap_or(false);
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(t.accent.r(), t.accent.g(), t.accent.b(), 26))
                .stroke(Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(t.accent.r(), t.accent.g(), t.accent.b(), 153)))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        widgets::badge(ui, "no data", t.warn);
                        ui.label(RichText::new("store.bin was not found, so the registry is empty.").size(12.5).color(t.ink));
                    });
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("  searched: {searched}")).size(10.5).monospace().color(t.muted));
                    if exe_temp {
                        ui.label(RichText::new("  the exe is running from a temp folder (WinRAR) — fully extract the release zip first, then browse to it below.").size(10.5).monospace().color(t.warn));
                    }
                    ui.label(RichText::new("  1 · open the Downloads tab and pick a region (downloads install into the data folder)").size(10.5).monospace().color(t.muted));
                    ui.label(RichText::new("  2 · or change the data folder to one that already holds store.bin").size(10.5).monospace().color(t.muted));
                    ui.label(RichText::new("  3 · or tile-server extract <pbf> <data-dir> [w s e n]").size(10.5).monospace().color(t.muted));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if widgets::button(ui, "Open downloads", widgets::Kind::Primary, 0.0).clicked() {
                            self.tab = Tab::Downloads;
                        }
                        if widgets::button(ui, "Change data folder…", widgets::Kind::Secondary, 0.0).clicked() {
                            self.browse_folder();
                        }
                    });
                });
            ui.add_space(10.0);
        }
        if let Some(err) = &self.server_error.clone() {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(t.bad.r(), t.bad.g(), t.bad.b(), 22))
                .stroke(Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(t.bad.r(), t.bad.g(), t.bad.b(), 140)))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        widgets::badge(ui, "server", t.bad);
                        let first = err.lines().next().unwrap_or("").to_string();
                        ui.label(RichText::new(first).size(12.5).color(t.ink));
                    });
                    for line in err.lines().skip(1) {
                        ui.label(RichText::new(line.to_string()).size(10.5).monospace().color(t.muted));
                    }
                });
            ui.add_space(10.0);
        }
    }

    /* ---------------- Map tab ---------------- */

    fn map_tab(&mut self, ui: &mut egui::Ui) {
        let t = theme::t(ui.ctx());
        // Toolbar row.
        ui.horizontal(|ui| {
            let label = if self.server_running { "Stop server" } else { "Start server" };
            let kind = if self.server_running { widgets::Kind::Secondary } else { widgets::Kind::Primary };
            if widgets::button(ui, label, kind, 0.0).clicked() {
                self.toggle_server();
            }
            ui.add_space(6.0);
            let (dot_r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
            ui.painter().circle_filled(dot_r.center(), 4.0, if self.server_running { t.good } else { t.muted });
            ui.add_space(4.0);
            let url = format!("http://127.0.0.1:{}", self.port);
            widgets::hud(ui, &url, t.muted);
            ui.add_space(10.0);
            let stats = self.app.stats();
            widgets::hud_elided(
                ui,
                &format!(
                    "{} roads · {} buildings · {} pois",
                    thousands(stats.roads),
                    thousands(stats.buildings),
                    thousands(stats.pois)
                ),
                t.faint,
                360.0,
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let region = self.app.region_name.lock().unwrap().clone();
                widgets::hud_elided(ui, &region, t.ink, 180.0);
                ui.add_space(10.0);

                // Canvas tools (SELECT | POLYGON | PATH) — the same hard
                // cells the header tabs use; drawing state lives on MapState.
                // (Inside the right-to-left layout, so they sit left of the
                // region label; reversed to read left-to-right.)
                ui.spacing_mut().item_spacing.x = 0.0;
                for (label, tool) in [(Tool::Path.label(), Tool::Path), (Tool::Polygon.label(), Tool::Polygon), (Tool::Select.label(), Tool::Select)] {
                    let selected = self.map.tool == tool;
                    let galley = widgets::caps(ui, label, HUD_SIZE - 1.0, theme::mono_bold(), BUTTON_SPACING, t.ink);
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 22.0, CONTROL_H - 2.0), Sense::click());
                    let (fill, ink, stroke) = if selected {
                        (t.invert_bg, t.invert_fg, Stroke::new(1.0, t.invert_bg))
                    } else if resp.hovered() {
                        (t.sunken, t.ink_strong, theme::hairline())
                    } else {
                        (t.bg, t.muted, theme::hairline())
                    };
                    ui.painter().rect(rect, CornerRadius::ZERO, fill, stroke, egui::StrokeKind::Inside);
                    paint_cell_centred(ui, rect, galley, ink);
                    if resp.clicked() {
                        self.map.tool = tool;
                        if tool != Tool::Select {
                            self.map.work.clear();
                        }
                    }
                    ui.add_space(2.0);
                }
                ui.add_space(6.0);
            });
        });
        if self.map.tool != Tool::Select {
            widgets::hud(
                ui,
                match self.map.tool {
                    Tool::Polygon => "polygon — click: add point · click the first point / Enter: close · Backspace: undo · Esc: cancel · right-drag: pan",
                    _ => "path — click: add point · Enter / double-click: finish · Backspace: undo · Esc: cancel · right-drag: pan",
                },
                t.accent_ink,
            );
            ui.add_space(2.0);
        }
        ui.add_space(6.0);

        self.notices_ui(ui);

        // Canvas fills the rest (the layers live in the right-side panel).
        let Shell { app, map, annos, layers, data_version, port, .. } = self;
        egui::Frame::new().fill(t.bg).stroke(theme::hairline()).show(ui, |ui| {
            map.canvas_ui(ui, app, data_version, *port, annos, &mut layers.selected);
        });
    }

    fn toggle_server(&mut self) {
        let t = theme::Tokens::DARK;
        if self.server_running {
            if let Some(h) = self.server.take() {
                h.stop();
            }
            self.server_running = false;
            self.server_error = None;
            self.set_toast(format!("Server stopped (was :{})", self.port), t.warn);
        } else {
            match spawn_server(self.port, self.app.clone()) {
                Ok(h) => {
                    self.server = Some(h);
                    self.server_running = true;
                    self.server_error = None;
                    self.set_toast(format!("Serving tiles on :{}", self.port), t.good);
                }
                Err(e) => {
                    self.server_running = false;
                    let hint = bind_error_hint(&e, self.port);
                    eprintln!("server start failed: {hint}");
                    self.server_error = Some(hint);
                    self.set_toast("Start failed — see the banner for the fix", t.bad);
                }
            }
        }
    }

    /* ---------------- Downloads tab ---------------- */

    fn downloads_tab(&mut self, ui: &mut egui::Ui) {
        let t = theme::t(ui.ctx());

        // Data folder row.
        ui.horizontal(|ui| {
            widgets::hud(ui, "data folder", t.faint);
            ui.add_space(4.0);
            let ws = self.workspace.display().to_string();
            widgets::hud_elided(ui, &ws, t.muted, (ui.available_width() - 330.0).max(120.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::button(ui, "Change folder…", widgets::Kind::Secondary, 0.0).clicked() {
                    self.browse_folder();
                }
                if widgets::glyph_button(ui, widgets::Glyph::Folder).clicked() {
                    open_folder(&self.workspace);
                    self.set_toast(format!("Opened {}", self.workspace.display()), t.accent_ink);
                }
            });
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            widgets::hud(ui, "or type a path", t.faint);
            let w = ui.available_width() - 64.0;
            ui.add(
                egui::TextEdit::singleline(&mut self.downloads.path_edit)
                    .hint_text(if cfg!(windows) { "C:\\maps\\tanzania-data" } else { "/home/user/maps/tanzania-data" })
                    .desired_width(w.max(120.0))
                    .font(FontId::new(12.0, theme::mono()))
                    .vertical_align(Align::Center)
                    .margin(Vec2::new(10.0, 0.0))
                    .min_size(Vec2::new(0.0, CONTROL_H)),
            );
            if widgets::button(ui, "Set", widgets::Kind::Secondary, 0.0).clicked() {
                let p = PathBuf::from(self.downloads.path_edit.trim());
                if !p.as_os_str().is_empty() {
                    self.apply_workspace(p);
                }
            }
        });
        ui.add_space(8.0);

        self.notices_ui(ui);

        egui::ScrollArea::vertical().id_salt("downloads").auto_shrink([false, false]).show(ui, |ui| {
            // ---- Tanzania map parts table ----
            widgets::hud(ui, "tanzania map parts — download · extract · serve", t.faint);
            ui.add_space(6.0);
            self.regions_table(ui);
            ui.add_space(16.0);

            // ---- Custom source ----
            widgets::hud(ui, "custom source — any osm pbf url", t.faint);
            ui.add_space(6.0);
            egui::Frame::new().fill(t.bg).stroke(theme::hairline()).inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let w = ui.available_width() - 220.0;
                    ui.add(
                        egui::TextEdit::singleline(&mut self.downloads.custom_url)
                            .hint_text("https://…/any-extract.osm.pbf")
                            .desired_width(w.max(120.0))
                            .font(FontId::new(12.0, theme::mono()))
                            .vertical_align(Align::Center)
                            .margin(Vec2::new(10.0, 0.0))
                            .min_size(Vec2::new(0.0, CONTROL_H)),
                    );
                    let enabled = self.downloads.custom_url.trim().starts_with("http");
                    ui.add_enabled_ui(enabled, |ui| {
                        if widgets::button(ui, "Download & extract", widgets::Kind::Primary, 0.0).clicked() {
                            let url = self.downloads.custom_url.trim().to_string();
                            let stem = pbf_name_for(&url).trim_end_matches(".osm.pbf").trim_end_matches(".pbf").to_string();
                            let id = format!("custom-{}", sanitize(&stem));
                            self.downloads.start(&self.app, &id, &stem, self.downloads.custom_bbox_world, &url, &self.workspace);
                        }
                    });
                });
            });
            ui.add_space(16.0);

            // ---- Local data table ----
            widgets::hud(ui, &format!("local data — {}", self.workspace.display()), t.faint);
            ui.add_space(6.0);
            self.local_table(ui);
            ui.add_space(10.0);
            widgets::hud(
                ui,
                "data © openstreetmap contributors (odbl) · pbf extracts by download.geofabrik.de",
                t.faint,
            );
        });
    }

    /// The craft-launcher-style region table.
    fn regions_table(&mut self, ui: &mut egui::Ui) {
        let t = theme::t(ui.ctx());
        let regions: Vec<Region> = tile_server_core::download::regions();
        let width = ui.available_width();
        let (act_w, st_w, size_w, cov_w) = (300.0f32, 140.0, 100.0, 230.0);
        let name_w = (width - act_w - st_w - size_w - cov_w).max(180.0);

        // Header row (42 px, HUD labels).
        let header = |ui: &mut egui::Ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(width, 42.0), Sense::hover());
            let p = ui.painter();
            let cells = [("region", 0.0, name_w), ("coverage", name_w, cov_w), ("size", name_w + cov_w, size_w), ("status", name_w + cov_w + size_w, st_w), ("", width - act_w, act_w)];
            for (label, _x, _w) in cells {
                if label.is_empty() {
                    continue;
                }
                let galley = widgets::caps(ui, label, HUD_SIZE, theme::mono(), HUD_SPACING, t.faint);
                p.galley_with_override_text_color(Pos2::new(r.left() + _x + 2.0, r.center().y - galley.size().y / 2.0), galley, t.faint);
            }
            p.hline(r.left()..=r.right(), r.bottom(), theme::hairline());
        };
        header(ui);

        let query = self.downloads.search.trim().to_lowercase();
        let active_region_dir = self.app.data_dir.lock().unwrap().clone();
        let region_name = self.app.region_name.lock().unwrap().clone();

        egui::ScrollArea::vertical().id_salt("region-rows").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for r in &regions {
                if !query.is_empty() && !(r.name.to_lowercase().contains(&query) || r.desc.to_lowercase().contains(&query) || r.id.contains(&query)) {
                    continue;
                }
                let phase = self.downloads.jobs.get(r.id).cloned().unwrap_or(Phase::Idle);
                let dir = self.regions_dir().join(r.id);
                let installed = store_installed(&dir) || matches!(phase, Phase::Installed { .. });
                let in_use = installed && (active_region_dir == dir || region_name == r.name);
                self.region_row(ui, r, &phase, installed, in_use, name_w, cov_w, size_w, st_w, act_w);
            }
            if query.is_empty() {
                // nothing extra
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn region_row(&mut self, ui: &mut egui::Ui, r: &Region, phase: &Phase, installed: bool, in_use: bool, name_w: f32, cov_w: f32, size_w: f32, st_w: f32, act_w: f32) {
        let t = theme::t(ui.ctx());
        let dir = self.regions_dir().join(r.id);
        let width = ui.available_width();
        let h = 64.0f32;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
        if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, t.sunken.gamma_multiply(0.5));
        }
        let p = ui.painter();
        p.hline(rect.left()..=rect.right(), rect.bottom(), theme::hairline());

        // Region cell: name (display) + description (muted).
        let name_cell = Rect::from_min_size(rect.min, Vec2::new(name_w, h));
        let np = ui.painter_at(name_cell.shrink2(Vec2::new(2.0, 0.0)));
        np.text(Pos2::new(name_cell.left() + 2.0, name_cell.top() + 14.0), Align2::LEFT_CENTER, r.name, FontId::new(15.0, theme::display()), t.ink_strong);
        let desc_g = np.layout(r.desc.to_string(), FontId::new(11.5, FontFamily::Proportional), t.muted, name_w - 12.0);
        np.galley(Pos2::new(name_cell.left() + 2.0, name_cell.top() + 30.0), desc_g, t.muted);

        // Coverage cell: bbox + shared/direct + source link.
        let cov_cell = Rect::from_min_size(rect.min + Vec2::new(name_w, 0.0), Vec2::new(cov_w, h));
        let cp = ui.painter_at(cov_cell);
        cp.text(Pos2::new(cov_cell.left() + 2.0, cov_cell.center().y - 14.0), Align2::LEFT_CENTER, format!("{:.1}, {:.1} → {:.1}, {:.1}", r.bbox[0], r.bbox[1], r.bbox[2], r.bbox[3]), FontId::new(10.5, FontFamily::Monospace), t.muted);
        cp.text(Pos2::new(cov_cell.left() + 2.0, cov_cell.center().y + 0.0), Align2::LEFT_CENTER, if r.shared { "shared pbf crop" } else { "direct download" }, FontId::new(9.0, theme::mono()), t.faint);
        let mut cup = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(cov_cell.min + Vec2::new(2.0, cov_cell.height() / 2.0 + 6.0), Vec2::new(cov_w - 6.0, 16.0))));
        widgets::hud_link(&mut cup, &shorten_url(r.url, 26), r.url);

        // Size cell.
        let size_cell = Rect::from_min_size(rect.min + Vec2::new(name_w + cov_w, 0.0), Vec2::new(size_w, h));
        ui.painter_at(size_cell).text(size_cell.left_top() + Vec2::new(2.0, h / 2.0), Align2::LEFT_CENTER, r.size_hint, FontId::new(10.5, FontFamily::Monospace), t.muted);

        // Status cell.
        let st_cell = Rect::from_min_size(rect.min + Vec2::new(name_w + cov_w + size_w, 0.0), Vec2::new(st_w, h));
        let mut sui = ui.new_child(egui::UiBuilder::new().max_rect(st_cell).layout(Layout::left_to_right(Align::Center)));
        match phase {
            Phase::Idle if in_use => widgets::badge(&mut sui, "in use", t.good),
            Phase::Idle if installed => widgets::badge(&mut sui, "installed", t.good),
            Phase::Idle => widgets::badge(&mut sui, "not installed", t.muted),
            Phase::Queued => widgets::badge(&mut sui, "queued", t.warn),
            Phase::Downloading { .. } | Phase::Reconnecting { .. } => widgets::badge(&mut sui, "downloading", t.accent_ink),
            Phase::Extracting { .. } => widgets::badge(&mut sui, "extracting", t.warn),
            Phase::Installed { .. } => widgets::badge(&mut sui, "installed", t.good),
            Phase::Failed { .. } => widgets::badge(&mut sui, "failed", t.bad),
        };

        // Action cell: progress + cancel while active, buttons otherwise.
        let act_cell = Rect::from_min_size(rect.min + Vec2::new(name_w + cov_w + size_w + st_w, 0.0), Vec2::new(act_w, h));
        let mut aui = ui.new_child(egui::UiBuilder::new().max_rect(act_cell.shrink2(Vec2::new(0.0, 13.0))).layout(Layout::left_to_right(Align::Center)));
        let active = phase.is_active();
        if active {
            let bar_w = act_w - CONTROL_H - 8.0;
            match phase {
                Phase::Downloading { done, total } | Phase::Reconnecting { done, total } => {
                    let frac = if *total > 0 { Some(*done as f32 / *total as f32) } else { None };
                    let caption = if matches!(phase, Phase::Reconnecting { .. }) {
                        format!("Reconnecting — {} of {}", fmt_bytes(*done), if *total > 0 { fmt_bytes(*total) } else { "?".into() })
                    } else {
                        format!("{} of {} · {}%", fmt_bytes(*done), if *total > 0 { fmt_bytes(*total) } else { "?".into() }, (frac.unwrap_or(0.0) * 100.0) as u32)
                    };
                    widgets::progress(&mut aui, frac, &caption, t.accent_ink, bar_w);
                }
                Phase::Extracting { message } => widgets::progress(&mut aui, None, message, t.warn, bar_w),
                _ => widgets::progress(&mut aui, None, "Working…", t.accent_ink, bar_w),
            }
            if widgets::glyph_button(&mut aui, widgets::Glyph::Close).clicked() {
                self.downloads.manager.cancel(r.id);
                self.set_toast(format!("Cancelled {}", r.name), t.warn);
            }
        } else {
            aui.with_layout(Layout::right_to_left(Align::Center), |aui| {
                let dl_label = if installed { "Re-download" } else { "Download" };
                let kind = if installed { widgets::Kind::Secondary } else { widgets::Kind::Primary };
                if widgets::button(aui, dl_label, kind, 0.0).clicked() {
                    self.downloads.start(&self.app, r.id, r.name, r.bbox, r.url, &self.workspace);
                }
                if installed && !in_use && widgets::button(aui, "Use", widgets::Kind::Secondary, 0.0).clicked() {
                    match carto_core::Store::load(&dir) {
                        Ok(store) => {
                            self.app.set_store(store, r.name.to_string(), dir.clone());
                            self.data_version.fetch_add(1, Ordering::Relaxed);
                            self.set_toast(format!("Serving {}", r.name), t.good);
                        }
                        Err(e) => {
                            self.set_toast(format!("store load failed: {e}"), t.bad);
                        }
                    }
                }
            });
        }
    }

    /// Files already in the workspace: what you have locally.
    fn local_table(&mut self, ui: &mut egui::Ui) {
        let t = theme::t(ui.ctx());
        let width = ui.available_width();
        let (act_w, mod_w, size_w, kind_w) = (170.0f32, 110.0, 90.0, 110.0);
        let name_w = (width - act_w - mod_w - size_w - kind_w).max(160.0);

        let (r, _) = ui.allocate_exact_size(Vec2::new(width, 36.0), Sense::hover());
        let p = ui.painter();
        for (label, _x, _w) in [("file", 0.0, name_w), ("kind", name_w, kind_w), ("size", name_w + kind_w, size_w), ("modified", name_w + kind_w + size_w, mod_w)] {
            let galley = widgets::caps(ui, label, HUD_SIZE, theme::mono(), HUD_SPACING, t.faint);
            p.galley_with_override_text_color(Pos2::new(r.left() + _x + 2.0, r.center().y - galley.size().y / 2.0), galley, t.faint);
        }
        p.hline(r.left()..=r.right(), r.bottom(), theme::hairline());

        // Scan the workspace.
        let ws = self.workspace.clone();
        let active_dir = self.app.data_dir.lock().unwrap().clone();
        let mut rows: Vec<(String, String, u64, String, PathBuf, bool)> = Vec::new(); // name, kind, size, modified, path, is_dir
        if let Ok(rd) = std::fs::read_dir(&ws) {
            let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let name = e.file_name().to_string_lossy().to_string();
                let path = e.path();
                let meta = e.metadata().ok();
                let modified = meta.as_ref().and_then(|m| m.modified().ok()).map(|t| {
                    let d = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
                    // YYYY-MM-DD HH:MM (local-free, UTC label avoided)
                    let days = d / 86_400;
                    let (y, mo, da) = civil_from_days(days as i64);
                    let (h, mi) = ((d % 86_400) / 3600, (d % 3600) / 60);
                    format!("{y:04}-{mo:02}-{da:02} {h:02}:{mi:02}")
                }).unwrap_or_else(|| "—".into());
                if path.is_dir() {
                    if path.join("store.bin").is_file() {
                        let size = std::fs::metadata(path.join("store.bin")).map(|m| m.len()).unwrap_or(0);
                        rows.push((name, "region".into(), size, modified, path, true));
                    }
                } else {
                    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                    let lower = name.to_lowercase();
                    let kind = match lower.as_str() {
                        "store.bin" => "store".to_string(),
                        "registry.json" => "registry".to_string(),
                        s if s.ends_with(".osm.pbf") || s.ends_with(".pbf") => "source pbf".to_string(),
                        _ => continue,
                    };
                    rows.push((name, kind, size, modified, path, false));
                }
            }
        }

        if rows.is_empty() {
            ui.add_space(24.0);
            let galley = widgets::caps(ui, "no supported files here — download a region above, or pick another folder", HUD_SIZE, theme::mono(), HUD_SPACING, t.faint);
            let x = (width - galley.size().x) / 2.0;
            ui.painter().galley_with_override_text_color(Pos2::new(x, ui.cursor().top()), galley, t.faint);
            ui.add_space(40.0);
            return;
        }

        egui::ScrollArea::vertical().id_salt("local-rows").max_height(240.0).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (name, kind, size, modified, path, is_region) in rows {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::hover());
                if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::ZERO, t.sunken.gamma_multiply(0.5));
                }
                let p = ui.painter();
                p.hline(rect.left()..=rect.right(), rect.bottom(), theme::hairline());
                let is_active = path == active_dir || (path.join("store.bin").is_file() && path == active_dir);
                let paint = |ui: &egui::Ui, x: f32, w: f32, text: String, font: FontId, color: Color32| {
                    let cp = ui.painter_at(Rect::from_min_size(rect.min + Vec2::new(x, 0.0), Vec2::new(w, 34.0)));
                    cp.text(Pos2::new(rect.left() + x + 2.0, rect.center().y), Align2::LEFT_CENTER, text, font, color);
                };
                paint(ui, 0.0, name_w, name.clone(), FontId::new(11.5, FontFamily::Monospace), if is_active { t.ink_strong } else { t.ink });
                paint(ui, name_w, kind_w, kind.clone(), FontId::new(9.0, theme::mono()), t.faint);
                paint(ui, name_w + kind_w, size_w, fmt_bytes(size), FontId::new(10.5, FontFamily::Monospace), t.muted);
                paint(ui, name_w + kind_w + size_w, mod_w, modified, FontId::new(10.5, FontFamily::Monospace), t.muted);
                let mut aui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(rect.min + Vec2::new(name_w + kind_w + size_w + mod_w, 0.0), Vec2::new(act_w, 34.0))).layout(Layout::right_to_left(Align::Center)));
                aui.add_space(6.0);
                if is_active {
                    widgets::badge(&mut aui, "active", t.good);
                } else if is_region {
                    if widgets::button(&mut aui, "Use", widgets::Kind::Secondary, 0.0).clicked() {
                        match carto_core::Store::load(&path) {
                            Ok(store) => {
                                let label = region_label(&name);
                                self.app.set_store(store, label, path.clone());
                                self.data_version.fetch_add(1, Ordering::Relaxed);
                                self.set_toast(format!("Serving {name}"), t.good);
                            }
                            Err(e) => self.set_toast(format!("store load failed: {e}"), t.bad),
                        }
                    }
                }
            }
        });
    }

    /* ---------------- workspace (data folder) ---------------- */

    fn regions_dir(&self) -> PathBuf {
        self.workspace.join("regions")
    }

    fn browse_folder(&mut self) {
        #[cfg(windows)]
        {
            let (tx, rx) = mpsc::channel();
            self.folder_rx = Some(rx);
            std::thread::spawn(move || {
                let picked = rfd::FileDialog::new().set_title("Choose the data folder (it holds or will hold store.bin)").pick_folder();
                let _ = tx.send(picked);
            });
        }
        #[cfg(not(windows))]
        {
            self.set_toast("Type a folder path into the field below — the native picker ships on Windows", theme::Tokens::DARK.accent_ink);
        }
    }

    /// Point the app at a folder: load its store when it has one, and use it
    /// for new downloads either way.
    fn apply_workspace(&mut self, picked: PathBuf) {
        let t = theme::Tokens::DARK;
        let with_store = if picked.join("store.bin").is_file() {
            Some(picked.clone())
        } else if picked.join("data").join("store.bin").is_file() {
            Some(picked.join("data"))
        } else {
            None
        };
        if let Some(dir) = with_store {
            match carto_core::Store::load(&dir) {
                Ok(store) => {
                    let label = region_label(&dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                    let label = if label.is_empty() { "local folder".to_string() } else { label };
                    self.app.set_store(store, label, dir.clone());
                    self.workspace = dir;
                    self.data_version.fetch_add(1, Ordering::Relaxed);
                    self.set_toast(format!("Data folder set — serving {}", self.workspace.display()), t.good);
                }
                Err(e) => {
                    self.workspace = picked;
                    self.set_toast(format!("store.bin found but failed to load: {e}"), t.bad);
                }
            }
        } else {
            self.workspace = picked;
            self.set_toast("Folder set — new downloads will install here", t.accent_ink);
        }
        self.settings.data_dir = Some(self.workspace.clone());
        self.settings.save();
    }
}

/// Paint a galley centred in `rect` (free function so closures can use it).
fn paint_cell_centred(ui: &egui::Ui, rect: Rect, galley: Arc<egui::Galley>, color: Color32) {
    ui.painter().galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, color);
}

fn open_folder(dir: &Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(dir).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// Days-since-epoch to (y, m, d) — civil calendar, no external crates.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }), m, d)
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

/// The selected row of the Layers panel — an engine layer id or a drawing.
#[derive(Clone, Debug, PartialEq)]
enum Sel {
    Layer(String),
    Anno(usize),
}

/// The Map canvas tools: pan/select, or the pen tools that draw annotations.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tool {
    Select,
    Polygon,
    Path,
}

impl Tool {
    fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Polygon => "Polygon",
            Tool::Path => "Path",
        }
    }
}

struct MapState {
    /// Fractional zoom (4..=19).
    zoom: f32,
    /// Center lon/lat.
    center: (f64, f64),
    tex: HashMap<(u8, u32, u32), (TextureHandle, u64)>,
    /// The active canvas tool (also switched from the toolbar and the Layers
    /// panel's new-drawing buttons).
    tool: Tool,
    /// In-progress pen vertices (lon, lat).
    work: Vec<(f64, f64)>,
    /// An annotation being moved whole.
    move_drag: Option<usize>,
    /// The canvas rect last frame (fit-to-selection, hit tests).
    last_rect: Option<Rect>,
}

impl MapState {
    fn new(bbox: [f64; 4]) -> Self {
        Self {
            zoom: 12.0,
            center: ((bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0),
            tex: HashMap::new(),
            tool: Tool::Select,
            work: vec![],
            move_drag: None,
            last_rect: None,
        }
    }

    fn z(&self) -> u8 {
        self.zoom.round().clamp(4.0, 19.0) as u8
    }

    /// lon/lat → canvas pixel.
    fn project(&self, rect: Rect, lon: f64, lat: f64) -> Pos2 {
        let world = world_px(self.z()) as f64;
        let (cx_m, cy_m) = lonlat_to_merc(self.center.0, self.center.1);
        let tl = (cx_m * world - rect.width() as f64 / 2.0, cy_m * world - rect.height() as f64 / 2.0);
        let (mx, my) = lonlat_to_merc(lon, lat);
        Pos2::new((mx * world - tl.0) as f32 + rect.left(), (my * world - tl.1) as f32 + rect.top())
    }

    /// Canvas pixel → lon/lat.
    fn unproject(&self, rect: Rect, p: Pos2) -> (f64, f64) {
        let world = world_px(self.z()) as f64;
        let (cx_m, cy_m) = lonlat_to_merc(self.center.0, self.center.1);
        let tl = (cx_m * world - rect.width() as f64 / 2.0, cy_m * world - rect.height() as f64 / 2.0);
        let mx = ((p.x as f64 - rect.left() as f64 + tl.0) / world).clamp(0.0, 1.0);
        let my = ((p.y as f64 - rect.top() as f64 + tl.1) / world).clamp(0.0, 1.0);
        merc_to_lonlat(mx, my)
    }

    /// Center + zoom so `bbox` fits the canvas.
    fn fit_bbox(&mut self, rect: Rect, bbox: [f64; 4]) {
        let (x0, y0) = lonlat_to_merc(bbox[0], bbox[3]);
        let (x1, y1) = lonlat_to_merc(bbox[2], bbox[1]);
        self.center = merc_to_lonlat((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let w = (x1 - x0).abs().max(1e-9);
        let h = (y1 - y0).abs().max(1e-9);
        let fit = ((rect.width() as f64 * 0.8) / w).min((rect.height() as f64 * 0.8) / h);
        self.zoom = ((fit / 256.0).log2() as f32).clamp(4.0, 19.0);
    }

    /// Frame an annotation in view.
    fn fit_annotation(&mut self, rect: Rect, a: &Anno) {
        if a.pts.is_empty() {
            return;
        }
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for &(lon, lat) in &a.pts {
            b = [b[0].min(lon), b[1].min(lat), b[2].max(lon), b[3].max(lat)];
        }
        self.fit_bbox(rect, b);
    }

    /// Whether pointer `p` is over annotation `a` (inside a polygon, or near
    /// the stroke of either kind).
    fn anno_hit(&self, rect: Rect, a: &Anno, p: Pos2) -> bool {
        let pts: Vec<Pos2> = a.pts.iter().map(|&(lon, lat)| self.project(rect, lon, lat)).collect();
        if a.kind == "polygon" && pts.len() >= 3 && point_in_poly(&pts, p) {
            return true;
        }
        let w = (a.width.max(2.0) * 2.5).clamp(6.0, 14.0);
        pts.windows(2).any(|s| dist_pt_seg(p, s[0], s[1]) <= w)
    }

    /// Close the pen: turn the work vertices into an annotation, select it
    /// and go back to Select.
    fn finish_work(&mut self, annos: &mut Vec<Anno>, sel: &mut Option<Sel>) {
        let kind = match self.tool {
            Tool::Polygon => "polygon",
            Tool::Path => "path",
            Tool::Select => return,
        };
        // Drop a duplicated last point (double-click to finish adds one).
        while self.work.len() >= 2 {
            let (a, b) = (self.work[self.work.len() - 2], self.work[self.work.len() - 1]);
            if (a.0 - b.0).abs() < 1e-7 && (a.1 - b.1).abs() < 1e-7 {
                self.work.pop();
            } else {
                break;
            }
        }
        let ok = if kind == "polygon" { self.work.len() >= 3 } else { self.work.len() >= 2 };
        if ok {
            let n = annos.iter().filter(|a| a.kind == kind).count() + 1;
            let id = Anno::next_id(annos);
            annos.push(Anno {
                id,
                name: if kind == "polygon" { format!("Polygon {n}") } else { format!("Path {n}") },
                kind: kind.into(),
                pts: self.work.clone(),
                color: [0x3f, 0xd6, 0xb8],
                opacity: 1.0,
                width: 2.0,
                filled: true,
                visible: true,
                locked: false,
            });
            *sel = Some(Sel::Anno(annos.len() - 1));
            annotations::save(annos);
        }
        self.work.clear();
        self.tool = Tool::Select;
    }

    fn canvas_ui(&mut self, ui: &mut egui::Ui, app: &Arc<App>, data_version: &Arc<AtomicU64>, port: u16, annos: &mut Vec<Anno>, sel: &mut Option<Sel>) {
        ui.add_space(8.0);
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());

        // --- interaction ---------------------------------------------------
        let z_int = self.z();
        let scale = world_px(z_int) as f64;
        let annos_group_on = app.layers.read().unwrap().is_visible("annotations");

        if self.tool == Tool::Select {
            if resp.drag_started() {
                // Grab a drawing under the pointer, else pan.
                self.move_drag = resp
                    .interact_pointer_pos()
                    .and_then(|p| annos.iter().enumerate().rev().find(|(_, a)| annos_group_on && a.visible && !a.locked && self.anno_hit(rect, a, p)).map(|(i, _)| i));
            }
            if resp.dragged() {
                let d = resp.drag_delta();
                match self.move_drag {
                    Some(i) => {
                        if let Some(a) = annos.get_mut(i) {
                            for pt in &mut a.pts {
                                let (mx, my) = lonlat_to_merc(pt.0, pt.1);
                                *pt = merc_to_lonlat((mx + d.x as f64 / scale).clamp(0.0, 1.0), (my + d.y as f64 / scale).clamp(0.0, 1.0));
                            }
                        }
                    }
                    None => {
                        let (mut mx, mut my) = lonlat_to_merc(self.center.0, self.center.1);
                        mx -= d.x as f64 / scale;
                        my += d.y as f64 / scale;
                        self.center = merc_to_lonlat(mx.clamp(0.0, 1.0), my.clamp(0.0, 1.0));
                    }
                }
            }
            if resp.drag_stopped() && self.move_drag.is_some() {
                self.move_drag = None;
                annotations::save(annos);
            }
            if resp.clicked() {
                *sel = resp
                    .interact_pointer_pos()
                    .and_then(|p| annos.iter().enumerate().rev().find(|(_, a)| annos_group_on && a.visible && self.anno_hit(rect, a, p)).map(|(i, _)| Sel::Anno(i)));
            }
            if resp.double_clicked() {
                self.zoom = (self.zoom + 1.0).clamp(4.0, 19.0);
            }
        } else {
            // Pen tools: left-click adds points, right-drag pans.
            if ui.input(|i| i.pointer.secondary_down()) && resp.hovered() {
                let d = ui.input(|i| i.pointer.delta());
                let (mut mx, mut my) = lonlat_to_merc(self.center.0, self.center.1);
                mx -= d.x as f64 / scale;
                my += d.y as f64 / scale;
                self.center = merc_to_lonlat(mx.clamp(0.0, 1.0), my.clamp(0.0, 1.0));
            }
            if resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    if let Some(first) = self.work.first() {
                        let fp = self.project(rect, first.0, first.1);
                        if p.distance(fp) <= 10.0 && self.work.len() >= 3 {
                            self.finish_work(annos, sel);
                            return;
                        }
                    }
                    let ll = self.unproject(rect, p);
                    self.work.push(ll);
                }
            }
            if resp.double_clicked() {
                self.finish_work(annos, sel);
            }
            let (enter, back, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Backspace), i.key_pressed(egui::Key::Escape)));
            if enter {
                self.finish_work(annos, sel);
            }
            if back {
                self.work.pop();
            }
            if esc {
                self.work.clear();
                self.tool = Tool::Select;
            }
        }
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if resp.hovered() && scroll.abs() > 0.5 {
            self.zoom = (self.zoom + scroll * 0.01).clamp(4.0, 19.0);
        }

        // --- paint ----------------------------------------------------------
        let painter = ui.painter_at(rect);
        let land = app.palette.get("land-color").unwrap_or(carto_style::Rgba::rgb(0xf2, 0xef, 0xe9));
        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(land.r, land.g, land.b));

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

        // --- annotations (above tiles, below the badges) ---------------------
        let t = theme::Tokens::DARK;
        let mut vertex_updates: Vec<(usize, usize, f64, f64)> = vec![];
        let mut vertex_save = false;
        for (ai, a) in annos.iter().enumerate() {
            if !annos_group_on || !a.visible || a.pts.is_empty() {
                continue;
            }
            let pts: Vec<Pos2> = a.pts.iter().map(|&(lon, lat)| self.project(rect, lon, lat)).collect();
            let col = Color32::from_rgb(a.color[0], a.color[1], a.color[2]);
            let is_sel = matches!(sel, Some(Sel::Anno(i)) if *i == ai);
            // Fill (ear-clipped: user polygons may be concave).
            if a.kind == "polygon" && a.filled && pts.len() >= 3 {
                let alpha = (a.opacity.clamp(0.0, 1.0) * 0.42 * 255.0) as u8;
                let fill = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha);
                for (i, j, k) in triangulate(&pts) {
                    painter.add(egui::Shape::convex_polygon(vec![pts[i], pts[j], pts[k]], fill, Stroke::NONE));
                }
            }
            // Stroke (closed ring for polygons).
            if pts.len() >= 2 {
                let mut segs = pts.clone();
                if a.kind == "polygon" {
                    segs.push(pts[0]);
                }
                painter.add(egui::Shape::line(segs, Stroke::new(a.width.max(1.0), col)));
            }
            // Selected: vectorcraft-style anchor squares, draggable to edit
            // the vertices.
            if is_sel {
                for (vi, pt) in pts.iter().enumerate() {
                    let sq = Rect::from_center_size(*pt, Vec2::splat(7.0));
                    painter.rect_filled(sq, CornerRadius::ZERO, Color32::WHITE);
                    painter.rect_stroke(sq, CornerRadius::ZERO, Stroke::new(1.2, t.sunken), egui::StrokeKind::Inside);
                    if !a.locked {
                        let hresp = ui.interact(Rect::from_center_size(*pt, Vec2::splat(16.0)), egui::Id::new(("anno-h", a.id, vi)), Sense::drag());
                        if hresp.dragged() {
                            if let Some(p) = hresp.interact_pointer_pos() {
                                let (lon, lat) = self.unproject(rect, p);
                                vertex_updates.push((ai, vi, lon, lat));
                            }
                        }
                        if hresp.drag_stopped() {
                            vertex_save = true;
                        }
                    }
                }
            }
        }
        for (ai, vi, lon, lat) in vertex_updates {
            if let Some(pt) = annos.get_mut(ai).and_then(|a| a.pts.get_mut(vi)) {
                *pt = (lon, lat);
            }
        }
        if vertex_save {
            annotations::save(annos);
        }

        // Work-in-progress pen overlay.
        if self.tool != Tool::Select {
            let wpts: Vec<Pos2> = self.work.iter().map(|&(lon, lat)| self.project(rect, lon, lat)).collect();
            let col = t.good;
            if wpts.len() >= 2 {
                painter.add(egui::Shape::line(wpts.clone(), Stroke::new(2.0, col)));
            }
            if let Some(p) = ui.input(|i| i.pointer.hover_pos()).filter(|_| resp.hovered()) {
                if let Some(last) = wpts.last() {
                    painter.add(egui::Shape::dashed_line(&[*last, p], Stroke::new(1.5, col.gamma_multiply(0.85)), 4.0, 3.0));
                }
                if let (Some(first), true) = (wpts.first(), wpts.len() >= 3) {
                    if p.distance(*first) <= 10.0 {
                        painter.circle_stroke(*first, 7.0, Stroke::new(2.0, col));
                    }
                }
            }
            for pt in &wpts {
                let sq = Rect::from_center_size(*pt, Vec2::splat(6.0));
                painter.rect_filled(sq, CornerRadius::ZERO, Color32::WHITE);
                painter.rect_stroke(sq, CornerRadius::ZERO, Stroke::new(1.0, t.sunken), egui::StrokeKind::Inside);
            }
            if let Some(first) = wpts.first() {
                painter.rect_stroke(Rect::from_center_size(*first, Vec2::splat(10.0)), CornerRadius::ZERO, Stroke::new(1.3, col), egui::StrokeKind::Middle);
            }
        }

        // --- overlays --------------------------------------------------------
        let badge = |painter: &egui::Painter, pos: Pos2, text: String, col: Color32| {
            let size = painter.layout_no_wrap(text.clone(), FontId::new(10.0, theme::mono()), col).size();
            let r = Rect::from_min_size(pos + Vec2::new(8.0, 8.0), Vec2::new(size.x + 16.0, 22.0));
            painter.rect_filled(r, CornerRadius::ZERO, t.sunken.gamma_multiply(0.88));
            painter.rect_stroke(r, CornerRadius::ZERO, theme::hairline(), egui::StrokeKind::Inside);
            painter.text(r.center(), Align2::CENTER_CENTER, text, FontId::new(10.0, theme::mono()), col);
        };
        let region = app.region_name.lock().unwrap().clone();
        badge(&painter, rect.left_top(), region, t.ink);
        badge(&painter, rect.left_bottom() + Vec2::new(0.0, -30.0), format!("z{:.1}", self.zoom), t.accent_ink);
        painter.text(
            rect.right_bottom() + Vec2::new(-8.0, -6.0),
            Align2::RIGHT_BOTTOM,
            format!("tiles http://127.0.0.1:{port}/tile/{{z}}/{{x}}/{{y}}.png · © OpenStreetMap contributors"),
            FontId::new(10.5, theme::mono()),
            theme::Tokens::DARK.faint,
        );

        // Zoom controls: a square stack anchored above the attribution line.
        let stack = Rect::from_min_size(
            Pos2::new(rect.right() - 12.0 - CONTROL_H, rect.bottom() - 36.0 - 3.0 * CONTROL_H - 12.0),
            Vec2::new(CONTROL_H, 3.0 * CONTROL_H + 12.0),
        );
        let mut zui = ui.new_child(egui::UiBuilder::new().max_rect(stack));
        zui.vertical(|ui| {
            if widgets::glyph_button(ui, widgets::Glyph::Plus).clicked() {
                self.zoom = (self.zoom + 1.0).clamp(4.0, 19.0);
            }
            if widgets::glyph_button(ui, widgets::Glyph::Minus).clicked() {
                self.zoom = (self.zoom - 1.0).clamp(4.0, 19.0);
            }
            if widgets::glyph_button(ui, widgets::Glyph::Home).clicked() {
                let b = app.store.read().unwrap().bbox;
                self.center = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
                self.zoom = 12.0;
            }
        });

        self.last_rect = Some(rect);
        if budget < 3 {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }
}

/* ---------------- geometry helpers (screen space) ---------------- */

/// Ray-cast point-in-polygon.
fn point_in_poly(pts: &[Pos2], p: Pos2) -> bool {
    let mut inside = false;
    let n = pts.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (pts[i], pts[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y + f32::EPSILON) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Distance from `p` to segment `a–b`.
fn dist_pt_seg(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let t = (ap.dot(ab) / (ab.length_sq() + 1e-6)).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

fn cross3(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

fn point_in_tri(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
    let (s1, s2, s3) = (cross3(a, b, p), cross3(b, c, p), cross3(c, a, p));
    (s1 >= 0.0 && s2 >= 0.0 && s3 >= 0.0) || (s1 <= 0.0 && s2 <= 0.0 && s3 <= 0.0)
}

/// Ear-clip triangulation of a simple polygon (screen space) — user polygons
/// may be concave, so a triangle fan won't do.
fn triangulate(pts: &[Pos2]) -> Vec<(usize, usize, usize)> {
    let n = pts.len();
    if n < 3 {
        return vec![];
    }
    let mut idx: Vec<usize> = (0..n).collect();
    let area2: f32 = (0..n).map(|i| {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        a.x * b.y - b.x * a.y
    }).sum();
    if area2 < 0.0 {
        idx.reverse();
    }
    let mut tris = vec![];
    let mut guard = 0usize;
    while idx.len() > 3 && guard < 10_000 {
        guard += 1;
        let mut clipped = false;
        'ear: for i in 0..idx.len() {
            let m = idx.len();
            let (a, b, c) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (pa, pb, pc) = (pts[a], pts[b], pts[c]);
            if cross3(pa, pb, pc) <= 0.0 {
                continue; // reflex or degenerate
            }
            if idx.iter().any(|&p| {
                let p = pts[p];
                p != pa && p != pb && p != pc && point_in_tri(p, pa, pb, pc)
            }) {
                continue;
            }
            tris.push((a, b, c));
            idx.remove(i);
            clipped = true;
            break 'ear;
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        tris.push((idx[0], idx[1], idx[2]));
    }
    tris
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

/// Panel geometry, straight from the reference: eye column, lock column,
/// colour bar, disclosure chevron, name, then the badges on the right.
const COLUMN: f32 = 25.0;
const INDENT: f32 = 14.0;
const ROW_H: f32 = 26.0;

struct PanelState {
    search: String,
    /// Open group ids (engine groups + the synthetic `annotations` group).
    open: std::collections::HashSet<String>,
    selected: Option<Sel>,
    /// Property editors for the selected engine layer.
    edit_zoom: [i32; 2],
    /// Inline rename in progress: (anno id, buffer).
    rename: Option<(u64, String)>,
}

impl Default for PanelState {
    fn default() -> Self {
        // The two most useful branches start expanded so the tree reads at a
        // glance; everything else is a click away.
        Self {
            search: String::new(),
            open: ["transport".to_string(), "water".to_string(), "annotations".to_string()].into_iter().collect(),
            selected: None,
            edit_zoom: [0, 0],
            rename: None,
        }
    }
}

/// One flattened row of the panel tree.
struct Row {
    key: String,
    depth: usize,
    opens: bool,
    open: bool,
    colour: Color32,
    /// Drawn dimmed: hidden itself or inside a hidden group.
    dim: bool,
    src: Src,
}

/// Where a tree row's state comes from.
#[derive(Clone, Copy)]
enum Src {
    Engine(LayerDef),
    /// The synthetic `Annotations` group.
    AnnoGroup,
    Anno(usize),
}

/// Whether `d`'s subtree matches the search (the row stays for a match of its
/// own or of anything inside it).
fn subtree_matches(defs: &[LayerDef], d: &LayerDef, query: &str) -> bool {
    d.name.to_lowercase().contains(query) || d.id.contains(query) || children_of(defs, d.id).iter().any(|c| subtree_matches(defs, c, query))
}

/// A group's bar colour: its own paint, else its first painted child's.
fn group_colour(defs: &[LayerDef], d: &LayerDef, vis: &LayerVis, app: &Arc<App>) -> Color32 {
    let paint = |dd: &LayerDef| layer_color(app, vis, dd);
    if d.color_var.is_some() || d.id == "background" {
        return paint(d);
    }
    children_of(defs, d.id).iter().find(|c| c.color_var.is_some()).map(paint).unwrap_or_else(|| Color32::from_rgb(d.fallback[0], d.fallback[1], d.fallback[2]))
}

impl PanelState {
    fn panel_ui(&mut self, ui: &mut egui::Ui, app: &Arc<App>, data_version: &Arc<AtomicU64>, annos: &mut Vec<Anno>, map: &mut MapState, workspace: &Path) {
        let t = theme::Tokens::DARK;
        let defs = layer_catalog();
        let vis_snapshot = app.layers.read().unwrap().clone();
        let query = self.search.trim().to_lowercase();
        let narrowing = !query.is_empty();

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            widgets::hud(ui, "layers", t.faint);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::button(ui, "Hide all", widgets::Kind::Secondary, 0.0).clicked() {
                    let mut vis = app.layers.write().unwrap();
                    for d in &defs {
                        vis.visible.insert(d.id.to_string(), false);
                    }
                    vis.visible.insert("annotations".to_string(), false);
                    for a in annos.iter_mut() {
                        a.visible = false;
                    }
                    vis.touch();
                    drop(vis);
                    app.invalidate();
                    annotations::save(annos);
                    data_version.fetch_add(1, Ordering::Relaxed);
                }
                if widgets::button(ui, "Show all", widgets::Kind::Secondary, 0.0).clicked() {
                    let mut vis = app.layers.write().unwrap();
                    vis.visible.clear();
                    for a in annos.iter_mut() {
                        a.visible = true;
                    }
                    vis.touch();
                    drop(vis);
                    app.invalidate();
                    annotations::save(annos);
                    data_version.fetch_add(1, Ordering::Relaxed);
                }
            });
        });
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search layers…")
                .desired_width(f32::INFINITY)
                .vertical_align(Align::Center)
                .margin(Vec2::new(10.0, 0.0))
                .min_size(Vec2::new(0.0, CONTROL_H)),
        );
        ui.add_space(4.0);

        // Flatten the tree: engine groups (open ones expand, depth-indented),
        // then the synthetic Annotations group with the drawings inside it.
        let mut rows: Vec<Row> = Vec::new();
        fn push(d: &LayerDef, depth: usize, shown_parent: bool, defs: &[LayerDef], vis: &LayerVis, app: &Arc<App>, open: &std::collections::HashSet<String>, query: &str, rows: &mut Vec<Row>) {
            if !query.is_empty() && !subtree_matches(defs, d, query) {
                return;
            }
            let shown = shown_parent && vis.is_visible(d.id);
            let kids = children_of(defs, d.id);
            let opens = !kids.is_empty();
            let is_open = opens && (open.contains(d.id) || !query.is_empty());
            let colour = group_colour(defs, d, vis, app);
            rows.push(Row { key: d.id.to_string(), depth, opens, open: is_open, colour, dim: !shown, src: Src::Engine(*d) });
            if is_open {
                for c in &kids {
                    push(c, depth + 1, shown, defs, vis, app, open, query, rows);
                }
            }
        }
        for d in defs.iter().filter(|d| d.parent.is_empty()) {
            push(d, 0, true, &defs, &vis_snapshot, app, &self.open, &query, &mut rows);
        }
        // The Annotations group (drawings live in it, vectorcraft-style).
        let annos_hit = narrowing && annos.iter().any(|a| a.name.to_lowercase().contains(&query));
        if !narrowing || annos_hit {
            let group_on = vis_snapshot.is_visible("annotations");
            let group_open = self.open.contains("annotations") || narrowing;
            rows.push(Row { key: "annotations".into(), depth: 0, opens: !annos.is_empty(), open: group_open, colour: t.good, dim: !group_on, src: Src::AnnoGroup });
            if group_open {
                for (i, a) in annos.iter().enumerate() {
                    if narrowing && !a.name.to_lowercase().contains(&query) {
                        continue;
                    }
                    let colour = Color32::from_rgb(a.color[0], a.color[1], a.color[2]);
                    rows.push(Row { key: format!("anno-{}", a.id), depth: 1, opens: false, open: false, colour, dim: !group_on || !a.visible, src: Src::Anno(i) });
                }
            }
        }

        // The properties editor takes the bottom half; the tree scrolls on top.
        let total_h = ui.available_height() - 36.0; // bottom bar
        let list_h = if self.selected.is_some() { (total_h * 0.52).max(160.0) } else { total_h };

        egui::ScrollArea::vertical().id_salt("layer-rows").max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in &rows {
                self.row_ui(ui, row, &vis_snapshot, app, data_version, annos);
            }
        });

        // Bottom bar: counts + the drawing tools (vectorcraft's footer).
        self.bottom_bar(ui, app, data_version, annos, map);

        // Properties for the selected row.
        match self.selected.clone() {
            Some(Sel::Layer(id)) => {
                if let Some(def) = defs.iter().find(|d| d.id == id) {
                    ui.add_space(6.0);
                    self.properties_ui(ui, def, app, data_version, &vis_snapshot);
                }
            }
            Some(Sel::Anno(i)) => {
                ui.add_space(6.0);
                self.anno_properties_ui(ui, app, data_version, annos, map, workspace, i);
            }
            None => {}
        }
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui, app: &Arc<App>, data_version: &Arc<AtomicU64>, annos: &mut Vec<Anno>, map: &mut MapState) {
        let t = theme::Tokens::DARK;
        let (bar, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
        ui.painter().hline(bar.left()..=bar.right(), bar.top(), theme::hairline());
        let layers_n = layer_catalog().len();
        ui.painter().text(
            bar.left_center() + Vec2::new(4.0, 0.0),
            Align2::LEFT_CENTER,
            format!("{layers_n} layers · {} {}", annos.len(), if annos.len() == 1 { "drawing" } else { "drawings" }),
            FontId::new(10.0, theme::mono()),
            t.faint,
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(Layout::right_to_left(Align::Center)));
        child.add_space(6.0);
        // Delete the selected drawing.
        let sel_anno = matches!(self.selected, Some(Sel::Anno(_))).then(|| match self.selected {
            Some(Sel::Anno(i)) if annos.get(i).is_some_and(|a| !a.locked) => Some(i),
            _ => None,
        }).flatten();
        if mini_icon(&mut child, "trash", paint_trash).clicked() {
            if let Some(i) = sel_anno {
                annos.remove(i);
                self.selected = None;
                annotations::save(annos);
                app.invalidate();
                data_version.fetch_add(1, Ordering::Relaxed);
            }
        }
        // Frame the selection.
        if mini_icon(&mut child, "fit", paint_fit).clicked() {
            match self.selected {
                Some(Sel::Anno(i)) => {
                    if let (Some(r), Some(a)) = (map.last_rect, annos.get(i)) {
                        map.fit_annotation(r, a);
                    }
                }
                _ => {
                    if let Some(r) = map.last_rect {
                        let b = app.store.read().unwrap().bbox;
                        map.fit_bbox(r, b);
                    }
                }
            }
        }
        // New path / new polygon: switch to the pen tools.
        if mini_icon(&mut child, "path", paint_path_icon).clicked() {
            map.tool = Tool::Path;
            map.work.clear();
        }
        if mini_icon(&mut child, "poly", paint_poly_icon).clicked() {
            map.tool = Tool::Polygon;
            map.work.clear();
        }
    }

    fn row_ui(&mut self, ui: &mut egui::Ui, row: &Row, vis: &LayerVis, app: &Arc<App>, data_version: &Arc<AtomicU64>, annos: &mut Vec<Anno>) {
        let t = theme::Tokens::DARK;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
        let p = ui.painter().clone();

        let sel = match (&self.selected, &row.src) {
            (Some(Sel::Layer(s)), Src::Engine(d)) => s == d.id,
            (Some(Sel::Anno(i)), Src::Anno(j)) => i == j,
            _ => false,
        };
        if sel {
            p.rect_filled(rect, CornerRadius::ZERO, t.sunken);
            p.rect_filled(Rect::from_min_size(rect.min, Vec2::new(2.0, ROW_H)), CornerRadius::ZERO, t.accent_ink);
        } else if resp.hovered() {
            p.rect_filled(rect, CornerRadius::ZERO, t.sunken.gamma_multiply(0.6));
        }
        p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.line));

        // Per-row state.
        let (visible, locked) = match row.src {
            Src::Engine(d) => (vis.is_visible(d.id), vis.is_locked(d.id)),
            Src::Anno(i) => annos.get(i).map_or((false, true), |a| (a.visible, a.locked)),
            Src::AnnoGroup => (vis.is_visible("annotations"), !annos.is_empty() && annos.iter().all(|a| a.locked)),
        };

        // Eye column.
        let eye = Rect::from_min_size(rect.min, Vec2::new(COLUMN, ROW_H));
        let lock_r = Rect::from_min_size(rect.min + Vec2::new(COLUMN, 0.0), Vec2::new(COLUMN, ROW_H));
        p.line_segment([eye.right_top(), eye.right_bottom()], Stroke::new(1.0, t.line));
        p.line_segment([lock_r.right_top(), lock_r.right_bottom()], Stroke::new(1.0, t.line));
        let eye_col = if visible { t.good } else { t.muted };
        let ec = eye.center();
        p.circle_stroke(ec, 5.0, Stroke::new(1.4, eye_col));
        if visible {
            p.circle_filled(ec, 2.4, eye_col);
        }
        let eresp = ui.interact(eye, ui.id().with(("eye", &row.key)), Sense::click());

        // Lock column (painted padlock; ghost on hover).
        let lc = lock_r.center();
        if locked {
            paint_padlock(&p, lc, t.warn);
        } else {
            let lr_hover = ui.interact(lock_r, ui.id().with(("lock", &row.key)), Sense::click()).hovered();
            if lr_hover {
                paint_padlock(&p, lc, t.faint);
            }
        }
        let lresp = ui.interact(lock_r, ui.id().with(("lock", &row.key)), Sense::click());

        // Colour bar (groups wide, leaves thin) — vectorcraft's marker.
        let mut x = rect.left() + 2.0 * COLUMN + 2.0;
        let bar_w = if row.opens { 4.0 } else { 2.0 };
        p.rect_filled(Rect::from_min_size(Pos2::new(x - 1.0, rect.top() + 3.0), Vec2::new(bar_w, ROW_H - 6.0)), CornerRadius::ZERO, row.colour);
        x += 8.0 + row.depth as f32 * INDENT;

        // Disclosure chevron.
        if row.opens {
            let dr = Rect::from_min_size(Pos2::new(x, rect.center().y - 8.0), Vec2::new(14.0, 16.0));
            let c = dr.center();
            let tri: Vec<Pos2> = if row.open {
                vec![Pos2::new(c.x - 4.0, c.y - 2.0), Pos2::new(c.x + 4.0, c.y - 2.0), Pos2::new(c.x, c.y + 3.0)]
            } else {
                vec![Pos2::new(c.x - 2.0, c.y - 4.0), Pos2::new(c.x - 2.0, c.y + 4.0), Pos2::new(c.x + 3.0, c.y)]
            };
            p.add(egui::Shape::convex_polygon(tri, t.muted, Stroke::NONE));
            let dresp = ui.interact(dr, ui.id().with(("disc", &row.key)), Sense::click());
            if dresp.clicked() {
                if row.open {
                    self.open.remove(&row.key);
                } else {
                    self.open.insert(row.key.clone());
                }
            }
        }
        x += 16.0;

        let ink = if row.dim { t.muted } else { t.ink };

        // Name (inline editor while renaming a drawing).
        let name = match row.src {
            Src::Engine(d) => d.name.to_string(),
            Src::Anno(i) => annos.get(i).map(|a| a.name.clone()).unwrap_or_default(),
            Src::AnnoGroup => "Annotations".to_string(),
        };
        let name_rect = Rect::from_min_max(Pos2::new(x - 2.0, rect.center().y - 10.0), Pos2::new(rect.right() - 74.0, rect.center().y + 10.0));
        let mut renamed = false;
        if let Src::Anno(i) = row.src {
            let id = annos.get(i).map(|a| a.id).unwrap_or(0);
            if self.rename.as_ref().is_some_and(|(rid, _)| *rid == id) {
                let mut buf = self.rename.take().unwrap().1;
                let te = ui.new_child(egui::UiBuilder::new().max_rect(name_rect)).add(
                    egui::TextEdit::singleline(&mut buf).desired_width(name_rect.width()).font(FontId::new(12.0, FontFamily::Proportional)),
                );
                if !te.has_focus() && !te.lost_focus() {
                    te.request_focus();
                }
                if te.lost_focus() {
                    renamed = true;
                    if !ui.input(|i| i.key_pressed(egui::Key::Escape)) && buf != name {
                        if let Some(a) = annos.get_mut(i) {
                            a.name = buf.clone();
                        }
                    }
                } else {
                    self.rename = Some((id, buf));
                }
            }
        }
        if !renamed {
            let galley = p.layout(name.clone(), FontId::new(12.5, FontFamily::Proportional), ink, name_rect.width());
            let clip = p.with_clip_rect(name_rect);
            clip.galley(Pos2::new(x - 2.0, rect.center().y - galley.size().y / 2.0), galley.clone(), ink);
        }

        // Kind chip + right badge.
        match row.src {
            Src::Engine(d) if d.kind != "group" => {
                let kind_col = match d.kind {
                    "engine" => t.warn,
                    "label" => t.accent_ink,
                    _ => t.faint,
                };
                let name_w = p.layout_no_wrap(name.clone(), FontId::new(12.5, FontFamily::Proportional), ink).size().x;
                p.text(Pos2::new(x + name_w + 8.0, rect.center().y), Align2::LEFT_CENTER, d.kind.to_uppercase(), FontId::new(8.5, theme::mono()), kind_col);
                let (zmin, zmax) = vis.zoom.get(d.id).copied().unwrap_or((d.min_zoom, d.max_zoom));
                let badge = if zmax >= 19 { format!("z{zmin}+") } else { format!("z{zmin}–{zmax}") };
                p.text(Pos2::new(rect.right() - 6.0, rect.center().y), Align2::RIGHT_CENTER, badge, FontId::new(9.5, theme::mono()), t.faint);
            }
            Src::Anno(i) => {
                if let Some(a) = annos.get(i) {
                    let name_w = p.layout_no_wrap(name.clone(), FontId::new(12.5, FontFamily::Proportional), ink).size().x;
                    p.text(Pos2::new(x + name_w + 8.0, rect.center().y), Align2::LEFT_CENTER, a.kind.to_uppercase(), FontId::new(8.5, theme::mono()), t.good);
                    p.text(Pos2::new(rect.right() - 6.0, rect.center().y), Align2::RIGHT_CENTER, format!("{} pts", a.pts.len()), FontId::new(9.5, theme::mono()), t.faint);
                }
            }
            _ => {}
        }

        // --- interactions --------------------------------------------------
        let alt = ui.input(|i| i.modifiers.alt);
        let on_col = |r: &egui::Response| r.interact_pointer_pos().is_some_and(|pp| pp.x < rect.left() + 2.0 * COLUMN);

        if eresp.clicked() {
            let defs = layer_catalog();
            if alt {
                // Isolate: hide every other top-level group (or show all when
                // everything else is already hidden) — vectorcraft's
                // Hide/Show Others, for easy drawing on one layer.
                let top = match row.src {
                    Src::Engine(d) => top_ancestor_of(&defs, d.id),
                    _ => "annotations".to_string(),
                };
                let mut v = app.layers.write().unwrap();
                isolate_top(&mut v, annos, &top);
                drop(v);
                app.invalidate();
                annotations::save(annos);
                data_version.fetch_add(1, Ordering::Relaxed);
            } else {
                match row.src {
                    Src::Engine(d) => {
                        let nv = !visible;
                        let mut v = app.layers.write().unwrap();
                        v.visible.insert(d.id.to_string(), nv);
                        for id in descendants_of(&defs, d.id) {
                            v.visible.insert(id.to_string(), nv);
                        }
                        v.touch();
                        drop(v);
                        app.invalidate();
                        data_version.fetch_add(1, Ordering::Relaxed);
                    }
                    Src::AnnoGroup => {
                        let nv = !visible;
                        app.layers.write().unwrap().visible.insert("annotations".to_string(), nv);
                        app.invalidate();
                        data_version.fetch_add(1, Ordering::Relaxed);
                    }
                    Src::Anno(i) => {
                        if let Some(a) = annos.get_mut(i) {
                            a.visible = !a.visible;
                        }
                        annotations::save(annos);
                    }
                }
            }
        }
        if lresp.clicked() {
            let defs = layer_catalog();
            match row.src {
                Src::Engine(d) => {
                    let nv = !locked;
                    let mut v = app.layers.write().unwrap();
                    v.locked.insert(d.id.to_string(), nv);
                    for id in descendants_of(&defs, d.id) {
                        v.locked.insert(id.to_string(), nv);
                    }
                    v.touch();
                }
                Src::AnnoGroup => {
                    let nv = !locked;
                    for a in annos.iter_mut() {
                        a.locked = nv;
                    }
                    annotations::save(annos);
                }
                Src::Anno(i) => {
                    if let Some(a) = annos.get_mut(i) {
                        a.locked = !a.locked;
                    }
                    annotations::save(annos);
                }
            }
        }

        if resp.clicked() && !on_col(&resp) && !eresp.hovered() && !lresp.hovered() {
            match row.src {
                Src::Engine(d) => {
                    self.selected = Some(Sel::Layer(d.id.to_string()));
                    let (mn, mx) = vis.zoom.get(d.id).copied().unwrap_or((d.min_zoom, d.max_zoom));
                    self.edit_zoom = [mn as i32, mx as i32];
                }
                Src::Anno(i) => self.selected = Some(Sel::Anno(i)),
                Src::AnnoGroup => self.selected = Some(Sel::Layer("annotations".into())),
            }
        }
        // Double-click a drawing row to rename it inline.
        if resp.double_clicked() && !on_col(&resp) {
            if let Src::Anno(i) = row.src {
                if let Some(a) = annos.get(i) {
                    self.rename = Some((a.id, a.name.clone()));
                }
            }
        }
    }

    fn properties_ui(&mut self, ui: &mut egui::Ui, def: &LayerDef, app: &Arc<App>, data_version: &Arc<AtomicU64>, vis: &LayerVis) {
        let t = theme::Tokens::DARK;
        let locked = vis.is_locked(def.id);
        ui.horizontal(|ui| {
            widgets::hud(ui, "engine layer properties", t.faint);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if locked {
                    widgets::badge(ui, "locked", t.warn);
                }
                if widgets::button(ui, "Reset", widgets::Kind::Secondary, 0.0).clicked() && !locked {
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
            widgets::hud(ui, "vector path", t.faint);
            egui::Frame::new().fill(t.sunken).stroke(theme::hairline()).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(def.vector_path).size(11.0).monospace().color(t.ink));
            });

            if locked {
                ui.add_space(6.0);
                widgets::hud(ui, "locked — unlock the row in the tree to edit", t.warn);
                return;
            }

            // Paint: fill colour + swatch.
            ui.add_space(4.0);
            widgets::hud(ui, match def.kind { "fill" => "fill", "stroke" => "stroke", "label" => "label paint", "engine" => "engine paint", _ => "paint" }, t.faint);
            ui.horizontal(|ui| {
                let col = layer_color(app, vis, def);
                let (sw, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
                ui.painter().rect_filled(sw, CornerRadius::ZERO, col);
                ui.painter().rect_stroke(sw, CornerRadius::ZERO, theme::hairline(), egui::StrokeKind::Outside);
                let mut hex = format!("#{:02X}{:02X}{:02X}", col.r(), col.g(), col.b());
                let resp = ui.add(egui::TextEdit::singleline(&mut hex).desired_width(86.0).font(FontId::new(12.0, FontFamily::Monospace)));
                if resp.changed() {
                    if let Some(c) = parse_hex(&hex) {
                        let mut v = app.layers.write().unwrap();
                        v.color.insert(def.id.to_string(), c);
                        v.touch();
                        app.invalidate();
                        data_version.fetch_add(1, Ordering::Relaxed);
                    }
                }
                if vis.color_override(def.id).is_some() && widgets::button(ui, "Palette", widgets::Kind::Secondary, 0.0).clicked() {
                    let mut v = app.layers.write().unwrap();
                    v.color.remove(def.id);
                    v.touch();
                    app.invalidate();
                    data_version.fetch_add(1, Ordering::Relaxed);
                }
            });

            // Opacity.
            ui.add_space(4.0);
            widgets::hud(ui, "opacity", t.faint);
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
            widgets::hud(ui, "zoom range", t.faint);
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
                    ui.label(RichText::new(format!("(default {}–{})", def.min_zoom, def.max_zoom)).size(9.5).color(t.faint));
                }
            });

            // Engine flags (roads only).
            if def.parent == "roads" || def.id == "roads" {
                ui.add_space(4.0);
                widgets::hud(ui, "engine", t.faint);
                ui.label(RichText::new("Casing under fill · oneway arrows from z16 · name shields from z13 · lane widths from maxspeed/lanes tags").size(10.0).color(t.faint));
            }
        });
    }

    fn anno_properties_ui(&mut self, ui: &mut egui::Ui, app: &Arc<App>, data_version: &Arc<AtomicU64>, annos: &mut Vec<Anno>, map: &mut MapState, workspace: &Path, i: usize) {
        let t = theme::Tokens::DARK;
        let Some(a) = annos.get(i) else { return };
        let (id, name, kind, color, opacity, width, filled, locked) =
            (a.id, a.name.clone(), a.kind.clone(), a.color, a.opacity, a.width, a.filled, a.locked);
        let (n_pts, len_m, area_m2) = (a.pts.len(), a.length_m(), a.area_m2());
        let bbox = {
            let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            for &(lon, lat) in &annos[i].pts {
                b = [b[0].min(lon), b[1].min(lat), b[2].max(lon), b[3].max(lat)];
            }
            b
        };
        ui.horizontal(|ui| {
            widgets::hud(ui, "drawing properties", t.faint);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if locked {
                    widgets::badge(ui, "locked", t.warn);
                }
                if widgets::button(ui, "Delete", widgets::Kind::Secondary, 0.0).clicked() && !locked {
                    annos.remove(i);
                    self.selected = None;
                    annotations::save(annos);
                    app.invalidate();
                    data_version.fetch_add(1, Ordering::Relaxed);
                }
            });
        });
        egui::ScrollArea::vertical().id_salt("anno-props").auto_shrink([false, false]).show(ui, |ui| {
            // Name.
            widgets::hud(ui, "name", t.faint);
            let mut buf = name.clone();
            if ui.add(egui::TextEdit::singleline(&mut buf).desired_width(f32::INFINITY).font(FontId::new(12.5, FontFamily::Proportional))).lost_focus()
                && buf != name
                && !buf.trim().is_empty()
            {
                if let Some(a) = annos.iter_mut().find(|a| a.id == id) {
                    a.name = buf.trim().to_string();
                }
                annotations::save(annos);
            }

            // Paint.
            ui.add_space(4.0);
            widgets::hud(ui, "paint", t.faint);
            ui.horizontal(|ui| {
                let (sw, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
                let col = Color32::from_rgb(color[0], color[1], color[2]);
                ui.painter().rect_filled(sw, CornerRadius::ZERO, col);
                ui.painter().rect_stroke(sw, CornerRadius::ZERO, theme::hairline(), egui::StrokeKind::Outside);
                let mut hex = format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]);
                let resp = ui.add(egui::TextEdit::singleline(&mut hex).desired_width(86.0).font(FontId::new(12.0, FontFamily::Monospace)));
                if resp.changed() {
                    if let Some(c) = parse_hex(&hex) {
                        if let Some(a) = annos.iter_mut().find(|a| a.id == id) {
                            a.color = c;
                        }
                        annotations::save(annos);
                    }
                }
            });

            // Opacity + stroke width.
            ui.add_space(4.0);
            widgets::hud(ui, "opacity", t.faint);
            let mut op = opacity;
            if ui.add(egui::Slider::new(&mut op, 0.0..=1.0).show_value(true)).changed() {
                if let Some(a) = annos.iter_mut().find(|a| a.id == id) {
                    a.opacity = op;
                }
                annotations::save(annos);
            }
            ui.add_space(4.0);
            widgets::hud(ui, "stroke width", t.faint);
            let mut w = width;
            if ui.add(egui::Slider::new(&mut w, 1.0..=8.0).show_value(true)).changed() {
                if let Some(a) = annos.iter_mut().find(|a| a.id == id) {
                    a.width = w;
                }
                annotations::save(annos);
            }
            if kind == "polygon" {
                ui.add_space(4.0);
                let mut f = filled;
                if ui.checkbox(&mut f, "Fill the polygon").changed() {
                    if let Some(a) = annos.iter_mut().find(|a| a.id == id) {
                        a.filled = f;
                    }
                    annotations::save(annos);
                }
            }

            // Stats.
            ui.add_space(4.0);
            widgets::hud(ui, "geometry", t.faint);
            egui::Frame::new().fill(t.sunken).stroke(theme::hairline()).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("{n_pts} vertices · ≈{} long", human_m(len_m))).size(11.0).monospace().color(t.ink));
                if area_m2 > 0.0 {
                    ui.label(RichText::new(format!("area ≈{}", human_m2(area_m2))).size(11.0).monospace().color(t.ink));
                }
            });

            // Actions.
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if widgets::button(ui, "Fit view", widgets::Kind::Secondary, 0.0).clicked() {
                    if let Some(r) = map.last_rect {
                        map.fit_bbox(r, bbox);
                    }
                }
                if widgets::button(ui, "Export GeoJSON", widgets::Kind::Secondary, 0.0).clicked() {
                    match annotations::export_geojson(annos, workspace) {
                        Some(p) => {
                            // Toast comes from the shell; a quiet print here.
                            println!("annotations exported: {}", p.display());
                        }
                        None => println!("annotations export failed"),
                    }
                }
            });
            if locked {
                ui.add_space(4.0);
                widgets::hud(ui, "locked — unlock the row in the tree to edit", t.warn);
            }
        });
    }
}

/// Hide every top-level group except `top` (or show all when everything else
/// is already hidden) — vectorcraft's Alt-click Hide/Show Others.
fn isolate_top(vis: &mut LayerVis, annos: &mut [Anno], top: &str) {
    let defs = layer_catalog();
    let tops: Vec<String> = defs.iter().filter(|d| d.parent.is_empty()).map(|d| d.id.to_string()).chain(std::iter::once("annotations".to_string())).collect();
    let others_visible = tops.iter().filter(|t| t.as_str() != top).any(|t| {
        if t == "annotations" {
            vis.is_visible("annotations") && annos.iter().any(|a| a.visible)
        } else {
            vis.is_visible(t)
        }
    });
    if others_visible {
        for t in tops.iter().filter(|t| t.as_str() != top) {
            if t == "annotations" {
                vis.visible.insert("annotations".to_string(), false);
                for a in annos.iter_mut() {
                    a.visible = false;
                }
            } else {
                vis.visible.insert(t.clone(), false);
                for id in descendants_of(&defs, t) {
                    vis.visible.insert(id.to_string(), false);
                }
            }
        }
    } else {
        vis.visible.clear();
        for a in annos.iter_mut() {
            a.visible = true;
        }
    }
    vis.touch();
}

fn human_m(m: f64) -> String {
    if m >= 1000.0 {
        format!("{:.1} km", m / 1000.0)
    } else {
        format!("{m:.0} m")
    }
}

fn human_m2(m2: f64) -> String {
    if m2 >= 1_000_000.0 {
        format!("{:.2} km²", m2 / 1_000_000.0)
    } else {
        format!("{m2:.0} m²")
    }
}

/// A 24px square icon button with a painter-drawn glyph (panel footer).
fn mini_icon(ui: &mut egui::Ui, id: &str, paint: fn(&egui::Painter, Rect, Color32)) -> egui::Response {
    let t = theme::Tokens::DARK;
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    let (fill, stroke_col, ink) = if resp.hovered() { (t.sunken, t.line_strong, t.ink) } else { (t.bg, t.line, t.muted) };
    ui.painter().rect(r, CornerRadius::ZERO, fill, Stroke::new(1.0, stroke_col), egui::StrokeKind::Inside);
    paint(ui.painter(), r, ink);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(match id {
        "poly" => "Draw a new polygon",
        "path" => "Draw a new path",
        "fit" => "Frame the selection",
        _ => "Delete the selected drawing",
    })
}

fn paint_poly_icon(p: &egui::Painter, r: Rect, ink: Color32) {
    let c = r.center();
    let pts = [Pos2::new(c.x, c.y - 5.5), Pos2::new(c.x + 5.5, c.y + 4.0), Pos2::new(c.x - 5.5, c.y + 4.0)];
    p.add(egui::Shape::convex_polygon(pts.to_vec(), Color32::TRANSPARENT, Stroke::new(1.4, ink)));
}

fn paint_path_icon(p: &egui::Painter, r: Rect, ink: Color32) {
    let c = r.center();
    p.add(egui::Shape::line(
        vec![Pos2::new(c.x - 5.5, c.y + 4.5), Pos2::new(c.x - 1.0, c.y - 4.5), Pos2::new(c.x + 2.5, c.y + 1.5), Pos2::new(c.x + 5.5, c.y - 3.5)],
        Stroke::new(1.4, ink),
    ));
}

fn paint_fit(p: &egui::Painter, r: Rect, ink: Color32) {
    let c = r.center();
    p.circle_stroke(c, 4.5, Stroke::new(1.3, ink));
    p.circle_filled(c, 1.2, ink);
    for d in [Vec2::new(0.0, -8.0), Vec2::new(0.0, 8.0), Vec2::new(-8.0, 0.0), Vec2::new(8.0, 0.0)] {
        p.line_segment([c + d * 0.55, c + d], Stroke::new(1.3, ink));
    }
}

fn paint_trash(p: &egui::Painter, r: Rect, ink: Color32) {
    let c = r.center();
    let s = Stroke::new(1.3, ink);
    p.hline(c.x - 4.5..=c.x + 4.5, c.y - 3.5, s);
    p.add(egui::Shape::line(
        vec![Pos2::new(c.x - 2.0, c.y - 3.5), Pos2::new(c.x - 2.0, c.y - 5.5), Pos2::new(c.x + 2.0, c.y - 5.5), Pos2::new(c.x + 2.0, c.y - 3.5)],
        s,
    ));
    p.rect_stroke(Rect::from_min_size(Pos2::new(c.x - 3.5, c.y - 3.5), Vec2::new(7.0, 9.5)), CornerRadius::ZERO, s, egui::StrokeKind::Inside);
    p.vline(c.x - 1.0, c.y - 1.0..=c.y + 4.0, s);
    p.vline(c.x + 1.0, c.y - 1.0..=c.y + 4.0, s);
}

fn paint_padlock(p: &egui::Painter, c: Pos2, ink: Color32) {
    let s = Stroke::new(1.2, ink);
    p.rect_stroke(Rect::from_min_size(Pos2::new(c.x - 3.5, c.y - 1.0), Vec2::new(7.0, 6.0)), CornerRadius::ZERO, s, egui::StrokeKind::Inside);
    p.circle_stroke(c + Vec2::new(0.0, -1.5), 2.6, Stroke::new(1.2, ink));
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
/* Downloads state (the manager wiring)                                    */
/* --------------------------------------------------------------------- */

struct DownloadsState {
    manager: Arc<Manager>,
    jobs: HashMap<String, Phase>,
    last_jobs: HashMap<String, Phase>,
    custom_url: String,
    custom_bbox_world: [f64; 4],
    path_edit: String,
    search: String,
    data_version: Arc<AtomicU64>,
}

impl DownloadsState {
    fn new() -> Self {
        Self { manager: Arc::new(Manager::new()), jobs: HashMap::new(), last_jobs: HashMap::new(), custom_url: String::new(), custom_bbox_world: [-180.0, -85.0, 180.0, 85.0], path_edit: String::new(), search: String::new(), data_version: Arc::new(AtomicU64::new(1)) }
    }

    /// Poll the manager, swap in a freshly extracted store, keep the UI alive.
    fn poll(&mut self, ctx: &egui::Context, app: &Arc<App>, data_version: &Arc<AtomicU64>, workspace: &Path) {
        self.jobs = self.manager.all();
        let mut any_active = false;
        for (id, phase) in &self.jobs {
            if phase.is_active() {
                any_active = true;
            }
            let was = self.last_jobs.get(id);
            if matches!(phase, Phase::Installed { .. }) && !was.is_some_and(|p| matches!(p, Phase::Installed { .. })) {
                // Freshly installed: swap the store in and tell the user.
                let dir = workspace.join("regions").join(id);
                match carto_core::Store::load(&dir) {
                    Ok(store) => {
                        let label = region_label(id);
                        app.set_store(store, label, dir);
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

    fn start(&mut self, _app: &Arc<App>, id: &str, name: &str, bbox: [f64; 4], url: &str, workspace: &Path) {
        self.manager.start(
            RegionReq { id: id.to_string(), name: name.to_string(), bbox, url: url.to_string() },
            workspace.join("downloads"),
            workspace.join("regions"),
        );
    }
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
