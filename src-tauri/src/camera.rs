// Camera capture: spawns the AVFoundation helper binary and reads its
// length-prefixed JPEG frames (4-byte little-endian length, then the bytes) from
// stdout at ~1 Hz. The latest frame is written to `frame_path` (overwritten each
// tick) for the vision pipeline and the live preview to pick up, and a
// `camera-frame` event tells the UI to refresh. Capture runs whenever the camera
// is enabled, independent of recording. Requires the Camera permission.

use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use tauri::{AppHandle, Emitter};

// Guard against a desync on the framing protocol allocating absurd buffers.
const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

pub struct CameraCapture {
    child: Child,
}

impl CameraCapture {
    pub fn start(helper_path: &str, frame_path: PathBuf, app: AppHandle) -> Result<Self, String> {
        let mut child = Command::new(helper_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to start camera helper ({helper_path}): {e}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "camera helper produced no stdout".to_string())?;

        // Read length-prefixed JPEG frames; persist the latest and notify the UI.
        let frame_app = app.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut len_buf = [0u8; 4];
                if reader.read_exact(&mut len_buf).is_err() {
                    break;
                }
                let len = u32::from_le_bytes(len_buf) as usize;
                if len == 0 || len > MAX_FRAME_BYTES {
                    break;
                }
                let mut jpeg = vec![0u8; len];
                if reader.read_exact(&mut jpeg).is_err() {
                    break;
                }
                // Write to a temp file then rename so a reader never sees a
                // half-written frame.
                let tmp = frame_path.with_extension("tmp");
                if std::fs::write(&tmp, &jpeg).is_ok() && std::fs::rename(&tmp, &frame_path).is_ok() {
                    let _ = frame_app.emit("camera-frame", ());
                }
            }
        });

        // Surface helper messages (permission errors, ready, etc.) to the UI.
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    eprintln!("[camera] {line}");
                    let lower = line.to_lowercase();
                    let msg = if lower.contains("permission denied") {
                        "Camera needs permission — grant it in System Settings › Privacy & \
                         Security › Camera, then restart."
                            .to_string()
                    } else if lower.contains("no camera available") {
                        "No camera found.".to_string()
                    } else if lower.contains("capturing camera") {
                        "Capturing camera frames.".to_string()
                    } else {
                        continue;
                    };
                    let _ = app.emit("camera-status", msg);
                }
            });
        }

        Ok(Self { child })
    }
}

impl Drop for CameraCapture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
