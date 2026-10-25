import assert from "node:assert/strict";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { turnActivity } from "@/app/conversation/activity/group";
import { stepLabel, summarize, toolStepWords, type ToolStep } from "@/app/conversation/activity/words";
import { answerIndex, answeringIndex, type Block, type BoardDigest, blockFinished, blockSequence, foldsWork, isLive } from "@/app/conversation/blocks";
import { cliBlocks } from "@/app/conversation/cliBlocks";
import { turnParts } from "@/app/conversation/workParts";
import type { BackgroundTask, FileEntry, ProviderEvent, ThreadView } from "@/ipc/generated";
import { mergeEntries } from "@/state/cliThread";

// D44: the turn's work as the thread draws it (groups, their summaries, each row's words, the
// fold), from the recorded CLI versions (D29) live, reloaded and replayed event by event. A row
// never runs in a turn that ended, unless its CLI still runs it in the background.
const root = new URL("../../../../../crates/providers/fixtures/versions/", import.meta.url);
const folders = readdirSync(root, { withFileTypes: true }).filter((entry) => entry.isDirectory());

type Fixture = { events: ProviderEvent[]; live: ThreadView; persisted: ThreadView };

function load(folder: string, scenario: string): Fixture {
  const base = new URL(`${folder}/`, root);
  const index = JSON.parse(readFileSync(new URL("index.json", base), "utf8")) as { scenarios: Record<string, { view: string }> };
  const view = index.scenarios[scenario]?.view;
  assert.ok(view, `${folder}/${scenario}`);
  return JSON.parse(readFileSync(new URL(view, base), "utf8")) as Fixture;
}

const toolSteps = (block: Block): ToolStep[] => block.orchestratorSteps.flatMap((step) => (step.kind.type === "tool" ? [step.kind] : []));

/** A row's words: a stopped call says so (its row reads "Stopped sleep 30 after 4s"). */
function rowWords(kind: ToolStep): string {
  const words = toolStepWords(kind);
  return kind.stopped ? `stopped: ${words.done}` : stepLabel(words, kind.status, kind.exit ?? null);
}

/** Each block's work as the thread folds it: its groups (key, summary, rows) and what stands between. */
function work(view: ThreadView) {
  return cliBlocks(view, "recorded-thread").map((block) => {
    const steers = block.steers.map((steer) => ({ position: steer.position, text: steer.text, atMs: steer.message.createdAtMs, attachments: steer.message.attachments }));
    const sequence = blockSequence({ ...block, steers });
    const finished = blockFinished(block.state);
    const live = isLive(block.state);
    const answering = !finished && live ? answeringIndex(block.texts, sequence) : null;
    const done = finished || answering !== null;
    const answer = answering ?? answerIndex(block.texts, finished);
    const parts = turnParts(turnActivity(sequence, false), answer, done, live, (index) => {
      const text = block.texts[index];
      return !!text && !text.commentary && !text.update;
    });
    return {
      key: block.key,
      state: block.state,
      folded: foldsWork(done, answer, parts.some((part) => part.type === "work" && part.items.length > 0)),
      parts: parts.map((part) =>
        part.type === "between"
          ? { between: part.entry.kind }
          : {
              work: part.items.map((item) =>
                item.type === "group"
                  ? {
                      group: item.key,
                      summary: summarize(item.items.flatMap((each) => (each.step.kind.type === "tool" ? [toolStepWords(each.step.kind)] : []))),
                      rows: item.items.map((each) => (each.step.kind.type === "tool" ? rowWords(each.step.kind) : each.step.kind.type)),
                    }
                  : { item: item.entry.kind },
              ),
              ...(part.answer && { answer: true }),
            },
      ),
    };
  });
}

function noRunningRowAfterItsTurn(view: ThreadView, where: string) {
  for (const block of cliBlocks(view, "recorded-thread")) {
    if (!blockFinished(block.state)) continue;
    for (const kind of toolSteps(block)) {
      assert.ok(kind.status !== "inProgress" || kind.background, `${where}: ${kind.itemId} still runs in ended block ${block.key}`);
    }
  }
}

/** The newest block's calls in the order drawn, and its groups' keys. */
function newestShape(view: ThreadView): { order: string[]; groups: Map<string, string> } {
  const block = cliBlocks(view, "recorded-thread").at(-1);
  if (!block) return { order: [], groups: new Map() };
  const steers = block.steers.map((steer) => ({ position: steer.position, text: steer.text, atMs: steer.message.createdAtMs, attachments: steer.message.attachments }));
  const activity = turnActivity(blockSequence({ ...block, steers }), false);
  const groups = new Map<string, string>();
  const order: string[] = [];
  for (const item of activity) {
    if (item.type !== "group") continue;
    const ids = item.items.flatMap((each) => (each.step.kind.type === "tool" ? [each.step.kind.itemId] : []));
    order.push(...ids);
    if (ids[0] !== undefined) groups.set(ids[0], item.key);
  }
  return { order, groups };
}

/** The calls of `order` that `other` has too, in the order of `order`. */
function both(order: string[], other: string[]): string[] {
  return order.filter((id) => other.includes(id));
}

/**
 * The turn as read again from the file at its end against how it was drawn live just before:
 * no call changes place, and each group keeps its key (an opened one stays open, D44).
 */
function keepsItsPlace(live: ReturnType<typeof newestShape>, reread: ReturnType<typeof newestShape>, where: string) {
  assert.deepEqual(both(reread.order, live.order), both(live.order, reread.order), `${where}: a call moved`);
  for (const [first, key] of live.groups) {
    if (reread.groups.has(first)) assert.equal(reread.groups.get(first), key, `${where}: group of ${first}`);
  }
}

/** The view after each recorded event, the turn running until its end, then read again from the file. */
function* replay(fixture: Fixture): Generator<[string, ThreadView]> {
  const view = fixture.persisted;
  const starts = view.entries.flatMap((entry, index) => (entry.event.type === "message" && entry.event.role === "user" && !entry.parent ? [index] : []));
  let entries: FileEntry[] = [];
  let turn = -1;
  let at = 1_800_000_000_000;
  for (const [nth, event] of fixture.events.entries()) {
    at += 1000;
    if (event.type === "message" && event.role === "user") turn += 1;
    if (event.type === "turnCompleted") {
      const next = starts[turn + 1] ?? view.entries.length;
      yield [`event ${nth} (turn ${turn} end, reread)`, { ...view, entries: view.entries.slice(0, next), turns: view.turns.slice(0, turn + 1) }];
      continue;
    }
    if (turn < 0 || !("itemId" in event || event.type === "compactionEnded")) continue;
    entries = mergeEntries(entries, [{ event, atMs: at } as FileEntry]);
    const turns = view.turns.slice(0, turn + 1).map((each, index) => (index === turn ? { ...each, end: null, endedAtMs: null } : each));
    yield [`event ${nth}`, { ...view, entries, turns }];
  }
}

for (const folder of folders) {
  const base = new URL(`${folder.name}/`, root);
  const index = JSON.parse(readFileSync(new URL("index.json", base), "utf8")) as { scenarios: Record<string, { view: string }> };
  for (const name of Object.keys(index.scenarios)) {
    test(`${folder.name}/${name}: the turn's work blocks, live, reloaded and replayed`, () => {
      const fixture = load(folder.name, name);
      const shown = { live: work(fixture.live), persisted: work(fixture.persisted) };
      noRunningRowAfterItsTurn(fixture.live, "live");
      noRunningRowAfterItsTurn(fixture.persisted, "persisted");
      let live: ReturnType<typeof newestShape> | null = null;
      for (const [frame, view] of replay(fixture)) {
        noRunningRowAfterItsTurn(view, frame);
        const shape = newestShape(view);
        const reread = frame.endsWith("reread)");
        if (reread && live) keepsItsPlace(live, shape, frame);
        live = reread ? null : shape;
      }
      const path = new URL(`${name}.work.json`, base);
      if (process.env.UPDATE_FIXTURES === "1") writeFileSync(path, `${JSON.stringify(shown, null, 2)}\n`);
      assert.deepEqual(shown, JSON.parse(readFileSync(path, "utf8")), `${fileURLToPath(path)} changed; inspect before UPDATE_FIXTURES=1`);
    });
  }
}

test("a turn that ended with no reply keeps its work in view, live and reread, on every CLI (D44, D50)", () => {
  for (const folder of folders) {
    const fixture = load(folder.name, "tools");
    for (const source of ["live", "persisted"] as const) {
      const view = fixture[source];
      const worked = (blocks: ReturnType<typeof work>) => blocks.filter((block) => block.state === "done" && block.parts.some((part) => "work" in part));
      const answered = worked(work(view));
      assert.ok(answered.length > 0 && answered.every((block) => block.folded), `${folder.name} ${source}: an answered turn folds`);
      // The same recorded turns with their replies taken out: nothing for the work to fold under.
      const entries = view.entries.filter((entry) => !(entry.event.type === "message" && entry.event.role === "assistant"));
      const silent = worked(work({ ...view, entries }));
      assert.ok(silent.length > 0, `${folder.name} ${source}: a turn with work`);
      for (const block of silent) assert.equal(block.folded, false, `${folder.name} ${source}: ${block.key} folded with no reply`);
    }
  }
});

/** The recorded `sleep 30` call, as the CLI's file keeps it when the turn ends before its result. */
function cutOff(end: "interrupted" | "failed"): { view: ThreadView; itemId: string } {
  const fixture = load("claude-2.1.310", "interrupt");
  const start = fixture.events.find((event) => event.type === "command" && event.command === "sleep 30" && event.status === "inProgress");
  assert.ok(start?.type === "command");
  const view = fixture.persisted;
  const entries = view.entries.map((entry) => (entry.event.type === "command" && entry.event.itemId === start.itemId ? { ...entry, event: start } : entry));
  const turns = view.turns.map((turn) => (turn.end === "stopped" ? { ...turn, end, error: null } : turn));
  return { view: { ...view, entries, turns }, itemId: start.itemId };
}

test("a call still running when its turn failed or was cut off reads stopped, as its turn ended", () => {
  for (const end of ["interrupted", "failed"] as const) {
    const { view, itemId } = cutOff(end);
    const turn = view.turns.find((each) => each.end === end);
    const step = cliBlocks(view, "recorded-thread").flatMap(toolSteps).find((kind) => kind.itemId === itemId);
    assert.equal(step?.status, "failed", end);
    assert.equal(step?.stopped, true, end);
    assert.equal(step?.endedAtMs, turn?.endedAtMs, end);
    // While the turn runs, it runs.
    const running = { ...view, turns: view.turns.map((each) => (each.end === end ? { ...each, end: null, endedAtMs: null } : each)) };
    assert.equal(cliBlocks(running, "recorded-thread").flatMap(toolSteps).find((kind) => kind.itemId === itemId)?.status, "inProgress");
  }
});

/** The recorded `sleep 2` call with the CLI's background task as its event last said it. */
function inBackground(task: (tasks: BackgroundTask[]) => BackgroundTask | undefined): { step: ToolStep | undefined; task: BackgroundTask } {
  const fixture = load("claude-2.1.310", "background");
  const said = fixture.events.flatMap((event) => (event.type === "backgroundTask" && event.task.itemId !== null ? [event.task] : []));
  const shown = task(said);
  assert.ok(shown);
  const view = { ...fixture.persisted, background: [shown] };
  return { step: cliBlocks(view, "recorded-thread").flatMap(toolSteps).find((kind) => kind.itemId === shown.itemId), task: shown };
}

test("a call its CLI runs in the background reads running until its task ends, then as the task ended", () => {
  const running = inBackground((tasks) => tasks.find((task) => task.status === "running"));
  assert.equal(running.step?.status, "inProgress");
  assert.equal(running.step?.background, true);
  assert.equal(running.step?.endedAtMs, undefined);
  assert.equal(stepLabel(toolStepWords(running.step!), "inProgress"), "Running sleep 2 in the background");

  const ended = inBackground((tasks) => tasks.find((task) => task.status === "completed"));
  assert.equal(ended.step?.status, "completed");
  assert.equal(ended.step?.endedAtMs, ended.task.endedAtMs);
  assert.equal(rowWords(ended.step!), "Ran sleep 2");

  const stopped = inBackground((tasks) => {
    const done = tasks.find((task) => task.status === "completed");
    return done && { ...done, status: "stopped" };
  });
  assert.equal(stopped.step?.stopped, true);
  assert.equal(rowWords(stopped.step!), "stopped: Ran sleep 2");
});

/**
 * The recorded `sleep 2` (run in the background) or `sleep 30` call as one of the thread's own
 * `brigadier` calls: the recording's events, only the command's text changed (D29: no new
 * recording for it).
 */
function asBrigadier(scenario: "background" | "interrupt", command: string, board?: (atMs: number, requestId: string) => BoardDigest) {
  const fixture = load("claude-2.1.310", scenario);
  const recorded = scenario === "background" ? "sleep 2" : "sleep 30";
  const view = fixture.persisted;
  const call = view.entries.find((entry) => entry.event.type === "command" && entry.event.command === recorded);
  assert.ok(call?.event.type === "command" && call.atMs !== undefined);
  const { itemId } = call.event;
  const label = "Wait for the workers";
  const entries = view.entries.map((entry) => {
    if (entry === call && entry.event.type === "command") return { ...entry, event: { ...entry.event, command } };
    if (entry.event.type === "taskNotice") return { ...entry, event: { ...entry.event, text: `Background command "${label}" completed (exit code 0)` } };
    return entry;
  });
  const task = fixture.events.flatMap((event) => (event.type === "backgroundTask" && event.task.status === "completed" ? [{ ...event.task, label }] : [])).at(-1);
  const changed = { ...view, entries, background: task ? [task] : [] };
  // The board's rows go in the block of the call's request.
  const key = cliBlocks(changed, "recorded-thread").find((block) => toolSteps(block).some((kind) => kind.itemId === itemId))?.key ?? "";
  const blocks = cliBlocks(changed, "recorded-thread", board?.(call.atMs, key));
  return { blocks, step: blocks.flatMap(toolSteps).find((kind) => kind.itemId === itemId) };
}

const NO_BOARD: BoardDigest = { tasks: {}, questions: {}, orchestratorSteps: [], machineSteps: [], decisions: [], compactions: {} };

/** What the recorded call's row says once done, as `command`. */
function doneWords(command: string): string {
  const { step } = asBrigadier("interrupt", command);
  assert.ok(step, command);
  return toolStepWords({ ...step, status: "completed", stopped: false }).done;
}

/** The board with the "Messaged" row a `brigadier send` made, just after the call. */
function messaged(atMs: number, requestId: string): BoardDigest {
  return { ...NO_BOARD, orchestratorSteps: [{ requestId, atMs: atMs + 300, position: 0, kind: { type: "messaged", taskId: "b36f2e6e", text: "Go on." } }] };
}

test("the thread's own brigadier wait and start are no rows, and a wait's end no line (D38, D47)", () => {
  const waited = asBrigadier("background", "brigadier wait --any --timeout 600");
  assert.equal(waited.step, undefined);
  assert.deepEqual(waited.blocks.flatMap((block) => block.taskNotices), []);
  // Any other background command's end still says so.
  assert.equal(load("claude-2.1.310", "background").persisted.entries.filter((entry) => entry.event.type === "taskNotice").length, 1);
  assert.equal(cliBlocks(load("claude-2.1.310", "background").persisted, "recorded-thread").flatMap((block) => block.taskNotices).length, 1);
  assert.equal(asBrigadier("interrupt", 'cd /repo && brigadier start "Fix the parser" --title Parser').step, undefined);
});

test("the thread's other brigadier calls read in plain words, by the first brigadier call of a command (D47)", () => {
  assert.equal(doneWords('brigadier ledger add --kind decision --source user "Ship it"'), "Updated the ledger");
  assert.equal(doneWords("brigadier ledger"), "Read the ledger");
  assert.equal(doneWords("brigadier workers"), "Checked the workers");
  assert.equal(doneWords("brigadier models; brigadier ledger show"), "Checked the models");
  assert.equal(doneWords("brigadier context"), "Read the brigadier context");
  assert.equal(doneWords("brigadier close b36f2e6e"), "Closed a worker");
  assert.equal(doneWords("brigadier send b36f2e6e \"$(cat <<'EOF'\nGo on.\nEOF\n)\""), "Messaged workers");
  // Words inside an argument are no call of brigadier's.
  assert.equal(doneWords('echo "brigadier ledger add"'), 'Ran echo "brigadier ledger add"');
});

test("a brigadier send whose message has its own Messaged row is no row of its own (D47)", () => {
  const send = "brigadier send b36f2e6e \"$(cat <<'EOF'\nGo on.\nEOF\n)\"";
  const shown = asBrigadier("interrupt", send, messaged);
  assert.equal(shown.step, undefined);
  assert.ok(shown.blocks.some((block) => block.orchestratorSteps.some((step) => step.kind.type === "messaged")));
  // A Messaged row from another time is another message: the send keeps its row.
  assert.ok(asBrigadier("interrupt", send, (atMs, requestId) => messaged(atMs + 60_000, requestId)).step);
});

/** The recorded ToolSearch call as another tool: the recording's event, only its name and input changed (D29). */
function recordedCallAs(name: string, input: string): ToolStep | undefined {
  const view = load("claude-2.1.310", "tools").persisted;
  const call = view.entries.find((entry) => entry.event.type === "toolCall" && entry.event.name === "ToolSearch");
  assert.ok(call?.event.type === "toolCall");
  const { itemId } = call.event;
  const entries = view.entries.map((entry) => (entry === call && entry.event.type === "toolCall" ? { ...entry, event: { ...entry.event, name, input } } : entry));
  return cliBlocks({ ...view, entries }, "recorded-thread").flatMap(toolSteps).find((kind) => kind.itemId === itemId);
}

test("a subagent's row names its task and a skill's its name; Codex's wait for its agents is no row (D44, B1)", () => {
  const agent = recordedCallAs("Agent", JSON.stringify({ description: "Find the parser", prompt: "Look in src.", subagent_type: "Explore" }));
  assert.ok(agent);
  assert.equal(rowWords(agent), "Ran agent: Find the parser");
  const codex = recordedCallAs("agents/spawnAgent", "Review the diff\nFocus on the reader.");
  assert.ok(codex);
  assert.equal(rowWords(codex), "Ran agent: Review the diff");
  const skill = recordedCallAs("Skill", JSON.stringify({ skill: "brigade" }));
  assert.ok(skill);
  assert.equal(rowWords(skill), "Used skill brigade");
  assert.equal(recordedCallAs("agents/wait", ""), undefined);
});
