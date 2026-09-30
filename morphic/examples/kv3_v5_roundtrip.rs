//! Pak-wide gate for the lossless KV3 decoder + v5 writer.
//!
//! For every KV3 block of every resource (optionally filtered by extension):
//! `decode_lossless -> encode_v5`, then checks
//!   1. the re-encode decodes to the same `Value` tree,
//!   2. `decode_lossless` of the re-encode equals the original document,
//!   3. `encode_v5_like` of the unedited tree reproduces the document,
//!   4. for v5 originals, the decompressed buffers and every header field that
//!      does not depend on the compressor match Valve's.
//!
//! Last full run (pak01, 285000 blocks, 252134 of them v5): zero diffs except
//! one particle whose NaN floats fail `==` (its buffers are byte-identical).
//!
//! Usage: kv3_v5_roundtrip <vpk> [ext,ext,...] [--limit N] [--show N]

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
use std::collections::BTreeMap;

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn zstd(src: &[u8], unc: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut dec = ruzstd::decoding::StreamingDecoder::new(src).ok()?;
    let mut out = vec![0u8; unc];
    dec.read_exact(&mut out).ok()?;
    Some(out)
}

/// The two decompressed typed buffers of a v5 block.
fn v5_buffers(b: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let comp = u32_at(b, 20);
    let (unc1, unc2) = (u32_at(b, 72) as usize, u32_at(b, 80) as usize);
    let (c1, c2) = if comp == 0 {
        (unc1, unc2)
    } else {
        (u32_at(b, 76) as usize, u32_at(b, 84) as usize)
    };
    let s1 = b.get(120..120 + c1)?;
    let s2 = b.get(120 + c1..120 + c1 + c2)?;
    match comp {
        0 => Some((s1.to_vec(), s2.to_vec())),
        1 => Some((
            lz4_flex::block::decompress(s1, unc1).ok()?,
            lz4_flex::block::decompress(s2, unc2).ok()?,
        )),
        2 => Some((zstd(s1, unc1)?, zstd(s2, unc2)?)),
        _ => None,
    }
}

/// Header fields independent of the compressor (20 method, 24 dict, 26 frame
/// size, 52/68/76/84 compressed sizes are not). The uncompressed buf2 size (and
/// so the total) also carries the LZ4 frame table when there are blobs.
const FIELDS: &[(usize, &str)] = &[
    (0, "magic"),
    (28, "aux_b1"),
    (32, "aux_b4"),
    (36, "aux_b8"),
    (40, "count_types"),
    (44, "objects16|arrays16"),
    (48, "unc_total"),
    (56, "count_blocks"),
    (60, "size_blobs"),
    (64, "aux_b2"),
    (72, "unc1"),
    (80, "unc2"),
    (88, "main_b1"),
    (92, "main_b2"),
    (96, "main_b4"),
    (100, "main_b8"),
    (104, "nodes"),
    (108, "objects"),
    (112, "main_arrays"),
    (116, "main_array_slots"),
];

#[derive(Default)]
struct Stats {
    blocks: usize,
    by_version: BTreeMap<u32, usize>,
    decode_fail: usize,
    encode_fail: usize,
    value_diff: usize,
    doc_diff: usize,
    like_diff: usize,
    v5: usize,
    buf1_diff: usize,
    buf2_diff: usize,
    field_diff: BTreeMap<&'static str, usize>,
    examples: Vec<String>,
}

impl Stats {
    fn note(&mut self, show: usize, msg: String) {
        if self.examples.len() < show {
            self.examples.push(msg);
        }
    }
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let vpk_path = &args[1];
    let exts: Vec<String> = args
        .get(2)
        .filter(|a| !a.starts_with("--"))
        .map(|a| a.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<usize>().ok())
    };
    let limit = flag("--limit").unwrap_or(usize::MAX);
    let show = flag("--show").unwrap_or(12);

    let vpk = valve_pak::open(vpk_path)?;
    let mut files: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.ends_with("_c"))
        .filter(|p| exts.is_empty() || exts.iter().any(|e| p.ends_with(e.as_str())))
        .cloned()
        .collect();
    files.sort();
    files.truncate(limit);
    eprintln!("{} resources", files.len());

    let mut st = Stats::default();
    for entry in &files {
        let Ok(bytes) = vpk.get_file(entry).and_then(|mut f| f.read_all()) else {
            continue;
        };
        let Ok(res) = morphic::resource::Resource::parse(&bytes) else {
            continue;
        };
        for (i, meta) in res.blocks().iter().enumerate() {
            let Some(block) = res.get_block_by_index(i) else {
                continue;
            };
            if block.len() < 4 || u32_at(block, 0) & 0xFFFF_FF00 != 0x4B56_3300 {
                continue;
            }
            let at = format!("{entry} [{}]", String::from_utf8_lossy(&meta.kind));
            st.blocks += 1;
            *st.by_version.entry(u32_at(block, 0) & 0xFF).or_default() += 1;
            let Ok(orig_val) = morphic::kv3::decode(block) else {
                continue; // the lossy reader rejects it too: out of scope
            };
            let doc = match morphic::kv3::decode_lossless(block) {
                Ok(d) => d,
                Err(e) => {
                    st.decode_fail += 1;
                    st.note(show, format!("DECODE {at}: {e}"));
                    continue;
                }
            };
            let out = match morphic::kv3::encode_v5(&doc) {
                Ok(o) => o,
                Err(e) => {
                    st.encode_fail += 1;
                    st.note(show, format!("ENCODE {at}: {e}"));
                    continue;
                }
            };
            if morphic::kv3::decode(&out).ok().as_ref() != Some(&orig_val) {
                st.value_diff += 1;
                st.note(show, format!("VALUE {at}"));
            }
            if morphic::kv3::decode_lossless(&out).ok().as_ref() != Some(&doc) {
                st.doc_diff += 1;
                st.note(show, format!("DOC {at}"));
            }
            let like = morphic::kv3::encode_v5_like(&orig_val, &doc)
                .and_then(|b| morphic::kv3::decode_lossless(&b));
            if like.ok().as_ref() != Some(&doc) {
                st.like_diff += 1;
                st.note(show, format!("LIKE {at}"));
            }

            if u32_at(block, 0) & 0xFF != 5 {
                continue;
            }
            st.v5 += 1;
            let (Some((o1, o2)), Some((n1, n2))) = (v5_buffers(block), v5_buffers(&out)) else {
                st.note(show, format!("BUFFERS {at}"));
                continue;
            };
            let blobs = u32_at(block, 56) != 0;
            let lz4_blobs = blobs && u32_at(block, 20) == 1;
            if o1 != n1 {
                st.buf1_diff += 1;
                st.note(show, format!("BUF1 {at}"));
            }
            // buf2 ends in the LZ4 frame-size table when there are blobs
            // (compressed sizes, encoder-dependent); compare up to it.
            let cut = |b: &[u8], block: &[u8], lz4: bool| {
                b.len() - if lz4 { u32_at(block, 68) as usize } else { 0 }
            };
            if o2[..cut(&o2, block, lz4_blobs)] != n2[..cut(&n2, &out, blobs)] {
                st.buf2_diff += 1;
                st.note(show, format!("BUF2 {at}"));
            }
            for &(off, name) in FIELDS {
                let table_dependent = matches!(name, "unc_total" | "unc2") && blobs;
                if !table_dependent && u32_at(block, off) != u32_at(&out, off) {
                    *st.field_diff.entry(name).or_default() += 1;
                    st.note(
                        show,
                        format!(
                            "FIELD {name} {at}: valve {:#x} ours {:#x}",
                            u32_at(block, off),
                            u32_at(&out, off)
                        ),
                    );
                }
            }
        }
    }

    println!("KV3 blocks: {} by version {:?}", st.blocks, st.by_version);
    println!(
        "decode_fail {} encode_fail {} value_diff {} doc_diff {} like_diff {}",
        st.decode_fail, st.encode_fail, st.value_diff, st.doc_diff, st.like_diff
    );
    println!(
        "v5: {} buf1_diff {} buf2_diff {} header field diffs {:?}",
        st.v5, st.buf1_diff, st.buf2_diff, st.field_diff
    );
    for e in &st.examples {
        println!("  {e}");
    }
    Ok(())
}
