// Thin wrapper around whisper.cpp (via whisper-rs).
// Loads the model once, then transcribes 16 kHz mono f32 audio windows.

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct Transcriber {
    ctx: WhisperContext,
}

pub struct Transcription {
    pub text: String,
    /// Full language name (e.g. "english"), detected when `language` is "auto".
    pub language: Option<String>,
}

impl Transcriber {
    pub fn new(model_path: &str) -> Result<Self, String> {
        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .map_err(|e| format!("failed to load whisper model: {e}"))?;
        Ok(Self { ctx })
    }

    /// Transcribe one window of 16 kHz mono audio. `language` is a 2-letter code
    /// (e.g. "en") or "auto" to let whisper detect it. Returns the text plus the
    /// language whisper used/detected.
    pub fn transcribe(&self, samples: &[f32], language: &str) -> Result<Transcription, String> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| format!("failed to create whisper state: {e}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(language));
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, samples)
            .map_err(|e| format!("whisper inference failed: {e}"))?;

        let num_segments = state
            .full_n_segments()
            .map_err(|e| format!("failed to count segments: {e}"))?;

        let mut text = String::new();
        for i in 0..num_segments {
            if let Ok(seg) = state.full_get_segment_text(i) {
                text.push_str(seg.trim());
                text.push(' ');
            }
        }

        let language = state
            .full_lang_id_from_state()
            .ok()
            .and_then(whisper_rs::get_lang_str_full)
            .map(|s| s.to_string());

        Ok(Transcription {
            text: text.trim().to_string(),
            language,
        })
    }
}
