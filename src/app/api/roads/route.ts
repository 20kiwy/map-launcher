import { proxyTileServer } from "@/lib/tile-proxy";

export const dynamic = "force-dynamic";

/** GET /api/roads?limit=N -> roads registry, named first (proxied). */
export async function GET(request: Request) {
  return proxyTileServer(request, "/api/roads");
}
