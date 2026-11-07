// What the OS says about a process's sandbox, read from outside it. Chromium's own word (app.getAppMetrics) is
// printed beside it, never used in its place. The browser process is the negative control: it must read as open.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));

function macReader(folder) {
  const tool = path.join(folder, "sandbox-state");
  execFileSync("cc", [path.join(here, "native/sandbox-state.c"), "-o", tool]);
  return (pid) => {
    const value = execFileSync(tool, [String(pid)], { encoding: "utf8" }).trim();
    const args = execFileSync("ps", ["-o", "command=", "-p", String(pid)], { encoding: "utf8" });
    return { sandboxed: value === "1", detail: `sandbox_check ${value}${args.includes("--seatbelt-client") ? ", seatbelt client" : ""}` };
  };
}

function linuxReader() {
  return (pid) => {
    const status = fs.readFileSync(`/proc/${pid}/status`, "utf8");
    const seccomp = status.match(/^Seccomp:\s+(\d)/m)?.[1];
    const nspids = status.match(/^NSpid:\s+(.+)$/m)?.[1].trim().split(/\s+/) ?? [];
    return { sandboxed: seccomp === "2" && nspids.length > 1, detail: `Seccomp ${seccomp}, pid namespaces ${nspids.length}` };
  };
}

// A token's integrity level: a Windows renderer runs at Untrusted (0) or Low (4096), the browser at Medium or above.
const WINDOWS_READER = String.raw`
param([int]$target)
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public static class T {
  [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr OpenProcess(uint a, bool i, int p);
  [DllImport("advapi32.dll", SetLastError=true)] public static extern bool OpenProcessToken(IntPtr p, uint a, out IntPtr t);
  [DllImport("advapi32.dll", SetLastError=true)] public static extern bool GetTokenInformation(IntPtr t, int c, IntPtr b, int l, out int r);
  [DllImport("advapi32.dll")] public static extern IntPtr GetSidSubAuthority(IntPtr s, uint n);
  [DllImport("advapi32.dll")] public static extern IntPtr GetSidSubAuthorityCount(IntPtr s);
  public static int Level(int pid) {
    IntPtr p = OpenProcess(0x1000, false, pid);
    if (p == IntPtr.Zero) throw new Exception("OpenProcess " + Marshal.GetLastWin32Error());
    IntPtr t; if (!OpenProcessToken(p, 8, out t)) throw new Exception("OpenProcessToken " + Marshal.GetLastWin32Error());
    int n; GetTokenInformation(t, 25, IntPtr.Zero, 0, out n);
    IntPtr b = Marshal.AllocHGlobal(n);
    if (!GetTokenInformation(t, 25, b, n, out n)) throw new Exception("GetTokenInformation " + Marshal.GetLastWin32Error());
    IntPtr sid = Marshal.ReadIntPtr(b);
    return Marshal.ReadInt32(GetSidSubAuthority(sid, (uint)(Marshal.ReadByte(GetSidSubAuthorityCount(sid)) - 1)));
  }
}
"@
[T]::Level($target)
`;

function windowsReader(folder) {
  const script = path.join(folder, "sandbox-state.ps1");
  fs.writeFileSync(script, WINDOWS_READER);
  return (pid) => {
    const level = Number(execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, String(pid)], { encoding: "utf8" }).trim());
    return { sandboxed: level <= 4096, detail: `integrity level ${level}` };
  };
}

/** A function from pid to { sandboxed, detail }, by this OS's own means. */
export function sandboxReader(folder) {
  return { darwin: macReader, linux: linuxReader, win32: windowsReader }[process.platform](folder);
}
