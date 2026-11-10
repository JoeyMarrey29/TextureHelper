use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io::{BufRead, Read, Write}, net::TcpListener, sync::Arc, time::{Duration, Instant}};

const ORIGIN: &str = "https://texture-helper-workshop.work-855.workers.dev";
const MAX_PACKAGE: usize = 24 * 1024 * 1024;
const MAX_PREVIEW: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub struct WorkshopState(Arc<Mutex<WorkshopSession>>);

#[derive(Default)]
struct WorkshopSession {
    token: Option<String>,
    login: Option<Login>,
    pending: Option<PendingSkin>,
}

#[derive(Clone)]
struct Login {
    id: String,
    verifier: String,
    code: Option<String>,
    error: Option<String>,
}

fn callback_code(line: &str, path: &str, id: &str) -> Option<String> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "GET" { return None; }
    let target = fields.next()?;
    if !target.starts_with('/') || fields.next()? != "HTTP/1.1" || fields.next().is_some() { return None; }
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if url.host_str()? != "127.0.0.1" || url.path() != path { return None; }
    let query: Vec<_> = url.query_pairs().collect();
    if query.len() != 2 || query.iter().filter(|(key, value)| key == "id" && value == id).count() != 1 { return None; }
    query.iter().find(|(key, value)| key == "code" && value.len() == 64
        && value.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())).map(|(_, value)| value.to_string())
}

fn receive_callback(listener: TcpListener, shared: &Mutex<WorkshopSession>, id: &str, path: &str) -> Result<(), String> {
    listener.set_nonblocking(true).map_err(|_| "Could not configure local Steam callback")?;
    let deadline = Instant::now() + Duration::from_secs(600);
    while Instant::now() < deadline {
        if !shared.lock().map_err(|_| "Workshop lock failed")?.login.as_ref().is_some_and(|login| login.id == id) { return Ok(()); }
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100)); continue;
            }
            Err(_) => return Err("Local Steam callback listener failed".into()),
        };
        stream.set_nonblocking(false).map_err(|_| "Could not configure local callback connection")?;
        stream.set_read_timeout(Some(Duration::from_secs(2))).map_err(|_| "Could not configure callback timeout")?;
        stream.set_write_timeout(Some(Duration::from_secs(2))).map_err(|_| "Could not configure callback timeout")?;
        let mut line = String::new();
        let code = match std::io::BufReader::new((&mut stream).take(4096)).read_line(&mut line) {
            Ok(_) => callback_code(&line, path, id),
            Err(_) => None,
        };
        let accepted = code.is_some();
        if let Some(code) = code {
            let mut session = shared.lock().map_err(|_| "Workshop lock failed")?;
            if let Some(login) = session.login.as_mut().filter(|login| login.id == id) { login.code = Some(code); }
            else { return Ok(()); }
        }
        let (status, title, detail) = if accepted {
            ("200 OK", "Return to Texture Helper", "Your Steam sign-in was handed back to the app. Return to Skin Workshop to finish. You can close this browser tab.")
        } else { ("400 Bad Request", "Sign-in link unavailable", "Start a new Steam sign-in from Texture Helper. This link does not belong to an active sign-in.") };
        let message = format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>Texture Helper - Steam sign-in</title><body><h1>{title}</h1><p>{detail}</p></body></html>");
        if write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'; base-uri 'none'\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{message}", message.len()).is_err() {
            eprintln!("Workshop local callback response could not be delivered");
        }
        if accepted { return Ok(()); }
    }
    Err("Steam sign-in timed out. Start again in the app.".into())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSkin {
    id: String,
    revision: String,
    title: String,
    preview: String,
    draft: drafts::Draft,
}

fn validate_preview(bytes: &[u8]) -> Result<(), String> {
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    limits.max_alloc = Some(8 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(|_| "Invalid preview PNG or preview exceeds 1024 pixels")?;
    Ok(())
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder().timeout(Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let host = attempt.url().host_str().unwrap_or("");
            if attempt.previous().len() < 5 && attempt.url().scheme() == "https"
                && (host == "github.com" || host.ends_with(".githubusercontent.com")) {
                attempt.follow()
            } else { attempt.error("Workshop redirect rejected") }
        })).build().map_err(|error| error.to_string())
}

async fn bounded(mut response: reqwest::Response, maximum: usize) -> Result<Vec<u8>, String> {
    if response.content_length().is_some_and(|length| length > maximum as u64) {
        return Err("Workshop response exceeds its size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "Workshop download interrupted")? {
        if bytes.len() + chunk.len() > maximum { return Err("Workshop response exceeds its size limit".into()); }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn api(method: reqwest::Method, path: &str, token: Option<&str>, body: Option<Value>) -> Result<Value, String> {
    let mut request = client()?.request(method, format!("{ORIGIN}{path}"));
    if let Some(token) = token { request = request.bearer_auth(token); }
    if let Some(body) = body { request = request.json(&body); }
    let response = request.send().await.map_err(|_| "Workshop is unreachable. Check your connection; setup may still be incomplete.")?;
    let status = response.status();
    let bytes = bounded(response, 512 * 1024).await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "Workshop returned an invalid response")?;
    if !status.is_success() {
        return Err(value["error"].as_str().unwrap_or("Workshop request failed").chars().take(500).collect());
    }
    Ok(value)
}

fn token(state: &WorkshopState) -> Result<String, String> {
    state.0.lock().map_err(|_| "Workshop lock failed")?.token.clone().ok_or("Sign in with Steam first".into())
}

fn valid_id(id: &str) -> Result<(), String> {
    if id.len() != 32 || !id.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
        return Err("Invalid workshop skin ID".into());
    }
    Ok(())
}

fn open_browser(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "Invalid browser URL")?;
    if parsed.scheme() != "https"
        || ![Some("texture-helper-workshop.work-855.workers.dev"), Some("steamcommunity.com")].contains(&parsed.host_str()) {
        return Err("Unexpected browser URL".into());
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(window: *mut std::ffi::c_void, operation: *const u16, file: *const u16,
            parameters: *const u16, directory: *const u16, show: i32) -> isize;
    }
    let operation: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
    // Both strings are null-terminated and remain alive for the shell call.
    let result = unsafe { ShellExecuteW(std::ptr::null_mut(), operation.as_ptr(), target.as_ptr(),
        std::ptr::null(), std::ptr::null(), 1) };
    if result <= 32 { return Err(format!("Windows could not open your default browser (code {result})")); }
    Ok(())
}

#[tauri::command]
pub async fn workshop_list(before: Option<String>) -> Result<Value, String> {
    if before.as_ref().is_some_and(|cursor| cursor.len() > 45
        || !cursor.bytes().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == b'-')) {
        return Err("Invalid workshop page cursor".into());
    }
    api(reqwest::Method::GET, &format!("/skins?before={}", before.unwrap_or_default()), None, None).await
}

#[tauri::command]
pub fn workshop_profile(steam_id: String) -> Result<(), String> {
    if steam_id.len() != 17 || !steam_id.starts_with("7656119") || !steam_id.bytes().all(|c| c.is_ascii_digit()) {
        return Err("Invalid SteamID".into());
    }
    open_browser(&format!("https://steamcommunity.com/profiles/{steam_id}"))
}

#[tauri::command]
pub async fn workshop_signin(state: State<'_, WorkshopState>) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "Could not open a local Steam callback listener")?;
    let port = listener.local_addr().map_err(|_| "Could not read local callback address")?.port();
    let mut random = [0u8; 64];
    getrandom::fill(&mut random).map_err(|_| "Could not create secure sign-in proof")?;
    let verifier = random[..32].iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let path = format!("/workshop/{}", random[32..].iter().map(|byte| format!("{byte:02x}")).collect::<String>());
    let challenge = format!("{:x}", Sha256::digest(verifier.as_bytes()));
    let result = api(reqwest::Method::POST, "/auth/start", None, Some(json!({"challenge":challenge, "port":port, "callbackPath":path}))).await?;
    let id = result["id"].as_str().filter(|id| id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or("Invalid workshop sign-in response")?.to_owned();
    let expected = format!("{ORIGIN}/auth/steam?id={id}");
    if result["url"].as_str() != Some(&expected) { return Err("Unexpected workshop sign-in URL".into()); }
    state.0.lock().map_err(|_| "Workshop lock failed")?.login = Some(Login {id:id.clone(), verifier, code:None, error:None});
    let shared = state.0.clone();
    std::thread::spawn(move || {
        if let Err(error) = receive_callback(listener, &shared, &id, &path) {
            match shared.lock() {
                Ok(mut session) => {
                    if let Some(login) = session.login.as_mut().filter(|login| login.id == id) { login.error = Some(error); }
                }
                Err(_) => eprintln!("Workshop callback error could not be recorded: lock failed"),
            }
        }
    });
    if let Err(error) = open_browser(&expected) {
        state.0.lock().map_err(|_| "Workshop lock failed")?.login = None;
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
pub async fn workshop_account(poll: bool, state: State<'_, WorkshopState>) -> Result<Value, String> {
    let login = if poll { state.0.lock().map_err(|_| "Workshop lock failed")?.login.clone() } else { None };
    if let Some(login) = login {
        if let Some(error) = login.error { return Err(error); }
        let Some(code) = login.code else { return Ok(json!({"pending":true})); };
        let response = api(reqwest::Method::POST, "/auth/exchange", None, Some(json!({"id":login.id,"verifier":login.verifier,"code":code}))).await?;
        let auth = response["token"].as_str().filter(|value| value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()))
            .ok_or("Invalid workshop session response")?.to_owned();
        let mut state = state.0.lock().map_err(|_| "Workshop lock failed")?;
        state.token = Some(auth);
        state.login = None;
    }
    let auth = state.0.lock().map_err(|_| "Workshop lock failed")?.token.clone();
    if let Some(auth) = auth { api(reqwest::Method::GET, "/me", Some(&auth), None).await }
    else { Ok(json!({"connected":false})) }
}

#[tauri::command]
pub async fn workshop_signout(state: State<'_, WorkshopState>) -> Result<(), String> {
    let auth = {
        let mut state = state.0.lock().map_err(|_| "Workshop lock failed")?;
        state.login = None;
        state.pending = None;
        state.token.take()
    };
    if let Some(auth) = auth { api(reqwest::Method::POST, "/logout", Some(&auth), Some(json!({}))).await?; }
    Ok(())
}

#[tauri::command]
pub async fn workshop_stage(id: String, preview: String, app: tauri::AppHandle, state: State<'_, WorkshopState>) -> Result<(), String> {
    let auth = token(&state)?;
    api(reqwest::Method::GET, "/me", Some(&auth), None).await?;
    if preview.len() > MAX_PREVIEW * 4 / 3 + 32 { return Err("Workshop preview is too large".into()); }
    let bytes = STANDARD.decode(preview.strip_prefix("data:image/png;base64,").unwrap_or(&preview)).map_err(|_| "Invalid preview PNG")?;
    validate_preview(&bytes)?;
    let draft = drafts::read_at(&drafts::storage(&app)?, &id)?;
    if draft.textures.is_empty() || draft.textures.len() > 12 { return Err("Choose an editable draft with 1–12 textures".into()); }
    let mut draft = draft;
    draft.ai_originals.clear();
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&draft).map_err(|error| error.to_string())?);
    digest.update(&bytes);
    let revision = format!("{:x}", digest.finalize());
    state.0.lock().map_err(|_| "Workshop lock failed")?.pending = Some(PendingSkin {id, revision, title:draft.name.clone(), preview, draft});
    Ok(())
}

#[tauri::command]
pub fn workshop_pending(state: State<'_, WorkshopState>) -> Result<Option<PendingSkin>, String> {
    Ok(state.0.lock().map_err(|_| "Workshop lock failed")?.pending.clone())
}

#[tauri::command]
pub fn workshop_discard(revision: String, state: State<'_, WorkshopState>) -> Result<(), String> {
    let mut session = state.0.lock().map_err(|_| "Workshop lock failed")?;
    if session.pending.as_ref().is_some_and(|pending| pending.revision != revision) {
        return Err("The selected draft changed. Review it again before cancelling.".into());
    }
    session.pending = None;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Submission {
    revision: String,
    title: String,
    description: String,
    consent: bool,
    parent: Option<String>,
}

#[tauri::command]
pub async fn workshop_publish(submission: Submission, state: State<'_, WorkshopState>) -> Result<Value, String> {
    if !submission.consent { return Err("Confirm your sharing rights and the public licence".into()); }
    let pending = state.0.lock().map_err(|_| "Workshop lock failed")?.pending.clone().ok_or("Choose a draft to publish from Custom skins first")?;
    if submission.revision != pending.revision { return Err("The selected draft changed. Review it again before publishing.".into()); }
    let bytes = serde_json::to_vec(&json!({"draft":pending.draft, "preview":pending.preview})).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_PACKAGE { return Err("Workshop upload exceeds 24 MiB".into()); }
    let auth = token(&state)?;
    let created = api(reqwest::Method::POST, "/skins", Some(&auth), Some(json!({
        "title":submission.title, "description":submission.description, "consent":true,
        "license":"CC-BY-4.0", "parent":submission.parent}))).await?;
    let id = created["id"].as_str().ok_or("Workshop did not create a submission")?;
    valid_id(id)?;
    let response = client()?.post(format!("{ORIGIN}/skins/{id}/upload")).bearer_auth(&auth)
        .header(reqwest::header::CONTENT_TYPE, "application/json").body(bytes).send().await
        .map_err(|_| format!("Upload interrupted. Check submission {id} before retrying."))?;
    let status = response.status();
    let result: Value = serde_json::from_slice(&bounded(response, 4096).await?).map_err(|_| "Invalid upload response")?;
    if !status.is_success() { return Err(result["error"].as_str().unwrap_or("Workshop upload failed").to_owned()); }
    let mut state = state.0.lock().map_err(|_| "Workshop lock failed")?;
    if state.pending.as_ref().is_some_and(|current| current.revision == pending.revision) {
        state.pending = None;
    }
    Ok(result)
}

#[tauri::command]
pub async fn workshop_submissions(state: State<'_, WorkshopState>) -> Result<Value, String> {
    api(reqwest::Method::GET, "/submissions", Some(&token(&state)?), None).await
}

#[tauri::command]
pub async fn workshop_bans(after: Option<String>, state: State<'_, WorkshopState>) -> Result<Value, String> {
    let after = after.unwrap_or_default();
    if !after.is_empty() && (after.len() != 17 || !after.bytes().all(|c| c.is_ascii_digit())) {
        return Err("Invalid ban page cursor".into());
    }
    api(reqwest::Method::GET, &format!("/bans?after={after}"), Some(&token(&state)?), None).await
}

async fn asset(id: &str, name: &str, hash: &str, maximum: usize) -> Result<Vec<u8>, String> {
    valid_id(id)?;
    if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) { return Err("Invalid workshop content hash".into()); }
    let response = client()?.get(format!("{ORIGIN}/skins/{id}/{name}")).send().await.map_err(|_| "Workshop download failed")?;
    if !response.status().is_success() { return Err("Skin is unavailable or has been removed".into()); }
    let bytes = bounded(response, maximum).await?;
    if format!("{:x}", Sha256::digest(&bytes)) != hash { return Err("Workshop content failed its SHA256 check".into()); }
    Ok(bytes)
}

#[tauri::command]
pub async fn workshop_preview(id: String, hash: String) -> Result<String, String> {
    let bytes = asset(&id, "preview.png", &hash, MAX_PREVIEW).await?;
    validate_preview(&bytes)?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}

#[tauri::command]
pub async fn workshop_download(id: String, hash: String, app: tauri::AppHandle) -> Result<String, String> {
    let bytes = asset(&id, "skin.phskin", &hash, MAX_PACKAGE).await?;
    let draft = drafts::import_package_at(&drafts::storage(&app)?, &bytes)?;
    Ok(draft.name)
}

#[tauri::command]
pub async fn workshop_moderate(id: String, action: String, steam_id: Option<String>, reason: Option<String>, state: State<'_, WorkshopState>) -> Result<(), String> {
    if !["hide", "cancel", "remove", "ban", "unban"].contains(&action.as_str()) { return Err("Invalid moderation action".into()); }
    let skin_action = action == "hide" || action == "cancel" || action == "remove";
    if skin_action { valid_id(&id)?; }
    let mut body = json!({"action":action,"steamId":steam_id});
    if let Some(reason) = reason { body["reason"] = json!(reason); }
    api(reqwest::Method::POST, &format!("/moderate/{}", if skin_action { &id } else { "account" }),
        Some(&token(&state)?), Some(body)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_decoder_enforces_limits_before_allocating_pixels() {
        let png = |width| {
            let image = image::RgbaImage::new(width, 1);
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut bytes, ImageFormat::Png).unwrap();
            bytes.into_inner()
        };
        assert!(validate_preview(&png(1024)).is_ok());
        assert!(validate_preview(&png(1025)).is_err());
        assert!(validate_preview(b"<svg/>").is_err());
        assert!(validate_preview(&[]).is_err());
    }

    #[test]
    fn asset_ids_cannot_inject_paths_or_queries() {
        assert!(valid_id(&"a".repeat(32)).is_ok());
        for id in ["../skin", "abc?name=evil", "", &"A".repeat(32), &"a".repeat(33)] {
            assert!(valid_id(id).is_err());
        }
    }

    #[test]
    fn callback_requires_the_initiating_apps_random_path_and_login_id() {
        let id = "a".repeat(64);
        let path = format!("/workshop/{}", "b".repeat(64));
        let code = "c".repeat(64);
        let target = format!("{path}?id={id}&code={code}");
        assert_eq!(callback_code(&format!("GET {target} HTTP/1.1\r\n"), &path, &id), Some(code.clone()));
        for line in [
            format!("POST {target} HTTP/1.1"),
            format!("GET {target}&id={id} HTTP/1.1"),
            format!("GET {target}&extra=1 HTTP/1.1"),
            format!("GET http://attacker.example{target} HTTP/1.1"),
            format!("GET //attacker.example{target} HTTP/1.1"),
            format!("GET {path}?id={}&code={code} HTTP/1.1", "d".repeat(64)),
            format!("GET /workshop/other?id={id}&code={code} HTTP/1.1"),
        ] {
            assert!(callback_code(&line, &path, &id).is_none());
        }
    }

    #[test]
    fn loopback_listener_delivers_code_only_to_its_matching_native_login() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let id = "a".repeat(64);
        let path = format!("/workshop/{}", "b".repeat(64));
        let code = "c".repeat(64);
        let shared = Arc::new(Mutex::new(WorkshopSession {
            login:Some(Login {id:id.clone(), verifier:"d".repeat(64), code:None, error:None}),
            ..WorkshopSession::default()
        }));
        let state = shared.clone();
        let expected_id = id.clone();
        let expected_path = path.clone();
        let thread = std::thread::spawn(move || receive_callback(listener, &state, &expected_id, &expected_path));
        let mut connection = std::net::TcpStream::connect(address).unwrap();
        connection.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(connection, "GET {path}?id={id}&code={code} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut response = String::new();
        connection.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Content-Type: text/html"));
        assert!(response.contains("<h1>Return to Texture Helper</h1>"));
        assert!(!response.contains(&code));
        thread.join().unwrap().unwrap();
        assert_eq!(shared.lock().unwrap().login.as_ref().unwrap().code.as_deref(), Some(code.as_str()));
    }
}
