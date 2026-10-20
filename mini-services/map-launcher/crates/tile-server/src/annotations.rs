//! User-drawn map annotations (polygons and paths) with persistence and
//! GeoJSON export. Drawn on the Map canvas with the vectorcraft-style pen
//! tools; managed from the Layers panel tree under the `Annotations` group.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::settings::config_dir;

/// One user drawing: a closed polygon or an open path (polyline), stored in
/// lon/lat so it survives pan/zoom and exports cleanly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Anno {
    pub id: u64,
    pub name: String,
    /// `"polygon"` | `"path"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Vertices as `(lon, lat)`.
    pub pts: Vec<(f64, f64)>,
    /// Fill/stroke colour as rgb.
    pub color: [u8; 3],
    /// 0..=1.
    pub opacity: f32,
    /// Stroke width in px.
    pub width: f32,
    /// Whether a polygon paints its fill (paths always stroke-only).
    pub filled: bool,
    pub visible: bool,
    pub locked: bool,
}

impl Anno {
    pub fn next_id(all: &[Anno]) -> u64 {
        all.iter().map(|a| a.id).max().unwrap_or(0) + 1
    }

    /// Rough ground length in metres (haversine over the segments).
    pub fn length_m(&self) -> f64 {
        self.pts
            .windows(2)
            .map(|w| haversine(w[0].1, w[0].0, w[1].1, w[1].0))
            .sum()
    }

    /// Rough planar area in m² (equirectangular shoelace around the centroid).
    pub fn area_m2(&self) -> f64 {
        if self.kind != "polygon" || self.pts.len() < 3 {
            return 0.0;
        }
        let lat0 = self.pts.iter().map(|p| p.1).sum::<f64>() / self.pts.len() as f64;
        let k = 111_320.0 * lat0.to_radians().cos();
        let pts: Vec<(f64, f64)> = self.pts.iter().map(|&(lon, lat)| (lon * k, lat * 110_574.0)).collect();
        let mut acc = 0.0;
        for (i, a) in pts.iter().enumerate() {
            let b = pts[(i + 1) % pts.len()];
            acc += a.0 * b.1 - b.0 * a.1;
        }
        (acc / 2.0).abs()
    }
}

fn haversine(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (la1, la2) = (lat1.to_radians(), lat2.to_radians());
    let dla = (lat2 - lat1).to_radians();
    let dlo = (lon2 - lon1).to_radians();
    let a = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    2.0 * 6_371_000.0 * a.sqrt().asin()
}

fn annos_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("annotations.json"))
}

/// Load the saved drawings (an empty list when none — never an error).
pub fn load() -> Vec<Anno> {
    let Some(text) = annos_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return vec![] };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Persist the drawings (a small file — safe to call on every edit).
pub fn save(all: &[Anno]) {
    let Some(dir) = config_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string_pretty(all) {
        let _ = std::fs::write(dir.join("annotations.json"), json);
    }
}

/// Write every drawing as a GeoJSON FeatureCollection next to the data
/// folder; returns the path it wrote.
pub fn export_geojson(all: &[Anno], workspace: &std::path::Path) -> Option<PathBuf> {
    let features: Vec<String> = all
        .iter()
        .map(|a| {
            let ring: Vec<String> = a.pts.iter().map(|&(lon, lat)| format!("[{lon:.7},{lat:.7}]")).collect();
            let (geom, ring) = if a.kind == "polygon" {
                ("Polygon", format!("[[{}]]", ring.join(",")))
            } else {
                ("LineString", format!("[{}]", ring.join(",")))
            };
            format!(
                r#"{{"type":"Feature","properties":{{"name":{},"type":"{}"}},"geometry":{{"type":"{}","coordinates":{}}}}}"#,
                serde_json::to_string(&a.name).unwrap_or_else(|_| "\"\"".into()),
                a.kind,
                geom,
                ring
            )
        })
        .collect();
    let json = format!(r#"{{"type":"FeatureCollection","features":[{}]}}"#, features.join(","));
    let path = workspace.join("annotations.geojson");
    std::fs::write(&path, json).ok()?;
    Some(path)
}
