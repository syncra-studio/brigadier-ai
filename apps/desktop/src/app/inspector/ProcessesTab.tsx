import { useEffect } from "react";

import { Badge } from "@/components/controls";
import { formatDateTime, formatMb } from "@/lib/format";
import { refreshDiagnostics } from "@/state/actions";
import { useApp } from "@/state/store";

const REFRESH_MS = 2000;

/** Refreshes diagnostics every 2 s while mounted and the window is visible. */
export function useDiagnosticsPolling() {
  const visible = useApp((s) => s.windowVisible);
  const connected = useApp((s) => s.connection.status === "connected");
  useEffect(() => {
    if (!visible || !connected) return;
    const refresh = () =>
      void refreshDiagnostics().catch(() => {
        // Retried on the next tick; the connection pill shows outages.
      });
    refresh();
    const timer = setInterval(refresh, REFRESH_MS);
    return () => clearInterval(timer);
  }, [visible, connected]);
}

export function ProcessesTab() {
  useDiagnosticsPolling();
  const processes = useApp((s) => s.inspector.diagnostics?.processes ?? null);

  if (!processes) {
    return <p className="text-muted-foreground p-4 text-xs">Loading processes…</p>;
  }
  return (
    <table data-selectable className="w-full text-xs">
      <thead className="text-muted-foreground text-start">
        <tr className="h-row-sm border-b">
          <th className="px-3 text-start font-medium">Role</th>
          <th className="px-2 text-end font-medium">PID</th>
          <th className="px-2 text-start font-medium">Name</th>
          <th className="px-2 text-end font-medium">RSS</th>
          <th className="px-2 text-end font-medium">CPU</th>
          <th className="px-3 text-start font-medium">Started</th>
        </tr>
      </thead>
      <tbody>
        {processes.map((process) => (
          <tr key={process.pid} className="h-row-sm border-b last:border-b-0">
            <td className="px-3">
              <Badge variant="secondary">{process.role}</Badge>
            </td>
            <td className="px-2 text-end font-mono tabular-nums">{process.pid}</td>
            <td className="max-w-0 truncate px-2">{process.name}</td>
            <td className="px-2 text-end tabular-nums">{formatMb(process.rssBytes)}</td>
            <td className="px-2 text-end tabular-nums">
              {process.cpuPercent.toFixed(1)}%
            </td>
            <td className="text-muted-foreground px-3 whitespace-nowrap">
              {process.startedAtMs === null ? "—" : formatDateTime(process.startedAtMs)}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
