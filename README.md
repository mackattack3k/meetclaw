# MeetClaw

A desktop meeting assistant. It listens to your microphone, transcribes the
meeting in real time with a local Whisper model, and (later) acts like a quiet
third participant that suggests questions you might want to ask.

This is currently a **walking skeleton**: mic capture → local transcription →
live transcript on screen. No AI analysis yet.

## How it works

```
microphone ──cpal──▶ mono f32 buffer ──resample 16kHz──▶ whisper.cpp ──▶ transcript events ──▶ UI
   (Rust)                  (Rust)             (Rust)        (whisper-rs)       (Tauri)        (TS)
```

- **Audio**: `cpal` captures the default input device, downmixed to mono.
  See `src-tauri/src/audio.rs`.
- **Transcription**: `whisper-rs` (bindings to whisper.cpp) runs the
  `ggml-base.en` model on 5-second windows. See `src-tauri/src/transcribe.rs`.
- **Orchestration**: a worker thread buffers audio, resamples to 16 kHz, runs
  whisper, and emits `transcript` events. See `src-tauri/src/lib.rs`.
- **UI**: vanilla TypeScript listens for events and appends transcript lines.
  See `src/main.ts`.

## Prerequisites

- Rust + Cargo
- Node + npm
- `cmake` (whisper.cpp builds from source) — `brew install cmake`
- The model file at `src-tauri/models/ggml-base.en.bin` (~141 MB).
  Download:
  ```
  curl -L -o src-tauri/models/ggml-base.en.bin \
    https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin
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

## Roadmap

1. **(done)** Skeleton: mic → local transcript.
2. AI layer: feed the rolling transcript to the Claude API and surface
   "questions a third participant might ask."
3. System audio capture for digital meetings (Zoom/Meet/Teams) via
   ScreenCaptureKit or a virtual audio device (BlackHole/Loopback).
4. Camera/whiteboard capture at ~1 Hz into a vision model.
5. Screen capture for digital meetings.
