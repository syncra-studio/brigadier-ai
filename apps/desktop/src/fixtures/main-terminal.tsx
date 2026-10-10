/** A main terminal tab's shell exiting, a reconnect and a restart, with synthetic IPC; never contacts a daemon. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { MainTerminalTab } from "@/app/conversation/MainTerminalTab";
import type { Request } from "@/ipc/generated";
import { newSessionTab, sessionTabs, type TerminalTabState } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { emitTerminalOutput } from "@/state/terminalPlaces";

const conversationId = "main-terminal-fixture";
let next = 0;
let current: string | null = null;
const opens: { fresh: boolean; id: string }[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "openTerminal") {
    // The daemon reattaches to a running shell, else starts one.
    if (req.fresh || current === null) current = `shell-${++next}`;
    opens.push({ fresh: req.fresh ?? false, id: current });
    return { method: req.method, terminal: { id: current, shell: "/bin/zsh", cwd: "/tmp", scrollback: "" } };
  }
  if (req.method === "closeTerminal" && req.terminalId === current) current = null;
  return { method: req.method };
});
useApp.setState({
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});
const tabId = newSessionTab(conversationId, "terminal");
const tab = sessionTabs(conversationId).tabs.find((entry) => entry.id === tabId) as TerminalTabState;

const pause = (ms = 300) => new Promise((resolve) => setTimeout(resolve, ms));
const notice = () => document.querySelector('[role="status"]')?.textContent ?? null;
const restart = () => [...document.querySelectorAll("button")].find((button) => button.textContent === "Restart");
function exit(): void {
  const id = current!;
  current = null;
  emitTerminalOutput({ type: "exited", terminalId: id, code: 0 });
}
function setConnected(connected: boolean): void {
  useApp.setState({ connection: { status: connected ? "connected" : "disconnected", daemon: null, reason: null } });
}

async function drive() {
  await pause();
  const opened = notice();
  exit();
  await pause();
  const exited = { notice: notice(), restart: restart() !== undefined };
  // The connection drops and comes back: the view reopens on a new shell.
  setConnected(false);
  await pause();
  setConnected(true);
  await pause();
  const reconnected = { notice: notice(), shell: current };
  exit();
  await pause();
  restart()!.click();
  await pause();
  const restarted = { notice: notice(), shell: current };
  const result = document.createElement("pre");
  result.id = "main-terminal-result";
  result.textContent = JSON.stringify({ opened, exited, reconnected, restarted, opens });
  document.body.append(result);
}

createRoot(document.getElementById("root")!).render(
  <div className="flex h-screen flex-col"><MainTerminalTab conversationId={conversationId} tab={tab} active /></div>,
);
void drive();
