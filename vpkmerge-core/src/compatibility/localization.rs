use std::collections::BTreeSet;

use anyhow::{bail, ensure, Context, Result};

enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

fn text(bytes: &[u8]) -> Result<(String, Encoding)> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        ensure!(
            bytes.len().is_multiple_of(2),
            "Truncated UTF-16 localization"
        );
        let le = bytes[0] == 0xff;
        let units: Vec<_> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| {
                if le {
                    u16::from_le_bytes(b)
                } else {
                    u16::from_be_bytes(b)
                }
            })
            .collect();
        return Ok((
            String::from_utf16(&units)?,
            if le {
                Encoding::Utf16Le
            } else {
                Encoding::Utf16Be
            },
        ));
    }
    let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    Ok((
        std::str::from_utf8(if bom { &bytes[3..] } else { bytes })?.into(),
        if bom {
            Encoding::Utf8Bom
        } else {
            Encoding::Utf8
        },
    ))
}

fn encode(text: &str, encoding: &Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat(),
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let le = matches!(encoding, Encoding::Utf16Le);
            let mut out = if le {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in text.encode_utf16() {
                out.extend(if le {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            out
        }
    }
}

struct Token {
    value: String,
    start: usize,
    end: usize,
}

// Preserve the original source text, including comments, hints and escapes.
// Conditionals are declined because appending outside their scope changes meaning.
fn lex(text: &str) -> Result<Vec<Token>> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos].is_ascii_whitespace() {
            pos += 1;
            continue;
        }
        if bytes.get(pos..pos + 2) == Some(b"//") {
            while pos < bytes.len() && bytes[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        let start = pos;
        if matches!(bytes[pos], b'{' | b'}') {
            pos += 1;
            tokens.push(Token {
                value: text[start..pos].into(),
                start,
                end: pos,
            });
            continue;
        }
        ensure!(
            bytes[pos] == b'"',
            "Unsupported localization syntax at byte {pos}"
        );
        pos += 1;
        let mut value = String::new();
        let mut closed = false;
        while pos < bytes.len() {
            let ch = text[pos..]
                .chars()
                .next()
                .context("Invalid localization string")?;
            pos += ch.len_utf8();
            match ch {
                '"' => {
                    closed = true;
                    break;
                }
                '\\' => {
                    let escaped = text[pos..]
                        .chars()
                        .next()
                        .context("Truncated localization escape")?;
                    pos += escaped.len_utf8();
                    value.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        other => other,
                    });
                }
                other => value.push(other),
            }
        }
        ensure!(closed, "Unterminated localization string");
        tokens.push(Token {
            value,
            start,
            end: pos,
        });
    }
    Ok(tokens)
}

fn normalized(key: &str) -> String {
    key.split(':').next().unwrap_or(key).to_lowercase()
}

fn table(text: &str) -> Result<(Vec<(String, String)>, usize)> {
    let tokens = lex(text)?;
    ensure!(
        tokens.len() >= 6 && tokens[0].value.eq_ignore_ascii_case("lang") && tokens[1].value == "{",
        "Missing lang block"
    );
    let mut pos = 2;
    let mut rows = None;
    let mut close = 0;
    while pos + 1 < tokens.len() && tokens[pos].value != "}" {
        if tokens[pos].value.eq_ignore_ascii_case("tokens") {
            ensure!(
                rows.is_none() && tokens[pos + 1].value == "{",
                "Ambiguous Tokens block"
            );
            pos += 2;
            let mut pairs = Vec::new();
            let mut keys = BTreeSet::new();
            while pos < tokens.len() && tokens[pos].value != "}" {
                let key = tokens.get(pos).context("Missing token key")?;
                let value = tokens.get(pos + 1).context("Missing token value")?;
                ensure!(
                    !matches!(key.value.as_str(), "{" | "}")
                        && !matches!(value.value.as_str(), "{" | "}"),
                    "Nested or incomplete token"
                );
                ensure!(
                    keys.insert(normalized(&key.value)),
                    "Duplicate localization token"
                );
                pairs.push((
                    normalized(&key.value),
                    text[key.start..value.end].to_string(),
                ));
                pos += 2;
            }
            close = tokens.get(pos).context("Unclosed Tokens block")?.start;
            rows = Some(pairs);
            pos += 1;
        } else {
            ensure!(tokens[pos + 1].value != "{", "Unknown localization block");
            pos += 2;
        }
    }
    ensure!(
        pos + 1 == tokens.len() && tokens[pos].value == "}",
        "Unclosed or extra localization blocks"
    );
    Ok((rows.context("Missing Tokens block")?, close))
}

pub(super) fn repair(source: &[u8], base: &[u8]) -> Result<Option<(Vec<u8>, usize)>> {
    let (mut source, encoding) = text(source)?;
    let (base, _) = text(base)?;
    let (old, insert) = table(&source)?;
    let (current, _) = table(&base)?;
    let keys: BTreeSet<_> = old.iter().map(|(key, _)| key).collect();
    let missing: Vec<_> = current
        .iter()
        .filter(|(key, _)| !keys.contains(key))
        .collect();
    if missing.is_empty() {
        return Ok(None);
    }
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut added = newline.to_owned();
    for (_, pair) in &missing {
        added.push_str("\t\t");
        added.push_str(pair);
        added.push_str(newline);
    }
    source.insert_str(insert, &added);
    let (result, _) = table(&source)?;
    if result.len() != old.len() + missing.len() {
        bail!("Localization verification failed");
    }
    Ok(Some((encode(&source, &encoding), missing.len())))
}
