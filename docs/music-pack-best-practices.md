# Music pack best practices (from the Deadlock soundevent schematics)

Companion to [deadlock-music-pack-research.md](./deadlock-music-pack-research.md),
which maps *which* sound each music event plays. This doc reads the *schematics*
of those events (the soundevent fields, the inheritance, the loop metadata) and
draws conclusions about how to make a custom pack (e.g. `music-packs/title-fight`)
sound continuous and intentional instead of choppy.

Source: decoded `soundevents/music.vsndevts_c` and `soundevents/base/music.vsndevts_c`
from the live pak01 (`vpkmerge soundevents <entry> --from-vpk pak01_dir.vpk`).

## How Valve's music system is wired

Every music event inherits from a small set of templates and a typed playback
behavior. The continuity we want is almost entirely *built into these fields*; a
custom pack inherits it for free **as long as it doesn't fight it.**

### The base templates (the mix contract)

```
Base.Music.2d      type=citadel_default_2d  mixgroup=Music     voice_layer=game  volume=-10  volume_fade_out=2.0
Base.Music.Queue   type=citadel_music       mixgroup=Music     voice_layer=game  volume=-10  volume_fade_out=2.0  preload_vsnds=1
Base.Stingers.2d   base=Base.Music.2d                          volume=-1   volume_fade_out=0.5
Base.Stingers.KillStreaks.2d  base=Base.Stingers.2d  mixgroup=Music-KS  volume=2  delay=0.1
```

Conclusions:

- **Beds and stingers live on different mix buses.** Looping music is on
  `Music`; killstreak stingers are on `Music-KS`. That is *why* a stinger can
  punch through without cutting the bed: they are not the same channel. A pack
  doesn't set this (it's inherited), but it tells you the intended loudness
  relationship: **beds sit low (-10 dB), stingers sit hot (-1 to +2 dB).**
- **`volume_fade_out` is the state-transition crossfade**, not a loop fade. Beds
  get 2.0 s, stingers get 0.5 s. When the engine moves from one music state to
  the next (menu -> title, hideout -> match), it fades the outgoing event over
  this many seconds while the incoming one fades in. **This is the continuity
  the pack is missing, and it is already wired** -- our job is to not cancel it.
- **`event_use_music_convar` + `event_voice_layer`** route music under the music
  volume slider and duck it under VO. Inherited; leave alone.

### The loop metadata (the seamless-loop contract)

The looping events carry explicit, per-clip loop geometry tuned to Valve's
original audio:

```
Music.Hideout         sync_bpm=[155]  startpoint=[0.0]    endpoint=[43.355]   type=citadel_music_hideout
Music.Hideout.Search  sync_bpm=[155]  startpoint=[0.774]  endpoint=[75.096]
Music.Hideout.Build                   startpoint=[0.0]    endpoint=[5.714]    syncpoints=[1.548]
Music.Brawl.Draft     sync_bpm=[95]                                           type=citadel_music_brawl_draft
Music.Brawl.RoundStartN               startpoint=[0.0]    endpoint=[10.104]
```

(`startpoint`/`endpoint`/`sync_bpm`/`syncpoints` are **float arrays**, e.g.
`[43.355]`, not scalars.)

Conclusions:

- **`endpoint` is the loop-back point in seconds.** The engine plays to
  `endpoint`, then jumps to `startpoint` -- not to the end of the file. It loops
  on a *musical* boundary that Valve measured for their clip.
- **`sync_bpm` lets the engine quantize state changes to the beat** so a
  transition or a layer swap lands on a downbeat instead of mid-phrase.
- The transition/round events (`MatchIntro.*`, `Brawl.RoundStartN`,
  `Brawl.Titles`) use `startpoint`/`endpoint` to clip a one-shot to its musical
  in/out so it hands off cleanly to the loop that follows.

### Layered (stem) events

`Music.Hideout` and `Music.Brawl.Draft` are multi-stem: `vsnd_files_play_base` /
`_play_base_fx` / `_play_high` / `_play_low` (hideout) and `vsnd_files_draft` /
`_draft_fx` / `_draft_timer` (draft) play **simultaneously**, mixed by intensity
(`afk_vol_floor_*`, `layer_draft_*_vol_offset_db`, the `play_button_*_layer`
velocities). They are a vertical re-orchestration of one piece, all at the same
tempo and key. A pack must fill *every* stem field of a layered event with cuts
**from the same track at the same tempo**, or the layers beat against each other.

## Why the current title-fight pack sounds choppy

The build (`vpkmerge-core/examples/build_music_pack.rs`) does the wiring
correctly: it loads Valve's `music.vsndevts_c` from pak01 and only retargets
`vsnd_files`, sets `vsnd_duration`, and applies the manifest's
`soundevent_fields`. So **all the inheritance above is preserved.** The
choppiness comes from two things the build does to the *audio*, plus track
selection:

### 1. Loop clips are faded to silence at their tail (the big one)

`prep_audio` applies `afade=t=out` over the last `fade_out_seconds` of **every**
clip, including loops, after `-stream_loop -1` fills the clip to
`target_seconds`. So a 90 s "loop" ramps to silence at 87-90 s, then the engine
hard-jumps back to the head. **Result: a dip to silence on every loop iteration**
-- the textbook choppy/pumping loop. 32 of the title-fight loop entries do this
(`main_menu`, `title_screen`, `postgame`, `hideout_*`, `brawl_draft`,
`base_attack`, `shop*`, `idol_*`, `pause_music`, ...).

Valve's loops never fade to silence. The bed plays flat and lets
`volume_fade_out` (the inherited 2 s state crossfade) handle the *only* fade,
which fires when the music *state changes*, not every loop.

**Fix:** for `loop: true` entries, do **not** bake an `afade` out (and usually
not an `afade` in either). Fade only one-shots/stingers. Let the engine's
`volume_fade_out` do the inter-state fade. If a hard seam is audible at the loop
point, fix it with a seamless loop (below), not a fade to silence.

### 2. We keep Valve's loop geometry against a differently-structured track

We swap in a Title Fight track but leave `endpoint`/`startpoint`/`sync_bpm` at
Valve's values (e.g. Hideout still loops back at 43.355 s, still quantizes to
155 BPM). Our track is neither 155 BPM nor phrased to loop at 43.355 s, so the
loop-back lands mid-phrase and any beat-quantized transition snaps to a grid the
audio doesn't share. The build sets `vsnd_duration` but never touches the loop
geometry.

**Fix (two options):**
- *Cheap:* prepare each loop clip so its **full trimmed length is itself a clean
  loop** (cut on bar lines, match head/tail), and override `endpoint` to our clip
  length with `startpoint=0`, clearing/relaxing `sync_bpm`. Requires a
  `set_double_array_field` helper on `SoundEvents` (mirror of the existing
  `set_string_array_field`; `set_event_field` only writes scalars and would write
  `endpoint: 43.0` instead of `endpoint: [43.0]`, which the engine won't read).
- *Better:* author real loop points per track (bar-aligned `startpoint`/`endpoint`,
  optional equal-power crossfade across the seam) and set them per entry in the
  manifest.

### 3. Track-fit problems ("some sounds just don't fit")

- **Win and Loss play the same track** (`your_pain_is_mine_now` on both
  `Music.Match.Win` and `Music.Match.Lose`). Opposite outcomes should not share a
  cut. Pick a triumphant section for Win and a desolate one for Loss.
- **One track stretched across a whole phase reads as monotony, not theme.**
  `rose_of_sharon` covers all four hideout states *and* both shops (x14);
  `hypnotize` covers all of brawl (x15). Continuity wants a *shared palette*, not
  a single looped cut -- vary the section per state (build vs search vs wait) so
  movement between states is audible.
- **Stingers need a transient onset.** A stinger cut from a sustained passage has
  no attack and reads as the bed momentarily getting louder. Cut death/respawn/
  killstreak stingers on a hit/downbeat with `fade_in: 0` (they already use
  `fade_in: 0.0`, good -- the issue is *where in the song* the cut starts).

## Concrete remediation plan (prioritized)

1. **Stop fading loops to silence.** In `prep_audio`, gate the `afade` filters on
   `!e.loop_clip` (or add a manifest `seamless` flag). This alone removes the
   per-loop silence dip on 32 entries. *(Build change, no manifest edits.)*
2. **Set real loop geometry.** Add `set_double_array_field` to `SoundEvents`;
   accept `loop_start` / `loop_end` / `sync_bpm` in `soundevent_fields` (as
   arrays) and write them. Prep loop clips bar-aligned so head==tail energy.
3. **Fix the win/loss collision and de-duplicate phase beds.** Re-assign
   `source_track_key` so Win != Loss, and give hideout build/search/wait distinct
   sections (still from a shared track if you want the palette, but different
   timestamps via `start_secs`).
4. **Fill every stem of the layered events from the same track/tempo** (hideout
   base/high/low, draft/draft_fx/draft_timer). Today the `_fx`/`high`/`low`/timer
   stems are pointed at `silence`, which throws away the intensity system -- fine
   as a v1, but the layered swell is part of "high quality."
5. **Set per-class `volume`** in `soundevent_fields` to match the schematic
   intent (beds quiet, stingers hot) so stingers accent rather than the bed
   appearing to jump.

Items 1 and 3 are the highest ratio of perceived-quality to effort and need no
new code beyond the `afade` gate.

## Implemented (2026-06-16)

Done for `music-packs/title-fight`:

- **Loop beds no longer fade to silence.** `prep_audio` gates the `afade` filters
  on `!e.loop_clip`. The 32 affected loops now play flat and rely on the inherited
  `volume_fade_out` for the music-state crossfade.
- **Loop window follows our clip.** New `SoundEvents::set_double_array_field`
  (the loop-geometry fields are float arrays); `build_music_pack` auto-sets
  `startpoint=[0]` / `endpoint=[clip length]` on every loop event so the engine
  loops our boundary, not Valve's. Across randomized loop choices it takes the
  longest (`endpoint = max`). A manifest `soundevent_fields` `endpoint`/`startpoint`
  array still overrides with hand-authored bar points. (`sync_bpm` is still
  retained; removing it needs a delete API, deferred.)
- **Win/Loss now read as opposites** (research-driven, `docs` agent over the Title
  Fight catalog). `Music.Match.Win` randomizes triumphant cuts
  (`numb_but_i_still_feel_it`, `secret_society`, `shed`); `Music.Match.Lose`
  randomizes somber ones (`your_pain_is_mine_now`, `rose_of_sharon`,
  `murder_your_memory`).
- **Phase variety via the randomizer** (multiple `vsnd_files` -> engine random
  pick, the Silver-mod technique). Each hideout loop (base stem / search / wait),
  the brawl draft loop, and both shops now offer 2-3 dreamy-palette choices instead
  of one stretched track. Shops moved off `rose_of_sharon` (which was shared with
  the hideout) onto `chlorine` / `blush`.

### Match-intro continuity (the loading-screen -> spawn-in arc)

The `Music.MatchIntro.*` sequence (`Connecting` -> `HeroReveal` ->
`MatchStart.King`/`Mother`) is its own subsystem and was the choppiest part
in-game. Two defects, both now fixed:

- **`Connecting` was a 20s one-shot.** Valve's `Connecting` is a **269s bed**: it
  plays continuously through the whole variable-length connect/load screen. A 20s
  one-shot plays then drops to silence for the rest of the load -> dead air. Fixed:
  `Connecting` is now a 150s **loop** (`loop: true`), so it fills any load duration.
- **All four phases were the same song at sequential offsets** (`secret_society` at
  0/20/38/54s), which assumes each phase lasts exactly the clip length (it never
  does) and makes the engine crossfade `secret_society` *into itself* at a different
  offset = a smeared, phasey restart on every transition. Fixed: the flourishes use
  **distinct tracks** (`HeroReveal` = `loud_and_clear`, `MatchStart` =
  `numb_but_i_still_feel_it`), so each transition is a clean musical change and the
  intro builds energy (bed -> punchy reveal -> triumphant spawn-in drop).

General rule: a phase whose duration is variable (anything gated on load/connect
time) must be a **loop**, never a fixed one-shot, or it goes silent. And never
crossfade a track into itself across a state change; adjacent music states that
crossfade should be different audio (or the same continuous loop, untouched).

Open / next: per-track bar-aligned loop points (cut beds on the bar so even the
loop *seam* is clean, not just the silence dip removed); a `sync_bpm` set/clear so
beat-quantized transitions match our audio; tune stinger in-points to land on a
transient. All of these want an in-game listen pass first.
