import type { Theme } from "./api";

export const DEFAULT_ACCENT = "#0a84ff";

export const ACCENTS: { name: string; value: string }[] = [
  { name: "Blue", value: "#0a84ff" },
  { name: "Orange", value: "#f97316" },
  { name: "Purple", value: "#8b5cf6" },
  { name: "Green", value: "#22c55e" },
  { name: "Pink", value: "#ec4899" },
  { name: "Graphite", value: "#6b7280" },
];

type Rgb = [number, number, number];

const BG: Record<"light" | "dark", Rgb> = { light: [255, 255, 255], dark: [25, 25, 27] };
const NEAR_BLACK: Rgb = [17, 17, 19];
const WHITE: Rgb = [255, 255, 255];

function parse(hex: string): Rgb | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return null;
  const n = Number.parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function toHex([r, g, b]: Rgb): string {
  return `#${[r, g, b].map((v) => Math.round(v).toString(16).padStart(2, "0")).join("")}`;
}

function channel(v: number): number {
  const c = v / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function luminance([r, g, b]: Rgb): number {
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

function mix(a: Rgb, b: Rgb, t: number): Rgb {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
}

/**
 * Filled buttons keep white text when a slight darkening of the accent is
 * enough for 4.5:1; vivid light accents (orange, green) get near-black text.
 */
function fillFor(accent: Rgb): { fill: Rgb; on: Rgb } {
  for (let t = 0; t <= 0.2001; t += 0.02) {
    const fill = mix(accent, [0, 0, 0], t);
    if (contrast(fill, WHITE) >= 4.5) return { fill, on: WHITE };
  }
  return { fill: accent, on: NEAR_BLACK };
}

/** Accent used as text (links, selected labels) readable on the page background. */
function inkFor(accent: Rgb, mode: "light" | "dark"): Rgb {
  const target = mode === "light" ? [0, 0, 0] : [255, 255, 255];
  for (let t = 0; t <= 1.0001; t += 0.05) {
    const ink = mix(accent, target as Rgb, t);
    if (contrast(ink, BG[mode]) >= 4.5) return ink;
  }
  return target as Rgb;
}

export function resolveMode(theme: Theme): "light" | "dark" {
  if (theme === "light" || theme === "dark") return theme;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function applyTheme(theme: Theme, accentHex: string) {
  const root = document.documentElement;
  const mode = resolveMode(theme);
  const accent = parse(accentHex) ?? parse(DEFAULT_ACCENT)!;
  const { fill, on } = fillFor(accent);
  root.dataset.theme = mode;
  root.style.colorScheme = mode;
  root.style.setProperty("--accent", toHex(accent));
  root.style.setProperty("--accent-fill", toHex(fill));
  root.style.setProperty("--on-accent", toHex(on));
  root.style.setProperty("--accent-ink", toHex(inkFor(accent, mode)));
}

/** Keeps `system` in sync with the OS setting. Returns an unsubscribe. */
export function watchSystemTheme(onChange: () => void): () => void {
  const query = window.matchMedia("(prefers-color-scheme: dark)");
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}
