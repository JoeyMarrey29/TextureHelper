use base64::{Engine, engine::general_purpose::STANDARD};
use image::{ImageFormat, Rgba, RgbaImage, imageops::{self, FilterType}};
use reqwest::{Client, header::HeaderValue};
use serde::Serialize;
use serde_json::{Value, json};
use std::{io::Cursor, sync::Mutex, time::Duration};

const ENDPOINT: &str = "https://api.openai.com/v1/images/edits";
const WORKING_SIZE: u32 = 1024;
const MAX_RESPONSE: usize = 48 * 1024 * 1024;

#[derive(Clone)]
struct Session {
    id: String,
    token: HeaderValue,
    client: Client,
}

#[derive(Default)]
pub struct OpenAiState(Mutex<Option<Session>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    connection_id: String,
}

fn session(api_key: &str) -> Result<Session, String> {
    let key = api_key.trim();
    if key.is_empty() || key.len() > 4096 || key.bytes().any(|byte| !byte.is_ascii_graphic()) {
        return Err("Enter a valid OpenAI API key, not your ChatGPT password".into());
    }
    let mut token = HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| "Invalid OpenAI API key")?;
    token.set_sensitive(true);
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|_| "Could not create an OpenAI session identifier")?;
    Ok(Session {
        id: random.iter().map(|byte| format!("{byte:02x}")).collect(), token,
        client: Client::builder().redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(300))
            .build().map_err(|_| "Could not create the OpenAI API client")?,
    })
}

#[tauri::command]
pub fn openai_connect(state: tauri::State<'_, OpenAiState>, api_key: String) -> Result<Connected, String> {
    let session = session(&api_key)?;
    let result = Connected { connection_id:session.id.clone() };
    *state.0.lock().map_err(|_| "OpenAI session lock failed")? = Some(session);
    Ok(result)
}

#[tauri::command]
pub fn openai_disconnect(state: tauri::State<'_, OpenAiState>) -> Result<(), String> {
    *state.0.lock().map_err(|_| "OpenAI session lock failed")? = None;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct Frame { x: u32, y: u32, width: u32, height: u32 }

fn prepare(original: &RgbaImage) -> Result<(String, Frame), String> {
    let longest = original.width().max(original.height());
    let width = ((original.width() as u64 * WORKING_SIZE as u64) / longest as u64).max(1) as u32;
    let height = ((original.height() as u64 * WORKING_SIZE as u64) / longest as u64).max(1) as u32;
    let mut rgb = original.clone();
    for pixel in rgb.pixels_mut() { pixel[3] = 255; }
    let resized = imageops::resize(&rgb, width, height, FilterType::Nearest);
    let frame = Frame { x:(WORKING_SIZE - width) / 2, y:(WORKING_SIZE - height) / 2, width, height };
    let mut padded = RgbaImage::from_pixel(WORKING_SIZE, WORKING_SIZE, Rgba([32, 32, 32, 255]));
    imageops::replace(&mut padded, &resized, frame.x as i64, frame.y as i64);
    let mut bytes = Cursor::new(Vec::new());
    padded.write_to(&mut bytes, ImageFormat::Png).map_err(|error| error.to_string())?;
    Ok((format!("data:image/png;base64,{}", STANDARD.encode(bytes.into_inner())), frame))
}

fn request_body(model: &str, quality: &str, prompt: &str, label: &str, original: &RgbaImage) -> Result<(Value, Frame), String> {
    if model.is_empty() || model.len() > 100 || !model.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_')) {
        return Err("Enter an OpenAI image-editing model ID, for example gpt-image-2.5-sunburst".into());
    }
    if !matches!(quality, "low" | "medium" | "high") { return Err("Choose low, medium or high image quality".into()); }
    if prompt.trim().is_empty() || prompt.len() > 16000 || label.len() > 4096 {
        return Err("Enter a prompt of up to 16,000 bytes; texture labels must be at most 4,096 bytes".into());
    }
    super::textures::validate_replacement_dimensions(original.width(), original.height(), original.width(), original.height())?;
    let (png, frame) = prepare(original)?;
    let prompt = format!("{}\nThe supplied image is a 1024 x 1024 square editing canvas. \
Only edit the texture rectangle at x={}, y={}, width={}, height={} (pixel coordinates, origin at top left). \
Keep that rectangle in exactly the same position; do not crop, rotate, zoom, rearrange UV islands, add borders, \
or render a 3D object. Leave padding unchanged. Return the complete 1024 x 1024 image. \
The app maps the rectangle back to the original {} x {} texture and preserves its original alpha mask.\n\
Texture label: {label}\nUser's cosmetic editing request:\n{prompt}",
        super::mcp::CONTEXT, frame.x, frame.y, frame.width, frame.height, original.width(), original.height());
    Ok((json!({
        "model":model, "images":[{"image_url":png}], "prompt":prompt,
        "n":1, "size":"1024x1024", "quality":quality, "output_format":"png"
    }), frame))
}

fn output_image(result: &Value, original: &RgbaImage, frame: &Frame) -> Result<String, String> {
    if result.get("error").is_some_and(|error| !error.is_null()) || result.get("refusal").is_some_and(|refusal| !refusal.is_null()) {
        return Err(super::ai_error::describe("OpenAI API", result, "Image generation was refused."));
    }
    let data = result["data"].as_array().filter(|data| data.len() == 1)
        .ok_or("OpenAI must return exactly one image. No edits applied.")?;
    let encoded = data[0]["b64_json"].as_str().filter(|data| !data.is_empty())
        .ok_or("OpenAI returned no base64 image. URLs and files are not downloaded.")?;
    let generated = super::mcp::decode_image(encoded, ImageFormat::Png)?;
    if generated.dimensions() != (WORKING_SIZE, WORKING_SIZE) {
        return Err("OpenAI returned an unexpected canvas size; expected 1024 x 1024. No edits applied.".into());
    }
    let cropped = imageops::crop_imm(&generated, frame.x, frame.y, frame.width, frame.height).to_image();
    let edited = imageops::resize(&cropped, original.width(), original.height(), FilterType::Lanczos3);
    super::mcp::preserve_alpha(edited, original)
}

async fn edit(session: &Session, endpoint: &str, model: &str, quality: &str, prompt: &str, png: &str, label: &str) -> Result<String, String> {
    let original = super::mcp::decode_image(png.strip_prefix("data:image/png;base64,").unwrap_or(png), ImageFormat::Png)?;
    let (body, frame) = request_body(model, quality, prompt, label, &original)?;
    let mut response = session.client.post(endpoint).header("Authorization", &session.token)
        .json(&body).send().await.map_err(|error| {
            if error.is_timeout() { "OpenAI request timed out. It may still complete or incur charges.".to_string() }
            else { "Could not reach the OpenAI API. Check your internet connection.".to_string() }
        })?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "OpenAI response interrupted or timed out; the request may still incur charges")? {
        if bytes.len() + chunk.len() > MAX_RESPONSE { return Err("OpenAI response exceeds 48 MiB".into()); }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let value = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
        let detail = super::ai_error::describe("OpenAI API", &value, "Generation request failed.");
        let key = session.token.to_str().ok().and_then(|token| token.strip_prefix("Bearer ")).unwrap_or("");
        let detail = if key.is_empty() { detail } else { detail.replace(key, "[redacted]") };
        let advice = match status.as_u16() {
            401 => "Check your OpenAI API key (not your ChatGPT password).",
            403 => "Check model access, project permissions and any organization-verification requirements.",
            429 => "Check API billing, quota and rate limits. ChatGPT subscription billing is separate.",
            _ => "No automatic retry was made. Review the error before trying again.",
        };
        return Err(format!("OpenAI API HTTP {status}: {} {advice}", detail.chars().take(2000).collect::<String>()));
    }
    let result = serde_json::from_slice(&bytes).map_err(|_| "OpenAI returned invalid JSON")?;
    output_image(&result, &original, &frame)
}

#[tauri::command]
pub async fn openai_edit_texture(
    state: tauri::State<'_, OpenAiState>, connection_id: String, model: String, quality: String,
    prompt: String, png: String, label: String,
) -> Result<String, String> {
    let session = state.0.lock().map_err(|_| "OpenAI session lock failed")?
        .as_ref().filter(|session| session.id == connection_id).cloned().ok_or("OpenAI session changed; enter your API key again")?;
    edit(&session, ENDPOINT, &model, &quality, &prompt, &png, &label).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(image: &RgbaImage) -> String {
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        STANDARD.encode(bytes.into_inner())
    }

    #[test]
    fn prepares_square_working_canvas_without_losing_hidden_rgb_or_aspect_ratio() {
        let original = RgbaImage::from_pixel(16, 8, Rgba([90, 80, 70, 0]));
        let (body, frame) = request_body("gpt-image-2.5-sunburst", "medium", "Blue finish", "Stock", &original).unwrap();
        assert_eq!(frame, Frame { x:0, y:256, width:1024, height:512 });
        let input = body["images"][0]["image_url"].as_str().unwrap();
        let decoded = super::super::mcp::decode_image(input.strip_prefix("data:image/png;base64,").unwrap(), ImageFormat::Png).unwrap();
        assert_eq!(decoded.get_pixel(512, 512).0, [90, 80, 70, 255]);
        assert_eq!(decoded.get_pixel(512, 0).0, [32, 32, 32, 255]);
        assert_eq!(body["size"], "1024x1024");
        assert_eq!(body["n"], 1);
        assert_eq!(body["output_format"], "png");
        assert!(body.get("moderation").is_none());
        assert!(body["prompt"].as_str().unwrap().contains("fictional PERPHEADS"));
        assert!(body["prompt"].as_str().unwrap().contains("Blue finish"));
        assert!(body["prompt"].as_str().unwrap().contains("y=256"));
    }

    #[test]
    fn maps_output_to_original_dimensions_and_restores_each_alpha_value() {
        let mut original = RgbaImage::from_pixel(8, 16, Rgba([1, 2, 3, 17]));
        original.get_pixel_mut(0, 0)[3] = 0;
        original.get_pixel_mut(1, 0)[3] = 255;
        let (_, frame) = prepare(&original).unwrap();
        assert_eq!(frame, Frame { x:256, y:0, width:512, height:1024 });
        let mut generated = RgbaImage::from_pixel(1024, 1024, Rgba([255, 0, 0, 255]));
        for y in 0..1024 { for x in 256..768 { generated.put_pixel(x, y, Rgba([90, 80, 70, 255])); } }
        let result = json!({"data":[{"b64_json":encoded(&generated)}]});
        let png = output_image(&result, &original, &frame).unwrap();
        let decoded = super::super::mcp::decode_image(png.strip_prefix("data:image/png;base64,").unwrap(), ImageFormat::Png).unwrap();
        assert_eq!(decoded.dimensions(), original.dimensions());
        assert!(decoded.pixels().zip(original.pixels()).all(|(edited, source)| edited[3] == source[3] && edited.0[..3] == [90, 80, 70]));
        assert!(output_image(&json!({"data":[]}), &original, &frame).is_err());
        assert!(output_image(&json!({"data":[{"url":"https://example.test/image.png"}]}), &original, &frame).is_err());
        let wrong_size = json!({"data":[{"b64_json":encoded(&RgbaImage::new(4,4))}]});
        assert!(output_image(&wrong_size, &original, &frame).unwrap_err().contains("canvas size"));
    }

    #[test]
    fn rejects_invalid_settings_without_requests() {
        assert!(session("").is_err());
        assert!(session("key\r\nInjected: value").is_err());
        let original = RgbaImage::new(4, 4);
        assert!(request_body("", "medium", "Blue", "Stock", &original).is_err());
        assert!(request_body("gpt-image-2.5-sunburst", "invalid", "Blue", "Stock", &original).is_err());
        assert!(request_body("gpt-image-2.5-sunburst", "medium", "", "Stock", &original).is_err());
        assert!(request_body("gpt-image-2.5-sunburst", "medium", "Blue", "Stock", &RgbaImage::new(0, 0)).is_err());
    }

    #[test]
    fn posts_image_edit_to_api_and_returns_one_validated_png() {
        let generated = RgbaImage::from_pixel(1024, 1024, Rgba([10, 20, 30, 255]));
        let (url, server) = crate::mcp::tests::serve_http(|_| vec![
            (200, "application/json", json!({"data":[{"b64_json":encoded(&generated)}]}).to_string()),
        ]);
        let original = RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 7]));
        let session = session("fixture-key").unwrap();
        let png = tauri::async_runtime::block_on(edit(&session, &url, "gpt-image-2.5-sunburst", "low", "Blue", &encoded(&original), "Stock")).unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].1["authorization"], "Bearer fixture-key");
        let body: Value = serde_json::from_str(&requests[0].2).unwrap();
        assert_eq!(body["model"], "gpt-image-2.5-sunburst");
        assert_eq!(body["quality"], "low");
        assert_eq!(body["images"].as_array().unwrap().len(), 1);
        let decoded = super::super::mcp::decode_image(png.strip_prefix("data:image/png;base64,").unwrap(), ImageFormat::Png).unwrap();
        assert_eq!(decoded.get_pixel(0,0).0, [10, 20, 30, 7]);
    }

    #[test]
    fn reports_refusals_and_billing_errors_without_retrying_or_leaking_keys() {
        for (code, message) in [(400, "content_policy_violation"), (401, "Invalid fixture-secret-key"), (429, "insufficient_quota")] {
            let (url, server) = crate::mcp::tests::serve_http(|_| vec![
                (code, "application/json", json!({"error":{"message":message}}).to_string()),
            ]);
            let session = session("fixture-secret-key").unwrap();
            let error = tauri::async_runtime::block_on(edit(&session, &url, "gpt-image-2.5-sunburst", "medium",
                "Blue", &encoded(&RgbaImage::new(4,4)), "Stock")).unwrap_err();
            let requests = server.join().unwrap();
            assert_eq!(requests.len(), 1);
            assert!(error.contains(&code.to_string()));
            assert!(!error.contains("fixture-secret-key"));
            if code == 429 { assert!(error.contains("billing")); }
            if code == 400 { assert!(error.contains("Content blocked by OpenAI API")); }
        }
    }

    #[test]
    fn reports_provider_moderation_category_from_http_error() {
        let (url, server) = crate::mcp::tests::serve_http(|_| vec![(400, "application/json",
            json!({"error":{"code":"moderation_blocked","message":"Rejected by the safety system.",
                "moderation_details":{"moderation_stage":"output","categories":["violence"]}}}).to_string())]);
        let error = tauri::async_runtime::block_on(edit(&session("fixture-secret-key").unwrap(), &url,
            "gpt-image-2.5-sunburst", "medium", "Blue", &encoded(&RgbaImage::new(4,4)), "Stock")).unwrap_err();
        assert!(error.contains("Content blocked by OpenAI API"));
        assert!(error.contains("violence"));
        assert!(error.contains("output"));
        assert_eq!(server.join().unwrap().len(),1);
    }
}
