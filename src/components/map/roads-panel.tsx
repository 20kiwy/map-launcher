"use client";

import { useCallback, useMemo, useState } from "react";
import { Loader2, Search } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { ScrollArea, ScrollBar } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  formatLength,
  formatNumber,
  gwUrl,
  roadClassColor,
  type RoadDetail,
  type RoadSummary,
  type RoadsResponse,
} from "@/lib/map-api";
import { useFetch } from "@/lib/use-fetch";
import { CountLine, EmptyNote, ErrorNote, ListSkeleton } from "./panel-bits";

const ROADS_URL = gwUrl("/api/roads?limit=50000");
const MAX_ROWS = 300;

interface RoadsPanelProps {
  selectedRoadId: number | null;
  onSelectRoad: (detail: RoadDetail) => void;
}

export function RoadsPanel({ selectedRoadId, onSelectRoad }: RoadsPanelProps) {
  const { data, error, loading, retry } = useFetch<RoadsResponse>(ROADS_URL);
  const [query, setQuery] = useState("");
  const [loadingId, setLoadingId] = useState<number | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);

  const roads = useMemo(() => data?.roads ?? [], [data]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q === "") return roads;
    return roads.filter(
      (r) =>
        (r.name ?? "").toLowerCase().includes(q) || (r.class ?? "").toLowerCase().includes(q)
    );
  }, [roads, query]);

  const visible = useMemo(() => filtered.slice(0, MAX_ROWS), [filtered]);

  const openRoad = useCallback(
    async (road: RoadSummary) => {
      setLoadingId(road.id);
      setDetailError(null);
      try {
        const res = await fetch(gwUrl(`/api/roads/${road.id}`));
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const detail = (await res.json()) as RoadDetail;
        onSelectRoad(detail);
      } catch (err: unknown) {
        setDetailError(err instanceof Error ? err.message : "failed to load road geometry");
      } finally {
        setLoadingId(null);
      }
    },
    [onSelectRoad]
  );

  if (loading) {
    return (
      <div className="flex flex-col gap-2.5">
        <Skeleton className="h-11 w-full" />
        <ListSkeleton rows={7} />
      </div>
    );
  }

  if (error) {
    return <ErrorNote title="Could not load the roads registry" detail={error} onRetry={retry} />;
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
          placeholder="Search roads by name or class…"
          aria-label="Search roads"
          className="min-h-11 border-[#2c2c2c]/15 bg-white pl-8 text-[13px]"
        />
      </div>

      <CountLine>
        <span>
          {formatNumber(data?.count ?? roads.length)} roads registered
          {query !== "" ? ` · ${formatNumber(filtered.length)} match` : ""}
        </span>
        {filtered.length > MAX_ROWS ? <span>showing first {MAX_ROWS}</span> : null}
      </CountLine>

      {detailError ? (
        <p role="alert" className="rounded-md bg-[#dc2a67]/10 px-2.5 py-1.5 text-[11px] text-[#8f1f48]">
          {detailError}
        </p>
      ) : null}

      {visible.length === 0 ? (
        <EmptyNote>
          no roads match “{query}” — try a shorter query (e.g. “dodoma”, “primary”)
        </EmptyNote>
      ) : (
        <ScrollArea className="min-h-0 max-h-[55vh] flex-1 lg:max-h-none">
          <ul className="flex flex-col gap-1 pr-1">
            {visible.map((road) => {
              const selected = road.id === selectedRoadId;
              return (
                <li key={road.id}>
                  <button
                    type="button"
                    onClick={() => void openRoad(road)}
                    aria-pressed={selected}
                    className={`flex min-h-11 w-full items-center gap-2 rounded-md border px-2 py-1.5 text-left transition-colors ${
                      selected
                        ? "border-[#dc2a67]/40 bg-[#fbe4ec]"
                        : "border-transparent hover:bg-[#ece8de]"
                    }`}
                  >
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[13px] font-medium text-[#2c2c2c]">
                        {road.name === "" ? (
                          <span className="text-[#8b877e] italic">(unnamed)</span>
                        ) : (
                          road.name
                        )}
                      </span>
                      <span className="block truncate font-mono text-[10.5px] text-[#6b6b66]">
                        {formatLength(road.length_m)} · id {road.id}
                      </span>
                    </span>
                    <Badge
                      className="border border-black/10 text-[10px] text-[#2c2c2c]"
                      style={{ backgroundColor: roadClassColor(road.class) }}
                    >
                      {road.class}
                    </Badge>
                    {loadingId === road.id ? (
                      <Loader2
                        className="size-4 shrink-0 animate-spin text-[#6b6b66]"
                        aria-label="Loading road geometry"
                      />
                    ) : null}
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
