import { type ReactNode, useMemo } from "react";

import { hostOf, useUsedSources } from "@/app/conversation/PinnedSummary";
import { McpServerPanel } from "@/components/assistant-ui/elements/mcp-server-panel";
import { Globe, Search } from "@/components/ui/icon";
import { useMcpServers } from "@/state/mcpServers";
import { toast } from "@/state/toasts";
import { useBoard } from "@/state/board";
import { openLink } from "@/state/links";

/**
 * Every source the session used, in a tab of its own (the context card's Sources › View all):
 * the MCP servers its agents have connected, under each agent's name, then what it used
 * (attachments, links, integrations), then each web search with its words and each page it
 * opened, which opens in the browser again.
 */
export function SourcesTab({ conversationId }: { conversationId: string }) {
  const used = useUsedSources(conversationId).filter((source) => source.key !== "web");
  const mcp = useMcpServers(conversationId);
  const json = useBoard((s) =>
    s.board?.conversationId === conversationId
      ? JSON.stringify(
          s.board.orchestratorSteps.flatMap((step) =>
            step.kind.type === "searchedWeb" ? [["search", step.kind.query]] : step.kind.type === "readPage" ? [["page", step.kind.url]] : [],
          ),
        )
      : "[]",
  );
  const { searches, pages } = useMemo(() => {
    const steps = JSON.parse(json) as ["search" | "page", string][];
    const of = (kind: "search" | "page") => [...new Set(steps.flatMap(([each, value]) => (each === kind ? [value] : [])))];
    return { searches: of("search"), pages: of("page") };
  }, [json]);
  const empty = mcp.length === 0 && used.length === 0 && searches.length === 0 && pages.length === 0;
  return (
    <div data-slot="sources-tab" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="max-w-thread mx-auto flex w-full flex-col gap-6 px-5 pt-6 pb-10">
        {empty && <p className="text-muted-foreground text-sm">No sources yet.</p>}
        {mcp.length > 0 && (
          <section data-slot="sources-part" className="flex flex-col gap-3">
            <h3 className="text-muted-foreground text-label">MCP servers</h3>
            {mcp.map((group) => (
              <div key={group.key} data-slot="sources-mcp" className="flex flex-col gap-1">
                {/* Whose servers: each agent's CLI connects its own. */}
                <p className="text-muted-foreground text-xs">{group.agentLabel}</p>
                <McpServerPanel servers={group.servers} />
              </div>
            ))}
          </section>
        )}
        {used.length > 0 && (
          <Part title="Used">
            {used.map((source) => (
              <Line key={source.key} icon={source.icon} title={source.title} onClick={source.open}>
                {source.label}
              </Line>
            ))}
          </Part>
        )}
        {searches.length > 0 && (
          <Part title={`Searched ${searches.length} ${searches.length === 1 ? "time" : "times"}`}>
            {searches.map((query) => (
              <Line key={query} icon={<Search />} title={query}>
                {query}
              </Line>
            ))}
          </Part>
        )}
        {pages.length > 0 && (
          <Part title={`Opened ${pages.length} ${pages.length === 1 ? "page" : "pages"}`}>
            {pages.map((url) => (
              <Line key={url} icon={<Globe />} title={url} onClick={() => open(url)}>
                {hostOf(url)}
                <span className="text-muted-foreground ms-2 min-w-0 truncate">{url}</span>
              </Line>
            ))}
          </Part>
        )}
      </div>
    </div>
  );
}

function open(url: string): void {
  openLink(url, { agent: true }).catch((error: unknown) =>
    toast(error instanceof Error ? error.message : String(error), { tone: "error" }),
  );
}

function Part({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section data-slot="sources-part" className="flex flex-col gap-1">
      <h3 className="text-muted-foreground text-label">{title}</h3>
      <ul className="flex flex-col">{children}</ul>
    </section>
  );
}

/** One source on one line: its icon and words; a click opens it, when it opens. */
function Line({ icon, title, onClick, children }: { icon: ReactNode; title: string; onClick?: (() => void) | undefined; children: ReactNode }) {
  const body = (
    <>
      <span aria-hidden className="text-muted-foreground flex size-4 shrink-0 items-center justify-center [&_svg]:size-4">{icon}</span>
      <span className="flex min-w-0 flex-1 items-baseline truncate">{children}</span>
    </>
  );
  return (
    <li data-slot="sources-line" title={title} className="text-sm">
      {onClick ? (
        <button type="button" onClick={onClick} className="hover:bg-foreground/8 rounded-control flex h-row-sm w-full min-w-0 items-center gap-2 px-2 text-start">
          {body}
        </button>
      ) : (
        <div className="flex h-row-sm min-w-0 items-center gap-2 px-2">{body}</div>
      )}
    </li>
  );
}
