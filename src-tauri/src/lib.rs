mod audio;
mod transcribe;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use audio::{resample, AudioCapture};
use transcribe::Transcriber;

const TARGET_RATE: u32 = 16_000;
// How many seconds of audio to buffer before running a transcription pass.
const CHUNK_SECONDS: f32 = 5.0;

struct AppState {
    running: Arc<AtomicBool>,
}

#[derive(Clone, Serialize)]
struct TranscriptPayload {
    text: String,
}

#[tauri::command]
fn start_listening(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    // If already running, do nothing.
    if state.running.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let running = state.running.clone();

    std::thread::spawn(move || {
        if let Err(e) = run_pipeline(&app, running.clone()) {
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

fn model_path() -> String {
    format!("{}/models/ggml-base.en.bin", env!("CARGO_MANIFEST_DIR"))
}

fn run_pipeline(app: &AppHandle, running: Arc<AtomicBool>) -> Result<(), String> {
    // Load the model first so any error surfaces before we touch the mic.
    let transcriber = Transcriber::new(&model_path())?;

    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let capture = AudioCapture::start(tx)?;
    let native_rate = capture.sample_rate;

    let chunk_native_len = (native_rate as f32 * CHUNK_SECONDS) as usize;
    let mut buffer: Vec<f32> = Vec::with_capacity(chunk_native_len);

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
                    let _ = app.emit("transcript", TranscriptPayload { text });
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
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![start_listening, stop_listening])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
