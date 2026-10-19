//! Detect and heal model swaps built before Valve re-framed the `root_motion`
//! attachments.
//!
//! A Deadlock update re-expressed every attachment parented to `root_motion`
//! (the camera pivots, `aim`, `root_aim`, `zipline`, `lean`, ...) in a new
//! frame. On the werewolf model `standing_pivot` went from offset `[0, 94, 0]`
//! at angles `(-90, -90, 0)` to `[0, 0, 94]` at `(0, 0, 0)`. The skeleton did
//! not change, but a model compiled against the old base still carries the old
//! values in every mesh's `MDAT` block, and the hero's camera breaks in game.
//!
//! [`AttachmentHealer::check`] compares those attachments with the game's copy
//! of the same model. [`AttachmentHealer::heal`] updates existing vectors in
//! place, restores absent camera attachments and settings, and converts legacy
//! extra camera poses that no longer have a same-name game counterpart. Every
//! other VPK entry is repacked unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use morphic::kv3::{Seg, Value};
use morphic::resource::Resource;
use serde::Serialize;

mod camera;

const ROOT_BONE: &str = "root_motion";

/// Rotation gap from the game's value past which a `root_motion` attachment
/// counts as built for the old frame. The re-frame turned them by 90 degrees or
/// more; an author's deliberate tweak is far smaller.
pub const STALE_DEGREES: f64 = 30.0;

/// How a mod's model compares with the game's copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttachmentStatus {
    /// Camera inputs are complete and use the current attachment frame.
    Current,
    /// An attachment uses the old frame, or required camera inputs are absent.
    Stale,
    /// The game ships no model at this path. Nothing to compare.
    NotInGame,
    /// The game's model has no `root_motion` attachments (props, abilities), so
    /// the re-frame does not touch it.
    Unaffected,
    /// Either copy failed to read or decode.
    Undecodable,
}

/// Comparison of one `.vmdl_c` entry in a mod against the game's copy.
#[derive(Debug, Clone, Serialize)]
pub struct AttachmentReport {
    /// VPK entry path, e.g. `models/heroes_wip/werewolf/werewolf.vmdl_c`.
    pub entry: String,
    pub status: AttachmentStatus,
    /// Outdated or missing attachment names, plus `CitadelCameraSettings_t` when
    /// the required settings block is absent.
    pub stale: Vec<String>,
    /// Error text for [`AttachmentStatus::Undecodable`], prefixed `mod: ` or `game: `.
    pub error: Option<String>,
}

impl AttachmentReport {
    fn new(entry: &str, status: AttachmentStatus) -> Self {
        Self {
            entry: entry.to_string(),
            status,
            stale: Vec::new(),
            error: None,
        }
    }

    fn undecodable(entry: &str, error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::new(entry, AttachmentStatus::Undecodable)
        }
    }
}

/// One model [`AttachmentHealer::heal`] rewrote.
#[derive(Debug, Clone, Serialize)]
pub struct HealedModel {
    pub entry: String,
    /// Updated or restored camera inputs.
    pub attachments: Vec<String>,
    /// `MDAT` blocks patched (each LOD mesh carries its own copy).
    pub blocks: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct Attachment {
    /// Position in the block's `m_attachments` array, for patch paths.
    index: usize,
    name: String,
    parent: String,
    offsets: Vec<Vec<f64>>,
    rotations: Vec<Vec<f64>>,
}

enum BaseModel {
    Missing,
    Attachments(GameModel),
    Error(String),
}

struct GameModel {
    root: HashMap<String, Attachment>,
    camera: camera::Template,
}

/// Checks and heals mods against one game pak. The pak is opened the first
/// time a mod ships a `.vmdl_c`, and each game model is read and decoded at
/// most once.
pub struct AttachmentHealer {
    base_path: PathBuf,
    base: Option<valve_pak::VPK>,
    models: HashMap<String, BaseModel>,
}

impl AttachmentHealer {
    pub fn new<B: AsRef<Path>>(base_vpk: B) -> Self {
        Self {
            base_path: base_vpk.as_ref().to_path_buf(),
            base: None,
            models: HashMap::new(),
        }
    }

    /// Compare every `.vmdl_c` in `mod_vpk` (sorted by path) against the game
    /// pak. A mod that ships none returns an empty list without touching the pak.
    ///
    /// # Errors
    /// Fails if either VPK cannot be opened. Per-entry problems are reported in
    /// the returned [`AttachmentReport`]s instead.
    pub fn check<M: AsRef<Path>>(&mut self, mod_vpk: M) -> Result<Vec<AttachmentReport>> {
        Ok(self
            .scan(mod_vpk.as_ref())?
            .into_iter()
            .map(|(report, _)| report)
            .collect())
    }

    /// Rewrite the `root_motion` attachments of every stale model in `mod_vpk`
    /// with the game's values and write the result to `output`, every other
    /// entry unchanged. Writes nothing and returns an empty list when no model
    /// is stale. `output` must not be `mod_vpk`.
    ///
    /// # Errors
    /// Fails if either VPK cannot be opened, a stale model cannot be patched,
    /// or `output` cannot be written.
    pub fn heal<M: AsRef<Path>, O: AsRef<Path>>(
        &mut self,
        mod_vpk: M,
        output: O,
    ) -> Result<Vec<HealedModel>> {
        let mod_vpk = mod_vpk.as_ref();
        let mut healed = Vec::new();
        let mut patched: Vec<(String, Vec<u8>)> = Vec::new();
        for (report, bytes) in self.scan(mod_vpk)? {
            let Some(bytes) = bytes else { continue };
            let Some(BaseModel::Attachments(base)) = self.models.get(&report.entry) else {
                continue;
            };
            let (out, model) = heal_model(&report.entry, &bytes, base)
                .with_context(|| format!("healing {}", report.entry))?;
            patched.push((report.entry, out));
            healed.push(model);
        }
        if patched.is_empty() {
            return Ok(healed);
        }
        let extras: Vec<(&str, &[u8])> = patched
            .iter()
            .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
            .collect();
        crate::embed_metadata(mod_vpk, None, &extras, &[], output)?;
        Ok(healed)
    }

    /// Reports for every `.vmdl_c` in the mod, with the mod's bytes kept for the
    /// stale ones.
    fn scan(&mut self, mod_path: &Path) -> Result<Vec<(AttachmentReport, Option<Vec<u8>>)>> {
        let mod_pak = valve_pak::open(mod_path)
            .with_context(|| format!("opening mod {}", mod_path.display()))?;
        let mut entries: Vec<String> = mod_pak
            .file_paths()
            .filter(|p| p.ends_with(".vmdl_c"))
            .cloned()
            .collect();
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        entries.sort();

        let base = match &mut self.base {
            Some(base) => base,
            slot @ None => slot.insert(
                valve_pak::open(&self.base_path)
                    .with_context(|| format!("opening base {}", self.base_path.display()))?,
            ),
        };
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let report = match base_model(base, &mut self.models, &entry) {
                BaseModel::Missing => AttachmentReport::new(&entry, AttachmentStatus::NotInGame),
                BaseModel::Error(e) => AttachmentReport::undecodable(&entry, format!("game: {e}")),
                BaseModel::Attachments(atts) if atts.root.is_empty() => {
                    AttachmentReport::new(&entry, AttachmentStatus::Unaffected)
                }
                BaseModel::Attachments(atts) => {
                    match mod_pak.get_file(&entry).and_then(|mut f| f.read_all()) {
                        Err(e) => AttachmentReport::undecodable(&entry, format!("mod: {e:#}")),
                        Ok(bytes) => match model_issues(&bytes, atts) {
                            Err(e) => AttachmentReport::undecodable(&entry, format!("mod: {e:#}")),
                            Ok(stale) if stale.is_empty() => {
                                AttachmentReport::new(&entry, AttachmentStatus::Current)
                            }
                            Ok(stale) => {
                                let mut report =
                                    AttachmentReport::new(&entry, AttachmentStatus::Stale);
                                report.stale = stale;
                                out.push((report, Some(bytes)));
                                continue;
                            }
                        },
                    }
                }
            };
            out.push((report, None));
        }
        Ok(out)
    }
}

/// The game's `root_motion` attachments for `entry`, read on first use.
fn base_model<'a>(
    base: &valve_pak::VPK,
    models: &'a mut HashMap<String, BaseModel>,
    entry: &str,
) -> &'a BaseModel {
    models.entry(entry.to_string()).or_insert_with(|| {
        let Ok(mut file) = base.get_file(entry) else {
            return BaseModel::Missing;
        };
        match file.read_all().and_then(|bytes| {
            Ok(GameModel {
                root: root_attachments(&bytes)?,
                camera: camera::Template::read(&bytes)?,
            })
        }) {
            Ok(model) => BaseModel::Attachments(model),
            Err(e) => BaseModel::Error(format!("{e:#}")),
        }
    })
}

fn model_issues(model: &[u8], base: &GameModel) -> Result<Vec<String>> {
    let mut issues = stale_attachments(model, &base.root)?;
    for name in base.camera.issues(model)? {
        if !issues.contains(&name) {
            issues.push(name);
        }
    }
    Ok(issues)
}

/// The `root_motion` attachments of the first mesh block that has any attachments.
fn root_attachments(model: &[u8]) -> Result<HashMap<String, Attachment>> {
    let res = Resource::parse(model)?;
    for raw in mdat_blocks(&res) {
        if let Some(atts) = attachments(&morphic::kv3::decode(raw?)?) {
            return Ok(atts
                .into_iter()
                .filter(|a| a.parent == ROOT_BONE)
                .map(|a| (a.name.clone(), a))
                .collect());
        }
    }
    Ok(HashMap::new())
}

fn mdat_blocks<'a>(res: &'a Resource<'a>) -> impl Iterator<Item = Result<&'a [u8]>> + 'a {
    res.blocks()
        .iter()
        .enumerate()
        .filter(|(_, b)| &b.kind == b"MDAT")
        .map(|(i, _)| res.get_block_by_index(i).context("MDAT out of range"))
}

/// Names of the mod's stale attachments across every `MDAT` block, deduped,
/// in first-seen order.
fn stale_attachments(model: &[u8], base: &HashMap<String, Attachment>) -> Result<Vec<String>> {
    let res = Resource::parse(model)?;
    let mut stale: Vec<String> = Vec::new();
    for raw in mdat_blocks(&res) {
        let Some(atts) = attachments(&morphic::kv3::decode(raw?)?) else {
            continue;
        };
        for name in stale_in(&atts, base) {
            if !stale.contains(&name) {
                stale.push(name);
            }
        }
    }
    Ok(stale)
}

/// Attachments both sides parent to `root_motion` whose first influence is
/// rotated more than [`STALE_DEGREES`] away from the game's.
fn stale_in(atts: &[Attachment], base: &HashMap<String, Attachment>) -> Vec<String> {
    atts.iter()
        .filter_map(|a| {
            if let Some(b) = shared(a, base) {
                let gap = rotation_gap_degrees(a.rotations.first()?, b.rotations.first()?)?;
                (gap > STALE_DEGREES).then(|| a.name.clone())
            } else {
                camera::reframe_extra(a, base).map(|_| a.name.clone())
            }
        })
        .collect()
}

/// The game's attachment matching `att` by name, when both parent it to `root_motion`.
fn shared<'a>(att: &Attachment, base: &'a HashMap<String, Attachment>) -> Option<&'a Attachment> {
    if att.parent != ROOT_BONE {
        return None;
    }
    base.get(&att.name).filter(|b| b.parent == ROOT_BONE)
}

/// Angle in degrees between two `[x, y, z, w]` quaternions, sign-insensitive.
fn rotation_gap_degrees(a: &[f64], b: &[f64]) -> Option<f64> {
    if a.len() != 4 || b.len() != 4 {
        return None;
    }
    let norm = |q: &[f64]| q.iter().map(|c| c * c).sum::<f64>().sqrt();
    let (na, nb) = (norm(a), norm(b));
    if na < 1e-9 || nb < 1e-9 {
        return None;
    }
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>() / (na * nb);
    Some(2.0 * dot.abs().min(1.0).acos().to_degrees())
}

/// Patch every `MDAT` block of a stale model (each LOD mesh carries its own
/// copy of the attachments).
fn heal_model(entry: &str, model: &[u8], base: &GameModel) -> Result<(Vec<u8>, HealedModel)> {
    let mdat_indices: Vec<usize> = Resource::parse(model)?
        .blocks()
        .iter()
        .enumerate()
        .filter(|(_, b)| &b.kind == b"MDAT")
        .map(|(i, _)| i)
        .collect();

    let mut out = model.to_vec();
    let mut names: Vec<String> = Vec::new();
    let mut blocks = 0;
    for index in mdat_indices {
        let res = Resource::parse(&out)?;
        let raw = res.get_block_by_index(index).context("MDAT out of range")?;
        let vectors =
            heal_block(raw, &base.root).with_context(|| format!("patching MDAT block {index}"))?;
        let attachments = base
            .camera
            .restore_block(vectors.as_ref().map_or(raw, |(bytes, _)| bytes))?;
        if vectors.is_none() && attachments.is_none() {
            continue;
        }
        let mut healed = vectors
            .as_ref()
            .map_or_else(Vec::new, |(_, names)| names.clone());
        if let Some((_, added)) = &attachments {
            healed.extend(added.iter().cloned());
        }
        let patched = attachments.or(vectors).context("Missing patched MDAT")?.0;
        for name in healed {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        out = res.rebuild_with_block(index, &patched)?;
        blocks += 1;
    }
    if let Some(updated) = base.camera.restore_settings(&out)? {
        out = updated;
        names.push(camera::SETTINGS.into());
    }
    Ok((
        out,
        HealedModel {
            entry: entry.to_string(),
            attachments: names,
            blocks,
        },
    ))
}

/// Write the game's offsets and rotations over every attachment of one `MDAT`
/// KV3 block that both sides parent to `root_motion`, in place at whatever width
/// the mod stores them (`kv3::set_reals`). Returns the patched block and the
/// healed names, or `None` when the block has no such attachment.
fn heal_block(
    raw: &[u8],
    base: &HashMap<String, Attachment>,
) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let Some(atts) = attachments(&morphic::kv3::decode(raw)?) else {
        return Ok(None);
    };
    let mut block: Option<Vec<u8>> = None;
    let mut names = Vec::new();
    for a in &atts {
        let extra = camera::reframe_extra(a, base);
        let Some(game) = shared(a, base).or(extra.as_ref()) else {
            continue;
        };
        let edits = attachment_edits(a, game);
        if edits.is_empty() {
            continue;
        }
        let patched = morphic::kv3::set_reals(block.as_deref().unwrap_or(raw), &edits)
            .with_context(|| format!("attachment {}", a.name))?;
        block = Some(patched);
        names.push(a.name.clone());
    }
    Ok(block.map(|b| (b, names)))
}

/// Writes copying the game's offset and rotation components onto the mod's
/// attachment, influence by influence, at the mod's index.
fn attachment_edits(ours: &Attachment, game: &Attachment) -> Vec<(Vec<Seg>, f64)> {
    let mut edits = Vec::new();
    for (field, mod_vectors, game_vectors) in [
        ("m_vInfluenceOffsets", &ours.offsets, &game.offsets),
        ("m_vInfluenceRotations", &ours.rotations, &game.rotations),
    ] {
        for (j, (mine, theirs)) in mod_vectors.iter().zip(game_vectors).enumerate() {
            for (c, value) in theirs.iter().enumerate().take(mine.len()) {
                edits.push((
                    vec![
                        Seg::Key("m_attachments".into()),
                        Seg::Index(ours.index),
                        Seg::Key("value".into()),
                        Seg::Key(field.into()),
                        Seg::Index(j),
                        Seg::Index(c),
                    ],
                    *value,
                ));
            }
        }
    }
    edits
}

/// A mesh block's attachments, or `None` when it has no `m_attachments`.
/// Entries missing a name, parent or transform are skipped.
fn attachments(mdat: &Value) -> Option<Vec<Attachment>> {
    let list = mdat.get("m_attachments")?.as_array()?;
    Some(
        list.iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let value = item.get("value")?;
                Some(Attachment {
                    index,
                    name: item.get("key")?.as_str()?.to_string(),
                    parent: value
                        .get("m_influenceNames")?
                        .as_array()?
                        .first()?
                        .as_str()?
                        .to_string(),
                    offsets: vectors(value.get("m_vInfluenceOffsets")?)?,
                    rotations: vectors(value.get("m_vInfluenceRotations")?)?,
                })
            })
            .collect(),
    )
}

fn vectors(v: &Value) -> Option<Vec<Vec<f64>>> {
    v.as_array()?
        .iter()
        .map(|vec| vec.as_array()?.iter().map(Value::as_f64).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_FRAME: [f64; 4] = [-0.5, -0.5, -0.5, 0.5];
    const IDENTITY: [f64; 4] = [0.0, 0.0, 0.0, 1.0];

    fn att(index: usize, name: &str, parent: &str, off: [f64; 3], rot: [f64; 4]) -> Attachment {
        Attachment {
            index,
            name: name.into(),
            parent: parent.into(),
            offsets: vec![off.to_vec()],
            rotations: vec![rot.to_vec()],
        }
    }

    fn base(atts: &[Attachment]) -> HashMap<String, Attachment> {
        atts.iter().map(|a| (a.name.clone(), a.clone())).collect()
    }

    #[test]
    fn gap_is_sign_insensitive() {
        let neg: Vec<f64> = IDENTITY.iter().map(|c| -c).collect();
        assert!(rotation_gap_degrees(&IDENTITY, &neg).unwrap() < 1e-6);
    }

    #[test]
    fn old_frame_is_120_degrees_off() {
        let gap = rotation_gap_degrees(&OLD_FRAME, &IDENTITY).unwrap();
        assert!((gap - 120.0).abs() < 1e-6, "{gap}");
    }

    #[test]
    fn old_frame_pivot_is_stale() {
        let game = base(&[att(
            0,
            "standing_pivot",
            ROOT_BONE,
            [0.0, 0.0, 94.0],
            IDENTITY,
        )]);
        let modded = [att(
            3,
            "standing_pivot",
            ROOT_BONE,
            [0.0, 94.0, 0.0],
            OLD_FRAME,
        )];
        assert_eq!(stale_in(&modded, &game), ["standing_pivot"]);
    }

    #[test]
    fn small_tweak_is_current() {
        let game = base(&[att(0, "aim", ROOT_BONE, [0.0, 0.0, 55.0], IDENTITY)]);
        // 10 degrees of yaw: sin(5deg), cos(5deg).
        let tweak = [0.0, 0.0, 0.087_156, 0.996_195];
        let modded = [att(0, "aim", ROOT_BONE, [0.0, 0.0, 60.0], tweak)];
        assert_eq!(stale_in(&modded, &game), Vec::<String>::new());
    }

    #[test]
    fn other_parents_are_ignored() {
        let game = base(&[att(0, "palm_R", "hand_R", [1.0, 0.0, 0.0], IDENTITY)]);
        let modded = [att(0, "palm_R", "hand_R", [1.0, 0.0, 0.0], OLD_FRAME)];
        assert_eq!(stale_in(&modded, &game), Vec::<String>::new());
        assert!(heal_block(&double_block(&modded), &game).unwrap().is_none());
    }

    #[test]
    fn reparented_attachment_is_left_alone() {
        let game = base(&[att(0, "aim", ROOT_BONE, [0.0, 0.0, 55.0], IDENTITY)]);
        let modded = [att(0, "aim", "spine_2", [0.0, 0.0, 55.0], OLD_FRAME)];
        assert_eq!(stale_in(&modded, &game), Vec::<String>::new());
        assert!(heal_block(&double_block(&modded), &game).unwrap().is_none());
    }

    #[test]
    fn edits_copy_offsets_and_rotations_at_mod_index() {
        let game = att(9, "standing_pivot", ROOT_BONE, [0.0, 0.0, 94.0], IDENTITY);
        let modded = att(1, "standing_pivot", ROOT_BONE, [0.0, 94.0, 0.0], OLD_FRAME);
        let edits = attachment_edits(&modded, &game);
        assert_eq!(edits.len(), 7);
        assert!(edits.iter().all(|(path, _)| path[1] == Seg::Index(1)));
        let values: Vec<f64> = edits.iter().map(|(_, v)| *v).collect();
        assert_eq!(values, [0.0, 0.0, 94.0, 0.0, 0.0, 0.0, 1.0]);
    }

    fn reals(values: &[f64]) -> Value {
        Value::Array(values.iter().copied().map(Value::Double).collect())
    }

    /// An `MDAT`-shaped tree holding `atts` (their `index` must match their
    /// position) plus one unrelated field.
    fn mdat(atts: &[Attachment]) -> Value {
        let vectors = |vs: &[Vec<f64>]| Value::Array(vs.iter().map(|v| reals(v)).collect());
        let list = atts
            .iter()
            .map(|a| {
                let value = Value::Object(vec![
                    (
                        "m_influenceNames".into(),
                        Value::Array(vec![Value::String(a.parent.clone())]),
                    ),
                    ("m_vInfluenceOffsets".into(), vectors(&a.offsets)),
                    ("m_vInfluenceRotations".into(), vectors(&a.rotations)),
                ]);
                Value::Object(vec![
                    ("key".into(), Value::String(a.name.clone())),
                    ("value".into(), value),
                ])
            })
            .collect();
        Value::Object(vec![
            ("m_name".into(), Value::String("kept".into())),
            ("m_attachments".into(), Value::Array(list)),
        ])
    }

    /// Every component a real `DOUBLE`, except 0 and 1, which morphic's encoder
    /// writes as the tagless `DOUBLE_ZERO`/`DOUBLE_ONE`.
    fn double_block(atts: &[Attachment]) -> Vec<u8> {
        morphic::kv3::encode(&mdat(atts), &morphic::kv3::Format([0; 16]))
    }

    /// Every component a real `FLOAT` (the values must be f32-exact and not 0
    /// or 1): the insert encoder picks the narrowest exact tag.
    fn float_block(atts: &[Attachment]) -> Vec<u8> {
        let hollow: Vec<Attachment> = atts
            .iter()
            .map(|a| Attachment {
                offsets: vec![Vec::new(); a.offsets.len()],
                rotations: vec![Vec::new(); a.rotations.len()],
                ..a.clone()
            })
            .collect();
        let mut block = double_block(&hollow);
        for a in atts {
            for (field, vecs) in [
                ("m_vInfluenceOffsets", &a.offsets),
                ("m_vInfluenceRotations", &a.rotations),
            ] {
                for (j, v) in vecs.iter().enumerate() {
                    let path = component(a.index, field, j, 0)[..5].to_vec();
                    for (c, x) in v.iter().enumerate() {
                        block = morphic::kv3::insert_array_element_adding(
                            &block,
                            &path,
                            c,
                            &Value::Double(*x),
                        )
                        .unwrap();
                    }
                }
            }
        }
        block
    }

    fn component(index: usize, field: &str, j: usize, c: usize) -> Vec<Seg> {
        vec![
            Seg::Key("m_attachments".into()),
            Seg::Index(index),
            Seg::Key("value".into()),
            Seg::Key(field.into()),
            Seg::Index(j),
            Seg::Index(c),
        ]
    }

    fn game_pivot() -> HashMap<String, Attachment> {
        base(&[att(
            7,
            "standing_pivot",
            ROOT_BONE,
            [0.0, 0.0, 94.0],
            IDENTITY,
        )])
    }

    /// Heals `block` against [`game_pivot`] and checks the result decodes to
    /// `modded` with the pivot carrying the game's values, at the same size.
    fn assert_heals(block: &[u8], modded: &[Attachment]) {
        let (healed, names) = heal_block(block, &game_pivot()).unwrap().unwrap();
        assert_eq!(names, ["standing_pivot"]);
        assert_eq!(healed.len(), block.len(), "patched in place");
        let mut want = modded.to_vec();
        want[1].offsets = vec![vec![0.0, 0.0, 94.0]];
        want[1].rotations = vec![IDENTITY.to_vec()];
        assert_eq!(morphic::kv3::decode(&healed).unwrap(), mdat(&want));
    }

    #[test]
    fn heals_f32_attachments_in_place() {
        let modded = [
            att(0, "head_fx", "head", [1.5, 2.5, 3.5], OLD_FRAME),
            att(1, "standing_pivot", ROOT_BONE, [2.5, 94.0, -3.0], OLD_FRAME),
        ];
        let block = float_block(&modded);
        let path = component(1, "m_vInfluenceOffsets", 0, 0);
        assert!(morphic::kv3::set_floats(&block, &[(path, 0.0)]).is_ok());
        assert_heals(&block, &modded);
    }

    #[test]
    fn heals_f64_attachments_in_place() {
        let modded = [
            att(0, "head_fx", "head", [1.5, 2.5, 3.5], OLD_FRAME),
            att(1, "standing_pivot", ROOT_BONE, [2.5, 94.0, -3.0], OLD_FRAME),
        ];
        let block = double_block(&modded);
        let path = component(1, "m_vInfluenceOffsets", 0, 0);
        assert!(morphic::kv3::set_floats(&block, &[(path.clone(), 0.0)]).is_err());
        assert!(morphic::kv3::set_doubles(&block, &[(path, 0.0)]).is_ok());
        assert_heals(&block, &modded);
    }

    #[test]
    fn heals_tagless_zero_and_one_by_flipping_them() {
        // Offsets: tagless 0, a real double, tagless 1. Rotation: all tagless.
        let modded = [
            att(0, "head_fx", "head", [1.5, 2.5, 3.5], OLD_FRAME),
            att(
                1,
                "standing_pivot",
                ROOT_BONE,
                [0.0, 7.5, 1.0],
                [1.0, 0.0, 0.0, 0.0],
            ),
        ];
        let game = base(&[att(
            7,
            "standing_pivot",
            ROOT_BONE,
            [1.0, 0.0, 0.0],
            IDENTITY,
        )]);
        let block = double_block(&modded);
        let (healed, _) = heal_block(&block, &game).unwrap().unwrap();
        assert_eq!(healed.len(), block.len(), "patched in place");
        let mut want = modded.to_vec();
        want[1].offsets = vec![vec![1.0, 0.0, 0.0]];
        want[1].rotations = vec![IDENTITY.to_vec()];
        assert_eq!(morphic::kv3::decode(&healed).unwrap(), mdat(&want));
    }

    #[test]
    fn tagless_component_needing_stored_bytes_fails_loudly() {
        // The pivot's z offset is a tagless 0 that would have to become 94.
        let modded = [
            att(0, "head_fx", "head", [1.5, 2.5, 3.5], OLD_FRAME),
            att(1, "standing_pivot", ROOT_BONE, [0.0, 94.0, 0.0], OLD_FRAME),
        ];
        let err = heal_block(&double_block(&modded), &game_pivot()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("standing_pivot"), "{msg}");
        assert!(msg.contains("0.0 or 1.0"), "{msg}");
    }

    #[test]
    fn reframes_extra_wolf_poses_after_the_pivot_was_already_fixed() {
        let game = game_pivot();
        let half = 0.5_f64.sqrt();
        for (name, rotation, height) in [
            ("far_01", [0.0, 0.0, -half, half], 178.0),
            ("far_02", [half, half, 0.0, 0.0], 14.0),
        ] {
            let old = att(1, name, ROOT_BONE, [-50.0, height, 25.0], rotation);
            assert_eq!(stale_in(std::slice::from_ref(&old), &game), [name]);
            let updated = camera::reframe_extra(&old, &game).unwrap();
            assert_eq!(updated.offsets[0], [25.0, -50.0, height]);
            assert!(camera::reframe_extra(&updated, &game).is_none());
            assert!(stale_in(&[updated], &game).is_empty());
        }
    }

    #[test]
    fn extra_pose_detection_preserves_current_custom_and_non_camera_attachments() {
        let game = game_pivot();
        let half = std::f64::consts::FRAC_1_SQRT_2;
        for (name, parent, rotation) in [
            ("far_01", ROOT_BONE, [0.0, half, 0.0, half]),
            ("far_01", ROOT_BONE, [0.0, 0.0, 0.087_156, 0.996_195]),
            ("custom_camera", ROOT_BONE, OLD_FRAME),
            ("far_01", "head", OLD_FRAME),
        ] {
            let authored = att(0, name, parent, [1.0, 2.0, 3.0], rotation);
            assert!(camera::reframe_extra(&authored, &game).is_none());
        }
    }

    fn camera_model(atts: &[Attachment], settings: bool) -> Vec<u8> {
        let mut mesh = mdat(atts);
        if let Value::Object(ref mut fields) = mesh {
            fields.push((
                "m_skeleton".into(),
                Value::Object(vec![(
                    "m_bones".into(),
                    Value::Array(vec![Value::Object(vec![(
                        "m_boneName".into(),
                        Value::String(ROOT_BONE.into()),
                    )])]),
                )]),
            ));
        }
        let binding = if settings {
            "CitadelCameraSettings_t = { m_flCameraHeightStanding = 60.0 }"
        } else {
            ""
        };
        let data = Value::Object(vec![(
            "m_modelInfo".into(),
            Value::Object(vec![(
                "m_keyValueText".into(),
                Value::String(format!(
                    "{} {{ custom = 17 {binding} }}",
                    crate::model_metadata::HEADER
                )),
            )]),
        )]);
        let data = morphic::kv3::encode(&data, &morphic::kv3::Format([0; 16]));
        let mesh = morphic::kv3::encode(&mesh, &morphic::kv3::Format([0; 16]));
        let payloads: [(&[u8; 4], &[u8]); 3] = [
            (b"DATA", &data),
            (b"MDAT", &mesh),
            (b"MVTX", b"geometry-kept"),
        ];
        let mut bytes = vec![0; 16 + payloads.len() * 12];
        bytes[4..6].copy_from_slice(&12_u16.to_le_bytes());
        bytes[8..12].copy_from_slice(&8_u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&u32::try_from(payloads.len()).unwrap().to_le_bytes());
        for (i, (kind, payload)) in payloads.iter().enumerate() {
            let row = 16 + i * 12;
            bytes[row..row + 4].copy_from_slice(*kind);
            let offset = u32::try_from(bytes.len() - row - 4).unwrap();
            bytes[row + 4..row + 8].copy_from_slice(&offset.to_le_bytes());
            bytes[row + 8..row + 12]
                .copy_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
            bytes.extend_from_slice(payload);
        }
        let len = u32::try_from(bytes.len()).unwrap();
        bytes[..4].copy_from_slice(&len.to_le_bytes());
        bytes
    }

    #[test]
    fn restores_missing_cat_points_and_settings_as_one_repair() {
        let stock = camera_model(
            &[
                att(0, "standing_pivot", ROOT_BONE, [0.0, 0.0, 60.0], IDENTITY),
                att(1, "near_00", ROOT_BONE, [-28.0, -72.0, 60.0], IDENTITY),
            ],
            true,
        );
        let template = GameModel {
            root: root_attachments(&stock).unwrap(),
            camera: camera::Template::read(&stock).unwrap(),
        };
        let source = camera_model(&[att(0, "carry", "head", [1.0, 2.0, 3.0], IDENTITY)], false);
        let before = Resource::parse(&source).unwrap();
        let issues = model_issues(&source, &template).unwrap();
        assert_eq!(issues, ["standing_pivot", "near_00", camera::SETTINGS]);
        let (healed, report) = heal_model("cat.vmdl_c", &source, &template).unwrap();
        assert_eq!(report.attachments, issues);
        assert!(model_issues(&healed, &template).unwrap().is_empty());
        let after = Resource::parse(&healed).unwrap();
        assert_eq!(before.find_block(*b"MVTX"), after.find_block(*b"MVTX"));
        let old_mesh = morphic::kv3::decode(before.find_block(*b"MDAT").unwrap()).unwrap();
        let new_mesh = morphic::kv3::decode(after.find_block(*b"MDAT").unwrap()).unwrap();
        assert_eq!(
            old_mesh.get("m_attachments").unwrap().as_array().unwrap()[0],
            new_mesh.get("m_attachments").unwrap().as_array().unwrap()[0]
        );
    }

    #[test]
    fn settings_only_repair_preserves_an_authored_camera_block() {
        let atts = [att(
            0,
            "standing_pivot",
            ROOT_BONE,
            [0.0, 0.0, 60.0],
            IDENTITY,
        )];
        let stock = camera_model(&atts, true);
        let template = GameModel {
            root: root_attachments(&stock).unwrap(),
            camera: camera::Template::read(&stock).unwrap(),
        };
        let partial = camera_model(&atts, false);
        assert_eq!(
            model_issues(&partial, &template).unwrap(),
            [camera::SETTINGS]
        );
        let (healed, _) = heal_model("cat.vmdl_c", &partial, &template).unwrap();
        assert!(model_issues(&healed, &template).unwrap().is_empty());
        assert!(template.camera.restore_settings(&stock).unwrap().is_none());
    }
}
