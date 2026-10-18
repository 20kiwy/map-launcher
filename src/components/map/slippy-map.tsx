"use client";

import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from "react";
import { Home, Minus, Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { CARTO, tileUrl, type BBox, type LonLat } from "@/lib/map-api";

/* ------------------------------------------------------------------ */
/* Web-Mercator math                                                   */
/* ------------------------------------------------------------------ */

const TILE = 256;
const MIN_Z = 5;
const MAX_Z = 16;
const MAX_CACHE = 400;

interface View {
  lon: number;
  lat: number;
  z: number;
}

const INITIAL_VIEW: View = { lon: 35.7395, lat: -6.1662, z: 11 };

function lonToWorldX(lon: number, z: number): number {
  return ((lon + 180) / 360) * TILE * 2 ** z;
}

function latToWorldY(lat: number, z: number): number {
  const s = Math.sin((lat * Math.PI) / 180);
  const clamped = Math.max(Math.min(s, 1 - 1e-9), -1 + 1e-9);
  return (0.5 - Math.log((1 + clamped) / (1 - clamped)) / (4 * Math.PI)) * TILE * 2 ** z;
}

function worldXToLon(x: number, z: number): number {
  return (x / (TILE * 2 ** z)) * 360 - 180;
}

function worldYToLat(y: number, z: number): number {
  const n = Math.PI - (2 * Math.PI * y) / (TILE * 2 ** z);
  return (180 / Math.PI) * Math.atan(0.5 * (Math.exp(n) - Math.exp(-n)));
}

function clampZoom(z: number): number {
  return Math.max(MIN_Z, Math.min(MAX_Z, Math.round(z)));
}

/* ------------------------------------------------------------------ */
/* Tile LRU cache                                                      */
/* ------------------------------------------------------------------ */

function cacheGet(cache: Map<string, HTMLImageElement>, key: string): HTMLImageElement | undefined {
  const img = cache.get(key);
  if (img !== undefined) {
    cache.delete(key);
    cache.set(key, img); // refresh recency
  }
  return img;
}

function cachePut(cache: Map<string, HTMLImageElement>, key: string, img: HTMLImageElement): void {
  cache.delete(key);
  cache.set(key, img);
  while (cache.size > MAX_CACHE) {
    const oldest = cache.keys().next().value;
    if (oldest === undefined) break;
    cache.delete(oldest);
  }
}

/* ------------------------------------------------------------------ */
/* Public handle                                                       */
/* ------------------------------------------------------------------ */

export interface SlippyMapHandle {
  /** Zoom + center so the bbox fills the viewport. */
  fitBBox: (bbox: BBox) => void;
  /** Center on a point (default z 15). */
  flyTo: (lat: number, lon: number, z?: number) => void;
  /** Back to the initial Dodoma view. */
  resetView: () => void;
}

interface SlippyMapProps {
  /** Selected road geometry as [[lon, lat], ...] — drawn as polyline overlay. */
  roadPoints?: LonLat[] | null;
  roadLabel?: string | null;
  /** Selected POI — drawn as pin overlay. */
  poi?: { lat: number; lon: number; name: string } | null;
}

function projectToScreen(lon: number, lat: number, v: View, w: number, h: number): { x: number; y: number } {
  return {
    x: lonToWorldX(lon, v.z) - lonToWorldX(v.lon, v.z) + w / 2,
    y: latToWorldY(lat, v.z) - latToWorldY(v.lat, v.z) + h / 2,
  };
}

function drawPin(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  ctx.save();
  ctx.shadowColor = "rgba(0,0,0,0.35)";
  ctx.shadowBlur = 5;
  ctx.shadowOffsetY = 2;
  ctx.beginPath();
  ctx.arc(x, y - 20, 9, 0, Math.PI * 2);
  ctx.moveTo(x - 6.5, y - 14.5);
  ctx.lineTo(x, y);
  ctx.lineTo(x + 6.5, y - 14.5);
  ctx.closePath();
  ctx.fillStyle = CARTO.accent;
  ctx.fill();
  ctx.restore();
  ctx.beginPath();
  ctx.arc(x, y - 20, 3.5, 0, Math.PI * 2);
  ctx.fillStyle = "#ffffff";
  ctx.fill();
}

function drawLabel(
  ctx: CanvasRenderingContext2D,
  text: string,
  x: number,
  y: number,
  w: number
): void {
  ctx.save();
  ctx.font = "600 11px ui-monospace, SFMono-Regular, Menlo, monospace";
  const padX = 6;
  const tw = Math.min(ctx.measureText(text).width, 200);
  const bh = 18;
  let bx = x + 12;
  let by = y - 38;
  if (bx + tw + padX * 2 > w - 4) bx = w - tw - padX * 2 - 4;
  if (by < 4) by = y + 12;
  ctx.beginPath();
  ctx.roundRect(bx, by, tw + padX * 2, bh, 5);
  ctx.fillStyle = "rgba(255,255,255,0.92)";
  ctx.fill();
  ctx.strokeStyle = "rgba(44,44,44,0.25)";
  ctx.lineWidth = 1;
  ctx.stroke();
  ctx.fillStyle = CARTO.ink;
  ctx.textBaseline = "middle";
  ctx.fillText(text, bx + padX, by + bh / 2 + 0.5, tw);
  ctx.restore();
}

/* ------------------------------------------------------------------ */
/* Component                                                           */
/* ------------------------------------------------------------------ */

export const SlippyMap = forwardRef<SlippyMapHandle, SlippyMapProps>(function SlippyMap(
  { roadPoints = null, roadLabel = null, poi = null },
  ref
) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const baseRef = useRef<HTMLCanvasElement>(null);
  const overlayRef = useRef<HTMLCanvasElement>(null);
  const cacheRef = useRef<Map<string, HTMLImageElement>>(new Map());
  const pendingRef = useRef<Set<string>>(new Set());
  const failedRef = useRef<Set<string>>(new Set());
  const viewRef = useRef<View>({ ...INITIAL_VIEW });
  const sizeRef = useRef<{ w: number; h: number }>({ w: 0, h: 0 });
  const drawAllRef = useRef<() => void>(() => {});

  const [view, setViewState] = useState<View>({ ...INITIAL_VIEW });
  const [size, setSize] = useState<{ w: number; h: number }>({ w: 0, h: 0 });
  const [cacheSize, setCacheSize] = useState(0);
  const [failedTiles, setFailedTiles] = useState(0);

  const setView = useCallback((v: View) => {
    const next: View = { lon: v.lon, lat: v.lat, z: clampZoom(v.z) };
    viewRef.current = next;
    setViewState(next);
  }, []);

  /* ---------------- imperative navigation ---------------- */

  useImperativeHandle(
    ref,
    (): SlippyMapHandle => ({
      fitBBox: (bbox: BBox) => {
        const [w0, s0, e0, n0] = bbox;
        if (![w0, s0, e0, n0].every(Number.isFinite)) return;
        const { w, h } = sizeRef.current;
        if (w < 40 || h < 40) return;
        const pad = 48;
        let best = MIN_Z;
        for (let z = MAX_Z; z >= MIN_Z; z--) {
          const bw = Math.abs(lonToWorldX(e0, z) - lonToWorldX(w0, z));
          const bh = Math.abs(latToWorldY(s0, z) - latToWorldY(n0, z));
          if (bw <= Math.max(w - pad * 2, 60) && bh <= Math.max(h - pad * 2, 60)) {
            best = z;
            break;
          }
        }
        setView({ lon: (w0 + e0) / 2, lat: (s0 + n0) / 2, z: best });
      },
      flyTo: (lat: number, lon: number, z: number = 15) => {
        if (!Number.isFinite(lat) || !Number.isFinite(lon)) return;
        setView({ lon, lat, z });
      },
      resetView: () => setView({ ...INITIAL_VIEW }),
    }),
    [setView]
  );

  /* ---------------- pan / zoom primitives ---------------- */

  const panBy = useCallback(
    (dx: number, dy: number) => {
      const v = viewRef.current;
      const wx = lonToWorldX(v.lon, v.z) - dx;
      const wy = latToWorldY(v.lat, v.z) - dy;
      setView({ lon: worldXToLon(wx, v.z), lat: worldYToLat(wy, v.z), z: v.z });
    },
    [setView]
  );

  /** Zoom keeping the geographic point under (px, py) fixed on screen. */
  const zoomAtPoint = useCallback(
    (px: number, py: number, zNext: number) => {
      const { w, h } = sizeRef.current;
      if (w < 40 || h < 40) return;
      const z = clampZoom(zNext);
      const v = viewRef.current;
      if (z === v.z) return;
      const cx = lonToWorldX(v.lon, v.z);
      const cy = latToWorldY(v.lat, v.z);
      const geoX = cx + (px - w / 2);
      const geoY = cy + (py - h / 2);
      const geoLon = worldXToLon(geoX, v.z);
      const geoLat = worldYToLat(geoY, v.z);
      const wx = lonToWorldX(geoLon, z) - (px - w / 2);
      const wy = latToWorldY(geoLat, z) - (py - h / 2);
      setView({ lon: worldXToLon(wx, z), lat: worldYToLat(wy, z), z });
    },
    [setView]
  );

  /* ---------------- tile loading ---------------- */

  const requestTile = useCallback((key: string, z: number, x: number, y: number) => {
    if (pendingRef.current.has(key)) return;
    pendingRef.current.add(key);
    const img = new Image();
    img.decoding = "async";
    img.onload = () => {
      pendingRef.current.delete(key);
      failedRef.current.delete(key);
      cachePut(cacheRef.current, key, img);
      drawAllRef.current();
    };
    img.onerror = () => {
      pendingRef.current.delete(key);
      failedRef.current.add(key);
      if (failedRef.current.size > 800) failedRef.current.clear();
      setFailedTiles(failedRef.current.size);
      drawAllRef.current();
    };
    img.src = tileUrl(z, x, y);
  }, []);

  /* ---------------- drawing ---------------- */

  const drawTiles = useCallback(
    (ctx: CanvasRenderingContext2D, v: View, w: number, h: number, dpr: number) => {
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.fillStyle = "#e7e3d8"; // out-of-region backdrop
      ctx.fillRect(0, 0, w, h);

      const cx = lonToWorldX(v.lon, v.z);
      const cy = latToWorldY(v.lat, v.z);
      const x0 = cx - w / 2;
      const y0 = cy - h / 2;
      const tx0 = Math.floor(x0 / TILE);
      const ty0 = Math.floor(y0 / TILE);
      const tx1 = Math.floor((x0 + w) / TILE);
      const ty1 = Math.floor((y0 + h) / TILE);
      const n = 2 ** v.z;
      const cache = cacheRef.current;

      for (let ty = ty0; ty <= ty1; ty++) {
        if (ty < 0 || ty >= n) continue;
        for (let tx = tx0; tx <= tx1; tx++) {
          const wx = ((tx % n) + n) % n;
          const px = tx * TILE - x0;
          const py = ty * TILE - y0;
          const key = `${v.z}/${wx}/${ty}`;
          // graceful placeholder while loading / failed
          ctx.fillStyle = failedRef.current.has(key) ? "#e3ded1" : "#edeae0";
          ctx.fillRect(px, py, TILE, TILE);
          ctx.strokeStyle = "rgba(44,44,44,0.06)";
          ctx.lineWidth = 1;
          ctx.strokeRect(px + 0.5, py + 0.5, TILE - 1, TILE - 1);
          const img = cacheGet(cache, key);
          if (img !== undefined) {
            ctx.drawImage(img, px, py, TILE, TILE);
          } else if (!failedRef.current.has(key)) {
            requestTile(key, v.z, wx, ty);
          }
        }
      }
      setCacheSize(cache.size);
    },
    [requestTile]
  );

  const drawOverlay = useCallback(
    (ctx: CanvasRenderingContext2D, v: View, w: number, h: number, dpr: number) => {
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);

      if (roadPoints && roadPoints.length > 1) {
        const pts = roadPoints.map((p) => projectToScreen(p[0], p[1], v, w, h));
        ctx.lineJoin = "round";
        ctx.lineCap = "round";
        // soft halo
        ctx.beginPath();
        pts.forEach((p, i) => (i === 0 ? ctx.moveTo(p.x, p.y) : ctx.lineTo(p.x, p.y)));
        ctx.strokeStyle = "rgba(220,42,103,0.25)";
        ctx.lineWidth = 10;
        ctx.stroke();
        // core line
        ctx.strokeStyle = CARTO.accent;
        ctx.lineWidth = 4;
        ctx.stroke();
        // endpoints
        for (const p of [pts[0], pts[pts.length - 1]]) {
          ctx.beginPath();
          ctx.arc(p.x, p.y, 4.5, 0, Math.PI * 2);
          ctx.fillStyle = "#ffffff";
          ctx.fill();
          ctx.lineWidth = 2.5;
          ctx.strokeStyle = CARTO.accent;
          ctx.stroke();
        }
      }

      if (poi) {
        const p = projectToScreen(poi.lon, poi.lat, v, w, h);
        if (p.x > -40 && p.x < w + 40 && p.y > -40 && p.y < h + 40) {
          drawPin(ctx, p.x, p.y);
          drawLabel(ctx, poi.name, p.x, p.y, w);
        }
      }
    },
    [roadPoints, poi]
  );

  const drawAll = useCallback(() => {
    const { w, h } = sizeRef.current;
    const base = baseRef.current;
    const overlay = overlayRef.current;
    if (!base || !overlay || w < 2 || h < 2) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const bw = Math.round(w * dpr);
    const bh = Math.round(h * dpr);
    if (base.width !== bw || base.height !== bh) {
      base.width = bw;
      base.height = bh;
    }
    if (overlay.width !== bw || overlay.height !== bh) {
      overlay.width = bw;
      overlay.height = bh;
    }
    const v = viewRef.current;
    const baseCtx = base.getContext("2d");
    const overlayCtx = overlay.getContext("2d");
    if (baseCtx) drawTiles(baseCtx, v, w, h, dpr);
    if (overlayCtx) drawOverlay(overlayCtx, v, w, h, dpr);
  }, [drawTiles, drawOverlay]);

  useEffect(() => {
    drawAllRef.current = drawAll; // keep latest draw fn reachable from tile callbacks
    drawAll();
  }, [drawAll, view, size]);

  /* ---------------- resize ---------------- */

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect;
      if (!box) return;
      const w = Math.round(box.width);
      const h = Math.round(box.height);
      sizeRef.current = { w, h };
      setSize({ w, h });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  /* ---------------- pointer (pan + pinch) ---------------- */

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const pointers = new Map<number, { x: number; y: number }>();
    let lastPan: { x: number; y: number } | null = null;
    let pinchDist = 0;
    let zoomAcc = 0;

    const pos = (e: PointerEvent): { x: number; y: number } => {
      const r = el.getBoundingClientRect();
      return { x: e.clientX - r.left, y: e.clientY - r.top };
    };

    const onDown = (e: PointerEvent) => {
      el.setPointerCapture(e.pointerId);
      pointers.set(e.pointerId, pos(e));
      if (pointers.size === 1) {
        lastPan = pos(e);
      } else if (pointers.size === 2) {
        const [a, b] = [...pointers.values()];
        pinchDist = Math.hypot(a.x - b.x, a.y - b.y);
        lastPan = null;
        zoomAcc = 0;
      }
    };

    const onMove = (e: PointerEvent) => {
      if (!pointers.has(e.pointerId)) return;
      const p = pos(e);
      pointers.set(e.pointerId, p);

      if (pointers.size === 1 && lastPan !== null) {
        const dx = p.x - lastPan.x;
        const dy = p.y - lastPan.y;
        lastPan = p;
        if (dx !== 0 || dy !== 0) panBy(dx, dy);
      } else if (pointers.size === 2) {
        const [a, b] = [...pointers.values()];
        const d = Math.hypot(a.x - b.x, a.y - b.y);
        const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
        if (pinchDist > 0 && d > 0) {
          zoomAcc += Math.log2(d / pinchDist);
          if (zoomAcc >= 1) {
            zoomAcc = 0;
            zoomAtPoint(mid.x, mid.y, viewRef.current.z + 1);
          } else if (zoomAcc <= -1) {
            zoomAcc = 0;
            zoomAtPoint(mid.x, mid.y, viewRef.current.z - 1);
          }
        }
        pinchDist = d;
      }
    };

    const onUp = (e: PointerEvent) => {
      if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
      pointers.delete(e.pointerId);
      if (pointers.size === 1) {
        lastPan = [...pointers.values()][0] ?? null;
        pinchDist = 0;
        zoomAcc = 0;
      } else if (pointers.size === 0) {
        lastPan = null;
        pinchDist = 0;
        zoomAcc = 0;
      }
    };

    el.addEventListener("pointerdown", onDown);
    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerup", onUp);
    el.addEventListener("pointercancel", onUp);
    return () => {
      el.removeEventListener("pointerdown", onDown);
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerup", onUp);
      el.removeEventListener("pointercancel", onUp);
    };
  }, [panBy, zoomAtPoint]);

  /* ---------------- wheel (non-passive) ---------------- */

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    let acc = 0;
    let cooldown = 0;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const now = performance.now();
      if (now - cooldown > 200) acc = 0;
      acc += e.deltaY;
      if (Math.abs(acc) >= 50) {
        const r = el.getBoundingClientRect();
        const dir = acc > 0 ? -1 : 1;
        acc = 0;
        cooldown = now;
        zoomAtPoint(e.clientX - r.left, e.clientY - r.top, viewRef.current.z + dir);
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [zoomAtPoint]);

  /* ---------------- keyboard ---------------- */

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      const step = 96;
      switch (e.key) {
        case "ArrowLeft":
          panBy(step, 0);
          break;
        case "ArrowRight":
          panBy(-step, 0);
          break;
        case "ArrowUp":
          panBy(0, step);
          break;
        case "ArrowDown":
          panBy(0, -step);
          break;
        case "+":
        case "=":
          zoomAtPoint(sizeRef.current.w / 2, sizeRef.current.h / 2, viewRef.current.z + 1);
          break;
        case "-":
        case "_":
          zoomAtPoint(sizeRef.current.w / 2, sizeRef.current.h / 2, viewRef.current.z - 1);
          break;
        default:
          return;
      }
      e.preventDefault();
    },
    [panBy, zoomAtPoint]
  );

  /* ---------------- render ---------------- */

  const hasSelection = roadLabel !== null || poi !== null;

  return (
    <div
      ref={wrapRef}
      role="application"
      aria-label="Interactive slippy map of Central Tanzania. Drag to pan, scroll or use buttons to zoom."
      tabIndex={0}
      onKeyDown={onKeyDown}
      className="relative h-full w-full cursor-grab touch-none overflow-hidden select-none outline-none focus-visible:ring-2 focus-visible:ring-[#dc2a67]/60 active:cursor-grabbing"
    >
      <canvas ref={baseRef} className="absolute inset-0 h-full w-full" aria-hidden="true" />
      <canvas ref={overlayRef} className="pointer-events-none absolute inset-0 h-full w-full" aria-hidden="true" />

      {/* zoom controls */}
      <nav aria-label="Map zoom controls" className="absolute top-2 right-2 flex flex-col gap-1.5">
        <Button
          type="button"
          variant="outline"
          size="icon"
          className="size-11 border-black/10 bg-white/90 text-[#2c2c2c] shadow-sm hover:bg-white"
          aria-label="Zoom in"
          onClick={() => zoomAtPoint(size.w / 2, size.h / 2, view.z + 1)}
        >
          <Plus className="size-5" />
        </Button>
        <Button
          type="button"
          variant="outline"
          size="icon"
          className="size-11 border-black/10 bg-white/90 text-[#2c2c2c] shadow-sm hover:bg-white"
          aria-label="Zoom out"
          onClick={() => zoomAtPoint(size.w / 2, size.h / 2, view.z - 1)}
        >
          <Minus className="size-5" />
        </Button>
        <Button
          type="button"
          variant="outline"
          size="icon"
          className="size-11 border-black/10 bg-white/90 text-[#2c2c2c] shadow-sm hover:bg-white"
          aria-label="Reset view to Dodoma"
          onClick={() => setView({ ...INITIAL_VIEW })}
        >
          <Home className="size-5" />
        </Button>
      </nav>

      {/* HUD */}
      <div className="pointer-events-none absolute bottom-2 left-2 rounded-md bg-white/85 px-2.5 py-1.5 font-mono text-[11px] leading-tight text-[#2c2c2c] shadow-sm backdrop-blur-sm">
        z{view.z} · {view.lat.toFixed(4)}, {view.lon.toFixed(4)} · {cacheSize} tiles
        {failedTiles > 0 ? <span className="text-[#b45309]"> · {failedTiles} failed</span> : null}
      </div>

      {/* attribution */}
      <div className="pointer-events-none absolute right-2 bottom-2 rounded bg-white/75 px-1.5 py-0.5 text-[10px] text-[#2c2c2c]/80">
        © OpenStreetMap contributors
      </div>

      {/* selection banner */}
      {hasSelection ? (
        <div className="pointer-events-none absolute top-2 left-2 max-w-[calc(100%-4.5rem)] truncate rounded-md bg-[#141719]/85 px-2.5 py-1.5 text-[11px] font-medium text-[#f2efe9] shadow-sm backdrop-blur-sm">
          {roadLabel ? <span title={roadLabel}>{roadLabel}</span> : null}
          {roadLabel && poi ? " · " : ""}
          {poi ? <span>{poi.name}</span> : null}
        </div>
      ) : null}
    </div>
  );
});
