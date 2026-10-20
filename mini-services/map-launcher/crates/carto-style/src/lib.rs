//! openstreetmap-carto style engine (pure `std`).
//!
//! Complete CartoCSS/MSS variable parser: lexer (comments, blocks, strings),
//! color literals (hex / named / rgb() / rgba() / hsl() / hsla()), variable
//! references and the carto color functions (`lighten`, `darken`, `fade`,
//! `spin`, `mix`, `saturate`, `desaturate`, `greyscale`/`grayscale`).
//!
//! Also hosts the semantic palette registry (roles -> @var -> color),
//! the SVG/PNG symbol registry and the system font loader helpers.

use include_dir::{Dir, DirEntry};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/* --------------------------------------------------------------------- */
/* Embedded openstreetmap-carto assets                                     */
/* --------------------------------------------------------------------- */
//
// The release executables are fully self-contained: every openstreetmap-carto
// `.mss` style sheet, the whole SVG/PNG symbol set and the bundled DejaVu
// fonts are compiled into the binary. A bare `tile-server.exe` therefore
// boots with the complete Carto palette + symbols on any machine (e.g. a
// Windows box with no repository checkout). An on-disk `assets/` directory
// always takes precedence so development stays overridable.

/// Verbatim `assets/style/*.mss` (openstreetmap-carto master).
pub static EMBEDDED_STYLE: Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../assets/style");

/// Verbatim `assets/symbols/**` (openstreetmap-carto SVG/PNG symbol set).
pub static EMBEDDED_SYMBOLS: Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../assets/symbols");

/// Verbatim `assets/patterns/**` (openstreetmap-carto polygon pattern SVGs).
pub static EMBEDDED_PATTERNS: Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../assets/patterns");

/// Verbatim `assets/fonts/*.ttf` (guaranteed fallback faces).
pub static EMBEDDED_FONTS: Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../assets/fonts");

/* --------------------------------------------------------------------- */
/* Color model                                                             */
/* --------------------------------------------------------------------- */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    pub fn hex(&self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
    pub fn rgba_tuple(&self) -> (u8, u8, u8, u8) {
        (self.r, self.g, self.b, self.a)
    }
    pub fn parse_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        let v = match s.len() {
            3 => s
                .chars()
                .flat_map(|c| [c, c])
                .collect::<Vec<_>>(),
            6 | 8 => s.chars().collect::<Vec<_>>(),
            _ => return None,
        };
        let byte = |pair: &[char]| -> Option<u8> {
            u8::from_str_radix(&pair.iter().collect::<String>(), 16).ok()
        };
        if v.len() == 3 {
            // expanded above to 6
        }
        if v.len() == 6 {
            Some(Self::rgb(byte(&v[0..2])?, byte(&v[2..4])?, byte(&v[4..6])?))
        } else if v.len() == 8 {
            Some(Self::rgba(byte(&v[0..2])?, byte(&v[2..4])?, byte(&v[4..6])?, byte(&v[6..8])?))
        } else {
            None
        }
    }
}

fn clamp_byte(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// Parse a CSS named color (subset used across openstreetmap-carto).
pub fn named_color(name: &str) -> Option<Rgba> {
    let c = match name.trim().to_ascii_lowercase().as_str() {
        "white" => Rgba::rgb(255, 255, 255),
        "black" => Rgba::rgb(0, 0, 0),
        "red" => Rgba::rgb(255, 0, 0),
        "green" => Rgba::rgb(0, 128, 0),
        "blue" => Rgba::rgb(0, 0, 255),
        "salmon" => Rgba::rgb(250, 128, 114),
        "pink" => Rgba::rgb(255, 192, 203),
        "orange" => Rgba::rgb(255, 165, 0),
        "yellow" => Rgba::rgb(255, 255, 0),
        "gray" | "grey" => Rgba::rgb(128, 128, 128),
        "silver" => Rgba::rgb(192, 192, 192),
        "navy" => Rgba::rgb(0, 0, 128),
        "teal" => Rgba::rgb(0, 128, 128),
        "olive" => Rgba::rgb(128, 128, 0),
        "maroon" => Rgba::rgb(128, 0, 0),
        "purple" => Rgba::rgb(128, 0, 128),
        "lime" => Rgba::rgb(0, 255, 0),
        "aqua" | "cyan" => Rgba::rgb(0, 255, 255),
        "fuchsia" | "magenta" => Rgba::rgb(255, 0, 255),
        "brown" => Rgba::rgb(165, 42, 42),
        "gold" => Rgba::rgb(255, 215, 0),
        "khaki" => Rgba::rgb(240, 230, 140),
        "indigo" => Rgba::rgb(75, 0, 130),
        "violet" => Rgba::rgb(238, 130, 238),
        "orange-red" | "orangered" => Rgba::rgb(255, 69, 0),
        "transparent" => Rgba::rgba(0, 0, 0, 0),
        _ => return None,
    };
    Some(c)
}

/* ---------------------------- HSL helpers ---------------------------- */

fn rgb_to_hsl(c: Rgba) -> (f64, f64, f64) {
    let r = f64::from(c.r) / 255.0;
    let g = f64::from(c.g) / 255.0;
    let b = f64::from(c.b) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f64::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if (max - r).abs() < f64::EPSILON {
        ((g - b) / d) + if g < b { 6.0 } else { 0.0 }
    } else if (max - g).abs() < f64::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    (h, s, l)
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);
    if s.abs() < f64::EPSILON {
        let v = clamp_byte(l * 255.0);
        return (v, v, v);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hk = h / 360.0;
    let channel = |mut t: f64| -> u8 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        clamp_byte(v * 255.0)
    };
    (channel(hk + 1.0 / 3.0), channel(hk), channel(hk - 1.0 / 3.0))
}

fn hsl_adjust(c: Rgba, dl: f64, ds: f64, dh: f64) -> Rgba {
    let (h, s, l) = rgb_to_hsl(c);
    let (r, g, b) = hsl_to_rgb(h + dh, (s + ds).clamp(0.0, 1.0), (l + dl).clamp(0.0, 1.0));
    Rgba::rgba(r, g, b, c.a)
}

/* --------------------------------------------------------------------- */
/* Carto color functions                                                   */
/* --------------------------------------------------------------------- */

/// `lighten(color, percentage)`
pub fn lighten(c: Rgba, pct: f64) -> Rgba {
    hsl_adjust(c, pct / 100.0, 0.0, 0.0)
}
/// `darken(color, percentage)`
pub fn darken(c: Rgba, pct: f64) -> Rgba {
    hsl_adjust(c, -pct / 100.0, 0.0, 0.0)
}
/// `saturate(color, percentage)`
pub fn saturate(c: Rgba, pct: f64) -> Rgba {
    hsl_adjust(c, 0.0, pct / 100.0, 0.0)
}
/// `desaturate(color, percentage)`
pub fn desaturate(c: Rgba, pct: f64) -> Rgba {
    hsl_adjust(c, 0.0, -pct / 100.0, 0.0)
}
/// `fade(color, percentage)` — sets absolute alpha.
pub fn fade(c: Rgba, pct: f64) -> Rgba {
    Rgba { a: clamp_byte(pct / 100.0 * 255.0), ..c }
}
/// `spin(color, degrees)`
pub fn spin(c: Rgba, deg: f64) -> Rgba {
    hsl_adjust(c, 0.0, 0.0, deg)
}
/// `mix(a, b, percentage)` — percentage of b in a.
pub fn mix(a: Rgba, b: Rgba, pct: f64) -> Rgba {
    let t = (pct / 100.0).clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| clamp_byte(f64::from(x) * (1.0 - t) + f64::from(y) * t);
    Rgba::rgba(ch(a.r, b.r), ch(a.g, b.g), ch(a.b, b.b), ch(a.a, b.a))
}
/// `greyscale(color)`
pub fn greyscale(c: Rgba) -> Rgba {
    let l = (0.299 * f64::from(c.r) + 0.587 * f64::from(c.g) + 0.114 * f64::from(c.b)).round();
    Rgba::rgba(l as u8, l as u8, l as u8, c.a)
}

/* --------------------------------------------------------------------- */
/* MSS lexer / parser                                                      */
/* --------------------------------------------------------------------- */

#[derive(Clone, Debug)]
pub enum Expr {
    Color(Rgba),
    Var(String),
    Num(f64, String), // value + unit ("", "%", "deg", "px")
    Func(String, Vec<Expr>),
}

#[derive(Clone, Debug)]
pub struct VarDecl {
    pub name: String,
    pub expr: Expr,
    pub source: String,
}

/// Strip comments and split into var declarations (recursively flattened).
fn extract_vars(text: &str, source: &str, out: &mut Vec<VarDecl>) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let mut depth = 0usize;
    let mut stmt_start = 0usize;

    let push_stmt = |seg: &str, out: &mut Vec<VarDecl>| {
        // Segments may carry leading comments (they were scanned over but
        // remain part of the raw slice) — strip them before matching @var.
        let cleaned = strip_comments(seg);
        let seg = cleaned.trim();
        // A fragment may open with a leftover block closer, e.g.
        // "}\n\n@water-color: …" (statement after a Map block). Jump to the
        // final top-level '@' in that case.
        let seg = if seg.starts_with('@') {
            seg
        } else {
            match seg.rfind("\n@") {
                Some(idx) => seg[idx + 1..].trim(),
                None => "",
            }
        };
        if let Some(rest) = seg.strip_prefix('@') {
            if let Some(colon) = rest.find(':') {
                let name = rest[..colon].trim().to_string();
                let val = rest[colon + 1..].trim().trim_end_matches(';').trim();
                if !name.is_empty() && !val.is_empty() {
                    if let Some(expr) = parse_expr(val) {
                        out.push(VarDecl { name, expr, source: source.to_string() });
                    }
                }
            }
        }
    };

    while i < chars.len() {
        match chars[i] {
            '/' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            '/' if i + 1 < chars.len() && chars[i + 1] == '/' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '{' => {
                depth += 1;
                i += 1;
                continue;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                i += 1;
                continue;
            }
            ';' if depth <= 1 => {
                let seg: String = chars[stmt_start..i].iter().collect();
                push_stmt(&seg, out);
                stmt_start = i + 1;
                i += 1;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    let tail: String = chars[stmt_start.min(chars.len())..].iter().collect();
    push_stmt(&tail, out);
}

/// Remove `// line` and `/* block */` comments from a statement fragment.
fn strip_comments(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
        } else if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn parse_expr(s: &str) -> Option<Expr> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Function call: name(args)
    if let Some(paren) = s.find('(') {
        let name = s[..paren].trim().to_ascii_lowercase();
        if s.ends_with(')') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            let args_raw = &s[paren + 1..s.len() - 1];
            let args = split_top_level(args_raw)
                .into_iter()
                .filter_map(|s| parse_expr(&s))
                .collect();
            return Some(Expr::Func(name, args));
        }
    }
    // Hex color
    if s.starts_with('#') {
        return Rgba::parse_hex(s).map(Expr::Color);
    }
    // Number with optional unit (% / deg / px)
    if let Some(rest) = s.strip_suffix('%') {
        if let Ok(v) = rest.trim().parse::<f64>() {
            return Some(Expr::Num(v, "%".into()));
        }
    }
    if let Some(rest) = s.strip_suffix("deg") {
        if let Ok(v) = rest.trim().parse::<f64>() {
            return Some(Expr::Num(v, "deg".into()));
        }
    }
    if let Ok(v) = s.parse::<f64>() {
        return Some(Expr::Num(v, String::new()));
    }
    // Variable reference
    if let Some(name) = s.strip_prefix('@') {
        return Some(Expr::Var(name.trim().to_string()));
    }
    // rgb()/rgba()/hsl() handled as Func above; named color here
    named_color(s).map(Expr::Color)
}

fn split_top_level(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                parts.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

/* --------------------------------------------------------------------- */
/* Palette                                                                 */
/* --------------------------------------------------------------------- */

#[derive(Clone, Debug)]
pub struct PaletteRole {
    pub role: String,
    pub var: String,
    pub color: Rgba,
    pub source: String,
}

pub struct Palette {
    /// All evaluated var declarations in file order (var_count).
    pub decls: Vec<VarDecl>,
    /// Resolved color per var name (last definition wins).
    pub vars: HashMap<String, Rgba>,
    /// Semantic roles.
    pub roles: Vec<PaletteRole>,
    pub color_count: usize,
}

/// Semantic roles exported by the palette registry (role name, @var name).
pub const ROLE_VARS: &[(&str, &str)] = &[
    ("land", "land-color"),
    ("water", "water-color"),
    ("water-text", "water-text"),
    ("glacier", "glacier"),
    ("forest", "forest"),
    ("forest-text", "forest-text"),
    ("grass", "grass"),
    ("scrub", "scrub"),
    ("park", "park"),
    ("orchard", "orchard"),
    ("allotments", "allotments"),
    ("farmland", "farmland"),
    ("farmland-line", "farmland-line"),
    ("farmyard", "farmyard"),
    ("farmyard-line", "farmyard-line"),
    ("residential", "residential"),
    ("residential-line", "residential-line"),
    ("retail", "retail"),
    ("retail-line", "retail-line"),
    ("commercial", "commercial"),
    ("commercial-line", "commercial-line"),
    ("industrial", "industrial"),
    ("industrial-line", "industrial-line"),
    ("built-up-lowzoom", "built-up-lowzoom"),
    ("building-fill", "building-fill"),
    ("building-line", "building-line"),
    ("building-low-zoom", "building-low-zoom"),
    ("building-major-fill", "building-major-fill"),
    ("motorway-fill", "motorway-fill"),
    ("motorway-casing", "motorway-casing"),
    ("motorway-low-zoom", "motorway-low-zoom"),
    ("motorway-shield", "motorway-shield"),
    ("trunk-fill", "trunk-fill"),
    ("trunk-casing", "trunk-casing"),
    ("trunk-low-zoom", "trunk-low-zoom"),
    ("trunk-shield", "trunk-shield"),
    ("primary-fill", "primary-fill"),
    ("primary-casing", "primary-casing"),
    ("primary-low-zoom", "primary-low-zoom"),
    ("primary-shield", "primary-shield"),
    ("secondary-fill", "secondary-fill"),
    ("secondary-casing", "secondary-casing"),
    ("secondary-low-zoom", "secondary-low-zoom"),
    ("secondary-shield", "secondary-shield"),
    ("tertiary-fill", "tertiary-fill"),
    ("tertiary-casing", "tertiary-casing"),
    ("tertiary-shield", "tertiary-shield"),
    ("residential-fill", "residential-fill"),
    ("residential-casing", "residential-casing"),
    ("service-fill", "service-fill"),
    ("living-street-fill", "living-street-fill"),
    ("pedestrian-fill", "pedestrian-fill"),
    ("track-fill", "track-fill"),
    ("footway-fill", "footway-fill"),
    ("default-casing", "default-casing"),
];

impl Palette {
    /// Parse MSS sources `(source name, text)` in import order and evaluate
    /// all variables.
    pub fn from_mss_sources<I>(sources: I) -> Palette
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut decls: Vec<VarDecl> = Vec::new();
        for (src, text) in sources {
            extract_vars(&text, &src, &mut decls);
        }
        Palette::from_decls(decls)
    }

    /// Palette from the style sheets embedded in this binary — the verbatim
    /// openstreetmap-carto `assets/style` set. Lets a bare executable boot
    /// with the full Carto palette (Windows release binaries, CI, tests).
    pub fn from_embedded() -> Palette {
        let mut sources: Vec<(String, String)> = Vec::new();
        collect_embedded_mss(&EMBEDDED_STYLE, &mut sources);
        sources.sort_by(|a, b| a.0.cmp(&b.0));
        Palette::from_mss_sources(sources)
    }

    /// Parse every `.mss` file in `dir` (sorted by name so cross-file var
    /// references resolve in import order) and evaluate all variables.
    pub fn from_dir<P: AsRef<Path>>(dir: P) -> std::io::Result<Palette> {
        let mut files: Vec<PathBuf> = fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "mss").unwrap_or(false))
            .collect();
        files.sort();
        let mut sources: Vec<(String, String)> = Vec::with_capacity(files.len());
        for f in &files {
            let src = f.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let text = fs::read_to_string(f)?;
            sources.push((src, text));
        }
        Ok(Palette::from_mss_sources(sources))
    }

    /// Shared evaluation: growing var map, multiple passes to resolve forward
    /// references (a var used before its declaration) until fixpoint.
    fn from_decls(decls: Vec<VarDecl>) -> Palette {
        let mut vars: HashMap<String, Rgba> = HashMap::new();
        let mut seen: HashSet<String> = HashSet::new();
        for _ in 0..4 {
            let mut progressed = false;
            for d in &decls {
                if let Some(c) = eval_expr(&d.expr, &vars) {
                    let inserted = !vars.contains_key(&d.name);
                    if inserted || vars[&d.name] != c {
                        progressed = true;
                    }
                    vars.insert(d.name.clone(), c);
                    seen.insert(c.hex());
                }
            }
            if !progressed {
                break;
            }
        }
        let roles = ROLE_VARS
            .iter()
            .filter_map(|(role, var)| {
                vars.get(*var).map(|c| PaletteRole {
                    role: (*role).to_string(),
                    var: (*var).to_string(),
                    color: *c,
                    source: decls
                        .iter()
                        .rev()
                        .find(|d| d.name == *var)
                        .map(|d| d.source.clone())
                        .unwrap_or_default(),
                })
            })
            .collect();
        Palette { decls, vars, roles, color_count: seen.len() }
    }

    pub fn var_count(&self) -> usize {
        self.decls.len()
    }

    pub fn get(&self, var: &str) -> Option<Rgba> {
        self.vars.get(var).copied()
    }
}

/// Evaluate an expression against the current variable environment
/// (supports nested carto color functions up to 16 levels deep).
pub fn eval_expr(e: &Expr, env: &HashMap<String, Rgba>) -> Option<Rgba> {
    eval_depth(e, env, 0)
}

fn eval_depth(e: &Expr, env: &HashMap<String, Rgba>, depth: u32) -> Option<Rgba> {
    if depth > 16 {
        return None;
    }
    match e {
        Expr::Color(c) => Some(*c),
        Expr::Var(name) => env.get(name).copied(),
        Expr::Num(..) => None,
        Expr::Func(name, args) => {
            let eval_n = |i: usize| -> Option<Rgba> {
                args.get(i).and_then(|a| eval_depth(a, env, depth + 1))
            };
            let num = |i: usize| -> Option<f64> {
                match args.get(i)? {
                    Expr::Num(v, _) => Some(*v),
                    Expr::Var(_) => None,
                    _ => None,
                }
            };
            match name.as_str() {
                "lighten" => Some(lighten(eval_n(0)?, num(1).unwrap_or(10.0))),
                "darken" => Some(darken(eval_n(0)?, num(1).unwrap_or(10.0))),
                "saturate" => Some(saturate(eval_n(0)?, num(1).unwrap_or(10.0))),
                "desaturate" => Some(desaturate(eval_n(0)?, num(1).unwrap_or(10.0))),
                "fade" => Some(fade(eval_n(0)?, num(1).unwrap_or(100.0))),
                "spin" => Some(spin(eval_n(0)?, num(1).unwrap_or(0.0))),
                "mix" => {
                    let a = eval_n(0)?;
                    let b = eval_n(1)?;
                    Some(mix(a, b, num(2).unwrap_or(50.0)))
                }
                "greyscale" | "grayscale" => Some(greyscale(eval_n(0)?)),
                "rgb" => Some(Rgba::rgb(
                    clamp_byte(num(0)?),
                    clamp_byte(num(1)?),
                    clamp_byte(num(2)?),
                )),
                "rgba" => Some(Rgba::rgba(
                    clamp_byte(num(0)?),
                    clamp_byte(num(1)?),
                    clamp_byte(num(2)?),
                    clamp_byte(num(3).unwrap_or(255.0)),
                )),
                "hsl" => {
                    let (r, g, b) = hsl_to_rgb(num(0)?, num(1)? / 100.0, num(2)? / 100.0);
                    Some(Rgba::rgb(r, g, b))
                }
                "hsla" => {
                    let (r, g, b) = hsl_to_rgb(num(0)?, num(1)? / 100.0, num(2)? / 100.0);
                    Some(Rgba::rgba(r, g, b, clamp_byte(num(3).unwrap_or(100.0) / 100.0 * 255.0)))
                }
                _ => None,
            }
        }
    }
}

/* --------------------------------------------------------------------- */
/* Symbol registry                                                         */
/* --------------------------------------------------------------------- */

#[derive(Debug, Default)]
pub struct SymbolRegistry {
    pub files: Vec<String>,
    pub svg_count: usize,
    pub png_count: usize,
}

/// Recursively collect `(relative path, text)` for embedded `.mss` files.
fn collect_embedded_mss(dir: &Dir<'static>, out: &mut Vec<(String, String)>) {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(d) => collect_embedded_mss(d, out),
            DirEntry::File(f) => {
                if f.path().extension().map(|x| x == "mss").unwrap_or(false) {
                    let text = f.contents_utf8().unwrap_or_default().to_string();
                    out.push((f.path().display().to_string(), text));
                }
            }
        }
    }
}

impl SymbolRegistry {
    /// Symbol registry from the embedded openstreetmap-carto symbol set
    /// (works with a bare executable — no on-disk assets needed).
    pub fn embedded() -> SymbolRegistry {
        let mut reg = SymbolRegistry::default();
        walk_embedded_symbols(&EMBEDDED_SYMBOLS, &mut reg);
        reg.files.sort();
        reg
    }

    pub fn scan<P: AsRef<Path>>(root: P) -> SymbolRegistry {
        let mut reg = SymbolRegistry::default();
        let mut stack = vec![root.as_ref().to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for e in entries.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase());
                    match ext.as_deref() {
                        Some("svg") => {
                            reg.svg_count += 1;
                            reg.files.push(p.to_string_lossy().to_string());
                        }
                        Some("png") => {
                            reg.png_count += 1;
                            reg.files.push(p.to_string_lossy().to_string());
                        }
                        _ => {}
                    }
                }
            }
        }
        reg.files.sort();
        reg
    }
}

/// Recursively walk the embedded symbol tree collecting svg/png entries.
fn walk_embedded_symbols(dir: &Dir<'static>, reg: &mut SymbolRegistry) {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(d) => walk_embedded_symbols(d, reg),
            DirEntry::File(f) => {
                let ext = f.path().extension().map(|x| x.to_string_lossy().to_lowercase());
                match ext.as_deref() {
                    Some("svg") => {
                        reg.svg_count += 1;
                        reg.files.push(f.path().display().to_string());
                    }
                    Some("png") => {
                        reg.png_count += 1;
                        reg.files.push(f.path().display().to_string());
                    }
                    _ => {}
                }
            }
        }
    }
}

/* --------------------------------------------------------------------- */
/* Fonts                                                                   */
/* --------------------------------------------------------------------- */
//
// Font importing follows the craft-launcher pattern (clone/craft-launcher
// `crates/ui-egui/src/{theme.rs,cjk_font.rs}`):
//   1. a bundled face ships with the app and is the guaranteed baseline,
//   2. system font directories are scanned per-OS (Windows %SystemRoot%\Fonts,
//      macOS /System/Library/Fonts, Linux /usr/share/fonts ...), matching a
//      family by name and preferring the Regular/Bold face,
//   3. the bundled face is the fallback when the system has none.
// The family priority mirrors openstreetmap-carto `style/fonts.mss`, whose
// first choice is Noto Sans with DejaVu Sans behind it.

/// Bundled assets dir, resolved from this crate (works in tests/CI where the
/// process CWD is the crate dir, and at runtime).
pub fn bundled_fonts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts")
}

/// Per-platform system font directories (craft-launcher `candidates()` style).
pub fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "windows")]
    {
        let root = std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        dirs.push(root.join("Fonts"));
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            dirs.push(home.join(".fonts"));
            dirs.push(home.join(".local/share/fonts"));
        }
    }
    dirs
}

/// lowercase alnum-only normalisation of a file stem for family matching.
fn normalize_stem(stem: &str) -> String {
    stem.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Does `stem` belong to `family`? Craft-launcher matches by family name; we
/// match the normalised stem against the normalised family prefix and exclude
/// sibling families (mono/serif/script-specific) that would shadow the Latin face.
fn stem_matches_family(norm: &str, family_norm: &str) -> bool {
    if !norm.starts_with(family_norm) {
        return false;
    }
    let rest = &norm[family_norm.len()..];
    const EXCLUDE: [&str; 8] = ["mono", "serif", "symbols", "math", "music", "display", "emoji", "condensed"];
    if EXCLUDE.iter().any(|x| rest.contains(x)) {
        return false;
    }
    // Script-specific faces (Noto Sans Arabic, NotoSansSC…) rarely carry Latin.
    const SCRIPTS: [&str; 12] = ["arabic", "hebrew", "thai", "tamil", "devanagari", "ethiopic",
        "georgian", "armenian", "kannada", "telugu", "myanmar", "khmer"];
    !SCRIPTS.iter().any(|x| rest.contains(x))
}

/// Score a candidate file for the wanted style. Higher is better.
fn style_score(stem_lower: &str, bold: bool) -> i32 {
    let norm = normalize_stem(stem_lower);
    if bold {
        // DejaVuSans-Bold, NotoSans-Bold, NotoSans-BoldItalic
        if norm.contains("bold") {
            // avoid semibold/extrabold/light overspill
            if norm.contains("semibold") || norm.contains("extrabold") || norm.contains("extralight") {
                1
            } else {
                6
            }
        } else if norm.ends_with("bd") || norm.ends_with("b") {
            // Windows short bold suffix: arialbd.ttf, tahomabd.ttf, segoeuib.ttf
            5
        } else if norm.contains("black") || norm.contains("heavy") {
            3
        } else {
            0
        }
    } else {
        if norm.contains("regular") {
            6
        } else if norm.ends_with("wght") || norm.contains("variable") {
            5 // variable face: defaults to Regular
        } else if norm.contains("bold") || norm.ends_with("bd") || norm.ends_with("b") {
            1 // bold face requested as regular — last resort
        } else if norm.contains("italic") || norm.contains("oblique") {
            1
        } else if norm.contains("medium") || norm.contains("book") {
            4
        } else {
            3 // bare family face, e.g. DejaVuSans.ttf
        }
    }
}

/// Walk one directory (max depth 4) collecting *.ttf/*.otf paths.
fn collect_font_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_font_files(&p, out, depth + 1);
        } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            if matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf") {
                out.push(p);
            }
        }
    }
}

/// Best system face for `family` ("Noto Sans", "DejaVu Sans", ...), or None.
pub fn find_system_font(family: &str, bold: bool) -> Option<PathBuf> {
    let family_norm = normalize_stem(family);
    let mut files = Vec::new();
    for d in system_font_dirs() {
        collect_font_files(&d, &mut files, 0);
    }
    let mut best: Option<(i32, PathBuf)> = None;
    for p in files {
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
        let norm = normalize_stem(stem);
        if !stem_matches_family(&norm, &family_norm) {
            continue;
        }
        let score = style_score(stem, bold);
        if score > 0 && best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, p));
        }
    }
    best.map(|(_, p)| p)
}

/// Bundled face for `family` inside assets/fonts.
fn bundled_font(family: &str, bold: bool) -> Option<PathBuf> {
    let family_norm = normalize_stem(family);
    let mut files = Vec::new();
    collect_font_files(&bundled_fonts_dir(), &mut files, 0);
    let mut best: Option<(i32, PathBuf)> = None;
    for p in files {
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else { continue };
        let norm = normalize_stem(stem);
        if !stem_matches_family(&norm, &family_norm) {
            continue;
        }
        let score = style_score(stem, bold);
        if score > 0 && best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, p));
        }
    }
    best.map(|(_, p)| p)
}

/// (relative path, bytes) for every font embedded from assets/fonts.
fn embedded_font_files() -> Vec<(&'static str, &'static [u8])> {
    let mut out = Vec::new();
    fn walk(dir: &Dir<'static>, out: &mut Vec<(&'static str, &'static [u8])>) {
        for entry in dir.entries() {
            match entry {
                DirEntry::Dir(d) => walk(d, out),
                DirEntry::File(f) => {
                    let ext = f.path().extension().map(|x| x == "ttf" || x == "otf").unwrap_or(false);
                    if ext {
                        let name = f.path().to_str().unwrap_or_default();
                        out.push((name, f.contents()));
                    }
                }
            }
        }
    }
    walk(&EMBEDDED_FONTS, &mut out);
    out
}

/// Embedded face for `family` inside assets/fonts — the guaranteed fallback
/// when neither the system scan nor the build-machine assets dir resolves
/// (bare release executables, e.g. tile-server.exe on Windows).
/// Returns `(file name, bytes)`.
pub fn embedded_font(family: &str, bold: bool) -> Option<(&'static str, &'static [u8])> {
    let family_norm = normalize_stem(family);
    let mut best: Option<(i32, &'static str, &'static [u8])> = None;
    for (name, bytes) in embedded_font_files() {
        let stem = std::path::Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let norm = normalize_stem(stem);
        if !stem_matches_family(&norm, &family_norm) {
            continue;
        }
        let score = style_score(stem, bold);
        if score > 0 && best.as_ref().is_none_or(|(b, _, _)| score > *b) {
            best = Some((score, name, bytes));
        }
    }
    best.map(|(_, name, bytes)| (name, bytes))
}

/// The map-label face: openstreetmap-carto fonts.mss order — Noto Sans first,
/// then DejaVu Sans. System fonts preferred (craft-launcher style), bundled
/// assets/fonts as the guaranteed fallback. Windows ships neither Noto Sans
/// nor DejaVu Sans, so the stock Windows faces are last-resort families
/// before the embedded font bytes are used.
pub fn load_map_font(bold: bool) -> Option<PathBuf> {
    const FAMILIES: &[&str] = if cfg!(target_os = "windows") {
        &["Noto Sans", "DejaVu Sans", "Segoe UI", "Arial", "Tahoma"]
    } else {
        &["Noto Sans", "DejaVu Sans"]
    };
    for family in FAMILIES.iter().copied() {
        if let Some(p) = find_system_font(family, bold) {
            return Some(p);
        }
        if let Some(p) = bundled_font(family, bold) {
            return Some(p);
        }
    }
    None
}

/// Back-compat helper: locate a font file by short family key.
pub fn find_font(family: &str, bold: bool) -> Option<PathBuf> {
    let fam = match family {
        "dejavu" => "DejaVu Sans",
        "noto" => "Noto Sans",
        other => other,
    };
    if let Some(p) = find_system_font(fam, bold) {
        return Some(p);
    }
    bundled_font(fam, bold).or_else(|| {
        // Legacy direct paths.
        let name = if bold { "DejaVuSans-Bold.ttf" } else { "DejaVuSans.ttf" };
        [
            bundled_fonts_dir().join(name),
            PathBuf::from("assets/fonts").join(name),
            PathBuf::from("/usr/share/fonts/truetype/dejavu").join(name),
        ]
        .into_iter()
        .find(|p| p.is_file())
    })
}

/* --------------------------------------------------------------------- */
/* Tests                                                                   */
/* --------------------------------------------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parse() {
        assert_eq!(Rgba::parse_hex("#f2efe9"), Some(Rgba::rgb(0xf2, 0xef, 0xe9)));
        assert_eq!(Rgba::parse_hex("e892a2"), Some(Rgba::rgb(0xe8, 0x92, 0xa2)));
        assert_eq!(Rgba::parse_hex("#aad3df"), Some(Rgba::rgb(0xaa, 0xd3, 0xdf)));
        assert_eq!(Rgba::parse_hex("#bad"), Some(Rgba::rgb(0xbb, 0xaa, 0xdd)));
        assert_eq!(Rgba::parse_hex("#123456").unwrap().hex(), "#123456");
    }

    #[test]
    fn named() {
        assert_eq!(named_color("salmon"), Some(Rgba::rgb(250, 128, 114)));
        assert_eq!(named_color("WHITE"), Some(Rgba::rgb(255, 255, 255)));
    }

    #[test]
    fn light_dark_spin() {
        let w = Rgba::rgb(200, 200, 200);
        assert!(lighten(w, 50.0).r >= w.r);
        assert!(darken(w, 50.0).r <= w.r);
        let base = Rgba::parse_hex("#e892a2").unwrap();
        let spun = spin(base, 180.0);
        assert_ne!(spun, base);
        assert_eq!(fade(base, 50.0).a, 128);
    }

    #[test]
    fn parse_and_eval_snippet() {
        let mut decls = Vec::new();
        extract_vars(
            r#"
// comment
@land-color: #f2efe9;
Map { background-color: @land-color; }
/* block
   comment */
@building-line: darken(@building-fill, 15%);
@building-fill: #d9d0c9;
@water-text: hsl(60, 50%, 50%);
@motorway-tunnel-fill: lighten(@motorway-fill, 10%);
@motorway-fill: #e892a2;
"#,
            "test.mss",
            &mut decls,
        );
        assert_eq!(
            decls.len(),
            6,
            "decls={:?}",
            decls.iter().map(|d| (d.name.as_str(), d.source.as_str())).collect::<Vec<_>>()
        );
        let mut env = HashMap::new();
        for _ in 0..4 {
            let mut progressed = false;
            for d in &decls {
                if let Some(c) = eval_expr(&d.expr, &env) {
                    if !env.contains_key(&d.name) {
                        progressed = true;
                    }
                    env.insert(d.name.clone(), c);
                }
            }
            if !progressed {
                break;
            }
        }
        assert_eq!(
            env["land-color"],
            Rgba::rgb(0xf2, 0xef, 0xe9),
            "env keys={:?}",
            env.keys().collect::<Vec<_>>()
        );
        // building-line = darken(#d9d0c9, 15%) evaluated despite decl order
        let bl = env["building-line"];
        assert!(bl.r < 0xd9 && bl.g < 0xd0);
        assert_eq!(env["motorway-tunnel-fill"].r, lighten(Rgba::rgb(0xe8, 0x92, 0xa2), 10.0).r);
    }

    #[test]
    fn registry_roles() {
        let pal = Palette::from_dir("../../assets/style").expect("palette dir");
        assert!(pal.var_count() > 400, "var_count={}", pal.var_count());
        assert!(pal.color_count > 100, "color_count={}", pal.color_count);
        assert!(pal.roles.len() > 40, "roles={}", pal.roles.len());
        assert_eq!(pal.get("land-color"), Some(Rgba::rgb(0xf2, 0xef, 0xe9)));
        assert_eq!(pal.get("water-color"), Some(Rgba::rgb(0xaa, 0xd3, 0xdf)));
        assert_eq!(pal.get("motorway-fill"), Some(Rgba::rgb(0xe8, 0x92, 0xa2)));
        assert_eq!(pal.get("motorway-casing"), Some(Rgba::rgb(0xdc, 0x2a, 0x67)));
        assert_eq!(pal.get("forest"), Some(Rgba::rgb(0xad, 0xd1, 0x9e)));
        assert_eq!(pal.get("farmland"), Some(Rgba::rgb(0xee, 0xf0, 0xd5)));
        assert_eq!(pal.get("building-fill"), Some(Rgba::rgb(0xd9, 0xd0, 0xc9)));
    }

    #[test]
    fn symbols_and_fonts() {
        let reg = SymbolRegistry::scan("../../assets/symbols");
        assert!(reg.svg_count > 40, "svg={}", reg.svg_count);
        assert!(find_font("dejavu", false).is_some());
        assert!(find_font("dejavu", true).is_some());
        // openstreetmap-carto fonts.mss order: a Noto Sans or DejaVu face
        // must resolve (system scan, bundled assets fallback).
        assert!(load_map_font(false).is_some(), "no regular map font");
        assert!(load_map_font(true).is_some(), "no bold map font");
    }

    #[test]
    fn embedded_palette_matches_carto() {
        let pal = Palette::from_embedded();
        assert!(pal.var_count() > 400, "vars={}", pal.var_count());
        assert!(pal.color_count > 100, "colors={}", pal.color_count);
        assert!(pal.roles.len() >= 50, "roles={}", pal.roles.len());
        assert_eq!(pal.get("land-color"), Some(Rgba::rgb(0xf2, 0xef, 0xe9)));
        assert_eq!(pal.get("water-color"), Some(Rgba::rgb(0xaa, 0xd3, 0xdf)));
        assert_eq!(pal.get("motorway-fill"), Some(Rgba::rgb(0xe8, 0x92, 0xa2)));
        assert_eq!(pal.get("motorway-casing"), Some(Rgba::rgb(0xdc, 0x2a, 0x67)));
    }

    #[test]
    fn embedded_symbols_and_fonts() {
        let reg = SymbolRegistry::embedded();
        assert!(reg.svg_count > 500, "svg={}", reg.svg_count);
        assert!(reg.png_count > 5, "png={}", reg.png_count);
        let (name, bytes) = embedded_font("DejaVu Sans", false).expect("embedded regular");
        assert!(name.ends_with("DejaVuSans.ttf"), "name={name}");
        assert!(bytes.len() > 100_000);
        let (bold_name, bold_bytes) = embedded_font("DejaVu Sans", true).expect("embedded bold");
        assert!(bold_name.ends_with("DejaVuSans-Bold.ttf"), "name={bold_name}");
        assert!(bold_bytes.len() > 100_000);
    }

    #[test]
    fn font_family_matching() {
        assert!(stem_matches_family("dejavusans", "dejavusans"));
        assert!(stem_matches_family("dejavusansbold", "dejavusans"));
        assert!(!stem_matches_family("dejavusansmono", "dejavusans"));
        assert!(!stem_matches_family("dejavuserif", "dejavusans"));
        assert!(stem_matches_family("notosanswght", "notosans"));
        assert!(stem_matches_family("notosansscwght", "notosans"));
        assert!(!stem_matches_family("notosansarabic", "notosans"));
        assert!(!stem_matches_family("notoserifsc", "notosans"));
        // style scores: prefer Regular/Bold over italic/medium
        assert!(style_score("DejaVuSans-Bold", true) > style_score("DejaVuSans-SemiBold", true));
        assert!(style_score("NotoSans-Regular", false) > style_score("NotoSans-Italic", false));
    }
}
