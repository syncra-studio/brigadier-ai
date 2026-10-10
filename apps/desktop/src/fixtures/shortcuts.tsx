/** Settings › Keyboard shortcuts: search by words, and by keystrokes recorded ahead of the app. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ShortcutsPage } from "@/app/settings/ShortcutsPage";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useApp } from "@/state/store";

/*
 * ?drive=1 searches by words, records key presses (one a menu sends again right after, one Esc)
 * and writes what it saw to #shortcuts-result (ShortcutsPageRender.test.ts reads it).
 */
const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}

useApp.setState((state) => ({ info: { ...state.info, platform: "macos" } as typeof state.info }));

const capture: boolean[] = [];
mockIPC((command, payload) => {
  if (command === "set_key_capture") capture.push((payload as { on: boolean }).on);
  return null;
});

// What the app's own key handlers would see (they listen on the window, after capture).
const reached: string[] = [];
window.addEventListener("keydown", (event) => reached.push(`${event.metaKey ? "⌘" : ""}${event.key}`));

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const rows = () => all('[data-slot="shortcut-row"]').map((row) => row.dataset.setting);
const button = (name: string) =>
  all<HTMLButtonElement>("button").find((element) => element.getAttribute("aria-label") === name || text(element) === name);
const key = (code: string, keyName: string, init: KeyboardEventInit = {}) =>
  (document.activeElement ?? document.body).dispatchEvent(
    new KeyboardEvent("keydown", { code, key: keyName, bubbles: true, cancelable: true, ...init }),
  );
const typeSearch = (value: string) => {
  const input = document.querySelector<HTMLInputElement>('input[aria-label="Search shortcuts"]');
  if (!input) return;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: [number, () => void][] = [
      [0, () => {
        seen.count = rows().length;
        seen.chips = all('[data-setting="Toggle sidebar"] kbd').map(text);
        typeSearch("dictat");
      }],
      [0, () => {
        seen.byWords = rows();
        typeSearch("zzz");
      }],
      [0, () => {
        seen.none = text(document.querySelector('main [role="status"]'));
        button("Clear search")?.click();
      }],
      [0, () => button("Search by keystrokes")?.click()],
      [0, () => {
        seen.recording = text(document.querySelector('[data-slot="shortcuts-search"] [role="status"]'));
        key("BracketLeft", "[", { metaKey: true });
        // The macOS menu sends its item again as the same key, right after.
        key("BracketLeft", "[", { metaKey: true });
      }],
      [0, () => {
        seen.byPress = rows();
        seen.pressChip = all('[data-slot="shortcuts-search"] kbd').map(text);
        seen.reachedWhileRecording = [...reached];
        button("Search by keystrokes")?.click();
      }],
      [0, () => key("Escape", "Escape")],
      [0, () => {
        seen.afterEsc = { rows: rows().length, input: document.querySelector('input[aria-label="Search shortcuts"]') !== null };
        button("Search by keystrokes")?.click();
      }],
      [0, () => {
        key("ShiftLeft", "Shift", { shiftKey: true, metaKey: true });
        key("KeyS", "s", { shiftKey: true, metaKey: true });
      }],
      [500, () => {
        seen.clash = rows();
        seen.reachedAfterRecording = [...reached];
        // Once the echo window has passed, keys reach the app again.
        key("BracketLeft", "[", { metaKey: true });
      }],
      [0, () => {
        seen.reachedLater = [...reached];
        seen.capture = capture;
        setResult(JSON.stringify(seen));
      }],
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    void (async () => {
      await wait(300);
      for (const [delay, step] of steps) {
        if (stopped) return;
        await wait(delay);
        step();
        await wait(100);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <main className="flex h-full min-w-0 flex-1">
          <div className="min-w-0 flex-1">
            <ShortcutsPage />
          </div>
        </main>
      </div>
      {result && <pre id="shortcuts-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
