// Replaces this process with argv[1...] as its own "responsible" process for macOS privacy checks,
// so a permission the app asks for is judged against the app and not whatever started this tool.
// Keeps every inherited descriptor, which --remote-debugging-pipe needs (3 and 4).
import Darwin
import Foundation

typealias Disclaim = @convention(c) (UnsafeMutablePointer<posix_spawnattr_t?>, Int32) -> Int32

let args = Array(CommandLine.arguments.dropFirst())
guard let program = args.first else {
  fputs("usage: spawn-disclaimed <program> [args...]\n", stderr)
  exit(64)
}
var attr: posix_spawnattr_t?
posix_spawnattr_init(&attr)
posix_spawnattr_setflags(&attr, Int16(POSIX_SPAWN_SETEXEC))
guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "responsibility_spawnattrs_setdisclaim") else {
  fputs("responsibility_spawnattrs_setdisclaim is missing\n", stderr)
  exit(70)
}
_ = unsafeBitCast(symbol, to: Disclaim.self)(&attr, 1)
var cargs: [UnsafeMutablePointer<CChar>?] = args.map { strdup($0) }
cargs.append(nil)
var pid: pid_t = 0
let code = posix_spawn(&pid, program, nil, &attr, cargs, environ)
fputs("posix_spawn failed: \(String(cString: strerror(code)))\n", stderr)
exit(71)
