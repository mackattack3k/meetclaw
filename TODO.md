# MeetClaw — TODO

## Phase 1 — Walking skeleton (done)
- [x] Scaffold Tauri v2 app (vanilla TS)
- [x] Mic capture via `cpal`, mono downmix
- [x] Resample to 16 kHz
- [x] Local transcription via `whisper-rs` (ggml-base.en)
- [x] Worker thread pipeline + `transcript` events
- [x] Start/Stop UI with live transcript pane
- [x] macOS mic permission (`Info.plist`)

## Phase 2 — AI question suggestions (done)
- [x] Gemini API client in Rust (`reqwest`), key from `GEMINI_API_KEY`
- [x] Rolling transcript buffer; trigger analysis every ~20s
- [x] Prompt: "act as a quiet third participant, suggest questions to ask"
- [x] Structured output (`responseSchema`) for parseable JSON
- [x] Emit `suggestions` events to the UI
- [x] UI panel for suggested questions (separate from transcript)
- [x] Handle missing API key gracefully in the UI (`analysis-disabled` note)
- [x] Model selector dropdown (Gemini 3.5 Flash / 3.1 Flash-Lite / 2.5 Pro), live-switchable
- [ ] Tune trigger cadence (time vs. number of new segments)
- [ ] In-app credential entry + macOS Keychain (so no env var)
- [ ] Show active model in the UI

### Provider history
- Started on Claude (Anthropic API key, then Google Vertex AI via `gcp_auth`),
  then switched to the Gemini Developer API key (simplest auth, native to GCP).
- Claude.ai Pro/Max OAuth was ruled out — restricted to Claude Code, rejected by
  the Messages API (Anthropic policy, Feb 2026).

## Phase 3 — Microphone & device selection
- [ ] List available input devices (`cpal` enumerate)
- [ ] Tauri command to return device list to the UI
- [ ] Device picker dropdown in the UI
- [ ] Pass selected device into the capture pipeline
- [ ] Persist last-used device

## Phase 4 — System audio (digital meetings)
- [ ] Capture other participants' audio (Zoom/Meet/Teams)
- [ ] Option A: ScreenCaptureKit (macOS 13+) — Swift/ObjC bridge from Rust
- [ ] Option B: virtual audio device (BlackHole/Loopback) — user installs, read as input
- [ ] Mix/label mic vs system audio in the transcript

## Phase 5 — Vision (whiteboard / screen)
- [ ] Camera capture at ~1 Hz
- [ ] Send frames to a vision model (whiteboard reading)
- [ ] Screen capture for digital meetings
- [ ] Fold visual context into the suggestion prompt

## Refinements / tech debt
- [ ] Sliding window with overlap (fix words clipped at 5s chunk boundaries)
- [ ] Bundle the whisper model as a Tauri resource for distributable builds
- [ ] Enable `whisper-rs` `metal` feature for GPU inference (speed)
- [ ] Fall back to `tiny.en` if `base.en` can't keep up
- [ ] Persist/export transcript + suggestions per meeting
