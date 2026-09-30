# Deadlock modding knowledge base

This is the durable, developer-facing index for what this repository has learned
about modding Deadlock. It focuses on observations that are useful beyond one mod,
one workstation, or one debugging session.

Deadlock changes frequently. Treat exact asset paths, hero aliases, field layouts,
and tool compatibility as versioned observations. Each focused guide separates
general Source 2 behavior from Deadlock-specific findings and calls out known
verification limits.

Last reviewed: 2026-08-05.

## Start by task

| If you want to... | Read first | Then go deeper |
|---|---|---|
| Understand how a mod reaches the game | [Fundamentals](fundamentals.md) | [Testing and debugging](testing-debugging.md) |
| Identify an unfamiliar compiled asset | [Asset formats](asset-formats.md) | [`morphic` README](../../morphic/README.md) |
| Recolor or replace a skin | [Textures and materials](textures-materials.md) | [Skin texture findings](../findings-deadlock-skin-textures.md) |
| Export, edit, or replace a model | [Models and animation](models-animation.md) | [Model editing handoff](../handoff-model-edit.md) |
| Recolor an ability or author an effect | [Particles and VFX](particles-vfx.md) | [Customization frontier](../customization-frontier.md) |
| Replace ability, weapon, VO, or music audio | [Audio and soundevents](audio-soundevents.md) | [Music-pack research](../deadlock-music-pack-research.md) |
| Resolve hero naming mismatches | [Hero identifiers](hero-identifiers.md) | [Models and animation](models-animation.md) |
| Check a mod after a game update | [Testing and debugging](testing-debugging.md) | [Source 2 preview contract](../source2-preview-data-contract.md) |

## Verification language

The guides use these terms deliberately:

- **Engine-verified**: loaded and observed in a retail Deadlock build.
- **Offline-verified**: parsed, round-tripped, rendered, or diffed by tools, but not
  necessarily loaded by the game.
- **Research**: a useful hypothesis or format observation that still needs an
  in-game gate.
- **Historical**: tied to a specific Deadlock or community-tool build. Recheck
  before applying it to current assets.

An offline parser accepting a file does not prove that Source 2 will accept it.
The engine is the final compatibility test.

## Deeper repository documents

The files below contain implementation reports, format research, and project
handoffs. They remain useful, but some describe a point in time rather than a
stable contract.

### Source 2 formats and architecture

- [Repository architecture](../architecture-mermaid.md)
- [Source 2 preview data contract](../source2-preview-data-contract.md)
- [VPK splicing](../splicing.md)
- [Binary KV3 soundevents](../spike-vsndevts-kv3.md)
- [Blobbed material patching](../spike-blobbed-vmat-recolor.md)
- [VPK identity metadata](../spike-vpk-metadata-embed.md)

### Models, animation, and preview

- [Model exporter](../vmdl-glb-exporter.md)
- [Exporter continuation handoff](../vmdl-glb-exporter-handoff.md)
- [Animation authoring pipeline](../anim-authoring-pipeline.md)
- [Animation export handoff](../anim-export-handoff.md)
- [Loose animation clip posing](../handoff-nm-loose-clip-pose.md)
- [Preview fidelity roadmap](../preview-fidelity-roadmap.md)
- [VRF renderer gap report](../vrf-renderer-gap-report.md)

### Textures, materials, and VFX

- [Deadlock skin texture findings](../findings-deadlock-skin-textures.md)
- [Texture-edit CLI handoff](../handoff-texture-edit-cli.md)
- [Vertex-color recolor handoff](../handoff-vertex-color-recolor.md)
- [NPR and toon shading spike](../spike-npr-toon-shading.md)
- [Rainbow Prism QA findings](../rainbow-prism-qa-findings.md)
- [Roster recolor audit](../remaining-roster-recolor-audit.md)

### Audio

- [Deadlock music-pack research](../deadlock-music-pack-research.md)
- [Music-pack best practices](../music-pack-best-practices.md)

### Experimental and historical handoffs

- [Customization frontier](../customization-frontier.md)
- [Soul container Rust workflow](../soul-container-rust-import-workflow.md)
- [Soul container ResourceCompiler findings](../soul-container-resourcecompiler.md)
- [Hero Locker live preview feasibility](../hero-locker-live-preview.md)

## Adding a durable finding

Add a fact here only when another developer can reproduce it. A focused document
should record:

1. The Deadlock build date or manifest context.
2. The exact asset path or discovery query.
3. Whether the result is engine-verified, offline-verified, or research.
4. The smallest reproduction command.
5. The failure mode and recovery path.
6. Which paths, aliases, or formats are likely to drift in a game update.

Keep personal paths, installed addon slot numbers, temporary output locations,
and session history out of repository documentation.
