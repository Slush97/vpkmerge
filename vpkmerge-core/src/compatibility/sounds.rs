//! Restore only an absent tagged-sound binding. Other model game data stays
//! byte-for-byte intact, including authored cameras, particles and constraints.
use anyhow::{bail, ensure, Context, Result};
use morphic::kv3::Value;

use super::{CompatibilityRepairer, Document};

#[cfg(test)]
use crate::model_metadata::HEADER;
use crate::model_metadata::{document, members, Member, Node, TextValue};

mod references;

const SETTINGS: &str = "TaggedSoundSettings_t";

fn binding(node: &Node) -> Result<Option<&Member>> {
    Ok(members(node)?
        .iter()
        .find(|member| member.key.eq_ignore_ascii_case(SETTINGS)))
}

fn metadata(value: &Value) -> Option<&str> {
    value.get("m_modelInfo")?.get("m_keyValueText")?.as_str()
}

pub(super) fn repair(
    source: &[u8],
    base: &[u8],
    game: &mut CompatibilityRepairer,
) -> Result<Option<(Vec<u8>, usize)>> {
    let mut old = Document::read(source)?;
    let current = Document::read(base)?;
    let (Some(authored), Some(stock)) = (metadata(&old.value), metadata(&current.value)) else {
        return Ok(None);
    };
    let stock_doc = document(stock)?;
    let Some(settings) = binding(&stock_doc)? else {
        return Ok(None);
    };
    let authored_doc = document(authored)?;
    // Even an explicitly empty block may intentionally silence these sounds.
    if binding(&authored_doc)?.is_some() {
        return Ok(None);
    }
    let fields = members(&settings.node)?;
    ensure!(
        fields.len() == 1 && fields[0].key == "m_taggedSounds",
        "Unknown tagged-sound settings schema"
    );
    let TextValue::Array(refs) = &fields[0].node.value else {
        bail!("Expected tagged-sound references")
    };
    ensure!(!refs.is_empty(), "Empty stock tagged-sound references");
    let mut paths = Vec::new();
    for reference in refs {
        let TextValue::Resource(path) = &reference.value else {
            bail!("Expected a tagged-sound resource reference")
        };
        ensure!(
            path.starts_with("scripts/tagged_sounds/")
                && std::path::Path::new(path)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("vdata"))
                && path
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b)),
            "Unexpected tagged-sound path"
        );
        let bytes = game
            .game_bytes(&format!("{path}_c"))?
            .context("Tagged-sound table is missing from the installed game")?;
        let table = Document::read(&bytes)?;
        ensure!(
            table.value.get("generic_data_type").and_then(Value::as_str) == Some("CTaggedSound_t"),
            "Unknown tagged-sound table schema"
        );
        paths.push(path.as_str());
    }
    let at = authored_doc.end - 1;
    let updated = format!(
        "{}\n\t{}\n{}",
        &authored[..at],
        &stock[settings.start..settings.node.end],
        &authored[at..]
    );
    document(&updated)?;
    *old.value
        .get_mut("m_modelInfo")
        .and_then(|v| v.get_mut("m_keyValueText"))
        .context("Missing model metadata")? = Value::String(updated);
    let updated = old.write(source)?;
    Ok(Some((references::restore(&updated, base, &paths)?, 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_distinguishes_nested_quoted_and_commented_bindings() -> Result<()> {
        let text = format!("{HEADER}\n{{ // TaggedSoundSettings_t = {{ }}\n note = \"TaggedSoundSettings_t = {{ }}\" nested = {{ TaggedSoundSettings_t = {{ }} }} }}");
        assert!(binding(&document(&text)?)?.is_none());
        let text = format!("{HEADER} {{ \"TaggedSoundSettings_t\" = {{ }} }}");
        assert!(binding(&document(&text)?)?.is_some());
        for body in [
            "{ x = [ 1 2 ] }",
            "{ x = { } X = { } }",
            "{ x = /* open }",
            "{ x = \"open }",
            "{} extra",
            "{ x = [ } ] }",
        ] {
            assert!(document(&format!("{HEADER}{body}")).is_err(), "{body}");
        }
        Ok(())
    }
}
