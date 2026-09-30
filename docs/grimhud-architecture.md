# grimhud architecture (successor design)

Engineering design for **grimhud**, the original Grimoire-native successor to the community
HUD mod **QOL Lock**. This is the "how we build it well" companion to two existing docs:

- `docs/qol-lock-teardown.md` : what QOL Lock is, its two foundational constraints, and the
  five-pillar strategy (A persistence, B schema, C adapter, D loader, E build).
- `grimhud/HANDOFF.md` : the proven build->in-game pipeline and current feature batch.

This doc turns the **performance + architecture audit of QOL Lock** (2026-06-30) into concrete
runtime design. It supersedes nothing; it makes Pillars A-E buildable.

Source of truth for behavior is the code in `grimhud/src/panorama/`. Where this doc and the
code disagree, the code wins and this doc is stale: fix it.

---

## 0. What the audit actually found (the design inputs)

We decoded QOL Lock (`vpkmerge panorama dump`) and read the always-on runtime across four
lenses. The headline is a **reframe**: QOL Lock is not naive spaghetti. It has a real feature
registry, a single 5 Hz core loop, per-feature gates, signature caching, panel caching,
idle-scaling, and phase-staggering. It is a **half-finished migration to a good architecture**.
The FPS cost comes from the loops and features that *escape* those guards, plus the structural
debt of running two dispatch models at once.

The offenders, distilled to the rules they teach us:

| QOL Lock cost (file:line) | Mechanism | grimhud rule it forces |
|---|---|---|
| `hud_quickbuy_total_summary.vjs:730` `UpdateQuickbuyQueueCostPanels` @ 20 Hz, uncached, runs even when inactive | ~300-400 uncached `FindChildTraverse`/sec | **One dispatcher. No side-loops. Resolve panels through a cache, never raw per tick.** |
| `ql_core.vjs:15183` `compassLoop` @ 20 Hz | 4x the base rate for a subset of features | **Per-feature cadence off one clock; nothing runs faster than it must.** |
| `ql_feat_bottombar.vjs:93` `applyCurrencyColor` before its sig gate | ~10 near-root class scans every 5 Hz tick | **Gate first, resolve second, write last. Always.** |
| `ql_feat_colorwarnings.vjs:508/637` reads `actuallayoutheight` per bar per tick | forced synchronous layout flush to infer HP% | **Never derive state from geometry. Read the bound value.** |
| `ql_core.vjs:3067` uncached `FindChildTraverse("Hud")` + config read every tick | fixed per-tick tax | **Read shared state once per tick into a frame context.** |
| healthbar scraped 3x/tick (self+ally+enemy); 4 features each cache the same 13 top-bar player panels | redundant re-scrape | **Scrape once per frame into a value object; features read fields.** |
| shared panel cache with **no negative caching** (`ql_core.vjs:71`) | absent panel = full-tree walk every tick forever | **Cache misses too, with backoff.** |
| `ql_recent_purchases_data.vjs` 375 KB icon map + `ql_minimap_crate_data.vjs` 41 KB dead table parsed every match | load-time bloat, some structurally unusable | **Lazy-load feature code + data; disabled feature costs zero.** |
| ~13 extra `world-blur: ingameHudBlur` panels; damage fountain animating `background-*` | per-frame blur passes + repaints | **Blur is opt-in and budgeted. Animate composited props (transform/opacity) only.** |
| config persisted by hijacking `hero_airheart` build/shop UI + a forever `buildRequestLoop` | a state machine + watcher loop for what should be a file write | **Persistence is a Grimoire file, baked in at install. No in-match writes.** |
| brittle ids, silent no-op on miss, keeps polling null | broken feature burns walks invisibly | **On miss: surface "HUD desync" and stop that feature's work.** |

CSS was largely exonerated: QOL is partly self-aware there (ships `world-blur: none` opt-outs,
collapses its own overlays). The FPS story is the JS runtime.

---

## 1. Design principles (the rules, standalone)

1. **One clock.** Exactly one `$.Schedule` self-loop in the whole mod. Every periodic action
   hangs off it. No feature and no panel instance calls the scheduler.
2. **Gate, resolve, diff, write.** Every feature runs in that order and bails at the first
   step that says "nothing to do." Disabled -> return before any DOM touch. Resolved-null ->
   return. Unchanged signature -> return before any style write.
3. **Read state once.** A per-tick **frame context** holds the shared, expensive reads (root,
   in-match flag, clock seconds, team slot arrays, local HP). Features read the context; they
   do not re-scrape.
4. **Cache handles, misses included.** Panel lookups go through a resolver that memoizes the
   handle, invalidates it when the panel goes invalid, and remembers a miss with exponential
   backoff so an absent panel is not a full-tree walk every tick.
5. **True values only.** State comes from bound attributes (`.value`, `{i:...}`/`{s:...}`
   labels), never from pixel geometry or locale-formatted text we re-parse if a binding exists.
6. **Cost scales with change, not time.** Steady-state (nothing on screen changed) is nearly
   free: signatures short-circuit. Cost appears only when the game state a feature watches
   actually moves.
7. **Zero cost when off.** A disabled feature does no work and, ideally, is not even loaded.
   Data tables load with their feature.
8. **Fail loud, then stop.** A missing selector surfaces a visible "HUD desync" signal and
   disables the dependent feature until it resolves, rather than silently polling null.
9. **One schema.** Every knob is one row in one table. Defaults, wire codec, settings UI,
   validation, and migration are all derived from it. Nothing is hand-synced.
10. **Budget honestly.** grimhud measures its own tick cost and can show it. If we add a blur
    or an animation, it is opt-in and it shows up on our own meter.

---

## 2. Current baseline (what grimhud already does right)

`grimhud/src/panorama/scripts/grimhud_core.vjs` (as of this writing) already honors several
rules, and the design below is an evolution of it, not a rewrite:

- **One tick.** A single `tick()` at 0.5 s runs all features in a list, each with a
  signature-skip (`grimhud_core.vjs:507`). No per-instance loops, no 20 Hz side-loops. This is
  already ahead of QOL's hybrid.
- **True values.** Low-HP reads the bound `current_health`/`max_health` labels and computes an
  exact percent (`:163-192`), explicitly rejecting QOL's bar-pixel-height inference. Top-bar
  ally/enemy warnings read `HeroHealth.value` (`:199-216`). Econ/respawn read bound labels.
- **Adapter + self-test.** All game panel ids live in one `HUD_IDS` map (`:21-45`), resolved
  through one `resolve()` that logs a miss once and drives an on-screen `N/M` self-test marker
  (`:466-505`). This is Pillar C in seed form.
- **Baked config.** Features read `GRIMHUD_CONFIG` from `grimhud_config.vjs` (`:12-16`). No
  hero hijack, no in-match writes. Pillar A in seed form.
- **Neutral-clear discipline.** A "1.00" scale/opacity is cleared rather than written, to avoid
  forcing a panel onto its own compositing layer (`:72-94`). This is real Panorama-perf care.

The gaps this doc closes (all are QOL patterns grimhud still shares, just at 2 Hz instead of
20 Hz, so cheaper but not yet clean):

- `resolve()` calls `FindChildTraverse` **every tick, uncached** (`:52`), with no negative
  caching. Rule 4 unmet.
- `inMatch()` (`:269`), `clockSeconds()` (`:236`), and team-slot walks are recomputed
  **independently by 4+ features per tick** (timer, econ, respawn, topbar-hp each call some of
  them). Rules 3 + 6 partially unmet: the same `TeamFriendly` subtree is walked several times
  per tick, and the parent-chain `inMatch` probe runs per feature.
- Config keys are scattered string literals with inline defaults (`num(cfg.X, default)`
  everywhere). No schema. Rule 9 unmet.
- Every feature runs every tick even when disabled (cheap: one config read + return), and every
  feature runs at the one global rate. No per-feature cadence. Rules 6 + 7 partially unmet.

---

## 3. The v2 runtime

Four small pieces: **FrameContext**, **Resolver**, **Feature/Registry**, **Dispatcher**. All
live in the same Panorama JS sandbox idiom already in use (ES5, `"use strict"`, `$.Schedule`,
the panel API). Sketches below are illustrative, not final.

### 3.1 FrameContext (read shared state once)

Built once at the top of each tick and passed to every feature. It owns the expensive shared
reads that QOL (and current grimhud) duplicate across features.

```js
// Built once per tick. Features READ this; they never re-scrape what it holds.
function buildFrameContext(root, resolver) {
    var ctx = {
        root: root,
        resolver: resolver,
        _clock: undefined,      // lazy: computed on first use, then memoized for the tick
        _inMatch: undefined,
        _allySlots: undefined,
        _enemySlots: undefined
    };
    ctx.inMatch = function () {
        if (this._inMatch === undefined) this._inMatch = computeInMatch(root, resolver);
        return this._inMatch;
    };
    ctx.clockSeconds = function () {
        if (this._clock === undefined) this._clock = computeClockSeconds(resolver);
        return this._clock;
    };
    ctx.allySlots = function () {
        if (this._allySlots === undefined) this._allySlots = collectSlots(resolver, "teamFriendly");
        return this._allySlots;
    };
    ctx.enemySlots = function () { /* symmetric */ };
    return ctx;
}
```

Result: `inMatch` is computed at most once per tick no matter how many features ask; the
`TeamFriendly` subtree is walked once for ally features; the clock label is read once for both
the timer card and the econ line. This alone deletes the current per-feature duplication at
`grimhud_core.vjs:283, 292, 354, 361, 409, 412`.

Lazy (compute-on-first-use) rather than eager so a tick where no enabled feature needs the
ally slots does not walk them. The memo lives for one tick only, so it never serves stale state.

### 3.2 Resolver (cache handles, misses included)

Replaces the raw `resolve()`. Same call site shape, but memoized with invalidation and negative
caching. This is the direct fix for QOL's no-negative-cache walk-forever (`ql_core.vjs:71`) and
grimhud's uncached per-tick `FindChildTraverse` (`grimhud_core.vjs:52`).

```js
function makeResolver(HUD_IDS) {
    var cache = {};                 // name -> panel | null
    var missUntilTick = {};         // name -> tick index to retry a miss (backoff)
    var status = {};                // name -> bool, drives the self-test marker
    var tickIndex = 0;

    function valid(p) { return p && p.IsValid && p.IsValid(); }

    return {
        beginTick: function () { tickIndex++; },
        status: status,
        resolve: function (root, name) {
            var hit = cache[name];
            if (valid(hit)) { status[name] = true; return hit; }     // fast path: cached handle
            if (missUntilTick[name] > tickIndex) { status[name] = false; return null; } // backoff

            var ids = HUD_IDS[name] || [];
            for (var i = 0; i < ids.length; i++) {
                var p = root.FindChildTraverse(ids[i]);
                if (valid(p)) { cache[name] = p; status[name] = true; return p; }
            }
            cache[name] = null; status[name] = false;
            // Absent: back off so we do not full-tree-walk every tick. 1,2,4,8... capped.
            var prev = missUntilTick[name] ? (missUntilTick[name] - tickIndex) : 0;
            missUntilTick[name] = tickIndex + Math.min(Math.max(prev * 2, 1), 16);
            surfaceMiss(name, ids);   // rule 8: loud, once
            return null;
        }
    };
}
```

Notes:
- The fast path is a validity check, not a tree walk. A resolved panel costs ~nothing on
  subsequent ticks.
- Backoff means a panel that only exists in-match (e.g. `TeamFriendly` in the hideout) is
  probed ~1x/second at most while absent, not 2x/tick.
- `status` still feeds the on-screen self-test unchanged.
- Invalidation is automatic: when the engine rebuilds the top bar across a match/hideout
  transition, the cached handle goes invalid and the next `resolve()` re-walks once.

### 3.3 Feature model + registry

A feature becomes data, not a bare function in a list. This lets the dispatcher gate and pace
it without every feature re-implementing the pattern (and makes the "disabled costs zero" rule
enforceable centrally).

```js
// A feature declares how it is gated and how often it needs to run.
// update(ctx) does the work; it may assume it is enabled and (if it declared anchors) resolved.
registry.add({
    id: "topbarAllyHpWarn",
    enabledKey: "ALLY_HP_WARN_ENABLED",   // dispatcher checks config BEFORE calling update
    cadence: 2,                            // run every 2nd tick (see dispatcher); default 1
    anchors: ["teamFriendly"],             // dispatcher resolves these; all-miss => skip + mark desync
    update: function (ctx) {
        var slots = ctx.allySlots();       // shared, walked once per tick
        var th = cfg.num("ALLY_HP_THRESHOLD", 35) / 100;
        for (var i = 0; i < slots.length; i++) {
            var hb = slots[i].FindChildTraverse("HeroHealth");
            if (!hb) continue;
            var v = Number(hb.value);
            if (v === v) hb.style.washColor = (v > 0 && v <= th) ? cfg.color("ALLY_HP_COLOR") : "white";
        }
    }
});
```

The dispatcher, not the feature, does: config-enabled check, cadence check, anchor resolve,
and try/catch isolation. A feature body is only the diff-and-write. Feature-local signature
state (the `sig` closures already in grimhud) stays inside `update` or moves to a small helper.

### 3.4 Dispatcher (the one clock)

```js
var TICK_SEC = 0.25;      // one base rate; cadence multiplies it per feature
var tickN = 0;

function tick() {
    try {
        var root = $.GetContextPanel();
        if (root) {
            resolver.beginTick();
            var ctx = buildFrameContext(root, resolver);
            var feats = registry.list();
            for (var i = 0; i < feats.length; i++) {
                var f = feats[i];
                if (f.enabledKey && cfg.num(f.enabledKey, f.defaultEnabled ? 1 : 0) !== 1) continue; // gate
                if (f.cadence > 1 && (tickN % f.cadence) !== 0) continue;                            // pace
                if (f.anchors && !anchorsPresent(ctx, f.anchors)) continue;                          // resolve/skip
                var t0 = perfNow();
                try { f.update(ctx); } catch (e) { $.Msg("[GRIMHUD] " + f.id + ": " + e + "\n"); }
                perfAccum(f.id, perfNow() - t0);   // rule 10: self-measure
            }
            selfTest.update(ctx, resolver.status);
        }
    } catch (e) { $.Msg("[GRIMHUD] tick: " + e + "\n"); }
    tickN++;
    $.Schedule(TICK_SEC, tick);
}
```

Cadence lets cheap-but-slow readouts (econ pace, objective timers) run at 1 Hz while
reposition/scale features (which only change when config or the panel changes) can run at the
base rate but exit instantly on an unchanged signature. There is still exactly one
`$.Schedule` loop. Base rate is a single tunable; start at 0.25 s (4 Hz) and let cadence stretch
individual features, rather than QOL's spread of 0.01-1.2 s literals.

### 3.5 Adapter self-test (formalize what exists)

Keep grimhud's on-screen `N/M` marker (`grimhud_core.vjs:466`), but drive the "HUD desync"
state from the resolver's backoff, and version the id table. When a required anchor for an
*enabled* feature misses, the marker goes red and names the feature, not just the selector.
Ship a `HUD_IDS_VERSION` stamped against a known game build so a patch-day miss is
attributable. This is the reliability headline: when Valve renames a panel, grimhud tells the
user "reposition-minimap: HUD changed, update needed" instead of quietly not moving the minimap.

### 3.6 Self-measured perf HUD (rule 10)

`perfAccum` keeps a rolling per-feature microsecond total; a dev overlay (behind
`DEBUG_OVERLAY`, same gate as the current marker) prints the top few features by cost and the
whole-tick total. This is the honest inverse of QOL shipping a perf overlay while being a perf
cost: grimhud can prove its own tick is sub-millisecond, and any regression (a new feature that
walks from root) is visible immediately during development. It costs nothing in release (gated
off).

---

## 4. Config architecture (Pillar B, made concrete)

One schema table is the single source of truth. Author it once (Rust side, canonical), generate
the JS mirror at build time.

```
SETTINGS = [
  { id: "SOULS_X_OFFSET", type: "int",   default: 0,  min: -400, max: 400, step: 1,
    label: "Souls X offset", category: "Layout", tab: "HUD Layout", feature: "reposSouls",
    perfTier: "free" },
  { id: "ALLY_HP_WARN_ENABLED", type: "bool", default: true,
    label: "Warn on low ally HP", category: "Warnings", tab: "Combat", feature: "topbarAllyHpWarn",
    perfTier: "cheap" },
  ...
]
```

Everything derives from this table, so nothing is hand-synced (QOL maintained defaults, wire
codec, and UI as three parallel structures kept in sync by a runtime cross-check guard):

- **Defaults map** -> baked into `grimhud_config.vjs` at build (the values the user chose,
  falling back to `default`).
- **Wire codec** -> field order + bit widths for the shareable preset token (keep QOL's
  bit-packed base64url idea; it was the cleanest part of the original, but for *sharing* presets
  between users, not for persistence).
- **Settings UI** -> Grimoire renders rows from the table (type -> control, min/max/step ->
  slider bounds, category/tab -> grouping). No hand-built settings panels.
- **Validation + migration** -> clamp to min/max on load; a schema version + additive-only rule
  (mirror the `/v1` additive discipline from grimoire-social) handles old saved presets.
- **perfTier** -> shown in the UI ("free / cheap / moderate") so a user throttling for FPS knows
  what each toggle costs. Derived from the audit's cost model, honest by construction.

The JS runtime reads only the flat baked `GRIMHUD_CONFIG`; it never sees the schema. `cfg.num`,
`cfg.bool`, `cfg.color` wrap the reads with the schema defaults so feature bodies stop repeating
literal fallbacks.

---

## 5. Persistence (Pillar A, made concrete)

No `hero_airheart`, no shop puppet, no `buildRequestLoop`. The flow:

1. User edits settings in Grimoire (real SQLite/JSON storage; Grimoire owns it).
2. On install/apply, the Rust builder bakes the chosen values into `grimhud_config.vjs` and
   packs the addon VPK (the proven `build_panorama_addon.py` + `vpkmerge` path from HANDOFF).
3. In-match, the runtime only **reads** its config. It never writes. There is no persistence
   code in the hot path at all.

Preset sharing (the one thing worth keeping from QOL's codec): serialize a config to the
bit-packed base64url token from the schema wire layout, so users can paste presets to each
other. That is a pure function, not a storage mechanism, and never runs in-match.

Cost delta vs QOL: ~3000 lines of state machine + a forever watcher loop become a build step
and a file read.

---

## 6. Packaging + hygiene

The build pipeline is proven (`grimhud/HANDOFF.md`): `morphic-oracle panorama decompile` to get
the vanilla `hud.vxml`, `inject_hud_scripts.py` to add our includes, `build_panorama_addon.py`
to compile via resourcecompiler (Proton) and pack via `vpkmerge`. Note that
`vpkmerge panorama build` (added in #36) also rebuilds an edited dump, useful for round-trip
tests. Hygiene rules the QOL VPK violated and we should not:

- **Ship only runtime.** No `resourcecompilerwarnings_*.txt` build logs (QOL shipped one leaking
  its author's build path and a broken-texture compile error). No dev/build scripts
  (`minify_*`, `validate_*`) in the release VPK.
- **No dead data.** Do not pack a data table a feature cannot use (QOL ships a 41 KB crate-coord
  table gated behind an absent `Game.GetMapInfo`). If the host API is not there, the feature and
  its data do not ship.
- **Lazy feature bundles.** Long term, feature JS + its data load only when the feature is
  enabled in the baked config, so a minimal HUD is a minimal VPK (kills QOL's 375 KB
  always-parsed icon map). Short term, since the config is known at build time, the builder can
  simply omit disabled features' code from the packed bundle.

---

## 7. Module layout (Pillar D)

Split the single IIFE into real modules, loaded in dependency order by the injected `<scripts>`
block. Keep each small; the god-object `State` QOL carries (656 fields, `ql_core.vjs:100`) is the
anti-pattern, so state stays per-feature or on the FrameContext.

```
grimhud/src/panorama/scripts/
  grimhud_config.vjs      generated: flat GRIMHUD_CONFIG (baked values) + schema defaults mirror
  grimhud_adapter.vjs     HUD_IDS + HUD_IDS_VERSION + makeResolver + surfaceMiss + selfTest
  grimhud_context.vjs     buildFrameContext + computeInMatch/clock/slots (shared reads)
  grimhud_registry.vjs    feature registry (add/list) + cfg helpers (num/bool/color)
  grimhud_features/       one file per feature (or per small group); each calls registry.add(...)
  grimhud_core.vjs        the dispatcher: builds context, runs the registry, self-test, perf HUD
```

Load order is dependency order (config -> adapter -> context -> registry -> features -> core),
but no feature should depend on another feature's load having happened: the registry is the only
cross-feature contract, and `core` runs last.

---

## 8. Rollout order

Do not rewrite in one shot. Migrate `grimhud_core.vjs` in place, smallest-risk first, verifying
in-game (or at least that the packed VPK still loads the HUD, the CSDK-vs-retail CSS footgun from
HANDOFF is real) after each step:

1. **Resolver with caching + negative caching.** Drop-in replace `resolve()`. Pure perf, no
   behavior change. Verify the self-test marker still reads the same hits/misses.
2. **FrameContext.** Move `inMatch`/`clockSeconds`/team-slot walks behind the context; point the
   4+ callers at it. Pure perf, no behavior change.
3. **Registry + dispatcher gating/cadence.** Convert the `FEATURES.push(fn)` list to
   `registry.add({...})`. Move the enabled-check and try/catch into the dispatcher. Add cadence
   to the timer/econ/respawn readouts.
4. **Schema + generated config.** Author the Rust schema table, generate `grimhud_config.vjs` and
   the JS defaults mirror; delete inline literal fallbacks. Wire the Grimoire settings UI to the
   table. This is the keystone (Pillar A+B) and the biggest single quality jump.
5. **Self-measured perf HUD + versioned adapter.** Formalize the self-test; add the dev perf
   overlay.
6. **Module split + build-time feature omission.** Once the shape is stable, split files and
   teach the builder to drop disabled features.

Steps 1-2 are pure wins with no behavior change and should land first. Step 4 is where "quality
across the board" actually compounds, because it deletes the three-way config sync, the scattered
defaults, and the missing settings UI in one move.

Scope guidance still holds (`qol-lock-teardown.md` section 5): a small high-confidence core on
this foundation beats a breadth-first clone. The foundation is the product.

---

## 9. Open questions

- **Host game-state API.** The single highest-leverage move is Grimoire (or a companion) exposing
  real game state so features stop scraping the DOM entirely. That deletes the FrameContext's
  scrape helpers, the selector brittleness, and the last of the persistence hijack rationale. It
  is also the largest unknown (does Grimoire have a channel into the running game? the Panorama
  sandbox says no native API). Until it exists, the FrameContext scrape layer is the seam where it
  would plug in: keep that seam clean.
- **Base tick rate.** 0.25 s is a guess. The self-measured perf HUD should set it empirically:
  fast enough that readouts feel live, slow enough that the whole tick is a rounding error.
- **CSS re-add discipline.** HANDOFF notes retail CSS is stricter than CSDK and one bad property
  fatally breaks HUD load. Re-add polish (gradients, shadows, the timer pulse) one property at a
  time behind a load test, and keep any `world-blur` opt-in and on the perf meter.
- **Objective cadence rot.** The `OBJECTIVE_CADENCE` table is version-stamped and isolated
  (`grimhud_core.vjs:224`) but still unverified placeholder timing. Prefer observed-edge
  correction (anchor to a real spawn announcement) over an open-loop stopwatch once we can read
  one.
```
