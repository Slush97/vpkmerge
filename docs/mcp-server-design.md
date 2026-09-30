# Design: `vpkmerge-mcp` (local MCP server)

**Status:** proposal / not yet built
**Date:** 2026-06-26
**Owner:** slush97

A local [Model Context Protocol](https://modelcontextprotocol.io) server that exposes
`vpkmerge-core`'s asset-forge engine as a toolbox an LLM client (Claude Desktop, Claude
Code, any MCP host) can drive in natural language: "make this sound louder", "give Seven a
vine-boom melee", "make a custom card for Vindicta", "recolor Paige's ult purple".

The engine already does the hard work. This server is the **natural-language front door**
to it: a thin Rust crate that maps a small set of intent-shaped tools onto existing core
functions, plus a catalog/discovery layer so the model can find what's swappable before it
swaps.

---

## 1. Goals / non-goals

**Goals**
- Let an AI client compose real Deadlock mods from plain English, against the user's own
  local game install, and hand the result to Grimoire to install.
- Reuse `vpkmerge-core` directly (in-process, pure Rust, no subprocess, no .NET).
- A tool surface a model uses *correctly* without hand-holding: few tools, intent-shaped,
  well described, discovery-first.

**Non-goals (v1)**
- No remote / hosted transport. **Local stdio only.** (The hosted-website question is a
  separate effort; see the dominant constraint below for why it can't be a naive cloud
  service.)
- No new modding capability. If the CLI can't do it, neither can the MCP server.
- No auto-install without confirmation. Output lands in a staging dir; installing is a
  separate, gated step.

---

## 2. The dominant constraint (recap)

Almost every operation reads the user's **local `citadel/pak01`** (the installed Deadlock
files): donor clips, the catalog of swappable events, base textures, icon templates. The
tool is a function of *the user's game install*.

Consequences that shape this design:
- The server must run **locally**, where the game files are. Stdio transport, local client.
- We never ship or proxy Valve assets. The server reads the user's install; it does not
  redistribute anything from it.
- A future website cannot do the real work in the cloud (it would have to host ~50 GB of
  copyrighted game assets). The web path is "web UI orchestrates a local executor" — and
  **this MCP server is exactly that local executor.** Building it now is a no-regret step
  toward the website later.

---

## 3. Architecture

```
                 ┌─────────────────────────┐
  Claude Desktop │  MCP host (LLM client)  │
  / Claude Code  └───────────┬─────────────┘
                             │ stdio (JSON-RPC, MCP)
                 ┌───────────▼─────────────┐
                 │     vpkmerge-mcp        │   new workspace crate
                 │  - tool dispatch        │
                 │  - JSON schemas         │
                 │  - catalog cache        │
                 └───────────┬─────────────┘
                             │ in-process Rust calls
                 ┌───────────▼─────────────┐
                 │     vpkmerge-core       │   unchanged engine
                 └───────────┬─────────────┘
                             │ reads
              user's Deadlock install (citadel/pak01)  →  staging dir (addon VPKs)
                             │                                      │
                             └──────────────► Grimoire (managed install) ◄──┘
```

- **New crate:** `vpkmerge-mcp` (workspace member alongside `vpkmerge-cli`). Binary target
  `vpkmerge-mcp` that speaks MCP over stdio.
- **SDK:** the official Rust MCP SDK [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk).
  Tools are async handlers returning structured content.
- **Transport:** stdio in v1. (The same crate can later add the streamable-HTTP transport
  for the website's local-agent mode without touching tool logic.)
- **No new engine code** for the happy path — but see the prerequisite refactor.

### 3.1 Prerequisite refactor: push orchestration down into core

Today `vpkmerge-cli/src/main.rs` is a ~4k-line `clap` app that holds real orchestration,
not just argument parsing. Example: clip-mode soundswap is "read the donor clip from the
VPK → `mint_swapped_clip` → `pack` at the entry path", and that sequence lives in
`main.rs`, not in core. If the MCP server re-implements it, the two drift.

**Recommendation:** extract the per-command orchestration into `vpkmerge-core` as
higher-level one-call functions, so CLI and MCP are both thin callers. Concretely, add
functions like:

| New core fn (proposed) | Wraps today's CLI logic for |
|---|---|
| `soundswap::swap_clip_to_addon(vpk, clip_entry, mp3, opts) -> AddonBuild` | `soundswap --clip` |
| `soundswap::swap_event_to_addon(vpk, sndevts, event, mp3, opts) -> EventSwap` | `soundswap --event` (mostly exists: `swap_event_audio`) |
| `icon::build_icon_addon(template_vpk, &[(entry, png)]) -> AddonBuild` | `icon --set` |
| `mp3::louder(mp3, gain_db) -> Vec<u8>` (thin alias over `apply_mp3_gain`) | `--gain-db` |

`AddonBuild` is a small struct: `{ vpk_path, entries_written, notes, skipped }`. Both the
CLI and the MCP server return/print it. This is the single most important best-practice
move: **one orchestration layer, two front ends.** It's also a good cleanup for the CLI
regardless of MCP.

Where a one-call function already exists (`merge`, `detect_conflicts`, `inspect`, `pack`,
`recolor_hero_to_addon`, `prism_recolor_hero_to_addon`, `swap_event_audio`,
`build_*_index`, `build_hero_roster`), the MCP tool calls it directly.

---

## 4. Tool surface

Design rules:
- **Intent-shaped, not flag-shaped.** ~10 tools, each a task a user would name, not a
  mirror of every CLI switch. The model reasons about *which tool*; parameters are minimal.
- **Discovery first.** The model can't swap `Seven.Melee.Swing` if it doesn't know the
  event exists. Catalog/search tools come before mutation tools in priority and in the
  system prompt's suggested workflow.
- **Codenames are an internal detail.** Users say "Vindicta"; tools accept display names
  and resolve to codenames (`hornet`) via `build_hero_roster`. Never make the model guess
  `vampirebat` for Mina.
- **Every mutation returns a staged path + a human summary**, never a silent file drop.

### 4.1 Discovery tools

#### `browse_sounds`
Search the sound/voice-event catalog so the model can find a swap target.
```jsonc
// params
{
  "hero":   "string?  // display name or codename; omit for all",
  "search": "string?  // prose match against the spelled-out event label",
  "kind":   "voiceline | gameplay | any   // default any",
  "limit":  "integer? // default 30, truncation is reported not silent"
}
// returns: [{ event, hero, label, clipCount, durationSeconds, isPool }]
```
Backed by `build_voiceline_index` / `build_hero_sound_index`, served through
`CatalogCache` (warm load ~0.25 s). `event` is the verbatim swap target; `isPool` tells the
model a pool swap (`swap_hero_sound`) is needed rather than a single clip.

#### `browse_textures`
Visual counterpart: find an icon / card / hero-image / ability-icon to replace or recolor.
```jsonc
{
  "category": "ability-icon | item-icon | hero-image | hero-model | ability-vfx | other?",
  "hero":     "string?",
  "search":   "string?",
  "limit":    "integer?",
  "thumbnails": "boolean?  // if true, also write PNG thumbs to staging and return paths"
}
// returns: [{ path, category, hero, label, thumbnailPath? }]
```
Backed by `build_texture_index` (+ `thumbnail_png` / `cache_texture_thumbnails` when
`thumbnails:true`). `path` is the verbatim icon/recolor target entry.

#### `list_heroes`
Display-name ⇄ codename roster, so the model can translate freely.
```jsonc
{ "all": "boolean?  // include in-dev/disabled; default selectable-only" }
// returns: [{ codename, name, selectable, inDevelopment, disabled }]
```
Backed by `build_hero_roster`.

### 4.2 Sound tools

#### `make_sound_louder`
The headline use case, kept first-class.
```jsonc
{
  "audioPath": "string  // user's .mp3 on disk",
  "gainDb":    "number  // e.g. 6.0; positive = louder",
  "outPath":   "string? // where to write the result; default alongside input"
}
// returns: { outPath, gainDb, framesEdited }
```
Backed by `apply_mp3_gain` (lossless mp3gain `global_gain` shift, no transcode).
**Honest limitation:** v1 applies an explicit dB. "Make it as loud as the original sound"
(auto-match) needs an RMS measurement, which needs an MP3 *decoder* in core — currently
deferred (the GUI measures in Web Audio instead). See §9. The tool description tells the
model to ask for a target dB or pick a sensible default (e.g. +6) rather than promise
matching.

#### `swap_hero_sound`
Replace a hero sound **event** (the common case: most gameplay events are randomizer pools
of 10–35 clips, so swapping one clip only replaces 1 of N).
```jsonc
{
  "hero":      "string  // display name or codename",
  "event":     "string  // verbatim event from browse_sounds",
  "audioPath": "string  // user .mp3",
  "pool":      "all | collapse   // default all: mint into every clip so it always plays",
  "heroOnly":  "boolean? // collapse + new clip path: affect only this hero for a shared sound",
  "loop":      "auto | on | off  // default auto (inherit donor)",
  "trimStartMs": "integer?",
  "trimEndMs":   "integer?",
  "gainDb":      "number?"
}
// returns: { stagedVpk, event, clipsMinted, poolPolicy, skipped: [...] }
```
Backed by `swap_event_audio` (+ a `trim_mp3` / `apply_mp3_gain` pre-pass when those params
are set). `--soundevents <entry>` is resolved from `hero` for gameplay; a `soundeventsEntry`
override param covers VO/other trees. `skipped` surfaces pool clips that live in another pak
(never a hard failure).

#### `swap_sound_clip`
Single-clip swap (when the model knows it wants exactly one clip, or for non-pool sounds).
```jsonc
{
  "clipEntry": "string  // .vsnd_c entry from browse_sounds / catalog",
  "audioPath": "string",
  "loop": "auto | on | off",
  "trimStartMs": "integer?", "trimEndMs": "integer?", "gainDb": "number?"
}
// returns: { stagedVpk, clipEntry }
```
Backed by the proposed `soundswap::swap_clip_to_addon` (donor read → `mint_swapped_clip`
→ `pack`).

### 4.3 Image tools

#### `create_icon_mod`
Replace one or more base textures (hero cards, ability icons, item icons) with user PNGs.
```jsonc
{
  "replacements": [
    { "entry": "string  // target .vtex_c from browse_textures", "pngPath": "string" }
  ]
}
// returns: { stagedVpk, written: [{ entry, srcWidth, srcHeight }] }
```
Backed by `build_icon_from_template` (resizes the PNG to the template's own dimensions and
re-encodes in its format) + `pack`. Multiple replacements → one addon VPK.

### 4.4 VFX tools

#### `recolor_hero_vfx`
Recolor a hero's whole ability-VFX set (particles + color textures + baked vertex colors)
to one hue, or spread it into a rainbow.
```jsonc
{
  "hero": "string",
  "mode": "solid | prism   // default solid",
  "hue":  "number?  // 0-360, required for solid",
  "saturation": "number?", "brightness": "number?",
  "animated": "boolean?  // prism only: byte-faithful timing pass",
  "previewOnly": "boolean? // render a swatch PNG instead of baking the addon"
}
// returns: { stagedVpk } | { previewPng }
```
Backed by `recolor_hero_to_addon` / `prism_recolor_hero_to_addon[_tuned]`; preview via
`recolor_hero_preview_png`. An unknown hero returns the pinned-codename list
(`pinned_hero_codenames`) so the model can recover. `rainbow-scan`
(`scan_hero_rainbow_support`) can back an optional `assess_rainbow` tool, but is probably
better folded into `recolor_hero_vfx`'s error/guidance text.

### 4.5 Merge / utility

#### `merge_mods`
```jsonc
{ "inputs": ["string"], "outPath": "string?", "policy": "last-wins | first-wins | strict" }
// returns: { outPath, conflictsResolved }
```
Backed by `merge` (+ `detect_conflicts` for the dry-run preview below).

#### `inspect_mod`
Read-only: list a VPK's contents / preview collisions before merging.
```jsonc
{ "vpk": "string", "against": ["string"]?  // if set, report cross-input collisions }
// returns: { entryCount, entries?: [...], conflicts?: [...] }
```
Backed by `inspect` / `detect_conflicts`.

### 4.6 Install (gated)

#### `install_mod`
Register a staged addon VPK into Grimoire as a managed local mod.
```jsonc
{
  "stagedVpk": "string",
  "name": "string", "hero": "string?", "category": "string?", "imagePath": "string?"
}
// returns: { installedAs, grimoireModId }
```
Backed by the Grimoire local-mod importer (`scripts/import-local-mod.mjs` path), invoked as
the managed install (not a raw `cp`, which Grimoire prunes). **This tool is the one
explicit side-effect on the user's game/manager state** and must be confirmation-gated
(§7). For v1 it's acceptable to omit `install_mod` entirely and just hand back the staged
path with instructions; add it once the flow is trusted.

### 4.7 Tool count summary

10 tools: `browse_sounds`, `browse_textures`, `list_heroes`, `make_sound_louder`,
`swap_hero_sound`, `swap_sound_clip`, `create_icon_mod`, `recolor_hero_vfx`, `merge_mods`,
`inspect_mod` (+ optional `install_mod`). That's a surface a model holds in working memory
and routes correctly.

---

## 5. Resources (optional, phase 2)

MCP *resources* are read-only context the host can pull on demand. Two fit well:
- `catalog://sounds` and `catalog://textures` — the cached indexes as browsable resources,
  so a host can let the *user* scroll them, not only the model.
- `mod://staging` — list of addon VPKs built this session, with their summaries.

Resources are a nice-to-have; the `browse_*` tools already cover the model's needs. Ship
tools first.

---

## 6. Configuration & game discovery

The server needs to find the Deadlock install and a staging dir. Precedence:
1. Explicit config in the MCP host's server entry (args / env), e.g.
   `DEADLOCK_GAME_DIR`, `VPKMERGE_STAGING_DIR`.
2. Auto-discovery: walk Steam library folders for `Deadlock/game/citadel/pak01_dir.vpk`
   (same probe the rest of the toolchain already does for `DEADLOCK_PAK`).
3. If neither resolves, every tool that needs the pak returns a clear, actionable error
   ("set DEADLOCK_GAME_DIR to your Deadlock install") rather than failing deep in a decode.

Example Claude Desktop config:
```jsonc
{
  "mcpServers": {
    "vpkmerge": {
      "command": "/path/to/vpkmerge-mcp",
      "env": {
        "DEADLOCK_GAME_DIR": "/home/me/.steam/steam/steamapps/common/Deadlock",
        "VPKMERGE_STAGING_DIR": "/home/me/.local/share/grimoire/staging"
      }
    }
  }
}
```

Catalog cache lives under the staging/config dir and is fingerprinted by the pak's
length+mtime (`catalog_cache`), so `browse_*` is instant after the first warm-up and
auto-rebuilds after a game update.

---

## 7. Output & install model (safety)

- **Pure functions write to staging, not the game.** Every mutation tool writes an addon
  VPK under `VPKMERGE_STAGING_DIR` and returns the path. Nothing touches
  `citadel/addons/` directly.
- **Read tools are free** (`browse_*`, `list_heroes`, `inspect_mod`, previews). They never
  mutate and can run without prompting.
- **`install_mod` is the only state-changing-on-the-user side effect** and is gated: the
  MCP host surfaces it for confirmation, and the tool description says so explicitly. We do
  not auto-install. This matches the workspace rule that outward/hard-to-reverse actions
  get confirmed first.
- **No telemetry.** Consistent with the workspace hard rule. The server phones home for
  nothing.

---

## 8. Model-friendliness (the part that actually decides success)

The code is easy; getting the model to use it well is the work.

- **Descriptions over parameters.** Each tool's description states when to use it, what it
  needs, and what it returns, with a one-line example. `swap_hero_sound`'s description must
  say "most gameplay events are randomizer pools; use `pool: all` so your sound always
  plays."
- **Suggested workflow in the server instructions** (MCP servers can ship an `instructions`
  block): *discover → build to staging → preview/summarize → (gated) install*. Tell the
  model to call `browse_sounds`/`list_heroes` before a swap rather than guessing entries.
- **Errors are recoverable and instructive.** Unknown hero → return the pinned list.
  Missing pak → return the config fix. Event not found → suggest `browse_sounds`. The model
  should be able to self-correct from the error text.
- **Summaries, not blobs.** Tools return small JSON (paths, counts, skipped lists), never
  raw VPK bytes or 76k-row catalogs. `limit` + a reported truncation note keeps payloads
  model-sized.

---

## 9. The loudness gap (be honest about it)

"Make sounds louder" splits into two asks:
- **"+N dB"** — fully supported today via `apply_mp3_gain` (lossless, byte-exact
  round-trips verified). `make_sound_louder` ships on day one.
- **"as loud as the sound it replaces" (auto-match)** — needs to *measure* loudness, which
  needs an MP3 decoder in core. Currently deferred; the GUI does RMS measurement in Web
  Audio and passes a `gainDb` down. A headless MCP server has no Web Audio.

v1 stance: `make_sound_louder` and the `gainDb` params take an explicit dB; the model is
told to pick a default (≈ +6) or ask. Auto-match is a **fast follow** once a core decoder
(e.g. `minimp3`/`symphonia` behind a feature flag) lands `mp3::measure_loudness`. Flag this
in the doc so it isn't a surprise.

---

## 10. Implementation milestones

| # | Milestone | Scope |
|---|---|---|
| M0 | Shared-orchestration refactor | Add `AddonBuild` + the `*_to_addon` core fns (§3.1). CLI switches to them; behavior unchanged. Independent value. |
| M1 | Crate + transport skeleton | `vpkmerge-mcp` crate, `rmcp` stdio server, config/game discovery, one tool end-to-end: `make_sound_louder`. Wire into Claude Desktop, confirm a real round-trip. |
| M2 | Discovery tools | `browse_sounds`, `browse_textures`, `list_heroes` on `CatalogCache`. |
| M3 | Core mutations | `swap_hero_sound`, `swap_sound_clip`, `create_icon_mod`, `recolor_hero_vfx`, `merge_mods`, `inspect_mod`. All write to staging. |
| M4 | Install + polish | gated `install_mod` (Grimoire importer), server `instructions`, error-recovery text, resources (optional). |
| M5 | (later) Auto-loudness | core MP3 decoder → `mp3::measure_loudness` → `make_sound_louder` match mode. Unblocks the website's headless normalize too. |

M1 is the proof point: a user types "make `kick.mp3` 6 dB louder and save it" into Claude
Desktop and gets a louder MP3 back. From there each milestone adds tools without changing
the shape.

---

## 11. Open questions (for slush97)

1. **Crate home now or fold into Grimoire later?** Standalone `vpkmerge-mcp` binary ships
   independently and is testable today; the eventual Grimoire desktop client could embed
   the same server. Recommend standalone crate now, embed later — no rework, since tools
   call core either way.
2. **Ship `install_mod` in v1, or hand back the staged path and let the user install via
   Grimoire's UI?** Safer to defer the auto-install tool to M4 once the build flow is
   trusted.
3. **How far to push the M0 refactor?** Minimum is the four `*_to_addon` fns the MCP tools
   need; maximum is moving *all* CLI orchestration into core. Recommend minimum-for-MCP now,
   opportunistic cleanup later.
4. **CI:** the workspace lints `--lib --bins --tests` and skips examples. The MCP crate is a
   bin + lib, so it's in scope for `fmt`/`clippy`/`test` — budget for clean pedantic-clippy
   from the start.

---

## 12. Why this is the right first step toward the website

The hosted-website idea can't be a naive cloud service (it would have to host Valve's game
files). The viable web architecture is "web UI orchestrates a local executor that has the
game install." **This MCP server *is* that local executor.** Build it for Claude Desktop
first; later, the same crate's HTTP transport + a thin web front end (on the existing
Cloudflare Workers stack for auth/orchestration) becomes the website — with zero change to
the tool logic or the engine.
