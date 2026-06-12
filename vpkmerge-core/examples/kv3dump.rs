//! Dump a KV3-resource VPK entry as pretty-printed Rust Debug text, for
//! diffing two builds of the same particle/material.
//!
//! Usage: cargo run --release --example kv3dump -- <vpk> <entry>

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let vpk = a.next().expect("vpk");
    let entry = a.next().expect("entry");
    let bytes = vpkmerge_core::read_vpk_entry(&vpk, &entry)?;
    let doc = morphic::decode_kv3_resource(&bytes)?;
    println!("{doc:#?}");
    Ok(())
}
