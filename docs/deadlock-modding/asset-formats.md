# Deadlock and Source 2 asset formats

Deadlock ships compiled Source 2 resources. The `_c` suffix means a source asset
has been compiled into an engine resource, not that it is a conventional file
with a header added.

## Format map

| Extension | Role | Typical safe operation | Common failure signal |
|---|---|---|---|
| `.vpk` | Archive and virtual filesystem | Read, merge, split, or pack entries | Mod does not mount, collision wins unexpectedly |
| `.vtex_c` | Texture, mips, image metadata | Decode; replace compatible mip chain | Garbled channels, wrong crop, missing texture |
| `.vmat_c` | Material and shader parameters | Patch existing scalar/vector/blob values | Red error material or flat incorrect shading |
| `.vpcf_c` | Particle system graph | Patch known KV3 values and references | Red X, square sprite, missing child system |
| `.vmdl_c` | Model resource and dependencies | Export; patch or replace understood mesh blocks | Missing geometry, exploded mesh, wrong scale |
| `.vnmclip_c` | Animation clip | Decode or attach to a compatible skeleton | Static pose, distorted cloth, wrong rest pose |
| `.vsndevts_c` | Soundevent definitions in binary KV3 | Edit event fields and clip lists | Event silent, wrong pool, unintended shared swap |
| `.vsnd_c` | Compiled audio container plus payload | Donor-based clip replacement | Silence, wrong duration, bad loop behavior |
| `.vdata_c` | Gameplay and roster data | Read for discovery; patch only with care | Broad gameplay behavior changes |

## Resource blocks

Compiled resources expose a block table. Common blocks include:

- `DATA`: the primary typed payload.
- `RERL`: external resource references.
- `RED2`: edit-info and compiler metadata, including texture special
  dependencies.
- `CTRL`: control data used by resources such as compiled sounds.
- Model-specific blocks such as mesh vertex/index data, animation, physics, and
  data-model metadata.

Unknown blocks are not disposable. A file can appear correct in a decoded tree
while still depending on metadata or trailing data that was dropped during
re-encode.

## Binary KV3

Materials, particles, soundevents, and other resources frequently embed binary
KeyValues3. Important properties for mod tooling:

- Versions and compression modes vary across assets.
- Typed values can use compact tag encodings rather than full stored scalars.
- Version 5 resources can contain multiple compressed buffers and one or more
  blob frames.
- Dynamic material expressions are bytecode blobs, not plain strings.
- A generic decode and re-encode can change layout even when the logical tree is
  identical.

Prefer a targeted patch when a value already exists. If a new member must be
inserted, validate the result with both an independent parser and the game.

## Texture compiler metadata

The BCn format alone does not always describe the displayed pixels. `RED2`
special dependencies can request post-decode transforms such as:

- YCoCg reconstruction for some DXT5 panorama textures.
- DXT5 normal reconstruction where the stored channels are not display RGB.
- Actual-dimension cropping when storage was padded to a power of two.

A correct raw BC3 decoder can therefore produce a visibly wrong image if it
ignores compiler metadata. The inverse transform must also be honored on write,
or the game will apply a decode transform to already-RGB data.

## Model layout and alignment

Model resources are collections of independently addressed blocks. Valve's
exact padding is not a stable serialization contract. Observed models commonly
align most blocks to 16 bytes while `RERL` can be only 4-byte aligned. An engine
can accept a layout that is not byte-identical when the block table is correct,
but this must be verified in-engine.

## Tool ownership in this repository

- `vpkmerge-core` owns VPK operations and Deadlock-aware asset workflows.
- `morphic` owns Source 2 resource, KV3, texture, model, and sound decoding and
  patching primitives.
- `vpkmerge-cli` exposes stable workflows to scripts and other developers.
- `tools/morphic-oracle` is an independent development oracle based on
  ValveResourceFormat. It is not a shipped runtime dependency.

See the [`morphic` README](../../morphic/README.md) for current implementation
coverage.
