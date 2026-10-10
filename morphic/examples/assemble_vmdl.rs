//! Assemble a Source 2 resource from blocks taken from two source files.
//! Lets us splice vanilla's envelope blocks (ANIM/ASEQ/AGRP/PHYS/DSTF, which
//! carry the baked hero camera) onto the working hat-man mesh model.
//!
//! Usage:
//!   assemble_vmdl <out> <base.vmdl_c> <donor.vmdl_c> <spec>
//!
//! spec = comma-separated tokens, in final block order. Each token:
//!   b#<n>   base block at declaration index n
//!   d#<n>   donor block at declaration index n
//!   bFOURCC base block, first by FOURCC (e.g. bDATA, bCTRL)
//!   dFOURCC donor block, first by FOURCC (e.g. dANIM, dPHYS)
//! resource_version is copied from <base>.
//!
//! Mesh buffers are referenced by declaration INDEX from CTRL, so keep the
//! mesh blocks at the indices the chosen CTRL expects (i.e. list them first,
//! same order as the file CTRL came from).

use morphic::resource::Resource;

fn align16(n: usize) -> usize {
    (n + 15) & !15
}

fn pick<'a>(res: &Resource<'a>, tok: &str) -> (&'a [u8], [u8; 4]) {
    // tok is the part after the b/d prefix.
    if let Some(rest) = tok.strip_prefix('#') {
        let idx: usize = rest.parse().expect("index");
        let b = res.blocks().get(idx).expect("block index");
        (res.get_block_by_index(idx).expect("payload"), b.kind)
    } else {
        let fourcc = tok.as_bytes();
        assert!(fourcc.len() == 4, "fourcc must be 4 chars: {tok}");
        let mut k = [0u8; 4];
        k.copy_from_slice(fourcc);
        let payload = res.find_block(k).expect("fourcc block not found");
        (payload, k)
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let out = &a[1];
    let base_bytes = std::fs::read(&a[2]).expect("read base");
    let donor_bytes = std::fs::read(&a[3]).expect("read donor");
    let spec = &a[4];

    let base = Resource::parse(&base_bytes).expect("parse base");
    let donor = Resource::parse(&donor_bytes).expect("parse donor");
    let resource_version = u16::from_le_bytes([base_bytes[6], base_bytes[7]]);

    // Resolve each token to (kind, payload).
    let mut blocks: Vec<([u8; 4], &[u8])> = Vec::new();
    for tok in spec.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        let (which, rest) = tok.split_at(1);
        let (payload, kind) = match which {
            "b" => pick(&base, rest),
            "d" => pick(&donor, rest),
            other => panic!("token must start with b/d: {other}"),
        };
        blocks.push((kind, payload));
    }

    // Lay out: header(16) + table(12/block) aligned to 16, then each payload 16-aligned.
    let block_count = blocks.len();
    let table_len = block_count * 12;
    let mut cursor = align16(16 + table_len);
    let mut offsets = Vec::with_capacity(block_count);
    for (_, p) in &blocks {
        offsets.push(cursor);
        cursor = align16(cursor + p.len());
    }
    let total = cursor;

    let mut o = vec![0u8; total];
    o[0..4].copy_from_slice(&(u32::try_from(total).unwrap()).to_le_bytes());
    o[4..6].copy_from_slice(&12u16.to_le_bytes());
    o[6..8].copy_from_slice(&resource_version.to_le_bytes());
    o[8..12].copy_from_slice(&8u32.to_le_bytes());
    o[12..16].copy_from_slice(&(u32::try_from(block_count).unwrap()).to_le_bytes());
    for (i, ((kind, p), off)) in blocks.iter().zip(&offsets).enumerate() {
        let e = 16 + i * 12;
        o[e..e + 4].copy_from_slice(kind);
        let ofp = e + 4;
        let rel = u32::try_from(off - ofp).unwrap();
        o[ofp..ofp + 4].copy_from_slice(&rel.to_le_bytes());
        o[ofp + 4..ofp + 8].copy_from_slice(&(u32::try_from(p.len()).unwrap()).to_le_bytes());
        o[*off..*off + p.len()].copy_from_slice(p);
    }

    std::fs::write(out, &o).expect("write");
    println!("wrote {out} ({total} bytes, {block_count} blocks):");
    for (i, (kind, p)) in blocks.iter().enumerate() {
        println!("  #{i:<2} {} {}", String::from_utf8_lossy(kind), p.len());
    }
}
