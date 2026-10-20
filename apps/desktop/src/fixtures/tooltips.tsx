/**
 * Two controls side by side, each with its tooltip. ?drive=1 hovers the first, moves onto its
 * tooltip, then onto the second control, with the real pointer, and writes the tooltips showing
 * to #tooltips-result (TooltipRender.test.ts reads it).
 */
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { Button } from "@/components/controls";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/overlays";
import { fixtureInput } from "@/fixtures/input";

const params = new URLSearchParams(location.search);

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const rect = (selector: string) => document.querySelector(selector)!.getBoundingClientRect();
const pointer = (x: number, y: number) => fixtureInput({ type: "pointer", x, y });
/** The tooltips showing now: each one's words. */
const showing = () =>
  [...document.querySelectorAll('[data-slot="tooltip-content"]:not([data-state="closed"])')].map(
    (tip) => tip.firstChild?.textContent?.trim() ?? "",
  );

function Fixture() {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    if (!params.has("drive")) return;
    void (async () => {
      await wait(300);
      const first = rect("#first");
      await pointer(first.x + first.width / 2, first.y + first.height / 2);
      await wait(300);
      const onFirst = showing();
      // Up onto its tooltip, which reaches over the second control, then along it.
      const tip = rect('[data-slot="tooltip-content"]');
      const second = rect("#second");
      await pointer(first.x + first.width / 2, tip.y + tip.height / 2);
      await wait(100);
      await pointer(second.x + 2, tip.y + tip.height / 2);
      await wait(100);
      // Down onto the second control, near the first, and a little further in: at mid-height,
      // since a capsule's box corners are outside it and the pointer there is on nothing.
      await pointer(second.x + 1, second.y + second.height / 2);
      await wait(100);
      await pointer(second.x + 2, second.y + second.height / 2);
      await wait(300);
      setResult(JSON.stringify({ onFirst, onSecond: showing() }));
    })();
  }, []);
  return (
    <TooltipProvider>
      <div className="flex h-screen items-center justify-center">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button id="first" variant="outline">A</Button>
          </TooltipTrigger>
          <TooltipContent>The first control’s tooltip, long enough to reach past the next one</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button id="second" variant="outline">B</Button>
          </TooltipTrigger>
          <TooltipContent>The second control’s tooltip</TooltipContent>
        </Tooltip>
      </div>
      {result && <pre id="tooltips-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
