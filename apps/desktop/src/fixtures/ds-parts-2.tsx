/**
 * The design system's merged small parts (cleanup §3, groups 11–14: diff stat, copy button, pager, key chip),
 * one block per group, each read back as the browser drew it into #parts2-result for
 * DsParts2Render.test.ts.
 */
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { Counts } from "@/app/conversation/Counts";
import { TooltipProvider } from "@/components/ui/tooltip";
import { colourOf } from "@/fixtures/look";

const one = (selector: string) => document.querySelector(selector);

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
    result.id = "parts2-result";
    result.textContent = JSON.stringify(read());
    document.body.append(result);
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
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
