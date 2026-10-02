//! The modding knowledge base an agent can search and read.
//!
//! The curated guides ship inside the binary; `VPKMERGE_DOCS_DIRS` adds any
//! markdown directories on disk (working notes, the community knowledge base).
//! Documents are split at headings and ranked with BM25, so a search returns the
//! section that answers the question rather than a whole file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

macro_rules! embedded {
    ($($id:literal => $path:literal),* $(,)?) => {
        &[$(($id, include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../", $path)))),*]
    };
}

const EMBEDDED: &[(&str, &str)] = embedded! {
    "guides/README.md" => "docs/deadlock-modding/README.md",
    "guides/fundamentals.md" => "docs/deadlock-modding/fundamentals.md",
    "guides/asset-formats.md" => "docs/deadlock-modding/asset-formats.md",
    "guides/textures-materials.md" => "docs/deadlock-modding/textures-materials.md",
    "guides/models-animation.md" => "docs/deadlock-modding/models-animation.md",
    "guides/particles-vfx.md" => "docs/deadlock-modding/particles-vfx.md",
    "guides/audio-soundevents.md" => "docs/deadlock-modding/audio-soundevents.md",
    "guides/hero-identifiers.md" => "docs/deadlock-modding/hero-identifiers.md",
    "guides/testing-debugging.md" => "docs/deadlock-modding/testing-debugging.md",
    "skills/package-mods.md" => "vpkmerge-harness/skills/package-mods/SKILL.md",
    "skills/blender-mod-workflow.md" => "vpkmerge-harness/skills/blender-mod-workflow/SKILL.md",
};

/// Above this a whole-document read returns the outline instead.
const FULL_READ_LIMIT: usize = 24_000;
const SNIPPET_CHARS: usize = 320;
const HEADING_WEIGHT: u32 = 3;
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;
/// Embedded guides are reviewed; extra dirs are often working notes.
const CURATED_BOOST: f64 = 1.5;

struct Doc {
    id: String,
    title: String,
    text: String,
    curated: bool,
}

struct Section {
    doc: usize,
    heading: String,
    body: String,
    terms: HashMap<String, u32>,
    len: u32,
}

pub struct Hit<'a> {
    pub doc: &'a str,
    pub title: &'a str,
    pub section: &'a str,
    pub snippet: String,
    pub score: f64,
}

pub struct DocIndex {
    docs: Vec<Doc>,
    sections: Vec<Section>,
    doc_freq: HashMap<String, u32>,
    avg_len: f64,
}

impl DocIndex {
    /// The embedded guides plus every `.md` under `extra_dirs`.
    #[must_use]
    pub fn load(extra_dirs: &[PathBuf]) -> Self {
        let curated = EMBEDDED
            .iter()
            .map(|(id, text)| ((*id).to_owned(), (*text).to_owned()))
            .collect();
        let mut extra = Vec::new();
        for dir in extra_dirs {
            let label = dir.file_name().map_or_else(
                || dir.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let mut files = Vec::new();
            collect_markdown(dir, &mut files);
            files.sort();
            for file in files {
                let Ok(text) = std::fs::read_to_string(&file) else {
                    continue;
                };
                let rel = file.strip_prefix(dir).unwrap_or(&file);
                let id = format!("{label}/{}", rel.to_string_lossy().replace('\\', "/"));
                extra.push((id, text));
            }
        }
        Self::from_sources(curated, extra)
    }

    /// Index `(id, markdown)` documents. `curated` ones rank above `extra`.
    #[must_use]
    pub fn from_sources(curated: Vec<(String, String)>, extra: Vec<(String, String)>) -> Self {
        let mut docs: Vec<Doc> = Vec::new();
        let mut sections = Vec::new();
        let curated_count = curated.len();
        for (i, (id, text)) in curated.into_iter().chain(extra).enumerate() {
            // The same guide can arrive embedded and from an extra dir.
            if docs.iter().any(|d| d.text == text) {
                continue;
            }
            let doc = docs.len();
            let mut title = None;
            for (level, heading, body) in split_sections(&text) {
                if level == 1 && title.is_none() {
                    title = Some(heading.clone());
                }
                let mut terms: HashMap<String, u32> = HashMap::new();
                for t in tokenize(&heading) {
                    *terms.entry(t).or_default() += HEADING_WEIGHT;
                }
                for t in tokenize(&body) {
                    *terms.entry(t).or_default() += 1;
                }
                let len = terms.values().sum();
                if len == 0 {
                    continue;
                }
                sections.push(Section {
                    doc,
                    heading,
                    body,
                    terms,
                    len,
                });
            }
            docs.push(Doc {
                title: title.unwrap_or_else(|| id.clone()),
                id,
                text,
                curated: i < curated_count,
            });
        }

        let mut doc_freq: HashMap<String, u32> = HashMap::new();
        for s in &sections {
            for t in s.terms.keys() {
                *doc_freq.entry(t.clone()).or_default() += 1;
            }
        }
        let total: f64 = sections.iter().map(|s| f64::from(s.len)).sum();
        #[allow(clippy::cast_precision_loss)]
        let avg_len = total / sections.len().max(1) as f64;
        Self {
            docs,
            sections,
            doc_freq,
            avg_len,
        }
    }

    /// `(id, title)` of every document.
    pub fn catalog(&self) -> impl Iterator<Item = (&str, &str)> {
        self.docs.iter().map(|d| (d.id.as_str(), d.title.as_str()))
    }

    /// The best-matching sections for `query`, highest score first.
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit<'_>> {
        let terms = tokenize(query);
        #[allow(clippy::cast_precision_loss)]
        let n = self.sections.len() as f64;
        let mut scored: Vec<(f64, &Section)> = self
            .sections
            .iter()
            .filter_map(|s| {
                let score: f64 = terms
                    .iter()
                    .filter_map(|t| {
                        let tf = f64::from(*s.terms.get(t)?);
                        let df = f64::from(self.doc_freq[t]);
                        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                        let norm = 1.0 - BM25_B + BM25_B * f64::from(s.len) / self.avg_len;
                        Some(idf * tf * (BM25_K1 + 1.0) / (tf + BM25_K1 * norm))
                    })
                    .sum();
                let boost = if self.docs[s.doc].curated {
                    CURATED_BOOST
                } else {
                    1.0
                };
                (score > 0.0).then_some((score * boost, s))
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored
            .into_iter()
            .take(limit)
            .map(|(score, s)| Hit {
                doc: &self.docs[s.doc].id,
                title: &self.docs[s.doc].title,
                section: &s.heading,
                snippet: snippet(&s.body, &terms),
                score,
            })
            .collect()
    }

    /// A whole document, or one section of it (with its subsections). A large
    /// document read without a section returns its outline instead.
    pub fn read(&self, doc: &str, section: Option<&str>) -> Result<String> {
        let doc = self.find(doc)?;
        let Some(section) = section else {
            if doc.text.len() <= FULL_READ_LIMIT {
                return Ok(doc.text.clone());
            }
            return Ok(format!(
                "{} is {} KB, too long to return whole. Pass `section` with one of these \
                 headings:\n{}",
                doc.id,
                doc.text.len() / 1024,
                outline(&doc.text)
            ));
        };
        let want = section.trim().trim_start_matches('#').trim().to_lowercase();
        let headings = split_sections(&doc.text);
        let Some(start) = headings
            .iter()
            .position(|(_, h, _)| h.to_lowercase() == want)
            .or_else(|| {
                headings
                    .iter()
                    .position(|(_, h, _)| h.to_lowercase().contains(&want))
            })
        else {
            bail!(
                "no section {section:?} in {}. Its headings:\n{}",
                doc.id,
                outline(&doc.text)
            );
        };
        let level = headings[start].0;
        let mut out = String::new();
        for (i, (l, heading, body)) in headings.iter().enumerate().skip(start) {
            if i > start && *l <= level {
                break;
            }
            if *l > 0 {
                out.push_str(&"#".repeat(usize::from(*l)));
                out.push(' ');
                out.push_str(heading);
                out.push('\n');
            }
            out.push_str(body);
            out.push('\n');
        }
        Ok(out)
    }

    fn find(&self, id: &str) -> Result<&Doc> {
        if let Some(doc) = self.docs.iter().find(|d| d.id == id) {
            return Ok(doc);
        }
        let matches: Vec<&Doc> = self.docs.iter().filter(|d| d.id.contains(id)).collect();
        match matches.as_slice() {
            [doc] => Ok(doc),
            [] => bail!(
                "no document {id:?}. Available:\n{}",
                self.catalog()
                    .map(|(id, title)| format!("- {id}: {title}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            many => bail!(
                "{id:?} matches several documents: {}",
                many.iter()
                    .map(|d| d.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// Split markdown at `#`..`###` headings into `(level, heading, body)`. Text
/// before the first heading is level 0 with an empty heading. `#` lines inside
/// code fences are not headings.
fn split_sections(text: &str) -> Vec<(u8, String, String)> {
    let mut sections = vec![(0u8, String::new(), String::new())];
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        let hashes = line.bytes().take_while(|b| *b == b'#').count();
        if !fenced && (1..=3).contains(&hashes) && line[hashes..].starts_with(' ') {
            #[allow(clippy::cast_possible_truncation)]
            sections.push((
                hashes as u8,
                line[hashes..].trim().to_owned(),
                String::new(),
            ));
        } else if let Some((_, _, body)) = sections.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    sections.retain(|(level, _, body)| *level > 0 || !body.trim().is_empty());
    sections
}

fn outline(text: &str) -> String {
    split_sections(text)
        .iter()
        .filter(|(level, _, _)| *level > 0)
        .map(|(level, heading, _)| format!("{}- {heading}", "  ".repeat(usize::from(*level) - 1)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Lowercased word terms. Identifiers keep their `_` form and also index each
/// part (`vsnd_files` matches `vsnd`), and a trailing plural `s` is dropped.
fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let word = word.trim_matches('_').to_lowercase();
        if word.len() < 2 {
            continue;
        }
        if word.contains('_') {
            out.extend(word.split('_').filter(|p| p.len() >= 2).map(stem));
        }
        out.push(stem(&word));
    }
    out
}

fn stem(word: &str) -> String {
    match word.strip_suffix('s') {
        Some(base) if base.len() >= 3 && !base.ends_with('s') => base.to_owned(),
        _ => word.to_owned(),
    }
}

/// The part of `body` around the first line that mentions a query term.
fn snippet(body: &str, terms: &[String]) -> String {
    let line = body
        .lines()
        .find(|line| tokenize(line).iter().any(|t| terms.contains(t)))
        .and_then(|line| body.find(line))
        .unwrap_or(0);
    // Open at the paragraph start so the snippet reads as a sentence, unless
    // that would push the matching line out of the window.
    let paragraph = body[..line].rfind("\n\n").map_or(0, |i| i + 2);
    let start = if line - paragraph < SNIPPET_CHARS / 2 {
        paragraph
    } else {
        line
    };
    let mut out: String = body[start..].chars().take(SNIPPET_CHARS).collect();
    if body[start..].chars().count() > SNIPPET_CHARS {
        out.push_str("...");
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> DocIndex {
        let sources = vec![
            (
                "a/audio.md".to_owned(),
                "# Audio\n\nIntro.\n\n## Randomizer pools\n\nMost events list several \
                 vsnd_files clips.\n\n### Collapse\n\nRewrite the pool to one clip.\n\n## Loops\n\n\
                 Looping clips keep m_nLoopStart.\n"
                    .to_owned(),
            ),
            (
                "a/textures.md".to_owned(),
                "# Textures\n\n```\n# not a heading\n```\n\n## Recolor\n\nSet the hue of a \
                 texture.\n"
                    .to_owned(),
            ),
        ];
        DocIndex::from_sources(sources, Vec::new())
    }

    #[test]
    fn search_ranks_the_matching_section_first() {
        let idx = index();
        let hits = idx.search("sound event pool", 5);
        assert_eq!(hits[0].doc, "a/audio.md");
        assert_eq!(hits[0].section, "Randomizer pools");
        assert!(hits[0].snippet.contains("vsnd_files"));
        assert_eq!(idx.search("hue", 5)[0].section, "Recolor");
        assert!(idx.search("zzzz", 5).is_empty());
    }

    #[test]
    fn identifier_parts_are_searchable() {
        assert_eq!(index().search("vsnd", 5)[0].section, "Randomizer pools");
    }

    #[test]
    fn read_section_includes_its_subsections_only() {
        let text = index().read("audio", Some("randomizer pools")).unwrap();
        assert!(text.contains("### Collapse") && text.contains("one clip"));
        assert!(!text.contains("Loops"));
    }

    #[test]
    fn fenced_hashes_are_not_headings() {
        let text = index().read("a/textures.md", Some("Textures")).unwrap();
        assert!(text.contains("# not a heading"));
        assert!(index()
            .read("a/textures.md", Some("not a heading"))
            .is_err());
    }

    #[test]
    fn unknown_document_lists_the_catalog() {
        let err = index().read("nope", None).unwrap_err().to_string();
        assert!(err.contains("a/audio.md: Audio"));
    }

    #[test]
    fn embedded_guides_are_indexed() {
        let idx = DocIndex::load(&[]);
        assert_eq!(idx.catalog().count(), EMBEDDED.len());
        let hits = idx.search("randomizer pool soundevent", 3);
        assert!(hits.iter().any(|h| h.doc == "guides/audio-soundevents.md"));
    }

    #[test]
    fn curated_docs_outrank_notes_and_duplicates_are_dropped() {
        let doc = |id: &str, body: &str| (id.to_owned(), format!("# Loops\n\n{body}\n"));
        let idx = DocIndex::from_sources(
            vec![doc("guide.md", "Looping clips restart.")],
            vec![
                doc("note.md", "Looping clips restart, I think."),
                doc("copy.md", "Looping clips restart."),
            ],
        );
        let hits = idx.search("looping clips", 5);
        let ids: Vec<&str> = hits.iter().map(|h| h.doc).collect();
        assert_eq!(ids, ["guide.md", "note.md"]);
    }
}
