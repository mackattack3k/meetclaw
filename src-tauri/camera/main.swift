// meetclaw-camera: captures the default camera at ~1 Hz and writes each frame to
// stdout as a length-prefixed JPEG (4-byte little-endian length, then the JPEG
// bytes). The Rust app spawns this as a sidecar and persists the latest frame.
//
// Requires the Camera permission (System Settings › Privacy & Security ›
// Camera). On failure it prints to stderr and exits non-zero.

import AVFoundation
import CoreImage
import Darwin
import Foundation
import ImageIO

let TARGET_INTERVAL = 1.0 // seconds between emitted frames
let JPEG_QUALITY = 0.6

func logErr(_ s: String) {
    FileHandle.standardError.write((s + "\n").data(using: .utf8)!)
}

// Write all bytes to stdout; exit quietly if the parent closed the pipe.
func writeAll(_ data: Data) {
    data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) in
        guard let base = raw.baseAddress else { return }
        var off = 0
        while off < data.count {
            let n = Darwin.write(1, base + off, data.count - off)
            if n <= 0 { exit(0) }
            off += n
        }
    }
}

final class Frames: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
    let ctx = CIContext()
    var lastEmit = Date.distantPast
    var loggedFirst = false

    func captureOutput(
        _ output: AVCaptureOutput,
        didOutput sampleBuffer: CMSampleBuffer,
        from connection: AVCaptureConnection
    ) {
        let now = Date()
        if now.timeIntervalSince(lastEmit) < TARGET_INTERVAL { return }
        lastEmit = now

        guard let pixels = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
        let image = CIImage(cvImageBuffer: pixels)
        let options: [CIImageRepresentationOption: Any] = [
            CIImageRepresentationOption(rawValue: kCGImageDestinationLossyCompressionQuality as String):
                JPEG_QUALITY
        ]
        guard
            let jpeg = ctx.jpegRepresentation(
                of: image, colorSpace: CGColorSpaceCreateDeviceRGB(), options: options)
        else { return }

        if !loggedFirst {
            loggedFirst = true
            logErr("first frame captured")
        }
        var len = UInt32(jpeg.count).littleEndian
        writeAll(Data(bytes: &len, count: 4))
        writeAll(jpeg)
    }
}

// Held for the process lifetime so capture keeps delivering buffers.
var activeSession: AVCaptureSession?
let delegate = Frames()

func startSession() {
    guard let device = AVCaptureDevice.default(for: .video) else {
        logErr("no camera available")
        exit(2)
    }
    do {
        let input = try AVCaptureDeviceInput(device: device)
        let session = AVCaptureSession()
        session.sessionPreset = .high
        if session.canAddInput(input) { session.addInput(input) }
        let outputData = AVCaptureVideoDataOutput()
        outputData.alwaysDiscardsLateVideoFrames = true
        outputData.setSampleBufferDelegate(
            delegate, queue: DispatchQueue(label: "meetclaw.camera"))
        if session.canAddOutput(outputData) { session.addOutput(outputData) }
        session.startRunning()
        activeSession = session
        logErr("capturing camera")
    } catch {
        logErr("camera setup failed: \(error)")
        exit(4)
    }
}

AVCaptureDevice.requestAccess(for: .video) { granted in
    if !granted {
        logErr("camera permission denied")
        exit(3)
    }
    DispatchQueue.main.async { startSession() }
}

signal(SIGPIPE, SIG_IGN)
RunLoop.main.run()
