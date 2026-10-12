/** The sidebar over a faked daemon: the Chats section's "+", and updates that leave by themselves. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AppSidebar, AppStrip } from "@/app/AppSidebar";
import { SidebarFoot } from "@/app/SidebarFoot";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { EventEnvelope, Request, UpdateItem, UpdateTarget } from "@/ipc/generated";
import { applyUpdateEvents } from "@/state/updates";
import { useApp } from "@/state/store";

/*
 * ?drive=1 clicks the Chats "+", runs a lone agent's update from the pill, runs one of two from
 * the list, and has one fail to start, writing what it saw to #updates-result
 * (SidebarUpdatesRender.test.ts reads it). Without it, the page shows two updates, for screenshots.
 */
const params = new URLSearchParams(location.search);

const calls: Request[] = [];
let failNext = false;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  if (req.method === "runUpdate" && failNext) {
    failNext = false;
    throw new Error("Couldn't reach npm");
  }
  return { method: req.method };
});

useApp.setState({
  catalogLoaded: true,
  connection: { status: "connected", daemon: null, reason: null },
  projects: {
    web: {
      id: "web",
      name: "storefront-web",
      repos: [{ path: "/w/storefront-web" }],
      createdAtMs: 1,
    } as never,
  },
});

function item(target: UpdateTarget, latest: string, progress: UpdateItem["progress"]["type"]): UpdateItem {
  return {
    target,
    current: "1.0.0",
    latest,
    action: { type: "run", command: `npm i -g ${target}` },
    progress: { type: progress } as UpdateItem["progress"],
  };
}

/** The daemon's word that the updates now stand so. */
function publish(items: UpdateItem[]): void {
  applyUpdateEvents([
    { seq: 0, stream: "updates", streamSeq: 0, event: { type: "updatesChanged", updates: { items, checkedAtMs: 1 } } },
  ] as unknown as EventEnvelope[]);
}

if (!params.has("drive")) publish([item("claude", "2.0.0", "available"), item("codex", "1.1.0", "available")]);

const q = <E extends Element = HTMLElement>(selector: string, root: ParentNode = document) =>
  root.querySelector<E & HTMLElement>(selector);
const all = <E extends Element = HTMLElement>(selector: string, root: ParentNode = document) =>
  [...root.querySelectorAll<E & HTMLElement>(selector)];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
/** The foot's updates chip, by the start of its name. */
const pill = (name: string) =>
  all<HTMLButtonElement>("button[aria-label]").find((button) => button.getAttribute("aria-label")?.startsWith(name));
const menu = () => q('[data-slot="popover-content"]');
/** The list's rows still standing: each one's name. */
const rows = () =>
  all('[data-slot="popover-content"] > div:not([aria-hidden])')
    .map((row) => text(q(".truncate", row)))
    .filter(Boolean);

function Fixture() {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void | Promise<void>)[] = [
      // The Chats "+", made like the Projects one, starts a chat.
      () => {
        const section = (title: string) =>
          all("section").find((element) => text(q("button[aria-expanded]", element)).startsWith(title));
        const projectsPlus = q('button[aria-label="Add project"]', section("Projects"));
        const chatsPlus = q<HTMLButtonElement>('button[aria-label="New chat"]', section("Chats"));
        seen.chatsPlus = { there: Boolean(chatsPlus), sameLook: chatsPlus?.className === projectsPlus?.className };
        chatsPlus?.click();
        seen.selection = useApp.getState().selection;
      },
      // A lone agent's update: one click on the chip runs it, with no list.
      () => publish([item("codex", "1.1.0", "available")]),
      () => {
        seen.loneTip = pill("Update Codex")?.getAttribute("aria-label");
        pill("Update Codex")?.click();
      },
      () => {
        seen.loneMenu = Boolean(menu());
        seen.loneCalls = calls.filter((call) => call.method === "runUpdate");
        publish([item("codex", "1.1.0", "updating")]);
      },
      () => {
        seen.loneUpdating = pill("Updating")?.getAttribute("aria-label");
        publish([item("codex", "1.1.0", "updated")]);
      },
      () => {
        seen.loneUpdated = pill("Codex updated")?.getAttribute("aria-label") ?? null;
      },
      // Gone on its own once its check has shown a moment.
      () => wait(1400),
      () => {
        seen.loneAfter = pill("Codex") ? "still there" : "gone";
      },
      // Two updates: the list; one finishes and leaves while the list stays open.
      () => publish([item("claude", "2.0.0", "available"), item("codex", "1.2.0", "available")]),
      () => pill("Claude Code and Codex")?.click(),
      () => {
        seen.twoRows = rows();
        const codexRow = all('[data-slot="popover-content"] > div').find((row) => text(row).includes("Codex"));
        all<HTMLButtonElement>("button", codexRow).find((button) => text(button) === "Update")?.click();
      },
      () => {
        seen.twoCalls = calls.filter((call) => call.method === "runUpdate").length;
        publish([item("claude", "2.0.0", "available"), item("codex", "1.2.0", "updated")]);
      },
      () => {
        seen.finishedShowing = rows();
      },
      () => wait(1400),
      () => {
        seen.afterFirst = { rows: rows(), open: Boolean(menu()) };
        publish([item("claude", "2.0.0", "updated"), item("codex", "1.2.0", "updated")]);
      },
      () => wait(1600),
      () => {
        seen.afterLast = { open: Boolean(menu()), pill: pill("Claude") ? "still there" : "gone" };
      },
      // An update that can't start: the list opens on why.
      () => {
        failNext = true;
        publish([item("claude", "2.1.0", "available")]);
      },
      () => pill("Update Claude Code")?.click(),
      () => wait(300),
      () => {
        seen.failed = { open: Boolean(menu()), alert: text(q('[data-slot="popover-content"] [role="alert"]')) };
        setResult(JSON.stringify(seen));
      },
    ];
    let stopped = false;
    void (async () => {
      await wait(300);
      for (const step of steps) {
        if (stopped) return;
        await step();
        await wait(250);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-chrome flex h-screen flex-col">
        <SidebarProvider className="relative min-h-0 flex-1 flex-col" defaultOpen>
          <div className="relative flex min-h-0 min-w-0 flex-1">
            <SidebarPanel strip={<AppStrip />} foot={<SidebarFoot />}>
              <AppSidebar />
            </SidebarPanel>
            <main className="bg-background flex-1" />
          </div>
        </SidebarProvider>
      </div>
      {result && <pre id="updates-result">{result}</pre>}
    </TooltipProvider>
  );
}

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
