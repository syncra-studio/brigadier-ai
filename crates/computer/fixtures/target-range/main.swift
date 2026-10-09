// The computer-use benchmark's target app (docs/COMPUTER-USE-PLAN.md §7).
//
// Plain AppKit, written the way real apps are, so its controls follow the same rules (the
// first-click rule included). Every control writes what happened to it to a log, one JSON
// object a line: the benchmark's ground truth.
//
//   target-range <log path>
//
// It never activates itself: it is opened in the background (`open -n -g`) and driven there, its
// windows ordered behind every other window, so it never shows over the user's work and a Space
// switch never hands it the front.

import AppKit

/// `FIXTURE_SCREEN=<n>`: the window opens on display n (`NSScreen.screens` order), near its top
/// left, for the several-displays checks.
func placeOnScreen(_ win: NSWindow) {
  guard let n = ProcessInfo.processInfo.environment["FIXTURE_SCREEN"].flatMap({ Int($0) }),
        n < NSScreen.screens.count else { return }
  let v = NSScreen.screens[n].visibleFrame
  win.setFrameTopLeftPoint(NSPoint(x: v.minX + 40, y: v.maxY - 40))
}

let logPath = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "/tmp/target-range.log"
FileManager.default.createFile(atPath: logPath, contents: nil)
let logHandle = FileHandle(forWritingAtPath: logPath)!

func log(_ id: String, _ ev: String, x: Double? = nil, y: Double? = nil, v: String? = nil) {
  var o: [String: Any] = ["t": Date().timeIntervalSince1970 * 1000, "id": id, "ev": ev]
  if let x { o["x"] = x }
  if let y { o["y"] = y }
  if let v { o["v"] = v }
  let data = try! JSONSerialization.data(withJSONObject: o, options: [.sortedKeys])
  logHandle.write(data + Data([0x0a]))
  if let line = lastActionText(id, ev, v) { lastAction?.stringValue = "Last action: " + line }
}

// What a person sees after a press, a menu pick or a click on a dot: real apps show the effect,
// so a worker can check it on screen rather than in the log.
var lastAction: NSTextField?
var presses: [String: Int] = [:]
let dotNames = ["dot-8": "red", "dot-12": "blue", "dot-16": "green", "dot-24": "orange"]
func lastActionText(_ id: String, _ ev: String, _ v: String?) -> String? {
  switch ev {
  case "press" where id.hasPrefix("button-"):
    presses[id, default: 0] += 1
    let n = presses[id]!
    return "pressed Button \(id.dropFirst(7)) pt (\(n) \(n == 1 ? "time" : "times") in all)"
  case "pick" where id == "menu": return "picked \(v ?? "") in the Targets menu"
  case "down" where id.hasPrefix("dot-"): return "clicked the \(dotNames[id] ?? "purple") dot"
  case "down" where id == "canvas": return "clicked the canvas, on no dot"
  default: return nil
  }
}

final class Flipped: NSView { override var isFlipped: Bool { true } }

/// Dots with no accessibility at all. It refuses the first click, like most custom views.
final class DotCanvas: NSView {
  struct Dot { let id: String; let rect: NSRect; let color: NSColor }
  var dots: [Dot] = []
  override var isFlipped: Bool { true }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { false }
  override func isAccessibilityElement() -> Bool { false }
  override func accessibilityChildren() -> [Any]? { [] }
  override func draw(_ r: NSRect) {
    NSColor(white: 0.96, alpha: 1).setFill(); bounds.fill()
    for d in dots { d.color.setFill(); NSBezierPath(ovalIn: d.rect).fill() }
  }
  func report(_ ev: String, _ e: NSEvent) {
    let p = convert(e.locationInWindow, from: nil)
    let hit = dots.first { NSBezierPath(ovalIn: $0.rect).contains(p) }?.id ?? "canvas"
    log(hit, ev, x: Double(p.x), y: Double(p.y))
  }
  override func mouseDown(with e: NSEvent) { report("down", e) }
  override func mouseDragged(with e: NSEvent) { report("dragged", e) }
  override func mouseUp(with e: NSEvent) { report("up", e) }
}

final class Handler: NSObject, NSTextFieldDelegate, NSTableViewDataSource, NSTableViewDelegate, NSTabViewDelegate {
  var window: NSWindow!
  var sheet: NSWindow?
  @objc func pressed(_ b: NSButton) { log(b.identifier!.rawValue, "press") }
  @objc func toggled(_ b: NSButton) { log(b.identifier!.rawValue, "toggle", v: b.state == .on ? "on" : "off") }
  @objc func slid(_ s: NSSlider) { log("slider", "value", v: String(s.integerValue)) }
  @objc func stepped(_ s: NSStepper) { log("stepper", "value", v: String(s.integerValue)) }
  @objc func picked(_ p: NSPopUpButton) { log("popup", "pick", v: p.titleOfSelectedItem ?? "") }
  @objc func menuPicked(_ m: NSMenuItem) { log("menu", "pick", v: m.title) }
  @objc func openSheet(_ b: NSButton) {
    log("open-sheet", "press")
    let s = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 240, height: 90), styleMask: [.titled], backing: .buffered, defer: false)
    let close = NSButton(title: "Close Sheet", target: self, action: #selector(closeSheet(_:)))
    close.identifier = NSUserInterfaceItemIdentifier("close-sheet")
    close.frame = NSRect(x: 60, y: 30, width: 120, height: 30)
    s.contentView!.addSubview(close)
    sheet = s
    window.beginSheet(s) { _ in log("sheet", "closed") }
    log("sheet", "opened")
  }
  @objc func closeSheet(_ b: NSButton) { log("close-sheet", "press"); if let s = sheet { window.endSheet(s) } }
  func controlTextDidChange(_ n: Notification) {
    let f = n.object as! NSTextField
    let id = f.identifier!.rawValue
    // The secure field's value is never logged, only its length.
    log(id, "text", v: id == "password" ? String(f.stringValue.count) : f.stringValue)
  }
  func numberOfRows(in t: NSTableView) -> Int { 200 }
  func tableView(_ t: NSTableView, objectValueFor c: NSTableColumn?, row: Int) -> Any? { "Row \(row)" }
  func tableViewSelectionDidChange(_ n: Notification) { log("table", "select", v: String((n.object as! NSTableView).selectedRow)) }
  func tabView(_ t: NSTabView, didSelect item: NSTabViewItem?) { log("tabs", "select", v: item?.label ?? "") }
  @objc func scrolled(_ n: Notification) { log("table-scroll", "scroll", v: String(Int((n.object as! NSClipView).bounds.origin.y))) }
}

// Grounding boards (the P3 trials): `target-range <log> --grounding <size> [--seed n] [--boards n]`.
// Each board draws five numbered markers and five lettered decoys of one size, at seeded random
// places, on a canvas with no accessibility that refuses the first click. A click is logged with
// the marker it hit (`m3`, `dC`) or `canvas`, and the board it was on. "Next board" deals the next.
struct Rng {
  var s: UInt64
  mutating func next() -> UInt64 { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s }
  mutating func unit() -> CGFloat { CGFloat(next() % 1_000_000) / 1_000_000 }
}

final class GroundingCanvas: NSView {
  struct Marker { let id: String; let label: String; let rect: NSRect; let color: NSColor }
  var markers: [Marker] = []
  var board = 0
  override var isFlipped: Bool { true }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { false }
  override func isAccessibilityElement() -> Bool { false }
  override func accessibilityChildren() -> [Any]? { [] }
  override func draw(_ r: NSRect) {
    NSColor(white: 0.97, alpha: 1).setFill(); bounds.fill()
    let font = NSFont.systemFont(ofSize: 12, weight: .semibold)
    for m in markers {
      m.color.setFill(); NSBezierPath(ovalIn: m.rect).fill()
      (m.label as NSString).draw(at: NSPoint(x: m.rect.maxX + 3, y: m.rect.midY - 8), withAttributes: [.font: font, .foregroundColor: NSColor.black])
    }
  }
  func deal(size: CGFloat, rng: inout Rng) {
    let colors: [NSColor] = [.systemRed, .systemBlue, .systemGreen, .systemOrange, .systemPurple]
    var placed: [NSRect] = []
    var out: [Marker] = []
    let labels = ["1", "2", "3", "4", "5", "A", "B", "C", "D", "E"]
    for (i, label) in labels.enumerated() {
      // Apart enough that a label never sits on another marker; near enough to need care.
      var rect = NSRect.zero
      for _ in 0..<500 {
        let x = 20 + rng.unit() * (bounds.width - 60 - size)
        let y = 20 + rng.unit() * (bounds.height - 40 - size)
        rect = NSRect(x: x, y: y, width: size, height: size)
        if !placed.contains(where: { $0.insetBy(dx: -28, dy: -14).intersects(rect.insetBy(dx: -4, dy: -4)) }) { break }
      }
      placed.append(rect)
      out.append(.init(id: i < 5 ? "m\(label)" : "d\(label)", label: label, rect: rect, color: colors[Int(rng.next() % 5)]))
    }
    markers = out
    board += 1
    let layout = out.map { "\($0.id):\(Int($0.rect.midX)),\(Int($0.rect.midY))" }.joined(separator: " ")
    log("board", "layout", v: "\(board) \(layout)")
    needsDisplay = true
  }
  func report(_ ev: String, _ e: NSEvent) {
    let p = convert(e.locationInWindow, from: nil)
    let hit = markers.first { NSBezierPath(ovalIn: $0.rect).contains(p) }?.id ?? "canvas"
    log(hit, ev, x: Double(p.x), y: Double(p.y), v: String(board))
  }
  override func mouseDown(with e: NSEvent) { report("down", e) }
  override func mouseUp(with e: NSEvent) { report("up", e) }
}

final class GroundingHandler: NSObject {
  let canvas: GroundingCanvas
  let size: CGFloat
  let boards: Int
  var rng: Rng
  let status: NSTextField
  init(canvas: GroundingCanvas, size: CGFloat, seed: UInt64, boards: Int, status: NSTextField) {
    self.canvas = canvas; self.size = size; self.boards = boards; self.status = status
    rng = Rng(s: seed == 0 ? 0x9E37_79B9_7F4A_7C15 : seed)
  }
  func showBoard() { status.stringValue = "Board \(canvas.board) of \(boards)" }
  @objc func next(_ b: NSButton) {
    log("next-board", "press", v: String(canvas.board))
    if canvas.board >= boards {
      canvas.markers = []; canvas.needsDisplay = true
      status.stringValue = "All boards done"
      log("board", "done")
      return
    }
    canvas.deal(size: size, rng: &rng); showBoard()
  }
}

func argValue(_ name: String) -> String? {
  guard let i = CommandLine.arguments.firstIndex(of: name), i + 1 < CommandLine.arguments.count else { return nil }
  return CommandLine.arguments[i + 1]
}

func runGrounding(size: CGFloat) -> Never {
  let gw = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 700, height: 520), styleMask: [.titled, .closable], backing: .buffered, defer: false)
  gw.title = "Grounding \(Int(size)) pt"
  let groot = Flipped(frame: NSRect(x: 0, y: 0, width: 700, height: 520))
  gw.contentView = groot
  let canvas = GroundingCanvas(frame: NSRect(x: 10, y: 50, width: 680, height: 460))
  groot.addSubview(canvas)
  let status = NSTextField(labelWithString: "")
  status.frame = NSRect(x: 150, y: 16, width: 300, height: 18)
  status.identifier = NSUserInterfaceItemIdentifier("board-status")
  groot.addSubview(status)
  let gh = GroundingHandler(canvas: canvas, size: size, seed: UInt64(argValue("--seed") ?? "1") ?? 1, boards: Int(argValue("--boards") ?? "10") ?? 10, status: status)
  let next = NSButton(title: "Next board", target: gh, action: #selector(GroundingHandler.next(_:)))
  next.identifier = NSUserInterfaceItemIdentifier("next-board")
  next.frame = NSRect(x: 10, y: 10, width: 120, height: 30)
  groot.addSubview(next)
  canvas.deal(size: size, rng: &gh.rng); gh.showBoard()
  placeOnScreen(gw)
  gw.orderBack(nil)
  log("app", "ready", v: String(ProcessInfo.processInfo.processIdentifier))
    withExtendedLifetime(gh) { NSApplication.shared.run() }
  exit(0)
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
// The suite opens it through LaunchServices and learns its pid from this file.
if let p = ProcessInfo.processInfo.environment["FIXTURE_PID_FILE"] {
  try? String(ProcessInfo.processInfo.processIdentifier).write(toFile: p, atomically: true, encoding: .utf8)
}
if let s = argValue("--grounding"), let size = Double(s) { runGrounding(size: CGFloat(size)) }
let h = Handler()

// Menu bar: Targets › Level 1 › Level 2 › Pick Me 1–3, and Targets › Plain Item.
let mainMenu = NSMenu()
let appItem = NSMenuItem(); mainMenu.addItem(appItem)
let appMenu = NSMenu(); appMenu.addItem(withTitle: "Quit Target Range", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"); appItem.submenu = appMenu
let targetsItem = NSMenuItem(); mainMenu.addItem(targetsItem)
let targets = NSMenu(title: "Targets"); targetsItem.submenu = targets
let plain = NSMenuItem(title: "Plain Item", action: #selector(Handler.menuPicked(_:)), keyEquivalent: ""); plain.target = h; targets.addItem(plain)
let l1 = NSMenuItem(title: "Level 1", action: nil, keyEquivalent: ""); targets.addItem(l1)
let m1 = NSMenu(title: "Level 1"); l1.submenu = m1
let l2 = NSMenuItem(title: "Level 2", action: nil, keyEquivalent: ""); m1.addItem(l2)
let m2 = NSMenu(title: "Level 2"); l2.submenu = m2
for i in 1...3 { let it = NSMenuItem(title: "Pick Me \(i)", action: #selector(Handler.menuPicked(_:)), keyEquivalent: ""); it.target = h; m2.addItem(it) }
app.mainMenu = mainMenu

let w = NSWindow(contentRect: NSRect(x: 80, y: 120, width: 900, height: 600), styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
w.title = "Target Range"
h.window = w
let root = Flipped(frame: NSRect(x: 0, y: 0, width: 900, height: 600))
w.contentView = root

func label(_ s: String, _ x: CGFloat, _ y: CGFloat) {
  let l = NSTextField(labelWithString: s); l.frame = NSRect(x: x, y: y, width: 200, height: 16); root.addSubview(l)
}

// Buttons and checkboxes at 8, 12, 16 and 24 pt.
label("Buttons", 20, 12)
label("Checkboxes", 20, 62)
var x: CGFloat = 110
var checks: [NSButton] = []
for size in [8, 12, 16, 24] {
  let s = CGFloat(size)
  let b = NSButton(title: "", target: h, action: #selector(Handler.pressed(_:)))
  b.bezelStyle = .smallSquare; b.isBordered = true
  b.frame = NSRect(x: x, y: 10 + (24 - s) / 2, width: s, height: s)
  b.identifier = NSUserInterfaceItemIdentifier("button-\(size)")
  b.setAccessibilityLabel("Button \(size) pt")
  root.addSubview(b)
  let c = NSButton(checkboxWithTitle: "", target: h, action: #selector(Handler.toggled(_:)))
  c.frame = NSRect(x: x, y: 60 + (24 - s) / 2, width: s, height: s)
  c.identifier = NSUserInterfaceItemIdentifier("check-\(size)")
  c.setAccessibilityLabel("Check \(size) pt")
  root.addSubview(c)
  checks.append(c)
  x += 60
}

// Text, password, slider, stepper, pop-up.
label("Name", 20, 112)
let name = NSTextField(frame: NSRect(x: 110, y: 108, width: 220, height: 22))
name.identifier = NSUserInterfaceItemIdentifier("name"); name.placeholderString = "Name"; name.delegate = h
root.addSubview(name)
label("Password", 20, 142)
let pw = NSSecureTextField(frame: NSRect(x: 110, y: 138, width: 220, height: 22))
pw.identifier = NSUserInterfaceItemIdentifier("password"); pw.placeholderString = "Password"; pw.delegate = h
root.addSubview(pw)
label("Notes", 20, 172)
let notes = NSTextField(frame: NSRect(x: 110, y: 168, width: 220, height: 22))
notes.identifier = NSUserInterfaceItemIdentifier("notes"); notes.placeholderString = "Notes"; notes.delegate = h
root.addSubview(notes)
let slider = NSSlider(value: 50, minValue: 0, maxValue: 100, target: h, action: #selector(Handler.slid(_:)))
slider.frame = NSRect(x: 110, y: 200, width: 220, height: 24); slider.setAccessibilityLabel("Level")
root.addSubview(slider)
let stepper = NSStepper(frame: NSRect(x: 110, y: 232, width: 20, height: 28))
stepper.minValue = 0; stepper.maxValue = 1000; stepper.integerValue = 5; stepper.target = h; stepper.action = #selector(Handler.stepped(_:))
stepper.setAccessibilityLabel("Count")
root.addSubview(stepper)
let popup = NSPopUpButton(frame: NSRect(x: 150, y: 232, width: 140, height: 26), pullsDown: false)
popup.addItems(withTitles: ["Alpha", "Beta", "Gamma", "Delta"]); popup.target = h; popup.action = #selector(Handler.picked(_:))
popup.setAccessibilityLabel("Letter")
root.addSubview(popup)
let sheetButton = NSButton(title: "Open Sheet", target: h, action: #selector(Handler.openSheet(_:)))
sheetButton.identifier = NSUserInterfaceItemIdentifier("open-sheet")
sheetButton.frame = NSRect(x: 110, y: 268, width: 120, height: 30)
root.addSubview(sheetButton)

// Tabs.
let tabs = NSTabView(frame: NSRect(x: 20, y: 310, width: 320, height: 110))
for t in ["First", "Second"] { let it = NSTabViewItem(identifier: t); it.label = t; tabs.addTabViewItem(it) }
tabs.delegate = h
root.addSubview(tabs)

// A table of 200 rows.
let scroll = NSScrollView(frame: NSRect(x: 360, y: 10, width: 200, height: 410))
let table = NSTableView()
let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("row")); col.title = "Rows"; col.width = 180
table.addTableColumn(col); table.dataSource = h; table.delegate = h
scroll.documentView = table; scroll.hasVerticalScroller = true
scroll.contentView.postsBoundsChangedNotifications = true
NotificationCenter.default.addObserver(h, selector: #selector(Handler.scrolled(_:)), name: NSView.boundsDidChangeNotification, object: scroll.contentView)
root.addSubview(scroll)

// The dot canvas: no accessibility, refuses the first click.
let canvas = DotCanvas(frame: NSRect(x: 580, y: 10, width: 300, height: 410))
var dy: CGFloat = 30
for (i, size) in [8, 12, 16, 24].enumerated() {
  let s = CGFloat(size)
  canvas.dots.append(.init(id: "dot-\(size)", rect: NSRect(x: 60 - s / 2, y: dy - s / 2, width: s, height: s), color: [NSColor.systemRed, .systemBlue, .systemGreen, .systemOrange][i]))
  canvas.dots.append(.init(id: "dot-\(size)-b", rect: NSRect(x: 200 - s / 2, y: dy + 40 - s / 2, width: s, height: s), color: .systemPurple))
  dy += 90
}
root.addSubview(canvas)

let last = NSTextField(labelWithString: "Last action: none")
last.frame = NSRect(x: 20, y: 440, width: 560, height: 18)
last.identifier = NSUserInterfaceItemIdentifier("last-action")
root.addSubview(last)
lastAction = last

placeOnScreen(w)
w.orderBack(nil)

// A second window, minimised: background pointer events can't reach it.
let mini = NSWindow(contentRect: NSRect(x: 120, y: 160, width: 300, height: 200), styleMask: [.titled, .miniaturizable], backing: .buffered, defer: false)
mini.title = "Minimised Target"
let miniCanvas = DotCanvas(frame: NSRect(x: 0, y: 0, width: 300, height: 200))
miniCanvas.dots = [.init(id: "mini-dot", rect: NSRect(x: 140, y: 90, width: 20, height: 20), color: .systemRed)]
mini.contentView = miniCanvas
mini.orderBack(nil)
DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { mini.miniaturize(nil) }

// SIGUSR1 logs every control's state: the end state a checker reads, since a value set through
// accessibility sends a control no action or change notice to log.
signal(SIGUSR1, SIG_IGN)
let snapshot = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
snapshot.setEventHandler {
  var state: [String: String] = [
    "name": name.stringValue, "notes": notes.stringValue, "password": String(pw.stringValue.count),
    "slider": String(slider.integerValue), "stepper": String(stepper.integerValue),
    "popup": popup.titleOfSelectedItem ?? "", "tabs": tabs.selectedTabViewItem?.label ?? "",
    "table": String(table.selectedRow), "sheet": h.sheet?.isVisible == true ? "open" : "closed",
  ]
  for c in checks { state[c.identifier!.rawValue] = c.state == .on ? "on" : "off" }
  let data = try! JSONSerialization.data(withJSONObject: state, options: [.sortedKeys])
  log("state", "snapshot", v: String(data: data, encoding: .utf8)!)
}
snapshot.resume()

NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: nil) { _ in log("app", "active") }
NotificationCenter.default.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: nil) { _ in log("app", "inactive") }
log("app", "ready", v: String(ProcessInfo.processInfo.processIdentifier))
app.run()
