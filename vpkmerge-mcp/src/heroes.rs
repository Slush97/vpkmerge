//! Hero identity: one hero, several names.
//!
//! Deadlock names a hero differently per asset system: the roster record
//! (`orion`), the display name (`Grey Talon`), and asset namespaces that can be
//! either or neither (`archer`). Users say the display name. This joins them so
//! every tool accepts any of those and finds the right key in its own index.

use std::path::Path;

use anyhow::{bail, Result};
use vpkmerge_core::{build_hero_roster, HeroInfo, DEFAULT_LANG};

/// Asset namespaces that are neither the roster codename nor a word of the
/// display name. Observed in shipped paths (see docs/deadlock-modding/
/// hero-identifiers.md); they drift with game updates.
const EXTRA_ALIASES: &[(&str, &[&str])] = &[
    // Abrams: roster `atlas`, assets migrating `abrams` -> `bull`.
    ("atlas", &["bull"]),
    // Grey Talon: roster `orion`, textures + recolor recipe under `archer`.
    ("orion", &["archer"]),
    // Mo & Krill: recolor recipe + particles under `digger`.
    ("krill", &["digger"]),
];

pub struct Hero {
    pub info: HeroInfo,
    aliases: Vec<String>,
}

impl Hero {
    fn new(info: HeroInfo) -> Self {
        let mut aliases = vec![norm(&info.codename), norm(&info.name)];
        aliases.extend(
            info.name
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.len() >= 3 && !w.eq_ignore_ascii_case("the"))
                .map(norm),
        );
        if let Some((_, extra)) = EXTRA_ALIASES.iter().find(|(c, _)| *c == info.codename) {
            aliases.extend(extra.iter().map(|a| (*a).to_owned()));
        }
        aliases.sort();
        aliases.dedup();
        Self { info, aliases }
    }

    /// Whether an index key names this hero. Tolerates the `hero_` prefix the VO
    /// tree uses and variant suffixes (`hornet_v3`, `atlas_detective`).
    #[must_use]
    pub fn owns(&self, key: &str) -> bool {
        let key = key.to_lowercase();
        let key = key.strip_prefix("hero_").unwrap_or(&key);
        let first = key.split('_').next().unwrap_or(key);
        let whole = norm(key);
        self.aliases.iter().any(|a| *a == whole || a == first)
    }
}

pub struct Roster(Vec<Hero>);

impl Roster {
    pub fn load(pak: &Path) -> Result<Self> {
        Ok(Self::from_infos(build_hero_roster(
            pak,
            None,
            DEFAULT_LANG,
        )?))
    }

    #[must_use]
    pub fn from_infos(infos: Vec<HeroInfo>) -> Self {
        Self(infos.into_iter().map(Hero::new).collect())
    }

    #[must_use]
    pub fn heroes(&self) -> &[Hero] {
        &self.0
    }

    /// Resolve what a user typed (display name, codename, asset alias, or a
    /// distinctive part of the name) to one hero.
    pub fn resolve(&self, query: &str) -> Result<&Hero> {
        let q = norm(query);
        if let Some(hero) = self
            .0
            .iter()
            .find(|h| norm(&h.info.codename) == q || norm(&h.info.name) == q)
        {
            return Ok(hero);
        }
        let mut matches: Vec<&Hero> = self.0.iter().filter(|h| h.aliases.contains(&q)).collect();
        if matches.is_empty() && q.len() >= 3 {
            matches = self
                .0
                .iter()
                .filter(|h| norm(&h.info.name).contains(&q) || h.info.codename.contains(&q))
                .collect();
        }
        match matches.as_slice() {
            [hero] => Ok(hero),
            [] => bail!(
                "unknown hero {query:?}. Known heroes: {}",
                self.names(|h| h.info.selectable)
            ),
            many => bail!(
                "{query:?} is ambiguous between {}",
                many.iter()
                    .map(|h| h.info.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// The hero an index key belongs to, if any.
    #[must_use]
    pub fn owner(&self, key: &str) -> Option<&Hero> {
        self.0.iter().find(|h| h.owns(key))
    }

    pub fn names(&self, keep: impl Fn(&Hero) -> bool) -> String {
        self.0
            .iter()
            .filter(|h| keep(h))
            .map(|h| h.info.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> Roster {
        let hero = |codename: &str, name: &str| HeroInfo {
            codename: codename.to_owned(),
            name: name.to_owned(),
            selectable: true,
            in_development: false,
            disabled: false,
        };
        Roster::from_infos(vec![
            hero("atlas", "Abrams"),
            hero("ghost", "Lady Geist"),
            hero("hornet", "Vindicta"),
            hero("krill", "Mo & Krill"),
            hero("orion", "Grey Talon"),
            hero("doorman", "The Doorman"),
            hero("wraith", "Wraith"),
        ])
    }

    #[test]
    fn resolves_display_names_codenames_and_aliases() {
        let r = roster();
        for (query, codename) in [
            ("Vindicta", "hornet"),
            ("hornet", "hornet"),
            ("lady geist", "ghost"),
            ("Geist", "ghost"),
            ("grey talon", "orion"),
            ("archer", "orion"),
            ("bull", "atlas"),
            ("Mo & Krill", "krill"),
            ("doorman", "doorman"),
            ("vind", "hornet"),
        ] {
            assert_eq!(r.resolve(query).unwrap().info.codename, codename, "{query}");
        }
    }

    #[test]
    fn unknown_hero_lists_the_roster() {
        let err = roster().resolve("nobody").err().unwrap().to_string();
        assert!(err.contains("Vindicta") && err.contains("Grey Talon"));
    }

    #[test]
    fn owns_index_keys_across_namespaces() {
        let r = roster();
        let owner = |key: &str| r.owner(key).map(|h| h.info.codename.as_str());
        assert_eq!(owner("hero_atlas"), Some("atlas"));
        assert_eq!(owner("abrams"), Some("atlas"));
        assert_eq!(owner("atlas_detective_v2"), Some("atlas"));
        assert_eq!(owner("geist"), Some("ghost"));
        assert_eq!(owner("grey_talon"), Some("orion"));
        assert_eq!(owner("vindicta"), Some("hornet"));
        assert_eq!(owner("hornet_v3"), Some("hornet"));
        assert_eq!(owner("digger"), Some("krill"));
        assert_eq!(owner("wraith_gen_man"), Some("wraith"));
        assert_eq!(owner("npc_shopkeeper"), None);
    }
}
