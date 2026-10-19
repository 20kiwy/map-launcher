#!/bin/sh
# map-launcher dev script: builds the Rust workspace, extracts the OSM data
# store on first run, then serves the tile-server daemon on :3100.
set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$ROOT/target"

BIN="$CARGO_TARGET_DIR/release/tile-server"

if [ "$1" = "--build-only" ]; then
    cargo build --release
    exit 0
fi

# Rebuild when the binary is missing or any source is newer than it.
if [ ! -x "$BIN" ] || [ -n "$(find crates -name '*.rs' -newer "$BIN" -print -quit 2>/dev/null)" ]; then
    echo "[map-launcher] building rust workspace..."
    cargo build --release
fi

# First-run data pipeline: PBF -> store.bin (Dodoma / Central Tanzania bbox).
if [ ! -f data/store.bin ]; then
    if [ -f data/tanzania-latest.osm.pbf ]; then
        echo "[map-launcher] extracting Central Tanzania OSM data..."
        "$BIN" extract data/tanzania-latest.osm.pbf data
    else
        echo "[map-launcher] WARNING: no data/tanzania-latest.osm.pbf — serving with empty registry"
    fi
fi

echo "[map-launcher] tile-server listening on :3100"
exec "$BIN" serve --port 3100
