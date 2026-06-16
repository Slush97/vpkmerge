# Deadlock music pack research

This scan was run against the local Deadlock install at:

`~/.local/share/Steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk`

## File families

- Main archive: `game/citadel/pak01_dir.vpk`
- Addon override location: `game/citadel/addons/pakNN_dir.vpk`
- Dedicated music payloads: `sounds/music/**/*.vsnd_c`
- Music sound events: `soundevents/music.vsndevts_c`
- Music arpeggiator events: `soundevents/music_arpeggiator.vsndevts_c`
- Hideout ambience music events: `soundevents/ambience/ambience_hideout.vsndevts_c`
- Base music template: `soundevents/base/music.vsndevts_c`
- Music state data: `scripts/music_data.vdata_c`
- Music soundstacks:
  - `soundstacks/soundstack_citadel_music.vsndstck_c`
  - `soundstacks/soundstack_citadel_music_hideout.vsndstck_c`
  - `soundstacks/soundstack_citadel_music_title.vsndstck_c`
  - `soundstacks/soundstack_citadel_music_brawl_draft.vsndstck_c`
  - `soundstacks/soundstack_citadel_music_urn.vsndstck_c`
  - `soundstacks/soundstack_citadel_music_urn_drop.vsndstck_c`
  - `soundstacks/soundstack_citadel_music_end_game.vsndstck_c`
  - `soundstacks/soundstack_citadel_diagetic_music.vsndstck_c`

Source 2 soundevent manifests reference `.vsnd` paths, but the packed VPK entries
are the compiled `.vsnd_c` files. For example, the soundevent path
`sounds/music/music_menu_lp.vsnd` resolves to the VPK entry
`sounds/music/music_menu_lp.vsnd_c`.

## Counts from this build

- `129814` total VPK entries.
- `149` `.vsndevts_c` soundevent files.
- `140` dedicated `sounds/music/**/*.vsnd_c` payloads.
- `125` dedicated `sounds/music/**/*.vsnd` paths are referenced by decoded soundevents.
- The referenced score music comes from only three soundevent files:
  - `soundevents/music.vsndevts_c`
  - `soundevents/music_arpeggiator.vsndevts_c`
  - `soundevents/ambience/ambience_hideout.vsndevts_c`

## Main music event map

| Event | Referenced sound |
| --- | --- |
| `Music.MainMenu` | `sounds/music/music_menu_lp.vsnd` |
| `Music.Title` | `sounds/music/music_title_155bpm.vsnd` plus `vsnd_files_fx = sounds/music/music_title_fx_155bpm.vsnd` |
| `Music.Hideout.Build` | `sounds/music/music_hideout_build_155bpm.vsnd` |
| `Music.Hideout` | layered event: `vsnd_files_play_base`, `vsnd_files_play_base_fx`, `vsnd_files_play_high`, `vsnd_files_play_low` |
| `Music.Hideout.Search` | `sounds/music/music_hideout_search_155bpm.vsnd` |
| `Music.Hideout.Wait` | `sounds/music/music_search_wait_temp.vsnd` |
| `Map.Broadcast.GameStart` | `sounds/ui/match_start_01.vsnd` |
| `Music.MatchIntro.Connecting` | `sounds/music/match_intro/music_match_intro_connecting_60bpm.vsnd` |
| `Music.MatchIntro.HeroReveal` | `sounds/music/match_intro/music_match_intro_flourish_160bpm.vsnd` |
| `Music.MatchIntro.MatchStart.King` | `sounds/music/match_intro/music_match_intro_king_160bpm.vsnd` |
| `Music.MatchIntro.MatchStart.Mother` | `sounds/music/match_intro/music_match_intro_mother_160bpm.vsnd` |
| `Music.Zipline.Lp` | `sounds/music/music_silence_1s_loop.vsnd` |
| `Music.Match.Formed` | `sounds/common/null.vsnd` |
| `Music.Base.Attack` | `sounds/music/music_core_exposed_lp.vsnd` |
| `Music.Match.Win` | `sounds/music/music_stinger_game_over_win.vsnd` |
| `Music.Match.Lose` | `sounds/music/music_stinger_game_over_lose.vsnd` |
| `Music.PostGame` | `sounds/music/music_postgame_155bpm.vsnd` |
| `Gameplay.Pause.Music.Lp` | `sounds/common/null.vsnd` |
| `Stinger.Respawn` | `sounds/music/music_stinger_player_respawn.vsnd` |
| `Stinger.Respawn.Countdown` | `sounds/music/music_stinger_player_respawn_countdown.vsnd` |
| `Stinger.Death` | `sounds/music/music_stinger_player_death.vsnd` |
| `Stinger.Rejuvinator.Claimed.Friendly` | `sounds/music/music_stinger_rejuv_won.vsnd` |
| `Stinger.Rejuvinator.Claimed.Enemy` | `sounds/music/music_stinger_rejuv_lost.vsnd` |
| `Stinger.Rejuvinator.Descent` | `sounds/music/music_stinger_rejuv_drop_6s.vsnd` |
| `Stinger.Respawn.Rejuvinator` | `sounds/music/music_stinger_rejuv_won.vsnd` |
| `Stinger.Rejuvinator.Expired` | `sounds/music/music_stinger_rejuv_lost.vsnd` |
| `Stinger.CoreExposed` | `sounds/common/null.vsnd` |
| `Stinger.MidBoss.Arrived` | `sounds/music/music_stinger_mid_boss_arrived.vsnd` |
| `Stinger.Tier1.Killed.Friendly` | `sounds/music/music_stinger_t1_killed.vsnd` |
| `Stinger.Tier1.Killed.Enemy` | `sounds/music/music_stinger_t1_killed.vsnd` |
| `Stinger.Tier2.Killed.Friendly` | `sounds/music/music_stinger_t1_killed.vsnd` |
| `Stinger.Tier2.Killed.Enemy` | `sounds/music/music_stinger_t1_killed.vsnd` |
| `Stinger.Titan.Killed.Friendly` | `sounds/music/music_stinger_t3_killed.vsnd` |
| `Stinger.Titan.Killed.Enemy` | `sounds/music/music_stinger_t3_killed.vsnd` |
| `Stinger.TitanShield1.Killed.Friendly` | `sounds/music/music_stinger_generator_killed.vsnd` |
| `Stinger.TitanShield1.Killed.Enemy` | `sounds/music/music_stinger_generator_killed.vsnd` |
| `Stinger.TitanShield2.Killed.Friendly` | `sounds/music/music_stinger_generator_killed.vsnd` |
| `Stinger.TitanShield2.Killed.Enemy` | `sounds/music/music_stinger_generator_killed.vsnd` |
| `Music.Idol.Pickup.Lp` | `sounds/music/music_idol_carry_lp_141bpm.vsnd` plus `sounds/music/music_idol_carry_distant_lp_141bpm.vsnd` |
| `Music.Idol.Timer.Lp` | `sounds/music/music_idol_timer_lp_team_160bpm.vsnd` plus `vsnd_files_opponent_control = sounds/music/music_idol_timer_lp_opponent_160bpm.vsnd` |
| `Stinger.Idol.Returned.Team` | `sounds/music/music_idol_return_team.vsnd` |
| `Stinger.Idol.Returned.Opponent` | `sounds/music/music_idol_return_opponent.vsnd` |
| `Stinger.Idol.AnnounceDrop` | `sounds/music/music_idol_announce.vsnd` |
| `Stinger.KillStreak` | `sounds/music/music_stringer_kill_streak.vsnd` |
| `Stinger.KillStreak.FirstBlood` | `sounds/music/music_stinger_first_blood.vsnd` |
| `Stinger.KillStreak_01` | `sounds/music/music_stinger_ks_01.vsnd` |
| `Stinger.KillStreak_02` | `sounds/music/music_stinger_ks_02.vsnd` |
| `Stinger.KillStreak_03` | `sounds/music/music_stinger_ks_03.vsnd` |
| `Stinger.KillStreak_04` | `sounds/music/music_stinger_ks_04.vsnd` |
| `Stinger.KillStreak_05` | `sounds/music/music_stinger_ks_05.vsnd` |
| `Stinger.KillStreak_06` | `sounds/music/music_stinger_ks_06.vsnd` |
| `Stinger.KillStreak_07` | `sounds/music/music_stinger_ks_07.vsnd` |
| `Stinger.KillStreak_08` | `sounds/music/music_stinger_ks_08.vsnd` |
| `Stinger.KillStreak_09` | `sounds/music/music_stinger_ks_09.vsnd` |
| `Stinger.KillStreak_10` | `sounds/music/music_stinger_ks_10.vsnd` |
| `Stinger.RevealRank_01` | `sounds/music/music_stinger_ks_01.vsnd` |
| `Stinger.RevealRank_02` | `sounds/music/music_stinger_ks_01.vsnd` |
| `Stinger.RevealRank_03` | `sounds/music/music_stinger_ks_02.vsnd` |
| `Stinger.RevealRank_04` | `sounds/music/music_stinger_ks_03.vsnd` |
| `Stinger.RevealRank_05` | `sounds/music/music_stinger_ks_04.vsnd` |
| `Stinger.RevealRank_06` | `sounds/music/music_stinger_ks_05.vsnd` |
| `Stinger.RevealRank_07` | `sounds/music/music_stinger_ks_06.vsnd` |
| `Stinger.RevealRank_08` | `sounds/music/music_stinger_ks_07.vsnd` |
| `Stinger.RevealRank_09` | `sounds/music/music_stinger_ks_08.vsnd` |
| `Stinger.RevealRank_10` | `sounds/music/music_stinger_ks_09.vsnd` |
| `Stinger.RevealRank_11` | `sounds/music/music_stinger_ks_10.vsnd` |
| `Stinger.RevealVote` | `sounds/music/music_stinger_ks_03.vsnd` |
| `Music.Shop.Silence` | `sounds/music/music_silence_1s_loop.vsnd` |
| `Music.Shop` | `sounds/music/menu/curio_music.vsnd` |
| `Music.Shop.Secret` | `sounds/music/menu/curio_music_02.vsnd` |
| `Stinger.Brawl.Overtime.Announce` | `sounds/music/brawl/music_brawl_overtime_95bpm.vsnd` |
| `Music.Brawl.Round.Won` | `sounds/music/brawl/music_brawl_round_won_95bpm.vsnd` |
| `Music.Brawl.Round.Lost` | `sounds/music/brawl/music_brawl_round_lost_95bpm.vsnd` |
| `Music.Brawl.Titles` | `sounds/music/brawl/music_brawl_titles_117bpm-95bpm.vsnd` |
| `Music.Brawl.Draft` | layered event: `vsnd_files_draft`, `vsnd_files_draft_fx`, `vsnd_files_draft_timer` |
| `Music.Brawl.RoundStart1` | `sounds/music/brawl/music_brawl_round_1_start_95bpm.vsnd` |
| `Music.Brawl.RoundStart2` | `sounds/music/brawl/music_brawl_round_2_start_95bpm.vsnd` |
| `Music.Brawl.RoundStart3` | `sounds/music/brawl/music_brawl_round_3_start_95bpm.vsnd` |
| `Music.Brawl.RoundStart4` | `sounds/music/brawl/music_brawl_round_4_start_95bpm.vsnd` |
| `Music.Brawl.RoundStart5` | `sounds/music/brawl/music_brawl_round_5_start_95bpm.vsnd` |
| `Music.Brawl.Match.Won` | `sounds/music/brawl/music_brawl_match_won_95bpm.vsnd` |
| `Music.Brawl.Match.Lost` | `sounds/music/brawl/music_brawl_match_lost_95bpm.vsnd` |

## Layered music details

`Music.Hideout` is a custom `citadel_music_hideout` event. It has separate clip
fields:

- `vsnd_files_play_base = sounds/music/music_hideout_play_base_lp_155bpm.vsnd`
- `vsnd_files_play_base_fx = sounds/music/music_hideout_play_base_lp_fx_155bpm.vsnd`
- `vsnd_files_play_high = sounds/music/music_hideout_play_high_lp_155bpm.vsnd`
- `vsnd_files_play_low = sounds/music/music_hideout_play_low_lp_155bpm.vsnd`

`Music.Brawl.Draft` is a custom `citadel_music_brawl_draft` event:

- `vsnd_files_draft = sounds/music/brawl/music_brawl_draft_95bpm.vsnd`
- `vsnd_files_draft_fx = sounds/music/brawl/music_brawl_draft_fx_95bpm.vsnd`
- `vsnd_files_draft_timer = sounds/music/brawl/music_brawl_draft_timer_95bpm.vsnd`

`soundevents/music_arpeggiator.vsndevts_c` has `Music.Arp.Hideout.Bell`, which
references:

- `sounds/music/arpeggiator/hideout_bell/music_hideout_arp_bell_14.vsnd`
- every numbered bell clip through
- `sounds/music/arpeggiator/hideout_bell/music_hideout_arp_bell_71.vsnd`

`soundevents/ambience/ambience_hideout.vsndevts_c` references hideout REM music:

- `sounds/music/music_hideout_rem_70bpm.vsnd`
- `sounds/music/music_hideout_rem_fx_70bpm.vsnd`
- `sounds/music/music_hideout_rem_fx_far_70bpm.vsnd`

## Present but not referenced by decoded soundevents

These `sounds/music` payloads exist in the VPK but were not referenced by any
decoded `.vsndevts_c` file in this build:

- `sounds/music/match_intro/music_match_start_king_160bpm.vsnd`
- `sounds/music/menu/curio_music_2.vsnd`
- `sounds/music/music_curio_christmas.vsnd`
- `sounds/music/music_curio_seasonal.vsnd`
- `sounds/music/music_curio_secret_christmas.vsnd`
- `sounds/music/music_curio_secret_seasonal.vsnd`
- `sounds/music/music_match_countdown.vsnd`
- `sounds/music/music_menu_winter24_lp.vsnd`
- `sounds/music/music_postgame_temp_155bpm.vsnd`
- `sounds/music/music_stinger_rejuv_drop_6_5s.vsnd`
- `sounds/music/music_stinger_rejuv_drop_7_5s.vsnd`
- `sounds/music/music_stinger_rejuv_drop_7s.vsnd`
- `sounds/music/music_stinger_rejuv_drop_8s.vsnd`
- `sounds/music/music_stinger_rejuv_drop_9s.vsnd`
- `sounds/music/music_teleporter_01.vsnd`

## Music-like payloads outside `sounds/music`

These are not part of the main score folder, but are worth knowing about for
custom packs:

- `sounds/ambient/soundscapes/hotel_music_blend.vsnd_c`
- `sounds/ambient/soundscapes/hotel_music.vsnd_c`
- `sounds/ambient/soundscapes/hotel_music_close.vsnd_c`
- `sounds/ambient/soundscapes/hotel_music_far.vsnd_c`
- `sounds/ambient/soundscapes/loops/hotel_music.vsnd_c`
- `sounds/ambient/soundscapes/loops/music_choir_01.vsnd_c`
- `sounds/ambient/soundscapes/teleporter_music_blend.vsnd_c`
- `sounds/ambient/soundscapes/teleporter_music_comp.vsnd_c`
- `sounds/ambient/soundscapes/teleporter_music_melody.vsnd_c`
- `sounds/abilities/cadence/cadence_anthem_music.vsnd_c`
- `sounds/abilities/cadence/cadence_lullaby_music.vsnd_c`
- `sounds/abilities/cadence/cadence_silencecontraptions_music.vsnd_c`
- `sounds/npc/neutrals/vaults/vault_music.vsnd_c`
- `sounds/world/vault/vault_music.vsnd_c`
- `sounds/world/vault/vault_music_hit_01.vsnd_c`
- `sounds/world/vault/vault_music_hit_02.vsnd_c`
- `sounds/world/vault/vault_music_hit_03.vsnd_c`
- `sounds/world/vault/vault_music_hit_04.vsnd_c`
- `sounds/world/vault/vault_music_hit_05.vsnd_c`

Only three of these were found in decoded soundevents:

- `soundevents/ambience/lanes.vsndevts_c -> sounds/ambient/soundscapes/hotel_music_blend.vsnd`
- `soundevents/ambience/lanes.vsndevts_c -> sounds/ambient/soundscapes/loops/music_choir_01.vsnd`
- `soundevents/npc/neut_vaults.vsndevts_c -> sounds/npc/neutrals/vaults/vault_music.vsnd`

## Custom pack approaches

1. Payload replacement: mint a new `.vsnd_c` and pack it at the exact original
   `sounds/music/...vsnd_c` path. This avoids editing soundevents.
2. Event retargeting: mint new clips under a custom path, edit
   `soundevents/music.vsndevts_c` or `soundevents/music_arpeggiator.vsndevts_c`
   so the event points at the new `.vsnd` path, then pack both the edited
   soundevent and compiled clips.
3. Layered-event replacement: for `Music.Hideout`, `Music.Brawl.Draft`,
   `Music.Title`, and `Music.Idol.Timer.Lp`, update every relevant `vsnd_files*`
   field, not only the plain `vsnd_files` field.

The repo already has the hard parts:

- `morphic::encode_vsnd_c` can forge `.vsnd_c` files from MP3 data using a donor
  Source 2 sound container.
- `vpkmerge_core::SoundEvents` can decode/edit/re-encode `.vsndevts_c`.
- `vpkmerge_core::pack` can write the addon VPK.
- `vpkmerge-core/examples/bake_silver_ult.rs` is the closest existing example:
  it mints custom clips, rewrites a soundevent, and packs the result.

## Repro commands

List all VPK entries:

```bash
cargo run -q -p vpkmerge-core --example listentries -- \
  "$HOME/.local/share/Steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk" \
  > /tmp/deadlock_paths.txt
```

List dedicated music payloads:

```bash
rg '^sounds/music/' /tmp/deadlock_paths.txt | sort
```

Decode the two main music event manifests:

```bash
target/debug/vpkmerge soundevents soundevents/music.vsndevts_c \
  --from-vpk "$HOME/.local/share/Steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk" \
  > /tmp/deadlock_music_events.json

target/debug/vpkmerge soundevents soundevents/music_arpeggiator.vsndevts_c \
  --from-vpk "$HOME/.local/share/Steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk" \
  > /tmp/deadlock_music_arpeggiator_events.json
```

Extract event-to-sound mappings from decoded soundevents:

```bash
jq -r 'to_entries[] | .key as $event | .value.vsnd_files? // [] | .[] |
  [$event, .] | @tsv' /tmp/deadlock_music_events.json
```

## External format reference

ValveResourceFormat/Source 2 Viewer lists `vsnd` as Sound, `vsndevts` as Sound
Event Script, `vsndstck` as Sound Stack Script, and `vpk` as Pak/package. It is
also explicitly a Source 2 VPK browser/extractor/decompiler for sounds and other
assets: https://github.com/ValveResourceFormat/ValveResourceFormat
