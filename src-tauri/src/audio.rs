// Microphone capture via cpal.
// Captures a chosen (or default) input device, converts any sample format to
// f32, downmixes to mono, and streams the samples through an mpsc channel.
// Resampling to 16 kHz happens downstream in the pipeline.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, Host, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::sync::mpsc::Sender;

pub struct AudioCapture {
    // Keep the stream alive for as long as we want to record.
    _stream: Stream,
    pub sample_rate: u32,
}

impl AudioCapture {
    /// Start capturing. `device_name` selects a specific input device by name;
    /// `None` uses the system default. Mono f32 samples are sent through `tx`.
    /// The stream stops when the returned `AudioCapture` drops.
    pub fn start(tx: Sender<Vec<f32>>, device_name: Option<&str>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = match device_name {
            Some(name) => find_input_device(&host, name)?,
            None => host
                .default_input_device()
                .ok_or_else(|| "no input (microphone) device found".to_string())?,
        };

        let default_config = device
            .default_input_config()
            .map_err(|e| format!("failed to read default input config: {e}"))?;

        let sample_rate = default_config.sample_rate().0;
        let channels = default_config.channels() as usize;
        let sample_format = default_config.sample_format();
        let config: StreamConfig = default_config.into();

        // Handle the common device sample formats; convert each to f32.
        let stream = match sample_format {
            SampleFormat::F32 => build_stream::<f32>(&device, &config, channels, tx),
            SampleFormat::I16 => build_stream::<i16>(&device, &config, channels, tx),
            SampleFormat::U16 => build_stream::<u16>(&device, &config, channels, tx),
            SampleFormat::I32 => build_stream::<i32>(&device, &config, channels, tx),
            SampleFormat::I8 => build_stream::<i8>(&device, &config, channels, tx),
            SampleFormat::U8 => build_stream::<u8>(&device, &config, channels, tx),
            other => Err(format!("unsupported sample format: {other:?}")),
        }?;

        stream
            .play()
            .map_err(|e| format!("failed to start input stream: {e}"))?;

        Ok(Self {
            _stream: stream,
            sample_rate,
        })
    }
}

/// List the names of available input devices.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut names = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if let Ok(name) = device.name() {
                names.push(name);
            }
        }
    }
    names
}

fn find_input_device(host: &Host, name: &str) -> Result<Device, String> {
    host.input_devices()
        .map_err(|e| format!("failed to enumerate input devices: {e}"))?
        .find(|d| d.name().map(|n| n == name).unwrap_or(false))
        .ok_or_else(|| format!("input device '{name}' not found"))
}

fn build_stream<T>(
    device: &Device,
    config: &StreamConfig,
    channels: usize,
    tx: Sender<Vec<f32>>,
) -> Result<Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let _ = tx.send(downmix_to_mono(data, channels));
            },
            stream_err,
            None,
        )
        .map_err(|e| format!("failed to build input stream: {e}"))
}

fn stream_err(err: cpal::StreamError) {
    eprintln!("audio stream error: {err}");
}

fn downmix_to_mono<T>(data: &[T], channels: usize) -> Vec<f32>
where
    T: Sample,
    f32: FromSample<T>,
{
    if channels <= 1 {
        return data.iter().map(|&s| f32::from_sample(s)).collect();
    }
    data.chunks(channels)
        .map(|frame| {
            let sum: f32 = frame.iter().map(|&s| f32::from_sample(s)).sum();
            sum / channels as f32
        })
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
