"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Building2, MapPin, RefreshCw, Route, Tags } from "lucide-react";
import { Card } from "@/components/ui/card";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PalettePanel } from "@/components/map/palette-panel";
import { PoisPanel } from "@/components/map/pois-panel";
import { RoadsPanel } from "@/components/map/roads-panel";
import { ScreenshotsStrip } from "@/components/map/screenshots-strip";
import { SlippyMap, type SlippyMapHandle } from "@/components/map/slippy-map";
import {
  CARTO,
  formatNumber,
  gwUrl,
  type LonLat,
  type Poi,
  type RoadDetail,
  type Stats,
} from "@/lib/map-api";
import { useFetch } from "@/lib/use-fetch";

const STATS_URL = gwUrl("/api/stats");

/** bbox [w, s, e, n] from a list of [lon, lat] points. */
function bboxOfPoints(points: LonLat[]): [number, number, number, number] {
  let w = Number.POSITIVE_INFINITY;
  let s = Number.POSITIVE_INFINITY;
  let e = Number.NEGATIVE_INFINITY;
  let n = Number.NEGATIVE_INFINITY;
  for (const [lon, lat] of points) {
    if (lon < w) w = lon;
    if (lat < s) s = lat;
    if (lon > e) e = lon;
    if (lat > n) n = lat;
  }
  if (!Number.isFinite(w) || !Number.isFinite(s)) return [0, 0, 0, 0];
  return [w, s, e, n];
}

function scrollToMapOnMobile(): void {
  if (typeof window !== "undefined" && window.matchMedia("(max-width: 1023px)").matches) {
    window.scrollTo({ top: 0, behavior: "smooth" });
  }
}

function StatChip({
  icon,
  label,
  value,
}: {
  icon: ReactNode;
  label: string;
  value: string;
}) {
  return (
    <span
      className="inline-flex items-center gap-1.5 rounded-full bg-white/10 px-2.5 py-1 text-[11px] text-[#aeb6ac]"
      title={label}
    >
      {icon}
      <span className="font-semibold text-[#f2efe9]">{value}</span>
      <span className="hidden sm:inline">{label}</span>
    </span>
  );
}

export default function Home() {
  const mapRef = useRef<SlippyMapHandle | null>(null);
  const stats = useFetch<Stats>(STATS_URL);
  const [roadDetail, setRoadDetail] = useState<RoadDetail | null>(null);
  const [poi, setPoi] = useState<Poi | null>(null);
  const [serviceEpoch, setServiceEpoch] = useState(0);
  const statsHadError = useRef(false);
  const retryRef = useRef<() => void>(() => {});

  // Keep a stable handle on the stats retry (written inside an effect).
  useEffect(() => {
    retryRef.current = stats.retry;
  }, [stats.retry]);

  // Watch for failed stats fetches (no setState here).
  useEffect(() => {
    if (stats.error) statsHadError.current = true;
  }, [stats.error]);

  /** Make the Next.js server adopt/spawn the tile-server daemon, then
   *  refresh stats; remount registry panels if they had errored. */
  const ensureAndRetry = useCallback(() => {
    void (async () => {
      try {
        await fetch("/api/map-service", { cache: "no-store" });
      } catch {
        // ignore — the stats fetch below reports the real status
      }
      if (statsHadError.current) {
        statsHadError.current = false;
        setServiceEpoch((e) => e + 1);
      }
      retryRef.current();
    })();
  }, []);

  // On mount: make sure the daemon is up, then refresh stats.
  useEffect(() => {
    const t = setTimeout(ensureAndRetry, 250);
    return () => clearTimeout(t);
  }, [ensureAndRetry]);

  const handleSelectRoad = useCallback((detail: RoadDetail) => {
    setRoadDetail(detail);
    if (detail.points.length > 1) {
      mapRef.current?.fitBBox(bboxOfPoints(detail.points));
    }
    scrollToMapOnMobile();
  }, []);

  const handleSelectPoi = useCallback((selected: Poi) => {
    setPoi(selected);
    mapRef.current?.flyTo(selected.lat, selected.lon, 15);
    scrollToMapOnMobile();
  }, []);

  const online = stats.data !== null && stats.error === null;

  return (
    <div
      className="flex min-h-screen flex-col text-[#2c2c2c]"
      style={{ backgroundColor: CARTO.land }}
    >
      {/* ---------------- header / hero ---------------- */}
      <header className="sticky top-0 z-30 border-b border-white/10 text-[#f2efe9] shadow-sm" style={{ backgroundColor: CARTO.headerBg }}>
        <div className="mx-auto flex w-full max-w-[1600px] flex-wrap items-center gap-x-4 gap-y-2 px-4 py-2.5 lg:px-6 lg:py-3">
          <div className="min-w-0">
            <h1 className="truncate text-base font-semibold tracking-tight sm:text-lg">
              map-launcher — Central Tanzania
            </h1>
            <p className="truncate text-[11px] text-[#9aa39b] sm:text-xs">
              openstreetmap-carto full palette · Rust tile server
            </p>
          </div>

          <div className="ml-auto flex flex-wrap items-center gap-1.5">
            <StatChip
              icon={<Route className="size-3.5 text-[#e892a2]" aria-hidden="true" />}
              label="roads"
              value={stats.data ? formatNumber(stats.data.roads) : "—"}
            />
            <StatChip
              icon={<Tags className="size-3.5 text-[#fcd6a4]" aria-hidden="true" />}
              label="named"
              value={stats.data ? formatNumber(stats.data.named_roads) : "—"}
            />
            <StatChip
              icon={<MapPin className="size-3.5 text-[#dc2a67]" aria-hidden="true" />}
              label="POIs"
              value={stats.data ? formatNumber(stats.data.pois) : "—"}
            />
            <StatChip
              icon={<Building2 className="size-3.5 text-[#d9d0c9]" aria-hidden="true" />}
              label="buildings"
              value={stats.data ? formatNumber(stats.data.buildings) : "—"}
            />

            <button
              type="button"
              onClick={ensureAndRetry}
              aria-live="polite"
              aria-label={
                stats.loading
                  ? "Checking tile server status"
                  : online
                    ? "Tile server online"
                    : "Tile server offline — click to retry"
              }
              className="inline-flex min-h-9 items-center gap-2 rounded-full border border-white/15 px-3 py-1.5 text-xs font-medium transition-colors hover:bg-white/10"
            >
              <span
                aria-hidden="true"
                className={`size-2 rounded-full ${
                  stats.loading
                    ? "animate-pulse bg-amber-400"
                    : online
                      ? "bg-emerald-400 shadow-[0_0_6px_rgba(52,211,153,0.9)]"
                      : "bg-red-400 shadow-[0_0_6px_rgba(248,113,113,0.9)]"
                }`}
              />
              {stats.loading ? "checking…" : online ? "tile server online" : "tile server offline"}
              {!online && !stats.loading ? (
                <RefreshCw className="size-3.5" aria-hidden="true" />
              ) : null}
            </button>
          </div>
        </div>
      </header>

      {/* ---------------- main ---------------- */}
      <main className="mx-auto w-full max-w-[1600px] flex-1 px-4 py-4 lg:px-6 lg:py-5">
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
          {/* map */}
          <section aria-label="Map of Central Tanzania" className="lg:col-span-2">
            <Card className="gap-0 overflow-hidden p-0">
              <div className="h-[420px] w-full sm:h-[480px] lg:h-[calc(100dvh-12.5rem)] lg:min-h-[540px]">
                <SlippyMap
                  ref={mapRef}
                  roadPoints={roadDetail ? roadDetail.points : null}
                  roadLabel={roadDetail ? roadDetail.name || "(unnamed)" : null}
                  poi={
                    poi
                      ? { lat: poi.lat, lon: poi.lon, name: poi.name || "(unnamed)" }
                      : null
                  }
                />
              </div>
            </Card>
          </section>

          {/* registries */}
          <aside aria-label="Data registries" className="lg:h-[calc(100dvh-12.5rem)] lg:min-h-[540px]">
            <Card className="flex h-full flex-col gap-3 p-4">
              <Tabs
                defaultValue="roads"
                key={serviceEpoch}
                className="flex min-h-0 flex-1 flex-col gap-3"
              >
                <TabsList className="h-auto w-full grid-cols-3">
                  <TabsTrigger value="roads" className="min-h-9 gap-1.5">
                    <Route className="size-4" aria-hidden="true" />
                    Roads
                  </TabsTrigger>
                  <TabsTrigger value="points" className="min-h-9 gap-1.5">
                    <MapPin className="size-4" aria-hidden="true" />
                    Points
                  </TabsTrigger>
                  <TabsTrigger value="palette" className="min-h-9 gap-1.5">
                    <Tags className="size-4" aria-hidden="true" />
                    Palette
                  </TabsTrigger>
                </TabsList>

                <TabsContent
                  value="roads"
                  forceMount
                  className="flex min-h-0 flex-1 flex-col data-[state=inactive]:hidden"
                >
                  <RoadsPanel selectedRoadId={roadDetail?.id ?? null} onSelectRoad={handleSelectRoad} />
                </TabsContent>

                <TabsContent
                  value="points"
                  forceMount
                  className="flex min-h-0 flex-1 flex-col data-[state=inactive]:hidden"
                >
                  <PoisPanel selectedPoiId={poi?.id ?? null} onSelectPoi={handleSelectPoi} />
                </TabsContent>

                <TabsContent
                  value="palette"
                  forceMount
                  className="flex min-h-0 flex-1 flex-col data-[state=inactive]:hidden"
                >
                  <PalettePanel />
                </TabsContent>
              </Tabs>
            </Card>
          </aside>
        </div>

        {/* screenshots strip */}
        <section aria-label="Launcher screenshots" className="mt-4">
          <ScreenshotsStrip />
        </section>
      </main>

      {/* ---------------- sticky footer ---------------- */}
      <footer className="mt-auto border-t border-white/10 text-[#9aa39b]" style={{ backgroundColor: CARTO.headerBg }}>
        <div className="mx-auto w-full max-w-[1600px] px-4 py-3 pb-[max(0.75rem,env(safe-area-inset-bottom))] text-center text-[11px] leading-relaxed lg:px-6">
          data © OpenStreetMap contributors · style: openstreetmap-carto (MSS palette vendored) ·
          renderer: Rust carto-core + tiny-skia · UI inspired by craft-launcher
        </div>
      </footer>
    </div>
  );
}
