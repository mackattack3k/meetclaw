// One-shot, whole-file transcription for quality analysis.
// Usage: cargo run --example transcribe -- /path/to/audio.wav
//
// Unlike the live pipeline (which transcribes short silence-delimited chunks),
// this decodes the entire recording in a single pass with full context — i.e.
// what a slower, higher-quality "final transcript" tier would produce.

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn main() {
    let path = std::env::args().nth(1).expect("usage: transcribe <audio.wav>");
    let model = format!("{}/models/ggml-base.bin", env!("CARGO_MANIFEST_DIR"));

    let mut reader = hound::WavReader::open(&path).expect("open wav");
    let spec = reader.spec();
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.expect("sample") as f32 / 32768.0)
        .collect();
    eprintln!(
        "loaded {} samples @ {} Hz, {} ch (~{:.1}s)",
        samples.len(),
        spec.sample_rate,
        spec.channels,
        samples.len() as f32 / spec.sample_rate as f32
    );

    let ctx = WhisperContext::new_with_params(&model, WhisperContextParameters::default())
        .expect("load model");
    let mut state = ctx.create_state().expect("state");

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("auto"));
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    state.full(params, &samples).expect("transcribe");

    let n = state.full_n_segments().expect("segments");
    let mut text = String::new();
    for i in 0..n {
        if let Ok(seg) = state.full_get_segment_text(i) {
            text.push_str(seg.trim());
            text.push(' ');
        }
    }
    if let Ok(id) = state.full_lang_id_from_state() {
        eprintln!("detected language: {:?}", whisper_rs::get_lang_str_full(id));
    }
    println!("{}", text.trim());
}
