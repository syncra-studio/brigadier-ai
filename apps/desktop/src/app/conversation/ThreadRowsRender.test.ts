import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Shell = {
  command: string | null; clamped: boolean; shrinks: string; outputLines: number; output: string | null; colored: [string, string][];
  ending: string | null; clipped: string | null; notes: (string | null)[]; folded: boolean;
};

test("the thread's rows read like the reference thread: Shell boxes, stopped steps, a live group's children, web searches", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-rows.html", "thread-rows-result", 90000)) as Record<
    "shell-long" | "shell-failed" | "shell-cli-failed" | "shell-running" | "shell-clipped" | "shell-command" | "shell-note",
    Shell
  > & {
    error?: string;
    commandClicked: boolean;
    stopped: { row: string | null; state: string | null; shimmer: boolean };
    stoppedShell: string | null;
    stoppedFailed: { row: string | null; state: string | null; icon: string | null };
    stoppedFailedShell: string | null;
    groupRow: string | null;
    children: (string | null)[];
    search: { row: string | null; opens: boolean };
    chips: [string, string][];
    code: string[];
    longCode: Record<"atRest" | "atEnd" | "between", { more: string | null; mask: boolean; scrolls?: boolean }>;
    asked: string[];
    viewer: { first: { open: boolean; count: string | null; alt: string | null }; second: { count: string | null; alt: string | null }; closed: boolean };
    zoomKeys: (string | null)[];
    boundary: { fallback: boolean; neighbour: string | null };
    mended: string | null;
    links: { before: string[]; after: string[]; asked: string[] };
    linkExtras: { address: string | null; joined: boolean; dashed: boolean; menu: string[]; mailMenu: string[]; chipMenu: string[] };
    doneGroup: { row: string | null; icon: string | null };
    doneChildren: (string | null)[];
    groupShell: { card: boolean; whole: boolean; commandClear: boolean; scrolls: boolean };
    loneRan: string | null;
    kindIcons: Record<string, string | null>;
    diffDialog: { fits: boolean; files: number; scrolls: boolean; whole: boolean };
    editRow: { closed: string | null; open: string | null };
    renameRows: (string | null)[];
    tools: Record<string, { json: string | null; text: string | null; empty: string | null; error: boolean; argumentsShown: boolean; raw: string | null }>;
    toolRaw: string | null;
    compaction: (string | null)[];
    sentImages: [string | null, string | null, boolean][];
    user: { strong: string[]; em: string[]; breaks: number; paragraphs: number; code: string[]; text: string; image: string[] };
  } & Record<"diff-patch" | "diff-edit" | "diff-empty" | "diff-missing", {
    name: string | null; counts: string | null; numbers: string[]; kinds: string[]; gaps: string[]; marks: string[]; empty: string | null; coloured: boolean;
  }>;
  assert.equal(seen.error, undefined);
  // The whole output, newest line in view, no head-and-tail fold; its colours kept; success said.
  const long = seen["shell-long"];
  assert.deepEqual([long.outputLines, long.folded, long.ending, long.clipped], [60, false, "Success", null]);
  assert.deepEqual(long.colored, [["✓ 60 tests passed", "var(--ansi-green)"]]);
  assert.ok(!(long.output ?? "").includes(String.fromCharCode(27)), "no escape left in the output");
  assert.equal(seen["shell-failed"].ending, "Exit code 1");
  // The CLI's "Exit code 1" first line is the box's ending, not its output.
  assert.deepEqual([seen["shell-cli-failed"].output, seen["shell-cli-failed"].ending], [".gitignore\nREADME.md", "Exit code 1"]);
  // Running: nothing printed yet and no ending.
  assert.deepEqual([seen["shell-running"].output, seen["shell-running"].ending], [null, null]);
  // Cut short by the daemon: says so, and the marker isn't shown as output.
  assert.equal(seen["shell-clipped"].output, "head of the log");
  assert.equal(seen["shell-clipped"].clipped, "Only the start was kept: 12 KB more not shown");
  // What run's commit made reads under the output, in plain words, and the box still ends "Success".
  assert.deepEqual([seen["shell-note"].output, seen["shell-note"].notes, seen["shell-note"].ending], [
    "ok",
    ['Committed 1a2b3c4 "Fix rounding": 1 file changed, 2 insertions(+) (src/round.js). Nothing is left uncommitted.'],
    "Success",
  ]);
  assert.deepEqual(seen["shell-long"].notes, []);
  // A long command: two lines until clicked.
  assert.deepEqual([seen["shell-command"].clamped, seen.commandClicked], [true, false]);
  assert.equal(seen["shell-command"].shrinks, "0");
  // A command the user's Stop cut: "Stopped … after 4s", no shimmer; its box ends "Stopped".
  assert.deepEqual(seen.stopped, { row: "Stopped sleep 30 after 4s", state: "stopped", shimmer: false });
  assert.equal(seen.stoppedShell, "Stopped");
  // A call that failed as the Stop killed it reads stopped, not failed.
  assert.deepEqual(seen.stoppedFailed, { row: "Stopped npm test after 4s", state: "stopped", icon: "stop" });
  assert.equal(seen.stoppedFailedShell, "Stopped");
  // Opened while live: the running command ticks, the read still running is left out.
  assert.equal(seen.groupRow, "Reading round.js");
  assert.equal(seen.children.length, 2);
  assert.equal(seen.children[0], "Read math.js");
  assert.match(seen.children[1] ?? "", /^Running command for [3-9]s$/);
  // A finished group sums up in the fixed order, with the icon of its first segment's first step,
  // not of its first step; opened, the command says how long it took; a row on its own doesn't.
  assert.deepEqual(seen.doneGroup, { row: "Read files, ran a command, checked project memory", icon: "read" });
  // (The time is its own span after the words.)
  assert.deepEqual(seen.doneChildren, ["Checked project memory for “rounding”", "Ran npm testin 41s", "Read math.js"]);
  // A Shell box opened inside a group is never squeezed by the group's 224px: the list scrolls.
  assert.deepEqual(seen.groupShell, { card: true, whole: true, commandClear: true, scrolls: true });
  assert.equal(seen.loneRan, "Ran npm test");
  // Each kind's icon: a terminal, a file, a magnifier, a folder, a pencil, the globe for a public page's GET.
  assert.deepEqual(seen.kindIcons, { run: "run", read: "read", search: "code", list: "list", edit: "edit", web: "web" });
  // A web search: its site after a bar, nothing to open.
  assert.deepEqual(seen.search, { row: "Searched the web for node lts release | nodejs.org", opens: false });
  // A real patch: numbered from its hunks, the stretch between them counted, changed words marked, coloured.
  const patch = seen["diff-patch"];
  assert.deepEqual([patch.name, patch.counts], ["math.js", "+2 −1"]);
  assert.deepEqual(patch.numbers, ["1", "2", "2", "3", "4", "20", "21", "22"]);
  assert.deepEqual(patch.kinds, ["context", "change-deletion", "change-addition", "context", "context", "context", "change-addition", "context"]);
  assert.deepEqual(patch.gaps, ["15 unmodified lines"]);
  assert.deepEqual(patch.marks, ["(", "+ Number.EPSILON) "]);
  assert.equal(patch.coloured, true);
  // An edit rebuilt from its call: no numbers, its changed word marked on both sides.
  assert.deepEqual([seen["diff-edit"].numbers, seen["diff-edit"].marks, seen["diff-edit"].counts], [[], ["1", "2"], "+1 −1"]);
  assert.equal(seen["diff-empty"].empty, "No changes");
  // A sent image read shows; one that can't be read says "Failed"; one on its way pulses.
  assert.deepEqual(seen.sentImages, [[null, null, true], ["failed", "Failed", false], ["loading", null, false]]);
  // A compaction says it compacts; after 10 s, that it can take a few minutes.
  assert.deepEqual(seen.compaction, ["Compacting context", "Compacting context · This can take a few minutes"]);
  // A tool's result: structured data as a JSON code block, text as it is, nothing said so, a
  // failure in the error callout; its arguments only in the raw view.
  const { tools } = seen;
  assert.match(tools["tool-json"]?.json ?? "", /"ok": true/);
  assert.equal(tools["tool-text"]?.text, "Found it.");
  assert.equal(tools["tool-empty"]?.empty, "Tool returned no content");
  assert.equal(tools["tool-failed"]?.error, true);
  for (const each of Object.values(tools)) assert.deepEqual([each.argumentsShown, each.raw], [false, "Show raw tool call output"]);
  assert.match(seen.toolRaw ?? "", /lookup.*Input.*"id":7.*Output/);
  // An edit row opened to its diff still names its file (#175).
  assert.deepEqual(seen.editRow, { closed: "Edited math.js", open: "Edited math.js" });
  // A Codex rename is a rename row, not "Edited new.rs +0 −0".
  assert.deepEqual(seen.renameRows, ["Renamed old.rs to new.rs +0 −0", "Moved a.rs +0 −0"]);
  assert.equal(seen["diff-missing"].empty, "Diff too large to preview");
  // Inline code naming a checkout file is a chip: from the list, or asked by its exact path
  // because the list was cut short; other code stays code, and a missing file is no chip.
  assert.deepEqual(seen.chips, [["src/cli.ts (line 64)", "/repo/src/cli.ts:64"], ["src/made-later.ts", "/repo/src/made-later.ts"]]);
  assert.deepEqual(seen.code, ["Math.round", "src/gone.ts"]);
  // A code line longer than the card scrolls sideways, and the side with more fades out.
  assert.deepEqual(seen.longCode, {
    atRest: { more: "end", mask: true, scrolls: true },
    atEnd: { more: "start", mask: true },
    between: { more: "both", mask: true },
  });
  assert.deepEqual(seen.asked, ["/repo/src/gone.ts", "/repo/src/made-later.ts"]);
  // The image viewer steps through its set and closes from the backdrop.
  assert.deepEqual(seen.viewer, {
    first: { open: true, count: "1 of 3", alt: "image-1.png" },
    second: { count: "2 of 3", alt: "image-2.png" },
    closed: true,
  });
  // + zooms and 0 fits again, pressed with focus on Close (outside the image's own frame).
  assert.deepEqual(seen.zoomKeys, ["100%", "110%", "100%"]);
  // Web links: a globe until the site's icon comes, kept when the site has none, none for a
  // plain-HTTP link; a site is asked once however many links it has.
  assert.deepEqual(seen.links, {
    before: ["globe", "globe", "globe", "none", "mail"],
    after: ["icon", "icon", "globe", "none", "mail"],
    asked: ["https://nodejs.org", "https://none.example"],
  });
  // Its address on hover, no word joiner inserted before its text, a dashed underline on hover; a
  // right-click opens it outside or copies it, and a file chip's opens it, shows it, copies its
  // path or saves a copy.
  assert.deepEqual(seen.linkExtras, {
    address: "nodejs.orghttps://nodejs.org/en/about",
    joined: false,
    dashed: true,
    menu: ["Open in external browser", "Copy link"],
    mailMenu: ["Copy email address"],
    // (The fixture names no platform: "Finder" on a Mac.)
    chipMenu: ["Open in", "Show in file manager", "Copy path", "Download a copy"],
  });
  // A worker's diff opens as its files' diffs in a dialog within the window: the list scrolls,
  // each file's diff whole.
  assert.deepEqual(seen.diffDialog, { fits: true, files: 6, scrolls: true, whole: true });
  // A turn that fails to render says so in its place, the rest stays; Try again renders it afresh.
  assert.deepEqual(seen.boundary, { fallback: true, neighbour: "The next turn" });
  assert.equal(seen.mended, "Rendered again");
  // The user's own Markdown renders as one document, its line breaks kept; a mention of no known
  // worker stays as written, and code keeps a mention's words as typed; a pasted image is a chip
  // inside its sentence.
  assert.deepEqual(seen.user, {
    strong: ["1. CSV"], em: ["line"], breaks: 1, paragraphs: 3,
    code: ["round", "@task-9", "ping @task-9"],
    text: "1. CSV — first line second line Ask @task-9 about round and @task-9",
    image: ["Before Image #1 after."],
  });
});
