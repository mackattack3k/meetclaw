# MeetClaw

A desktop meeting assistant. It listens to your microphone, transcribes the
meeting in real time with a local Whisper model, and acts like a quiet third
participant that suggests questions you might want to ask (via the Gemini API).

## How it works

```
microphone ──cpal──▶ mono f32 buffer ──resample 16kHz──▶ whisper.cpp ──▶ transcript events ──▶ UI
   (Rust)                  (Rust)             (Rust)        (whisper-rs)       (Tauri)        (TS)
```

- **Audio**: `cpal` captures the default input device, downmixed to mono.
  See `src-tauri/src/audio.rs`.
- **Transcription**: `whisper-rs` (bindings to whisper.cpp) runs the
  multilingual `ggml-base` model on silence-delimited windows, with auto-detect or
a chosen language. See `src-tauri/src/transcribe.rs`.
- **Orchestration**: a worker thread buffers audio, resamples to 16 kHz, runs
  whisper, and emits `transcript` events. See `src-tauri/src/lib.rs`.
- **UI**: vanilla TypeScript listens for events and appends transcript lines.
  See `src/main.ts`.

## Prerequisites

- Rust + Cargo
- Node + npm
- `cmake` (whisper.cpp builds from source) — `brew install cmake`
- The model file at `src-tauri/models/ggml-base.bin` (~141 MB).
  Download:
  ```
  curl -L -o src-tauri/models/ggml-base.bin \
    https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin
  ```

## Run

```
npm install
npm run tauri dev
```

macOS will ask for microphone permission the first time you press
**Start listening**.

## Known limitations (skeleton stage)

- Transcription runs on non-overlapping 5-second chunks, so words on a chunk
  boundary can be clipped. A sliding window with overlap is a later refinement.
- Model path is resolved relative to the crate at dev time. Bundling the model
  as a Tauri resource is needed for a distributable build.
- CPU inference. The `metal` feature of `whisper-rs` would speed this up.

## AI question suggestions (Phase 2)

While listening, MeetClaw sends the rolling transcript to the Gemini API every
~20 seconds and shows up to 3 questions a sharp third participant might ask, in
the right-hand panel. See `src-tauri/src/analyze.rs`.

Auth is a plain API key. Get one from
[Google AI Studio](https://ai.google.dev/gemini-api/docs/api-key), then:

```
export GEMINI_API_KEY=...
npm run tauri dev
```

If the key isn't set, transcription still works and the suggestions panel shows
a "disabled" note. The model is chosen from the dropdown in the app
(Gemini 3.5 Flash / 3.1 Flash-Lite / 2.5 Pro), live-switchable.

## System audio (digital meetings)

To transcribe the *other participants* in a Zoom/Meet/Teams call (not just your
mic), set **Audio source → System audio** in Settings. This uses a small
ScreenCaptureKit helper (`src-tauri/syscap/main.swift`) that streams system
audio to the app.

Build the helper once (needs Xcode command-line tools):

```
src-tauri/syscap/build.sh
```

It requires the **Screen Recording** permission (System Settings › Privacy &
Security › Screen Recording) — grant it to MeetClaw and restart. Until then the
app shows a permission note when you start with System audio selected.

> Note: system audio captures the remote participants only (what's played out),
> not your own mic. Mixing both into one recording is a planned improvement.

## Meeting library

Every meeting is saved continuously to a folder under the app data dir
(`~/Library/Application Support/<bundle id>/meetings/<id>/`):

- `meeting.json` — title + timestamps
- `transcript.txt`, `notes.md`, `suggestions.jsonl`
- `audio.pcm` (16 kHz mono, appended live) → `audio.wav` when you press Stop

Give the meeting a title in the top bar. **Library** lists past meetings;
opening one loads it as the current meeting and lets you **resume recording**
into it. **New** starts a fresh meeting. See `src-tauri/src/meeting.rs`.

## Roadmap

1. **(done)** Skeleton: mic → local transcript.
2. **(done)** AI layer: rolling transcript → Gemini → suggested questions.
3. Microphone / input device selection in the UI.
4. System audio capture for digital meetings (Zoom/Meet/Teams) via
   ScreenCaptureKit or a virtual audio device (BlackHole/Loopback).
5. Camera/whiteboard capture at ~1 Hz into a vision model.
6. Screen capture for digital meetings.
