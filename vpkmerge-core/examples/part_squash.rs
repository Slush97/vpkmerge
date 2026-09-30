// Shorten a mesh part by squashing its bind-pose vertices vertically toward its
// top edge: z' = z_top - (z_top - z) * factor (factor < 1 raises the bottom).
// Positions only (Tier 0 displacement, the in-game-proven vertex edit); a cloth
// part keeps its sim nodes, so the rendered hem simply rides above them.
//
// usage: part_squash <in.vmdl_c> <out.vmdl_c> <mesh-part> <factor>
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let mut bytes = std::fs::read(&a[1])?;
    let factor: f32 = a[4].parse()?;
    let targets = morphic::model::vertex_targets(&bytes)?;
    let selected: Vec<_> = targets.iter().filter(|t| t.mesh_name == a[3]).collect();
    anyhow::ensure!(!selected.is_empty(), "no mesh part {}", a[3]);
    anyhow::ensure!(selected.iter().all(|t| t.editable), "part {} is not displacement-editable", a[3]);

    let mut buffers = Vec::new();
    for t in &selected {
        buffers.push((t.block_index, morphic::model::read_vertex_positions(&bytes, t.block_index)?));
    }
    let z_top = buffers
        .iter()
        .flat_map(|(_, p)| p.iter().map(|v| v[2]))
        .fold(f32::MIN, f32::max);
    let z_bot = buffers
        .iter()
        .flat_map(|(_, p)| p.iter().map(|v| v[2]))
        .fold(f32::MAX, f32::min);
    for (block, positions) in &buffers {
        let moved: Vec<[f32; 3]> = positions
            .iter()
            .map(|v| [v[0], v[1], z_top - (z_top - v[2]) * factor])
            .collect();
        bytes = morphic::model::replace_vertex_positions(&bytes, *block, &moved)?;
    }
    std::fs::write(&a[2], &bytes)?;
    println!(
        "{}: z {z_bot:.2}..{z_top:.2} -> {:.2}..{z_top:.2} ({} buffer(s))",
        a[3],
        z_top - (z_top - z_bot) * factor,
        buffers.len()
    );
    Ok(())
}
