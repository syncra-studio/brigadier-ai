/**
 * A standalone Vite entry, excluded from the app entry and production build: a dialog and a
 * popover from the app's own components, with the page-hidden mark the app sets at startup.
 *
 * `?drive=1` closes each of them three ways and writes JSON into `#hidden-exit-result`
 * (`pageHidden.test.ts` checks it). In a hidden or covered macOS window WebKit runs no animation
 * frames, so an exit animation never ends; here every animation is paused instead ("frozen"), and
 * `document.hidden` is faked. For each, the parts still mounted 400 ms after Escape:
 * - `shown`: visible, frames running, so the exit animation plays and ends;
 * - `hidden`: hidden, frames frozen, so only the mark lets it leave;
 * - `frozen`: visible, frames frozen, the control: it stays, as the app's window did.
 */
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { Button } from "@/components/controls";
import { Dialog, DialogContent, DialogDescription, DialogTitle, DialogTrigger, Popover, PopoverContent, PopoverTrigger } from "@/components/overlays";
import { trackPageHidden } from "@/lib/pageHidden";

trackPageHidden();

const SURFACES = {
  popover: ['[data-slot="popover-content"]'],
  dialog: ['[data-slot="dialog-content"]', '[data-slot="dialog-overlay"]'],
};
const CASES = { shown: [false, false], hidden: [true, true], frozen: [false, true] } as const;
/** Longer than any exit animation here (100 ms), so a closed part still mounted is stuck. */
const SETTLE_MS = 400;

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function setHidden(hidden: boolean) {
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
  document.dispatchEvent(new Event("visibilitychange"));
}

async function drive() {
  const freeze = document.createElement("style");
  freeze.textContent = "*, *::before, *::after { animation-play-state: paused !important; }";
  const seen: Record<string, { marked: boolean; mounted: string[] }> = {};
  for (const [surface, parts] of Object.entries(SURFACES)) {
    for (const [name, [hidden, frozen]] of Object.entries(CASES)) {
      document.querySelector<HTMLButtonElement>(`[data-testid="open-${surface}"]`)!.click();
      await wait(300);
      if (frozen) document.head.append(freeze);
      setHidden(hidden);
      const focused = document.activeElement ?? document.body;
      focused.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      await wait(SETTLE_MS);
      seen[`${surface} ${name}`] = {
        marked: "pageHidden" in document.documentElement.dataset,
        mounted: parts.filter((part) => document.querySelector(`${part}[data-state="closed"]`)),
      };
      setHidden(false);
      freeze.remove();
    }
  }
  const result = document.createElement("pre");
  result.id = "hidden-exit-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

function Fixture() {
  useEffect(() => {
    if (new URLSearchParams(location.search).has("drive")) void drive();
  }, []);
  return (
    <div className="bg-background text-foreground flex min-h-screen items-start gap-4 p-8">
      <Dialog>
        <DialogTrigger asChild>
          <Button data-testid="open-dialog">Open dialog</Button>
        </DialogTrigger>
        <DialogContent>
          <DialogTitle>Trust this folder?</DialogTitle>
          <DialogDescription>A dialog that closes while the window is covered.</DialogDescription>
        </DialogContent>
      </Dialog>
      <Popover>
        <PopoverTrigger asChild>
          <Button data-testid="open-popover">Open popover</Button>
        </PopoverTrigger>
        <PopoverContent>A popover</PopoverContent>
      </Popover>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
