//! Import a Blender-authored animation (`.glb`) onto a Deadlock NM clip and pack
//! it into an installable addon VPK. This is the third command of the SDK-free
//! authoring loop (see `docs/anim-authoring-pipeline.md`): export the slot's clip
//! to a `.glb` (`nm_clip_preview_glb`), keyframe the armature in Blender, then run
//! this to splice the authored motion back into the compiled clip at its slot path.
//!
//! The animation maps onto the clip's bones **by node name** (do not rename or
//! reorder bones in Blender) and is time-stretched onto the slot's frame count.
//! By default it uses the engine-confirmed **v5 in-place** path
//! (`import_glb_onto_nm_clip` -> `reencode_nm_clip`): rotation tracks may be edited
//! or added (a static bone becomes animated), while translation/scale are edited
//! only where the slot already animates them. `--full` rebuilds the clip instead
//! (`import_glb_onto_nm_clip_full` -> `reencode_nm_clip_full`, KV3 v5 with Valve's
//! typed framing): it also **adds** translation/scale channels, and `--frames N`
//! resamples the slot to N frames over the same duration (`--full` is in-game
//! confirmed for rotation edits, added translation channels, and `--frames`). Pick a slot whose bone mask un-masks the bones you animated
//! (a full-body idle for whole-body motion, an upper-body slot for arm/torso work).
//!
//! Usage:
//!   cargo run --release -p vpkmerge-core --example nm_clip_import_glb -- \
//!       <pak01_dir.vpk> <clip_entry.vnmclip_c> <anim.glb> <out_dir.vpk> \
//!       [extra_slot_entry.vnmclip_c ...] [--mesh <mesh_entry.vmdl_c> --preview <out.glb>] \
//!       [--full [--frames N]]
//!
//! Example (author Yamato's reload slot, also write a preview GLB):
//!   ... pak01_dir.vpk \
//!       models/heroes_wip/yamato/clips/reload_idle_quick.vnmclip_c my_taunt.glb \
//!       yamato_taunt_dir.vpk \
//!       models/heroes_wip/yamato/clips/reload_idle.vnmclip_c \
//!       --mesh models/heroes_staging/yamato_v2/yamato.vmdl_c --preview taunt_preview.glb

// File paths and flags in the usage doc above are not Rust items.
#![allow(clippy::doc_markdown)]

use anyhow::{Context, Result};
use morphic::model::{
    decode, decode_nm_clip, decode_nm_skeleton, import_glb_onto_nm_clip,
    import_glb_onto_nm_clip_full, nm_clip_to_clip, to_glb,
};

fn main() -> Result<()> {
    let mut positional: Vec<String> = Vec::new();
    let mut mesh_entry: Option<String> = None;
    let mut preview_out: Option<String> = None;
    let mut full = false;
    let mut frames: Option<u32> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mesh" => mesh_entry = Some(args.next().context("--mesh needs a value")?),
            "--preview" => preview_out = Some(args.next().context("--preview needs a value")?),
            "--full" => full = true,
            "--frames" => {
                frames = Some(args.next().context("--frames needs a value")?.parse()?);
            }
            _ => positional.push(arg),
        }
    }

    let pak = positional.first().context("missing arg: pak01_dir.vpk")?;
    let clip_entry = positional.get(1).context("missing arg: clip entry")?;
    let glb_path = positional.get(2).context("missing arg: anim.glb")?;
    let out_vpk = positional.get(3).context("missing arg: out_dir.vpk")?;
    // Any further positionals are extra slot entries to override with the same clip.
    let extra_slots: Vec<&String> = positional.iter().skip(4).collect();

    let clip_bytes = vpkmerge_core::read_vpk_entry(pak, clip_entry)
        .with_context(|| format!("reading clip entry {clip_entry}"))?;
    let clip = decode_nm_clip(&clip_bytes).context("decode clip")?;
    let skel = decode_nm_skeleton(&vpkmerge_core::read_vpk_entry(
        pak,
        &resolve_skel(&clip.skeleton_ref),
    )?)
    .context("decode vnmskel")?;
    let glb = std::fs::read(glb_path).with_context(|| format!("reading {glb_path}"))?;

    println!(
        "slot {clip_entry}\n  {} frames | {} bones | additive: {}",
        clip.frame_count,
        clip.tracks.len(),
        clip.additive
    );

    // Import the authored animation onto the slot (first/only glTF animation).
    anyhow::ensure!(full || frames.is_none(), "--frames needs --full");
    let patched = if full {
        import_glb_onto_nm_clip_full(&clip_bytes, &skel, &glb, None, frames)
    } else {
        import_glb_onto_nm_clip(&clip_bytes, &skel, &glb, None)
    }
    .context("importing glb animation onto the clip")?;

    // Report what landed: how many bone tracks now carry an animated rotation
    // versus the original, so a caller can see the authored motion took effect.
    let redec = decode_nm_clip(&patched).context("re-decode patched clip")?;
    let before = clip.tracks.iter().filter(|t| t.rotations.is_some()).count();
    let after = redec
        .tracks
        .iter()
        .filter(|t| t.rotations.is_some())
        .count();
    println!("  animated-rotation tracks: {before} -> {after}");
    let moving = |c: &morphic::model::NmClip| {
        c.tracks
            .iter()
            .filter(|t| t.translations.is_some() || t.scales.is_some())
            .count()
    };
    println!(
        "  animated translation/scale tracks: {} -> {} | frames {} -> {}",
        moving(&clip),
        moving(&redec),
        clip.frame_count,
        redec.frame_count
    );

    // Optional preview GLB so the authored motion can be eyeballed before install.
    if let (Some(mesh_entry), Some(preview_out)) = (&mesh_entry, &preview_out) {
        let mut model =
            decode(&vpkmerge_core::read_vpk_entry(pak, mesh_entry)?).context("decode mesh")?;
        model.animations = vec![nm_clip_to_clip(&redec, &skel, &model.skeleton, "imported")];
        let preview = to_glb(&model).context("write preview glb")?;
        std::fs::write(preview_out, &preview)?;
        println!("wrote preview {preview_out} ({} bytes)", preview.len());
    }

    // Pack the edited clip at its slot path (+ any extra slots) into one addon VPK.
    let mut entries: Vec<(&str, &[u8])> = vec![(clip_entry.as_str(), patched.as_slice())];
    for slot in &extra_slots {
        entries.push((slot.as_str(), patched.as_slice()));
    }
    vpkmerge_core::pack(&entries, out_vpk)?;
    println!(
        "\npacked imported animation at {} slot(s) -> {out_vpk}",
        entries.len()
    );
    println!(
        "install: copy to game/citadel/addons/ as a free pakNN_dir.vpk; trigger the slot in-game"
    );
    Ok(())
}

/// The clip's `m_skeleton` is an uncompiled ref (`models/.../h.vnmskel`); the VPK
/// entry is the compiled `_c`. Add the suffix if it is missing.
fn resolve_skel(reference: &str) -> String {
    if reference.ends_with("_c") {
        reference.to_owned()
    } else {
        format!("{reference}_c")
    }
}
