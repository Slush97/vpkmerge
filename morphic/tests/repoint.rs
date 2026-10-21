//! `repoint_resource_refs` and the `RERL` codec on committed Valve fixtures:
//! particles (KV3 v4 and v5) and materials (plain and blobbed v5, plain v4).
//!
//! Every case checks the same promises: the decoded tree changes only at the
//! redirected strings, the lossless wire form (tags, flags, typed-array framing)
//! changes nowhere else, every block but `DATA` and `RERL` is byte-identical, and
//! `RERL` stays sorted, de-duplicated and id-consistent. Set
//! `MORPHIC_REPOINT_DUMP` to a directory to also write each output there, for the
//! strict loader (`morphic-oracle validate --file`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use morphic::kv3::{decode_lossless, Node, Val};
use morphic::resource::{read_rerl, resource_id, write_rerl, Resource};
use morphic::{repoint_resource_refs, DecodeError};

const FLARE: &str = "materials/particle/particle_flares/particle_flare_001.vtex";
const TRAIL: &str = "particles/weapon_fx/grenade_frag_trail.vpcf";
const MASK: &str = "materials/default/default_mask_tga_344101f8.vtex";
const NORMAL: &str = "materials/default/default_normal_tga_7be61377.vtex";
const GLOW: &str = "materials/effects/glow_radial_trans_psd_867980fd.vtex";

fn fixture(rel: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(rel),
    )
    .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn block(bytes: &[u8], kind: [u8; 4]) -> Vec<u8> {
    Resource::parse(bytes)
        .unwrap()
        .find_block(kind)
        .unwrap()
        .to_vec()
}

fn rerl(bytes: &[u8]) -> Vec<(u64, String)> {
    read_rerl(&block(bytes, *b"RERL")).unwrap()
}

fn map(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(o, n)| ((*o).to_string(), (*n).to_string()))
        .collect()
}

fn substitute_wire(node: &mut Node, pairs: &[(&str, &str)]) {
    fn val(v: &mut Val, pairs: &[(&str, &str)]) {
        match v {
            Val::String(s) => {
                if let Some((_, new)) = pairs.iter().find(|(o, _)| s.eq_ignore_ascii_case(o)) {
                    *s = new.to_ascii_lowercase();
                }
            }
            Val::Array(items) => items.iter_mut().for_each(|n| substitute_wire(n, pairs)),
            Val::Typed { items, .. } => items.iter_mut().for_each(|i| val(i, pairs)),
            Val::Object(members) => members
                .iter_mut()
                .for_each(|(_, n)| substitute_wire(n, pairs)),
            _ => {}
        }
    }
    val(&mut node.val, pairs);
}

/// Repoint `pairs` in `rel`, run the checks every case shares, and return the
/// original, the output and the redirected-field count.
fn repoint(rel: &str, pairs: &[(&str, &str)]) -> (Vec<u8>, Vec<u8>, usize) {
    let original = fixture(rel);
    let (out, count) = repoint_resource_refs(&original, &map(pairs)).unwrap();

    let mut want_tree = morphic::decode_kv3_resource(&original).unwrap();
    let mut want_count = 0;
    want_tree.for_each_string_mut(&mut |s| {
        if let Some((_, new)) = pairs.iter().find(|(o, _)| s.eq_ignore_ascii_case(o)) {
            *s = new.to_ascii_lowercase();
            want_count += 1;
        }
    });
    assert_eq!(morphic::decode_kv3_resource(&out).unwrap(), want_tree);
    assert_eq!(count, want_count);

    let data_of = |bytes: &[u8]| block(bytes, *b"DATA");
    let mut want_doc = decode_lossless(&data_of(&original)).unwrap();
    substitute_wire(&mut want_doc.root, pairs);
    let got_doc = decode_lossless(&data_of(&out)).unwrap();
    assert_eq!(got_doc.root, want_doc.root, "wire tags/flags/framing moved");

    let (before, after) = (
        Resource::parse(&original).unwrap(),
        Resource::parse(&out).unwrap(),
    );
    assert_eq!(before.blocks().len(), after.blocks().len());
    for (i, b) in before.blocks().iter().enumerate() {
        assert_eq!(b.kind, after.blocks()[i].kind);
        if &b.kind != b"DATA" && &b.kind != b"RERL" {
            assert_eq!(before.get_block_by_index(i), after.get_block_by_index(i));
        }
    }

    let refs = rerl(&out);
    assert!(refs.iter().all(|(id, n)| *id == resource_id(n)));
    assert!(refs.windows(2).all(|w| w[0].1 < w[1].1), "sorted, no dupes");

    if let Some(dir) = std::env::var_os("MORPHIC_REPOINT_DUMP") {
        static DUMPS: AtomicUsize = AtomicUsize::new(0);
        let (stem, ext) = rel.rsplit('/').next().unwrap().split_once('.').unwrap();
        let n = DUMPS.fetch_add(1, Ordering::Relaxed);
        std::fs::write(PathBuf::from(dir).join(format!("{stem}_{n}.{ext}")), &out).unwrap();
    }
    (original, out, count)
}

/// `RERL` of `original` without `removed`, plus `added`, in Valve's order.
fn expect_rerl(original: &[u8], removed: &[&str], added: &[&str]) -> Vec<(u64, String)> {
    let mut refs: Vec<(u64, String)> = rerl(original)
        .into_iter()
        .filter(|(_, n)| !removed.contains(&n.as_str()))
        .chain(added.iter().map(|n| (resource_id(n), (*n).to_string())))
        .collect();
    refs.sort_by(|a, b| a.1.cmp(&b.1));
    refs
}

#[test]
fn resource_id_matches_stored_ids() {
    assert_eq!(resource_id(MASK), 0xb75c_3785_9897_7cd0);
    assert_eq!(resource_id(FLARE), 0x8009_fbc8_533a_78ab);
}

#[test]
fn rerl_round_trips_every_fixture() {
    let fixtures = [
        "particle/grenade_frag_fx.vpcf_c",
        "particle/wraith_rapid_fire_buff.vpcf_c",
        "particle/wraith_rapid_fire_cast_proj.vpcf_c",
        "particle/wraith_ult_range_indicator_projected.vpcf_c",
        "material/haze_hair_cable.vmat_c",
        "material/necro_hands.vmat_c",
        "material/necro_picker_hand_effect.vmat_c",
        "material/necro_gravestone.vmat_c",
        "material/picker_hand_glow.vmat_c",
        "material/vindicta_headv2.vmat_c",
        "soul/soul_material_donor.vmat_c",
    ];
    for rel in fixtures {
        let stored = block(&fixture(rel), *b"RERL");
        let refs = read_rerl(&stored).unwrap();
        assert!(!refs.is_empty(), "{rel}");
        assert!(refs.iter().all(|(id, n)| *id == resource_id(n)), "{rel}");
        assert!(refs.windows(2).all(|w| w[0].1 < w[1].1), "{rel}");

        // Valve starts the table at 8, or at 12 behind four zero bytes when that
        // 8-aligns it in the file; `write_rerl` always emits the 8 form.
        let mut canonical = stored.clone();
        if canonical[0] == 12 {
            canonical.drain(8..12);
            canonical[0] = 8;
        }
        assert_eq!(write_rerl(&refs), canonical, "{rel}");
    }
}

#[test]
fn v4_particle_texture_moves_to_its_sorted_slot() {
    let new = "zz/flare.vtex";
    let (original, out, count) = repoint("particle/grenade_frag_fx.vpcf_c", &[(FLARE, new)]);
    assert_eq!(count, 1);
    assert_eq!(rerl(&out), expect_rerl(&original, &[FLARE], &[new]));
    assert_eq!(rerl(&out).last().unwrap().1, new);
}

#[test]
fn v4_particle_child_reference() {
    let new = "aaa/trail.vpcf";
    let (original, out, count) = repoint("particle/grenade_frag_fx.vpcf_c", &[(TRAIL, new)]);
    assert_eq!(count, 1);
    assert_eq!(rerl(&out), expect_rerl(&original, &[TRAIL], &[new]));
    assert_eq!(rerl(&out).first().unwrap().1, new);
}

#[test]
fn v5_particle_child_reference() {
    let old = "particles/abilities/wraith/wraith_rapid_fire_buff_muzzle_flame.vpcf";
    let new = "particles/abilities/wraith/zz_flame.vpcf";
    let (original, out, count) = repoint("particle/wraith_rapid_fire_buff.vpcf_c", &[(old, new)]);
    assert_eq!(count, 1);
    assert_eq!(rerl(&out), expect_rerl(&original, &[old], &[new]));
    assert_eq!(rerl(&out).len(), 3);
}

#[test]
fn particle_material_reference_v4_and_v5() {
    let cases = [
        (
            "particle/wraith_rapid_fire_cast_proj.vpcf_c",
            "materials/particle/projected/wraith_rapid_fire_ground_projected.vmat",
        ),
        (
            "particle/wraith_ult_range_indicator_projected.vpcf_c",
            "materials/particle/projected/ui_range_finder_ground_projected.vmat",
        ),
    ];
    for (rel, old) in cases {
        let new = "materials/zz/copy.vmat";
        let (original, out, count) = repoint(rel, &[(old, new)]);
        assert_eq!(count, 1, "{rel}");
        assert_eq!(rerl(&out), expect_rerl(&original, &[old], &[new]), "{rel}");
    }
}

#[test]
fn v4_material_texture_reference() {
    let old = "materials/particle/cables/haze_hair_cable_vmat_g_tcolor_79513339.vtex";
    let new = "materials/particle/cables/zz_color.vtex";
    let (original, out, count) = repoint("material/haze_hair_cable.vmat_c", &[(old, new)]);
    assert_eq!(count, 1);
    assert_eq!(rerl(&out), expect_rerl(&original, &[old], &[new]));
}

#[test]
fn texture_named_by_two_fields_is_redirected_in_both() {
    let new = "materials/zz/mask.vtex";
    let (original, out, count) = repoint("material/picker_hand_glow.vmat_c", &[(MASK, new)]);
    assert_eq!(count, 2);
    assert_eq!(rerl(&out), expect_rerl(&original, &[MASK], &[new]));
}

#[test]
fn blobbed_materials_stay_compressed() {
    let new = "materials/zz/copy.vtex";
    let cases = [
        ("material/necro_hands.vmat_c", MASK, 2),
        (
            "material/necro_picker_hand_effect.vmat_c",
            "materials/ui/shadow_grunge_trans_psd_fde5ddf2.vtex",
            1,
        ),
    ];
    for (rel, old, want) in cases {
        let (original, out, count) = repoint(rel, &[(old, new)]);
        assert_eq!(count, want, "{rel}");
        assert_eq!(rerl(&out), expect_rerl(&original, &[old], &[new]), "{rel}");

        assert!(morphic::kv3_resource_has_blobs(&out).unwrap(), "{rel}");
        let (before, after) = (block(&original, *b"DATA"), block(&out, *b"DATA"));
        let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        assert_eq!(u32_at(&after, 20), 1, "{rel}: DATA must stay LZ4");
        assert_eq!(u32_at(&after, 56), u32_at(&before, 56), "{rel}: blob count");
    }
}

#[test]
fn new_name_already_in_rerl_is_not_duplicated() {
    let (original, out, count) = repoint("material/picker_hand_glow.vmat_c", &[(GLOW, NORMAL)]);
    assert_eq!(count, 1);
    assert_eq!(rerl(&out), expect_rerl(&original, &[GLOW], &[]));
    assert_eq!(rerl(&out).len(), rerl(&original).len() - 1);
}

#[test]
fn old_entry_survives_while_a_redirect_still_names_it() {
    // MASK -> NORMAL and NORMAL -> new: the MASK fields now say NORMAL, so
    // NORMAL keeps its entry even though its own fields moved on.
    let new = "materials/zz/normal.vtex";
    let (original, out, count) = repoint(
        "material/picker_hand_glow.vmat_c",
        &[(MASK, NORMAL), (NORMAL, new)],
    );
    assert_eq!(count, 3);
    assert_eq!(rerl(&out), expect_rerl(&original, &[MASK], &[new]));
}

#[test]
fn names_match_case_insensitively_and_new_is_lowercased() {
    let (original, out, count) = repoint(
        "particle/grenade_frag_fx.vpcf_c",
        &[(
            "Materials/Particle/PARTICLE_Flares/particle_flare_001.VTEX",
            "ZZ/Flare.vtex",
        )],
    );
    assert_eq!(count, 1);
    assert_eq!(
        rerl(&out),
        expect_rerl(&original, &[FLARE], &["zz/flare.vtex"])
    );
}

#[test]
fn unreferenced_old_name_is_an_error() {
    let original = fixture("particle/grenade_frag_fx.vpcf_c");
    let err = repoint_resource_refs(&original, &map(&[("particles/nope.vpcf", "zz/a.vpcf")]))
        .unwrap_err();
    assert!(
        matches!(&err, DecodeError::MissingReference(n) if n == "particles/nope.vpcf"),
        "{err}"
    );
}
