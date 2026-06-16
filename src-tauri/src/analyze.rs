// Calls the Gemini Developer API (generativelanguage.googleapis.com) to act as
// a quiet third participant: given the rolling transcript, it suggests questions
// the user might want to ask next.
//
// Auth is a simple API key (GEMINI_API_KEY or GOOGLE_API_KEY), sent in the
// x-goog-api-key header. Structured output is requested via generationConfig so
// the questions come back as parseable JSON.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

const GEMINI_MODELS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const GEMINI_UPLOAD_URL: &str = "https://generativelanguage.googleapis.com/upload/v1beta/files";
const GEMINI_FILES_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

const TRANSCRIBE_PROMPT: &str = "Transcribe this meeting audio verbatim. Output ONLY the transcript \
text as continuous prose — no timestamps, no speaker labels, no preamble or commentary. The audio \
may be noisy with multiple overlapping speakers; do your best with unclear speech. Keep the \
original spoken language(s); do not translate.";

const SYSTEM_PROMPT: &str = "You are a sharp, quiet third participant sitting in on a live meeting. \
You are given a rolling transcript of what has been said so far, and sometimes the user's own notes. \
Suggest up to 3 specific, insightful questions the user might want to ask next. \
\
Anchor every question to the concrete topic on the table: name the actual systems, technologies, \
people, and decisions being discussed, and ask what a domain expert in that exact area would ask. \
Probe design tradeoffs, hidden assumptions, risks, edge cases, and how success will be measured. \
\
For example, if the transcript says \"let's design a RAG system for Hermes\", good questions are: \
\"What's the chunking strategy and target chunk size for the Hermes corpus?\", \
\"How will we evaluate retrieval quality, and what's the latency budget per query?\", \
\"Which embedding model, and do we re-embed when documents change?\". \
Bad (generic) questions to avoid: \"What are the requirements?\", \"Who owns this?\", \"What's the timeline?\". \
\
When the user has written notes, build on what they clearly care about and do not re-suggest things \
they have already written down. If nothing genuinely useful comes to mind, return an empty list.";

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

/// Transcribe a whole audio file with Gemini (optional high-accuracy final
/// pass). Uploads the file via the Files API, then asks `model` for a verbatim
/// transcript. Returns the transcript text.
pub fn transcribe_audio(api_key: &str, model: &str, audio_path: &Path) -> Result<String, String> {
    let bytes =
        std::fs::read(audio_path).map_err(|e| format!("failed to read audio for transcription: {e}"))?;
    let file_uri = upload_audio(api_key, &bytes, "audio/wav")?;

    let url = format!("{GEMINI_MODELS_URL}/{model}:generateContent");
    let body = json!({
        "contents": [{
            "role": "user",
            "parts": [
                { "text": TRANSCRIBE_PROMPT },
                { "fileData": { "mimeType": "audio/wav", "fileUri": file_uri } }
            ]
        }],
        "generationConfig": { "temperature": 0.0 }
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(&url)
        .header("x-goog-api-key", api_key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("Gemini transcription request failed: {e}"))?;
    let status = resp.status();
    let raw = resp
        .text()
        .map_err(|e| format!("failed to read transcription response: {e}"))?;
    if !status.is_success() {
        return Err(format!("Gemini transcription error {status}: {raw}"));
    }
    Ok(first_text(&raw)?.unwrap_or_default().trim().to_string())
}

/// Upload audio bytes via the Gemini Files API (resumable upload, single chunk),
/// then poll until the file is ACTIVE. Returns the file URI to reference.
fn upload_audio(api_key: &str, bytes: &[u8], mime: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;

    // 1. Start the resumable upload; the server returns an upload URL in a header.
    let start = client
        .post(GEMINI_UPLOAD_URL)
        .header("x-goog-api-key", api_key)
        .header("X-Goog-Upload-Protocol", "resumable")
        .header("X-Goog-Upload-Command", "start")
        .header("X-Goog-Upload-Header-Content-Length", bytes.len().to_string())
        .header("X-Goog-Upload-Header-Content-Type", mime)
        .header("content-type", "application/json")
        .json(&json!({ "file": { "display_name": "meeting-audio" } }))
        .send()
        .map_err(|e| format!("file upload start failed: {e}"))?;
    let upload_url = start
        .headers()
        .get("x-goog-upload-url")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or("file upload start returned no upload URL")?;

    // 2. Upload all bytes and finalize in one request.
    let resp = client
        .post(&upload_url)
        .header("x-goog-api-key", api_key)
        .header("X-Goog-Upload-Offset", "0")
        .header("X-Goog-Upload-Command", "upload, finalize")
        .body(bytes.to_vec())
        .send()
        .map_err(|e| format!("file upload failed: {e}"))?;
    let raw = resp.text().map_err(|e| e.to_string())?;
    let v: Value =
        serde_json::from_str(&raw).map_err(|e| format!("failed to parse upload response: {e}"))?;
    let name = v["file"]["name"]
        .as_str()
        .ok_or("upload response missing file name")?
        .to_string();
    let uri = v["file"]["uri"]
        .as_str()
        .ok_or("upload response missing file uri")?
        .to_string();
    let mut state = v["file"]["state"].as_str().unwrap_or("").to_string();

    // 3. Audio is processed server-side; poll until ACTIVE before using it.
    for _ in 0..60 {
        match state.as_str() {
            "ACTIVE" => return Ok(uri),
            "FAILED" => return Err("uploaded file failed processing".to_string()),
            _ => {}
        }
        std::thread::sleep(Duration::from_secs(1));
        let g = client
            .get(format!("{GEMINI_FILES_BASE}/{name}"))
            .header("x-goog-api-key", api_key)
            .send()
            .map_err(|e| format!("file status check failed: {e}"))?;
        let gv: Value = serde_json::from_str(&g.text().map_err(|e| e.to_string())?)
            .map_err(|e| format!("failed to parse file status: {e}"))?;
        state = gv["state"].as_str().unwrap_or("").to_string();
    }
    Err("timed out waiting for uploaded file to become ACTIVE".to_string())
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
