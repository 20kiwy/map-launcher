/**
 * Shared types + helpers for the map-launcher preview UI.
 *
 * The Rust tile-server daemon listens on an internal port (3100) and is only
 * reachable from the browser through the gateway: relative URLs carrying the
 * `XTransformPort=3100` query param. Never use absolute URLs or other ports.
 */

export const TILE_SERVER_PORT = 3100;

/** Wrap a relative API path so the gateway forwards it to the tile server. */
export function gwUrl(path: string): string {
  const sep = path.includes("?") ? "&" : "?";
  return `${path}${sep}XTransformPort=${TILE_SERVER_PORT}`;
}

/** XYZ tile URL routed through the gateway. */
export function tileUrl(z: number, x: number, y: number): string {
  return `/tile/${z}/${x}/${y}.png?XTransformPort=${TILE_SERVER_PORT}`;
}

/* ------------------------------------------------------------------ */
/* Server response types                                               */
/* ------------------------------------------------------------------ */

/** [west, south, east, north] in degrees. */
export type BBox = [number, number, number, number];
/** [lon, lat] pairs. */
export type LonLat = [number, number];

export interface Stats {
  roads: number;
  named_roads: number;
  railways: number;
  waterways: number;
  water_areas: number;
  landuse: number;
  buildings: number;
  pois: number;
  bounds: BBox;
}

export interface RoadSummary {
  id: number;
  name: string;
  class: string;
  length_m: number;
  bbox: BBox;
  named: boolean;
}

export interface RoadsResponse {
  count: number;
  roads: RoadSummary[];
}

export interface RoadDetail {
  id: number;
  name: string;
  class: string;
  length_m: number;
  points: LonLat[];
}

export interface Poi {
  id: number;
  name: string;
  category: string;
  lat: number;
  lon: number;
}

export interface PoisResponse {
  count: number;
  pois: Poi[];
}

export interface PaletteRole {
  role: string;
  var: string;
  hex: string;
  rgba: [number, number, number, number?];
  source: string;
}

export interface PaletteResponse {
  var_count: number;
  color_count: number;
  roles: PaletteRole[];
}

export interface ScreenshotVersion {
  version: string;
  files: { file: string; bytes: number }[];
}

export interface ScreenshotsResponse {
  versions: ScreenshotVersion[];
}

/* ------------------------------------------------------------------ */
/* openstreetmap-carto palette tokens                                  */
/* ------------------------------------------------------------------ */

export const CARTO = {
  land: "#f2efe9",
  ink: "#2c2c2c",
  accent: "#dc2a67", // motorway casing — used as UI accent
  water: "#aad3df",
  forest: "#add19e",
  farmland: "#eef0d5",
  building: "#d9d0c9",
  headerBg: "#141719",
} as const;

/** Badge fill colors per road class (openstreetmap-carto line colors). */
export const ROAD_CLASS_COLORS: Record<string, string> = {
  motorway: "#e892a2",
  trunk: "#f9b29c",
  primary: "#fcd6a4",
  secondary: "#f7fabf",
  tertiary: "#ffffff",
  residential: "#ffffff",
  unclassified: "#ffffff",
  living_street: "#ffffff",
  pedestrian: "#ffffff",
  service: "#e2e0da",
  track: "#d7937c",
  footway: "#fa8072",
  path: "#fa8072",
  cycleway: "#fa8072",
  steps: "#fa8072",
};

export function roadClassColor(cls: string): string {
  return ROAD_CLASS_COLORS[cls] ?? "#ece8de";
}

/* ------------------------------------------------------------------ */
/* Small formatting helpers                                            */
/* ------------------------------------------------------------------ */

export function formatNumber(n: number): string {
  return n.toLocaleString("en-US");
}

export function formatLength(meters: number): string {
  if (!Number.isFinite(meters)) return "—";
  if (meters < 1000) return `${Math.round(meters)} m`;
  return `${(meters / 1000).toFixed(meters < 10_000 ? 2 : 1)} km`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** "amenity_hospital" -> "amenity · hospital" (prettier chip label). */
export function prettyCategory(category: string): string {
  const i = category.indexOf("_");
  if (i <= 0) return category;
  return `${category.slice(0, i)} · ${category.slice(i + 1).replace(/_/g, " ")}`;
}
