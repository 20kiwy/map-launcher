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

/// The full layer catalog, groups first, children after their parent — the
/// complete openstreetmap-carto section tree (every project.mml layer has a
/// row). Leaf ids are stable: the renderer and `/api/layers` key on them.
pub fn layer_catalog() -> Vec<LayerDef> {
    vec![
        // -- base ----------------------------------------------------------
        LayerDef { id: "background", name: "Background", parent: "", vector_path: "— (land polygon)", kind: "base", color_var: Some("land-color"), fallback: [0xf2, 0xef, 0xe9], min_zoom: 0, max_zoom: 19 },
        // -- terrain -------------------------------------------------------
        LayerDef { id: "terrain", name: "Terrain", parent: "", vector_path: "landuse=* / natural=wood,scrub / leisure=park,pitch", kind: "group", color_var: None, fallback: [0xe8, 0xdf, 0xd6], min_zoom: 4, max_zoom: 19 },
        LayerDef { id: "landuse", name: "Landuse fills", parent: "terrain", vector_path: "landuse=* / natural=wood,scrub,heath,sand / leisure=park,pitch / amenity=*", kind: "fill", color_var: None, fallback: [0xe8, 0xdf, 0xd6], min_zoom: 4, max_zoom: 19 },
        LayerDef { id: "landcover-patterns", name: "Pattern fills", parent: "terrain", vector_path: "forest / orchard / vineyard / quarry / cemetery (patterns)", kind: "fill", color_var: None, fallback: [0xad, 0xd1, 0x9e], min_zoom: 9, max_zoom: 19 },
        LayerDef { id: "protected-areas", name: "Protected areas", parent: "terrain", vector_path: "boundary=protected_area / leisure=nature_reserve", kind: "fill", color_var: Some("protected-areas"), fallback: [0x9c, 0xd4, 0x9c], min_zoom: 9, max_zoom: 19 },
        LayerDef { id: "tree-rows", name: "Tree rows & hedges", parent: "terrain", vector_path: "natural=tree_row,hedge", kind: "stroke", color_var: Some("forest"), fallback: [0x6b, 0xa3, 0x68], min_zoom: 16, max_zoom: 19 },
        LayerDef { id: "misc", name: "Golf / tracks / rides", parent: "terrain", vector_path: "golf=* / leisure=track / attraction=waterslide / roller_coaster=*", kind: "stroke", color_var: None, fallback: [0x9b, 0xd0, 0x9b], min_zoom: 13, max_zoom: 19 },
        // -- water ---------------------------------------------------------
        LayerDef { id: "water", name: "Water", parent: "", vector_path: "natural=water / waterway=* / coastline", kind: "group", color_var: Some("water-color"), fallback: [0xaa, 0xd3, 0xdf], min_zoom: 0, max_zoom: 19 },
        LayerDef { id: "ocean", name: "Ocean", parent: "water", vector_path: "natural=coastline (closed rings)", kind: "fill", color_var: Some("water-color"), fallback: [0xaa, 0xd3, 0xdf], min_zoom: 0, max_zoom: 19 },
        LayerDef { id: "islands", name: "Islands", parent: "water", vector_path: "natural=coastline (closed rings, land)", kind: "fill", color_var: Some("land-color"), fallback: [0xf2, 0xef, 0xe9], min_zoom: 0, max_zoom: 19 },
        LayerDef { id: "water-areas", name: "Water areas", parent: "water", vector_path: "natural=water / landuse=reservoir,basin", kind: "fill", color_var: Some("water-color"), fallback: [0xaa, 0xd3, 0xdf], min_zoom: 7, max_zoom: 19 },
        LayerDef { id: "waterways", name: "Waterways", parent: "water", vector_path: "waterway=river,canal,stream,ditch,drain", kind: "stroke", color_var: Some("water-color"), fallback: [0xaa, 0xd3, 0xdf], min_zoom: 5, max_zoom: 19 },
        LayerDef { id: "waterways-text", name: "Waterway names", parent: "water", vector_path: "waterway=* name=* (along path)", kind: "label", color_var: None, fallback: [0x4d, 0x70, 0x8a], min_zoom: 13, max_zoom: 19 },
        LayerDef { id: "water-barriers", name: "Dams & weirs", parent: "water", vector_path: "waterway=dam,weir / man_made=groyne,breakwater", kind: "fill", color_var: Some("dam"), fallback: [0xad, 0xad, 0xad], min_zoom: 13, max_zoom: 19 },
        LayerDef { id: "piers", name: "Piers", parent: "water", vector_path: "man_made=pier", kind: "fill", color_var: Some("pier"), fallback: [0xad, 0xad, 0xad], min_zoom: 12, max_zoom: 19 },
        // -- transport -----------------------------------------------------
        LayerDef { id: "transport", name: "Transport", parent: "", vector_path: "highway=* / railway=* / aeroway=* / route=ferry", kind: "group", color_var: Some("motorway-fill"), fallback: [0xe8, 0x9a, 0x51], min_zoom: 5, max_zoom: 19 },
        LayerDef { id: "roads", name: "Roads", parent: "transport", vector_path: "highway=*", kind: "group", color_var: Some("motorway-fill"), fallback: [0xe8, 0x9a, 0x51], min_zoom: 5, max_zoom: 19 },
        LayerDef { id: "road-casing", name: "Road casing", parent: "roads", vector_path: "highway=* (casing line)", kind: "stroke", color_var: Some("motorway-casing"), fallback: [0xe0, 0xb8, 0x91], min_zoom: 12, max_zoom: 19 },
        LayerDef { id: "road-fill", name: "Road fill", parent: "roads", vector_path: "highway=motorway,…,path (dashes for paths)", kind: "stroke", color_var: Some("motorway-fill"), fallback: [0xe8, 0x9a, 0x51], min_zoom: 5, max_zoom: 19 },
        LayerDef { id: "road-areas", name: "Highway areas", parent: "roads", vector_path: "highway=* area=yes (plazas)", kind: "fill", color_var: Some("pedestrian-fill"), fallback: [0xdd, 0xdd, 0xdd], min_zoom: 13, max_zoom: 19 },
        LayerDef { id: "road-oneway", name: "Oneway arrows", parent: "roads", vector_path: "highway=* oneway=yes/-1 (engine)", kind: "engine", color_var: None, fallback: [0x5b, 0x5b, 0x5b], min_zoom: 16, max_zoom: 19 },
        LayerDef { id: "road-labels", name: "Road names", parent: "roads", vector_path: "highway=* name=* (along path)", kind: "label", color_var: None, fallback: [0x33, 0x33, 0x33], min_zoom: 13, max_zoom: 19 },
        LayerDef { id: "road-refs", name: "Ref shields", parent: "roads", vector_path: "highway=* ref=* (shield plates)", kind: "label", color_var: Some("motorway-shield"), fallback: [0x3b, 0x3b, 0x3b], min_zoom: 14, max_zoom: 19 },
        LayerDef { id: "tunnels", name: "Tunnels", parent: "roads", vector_path: "highway=* tunnel=yes (engine)", kind: "engine", color_var: None, fallback: [0x88, 0x88, 0x88], min_zoom: 12, max_zoom: 19 },
        LayerDef { id: "bridges", name: "Bridges", parent: "roads", vector_path: "highway=* bridge=yes (engine)", kind: "engine", color_var: None, fallback: [0x66, 0x66, 0x66], min_zoom: 12, max_zoom: 19 },
        LayerDef { id: "rail", name: "Railways", parent: "transport", vector_path: "railway=rail,spur,…", kind: "stroke", color_var: None, fallback: [0x8a, 0x8a, 0x8a], min_zoom: 9, max_zoom: 19 },
        LayerDef { id: "aeroways", name: "Aeroways", parent: "transport", vector_path: "aeroway=runway,taxiway,apron,helipad", kind: "fill", color_var: Some("runway"), fallback: [0xd5, 0xd3, 0xcc], min_zoom: 10, max_zoom: 19 },
        LayerDef { id: "ferry", name: "Ferry routes", parent: "transport", vector_path: "route=ferry (dashed)", kind: "stroke", color_var: Some("ferry-route"), fallback: [0x66, 0x66, 0xff], min_zoom: 10, max_zoom: 19 },
        LayerDef { id: "junctions", name: "Motorway junctions", parent: "transport", vector_path: "highway=motorway_junction ref=*", kind: "label", color_var: None, fallback: [0x33, 0x33, 0x33], min_zoom: 13, max_zoom: 19 },
        LayerDef { id: "crossings", name: "Level crossings", parent: "transport", vector_path: "railway=level_crossing,crossing", kind: "label", color_var: None, fallback: [0x33, 0x33, 0x33], min_zoom: 14, max_zoom: 19 },
        // -- buildings -----------------------------------------------------
        LayerDef { id: "buildings", name: "Buildings", parent: "", vector_path: "building=*", kind: "group", color_var: Some("building-fill"), fallback: [0xd9, 0xd0, 0xc9], min_zoom: 13, max_zoom: 19 },
        LayerDef { id: "building-text", name: "Building names", parent: "buildings", vector_path: "building=* name=* (z16+)", kind: "label", color_var: None, fallback: [0x6b, 0x66, 0x5f], min_zoom: 16, max_zoom: 19 },
        // -- boundaries ----------------------------------------------------
        LayerDef { id: "boundaries", name: "Boundaries", parent: "", vector_path: "boundary=administrative", kind: "group", color_var: Some("admin-boundaries"), fallback: [0x8d, 0x61, 0x8b], min_zoom: 4, max_zoom: 19 },
        LayerDef { id: "admin", name: "Admin boundaries", parent: "boundaries", vector_path: "boundary=administrative admin_level=2,4,…", kind: "stroke", color_var: Some("admin-boundaries"), fallback: [0x8d, 0x61, 0x8b], min_zoom: 4, max_zoom: 19 },
        // -- power ---------------------------------------------------------
        LayerDef { id: "power", name: "Power", parent: "", vector_path: "power=line,minor_line / power=tower", kind: "group", color_var: Some("power-line-color"), fallback: [0x88, 0x88, 0x88], min_zoom: 13, max_zoom: 19 },
        LayerDef { id: "power-lines", name: "Power lines", parent: "power", vector_path: "power=line,minor_line", kind: "stroke", color_var: Some("power-line-color"), fallback: [0x88, 0x88, 0x88], min_zoom: 13, max_zoom: 19 },
        LayerDef { id: "power-towers", name: "Towers & poles", parent: "power", vector_path: "power=tower,pole (sprites)", kind: "label", color_var: None, fallback: [0x88, 0x88, 0x88], min_zoom: 14, max_zoom: 19 },
        // -- barriers ------------------------------------------------------
        LayerDef { id: "barriers", name: "Barriers", parent: "", vector_path: "barrier=fence,wall,… / natural=cliff", kind: "group", color_var: None, fallback: [0x6a, 0x6a, 0x6a], min_zoom: 15, max_zoom: 19 },
        LayerDef { id: "fences", name: "Fences & walls", parent: "barriers", vector_path: "barrier=fence,wall,city_wall,hedge,kerb,…", kind: "stroke", color_var: None, fallback: [0x6a, 0x6a, 0x6a], min_zoom: 16, max_zoom: 19 },
        LayerDef { id: "cliffs", name: "Cliffs", parent: "barriers", vector_path: "natural=cliff (hash ticks)", kind: "stroke", color_var: None, fallback: [0x70, 0x65, 0x5c], min_zoom: 15, max_zoom: 19 },
        // -- places --------------------------------------------------------
        LayerDef { id: "places", name: "Places", parent: "", vector_path: "place=* / amenity=* / shop=* / tourism=…", kind: "group", color_var: Some("motorway-casing"), fallback: [0xdc, 0x2a, 0x67], min_zoom: 14, max_zoom: 19 },
        LayerDef { id: "places-country", name: "Countries", parent: "places", vector_path: "place=country (letter-spaced caps)", kind: "label", color_var: Some("country-labels"), fallback: [0x7a, 0x5b, 0x7a], min_zoom: 4, max_zoom: 19 },
        LayerDef { id: "places-state", name: "States & regions", parent: "places", vector_path: "place=state", kind: "label", color_var: Some("state-labels"), fallback: [0x7a, 0x5b, 0x7a], min_zoom: 5, max_zoom: 19 },
        LayerDef { id: "places-city", name: "Cities & capitals", parent: "places", vector_path: "place=city (capital=yes from z7)", kind: "label", color_var: None, fallback: [0x1a, 0x1a, 0x1a], min_zoom: 8, max_zoom: 19 },
        LayerDef { id: "places-town", name: "Towns", parent: "places", vector_path: "place=town", kind: "label", color_var: None, fallback: [0x22, 0x22, 0x22], min_zoom: 10, max_zoom: 19 },
        LayerDef { id: "places-small", name: "Villages & localities", parent: "places", vector_path: "place=village,hamlet,suburb,locality,…", kind: "label", color_var: Some("placenames-light"), fallback: [0x55, 0x55, 0x55], min_zoom: 13, max_zoom: 19 },
        LayerDef { id: "stations", name: "Stations", parent: "places", vector_path: "railway=station,halt", kind: "label", color_var: Some("station-color"), fallback: [0x79, 0x81, 0xb0], min_zoom: 13, max_zoom: 19 },
        LayerDef { id: "addresses", name: "Addresses", parent: "places", vector_path: "addr:housenumber=* (z17+)", kind: "label", color_var: Some("address-color"), fallback: [0x66, 0x66, 0x66], min_zoom: 17, max_zoom: 19 },
        LayerDef { id: "peaks", name: "Peaks & springs", parent: "places", vector_path: "natural=peak,spring,waterfall", kind: "label", color_var: None, fallback: [0x50, 0x40, 0x30], min_zoom: 12, max_zoom: 19 },
        LayerDef { id: "trees", name: "Trees", parent: "places", vector_path: "natural=tree (z16+)", kind: "label", color_var: None, fallback: [0x4d, 0x85, 0x4d], min_zoom: 16, max_zoom: 19 },
        LayerDef { id: "lamps", name: "Street lamps", parent: "places", vector_path: "highway=street_lamp (z17+)", kind: "label", color_var: None, fallback: [0xcc, 0xb4, 0x55], min_zoom: 17, max_zoom: 19 },
        LayerDef { id: "entrances", name: "Entrances", parent: "places", vector_path: "entrance=* (z17+)", kind: "label", color_var: None, fallback: [0x44, 0x44, 0x44], min_zoom: 17, max_zoom: 19 },
        LayerDef { id: "pois", name: "Points of interest", parent: "places", vector_path: "amenity=* / shop=* / tourism=* / office=… (sprites)", kind: "label", color_var: Some("motorway-casing"), fallback: [0xdc, 0x2a, 0x67], min_zoom: 14, max_zoom: 19 },
    ]
}

/// The top-level groups, in catalog order (the tree's roots).
pub fn top_level() -> Vec<LayerDef> {
    layer_catalog().into_iter().filter(|d| d.parent.is_empty()).collect()
}

/// The direct children of `id`, in catalog order.
pub fn children_of(cat: &[LayerDef], id: &str) -> Vec<LayerDef> {
    cat.iter().filter(|d| d.parent == id).copied().collect()
}

/// Every id under `id` (all levels, catalog order), excluding `id` itself.
pub fn descendants_of(cat: &[LayerDef], id: &str) -> Vec<&'static str> {
    let mut out = vec![];
    for d in cat.iter().filter(|d| d.parent == id) {
        out.push(d.id);
        out.extend(descendants_of(cat, d.id));
    }
    out
}

/// The top-level ancestor of `id` (`id` itself when it is top-level).
pub fn top_ancestor_of(cat: &[LayerDef], id: &str) -> String {
    let mut cur = id;
    loop {
        match cat.iter().find(|d| d.id == cur) {
            Some(d) if !d.parent.is_empty() => cur = d.parent,
            _ => return cur.to_string(),
        }
    }
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
    /// `id -> locked` (panel-only: locked rows reject property edits).
    pub locked: HashMap<String, bool>,
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

    /// Whether layer `id` is locked in the panel (unlisted = unlocked).
    pub fn is_locked(&self, id: &str) -> bool {
        self.locked.get(id).copied().unwrap_or(false)
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
        self.locked.clear();
        self.touch();
    }
}
