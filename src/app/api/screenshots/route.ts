import { NextResponse } from "next/server";
import { readFile, readdir, stat } from "fs/promises";
import path from "path";

/**
 * Serves the versioned egui launcher screenshots from
 * mini-services/map-launcher/screenshots/**
 *
 * GET /api/screenshots                  -> { versions: [{version, files:[{file, bytes}]}] }
 * GET /api/screenshots?v=v0.1.0&file=xxx.png -> PNG bytes
 */
const ROOT = path.join(
  process.cwd(),
  "mini-services",
  "map-launcher",
  "screenshots"
);

function safeJoin(root: string, rel: string): string | null {
  const p = path.normalize(path.join(root, rel));
  if (!p.startsWith(root)) return null;
  return p;
}

export async function GET(request: Request) {
  const { searchParams } = new URL(request.url);
  const version = searchParams.get("v");
  const file = searchParams.get("file");

  try {
    if (version && file) {
      const p = safeJoin(ROOT, path.join(version, path.basename(file)));
      if (!p) {
        return NextResponse.json({ error: "invalid path" }, { status: 400 });
      }
      const buf = await readFile(p);
      return new NextResponse(new Uint8Array(buf), {
        headers: {
          "Content-Type": "image/png",
          "Cache-Control": "public, max-age=60",
        },
      });
    }

    // list versions
    const entries = await readdir(ROOT, { withFileTypes: true });
    const versions: {
      version: string;
      files: { file: string; bytes: number }[];
    }[] = [];
    for (const e of entries) {
      if (!e.isDirectory() || !e.name.startsWith("v")) continue;
      const dir = path.join(ROOT, e.name);
      const files = await readdir(dir);
      const details = await Promise.all(
        files
          .filter((f) => f.endsWith(".png"))
          .sort()
          .map(async (f) => {
            const st = await stat(path.join(dir, f));
            return { file: f, bytes: st.size };
          })
      );
      versions.push({ version: e.name, files: details });
    }
    versions.sort((a, b) => b.version.localeCompare(a.version));
    return NextResponse.json({ versions });
  } catch (err) {
    return NextResponse.json(
      { error: "screenshots unavailable", detail: String(err), versions: [] },
      { status: 200 }
    );
  }
}
