use base64::{Engine, engine::general_purpose::STANDARD};
use github_copilot_sdk::{
    Attachment, Client, ClientMode, ClientOptions, CliProgram, ImageGenerationConfig,
    MessageOptions, SessionConfig, SystemMessageConfig, ToolSet,
    hooks::{HookActor, HookEvent, HookOutput, PreToolUseOutput, SessionHooks},
};
use image::{ImageFormat, imageops::{self, FilterType}};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashSet, io::{Cursor, Read, Seek, SeekFrom}, path::PathBuf, process::{Command, Stdio},
    sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}}, time::{Duration, Instant},
};

struct Runtime {
    id: String,
    client: Client,
    _directory: tempfile::TempDir,
}

#[derive(Default)]
pub struct CopilotState(tokio::sync::Mutex<Option<Arc<Runtime>>>, AtomicBool);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    installed: bool,
    winget_available: bool,
}

fn winget_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .map(|path| path.join("Microsoft").join("WindowsApps").join("winget.exe"))
        .filter(|path| path.is_file())
}

#[tauri::command]
pub fn copilot_installation() -> Installation {
    Installation { installed:runtime_path().is_ok(), winget_available:winget_path().is_some() }
}

#[tauri::command]
pub fn codex_installation() -> Installation {
    Installation { installed:super::codex::runtime_path().is_ok(), winget_available:winget_path().is_some() }
}

fn install_command(program: PathBuf, codex: bool) -> Command {
    let mut command = Command::new(program);
    command.args(["install", "--id", if codex { "OpenAI.Codex" } else { "GitHub.Copilot" }, "--exact", "--source", "winget",
        "--scope", "user", "--silent", "--accept-source-agreements", "--accept-package-agreements",
        "--disable-interactivity"]);
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000);
    command
}

fn install_cli(confirmed: bool, codex: bool) -> Result<Installation, String> {
    if !confirmed { return Err("CLI installation requires confirmation".into()); }
    let check = if codex { codex_installation } else { copilot_installation };
    if check().installed { return Ok(check()); }
    let program = winget_path().ok_or("Windows Package Manager is unavailable. Install or update Microsoft's App Installer from the Microsoft Store, then try again.")?;
    let mut log = tempfile::tempfile().map_err(|error| format!("Could not prepare installer log: {error}"))?;
    let output = log.try_clone().map_err(|error| format!("Could not capture installer output: {error}"))?;
    let errors = log.try_clone().map_err(|error| format!("Could not capture installer errors: {error}"))?;
    let mut child = install_command(program, codex).stdin(Stdio::null()).stdout(output).stderr(errors)
        .spawn().map_err(|error| format!("Could not start Windows Package Manager: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(600);
    let exit = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break exit,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            Ok(None) => {
                child.kill().map_err(|error| format!("Installation timed out; could not stop Windows Package Manager: {error}"))?;
                child.wait().map_err(|error| format!("Could not finish the package-manager process: {error}"))?;
                return Err("CLI installation timed out. A child installer may still finish; check again before retrying.".into());
            }
            Err(error) => return Err(format!("Could not check the CLI installer: {error}")),
        }
    };
    if !exit.success() {
        let length = log.metadata().map_err(|error| format!("Could not read installer log size: {error}"))?.len();
        log.seek(SeekFrom::Start(length.saturating_sub(4000))).map_err(|error| format!("Could not read installer log: {error}"))?;
        let mut bytes = Vec::new();
        log.take(4000).read_to_end(&mut bytes).map_err(|error| format!("Could not read installer failure: {error}"))?;
        return Err(format!("Windows Package Manager failed ({exit}): {}", String::from_utf8_lossy(&bytes).trim()));
    }
    let installation = check();
    if !installation.installed {
        return Err("Windows Package Manager finished, but the official CLI was not found. Restart Texture Helper and check the installation in Windows Terminal.".into());
    }
    Ok(installation)
}

#[tauri::command]
pub async fn copilot_install(state: tauri::State<'_, CopilotState>, confirmed: bool) -> Result<Installation, String> {
    install_runtime(state, confirmed, false).await
}

#[tauri::command]
pub async fn codex_install(state: tauri::State<'_, CopilotState>, confirmed: bool) -> Result<Installation, String> {
    install_runtime(state, confirmed, true).await
}

async fn install_runtime(state: tauri::State<'_, CopilotState>, confirmed: bool, codex: bool) -> Result<Installation, String> {
    if state.1.swap(true, Ordering::SeqCst) { return Err("A CLI installation is already running".into()); }
    let result = tauri::async_runtime::spawn_blocking(move || install_cli(confirmed, codex)).await;
    state.1.store(false, Ordering::SeqCst);
    result.map_err(|error| format!("CLI installer task failed: {error}"))?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    connection_id: String,
    login: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Edited {
    png: String,
    original_png: String,
}

fn runtime_path() -> Result<PathBuf, String> {
    for base in [std::env::var_os("LOCALAPPDATA").map(PathBuf::from).map(|path| path.join("GitHubCopilotCLI")),
        std::env::var_os("ProgramFiles").map(PathBuf::from).map(|path| path.join("GitHubCopilotCLI")),
        std::env::var_os("ProgramFiles").map(PathBuf::from).map(|path| path.join("GitHub Copilot CLI"))].into_iter().flatten() {
        let path = base.join("copilot.exe");
        if path.is_file() { return Ok(path); }
    }
    Err("Install the official GitHub Copilot CLI with: winget install --id GitHub.Copilot --exact. Restart Texture Helper afterwards.".into())
}

fn client_options(directory: PathBuf) -> Result<ClientOptions, String> {
    let home = std::env::var_os("COPILOT_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".copilot")))
        .ok_or("Could not locate the Copilot CLI account directory")?;
    let mut options = ClientOptions::default().with_mode(ClientMode::Empty)
        .with_base_directory(home).with_disable_client_telemetry(true).with_use_logged_in_user(true);
    options.program = CliProgram::Path(runtime_path()?);
    options.working_directory = directory;
    options.env_remove = ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN", "COPILOT_ALLOW_ALL", "COPILOT_ASSISTED_APPROVAL"]
        .into_iter().map(Into::into).collect();
    options.extra_args = vec!["--no-auto-update".into(), "--no-custom-instructions".into(), "--disable-builtin-mcps".into()];
    Ok(options)
}

fn login() -> Result<(), String> {
    let mut command = Command::new(runtime_path()?);
    command.args(["login", "--web-flow"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // The browser handles credentials; no shell or token is passed to the app.
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000);
    let mut child = command.spawn().map_err(|error| format!("Could not start GitHub sign-in: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return if status.success() { Ok(()) } else {
                Err("GitHub sign-in failed or was cancelled. Try signing in with the Copilot CLI, then choose Use signed-in account.".into())
            },
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                child.kill().map_err(|error| format!("Sign-in timed out and its process could not be stopped: {error}"))?;
                child.wait().map_err(|error| format!("Could not finish the sign-in process: {error}"))?;
                return Err("GitHub sign-in timed out after three minutes".into());
            }
            Err(error) => return Err(format!("Could not check GitHub sign-in: {error}")),
        }
    }
}

#[tauri::command]
pub async fn copilot_connect(state: tauri::State<'_, CopilotState>, sign_in: bool) -> Result<Connected, String> {
    copilot_disconnect(state.clone()).await?;
    if sign_in { tauri::async_runtime::spawn_blocking(login).await.map_err(|_| "GitHub sign-in task failed")??; }
    let directory = tempfile::tempdir().map_err(|error| format!("Could not prepare Copilot workspace: {error}"))?;
    let mut bytes = [0;16];
    getrandom::fill(&mut bytes).map_err(|_| "Could not create Copilot connection ID")?;
    let client = tokio::time::timeout(Duration::from_secs(45), Client::start(client_options(directory.path().into())?)).await
        .map_err(|_| "Copilot runtime startup timed out")?.map_err(|error| format!("Could not start Copilot: {error}"))?;
    let auth = match tokio::time::timeout(Duration::from_secs(30), client.get_auth_status()).await {
        Ok(Ok(auth)) if auth.is_authenticated => auth,
        outcome => {
            client.stop().await.map_err(|error| format!("Could not stop Copilot after authentication check: {error}"))?;
            return Err(match outcome {
                Ok(Ok(_)) => "Sign in to GitHub Copilot first. A paid plan with image-generation access is required.".into(),
                Ok(Err(error)) => format!("Could not check Copilot sign-in: {error}"),
                Err(_) => "Copilot sign-in check timed out".into(),
            });
        }
    };
    let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let connected = Connected { connection_id:id.clone(), login:auth.login };
    *state.0.lock().await = Some(Arc::new(Runtime { id, client, _directory:directory }));
    Ok(connected)
}

#[tauri::command]
pub async fn copilot_disconnect(state: tauri::State<'_, CopilotState>) -> Result<(), String> {
    if let Some(runtime) = state.0.lock().await.take() {
        runtime.client.stop().await.map_err(|error| format!("Could not stop Copilot runtime: {error}"))?;
    }
    Ok(())
}

#[derive(Default)]
struct ImageOnly {
    calls: AtomicUsize,
    approved: Mutex<HashSet<String>>,
}

#[async_trait::async_trait]
impl SessionHooks for ImageOnly {
    async fn on_hook_with_actor(&self, event: HookEvent, actor: HookActor) -> HookOutput {
        let HookEvent::PreToolUse { input, .. } = event else { return HookOutput::None };
        let allow = input.tool_name == "image_generation" && self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        if allow {
            if let Some(id) = actor.tool_call_id() {
                match self.approved.lock() {
                    Ok(mut ids) => { ids.insert(id.into()); }
                    Err(_) => return HookOutput::PreToolUse(PreToolUseOutput {
                        permission_decision:Some("deny".into()),
                        permission_decision_reason:Some("Image permission state failed".into()), ..Default::default()
                    }),
                }
            }
        }
        HookOutput::PreToolUse(PreToolUseOutput {
            permission_decision:(!allow).then(|| "deny".into()),
            permission_decision_reason:(!allow).then(|| "Only one image-generation call is authorized. Do not retry or use other tools.".into()),
            ..Default::default()
        })
    }
}

fn configuration(directory: PathBuf, policy: Arc<ImageOnly>) -> Result<SessionConfig, String> {
    let approvals = policy.clone();
    let tools = ToolSet::new().add_builtin("image_generation").map_err(|error| error.to_string())?.to_vec();
    let mut config = SessionConfig::default()
        .with_working_directory(directory)
        .with_system_message(SystemMessageConfig::new().with_mode("append").with_content(super::mcp::CONTEXT))
        .with_available_tools(tools)
        .with_image_generation(ImageGenerationConfig::new().with_enabled(true).with_save_to_disk(false))
        .with_hooks(policy)
        .approve_permissions_if(move |request| request.managed_approval_required != Some(true)
            && request.tool_call_id.as_ref().is_some_and(|id|
            approvals.approved.lock().is_ok_and(|ids| ids.contains(id))));
    config.included_builtin_agents = Some(Vec::new());
    config.included_builtin_skills = Some(Vec::new());
    config.client_name = Some("PERPHEADS Texture Helper".into());
    Ok(config)
}

pub(super) fn edited_image(data: &str, original: &image::RgbaImage) -> Result<Edited, String> {
    let generated = super::mcp::decode_image(data, ImageFormat::Png)?;
    if generated.width() as u64 * original.height() as u64 != generated.height() as u64 * original.width() as u64 {
        return Err("The AI provider changed the texture's aspect ratio. No edits applied; the app will not crop the image or its provenance marks.".into());
    }
    let edited = imageops::resize(&generated, original.width(), original.height(), FilterType::Lanczos3);
    Ok(Edited {
        png:super::mcp::preserve_alpha(edited, original)?,
        // Keep the exact provider bytes (including C2PA) alongside the derived texture.
        original_png:format!("data:image/png;base64,{data}"),
    })
}

fn image_content(data: &Value) -> Result<Option<String>, String> {
    if data["success"] != true {
        return Err(super::ai_error::describe("GitHub Copilot", &data["error"], "Image generation failed or was refused."));
    }
    let Some(contents) = data["result"]["contents"].as_array() else { return Ok(None) };
    let images: Vec<_> = contents.iter().filter(|item| item["type"] == "image").collect();
    if images.is_empty() { return Ok(None); }
    if images.len() != 1 || images[0]["mimeType"] != "image/png" {
        return Err("Copilot must return exactly one PNG image. No edits applied.".into());
    }
    images[0]["data"].as_str().map(|data| Some(data.into())).ok_or("Copilot PNG has no inline data".into())
}

#[tauri::command]
pub async fn copilot_edit_texture(
    state: tauri::State<'_, CopilotState>, connection_id: String, prompt: String, png: String, label: String,
) -> Result<Edited, String> {
    if prompt.trim().is_empty() || prompt.len() > 16000 || label.len() > 4096 {
        return Err("Enter a prompt of up to 16,000 UTF-8 bytes and a texture label of up to 4,096 bytes".into());
    }
    let original = super::mcp::decode_image(png.strip_prefix("data:image/png;base64,").unwrap_or(&png), ImageFormat::Png)?;
    super::textures::validate_replacement_dimensions(original.width(), original.height(), original.width(), original.height())?;
    let mut rgb = original.clone();
    for pixel in rgb.pixels_mut() { pixel[3] = 255; }
    let mut encoded = Cursor::new(Vec::new());
    rgb.write_to(&mut encoded, ImageFormat::Png).map_err(|error| error.to_string())?;
    let runtime = state.0.lock().await.as_ref().filter(|runtime| runtime.id == connection_id).cloned()
        .ok_or("Copilot connection changed; reconnect")?;
    let policy = Arc::new(ImageOnly::default());
    let session = runtime.client.create_session(configuration(runtime._directory.path().into(), policy)?)
        .await.map_err(|error| format!("Could not create Copilot image session: {error}"))?;
    let mut events = session.subscribe();
    let prompt = format!("Use the attached image as the reference for exactly one image_generation edit. \
Keep its original aspect ratio {}:{} and UV island positions. Preserve all provenance labels and watermarks. \
Do not generate a 3D rendering. Do not retry a refusal or make a second image call. \
Texture label: {label}\nUser's cosmetic editing request:\n{prompt}",
        original.width(), original.height());
    let mut message = MessageOptions::new(prompt).with_attachments(vec![Attachment::Blob {
        data:STANDARD.encode(encoded.into_inner()), mime_type:"image/png".into(), display_name:Some("game-texture.png".into()),
    }]);
    message.wait_timeout = Some(Duration::from_secs(300));
    let mut send = Box::pin(session.send_and_wait(message));
    let result: Result<Edited, String> = async {
        let mut image = None;
        let completion = loop {
            tokio::select! {
                result = &mut send => break result,
                event = events.recv() => {
                    let event = event.map_err(|error| format!("Copilot image event stream failed: {error}"))?;
                    if event.event_type == "tool.execution_complete" {
                        if let Some(data) = image_content(&event.data)? {
                            if image.replace(data).is_some() { return Err("Copilot returned more than one image result. No edits applied.".into()); }
                        }
                    }
                }
            }
        };
        completion.map_err(|error| super::ai_error::describe("GitHub Copilot", &Value::String(error.to_string()), "Generation stopped."))?;
        // The SDK queues the final tool event before completing the wait.
        loop {
            let event = match tokio::time::timeout(Duration::from_millis(50), events.recv()).await {
                Ok(Ok(event)) => event,
                Ok(Err(error)) => return Err(format!("Copilot final image events could not be read: {error}")),
                Err(_) => break,
            };
            if event.event_type == "tool.execution_complete" {
                if let Some(data) = image_content(&event.data)? {
                    if image.replace(data).is_some() { return Err("Copilot returned multiple image results. No edits applied.".into()); }
                }
            }
        }
        let data = image.ok_or("Copilot returned no inline PNG. Your plan/policy/runtime may not support image generation. No retry was made.")?;
        edited_image(&data, &original)
    }.await;
    drop(send);
    if result.is_err() {
        session.abort().await.map_err(|error| format!("Could not stop Copilot generation: {error}. AI credits may already have been used."))?;
    }
    let cleanup = runtime.client.delete_session(session.id()).await;
    if let Err(error) = cleanup { return Err(format!("Copilot session cleanup failed: {error}. No proposal applied. AI credits may have been used.")); }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_requires_consent_and_pins_official_packages() {
        assert!(install_cli(false, false).unwrap_err().contains("confirmation"));
        assert!(install_cli(false, true).unwrap_err().contains("confirmation"));
        for (codex, package) in [(false,"GitHub.Copilot"),(true,"OpenAI.Codex")] {
            let command = install_command(PathBuf::from(r"C:\Windows\winget.exe"), codex);
            let args:Vec<_> = command.get_args().map(|arg| arg.to_str().unwrap()).collect();
            assert!(args.windows(2).any(|pair| pair == ["--id",package]));
            assert!(args.windows(2).any(|pair| pair == ["--source","winget"]));
            assert!(args.contains(&"--exact"));
            assert!(args.contains(&"--disable-interactivity"));
        }
    }

    #[tokio::test]
    #[ignore = "Requires installed official Copilot CLI; authentication check only, no prompts"]
    async fn runtime_handshake_without_generation() {
        let directory = tempfile::tempdir().unwrap();
        let client = tokio::time::timeout(Duration::from_secs(45),
            Client::start(client_options(directory.path().into()).unwrap())).await.unwrap().unwrap();
        let auth = tokio::time::timeout(Duration::from_secs(30), client.get_auth_status()).await.unwrap();
        if let Ok(auth) = &auth {
            if auth.is_authenticated {
                let session = client.create_session(configuration(directory.path().into(), Arc::new(ImageOnly::default())).unwrap()).await.unwrap();
                client.delete_session(session.id()).await.unwrap();
                println!("Restricted image session created and deleted; no prompt sent.");
            }
        }
        let stopped = client.stop().await;
        println!("Runtime authentication-status request completed; no prompt sent.");
        auth.unwrap();
        stopped.unwrap();
    }

    #[tokio::test]
    async fn allows_one_image_tool_and_denies_other_tools() {
        let policy = ImageOnly::default();
        for (name, denied) in [("shell", true), ("image_generation", false), ("image_generation", true)] {
            let event = HookEvent::PreToolUse {
                input:serde_json::from_value(serde_json::json!({
                    "sessionId":"test", "timestamp":0, "cwd":r"C:\test", "toolName":name, "toolArgs":{}
                })).unwrap(),
                ctx:github_copilot_sdk::hooks::HookContext { session_id:"test".into() },
            };
            let HookOutput::PreToolUse(output) = policy.on_hook_with_actor(event, HookActor::default()).await else { panic!("Missing tool decision") };
            assert_eq!(output.permission_decision.as_deref() == Some("deny"), denied);
        }
    }

    #[test]
    fn limits_session_to_image_generation_without_ambient_tools() {
        let config = configuration(PathBuf::from(r"C:\test"), Arc::new(ImageOnly::default())).unwrap();
        assert_eq!(config.available_tools, Some(vec!["builtin:image_generation".into()]));
        assert_eq!(config.included_builtin_agents, Some(Vec::new()));
        assert_eq!(config.image_generation.unwrap().enabled, Some(true));
    }

    #[test]
    fn accepts_one_inline_png_and_surfaces_refusals() {
        assert_eq!(image_content(&serde_json::json!({"success":true,"result":{"contents":[{"type":"image","mimeType":"image/png","data":"test"}]}})).unwrap(), Some("test".into()));
        assert!(image_content(&serde_json::json!({"success":false,"error":{"message":"Provider refusal"}})).unwrap_err().contains("Provider refusal"));
        let blocked = image_content(&serde_json::json!({"success":false,"error":{"code":"content_filter",
            "message":"Blocked by safety policy","moderation_details":{"categories":["violence"]}}})).unwrap_err();
        assert!(blocked.contains("Content blocked by GitHub Copilot"));
        assert!(blocked.contains("violence"));
        assert!(image_content(&serde_json::json!({"success":true,"result":{"contents":[{"type":"image","mimeType":"image/jpeg","data":"test"}]}})).is_err());
    }

    #[test]
    fn preserves_provider_bytes_alpha_and_aspect_ratio() {
        let original = image::RgbaImage::from_pixel(4, 2, image::Rgba([1, 2, 3, 17]));
        let generated = image::RgbaImage::from_pixel(8, 4, image::Rgba([90, 80, 70, 255]));
        let mut bytes = Cursor::new(Vec::new());
        generated.write_to(&mut bytes, ImageFormat::Png).unwrap();
        let data = STANDARD.encode(bytes.into_inner());
        let result = edited_image(&data, &original).unwrap();
        assert_eq!(result.original_png, format!("data:image/png;base64,{data}"));
        let decoded = super::super::mcp::decode_image(result.png.strip_prefix("data:image/png;base64,").unwrap(), ImageFormat::Png).unwrap();
        assert_eq!(decoded.get_pixel(0,0).0, [90, 80, 70, 17]);
        assert!(edited_image(&data, &image::RgbaImage::new(4,4)).unwrap_err().contains("aspect ratio"));
    }
}
