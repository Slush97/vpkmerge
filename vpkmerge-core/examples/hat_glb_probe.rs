// Throwaway: report a hat GLB's native bbox and what the YUp+swizzle pipeline
// (read_glb_primitives YUp, then merge-loop [x,z,-y]) does to its up-axis.
// usage: cargo run --release --example hat_glb_probe -- <hat.glb>
use vpkmerge_core::soul_import_clone::read_glb_primitives;
use vpkmerge_core::SoulOrient;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("arg1: hat.glb");
    let glb = std::fs::read(&path)?;
    let (prims, _) = read_glb_primitives(&glb, SoulOrient::YUp, None)?;

    // Native (post read_glb_primitives YUp = identity) bbox.
    let (mut mn, mut mx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    // After the merge-loop swizzle [x, z, -y].
    let (mut sn, mut sx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    let mut n = 0usize;
    for p in &prims {
        for v in &p.vertex_buffer.positions {
            let s = [v[0], v[2], -v[1]];
            for k in 0..3 {
                mn[k] = mn[k].min(v[k]);
                mx[k] = mx[k].max(v[k]);
                sn[k] = sn[k].min(s[k]);
                sx[k] = sx[k].max(s[k]);
            }
            n += 1;
        }
    }
    println!("{path}: {n} verts, {} prim(s)", prims.len());
    println!(
        "NATIVE  x[{:.2},{:.2}] y[{:.2},{:.2}] z[{:.2},{:.2}]  (spans {:.2}/{:.2}/{:.2})",
        mn[0],
        mx[0],
        mn[1],
        mx[1],
        mn[2],
        mx[2],
        mx[0] - mn[0],
        mx[1] - mn[1],
        mx[2] - mn[2]
    );
    println!(
        "SWIZZLED[x,z,-y] -> Source  x[{:.2},{:.2}] y[{:.2},{:.2}] z[{:.2},{:.2}]  (spans {:.2}/{:.2}/{:.2})",
        sn[0], sx[0], sn[1], sx[1], sn[2], sx[2],
        sx[0]-sn[0], sx[1]-sn[1], sx[2]-sn[2]
    );
    let tallest_native = ["X", "Y", "Z"][argmax([mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]])];
    println!(
        "tallest NATIVE axis = {tallest_native} (hats are usually tall along their crown axis)"
    );
    Ok(())
}

fn argmax(a: [f32; 3]) -> usize {
    let mut m = 0;
    for i in 1..3 {
        if a[i] > a[m] {
            m = i;
        }
    }
    m
}
