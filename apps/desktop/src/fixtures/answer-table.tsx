/**
 * An answer's table at a Chat's reading width, built from the answer's own Markdown parts: its
 * other columns hold an import path and a URL that can't break at a space. Writes how wide each
 * column came out, and whether the last header's text keeps clear of the hover buttons (Expand
 * table, Copy table), to #answer-table-result (AnswerTableRender.test.ts reads it). `narrow=1`
 * puts a five-column comparison in a column squeezed by a side panel, and also writes every word
 * of a header or cell that breaks across lines, and whether the table scrolls sideways.
 */
import { type ComponentProps, type FC, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { defaultComponents } from "@/components/conversation/markdown-text";
import { TooltipProvider } from "@/components/overlays";
import { cn } from "@/lib/utils";

/** One of the answer's Markdown parts, as the element it renders. */
const part = <K extends "table" | "tr" | "th" | "td" | "code" | "a">(name: K) =>
  (defaultComponents as Record<string, unknown>)[name] as FC<ComponentProps<K>>;
const [Table, Tr, Th, Td, Code, A] = [part("table"), part("tr"), part("th"), part("td"), part("code"), part("a")];

const narrow = new URLSearchParams(location.search).has("narrow");

const HEADERS = narrow ? ["Platform", "Free terms today", "Database", "Catch", "Best for"] : ["Package", "Import", "Docs", "Size", "Best for"];

const rows = narrow ? [
  ["Cloudflare Workers + D1", "Free with no end date: 100,000 requests a day", "D1 (SQLite-style database): 5 GB free", "Some setup specific to Workers", "A small app you want always on"],
  ["Render", "Free web service that sleeps after 15 minutes", "Postgres free for 30 days", "Slow wake-ups; the database expires", "Deploying Express unchanged"],
] : [
  ["Dialog", "@tauri-apps/plugin-dialog", "https://v2.tauri.app/plugin/dialog/#javascript-api-reference", "12 kB", "Native open and save dialogs from the webview"],
  ["Shell", "@tauri-apps/plugin-shell::Command.sidecar", "https://v2.tauri.app/plugin/shell/#permission-table-and-scopes", "18 kB", "Running bundled helpers and opening links in the browser"],
  ["Store", "@tauri-apps/plugin-store/LazyStore", "https://v2.tauri.app/plugin/store/#usage-with-lazystore-and-defaults", "9 kB", "Small settings that should survive restarts"],
];

/**
 * Every word of a header or cell, outside code and links, that runs over more than one line (a
 * break after a hyphen is a line break, not a broken word).
 */
function brokenWords(): string[] {
  const broken: string[] = [];
  for (const cell of document.querySelectorAll("th, td")) {
    const walker = document.createTreeWalker(cell, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.parentElement?.closest("code, a")) continue;
      const value = node.textContent ?? "";
      for (const match of value.matchAll(/[^\s-]+-?/g)) {
        const range = document.createRange();
        range.setStart(node, match.index);
        range.setEnd(node, match.index + match[0].length);
        const lines = new Set([...range.getClientRects()].map((rect) => Math.round(rect.top)));
        if (lines.size > 1) broken.push(match[0]);
      }
    }
  }
  return broken;
}

function Fixture() {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    const timer = setTimeout(() => {
      const headers = [...document.querySelectorAll("th")];
      const last = headers.at(-1)!;
      const text = document.createRange();
      text.selectNodeContents(last);
      const buttons = document.querySelector(".aui-md-table-wrapper > div.absolute")!.getBoundingClientRect();
      const scroller = document.querySelector(".aui-md-table-wrapper > div.overflow-x-auto")!;
      setResult(JSON.stringify({
        widths: headers.map((th) => Math.round(th.getBoundingClientRect().width)),
        tableFits: document.querySelector("table")!.getBoundingClientRect().width <= document.querySelector("main")!.getBoundingClientRect().width,
        width: Math.round(document.querySelector("main")!.getBoundingClientRect().width),
        lastHeaderClear: text.getBoundingClientRect().right <= buttons.left,
        broken: brokenWords(),
        scrolls: scroller.scrollWidth > scroller.clientWidth,
      }));
    }, 300);
    return () => clearTimeout(timer);
  }, []);
  return (
    <TooltipProvider>
      <main className={cn("bg-background text-foreground w-full text-sm", narrow ? "max-w-md" : "max-w-thread")}>
        <Table>
          <thead>
            <Tr>{HEADERS.map((name) => <Th key={name}>{name}</Th>)}</Tr>
          </thead>
          <tbody>
            {rows.map(([name, path, url, size, best]) => (
              <Tr key={name}>
                <Td>{name}</Td>
                <Td>{narrow ? path : <Code>{path}</Code>}</Td>
                <Td>{narrow ? url : <A href={url}>{url}</A>}</Td>
                <Td>{size}</Td>
                <Td>{best}</Td>
              </Tr>
            ))}
          </tbody>
        </Table>
      </main>
      {result && <pre id="answer-table-result">{result}</pre>}
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
