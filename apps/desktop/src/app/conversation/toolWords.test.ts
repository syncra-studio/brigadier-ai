import assert from "node:assert/strict";
import { test } from "node:test";

import {
  toolActivity,
  toolHasOwnResult,
  toolName,
  toolWords,
  type ToolStep,
} from "@/app/conversation/toolWords";
import type { Task } from "@/ipc/generated";

const call = (
  name: string,
  status: ToolStep["status"] = "completed",
  detail: string | null = null,
): ToolStep => ({
  type: "tool",
  name,
  status,
  detail,
  itemId: "call",
  throughPosition: 12,
});

test("every registered action keeps its verb across provider namespaces and outcomes", () => {
  const actions = [
    "query_brain",
    "code_search",
    "code_refs",
    "project_map",
    "delegate_task",
    "message_worker",
    "answer_worker",
    "route_follow_up",
    "stop_worker",
    "list_tasks",
    "read_report",
    "read_artifact",
    "remember",
    "search_transcript",
    "plan_phases",
    "propose_phases",
    "propose_overnight",
    "approve_outline",
    "start_verifier",
    "review_code",
    "review_plan",
    "request_approval",
    "ask_user",
    "ask_orchestrator",
    "submit_outline",
    "submit_report",
    "land_phase",
    "phase_done",
    "finish_session",
    "note_for_user",
    "WebSearch",
    "WebFetch",
    "Read",
    "Edit",
    "exec_command",
    "apply_patch",
    "run",
    "run_unsandboxed",
    "start_preview",
    "stop_preview",
    "preview_log",
    "shell",
  ];
  for (const action of actions) {
    for (const name of [
      action,
      `mcp__brigadier__${action}`,
      `functions.${action}`,
      `brigadier/${action}`,
    ]) {
      assert.equal(toolName(name), action);
      assert.equal(toolActivity(name).done, toolActivity(action).done);
      for (const status of [
        "completed",
        "inProgress",
        "failed",
        "declined",
      ] as const) {
        assert.doesNotMatch(
          toolWords(call(name, status)),
          /Used a tool|Using a tool|mcp__|functions\.|brigadier\//,
        );
      }
    }
  }
  assert.equal(
    toolWords(call("custom_server/export_document")),
    "Called export document",
  );
  assert.equal(toolWords(call("tool")), "Completed an action");
  assert.equal(
    toolWords(call("query_brain", "failed")),
    "Searched project memory — failed",
  );
});

test("commands name the command while retaining its outcome", () => {
  assert.equal(toolWords(call("functions.exec_command", "inProgress", "pnpm typecheck")), "Running pnpm typecheck");
  assert.equal(toolWords(call("Bash", "failed", "pnpm test settings")), "Ran pnpm test settings — failed");
});

test("worker references become their names, including unavailable references", () => {
  const tasks = {
    worker: {
      id: "worker",
      number: 2,
      title: "Map terminal panes",
      role: null,
      subject: null,
    } as Task,
  };
  assert.equal(
    toolWords(call("read_report", "completed", "task-2"), tasks),
    "Read “Map terminal panes”’s report",
  );
  assert.equal(
    toolWords(call("message_worker", "completed", "worker"), tasks),
    "Sent a message to “Map terminal panes”",
  );
  assert.equal(
    toolWords(call("answer_worker", "failed", "task-9"), tasks),
    "Answered a worker: a worker — failed",
  );
  assert.match(
    toolWords(call("read_file", "completed", "docs/task-2.md"), tasks),
    /docs\/task-2.md/,
  );
});

test("deduplication needs the matching authored result in this call's window", () => {
  const raw = call("mcp__brigadier__delegate_task");
  const result = {
    requestId: "request",
    position: 13,
    kind: { type: "created" },
  };
  assert.equal(toolHasOwnResult(raw, [], "request", 10), false);
  assert.equal(toolHasOwnResult(raw, [result], "request", 10), true);
  assert.equal(toolHasOwnResult(raw, [result], "other", 10), false);
  assert.equal(
    toolHasOwnResult({ ...raw, status: "failed" }, [result], "request", 10),
    false,
  );
  assert.equal(
    toolHasOwnResult(raw, [{ ...result, position: 9 }], "request", 10),
    false,
  );
  assert.equal(
    toolHasOwnResult(
      raw,
      [
        { ...result, position: 13, kind: { type: "tool" } },
        { ...result, position: 14 },
      ],
      "request",
      10,
    ),
    false,
  );
  assert.equal(
    toolHasOwnResult(
      raw,
      [{ ...result, position: 11, kind: { type: "tool" } }, result],
      "request",
      10,
    ),
    false,
    "an overlapping call's result cannot hide the earlier action",
  );
});

test("the thread's own steps name what they act on", () => {
  assert.equal(
    toolWords(call("Bash", "inProgress", "cargo test -p core")),
    "Running cargo test -p core",
  );
  assert.equal(
    toolWords(call("mcp__brigadier__run", "completed", "pnpm test")),
    "Ran pnpm test",
  );
  assert.equal(
    toolWords(call("shell", "failed", "cargo build")),
    "Ran cargo build — failed",
  );
  assert.equal(
    toolWords(call("Read", "inProgress", "/repo/src/main.rs")),
    "Reading /repo/src/main.rs",
  );
  assert.equal(
    toolWords(call("apply_patch", "completed", "src/a.rs and 2 more")),
    "Edited files: src/a.rs and 2 more",
  );
  assert.equal(
    toolWords(call("mcp__brigadier__start_preview", "inProgress", "web")),
    "Starting a preview: web",
  );
  assert.equal(
    toolWords(call("mcp__brigadier__preview_log", "completed")),
    "Read a preview’s output",
  );
});
