//! Build a soft-light-blue recolor of the Glamorous Geist addon.
//!
//! The custom skin inherits the stock Geist clothes and animated arm materials.
//! This recolors the green/teal regions of the clothes atlas, tints the custom
//! veil/hat crown, and changes every shader tint used by the scrolling/pulsing
//! arm effect without replacing its animation expression.
//!
//! Usage:
//!   cargo run --release -p vpkmerge-core --example reskin_glamorous_geist_blue -- \
//!     <base-pak01_dir.vpk> <glamorous-geist_dir.vpk> <overlay_dir.vpk> <preview.png>

use morphic::{Image, ImageData};
use vpkmerge_core::{patch_vmat_params, Recolor, VmatEdit};

const CLOTHES_COLOR: &str =
    "models/heroes_wip/geist/materials/geist_clothes_color_png_7a91a619.vtex_c";
const MODEL: &str = "models/heroes_wip/geist/geist.vmdl_c";
const VEIL_VMAT: &str = "models/heroes_wip/geist/materials/geist_veil.vmat_c";
const VEIL_COLOR: &str =
    "models/heroes_wip/geist/materials/geist_veil_vmat_g_tcolor_c0296ee0.vtex_c";
const ARM_VMAT: &str = "models/heroes_wip/geist/materials/geist_arm.vmat_c";
const ARM_COLOR: &str = "models/heroes_wip/geist/materials/geist_arm_vmat_g_tcolor_c36f4af0.vtex_c";
const ARM_GLOW_VMAT: &str = "models/heroes_wip/geist/materials/geist_armglow.vmat_c";
const SPECTRE_ARM_VMAT: &str = "models/heroes_staging/ghost/materials/ghost2_arm.vmat_c";
const SPECTRE_ARM_COLOR: &str =
    "models/heroes_staging/ghost/materials/ghost2_arm_color_png_c42e97cd.vtex_c";
const LEGACY_ARM_GLOW_VMAT: &str = "models/heroes_staging/ghost/materials/ghost_armglow.vmat_c";
const SPECTRE_HAND_PARTICLE: &str =
    "particles/abilities/ghost/ghost_blood_exchange_precast_tgt_hand.vpcf_c";

fn rgba8_mut(img: &mut Image) -> anyhow::Result<&mut Vec<u8>> {
    match &mut img.data {
        ImageData::Rgba8(v) => Ok(v),
        ImageData::Rgba16F(_) => anyhow::bail!("unexpected HDR texture"),
    }
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max <= f32::EPSILON { 0.0 } else { d / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let [r, g, b] = match (h / 60.0).floor() as i32 {
        0 => [c, x, 0.0],
        1 => [x, c, 0.0],
        2 => [0.0, c, x],
        3 => [0.0, x, c],
        4 => [x, 0.0, c],
        _ => [c, 0.0, x],
    };
    [r + m, g + m, b + m]
}

fn recolor_green_fabric(img: &mut Image) -> anyhow::Result<()> {
    let px = rgba8_mut(img)?;
    for p in px.chunks_exact_mut(4) {
        let (h, s, v) = rgb_to_hsv(
            p[0] as f32 / 255.0,
            p[1] as f32 / 255.0,
            p[2] as f32 / 255.0,
        );

        // The dress/hat fabric occupies the teal-green family.  Feather the
        // edge of the hue selection to avoid seams while preserving neutral,
        // gold, brown, black, and purple details in the shared atlas.
        let hue_weight = if (128.0..=194.0).contains(&h) {
            1.0
        } else if (112.0..128.0).contains(&h) {
            (h - 112.0) / 16.0
        } else if (194.0..=208.0).contains(&h) {
            (208.0 - h) / 14.0
        } else {
            0.0
        };
        let sat_weight = ((s - 0.12) / 0.28).clamp(0.0, 1.0);
        let weight = hue_weight * sat_weight;
        if weight <= 0.0 {
            continue;
        }

        // Soft powder blue. Preserve the source value/shading, lift deep
        // fabric slightly, and mute saturation so highlights stay delicate.
        // Deadlock's warm hideout lighting desaturates the earlier pass into
        // gray. Keep this visibly blue while retaining the fabric's shadows.
        let target_s = (0.50 + 0.12 * s).clamp(0.0, 0.66);
        let target_v = (v * 1.16 + 0.13).clamp(0.0, 1.0);
        let target = hsv_to_rgb(205.0, target_s, target_v);
        for c in 0..3 {
            let old = p[c] as f32 / 255.0;
            p[c] = ((old + weight * (target[c] - old)) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
    Ok(())
}

fn tint_neutral_hat_texture(img: &mut Image) -> anyhow::Result<()> {
    let px = rgba8_mut(img)?;
    // Multiplicative tint preserves any black/gray structure and alpha while
    // turning the white crown into the same soft powder blue as the dress.
    let tint = [0.62_f32, 0.84, 0.96];
    for p in px.chunks_exact_mut(4) {
        for c in 0..3 {
            p[c] = (p[c] as f32 * tint[c]).round().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(())
}

fn recolor_green_vertex(c: [f32; 4]) -> [f32; 4] {
    let (h, s, v) = rgb_to_hsv(c[0], c[1], c[2]);
    if (105.0..=195.0).contains(&h) && s > 0.14 {
        let [r, g, b] = hsv_to_rgb(205.0, (s * 0.72).clamp(0.34, 0.62), (v * 1.12).min(1.0));
        [r, g, b, c[3]]
    } else {
        c
    }
}

fn vec3(name: &str, rgb: [f64; 3]) -> VmatEdit {
    VmatEdit::Vector {
        name: name.to_owned(),
        value: [rgb[0], rgb[1], rgb[2], 0.0],
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let base = args.next().expect("base pak01_dir.vpk");
    let skin = args.next().expect("glamorous geist addon VPK");
    let out = args.next().expect("overlay output VPK");
    let preview = args.next().expect("preview PNG");

    let clothes_bytes = vpkmerge_core::read_vpk_entry(&base, CLOTHES_COLOR)?;
    let mut clothes = morphic::decode(&clothes_bytes)?;
    recolor_green_fabric(&mut clothes)?;
    if let ImageData::Rgba8(ref pixels) = clothes.data {
        image::RgbaImage::from_raw(clothes.width, clothes.height, pixels.clone())
            .expect("valid clothes image")
            .save(&preview)?;
    }
    let new_clothes = morphic::replace_mip_chain(&clothes_bytes, &clothes)?;

    let veil_color_bytes = vpkmerge_core::read_vpk_entry(&skin, VEIL_COLOR)?;
    let mut veil_color = morphic::decode(&veil_color_bytes)?;
    tint_neutral_hat_texture(&mut veil_color)?;
    let new_veil_color = morphic::replace_mip_chain(&veil_color_bytes, &veil_color)?;

    // The arm has a third color layer baked into COLOR vertex attributes.
    // Recolor only green/teal vertices so skin, hair, gun, and gold trim remain
    // unchanged. The model's geometry, UVs, rig, physics, and alpha are intact.
    let mut new_model = vpkmerge_core::read_vpk_entry(&skin, MODEL)?;
    for target in morphic::model::vertex_targets(&new_model)? {
        if target.has_color {
            let (patched, _) = morphic::model::recolor_vertex_buffer(
                &new_model,
                target.block_index,
                recolor_green_vertex,
            )?;
            new_model = patched;
        }
    }

    let veil_bytes = vpkmerge_core::read_vpk_entry(&skin, VEIL_VMAT)?;
    let (new_veil, _) = patch_vmat_params(
        &veil_bytes,
        &[
            vec3("g_vColorTint1", [0.60, 0.82, 0.96]),
            vec3("g_vHighlightTint1", [0.68, 0.88, 1.00]),
            VmatEdit::Int {
                name: "F_DISABLE_NPR_OUTLINE".to_owned(),
                value: 1,
            },
        ],
    )?;

    // The most visible idle-arm green is baked into this tiny 4x4 solid-color
    // texture. Shader tint edits alone cannot replace it.
    let arm_color_bytes = vpkmerge_core::read_vpk_entry(&base, ARM_COLOR)?;
    let new_arm_color =
        vpkmerge_core::recolor_texture_hue(&arm_color_bytes, Recolor::new(205.0, 0.55, 1.25))?;

    let arm_bytes = vpkmerge_core::read_vpk_entry(&base, ARM_VMAT)?;
    let (new_arm, _) = patch_vmat_params(
        &arm_bytes,
        &[
            vec3("TextureColor1", [0.32, 0.70, 0.94]),
            vec3("TextureNprTramsissiveColor1", [0.18, 0.55, 0.86]),
            vec3("g_vSelfIllumTint1", [0.32, 0.78, 1.00]),
            vec3("g_vSelfIllumFresnelMaskTint1", [0.68, 0.90, 1.00]),
            vec3("g_vSolidOutlineTint", [0.06, 0.24, 0.42]),
        ],
    )?;

    let arm_glow_bytes = vpkmerge_core::read_vpk_entry(&base, ARM_GLOW_VMAT)?;
    let (new_arm_glow, _) = patch_vmat_params(
        &arm_glow_bytes,
        &[
            vec3("g_vColorTint1", [0.50, 0.82, 1.00]),
            vec3("g_vSelfIllumTint1", [0.38, 0.82, 1.00]),
            vec3("g_vSelfIllumFresnelMaskTint1", [0.70, 0.92, 1.00]),
        ],
    )?;

    // Blood Exchange renders a separate spectre-hand model. Its material and
    // 1024px albedo carry their own green, while the friendly particle adds a
    // green m_ConstantColor. Recolor all three so the ability matches the arm.
    let spectre_arm_bytes = vpkmerge_core::read_vpk_entry(&base, SPECTRE_ARM_VMAT)?;
    let (new_spectre_arm, _) = patch_vmat_params(
        &spectre_arm_bytes,
        &[
            vec3("g_vColorTint1", [0.60, 0.83, 0.97]),
            vec3("g_vSelfIllumTint1", [0.42, 0.80, 1.00]),
        ],
    )?;
    let spectre_color_bytes = vpkmerge_core::read_vpk_entry(&base, SPECTRE_ARM_COLOR)?;
    let new_spectre_color =
        vpkmerge_core::recolor_texture_hue(&spectre_color_bytes, Recolor::new(205.0, 0.62, 1.18))?;
    let legacy_glow_bytes = vpkmerge_core::read_vpk_entry(&base, LEGACY_ARM_GLOW_VMAT)?;
    let (new_legacy_glow, _) = patch_vmat_params(
        &legacy_glow_bytes,
        &[
            vec3("g_vColorTint1", [0.55, 0.80, 0.96]),
            vec3("g_vSelfIllumTint1", [0.36, 0.78, 1.00]),
            vec3("g_vSelfIllumFresnelMaskTint1", [0.68, 0.90, 1.00]),
        ],
    )?;
    let spectre_particle_bytes = vpkmerge_core::read_vpk_entry(&base, SPECTRE_HAND_PARTICLE)?;
    let new_spectre_particle = vpkmerge_core::recolor_particle_bytes(
        &spectre_particle_bytes,
        Recolor::new(205.0, 0.45, 1.0),
    )?
    .expect("friendly spectre-hand particle has a green m_ConstantColor");

    let readme = b"Glamorous Geist - Soft Light Blue\n\
Dress and matching hat fabric recolored from green/teal to powder blue.\n\
Hat crown tinted to match. Animated arm keeps its original scrolling masks,\n\
vertex jitter, and 2*sin(2*time)+4 pulse; all contributing green shader tints\n\
were changed to coordinated blues. Built as an overlay for the original skin.\n";

    vpkmerge_core::pack(
        &[
            (MODEL, new_model.as_slice()),
            (CLOTHES_COLOR, new_clothes.as_slice()),
            (VEIL_VMAT, new_veil.as_slice()),
            (VEIL_COLOR, new_veil_color.as_slice()),
            (ARM_VMAT, new_arm.as_slice()),
            (ARM_COLOR, new_arm_color.as_slice()),
            (ARM_GLOW_VMAT, new_arm_glow.as_slice()),
            (SPECTRE_ARM_VMAT, new_spectre_arm.as_slice()),
            (SPECTRE_ARM_COLOR, new_spectre_color.as_slice()),
            (LEGACY_ARM_GLOW_VMAT, new_legacy_glow.as_slice()),
            (SPECTRE_HAND_PARTICLE, new_spectre_particle.as_slice()),
            ("GLAMOROUS_GEIST_BLUE_README.txt", readme.as_slice()),
        ],
        &out,
    )?;
    println!("wrote {out}");
    println!("wrote {preview}");
    Ok(())
}
