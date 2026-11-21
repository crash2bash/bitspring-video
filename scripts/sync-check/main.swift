// Records one application's window together with the sound it makes, both stamped by the
// same host clock, so the distance between a flash and its beep can be read off.
//
// usage: capture <pid> <seconds> <output prefix>
// writes <prefix>.video.txt  (host seconds when shown, mean brightness 0-255)
//        <prefix>.audio.txt  (host seconds of the buffer's first sample, frames, sample rate, offset into .raw)
//        <prefix>.audio.raw  (float32 samples of the first channel)

import AVFoundation
import AppKit
import CoreMedia
import CoreVideo
import Foundation
import ScreenCaptureKit

var timebase = mach_timebase_info_data_t()
mach_timebase_info(&timebase)
func seconds(_ host: UInt64) -> Double {
    Double(host) * Double(timebase.numer) / Double(timebase.denom) / 1e9
}

final class Recorder: NSObject, SCStreamOutput, SCStreamDelegate {
    let video: FileHandle
    let index: FileHandle
    let raw: FileHandle
    var written = 0
    var frames = 0
    var buffers = 0

    init(prefix: String) {
        func make(_ suffix: String) -> FileHandle {
            let path = prefix + suffix
            FileManager.default.createFile(atPath: path, contents: nil)
            return FileHandle(forWritingAtPath: path)!
        }
        video = make(".video.txt")
        index = make(".audio.txt")
        raw = make(".audio.raw")
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        FileHandle.standardError.write("stream stopped: \(error)\n".data(using: .utf8)!)
    }

    func stream(_ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard buffer.isValid else { return }
        switch type {
        case .screen:
            guard
                let list = CMSampleBufferGetSampleAttachmentsArray(buffer, createIfNecessary: false)
                    as? [[SCStreamFrameInfo: Any]],
                let info = list.first,
                let code = info[.status] as? Int,
                SCFrameStatus(rawValue: code) == .complete,
                let pixels = buffer.imageBuffer
            else { return }
            let stamped = buffer.presentationTimeStamp.seconds
            let shown = (info[.displayTime] as? UInt64).map(seconds) ?? 0

            CVPixelBufferLockBaseAddress(pixels, .readOnly)
            let width = CVPixelBufferGetWidth(pixels)
            let height = CVPixelBufferGetHeight(pixels)
            let stride = CVPixelBufferGetBytesPerRow(pixels)
            var sum = 0
            var count = 0
            if let base = CVPixelBufferGetBaseAddress(pixels) {
                let bytes = base.assumingMemoryBound(to: UInt8.self)
                // The middle of the picture, away from the controls and the window's edges.
                for y in Swift.stride(from: height * 3 / 8, to: height * 5 / 8, by: 4) {
                    for x in Swift.stride(from: width * 3 / 8, to: width * 5 / 8, by: 4) {
                        let p = bytes + y * stride + x * 4
                        sum += Int(p[0]) + Int(p[1]) + Int(p[2])
                        count += 3
                    }
                }
            }
            CVPixelBufferUnlockBaseAddress(pixels, .readOnly)
            let brightness = count > 0 ? Double(sum) / Double(count) : 0
            video.write(String(format: "%.6f %.6f %.2f\n", shown, stamped, brightness).data(using: .utf8)!)
            frames += 1
        case .audio:
            guard let format = buffer.formatDescription?.audioStreamBasicDescription else { return }
            let stamped = buffer.presentationTimeStamp.seconds
            let count = buffer.numSamples
            try? buffer.withAudioBufferList { list, _ in
                guard let first = list.first, let data = first.mData else { return }
                let interleaved = format.mFormatFlags & kAudioFormatFlagIsNonInterleaved == 0
                let step = interleaved ? Int(format.mChannelsPerFrame) : 1
                let samples = data.assumingMemoryBound(to: Float32.self)
                var channel = [Float32](repeating: 0, count: count)
                for i in 0..<count { channel[i] = samples[i * step] }
                channel.withUnsafeBytes { raw.write(Data($0)) }
            }
            index.write(String(format: "%.6f %d %.0f %d\n", stamped, count, format.mSampleRate, written).data(using: .utf8)!)
            written += count
            buffers += 1
        default:
            break
        }
    }
}

// A connection to the window server, which ScreenCaptureKit needs from a plain program.
_ = NSApplication.shared
_ = CGMainDisplayID()

let arguments = CommandLine.arguments
guard arguments.count == 4, let pid = Int32(arguments[1]), let length = Double(arguments[2]) else {
    print("usage: capture <pid> <seconds> <output prefix>")
    exit(2)
}

let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
let own = content.windows.filter { $0.owningApplication?.processID == pid && $0.windowLayer == 0 }
guard let window = own.max(by: { $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height }) else {
    print("no window of process \(pid)")
    exit(1)
}
print("window \(window.windowID) \(Int(window.frame.width))x\(Int(window.frame.height)) on screen: \(window.isOnScreen)")

let configuration = SCStreamConfiguration()
configuration.width = 320
configuration.height = 180
configuration.minimumFrameInterval = CMTime(value: 1, timescale: 240)
configuration.pixelFormat = kCVPixelFormatType_32BGRA
configuration.showsCursor = false
configuration.queueDepth = 8
configuration.capturesAudio = true
configuration.sampleRate = 48000
configuration.channelCount = 2
configuration.excludesCurrentProcessAudio = true

let recorder = Recorder(prefix: arguments[3])
let stream = SCStream(filter: SCContentFilter(desktopIndependentWindow: window), configuration: configuration, delegate: recorder)
let queue = DispatchQueue(label: "capture")
try stream.addStreamOutput(recorder, type: .screen, sampleHandlerQueue: queue)
try stream.addStreamOutput(recorder, type: .audio, sampleHandlerQueue: queue)
try await stream.startCapture()
print(String(format: "started at host %.6f", seconds(mach_absolute_time())))
try await Task.sleep(nanoseconds: UInt64(length * 1e9))
try await stream.stopCapture()
queue.sync {}
print("frames \(recorder.frames), sound buffers \(recorder.buffers), samples \(recorder.written)")
