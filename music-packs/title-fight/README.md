# Title Fight music kit

This folder is a planning/metadata scaffold for a Deadlock music kit inspired by
Title Fight.

It does not download or store copyrighted audio. Put audio you own or are
licensed to use under `source_audio/`, matching the filenames in `manifest.json`.
The YouTube Music IDs in `source-tracks.json` are only references for identifying
the intended tracks.

## Layout

- `source-tracks.json`: resolved YouTube Music metadata for candidate Title Fight
  tracks.
- `manifest.json`: expanded Deadlock event mapping for the Title Fight kit.
- `source_audio/`: user-provided WAV/FLAC/MP3 files, ignored by git.
- `compiled/`: generated `.vsnd_c` files, ignored by git.

## Build

`vpkmerge-core/examples/build_music_pack.rs` is the manifest-driven builder. For
each entry it reads the local `source_audio` file, trims/loops it to the target
length and fades it (ffmpeg), mints a `.vsnd_c` from the trimmed MP3 using a donor
music container, retargets the named soundevent(s) at the new `.vsnd` path, and
packs every clip plus the edited `.vsndevts_c` into one addon VPK.

```bash
PAK="$HOME/.local/share/Steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk"

# preview the plan (touches no audio)
cargo run --release --example build_music_pack -- \
  music-packs/title-fight/manifest.json "$PAK" /tmp/title_fight_dir.vpk --dry-run

# build the addon VPK
cargo run --release --example build_music_pack -- \
  music-packs/title-fight/manifest.json "$PAK" /tmp/title_fight_dir.vpk
```

Entries whose `local_audio` is missing are skipped with a warning, so a partial
`source_audio/` still builds. Default donor is `sounds/music/music_menu_lp.vsnd_c`
(a stock MP3 `CVoiceContainerDefault`); override with `--donor <vpk_entry>`. The
soundevent file defaults to `soundevents/music.vsndevts_c` (override per entry
with `soundevent_entry`, or pack-wide with a top-level `default_soundevent`). To
test in-game, drop the output as `pakNN_dir.vpk` into
`Steam/steamapps/common/Deadlock/game/citadel/addons/`.

`download_songs.py` creates `source_audio/silence.wav` locally for silent layer
replacements, and lists it as a generated utility source.

Current coverage includes:

- title/menu, match intro, hideout/search/wait, postgame, pause, shop, zipline,
  and base-attack loops
- win/loss/death/respawn, rejuvenator, objective, killstreak, reveal-rank, vote,
  and tutorial stingers
- brawl titles/draft/round/match music
- idol pickup/timer/return/announce music
- hideout arpeggiator and REM-room ambience music
- world music-like emitters for hotel, choir, teleporter, and vault idle loops
