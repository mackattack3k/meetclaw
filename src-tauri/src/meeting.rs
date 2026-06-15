// Meeting persistence. Each meeting is a folder under the app data dir:
//
//   <app_data>/meetings/<id>/
//     meeting.json      — { id, title, created_at_ms, updated_at_ms }
//     transcript.txt     — one transcribed line per row (appended live)
//     notes.md           — the user's notes (overwritten on change)
//     suggestions.jsonl  — one JSON array of questions per row (appended live)
//     audio.pcm          — raw 16 kHz mono i16 LE samples (appended live)
//     audio.wav          — a real WAV, written from the PCM when recording stops
//
// Writes are continuous (autosave): callers append as data arrives. PCM is used
// during recording because it is trivially appendable (and resume-safe); the WAV
// is regenerated from it on stop.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

const SAMPLE_RATE: u32 = 16_000;

#[derive(Serialize, Deserialize, Clone)]
pub struct MeetingMeta {
    pub id: String,
    pub title: String,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Serialize)]
pub struct MeetingDetail {
    pub id: String,
    pub title: String,
    pub created_at_ms: u128,
    pub transcript: String,
    pub notes: String,
    pub suggestions: Vec<Vec<String>>,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn meetings_root(app: &AppHandle) -> Result<PathBuf, String> {
    // Use the user-configured save location if set, else the default app data dir.
    let dir = match crate::settings::save_dir(app) {
        Some(custom) => custom,
        None => app
            .path()
            .app_data_dir()
            .map_err(|e| format!("no app data dir: {e}"))?
            .join("meetings"),
    };
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create meetings dir: {e}"))?;
    Ok(dir)
}

pub fn meeting_dir(app: &AppHandle, id: &str) -> Result<PathBuf, String> {
    Ok(meetings_root(app)?.join(id))
}

pub fn create(app: &AppHandle, title: &str) -> Result<MeetingMeta, String> {
    let ms = now_ms();
    let id = format!("m{ms}");
    let dir = meetings_root(app)?.join(&id);
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create meeting dir: {e}"))?;

    let title = if title.trim().is_empty() {
        "Untitled meeting".to_string()
    } else {
        title.to_string()
    };
    let meta = MeetingMeta {
        id,
        title,
        created_at_ms: ms,
        updated_at_ms: ms,
    };
    write_meta(&dir, &meta)?;
    Ok(meta)
}

fn write_meta(dir: &Path, meta: &MeetingMeta) -> Result<(), String> {
    let json = serde_json::to_string_pretty(meta).map_err(|e| format!("serialize meta: {e}"))?;
    fs::write(dir.join("meeting.json"), json).map_err(|e| format!("write meeting.json: {e}"))
}

fn read_meta(dir: &Path) -> Result<MeetingMeta, String> {
    let raw = fs::read_to_string(dir.join("meeting.json"))
        .map_err(|e| format!("read meeting.json: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse meeting.json: {e}"))
}

pub fn get_title(dir: &Path) -> Option<String> {
    read_meta(dir).ok().map(|m| m.title)
}

pub fn read_transcript(dir: &Path) -> String {
    fs::read_to_string(dir.join("transcript.txt")).unwrap_or_default()
}

pub fn read_notes(dir: &Path) -> String {
    fs::read_to_string(dir.join("notes.md")).unwrap_or_default()
}

pub fn set_title(dir: &Path, title: &str) -> Result<(), String> {
    let mut meta = read_meta(dir)?;
    meta.title = title.to_string();
    meta.updated_at_ms = now_ms();
    write_meta(dir, &meta)
}

/// Read the meeting's recorded audio (16 kHz mono i16) back as f32 samples.
pub fn read_wav_samples(dir: &Path) -> Vec<f32> {
    let path = dir.join("audio.wav");
    let Ok(reader) = hound::WavReader::open(path) else {
        return Vec::new();
    };
    reader
        .into_samples::<i16>()
        .filter_map(|s| s.ok())
        .map(|s| s as f32 / 32768.0)
        .collect()
}

/// Replace the transcript wholesale (used by the high-quality final pass).
pub fn overwrite_transcript(dir: &Path, text: &str) -> Result<(), String> {
    fs::write(dir.join("transcript.txt"), text).map_err(|e| format!("write transcript: {e}"))
}

pub fn append_transcript(dir: &Path, line: &str) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("transcript.txt"))
        .map_err(|e| format!("open transcript: {e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("write transcript: {e}"))
}

pub fn write_notes(dir: &Path, notes: &str) -> Result<(), String> {
    fs::write(dir.join("notes.md"), notes).map_err(|e| format!("write notes: {e}"))
}

pub fn append_suggestion(dir: &Path, questions: &[String]) -> Result<(), String> {
    let line = serde_json::to_string(questions).map_err(|e| format!("serialize suggestion: {e}"))?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("suggestions.jsonl"))
        .map_err(|e| format!("open suggestions: {e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("write suggestion: {e}"))
}

pub fn append_pcm(dir: &Path, samples: &[i16]) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("audio.pcm"))
        .map_err(|e| format!("open pcm: {e}"))?;
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    f.write_all(&bytes).map_err(|e| format!("write pcm: {e}"))
}

/// Convert the accumulated PCM into a real WAV file. Called when recording stops.
pub fn finalize_wav(dir: &Path) -> Result<(), String> {
    let pcm_path = dir.join("audio.pcm");
    if !pcm_path.exists() {
        return Ok(());
    }
    let raw = fs::read(&pcm_path).map_err(|e| format!("read pcm: {e}"))?;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(dir.join("audio.wav"), spec).map_err(|e| format!("create wav: {e}"))?;
    for chunk in raw.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        writer
            .write_sample(sample)
            .map_err(|e| format!("write wav sample: {e}"))?;
    }
    writer.finalize().map_err(|e| format!("finalize wav: {e}"))
}

/// Recover meetings that were terminated before `finalize_wav` ran (app crash,
/// hard kill, or quit mid-recording): rebuild `audio.wav` from `audio.pcm`
/// wherever the WAV is missing or shorter than the PCM implies. A finalized WAV
/// is a 44-byte header plus the PCM bytes, so anything materially smaller than
/// `44 + pcm_len` means it was never (fully) written. Safe to run at startup
/// when no pipeline is recording.
pub fn recover_unfinalized(app: &AppHandle) {
    let Ok(root) = meetings_root(app) else {
        return;
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(pcm_meta) = fs::metadata(dir.join("audio.pcm")) else {
            continue; // no PCM to recover from
        };
        let pcm_len = pcm_meta.len();
        if pcm_len == 0 {
            continue;
        }
        // Tolerate the header and a dropped trailing odd byte (chunks_exact).
        let needs_rebuild = match fs::metadata(dir.join("audio.wav")) {
            Ok(wav_meta) => wav_meta.len() + 40 < pcm_len + 44,
            Err(_) => true,
        };
        if needs_rebuild {
            let _ = finalize_wav(&dir);
        }
    }
}

pub fn list(app: &AppHandle) -> Result<Vec<MeetingMeta>, String> {
    let root = meetings_root(app)?;
    let mut metas = Vec::new();
    for entry in fs::read_dir(&root).map_err(|e| format!("read meetings dir: {e}"))? {
        let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
        if entry.path().is_dir() {
            if let Ok(meta) = read_meta(&entry.path()) {
                metas.push(meta);
            }
        }
    }
    // Newest first.
    metas.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    Ok(metas)
}

pub fn load(app: &AppHandle, id: &str) -> Result<MeetingDetail, String> {
    let dir = meeting_dir(app, id)?;
    let meta = read_meta(&dir)?;
    let transcript = fs::read_to_string(dir.join("transcript.txt")).unwrap_or_default();
    let notes = fs::read_to_string(dir.join("notes.md")).unwrap_or_default();

    let mut suggestions = Vec::new();
    if let Ok(file) = File::open(dir.join("suggestions.jsonl")) {
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if let Ok(batch) = serde_json::from_str::<Vec<String>>(&line) {
                suggestions.push(batch);
            }
        }
    }

    Ok(MeetingDetail {
        id: meta.id,
        title: meta.title,
        created_at_ms: meta.created_at_ms,
        transcript,
        notes,
        suggestions,
    })
}

pub fn delete(app: &AppHandle, id: &str) -> Result<(), String> {
    let dir = meeting_dir(app, id)?;
    fs::remove_dir_all(&dir).map_err(|e| format!("delete meeting: {e}"))
}

/// Export a meeting as a zip at `dest`: a formatted Markdown summary (title,
/// notes, transcript, and the suggested questions) plus the recorded audio.
pub fn export_zip(app: &AppHandle, id: &str, dest: &Path) -> Result<(), String> {
    let detail = load(app, id)?;
    let dir = meeting_dir(app, id)?;

    let file = File::create(dest).map_err(|e| format!("create export file: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let md = render_markdown(&detail);
    zip.start_file(format!("{}.md", sanitize_filename(&detail.title)), opts)
        .map_err(|e| format!("zip summary: {e}"))?;
    zip.write_all(md.as_bytes())
        .map_err(|e| format!("write summary: {e}"))?;

    // Include the recorded audio if it exists.
    if let Ok(bytes) = fs::read(dir.join("audio.wav")) {
        zip.start_file("audio.wav", opts)
            .map_err(|e| format!("zip audio: {e}"))?;
        zip.write_all(&bytes).map_err(|e| format!("write audio: {e}"))?;
    }

    zip.finish().map_err(|e| format!("finalize export: {e}"))?;
    Ok(())
}

/// Render a meeting as a shareable Markdown document. Notes, transcript, and the
/// suggested questions all go into one file (#3).
fn render_markdown(d: &MeetingDetail) -> String {
    let section = |body: &str| {
        if body.trim().is_empty() {
            "_(none)_".to_string()
        } else {
            body.trim().to_string()
        }
    };

    let mut questions = String::new();
    let mut seen = std::collections::HashSet::new();
    for batch in &d.suggestions {
        for q in batch {
            if seen.insert(q.as_str()) {
                questions.push_str(&format!("- {q}\n"));
            }
        }
    }
    let questions = if questions.is_empty() {
        "_(none)_".to_string()
    } else {
        questions.trim_end().to_string()
    };

    format!(
        "# {}\n\n## Notes\n\n{}\n\n## Transcript\n\n{}\n\n## Suggested questions\n\n{}\n",
        d.title,
        section(&d.notes),
        section(&d.transcript),
        questions,
    )
}

/// Make a title safe to use as a file name (strip path separators and other
/// awkward characters), falling back to "meeting" when nothing usable is left.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .take(200) // keep room for the ".md" suffix under the 255-byte filename limit
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "meeting".to_string()
    } else {
        trimmed.to_string()
    }
}
