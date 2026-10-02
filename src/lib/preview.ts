import type { Part } from "./api";

/** A 3D model a tool built for the user to look at (vpkmerge's preview_hero). */
export interface ModelPreview {
  /** Absolute path of the .glb. */
  glb: string;
  /** Hero display name, which is also how the animation tools find the hero. */
  hero: string;
  /** Path of the mod shown on the hero, or null for the base game. */
  vpk: string | null;
  /** Animation the model opens in (its menu pose), or null for a static mesh. */
  pose: string | null;
}

/** Recognized by the result's shape, so it works whichever model or agent ran the tool. */
export function previewFrom(part: Part): ModelPreview | null {
  if (part.type !== "toolResult" || part.isError || !part.output.includes('"previewGlb"')) return null;
  let value: unknown;
  try {
    value = JSON.parse(part.output);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const { previewGlb, hero, vpk, pose } = value as Record<string, unknown>;
  if (typeof previewGlb !== "string" || !previewGlb.endsWith(".glb")) return null;
  return {
    glb: previewGlb,
    hero: typeof hero === "string" ? hero : "Model",
    vpk: typeof vpk === "string" ? vpk : null,
    pose: typeof pose === "string" ? pose : null,
  };
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** `vampirebat_ability_ult_start` reads as "ability ult start" for Mina. */
export function animationLabel(name: string, prefix: string): string {
  const bare = name.replace(/\.vnmclip\+non_additive$/, "");
  const unprefixed = prefix && bare.startsWith(prefix) ? bare.slice(prefix.length) : bare;
  return unprefixed.replace(/_+/g, " ").trim();
}
