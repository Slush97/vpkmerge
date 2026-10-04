//! The tools: typed params and results over `vpkmerge-core`.
//!
//! Everything here is synchronous and transport-free so it can be tested
//! directly; `server.rs` is the MCP glue. Read tools only read the game pak;
//! build tools write to the staging dir and return the path. Nothing here
//! touches the game's `addons` folder.

// Doc comments on the param and result types are the JSON-schema descriptions the
// model reads, so they are written as prose for it, not as rustdoc. The bools are
// the wire shape.
#![allow(clippy::doc_markdown, clippy::struct_excessive_bools)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use anyhow::{bail, ensure, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use vpkmerge_core::{
    build_hero_sound_index, build_shared_sound_index, compiled_clip_entry, pinned_hero_codenames,
    BuildFingerprint, CatalogCache, CollisionPolicy, HeroSound, HeroSoundCategory, MergeOptions,
    PoolPolicy, PrismTuning, Recolor, SharedSound, TextureCategory, TextureEntry, ThumbnailOutcome,
    VdataStatus, VoiceLine,
};

use crate::config::Config;
use crate::docs::DocIndex;
use crate::heroes::{Hero, Roster};

const DEFAULT_LIMIT: usize = 30;
const MAX_LIMIT: usize = 200;
const THUMBNAIL_EDGE: u32 = 256;

pub struct Engine {
    config: Config,
    loaded: Mutex<Loaded>,
    docs: OnceLock<DocIndex>,
}

/// Indexes built from the pak, dropped together when a game update changes it.
#[derive(Default)]
struct Loaded {
    fingerprint: Option<BuildFingerprint>,
    roster: Option<Arc<Roster>>,
    entries: Option<Arc<HashSet<String>>>,
    hero_sounds: Option<Arc<Vec<HeroSound>>>,
    shared_sounds: Option<Arc<Vec<SharedSound>>>,
    voicelines: Option<Arc<Vec<VoiceLine>>>,
    textures: Option<Arc<Vec<TextureEntry>>>,
}

impl Engine {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            config,
            loaded: Mutex::new(Loaded::default()),
            docs: OnceLock::new(),
        }
    }

    fn cached<T>(
        &self,
        slot: fn(&mut Loaded) -> &mut Option<Arc<T>>,
        build: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<Arc<T>> {
        let pak = self.config.pak()?;
        let fingerprint = BuildFingerprint::for_vpk(pak)?;
        let mut loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        if loaded.fingerprint.as_ref() != Some(&fingerprint) {
            *loaded = Loaded {
                fingerprint: Some(fingerprint),
                ..Loaded::default()
            };
        }
        let slot = slot(&mut loaded);
        if let Some(value) = slot {
            return Ok(Arc::clone(value));
        }
        let value = Arc::new(build(pak)?);
        *slot = Some(Arc::clone(&value));
        Ok(value)
    }

    fn roster(&self) -> Result<Arc<Roster>> {
        self.cached(|l| &mut l.roster, Roster::load)
    }

    /// Every entry path in the pak, to tell shipped clips from dangling references.
    fn entries(&self) -> Result<Arc<HashSet<String>>> {
        self.cached(
            |l| &mut l.entries,
            |pak| {
                Ok(vpkmerge_core::inspect(pak)?
                    .file_paths
                    .into_iter()
                    .collect())
            },
        )
    }

    fn hero_sounds(&self) -> Result<Arc<Vec<HeroSound>>> {
        self.cached(|l| &mut l.hero_sounds, |pak| build_hero_sound_index(pak))
    }

    fn shared_sounds(&self) -> Result<Arc<Vec<SharedSound>>> {
        self.cached(
            |l| &mut l.shared_sounds,
            |pak| build_shared_sound_index(pak),
        )
    }

    fn voicelines(&self) -> Result<Arc<Vec<VoiceLine>>> {
        let cache = CatalogCache::new(self.config.cache_dir());
        self.cached(|l| &mut l.voicelines, |pak| cache.voicelines(pak))
    }

    fn textures(&self) -> Result<Arc<Vec<TextureEntry>>> {
        let cache = CatalogCache::new(self.config.cache_dir());
        self.cached(|l| &mut l.textures, |pak| cache.textures(pak))
    }

    fn docs(&self) -> &DocIndex {
        self.docs
            .get_or_init(|| DocIndex::load(&self.config.docs_dirs))
    }

    fn staged(&self, name: &str) -> PathBuf {
        self.config.staging.join(format!("{name}_dir.vpk"))
    }
}

// ---------------------------------------------------------------- game_status

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameStatus {
    /// The base game pak every tool reads. Absent when Deadlock was not found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pak: Option<String>,
    /// Why the game pak is unavailable, and how to fix it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// Where built mods are written.
    pub staging_dir: String,
    /// The game's addons folder, where installed mods live.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addons_dir: Option<String>,
    /// Installed addon slots (`pakNN_dir.vpk`).
    pub addons: Vec<AddonSlot>,
    /// The lowest unused slot filename, where a new mod would install.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_free_slot: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddonSlot {
    pub file: String,
    pub size_bytes: u64,
}

impl Engine {
    #[must_use]
    pub fn game_status(&self) -> GameStatus {
        let pak = self.config.pak();
        let addons_dir = self.config.addons_dir();
        let mut addons: Vec<AddonSlot> = addons_dir
            .as_deref()
            .and_then(|dir| std::fs::read_dir(dir).ok())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let file = entry.file_name().to_string_lossy().into_owned();
                addon_slot(&file)?;
                Some(AddonSlot {
                    file,
                    size_bytes: entry.metadata().map_or(0, |m| m.len()),
                })
            })
            .collect();
        addons.sort_by(|a, b| a.file.cmp(&b.file));
        let used: HashSet<u32> = addons.iter().filter_map(|a| addon_slot(&a.file)).collect();
        GameStatus {
            pak: pak.as_ref().ok().map(|p| p.display().to_string()),
            problem: pak.as_ref().err().map(ToString::to_string),
            staging_dir: self.config.staging.display().to_string(),
            next_free_slot: addons_dir.as_ref().and_then(|_| {
                (1..100)
                    .find(|n| !used.contains(n))
                    .map(|n| format!("pak{n:02}_dir.vpk"))
            }),
            addons_dir: addons_dir.map(|d| d.display().to_string()),
            addons,
        }
    }
}

/// The slot number of a mountable addon filename (`pak07_dir.vpk` -> 7).
fn addon_slot(file: &str) -> Option<u32> {
    let digits = file.strip_prefix("pak")?.strip_suffix("_dir.vpk")?;
    (digits.len() == 2).then(|| digits.parse().ok())?
}

// ---------------------------------------------------------------- list_heroes

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListHeroesParams {
    /// Include in-development and disabled heroes. Default: playable heroes only.
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeroList {
    pub heroes: Vec<HeroRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeroRow {
    /// In-game display name.
    pub name: String,
    /// Internal roster codename.
    pub codename: String,
    pub selectable: bool,
    pub in_development: bool,
    pub disabled: bool,
    /// Whether `recolor_hero_vfx` supports this hero.
    pub vfx_recolor: bool,
}

impl Engine {
    pub fn list_heroes(&self, p: &ListHeroesParams) -> Result<HeroList> {
        let roster = self.roster()?;
        let heroes = roster
            .heroes()
            .iter()
            .filter(|h| p.all || (h.info.selectable && !h.info.disabled))
            .map(|h| HeroRow {
                name: h.info.name.clone(),
                codename: h.info.codename.clone(),
                selectable: h.info.selectable,
                in_development: h.info.in_development,
                disabled: h.info.disabled,
                vfx_recolor: recolor_codename(h).is_some(),
            })
            .collect();
        Ok(HeroList { heroes })
    }
}

/// The pinned recolor recipe this hero maps to, if one exists.
fn recolor_codename(hero: &Hero) -> Option<&'static str> {
    pinned_hero_codenames()
        .iter()
        .copied()
        .find(|c| hero.owns(c))
}

// -------------------------------------------------------------- browse_sounds

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SoundKind {
    /// One hero's own weapon, ability and movement sounds.
    Gameplay,
    /// Sounds every hero shares and world sounds: melee, parry, footsteps, items,
    /// NPCs, UI, music.
    Shared,
    /// Hero voice lines (barks, pings, kill lines).
    Voiceline,
    #[default]
    Any,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowseSoundsParams {
    /// Hero display name or codename. Omit to search every hero and shared sounds.
    pub hero: Option<String>,
    /// Words that must all appear in the sound's label or event name, e.g. "melee swing".
    pub search: Option<String>,
    /// Which sounds to search. Default: any (gameplay first, then shared, then voice lines).
    #[serde(default)]
    pub kind: SoundKind,
    /// Maximum rows to return. Default 30, max 200.
    pub limit: Option<usize>,
    /// Also list every clip entry of each sound (needed for swap_sound_clip).
    #[serde(default)]
    pub include_clips: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SoundList {
    /// How many sounds matched.
    pub total: usize,
    /// Present when the list is cut short or a different search would help.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub sounds: Vec<SoundRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SoundRow {
    /// The exact event name to pass to swap_hero_sound.
    pub event: String,
    /// "gameplay", "shared" or "voiceline".
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hero: Option<String>,
    pub label: String,
    /// Gameplay sounds only: weapon, ability, movement, melee or other.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ability: Option<String>,
    /// How many clips the event picks from at random.
    pub clip_count: usize,
    /// True when the event is a randomizer pool (more than one clip).
    pub is_pool: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    /// The soundevents file that defines the event.
    pub soundevents_entry: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<String>>,
}

/// Display names for index hero keys, resolved once per distinct key.
struct HeroNames<'a> {
    roster: &'a Roster,
    seen: HashMap<String, String>,
}

impl<'a> HeroNames<'a> {
    fn new(roster: &'a Roster) -> Self {
        Self {
            roster,
            seen: HashMap::new(),
        }
    }

    fn display(&mut self, key: &str) -> String {
        if let Some(name) = self.seen.get(key) {
            return name.clone();
        }
        let name = self
            .roster
            .owner(key)
            .map_or_else(|| key.to_owned(), |h| h.info.name.clone());
        self.seen.insert(key.to_owned(), name.clone());
        name
    }
}

fn search_terms(search: Option<&str>) -> Vec<String> {
    search
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_lowercase)
        .collect()
}

fn matches(terms: &[String], fields: &[&str]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let haystack = fields.join(" ").to_lowercase();
    terms.iter().all(|t| haystack.contains(t))
}

fn limit(requested: Option<usize>) -> usize {
    requested.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn truncation_note(shown: usize, total: usize) -> Option<String> {
    (shown < total)
        .then(|| format!("Showing {shown} of {total}. Narrow with search or hero, or raise limit."))
}

fn clip_entries(vsnd: &[String]) -> Vec<String> {
    vsnd.iter().map(|c| compiled_clip_entry(c)).collect()
}

fn category_label(category: HeroSoundCategory) -> &'static str {
    match category {
        HeroSoundCategory::Weapon => "weapon",
        HeroSoundCategory::Ability => "ability",
        HeroSoundCategory::Movement => "movement",
        HeroSoundCategory::Melee => "melee",
        HeroSoundCategory::Other => "other",
    }
}

/// One index row, whichever index it came from.
struct Candidate<'a> {
    event: &'a str,
    kind: &'static str,
    hero_key: Option<&'a str>,
    label: &'a str,
    category: Option<&'static str>,
    ability: Option<&'a str>,
    vsnd: &'a [String],
    duration: Option<f64>,
    source: &'a str,
}

struct SoundSearch<'a> {
    hero: Option<&'a Hero>,
    terms: Vec<String>,
    shipped: &'a HashSet<String>,
    names: HeroNames<'a>,
    limit: usize,
    include_clips: bool,
    total: usize,
    sounds: Vec<SoundRow>,
}

impl SoundSearch<'_> {
    fn consider(&mut self, c: &Candidate) {
        if self
            .hero
            .is_some_and(|h| !c.hero_key.is_some_and(|k| h.owns(k)))
            || !matches(
                &self.terms,
                &[c.label, c.event, c.ability.unwrap_or_default()],
            )
        {
            return;
        }
        // An event whose clips are all missing from the pak (unreleased content)
        // has no donor to mint from, so it is not a swap target.
        let clips = clip_entries(c.vsnd);
        if !clips.iter().any(|clip| self.shipped.contains(clip)) {
            return;
        }
        self.total += 1;
        if self.sounds.len() < self.limit {
            self.sounds.push(SoundRow {
                event: c.event.to_owned(),
                kind: c.kind,
                hero: c.hero_key.map(|k| self.names.display(k)),
                label: c.label.to_owned(),
                category: c.category,
                ability: c.ability.map(str::to_owned),
                clip_count: clips.len(),
                is_pool: clips.len() > 1,
                duration_seconds: c.duration,
                soundevents_entry: c.source.to_owned(),
                clips: self.include_clips.then_some(clips),
            });
        }
    }
}

impl Engine {
    pub fn browse_sounds(&self, p: &BrowseSoundsParams) -> Result<SoundList> {
        let roster = self.roster()?;
        let shipped = self.entries()?;
        let mut search = SoundSearch {
            hero: p.hero.as_deref().map(|h| roster.resolve(h)).transpose()?,
            terms: search_terms(p.search.as_deref()),
            shipped: &shipped,
            names: HeroNames::new(&roster),
            limit: limit(p.limit),
            include_clips: p.include_clips,
            total: 0,
            sounds: Vec::new(),
        };
        let wants = |kind| p.kind == SoundKind::Any || p.kind == kind;

        if wants(SoundKind::Gameplay) {
            for s in self.hero_sounds()?.iter() {
                search.consider(&Candidate {
                    event: &s.event,
                    kind: "gameplay",
                    hero_key: Some(&s.hero),
                    label: &s.label,
                    category: Some(category_label(s.category)),
                    ability: s.ability.as_deref(),
                    vsnd: &s.vsnd,
                    duration: s.duration,
                    source: &s.source,
                });
            }
        }
        if wants(SoundKind::Shared) && search.hero.is_none() {
            for s in self.shared_sounds()?.iter() {
                search.consider(&Candidate {
                    event: &s.event,
                    kind: "shared",
                    hero_key: None,
                    label: &s.label,
                    category: None,
                    ability: None,
                    vsnd: &s.vsnd,
                    duration: s.duration,
                    source: &s.source,
                });
            }
        }
        if wants(SoundKind::Voiceline) {
            for l in self.voicelines()?.iter() {
                search.consider(&Candidate {
                    event: &l.event,
                    kind: "voiceline",
                    hero_key: l.hero.as_deref(),
                    label: &l.label,
                    category: None,
                    ability: None,
                    vsnd: &l.vsnd,
                    duration: l.duration,
                    source: &l.source,
                });
            }
        }

        let note = if search.total == 0 && search.hero.is_some() {
            Some(
                "Nothing matched for this hero. Sounds every hero shares (melee, parry, \
                 footsteps) and world sounds are not per hero: search again without hero."
                    .to_owned(),
            )
        } else {
            truncation_note(search.sounds.len(), search.total)
        };
        Ok(SoundList {
            total: search.total,
            note,
            sounds: search.sounds,
        })
    }
}

// ------------------------------------------------------------ browse_textures

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowseTexturesParams {
    /// One of: ability-icon, item-icon, hero-image (portraits and cards),
    /// hero-model (skin textures), ability-vfx, other.
    pub category: Option<String>,
    /// Hero display name or codename.
    pub hero: Option<String>,
    /// Words that must all appear in the texture's label or path.
    pub search: Option<String>,
    /// Maximum rows to return. Default 30, max 200.
    pub limit: Option<usize>,
    /// Also write a PNG thumbnail of each returned texture and return its path.
    #[serde(default)]
    pub thumbnails: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextureList {
    /// How many textures matched.
    pub total: usize,
    /// Present when `textures` holds fewer rows than `total`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub textures: Vec<TextureRow>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextureRow {
    /// The exact entry to pass to replace_textures.
    pub path: String,
    pub category: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hero: Option<String>,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_path: Option<String>,
}

impl Engine {
    pub fn browse_textures(&self, p: &BrowseTexturesParams) -> Result<TextureList> {
        let category = p
            .category
            .as_deref()
            .map(|c| {
                TextureCategory::from_id(c).with_context(|| {
                    format!(
                        "unknown category {c:?}. Use one of: ability-icon, item-icon, \
                         hero-image, hero-model, ability-vfx, other"
                    )
                })
            })
            .transpose()?;
        let roster = self.roster()?;
        let hero = p.hero.as_deref().map(|h| roster.resolve(h)).transpose()?;
        let terms = search_terms(p.search.as_deref());
        let limit = limit(p.limit);

        let index = self.textures()?;
        let matching: Vec<&TextureEntry> = index
            .iter()
            .filter(|e| {
                category.is_none_or(|c| e.category == c)
                    && hero.is_none_or(|h| e.hero.as_deref().is_some_and(|k| h.owns(k)))
                    && matches(&terms, &[&e.label, &e.path])
            })
            .collect();
        let total = matching.len();
        let shown: Vec<TextureEntry> = matching.into_iter().take(limit).cloned().collect();

        let mut thumbnails: HashMap<String, String> = HashMap::new();
        if p.thumbnails {
            let dir = self.config.staging.join("thumbnails");
            let outcomes = vpkmerge_core::cache_texture_thumbnails(
                self.config.pak()?,
                &shown,
                &dir,
                THUMBNAIL_EDGE,
            )?;
            for outcome in outcomes {
                if let ThumbnailOutcome::Cached(c) = outcome {
                    thumbnails.insert(c.entry, dir.join(c.file).display().to_string());
                }
            }
        }

        let mut names = HeroNames::new(&roster);
        let textures: Vec<TextureRow> = shown
            .into_iter()
            .map(|e| TextureRow {
                thumbnail_path: thumbnails.remove(&e.path),
                category: e.category.id(),
                hero: e.hero.as_deref().map(|k| names.display(k)),
                label: e.label,
                path: e.path,
            })
            .collect();
        Ok(TextureList {
            total,
            note: truncation_note(textures.len(), total),
            textures,
        })
    }
}

// ---------------------------------------------------------- make_sound_louder

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LouderParams {
    /// Absolute path of the .mp3 to change.
    pub audio_path: String,
    /// Decibels to add: positive is louder, negative quieter. +6 is a clear boost.
    pub gain_db: f64,
    /// Where to write the result: a path that does not exist yet. Default: next to
    /// the input, original untouched.
    pub out_path: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LouderSound {
    pub out_path: String,
    /// The gain actually applied: the request rounded to the MP3 gain step (about 1.5 dB).
    pub applied_db: f64,
}

impl Engine {
    pub fn make_sound_louder(p: &LouderParams) -> Result<LouderSound> {
        let input = user_path(&p.audio_path);
        let raw = read_input(&input, "audio")?;
        let out = if let Some(out) = &p.out_path {
            new_output(out)?
        } else {
            let stem = input
                .file_stem()
                .map_or_else(|| "audio".to_owned(), |s| s.to_string_lossy().into_owned());
            input.with_file_name(format!("{stem}_{:+.1}dB.mp3", p.gain_db))
        };
        let louder = vpkmerge_core::apply_mp3_gain(&raw, p.gain_db)
            .with_context(|| format!("{} is not an MP3", input.display()))?;
        write_output(&out, &louder)?;
        let step = vpkmerge_core::mp3::GAIN_STEP_DB;
        Ok(LouderSound {
            out_path: out.display().to_string(),
            applied_db: (p.gain_db / step).round() * step,
        })
    }
}

// ------------------------------------------------------------ sound swapping

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PoolMode {
    /// Put the audio in every clip of the pool, so it always plays.
    #[default]
    All,
    /// Rewrite the event to play one clip only. Smaller output.
    Collapse,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LoopMode {
    /// Loop only if the sound being replaced loops.
    #[default]
    Auto,
    On,
    Off,
}

impl LoopMode {
    fn looped_override(self) -> Option<bool> {
        match self {
            Self::Auto => None,
            Self::On => Some(true),
            Self::Off => Some(false),
        }
    }
}

/// Optional edits applied to the user's MP3 before it is minted.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioEdit {
    /// Drop audio before this many milliseconds.
    pub trim_start_ms: Option<u32>,
    /// Drop audio after this many milliseconds.
    pub trim_end_ms: Option<u32>,
    /// Decibels to add (positive is louder), in steps of about 1.5 dB.
    pub gain_db: Option<f64>,
}

impl AudioEdit {
    fn apply(&self, raw: &[u8]) -> Result<Vec<u8>> {
        let trim = match (self.trim_start_ms, self.trim_end_ms) {
            (None, None) => None,
            (start, end) => Some((start.unwrap_or(0), end.unwrap_or(u32::MAX))),
        };
        vpkmerge_core::prepare_swap_audio(raw, trim, self.gain_db)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SwapHeroSoundParams {
    /// The exact event name from browse_sounds, e.g. "Seven.Wpn.Fire".
    pub event: String,
    /// Absolute path of the replacement .mp3.
    pub audio_path: String,
    /// Hero display name or codename. Only needed if the event name exists for several heroes.
    pub hero: Option<String>,
    /// The soundevents file from browse_sounds. Only needed to pick between duplicates.
    pub soundevents_entry: Option<String>,
    /// How to treat a randomizer pool. Default: all.
    #[serde(default)]
    pub pool: PoolMode,
    /// Change this hero only, even when the sound's clips are also played by other
    /// heroes' events. Implies pool: collapse.
    #[serde(default)]
    pub hero_only: bool,
    /// Whether the new sound loops. Default: auto.
    #[serde(default, rename = "loop")]
    pub loop_mode: LoopMode,
    #[serde(flatten)]
    pub edit: AudioEdit,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeroSoundSwap {
    /// The built addon, in staging. Not installed.
    pub staged_vpk: String,
    pub event: String,
    pub soundevents_entry: String,
    /// "all", "collapse", or "collapse (hero only)".
    pub pool: &'static str,
    /// Clips the event's pool holds.
    pub pool_size: usize,
    /// Clips that now carry the new audio.
    pub clips_minted: usize,
    pub looped: bool,
    /// Pool clips that live in another pak and keep their original sound.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

struct EventTarget {
    event: String,
    source: String,
    vsnd: Vec<String>,
    /// Defined outside any one hero's file, so every hero plays it.
    shared: bool,
}

impl EventTarget {
    fn new(event: &str, source: &str, vsnd: &[String], shared: bool) -> Self {
        Self {
            event: event.to_owned(),
            source: source.to_owned(),
            vsnd: vsnd.to_vec(),
            shared,
        }
    }
}

impl Engine {
    /// Find the soundevents file that defines `event`: a hero's own sounds first,
    /// then shared sounds, then voice lines.
    fn locate_event(&self, event: &str, hero: Option<&Hero>) -> Result<EventTarget> {
        let mut found: Vec<EventTarget> = self
            .hero_sounds()?
            .iter()
            .filter(|s| s.event.eq_ignore_ascii_case(event) && hero.is_none_or(|h| h.owns(&s.hero)))
            .map(|s| EventTarget::new(&s.event, &s.source, &s.vsnd, false))
            .collect();
        if found.is_empty() {
            found = self
                .shared_sounds()?
                .iter()
                .filter(|s| s.event.eq_ignore_ascii_case(event))
                .map(|s| EventTarget::new(&s.event, &s.source, &s.vsnd, true))
                .collect();
        }
        if found.is_empty() {
            found = self
                .voicelines()?
                .iter()
                .filter(|l| {
                    l.event.eq_ignore_ascii_case(event)
                        && hero.is_none_or(|h| l.hero.as_deref().is_some_and(|k| h.owns(k)))
                })
                .map(|l| EventTarget::new(&l.event, &l.source, &l.vsnd, false))
                .collect();
        }
        if found.len() > 1 {
            bail!(
                "event {event:?} is defined in several files: {}. Pass soundeventsEntry (or \
                 hero) to pick one.",
                found
                    .iter()
                    .map(|t| t.source.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        found.pop().with_context(|| {
            format!(
                "no sound event named {event:?}{}. Call browse_sounds to find the exact \
                 event name.",
                hero.map_or_else(String::new, |h| format!(" for {}", h.info.name))
            )
        })
    }

    /// How many other heroes' soundevents files play any of `target`'s clips.
    fn sharing_files(&self, target: &EventTarget) -> Result<usize> {
        let clips: HashSet<&str> = target.vsnd.iter().map(String::as_str).collect();
        let sounds = self.hero_sounds()?;
        let sources: HashSet<&str> = sounds
            .iter()
            .filter(|s| s.source != target.source)
            .filter(|s| s.vsnd.iter().any(|c| clips.contains(c.as_str())))
            .map(|s| s.source.as_str())
            .collect();
        Ok(sources.len())
    }

    pub fn swap_hero_sound(&self, p: &SwapHeroSoundParams) -> Result<HeroSoundSwap> {
        let pak = self.config.pak()?;
        let target = if let Some(entry) = &p.soundevents_entry {
            EventTarget::new(&p.event, entry, &[], false)
        } else {
            let roster = self.roster()?;
            let hero = p.hero.as_deref().map(|h| roster.resolve(h)).transpose()?;
            self.locate_event(&p.event, hero)?
        };
        ensure!(
            !(target.shared && p.hero_only),
            "{} is a shared sound that every hero plays from {}; this game build has no \
             per-hero version, so heroOnly cannot apply. Swap it without heroOnly to change \
             it for all heroes.",
            target.event,
            target.source
        );

        let audio_path = user_path(&p.audio_path);
        let audio = p.edit.apply(&read_input(&audio_path, "audio")?)?;

        let slug = slug(&target.event);
        let (policy, pool, collapse_target) = if p.hero_only {
            let clip = format!("sounds/vpkmerge/{slug}.vsnd_c");
            (PoolPolicy::Collapse, "collapse (hero only)", Some(clip))
        } else if p.pool == PoolMode::Collapse {
            (PoolPolicy::Collapse, "collapse", None)
        } else {
            (PoolPolicy::ReplaceAll, "all", None)
        };

        let out = self.staged(&format!("sound-{slug}"));
        let swap = vpkmerge_core::swap_event_to_addon(
            pak,
            &target.source,
            &target.event,
            &audio,
            p.loop_mode.looped_override(),
            policy,
            collapse_target.as_deref(),
            &out,
        )?;

        let note = if target.shared {
            Some("This is a shared sound: the mod changes it for every hero.".to_owned())
        } else if p.hero_only {
            None
        } else {
            let sharing = self.sharing_files(&target)?;
            (sharing > 0).then(|| {
                format!(
                    "These clips are shared: {sharing} other hero soundevents file(s) play \
                     them, so this mod changes the sound for those heroes too. Rebuild with \
                     heroOnly: true to change only this hero."
                )
            })
        };
        Ok(HeroSoundSwap {
            staged_vpk: out.display().to_string(),
            event: target.event,
            soundevents_entry: target.source,
            pool,
            pool_size: swap.pool_size,
            clips_minted: swap.clips.len(),
            looped: swap.looped,
            skipped: swap.skipped,
            note,
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SwapSoundClipParams {
    /// One clip entry (.vsnd_c) from browse_sounds with includeClips.
    pub clip_entry: String,
    /// Absolute path of the replacement .mp3.
    pub audio_path: String,
    /// Whether the new sound loops. Default: auto.
    #[serde(default, rename = "loop")]
    pub loop_mode: LoopMode,
    #[serde(flatten)]
    pub edit: AudioEdit,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SoundClipSwap {
    /// The built addon, in staging. Not installed.
    pub staged_vpk: String,
    pub clip_entry: String,
    pub looped: bool,
}

impl Engine {
    pub fn swap_sound_clip(&self, p: &SwapSoundClipParams) -> Result<SoundClipSwap> {
        let pak = self.config.pak()?;
        let audio_path = user_path(&p.audio_path);
        let audio = p.edit.apply(&read_input(&audio_path, "audio")?)?;
        let stem = Path::new(&p.clip_entry)
            .file_stem()
            .map_or_else(|| "clip".to_owned(), |s| s.to_string_lossy().into_owned());
        let out = self.staged(&format!("clip-{}", slug(&stem)));
        let swap = vpkmerge_core::swap_clip_to_addon(
            pak,
            &p.clip_entry,
            &audio,
            p.loop_mode.looped_override(),
            None,
            &out,
        )
        .context("Call browse_sounds with includeClips to find valid clip entries")?;
        Ok(SoundClipSwap {
            staged_vpk: out.display().to_string(),
            clip_entry: swap.entry,
            looped: swap.looped,
        })
    }
}

// ---------------------------------------------------------- list_hero_textures

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListHeroTexturesParams {
    /// Hero display name or codename.
    pub hero: String,
    /// Words that must all appear in the material path, e.g. "dress" or "gun".
    pub material: Option<String>,
    /// Absolute path of a mod VPK whose model or materials override the game's.
    pub vpk: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeroTextureList {
    pub hero: String,
    /// The body model the game renders for this hero (from scripts/heroes.vdata_c).
    pub model: String,
    /// Largest first, by vertex count.
    pub materials: Vec<MaterialRow>,
    /// What each texture slot listed here holds in Deadlock's pbr.vfx.
    pub slots: BTreeMap<String, &'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MaterialRow {
    /// Compiled material entry.
    pub material: String,
    /// "body", "weapon" or "shared".
    pub usage: &'static str,
    pub meshes: Vec<String>,
    /// Triangles drawn with this material.
    pub triangles: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shader: Option<String>,
    /// Shader feature flags set on the material.
    pub flags: Vec<String>,
    /// True when per-vertex colors multiply the albedo, so part of the visible color
    /// is in the mesh, not the texture.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub vertex_color: bool,
    /// Texture-coordinate params with a non-identity value: the texture is
    /// scaled, offset or scrolled on the mesh.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub uv_params: BTreeMap<String, [f32; 4]>,
    pub textures: Vec<TextureSlotRow>,
    /// Slots bound to shared engine placeholders (materials/default/). Never
    /// replace those: every material in the game uses them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub engine_defaults: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextureSlotRow {
    pub slot: String,
    /// Texture entry for map_texture_regions and replace_textures.
    pub path: String,
    /// "WIDTHxHEIGHT".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// "#rrggbbaa" when every texel holds this one value: a placeholder that
    /// replace_textures can only recolor, not paint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uniform: Option<String>,
    /// "min-max" of the alpha channel, when not fully opaque.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha: Option<String>,
    /// DXT5 storing YCoCg: readable, but replace_textures cannot re-encode it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ycocg: bool,
    /// Other materials of this hero that sample the same texture: a replacement
    /// changes them too.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shared_with: Vec<String>,
    /// The entry is missing from the game files.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
}

/// Channel layouts of the pbr.vfx slots Deadlock hero materials bind.
fn slot_meaning(slot: &str) -> Option<&'static str> {
    Some(match slot {
        "g_tColor" => {
            "RGB albedo (sRGB). Alpha is opacity only with F_TRANSLUCENT or F_ALPHA_TEST; \
             otherwise it is a data channel, so keep it."
        }
        "g_tNormalRoughness" => {
            "R,G tangent-space normal X,Y (128,128 is flat); B roughness (0 glossy, 255 \
             matte); A unused. Linear, not sRGB."
        }
        "g_tAmbientOcclusion" => "Ambient occlusion, grayscale (white is unoccluded).",
        "g_tSelfIllumMask" => "Self-illumination mask, grayscale (white glows).",
        "g_tTintMaskRimLightMask" => "R tint mask, G rim-light mask.",
        "g_tNprOutlineMask" => "Where the toon ink outline draws (white draws).",
        "g_tNprTransmissiveColor" => "Toon transmission color: light bleeding through.",
        "g_tMetalness" => "Metalness, grayscale.",
        _ => return None,
    })
}

impl Engine {
    pub fn list_hero_textures(&self, p: &ListHeroTexturesParams) -> Result<HeroTextureList> {
        let roster = self.roster()?;
        let hero = roster.resolve(&p.hero)?;
        let (first, base) = self.preview_stack(p.vpk.as_deref())?;
        let found = vpkmerge_core::hero_textures(&first, base.as_deref(), &hero.info.codename)?;
        let terms = search_terms(p.material.as_deref());

        let mut slots = BTreeMap::new();
        let mut uniform_albedo = false;
        let mut materials = Vec::new();
        for m in found.materials {
            if !matches(&terms, &[&m.material]) {
                continue;
            }
            let mut engine_defaults = Vec::new();
            let mut textures = Vec::new();
            for t in m.textures {
                if t.engine_default {
                    engine_defaults.push(t.slot);
                    continue;
                }
                if let Some(meaning) = slot_meaning(&t.slot) {
                    slots.insert(t.slot.clone(), meaning);
                }
                let f = t.facts.as_ref();
                let uniform = f
                    .and_then(|f| f.uniform)
                    .map(|[r, g, b, a]| format!("#{r:02x}{g:02x}{b:02x}{a:02x}"));
                uniform_albedo |= t.slot == "g_tColor" && uniform.is_some();
                textures.push(TextureSlotRow {
                    path: t.entry,
                    size: f.map(|f| format!("{}x{}", f.width, f.height)),
                    format: f.map(|f| f.format.clone()),
                    alpha: f
                        .filter(|f| uniform.is_none() && f.alpha != (255, 255))
                        .map(|f| format!("{}-{}", f.alpha.0, f.alpha.1)),
                    uniform,
                    ycocg: f.is_some_and(|f| f.ycocg),
                    shared_with: t.shared_with,
                    missing: f.is_none(),
                    slot: t.slot,
                });
            }
            materials.push(MaterialRow {
                usage: match (m.body, m.weapon) {
                    (true, true) => "shared",
                    (false, true) => "weapon",
                    _ => "body",
                },
                material: m.material,
                meshes: m.meshes,
                triangles: m.triangles,
                shader: m.shader,
                flags: m.flags,
                vertex_color: m.vertex_color,
                uv_params: m.uv_params.into_iter().collect(),
                textures,
                engine_defaults,
            });
        }
        ensure!(
            !materials.is_empty(),
            "no material of {} matches {:?}. Call again without material to list them all.",
            found.model_entry,
            p.material.as_deref().unwrap_or_default()
        );

        let mut notes = Vec::new();
        if uniform_albedo {
            notes.push(
                "A uniform g_tColor is a 1x1 or 4x4 placeholder: that material's color comes \
                 from vertex colors or tint params, and replacing the texture only changes \
                 its one color."
                    .to_owned(),
            );
        }
        Ok(HeroTextureList {
            hero: hero.info.name.clone(),
            model: found.model_entry,
            materials,
            slots,
            notes,
        })
    }
}

// --------------------------------------------------------- map_texture_regions

const DEFAULT_REGION_ROWS: usize = 25;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RegionMode {
    /// Body areas: each triangle goes to the skeleton bone that skins it most
    /// (head, arm_upper_L, ...), small bones folded into their parents. A region
    /// can be several separate patches on the texture.
    #[default]
    Bone,
    /// Connected UV islands: exact texture patches, usually hundreds per texture.
    Island,
    /// Mesh parts (body, gun, ...).
    Part,
}

impl RegionMode {
    fn segment_by(self) -> vpkmerge_core::SegmentBy {
        match self {
            Self::Bone => vpkmerge_core::SegmentBy::Bone,
            Self::Island => vpkmerge_core::SegmentBy::Island,
            Self::Part => vpkmerge_core::SegmentBy::Part,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MapTextureRegionsParams {
    /// Hero display name or codename.
    pub hero: String,
    /// A texture `path` from list_hero_textures.
    pub texture: String,
    /// How to split the texture into regions. Default: bone.
    #[serde(default)]
    pub by: RegionMode,
    /// Region ids to bake into a mask PNG at the texture's full size, from an
    /// earlier call with the same hero, texture and `by`.
    #[serde(default)]
    pub select: Vec<usize>,
    /// Texels the mask grows into unused texture space so mipmaps keep the new
    /// paint at seams. Default 4.
    pub padding: Option<u32>,
    /// Maximum region rows to return and label on the overlay. Default 25, max 200.
    pub limit: Option<usize>,
    /// Absolute path of a mod VPK whose model overrides the game's.
    pub vpk: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextureRegions {
    pub texture: String,
    /// "WIDTHxHEIGHT".
    pub size: String,
    pub format: String,
    /// The materials and slots of this hero that sample the texture.
    pub sampled_by: Vec<String>,
    /// Percent of the texture any region samples. The rest never shows in game.
    pub used_percent: f32,
    /// "bone", "island" or "part".
    pub by: &'static str,
    pub region_count: usize,
    /// Largest first. Every id up to regionCount - 1 is valid for select, listed or not.
    pub regions: Vec<RegionRow>,
    /// The texture's RGB at full size: the image to paint on.
    pub texture_png: String,
    /// The texture's alpha as grayscale, when it is not fully opaque.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha_png: Option<String>,
    /// Up to 1024 px: each region tinted its own color and outlined, with its id
    /// drawn in its largest patch. Unused texture space is darkened.
    pub overlay_png: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<MaskRow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegionRow {
    pub id: usize,
    pub label: String,
    /// Percent of the texture's texels the region samples.
    pub coverage_percent: f32,
    /// Separate patches on the texture.
    pub pieces: usize,
    /// [x0, y0, x1, y1] texel bounds at full size (max exclusive), over all patches.
    pub bounds: [u32; 4],
    /// Bones that skin it, by share of weight, e.g. "leg_upper_R 88%, pelvis 4%".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bones: String,
    pub triangles: usize,
    /// Percent of its texels other regions also sample (often a mirrored
    /// left/right twin): paint there lands on both.
    #[serde(skip_serializing_if = "is_zero")]
    pub shared_percent: f32,
    /// Percent of its triangles mirrored in UV.
    #[serde(skip_serializing_if = "is_zero")]
    pub mirrored_percent: f32,
    /// UV area over covered texels, when above 1: its own triangles stack on the
    /// same texels (about 2 for mirrored halves).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uv_reuse: Option<f32>,
    /// False when other regions cover it entirely on the overlay, so no id is drawn.
    #[serde(skip_serializing_if = "is_true")]
    pub on_overlay: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MaskRow {
    /// White on black at the texture's full size: pass it as maskPath to
    /// replace_textures, or use it to composite.
    pub mask_png: String,
    pub regions: Vec<usize>,
    /// Percent of the texture the mask covers, before padding.
    pub coverage_percent: f32,
    /// Percent of the masked texels that unselected regions also sample.
    #[serde(skip_serializing_if = "is_zero")]
    pub shared_with_unselected_percent: f32,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(v: &bool) -> bool {
    *v
}

/// A 0..1 share as a percent with two decimals.
fn percent(share: f32) -> f32 {
    (share * 10_000.0).round() / 100.0
}

impl Engine {
    pub fn map_texture_regions(&self, p: &MapTextureRegionsParams) -> Result<TextureRegions> {
        let roster = self.roster()?;
        let hero = roster.resolve(&p.hero)?;
        let codename = &hero.info.codename;
        let (first, base) = self.preview_stack(p.vpk.as_deref())?;
        let rows = p.limit.unwrap_or(DEFAULT_REGION_ROWS).clamp(1, MAX_LIMIT);
        let stem = Path::new(&p.texture).file_stem().map_or_else(
            || "texture".to_owned(),
            |s| s.to_string_lossy().into_owned(),
        );
        let dir = self
            .config
            .staging
            .join("textures")
            .join(codename)
            .join(slug(&stem));
        let options = vpkmerge_core::RegionMapOptions {
            by: p.by.segment_by(),
            select: p.select.clone(),
            padding: p.padding.unwrap_or(4),
            labels: rows,
        };
        let map = vpkmerge_core::map_texture_regions(
            &first,
            base.as_deref(),
            codename,
            &p.texture,
            &options,
            &dir,
        )
        .with_context(|| {
            format!(
                "Call list_hero_textures with hero {:?} for the textures its model samples",
                hero.info.name
            )
        })?;

        let region_count = map.regions.len();
        let regions: Vec<RegionRow> = map
            .regions
            .iter()
            .take(rows)
            .map(|r| RegionRow {
                id: r.id,
                label: r.label.clone(),
                coverage_percent: percent(r.coverage),
                pieces: r.pieces,
                bounds: r.bounds,
                bones: r
                    .bones
                    .iter()
                    .filter(|(_, w)| *w >= 0.02)
                    .map(|(b, w)| format!("{b} {:.0}%", w * 100.0))
                    .collect::<Vec<_>>()
                    .join(", "),
                triangles: r.triangles,
                shared_percent: percent(r.shared),
                mirrored_percent: percent(r.mirrored),
                uv_reuse: (r.uv_reuse > 1.05).then(|| (r.uv_reuse * 100.0).round() / 100.0),
                on_overlay: r.labeled,
            })
            .collect();

        let mut notes = Vec::new();
        if regions.len() < region_count {
            notes.push(format!(
                "Showing the {} largest of {region_count} regions; raise limit for more.",
                regions.len()
            ));
        }
        if regions.iter().any(|r| r.shared_percent >= 50.0) {
            notes.push(
                "Regions with a high sharedPercent sample the same texels as another region \
                 (mirrored left/right parts usually do): painting one paints both, so the two \
                 cannot differ."
                    .to_owned(),
            );
        }
        let mask = map.mask.map(|m| MaskRow {
            mask_png: m.png.display().to_string(),
            regions: m.regions,
            coverage_percent: percent(m.coverage),
            shared_with_unselected_percent: percent(m.shared_with_unselected),
        });
        Ok(TextureRegions {
            texture: map.texture,
            size: format!("{}x{}", map.width, map.height),
            format: map.format,
            sampled_by: map
                .bindings
                .into_iter()
                .map(|(material, slot)| format!("{material} ({slot})"))
                .collect(),
            used_percent: percent(map.used),
            by: vpkmerge_core::texture_map::by_name(options.by),
            region_count,
            regions,
            texture_png: map.texture_png.display().to_string(),
            alpha_png: map.alpha_png.map(|p| p.display().to_string()),
            overlay_png: map.overlay_png.display().to_string(),
            mask,
            notes,
        })
    }
}

// ------------------------------------------------------------ replace_textures

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceTexturesParams {
    /// Each texture to replace. All go into one mod.
    pub replacements: Vec<TextureReplacementParam>,
    /// Short name for the staged file, e.g. "vindicta-card". Default: from the first entry.
    pub name: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AlphaMode {
    /// The PNG's alpha for UI art (paths under panorama/), the original's for
    /// material textures, where alpha is a data channel.
    #[default]
    Auto,
    /// The PNG's alpha (opaque when the PNG has none).
    Png,
    /// The original texture's alpha, untouched.
    Original,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextureReplacementParam {
    /// The texture entry (.vtex_c) from browse_textures or list_hero_textures.
    pub entry: String,
    /// Absolute path of the replacement .png, ideally at the texture's own size
    /// (other sizes are resized to it).
    pub png_path: String,
    /// Absolute path of a white-on-black mask PNG, e.g. from map_texture_regions:
    /// the PNG lands where white, the original stays where black.
    pub mask_path: Option<String>,
    /// Where the result's alpha comes from. Default: auto.
    #[serde(default)]
    pub alpha: AlphaMode,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextureMod {
    /// The built addon, in staging. Not installed.
    pub staged_vpk: String,
    pub written: Vec<TextureWritten>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextureWritten {
    pub entry: String,
    /// The size the PNG was fitted to.
    pub width: u32,
    pub height: u32,
    pub format: String,
    /// "png" or "original".
    pub alpha: &'static str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub masked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Engine {
    pub fn replace_textures(&self, p: &ReplaceTexturesParams) -> Result<TextureMod> {
        let pak = self.config.pak()?;
        let first = p
            .replacements
            .first()
            .context("replacements is empty: pass at least one { entry, pngPath }")?;
        let inputs = p
            .replacements
            .iter()
            .map(|r| {
                let png = read_input(&user_path(&r.png_path), "PNG")?;
                let mask = r
                    .mask_path
                    .as_deref()
                    .map(|m| read_input(&user_path(m), "mask"))
                    .transpose()?;
                Ok((png, mask))
            })
            .collect::<Result<Vec<_>>>()?;
        let replacements: Vec<vpkmerge_core::TextureReplacement> = p
            .replacements
            .iter()
            .zip(&inputs)
            .map(|(r, (png, mask))| vpkmerge_core::TextureReplacement {
                entry: &r.entry,
                png,
                mask: mask.as_deref(),
                alpha: match r.alpha {
                    AlphaMode::Png => vpkmerge_core::AlphaSource::Png,
                    AlphaMode::Auto if r.entry.starts_with("panorama/") => {
                        vpkmerge_core::AlphaSource::Png
                    }
                    AlphaMode::Original | AlphaMode::Auto => vpkmerge_core::AlphaSource::Original,
                },
            })
            .collect();

        let name = p.name.clone().unwrap_or_else(|| {
            Path::new(&first.entry).file_stem().map_or_else(
                || "textures".to_owned(),
                |s| s.to_string_lossy().into_owned(),
            )
        });
        let out = self.staged(&format!("texture-{}", slug(&name)));
        let written = vpkmerge_core::build_texture_addon(pak, &replacements, &out)
            .context("Call browse_textures or list_hero_textures to find valid texture entries")?;
        Ok(TextureMod {
            staged_vpk: out.display().to_string(),
            written: written
                .into_iter()
                .zip(&replacements)
                .map(|(w, r)| TextureWritten {
                    entry: w.entry,
                    width: w.template.width,
                    height: w.template.height,
                    format: w.template.format,
                    alpha: match r.alpha {
                        vpkmerge_core::AlphaSource::Png => "png",
                        vpkmerge_core::AlphaSource::Original => "original",
                    },
                    masked: r.mask.is_some(),
                    note: w.note,
                })
                .collect(),
        })
    }
}

// ----------------------------------------------------------- recolor_hero_vfx

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RecolorMode {
    /// One color for every effect.
    #[default]
    Solid,
    /// A rainbow spread across the effects.
    Prism,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecolorVfxParams {
    /// Hero display name or codename.
    pub hero: String,
    /// Default: solid.
    #[serde(default)]
    pub mode: RecolorMode,
    /// Solid mode: target hue in degrees. 0 red, 30 orange, 60 yellow, 120 green,
    /// 180 cyan, 240 blue, 280 purple, 320 pink.
    pub hue: Option<f64>,
    /// Saturation multiplier. Default 1.0; below 1 is more pastel.
    pub saturation: Option<f64>,
    /// Brightness multiplier. Default 1.0.
    pub brightness: Option<f64>,
    /// Prism mode: also animate the rainbow over each effect's lifetime.
    #[serde(default)]
    pub animated: bool,
    /// Solid mode: write a small PNG swatch of the result instead of building the mod.
    #[serde(default)]
    pub preview_only: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VfxRecolor {
    /// The built addon, in staging. Not installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staged_vpk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_png: Option<String>,
    pub hero: String,
    /// Files in the addon.
    pub entries: usize,
    pub particles: usize,
    pub textures: usize,
    pub materials: usize,
    pub models: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Engine {
    pub fn recolor_hero_vfx(&self, p: &RecolorVfxParams) -> Result<VfxRecolor> {
        let pak = self.config.pak()?;
        let roster = self.roster()?;
        let hero = roster.resolve(&p.hero)?;
        let codename = recolor_codename(hero).with_context(|| {
            format!(
                "{} has no VFX recolor recipe yet. Heroes that do: {}",
                hero.info.name,
                roster.names(|h| recolor_codename(h).is_some())
            )
        })?;
        let saturation = p.saturation.unwrap_or(1.0);
        let brightness = p.brightness.unwrap_or(1.0);
        let mut result = VfxRecolor {
            staged_vpk: None,
            preview_png: None,
            hero: hero.info.name.clone(),
            entries: 0,
            particles: 0,
            textures: 0,
            materials: 0,
            models: 0,
            note: None,
        };
        let partial = |particles: usize, materials: usize| {
            (particles + materials > 0).then(|| {
                format!(
                    "Partial: {particles} particle(s) and {materials} material(s) could not \
                     be patched and keep their original color."
                )
            })
        };

        match p.mode {
            RecolorMode::Solid => {
                let hue = p.hue.context(
                    "solid mode needs hue (0-360): 0 red, 120 green, 240 blue, 280 purple",
                )?;
                let recolor = Recolor::new(hue, saturation, brightness);
                if p.preview_only {
                    let png =
                        vpkmerge_core::recolor_hero_preview_png(pak, None, codename, recolor)?;
                    let out = self
                        .config
                        .staging
                        .join(format!("preview-{codename}-hue{hue:.0}.png"));
                    write_output(&out, &png)?;
                    result.preview_png = Some(out.display().to_string());
                    return Ok(result);
                }
                let out = self.staged(&format!("vfx-{codename}-hue{hue:.0}"));
                let r = vpkmerge_core::recolor_hero_to_addon(pak, None, codename, recolor, &out)?;
                result.staged_vpk = Some(out.display().to_string());
                result.entries = r.total_entries;
                result.particles = r.particles_recolored;
                result.textures = r.textures_recolored;
                result.materials = r.materials_recolored;
                result.models = r.models_recolored;
                result.note = partial(r.particles_unpatchable, r.materials_unpatchable);
            }
            RecolorMode::Prism => {
                ensure!(
                    !p.preview_only,
                    "previewOnly is for solid mode. Build the prism mod to see it."
                );
                let tuning = PrismTuning {
                    saturation,
                    brightness,
                    ..PrismTuning::default()
                };
                let suffix = if p.animated { "-animated" } else { "" };
                let out = self.staged(&format!("vfx-{codename}-prism{suffix}"));
                let r = vpkmerge_core::prism_recolor_hero_to_addon_tuned(
                    pak, None, codename, p.animated, tuning, &out,
                )?;
                result.staged_vpk = Some(out.display().to_string());
                result.entries = r.total_entries;
                result.particles = r.particles_recolored;
                result.textures = r.textures_recolored;
                result.materials = r.materials_recolored;
                result.models = r.models_recolored;
                result.note = partial(r.particles_unpatchable, r.materials_unpatchable);
            }
        }
        Ok(result)
    }
}

// ------------------------------------------------------- merge_mods / inspect

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MergePolicy {
    /// When two mods ship the same file, the later input wins.
    #[default]
    LastWins,
    /// The earlier input wins.
    FirstWins,
    /// Refuse to merge if any file appears in more than one input.
    Strict,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MergeModsParams {
    /// Absolute paths of the VPKs to merge, in priority order. For a multi-file
    /// VPK pass only its _dir.vpk.
    pub inputs: Vec<String>,
    /// Where to write the merged VPK: a path that does not exist yet. Default: staging.
    pub out_path: Option<String>,
    /// Default: last-wins.
    #[serde(default)]
    pub policy: MergePolicy,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MergedMod {
    pub out_path: String,
    pub total_entries: usize,
    /// Files that appeared in more than one input and were resolved by the policy.
    pub conflicts_resolved: usize,
}

impl Engine {
    pub fn merge_mods(&self, p: &MergeModsParams) -> Result<MergedMod> {
        let inputs: Vec<PathBuf> = p.inputs.iter().map(|i| user_path(i)).collect();
        let out = match &p.out_path {
            Some(out) => new_output(out)?,
            None => self.staged("merged"),
        };
        ensure!(
            !inputs.contains(&out),
            "The default output path is one of the inputs. Pass outPath."
        );
        let options = MergeOptions {
            collision_policy: match p.policy {
                MergePolicy::LastWins => CollisionPolicy::LastWins,
                MergePolicy::FirstWins => CollisionPolicy::FirstWins,
                MergePolicy::Strict => CollisionPolicy::Error,
            },
            ..MergeOptions::default()
        };
        let report = vpkmerge_core::merge(&inputs, &out, &options)?;
        Ok(MergedMod {
            out_path: out.display().to_string(),
            total_entries: report.total_entries,
            conflicts_resolved: report.overridden_paths,
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InspectModParams {
    /// Absolute path of the VPK (its _dir.vpk).
    pub vpk: String,
    /// Other VPKs to check for files that collide with this one.
    #[serde(default)]
    pub against: Vec<String>,
    /// Maximum entries and conflicts to list. Default 30, max 200.
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModInspection {
    pub entry_count: usize,
    pub size_bytes: u64,
    /// The first `limit` entries, sorted.
    pub entries: Vec<String>,
    /// Present when `against` was given: how many files collide.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_count: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<ModConflict>,
    /// One row per `.vdata_c` the mod ships, compared against the game's copy.
    /// Absent when the mod ships none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vdata: Vec<VdataFinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VdataFinding {
    /// VPK entry path, e.g. `scripts/heroes.vdata_c`.
    pub entry: String,
    pub status: VdataVerdict,
    /// How many key paths the game's copy has that the mod's lacks. A mod's
    /// `.vdata_c` replaces the whole file, so these are deleted in game.
    pub missing_count: usize,
    /// How many key paths only the mod's copy has (fields the game dropped).
    pub extra_count: usize,
    /// How many values differ from the game's copy. Mixes the mod's intended
    /// edits with later Valve rebalances, so it is a count only.
    pub changed_count: usize,
    /// The first `limit` missing paths, shallowest first (a whole missing hero
    /// or ability comes before its fields).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// The first `limit` paths only the mod has.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<String>,
    /// Decode error text when status is `undecodable`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum VdataVerdict {
    /// The mod's copy lacks fields the current game has, so it deletes them in
    /// game. The mod was built against an older game and needs an update or rebuild.
    Outdated,
    /// Same structure as the game's copy with different values: the mod's
    /// intended edits, possibly mixed with later Valve rebalances.
    Modified,
    /// Identical to the game's copy: redundant, harmless.
    Current,
    /// The game has no file at this path (compiler leftover or custom file). Nothing to compare.
    NotInGame,
    /// Either copy failed to decode as KV3.
    Undecodable,
}

impl From<VdataStatus> for VdataVerdict {
    fn from(s: VdataStatus) -> Self {
        match s {
            VdataStatus::Outdated => Self::Outdated,
            VdataStatus::Modified => Self::Modified,
            VdataStatus::Current => Self::Current,
            VdataStatus::NotInGame => Self::NotInGame,
            VdataStatus::Undecodable => Self::Undecodable,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModConflict {
    pub path: String,
    /// The VPKs that ship this file.
    pub owners: Vec<String>,
}

impl Engine {
    pub fn inspect_mod(&self, p: &InspectModParams) -> Result<ModInspection> {
        let limit = limit(p.limit);
        let vpk = user_path(&p.vpk);
        let info = vpkmerge_core::inspect(&vpk)?;
        let mut entries = info.file_paths;
        entries.sort();
        let has_vdata = entries.iter().any(|e| e.ends_with(".vdata_c"));
        let entry_count = entries.len();
        entries.truncate(limit);

        let mut notes = Vec::new();
        let mut vdata = Vec::new();
        if has_vdata {
            match self.config.pak() {
                Ok(pak) => {
                    for r in vpkmerge_core::check_vdata(&vpk, pak)? {
                        vdata.push(VdataFinding {
                            entry: r.entry,
                            status: r.status.into(),
                            missing_count: r.missing.len(),
                            extra_count: r.extra.len(),
                            changed_count: r.changed.len(),
                            missing: r.missing.into_iter().take(limit).collect(),
                            extra: r.extra.into_iter().take(limit).collect(),
                            error: r.error,
                        });
                    }
                }
                Err(e) => notes.push(format!("vdata was not checked against the game: {e:#}")),
            }
        }

        let mut conflict_count = None;
        let mut conflicts = Vec::new();
        if !p.against.is_empty() {
            let mut inputs = vec![vpk];
            inputs.extend(p.against.iter().map(|a| user_path(a)));
            let mut found = vpkmerge_core::detect_conflicts(&inputs)?;
            found.sort_by(|a, b| a.path.cmp(&b.path));
            conflict_count = Some(found.len());
            conflicts = found
                .into_iter()
                .take(limit)
                .map(|c| ModConflict {
                    path: c.path,
                    owners: c
                        .owner_indices
                        .iter()
                        .map(|i| inputs[*i].display().to_string())
                        .collect(),
                })
                .collect();
        }
        let truncated = entries.len() < entry_count
            || conflict_count.is_some_and(|n| conflicts.len() < n)
            || vdata
                .iter()
                .any(|v| v.missing.len() < v.missing_count || v.extra.len() < v.extra_count);
        if truncated {
            notes.push(format!(
                "Lists are cut to {limit} rows. Raise limit for more."
            ));
        }
        Ok(ModInspection {
            entry_count,
            size_bytes: info.size_bytes,
            entries,
            conflict_count,
            conflicts,
            vdata,
            note: (!notes.is_empty()).then(|| notes.join(" ")),
        })
    }
}

// ----------------------------------------------------------------------- docs

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchDocsParams {
    /// What you want to know, as keywords, e.g. "addon load order" or "loop points vsnd".
    pub query: String,
    /// Maximum sections to return. Default 6, max 20.
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocSearch {
    pub results: Vec<DocHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocHit {
    /// Document id to pass to read_doc.
    pub doc: String,
    pub title: String,
    /// Section heading to pass to read_doc.
    pub section: String,
    pub snippet: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadDocParams {
    /// Document id from search_docs.
    pub doc: String,
    /// Section heading from search_docs. Omit to read the whole document.
    pub section: Option<String>,
}

impl Engine {
    #[must_use]
    pub fn search_docs(&self, p: &SearchDocsParams) -> DocSearch {
        let docs = self.docs();
        let results: Vec<DocHit> = docs
            .search(&p.query, p.limit.unwrap_or(6).clamp(1, 20))
            .into_iter()
            .map(|h| DocHit {
                doc: h.doc.to_owned(),
                title: h.title.to_owned(),
                section: h.section.to_owned(),
                snippet: h.snippet,
            })
            .collect();
        let note = results.is_empty().then(|| {
            format!(
                "Nothing matched. Try other keywords, or read one of these with read_doc:\n{}",
                docs.catalog()
                    .map(|(id, title)| format!("- {id}: {title}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        });
        DocSearch { results, note }
    }

    pub fn read_doc(&self, p: &ReadDocParams) -> Result<String> {
        self.docs().read(&p.doc, p.section.as_deref())
    }
}

// ---------------------------------------------------------------- preview_hero

/// Each preview is tens of megabytes of textures, so only the newest few stay.
const KEPT_PREVIEWS: usize = 5;
/// Animation files are small, but one is written per animation played.
const KEPT_ANIMATIONS: usize = 40;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreviewHeroParams {
    /// Hero display name or codename, e.g. "Mina".
    pub hero: String,
    /// Absolute path of a mod VPK (its _dir.vpk) to show on the hero. Omit to show
    /// the unmodded hero.
    pub vpk: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeroPreview {
    /// The hero as a binary glTF. Apps that understand it show it to the user on
    /// their own; anywhere else, give the user this path.
    pub preview_glb: String,
    pub hero: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vpk: Option<String>,
    /// The menu pose the model opens in, one of list_hero_animations. Absent when
    /// the hero has no pose to play and the model is a static posed mesh.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pose: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeroAnimationsParams {
    /// Hero display name or codename.
    pub hero: String,
    /// The mod VPK a preview shows, if any. A mod can ship its own animations.
    pub vpk: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeroAnimations {
    pub hero: String,
    /// Prefix of the hero's own animations, as in `<codename>_ability_...`.
    pub codename: String,
    /// Names for preview_hero_animation: idles, movement, abilities, emotes.
    pub animations: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnimationParams {
    /// Hero display name or codename.
    pub hero: String,
    /// The mod VPK the preview shows, if any.
    pub vpk: Option<String>,
    /// One name from list_hero_animations.
    pub animation: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeroAnimationClip {
    /// The skeleton and this one animation as a binary glTF, played on a model
    /// from preview_hero.
    pub animation_glb: String,
    pub animation: String,
}

impl Engine {
    pub fn preview_hero(&self, p: &PreviewHeroParams) -> Result<HeroPreview> {
        let roster = self.roster()?;
        let hero = roster.resolve(&p.hero)?;
        let codename = &hero.info.codename;
        let (first, base) = self.preview_stack(p.vpk.as_deref())?;

        let dir = self.config.staging.join(".previews");
        let out = dir.join(format!("{codename}-{}.glb", unix_millis()));
        let pose =
            vpkmerge_core::model::export_hero_preview(&first, codename, base.as_deref(), &out)?;
        if pose.is_none() {
            // Nothing to play the pose from. `model export --pose` also reaches a
            // mesh skin's base-game clips, so bake its still instead of a T-pose.
            let anim = vpkmerge_core::AnimOptions {
                pose: Some(vpkmerge_core::PoseSelection::default()),
                ..vpkmerge_core::AnimOptions::default()
            };
            vpkmerge_core::export_hero_model(&first, codename, base.as_deref(), &anim, &out)?;
        }
        prune_glbs(&dir, KEPT_PREVIEWS);

        Ok(HeroPreview {
            preview_glb: out.display().to_string(),
            hero: hero.info.name.clone(),
            vpk: p.vpk.as_deref().map(|v| user_path(v).display().to_string()),
            pose,
        })
    }

    pub fn list_hero_animations(&self, p: &HeroAnimationsParams) -> Result<HeroAnimations> {
        let roster = self.roster()?;
        let hero = roster.resolve(&p.hero)?;
        let (first, base) = self.preview_stack(p.vpk.as_deref())?;
        Ok(HeroAnimations {
            hero: hero.info.name.clone(),
            codename: hero.info.codename.clone(),
            animations: vpkmerge_core::model::hero_preview_clips(
                &first,
                &hero.info.codename,
                base.as_deref(),
            )?,
        })
    }

    pub fn preview_hero_animation(&self, p: &PreviewAnimationParams) -> Result<HeroAnimationClip> {
        let roster = self.roster()?;
        let hero = roster.resolve(&p.hero)?;
        let codename = &hero.info.codename;
        let (first, base) = self.preview_stack(p.vpk.as_deref())?;

        let dir = self.config.staging.join(".previews").join("animations");
        let out = dir.join(format!(
            "{codename}-{}-{}.glb",
            slug(&p.animation),
            unix_millis()
        ));
        vpkmerge_core::model::export_hero_clip(
            &first,
            codename,
            base.as_deref(),
            &p.animation,
            &out,
        )?;
        prune_glbs(&dir, KEPT_ANIMATIONS);

        Ok(HeroAnimationClip {
            animation_glb: out.display().to_string(),
            animation: p.animation.clone(),
        })
    }

    /// The VPK a preview reads first, and the base pak behind it: a mod's files
    /// override the game's.
    fn preview_stack(&self, vpk: Option<&str>) -> Result<(PathBuf, Option<PathBuf>)> {
        let pak = self.config.pak()?.to_path_buf();
        Ok(match vpk {
            Some(vpk) => (user_path(vpk), Some(pak)),
            None => (pak, None),
        })
    }
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn prune_glbs(dir: &Path, keep: usize) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut glbs: Vec<_> = read
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "glb"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    glbs.sort_by_key(|g| std::cmp::Reverse(g.0));
    for (_, path) in glbs.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}

// ------------------------------------------------------------ open_in_grimoire

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenInGrimoireParams {
    /// Absolute path of the mod VPK (its _dir.vpk), usually one a build tool staged.
    pub vpk: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GrimoireHandoff {
    pub grimoire: String,
    pub vpk: String,
    pub note: String,
}

impl Engine {
    pub fn open_in_grimoire(p: &OpenInGrimoireParams) -> Result<GrimoireHandoff> {
        let vpk = user_path(&p.vpk);
        let bin = crate::grimoire::find()?;
        crate::grimoire::open(&bin, &vpk)?;
        Ok(GrimoireHandoff {
            grimoire: bin.display().to_string(),
            vpk: vpk.display().to_string(),
            note: "Grimoire's import dialog opens with this mod. The user confirms the import \
                   there and can enable it from Installed."
                .to_owned(),
        })
    }
}

// -------------------------------------------------------------------- helpers

fn user_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// A caller-chosen output path. It must be new: the tools only add files, which is
/// what lets them declare themselves non-destructive to the host.
fn new_output(path: &str) -> Result<PathBuf> {
    let path = user_path(path);
    ensure!(
        !path.exists(),
        "{} already exists. Pick an outPath that does not exist yet.",
        path.display()
    );
    Ok(path)
}

fn read_input(path: &Path, what: &str) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {what} file {}", path.display()))
}

fn write_output(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// A filename-safe form of `s`: lowercase alphanumerics joined by single dashes.
fn slug(s: &str) -> String {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_filename_safe() {
        assert_eq!(slug("Seven.Melee.Swing"), "seven-melee-swing");
        assert_eq!(slug("  a//b__C "), "a-b-c");
    }

    #[test]
    fn addon_slot_accepts_only_mountable_names() {
        assert_eq!(addon_slot("pak07_dir.vpk"), Some(7));
        assert_eq!(addon_slot("pak7_dir.vpk"), None);
        assert_eq!(addon_slot("cool_mod_dir.vpk"), None);
        assert_eq!(addon_slot("pak07_000.vpk"), None);
    }

    #[test]
    fn search_needs_every_term() {
        let terms = search_terms(Some("Melee  swing"));
        assert!(matches(&terms, &["Swing", "Seven.Melee.Swing"]));
        assert!(!matches(&terms, &["Fire", "Seven.Wpn.Fire"]));
        assert!(matches(&[], &["anything"]));
    }

    #[test]
    fn open_ended_trim_fills_the_missing_side() {
        let edit = |start, end| AudioEdit {
            trim_start_ms: start,
            trim_end_ms: end,
            gain_db: None,
        };
        // Not MP3, so a requested trim fails and no trim passes the bytes through.
        assert!(edit(Some(100), None).apply(b"not audio").is_err());
        assert!(edit(None, Some(100)).apply(b"not audio").is_err());
        assert_eq!(edit(None, None).apply(b"not audio").unwrap(), b"not audio");
    }
}
