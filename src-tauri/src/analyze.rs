// Calls Claude (raw HTTP — there is no official Rust SDK for the Messages API
// on either the direct Anthropic endpoint or Vertex) to act as a quiet third
// participant: given the rolling transcript, it suggests questions to ask next.
//
// Two providers, auto-selected from the environment:
//   - Anthropic direct: ANTHROPIC_API_KEY
//   - Google Vertex AI: VERTEX_PROJECT_ID (+ GOOGLE_APPLICATION_CREDENTIALS or ADC)
//
// Structured outputs (output_config.format) are supported on both, so questions
// come back as guaranteed-parseable JSON.

use std::sync::{Arc, Mutex, OnceLock};

use gcp_auth::TokenProvider;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::runtime::Runtime;

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const VERTEX_ANTHROPIC_VERSION: &str = "vertex-2023-10-16";

const SYSTEM_PROMPT: &str = "You are a sharp, quiet third participant sitting in on a live meeting. \
You are given a rolling transcript of what has been said so far. \
Suggest up to 3 specific, insightful questions the user might want to ask next, \
grounded in what is actually being discussed. \
Prefer questions that surface assumptions, clarify scope, or move the conversation forward. \
Do not suggest generic questions. If nothing useful comes to mind, return an empty list.";

/// Which backend to talk to. Chosen once from the environment.
#[derive(Clone)]
pub enum Provider {
    Anthropic { api_key: String },
    Vertex { project_id: String, region: String },
}

/// Pick a provider from the environment, preferring the direct Anthropic API.
pub fn detect_provider() -> Option<Provider> {
    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
        if !api_key.trim().is_empty() {
            return Some(Provider::Anthropic { api_key });
        }
    }
    if let Ok(project_id) = std::env::var("VERTEX_PROJECT_ID") {
        if !project_id.trim().is_empty() {
            let region = std::env::var("VERTEX_REGION").unwrap_or_else(|_| "global".to_string());
            return Some(Provider::Vertex { project_id, region });
        }
    }
    None
}

/// Ask Claude for suggested questions based on the current transcript context.
pub fn suggest_questions(
    provider: &Provider,
    model: &str,
    transcript: &str,
) -> Result<Vec<String>, String> {
    let client = reqwest::blocking::Client::new();
    let is_vertex = matches!(provider, Provider::Vertex { .. });
    let body = request_body(model, transcript, is_vertex);

    let raw = match provider {
        Provider::Anthropic { api_key } => {
            let resp = client
                .post(ANTHROPIC_URL)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .map_err(|e| format!("Claude request failed: {e}"))?;
            read_ok(resp)?
        }
        Provider::Vertex { project_id, region } => {
            let token = vertex_access_token()?;
            let url = vertex_url(project_id, region, &vertex_model_id(model));
            let resp = client
                .post(url)
                .bearer_auth(token)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .map_err(|e| format!("Vertex request failed: {e}"))?;
            read_ok(resp)?
        }
    };

    parse_questions(&raw)
}

fn request_body(model: &str, transcript: &str, is_vertex: bool) -> Value {
    let mut body = json!({
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
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "questions": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["questions"],
                    "additionalProperties": false
                }
            }
        }
    });

    if is_vertex {
        // On Vertex the model lives in the URL; the body carries anthropic_version.
        body["anthropic_version"] = json!(VERTEX_ANTHROPIC_VERSION);
    } else {
        body["model"] = json!(model);
    }
    body
}

/// Map a direct-API model id to its Vertex equivalent. Most are identical;
/// Haiku 4.5 carries a date suffix on Vertex.
fn vertex_model_id(model: &str) -> String {
    match model {
        "claude-haiku-4-5" => "claude-haiku-4-5@20251001".to_string(),
        other => other.to_string(),
    }
}

fn vertex_url(project_id: &str, region: &str, model_id: &str) -> String {
    // Global endpoint drops the region prefix from the host; regional/multi-region keep it.
    let host = if region == "global" {
        "aiplatform.googleapis.com".to_string()
    } else {
        format!("{region}-aiplatform.googleapis.com")
    };
    format!(
        "https://{host}/v1/projects/{project_id}/locations/{region}\
         /publishers/anthropic/models/{model_id}:rawPredict"
    )
}

fn read_ok(resp: reqwest::blocking::Response) -> Result<String, String> {
    let status = resp.status();
    let raw = resp
        .text()
        .map_err(|e| format!("failed to read response body: {e}"))?;
    if !status.is_success() {
        return Err(format!("API error {status}: {raw}"));
    }
    Ok(raw)
}

fn parse_questions(raw: &str) -> Result<Vec<String>, String> {
    let parsed: ApiResponse =
        serde_json::from_str(raw).map_err(|e| format!("failed to parse response: {e}"))?;

    let json_text = parsed
        .content
        .into_iter()
        .find_map(|block| if block.block_type == "text" { block.text } else { None })
        .ok_or_else(|| "no text block in response".to_string())?;

    let questions: Questions = serde_json::from_str(&json_text)
        .map_err(|e| format!("failed to parse questions JSON: {e}"))?;
    Ok(questions.questions)
}

// --- Vertex / GCP auth -----------------------------------------------------

struct VertexAuth {
    rt: Runtime,
    provider: Arc<dyn TokenProvider>,
}

// gcp_auth is async; we bridge it into our blocking pipeline with one runtime,
// built lazily and reused. gcp_auth caches tokens internally.
static VERTEX_AUTH: OnceLock<Result<Mutex<VertexAuth>, String>> = OnceLock::new();

fn vertex_access_token() -> Result<String, String> {
    let cell = VERTEX_AUTH.get_or_init(|| {
        let rt = Runtime::new().map_err(|e| format!("failed to start tokio runtime: {e}"))?;
        let provider = rt.block_on(gcp_auth::provider()).map_err(|e| {
            format!(
                "GCP auth failed (set GOOGLE_APPLICATION_CREDENTIALS to a service \
                 account key, or run `gcloud auth application-default login`): {e}"
            )
        })?;
        Ok(Mutex::new(VertexAuth { rt, provider }))
    });

    let auth = cell.as_ref().map_err(|e| e.clone())?;
    let guard = auth.lock().map_err(|_| "vertex auth lock poisoned".to_string())?;
    let token = guard
        .rt
        .block_on(
            guard
                .provider
                .token(&["https://www.googleapis.com/auth/cloud-platform"]),
        )
        .map_err(|e| format!("failed to get GCP access token: {e}"))?;
    Ok(token.as_str().to_string())
}

// --- Response shapes (shared by both providers) ----------------------------

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
