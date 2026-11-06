// System-side watcher for one app. Polls NSWorkspace and CGWindowList (no private API), one JSON line
// per change: the front app, whether the watched app is active or hidden, its windows, and any window
// from a process that exists to show a system dialog. Pairs with `log stream` in watch.mjs.
// watcher <pid> [interval-ms]
import AppKit
import CoreGraphics
import Foundation

let pid = Int32(CommandLine.arguments[1])!
let intervalMs = CommandLine.arguments.count > 2 ? Int(CommandLine.arguments[2])! : 200
let dialogOwners: Set<String> = [
  "UserNotificationCenter", "SecurityAgent", "CoreServicesUIAgent", "universalAccessAuthWarn",
  "AXVisualSupportAgent", "coreautha", "LocalAuthenticationRemoteService", "TCC", "tccd",
]
func emit(_ row: [String: Any]) {
  var value = row
  value["at"] = ISO8601DateFormatter().string(from: Date())
  if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) {
    FileHandle.standardOutput.write(data + Data("\n".utf8))
  }
}
var lastFront = ""
var lastActive: Bool? = nil
var lastDialogs = ""
var lastWindows = ""
var lastOnScreen = ""
func tick() {
  let front = NSWorkspace.shared.frontmostApplication
  let frontKey = "\(front?.localizedName ?? "?")#\(front?.processIdentifier ?? -1)"
  if frontKey != lastFront {
    lastFront = frontKey
    emit(["kind": "front", "app": front?.localizedName ?? "?", "pid": front?.processIdentifier ?? -1, "isWatched": front?.processIdentifier == pid])
  }
  let watched = NSRunningApplication(processIdentifier: pid)
  let active = watched?.isActive ?? false
  if active != lastActive {
    lastActive = active
    emit(["kind": "active", "active": active, "hidden": watched?.isHidden ?? false, "alive": watched != nil])
  }
  let all = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
  var dialogs: [String] = []
  var mine: [String] = []
  var onUserScreen: [String] = []
  for window in all {
    let owner = window[kCGWindowOwnerName as String] as? String ?? ""
    let ownerPid = window[kCGWindowOwnerPID as String] as? Int32 ?? -1
    let layer = window[kCGWindowLayer as String] as? Int ?? 0
    let id = window[kCGWindowNumber as String] as? Int ?? -1
    if dialogOwners.contains(owner) { dialogs.append("\(owner)#\(id)@\(layer)") }
    if ownerPid == pid {
      mine.append("\(id)@\(layer)")
      // The app must keep off the user's screen in unfocused runs, but for a thin strip at its edge.
      if let frame = window[kCGWindowBounds as String] as? NSDictionary, let rect = CGRect(dictionaryRepresentation: frame) {
        let seen = rect.intersection(CGDisplayBounds(CGMainDisplayID()))
        if seen.width > 70 && seen.height > 70 { onUserScreen.append("\(id)=\(Int(seen.width))x\(Int(seen.height))") }
      }
    }
  }
  let dialogKey = dialogs.sorted().joined(separator: ",")
  if dialogKey != lastDialogs {
    lastDialogs = dialogKey
    emit(["kind": "dialog-windows", "windows": dialogs])
  }
  let screenKey = onUserScreen.sorted().joined(separator: ",")
  if screenKey != lastOnScreen {
    lastOnScreen = screenKey
    if !onUserScreen.isEmpty { emit(["kind": "dev-window-on-user-screen", "windows": onUserScreen]) }
  }
  let windowKey = mine.sorted().joined(separator: ",")
  if windowKey != lastWindows {
    lastWindows = windowKey
    emit(["kind": "dev-windows", "windows": mine])
  }
}
tick()
Timer.scheduledTimer(withTimeInterval: Double(intervalMs) / 1000, repeats: true) { _ in tick() }
RunLoop.main.run()
