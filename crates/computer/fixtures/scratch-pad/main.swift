// The computer-use suite's document editor (docs/COMPUTER-USE-PLAN.md §7, Phase 4).
//
// Plain AppKit, the way document apps are written: an NSTextView with the system's text
// substitutions, undo, a find bar, an edited state, and a Save that is enabled only after an edit
// and writes the file. It stands in for a real editor in tests that must not depend on the
// user's Spaces or on a sandboxed app's launch.
//
//   scratch-pad <file>
//
// It never activates itself: it is opened in the background (`open -n -g`) and driven there.

import AppKit

let path = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "/tmp/scratch-pad.txt"
let url = URL(fileURLWithPath: path)

final class Editor: NSObject, NSTextViewDelegate, NSMenuItemValidation {
  var window: NSWindow!
  var text: NSTextView!
  func textDidChange(_ n: Notification) { window.isDocumentEdited = true }
  @objc func save(_ s: Any?) {
    try? text.string.write(to: url, atomically: true, encoding: .utf8)
    window.isDocumentEdited = false
  }
  func validateMenuItem(_ item: NSMenuItem) -> Bool {
    item.action == #selector(save(_:)) ? window.isDocumentEdited : true
  }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
// The suite opens it through LaunchServices and learns its pid from this file.
if let p = ProcessInfo.processInfo.environment["FIXTURE_PID_FILE"] {
  try? String(ProcessInfo.processInfo.processIdentifier).write(toFile: p, atomically: true, encoding: .utf8)
}
let ed = Editor()

let mainMenu = NSMenu()
let appItem = NSMenuItem(); mainMenu.addItem(appItem)
let appMenu = NSMenu(); appMenu.addItem(withTitle: "Quit Scratch Pad", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"); appItem.submenu = appMenu
let fileItem = NSMenuItem(); mainMenu.addItem(fileItem)
let fileMenu = NSMenu(title: "File"); fileItem.submenu = fileMenu
let saveItem = NSMenuItem(title: "Save", action: #selector(Editor.save(_:)), keyEquivalent: "s"); saveItem.target = ed; fileMenu.addItem(saveItem)
let editItem = NSMenuItem(); mainMenu.addItem(editItem)
let editMenu = NSMenu(title: "Edit"); editItem.submenu = editMenu
editMenu.addItem(withTitle: "Undo", action: Selector(("undo:")), keyEquivalent: "z")
editMenu.addItem(withTitle: "Redo", action: Selector(("redo:")), keyEquivalent: "Z")
editMenu.addItem(.separator())
editMenu.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
editMenu.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
editMenu.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
editMenu.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
editMenu.addItem(.separator())
let findItem = NSMenuItem(title: "Find", action: nil, keyEquivalent: ""); editMenu.addItem(findItem)
let findMenu = NSMenu(title: "Find"); findItem.submenu = findMenu
func findAction(_ title: String, _ tag: NSTextFinder.Action, _ key: String, _ mods: NSEvent.ModifierFlags = .command) {
  let it = NSMenuItem(title: title, action: #selector(NSTextView.performFindPanelAction(_:)), keyEquivalent: key)
  it.tag = tag.rawValue; it.keyEquivalentModifierMask = mods; findMenu.addItem(it)
}
findAction("Find…", .showFindInterface, "f")
findAction("Find and Replace…", .showReplaceInterface, "f", [.command, .option])
findAction("Find Next", .nextMatch, "g")
app.mainMenu = mainMenu

let w = NSWindow(contentRect: NSRect(x: 160, y: 160, width: 640, height: 420), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
w.title = url.lastPathComponent
w.representedURL = url
ed.window = w
let scroll = NSTextView.scrollableTextView()
scroll.frame = w.contentView!.bounds
scroll.autoresizingMask = [.width, .height]
let tv = scroll.documentView as! NSTextView
tv.isRichText = false
tv.allowsUndo = true
tv.usesFindBar = true
tv.isIncrementalSearchingEnabled = true
tv.font = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
tv.string = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
tv.delegate = ed
tv.setAccessibilityLabel("Document")
ed.text = tv
w.contentView!.addSubview(scroll)
w.makeFirstResponder(tv)
w.orderFront(nil)
app.run()
