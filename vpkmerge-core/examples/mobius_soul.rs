// Import a vertex-coloured, UN-textured GLB (e.g. a rainbow Mobius band) as a soul
// container, preserving its per-vertex colour AND animating it.
//
// The stock soul mesh layout has no COLOR stream and the band has no UVs or texture,
// so the usual clone path would render it flat grey. Trick: the band's own HUE is a
// 1-D parameterisation (it sweeps around the loop), so we
//   - set each vertex U = its own hue, V = its own value (brightness),
//   - bake the albedo as an HSV chart (U -> hue across width, V -> value down height),
//   - which reconstructs the original rainbow exactly, and then
//   - set g_vAlbedoScrollSpeed1 so U scrolls -> the rainbow FLOWS around the band.
//
// All on the proven BCn-donor albedo + scroll vmat paths (both in-game confirmed).
//
// usage: cargo run --release --example mobius_soul -- \
//          <pak01_dir.vpk> <model.glb> <out_dir.vpk> [name] [--png grad.png]
//   SOUL_SCROLL=0.12     U scroll speed (default 0.12; 0 = static rainbow)
//   SOUL_ROTATE=-90,0,0  X,Y,Z euler deg (default -90,0,0; X=-90 stands the
//                        Sketchfab Z-up band up, Y/Z tilt it)
use anyhow::{anyhow, Context, Result};
use morphic::kv3::{Seg, Value as Kv3};
use morphic::model::{replace_mesh_part_uncompressed, set_model_material, VertexBuffer};
use morphic::{replace_mip_chain, Image, ImageData};

const MODEL: &str = "models/props_gameplay/soul_container/soul_container.vmdl_c";
const MAT_DIR: &str = "models/props_gameplay/soul_container/materials";
const DONOR_VMAT: &[u8] = include_bytes!("../../morphic/fixtures/soul/soul_material_donor.vmat_c");
const COLOR_DONOR: &str = "dev/helper/testgrid_color_tga_2d6cc34.vtex_c";
const DEFAULT_NORMAL: &str = "materials/default/default_normal_tga_7be61377.vtex";
const ATLAS: u32 = 512;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let pak = args.next().context("arg1: pak01_dir.vpk")?;
    let glb_path = args.next().context("arg2: model glb")?;
    let out = args.next().context("arg3: out_dir.vpk")?;
    let mut name = "mobius".to_string();
    let mut png: Option<String> = None;
    let rest: Vec<String> = args.collect();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--png" => {
                png = Some(rest.get(i + 1).context("--png needs a path")?.clone());
                i += 2;
            }
            other => {
                name = other.to_string();
                i += 1;
            }
        }
    }
    let scroll: f64 = std::env::var("SOUL_SCROLL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.12);
    let rot = parse_rotate(std::env::var("SOUL_ROTATE").ok().as_deref())?;

    let glb = std::fs::read(&glb_path)?;
    let (positions, normals, colors, indices) = read_glb(&glb, rot)?;
    eprintln!("rotate: [{}, {}, {}] deg", rot[0], rot[1], rot[2]);
    eprintln!("glb: {} verts, {} tris", positions.len(), indices.len() / 3);

    // --- per-vertex UV from (hue, value); build merged buffer ---
    let mut texcoords = Vec::with_capacity(positions.len());
    for c in &colors {
        let (h, _s, v) = rgb_to_hsv(c[0], c[1], c[2]);
        texcoords.push([(h / 360.0).clamp(0.0, 1.0) as f32, v.clamp(0.0, 1.0) as f32]);
    }
    let mut merged = VertexBuffer {
        element_count: positions.len(),
        positions: positions.clone(),
        normals,
        texcoords: vec![texcoords],
        ..VertexBuffer::default()
    };

    // --- fit to orb bounds ---
    let vpk = valve_pak::open(&pak)?;
    let model_bytes = {
        let mut f = vpk.get_file(MODEL).context("stock soul model not in pak")?;
        f.read_all()?
    };
    let orb = morphic::model::decode(&model_bytes)
        .map_err(|e| anyhow!("decode orb: {e}"))?
        .position_bounds()
        .ok_or_else(|| anyhow!("orb has no positions"))?;
    let orb_center = [
        mid(orb.min[0], orb.max[0]),
        mid(orb.min[1], orb.max[1]),
        mid(orb.min[2], orb.max[2]),
    ];
    let orb_size = (0..3)
        .map(|k| orb.max[k] - orb.min[k])
        .fold(0.0_f32, f32::max);
    let (mc, ms) = bounds(&merged.positions);
    let scale = if ms > 0.0 { orb_size / ms } else { 1.0 };
    for p in &mut merged.positions {
        for k in 0..3 {
            p[k] = (p[k] - mc[k]) * scale + orb_center[k];
        }
    }
    eprintln!("fit: x{scale:.3} (orb span {orb_size:.2})");

    // --- gradient albedo: HSV chart (x -> hue 0..360, y -> value 0..1) ---
    let mut px = vec![0u8; (ATLAS * ATLAS * 4) as usize];
    for y in 0..ATLAS {
        let val = f64::from(y) / f64::from(ATLAS - 1);
        for x in 0..ATLAS {
            let hue = f64::from(x) / f64::from(ATLAS - 1) * 360.0;
            let (r, g, b) = hsv_to_rgb(hue, 1.0, val);
            let o = ((y * ATLAS + x) * 4) as usize;
            px[o] = to_srgb_u8(r);
            px[o + 1] = to_srgb_u8(g);
            px[o + 2] = to_srgb_u8(b);
            px[o + 3] = 255;
        }
    }
    if let Some(p) = &png {
        let img: image::RgbaImage = image::ImageBuffer::from_raw(ATLAS, ATLAS, px.clone()).unwrap();
        img.save(p)?;
        eprintln!("wrote gradient preview {p}");
    }

    let donor = {
        let mut f = vpk
            .get_file(COLOR_DONOR)
            .context("color donor not in pak")?;
        f.read_all()?
    };
    let color_vtex = format!("{MAT_DIR}/{name}_color.vtex");
    let color_entry = format!("{MAT_DIR}/{name}_color.vtex_c");
    let color_tex = replace_mip_chain(
        &donor,
        &Image {
            width: ATLAS,
            height: ATLAS,
            data: ImageData::Rgba8(px),
        },
    )
    .map_err(|e| anyhow!("encoding gradient: {e}"))?;

    // --- swap mesh (uncompressed) + repoint material ---
    let (mesh_swapped, _rep) =
        replace_mesh_part_uncompressed(&model_bytes, "soul_container", &merged, &indices)
            .map_err(|e| anyhow!("replacing mesh: {e}"))?;
    let vmat_path = format!("{MAT_DIR}/{name}.vmat");
    let edited_model = set_model_material(&mesh_swapped, &vmat_path)
        .map_err(|e| anyhow!("repoint material: {e}"))?;

    let vmat = build_material(&color_vtex, scroll)?;

    let entries: Vec<(String, Vec<u8>)> = vec![
        (MODEL.to_string(), edited_model),
        (format!("{MAT_DIR}/{name}.vmat_c"), vmat),
        (color_entry, color_tex),
    ];
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out)?;
    eprintln!("wrote {out} ({} entries); scroll {scroll}", refs.len());
    Ok(())
}

/// Donor material: g_tColor -> gradient, normal -> flat default, scroll on.
fn build_material(color_vtex: &str, scroll: f64) -> Result<Vec<u8>> {
    let v = morphic::decode_kv3_resource(DONOR_VMAT).map_err(|e| anyhow!("decode donor: {e}"))?;
    let mut str_edits: Vec<(Vec<Seg>, String)> = Vec::new();
    str_edits.push((
        tex_path(&v, "g_tColor").ok_or_else(|| anyhow!("no g_tColor"))?,
        color_vtex.to_string(),
    ));
    if let Some(p) = tex_path(&v, "g_tNormalRoughness") {
        str_edits.push((p, DEFAULT_NORMAL.to_string()));
    }
    let patched = morphic::patch_kv3_resource_strings_adding(DONOR_VMAT, &str_edits)
        .map_err(|e| anyhow!("repoint textures: {e}"))?;

    // Scroll the albedo in U (g_vAlbedoScrollSpeed1.x).
    let v2 = morphic::decode_kv3_resource(&patched).map_err(|e| anyhow!("re-decode: {e}"))?;
    let out = if let Some(i) = vec_index(&v2, "g_vAlbedoScrollSpeed1") {
        let edits = vec![(
            vec![
                Seg::Key("m_vectorParams".into()),
                Seg::Index(i),
                Seg::Key("m_value".into()),
                Seg::Index(0),
            ],
            scroll,
        )];
        morphic::patch_kv3_resource_doubles(&patched, &edits)
            .map_err(|e| anyhow!("set scroll: {e}"))?
    } else {
        eprintln!("warning: donor has no g_vAlbedoScrollSpeed1; shipping static");
        patched
    };

    let check = morphic::decode_kv3_resource(&out).map_err(|e| anyhow!("verify: {e}"))?;
    if tex_value(&check, "g_tColor").as_deref() != Some(color_vtex) {
        return Err(anyhow!("g_tColor repoint did not take"));
    }
    Ok(out)
}

fn tex_index(v: &Kv3, name: &str) -> Option<usize> {
    v.get("m_textureParams")?
        .as_array()?
        .iter()
        .position(|p| p.get("m_name").and_then(Kv3::as_str) == Some(name))
}
fn tex_path(v: &Kv3, name: &str) -> Option<Vec<Seg>> {
    let i = tex_index(v, name)?;
    Some(vec![
        Seg::Key("m_textureParams".into()),
        Seg::Index(i),
        Seg::Key("m_pValue".into()),
    ])
}
fn tex_value(v: &Kv3, name: &str) -> Option<String> {
    let i = tex_index(v, name)?;
    v.get("m_textureParams")?
        .as_array()?
        .get(i)?
        .get("m_pValue")?
        .as_str()
        .map(str::to_string)
}
fn vec_index(v: &Kv3, name: &str) -> Option<usize> {
    v.get("m_vectorParams")?
        .as_array()?
        .iter()
        .position(|p| p.get("m_name").and_then(Kv3::as_str) == Some(name))
}

// --- GLB reading: positions/normals (Source space, fitted later) + COLOR_0 + indices ---
type Mat3 = [[f32; 3]; 3];
type Mat4 = [[f32; 4]; 4];

fn parse_rotate(spec: Option<&str>) -> Result<[f32; 3]> {
    let Some(s) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok([-90.0, 0.0, 0.0]);
    };
    let parts: Vec<f32> = s
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| anyhow!("SOUL_ROTATE must be X,Y,Z degrees, got {s:?}"))?;
    if parts.len() != 3 {
        return Err(anyhow!("SOUL_ROTATE must be X,Y,Z degrees, got {s:?}"));
    }
    Ok([parts[0], parts[1], parts[2]])
}

fn read_glb(
    glb: &[u8],
    rot: [f32; 3],
) -> Result<(Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 4]>, Vec<u32>)> {
    let parsed =
        gltf::Gltf::from_slice_without_validation(glb).map_err(|e| anyhow!("parse glb: {e}"))?;
    let buffers = gltf::import_buffers(&parsed.document, None, parsed.blob)
        .map_err(|e| anyhow!("glb buffers: {e}"))?;
    let doc = parsed.document;
    let world = node_worlds(&doc);
    // Z * Y * X: X stands the band up first, then Y/Z tilt it.
    let orient = mat3_mul(
        rotate_z(rot[2]),
        mat3_mul(rotate_y(rot[1]), rotate_x(rot[0])),
    );

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut colors = Vec::new();
    let mut indices = Vec::new();
    for node in doc.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        let nw = world[node.index()];
        for prim in mesh.primitives() {
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
            let base = u32::try_from(positions.len())?;
            let ps: Vec<[f32; 3]> = reader
                .read_positions()
                .ok_or_else(|| anyhow!("no POSITION"))?
                .map(|p| {
                    merge_swizzle(transform3(&orient, gltf_to_source(transform_point(&nw, p))))
                })
                .collect();
            let ns: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|it| {
                    it.map(|n| {
                        norm(merge_swizzle(transform3(
                            &orient,
                            gltf_to_source(transform_vec(&nw, n)),
                        )))
                    })
                    .collect()
                })
                .unwrap_or_else(|| vec![[0.0, 0.0, 1.0]; ps.len()]);
            let cs: Vec<[f32; 4]> = reader
                .read_colors(0)
                .map(|c| c.into_rgba_f32().collect())
                .unwrap_or_else(|| vec![[1.0, 1.0, 1.0, 1.0]; ps.len()]);
            let idx: Vec<u32> = reader
                .read_indices()
                .ok_or_else(|| anyhow!("no indices"))?
                .into_u32()
                .map(|x| x + base)
                .collect();
            positions.extend(ps);
            normals.extend(ns);
            colors.extend(cs);
            indices.extend(idx);
        }
    }
    if positions.is_empty() {
        return Err(anyhow!("glb has no mesh"));
    }
    Ok((positions, normals, colors, indices))
}

const IDENTITY4: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn gltf_to_source(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[2], -p[1]]
}
fn merge_swizzle(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[2], -p[1]]
}
fn node_worlds(doc: &gltf::Document) -> Vec<Mat4> {
    let mut out = vec![IDENTITY4; doc.nodes().count()];
    for scene in doc.scenes() {
        for node in scene.nodes() {
            accumulate(&node, IDENTITY4, &mut out);
        }
    }
    out
}
fn accumulate(node: &gltf::Node<'_>, parent: Mat4, out: &mut [Mat4]) {
    let world = mat4_mul(parent, node.transform().matrix());
    out[node.index()] = world;
    for c in node.children() {
        accumulate(&c, world, out);
    }
}
fn mat4_mul(a: Mat4, b: Mat4) -> Mat4 {
    let mut r = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            for k in 0..4 {
                r[col][row] += a[k][row] * b[col][k];
            }
        }
    }
    r
}
fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let v = [p[0], p[1], p[2], 1.0];
    let mut o = [0.0f32; 3];
    for (row, oo) in o.iter_mut().enumerate() {
        for (col, &vc) in v.iter().enumerate() {
            *oo += m[col][row] * vc;
        }
    }
    o
}
fn transform_vec(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let v = [p[0], p[1], p[2], 0.0];
    let mut o = [0.0f32; 3];
    for (row, oo) in o.iter_mut().enumerate() {
        for (col, &vc) in v.iter().enumerate() {
            *oo += m[col][row] * vc;
        }
    }
    o
}
fn transform3(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}
fn mat3_mul(a: Mat3, b: Mat3) -> Mat3 {
    let mut r = [[0.0f32; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            for k in 0..3 {
                r[row][col] += a[row][k] * b[k][col];
            }
        }
    }
    r
}
fn rotate_x(deg: f32) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}
fn rotate_y(deg: f32) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}
fn rotate_z(deg: f32) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}
fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l <= f32::EPSILON {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / l, v[1] / l, v[2] / l]
    }
}
fn mid(a: f32, b: f32) -> f32 {
    (a + b) / 2.0
}
fn bounds(ps: &[[f32; 3]]) -> ([f32; 3], f32) {
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for p in ps {
        for k in 0..3 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
        }
    }
    (
        [mid(mn[0], mx[0]), mid(mn[1], mx[1]), mid(mn[2], mx[2])],
        (0..3).map(|k| mx[k] - mn[k]).fold(0.0_f32, f32::max),
    )
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f64, f64, f64) {
    let (r, g, b) = (f64::from(r), f64::from(g), f64::from(b));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f64::EPSILON {
        0.0
    } else if (max - r).abs() < f64::EPSILON {
        (((g - b) / d).rem_euclid(6.0)) * 60.0
    } else if (max - g).abs() < f64::EPSILON {
        ((b - r) / d + 2.0) * 60.0
    } else {
        ((r - g) / d + 4.0) * 60.0
    };
    let s = if max <= f64::EPSILON { 0.0 } else { d / max };
    (h.rem_euclid(360.0), s, max)
}
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    let c = v * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (r + m, g + m, b + m)
}
fn to_srgb_u8(c: f64) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}
