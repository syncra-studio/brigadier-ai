// The computer-use benchmark's target app (docs/COMPUTER-USE-PLAN.md §7).
//
// Plain AppKit, written the way real apps are, so its controls follow the same rules (the
// first-click rule included). Every control writes what happened to it to a log, one JSON
// object a line: the benchmark's ground truth.
//
//   target-range <log path>
//
// It never activates itself: the benchmark drives it in the background.

import AppKit

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

let app = NSApplication.shared
app.setActivationPolicy(.regular)
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

w.orderFront(nil)

// A second window, minimised: background pointer events can't reach it.
let mini = NSWindow(contentRect: NSRect(x: 120, y: 160, width: 300, height: 200), styleMask: [.titled, .miniaturizable], backing: .buffered, defer: false)
mini.title = "Minimised Target"
let miniCanvas = DotCanvas(frame: NSRect(x: 0, y: 0, width: 300, height: 200))
miniCanvas.dots = [.init(id: "mini-dot", rect: NSRect(x: 140, y: 90, width: 20, height: 20), color: .systemRed)]
mini.contentView = miniCanvas
mini.orderFront(nil)
DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { mini.miniaturize(nil) }

NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: nil) { _ in log("app", "active") }
NotificationCenter.default.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: nil) { _ in log("app", "inactive") }
log("app", "ready", v: String(ProcessInfo.processInfo.processIdentifier))
app.run()
