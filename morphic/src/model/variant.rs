use crate::{error::DecodeError, kv3::Value};
use std::collections::HashMap;

/// Authored body and material selections for a preview. Does not edit the model resource.
#[derive(Debug, Clone, Default)]
pub struct ModelVariant {
    pub mesh_group: Option<String>,
    pub material_group: Option<String>,
    pub material_override: Option<String>,
    pub draw_material_overrides: Vec<DrawMaterialOverride>,
}

/// A world-node material override. The first whole-model entry wins; otherwise
/// the last matching mesh/draw entry wins, as VRF WorldNode.MaterialOverride.Find.
#[derive(Debug, Clone)]
pub struct DrawMaterialOverride {
    pub mesh_index: usize,
    pub draw_call_index: u32,
    pub material: String,
}

impl ModelVariant {
    pub(super) fn draw_material(&self, mesh_index: usize, draw_call_index: usize) -> Option<&str> {
        let mut selected = None;
        for entry in &self.draw_material_overrides {
            if entry.material.is_empty() {
                continue;
            }
            if entry.draw_call_index == u32::MAX {
                return Some(&entry.material);
            }
            if entry.mesh_index == mesh_index
                && usize::try_from(entry.draw_call_index).ok() == Some(draw_call_index)
            {
                selected = Some(entry.material.as_str());
            }
        }
        selected
    }
    pub(super) fn mesh_mask(&self, data: &Value) -> Result<Option<u64>, DecodeError> {
        let Some(name) = &self.mesh_group else {
            return Ok(None);
        };
        let groups = data
            .get("m_meshGroups")
            .and_then(Value::as_array)
            .ok_or(DecodeError::Model("model has no body groups"))?;
        let index = groups
            .iter()
            .position(|v| {
                v.as_str()
                    .is_some_and(|s| s == name || s.rsplit("#&").next() == Some(name.as_str()))
            })
            .filter(|&i| i < 64)
            .ok_or(DecodeError::Model("requested body group is missing"))?;
        Ok(Some(1u64 << index))
    }

    pub(super) fn material_map(
        &self,
        data: &Value,
    ) -> Result<HashMap<String, String>, DecodeError> {
        fn materials(v: &Value) -> Result<&[Value], DecodeError> {
            v.get("m_materials")
                .and_then(Value::as_array)
                .ok_or(DecodeError::Model("material group has no material list"))
        }
        let Some(name) = &self.material_group else {
            return Ok(HashMap::new());
        };
        let groups = data
            .get("m_materialGroups")
            .and_then(Value::as_array)
            .ok_or(DecodeError::Model("model has no material groups"))?;
        let selected = groups
            .iter()
            .find(|v| v.get("m_name").and_then(Value::as_str) == Some(name.as_str()))
            .ok_or(DecodeError::Model("requested material group is missing"))?;
        let original = materials(
            groups
                .first()
                .ok_or(DecodeError::Model("model has no default material group"))?,
        )?;
        let replacements = materials(selected)?;
        if original.len() != replacements.len() {
            return Err(DecodeError::Model("material group slot counts differ"));
        }
        original
            .iter()
            .zip(replacements)
            .map(|(a, b)| {
                Ok((
                    a.as_str()
                        .ok_or(DecodeError::Model("invalid material group path"))?
                        .to_string(),
                    b.as_str()
                        .ok_or(DecodeError::Model("invalid material group path"))?
                        .to_string(),
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn world_material_override_precedence_matches_vrf() {
        let entry = |mesh_index, draw_call_index, material: &str| DrawMaterialOverride {
            mesh_index,
            draw_call_index,
            material: material.into(),
        };
        let mut variant = ModelVariant {
            draw_material_overrides: vec![
                entry(1, 0, "first"),
                entry(1, 0, "last"),
                entry(0, 1, "other"),
            ],
            ..Default::default()
        };
        assert_eq!(variant.draw_material(1, 0), Some("last"));
        assert_eq!(variant.draw_material(0, 1), Some("other"));
        assert_eq!(variant.draw_material(0, 0), None);
        variant.draw_material_overrides.extend([
            entry(9, u32::MAX, ""),
            entry(9, u32::MAX, "all"),
            entry(0, u32::MAX, "ignored"),
        ]);
        assert_eq!(variant.draw_material(1, 0), Some("all"));
        assert_eq!(variant.draw_material(0, 0), Some("all"));
    }
    fn object(values: Vec<(&str, Value)>) -> Value {
        Value::Object(
            values
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }
    fn strings(values: &[&str]) -> Value {
        Value::Array(
            values
                .iter()
                .map(|s| Value::String((*s).to_string()))
                .collect(),
        )
    }
    #[test]
    fn selects_authored_body_group_and_rejects_missing_group() {
        let data = object(vec![(
            "m_meshGroups",
            strings(&["default_@0_#&body", "default_@1_#&inflated"]),
        )]);
        let mut variant = ModelVariant {
            mesh_group: Some("inflated".into()),
            ..Default::default()
        };
        assert_eq!(variant.mesh_mask(&data).unwrap(), Some(2));
        variant.mesh_group = Some("missing".into());
        assert!(variant.mesh_mask(&data).is_err());
        assert_eq!(ModelVariant::default().mesh_mask(&data).unwrap(), None);
    }
    #[test]
    fn maps_material_slots_without_changing_shared_materials() {
        let group = |name: &str, paths: &[&str]| {
            object(vec![
                ("m_name", Value::String(name.into())),
                ("m_materials", strings(paths)),
            ])
        };
        let data = object(vec![(
            "m_materialGroups",
            Value::Array(vec![
                group("default", &["body.vmat", "gun.vmat"]),
                group("ultimate", &["rage.vmat", "gun.vmat"]),
            ]),
        )]);
        let variant = ModelVariant {
            material_group: Some("ultimate".into()),
            ..Default::default()
        };
        let map = variant.material_map(&data).unwrap();
        assert_eq!(map["body.vmat"], "rage.vmat");
        assert_eq!(map["gun.vmat"], "gun.vmat");
        assert!(ModelVariant {
            material_group: Some("missing".into()),
            ..Default::default()
        }
        .material_map(&data)
        .is_err());
    }
}
