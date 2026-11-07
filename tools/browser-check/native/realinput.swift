// Real input for go.mjs, as its own small app ("Brigadier Check Input.app"): macOS grants Accessibility
// to this and nothing else, and an ad-hoc grant is tied to its hash, so it isn't rebuilt once made. Events
// are posted to the system session, each marked, while a listen-only tap watches for any event without
// the mark. That's a person, and the first one ends everything: the pointer goes back and the tool exits 3.
// go.mjs starts it through LaunchServices with --socket <path>; commands and replies are JSON lines there.
// --check only reports (permission, pointer, front app, cursor) and exits: no tap, no input, no prompt.
// --ask raises macOS's own prompt, and is for making the grant, not for any check.
import AppKit
import ApplicationServices
import Foundation

let MARK: Int64 = 0xB1_6AD1
let source = CGEventSource(stateID: .privateState)
let startPoint = CGEvent(source: nil)?.location ?? .zero
let startFront = NSWorkspace.shared.frontmostApplication

let arguments = CommandLine.arguments
// --ask: once, with the user expecting it, so that macOS lists this app under its privacy setting.
if arguments.contains("--ask") { _ = NSApplication.shared; _ = AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary); RunLoop.main.run(until: Date().addingTimeInterval(3)); exit(0) }
guard let socketAt = arguments.firstIndex(of: "--socket"), socketAt + 1 < arguments.count else { exit(64) }
let channel = socket(AF_UNIX, SOCK_STREAM, 0)
var address = sockaddr_un()
address.sun_family = sa_family_t(AF_UNIX)
_ = arguments[socketAt + 1].withCString { path in
  withUnsafeMutableBytes(of: &address.sun_path) { strncpy($0.baseAddress!.assumingMemoryBound(to: CChar.self), path, $0.count - 1) }
}
let connected = withUnsafePointer(to: &address) {
  $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(channel, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
}
guard connected == 0 else { exit(65) }

func reply(_ value: [String: Any]) {
  if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) {
    let line = data + Data("\n".utf8)
    _ = line.withUnsafeBytes { write(channel, $0.baseAddress, $0.count) }
  }
}

func restore() {
  CGWarpMouseCursorPosition(startPoint)
  CGAssociateMouseAndMouseCursorPosition(1)
  startFront?.activate()
}

func human(_ what: String) -> Never {
  restore()
  reply(["human": what])
  exit(3)
}

_ = NSApplication.shared

/** The cursor's identity, by what AppKit calls it: the app must be in front for macOS to show it. */
func cursorName() -> String {
  guard let cursor = NSCursor.currentSystem else { return "none" }
  let spot = cursor.hotSpot
  let size = cursor.image.size
  let known: [(String, NSCursor)] = [("arrow", .arrow), ("hand", .pointingHand), ("text", .iBeam), ("resize-col", .resizeLeftRight)]
  for (name, each) in known where each.image.size == size && each.hotSpot == spot { return name }
  return "other \(Int(size.width))x\(Int(size.height))@\(Int(spot.x)),\(Int(spot.y))"
}

// AXIsProcessTrusted never prompts, and nothing that could prompt runs before it says yes.
reply([
  "hello": true,
  "trusted": AXIsProcessTrusted(),
  "pid": getpid(),
  "pointer": [startPoint.x, startPoint.y],
  "front": startFront?.localizedName ?? "?",
  "frontPid": startFront?.processIdentifier ?? -1,
  "cursor": cursorName(),
])
if arguments.contains("--check") { exit(0) }

guard AXIsProcessTrusted() else {
  reply(["error": "needs Accessibility permission for the tool that started this; grant it and run go again"])
  exit(4)
}

let mask = [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp, .leftMouseDragged, .rightMouseDown, .rightMouseUp, .keyDown, .keyUp, .flagsChanged, .scrollWheel]
  .reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << $1.rawValue) }
guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .listenOnly, eventsOfInterest: mask, callback: { _, type, event, _ in
  if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput { return Unmanaged.passUnretained(event) }
  if event.getIntegerValueField(.eventSourceUserData) != MARK { human("\(type.rawValue)") }
  return Unmanaged.passUnretained(event)
}, userInfo: nil) else {
  reply(["error": "couldn't watch for human input, so nothing runs"])
  exit(4)
}
CFRunLoopAddSource(CFRunLoopGetMain(), CFMachPortCreateRunLoopSource(nil, tap, 0), .commonModes)
CGEvent.tapEnable(tap: tap, enable: true)

func post(_ event: CGEvent?) {
  guard let event else { return }
  event.setIntegerValueField(.eventSourceUserData, value: MARK)
  event.post(tap: .cgSessionEventTap)
}
func mouse(_ type: CGEventType, _ point: CGPoint, _ button: CGMouseButton = .left, clicks: Int64 = 0) {
  let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: button)
  if clicks > 0 { event?.setIntegerValueField(.mouseEventClickState, value: clicks) }
  post(event)
}
func pause(_ ms: Double) { RunLoop.current.run(until: Date().addingTimeInterval(ms / 1000)) }

func handle(_ c: [String: Any]) -> [String: Any] {
  let at = CGPoint(x: c["x"] as? Double ?? 0, y: c["y"] as? Double ?? 0)
  switch c["op"] as? String ?? "" {
  case "move":
    mouse(.mouseMoved, at)
  case "click":
    mouse(.mouseMoved, at); pause(60)
    mouse(.leftMouseDown, at, clicks: 1); pause(40)
    mouse(.leftMouseUp, at, clicks: 1)
  case "drag":
    let to = CGPoint(x: c["toX"] as? Double ?? 0, y: c["toY"] as? Double ?? 0)
    let steps = c["steps"] as? Int ?? 40
    mouse(.mouseMoved, at); pause(60)
    mouse(.leftMouseDown, at, clicks: 1); pause(40)
    for step in 1...steps {
      let t = Double(step) / Double(steps)
      mouse(.leftMouseDragged, CGPoint(x: at.x + (to.x - at.x) * t, y: at.y + (to.y - at.y) * t))
      pause(16)
    }
    mouse(.leftMouseUp, to, clicks: 1)
  case "scroll":
    post(CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 1, wheel1: Int32(c["dy"] as? Int ?? 0), wheel2: 0, wheel3: 0))
  case "key":
    let code = CGKeyCode(c["code"] as? Int ?? 0)
    var flags = CGEventFlags()
    for name in c["flags"] as? [String] ?? [] {
      switch name {
      case "cmd": flags.insert(.maskCommand)
      case "shift": flags.insert(.maskShift)
      case "ctrl": flags.insert(.maskControl)
      default: break
      }
    }
    for down in [true, false] {
      let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down)
      event?.flags = flags
      post(event)
      pause(30)
    }
  case "type":
    for scalar in (c["text"] as? String ?? "").utf16 {
      for down in [true, false] {
        let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down)
        var unit = scalar
        event?.keyboardSetUnicodeString(stringLength: 1, unicodeString: &unit)
        post(event)
        pause(12)
      }
    }
  case "cursor":
    return ["cursor": cursorName()]
  case "front":
    let front = NSWorkspace.shared.frontmostApplication
    return ["front": front?.localizedName ?? "?", "pid": front?.processIdentifier ?? -1]
  case "activate":
    NSRunningApplication(processIdentifier: Int32(c["pid"] as? Int ?? -1))?.activate()
  case "restore":
    restore()
    reply(["restored": true])
    exit(0)
  default:
    return ["error": "unknown op"]
  }
  return ["ok": true]
}

// Replies on the main thread, so the tap keeps running through every pause.
let lines = DispatchQueue(label: "commands")
lines.async {
  let input = fdopen(channel, "r")
  var buffer: UnsafeMutablePointer<CChar>? = nil
  var capacity = 0
  while getline(&buffer, &capacity, input) > 0, let text = buffer {
    let data = Data(String(cString: text).utf8)
    guard let command = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { continue }
    DispatchQueue.main.sync { reply(handle(command)) }
  }
  DispatchQueue.main.async { restore(); exit(0) }
}
RunLoop.main.run()
