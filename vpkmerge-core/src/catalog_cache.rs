//! On-disk cache for the Foundry catalog indexes, invalidated by game build.
//!
//! Building the catalog is not free: the voice-line index scans every
//! `soundevents/vo/*.vsndevts_c` (~76K events on the live pak), and a UI wants it
//! ready at launch, not after a multi-second scan. This module persists a built
//! index to disk tagged with a [`BuildFingerprint`] of the source VPK, and serves
//! it back on the next run when the pak is unchanged.
//!
//! **Invalidation.** Steam rewrites `citadel/pak01_dir.vpk` whenever an update
//! touches the pak (the same property the chunk-mtime update-diff tooling relies
//! on: a patch rewrites the files it changes). So the fingerprint is just the
//! `_dir.vpk` byte length plus its modification time. This is cheap (a single
//! `stat`, no open) and never serves stale data after a real update; the only
//! cost is an occasional needless rebuild if something touches the file's mtime
//! without changing its bytes (a "verify game files" pass). A
//! [`CACHE_SCHEMA_VERSION`] bump invalidates every cache when the on-disk shape
//! changes.
//!
//! The high-level entry points are [`CatalogCache::voicelines`] and
//! [`CatalogCache::textures`]: load-or-build-and-store in one call.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::catalog::{build_voiceline_index, VoiceLine};
use crate::texture_catalog::{build_texture_index, TextureEntry};

/// On-disk format version. Bump when the cached JSON shape changes so older
/// caches are treated as a miss instead of mis-parsed.
pub const CACHE_SCHEMA_VERSION: u32 = 2;

/// Cache-file stem for the voice-line index.
const KIND_VOICELINE: &str = "voiceline";
/// Cache-file stem for the texture / icon index.
const KIND_TEXTURE: &str = "texture";

/// Identifies a specific build of a source VPK by the `_dir.vpk` file's size and
/// modification time. Two fingerprints compare equal iff the pak is (to a `stat`)
/// the same build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildFingerprint {
    /// `_dir.vpk` byte length.
    pub vpk_len: u64,
    /// `_dir.vpk` mtime in whole seconds since the Unix epoch (signed: pre-epoch
    /// mtimes are theoretically possible).
    pub vpk_mtime_secs: i64,
    /// Sub-second part of the mtime, in nanoseconds.
    pub vpk_mtime_nanos: u32,
}

impl BuildFingerprint {
    /// Stat `vpk_path` and capture its build fingerprint. Does not open or parse
    /// the VPK.
    pub fn for_vpk(vpk_path: impl AsRef<Path>) -> Result<Self> {
        let vpk_path = vpk_path.as_ref();
        let meta =
            std::fs::metadata(vpk_path).with_context(|| format!("stat {}", vpk_path.display()))?;
        let mtime = meta
            .modified()
            .with_context(|| format!("reading mtime of {}", vpk_path.display()))?;
        let (secs, nanos) = match mtime.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => (
                i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
                d.subsec_nanos(),
            ),
            // mtime before the epoch.
            Err(e) => {
                let d = e.duration();
                (
                    i64::try_from(d.as_secs()).map_or(i64::MIN, |s| -s),
                    d.subsec_nanos(),
                )
            }
        };
        Ok(Self {
            vpk_len: meta.len(),
            vpk_mtime_secs: secs,
            vpk_mtime_nanos: nanos,
        })
    }
}

/// The on-disk wrapper around a cached index: a schema tag, the build it was
/// built from, and the items.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEnvelope<T> {
    schema: u32,
    kind: String,
    fingerprint: BuildFingerprint,
    items: T,
}

/// A directory holding cached catalog indexes (one JSON file per kind). Cheap to
/// construct; the directory is created lazily on the first store.
#[derive(Debug, Clone)]
pub struct CatalogCache {
    dir: PathBuf,
}

impl CatalogCache {
    /// Use `dir` as the cache directory. Nothing touches the filesystem until a
    /// store or load runs.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The cache directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_for(&self, kind: &str) -> PathBuf {
        self.dir.join(format!("{kind}.json"))
    }

    /// Load a cached index of `kind` if one exists whose schema and fingerprint
    /// match `fingerprint`. Returns `Ok(None)` on a miss (no file, wrong schema,
    /// stale fingerprint, or an unreadable/corrupt file: a bad cache is a miss,
    /// not an error, so a UI silently rebuilds).
    #[must_use]
    pub fn load<T: DeserializeOwned>(
        &self,
        kind: &str,
        fingerprint: &BuildFingerprint,
    ) -> Option<T> {
        let path = self.path_for(kind);
        let bytes = std::fs::read(&path).ok()?;
        let envelope: CacheEnvelope<T> = serde_json::from_slice(&bytes).ok()?;
        if envelope.schema == CACHE_SCHEMA_VERSION
            && envelope.kind == kind
            && &envelope.fingerprint == fingerprint
        {
            Some(envelope.items)
        } else {
            None
        }
    }

    /// Write `items` to the cache for `kind`, tagged with `fingerprint`. Creates
    /// the cache directory if needed. Writes atomically (temp file + rename) so a
    /// crash mid-write never leaves a half-written cache that would parse wrong.
    pub fn store<T: Serialize>(
        &self,
        kind: &str,
        fingerprint: &BuildFingerprint,
        items: &T,
    ) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating cache dir {}", self.dir.display()))?;
        let envelope = CacheEnvelope {
            schema: CACHE_SCHEMA_VERSION,
            kind: kind.to_owned(),
            fingerprint: fingerprint.clone(),
            items,
        };
        let json = serde_json::to_vec(&envelope).context("serializing cache envelope")?;

        let dest = self.path_for(kind);
        let tmp = self.dir.join(format!("{kind}.json.tmp"));
        std::fs::write(&tmp, &json).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &dest)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), dest.display()))?;
        Ok(())
    }

    /// Load the voice-line index for `vpk_path` from cache, or build it (see
    /// [`build_voiceline_index`]) and cache it for next time. The boolean is
    /// `true` on a cache hit.
    pub fn voicelines_cached(&self, vpk_path: impl AsRef<Path>) -> Result<(Vec<VoiceLine>, bool)> {
        let vpk_path = vpk_path.as_ref();
        let fingerprint = BuildFingerprint::for_vpk(vpk_path)?;
        if let Some(items) = self.load::<Vec<VoiceLine>>(KIND_VOICELINE, &fingerprint) {
            return Ok((items, true));
        }
        let items = build_voiceline_index(vpk_path)?;
        self.store(KIND_VOICELINE, &fingerprint, &items)?;
        Ok((items, false))
    }

    /// Load-or-build the voice-line index, discarding the hit/miss flag.
    pub fn voicelines(&self, vpk_path: impl AsRef<Path>) -> Result<Vec<VoiceLine>> {
        Ok(self.voicelines_cached(vpk_path)?.0)
    }

    /// Load the texture / icon index for `vpk_path` from cache, or build it (see
    /// [`build_texture_index`]) and cache it. The boolean is `true` on a hit.
    pub fn textures_cached(&self, vpk_path: impl AsRef<Path>) -> Result<(Vec<TextureEntry>, bool)> {
        let vpk_path = vpk_path.as_ref();
        let fingerprint = BuildFingerprint::for_vpk(vpk_path)?;
        if let Some(items) = self.load::<Vec<TextureEntry>>(KIND_TEXTURE, &fingerprint) {
            return Ok((items, true));
        }
        let items = build_texture_index(vpk_path)?;
        self.store(KIND_TEXTURE, &fingerprint, &items)?;
        Ok((items, false))
    }

    /// Load-or-build the texture / icon index, discarding the hit/miss flag.
    pub fn textures(&self, vpk_path: impl AsRef<Path>) -> Result<Vec<TextureEntry>> {
        Ok(self.textures_cached(vpk_path)?.0)
    }

    /// Delete every cache file in the directory (voiceline + texture). A missing
    /// file is not an error. Use to force a rebuild.
    pub fn clear(&self) -> Result<()> {
        for kind in [KIND_VOICELINE, KIND_TEXTURE] {
            let path = self.path_for(kind);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture_catalog::TextureCategory;

    fn sample_textures() -> Vec<TextureEntry> {
        vec![
            TextureEntry {
                path: "panorama/images/hud/abilities/astro/shotgun_psd.vtex_c".to_owned(),
                category: TextureCategory::AbilityIcon,
                hero: Some("astro".to_owned()),
                label: "shotgun".to_owned(),
            },
            TextureEntry {
                path: "materials/brick/brick_color_tga_1234abcd.vtex_c".to_owned(),
                category: TextureCategory::Other,
                hero: None,
                label: "brick color".to_owned(),
            },
        ]
    }

    fn fp(len: u64) -> BuildFingerprint {
        BuildFingerprint {
            vpk_len: len,
            vpk_mtime_secs: 1_700_000_000,
            vpk_mtime_nanos: 42,
        }
    }

    #[test]
    fn store_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(dir.path());
        let items = sample_textures();
        let fingerprint = fp(100);

        assert!(cache
            .load::<Vec<TextureEntry>>(KIND_TEXTURE, &fingerprint)
            .is_none());
        cache.store(KIND_TEXTURE, &fingerprint, &items).unwrap();
        let loaded = cache
            .load::<Vec<TextureEntry>>(KIND_TEXTURE, &fingerprint)
            .expect("hit");
        assert_eq!(loaded, items);
    }

    #[test]
    fn fingerprint_mismatch_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(dir.path());
        cache
            .store(KIND_TEXTURE, &fp(100), &sample_textures())
            .unwrap();
        // A different build (length changed) does not resolve.
        assert!(cache
            .load::<Vec<TextureEntry>>(KIND_TEXTURE, &fp(200))
            .is_none());
    }

    #[test]
    fn wrong_schema_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(dir.path());
        let fingerprint = fp(100);
        // Hand-write an envelope with a future schema version.
        let bogus = serde_json::json!({
            "schema": CACHE_SCHEMA_VERSION + 1,
            "kind": KIND_TEXTURE,
            "fingerprint": fingerprint,
            "items": sample_textures(),
        });
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(
            dir.path().join(format!("{KIND_TEXTURE}.json")),
            serde_json::to_vec(&bogus).unwrap(),
        )
        .unwrap();
        assert!(cache
            .load::<Vec<TextureEntry>>(KIND_TEXTURE, &fingerprint)
            .is_none());
    }

    #[test]
    fn corrupt_cache_is_a_miss_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(dir.path());
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join(format!("{KIND_TEXTURE}.json")), b"not json").unwrap();
        assert!(cache
            .load::<Vec<TextureEntry>>(KIND_TEXTURE, &fp(100))
            .is_none());
    }

    #[test]
    fn clear_removes_caches_and_tolerates_missing() {
        let dir = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(dir.path());
        cache.clear().unwrap(); // nothing there yet: fine
        cache
            .store(KIND_TEXTURE, &fp(100), &sample_textures())
            .unwrap();
        assert!(dir.path().join("texture.json").exists());
        cache.clear().unwrap();
        assert!(!dir.path().join("texture.json").exists());
    }
}
