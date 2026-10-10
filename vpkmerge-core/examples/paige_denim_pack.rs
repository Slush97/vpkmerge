// Pack the Paige denim enhancement: denim albedo (override g_tColor) + packed
// normal-roughness (new texture) + w_b_r_s.vmat repointed at it.
//   usage: paige_denim_pack <skin_pak> <albedo.png> <normal.png> <rough.png> <out_dir.vpk>
use morphic::{Image, ImageData, TextureFlags};

const DIR: &str = "models/heroes_wip/bookworm/materials/";
const COLOR: &str = "clothes_color_png_e472671.vtex_c"; // jeans g_tColor -> override
const DONOR_2K: &str = "bookworm_upper_color_tga_fc1e5171.vtex_c"; // 2048 BC7 sRGB container (albedo)
const VMAT: &str = "w_b_r_s.vmat_c"; // jeans material -> repoint NR
const NR_STEM: &str = "clothes_denim_nr"; // new normal-roughness texture

fn load_png(path: &str) -> anyhow::Result<Image> {
    let d = image::load_from_memory(&std::fs::read(path)?)?.to_rgba8();
    let (w, h) = d.dimensions();
    Ok(Image {
        width: w,
        height: h,
        data: ImageData::Rgba8(d.into_raw()),
    })
}
fn raw(img: &Image) -> &Vec<u8> {
    match &img.data {
        ImageData::Rgba8(v) => v,
        _ => panic!("hdr"),
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (pak, albedo_png, normal_png, rough_png, out) = (&a[1], &a[2], &a[3], &a[4], &a[5]);

    // --- 1. albedo -> BC7 (sRGB) via 2048 upper donor, at existing g_tColor path ---
    let donor = vpkmerge_core::read_vpk_entry(pak, &format!("{DIR}{DONOR_2K}"))?;
    let albedo = load_png(albedo_png)?;
    let albedo_vtex = morphic::replace_mip_chain(&donor, &albedo)?;
    println!(
        "albedo {}x{} -> {} bytes (BC7 sRGB)",
        albedo.width,
        albedo.height,
        albedo_vtex.len()
    );

    // --- 2. normal-roughness: R,G = normal(GL, no flip); B = roughness; linear PNG_RGBA8888 ---
    let n = load_png(normal_png)?;
    let r = load_png(rough_png)?;
    assert_eq!(
        (n.width, n.height),
        (r.width, r.height),
        "normal/rough dims differ"
    );
    let (nr, rr) = (raw(&n), raw(&r));
    let mut nrbuf = vec![0u8; nr.len()];
    for (i, px) in nrbuf.chunks_exact_mut(4).enumerate() {
        px[0] = nr[i * 4]; // normal x
        px[1] = nr[i * 4 + 1]; // normal y  (Source g_tNormalRoughness is GL/Y+, no flip)
        px[2] = rr[i * 4]; // roughness (denim ~0.8 baked from the material)
        px[3] = 255;
    }
    let nr_img = Image {
        width: n.width,
        height: n.height,
        data: ImageData::Rgba8(nrbuf),
    };
    let nr_vtex = morphic::encode_vtex_png_rgba8888(&nr_img, TextureFlags::empty())?; // linear
    println!(
        "normal-roughness {}x{} -> {} bytes (PNG_RGBA8888 linear)",
        n.width,
        n.height,
        nr_vtex.len()
    );

    // --- 3. repoint w_b_r_s.vmat g_tNormalRoughness at the new texture (byte-faithful) ---
    let vmat_donor = vpkmerge_core::read_vpk_entry(pak, &format!("{DIR}{VMAT}"))?;
    let nr_src = format!("{DIR}{NR_STEM}.vtex"); // source-relative .vtex reference
    let vmat = morphic::compile_pbr_vmat(
        &vmat_donor,
        &format!("{DIR}w_b_r_s.vmat"),
        &[("g_tNormalRoughness", nr_src.as_str())],
    )?;
    println!("repointed w_b_r_s.vmat g_tNormalRoughness -> {nr_src}");

    // --- 4. pack override addon ---
    let color_e = format!("{DIR}{COLOR}");
    let nr_e = format!("{DIR}{NR_STEM}.vtex_c");
    let vmat_e = format!("{DIR}{VMAT}");
    let refs: Vec<(&str, &[u8])> = vec![
        (color_e.as_str(), albedo_vtex.as_slice()),
        (nr_e.as_str(), nr_vtex.as_slice()),
        (vmat_e.as_str(), vmat.as_slice()),
    ];
    vpkmerge_core::pack(&refs, out)?;
    println!("wrote override addon: {out}  ({} files)", refs.len());
    Ok(())
}
