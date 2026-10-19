import { Mcp } from "@openai/apps-sdk-ui/components/Icon";
import { type ComponentProps, useId, useState } from "react";

import { mono } from "@/components/assistant-ui/elements/surfaces";
import { summaryRow, summaryRowInteractive } from "@/components/assistant-ui/elements/summary-section";
import { Spinner } from "@/components/glyphs/spinner";
import { cn } from "@/lib/utils";

/**
 * The tool servers an agent has (assistant-ui's MCP server panel element,
 * https://www.assistant-ui.com/elements, on Brigadier's tokens and icons): one line per server,
 * its state in its icon and words, its tools behind a click. Signing in to a server is the
 * agent's own business: "Needs sign-in" is said, not offered.
 */

export type McpServerStatus = "connected" | "connecting" | "needs-auth" | "failed";

export interface McpServer {
  id: string;
  name: string;
  /** How the agent reaches it ("stdio", "http"), when the agent says. */
  transport?: string | undefined;
  status: McpServerStatus;
  /** The state as the agent reported it when it started, not kept current since. */
  atStart?: boolean | undefined;
  tools: readonly string[];
}

/** A server's state in words: at the row's end in place of its tools until it connects. */
const STATUS_WORDS: Record<McpServerStatus, string> = {
  connected: "Connected",
  connecting: "Connecting",
  "needs-auth": "Needs sign-in",
  failed: "Failed",
};

function ServerRow({ server }: { server: McpServer }) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const tools = server.tools.length;
  return (
    <li className="flex min-w-0 flex-col">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen(!open)}
        className={cn(summaryRow, summaryRowInteractive, "isolate")}
      >
        <span aria-hidden className="text-muted-foreground flex min-w-icon-md shrink-0 items-center justify-center">
          {server.status === "connecting" ? (
            <Spinner className="size-icon-md motion-safe:animate-spin" />
          ) : (
            <Mcp className={cn("size-icon-md", server.status === "failed" && "text-destructive")} />
          )}
        </span>
        <span className={cn("min-w-0 flex-1 truncate", server.status === "connecting" && "text-muted-foreground")}>
          {server.name}
        </span>
        <span
          className={cn(
            "shrink-0 text-xs tabular-nums",
            server.status === "failed" ? "text-destructive" : "text-muted-foreground/70",
          )}
        >
          {server.status === "connected" ? `${tools} ${tools === 1 ? "tool" : "tools"}` : STATUS_WORDS[server.status]}
        </span>
      </button>
      <div id={id} hidden={!open} className="flex min-w-0 flex-col gap-1 ps-6 pb-2">
        <div className="text-muted-foreground flex min-w-0 items-center gap-1.5 text-xs">
          {server.transport && (
            <>
              <span className={mono}>{server.transport}</span>
              <span aria-hidden>·</span>
            </>
          )}
          <span>
            {STATUS_WORDS[server.status]}
            {server.atStart && " at start"}
          </span>
        </div>
        {tools > 0 && (
          <p className={cn(mono, "text-muted-foreground wrap-anywhere")}>{server.tools.join(" · ")}</p>
        )}
      </div>
    </li>
  );
}

export function McpServerPanel({
  servers,
  className,
  ...props
}: Omit<ComponentProps<"ul">, "children"> & { servers: readonly McpServer[] }) {
  return (
    <ul data-slot="mcp-server-panel" className={cn("flex w-full min-w-0 flex-col", className)} {...props}>
      {servers.map((server) => (
        <ServerRow key={server.id} server={server} />
      ))}
    </ul>
  );
}
