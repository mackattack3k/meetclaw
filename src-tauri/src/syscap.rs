// System-audio capture: spawns the ScreenCaptureKit helper binary and reads its
// raw 48 kHz mono Float32 LE PCM from stdout into the audio channel, so the
// pipeline can transcribe what other apps are playing (e.g. remote meeting
// participants). Requires the Screen Recording permission.

use std::io::{BufRead, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;

use tauri::{AppHandle, Emitter};

pub const SAMPLE_RATE: u32 = 48_000;

pub struct SystemAudioCapture {
    child: Child,
}

impl SystemAudioCapture {
    pub fn start(
        tx: Sender<Vec<f32>>,
        helper_path: &str,
        app: AppHandle,
    ) -> Result<Self, String> {
        let mut child = Command::new(helper_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                format!("failed to start system-audio helper ({helper_path}): {e}")
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "system-audio helper produced no stdout".to_string())?;

        // Read PCM and forward to the pipeline. Carry partial samples across reads.
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut buf = [0u8; 8192];
            let mut acc: Vec<u8> = Vec::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        let whole = acc.len() - (acc.len() % 4);
                        if whole > 0 {
                            let samples: Vec<f32> = acc[..whole]
                                .chunks_exact(4)
                                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                                .collect();
                            acc.drain(..whole);
                            if tx.send(samples).is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Surface helper messages (permission errors, ready, etc.) to the UI.
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(stderr)
                    .lines()
                    .map_while(Result::ok)
                {
                    eprintln!("[syscap] {line}");
                    let lower = line.to_lowercase();
                    let msg = if lower.contains("declined tcc") || lower.contains("capture failed") {
                        "System audio needs Screen Recording permission — grant it in System \
                         Settings › Privacy & Security › Screen Recording, then restart."
                            .to_string()
                    } else if lower.contains("capturing system audio") {
                        "Capturing system audio (other participants).".to_string()
                    } else {
                        continue;
                    };
                    let _ = app.emit("syscap-status", msg);
                }
            });
        }

        Ok(Self { child })
    }
}

impl Drop for SystemAudioCapture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
