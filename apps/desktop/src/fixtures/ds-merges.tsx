/**
 * The design system's merged parts (cleanup §3), one block per group, each read back as the
 * browser drew it into #merges-result for DsMergesRender.test.ts.
 */
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { ChevronRight, Plus } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { TooltipProvider } from "@/components/ui/tooltip";
import { colourOf, surface } from "@/fixtures/look";

const one = (selector: string) => document.querySelector(selector);

/** A probed element's corners, fill and text colour. */
function probe(slot: string) {
  const element = one(`[data-probe="${slot}"]`);
  const look = surface(element);
  return element && look && { radius: look.radius, background: look.background, color: getComputedStyle(element).color };
}

/** The corner `className` gives, read off a probe. */
function radiusOf(className: string): string {
  const element = document.body.appendChild(Object.assign(document.createElement("div"), { className }));
  const radius = getComputedStyle(element).borderTopLeftRadius;
  element.remove();
  return radius;
}

function read() {
  const button = probe;
  return {
    buttons: {
      default: button("button-default"),
      soft: button("button-soft"),
      softDestructive: button("button-soft-destructive"),
      linkBlue: button("button-link-blue"),
      icon: button("button-icon"),
      radii: { capsule: radiusOf("rounded-capsule"), control: radiusOf("rounded-control") },
      colours: {
        soft: colourOf("bg-foreground/5"),
        softDestructive: colourOf("bg-destructive/10"),
        link: colourOf("text-link", "color"),
      },
    },
  };
}

function Fixture() {
  useEffect(() => {
    const result = document.createElement("pre");
    result.id = "merges-result";
    result.textContent = JSON.stringify(read());
    document.body.append(result);
    return () => result.remove();
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-card flex flex-col gap-6 p-6">
        <section data-group="buttons" className="flex items-center gap-2">
          <Button size="sm" data-probe="button-default">Add rule</Button>
          <Button variant="ghost" size="sm">Cancel</Button>
          <Button variant="soft" size="sm" data-probe="button-soft">
            <Plus aria-hidden />
            Add rule
          </Button>
          <Button variant="soft-destructive" size="sm" data-probe="button-soft-destructive">Delete…</Button>
          <Button variant="link-blue" size="inline" data-probe="button-link-blue">
            Show all
            <ChevronRight aria-hidden />
          </Button>
          <Button variant="outline" size="icon-sm" aria-label="Add" data-probe="button-icon">
            <Plus />
          </Button>
        </section>
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
