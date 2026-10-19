//! Bounded parsing of model game-data text with source spans.
use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeSet;

pub(crate) const HEADER: &str = "<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} format:vmdlkeys4:version{9f5c1002-f19f-470c-bd42-c3c26c063d33} -->";

pub(crate) struct Member {
    pub(crate) key: String,
    pub(crate) start: usize,
    pub(crate) node: Node,
}

pub(crate) struct Node {
    pub(crate) end: usize,
    pub(crate) value: TextValue,
}

pub(crate) enum TextValue {
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

pub(crate) fn document(text: &str) -> Result<Node> {
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

pub(crate) fn members(node: &Node) -> Result<&[Member]> {
    let TextValue::Object(members) = &node.value else {
        bail!("Expected model settings object")
    };
    Ok(members)
}

/// The complete top-level setting assignment, excluding surrounding trivia.
pub(crate) fn block<'a>(text: &'a str, name: &str) -> Result<Option<&'a str>> {
    let root = document(text)?;
    Ok(members(&root)?
        .iter()
        .find(|m| m.key.eq_ignore_ascii_case(name))
        .map(|m| &text[m.start..m.node.end]))
}

/// Append an absent setting without rewriting any authored text.
pub(crate) fn insert_missing(text: &str, setting: &str, name: &str) -> Result<Option<String>> {
    let root = document(text)?;
    if members(&root)?
        .iter()
        .any(|m| m.key.eq_ignore_ascii_case(name))
    {
        return Ok(None);
    }
    let at = root.end - 1;
    let updated = format!("{}\n\t{}\n{}", &text[..at], setting, &text[at..]);
    document(&updated)?;
    Ok(Some(updated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_only_absent_top_level_camera_settings() {
        let original = format!("{HEADER}\n{{ // CitadelCameraSettings_t = {{}}\n nested = {{ CitadelCameraSettings_t = {{}} }} note = \"CitadelCameraSettings_t\" }}");
        let settings = "CitadelCameraSettings_t = { m_flCameraHeightStanding = 60.0 }";
        assert!(block(&original, "CitadelCameraSettings_t")
            .unwrap()
            .is_none());
        let updated = insert_missing(&original, settings, "CitadelCameraSettings_t")
            .unwrap()
            .unwrap();
        assert_eq!(
            block(&updated, "CitadelCameraSettings_t").unwrap(),
            Some(settings)
        );
        assert!(
            insert_missing(&updated, settings, "CitadelCameraSettings_t")
                .unwrap()
                .is_none()
        );
        let authored = format!("{HEADER} {{ CitadelCameraSettings_t = {{ custom = 123 }} }}");
        assert!(
            insert_missing(&authored, settings, "CitadelCameraSettings_t")
                .unwrap()
                .is_none()
        );
    }
}
