mod analyze;
mod audio;
mod transcribe;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use audio::{resample, AudioCapture};
use transcribe::Transcriber;

const TARGET_RATE: u32 = 16_000;
// How many seconds of audio to buffer before running a transcription pass.
const CHUNK_SECONDS: f32 = 5.0;

// Gemini analysis (Phase 2).
// Default model; the user can change it live from the UI dropdown.
const DEFAULT_MODEL: &str = "gemini-3.5-flash";
// How often (at most) to ask Claude for fresh question suggestions.
const ANALYSIS_INTERVAL_SECONDS: u64 = 20;
// How much recent transcript (in characters) to send as context.
const MAX_CONTEXT_CHARS: usize = 4000;

struct AppState {
    running: Arc<AtomicBool>,
    model: Arc<Mutex<String>>,
}

#[derive(Clone, Serialize)]
struct TranscriptPayload {
    text: String,
}

#[derive(Clone, Serialize)]
struct SuggestionsPayload {
    questions: Vec<String>,
}

#[tauri::command]
fn start_listening(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    // If already running, do nothing.
    if state.running.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let running = state.running.clone();
    let model = state.model.clone();

    std::thread::spawn(move || {
        if let Err(e) = run_pipeline(&app, running.clone(), model) {
            let _ = app.emit("transcribe-error", e);
        }
        running.store(false, Ordering::SeqCst);
        let _ = app.emit("listening-stopped", ());
    });

    Ok(())
}

#[tauri::command]
fn stop_listening(state: State<AppState>) {
    state.running.store(false, Ordering::SeqCst);
}

#[tauri::command]
fn set_model(model: String, state: State<AppState>) {
    if let Ok(mut current) = state.model.lock() {
        *current = model;
    }
}

fn model_path() -> String {
    format!("{}/models/ggml-base.en.bin", env!("CARGO_MANIFEST_DIR"))
}

/// Join the transcript history and keep only the most recent `max_chars`
/// characters (char-safe, so it never splits a multi-byte character).
fn recent_context(history: &[String], max_chars: usize) -> String {
    let joined = history.join(" ");
    let char_count = joined.chars().count();
    if char_count <= max_chars {
        return joined;
    }
    joined.chars().skip(char_count - max_chars).collect()
}

fn run_pipeline(
    app: &AppHandle,
    running: Arc<AtomicBool>,
    model: Arc<Mutex<String>>,
) -> Result<(), String> {
    // Load the model first so any error surfaces before we touch the mic.
    let transcriber = Transcriber::new(&model_path())?;

    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let capture = AudioCapture::start(tx)?;
    let native_rate = capture.sample_rate;

    let chunk_native_len = (native_rate as f32 * CHUNK_SECONDS) as usize;
    let mut buffer: Vec<f32> = Vec::with_capacity(chunk_native_len);

    // Phase 2: rolling transcript + periodic Gemini analysis.
    let api_key = analyze::api_key();
    if api_key.is_none() {
        let _ = app.emit(
            "analysis-disabled",
            "No GEMINI_API_KEY set — question suggestions are off. Get a key at \
             https://ai.google.dev/gemini-api/docs/api-key",
        );
    }
    let mut transcript_history: Vec<String> = Vec::new();
    let mut last_analysis = Instant::now();

    let _ = app.emit("listening-started", ());

    while running.load(Ordering::SeqCst) {
        // Pull whatever audio is available without busy-waiting.
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(samples) => buffer.extend_from_slice(&samples),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if buffer.len() >= chunk_native_len {
            // Take one chunk's worth, leave the remainder for the next pass.
            let chunk: Vec<f32> = buffer.drain(..chunk_native_len).collect();
            let resampled = resample(&chunk, native_rate, TARGET_RATE);

            match transcriber.transcribe(&resampled) {
                Ok(text) if !text.is_empty() => {
                    transcript_history.push(text.clone());
                    let _ = app.emit("transcript", TranscriptPayload { text });

                    // Periodically ask Gemini for question suggestions. Run the
                    // call on its own thread so transcription keeps flowing.
                    if let Some(key) = &api_key {
                        if last_analysis.elapsed()
                            >= Duration::from_secs(ANALYSIS_INTERVAL_SECONDS)
                        {
                            last_analysis = Instant::now();
                            let context = recent_context(&transcript_history, MAX_CONTEXT_CHARS);
                            let key = key.clone();
                            let selected_model = model
                                .lock()
                                .map(|m| m.clone())
                                .unwrap_or_else(|_| DEFAULT_MODEL.to_string());
                            let app_for_analysis = app.clone();
                            std::thread::spawn(move || {
                                match analyze::suggest_questions(&key, &selected_model, &context) {
                                    Ok(questions) if !questions.is_empty() => {
                                        let _ = app_for_analysis
                                            .emit("suggestions", SuggestionsPayload { questions });
                                    }
                                    Ok(_) => {} // nothing useful this round
                                    Err(e) => {
                                        let _ = app_for_analysis.emit("analysis-error", e);
                                    }
                                }
                            });
                        }
                    }
                }
                Ok(_) => {} // silence / no speech
                Err(e) => {
                    let _ = app.emit("transcribe-error", e);
                }
            }
        }
    }

    // Dropping the capture stops the audio stream.
    drop(capture);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            app.manage(AppState {
                running: Arc::new(AtomicBool::new(false)),
                model: Arc::new(Mutex::new(DEFAULT_MODEL.to_string())),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_listening,
            stop_listening,
            set_model
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
