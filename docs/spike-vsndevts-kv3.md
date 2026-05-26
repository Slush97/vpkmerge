# Spike: read/write Deadlock `.vsndevts_c` (compiled KV3)

Status: **All phases done. In-game verified: the engine loads our uncompressed v4
KV3 and honors edited params. Param control is viable; GO for the Grimoire picker.**

## Goal

Let Grimoire read, modify, and re-emit `soundevents/hero/<codename>.vsndevts_c` so a
per-ability sound picker can control `volume` / `pitch` / clip selection (not just
swap the audio file at a vanilla path). This is generic Source 2 resource work, so it
lives here next to `morphic` (`.vtex_c`) and `valve_pak`, not in the Electron client.

## What the file is (verified against the live install)

`soundevents/hero/gigawatt.vsndevts_c` (16,229 bytes, pulled from
`game/citadel/pak01_dir.vpk`):

- Source 2 resource container: 16-byte header, block table, `RED2` block (10,958 B,
  edit/dependency info) + `DATA` block (5,221 B). `morphic`'s existing resource parser
  already handles this envelope.
- `DATA` is **binary KV3 version 5** (magic `05 33 56 4B` = `0x4B563305`),
  **LZ4-compressed** (`compressionMethod = 1`, frame size 16384). The v5 layout splits
  content into two LZ4 blocks: an *auxiliary* buffer (string table + primitive arrays)
  and a *main* buffer (per-object member counts, scalar lanes, the type stream).
- Tree shape: 44 named hero soundevents. Each event is an object with a `base`
  (inherited template, a plain string like `Base.Weapon.Pistol`), a `vsnd_files` array
  (the `.vsnd` clips it can play; multiple = random pick), and params (`volume`,
  `low_ammo_threshold`, `vsnd_duration`, ...). Almost every number is a KV3 `DOUBLE`.
  **No KVFlags and no binary blobs appear in this file.**

## What was built

### `morphic::kv3` (the generic codec)

- `kv3::decode(&[u8]) -> kv3::Value` - binary KV3 reader, ports VRF's `BinaryKV3`,
  handles **v1..=5** including the v5 two-buffer/LZ4 path. (`reader.rs`)
- `kv3::encode(&kv3::Value, &kv3::Format) -> Vec<u8>` - writer that emits
  **v4 uncompressed**, exactly as VRF's own `Serialize` does. (`writer.rs`)
- `kv3::Value` - order-preserving tree (`Object` is a `Vec<(String, Value)>` so key
  order and `base`-first semantics survive a round-trip). (`types.rs`)
- Resource-level helpers on `morphic`:
  - `decode_kv3_resource(file_bytes) -> Value`
  - `encode_kv3_resource(original_bytes, &Value) -> Vec<u8>` - re-encodes only the
    `DATA` block, **preserving the original format GUID and the `RED2` block verbatim**,
    and rebuilds the container's block table (`resource::rebuild_with_data`).

Decode/encode were ported from
[ValveResourceFormat](https://github.com/ValveResourceFormat/ValveResourceFormat) (MIT):
`BinaryKV3.cs` (reader), `BinaryKV3.Serialization.cs` (writer), `BinaryKV3.NodeType.cs`.

### `vpkmerge-core::soundevents`

`SoundEvents` wraps a decoded file with soundevents-aware helpers: `from_file` /
`from_vpk` / `from_bytes`, `summaries()`, `to_json()` (key order preserved),
`swap_vsnd(from, to)`, `set_event_field(event, field, value)`, `encode()`.

### CLI: `vpkmerge soundevents`

```bash
# Decode to JSON on stdout, human summary on stderr:
vpkmerge soundevents gigawatt.vsndevts_c
vpkmerge soundevents soundevents/hero/gigawatt.vsndevts_c --from-vpk pak01_dir.vpk

# Edit and re-emit an uncompressed, loadable file:
vpkmerge soundevents gigawatt.vsndevts_c \
  --swap-vsnd "sounds/a/old.vsnd=sounds/b/new.vsnd" \
  --set "Seven.Wpn.Fire/volume=0.25" \
  --encode out.vsndevts_c
```

## Key design decisions / why it is lower-risk than feared

- **Encode as v4 uncompressed, not v5 LZ4.** VRF's own serializer emits v4 uncompressed,
  and the engine reads v1..=5, so this is a spec-blessed path that avoids writing an LZ4
  *encoder*. The re-emitted file is ~2x larger (gigawatt: 16 KB -> 32 KB); that is the
  only cost.
- **Numeric/array widening is lossless.** Decode folds `INT32`/`FLOAT`/`INT16`/... to
  `Int(i64)`/`Double(f64)`; the writer re-emits `INT64`/`DOUBLE` and a generic `ARRAY`.
  KV3 consumers coerce by key, so values are byte-for-byte preserved and the game reads
  them identically.
- **Round-trip is proven at the tree level.** Decoding Valve's v5/LZ4 gigawatt,
  re-encoding to v4 uncompressed, and decoding again yields the *identical* `Value`
  tree (every typed sub-buffer is consumed exactly, a strong internal invariant). Tests:
  `morphic/tests/kv3.rs`, plus `vpkmerge-core::soundevents` unit tests.

## Known simplifications (none reached by soundevents data)

- KVFlags (`Resource`, `SoundEvent`, ...) are consumed but discarded - the `Value` tree
  has no slot for them. gigawatt uses none. If a future file relies on a flag, the tree
  would lose it on re-encode.
- Binary blobs (`countBlocks > 0`) are rejected on decode. No soundevents file ships
  them.
- Zstd-compressed payloads (`compressionMethod = 2`) error out; Deadlock ships LZ4.

## Independent validation (besides the self-round-trip)

The round-trip test only proves morphic's decoder and encoder agree with each other.
Two external checks confirm the output is genuinely spec-valid:

- **ValveResourceFormat reads our re-encode.** `tools/morphic-oracle kv3dump --file X`
  loads a file with VRF; it parses both the identity and edited uncompressed re-encodes
  as `BinaryKV3`, preserving the original format GUID. (Independent decoder, not ours.)
- **Corpus decode.** morphic's decoder reads **148 of 149** `.vsndevts_c` files in the
  base `pak01` (the lone failure is the documented flag/blob case).

Also confirmed offline: `soundevents/hero/gigawatt.vsndevts_c` is the *only* base file
defining `Seven.Wpn.Fire` / referencing the gun clips, so it is the correct edit target.

## Phase 4: in-game verification (THE crux risk) - PASSED

**Result: the engine loads our uncompressed v4 KV3 addon and applies edited params.**
Confirmed by setting `Seven.Wpn.Fire`'s `volume` to -96 dB and observing gigawatt's
primary fire go silent in-game. So `volume`/`pitch` control (not just audio-file swaps)
is viable; proceed to wire Grimoire.

**Gotcha that cost two test rounds:** Deadlock mounts addon VPKs only at **process
start**. Returning to the menu or reloading does *not* remount a changed
`addons/pakNN_dir.vpk`. Always fully quit to desktop and relaunch between edits.
(Earlier clip-swap tests read as "normal sound" purely because the game had not been
cold-restarted.)

The staged addon and procedure used:

- `~/gigawatt_soundtest_dir.vpk` - an addon VPK containing
  `soundevents/hero/gigawatt.vsndevts_c`, edited so `Seven.Wpn.Fire` (gigawatt's basic
  weapon fire) plays the **power-surge fire** clips instead. Both clip sets already ship
  in the base pak (no custom assets) and are already `RED2` dependencies of this file,
  so this isolates "does uncompressed KV3 load" from any missing-asset confound.

Test procedure:

1. Copy the addon in: `cp ~/gigawatt_soundtest_dir.vpk <Deadlock>/game/citadel/addons/`
2. Enable it the way you normally enable a `*_dir.vpk` addon, launch, pick gigawatt
   (Seven), and fire the primary weapon.
3. Interpret:
   - **Gun now sounds like the power-surge fire** -> SUCCESS. The engine loaded our
     uncompressed, edited KV3. Param control (volume/pitch) is viable; proceed to wire
     Grimoire.
   - **Gun is silent / hitches / addon fails to mount** -> the engine likely rejects
     uncompressed KV3 (or cross-checks `RED2`). Report back; fall back to audio-swap-only
     (replace the `.vsnd_c` at the vanilla path, no param control).

Residual risk to watch even on success: we keep the *original* `RED2` dependency list.
Swapping a clip to a path **not** already listed in `RED2` (e.g. a truly custom
`.vsnd_c`) may need `RED2` regeneration. The staged test deliberately swaps to paths
already in `RED2` to test the format first; a custom-asset swap is the natural follow-up.
