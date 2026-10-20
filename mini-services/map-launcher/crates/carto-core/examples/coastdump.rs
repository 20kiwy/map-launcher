//! Debug tool: dump coastline ways inside a bbox as JSON, then (with --rings)
//! run build_ocean_rings on them and emit an SVG colored by classification.
//!
//! Usage:
//!   cargo run --release -p carto-core --example coastdump -- <pbf> w s e n out.json
//!   cargo run --release -p carto-core --example coastdump -- --rings out.json w s e n out.svg

use std::collections::HashMap;
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 7 && args[1] != "--rings" {
        dump(&args);
    } else if args.len() >= 8 {
        rings(&args);
    } else {
        eprintln!("usage: coastdump <pbf> w s e n out.json | --rings in.json w s e n out.svg");
    }
}

fn dump(args: &[String]) {
    let pbf = &args[1];
    let (w, s, e, n): (f64, f64, f64, f64) = (args[2].parse().unwrap(), args[3].parse().unwrap(), args[4].parse().unwrap(), args[5].parse().unwrap());
    let mut ways: Vec<(i64, Vec<i64>)> = Vec::new();
    let reader = osmpbf::ElementReader::from_path(pbf).unwrap();
    reader.for_each(|el| match el {
        osmpbf::Element::Way(wy) => {
            let coast = wy.tags().any(|(k, v)| k == "natural" && v == "coastline");
            if coast {
                ways.push((wy.id(), wy.refs().collect()));
            }
        }
        _ => {}
    })
    .unwrap();
    eprintln!("coastline ways total: {}", ways.len());
    let mut wanted: HashMap<i64, ()> = HashMap::new();
    for (_, refs) in &ways {
        for r in refs {
            wanted.insert(*r, ());
        }
    }
    let mut coords: HashMap<i64, (f64, f64)> = HashMap::new();
    let reader = osmpbf::ElementReader::from_path(pbf).unwrap();
    reader.for_each(|el| match el {
        osmpbf::Element::Node(nd) => {
            if wanted.contains_key(&nd.id()) {
                coords.insert(nd.id(), (nd.lon(), nd.lat()));
            }
        }
        osmpbf::Element::DenseNode(d) => {
            if wanted.contains_key(&d.id()) {
                coords.insert(d.id(), (d.lon(), d.lat()));
            }
        }
        _ => {}
    })
    .unwrap();
    let mut out = String::from("{\"ways\":[");
    let mut first = true;
    for (id, refs) in &ways {
        // Mirror the extract pipeline exactly: only nodes INSIDE the bbox are
        // resolved, so chain endpoints are the last inside nodes.
        let pts: Vec<(f64, f64)> = refs
            .iter()
            .filter_map(|r| coords.get(r).copied())
            .filter(|(x, y)| *x >= w && *x <= e && *y >= s && *y <= n)
            .collect();
        if pts.len() < 2 {
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!("{{\"id\":{id},\"pts\":["));
        for (i, (x, y)) in pts.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!("[{x:.7},{y:.7}]"));
        }
        out.push_str("]}");
    }
    out.push_str("]}");
    let mut f = std::fs::File::create(&args[6]).unwrap();
    f.write_all(out.as_bytes()).unwrap();
    eprintln!("wrote {}", args[6]);
}

fn rings(args: &[String]) {
    let json = std::fs::read_to_string(&args[2]).unwrap();
    let (w, s, e, n): (f64, f64, f64, f64) = (args[3].parse().unwrap(), args[4].parse().unwrap(), args[5].parse().unwrap(), args[6].parse().unwrap());
    // tiny JSON scan: find "pts":[ then read all [x,y] pairs until ']'
    let mut chains: Vec<Vec<(f64, f64)>> = Vec::new();
    let b = json.as_bytes();
    let mut i = 0usize;
    while i + 6 <= b.len() {
        if &json[i..i + 6] == "\"pts\":" {
            i += 6;
            if i < b.len() && b[i] == b'[' {
                i += 1;
                let mut pts = Vec::new();
                let mut cur = String::new();
                let mut nums: Vec<f64> = Vec::new();
                while i < b.len() {
                    let c = b[i];
                    if c == b']' {
                        if !cur.is_empty() {
                            if let Ok(v) = cur.parse::<f64>() { nums.push(v); }
                        }
                        if nums.len() >= 2 {
                            pts.push((nums[nums.len() - 2], nums[nums.len() - 1]));
                        }
                        nums.clear();
                        cur.clear();
                        i += 1;
                        if i < b.len() && b[i] == b']' {
                            break; // end of pts array
                        }
                        continue;
                    }
                    if c == b',' || c == b'[' {
                        if !cur.is_empty() {
                            if let Ok(v) = cur.parse::<f64>() { nums.push(v); }
                            cur.clear();
                        }
                        i += 1;
                        continue;
                    }
                    cur.push(c as char);
                    i += 1;
                }
                if pts.len() >= 2 {
                    chains.push(pts);
                }
            }
        } else {
            i += 1;
        }
    }
    eprintln!("chains: {}", chains.len());
    let rings = carto_core::build_ocean_rings(chains, [w, s, e, n]);
    eprintln!("rings: {}", rings.len());
    let mut svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='800' height='800' viewBox='0 0 800 800'>\n<rect width='800' height='800' fill='#f2efe9'/>\n"
    );
    let px = |p: (f64, f64)| -> (f64, f64) {
        let x = (p.0 - w) / (e - w) * 780.0 + 10.0;
        let y = (n - p.1) / (n - s) * 780.0 + 10.0;
        (x, y)
    };
    for (island, ring) in &rings {
        let fill = if *island { "#9bd09b" } else { "#aad3df" };
        let mut d = String::new();
        for (i, p) in ring.iter().enumerate() {
            let (x, y) = px(*p);
            d.push_str(&if i == 0 { format!("M{x:.1},{y:.1}") } else { format!("L{x:.1},{y:.1}") });
        }
        d.push('Z');
        svg.push_str(&format!("<path d='{d}' fill='{fill}' fill-opacity='0.7' stroke='#555' stroke-width='0.5'/>\n"));
    }
    // coastlines raw
    svg.push_str("</svg>\n");
    let mut f = std::fs::File::create(&args[7]).unwrap();
    f.write_all(svg.as_bytes()).unwrap();
    for (island, ring) in &rings {
        eprintln!(
            "ring: island={island} pts={} area={:.6}",
            ring.len(),
            carto_core::ring_signed_area(ring)
        );
    }
    eprintln!("wrote {}", args[7]);
}
