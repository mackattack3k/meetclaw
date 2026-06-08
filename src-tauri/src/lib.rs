mod analyze;
mod audio;
mod meeting;
mod mixer;
mod settings;
mod syscap;
mod transcribe;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, State};

use audio::{resample, AudioCapture};
use transcribe::Transcriber;

const TARGET_RATE: u32 = 16_000;
// Silence-based chunking: cut audio at natural pauses, not on a fixed clock, so
// words aren't sliced across chunk boundaries.
const MIN_CHUNK_SECONDS: f32 = 1.0; // don't transcribe sub-second fragments
const MAX_CHUNK_SECONDS: f32 = 12.0; // force a cut if someone talks without pausing
const SILENCE_HANG_SECONDS: f32 = 0.4; // how much quiet marks the end of a phrase
const SILENCE_RMS_THRESHOLD: f32 = 0.01; // below this RMS counts as silence

// Gemini analysis (Phase 2).
// Default model; the user can change it live from the UI dropdown.
const DEFAULT_MODEL: &str = "gemini-3.5-flash";
// Transcription language: "auto" (detect) or a 2-letter code like "en".
const DEFAULT_LANGUAGE: &str = "auto";
// Audio source: "mic" or "system" (ScreenCaptureKit, for digital meetings).
const DEFAULT_AUDIO_SOURCE: &str = "mic";

/// Keeps the active capture alive for the duration of a recording (RAII guard;
/// the fields are only held so the cpal stream / helper process stay alive).
#[allow(dead_code)]
enum Capture {
    Mic(audio::AudioCapture),
    System(syscap::SystemAudioCapture),
    Mixed(mixer::MixedCapture),
}

fn syscap_path() -> String {
    format!("{}/binaries/meetclaw-syscap", env!("CARGO_MANIFEST_DIR"))
}
// How often (at most) to ask Claude for fresh question suggestions.
const ANALYSIS_INTERVAL_SECONDS: u64 = 20;
// How much recent transcript (in characters) to send as context.
const MAX_CONTEXT_CHARS: usize = 4000;

struct AppState {
    running: Arc<AtomicBool>,
    model: Arc<Mutex<String>>,
    notes: Arc<Mutex<String>>,
    meeting: Arc<Mutex<Option<CurrentMeeting>>>,
    // Selected input device name; None means the system default.
    device: Arc<Mutex<Option<String>>>,
    // Transcription language ("auto" or a 2-letter code).
    language: Arc<Mutex<String>>,
    // Audio source ("mic" or "system").
    audio_source: Arc<Mutex<String>>,
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
    let device = state.device.lock().ok().and_then(|d| d.clone());
    let language = state.language.clone();
    let source = state
        .audio_source
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| DEFAULT_AUDIO_SOURCE.to_string());

    std::thread::spawn(move || {
        if let Err(e) = run_pipeline(
            &app,
            running.clone(),
            model,
            notes,
            dir.clone(),
            device,
            language,
            source,
        ) {
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
fn set_model(app: AppHandle, model: String, state: State<AppState>) {
    if let Ok(mut current) = state.model.lock() {
        *current = model.clone();
    }
    let _ = settings::update(&app, |s| s.model = Some(model));
}

#[tauri::command]
fn list_devices() -> Vec<String> {
    audio::list_input_devices()
}

#[tauri::command]
fn set_device(app: AppHandle, device: Option<String>, state: State<AppState>) {
    // Treat an empty selection as "use the default device".
    let device = device.filter(|d| !d.is_empty());
    if let Ok(mut current) = state.device.lock() {
        *current = device.clone();
    }
    let _ = settings::update(&app, |s| s.device = device);
}

#[tauri::command]
fn set_language(app: AppHandle, language: String, state: State<AppState>) {
    if let Ok(mut current) = state.language.lock() {
        *current = language.clone();
    }
    let _ = settings::update(&app, |s| s.language = Some(language));
}

#[tauri::command]
fn set_audio_source(app: AppHandle, source: String, state: State<AppState>) {
    if let Ok(mut current) = state.audio_source.lock() {
        *current = source.clone();
    }
    let _ = settings::update(&app, |s| s.audio_source = Some(source));
}

#[derive(serde::Serialize)]
struct SettingsView {
    model: String,
    device: Option<String>,
    save_dir: Option<String>,
    default_save_dir: String,
    has_api_key: bool,
    language: String,
    audio_source: String,
}

#[tauri::command]
fn get_settings(app: AppHandle, state: State<AppState>) -> SettingsView {
    let s = settings::load(&app);
    let model = state
        .model
        .lock()
        .map(|m| m.clone())
        .unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    let device = state.device.lock().ok().and_then(|d| d.clone());
    let language = state
        .language
        .lock()
        .map(|l| l.clone())
        .unwrap_or_else(|_| DEFAULT_LANGUAGE.to_string());
    let audio_source = state
        .audio_source
        .lock()
        .map(|a| a.clone())
        .unwrap_or_else(|_| DEFAULT_AUDIO_SOURCE.to_string());
    let default_save_dir = app
        .path()
        .app_data_dir()
        .map(|p| p.join("meetings").to_string_lossy().to_string())
        .unwrap_or_default();
    SettingsView {
        model,
        device,
        save_dir: s.save_dir,
        default_save_dir,
        has_api_key: settings::has_api_key(),
        language,
        audio_source,
    }
}

#[tauri::command]
fn set_save_dir(app: AppHandle, path: Option<String>) -> Result<(), String> {
    let path = path.filter(|p| !p.trim().is_empty());
    settings::update(&app, |s| s.save_dir = path)
}

#[tauri::command]
fn set_api_key(key: String) -> Result<(), String> {
    settings::set_api_key(&key)
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
    // Multilingual base model (supports auto-detect + ~99 languages).
    format!("{}/models/ggml-base.bin", env!("CARGO_MANIFEST_DIR"))
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

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

fn run_pipeline(
    app: &AppHandle,
    running: Arc<AtomicBool>,
    model: Arc<Mutex<String>>,
    notes: Arc<Mutex<String>>,
    dir: PathBuf,
    device: Option<String>,
    language: Arc<Mutex<String>>,
    source: String,
) -> Result<(), String> {
    // Load the model first so any error surfaces before we touch the mic.
    let transcriber = Transcriber::new(&model_path())?;

    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    // `capture` is held for the duration so the stream/helper stays alive.
    let (capture, native_rate) = match source.as_str() {
        "system" => {
            let cap = syscap::SystemAudioCapture::start(tx, &syscap_path(), app.clone())?;
            (Capture::System(cap), syscap::SAMPLE_RATE)
        }
        "both" => {
            // The mixer emits already-resampled 16 kHz audio.
            let cap =
                mixer::MixedCapture::start(tx, device.as_deref(), &syscap_path(), app.clone())?;
            (Capture::Mixed(cap), TARGET_RATE)
        }
        _ => {
            let cap = AudioCapture::start(tx, device.as_deref())?;
            let rate = cap.sample_rate;
            (Capture::Mic(cap), rate)
        }
    };

    let min_samples = (native_rate as f32 * MIN_CHUNK_SECONDS) as usize;
    let max_samples = (native_rate as f32 * MAX_CHUNK_SECONDS) as usize;
    let silence_hang_samples = (native_rate as f32 * SILENCE_HANG_SECONDS) as usize;
    let mut buffer: Vec<f32> = Vec::new();
    let mut silence_run: usize = 0;
    let mut had_speech = false;

    // Phase 2: rolling transcript + periodic Gemini analysis.
    let api_key = analyze::api_key();
    if api_key.is_none() {
        let _ = app.emit(
            "analysis-disabled",
            "No Gemini API key — question suggestions are off. Add one in Settings.",
        );
    }
    let mut transcript_history: Vec<String> = Vec::new();
    let mut last_analysis = Instant::now();
    let mut last_lang: Option<String> = None;

    let _ = app.emit("listening-started", ());

    // Process one finished chunk: always record its audio; if it contained
    // speech, transcribe it and periodically ask for suggestions.
    let mut process_chunk = |chunk: Vec<f32>, speech: bool| {
        let resampled = resample(&chunk, native_rate, TARGET_RATE);
        let _ = meeting::append_pcm(&dir, &f32_to_i16(&resampled));
        if !speech {
            return; // silence: recorded, but nothing to transcribe
        }
        let lang = language
            .lock()
            .map(|l| l.clone())
            .unwrap_or_else(|_| DEFAULT_LANGUAGE.to_string());
        match transcriber.transcribe(&resampled, &lang) {
            Ok(t) if !t.text.is_empty() => {
                // Surface the detected/used language when it changes.
                if let Some(detected) = &t.language {
                    if last_lang.as_deref() != Some(detected.as_str()) {
                        last_lang = Some(detected.clone());
                        let _ = app.emit("language-detected", detected.clone());
                    }
                }
                let text = t.text;
                transcript_history.push(text.clone());
                let _ = meeting::append_transcript(&dir, &text);
                let _ = app.emit("transcript", TranscriptPayload { text });

                // Periodically ask Gemini for question suggestions. Run the
                // call on its own thread so transcription keeps flowing.
                if let Some(key) = &api_key {
                    if last_analysis.elapsed() >= Duration::from_secs(ANALYSIS_INTERVAL_SECONDS) {
                        last_analysis = Instant::now();
                        let context = recent_context(&transcript_history, MAX_CONTEXT_CHARS);
                        let key = key.clone();
                        let selected_model = model
                            .lock()
                            .map(|m| m.clone())
                            .unwrap_or_else(|_| DEFAULT_MODEL.to_string());
                        let user_notes = notes.lock().map(|n| n.clone()).unwrap_or_default();
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
                                    let _ =
                                        meeting::append_suggestion(&dir_for_analysis, &questions);
                                    let _ = app_for_analysis
                                        .emit("suggestions", SuggestionsPayload { questions });
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    let _ = app_for_analysis.emit("analysis-error", e);
                                }
                            }
                        });
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                let _ = app.emit("transcribe-error", e);
            }
        }
    };

    while running.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(samples) => {
                if rms(&samples) < SILENCE_RMS_THRESHOLD {
                    silence_run += samples.len();
                } else {
                    silence_run = 0;
                    had_speech = true;
                }
                buffer.extend_from_slice(&samples);

                // Cut at a natural pause, or force a cut if the chunk is too long.
                let long_enough = buffer.len() >= min_samples;
                let at_pause = silence_run >= silence_hang_samples;
                let too_long = buffer.len() >= max_samples;
                if long_enough && (at_pause || too_long) {
                    let chunk = std::mem::take(&mut buffer);
                    let speech = had_speech;
                    silence_run = 0;
                    had_speech = false;
                    process_chunk(chunk, speech);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Flush whatever is left so the tail of the meeting isn't lost.
    if !buffer.is_empty() {
        let chunk = std::mem::take(&mut buffer);
        process_chunk(chunk, had_speech);
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
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let saved = settings::load(app.handle());
            app.manage(AppState {
                running: Arc::new(AtomicBool::new(false)),
                model: Arc::new(Mutex::new(
                    saved.model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
                )),
                notes: Arc::new(Mutex::new(String::new())),
                meeting: Arc::new(Mutex::new(None)),
                device: Arc::new(Mutex::new(saved.device)),
                language: Arc::new(Mutex::new(
                    saved.language.unwrap_or_else(|| DEFAULT_LANGUAGE.to_string()),
                )),
                audio_source: Arc::new(Mutex::new(
                    saved
                        .audio_source
                        .unwrap_or_else(|| DEFAULT_AUDIO_SOURCE.to_string()),
                )),
            });

            // Native macOS menu: Settings… bound to Cmd+, under the app menu,
            // plus a standard Edit menu so copy/paste works in the notes field.
            let handle = app.handle();
            let settings_item = MenuItemBuilder::new("Settings…")
                .id("settings")
                .accelerator("CmdOrCtrl+,")
                .build(handle)?;
            let app_menu = SubmenuBuilder::new(handle, "MeetClaw")
                .about(None)
                .separator()
                .item(&settings_item)
                .separator()
                .services()
                .separator()
                .hide()
                .hide_others()
                .show_all()
                .separator()
                .quit()
                .build()?;
            let edit_menu = SubmenuBuilder::new(handle, "Edit")
                .undo()
                .redo()
                .separator()
                .cut()
                .copy()
                .paste()
                .select_all()
                .build()?;
            let window_menu = SubmenuBuilder::new(handle, "Window")
                .minimize()
                .close_window()
                .build()?;
            let menu = MenuBuilder::new(handle)
                .items(&[&app_menu, &edit_menu, &window_menu])
                .build()?;
            app.set_menu(menu)?;

            Ok(())
        })
        .on_menu_event(|app, event| {
            let id: &str = event.id.as_ref();
            if id == "settings" {
                let _ = app.emit("menu:settings", ());
            }
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
            delete_meeting,
            list_devices,
            set_device,
            get_settings,
            set_save_dir,
            set_api_key,
            set_language,
            set_audio_source
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
