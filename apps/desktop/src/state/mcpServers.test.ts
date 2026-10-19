import assert from "node:assert/strict";
import { test } from "node:test";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import { workerName } from "@/app/conversation/rowWords";
import type { AgentMcpServers, EventEnvelope, McpServerState, Task } from "@/ipc/generated";
import { applyToBoard, emptyBoard } from "@/state/board";
import { mcpServerGroups } from "@/state/mcpServers";

const tasks = night.tasks as unknown as Record<string, Task>;
const byNumber = (number: number): Task => {
  const task = Object.values(tasks).find((entry) => entry.number === number);
  assert.ok(task);
  return task;
};

const updated = (event: Omit<Extract<EventEnvelope["event"], { type: "mcpServersUpdated" }>, "type" | "conversationId" | "boot">, seq: number): EventEnvelope =>
  ({
    stream: "conversation:c",
    streamSeq: seq,
    atMs: seq,
    event: { type: "mcpServersUpdated", conversationId: "c", boot: "b", ...event },
  }) as unknown as EventEnvelope;

test("each agent's MCP servers read as a group under its name, the lead's first, then the workers' in task order", () => {
  const second = byNumber(2);
  const first = byNumber(1);
  const agents: Record<string, AgentMcpServers> = {
    [`task:${second.id}`]: {
      taskId: second.id,
      provider: "codex",
      atStart: false,
      servers: [{ name: "pg", status: "failed", tools: [], transport: null }],
    },
    lead: {
      taskId: null,
      provider: "claude",
      atStart: true,
      servers: [
        { name: "github", status: "connected", tools: ["list_issues"], transport: null },
        { name: "remote", status: "needs-auth", tools: [], transport: "http" },
      ],
    },
    [`task:${first.id}`]: {
      taskId: first.id,
      provider: "claude",
      atStart: true,
      servers: [{ name: "docs", status: "connecting", tools: [], transport: null }],
    },
    "task:not-on-board": {
      taskId: "not-on-board",
      provider: "codex",
      atStart: false,
      servers: [{ name: "x", status: "connected", tools: ["y"], transport: null }],
    },
  };
  assert.deepEqual(mcpServerGroups(agents, tasks), [
    {
      key: "lead",
      agentLabel: "Lead · Claude",
      atStart: true,
      servers: [
        { id: "github", name: "github", transport: undefined, status: "connected", atStart: true, tools: ["list_issues"] },
        { id: "remote", name: "remote", transport: "http", status: "needs-auth", atStart: true, tools: [] },
      ],
    },
    {
      key: `task:${first.id}`,
      agentLabel: workerName(tasks, first),
      atStart: true,
      servers: [{ id: "docs", name: "docs", transport: undefined, status: "connecting", atStart: true, tools: [] }],
    },
    {
      key: `task:${second.id}`,
      agentLabel: workerName(tasks, second),
      atStart: false,
      servers: [{ id: "pg", name: "pg", transport: undefined, status: "failed", atStart: false, tools: [] }],
    },
    {
      key: "task:not-on-board",
      agentLabel: "Worker",
      atStart: false,
      servers: [{ id: "x", name: "x", transport: undefined, status: "connected", atStart: false, tools: ["y"] }],
    },
  ]);
  assert.deepEqual(mcpServerGroups({}, tasks), []);
});

test("an agent's MCP servers on the board follow its updates, and go when it has none", () => {
  const github: McpServerState = { name: "github", status: "connected", tools: [], transport: null };
  let board = applyToBoard(emptyBoard("c"), updated({ taskId: null, provider: "codex", servers: [github], atStart: false }, 1));
  board = applyToBoard(board, updated({ taskId: "t1", provider: "claude", servers: [github], atStart: true }, 2));
  assert.deepEqual(Object.keys(board.mcpServers).toSorted(), ["lead", "task:t1"]);
  assert.equal(mcpServerGroups(board.mcpServers, {})[0]?.agentLabel, "Lead · Codex");
  board = applyToBoard(board, updated({ taskId: null, provider: "codex", servers: [], atStart: false }, 3));
  assert.deepEqual(Object.keys(board.mcpServers), ["task:t1"]);
  // Nothing to remove: the same board.
  assert.equal(applyToBoard(board, updated({ taskId: null, provider: "codex", servers: [], atStart: false }, 4)), board);
});
