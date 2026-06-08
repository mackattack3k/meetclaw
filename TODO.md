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
- [x] Anthropic API client in Rust (`reqwest`), key from `ANTHROPIC_API_KEY`
- [x] Rolling transcript buffer; trigger analysis every ~20s
- [x] Prompt: "act as a quiet third participant, suggest questions to ask"
- [x] Structured output (`output_config.format`) for guaranteed-parseable JSON
- [x] Emit `suggestions` events to the UI
- [x] UI panel for suggested questions (separate from transcript)
- [x] Handle missing API key gracefully in the UI (`analysis-disabled` note)
- [ ] Pick final model (default `claude-opus-4-8`; consider `claude-haiku-4-5` for cost/latency)
- [ ] Tune trigger cadence (time vs. number of new segments)

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
