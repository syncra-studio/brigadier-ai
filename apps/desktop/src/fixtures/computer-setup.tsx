/** Computer use's setup window over a faked daemon. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SetupWindow } from "@/app/setup/computerSetup/SetupWindow";
import type { ComputerSetup, ComputerUseStatus, PermissionState, Request } from "@/ipc/generated";
import { onComputerUse } from "@/state/computerUse";

/*
 * ?drive=1 walks the window through its three steps as the permissions arrive, presses its
 * buttons, and writes what it saw to #computer-setup-result (ComputerSetupRender.test.ts).
 */
const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}

let current: ComputerUseStatus = status({ state: "notSetUp" });
function status(setup: ComputerSetup, accessibility: PermissionState = "unknown", screenRecording: PermissionState = "unknown"): ComputerUseStatus {
  return { available: true, accessibility, screenRecording, current: true, setup, sessions: [] };
}
let problem: string | null = "the window couldn't be captured";

const calls: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") {
    const permission = (payload as { permission?: string } | undefined)?.permission;
    calls.push(permission ? `${command} ${permission}` : command);
    return null;
  }
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getComputerUse":
      return { method: req.method, computerUse: current };
    case "checkComputerUse":
      calls.push(req.method);
      return { method: req.method, computerUse: current };
    case "confirmComputerCapture": {
      calls.push(req.method);
      if (!problem) current = status({ state: "ready" }, "granted", "granted");
      return { method: req.method, computerUse: current, problem };
    }
    default:
      calls.push(req.method);
      return { method: req.method };
  }
});

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const button = (name: string) => all<HTMLButtonElement>("button").find((element) => text(element) === name);

function look() {
  return {
    steps: all('[data-slot="setup-step"]').map((step) => ({
      title: text(step),
      done: step.dataset.done !== undefined,
      current: step.getAttribute("aria-current") === "step",
    })),
    title: text(document.querySelector("main h1")),
    buttons: all<HTMLButtonElement>("main button").map(text),
    card: document.querySelector('[data-slot="helper-card"]') !== null,
    text: text(document.querySelector("main")),
  };
}

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const set = (next: ComputerUseStatus) => {
      current = next;
      onComputerUse(next);
    };
    const steps: (() => void)[] = [
      () => {
        seen.opened = [...calls];
        seen.accessibility = look();
        button("Check again")?.click();
      },
      () => button("Open System Settings")?.click(),
      () => document.querySelector('[data-slot="helper-card"]')?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0 })),
      () => button("Show in Finder")?.click(),
      () => set(status({ state: "missing" }, "granted", "notGranted")),
      () => {
        seen.screenRecording = look();
        // Back from System Settings.
        window.dispatchEvent(new Event("focus"));
      },
      () => set(status({ state: "confirmCapture" }, "granted", "granted")),
      () => {
        seen.capture = look();
        button("Take a test picture")?.click();
      },
      () => {
        seen.captureFailed = look();
        problem = null;
        button("Take a test picture")?.click();
      },
      () => {
        seen.done = look();
        button("Done")?.click();
      },
      () => {
        seen.closed = closedRef.value;
        seen.calls = calls;
        setResult(JSON.stringify(seen));
      },
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    void (async () => {
      await wait(400);
      for (const step of steps) {
        if (stopped) return;
        step();
        await wait(150);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <>
      <SetupWindow onClose={() => (closedRef.value = true)} />
      {result && <pre id="computer-setup-result">{result}</pre>}
    </>
  );
}
const closedRef = { value: false };

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
