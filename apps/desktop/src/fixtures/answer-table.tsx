/**
 * An answer's table at a Chat's reading width, built from the answer's own Markdown parts: its
 * other columns hold an import path and a URL that can't break at a space. Writes how wide each
 * column came out, and whether the last header's text keeps clear of the hover buttons (Expand
 * table, Copy table), to #answer-table-result (AnswerTableRender.test.ts reads it).
 */
import { type ComponentProps, type FC, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { defaultComponents } from "@/components/assistant-ui/markdown-text";
import { TooltipProvider } from "@/components/ui/tooltip";

/** One of the answer's Markdown parts, as the element it renders. */
const part = <K extends "table" | "tr" | "th" | "td" | "code" | "a">(name: K) =>
  defaultComponents[name] as unknown as FC<ComponentProps<K>>;
const [Table, Tr, Th, Td, Code, A] = [part("table"), part("tr"), part("th"), part("td"), part("code"), part("a")];

const rows = [
  ["Dialog", "@tauri-apps/plugin-dialog", "https://v2.tauri.app/plugin/dialog/#javascript-api-reference", "12 kB", "Native open and save dialogs from the webview"],
  ["Shell", "@tauri-apps/plugin-shell::Command.sidecar", "https://v2.tauri.app/plugin/shell/#permission-table-and-scopes", "18 kB", "Running bundled helpers and opening links in the browser"],
  ["Store", "@tauri-apps/plugin-store/LazyStore", "https://v2.tauri.app/plugin/store/#usage-with-lazystore-and-defaults", "9 kB", "Small settings that should survive restarts"],
];

function Fixture() {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    const timer = setTimeout(() => {
      const headers = [...document.querySelectorAll("th")];
      const last = headers.at(-1)!;
      const text = document.createRange();
      text.selectNodeContents(last);
      const buttons = document.querySelector(".aui-md-table-wrapper > div.absolute")!.getBoundingClientRect();
      setResult(JSON.stringify({
        widths: headers.map((th) => Math.round(th.getBoundingClientRect().width)),
        tableFits: document.querySelector("table")!.getBoundingClientRect().width <= document.querySelector("main")!.getBoundingClientRect().width,
        width: Math.round(document.querySelector("main")!.getBoundingClientRect().width),
        lastHeaderClear: text.getBoundingClientRect().right <= buttons.left,
      }));
    }, 300);
    return () => clearTimeout(timer);
  }, []);
  return (
    <TooltipProvider>
      <main className="bg-background text-foreground max-w-thread w-full text-sm">
        <Table>
          <thead>
            <Tr>{["Package", "Import", "Docs", "Size", "Best for"].map((name) => <Th key={name}>{name}</Th>)}</Tr>
          </thead>
          <tbody>
            {rows.map(([name, path, url, size, best]) => (
              <Tr key={name}>
                <Td>{name}</Td>
                <Td><Code>{path}</Code></Td>
                <Td><A href={url}>{url}</A></Td>
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
