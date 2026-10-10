// Flat-albedo orthographic render of a hero model, used to see baked-in color
// that has nothing to do with lighting.
//
// Per pixel it resolves the material's base color exactly the way `pbr.vfx`
// composes it for the albedo term: g_tColor sampled at the UV, times the baked
// vertex COLOR when the material sets F_VERTEX_COLOR (scaled by
// g_fVertexColorStrength1). No lights, no AO, no shadows. Anything dark here is
// dark in the asset, not in the scene.
//
// Writes three panels side by side: albedo with the vertex-color multiply
// applied (what ships), the same without it, and the vertex color alone.
//
// usage: cargo run --example albedo_render -- <vpk> <hero-codename> <out.png>
//        [--view front|side] [--zmax N] [--width N]

use std::collections::BTreeMap;

struct Tex {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

impl Tex {
    fn sample(&self, u: f32, v: f32) -> [f32; 3] {
        if self.w == 0 || self.h == 0 {
            return [1.0; 3];
        }
        let x = ((u.rem_euclid(1.0)) * self.w as f32) as usize % self.w;
        let y = ((v.rem_euclid(1.0)) * self.h as f32) as usize % self.h;
        let i = (y * self.w + x) * 4;
        [
            f32::from(self.rgba[i]) / 255.0,
            f32::from(self.rgba[i + 1]) / 255.0,
            f32::from(self.rgba[i + 2]) / 255.0,
        ]
    }
}

/// Draw-call material paths omit the compiled `_c` suffix; VPK entries carry it.
fn vmat_entry(path: &str) -> String {
    if path.ends_with("_c") {
        path.to_string()
    } else {
        format!("{path}_c")
    }
}

fn half_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1f);
    let mant = u32::from(h & 0x3ff);
    if exp == 0 {
        return f32::from_bits(sign) * (mant as f32 / 1024.0);
    }
    f32::from_bits(sign | ((exp + 112) << 23) | (mant << 13))
}

fn load_tex(vpk_path: &str, path: &str) -> Option<Tex> {
    let entry = if path.ends_with("_c") {
        path.to_string()
    } else {
        format!("{path}_c")
    };
    let bytes = vpkmerge_core::read_vpk_entry(vpk_path, &entry).ok()?;
    let img = morphic::decode(&bytes).ok()?;
    let rgba = match &img.data {
        morphic::ImageData::Rgba8(px) => px.clone(),
        morphic::ImageData::Rgba16F(px) => px
            .iter()
            .map(|h| (half_to_f32(h.to_bits()).clamp(0.0, 1.0) * 255.0) as u8)
            .collect(),
    };
    Some(Tex {
        w: img.width as usize,
        h: img.height as usize,
        rgba,
    })
}

struct MatInfo {
    color: Option<Tex>,
    vertex_color: bool,
    strength: f32,
    tint: [f32; 3],
    /// Additive / alpha-blended shells (Vindicta's `ghost_glow`) are skipped:
    /// this rasterizer is opaque-only, and drawing them would hide the body.
    translucent: bool,
}

/// sRGB encode for display (the decoded albedo is already sRGB-ish, so this is
/// identity; kept explicit so the panels are comparable).
fn to_u8(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[allow(clippy::too_many_lines)]
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let vpk_path = args[0].clone();
    let codename = args[1].clone();
    let out_path = args[2].clone();
    let side = args.iter().any(|a| a == "side");
    let width: usize = args
        .iter()
        .position(|a| a == "--width")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(420);
    let zmin_arg: Option<f32> = args
        .iter()
        .position(|a| a == "--zmin")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());
    let zmax_arg: Option<f32> = args
        .iter()
        .position(|a| a == "--zmax")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());

    // `--model-vpk` reads the geometry from an override pak (an edited addon)
    // while materials and textures still resolve from the base pak.
    let model_vpk = args
        .iter()
        .position(|a| a == "--model-vpk")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| vpk_path.clone());
    let entry = vpkmerge_core::hero_model_entry(&model_vpk, None, &codename)?;
    let bytes = vpkmerge_core::read_vpk_entry(&model_vpk, &entry)?;
    let model = morphic::model::decode(&bytes)?;
    println!("model: {entry} (from {model_vpk})");

    let mut mats: BTreeMap<String, MatInfo> = BTreeMap::new();

    // World bounds over LOD0 only.
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for mesh in &model.meshes {
        if mesh.name.contains("_lod") {
            continue;
        }
        for vb in &mesh.vertex_buffers {
            for p in &vb.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
    }
    let z_hi = zmax_arg.unwrap_or(hi[2]);
    let z_lo = zmin_arg.unwrap_or(lo[2]);
    println!(
        "bounds x {:.1}..{:.1}  y {:.1}..{:.1}  z {:.1}..{:.1}  (rendering z up to {z_hi:.1})",
        lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
    );

    // Horizontal axis: X for front view, Y for side view.
    let ax = usize::from(side);
    let h_lo = lo[ax];
    let h_hi = hi[ax];
    let h_span = (h_hi - h_lo).max(1e-3);
    let v_span = (z_hi - z_lo).max(1e-3);
    let scale = width as f32 / h_span;
    let height = (v_span * scale).ceil() as usize;

    const PANELS: usize = 4;
    let gap = 12usize;
    let total_w = width * PANELS + gap * (PANELS - 1);

    let mut fb = vec![[24u8, 24, 28]; total_w * height];
    let mut depth = vec![f32::INFINITY; width * height * PANELS];

    for mesh in &model.meshes {
        if mesh.name.contains("_lod") {
            continue;
        }
        for prim in &mesh.primitives {
            let info = mats.entry(prim.material.clone()).or_insert_with(|| {
                let targets =
                    vpkmerge_core::vmat_style::VmatTargets::Entries(vec![vmat_entry(&prim.material)]);
                let mats = vpkmerge_core::vmat_style::list_materials(&vpk_path, None, &targets)
                    .unwrap_or_default();
                let Some(m) = mats.first() else {
                    return MatInfo {
                        color: None,
                        vertex_color: false,
                        strength: 0.0,
                        tint: [1.0; 3],
                        translucent: false,
                    };
                };
                let vertex_color = m.flags.iter().any(|(f, v)| f == "F_VERTEX_COLOR" && *v != 0);
                let strength = m
                    .floats
                    .iter()
                    .find(|(n, _)| n == "g_fVertexColorStrength1")
                    .map_or(0.0, |(_, v)| *v as f32);
                let tint = m
                    .vectors
                    .iter()
                    .find(|(n, _)| n == "g_vColorTint1")
                    .map_or([1.0; 3], |(_, v)| {
                        [
                            *v.first().unwrap_or(&1.0) as f32,
                            *v.get(1).unwrap_or(&1.0) as f32,
                            *v.get(2).unwrap_or(&1.0) as f32,
                        ]
                    });
                let mut hash = 0u32;
                for b in prim.material.bytes() {
                    hash = hash.wrapping_mul(31).wrapping_add(u32::from(b));
                }
                let swatch = if vertex_color {
                    [255u8, 0, 255]
                } else {
                    [
                        to_u8(0.25 + 0.5 * ((hash >> 3) & 0xFF) as f32 / 255.0),
                        to_u8(0.25 + 0.5 * ((hash >> 11) & 0xFF) as f32 / 255.0),
                        to_u8(0.25 + 0.5 * ((hash >> 19) & 0xFF) as f32 / 255.0),
                    ]
                };
                println!(
                    "  {:<28} id_rgb={swatch:?} F_VERTEX_COLOR={vertex_color} strength={strength} tint={tint:?}",
                    prim.material.rsplit('/').next().unwrap_or(&prim.material)
                );
                let translucent = m.flags.iter().any(|(f, v)| {
                    *v != 0 && (f == "F_TRANSLUCENT" || f == "F_ADDITIVE_BLEND")
                });
                MatInfo {
                    color: m
                        .textures
                        .iter()
                        .find(|(k, _)| k == "g_tColor")
                        .and_then(|(_, p)| load_tex(&vpk_path, p)),
                    vertex_color,
                    strength,
                    tint,
                    translucent,
                }
            });

            if info.translucent {
                continue;
            }

            let vb = &mesh.vertex_buffers[prim.vertex_buffer];
            let uvs = vb.texcoords.first();
            let vcols = vb.colors.first();

            for tri in prim.indices.chunks_exact(3) {
                let idx = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
                if idx.iter().any(|&i| i >= vb.element_count) {
                    continue;
                }
                let p: Vec<[f32; 3]> = idx.iter().map(|&i| vb.positions[i]).collect();
                // Screen coords; depth = the axis we look along.
                let sx: Vec<f32> = p.iter().map(|q| (q[ax] - h_lo) * scale).collect();
                let sy: Vec<f32> = p.iter().map(|q| (z_hi - q[2]) * scale).collect();
                let dep: Vec<f32> = p.iter().map(|q| if side { q[0] } else { -q[1] }).collect();

                let min_x = sx.iter().copied().fold(f32::INFINITY, f32::min).floor() as isize;
                let max_x = sx.iter().copied().fold(f32::NEG_INFINITY, f32::max).ceil() as isize;
                let min_y = sy.iter().copied().fold(f32::INFINITY, f32::min).floor() as isize;
                let max_y = sy.iter().copied().fold(f32::NEG_INFINITY, f32::max).ceil() as isize;

                let area = (sx[1] - sx[0]) * (sy[2] - sy[0]) - (sx[2] - sx[0]) * (sy[1] - sy[0]);
                if area.abs() < 1e-9 {
                    continue;
                }

                for py in min_y.max(0)..=max_y.min(height as isize - 1) {
                    for px in min_x.max(0)..=max_x.min(width as isize - 1) {
                        let fx = px as f32 + 0.5;
                        let fy = py as f32 + 0.5;
                        let w0 = ((sx[1] - fx) * (sy[2] - fy) - (sx[2] - fx) * (sy[1] - fy)) / area;
                        let w1 = ((sx[2] - fx) * (sy[0] - fy) - (sx[0] - fx) * (sy[2] - fy)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        let d = w0 * dep[0] + w1 * dep[1] + w2 * dep[2];
                        let di = py as usize * width + px as usize;
                        if d >= depth[di] {
                            continue;
                        }
                        depth[di] = d;

                        let base = match (&info.color, uvs) {
                            (Some(t), Some(uv)) => {
                                let u =
                                    w0 * uv[idx[0]][0] + w1 * uv[idx[1]][0] + w2 * uv[idx[2]][0];
                                let v =
                                    w0 * uv[idx[0]][1] + w1 * uv[idx[1]][1] + w2 * uv[idx[2]][1];
                                t.sample(u, v)
                            }
                            _ => [0.6, 0.6, 0.6],
                        };
                        let vc = match vcols {
                            Some(c) if c.len() == vb.element_count => {
                                let mut o = [0f32; 3];
                                for k in 0..3 {
                                    o[k] =
                                        w0 * c[idx[0]][k] + w1 * c[idx[1]][k] + w2 * c[idx[2]][k];
                                }
                                o
                            }
                            _ => [1.0; 3],
                        };

                        let mut shipped = [0f32; 3];
                        let mut no_vc = [0f32; 3];
                        for k in 0..3 {
                            let m = if info.vertex_color {
                                1.0 + (vc[k] - 1.0) * info.strength
                            } else {
                                1.0
                            };
                            shipped[k] = base[k] * info.tint[k] * m;
                            no_vc[k] = base[k] * info.tint[k];
                        }

                        // 4th panel: which material owns this pixel, so a
                        // vertex-colored region can be told from a neighbour
                        // that merely has a dark albedo.
                        let mut hash = 0u32;
                        for b in prim.material.bytes() {
                            hash = hash.wrapping_mul(31).wrapping_add(u32::from(b));
                        }
                        let ident = if info.vertex_color {
                            [1.0, 0.0, 1.0] // magenta: the F_VERTEX_COLOR material
                        } else {
                            [
                                0.25 + 0.5 * ((hash >> 3) & 0xFF) as f32 / 255.0,
                                0.25 + 0.5 * ((hash >> 11) & 0xFF) as f32 / 255.0,
                                0.25 + 0.5 * ((hash >> 19) & 0xFF) as f32 / 255.0,
                            ]
                        };
                        let panels = [shipped, no_vc, vc, ident];
                        for (pi, col) in panels.iter().enumerate() {
                            let ox = pi * (width + gap) + px as usize;
                            fb[py as usize * total_w + ox] =
                                [to_u8(col[0]), to_u8(col[1]), to_u8(col[2])];
                        }
                    }
                }
            }
        }
    }

    // Write PNG (RGB8, one filter-0 scanline per row).
    let mut raw = Vec::with_capacity(height * (1 + total_w * 3));
    for y in 0..height {
        raw.push(0u8);
        for x in 0..total_w {
            raw.extend_from_slice(&fb[y * total_w + x]);
        }
    }
    let png = encode_png(total_w as u32, height as u32, &raw);
    std::fs::write(&out_path, png)?;
    println!(
        "\nwrote {out_path}  ({total_w}x{height}) panels: [shipped] [albedo only] [vertex color only] [material id, magenta = F_VERTEX_COLOR]"
    );
    Ok(())
}

fn encode_png(w: u32, h: u32, raw: &[u8]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (i, e) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c = table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
        }
        c ^ 0xFFFF_FFFF
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut full = kind.to_vec();
        full.extend_from_slice(body);
        out.extend_from_slice(&full);
        out.extend_from_slice(&crc32(&full).to_be_bytes());
    }
    // Stored (uncompressed) deflate blocks wrapped in a zlib header.
    fn zlib_store(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x78, 0x01];
        let mut a = 1u32;
        let mut b = 0u32;
        for &byte in data {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        for (i, block) in data.chunks(65535).enumerate() {
            let last = u8::from((i + 1) * 65535 >= data.len());
            out.push(last);
            out.extend_from_slice(&(block.len() as u16).to_le_bytes());
            out.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
            out.extend_from_slice(block);
        }
        out.extend_from_slice(&((b << 16) | a).to_be_bytes());
        out
    }
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib_store(raw));
    chunk(&mut png, b"IEND", &[]);
    png
}
