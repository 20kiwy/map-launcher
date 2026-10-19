//! tile-server-cli — the headless map tile server.
//!
//!   tile-server-cli serve [--port N] [--data-dir D]
//!   tile-server-cli extract <pbf> <outdir> [w s e n]
//!   tile-server-cli --version
//!
//! The GUI twin is `tile-server` (same engine, plus the Map/Downloads window).

use std::path::PathBuf;
use std::process::ExitCode;

/// The world crop box used when no bbox is given (everything in the PBF).
mod carto_core_marker {
    pub const WORLD_BBOX: [f64; 4] = [-180.0, -85.0, 180.0, 85.0];
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--version" || a == "-V") {
        println!("tile-server-cli {}", tile_server_core::VERSION);
        return ExitCode::SUCCESS;
    }
    match args.first().map(String::as_str) {
        Some("extract") => ExitCode::from(tile_server_core::run_extract(&args[1..]) as u8),
        Some("fetch") => {
            // tile-server-cli fetch <url> [outdir] [w s e n]
            if args.len() < 2 {
                eprintln!("usage: tile-server-cli fetch <pbf-url> [outdir] [w s e n]");
                return ExitCode::FAILURE;
            }
            let url = args[1].clone();
            let out = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("data"));
            let bbox = if args.len() >= 7 {
                [
                    args[3].parse().unwrap_or(0.0),
                    args[4].parse().unwrap_or(0.0),
                    args[5].parse().unwrap_or(0.0),
                    args[6].parse().unwrap_or(0.0),
                ]
            } else {
                carto_core_marker::WORLD_BBOX
            };
            std::process::exit(tile_server_core::run_fetch(&url, &out, bbox));
        }
        Some("serve") | None => {
            let mut port: u16 = 3100;
            let mut data_flag: Option<String> = None;
            let mut i = if args.first().is_some_and(|a| a == "serve") { 1 } else { 0 };
            while i < args.len() {
                match args[i].as_str() {
                    "--port" if i + 1 < args.len() => {
                        port = args[i + 1].parse().unwrap_or(3100);
                        i += 2;
                    }
                    "--data-dir" if i + 1 < args.len() => {
                        data_flag = Some(args[i + 1].clone());
                        i += 2;
                    }
                    _ => i += 1,
                }
            }
            ExitCode::from(tile_server_core::run_headless(port, data_flag) as u8)
        }
        Some(other) => {
            eprintln!("unknown command: {other}\nusage: tile-server-cli serve [--port N] [--data-dir D] | extract <pbf> <outdir> [w s e n]");
            ExitCode::FAILURE
        }
    }
}
