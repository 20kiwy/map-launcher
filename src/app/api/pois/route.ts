import { proxyTileServer } from "@/lib/tile-proxy";

export const dynamic = "force-dynamic";

/** GET /api/pois?limit=N -> POI registry (proxied). */
export async function GET(request: Request) {
  return proxyTileServer(request, "/api/pois");
}
