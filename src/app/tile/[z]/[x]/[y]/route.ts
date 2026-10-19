import { TILE_SERVER_URL } from "@/lib/tile-proxy";

export const dynamic = "force-dynamic";

/**
 * GET /tile/:z/:x/:y.png -> server-rendered carto PNG tile (proxied).
 *
 * Same-origin binary streaming keeps the canvas untainted and works without
 * the sandbox gateway, so a local release build (tile-server.exe + the
 * standalone Next.js server) renders identically.
 */
export async function GET(
  _request: Request,
  ctx: { params: Promise<{ z: string; x: string; y: string }> },
) {
  const { z, x, y } = await ctx.params;
  const file = y.endsWith(".png") ? y : `${y}.png`;
  try {
    const upstream = await fetch(`${TILE_SERVER_URL}/tile/${z}/${x}/${file}`, {
      cache: "no-store",
      signal: AbortSignal.timeout(20_000),
    });
    if (!upstream.ok || !upstream.body) {
      return new Response("tile unavailable", { status: upstream.status || 502 });
    }
    return new Response(upstream.body, {
      status: 200,
      headers: {
        "content-type": "image/png",
        "cache-control": "public, max-age=86400",
      },
    });
  } catch {
    return new Response("tile-server unreachable", { status: 502 });
  }
}
