//! Build the Foundry asset catalog from a Deadlock install.
//!
//! The marquee piece is the **voice-line search index**: one searchable row per
//! VO sound event ([`VoiceLine`]), carrying the event name, hero, clip path(s), a
//! human-readable label, and the engine duration. A UI searches it (by the
//! descriptive label, e.g. "ally atlas killed in lane") and forges a swap for the
//! chosen event.
//!
//! Note on subtitles: the game also ships a compiled caption database
//! (`resource/localization/citadel_generated_vo/citadel_generated_vo_<lang>.dat`,
//! VCCD v2, keyed by `crc32(token)`). [`CaptionDb`] reads it, but empirically the
//! per-hero VO events all map to *empty* English captions (Deadlock does not
//! subtitle hero combat barks); the authored caption text lives under a separate
//! token namespace not tied to swappable sound events. So caption text is only a
//! best-effort enrichment on the index, almost always `None` for hero VO. The
//! searchable text is the descriptive event name itself. Findings pinned down
//! against the live pak; see `grimoire/docs/foundry-tab-design.md`.
//!
//! Pure Rust, no extra dependencies (the caption hash is the standard
//! CRC-32/ISO-HDLC, the same one `zlib`/`gzip` use).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use morphic::kv3::Value;

use crate::soundevents::SoundEvents;

/// Entry path of the English compiled VO captions inside `citadel/pak01`. Other
/// languages live at the same relative path inside their `citadel_<lang>` pak.
pub const ENGLISH_CAPTIONS_ENTRY: &str =
    "resource/localization/citadel_generated_vo/citadel_generated_vo_english.dat";

/// The per-hero / announcer VO tree.
const VO_TREE_PREFIX: &str = "soundevents/vo/";
/// Per-hero VO files are named `generated_vo_hero_<code>.vsndevts_c`.
const HERO_VO_STEM: &str = "generated_vo_hero_";

/// The per-hero gameplay-sound tree: one `soundevents/hero/<code>.vsndevts_c`
/// per hero, holding weapon / ability / movement / melee events (the non-VO
/// counterpart to the VO tree).
const HERO_TREE_PREFIX: &str = "soundevents/hero/";

/// One VO sound event, ready to search and swap.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VoiceLine {
    /// Soundevent name, e.g. `bebop_self_ultimate_cast_01_hero_3d`. Usable
    /// verbatim as the swap target in the soundevents layer.
    pub event: String,
    /// Hero codename (`bebop`, `astro`, ...) from the event's `context_name`,
    /// falling back to the source filename. `None` for announcer / non-hero VO.
    pub hero: Option<String>,
    /// Human-readable label derived from the event name, e.g.
    /// `"ally atlas killed in lane"`. This is the searchable text.
    pub label: String,
    /// Clip path(s) the event plays (more than one == a randomizer pool).
    pub vsnd: Vec<String>,
    /// The `.vsndevts_c` entry that defines the event: the file an event swap
    /// reads the pool from (and edits, in collapse mode).
    pub source: String,
    /// Engine playback duration in seconds, if the event records one.
    pub duration: Option<f64>,
    /// Authored English subtitle, if the caption database resolves this event.
    /// Almost always `None` for hero VO; see the module note.
    pub caption: Option<String>,
}

/// A decoded VCCD (Valve Compiled Caption Database).
///
/// Maps `crc32(token)` to caption text. For generated VO the token is the
/// soundevent name, so [`Self::for_event`] resolves a line's subtitle directly.
pub struct CaptionDb {
    by_hash: HashMap<u32, String>,
}

impl CaptionDb {
    /// Parse a `.dat` caption database (VCCD v2).
    ///
    /// Header (24 bytes, little-endian): `magic 'VCCD' | version | numBlocks |
    /// blockSize | dirEntries | dataOffset`, then `dirEntries` 12-byte directory
    /// records (`u32 hash | i32 blockNum | u16 offset | u16 length`), then
    /// UTF-16LE NUL-terminated strings packed into `blockSize` blocks beginning
    /// at `dataOffset`.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 24 || &bytes[0..4] != b"VCCD" {
            bail!("not a VCCD caption database (bad magic)");
        }
        let rd = |off: usize| u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        let version = rd(4);
        if version != 1 && version != 2 {
            bail!("unsupported VCCD version {version}");
        }
        let block_size = rd(12) as usize;
        let dir_entries = rd(16) as usize;
        let data_offset = rd(20) as usize;

        let dir_start = 24usize;
        let dir_bytes = dir_entries
            .checked_mul(12)
            .and_then(|n| n.checked_add(dir_start))
            .context("caption directory size overflow")?;
        if dir_bytes > bytes.len() {
            bail!("caption directory ({dir_entries} entries) runs past end of file");
        }

        let mut by_hash: HashMap<u32, String> = HashMap::with_capacity(dir_entries);
        for i in 0..dir_entries {
            let base = dir_start + i * 12;
            let hash = u32::from_le_bytes(bytes[base..base + 4].try_into().unwrap());
            let block = i32::from_le_bytes(bytes[base + 4..base + 8].try_into().unwrap());
            let offset =
                u16::from_le_bytes(bytes[base + 8..base + 10].try_into().unwrap()) as usize;
            let length = usize::from(u16::from_le_bytes(
                bytes[base + 10..base + 12].try_into().unwrap(),
            ));
            let Ok(block) = usize::try_from(block) else {
                continue;
            };
            if length == 0 {
                // Empty caption (common for routine combat callouts): skip it,
                // but never let a later empty dup clobber a real string.
                by_hash.entry(hash).or_default();
                continue;
            }
            let start = data_offset + block * block_size + offset;
            let Some(raw) = bytes.get(start..start + length) else {
                continue;
            };
            let text = decode_utf16le(raw);
            if text.is_empty() {
                by_hash.entry(hash).or_default();
            } else {
                // Prefer a non-empty string on a hash collision.
                by_hash.insert(hash, text);
            }
        }
        Ok(Self { by_hash })
    }

    /// Caption text for a raw token hash, if present and non-empty.
    #[must_use]
    pub fn by_hash(&self, hash: u32) -> Option<&str> {
        self.by_hash
            .get(&hash)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Caption text for a token string (hashed with [`caption_hash`]).
    #[must_use]
    pub fn get(&self, token: &str) -> Option<&str> {
        self.by_hash(caption_hash(token))
    }

    /// Caption text for a soundevent name (the generated-VO token is the event
    /// name verbatim).
    #[must_use]
    pub fn for_event(&self, event: &str) -> Option<&str> {
        self.get(event)
    }

    /// Total directory entries retained (including empty captions).
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    /// Number of distinct hashes carrying non-empty caption text.
    #[must_use]
    pub fn non_empty_count(&self) -> usize {
        self.by_hash.values().filter(|s| !s.is_empty()).count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }
}

/// Build the voice-line search index for one VPK whose caption database it also
/// carries (the base `citadel/pak01` for English). Convenience over
/// [`build_voiceline_index_with_captions`].
pub fn build_voiceline_index(vpk_path: impl AsRef<Path>) -> Result<Vec<VoiceLine>> {
    let vpk_path = vpk_path.as_ref();
    // Builds from 2026-09 on no longer ship the compiled caption DB in pak01.
    // Captions never resolved for hero VO anyway, so its absence only drops the
    // best-effort `caption` field, not the index.
    let captions = match crate::read_vpk_entry(vpk_path, ENGLISH_CAPTIONS_ENTRY) {
        Ok(bytes) => CaptionDb::parse(&bytes)?,
        Err(_) => CaptionDb {
            by_hash: HashMap::new(),
        },
    };
    build_voiceline_index_with_captions(vpk_path, &captions)
}

/// Build the voice-line index from the VO soundevents tree in `vpk_path`,
/// enriching each row with caption text from `captions` where it resolves. Use
/// this overload when the captions live in a different pak (a non-English
/// `citadel_<lang>` pak).
///
/// Every event in `soundevents/vo/*.vsndevts_c` becomes a [`VoiceLine`]; the
/// `base` inheritance template is not an event and is skipped. Lines are sorted
/// by hero then event for a stable index.
pub fn build_voiceline_index_with_captions(
    vpk_path: impl AsRef<Path>,
    captions: &CaptionDb,
) -> Result<Vec<VoiceLine>> {
    let vpk_path = vpk_path.as_ref();
    let vpk =
        valve_pak::open(vpk_path).with_context(|| format!("opening {}", vpk_path.display()))?;

    let mut vo_entries: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.starts_with(VO_TREE_PREFIX) && p.ends_with(".vsndevts_c"))
        .cloned()
        .collect();
    vo_entries.sort();

    let mut lines = Vec::new();
    for entry in &vo_entries {
        let file_hero = hero_from_vo_entry(entry);
        let mut file = vpk
            .get_file(entry)
            .with_context(|| format!("locating {entry}"))?;
        let bytes = file
            .read_all()
            .with_context(|| format!("reading {entry}"))?;
        // A VO file that fails to decode should not sink the whole index.
        let Ok(se) = SoundEvents::from_bytes(bytes) else {
            continue;
        };
        let Value::Object(pairs) = &se.root else {
            continue;
        };
        for (name, event) in pairs {
            // The `base` key is a shared template, not a playable event.
            if name == "base" {
                continue;
            }
            let hero = event
                .get("context_name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| file_hero.clone());
            let vsnd = event_clips(event);
            lines.push(VoiceLine {
                label: pretty_label(name, hero.as_deref()),
                caption: captions.for_event(name).map(str::to_owned),
                duration: event.get("vsnd_duration").and_then(Value::as_f64),
                hero,
                vsnd,
                source: entry.clone(),
                event: name.clone(),
            });
        }
    }

    lines.sort_by(|a, b| a.hero.cmp(&b.hero).then_with(|| a.event.cmp(&b.event)));
    Ok(lines)
}

/// Hero codename from a VO entry path, or `None` for announcer / non-hero VO.
/// `soundevents/vo/generated_vo_hero_bebop.vsndevts_c` -> `Some("bebop")`.
fn hero_from_vo_entry(entry: &str) -> Option<String> {
    let stem = entry.rsplit('/').next()?.strip_suffix(".vsndevts_c")?;
    stem.strip_prefix(HERO_VO_STEM).map(str::to_owned)
}

/// Turn a soundevent name into searchable prose: drop the speaker (hero) prefix,
/// the `_hero_3d` / `_2d` channel suffix and a trailing take number, then spell
/// the underscores as spaces. `bebop_ally_atlas_killed_in_lane_01_hero_3d` with
/// hero `bebop` -> `"ally atlas killed in lane"`.
fn pretty_label(event: &str, hero: Option<&str>) -> String {
    let mut s = event;
    if let Some(h) = hero {
        if let Some(rest) = s.strip_prefix(h) {
            s = rest.trim_start_matches('_');
        }
    }
    for suffix in ["_hero_3d", "_hero_2d", "_3d", "_2d"] {
        if let Some(rest) = s.strip_suffix(suffix) {
            s = rest;
            break;
        }
    }
    // Trailing take/variant segments (`_01`, `_alt`, and combinations like
    // `_01_alt_01`), peeled one segment at a time.
    while let Some((head, tail)) = s.rsplit_once('_') {
        if tail == "alt" || (!tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit())) {
            s = head;
        } else {
            break;
        }
    }
    s.replace('_', " ").trim().to_owned()
}

/// Decode a UTF-16LE byte slice, dropping the trailing NUL terminator(s).
fn decode_utf16le(raw: &[u8]) -> String {
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// CRC-32/ISO-HDLC (the `zlib`/`gzip` CRC): reflected poly `0xEDB88320`, init
/// and xorout `0xFFFFFFFF`. This is the hash VCCD uses to key caption tokens.
#[must_use]
pub fn caption_hash(token: &str) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in token.as_bytes() {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Which broad family a hero gameplay sound belongs to. Derived from the event
/// name's grouping segment (`Haze.Wpn.Fire.Main` -> `Weapon`,
/// `Haze.Finesse.Dagger.Cast` -> `Ability`); see [`build_hero_sound_index`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HeroSoundCategory {
    /// Primary-weapon sounds: fire, reload, zoom, whizby, impact, foley.
    Weapon,
    /// A hero ability (named, e.g. `Finesse`, or slot-tagged `A1`..`A4`).
    Ability,
    /// Locomotion: footsteps, jump/land, mantle, ladder steps.
    Movement,
    /// Melee swing.
    Melee,
    /// Anything that doesn't fit the above (e.g. the progression-page stingers).
    Other,
}

/// One playable hero gameplay sound event (weapon / ability / movement / melee),
/// the non-VO counterpart to [`VoiceLine`]. Read from `soundevents/hero/`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HeroSound {
    /// Soundevent name, e.g. `Haze.Finesse.Dagger.Cast`. Usable verbatim as the
    /// swap target in the soundevents layer.
    pub event: String,
    /// Hero "sound" codename from the source filename stem (`abrams`, `gigawatt`,
    /// `vampirebat`). This is the sound-path namespace, which diverges from the
    /// roster/script codename for some heroes (Abrams' roster codename is `atlas`).
    pub hero: String,
    /// Which family the event belongs to.
    pub category: HeroSoundCategory,
    /// The ability's display name when `category` is `Ability` (e.g. `Finesse`,
    /// `Siphon Life`), else `None`. The grouping key for an ability picker.
    pub ability: Option<String>,
    /// Ability slot 1..4 when the event carries an `A1`..`A4` token (Abrams), else
    /// `None`. Most heroes name abilities instead of slot-tagging them.
    pub slot: Option<u8>,
    /// Human-readable label derived from the event name, e.g. `"Fire Main"`,
    /// `"Dagger Cast"`. The searchable text.
    pub label: String,
    /// Clip path(s) the event plays (more than one == a randomizer pool).
    pub vsnd: Vec<String>,
    /// The `.vsndevts_c` entry that defines the event: the file an event swap
    /// reads the pool from (and edits, in collapse mode).
    pub source: String,
    /// Engine playback duration in seconds, if the event records one.
    pub duration: Option<f64>,
}

/// Build the hero gameplay-sound index from the `soundevents/hero/` tree in
/// `vpk_path`: one [`HeroSound`] per event across every `soundevents/hero/<code>
/// .vsndevts_c` (the shared `_shared` template file and `base` inheritance keys
/// are skipped). Rows are sorted by hero then event for a stable index.
///
/// This is the gameplay-sound counterpart to [`build_voiceline_index`]; together
/// they cover a hero's full audible surface (abilities + gun here, voice barks
/// there).
pub fn build_hero_sound_index(vpk_path: impl AsRef<Path>) -> Result<Vec<HeroSound>> {
    let vpk_path = vpk_path.as_ref();
    let vpk =
        valve_pak::open(vpk_path).with_context(|| format!("opening {}", vpk_path.display()))?;

    let mut entries: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.starts_with(HERO_TREE_PREFIX) && p.ends_with(".vsndevts_c"))
        .cloned()
        .collect();
    entries.sort();

    let mut sounds = Vec::new();
    for entry in &entries {
        let Some(hero) = hero_from_hero_entry(entry) else {
            continue; // `_shared` and any other non-hero file
        };
        let mut file = vpk
            .get_file(entry)
            .with_context(|| format!("locating {entry}"))?;
        let bytes = file
            .read_all()
            .with_context(|| format!("reading {entry}"))?;
        // A file that fails to decode should not sink the whole index.
        let Ok(se) = SoundEvents::from_bytes(bytes) else {
            continue;
        };
        let Value::Object(pairs) = &se.root else {
            continue;
        };
        for (name, event) in pairs {
            // The `base` key is a shared template, not a playable event.
            if name == "base" {
                continue;
            }
            let vsnd = event_clips(event);
            // Events with no clips (pure parameter/template rows) aren't playable.
            if vsnd.is_empty() {
                continue;
            }
            let mut classified = classify_hero_event(name);
            // Most heroes name abilities instead of slot-tagging the event, but
            // the clip path usually still carries an `aN` folder
            // (`sounds/abilities/<code>/a2_batblink/...`). Use it to recover the
            // slot so an ability picker can order 1..4 like in-game.
            if classified.slot.is_none() && classified.category == HeroSoundCategory::Ability {
                classified.slot = slot_from_vsnd(&vsnd);
            }
            sounds.push(HeroSound {
                category: classified.category,
                ability: classified.ability,
                slot: classified.slot,
                label: classified.label,
                duration: event.get("vsnd_duration").and_then(Value::as_f64),
                hero: hero.clone(),
                vsnd,
                source: entry.clone(),
                event: name.clone(),
            });
        }
    }

    sounds.sort_by(|a, b| a.hero.cmp(&b.hero).then_with(|| a.event.cmp(&b.event)));
    Ok(sounds)
}

/// The clip path(s) an event plays: its `vsnd_files`, one string or an array.
fn event_clips(event: &Value) -> Vec<String> {
    match event.get("vsnd_files") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// One playable sound event that belongs to no single hero: the shared hero
/// defaults (`soundevents/hero/_shared`: melee swing, footsteps), plus player,
/// item, NPC, UI, music and ambience events.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SharedSound {
    /// Soundevent name, e.g. `Hero.Default.Melee.Swing`. Usable verbatim as the
    /// swap target in the soundevents layer.
    pub event: String,
    /// The event name with its separators spelled as spaces. The searchable text.
    pub label: String,
    /// Clip path(s) the event plays (more than one == a randomizer pool).
    pub vsnd: Vec<String>,
    /// The `.vsndevts_c` entry that defines the event.
    pub source: String,
    /// Engine playback duration in seconds, if the event records one.
    pub duration: Option<f64>,
}

/// Build the index of every playable sound event outside the VO tree and the
/// per-hero files: the counterpart to [`build_voiceline_index`] and
/// [`build_hero_sound_index`] that completes the swappable surface. Rows are
/// sorted by source file then event.
pub fn build_shared_sound_index(vpk_path: impl AsRef<Path>) -> Result<Vec<SharedSound>> {
    let vpk_path = vpk_path.as_ref();
    let vpk =
        valve_pak::open(vpk_path).with_context(|| format!("opening {}", vpk_path.display()))?;

    let mut entries: Vec<String> = vpk
        .file_paths()
        .filter(|p| {
            p.starts_with("soundevents/")
                && p.ends_with(".vsndevts_c")
                && !p.starts_with(VO_TREE_PREFIX)
                && !(p.starts_with(HERO_TREE_PREFIX) && hero_from_hero_entry(p).is_some())
        })
        .cloned()
        .collect();
    entries.sort();

    let mut sounds = Vec::new();
    for entry in &entries {
        let mut file = vpk
            .get_file(entry)
            .with_context(|| format!("locating {entry}"))?;
        let bytes = file
            .read_all()
            .with_context(|| format!("reading {entry}"))?;
        // A file that fails to decode should not sink the whole index.
        let Ok(se) = SoundEvents::from_bytes(bytes) else {
            continue;
        };
        let Value::Object(pairs) = &se.root else {
            continue;
        };
        for (name, event) in pairs {
            let vsnd = event_clips(event);
            if name == "base" || vsnd.is_empty() {
                continue;
            }
            sounds.push(SharedSound {
                label: name.replace(['.', '_'], " "),
                duration: event.get("vsnd_duration").and_then(Value::as_f64),
                vsnd,
                source: entry.clone(),
                event: name.clone(),
            });
        }
    }

    sounds.sort_by(|a, b| a.source.cmp(&b.source).then_with(|| a.event.cmp(&b.event)));
    Ok(sounds)
}

/// Hero sound-codename from a hero-tree entry path, or `None` for the shared
/// template. `soundevents/hero/abrams.vsndevts_c` -> `Some("abrams")`;
/// `soundevents/hero/_shared.vsndevts_c` -> `None`.
fn hero_from_hero_entry(entry: &str) -> Option<String> {
    let stem = entry.rsplit('/').next()?.strip_suffix(".vsndevts_c")?;
    if stem.starts_with('_') {
        return None;
    }
    Some(stem.to_owned())
}

/// Generic grouping segments that pin a non-ability category. Anything not in
/// these (after the speaker prefix and an optional `A1..A4` slot token) is read
/// as a named ability.
const WEAPON_GROUPS: &[&str] = &[
    "wpn",
    "weapon",
    "zoomin",
    "zoomout",
    "bulletwhizby",
    "whizby",
    "reload",
    "foley",
];
const MOVEMENT_GROUPS: &[&str] = &[
    "footstep",
    "footsteps",
    "movement",
    "jumpland",
    "jump",
    "land",
    "mantle",
    "ladderstep",
    "sprint",
    "slide",
    "dash",
    "rope",
    "zipline",
    "wallrun",
];
const MELEE_GROUPS: &[&str] = &["melee"];
/// Progression-page stingers (`Atlas.Progession.Page.Win.VO`); the in-game key is
/// misspelled `Progession`, so match both spellings.
const OTHER_GROUPS: &[&str] = &["progession", "progression"];

struct ClassifiedEvent {
    category: HeroSoundCategory,
    ability: Option<String>,
    slot: Option<u8>,
    label: String,
}

/// Classify a hero soundevent name into a category + ability/slot + label.
///
/// Events are namespaced `<Speaker>.<Group>.<detail...>`, where the speaker is a
/// hero prefix that can differ from the file stem (Gigawatt's file mixes `Seven.*`
/// and `Gigawatt.*`). A leading literal `Ability.` and an `A1..A4` slot token are
/// peeled first; the grouping segment then decides the category (a known generic,
/// else a named ability). The label is the prose remainder after the speaker.
fn classify_hero_event(event: &str) -> ClassifiedEvent {
    let segs: Vec<&str> = event.split('.').filter(|s| !s.is_empty()).collect();
    // segs[0] is the speaker; a few events lead with a literal `Ability` first.
    let mut i = usize::from(segs.first() == Some(&"Ability"));
    i += 1; // skip the speaker prefix itself
    let after_speaker = &segs[i.min(segs.len())..];

    // Peel an A1..A4 slot token if present (Abrams). The ability name then follows.
    let mut slot = None;
    let mut rest = after_speaker;
    if let Some(first) = rest.first() {
        if first.len() == 2
            && first.as_bytes()[0] == b'A'
            && (b'1'..=b'4').contains(&first.as_bytes()[1])
        {
            slot = Some(first.as_bytes()[1] - b'0');
            rest = &rest[1..];
        }
    }

    let group = rest.first().copied();
    // The label is the detail *after* the grouping segment, since the category
    // (and, for abilities, the ability name) already convey the group. When there
    // is no detail, fall back to the group's own pretty name.
    let detail = rest.get(1..).unwrap_or(&[]);
    let label = if detail.is_empty() {
        prettify_segment(group.unwrap_or(event))
    } else {
        prettify_segments(detail)
    };

    let Some(group) = group else {
        // No grouping segment at all (a bare speaker event); treat as Other.
        return ClassifiedEvent {
            category: HeroSoundCategory::Other,
            ability: None,
            slot,
            label,
        };
    };
    let g = group.to_ascii_lowercase();

    let category = if WEAPON_GROUPS.contains(&g.as_str()) {
        HeroSoundCategory::Weapon
    } else if MOVEMENT_GROUPS.contains(&g.as_str()) {
        HeroSoundCategory::Movement
    } else if MELEE_GROUPS.contains(&g.as_str()) {
        HeroSoundCategory::Melee
    } else if OTHER_GROUPS.contains(&g.as_str()) {
        HeroSoundCategory::Other
    } else {
        // A named group (or one proved an ability by its A1..A4 slot token).
        HeroSoundCategory::Ability
    };

    let ability = if category == HeroSoundCategory::Ability {
        Some(prettify_segment(group))
    } else {
        None
    };

    ClassifiedEvent {
        category,
        ability,
        slot,
        label,
    }
}

/// Recover an ability slot (1..4) from a clip path's `aN` token: a folder
/// `.../a2_batblink/...`, a bare `.../a4/...`, or an infix `..._a1_...`. Returns
/// the first match across `paths`. Mirrors the path convention the Locker's
/// per-ability sound classifier relies on.
fn slot_from_vsnd(paths: &[String]) -> Option<u8> {
    for path in paths {
        let lower = path.to_ascii_lowercase();
        let bytes = lower.as_bytes();
        // Scan for `a` preceded by a `/` or `_` boundary, followed by 1..4 and a
        // `_` or `/` boundary.
        for i in 0..bytes.len() {
            if bytes[i] != b'a' {
                continue;
            }
            let before_ok = i == 0 || bytes[i - 1] == b'/' || bytes[i - 1] == b'_';
            if !before_ok {
                continue;
            }
            let Some(&digit) = bytes.get(i + 1) else {
                continue;
            };
            if !(b'1'..=b'4').contains(&digit) {
                continue;
            }
            let after = bytes.get(i + 2).copied();
            if matches!(after, None | Some(b'_' | b'/')) {
                return Some(digit - b'0');
            }
        }
    }
    None
}

/// Prettify one event segment: split CamelCase (`LoveBites` -> `Love Bites`,
/// `ZoomIn` -> `Zoom In`) and Title-case the words. `Wpn` reads as `Weapon`.
fn prettify_segment(seg: &str) -> String {
    if seg.eq_ignore_ascii_case("wpn") {
        return "Weapon".to_owned();
    }
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for ch in seg.chars() {
        if ch.is_uppercase() && prev_lower && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        cur.push(ch);
        prev_lower = ch.is_lowercase() || ch.is_ascii_digit();
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
        .iter()
        .map(|w| title_case_word(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Prettify a run of segments, joining with spaces (`["Dagger", "Cast"]` ->
/// `"Dagger Cast"`). A leading `Wpn` is spelled `Weapon`.
fn prettify_segments(segs: &[&str]) -> String {
    segs.iter()
        .map(|s| prettify_segment(s))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Upper-case the first character, lower-case the rest (`fire` -> `Fire`,
/// `BulletWhizby` after a split is already one word). Leaves all-caps short tokens
/// like `Lp` readable.
fn title_case_word(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_canonical_check_value() {
        // The standard CRC-32 check vector.
        assert_eq!(caption_hash("123456789"), 0xCBF4_3926);
    }

    /// Build a minimal valid VCCD v2 blob with two tokens and parse it back.
    #[test]
    fn parses_and_resolves_tokens() {
        let block_size = 8192u32;
        let tok_a = "hero_ability_cast_01";
        let tok_b = "hero_self_death_01";
        let text_a = "Witness my power!";
        let text_b = ""; // empty caption, must be dropped

        let enc = |s: &str| -> Vec<u8> {
            let mut v: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
            v.extend_from_slice(&[0, 0]); // NUL terminator
            v
        };
        let data_a = enc(text_a);
        let data_b = enc(text_b);

        let header_len = 24u32;
        let dir_len = 2 * 12;
        let data_offset = header_len + dir_len;
        let len_a = u16::try_from(data_a.len()).unwrap();
        let len_b = u16::try_from(data_b.len()).unwrap();

        let mut buf = Vec::new();
        buf.extend_from_slice(b"VCCD");
        buf.extend_from_slice(&2u32.to_le_bytes()); // version
        buf.extend_from_slice(&1u32.to_le_bytes()); // numBlocks
        buf.extend_from_slice(&block_size.to_le_bytes());
        buf.extend_from_slice(&2u32.to_le_bytes()); // dirEntries
        buf.extend_from_slice(&data_offset.to_le_bytes());

        // Directory: hash, blockNum, offset, length.
        buf.extend_from_slice(&caption_hash(tok_a).to_le_bytes());
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&len_a.to_le_bytes());

        buf.extend_from_slice(&caption_hash(tok_b).to_le_bytes());
        buf.extend_from_slice(&0i32.to_le_bytes());
        buf.extend_from_slice(&len_a.to_le_bytes());
        buf.extend_from_slice(&len_b.to_le_bytes());

        // Data block.
        buf.extend_from_slice(&data_a);
        buf.extend_from_slice(&data_b);

        let db = CaptionDb::parse(&buf).expect("parse VCCD");
        assert_eq!(db.get(tok_a), Some(text_a));
        assert_eq!(db.for_event(tok_a), Some(text_a));
        // Empty caption resolves to None.
        assert_eq!(db.get(tok_b), None);
        // Unknown token.
        assert_eq!(db.get("no_such_event"), None);
        assert_eq!(db.non_empty_count(), 1);
    }

    #[test]
    fn label_is_searchable_prose() {
        assert_eq!(
            pretty_label("bebop_ally_atlas_killed_in_lane_01_hero_3d", Some("bebop")),
            "ally atlas killed in lane"
        );
        assert_eq!(
            pretty_label("bebop_self_ultimate_cast_01_hero_3d", Some("bebop")),
            "self ultimate cast"
        );
        assert_eq!(
            pretty_label(
                "bebop_ally_warden_killed_in_lane_01_alt_01_hero_3d",
                Some("bebop")
            ),
            "ally warden killed in lane"
        );
        // No hero prefix to strip (announcer-style).
        assert_eq!(pretty_label("round_start_01", None), "round start");
    }

    #[test]
    fn hero_parsing_from_vo_path() {
        assert_eq!(
            hero_from_vo_entry("soundevents/vo/generated_vo_hero_bebop.vsndevts_c").as_deref(),
            Some("bebop")
        );
        assert_eq!(
            hero_from_vo_entry("soundevents/vo/announcer.vsndevts_c"),
            None
        );
    }

    #[test]
    fn rejects_non_vccd() {
        assert!(CaptionDb::parse(b"not a caption db").is_err());
    }

    #[test]
    fn hero_from_hero_entry_strips_stem_and_skips_shared() {
        assert_eq!(
            hero_from_hero_entry("soundevents/hero/abrams.vsndevts_c").as_deref(),
            Some("abrams")
        );
        assert_eq!(
            hero_from_hero_entry("soundevents/hero/_shared.vsndevts_c"),
            None
        );
    }

    #[test]
    fn classifies_weapon_movement_melee() {
        let w = classify_hero_event("Haze.Wpn.Fire.Main");
        assert_eq!(w.category, HeroSoundCategory::Weapon);
        assert_eq!(w.label, "Fire Main");
        assert_eq!(w.ability, None);

        // Bare weapon token, no detail segments.
        let z = classify_hero_event("Seven.ZoomIn");
        assert_eq!(z.category, HeroSoundCategory::Weapon);
        assert_eq!(z.label, "Zoom In");

        assert_eq!(
            classify_hero_event("Haze.Footstep").category,
            HeroSoundCategory::Movement
        );
        assert_eq!(
            classify_hero_event("Abrams.Melee.Swing.Charged").category,
            HeroSoundCategory::Melee
        );
        assert_eq!(
            classify_hero_event("Atlas.Progession.Page.Win.VO").category,
            HeroSoundCategory::Other
        );
    }

    #[test]
    fn classifies_named_and_slotted_abilities() {
        // Named ability: the grouping segment is the ability.
        let n = classify_hero_event("Haze.Finesse.Dagger.Cast");
        assert_eq!(n.category, HeroSoundCategory::Ability);
        assert_eq!(n.ability.as_deref(), Some("Finesse"));
        assert_eq!(n.label, "Dagger Cast");
        assert_eq!(n.slot, None);

        // CamelCase ability name splits into words.
        assert_eq!(
            classify_hero_event("VampireBat.LoveBites.Buildup")
                .ability
                .as_deref(),
            Some("Love Bites")
        );

        // A1..A4 slot token is peeled; the ability name follows.
        let s = classify_hero_event("Abrams.A1.SiphonLife.Cast");
        assert_eq!(s.category, HeroSoundCategory::Ability);
        assert_eq!(s.slot, Some(1));
        assert_eq!(s.ability.as_deref(), Some("Siphon Life"));
        assert_eq!(s.label, "Cast");

        // Leading literal `Ability.` prefix is skipped.
        let a = classify_hero_event("Ability.Abrams.Charge.Step");
        assert_eq!(a.category, HeroSoundCategory::Ability);
        assert_eq!(a.ability.as_deref(), Some("Charge"));
    }

    #[test]
    fn slot_recovered_from_clip_path() {
        assert_eq!(
            slot_from_vsnd(&["sounds/abilities/vampirebat/a2_batblink/x_01.vsnd".to_owned()]),
            Some(2)
        );
        assert_eq!(
            slot_from_vsnd(&["sounds/abilities/h/a4/ult_01.vsnd".to_owned()]),
            Some(4)
        );
        // No aN token anywhere.
        assert_eq!(
            slot_from_vsnd(&["sounds/weapons/abrams/fire_01.vsnd".to_owned()]),
            None
        );
    }
}
