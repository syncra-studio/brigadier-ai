import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { LabeledDivider } from "@/components/assistant-ui/elements/labeled-divider";
import { ToolBox } from "@/components/assistant-ui/elements/tool-box";
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
  const value = { background: style.backgroundColor, color: style.color, fontSize: style.fontSize, radius: style.borderTopLeftRadius, fontFamily: style.fontFamily };
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
  const boxes = [...document.querySelectorAll('[data-group="box"] > *')].map((node) => ({
    ...box(node),
    border: parseFloat(getComputedStyle(node).borderTopWidth),
    font: getComputedStyle(node.querySelector("[data-selectable]")!).fontFamily,
    expand: [...node.querySelectorAll("button")].map((button) => button.textContent),
  }));
  return {
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
      <section data-group="box" className="flex flex-col gap-2">
        <ToolBox title="Output">{"Compiling brigadier v0.1.0\nFinished in 2.3s"}</ToolBox>
        <ToolBox clamp>{"git push origin main\nline 2\nline 3\nline 4\nline 5"}</ToolBox>
        <ToolBox prose>Look into the flaky upload test and say what makes it fail.</ToolBox>
      </section>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
