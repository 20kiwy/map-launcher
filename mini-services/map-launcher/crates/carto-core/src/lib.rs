//! Central-Tanzania OSM registry: data model, PBF extraction and
//! bincode persistence for the map-launcher tile server.
//!
//! The extractor consumes a `*.osm.pbf` (Geofabrik Tanzania), keeps every
//! node inside the Dodoma / Central-Tanzania bbox and resolves all ways to
//! node indices, so the server never needs an id lookup at runtime.

use bincode::{deserialize_from, serialize_into};
use carto_math::haversine_m;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

/// Central Tanzania (Dodoma region) test-data bbox [w, s, e, n].
pub const DODOMA_BBOX: [f64; 4] = [35.1, -7.2, 36.7, -5.4];

/* --------------------------------------------------------------------- */
/* Data model                                                              */
/* --------------------------------------------------------------------- */

/// Road classes matching the openstreetmap-carto roads palette keys
/// (serialized as u8; index into ROAD_CLASSES).
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
        "residential" => 9,
        "unclassified" | "road" => 10,
        "living_street" => 11,
        "pedestrian" => 12,
        "service" => 13,
        "track" => 14,
        "footway" => 15,
        "path" => 16,
        "cycleway" => 17,
        "steps" => 18,
        _ => return None,
    };
    Some(idx)
}

pub fn road_class_name(idx: u8) -> &'static str {
    ROAD_CLASSES.get(idx as usize).copied().unwrap_or("unclassified")
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
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Area {
    pub id: i64,
    /// e.g. "forest", "farmland", "residential", "" for plain buildings
    pub kind: String,
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
}

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

use std::io::BufReader;

/* --------------------------------------------------------------------- */
/* Extraction                                                              */
/* --------------------------------------------------------------------- */

#[derive(Clone)]
enum Kind {
    Road(u8),
    Railway,
    Waterway,
    Water,
    Landuse(String),
    Building,
}

struct RawWay {
    id: i64,
    kind: Kind,
    name: String,
    oneway: u8,
    lanes: u8,
    maxspeed: u16,
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

fn classify_way<'a, F: Fn(&str) -> Option<&'a str>>(tags: F, id: i64) -> Option<RawWay> {
    let name = tags("name").unwrap_or("").to_string();
    let oneway = oneway_code(tags("oneway"));
    let lanes = lanes_count(tags("lanes"));
    let maxspeed = maxspeed_kmh(tags("maxspeed"));
    let kind = if let Some(hw) = tags("highway") {
        Kind::Road(road_class_index(hw)?)
    } else if tags("railway").is_some() {
        Kind::Railway
    } else if let Some(w) = tags("waterway") {
        if w == "riverbank" {
            Kind::Water
        } else {
            Kind::Waterway
        }
    } else if tags("building").is_some() || tags("building:part").is_some() {
        Kind::Building
    } else if let Some(lu) = tags("landuse") {
        Kind::Landuse(lu.to_string())
    } else if let Some(nat) = tags("natural") {
        match nat {
            "water" | "wetland" | "pond" | "lake" => Kind::Water,
            "wood" => Kind::Landuse("forest".into()),
            "scrub" => Kind::Landuse("scrub".into()),
            "grassland" => Kind::Landuse("grass".into()),
            "sand" => Kind::Landuse("sand".into()),
            "beach" => Kind::Landuse("beach".into()),
            _ => return None,
        }
    } else if let Some(le) = tags("leisure") {
        match le {
            "park" | "garden" | "pitch" | "golf_course" | "playground" => {
                Kind::Landuse(le.to_string())
            }
            _ => return None,
        }
    } else if let Some(am) = tags("amenity") {
        match am {
            "school" | "hospital" | "college" | "university" | "place_of_worship" | "marketplace" => {
                Kind::Landuse(am.into())
            }
            _ => return None,
        }
    } else {
        return None;
    };
    Some(RawWay { id, kind, name, oneway, lanes, maxspeed, pts: Vec::new() })
}

/// Extract the Dodoma-bbox dataset from a Geofabrik `.osm.pbf` and write
/// `store.bin` + `registry.json` into `out_dir`.
pub fn extract(pbf: &Path, out_dir: &Path, bbox: [f64; 4]) -> std::io::Result<Store> {
    let (w, s, e, n) = (bbox[0], bbox[1], bbox[2], bbox[3]);
    let mut node_id_to_idx: HashMap<i64, u32> = HashMap::with_capacity(5_000_000);
    let mut lon: Vec<f32> = Vec::with_capacity(5_000_000);
    let mut lat: Vec<f32> = Vec::with_capacity(5_000_000);
    let mut pois: Vec<Poi> = Vec::new();
    let mut ways: Vec<RawWay> = Vec::new();

    let mut node_count = 0usize;
    let reader = osmpbf::ElementReader::from_path(pbf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    reader.for_each(|el| match el {
        osmpbf::Element::Node(node) => {
            node_count += 1;
            let (x, y) = (node.lon(), node.lat());
            if x >= w && x <= e && y >= s && y <= n {
                let idx = lon.len() as u32;
                node_id_to_idx.insert(node.id(), idx);
                lon.push(x as f32);
                lat.push(y as f32);
                // POI capture (named amenity/shop/tourism/leisure/healthcare nodes)
                let mut name = String::new();
                let mut category: Option<String> = None;
                for (k, v) in node.tags() {
                    match k {
                        "name" => name = v.to_string(),
                        "amenity" | "shop" | "tourism" | "leisure" | "healthcare" | "craft"
                        | "office" => {
                            if category.is_none() {
                                category = Some(format!("{k}_{v}"));
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(cat) = category {
                    if !name.is_empty() {
                        pois.push(Poi { id: node.id(), name, category: cat, idx });
                    }
                }
            }
            if node_count % 10_000_000 == 0 {
                eprintln!("  nodes scanned: {node_count}…");
            }
        }
        osmpbf::Element::DenseNode(d) => {
            node_count += 1;
            let (x, y) = (d.lon(), d.lat());
            if x >= w && x <= e && y <= n && y >= s {
                let idx = lon.len() as u32;
                node_id_to_idx.insert(d.id(), idx);
                lon.push(x as f32);
                lat.push(y as f32);
                let mut name = String::new();
                let mut category: Option<String> = None;
                for (k, v) in d.tags() {
                    match k {
                        "name" => name = v.to_string(),
                        "amenity" | "shop" | "tourism" | "leisure" | "healthcare" | "craft"
                        | "office" => {
                            if category.is_none() {
                                category = Some(format!("{k}_{v}"));
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(cat) = category {
                    if !name.is_empty() {
                        pois.push(Poi { id: d.id(), name, category: cat, idx });
                    }
                }
            }
            if node_count % 10_000_000 == 0 {
                eprintln!("  nodes scanned: {node_count}…");
            }
        }
        osmpbf::Element::Way(way) => {
            let get = |key: &str| -> Option<&str> {
                way.tags().find(|(k, _)| *k == key).map(|(_, v)| v)
            };
            // Geofabrik PBFs list all nodes before ways, so the node map is
            // already complete here — resolve immediately and keep only ways
            // that are substantially inside the bbox (bounds memory).
            if let Some(mut rw) = classify_way(get, way.id()) {
                let mut pts: Vec<u32> = Vec::with_capacity(8);
                let mut total = 0usize;
                for r in way.refs() {
                    total += 1;
                    if let Some(&i) = node_id_to_idx.get(&r) {
                        pts.push(i);
                    }
                }
                if pts.len() >= 2 && pts.len() * 3 >= total {
                    rw.pts = pts;
                    ways.push(rw);
                }
            }
        }
        osmpbf::Element::Relation(_) => {}
    })
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    eprintln!(
        "  scanned {node_count} nodes; kept {} in bbox; {} ways; {} pois",
        node_id_to_idx.len(),
        ways.len(),
        pois.len()
    );

    let mut store = Store::empty(bbox);
    store.lon = lon;
    store.lat = lat;
    store.pois = pois;

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
        match wway.kind {
            Kind::Road(class) => store.roads.push(Line {
                id: wway.id,
                name: wway.name,
                class,
                length_m: length as f32,
                bbox: bb,
                pts,
                oneway: wway.oneway,
                lanes: wway.lanes,
                maxspeed: wway.maxspeed,
            }),
            Kind::Railway => store.railways.push(Line {
                id: wway.id,
                name: wway.name,
                class: 19,
                length_m: length as f32,
                bbox: bb,
                pts,
                oneway: 0,
                lanes: 0,
                maxspeed: 0,
            }),
            Kind::Waterway => store.waterways.push(Line {
                id: wway.id,
                name: wway.name,
                class: 0,
                length_m: length as f32,
                bbox: bb,
                pts,
                oneway: 0,
                lanes: 0,
                maxspeed: 0,
            }),
            Kind::Water => store.water_areas.push(Area { id: wway.id, kind: "water".into(), bbox: bb, pts }),
            Kind::Landuse(kind) => store.landuse.push(Area { id: wway.id, kind, bbox: bb, pts }),
            Kind::Building => store.buildings.push(Area { id: wway.id, kind: String::new(), bbox: bb, pts }),
        }
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
    });
    fs::create_dir_all(out_dir)?;
    fs::write(out_dir.join("registry.json"), summary)?;
    store.save(out_dir)?;
    eprintln!("  store written: {} roads ({named} named)", store.roads.len());
    Ok(store)
}

fn serde_json_to_string(v: &RegistrySummary) -> String {
    // Tiny hand-rolled JSON to avoid a serde_json dep in this crate.
    format!(
        "{{\"source\":\"{}\",\"bbox\":[{},{},{},{}],\"nodes\":{},\"roads\":{},\"named_roads\":{},\"railways\":{},\"waterways\":{},\"water_areas\":{},\"landuse\":{},\"buildings\":{},\"pois\":{}}}",
        v.source.replace('\\', "\\\\").replace('"', "\\\""),
        v.bbox[0], v.bbox[1], v.bbox[2], v.bbox[3],
        v.nodes, v.roads, v.named_roads, v.railways, v.waterways, v.water_areas, v.landuse,
        v.buildings, v.pois
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
    }

    #[test]
    fn class_roundtrip() {
        for (i, name) in ROAD_CLASSES.iter().enumerate() {
            if name == &"rail" {
                continue;
            }
            assert_eq!(road_class_index(name), Some(i as u8));
            assert_eq!(road_class_name(i as u8), *name);
        }
        assert_eq!(road_class_index("motorway"), Some(0));
        assert_eq!(road_class_index("bogus"), None);
        assert_eq!(road_class_name(200), "unclassified");
    }
}
