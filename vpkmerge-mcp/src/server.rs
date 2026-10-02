//! MCP glue: tool names, the descriptions the model reads, and the hop onto a
//! blocking thread. The behavior lives in `tools.rs`.

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::{tool, tool_handler, tool_router, Json, ServerHandler};

use crate::config::Config;
use crate::tools::{
    BrowseSoundsParams, BrowseTexturesParams, DocSearch, Engine, GameStatus, GrimoireHandoff,
    HeroAnimationClip, HeroAnimations, HeroAnimationsParams, HeroList, HeroPreview, HeroSoundSwap,
    HeroTextureList, InspectModParams, ListHeroTexturesParams, ListHeroesParams, LouderParams,
    LouderSound, MapTextureRegionsParams, MergeModsParams, MergedMod, ModInspection,
    OpenInGrimoireParams, PreviewAnimationParams, PreviewHeroParams, ReadDocParams,
    RecolorVfxParams, ReplaceTexturesParams, SearchDocsParams, SoundClipSwap, SoundList,
    SwapHeroSoundParams, SwapSoundClipParams, TextureList, TextureMod, TextureRegions, VfxRecolor,
};

const INSTRUCTIONS: &str = "\
Deadlock modding tools. They read the user's local Deadlock install and build mods (addon \
VPK files) into a staging folder. They never install anything into the game.

Workflow:
1. Discover first, never guess names. list_heroes translates hero names. browse_sounds, \
browse_textures and list_hero_textures return the exact `event` and `path` values the build \
tools need.
2. Build. swap_hero_sound, swap_sound_clip, replace_textures, recolor_hero_vfx and merge_mods \
each write one `*_dir.vpk` to staging and return its path.
3. Tell the user the staged path and what changed. Installing is their call. When they want it \
installed, open_in_grimoire hands the mod to the Grimoire mod manager, where they confirm. \
Without Grimoire, copy the file into the addons folder under the next free `pakNN_dir.vpk` name \
(see game_status), but never while a mod manager is running: it renumbers that folder. Do not \
copy files into the game unless the user asks you to.
4. When a mod changes how a hero looks, call preview_hero with its path so the user can see it.

Repainting part of a hero: list_hero_textures to find the texture, map_texture_regions to see \
its regions (view overlayPng) and bake a mask of the ones to change, edit texturePng, then \
replace_textures with the mask. Edit in the texture's own layout; hero UVs are mirrored and \
overlap, so a 3D bake onto them breaks.

Heroes can be named by display name (\"Vindicta\") everywhere. File paths must be absolute. \
Audio input must be MP3, images PNG.

When you are unsure how Deadlock modding works (load order, file formats, what is possible, \
how to test in game), call search_docs and then read_doc before answering or building.

If a tool reports that the game was not found, call game_status and relay its fix.";

#[derive(Clone)]
pub struct Server {
    engine: Arc<Engine>,
}

impl Server {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            engine: Arc::new(Engine::new(config)),
        }
    }

    /// Run engine work off the async runtime. A failure becomes a tool error the
    /// model can read and recover from, not a protocol error.
    async fn run<T, F>(&self, work: F) -> Result<T, String>
    where
        F: FnOnce(&Engine) -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let engine = Arc::clone(&self.engine);
        match tokio::task::spawn_blocking(move || work(&engine)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(format!("{e:#}")),
            Err(e) => Err(format!("tool crashed: {e}")),
        }
    }
}

#[tool_router]
impl Server {
    #[tool(
        description = "Report whether the Deadlock install was found, where built mods are \
            staged, which addon slots (pakNN_dir.vpk) are installed, and the next free slot. \
            Call this when a tool says the game was not found, or before telling the user how \
            to install a mod.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn game_status(&self) -> Result<Json<GameStatus>, String> {
        self.run(|e| Ok(e.game_status())).await.map(Json)
    }

    #[tool(
        description = "List Deadlock heroes with display name, internal codename, and whether \
            recolor_hero_vfx supports them. Use it to translate between names (Vindicta is \
            `hornet`, Mina is `vampirebat`) instead of guessing.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_heroes(
        &self,
        Parameters(p): Parameters<ListHeroesParams>,
    ) -> Result<Json<HeroList>, String> {
        self.run(move |e| e.list_heroes(&p)).await.map(Json)
    }

    #[tool(
        description = "Search the game's sound events to find what to replace: a hero's own \
            weapon, ability and movement sounds (gameplay); sounds every hero shares such as \
            melee, parry and footsteps, plus items, NPCs, UI and music (shared); and hero \
            voice lines. Returns the exact `event` name swap_hero_sound needs. `isPool` means \
            the event picks one of several clips at random. Examples: hero \"Seven\", search \
            \"fire\"; or no hero, search \"melee swing\".",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn browse_sounds(
        &self,
        Parameters(p): Parameters<BrowseSoundsParams>,
    ) -> Result<Json<SoundList>, String> {
        self.run(move |e| e.browse_sounds(&p)).await.map(Json)
    }

    #[tool(
        description = "Search the game's textures to find an image to replace: ability icons, \
            item icons, hero portraits and cards, hero skin textures, ability effect textures. \
            Returns the exact `path` replace_textures needs. Set thumbnails to get a PNG of \
            each result. For the textures a hero's body actually renders, use \
            list_hero_textures. Example: category \"hero-image\", hero \"Vindicta\", search \
            \"card\".",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn browse_textures(
        &self,
        Parameters(p): Parameters<BrowseTexturesParams>,
    ) -> Result<Json<TextureList>, String> {
        self.run(move |e| e.browse_textures(&p)).await.map(Json)
    }

    #[tool(
        description = "List every texture a hero's in-game body model samples, per material \
            (dress, hair, head, gun, ...): slot, size, format, alpha range, and whether it is \
            a uniform placeholder or shared with another material. `slots` explains each \
            slot's channels. Filter with material, e.g. \"dress\".",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_hero_textures(
        &self,
        Parameters(p): Parameters<ListHeroTexturesParams>,
    ) -> Result<Json<HeroTextureList>, String> {
        self.run(move |e| e.list_hero_textures(&p)).await.map(Json)
    }

    #[tool(
        description = "Split one hero texture into regions and say where each sits on it: \
            by bone (body areas such as head, arm_upper_L), UV island, or mesh part. Writes \
            the texture as PNG and an overlay PNG with each region's id drawn on it; returns \
            the largest regions with coverage, texel bounds and the bones that skin them. Pass \
            select with region ids to also bake a white-on-black mask at full size for \
            replace_textures. Get `texture` from list_hero_textures.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn map_texture_regions(
        &self,
        Parameters(p): Parameters<MapTextureRegionsParams>,
    ) -> Result<Json<TextureRegions>, String> {
        self.run(move |e| e.map_texture_regions(&p)).await.map(Json)
    }

    #[tool(
        description = "Make an MP3 louder or quieter by a number of decibels, losslessly (no \
            re-encode). Writes a new file and keeps the original. It cannot match another \
            sound's volume automatically: ask for a dB amount, or use +6 for a clear boost.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn make_sound_louder(
        &self,
        Parameters(p): Parameters<LouderParams>,
    ) -> Result<Json<LouderSound>, String> {
        self.run(move |_| Engine::make_sound_louder(&p))
            .await
            .map(Json)
    }

    #[tool(
        description = "Replace a sound event (a hero's own, a shared one, or a voice line) \
            with the user's MP3 and build a mod in staging. Most gameplay events are \
            randomizer pools; the default pool \"all\" puts the new audio in every clip so it \
            always plays. If the result notes that the clips are also played by other heroes, \
            rebuild with heroOnly to change just this hero. Get `event` from browse_sounds \
            first. Example: event \"Seven.Wpn.Fire\", audioPath \"/home/me/vineboom.mp3\".",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn swap_hero_sound(
        &self,
        Parameters(p): Parameters<SwapHeroSoundParams>,
    ) -> Result<Json<HeroSoundSwap>, String> {
        self.run(move |e| e.swap_hero_sound(&p)).await.map(Json)
    }

    #[tool(
        description = "Replace exactly one sound clip (.vsnd_c) with the user's MP3 and build \
            a mod in staging. Use swap_hero_sound instead for a whole event: replacing one \
            clip of a pool only changes one of its random variants. Get `clipEntry` from \
            browse_sounds with includeClips.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn swap_sound_clip(
        &self,
        Parameters(p): Parameters<SwapSoundClipParams>,
    ) -> Result<Json<SoundClipSwap>, String> {
        self.run(move |e| e.swap_sound_clip(&p)).await.map(Json)
    }

    #[tool(
        description = "Replace game textures with PNGs and build one mod in staging: UI art \
            (hero cards, portraits, ability and item icons, from browse_textures) and hero \
            skin textures (from list_hero_textures). Each PNG is fitted to the texture's size. \
            With maskPath, only the mask's white part changes. A hero card has several \
            variants (card, small, minimap, vertical); replace each one the user wants changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn replace_textures(
        &self,
        Parameters(p): Parameters<ReplaceTexturesParams>,
    ) -> Result<Json<TextureMod>, String> {
        self.run(move |e| e.replace_textures(&p)).await.map(Json)
    }

    #[tool(
        description = "Recolor all of a hero's ability and weapon effects (particles, effect \
            textures, baked model colors) and build a mod in staging. Mode \"solid\" sets one \
            hue; mode \"prism\" makes a rainbow, optionally animated. It does not change the \
            hero's skin. Only heroes with vfxRecolor in list_heroes are supported. Example: \
            hero \"Paige\", hue 280 for purple.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn recolor_hero_vfx(
        &self,
        Parameters(p): Parameters<RecolorVfxParams>,
    ) -> Result<Json<VfxRecolor>, String> {
        self.run(move |e| e.recolor_hero_vfx(&p)).await.map(Json)
    }

    #[tool(
        description = "Combine several mod VPKs into one so they take a single addon slot. \
            Inputs are in priority order; by default a later input wins when two mods ship \
            the same file. Use inspect_mod with `against` first to see what collides.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn merge_mods(
        &self,
        Parameters(p): Parameters<MergeModsParams>,
    ) -> Result<Json<MergedMod>, String> {
        self.run(move |e| e.merge_mods(&p)).await.map(Json)
    }

    #[tool(
        description = "List what a mod VPK contains and, with `against`, which of its files \
            collide with other mods. Read-only. This inspects the file list only; it does not \
            prove the mod works in game.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn inspect_mod(
        &self,
        Parameters(p): Parameters<InspectModParams>,
    ) -> Result<Json<ModInspection>, String> {
        self.run(move |_| Engine::inspect_mod(&p)).await.map(Json)
    }

    #[tool(
        description = "Search the Deadlock modding knowledge base (how addons load, asset \
            formats, sound events, textures and materials, models, particles, hero naming, \
            in-game testing). Returns the best-matching sections with a snippet; pass `doc` \
            and `section` to read_doc for the full text. Use keywords, not a sentence.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn search_docs(
        &self,
        Parameters(p): Parameters<SearchDocsParams>,
    ) -> Result<Json<DocSearch>, String> {
        self.run(move |e| Ok(e.search_docs(&p))).await.map(Json)
    }

    #[tool(
        description = "Read a knowledge-base document or one section of it, as markdown. Get \
            `doc` and `section` from search_docs.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn read_doc(&self, Parameters(p): Parameters<ReadDocParams>) -> Result<String, String> {
        self.run(move |e| e.read_doc(&p)).await
    }

    #[tool(
        description = "Render a hero in 3D with a mod VPK applied over the base game, for the \
            user to look at. Call it after building or editing anything that changes how a \
            hero looks (skin textures, materials, model edits), and whenever the user asks to \
            see a hero or a mod. Omit `vpk` for the unmodded hero. Shows the body model and \
            its materials in the menu pose, and the hero's animations can play on it; it does \
            not show particles or ability effects.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn preview_hero(
        &self,
        Parameters(p): Parameters<PreviewHeroParams>,
    ) -> Result<Json<HeroPreview>, String> {
        self.run(move |e| e.preview_hero(&p)).await.map(Json)
    }

    #[tool(
        description = "List the animations a hero preview can play: idles, movement, \
            abilities, emotes. Additive layers are left out. Names go to \
            preview_hero_animation.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_hero_animations(
        &self,
        Parameters(p): Parameters<HeroAnimationsParams>,
    ) -> Result<Json<HeroAnimations>, String> {
        self.run(move |e| e.list_hero_animations(&p))
            .await
            .map(Json)
    }

    #[tool(
        description = "Export one hero animation, skeleton only, to play on a model from \
            preview_hero. Apps with a preview viewer fetch these themselves when the user \
            picks an animation.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn preview_hero_animation(
        &self,
        Parameters(p): Parameters<PreviewAnimationParams>,
    ) -> Result<Json<HeroAnimationClip>, String> {
        self.run(move |e| e.preview_hero_animation(&p))
            .await
            .map(Json)
    }

    #[tool(
        description = "Hand a mod VPK to the Grimoire mod manager to install. Grimoire opens \
            (or comes to the front) with its import dialog showing the mod; the user confirms \
            there. Use it when the user wants a staged mod installed and has Grimoire.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn open_in_grimoire(
        &self,
        Parameters(p): Parameters<OpenInGrimoireParams>,
    ) -> Result<Json<GrimoireHandoff>, String> {
        self.run(move |_| Engine::open_in_grimoire(&p))
            .await
            .map(Json)
    }
}

#[tool_handler]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("vpkmerge", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS.to_string())
    }
}
