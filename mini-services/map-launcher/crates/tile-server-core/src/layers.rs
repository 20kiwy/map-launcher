//! The layer engine: the static catalog of map layers the renderer can draw
//! (grouped like openstreetmap-carto's style sheets) plus the runtime
//! overrides (visibility, opacity, zoom range, colour) the UI edits and the
//! renderer honours.

use std::collections::HashMap;

use carto_style::Rgba;

/// One entry of the layer tree: a group (holds children) or a paintable layer.
#[derive(Clone, Copy, Debug)]
pub struct LayerDef {
    /// Stable id used by [`LayerVis`] and the renderer.
    pub id: &'static str,
    /// Panel name.
    pub name: &'static str,
    /// Parent group id (`""` for a top-level group).
    pub parent: &'static str,
    /// The OSM vector path this layer paints (filter expression).
    pub vector_path: &'static str,
    /// `base` | `fill` | `stroke` | `label` | `engine`.
    pub kind: &'static str,
    /// openstreetmap-carto palette variable supplying the default colour.
    pub color_var: Option<&'static str>,
    /// Fallback colour when the palette variable is missing.
    pub fallback: [u8; 3],
    /// Zoom range from the style sheets.
    pub min_zoom: u8,
    pub max_zoom: u8,
}

/// The full layer catalog, groups first, children after their parent.
pub fn layer_catalog() -> Vec<LayerDef> {
    vec![
        // -- base ----------------------------------------------------------
        LayerDef { id: "background", name: "Background", parent: "", vector_path: "— (land polygon)", kind: "base", color_var: Some("land-color"), fallback: [0xf2, 0xef, 0xe9], min_zoom: 0, max_zoom: 19 },
        // -- landuse -------------------------------------------------------
        LayerDef { id: "landuse", name: "Landuse", parent: "", vector_path: "landuse=* / natural=wood,scrub / leisure=park,pitch / amenity=*", kind: "fill", color_var: None, fallback: [0xe8, 0xdf, 0xd6], min_zoom: 8, max_zoom: 19 },
        // -- water ---------------------------------------------------------
        LayerDef { id: "water", name: "Water", parent: "", vector_path: "natural=water / waterway=*", kind: "group", color_var: Some("water-color"), fallback: [0xaa, 0xd3, 0xdf], min_zoom: 7, max_zoom: 19 },
        LayerDef { id: "water-areas", name: "Water areas", parent: "water", vector_path: "natural=water / landuse=reservoir,basin", kind: "fill", color_var: Some("water-color"), fallback: [0xaa, 0xd3, 0xdf], min_zoom: 7, max_zoom: 19 },
        LayerDef { id: "waterways", name: "Waterways", parent: "water", vector_path: "waterway=river,stream,canal,ditch,…", kind: "stroke", color_var: Some("water-color"), fallback: [0xaa, 0xd3, 0xdf], min_zoom: 8, max_zoom: 19 },
        // -- roads ---------------------------------------------------------
        LayerDef { id: "roads", name: "Roads", parent: "", vector_path: "highway=*", kind: "group", color_var: Some("motorway-fill"), fallback: [0xe8, 0x9a, 0x51], min_zoom: 5, max_zoom: 19 },
        LayerDef { id: "road-casing", name: "Road casing", parent: "roads", vector_path: "highway=* (casing line)", kind: "stroke", color_var: Some("motorway-casing"), fallback: [0xe0, 0xb8, 0x91], min_zoom: 12, max_zoom: 19 },
        LayerDef { id: "road-fill", name: "Road fill", parent: "roads", vector_path: "highway=motorway,trunk,primary,…,residential,track,path", kind: "stroke", color_var: Some("motorway-fill"), fallback: [0xe8, 0x9a, 0x51], min_zoom: 5, max_zoom: 19 },
        LayerDef { id: "road-oneway", name: "Oneway arrows", parent: "roads", vector_path: "highway=* oneway=yes/-1 (engine)", kind: "engine", color_var: None, fallback: [0x5b, 0x5b, 0x5b], min_zoom: 16, max_zoom: 19 },
        LayerDef { id: "road-labels", name: "Road names & shields", parent: "roads", vector_path: "highway=* name=* (engine text)", kind: "label", color_var: Some("motorway-shield"), fallback: [0x3b, 0x3b, 0x3b], min_zoom: 13, max_zoom: 19 },
        // -- rail ----------------------------------------------------------
        LayerDef { id: "rail", name: "Railways", parent: "", vector_path: "railway=rail,spur,…", kind: "stroke", color_var: None, fallback: [0x8a, 0x8a, 0x8a], min_zoom: 9, max_zoom: 19 },
        // -- buildings -----------------------------------------------------
        LayerDef { id: "buildings", name: "Buildings", parent: "", vector_path: "building=*", kind: "fill", color_var: Some("building-fill"), fallback: [0xd9, 0xd0, 0xc9], min_zoom: 13, max_zoom: 19 },
        // -- pois ----------------------------------------------------------
        LayerDef { id: "pois", name: "Points of interest", parent: "", vector_path: "amenity=* / shop=* / tourism=* / office=…", kind: "label", color_var: Some("motorway-casing"), fallback: [0xdc, 0x2a, 0x67], min_zoom: 14, max_zoom: 19 },
    ]
}

/// Runtime overrides for the layer engine, edited by the Layers panel and
/// honoured by the tile renderer. Unlisted layers render with their defaults.
#[derive(Clone, Debug, Default)]
pub struct LayerVis {
    /// `id -> visible` (missing = visible).
    pub visible: HashMap<String, bool>,
    /// `id -> opacity 0..=1` (missing = 1.0).
    pub opacity: HashMap<String, f32>,
    /// `id -> (min_zoom, max_zoom)` override (missing = catalog default).
    pub zoom: HashMap<String, (u8, u8)>,
    /// `id -> rgb` colour override (missing = palette colour).
    pub color: HashMap<String, [u8; 3]>,
    /// Bumped on every edit so canvases can invalidate their caches.
    pub version: u64,
}

impl LayerVis {
    pub fn touch(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    pub fn is_visible(&self, id: &str) -> bool {
        self.visible.get(id).copied().unwrap_or(true)
    }

    /// Whether layer `id` paints at zoom `z` (visibility + zoom override).
    pub fn on(&self, id: &str, z: u8) -> bool {
        if !self.is_visible(id) {
            return false;
        }
        if let Some((mn, mx)) = self.zoom.get(id) {
            return z >= *mn && z <= *mx;
        }
        true
    }

    /// Whether layer `id` paints at zoom `z`, also honouring the catalog's
    /// default zoom window.
    pub fn on_with_default(&self, id: &str, def_min: u8, def_max: u8, z: u8) -> bool {
        if !self.is_visible(id) {
            return false;
        }
        match self.zoom.get(id) {
            Some((mn, mx)) => z >= *mn && z <= *mx,
            None => z >= def_min && z <= def_max,
        }
    }

    pub fn alpha(&self, id: &str) -> f32 {
        self.opacity.get(id).copied().unwrap_or(1.0).clamp(0.0, 1.0)
    }

    /// The effective colour: override if set, else `base`.
    pub fn color(&self, id: &str, base: Rgba) -> Rgba {
        match self.color.get(id) {
            Some([r, g, b]) => Rgba::rgb(*r, *g, *b),
            None => base,
        }
    }

    /// The raw override colour for the properties panel, if any.
    pub fn color_override(&self, id: &str) -> Option<[u8; 3]> {
        self.color.get(id).copied()
    }

    pub fn clear_all(&mut self) {
        self.visible.clear();
        self.opacity.clear();
        self.zoom.clear();
        self.color.clear();
        self.touch();
    }
}
