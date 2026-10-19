import { proxyTileServer } from "@/lib/tile-proxy";

export const dynamic = "force-dynamic";

/** GET /api/roads/:id -> road geometry + tags (proxied). */
export async function GET(request: Request, ctx: { params: Promise<{ id: string }> }) {
  const { id } = await ctx.params;
  return proxyTileServer(request, `/api/roads/${encodeURIComponent(id)}`);
}
