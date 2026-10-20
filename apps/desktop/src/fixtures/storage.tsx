/** Settings › Storage over a faked daemon. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsView } from "@/app/settings/SettingsView";
import { StoragePage } from "@/app/settings/storage/StoragePage";
import { SidebarProvider } from "@/components/layout/sidebar";
import { TooltipProvider } from "@/components/overlays/tooltip";
import { surface } from "@/fixtures/look";
import type { CleanItem, ProjectUsage, Request, StorageReport } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { emitCleanProgress, useStorageUi } from "@/state/storage";
import { useApp } from "@/state/store";

/*
 * ?state=rest|review|removing|done|opened|all stops in that state, for screenshots. ?drive=1
 * goes through every state (the review ticked and unticked, removing with how far it got,
 * done and what was removed, a project opened and freed up alone, all projects searched,
 * sorted and paged, the projects not in use freed up, Manage and Uninstall) and writes what it
 * saw to #storage-result (StoragePageRender.test.ts reads it). ?drive=look reads Settings ›
 * General's card and button, then Storage's cards and its Review button, to #storage-result too.
 */
const params = new URLSearchParams(location.search);
const look = params.get("drive") === "look";
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

const KB = 1024;
const MB = 1024 * KB;
const DAY = 24 * 60 * 60 * 1000;
const NOW = Date.now();

function item(id: string, fields: Partial<CleanItem>): CleanItem {
  return {
    id,
    category: "agentFiles",
    label: id,
    path: null,
    bytes: 0,
    reason: "",
    checked: true,
    selectable: true,
    toTrash: false,
    badges: [],
    projectId: null,
    ...fields,
  };
}

function project(name: string, path: string, mb: number, daysAgo: number, fields: Partial<ProjectUsage> = {}): ProjectUsage {
  const bytes = Math.round(mb * MB);
  const blobs = Math.round(bytes * 0.0605);
  return {
    projectId: name,
    name,
    path,
    repoFound: true,
    lastUsedMs: NOW - daysAgo * DAY,
    conversations: 12,
    archived: 0,
    worktreesBytes: 0,
    brainBytes: bytes - blobs,
    blobsBytes: blobs,
    scratchBytes: 0,
    ...fields,
  };
}

// The design's numbers: 101 projects, 38 of them not used in three months.
const named = [
  project("brigadier", "/Users/me/code/brigadier", 30.1, 0, {
    conversations: 142,
    archived: 6,
    brainBytes: Math.round(28.0 * MB),
    blobsBytes: Math.round(2.1 * MB),
  }),
  project("website", "/Users/me/code/website", 12.4, 1),
  project("infra-scripts", "/Users/me/code/infra-scripts", 6.4, 4),
  project("notes", "/Users/me/Documents/notes", 3.5, 14),
  project("design-tokens", "/Users/me/code/design-tokens", 2.8, 150),
  project("mobile-app", "/Users/me/code/mobile-app", 2.3, 21),
  project("old-landing", "/Users/me/code/old-landing", 1.9, 240),
];
const idle = Array.from({ length: 36 }, (_, i) =>
  project(`archive-${String(i + 1).padStart(2, "0")}`, `/Users/me/code/archive-${i + 1}`, 5.1 / 36, 100 + i * 5),
);
const used = Array.from({ length: 58 }, (_, i) =>
  project(`tool-${String(i + 1).padStart(2, "0")}`, `/Users/me/code/tool-${i + 1}`, 15.2 / 58, 2 + (i % 60)),
);
const projects = [...named, ...idle, ...used];

const sessionFiles = [
  item("inspector", { label: "Inspector session that's gone", reason: "What it ran in and its session files. Branches stay." }),
  item("claude-1", {
    label: "Claude Code session",
    reason: "Its data folder is gone; it used Brigadier's tools",
    path: "/private/tmp/brigadier-polish-preview",
    bytes: 268 * KB,
  }),
  item("claude-2", {
    label: "Claude Code session",
    reason: "Its data folder is gone; it used Brigadier's tools",
    path: "/private/tmp/brigadier-t3-preview/data",
    bytes: Math.round(2.2 * MB),
  }),
  item("codex-1", {
    label: "Their data folder is gone; Codex recorded Brigadier as the app that started them",
    reason: "Its data folder is gone; Codex recorded Brigadier as the app that started them",
    path: "/private/tmp/codex-preview-c2/data",
    bytes: 340 * KB,
  }),
  // Nine more, two of them brigadier's own (1.2 MB of it can go).
  ...Array.from({ length: 9 }, (_, i) =>
    item(`claude-more-${i + 1}`, {
      label: "Claude Code session",
      reason: "Its data folder is gone; it used Brigadier's tools",
      path: `/private/tmp/brigadier-preview-${i + 1}/data`,
      bytes: i < 2 ? Math.round(0.6 * MB) : Math.round(((5.8 - 2.2 - 1.2) * MB - 268 * KB) / 7),
      projectId: i < 2 ? "brigadier" : null,
    }),
  ),
];

const report: StorageReport = {
  scanId: "scan-1",
  dataDir: "/Users/me/Library/Application Support/Brigadier",
  totalBytes: Math.round(245.5 * MB),
  cleanableBytes: Math.round(40 * MB),
  projects,
  shared: [
    { part: "models", bytes: Math.round(124.7 * MB) },
    { part: "logs", bytes: 444 * KB },
    { part: "other", bytes: Math.round(40.7 * MB) },
  ],
  items: [
    item("tmp", {
      category: "temporary",
      label: "Temporary files",
      reason: "Files Brigadier made while it worked.",
      path: "/Users/me/Library/Application Support/Brigadier/tmp",
      bytes: Math.round(33.9 * MB),
    }),
    ...sessionFiles,
    item("worktrees", {
      category: "finishedWork",
      label: "Empty work folder of a project",
      reason: "A project's folder of work folders with nothing left in it.",
      path: "/Users/me/Library/Application Support/Brigadier/worktrees/website",
    }),
  ],
  kept: [
    { category: "finishedWork", label: "Work folders of open sessions", bytes: 24 * KB, reason: "Their sessions are still open." },
    { category: "oldLogs", label: "Today's logs", bytes: 16 * KB, reason: "Brigadier writes to them now." },
  ],
};

useApp.setState((state) => ({ info: { ...state.info, platform: "macos" } as typeof state.info }));

const calls: string[] = [];
let scans = 0;
let cleaned = false;
let finishClean: (() => void) | null = null;
mockIPC((command, payload) => {
  if (command === "uninstall_app") return { identifier: "app.brigadier.fixture" };
  if (command === "reveal_path") {
    calls.push(`reveal ${(payload as { path: string }).path}`);
    return null;
  }
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "scanStorage": {
      scans++;
      // After a clean, what it removed is gone.
      const scanned = cleaned
        ? { ...report, totalBytes: Math.round(205.8 * MB), items: report.items.filter((i) => i.id === "codex-1") }
        : report;
      return { method: req.method, report: { ...scanned, scanId: `scan-${scans}` } };
    }
    case "cleanStorage": {
      calls.push(`clean ${req.items.length}`);
      const answer = {
        method: req.method,
        report: {
          removed: req.items.length,
          reclaimedBytes: Math.round(39.7 * MB),
          trashedBytes: 0,
          codexThreadsDeleted: 0,
          failures: [],
        },
      };
      return new Promise((resolve) => {
        finishClean = () => {
          cleaned = true;
          resolve(answer);
        };
      });
    }
    case "freeUpProjects":
      calls.push(`free up ${req.ids.length} projects`);
      for (const freed of report.projects.filter((p) => req.ids.includes(p.projectId))) {
        Object.assign(freed, { conversations: 0, archived: 0, brainBytes: 0, blobsBytes: 0 });
      }
      return {
        method: req.method,
        report: {
          freed: req.ids.length,
          conversationsDeleted: 456,
          reclaimedBytes: Math.round(1.2 * MB),
          trashedBytes: Math.round(8.6 * MB),
          keptBranches: [],
          failures: [],
        },
      };
    default:
      return { method: req.method };
  }
});

const q = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) =>
  root.querySelector<T>(selector);
const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find(
    (element) => text(element) === name || element.getAttribute("aria-label") === name,
  );
const click = (name: string, root?: ParentNode) => {
  const found = button(name, root);
  if (!found) throw new Error(`no button “${name}”`);
  found.click();
};
const card = () => q("[data-free-up]");
const projectRows = () => all("[data-project]").map((row) => row.dataset.project);
const setInput = (input: HTMLInputElement, value: string) => {
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};
/** The text of each cell of a row, by its leaf elements. */
const leaves = (root: Element | null) =>
  root
    ? all("span, h2, p, input, button", root)
        .filter((element) => element.children.length === 0 || element.tagName === "BUTTON")
        // A button's only words (a select's value) are its own, read once.
        .filter((element) => !(element.parentElement?.tagName === "BUTTON" && text(element.parentElement) === text(element)))
        .map((element) => (element instanceof HTMLInputElement ? element.placeholder : text(element)))
        .filter(Boolean)
    : [];

type Step = [state: string | null, run: () => void];

/** Settings › General's card and button, then Storage's cards and Review, as the browser drew them. */
function Look() {
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    openSettings("general");
    void (async () => {
      await pause(600);
      seen.general = {
        card: surface(q('[data-slot="settings-card"]')),
        button: surface(q('[data-slot="settings-card"] button[data-variant="soft"]')),
      };
      openSettings("storage");
      await pause(800);
      seen.storage = { cards: all('[data-slot="settings-card"]').map(surface), button: surface(button("Review")) };
      setResult(JSON.stringify(seen));
    })();
  }, []);
  return (
    <TooltipProvider>
      <SidebarProvider>
        <div className="bg-background text-foreground flex h-screen w-full">
          <main className="flex h-full min-w-0 flex-1">
            <div className="min-w-0 flex-1">{page && <SettingsView page={page} />}</div>
          </main>
        </div>
      </SidebarProvider>
      {result && <pre id="storage-result">{result}</pre>}
    </TooltipProvider>
  );
}

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    const stop = params.get("state");
    const drive = params.has("drive");
    if (!stop && !drive) return;
    const seen: Record<string, unknown> = {};
    const steps: Step[] = [
      [
        "rest",
        () => {
          seen.title = text(q("h1"));
          seen.legend = all("[data-legend]").map(text);
          seen.usage = text(q("[data-usage]"));
          seen.rest = leaves(card());
          seen.top = projectRows();
          seen.more = leaves(q('[data-project="more"]'));
          seen.shared = leaves(q('[data-project="shared"]'));
          seen.sections = all("section h2").map(text);
          click("Review");
        },
      ],
      [null, () => click("Open Agent session files")],
      [null, () => click("Their data folder is gone; Codex recorded Brigadier as the app that started them")],
      [
        "review",
        () => {
          seen.review = all("[data-group]").map((group) => leaves(group.firstElementChild));
          seen.items = all("[data-item]").map((row) => ({
            text: leaves(row).join(" | "),
            ticked: q('[role="checkbox"]', row)?.getAttribute("aria-checked"),
          }));
          seen.footer = leaves(card()?.lastElementChild ?? null);
          click("Show 9 more");
        },
      ],
      [
        null,
        () => {
          seen.afterMore = all("[data-item]").length;
          click("Free up 39.7 MB");
        },
      ],
      [null, () => emitCleanProgress({ scanId: "scan-1", done: 9, total: 14 })],
      [
        "removing",
        () => {
          seen.removing = leaves(card());
          finishClean?.();
        },
      ],
      [null, () => {}],
      [
        "done",
        () => {
          seen.done = leaves(card());
          click("What was removed");
        },
      ],
      [
        null,
        () => {
          seen.removed = all("[data-removed] > div").length;
          click("Refresh");
          cleaned = false;
        },
      ],
      [
        null,
        () => {
          seen.afterRefresh = q("[data-free-up]")?.dataset.freeUp;
          click("Refresh");
        },
      ],
      [null, () => (q('[data-project="brigadier"] > button') as HTMLButtonElement).click()],
      [
        "opened",
        () => {
          const opened = q('[data-project="brigadier"]');
          seen.opened = all("[data-part-row]", opened ?? document).map((row) => leaves(row));
          seen.openedFooter = leaves(opened?.lastElementChild?.lastElementChild ?? null);
          click("Free up 1.2 MB");
        },
      ],
      [
        null,
        () => {
          seen.projectReview = leaves(card()?.lastElementChild ?? null);
          click("Cancel");
        },
      ],
      [null, () => (q('[data-project="brigadier"] > button') as HTMLButtonElement).click()],
      [null, () => click("Show all")],
      [
        "all",
        () => {
          const list = q("[data-projects='all']");
          seen.allHead = leaves(list?.firstElementChild ?? null);
          seen.unused = leaves(q("[data-unused]"));
          seen.allRows = all("[data-project]", list ?? document)
            .slice(0, 7)
            .map((row) => leaves(row.firstElementChild));
          seen.allCount = all("[data-project]", list ?? document).length;
          seen.paging = leaves(list?.lastElementChild ?? null);
          click("Show 20 more");
        },
      ],
      [
        null,
        () => {
          seen.afterPage = all("[data-projects='all'] [data-project]").length;
          setInput(q<HTMLInputElement>("[data-projects='all'] input")!, "note");
        },
      ],
      [
        null,
        () => {
          seen.searched = projectRows();
          setInput(q<HTMLInputElement>("[data-projects='all'] input")!, "");
          click("Free up 9.8 MB", q("[data-unused]") ?? document);
        },
      ],
      [
        null,
        () => {
          seen.confirm = leaves(q("[data-unused]"));
          click("Free up 9.8 MB", q("[data-unused]") ?? document);
        },
      ],
      [null, () => {}],
      [
        null,
        () => {
          seen.unusedAfter = q("[data-unused]") ? leaves(q("[data-unused]")) : null;
          click("Manage");
        },
      ],
      [
        null,
        () => {
          seen.selection = useApp.getState().selection;
          click("Uninstall…");
        },
      ],
      [
        null,
        () => {
          seen.uninstall = useStorageUi.getState().uninstallOpen;
          seen.calls = calls;
          setResult(JSON.stringify(seen));
        },
      ],
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    void (async () => {
      await wait(400);
      for (const [state, run] of steps) {
        if (stopped) return;
        if (!drive && state === stop) {
          document.documentElement.dataset.state = stop ?? "";
          return;
        }
        try {
          run();
        } catch (error) {
          seen.error = `${state ?? "step"}: ${String(error)}`;
          setResult(JSON.stringify(seen));
          return;
        }
        await wait(200);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <main className="flex h-full min-w-0 flex-1">
          <div className="min-w-0 flex-1">
            <StoragePage />
          </div>
        </main>
      </div>
      {result && <pre id="storage-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(look ? <Look /> : <Fixture />);
