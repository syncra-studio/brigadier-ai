import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { LabeledDivider, TitlebarTips, Banner, EmptyState } from "@/components/shared";
import { OfferBody, OfferCard } from "@/app/conversation/OfferCard";
import { ToolBox } from "@/components/conversation";
import { Branch, Download, FeedbackGlyph, Folder, Info, Plus, Settings, X } from "@/components/icons";
import { TooltipIconButton } from "@/components/controls";
import { DropdownMenuLabel, itemBase, MenuRow } from "@/components/overlays";
import { cn } from "@/lib/utils";

/** A node's box and colours. */
function box(node: Element | null) {
  if (!node) return null;
  const style = getComputedStyle(node);
  return {
    height: Math.round(node.getBoundingClientRect().height * 10) / 10,
    paddingTop: parseFloat(style.paddingTop),
    paddingLeft: parseFloat(style.paddingLeft),
    radius: style.borderTopLeftRadius,
    background: style.backgroundColor,
    color: style.color,
    fontSize: style.fontSize,
  };
}

/** A colour a CSS declaration resolves to. */
function probe(css: string) {
  const node = document.createElement("span");
  node.style.cssText = `display: block; ${css}`;
  document.body.append(node);
  const style = getComputedStyle(node);
  const value = { width: style.width, background: style.backgroundColor, color: style.color, fontSize: style.fontSize, radius: style.borderTopLeftRadius, fontFamily: style.fontFamily };
  node.remove();
  return value;
}

/** The fill of a reference swatch in the banner group. */
function ref(name: string) {
  return getComputedStyle(document.querySelector(`[data-group="banner"] > [data-${name}]`)!).backgroundColor;
}

/** The look of each part the near-duplicates were merged into, read back for a render test. */
function measure() {
  const dividers = [...document.querySelectorAll('[data-group="divider"] > *')].map((divider) => ({
    ...box(divider),
    rules: [...divider.querySelectorAll(":scope > [data-rule]")].map((rule) => ({
      height: rule.getBoundingClientRect().height,
      background: getComputedStyle(rule).backgroundColor,
    })),
  }));
  const boxes = [...document.querySelectorAll('[data-group="box"] > *')].map((node) => ({
    ...box(node),
    border: parseFloat(getComputedStyle(node).borderTopWidth),
    font: getComputedStyle(node.querySelector("[data-selectable]")!).fontFamily,
    expand: [...node.querySelectorAll("button")].map((button) => button.textContent),
  }));
  const offers = [...document.querySelectorAll('[data-group="offer"] > section')].map((node) => {
    const tile = node.querySelector(":scope > div > span");
    return { ...box(node), tile: tile && box(tile), tileWidth: tile?.getBoundingClientRect().width };
  });
  const labels = [...document.querySelectorAll('[data-group="label"] > *')].map((node) => ({
    ...box(node),
    tag: node.tagName.toLowerCase(),
    case: getComputedStyle(node).textTransform,
    mono: getComputedStyle(node).fontFamily === probe("font-family: var(--font-mono)").fontFamily,
  }));
  const rows = [...document.querySelectorAll('[data-group="row"] > *')].map((node) => ({
    height: node.getBoundingClientRect().height,
    icon: node.querySelector('[data-slot="menu-row-icon"] svg')?.getBoundingClientRect().width ?? null,
    hint: node.querySelector('[data-slot="menu-row-hint"]')?.textContent ?? null,
    lines: node.querySelectorAll(".truncate").length,
  }));
  const empties = [...document.querySelectorAll('[data-group="empty"] [data-slot="empty-state"]')].map((node) => ({
    ...box(node),
    role: node.getAttribute("role"),
    align: getComputedStyle(node).textAlign,
    icon: node.querySelector("svg")?.getBoundingClientRect().width ?? null,
  }));
  const banners = [...document.querySelectorAll('[data-group="banner"] [data-slot="banner"]')].map((node) => ({
    ...box(node),
    role: node.getAttribute("role"),
    border: parseFloat(getComputedStyle(node).borderBottomWidth),
    dismiss: node.querySelector("button")?.textContent ?? null,
  }));
  const icons = [...document.querySelectorAll('[data-group="icon"] button')].map((node) => ({
    ...box(node),
    label: node.getAttribute("aria-label"),
    pressed: node.getAttribute("aria-pressed"),
    dot: node.querySelector("span.rounded-full") !== null,
  }));
  return {
    icon: {
      icons,
      corner: probe("border-radius: var(--radius-toolbar-button)").radius,
      strip: probe("border-radius: var(--radius-strip-button)").radius,
      navRow: parseFloat(probe("width: var(--spacing-nav-row)").width),
    },
    banner: {
      banners,
      wash: ref("wash"),
      red: ref("red"),
      destructive: probe("color: var(--destructive)").color,
      textSm: probe("font-size: var(--text-sm)").fontSize,
    },
    empty: {
      empties,
      dim: getComputedStyle(document.querySelector('[data-group="empty"] > [data-dim]')!).color,
      textSm: probe("font-size: var(--text-sm)").fontSize,
      navRow: parseFloat(probe("width: var(--spacing-nav-row)").width),
      iconMd: parseFloat(probe("width: var(--spacing-icon-md)").width),
      iconLg: parseFloat(probe("width: var(--spacing-icon-lg)").width),
    },
    row: { rows, iconMd: parseFloat(probe("width: var(--spacing-icon-md)").width) },
    label: { labels, muted: probe("color: var(--muted-foreground)").color, textSm: probe("font-size: var(--text-sm)").fontSize },
    offer: {
      offers,
      radius: probe("border-radius: var(--radius-question)").radius,
      tile: parseFloat(probe("width: var(--spacing-icon-button-md)").width),
    },
    box: {
      boxes,
      surface: probe("background: var(--code-surface)").background,
      radius: probe("border-radius: var(--radius-control)").radius,
      mono: probe("font-family: var(--font-mono)").fontFamily,
    },
    divider: {
      dividers,
      border: probe("background: var(--border)").background,
      muted: probe("color: var(--muted-foreground)").color,
      textSm: probe("font-size: var(--text-sm)").fontSize,
    },
  };
}

function Fixture() {
  useEffect(() => {
    // After a frame: a clamped box finds it is clamped once it is laid out.
    const result = document.createElement("pre");
    const frame = requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        result.id = "merged-parts-result";
        result.textContent = JSON.stringify(measure());
        document.body.append(result);
      }),
    );
    return () => {
      cancelAnimationFrame(frame);
      result.remove();
    };
  }, []);
  return (
    <div className="flex w-160 flex-col gap-6 p-6">
      <section data-group="divider" className="flex flex-col">
        <LabeledDivider>Claude usage limit reset · 9:39 PM</LabeledDivider>
        <LabeledDivider role="status">
          <Info aria-hidden className="size-icon-sm shrink-0" />
          <span className="min-w-0 text-center">Codex usage limit reached. It resets at 9:39 PM</span>
          <button type="button" className="text-link hover:text-link/80 shrink-0">Try again</button>
        </LabeledDivider>
        <LabeledDivider>
          <span className="flex items-center gap-1.5">
            <Branch className="size-icon-sm" />
            Continued from chat
          </span>
        </LabeledDivider>
      </section>
      <section data-group="icon" className="flex gap-2">
        <TitlebarTips>
          <TooltipIconButton tooltip="Close"><X /></TooltipIconButton>
          <TooltipIconButton look="toolbar" tooltip="New tab" pressed><Plus /></TooltipIconButton>
          <TooltipIconButton look="strip" tooltip="Settings" dot="bg-warning"><Settings /></TooltipIconButton>
        </TitlebarTips>
      </section>
      <section data-group="banner" className="flex flex-col">
        <span data-wash className="bg-foreground/5" />
        <span data-red className="bg-destructive/10" />
        <Banner text="The shell exited." />
        <Banner tone="error" text="Could not open the page" onDismiss={() => {}} />
      </section>
      <section data-group="empty" className="flex w-80 flex-col">
        <ul>
          <EmptyState as="li" title="No sessions yet" />
        </ul>
        <EmptyState variant="card" icon={Info} title="Nothing yet." />
        <span data-dim className="text-foreground/65" />
        <EmptyState variant="centered" icon={Download} title="No downloads yet" detail="Files you download show here." />
      </section>
      <section data-group="row" className="flex w-80 flex-col">
        <div className={itemBase}>
          <MenuRow icon={<Folder />} title="Work in a project" detail="Its own worktree" />
        </div>
        <div className={cn(itemBase, "[&_svg]:size-icon-md")}>
          <MenuRow icon={<Folder />} title="brigadier-ai" hint="⌘1" mono />
        </div>
        <div className={cn(itemBase, "h-auto rounded-xl")}>
          <MenuRow twoLine title="Opus 5.5" hint="Default" detail="For most work" />
        </div>
      </section>
      <section data-group="label" className="flex flex-col">
        <DropdownMenuLabel>Model</DropdownMenuLabel>
        <DropdownMenuLabel variant="heading">Effort</DropdownMenuLabel>
        <DropdownMenuLabel variant="heading" asChild>
          <h2>Pinned</h2>
        </DropdownMenuLabel>
      </section>
      <section data-group="offer" className="flex flex-col">
        <OfferCard label="Send this to Brigadier’s developers?" kind="feedbackOffered" icon={<FeedbackGlyph />} title="Send this to Brigadier’s developers?" line="A report of what went wrong">
          <OfferBody title="Uploads stall at 99%" text="The progress bar stops just before the end." />
        </OfferCard>
        <OfferCard label="Send these to Brigadier’s developers?" kind="feedbackOffered" icon={<FeedbackGlyph />} title="Send these to Brigadier’s developers?" line="2 reports" />
      </section>
      <section data-group="box" className="flex flex-col gap-2">
        <ToolBox title="Output">{"Compiling brigadier v0.1.0\nFinished in 2.3s"}</ToolBox>
        <ToolBox clamp>{"git push origin main\nline 2\nline 3\nline 4\nline 5"}</ToolBox>
        <ToolBox prose>Look into the flaky upload test and say what makes it fail.</ToolBox>
      </section>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
