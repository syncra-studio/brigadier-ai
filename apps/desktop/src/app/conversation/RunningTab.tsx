import { useEffect, useState } from "react";

import { previewActions, previewActive, previewStateLabel } from "@/app/conversation/previewStatus";
import { useAction } from "@/app/conversation/useAction";
import { Button } from "@/components/ui/button";
import type { Preview } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";
import { clearPreviews, pausePreview, previewLogTail, resumePreview, stopPreview } from "@/state/actions";
import { useBoard } from "@/state/board";
import { changeSessionTab, newSessionTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

export function RunningTab({ conversationId }: { conversationId: string }) {
  const previews = useBoard((s) => s.board?.conversationId === conversationId ? s.board.previews : null);
  const { busy, error, run } = useAction();
  const rows = Object.values(previews ?? {}).toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
  const active = rows.filter((preview) => previewActive(preview.state)).length;
  return (
    <section aria-label="Session previews" className="flex min-h-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center gap-2 px-3 pb-3">
        <span className="text-muted-foreground flex-1 text-xs">{active} active · {rows.length - active} finished</span>
        <Button variant="ghost" size="xs" disabled={busy || !active} onClick={() => run(() => stopPreview(conversationId, null))}>Stop all</Button>
        <Button variant="ghost" size="xs" disabled={busy || rows.length === active} onClick={() => run(() => clearPreviews(conversationId))}>Clear finished</Button>
      </div>
      {error && <p role="alert" className="text-destructive px-3 pb-3 text-xs">{error}</p>}
      <div className="min-h-0 flex-1 overflow-y-auto">
        {rows.length ? rows.map((preview) => <PreviewRow key={preview.id} preview={preview} disabled={busy} />) :
          <p className="text-muted-foreground px-3 py-6 text-sm">Previews started by this session appear here.</p>}
      </div>
    </section>
  );
}

function PreviewRow({ preview, disabled }: { preview: Preview; disabled: boolean }) {
  const platform = useApp((s) => s.info?.platform ?? "");
  const { busy, error, run } = useAction();
  const [expanded, setExpanded] = useState(false);
  const [log, setLog] = useState<string | null>(null);
  const [logError, setLogError] = useState<string | null>(null);
  const [url, setUrl] = useState(preview.url ?? null);
  const [now, setNow] = useState(Date.now);
  const active = previewActive(preview.state);
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function refresh() {
      try {
        const result = await previewLogTail(preview.conversationId, preview.id);
        if (cancelled) return;
        setLog(result.tail);
        setLogError(null);
        if (result.url) setUrl(result.url);
      } catch (cause) {
        if (!cancelled) setLogError(String(cause));
      }
      if (!cancelled && active) timer = setTimeout(() => { void refresh(); }, expanded ? 2000 : 5000);
    }
    void refresh();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [preview.conversationId, preview.id, active, expanded]);
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  const actions = previewActions(preview.state, platform);
  return (
    <article aria-label={preview.name} className="border-border/50 border-t px-3 py-4">
      <div className="flex items-baseline justify-between gap-3">
        <h3 className="min-w-0 break-words text-sm font-medium">{preview.name}</h3>
        <span className="text-muted-foreground shrink-0 text-xs tabular-nums" title={active ? "Time since started, including pauses" : "Duration"}>
          {formatDuration(Math.max(0, (preview.endedAtMs ?? now) - preview.startedAtMs))}
        </span>
      </div>
      <p className={cn("mt-1 break-words text-xs", preview.state.type === "running" ? "text-success" : "text-muted-foreground")}>{previewStateLabel(preview.state)}</p>
      <p className="text-muted-foreground mt-2 break-all font-mono text-xs">{preview.command}</p>
      {url && <Button variant="link" size="xs" className="mt-2 h-auto max-w-full justify-start whitespace-normal break-all p-0 text-start" onClick={() => {
        const tab = newSessionTab(preview.conversationId, "browser");
        changeSessionTab(preview.conversationId, tab, (current) => current.kind === "browser" ? { ...current, url } : current);
      }}>{url}</Button>}
      <div className="mt-3 flex flex-wrap items-center gap-1">
        {actions.map((action) => <Button key={action} variant="outline" size="xs" disabled={disabled || busy} onClick={() => run(() =>
          action === "Stop" ? stopPreview(preview.conversationId, preview.id) : action === "Pause" ? pausePreview(preview.conversationId, preview.id) : resumePreview(preview.conversationId, preview.id),
        )}>{action}</Button>)}
        <Button variant="ghost" size="xs" aria-expanded={expanded} aria-controls={`log-${preview.id}`} onClick={() => setExpanded(!expanded)}>{expanded ? "Hide log" : "Show log"}</Button>
      </div>
      {error && <p role="alert" className="text-destructive mt-2 text-xs">{error}</p>}
      {expanded && <div id={`log-${preview.id}`} className="mt-3">
        {logError && <p role="alert" className="text-destructive mb-2 text-xs">Could not refresh the log: {logError}</p>}
        <pre className="bg-muted/50 text-muted-foreground rounded-control max-h-64 overflow-auto p-3 font-mono text-xs whitespace-pre-wrap break-all">{log ?? (logError ? "Log unavailable." : "Loading log…")}{log === "" ? "No output yet." : ""}</pre>
      </div>}
    </article>
  );
}
