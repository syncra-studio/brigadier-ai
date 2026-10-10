// The computer-use suite's hard-surfaces fixture (docs/COMPUTER-USE-PLAN.md §8, Phase 5, stream B).
//
// One app with the surfaces real apps have and plain controls don't: a SwiftUI window, a save
// panel and an alert shown as sheets, a minimised window and a window that is ordered out. Every
// control writes what happened to it to a log, one JSON object a line, the same way the
// target-range fixture does: the checkers' ground truth.
//
//   quirk-pad <log path> <save dir>
//
// It never activates itself: it is opened in the background (`open -n -g`) and driven there, its
// windows ordered behind every other window. SIGUSR1 logs every control's state; SIGTERM quits.

import AppKit
import SwiftUI

let logPath = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "/tmp/quirk-pad.log"
let saveDir = URL(fileURLWithPath: CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "/tmp/quirk-pad-save", isDirectory: true).standardizedFileURL
try? FileManager.default.createDirectory(at: saveDir, withIntermediateDirectories: true)
FileManager.default.createFile(atPath: logPath, contents: nil)
let logHandle = FileHandle(forWritingAtPath: logPath)!

func log(_ id: String, _ ev: String, x: Double? = nil, y: Double? = nil, v: String? = nil) {
  var o: [String: Any] = ["t": Date().timeIntervalSince1970 * 1000, "id": id, "ev": ev]
  if let x { o["x"] = x }
  if let y { o["y"] = y }
  if let v { o["v"] = v }
  let data = try! JSONSerialization.data(withJSONObject: o, options: [.sortedKeys])
  logHandle.write(data + Data([0x0a]))
}

final class Flipped: NSView { override var isFlipped: Bool { true } }

/// A plain text view that logs the point of each mouse-down it receives (view points, top-left)
/// and the caret it ends up with: an ordinary control that refuses the first click.
final class NotesView: NSTextView {
  override func mouseDown(with e: NSEvent) {
    let p = convert(e.locationInWindow, from: nil)
    log("notes-view", "down", x: Double(p.x), y: Double(p.y))
    super.mouseDown(with: e)
  }
}

// The SwiftUI window's state, shared with the snapshot.
final class Model: ObservableObject {
  @Published var title = ""
  @Published var starred = false
  @Published var size = "Medium"
  var taps = 0
}
let model = Model()

struct PadView: View {
  @ObservedObject var m: Model
  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      TextField("Title", text: $m.title)
        .accessibilityLabel("Title")
        .onChange(of: m.title) { _, v in log("swiftui-title", "text", v: v) }
      Toggle("Starred", isOn: $m.starred)
        .onChange(of: m.starred) { _, v in log("swiftui-starred", "toggle", v: v ? "on" : "off") }
      Picker("Size", selection: $m.size) {
        ForEach(["Small", "Medium", "Large"], id: \.self) { Text($0) }
      }
      .pickerStyle(.segmented)
      .onChange(of: m.size) { _, v in log("swiftui-size", "pick", v: v) }
      HStack {
        Button("Save Item") {
          log("swiftui-save", "press", v: "\(m.title)|\(m.starred ? "on" : "off")|\(m.size)")
        }
        Button("Tap Me") {
          m.taps += 1
          log("swiftui-tap", "press", v: String(m.taps))
        }
      }
    }
    .padding(20)
    .frame(width: 380, height: 200, alignment: .topLeading)
  }
}

final class Handler: NSObject, NSTextViewDelegate, NSOpenSavePanelDelegate, NSTextFieldDelegate {
  var window: NSWindow!
  var items = 3
  var itemsLabel: NSTextField!
  var exported: String?
  @objc func plain(_ b: NSButton) { log("plain-button", "press") }
  func textViewDidChangeSelection(_ n: Notification) {
    let tv = n.object as! NSTextView
    log("notes-view", "select", v: String(tv.selectedRange().location))
  }
  /// The save panel starts in the trial's own folder and refuses any other.
  @objc func export(_ b: NSButton) {
    log("export", "open")
    let p = NSSavePanel()
    p.directoryURL = saveDir
    p.nameFieldStringValue = "Untitled.txt"
    p.canCreateDirectories = false
    p.delegate = self
    p.beginSheetModal(for: window) { r in
      guard r == .OK, let url = p.url?.standardizedFileURL else { log("export", "cancel"); return }
      guard url.deletingLastPathComponent().path == saveDir.path else {
        log("export", "rejected", v: url.path); return
      }
      try? "exported by quirk-pad\n".write(to: url, atomically: true, encoding: .utf8)
      self.exported = url.lastPathComponent
      log("export", "saved", v: url.lastPathComponent)
    }
  }
  func panel(_ sender: Any, validate url: URL) throws {
    if url.standardizedFileURL.deletingLastPathComponent().path != saveDir.path {
      throw NSError(domain: "quirk-pad", code: 1, userInfo: [NSLocalizedDescriptionKey: "Save into \(saveDir.lastPathComponent) only."])
    }
  }
  @objc func deleteAll(_ b: NSButton) {
    log("delete-all", "press")
    let a = NSAlert()
    a.messageText = "Delete all items?"
    a.informativeText = "This removes every item in the list."
    a.addButton(withTitle: "Delete")
    a.addButton(withTitle: "Cancel")
    a.beginSheetModal(for: window) { r in
      if r == .alertFirstButtonReturn {
        self.items = 0
        self.itemsLabel.stringValue = "Items: 0"
        log("alert", "delete")
      } else {
        log("alert", "cancel")
      }
    }
  }
  @objc func apply(_ b: NSButton) { log("mini-apply", "press", v: code.stringValue) }
  func controlTextDidChange(_ n: Notification) {
    let f = n.object as! NSTextField
    log(f.identifier!.rawValue, "text", v: f.stringValue)
  }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
// The suite opens it through LaunchServices and learns its pid from this file.
if let p = ProcessInfo.processInfo.environment["FIXTURE_PID_FILE"] {
  try? String(ProcessInfo.processInfo.processIdentifier).write(toFile: p, atomically: true, encoding: .utf8)
}
let h = Handler()

let mainMenu = NSMenu()
let appItem = NSMenuItem(); mainMenu.addItem(appItem)
let appMenu = NSMenu(); appMenu.addItem(withTitle: "Quit Quirk Pad", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"); appItem.submenu = appMenu
app.mainMenu = mainMenu

func button(_ title: String, _ id: String, _ action: Selector, _ frame: NSRect, in v: NSView) -> NSButton {
  let b = NSButton(title: title, target: h, action: action)
  b.identifier = NSUserInterfaceItemIdentifier(id)
  b.frame = frame
  v.addSubview(b)
  return b
}

// The AppKit window: a plain button, a text view, the save panel and the alert.
let w = NSWindow(contentRect: NSRect(x: 140, y: 140, width: 460, height: 300), styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
w.title = "AppKit Pad"
h.window = w
let root = Flipped(frame: NSRect(x: 0, y: 0, width: 460, height: 300))
w.contentView = root
_ = button("Plain Button", "plain-button", #selector(Handler.plain(_:)), NSRect(x: 16, y: 16, width: 130, height: 30), in: root)
_ = button("Export…", "export", #selector(Handler.export(_:)), NSRect(x: 156, y: 16, width: 110, height: 30), in: root)
_ = button("Delete All…", "delete-all", #selector(Handler.deleteAll(_:)), NSRect(x: 276, y: 16, width: 130, height: 30), in: root)
let itemsLabel = NSTextField(labelWithString: "Items: 3")
itemsLabel.frame = NSRect(x: 20, y: 56, width: 200, height: 18)
root.addSubview(itemsLabel)
h.itemsLabel = itemsLabel
let notes = NotesView(frame: NSRect(x: 16, y: 84, width: 428, height: 200))
notes.isRichText = false
notes.font = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
notes.string = (0..<10).map { "Line \($0): the quick brown fox jumps over the lazy dog." }.joined(separator: "\n")
notes.setAccessibilityLabel("Notes")
notes.delegate = h
root.addSubview(notes)
w.orderBack(nil)

// The SwiftUI window.
let sw = NSWindow(contentRect: NSRect(x: 620, y: 140, width: 380, height: 200), styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
sw.title = "SwiftUI Pad"
sw.contentView = NSHostingView(rootView: PadView(m: model))
sw.orderBack(nil)

// A minimised window with a field and a button: element actions reach it, pointer events don't.
let mw = NSWindow(contentRect: NSRect(x: 180, y: 480, width: 320, height: 120), styleMask: [.titled, .miniaturizable], backing: .buffered, defer: false)
mw.title = "Minimised Pad"
let mroot = Flipped(frame: NSRect(x: 0, y: 0, width: 320, height: 120))
mw.contentView = mroot
let codeLabel = NSTextField(labelWithString: "Code")
codeLabel.frame = NSRect(x: 16, y: 20, width: 50, height: 22)
mroot.addSubview(codeLabel)
let code = NSTextField(frame: NSRect(x: 70, y: 18, width: 160, height: 24))
code.identifier = NSUserInterfaceItemIdentifier("mini-code")
code.setAccessibilityLabel("Code")
code.delegate = h
mroot.addSubview(code)
_ = button("Apply", "mini-apply", #selector(Handler.apply(_:)), NSRect(x: 16, y: 60, width: 100, height: 30), in: mroot)
mw.orderBack(nil)
var miniMinimised = false
NotificationCenter.default.addObserver(forName: NSWindow.didMiniaturizeNotification, object: mw, queue: nil) { _ in miniMinimised = true; log("mini-window", "minimised") }
NotificationCenter.default.addObserver(forName: NSWindow.didDeminiaturizeNotification, object: mw, queue: nil) { _ in miniMinimised = false; log("mini-window", "restored") }
DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { mw.miniaturize(nil) }

// A window that is shown once, then ordered out: a stand-in for a window on another Space, which
// is off screen with its contents kept.
let ow = NSWindow(contentRect: NSRect(x: 700, y: 480, width: 300, height: 120), styleMask: [.titled], backing: .buffered, defer: false)
ow.title = "Ordered Out Pad"
let oroot = Flipped(frame: NSRect(x: 0, y: 0, width: 300, height: 120))
ow.contentView = oroot
_ = button("Out Button", "out-button", #selector(Handler.plain(_:)), NSRect(x: 16, y: 16, width: 130, height: 30), in: oroot)
let outLabel = NSTextField(labelWithString: "Ordered out content 42")
outLabel.frame = NSRect(x: 16, y: 60, width: 260, height: 18)
oroot.addSubview(outLabel)
ow.orderBack(nil)
DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { ow.orderOut(nil); log("out-window", "ordered-out") }

// SIGUSR1 logs every control's state: a value set through accessibility sends no notice.
signal(SIGUSR1, SIG_IGN)
let snapshot = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
snapshot.setEventHandler {
  let state: [String: String] = [
    "swiftui-title": model.title, "swiftui-starred": model.starred ? "on" : "off",
    "swiftui-size": model.size, "swiftui-taps": String(model.taps),
    "items": String(h.items), "exported": h.exported ?? "",
    "mini-code": code.stringValue, "mini-window": miniMinimised ? "minimised" : "restored",
    "notes-caret": String(notes.selectedRange().location),
    "sheet": w.attachedSheet == nil ? "none" : "open",
  ]
  let data = try! JSONSerialization.data(withJSONObject: state, options: [.sortedKeys])
  log("state", "snapshot", v: String(data: data, encoding: .utf8)!)
}
snapshot.resume()
signal(SIGTERM, SIG_IGN)
let quit = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
quit.setEventHandler { log("app", "quit"); exit(0) }
quit.resume()

NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: nil) { _ in log("app", "active") }
NotificationCenter.default.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: nil) { _ in log("app", "inactive") }
log("app", "ready", v: String(ProcessInfo.processInfo.processIdentifier))
app.run()
