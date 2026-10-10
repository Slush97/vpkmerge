# vpkmerge-mcp

An MCP server that lets an AI client build Deadlock mods with the vpkmerge engine. It runs on your machine, reads your own Deadlock install, and writes every mod it builds to a staging folder. It never copies anything into the game.

You ask for "make Wraith's gun sound like this mp3" or "recolor Paige's ult purple", the model looks up the real event or texture names, builds an addon VPK, and tells you where it put it. Installing it is still your move.

## Run it

```bash
cargo build --release -p vpkmerge-mcp   # target/release/vpkmerge-mcp
```

Point any MCP host at the binary. It speaks stdio.

```json
{
  "mcpServers": {
    "vpkmerge": {
      "command": "/path/to/vpkmerge/target/release/vpkmerge-mcp",
      "env": {
        "VPKMERGE_DOCS_DIRS": "/path/to/vpkmerge/docs:/path/to/deadlock-modding-knowledge-base/guides"
      }
    }
  }
}
```

For Claude Code: `claude mcp add vpkmerge /path/to/vpkmerge/target/release/vpkmerge-mcp`.

The Workbench app needs none of this. It ships the binary as a Tauri sidecar and registers it as the built-in server `vpkmerge`. In the workbench repo, `pnpm sidecar` builds it from this checkout (`../vpkmerge`, or `VPKMERGE_DIR`), and `pnpm tauri dev` / `pnpm tauri build` run that for you. To pass it env vars from the app, add `"vpkmerge": { "env": { ... } }` to the app's `mcp.json`.

It finds Deadlock by itself by reading Steam's `libraryfolders.vdf`, so extra library drives work. If it can't, set one of these.

| Env | What |
|---|---|
| `DEADLOCK_GAME_DIR` | The Deadlock install folder (the one with `game/citadel/pak01_dir.vpk` in it). |
| `DEADLOCK_PAK` | Or the `pak01_dir.vpk` itself. |
| `VPKMERGE_STAGING_DIR` | Where built mods go. Default `~/.local/share/vpkmerge/staging`. |
| `VPKMERGE_DOCS_DIRS` | Extra markdown folders for the docs tools, separated like `PATH`. |

## Tools

Look things up (read-only):

- `game_status`: was the game found, where is staging, which `pakNN` addon slots are taken, which is free.
- `list_heroes`: display name to codename, and who `recolor_hero_vfx` supports.
- `browse_sounds`: search hero sounds, shared sounds (melee, parry, items, UI) and voice lines. Gives the exact event name.
- `browse_textures`: search icons, hero cards, skin and effect textures. Gives the exact entry path, optionally PNG thumbnails.
- `list_hero_textures`: every texture the hero's in-game body model samples, per material: slot, size, format, alpha range, uniform placeholders (1x1 constants), vertex-color materials, textures shared between materials, and what each pbr.vfx slot's channels hold.
- `map_texture_regions`: split one of those textures into regions by bone (body areas), UV island or mesh part. Returns the largest regions with coverage, texel bounds, patch count, skin bones, and how much of each is shared with another region (mirrored left/right halves). Writes `texture.png` (RGB), `texture-alpha.png` when alpha is not opaque, and an overlay with region ids drawn on; `select` bakes a full-size white-on-black mask. Files land in `<staging>/textures/<codename>/<texture>/`.
- `inspect_mod`: what is in a VPK, what it collides with, and whether each `.vdata_c` it ships is outdated versus the game.
- `preview_hero`: the hero as a `.glb` with a mod VPK applied, opening in the menu pose, for the host app to show.
- `list_hero_animations` / `preview_hero_animation`: the hero's in-game animations (idles, movement, abilities, emotes), each exported as a small skeleton-only `.glb` to play on the preview.
- `search_docs` / `read_doc`: the modding knowledge base.

Build to staging:

- `swap_hero_sound`: replace a whole sound event with an MP3. Handles randomizer pools, and `heroOnly` for clips other heroes also play.
- `swap_sound_clip`: replace one clip.
- `make_sound_louder`: shift an MP3 by N dB without re-encoding.
- `replace_textures`: replace textures with PNGs, optionally only inside a mask. Material textures keep their original alpha (a data channel there) unless told otherwise; UI art under `panorama/` takes the PNG's.
- `recolor_hero_vfx`: one hue or a rainbow across a hero's ability effects.
- `merge_mods`: several VPKs into one.

Install:

- `open_in_grimoire`: opens the Grimoire mod manager's import dialog with a staged VPK. The user confirms there. Finds Grimoire through `GRIMOIRE_BIN`, then `PATH`, then the default install location.

Heroes go by display name everywhere ("Grey Talon" works, you never type `orion` or `archer`). Audio in is MP3, images are PNG.

## The knowledge base

The nine guides in `docs/deadlock-modding/` and the two harness skills are compiled into the binary, so the docs tools work with no setup. `VPKMERGE_DOCS_DIRS` adds folders from disk on top. Docs are split at headings and ranked with BM25, and the built-in guides outrank the extra folders because those tend to be working notes.

## Not here yet

- No `install_mod`. The model hands you the staged path and the next free slot.
- No automatic volume matching. `make_sound_louder` takes a dB number.
- Stdio only.

## Tests

```bash
cargo test -p vpkmerge-mcp
DEADLOCK_PAK=/path/to/citadel/pak01_dir.vpk cargo test --release -p vpkmerge-mcp --test live
```

The first runs anywhere, including a real stdio handshake against the binary. The second builds real mods from your install into a temp folder.
