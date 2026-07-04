# Spike: embed addon identity metadata (`addoninfo.txt`) into a VPK

Status: **Landed, unit-tested, and manually smoke-tested end to end. Grimoire
wiring (passing real title/author/GameBanana data through `modMerger.ts`) is
explicitly NOT done here, see "Grimoire integration" below.**

## The problem

Deadlock addon VPKs frequently end up with no identifying information once
they leave their original packaging: downloaded from GameBanana and renamed,
extracted from one of this project's own older releases, or merged from
several mods into one consolidated addon by Grimoire. Find one of these later
with no surrounding context (a `pak99_dir.vpk` sitting in a folder) and there
is no way to trace it back to where it came from or who made it.

The classic Source engine convention for this is a plain-text `addoninfo.txt`
at the root of the VPK (`"AddonInfo" { addonversion ... }`). This PoC adds that
convention to vpkmerge, plus a few extra fields useful for tracing a file back
to its GameBanana listing specifically.

## What was added

`vpkmerge-core/src/lib.rs`:

- `AddonMetadata`: `title` and `author` (required), `version` (defaults to
  `"1.0"`), `description`, `gamebanana_id`, `source_url`, `build_date` (all
  optional). `build_date` is caller-supplied (e.g. an ISO 8601 string the
  Electron/Node side already has via `Date.toISOString()`); nothing on the
  Rust side generates a timestamp.
- `embed_metadata(input, &metadata, output) -> Result<()>`: patches
  `addoninfo.txt` into an **already-built** VPK with no access to the original
  source assets. Opens `input`, re-extracts every entry into a tempdir (the
  same shape as `merge`'s winner-extraction loop), writes `addoninfo.txt` at
  the tempdir root (overwriting one that already exists rather than erroring),
  then repacks at `output` via `valve_pak::from_directory` + `.save`. Every
  original entry's bytes are preserved unchanged. `output` must differ from
  `input` (same restriction `merge`/`split` already enforce, since the input
  VPK's read handle is still open when the result gets written).
- `MergeOptions::metadata: Option<AddonMetadata>`: lets a normal `merge()`
  call stamp the same `addoninfo.txt` directly into its tempdir before
  packing, so a merged addon never exists without its identity file in the
  first place (no second pass over the finished VPK needed).

`vpkmerge-cli/src/main.rs`: a new `vpkmerge metadata` subcommand wraps
`embed_metadata` for the standalone "tag an already-built VPK" path:

```
vpkmerge metadata --vpk <INPUT.vpk> --title <T> --author <A> [--version <V>]
  [--description <D>] [--gamebanana-id <ID>] [--source-url <URL>]
  [--build-date <DATE>] --output <OUTPUT.vpk>
```

The `MergeOptions.metadata` path (stamping identity during a merge) is
exposed in the core API but not yet wired to a bare-merge CLI flag; that is a
stretch goal explicitly skipped in this PoC (see below).

## `addoninfo.txt` format

Classic KeyValues1, one `"AddonInfo"` block at the VPK root (same directory
level as `materials/`, `models/`, etc., so any generic VPK browser shows it
immediately). The four classic-spec keys come first
(`addonversion`/`addontitle`/`addonauthor`/`addonDescription`), followed by
the GameBanana-tracing extensions as additional keys in the same block
(`gamebananaId`/`sourceUrl`/`buildDate`). Every value is quoted; embedded
double quotes are escaped. Example (every optional field set):

```
"AddonInfo"
{
    addonversion "1.0"
    addontitle "Test Hero Skin"
    addonauthor "PoC Author"
    addonDescription "A test addon"
    gamebananaId "123456"
    sourceUrl "https://gamebanana.com/mods/123456"
    buildDate "2026-06-30T12:00:00Z"
}
```

Optional fields that were never set are simply omitted from the block, not
emitted as empty strings.

## Manual smoke test

Two tiny fake "mod" VPKs were packed with `vpkmerge_core::pack` (one file
each, under `materials/`), then `vpkmerge metadata` was run against one of
them with a realistic GameBanana-style example. Re-opening the output and
listing its entries plus the literal `addoninfo.txt` text:

```
== mod_a_tagged_dir.vpk ==
file_count: 2
  addoninfo.txt
  materials/foo.txt

-- addoninfo.txt --
"AddonInfo"
{
    addonversion "1.0"
    addontitle "Test Hero Skin"
    addonauthor "PoC Author"
    gamebananaId "123456"
    sourceUrl "https://gamebanana.com/mods/123456"
}
```

The original `materials/foo.txt` entry came through byte-identical (verified
both by this manual check and by the `embed_metadata_round_trips_fields_and_preserves_entries`
unit test). Writing to the same path as `--vpk` is correctly rejected
(`output path equals input path`), matching `merge`/`split`'s existing
in-place-overwrite guard.

## Known limitations / explicitly not done

- **No CLI flag wires `MergeOptions.metadata` into the bare-merge mode.** The
  core support exists (`MergeOptions { metadata: Some(..), .. }`), and is unit
  tested (`merge_with_metadata_includes_addoninfo_and_normal_entries`), but
  `vpkmerge <output> <in1> <in2> [flags]` does not yet expose
  `--addon-title`/`--addon-author`/etc. Adding that is a small follow-up: a
  few more `#[arg(long)]` fields on `Cli`, built into an `AddonMetadata` and
  passed through `MergeOptions` in `run_merge`.
- **No automatic build-date generation.** `build_date` is purely caller
  supplied; this keeps vpkmerge dependency-free (no `chrono`/`time` crate) and
  matches the existing house convention of pushing anything time-related to
  the Node/Electron caller, which already has a clock.
- **Quote escaping only.** `addon_info_text` escapes embedded `"` but not
  other KeyValues control characters (e.g. a literal newline in a title would
  break the block). Real-world titles/authors/descriptions from GameBanana are
  short single-line strings, so this is an acceptable PoC-scope simplification,
  not something hit by realistic input.

## Grimoire integration (follow-up, not done here)

Grimoire invokes the `vpkmerge` binary as a subprocess from
`electron/main/services/modMerger.ts` (`runVpkmerge`/`runVpkmergeStdout`). The
merge call site, `mergeModsLocked`, already assembles the args list right
before calling `runVpkmerge(args)`:

```ts
const args: string[] = [];
if (options.strict) args.push('--strict');
args.push(mergedPath);
for (const src of sources) args.push(src.path);
// a future --addon-title/--addon-author/... flag would be pushed here,
// before runVpkmerge(args) is called
```

This is the natural insertion point once the bare-merge mode gains identity
flags (see "Known limitations" above): `mergeModsLocked` already has the
merge's own title in scope (`trimmedName`, the user-supplied merge name) and
could pass it as `--addon-title`. It does **not** currently have an author to
pass: `Mod`/`ModMetadata` (`src/types/mod.ts`, `electron/main/services/metadata.ts`)
carry no author/submitter field anywhere, only GameBanana's
`GameBananaSubmitter.name` exists, and only on the Browse-page catalog types,
never persisted onto an installed mod. Wiring `--addon-author` for real would
need that field added to `ModMetadata` and stamped at download time first.

Per-source GameBanana id/url is fundamentally N-valued for a merge (multiple
sources, some local with no GameBanana id at all), so a single
`--gamebanana-id`/`--source-url` pair only makes sense for the standalone
`vpkmerge metadata` path (tagging one already-built VPK, e.g. re-tagging a
single downloaded mod before it ever enters a merge) or would need a future
per-input flag / JSON-plan extension on the merge side, not attempted here.

A second call site, `extractMergeSourceLocked` (rebuild-after-extract:
`runVpkmerge([buildPath, ...ordered.map((m) => m.path)])`), would need the
same flag added for consistency, since it rewrites the merged VPK in place on
every source-extraction and the identity stamp would otherwise go stale after
the first edit.
