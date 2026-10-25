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
  const preview = useBoard((s) => (s.board?.conversationId === conversationId && view.source === "preview" ? s.board.previews[view.id] : undefined));
  const task = useCliThread((s) => {
    if (view.source !== "task") return undefined;
    if (view.worker === undefined) return s.views[conversationId]?.background?.find((held) => held.id === view.id);
    return workerRowIn(s.views, view.worker)?.row.background?.find((held) => held.task.id === view.id)?.task;
  });
  return (
    <div data-slot="output-tab" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="max-w-thread mx-auto flex w-full flex-col gap-3 px-5 pt-6 pb-10">
        {preview ? <PreviewOutput preview={preview} /> : task ? <TaskOutput conversationId={conversationId} task={task} worker={view.worker} /> : (
          <p className="text-muted-foreground text-sm">It has ended, and what it printed is gone.</p>
        )}
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
      <ShellBox command={task.label} output={text || null} status={status} exit={null}
        stopped={task.status === "stopped"} notes={running && !text ? ["No output yet"] : []} />
    </>
  );
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
