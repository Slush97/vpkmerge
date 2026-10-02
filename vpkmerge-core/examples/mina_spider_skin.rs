use anyhow::Result;
use morphic::model::{
    replace_mesh_group_uncompressed, EditedPrimitive, PrimitiveSelection, VertexBuffer,
};
use std::collections::HashMap;
use vpkmerge_core::read_vpk_entry;
const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
include!("../../../output/mina-spider/mesh_helpers.rs");
fn add_vertex(
    v: &mut VertexBuffer,
    p: [f32; 3],
    n: [f32; 3],
    uv: [f32; 2],
    j: [u16; 4],
    w: [f32; 4],
) -> u32 {
    let i = v.positions.len() as u32;
    v.positions.push(p);
    v.normals.push(n);
    v.texcoords[0].push(uv);
    v.joints.push(j);
    v.weights.push(w);
    v.element_count += 1;
    i
}
fn ball(
    v: &mut VertexBuffer,
    idx: &mut Vec<u32>,
    c: [f32; 3],
    r: [f32; 3],
    uv: [f32; 2],
    j: [u16; 4],
    w: [f32; 4],
) {
    let base = v.element_count as u32;
    let (rows, cols) = (10, 16);
    for a in 0..=rows {
        let t = std::f32::consts::PI * a as f32 / rows as f32;
        for b in 0..=cols {
            let p = std::f32::consts::TAU * b as f32 / cols as f32;
            let n = [t.sin() * p.cos(), t.sin() * p.sin(), t.cos()];
            add_vertex(
                v,
                [c[0] + n[0] * r[0], c[1] + n[1] * r[1], c[2] + n[2] * r[2]],
                n,
                uv,
                j,
                w,
            );
        }
    }
    for a in 0..rows {
        for b in 0..cols {
            let i = base + (a * (cols + 1) + b) as u32;
            idx.extend([
                i,
                i + 1,
                i + cols as u32 + 1,
                i + 1,
                i + cols as u32 + 2,
                i + cols as u32 + 1,
            ]);
        }
    }
}
fn rod(
    v: &mut VertexBuffer,
    idx: &mut Vec<u32>,
    a: [f32; 3],
    b: [f32; 3],
    r: f32,
    j: [u16; 4],
    w: [f32; 4],
) {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len = (d[0] * d[0] + d[2] * d[2]).sqrt();
    let n = [-d[2] / len, 0., d[0] / len];
    let base = v.element_count as u32;
    for c in [a, b] {
        for k in 0..8 {
            let t = std::f32::consts::TAU * k as f32 / 8.;
            let no = [n[0] * t.cos(), t.sin(), n[2] * t.cos()];
            add_vertex(
                v,
                [c[0] + r * no[0], c[1] + r * no[1], c[2] + r * no[2]],
                no,
                [0.25, 0.5],
                j,
                w,
            );
        }
    }
    for k in 0..8u32 {
        let q = (k + 1) % 8;
        idx.extend([
            base + k,
            base + q,
            base + k + 8,
            base + q,
            base + q + 8,
            base + k + 8,
        ]);
    }
}
fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let pak = &a[1];
    let out = &a[2];
    let dir = std::path::Path::new(out).parent().unwrap();
    let original = read_vpk_entry(pak, ENTRY)?;
    let model = morphic::model::decode(&original)?;
    let mut bytes = original.clone();
    // Duplicate both real ocular meshes, retaining their iris UVs and full channels.
    let head = model.meshes.iter().find(|m| m.name == "head").unwrap();
    let eye_pi = head
        .primitives
        .iter()
        .position(|p| p.material.contains("mina_eyes."))
        .unwrap();
    let p = &head.primitives[eye_pi];
    let (mut vb, mut idx) = compact(&head.vertex_buffers[p.vertex_buffer], &p.indices);
    let mut extra = vb.clone();
    let head_bone = model
        .skeleton
        .bones
        .iter()
        .position(|b| b.name == "head")
        .unwrap() as u16;
    for (i, pos) in extra.positions.iter_mut().enumerate() {
        let s = if pos[1] > 0. { 1. } else { -1. };
        pos[0] = 2.32 + (pos[0] - 1.5) * 0.3;
        pos[1] = s * 1.95 + (pos[1] - s * 1.75) * 0.7;
        pos[2] = 87.77 + (pos[2] - 87.77) * 0.6 - 1.42;
        extra.joints[i] = [head_bone; 4];
        extra.weights[i] = [1., 0., 0., 0.];
    }
    let n = vb.element_count as u32;
    let old_idx = idx.clone();
    append(&mut vb, &extra, &old_idx, n, &mut idx);
    let d = EditedPrimitive {
        mesh_name: Some(head.name.clone()),
        material_name: Some(p.material.clone()),
        vertex_buffer: vb,
        indices: idx,
    };
    bytes = replace_mesh_group_uncompressed(
        &bytes,
        &[PrimitiveSelection {
            mesh_index: head.mesh_index,
            primitive_index: eye_pi,
        }],
        &[d],
    )?
    .0;
    // Almond-shaped eyeliner rims seat the additional pair into the face.
    let fp = head
        .primitives
        .iter()
        .position(|p| p.material.contains("mina_head."))
        .unwrap();
    let face = &head.primitives[fp];
    let (mut fv, mut fi) = compact(&head.vertex_buffers[face.vertex_buffer], &face.indices);
    let tex = read_vpk_entry(
        pak,
        "models/heroes_wip/vampirebat/materials/mina_head_material__color_png_77d536c8.vtex_c",
    )?;
    let im = morphic::decode(&tex)?;
    let morphic::ImageData::Rgba8(pixels) = im.data else {
        panic!("expected rgba")
    };
    let mut uv = [0.5, 0.5];
    let mut darkest = 1000u32;
    for t in &fv.texcoords[0] {
        let x = (t[0].rem_euclid(1.) * im.width as f32) as usize;
        let y = (t[1].rem_euclid(1.) * im.height as f32) as usize;
        let o = (y * im.width as usize + x) * 4;
        let sum = pixels[o] as u32 + pixels[o + 1] as u32 + pixels[o + 2] as u32;
        if sum < darkest {
            darkest = sum;
            uv = *t;
        }
    }
    let mut rim = VertexBuffer {
        texcoords: vec![vec![]],
        ..Default::default()
    };
    let mut ri = vec![];
    for side in [-1., 1.] {
        let base = rim.element_count as u32;
        for (width, height) in [(0.66f32, 0.51f32), (0.51f32, 0.25f32)] {
            for k in 0..48 {
                let t = std::f32::consts::TAU * k as f32 / 48.;
                let y = side * 1.95 + width * t.cos();
                let z = 86.35 + height * t.sin();
                add_vertex(
                    &mut rim,
                    [2.60, y, z],
                    [1., 0., 0.],
                    uv,
                    [head_bone; 4],
                    [1., 0., 0., 0.],
                );
            }
        }
        for k in 0..48u32 {
            let q = (k + 1) % 48;
            ri.extend([
                base + k,
                base + q,
                base + k + 48,
                base + q,
                base + q + 48,
                base + k + 48,
            ]);
        }
    }
    let base = fv.element_count as u32;
    append(&mut fv, &rim, &ri, base, &mut fi);
    let donor = EditedPrimitive {
        mesh_name: Some(head.name.clone()),
        material_name: Some(face.material.clone()),
        vertex_buffer: fv,
        indices: fi,
    };
    bytes = replace_mesh_group_uncompressed(
        &bytes,
        &[PrimitiveSelection {
            mesh_index: head.mesh_index,
            primitive_index: fp,
        }],
        &[donor],
    )?
    .0;
    // Replace the bat charm with a sculpted spider, using the charm's original rig binding.
    let body = &model.meshes[0];
    let pi = body
        .primitives
        .iter()
        .position(|p| p.material.contains("mina_charm."))
        .unwrap();
    let p = &body.primitives[pi];
    let (old, _) = compact(&body.vertex_buffers[p.vertex_buffer], &p.indices);
    let j = old.joints[0];
    let w = old.weights[0];
    let mut v = VertexBuffer {
        texcoords: vec![vec![]],
        ..Default::default()
    };
    let mut ix = vec![];
    let c = [-8.8, -9.4, 64.2];
    ball(
        &mut v,
        &mut ix,
        [c[0], c[1], c[2] - 0.55],
        [0.65, 0.38, 0.9],
        [0.25, 0.5],
        j,
        w,
    );
    ball(
        &mut v,
        &mut ix,
        [c[0], c[1], c[2] + 0.55],
        [0.45, 0.35, 0.45],
        [0.75, 0.5],
        j,
        w,
    );
    for s in [-1., 1.] {
        for k in 0..4 {
            let z = 0.6 - k as f32 * 0.4;
            let bend = 1.9 - k as f32 * 1.2;
            let end = 2.4 - k as f32 * 1.6;
            let a = [c[0] + s * 0.35, c[1], c[2] + z];
            let b = [c[0] + s * 1.45, c[1] - 0.1, c[2] + bend];
            let e = [c[0] + s * 2.5, c[1] + 0.1, c[2] + end];
            rod(&mut v, &mut ix, a, b, 0.12, j, w);
            rod(&mut v, &mut ix, b, e, 0.09, j, w);
        }
    }
    let d = EditedPrimitive {
        mesh_name: Some(body.name.clone()),
        material_name: Some(p.material.clone()),
        vertex_buffer: v,
        indices: ix,
    };
    bytes = replace_mesh_group_uncompressed(
        &bytes,
        &[PrimitiveSelection {
            mesh_index: body.mesh_index,
            primitive_index: pi,
        }],
        &[d],
    )?
    .0;
    // Planar canopy UVs make the vector web follow all eight existing umbrella panels.
    let u = model.meshes.iter().find(|m| m.name == "umbrella").unwrap();
    let p = &u.primitives[0];
    let (mut vb, idx) = compact(&u.vertex_buffers[p.vertex_buffer], &p.indices);
    for (i, pos) in vb.positions.iter().enumerate() {
        vb.texcoords[0][i] = if pos[2] > 22. || pos[0] * pos[0] + pos[1] * pos[1] > 16. {
            [0.5 + pos[0] / 60., 0.5 + pos[1] / 60.]
        } else {
            [0.01, 0.01]
        };
    }
    let d = EditedPrimitive {
        mesh_name: Some(u.name.clone()),
        material_name: Some(p.material.clone()),
        vertex_buffer: vb,
        indices: idx,
    };
    bytes = replace_mesh_group_uncompressed(
        &bytes,
        &[PrimitiveSelection {
            mesh_index: u.mesh_index,
            primitive_index: 0,
        }],
        &[d],
    )?
    .0;
    let decoded = morphic::model::decode(&bytes)?;
    assert_eq!(model.skeleton.bones.len(), decoded.skeleton.bones.len());
    for m in &decoded.meshes {
        for p in &m.primitives {
            let v = &m.vertex_buffers[p.vertex_buffer];
            assert!(p.indices.iter().all(|i| (*i as usize) < v.positions.len()));
            assert!(v.positions.iter().flatten().all(|x| x.is_finite()));
            for ws in &v.weights {
                assert!((ws.iter().sum::<f32>() - 1.).abs() < 0.02);
            }
        }
    }
    let mut entries = vec![(ENTRY.to_string(), bytes)];
    for (entry, png) in [
        ("mina_umbrella_color_tga_198fc582.vtex_c", "web-canopy.png"),
        ("mina_charm_color_png_984ee5d6.vtex_c", "charm-palette.png"),
        (
            "mina_umbrella_selfillumask_tga_922ed376.vtex_c",
            "black.png",
        ),
        ("mina_umbrella_ao_tga_74c6bf5c.vtex_c", "white.png"),
        (
            "mina_umbrella_vmat_g_tnormalroughness_d52512a9.vtex_c",
            "normal.png",
        ),
    ] {
        let path = format!("models/heroes_wip/vampirebat/materials/{entry}");
        let base = read_vpk_entry(pak, &path)?;
        let dim = morphic::decode(&base)?;
        let im = image::open(dir.join(png))?
            .resize_exact(dim.width, dim.height, image::imageops::FilterType::Lanczos3)
            .to_rgba8();
        let encoded = morphic::replace_mip_chain(
            &base,
            &morphic::Image {
                width: dim.width,
                height: dim.height,
                data: morphic::ImageData::Rgba8(im.into_raw()),
            },
        )?;
        entries.push((path, encoded));
    }
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, out)?;
    let reread = read_vpk_entry(out, ENTRY)?;
    assert_eq!(reread, entries[0].1);
    println!("Wrote {out}; {} bones preserved; four eyes, spider charm, web canopy; packed model verified",model.skeleton.bones.len());
    Ok(())
}
