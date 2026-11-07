// One JSON line per on-screen-list window: id, owner, pid, layer, onscreen, alpha, bounds.
// `windows` once, or `windows --pid N`.
import CoreGraphics
import Foundation

var filter: Int? = nil
if let at = CommandLine.arguments.firstIndex(of: "--pid"), at + 1 < CommandLine.arguments.count {
  filter = Int(CommandLine.arguments[at + 1])
}
let all = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
for window in all {
  let pid = window[kCGWindowOwnerPID as String] as? Int ?? -1
  if let filter, filter != pid { continue }
  let bounds = window[kCGWindowBounds as String] as? [String: Double] ?? [:]
  let row: [String: Any] = [
    "id": window[kCGWindowNumber as String] as? Int ?? -1,
    "owner": window[kCGWindowOwnerName as String] as? String ?? "",
    "pid": pid,
    "layer": window[kCGWindowLayer as String] as? Int ?? 0,
    "onscreen": window[kCGWindowIsOnscreen as String] as? Bool ?? false,
    "alpha": window[kCGWindowAlpha as String] as? Double ?? 0,
    "bounds": bounds,
  ]
  let data = try JSONSerialization.data(withJSONObject: row, options: [.sortedKeys])
  print(String(decoding: data, as: UTF8.self))
}
