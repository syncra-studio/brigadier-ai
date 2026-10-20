import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";
import { promisify } from "node:util";

const run = promisify(execFile);

/** The lines `brigadier/thread-scroll-helper` reports in each file, by its name. */
async function reported(files: Record<string, string>): Promise<Record<string, number[]>> {
  const dir = mkdtempSync(join(tmpdir(), "brigadier-thread-scroll-lint-"));
  try {
    writeFileSync(join(dir, ".oxlintrc.json"), JSON.stringify({
      jsPlugins: [resolve("lint/brigadier-plugin.js")],
      categories: { correctness: "off", suspicious: "off" },
      rules: { "brigadier/thread-scroll-helper": "error" },
    }));
    for (const [name, text] of Object.entries(files)) writeFileSync(join(dir, name), text);
    const { stdout } = await run(process.execPath, [resolve("node_modules/oxlint/bin/oxlint"), "-c", join(dir, ".oxlintrc.json"), "-f", "json", dir], { windowsHide: true }).catch(
      (failed: { stdout: string }) => failed,
    );
    const seen: Record<string, number[]> = Object.fromEntries(Object.keys(files).map((name) => [name, []]));
    for (const each of (JSON.parse(stdout) as { diagnostics: { code: string; filename: string; labels: { span: { line: number } }[] }[] }).diagnostics) {
      if (each.code !== "brigadier(thread-scroll-helper)") continue;
      seen[each.filename.split(/[\\/]/).pop()!]!.push(each.labels[0]!.span.line);
    }
    return seen;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("code that scrolls the thread's view must say it jumps first, as the scroller needs to keep auto-follow right", async () => {
  const seen = await reported({
    // A file that works with the thread's view: every scroll in it waits for a jump said first.
    "thread.ts": [
      'import { jumpInThread } from "@/components/conversation/thread-scroll";',
      "export function bad(row: HTMLElement) {",
      '  row.scrollIntoView({ block: "center" });',
      "}",
      "export function good(row: HTMLElement) {",
      "  jumpInThread(row);",
      '  row.scrollIntoView({ block: "center" });',
      "}",
      "export function late(box: HTMLElement, row: HTMLElement) {",
      "  box.scrollTop = 4;",
      "  jumpInThread(row);",
      "}",
      "export function nested(row: HTMLElement) {",
      "  jumpInThread(row);",
      '  requestAnimationFrame(() => row.scrollIntoView({ block: "nearest" }));',
      "}",
      "export function own(list: HTMLElement) {",
      "  // oxlint-disable-next-line brigadier/thread-scroll-helper -- a list of its own.",
      "  list.scrollTop = 0;",
      "}",
    ].join("\n"),
    // Anywhere, a scroll of a `viewport` is the thread's; the scroller's release() says it jumps.
    "find.ts": [
      "export function bad(viewport: HTMLElement) {",
      "  viewport.scrollTo({ top: 0 });",
      "}",
      "export function good(viewport: HTMLElement, scroller: { release(): void }) {",
      "  scroller.release();",
      "  viewport.scrollBy({ top: 10 });",
      "}",
    ].join("\n"),
    // Elsewhere a list's own scroll is no concern of the thread's.
    "picker.ts": [
      "export function show(list: HTMLElement) {",
      '  list.querySelector("[data-highlighted]")?.scrollIntoView({ block: "nearest" });',
      "  list.scrollTop = 0;",
      "}",
    ].join("\n"),
  });
  assert.deepEqual(seen, { "thread.ts": [3, 10], "find.ts": [2], "picker.ts": [] });
});
