import { type ReactNode, useEffect, useState } from "react";

import { ShellBox } from "@/app/conversation/activity/StepDetail";
import { previewActions, previewActive, previewWord } from "@/app/conversation/previewStatus";
import { useAction } from "@/app/conversation/useAction";
import { Button } from "@/components/controls";
import { request } from "@/ipc/client";
import type { BackgroundTask, ItemStatus, Preview } from "@/ipc/generated";
import { pausePreview, previewLogTail, resumePreview, stopBackgroundTask, stopPreview } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useCliThread, workerRowIn } from "@/state/cliThread";
import { openLink } from "@/state/links";
import { findHistoricalOutput, readHistoryOutput, type BackgroundHistoryOutput, type BackgroundHistoryRecord } from "@/state/backgroundHistory";
import { openPanelTab } from "@/state/sessionTabs";
import type { PanelView } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

/** While it runs, what it printed is read again this often. */
const READ_EVERY_MS = 2000;

/**
 * What something in the context card's Background prints, in a tab of its own: a preview's log
 * (with Open for a web page that is up, Pause or Resume where the system can hold it, and Stop) or
 * a CLI's background command's output file (with Stop), read again while it runs. It stays once
 * the thing ends, with what it printed last.
 */
export function OutputTab({ conversationId, view }: { conversationId: string; view: Extract<PanelView, { type: "output" }> }) {
  const preview = useBoard((s) => (s.board?.conversationId === conversationId && view.source === "preview" ? s.board.previews[view.id]?.startedAtMs === (view.startedAtMs ?? s.board.previews[view.id]?.startedAtMs) ? s.board.previews[view.id] : undefined : undefined));
  const task = useCliThread((s) => {
    if (view.source !== "task") return undefined;
    if (view.worker === undefined) return s.views[conversationId]?.background?.find((held) => held.id === view.id && (view.startedAtMs === undefined || held.startedAtMs === view.startedAtMs));
    return workerRowIn(s.views, view.worker)?.row.background?.find((held) => held.task.id === view.id && (view.startedAtMs === undefined || held.task.startedAtMs === view.startedAtMs))?.task;
  });
  return (
    <div data-slot="output-tab" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="max-w-thread mx-auto flex w-full flex-col gap-3 px-5 pt-6 pb-10">
        {view.source === "history" ? <HistoricalOutput conversationId={conversationId} recordId={view.id} />
          : preview ? <PreviewOutput preview={preview} /> : task ? <TaskOutput conversationId={conversationId} task={task} worker={view.worker} />
          : <HistoricalFallback conversationId={conversationId} view={view} />}
      </div>
    </div>
  );
}

/** What a preview or a CLI's background command (of worker `worker`'s CLI, when given) has printed so far. */
function readOutput(source: "preview" | "task", conversationId: string, id: string, worker?: string): Promise<string> {
  return source === "preview"
    ? previewLogTail(conversationId, id).then(({ tail }) => tail)
    : request({ method: "readBackgroundTaskOutput", conversationId, taskId: id, ...(worker !== undefined && { workerId: worker }) }).then(({ output }) => output);
}

/** Reads what it printed now, and again every 2 s while `running`; the last text read stays shown. */
function useOutput(source: "preview" | "task", conversationId: string, id: string, state: string, running: boolean, worker?: string): { text: string | null; error: string | null } {
  const [output, setOutput] = useState<{ text: string | null; error: string | null }>({ text: null, error: null });
  useEffect(() => {
    let current = true;
    const once = () =>
      readOutput(source, conversationId, id, worker).then(
        (text) => current && setOutput({ text, error: null }),
        (cause: unknown) => current && setOutput((shown) => ({ ...shown, error: cause instanceof Error ? cause.message : String(cause) })),
      );
    void once();
    const timer = running ? window.setInterval(() => void once(), READ_EVERY_MS) : null;
    return () => {
      current = false;
      if (timer !== null) window.clearInterval(timer);
    };
    // A new state (it started, held or ended) reads it once more.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [source, conversationId, id, state, running, worker]);
  return output;
}

function PreviewOutput({ preview }: { preview: Preview }) {
  const running = previewActive(preview.state);
  const { text, error } = useOutput("preview", preview.conversationId, preview.id, preview.state.type, running);
  const url = preview.kind === "web" && preview.state.type === "running" ? preview.url : null;
  const platform = useApp((s) => s.info?.platform ?? "");
  const action = useAction();
  const { state, conversationId, id } = preview;
  const status: ItemStatus = running ? "inProgress" : state.type === "exited" && state.code !== 0 ? "failed" : "completed";
  return (
    <>
      <Header title={preview.name} state={previewWord(preview)}>
        {url && (
          <Button variant="ghost" size="xs" onClick={() => void openLink(url, { agent: true, conversationId: preview.conversationId })}>
            Open
          </Button>
        )}
        {previewActions(state, platform).map((label) => (
          <Button key={label} variant="ghost" size="xs" disabled={action.busy} onClick={() => action.run(() =>
            label === "Stop" ? stopPreview(conversationId, id) : label === "Pause" ? pausePreview(conversationId, id) : resumePreview(conversationId, id))}>
            {label}
          </Button>
        ))}
      </Header>
      {action.error && <p role="alert" className="text-destructive text-sm">{action.error}</p>}
      {error && <p className="text-destructive text-sm">{error}</p>}
      <ShellBox command={preview.command} output={text || null} status={status} exit={state.type === "exited" ? state.code : null}
        stopped={state.type === "stopped"} notes={running && !text ? ["No output yet"] : []} />
    </>
  );
}

function TaskOutput({ conversationId, task, worker }: { conversationId: string; task: BackgroundTask; worker?: string | undefined }) {
  const running = task.status === "running";
  const { text, error } = useOutput("task", conversationId, task.id, task.status, running, worker);
  const action = useAction();
  const status: ItemStatus = running ? "inProgress" : task.status === "completed" ? "completed" : "failed";
  return (
    <>
      <Header title={task.label} state={running ? "Running" : task.status === "completed" ? "Finished" : task.status === "stopped" ? "Stopped" : "Failed"}>
        {running && (
          <Button variant="ghost" size="xs" disabled={action.busy} onClick={() => action.run(() => stopBackgroundTask(conversationId, task.id, worker))}>
            Stop
          </Button>
        )}
      </Header>
      {action.error && <p role="alert" className="text-destructive text-sm">{action.error}</p>}
      {error && <p className="text-destructive text-sm">{error}</p>}
      <p className="text-muted-foreground text-xs">Latest output · up to 256 KiB</p>
      <ShellBox command={task.label} output={text || null} status={status} exit={null}
        stopped={task.status === "stopped"} notes={running && !text ? ["No output yet"] : []} />
    </>
  );
}

function HistoricalFallback({ conversationId, view }: { conversationId: string; view: Extract<PanelView, { type: "output" }> }) {
  const [result, setResult] = useState<{ data: BackgroundHistoryOutput | null; error: string | null; loaded: boolean }>({ data: null, error: null, loaded: false });
  useEffect(() => {
    let current = true;
    if (view.source === "history") return;
    findHistoricalOutput(conversationId, view.source, view.id, view.worker, view.startedAtMs).then(
      (data) => { if (current) setResult({ data, error: null, loaded: true }); },
      (error: unknown) => { if (current) setResult({ data: null, error: error instanceof Error ? error.message : String(error), loaded: true }); },
    );
    return () => { current = false; };
  }, [conversationId, view.source, view.id, view.worker, view.startedAtMs]);
  if (result.data) return <HistoricalOutput conversationId={conversationId} recordId={result.data.record.id} initial={result.data.record} />;
  return <p role={result.error ? "alert" : "status"} className="text-muted-foreground text-sm">{result.error ? `Couldn't load historical output: ${result.error}` : result.loaded ? "No historical output was recorded for this activity." : "Loading historical output…"}</p>;
}

/** Opens a durable record even after the CLI, worker, preview or provider output file is gone. */
export function HistoricalOutput({ conversationId, recordId, initial, inline = false }: { conversationId: string; recordId: string; initial?: BackgroundHistoryRecord; inline?: boolean }) {
  const [result, setResult] = useState<{ data: BackgroundHistoryOutput | null; error: string | null }>({ data: null, error: null });
  const [retry, setRetry] = useState(0);
  const state = result.data?.record.status ?? initial?.status;
  useEffect(() => {
    let current = true;
    void retry;
    const once = () => readHistoryOutput(conversationId, recordId).then(
      (data) => { if (current) setResult({ data, error: null }); },
      (error: unknown) => { if (current) setResult((shown) => ({ ...shown, error: error instanceof Error ? error.message : String(error) })); },
    );
    void once();
    const timer = state === "running" ? setInterval(() => void once(), READ_EVERY_MS) : null;
    return () => { current = false; if (timer !== null) clearInterval(timer); };
  // Retry deliberately starts a fresh read after a failed request.
  // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [conversationId, recordId, state, retry]);
  const record = result.data?.record ?? initial;
  const action = useAction();
  if (!record) return <p role={result.error ? "alert" : "status"} className="text-muted-foreground text-sm">{result.error ?? "Loading historical output…"}</p>;
  const running = record.status === "running";
  const itemStatus: ItemStatus = running ? "inProgress" : record.status === "failed" ? "failed" : "completed";
  return <>
    {!inline && <Header title={record.purpose} state={record.status === "unknown" ? "End not observed" : record.status} />}
    {(!inline || record.purpose.includes("\n")) && <p className="text-muted-foreground text-sm whitespace-pre-wrap">{record.purpose}</p>}
    {record.cwd && <p className="text-muted-foreground text-xs break-all">{record.cwd}</p>}
    {record.summary && <p className="text-muted-foreground text-xs">{record.summary}</p>}
    {result.error && <div role="alert" className="flex items-center gap-2 text-sm"><p className="text-destructive">Couldn't load output: {result.error}</p><Button size="xs" variant="ghost" onClick={() => setRetry((n) => n + 1)}>Retry</Button></div>}
    {record.status === "unknown" && record.outputAvailable && <p className="text-muted-foreground text-xs">Last saved output; further output may be missing.</p>}
    {record.outputTruncated && <p className="text-muted-foreground text-xs">Output truncated · latest 1 MiB of {record.outputBytes.toLocaleString()} bytes</p>}
    {result.data?.unavailableReason && <p className="text-muted-foreground text-xs">{result.data.unavailableReason}</p>}
    {!result.data && !result.error && <p role="status" className="text-muted-foreground text-xs">Loading output…</p>}
    <ShellBox command={record.command ?? (record.kind === "agent" ? "Subagent" : "Command not recorded")} output={result.data?.text ?? null}
      status={itemStatus} exit={record.exitCode} stopped={record.status === "stopped"} outcome={record.status === "unknown" ? "End not observed" : record.status === "completed" ? (record.exitCode === null ? "Finished" : `Finished · exit ${record.exitCode}`) : undefined} emptyOutput={record.outputAvailable ? "No output recorded" : "Output unavailable"} />
    <div className="flex flex-wrap gap-2">
      {inline && <Button size="xs" variant="ghost" onClick={() => openPanelTab(conversationId, { type: "output", source: "history", id: record.id }, record.purpose)}>Open output in a tab</Button>}
      {running && record.kind !== "preview" && (record.kind !== "agent" || record.taskId !== record.callId) && <Button size="xs" variant="ghost" disabled={action.busy} onClick={() => action.run(() => stopBackgroundTask(conversationId, record.taskId, record.workerId ?? undefined))}>Stop</Button>}
      {running && record.kind === "preview" && <Button size="xs" variant="ghost" disabled={action.busy} onClick={() => action.run(() => stopPreview(conversationId, record.taskId))}>Stop</Button>}
    </div>
    {action.error && <p role="alert" className="text-destructive text-sm">{action.error}</p>}
  </>;
}

function Header({ title, state, children }: { title: string; state: string; children?: ReactNode }) {
  return (
    <header className="flex min-w-0 items-center gap-2">
      <h2 className="min-w-0 truncate text-sm font-medium">{title}</h2>
      <span className="text-muted-foreground shrink-0 text-sm">{state}</span>
      <span className="flex-1" />
      {children}
    </header>
  );
}
