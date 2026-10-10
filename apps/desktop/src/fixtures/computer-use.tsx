/** Settings → Computer use with its permissions faked through the IPC mock. No daemon, no system grants. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ComputerUsePage } from "@/app/settings/ComputerUsePage";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ComputerAccess, Request } from "@/ipc/generated";

/*
 * ?state=missing (the default) | half | ready | restarting | problem picks what the permissions
 * read as. ?press=<row> presses that row's Allow… once shown (for a look at the page after it).
 * ?tip=1 focuses the footnote's tip from the keyboard, so it opens.
 * ?drive=1 presses the page's controls in turn and writes what it saw to
 * #computer-use-result (ComputerUsePageRender.test.ts reads it).
 */
const params = new URLSearchParams(location.search);
const none: ComputerAccess = { available: true, accessibility: false, screenRecording: false, restarting: false, problem: null };
const STATES = {
  missing: none,
  half: { ...none, accessibility: true },
  ready: { ...none, accessibility: true, screenRecording: true },
  restarting: { ...none, accessibility: true, screenRecording: true, restarting: true },
  problem: { ...none, problem: "Brigadier Computer Use didn't start: its copy in the data folder failed its signature check." },
} satisfies Record<string, ComputerAccess>;
let access: ComputerAccess = STATES[(params.get("state") ?? "missing") as keyof typeof STATES] ?? none;
const calls: string[] = [];

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getComputerAccess":
      calls.push("get");
      return { method: req.method, access };
    case "allowComputerAccess":
      calls.push(`allow ${req.grant}${req.startOver ? " startOver" : ""}`);
      return { method: req.method, access };
    case "openComputerSettings":
      calls.push(`open ${req.grant}`);
      return { method: req.method };
    default:
      return { method: req.method };
  }
});

const q = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const buttons = () => [...document.querySelectorAll<HTMLButtonElement>("main button")];
const labels = () => buttons().map((button) => button.textContent?.trim() || button.getAttribute("aria-label") || "");
const press = (label: string) => buttons().find((button) => button.textContent?.trim() === label || button.getAttribute("aria-label") === label)?.click();
const row = (label: string) => q(`[data-setting="${label}"]`);
const pressIn = (rowLabel: string, label: string) =>
  [...(row(rowLabel)?.querySelectorAll("button") ?? [])].find((button) => button.textContent?.trim() === label)?.click();
const status = () => q('[data-slot="computer-use-status"] [role="status"]')?.textContent ?? null;

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("tip")) return;
    const timer = setTimeout(() => q<HTMLButtonElement>('button[aria-label="Where to find it"]')?.focus(), 300);
    return () => clearTimeout(timer);
  }, []);
  useEffect(() => {
    const pressed = params.get("press");
    if (!pressed) return;
    const timer = setTimeout(() => pressIn(pressed, "Allow…"), 300);
    return () => clearTimeout(timer);
  }, []);
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const at = (ms: number, run: () => void) => setTimeout(run, ms);
    const timers = [
      at(300, () => {
        seen.first = { status: status(), buttons: labels() };
        pressIn("See the screen", "Allow…");
      }),
      at(600, () => {
        seen.asked = { buttons: labels(), seeTheScreen: row("See the screen")?.textContent };
        pressIn("See the screen", "Open System Settings");
        press("Start over");
      }),
      at(900, () => {
        // The user turns both on in System Settings, then presses Check again.
        access = STATES.ready;
        const before = calls.filter((call) => call === "get").length;
        press("Check again");
        seen.checked = before;
      }),
      at(1200, () => {
        seen.checked = calls.filter((call) => call === "get").length > (seen.checked as number);
        seen.ready = { status: status(), buttons: labels() };
        seen.calls = calls.filter((call) => call !== "get");
        // Keyboard: every control is a native button, reached in reading order.
        seen.focusable = buttons().every((button) => button.tabIndex === 0);
        setResult(JSON.stringify(seen));
      }),
    ];
    return () => timers.forEach(clearTimeout);
  }, []);
  return (
    <TooltipProvider>
      <main className="bg-background text-foreground h-screen">
        <ComputerUsePage />
      </main>
      {result && <pre id="computer-use-result">{result}</pre>}
    </TooltipProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Fixture />);
