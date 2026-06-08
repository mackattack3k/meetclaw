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
- [ ] Audio playback of saved meetings — when opening a meeting from the
      Library, show a player for its `audio.wav`. Serve the file with Tauri's
      asset protocol (`convertFileSrc`) into an `<audio controls>` element;
      add a command returning the meeting's audio path. Stretch: scrub the
      transcript in sync with playback (we have time-aligned 16 kHz audio).
- [ ] Export a meeting (zip / share)

## Phase 2.6 — Language support (done)
- [x] Language selector in the console rail (auto-detect + ~20 languages)
- [x] Show the detected/active language next to the Lang selector
- [x] Switched to the multilingual `ggml-base` model
- [x] Confidence + hysteresis auto-detect: ignore detections under 0.5 conf,
      require a new language to persist 2 chunks before switching. Rejects
      one-off mis-detects (the "Welsh"/"Finnish" blips) while still following a
      genuine mid-meeting language switch. Verified against real recordings.
- [x] Feed recent transcript as whisper `initial_prompt` for live continuity.
- [ ] Per-segment multilingual FINAL transcript — the whole-file final pass
      still picks one language for the whole recording, so a bilingual meeting's
      minority-language parts get mis-transcribed in the final tier. (Live tier
      handles sequential switches via hysteresis; final pass would need to
      segment by language and transcribe each separately.)

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
- [x] Two-tier transcript: fast live tier (chunked) for immediate notes +
      a whole-file re-transcription on Stop that replaces transcript.txt with a
      coherent, accurate version (`finalize_transcript` in lib.rs; UI swaps it
      in via `transcript-finalized`). Verified worth it on a real 94s clip.
- [ ] Use a bigger model (small/medium) for the final pass for even better
      quality (needs a separate model download; live pass stays on base).
- [ ] Real-time streaming transcription (instead of chunk-on-pause). Today we
      wait for a silence boundary, then transcribe the whole chunk, so text
      appears in bursts. Stream it: keep a rolling buffer and re-decode a
      sliding window every ~0.5–1s, emitting *interim* text that's replaced as
      the window advances and *finalized* at silence (whisper.cpp `stream`
      pattern). Needs: interim-vs-final transcript events, UI that shows interim
      text greyed then commits it, and dedupe/reconcile of overlapping windows.
      Tradeoff: more compute (overlapping re-decodes) — may want `tiny`/`base`
      + the `metal` feature to keep up. Bigger change to the pipeline.
- [ ] Label different speakers (diarization). Whisper doesn't do this natively.
      Options: (a) cheap 2-way labeling once system-audio (Phase 4) lands —
      mic = "Me", system = "Others"; (b) real diarization via speaker embeddings
      per segment + clustering (e.g. a pyannote-style model), heavier and no
      easy Rust path; (c) prompt the LLM to guess speaker turns from transcript
      content (rough). Show speaker tags in the transcript; feed them to Gemini.

## Phase 4 — System audio (digital meetings) (partial)
- [x] Capture other participants' audio via ScreenCaptureKit — Swift helper
      (`src-tauri/syscap/main.swift`) streams 48 kHz mono PCM to a Rust sidecar
      reader (`src-tauri/src/syscap.rs`)
- [x] "Audio source" setting: Microphone vs System audio
- [x] Surface the Screen Recording permission error to the UI (`syscap-status`)
- [x] Mix mic + system audio (capture BOTH at once) — "Both" source mode for
      hybrid meetings (you + room on mic, remote participants on system audio).
      Mic-driven mixer (`src-tauri/src/mixer.rs`): resample both to 16 kHz, sum
      onto each mic chunk, silence-fill when the call is quiet.
- [ ] Bundle the Swift helper as a Tauri sidecar for distributable builds
      (today it's spawned from `src-tauri/binaries/`, built via
      `src-tauri/syscap/build.sh`)
- [ ] Auto-trigger / guide the Screen Recording permission prompt on first use

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
