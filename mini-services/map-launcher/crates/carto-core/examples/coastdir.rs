//! Debug example: report the flow direction of coastline ways in a PBF.
//! Usage: cargo run -p carto-core --example coastdir -- <pbf>

use std::collections::HashMap;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pbf = &args[1];
    // pass 1: coastline way endpoints (node ids)
    let mut ends: Vec<(i64, i64, i64)> = Vec::new(); // way, first, last
    let reader = osmpbf::ElementReader::from_path(pbf).unwrap();
    reader.for_each(|el| match el {
        osmpbf::Element::Way(w) => {
            let coast = w.tags().any(|(k, v)| k == "natural" && v == "coastline");
            if coast {
                let refs: Vec<i64> = w.refs().collect();
                if refs.len() >= 2 {
                    ends.push((w.id(), refs[0], *refs.last().unwrap()));
                }
            }
        }
        _ => {}
    })
    .unwrap();
    eprintln!("coastline ways: {}", ends.len());
    let wanted: HashMap<i64, Vec<usize>> = {
        let mut m: HashMap<i64, Vec<usize>> = HashMap::new();
        for (i, (_, a, b)) in ends.iter().enumerate() {
            m.entry(*a).or_default().push(i);
            m.entry(*b).or_default().push(i);
        }
        m
    };
    // pass 2: resolve those node coords
    let mut coords: HashMap<i64, (f64, f64)> = HashMap::new();
    let reader = osmpbf::ElementReader::from_path(pbf).unwrap();
    reader.for_each(|el| match el {
        osmpbf::Element::Node(n) => {
            if wanted.contains_key(&n.id()) {
                coords.insert(n.id(), (n.lon(), n.lat()));
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
    let mut north = 0usize;
    let mut south = 0usize;
    let mut flat = 0usize;
    let mut unresolved = 0usize;
    for (id, a, b) in &ends {
        match (coords.get(a), coords.get(b)) {
            (Some((x1, y1)), Some((x2, y2))) => {
                let dlat = y2 - y1;
                let dlon = x2 - x1;
                if dlat.abs() > dlon.abs() {
                    if dlat > 0.0 {
                        north += 1;
                    } else {
                        south += 1;
                    }
                } else {
                    flat += 1;
                }
                if *id % 500 == 0 {
                    println!("way {id}: dlon={dlon:.4} dlat={dlat:.4}");
                }
            }
            _ => unresolved += 1,
        }
    }
    println!("northward={north} southward={south} flat={flat} unresolved={unresolved}");
}
