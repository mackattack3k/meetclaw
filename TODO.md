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
- [x] In-app credential entry + macOS Keychain (Settings → Gemini API key)
- [x] Show active model in the UI (chip in the toolbar)
- [x] Improve question quality/relevance — topic-anchored prompt + few-shot
      (RAG-for-Hermes style). Could still: surface the detected topic, send more context.
- [ ] Tune trigger cadence (time vs. number of new segments)

### Provider history
- Started on Claude (Anthropic API key, then Google Vertex AI via `gcp_auth`),
  then switched to the Gemini Developer API key (simplest auth, native to GCP).
- Claude.ai Pro/Max OAuth was ruled out — restricted to Claude Code, rejected by
  the Messages API (Anthropic policy, Feb 2026).

## Phase 2.5 — Personal notes (and as LLM context) (done)
- [x] Editable notes pane in the UI (write your own notes during the meeting)
- [x] Keep notes in app state (in-memory for the session) via `set_notes`
- [x] Feed the user's notes into the Gemini context alongside the transcript
- [x] Tune the prompt to use notes (build on them; don't re-suggest noted items)
- [ ] Persist notes to disk across restarts
- [ ] Include notes in any transcript/suggestions export

## Phase 2.7 — Meeting library (save / resume) (done)
- [x] Per-meeting folder under app data dir (`meeting.json`, `transcript.txt`,
      `notes.md`, `suggestions.jsonl`, `audio.pcm` → `audio.wav` on stop)
- [x] Meeting title field (autosaved)
- [x] Continuous autosave: transcript, notes, audio, suggestions written live
- [x] Audio capture to disk (16 kHz mono; PCM appended live, WAV on stop via `hound`)
- [x] Library modal: list past meetings (title + date), open, delete
- [x] Open a meeting = load it as current and resume recording into it
- [x] Confirm-before-delete (native dialog)
- [x] Configurable save location on disk (Settings → folder picker, persisted)
- [x] Auto-generate a title (Gemini) on stop when still untitled
- [ ] Audio playback for review (file is saved; no player UI yet)
- [ ] Export a meeting (zip / share)

## Phase 2.6 — Language support (done)
- [x] Language selector in Settings (auto-detect + ~20 languages)
- [x] Show the detected/active language as a chip in the toolbar
- [x] Switched to the multilingual `ggml-base` model
- [x] Pass the chosen language to whisper (or "auto"), read the detected
      language back via `full_lang_id_from_state` + `get_lang_str_full`

## Phase 3 — Microphone & device selection (done)
- [x] List available input devices (`cpal` enumerate)
- [x] Tauri command to return device list to the UI
- [x] Device picker dropdown in the UI
- [x] Pass selected device into the capture pipeline
- [x] Persist last-used device across restarts (settings.json)

## Audio capture quality (done)
- [x] Silence-based chunking (cut at pauses, min/max bounds) — fixes words
      sliced across the old fixed 5s boundaries
- [x] Flush the tail on Stop so the last seconds aren't lost
- [x] Accept i16/u16/i32/i8/u8 device formats (not just f32)
- [ ] Better resampler (linear → windowed-sinc) if transcription needs it

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

## Settings page (done)
- [x] Consolidate model, input device, save location, and API key into a
      dedicated settings panel; slim the top bar (Library / New / Settings)
- [x] Self-host fonts (offline, via @fontsource) — no Google Fonts CDN
- [x] Native macOS menu bar: Settings… bound to Cmd+, ; standard Edit menu
      (copy/paste in notes), Window menu; Esc closes overlays
- [ ] Detached native settings *window* (currently a panel opened via Cmd+,)

## Refinements / tech debt
- [x] Fix words clipped at chunk boundaries (done via silence-based chunking)
- [ ] Bundle the whisper model as a Tauri resource for distributable builds
- [ ] Enable `whisper-rs` `metal` feature for GPU inference (speed)
- [ ] Fall back to `tiny.en` if `base.en` can't keep up

## Deliberately deferred (heavier / needs care)
- Language support (Phase 2.6) — needs a multilingual model download + whisper
  language wiring + testing; do as a focused task.
- Audio playback UI — moderate; serve the saved WAV to an <audio> element.
- Export a meeting (zip, incl. notes) — moderate.
- Model bundling / metal / tiny.en fallback — packaging + build-tuning pass.
