// The system-side watcher. Two independent sources, each writing to watch.jsonl:
// - unified log (launchservicesd SETFRONT names every front-process change with a pid; WindowServer
//   and tccd name the prompts). Attribution of a past dialog from this same log is its positive control.
// - tools/browser-check/native/watcher: NSWorkspace's front app, the app's active flag, on-screen windows
//   of the app and of the processes that draw system dialogs.
// Anything from the watched app, or a dialog process coming forward, is an ALERT: the run stops there.
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";

import { binDir } from "./paths.mjs";

const DIALOG_PROCESSES = ["UserNotificationCenter", "SecurityAgent", "CoreServicesUIAgent", "universalAccessAuthWarn"];
export const LOG_PREDICATE = [
  `(process == "launchservicesd" AND eventMessage CONTAINS "SETFRONT")`,
  `(process == "tccd" AND eventMessage CONTAINS "ai.brigadier.dev" AND eventMessage CONTAINS "Prompt")`,
  `(process == "WindowServer" AND eventMessage CONTAINS "frontmostProcess")`,
].join(" OR ");

/** One log line as what it means; null when it's noise. */
export function classifyLogLine(row, pid) {
  const text = row.eventMessage ?? "";
  const front = /SETFRONT: pid=(\d+).*foreground=(\d).*oldFrontPid=(\d+)/.exec(text);
  if (front) {
    const to = Number(front[1]);
    return { kind: "log-front", to, foreground: Number(front[2]), from: Number(front[3]), isWatched: to === pid };
  }
  const named = /frontmostProcess: \S+ \(([^)]+)\)/.exec(text);
  if (named) return { kind: "log-front-name", name: named[1], isDialog: DIALOG_PROCESSES.includes(named[1]) };
  if (row.processImagePath?.endsWith("tccd")) return { kind: "log-tcc-prompt", text: text.slice(0, 200) };
  return null;
}

export async function startWatch({ checkDir }) {
  const file = path.join(checkDir, "watch.jsonl");
  const alerts = path.join(checkDir, "ALERTS.jsonl");
  const write = (row) => {
    const line = `${JSON.stringify({ at: new Date().toISOString(), ...row })}\n`;
    fs.appendFileSync(file, line);
    const alert =
      row.isWatched === true ||
      row.isDialog === true ||
      row.kind === "log-tcc-prompt" ||
      (row.kind === "active" && row.active === true) ||
      (row.kind === "dialog-windows" && row.windows.length > 0) ||
      (row.kind === "dev-window-on-user-screen" && process.env.BRIGADIER_FOCUSED !== "1");
    if (alert) fs.appendFileSync(alerts, line);
  };
  let pid = -1;
  const logStream = spawn("/usr/bin/log", ["stream", "--style", "ndjson", "--predicate", LOG_PREDICATE], { stdio: ["ignore", "pipe", "ignore"] });
  readline.createInterface({ input: logStream.stdout }).on("line", (line) => {
    if (!line.startsWith("{")) return;
    try {
      const meaning = classifyLogLine(JSON.parse(line), pid);
      if (meaning) write(meaning);
    } catch { /* a partial line at the end */ }
  });
  let watcher = null;
  // Give `log stream` its moment to attach, so the launch itself is watched.
  await new Promise((resolve) => setTimeout(resolve, 800));
  write({ kind: "watch-start" });
  return {
    setPid(next) {
      pid = next;
      watcher?.kill();
      watcher = spawn(path.join(binDir, "watcher"), [String(next), "200"], { stdio: ["ignore", "pipe", "ignore"] });
      readline.createInterface({ input: watcher.stdout }).on("line", (line) => {
        try {
          write(JSON.parse(line));
        } catch { /* a partial line at the end */ }
      });
    },
    stop() {
      write({ kind: "watch-stop" });
      logStream.kill();
      watcher?.kill();
    },
  };
}
