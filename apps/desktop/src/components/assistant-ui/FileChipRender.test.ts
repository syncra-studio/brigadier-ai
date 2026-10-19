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
      pictureTile: { width: number; height: number; below: boolean; label: string | null };
      otherTiles: string[];
      earlyBefore: { viewer: boolean; opened: number };
      earlyViewer: string | null;
      earlyOpened: number;
      listOrder: string[];
      listShelfLast: boolean;
      listSentence: string;
      swapped: { reads: string[]; newUrl: boolean; tileShowsIt: boolean; opened: number };
      sentence: string | null;
      viewerImage: string | null;
      imageOpenedElsewhere: boolean;
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
    // A worker's picture shows under its link as an 80px tile; the link opens the image viewer.
    assert.deepEqual(seen.pictureTile, { width: 80, height: 80, below: true, label: "Open chart.png" });
    assert.deepEqual(seen.otherTiles, []);
    // Clicked before its read ended, a picture opens in the viewer once it ends, never elsewhere.
    assert.deepEqual(seen.earlyBefore, { viewer: false, opened: 0 });
    assert.equal(seen.earlyViewer, "early.png");
    assert.equal(seen.earlyOpened, 0);
    // A list item's tiles keep their links' order, and stay after words streamed in later.
    assert.deepEqual(seen.listOrder, ["first.png", "second.png"]);
    assert.equal(seen.listShelfLast, true);
    assert.equal(seen.listSentence, "See first.png and second.png for details.");
    // An output stored again opens as its new copy, in the viewer and its tile.
    assert.deepEqual(seen.swapped, { reads: ["blob-7", "blob-8"], newUrl: true, tileShowsIt: true, opened: 0 });
    assert.equal(seen.sentence, "The list is in chart.png.", "the sentence reads on unbroken");
    assert.equal(seen.viewerImage, "chart.png");
    assert.equal(seen.imageReader, false, "a picture opens in the image viewer, not the reader");
    assert.equal(seen.imageOpenedElsewhere, false, "a picture stays in Brigadier");
    assert.equal(seen.toasts, 0);
    // In a narrow table cell the chip's icon stays on its name's line.
    assert.equal(seen.cellIconOnNameLine, true);
    const commands = seen.calls.map((call) => call.command ?? call.method);
    // The picture's link is looked up and its bytes read once, as it shows.
    assert.deepEqual(commands.slice(0, 2), ["resolveFileLink", "readAttachment"]);
    assert.equal(seen.calls[0]!.link, "outputs/task-1/chart.png");
    assert.equal(seen.calls[1]!.id, "blob-3");
    assert.deepEqual(commands.slice(2), [
      "resolveFileLink", "readArtifact", "save_artifact",
      "resolveFileLink", "readArtifact",
      "resolveFileLink", "readArtifact",
      "resolveFileLink", "readArtifact",
      "resolveFileLink",
      "resolveFileLink",
      "resolveFileLink", "open_artifact",
    ]);
    assert.deepEqual(seen.calls[2], {
      method: "resolveFileLink",
      conversationId: "chat-1",
      link: "/var/folders/xy/T/brigadier/scratch/task-1/outputs/rounding.md:3",
      path: "/var/folders/xy/T/brigadier/scratch/task-1/outputs/rounding.md",
      atMs: 1700,
    });
    // A chip outside a message passes no time.
    assert.equal(seen.calls[5]!.atMs, undefined);
    assert.equal(seen.calls[3]!.id, "blob-1");
    // The reader's Save keeps the stored copy under its own name.
    assert.equal(seen.calls[4]!.id, "blob-1");
    assert.equal(seen.calls[4]!.fileName, "rounding.md");
    assert.equal(seen.calls[5]!.link, "outputs/task-1/rounding.md");
    assert.equal(seen.calls[5]!.path, session ? "/repo/outputs/task-1/rounding.md" : "outputs/task-1/rounding.md");
    assert.equal(seen.calls[7]!.link, "./outputs/task-1/rounding.md");
    assert.equal(seen.calls[7]!.path, session ? "/repo/outputs/task-1/rounding.md" : "./outputs/task-1/rounding.md");
    assert.equal(seen.calls[10]!.id, "blob-2");
    assert.equal(seen.calls[11]!.link, "outputs/task-1/chart.png");
    assert.deepEqual(seen.defaultApp.map((call) => [call.id, call.fileName]), [["blob-1", "rounding.md"]]);
  });
}
