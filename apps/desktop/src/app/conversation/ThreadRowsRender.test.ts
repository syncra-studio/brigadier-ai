import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Shell = {
  command: string | null; clamped: boolean; outputLines: number; output: string | null; colored: [string, string][];
  ending: string | null; clipped: string | null; folded: boolean;
};

test("the thread's rows read like the reference thread: Shell boxes, stopped steps, a live group's children, web searches", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-rows.html", "thread-rows-result", 90000)) as Record<
    "shell-long" | "shell-failed" | "shell-running" | "shell-clipped" | "shell-command",
    Shell
  > & {
    error?: string;
    commandClicked: boolean;
    stopped: { row: string | null; state: string | null; shimmer: boolean };
    stoppedShell: string | null;
    groupRow: string | null;
    children: (string | null)[];
    search: { row: string | null; opens: boolean };
    chips: [string, string][];
    code: string[];
    asked: string[];
    viewer: { first: { open: boolean; count: string | null; alt: string | null }; second: { count: string | null; alt: string | null }; closed: boolean };
    zoomKeys: (string | null)[];
    boundary: { fallback: boolean; neighbour: string | null };
    mended: string | null;
    links: { before: string[]; after: string[]; asked: string[] };
    linkExtras: { address: string | null; joined: boolean; dashed: boolean; menu: string[]; mailMenu: string[]; chipMenu: string[] };
    doneGroup: { row: string | null; icon: string | null };
    doneChildren: (string | null)[];
    loneRan: string | null;
    kindIcons: Record<string, string | null>;
    diffDialog: { fits: boolean; files: number; scrolls: boolean; whole: boolean };
    editRow: { closed: string | null; open: string | null };
    turnDiff: Record<"many" | "one", { title: string | null; counts: string | null; buttons: (string | null)[]; files: (string | null)[][]; more: string | null }> & {
      all: { files: number; more: string | null }; opened: { name: string | null; counts: string | null }; asks: string[];
    };
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
  // Running: nothing printed yet and no ending.
  assert.deepEqual([seen["shell-running"].output, seen["shell-running"].ending], [null, null]);
  // Cut short by the daemon: says so, and the marker isn't shown as output.
  assert.equal(seen["shell-clipped"].output, "head of the log");
  assert.equal(seen["shell-clipped"].clipped, "Only the start was kept: 12 KB more not shown");
  // A long command: two lines until clicked.
  assert.deepEqual([seen["shell-command"].clamped, seen.commandClicked], [true, false]);
  // A command the user's Stop cut: "Stopped … after 4s", no shimmer; its box ends "Stopped".
  assert.deepEqual(seen.stopped, { row: "Stopped sleep 30 after 4s", state: "stopped", shimmer: false });
  assert.equal(seen.stoppedShell, "Stopped");
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
  // An edit row opened to its diff reads "Edited file".
  assert.deepEqual(seen.editRow, { closed: "Edited math.js", open: "Edited file" });
  // The turn's diff card: "Edited 5 files +5 −5", Undo and View changes; the first three files, a
  // folder dim before each name; then all five, and a row opens to its own diff.
  const { turnDiff } = seen;
  assert.deepEqual([turnDiff.many.title, turnDiff.many.counts, turnDiff.many.buttons], ["Edited 5 files", "+5 −5", ["View changed files", "Undo", "View changes"]]);
  assert.deepEqual(turnDiff.many.files, [["", "README.md"], ["src/", "math.js"], ["src/", "strings.js"]]);
  assert.equal(turnDiff.many.more, "Show 2 more files");
  assert.deepEqual(turnDiff.all, { files: 5, more: "Collapse files" });
  assert.deepEqual(turnDiff.opened, { name: "README.md", counts: "+1 −1" });
  assert.deepEqual(turnDiff.asks, [JSON.stringify({ type: "lastTurn", requestId: "many" })]);
  // One file: its name is the title, and there is no list.
  assert.deepEqual([turnDiff.one.title, turnDiff.one.files, turnDiff.one.more], ["Edited run.mjs", [], null]);
  assert.equal(seen["diff-missing"].empty, "Diff too large to preview");
  // Inline code naming a checkout file is a chip: from the list, or asked by its exact path
  // because the list was cut short; other code stays code, and a missing file is no chip.
  assert.deepEqual(seen.chips, [["src/cli.ts (line 64)", "/repo/src/cli.ts:64"], ["src/made-later.ts", "/repo/src/made-later.ts"]]);
  assert.deepEqual(seen.code, ["Math.round", "src/gone.ts"]);
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
  // Its address on hover, its icon kept with its first word, a dashed underline on hover; a
  // right-click opens it outside or copies it, and a file chip's opens it, shows it, copies its
  // path or saves a copy.
  assert.deepEqual(seen.linkExtras, {
    address: "https://nodejs.org/en/about",
    joined: true,
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
