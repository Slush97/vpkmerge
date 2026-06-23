//! Live end-to-end test for the hero-oriented event swap (`swap_event_audio`).
//!
//! Gated on `DEADLOCK_PAK` pointing at a real `citadel/pak01_dir.vpk`, because the
//! swap reads a soundevents file plus its donor `.vsnd_c` clips out of the pak (no
//! `.vsnd_c` is committed as a fixture). CI runs without the env var and skips.
//!
//! ```sh
//! DEADLOCK_PAK=~/.../Deadlock/game/citadel/pak01_dir.vpk \
//!   cargo test -p vpkmerge-core --test soundswap_event_live -- --nocapture
//! ```

use vpkmerge_core::PoolPolicy;

/// A short MP3 (CBR MPEG1 Layer III, 128 kbps / 44100 / stereo) to swap in. Three
/// silent frames is enough for the minter to parse rate/channels/duration.
fn tiny_mp3() -> Vec<u8> {
    let mut frame = vec![0xFFu8, 0xFB, 0x90, 0x00];
    frame.resize(417, 0); // 144 * 128000 / 44100 = 417 bytes, no padding
    let mut mp3 = Vec::new();
    for _ in 0..3 {
        mp3.extend_from_slice(&frame);
    }
    mp3
}

#[test]
fn replace_all_and_collapse_against_live_pak() {
    // A randomizer pool whose clips ship in pak01 (some events, e.g. the whizby
    // pools, reference clips that live in a different pak; those would land in
    // `skipped` instead). This one is a 3-clip pool packed in the base pak.
    const SNDEVTS: &str = "soundevents/hero/gigawatt.vsndevts_c";
    const EVENT: &str = "Gigawatt.StaticCharge.Proc.Other";

    let Ok(pak) = std::env::var("DEADLOCK_PAK") else {
        eprintln!("DEADLOCK_PAK not set; skipping live event-swap test");
        return;
    };
    let mp3 = tiny_mp3();

    // ReplaceAll: every pool clip is minted (overridden in place), the soundevents
    // file is left out of the output.
    let all = vpkmerge_core::swap_event_audio(
        &pak,
        SNDEVTS,
        EVENT,
        &mp3,
        None,
        PoolPolicy::ReplaceAll,
        None,
    )
    .expect("replace-all swap");
    assert!(
        all.pool_size > 1,
        "the test event should be a randomizer pool"
    );
    assert_eq!(all.clips.len(), all.pool_size - all.skipped.len());
    assert!(all.files.iter().all(|(e, _)| e.ends_with(".vsnd_c")));
    assert!(
        !all.files.iter().any(|(e, _)| e.ends_with(".vsndevts_c")),
        "replace-all must not touch the soundevents file"
    );

    // Collapse: one minted clip + the rewritten soundevents file, whose vsnd_files
    // now lists exactly that single clip.
    let col = vpkmerge_core::swap_event_audio(
        &pak,
        SNDEVTS,
        EVENT,
        &mp3,
        None,
        PoolPolicy::Collapse,
        None,
    )
    .expect("collapse swap");
    assert_eq!(col.clips.len(), 1);
    assert_eq!(col.files.len(), 2);
    assert!(col.files.iter().any(|(e, _)| e == SNDEVTS));
    assert!(col.files.iter().any(|(e, _)| e.ends_with(".vsnd_c")));
}
