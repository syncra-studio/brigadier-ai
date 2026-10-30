/**
 * How fast a conversation opens: the whole app started as `main.tsx` starts it (the bridge, then
 * App), over a real daemon whose requests and frames `scripts/measure-open-speed.mjs` passes
 * through, as the live-refresh fixture's are. The script drives `select` and samples the frames.
 */
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { App } from "@/app/App";
import type { AppInfo, BridgeEvent, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { startBridge } from "@/state/bridge";
import { useCliThread } from "@/state/cliThread";
import { useApp } from "@/state/store";

declare global {
  interface Window {
    /** The probe's way to the daemon: a request's answer, or its error. */
    daemon: (request: Request) => Promise<{ value?: unknown; error?: { code: string; message: string } }>;
    /** Asks the probe to subscribe; it then passes the daemon's frames to `bridged`. */
    subscribed: () => Promise<void>;
    bridged?: (event: BridgeEvent) => void;
    /** Each request the page made: when it asked and when its answer came (`performance.now()`). */
    openSpeedCalls: { method: string; asked: number; answered: number }[];
    openSpeed: { select: typeof select; views: () => ReturnType<typeof useCliThread.getState>["views"]; selected: () => string | null };
  }
}

const info: AppInfo = { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 };
document.documentElement.dataset.platform = "macos";
mockIPC(async (command, args) => {
  const payload = args as { request: Request; channel: { onmessage: (event: BridgeEvent) => void } };
  switch (command) {
    case "ipc_request": {
      const call = { method: payload.request.method, asked: performance.now(), answered: NaN };
      window.openSpeedCalls.push(call);
      const answer = await window.daemon(payload.request);
      call.answered = performance.now();
      if (answer.error) throw answer.error;
      return answer.value;
    }
    case "ipc_subscribe":
      window.bridged = (event) => payload.channel.onmessage(event);
      await window.subscribed();
      return null;
    case "app_info":
      return info;
    case "take_opened_folders":
      return [];
    default:
      return null;
  }
}, { shouldMockEvents: true });
mockWindows("main");

window.openSpeedCalls = [];
window.openSpeed = {
  select,
  views: () => useCliThread.getState().views,
  selected: () => {
    const { selection } = useApp.getState();
    return selection.type === "conversation" ? selection.id : null;
  },
};
useApp.setState({ info });
void startBridge().then(() => createRoot(document.getElementById("root")!).render(<App />));
