/**
 * The design system's merged small parts (cleanup §3, groups 11–14: diff stat, copy button, pager, key chip),
 * one block per group, each read back as the browser drew it into #parts2-result for
 * DsParts2Render.test.ts.
 */
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { Counts } from "@/app/conversation/Counts";
import { CopyButton } from "@/components/assistant-ui/copy-button";
import { TooltipProvider } from "@/components/ui/tooltip";
import { colourOf } from "@/fixtures/look";

const one = (selector: string) => document.querySelector(selector);

/** What the copy buttons wrote, the headless browser's clipboard being out of reach. */
const copied: string[] = [];
Object.defineProperty(navigator, "clipboard", { value: { writeText: async (text: string) => void copied.push(text) } });

/** Clicks that reached the card around the second copy button. */
let cardClicks = 0;

/** Clicks each copy button, then reads its name and icon once the copy has landed. */
async function copyButtons() {
  const buttons = ["copy", "copy-in-card"].map((slot) => one(`[data-probe="${slot}"]`) as HTMLButtonElement | null);
  const before = buttons.map((button) => button?.textContent);
  for (const button of buttons) button?.click();
  await new Promise((resolve) => setTimeout(resolve, 100));
  return {
    before,
    after: buttons.map((button) => button?.textContent),
    checked: buttons.map((button) => button?.querySelector("svg")?.classList.contains("zoom-in-50") ?? null),
    copied,
    cardClicks,
  };
}

/** A diff stat's face and its two numbers' colours. */
function counts(slot: string) {
  const element = one(`[data-probe="${slot}"]`);
  if (!element) return null;
  const css = getComputedStyle(element);
  const [plus, minus] = Array.from(element.children).map((part) => getComputedStyle(part).color);
  return { font: css.fontFamily, size: css.fontSize, text: element.textContent, plus, minus };
}

/** The font family or size `className` gives, read off a probe. */
function fontOf(className: string, property: "fontFamily" | "fontSize" = "fontFamily"): string {
  const probe = document.body.appendChild(Object.assign(document.createElement("div"), { className }));
  const font = getComputedStyle(probe)[property];
  probe.remove();
  return font;
}

function read() {
  return {
    counts: {
      plain: counts("counts"),
      zero: counts("counts-zero"),
      dim: counts("counts-dim"),
      quiet: counts("counts-quiet"),
      colours: {
        success: colourOf("text-success", "color"),
        destructive: colourOf("text-destructive", "color"),
        muted: colourOf("text-muted-foreground", "color"),
        mono: fontOf("font-mono"),
        xs: fontOf("text-xs", "fontSize"),
      },
    },
  };
}

function Fixture() {
  useEffect(() => {
    const result = document.createElement("pre");
    void copyButtons().then((copy) => {
      result.id = "parts2-result";
      result.textContent = JSON.stringify({ ...read(), copy });
      document.body.append(result);
    });
    return () => result.remove();
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-card flex flex-col gap-6 p-6">
        <section data-group="counts" className="flex items-center gap-4">
          <Counts data-probe="counts" insertions={88} deletions={7} />
          <Counts data-probe="counts-zero" insertions={12} deletions={0} />
          <Counts data-probe="counts-dim" dim insertions={88} deletions={7} />
          <span className="group">
            <Counts data-probe="counts-quiet" quiet insertions={7} deletions={2} />
          </span>
        </section>
        <section data-group="copy" className="flex items-center gap-4">
          <CopyButton data-probe="copy" text="npm test" tooltip="Copy command" size="icon-xs" />
          <div role="presentation" onClick={() => cardClicks++}>
            <CopyButton data-probe="copy-in-card" text="# Plan" tooltip="Copy plan" copiedDuration={1500} onClick={(event) => event.stopPropagation()} />
          </div>
        </section>
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
