"use client";

import { useMemo, useState } from "react";
import { MapPin, Search } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { ScrollArea, ScrollBar } from "@/components/ui/scroll-area";
import {
  formatNumber,
  gwUrl,
  prettyCategory,
  type Poi,
  type PoisResponse,
} from "@/lib/map-api";
import { useFetch } from "@/lib/use-fetch";
import { CountLine, EmptyNote, ErrorNote, ListSkeleton } from "./panel-bits";

const POIS_URL = gwUrl("/api/pois?limit=20000");
const MAX_ROWS = 300;

interface PoisPanelProps {
  selectedPoiId: number | null;
  onSelectPoi: (poi: Poi) => void;
}

export function PoisPanel({ selectedPoiId, onSelectPoi }: PoisPanelProps) {
  const { data, error, loading, retry } = useFetch<PoisResponse>(POIS_URL);
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<string | null>(null);

  const pois = useMemo(() => data?.pois ?? [], [data]);

  const categories = useMemo(() => {
    const counts = new Map<string, number>();
    for (const p of pois) counts.set(p.category, (counts.get(p.category) ?? 0) + 1);
    return [...counts.entries()].sort((a, b) => b[1] - a[1]);
  }, [pois]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return pois.filter(
      (p) =>
        (category === null || p.category === category) &&
        (q === "" ||
          (p.name ?? "").toLowerCase().includes(q) ||
          (p.category ?? "").toLowerCase().includes(q))
    );
  }, [pois, query, category]);

  const visible = useMemo(() => filtered.slice(0, MAX_ROWS), [filtered]);

  if (loading) return <ListSkeleton rows={8} />;

  if (error) {
    return <ErrorNote title="Could not load points of interest" detail={error} onRetry={retry} />;
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2.5">
      <div className="relative">
        <Search
          className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-[#8b877e]"
          aria-hidden="true"
        />
        <Input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search POIs by name or category…"
          aria-label="Search points of interest"
          className="min-h-11 border-[#2c2c2c]/15 bg-white pl-8 text-[13px]"
        />
      </div>

      {/* category chips with counts */}
      {categories.length > 0 ? (
        <div className="max-h-24 overflow-y-auto pr-1 [scrollbar-color:#c9c3b6_transparent] [scrollbar-width:thin]">
          <div className="flex flex-wrap gap-1.5" role="group" aria-label="Filter by category">
            <Chip
              active={category === null}
              label={`all · ${formatNumber(pois.length)}`}
              onClick={() => setCategory(null)}
            />
            {categories.map(([cat, count]) => (
              <Chip
                key={cat}
                active={category === cat}
                label={`${prettyCategory(cat)} · ${count}`}
                onClick={() => setCategory(category === cat ? null : cat)}
              />
            ))}
          </div>
        </div>
      ) : null}

      <CountLine>
        <span>
          {formatNumber(data?.count ?? pois.length)} points · {categories.length} categories
          {query !== "" || category !== null ? ` · ${formatNumber(filtered.length)} match` : ""}
        </span>
        {filtered.length > MAX_ROWS ? <span>showing first {MAX_ROWS}</span> : null}
      </CountLine>

      {visible.length === 0 ? (
        <EmptyNote>no points match the current filter — clear the search or category chip</EmptyNote>
      ) : (
        <ScrollArea className="min-h-0 max-h-[55vh] flex-1 lg:max-h-none">
          <ul className="flex flex-col gap-1 pr-1">
            {visible.map((poi) => {
              const selected = poi.id === selectedPoiId;
              return (
                <li key={poi.id}>
                  <button
                    type="button"
                    onClick={() => onSelectPoi(poi)}
                    aria-pressed={selected}
                    className={`flex min-h-11 w-full items-center gap-2 rounded-md border px-2 py-1.5 text-left transition-colors ${
                      selected
                        ? "border-[#dc2a67]/40 bg-[#fbe4ec]"
                        : "border-transparent hover:bg-[#ece8de]"
                    }`}
                  >
                    <MapPin
                      className="size-4 shrink-0 text-[#dc2a67]"
                      aria-hidden="true"
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[13px] font-medium text-[#2c2c2c]">
                        {poi.name === "" ? (
                          <span className="text-[#8b877e] italic">(unnamed)</span>
                        ) : (
                          poi.name
                        )}
                      </span>
                      <span className="block truncate font-mono text-[10.5px] text-[#6b6b66]">
                        {poi.lat.toFixed(5)}, {poi.lon.toFixed(5)}
                      </span>
                    </span>
                    <Badge
                      variant="outline"
                      className="shrink-0 border-[#2c2c2c]/15 bg-white text-[10px] font-normal text-[#55524b]"
                    >
                      {poi.category}
                    </Badge>
                  </button>
                </li>
              );
            })}
          </ul>
          <ScrollBar className="bg-transparent [&_[data-slot=scroll-area-thumb]]:bg-[#c9c3b6]" />
        </ScrollArea>
      )}
    </div>
  );
}

function Chip({
  active,
  label,
  onClick,
}: {
  active: boolean;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={`min-h-7 rounded-full border px-2.5 py-1 text-[11px] leading-none transition-colors ${
        active
          ? "border-[#2c2c2c] bg-[#2c2c2c] text-[#f2efe9]"
          : "border-[#2c2c2c]/15 bg-white text-[#55524b] hover:border-[#2c2c2c]/35"
      }`}
    >
      {label}
    </button>
  );
}
