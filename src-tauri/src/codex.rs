use base64::{Engine, engine::general_purpose::STANDARD};
use image::ImageFormat;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc, atomic::{AtomicU64, Ordering}},
    time::{Duration, Instant},
};
use tauri::Manager;

const MAX_MESSAGE: u64 = 48 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Default)]
pub struct CodexState(Arc<Mutex<Option<Arc<Session>>>>, Arc<AtomicU64>);

struct Session {
    id: String,
    server: Server,
}

struct Server {
    child: Mutex<Child>,
    wire: Mutex<Wire>,
    directory: tempfile::TempDir,
    home: PathBuf,
    diagnostics: Mutex<mpsc::Receiver<Result<(u64, String), String>>>,
    generation: Arc<AtomicU64>,
}

struct Wire {
    stdin: Box<dyn Write + Send>,
    messages: mpsc::Receiver<Result<Value, String>>,
    sequence: u64,
    pending: VecDeque<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    connection_id: String,
    login: Option<String>,
    plan: Option<String>,
}

pub(super) fn runtime_path() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .ok_or("Could not locate the Codex installation directory")?;
    let packages = local.join("Microsoft").join("WinGet").join("Packages");
    if packages.is_dir() {
        for entry in std::fs::read_dir(packages).map_err(|error| format!("Could not find Codex: {error}"))? {
            let entry = entry.map_err(|error| format!("Could not read Codex installation: {error}"))?;
            if entry.file_name().to_string_lossy().starts_with("OpenAI.Codex_") {
                let path = entry.path().join("codex-x86_64-pc-windows-msvc.exe");
                if path.is_file() { return Ok(path); }
            }
        }
    }
    Err("Install the official Codex CLI with Windows Terminal: winget install --id OpenAI.Codex --exact".into())
}

fn runtime_command(program: &Path, home: &Path, directory: &Path) -> Command {
    let mut command = Command::new(program);
    command.arg("app-server").arg("--stdio").current_dir(directory)
        .env("CODEX_HOME", home)
        .env("LOG_FORMAT", "json").env("RUST_LOG", "error")
        .env_remove("OPENAI_API_KEY").env_remove("CODEX_API_KEY").env_remove("OPENAI_BASE_URL");
    // Use an app-owned config/account directory, never the user's repository or Codex configuration.
    for setting in [
        "model_provider=\"openai\"", "forced_login_method=\"chatgpt\"",
        "cli_auth_credentials_store=\"auto\"",
        "sandbox_mode=\"read-only\"", "approval_policy=\"on-request\"",
        "web_search=\"disabled\"", "mcp_servers={}", "hooks={}",
        "plugins={_default={enabled=false}}", "skills={bundled={enabled=false},include_instructions=false}",
        "project_doc_max_bytes=0", "analytics.enabled=false",
        "features.image_generation=true", "features.shell_tool=false",
        "features.unified_exec=false", "features.code_mode=false", "features.code_mode_only=false",
        "features.multi_agent=false", "features.multi_agent_v2=false",
        "features.apps=false", "features.connectors=false", "features.plugins=false",
        "features.hooks=false", "features.codex_hooks=false", "features.plugin_hooks=false",
        "features.view_image=false", "features.memories=false", "features.memory_tool=false",
        "features.remote_control=false", "features.skip_host_skill_discovery=true",
    ] { command.arg("-c").arg(setting); }
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000);
    command
}

fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut bytes = Vec::new();
    let count = reader.take(MAX_MESSAGE + 1).read_until(b'\n', &mut bytes)
        .map_err(|error| format!("Could not read Codex response: {error}"))?;
    if count == 0 { return Ok(None); }
    if count as u64 > MAX_MESSAGE { return Err("Codex response exceeds the 48 MiB limit".into()); }
    serde_json::from_slice(&bytes).map(Some).map_err(|_| "Codex returned malformed protocol data".into())
}

impl Wire {
    fn write(&mut self, value: &Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.stdin, value).map_err(|error| format!("Could not send Codex request: {error}"))?;
        self.stdin.write_all(b"\n").and_then(|_| self.stdin.flush())
            .map_err(|error| format!("Could not flush Codex request: {error}"))
    }

    fn receive(&mut self, deadline: Instant) -> Result<Value, String> {
        loop {
            let message = self.messages.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|error| format!("Codex response stopped or timed out: {error}. Subscription usage may already have occurred."))??;
            if message.get("method").is_some() && message.get("id").is_some() {
                self.write(&json!({"id":message["id"],"error":{"code":-32601,"message":"Texture Helper does not authorize commands, file changes, external tools or additional permissions"}}))?;
                continue;
            }
            return Ok(message);
        }
    }

    fn next(&mut self, deadline: Instant) -> Result<Value, String> {
        if let Some(message) = self.pending.pop_front() { return Ok(message); }
        self.receive(deadline)
    }

    fn request(&mut self, method: &str, params: Value, deadline: Instant) -> Result<Value, String> {
        self.sequence += 1;
        let id = self.sequence;
        self.write(&json!({"id":id,"method":method,"params":params}))?;
        loop {
            let message = self.receive(deadline)?;
            if message["id"].as_u64() == Some(id) {
                if message.get("error").is_some() {
                    // Do not echo arbitrary protocol payloads or authentication details.
                    return Err(format!("Codex rejected {method} (code {}). Check account access and runtime compatibility.",
                        message["error"]["code"]));
                }
                return message.get("result").cloned().ok_or_else(|| format!("Codex {method} returned no result"));
            }
            if message.get("method").is_some() {
                if self.pending.len() >= 128 { return Err("Codex sent too many notifications before its response".into()); }
                self.pending.push_back(message);
            }
        }
    }
}

impl Server {
    fn start(home: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&home).map_err(|error| format!("Could not prepare Codex account directory: {error}"))?;
        let directory = tempfile::tempdir().map_err(|error| format!("Could not prepare Codex workspace: {error}"))?;
        let mut child = runtime_command(&runtime_path()?, &home, directory.path()).stdin(Stdio::piped()).stdout(Stdio::piped())
            .stderr(Stdio::piped()).spawn().map_err(|error| format!("Could not start Codex: {error}"))?;
        let stdin = child.stdin.take().ok_or("Codex input pipe unavailable")?;
        let stdout = child.stdout.take().ok_or("Codex output pipe unavailable")?;
        let stderr = child.stderr.take().ok_or("Codex diagnostic pipe unavailable")?;
        let (diagnostic_sender, diagnostics) = mpsc::sync_channel(32);
        let generation = Arc::new(AtomicU64::new(0));
        let diagnostic_generation = generation.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut oversized = false;
            loop {
                let mut bytes = Vec::new();
                match reader.by_ref().take(65537).read_until(b'\n', &mut bytes) {
                    Ok(0) => break,
                    Ok(_) => {
                        let complete = bytes.last() == Some(&b'\n');
                        if !oversized && bytes.len() <= 65536 {
                            if let Some(detail) = Self::image_diagnostic(&bytes) {
                                match diagnostic_sender.try_send(Ok((diagnostic_generation.load(Ordering::SeqCst), detail))) {
                                    Ok(()) => {}
                                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                                    Err(mpsc::TrySendError::Full(_)) => eprintln!("Codex failure-detail buffer is full; additional diagnostics were discarded"),
                                }
                            }
                        }
                        oversized = !complete;
                    }
                    Err(error) => {
                        let _ = diagnostic_sender.try_send(Err(format!("Could not read Codex failure details: {error}")));
                        break;
                    }
                }
            }
        });
        let (sender, messages) = mpsc::sync_channel(16);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => { if sender.send(Ok(message)).is_err() { break; } }
                    Ok(None) => break,
                    Err(error) => { let _ = sender.send(Err(error)); break; }
                }
            }
        });
        let server = Self { child:Mutex::new(child), wire:Mutex::new(Wire { stdin:Box::new(stdin), messages, sequence:0, pending:VecDeque::new() }), directory, home, diagnostics:Mutex::new(diagnostics), generation };
        {
            let mut wire = server.wire.lock().map_err(|_| "Codex connection lock failed")?;
            wire.request("initialize", json!({
                "clientInfo":{"name":"texture_helper","version":env!("CARGO_PKG_VERSION")},
                "capabilities":{"experimentalApi":true}
            }), Instant::now() + Duration::from_secs(30))?;
            wire.write(&json!({"method":"initialized"}))?;
        }
        Ok(server)
    }

    // Current Codex image events omit moderation reasons. Read only image failures from its
    // JSON stderr stream and retain only the sanitized provider reason. Error-only
    // tracing can omit the informational spans containing the turn ID.
    fn image_diagnostic(bytes: &[u8]) -> Option<String> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        let text = value["fields"]["error"].as_str().or_else(|| value["fields"]["message"].as_str())?;
        if !text.contains("image generation failed") && !text.contains("moderation_blocked") { return None; }
        let detail = text.find("Some(").and_then(|index| {
            serde_json::Deserializer::from_str(&text[index + 5..]).into_iter::<String>().next()?.ok()
        }).and_then(|text| serde_json::from_str::<Value>(&text).ok()).unwrap_or_else(|| Value::String(text.into()));
        Some(super::ai_error::describe("ChatGPT / Codex", &detail, "Image generation failed."))
    }

    fn failure_detail(server: &Server, generation: u64, fallback: String) -> String {
        let diagnostics = match server.diagnostics.lock() {
            Ok(diagnostics) => diagnostics,
            Err(_) => return format!("{fallback}. Codex failure-detail lock failed."),
        };
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            match diagnostics.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Ok((id, reason))) if id == generation => return reason,
                Ok(Ok(_)) => continue,
                Ok(Err(error)) => return format!("{fallback}. {error}"),
                Err(_) => return fallback,
            }
        }
    }

    fn stop(&self) -> Result<(), String> {
        let mut child = self.child.lock().map_err(|_| "Codex process lock failed")?;
        if child.try_wait().map_err(|error| error.to_string())?.is_none() {
            child.kill().map_err(|error| format!("Could not stop Codex: {error}"))?;
            child.wait().map_err(|error| format!("Could not finish Codex process: {error}"))?;
        }
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Err(error) = self.stop() { eprintln!("{error}"); }
    }
}

fn account(value: &Value) -> Result<(Option<String>, Option<String>), String> {
    if value["account"]["type"] != "chatgpt" {
        return Err("Sign in with ChatGPT first. API-key accounts are not accepted by this subscription connection.".into());
    }
    Ok((value["account"]["email"].as_str().map(Into::into),
        value["account"]["planType"].as_str().map(Into::into)))
}

#[tauri::command]
pub async fn codex_connect(app: tauri::AppHandle, state: tauri::State<'_, CodexState>, sign_in: bool) -> Result<Connected, String> {
    codex_disconnect(state.clone())?;
    let active = state.0.clone();
    let epoch = state.1.clone();
    let current = epoch.load(Ordering::SeqCst);
    let home = app.path().app_data_dir().map_err(|error| error.to_string())?.join("codex");
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<Connected, String> {
        let server = Server::start(home)?;
        let mut random = [0;16];
        getrandom::fill(&mut random).map_err(|_| "Could not create Codex connection ID")?;
        let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let session = Arc::new(Session { id:id.clone(), server });
        {
            let mut active = active.lock().map_err(|_| "Codex session lock failed")?;
            if epoch.load(Ordering::SeqCst) != current { return Err("ChatGPT connection cancelled".into()); }
            *active = Some(session.clone());
        }
        let (login, plan) = {
            let mut wire = session.server.wire.lock().map_err(|_| "Codex connection lock failed")?;
            let deadline = Instant::now() + Duration::from_secs(180);
            if sign_in {
                let login = wire.request("account/login/start", json!({"type":"chatgpt"}), deadline)?;
                let url = login["authUrl"].as_str().ok_or("Codex did not supply a browser sign-in URL")?;
                let parsed = reqwest::Url::parse(url).map_err(|_| "Codex returned an invalid sign-in URL")?;
                if parsed.scheme() != "https" || parsed.host_str() != Some("auth.openai.com") {
                    return Err("Codex returned an unexpected sign-in host".into());
                }
                super::open_web_url(url)?;
                loop {
                    let event = wire.next(deadline)?;
                    if event["method"] == "account/login/completed" && event["params"]["loginId"] == login["loginId"] {
                        if event["params"]["success"] != true { return Err("ChatGPT sign-in failed or was cancelled".into()); }
                        break;
                    }
                }
            }
            account(&wire.request("account/read", json!({"refreshToken":false}), deadline)?)?
        };
        Ok(Connected { connection_id:id, login, plan })
    }).await.unwrap_or_else(|error| Err(format!("Codex connection task failed: {error}")));
    if let Err(error) = &result {
        if state.1.load(Ordering::SeqCst) == current {
            codex_disconnect(state).map_err(|cleanup| format!("{error}. Codex disconnect also failed: {cleanup}"))?;
        }
    }
    result
}

#[tauri::command]
pub fn codex_disconnect(state: tauri::State<'_, CodexState>) -> Result<(), String> {
    state.1.fetch_add(1, Ordering::SeqCst);
    if let Some(session) = state.0.lock().map_err(|_| "Codex session lock failed")?.take() {
        session.server.stop()?;
    }
    Ok(())
}

fn thread_params(directory: &Path) -> Value {
    json!({
        "cwd":directory, "ephemeral":true, "approvalPolicy":"on-request", "approvalsReviewer":"user",
        "sandbox":"read-only", "environments":[], "dynamicTools":[],
        "developerInstructions":format!("{}\nUse only the built-in image generation tool to edit the attached UV texture. \
Generate exactly one PNG. Keep the aspect ratio and UV layout, and preserve provenance labels. \
Do not use shell, file editing, web, MCP, plugins or subagents. Do not retry refusals or use API keys.", super::mcp::CONTEXT)
    })
}

fn image_bytes(item: &Value, home: &Path) -> Result<Vec<u8>, String> {
    if item["status"] != "completed" || !item["failure"].is_null() {
        if item["failure"]["type"] == "usageLimitExceeded" {
            return Err("ChatGPT / Codex image-generation subscription limit reached. No automatic retry was made.".into());
        }
        return Err(super::ai_error::describe("ChatGPT / Codex",
            &json!({"failure":item.get("failure"),"error":item.get("error"),"reason":item.get("reason")}),
            "Image generation failed or was refused."));
    }
    if let Some(data) = item["result"].as_str().filter(|data| !data.is_empty()) {
        if data.len() > 45 * 1024 * 1024 { return Err("Codex PNG exceeds the image payload limit".into()); }
        return STANDARD.decode(data.strip_prefix("data:image/png;base64,").unwrap_or(data))
            .map_err(|_| "Codex returned invalid PNG base64".into());
    }
    let path = item["savedPath"].as_str().ok_or("Codex returned no inline PNG or saved image path")?;
    let root = home.join("generated_images").canonicalize().map_err(|_| "Codex generated-image directory is unavailable")?;
    let path = Path::new(path).canonicalize().map_err(|_| "Codex generated PNG could not be located")?;
    if !path.starts_with(&root) || path.extension().and_then(|extension| extension.to_str()) != Some("png") {
        return Err("Codex returned an image outside its generated-image directory".into());
    }
    let file = std::fs::File::open(&path).map_err(|error| format!("Could not open Codex PNG: {error}"))?;
    if file.metadata().map_err(|error| error.to_string())?.len() > 32 * 1024 * 1024 {
        return Err("Codex PNG exceeds the 32 MiB limit".into());
    }
    let mut bytes = Vec::new();
    file.take(32 * 1024 * 1024 + 1).read_to_end(&mut bytes).map_err(|error| format!("Could not read Codex PNG: {error}"))?;
    if bytes.len() > 32 * 1024 * 1024 { return Err("Codex PNG exceeds the 32 MiB limit".into()); }
    Ok(bytes)
}

fn collect_image(wire: &mut Wire, thread_id: &str, turn_id: &str, home: &Path, deadline: Instant) -> Result<Vec<u8>, String> {
    let mut generated = None;
    let mut image_calls = 0;
    loop {
        let event = wire.next(deadline)?;
        if event["params"]["threadId"] != thread_id { continue; }
        if event["method"] == "error" {
            return Err(super::ai_error::describe("ChatGPT / Codex", &event["params"]["error"], "Generation failed."));
        }
        if event["method"] == "item/started" && event["params"]["item"]["type"] == "imageGeneration" {
            image_calls += 1;
            if image_calls > 1 { return Err("Codex attempted multiple image generations; the turn was stopped. Subscription usage may already have occurred.".into()); }
        }
        if event["method"] == "item/completed" && event["params"]["item"]["type"] == "imageGeneration" {
            if generated.is_some() { return Err("Codex returned multiple generated images. No edits applied.".into()); }
            generated = Some(image_bytes(&event["params"]["item"], home)?);
        }
        if event["method"] == "turn/completed" && event["params"]["turn"]["id"] == turn_id {
            if event["params"]["turn"]["status"] != "completed" {
                return Err(super::ai_error::describe("ChatGPT / Codex", &event["params"]["turn"]["error"], "Generation failed or was interrupted."));
            }
            break;
        }
    }
    generated.ok_or("Codex returned no generated PNG. Your account or runtime may not support built-in image generation. No API fallback was used.".into())
}

fn generate(server: &Server, prompt: String, png: String, label: String) -> Result<super::copilot::Edited, String> {
    if prompt.trim().is_empty() || prompt.len() > 16000 || label.len() > 4096 { return Err("Invalid Codex prompt or texture label length".into()); }
    let original = super::mcp::decode_image(png.strip_prefix("data:image/png;base64,").unwrap_or(&png), ImageFormat::Png)?;
    super::textures::validate_replacement_dimensions(original.width(), original.height(), original.width(), original.height())?;
    let mut rgb = original.clone();
    for pixel in rgb.pixels_mut() { pixel[3] = 255; }
    let mut bytes = std::io::Cursor::new(Vec::new());
    rgb.write_to(&mut bytes, ImageFormat::Png).map_err(|error| error.to_string())?;
    let image = format!("data:image/png;base64,{}", STANDARD.encode(bytes.into_inner()));
    let mut wire = server.wire.lock().map_err(|_| "Codex connection lock failed")?;
    let deadline = Instant::now() + TIMEOUT;
    account(&wire.request("account/read", json!({"refreshToken":false}), deadline)?)?;
    let thread = wire.request("thread/start", thread_params(server.directory.path()), deadline)?;
    let thread_id = thread["thread"]["id"].as_str().ok_or("Codex returned no thread ID")?;
    // The wire lock serializes generation; tag diagnostics before turn/start,
    // including errors emitted before its response supplies a turn ID.
    let generation = server.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let mut turn_id = None;
    let result = (|| {
        let turn = wire.request("turn/start", json!({
            "threadId":thread_id,
            "input":[{"type":"text","text":format!("Edit this fictional game UV texture using image generation. \
Return one PNG, preserve the {}:{} aspect ratio and UV islands. Texture: {label}\n{prompt}",original.width(),original.height())},
                {"type":"image","url":image}]
        }), deadline)?;
        let id = turn["turn"]["id"].as_str().ok_or("Codex returned no turn ID")?.to_owned();
        turn_id = Some(id.clone());
        let bytes = collect_image(&mut wire, thread_id, &id, &server.home, deadline)?;
        super::copilot::edited_image(&STANDARD.encode(bytes), &original)
    })();
    let result = result.map_err(|error| Server::failure_detail(server, generation, error));
    if let Err(error) = &result {
        // A failed turn/start may have started work without returning an ID; stop the runtime.
        let Some(turn_id) = turn_id else {
            server.stop().map_err(|cleanup| format!("{error}. Codex shutdown also failed: {cleanup}"))?;
            return result;
        };
        if let Err(interrupt) = wire.request("turn/interrupt", json!({"threadId":thread_id,"turnId":turn_id}), Instant::now() + Duration::from_secs(10)) {
            let failure = format!("{error}. Could not interrupt Codex: {interrupt}");
            server.stop().map_err(|cleanup| format!("{failure}. Codex shutdown also failed: {cleanup}"))?;
            return Err(failure);
        }
    }
    let cleanup = wire.request("thread/unsubscribe", json!({"threadId":thread_id}), Instant::now() + Duration::from_secs(10));
    if let Err(error) = cleanup {
        let failure = match result {
            Err(original) => format!("{original}. Codex session cleanup also failed: {error}. No proposal applied."),
            Ok(_) => format!("Codex session cleanup failed: {error}. No proposal applied."),
        };
        server.stop().map_err(|cleanup| format!("{failure}. Codex shutdown also failed: {cleanup}"))?;
        return Err(failure);
    }
    result
}

#[tauri::command]
pub async fn codex_edit_texture(state: tauri::State<'_, CodexState>, connection_id: String, prompt: String, png: String, label: String) -> Result<super::copilot::Edited, String> {
    let session = state.0.lock().map_err(|_| "Codex session lock failed")?.as_ref()
        .filter(|session| session.id == connection_id).cloned().ok_or("Codex connection changed; reconnect")?;
    tauri::async_runtime::spawn_blocking(move || generate(&session.server, prompt, png, label))
        .await.map_err(|error| format!("Codex image task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_moderation_reason_from_codex_stderr_without_other_log_data() {
        let response = json!({"error":{"message":"Your request was rejected by the safety system.",
            "code":"moderation_blocked","moderation_details":{"moderation_stage":"output","categories":["violence"]}}});
        let event = json!({"level":"ERROR","fields":{"error":format!("image generation failed: http 400 Bad Request: Some({:?})",response.to_string())},
            "spans":[{"conversation.id":"thread-1"},{"turn.id":"turn-1"}],"other":"private-data"});
        let reason = Server::image_diagnostic(event.to_string().as_bytes()).unwrap();
        assert!(reason.starts_with("Content blocked by ChatGPT / Codex."));
        assert!(reason.contains("violence"));
        assert!(reason.contains("output"));
        assert!(!reason.contains("private-data"));
        let mut no_spans = event.clone();
        no_spans.as_object_mut().unwrap().remove("spans");
        assert_eq!(Server::image_diagnostic(no_spans.to_string().as_bytes()).unwrap(),reason);
        assert!(Server::image_diagnostic(b"not-json").is_none());
        assert!(Server::image_diagnostic(json!({"fields":{"error":"authentication failed"}}).to_string().as_bytes()).is_none());
        let quota = image_bytes(&json!({"status":"failed","failure":{"type":"usageLimitExceeded"}}),Path::new(r"C:\test")).unwrap_err();
        assert!(quota.contains("subscription limit"));
        assert!(!quota.contains("Content blocked"));
        let unknown = image_bytes(&json!({"type":"imageGeneration","status":"failed","failure":null}),Path::new(r"C:\test")).unwrap_err();
        assert!(unknown.contains("did not supply a reason"));
        assert!(!unknown.contains(": imageGeneration"));
    }

    #[test]
    fn runtime_uses_subscription_auth_and_disables_ambient_execution() {
        let command = runtime_command(Path::new(r"C:\test\codex.exe"),Path::new(r"C:\account"),Path::new(r"C:\empty"));
        let args:Vec<_> = command.get_args().map(|arg| arg.to_str().unwrap()).collect();
        for expected in ["forced_login_method=\"chatgpt\"","features.shell_tool=false","features.multi_agent=false",
            "features.code_mode=false","features.plugins=false","mcp_servers={}","hooks={}","features.image_generation=true"] {
            assert!(args.contains(&expected), "Missing {expected}");
        }
        let environment:Vec<_> = command.get_envs().map(|(key,value)| (key.to_str().unwrap(),value.and_then(|value|value.to_str()))).collect();
        assert!(environment.contains(&("OPENAI_API_KEY",None)));
        assert!(environment.contains(&("CODEX_API_KEY",None)));
        assert!(environment.contains(&("CODEX_HOME",Some(r"C:\account"))));
    }

    fn wire_with(messages: Vec<Value>) -> Wire {
        let (sender, receiver) = mpsc::channel();
        for message in messages { sender.send(Ok(message)).unwrap(); }
        Wire { stdin:Box::new(Vec::<u8>::new()), messages:receiver, sequence:0, pending:VecDeque::new() }
    }

    #[test]
    fn retains_early_notifications_and_rejects_server_tool_requests() {
        let mut wire = wire_with(vec![
            json!({"method":"item/started","params":{"item":{"type":"imageGeneration"}}}),
            json!({"id":88,"method":"item/commandExecution/requestApproval","params":{}}),
            json!({"id":1,"result":{"turn":{"id":"turn-1"}}}),
        ]);
        let response = wire.request("turn/start", json!({}), Instant::now()+Duration::from_secs(1)).unwrap();
        assert_eq!(response["turn"]["id"], "turn-1");
        assert_eq!(wire.next(Instant::now()+Duration::from_secs(1)).unwrap()["method"], "item/started");
    }

    #[test]
    fn collects_only_complete_matching_image_turn_and_stops_duplicates() {
        let item = json!({"method":"item/completed","params":{"threadId":"thread-1",
            "item":{"type":"imageGeneration","status":"completed","result":"dGVzdA=="}}});
        let end = json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}});
        let deadline = Instant::now()+Duration::from_secs(1);
        let mut wire = wire_with(vec![item.clone(),end.clone()]);
        assert_eq!(collect_image(&mut wire,"thread-1","turn-1",Path::new(r"C:\test"),deadline).unwrap(),b"test");
        let mut wire = wire_with(vec![item.clone(),item,end.clone()]);
        assert!(collect_image(&mut wire,"thread-1","turn-1",Path::new(r"C:\test"),deadline).unwrap_err().contains("multiple"));
        let mut wire = wire_with(vec![end]);
        assert!(collect_image(&mut wire,"thread-1","turn-1",Path::new(r"C:\test"),deadline).unwrap_err().contains("no generated"));
    }

    #[test]
    fn rejects_api_accounts_and_missing_authentication() {
        assert!(account(&json!({"account":{"type":"apiKey"}})).is_err());
        assert!(account(&json!({"account":null})).is_err());
        assert_eq!(account(&json!({"account":{"type":"chatgpt","email":"fixture@example.test","planType":"plus"}})).unwrap().1.as_deref(), Some("plus"));
    }

    #[test]
    fn parses_protocol_and_rejects_malformed_messages() {
        assert_eq!(read_message(&mut CursorReader::new(b"{\"id\":1,\"result\":{}}\n")).unwrap().unwrap()["id"], 1);
        assert!(read_message(&mut CursorReader::new(b"not-json\n")).is_err());
        assert!(read_message(&mut CursorReader::new(b"")).unwrap().is_none());
    }

    type CursorReader<'a> = std::io::Cursor<&'a [u8]>;

    #[test]
    fn accepts_inline_png_and_rejects_refusal_and_untrusted_paths() {
        assert_eq!(image_bytes(&json!({"status":"completed","result":"dGVzdA=="}), Path::new(r"C:\test")).unwrap(), b"test");
        assert!(image_bytes(&json!({"status":"failed","result":"dGVzdA=="}), Path::new(r"C:\test")).is_err());
        assert!(image_bytes(&json!({"status":"completed","savedPath":r"C:\Windows\test.png"}), Path::new(r"C:\test")).is_err());
        let params = thread_params(Path::new(r"C:\test"));
        assert_eq!(params["ephemeral"], true);
        assert_eq!(params["sandbox"], "read-only");
        assert_eq!(params["environments"], json!([]));
    }

    #[test]
    fn saved_images_are_limited_to_canonical_generated_directory_and_size() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("generated_images");
        std::fs::create_dir(&root).unwrap();
        let inside = root.join("image.png");
        std::fs::write(&inside,b"provider-original").unwrap();
        let outside = home.path().join("outside.png");
        std::fs::write(&outside,b"unrelated").unwrap();
        let result = |path:&Path| json!({"status":"completed","fileId":"file-1","savedPath":path});
        assert_eq!(image_bytes(&result(&inside),home.path()).unwrap(), b"provider-original");
        assert!(image_bytes(&result(&outside),home.path()).unwrap_err().contains("outside"));
        std::fs::File::create(&inside).unwrap().set_len(32*1024*1024+1).unwrap();
        assert!(image_bytes(&result(&inside),home.path()).unwrap_err().contains("32 MiB"));
    }

    #[test]
    fn protocol_output_preserves_exact_original_and_source_alpha() {
        let generated = image::RgbaImage::from_pixel(8,4,image::Rgba([90,80,70,255]));
        let source = image::RgbaImage::from_pixel(4,2,image::Rgba([1,2,3,17]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        generated.write_to(&mut bytes,ImageFormat::Png).unwrap();
        let encoded = STANDARD.encode(bytes.into_inner());
        let mut wire = wire_with(vec![
            json!({"method":"item/completed","params":{"threadId":"thread-1",
                "item":{"type":"imageGeneration","status":"completed","result":encoded}}}),
            json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}),
        ]);
        let result = collect_image(&mut wire,"thread-1","turn-1",Path::new(r"C:\test"),Instant::now()+Duration::from_secs(1)).unwrap();
        let edited = serde_json::to_value(super::super::copilot::edited_image(&STANDARD.encode(result),&source).unwrap()).unwrap();
        assert_eq!(edited["originalPng"],format!("data:image/png;base64,{encoded}"));
        let png = edited["png"].as_str().unwrap().strip_prefix("data:image/png;base64,").unwrap();
        let image = super::super::mcp::decode_image(png,ImageFormat::Png).unwrap();
        assert_eq!(image.dimensions(),(4,2));
        assert_eq!(image.get_pixel(0,0).0,[90,80,70,17]);
    }

    #[test]
    #[ignore = "Requires official installed Codex CLI; no sign-in, prompts or image generation"]
    fn actual_runtime_handshake_without_generation() {
        let home = tempfile::tempdir().unwrap();
        let server = Server::start(home.path().into()).unwrap();
        let mut wire = server.wire.lock().unwrap();
        let response = wire.request("account/read", json!({"refreshToken":false}), Instant::now() + Duration::from_secs(30)).unwrap();
        assert!(response["account"].is_null());
        let thread = wire.request("thread/start", thread_params(server.directory.path()), Instant::now() + Duration::from_secs(30)).unwrap();
        assert!(thread["thread"]["id"].as_str().is_some());
        wire.request("thread/unsubscribe",json!({"threadId":thread["thread"]["id"]}), Instant::now()+Duration::from_secs(10)).unwrap();
        drop(wire);
        server.stop().unwrap();
    }
}
