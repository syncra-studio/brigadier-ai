import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Shown = { title: string | null; heading: string | null; bold: string | null; plain: string | null; save: string | null; tooltip: boolean };

for (const session of [false, true]) {
  test(`in a ${session ? "session" : "Chat"}, a worker's file opens in the reader with its Save, no Save sits next to a link, and dead links say so`, { timeout: 90000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, `file-chip.html${session ? "?session" : ""}`, "file-chip-result", 60000)) as {
      error?: string;
      before: Record<string, string | null>;
      after: Record<string, string | null>;
      reader: Record<string, Shown>;
      presses: number[];
      imageReader: boolean;
      inlineSave: string[];
      defaultApp: Record<string, unknown>[];
      toasts: number;
      cellIconOnNameLine: boolean;
      calls: Record<string, unknown>[];
    };
    assert.equal(seen.error, undefined, `${seen.error} ${JSON.stringify(seen.calls)}`);
    const none = { stored: null, lost: null, outputs: null, dotOutputs: null, csv: null, image: null };
    assert.deepEqual(seen.before, none);
    // Only a dead link with nothing stored says so; a stored copy opens without a notice.
    assert.deepEqual(seen.after, { ...none, lost: " (This file is no longer here)" });
    assert.deepEqual(seen.inlineSave, []);
    // Markdown shows as the answer does; other text as written; each with its Save.
    const markdown = { title: "rounding.md", heading: "Rounding", bold: "half-even", plain: null, save: "Save…", tooltip: false };
    assert.deepEqual(seen.reader, {
      stored: markdown,
      outputs: markdown,
      dotOutputs: markdown,
      csv: { title: "rates.csv", heading: null, bold: null, plain: "currency,rate\nEUR,0.92\nMDL,17.8", save: "Save…", tooltip: false },
    });
    // The chip's path never stays over the reader: one Escape closes it.
    assert.deepEqual(seen.presses, [1, 1, 1, 1]);
    assert.equal(seen.imageReader, false, "an image opens in its own app, not the reader");
    assert.equal(seen.toasts, 0);
    // In a narrow table cell the chip's icon stays on its name's line.
    assert.equal(seen.cellIconOnNameLine, true);
    const commands = seen.calls.map((call) => call.command ?? call.method);
    assert.deepEqual(commands, [
      "resolveFileLink", "readArtifact", "save_artifact",
      "resolveFileLink", "readArtifact",
      "resolveFileLink", "readArtifact",
      "resolveFileLink", "readArtifact",
      "resolveFileLink", "open_artifact",
      "resolveFileLink",
      "resolveFileLink", "open_artifact",
    ]);
    assert.deepEqual(seen.calls[0], {
      method: "resolveFileLink",
      conversationId: "chat-1",
      link: "/var/folders/xy/T/brigadier/scratch/task-1/outputs/rounding.md:3",
      path: "/var/folders/xy/T/brigadier/scratch/task-1/outputs/rounding.md",
      atMs: 1700,
    });
    // A chip outside a message passes no time.
    assert.equal(seen.calls[3]!.atMs, undefined);
    assert.equal(seen.calls[1]!.id, "blob-1");
    // The reader's Save keeps the stored copy under its own name.
    assert.equal(seen.calls[2]!.id, "blob-1");
    assert.equal(seen.calls[2]!.fileName, "rounding.md");
    assert.equal(seen.calls[3]!.link, "outputs/task-1/rounding.md");
    assert.equal(seen.calls[3]!.path, session ? "/repo/outputs/task-1/rounding.md" : "outputs/task-1/rounding.md");
    assert.equal(seen.calls[5]!.link, "./outputs/task-1/rounding.md");
    assert.equal(seen.calls[5]!.path, session ? "/repo/outputs/task-1/rounding.md" : "./outputs/task-1/rounding.md");
    assert.equal(seen.calls[8]!.id, "blob-2");
    assert.equal(seen.calls[10]!.id, "blob-3");
    assert.equal(seen.calls[10]!.fileName, "chart.png");
    assert.deepEqual(seen.defaultApp.map((call) => [call.id, call.fileName]), [["blob-1", "rounding.md"]]);
  });
}
