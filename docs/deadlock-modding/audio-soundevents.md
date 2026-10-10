# Audio and soundevents

Deadlock audio replacement is a chain of event definitions and compiled clips.
Replacing one `.vsnd_c` file is only correct when the event actually resolves to
that one clip.

## The resolution chain

```text
gameplay or UI action
    -> named soundevent in .vsndevts_c
    -> one or more .vsnd source references
    -> compiled .vsnd_c entries in a VPK
    -> encoded audio payload
```

The event can add volume, pitch, looping, randomization, and start/stop semantics
on top of the clip.

## Pools

Many hero events use randomizer pools. A single clip override then plays only
when the engine selects that member.

Two useful replacement policies are:

- **Replace all**: mint the new audio into every clip path in the pool. The event
  remains unchanged and still randomizes, but every choice has the replacement.
- **Collapse**: mint one clip and rewrite `vsnd_files` to point only to it. This
  creates a smaller mod but requires editing the soundevents resource.

Some pool members can live in another archive. Report missing members rather than
silently claiming the event was fully replaced.

## Shared clips

A hero-specific event can point to a globally shared clip. Overriding that clip
in place changes every event that references it.

For a hero-only change, collapse the hero's event to a new, unique source path and
pack the compiled clip at the matching `_c` path. For an intentionally global
change, audit the entire pool first so it does not mix shared and hero-specific
foley.

## Minting compiled sound

Deadlock expects `.vsnd_c`, not a loose WAV or MP3. The repository's proven path
uses a shipped clip as a donor container:

1. Read the donor `CTRL` data.
2. Replace rate, format, channels, sample count, duration, streaming size, and
   loop metadata for the new MP3.
3. Regenerate duration-dependent envelope metadata.
4. Rebuild the control block while preserving the surrounding resource.
5. Append the MP3 payload and pack it at the intended compiled path.

Donor-based minting is engine-verified for custom MP3 playback. It is still wise
to choose a donor with similar role and loop behavior.

## Trimming and gain

MP3 frame operations can trim and adjust Layer III `global_gain` without a full
decode/re-encode. The result is frame-snapped, so trim boundaries are approximate.
Gain changes occur in MP3 gain steps rather than arbitrary sample-level values.

Loudness matching based only on RMS is a practical starting point, not a full
perceptual loudness guarantee. Preview the result against the donor and test it
in the actual game mix.

## Duration and lifecycle

A one-shot cast event does not automatically stop when an ability ends. For music
that should follow a channeled ability, transformation, zone, or modifier, trace
the ability data for a loop or ambient sound that the engine starts and stops with
that gameplay state.

Common fields include cast/start sounds, channel or ambient loops, and expired/end
sounds. Field names are inconsistent, so recognize soundevent-shaped values as
well as keys containing the word `sound`.

## Linux and path case

Mixed-case `.vsnd` references can resolve to lowercase compiled entries. Use a
case-insensitive VPK index, but preserve the actual archive path as the override
target. Otherwise an event can work on Windows and fail to find its clip in a
Linux toolchain.

## Audio validation checklist

- Resolve the exact event and print its full clip pool.
- Determine whether each clip is shared.
- Confirm every source reference maps to a compiled VPK entry.
- Re-extract the packed `.vsnd_c` and compare the audio payload.
- Verify loop points, start/stop lifecycle, volume, and pitch in-engine.
- Test repeated plays to exercise every randomizer member.

Related: [binary KV3 soundevents](../spike-vsndevts-kv3.md) and
[music-pack research](../deadlock-music-pack-research.md).
