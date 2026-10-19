import { spawn } from "child_process";
import { NextResponse } from "next/server";

/**
 * Ensures the Rust tile-server daemon (:3100) is running.
 *
 * The sandbox reaps orphan processes between tool sessions, so the daemon is
 * spawned as a child of the Next.js server (which the platform keeps alive).
 * GET /api/map-service:
 *   - service healthy  -> { ok: true, spawned: false, state: "online" }
 *   - service down     -> spawn scripts/dev.sh (build is cached; starts in ~2s)
 *                         and poll health for up to 20s
 */
const HEALTH_URL = "http://127.0.0.1:3100/api/health";
const DEV_SCRIPT = "/home/z/my-project/mini-services/map-launcher/scripts/dev.sh";

let lastSpawnAt = 0;

async function healthy(): Promise<boolean> {
  try {
    const res = await fetch(HEALTH_URL, {
      cache: "no-store",
      signal: AbortSignal.timeout(1500),
    });
    return res.ok;
  } catch {
    return false;
  }
}

export async function GET() {
  if (await healthy()) {
    return NextResponse.json({ ok: true, spawned: false, state: "online" });
  }

  // Windows release deployments start tile-server.exe manually (no bash /
  // cargo toolchain to spawn) — surface a clear state instead of spawning.
  if (process.platform === "win32") {
    return NextResponse.json(
      {
        ok: false,
        spawned: false,
        state: "offline",
        hint: "start tile-server.exe first (see the release README)",
      },
      { status: 503 },
    );
  }

  const now = Date.now();
  if (now - lastSpawnAt > 10_000) {
    lastSpawnAt = now;
    try {
      const child = spawn("bash", [DEV_SCRIPT], {
        detached: true,
        stdio: "ignore",
        env: {
          ...process.env,
          PATH: `${process.env.HOME ?? "/home/z"}/.cargo/bin:${process.env.PATH ?? ""}`,
        },
      });
      child.unref();
    } catch {
      // spawn failure surfaces as the 503 below
    }
  }

  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 600));
    if (await healthy()) {
      return NextResponse.json({ ok: true, spawned: true, state: "online" });
    }
  }
  return NextResponse.json(
    { ok: false, state: "starting", hint: "tile-server is rebuilding or data is missing" },
    { status: 503 }
  );
}
