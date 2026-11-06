// Records one window through ScreenCaptureKit, whatever covers it, to raw frames piped into ffmpeg,
// with a line per frame giving its time. winrec <windowId> <seconds> <out.mp4> [fps] [pixels per point]
import AppKit
import AVFoundation
import CoreMedia
import Foundation
import ScreenCaptureKit

let args = CommandLine.arguments
guard args.count >= 4, let windowId = CGWindowID(args[1]), let seconds = Double(args[2]) else {
  fputs("usage: winrec <windowId> <seconds> <out.mp4> [fps]\n", stderr)
  exit(64)
}
let out = args[3]
let fps = args.count > 4 ? Int32(args[4])! : 60
let pixelsPerPoint = args.count > 5 ? Double(args[5])! : 0

final class Sink: NSObject, SCStreamOutput {
  var ffmpeg: Process?
  var stdin: FileHandle?
  var times: FileHandle
  var count = 0
  let lock = NSLock()
  init(timesPath: String) {
    FileManager.default.createFile(atPath: timesPath, contents: nil)
    times = FileHandle(forWritingAtPath: timesPath)!
  }
  func start(width: Int, height: Int) throws {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/opt/homebrew/bin/ffmpeg")
    process.arguments = ["-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "bgra", "-s", "\(width)x\(height)", "-r", "\(fps)", "-i", "-", "-c:v", "libx264", "-preset", "ultrafast", "-crf", "12", "-pix_fmt", "yuv420p", out]
    let pipe = Pipe()
    process.standardInput = pipe
    try process.run()
    ffmpeg = process
    stdin = pipe.fileHandleForWriting
  }
  func stream(_ stream: SCStream, didOutputSampleBuffer buffer: CMSampleBuffer, of type: SCStreamOutputType) {
    guard type == .screen, buffer.isValid, let image = CMSampleBufferGetImageBuffer(buffer) else { return }
    lock.lock(); defer { lock.unlock() }
    CVPixelBufferLockBaseAddress(image, .readOnly)
    defer { CVPixelBufferUnlockBaseAddress(image, .readOnly) }
    let width = CVPixelBufferGetWidth(image), height = CVPixelBufferGetHeight(image)
    if ffmpeg == nil { try? start(width: width, height: height) }
    guard let base = CVPixelBufferGetBaseAddress(image) else { return }
    let stride = CVPixelBufferGetBytesPerRow(image)
    var data = Data(capacity: width * height * 4)
    for row in 0..<height { data.append(Data(bytes: base + row * stride, count: width * 4)) }
    stdin?.write(data)
    count += 1
    let t = CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(buffer))
    times.write(Data("\(count) \(t) \(width) \(height)\n".utf8))
  }
  func finish() {
    lock.lock(); defer { lock.unlock() }
    try? stdin?.close()
    ffmpeg?.waitUntilExit()
  }
}

// ScreenCaptureKit needs the process joined to the window server.
_ = NSApplication.shared

Task {
  do {
    let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
    guard let window = content.windows.first(where: { $0.windowID == windowId }) else {
      fputs("no window \(windowId)\n", stderr); exit(66)
    }
    let filter = SCContentFilter(desktopIndependentWindow: window)
    let config = SCStreamConfiguration()
    // 0: the window's own pixels. A smaller size is what lets 120 fps keep up.
    let scale = pixelsPerPoint > 0 ? CGFloat(pixelsPerPoint) : CGFloat(filter.pointPixelScale)
    config.width = Int(window.frame.width * scale)
    config.height = Int(window.frame.height * scale)
    config.minimumFrameInterval = CMTime(value: 1, timescale: fps)
    config.pixelFormat = kCVPixelFormatType_32BGRA
    config.showsCursor = false
    config.queueDepth = 8
    let sink = Sink(timesPath: out + ".times")
    let stream = SCStream(filter: filter, configuration: config, delegate: nil)
    try stream.addStreamOutput(sink, type: .screen, sampleHandlerQueue: DispatchQueue(label: "frames"))
    try await stream.startCapture()
    print("recording \(windowId) \(config.width)x\(config.height)"); fflush(stdout)
    try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
    try await stream.stopCapture()
    sink.finish()
    print("frames \(sink.count)")
    exit(0)
  } catch {
    fputs("winrec failed: \(error)\n", stderr)
    exit(1)
  }
}
RunLoop.main.run()
