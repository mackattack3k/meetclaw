// Calls the Gemini Developer API (generativelanguage.googleapis.com) to act as
// a quiet third participant: given the rolling transcript, it suggests questions
// the user might want to ask next.
//
// Auth is a simple API key (GEMINI_API_KEY or GOOGLE_API_KEY), sent in the
// x-goog-api-key header. Structured output is requested via generationConfig so
// the questions come back as parseable JSON.

use serde::Deserialize;
use serde_json::json;

const GEMINI_MODELS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";

const SYSTEM_PROMPT: &str = "You are a sharp, quiet third participant sitting in on a live meeting. \
You are given a rolling transcript of what has been said so far, and sometimes the user's own notes. \
Suggest up to 3 specific, insightful questions the user might want to ask next, \
grounded in what is actually being discussed. \
When the user has written notes, take them into account: build on what they clearly care about, \
and do not re-suggest questions about things they have already written down. \
Prefer questions that surface assumptions, clarify scope, or move the conversation forward. \
Do not suggest generic questions. If nothing useful comes to mind, return an empty list.";

/// Read the Gemini API key: keychain first (set via Settings), then env / .env.
pub fn api_key() -> Option<String> {
    if let Some(key) = crate::settings::get_api_key() {
        return Some(key);
    }
    for var in ["GEMINI_API_KEY", "GOOGLE_API_KEY"] {
        if let Ok(key) = std::env::var(var) {
            if !key.trim().is_empty() {
                return Some(key);
            }
        }
    }
    None
}

/// Ask Gemini for suggested questions based on the current transcript context.
pub fn suggest_questions(
    api_key: &str,
    model: &str,
    transcript: &str,
    notes: &str,
) -> Result<Vec<String>, String> {
    let url = format!("{GEMINI_MODELS_URL}/{model}:generateContent");

    let notes_section = if notes.trim().is_empty() {
        String::new()
    } else {
        format!("The user's own notes so far:\n\n{notes}\n\n")
    };
    let user_text = format!(
        "{notes_section}Live meeting transcript so far:\n\n{transcript}\n\n\
         Suggest up to 3 questions the user might want to ask next."
    );

    let body = json!({
        "systemInstruction": { "parts": [{ "text": SYSTEM_PROMPT }] },
        "contents": [{
            "role": "user",
            "parts": [{ "text": user_text }]
        }],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseSchema": {
                "type": "OBJECT",
                "properties": {
                    "questions": { "type": "ARRAY", "items": { "type": "STRING" } }
                },
                "required": ["questions"]
            }
        }
    });

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(&url)
        .header("x-goog-api-key", api_key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("Gemini request failed: {e}"))?;

    let status = resp.status();
    let raw = resp
        .text()
        .map_err(|e| format!("failed to read Gemini response body: {e}"))?;

    if !status.is_success() {
        return Err(format!("Gemini API error {status}: {raw}"));
    }

    let parsed: GeminiResponse =
        serde_json::from_str(&raw).map_err(|e| format!("failed to parse Gemini response: {e}"))?;

    // No candidate text (e.g. a safety filter) just means no suggestions this round.
    let json_text = parsed
        .candidates
        .into_iter()
        .flat_map(|c| c.content.parts)
        .find_map(|p| p.text);
    let json_text = match json_text {
        Some(text) => text,
        None => return Ok(Vec::new()),
    };

    let questions: Questions = serde_json::from_str(&json_text)
        .map_err(|e| format!("failed to parse questions JSON: {e}"))?;
    Ok(questions.questions)
}

/// Generate a short meeting title from the transcript/notes context.
pub fn generate_title(api_key: &str, model: &str, context: &str) -> Result<String, String> {
    let url = format!("{GEMINI_MODELS_URL}/{model}:generateContent");

    let body = json!({
        "systemInstruction": { "parts": [{ "text": "You write short, specific titles for meetings." }] },
        "contents": [{
            "role": "user",
            "parts": [{
                "text": format!(
                    "Based on the following meeting content, write a concise, specific title of \
                     3 to 7 words. Return only the title: no quotes, no preamble, no trailing \
                     punctuation.\n\n{context}"
                )
            }]
        }],
        "generationConfig": { "responseMimeType": "text/plain" }
    });

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post(&url)
        .header("x-goog-api-key", api_key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("Gemini title request failed: {e}"))?;

    let status = resp.status();
    let raw = resp
        .text()
        .map_err(|e| format!("failed to read Gemini title response: {e}"))?;
    if !status.is_success() {
        return Err(format!("Gemini title error {status}: {raw}"));
    }

    let text = first_text(&raw)?.unwrap_or_default();
    Ok(clean_title(&text))
}

/// Walk a Gemini response and return the first text part, if any.
fn first_text(raw: &str) -> Result<Option<String>, String> {
    let parsed: GeminiResponse =
        serde_json::from_str(raw).map_err(|e| format!("failed to parse Gemini response: {e}"))?;
    Ok(parsed
        .candidates
        .into_iter()
        .flat_map(|c| c.content.parts)
        .find_map(|p| p.text))
}

/// Tidy a model-produced title: first line, no surrounding quotes or "Title:" prefix.
fn clean_title(s: &str) -> String {
    let t = s.trim();
    let t = t.lines().next().unwrap_or(t).trim();
    let t = t.strip_prefix("Title:").unwrap_or(t).trim();
    t.trim_matches('"').trim().to_string()
}

#[derive(Deserialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
}

#[derive(Deserialize)]
struct Candidate {
    #[serde(default)]
    content: Content,
}

#[derive(Deserialize, Default)]
struct Content {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Part {
    text: Option<String>,
}

#[derive(Deserialize)]
struct Questions {
    questions: Vec<String>,
}
