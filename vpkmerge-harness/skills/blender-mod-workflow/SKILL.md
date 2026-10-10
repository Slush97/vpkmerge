---
name: blender-mod-workflow
description: Load, inspect, edit, and preview Deadlock mod assets through Blender MCP, then select a supported VPKMerge or CSDK export route.
---

# Blender mod workflow

Use the connected host's Blender MCP tools. The harness currently stores its
server name but does not connect to it. Inspect available tool schemas; the
repository's recorded integration requires `user_prompt` on Blender calls.

For hero import and framing, read the
[existing workflow](../../../docs/blender-mcp/workflows/load-hero-into-blender.md).
Resolve hero names against current assets rather than guessing codenames. Record
the source virtual path and distinguish a posed preview from a rigged export.

Inspect the scene before changes and save a `.blend` checkpoint for substantial
edits. Preserve unrelated objects. Record target objects, armature, transforms,
material/image mappings, and export settings. Use evaluated world-space bounds
when framing scaled rigs; a render can be more reliable than viewport framing.

For game-bound edits, check the current CLI model edit options and the existing
`tools/hero-model-compiler/` scripts. An exported GLB is not a universal round-trip
format. Choose a supported edit/import or CSDK authoring route for the requested
change before altering topology, skeletons, or animation bindings.

Record compiler/game versions and output freshness when using CSDK. Use
[models and animation](../../../docs/deadlock-modding/models-animation.md) and
[testing](../../../docs/deadlock-modding/testing-debugging.md) for acceptance.
A Blender render establishes appearance in Blender, not retail compatibility.
