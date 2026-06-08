// meetclaw-syscap: captures system audio (what other apps are playing — e.g. the
// other participants in a Zoom/Meet call) via ScreenCaptureKit and writes raw
// 48 kHz mono Float32 LE PCM to stdout. The Rust app spawns this as a sidecar.
//
// Requires the Screen Recording permission (System Settings › Privacy &
// Security › Screen Recording). On failure it prints to stderr and exits non-zero.

import AVFoundation
import CoreMedia
import Darwin
import ScreenCaptureKit

let SAMPLE_RATE = 48000

func logErr(_ s: String) {
    FileHandle.standardError.write((s + "\n").data(using: .utf8)!)
}

final class Capturer: NSObject, SCStreamOutput, SCStreamDelegate {
    var loggedFirst = false

    func stream(
        _ stream: SCStream,
        didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
        of type: SCStreamOutputType
    ) {
        guard type == .audio, sampleBuffer.isValid else { return }
        if !loggedFirst {
            loggedFirst = true
            logErr("first audio buffer received")
        }
        do {
            try sampleBuffer.withAudioBufferList { abl, _ in
                for buffer in abl {
                    guard let data = buffer.mData else { continue }
                    let n = Int(buffer.mDataByteSize)
                    if n > 0 {
                        // Write all bytes; bail out if the parent closed the pipe.
                        let written = Darwin.write(1, data, n)
                        if written < 0 { exit(0) }
                    }
                }
            }
        } catch {
            logErr("audio read error: \(error)")
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        logErr("stream stopped: \(error)")
        exit(1)
    }
}

// Held for the process lifetime — SCStream keeps its delegate/output weakly, so
// these must outlive run() or capture silently stops delivering buffers.
var activeStream: SCStream?
var activeCapturer: Capturer?

func run() async {
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(
            false, onScreenWindowsOnly: false)
        guard let display = content.displays.first else {
            logErr("no display available")
            exit(2)
        }

        let filter = SCContentFilter(display: display, excludingWindows: [])
        let config = SCStreamConfiguration()
        config.capturesAudio = true
        config.sampleRate = SAMPLE_RATE
        config.channelCount = 1
        config.excludesCurrentProcessAudio = true
        // We only want audio; keep the (required) video path minimal.
        config.width = 2
        config.height = 2
        config.minimumFrameInterval = CMTime(value: 1, timescale: 1)

        let capturer = Capturer()
        let stream = SCStream(filter: filter, configuration: config, delegate: capturer)
        activeCapturer = capturer
        activeStream = stream
        try stream.addStreamOutput(
            capturer, type: .audio, sampleHandlerQueue: DispatchQueue(label: "meetclaw.syscap.audio"))
        // A screen output is also attached (frames discarded) — ScreenCaptureKit
        // often won't deliver audio buffers without it.
        try stream.addStreamOutput(
            capturer, type: .screen, sampleHandlerQueue: DispatchQueue(label: "meetclaw.syscap.video"))
        try await stream.startCapture()
        logErr("capturing system audio")
    } catch {
        logErr("capture failed: \(error)")
        exit(3)
    }
}

signal(SIGPIPE, SIG_IGN)
Task { await run() }
RunLoop.main.run()
