# Deadlock modding fundamentals

This guide describes the common path from a shipped Deadlock asset to a small,
testable override mod.

## The override model

Most Deadlock mods do not mutate the base game archives. They ship a second file
at the same virtual path as a base asset. When the addon search path has higher
priority, Source 2 resolves the modded entry instead of the base entry.

That gives a useful mental model:

```text
base pak entry
    -> extract or decode
    -> change the smallest required payload
    -> pack at the same virtual path
    -> mount addon with higher priority
    -> restart or reload the affected resource class
```

The virtual path is part of the mod's behavior. A correctly encoded asset packed
under the wrong path is indistinguishable from a mod that never loaded.

## VPK conventions

- Pass the `*_dir.vpk` index to tools. Numbered chunks beside it are discovered
  through that index.
- A generated addon normally needs only a directory VPK unless it is large enough
  for the writer to create chunks.
- In the locally verified manual test setup, mounted addon files use
  `pak0N_dir.vpk` names under `game/citadel/addons/`. Descriptive filenames can be
  kept outside the active directory or in an `.disabled/` directory.
- Later or higher-priority overrides can hide an otherwise valid mod. When a fix
  appears ineffective, inventory every active addon that contains the same path.

Do not overwrite `citadel/pak01_dir.vpk`. That is the base archive index, not a
free addon slot.

## Compiled names versus source references

Source 2 commonly stores a source-style reference inside one resource while the
VPK contains the compiled form:

| Reference in data | Compiled VPK entry |
|---|---|
| `sounds/example.vsnd` | `sounds/example.vsnd_c` |
| `materials/example.vmat` | `materials/example.vmat_c` |
| `particles/example.vpcf` | `particles/example.vpcf_c` |
| `models/example.vmdl` | `models/example.vmdl_c` |

Preserve the form expected by the containing resource. For example, a
soundevent's `vsnd_files` list holds `.vsnd` paths even though the packed clip is
`.vsnd_c`.

## Case matters

ResourceCompiler may lowercase compiled VPK paths while references retain mixed
case. Windows often hides this mismatch; Linux does not. Resolve VPK entries with
a case-insensitive lookup when following a reference, then pack the override at
the actual compiled path found in the archive.

## Hero names are namespaces, not one identifier

A display name, roster record, model basename, soundevents basename, material
stem, and particle directory can all differ. Vindicta, for example, commonly
uses `hornet` for models and particles while materials use `vindicta` names.

Use [Hero identifiers](hero-identifiers.md) as a discovery guide. Do not build a
feature around display-name-to-directory guesses.

## Prefer minimal edits

Compiled resources often contain blocks that a high-level decoder does not fully
understand. The safest edit is usually the narrowest one:

1. Replace only a texture mip chain instead of reserializing the whole resource.
2. Patch known KV3 scalar or blob bytes instead of rebuilding a material or
   particle file.
3. Reuse a compatible donor resource when minting compiled audio.
4. Preserve unknown blocks, edit-info, dependencies, padding, and trailing data.
5. Pack only the entries the mod actually overrides.

This is not merely an optimization. Several files that round-trip through an
offline decoder still fail in-engine after a full re-encode.

## A repeatable workflow

1. **Identify** the target by tracing roster data, model references, materials,
   particle children, or soundevents.
2. **Inventory** every dependent entry and note which archive contains it.
3. **Extract and inspect** without modifying the base archive.
4. **Make one class of change** at a time.
5. **Validate offline** by reopening the output and diffing the intended fields.
6. **Pack at original paths** into a fresh addon.
7. **Test in isolation** with conflicting addons disabled.
8. **Record the game build and result** so the finding can survive handoff.

See [Testing and debugging](testing-debugging.md) for the validation ladder and
update workflow.
