/**
 * The design system's merged parts (cleanup §3), one block per group, each read back as the
 * browser drew it into #merges-result for DsMergesRender.test.ts.
 */
import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { SettingsSection } from "@/app/settings/parts";
import { ChevronRight, Plus } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { RowText, SettingsCard } from "@/components/ui/settings-card";
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

/** How far the hairline under a card's first child starts in from the card's own edge. */
function hairlineInset(slot: string) {
  const card = one(`[data-probe="${slot}"]`);
  const row = card?.firstElementChild;
  if (!card || !row) return null;
  const after = getComputedStyle(row, "::after");
  if (after.content === "none") return "none";
  return row.getBoundingClientRect().left + parseFloat(after.left) - card.getBoundingClientRect().left;
}

function read() {
  const button = probe;
  const section = one('[data-probe="section"]')?.closest("section");
  const aside = one('[data-probe="section-aside"]')?.parentElement;
  const words = (slot: string) => {
    const [title, line] = Array.from(one(`[data-probe="${slot}"]`)?.firstElementChild?.children ?? []).map((part) => getComputedStyle(part));
    return { weight: title?.fontWeight, overflow: title?.textOverflow, line: line?.color };
  };
  return {
    rowText: { plain: words("row-text"), quiet: words("row-text-quiet"), grey: colourOf("text-foreground/65", "color") },
    section: {
      setting: section?.getAttribute("data-setting"),
      aside: aside && getComputedStyle(aside).color,
      grey: colourOf("text-foreground/65", "color"),
      heading: section?.querySelector("h2")?.textContent,
    },
    cards: {
      rows: hairlineInset("card-rows"),
      inset: hairlineInset("card-inset"),
      plain: hairlineInset("card-plain"),
      surfaces: ["card-rows", "card-inset", "card-plain", "card-padded"].map((slot) => surface(one(`[data-probe="${slot}"]`))),
    },
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
        <SettingsSection
          title="Disk use by project"
          setting="Disk use"
          aside={<span data-probe="section-aside">Largest first</span>}
          actions={<Button variant="soft" size="sm">Refresh</Button>}
        >
          <span data-probe="section">placeholder</span>
        </SettingsSection>
        <section data-group="row-text" className="flex w-80 gap-4">
          <div data-probe="row-text" className="flex min-w-0 flex-1">
            <RowText truncate title="Explore the code, every file of it" line="Automatic picks the strongest model" />
          </div>
          <div data-probe="row-text-quiet" className="flex min-w-0 flex-1">
            <RowText quiet title="Who picks" line="Brigadier" />
          </div>
        </section>
        <section data-group="cards" className="grid grid-cols-4 gap-4">
          <SettingsCard data-probe="card-rows">
            <div className="px-4 py-3">A row padding itself</div>
            <div className="px-4 py-3">Another</div>
          </SettingsCard>
          <SettingsCard inset data-probe="card-inset">
            <div className="py-3">A row in an inset card</div>
            <div className="py-3">Another</div>
          </SettingsCard>
          <SettingsCard inset split={false} data-probe="card-plain">
            <div className="py-3">Content drawing its own lines</div>
            <div className="py-3">Another</div>
          </SettingsCard>
          <SettingsCard padded data-probe="card-padded">
            <div>Usage</div>
            <div>Legend</div>
          </SettingsCard>
        </section>
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
