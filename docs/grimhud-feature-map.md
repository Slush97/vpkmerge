# grimhud feature map (QOL Lock surface + triage)

The **product-surface** companion to `docs/grimhud-architecture.md` (how to build well) and
`docs/qol-lock-teardown.md` (what QOL Lock is). This doc maps **every** QOL Lock feature,
scores its cost, and gives a build verdict: what a higher-quality successor needs, what it can
drop, and what it should add that QOL never had.

It is written for an agent picking up one feature to build on the grimhud v2 runtime. For each
kept feature you get: the true data source (so you do not re-scrape pixels), the cost the
original paid, and the pitfall to avoid. Cost tiers come from the 2026-06-30 audit of the live
VPK; file:line refs are into the decode at `.scratch/qol/panorama/panorama/scripts/`.

## How to read the tables

**CPU** (per-frame runtime cost in QOL, from the DOM-walk + hot-loop audit):
- `HOT` : uncached full-tree walk or 20 Hz loop. Re-implement carefully; these caused the FPS drop.
- `MOD` : root/class walk but throttled to ~0.5-1 Hz, or heavy per-invocation work.
- `LOW` : cache/sig-gated, narrow, or clean. Cheap even in the original.
- `EVT` : event/burst only (save, death, popup). No steady per-tick cost.

**GPU** : `BLUR` = adds a `world-blur` backdrop pass; `PAINT` = animates repaint-forcing props
(background-*); blank = composited/cheap.

**Verdict** :
- `KEEP` : port as-is onto the v2 runtime (cheap + valuable).
- `IMPROVE` : port but fix a real defect (perf bug, dishonest data, rot-prone table).
- `REBUILD` : the idea is good but the QOL implementation is unsafe/wrong; redo from scratch.
- `CUT` : do not port. Dead code, structurally impossible, or not worth the surface.
- `NEW` : grimhud-only, no QOL equivalent.

**gh** : grimhud status today (`✓` shipped, `~` partial, `-` not yet).

---

## 1. HUD layout: reposition / scale / opacity / hide

The bread and butter. In QOL these are cheap generic transforms; grimhud already collapses all
of them into one `transformFeature` (`grimhud_core.vjs:75`). One feature, one config group each.

| Feature | QOL | What it does | CPU | GPU | Verdict | gh |
|---|---|---|---|---|---|---|
| Souls container move/fade | `ql_feat_souls.vjs` | reposition + opacity the gold/AP block | LOW | | KEEP | ✓ |
| Top bar move/scale/fade | `ql_feat_topbar.vjs` | reposition + scale + opacity the top scoreboard | LOW | | KEEP | ✓ |
| Bottom bar move/scale/fade **+ AP recolor** | `ql_feat_bottombar.vjs` | as above **plus** tint the AP currency icons | **HOT** | | IMPROVE | ~ |
| Abilities / items / passives / signature / quickbuy / hero-shop / crosshair / damage-ind / chat / stats bars | (core `transformFeature`) | reposition/scale/opacity/hide each HUD panel | LOW | | KEEP | ✓ |
| Minimap resize/zoom/reposition/opacity/minimalist/draw-over-UI | `ql_feat_minimapruntime.vjs` | live restyle + Alt/Tab zoom of the minimap | **HOT** | | IMPROVE | ~ |
| Minimap icon recolor | `ql_feat_minimapruntime.vjs` | recolor minimap icons from a palette | HOT | | IMPROVE | - |
| Nicknames | `ql_feat_nicknames.vjs` | custom names on top-bar player slots | LOW | | KEEP | - |
| Lane-with-party | `ql_feat_lanewithparty.vjs` | show party lane pref in the lobby/menu | LOW | | KEEP | - |

**Build notes.**
- **`bottombar` AP recolor is the single worst QOL loop** (`:93` `applyCurrencyColor` runs
  *before* its signature gate, ~10 near-root `FindChildrenWithClassTraverse` every 5 Hz tick,
  including from the UI root and context panel). The fix is the v2 rule verbatim: gate on a
  wash-color signature first, resolve a narrow cached parent, drop the root/context containers.
- **`minimapruntime`** is Tier A: `:380` does an uncached full-root `FindChildTraverse` up to
  2x/tick. Port the *behavior* but resolve through the caching Resolver; the "draw over UI" and
  zoom-on-Alt are the real value.
- QOL advertises minimap `rotate` / `flip` / `elevation` config keys that **no code reads**
  (teardown 5). Do not carry those keys.

---

## 2. Economy readouts

QOL's weakest data layer: it diffs locale-formatted `1.2k` labels (sub-100 changes round away)
and uses a hardcoded `{1:800,2:1600,3:3200,4:6400}` tier-cost table that desyncs on any balance
patch. grimhud already replaced the core of this with true-binding reads.

| Feature | QOL | What it does | CPU | Verdict | gh |
|---|---|---|---|---|---|
| SPM (souls per minute) | `ql_feat_spm.vjs` | pace from windowed label diffs | LOW | IMPROVE | ~ |
| Unspent souls nudge | `ql_feat_unspent.vjs` | warn when sitting on unspent gold | LOW | IMPROVE | ✓ |
| "Unsecured" souls readout | `ql_feat_unsecuredsouls.vjs` / `ql_feat_betterunsecuredhud.vjs` | show souls you would drop on death | LOW | KEEP | - |
| Stat bonuses (Golden Statues) | `ql_feat_statbonuses.vjs` | overlay current stat buffs | LOW | KEEP | - |
| statlocker.gg deep-links (x3) | `ql_feat_statlocker.vjs` + profile/card copies | open statlocker.gg for a player | MOD | CUT | - |
| **Soul lead + team pace** | (none) | team net-worth delta + true per-min | LOW | NEW | ✓ |

**Build notes.**
- **SPM / unspent**: read the bound `{i:hud_cur_gold}` (`unspentSouls` in grimhud's `HUD_IDS`)
  and `{s:team_networth}` labels, not scraped display text. Normalize to true souls/min
  (grimhud's econ line already does this at `grimhud_core.vjs:350`). Drop the hardcoded tier
  table; if unspent needs item costs, read them from game data or a versioned table, never a
  frozen literal.
- **statlocker.gg deep-links**: three near-identical copies (in-match, profile page, friend
  card), an external dependency for low value. `CUT` for the core; if wanted later, one shared
  helper, not three.
- The **soul lead** readout is a grimhud win QOL never had (QOL shows each team's net worth but
  never the lead or the pace). Keep it as the flagship economy feature.

---

## 3. Combat / crosshair

Mostly cheap in the original. Two exceptions (`targetshapes` at 60 ms refresh, `sigflash`
degrading to 5 Hz when its panel is absent) and one GPU cost (damage numbers).

| Feature | QOL | What it does | CPU | GPU | Verdict | gh |
|---|---|---|---|---|---|---|
| Ammo restyle | `ql_feat_ammo.vjs` | recolor/reposition the ammo readout | LOW | | KEEP | - |
| Stamina ring | `ql_feat_stamina.vjs` | color/angle the stamina charges | LOW | | KEEP | - |
| Clean damage numbers | `ql_feat_damagenumbers.vjs` + `qol_damage_fountain_full.vcss` | restyle floating damage text | LOW | PAINT | IMPROVE | - |
| Damage-impact reticle | `ql_feat_damageimpact.vjs` | hit-feedback on the crosshair | LOW | | KEEP | - |
| Combat-status overlay | `ql_feat_combatstatus.vjs` | in-combat / regen indicator | LOW | | KEEP | - |
| Target shapes / red diamond | `ql_feat_targetshapes.vjs` | restyle the lock-on target marker | MOD | | IMPROVE | - |
| Signature cooldown flash | `ql_feat_sigflash.vjs` | flash the signature when it comes up | MOD | | IMPROVE | - |
| Keyboard binding overlay | `ql_feat_keyboard.vjs` | on-screen key glyphs | LOW | | KEEP | - |
| Zipline boost overlay | `ql_feat_zipboost.vjs` | show zipline boost window | LOW | | IMPROVE | - |
| Self low-HP warning | `ql_feat_colorwarnings.vjs` (self) | tint own health bar at low HP | LOW | | KEEP | ✓ |
| Enemy/ally HP color warnings | `ql_feat_colorwarnings.vjs` (enemy/ally) | tint top-bar HP bars at a threshold | **HOT** | | KEEP | ✓ |

**Build notes.**
- **colorwarnings is the instructive one.** QOL derives HP% from **bar pixel height**
  (`actuallayoutheight` per bar per tick = a forced synchronous reflow), *and* reads that
  property twice more per bar as arguments to a debug logger that early-returns because the
  debug flag is off (pure wasted reflows, `:637/639`). grimhud already does this right: read the
  bound `HeroHealth.value` (0..1) and the `{i:health}`/`{i:maxHealth}` labels
  (`grimhud_core.vjs:163,199`). Never re-introduce geometry-derived state.
- **damage numbers**: the fountain CSS animates `background-size/position/image` (repaint-forcing)
  on many concurrent one-shot panels; in a teamfight that is the mod's biggest GPU cost. Keep the
  feature, but animate `transform`/`opacity` only, and make any heavy variant opt-in with a
  perfTier label.
- **targetshapes** refreshes at 60 ms (effectively 5 Hz) with two uncached full-root class
  walks; **sigflash** degrades from ~1 Hz to every tick when the signature panel is absent.
  Both are fine ideas; port through the cache + a proper backoff, not a raw re-walk.
- **zipboost** hardcodes the boost duration to 32 s (rot). Anchor to a true source if one exists,
  else version-stamp the constant and treat it as best-effort.

---

## 4. Minimap timers + objective tracking

The single largest QOL feature (`rejuvtimers` ~1300 lines) and one of its rot-prone data
tables. grimhud has a cleaner objective-timer overlay but with placeholder cadence.

| Feature | QOL | What it does | CPU | GPU | Verdict | gh |
|---|---|---|---|---|---|---|
| Rejuv + bridge-buff countdown | `ql_feat_rejuvtimers.vjs` | HUD + minimap countdown to rejuv/buff | MOD | BLUR | IMPROVE | ~ |
| Objective spawn timers | (grimhud) | mid-boss / urn / rejuv ETA card | LOW | | NEW | ~ |
| Crate overlay | `ql_minimap_crate_data.vjs` + `ql_feat_minimapruntime.vjs` | draw crate positions on the minimap | dead | | CUT | - |
| Rem-tunnel overlay | `minimap_..._tunnels` texture | draw a map-specific tunnel hint | LOW | | EVALUATE | - |

**Build notes.**
- **rejuvtimers** adds most of the mod's extra `world-blur` panels (`citadel_hud_top_bar.vcss`,
  7 blurred panels on `#BuffHUD`/`#RejuvHUD`). Keep the countdown value; drop or opt-in the blur;
  anchor the cadence to an observed spawn edge, not an open-loop stopwatch.
- **crate overlay is dead**: it needs a map key, but `ResolveMinimapCrateOverlayMapKey()` returns
  `""` because `Game.GetMapInfo` is absent (`ql_feat_minimapruntime.vjs:75`), and the crate table
  is stubbed to one map. `CUT` the feature and the 41 KB `ql_minimap_crate_data.vjs`.
- grimhud's objective card (`grimhud_core.vjs:218`) is the better foundation. Its
  `OBJECTIVE_CADENCE` is version-stamped but **unverified placeholder timing**; correct it against
  a real match clock before shipping enabled.

---

## 5. Audio reminders + announcers

A content pipeline as much as a feature: timed buff/objective voice reminders plus a swappable
announcer corpus. This is where the `morphic` vsnd_c minting path earns its keep.

| Feature | QOL | What it does | CPU | Verdict | gh |
|---|---|---|---|---|---|
| Timed buff/objective reminders (DL4D) | `ql_feat_legacyaudiopassive.vjs` + core | fire a voice cue N sec before an event | LOW | IMPROVE | - |
| Announcer voices (4) x volumes (3) | `.vsnd_c` corpus + 1 `.vsndevts_c` | pick an announcer voice + loudness | EVT | KEEP | - |
| 5 user-fillable announcer slots | custom-announcer slots | drop your own announcer audio | EVT | KEEP | - |
| Custom shopkeeper VO | announcer path | replace shopkeeper lines | EVT | EVALUATE | - |

**Build notes.**
- Reminders fire via `$.DispatchEvent("PlaySoundEffect", name)` against a `.vsndevts_c` binding.
  The timing table (`DL4D_REMINDER_EVENTS`) is rot-prone; version-stamp it and prefer an observed
  anchor. This overlaps grimhud's existing sound-swap / vsnd_c minting (`soundswap`,
  `vsnd-c-minting`), so the corpus is buildable on the Rust side, no .NET.
- Announcer slots are the proven "asset-slot" pattern (same as QOL's custom announcers). Good
  candidate for a Grimoire content picker, but scope it after the visual core ships.

---

## 6. Build manager + purchases

The Airheart persistence hijack lives here. Pillar A deletes the machinery; the build save/load
value survives as real Grimoire storage.

| Feature | QOL | What it does | CPU | Verdict | gh |
|---|---|---|---|---|---|
| Build save/load | `ql_feat_buildsave.vjs` / `ql_feat_buildload.vjs` | save/restore item builds | EVT | REBUILD | - |
| Build share token | (bit-packed base64url codec) | paste a build/preset to another user | EVT | KEEP | - |
| Airheart storage state machine | `ql_feat_buildbridge.vjs` + core (~3000 ln) | hijack a hero's shop UI as a KV store | EVT | CUT | - |
| Recent purchases + per-hero popups | `ql_feat_recentpurchases.vjs` + `ql_recent_purchases_data.vjs` (375 KB) | track and surface recent item buys | **HOT** | IMPROVE | - |
| Hero-shop / item-slot styling | `ql_feat_heroshop.vjs` | restyle the shop + item slots | LOW | KEEP | - |

**Build notes.**
- **CUT the entire Airheart hijack** (hero swap, shop puppet, corrupt-repair build deletion,
  payload size cliff, the forever `buildRequestLoop`). Persistence is a Grimoire file baked in at
  install (Pillar A). **KEEP only the bit-packed token codec**, repurposed for *sharing* presets,
  never for storage.
- **recentpurchases** is Tier A (a swarm of uncached per-purchase walks, `:742/336/108`) and
  drags in a 375 KB always-parsed icon map. If ported: memoize per-purchase lookups, gate on
  shop-open, and lazy-load the icon data with the feature (or omit it at build when disabled).

---

## 7. Misc / cosmetic

| Feature | QOL | What it does | CPU | Verdict | gh |
|---|---|---|---|---|---|
| Images-in-chat | `ql_feat_chatimg.vjs` | render remote image URLs inline in chat | LOW | REBUILD | - |
| Custom mouse cursor | `ql_feat_mousecursor.vjs` | swap the cursor image | LOW | KEEP | - |
| On-death arcade bridge | `ql_feat_ondeatharcade.vjs` | signal a separate minigame mod on death | **HOT** | CUT | - |
| In-settings arcade minigames | `ql_settings.vjs` (~835 ln) | Minesweeper/flappy/aim-trainer in the menu | n/a | CUT | - |
| Perf overlay | `ql_perf_overlay.vjs` | QOL's own FPS overlay | LOW | REBUILD | ~ |
| Legacy cooldowns | `ql_legacy_cooldowns.vjs` | (dead) hardcoded `return false`, loops per panel | dead | CUT | - |

**Build notes.**
- **images-in-chat is a security hole**: it renders arbitrary remote URLs from other players'
  chat with no allowlist, size cap, or proxy (an IP-grabber + shock-image vector). `REBUILD` only
  with an allowlist + size cap + click-to-load gate, or leave it out.
- **on-death arcade** is Tier S (3 uncached root walks per tick *while alive*, the common case)
  and only bridges to a separate minigame mod. `CUT`.
- **minigames** are ~835 lines parsed into every match for a menu toy. `CUT` from the HUD bundle
  entirely.
- **perf overlay**: replace QOL's with grimhud's honest self-measured per-feature meter
  (`docs/grimhud-architecture.md` 3.6), gated behind `DEBUG_OVERLAY`.
- **legacy cooldowns** self-loops forever on every buff/cooldown panel to `return false`. Pure
  dead work that scales with on-screen buff count. `CUT`.

---

## 8. grimhud-only (NEW): things QOL never had

These are the "surpass on day one" wins, mostly already in `grimhud_core.vjs`:

| Feature | What it does | gh | Why it matters |
|---|---|---|---|
| HUD-desync self-test | on-screen `N/M` per-selector hit/miss marker (`:466`) | ✓ | when Valve renames a panel, the mod *tells the user* instead of silently half-breaking |
| Soul lead + team pace | net-worth delta + true souls/min, rendered into the native clock pill (`:315`) | ✓ | the economy readout QOL never showed; locale/patch durable |
| Allies-down respawn list | consolidated downed-ally ETAs from bound `{d:respawn_time}` (`:388`) | ✓ | read at a glance vs scanning the top bar; true bindings, no timing table |
| Objective ETA card | mid-boss/urn/rejuv countdown (`:218`) | ~ | cleaner than rejuvtimers; needs verified cadence |
| Self-measured perf HUD | grimhud's own tick cost, per feature | - | proves the mod is not the FPS cost (the honest inverse of QOL's perf overlay) |
| Declarative settings + perfTier labels | one schema drives UI; each toggle shows its cost | - | users throttling for FPS know what each feature costs |

---

## 9. Cost hot-list (re-implement with care)

Ranked by the per-frame cost the original paid. These are the features to route through the v2
caching Resolver + FrameContext + gate-first dispatcher, or the FPS regression comes back:

1. **Quickbuy queue cost summary** (`hud_quickbuy_total_summary.vjs:730`) : 20 Hz, uncached,
   runs even when inactive. The single worst loop. (Not a user-facing "feature" so much as shop
   polish; port only if needed, and cached.)
2. **Bottom-bar AP recolor** (`ql_feat_bottombar.vjs:93`) : ~10 near-root class walks per 5 Hz
   tick, before its gate.
3. **On-death arcade** (`ql_feat_ondeatharcade.vjs`) : 3 uncached root walks/tick while alive.
   Mitigated by `CUT`.
4. **colorwarnings** (`ql_feat_colorwarnings.vjs`) : per-bar `actuallayoutheight` reflow +
   wasted debug reflows. Mitigated: grimhud reads the bound value instead.
5. **recentpurchases** (`ql_feat_recentpurchases.vjs`) : per-purchase uncached walks + 375 KB
   icon map.
6. **minimapruntime** (`ql_feat_minimapruntime.vjs:380`) : up to 2 uncached full-root walks/tick.
7. **compassLoop** (core, 20 Hz) : minimap rotation at 4x base rate. Fold into the one dispatcher
   with a cadence.

GPU: the **damage fountain** (background-* repaints in fights) and the **~13 extra world-blur
panels** (rejuvtimers + quickbuy summaries). Keep blur opt-in and on the perf meter.

---

## 10. Suggested build order

Ship a small, high-confidence core on the v2 runtime, prove it in-game, then widen. Do **not**
clone breadth-first.

- **Tier 0 (foundation, already mostly done):** reposition/scale/opacity catalog, self + ally +
  enemy HP color warnings, unspent nudge, soul lead / pace, respawn list, HUD-desync self-test.
  All cheap, all true-binding, all shipped or partial in grimhud.
- **Tier 1 (high value, low cost):** ammo, stamina, damage-impact, combat-status, nicknames,
  keyboard glyphs, hero-shop styling, custom cursor. Straight ports at LOW cost.
- **Tier 2 (value, needs care):** minimap restyle/zoom (IMPROVE the uncached walk), objective
  timers (verify cadence), stat bonuses, unsecured souls, damage numbers (opt-in heavy variant),
  target shapes + sigflash (cache + backoff).
- **Tier 3 (content pipeline):** audio reminders + announcer slots (Rust vsnd_c mint), build
  save/load (Grimoire storage) + share token.
- **Cut, do not port:** Airheart hijack machinery, on-death arcade, in-settings minigames, crate
  overlay, legacy cooldowns, dead minimap config keys, statlocker.gg deep-links (or consolidate
  to one). Rebuild-or-omit: images-in-chat (security).

The scorecard: of QOL's ~40 features, roughly **28 KEEP/IMPROVE**, **5 NEW** (grimhud), **7 CUT**,
**2 REBUILD** (images-in-chat, perf overlay). The value is real; the debt is in the ~3000-line
persistence hijack, the pixel/locale scraping, and a handful of uncached hot loops, all of which
the v2 foundation deletes by construction.
