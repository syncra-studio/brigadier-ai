import { useEffect, useState } from "react";

import { latestLine, previewActions, previewActive, previewIndicator, previewStateLabel } from "@/app/conversation/previewStatus";
import { useAction } from "@/app/conversation/useAction";
import { Button } from "@/components/ui/button";
import type { Preview } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";
import { BackgroundInbox, BackgroundInboxRow } from "@/components/assistant-ui/elements/background-inbox";
import { clearPreviews, pausePreview, previewLogTail, resumePreview, stopPreview } from "@/state/actions";
import { useBoard } from "@/state/board";
import { openLink } from "@/state/links";
import { useApp } from "@/state/store";

export function PreviewList({ conversationId, onOpenLink }: { conversationId: string; onOpenLink: () => void }) {
  const previews = useBoard((s) => s.board?.conversationId === conversationId ? s.board.previews : null);
  const { busy, error, run } = useAction();
  const rows = Object.values(previews ?? {}).toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
  const active = rows.filter((preview) => previewActive(preview.state)).length;
  const summary = [
    ...(["starting", "running", "paused"] as const).map((state) => {
      const count = rows.filter((preview) => preview.state.type === state).length;
      return count ? `${count} ${state}` : null;
    }),
    rows.length > active ? `${rows.length - active} finished` : null,
  ].filter(Boolean).join(" · ");
  return (
    <BackgroundInbox summary={summary} actions={<>
      <Button variant="ghost" size="xs" disabled={busy || !active} onClick={() => run(() => stopPreview(conversationId, null))}>Stop all</Button>
      <Button variant="ghost" size="xs" disabled={busy || rows.length === active} onClick={() => run(() => clearPreviews(conversationId))}>Clear finished</Button>
    </>}>
      {error && <p role="alert" className="text-destructive px-3 pb-3 text-xs">{error}</p>}
      {rows.map((preview) => <PreviewRow key={preview.id} preview={preview} disabled={busy} onOpenLink={onOpenLink} />)}
    </BackgroundInbox>
  );
}

function PreviewRow({ preview, disabled, onOpenLink }: { preview: Preview; disabled: boolean; onOpenLink: () => void }) {
  const platform = useApp((s) => s.info?.platform ?? "");
  const { busy, error, run } = useAction();
  const [expanded, setExpanded] = useState(false);
  const [log, setLog] = useState<string | null>(null);
  const [logError, setLogError] = useState<string | null>(null);
  // An app preview opens its own window: whatever address it prints isn't it.
  const web = preview.kind === "web";
  const [found, setFound] = useState<string | null>(null);
  const url = web ? preview.url ?? found : null;
  const [now, setNow] = useState(Date.now);
  const active = previewActive(preview.state);
  const starting = preview.state.type === "starting";
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function refresh() {
      try {
        const result = await previewLogTail(preview.conversationId, preview.id);
        if (cancelled) return;
        setLog(result.tail);
        setLogError(null);
        if (result.url) setFound(result.url);
      } catch (cause) {
        if (!cancelled) setLogError(String(cause));
      }
      if (!cancelled && active && (expanded || starting)) timer = setTimeout(() => { void refresh(); }, 2000);
    }
    void refresh();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [preview.conversationId, preview.id, active, expanded, starting]);
  useEffect(() => {
    if (!active) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  const actions = previewActions(preview.state, platform);
  // While it starts (a build can take minutes), its latest output says what it is doing.
  const line = starting && log ? latestLine(log) : "";
  return (
    <BackgroundInboxRow state={previewIndicator(preview.state)} status={previewStateLabel(preview.state)} name={preview.name}
      description={line || `${preview.command} · ${previewStateLabel(preview.state)}`}
      detail={line ? `${preview.command}\n${line}` : `${preview.command} · ${previewStateLabel(preview.state)}`}
      elapsed={formatDuration(Math.max(0, (preview.endedAtMs ?? now) - preview.startedAtMs))}>
      {url && <Button variant="link" size="xs" className="mt-2 h-auto max-w-full justify-start whitespace-normal break-all p-0 text-start" onClick={() => {
        onOpenLink();
        void openLink(url, { agent: true, conversationId: preview.conversationId });
      }}>{url}</Button>}
      <div className="mt-2 flex flex-wrap items-center gap-1">
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
    </BackgroundInboxRow>
  );
}
