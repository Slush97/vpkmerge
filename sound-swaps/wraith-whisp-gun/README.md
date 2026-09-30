# Wraith whispy/futuristic gun

Replaces Wraith's gunfire with a synthesized energy-pistol "pew": an airy,
downward-zapping shot. Pairs with the gen-man Shadow Wraith model build.

## What it swaps

| Event | Pool | New sound |
|---|---|---|
| `Wraith.Wpn.Fire.First`     | 6  | `fire_first.mp3` (300 ms, fuller first shot) |
| `Wraith.Wpn.Fire.Main`      | 6  | `fire_main.mp3` (225 ms, tighter sustained) |
| `Wraith.Wpn.Whizby`         | 30 | `whizby.mp3` (doppler pass-by, mixed lower) |
| `Wraith.Wpn.Reload.Start`   | 3  | `reload_start.mp3` (power-down + charge build) |
| `Wraith.Wpn.Reload.Clip.Out`| 1  | `reload_clip_out.mp3` (servo eject) |
| `Wraith.Wpn.Reload.Clip.In` | 1  | `reload_clip_in.mp3` (servo seat + lock) |
| `Wraith.Wpn.Reload.End`     | 3  | `reload_end.mp3` (charge-up to a "ready" blip) |

`--pool all`: every clip in each pool is overridden in place under
`sounds/weapons/wraith/`. The hero's `wraith.vsndevts_c` is never edited, so this
stays merge-safe with other mods. 50 clip entries total.

Foley (whizby + reloads) is mixed 3-5 dB under the gunfire so it doesn't
out-shout the gun. Zoom in/out is left stock (deliberate).

Note: whizby clips are stored lowercased by Valve (`groupa`) while the soundevent
references `groupA`; the swap needs the case-insensitive pool lookup added to
`vpkmerge_core::swap_event_audio`.

## Sound design

Pure-numpy DSP in `synth_whisp_gun.py`, no external samples. Each shot layers:
a downward freq-sweep tonal core (the "pew"), white noise through a swept
state-variable bandpass (the airy "whisp"), a light air-breath tail, and a short
sub thump for weight. Tune the `pew(...)` params at the bottom of the script.

## Build

```bash
./build.sh          # synth -> encode mp3 -> swap both events -> merge
```

Output: `wraith_whisp_gun_dir.vpk`. Audition the raw audio by playing
`fire_first.mp3` / `fire_main.mp3` directly.

## Install (in-game test)

Drop `wraith_whisp_gun_dir.vpk` into `citadel/addons/` as `pak0N_dir.vpk`, or
register it as a Grimoire local mod, or merge it into the Shadow Wraith skin VPK
so the model + sound ship as one mod.

Loudness: synth is peak-normalized to -1 dBFS. If it sits too loud/quiet against
the rest of the game, rebuild with `--gain-db <DB>` on the soundswap calls (or
add it to `build.sh`).
