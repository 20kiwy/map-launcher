import { NextResponse } from "next/server";

/**
 * Server-side proxy target for the Rust tile-server daemon.
 *
 * Default `http://127.0.0.1:3100` works both in the sandbox (daemon spawned
 * by /api/map-service) and in a local release deployment (tile-server.exe on
 * Windows). Override with the TILE_SERVER_URL env var when needed.
 */
export const TILE_SERVER_URL = process.env.TILE_SERVER_URL ?? "http://127.0.0.1:3100";

/** Forward an API request to the tile-server, preserving query + status. */
export async function proxyTileServer(request: Request, path: string): Promise<Response> {
  const search = new URL(request.url).search;
  try {
    const res = await fetch(`${TILE_SERVER_URL}${path}${search}`, {
      cache: "no-store",
      signal: AbortSignal.timeout(20_000),
    });
    const body = await res.arrayBuffer();
    return new Response(body, {
      status: res.status,
      headers: {
        "content-type": res.headers.get("content-type") ?? "application/json",
        "cache-control": "no-store",
      },
    });
  } catch (e) {
    return NextResponse.json(
      {
        error: "tile-server unreachable",
        hint: "start the tile-server daemon (:3100) or set TILE_SERVER_URL",
        detail: String(e),
      },
      { status: 502 },
    );
  }
}
