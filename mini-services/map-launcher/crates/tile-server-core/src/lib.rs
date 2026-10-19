//! map-launcher tile-server engine: the CPU renderer, HTTP tile/API server,
//! layer engine overrides, data-dir resolution and the headless entry points
//! shared by the `tile-server` (GUI) and `tile-server-cli` binaries.

pub mod download;
pub mod layers;

use carto_core::{road_class_name, Line, Store};
use carto_math::{lonlat_to_merc, tile_bbox, ClipRect};
use carto_style::{Palette, SymbolRegistry};
use fontdue::{Font, FontSettings};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

use layers::LayerVis;

pub use download::Manager as DownloadManager;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/* --------------------------------------------------------------------- */
/* Application state                                                       */
/* --------------------------------------------------------------------- */

type TileKey = (u8, u32, u32);

pub struct TileCache {
    map: HashMap<TileKey, Arc<Vec<u8>>>,
    order: VecDeque<TileKey>,
    cap: usize,
}

impl TileCache {
    fn new(cap: usize) -> Self {
        Self { map: HashMap::new(), order: VecDeque::new(), cap }
    }
    fn get(&mut self, k: &TileKey) -> Option<Arc<Vec<u8>>> {
        self.map.get(k).cloned()
    }
    fn put(&mut self, k: TileKey, v: Arc<Vec<u8>>) {
        if self.map.contains_key(&k) {
            return;
        }
        if self.map.len() >= self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
        self.map.insert(k, v);
        self.order.push_back(k);
    }
    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

pub struct App {
    /// The active region's store; swapped at runtime by the Downloads tab.
    pub store: RwLock<Store>,
    /// Layer-engine overrides edited by the Layers panel.
    pub layers: RwLock<LayerVis>,
    pub palette: Palette,
    pub font: Font,
    pub font_bold: Font,
    pub symbol_count: usize,
    pub cache: Mutex<TileCache>,
    pub hits: AtomicUsize,
    pub misses: AtomicUsize,
    /// Human label of the active region ("Central Tanzania (Dodoma)" etc).
    pub region_name: Mutex<String>,
}

impl App {
    /// Drop every cached tile (after a layer edit or a region switch).
    pub fn invalidate(&self) {
        self.cache.lock().unwrap().clear();
    }

    pub fn stats(&self) -> StatsDto {
        let store = self.store.read().unwrap();
        let named = store.roads.iter().filter(|r| !r.name.is_empty()).count();
        StatsDto {
            roads: store.roads.len(),
            named_roads: named,
            railways: store.railways.len(),
            waterways: store.waterways.len(),
            water_areas: store.water_areas.len(),
            landuse: store.landuse.len(),
            buildings: store.buildings.len(),
            pois: store.pois.len(),
            bounds: store.bbox,
        }
    }
}

/* --------------------------------------------------------------------- */
/* JSON DTOs (frontend contract)                                           */
/* --------------------------------------------------------------------- */

#[derive(Serialize)]
pub struct StatsDto {
    pub roads: usize,
    pub named_roads: usize,
    pub railways: usize,
    pub waterways: usize,
    pub water_areas: usize,
    pub landuse: usize,
    pub buildings: usize,
    pub pois: usize,
    pub bounds: [f64; 4],
}

#[derive(Serialize)]
struct RoadSummaryDto {
    id: i64,
    name: String,
    class: String,
    length_m: f32,
    bbox: [f32; 4],
    named: bool,
    oneway: u8,
    lanes: u8,
    maxspeed: u16,
}

#[derive(Serialize)]
struct RoadsRespDto {
    count: usize,
    roads: Vec<RoadSummaryDto>,
}

#[derive(Serialize)]
struct RoadDetailDto {
    id: i64,
    name: String,
    class: String,
    length_m: f32,
    oneway: u8,
    lanes: u8,
    maxspeed: u16,
    points: Vec<[f64; 2]>,
}

#[derive(Serialize)]
struct PoiDto {
    id: i64,
    name: String,
    category: String,
    lat: f64,
    lon: f64,
}

#[derive(Serialize)]
struct PoisRespDto {
    count: usize,
    pois: Vec<PoiDto>,
}

#[derive(Serialize)]
struct PaletteRoleDto {
    role: String,
    var: String,
    hex: String,
    rgba: Vec<u8>,
    source: String,
}

#[derive(Serialize)]
struct PaletteRespDto {
    var_count: usize,
    color_count: usize,
    roles: Vec<PaletteRoleDto>,
}

#[derive(Serialize)]
struct LayersDto {
    version: u64,
    layers: Vec<LayerStateDto>,
}

#[derive(Serialize)]
struct LayerStateDto {
    id: String,
    visible: bool,
    opacity: f32,
    zoom: Option<(u8, u8)>,
    color: Option<[u8; 3]>,
}

fn road_line_to_dto(l: &Line) -> RoadSummaryDto {
    RoadSummaryDto {
        id: l.id,
        name: l.name.clone(),
        class: road_class_name(l.class).to_string(),
        length_m: l.length_m,
        bbox: l.bbox,
        named: !l.name.is_empty(),
        oneway: l.oneway,
        lanes: l.lanes,
        maxspeed: l.maxspeed,
    }
}

/* --------------------------------------------------------------------- */
/* Rendering                                                               */
/* --------------------------------------------------------------------- */

/// Minimum zoom at which each road class appears.
fn class_min_zoom(class: u8) -> u8 {
    match class {
        0 => 5,   // motorway
        1 => 12,  // motorway_link
        2 => 7,   // trunk
        3 => 12,  // trunk_link
        4 => 8,   // primary
        5 => 13,  // primary_link
        6 => 10,  // secondary
        7 => 13,  // secondary_link
        8 => 11,  // tertiary
        9 => 13,  // tertiary_link
        10 => 13, // residential
        11 => 13, // unclassified
        12 => 14, // service
        13 => 13, // living_street
        14 => 13, // pedestrian
        15 => 12, // track
        16 => 14, // footway
        17 => 14, // path
        18 => 14, // cycleway
        19 => 15, // steps
        _ => 14,
    }
}

fn road_width(class: u8, z: u8) -> f32 {
    let base = match class {
        0 => 2.6,  // motorway
        1 => 1.6,  // motorway_link
        2 => 2.2,  // trunk
        3 => 1.4,  // trunk_link
        4 => 2.0,  // primary
        5 => 1.3,  // primary_link
        6 => 1.6,  // secondary
        7 => 1.1,  // secondary_link
        8 => 1.4,  // tertiary
        9 => 1.0,  // tertiary_link
        10 => 1.1, // residential
        11 => 0.9, // unclassified
        12 => 0.6, // service
        13 => 1.0, // living_street
        14 => 1.0, // pedestrian
        15 => 0.8, // track
        16 => 0.5, // footway
        17 => 0.5, // path
        18 => 0.5, // cycleway
        19 => 0.5, // steps
        _ => 0.4,
    };
    let grow = 1.0 + f32::from(z.saturating_sub(10)) * 0.35;
    base * grow
}

fn draw_arrow_marker(pixmap: &mut Pixmap, x: f32, y: f32, ux: f32, uy: f32, color: Color) {
    // A small triangle pointing along the road direction.
    let size = 4.2;
    let nx = -uy;
    let ny = ux;
    let tip = (x + ux * size, y + uy * size);
    let left = (x - ux * size * 0.7 + nx * size * 0.6, y - uy * size * 0.7 + ny * size * 0.6);
    let right = (x - ux * size * 0.7 - nx * size * 0.6, y - uy * size * 0.7 - ny * size * 0.6);
    let mut pb = PathBuilder::new();
    pb.move_to(tip.0, tip.1);
    pb.line_to(left.0, left.1);
    pb.line_to(right.0, right.1);
    pb.close();
    if let Some(path) = pb.finish() {
        let paint = Paint { shader: tiny_skia::Shader::SolidColor(color), anti_alias: true, ..Default::default() };
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn draw_oneway_arrows(pixmap: &mut Pixmap, pts: &[(f32, f32)], color: Color, reversed: bool) {
    if pts.len() < 2 {
        return;
    }
    let n = pts.len();
    let positions = [n / 4, n / 2, 3 * n / 4];
    for idx in positions {
        if idx == 0 || idx >= n {
            continue;
        }
        let (a, b) = (pts[idx - 1], pts[idx]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.001 {
            continue;
        }
        let (ux, uy) = (dx / len, dy / len);
        let (ux, uy) = if reversed { (-ux, -uy) } else { (ux, uy) };
        draw_arrow_marker(pixmap, pts[idx].0, pts[idx].1, ux, uy, color);
    }
}

fn oneway_arrow_color(app: &App, vis: &LayerVis, class: u8) -> Option<carto_style::Rgba> {
    let var = match road_class_name(class) {
        "motorway" | "trunk" | "primary" => "oneway-arrow",
        _ => "oneway-arrow-path",
    };
    let base = app.palette.get(var).or_else(|| app.palette.get("oneway-arrow")).unwrap_or(carto_style::Rgba::rgb(0x5b, 0x5b, 0x5b));
    Some(vis.color("road-oneway", base))
}

fn skia_color(c: carto_style::Rgba, alpha_mul: f32) -> Color {
    Color::from_rgba8(c.r, c.g, c.b, (255.0 * alpha_mul.clamp(0.0, 1.0)) as u8)
}

fn draw_line(pixmap: &mut Pixmap, pts: &[(f32, f32)], width: f32, color: Color, clip_px: &ClipRect) {
    if pts.len() < 2 {
        return;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] {
        pb.line_to(p.0, p.1);
    }
    let Some(path) = pb.finish() else { return };
    let paint = Paint { shader: tiny_skia::Shader::SolidColor(color), anti_alias: true, ..Default::default() };
    pixmap.stroke_path(&path, &paint, &Stroke { width: width.max(0.4), ..Default::default() }, Transform::identity(), None);
    let _ = clip_px;
}

fn draw_polygon(pixmap: &mut Pixmap, ring_px: &[(f32, f32)], color: Color, clip_px: &ClipRect) {
    if ring_px.len() < 3 {
        return;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(ring_px[0].0, ring_px[0].1);
    for p in &ring_px[1..] {
        pb.line_to(p.0, p.1);
    }
    pb.close();
    let Some(path) = pb.finish() else { return };
    let paint = Paint { shader: tiny_skia::Shader::SolidColor(color), anti_alias: true, ..Default::default() };
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    let _ = clip_px;
}

/// Blend one coverage bitmap onto the pixmap (premultiplied math).
fn blit_coverage(pixmap: &mut Pixmap, bmp: &[u8], w: usize, h: usize, ox: i32, oy: i32, col: (u8, u8, u8), alpha: u8) {
    let (pw, ph) = (pixmap.width() as usize, pixmap.height() as usize);
    let data = pixmap.data_mut();
    for row in 0..h {
        let py = oy + row as i32;
        if py < 0 || py >= ph as i32 {
            continue;
        }
        for cx in 0..w {
            let cov = bmp[row * w + cx] as u32;
            if cov == 0 {
                continue;
            }
            let px = ox + cx as i32;
            if px < 0 || px >= pw as i32 {
                continue;
            }
            let a = (cov * alpha as u32) / 255;
            if a == 0 {
                continue;
            }
            let i = (py as usize * pw + px as usize) * 4;
            let af = a as f32 / 255.0;
            let inv = 1.0 - af;
            // src premultiplied
            let sr = col.0 as f32 * af;
            let sg = col.1 as f32 * af;
            let sb = col.2 as f32 * af;
            data[i] = (sr + data[i] as f32 * inv).round().clamp(0.0, 255.0) as u8;
            data[i + 1] = (sg + data[i + 1] as f32 * inv).round().clamp(0.0, 255.0) as u8;
            data[i + 2] = (sb + data[i + 2] as f32 * inv).round().clamp(0.0, 255.0) as u8;
            data[i + 3] = (a as f32 + data[i + 3] as f32 * inv).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Draw text centered at (cx, baseline) with optional halo.
fn draw_text(
    pixmap: &mut Pixmap,
    font: &Font,
    text: &str,
    cx: f32,
    baseline: f32,
    size: f32,
    color: (u8, u8, u8),
    halo: Option<(u8, u8, u8)>,
    clip_px: &ClipRect,
) {
    if text.is_empty() {
        return;
    }
    // Quick reject: text bbox (approx by advance) outside clip.
    let mut glyphs: Vec<(fontdue::Metrics, Vec<u8>)> = Vec::new();
    let mut total_w = 0f32;
    for ch in text.chars() {
        let (m, bmp) = font.rasterize(ch, size);
        total_w += m.advance_width;
        glyphs.push((m, bmp));
    }
    let mut pen = cx - total_w / 2.0;
    let put = |pixmap: &mut Pixmap, pen: f32, col: (u8, u8, u8), alpha: u8| {
        let mut x = pen;
        for (m, bmp) in &glyphs {
            let ox = (x + m.xmin as f32).round() as i32;
            let oy = (baseline - (m.ymin as f32 + m.height as f32)).round() as i32;
            if !bmp.is_empty() {
                blit_coverage(pixmap, bmp, m.width, m.height, ox, oy, col, alpha);
            }
            x += m.advance_width;
        }
    };
    if let Some(hc) = halo {
        for &(dx, _dy) in &[
            (-1.0, 0.0),
            (1.0, 0.0),
            (0.0, -1.0),
            (0.0, 1.0),
            (-1.0, -1.0),
            (1.0, -1.0),
            (-1.0, 1.0),
            (1.0, 1.0),
        ] {
            put(pixmap, pen + dx, hc, 200);
        }
    }
    let _ = clip_px; // glyphs are small; blit clips per-pixel already
    put(pixmap, pen, color, 255);
    let _ = &mut pen;
}
pub fn render_tile_pixmap(app: &App, store: &Store, vis: &LayerVis, z: u8, x: u32, y: u32) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(256, 256)?;
    let land = app.palette.get("land-color").unwrap_or(carto_style::Rgba::rgb(0xf2, 0xef, 0xe9));
    let land = vis.color("background", land);
    if vis.is_visible("background") {
        pixmap.fill(skia_color(land, 1.0));
    } else {
        pixmap.fill(Color::from_rgba8(0, 0, 0, 0));
    }

    let [w, s, e, n] = tile_bbox(z, x, y);
    let (tw, ts, te, tn) = (w as f32, s as f32, e as f32, n as f32);
    let clip = ClipRect::new(-40.0, -40.0, 296.0, 296.0);

    // lon/lat -> tile-local px
    let scale = carto_math::world_px(z);
    let ox = f64::from(x) * 256.0;
    let oy = f64::from(y) * 256.0;
    let project = |i: u32| -> (f32, f32) {
        let (mx, my) = lonlat_to_merc(f64::from(store.lon[i as usize]), f64::from(store.lat[i as usize]));
        ((mx * scale - ox) as f32, (my * scale - oy) as f32)
    };
    let in_tile = |b: &[f32; 4]| b[2] >= tw && b[0] <= te && b[3] >= ts && b[1] <= tn;

    let line_px = |pts: &Vec<u32>| -> Vec<(f32, f32)> { pts.iter().map(|&i| project(i)).collect() };

    /* ---- landuse polygons ---- */
    let lu_colors: &[(&str, &str)] = &[
        ("forest", "forest"),
        ("farmland", "farmland"),
        ("farmyard", "farmyard"),
        ("grass", "grass"),
        ("park", "park"),
        ("garden", "park"),
        ("pitch", "grass"),
        ("golf_course", "grass"),
        ("playground", "grass"),
        ("orchard", "orchard"),
        ("allotments", "allotments"),
        ("scrub", "scrub"),
        ("residential", "residential"),
        ("retail", "retail"),
        ("commercial", "commercial"),
        ("industrial", "industrial"),
        ("school", "residential"),
        ("hospital", "residential"),
        ("college", "residential"),
        ("university", "residential"),
        ("place_of_worship", "residential"),
        ("marketplace", "retail"),
        ("sand", "farmland"),
        ("beach", "farmland"),
    ];
    if vis.on_with_default("landuse", 8, 19, z) {
        let la = vis.alpha("landuse");
        for a in &store.landuse {
            if z < 8 || !in_tile(&a.bbox) {
                continue;
            }
            let is_major = matches!(a.kind.as_str(), "forest" | "farmland" | "scrub");
            if z < 10 && !is_major {
                continue;
            }
            let var = lu_colors.iter().find(|(k, _)| *k == a.kind).map(|(_, v)| *v);
            let Some(var) = var else { continue };
            let Some(c) = app.palette.get(var) else { continue };
            let c = vis.color("landuse", c);
            let ring = line_px(&a.pts);
            if ring.len() >= 3 {
                draw_polygon(&mut pixmap, &ring, skia_color(c, la), &clip);
            }
        }
    }

    /* ---- water areas ---- */
    if vis.on_with_default("water-areas", 7, 19, z) && z >= 7 {
        let wa = vis.alpha("water-areas");
        let wc = app.palette.get("water-color").unwrap_or(carto_style::Rgba::rgb(0xaa, 0xd3, 0xdf));
        let wc = vis.color("water-areas", wc);
        for a in &store.water_areas {
            if in_tile(&a.bbox) {
                let ring = line_px(&a.pts);
                if ring.len() >= 3 {
                    draw_polygon(&mut pixmap, &ring, skia_color(wc, wa), &clip);
                }
            }
        }
    }

    /* ---- waterways ---- */
    if vis.on_with_default("waterways", 8, 19, z) && z >= 8 {
        let wa = vis.alpha("waterways");
        let wc = app.palette.get("water-color").unwrap_or(carto_style::Rgba::rgb(0xaa, 0xd3, 0xdf));
        let wc = vis.color("waterways", wc);
        let width = match z {
            8 => 0.7,
            9 => 1.2,
            10 => 1.5,
            11 => 1.8,
            12 => 2.3,
            13 => 3.0,
            14 => 3.5,
            15 => 4.5,
            _ => 6.0,
        };
        for wway in &store.waterways {
            if !in_tile(&wway.bbox) {
                continue;
            }
            draw_line(&mut pixmap, &line_px(&wway.pts), width, skia_color(wc, wa), &clip);
        }
    }

    /* ---- buildings ---- */
    if vis.on_with_default("buildings", 13, 19, z) && z >= 13 {
        let ba = vis.alpha("buildings");
        let bf = app.palette.get("building-fill").unwrap_or(carto_style::Rgba::rgb(0xd9, 0xd0, 0xc9));
        let bf = vis.color("buildings", bf);
        for b in &store.buildings {
            if in_tile(&b.bbox) {
                let ring = line_px(&b.pts);
                if ring.len() >= 3 {
                    draw_polygon(&mut pixmap, &ring, skia_color(bf, ba), &clip);
                }
            }
        }
        if z >= 15 && ba > 0.0 {
            let bl = app.palette.get("building-line").unwrap_or(carto_style::Rgba::rgb(0xbc, 0xb3, 0xaa));
            for b in &store.buildings {
                if in_tile(&b.bbox) {
                    let ring = line_px(&b.pts);
                    if ring.len() >= 3 {
                        draw_polygon(&mut pixmap, &ring, skia_color(bl, 0.6 * ba), &clip);
                    }
                }
            }
        }
    }

    /* ---- railways (casing) ---- */
    if vis.on_with_default("rail", 9, 19, z) && z >= 9 {
        let ra = vis.alpha("rail");
        let rc = vis.color("rail", carto_style::Rgba::rgb(0x8a, 0x8a, 0x8a));
        let width = match z {
            9 | 10 => 0.9,
            11 | 12 => 1.4,
            13 | 14 => 2.0,
            _ => 2.6,
        };
        for r in &store.railways {
            if in_tile(&r.bbox) {
                draw_line(&mut pixmap, &line_px(&r.pts), width, skia_color(rc, ra), &clip);
            }
        }
    }

    /* ---- roads: casings then fills ---- */
    let casing_for = |class: u8| -> carto_style::Rgba {
        let name = road_class_name(class);
        let var = match name {
            "motorway" | "motorway_link" => "motorway-casing",
            "trunk" | "trunk_link" => "trunk-casing",
            "primary" | "primary_link" => "primary-casing",
            "secondary" | "secondary_link" => "secondary-casing",
            "tertiary" => "tertiary-casing",
            "residential" | "unclassified" | "service" => "residential-casing",
            "living_street" | "pedestrian" => "residential-casing",
            "track" => "default-casing",
            "footway" | "path" | "cycleway" | "steps" => "default-casing",
            _ => "default-casing",
        };
        app.palette.get(var).unwrap_or(carto_style::Rgba::rgb(255, 255, 255))
    };
    let fill_for = |app: &App, class: u8, z: u8| -> carto_style::Rgba {
        let name = road_class_name(class);
        let low = |base: &str, lowzoom: &str| {
            if z <= 11 {
                app.palette.get(lowzoom).or_else(|| app.palette.get(base)).unwrap()
            } else {
                app.palette.get(base).unwrap()
            }
        };
        match name {
            "motorway" | "motorway_link" => low("motorway-fill", "motorway-low-zoom"),
            "trunk" | "trunk_link" => low("trunk-fill", "trunk-low-zoom"),
            "primary" | "primary_link" => low("primary-fill", "primary-low-zoom"),
            "secondary" | "secondary_link" => low("secondary-fill", "secondary-low-zoom"),
            "tertiary" => app.palette.get("tertiary-fill").unwrap(),
            "residential" | "unclassified" => app.palette.get("residential-fill").unwrap(),
            "living_street" => app.palette.get("living-street-fill").unwrap(),
            "pedestrian" => app.palette.get("pedestrian-fill").unwrap(),
            "service" => app.palette.get("residential-fill").unwrap(),
            "track" => app.palette.get("track-fill").unwrap(),
            "footway" | "path" | "cycleway" | "steps" => app.palette.get("footway-fill").unwrap(),
            _ => carto_style::Rgba::rgb(0xdd, 0xdd, 0xdd),
        }
    };

    let mut labeled: Vec<(&Line, Vec<(f32, f32)>)> = Vec::new();
    let show_casing = z >= 12 && vis.on_with_default("road-casing", 12, 19, z);
    let casing_alpha = vis.alpha("road-casing");
    // casings
    if show_casing {
        for r in &store.roads {
            let width = road_width(r.class, z);
            if width <= 0.0 || !in_tile(&r.bbox) {
                continue;
            }
            let c = vis.color("road-casing", casing_for(r.class));
            let cw = width + if width >= 4.0 { 2.0 } else { 1.2 };
            draw_line(&mut pixmap, &line_px(&r.pts), cw, skia_color(c, casing_alpha), &clip);
        }
    }
    // fills
    if vis.on_with_default("road-fill", 5, 19, z) {
        let fa = vis.alpha("road-fill");
        for r in &store.roads {
            let width = road_width(r.class, z);
            if width <= 0.0 || !in_tile(&r.bbox) || z < class_min_zoom(r.class) {
                continue;
            }
            let pts = line_px(&r.pts);
            let c = vis.color("road-fill", fill_for(app, r.class, z));
            draw_line(&mut pixmap, &pts, width, skia_color(c, fa), &clip);
            // ::directions — oneway arrows from z16 (roads.mss), under the labels
            if z >= 16 && r.oneway != 0 && width >= 1.4 && vis.on_with_default("road-oneway", 16, 19, z) {
                if let Some(ac) = oneway_arrow_color(app, vis, r.class) {
                    draw_oneway_arrows(&mut pixmap, &pts, skia_color(ac, fa), r.oneway == 2);
                }
            }
            let label_cap = 24 + usize::from(z.saturating_sub(14)) * 12;
            if z >= 13 && !r.name.is_empty() && width >= 1.8 && labeled.len() < label_cap {
                labeled.push((r, pts));
            }
        }
    }

    /* ---- road labels ---- */
    if z >= 13 && vis.on_with_default("road-labels", 13, 19, z) && !labeled.is_empty() {
        let mut used: HashSet<(u16, u16)> = HashSet::new();
        let size = if z >= 15 { 11.0 } else { 10.0 };
        let fnt: &Font = if z >= 15 { &app.font_bold } else { &app.font };
        let shield = |class: u8| -> (u8, u8, u8) {
            let c = match road_class_name(class) {
                "motorway" | "motorway_link" => app.palette.get("motorway-shield"),
                "trunk" | "trunk_link" => app.palette.get("trunk-shield"),
                "primary" | "primary_link" => app.palette.get("primary-shield"),
                "secondary" | "secondary_link" => app.palette.get("secondary-shield"),
                "tertiary" => app.palette.get("tertiary-shield"),
                _ => None,
            };
            c.map(|x| (x.r, x.g, x.b)).unwrap_or((0x3b, 0x3b, 0x3b))
        };
        for (r, pts) in &labeled {
            // label at the middle vertex
            let mid = pts[pts.len() / 2];
            let gx = (mid.0 / 48.0) as u16;
            let gy = (mid.1 / 48.0) as u16;
            if !used.insert((gx, gy)) {
                continue;
            }
            if mid.0 < 8.0 || mid.0 > 248.0 || mid.1 < 8.0 || mid.1 > 248.0 {
                continue;
            }
            let halo = app.palette.get("default-casing").map(|c| (c.r, c.g, c.b)).unwrap_or((255, 255, 255));
            let base = carto_style::Rgba::rgb(shield(r.class).0, shield(r.class).1, shield(r.class).2);
            let col = vis.color("road-labels", base);
            draw_text(
                &mut pixmap,
                fnt,
                &r.name,
                mid.0,
                mid.1 + size / 3.0,
                size,
                (col.r, col.g, col.b),
                Some(halo),
                &clip,
            );
        }
    }

    /* ---- POI dots + labels ---- */
    if vis.on_with_default("pois", 14, 19, z) && z >= 14 {
        let pa = vis.alpha("pois");
        let accent = app.palette.get("motorway-casing").unwrap_or(carto_style::Rgba::rgb(0xdc, 0x2a, 0x67));
        let accent = vis.color("pois", accent);
        let mut count = 0;
        for p in &store.pois {
            let (lon, lat) = (f64::from(store.lon[p.idx as usize]), f64::from(store.lat[p.idx as usize]));
            if lon < f64::from(tw) || lon > f64::from(te) || lat < f64::from(ts) || lat > f64::from(tn) {
                continue;
            }
            let (px, py) = {
                let (mx, my) = lonlat_to_merc(lon, lat);
                ((mx * scale - ox) as f32, (my * scale - oy) as f32)
            };
            if px < 4.0 || px > 252.0 || py < 4.0 || py > 252.0 || count >= 40 {
                continue;
            }
            count += 1;
            let mut pb = PathBuilder::new();
            pb.push_circle(px, py, 2.4);
            if let Some(path) = pb.finish() {
                let paint = Paint { shader: tiny_skia::Shader::SolidColor(skia_color(accent, pa)), anti_alias: true, ..Default::default() };
                pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                let paint_w = Paint { shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(255, 255, 255, 220)), anti_alias: true, ..Default::default() };
                let mut pbc = PathBuilder::new();
                pbc.push_circle(px, py, 2.4);
                if let Some(ring) = pbc.finish() {
                    pixmap.stroke_path(&ring, &paint_w, &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
                }
            }
            if z >= 16 {
                draw_text(&mut pixmap, &app.font, &p.name, px, py - 5.0, 9.0, (0x2c, 0x2c, 0x2c), Some((255, 255, 255)), &clip);
            }
        }
    }

    Some(pixmap)
}

/// PNG-encoded tile (the HTTP server path).
pub fn render_tile(app: &App, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    let store = app.store.read().unwrap();
    let vis = app.layers.read().unwrap().clone();
    let pm = render_tile_pixmap(app, &store, &vis, z, x, y)?;
    pm.encode_png().ok().map(|v| v.to_vec())
}

/* --------------------------------------------------------------------- */
/* HTTP plumbing                                                           */
/* --------------------------------------------------------------------- */

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub cache_hdr: Option<&'static str>,
}

impl Response {
    fn json(status: u16, body: String) -> Response {
        Response { status, content_type: "application/json; charset=utf-8", body: body.into_bytes(), cache_hdr: None }
    }
    fn png(body: Vec<u8>) -> Response {
        Response { status: 200, content_type: "image/png", body, cache_hdr: Some("public, max-age=86400") }
    }
    fn not_found(msg: &str) -> Response {
        Response::json(404, format!("{{\"error\":\"{msg}\"}}"))
    }
}

fn write_response(stream: &mut TcpStream, resp: &Response, keep_alive: bool) -> std::io::Result<()> {
    let reason = match resp.status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n",
        resp.status, reason, resp.content_type, resp.body.len()
    );
    if let Some(c) = resp.cache_hdr {
        head.push_str(&format!("Cache-Control: {c}\r\n"));
    }
    head.push_str(if keep_alive { "Connection: keep-alive\r\n\r\n" } else { "Connection: close\r\n\r\n" });
    stream.write_all(head.as_bytes())?;
    stream.write_all(&resp.body)?;
    stream.flush()
}

fn handle_client(stream: TcpStream, app: Arc<App>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let mut reader = BufReader::new(stream);

    loop {
        // Request line + headers
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");
        let version = parts.next().unwrap_or("HTTP/1.1");
        if method != "GET" {
            let _ = write_response(&mut writer, &Response::not_found("method not allowed"), false);
            return;
        }
        // Drain headers (discard Content-Length bodies just in case).
        let mut content_length = 0usize;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            let h = h.trim();
            if h.is_empty() {
                break;
            }
            let lower = h.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
        if content_length > 0 {
            let mut sink = vec![0u8; content_length.min(1 << 20)];
            let _ = reader.read_exact(&mut sink);
        }

        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (target.to_string(), String::new()),
        };
        let qp: HashMap<&str, &str> = query.split('&').filter_map(|kv| kv.split_once('=')).collect();
        let keep_alive = version != "HTTP/1.0"
            && !qp.get("Connection").map(|c| c.eq_ignore_ascii_case("close")).unwrap_or(false);

        let resp = {
            // Snapshot the store + layer overrides for this request.
            let store = app.store.read().unwrap();
            let vis = app.layers.read().unwrap().clone();
            route(&app, &store, &vis, &path, &qp)
        };
        if write_response(&mut writer, &resp, keep_alive).is_err() {
            return;
        }
        if !keep_alive {
            return;
        }
    }
}

fn route(app: &App, store: &Store, vis: &LayerVis, path: &str, qp: &HashMap<&str, &str>) -> Response {
    match path {
        "/" => Response::json(
            200,
            format!("{{\"ok\":true,\"service\":\"map-launcher tile-server\",\"version\":\"{VERSION}\"}}"),
        ),
        "/api/health" => Response::json(200, "{\"ok\":true}".to_string()),
        "/api/stats" => {
            let named = store.roads.iter().filter(|r| !r.name.is_empty()).count();
            let dto = StatsDto {
                roads: store.roads.len(),
                named_roads: named,
                railways: store.railways.len(),
                waterways: store.waterways.len(),
                water_areas: store.water_areas.len(),
                landuse: store.landuse.len(),
                buildings: store.buildings.len(),
                pois: store.pois.len(),
                bounds: store.bbox,
            };
            Response::json(200, serde_json::to_string(&dto).unwrap_or_default())
        }
        "/api/roads" => {
            let limit: usize = qp.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50_000).min(200_000);
            let roads: Vec<RoadSummaryDto> = store.roads.iter().take(limit).map(road_line_to_dto).collect();
            let dto = RoadsRespDto { count: roads.len(), roads };
            Response::json(200, serde_json::to_string(&dto).unwrap_or_default())
        }
        "/api/pois" => {
            let limit: usize = qp.get("limit").and_then(|v| v.parse().ok()).unwrap_or(20_000).min(100_000);
            let pois: Vec<PoiDto> = store
                .pois
                .iter()
                .take(limit)
                .map(|p| PoiDto {
                    id: p.id,
                    name: p.name.clone(),
                    category: p.category.clone(),
                    lat: f64::from(store.lat[p.idx as usize]),
                    lon: f64::from(store.lon[p.idx as usize]),
                })
                .collect();
            let dto = PoisRespDto { count: pois.len(), pois };
            Response::json(200, serde_json::to_string(&dto).unwrap_or_default())
        }
        "/api/palette" => {
            let roles: Vec<PaletteRoleDto> = app
                .palette
                .roles
                .iter()
                .map(|r| {
                    let (rr, gg, bb, aa) = r.color.rgba_tuple();
                    PaletteRoleDto {
                        role: r.role.clone(),
                        var: r.var.clone(),
                        hex: r.color.hex(),
                        rgba: if aa == 255 { vec![rr, gg, bb] } else { vec![rr, gg, bb, aa] },
                        source: r.source.clone(),
                    }
                })
                .collect();
            let dto = PaletteRespDto { var_count: app.palette.var_count(), color_count: app.palette.color_count, roles };
            Response::json(200, serde_json::to_string(&dto).unwrap_or_default())
        }
        "/api/layers" => {
            let defs = layers::layer_catalog();
            let state: Vec<LayerStateDto> = defs
                .iter()
                .map(|d| LayerStateDto {
                    id: d.id.to_string(),
                    visible: vis.is_visible(d.id),
                    opacity: vis.alpha(d.id),
                    zoom: vis.zoom.get(d.id).copied(),
                    color: vis.color_override(d.id),
                })
                .collect();
            let dto = LayersDto { version: vis.version, layers: state };
            Response::json(200, serde_json::to_string(&dto).unwrap_or_default())
        }
        _ => {
            if let Some(rest) = path.strip_prefix("/api/roads/") {
                let id: i64 = match rest.parse() {
                    Ok(v) => v,
                    Err(_) => return Response::not_found("invalid road id"),
                };
                let r = store.roads.iter().find(|r| r.id == id);
                return match r {
                    Some(r) => {
                        let points: Vec<[f64; 2]> = r
                            .pts
                            .iter()
                            .map(|&i| [f64::from(store.lon[i as usize]), f64::from(store.lat[i as usize])])
                            .collect();
                        let dto = RoadDetailDto {
                            id: r.id,
                            name: r.name.clone(),
                            class: road_class_name(r.class).to_string(),
                            length_m: r.length_m,
                            oneway: r.oneway,
                            lanes: r.lanes,
                            maxspeed: r.maxspeed,
                            points,
                        };
                        Response::json(200, serde_json::to_string(&dto).unwrap_or_default())
                    }
                    None => Response::not_found("road not found"),
                };
            }
            if let Some(rest) = path.strip_prefix("/tile/") {
                return tile_route(app, rest);
            }
            Response::not_found("no such route")
        }
    }
}

fn tile_route(app: &App, rest: &str) -> Response {
    let rest = rest.strip_suffix(".png").unwrap_or(rest);
    let segs: Vec<&str> = rest.split('/').collect();
    if segs.len() != 3 {
        return Response::not_found("bad tile path");
    }
    let (z, x, y): (u8, u32, u32) = match (segs[0].parse(), segs[1].parse(), segs[2].parse()) {
        (Ok(z), Ok(x), Ok(y)) => (z, x, y),
        _ => return Response::not_found("bad tile coords"),
    };
    if z > 19 {
        return Response::not_found("zoom out of range");
    }
    let key: TileKey = (z, x, y);
    {
        let mut cache = app.cache.lock().unwrap();
        if let Some(png) = cache.get(&key) {
            app.hits.fetch_add(1, Ordering::Relaxed);
            return Response::png((*png).clone());
        }
        app.misses.fetch_add(1, Ordering::Relaxed);
    }
    let png = render_tile(app, z, x, y).unwrap_or_else(|| {
        // 1x1 transparent fallback
        let pm = Pixmap::new(1, 1).unwrap();
        pm.encode_png().unwrap().to_vec()
    });
    let arc = Arc::new(png);
    app.cache.lock().unwrap().put(key, arc.clone());
    Response::png((*arc).clone())
}

/* --------------------------------------------------------------------- */
/* Server lifecycle                                                        */
/* --------------------------------------------------------------------- */

/// A running tile-server listener that can be stopped from the GUI.
pub struct ServerHandle {
    pub port: u16,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ServerHandle {
    /// Stop accepting and join the listener thread.
    pub fn stop(mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        // Wake the poll loop by connecting once.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let addr: std::net::SocketAddr = format!("127.0.0.1:{}", self.port).parse().unwrap_or_else(|_| "127.0.0.1:0".parse().unwrap());
        let _ = TcpStream::connect_timeout(&addr, Duration::from_millis(200));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Start the HTTP listener in a background thread (GUI mode keeps serving
/// while the window is open).
pub fn spawn_server(port: u16, app: Arc<App>) -> std::io::Result<ServerHandle> {
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    listener.set_nonblocking(true)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let sd = shutdown.clone();
    let thread = std::thread::spawn(move || {
        while !sd.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let app = app.clone();
                    std::thread::spawn(move || handle_client(stream, app));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(40));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(40)),
            }
        }
    });
    Ok(ServerHandle { port, shutdown, thread: Some(thread) })
}

/* --------------------------------------------------------------------- */
/* Startup stack (palette + fonts + store)                                 */
/* --------------------------------------------------------------------- */

pub struct Stack {
    pub palette: Palette,
    pub palette_src: String,
    pub symbols: SymbolRegistry,
    pub font: Font,
    pub font_src: String,
    pub font_bold: Font,
    pub font_bold_src: String,
    pub store: Store,
    pub data_dir: PathBuf,
    pub searched: Vec<PathBuf>,
}

pub fn abs_root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Resolve the data directory holding `store.bin` (+ `registry.json`).
/// Priority: `--data-dir` flag > `MAP_LAUNCHER_DATA` env > `./data` (cwd) >
/// `<exe dir>/data` > `<exe dir>` (flat release zip layout). The first
/// candidate that actually contains `store.bin` wins; all searched paths are
/// reported so a bare release binary tells the user exactly what to fix.
pub fn resolve_data_dir(root: &PathBuf, flag: Option<String>) -> (PathBuf, Vec<PathBuf>) {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(d) = flag {
        candidates.push(PathBuf::from(d));
    }
    if let Ok(v) = std::env::var("MAP_LAUNCHER_DATA") {
        if !v.trim().is_empty() {
            candidates.push(PathBuf::from(v.trim().to_string()));
        }
    }
    candidates.push(root.join("data"));
    if let Some(d) = exe_dir {
        candidates.push(d.join("data"));
        candidates.push(d);
    }
    let mut unique: Vec<PathBuf> = Vec::new();
    for c in candidates {
        if !unique.contains(&c) {
            unique.push(c);
        }
    }
    let chosen = unique
        .iter()
        .find(|d| d.join("store.bin").is_file())
        .cloned()
        .unwrap_or_else(|| unique.first().cloned().unwrap_or_else(|| root.join("data")));
    (chosen, unique)
}

/// Load the label font face: system scan first (craft-launcher style), then
/// the build-machine bundled assets, then the DejaVu bytes embedded in this
/// binary — the guaranteed fallback for bare release executables.
pub fn load_font_face(bold: bool) -> (Font, String) {
    let path = carto_style::load_map_font(bold).or_else(|| carto_style::find_font("dejavu", bold));
    if let Some(p) = path {
        match std::fs::read(&p) {
            Ok(bytes) => match Font::from_bytes(bytes, FontSettings { scale: 12.0, ..Default::default() }) {
                Ok(f) => return (f, p.display().to_string()),
                Err(e) => eprintln!("font {} parse failed ({e}) — using embedded DejaVu", p.display()),
            },
            Err(e) => eprintln!("font {} unreadable ({e}) — using embedded DejaVu", p.display()),
        }
    }
    let (name, bytes) = carto_style::embedded_font("DejaVu Sans", bold).expect("embedded DejaVu font missing (build corruption)");
    let font = Font::from_bytes(bytes.to_vec(), FontSettings { scale: 12.0, ..Default::default() }).expect("embedded font parse failed");
    (font, format!("embedded:{name}"))
}

impl Stack {
    /// The startup banner (shared by both binaries).
    pub fn log(&self) {
        eprintln!(
            "palette[{}]: {} vars, {} colors, {} roles, {} symbols",
            self.palette_src, self.palette.var_count(), self.palette.color_count, self.palette.roles.len(), self.symbols.svg_count
        );
        eprintln!("fonts: regular={} bold={}", self.font_src, self.font_bold_src);
        eprintln!(
            "registry: {} roads ({} named), {} buildings, {} pois, {} landuse, {} waterways",
            self.store.roads.len(),
            self.store.roads.iter().filter(|r| !r.name.is_empty()).count(),
            self.store.buildings.len(),
            self.store.pois.len(),
            self.store.landuse.len(),
            self.store.waterways.len()
        );
    }

    pub fn region_label(&self) -> String {
        "Active data".to_string()
    }
}

/// Load palette, fonts and the store exactly like the server always did.
pub fn load_stack(data_flag: Option<String>) -> Stack {
    let root = abs_root();
    let assets = root.join("assets");

    // Palette: on-disk assets/style first (dev checkout), else the verbatim
    // openstreetmap-carto style sheets embedded in this binary.
    let style_dir = assets.join("style");
    let (palette, palette_src) = if style_dir.is_dir() {
        match Palette::from_dir(&style_dir) {
            Ok(p) => (p, style_dir.display().to_string()),
            Err(e) => {
                eprintln!("palette load from {} failed ({e}) — using embedded openstreetmap-carto style", style_dir.display());
                (Palette::from_embedded(), "embedded assets/style".to_string())
            }
        }
    } else {
        (Palette::from_embedded(), "embedded assets/style".to_string())
    };
    let symbols_dir = assets.join("symbols");
    let symbols = if symbols_dir.is_dir() {
        SymbolRegistry::scan(&symbols_dir)
    } else {
        SymbolRegistry::embedded()
    };

    let (font, font_src) = load_font_face(false);
    let (font_bold, font_bold_src) = load_font_face(true);

    let (data_dir, searched) = resolve_data_dir(&root, data_flag);
    let store = match Store::load(&data_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("store load failed ({e}) — empty registry");
            eprintln!("  searched: {}", searched.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", "));
            eprintln!("  fix 1: unzip the map-data release asset so store.bin sits in one of those dirs");
            eprintln!("  fix 2: tile-server extract <pbf> <data-dir> [w s e n]");
            eprintln!("  fix 3: open the GUI and pick a region on the Downloads tab");
            Store::empty(carto_core::DODOMA_BBOX)
        }
    };
    Stack { palette, palette_src, symbols, font, font_src, font_bold, font_bold_src, store, data_dir, searched }
}

/// Build the shared [`App`] from a loaded [`Stack`].
pub fn app_from_stack(stack: Stack) -> Arc<App> {
    let region = if stack.store.roads.is_empty() { "no data loaded".to_string() } else { "active data".to_string() };
    Arc::new(App {
        store: RwLock::new(stack.store),
        layers: RwLock::new(LayerVis::default()),
        palette: stack.palette,
        font: stack.font,
        font_bold: stack.font_bold,
        symbol_count: stack.symbols.svg_count,
        cache: Mutex::new(TileCache::new(768)),
        hits: AtomicUsize::new(0),
        misses: AtomicUsize::new(0),
        region_name: Mutex::new(region),
    })
}

/* --------------------------------------------------------------------- */
/* CLI (headless entry points, shared by tile-server and tile-server-cli)  */
/* --------------------------------------------------------------------- */

/// `tile-server [-|cli] extract <pbf> <outdir> [w s e n]`
pub fn run_extract(args: &[String]) -> i32 {
    if args.len() < 2 {
        eprintln!("usage: tile-server extract <pbf> <outdir> [w s e n]");
        return 2;
    }
    let pbf = PathBuf::from(&args[0]);
    let out = PathBuf::from(&args[1]);
    let bbox = if args.len() >= 6 {
        [args[2].parse().unwrap(), args[3].parse().unwrap(), args[4].parse().unwrap(), args[5].parse().unwrap()]
    } else {
        carto_core::DODOMA_BBOX
    };
    eprintln!("extracting {pbf:?} bbox={bbox:?} -> {out:?}");
    match carto_core::extract(&pbf, &out, bbox) {
        Ok(store) => {
            eprintln!("done: {} roads / {} buildings / {} pois", store.roads.len(), store.buildings.len(), store.pois.len());
            0
        }
        Err(e) => {
            eprintln!("extract failed: {e}");
            1
        }
    }
}

/// `tile-server fetch <url> <outdir> [w s e n]` — download a PBF (with
/// progress, reconnect and resume, like the launcher) and build the store.
pub fn run_fetch(url: &str, out_dir: &PathBuf, bbox: [f64; 4]) -> i32 {
    use std::path::Path;
    let downloads_dir = out_dir.join("downloads");
    let _ = std::fs::create_dir_all(&downloads_dir);
    let pbf = downloads_dir.join(download::pbf_name_for(url));
    eprintln!("fetching {url} -> {}", pbf.display());
    let jobs = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    if let Err(e) = download::download_with_resume(url, &pbf, "fetch", &jobs, &cancel) {
        eprintln!("fetch failed: {e}");
        return 1;
    }
    eprintln!("PBF cached: {}", pbf.display());
    match carto_core::extract(Path::new(&pbf), Path::new(out_dir), bbox) {
        Ok(store) => {
            eprintln!("done: {} roads / {} buildings / {} pois", store.roads.len(), store.buildings.len(), store.pois.len());
            0
        }
        Err(e) => {
            eprintln!("extract failed: {e}");
            1
        }
    }
}

/// `tile-server serve [--port N] [--data-dir D]` — the headless HTTP server.
pub fn run_headless(port: u16, data_flag: Option<String>) -> i32 {
    let stack = load_stack(data_flag);
    stack.log();
    let app = app_from_stack(stack);
    match spawn_server(port, app) {
        Ok(handle) => {
            eprintln!("map-launcher tile-server v{VERSION} listening on :{}", handle.port);
            // Block forever (the caller's process is the server).
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        Err(e) => {
            eprintln!("bind :{port} failed ({e}) — is another tile-server already running?");
            1
        }
    }
}
