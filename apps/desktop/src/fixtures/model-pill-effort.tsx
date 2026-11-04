/**
 * A standalone Vite entry, excluded from the app entry and production build: the composer's
 * model pill for models with and without efforts, none picked (issue 670). OpenCode names no
 * default variant (its default is the provider's own), so its pill says Default, as Claude's and
 * Codex's say the effort they run at. What the pills read goes as JSON into
 * `<pre id="model-pill-effort-result">`.
 */
import { createRoot } from "react-dom/client";

import { TooltipProvider } from "@/components/overlays";
import { type ModelGroup, ModelSelector } from "@/components/shared";
import { model } from "@/fixtures/agents";
import type { ModelChoice } from "@/ipc/generated";

const groups: ModelGroup[] = [
  { provider: "claude", label: "Claude Code", unavailable: null, models: [model("opus", "Opus 5.5", { isDefault: true, efforts: ["low", "medium", "high"], defaultEffort: "high" })] },
  {
    provider: "opencode",
    label: "OpenCode",
    unavailable: null,
    models: [
      model("opencode-go/glm-5.3", "GLM-5.3", { isDefault: true, efforts: ["low", "medium", "high"] }),
      model("opencode/big-pickle", "Big Pickle"),
    ],
  },
];
const pills: [string, ModelChoice][] = [
  ["claude", { provider: "claude", model: "opus", effort: null }],
  ["opencode", { provider: "opencode", model: "opencode-go/glm-5.3", effort: null }],
  ["opencodeHigh", { provider: "opencode", model: "opencode-go/glm-5.3", effort: "high" }],
  ["noEfforts", { provider: "opencode", model: "opencode/big-pickle", effort: null }],
];

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const triggers = () => [...document.querySelectorAll<HTMLElement>("[data-slot=model-selector-trigger]")];
async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    const end = performance.now() + 10_000;
    while (triggers().length < pills.length) {
      if (performance.now() > end) throw new Error("timed out waiting for the pills");
      await sleep(16);
    }
    triggers().forEach((trigger, index) => {
      seen[pills[index]![0]] = trigger.textContent;
    });
  } catch (error) {
    seen.error = String(error);
  }
  const out = document.createElement("pre");
  out.id = "model-pill-effort-result";
  out.textContent = JSON.stringify(seen);
  document.body.append(out);
}

createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    {pills.map(([name, value]) => (
      <ModelSelector key={name} groups={groups} value={value} defaultChoice={pills[0]![1]} onChange={() => {}} />
    ))}
  </TooltipProvider>,
);
void drive();
