import { proxyTileServer } from "@/lib/tile-proxy";

export const dynamic = "force-dynamic";

/** GET /api/health -> tile-server liveness (proxied). */
export async function GET(request: Request) {
  return proxyTileServer(request, "/api/health");
}
