//! Per-material `.vmat_c` shader-parameter styling.
//!
//! Deadlock's hero shader (`pbr.vfx`) exposes a large per-material vocabulary:
//! the NPR/toon controls (solid outlines, unlit) and the PBR/specular side
//! (sheen, glass, translucency) are all plain `m_intParams` / `m_floatParams` /
//! `m_vectorParams` data in the compiled material. This module sets or inserts
//! those params byte-faithfully (no full KV3 re-encode; same discipline as the
//! particle and scroll patches) and packs the result into an addon VPK.
//!
//! `TextureXxx1` vector params double as flat constant fallbacks when no
//! texture is bound to the matching sampler, so looks like sheen can be enabled
//! with constants alone: Valve's own `xmas_vindicta_dress.vmat_c` is the model
//! for the gem preset. Survey + background: `docs/spike-npr-toon-shading.md`.

use std::path::Path;

use anyhow::{Context, Result};
use morphic::kv3::{Seg, Value};

use crate::trippy::{hero_path_match, is_weapon_path, open_vpks, read_entry};

/// One set-or-insert edit against a compiled material's parameter tables.
#[derive(Debug, Clone, PartialEq)]
pub enum VmatEdit {
    /// `m_intParams` entry (feature flags like `F_SHEEN` and int/bool knobs).
    Int { name: String, value: i64 },
    /// `m_floatParams` entry.
    Float { name: String, value: f64 },
    /// `m_vectorParams` entry (RGBA; pass 0 for unused lanes).
    Vector { name: String, value: [f64; 4] },
    /// `m_intAttributes` entry. This is the speculative path for shader
    /// variables declared as `__Attribute__` in the VCS.
    IntAttribute { name: String, value: i64 },
    /// `m_floatAttributes` entry. This is the speculative path for shader
    /// variables declared as `__Attribute__` in the VCS.
    FloatAttribute { name: String, value: f64 },
    /// `m_vectorAttributes` entry. This is the speculative path for shader
    /// variables declared as `__Attribute__` in the VCS.
    VectorAttribute { name: String, value: [f64; 4] },
    /// `m_dynamicParams` entry: a compiled dynamic expression
    /// ([`morphic::vfx_expr::compile`]) driving the param per frame.
    /// `attributes` lists every render attribute the expression reads; they
    /// are registered in the material's `m_renderAttributesUsed` so the
    /// engine feeds them (Valve does the same: a shipped material using
    /// `$ent_age` carries it there).
    Expr {
        name: String,
        bytecode: Vec<u8>,
        attributes: Vec<String>,
    },
    /// In-place edit of an *existing* `m_dynamicParams` expression: decompile
    /// the current bytecode, replace every occurrence of `find` with `replace`
    /// in the source, then recompile. Resolved to [`VmatEdit::Expr`] per target
    /// material (it depends on that material's current expression) at the top of
    /// [`patch_vmat_params`]; the other methods never see this variant.
    EditExpr {
        name: String,
        find: String,
        replace: String,
    },
}

impl VmatEdit {
    /// Builds an `Expr` edit by compiling `src`.
    ///
    /// # Errors
    /// Fails when the expression does not compile.
    pub fn expr(name: impl Into<String>, src: &str) -> Result<Self> {
        let compiled = morphic::vfx_expr::compile(src)
            .map_err(|e| anyhow::anyhow!("compiling expression for {src}: {e}"))?;
        Ok(Self::Expr {
            name: name.into(),
            bytecode: compiled.bytecode,
            attributes: compiled.attributes,
        })
    }

    fn table(&self) -> &'static str {
        match self {
            Self::Int { .. } => "m_intParams",
            Self::Float { .. } => "m_floatParams",
            Self::Vector { .. } => "m_vectorParams",
            Self::IntAttribute { .. } => "m_intAttributes",
            Self::FloatAttribute { .. } => "m_floatAttributes",
            Self::VectorAttribute { .. } => "m_vectorAttributes",
            Self::Expr { .. } | Self::EditExpr { .. } => "m_dynamicParams",
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Int { name, .. }
            | Self::Float { name, .. }
            | Self::Vector { name, .. }
            | Self::IntAttribute { name, .. }
            | Self::FloatAttribute { name, .. }
            | Self::VectorAttribute { name, .. }
            | Self::Expr { name, .. }
            | Self::EditExpr { name, .. } => name,
        }
    }

    fn is_material_attribute(&self) -> bool {
        matches!(
            self,
            Self::IntAttribute { .. } | Self::FloatAttribute { .. } | Self::VectorAttribute { .. }
        )
    }

    fn as_object(&self) -> Value {
        let (key, value) = match self {
            Self::Int { value, .. } | Self::IntAttribute { value, .. } => {
                ("m_nValue", Value::Int(*value))
            }
            Self::Float { value, .. } | Self::FloatAttribute { value, .. } => {
                ("m_flValue", Value::Double(*value))
            }
            Self::Vector { value, .. } | Self::VectorAttribute { value, .. } => (
                "m_value",
                Value::Array(value.iter().map(|&c| Value::Double(c)).collect()),
            ),
            Self::Expr { bytecode, .. } => ("m_value", Value::Binary(bytecode.clone())),
            Self::EditExpr { .. } => unreachable!("EditExpr is resolved to Expr before as_object"),
        };
        Value::Object(vec![
            ("m_name".to_string(), Value::String(self.name().to_string())),
            (key.to_string(), value),
        ])
    }
}

/// Per-material outcome counters for [`patch_vmat_params`].
#[derive(Debug, Clone, Default)]
pub struct VmatPatchStats {
    /// Params that already existed and were value-patched.
    pub set: usize,
    /// Params inserted into their table.
    pub inserted: usize,
    /// Param names that could not be applied (e.g. blob-section materials
    /// refuse structural inserts).
    pub failed: Vec<String>,
}

fn param_index(root: &Value, table: &str, name: &str) -> Option<usize> {
    root.get(table)?
        .as_array()?
        .iter()
        .position(|p| p.get("m_name").and_then(Value::as_str) == Some(name))
}

/// Does the decoded param at `table[i]` already hold this edit's value?
fn already_applied(root: &Value, i: usize, edit: &VmatEdit) -> bool {
    let Some(param) = root
        .get(edit.table())
        .and_then(Value::as_array)
        .and_then(|a| a.get(i))
    else {
        return false;
    };
    match edit {
        VmatEdit::Int { value, .. } | VmatEdit::IntAttribute { value, .. } => {
            param.get("m_nValue").and_then(Value::as_int) == Some(*value)
        }
        VmatEdit::Float { value, .. } | VmatEdit::FloatAttribute { value, .. } => {
            param.get("m_flValue").and_then(Value::as_f64) == Some(*value)
        }
        VmatEdit::Vector { value, .. } | VmatEdit::VectorAttribute { value, .. } => param
            .get("m_value")
            .and_then(Value::as_array)
            .is_some_and(|a| {
                a.len() == 4 && a.iter().zip(value).all(|(c, w)| c.as_f64() == Some(*w))
            }),
        VmatEdit::Expr { bytecode, .. } => {
            matches!(param.get("m_value"), Some(Value::Binary(b)) if b == bytecode)
        }
        VmatEdit::EditExpr { .. } => {
            unreachable!("EditExpr is resolved to Expr before already_applied")
        }
    }
}

/// Applies one edit byte-faithfully: in-place set when the param exists,
/// structural insert when it does not.
fn apply_in_place(
    working: &[u8],
    root: &Value,
    edit: &VmatEdit,
) -> std::result::Result<(Vec<u8>, bool), morphic::DecodeError> {
    let table = edit.table();
    if let Some(i) = param_index(root, table, edit.name()) {
        let bytes = match edit {
            VmatEdit::Int { value, .. } | VmatEdit::IntAttribute { value, .. } => {
                morphic::patch_kv3_resource_scalars(
                    working,
                    &[(
                        vec![
                            Seg::Key(table.to_string()),
                            Seg::Index(i),
                            Seg::Key("m_nValue".to_string()),
                        ],
                        *value,
                    )],
                )?
            }
            VmatEdit::Float { value, .. } | VmatEdit::FloatAttribute { value, .. } => {
                morphic::patch_kv3_resource_doubles(
                    working,
                    &[(
                        vec![
                            Seg::Key(table.to_string()),
                            Seg::Index(i),
                            Seg::Key("m_flValue".to_string()),
                        ],
                        *value,
                    )],
                )?
            }
            VmatEdit::Vector { value, .. } | VmatEdit::VectorAttribute { value, .. } => {
                let edits: Vec<(Vec<Seg>, f64)> = value
                    .iter()
                    .enumerate()
                    .map(|(k, &c)| {
                        (
                            vec![
                                Seg::Key(table.to_string()),
                                Seg::Index(i),
                                Seg::Key("m_value".to_string()),
                                Seg::Index(k),
                            ],
                            c,
                        )
                    })
                    .collect();
                morphic::patch_kv3_resource_doubles(working, &edits)?
            }
            // a different-length bytecode blob cannot be patched in place;
            // route to the re-encode fallback
            VmatEdit::Expr { .. } => {
                return Err(morphic::DecodeError::Kv3(
                    "existing dynamic param needs a re-encode",
                ))
            }
            VmatEdit::EditExpr { .. } => {
                unreachable!("EditExpr is resolved to Expr before apply_in_place")
            }
        };
        Ok((bytes, false))
    } else {
        let bytes = morphic::patch_kv3_resource_array_insert(
            working,
            &[Seg::Key(table.to_string())],
            0,
            &edit.as_object(),
        )?;
        Ok((bytes, true))
    }
}

/// Applies one edit to the decoded tree (the re-encode fallback path).
fn apply_to_tree(tree: &mut Value, edit: &VmatEdit) -> bool {
    let exists = param_index(tree, edit.table(), edit.name());
    let Some(Value::Array(params)) = tree.get_mut(edit.table()) else {
        return false;
    };
    match exists {
        Some(i) => {
            let value_key = match edit {
                VmatEdit::Int { .. } | VmatEdit::IntAttribute { .. } => "m_nValue",
                VmatEdit::Float { .. } | VmatEdit::FloatAttribute { .. } => "m_flValue",
                VmatEdit::Vector { .. }
                | VmatEdit::VectorAttribute { .. }
                | VmatEdit::Expr { .. } => "m_value",
                VmatEdit::EditExpr { .. } => {
                    unreachable!("EditExpr is resolved to Expr before apply_to_tree")
                }
            };
            let Some(slot) = params[i].get_mut(value_key) else {
                return false;
            };
            *slot = match edit {
                VmatEdit::Int { value, .. } | VmatEdit::IntAttribute { value, .. } => {
                    Value::Int(*value)
                }
                VmatEdit::Float { value, .. } | VmatEdit::FloatAttribute { value, .. } => {
                    Value::Double(*value)
                }
                VmatEdit::Vector { value, .. } | VmatEdit::VectorAttribute { value, .. } => {
                    Value::Array(value.iter().map(|&c| Value::Double(c)).collect())
                }
                VmatEdit::Expr { bytecode, .. } => Value::Binary(bytecode.clone()),
                VmatEdit::EditExpr { .. } => {
                    unreachable!("EditExpr is resolved to Expr before apply_to_tree")
                }
            };
        }
        None => params.push(edit.as_object()),
    }
    true
}

/// The attribute names a material declares in `m_renderAttributesUsed` (the
/// dictionary that lets [`morphic::vfx_expr::decompile`] recover `$names` from
/// their hashed tokens).
fn render_attrs(root: &Value) -> Vec<String> {
    root.get("m_renderAttributesUsed")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Replace an existing dynamic-expression blob in a blob-bearing `.vmat_c`
/// byte-faithfully (content-keyed on the current bytecode), keeping the block
/// compressed. Returns `None` for a non-`Expr` edit (caller reports it failed),
/// `Some(Ok(bytes))` on success, or `Some(Err(msg))` if the param/blob could not
/// be located or the blob splice failed. Built so `--edit-expr` /
/// `--set-expr`-over-an-existing-param stop being refused on blobbed materials.
fn replace_expr_blob(
    working: &[u8],
    edit: &VmatEdit,
) -> Option<std::result::Result<Vec<u8>, String>> {
    let VmatEdit::Expr { name, bytecode, .. } = edit else {
        return None;
    };
    let root = match morphic::decode_kv3_resource(working) {
        Ok(r) => r,
        Err(e) => return Some(Err(format!("{name}: decode failed ({e})"))),
    };
    // The current bytecode is the blob to replace (content-keyed). It is unique
    // among the material's blobs, so the swap is unambiguous even with several
    // dynamic expressions present.
    let old = (|| {
        let i = param_index(&root, "m_dynamicParams", name)?;
        root.get("m_dynamicParams")
            .and_then(Value::as_array)
            .and_then(|a| a.get(i))
            .and_then(|p| p.get("m_value"))
            .and_then(expr_bytes)
    })();
    let Some(old) = old else {
        return Some(Err(format!(
            "{name}: no existing dynamic expression bytecode to replace"
        )));
    };
    if old == *bytecode {
        // Recompiled to the same bytes; nothing to do (counted as set upstream).
        return Some(Ok(working.to_vec()));
    }
    match morphic::patch_kv3_resource_blob(working, &old, bytecode) {
        Ok(bytes) => Some(Ok(bytes)),
        Err(e) => Some(Err(format!("{name}: blob-aware replace failed ({e})"))),
    }
}

/// Resolves a [`VmatEdit::EditExpr`] against `root` (one target material):
/// decompile the named `m_dynamicParams` expression, substitute `find`->
/// `replace` in the source, recompile. Returns the equivalent
/// [`VmatEdit::Expr`], or an error string describing why it could not apply.
fn resolve_edit_expr(
    root: &Value,
    name: &str,
    find: &str,
    replace: &str,
) -> std::result::Result<VmatEdit, String> {
    let idx = param_index(root, "m_dynamicParams", name)
        .ok_or_else(|| format!("{name}: no existing dynamic expression to edit"))?;
    let bytes = root
        .get("m_dynamicParams")
        .and_then(Value::as_array)
        .and_then(|a| a.get(idx))
        .and_then(|p| p.get("m_value"))
        .and_then(expr_bytes)
        .ok_or_else(|| format!("{name}: dynamic param has no readable bytecode"))?;
    let attrs = render_attrs(root);
    let src = morphic::vfx_expr::decompile(&bytes, &attrs)
        .map_err(|e| format!("{name}: decompile failed ({e})"))?;
    if !src.contains(find) {
        return Err(format!(
            "{name}: {find:?} not found in current expression {src:?}"
        ));
    }
    let new_src = src.replace(find, replace);
    let compiled = morphic::vfx_expr::compile(&new_src)
        .map_err(|e| format!("{name}: recompiled {new_src:?} did not parse ({e})"))?;
    Ok(VmatEdit::Expr {
        name: name.to_string(),
        bytecode: compiled.bytecode,
        attributes: compiled.attributes,
    })
}

/// Converts every [`VmatEdit::EditExpr`] in `edits` into a concrete
/// [`VmatEdit::Expr`] resolved against `bytes` (one material), passing other
/// edits through unchanged. A resolution that fails (no such expression, `find`
/// absent, recompile error) is recorded in `stats.failed` and dropped.
fn resolve_in_place_edits(
    bytes: &[u8],
    edits: &[VmatEdit],
    stats: &mut VmatPatchStats,
) -> Result<Vec<VmatEdit>> {
    let mut out = Vec::with_capacity(edits.len());
    let mut root = None;
    for edit in edits {
        match edit {
            VmatEdit::EditExpr {
                name,
                find,
                replace,
            } => {
                let tree = match &root {
                    Some(t) => t,
                    None => root.insert(
                        morphic::decode_kv3_resource(bytes)
                            .context("decoding material KV3 to edit an expression")?,
                    ),
                };
                match resolve_edit_expr(tree, name, find, replace) {
                    Ok(resolved) => out.push(resolved),
                    Err(msg) => stats.failed.push(msg),
                }
            }
            other => out.push(other.clone()),
        }
    }
    Ok(out)
}

/// Applies `edits` to a compiled `.vmat_c`, setting existing params and
/// inserting missing ones.
///
/// [`VmatEdit::EditExpr`] edits are resolved first against this material's
/// current expressions (decompile -> substitute -> recompile) into ordinary
/// `Expr` edits; one that has no matching expression or whose substitution
/// does not recompile is reported in [`VmatPatchStats::failed`] and skipped.
///
/// Byte-faithful in-place patching is tried first. A tagless stored value (a
/// 0/1 encoded with no data bytes) cannot be patched in place; in that case a
/// non-blobbed material is fully re-encoded ([`morphic::encode_kv3_resource`],
/// which preserves the texture dependency blocks) with all edits applied to
/// the tree. The same discipline as `hero_recolor`'s tint stamping. A blobbed
/// material that needs the fallback reports the edit in
/// [`VmatPatchStats::failed`] instead (a re-encode would mangle its blob
/// framing).
///
/// # Errors
/// Fails only if the bytes are not a decodable KV3 resource.
#[allow(clippy::too_many_lines)]
pub fn patch_vmat_params(bytes: &[u8], edits: &[VmatEdit]) -> Result<(Vec<u8>, VmatPatchStats)> {
    let mut working = bytes.to_vec();
    let mut stats = VmatPatchStats::default();
    let mut needs_reencode = Vec::new();

    // Resolve any in-place expression edits against this material's *current*
    // expressions first; they become ordinary `Expr` edits (or are recorded as
    // failed and dropped), so the rest of the pipeline never sees `EditExpr`.
    let resolved = resolve_in_place_edits(&working, edits, &mut stats)?;
    let edits = &resolved;

    for edit in edits {
        let root = morphic::decode_kv3_resource(&working)
            .context("decoding material KV3 for parameter lookup")?;
        if let Some(i) = param_index(&root, edit.table(), edit.name()) {
            if already_applied(&root, i, edit) {
                stats.set += 1;
                continue;
            }
        }
        match apply_in_place(&working, &root, edit) {
            Ok((bytes, was_insert)) => {
                working = bytes;
                if was_insert {
                    stats.inserted += 1;
                } else {
                    stats.set += 1;
                }
            }
            Err(_) => needs_reencode.push(edit.clone()),
        }
    }

    // Every render attribute an expression edit reads must end up in the
    // material's m_renderAttributesUsed so the engine feeds the values
    // (shipped materials register their attributes the same way).
    let mut pending_attrs: Vec<&str> = Vec::new();
    for edit in edits {
        if let VmatEdit::Expr { attributes, .. } = edit {
            if stats.failed.iter().any(|f| f == edit.name()) {
                continue;
            }
            for a in attributes {
                if !pending_attrs.contains(&a.as_str()) {
                    pending_attrs.push(a);
                }
            }
        }
        if edit.is_material_attribute() && !pending_attrs.contains(&edit.name()) {
            pending_attrs.push(edit.name());
        }
    }

    if !needs_reencode.is_empty() {
        if morphic::kv3_resource_has_blobs(&working).unwrap_or(true) {
            // A blob-bearing material refuses the re-encode fallback. Changing an
            // *existing* dynamic expression lands here: its bytecode IS a binary
            // blob. Replace that blob byte-faithfully (content-keyed on the current
            // bytecode), keeping the block compressed, instead of re-encoding the
            // whole material. Any edit that is not such an Expr swap still fails.
            let mut still_failed = Vec::new();
            for e in &needs_reencode {
                match replace_expr_blob(&working, e) {
                    Some(Ok(bytes)) => {
                        working = bytes;
                        stats.set += 1;
                    }
                    Some(Err(msg)) => still_failed.push(msg),
                    None => still_failed.push(e.name().to_string()),
                }
            }
            stats.failed.extend(still_failed);
        } else {
            let mut tree = morphic::decode_kv3_resource(&working)
                .context("decoding material KV3 for re-encode fallback")?;
            for edit in &needs_reencode {
                if apply_to_tree(&mut tree, edit) {
                    stats.set += 1;
                } else {
                    stats.failed.push(edit.name().to_string());
                }
            }
            // ride the same re-encode for attribute registration
            for attr in pending_attrs.drain(..) {
                if attribute_edit_failed(&stats, attr) {
                    continue;
                }
                register_attribute_in_tree(&mut tree, attr);
            }
            working = morphic::encode_kv3_resource(&working, &tree)
                .context("re-encoding material to promote tagless params")?;
        }
    }

    // No re-encode happened: register byte-faithfully via structural insert.
    if !pending_attrs.is_empty() {
        let root = morphic::decode_kv3_resource(&working)
            .context("decoding material KV3 for attribute registration")?;
        let registered: Vec<String> = root
            .get("m_renderAttributesUsed")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_ascii_lowercase))
                    .collect()
            })
            .unwrap_or_default();
        for attr in pending_attrs {
            if attribute_edit_failed(&stats, attr) {
                continue;
            }
            if registered.iter().any(|r| r == attr) {
                continue;
            }
            match morphic::patch_kv3_resource_array_insert(
                &working,
                &[Seg::Key("m_renderAttributesUsed".to_string())],
                0,
                &Value::String(attr.to_string()),
            ) {
                Ok(bytes) => working = bytes,
                Err(_) => stats.failed.push(format!("m_renderAttributesUsed:{attr}")),
            }
        }
    }

    Ok((working, stats))
}

/// Adds `attr` to the tree's `m_renderAttributesUsed` unless already present.
fn register_attribute_in_tree(tree: &mut Value, attr: &str) {
    let Some(Value::Array(list)) = tree.get_mut("m_renderAttributesUsed") else {
        return;
    };
    let present = list
        .iter()
        .any(|v| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case(attr)));
    if !present {
        list.push(Value::String(attr.to_string()));
    }
}

fn attribute_edit_failed(stats: &VmatPatchStats, attr: &str) -> bool {
    stats.failed.iter().any(|f| {
        f == attr
            || f.strip_prefix(attr)
                .is_some_and(|rest| rest.starts_with(':'))
    })
}

/// Curated parameter bundles for common looks. All values are modeled on
/// shipped Valve materials so the engine demonstrably accepts them (see the
/// spike doc for the survey).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmatPreset {
    /// Gemstone sheen: `F_SHEEN` with constant sheen color + low sheen
    /// roughness (recipe: `xmas_vindicta_dress.vmat_c`). `tint` colors the
    /// sheen lobe.
    Gem,
    /// Glassy specular coat: `F_GLASS` with a full constant mask (recipe:
    /// `viscous_body.vmat_c`, minus its bound mask texture).
    Glass,
    /// Drop the hero NPR lighting path entirely: full PBR response, real
    /// environment reflections.
    Pbr,
    /// Fully unlit (lighting ignored; albedo as authored).
    Unlit,
    /// Thick solid-color outline (`tint` is the ink color).
    Ink,
}

impl VmatPreset {
    /// Parse a CLI preset name.
    ///
    /// # Errors
    /// Fails on an unknown name, listing the valid ones.
    pub fn from_name(name: &str) -> Result<Self> {
        match name.to_ascii_lowercase().as_str() {
            "gem" | "sheen" => Ok(Self::Gem),
            "glass" => Ok(Self::Glass),
            "pbr" | "no-npr" => Ok(Self::Pbr),
            "unlit" => Ok(Self::Unlit),
            "ink" | "outline" => Ok(Self::Ink),
            other => anyhow::bail!("unknown preset {other:?} (gem, glass, pbr, unlit, ink)"),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gem => "gem",
            Self::Glass => "glass",
            Self::Pbr => "pbr",
            Self::Unlit => "unlit",
            Self::Ink => "ink",
        }
    }

    /// The edit bundle for this preset. `tint` is linear RGB 0..=1 where the
    /// preset has a color (gem sheen color, ink outline color).
    #[must_use]
    pub fn edits(self, tint: Option<[f64; 3]>) -> Vec<VmatEdit> {
        let int = |name: &str, value: i64| VmatEdit::Int {
            name: name.to_string(),
            value,
        };
        let float = |name: &str, value: f64| VmatEdit::Float {
            name: name.to_string(),
            value,
        };
        let vec3 = |name: &str, [r, g, b]: [f64; 3]| VmatEdit::Vector {
            name: name.to_string(),
            value: [r, g, b, 0.0],
        };
        match self {
            Self::Gem => {
                // Icy blue like Valve's snow dress unless the caller tints.
                let sheen = tint.unwrap_or([0.67, 0.76, 1.0]);
                vec![
                    int("F_SHEEN", 1),
                    int("g_bSheenMaskColorTint1", 0),
                    int("g_bSheenMaskVertexColorTint1", 1),
                    int("g_nSheenTextureColorTintMode1", 1),
                    float("g_fSheenVertexColorStrength1", 1.0),
                    vec3("g_vSheenColorTint1", sheen),
                    // Constant fallback for the unbound sheen-roughness
                    // sampler. Lower = tighter, more crystalline highlight
                    // (Valve ships ~0.5).
                    vec3("TextureSheenRoughness1", [0.25, 0.25, 0.25]),
                ]
            }
            Self::Glass => vec![
                int("F_GLASS", 1),
                vec3("TextureGlassMask1", [1.0, 1.0, 1.0]),
            ],
            Self::Pbr => vec![int("F_USE_NPR_LIGHTING", 0)],
            Self::Unlit => vec![int("F_UNLIT", 1)],
            Self::Ink => {
                let ink = tint.unwrap_or([1.0, 1.0, 1.0]);
                vec![
                    int("F_SOLID_COLOR_OUTLINE", 1),
                    int("F_OVERRIDE_NPR_OUTLINE", 1),
                    float("g_flOverrideNprOutlineThickness", 3.0),
                    vec3("g_vSolidOutlineTint", ink),
                ]
            }
        }
    }
}

/// Which materials a styling run targets.
#[derive(Debug, Clone)]
pub enum VmatTargets {
    /// Discover every `models/heroes*` material for a hero codename.
    Hero {
        codename: String,
        include_body: bool,
        include_weapons: bool,
    },
    /// Explicit `.vmat_c` entry paths.
    Entries(Vec<String>),
}

/// Summary returned by [`style_materials_to_addon`].
#[derive(Debug, Clone, Default)]
pub struct VmatStyleReport {
    pub materials_patched: usize,
    pub params_set: usize,
    pub params_inserted: usize,
    /// Materials skipped because their shader is not `pbr.vfx`.
    pub skipped_non_pbr: usize,
    pub skipped_unreadable: usize,
    /// `(entry, param)` pairs the byte-faithful patcher refused.
    pub failed_params: Vec<(String, String)>,
}

/// A discovered material and the bits of it relevant to styling decisions.
#[derive(Debug, Clone)]
pub struct VmatInfo {
    pub entry: String,
    pub shader: String,
    /// Nonzero `F_*` feature flags.
    pub flags: Vec<(String, i64)>,
    /// Bound texture samplers (`g_t*` name -> resource path).
    pub textures: Vec<(String, String)>,
    /// Per-frame dynamic expressions decompiled from `m_dynamicParams` and
    /// `m_dynamicTextureParams` (param name -> source). A blob that fails to
    /// decompile is reported as `<error: ...>` rather than dropped.
    pub expressions: Vec<(String, String)>,
    /// Scalar `m_floatParams` (`m_name` -> `m_flValue`).
    pub floats: Vec<(String, f64)>,
    /// `m_vectorParams` (`m_name` -> `m_value` lanes).
    pub vectors: Vec<(String, Vec<f64>)>,
    /// Material attributes (`m_*Attributes`) exposed to shader `__Attribute__`
    /// variables, formatted for the CLI list view.
    pub attributes: Vec<(String, String)>,
    /// Carries a per-frame expression blob / live logic
    /// (`kv3_resource_has_blobs`).
    pub dynamic: bool,
}

/// Pull the raw expression bytecode out of a `m_value` node, whether morphic
/// decoded it as a binary blob or as a typed byte array.
fn expr_bytes(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Binary(b) => Some(b.clone()),
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_int().and_then(|n| u8::try_from(n).ok()))
            .collect(),
        _ => None,
    }
}

/// Decompile every dynamic-expression param in `root`, using the material's
/// `m_renderAttributesUsed` to recover attribute names from their hashes.
fn material_expressions(root: &Value) -> Vec<(String, String)> {
    let attrs: Vec<String> = root
        .get("m_renderAttributesUsed")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut out = Vec::new();
    for table in ["m_dynamicParams", "m_dynamicTextureParams"] {
        let Some(Value::Array(params)) = root.get(table) else {
            continue;
        };
        for p in params {
            let Some(name) = p.get("m_name").and_then(Value::as_str) else {
                continue;
            };
            let Some(bytes) = p.get("m_value").and_then(expr_bytes) else {
                continue;
            };
            let src = morphic::vfx_expr::decompile(&bytes, &attrs)
                .unwrap_or_else(|e| format!("<error: {e}>"));
            out.push((name.to_string(), src));
        }
    }
    out
}

fn discover_legacy_materials(vpks: &[valve_pak::VPK], targets: &VmatTargets) -> Vec<String> {
    match targets {
        VmatTargets::Entries(entries) => entries.clone(),
        VmatTargets::Hero {
            codename,
            include_body,
            include_weapons,
        } => {
            let mut out = std::collections::BTreeSet::new();
            for vpk in vpks {
                for path in vpk.file_paths() {
                    if path.ends_with(".vmat_c")
                        && path.starts_with("models/heroes")
                        && hero_path_match(path.as_str(), codename)
                    {
                        let weapon = is_weapon_path(path.as_str());
                        if (weapon && *include_weapons) || (!weapon && *include_body) {
                            out.insert(path.clone());
                        }
                    }
                }
            }
            out.into_iter().collect()
        }
    }
}

fn discover_materials(
    vpk: &Path,
    base: Option<&Path>,
    vpks: &[valve_pak::VPK],
    targets: &VmatTargets,
) -> Vec<String> {
    let VmatTargets::Hero {
        codename,
        include_body,
        include_weapons,
    } = targets
    else {
        return discover_legacy_materials(vpks, targets);
    };

    match crate::model::live_hero_materials(vpk, base, codename) {
        Ok(materials) if !materials.is_empty() => materials
            .into_iter()
            .filter(|m| (m.weapon && *include_weapons) || (m.body && *include_body))
            .map(|m| compiled_material_entry(&m.material))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        Ok(_) => discover_legacy_materials(vpks, targets),
        Err(err) => {
            eprintln!(
                "  note: live hero material resolution failed for {codename}: {err:#}; \
                 falling back to legacy path-name discovery"
            );
            discover_legacy_materials(vpks, targets)
        }
    }
}

fn compiled_material_entry(material: &str) -> String {
    material
        .strip_suffix(".vmat")
        .map_or_else(|| material.to_string(), |stem| format!("{stem}.vmat_c"))
}

fn material_info(entry: &str, root: &Value) -> VmatInfo {
    let shader = root
        .get("m_shaderName")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    let mut flags = Vec::new();
    if let Some(Value::Array(params)) = root.get("m_intParams") {
        for p in params {
            if let (Some(name), Some(value)) = (
                p.get("m_name").and_then(Value::as_str),
                p.get("m_nValue").and_then(Value::as_int),
            ) {
                if name.starts_with("F_") && value != 0 {
                    flags.push((name.to_string(), value));
                }
            }
        }
    }
    let mut textures = Vec::new();
    if let Some(Value::Array(params)) = root.get("m_textureParams") {
        for p in params {
            if let (Some(name), Some(path)) = (
                p.get("m_name").and_then(Value::as_str),
                p.get("m_pValue").and_then(Value::as_str),
            ) {
                textures.push((name.to_string(), path.to_string()));
            }
        }
    }
    let mut floats = Vec::new();
    if let Some(Value::Array(params)) = root.get("m_floatParams") {
        for p in params {
            if let (Some(name), Some(value)) = (
                p.get("m_name").and_then(Value::as_str),
                p.get("m_flValue").and_then(Value::as_f64),
            ) {
                floats.push((name.to_string(), value));
            }
        }
    }
    let mut vectors = Vec::new();
    if let Some(Value::Array(params)) = root.get("m_vectorParams") {
        for p in params {
            if let (Some(name), Some(Value::Array(lanes))) =
                (p.get("m_name").and_then(Value::as_str), p.get("m_value"))
            {
                vectors.push((
                    name.to_string(),
                    lanes.iter().filter_map(Value::as_f64).collect(),
                ));
            }
        }
    }
    let mut attributes = Vec::new();
    if let Some(Value::Array(params)) = root.get("m_intAttributes") {
        for p in params {
            if let (Some(name), Some(value)) = (
                p.get("m_name").and_then(Value::as_str),
                p.get("m_nValue").and_then(Value::as_int),
            ) {
                attributes.push((name.to_string(), value.to_string()));
            }
        }
    }
    if let Some(Value::Array(params)) = root.get("m_floatAttributes") {
        for p in params {
            if let (Some(name), Some(value)) = (
                p.get("m_name").and_then(Value::as_str),
                p.get("m_flValue").and_then(Value::as_f64),
            ) {
                attributes.push((name.to_string(), value.to_string()));
            }
        }
    }
    if let Some(Value::Array(params)) = root.get("m_vectorAttributes") {
        for p in params {
            if let (Some(name), Some(Value::Array(lanes))) =
                (p.get("m_name").and_then(Value::as_str), p.get("m_value"))
            {
                let value = lanes
                    .iter()
                    .filter_map(Value::as_f64)
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                attributes.push((name.to_string(), value));
            }
        }
    }
    VmatInfo {
        entry: entry.to_string(),
        shader,
        flags,
        textures,
        expressions: material_expressions(root),
        floats,
        vectors,
        attributes,
        dynamic: false,
    }
}

/// Lists the targeted materials with their shader, active feature flags, and
/// bound texture channels: the "what is this skin made of / what is unbound"
/// view used to plan a styling run.
///
/// # Errors
/// Fails if a VPK cannot be opened.
pub fn list_materials(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    targets: &VmatTargets,
) -> Result<Vec<VmatInfo>> {
    let vpk = vpk.as_ref();
    let vpks = open_vpks(vpk, base)?;
    let mut out = Vec::new();
    for entry in discover_materials(vpk, base, &vpks, targets) {
        let Some(bytes) = read_entry(&vpks, &entry) else {
            continue;
        };
        let Ok(root) = morphic::decode_kv3_resource(&bytes) else {
            continue;
        };
        let mut info = material_info(&entry, &root);
        info.dynamic = morphic::kv3_resource_has_blobs(&bytes).unwrap_or(false);
        out.push(info);
    }
    Ok(out)
}

/// Applies `edits` to every targeted `pbr.vfx` material and packs the patched
/// materials into a standalone addon VPK at their original entry paths.
///
/// # Errors
/// Fails if VPKs cannot be opened, no material accepted an edit, or the
/// output cannot be written.
pub fn style_materials_to_addon(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    targets: &VmatTargets,
    edits: &[VmatEdit],
    out: impl AsRef<Path>,
) -> Result<VmatStyleReport> {
    anyhow::ensure!(!edits.is_empty(), "no edits given");
    let vpk = vpk.as_ref();
    let vpks = open_vpks(vpk, base)?;
    let mut report = VmatStyleReport::default();
    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();

    for entry in discover_materials(vpk, base, &vpks, targets) {
        let Some(bytes) = read_entry(&vpks, &entry) else {
            report.skipped_unreadable += 1;
            continue;
        };
        let Ok(root) = morphic::decode_kv3_resource(&bytes) else {
            report.skipped_unreadable += 1;
            continue;
        };
        if root.get("m_shaderName").and_then(Value::as_str) != Some("pbr.vfx") {
            report.skipped_non_pbr += 1;
            continue;
        }
        let (patched, stats) =
            patch_vmat_params(&bytes, edits).with_context(|| format!("patching {entry}"))?;
        for name in stats.failed {
            report.failed_params.push((entry.clone(), name));
        }
        if stats.set + stats.inserted == 0 {
            continue;
        }
        report.params_set += stats.set;
        report.params_inserted += stats.inserted;
        report.materials_patched += 1;
        packed.push((entry, patched));
    }

    anyhow::ensure!(
        !packed.is_empty(),
        "no materials accepted the edits ({} unreadable, {} non-pbr){}",
        report.skipped_unreadable,
        report.skipped_non_pbr,
        if report.failed_params.is_empty() {
            String::new()
        } else {
            let reasons: Vec<String> = report
                .failed_params
                .iter()
                .map(|(e, why)| format!("\n  {e}: {why}"))
                .collect();
            format!("; failures:{}", reasons.join(""))
        }
    );
    let files: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(e, b)| (e.as_str(), b.as_slice()))
        .collect();
    crate::pack(&files, out.as_ref())?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../morphic/fixtures/material/vindicta_headv2.vmat_c"
        ))
        .expect("committed vmat fixture")
    }

    fn int_param(root: &Value, name: &str) -> Option<i64> {
        let i = param_index(root, "m_intParams", name)?;
        root.get("m_intParams")?
            .as_array()?
            .get(i)?
            .get("m_nValue")?
            .as_int()
    }

    #[test]
    fn set_existing_and_insert_missing_params() {
        let bytes = fixture();
        let before = morphic::decode_kv3_resource(&bytes).unwrap();
        assert_eq!(
            int_param(&before, "F_USE_NPR_LIGHTING"),
            Some(1),
            "fixture should be an NPR hero material"
        );
        assert_eq!(int_param(&before, "F_SHEEN"), None);

        let edits = [
            VmatEdit::Int {
                name: "F_USE_NPR_LIGHTING".into(),
                value: 0,
            },
            VmatEdit::Int {
                name: "F_SHEEN".into(),
                value: 1,
            },
            VmatEdit::Float {
                name: "g_fSheenVertexColorStrength1".into(),
                value: 1.0,
            },
            VmatEdit::Vector {
                name: "g_vSheenColorTint1".into(),
                value: [0.5, 0.25, 1.0, 0.0],
            },
        ];
        let (patched, stats) = patch_vmat_params(&bytes, &edits).unwrap();
        assert_eq!(stats.set, 1, "{:?}", stats.failed);
        assert_eq!(stats.inserted, 3, "{:?}", stats.failed);
        assert!(stats.failed.is_empty(), "{:?}", stats.failed);

        let after = morphic::decode_kv3_resource(&patched).unwrap();
        assert_eq!(int_param(&after, "F_USE_NPR_LIGHTING"), Some(0));
        assert_eq!(int_param(&after, "F_SHEEN"), Some(1));
        let i = param_index(&after, "m_vectorParams", "g_vSheenColorTint1").unwrap();
        let v = after
            .get("m_vectorParams")
            .and_then(Value::as_array)
            .and_then(|a| a.get(i))
            .and_then(|p| p.get("m_value"))
            .and_then(Value::as_array)
            .unwrap();
        let comps: Vec<f64> = v.iter().filter_map(Value::as_f64).collect();
        assert_eq!(comps, vec![0.5, 0.25, 1.0, 0.0]);

        // Untouched tables survive byte-faithfully through a re-decode.
        assert_eq!(
            before.get("m_textureParams"),
            after.get("m_textureParams"),
            "texture params must be untouched"
        );
    }

    #[test]
    fn material_info_reports_scalar_params() {
        let bytes = fixture();
        let root = morphic::decode_kv3_resource(&bytes).unwrap();
        let info = material_info("x.vmat_c", &root);
        // The fixture is a real hero material: it has scalar params, and at
        // least one vector lane (every g_v* tint is RGBA).
        assert!(!info.floats.is_empty(), "expected m_floatParams");
        assert!(
            info.vectors.iter().any(|(_, lanes)| !lanes.is_empty()),
            "expected m_vectorParams lanes"
        );
    }

    #[test]
    fn legacy_discovery_keeps_entry_targets_exact() {
        let entries = vec!["models/heroes/example/materials/body.vmat_c".to_string()];
        assert_eq!(
            discover_legacy_materials(&[], &VmatTargets::Entries(entries.clone())),
            entries
        );
    }

    #[test]
    fn live_material_names_are_compiled_for_patching() {
        assert_eq!(
            compiled_material_entry("models/heroes_wip/geist/materials/geist_clothes.vmat"),
            "models/heroes_wip/geist/materials/geist_clothes.vmat_c"
        );
        assert_eq!(
            compiled_material_entry("models/heroes_wip/geist/materials/geist_clothes.vmat_c"),
            "models/heroes_wip/geist/materials/geist_clothes.vmat_c"
        );
    }

    #[test]
    fn set_material_attributes_and_register_names() {
        let bytes = fixture();
        let edits = [
            VmatEdit::FloatAttribute {
                name: "g_flNPRDiffuseStepSharpness".into(),
                value: 8.0,
            },
            VmatEdit::IntAttribute {
                name: "g_nNPRSpecularSteps".into(),
                value: 2,
            },
            VmatEdit::VectorAttribute {
                name: "g_vNPROutlineBrightColor".into(),
                value: [1.0, 0.9, 0.25, 0.0],
            },
        ];
        let (patched, stats) = patch_vmat_params(&bytes, &edits).unwrap();
        assert_eq!(stats.inserted, 3, "{:?}", stats.failed);
        assert!(stats.failed.is_empty(), "{:?}", stats.failed);

        let after = morphic::decode_kv3_resource(&patched).unwrap();
        let i = param_index(&after, "m_floatAttributes", "g_flNPRDiffuseStepSharpness").unwrap();
        assert_eq!(
            after
                .get("m_floatAttributes")
                .and_then(Value::as_array)
                .and_then(|a| a.get(i))
                .and_then(|p| p.get("m_flValue"))
                .and_then(Value::as_f64),
            Some(8.0)
        );
        let i = param_index(&after, "m_intAttributes", "g_nNPRSpecularSteps").unwrap();
        assert_eq!(
            after
                .get("m_intAttributes")
                .and_then(Value::as_array)
                .and_then(|a| a.get(i))
                .and_then(|p| p.get("m_nValue"))
                .and_then(Value::as_int),
            Some(2)
        );
        let attrs = after
            .get("m_renderAttributesUsed")
            .and_then(Value::as_array)
            .unwrap();
        for name in [
            "g_flNPRDiffuseStepSharpness",
            "g_nNPRSpecularSteps",
            "g_vNPROutlineBrightColor",
        ] {
            assert!(
                attrs.iter().any(|v| v.as_str() == Some(name)),
                "{name} must be registered in m_renderAttributesUsed: {attrs:?}"
            );
        }

        let info = material_info("x.vmat_c", &after);
        assert!(
            info.attributes
                .iter()
                .any(|(name, value)| name == "g_flNPRDiffuseStepSharpness" && value == "8"),
            "list view should expose injected attributes: {:?}",
            info.attributes
        );
    }

    #[test]
    fn edit_expr_resolves_against_current_expression() {
        // Seed an expression (blob-aware insert works on the blobbed fixture),
        // then resolve an in-place substitution against it.
        let bytes = fixture();
        let (seeded, s0) = patch_vmat_params(
            &bytes,
            &[VmatEdit::expr("g_flSelfIllumScale1", "-1 * sin(10 * time())").unwrap()],
        )
        .unwrap();
        assert!(s0.failed.is_empty(), "{:?}", s0.failed);
        let tree = morphic::decode_kv3_resource(&seeded).unwrap();

        // 10 -> 20: decompile, substitute, recompile.
        let resolved =
            resolve_edit_expr(&tree, "g_flSelfIllumScale1", "10 * time()", "20 * time()").unwrap();
        let VmatEdit::Expr { bytecode, .. } = &resolved else {
            panic!("expected an Expr edit")
        };
        assert_eq!(
            *bytecode,
            morphic::vfx_expr::compile("-1 * sin(20 * time())")
                .unwrap()
                .bytecode
        );

        // No such expression / absent FIND are clear errors, not silent no-ops.
        assert!(resolve_edit_expr(&tree, "g_flNope", "x", "y")
            .unwrap_err()
            .contains("no existing dynamic expression"));
        assert!(
            resolve_edit_expr(&tree, "g_flSelfIllumScale1", "99 * time()", "1")
                .unwrap_err()
                .contains("not found")
        );
    }

    #[test]
    fn edit_expr_on_blob_material_succeeds() {
        // Editing an existing dynamic expression on a blob-bearing material now
        // works: the bytecode IS a binary blob, and `replace_expr_blob` swaps it
        // byte-faithfully while keeping the block compressed (no re-encode, which
        // would mangle the blob framing). Two expressions are seeded first so the
        // edit exercises the MULTI-blob path (countBlocks == 2, the swap is
        // content-keyed to target the right one). The recompiled bytecode also
        // differs in length from the original here (`* 1` vs `* 100`), proving the
        // re-chunk handles a length change.
        let bytes = fixture();
        let (seeded, s0) = patch_vmat_params(
            &bytes,
            &[
                VmatEdit::expr("g_flSelfIllumScale1", "-1 * sin(10 * time())").unwrap(),
                VmatEdit::expr("g_vColorTint1", "float3(1,1,1) * 1").unwrap(),
            ],
        )
        .unwrap();
        assert!(s0.failed.is_empty(), "seed: {:?}", s0.failed);
        // precondition: a real 2-blob block
        let data = morphic::kv3_resource_data_block(&seeded).unwrap();
        assert_eq!(data[20], 1, "LZ4");
        assert_eq!(
            i32::from_le_bytes(data[56..60].try_into().unwrap()),
            2,
            "2 blobs"
        );

        let edit = VmatEdit::EditExpr {
            name: "g_vColorTint1".into(),
            find: "* 1".into(),
            replace: "* 100".into(),
        };
        let (patched, stats) = patch_vmat_params(&seeded, &[edit]).unwrap();
        assert!(stats.failed.is_empty(), "edit: {:?}", stats.failed);
        assert_eq!(stats.set, 1);

        // The block still decodes, stays a 2-blob compressed v5, and the edited
        // expression now reads back as the recompiled source; the *other* blob is
        // untouched.
        let after = morphic::decode_kv3_resource(&patched).unwrap();
        let read = |root: &Value, n: &str| -> Vec<u8> {
            let i = param_index(root, "m_dynamicParams", n).unwrap();
            root.get("m_dynamicParams")
                .and_then(Value::as_array)
                .and_then(|a| a.get(i))
                .and_then(|p| p.get("m_value"))
                .and_then(expr_bytes)
                .unwrap()
        };
        assert_eq!(
            read(&after, "g_vColorTint1"),
            morphic::vfx_expr::compile("float3(1,1,1) * 100")
                .unwrap()
                .bytecode,
            "edited expression must be the new bytecode"
        );
        assert_eq!(
            read(&after, "g_flSelfIllumScale1"),
            morphic::vfx_expr::compile("-1 * sin(10 * time())")
                .unwrap()
                .bytecode,
            "the other expression blob must be byte-identical"
        );
        let pdata = morphic::kv3_resource_data_block(&patched).unwrap();
        assert_eq!(pdata[20], 1, "stays LZ4-compressed");
        assert_eq!(
            i32::from_le_bytes(pdata[56..60].try_into().unwrap()),
            2,
            "stays 2 blobs"
        );
        // The header size totals the engine validates: a stale sizeUncTotal@48
        // (not updated when unc2 changed) crashed Deadlock with "Bad KV3 data".
        let h = |o: usize| i32::from_le_bytes(pdata[o..o + 4].try_into().unwrap());
        assert_eq!(h(48), h(72) + h(80), "sizeUncTotal@48 == unc1+unc2");
        assert_eq!(h(52), h(76) + h(84), "sizeCompTotal@52 == comp1+comp2");
        // Per-blob LZ4 framing: two small blobs must be two frames, not one
        // concatenated frame. sizeBlockCompressed@68 is the frame-table byte count
        // (2 bytes/frame), so two frames => 4. A region-chunked single frame (=> 2)
        // decodes in our reader but the engine rejects it ("Bad KV3 data").
        assert_eq!(h(68), 4, "two small blobs must be two per-blob LZ4 frames");
    }

    #[test]
    fn insert_dynamic_expression_and_register_attribute() {
        let bytes = fixture();
        let edits = [
            VmatEdit::expr(
                "g_vColorTint1",
                "$ent_health < .4 ? float3(1,.1,.1) : float3(1,1,1)",
            )
            .unwrap(),
            VmatEdit::expr("g_flSelfIllumScale1", "(1 - $ent_health) * 3").unwrap(),
        ];
        let (patched, stats) = patch_vmat_params(&bytes, &edits).unwrap();
        // expression blobs land via the re-encode fallback (counted as set)
        // or a structural insert, depending on what the patcher supports
        assert_eq!(stats.set + stats.inserted, 2, "{:?}", stats.failed);
        assert!(stats.failed.is_empty(), "{:?}", stats.failed);

        let after = morphic::decode_kv3_resource(&patched).unwrap();
        let i = param_index(&after, "m_dynamicParams", "g_vColorTint1").unwrap();
        let blob = after
            .get("m_dynamicParams")
            .and_then(Value::as_array)
            .and_then(|a| a.get(i))
            .and_then(|p| p.get("m_value"))
            .unwrap();
        let VmatEdit::Expr { bytecode, .. } = &edits[0] else {
            unreachable!()
        };
        assert_eq!(blob, &Value::Binary(bytecode.clone()));

        let attrs = after
            .get("m_renderAttributesUsed")
            .and_then(Value::as_array)
            .unwrap();
        assert!(
            attrs.iter().any(|v| v.as_str() == Some("$ent_health")),
            "expression attribute must be registered: {attrs:?}"
        );

        // applying again is a no-op (already_applied catches the same blob)
        let (_, stats2) = patch_vmat_params(&patched, &edits).unwrap();
        assert_eq!(stats2.set, 2);
        assert_eq!(stats2.inserted, 0);

        // the engine only accepts blob sections in the native LZ4 form: the
        // patched DATA block must stay v5 with compressionMethod=1 and a
        // two-blob section (an uncompressed re-emit renders red wireframe)
        let data = morphic::kv3_resource_data_block(&patched).unwrap();
        assert_eq!(data[0], 5, "kv3 version");
        assert_eq!(
            i32::from_le_bytes(data[20..24].try_into().unwrap()),
            1,
            "compressionMethod must stay LZ4"
        );
        assert_eq!(
            i32::from_le_bytes(data[56..60].try_into().unwrap()),
            2,
            "blob count"
        );
    }

    #[test]
    fn preset_names_round_trip() {
        for name in ["gem", "glass", "pbr", "unlit", "ink"] {
            let p = VmatPreset::from_name(name).unwrap();
            assert_eq!(p.as_str(), name);
            assert!(!p.edits(None).is_empty(), "preset {name} has no edits");
        }
        assert!(VmatPreset::from_name("nope").is_err());
    }
}
