import type { Session } from "./api";

export interface SessionGroup {
  label: string;
  items: Session[];
}

export function groupSessions(sessions: Session[], now = Date.now()): SessionGroup[] {
  const startOfToday = new Date(now);
  startOfToday.setHours(0, 0, 0, 0);
  const today = startOfToday.getTime();
  const weekAgo = today - 6 * 86_400_000;
  const groups: SessionGroup[] = [
    { label: "Today", items: [] },
    { label: "This week", items: [] },
    { label: "Older", items: [] },
  ];
  for (const s of sessions) {
    const index = s.updatedAt >= today ? 0 : s.updatedAt >= weekAgo ? 1 : 2;
    groups[index].items.push(s);
  }
  return groups.filter((g) => g.items.length > 0);
}

export function prettyToolName(name: string): string {
  if (name.startsWith("mcp__")) {
    const rest = name.slice(5);
    const split = rest.indexOf("__");
    if (split > 0) return `${rest.slice(0, split)}.${rest.slice(split + 2)}`;
  }
  return name;
}

function parse(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

export function prettyJson(text: string): string {
  const value = parse(text);
  return value === undefined ? text : JSON.stringify(value, null, 2);
}

export function argsSummary(text: string): string {
  const value = parse(text);
  let out = text;
  if (value && typeof value === "object" && !Array.isArray(value)) {
    out = Object.entries(value as Record<string, unknown>)
      .slice(0, 3)
      .map(([k, v]) => `${k}: ${typeof v === "string" ? v : JSON.stringify(v)}`)
      .join(", ");
  }
  return out.length > 140 ? `${out.slice(0, 140)}...` : out;
}
