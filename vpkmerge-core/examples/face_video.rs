// Build a "video face" addon for a flipbook-screen material (RUINER Billy
// style): tile a directory of PNG frames into the donor sheet's grid,
// re-encode the sheet in its own format, retime the material's flipbook
// expression to the real frame count/rate, and optionally patch the same two
// entries inside the mod's nested UI-map VPKs so the shop / postgame scenes
// play the new video too.
//
// The playback contract is the one RUINER Billy ships: the face mesh's UVs
// sit in the grid's first cell and a dynamic expression on
// g_vAlbedoTexcoordOffset1 walks the offset cell by cell:
//   float2((floor(time()*FPS)%N)%G/G, floor((floor(time()*FPS)%N)/G)/G)
//
// `--whole-material` is the stock-hero mode: no parked screen UVs needed.
// It additionally sets g_vAlbedoTexcoordScale1 = 1/grid so the material's
// normal UV layout compresses into one cell per frame (each cell is a full
// albedo variant). Only clean when the material's UVs stay inside [0,1]:
// check with the uv_range example first.
//
// usage: cargo run --release -p vpkmerge-core --example face_video -- \
//          --mod ruiner_billy_dir.vpk --frames DIR --out my_face_dir.vpk \
//          [--fps 24] [--grid 32] [--maps shop|all|none] [--sheet-png OUT.png] \
//          [--sheet-entry PATH.vtex_c] [--vmat-entry PATH.vmat_c] \
//          [--whole-material]
use std::path::PathBuf;

use anyhow::{bail, ensure, Context, Result};
use morphic::ImageData;
use vpkmerge_core::vmat_style::{patch_vmat_params, VmatEdit};

const DEFAULT_SHEET_ENTRY: &str =
    "models/heroes_wip/punkgoat/ruinermaterials/8192working_png_5ac28392.vtex_c";
const DEFAULT_VMAT_ENTRY: &str = "models/heroes_wip/punkgoat/ruinermaterials/screen2.vmat_c";
const EXPR_PARAM: &str = "g_vAlbedoTexcoordOffset1";

#[derive(Clone, Copy, PartialEq)]
enum MapMode {
    None,
    Shop,
    All,
}

fn main() -> Result<()> {
    let mut mod_vpk: Option<PathBuf> = None;
    let mut frames_dir: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut fps: f64 = 24.0;
    let mut grid: u32 = 32;
    let mut maps = MapMode::Shop;
    let mut sheet_entry = DEFAULT_SHEET_ENTRY.to_string();
    let mut vmat_entry = DEFAULT_VMAT_ENTRY.to_string();
    let mut sheet_png: Option<PathBuf> = None;
    let mut whole_material = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().with_context(|| format!("{name} needs a value"));
        match a.as_str() {
            "--mod" => mod_vpk = Some(PathBuf::from(val("--mod")?)),
            "--frames" => frames_dir = Some(PathBuf::from(val("--frames")?)),
            "--out" => out = Some(PathBuf::from(val("--out")?)),
            "--fps" => fps = val("--fps")?.parse().context("--fps")?,
            "--grid" => grid = val("--grid")?.parse().context("--grid")?,
            "--maps" => {
                maps = match val("--maps")?.as_str() {
                    "none" => MapMode::None,
                    "shop" => MapMode::Shop,
                    "all" => MapMode::All,
                    other => bail!("--maps {other:?}: expected none|shop|all"),
                }
            }
            "--sheet-entry" => sheet_entry = val("--sheet-entry")?,
            "--vmat-entry" => vmat_entry = val("--vmat-entry")?,
            "--sheet-png" => sheet_png = Some(PathBuf::from(val("--sheet-png")?)),
            "--whole-material" => whole_material = true,
            other => bail!("unknown flag {other:?}"),
        }
    }
    let mod_vpk = mod_vpk.context("--mod <mod_dir.vpk> is required")?;
    let frames_dir = frames_dir.context("--frames <dir> is required")?;
    let out = out.context("--out <out_dir.vpk> is required")?;
    ensure!(grid >= 1 && grid <= 64, "--grid must be 1..=64");
    ensure!(fps > 0.0, "--fps must be positive");

    // Frames, lexicographic order (ffmpeg's f_%04d.png naming sorts right).
    let mut frame_paths: Vec<PathBuf> = std::fs::read_dir(&frames_dir)
        .with_context(|| format!("reading {}", frames_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
        .collect();
    frame_paths.sort();
    let n_frames = frame_paths.len();
    let max_frames = (grid * grid) as usize;
    ensure!(n_frames > 0, "no .png frames in {}", frames_dir.display());
    ensure!(
        n_frames <= max_frames,
        "{n_frames} frames exceed the {grid}x{grid} grid ({max_frames}); \
         lower --fps or shorten the video"
    );

    // Donor sheet: decode, stamp frames over RGB (donor alpha plane kept),
    // black out the unused tail cells so no stale frames ship.
    let sheet_bytes = vpkmerge_core::read_vpk_entry(&mod_vpk, &sheet_entry)?;
    let mut img =
        morphic::decode(&sheet_bytes).with_context(|| format!("decoding {sheet_entry}"))?;
    let (sw, sh) = (img.width, img.height);
    ensure!(
        sw % grid == 0 && sh == sw,
        "sheet is {sw}x{sh}; expected square with side divisible by --grid {grid}"
    );
    let cell = sw / grid;
    let ImageData::Rgba8(ref mut px) = img.data else {
        bail!("expected an LDR (Rgba8) sheet texture");
    };
    println!(
        "sheet {sheet_entry}: {sw}x{sh}, grid {grid}x{grid}, cell {cell}px, \
         {n_frames} frames @ {fps} fps ({:.2}s loop)",
        n_frames as f64 / fps
    );

    for (i, fp) in frame_paths.iter().enumerate() {
        let frame = image::open(fp)
            .with_context(|| format!("opening {}", fp.display()))?
            .to_rgba8();
        let frame = if frame.width() == cell && frame.height() == cell {
            frame
        } else {
            image::imageops::resize(&frame, cell, cell, image::imageops::FilterType::Triangle)
        };
        let (cx, cy) = ((i as u32 % grid) * cell, (i as u32 / grid) * cell);
        for y in 0..cell {
            let dst_row = (((cy + y) * sw + cx) * 4) as usize;
            let src = frame.as_raw();
            let src_row = (y * cell * 4) as usize;
            // Screen mode keeps the donor's alpha plane (albedo alpha is a
            // mask); whole-material mode writes opaque alpha because the
            // donor's mask layout is spatially wrong once cells are frames.
            let lanes = if whole_material { 4 } else { 3 };
            for x in 0..cell as usize {
                px[dst_row + x * 4..dst_row + x * 4 + lanes]
                    .copy_from_slice(&src[src_row + x * 4..src_row + x * 4 + lanes]);
            }
        }
    }
    for i in n_frames..max_frames {
        let (cx, cy) = ((i as u32 % grid) * cell, (i as u32 / grid) * cell);
        for y in 0..cell {
            let dst_row = (((cy + y) * sw + cx) * 4) as usize;
            for x in 0..cell as usize {
                px[dst_row + x * 4..dst_row + x * 4 + 3].fill(0);
            }
        }
    }
    if let Some(p) = &sheet_png {
        image::RgbaImage::from_raw(sw, sh, px.clone())
            .context("sheet buffer")?
            .save(p)
            .with_context(|| format!("writing {}", p.display()))?;
        println!("wrote sheet preview {}", p.display());
    }

    println!("re-encoding sheet mip chain (this is the slow step)...");
    let new_sheet = morphic::replace_mip_chain(&sheet_bytes, &img)?;
    println!("  {} -> {} bytes", sheet_bytes.len(), new_sheet.len());

    // Retimed flipbook expression. Local-free equivalent of the shipped
    // opcode-0x08 form; % binds at multiplicative precedence, left-assoc.
    let src = format!(
        "{EXPR_PARAM}=float2((floor(time()*{fps})%{n_frames})%{grid}/{grid},\
         floor((floor(time()*{fps})%{n_frames})/{grid})/{grid})"
    );
    let (name, expr_src) = src.split_once('=').expect("static split");
    let vmat_bytes = vpkmerge_core::read_vpk_entry(&mod_vpk, &vmat_entry)?;
    let mut edits = vec![VmatEdit::expr(name, expr_src)?];
    if whole_material {
        let s = 1.0 / f64::from(grid);
        edits.push(VmatEdit::Vector {
            name: "g_vAlbedoTexcoordScale1".to_string(),
            value: [s, s, 0.0, 0.0],
        });
        println!("whole-material mode: g_vAlbedoTexcoordScale1 = [{s}, {s}]");
    }
    let (new_vmat, stats) =
        patch_vmat_params(&vmat_bytes, &edits).with_context(|| format!("patching {vmat_entry}"))?;
    println!("expression: {expr_src}");
    println!("  vmat patch: {stats:?}");

    // Output entries.
    let mut files: Vec<(String, Vec<u8>)> = vec![
        (sheet_entry.clone(), new_sheet),
        (vmat_entry.clone(), new_vmat),
    ];

    // Nested UI-map VPKs embed their own copy of the sheet + material; patch
    // and re-ship the ones the mode asks for so the shop scene matches.
    if maps != MapMode::None {
        let tmp = tempfile::tempdir().context("tempdir")?;
        let info = vpkmerge_core::inspect(&mod_vpk)?;
        let map_entries: Vec<&String> = info
            .file_paths
            .iter()
            .filter(|p| p.starts_with("maps/ui/") && p.ends_with(".vpk"))
            .filter(|p| maps == MapMode::All || p.contains("/hero_shop/"))
            .collect();
        for map_entry in map_entries {
            let inner_bytes = vpkmerge_core::read_vpk_entry(&mod_vpk, map_entry)?;
            let inner_path = tmp.path().join("inner_dir.vpk");
            std::fs::write(&inner_path, &inner_bytes)?;
            let inner = vpkmerge_core::inspect(&inner_path)?;
            if !inner.file_paths.iter().any(|p| p == &sheet_entry) {
                println!("skipping {map_entry}: no {sheet_entry} inside");
                continue;
            }
            let mut inner_files: Vec<(String, Vec<u8>)> = Vec::new();
            for p in &inner.file_paths {
                let bytes = if p == &sheet_entry {
                    files[0].1.clone()
                } else if p == &vmat_entry {
                    files[1].1.clone()
                } else {
                    vpkmerge_core::read_vpk_entry(&inner_path, p)?
                };
                inner_files.push((p.clone(), bytes));
            }
            let refs: Vec<(&str, &[u8])> = inner_files
                .iter()
                .map(|(p, b)| (p.as_str(), b.as_slice()))
                .collect();
            let rebuilt_path = tmp.path().join("rebuilt_dir.vpk");
            vpkmerge_core::pack(&refs, &rebuilt_path)?;
            let rebuilt = std::fs::read(&rebuilt_path)?;
            println!(
                "patched {map_entry}: {} entries, {} -> {} bytes",
                inner_files.len(),
                inner_bytes.len(),
                rebuilt.len()
            );
            files.push((map_entry.clone(), rebuilt));
        }
    }

    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out)?;
    println!(
        "packed {} ({} entries): install alongside the base mod at higher priority",
        out.display(),
        files.len()
    );
    Ok(())
}
