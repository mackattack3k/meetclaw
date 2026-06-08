// Mixed capture: record the microphone (you) and system audio (the other
// participants) at once, summed into a single 16 kHz stream for the pipeline.
//
// The mic is the clock: it's continuous, so we drive mixing off each mic chunk
// and overlay whatever system audio has arrived since. System audio is bursty
// (silent when the call is quiet), so missing system samples just mean "no one
// else is talking" and the mic chunk passes through unchanged.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::AppHandle;

use crate::audio::{resample, AudioCapture};
use crate::syscap::{self, SystemAudioCapture};
use crate::TARGET_RATE;

pub struct MixedCapture {
    _mic: AudioCapture,
    _sys: SystemAudioCapture,
    running: Arc<AtomicBool>,
}

impl MixedCapture {
    /// Emits 16 kHz mono f32 (mic + system, summed) to `out_tx`.
    pub fn start(
        out_tx: Sender<Vec<f32>>,
        device: Option<&str>,
        helper_path: &str,
        app: AppHandle,
    ) -> Result<Self, String> {
        let (mic_tx, mic_rx) = mpsc::channel::<Vec<f32>>();
        let (sys_tx, sys_rx) = mpsc::channel::<Vec<f32>>();

        let mic = AudioCapture::start(mic_tx, device)?;
        let mic_rate = mic.sample_rate;
        let sys = SystemAudioCapture::start(sys_tx, helper_path, app)?;

        let running = Arc::new(AtomicBool::new(true));
        let sys_buf: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));

        // System-audio reader: resample to 16 kHz and queue it.
        {
            let sys_buf = sys_buf.clone();
            let running = running.clone();
            std::thread::spawn(move || {
                let cap = TARGET_RATE as usize * 30; // bound the backlog to ~30s
                while running.load(Ordering::SeqCst) {
                    match sys_rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(chunk) => {
                            let r = resample(&chunk, syscap::SAMPLE_RATE, TARGET_RATE);
                            if let Ok(mut b) = sys_buf.lock() {
                                b.extend(r);
                                while b.len() > cap {
                                    b.pop_front();
                                }
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
        }

        // Mic-driven mixer: for each mic chunk, overlay queued system audio.
        {
            let sys_buf = sys_buf.clone();
            let running = running.clone();
            std::thread::spawn(move || {
                while running.load(Ordering::SeqCst) {
                    match mic_rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(chunk) => {
                            let mut mixed = resample(&chunk, mic_rate, TARGET_RATE);
                            if let Ok(mut b) = sys_buf.lock() {
                                let take = mixed.len().min(b.len());
                                for sample in mixed.iter_mut().take(take) {
                                    if let Some(s) = b.pop_front() {
                                        *sample = (*sample + s).clamp(-1.0, 1.0);
                                    }
                                }
                            }
                            if out_tx.send(mixed).is_err() {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
        }

        Ok(Self {
            _mic: mic,
            _sys: sys,
            running,
        })
    }
}

impl Drop for MixedCapture {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Dropping `_mic` / `_sys` stops the underlying captures.
    }
}
