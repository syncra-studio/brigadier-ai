/** Settings → Storage over a faked daemon: scans, the Review and Choose dialogs, and a clean. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { revealSetting } from "@/app/settings/reveal";
import { STORAGE_ROWS, StoragePage } from "@/app/settings/StoragePage";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { CleanItem, Request, StorageReport } from "@/ipc/generated";
import { useApp } from "@/state/store";

/*
 * ?drive=1 refreshes, reviews and frees up space, chooses items by hand, opens disk use per
 * project and Manage, and writes what it saw to #storage-result (StoragePageRender.test.ts
 * reads it). Without it, the page stays as it opens, for screenshots.
 */
const params = new URLSearchParams(location.search);
document.documentElement.dataset.density = params.get("density") === "compact" ? "compact" : "normal";
// Driven, nothing animates or blurs (see the archived chats fixture).
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}
const GB = 1024 ** 3;
const MB = 1024 ** 2;

function item(id: string, fields: Partial<CleanItem>): CleanItem {
  return {
    id,
    category: "finishedWork",
    label: id,
    path: null,
    bytes: 0,
    reason: "",
    checked: true,
    selectable: true,
    toTrash: false,
    badges: [],
    ...fields,
  };
}

const report: StorageReport = {
  scanId: "scan-1",
  dataDir: "/Users/me/Library/Application Support/Brigadier",
  totalBytes: 31 * GB,
  cleanableBytes: 0,
  projects: [
    { projectId: "web", name: "storefront-web", repoFound: true, worktreesBytes: 14 * GB, brainBytes: GB, blobsBytes: 2 * GB, scratchBytes: 200 * MB },
    { projectId: "api", name: "billing-api", repoFound: false, worktreesBytes: 4 * GB, brainBytes: 300 * MB, blobsBytes: 500 * MB, scratchBytes: 0 },
  ],
  shared: [
    { part: "database", bytes: 600 * MB },
    { part: "models", bytes: 3 * GB },
    { part: "logs", bytes: 900 * MB },
    { part: "personalBrain", bytes: 200 * MB },
    { part: "other", bytes: GB },
  ],
  items: [
    item("done", { label: "Work folder of “Fix login”", path: "/w/fix-login", bytes: 9 * GB, reason: "Its session ended and its branch is merged." }),
    item("build", { category: "buildFiles", label: "Build files of “Checkout page”", bytes: 2 * GB, reason: "Rebuilt when the session runs again." }),
    item("logs", { category: "oldLogs", label: "Logs older than 30 days", bytes: 400 * MB, reason: "Old logs." }),
    item("dirty", {
      label: "Work folder of “Draft”",
      bytes: 2 * GB,
      checked: false,
      reason: "Has unsaved changes, so it isn't part of the sweep.",
      badges: [{ type: "hasChanges" }],
    }),
    item("unmerged", {
      label: "Branch brigadier/b in storefront-web",
      checked: false,
      selectable: false,
      reason: "Has 3 commits not merged into main.",
      badges: [{ type: "notMerged", ahead: 3 }],
    }),
    item("model", { category: "models", label: "Speech model (base)", bytes: 150 * MB, checked: false, toTrash: true }),
  ],
  kept: [{ category: "finishedWork", label: "2 work folders of open sessions", bytes: 4 * GB, reason: "Their sessions are still open." }],
};

useApp.setState((state) => ({ info: { ...state.info, platform: "macos" } as typeof state.info }));

const calls: string[] = [];
let scans = 0;
// Set, the next scan is answered only when `release` is called.
let holdNextScan = false;
let release: (() => void) | null = null;
mockIPC((command, payload) => {
  if (command === "uninstall_app") return { identifier: "app.brigadier.fixture" };
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "scanStorage": {
      scans++;
      const answer = { method: req.method, report: { ...report, scanId: `scan-${scans}` } };
      if (!holdNextScan) return answer;
      holdNextScan = false;
      return new Promise((resolve) => {
        release = () => resolve(answer);
      });
    }
    case "cleanStorage":
      calls.push(`clean ${req.scanId} ${req.items.join(",")}`);
      return {
        method: req.method,
        report: {
          removed: req.items.length - 1,
          reclaimedBytes: 9 * GB,
          trashedBytes: 0,
          codexThreadsDeleted: 0,
          failures: [{ id: "build", label: "Build files of “Checkout page”", error: "a process runs in it" }],
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
const dialog = () => q('[role="dialog"]');
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find(
    (element) => text(element) === name || element.getAttribute("aria-label") === name,
  );
const row = (label: string) => all('[data-slot="settings-row"]').find((element) => element.dataset.setting === label);

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void)[] = [
      () => {
        seen.status = text(q('main [role="status"]'));
        seen.legend = all("main ul li").map(text);
        seen.freeRow = text(row("Free up space"));
        seen.scansAtOpen = scans;
        holdNextScan = true;
        button("Refresh")?.click();
      },
      () => {
        seen.scansAfterRefresh = scans;
        // Review while that scan is out; it lands with the dialog open and gives the items other
        // ids, so freeing up must name the scan the dialog shows.
        button("Review")?.click();
      },
      () => release?.(),
      () => {
        const open = dialog();
        seen.reviewWidth = Math.round(open?.getBoundingClientRect().width ?? 0);
        seen.review = {
          title: text(q("h2", open ?? document)),
          groups: all('[data-slot="storage-group"] > button', open ?? document).map(text),
          footer: text(q("p.text-xs", open ?? document)),
        };
        all<HTMLButtonElement>('[data-slot="storage-group"] > button', open ?? document)[0]?.click();
      },
      () => {
        seen.firstGroup = all('[data-slot="storage-group"] li', dialog() ?? document).map(text);
        button("Free up 11.4 GB", dialog() ?? document)?.click();
      },
      () => {
        seen.result = text(dialog());
        button("Done", dialog() ?? document)?.click();
      },
      () => {
        seen.afterDone = { dialog: dialog() !== null, scans };
        // A search result inside Advanced opens it.
        revealSetting(STORAGE_ROWS.usage.label);
      },
      () => {
        seen.revealed = q('[data-slot="settings-advanced"]')?.dataset.state;
        button("Choose…")?.click();
      },
      () => {
        const open = dialog();
        seen.choose = all('button[role="checkbox"]', open ?? document).map((box) => ({
          label: text(box.parentElement?.querySelector("label span")),
          checked: box.getAttribute("data-state") === "checked",
        }));
        // Add the folder with unsaved changes.
        all<HTMLButtonElement>('button[role="checkbox"]', open ?? document)
          .find((box) => text(box.parentElement?.querySelector("label span")) === "Work folder of “Draft”")
          ?.click();
      },
      () => {
        const remove = all<HTMLButtonElement>("button", dialog() ?? document).find((b) => text(b).startsWith("Remove"));
        seen.removeButton = text(remove);
        remove?.click();
      },
      () => {
        const open = dialog();
        seen.chosenReview = {
          title: text(q("h2", open ?? document)),
          groups: all('[data-slot="storage-group"] > button', open ?? document).map(text),
          action: text(all("button", open ?? document).find((b) => text(b).startsWith("Free up"))),
        };
        button("Cancel", open ?? document)?.click();
      },
      () => {
        seen.afterCancel = { dialog: dialog() !== null, calls: calls.length };
        button("View")?.click();
      },
      () => {
        seen.usage = all("tbody tr", dialog() ?? document).map((tr) => all("td", tr).map(text));
        button("Close", dialog() ?? document)?.click();
      },
      () => {
        button("Manage")?.click();
      },
      () => {
        seen.selection = useApp.getState().selection;
        seen.calls = calls;
        setResult(JSON.stringify(seen));
      },
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    void (async () => {
      await wait(300);
      for (const step of steps) {
        if (stopped) return;
        step();
        await wait(150);
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
if (root) createRoot(root).render(<Fixture />);
