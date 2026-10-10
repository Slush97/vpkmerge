//! Lossless KV3 decode + v5 writer against committed Valve files.
//!
//! The pak-wide version of this gate is `examples/kv3_v5_roundtrip.rs` (every
//! KV3 block in pak01: decompressed buffers and header byte-identical). These
//! fixtures cover the shapes that matter here in CI: soundevents, blob-bearing
//! NM clips, and one- and two-blob materials.

use morphic::kv3::{self, Document, Val};
use morphic::model::{decode_nm_clip, reencode_nm_clip_full};

const FIXTURES: &[&str] = &[
    "kv3/gigawatt.vsndevts_c",
    "nm/yamato_flinch_back.vnmclip_c",
    "nm/yamato_reload_idle_quick.vnmclip_c",
    "nm/yamato_rope_climb_idle.vnmclip_c",
    "nm/yamato_ui_hero_select.vnmclip_c",
    "material/necro_gravestone.vmat_c",
    "material/necro_hands.vmat_c",
    "material/necro_picker_hand_effect.vmat_c",
    "material/picker_hand_glow.vmat_c",
    "material/vindicta_headv2.vmat_c",
];

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"))
}

fn data(bytes: &[u8]) -> Vec<u8> {
    morphic::kv3_resource_data_block(bytes).expect("DATA block")
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// The two decompressed typed buffers of an LZ4 v5 block.
fn buffers(b: &[u8]) -> (Vec<u8>, Vec<u8>) {
    assert_eq!(u32_at(b, 0), 0x4B56_3305, "v5 block");
    assert_eq!(u32_at(b, 20), 1, "LZ4 block");
    let (unc1, c1, unc2, c2) = (
        u32_at(b, 72) as usize,
        u32_at(b, 76) as usize,
        u32_at(b, 80) as usize,
        u32_at(b, 84) as usize,
    );
    let lz4 = |src: &[u8], unc: usize| lz4_flex::block::decompress(src, unc).expect("LZ4");
    (
        lz4(&b[120..120 + c1], unc1),
        lz4(&b[120 + c1..120 + c1 + c2], unc2),
    )
}

#[test]
fn v5_writer_reproduces_valve_buffers_and_header() {
    for name in FIXTURES {
        let orig = data(&fixture(name));
        let doc = kv3::decode_lossless(&orig).unwrap_or_else(|e| panic!("{name}: {e}"));
        let out = kv3::encode_v5(&doc).unwrap_or_else(|e| panic!("{name}: {e}"));

        assert_eq!(
            kv3::decode_lossless(&out).expect("re-decode"),
            doc,
            "{name}: document"
        );
        let blobs = u32_at(&orig, 56) != 0;
        let (o1, o2) = buffers(&orig);
        let (n1, n2) = buffers(&out);
        assert_eq!(o1, n1, "{name}: aux buffer");
        // With blobs, buf2 ends in the LZ4 frame-size table (compressed sizes,
        // encoder-dependent); everything before it must match.
        let cut =
            |b: &[u8], block: &[u8]| b.len() - if blobs { u32_at(block, 68) as usize } else { 0 };
        assert_eq!(
            o2[..cut(&o2, &orig)],
            n2[..cut(&n2, &out)],
            "{name}: main buffer"
        );
        // Every header field that does not depend on the compressor.
        for off in (28..=48)
            .step_by(4)
            .chain([56, 60, 64, 72, 80])
            .chain((88..=116).step_by(4))
        {
            assert_eq!(
                u32_at(&orig, off),
                u32_at(&out, off),
                "{name}: header @{off}"
            );
        }
    }
}

#[test]
fn shaping_an_unedited_tree_reproduces_the_document() {
    for name in FIXTURES {
        let orig = data(&fixture(name));
        let doc = kv3::decode_lossless(&orig).expect("decode");
        let out = kv3::encode_v5_like(&doc.to_value(), &doc).expect("encode like");
        assert_eq!(
            kv3::decode_lossless(&out).expect("re-decode"),
            doc,
            "{name}"
        );
    }
}

fn root_tag(doc: &Document, key: &str) -> (u8, Option<u8>) {
    let n = doc.root.get(key).unwrap_or_else(|| panic!("no {key}"));
    match &n.val {
        Val::Typed { sub, .. } => (n.tag, Some(*sub)),
        _ => (n.tag, None),
    }
}

/// Every sample doubled in place: a channel resampled to twice the frames.
fn dup<T: Copy>(v: &mut Option<Vec<T>>) {
    if let Some(v) = v {
        *v = v.iter().flat_map(|x| [*x, *x]).collect();
    }
}

#[test]
fn full_nm_reencode_keeps_valve_typed_framing() {
    // Double the frame count (the edit the in-place patcher cannot do) and check
    // the rebuilt clip is v5 with the same typed framing Valve ships: offsets as
    // an auxiliary-buffer UINT32 array, track settings as a typed OBJECT array.
    let bytes = fixture("nm/yamato_reload_idle_quick.vnmclip_c");
    let orig_doc = kv3::decode_lossless(&data(&bytes)).expect("decode original");
    let mut clip = decode_nm_clip(&bytes).expect("decode clip");
    for t in &mut clip.tracks {
        dup(&mut t.rotations);
        dup(&mut t.translations);
        dup(&mut t.scales);
    }
    clip.frame_count *= 2;

    let out = reencode_nm_clip_full(&bytes, &clip).expect("full re-encode");
    let block = data(&out);
    assert_eq!(u32_at(&block, 0), 0x4B56_3305, "rebuilt DATA is v5");
    let doc = kv3::decode_lossless(&block).expect("decode rebuilt");
    for key in [
        "m_compressedPoseOffsets",
        "m_trackCompressionSettings",
        "m_nNumFrames",
    ] {
        assert_eq!(
            root_tag(&doc, key),
            root_tag(&orig_doc, key),
            "{key} framing"
        );
    }
    let round = decode_nm_clip(&out).expect("re-decode clip");
    assert_eq!(round.frame_count, clip.frame_count);
    assert_eq!(round.tracks.len(), clip.tracks.len());
}
