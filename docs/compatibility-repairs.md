# Compatibility repairs

```powershell
vpkmerge compatibility check --base GAME_PAK --json MOD_VPK [MORE_MODS]
vpkmerge compatibility heal --base GAME_PAK --output NEW_VPK --json MOD_VPK
```

Checks emit per-mod JSON reports. `repairable` entries passed the adapter's
checks and serialization verification. `unsupported` entries include a reason
and remain unchanged. An empty report means no supported repair was found, not
that a mod is fully compatible. Heal writes nothing when no repair is available.
The output must be a new path. Every packed entry is verified against either
the input or its planned replacement before success is returned.

The adapters restore missing ability/hero object fields, append missing
localization tokens, refresh equivalent sampling skeleton layouts, and restore
absent modern model animation bindings after checking named parents and bind
poses. Existing mod values and arrays are preserved. Larger pose/hierarchy
changes, custom masks, authored animation clips and unknown schemas are declined.
Pass `--animation-overrides` when other enabled mods supply skeletons, graphs or
clips. The caller owns the complete enabled-mod context; a standalone invocation
cannot discover another manager's active mods.

The existing morphic codec now exposes `decode_flagged`/`encode_flagged` for
structural edits that need semantic resource flags. A pre-versioned VKV3 reader
supports raw, LZ4 and Valve block compression, with explicit refusal of legacy
semantic flags that have no implemented conversion. The legacy format reference
is ValveResourceFormat, already covered by morphic's MIT third-party notice.

Run `DEADLOCK_PAK`-gated compatibility tests against the installed game to check
the real compiled-model round trip. These tests use temporary mod archives and
do not modify the installation. `examples/compatibility_fixture.rs` builds a
separate demo installation for Grimoire's packaged UI check. Resource-contract
tests do not establish in-game rendering or animation behavior.
