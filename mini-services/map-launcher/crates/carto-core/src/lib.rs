//! Tanzania-scale OSM registry: data model, PBF extraction and bincode
//! persistence for the map-launcher tile server.
//!
//! The extractor consumes a `*.osm.pbf` (Geofabrik Tanzania) with two passes —
//! nodes + relations, then ways — keeps every node inside the extract bbox,
//! resolves all ways to node indices, joins coastline ways into ocean/island
//! polygons and stores every openstreetmap-carto dataset the renderer paints,
//! so the server never needs an id lookup at runtime.

use bincode::{deserialize_from, serialize_into};
use carto_math::haversine_m;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::Path;

/// Central Tanzania (Dodoma region) test-data bbox [w, s, e, n].
pub const DODOMA_BBOX: [f64; 4] = [35.1, -7.2, 36.7, -5.4];

/* --------------------------------------------------------------------- */
/* Data model                                                              */
/* --------------------------------------------------------------------- */

/// Road classes matching the openstreetmap-carto roads palette keys
/// (serialized as u8; index into ROAD_CLASSES). Railways ride at index
/// [`CLASS_RAIL`]; waterway classes use 30..=35 (see [`WATER_CLASSES`]).
pub const ROAD_CLASSES: &[&str] = &[
    "motorway",
    "motorway_link",
    "trunk",
    "trunk_link",
    "primary",
    "primary_link",
    "secondary",
    "secondary_link",
    "tertiary",
    "tertiary_link",
    "residential",
    "unclassified",
    "living_street",
    "pedestrian",
    "service",
    "track",
    "footway",
    "path",
    "cycleway",
    "steps",
    "rail",
];

/// Railways ride at the end of the road class table.
pub const CLASS_RAIL: u8 = 20;

/// Waterway classes (Line.class 30..=35, stored in `store.waterways`).
pub const WATER_CLASSES: &[&str] = &["river", "canal", "stream", "ditch", "drain", "other"];

pub const WATER_RIVER: u8 = 30;
pub const WATER_CANAL: u8 = 31;
pub const WATER_STREAM: u8 = 32;
pub const WATER_DITCH: u8 = 33;
pub const WATER_DRAIN: u8 = 34;
pub const WATER_OTHER: u8 = 35;

pub fn waterway_class(w: &str) -> u8 {
    match w {
        "river" => WATER_RIVER,
        "canal" => WATER_CANAL,
        "stream" | "flowline" => WATER_STREAM,
        "ditch" => WATER_DITCH,
        "drain" => WATER_DRAIN,
        _ => WATER_OTHER,
    }
}

pub fn road_class_index(hw: &str) -> Option<u8> {
    let idx = match hw {
        "motorway" => 0,
        "motorway_link" => 1,
        "trunk" => 2,
        "trunk_link" => 3,
        "primary" => 4,
        "primary_link" => 5,
        "secondary" => 6,
        "secondary_link" => 7,
        "tertiary" => 8,
        "tertiary_link" => 9,
        "residential" => 10,
        "unclassified" | "road" => 11,
        "living_street" => 12,
        "pedestrian" => 13,
        "service" => 14,
        "track" => 15,
        "footway" => 16,
        "path" => 17,
        "cycleway" => 18,
        "steps" => 19,
        _ => return None,
    };
    Some(idx)
}

pub fn road_class_name(idx: u8) -> &'static str {
    ROAD_CLASSES.get(idx as usize).copied().unwrap_or("unclassified")
}

/// A generic line feature (power, aeroways, barriers, piers, dams, ferry,
/// golf/track/waterslide…). `class` indexes into the owning collection's
/// class table (documented on the Store field).
#[derive(Serialize, Deserialize, Clone)]
pub struct FeatLine {
    pub id: i64,
    pub class: u8,
    pub name: String,
    pub bbox: [f32; 4],
    pub pts: Vec<u32>,
    /// Closed way -> treat as a polygon.
    pub area: bool,
}

/// A generic point feature (trees, lamps, entrances, crossings, towers,
/// junctions, stations, addresses, peaks, springs…).
#[derive(Serialize, Deserialize, Clone)]
pub struct FeatPoint {
    pub id: i64,
    pub class: u8,
    pub name: String,
    /// Elevation (peaks) or other numeric tag.
    pub value: u32,
    pub idx: u32,
}

/// FeatPoint classes (Store.points.class).
pub const PT_TREE: u8 = 0;
pub const PT_LAMP: u8 = 1;
pub const PT_ENTRANCE: u8 = 2;
pub const PT_CROSSING: u8 = 3;
pub const PT_TOWER: u8 = 4;
pub const PT_POLE: u8 = 5;
pub const PT_JUNCTION: u8 = 6;
pub const PT_STATION: u8 = 7;
pub const PT_ADDRESS: u8 = 8;
pub const PT_PEAK: u8 = 9;
pub const PT_SPRING: u8 = 10;
pub const PT_WATERFALL: u8 = 11;
pub const PT_TURNING: u8 = 12;

/// place=* classes (Place.class).
pub const PL_COUNTRY: u8 = 0;
pub const PL_STATE: u8 = 1;
pub const PL_COUNTY: u8 = 2;
pub const PL_CITY: u8 = 3;
pub const PL_TOWN: u8 = 4;
pub const PL_VILLAGE: u8 = 5;
pub const PL_HAMLET: u8 = 6;
pub const PL_SUBURB: u8 = 7;
pub const PL_QUARTER: u8 = 8;
pub const PL_NEIGHBOURHOOD: u8 = 9;
pub const PL_LOCALITY: u8 = 10;
pub const PL_FARM: u8 = 11;
pub const PL_DWELLING: u8 = 12;
pub const PL_ISLET: u8 = 13;
pub const PL_SEA: u8 = 14;
pub const PL_ISLAND: u8 = 15;
pub const PL_CONTINENT: u8 = 16;

pub fn place_class(p: &str) -> Option<u8> {
    Some(match p {
        "country" => PL_COUNTRY,
        "state" => PL_STATE,
        "county" | "region" | "district" | "municipality" => PL_COUNTY,
        "city" => PL_CITY,
        "town" => PL_TOWN,
        "village" => PL_VILLAGE,
        "hamlet" => PL_HAMLET,
        "suburb" => PL_SUBURB,
        "quarter" => PL_QUARTER,
        "neighbourhood" => PL_NEIGHBOURHOOD,
        "locality" => PL_LOCALITY,
        "farm" => PL_FARM,
        "isolated_dwelling" => PL_DWELLING,
        "islet" | "island" => PL_ISLET,
        "sea" => PL_SEA,
        "ocean" => PL_SEA,
        "continent" => PL_CONTINENT,
        _ => return None,
    })
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Place {
    pub id: i64,
    pub name: String,
    pub class: u8,
    pub capital: bool,
    pub population: u32,
    pub idx: u32,
}

/// An administrative boundary way (from boundary=administrative relations
/// or directly tagged ways). `level` is the OSM `admin_level` (2, 4, …);
/// 0 when untagged.
#[derive(Serialize, Deserialize, Clone)]
pub struct AdminBoundary {
    pub id: i64,
    pub level: u8,
    pub name: String,
    pub bbox: [f32; 4],
    pub pts: Vec<u32>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Line {
    pub id: i64,
    pub name: String,
    pub class: u8,
    pub length_m: f32,
    pub bbox: [f32; 4],
    /// Node indices into the store's coordinate arrays.
    pub pts: Vec<u32>,
    /// ONEWAY tag: 0 = none, 1 = yes (forward), 2 = -1 (reverse).
    pub oneway: u8,
    /// Lanes tag (total, both directions); 0 = untagged.
    pub lanes: u8,
    /// Maxspeed in km/h; 0 = untagged.
    pub maxspeed: u16,
    /// Highway `ref` (route number), empty when untagged.
    pub refs: String,
    pub tunnel: bool,
    pub bridge: bool,
    /// OSM `layer` (bridge/tunnel stacking), 0 = untagged.
    pub layer: i8,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Area {
    pub id: i64,
    /// e.g. "forest", "farmland", "residential", "" for plain buildings
    pub kind: String,
    /// Area name (building/landuse labels).
    pub name: String,
    pub bbox: [f32; 4],
    pub pts: Vec<u32>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Poi {
    pub id: i64,
    pub name: String,
    /// "amenity_hospital", "shop_supermarket", ...
    pub category: String,
    pub idx: u32,
}

#[derive(Serialize, Deserialize)]
pub struct Store {
    pub bbox: [f64; 4],
    pub lon: Vec<f32>,
    pub lat: Vec<f32>,
    pub roads: Vec<Line>,
    pub railways: Vec<Line>,
    pub waterways: Vec<Line>,
    pub water_areas: Vec<Area>,
    pub landuse: Vec<Area>,
    pub buildings: Vec<Area>,
    pub pois: Vec<Poi>,
    /// Ocean polygons (closed from coastline ways + bbox arcs).
    pub ocean: Vec<Area>,
    /// Closed coastline rings -> land (islands) painted over the ocean.
    pub islands: Vec<Area>,
    pub places: Vec<Place>,
    pub admin: Vec<AdminBoundary>,
    /// class 0 = power=line, 1 = power=minorline
    pub power: Vec<FeatLine>,
    /// class 0 = runway, 1 = taxiway, 2 = apron, 3 = helipad, 4 = parking_position
    pub aeroways: Vec<FeatLine>,
    /// class 0..8 = barrier=* kinds, 9 = natural=cliff
    pub barriers: Vec<FeatLine>,
    /// man_made=pier
    pub piers: Vec<FeatLine>,
    /// class 0 = dam, 1 = weir, 2 = groyne, 3 = breakwater
    pub water_barriers: Vec<FeatLine>,
    /// class 0 = natural=tree_row, 1 = natural=hedge
    pub tree_rows: Vec<FeatLine>,
    /// route=ferry ways
    pub ferry: Vec<FeatLine>,
    /// class 0 = golf line, 1 = leisure=track, 2 = waterslide, 3 = roller_coaster
    pub misc: Vec<FeatLine>,
    /// Closed highway ways (plazas / area=yes) — kind = road class name.
    pub road_areas: Vec<Area>,
    /// leisure=marina polygons (also in landuse as "marina" for fills).
    pub marinas: Vec<Area>,
    /// boundary=protected_area / leisure=nature_reserve polygons.
    pub protected: Vec<Area>,
    pub points: Vec<FeatPoint>,
}

/// Store format revision — bump when the schema changes so stale store.bin
/// files are detected and reported as "re-extract".
pub const STORE_VERSION: u32 = 2;

impl Store {
    pub fn empty(bbox: [f64; 4]) -> Self {
        Self {
            bbox,
            lon: Vec::new(),
            lat: Vec::new(),
            roads: Vec::new(),
            railways: Vec::new(),
            waterways: Vec::new(),
            water_areas: Vec::new(),
            landuse: Vec::new(),
            buildings: Vec::new(),
            pois: Vec::new(),
            ocean: Vec::new(),
            islands: Vec::new(),
            places: Vec::new(),
            admin: Vec::new(),
            power: Vec::new(),
            aeroways: Vec::new(),
            barriers: Vec::new(),
            piers: Vec::new(),
            water_barriers: Vec::new(),
            tree_rows: Vec::new(),
            ferry: Vec::new(),
            misc: Vec::new(),
            road_areas: Vec::new(),
            marinas: Vec::new(),
            protected: Vec::new(),
            points: Vec::new(),
        }
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        let out = dir.join("store.bin");
        let f = File::create(&out)?;
        serialize_into(BufWriter::new(f), self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(())
    }

    pub fn load(dir: &Path) -> std::io::Result<Self> {
        let f = File::open(dir.join("store.bin"))?;
        deserialize_from(BufReader::new(f))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

/* --------------------------------------------------------------------- */
/* Extraction                                                              */
/* --------------------------------------------------------------------- */

#[derive(Clone)]
enum Kind {
    Road(u8),
    Railway,
    Waterway(u8),
    Water,
    Landuse(String),
    Building,
    Coastline,
    AdminWay(u8),
    ProtectedWay,
    FerryWay,
    Power(u8),
    Aeroway(u8, bool),
    Barrier(u8),
    Pier,
    WaterBarrier(u8),
    TreeRow(u8),
    Misc(u8),
    RoadArea(u8),
    Marina,
}

struct RawWay {
    id: i64,
    kind: Kind,
    name: String,
    oneway: u8,
    lanes: u8,
    maxspeed: u16,
    refs: String,
    tunnel: bool,
    bridge: bool,
    layer: i8,
    /// Resolved node indices (only nodes inside the bbox).
    pts: Vec<u32>,
}

/// `oneway` tag -> 0 none / 1 forward / 2 reverse (matches openstreetmap-carto
/// `oneway = '-1'` handling for the reverse arrow marker).
fn oneway_code(v: Option<&str>) -> u8 {
    match v {
        Some("yes") | Some("true") | Some("1") => 1,
        Some("-1") | Some("reverse") => 2,
        _ => 0,
    }
}

/// `maxspeed` tag -> km/h. Handles plain numbers ("50"), imprecise lists
/// ("50|60") and discards zone/speed-type values that have no number.
fn maxspeed_kmh(v: Option<&str>) -> u16 {
    let Some(v) = v else { return 0 };
    let first = v.split('|').next().unwrap_or(v).trim();
    let digits: String = first.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

/// `lanes` tag -> lane count (first value of a "2;3"-style list).
fn lanes_count(v: Option<&str>) -> u8 {
    let Some(v) = v else { return 0 };
    let first = v.split(';').next().unwrap_or(v).trim();
    first.parse::<u8>().unwrap_or(0).min(30)
}

/// First integer inside a free-form tag ("12 m", "1,234", "≈2,000").
fn leading_u32(v: &str) -> u32 {
    let digits: String = v
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(|c| *c != ',')
        .collect();
    digits.parse().unwrap_or(0)
}

/// barrier=* -> Barrier class (see Store.barriers doc).
fn barrier_class(v: &str) -> u8 {
    match v {
        "fence" => 0,
        "wall" => 1,
        "city_wall" => 2,
        "hedge" => 3,
        "retaining_wall" => 4,
        "kerb" => 5,
        "guard_rail" => 6,
        "ditch" => 7,
        "embankment" => 8,
        _ => 0,
    }
}

/// The ways that MUST be kept even when most of their nodes fall outside the
/// extract bbox (coastlines and relation-sourced boundaries must not break).
#[derive(Default)]
struct MustKeep {
    admin: HashMap<i64, u8>,
    protected: HashSet<i64>,
    ferry: HashSet<i64>,
}

fn classify_way<'a, F: Fn(&str) -> Option<&'a str>>(
    tags: F,
    id: i64,
    keep: &MustKeep,
) -> Option<RawWay> {
    let name = tags("name").unwrap_or("").to_string();
    let oneway = oneway_code(tags("oneway"));
    let lanes = lanes_count(tags("lanes"));
    let maxspeed = maxspeed_kmh(tags("maxspeed"));
    let refs = tags("ref").unwrap_or("").to_string();
    let tunnel = matches!(tags("tunnel"), Some("yes") | Some("true") | Some("1"));
    let bridge = matches!(tags("bridge"), Some("yes") | Some("true") | Some("1"));
    let layer = tags("layer").and_then(|v| v.trim().parse::<i8>().ok()).unwrap_or(0);

    // Coastline first: a beach path tagged along a coast must not win.
    let kind = if tags("natural") == Some("coastline") {
        Kind::Coastline
    } else if let Some(hw) = tags("highway") {
        let class = road_class_index(hw)?;
        Kind::Road(class)
    } else if let Some(rw) = tags("railway") {
        if rw == "platform" {
            Kind::Landuse("platform".into())
        } else {
            Kind::Railway
        }
    } else if let Some(w) = tags("waterway") {
        if w == "dam" {
            Kind::WaterBarrier(0)
        } else if w == "weir" {
            Kind::WaterBarrier(1)
        } else if w == "groyne" {
            Kind::WaterBarrier(2)
        } else if w == "breakwater" {
            Kind::WaterBarrier(3)
        } else {
            Kind::Waterway(waterway_class(w))
        }
    } else if tags("building").is_some() || tags("building:part").is_some() {
        Kind::Building
    } else if let Some(mm) = tags("man_made") {
        // man_made before landuse: quay/wharf/pier beat residual landuse tags
        match mm {
            "pier" => Kind::Pier,
            "quay" | "wharf" => Kind::Landuse("quay".into()),
            "breakwater" => Kind::WaterBarrier(3),
            "groyne" => Kind::WaterBarrier(2),
            "embankment" => Kind::Barrier(8),
            _ => return None,
        }
    } else if let Some(lu) = tags("landuse") {
        Kind::Landuse(lu.to_string())
    } else if let Some(nat) = tags("natural") {
        match nat {
            "water" | "wetland" | "pond" | "lake" | "riverbank" | "reservoir" | "basin"
            | "canal" => Kind::Water,
            "wood" => Kind::Landuse("forest".into()),
            "scrub" => Kind::Landuse("scrub".into()),
            "grassland" => Kind::Landuse("grass".into()),
            "heath" => Kind::Landuse("heath".into()),
            "sand" => Kind::Landuse("sand".into()),
            "beach" => Kind::Landuse("beach".into()),
            "bare_rock" | "scree" => Kind::Landuse("bare_rock".into()),
            "shingle" => Kind::Landuse("beach".into()),
            "mud" => Kind::Landuse("mud".into()),
            "cliff" => Kind::Barrier(9),
            "tree_row" => Kind::TreeRow(0),
            "hedge" => Kind::TreeRow(1),
            _ => return None,
        }
    } else if let Some(le) = tags("leisure") {
        match le {
            "park" | "garden" | "pitch" | "golf_course" | "playground" | "common"
            | "recreation_ground" | "dog_park" | "mini_golf" | "stadium" | "track" => {
                if le == "track" {
                    Kind::Misc(1)
                } else if le == "mini_golf" {
                    Kind::Landuse("mini_golf".into())
                } else {
                    Kind::Landuse(le.to_string())
                }
            }
            "marina" => Kind::Marina,
            "nature_reserve" => Kind::ProtectedWay,
            "swimming_pool" => Kind::Water,
            _ => return None,
        }
    } else if let Some(am) = tags("amenity") {
        match am {
            "school" | "hospital" | "college" | "university" | "place_of_worship"
            | "marketplace" | "parking" | "parking_space" | "fountain" => {
                if am == "fountain" {
                    Kind::Water
                } else {
                    Kind::Landuse(am.into())
                }
            }
            _ => return None,
        }
    } else if let Some(aw) = tags("aeroway") {
        match aw {
            "runway" => Kind::Aeroway(0, false),
            "taxiway" => Kind::Aeroway(1, false),
            "apron" => Kind::Aeroway(2, true),
            "helipad" => Kind::Aeroway(3, true),
            "parking_position" => Kind::Aeroway(4, false),
            _ => return None,
        }
    } else if let Some(pw) = tags("power") {
        match pw {
            "line" => Kind::Power(0),
            "minor_line" => Kind::Power(1),
            "plant" => Kind::Landuse("industrial".into()),
            "substation" => Kind::Landuse("industrial".into()),
            _ => return None,
        }
    } else if let Some(bar) = tags("barrier") {
        if bar == "city_wall" {
            Kind::Barrier(2)
        } else {
            Kind::Barrier(barrier_class(bar))
        }
    } else if let Some(gf) = tags("golf") {
        match gf {
            "hole" | "cartpath" | "path" => Kind::Misc(0),
            "rough" | "fairway" | "green" | "tee" | "bunker" | "water_hazard" => {
                Kind::Landuse("golf_rough".into())
            }
            _ => return None,
        }
    } else if tags("attraction") == Some("waterslide") {
        Kind::Misc(2)
    } else if tags("roller_coaster") == Some("track")
        || tags("roller_coaster") == Some("structure")
    {
        Kind::Misc(3)
    } else if let Some(b) = tags("boundary") {
        match b {
            "administrative" => Kind::AdminWay(
                tags("admin_level").and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(0),
            ),
            "protected_area" => Kind::ProtectedWay,
            "maritime" => Kind::AdminWay(2),
            _ => return None,
        }
    } else if tags("route") == Some("ferry") {
        Kind::FerryWay
    } else if let Some(rel_kind) = relation_kind(id, keep) {
        rel_kind
    } else {
        return None;
    };

    // Direct highway=* closed ways (plazas) become RoadArea.
    let closed_hint = tags("area").map(|v| v == "yes" || v == "true" || v == "1").unwrap_or(false);
    if let Kind::Road(class) = kind {
        if closed_hint {
            return Some(RawWay {
                id,
                kind: Kind::RoadArea(class),
                name,
                oneway: 0,
                lanes,
                maxspeed,
                refs,
                tunnel,
                bridge,
                layer,
                pts: Vec::new(),
            });
        }
    }

    Some(RawWay { id, kind, name, oneway, lanes, maxspeed, refs, tunnel, bridge, layer, pts: Vec::new() })
}

/// Fall back to relation membership when direct tags don't classify a way.
fn relation_kind(id: i64, keep: &MustKeep) -> Option<Kind> {
    if let Some(&level) = keep.admin.get(&id) {
        return Some(Kind::AdminWay(level));
    }
    if keep.protected.contains(&id) {
        return Some(Kind::ProtectedWay);
    }
    if keep.ferry.contains(&id) {
        return Some(Kind::FerryWay);
    }
    None
}

/// Shoelace signed area in geographic coords (y-up): negative = clockwise =
/// interior on the right of the walk direction (OSM coastline: water).
pub fn ring_signed_area(pts: &[(f64, f64)]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        let (x1, y1) = pts[i];
        let (x2, y2) = pts[(i + 1) % n];
        a += x1 * y2 - x2 * y1;
    }
    a / 2.0
}

/// Ray-casting point-in-polygon on lon/lat rings.
pub fn point_in_ring(p: (f64, f64), ring: &[(f64, f64)]) -> bool {
    let (x, y) = p;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = ring[i];
        let (xj, yj) = ring[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Close an open, flow-normalized coastline chain (water on the RIGHT of the
/// walk direction) against the extract bbox. Returns `(is_water, ring)`:
/// `true` when a clockwise (water-inside) closure exists — the sea sliver —
/// and `false` when both arc closures are counter-clockwise, meaning the
/// chain bounds land (e.g. an island tip cut by the bbox edge).
pub fn close_coast_ring(chain: &[(f64, f64)], bbox: [f64; 4]) -> Option<(bool, Vec<(f64, f64)>)> {
    const EPS: f64 = 1e-7;
    let (w, s, e, n) = (bbox[0], bbox[1], bbox[2], bbox[3]);
    if chain.len() < 2 {
        return None;
    }
    // The extract keeps only nodes INSIDE the bbox, so a coastline way that
    // crosses the boundary ends one segment short of the edge. Snap endpoints
    // within tolerance onto the nearest perimeter edge so the arc closure
    // applies (the polygon closes visually across the small jog).
    const SNAP_TOL: f64 = 0.08; // ~9 km
    let snap = |p: (f64, f64)| -> (f64, f64) {
        let (dl, dr, db, dt) = ((p.0 - w).abs(), (e - p.0).abs(), (p.1 - s).abs(), (n - p.1).abs());
        let m = dl.min(dr).min(db).min(dt);
        if m >= SNAP_TOL {
            return p;
        }
        let mut q = p;
        if m == dl {
            q.0 = w;
            q.1 = q.1.clamp(s, n);
        } else if m == dr {
            q.0 = e;
            q.1 = q.1.clamp(s, n);
        } else if m == db {
            q.1 = s;
            q.0 = q.0.clamp(w, e);
        } else {
            q.1 = n;
            q.0 = q.0.clamp(w, e);
        }
        q
    };
    let start = snap(chain[0]);
    let end = snap(*chain.last().unwrap());
    let on_perimeter = |p: (f64, f64)| -> bool {
        let on_x = (p.0 - w).abs() < EPS || (p.0 - e).abs() < EPS;
        let on_y = (p.1 - s).abs() < EPS || (p.1 - n).abs() < EPS;
        (on_x && p.1 >= s - EPS && p.1 <= n + EPS) || (on_y && p.0 >= w - EPS && p.0 <= e + EPS)
    };

    // Both endpoints on the bbox perimeter -> close via the perimeter arc.
    if std::env::var("CARTO_DEBUG").is_ok() {
        eprintln!(
            "[coast] start={:?} end={:?} on_perim(start)={} on_perim(end)={}",
            start, end, on_perimeter(start), on_perimeter(end)
        );
    }
    if on_perimeter(start) && on_perimeter(end) {
        let perim = |p: (f64, f64)| -> f64 {
            // param along cw perimeter from NW corner
            let (x, y) = p;
            if (y - n).abs() < EPS {
                return x - w; // top edge west->east
            }
            if (x - e).abs() < EPS {
                return (e - w) + (n - y); // east edge north->south
            }
            if (y - s).abs() < EPS {
                return (e - w) + (n - s) + (e - x); // bottom edge east->west
            }
            (e - w) * 2.0 + (n - s) + (y - s) // west edge south->north
        };
        let t_start = perim(start);
        let t_end = perim(end);
        let total = (e - w) * 2.0 + (n - s) * 2.0;

        let arc = |from: f64, to: f64| -> Vec<(f64, f64)> {
            // walk cw perimeter from `from` to `to`, sampling corners passed
            let mut pts = Vec::new();
            let corner_ts = [
                0.0,
                e - w,
                (e - w) + (n - s),
                (e - w) * 2.0 + (n - s),
                total,
            ];
            let corner_pts = [(w, n), (e, n), (e, s), (w, s)];
            let mut t = from;
            if to < from {
                for i in 0..4 {
                    if corner_ts[i] > t + EPS && corner_ts[i] < total - EPS {
                        pts.push(corner_pts[i]);
                    }
                }
                pts.push(corner_pts[0]);
                t = 0.0;
            }
            for i in 0..4 {
                if corner_ts[i] > t + EPS && corner_ts[i] < to - EPS {
                    pts.push(corner_pts[i]);
                }
            }
            pts
        };

        // candidate 1: cw arc end -> start
        let mut ring1 = chain.to_vec();
        ring1.extend(arc(t_end, t_start));
        // candidate 2: ccw arc end -> start (cw arc start -> end, reversed)
        let mut ring2 = chain.to_vec();
        let mut a2 = arc(t_start, t_end);
        a2.reverse();
        ring2.extend(a2);

        let (a1, a2s) = (ring_signed_area(&ring1), ring_signed_area(&ring2));
        if std::env::var("CARTO_DEBUG").is_ok() {
            eprintln!("[coast] t_start={t_start:.4} t_end={t_end:.4} total={total:.4} a1={a1:.6} a2={a2s:.6}");
        }
        if a1 < 0.0 && a2s >= 0.0 {
            return Some((true, ring1));
        }
        if a2s < 0.0 && a1 >= 0.0 {
            return Some((true, ring2));
        }
        if a1 < 0.0 && a2s < 0.0 {
            // both clockwise (self-intersecting): keep the smaller sea sliver
            return if a1.abs() <= a2s.abs() {
                Some((true, ring1))
            } else {
                Some((true, ring2))
            };
        }
        // Neither closure is clockwise: this chain bounds LAND (island tip
        // cut by the bbox edge). Close with the shorter arc as land.
        return if a1.abs() <= a2s.abs() {
            Some((false, ring1))
        } else {
            Some((false, ring2))
        };
    }

    // Broken chain (endpoint not on the perimeter): close with a chord and
    // orient it clockwise so it still paints water (best effort for gaps).
    let mut ring = chain.to_vec();
    ring.push(start);
    if ring_signed_area(&ring) > 0.0 {
        ring.reverse();
    }
    Some((true, ring))
}

/// Join coastline chains and close them into ocean/island rings.
/// `chains` are lon/lat polylines in OSM way direction (water on the RIGHT).
/// Returns `(is_island, ring)` pairs. OSM flow is tracked while joining so
/// the "water on the right" rule survives chain reversal, and closed rings
/// are classified by orientation: counter-clockwise (interior on the left) =
/// island land, clockwise (interior on the right) = enclosed sea.
pub fn build_ocean_rings(chains: Vec<Vec<(f64, f64)>>, bbox: [f64; 4]) -> Vec<(bool, Vec<(f64, f64)>)> {
    const EPS: f64 = 1e-7;
    let mut open: Vec<(Vec<(f64, f64)>, i32)> = Vec::new();
    let mut out: Vec<(bool, Vec<(f64, f64)>)> = Vec::new();

    for c in chains {
        if c.len() < 2 {
            continue;
        }
        let (sx, sy) = c[0];
        let (ex, ey) = *c.last().unwrap();
        if (sx - ex).abs() < EPS && (sy - ey).abs() < EPS && c.len() >= 4 {
            // Closed ring: normalize to flow, then CCW (positive) = island.
            let mut ring = c;
            // A self-closing way is its own flow reference; CCW = island.
            let island = ring_signed_area(&ring) > 0.0;
            out.push((island, ring));
        } else {
            open.push((c, 1));
        }
    }

    // Join open chains sharing endpoints, preserving OSM flow (+1 when a
    // piece is walked in its own direction, -1 when reversed). Both the tail
    // and the head of the growing chain are extended, searching every other
    // accumulated chain so split coastways knit into one ring.
    let mut joined: Vec<(Vec<(f64, f64)>, i32)> = Vec::new();
    'outer: while let Some((mut chain, mut flow)) = open.pop() {
        loop {
            let (ex, ey) = *chain.last().unwrap();
            let (sx, sy) = chain[0];
            // A: other.start == chain.end  -> (true, false)  append as-is
            // B: other.end   == chain.end  -> (true, true)   append reversed
            // C: other.end   == chain.start-> (false, false) prepend as-is
            // D: other.start == chain.start-> (false, true)  prepend reversed
            let mut found: Option<(bool, usize, bool, bool)> = None;
            for (list, haystack) in [(false, &open), (true, &joined)] {
                for (j, (other, _)) in haystack.iter().enumerate() {
                    let (o_sx, o_sy) = (other[0].0, other[0].1);
                    let (o_ex, o_ey) = (other.last().unwrap().0, other.last().unwrap().1);
                    if (o_sx - ex).abs() < EPS && (o_sy - ey).abs() < EPS {
                        found = Some((list, j, true, false));
                        break;
                    }
                    if (o_ex - ex).abs() < EPS && (o_ey - ey).abs() < EPS {
                        found = Some((list, j, true, true));
                        break;
                    }
                    if (o_ex - sx).abs() < EPS && (o_ey - sy).abs() < EPS {
                        found = Some((list, j, false, false));
                        break;
                    }
                    if (o_sx - sx).abs() < EPS && (o_sy - sy).abs() < EPS {
                        found = Some((list, j, false, true));
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            let Some((from_joined, j, end_match, rev)) = found else {
                joined.push((chain, flow));
                continue 'outer;
            };
            let (mut other, oflow) = if from_joined {
                joined.swap_remove(j)
            } else {
                open.swap_remove(j)
            };
            let piece_flow = if rev { -oflow } else { oflow };
            if rev {
                other.reverse();
            }
            if end_match {
                other.remove(0);
                chain.extend(other);
            } else {
                other.pop();
                other.extend(chain);
                chain = other;
            }
            flow += piece_flow;
            let (cx, cy) = *chain.last().unwrap();
            let (csx, csy) = chain[0];
            if (cx - csx).abs() < EPS && (cy - csy).abs() < EPS {
                if chain.len() >= 4 {
                    let mut ring = chain;
                    if flow < 0 {
                        ring.reverse();
                    }
                    let island = ring_signed_area(&ring) > 0.0;
                    out.push((island, ring));
                }
                continue 'outer;
            }
        }
    }

    for (chain, flow) in joined {
        // Normalize to OSM flow (water on the right of the walk).
        let chain = if flow < 0 {
            let mut c = chain;
            c.reverse();
            c
        } else {
            chain
        };
        if let Some((is_water, ring)) = close_coast_ring(&chain, bbox) {
            out.push((!is_water, ring));
        }
    }
    out
}

/// Extract the bbox dataset from a Geofabrik `.osm.pbf` and write
/// `store.bin` + `registry.json` into `out_dir`.
pub fn extract(pbf: &Path, out_dir: &Path, bbox: [f64; 4]) -> std::io::Result<Store> {
    let (w, s, e, n) = (bbox[0], bbox[1], bbox[2], bbox[3]);
    let mut node_id_to_idx: HashMap<i64, u32> = HashMap::with_capacity(5_000_000);
    let mut lon: Vec<f32> = Vec::with_capacity(5_000_000);
    let mut lat: Vec<f32> = Vec::with_capacity(5_000_000);
    let mut pois: Vec<Poi> = Vec::new();
    let mut places: Vec<Place> = Vec::new();
    let mut points: Vec<FeatPoint> = Vec::new();
    let mut ways: Vec<RawWay> = Vec::new();
    let mut keep = MustKeep::default();

    let mut node_count = 0usize;

    /* ---- PASS 1: nodes (map + pois + places + points) and relations ---- */
    let reader = osmpbf::ElementReader::from_path(pbf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    reader.for_each(|el| match el {
        osmpbf::Element::Node(node) => {
            node_count += 1;
            capture_node(&node, &mut |tags: NodeTags| {
                node_capture(
                    tags, &w, &e, &s, &n, &mut node_id_to_idx, &mut lon, &mut lat,
                    &mut pois, &mut places, &mut points,
                )
            });
            if node_count % 10_000_000 == 0 {
                eprintln!("  nodes scanned: {node_count}…");
            }
        }
        osmpbf::Element::DenseNode(d) => {
            node_count += 1;
            capture_dense(&d, &mut |tags: NodeTags| {
                node_capture(
                    tags, &w, &e, &s, &n, &mut node_id_to_idx, &mut lon, &mut lat,
                    &mut pois, &mut places, &mut points,
                )
            });
            if node_count % 10_000_000 == 0 {
                eprintln!("  nodes scanned: {node_count}…");
            }
        }
        osmpbf::Element::Relation(rel) => {
            let get = |key: &str| -> Option<String> {
                rel.tags().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
            };
            let rtype = get("type").unwrap_or_default();
            let boundary = get("boundary").unwrap_or_default();
            let level = get("admin_level").and_then(|v| v.trim().parse::<u8>().ok()).unwrap_or(0);
            match (rtype.as_str(), boundary.as_str(), get("route").as_deref()) {
                ("boundary", "administrative", _) | ("multipolygon", "administrative", _) => {
                    for m in rel.members() {
                        if matches!(m.member_type, osmpbf::RelMemberType::Way) {
                            let e = keep.admin.entry(m.member_id).or_insert(level);
                            if level > 0 && (*e == 0 || level < *e) {
                                *e = level;
                            }
                        }
                    }
                }
                ("boundary", "protected_area", _) | ("multipolygon", "protected_area", _) => {
                    for m in rel.members() {
                        if matches!(m.member_type, osmpbf::RelMemberType::Way) {
                            keep.protected.insert(m.member_id);
                        }
                    }
                }
                ("route", _, Some("ferry")) => {
                    for m in rel.members() {
                        if matches!(m.member_type, osmpbf::RelMemberType::Way) {
                            keep.ferry.insert(m.member_id);
                        }
                    }
                }
                _ => {}
            }
        }
        osmpbf::Element::Way(_) => {}
    })
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    eprintln!(
        "  pass 1 done: {} nodes in bbox; {} admin ways; {} protected; {} ferry",
        node_id_to_idx.len(),
        keep.admin.len(),
        keep.protected.len(),
        keep.ferry.len()
    );

    /* ---- PASS 2: ways (resolve geometry) ---- */
    let reader = osmpbf::ElementReader::from_path(pbf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    reader.for_each(|el| match el {
        osmpbf::Element::Way(way) => {
            let get = |key: &str| -> Option<&str> {
                way.tags().find(|(k, _)| *k == key).map(|(_, v)| v)
            };
            let id = way.id();
            let member = keep.admin.contains_key(&id)
                || keep.protected.contains(&id)
                || keep.ferry.contains(&id);
            if let Some(mut rw) = classify_way(get, id, &keep) {
                let must = matches!(
                    rw.kind,
                    Kind::Coastline | Kind::AdminWay(_) | Kind::ProtectedWay | Kind::FerryWay
                ) || member;
                let mut pts: Vec<u32> = Vec::with_capacity(8);
                let mut total = 0usize;
                for r in way.refs() {
                    total += 1;
                    if let Some(&i) = node_id_to_idx.get(&r) {
                        pts.push(i);
                    }
                }
                let ok = if must {
                    pts.len() >= 2
                } else {
                    pts.len() >= 2 && pts.len() * 3 >= total
                };
                if ok {
                    rw.pts = pts;
                    ways.push(rw);
                }
            }
        }
        _ => {}
    })
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    eprintln!("  pass 2 done: {} ways kept", ways.len());

    /* ---- finalize store ---- */
    let mut store = Store::empty(bbox);
    store.lon = lon;
    store.lat = lat;
    store.pois = pois;
    store.places = places;
    store.points = points;

    // Ways first: landuse fills double as known-land probes for validating
    // ocean rings against flood false-positives.
    let coast_chains: Vec<Vec<(f64, f64)>> = ways
        .iter()
        .filter(|w| matches!(w.kind, Kind::Coastline))
        .map(|wway| {
            wway.pts
                .iter()
                .map(|&i| (f64::from(store.lon[i as usize]), f64::from(store.lat[i as usize])))
                .collect()
        })
        .collect();
    let rings = build_ocean_rings(coast_chains, bbox);

    for wway in ways {
        let pts = wway.pts;
        // bbox: west/south start at +MAX, east/north at -MAX
        let mut bb = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for &i in &pts {
            let (x, y) = (store.lon[i as usize], store.lat[i as usize]);
            bb[0] = bb[0].min(x);
            bb[1] = bb[1].min(y);
            bb[2] = bb[2].max(x);
            bb[3] = bb[3].max(y);
        }
        let mut length = 0f64;
        for w in pts.windows(2) {
            length += haversine_m(
                f64::from(store.lon[w[0] as usize]),
                f64::from(store.lat[w[0] as usize]),
                f64::from(store.lon[w[1] as usize]),
                f64::from(store.lat[w[1] as usize]),
            );
        }
        let closed = pts.len() >= 4 && pts.first() == pts.last();
        let line = |class: u8| Line {
            id: wway.id,
            name: wway.name.clone(),
            class,
            length_m: length as f32,
            bbox: bb,
            pts: pts.clone(),
            oneway: wway.oneway,
            lanes: wway.lanes,
            maxspeed: wway.maxspeed,
            refs: wway.refs.clone(),
            tunnel: wway.tunnel,
            bridge: wway.bridge,
            layer: wway.layer,
        };
        let feat = |class: u8, area: bool| FeatLine {
            id: wway.id,
            class,
            name: wway.name.clone(),
            bbox: bb,
            pts: pts.clone(),
            area,
        };
        let area = |kind: &str| Area {
            id: wway.id,
            kind: kind.to_string(),
            name: wway.name.clone(),
            bbox: bb,
            pts: pts.clone(),
        };
        match wway.kind {
            Kind::Road(class) => store.roads.push(line(class)),
            Kind::Railway => store.railways.push(line(CLASS_RAIL)),
            Kind::Waterway(class) => store.waterways.push(line(class)),
            Kind::Water => store.water_areas.push(area("water")),
            Kind::Landuse(kind) => store.landuse.push(area(&kind)),
            Kind::Building => store.buildings.push(area("")),
            Kind::RoadArea(class) => store.road_areas.push(area(road_class_name(class))),
            Kind::Marina => {
                store.marinas.push(area("marina"));
                store.landuse.push(area("marina"));
            }
            Kind::ProtectedWay => store.protected.push(area("protected")),
            Kind::Coastline => {} // consumed above
            Kind::AdminWay(level) => store.admin.push(AdminBoundary {
                id: wway.id,
                level,
                name: wway.name.clone(),
                bbox: bb,
                pts,
            }),
            Kind::Power(minor) => store.power.push(feat(minor, false)),
            Kind::Aeroway(kind, a) => store.aeroways.push(feat(kind, a || closed)),
            Kind::Barrier(class) => store.barriers.push(feat(class, false)),
            Kind::Pier => store.piers.push(feat(0, closed)),
            Kind::WaterBarrier(class) => store.water_barriers.push(feat(class, closed)),
            Kind::TreeRow(class) => store.tree_rows.push(feat(class, false)),
            Kind::FerryWay => store.ferry.push(feat(0, false)),
            Kind::Misc(class) => store.misc.push(feat(class, closed && class == 1)),
        }
    }

    // Ocean/island rings: validate ocean rings against known-land probes
    // (centroids of solid inland landuse fills, EXCLUDING anything on an
    // island — island land legitimately sits inside the mainland's sea
    // sliver). A "sea" ring that swallows mainland land points is a flood
    // false-positive (a partially-included island closed the wrong way) and
    // is dropped; the genuine sea sliver never covers the mainland.
    let (island_rings, other_rings): (Vec<_>, Vec<_>) =
        rings.into_iter().partition(|(is_island, _)| *is_island);
    let solid_land = |kind: &str| {
        matches!(
            kind,
            "farmland" | "forest" | "residential" | "scrub" | "grass" | "industrial"
                | "commercial" | "school" | "hospital" | "college" | "university"
        )
    };
    let mut centroids: Vec<(f64, f64)> = Vec::new();
    for a in &store.landuse {
        if a.pts.len() < 3 || !solid_land(&a.kind) {
            continue;
        }
        let (mut sx, mut sy) = (0.0f64, 0.0f64);
        for &i in &a.pts {
            sx += f64::from(store.lon[i as usize]);
            sy += f64::from(store.lat[i as usize]);
        }
        let c = (sx / a.pts.len() as f64, sy / a.pts.len() as f64);
        // skip centroids that fall on an island (inside any island ring)
        if island_rings.iter().any(|(_, ring)| point_in_ring(c, ring)) {
            continue;
        }
        centroids.push(c);
        if centroids.len() >= 4000 {
            break;
        }
    }
    let probes = &centroids;
    let mut ocean_kept = 0usize;
    let mut floods = 0usize;
    for (is_island, ring) in island_rings.into_iter().chain(other_rings) {
        // A genuine sea sliver holds ~no mainland land; a flood ring (an
        // island tip closed the wrong way around the bbox) holds nearly all
        // of it. Rings that mix both (a coast exiting an adjacent edge keeps
        // a small land wedge north of its entry point) are kept — the wedge
        // is small and landuse fills repaint over it.
        if !is_island && !probes.is_empty() {
            let inside = probes.iter().filter(|p| point_in_ring(**p, &ring)).count();
            if inside as f64 > probes.len() as f64 * 0.25 {
                floods += 1;
                continue;
            }
        }
        let mut bb = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for (x, y) in &ring {
            bb[0] = bb[0].min(*x as f32);
            bb[1] = bb[1].min(*y as f32);
            bb[2] = bb[2].max(*x as f32);
            bb[3] = bb[3].max(*y as f32);
        }
        // Append ring vertices as synthetic nodes so the renderer draws them
        // through the same pipeline.
        let base = store.lon.len() as u32;
        for (x, y) in &ring {
            store.lon.push(*x as f32);
            store.lat.push(*y as f32);
        }
        let pts: Vec<u32> = (base..base + ring.len() as u32).collect();
        let mut area = Area { id: 0, kind: String::new(), name: String::new(), bbox: bb, pts };
        if is_island {
            area.kind = "island".into();
            store.islands.push(area);
        } else {
            area.kind = "ocean".into();
            store.ocean.push(area);
            ocean_kept += 1;
        }
    }
    if !probes.is_empty() || ocean_kept > 0 {
        eprintln!("  ocean rings kept {ocean_kept}, {floods} flood rings dropped (land probes: {})", probes.len());
    }

    // Sort roads: named first (alphabetical), then unnamed by id — so a
    // limit-truncated registry list always contains every named road.
    store.roads.sort_by(|a, b| match (a.name.is_empty(), b.name.is_empty()) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        (false, false) => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        (true, true) => a.id.cmp(&b.id),
    });

    let named = store.roads.iter().filter(|r| !r.name.is_empty()).count();
    let summary = serde_json_to_string(&RegistrySummary {
        source: pbf.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        bbox,
        nodes: node_id_to_idx.len(),
        roads: store.roads.len(),
        named_roads: named,
        railways: store.railways.len(),
        waterways: store.waterways.len(),
        water_areas: store.water_areas.len(),
        landuse: store.landuse.len(),
        buildings: store.buildings.len(),
        pois: store.pois.len(),
        places: store.places.len(),
        admin: store.admin.len(),
        power: store.power.len(),
        aeroways: store.aeroways.len(),
        barriers: store.barriers.len(),
        piers: store.piers.len(),
        water_barriers: store.water_barriers.len(),
        tree_rows: store.tree_rows.len(),
        ferry: store.ferry.len(),
        misc: store.misc.len(),
        road_areas: store.road_areas.len(),
        marinas: store.marinas.len(),
        protected: store.protected.len(),
        points: store.points.len(),
        ocean: store.ocean.len(),
        islands: store.islands.len(),
    });
    fs::create_dir_all(out_dir)?;
    fs::write(out_dir.join("registry.json"), summary)?;
    store.save(out_dir)?;
    eprintln!(
        "  store written: {} roads ({named} named), {} places, {} admin, {} ocean, {} islands",
        store.roads.len(),
        store.places.len(),
        store.admin.len(),
        store.ocean.len(),
        store.islands.len()
    );
    Ok(store)
}

/* ---- node capture helpers (shared by Node and DenseNode) ---- */

pub struct NodeTags<'a> {
    pub id: i64,
    pub lon: f64,
    pub lat: f64,
    pub tags: Vec<(&'a str, &'a str)>,
}

fn capture_node<'a, F: FnMut(NodeTags<'a>)>(node: &'a osmpbf::Node, f: &mut F) {
    let tags: Vec<(&str, &str)> = node.tags().collect();
    f(NodeTags { id: node.id(), lon: node.lon(), lat: node.lat(), tags });
}

fn capture_dense<'a, F: FnMut(NodeTags<'a>)>(d: &'a osmpbf::DenseNode, f: &mut F) {
    let tags: Vec<(&str, &str)> = d.tags().collect();
    f(NodeTags { id: d.id(), lon: d.lon(), lat: d.lat(), tags });
}

#[allow(clippy::too_many_arguments)]
fn node_capture<'a>(
    t: NodeTags<'a>,
    w: &f64,
    e: &f64,
    s: &f64,
    n: &f64,
    node_id_to_idx: &mut HashMap<i64, u32>,
    lon: &mut Vec<f32>,
    lat: &mut Vec<f32>,
    pois: &mut Vec<Poi>,
    places: &mut Vec<Place>,
    points: &mut Vec<FeatPoint>,
) {
    let (x, y) = (t.lon, t.lat);
    if !(x >= *w && x <= *e && y >= *s && y <= *n) {
        return;
    }
    let idx = lon.len() as u32;
    node_id_to_idx.insert(t.id, idx);
    lon.push(x as f32);
    lat.push(y as f32);

    let mut name = String::new();
    let mut category: Option<String> = None;
    let mut place: Option<u8> = None;
    let mut capital = false;
    let mut population = 0u32;
    let mut pt_class: Option<u8> = None;
    let mut pt_value = 0u32;
    for (k, v) in &t.tags {
        match *k {
            "name" => name = (*v).to_string(),
            "amenity" | "shop" | "tourism" | "leisure" | "healthcare" | "craft" | "office" => {
                if category.is_none() {
                    category = Some(format!("{k}_{v}"));
                }
            }
            "place" => place = place_class(v),
            "capital" => capital = *v == "yes",
            "population" => population = leading_u32(v),
            "natural" => match *v {
                "tree" => pt_class = Some(PT_TREE),
                "peak" => {
                    pt_class = Some(PT_PEAK);
                    pt_value = 1; // marker; elevation filled below
                }
                "spring" => pt_class = Some(PT_SPRING),
                _ => {}
            },
            "ele" => {
                if pt_class == Some(PT_PEAK) {
                    pt_value = leading_u32(v);
                }
            }
            "highway" => match *v {
                "street_lamp" => pt_class = Some(PT_LAMP),
                "motorway_junction" => pt_class = Some(PT_JUNCTION),
                "turning_circle" | "turning_loop" => pt_class = Some(PT_TURNING),
                _ => {}
            },
            "railway" => match *v {
                "level_crossing" | "crossing" => pt_class = Some(PT_CROSSING),
                "station" | "halt" => pt_class = Some(PT_STATION),
                _ => {}
            },
            "power" => match *v {
                "tower" | "catenary_mast" => pt_class = Some(PT_TOWER),
                "pole" => pt_class = Some(PT_POLE),
                _ => {}
            },
            "entrance" => pt_class = Some(PT_ENTRANCE),
            "waterway" if *v == "waterfall" => pt_class = Some(PT_WATERFALL),
            "addr:housenumber" => {
                if pt_class.is_none() {
                    pt_class = Some(PT_ADDRESS);
                }
            }
            _ => {}
        }
    }
    if let Some(pclass) = place {
        if !name.is_empty() {
            places.push(Place {
                id: t.id,
                name: name.clone(),
                class: pclass,
                capital,
                population,
                idx,
            });
        }
    }
    if let Some(pc) = pt_class {
        let label = match pc {
            PT_ADDRESS => t
                .tags
                .iter()
                .find(|(k, _)| *k == "addr:housenumber")
                .map(|(_, v)| (*v).to_string())
                .unwrap_or_default(),
            PT_JUNCTION => t
                .tags
                .iter()
                .find(|(k, _)| *k == "ref")
                .map(|(_, v)| (*v).to_string())
                .unwrap_or_default(),
            _ => name.clone(),
        };
        points.push(FeatPoint { id: t.id, class: pc, name: label, value: pt_value, idx });
    }
    if let Some(cat) = category {
        if !name.is_empty() {
            pois.push(Poi { id: t.id, name, category: cat, idx });
        }
    }
}

fn serde_json_to_string(v: &RegistrySummary) -> String {
    // Tiny hand-rolled JSON to avoid a serde_json dep in this crate.
    format!(
        "{{\"source\":\"{}\",\"bbox\":[{},{},{},{}],\"nodes\":{},\"roads\":{},\"named_roads\":{},\"railways\":{},\"waterways\":{},\"water_areas\":{},\"landuse\":{},\"buildings\":{},\"pois\":{},\"places\":{},\"admin\":{},\"power\":{},\"aeroways\":{},\"barriers\":{},\"piers\":{},\"water_barriers\":{},\"tree_rows\":{},\"ferry\":{},\"misc\":{},\"road_areas\":{},\"marinas\":{},\"protected\":{},\"points\":{},\"ocean\":{},\"islands\":{}}}",
        v.source.replace('\\', "\\\\").replace('"', "\\\""),
        v.bbox[0], v.bbox[1], v.bbox[2], v.bbox[3],
        v.nodes, v.roads, v.named_roads, v.railways, v.waterways, v.water_areas, v.landuse,
        v.buildings, v.pois, v.places, v.admin, v.power, v.aeroways, v.barriers, v.piers,
        v.water_barriers, v.tree_rows, v.ferry, v.misc, v.road_areas, v.marinas, v.protected,
        v.points, v.ocean, v.islands
    )
}

#[derive(Serialize, Deserialize)]
struct RegistrySummary {
    source: String,
    bbox: [f64; 4],
    nodes: usize,
    roads: usize,
    named_roads: usize,
    railways: usize,
    waterways: usize,
    water_areas: usize,
    landuse: usize,
    buildings: usize,
    pois: usize,
    places: usize,
    admin: usize,
    power: usize,
    aeroways: usize,
    barriers: usize,
    piers: usize,
    water_barriers: usize,
    tree_rows: usize,
    ferry: usize,
    misc: usize,
    road_areas: usize,
    marinas: usize,
    protected: usize,
    points: usize,
    ocean: usize,
    islands: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oneway_and_speed_tags() {
        assert_eq!(oneway_code(Some("yes")), 1);
        assert_eq!(oneway_code(Some("1")), 1);
        assert_eq!(oneway_code(Some("-1")), 2);
        assert_eq!(oneway_code(Some("reverse")), 2);
        assert_eq!(oneway_code(Some("no")), 0);
        assert_eq!(oneway_code(None), 0);

        assert_eq!(maxspeed_kmh(Some("50")), 50);
        assert_eq!(maxspeed_kmh(Some("50 knots")), 50);
        assert_eq!(maxspeed_kmh(Some("50|60")), 50);
        assert_eq!(maxspeed_kmh(Some("RU:urban")), 0);
        assert_eq!(maxspeed_kmh(Some("walk")), 0);
        assert_eq!(maxspeed_kmh(None), 0);

        assert_eq!(lanes_count(Some("2")), 2);
        assert_eq!(lanes_count(Some("2;3")), 2);
        assert_eq!(lanes_count(Some("99")), 30);
        assert_eq!(lanes_count(None), 0);

        assert_eq!(leading_u32("1,234 m"), 1234);
        assert_eq!(leading_u32("≈2000"), 2000);
        assert_eq!(leading_u32("none"), 0);
    }

    #[test]
    fn class_roundtrip() {
        for (i, name) in ROAD_CLASSES.iter().enumerate() {
            if name == &"rail" {
                assert_eq!(i as u8, CLASS_RAIL);
                continue;
            }
            assert_eq!(road_class_index(name), Some(i as u8));
            assert_eq!(road_class_name(i as u8), *name);
        }
        assert_eq!(road_class_index("motorway"), Some(0));
        assert_eq!(road_class_index("bogus"), None);
        assert_eq!(road_class_name(200), "unclassified");

        assert_eq!(waterway_class("river"), WATER_RIVER);
        assert_eq!(waterway_class("stream"), WATER_STREAM);
        assert_eq!(waterway_class("drain"), WATER_DRAIN);
        assert_eq!(waterway_class("weird"), WATER_OTHER);
    }

    #[test]
    fn place_classes() {
        assert_eq!(place_class("country"), Some(PL_COUNTRY));
        assert_eq!(place_class("city"), Some(PL_CITY));
        assert_eq!(place_class("village"), Some(PL_VILLAGE));
        assert_eq!(place_class("bogus"), None);
    }

    #[test]
    fn ocean_closure_picks_the_sea_side() {
        // Coast running south along x=39. OSM coastlines keep water on the
        // RIGHT of the walk direction: heading south, the sea is WEST.
        let bbox = [38.0, -7.0, 40.0, -5.0];
        let chain = vec![(39.0, -5.0), (39.0, -6.0), (39.0, -7.0)];
        let (is_water, ring) = close_coast_ring(&chain, bbox).unwrap();
        assert!(is_water, "southward coast must close as sea");
        let area = ring_signed_area(&ring);
        assert!(area < 0.0, "expected clockwise ring, got {area}");
        assert!(ring.iter().any(|&(x, _)| x < 38.5), "ring must use the west edge");
        assert!(ring.iter().all(|&(x, _)| x <= 39.001), "ring leaked east");
    }

    #[test]
    fn ocean_closure_reverse_direction() {
        // Same coast walked northward — sea on the right is now EAST.
        let bbox = [38.0, -7.0, 40.0, -5.0];
        let chain = vec![(39.0, -7.0), (39.0, -6.0), (39.0, -5.0)];
        let (is_water, ring) = close_coast_ring(&chain, bbox).unwrap();
        assert!(is_water);
        assert!(ring_signed_area(&ring) < 0.0);
        assert!(ring.iter().any(|&(x, _)| x > 39.5));
        assert!(ring.iter().all(|&(x, _)| x >= 38.999));
    }

    #[test]
    fn closed_chain_orientation_classifies_land_and_sea() {
        let bbox = [38.0, -7.0, 40.0, -5.0];
        // Counter-clockwise ring (east, north, west, south — positive
        // shoelace; interior on the LEFT of the walk, water on the right) =
        // island land.
        let ccw = vec![(39.0, -6.5), (39.5, -6.5), (39.5, -6.0), (39.0, -6.0), (39.0, -6.5)];
        let rings = build_ocean_rings(vec![ccw], bbox);
        assert_eq!(rings.len(), 1);
        assert!(rings[0].0, "CCW ring should be an island");
        // Clockwise ring (interior on the RIGHT = water side) = sea.
        let cw = vec![(39.0, -6.0), (39.5, -6.0), (39.5, -6.5), (39.0, -6.5), (39.0, -6.0)];
        let rings = build_ocean_rings(vec![cw], bbox);
        assert_eq!(rings.len(), 1);
        assert!(!rings[0].0, "CW ring should be sea");
    }

    #[test]
    fn anti_flow_chain_still_closes_on_the_sea_side() {
        // The mainland coast of Tanzania runs southward in OSM flow (sea on
        // the right = west). Simulate a chain that was joined backwards
        // (flow = -1): normalization must flip it before closing.
        let bbox = [38.0, -7.0, 40.0, -5.0];
        // northward physical sequence; in flow the sea would be EAST, so a
        // northward-flow chain closes east; the same geometry walked
        // anti-flow must still close east after normalization.
        let north = vec![(39.0, -7.0), (39.0, -6.0), (39.0, -5.0)];
        let rings = build_ocean_rings(vec![north], bbox);
        assert_eq!(rings.len(), 1);
        assert!(!rings[0].0, "open northward coast closes as ocean");
        assert!(rings[0].1.iter().any(|&(x, _)| x > 39.5), "sea sliver must be east");
    }

    #[test]
    fn chains_join_across_ways() {
        let bbox = [38.0, -7.0, 40.0, -5.0];
        // Two ways forming one N-S coast walked southward — sea on the right
        // (west side, like ocean_closure_picks_the_sea_side).
        let a = vec![(39.0, -5.0), (39.0, -6.0)];
        let b = vec![(39.0, -6.0), (39.0, -7.0)];
        let rings = build_ocean_rings(vec![b, a], bbox);
        assert_eq!(rings.len(), 1);
        assert!(!rings[0].0, "joined open chain should close as ocean");
        assert!(rings[0].1.iter().any(|&(x, _)| x < 38.5), "ring must take the west edge");
    }
}
