//! Swap one Deadlock sound clip with user-supplied MP3 audio.
//!
//! Deadlock VO / ability / weapon clips ship as `.vsnd_c` containers: a `CTRL`
//! `KeyValues3` block describing the audio (`m_nRate`, `m_nChannels`, duration,
//! loop points, envelope) followed by the raw MP3 stream appended at the tail.
//! [`morphic::encode_vsnd_c`] mints a new `.vsnd_c` by reusing an existing clip as
//! a *donor* template and substituting fresh MP3 bytes + the matching audio
//! params, exactly the way [`crate::icon`] reuses a texture as a template. Packing
//! the result at the donor's own entry path overrides the clip in place: the
//! soundevent keeps pointing at the same path, the bytes there are now the user's.
//!
//! This is the Foundry sound-swap backbone (drop an MP3 on a hero's sound -> mint
//! -> pack an addon VPK -> install as a managed local mod). The mint technique is
//! in-game-proven (the music-pack / custom-`.vsnd_c` pipeline). v1 takes **MP3**
//! input: the audio params (rate / channels / duration) are parsed from the MP3
//! frame headers in pure Rust, so the tool stays a dependency-free standalone
//! binary (no ffmpeg at runtime). Transcoding other formats to MP3 is a caller
//! concern (a later enhancement can add it here behind a feature).

use std::path::Path;

use anyhow::{bail, Context, Result};
use morphic::VsndParams;

use crate::mp3::{find_first_frame, skip_id3v2, FrameHeader};
use crate::soundevents::SoundEvents;

/// How an event's clip pool is handled by [`swap_event_audio`]. Most hero
/// gameplay events (and many VO barks) are randomizer pools: the soundevent
/// lists N `.vsnd` clips and the engine rolls one per play. A swap has to decide
/// what "swap this sound" means for a pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolPolicy {
    /// Mint the user's audio into **every** clip in the pool and override each in
    /// place. The `.vsndevts_c` is left untouched; the event keeps randomizing but
    /// every option is now your sound, so it always plays. The robust default: no
    /// soundevent edit, survives the clip being referenced from elsewhere.
    ReplaceAll,
    /// Mint the user's audio **once** and rewrite the event's `vsnd_files` to that
    /// single clip, removing randomization. Smaller (one clip, not N copies) but
    /// edits and repacks the `.vsndevts_c`.
    Collapse,
}

/// The set of files an event swap produces, ready to pack into one addon VPK.
pub struct EventSwap {
    /// `(entry_path, bytes)` pairs to pack. For [`PoolPolicy::ReplaceAll`] these
    /// are the minted clips at their `.vsnd_c` paths; for [`PoolPolicy::Collapse`]
    /// the edited `.vsndevts_c` plus the one minted clip.
    pub files: Vec<(String, Vec<u8>)>,
    /// Clip `.vsnd_c` entry paths that were minted (overridden or created).
    pub clips: Vec<String>,
    /// Pool clip paths skipped because their `.vsnd_c` was not in `--from-vpk`
    /// (e.g. referenced from a different pak). Non-fatal; the slot keeps the
    /// original sound. Empty on a clean swap.
    pub skipped: Vec<String>,
    /// Whether the minted clip(s) loop.
    pub looped: bool,
    /// Number of clips the event's pool held.
    pub pool_size: usize,
}

/// Map a soundevent `.vsnd` source path to the compiled `.vsnd_c` VPK entry it is
/// packed under (the engine resolves the source path to the compiled asset).
fn compiled_clip_entry(vsnd_path: &str) -> String {
    if vsnd_path.ends_with(".vsnd_c") {
        vsnd_path.to_owned()
    } else if let Some(stem) = vsnd_path.strip_suffix(".vsnd") {
        format!("{stem}.vsnd_c")
    } else {
        format!("{vsnd_path}.vsnd_c")
    }
}

/// The source `.vsnd` form of a clip path (what `vsnd_files` stores), the inverse
/// of [`compiled_clip_entry`]: `foo.vsnd_c` -> `foo.vsnd`, `foo.vsnd` -> `foo.vsnd`,
/// `foo` -> `foo.vsnd`.
fn source_clip_path(path: &str) -> String {
    if let Some(stem) = path.strip_suffix(".vsnd_c") {
        format!("{stem}.vsnd")
    } else if path.strip_suffix(".vsnd").is_some() {
        path.to_owned()
    } else {
        format!("{path}.vsnd")
    }
}

/// Read an event's `vsnd_files` clip pool out of a decoded soundevents tree.
fn event_pool(se: &SoundEvents, event: &str) -> Result<Vec<String>> {
    use morphic::kv3::Value;
    let Some(event_val) = se.root.get(event) else {
        let mut names = se.event_names();
        names.sort_unstable();
        let preview: Vec<&str> = names.iter().take(20).copied().collect();
        bail!(
            "event {event:?} not found in the soundevents file ({} events; e.g. {})",
            names.len(),
            preview.join(", ")
        );
    };
    let pool: Vec<String> = match event_val.get(KEY_VSND_FILES) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    };
    if pool.is_empty() {
        bail!("event {event:?} has no vsnd_files clips to swap (a template/parameter row)");
    }
    Ok(pool)
}

const KEY_VSND_FILES: &str = "vsnd_files";

/// Swap one soundevent's audio for the user's MP3, producing the files for a
/// single addon VPK. This is the hero-oriented join over [`mint_swapped_clip`]:
/// rather than a single known clip, it takes the **event** (the swap target the
/// catalog surfaces) and resolves its clip pool from the `.vsndevts_c` itself,
/// then applies `policy` (see [`PoolPolicy`]).
///
/// `vpk_path` is the pak the soundevents file and its donor clips are read from
/// (the base `pak01_dir.vpk`). `soundevents_entry` is the `.vsndevts_c` carrying
/// `event` (e.g. `soundevents/hero/gigawatt.vsndevts_c`). `looped_override` is
/// `None` to inherit each donor's own loop flag (auto) or `Some(bool)` to force.
///
/// `collapse_target` applies only to [`PoolPolicy::Collapse`]: `None` reuses the
/// first pool clip's path (overriding it in place); `Some(path)` mints the audio
/// at a **new** clip path and points the event there instead. The custom-path form
/// is the safe way to swap a sound whose clips are **shared** between heroes (e.g.
/// the `sounds/player/melee/shared/...` swing): overriding the shared clip in place
/// changes melee for every hero, but minting a fresh path and repointing only this
/// (hero-specific) event keeps the swap to the one hero.
///
/// # Errors
/// Fails if the soundevents file or any required donor clip cannot be read, if
/// `event` is absent or has no clips, if `mp3` is not MP3, or if every pool clip
/// is missing from `vpk_path`.
pub fn swap_event_audio(
    vpk_path: impl AsRef<Path>,
    soundevents_entry: &str,
    event: &str,
    mp3: &[u8],
    looped_override: Option<bool>,
    policy: PoolPolicy,
    collapse_target: Option<&str>,
) -> Result<EventSwap> {
    let vpk_path = vpk_path.as_ref();
    let mut se = SoundEvents::from_vpk(vpk_path, soundevents_entry)
        .with_context(|| format!("loading soundevents {soundevents_entry}"))?;
    let pool = event_pool(&se, event)?;
    let pool_size = pool.len();

    let vpk =
        valve_pak::open(vpk_path).with_context(|| format!("opening {}", vpk_path.display()))?;
    let read_clip = |entry: &str| -> Result<Vec<u8>> {
        let mut file = vpk
            .get_file(entry)
            .with_context(|| format!("locating donor clip {entry}"))?;
        file.read_all()
            .with_context(|| format!("reading donor clip {entry}"))
    };

    match policy {
        PoolPolicy::Collapse => {
            // The donor template is always the first pool clip (for its envelope /
            // format GUID / loop flag); the mint *target* is either that same path
            // (override in place) or a fresh custom path (hero-specific swap of a
            // shared clip), repointing the event there.
            let donor_entry = compiled_clip_entry(&pool[0]);
            let donor = read_clip(&donor_entry)?;
            let looped = looped_override
                .map_or_else(|| donor_is_looped(&donor), Ok)
                .with_context(|| format!("resolving loop flag from {donor_entry}"))?;
            let minted = mint_swapped_clip(&donor, mp3, looped)
                .with_context(|| format!("minting from {donor_entry}"))?;

            let (src_path, entry) = match collapse_target {
                Some(t) => (source_clip_path(t), compiled_clip_entry(t)),
                None => (source_clip_path(&pool[0]), donor_entry),
            };
            if !se.set_vsnd_files(event, std::slice::from_ref(&src_path)) {
                bail!("failed to rewrite vsnd_files on event {event:?}");
            }
            let encoded = se
                .encode()
                .with_context(|| format!("re-encoding {soundevents_entry}"))?;
            Ok(EventSwap {
                files: vec![
                    (soundevents_entry.to_owned(), encoded),
                    (entry.clone(), minted),
                ],
                clips: vec![entry],
                skipped: Vec::new(),
                looped,
                pool_size,
            })
        }
        PoolPolicy::ReplaceAll => {
            // The loop flag is resolved once (forced, or from the first readable
            // donor) and applied to every minted clip so the pool is uniform.
            let mut files = Vec::new();
            let mut clips = Vec::new();
            let mut skipped = Vec::new();
            let mut looped = looped_override;
            for clip in &pool {
                let entry = compiled_clip_entry(clip);
                let Ok(donor) = read_clip(&entry) else {
                    skipped.push(clip.clone());
                    continue;
                };
                let this_looped = if let Some(l) = looped {
                    l
                } else {
                    let l = donor_is_looped(&donor)
                        .with_context(|| format!("resolving loop flag from {entry}"))?;
                    looped = Some(l);
                    l
                };
                let minted = mint_swapped_clip(&donor, mp3, this_looped)
                    .with_context(|| format!("minting {entry}"))?;
                files.push((entry.clone(), minted));
                clips.push(entry);
            }
            if files.is_empty() {
                bail!(
                    "none of the {pool_size} pool clips for event {event:?} were found in {} \
                     (referenced from another pak?)",
                    vpk_path.display()
                );
            }
            Ok(EventSwap {
                files,
                clips,
                skipped,
                looped: looped.unwrap_or(false),
                pool_size,
            })
        }
    }
}

/// Parse the audio parameters needed to mint a `.vsnd_c` straight from an MP3
/// byte stream: sample rate, channel count, and the total sample count / duration
/// (derived by walking the frame headers, so it is correct for both CBR and VBR).
///
/// `looped` selects whether the minted resource loops (a one-shot VO / ability
/// cast is `false`; music is `true`); it is carried through to [`VsndParams`].
///
/// # Errors
/// Fails if the bytes carry no decodable MPEG audio frame (not an MP3).
pub fn parse_mp3_params(mp3: &[u8], looped: bool) -> Result<VsndParams> {
    let start = skip_id3v2(mp3);
    let mut cursor = find_first_frame(mp3, start)
        .context("input is not MP3 (no MPEG audio frame sync found)")?;

    // Read the first frame for the stream-wide rate + channel count (constant
    // across an MP3), then walk every frame to total the sample count.
    let first = FrameHeader::parse(&mp3[cursor..])
        .context("input is not MP3 (first frame header is invalid)")?;
    let rate = first.sample_rate;
    let channels = first.channels;

    let mut total_samples: u64 = 0;
    while let Some(frame) = mp3.get(cursor..).and_then(FrameHeader::parse) {
        total_samples += u64::from(frame.samples_per_frame);
        let len = frame.frame_len();
        if len == 0 {
            break;
        }
        cursor += len;
    }

    if total_samples == 0 {
        bail!("input is not MP3 (no audio frames decoded)");
    }

    let sample_count = u32::try_from(total_samples).unwrap_or(u32::MAX);
    let duration = f64::from(sample_count) / f64::from(rate);

    Ok(VsndParams {
        rate,
        channels,
        sample_count,
        duration,
        looped,
    })
}

/// Whether a donor `.vsnd_c` clip is authored to loop (reads its
/// `m_vSound.m_nLoopStart`; `-1` = one-shot). A swap can inherit this so a
/// `..._loop` / music clip stays looping and a VO line stays one-shot, instead of
/// asking the caller to know.
///
/// # Errors
/// Fails if `donor` is not a readable `.vsnd_c` (no `CTRL` / `m_vSound`).
pub fn donor_is_looped(donor: &[u8]) -> Result<bool> {
    // Reached via the public `sound` module path rather than a top-level re-export
    // so this commit does not touch morphic/src/lib.rs.
    morphic::sound::vsnd_looped(donor)
        .map_err(|e| anyhow::anyhow!("reading donor loop flag failed: {e}"))
}

/// Mint a replacement `.vsnd_c` from `donor` (a template clip, typically the very
/// clip being overridden, read from the pak) and `mp3` (the user's audio). The
/// returned bytes pack back at the donor's entry path to override the clip in
/// place.
///
/// # Errors
/// Fails if `mp3` is not MP3, or if `donor` is not a mintable `.vsnd_c`
/// (`CVoiceContainerDefault` MP3 shape: a `CTRL` block + appended MP3).
pub fn mint_swapped_clip(donor: &[u8], mp3: &[u8], looped: bool) -> Result<Vec<u8>> {
    let params = parse_mp3_params(mp3, looped)?;
    morphic::encode_vsnd_c(donor, mp3, &params)
        .map_err(|e| anyhow::anyhow!("minting .vsnd_c from the donor failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a CBR MPEG1 Layer III frame (128 kbps, 44100 Hz, stereo) padded to
    /// its computed length with zeros. Header: FF FB 90 00.
    fn mpeg1_l3_frame() -> Vec<u8> {
        // frame_len = 144 * 128000 / 44100 = 417 (no padding).
        let mut f = vec![0xFF, 0xFB, 0x90, 0x00];
        f.resize(417, 0);
        f
    }

    #[test]
    fn parses_cbr_mpeg1_layer3() {
        let mut mp3 = Vec::new();
        for _ in 0..3 {
            mp3.extend_from_slice(&mpeg1_l3_frame());
        }
        let p = parse_mp3_params(&mp3, false).expect("parse");
        assert_eq!(p.rate, 44100);
        assert_eq!(p.channels, 2);
        // 3 frames * 1152 samples each.
        assert_eq!(p.sample_count, 3 * 1152);
        assert!((p.duration - (3.0 * 1152.0 / 44100.0)).abs() < 1e-6);
        assert!(!p.looped);
    }

    #[test]
    fn skips_id3v2_then_parses() {
        let mut mp3 = vec![b'I', b'D', b'3', 4, 0, 0, 0, 0, 0, 5]; // 10-byte header, body size 5
        mp3.extend_from_slice(&[0u8; 5]); // tag body
        mp3.extend_from_slice(&mpeg1_l3_frame());
        let p = parse_mp3_params(&mp3, true).expect("parse");
        assert_eq!(p.rate, 44100);
        assert_eq!(p.sample_count, 1152);
        assert!(p.looped);
    }

    #[test]
    fn rejects_non_mp3() {
        assert!(parse_mp3_params(b"this is not audio at all, no sync here", false).is_err());
    }

    #[test]
    fn compiled_clip_entry_maps_source_to_compiled() {
        // A soundevent `.vsnd` path resolves to the compiled `.vsnd_c` VPK entry.
        assert_eq!(compiled_clip_entry("sounds/a/b.vsnd"), "sounds/a/b.vsnd_c");
        // An already-compiled path is left as-is (idempotent).
        assert_eq!(
            compiled_clip_entry("sounds/a/b.vsnd_c"),
            "sounds/a/b.vsnd_c"
        );
        // A path with no known suffix gets `.vsnd_c` appended.
        assert_eq!(compiled_clip_entry("sounds/a/b"), "sounds/a/b.vsnd_c");
    }

    const SNDEVTS_FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../morphic/fixtures/kv3/gigawatt.vsndevts_c"
    );

    #[test]
    fn event_pool_reads_clips_and_errors_helpfully() {
        let se = SoundEvents::from_file(SNDEVTS_FIXTURE).expect("load fixture");

        // A known randomizer event yields its full clip pool.
        let pool = event_pool(&se, "Seven.Wpn.Fire").expect("pool");
        assert_eq!(pool.len(), 7);
        assert!(pool[0].strip_suffix(".vsnd").is_some());

        // A missing event errors, and the message lists real events to help.
        let err = event_pool(&se, "No.Such.Event").unwrap_err().to_string();
        assert!(err.contains("not found"));
        assert!(err.contains("events; e.g."));
    }
}
