// Moves windows between Spaces through SkyLight's private calls. Nothing here changes which Space the
// user is on. `spaces list`, `spaces create`, `spaces move <windowId> <spaceId>`, `spaces where <windowId>`,
// `spaces destroy <spaceId>`.
import CoreGraphics
import Foundation

typealias Conn = Int32
let sky = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_LAZY)
func fn<T>(_ name: String, _ type: T.Type) -> T? {
  guard let symbol = dlsym(sky, name) else { return nil }
  return unsafeBitCast(symbol, to: type)
}
let mainConn = fn("SLSMainConnectionID", (@convention(c) () -> Conn).self)!
let conn = mainConn()
let copyManaged = fn("SLSCopyManagedDisplaySpaces", (@convention(c) (Conn) -> CFArray).self)
let activeSpace = fn("SLSGetActiveSpace", (@convention(c) (Conn) -> UInt64).self)
let spaceCreate = fn("SLSSpaceCreate", (@convention(c) (Conn, Int32, CFDictionary?) -> UInt64).self)
let spaceDestroy = fn("SLSSpaceDestroy", (@convention(c) (Conn, UInt64) -> Void).self)
let addTo = fn("SLSAddWindowsToSpaces", (@convention(c) (Conn, CFArray, CFArray) -> Void).self)
let removeFrom = fn("SLSRemoveWindowsFromSpaces", (@convention(c) (Conn, CFArray, CFArray) -> Void).self)
let moveManaged = fn("SLSMoveWindowsToManagedSpace", (@convention(c) (Conn, CFArray, UInt64) -> Void).self)
let copySpaces = fn("SLSCopySpacesForWindows", (@convention(c) (Conn, Int32, CFArray) -> CFArray).self)

let a = CommandLine.arguments
func ids(_ value: UInt64) -> CFArray { [NSNumber(value: value)] as CFArray }
switch a.count > 1 ? a[1] : "" {
case "list":
  let active = activeSpace?(conn) ?? 0
  var out: [UInt64] = []
  if let list = copyManaged?(conn) as? [[String: Any]] {
    for display in list {
      for space in display["Spaces"] as? [[String: Any]] ?? [] {
        if let id = space["ManagedSpaceID"] as? UInt64 ?? (space["id64"] as? UInt64) { out.append(id) }
      }
    }
  }
  print("{\"active\":\(active),\"managed\":\(out)}")
case "where":
  let result = copySpaces?(conn, 0x7, ids(UInt64(a[2])!)) as? [UInt64] ?? []
  print("{\"spaces\":\(result)}")
case "create":
  let id = spaceCreate?(conn, 1, nil) ?? 0
  print("{\"space\":\(id)}")
case "destroy":
  spaceDestroy?(conn, UInt64(a[2])!)
  print("{\"ok\":true}")
case "add":
  addTo?(conn, ids(UInt64(a[2])!), ids(UInt64(a[3])!))
  print("{\"ok\":true}")
case "remove":
  removeFrom?(conn, ids(UInt64(a[2])!), ids(UInt64(a[3])!))
  print("{\"ok\":true}")
case "move":
  moveManaged?(conn, ids(UInt64(a[2])!), UInt64(a[3])!)
  print("{\"ok\":true}")
default:
  fputs("usage: spaces list|where|create|destroy|add|remove|move\n", stderr)
  exit(64)
}
