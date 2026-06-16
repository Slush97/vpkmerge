# grimhud (working name)

A Grimoire-native Deadlock HUD/QOL mod: the original successor to the community
**QOL Lock** mod. Full teardown of QOL Lock and the strategy behind this project:
[`../docs/qol-lock-teardown.md`](../docs/qol-lock-teardown.md).

## Status: build path PROVEN (2026-06-15)

The gating unknown is resolved. We **can** mint engine-loadable Panorama resources
(`.vjs_c` / `.vcss_c` / `.vxml_c`) on Linux, with no Windows box and no hand-written
resource writer, by driving Valve's `resourcecompiler.exe` through the existing
Proton/CSDK harness. The first spike authored three source files, compiled them, and
packed a valid addon VPK with our JS source round-tripped into the compiled `.vjs_c`.

This means the planned architecture is viable: a reproducible Rust+Proton build that
**bakes config in at install time** (killing QOL Lock's hero-hijack storage hack) and a
**Grimoire desktop settings UI** as the source of truth, with in-game live-preview +
token-export as a later additive layer.

## Layout

```
grimhud/
  src/panorama/
    layout/    .vxml   panel layouts (and hud.vxml override for injection)
    scripts/   .vjs    runtime
    styles/    .vcss   styles
  README.md
```

The current `src/` is the **probe**: a standalone `grimhud_probe.{vxml,vjs,vcss}` that
drops a "GRIMHUD LOADED" marker. It proves the pipeline; it is not yet wired into the
game HUD.

## Build

```sh
# from repo root
python3 tools/panorama-compiler/build_panorama_addon.py grimhud/src \
  --addon grimhud_probe --output target/grimhud_probe_dir.vpk --force
```

Prereqs (same as the soul-container compiler): CSDK 12 at `$CSDK_ROOT`
(`/home/esoc/csdk12/Reduced_CSDK_12`), Proton Experimental, Steam root. The wrapper
stages `src/panorama/**` under the CSDK content addon, compiles every
`.vxml`/`.vjs`/`.vcss`/`.vsvg` via `resourcecompiler.exe`, and packs the compiled
`game/citadel_addons/<addon>` tree into a dir VPK via the `pack_tree` example.

### Two hard-won build notes
- resourcecompiler must be invoked by its **absolute path** through `proton run`; the
  bare name fails silently with "Failed to create process: 2".
- CSDK 12 binaries carry a **schema-version skew** vs current content
  (`ParticleFloatType_t` member-count mismatch). resourcecompiler aborts on it unless
  `-danger_mode_ignore_schema_mismatches` is passed. The wrapper always passes it. (Same
  skew family as the CSDK map-hosting crash; harmless for compiling UqI resources.)

## HUD injection (BUILT 2026-06-15, in-game visual confirm PENDING)

The mod injects by overriding `panorama/layout/hud.vxml`. An addon's `hud.vxml_c`
*replaces* the game's entirely, so the override must faithfully reproduce the current
retail HUD and only add our includes. The repeatable transform:

```sh
# 1. Decompile the CURRENT retail hud.vxml to source XML (re-derive every patch).
tools/morphic-oracle  panorama decompile \
  --vpk "$DEADLOCK/game/citadel/pak01_dir.vpk" \
  --entry panorama/layout/hud.vxml_c --out /tmp/hud_retail.vxml
# 2. Inject our <styles>/<scripts> includes.
python3 tools/panorama-compiler/inject_hud_scripts.py /tmp/hud_retail.vxml \
  grimhud/src/panorama/layout/hud.vxml \
  --style panorama/styles/grimhud_probe.vcss_c \
  --script panorama/scripts/grimhud_probe.vjs_c
# 3. Compile + pack.
python3 tools/panorama-compiler/build_panorama_addon.py grimhud/src \
  --addon grimhud --output target/grimhud_dir.vpk --force
```

Verified: the compiled `hud.vxml_c` round-trips (decompile of our compiled output is
structurally **identical** to the injected source, 0 drift) and our two includes survive.
The `morphic-oracle panorama decompile` command (VRF `FileExtract`) reconstructs `.vxml_c`
XML from the compiled AST and recovers `.vjs_c`/`.vcss_c` source text. It is also the
basis of milestone 2 (patch-day selector check) and proved its worth immediately: the
retail HUD already differs from the CSDK copy (`CitadelHudJoinTeam`, radial angles,
`CitadelHudGameAnnouncements`), so overriding from the CSDK copy would have reverted live
changes.

### In-game test (your step)
Installed at `$DEADLOCK/game/citadel/addons/pak13_dir.vpk`. Launch Deadlock, enter any
match/sandbox; a green **"GRIMHUD LOADED ✓"** box should appear top-left of the HUD. To
disable: delete that file or move it into `addons/.disabled/`.

## Next milestones

1. ~~In-game load test~~ (built; awaiting your visual confirm).
2. **HUD-adapter + selector manifest**: one registry of every panel ID / class the mod
   depends on, with a `vpkmerge`/`oracle`-driven patch-day check against the live `pak01`
   (decompile -> resolve every selector -> report what broke).
3. **Declarative schema -> config bake**: a single schema table that generates the
   defaults, the Grimoire settings UI, and the baked `grimhud_config.vjs`.
4. **First real feature**: souls-counter reposition, end-to-end on the new foundation.
