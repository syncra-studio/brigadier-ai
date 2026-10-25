/** Settings → Archived over a seeded store, with restore and delete faked through the IPC mock. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { DeleteDialog } from "@/app/shell/dialogs/DeleteDialog";
import { ArchivedPage } from "@/app/settings/ArchivedPage";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, Project, Request } from "@/ipc/generated";
import { useApp } from "@/state/store";

/*
 * ?empty=1 shows the page with nothing archived. ?drive=1 searches, filters, sorts, picks
 * after filtering, uses the bulk bar, the "…" menu and the right-click menu, and writes what it
 * saw to #archived-result (ArchivedPageRender.test.ts reads it).
 */
const params = new URLSearchParams(location.search);
// Driven, nothing animates or blurs: an exit animation never ends in Chrome's virtual time, so a
// closed menu or dialog would stay mounted, and the single-process headless Chrome can crash on
// exit (SIGTRAP) after painting the dialog's backdrop blur.
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}
const DAY = 24 * 60 * 60 * 1000;
const now = Date.UTC(2026, 9, 10, 12);

function conversation(id: string, fields: Partial<Conversation>): Conversation {
  return {
    id,
    kind: "chat",
    projectId: null,
    title: id,
    pinnedAtMs: null,
    createdAtMs: now - 30 * DAY,
    updatedAtMs: now - DAY,
    setup: null,
    lifecycle: "archived",
    forkedFrom: null,
    sideOf: null,
   
    quotaWait: null,
    ...fields,
  };
}

const projects = {
  web: { id: "web", name: "storefront-web" },
  api: { id: "api", name: "billing-api" },
} as unknown as Record<string, Project>;

const seeded: Conversation[] = params.has("empty")
  ? []
  : [
      conversation("s1", { kind: "session", projectId: "web", title: "Checkout page redesign", createdAtMs: now - 9 * DAY, updatedAtMs: now - 2 * DAY }),
      conversation("s2", { kind: "session", projectId: "web", title: "Fix flaky cart tests", createdAtMs: now - 20 * DAY, updatedAtMs: now - 6 * DAY }),
      conversation("s3", { kind: "session", projectId: "api", title: "Add invoice export", createdAtMs: now - 3 * DAY, updatedAtMs: now - 3 * DAY }),
      conversation("s4", { kind: "session", projectId: "api", title: "Billing webhooks retry", createdAtMs: now - 40 * DAY, updatedAtMs: now - 12 * DAY }),
      conversation("c1", { title: "Takeaway app plan", createdAtMs: now - 50 * DAY, updatedAtMs: now - 45 * DAY }),
      conversation("c2", { title: "Trip to Lisbon", createdAtMs: now - 4 * DAY, updatedAtMs: now - 4 * DAY }),
      conversation("live", { title: "Not archived", lifecycle: "active", updatedAtMs: now }),
    ];

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  projects,
  conversations: Object.fromEntries(seeded.map((c) => [c.id, c])),
}));

const calls: string[] = [];
let releasePreview: (() => void) | null = null;
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "restore": {
      calls.push(`restore ${req.ids.join(",")}`);
      const all = useApp.getState().conversations;
      return {
        method: req.method,
        outcomes: req.ids.map((id) => ({ id, conversation: { ...all[id], lifecycle: "active" }, error: null })),
      };
    }
    case "previewDelete":
      // Deleting several, the check is held until the driver lets it answer: Delete mustn't wait.
      if (req.ids.length > 1) {
        return new Promise((resolve) => {
          releasePreview = () =>
            resolve({
              method: req.method,
              branches: [{ repo: "/repo", name: "brigadier/912a9469/session", unknown: false }],
              files: [],
            });
        });
      }
      return { method: req.method, branches: [], files: [] };
    case "delete":
      calls.push(`delete ${req.ids.join(",")}`);
      return { method: req.method, outcomes: req.ids.map((id) => ({ id, conversation: null, error: null })) };
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
const groups = () =>
  all("main section").map((section) => ({
    label: section.getAttribute("aria-label"),
    rows: all('[data-slot="archived-row"] button.text-link', section).map(text),
  }));
const titles = () => all<HTMLButtonElement>('[data-slot="archived-row"] button.text-link');
const title = (name: string) => titles().find((button) => text(button) === name);
const button = (name: string, root: ParentNode = document) =>
  all<HTMLButtonElement>("button", root).find(
    (element) => text(element) === name || element.getAttribute("aria-label") === name,
  );
/** Opens a toolbar menu; the step runner then waits for it. */
const openMenu = (label: string): "menu" => {
  // From the keyboard: a closed menu that stays mounted (its exit animation never ends in
  // virtual time) would swallow a pointer press on its trigger.
  const trigger = button(label);
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  return "menu";
};
const menuItems = () => all('[role="menu"][data-state="open"] [role^="menuitem"]').map(text);
const pickMenu = (name: string) =>
  (all('[role="menu"][data-state="open"] [role^="menuitem"]').find((item) => text(item) === name) as HTMLElement | undefined)?.click();
const clickWith = (element: Element | null | undefined, init: MouseEventInit) =>
  element?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0, ...init }));
const typeSearch = (value: string) => {
  const input = q<HTMLInputElement>('input[aria-label="Search archived chats"]');
  if (!input) return;
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  setter?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};
const bar = () => text(q('[role="toolbar"][aria-label="Picked chats"]'));
const dialog = () => text(q('[role="dialog"]'));
const status = () => text(q('main [role="status"]'));

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    if (params.has("empty")) {
      void new Promise((resolve) => setTimeout(resolve, 300)).then(() => {
        seen.emptyState = status();
        setResult(JSON.stringify(seen));
      });
      return;
    }
    const steps: (() => void | "menu")[] = [
      () => {
        seen.first = groups();
        seen.triggers = [button("Filter archived chats"), button("Filter archived chats by project")].map(text);
        typeSearch("billing");
      },
      () => {
        seen.searched = groups();
        typeSearch("zzz");
      },
      () => {
        seen.noMatch = status();
        typeSearch("");
      },
      () => openMenu("Filter archived chats"),
      () => {
        seen.kindMenu = menuItems();
        pickMenu("Sessions");
      },
      () => openMenu("Filter archived chats"),
      () => pickMenu("Alphabetical"),
      () => {
        seen.sessionsByName = groups();
        return openMenu("Filter archived chats by project");
      },
      () => {
        seen.projectMenu = menuItems();
        pickMenu("storefront-web");
      },
      () => {
        seen.oneProject = groups();
        seen.trigger = text(button("Filter archived chats by project"));
        return openMenu("Filter archived chats by project");
      },
      () => pickMenu("All projects"),
      // Picking after filtering and sorting: ⌘-click two, then Shift-click a range.
      () => {
        clickWith(title("Add invoice export"), { metaKey: true });
        clickWith(title("Checkout page redesign"), { metaKey: true });
      },
      () => {
        seen.pickedTwo = bar();
        clickWith(title("Fix flaky cart tests"), { shiftKey: true });
      },
      () => {
        seen.pickedRange = bar();
        seen.selectedRows = all('[data-slot="archived-row"][data-selected="true"] button.text-link').map(text);
        button("Unarchive (2)")?.click();
      },
      () => {
        seen.afterUnarchive = { groups: groups(), bar: bar() };
        clickWith(title("Billing webhooks retry"), { metaKey: true });
      },
      () => button("Delete (1)…")?.click(),
      () => {
        seen.deleteDialog = dialog();
        button("Delete session", q('[role="dialog"]') ?? document)?.click();
      },
      () => {
        seen.afterDelete = groups();
        // The right-click menu on an unpicked row acts on that row.
        const row = q('[data-slot="archived-row"]');
        row?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 10, clientY: 10 }));
      },
      () => {
        seen.contextMenu = menuItems();
        document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
        return openMenu("Filter archived chats");
      },
      () => pickMenu("All chats"),
      () => openMenu("More"),
      () => {
        seen.moreMenu = menuItems();
        pickMenu("Delete all…");
      },
      () => {
        seen.deleteAll = dialog();
        const confirm = button("Delete 3 conversations", q('[role="dialog"]') ?? document);
        seen.deleteAtOnce = { enabled: confirm?.disabled === false, focused: document.activeElement === confirm };
        releasePreview?.();
      },
      () => {
        seen.unlanded = dialog();
        // Closing, the box keeps its text until its exit animation ends: hold that animation.
        const hold = document.createElement("style");
        hold.textContent =
          '[role="dialog"][data-state="closed"] { animation: hold 1s paused !important; } @keyframes hold { to { opacity: 0.5; } }';
        document.head.append(hold);
        button("Cancel", q('[role="dialog"]') ?? document)?.click();
      },
      () => {
        seen.closing = text(q('[role="dialog"][data-state="closed"]'));
      },
      () => {
        seen.calls = calls;
        setResult(JSON.stringify(seen));
      },
    ];
    // One step at a time, each after the page has settled; a menu step waits for its menu.
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    const until = async (done: () => boolean) => {
      for (let tries = 0; tries < 40 && !done(); tries++) await wait(50);
    };
    const menuOpen = () => q('[role="menu"][data-state="open"]') !== null;
    void (async () => {
      await wait(300);
      for (const step of steps) {
        if (stopped) return;
        const opened = step();
        await wait(100);
        if (opened === "menu") await until(menuOpen);
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
            <ArchivedPage />
          </div>
        </main>
        <DeleteDialog />
      </div>
      {result && <pre id="archived-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
