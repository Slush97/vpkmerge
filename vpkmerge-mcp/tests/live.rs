//! End-to-end tool runs against a real Deadlock install.
//!
//! Gated on `DEADLOCK_PAK` pointing at `citadel/pak01_dir.vpk`; CI runs without
//! it and skips. Everything is built into a temp staging dir.
//!
//! ```sh
//! DEADLOCK_PAK=~/.../Deadlock/game/citadel/pak01_dir.vpk cargo test -p vpkmerge-mcp --test live
//! ```

use std::path::PathBuf;

use vpkmerge_mcp::config::Config;
use vpkmerge_mcp::tools::{
    AlphaMode, AudioEdit, BrowseSoundsParams, BrowseTexturesParams, Engine, HeroAnimationsParams,
    HeroTextureList, InspectModParams, ListHeroTexturesParams, ListHeroesParams, LoopMode,
    MapTextureRegionsParams, MergeModsParams, MergePolicy, PoolMode, PreviewAnimationParams,
    PreviewHeroParams, RecolorMode, RecolorVfxParams, RegionMode, ReplaceTexturesParams, SoundKind,
    SwapHeroSoundParams, SwapSoundClipParams, TextureReplacementParam,
};

fn engine() -> Option<(Engine, tempfile::TempDir)> {
    let Ok(pak) = std::env::var("DEADLOCK_PAK") else {
        eprintln!("DEADLOCK_PAK not set; skipping live test");
        return None;
    };
    let staging = tempfile::tempdir().unwrap();
    let config = Config::new(Some(PathBuf::from(pak)), staging.path().to_path_buf());
    Some((Engine::new(config), staging))
}

/// Three silent CBR MPEG1 Layer III frames (128 kbps, 44100 Hz, stereo).
fn tiny_mp3(dir: &std::path::Path) -> String {
    let mut frame = vec![0xFFu8, 0xFB, 0x90, 0x00];
    frame.resize(417, 0);
    let path = dir.join("tiny.mp3");
    std::fs::write(&path, frame.repeat(3)).unwrap();
    path.display().to_string()
}

fn entries(vpk: &str) -> Vec<String> {
    vpkmerge_core::inspect(vpk).unwrap().file_paths
}

fn swap(event: &str, audio_path: &str) -> SwapHeroSoundParams {
    SwapHeroSoundParams {
        event: event.to_owned(),
        audio_path: audio_path.to_owned(),
        hero: None,
        soundevents_entry: None,
        pool: PoolMode::All,
        hero_only: false,
        loop_mode: LoopMode::Auto,
        edit: AudioEdit::default(),
    }
}

#[test]
fn heroes_resolve_by_display_name() {
    let Some((engine, _staging)) = engine() else {
        return;
    };
    let heroes = engine
        .list_heroes(&ListHeroesParams::default())
        .unwrap()
        .heroes;
    assert!(heroes.len() > 30, "only {} playable heroes", heroes.len());
    let vindicta = heroes.iter().find(|h| h.name == "Vindicta").unwrap();
    assert_eq!(vindicta.codename, "hornet");
    assert!(vindicta.vfx_recolor);
    // Roster `orion`, recipe `archer`: the alias join has to bridge them.
    assert!(
        heroes
            .iter()
            .find(|h| h.name == "Grey Talon")
            .unwrap()
            .vfx_recolor
    );
}

#[test]
fn hero_sound_swap_warns_when_clips_are_shared_and_isolates_with_hero_only() {
    // Wraith's slam plays Lash's death-slam clips.
    const EVENT: &str = "Wraith.Telekenisis.Slam";
    let Some((engine, staging)) = engine() else {
        return;
    };
    let found = engine
        .browse_sounds(&BrowseSoundsParams {
            hero: Some("Wraith".to_owned()),
            search: Some("slam".to_owned()),
            kind: SoundKind::Gameplay,
            limit: None,
            include_clips: true,
        })
        .unwrap();
    let row = found
        .sounds
        .iter()
        .find(|s| s.event == EVENT)
        .expect("the slam event in browse_sounds");
    assert_eq!(row.hero.as_deref(), Some("Wraith"));
    assert_eq!(row.soundevents_entry, "soundevents/hero/wraith.vsndevts_c");
    assert_eq!(row.clips.as_ref().unwrap().len(), row.clip_count);

    let mp3 = tiny_mp3(staging.path());

    // Default pool mode overrides the shared clips in place and says so.
    let all = engine.swap_hero_sound(&swap(EVENT, &mp3)).unwrap();
    assert_eq!(all.clips_minted + all.skipped.len(), all.pool_size);
    assert!(all.note.as_deref().is_some_and(|n| n.contains("heroOnly")));
    assert!(entries(&all.staged_vpk)
        .iter()
        .all(|e| e.ends_with(".vsnd_c")));

    // heroOnly repoints Wraith's event at a fresh clip and touches nothing shared.
    let only = engine
        .swap_hero_sound(&SwapHeroSoundParams {
            hero_only: true,
            ..swap(&EVENT.to_lowercase(), &mp3)
        })
        .unwrap();
    assert!(only.note.is_none());
    let mut packed = entries(&only.staged_vpk);
    packed.sort();
    assert_eq!(
        packed,
        [
            "soundevents/hero/wraith.vsndevts_c",
            "sounds/vpkmerge/wraith-telekenisis-slam.vsnd_c"
        ]
    );

    // One clip of the pool, addressed by its entry.
    let clip = engine
        .swap_sound_clip(&SwapSoundClipParams {
            clip_entry: row.clips.as_ref().unwrap()[0].clone(),
            audio_path: mp3,
            loop_mode: LoopMode::Auto,
            edit: AudioEdit::default(),
        })
        .unwrap();
    assert_eq!(entries(&clip.staged_vpk), [clip.clip_entry]);
}

#[test]
fn melee_swing_is_a_shared_sound() {
    let Some((engine, staging)) = engine() else {
        return;
    };
    let search = |hero: Option<&str>| {
        engine
            .browse_sounds(&BrowseSoundsParams {
                hero: hero.map(str::to_owned),
                search: Some("melee swing".to_owned()),
                ..BrowseSoundsParams::default()
            })
            .unwrap()
    };
    // Per hero there is nothing, and the note says where to look instead.
    let for_seven = search(Some("Seven"));
    assert_eq!(for_seven.total, 0);
    assert!(for_seven.note.unwrap().contains("without hero"));

    let shared = search(None);
    let row = shared
        .sounds
        .iter()
        .find(|s| s.event == "Hero.Default.Melee.Swing")
        .expect("the shared melee swing");
    assert_eq!(row.kind, "shared");
    assert!(row.is_pool);

    let mp3 = tiny_mp3(staging.path());
    let swapped = engine.swap_hero_sound(&swap(&row.event, &mp3)).unwrap();
    assert_eq!(swapped.clips_minted, row.clip_count);
    assert!(swapped.note.unwrap().contains("every hero"));

    let err = engine
        .swap_hero_sound(&SwapHeroSoundParams {
            hero_only: true,
            ..swap(&row.event, &mp3)
        })
        .unwrap_err();
    assert!(format!("{err:#}").contains("no per-hero version"));
}

#[test]
fn mixed_case_pool_clips_are_found() {
    let Some((engine, staging)) = engine() else {
        return;
    };
    // This pool's `vsnd_files` say `groupA-001`; the pak stores `groupa-001`.
    let mp3 = tiny_mp3(staging.path());
    let swapped = engine
        .swap_hero_sound(&swap("Wraith.Wpn.Whizby", &mp3))
        .unwrap();
    assert!(swapped.skipped.is_empty(), "skipped {:?}", swapped.skipped);
    assert_eq!(swapped.clips_minted, swapped.pool_size);
}

#[test]
fn voice_lines_are_searchable_and_swappable() {
    let Some((engine, staging)) = engine() else {
        return;
    };
    let found = engine
        .browse_sounds(&BrowseSoundsParams {
            hero: Some("Abrams".to_owned()),
            kind: SoundKind::Voiceline,
            limit: Some(5),
            ..BrowseSoundsParams::default()
        })
        .unwrap();
    assert!(found.total > 100, "only {} Abrams voice lines", found.total);
    assert!(found.note.is_some());
    let line = &found.sounds[0];
    assert_eq!(line.hero.as_deref(), Some("Abrams"));

    let mp3 = tiny_mp3(staging.path());
    let swapped = engine.swap_hero_sound(&swap(&line.event, &mp3)).unwrap();
    assert_eq!(swapped.soundevents_entry, line.soundevents_entry);
    assert!(swapped.clips_minted > 0);
}

#[test]
fn unknown_names_point_at_the_discovery_tool() {
    let Some((engine, staging)) = engine() else {
        return;
    };
    let mp3 = tiny_mp3(staging.path());
    let err = engine
        .swap_hero_sound(&swap("No.Such.Event", &mp3))
        .unwrap_err();
    assert!(format!("{err:#}").contains("browse_sounds"));

    let err = engine
        .browse_sounds(&BrowseSoundsParams {
            hero: Some("Nobody".to_owned()),
            ..BrowseSoundsParams::default()
        })
        .unwrap_err();
    assert!(format!("{err:#}").contains("Vindicta"));
}

#[test]
fn hero_card_replacement_round_trips_through_a_thumbnail() {
    let Some((engine, _staging)) = engine() else {
        return;
    };
    let found = engine
        .browse_textures(&BrowseTexturesParams {
            category: Some("hero-image".to_owned()),
            hero: Some("Vindicta".to_owned()),
            search: Some("card".to_owned()),
            limit: Some(2),
            thumbnails: true,
        })
        .unwrap();
    assert!(
        found.total >= 2,
        "only {} Vindicta card textures",
        found.total
    );
    let replacements: Vec<TextureReplacementParam> = found
        .textures
        .iter()
        .map(|t| TextureReplacementParam {
            entry: t.path.clone(),
            png_path: t.thumbnail_path.clone().expect("thumbnail written"),
            mask_path: None,
            alpha: AlphaMode::Auto,
        })
        .collect();

    let built = engine
        .replace_textures(&ReplaceTexturesParams {
            replacements,
            name: Some("Vindicta card".to_owned()),
        })
        .unwrap();
    assert!(built.staged_vpk.ends_with("texture-vindicta-card_dir.vpk"));
    assert!(built.written.iter().all(|w| w.alpha == "png"));
    let mut packed = entries(&built.staged_vpk);
    packed.sort();
    let mut expected: Vec<String> = found.textures.iter().map(|t| t.path.clone()).collect();
    expected.sort();
    assert_eq!(packed, expected);
}

fn vindicta_textures(engine: &Engine, material: Option<&str>) -> HeroTextureList {
    engine
        .list_hero_textures(&ListHeroTexturesParams {
            hero: "Vindicta".to_owned(),
            material: material.map(str::to_owned),
            vpk: None,
        })
        .unwrap()
}

#[test]
fn hero_textures_list_the_live_model_materials() {
    let Some((engine, _staging)) = engine() else {
        return;
    };
    let listed = vindicta_textures(&engine, None);
    assert!(listed.model.ends_with("hornet.vmdl_c"), "{}", listed.model);
    assert!(listed.slots.contains_key("g_tNormalRoughness"));
    let dress = listed
        .materials
        .iter()
        .find(|m| m.material.contains("vindicta_dress"))
        .expect("dress material");
    assert!(dress
        .engine_defaults
        .iter()
        .any(|s| s == "g_tAltColor_Reveal"));
    let albedo = dress
        .textures
        .iter()
        .find(|t| t.slot == "g_tColor")
        .expect("dress albedo");
    assert_eq!(albedo.size.as_deref(), Some("2048x2048"));
    assert!(albedo.uniform.is_none());
    // The bare legs draw a placeholder albedo tinted by vertex colors.
    let skin = listed
        .materials
        .iter()
        .find(|m| m.material.ends_with("/vindicta_head.vmat_c"))
        .expect("skin material");
    assert!(skin.vertex_color);
    assert!(!listed.notes.is_empty());
}

#[test]
fn hero_texture_paints_through_a_region_mask() {
    let Some((engine, staging)) = engine() else {
        return;
    };
    let listed = vindicta_textures(&engine, Some("dress"));
    let albedo = listed.materials[0]
        .textures
        .iter()
        .find(|t| t.slot == "g_tColor")
        .expect("dress albedo");

    let map = |select: Vec<usize>| {
        engine
            .map_texture_regions(&MapTextureRegionsParams {
                hero: "Vindicta".to_owned(),
                texture: albedo.path.clone(),
                by: RegionMode::Bone,
                select,
                padding: None,
                limit: None,
                vpk: None,
            })
            .unwrap()
    };
    let regions = map(Vec::new());
    assert_eq!(regions.size, "2048x2048");
    assert!(regions.mask.is_none());
    assert!(
        regions.alpha_png.is_some(),
        "the dress albedo alpha is a data channel"
    );
    let leg = regions
        .regions
        .iter()
        .find(|r| r.label == "leg_upper_L")
        .expect("a leg region");
    assert!(regions.regions.iter().any(|r| r.label == "arm_upper_R"));

    let masked = map(vec![leg.id]);
    let mask = masked.mask.expect("mask baked");
    assert!(mask.coverage_percent > 1.0);
    let mask_img = image::open(&mask.mask_png).unwrap().to_luma8();
    assert_eq!(mask_img.dimensions(), (2048, 2048));

    let red = staging.path().join("red.png");
    image::RgbImage::from_pixel(2048, 2048, image::Rgb([255, 0, 0]))
        .save(&red)
        .unwrap();
    let built = engine
        .replace_textures(&ReplaceTexturesParams {
            replacements: vec![TextureReplacementParam {
                entry: albedo.path.clone(),
                png_path: red.display().to_string(),
                mask_path: Some(mask.mask_png.clone()),
                alpha: AlphaMode::Auto,
            }],
            name: Some("vindicta red leg".to_owned()),
        })
        .unwrap();
    assert_eq!(built.written[0].alpha, "original");
    assert!(built.written[0].masked);

    // Inside the mask the texture turns red; outside it keeps the original.
    let rebuilt = vpkmerge_core::read_vpk_entry(&built.staged_vpk, &albedo.path).unwrap();
    let png = vpkmerge_core::thumbnail_png(&rebuilt, 2048).unwrap().png;
    let rebuilt = image::load_from_memory(&png).unwrap().to_rgba8();
    let original = image::open(&regions.texture_png).unwrap().to_rgb8();
    let (mut inside, mut outside) = (None, None);
    for (x, y, m) in mask_img.enumerate_pixels() {
        if m.0[0] == 255 && inside.is_none() && x % 4 == 2 && y % 4 == 2 {
            inside = Some((x, y));
        }
        if m.0[0] == 0 && outside.is_none() && original.get_pixel(x, y).0 != [0, 0, 0] {
            outside = Some((x, y));
        }
    }
    let (ix, iy) = inside.unwrap();
    let px = rebuilt.get_pixel(ix, iy).0;
    assert!(
        px[0] > 200 && px[1] < 60 && px[2] < 60,
        "masked texel {px:?}"
    );
    let (ox, oy) = outside.unwrap();
    let (now, was) = (rebuilt.get_pixel(ox, oy).0, original.get_pixel(ox, oy).0);
    assert!(
        (0..3).all(|c| now[c].abs_diff(was[c]) <= 24),
        "unmasked texel changed: {was:?} -> {now:?}"
    );
}

#[test]
fn vfx_recolor_builds_previews_and_merges() {
    let Some((engine, _staging)) = engine() else {
        return;
    };
    let recolor = |mode, hue, preview_only| RecolorVfxParams {
        hero: "Paige".to_owned(),
        mode,
        hue,
        saturation: None,
        brightness: None,
        animated: false,
        preview_only,
    };

    let preview = engine
        .recolor_hero_vfx(&recolor(RecolorMode::Solid, Some(280.0), true))
        .unwrap();
    let png = std::fs::read(preview.preview_png.unwrap()).unwrap();
    assert_eq!(&png[1..4], b"PNG");

    let solid = engine
        .recolor_hero_vfx(&recolor(RecolorMode::Solid, Some(280.0), false))
        .unwrap();
    assert_eq!(solid.hero, "Paige");
    assert!(solid.particles > 0 && solid.entries >= solid.particles);
    let solid_vpk = solid.staged_vpk.unwrap();

    let prism = engine
        .recolor_hero_vfx(&recolor(RecolorMode::Prism, None, false))
        .unwrap();
    let prism_vpk = prism.staged_vpk.unwrap();
    assert_ne!(solid_vpk, prism_vpk);

    let err = engine
        .recolor_hero_vfx(&recolor(RecolorMode::Solid, None, false))
        .unwrap_err();
    assert!(format!("{err:#}").contains("hue"));

    // Both recolor the same files, so they collide entry for entry.
    let inspection = Engine::inspect_mod(&InspectModParams {
        vpk: solid_vpk.clone(),
        against: vec![prism_vpk.clone()],
        limit: Some(5),
    })
    .unwrap();
    assert_eq!(inspection.entry_count, solid.entries);
    assert!(inspection.conflict_count.unwrap() > 0);
    assert_eq!(inspection.entries.len(), 5);

    let merged = engine
        .merge_mods(&MergeModsParams {
            inputs: vec![solid_vpk.clone(), prism_vpk],
            out_path: None,
            policy: MergePolicy::LastWins,
        })
        .unwrap();
    assert_eq!(
        merged.conflicts_resolved,
        inspection.conflict_count.unwrap()
    );

    let strict = engine.merge_mods(&MergeModsParams {
        inputs: vec![solid_vpk.clone(), solid_vpk],
        out_path: None,
        policy: MergePolicy::Strict,
    });
    assert!(strict.is_err());
}

#[test]
fn hero_preview_opens_in_a_pose_its_animations_can_replace() {
    let Some((engine, _staging)) = engine() else {
        return;
    };
    let preview = engine
        .preview_hero(&PreviewHeroParams {
            hero: "Mina".to_owned(),
            vpk: None,
        })
        .unwrap();
    assert_eq!(preview.hero, "Mina");
    assert_eq!(preview.pose.as_deref(), Some("vampirebat_hero_pose"));
    let bytes = std::fs::read(&preview.preview_glb).unwrap();
    assert_eq!(&bytes[..4], b"glTF");

    let listed = engine
        .list_hero_animations(&HeroAnimationsParams {
            hero: "Mina".to_owned(),
            vpk: None,
        })
        .unwrap();
    assert_eq!(listed.codename, "vampirebat");
    // The loose NM clips hold the move set; the additive aim layer is left out.
    for name in [
        "vampirebat_hero_pose",
        "vampirebat_ability_ult_start",
        "item_run_n",
    ] {
        assert!(
            listed.animations.iter().any(|a| a == name),
            "{name} missing"
        );
    }
    assert!(!listed.animations.iter().any(|a| a == "aim_idle"));

    let clip = engine
        .preview_hero_animation(&PreviewAnimationParams {
            hero: "Mina".to_owned(),
            vpk: None,
            animation: "item_run_n".to_owned(),
        })
        .unwrap();
    let bytes = std::fs::read(&clip.animation_glb).unwrap();
    assert_eq!(&bytes[..4], b"glTF");
    assert!(
        bytes.len() < 2_000_000,
        "skeleton-only clip is {} bytes",
        bytes.len()
    );
}
