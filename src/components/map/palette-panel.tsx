"use client";

import { ScrollArea, ScrollBar } from "@/components/ui/scroll-area";
import { formatNumber, gwUrl, type PaletteResponse, type PaletteRole } from "@/lib/map-api";
import { useFetch } from "@/lib/use-fetch";
import { EmptyNote, ErrorNote, ListSkeleton } from "./panel-bits";

const PALETTE_URL = gwUrl("/api/palette");

export function PalettePanel() {
  const { data, error, loading, retry } = useFetch<PaletteResponse>(PALETTE_URL);

  if (loading) return <ListSkeleton rows={8} />;

  if (error) {
    return <ErrorNote title="Could not load the MSS palette" detail={error} onRetry={retry} />;
  }

  const roles = data?.roles ?? [];

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2.5">
      <p className="rounded-md bg-[#eef0d5] px-2.5 py-2 text-[11px] text-[#55524b]">
        <span className="font-semibold text-[#2c2c2c]">{formatNumber(data?.var_count ?? 0)}</span>{" "}
        MSS variables ·{" "}
        <span className="font-semibold text-[#2c2c2c]">{formatNumber(data?.color_count ?? 0)}</span>{" "}
        distinct colors ·{" "}
        <span className="font-semibold text-[#2c2c2c]">{formatNumber(roles.length)}</span> semantic
        roles — vendored from openstreetmap-carto <span className="font-mono">style.mss</span>
      </p>

      {roles.length === 0 ? (
        <EmptyNote>the palette registry is empty</EmptyNote>
      ) : (
        <ScrollArea className="min-h-0 max-h-[55vh] flex-1 lg:max-h-none">
          <ul className="grid grid-cols-1 gap-1.5 pr-1">
            {roles.map((role: PaletteRole) => (
              <li
                key={role.role + role.var}
                className="flex min-h-11 items-center gap-2.5 rounded-md border border-transparent px-1.5 py-1.5 transition-colors hover:bg-[#ece8de]"
              >
                <span
                  className="size-7 shrink-0 rounded border border-black/15 shadow-inner"
                  style={{ backgroundColor: role.hex }}
                  aria-hidden="true"
                />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[12.5px] font-medium text-[#2c2c2c]">
                    {role.role}
                  </span>
                  <span className="block truncate font-mono text-[10.5px] text-[#6b6b66]">
                    {role.var}
                  </span>
                </span>
                <span className="shrink-0 text-right">
                  <span className="block font-mono text-[11px] text-[#2c2c2c]">{role.hex}</span>
                  <span className="block font-mono text-[10px] text-[#8b877e]">{role.source}</span>
                </span>
              </li>
            ))}
          </ul>
          <ScrollBar className="bg-transparent [&_[data-slot=scroll-area-thumb]]:bg-[#c9c3b6]" />
        </ScrollArea>
      )}
    </div>
  );
}
