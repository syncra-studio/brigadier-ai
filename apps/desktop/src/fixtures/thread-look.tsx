/**
 * A standalone Vite entry, excluded from the app entry and production build: the thread's look
 * measured as computed styles, against the reference's. IPC is mocked, no daemon. The result goes
 * to `<pre id="thread-look-result">`; `stop=1` holds the page for a screenshot.
 *
 * - The column 736px wide; prose at weight 430; a web link's blue; a code block's 16px corners.
 * - A row: its object at 40% white; its chevron hidden at rest and shown while open; a group
 *   opening and closing over 300ms.
 * - A streaming reply: a list item, a rule and an image that arrive fade in over 700ms like words.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ActivityGroup, type Described, StepRow } from "@/app/conversation/activity/ActivityGroup";
import { stepWords } from "@/app/conversation/activity/words";
import { MarkdownBlock } from "@/components/assistant-ui/thread";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Request } from "@/ipc/generated";

const query = new URLSearchParams(location.search);
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  return { method: req.method };
});

const steps: { key: string; described: Described }[] = [
  { key: "a", described: { words: stepWords({ name: "Read", detail: "/repo/a.ts", status: "completed" }), status: "completed" } },
  { key: "b", described: { words: stepWords({ name: "Bash", detail: "npm test", status: "completed" }), status: "completed" } },
];
/** A local path: the reply's markdown drops `data:` images. */
const IMAGE = "/fixture-missing-chart.png";
const FIRST = "The fix is in.";
const MORE = `${FIRST}\n\n- one item\n\n---\n\n![chart](${IMAGE})\n\nSee [the docs](https://nodejs.org/en).\n\n\`\`\`ts\nconst a = 1;\n\`\`\`\n`;

const reply = { grow: () => {} };
function Streaming() {
  const [text, setText] = useState(FIRST);
  useEffect(() => {
    reply.grow = () => setText(MORE);
  }, []);
  return <MarkdownBlock text={text} streaming />;
}

function Page() {
  return (
    <TooltipProvider>
      <main className="bg-background text-foreground flex min-h-screen w-full flex-col gap-6 p-8">
        <div data-case="column" className="max-w-thread w-screen" />
        <section data-case="group">
          <ActivityGroup
            items={steps.map(({ key }) => ({ type: "step", key, step: key }))}
            live={false}
            describe={(key) => steps.find((each) => each.key === key)!.described}
            renderStep={(key) => <StepRow key={key} {...steps.find((each) => each.key === key)!.described} />}
          />
        </section>
        <section data-case="row">
          <StepRow {...steps[1]!.described} />
        </section>
        <section data-case="reply" className="max-w-thread">
          <Streaming />
        </section>
      </main>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Page />);

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
const at = (id: string) => document.querySelector<HTMLElement>(`[data-case=${id}]`)!;
const style = (element: Element | null | undefined) => (element ? getComputedStyle(element) : null);

/** A colour as its sRGB channels, painted on a canvas and read back. */
function channels(color: string | null | undefined): number[] | null {
  if (!color) return null;
  const canvas = document.createElement("canvas");
  canvas.width = 1;
  canvas.height = 1;
  const context = canvas.getContext("2d");
  if (!context) return null;
  context.fillStyle = color;
  context.fillRect(0, 0, 1, 1);
  return Array.from(context.getImageData(0, 0, 1, 1).data.subarray(0, 3));
}

async function measure() {
  for (const end = performance.now() + 8000; performance.now() < end && !at("reply")?.querySelector("p"); ) await frame();
  await wait(300);
  const out: Record<string, unknown> = {};
  out.column = Math.round(at("column").getBoundingClientRect().width);
  const trigger = at("group").querySelector<HTMLElement>("button");
  const chevron = () => trigger?.querySelector(":scope > svg:last-child") ?? null;
  out.chevronRest = style(chevron())?.opacity ?? null;
  trigger?.click();
  await frame();
  const content = at("group").querySelector("[data-slot=collapsible-content]");
  out.groupOpen = { duration: style(content)?.animationDuration ?? null, easing: style(content)?.animationTimingFunction ?? null };
  await wait(400);
  out.chevronOpen = style(chevron())?.opacity ?? null;
  out.object = style(at("row").querySelector(".text-row-object"))?.color ?? null;
  // The reply grows: what arrives fades in.
  reply.grow();
  for (const end = performance.now() + 4000; performance.now() < end && !at("reply").querySelector("img"); ) await frame();
  await frame();
  const fades = (selector: string) => {
    const element = at("reply").querySelector(selector);
    return { classed: !!element?.classList.contains("aui-fade-block"), duration: style(element)?.animationDuration ?? null };
  };
  out.fades = { li: fades("li"), hr: fades("hr"), img: fades("img") };
  await wait(1200);
  const prose = at("reply").querySelector(".aui-md");
  out.prose = style(prose)?.fontWeight ?? null;
  const link = at("reply").querySelector("a");
  out.link = { href: link?.getAttribute("href") ?? null, rgb: channels(style(link)?.color) };
  for (const end = performance.now() + 4000; performance.now() < end && !at("reply").querySelector("[data-slot=code-block], pre"); ) await frame();
  const block = at("reply").querySelector("pre")?.closest("div.border") ?? at("reply").querySelector("pre");
  out.codeRadius = Number.parseFloat(style(block)?.borderTopLeftRadius ?? "");
  return out;
}

void measure()
  .catch((error: unknown) => ({ error: String(error) }))
  .then(async (result) => {
    if (query.get("stop") === "1") await new Promise(() => {});
    const out = document.createElement("pre");
    out.id = "thread-look-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
