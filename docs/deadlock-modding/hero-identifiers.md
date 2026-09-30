# Hero identifiers and asset discovery

Deadlock does not use one canonical hero name across every asset system. Treat
hero identity as a join across several namespaces.

## Namespaces to record

| Namespace | Example for Vindicta | Where it appears |
|---|---|---|
| Display name | `Vindicta` | UI and player-facing data |
| Model/asset codename | `hornet` | Model basenames and many particle paths |
| Material stem | `vindicta` | Material and texture names |
| Roster record | may differ again | `scripts/heroes.vdata_c` |
| Soundevents basename | hero-dependent | `soundevents/hero/*.vsndevts_c` |

Do not infer one column from another without tracing the shipped references.

## Frequently encountered aliases

These mappings have been observed in shipped assets. They are discovery hints,
not a permanent game API.

| Display name | Common asset codename | Notes |
|---|---|---|
| Vindicta | `hornet` | Models and particles use `hornet`; materials commonly use `vindicta` |
| Mina | `vampirebat` | Model and hero sound asset codename |
| Celeste | `unicorn` | Used by model, particle, and recipe tooling |
| Paige | `bookworm` | Used by model, particle, and recipe tooling |
| Seven | `gigawatt` | Common soundevents and particle codename |
| Graves | `necro` | Common material, particle, and sound asset codename |
| Infernus | `inferno` | Common model and effect codename |
| Paradox | `chrono` | Common body, material, and effect codename |
| Holliday | `astro` | Common effect recipe codename |
| Lady Geist | `ghost` | Common effect recipe namespace; verify model/material paths separately |
| Rem | `familiar` | Common ability and particle namespace |
| Sinclair | `magician` | Model namespace |
| Apollo | `fencer` | Model and effect namespace |
| Calico | `nano` | Model and effect namespace |
| Victor | `frank` | Ability and particle namespace |
| Silver | `werewolf` | Ability and soundevents namespace |
| Abrams | `abrams`, `bull` | Assets have migrated across both namespaces; recipes may need both |
| Pocket | `pocket`, `synth` | Roster and ability/material namespaces can differ |
| Yamato | `yamato` | One of the simpler same-name cases |
| Wraith | `wraith` | One of the simpler same-name cases |

## Discovery procedure

For a feature that must survive updates:

1. Find the hero record in `scripts/heroes.vdata_c` by display name or known
   class metadata.
2. Record its bound ability records and model reference.
3. Follow the model into material dependencies rather than guessing stems.
4. Search particle and soundevent data for the bound ability and event values.
5. Confirm every path exists in the current VPK inventory.
6. Store aliases as data with provenance, not scattered hard-coded switches.

When a directory name changes, keep both namespaces only while current base data
still references both. Abrams has been observed with ability effects split across
`abrams` and `bull`; this is exactly the kind of migration a path-existence audit
should detect.

## Skeleton identity is separate

Sharing an asset codename or humanoid bone names does not imply drop-in skeleton
compatibility. Rest poses and weighted-bone coverage vary substantially. See
[Models and animation](models-animation.md) for the checks required before a
model swap.
