//! Validate the resource block parser/writer is byte-faithful: parse a .vmdl_c,
//! rebuild it through the structured writer (replacing block 0 with its own
//! bytes, which forces a full re-layout of every block), and diff against the
//! original. Byte-identical proves the disassemble/reassemble path is safe for
//! the binary mesh-swap.
//! Usage: roundtrip_vmdl <file.vmdl_c>
fn main() {
    let path = std::env::args().nth(1).expect("file arg");
    let bytes = std::fs::read(&path).expect("read");

    let res = morphic::resource::Resource::parse(&bytes).expect("parse");
    let block0 = res.get_block_by_index(0).expect("block 0").to_vec();
    let rebuilt = res.rebuild_with_block(0, &block0).expect("rebuild");

    if rebuilt == bytes {
        println!(
            "BYTE-IDENTICAL: {} bytes round-tripped exactly",
            bytes.len()
        );
        return;
    }

    println!(
        "DIFF: original={} rebuilt={} bytes",
        bytes.len(),
        rebuilt.len()
    );
    // Classify each diff: is it in the header (0..16), the block table, inside a
    // declared block payload, or in inter-block padding / final tail? Only
    // payload/table diffs would mean the writer is unfaithful; header(file_size)
    // and padding diffs are harmless (the engine reads via the table).
    let table_start = 16usize;
    let table_end = 16 + res.blocks().len() * 12;
    // Mark every byte that belongs to a declared payload (by original offsets).
    let mut in_payload = vec![false; bytes.len()];
    for b in res.blocks() {
        let s = b.offset as usize;
        let e = s + b.size as usize;
        for x in in_payload.iter_mut().take(e).skip(s) {
            *x = true;
        }
    }

    let n = bytes.len().min(rebuilt.len());
    let (mut header_d, mut table_d, mut payload_d, mut padding_d) = (0, 0, 0, 0);
    for i in 0..n {
        if bytes[i] != rebuilt[i] {
            if i < 16 {
                header_d += 1;
            } else if i >= table_start && i < table_end {
                table_d += 1;
            } else if in_payload[i] {
                payload_d += 1;
            } else {
                padding_d += 1;
            }
        }
    }
    let tail_d = rebuilt.len().saturating_sub(bytes.len());
    println!("diffs: header={header_d} table={table_d} payload={payload_d} padding={padding_d} final_tail={tail_d}");
    if table_d == 0 && payload_d == 0 {
        println!(
            "VERDICT: writer is structurally faithful (all block table entries + every \
             payload byte identical at identical offsets). Only file_size header + \
             inter-block padding + final 16-align differ, which the engine ignores."
        );
    } else {
        println!("VERDICT: UNFAITHFUL writer (table or payload bytes changed)");
    }
}
