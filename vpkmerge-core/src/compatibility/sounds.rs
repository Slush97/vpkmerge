//! Restore only an absent tagged-sound binding. Other model game data stays
//! byte-for-byte intact, including authored cameras, particles and constraints.
use std::collections::BTreeSet;

use anyhow::{bail, ensure, Context, Result};
use morphic::kv3::Value;

use super::{CompatibilityRepairer, Document};

mod references;

const HEADER: &str = "<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} format:vmdlkeys4:version{9f5c1002-f19f-470c-bd42-c3c26c063d33} -->";
const SETTINGS: &str = "TaggedSoundSettings_t";

struct Member {
    key: String,
    start: usize,
    node: Node,
}

struct Node {
    end: usize,
    value: TextValue,
}

enum TextValue {
    Object(Vec<Member>),
    Array(Vec<Node>),
    Resource(String),
    Other,
}

// This bounded text-KV3 subset deliberately rejects unsupported syntax. Parsing
// values rather than searching text prevents comments, strings or nested fields
// from masquerading as a top-level binding.
struct Parser<'a> {
    text: &'a str,
    at: usize,
    budget: usize,
}

impl Parser<'_> {
    fn trivia(&mut self) -> Result<()> {
        loop {
            while self
                .text
                .as_bytes()
                .get(self.at)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.at += 1;
            }
            let remaining = &self.text[self.at..];
            if remaining.starts_with("//") {
                self.at += remaining.find('\n').unwrap_or(remaining.len());
            } else if remaining.starts_with("/*") {
                self.at += remaining
                    .find("*/")
                    .context("Unclosed model metadata comment")?
                    + 2;
            } else {
                return Ok(());
            }
        }
    }

    fn eat(&mut self, byte: u8) -> Result<bool> {
        self.trivia()?;
        if self.text.as_bytes().get(self.at) == Some(&byte) {
            self.at += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn token(&mut self) -> Result<String> {
        self.trivia()?;
        let start = self.at;
        if self.text.as_bytes().get(self.at) == Some(&b'"') {
            ensure!(
                !self.text[self.at..].starts_with("\"\"\""),
                "Multiline model strings require a separate adapter"
            );
            self.at += 1;
            while let Some(&byte) = self.text.as_bytes().get(self.at) {
                self.at += 1;
                if byte == b'\\' {
                    self.at += 1;
                } else if byte == b'"' {
                    return Ok(serde_json::from_str(&self.text[start..self.at])?);
                }
            }
            bail!("Unclosed model metadata string");
        }
        while self
            .text
            .as_bytes()
            .get(self.at)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || b"_+-.".contains(byte))
        {
            self.at += 1;
        }
        ensure!(start != self.at, "Unsupported model metadata token");
        Ok(self.text[start..self.at].into())
    }

    fn node(&mut self, depth: usize) -> Result<Node> {
        ensure!(
            depth <= 96 && self.budget > 0,
            "Model metadata exceeds repair limits"
        );
        self.budget -= 1;
        let value = if self.eat(b'{')? {
            let mut members = Vec::new();
            let mut keys = BTreeSet::new();
            while !self.eat(b'}')? {
                self.trivia()?;
                let start = self.at;
                let key = self.token()?;
                ensure!(
                    keys.insert(key.to_ascii_lowercase()),
                    "Ambiguous model metadata key: {key}"
                );
                ensure!(self.eat(b'=')?, "Missing model metadata assignment");
                let node = self.node(depth + 1)?;
                members.push(Member { key, start, node });
                self.eat(b',')?;
            }
            TextValue::Object(members)
        } else if self.eat(b'[')? {
            let mut items = Vec::new();
            while !self.eat(b']')? {
                items.push(self.node(depth + 1)?);
                if !self.eat(b',')? {
                    ensure!(self.eat(b']')?, "Missing model metadata array separator");
                    break;
                }
            }
            TextValue::Array(items)
        } else {
            let token = self.token()?;
            if self.eat(b':')? {
                self.trivia()?;
                ensure!(
                    self.text.as_bytes().get(self.at) == Some(&b'"'),
                    "Expected a flagged model string"
                );
                let path = self.token()?;
                if token == "resource" {
                    TextValue::Resource(path)
                } else {
                    TextValue::Other
                }
            } else {
                TextValue::Other
            }
        };
        Ok(Node {
            end: self.at,
            value,
        })
    }
}

fn document(text: &str) -> Result<Node> {
    ensure!(
        text.len() <= 8 * 1024 * 1024,
        "Model metadata exceeds repair limit"
    );
    let trimmed = text.trim_start();
    ensure!(trimmed.starts_with(HEADER), "Unknown model metadata format");
    let mut parser = Parser {
        text,
        at: text.len() - trimmed.len() + HEADER.len(),
        budget: 1_000_000,
    };
    let node = parser.node(0)?;
    parser.trivia()?;
    ensure!(parser.at == text.len(), "Trailing model metadata");
    ensure!(
        matches!(node.value, TextValue::Object(_)),
        "Expected model metadata object"
    );
    Ok(node)
}

fn members(node: &Node) -> Result<&[Member]> {
    let TextValue::Object(members) = &node.value else {
        bail!("Expected sound settings object")
    };
    Ok(members)
}

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
