/**
 * The design system's merged small parts (cleanup §3, groups 6–10), one block per group, each
 * read back as the browser drew it into #parts-result for DsPartsRender.test.ts.
 */
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { Picker } from "@/app/inspector/providers/Picker";
import { Grid, SquareImage } from "@/components/icons";
import { Segmented } from "@/components/controls/segmented";
import { SettingsSearch } from "@/components/shared/settings-search";
import { StateDot } from "@/components/shared/state-dot";
import { TooltipProvider } from "@/components/overlays/tooltip";
import { UsageBar } from "@/components/shared/usage-bar";
import { colourOf, wait } from "@/fixtures/look";

const one = (selector: string) => document.querySelector(selector);

/** The corner `className` gives, read off a probe. */
function radiusOf(className: string): string {
  const element = document.body.appendChild(Object.assign(document.createElement("div"), { className }));
  const radius = getComputedStyle(element).borderTopLeftRadius;
  element.remove();
  return radius;
}

/** A dropdown trigger's words, whether it is off, and how it sits in its grid cell. */
function picker(label: string) {
  const trigger = one(`[aria-label="${label}"]`) as HTMLButtonElement | null;
  return (
    trigger && {
      text: trigger.textContent,
      disabled: trigger.disabled,
      justify: getComputedStyle(trigger).justifySelf,
    }
  );
}

/** Each choice of the segmented control `label`: its name, whether it is chosen, fill and height. */
function segments(label: string) {
  return Array.from(document.querySelectorAll(`[aria-label="${label}"] > button`)).map((item) => ({
    name: item.getAttribute("aria-label") ?? item.textContent,
    role: item.getAttribute("role"),
    on: item.getAttribute("data-state") === "on",
    fill: getComputedStyle(item).backgroundColor,
    height: item.getBoundingClientRect().height,
  }));
}

/** A search field by its input's name: its box, the input's kind and words, a clear button. */
function field(label: string) {
  const input = one(`input[aria-label="${label}"]`) as HTMLInputElement | null;
  const box = input?.closest('[data-slot="settings-search"]');
  return (
    input &&
    box && {
      box: box.tagName.toLowerCase(),
      type: input.type,
      value: input.value,
      radius: getComputedStyle(box).borderTopLeftRadius,
      height: box.getBoundingClientRect().height,
      icon: !!box.querySelector("svg[aria-hidden]"),
      clear: !!box.querySelector('[aria-label="Clear search"]'),
      buttons: Array.from(box.querySelectorAll("button")).map((button) => button.getAttribute("aria-label")),
    }
  );
}

const escape = (label: string) =>
  one(`input[aria-label="${label}"]`)?.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
  );

/** Esc in each field; what the fields hold after, and which Esc reached the page. */
async function escapes() {
  const heard: string[] = [];
  document.addEventListener("keydown", (event) =>
    heard.push((event.target as HTMLInputElement).ariaLabel ?? ""),
  );
  const before = {
    pill: field("Search archived chats"),
    picker: field("Search projects"),
    panel: field("Search"),
  };
  escape("Search archived chats");
  escape("Search projects");
  await wait(50);
  return { before, after: { pill: field("Search archived chats"), picker: field("Search projects") }, heard };
}

/** A bar by its probe: track height, corners and fill; each fill's width and corners. */
function bar(slot: string) {
  const track = one(`[data-probe="${slot}"]`);
  if (!track) return null;
  const css = getComputedStyle(track);
  return {
    height: track.getBoundingClientRect().height,
    radius: css.borderTopLeftRadius,
    background: css.backgroundColor,
    hidden: track.getAttribute("aria-hidden"),
    role: track.getAttribute("role"),
    fills: Array.from(track.children).map((fill) => ({
      width: fill.getBoundingClientRect().width,
      radius: getComputedStyle(fill).borderTopLeftRadius,
    })),
  };
}

function read() {
  return {
    dots: Array.from(document.querySelectorAll('[data-group="dots"] > span')).map((dot) => ({
      size: dot.getBoundingClientRect().width,
      fill: getComputedStyle(dot).backgroundColor,
      hidden: dot.getAttribute("aria-hidden"),
    })),
    dotFills: ["bg-success", "bg-warning", "bg-muted-foreground", "bg-chart-2"].map((fill) => colourOf(fill)),
    bars: {
      sm: bar("bar-sm"),
      md: bar("bar-md"),
      parts: bar("bar-parts"),
      waiting: bar("bar-waiting"),
      empty: bar("bar-empty"),
      track: colourOf("bg-muted"),
    },
    segmented: {
      words: segments("How it picks"),
      icons: segments("Image view"),
      chosen: colourOf("bg-foreground/5"),
    },
    picker: {
      chosen: picker("Session"),
      unpicked: picker("Window"),
      empty: picker("Task"),
    },
  };
}

const WINDOWS = [
  { value: "five_hour", label: "five_hour" },
  { value: "seven_day", label: "seven_day" },
];

/** The search fields, each holding words. */
function Searches() {
  const [pill, setPill] = useState("plans");
  const [project, setProject] = useState("brig");
  const [panel, setPanel] = useState("todo");
  return (
    <section data-group="search" className="flex w-96 flex-col gap-2">
      <div className="flex">
        <SettingsSearch label="Search archived chats" value={pill} onChange={setPill} />
      </div>
      <SettingsSearch
        variant="bordered"
        label="Search projects"
        value={project}
        onChange={setProject}
        clearable={false}
        clearOnEscape={false}
        role="combobox"
        aria-expanded
        aria-controls="search-projects-list"
      />
      <div id="search-projects-list" role="listbox" aria-label="Projects" />
      <SettingsSearch
        variant="bordered"
        icon={null}
        clearable={false}
        clearOnEscape={false}
        label="Search"
        value={panel}
        onChange={setPanel}
      >
        <button type="button" aria-label="Match case">
          Aa
        </button>
      </SettingsSearch>
    </section>
  );
}

function Fixture() {
  useEffect(() => {
    const result = document.createElement("pre");
    result.id = "parts-result";
    const seen = read();
    void escapes().then((search) => {
      result.textContent = JSON.stringify({
        ...seen,
        search,
        radii: {
          capsule: radiusOf("rounded-capsule"),
          control: radiusOf("rounded-control"),
          square: radiusOf("rounded-none"),
        },
      });
      document.body.append(result);
    });
    return () => result.remove();
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-card flex flex-col gap-6 p-6">
        <section data-group="segmented" className="flex items-center gap-4">
          <Segmented
            label="How it picks"
            value="automatic"
            options={[
              { value: "automatic", label: "Automatic" },
              { value: "order", label: "My order" },
            ]}
            onChange={() => {}}
          />
          <Segmented
            label="Image view"
            value="canvas"
            options={[
              { value: "focused", label: <SquareImage className="size-icon-sm" />, tip: "Focused view" },
              { value: "canvas", label: <Grid className="size-icon-sm" />, tip: "Canvas view" },
            ]}
            onChange={() => {}}
          />
        </section>
        <Searches />
        <section data-group="dots" className="flex items-center gap-2">
          <StateDot tone="ready" />
          <StateDot tone="needs" />
          <StateDot tone="off" />
          <StateDot color="bg-chart-2" />
        </section>
        <section data-group="bars" className="flex w-50 flex-col gap-2">
          <UsageBar data-probe="bar-sm" value={40} fill="bg-warning" className="w-25" />
          <UsageBar
            data-probe="bar-md"
            role="meter"
            aria-label="Context used"
            aria-valuenow={60}
            value={60}
            size="md"
          />
          <UsageBar
            data-probe="bar-parts"
            size="md"
            parts={[
              { key: "a", value: 50, className: "bg-primary" },
              { key: "b", value: 0.1, className: "bg-warning" },
            ]}
          />
          <UsageBar data-probe="bar-waiting" role="progressbar" indeterminate size="md" />
          <UsageBar data-probe="bar-empty" value={null} />
        </section>
        <section data-group="picker" className="grid w-80 grid-cols-[auto_1fr] gap-2">
          <Picker label="Session" value="seven_day" options={WINDOWS} onChange={() => {}} />
          <Picker
            label="Window"
            value=""
            options={WINDOWS}
            onChange={() => {}}
            placeholder="Pick…"
            className="max-w-full justify-self-start"
          />
          <Picker label="Task" value="" options={[]} onChange={() => {}} placeholder="Pick…" />
        </section>
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
