use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, Url};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}},
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct McpAuthState(Mutex<Option<Arc<AtomicBool>>>);

fn random() -> Result<String, String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| "Could not generate OAuth security parameters")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn secure_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "OAuth metadata contains an invalid URL")?;
    if (url.scheme() != "https" && !(url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]" | "::1"))))
        || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err("OAuth endpoints must use HTTPS (or localhost HTTP), without embedded credentials or fragments".into());
    }
    Ok(url)
}

async fn json_response(mut response: reqwest::Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!("OAuth endpoint returned HTTP {}", response.status()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "OAuth response interrupted or timed out")? {
        if bytes.len() + chunk.len() > 1024 * 1024 { return Err("OAuth response exceeds 1 MiB".into()); }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "OAuth endpoint returned invalid JSON".into())
}

async fn metadata(client: &Client, url: Url) -> Result<Option<Value>, String> {
    let response = client.get(url).header("MCP-Protocol-Version", "2025-06-18")
        .send().await.map_err(|_| "Could not discover OAuth metadata")?;
    if response.status() == 404 { return Ok(None); }
    json_response(response).await.map(Some)
}

fn callback_code(line: &str, state: &str) -> Result<Option<String>, String> {
    let Some(target) = line.strip_prefix("GET ").and_then(|line| line.split_once(" HTTP/1.").map(|pair| pair.0)) else { return Ok(None) };
    if !target.starts_with("/mcp-oauth-callback?") { return Ok(None); }
    let url = Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| "Invalid OAuth callback")?;
    let pairs: Vec<_> = url.query_pairs().collect();
    let states: Vec<_> = pairs.iter().filter(|(name, _)| name == "state").collect();
    if states.len() != 1 || states[0].1 != state { return Ok(None); }
    if let Some((_, error)) = pairs.iter().find(|(name, _)| name == "error") {
        return Err(format!("Sign-in was declined or failed: {}", error.chars().take(100).collect::<String>()));
    }
    let codes: Vec<_> = pairs.iter().filter(|(name, _)| name == "code").collect();
    if codes.len() != 1 || codes[0].1.is_empty() { return Err("OAuth callback did not contain one authorization code".into()); }
    Ok(Some(codes[0].1.to_string()))
}

fn receive_code(listener: TcpListener, state: String, cancelled: Arc<AtomicBool>) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|_| "Could not configure OAuth callback listener")?;
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        if cancelled.load(Ordering::SeqCst) { return Err("MCP sign-in cancelled".into()); }
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100)); continue;
            }
            Err(_) => return Err("OAuth callback listener failed".into()),
        };
        stream.set_nonblocking(false).map_err(|_| "Could not configure OAuth callback connection")?;
        stream.set_read_timeout(Some(Duration::from_secs(2))).map_err(|_| "Could not set OAuth callback timeout")?;
        stream.set_write_timeout(Some(Duration::from_secs(2))).map_err(|_| "Could not set OAuth callback timeout")?;
        let mut line = String::new();
        let read = std::io::BufReader::new((&mut stream).take(8192)).read_line(&mut line);
        let result = if read.is_ok() { callback_code(&line, &state) } else { Ok(None) };
        let matched = !matches!(result, Ok(None));
        let body = if matched { "Sign-in response received. Return to Texture Helper to check the connection." }
            else { "Unrecognized sign-in response. Return to the original sign-in page." };
        if write!(stream, "HTTP/1.1 {} OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
            if matched { 200 } else { 400 }, body.len()).is_err() {
            eprintln!("MCP OAuth callback response could not be delivered");
        }
        if let Some(code) = result? { return Ok(code); }
    }
    Err("MCP sign-in timed out after three minutes".into())
}

async fn authorize(
    url: &str, client_id: &str, scope: &str, cancelled: Arc<AtomicBool>,
    open_browser: impl FnOnce(&str) -> Result<(), String>,
) -> Result<String, String> {
    let resource = secure_url(url)?;
    let client = Client::builder().redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(30))
        .build().map_err(|_| "Could not create OAuth client")?;
    let mut issuer = resource.join("/").map_err(|_| "Invalid OAuth origin")?;
    let resource_metadata = resource.join(&format!("/.well-known/oauth-protected-resource{}", resource.path()))
        .map_err(|_| "Invalid protected resource metadata URL")?;
    let protected = match metadata(&client, resource_metadata).await? {
        Some(value) => Some(value),
        None => metadata(&client, resource.join("/.well-known/oauth-protected-resource").map_err(|_| "Invalid OAuth origin")?).await?,
    };
    if let Some(protected) = protected {
        if protected["resource"].as_str() != Some(resource.as_str()) {
            return Err("OAuth protected resource metadata does not match the selected MCP URL".into());
        }
        let authorization_servers = protected["authorization_servers"].as_array().ok_or("OAuth resource metadata has no authorization servers")?;
        if authorization_servers.len() != 1 {
            return Err("This MCP service must advertise exactly one OAuth authorization server".into());
        }
        issuer = secure_url(authorization_servers[0].as_str().ok_or("Invalid OAuth authorization server")?)?;
    }
    let discovery = issuer.join(&format!("/.well-known/oauth-authorization-server{}", issuer.path().trim_end_matches('/')))
        .map_err(|_| "Invalid OAuth discovery URL")?;
    let info = metadata(&client, discovery).await?.ok_or("This MCP service does not publish OAuth authorization-server metadata. Use a bearer token instead.")?;
    if info["issuer"].as_str().map(|value| value.trim_end_matches('/')) != Some(issuer.as_str().trim_end_matches('/')) {
        return Err("OAuth authorization-server issuer mismatch".into());
    }
    let authorization = secure_url(info["authorization_endpoint"].as_str().ok_or("Missing OAuth authorization endpoint")?)?;
    let token_endpoint = secure_url(info["token_endpoint"].as_str().ok_or("Missing OAuth token endpoint")?)?;
    if !info["code_challenge_methods_supported"].as_array().is_some_and(|methods| methods.iter().any(|method| method == "S256")) {
        return Err("MCP account sign-in requires OAuth PKCE S256 support".into());
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| "Could not open the local OAuth callback")?;
    let redirect = format!("http://127.0.0.1:{}/mcp-oauth-callback", listener.local_addr().map_err(|_| "Could not read the OAuth callback address")?.port());
    let id = if client_id.trim().is_empty() {
        let registration = secure_url(info["registration_endpoint"].as_str().ok_or("This service requires a registered public OAuth client ID. Enter one from its administrator, or use a bearer token.")?)?;
        let result = json_response(client.post(registration).json(&json!({
            "client_name":"PERPHEADS Texture Helper", "redirect_uris":[redirect],
            "grant_types":["authorization_code"],"response_types":["code"],
            "token_endpoint_auth_method":"none"
        })).send().await.map_err(|_| "OAuth client registration failed")?).await?;
        if result["token_endpoint_auth_method"].as_str().is_some_and(|method| method != "none") {
            return Err("The OAuth service did not register a public client".into());
        }
        result["client_id"].as_str().ok_or("OAuth registration returned no client ID")?.to_string()
    } else { client_id.trim().into() };
    let verifier = random()?;
    let state = random()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut authorization = authorization;
    authorization.query_pairs_mut().extend_pairs([
        ("response_type","code"),("client_id",id.as_str()),("redirect_uri",redirect.as_str()),
        ("state",state.as_str()),("code_challenge",challenge.as_str()),("code_challenge_method","S256"),
        ("resource",resource.as_str()),
    ]);
    if !scope.trim().is_empty() { authorization.query_pairs_mut().append_pair("scope", scope.trim()); }
    if cancelled.load(Ordering::SeqCst) { return Err("MCP sign-in cancelled".into()); }
    open_browser(authorization.as_str())?;
    let code = tauri::async_runtime::spawn_blocking(move || receive_code(listener, state, cancelled))
        .await.map_err(|_| "OAuth callback task failed")??;
    let result = json_response(client.post(token_endpoint).form(&[
        ("grant_type","authorization_code"),("code",code.as_str()),("client_id",id.as_str()),
        ("redirect_uri",redirect.as_str()),("code_verifier",verifier.as_str()),("resource",resource.as_str()),
    ]).send().await.map_err(|_| "OAuth token exchange failed")?).await?;
    if !result["token_type"].as_str().is_some_and(|kind| kind.eq_ignore_ascii_case("bearer")) {
        return Err("OAuth server did not return a bearer token".into());
    }
    result["access_token"].as_str().filter(|token| !token.is_empty()).map(String::from)
        .ok_or("OAuth token response has no access token".into())
}

#[tauri::command]
pub fn mcp_cancel_signin(state: tauri::State<'_, McpAuthState>) -> Result<(), String> {
    if let Some(cancelled) = state.0.lock().map_err(|_| "OAuth state lock failed")?.as_ref() {
        cancelled.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[tauri::command]
pub async fn mcp_signin(
    state: tauri::State<'_, McpAuthState>, mcp: tauri::State<'_, super::mcp::McpState>,
    url: String, client_id: String, scope: String,
) -> Result<super::mcp::Connected, String> {
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let mut current = state.0.lock().map_err(|_| "OAuth state lock failed")?;
        if let Some(previous) = current.replace(cancelled.clone()) { previous.store(true, Ordering::SeqCst); }
    }
    let outcome = async {
        let token = authorize(&url, &client_id, &scope, cancelled.clone(), super::open_web_url).await?;
        if cancelled.load(Ordering::SeqCst) { return Err("MCP sign-in cancelled".into()); }
        let connected = super::mcp::mcp_connect(mcp.clone(), url, token).await?;
        if cancelled.load(Ordering::SeqCst) {
            super::mcp::mcp_disconnect(mcp)?;
            return Err("MCP sign-in cancelled".into());
        }
        Ok(connected)
    }.await;
    let mut current = state.0.lock().map_err(|_| "OAuth state lock failed")?;
    if current.as_ref().is_some_and(|value| Arc::ptr_eq(value, &cancelled)) { *current = None; }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_discovers_registers_and_exchanges_pkce_code_without_passwords() {
        let (url, server) = crate::mcp::tests::serve_http(|origin| vec![
            (200, "application/json", json!({"resource":format!("{origin}/mcp"),"authorization_servers":[origin]}).to_string()),
            (200, "application/json", json!({
                "issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),
                "token_endpoint":format!("{origin}/token"),"registration_endpoint":format!("{origin}/register"),
                "code_challenge_methods_supported":["S256"]
            }).to_string()),
            (201, "application/json", json!({"client_id":"public-fixture-client","token_endpoint_auth_method":"none"}).to_string()),
            (200, "application/json", json!({"access_token":"fixture-access-token","token_type":"Bearer"}).to_string()),
        ]);
        let authorization = Arc::new(Mutex::new(None));
        let captured = authorization.clone();
        let outcome = tauri::async_runtime::block_on(authorize(&url, "", "images.edit", Arc::new(AtomicBool::new(false)), move |url| {
            let authorization_url = Url::parse(url).unwrap();
            let pairs: std::collections::BTreeMap<String, String> = authorization_url.query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect();
            let mut callback = Url::parse(&pairs["redirect_uri"]).unwrap();
            callback.query_pairs_mut().extend_pairs([("state",pairs["state"].as_str()),("code","fixture-code")]);
            *captured.lock().unwrap() = Some(pairs);
            std::thread::spawn(move || {
                let mut stream = std::net::TcpStream::connect(("127.0.0.1", callback.port().unwrap())).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                write!(stream, "GET {}?{} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", callback.path(), callback.query().unwrap()).unwrap();
                let mut response = String::new(); stream.read_to_string(&mut response).unwrap();
                assert!(response.starts_with("HTTP/1.1 200"));
                assert!(!response.contains("fixture-code"));
            });
            Ok(())
        }));
        let requests = server.join().unwrap();
        assert_eq!(outcome.unwrap(), "fixture-access-token");
        assert!(requests[0].0.starts_with("GET /.well-known/oauth-protected-resource/mcp "));
        let registration: Value = serde_json::from_str(&requests[2].2).unwrap();
        assert_eq!(registration["token_endpoint_auth_method"], "none");
        assert_eq!(registration["redirect_uris"].as_array().unwrap().len(), 1);
        let token_params: std::collections::BTreeMap<_, _> = reqwest::Url::parse(&format!("http://localhost/?{}", requests[3].2)).unwrap()
            .query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect();
        let authorization = authorization.lock().unwrap();
        let params = authorization.as_ref().unwrap();
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["scope"], "images.edit");
        assert_eq!(params["resource"], url);
        assert_eq!(params["client_id"], "public-fixture-client");
        assert_eq!(URL_SAFE_NO_PAD.encode(Sha256::digest(token_params["code_verifier"].as_bytes())), params["code_challenge"]);
        assert_eq!(token_params["code"], "fixture-code");
        assert!(!requests[3].2.contains("client_secret"));
    }

    #[test]
    fn callback_requires_exact_state_and_one_code() {
        assert_eq!(callback_code("GET /mcp-oauth-callback?state=secret&code=ok HTTP/1.1\r\n", "secret").unwrap(), Some("ok".into()));
        for line in [
            "GET /mcp-oauth-callback?state=wrong&code=ok HTTP/1.1",
            "GET /other?state=secret&code=ok HTTP/1.1",
            "GET /mcp-oauth-callback?state=secret&state=secret&code=ok HTTP/1.1",
        ] { assert!(callback_code(line, "secret").unwrap().is_none()); }
        assert!(callback_code("GET /mcp-oauth-callback?state=secret&error=access_denied HTTP/1.1", "secret").is_err());
        assert!(callback_code("GET /mcp-oauth-callback?state=secret&code=a&code=b HTTP/1.1", "secret").is_err());
    }

    #[test]
    fn pkce_is_random_and_urls_are_secure() {
        let verifier = random().unwrap();
        assert_eq!(verifier.len(), 43);
        assert_ne!(verifier, random().unwrap());
        assert_eq!(URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())).len(), 43);
        assert!(secure_url("https://accounts.example.com/authorize").is_ok());
        assert!(secure_url("http://accounts.example.com/token").is_err());
        assert!(secure_url("https://user:pass@example.com/token").is_err());
    }

    #[test]
    fn cancellation_closes_the_callback_without_waiting_for_a_browser() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let error = receive_code(listener, "state".into(), Arc::new(AtomicBool::new(true))).unwrap_err();
        assert_eq!(error, "MCP sign-in cancelled");
    }

    #[test]
    fn rejects_resource_metadata_mismatch_before_opening_a_browser() {
        let (url, server) = crate::mcp::tests::serve_http(|origin| vec![
            (200, "application/json", json!({"resource":format!("{origin}/other"),"authorization_servers":[origin]}).to_string()),
        ]);
        let result = tauri::async_runtime::block_on(authorize(&url, "", "", Arc::new(AtomicBool::new(false)), |_| {
            panic!("Browser must not open for mismatched OAuth metadata")
        }));
        server.join().unwrap();
        assert!(result.unwrap_err().contains("does not match"));
    }
}
