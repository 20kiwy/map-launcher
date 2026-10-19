map-launcher web preview (Next.js standalone build)
===================================================

Prerequisite: Node.js 20 or newer (https://nodejs.org).
Start the Rust tile-server first (tile-server.exe from the other zip) —
it must be listening on port 3100.

Run (Windows CMD):
    set PORT=3000
    set HOSTNAME=127.0.0.1
    node server.js

Run (PowerShell):
    $env:PORT=3000; $env:HOSTNAME="127.0.0.1"; node server.js

Run (Linux/macOS):
    PORT=3000 HOSTNAME=127.0.0.1 node server.js

Then open http://localhost:3000

Environment variables:
    PORT              HTTP port for the web UI (default 3000)
    HOSTNAME          bind address (set 127.0.0.1 for local use)
    TILE_SERVER_URL   tile-server address (default http://127.0.0.1:3100)

What you get:
    - slippy map with server-rendered openstreetmap-carto tiles (z5-z19)
    - roads registry (116k roads, search by name, one-way / lanes / speed)
    - POI registry with category chips
    - live openstreetmap-carto palette browser
