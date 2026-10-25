import assert from "node:assert/strict";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { turnActivity } from "@/app/conversation/activity/group";
import { stepLabel, summarize, toolStepWords, type ToolStep } from "@/app/conversation/activity/words";
import { answerIndex, answeringIndex, type Block, blockFinished, blockSequence, isLive } from "@/app/conversation/blocks";
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
      folded: done,
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
      for (const [frame, view] of replay(fixture)) noRunningRowAfterItsTurn(view, frame);
      const path = new URL(`${name}.work.json`, base);
      if (process.env.UPDATE_FIXTURES === "1") writeFileSync(path, `${JSON.stringify(shown, null, 2)}\n`);
      assert.deepEqual(shown, JSON.parse(readFileSync(path, "utf8")), `${fileURLToPath(path)} changed; inspect before UPDATE_FIXTURES=1`);
    });
  }
}

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
