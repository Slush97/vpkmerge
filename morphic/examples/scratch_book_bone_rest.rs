// Throwaway: model-space rest origins of the book bone chain vs the prop bounds.
// usage: cargo run -p morphic --example scratch_book_bone_rest -- <vpk> <model entry>
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).unwrap();
    let bytes = vpk.get_file(&a[2]).unwrap().read_all().unwrap();
    let model = morphic::model::decode(&bytes).unwrap();

    println!("bone                  parent            rest origin (model space)");
    for bone in &model.skeleton.bones {
        let n = &bone.name;
        if n.contains("book") || n.contains("cover") || n.contains("page") || n.contains("clasp") {
            let m = bone.global_bind.m;
            let parent = bone
                .parent
                .map(|p| model.skeleton.bones[p].name.clone())
                .unwrap_or_else(|| "-".into());
            println!(
                "{n:<22}{parent:<18}[{:8.2},{:8.2},{:8.2}]",
                m[12], m[13], m[14]
            );
        }
    }
}
