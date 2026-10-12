/**
 * Search with as much as the daemon sends at most: 2,000 files and 20,000 matching lines,
 * streamed in its batches (64 files every 50 ms). `scripts/measure-search-scale.mjs` times the
 * results showing, the view opening again on them, and scrolling through them.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SearchPanel } from "@/app/conversation/SearchPanel";
import type { Request, SearchFile } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { emitSearchUpdate } from "@/state/workspaceSearch";

declare global {
  interface Window {
    /** Shows the Search view, or takes it away. */
    showSearch: (shown: boolean) => void;
  }
}

const FILES = 2000;
const LINES = 10;
const files: SearchFile[] = Array.from({ length: FILES }, (_, file) => ({
  path: `${["apps", "crates", "docs", "scripts"][file % 4]}/part-${file % 37}/file-${file}.ts`,
  hash: `h${file}`,
  matches: LINES,
  lines: Array.from({ length: LINES }, (_line, index) => ({
    line: index * 7 + 1,
    preview: `  const needle${index} = find(needle, haystack[${index}]); // needle in file ${file}`,
    ranges: [{ start: 8, end: 14 }, { start: 30, end: 36 }],
  })),
}));

useApp.setState({ connection: { status: "connected", daemon: null, reason: null } });
let ids = 0;
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  if (req.method === "startSearch") {
    const id = ++ids;
    void (async () => {
      for (let at = 0; at < files.length; at += 64) {
        await new Promise((resolve) => setTimeout(resolve, 50));
        emitSearchUpdate(id, { type: "files", files: files.slice(at, at + 64) });
      }
      emitSearchUpdate(id, { type: "done", files: FILES, matches: FILES * LINES, capped: true });
    })();
    return { method: req.method, id };
  }
  return { method: req.method };
});

const target = { type: "conversation", conversationId: "search-scale" } as const;

function Fixture() {
  const [shown, setShown] = useState(true);
  useEffect(() => {
    window.showSearch = setShown;
  }, []);
  return <div className="bg-background text-foreground flex h-screen">
    <div data-panel="search" className="flex w-80 flex-col">
      {shown && <SearchPanel target={target} readOnly={false} onOpen={() => undefined} />}
    </div>
  </div>;
}

createRoot(document.getElementById("root")!).render(<Fixture />);
