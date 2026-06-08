mod analyze;
mod audio;
mod meeting;
mod transcribe;

use std::path::PathBuf;
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
    notes: Arc<Mutex<String>>,
    meeting: Arc<Mutex<Option<CurrentMeeting>>>,
}

/// The meeting currently being recorded into / edited.
#[derive(Clone)]
struct CurrentMeeting {
    id: String,
    dir: PathBuf,
}

/// Return the current meeting, creating a fresh untitled one if there is none.
fn ensure_meeting(app: &AppHandle, state: &AppState) -> Result<CurrentMeeting, String> {
    let mut guard = state.meeting.lock().map_err(|_| "meeting lock poisoned".to_string())?;
    if let Some(current) = guard.as_ref() {
        return Ok(current.clone());
    }
    let meta = meeting::create(app, "")?;
    let dir = meeting::meeting_dir(app, &meta.id)?;
    let current = CurrentMeeting { id: meta.id, dir };
    *guard = Some(current.clone());
    Ok(current)
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
    let current = match ensure_meeting(&app, &state) {
        Ok(c) => c,
        Err(e) => {
            state.running.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };

    let running = state.running.clone();
    let model = state.model.clone();
    let model_for_title = state.model.clone();
    let notes = state.notes.clone();
    let dir = current.dir;

    std::thread::spawn(move || {
        if let Err(e) = run_pipeline(&app, running.clone(), model, notes, dir.clone()) {
            let _ = app.emit("transcribe-error", e);
        }
        running.store(false, Ordering::SeqCst);
        // Write the WAV from the accumulated PCM now that recording has stopped.
        let _ = meeting::finalize_wav(&dir);
        // Auto-name the meeting if it's still untitled.
        maybe_generate_title(&app, &dir, &model_for_title);
        let _ = app.emit("listening-stopped", ());
    });

    Ok(())
}

/// If the meeting has no real title yet, ask Gemini to name it from its content.
fn maybe_generate_title(app: &AppHandle, dir: &std::path::Path, model: &Arc<Mutex<String>>) {
    let needs_title = match meeting::get_title(dir) {
        Some(t) => t.is_empty() || t == "Untitled meeting",
        None => false,
    };
    if !needs_title {
        return;
    }

    let transcript = meeting::read_transcript(dir);
    if transcript.trim().is_empty() {
        return;
    }
    let Some(key) = analyze::api_key() else {
        return;
    };

    let notes = meeting::read_notes(dir);
    let combined = if notes.trim().is_empty() {
        transcript
    } else {
        format!("Notes:\n{notes}\n\nTranscript:\n{transcript}")
    };
    let context: String = combined.chars().take(6000).collect();

    let selected_model = model
        .lock()
        .map(|m| m.clone())
        .unwrap_or_else(|_| DEFAULT_MODEL.to_string());

    if let Ok(title) = analyze::generate_title(&key, &selected_model, &context) {
        let title = title.trim();
        if !title.is_empty() {
            let _ = meeting::set_title(dir, title);
            let _ = app.emit("title-updated", title);
        }
    }
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

#[tauri::command]
fn set_notes(notes: String, state: State<AppState>) {
    if let Ok(mut current) = state.notes.lock() {
        *current = notes.clone();
    }
    // Persist to the current meeting, if one exists.
    if let Ok(guard) = state.meeting.lock() {
        if let Some(current) = guard.as_ref() {
            let _ = meeting::write_notes(&current.dir, &notes);
        }
    }
}

#[tauri::command]
fn set_title(app: AppHandle, title: String, state: State<AppState>) -> Result<(), String> {
    let current = ensure_meeting(&app, &state)?;
    meeting::set_title(&current.dir, &title)
}

#[tauri::command]
fn new_meeting(app: AppHandle, state: State<AppState>) -> Result<meeting::MeetingMeta, String> {
    let meta = meeting::create(&app, "")?;
    let dir = meeting::meeting_dir(&app, &meta.id)?;
    *state.meeting.lock().map_err(|_| "meeting lock poisoned".to_string())? =
        Some(CurrentMeeting { id: meta.id.clone(), dir });
    if let Ok(mut notes) = state.notes.lock() {
        notes.clear();
    }
    Ok(meta)
}

#[tauri::command]
fn list_meetings(app: AppHandle) -> Result<Vec<meeting::MeetingMeta>, String> {
    meeting::list(&app)
}

#[tauri::command]
fn load_meeting(
    app: AppHandle,
    id: String,
    state: State<AppState>,
) -> Result<meeting::MeetingDetail, String> {
    let detail = meeting::load(&app, &id)?;
    let dir = meeting::meeting_dir(&app, &id)?;
    *state.meeting.lock().map_err(|_| "meeting lock poisoned".to_string())? =
        Some(CurrentMeeting { id: id.clone(), dir });
    if let Ok(mut notes) = state.notes.lock() {
        *notes = detail.notes.clone();
    }
    Ok(detail)
}

#[tauri::command]
fn delete_meeting(app: AppHandle, id: String, state: State<AppState>) -> Result<(), String> {
    meeting::delete(&app, &id)?;
    if let Ok(mut guard) = state.meeting.lock() {
        if guard.as_ref().map(|m| m.id == id).unwrap_or(false) {
            *guard = None;
        }
    }
    Ok(())
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

fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect()
}

fn run_pipeline(
    app: &AppHandle,
    running: Arc<AtomicBool>,
    model: Arc<Mutex<String>>,
    notes: Arc<Mutex<String>>,
    dir: PathBuf,
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

            // Tee the audio to disk (raw PCM, appended live) for the recording.
            let _ = meeting::append_pcm(&dir, &f32_to_i16(&resampled));

            match transcriber.transcribe(&resampled) {
                Ok(text) if !text.is_empty() => {
                    transcript_history.push(text.clone());
                    let _ = meeting::append_transcript(&dir, &text);
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
                            let user_notes =
                                notes.lock().map(|n| n.clone()).unwrap_or_default();
                            let app_for_analysis = app.clone();
                            let dir_for_analysis = dir.clone();
                            std::thread::spawn(move || {
                                match analyze::suggest_questions(
                                    &key,
                                    &selected_model,
                                    &context,
                                    &user_notes,
                                ) {
                                    Ok(questions) if !questions.is_empty() => {
                                        let _ = meeting::append_suggestion(
                                            &dir_for_analysis,
                                            &questions,
                                        );
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
    // Load a .env (searches the cwd and parent dirs) so GEMINI_API_KEY can live
    // in a file instead of the shell environment. Ignored if there's no .env.
    let _ = dotenvy::dotenv();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            app.manage(AppState {
                running: Arc::new(AtomicBool::new(false)),
                model: Arc::new(Mutex::new(DEFAULT_MODEL.to_string())),
                notes: Arc::new(Mutex::new(String::new())),
                meeting: Arc::new(Mutex::new(None)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_listening,
            stop_listening,
            set_model,
            set_notes,
            set_title,
            new_meeting,
            list_meetings,
            load_meeting,
            delete_meeting
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
