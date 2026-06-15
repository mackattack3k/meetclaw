mod agent;
mod analyze;
mod audio;
mod camera;
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
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use audio::{resample, AudioCapture};
use transcribe::Transcriber;

const TARGET_RATE: u32 = 16_000;
// Silence-based chunking: cut audio at natural pauses, not on a fixed clock, so
// words aren't sliced across chunk boundaries.
const MIN_CHUNK_SECONDS: f32 = 1.0; // don't transcribe sub-second fragments
const MAX_CHUNK_SECONDS: f32 = 12.0; // force a cut if someone talks without pausing
const SILENCE_HANG_SECONDS: f32 = 0.4; // how much quiet marks the end of a phrase
// Adaptive silence: "silence" is relative to the recent loudest level, so quiet
// (e.g. low-level system audio) still registers as speech. Falls back to an
// absolute floor when nothing loud has been seen yet.
const SILENCE_PEAK_DECAY: f32 = 0.995; // per audio block
const SILENCE_REL_FRACTION: f32 = 0.12; // below 12% of recent peak = silence
const SILENCE_ABS_FLOOR: f32 = 0.0015; // never call anything above this silence-floor noise
// A chunk is only transcribed if its loudest moment reaches this fraction of the
// loudest speech heard so far — otherwise it's silence/noise and is skipped.
const SPEECH_MIN_FRACTION: f32 = 0.12;
// Whisper struggles on very quiet audio (mis-detects language, hallucinates), so
// boost quiet chunks toward this peak before transcription.
const ASR_TARGET_PEAK: f32 = 0.3;
const ASR_MAX_GAIN: f32 = 12.0;
// Live input-level meter: emit the recent loudness to the UI at this cadence,
// reporting the loudest moment since the last emit (so transients aren't missed).
const LEVEL_EMIT_MS: u64 = 66; // ~15 Hz
const LEVEL_FLOOR_DB: f32 = -60.0; // anything quieter reads as the meter's floor
// Finalize normalizes the whole file per window rather than with one global gain:
// a single loud moment would otherwise pin the peak and leave the rest inaudible.
const ASR_NORM_WINDOW_SAMPLES: usize = 16_000 * 5; // 5s windows at 16 kHz

// Gemini analysis (Phase 2).
// Default model; the user can change it live from the UI dropdown.
const DEFAULT_MODEL: &str = "gemini-3.5-flash";
// Transcription language: "auto" (detect) or a 2-letter code like "en".
const DEFAULT_LANGUAGE: &str = "auto";
// Auto-detect hysteresis: ignore detections below this confidence, and require a
// new language to persist this many chunks before switching (rejects one-off
// mis-detections while still following genuine language switches mid-meeting).
const LANG_MIN_CONFIDENCE: f32 = 0.5;
const LANG_SWITCH_CHUNKS: usize = 2;
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
fn camera_path() -> String {
    format!("{}/binaries/meetclaw-camera", env!("CARGO_MANIFEST_DIR"))
}
// Analysis cadence. Suggestions fire when EITHER the timer elapses (so a slow,
// sparse conversation still gets refreshed) OR enough new transcript segments
// have accumulated (so dense back-and-forth gets suggestions sooner) — but never
// closer together than the minimum-gap floor, which keeps a rapid burst of short
// segments from spamming the API.
const ANALYSIS_INTERVAL_SECONDS: u64 = 20; // time fallback / max gap
const ANALYSIS_SEGMENT_THRESHOLD: usize = 4; // new segments that trigger early
const ANALYSIS_MIN_GAP_SECONDS: u64 = 8; // never fire more often than this
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
    // Whether camera capture (~1 Hz) is enabled.
    camera: Arc<AtomicBool>,
    // The running camera helper, when capture is active.
    camera_capture: Arc<Mutex<Option<camera::CameraCapture>>>,
    // Agent: the active run (if any), persisted allow-rules, and auto mode.
    agent_handle: Arc<Mutex<Option<agent::AgentHandle>>>,
    agent_rules: Arc<Mutex<Vec<String>>>,
    agent_auto: Arc<AtomicBool>,
}

/// Where the latest camera frame is written (app cache); the preview reads it.
fn preview_frame_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_cache_dir()
        .map(|d| d.join("camera-frame.jpg"))
        .map_err(|e| format!("no cache dir: {e}"))
}

/// Start the camera helper (writing to the preview frame path) and hold it in
/// state. No-op if it's already running. Errors surface as a `camera-status`.
fn start_camera(app: &AppHandle, state: &AppState) {
    // Hold the lock across the whole start so two racing callers (rapid toggles,
    // or set_camera vs. start_listening) can't both spawn a helper.
    let mut guard = match state.camera_capture.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_some() {
        return;
    }
    let report = |e: String| {
        let _ = app.emit("camera-status", format!("Camera unavailable: {e}"));
    };
    let path = match preview_frame_path(app) {
        Ok(p) => p,
        Err(e) => return report(e),
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return report(format!("create cache dir: {e}"));
        }
    }
    match camera::CameraCapture::start(&camera_path(), path, app.clone()) {
        Ok(cap) => *guard = Some(cap),
        Err(e) => report(e),
    }
}

/// Stop the camera helper (dropping the guard kills the process).
fn stop_camera(state: &AppState) {
    if let Ok(mut guard) = state.camera_capture.lock() {
        *guard = None;
    }
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

#[derive(Clone, Serialize)]
struct LevelPayload {
    db: f32, // input loudness in dBFS, clamped to [LEVEL_FLOOR_DB, 0]
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
    let language_for_final = state.language.clone();
    let notes = state.notes.clone();
    let dir = current.dir;
    let device = state.device.lock().ok().and_then(|d| d.clone());
    let language = state.language.clone();
    let source = state
        .audio_source
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| DEFAULT_AUDIO_SOURCE.to_string());
    // Ensure capture is running if the camera is enabled (covers a persisted-on
    // toggle not re-clicked this session). Capture is independent of recording,
    // so it isn't stopped when recording ends.
    if state.camera.load(Ordering::SeqCst) {
        start_camera(&app, &state);
    }

    std::thread::spawn(move || {
        let outcome = match run_pipeline(
            &app,
            running.clone(),
            model,
            notes,
            dir.clone(),
            device,
            language,
            source,
        ) {
            Ok(o) => o,
            Err(e) => {
                let _ = app.emit("transcribe-error", e);
                PipelineOutcome::default()
            }
        };
        running.store(false, Ordering::SeqCst);
        // Write the WAV from the accumulated PCM now that recording has stopped.
        let _ = meeting::finalize_wav(&dir);
        let _ = app.emit("listening-stopped", ());
        // High-quality whole-file re-transcription replaces the live transcript.
        finalize_transcript(
            &app,
            &dir,
            &language_for_final,
            outcome.established,
            outcome.language_spans,
        );
        // Auto-name the meeting from the (now refined) transcript if still untitled.
        maybe_generate_title(&app, &dir, &model_for_title);
    });

    Ok(())
}

/// Two-tier transcript: after recording stops, re-transcribe the whole audio in
/// one pass (full context) and replace the live chunked transcript with it.
fn finalize_transcript(
    app: &AppHandle,
    dir: &std::path::Path,
    language: &Arc<Mutex<String>>,
    established: Option<String>,
    language_spans: Vec<(usize, String)>,
) {
    let samples = meeting::read_wav_samples(dir);
    if samples.is_empty() {
        return;
    }
    let samples = normalize_for_asr(&samples);
    let _ = app.emit("transcript-finalizing", ());

    let transcriber = match Transcriber::new(&model_path(app)) {
        Ok(t) => t,
        Err(_) => return,
    };
    let setting = language
        .lock()
        .map(|l| l.clone())
        .unwrap_or_else(|_| DEFAULT_LANGUAGE.to_string());

    // A forced language transcribes the whole file in one pass. On "auto" we
    // split by language run so a bilingual meeting's minority-language stretches
    // aren't forced into the majority language (#6).
    let text = if setting == "auto" {
        finalize_multilingual(&transcriber, &samples, &language_spans, established)
    } else {
        transcriber
            .transcribe(&samples, &setting, "")
            .map(|r| r.text.trim().to_string())
            .unwrap_or_default()
    };

    // Guard against a collapsed pass wiping a good live transcript: refuse to
    // replace if the result is drastically shorter (< 25%) than the live tier's.
    let live_len = meeting::read_transcript(dir).trim().len();
    if !text.is_empty() && text.len() * 4 >= live_len {
        let _ = meeting::overwrite_transcript(dir, &text);
        let _ = app.emit("transcript-finalized", text);
    }
}

/// Final-pass transcription for "auto" language. Merges the live tier's
/// per-chunk language detections into contiguous same-language runs and
/// transcribes each run over its own audio slice, then joins the results. When
/// only one language was seen (the common, monolingual case) this collapses to a
/// single whole-file pass — identical to the previous behavior, no regression.
fn finalize_multilingual(
    transcriber: &Transcriber,
    samples: &[f32],
    language_spans: &[(usize, String)],
    established: Option<String>,
) -> String {
    // Collapse consecutive same-language detections into runs (start, language).
    let mut runs: Vec<(usize, String)> = Vec::new();
    for (start, lang) in language_spans {
        if runs.last().map(|(_, l)| l == lang).unwrap_or(false) {
            continue;
        }
        runs.push((*start, lang.clone()));
    }

    // Zero or one language: one pass with full context, preferring the run's
    // language, then the live tier's settled language, then whole-file detect.
    if runs.len() <= 1 {
        let lang = runs
            .into_iter()
            .next()
            .map(|(_, l)| l)
            .or(established)
            .unwrap_or_else(|| "auto".to_string());
        return transcriber
            .transcribe(samples, &lang, "")
            .map(|r| r.text.trim().to_string())
            .unwrap_or_default();
    }

    // Multiple languages: transcribe each run over [run.start, next.start).
    // The first run starts at 0 so any leading audio is included.
    let mut parts: Vec<String> = Vec::new();
    for i in 0..runs.len() {
        let start = if i == 0 { 0 } else { runs[i].0 };
        let end = if i + 1 < runs.len() {
            runs[i + 1].0
        } else {
            samples.len()
        }
        .min(samples.len());
        if start >= end {
            continue;
        }
        if let Ok(r) = transcriber.transcribe(&samples[start..end], &runs[i].1, "") {
            let t = r.text.trim().to_string();
            if !t.is_empty() {
                parts.push(t);
            }
        }
    }
    parts.join("\n")
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

/// Flush the active meeting to disk when the app is exiting: stop recording,
/// persist the latest notes, and write audio.wav from the PCM captured so far.
/// This handles clean quits; crashes/hard kills are covered by
/// `meeting::recover_unfinalized` at the next startup. The slow whole-file
/// transcript pass is intentionally skipped here so quitting stays instant (the
/// live transcript is already on disk).
fn save_on_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.running.store(false, Ordering::SeqCst);
    let dir = state
        .meeting
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|m| m.dir.clone()));
    if let Some(dir) = dir {
        if let Ok(notes) = state.notes.lock() {
            let _ = meeting::write_notes(&dir, &notes);
        }
        let _ = meeting::finalize_wav(&dir);
    }
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

#[tauri::command]
fn set_camera(app: AppHandle, enabled: bool, state: State<AppState>) {
    state.camera.store(enabled, Ordering::SeqCst);
    let _ = settings::update(&app, |s| s.camera = Some(enabled));
    // Start/stop capture immediately so the preview works without recording.
    if enabled {
        start_camera(&app, &state);
    } else {
        stop_camera(&state);
    }
}

/// Read the latest camera frame (JPEG) for the live preview.
#[tauri::command]
fn read_current_frame(app: AppHandle) -> Result<tauri::ipc::Response, String> {
    let bytes = std::fs::read(preview_frame_path(&app)?).map_err(|e| format!("no frame: {e}"))?;
    Ok(tauri::ipc::Response::new(bytes))
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
    camera: bool,
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
        camera: state.camera.load(Ordering::SeqCst),
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
fn set_notes(app: AppHandle, notes: String, state: State<AppState>) -> Result<(), String> {
    if let Ok(mut current) = state.notes.lock() {
        *current = notes.clone();
    }
    // Don't spin up a meeting folder just to store empty notes (e.g. the user
    // typed then cleared the pane before anything else exists).
    let has_meeting = state.meeting.lock().map(|g| g.is_some()).unwrap_or(false);
    if notes.trim().is_empty() && !has_meeting {
        return Ok(());
    }
    // Ensure a meeting exists so notes are always written to disk and survive a
    // restart — mirrors set_title, which already does this.
    let current = ensure_meeting(&app, &state)?;
    meeting::write_notes(&current.dir, &notes)
}

/// Open the detached settings window, or focus it if it's already open.
/// Shared by the gear button (via the `open_settings` command) and the Cmd+,
/// menu item.
fn open_settings_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(win) = app.get_webview_window("settings") {
        win.set_focus()?;
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Settings")
        .inner_size(520.0, 660.0)
        .min_inner_size(420.0, 420.0)
        .resizable(true)
        .build()?;
    Ok(())
}

#[tauri::command]
fn open_settings(app: AppHandle) -> Result<(), String> {
    open_settings_window(&app).map_err(|e| e.to_string())
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
fn read_meeting_audio(app: AppHandle, id: String) -> Result<tauri::ipc::Response, String> {
    let dir = meeting::meeting_dir(&app, &id)?;
    let bytes = std::fs::read(dir.join("audio.wav"))
        .map_err(|e| format!("no audio for this meeting: {e}"))?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
fn export_meeting(app: AppHandle, id: String, dest: String) -> Result<(), String> {
    meeting::export_zip(&app, &id, std::path::Path::new(&dest))
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

// --- Agent (issue #34) ---

/// Start an agent run from a manual request. Replaces any in-flight run.
#[tauri::command]
fn agent_ask(app: AppHandle, text: String, state: State<AppState>) -> Result<(), String> {
    let api_key = analyze::api_key().ok_or("No Gemini API key set (Settings).")?;
    let model = state
        .model
        .lock()
        .map(|m| m.clone())
        .unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    let notes = state.notes.lock().map(|n| n.clone()).unwrap_or_default();
    let transcript = state
        .meeting
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|m| meeting::read_transcript(&m.dir)))
        .unwrap_or_default();
    let agent_md = settings::read_agent_config(&app);
    let workspace = settings::agent_workspace(&app)?;

    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    {
        let mut handle = state
            .agent_handle
            .lock()
            .map_err(|_| "agent lock poisoned".to_string())?;
        if let Some(prev) = handle.take() {
            prev.stop.store(true, Ordering::SeqCst); // cancel any prior run
        }
        *handle = Some(agent::AgentHandle {
            decision_tx: tx,
            stop: stop.clone(),
        });
    }

    let args = agent::RunArgs {
        api_key,
        model,
        ask: text,
        transcript,
        notes,
        agent_md,
        workspace,
        rules: state.agent_rules.clone(),
        auto: state.agent_auto.clone(),
    };
    let app_for_agent = app.clone();
    std::thread::spawn(move || agent::run_agent(app_for_agent, args, rx, stop));
    Ok(())
}

/// Forward the user's verdict on a pending proposal. `decision` is one of
/// "allow_once" | "allow_always" | "deny"; for "allow_always", `scope` is the
/// allow-rule to persist (e.g. "web_search" or "run_command:gh").
#[tauri::command]
fn agent_decision(
    app: AppHandle,
    decision: String,
    scope: Option<String>,
    state: State<AppState>,
) -> Result<(), String> {
    let verdict = match decision.as_str() {
        "allow_once" => agent::Decision::AllowOnce,
        "allow_always" => {
            if let Some(rule) = scope.filter(|s| !s.trim().is_empty()) {
                add_allow_rule(&app, &state, rule)?;
            }
            agent::Decision::AllowAlways
        }
        "deny" => agent::Decision::Deny,
        other => return Err(format!("unknown decision: {other}")),
    };
    let handle = state
        .agent_handle
        .lock()
        .map_err(|_| "agent lock poisoned".to_string())?;
    if let Some(h) = handle.as_ref() {
        h.decision_tx
            .send(verdict)
            .map_err(|_| "agent run already ended".to_string())?;
    }
    Ok(())
}

/// Append an allow-rule to the live set and persist it.
fn add_allow_rule(app: &AppHandle, state: &State<AppState>, rule: String) -> Result<(), String> {
    if let Ok(mut rules) = state.agent_rules.lock() {
        if !rules.contains(&rule) {
            rules.push(rule);
        }
        let snapshot = rules.clone();
        settings::update(app, |s| s.agent_allow_rules = Some(snapshot))?;
    }
    Ok(())
}

#[tauri::command]
fn agent_stop(state: State<AppState>) -> Result<(), String> {
    if let Ok(handle) = state.agent_handle.lock() {
        if let Some(h) = handle.as_ref() {
            h.stop.store(true, Ordering::SeqCst);
        }
    }
    Ok(())
}

#[tauri::command]
fn agent_set_auto(app: AppHandle, enabled: bool, state: State<AppState>) -> Result<(), String> {
    state.agent_auto.store(enabled, Ordering::SeqCst);
    settings::update(&app, |s| s.agent_auto = Some(enabled))
}

#[tauri::command]
fn get_agent_settings(app: AppHandle, state: State<AppState>) -> AgentSettingsView {
    AgentSettingsView {
        config: settings::read_agent_config(&app),
        allow_rules: state
            .agent_rules
            .lock()
            .map(|r| r.clone())
            .unwrap_or_default(),
        auto: state.agent_auto.load(Ordering::SeqCst),
        workspace: settings::agent_workspace(&app)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

#[tauri::command]
fn set_agent_config(app: AppHandle, content: String) -> Result<(), String> {
    settings::write_agent_config(&app, &content)
}

#[tauri::command]
fn set_allow_rules(
    app: AppHandle,
    rules: Vec<String>,
    state: State<AppState>,
) -> Result<(), String> {
    if let Ok(mut current) = state.agent_rules.lock() {
        *current = rules.clone();
    }
    settings::update(&app, |s| s.agent_allow_rules = Some(rules))
}

/// Set (or clear, with `None`) the directory the agent runs commands in.
#[tauri::command]
fn set_agent_workspace(app: AppHandle, path: Option<String>) -> Result<(), String> {
    settings::update(&app, |s| s.agent_workspace = path.filter(|p| !p.trim().is_empty()))
}

#[derive(Serialize)]
struct AgentSettingsView {
    config: String,
    allow_rules: Vec<String>,
    auto: bool,
    workspace: String,
}

// Multilingual base model (supports auto-detect + ~99 languages).
const MODEL_REL_PATH: &str = "models/ggml-base.bin";

/// Locate the whisper model. In a packaged app it's bundled into the Tauri
/// resource dir; in `cargo`/dev builds that resource isn't staged, so fall back
/// to the copy checked into the source tree.
fn model_path(app: &AppHandle) -> String {
    if let Ok(p) = app.path().resolve(MODEL_REL_PATH, BaseDirectory::Resource) {
        if p.exists() {
            return p.to_string_lossy().into_owned();
        }
    }
    format!("{}/{}", env!("CARGO_MANIFEST_DIR"), MODEL_REL_PATH)
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

/// Boost quiet audio toward a target peak (only amplifies, never attenuates) so
/// whisper detects/transcribes it reliably. Near-silence is left untouched.
fn normalize_for_asr(samples: &[f32]) -> Vec<f32> {
    // Normalize per window instead of with a single global gain. On a meeting
    // with high dynamic range (one loud moment, long quiet stretches) a global
    // peak gain clamps to ~1.0 and the quiet speech stays inaudible — which is
    // how a 59-minute recording finalized to an empty transcript. Each window
    // is boosted toward the target peak independently; near-silent windows are
    // left untouched so we don't amplify background noise.
    let mut out = Vec::with_capacity(samples.len());
    for window in samples.chunks(ASR_NORM_WINDOW_SAMPLES) {
        let peak = window.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        if peak < 0.005 {
            out.extend_from_slice(window); // near-silence: don't amplify noise
            continue;
        }
        let gain = (ASR_TARGET_PEAK / peak).clamp(1.0, ASR_MAX_GAIN);
        if gain <= 1.001 {
            out.extend_from_slice(window);
        } else {
            out.extend(window.iter().map(|&s| (s * gain).clamp(-1.0, 1.0)));
        }
    }
    out
}

/// What `run_pipeline` hands back to the finalizer once recording stops.
#[derive(Default)]
struct PipelineOutcome {
    /// Language the live tier confidently settled on, if any (whole-file fallback).
    established: Option<String>,
    /// Per-chunk language detections: (sample offset in the recorded 16 kHz
    /// audio, language code). Drives per-language splitting in the final pass.
    language_spans: Vec<(usize, String)>,
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
) -> Result<PipelineOutcome, String> {
    // Load the model first so any error surfaces before we touch the mic.
    let transcriber = Transcriber::new(&model_path(app))?;

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
    let mut peak_level: f32 = 0.0;
    let mut speech_ceiling: f32 = 0.0; // loudest speech heard so far (no decay)
    let mut chunk_peak: f32 = 0.0; // loudest moment in the current chunk

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
    let mut segments_since_analysis: usize = 0;
    let mut last_lang: Option<String> = None;
    let mut last_emitted: Option<String> = None;
    // Auto-detect hysteresis state (only used when language = "auto").
    let mut cur_lang: Option<(String, String)> = None; // (code, full name)
    let mut pending_lang: Option<String> = None;
    let mut pending_count: usize = 0;

    let _ = app.emit("listening-started", ());

    // Live input-level meter state: throttle emits and report the loudest block
    // seen since the last emit.
    let mut last_level_emit = Instant::now();
    let mut level_accum: f32 = 0.0;

    // Language timeline: (sample offset in the recorded 16 kHz audio -> detected
    // language) so the final pass can re-transcribe each language run on its own
    // (#6). `samples_written` tracks the running length of audio.pcm/.wav.
    let mut language_spans: Vec<(usize, String)> = Vec::new();
    let mut samples_written: usize = 0;

    // Process one finished chunk: always record its audio; if it contained
    // speech, transcribe it and periodically ask for suggestions.
    let mut process_chunk = |chunk: Vec<f32>, speech: bool| {
        let resampled = resample(&chunk, native_rate, TARGET_RATE);
        // Record the original audio; transcribe a level-boosted copy.
        let chunk_start = samples_written;
        let _ = meeting::append_pcm(&dir, &f32_to_i16(&resampled));
        samples_written += resampled.len();
        if !speech {
            return; // silence: recorded, but nothing to transcribe
        }
        let resampled = normalize_for_asr(&resampled);
        let user_setting = language
            .lock()
            .map(|l| l.clone())
            .unwrap_or_else(|_| DEFAULT_LANGUAGE.to_string());

        // Decide the language for this chunk. Explicit setting wins; "auto" uses
        // confidence + hysteresis so a one-off mis-detect can't flip us, but a
        // sustained language switch does.
        let chosen = if user_setting != "auto" {
            cur_lang = None;
            pending_lang = None;
            pending_count = 0;
            user_setting.clone()
        } else {
            if let Some((code, full, conf)) = transcriber.detect_language(&resampled) {
                if conf >= LANG_MIN_CONFIDENCE {
                    match &cur_lang {
                        None => cur_lang = Some((code, full)),
                        Some((cur_code, _)) if *cur_code == code => {
                            pending_lang = None;
                            pending_count = 0;
                        }
                        Some(_) => {
                            if pending_lang.as_deref() == Some(code.as_str()) {
                                pending_count += 1;
                            } else {
                                pending_lang = Some(code.clone());
                                pending_count = 1;
                            }
                            if pending_count >= LANG_SWITCH_CHUNKS {
                                cur_lang = Some((code, full));
                                pending_lang = None;
                                pending_count = 0;
                            }
                        }
                    }
                }
            }
            cur_lang
                .as_ref()
                .map(|(c, _)| c.clone())
                .unwrap_or_else(|| "auto".to_string())
        };

        // Record this chunk's language at its offset so the final pass can split
        // a bilingual meeting by language run (#6).
        language_spans.push((chunk_start, chosen.clone()));

        // No initial_prompt here: feeding prior text back makes whisper loop on
        // hallucinations with quiet/ambiguous audio.
        match transcriber.transcribe(&resampled, &chosen, "") {
            Ok(t) if !t.text.is_empty() => {
                // Surface the active language when it changes.
                let display = if user_setting == "auto" {
                    cur_lang.as_ref().map(|(_, full)| full.clone())
                } else {
                    t.language.clone()
                };
                if let Some(name) = display {
                    if last_lang.as_deref() != Some(name.as_str()) {
                        last_lang = Some(name.clone());
                        let _ = app.emit("language-detected", name);
                    }
                }
                let text = t.text;
                // Drop a line that just repeats the previous one (hallucination loop).
                if last_emitted.as_deref() == Some(text.as_str()) {
                    return;
                }
                last_emitted = Some(text.clone());
                transcript_history.push(text.clone());
                segments_since_analysis += 1;
                let _ = meeting::append_transcript(&dir, &text);
                let _ = app.emit("transcript", TranscriptPayload { text });

                // Ask Gemini for question suggestions when the timer is due, or
                // early once enough new segments have piled up — but never sooner
                // than the minimum-gap floor (the timer fallback is always past
                // the floor, so only the segment path is gated by it).
                if let Some(key) = &api_key {
                    let elapsed = last_analysis.elapsed();
                    let timer_due = elapsed >= Duration::from_secs(ANALYSIS_INTERVAL_SECONDS);
                    let segments_due = segments_since_analysis >= ANALYSIS_SEGMENT_THRESHOLD
                        && elapsed >= Duration::from_secs(ANALYSIS_MIN_GAP_SECONDS);
                    if timer_due || segments_due {
                        last_analysis = Instant::now();
                        segments_since_analysis = 0;
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
                // Adaptive silence: threshold relative to the recent loudest level.
                let level = rms(&samples);
                // Feed the live input-level meter (loudest block since last emit).
                level_accum = level_accum.max(level);
                if last_level_emit.elapsed() >= Duration::from_millis(LEVEL_EMIT_MS) {
                    last_level_emit = Instant::now();
                    let db = if level_accum > 0.0 {
                        20.0 * level_accum.log10()
                    } else {
                        LEVEL_FLOOR_DB
                    };
                    let _ = app.emit(
                        "input-level",
                        LevelPayload {
                            db: db.clamp(LEVEL_FLOOR_DB, 0.0),
                        },
                    );
                    level_accum = 0.0;
                }
                peak_level = (peak_level * SILENCE_PEAK_DECAY).max(level);
                speech_ceiling = speech_ceiling.max(level);
                chunk_peak = chunk_peak.max(level);
                let threshold = (peak_level * SILENCE_REL_FRACTION).max(SILENCE_ABS_FLOOR);
                if level < threshold {
                    silence_run += samples.len();
                } else {
                    silence_run = 0;
                }
                buffer.extend_from_slice(&samples);

                // Cut at a natural pause, or force a cut if the chunk is too long.
                let long_enough = buffer.len() >= min_samples;
                let at_pause = silence_run >= silence_hang_samples;
                let too_long = buffer.len() >= max_samples;
                if long_enough && (at_pause || too_long) {
                    let chunk = std::mem::take(&mut buffer);
                    // Only transcribe if the chunk has real speech, not noise.
                    let speech =
                        chunk_peak >= (speech_ceiling * SPEECH_MIN_FRACTION).max(SILENCE_ABS_FLOOR);
                    silence_run = 0;
                    chunk_peak = 0.0;
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
        let speech = chunk_peak >= (speech_ceiling * SPEECH_MIN_FRACTION).max(SILENCE_ABS_FLOOR);
        process_chunk(chunk, speech);
    }

    // Release the closure's borrows so we can read the established language.
    drop(process_chunk);
    // Dropping the capture stops the audio stream.
    drop(capture);
    Ok(PipelineOutcome {
        established: cur_lang.map(|(code, _)| code),
        language_spans,
    })
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
            // Rebuild WAVs for any meeting that didn't finalize (crash / hard
            // kill / quit mid-recording). Off the main thread so startup is
            // never blocked by large recordings.
            let recover_handle = app.handle().clone();
            std::thread::spawn(move || meeting::recover_unfinalized(&recover_handle));

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
                camera: Arc::new(AtomicBool::new(saved.camera.unwrap_or(false))),
                camera_capture: Arc::new(Mutex::new(None)),
                agent_handle: Arc::new(Mutex::new(None)),
                agent_rules: Arc::new(Mutex::new(settings::agent_allow_rules(app.handle()))),
                agent_auto: Arc::new(AtomicBool::new(saved.agent_auto.unwrap_or(false))),
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
                // The menu closure can't return a Result, so log on failure.
                if let Err(e) = open_settings_window(app) {
                    eprintln!("failed to open settings window: {e}");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            start_listening,
            stop_listening,
            set_model,
            set_notes,
            open_settings,
            set_title,
            new_meeting,
            list_meetings,
            load_meeting,
            delete_meeting,
            export_meeting,
            read_meeting_audio,
            list_devices,
            set_device,
            get_settings,
            set_save_dir,
            set_api_key,
            set_language,
            set_audio_source,
            set_camera,
            read_current_frame,
            agent_ask,
            agent_decision,
            agent_stop,
            agent_set_auto,
            get_agent_settings,
            set_agent_config,
            set_allow_rules,
            set_agent_workspace
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Persist the active meeting before the app exits.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                save_on_exit(app_handle);
            }
        });
}
