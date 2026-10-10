//! Merge a reference hero model's `m_keyValueText` sections INTO a compiled
//! `.vmdl_c`, keeping the target's OWN sections where both define one.
//!
//! Why not a wholesale copy (inject_keyvaluetext)? The hero camera's
//! over-the-shoulder offset is driven by the NATIVELY-compiled `AttachmentCameraData`
//! (baked by resourcecompiler from the source's per-attachment "Attachment Camera
//! Preview" nodes). Replacing the whole blob with the donor's de-links that native
//! bake -> the camera re-centers. But the target's native blob lacks
//! `CitadelCameraSettings_t` (camera height/back/side offsets) and the procedural
//! rig (`BoneConstraintList`/`ikdata`/`LookAtList`/`FeetSettings`). So we MERGE:
//! keep the target's native sections (incl `AttachmentCameraData`), add the donor's
//! sections the target doesn't have. Result = native camera bake + donor height + rig.
//!
//! The blob is `vmdlkeys` text: a top-level `{ KEY = <{...}|[...]|scalar> ... }`.
//! We split each into ordered top-level sections (brace/bracket matched), union by
//! key (target wins on conflict), and reassemble, then patch via
//! `patch_kv3_resource_strings_adding` (no re-encode).
//!
//! Usage: merge_keyvaluetext <in.vmdl_c> <ref.vmdl_c> <out.vmdl_c>
use morphic::kv3::{Seg, Value};

fn key_value_text(bytes: &[u8]) -> String {
    let tree = morphic::decode_kv3_resource(bytes).expect("decode DATA");
    tree.get("m_modelInfo")
        .and_then(|m| m.get("m_keyValueText"))
        .and_then(Value::as_str)
        .expect("m_modelInfo.m_keyValueText")
        .to_owned()
}

/// Split top-level `vmdlkeys` text into ordered (key, full_span) sections.
/// `full_span` is the exact substring from the key's line start through the end
/// of its value, preserving original tabs/newlines.
fn sections(text: &str) -> Vec<(String, String)> {
    let b = text.as_bytes();
    // skip the `<!-- kv3 encoding... -->` header (its version{guid} has braces).
    let after_header = text.find("-->").map_or(0, |p| p + 3);
    // find the outer content '{' after the header
    let mut i = match text[after_header..].find('{') {
        Some(p) => after_header + p + 1,
        None => return Vec::new(),
    };
    let n = b.len();
    let mut out = Vec::new();
    loop {
        // skip whitespace to the next token
        while i < n && (b[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= n || b[i] == b'}' {
            break;
        }
        let key_start = i;
        // read key identifier
        while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
            i += 1;
        }
        let key = text[key_start..i].to_string();
        // skip ` = ` and any whitespace/newline up to the value
        while i < n && (b[i] as char).is_whitespace() || (i < n && b[i] == b'=') {
            i += 1;
        }
        // read value
        if i < n && (b[i] == b'{' || b[i] == b'[') {
            let (open, close) = if b[i] == b'{' {
                (b'{', b'}')
            } else {
                (b'[', b']')
            };
            let mut depth = 0i32;
            let mut in_str = false;
            while i < n {
                let c = b[i];
                if c == b'"' {
                    in_str = !in_str;
                } else if !in_str {
                    if c == open {
                        depth += 1;
                    } else if c == close {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                }
                i += 1;
            }
        } else {
            // scalar value: read to end of line
            while i < n && b[i] != b'\n' {
                i += 1;
            }
        }
        // include the leading indentation of the key line in the span
        let mut span_start = key_start;
        while span_start > 0 && (b[span_start - 1] == b'\t' || b[span_start - 1] == b' ') {
            span_start -= 1;
        }
        out.push((key, text[span_start..i].to_string()));
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 4 {
        eprintln!("usage: merge_keyvaluetext <in.vmdl_c> <ref.vmdl_c> <out.vmdl_c>");
        std::process::exit(2);
    }
    let (inp, refp, out) = (&a[1], &a[2], &a[3]);
    let in_bytes = std::fs::read(inp).expect("read in");
    let ref_bytes = std::fs::read(refp).expect("read ref");

    let in_kvt = key_value_text(&in_bytes);
    let ref_kvt = key_value_text(&ref_bytes);

    let in_secs = sections(&in_kvt);
    let ref_secs = sections(&ref_kvt);
    let in_keys: std::collections::HashSet<&str> =
        in_secs.iter().map(|(k, _)| k.as_str()).collect();

    let added: Vec<&(String, String)> = ref_secs
        .iter()
        .filter(|(k, _)| !in_keys.contains(k.as_str()))
        .collect();

    println!(
        "in sections: {:?}",
        in_secs.iter().map(|(k, _)| k).collect::<Vec<_>>()
    );
    println!(
        "adding from ref: {:?}",
        added.iter().map(|(k, _)| k).collect::<Vec<_>>()
    );

    // Preserve the original `<!-- kv3 ... -->` header (engine needs it to parse).
    let header = match in_kvt.find("-->") {
        Some(p) => &in_kvt[..p + 3],
        None => "",
    };
    // Reassemble: header, then keep all in sections in order, then the new ref sections.
    let mut body = String::new();
    for (_, span) in &in_secs {
        body.push_str(span);
        body.push('\n');
    }
    for (_, span) in &added {
        body.push_str(span);
        body.push('\n');
    }
    let merged = if header.is_empty() {
        format!("{{\n{body}}}")
    } else {
        format!("{header}\n{{\n{body}}}")
    };

    // sanity: merged keeps native AttachmentCameraData and gains the height + rig
    assert!(
        merged.contains("AttachmentCameraData"),
        "lost AttachmentCameraData"
    );
    assert!(
        merged.contains("CitadelCameraSettings_t"),
        "merge did not add CitadelCameraSettings_t"
    );

    let edit = (
        vec![
            Seg::Key("m_modelInfo".into()),
            Seg::Key("m_keyValueText".into()),
        ],
        merged.clone(),
    );
    let patched = morphic::patch_kv3_resource_strings_adding(&in_bytes, &[edit])
        .expect("patch m_keyValueText");
    let got = key_value_text(&patched);
    assert_eq!(got, merged, "m_keyValueText did not take");

    std::fs::write(out, &patched).expect("write out");
    println!(
        "wrote {out} ({} bytes); merged keyValueText {} chars (was {} native + ref extras)",
        patched.len(),
        merged.len(),
        in_kvt.len()
    );
}
