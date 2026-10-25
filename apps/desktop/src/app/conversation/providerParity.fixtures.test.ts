import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { turnActivity } from "@/app/conversation/activity/group";
import { stepLabel, toolStepWords, type ToolStep } from "@/app/conversation/activity/words";
import { answerIndex, answeringIndex, blockFinished, blockSequence, foldsWork, isLive } from "@/app/conversation/blocks";
import { cliBlocks } from "@/app/conversation/cliBlocks";
import { turnParts } from "@/app/conversation/workParts";
import type { BackgroundTask, FileEntry, ProviderEvent, ThreadView } from "@/ipc/generated";

// D52: the same recorded scenario reads the same on Claude and Codex, as a session draws it,
// live and read again (D50, D44). The recordings are the CLIs' own (D29); two facts the
// recordings can't hold are derived and labelled (D57), and what differs only because a CLI
// does differently is listed in EXEMPT, each with the decision that allows it.
const root = new URL("../../../../../crates/providers/fixtures/versions/", import.meta.url);
const CLAUDE = "claude-2.1.310";
const CODEX = "codex-0.164.0";

type Fixture = { events: ProviderEvent[]; live: ThreadView; persisted: ThreadView };

function load(folder: string, scenario: string): Fixture {
  const base = new URL(`${folder}/`, root);
  const index = JSON.parse(readFileSync(new URL("index.json", base), "utf8")) as { scenarios: Record<string, { view: string }> };
  const view = index.scenarios[scenario]?.view;
  assert.ok(view, `${folder}/${scenario}`);
  return JSON.parse(readFileSync(new URL(view, base), "utf8")) as Fixture;
}

/** The background jobs as the CLI last told them in the recording: the app's `background`. */
function withBackground(view: ThreadView, events: ProviderEvent[]): ThreadView {
  const tasks = new Map<string, BackgroundTask>();
  for (const event of events) if (event.type === "backgroundTask") tasks.set(event.task.id, event.task);
  return { ...view, background: [...tasks.values()] };
}

/**
 * Derived fact 2 (D29, D57): Codex lists its terminals every 2 s, and the recording's session
 * closed before the listing after `sleep 2` ended, so its last word still says running. A
 * terminal whose command item completed with an exit code is ended, as the next listing says.
 */
function withCodexTerminalEnd(view: ThreadView): ThreadView {
  const ended = new Map<string, number>();
  for (const entry of view.entries) {
    const event = entry.event;
    if (event.type === "command" && event.exitCode !== null && event.exitCode !== undefined) ended.set(event.itemId, entry.updatedAtMs ?? entry.atMs ?? 0);
  }
  const background = (view.background ?? []).map((task) => {
    const at = task.itemId ? ended.get(task.itemId) : undefined;
    return task.status === "running" && at !== undefined ? { ...task, status: "completed" as const, endedAtMs: at } : task;
  });
  return { ...view, background };
}

/**
 * Derived fact 1 (D29, D57): Codex's word that a command ended after its turn is the engine's
 * (`lingering_ended` in conversation.rs), never in the CLI's recording: derived here as the
 * engine sends it, where Claude's own word of the same end came, after the compaction.
 */
function withCodexWake(view: ThreadView): ThreadView {
  const command = view.entries.find((entry) => entry.event.type === "command" && entry.event.command.includes("sleep 2"));
  assert.ok(command?.event.type === "command");
  const exit = command.event.exitCode ?? 0;
  const at = view.entries.findIndex((entry) => entry.event.type === "compactionEnded") + 1;
  assert.ok(at > 0);
  const atMs = (view.entries[at - 1]?.atMs ?? 0) + 1;
  const said = (event: ProviderEvent): FileEntry => ({ event, atMs, updatedAtMs: null, parent: null, steer: false, images: [], phase: null }) as unknown as FileEntry;
  const wake = said({ type: "message", itemId: "derived-wake", role: "user", text: `[your background command \`sleep 2\` finished after your turn · exit ${exit}]\n` } as ProviderEvent);
  const reply = said({ type: "message", itemId: "derived-wake-reply", role: "assistant", text: "OK" } as ProviderEvent);
  const entries = [...view.entries.slice(0, at), wake, reply, ...view.entries.slice(at)];
  // The turn it starts carries the request of the turn that started the command.
  const first = view.turns[0]!;
  const turns = [{ ...first, requests: ["r1"] }, { ...first, userMessage: "derived-wake", requests: ["r1"] }, ...view.turns.slice(1)];
  return { ...view, entries, turns };
}

/** A row as the user reads it; a subagent's name is the model's own words. */
function row(kind: ToolStep): string {
  const words = toolStepWords(kind);
  const said = kind.stopped ? `stopped ${words.done}` : stepLabel(words, kind.status, kind.exit ?? null);
  return said.replace(/^(Ran|Running) agent: .*/, "$1 agent: …");
}

/**
 * Each block as the user sees it: its bubble, state, whether its work folds under "Worked for",
 * its rows and lines in order, and whether an answer stands under it. The model's own words
 * (its replies between steps, its prompts' tool names) are not compared.
 */
function seen(view: ThreadView, prompts: RegExp) {
  return cliBlocks(view, "parity", undefined, true)
    .filter((block) => block.user?.kind === "message" && prompts.test(block.user.text))
    .map((block) => {
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
      const shown = parts.flatMap((part) =>
        part.type === "between"
          ? [`between ${part.entry.kind}`]
          : part.items.flatMap((item) =>
              item.type === "group"
                ? item.items.map((each) => (each.step.kind.type === "tool" ? row(each.step.kind) : each.step.kind.type))
                : item.entry.kind === "text"
                  ? []
                  : item.entry.kind === "taskNotice"
                    ? [`line ${block.taskNotices.find((notice) => notice.position === item.entry.position)?.text ?? "?"}`]
                    : [item.entry.kind],
            ),
      );
      return {
        state: block.state,
        folded: foldsWork(done, answer, parts.some((part) => part.type === "work" && part.items.length > 0)),
        shown,
        answer: answer !== null,
      };
    });
}

/** What each scenario compares: its turns, by the words their prompts share on both CLIs. */
const SCENARIOS: Record<string, RegExp> = {
  plain: /^(Run sleep 2|Reply OK\.)/,
  tools: /^(Run sleep 2|Reply OK\.)/,
  background: /^Run sleep 2/,
  compaction: /^Run sleep 2/,
  interrupt: /^Run sleep 30/,
  resume: /^Reply OK\./,
  plan: /ask Ready\?/,
  question: /ask Ready\?/,
  subagent: /^Start one subagent/,
};

/**
 * Where the CLIs genuinely differ, and why that is allowed. None of these shows in what
 * `seen` compares; each is kept here, labelled, so a new difference can't hide behind it.
 * - (a) The model's own words: its texts in the work, a subagent's name. Proposed (D52).
 * - (b) Codex folds when its final answer starts, Claude at turn end: Claude has no answer
 *   phase (D55). The end views compared here don't see it.
 * - (c) Claude's model ends its turn while its background subagent runs, and a second CLI
 *   turn answers when it is done, joined to the same block; Codex waits in one turn (D27, D57).
 */
const EXEMPT = {
  /** (c): Claude's subagent block holds two replies, Codex's one. */
  subagentReplies: { claude: 2, codex: 1 },
};
function compare(scenario: string, source: "live" | "persisted") {
  const prompts = SCENARIOS[scenario]!;
  const claude = load(CLAUDE, scenario);
  const codex = load(CODEX, scenario);
  const codexView = ["plain", "tools", "background", "compaction"].includes(scenario) ? withCodexWake(codex[source]) : codex[source];
  const left = seen(withBackground(claude[source], claude.events), prompts);
  const right = seen(withCodexTerminalEnd(withBackground(codexView, codex.events)), prompts);
  assert.ok(right.length > 0, `${scenario}: no turn compared`);
  assert.deepEqual(left, right, `${scenario} (${source}) reads differently on Claude and Codex`);
}

function replies(folder: string, source: "live" | "persisted"): number[] {
  return cliBlocks(load(folder, "subagent")[source], "parity", undefined, true)
    .filter((block) => block.user?.kind === "message" && SCENARIOS.subagent!.test(block.user.text))
    .map((block) => block.texts.length);
}

for (const scenario of Object.keys(SCENARIOS)) {
  for (const source of ["live", "persisted"] as const) {
    test(`${scenario} reads the same on Claude and Codex, ${source} (D52)`, () => compare(scenario, source));
  }
}

for (const source of ["live", "persisted"] as const) {
  test(`exemption (c): Claude answers its subagent in a second turn of the same block, Codex in one, ${source} (D27, D57)`, () => {
    assert.deepEqual(replies(CLAUDE, source), [EXEMPT.subagentReplies.claude]);
    assert.deepEqual(replies(CODEX, source), [EXEMPT.subagentReplies.codex]);
  });
}
