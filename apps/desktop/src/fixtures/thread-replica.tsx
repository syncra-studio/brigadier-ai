/** Production thread with namespaced calls, failures, reasoning, workers and web results. */
import { leadTranscript } from "@/fixtures/flow";
import type { OrchestratorStep, ThinkingSegment } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const done = query.get("view") === "done";
const now = Date.now();
const step = (
  position: number,
  name: string,
  status: "completed" | "failed" | "inProgress",
  detail: string | null,
): OrchestratorStep => ({
  requestId: "r2",
  atMs: now - 120000 + position * 100,
  position,
  kind: {
    type: "tool",
    itemId: `call-${position}`,
    name,
    status,
    detail,
    throughPosition: position + 0.1,
  },
});
const thought = (
  itemId: string,
  position: number,
  text: string,
  start: number,
  end: number,
  complete: boolean,
): ThinkingSegment => ({
  itemId,
  position,
  throughPosition: position,
  requestId: "r2",
  text,
  startedAtMs: now - start * 1000,
  updatedAtMs: now - end * 1000,
  complete,
});

useBoard.setState(({ board }) => {
  if (!board) return {};
  const transcript = board.transcripts.t2!;
  const entries = transcript.entries.map((entry) =>
    done &&
    entry.event.type === "command" &&
    entry.event.status === "inProgress"
      ? {
          ...entry,
          event: {
            ...entry.event,
            status: "completed" as const,
            output: "12 tests passed",
            exitCode: 0,
            durationMs: 3100,
          },
        }
      : entry,
  );
  leadTranscript.splice(0, leadTranscript.length, ...entries);
  const extra = [
    step(11.2, "functions.query_brain", "failed", null),
    step(
      11.4,
      "mcp__brigadier__query_brain",
      "completed",
      "theme settings and first paint",
    ),
    step(11.6, "brigadier/project_map", "completed", "apps/desktop/src"),
    step(11.65, "functions.read_file", "completed", "apps/desktop/src/state/settings.ts"),
    step(11.7, "functions.exec_command", "completed", "pnpm typecheck"),
    // Each successful call gives way to its authored result; no duplicate generic row.
    step(
      11.8,
      "mcp__brigadier__delegate_task",
      "completed",
      "Map settings storage",
    ),
    step(15.2, "functions.read_report", "failed", "task-2"),
    step(
      15.4,
      "mcp__brigadier__remember",
      "completed",
      "Keep the theme in the settings store",
    ),
    { requestId: "r2", atMs: now - 90000, position: 15.6, kind: { type: "searchedWeb", query: "prefers-color-scheme theme initialization" } } satisfies OrchestratorStep,
    ...(done
      ? []
      : [
          step(
            25,
            "mcp__brigadier__code_search",
            "inProgress",
            "prefers-color-scheme",
          ),
        ]),
  ];
  return {
    board: {
      ...board,
      run: done ? "idle" : "running",
      runRequest: done ? null : "r2",
      orchestratorSteps: [...board.orchestratorSteps, ...extra].toSorted(
        (a, b) => a.position - b.position,
      ),
      thinking: [
        thought(
          "initial",
          11.1,
          "I’ll inspect the settings store and keep the theme choice alongside the other preferences.",
          128,
          121,
          true,
        ),
        ...(query.get("thinking") === "1"
          ? [
              thought(
                "current",
                26,
                "The theme now applies before the first paint.\nI’m checking the remaining tests and the system preference changes.",
                7,
                done ? 2 : 0,
                done,
              ),
            ]
          : []),
      ],
      transcripts: { ...board.transcripts, t2: { ...transcript, entries } },
    },
  };
});

// The current request alone keeps every state visible at a reproducible viewport.
useApp.setState((state) => {
  const id = "flow-session";
  const thread = state.threads[id];
  if (!thread) return {};
  return {
    threads: {
      ...state.threads,
      [id]: {
        ...thread,
        items: thread.items.filter((message) => message.requestId === "r2"),
      },
    },
  };
});
