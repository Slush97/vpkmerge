//! Sample an atlas .vtex_c at grid cell centres. Usage: <file.vtex_c> <n>
use morphic::ImageData;
fn main() -> anyhow::Result<()> {
    let f = std::env::args().nth(1).unwrap();
    let n: usize = std::env::args().nth(2).unwrap().parse()?;
    let bytes = std::fs::read(&f)?;
    let img = morphic::decode(&bytes)?;
    let (w, h) = (img.width as usize, img.height as usize);
    let ImageData::Rgba8(px) = &img.data else {
        anyhow::bail!("not rgba8")
    };
    let cols = (n as f64).sqrt().ceil() as usize;
    let rows = (n + cols - 1) / cols;
    println!("atlas {w}x{h}, {cols}x{rows} cells");
    for i in 0..n {
        let (c, r) = (i % cols, i / cols);
        let cx = (c * w / cols) + w / cols / 2;
        let cy = (r * h / rows) + h / rows / 2;
        let o = (cy * w + cx) * 4;
        println!(
            "cell {i} @({cx},{cy}) = rgb({},{},{})",
            px[o],
            px[o + 1],
            px[o + 2]
        );
    }
    Ok(())
}
