//! Source 2 cloth (`FeModel`) anchor extraction from a model's `PHYS` block.
//!
//! Deadlock fabric is driven at runtime by the `FeModel` finite-element cloth
//! solver, which writes the world transforms of dedicated `$cloth_*` bones every
//! frame. Those bones are skeleton ROOTS with no animation track, so a static
//! posed bake (no solver) leaves them at bind while the body moves and the
//! fabric detaches (or, with a naive nearest-bone guess, smears). The `FeModel`
//! records, per cloth node, the body bone that drives it: `m_SkelParents` forms a
//! node tree that terminates at driver nodes whose `m_CtrlName` is a real
//! skeleton bone (`pelvis`, `clavicle_R`, `coat_e_0`, ...). [`ClothAnchors`]
//! exposes that `$cloth` bone -> anchor bone mapping so the pose baker can rigidly
//! carry each cloth root with its TRUE anchor instead of guessing.
//!
//! This is the static-export fix: it reproduces the engine's settled rest drape
//! (kinematic nodes exactly, hanging nodes at their authored rest shape). It does
//! not run the cloth solver, so it does not reproduce live sway/collision under
//! an arbitrary action pose; for a standing menu/idle snapshot the rest drape is
//! exactly what the engine shows.

use std::collections::HashMap;

use crate::kv3::Value;
use crate::resource::Resource;

/// Maps a cloth bone name (`$cloth_*`) to the skeleton bone that drives it.
#[derive(Debug, Clone, Default)]
pub struct ClothAnchors {
    anchor: HashMap<String, String>,
}

impl ClothAnchors {
    /// The driver/anchor bone name for `cloth_bone`, if the `FeModel` records one.
    #[must_use]
    pub fn anchor_of(&self, cloth_bone: &str) -> Option<&str> {
        self.anchor.get(cloth_bone).map(String::as_str)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.anchor.len()
    }
}

/// Parse the cloth-anchor map from a `.vmdl_c`'s `PHYS` block. Returns `None`
/// when the model carries no `PHYS` block or no `FeModel` (weapons, most props,
/// heroes whose only secondary motion is parented hair/coat that follows FK).
#[must_use]
pub fn decode_cloth_anchors(model_bytes: &[u8]) -> Option<ClothAnchors> {
    let resource = Resource::parse(model_bytes).ok()?;
    let phys = resource.find_block(*b"PHYS")?;
    let root = crate::kv3::decode(phys).ok()?;
    anchors_from_phys(&root)
}

fn anchors_from_phys(root: &Value) -> Option<ClothAnchors> {
    let fe = find_fe_model(root)?;
    let names = fe.get("m_CtrlName").and_then(Value::as_array)?;
    let parents = fe.get("m_SkelParents").and_then(Value::as_array)?;
    if names.len() != parents.len() {
        return None;
    }
    let node_name: Vec<&str> = names.iter().map(|v| v.as_str().unwrap_or("")).collect();
    let node_parent: Vec<i64> = parents.iter().map(|v| v.as_int().unwrap_or(-1)).collect();

    let mut anchor = HashMap::new();
    for (i, name) in node_name.iter().enumerate() {
        // Only rootless cloth nodes need an anchor; a node whose own name is not a
        // `$cloth*` bone is a driver bone the skeleton already poses via FK.
        if !is_cloth_node_name(name) {
            continue;
        }
        let Some(terminal) = walk_to_terminal(i, &node_parent) else {
            continue;
        };
        let anchor_name = node_name[terminal];
        // The terminal must be a real (non-cloth) driver bone for the map to help.
        if !anchor_name.is_empty() && !is_cloth_node_name(anchor_name) {
            anchor.insert((*name).to_string(), anchor_name.to_string());
        }
    }
    if anchor.is_empty() {
        None
    } else {
        Some(ClothAnchors { anchor })
    }
}

/// Walk the `m_SkelParents` node tree from `start` to its terminal (parent < 0),
/// guarding against cycles and out-of-range indices. Returns the terminal node
/// index, or `None` on a malformed (cyclic) chain.
fn walk_to_terminal(start: usize, parent: &[i64]) -> Option<usize> {
    let mut cur = start;
    for _ in 0..=parent.len() {
        let p = *parent.get(cur)?;
        if p < 0 {
            return Some(cur);
        }
        let p = usize::try_from(p).ok()?;
        if p >= parent.len() || p == cur {
            return Some(cur);
        }
        cur = p;
    }
    None
}

fn is_cloth_node_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("$cloth") || lower.starts_with("cloth")
}

/// Locate the `FeModel` object inside the `PHYS` KV3 tree. It sits under
/// `m_feModel`/`m_pFeModel`, optionally nested in `m_parts[*]`.
fn find_fe_model(root: &Value) -> Option<&Value> {
    if let Some(fe) = root.get("m_feModel").or_else(|| root.get("m_pFeModel")) {
        return Some(fe);
    }
    let parts = root.get("m_parts").and_then(Value::as_array)?;
    parts
        .iter()
        .find_map(|p| p.get("m_pFeModel").or_else(|| p.get("m_feModel")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv3::Value;

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }
    fn i(v: i64) -> Value {
        Value::Int(v)
    }

    /// A node tree where two cloth nodes chain up through cloth parents to a real
    /// driver bone resolves each cloth bone to that bone.
    #[test]
    fn resolves_cloth_nodes_to_terminal_driver_bone() {
        // nodes: 0 driver "pelvis" (root), 1 "$cloth_a" -> 0, 2 "$cloth_b" -> 1
        let fe = Value::Object(vec![
            (
                "m_CtrlName".into(),
                Value::Array(vec![s("pelvis"), s("$cloth_a"), s("$cloth_b")]),
            ),
            (
                "m_SkelParents".into(),
                Value::Array(vec![i(-1), i(0), i(1)]),
            ),
        ]);
        let root = Value::Object(vec![("m_feModel".into(), fe)]);
        let anchors = anchors_from_phys(&root).expect("anchors");
        assert_eq!(anchors.anchor_of("$cloth_a"), Some("pelvis"));
        assert_eq!(anchors.anchor_of("$cloth_b"), Some("pelvis"));
        // The driver bone itself is not in the map (it is FK-posed).
        assert_eq!(anchors.anchor_of("pelvis"), None);
    }

    /// A cyclic chain is dropped rather than looping forever.
    #[test]
    fn cyclic_chain_is_ignored() {
        let fe = Value::Object(vec![
            (
                "m_CtrlName".into(),
                Value::Array(vec![s("$cloth_a"), s("$cloth_b")]),
            ),
            ("m_SkelParents".into(), Value::Array(vec![i(1), i(0)])),
        ]);
        let root = Value::Object(vec![("m_feModel".into(), fe)]);
        assert!(anchors_from_phys(&root).is_none());
    }
}
