import { proxyTileServer } from "@/lib/tile-proxy";

export const dynamic = "force-dynamic";

/** GET /api/stats -> registry counts + bbox (proxied to tile-server :3100). */
export async function GET(request: Request) {
  return proxyTileServer(request, "/api/stats");
}
