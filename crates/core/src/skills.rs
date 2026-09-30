//! Skills are folders with a `SKILL.md` (YAML front matter: `name`,
//! `description`). The model sees the index and loads a body on demand.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::providers::ToolSpec;

pub const LOAD_TOOL: &str = "load_skill";
pub const READ_FILE_TOOL: &str = "read_skill_file";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
    pub enabled: bool,
}

#[derive(Deserialize)]
struct FrontMatter {
    name: Option<String>,
    description: Option<String>,
}

pub fn scan(dirs: &[PathBuf], disabled: &BTreeSet<String>) -> Vec<Skill> {
    let mut skills: Vec<Skill> = Vec::new();
    for root in dirs {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        let mut found: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        found.sort();
        if root.join("SKILL.md").is_file() {
            found.insert(0, root.clone());
        }
        for dir in found {
            let file = dir.join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let (front, _) = split_front_matter(&text);
            let meta: Option<FrontMatter> = front.and_then(|f| serde_yaml_ng::from_str(f).ok());
            let name = meta
                .as_ref()
                .and_then(|m| m.name.clone())
                .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            if name.is_empty() || skills.iter().any(|s| s.name == name) {
                continue;
            }
            skills.push(Skill {
                enabled: !disabled.contains(&name),
                description: meta.and_then(|m| m.description).unwrap_or_default(),
                name,
                dir,
            });
        }
    }
    skills
}

fn split_front_matter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---") else {
        return (None, text);
    };
    let rest = rest.trim_start_matches(['\r', '\n']);
    match rest.find("\n---") {
        Some(end) => {
            let body = &rest[end + 4..];
            (Some(&rest[..end]), body.trim_start_matches(['-', '\r', '\n']))
        }
        None => (None, text),
    }
}

pub fn index_prompt(skills: &[Skill]) -> Option<String> {
    let enabled: Vec<&Skill> = skills.iter().filter(|s| s.enabled).collect();
    if enabled.is_empty() {
        return None;
    }
    let mut out = format!(
        "## Skills\nSkills are instruction packs for specific jobs. When a request matches one, \
         call `{LOAD_TOOL}` with its name before doing the work, then follow it.\n"
    );
    for s in enabled {
        let _ = writeln!(out, "- {}: {}", s.name, s.description.replace('\n', " "));
    }
    Some(out)
}

pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: LOAD_TOOL.into(),
            description: "Load a skill's full instructions by name.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"],
            }),
        },
        ToolSpec {
            name: READ_FILE_TOOL.into(),
            description: "Read a file that ships with a skill, by path relative to the skill folder.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "path": { "type": "string" },
                },
                "required": ["name", "path"],
            }),
        },
    ]
}

pub fn load(skills: &[Skill], name: &str) -> anyhow::Result<String> {
    let skill = find(skills, name)?;
    let text = std::fs::read_to_string(skill.dir.join("SKILL.md"))?;
    let (_, body) = split_front_matter(&text);
    let mut files = Vec::new();
    collect_files(&skill.dir, &skill.dir, &mut files);
    files.retain(|f| f != "SKILL.md");
    let mut out = body.to_owned();
    if !files.is_empty() {
        let _ = writeln!(out, "\n\n---\nFiles in this skill (read with `{READ_FILE_TOOL}`):");
        for f in files.iter().take(200) {
            let _ = writeln!(out, "- {f}");
        }
    }
    Ok(out)
}

pub fn read_file(skills: &[Skill], name: &str, rel: &str) -> anyhow::Result<String> {
    let skill = find(skills, name)?;
    let root = skill.dir.canonicalize()?;
    let target = root
        .join(rel)
        .canonicalize()
        .with_context(|| format!("{rel} not found in skill {name}"))?;
    if !target.starts_with(&root) {
        bail!("path escapes the skill folder");
    }
    let bytes = std::fs::read(&target)?;
    if bytes.len() > 512 * 1024 {
        bail!("{rel} is larger than 512 KB");
    }
    String::from_utf8(bytes).context("not a text file")
}

fn find<'a>(skills: &'a [Skill], name: &str) -> anyhow::Result<&'a Skill> {
    skills
        .iter()
        .find(|s| s.enabled && s.name == name)
        .with_context(|| format!("no enabled skill named {name}"))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().into_owned());
        }
    }
    out.sort();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_front_matter_and_confines_reads() {
        let root = std::env::temp_dir().join(format!("wb-skills-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("merge/refs")).unwrap();
        std::fs::write(
            root.join("merge/SKILL.md"),
            "---\nname: vpk-merge\ndescription: |\n  Merge VPKs.\n  Handles conflicts.\n---\nStep one.\n",
        )
        .unwrap();
        std::fs::write(root.join("merge/refs/policy.md"), "first wins").unwrap();
        std::fs::create_dir_all(root.join("plain")).unwrap();
        std::fs::write(root.join("plain/SKILL.md"), "no front matter").unwrap();

        let disabled = BTreeSet::from(["plain".to_owned()]);
        let skills = scan(std::slice::from_ref(&root), &disabled);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "vpk-merge");
        assert!(skills[0].description.contains("Handles conflicts."));
        assert!(!skills[1].enabled);

        let index = index_prompt(&skills).unwrap();
        assert!(index.contains("- vpk-merge: Merge VPKs. Handles conflicts."));
        assert!(!index.contains("plain"));

        let body = load(&skills, "vpk-merge").unwrap();
        assert!(body.starts_with("Step one."));
        assert!(body.contains("refs/policy.md"));
        assert_eq!(read_file(&skills, "vpk-merge", "refs/policy.md").unwrap(), "first wins");
        assert!(read_file(&skills, "vpk-merge", "../plain/SKILL.md").is_err());
        assert!(load(&skills, "plain").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
