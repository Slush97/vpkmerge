---
name: package-mods
description: Inspect, combine, and verify Deadlock VPK mods using this repository's core library and CLI, including explicit conflict priorities.
---

# Package mods

Use the harness `inspect_mod` and `detect_conflicts` tools to discover archive
contents and overlaps before selecting winners. Follow `next_offset` when present.
These tools inspect only; build operations currently use the existing VPKMerge CLI.
Read its `--help` before constructing commands.

Keep input order and collision policy explicit: core/CLI defaults to last input
wins, while the GUI defaults to first input wins. Use the user's chosen priority;
if conflicting intent cannot be resolved, present the conflicting paths and owners.
Pass `_dir.vpk` indexes and retain their companion chunks.

Build to a project output location, preserve source archives, and inspect the
result's virtual paths and winning contents. Record input paths, tool revision,
policy, output, and skipped work. Compare entry contents for repeatability rather
than assuming archive hashes are stable across builds.

Use [testing and debugging](../../../docs/deadlock-modding/testing-debugging.md)
for validation and game-update failures. Report inventory inspection, offline
checks, and in-game observations separately. Packaging does not itself request
installation into the game.
