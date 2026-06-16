# grimhud HANDOFF

Continuation doc for the grimhud mod (original successor to community mod **QOL Lock**).
Read this first; it is self-contained. Companions: `README.md` (this dir), strategy in
`../docs/qol-lock-teardown.md`, QOL Lock teardown in the same doc.

Working dir for all commands: `/home/esoc/grimoire-workspace/vpkmerge`.

## TL;DR status (2026-06-15)

The entire build->in-game pipeline is **proven in reality**. **4 features confirmed in-game**
(souls / top-bar / ability-bar reposition, low-HP warning). A large batch is **built into
pak13, in-game confirm PENDING**:

- **Reposition catalog** (generic `transformFeature`): minimap, items bar, quickbuy, hero
  shop (neutral), passives, signature bar, AP currency, damage indicators, crosshair (neutral).
- **Enemy/ally health color-warnings** (`topbarHpFeature`): walks every `HeroContents` slot
  under `TeamFriendly`/`TeamEnemy`, reads each `HeroHealth` ProgressBar's `.value` (TRUE
  fraction, no scraping), washes the bar amber (ally) / green (enemy) at/under a threshold.
- **Objective timer overlay**: a countdown card (`GrimhudTimers` panel + `.grimhud-timer-*`
  CSS). Reads the match clock by scraping the `GameTime` label (`{s:game_clock}` -> "MM:SS")
  and computes each objective's next spawn from `OBJECTIVE_CADENCE` in core. **The cadence
  numbers are UNVERIFIED placeholders** (`OBJECTIVE_CADENCE_VERSION`) - the table is isolated
  + version-stamped; tune spawn times in-game. This is the known "timing tables rot" weakness,
  contained. **CSS was rewritten conservatively** after the first build fatally broke hud load
  in retail (see the CSDK-vs-retail CSS footgun under Footguns); polish (gradient / box-shadow
  / pulse animation) is deferred until re-added one prop at a time behind a load test.

Config is hand-authored test values; everything loads, reads config, resolves selectors, and
mutates the live HUD. The on-screen dashboard marker (top-left) is **data-driven over every
`HUD_IDS` selector**: it prints `N/M` plus a per-selector hit/miss (`map✓ ally✓ clock✗ ...`)
+ live `HP NN%`, red at low HP. That marker is the patch-day self-test, surfaced on screen.

Every new selector was verified present in the current retail layout decompiles (`hud.vxml`
+ the `citadel_hud_top_bar*` / `hud_ability_resource` sub-layouts) before adding, and verified
round-tripped back out of the packed pak13 VPK after building.

### Open items from this batch (verify/iterate in-game)
- `HeroHealth.value` readability: assumed the ProgressBar exposes a numeric `.value` (0..1).
  If the ally/enemy wash never fires in-match, that read is the suspect - fall back to
  reading the bound attribute or the bar geometry.
- `OBJECTIVE_CADENCE` spawn times are guesses; correct them against a real match clock.
- `teamFriendly`/`teamEnemy`/`gameClock` only resolve **in-match** (top bar), so they show
  `✗` on the marker in the hideout - expected, not a miss.

What is NOT done yet: the config keystone (declarative schema + Grimoire settings UI +
bake-at-install), the patch-day selector harness, and the bulk of QOL Lock's ~40 features.

## The build pipeline (all proven)

Three tools, all in `tools/`:
- `morphic-oracle panorama decompile` (C#/VRF) - reconstructs `.vxml_c` XML from the
  compiled AST and recovers `.vjs_c`/`.vcss_c` source. Build it once:
  `cd tools/morphic-oracle && DOTNET_ROOT=$HOME/.dotnet PATH=$HOME/.dotnet:$PATH dotnet build -c Release`.
  Run: `dotnet run -c Release --no-build -- panorama decompile (--vpk PAK --entry PATH | --file FILE) [--out FILE]`.
- `tools/panorama-compiler/inject_hud_scripts.py` - injects our `<styles>`/`<scripts>`
  includes into a decompiled `hud.vxml`.
- `tools/panorama-compiler/build_panorama_addon.py` - stages `src/panorama/**` under the
  CSDK content addon, compiles every `.vxml/.vjs/.vcss` via `resourcecompiler.exe` (Proton),
  packs the compiled tree into a dir VPK via the `pack_tree` example.

### Rebuild + reinstall (the loop you'll run constantly)

```sh
# Only when you ADD/REMOVE a script or style file do you need to re-derive hud.vxml:
DEADLOCK=/home/esoc/.local/share/Steam/steamapps/common/Deadlock
( cd tools/morphic-oracle && DOTNET_ROOT=$HOME/.dotnet PATH=$HOME/.dotnet:$PATH \
  dotnet run -c Release --no-build -- panorama decompile \
  --vpk "$DEADLOCK/game/citadel/pak01_dir.vpk" \
  --entry panorama/layout/hud.vxml_c --out /tmp/hud_retail.vxml )
python3 tools/panorama-compiler/inject_hud_scripts.py /tmp/hud_retail.vxml grimhud/src/panorama/layout/hud.vxml \
  --style panorama/styles/grimhud.vcss_c \
  --script panorama/scripts/grimhud_config.vjs_c \
  --script panorama/scripts/grimhud_core.vjs_c
# (add more --script/--style as files are added)

# Always: compile + pack + install to the pak13 test slot.
python3 tools/panorama-compiler/build_panorama_addon.py grimhud/src \
  --addon grimhud --output target/grimhud_dir.vpk --force
cp target/grimhud_dir.vpk "$DEADLOCK/game/citadel/addons/pak13_dir.vpk"
```

Then the USER launches Deadlock and reports (retail has no console; on-screen marker is the
only feedback channel). Disable by moving `pak13_dir.vpk` into `addons/.disabled/`.

## Repo layout & architecture

```
grimhud/src/panorama/
  layout/hud.vxml          generated: retail hud + our <styles>/<scripts> includes (DO NOT hand-edit; regenerate)
  scripts/grimhud_config.vjs  baked config; runtime READS only. Later: generated by Grimoire.
  scripts/grimhud_core.vjs    runtime: HUD_IDS registry + resolve() + feature registry + tick loop + marker
  styles/grimhud.vcss         dashboard marker styles (temporary dev overlay)
```

Three patterns in `grimhud_core.vjs`:
1. **`HUD_IDS`** - logical name -> ordered candidate panel ids. ALL selectors live here.
   `resolve(root, name)` returns first valid hit, records `_status[name]`, logs a one-time
   MISS. This is the patch-day self-test seed: the harness (milestone) just reads `HUD_IDS`.
2. **`transformFeature(selector, {enabled,x,y,scale,opacity})`** - the generic "move/scale/
   fade a panel" feature. Most QOL reposition features are one-liners with this.
3. **feature registry** `FEATURES[]` - `tick()` (0.5s) runs each, each with a style-signature
   skip so an unchanged HUD costs ~nothing.

## How to add a feature (the common case)

1. Find the panel id (see "finding selectors"). Add it to `HUD_IDS`.
2. If it's reposition/scale/fade: `FEATURES.push(transformFeature("name", {enabled:"X_ENABLED", x:"X_OFF", ...}))`.
   If it reads a value or reacts: write a `function(root){ ... resolve(root,"name") ... }` and push it.
3. Add the config keys + visible TEST values to `grimhud_config.vjs`.
4. Rebuild + reinstall (re-inject only if you added a file). User confirms in-game.

## Finding selectors (do this, don't guess)

Decompile the relevant retail layout and read the real ids/classes/bindings:
```sh
( cd tools/morphic-oracle && DOTNET_ROOT=$HOME/.dotnet PATH=$HOME/.dotnet:$PATH \
  dotnet run -c Release --no-build -- panorama decompile \
  --vpk "$DEADLOCK/game/citadel/pak01_dir.vpk" --entry panorama/layout/<NAME>.vxml_c )
```
**Prefer data bindings over geometry/format-scraping.** Health was a big win: labels carry
`text="{i:health}"` / `"{i:maxHealth}"`, so we read the TRUE number for exact HP% instead of
QOL Lock's `barHeight/60` hack. Look for `{i:...}`/`{f:...}`/`{s:...}` bindings in the
decompiled layout - those are accurate, locale-safe value sources.

Selectors already known from the hud.vxml decompile (logical -> id): souls=`gold_and_ap_container`,
topBar=`TopBar`, abilities=`AbilitiesContainer`, health=`current_health`/`max_health`,
minimap=`minimap_persp`/`minimap_container`/`HudMinimapContainer`/`hud_minimap`/`minimap_frame`,
crosshair=`crosshair`, passives=`hud_passive_items`, signature=`hud_signature`,
items=`ActiveAbilitiesMenu`, AP currency=`APContainer`, quickbuy=`CitadelHudQuickbuy`,
heroShop=`CitadelHudHeroShop`, chat=`HudChat`/`Chat`, damageIndicators=`damage_indicator_canvas`,
modsBarGraph=`ModBarGraph`, statsAndMods=`StatsAndModsContainer`, topbar player slots=`TopBarPlayer<i>`.

## Known issues

- **Blue hue on top bar + ability bar in the hideout.** Setting `opacity` (and/or
  `preTransformScale2d`) forces the panel onto its own compositing layer; in the hideout the
  blue ambient backdrop/post-grade then shows through (top bar is at 0.6 opacity for the test;
  abilities got a layer from its scale prop). Likely benign and tied to the test values, but
  investigate: try setting `opacity:1.0`/no-scale and see if the hue goes; if a layer is
  unavoidable, set `wash-color` to white or check for a `background-color`. Reproduce in the
  hideout specifically. Low priority; does not affect in-match.

## Footguns (already solved - keep them solved)

- resourcecompiler must be passed by **absolute path** through `proton run` (bare name ->
  silent "Failed to create process: 2"). Handled in `build_panorama_addon.py`.
- CSDK 12 schema skew aborts compiles unless `-danger_mode_ignore_schema_mismatches` (passed).
- Proton prefix `/tmp/proton-vpkmerge-rc` is wiped on reboot; the wrapper mkdirs it.
- **Always decompile the user's CURRENT retail `pak01` hud**, never the CSDK copy - they
  already differ (retail has `CitadelHudJoinTeam`, `CitadelHudGameAnnouncements`, different
  radial angles). An addon `hud.vxml_c` REPLACES the game's entirely.
- Some ids are ambiguous (`health_bar` is also in `OffscreenEnemySnippet`). Disambiguate by
  walking up `GetParent()` from a unique child (see `findRealHealthBar`).
- `hud.vxml` is generated - never hand-edit; change the include list and re-run inject.
- **The CSDK resourcecompiler compiles `.vcss` LENIENTLY; the retail engine is STRICT and
  fatals the whole hud layout on an unsupported CSS property** ("FATAL ERROR: Unable to load
  layout file hud.xml"). A clean compile is NOT proof the CSS loads. Confirmed-bad in retail
  (all in one overlay block, took down hud load): `gradient(...)` backgrounds, `box-shadow`,
  `@keyframes`/`animation-*`, `text-transform`, `letter-spacing`, `fill-parent-flow(...)`.
  Known-good vocabulary (the marker + the conservative timer card both load): color,
  font-size/-weight, padding, margin(-top/-bottom/-left), horizontal/vertical-align,
  background-color (incl. `#rrggbbaa`), border, border-radius, text-shadow, z-index,
  flow-children, width/height (px), text-align, descendant selectors. Add any fancier prop
  ONE at a time behind an in-game load test, never a batch.

## Feature backlog (prioritized for "a bunch of features")

Batch 1 - trivial `transformFeature` adds (reposition/scale/opacity), each ~5 lines:
- DONE (built, in-game confirm pending): minimap, items bar (`ActiveAbilitiesMenu`),
  quickbuy, hero shop (wired neutral so the shop isn't disrupted), passive items.
- Remaining: bottom signature bar split from abilities (if wanted separate), damage
  indicators, crosshair, AP currency (`APContainer` is nested under the souls
  `gold_and_ap_container` we already move - watch for double-move). Just add selector +
  config keys. TEST values for new adds are reposition-only on purpose: opacity/scale spawn
  a compositing layer (the hideout blue-hue artifact under Known issues).

Batch 2 - scrape/read features (pattern = health warning; find the `{i:}`/`{s:}` binding):
- SPM + unspent souls + net-worth readouts (top bar player slots `TopBarPlayer<i>`, souls
  labels). Unsecured-souls timer. Ammo color/scale. Enemy/ally health color-warnings (same
  true-value approach as self health, applied to top-bar slots).

Batch 3 - injected overlays (create new panels under the gameplay HUD):
- combat-status overlay, zipline-boost overlay, keyboard binding overlay, stat-bonuses
  overlay. Pattern: `$.CreatePanel` once, position via config, update text from scraped
  bindings.

Defer (need infra first): build save/load (needs the config-storage decision; do NOT clone
QOL's hero_airheart hijack), recent purchases, rejuv/objective timers (static timing tables),
audio reminders (mint `.vsnd_c` via the [[vsnd-c-minting]] path + a `.vsndevts_c` binding).

## Bigger milestones (do roughly in this order, between feature batches)

1. **Declarative schema + config bake** (the keystone; ~18 real config keys exist to model).
   One schema table -> generates `grimhud_config.vjs` defaults + the wire codec + the
   Grimoire settings UI rows + value formatting. Persistence decision is locked: **Grimoire
   desktop UI is the source of truth, baked into the addon at install** (kills QOL Lock's
   hero hijack). In-game live-preview + token-export is a LATER additive layer; build the
   runtime config-driven now so it drops in without rework.
2. **Patch-day selector harness** - a `vpkmerge`/oracle command that decompiles the live
   `pak01` and checks every `HUD_IDS` selector resolves, reporting what a patch broke before
   shipping. Mechanical now that all selectors flow through `HUD_IDS`.
3. **Grimoire managed-mod registration** - register grimhud via the Grimoire local-mod
   importer (see [[grimoire-local-mod-importer]]) instead of the raw `pak13` copy.
4. **Replace the dev marker** with a real (optional, off-by-default) status indicator.

## Pointers

- Memory: `grimhud-mod` and `qol-lock-teardown` (in the auto-memory dir).
- QOL Lock decompiled source for reference: `.scratch/qol/src/` (and `.scratch/qol/extracted/`).
- The 4 current features + dashboard are all in `grimhud_core.vjs` - read it; it's ~150 lines
  and is the template for everything.
