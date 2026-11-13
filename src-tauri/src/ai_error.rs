use serde_json::Value;

pub fn describe(provider: &str, value: &Value, fallback: &str) -> String {
    let mut details = Vec::new();
    collect(value, &mut details, 0);
    let detail = details.join("; ");
    let lower = detail.to_ascii_lowercase();
    let blocked = [
        "moderation_blocked", "content_policy_violation", "content_filter",
        "content filter", "safety system", "safety_violations", "safety policy",
        "content policy", "responsibleaipolicyviolation", "content blocked",
    ].iter().any(|marker| lower.contains(marker));
    let detail = clean(&detail);
    if blocked {
        format!("Content blocked by {provider}. Provider reason: {detail}. These restrictions come from the provider, not Texture Helper. No automatic retry was made.")
    } else if detail.is_empty() {
        format!("{provider}: {fallback} The provider did not supply a reason. No automatic retry was made.")
    } else {
        format!("{provider}: {detail}. No automatic retry was made.")
    }
}

fn collect(value: &Value, details: &mut Vec<String>, depth: usize) {
    if depth > 5 || details.len() >= 16 { return; }
    match value {
        Value::String(text) if !text.trim().is_empty() => {
            let text: String = text.chars().take(4000).collect();
            if !details.contains(&text) { details.push(text); }
        }
        Value::Array(values) => {
            for value in values.iter().take(16) { collect(value, details, depth + 1); }
        }
        Value::Object(object) => {
            // Only provider error fields: never serialize arbitrary payloads, images or credentials.
            for field in ["error", "failure", "message", "code", "reason", "refusal", "type",
                "moderation_details", "categories", "category", "moderation_stage", "data", "text"] {
                if let Some(value) = object.get(field) { collect(value, details, depth + 1); }
            }
        }
        _ => {}
    }
}

fn clean(text: &str) -> String {
    let secret = regex::Regex::new(r#"(?i)(bearer\s+|(?:access_token|refresh_token|id_token|api_key)["\s:=]+)[^\s",;}]+|sk-[A-Za-z0-9_-]+"#).unwrap();
    let text = secret.replace_all(text, "[redacted]");
    text.chars().filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t')).take(2000).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identifies_blocks_and_retains_provider_category_without_secrets() {
        for provider in ["ChatGPT / Codex", "GitHub Copilot", "OpenAI API", "MCP"] {
            let error = describe(provider, &json!({"error":{"code":"moderation_blocked",
                "message":"Rejected by the safety system. Bearer private-token",
                "moderation_details":{"moderation_stage":"output","categories":["violence"]}},
                "access_token":"secret-value","image":"private-image"}), "Generation failed.");
            assert!(error.starts_with("Content blocked by "));
            assert!(error.contains("violence"));
            assert!(error.contains("output"));
            assert!(!error.contains("private-token"));
            assert!(!error.contains("secret-value"));
            assert!(!error.contains("private-image"));
        }
    }

    #[test]
    fn does_not_mislabel_quota_or_unknown_failures_as_moderation() {
        let error = describe("Codex", &json!({"type":"usageLimitExceeded"}), "Generation failed.");
        assert!(!error.contains("Content blocked"));
        assert!(error.contains("usageLimitExceeded"));
        assert!(describe("Codex", &Value::Null, "Generation failed.").contains("did not supply a reason"));
        assert!(describe("MCP", &json!("Blocked by content filters: violence"), "").contains("Content blocked"));
    }
}
