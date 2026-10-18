"use client";

import { Camera, ExternalLink } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { formatBytes, formatNumber, type ScreenshotsResponse } from "@/lib/map-api";
import { useFetch } from "@/lib/use-fetch";
import { ErrorNote } from "./panel-bits";

function screenshotHref(version: string, file: string): string {
  return `/api/screenshots?v=${encodeURIComponent(version)}&file=${encodeURIComponent(file)}`;
}

export function ScreenshotsStrip() {
  const { data, error, loading, retry } = useFetch<ScreenshotsResponse>("/api/screenshots");

  const newest = data?.versions?.[0] ?? null;
  const versionCount = data?.versions?.length ?? 0;

  return (
    <Card className="gap-3 p-4 sm:p-5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <h2 className="flex items-center gap-2 text-sm font-semibold text-[#2c2c2c]">
          <Camera className="size-4 text-[#dc2a67]" aria-hidden="true" />
          Launcher screenshots
        </h2>
        {newest ? (
          <Badge variant="outline" className="border-[#2c2c2c]/15 bg-white font-mono text-[10px] text-[#55524b]">
            {newest.version} · {formatNumber(newest.files.length)} image
            {newest.files.length === 1 ? "" : "s"}
            {versionCount > 1 ? ` · ${versionCount} versions` : ""}
          </Badge>
        ) : null}
      </div>

      {loading ? (
        <div className="flex gap-3" aria-hidden="true">
          {Array.from({ length: 4 }, (_, i) => (
            <div key={i} className="h-24 w-44 animate-pulse rounded-md bg-[#e9e5da]" />
          ))}
        </div>
      ) : error ? (
        <ErrorNote title="Could not list launcher screenshots" detail={error} onRetry={retry} />
      ) : !newest || newest.files.length === 0 ? (
        <p className="rounded-md border border-dashed border-[#2c2c2c]/15 bg-white/40 px-3 py-4 text-center text-xs text-[#6b6b66]">
          no screenshots yet — run the egui launcher to capture some
        </p>
      ) : (
        <ul className="flex gap-3 overflow-x-auto pb-1 [scrollbar-color:#c9c3b6_transparent] [scrollbar-width:thin]">
          {newest.files.map((f) => (
            <li key={f.file} className="shrink-0">
              <a
                href={screenshotHref(newest.version, f.file)}
                target="_blank"
                rel="noopener noreferrer"
                title={`${f.file} — open in new tab`}
                className="group block rounded-md border border-[#2c2c2c]/10 bg-white p-1.5 transition-colors hover:border-[#dc2a67]/50"
              >
                {/* Next lint config does not flag this img; plain <img> is intentional
                    since the PNG is served by a route handler, not a static asset. */}
                <img
                  src={screenshotHref(newest.version, f.file)}
                  alt={`egui launcher screenshot ${f.file}`}
                  loading="lazy"
                  className="max-h-24 w-auto rounded border border-[#2c2c2c]/10"
                />
                <span className="mt-1 flex max-w-44 items-center gap-1 text-[10px] text-[#6b6b66] group-hover:text-[#dc2a67]">
                  <span className="truncate font-mono">{f.file}</span>
                  <span className="shrink-0 font-mono opacity-70">{formatBytes(f.bytes)}</span>
                  <ExternalLink className="size-3 shrink-0" aria-hidden="true" />
                </span>
              </a>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}
