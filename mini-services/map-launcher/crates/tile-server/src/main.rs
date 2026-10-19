//! map-launcher tile server — pure Rust HTTP daemon on :3100.
//!
//! Routes (consumed by the Next.js preview UI through the gateway):
//!   GET /                     -> service banner JSON
//!   GET /api/health           -> {ok:true}
//!   GET /api/stats            -> registry counts + bbox
//!   GET /api/roads?limit=N    -> roads registry (named first)
//!   GET /api/roads/{id}       -> road geometry (lon/lat points)
//!   GET /api/pois?limit=N     -> POI registry
//!   GET /api/palette          -> parsed openstreetmap-carto palette
//!   GET /tile/{z}/{x}/{y}.png -> server-side carto-rendered PNG tile
//!
//! Also hosts the `extract` subcommand building the Central-Tanzania store.

use carto_core::{road_class_name, Line, Store};
use carto_math::{lonlat_to_merc, tile_bbox, ClipRect};
use carto_style::{Palette, SymbolRegistry};
use fontdue::{Font, FontSettings};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

/* --------------------------------------------------------------------- */
/* Application state                                                       */
/* --------------------------------------------------------------------- */

type TileKey = (u8, u32, u32);

struct TileCache {
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
}

struct App {
    store: Store,
    palette: Palette,
    font: Font,
    font_bold: Font,
    #[allow(dead_code)]
    symbol_count: usize,
    cache: Mutex<TileCache>,
    hits: AtomicUsize,
    misses: AtomicUsize,
}

/* --------------------------------------------------------------------- */
/* JSON DTOs (frontend contract)                                           */
/* --------------------------------------------------------------------- */

#[derive(Serialize)]
struct StatsDto {
    roads: usize,
    named_roads: usize,
    railways: usize,
    waterways: usize,
    water_areas: usize,
    landuse: usize,
    buildings: usize,
    pois: usize,
    bounds: [f64; 4],
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
        9 | 10 => 12,   // residential, unclassified
        11 | 12 => 13,  // living_street, pedestrian
        13 => 14, // service
        14 => 13, // track
        15 | 16 | 17 => 14, // footway, path, cycleway
        18 => 15, // steps
        _ => 9,   // rail
    }
}

/// Stroke width in pixels per class/zoom (roads.mss inspired ramp).
/// Index 0 = z7, last = z19; clamped at both ends so deep zooms keep widening.
fn road_width(class: u8, z: u8) -> f32 {
    let t = |z: u8, vals: [f32; 13]| -> f32 {
        let i = (z.saturating_sub(7) as usize).min(12);
        vals[i]
    };
    match class {
        0 => t(z, [0.0, 0.0, 1.5, 2.0, 2.4, 2.8, 3.5, 5.0, 6.5, 9.0, 11.0, 13.5, 16.0]),
        2 => t(z, [0.0, 0.0, 1.0, 1.6, 2.0, 2.4, 3.0, 4.4, 5.5, 8.0, 9.5, 11.5, 14.0]),
        4 => t(z, [0.0, 0.0, 0.0, 1.2, 1.6, 2.0, 2.6, 3.8, 5.0, 7.0, 8.5, 10.5, 12.5]),
        6 => t(z, [0.0, 0.0, 0.0, 0.0, 1.2, 1.6, 2.2, 3.2, 4.2, 6.0, 7.5, 9.0, 11.0]),
        8 => t(z, [0.0, 0.0, 0.0, 0.0, 0.0, 1.2, 2.0, 2.8, 3.6, 5.5, 6.5, 8.0, 9.5]),
        9 | 10 => t(z, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.8, 2.4, 3.2, 4.8, 5.8, 7.0, 8.5]),
        11 | 12 => t(z, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.2, 3.0, 4.5, 5.5, 6.5, 8.0]),
        13 => t(z, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        14 => t(z, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.8, 2.2, 3.2, 4.0, 4.8, 5.8]),
        15 | 16 | 17 => t(z, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.6, 2.4, 3.0, 3.6, 4.4]),
        18 => t(z, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.8, 2.4, 3.0, 3.6]),
        _ => t(z, [0.0, 0.0, 0.0, 1.0, 1.2, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0]), // rail
    }
}

/// One oneway direction marker (openstreetmap-carto `symbols/oneway.svg`:
/// a small triangle pointing along the way) drawn with tiny-skia.
fn draw_arrow_marker(pixmap: &mut Pixmap, x: f32, y: f32, ux: f32, uy: f32, color: Color) {
    let (tip_x, tip_y) = (x + ux * 4.5, y + uy * 4.5);
    let (base_x, base_y) = (x - ux * 3.5, y - uy * 3.5);
    let (ppx, ppy) = (-uy, ux); // perpendicular
    let mut pb = PathBuilder::new();
    pb.move_to(tip_x, tip_y);
    pb.line_to(base_x + ppx * 3.4, base_y + ppy * 3.4);
    pb.line_to(base_x - ppx * 3.4, base_y - ppy * 3.4);
    pb.close();
    if let Some(path) = pb.finish() {
        let paint = Paint { shader: tiny_skia::Shader::SolidColor(color), anti_alias: true, ..Default::default() };
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

/// oneway arrows every 180px along the road (roads.mss `::directions`,
/// marker-placement: line / marker-spacing: 180; flipped for oneway = -1).
fn draw_oneway_arrows(
    pixmap: &mut Pixmap,
    pts_px: &[(f32, f32)],
    color: Color,
    reverse: bool,
) {
    const SPACING: f32 = 180.0;
    let dir = f64::from(if reverse { -1.0 } else { 1.0 });
    let mut next = SPACING / 2.0;
    let mut acc = 0.0f32;
    let mut placed = 0usize;
    for w in pts_px.windows(2) {
        let (x1, y1) = w[0];
        let (x2, y2) = w[1];
        let (dx, dy) = (x2 - x1, y2 - y1);
        let seg = (dx * dx + dy * dy).sqrt();
        if seg < 1e-6 {
            continue;
        }
        while next <= acc + seg {
            let t = (next - acc) / seg;
            let (px, py) = (x1 + dx * t, y1 + dy * t);
            if (-8.0..=264.0).contains(&px) && (-8.0..=264.0).contains(&py) {
                let (ux, uy) = (dx / seg * dir as f32, dy / seg * dir as f32);
                draw_arrow_marker(pixmap, px, py, ux, uy, color);
                placed += 1;
                if placed >= 24 {
                    return;
                }
            }
            next += SPACING;
        }
        acc += seg;
    }
}

/// Per-class oneway arrow colour: roads.mss `@*-oneway-arrow-color`
/// (darken(casing, 10-40%) per class); None = class gets no arrows.
fn oneway_arrow_color(app: &App, class: u8) -> Option<carto_style::Rgba> {
    let (var, pct) = match road_class_name(class) {
        "motorway" | "motorway_link" => ("motorway-casing", 25.0),
        "trunk" | "trunk_link" => ("trunk-casing", 25.0),
        "primary" | "primary_link" => ("primary-casing", 15.0),
        "secondary" | "secondary_link" => ("secondary-casing", 10.0),
        "tertiary" => ("tertiary-casing", 30.0),
        "residential" | "unclassified" | "service" => ("residential-casing", 40.0),
        "living_street" => ("residential-casing", 30.0),
        "pedestrian" => ("pedestrian-casing", 25.0),
        _ => return None,
    };
    app.palette.get(var).map(|c| carto_style::darken(c, pct))
}

fn skia_color(c: carto_style::Rgba, alpha_mul: f32) -> Color {
    Color::from_rgba(
        f32::from(c.r) / 255.0,
        f32::from(c.g) / 255.0,
        f32::from(c.b) / 255.0,
        (f32::from(c.a) / 255.0 * alpha_mul).clamp(0.0, 1.0),
    )
    .unwrap_or(Color::from_rgba8(0, 0, 0, 255))
}

fn draw_line(
    pixmap: &mut Pixmap,
    pts_px: &[(f32, f32)],
    width: f32,
    color: Color,
    clip_px: &ClipRect,
) {
    if pts_px.len() < 2 || width <= 0.0 {
        return;
    }
    let pad = width + 2.0;
    let _ = pad;
    let mut pb = PathBuilder::new();
    let mut drawing = false;
    for w in pts_px.windows(2) {
        if let Some((x1, y1, x2, y2)) = clip_px.clip_line(
            f64::from(w[0].0),
            f64::from(w[0].1),
            f64::from(w[1].0),
            f64::from(w[1].1),
        ) {
            if !drawing {
                pb.move_to(x1 as f32, y1 as f32);
                drawing = true;
            }
            pb.line_to(x2 as f32, y2 as f32);
        } else {
            drawing = false;
        }
    }
    if let Some(path) = pb.finish() {
        let paint = Paint { shader: tiny_skia::Shader::SolidColor(color), anti_alias: true, ..Default::default() };
        let st = Stroke { width: width.max(0.4), line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
        pixmap.stroke_path(&path, &paint, &st, Transform::identity(), None);
    }
}

fn draw_polygon(pixmap: &mut Pixmap, ring_px: &[(f32, f32)], color: Color, clip_px: &ClipRect) {
    if ring_px.len() < 3 {
        return;
    }
    let flat: Vec<f64> = ring_px.iter().flat_map(|p| [f64::from(p.0), f64::from(p.1)]).collect();
    let clipped = clip_px.clip_polygon(&flat);
    if clipped.len() < 6 {
        return;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(clipped[0] as f32, clipped[1] as f32);
    for v in clipped[2..].chunks(2) {
        pb.line_to(v[0] as f32, v[1] as f32);
    }
    pb.close();
    if let Some(path) = pb.finish() {
        let paint = Paint { shader: tiny_skia::Shader::SolidColor(color), anti_alias: true, ..Default::default() };
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
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

/// Render one 256px carto tile to PNG bytes.
fn render_tile(app: &App, z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    let mut pixmap = Pixmap::new(256, 256)?;
    let land = app.palette.get("land-color").unwrap_or(carto_style::Rgba::rgb(0xf2, 0xef, 0xe9));
    pixmap.fill(skia_color(land, 1.0));

    let [w, s, e, n] = tile_bbox(z, x, y);
    let (tw, ts, te, tn) = (w as f32, s as f32, e as f32, n as f32);
    let clip = ClipRect::new(-40.0, -40.0, 296.0, 296.0);

    // lon/lat -> tile-local px
    let scale = carto_math::world_px(z);
    let ox = f64::from(x) * 256.0;
    let oy = f64::from(y) * 256.0;
    let store = &app.store;
    let project = |i: u32| -> (f32, f32) {
        let (mx, my) = lonlat_to_merc(f64::from(store.lon[i as usize]), f64::from(store.lat[i as usize]));
        ((mx * scale - ox) as f32, (my * scale - oy) as f32)
    };
    let in_tile = |b: &[f32; 4]| b[2] >= tw && b[0] <= te && b[3] >= ts && b[1] <= tn;

    let line_px = |pts: &Vec<u32>| -> Vec<(f32, f32)> { pts.iter().map(|&i| project(i)).collect() };

    /* ---- landuse polygons ---- */
    let lu_colors: &[(&str, &str)] = &[        ("forest", "forest"),
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
        let ring = line_px(&a.pts);
        if ring.len() >= 3 {
            draw_polygon(&mut pixmap, &ring, skia_color(c, 1.0), &clip);
        }
    }

    /* ---- water areas ---- */
    if z >= 7 {
        let wc = app.palette.get("water-color").unwrap_or(carto_style::Rgba::rgb(0xaa, 0xd3, 0xdf));
        for a in &store.water_areas {
            if in_tile(&a.bbox) {
                let ring = line_px(&a.pts);
                if ring.len() >= 3 {
                    draw_polygon(&mut pixmap, &ring, skia_color(wc, 1.0), &clip);
                }
            }
        }
    }

    /* ---- waterways ---- */
    if z >= 8 {
        let wc = app.palette.get("water-color").unwrap_or(carto_style::Rgba::rgb(0xaa, 0xd3, 0xdf));
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
            draw_line(&mut pixmap, &line_px(&wway.pts), width, skia_color(wc, 1.0), &clip);
        }
    }

    /* ---- buildings ---- */
    if z >= 13 {
        let bf = app.palette.get("building-fill").unwrap_or(carto_style::Rgba::rgb(0xd9, 0xd0, 0xc9));
        for b in &store.buildings {
            if in_tile(&b.bbox) {
                let ring = line_px(&b.pts);
                if ring.len() >= 3 {
                    draw_polygon(&mut pixmap, &ring, skia_color(bf, 1.0), &clip);
                }
            }
        }
        if z >= 15 {
            let bl = app.palette.get("building-line").unwrap_or(carto_style::Rgba::rgb(0xbc, 0xb3, 0xaa));
            for b in &store.buildings {
                if in_tile(&b.bbox) {
                    let ring = line_px(&b.pts);
                    if ring.len() >= 3 {
                        draw_polygon(&mut pixmap, &ring, skia_color(bl, 0.6), &clip);
                    }
                }
            }
        }
    }

    /* ---- railways (casing) ---- */
    if z >= 9 {
        let rc = carto_style::Rgba::rgb(0x8a, 0x8a, 0x8a);
        let width = match z {
            9 | 10 => 0.9,
            11 | 12 => 1.4,
            13 | 14 => 2.0,
            _ => 2.6,
        };
        for r in &store.railways {
            if in_tile(&r.bbox) {
                draw_line(&mut pixmap, &line_px(&r.pts), width, skia_color(rc, 1.0), &clip);
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
    let show_casing = z >= 12;
    // casings
    if show_casing {
        for r in &store.roads {
            let width = road_width(r.class, z);
            if width <= 0.0 || !in_tile(&r.bbox) {
                continue;
            }
            let c = casing_for(r.class);
            let cw = width + if width >= 4.0 { 2.0 } else { 1.2 };
            draw_line(&mut pixmap, &line_px(&r.pts), cw, skia_color(c, 1.0), &clip);
        }
    }
    // fills
    for r in &store.roads {
        let width = road_width(r.class, z);
        if width <= 0.0 || !in_tile(&r.bbox) || z < class_min_zoom(r.class) {
            continue;
        }
        let pts = line_px(&r.pts);
        let c = fill_for(app, r.class, z);
        draw_line(&mut pixmap, &pts, width, skia_color(c, 1.0), &clip);
        // ::directions — oneway arrows from z16 (roads.mss), under the labels
        if z >= 16 && r.oneway != 0 && width >= 1.4 {
            if let Some(ac) = oneway_arrow_color(app, r.class) {
                draw_oneway_arrows(&mut pixmap, &pts, skia_color(ac, 1.0), r.oneway == 2);
            }
        }
        let label_cap = 24 + usize::from(z.saturating_sub(14)) * 12;
        if z >= 13 && !r.name.is_empty() && width >= 1.8 && labeled.len() < label_cap {
            labeled.push((r, pts));
        }
    }

    /* ---- road labels ---- */
    if z >= 13 && !labeled.is_empty() {
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
            draw_text(
                &mut pixmap,
                fnt,
                &r.name,
                mid.0,
                mid.1 + size / 3.0,
                size,
                shield(r.class),
                Some(halo),
                &clip,
            );
        }
    }

    /* ---- POI dots + labels ---- */
    if z >= 14 {
        let accent = app.palette.get("motorway-casing").unwrap_or(carto_style::Rgba::rgb(0xdc, 0x2a, 0x67));
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
                let paint = Paint { shader: tiny_skia::Shader::SolidColor(skia_color(accent, 1.0)), anti_alias: true, ..Default::default() };
                pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
                let paint_w = Paint { shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(255, 255, 255, 220)), anti_alias: true, ..Default::default() };
                let mut pbc = PathBuilder::new();
                pbc.push_circle(px, py, 2.4);
                if let Some(ring) = pbc.finish() {
                    pixmap.stroke_path(&ring, &paint_w, &Stroke { width: 1.0, ..Default::default() }, Transform::identity(), None);
                }
            }
            if z >= 16 {
                draw_text(
                    &mut pixmap,
                    &app.font,
                    &p.name,
                    px,
                    py - 5.0,
                    9.0,
                    (0x2c, 0x2c, 0x2c),
                    Some((255, 255, 255)),
                    &clip,
                );
            }
        }
    }

    pixmap.encode_png().ok().map(|v| v.to_vec())
}

/* --------------------------------------------------------------------- */
/* HTTP plumbing                                                           */
/* --------------------------------------------------------------------- */

struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    cache_hdr: Option<&'static str>,
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
        resp.status,
        reason,
        resp.content_type,
        resp.body.len()
    );
    if let Some(c) = resp.cache_hdr {
        head.push_str(&format!("Cache-Control: {c}\r\n"));
    }
    head.push_str(if keep_alive {
        "Connection: keep-alive\r\n\r\n"
    } else {
        "Connection: close\r\n\r\n"
    });
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
        let qp: HashMap<&str, &str> = query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .collect();
        let keep_alive = version != "HTTP/1.0"
            && !qp.get("Connection").map(|c| c.eq_ignore_ascii_case("close")).unwrap_or(false);

        let resp = route(&app, &path, &qp);
        if write_response(&mut writer, &resp, keep_alive).is_err() {
            return;
        }
        if !keep_alive {
            return;
        }
    }
}

fn route(app: &App, path: &str, qp: &HashMap<&str, &str>) -> Response {
    match path {
        "/" => Response::json(
            200,
            "{\"ok\":true,\"service\":\"map-launcher tile-server\",\"version\":\"0.3.0\"}".to_string(),
        ),
        "/api/health" => Response::json(200, "{\"ok\":true}".to_string()),
        "/api/stats" => {
            let named = app.store.roads.iter().filter(|r| !r.name.is_empty()).count();
            let dto = StatsDto {
                roads: app.store.roads.len(),
                named_roads: named,
                railways: app.store.railways.len(),
                waterways: app.store.waterways.len(),
                water_areas: app.store.water_areas.len(),
                landuse: app.store.landuse.len(),
                buildings: app.store.buildings.len(),
                pois: app.store.pois.len(),
                bounds: app.store.bbox,
            };
            Response::json(200, serde_json::to_string(&dto).unwrap_or_default())
        }
        "/api/roads" => {
            let limit: usize = qp.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50_000).min(200_000);
            let roads: Vec<RoadSummaryDto> =
                app.store.roads.iter().take(limit).map(road_line_to_dto).collect();
            let dto = RoadsRespDto { count: roads.len(), roads };
            Response::json(200, serde_json::to_string(&dto).unwrap_or_default())
        }
        "/api/pois" => {
            let limit: usize = qp.get("limit").and_then(|v| v.parse().ok()).unwrap_or(20_000).min(100_000);
            let pois: Vec<PoiDto> = app
                .store
                .pois
                .iter()
                .take(limit)
                .map(|p| PoiDto {
                    id: p.id,
                    name: p.name.clone(),
                    category: p.category.clone(),
                    lat: f64::from(app.store.lat[p.idx as usize]),
                    lon: f64::from(app.store.lon[p.idx as usize]),
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
            let dto = PaletteRespDto {
                var_count: app.palette.var_count(),
                color_count: app.palette.color_count,
                roles,
            };
            Response::json(200, serde_json::to_string(&dto).unwrap_or_default())
        }
        _ => {
            if let Some(rest) = path.strip_prefix("/api/roads/") {
                let id: i64 = match rest.parse() {
                    Ok(v) => v,
                    Err(_) => return Response::not_found("invalid road id"),
                };
                let r = app.store.roads.iter().find(|r| r.id == id);
                return match r {
                    Some(r) => {
                        let points: Vec<[f64; 2]> = r
                            .pts
                            .iter()
                            .map(|&i| {
                                [
                                    f64::from(app.store.lon[i as usize]),
                                    f64::from(app.store.lat[i as usize]),
                                ]
                            })
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
/* CLI                                                                     */
/* --------------------------------------------------------------------- */

fn abs_root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && args[1] == "extract" {
        // tile-server extract <pbf> <outdir> [w s e n]
        if args.len() < 4 {
            eprintln!("usage: tile-server extract <pbf> <outdir> [w s e n]");
            std::process::exit(2);
        }
        let pbf = PathBuf::from(&args[2]);
        let out = PathBuf::from(&args[3]);
        let bbox = if args.len() >= 8 {
            [
                args[4].parse().unwrap(),
                args[5].parse().unwrap(),
                args[6].parse().unwrap(),
                args[7].parse().unwrap(),
            ]
        } else {
            carto_core::DODOMA_BBOX
        };
        eprintln!("extracting {pbf:?} bbox={bbox:?} -> {out:?}");
        match carto_core::extract(&pbf, &out, bbox) {
            Ok(store) => {
                eprintln!(
                    "done: {} roads / {} buildings / {} pois",
                    store.roads.len(),
                    store.buildings.len(),
                    store.pois.len()
                );
            }
            Err(e) => {
                eprintln!("extract failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // serve
    let mut port: u16 = 3100;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" if i + 1 < args.len() => {
                port = args[i + 1].parse().unwrap_or(3100);
                i += 2;
            }
            _ => i += 1,
        }
    }

    let root = abs_root();
    let assets = root.join("assets");
    let data_dir = root.join("data");

    let palette = Palette::from_dir(assets.join("style")).unwrap_or_else(|e| {
        eprintln!("palette load failed ({e}) — using empty palette");
        Palette { decls: Vec::new(), vars: HashMap::new(), roles: Vec::new(), color_count: 0 }
    });
    eprintln!(
        "palette: {} vars, {} colors, {} roles, {} symbols",
        palette.var_count(),
        palette.color_count,
        palette.roles.len(),
        SymbolRegistry::scan(assets.join("symbols")).svg_count
    );

    // Fonts: craft-launcher-style loader — system scan (Noto Sans first, the
    // openstreetmap-carto fonts.mss choice), bundled assets/fonts fallback.
    let regular_path = carto_style::load_map_font(false)
        .or_else(|| carto_style::find_font("dejavu", false))
        .expect("no map font found (system + assets/fonts)");
    let bold_path = carto_style::load_map_font(true)
        .or_else(|| carto_style::find_font("dejavu", true))
        .expect("no bold map font found (system + assets/fonts)");
    eprintln!("fonts: regular={} bold={}", regular_path.display(), bold_path.display());
    let font_bytes = std::fs::read(&regular_path).expect("read regular font");
    let font_bold_bytes = std::fs::read(&bold_path).expect("read bold font");
    let font = Font::from_bytes(font_bytes, FontSettings { scale: 12.0, ..Default::default() })
        .expect("font parse failed");
    let font_bold = Font::from_bytes(font_bold_bytes, FontSettings { scale: 12.0, ..Default::default() })
        .expect("font parse failed");

    let store = Store::load(&data_dir).unwrap_or_else(|e| {
        eprintln!("store load failed ({e}) — empty registry (run: tile-server extract <pbf> data)");
        Store::empty(carto_core::DODOMA_BBOX)
    });
    eprintln!(
        "registry: {} roads ({} named), {} buildings, {} pois, {} landuse, {} waterways",
        store.roads.len(),
        store.roads.iter().filter(|r| !r.name.is_empty()).count(),
        store.buildings.len(),
        store.pois.len(),
        store.landuse.len(),
        store.waterways.len()
    );

    let app = Arc::new(App {
        store,
        palette,
        font,
        font_bold,
        symbol_count: 0,
        cache: Mutex::new(TileCache::new(768)),
        hits: AtomicUsize::new(0),
        misses: AtomicUsize::new(0),
    });

    let listener = TcpListener::bind(("0.0.0.0", port)).expect("bind :3100");
    eprintln!("map-launcher tile-server 0.3.0 listening on :{port}");
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let app = app.clone();
                std::thread::spawn(move || handle_client(stream, app));
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}
