# Stored VTEX mip export

`panorama dump --mips` exports the mip levels present in a compiled texture. It never generates missing levels. Level 0 is the largest. Partial chains and NO_LOD one-level textures retain their stored level counts.

```sh
vpkmerge panorama dump --vpk <Deadlock>/game/citadel/pak01_dir.vpk --out-dir output --no-raw --mips \
  --prefix materials/particle/particle_glow_04.vtex_c
```

For `materials/particle/particle_glow_04.vtex_c`, this writes the existing preview `particle_glow_04.png`, adds `particle_glow_04.mip0.png` through `particle_glow_04.mip7.png`, and adds `particle_glow_04.mips.json` in the same directory. The sidecar records `format`, `level_count`, `one_level`, `no_lod`, `face`, `slice`, and each level's index, width, height, and workspace-relative PNG path. `_manifest.json` records those generated paths in `mip_paths`, including the sidecar, so later dumps remove stale exports.

The Rust API is `morphic::decode_stored_mips(&bytes) -> Result<Vec<Image>, DecodeError>`. It reads the header count, smallest-first storage, optional compressed-mip-size table and LZ4 payloads. It follows VRF's power-of-two display rectangle rules and selects face 0 / slice 0. Output is RGBA8 without premultiplication, gamma, YCoCg or normal-map conversions. The PNGs have no color profile and no preview size limit. The existing preview path still applies its own display conversions and size limit.

HDR formats return an error rather than tone-map. Zstd-compressed mip payloads return an error, as in the reference VRF decoder. Inline PNG/JPEG resources must have one stored level. The sidecars are export-only; `panorama build` continues to use the preview/raw workflow.

## Verification

Checked against VRF's `Texture.GenerateBitmap(mipLevel: m, decodeFlags: TextureCodec.None)` with the CPU decoder (TinyBCSharp 0.1.2), reading the game pak in place. The comparison checks PNG dimensions before decoding and compares unpremultiplied channel bytes, including RGB under zero alpha.

All **127 textures and 449 mip levels match exactly** in every R, G, B and A channel. This includes 23 sprite sheets, 34 NO_LOD one-level textures, and 19 textures cropped from padded canvases. All mip PNGs contain only IHDR, IDAT and IEND chunks.

| Stored format | Textures | Mip levels | Maximum R/G/B/A difference at every level |
|---|---:|---:|---|
| DXT1 | 20 | 54 | 0 / 0 / 0 / 0 |
| DXT5 | 52 | 149 | 0 / 0 / 0 / 0 |
| ATI1N | 6 | 9 | 0 / 0 / 0 / 0 |
| ATI2N | 5 | 20 | 0 / 0 / 0 / 0 |
| BC7 | 39 | 204 | 0 / 0 / 0 / 0 |
| RGBA8888 | 2 | 10 | 0 / 0 / 0 / 0 |
| BGRA8888 | 3 | 3 | 0 / 0 / 0 / 0 |

BC3 alpha needed a separate raw-export decoder. The existing bcdec preview uses biased integer division; VRF uses TinyBCSharp's fixed-point interpolation scales. Raw export uses those scales and leaves the existing preview unchanged. BC1 follows VRF's opaque-black three-color mode. There are no remaining rounding differences in the tested corpus.

The game samples in this set have no LZ4 mip payloads. Five synthetic resources add a mixed compressed/uncompressed LZ4 chain, an uncompressed partial chain, both BC3 alpha interpolation modes through 2x2 and 1x1 tails, and BC1 three-color mode. Their 15 levels also match VRF exactly. The synthetic VPK contains generated test bytes only.

## Default output

Without `--mips`, the added manifest field is omitted and no mip files are written. The CLI integration tests verify byte-identical preview/raw files with and without the flag, identical default manifests before and after a flagged dump, stale mip cleanup, and NO_LOD with `--no-raw`.
