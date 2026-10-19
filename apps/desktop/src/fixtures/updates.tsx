/**
 * The sidebar over a faked daemon and shell: the Chats section's "+", updates that leave by
 * themselves, and Brigadier's own update pill from offered to "Restart to update".
 */
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { AppSidebar, AppStrip } from "@/app/AppSidebar";
import { SidebarFoot } from "@/app/SidebarFoot";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { fixtureClick } from "@/fixtures/input";
import type { AccountsView, EventEnvelope, Request, UpdateItem, UpdateTarget } from "@/ipc/generated";
import { APP_UPDATE_EVENT, type AppUpdate, startAppUpdate } from "@/state/appUpdate";
import { applyUpdateEvents } from "@/state/updates";
import { useApp } from "@/state/store";

/*
 * ?drive=1 clicks the Chats "+", runs a lone agent's update from the pill, runs one of two from
 * the list, and has one fail to start, writing what it saw to #updates-result
 * (SidebarUpdatesRender.test.ts reads it). ?narrow=1 narrows the sidebar as far as it goes, with
 * the two updates and a nightly update ready, and writes whether its pill stays inside, clear of
 * the agents' pill. ?frames=1 (under renderFixtureLive) has the shell offer a nightly update, clicks
 * its pill, has the download take a moment and clicks the restart pill, recording the foot's icons
 * on every frame into #app-update-frames. ?fail=1 has the download fail, then clicks again.
 * ?app=available|updating|failed|ready shows the nightly update so, beside one agent update, for
 * screenshots (with ?collapsed=1 on the strip). Without any, the page shows two updates.
 */
const params = new URLSearchParams(location.search);

const VERSION = "0.1.0-nightly.202610200130";
/** How long the download takes after the click under ?frames=1. */
const DOWNLOAD_MS = 600;
/** The shell's commands the page called, in turn. */
const shellCalls: string[] = [];
const shell = (update: AppUpdate | null) => {
  void emit(APP_UPDATE_EVENT, update);
};

const calls: Request[] = [];
let failNext = false;
mockIPC((command, payload) => {
  if (command === "app_update") {
    const state = params.get("app");
    return state ? ({ state, version: VERSION } as AppUpdate) : null;
  }
  if (command === "take_app_update" || command === "restart_to_update") {
    shellCalls.push(command);
    if (command === "take_app_update") {
      // As the shell does: busy at once, then ready (or failed) once the download ends.
      shell({ state: "updating", version: VERSION });
      const failed = params.has("fail") && shellCalls.length === 1;
      setTimeout(() => shell({ state: failed ? "failed" : "ready", version: VERSION }), DOWNLOAD_MS);
    }
    return null;
  }
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  if (req.method === "getAccounts") return { method: req.method, accounts: { accounts: [], warnings: [] } satisfies AccountsView };
  if (req.method === "runUpdate" && failNext) {
    failNext = false;
    throw new Error("Couldn't reach npm");
  }
  return { method: req.method };
}, { shouldMockEvents: true });
void startAppUpdate();

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

if (params.has("frames") || params.has("fail") || params.has("app")) publish([item("codex", "1.1.0", "available")]);
else if (!params.has("drive")) publish([item("claude", "2.0.0", "available"), item("codex", "1.1.0", "available")]);

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
  const [frames, setFrames] = useState<string | null>(null);
  useEffect(() => {
    if (params.has("frames")) void recordFrames().then(setFrames);
    if (params.has("fail")) void failAndRetry().then(setResult);
  }, []);
  useEffect(() => {
    if (!params.has("narrow")) return;
    const wrapper = q('[data-slot="sidebar-wrapper"]');
    wrapper?.style.setProperty("--sidebar-width", "var(--spacing-sidebar-min)");
    shell({ state: "ready", version: VERSION });
    // Past the sidebar's and the item's own motion.
    const timer = window.setTimeout(() => {
      const restart = q('button[aria-label="Restart to update"]');
      const chip = q('button[aria-label$="updates available"]');
      const sidebar = restart?.closest(".sidebar-panel-width")?.getBoundingClientRect();
      const at = restart?.getBoundingClientRect();
      const beside = chip?.getBoundingClientRect();
      setResult(
        JSON.stringify({
          open: wrapper?.dataset.state,
          inside: Boolean(at && beside && sidebar && Math.max(at.right, beside.right) <= sidebar.right + 0.5),
          // Not on top of the updates pill: after it on its line, or on a line below.
          apart: Boolean(at && beside && (at.left >= beside.right - 0.5 || at.top >= beside.bottom - 0.5)),
        }),
      );
    }, 1000);
    return () => window.clearTimeout(timer);
  }, []);
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
        <SidebarProvider className="relative min-h-0 flex-1 flex-col" defaultOpen={!params.has("collapsed")}>
          <div className="relative flex min-h-0 min-w-0 flex-1">
            <SidebarPanel strip={<AppStrip />} foot={<SidebarFoot />}>
              <AppSidebar />
            </SidebarPanel>
            <main className="bg-background flex-1" />
          </div>
        </SidebarProvider>
      </div>
      {result && <pre id="updates-result">{result}</pre>}
      {frames && <pre id="app-update-frames">{frames}</pre>}
    </TooltipProvider>
  );
}

/** Brigadier's own update pill, whatever its state. */
const appPill = () =>
  all<HTMLButtonElement>("button[aria-label]").find((button) =>
    /Brigadier|Restart to update/.test(button.getAttribute("aria-label") ?? ""),
  );

/** One of the foot's icons on one frame: its name, where it is drawn and how big. */
type Box = { label: string; x: number; y: number; width: number; height: number; opacity: number; icons: number };
type Frame = { ms: number; boxes: Box[] };

const round = (value: number) => Math.round(value * 2) / 2;

/** The foot's icons on every frame for `ms` from `act`. */
function record(ms: number, act: () => unknown): Promise<Frame[]> {
  return new Promise((resolve) => {
    const seen: Frame[] = [];
    const start = performance.now();
    const tick = () => {
      const foot = q('button[aria-label="Settings"]')?.parentElement;
      const boxes = all<HTMLButtonElement>("button[aria-label]", foot ?? document.body).map((button) => {
        const rect = button.getBoundingClientRect();
        // Drawn as faded as its most faded wrapper (an item growing in fades with it).
        let opacity = 1;
        for (let at: HTMLElement | null = button; at && at !== foot; at = at.parentElement) {
          opacity *= Number(getComputedStyle(at).opacity);
        }
        return {
          label: button.getAttribute("aria-label") ?? "",
          x: round(rect.x),
          y: round(rect.y),
          width: round(rect.width),
          height: round(rect.height),
          opacity: Math.round(opacity * 100) / 100,
          icons: button.querySelectorAll("svg").length,
        };
      });
      seen.push({ ms: Math.round(performance.now() - start), boxes });
      if (performance.now() - start < ms) requestAnimationFrame(tick);
      else resolve(seen);
    };
    requestAnimationFrame(tick);
    void act();
  });
}

/** Offered, clicked, downloading, ready, restarted: the foot on every frame of it. */
async function recordFrames(): Promise<string> {
  await wait(800);
  const appear = await record(800, () => shell({ state: "available", version: VERSION }));
  const updating = await record(DOWNLOAD_MS + 600, () => fixtureClick(appPill()!));
  await fixtureClick(appPill()!);
  await wait(100);
  return JSON.stringify({ appear, updating, calls: shellCalls });
}

/** A download that fails: the pill says so, and a click tries again. */
async function failAndRetry(): Promise<string> {
  await wait(800);
  shell({ state: "available", version: VERSION });
  await wait(400);
  await fixtureClick(appPill()!);
  await wait(DOWNLOAD_MS + 200);
  const failed = appPill()?.getAttribute("aria-label");
  await fixtureClick(appPill()!);
  await wait(100);
  const again = appPill()?.getAttribute("aria-label");
  await wait(DOWNLOAD_MS + 200);
  return JSON.stringify({ failed, again, after: appPill()?.getAttribute("aria-label"), calls: shellCalls });
}

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
