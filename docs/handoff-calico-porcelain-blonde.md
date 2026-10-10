# Calico "Porcelain Blonde": white skin, blonde hair, light blue outfit

Builder: `vpkmerge-core/examples/reskin_nano_porcelain_blonde.rs`
(`--png <prefix>` previews the art, a second positional arg bakes the addon VPK,
`--accents harmony|mono|warm` picks the sash/tails treatment).
Verifier: `vpkmerge-core/examples/verify_calico_bake.rs`.
Probe used to derive the spec: `vpkmerge-core/examples/nano_palette_probe.rs`.
Renders and working artifacts: `blender-work/calico-reskin/`.

Built 2026-07-25. Installed as Grimoire local mod "Calico Porcelain Blonde"
(`addons/pak12_dir.vpk`). Blender-verified; **in-game reconfirm pending.**

## Why a plain `vpkmerge texture --hue` cannot do this

A hue-set keeps each pixel's saturation and value, and all three asks here are
value/saturation moves:

| ask | source | why hue alone fails |
|---|---|---|
| white skin | dark brown, V 0.28-0.39 @ S 0.42-0.49 | any hue at V 0.34 is still a dark brown |
| blonde hair | near-black, V 0.067 median | any hue at V 0.067 is still black |
| light blue outfit | dark periwinkle, V 0.15 (trousers) .. 0.62 (gloves) | a blue hue at V 0.15 is navy |

So each region gets a target hue, a saturation **scale**, and a value **remap**
(`v -> out_lo + t*(out_hi-out_lo)` over an input window), which lifts the midtone
and rescales the range rather than shifting it.

## Region mapping: the actual work

Calico is `nano`. Two mesh parts (`body`, `cat_v2`), three materials
(`nanov2_body`, `nanov2_head`, `nano_cat`), so `model mask --by part` and
`--by material` cannot separate hair from skin. `--by island` is no help either
(the head is a single 33k-tri connected component; the model reports 1419 islands
with one 52%-coverage monster). `g_tNprTransmissiveColor` is **4x4** on both head
and body, i.e. a flat constant, so there is no authored mask to borrow.

What the probes established, all against the live pak:

- **Her face AND her whole skull are `nanov2_head`; the BUN is `nanov2_body`.**
  Confirmed with a per-material flat-colour render (head red / body blue): the
  glasses, lenses, earrings and the bun are all body material.
- **She is shaved except for a crown patch feeding the bun.** The back and sides
  of the skull are painted *exactly* her skin tone: skull_back `rgb(86,58,48)`,
  skull_back_low `rgb(87,58,48)`, above_ear `rgb(91,61,51)` vs cheek
  `rgb(90,59,50)`. Only crown_top is hair, at `rgb(21,19,18)`. This is the single
  most important finding: her "big dark hair mass" in a stock render is mostly
  dark *skin*, and there is no hair there to make blonde.
- **Within the head texture, value splits hair from skin cleanly.** The albedo is
  bimodal: hair at V 0.08-0.12 (31% of texels), skin at V 0.20-0.42 (53%), with a
  near-empty valley at V 0.12-0.20 to cut on. Gold sits alone at a flat
  V 0.68 / S 0.68. Hue cannot split them (skin spans 8-18 deg, hair sits at 20).
- **The bun is one tight, effectively exclusive UV rectangle** of the body
  texture, u 0.878-0.975 / v 0.416-0.551: of the 1190 faces whose UV centre lands
  there, 958 are the bun, 220 are head-material (different texture, so harmless)
  and 12 are stray waist slivers. Gating on UV box **and** colour needs no baked
  mask, which keeps the builder standalone.
- **The whole main garment is one hue band.** Blazer, sleeves, shoulder puffs,
  gloves, cuffs and trousers all measure hue 247-259, so a single band recolours
  the outfit coherently. Sash is hue 287, coat tails hue 344, and the white dress
  shirt plus the grey gun are below the saturation gate, so they survive untouched
  for free.

Useful trick for the next one: the **head AO map doubles as a used-texel mask**.
Its top ~45% is pure black, marking texels the head material never samples, so
histograms over the raw albedo overcount dark pixels badly (the first pass thought
32% of the head was hair; much of that was unused padding).

## Bugs worth remembering

1. **Order the head tests so hair is decided before any saturation/hue gate.**
   Her hair is near-black, where hue and saturation are numerically unstable:
   plenty of hair texels measure S < 0.13 or a nonsense cool hue. Gating on those
   first skipped exactly those texels and left them black, which rendered as dark
   speckle through the new blonde. Anything that dark is hair, brow or lash
   whatever its hue says, so value decides alone. The body's bun rule has the same
   shape: it excludes only what is unmistakably the surrounding periwinkle coat
   (`200-300 deg && S > 0.25`) instead of requiring a warm hue.
2. **Blonde-on-pale separates by SATURATION, not value.** Golden blonde is about
   `rgb(230,195,120)` = hue 41 / S 0.48 / V 0.90; light skin is about
   `rgb(230,200,180)` = hue 26 / S 0.22 / V 0.90. Nearly the same brightness, more
   than twice the saturation. A pass that made the blonde slightly *darker* than
   the skin at a middling S 0.46 read as a dirty smudge on the skull.
3. **Do not run the skin's value window up near 1.0.** `(0.72, 0.97)` rendered a
   featureless paper-white blob with the brows, nostrils and lip line all clipped
   away together. Light skin albedo lives around V 0.70-0.85.
4. **Keep the blonde's output window narrow.** Her hair is painted flat
   near-black, so a wide window multiplies the BC7 noise in that near-black into
   visible blotching (a 0.46-0.76 window rendered as mottled khaki). The hair's
   form comes from the normal map and AO, as it does for the stock black, so the
   albedo only has to supply a clean colour.
5. **Keep the gold.** Glasses, earrings, the bun's spike and the shirt trim stay
   warm on purpose: it is the complement that stops the pale-blue-and-ivory read
   going flat. Same lesson as bebop's brass.
6. **Reading Blender `image.pixels` is not the same as reading the PNG.** These
   textures come back already sRGB-encoded, so applying a linear->sRGB conversion
   on top brightens every measurement (it inflated the blazer from V 0.28 to
   V 0.57 and sent an early spec off course). Also set `alpha_mode = 'NONE'`:
   Deadlock albedo alpha is a mask, not transparency.

## Accents

`--accents` only changes the sash (hue 287) and the coat tails (hue 344):

- `harmony` (default): sash to royal blue, tails to champagne. Keeps one warm mass
  to answer the gold and the blonde. This is what shipped.
- `mono`: sash and tails both into the blue family.
- `warm`: sash to gold, tails left coral. Boldest.

## Validation

`verify_calico_bake.rs` re-reads each entry from the addon, asserts the format and
dimensions still match the texture it overrides (an override that changes either
makes the engine sample garbage), and dumps the decode for diffing. BC7 round-trip
against the intended recolour is visually lossless: head mean |err| 0.179/255
(PSNR 54.9 dB), body 0.111/255 (PSNR 56.8 dB).

Only the two albedos are touched: no `.vmat_c` edit (a KV3 re-encode renders the
engine error shader on hero materials) and no normal/roughness change.

## Follow-ups

- **In-game reconfirm** (the one real gap).
- **Matching ability VFX**: `nano` has a pinned `recolor-hero` recipe, so
  `vpkmerge recolor-hero --hero nano --vpk <pak> --hue 205 --encode-vpk <out>`
  would bring her particles/textures into the same light blue.
- The cat (`nano_cat.vmat`) is deliberately untouched and still reads purple.
