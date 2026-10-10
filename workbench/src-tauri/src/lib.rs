// Tauri command arguments are deserialized into owned values by the macro.
#![allow(clippy::needless_pass_by_value)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::{Manager, RunEvent, State};
use tauri_plugin_opener::OpenerExt;
use workbench_core::acp::AgentId;
use workbench_core::config::{Settings, SettingsPatch};
use workbench_core::mcp::{ConfigFile, ServerConfig, ServerStatus};
use workbench_core::providers::{ModelInfo, ProviderId};
use workbench_core::skills::Skill;
use workbench_core::store::{Session, StoredMessage};
use workbench_core::{AgentEvent, AgentStatus, App, AppInfo, PreviewAnimations, ProviderStatus};

type Core<'a> = State<'a, Arc<App>>;
type CmdResult<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
fn app_info(core: Core<'_>) -> AppInfo {
    core.info()
}

#[tauri::command]
fn list_sessions(core: Core<'_>) -> CmdResult<Vec<Session>> {
    core.sessions().map_err(err)
}

#[tauri::command]
fn create_session(core: Core<'_>) -> CmdResult<Session> {
    core.create_session().map_err(err)
}

#[tauri::command]
fn rename_session(core: Core<'_>, id: String, title: String) -> CmdResult<()> {
    core.rename_session(&id, &title).map_err(err)
}

#[tauri::command]
fn delete_session(core: Core<'_>, id: String) -> CmdResult<()> {
    core.delete_session(&id).map_err(err)
}

#[tauri::command]
fn get_messages(core: Core<'_>, session_id: String) -> CmdResult<Vec<StoredMessage>> {
    core.messages(&session_id).map_err(err)
}

/// Resolves when the turn ends. Progress and the outcome travel over `on_event`.
#[tauri::command]
async fn send_message(
    core: Core<'_>,
    session_id: String,
    text: String,
    on_event: Channel<AgentEvent>,
) -> CmdResult<()> {
    let sink = move |event: AgentEvent| {
        let _ = on_event.send(event);
    };
    let _ = core.run_turn(&session_id, &text, &sink).await;
    Ok(())
}

#[tauri::command]
fn cancel_turn(core: Core<'_>, session_id: String) {
    core.cancel_turn(&session_id);
}

#[tauri::command]
fn get_settings(core: Core<'_>) -> Settings {
    core.settings()
}

#[tauri::command]
fn update_settings(core: Core<'_>, patch: SettingsPatch) -> CmdResult<Settings> {
    core.update_settings(patch).map_err(err)
}

#[tauri::command]
async fn list_providers(core: Core<'_>) -> CmdResult<Vec<ProviderStatus>> {
    core.providers().await.map_err(err)
}

#[tauri::command]
async fn sign_in(app: tauri::AppHandle, core: Core<'_>, provider: ProviderId) -> CmdResult<()> {
    let open = move |url: &str| {
        app.opener()
            .open_url(url, None::<&str>)
            .map_err(|e| anyhow::anyhow!("could not open the browser: {e}"))
    };
    core.sign_in(provider, &open).await.map_err(err)
}

#[tauri::command]
fn cancel_sign_in(core: Core<'_>) {
    core.cancel_sign_in();
}

#[tauri::command]
async fn set_api_key(core: Core<'_>, provider: ProviderId, key: String) -> CmdResult<()> {
    core.set_api_key(provider, &key).await.map_err(err)
}

#[tauri::command]
async fn sign_out(core: Core<'_>, provider: ProviderId) -> CmdResult<()> {
    core.sign_out(provider).await.map_err(err)
}

#[tauri::command]
async fn list_models(core: Core<'_>, provider: ProviderId) -> CmdResult<Vec<ModelInfo>> {
    core.models(provider).await.map_err(err)
}

#[tauri::command]
async fn list_agents(core: Core<'_>) -> CmdResult<Vec<AgentStatus>> {
    Ok(core.agents().await)
}

#[tauri::command]
async fn agent_sign_in(core: Core<'_>, agent: AgentId, method_id: String) -> CmdResult<()> {
    core.agent_sign_in(agent, &method_id).await.map_err(err)
}

#[tauri::command]
async fn respond_permission(
    core: Core<'_>,
    request_id: String,
    option_id: Option<String>,
) -> CmdResult<()> {
    core.respond_permission(&request_id, option_id.as_deref())
        .await
        .map_err(err)
}

#[tauri::command]
fn list_skills(core: Core<'_>) -> Vec<Skill> {
    core.skills()
}

#[tauri::command]
fn skill_dirs(core: Core<'_>) -> Vec<PathBuf> {
    core.skill_dirs()
}

#[tauri::command]
fn set_skill_enabled(core: Core<'_>, name: String, enabled: bool) -> CmdResult<()> {
    core.set_skill_enabled(&name, enabled).map_err(err)
}

#[tauri::command]
fn add_skill_dir(core: Core<'_>, path: PathBuf) -> CmdResult<()> {
    core.add_skill_dir(path).map_err(err)
}

#[tauri::command]
fn remove_skill_dir(core: Core<'_>, path: PathBuf) -> CmdResult<()> {
    core.remove_skill_dir(&path).map_err(err)
}

#[tauri::command]
async fn mcp_status(core: Core<'_>) -> CmdResult<Vec<ServerStatus>> {
    Ok(core.mcp_status().await)
}

#[tauri::command]
fn mcp_config(core: Core<'_>) -> CmdResult<String> {
    core.mcp_config_text().map_err(err)
}

#[tauri::command]
async fn save_mcp_config(core: Core<'_>, text: String) -> CmdResult<()> {
    core.save_mcp_config(&text).await.map_err(err)
}

#[tauri::command]
async fn reconnect_mcp(core: Core<'_>, name: String) -> CmdResult<()> {
    core.reconnect_mcp(&name).await.map_err(err)
}

/// Only web links: chat text is model output and must not launch local files.
#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) -> CmdResult<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http and https links can be opened".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_data_dir(app: tauri::AppHandle, core: Core<'_>) -> CmdResult<()> {
    app.opener()
        .open_path(core.info().data_dir.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn preview_animations(
    core: Core<'_>,
    hero: String,
    vpk: Option<String>,
) -> CmdResult<PreviewAnimations> {
    core.preview_animations(&hero, vpk.as_deref())
        .await
        .map_err(err)
}

/// One animation for the preview viewer: skeleton-only GLB bytes.
#[tauri::command]
async fn preview_animation(
    core: Core<'_>,
    hero: String,
    vpk: Option<String>,
    animation: String,
) -> CmdResult<tauri::ipc::Response> {
    let path = core
        .preview_animation(&hero, vpk.as_deref(), &animation)
        .await
        .map_err(err)?;
    let bytes = tauri::async_runtime::spawn_blocking(move || read_glb(&path))
        .await
        .map_err(|e| e.to_string())??;
    Ok(tauri::ipc::Response::new(bytes))
}

/// `preview://localhost/<path>`: a model preview a tool result points at. Tens of
/// megabytes failed over IPC in WebKitGTK, so the viewer fetches it as a URL.
fn serve_preview(request: &tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    let path = percent_encoding::percent_decode_str(request.uri().path().trim_start_matches('/'))
        .decode_utf8_lossy()
        .into_owned();
    let (status, body) = match read_glb(Path::new(&path)) {
        Ok(bytes) => (200, bytes),
        Err(e) => (404, e.into_bytes()),
    };
    tauri::http::Response::builder()
        .status(status)
        .header(
            "Content-Type",
            if status == 200 {
                "model/gltf-binary"
            } else {
                "text/plain"
            },
        )
        // The page's origin (the dev server, or tauri://localhost) differs from this scheme's.
        .header("Access-Control-Allow-Origin", "*")
        .body(body)
        .expect("static response parts are valid")
}

/// The path comes from model output, so only binary glTF is read, never arbitrary files.
fn read_glb(path: &Path) -> Result<Vec<u8>, String> {
    if path
        .extension()
        .is_none_or(|e| !e.eq_ignore_ascii_case("glb"))
    {
        return Err("only .glb previews can be opened".into());
    }
    let bytes = std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => {
            "This preview was cleaned up. Ask for a new one.".to_owned()
        }
        _ => e.to_string(),
    })?;
    if !bytes.starts_with(b"glTF") {
        return Err("not a binary glTF file".into());
    }
    Ok(bytes)
}

/// The MCP servers bundled with the app. Tauri places each `externalBin` sidecar
/// next to the main executable, in dev and in every bundle format.
fn builtin_mcp() -> tauri::Result<ConfigFile> {
    let exe = tauri::utils::platform::current_exe()?;
    let sidecar = exe.with_file_name(format!("vpkmerge-mcp{}", std::env::consts::EXE_SUFFIX));
    let vpkmerge = ServerConfig {
        command: Some(sidecar.to_string_lossy().into_owned()),
        builtin: true,
        ..ServerConfig::default()
    };
    Ok(ConfigFile {
        mcp_servers: [("vpkmerge".to_owned(), vpkmerge)].into(),
    })
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("preview", |_ctx, request, responder| {
            std::thread::spawn(move || responder.respond(serve_preview(&request)));
        })
        .setup(|app| {
            // With slight hinting WebKitGTK places glyphs on fractional pixels, which
            // smears Inter's thin stems across two pixels on 1x displays.
            #[cfg(target_os = "linux")]
            if let Some(settings) = gtk::Settings::default() {
                use gtk::prelude::GtkSettingsExt;
                settings.set_gtk_xft_hinting(1);
                settings.set_gtk_xft_hintstyle(Some("hintfull"));
            }
            let core = App::open(&app.path().app_data_dir()?, builtin_mcp()?)?;
            let background = Arc::clone(&core);
            tauri::async_runtime::spawn(async move {
                if let Err(e) = background.start_mcp().await {
                    eprintln!("MCP startup: {e:#}");
                }
            });
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_sessions,
            create_session,
            rename_session,
            delete_session,
            get_messages,
            send_message,
            cancel_turn,
            get_settings,
            update_settings,
            list_providers,
            sign_in,
            cancel_sign_in,
            set_api_key,
            sign_out,
            list_models,
            list_agents,
            agent_sign_in,
            respond_permission,
            list_skills,
            skill_dirs,
            set_skill_enabled,
            add_skill_dir,
            remove_skill_dir,
            mcp_status,
            mcp_config,
            save_mcp_config,
            reconnect_mcp,
            open_url,
            open_data_dir,
            preview_animations,
            preview_animation,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri app");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(core) = handle.try_state::<Arc<App>>() {
                tauri::async_runtime::block_on(core.shutdown());
            }
        }
    });
}
