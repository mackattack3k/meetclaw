// Calls the Claude API (raw HTTP — there is no official Rust SDK) to act as a
// quiet third participant: given the rolling meeting transcript, it suggests a
// few sharp questions the user might want to ask next.
//
// Uses structured outputs (output_config.format) so the model returns
// guaranteed-parseable JSON matching our schema.

use serde::Deserialize;
use serde_json::json;

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";

const SYSTEM_PROMPT: &str = "You are a sharp, quiet third participant sitting in on a live meeting. \
You are given a rolling transcript of what has been said so far. \
Suggest up to 3 specific, insightful questions the user might want to ask next, \
grounded in what is actually being discussed. \
Prefer questions that surface assumptions, clarify scope, or move the conversation forward. \
Do not suggest generic questions. If nothing useful comes to mind, return an empty list.";

/// Ask Claude for suggested questions based on the current transcript context.
pub fn suggest_questions(
    api_key: &str,
    model: &str,
    transcript: &str,
) -> Result<Vec<String>, String> {
    let body = json!({
        "model": model,
        "max_tokens": 1024,
        "system": SYSTEM_PROMPT,
        "messages": [{
            "role": "user",
            "content": format!(
                "Live meeting transcript so far:\n\n{transcript}\n\n\
                 Suggest up to 3 questions the user might want to ask next."
            )
        }],
        "output_config": {
            "effort": "low",
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "questions": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["questions"],
                    "additionalProperties": false
                }
            }
        }
    });

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(ANTHROPIC_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("Claude request failed: {e}"))?;

    let status = resp.status();
    let raw = resp
        .text()
        .map_err(|e| format!("failed to read Claude response body: {e}"))?;

    if !status.is_success() {
        return Err(format!("Claude API error {status}: {raw}"));
    }

    let parsed: ApiResponse =
        serde_json::from_str(&raw).map_err(|e| format!("failed to parse Claude response: {e}"))?;

    let json_text = parsed
        .content
        .into_iter()
        .find_map(|block| if block.block_type == "text" { block.text } else { None })
        .ok_or_else(|| "no text block in Claude response".to_string())?;

    let questions: Questions = serde_json::from_str(&json_text)
        .map_err(|e| format!("failed to parse questions JSON: {e}"))?;

    Ok(questions.questions)
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct Questions {
    questions: Vec<String>,
}
