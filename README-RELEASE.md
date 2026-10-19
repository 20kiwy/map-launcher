map-launcher release bundle
===========================

The tile-server binary is SELF-CONTAINED: the full openstreetmap-carto
palette (all .mss style sheets), the complete SVG/PNG symbol set and the
DejaVu label fonts are embedded in the executable. No assets folder needed.

QUICK START (Windows)
---------------------
1. Download from the release page:
     - tile-server-vX-windows-x64.zip   (this file + tile-server.exe)
     - map-data-vX.zip                  (Central Tanzania / Dodoma map data)
     - map-launcher-web-vX.zip          (Next.js web preview UI)
2. Unzip all three into ONE folder, e.g. C:\map-launcher, keeping structure:

     C:\map-launcher\
       tile-server.exe
       README.txt            <- this file
       data\store.bin        <- from map-data-vX.zip
       web\server.js ...     <- from map-launcher-web-vX.zip

3. Double-click tile-server.exe (or run it in a terminal). You should see:

     palette[embedded assets/style]: 470 vars, 162 colors, 55 roles, ...
     fonts: regular=... bold=...
     registry: 116443 roads (1304 named), 530815 buildings, ...
     map-launcher tile-server v0.4.0 listening on :3100

   The window must stay open while you use the map.

4. Start the web UI (needs Node.js 20+ installed):

     cd C:\map-launcher\web
     set PORT=3000
     set HOSTNAME=127.0.0.1
     node server.js

5. Open http://localhost:3000 in your browser.
   Deep-zoom to z19, click roads for lane/direction/speed info, browse the
   full Carto palette in the Palette tab.

QUICK START (Linux)
-------------------
   tar xzf tile-server-vX-linux-x64.tar.gz
   unzip map-data-vX.zip -d map-launcher-vX/data/
   ./map-launcher-vX/tile-server
   # web UI: unzip map-launcher-web-vX.zip and `node server.js` inside

NO DATA YET?
------------
Without map data the server still boots and renders the base carto land
colors. Add data either by unzipping map-data-vX.zip (store.bin) into the
data\ folder next to the exe, or by extracting your own OSM PBF:

   tile-server extract <pbf> <data-dir> [west south east north]

DATA / CONFIG OPTIONS
---------------------
   --data-dir <path>     where store.bin lives (default: .\data or exe dir)
   --port <n>            HTTP port (default 3100)
   MAP_LAUNCHER_DATA     env var alternative to --data-dir
   TILE_SERVER_URL       env var for the web UI if the tile-server runs
                         on another host/port

The openstreetmap-carto style, symbols and palette are copied verbatim from
https://github.com/openstreetmap-carto/openstreetmap-carto (vendored as a
git submodule in vendor/openstreetmap-carto).
