import { proxyTileServer } from "@/lib/tile-proxy";

export const dynamic = "force-dynamic";

/** GET /api/palette -> parsed openstreetmap-carto palette (proxied). */
export async function GET(request: Request) {
  return proxyTileServer(request, "/api/palette");
}
