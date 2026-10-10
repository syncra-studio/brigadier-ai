// The computer-use suite's web view (docs/COMPUTER-USE-PLAN.md §8, Phase 5, stream A).
//
// A window holding one WKWebView, the engine Safari and many apps embed, on a page the suite
// serves. It stands in for Safari, which tests must not drive: the user's own tabs live there.
// The page is read through its accessibility web area, as any browser without a debugging port.
//
//   web-view <url>
//
// It never activates itself: it is opened in the background (`open -n -g`) and driven there, its
// window ordered behind every other window.

import AppKit
import WebKit

let address = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "about:blank"

final class Titles: NSObject, WKNavigationDelegate {
  var window: NSWindow!
  func webView(_ v: WKWebView, didFinish n: WKNavigation!) {
    window.title = "Web View: " + (v.title ?? "")
  }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
// The suite opens it through LaunchServices and learns its pid from this file.
if let p = ProcessInfo.processInfo.environment["FIXTURE_PID_FILE"] {
  try? String(ProcessInfo.processInfo.processIdentifier).write(toFile: p, atomically: true, encoding: .utf8)
}

let mainMenu = NSMenu()
let appItem = NSMenuItem()
mainMenu.addItem(appItem)
let appMenu = NSMenu()
appMenu.addItem(withTitle: "Quit web-view", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
appItem.submenu = appMenu
app.mainMenu = mainMenu

let w = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 900, height: 700), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
w.title = "Web View"
w.isReleasedWhenClosed = false
let config = WKWebViewConfiguration()
let view = WKWebView(frame: w.contentView!.bounds, configuration: config)
view.autoresizingMask = [.width, .height]
let titles = Titles()
titles.window = w
view.navigationDelegate = titles
w.contentView!.addSubview(view)
if let url = URL(string: address) { view.load(URLRequest(url: url)) }
w.orderBack(nil)

final class Quit: NSObject, NSApplicationDelegate {
  func applicationShouldTerminateAfterLastWindowClosed(_ s: NSApplication) -> Bool { true }
}
let quit = Quit()
app.delegate = quit
app.run()
