# QOL Lock teardown + plan for an original successor

Analysis of the community mod **QOL Lock** (`qol_lock_dir.vpk`, 67 MB, pulled from
`Deadlock/game/citadel/addons/.disabled/`) and a strategy for building an original
Grimoire-native HUD/QOL mod that surpasses it.

Source extracted to `.scratch/qol/extracted/` (raw) and `.scratch/qol/src/` (decompiled
JS/CSS/XML). Re-extract any addon VPK with
`cargo run -p vpkmerge-core --example dump_all_entries -- <vpk> <outdir>`.

## 1. What QOL Lock is

A Panorama (Source 2 UI) HUD mod. It is **not** a skin/VFX mod: it ships compiled UI, not
materials. Payload by type:

| Type | Count | Role |
|---|---|---|
| `.vcss_c` | 127 | CSS overrides + injected styles |
| `.vtex_c` | 95 | UI icons (statlocker, minimap, cursor, item icons) |
| `.vsnd_c` | 66 | announcer + DL4D + shopkeeper VO corpus |
| `.vjs_c` | 55 | the runtime (~75k lines of JS) |
| `.vxml_c` | 28 | layout overrides |
| `.vsvg_c` | 4 | vector UI |
| `.vsndevts_c` | 1 | KV3 soundevents binding QOL event names -> vsnd_c |

**Injection mechanism:** it overrides the game's `panorama/layout/hud.vxml` and adds a
`<scripts>` block that loads the whole runtime (`ql_utils` -> `ql_perf_overlay` ->
`ql_shared_presets` -> `ql_core` -> `ql_features/*` -> `ql_settings`), then overrides ~60
more `citadel_hud_*` layout/CSS files to add the panel IDs/classes the runtime hooks into.
Standard Deadlock HUD-mod pattern.

### The two foundational constraints (these shape everything)

The Deadlock Panorama sandbox exposes **no game-data API** and **no persistent storage**.
Confirmed-absent in the code: `Game.GetGameTime`, `Game.GetLocalPlayerInfo`, `Players.*`,
`Game.GetMapInfo`, `$.persistentStorage`, `GameInterfaceAPI`, `GameUI.CustomUIConfig`,
readable convars. So QOL Lock is built on two heroic workarounds:

1. **All game state is screen-scraped.** Health, souls, SPM, clock, mid-boss spawn, ammo,
   cooldowns, respawn timer are read by parsing rendered panel `.text` and CSS classes, or
   even inferred from pixel geometry (health % = `fill.actuallayoutheight / parent height`).
2. **All persistence is a hijacked build.** There is no place to save settings, so it
   co-opts an almost-never-played hero (`hero_airheart`) as a key/value namespace and
   writes the **entire mod config**, bit-packed to a base64url token, into the *category
   name text field* of one of that hero's saved item builds. On launch it silently switches
   you to Airheart, opens the shop, reads the token back, decodes, applies, switches you
   back. Saving is scripted button-clicks + text-entry against the live shop UI.

Roughly 80% of the mod's complexity is machinery to make those two workarounds reliable.

## 2. Architecture

- **Module system** (`ql_shared_presets.js`): a shared mutable global `QOL`. `QOL.register`
  records feature descriptors `{configKeys, bucket(0-7), phase(-1..4), gate, update,
  cleanup}`; `QOL.import([...])` is an **eager snapshot dictionary lookup** (no deferred
  resolution) so correctness depends entirely on a fixed script load order. `ql_core.js`
  republishes ~150 inner functions onto `QOL` via a lazy-getter `try/catch` bridge (crash
  isolation, not live binding).
- **Scheduler** (`ql_core.js`): self-rescheduling `$.Schedule` loops (`loop()` 5 Hz,
  `compassLoop()` 20 Hz, `buildRequestLoop()` tiered). Two orthogonal staggering systems
  (8 intra-tick offset buckets + a 5-phase `tickSerial % 5` spread) plus idle degradation
  (up to 15x slower when out of match / HUD hidden / low FPS), gate-signature reuse, and a
  hard-gate early-return. Panel handles live in one `State.cachedPanels` bag, revalidated
  per read via `IsPanelValid` and swept once/second.
- **Config & settings** (`ql_settings.js`, 25k lines): triple-mirrored JSON blob written to
  panel attributes (`Deadlock_Mod_Settings_v1`) on 3 panels with a monotonic revision
  counter for last-write-wins reconciliation; debounced 0.3s saves. **Three parallel
  sources of truth**: `QOL_DEFAULT_CONFIG` (~700 keys), `QOL_COMPACT_SCHEMA_V2` (~69
  shareable fields w/ min/max/step), and the imperative `CreateRow(...)` UI calls
  (type/label). A semver-keyed schema registry (`3.1.4`, additive-only) drives a bit-packed
  base64url import/export codec (`QOL_CODEC`) and migration passes.
- **Build-time tooling** (`scripts/`): `validate_compact_schema.js` cross-checks the two
  schema copies in `vm` sandboxes and fuzzes the codec round-trip; `minify_panorama_js.js`
  is a hand-rolled minifier; `qollock_translations.js` extracts UI strings across 10
  languages.

### Engineering quality

Genuinely strong: layered try/catch isolation (per-bucket, per-feature, per-loop with
`finally` reschedule), an **auto-disable circuit breaker** (10 consecutive throws kills a
feature for the session), pervasive defensive helpers (`SafeGetAttribute`, hop-guarded
traversals), throttled logging, torn-write-resistant storage, a perf overlay with rolling
windows, and build-time schema/codec validation. This is a mature, battle-hardened
codebase, not a hack.

## 3. The full feature inventory (~40 features)

**HUD layout / bars:** souls, top bar, bottom bar (reposition/scale/opacity/recolor);
nicknames; lane-with-party.
**Economy readouts:** SPM (souls/minute, window-delta), unspent souls (net worth minus
tier-count * hardcoded cost table), stat bonuses (Golden Statues overlay), statlocker.gg
deep-links (3 separate copies: in-match, profile page, friend card).
**Combat / crosshair:** ammo restyle, stamina ring color/angle, clean damage numbers,
damage-impact reticle, combat-status overlay, target shapes / red diamond, signature
cooldown press-flash, keyboard binding overlay, zipline boost overlay, color warnings (self
+ enemy + ally health-bar threshold coloring).
**Minimap:** resize/zoom (Alt/Tab)/reposition/opacity/minimalist/icon-recolor/draw-over-UI,
crate overlay, Rem-tunnel overlay; rejuv + bridge-buff countdown timers (the single largest
feature, ~1300 lines, with HUD + minimap render modes).
**Audio:** timed buff/objective reminders ("DL4D") with 4 announcer voices x 3 volumes, 5
user-fillable custom-announcer slots, custom shopkeeper VO. Backed by a 54-file `vsnd_c`
corpus + one `.vsndevts_c` binding; fired by `$.DispatchEvent("PlaySoundEffect", name)`.
**Build manager:** local build save/load/share via the Airheart hijack; recent-purchases
tracking + per-hero purchase popups; hero-shop / item-slot HUD styling.
**Misc:** images-in-chat (renders remote URLs inline), on-death arcade bridge (signals a
separate minigame mod), custom mouse cursor, an in-settings Minesweeper.

## 4. Where it is weak (our opening)

1. **Game state from pixels and locale-sensitive text.** Health % from bar height, SPM from
   diffing `1.2k`-formatted labels (sub-100 changes vanish to rounding), clock from an
   `MM:SS` string, zipboost duration **hardcoded to 32s**, combat recovery a 3s guess. Any
   Valve HUD rename silently no-ops a feature *while predicted timers keep counting* (false
   confidence, no signal to the user).
2. **The Airheart storage hijack** is the biggest foot-gun: it switches your hero and opens
   your shop on every launch and every save; the return-hero step can strand you on the
   wrong hero on a hitchy connection; corrupt-repair **deletes builds**; payload size is
   bounded by an unknown text-field max with no length guard. ~3000 lines exist to make
   this not blow up.
3. **Three parallel config sources of truth** hand-synced (defaults / wire schema / UI
   rows), with value formatting sniffed from substring patterns in the key name
   (`id.indexOf("OPACITY")`). The build-time cross-check guard exists *because* of this.
4. **Static timing tables rot.** `DL4D_REMINDER_EVENTS`, `REJUV_SEQ`, the `{1:800, 2:1600,
   3:3200, 4:6400}` tier-cost table — all hardcode current Valve cadence/prices and desync
   on any balance patch.
5. **Duplication + dead code.** Account-id resolution reimplemented 3x; the red-pulse
   triangle wave hand-coded 3x; `ql_legacy_cooldowns.js` polls forever to `return false`;
   minimap advertises rotate/flip/elevation config keys no code reads; crate-overlay map
   detection stubbed to one map (`dl_midtown`); several `configKeys` advertise toggles
   `update()` never reads.
6. **Security: images-in-chat renders arbitrary remote URLs** from other players' chat with
   no allowlist / size cap / proxy — an IP-grabber + shock-image vector.
7. **Monolith files** (25k-line settings, 17k-line core) hostile to review and to the
   decompiler. Imperative per-element layout in JS where XML/CSS would do.

## 5. Strategy for the successor

We hold an advantage the QOL Lock author does not: we own the **whole pipeline** -
`vpkmerge` (pack/merge), `morphic` (Source 2 + KV3 + vsnd_c minting), and **Grimoire** (the
mod manager that installs the VPK and has a real filesystem + DB). That lets us attack the
two foundational constraints directly instead of working around them.

### Pillar A - solve persistence properly (kills the Airheart hijack)
Grimoire has real storage. Settings live in Grimoire (SQLite/JSON), and the addon VPK is
(re)built/patched with the chosen config **baked in at install time** via our Rust pipeline
(same `pack`/`from_directory` primitives the rest of vpkmerge uses). In-match, the runtime
only *reads* its config (already injected), never writes to a hijacked build. Result: no
hero swap, no shop puppet, no corrupt-repair build deletion, no payload size cliff. ~3000
lines of state machine become a config file. (Keep the bit-packed token codec for
*sharing* presets between users - it is the cleanest part of the original.)

### Pillar B - one declarative schema (kills the 3-way sync)
A single Rust-side (and JS-mirrored, generated) schema table:
`{id, type, default, min, max, step, label, category, tab, configKeys, perfTier}`. Derive
*everything* from it: the defaults map, the wire codec layout, the settings UI rows, value
formatting, migration. No more substring-sniffing, no more cross-check guard needed.

### Pillar C - a HUD-adapter layer with self-test (kills silent breakage)
Centralize every Valve panel ID / class / scrape selector into one versioned `HUD_IDS` +
`adapters` module. One `resolveByCandidates(root, ids, classes)` that **logs and surfaces a
visible "HUD desync - update needed" indicator when all candidates miss**, instead of
silently no-opping. This is the headline reliability win: when Valve patches the HUD, our
mod *tells the user* rather than quietly half-breaking. A startup self-check reports which
expected panels failed to resolve.

### Pillar D - real module loader + clean core
Deferred dependency resolution (pending-queue or live getters) so load order stops being
load-bearing; registry metadata is authoritative for gate/bucket/phase (delete the parallel
`ResolveRuntimeGates` booleans and the 4 hand-maintained sidecar tables). Split the
monoliths into real modules; move imperative overlay layout into XML/CSS, JS just toggles
classes + sets CSS vars.

### Pillar E - reproducible Rust build pipeline
Author UI source (JS/CSS/XML) + a feature manifest; a Rust `qolmod`-style builder compiles
to `.v*_c` / packs via vpkmerge, mints the announcer `vsnd_c` corpus via the proven
`morphic` path ([[vsnd-c-minting]]), and registers the result as a Grimoire local mod
([[grimoire-local-mod-importer]]). CI-able, no manual ModelDoc, no .NET.

### Quick wins to "surpass" on day one
- Chat-image allowlist + size cap + click-to-load gate (fix the security hole).
- Localization-safe readouts (numeric/token sources, never scraped `.text` + appended "s").
- Honest economy math (true per-minute SPM normalization; item costs from a versioned data
  table, ideally read from game data, not a frozen tier table).
- Version-stamp the objective/rejuv timing tables and prefer observed-edge correction over
  open-loop stopwatches.
- User-configurable cosmetics (cursor image, overlay textures) via the same asset-slot
  pattern the original already proved for custom announcers.

### Scope guidance
QOL Lock is ~40 features and years of hardening; do not clone it breadth-first. Ship a small
high-confidence core first (the bars, economy readouts, color warnings, minimap sizing,
announcer reminders) on the new foundation (Pillars A-E), prove the HUD-adapter self-test
and the Grimoire-baked config in-game, then expand. The win is the *foundation*, not
matching the feature count on day one.
