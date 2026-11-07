// A plain window of another process, for "behind other windows": never activates, takes no mouse.
// cover <x> <y-from-bottom> <width> <height> <seconds, 0 for until killed> [#hex level]
import AppKit

let a = CommandLine.arguments
guard a.count >= 6 else { fputs("usage: cover x y w h seconds [level]\n", stderr); exit(64) }
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
let window = NSWindow(
  contentRect: NSRect(x: Double(a[1])!, y: Double(a[2])!, width: Double(a[3])!, height: Double(a[4])!),
  styleMask: [.borderless], backing: .buffered, defer: false)
window.isOpaque = true
window.backgroundColor = .black
window.level = a.count > 6 && a[6] == "alert" ? .modalPanel : .statusBar
window.ignoresMouseEvents = true
window.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
window.orderFrontRegardless()
if let seconds = Double(a[5]), seconds > 0 {
  DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { exit(0) }
}
print(window.windowNumber); fflush(stdout)
app.run()
