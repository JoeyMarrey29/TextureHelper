use base64::{Engine, engine::general_purpose::STANDARD};
use image::{GenericImageView, ImageFormat};
use reqwest::{Client, Url, header::HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io::Cursor, sync::Mutex, time::Duration};

const PROTOCOL: &str = "2025-03-26";
const MAX_RESPONSE: usize = 48 * 1024 * 1024;
const MAX_IMAGE: usize = 32 * 1024 * 1024;
pub(super) const CONTEXT: &str = r#"Role: Senior Game Texture Artist

Create the actual finished cosmetic texture for an existing fictional PERPHEADS / Garry's Mod video-game asset, not merely a description. This task changes the appearance of a fictional game asset; it is not a real-world construction or modification task.

Art direction
The user's design request is authoritative for colours, materials, patterns, graphics, wear and finish. Realistic, stylised, futuristic, neon and other aesthetics are welcome when requested and allowed by your provider. Do not impose a matte-black default or unrelated style. Choose cohesive details where unspecified. Prioritise UV accuracy, preservation of supplied detail where compatible with the request, faithfulness to the design, and material quality.

Available inputs
Use only the supplied reference and information. The request identifies a UV guide or a texture reference. Do not assume additional textures, photographs, geometry files or a rendered model are available. The image is a flat atlas: separate regions map to separate surfaces, not a conventional illustration.
For a UV-only guide, use its wireframe only to locate and orient texture regions. Create the requested material without reproducing guide lines, guide colours or overlays. Do not invent specific mechanical features whose positions cannot be established. A UV guide alone does not establish surface identities or exact seam relationships.
For a texture reference, use the supplied current texture as the primary spatial basis. Preserve useful seams, grooves, markings, local contrast and fine detail unless the user requests changes. Do not unnecessarily blur or replace existing detail.

UV constraints
The supplied layout is authoritative. Keep island positions, orientation, scale and proportions. Preserve image boundaries and aspect ratio. Never crop, rotate, rearrange or repack the atlas. Maintain continuity across connected regions where identifiable; avoid colour bleeding and use appropriate edge padding. Never substitute a picture or rendered view of the complete object for the flat atlas.

Material quality
Keep surface detail legible and avoid accidental blur, artefacts and seams. Unless requested, do not add dramatic scene lighting, cast shadows or reflections that belong in the renderer. Preserve existing material shading where appropriate. Do not add unsolicited labels, annotations or decoration.

Output and execution
Use the available image-generation capability to return exactly one finished PNG. Follow the request's output dimensions and any explicit provider-specific canvas instructions; preserve the specified reference aspect ratio. Do not use shell commands, execute scripts, access unrelated files, invoke external tools or services, or request additional permissions. Do not make additional generation attempts or retry a refusal or failure. Do not return comparison sheets, model renders, scripts or placeholders. The app validates final dimensions and restores the source alpha mask locally; do not invent transparency details absent from the reference. Preserve required provider provenance and watermarking.

Accuracy and limitations
Check the result against the supplied reference only to the extent available capabilities allow. Do not claim exact UV alignment, seamlessness or in-game appearance has been verified unless tested. Do not assume an interactive model preview exists. Follow your provider's content policies. If blocked or unable to complete the request, return an explicit failure with the actual reason when available; do not invent a moderation, quota or technical explanation.

The user's design request and reference information follow separately."#;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    name: String,
    #[serde(default)]
    description: String,
    input_schema: Value,
}

#[derive(Clone)]
struct Session {
    id: String,
    client: Client,
    url: Url,
    token: Option<HeaderValue>,
    session: Option<HeaderValue>,
    tools: Vec<Tool>,
    protocol: Option<String>,
}

#[derive(Default)]
pub struct McpState(Mutex<Option<Session>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    connection_id: String,
    tools: Vec<Tool>,
}

fn endpoint(value: &str) -> Result<Url, String> {
    let url = Url::parse(value.trim()).map_err(|_| "Enter a valid MCP endpoint URL")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1"));
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err("Use HTTPS (or HTTP on localhost), without embedded credentials or a fragment".into());
    }
    Ok(url)
}

fn rpc_result(message: &Value, id: u64) -> Result<Option<Value>, String> {
    if message.get("id") != Some(&json!(id)) {
        if message.get("method").is_some() && message.get("id").is_some() {
            return Err("This MCP tool requires server-to-client requests (such as sampling), which are not supported".into());
        }
        return Ok(None);
    }
    if message["jsonrpc"] != "2.0" { return Err("Invalid MCP JSON-RPC response".into()); }
    if let Some(error) = message.get("error") {
        return Err(super::ai_error::describe("MCP", error, "Unspecified protocol error."));
    }
    message.get("result").cloned().map(Some).ok_or_else(|| "MCP response has no result".into())
}

fn sse_result(event: &[u8], id: u64) -> Result<Option<Value>, String> {
    let text = std::str::from_utf8(event).map_err(|_| "MCP stream is not UTF-8")?;
    let data = text.lines().filter_map(|line| line.strip_prefix("data:").map(|data| data.strip_prefix(' ').unwrap_or(data)))
        .collect::<Vec<_>>().join("\n");
    if data.is_empty() { return Ok(None); }
    let message: Value = serde_json::from_str(&data).map_err(|_| "Invalid MCP event JSON")?;
    rpc_result(&message, id)
}

impl Session {
    fn redact(&self, detail: String) -> String {
        match self.token.as_ref().and_then(|token| token.to_str().ok()).and_then(|token| token.strip_prefix("Bearer ")) {
            Some(token) if !token.is_empty() => detail.replace(token, "[redacted]"),
            _ => detail,
        }
    }

    async fn post(&mut self, message: Value) -> Result<Option<Value>, String> {
        let id = message.get("id").and_then(Value::as_u64);
        let mut request = self.client.post(self.url.clone())
            .header("Accept", "application/json, text/event-stream");
        if let Some(token) = &self.token { request = request.header("Authorization", token); }
        if let Some(session) = &self.session { request = request.header("Mcp-Session-Id", session); }
        if let Some(protocol) = &self.protocol { request = request.header("MCP-Protocol-Version", protocol); }
        let mut response = request.json(&message).send().await.map_err(|error| {
            if error.is_timeout() { "MCP request timed out; the server may still be processing it".to_string() }
            else { "Could not reach the MCP endpoint. Check its URL and connection.".to_string() }
        })?;
        if !response.status().is_success() {
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                return Err("MCP authorization expired or was refused (HTTP 401). Sign in again or check the bearer token.".into());
            }
            let status = response.status();
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| "MCP error response was interrupted")? {
                if bytes.len() + chunk.len() > 64000 { return Err(format!("MCP endpoint returned HTTP {status}; its error details exceeded 64 KB.")); }
                bytes.extend_from_slice(&chunk);
            }
            let value = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
            let detail = super::ai_error::describe("MCP provider", &value, "The endpoint rejected the request. Check authentication and Streamable HTTP support.");
            return Err(self.redact(format!("MCP endpoint returned HTTP {status}. {detail}")));
        }
        if message["method"] == "initialize" {
            self.session = response.headers().get("Mcp-Session-Id").cloned();
        }
        let Some(id) = id else { return Ok(None) };
        let is_sse = response.headers().get("Content-Type").and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(';').next().unwrap_or("").trim() == "text/event-stream");
        let mut buffer = Vec::new();
        let mut total = 0;
        let mut scanned = 0;
        while let Some(chunk) = response.chunk().await.map_err(|_| "MCP response interrupted or timed out; the server may still be processing it")? {
            total += chunk.len();
            if total > MAX_RESPONSE { return Err("MCP response exceeds 48 MiB".into()); }
            buffer.extend_from_slice(&chunk);
            if is_sse {
                while let Some((position, size)) = [
                    buffer[scanned..].windows(2).position(|bytes| bytes == b"\n\n").map(|p| (scanned + p, 2)),
                    buffer[scanned..].windows(4).position(|bytes| bytes == b"\r\n\r\n").map(|p| (scanned + p, 4)),
                ].into_iter().flatten().min_by_key(|(position, _)| *position) {
                    if let Some(result) = sse_result(&buffer[..position], id)? { return Ok(Some(result)); }
                    buffer.drain(..position + size);
                    scanned = 0;
                }
                scanned = buffer.len().saturating_sub(3);
            }
        }
        if is_sse { return Err("MCP stream closed without a matching response".into()); }
        let message = serde_json::from_slice(&buffer).map_err(|_| "Expected MCP JSON or a Streamable HTTP event stream")?;
        rpc_result(&message, id)?.map(Some).ok_or_else(|| "MCP response ID did not match the request".into())
    }

    async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        self.post(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
            .await.map_err(|error| self.redact(error))?.ok_or_else(|| "MCP request returned no result".into())
    }
}

async fn connect(url: &str, token: &str) -> Result<Session, String> {
    let token = if token.trim().is_empty() { None } else {
        let mut header = HeaderValue::from_str(&format!("Bearer {}", token.trim()))
            .map_err(|_| "Invalid bearer token")?;
        header.set_sensitive(true);
        Some(header)
    };
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|_| "Could not create an MCP connection identifier")?;
    let mut session = Session {
        id: random.iter().map(|byte| format!("{byte:02x}")).collect(),
        client: Client::builder().redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(300))
            .build().map_err(|_| "Could not create the MCP HTTP client")?,
        url: endpoint(url)?, token, session: None, tools: Vec::new(), protocol: None,
    };
    let initialized = session.request(1, "initialize", json!({
        "protocolVersion":PROTOCOL, "capabilities":{},
        "clientInfo":{"name":"perpheads-texture-helper","version":env!("CARGO_PKG_VERSION")}
    })).await?;
    let negotiated = initialized["protocolVersion"].as_str().ok_or("MCP server did not negotiate a protocol version")?;
    if !matches!(negotiated, "2025-03-26" | "2025-06-18" | "2025-11-25") {
        return Err("Unsupported MCP protocol version; supported versions are 2025-03-26, 2025-06-18 and 2025-11-25".into());
    }
    session.protocol = Some(negotiated.into());
    if !initialized["capabilities"]["tools"].is_object() {
        return Err("This MCP server does not advertise tools".into());
    }
    session.post(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await?;
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    for page in 0..20 {
        let params = cursor.as_ref().map(|cursor| json!({"cursor":cursor})).unwrap_or(json!({}));
        let result = session.request(2 + page, "tools/list", params).await?;
        let tools: Vec<Tool> = serde_json::from_value(result["tools"].clone())
            .map_err(|_| "MCP server returned an invalid tool list")?;
        for tool in tools {
            if tool.name.is_empty() || !tool.input_schema.is_object()
                || session.tools.iter().any(|known| known.name == tool.name) {
                return Err("MCP server returned an invalid or duplicate tool".into());
            }
            session.tools.push(tool);
        }
        if session.tools.len() > 256 { return Err("MCP server exceeds the 256-tool limit".into()); }
        match result.get("nextCursor") {
            None => {
                if session.tools.is_empty() { return Err("This MCP server has no tools".into()); }
                return Ok(session);
            }
            Some(Value::String(next)) if !next.is_empty() && seen.insert(next.clone()) => cursor = Some(next.clone()),
            _ => return Err("MCP server returned an invalid or repeated pagination cursor".into()),
        }
    }
    Err("MCP tool discovery exceeds 20 pages".into())
}

#[tauri::command]
pub async fn mcp_connect(state: tauri::State<'_, McpState>, url: String, token: String) -> Result<Connected, String> {
    let session = connect(&url, &token).await?;
    let result = Connected { connection_id: session.id.clone(), tools: session.tools.clone() };
    *state.0.lock().map_err(|_| "MCP connection lock failed")? = Some(session);
    Ok(result)
}

#[tauri::command]
pub fn mcp_disconnect(state: tauri::State<'_, McpState>) -> Result<(), String> {
    *state.0.lock().map_err(|_| "MCP connection lock failed")? = None;
    Ok(())
}

fn expand(value: &Value, prompt: &str, png: &str, label: &str, used: &mut [bool; 2]) -> Result<Value, String> {
    Ok(match value {
        Value::String(value) => match value.as_str() {
            "$PROMPT" => { used[0] = true; json!(prompt) }
            "$IMAGE" => { used[1] = true; json!(png) }
            "$IMAGE_BASE64" => { used[1] = true; json!(png.strip_prefix("data:image/png;base64,").unwrap_or(png)) }
            "$LABEL" => json!(label),
            "$MIME_TYPE" => json!("image/png"),
            _ => value.clone().into(),
        },
        Value::Array(values) => Value::Array(values.iter().map(|value| expand(value, prompt, png, label, used)).collect::<Result<_, _>>()?),
        Value::Object(values) => Value::Object(values.iter().map(|(key, value)|
            Ok((key.clone(), expand(value, prompt, png, label, used)?))).collect::<Result<_, String>>()?),
        _ => value.clone(),
    })
}

pub(super) fn decode_image(encoded: &str, format: ImageFormat) -> Result<image::RgbaImage, String> {
    if encoded.len() > MAX_IMAGE * 4 / 3 + 4 { return Err("AI image exceeds 32 MiB".into()); }
    let bytes = STANDARD.decode(encoded).map_err(|_| "AI image is not valid base64")?;
    if bytes.len() > MAX_IMAGE { return Err("AI image exceeds 32 MiB".into()); }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode().map_err(|error| format!("Invalid AI image: {error}"))?.to_rgba8())
}

pub(super) fn preserve_alpha(mut edited: image::RgbaImage, original: &image::RgbaImage) -> Result<String, String> {
    if edited.dimensions() != original.dimensions() {
        return Err("AI image dimensions do not match the original texture".into());
    }
    for (pixel, source) in edited.pixels_mut().zip(original.pixels()) { pixel[3] = source[3]; }
    let mut bytes = Cursor::new(Vec::new());
    edited.write_to(&mut bytes, ImageFormat::Png).map_err(|error| error.to_string())?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(bytes.into_inner())))
}

fn output_image(result: &Value, original: &image::RgbaImage) -> Result<String, String> {
    let content = result["content"].as_array().ok_or("MCP tool returned no content")?;
    let text = content.iter().filter_map(|item| item["text"].as_str()).collect::<Vec<_>>().join("\n");
    if result["isError"] == true {
        return Err(super::ai_error::describe("MCP provider", &json!({"error":result.get("structuredContent"),"text":text}), "Image generation failed or was refused."));
    }
    let images: Vec<_> = content.iter().filter(|item| item["type"] == "image").collect();
    if images.len() != 1 {
        if images.is_empty() && !text.trim().is_empty() {
            return Err(super::ai_error::describe("MCP provider", &Value::String(text), "No image returned."));
        }
        return Err(format!("Expected exactly one MCP image content block, received {}. No edits applied. {}",
            images.len(), text.chars().take(2000).collect::<String>()));
    }
    let format = match images[0]["mimeType"].as_str() {
        Some("image/png") => ImageFormat::Png,
        Some("image/jpeg") => ImageFormat::Jpeg,
        Some("image/webp") => ImageFormat::WebP,
        _ => return Err("MCP image must be PNG, JPEG or WebP; URLs and local file paths are not imported".into()),
    };
    let edited = decode_image(images[0]["data"].as_str().ok_or("MCP image has no base64 data")?, format)?;
    if edited.dimensions() != original.dimensions() {
        return Err("MCP image dimensions changed. Configure the tool to return the exact input size.".into());
    }
    preserve_alpha(edited, original)
}

#[tauri::command]
pub async fn mcp_edit_texture(
    state: tauri::State<'_, McpState>, connection_id: String, tool: String,
    arguments: Value, prompt: String, png: String, label: String,
) -> Result<String, String> {
    let mut session = state.0.lock().map_err(|_| "MCP connection lock failed")?
        .as_ref().filter(|session| session.id == connection_id).cloned().ok_or("MCP connection changed; reconnect")?;
    if !session.tools.iter().any(|known| known.name == tool) { return Err("Select a discovered MCP tool".into()); }
    if prompt.trim().is_empty() || prompt.len() > 16000 || label.len() > 4096
        || !arguments.is_object() || arguments.to_string().len() > 64000 {
        return Err("Enter a prompt (up to 16,000 bytes) and a JSON arguments object (up to 64,000 bytes)".into());
    }
    let original = decode_image(png.strip_prefix("data:image/png;base64,").unwrap_or(&png), ImageFormat::Png)?;
    super::textures::validate_replacement_dimensions(original.width(), original.height(), original.width(), original.height())?;
    let contextual_prompt = format!("{CONTEXT}\nTexture: {label}\nInput size: {} x {}.\nUser's cosmetic editing request:\n{prompt}",
        original.width(), original.height());
    let mut used = [false; 2];
    let arguments = expand(&arguments, &contextual_prompt, &png, &label, &mut used)?;
    if used != [true, true] { return Err("Arguments must include $PROMPT and either $IMAGE or $IMAGE_BASE64 as complete string values".into()); }
    if arguments.to_string().len() > MAX_RESPONSE { return Err("Expanded MCP arguments exceed 48 MiB".into()); }
    let result = session.request(100, "tools/call", json!({"name":tool,"arguments":arguments})).await?;
    output_image(&result, &original).map_err(|error| session.redact(error))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::{collections::BTreeMap, io::{BufRead, Read, Write}, net::TcpListener, thread};

    pub(crate) type HttpRequest = (String, BTreeMap<String, String>, String);
    pub(crate) type HttpReply = (u16, &'static str, String);

    #[test]
    fn artist_instructions_are_style_neutral_and_honor_reference_and_runtime_limits() {
        let context = CONTEXT.to_ascii_lowercase();
        assert!(!context.contains("weapon"));
        assert!(!context.contains("violence"));
        for required in ["uv-only guide", "texture reference", "user's design request is authoritative",
            "neon", "exactly one finished png", "do not use shell", "do not make additional generation attempts",
            "source alpha mask locally", "follow your provider's content policies"] {
            assert!(context.contains(required), "Missing {required}");
        }
    }

    #[test]
    fn reports_content_blocks_from_http_protocol_and_tool_results() {
        let failure = json!({"code":"moderation_blocked","message":"Blocked by safety policy",
            "moderation_details":{"categories":["violence"]}});
        let (url, server) = serve_http(|_| vec![(403,"application/json",json!({"error":failure}).to_string())]);
        let error = tauri::async_runtime::block_on(connect(&url,"private-fixture-token")).err().unwrap();
        assert!(error.contains("Content blocked by MCP"));
        assert!(error.contains("violence"));
        assert_eq!(server.join().unwrap().len(),1);
        let error = rpc_result(&json!({"jsonrpc":"2.0","id":1,"error":failure}),1).unwrap_err();
        assert!(error.contains("Content blocked by MCP"));
        assert!(error.contains("violence"));
        let original = image::RgbaImage::new(4,4);
        for is_error in [true,false] {
            let error = output_image(&json!({"isError":is_error,
                "content":[{"type":"text","text":"Blocked by content filters: violence"}]}),&original).unwrap_err();
            assert!(error.contains("Content blocked by MCP"));
            assert!(error.contains("violence"));
        }
    }

    pub(crate) fn serve_http(replies: impl FnOnce(&str) -> Vec<HttpReply>) -> (String, thread::JoinHandle<Vec<HttpRequest>>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let replies = replies(&origin);
        let handle = thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            let mut requests = Vec::new();
            for (status, content_type, body) in replies {
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                        Err(error) => panic!("Missing HTTP request: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = std::io::BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request = line.trim().to_string();
                let mut headers = BTreeMap::new();
                loop {
                    line.clear(); reader.read_line(&mut line).unwrap();
                    if line == "\r\n" { break; }
                    let (key, value) = line.split_once(':').unwrap();
                    headers.insert(key.to_ascii_lowercase(), value.trim().to_string());
                }
                let length = headers.get("content-length").map(|value| value.parse::<usize>().unwrap()).unwrap_or(0);
                let mut bytes = vec![0; length]; reader.read_exact(&mut bytes).unwrap();
                requests.push((request, headers, String::from_utf8(bytes).unwrap()));
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nMcp-Session-Id: fixture-session\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        (format!("{origin}/mcp"), handle)
    }

    #[test]
    fn streamable_http_initializes_paginates_and_calls_with_session_and_auth() {
        let (url, server) = serve_http(|_| vec![
            (200, "application/json", json!({"jsonrpc":"2.0","id":1,"result":{
                "protocolVersion":"2025-11-25","capabilities":{"tools":{}}}}).to_string()),
            (202, "application/json", String::new()),
            (200, "application/json", json!({"jsonrpc":"2.0","id":2,"result":{
                "tools":[{"name":"edit","inputSchema":{"type":"object"}}],"nextCursor":"page2"}}).to_string()),
            (200, "application/json", json!({"jsonrpc":"2.0","id":3,"result":{
                "tools":[{"name":"other","description":"Another tool","inputSchema":{"type":"object"}}]}}).to_string()),
            (200, "text/event-stream", concat!(
                "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\r\n\r\n",
                "data: {\"jsonrpc\":\"2.0\",\"id\":100,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n\n"
            ).into()),
        ]);
        let result = tauri::async_runtime::block_on(async {
            let mut session = connect(&url, "fixture-token").await?;
            assert_eq!(session.tools.len(), 2);
            session.request(100, "tools/call", json!({"name":"edit","arguments":{"prompt":"test"}})).await
        });
        let requests = server.join().unwrap();
        assert_eq!(result.unwrap()["content"][0]["text"], "done");
        assert!(requests.iter().all(|(line, headers, _)| line == "POST /mcp HTTP/1.1"
            && headers["authorization"] == "Bearer fixture-token"));
        assert_eq!(requests[1].1["mcp-session-id"], "fixture-session");
        assert_eq!(requests[2].1["mcp-protocol-version"], "2025-11-25");
        let page: Value = serde_json::from_str(&requests[3].2).unwrap();
        assert_eq!(page["params"]["cursor"], "page2");
    }

    #[test]
    fn reports_http_auth_errors_without_exposing_credentials() {
        let (url, server) = serve_http(|_| vec![(401, "application/json", "unauthorized".into())]);
        let error = tauri::async_runtime::block_on(connect(&url, "private-fixture-token")).err().unwrap();
        server.join().unwrap();
        assert!(error.contains("401"));
        assert!(!error.contains("private-fixture-token"));
    }

    fn encoded(image: &image::RgbaImage) -> String {
        let mut buffer = Cursor::new(Vec::new());
        image.write_to(&mut buffer, ImageFormat::Png).unwrap();
        STANDARD.encode(buffer.into_inner())
    }

    #[test]
    fn restricts_transport_and_credentials() {
        for url in ["https://example.com/mcp", "http://127.0.0.1:8080/mcp", "http://localhost:3000/mcp", "http://[::1]:8000/mcp"] {
            assert!(endpoint(url).is_ok(), "{url}");
        }
        for url in ["http://example.com/mcp", "file:///tmp/mcp", "https://user:pass@example.com/mcp", "https://example.com/#fragment"] {
            assert!(endpoint(url).is_err(), "{url}");
        }
    }

    #[test]
    fn expands_only_complete_markers_and_keeps_context() {
        let args = json!({"nested":[{"text":"$PROMPT","image":"$IMAGE_BASE64"}],"mime":"$MIME_TYPE","label":"$LABEL","literal":"prefix $PROMPT"});
        let mut used = [false; 2];
        let result = expand(&args, CONTEXT, "data:image/png;base64,YQ==", "stock", &mut used).unwrap();
        assert_eq!(used, [true, true]);
        assert_eq!(result["nested"][0]["text"], CONTEXT);
        assert_eq!(result["nested"][0]["image"], "YQ==");
        assert_eq!(result["mime"], "image/png");
        assert_eq!(result["literal"], "prefix $PROMPT");
    }

    #[test]
    fn reads_sse_notifications_and_matching_results() {
        assert!(sse_result(b": keepalive", 2).unwrap().is_none());
        assert!(sse_result(b"data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}", 2).unwrap().is_none());
        assert_eq!(sse_result(b"event: message\r\ndata: {\"jsonrpc\":\"2.0\",\r\ndata: \"id\":2,\"result\":{\"ok\":true}}", 2).unwrap(), Some(json!({"ok":true})));
        assert!(rpc_result(&json!({"jsonrpc":"2.0","id":2,"error":{"message":"denied"}}), 2).unwrap_err().contains("denied"));
        assert!(rpc_result(&json!({"jsonrpc":"2.0","id":9,"method":"sampling/createMessage"}), 2).is_err());
    }

    #[test]
    fn validates_output_and_restores_shader_alpha() {
        let original = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 17]));
        let edited = image::RgbaImage::from_pixel(2, 2, image::Rgba([90, 80, 70, 255]));
        let result = json!({"content":[{"type":"image","mimeType":"image/png","data":encoded(&edited)}]});
        let output = output_image(&result, &original).unwrap();
        let decoded = decode_image(output.strip_prefix("data:image/png;base64,").unwrap(), ImageFormat::Png).unwrap();
        assert_eq!(decoded.get_pixel(0, 0).0, [90, 80, 70, 17]);
        let wrong_size = json!({"content":[{"type":"image","mimeType":"image/png",
            "data":encoded(&image::RgbaImage::new(4,4))}]});
        assert!(output_image(&wrong_size, &original).is_err());
        assert!(output_image(&json!({"content":[result["content"][0],result["content"][0]]}), &original).is_err());
        assert!(output_image(&json!({"isError":true,"content":[{"type":"text","text":"Provider refusal"}]}), &original).unwrap_err().contains("Provider refusal"));
        assert!(output_image(&json!({"content":[{"type":"text","text":"No image"}]}), &original).unwrap_err().contains("No image"));
        assert!(decode_image("not-base64", ImageFormat::Png).is_err());
    }
}
