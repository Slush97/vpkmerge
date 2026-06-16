# Pokemon-style music and SFX kit

This folder is a scaffold for a Pokemon-inspired Deadlock pack. It is built
around short, high-recognition UI/gameplay sounds first, with a smaller music-kit
layer on top.

It does not download or store copyrighted Pokemon audio. Put audio you own or are
licensed to use under `source_audio/`, matching the filenames in `manifest.json`.
The YouTube Music entries in `source-candidates.json` are identification
references only and are not a download list.

## Why this pack is SFX-heavy

Deadlock has centralized soundevent files that map cleanly to Pokemon-style
feedback:

- `soundevents/ui.vsndevts_c`: cursor, select, cancel, shop, party, hero select,
  matchmaking, map ping, death notifications.
- `soundevents/gameplay.vsndevts_c`: gold, XP orbs, last-hit, rejuv, midboss.
- `soundevents/player.vsndevts_c`: low-health alert, dash, jump, parry, heal.
- `soundevents/damage.vsndevts_c`: damage, crit, poison, lethal, shield.
- `soundevents/world.vsndevts_c`: urn/objective pickup/drop/cash-in.
- `soundevents/npc/neut_vaults.vsndevts_c`: vault hits, success/fail, payout.

That means a small set of short sounds can touch a lot of the moment-to-moment
feel without replacing long music loops.

## Recommended first test

Build priority `1` entries first:

- menu cursor/select/cancel
- item get / gold get
- level up
- XP orb acquire
- map ping
- match found
- player faint/death
- low-health alert
- crit/lethal hit
- parry success

Those are the best bang-for-buck Pokemon-style changes.

## Local source layout

Expected local files live under:

```text
music-packs/pokemon/source_audio/
  music/
  sfx/
```

Generated files should go under:

```text
music-packs/pokemon/compiled/
```

## Build

`vpkmerge-core/examples/build_music_pack.rs` is the shared, manifest-driven
builder (same one the title-fight pack uses). It reads `music_entries` and
`sfx_entries`, mints `.vsnd_c` clips under `sounds/custom/pokemon/...`, retargets
each entry's `deadlock_event`/`deadlock_events` in its `soundevent_entry`, and
packs everything into one addon VPK.

```bash
PAK="$HOME/.local/share/Steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk"

# preview (lists every entry; flags ones still missing source audio)
cargo run --release --example build_music_pack -- \
  music-packs/pokemon/manifest.json "$PAK" /tmp/pokemon_dir.vpk --dry-run

# build once source_audio/ is populated
cargo run --release --example build_music_pack -- \
  music-packs/pokemon/manifest.json "$PAK" /tmp/pokemon_dir.vpk
```

Entries with no `local_audio` on disk are skipped with a warning, so you can
build the priority-1 set first and fill the rest in later.
