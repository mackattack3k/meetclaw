// Microphone capture via cpal.
// Captures the default input device, downmixes to mono, and streams f32
// samples (at the device's native sample rate) through an mpsc channel.
// Resampling to 16 kHz happens downstream in the pipeline.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Stream, StreamConfig};
use std::sync::mpsc::Sender;

pub struct AudioCapture {
    // Keep the stream alive for as long as we want to record.
    _stream: Stream,
    pub sample_rate: u32,
}

impl AudioCapture {
    /// Start capturing the default input device. Mono f32 samples are sent
    /// through `tx`. The stream stops when the returned `AudioCapture` drops.
    pub fn start(tx: Sender<Vec<f32>>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "no input (microphone) device found".to_string())?;

        let default_config = device
            .default_input_config()
            .map_err(|e| format!("failed to read default input config: {e}"))?;

        let sample_rate = default_config.sample_rate().0;
        let channels = default_config.channels() as usize;
        let config: StreamConfig = default_config.clone().into();

        let err_fn = |err| eprintln!("audio stream error: {err}");

        // We only handle f32 input here; the default config on macOS is f32.
        let stream = match default_config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono = downmix_to_mono(data, channels);
                    // Ignore send errors (receiver gone = we're shutting down).
                    let _ = tx.send(mono);
                },
                err_fn,
                None,
            ),
            other => {
                return Err(format!(
                    "unsupported sample format {other:?} (expected f32)"
                ))
            }
        }
        .map_err(|e| format!("failed to build input stream: {e}"))?;

        stream
            .play()
            .map_err(|e| format!("failed to start input stream: {e}"))?;

        Ok(Self {
            _stream: stream,
            sample_rate,
        })
    }
}

fn downmix_to_mono(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Simple linear resampler from `from_rate` to `to_rate` (mono).
/// Good enough for speech; whisper is robust to minor artifacts.
pub fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = (input.len() as f64 * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        let frac = src_pos - idx as f64;
        let a = input[idx.min(input.len() - 1)];
        let b = input[(idx + 1).min(input.len() - 1)];
        out.push(a + (b - a) * frac as f32);
    }
    out
}
