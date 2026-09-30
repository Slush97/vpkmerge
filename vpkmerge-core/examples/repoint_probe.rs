// Proof that a particle's sprite-sheet reference can be repointed byte-faithfully.
//
// Reads one `.vpcf_c`, rewrites every `m_Renderers[i]/m_vecTexturesInput[j]/m_hTexture`
// whose current value matches OLD to NEW via `patch_kv3_resource_strings_adding`
// (the string-table-growing patcher, since NEW is not already interned), then
// re-decodes the result and prints the before/after refs.
//
// This is the mechanism the fire -> water retarget rides on: no re-encode, so the
// KV3 v5 framing / value flags / typed-array tags the particle loader depends on
// survive intact.
//
// usage: cargo run -p vpkmerge-core --example repoint_probe -- <vpk> <entry> <OLD> <NEW>
use morphic::kv3::{Seg, Value};

/// Every `m_hTexture` path under the renderers, with its KV3 path.
fn texture_inputs(tree: &Value) -> Vec<(Vec<Seg>, String)> {
    let mut out = Vec::new();
    let Some(renderers) = tree.get("m_Renderers").and_then(Value::as_array) else {
        return out;
    };
    for (ri, renderer) in renderers.iter().enumerate() {
        let Some(inputs) = renderer.get("m_vecTexturesInput").and_then(Value::as_array) else {
            continue;
        };
        for (ii, input) in inputs.iter().enumerate() {
            if let Some(tex) = input.get("m_hTexture").and_then(Value::as_str) {
                out.push((
                    vec![
                        Seg::Key("m_Renderers".to_string()),
                        Seg::Index(ri),
                        Seg::Key("m_vecTexturesInput".to_string()),
                        Seg::Index(ii),
                        Seg::Key("m_hTexture".to_string()),
                    ],
                    tex.to_string(),
                ));
            }
        }
    }
    out
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: repoint_probe <vpk> <entry> <OLD> <NEW>");
        std::process::exit(2);
    }
    let (vpk, entry, old, new) = (&a[1], &a[2], &a[3], &a[4]);

    let bytes = vpkmerge_core::read_vpk_entry(vpk, entry)?;
    let tree = morphic::decode_kv3_resource(&bytes)?;

    let before = texture_inputs(&tree);
    println!("BEFORE ({} texture inputs)", before.len());
    for (_, t) in &before {
        println!("    {t}");
    }

    let edits: Vec<(Vec<Seg>, String)> = before
        .iter()
        .filter(|(_, t)| t == old)
        .map(|(p, _)| (p.clone(), new.clone()))
        .collect();
    if edits.is_empty() {
        anyhow::bail!("no texture input matches {old}");
    }
    println!("\n{} edit(s): {old} -> {new}", edits.len());

    let patched = morphic::patch_kv3_resource_strings_adding(&bytes, &edits)?;

    // The whole point: the patched bytes must still decode as the same tree shape.
    let retree = morphic::decode_kv3_resource(&patched)?;
    let after = texture_inputs(&retree);
    println!("\nAFTER ({} texture inputs)", after.len());
    for (_, t) in &after {
        println!("    {t}");
    }

    anyhow::ensure!(
        after.len() == before.len(),
        "texture input count changed: {} -> {}",
        before.len(),
        after.len()
    );
    let got = after.iter().filter(|(_, t)| t == new).count();
    anyhow::ensure!(
        got == edits.len(),
        "expected {} new refs, got {got}",
        edits.len()
    );
    anyhow::ensure!(
        !after.iter().any(|(_, t)| t == old),
        "old ref still present"
    );

    println!(
        "\nOK: {} -> {} took, tree shape preserved, {} bytes -> {} bytes",
        old,
        new,
        bytes.len(),
        patched.len()
    );
    Ok(())
}
