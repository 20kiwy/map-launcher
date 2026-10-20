//! map-launcher tile-server engine: the CPU renderer, HTTP tile/API server,
//! layer engine overrides, data-dir resolution and the headless entry points
//! shared by the `tile-server` (GUI) and `tile-server-cli` binaries.

pub mod download;
pub mod layers;
pub mod sprites;

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
    /// Cached SVG sprite rasterizer (POI icons + landcover patterns).
    pub sprites: sprites::Sprites,
    pub cache: Mutex<TileCache>,
    pub hits: AtomicUsize,
    pub misses: AtomicUsize,
    /// Human label of the active region ("Central Tanzania (Dodoma)" etc).
    pub region_name: Mutex<String>,
    /// Folder the active store was loaded from (shown as the data folder;
    /// updated whenever a region or a browsed folder is swapped in).
    pub data_dir: Mutex<PathBuf>,
}

impl App {
    /// Drop every cached tile (after a layer edit or a region switch).
    pub fn invalidate(&self) {
        self.cache.lock().unwrap().clear();
    }

    /// Swap in a new store at runtime (Downloads tab: install or browse).
    pub fn set_store(&self, store: Store, label: String, dir: PathBuf) {
        *self.store.write().unwrap() = store;
        *self.region_name.lock().unwrap() = label;
        *self.data_dir.lock().unwrap() = dir;
        self.invalidate();
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
            places: store.places.len(),
            ocean: store.ocean.len(),
            islands: store.islands.len(),
            admin: store.admin.len(),
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
    pub places: usize,
    pub ocean: usize,
    pub islands: usize,
    pub admin: usize,
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
        12 => 13, // living_street
        13 => 13, // pedestrian
        14 => 14, // service
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
        12 => 1.0, // living_street
        13 => 1.0, // pedestrian
        14 => 0.6, // service
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

/// Dashed stroke (tunnels, ferries, golf paths…).
fn draw_line_dashed(
    pixmap: &mut Pixmap,
    pts: &[(f32, f32)],
    width: f32,
    color: Color,
    dash: &[f32],
    clip_px: &ClipRect,
) {
    if pts.len() < 2 || dash.is_empty() {
        return draw_line(pixmap, pts, width, color, clip_px);
    }
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] {
        pb.line_to(p.0, p.1);
    }
    let Some(path) = pb.finish() else { return };
    let mut stroke = Stroke { width: width.max(0.4), ..Default::default() };
    stroke.dash = tiny_skia::StrokeDash::new(dash.to_vec(), 0.0);
    let paint = Paint { shader: tiny_skia::Shader::SolidColor(color), anti_alias: true, ..Default::default() };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

/// Polygon filled with a tiled SVG pattern (landcover-area-symbols).
fn draw_polygon_pattern(
    pixmap: &mut Pixmap,
    ring_px: &[(f32, f32)],
    base: Color,
    pattern: &Pixmap,
    alpha: f32,
    clip_px: &ClipRect,
) {
    if ring_px.len() < 3 {
        return;
    }
    // Base fill first (carto paints polygon-fill + polygon-pattern-file).
    draw_polygon(pixmap, ring_px, base, clip_px);
    let Some(path) = ring_path(ring_px) else { return };
    let shader = tiny_skia::Pattern::new(
        pattern.as_ref(),
        tiny_skia::SpreadMode::Repeat,
        tiny_skia::FilterQuality::Bilinear,
        alpha.clamp(0.0, 1.0),
        Transform::identity(),
    );
    let paint = Paint { shader, anti_alias: true, ..Default::default() };
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

fn ring_path(ring_px: &[(f32, f32)]) -> Option<tiny_skia::Path> {
    if ring_px.len() < 3 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(ring_px[0].0, ring_px[0].1);
    for p in &ring_px[1..] {
        pb.line_to(p.0, p.1);
    }
    pb.close();
    pb.finish()
}

/// A small filled circle (tree, crossing, POI dot…).
fn draw_dot(pixmap: &mut Pixmap, x: f32, y: f32, r: f32, fill: Color, ring: Option<Color>) {
    let mut pb = PathBuilder::new();
    pb.push_circle(x, y, r);
    if let Some(path) = pb.finish() {
        let paint = Paint { shader: tiny_skia::Shader::SolidColor(fill), anti_alias: true, ..Default::default() };
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
    if let Some(rc) = ring {
        let mut pbr = PathBuilder::new();
        pbr.push_circle(x, y, r);
        if let Some(path) = pbr.finish() {
            let paint = Paint { shader: tiny_skia::Shader::SolidColor(rc), anti_alias: true, ..Default::default() };
            pixmap.stroke_path(&path, &paint, &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
        }
    }
}

/// Blit a sprite pixmap centered at (x, y), scaled to fit `box_px`.
fn blit_sprite(pixmap: &mut Pixmap, sprite: &Pixmap, x: f32, y: f32, box_px: f32, alpha: f32) {
    let (w, h) = (sprite.width() as f32, sprite.height() as f32);
    if w <= 0.0 || h <= 0.0 || box_px <= 0.0 {
        return;
    }
    let s = (box_px / w).min(box_px / h);
    let paint = tiny_skia::PixmapPaint {
        opacity: alpha.clamp(0.0, 1.0),
        blend_mode: tiny_skia::BlendMode::SourceOver,
        quality: tiny_skia::FilterQuality::Bilinear,
    };
    let t = Transform::from_translate(x - w * s / 2.0, y - h * s / 2.0)
        .post_concat(Transform::from_scale(s, s));
    pixmap.draw_pixmap(0, 0, sprite.as_ref(), &paint, t, None);
}

/// One rasterized, tinted glyph as its own pixmap (for along-path text).
fn glyph_pixmap(font: &Font, ch: char, size: f32, col: (u8, u8, u8), alpha: u8) -> Option<Pixmap> {
    let (m, bmp) = font.rasterize(ch, size);
    if bmp.is_empty() || m.width == 0 || m.height == 0 {
        return None;
    }
    // generous canvas: glyph + halo spread
    let pad = 2usize;
    let (w, h) = (m.width + pad * 2, m.height + pad * 2);
    let mut pm = Pixmap::new(w as u32, h as u32)?;
    {
        let data = pm.data_mut();
        for row in 0..m.height {
            for cx in 0..m.width {
                let cov = bmp[row * m.width + cx] as u32 * alpha as u32 / 255;
                if cov == 0 {
                    continue;
                }
                let i = ((row + pad) * w + cx + pad) * 4;
                data[i] = col.0;
                data[i + 1] = col.1;
                data[i + 2] = col.2;
                data[i + 3] = cov.min(255) as u8;
            }
        }
    }
    Some(pm)
}

/// Text following a polyline (carto text-placement: line). Glyphs read
/// left→right; when the path runs backwards it is walked in reverse so labels
/// stay upright, matching carto's placement flip behaviour.
fn draw_text_along(
    pixmap: &mut Pixmap,
    font: &Font,
    text: &str,
    pts: &[(f32, f32)],
    size: f32,
    color: (u8, u8, u8),
    halo: Option<(u8, u8, u8)>,
    clip_px: &ClipRect,
) {
    if text.is_empty() || pts.len() < 2 {
        return;
    }
    // per-glyph advances
    let mut adv = Vec::with_capacity(text.chars().count());
    let mut total_w = 0f32;
    for ch in text.chars() {
        let (m, _) = font.rasterize(ch, size);
        adv.push(m.advance_width);
        total_w += m.advance_width;
    }
    if total_w < 2.0 {
        return;
    }

    // cumulative segments (unit direction + length)
    let mut segs: Vec<(f32, f32, f32, f32, f32)> = Vec::new();
    let mut total_len = 0f32;
    for w in pts.windows(2) {
        let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
        let len = (dx * dx + dy * dy).sqrt();
        if len > 0.01 {
            segs.push((w[0].0, w[0].1, dx / len, dy / len, len));
            total_len += len;
        }
    }
    if segs.is_empty() || total_len < total_w * 0.8 {
        return; // line too short for carto-style placement
    }

    // Flip so text reads left→right (screen x grows).
    if segs.iter().map(|s| s.2 * s.4).sum::<f32>() < 0.0 {
        segs.reverse();
        for s in &mut segs {
            s.2 = -s.2;
            s.3 = -s.3;
        }
    }

    let at = |dist: f32| -> (f32, f32, f32, f32) {
        let mut acc = 0f32;
        for s in &segs {
            if dist <= acc + s.4 {
                let t = (dist - acc).min(s.4);
                return (s.0 + s.2 * t, s.1 + s.3 * t, s.2, s.3);
            }
            acc += s.4;
        }
        let s = segs[segs.len() - 1];
        (s.0 + s.2 * s.4, s.1 + s.3 * s.4, s.2, s.3)
    };
    let blit = |pixmap: &mut Pixmap, g: &Pixmap, px: f32, py: f32, angle: f32| {
        let (gw, gh) = (g.width() as f32, g.height() as f32);
        let sin = angle.sin();
        let cos = angle.cos();
        // translate(px,py) * rotate(angle) * translate(-center) — tiny_skia's
        // from_row is [sx kx tx / ky sy ty], rotation is (cos,-sin,sin,cos).
        let t = Transform::from_row(cos, -sin, sin, cos, px, py)
            .post_concat(Transform::from_translate(-gw / 2.0, -gh / 2.0));
        pixmap.draw_pixmap(0, 0, g.as_ref(), &tiny_skia::PixmapPaint::default(), t, None);
    };

    let mut d = (total_len / 2.0 - total_w / 2.0).max(2.0);
    let end = total_len - 2.0;
    for (i, ch) in text.chars().enumerate() {
        if d > end {
            break;
        }
        let (px, py, ux, uy) = at(d);
        let angle = uy.atan2(ux);
        if let Some(hc) = halo {
            if let Some(g) = glyph_pixmap(font, ch, size, hc, 190) {
                for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
                    blit(pixmap, &g, px + dx, py + dy, angle);
                }
            }
        }
        if let Some(g) = glyph_pixmap(font, ch, size, color, 255) {
            blit(pixmap, &g, px, py, angle);
        }
        d += adv[i];
    }
    let _ = clip_px;
}

/// A road ref shield: rounded plate in the class shield colour + white ref.
fn draw_shield(pixmap: &mut Pixmap, font: &Font, refs: &str, cx: f32, cy: f32, bg: (u8, u8, u8), clip_px: &ClipRect) {
    if refs.is_empty() {
        return;
    }
    let mut tw = 0f32;
    for ch in refs.chars() {
        let (m, _) = font.rasterize(ch, 9.0);
        tw += m.advance_width;
    }
    let (w, h) = (tw + 7.0, 13.0);
    let rect = tiny_skia::Rect::from_xywh(cx - w / 2.0, cy - h / 2.0, w, h);
    let mut pb = PathBuilder::new();
    if let Some(rc) = rect {
        pb.push_rect(rc);
    }
    if let Some(path) = pb.finish() {
        let paint = Paint { shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(bg.0, bg.1, bg.2, 245)), anti_alias: true, ..Default::default() };
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        let ring = Paint { shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(255, 255, 255, 180)), anti_alias: true, ..Default::default() };
        pixmap.stroke_path(&path, &ring, &Stroke { width: 0.8, ..Default::default() }, Transform::identity(), None);
    }
    draw_text(pixmap, font, refs, cx, cy + 3.2, 9.0, (255, 255, 255), None, clip_px);
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

/// Landcover kind -> palette variable (landcover.mss + amenity area fills).
fn landuse_color_var(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "forest" => "forest",
        "farmland" => "farmland",
        "farmyard" => "farmyard",
        "farm" => "farmyard",
        "grass" | "meadow" | "village_green" | "garden" | "recreation_ground"
        | "pitch" | "playground" | "golf_course" | "mini_golf" | "dog_park" | "common" => "grass",
        "park" => "park",
        "orchard" | "vineyard" | "plant_nursery" => "orchard",
        "allotments" => "allotments",
        "scrub" | "heath" => "scrub",
        "wetland" => "wetland",
        "residential" | "garage" | "garages" => "residential",
        "retail" | "marketplace" => "retail",
        "commercial" => "commercial",
        "industrial" | "railway" | "port" => "industrial",
        "school" => "school",
        "hospital" => "hospital",
        "college" | "university" => "college",
        "place_of_worship" => "religion",
        "cemetery" | "grave_yard" | "christian" | "churchyard" => "cemetery",
        "quarry" => "quarry",
        "construction" => "construction",
        "parking" | "parking_space" => "pedestrian-fill",
        "platform" => "platform",
        "sand" | "beach" | "shingle" => "sand",
        "bare_rock" | "scree" => "bare_rock",
        "mud" => "mud",
        "marina" => "marina",
        "quay" | "wharf" => "quay",
        _ => return None,
    })
}

/// waterway class -> (min_zoom, width ladder). carto water-lines zooms.
fn waterway_style(class: u8, z: u8) -> Option<f32> {
    let width = match class {
        carto_core::WATER_RIVER => match z {
            0..=4 => return None,
            5..=6 => 0.6,
            7 => 0.9,
            8..=9 => 1.4,
            10..=11 => 2.0,
            12..=13 => 2.8,
            14..=15 => 3.8,
            16..=17 => 5.5,
            _ => 7.5,
        },
        carto_core::WATER_CANAL => if z < 8 { return None } else { match z {
            8..=9 => 1.2,
            10..=11 => 1.8,
            12..=13 => 2.6,
            14..=15 => 3.4,
            16..=17 => 4.8,
            _ => 6.5,
        }},
        carto_core::WATER_STREAM => if z < 12 { return None } else { match z {
            12..=13 => 1.2,
            14 => 1.7,
            15..=16 => 2.2,
            _ => 3.0,
        }},
        carto_core::WATER_DITCH | carto_core::WATER_DRAIN => if z < 13 { return None } else {
            if z < 15 { 0.9 } else { 1.6 }
        },
        _ => if z < 13 { return None } else { 0.9 },
    };
    Some(width)
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
    let Some(path) = ring_path(ring_px) else { return };
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
    let water = app.palette.get("water-color").unwrap_or(carto_style::Rgba::rgb(0xaa, 0xd3, 0xdf));
    let gray_land = vis.color("islands", land);

    /* ---- ocean (coastline rings closed against the extract bbox) ---- */
    if vis.on_with_default("ocean", 0, 19, z) {
        let oa = vis.alpha("ocean");
        let oc = vis.color("ocean", water);
        for a in &store.ocean {
            if in_tile(&a.bbox) {
                let ring = line_px(&a.pts);
                if ring.len() >= 3 {
                    draw_polygon(&mut pixmap, &ring, skia_color(oc, oa), &clip);
                }
            }
        }
    }

    /* ---- islands (closed coastline rings -> land over the ocean) ---- */
    if vis.on_with_default("islands", 0, 19, z) {
        let ia = vis.alpha("islands");
        for a in &store.islands {
            if in_tile(&a.bbox) {
                let ring = line_px(&a.pts);
                if ring.len() >= 3 {
                    draw_polygon(&mut pixmap, &ring, skia_color(gray_land, ia), &clip);
                }
            }
        }
    }

    /* ---- landuse polygons (landcover.mss; majors appear below z8) ---- */
    if vis.on_with_default("landuse", 4, 19, z) {
        let la = vis.alpha("landuse");
        for a in &store.landuse {
            if !in_tile(&a.bbox) {
                continue;
            }
            let is_major = matches!(a.kind.as_str(), "forest" | "farmland" | "scrub" | "grass" | "residential" | "industrial" | "sand" | "wetland" | "heath" | "bare_rock");
            if z < 10 && !is_major {
                continue;
            }
            let Some(var) = landuse_color_var(&a.kind) else { continue };
            let Some(c) = app.palette.get(var) else { continue };
            let c = vis.color("landuse", c);
            let ring = line_px(&a.pts);
            if ring.len() >= 3 {
                draw_polygon(&mut pixmap, &ring, skia_color(c, la), &clip);
            }
        }
    }

    /* ---- landcover area symbols (pattern fills) ---- */
    if vis.on_with_default("landcover-patterns", 9, 19, z) && z >= 9 {
        let pa = vis.alpha("landcover-patterns");
        for a in &store.landuse {
            if z < 10 && !matches!(a.kind.as_str(), "forest") {
                continue;
            }
            let Some(pat) = sprites::landcover_pattern(&a.kind) else { continue };
            let Some(base) = landuse_color_var(&a.kind).and_then(|v| app.palette.get(v)) else { continue };
            let base = vis.color("landcover-patterns", base);
            if let Some(tile) = app.sprites.pattern(pat) {
                if in_tile(&a.bbox) {
                    let ring = line_px(&a.pts);
                    if ring.len() >= 3 {
                        draw_polygon_pattern(&mut pixmap, &ring, skia_color(base, 1.0), &tile, pa, &clip);
                    }
                }
            }
        }
    }

    /* ---- protected areas (nature reserves) ---- */
    if vis.on_with_default("protected-areas", 9, 19, z) && z >= 9 {
        let pa = vis.alpha("protected-areas");
        let pc = app.palette.get("protected-areas").unwrap_or(carto_style::Rgba::rgba(0x9c, 0xd4, 0x9c, 255));
        let pc = vis.color("protected-areas", pc);
        for a in &store.protected {
            if in_tile(&a.bbox) {
                let ring = line_px(&a.pts);
                if ring.len() >= 3 {
                    draw_polygon(&mut pixmap, &ring, skia_color(pc, 0.30 * pa), &clip);
                }
            }
        }
    }

    /* ---- highway areas (plazas) under everything road-ish ---- */
    if vis.on_with_default("road-areas", 13, 19, z) && z >= 13 {
        let aa = vis.alpha("road-areas");
        let pf = app.palette.get("pedestrian-fill").unwrap_or(carto_style::Rgba::rgb(0xdd, 0xdd, 0xdd));
        let pf = vis.color("road-areas", pf);
        let cf = app.palette.get("residential-casing").unwrap_or(carto_style::Rgba::rgb(0xbb, 0xbb, 0xbb));
        for a in &store.road_areas {
            if in_tile(&a.bbox) {
                let ring = line_px(&a.pts);
                if ring.len() >= 3 {
                    draw_polygon(&mut pixmap, &ring, skia_color(pf, aa), &clip);
                    draw_polygon(&mut pixmap, &ring, skia_color(cf, 0.8 * aa), &clip);
                }
            }
        }
    }

    /* ---- water areas ---- */
    if vis.on_with_default("water-areas", 7, 19, z) && z >= 7 {
        let wa = vis.alpha("water-areas");
        let wc = vis.color("water-areas", water);
        for a in &store.water_areas {
            if in_tile(&a.bbox) {
                let ring = line_px(&a.pts);
                if ring.len() >= 3 {
                    draw_polygon(&mut pixmap, &ring, skia_color(wc, wa), &clip);
                }
            }
        }
    }

    /* ---- water barriers (dams, weirs, breakwaters) ---- */
    if vis.on_with_default("water-barriers", 13, 19, z) && z >= 13 {
        let ba = vis.alpha("water-barriers");
        let fill = app.palette.get("dam").unwrap_or(carto_style::Rgba::rgb(0xad, 0xad, 0xad));
        let fill = vis.color("water-barriers", fill);
        let line = app.palette.get("dam-line").unwrap_or(carto_style::Rgba::rgb(0x5e, 0x5e, 0x5e));
        for b in &store.water_barriers {
            if !in_tile(&b.bbox) {
                continue;
            }
            let pts = line_px(&b.pts);
            match b.class {
                0 => {
                    if b.area && pts.len() >= 3 {
                        draw_polygon(&mut pixmap, &pts, skia_color(fill, ba), &clip);
                    }
                    draw_line(&mut pixmap, &pts, if z >= 15 { 2.2 } else { 1.6 }, skia_color(line, ba), &clip);
                }
                1 => draw_line(&mut pixmap, &pts, 1.1, skia_color(vis.color("water-barriers", app.palette.get("weir-line").unwrap_or(carto_style::Rgba::rgb(0xaa, 0xaa, 0xaa))), ba), &clip),
                _ => draw_line(&mut pixmap, &pts, 1.4, skia_color(line, ba), &clip),
            }
        }
    }

    /* ---- piers ---- */
    if vis.on_with_default("piers", 12, 19, z) && z >= 12 {
        let pa = vis.alpha("piers");
        let pc = app.palette.get("pier").unwrap_or(carto_style::Rgba::rgb(0xad, 0xad, 0xad));
        let pc = vis.color("piers", pc);
        for p in &store.piers {
            if !in_tile(&p.bbox) {
                continue;
            }
            let pts = line_px(&p.pts);
            if p.area && pts.len() >= 3 {
                draw_polygon(&mut pixmap, &pts, skia_color(pc, pa), &clip);
            } else {
                draw_line(&mut pixmap, &pts, if z >= 15 { 2.4 } else { 1.6 }, skia_color(pc, pa), &clip);
            }
        }
    }

    /* ---- waterways (class ladder; big rivers from z5) ---- */
    if vis.on_with_default("waterways", 5, 19, z) {
        let wa = vis.alpha("waterways");
        let wc = vis.color("waterways", water);
        for wway in &store.waterways {
            let Some(width) = waterway_style(wway.class, z) else { continue };
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

    /* ---- aeroways (runways / taxiways / aprons) ---- */
    if vis.on_with_default("aeroways", 10, 19, z) && z >= 10 {
        let aa = vis.alpha("aeroways");
        let rf = app.palette.get("runway").unwrap_or(carto_style::Rgba::rgb(0xd5, 0xd3, 0xcc));
        let rf = vis.color("aeroways", rf);
        for a in &store.aeroways {
            if !in_tile(&a.bbox) {
                continue;
            }
            let pts = line_px(&a.pts);
            match a.class {
                0 => draw_line(&mut pixmap, &pts, if z >= 13 { 5.5 } else { 3.0 }, skia_color(rf, aa), &clip),
                1 => if z >= 11 {
                    draw_line(&mut pixmap, &pts, if z >= 13 { 3.0 } else { 1.8 }, skia_color(rf, 0.9 * aa), &clip);
                },
                _ => {
                    if a.area && pts.len() >= 3 {
                        draw_polygon(&mut pixmap, &pts, skia_color(rf, aa), &clip);
                    } else {
                        draw_line(&mut pixmap, &pts, 2.0, skia_color(rf, aa), &clip);
                    }
                }
            }
        }
    }

    /* ---- power lines ---- */
    if vis.on_with_default("power-lines", 13, 19, z) && z >= 13 {
        let pa = vis.alpha("power-lines");
        let pc = app.palette.get("power-line-color").unwrap_or(carto_style::Rgba::rgb(0x88, 0x88, 0x88));
        let pc = vis.color("power-lines", pc);
        for p in &store.power {
            if z < 15 && p.class != 0 {
                continue;
            }
            if !in_tile(&p.bbox) {
                continue;
            }
            draw_line(&mut pixmap, &line_px(&p.pts), if p.class == 0 { 1.0 } else { 0.7 }, skia_color(pc, pa), &clip);
        }
    }

    /* ---- roads: casings then fills (tunnels first, bridges last) ---- */
    let casing_for = |class: u8| -> carto_style::Rgba {
        let name = road_class_name(class);
        let var = match name {
            "motorway" | "motorway_link" => "motorway-casing",
            "trunk" | "trunk_link" => "trunk-casing",
            "primary" | "primary_link" => "primary-casing",
            "secondary" | "secondary_link" => "secondary-casing",
            "tertiary" | "tertiary_link" => "tertiary-casing",
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
            "tertiary" | "tertiary_link" => app.palette.get("tertiary-fill").unwrap(),
            "residential" | "unclassified" => app.palette.get("residential-fill").unwrap(),
            "living_street" => app.palette.get("living-street-fill").unwrap(),
            "pedestrian" => app.palette.get("pedestrian-fill").unwrap(),
            "service" => app.palette.get("residential-fill").unwrap(),
            "track" => app.palette.get("track-fill").unwrap(),
            "footway" | "path" | "cycleway" | "steps" => app.palette.get("footway-fill").unwrap(),
            _ => carto_style::Rgba::rgb(0xdd, 0xdd, 0xdd),
        }
    };

    let show_casing = z >= 12 && vis.on_with_default("road-casing", 12, 19, z);
    let casing_alpha = vis.alpha("road-casing");
    let show_fill = vis.on_with_default("road-fill", 5, 19, z);
    let fill_alpha = vis.alpha("road-fill");
    let show_tunnels = vis.is_visible("tunnels");
    let show_bridges = vis.is_visible("bridges");
    let dashes_for = |class: u8| -> Option<Vec<f32>> {
        match road_class_name(class) {
            "footway" | "path" => Some(vec![3.0, 2.4]),
            "steps" => Some(vec![1.6, 1.6]),
            "cycleway" => Some(vec![5.0, 2.4]),
            _ => None,
        }
    };

    // pass 1: tunnels (casing dashed + fill)
    if show_tunnels && z >= 12 {
        for r in &store.roads {
            if !r.tunnel || r.bridge {
                continue;
            }
            let width = road_width(r.class, z);
            if width <= 0.0 || !in_tile(&r.bbox) || z < class_min_zoom(r.class) {
                continue;
            }
            let pts = line_px(&r.pts);
            if show_casing {
                let c = vis.color("road-casing", casing_for(r.class));
                let cw = width + if width >= 4.0 { 2.0 } else { 1.2 };
                draw_line_dashed(&mut pixmap, &pts, cw, skia_color(c, 0.75 * casing_alpha), &[3.0, 2.2], &clip);
            }
            if show_fill {
                let c = vis.color("road-fill", fill_for(app, r.class, z));
                draw_line(&mut pixmap, &pts, width, skia_color(c, fill_alpha), &clip);
            }
        }
    }

    // pass 2: regular roads (skip tunnels & bridges; path classes get dashes)
    if show_casing {
        for r in &store.roads {
            if (r.tunnel && show_tunnels) || (r.bridge && show_bridges) {
                continue;
            }
            let width = road_width(r.class, z);
            if width <= 0.0 || !in_tile(&r.bbox) {
                continue;
            }
            let c = vis.color("road-casing", casing_for(r.class));
            let cw = width + if width >= 4.0 { 2.0 } else { 1.2 };
            draw_line(&mut pixmap, &line_px(&r.pts), cw, skia_color(c, casing_alpha), &clip);
        }
    }
    let mut labeled: Vec<(&Line, Vec<(f32, f32)>)> = Vec::new();
    let mut shielded: Vec<(&Line, f32, f32)> = Vec::new();
    if show_fill {
        for r in &store.roads {
            if (r.tunnel && show_tunnels) || (r.bridge && show_bridges) {
                continue;
            }
            let width = road_width(r.class, z);
            if width <= 0.0 || !in_tile(&r.bbox) || z < class_min_zoom(r.class) {
                continue;
            }
            let pts = line_px(&r.pts);
            let c = vis.color("road-fill", fill_for(app, r.class, z));
            let dash_opt = if z >= 14 && !r.bridge && !r.tunnel { dashes_for(r.class) } else { None };
            match dash_opt {
                Some(dv) => draw_line_dashed(&mut pixmap, &pts, width, skia_color(c, fill_alpha), &dv, &clip),
                None => draw_line(&mut pixmap, &pts, width, skia_color(c, fill_alpha), &clip),
            }
            // ::directions — oneway arrows from z16 (roads.mss), under the labels
            if z >= 16 && r.oneway != 0 && width >= 1.4 && vis.on_with_default("road-oneway", 16, 19, z) {
                if let Some(ac) = oneway_arrow_color(app, vis, r.class) {
                    draw_oneway_arrows(&mut pixmap, &pts, skia_color(ac, fill_alpha), r.oneway == 2);
                }
            }
            let label_cap = 24 + usize::from(z.saturating_sub(14)) * 12;
            if z >= 13 && !r.name.is_empty() && width >= 1.8 && labeled.len() < label_cap {
                labeled.push((r, pts.clone()));
            }
            if z >= 14 && !r.refs.is_empty() && matches!(road_class_name(r.class), "motorway" | "trunk" | "primary" | "secondary" | "motorway_link" | "trunk_link" | "primary_link" | "secondary_link") && shielded.len() < 8 {
                let mid = pts[pts.len() / 2];
                if mid.0 > 8.0 && mid.0 < 248.0 && mid.1 > 8.0 && mid.1 < 248.0 {
                    shielded.push((r, mid.0, mid.1));
                }
            }
        }
    }

    // pass 3: bridges (wider casing + fill over everything)
    if show_bridges && z >= 12 {
        for r in &store.roads {
            if !r.bridge || r.tunnel {
                continue;
            }
            let width = road_width(r.class, z);
            if width <= 0.0 || !in_tile(&r.bbox) || z < class_min_zoom(r.class) {
                continue;
            }
            let pts = line_px(&r.pts);
            if show_casing {
                let c = vis.color("road-casing", casing_for(r.class));
                let cw = width + if width >= 4.0 { 2.6 } else { 1.8 };
                draw_line(&mut pixmap, &pts, cw, skia_color(c, casing_alpha), &clip);
            }
            if show_fill {
                let c = vis.color("road-fill", fill_for(app, r.class, z));
                draw_line(&mut pixmap, &pts, width, skia_color(c, fill_alpha), &clip);
            }
        }
    }

    /* ---- railways (casing + two-tone fill like carto) ---- */
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
                if z >= 13 {
                    // white dashes over the rail = carto's double-rail look
                    draw_line_dashed(&mut pixmap, &line_px(&r.pts), width * 0.55, skia_color(carto_style::Rgba::rgb(0xff, 0xff, 0xff), 0.85 * ra), &[6.0, 6.0], &clip);
                }
            }
        }
    }

    /* ---- ferry routes ---- */
    if vis.on_with_default("ferry", 10, 19, z) && z >= 10 {
        let fa = vis.alpha("ferry");
        let fc = app.palette.get("ferry-route").unwrap_or(carto_style::Rgba::rgb(0x66, 0x66, 0xff));
        let fc = vis.color("ferry", fc);
        for f in &store.ferry {
            if in_tile(&f.bbox) {
                draw_line_dashed(&mut pixmap, &line_px(&f.pts), 1.1, skia_color(fc, fa), &[4.0, 3.0], &clip);
            }
        }
    }

    /* ---- admin boundaries (level ladder, halo + core line) ---- */
    if vis.on_with_default("admin", 4, 19, z) {
        let aa = vis.alpha("admin");
        let core = app.palette.get("admin-boundaries").unwrap_or(carto_style::Rgba::rgb(0x8d, 0x61, 0x8b));
        let core = vis.color("admin", core);
        let wide = app.palette.get("admin-boundaries-wide").unwrap_or(carto_style::Rgba::rgb(0xa3, 0x7d, 0xa1));
        let width = match z {
            4..=6 => 0.9,
            7..=8 => 1.2,
            9..=10 => 1.4,
            11..=12 => 1.7,
            _ => 2.0,
        };
        for a in &store.admin {
            let min = match a.level {
                2 => 4,
                4 => 7,
                _ => 9,
            };
            if z < min || !in_tile(&a.bbox) {
                continue;
            }
            let lvl_scale = if a.level <= 2 { 1.0 } else if a.level == 4 { 0.85 } else { 0.6 };
            let pts = line_px(&a.pts);
            draw_line(&mut pixmap, &pts, width * lvl_scale + 1.6, skia_color(wide, 0.45 * aa), &clip);
            draw_line(&mut pixmap, &pts, width * lvl_scale, skia_color(core, aa), &clip);
        }
    }

    /* ---- barriers / cliffs ---- */
    if vis.on_with_default("fences", 16, 19, z) && z >= 16 {
        let ba = vis.alpha("fences");
        let bc = carto_style::Rgba::rgb(0x6a, 0x6a, 0x6a);
        let bc = vis.color("fences", bc);
        for b in &store.barriers {
            if b.class == 9 || !in_tile(&b.bbox) {
                continue;
            }
            let w = match b.class {
                2 | 4 => 1.1, // city wall / retaining wall
                _ => 0.8,
            };
            draw_line(&mut pixmap, &line_px(&b.pts), w, skia_color(bc, ba), &clip);
        }
    }
    if vis.on_with_default("cliffs", 15, 19, z) && z >= 15 {
        let ca = vis.alpha("cliffs");
        let cc = carto_style::Rgba::rgb(0x70, 0x65, 0x5c);
        let cc = vis.color("cliffs", cc);
        for b in &store.barriers {
            if b.class != 9 || !in_tile(&b.bbox) {
                continue;
            }
            let pts = line_px(&b.pts);
            draw_line(&mut pixmap, &pts, 0.9, skia_color(cc, ca), &clip);
            // hash ticks along the cliff (carto's cliff pattern, simplified)
            for seg in pts.windows(2) {
                let (dx, dy) = (seg[1].0 - seg[0].0, seg[1].1 - seg[0].1);
                let len = (dx * dx + dy * dy).sqrt();
                let steps = (len / 9.0).floor() as usize;
                let (ux, uy) = if len > 0.01 { (dx / len, dy / len) } else { continue };
                for k in 1..=steps {
                    let t = k as f32 * 9.0;
                    let (px, py) = (seg[0].0 + ux * t, seg[0].1 + uy * t);
                    draw_line(&mut pixmap, &[(px, py), (px - uy * 3.2 - ux, py + ux * 3.2 - uy)], 0.8, skia_color(cc, ca), &clip);
                }
            }
        }
    }

    /* ---- tree rows & hedges ---- */
    if vis.on_with_default("tree-rows", 16, 19, z) && z >= 16 {
        let ta = vis.alpha("tree-rows");
        let tc = carto_style::Rgba::rgb(0x6b, 0xa3, 0x68);
        let tc = vis.color("tree-rows", tc);
        for t in &store.tree_rows {
            if in_tile(&t.bbox) {
                draw_line(&mut pixmap, &line_px(&t.pts), 1.2, skia_color(tc, ta), &clip);
            }
        }
    }

    /* ---- golf / tracks / waterslides / roller coasters ---- */
    if vis.on_with_default("misc", 13, 19, z) && z >= 13 {
        let ma = vis.alpha("misc");
        for m in &store.misc {
            if !in_tile(&m.bbox) {
                continue;
            }
            let pts = line_px(&m.pts);
            match m.class {
                0 => draw_line_dashed(&mut pixmap, &pts, 1.0, skia_color(carto_style::Rgba::rgb(0x8f, 0xaf, 0x8f), ma), &[4.0, 3.0], &clip),
                1 => {
                    if m.area && pts.len() >= 3 {
                        draw_polygon(&mut pixmap, &pts, skia_color(carto_style::Rgba::rgb(0xab, 0xdf, 0xc2), 0.8 * ma), &clip);
                    } else {
                        draw_line(&mut pixmap, &pts, 1.6, skia_color(carto_style::Rgba::rgb(0x9b, 0xd0, 0x9b), ma), &clip);
                    }
                }
                2 => draw_line(&mut pixmap, &pts, 1.0, skia_color(carto_style::Rgba::rgb(0x40, 0xcb, 0xa1), ma), &clip),
                _ => draw_line(&mut pixmap, &pts, 1.5, skia_color(carto_style::Rgba::rgb(0xd6, 0x66, 0x66), ma), &clip),
            }
        }
    }

    /* ---- road labels (along-path) + ref shields ---- */
    if z >= 13 && vis.on_with_default("road-labels", 13, 19, z) && !labeled.is_empty() {
        let mut used: HashSet<(u16, u16)> = HashSet::new();
        let size = if z >= 15 { 10.0 } else { 9.0 };
        let fnt: &Font = if z >= 15 { &app.font_bold } else { &app.font };
        let halo = app.palette.get("default-casing").map(|c| (c.r, c.g, c.b)).unwrap_or((255, 255, 255));
        let ink = app.palette.get("road-labels-text").unwrap_or(carto_style::Rgba::rgb(0x33, 0x33, 0x33));
        let ink = vis.color("road-labels", ink);
        for (r, pts) in &labeled {
            let mid = pts[pts.len() / 2];
            let gx = (mid.0 / 48.0) as u16;
            let gy = (mid.1 / 48.0) as u16;
            if !used.insert((gx, gy)) {
                continue;
            }
            if mid.0 < 8.0 || mid.0 > 248.0 || mid.1 < 8.0 || mid.1 > 248.0 {
                continue;
            }
            draw_text_along(&mut pixmap, fnt, &r.name, pts, size, (ink.r, ink.g, ink.b), Some(halo), &clip);
        }
    }
    if z >= 14 && vis.on_with_default("road-refs", 14, 19, z) && !shielded.is_empty() {
        let fnt = &app.font_bold;
        for (r, sx, sy) in &shielded {
            let shield = match road_class_name(r.class) {
                "motorway" | "motorway_link" => app.palette.get("motorway-shield"),
                "trunk" | "trunk_link" => app.palette.get("trunk-shield"),
                "primary" | "primary_link" => app.palette.get("primary-shield"),
                "secondary" | "secondary_link" => app.palette.get("secondary-shield"),
                _ => None,
            }
            .map(|c| (c.r, c.g, c.b))
            .unwrap_or((0x3b, 0x3b, 0x3b));
            draw_shield(&mut pixmap, fnt, &r.refs, *sx, *sy, shield, &clip);
        }
    }

    /* ---- waterway names (river/canal/stream, along path) ---- */
    if z >= 13 && vis.on_with_default("waterways-text", 13, 19, z) {
        let mut count = 0;
        for wway in &store.waterways {
            if count >= 10 {
                break;
            }
            if !matches!(wway.class, carto_core::WATER_RIVER | carto_core::WATER_CANAL | carto_core::WATER_STREAM) {
                continue;
            }
            if wway.class == carto_core::WATER_STREAM && z < 15 {
                continue;
            }
            if wway.name.is_empty() || !in_tile(&wway.bbox) {
                continue;
            }
            let pts = line_px(&wway.pts);
            let mid = pts[pts.len() / 2];
            if mid.0 < 8.0 || mid.0 > 248.0 || mid.1 < 8.0 || mid.1 > 248.0 {
                continue;
            }
            let ink = vis.color("waterways-text", carto_style::Rgba::rgb(0x4d, 0x70, 0x8a));
            draw_text_along(&mut pixmap, &app.font, &wway.name, &pts, 9.5, (ink.r, ink.g, ink.b), Some((255, 255, 255)), &clip);
            count += 1;
        }
    }

    /* ---- building names ---- */
    if z >= 16 && vis.on_with_default("building-text", 16, 19, z) {
        let ink = vis.color("building-text", carto_style::Rgba::rgb(0x6b, 0x66, 0x5f));
        let mut count = 0;
        for b in &store.buildings {
            if count >= 12 {
                break;
            }
            if b.name.is_empty() || !in_tile(&b.bbox) {
                continue;
            }
            let ring = line_px(&b.pts);
            let (sum_x, sum_y, cnt) =
                ring.iter().fold((0.0f32, 0.0f32, 0usize), |acc, p| (acc.0 + p.0, acc.1 + p.1, acc.2 + 1));
            if cnt == 0 {
                continue;
            }
            let (px, py) = (sum_x / cnt as f32, sum_y / cnt as f32);
            if px < 8.0 || px > 248.0 || py < 8.0 || py > 248.0 {
                continue;
            }
            draw_text(&mut pixmap, &app.font, &b.name, px, py + 3.0, 8.5, (ink.r, ink.g, ink.b), Some((255, 255, 255)), &clip);
            count += 1;
        }
    }

    /* ---- POI icons + labels ---- */
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
            if px < 6.0 || px > 250.0 || py < 6.0 || py > 250.0 || count >= 60 {
                continue;
            }
            if z < sprites::poi_min_zoom(&p.category) {
                continue;
            }
            count += 1;
            let has_icon = sprites::poi_icon(&p.category).is_some();
            let icon = sprites::poi_icon(&p.category).and_then(|path| app.sprites.icon(path));
            match icon {
                Some(sprite) => blit_sprite(&mut pixmap, &sprite, px, py, 16.0, pa),
                None => draw_dot(&mut pixmap, px, py, 2.4, skia_color(accent, pa), Some(Color::from_rgba8(255, 255, 255, 220))),
            }
            if z >= 16 {
                let dy = if has_icon { 11.0 } else { -5.0 };
                draw_text(&mut pixmap, &app.font, &p.name, px, py + dy, 9.0, (0x2c, 0x2c, 0x2c), Some((255, 255, 255)), &clip);
            }
        }
    }

    /* ---- place labels (countries .. villages) ---- */
    {
        let halo = app.palette.get("default-casing").map(|c| (c.r, c.g, c.b)).unwrap_or((255, 255, 255));
        let draw_place = |pixmap: &mut Pixmap, p: &carto_core::Place, min_z: u8, size: f32, bold: bool, ink: carto_style::Rgba, upper: bool| {
            if z < min_z {
                return;
            }
            let (lon, lat) = (f64::from(store.lon[p.idx as usize]), f64::from(store.lat[p.idx as usize]));
            if lon < f64::from(tw) || lon > f64::from(te) || lat < f64::from(ts) || lat > f64::from(tn) {
                return;
            }
            let (mx, my) = lonlat_to_merc(lon, lat);
            let (px, py) = ((mx * scale - ox) as f32, (my * scale - oy) as f32);
            if px < 10.0 || px > 246.0 || py < 10.0 || py > 246.0 {
                return;
            }
            let fnt = if bold { &app.font_bold } else { &app.font };
            let name = if upper { p.name.to_uppercase() } else { p.name.clone() };
            let ink = vis.color_of_place(p.class, ink);
            draw_text(pixmap, fnt, &name, px, py, size, (ink.r, ink.g, ink.b), Some(halo), &clip);
        };
        if vis.on_with_default("places-country", 4, 19, z) {
            for p in &store.places {
                if p.class == carto_core::PL_COUNTRY {
                    draw_place(&mut pixmap, p, 4, 11.5, true, carto_style::Rgba::rgb(0x7a, 0x5b, 0x7a), true);
                }
            }
        }
        if vis.on_with_default("places-state", 5, 19, z) {
            for p in &store.places {
                if p.class == carto_core::PL_STATE {
                    draw_place(&mut pixmap, p, 5, 9.5, false, carto_style::Rgba::rgb(0x7a, 0x5b, 0x7a), true);
                }
            }
        }
        if vis.on_with_default("places-city", 8, 19, z) {
            for p in &store.places {
                if p.class == carto_core::PL_CITY {
                    let min = if p.capital { 7 } else { 8 };
                    let size = if p.capital { 12.0 } else { 11.0 };
                    draw_place(&mut pixmap, p, min, size, true, carto_style::Rgba::rgb(0x1a, 0x1a, 0x1a), false);
                }
            }
        }
        if vis.on_with_default("places-town", 10, 19, z) {
            for p in &store.places {
                if p.class == carto_core::PL_TOWN {
                    draw_place(&mut pixmap, p, 10, 10.0, false, carto_style::Rgba::rgb(0x22, 0x22, 0x22), false);
                }
            }
        }
        if vis.on_with_default("places-small", 13, 19, z) {
            for p in &store.places {
                if p.class >= carto_core::PL_VILLAGE {
                    draw_place(&mut pixmap, p, 13, 9.0, false, carto_style::Rgba::rgb(0x55, 0x55, 0x55), false);
                }
            }
        }
    }

    /* ---- point features ---- */
    if z >= 12 {
        let halo = app.palette.get("default-casing").map(|c| (c.r, c.g, c.b)).unwrap_or((255, 255, 255));
        for pt in &store.points {
            let (lon, lat) = (f64::from(store.lon[pt.idx as usize]), f64::from(store.lat[pt.idx as usize]));
            if lon < f64::from(tw) || lon > f64::from(te) || lat < f64::from(ts) || lat > f64::from(tn) {
                continue;
            }
            let (mx, my) = lonlat_to_merc(lon, lat);
            let (px, py) = ((mx * scale - ox) as f32, (my * scale - oy) as f32);
            if px < 4.0 || px > 252.0 || py < 4.0 || py > 252.0 {
                continue;
            }
            match pt.class {
                carto_core::PT_PEAK if vis.on_with_default("peaks", 12, 19, z) => {
                    draw_dot(&mut pixmap, px, py - 3.0, 2.2, Color::from_rgba8(0x6b, 0x59, 0x45, 255), None);
                    let label = if pt.value > 0 { format!("{} {}", pt.name, pt.value) } else { pt.name.clone() };
                    let ink = vis.color("peaks", carto_style::Rgba::rgb(0x50, 0x40, 0x30));
                    draw_text(&mut pixmap, &app.font, &label, px, py + 8.0, 9.0, (ink.r, ink.g, ink.b), Some(halo), &clip);
                }
                carto_core::PT_STATION if vis.on_with_default("stations", 13, 19, z) && z >= 13 => {
                    let sc = vis.color("stations", carto_style::Rgba::rgb(0x79, 0x81, 0xb0));
                    draw_dot(&mut pixmap, px, py, 2.4, skia_color(sc, 1.0), Some(Color::from_rgba8(255, 255, 255, 230)));
                    if !pt.name.is_empty() && z >= 14 {
                        draw_text(&mut pixmap, &app.font_bold, &pt.name, px, py - 6.0, 9.0, (0x44, 0x44, 0x66), Some(halo), &clip);
                    }
                }
                carto_core::PT_JUNCTION if vis.on_with_default("junctions", 13, 19, z) && z >= 13 && !pt.name.is_empty() => {
                    draw_text(&mut pixmap, &app.font_bold, &pt.name, px, py + 3.0, 8.5, (0x33, 0x33, 0x33), Some(halo), &clip);
                }
                carto_core::PT_CROSSING if vis.on_with_default("crossings", 14, 19, z) && z >= 14 => {
                    draw_dot(&mut pixmap, px, py, 1.8, Color::from_rgba8(0x33, 0x33, 0x33, 255), Some(Color::from_rgba8(255, 255, 255, 255)));
                }
                carto_core::PT_TOWER if vis.on_with_default("power-towers", 14, 19, z) && z >= 14 => {
                    if let Some(sp) = app.sprites.icon("man_made/power_tower.svg") {
                        blit_sprite(&mut pixmap, &sp, px, py, 11.0, vis.alpha("power-towers"));
                    }
                }
                carto_core::PT_POLE if vis.on_with_default("power-towers", 16, 19, z) && z >= 16 => {
                    draw_dot(&mut pixmap, px, py, 1.2, Color::from_rgba8(0x88, 0x88, 0x88, 255), None);
                }
                carto_core::PT_TREE if vis.on_with_default("trees", 16, 19, z) && z >= 16 => {
                    draw_dot(&mut pixmap, px, py, 1.6, Color::from_rgba8(0x4d, 0x85, 0x4d, 235), None);
                }
                carto_core::PT_LAMP if vis.on_with_default("lamps", 17, 19, z) && z >= 17 => {
                    draw_dot(&mut pixmap, px, py, 1.1, Color::from_rgba8(0xcc, 0xb4, 0x55, 255), None);
                }
                carto_core::PT_ENTRANCE if vis.on_with_default("entrances", 17, 19, z) && z >= 17 => {
                    draw_dot(&mut pixmap, px, py, 1.4, Color::from_rgba8(0xff, 0xff, 0xff, 255), Some(Color::from_rgba8(0x44, 0x44, 0x44, 255)));
                }
                carto_core::PT_ADDRESS if vis.on_with_default("addresses", 17, 19, z) && z >= 17 && !pt.name.is_empty() => {
                    draw_text(&mut pixmap, &app.font, &pt.name, px, py + 3.0, 8.0, (0x66, 0x66, 0x66), Some(halo), &clip);
                }
                carto_core::PT_SPRING if vis.on_with_default("peaks", 14, 19, z) && z >= 14 => {
                    draw_dot(&mut pixmap, px, py, 1.8, Color::from_rgba8(0x6c, 0xa6, 0xd9, 255), None);
                }
                _ => {}
            }
        }
    }

    Some(pixmap)
}


impl LayerVis {
    /// Colour override keyed by the place bucket the label belongs to, so a
    /// user recolouring "Cities" also recolours capitals.
    fn color_of_place(&self, class: u8, base: carto_style::Rgba) -> carto_style::Rgba {
        let id = match class {
            carto_core::PL_COUNTRY => "places-country",
            carto_core::PL_STATE => "places-state",
            carto_core::PL_CITY => "places-city",
            carto_core::PL_TOWN => "places-town",
            _ => "places-small",
        };
        self.color(id, base)
    }
}

/// PNG-encoded tile (the HTTP server path).

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
                places: store.places.len(),
                ocean: store.ocean.len(),
                islands: store.islands.len(),
                admin: store.admin.len(),
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
        sprites: sprites::Sprites::new(),
        cache: Mutex::new(TileCache::new(768)),
        hits: AtomicUsize::new(0),
        misses: AtomicUsize::new(0),
        region_name: Mutex::new(region),
        data_dir: Mutex::new(stack.data_dir),
    })
}

/// Extra, platform-specific guidance for a listener bind failure — Windows
/// reports reserved port ranges (Hyper-V / WSL / Docker NAT) as WSAEACCES
/// 10013, which looks nothing like "port busy" to most users.
pub fn bind_error_hint(e: &std::io::Error, port: u16) -> String {
    match e.raw_os_error() {
        Some(10013) => format!(
            "bind 0.0.0.0:{port} failed: {e}\n  Windows: port {port} sits in a reserved range (Hyper-V / WSL / Docker NAT) or a service holds it.\n  1) check: netsh interface ipv4 show excludedportrange protocol=tcp\n  2) free it (admin): net stop winnat  &&  net start winnat\n  3) or pick another port: tile-server --port 3150 (or edit PORT in the header)"
        ),
        Some(10048) => format!("bind 0.0.0.0:{port} failed: {e}\n  another tile-server (or app) is already listening on {port} — stop it or use --port"),
        _ => format!("bind 0.0.0.0:{port} failed: {e}"),
    }
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
            eprintln!("{}", bind_error_hint(&e, port));
            1
        }
    }
}
