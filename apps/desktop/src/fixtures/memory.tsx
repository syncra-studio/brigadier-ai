/**
 * Settings → Memory over a faked daemon that keeps the memories: the list, an edit saved, an
 * edit the daemon refuses, a delete, and the page after it.
 *
 * ?drive=1 does each in turn and writes what it saw to #memory-result
 * (MemoryPageRender.test.ts reads it). Without it the page stays as it opens, for screenshots;
 * ?empty=1 opens it with nothing kept.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Node, Request } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent =
    "*, *::before, *::after { animation: none !important; transition: none !important; backdrop-filter: none !important; }";
  document.head.append(still);
}

const HOUR = 3_600_000;
const memory = (id: string, title: string, ago: number, sessionId: string | null) =>
  ({
    id,
    kind: "preference",
    title,
    body: "",
    updatedAtMs: Date.now() - ago,
    provenance: { origin: "user", sessionId },
  }) as unknown as Node;

let memories: Node[] = params.has("empty")
  ? []
  : [
      memory("m1", "Lives in Chisinau, Moldova", 2 * HOUR, "chat-1"),
      memory("m2", "Works as an electrician", 26 * HOUR, "chat-2"),
      memory("m3", "Prints on a Bambu Lab P1S with a hardened steel nozzle", 72 * HOUR, null),
    ];
const calls: string[] = [];

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  conversations: {
    ...state.conversations,
    "chat-1": { id: "chat-1", kind: "chat", title: "Filament shops" },
    "chat-2": { id: "chat-2", kind: "chat", title: "Wiring a garage" },
  } as unknown as typeof state.conversations,
}));

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "listMemories":
      return { method: req.method, memories };
    case "editMemory": {
      calls.push(`edit ${req.nodeId} ${req.text}`);
      // The daemon's refusal of a card number, as it words it.
      if (/\d{4} \d{4} \d{4} \d{4}/.test(req.text)) {
        throw new Error("Brigadier doesn't keep that: it holds a card number");
      }
      memories = memories.map((entry) =>
        entry.id === req.nodeId ? { ...entry, title: req.text.trim(), updatedAtMs: Date.now() } : entry,
      );
      return { method: req.method, memory: memories.find((entry) => entry.id === req.nodeId) };
    }
    case "forgetMemory":
      calls.push(`forget ${req.nodeId}`);
      memories = memories.filter((entry) => entry.id !== req.nodeId);
      return { method: req.method };
    default:
      return { method: req.method };
  }
});

const all = <T extends Element = HTMLElement>(selector: string, root: ParentNode = document) => [
  ...root.querySelectorAll<T>(selector),
];
const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
const buttonNamed = (name: string) => all<HTMLButtonElement>("main button").find((item) => text(item) === name);
const rows = () =>
  all('main [data-slot="settings-row"]').map((row) => ({
    text: text(row.querySelector(".font-medium")),
    line: text(row.querySelector(".text-xs")),
  }));
const type = (value: string) => {
  const input = document.querySelector<HTMLInputElement>('main input[aria-label="Memory"]');
  if (!input) return;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
};

function Fixture() {
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState("");
  useEffect(() => {
    openSettings("memory");
    if (!params.has("drive")) return;
    const seen: Record<string, unknown> = {};
    const steps: (() => void)[] = [
      () => {
        seen.title = text(document.querySelector("main h1"));
        seen.description = text(document.querySelector("main header p"));
        seen.first = rows();
        button("Edit “Lives in Chisinau, Moldova”")?.click();
      },
      () => {
        seen.editing = {
          value: document.querySelector<HTMLInputElement>('main input[aria-label="Memory"]')?.value,
          buttons: all("main [data-slot='settings-row'] button").map(text),
        };
        type("Lives in Iasi, Romania");
      },
      () => buttonNamed("Save")?.click(),
      () => {
        seen.edited = rows();
        button("Edit “Works as an electrician”")?.click();
      },
      () => type("Card 4111 1111 1111 1111"),
      () => buttonNamed("Save")?.click(),
      () => {
        seen.refused = {
          error: text(document.querySelector("main [data-slot='settings-row'] [role='alert']")),
          stillEditing: document.querySelector('main input[aria-label="Memory"]') !== null,
        };
        buttonNamed("Cancel")?.click();
      },
      () => {
        seen.cancelled = rows();
        button("Delete “Works as an electrician”")?.click();
      },
      () => {
        seen.deleted = rows();
        seen.calls = calls;
        setResult(JSON.stringify(seen));
      },
    ];
    let stopped = false;
    const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
    void (async () => {
      await wait(600);
      for (const step of steps) {
        if (stopped) return;
        step();
        await wait(300);
      }
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <aside className="w-64 shrink-0">
          <SettingsNav />
        </aside>
        <main className="flex h-full min-w-0 flex-1">
          <div className="min-w-0 flex-1">{page && <SettingsView page={page} />}</div>
        </main>
      </div>
      {result && <pre id="memory-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
