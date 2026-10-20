import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { LabeledDivider } from "@/components/assistant-ui/elements/labeled-divider";
import { Branch, Info } from "@/components/icons";

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
  node.style.cssText = css;
  document.body.append(node);
  const style = getComputedStyle(node);
  const value = { background: style.backgroundColor, color: style.color, fontSize: style.fontSize };
  node.remove();
  return value;
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
  return {
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
    const result = document.createElement("pre");
    result.id = "merged-parts-result";
    result.textContent = JSON.stringify(measure());
    document.body.append(result);
    return () => result.remove();
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
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
