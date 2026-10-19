map-launcher release bundle
===========================

Three products ship in every release:

  tile-server(.exe)     the desktop app AND the server in one: a Map tab
                        (live tile canvas + the layer-engine panel) and a
                        Downloads tab (Tanzania regions with progress,
                        reconnect and resume), while HTTP tile serving keeps
                        running on :3100
  tile-server-cli(.exe) the same engine headless: serve / extract / fetch
  map-launcher-web.zip  the Next.js web UI (roads registry, palette, POIs)

The binaries are SELF-CONTAINED: the full openstreetmap-carto palette (all
.mss style sheets), the complete SVG/PNG symbol set and the DejaVu label
fonts are embedded. No assets folder needed.

QUICK START (Windows, GUI)
--------------------------
1. Download from the release page:
     - tile-server-vX-windows-x64.zip   (this file + tile-server.exe + tile-server-cli.exe)
     - map-data-vX.zip                  (Central Tanzania / Dodoma map data)
     - map-launcher-web-vX.zip          (Next.js web preview UI)
2. Unzip all three into ONE folder, e.g. C:\map-launcher, keeping structure:

     C:\map-launcher\
       tile-server.exe
       tile-server-cli.exe
       README.txt            <- this file
       data\store.bin        <- from map-data-vX.zip
       web\server.js ...     <- from map-launcher-web-vX.zip

3. Double-click tile-server.exe. The window opens, the server starts
   automatically (green "live" dot, http://127.0.0.1:3100) and you get:

     - Map tab: pan with the mouse, zoom with the wheel (z4-z19), and the
       Layers panel on the right: eye toggles, colour bars, per-layer
       opacity / zoom range / fill colour, and the OSM vector path of every
       layer (Background, Landuse, Water, Roads with casing/fill/oneway
       arrows/name shields, Railways, Buildings, POIs).
     - Downloads tab: Tanzania cards (whole country + Dodoma, Dar es Salaam,
       North, Lake Zone, Southern Highlands). Download, watch the progress,
       then "Use this region" to serve it. Any PBF URL works via Custom URL.
   The store.bin from map-data is picked up automatically.

4. Optional web UI (needs Node.js 20+):

     cd C:\map-launcher\web
     set PORT=3000
     set HOSTNAME=127.0.0.1
     node server.js

   Open http://localhost:3000 in your browser: deep-zoom to z19, click roads
   for lane/direction/speed info, browse the full Carto palette.

HEADLESS (tile-server-cli)
--------------------------
  tile-server-cli serve [--port N] [--data-dir D]
  tile-server-cli extract <pbf> <outdir> [w s e n]
  tile-server-cli fetch <pbf-url> [outdir] [w s e n]   (download + extract,
                                                       resume + reconnect)
  tile-server GUI also accepts: tile-server serve / extract (no window)

DATA DIRECTORY RESOLUTION
-------------------------
--data-dir flag > MAP_LAUNCHER_DATA env > ./data > <exe dir>/data > <exe dir>.
The first candidate containing store.bin wins; missing data prints exactly
which folders were searched and how to fix it (or just use the Downloads tab).

Data (c) OpenStreetMap contributors, ODbL; extracts via download.geofabrik.de.
