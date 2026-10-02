//! Resolve Deadlock display names from the game's loose localization files.
//!
//! The catalog indexes ([`crate::catalog`], [`crate::texture_catalog`]) key
//! everything by the engine *codename* (`inferno`, `hornet`, `vampirebat`). A UI
//! wants the in-game *display name* (`Infernus`, `Vindicta`, `Mina`). Those
//! strings are **not** in `pak01`: they live in loose Valve-KeyValues `.txt`
//! files under `<game>/citadel/resource/localization/`, keyed by token. The hero
//! roster and its codenames come from `scripts/heroes.vdata_c` inside the pak; the
//! names come from `citadel_gc_hero_names/citadel_gc_hero_names_<lang>.txt`, where
//! token `hero_<codename>` maps straight to the display name (verified on the live
//! build: `hero_inferno -> "Infernus"`, `hero_vampirebat -> "Mina"`, all codenames
//! including the ones whose display name differs from the codename).
//!
//! Ability and item display names live in the same localization tree but join to
//! their assets only through a fuzzy icon-filename / vdata-node mapping; that is a
//! later pass. This module ships the clean, authoritative half: the hero roster.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use morphic::kv3::Value;

/// Default localization language suffix.
pub const DEFAULT_LANG: &str = "english";

/// Codenames in `heroes.vdata_c` that are not real playable heroes (templates,
/// test rigs, bots). Excluded from the roster.
const NON_HERO_CODENAMES: &[&str] = &[
    "base",
    "genericperson",
    "targetdummy",
    "testhero",
    "shieldguy",
];

/// One hero in the roster: its engine codename (the catalog's `hero` key), the
/// resolved display name, and the availability flags from `heroes.vdata_c`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroInfo {
    /// Engine codename, e.g. `inferno`, `hornet`, `vampirebat`. Matches the
    /// `hero` field on [`crate::catalog::VoiceLine`] / [`crate::texture_catalog::TextureEntry`].
    pub codename: String,
    /// In-game display name, e.g. `Infernus`. Falls back to the codename if the
    /// localization file is missing or the token does not resolve.
    pub name: String,
    /// `m_bPlayerSelectable`: the hero can be picked in a normal game.
    pub selectable: bool,
    /// `m_bInDevelopment`: a work-in-progress hero.
    pub in_development: bool,
    /// `m_bDisabled`: the hero is turned off.
    pub disabled: bool,
}

/// Derive the loose localization directory for a pak path:
/// `<...>/citadel/pak01_dir.vpk` -> `<...>/citadel/resource/localization`. Returns
/// `None` if the pak has no parent directory.
#[must_use]
pub fn localization_dir_for_pak(pak_path: impl AsRef<Path>) -> Option<PathBuf> {
    let parent = pak_path.as_ref().parent()?;
    Some(parent.join("resource").join("localization"))
}

/// Decode localization file bytes to text, honoring a UTF-8 or UTF-16 byte-order
/// mark (Valve ships these `.txt` files as UTF-8 with a BOM).
fn decode_loc_bytes(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Parse a Valve-KeyValues localization file's `Tokens` block into a
/// `token -> value` map.
///
/// The file shape is `"lang" { "Language" "x" "Tokens" { "tok:hint" "value" ... } }`.
/// Token/value pairs are exactly the quoted strings nested two braces deep (inside
/// `lang` then `Tokens`); the depth-1 `"Language"`/`"x"` pair is skipped for free.
/// A trailing `:hint` type marker on a key (`hero_inferno:n`) is stripped. `//`
/// line comments and `\"` escapes inside strings are handled.
#[must_use]
pub fn parse_kv_tokens(text: &str) -> BTreeMap<String, String> {
    // Collect every quoted string together with the brace depth it sits at.
    let mut strings_at_depth_2: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    let mut chars = text.chars().peekable();
    let mut in_line_comment = false;

    while let Some(c) = chars.next() {
        if in_line_comment {
            if c == '\n' {
                in_line_comment = false;
            }
            continue;
        }
        match c {
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                in_line_comment = true;
            }
            '{' => depth += 1,
            '}' => depth -= 1,
            '"' => {
                // Read the string body, honoring backslash escapes.
                let mut s = String::new();
                while let Some(ch) = chars.next() {
                    match ch {
                        '\\' => {
                            if let Some(esc) = chars.next() {
                                match esc {
                                    'n' => s.push('\n'),
                                    't' => s.push('\t'),
                                    other => s.push(other),
                                }
                            }
                        }
                        '"' => break,
                        other => s.push(other),
                    }
                }
                if depth == 2 {
                    strings_at_depth_2.push(s);
                }
            }
            _ => {}
        }
    }

    // Pair them up: even index = key, odd = value.
    let mut tokens = BTreeMap::new();
    let mut it = strings_at_depth_2.into_iter();
    while let (Some(key), Some(value)) = (it.next(), it.next()) {
        let key = key.split(':').next().unwrap_or(&key).to_owned();
        tokens.insert(key, value);
    }
    tokens
}

/// Load and parse one localization token file from disk.
pub fn load_token_file(path: impl AsRef<Path>) -> Result<BTreeMap<String, String>> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let text = decode_loc_bytes(&bytes);
    Ok(parse_kv_tokens(&text))
}

/// Load the hero-name token map (`hero_<codename> -> display name`) for `lang`
/// from a localization directory. Returns an empty map (not an error) if the file
/// is absent, so callers degrade to codenames.
pub fn hero_name_tokens(loc_dir: impl AsRef<Path>, lang: &str) -> Result<BTreeMap<String, String>> {
    let path = loc_dir
        .as_ref()
        .join("citadel_gc_hero_names")
        .join(format!("citadel_gc_hero_names_{lang}.txt"));
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    load_token_file(path)
}

/// Build the hero roster: every real hero in `scripts/heroes.vdata_c` (inside
/// `vpk_path`), each with its codename, availability flags, and display name
/// resolved from the loose localization at `loc_dir`.
///
/// `loc_dir` defaults to [`localization_dir_for_pak`] of `vpk_path` when `None`.
/// If the localization file is missing the names fall back to the codename, so
/// this still returns the roster (with flags) on an install without the loose
/// localization tree.
pub fn build_hero_roster(
    vpk_path: impl AsRef<Path>,
    loc_dir: Option<&Path>,
    lang: &str,
) -> Result<Vec<HeroInfo>> {
    let vpk_path = vpk_path.as_ref();

    let names = match loc_dir
        .map(Path::to_path_buf)
        .or_else(|| localization_dir_for_pak(vpk_path))
    {
        Some(dir) => hero_name_tokens(dir, lang)?,
        None => BTreeMap::new(),
    };

    let bytes = crate::read_vpk_entry(vpk_path, "scripts/heroes.vdata_c")
        .context("reading scripts/heroes.vdata_c")?;
    let root = morphic::decode_kv3_resource(&bytes).context("decoding heroes.vdata_c")?;
    let Value::Object(pairs) = &root else {
        anyhow::bail!("heroes.vdata_c root is not an object");
    };

    let mut roster = Vec::new();
    for (key, node) in pairs {
        let Some(codename) = key.strip_prefix("hero_") else {
            continue;
        };
        if NON_HERO_CODENAMES.contains(&codename) {
            continue;
        }
        // Only nodes that actually carry the hero flags are heroes.
        let player_selectable = node.get("m_bPlayerSelectable").and_then(Value::as_bool);
        let disabled = node.get("m_bDisabled").and_then(Value::as_bool);
        if player_selectable.is_none() && disabled.is_none() {
            continue;
        }
        let disabled = disabled.unwrap_or(false);
        let in_development = node
            .get("m_bInDevelopment")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // Builds from 2026-09 on dropped `m_bPlayerSelectable`; a hero is
        // pickable when it is neither disabled nor in development.
        let selectable = player_selectable.unwrap_or(!disabled && !in_development);

        let name = names
            .get(key)
            .cloned()
            .unwrap_or_else(|| codename.to_owned());

        roster.push(HeroInfo {
            codename: codename.to_owned(),
            name,
            selectable,
            in_development,
            disabled,
        });
    }

    roster.sort_by(|a, b| a.codename.cmp(&b.codename));
    Ok(roster)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}\"lang\"\r\n{\r\n\t\"Language\"\t\"english\"\r\n\t\"Tokens\"\r\n\t{\r\n\t\t// Hero names\r\n\t\t\"hero_inferno:n\" \"Infernus\"\r\n\t\t\"hero_inferno_search:n\" \"Infernus\"\r\n\t\t\"hero_ghost:n\" \"Lady Geist\"\r\n\t\t\"hero_krill:n\" \"Mo & Krill\"\r\n\t}\r\n}\r\n";

    #[test]
    fn parses_token_pairs_and_strips_hint() {
        let tokens = parse_kv_tokens(SAMPLE);
        assert_eq!(
            tokens.get("hero_inferno").map(String::as_str),
            Some("Infernus")
        );
        assert_eq!(
            tokens.get("hero_ghost").map(String::as_str),
            Some("Lady Geist")
        );
        assert_eq!(
            tokens.get("hero_krill").map(String::as_str),
            Some("Mo & Krill")
        );
        // The depth-1 Language/english pair is not a token.
        assert_eq!(tokens.get("Language"), None);
        assert!(!tokens.contains_key("english"));
    }

    #[test]
    fn decode_strips_utf8_bom() {
        let bytes = [0xEF, 0xBB, 0xBF, b'h', b'i'];
        assert_eq!(decode_loc_bytes(&bytes), "hi");
    }

    #[test]
    fn decode_handles_utf16le_bom() {
        let mut bytes = vec![0xFF, 0xFE];
        for u in "hi".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_loc_bytes(&bytes), "hi");
    }

    #[test]
    fn comments_and_escapes_are_handled() {
        let text =
            "\"lang\"\n{\n\"Tokens\"\n{\n// a comment with \"quotes\"\n\"a:n\" \"line\\nbreak\"\n}\n}";
        let tokens = parse_kv_tokens(text);
        assert_eq!(tokens.get("a").map(String::as_str), Some("line\nbreak"));
        assert_eq!(tokens.len(), 1);
    }

    #[test]
    fn loc_dir_derives_from_pak_path() {
        let dir = localization_dir_for_pak("/games/Deadlock/game/citadel/pak01_dir.vpk").unwrap();
        assert!(dir.ends_with("citadel/resource/localization"));
    }
}
