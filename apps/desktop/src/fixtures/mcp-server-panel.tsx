import { useEffect } from "react";
import { createRoot } from "react-dom/client";

import { type McpServer, McpServerPanel } from "@/components/assistant-ui/elements/mcp-server-panel";

const SERVERS: McpServer[] = [
  { id: "github", name: "github", transport: "http", status: "connected", tools: ["list_issues", "create_pr", "get_diff"] },
  { id: "linear", name: "linear", status: "needs-auth", atStart: true, tools: [] },
  { id: "postgres", name: "postgres", transport: "stdio", status: "failed", tools: [] },
  { id: "figma", name: "figma", transport: "stdio", status: "connecting", tools: [] },
  { id: "docs", name: "docs", transport: "stdio", status: "connected", tools: ["search"] },
];

const frame = () => new Promise((resolve) => requestAnimationFrame(resolve));

function Fixture() {
  useEffect(() => {
    void (async () => {
      await frame();
      const panel = document.querySelector<HTMLElement>('[data-slot="mcp-server-panel"]')!;
      const rows = [...panel.querySelectorAll<HTMLButtonElement>(":scope > li > button")];
      const details = () => [...panel.querySelectorAll<HTMLElement>(":scope > li > div")];
      const seen: Record<string, unknown> = {
        rows: rows.map((row) => row.textContent),
        heights: [...new Set(rows.map((row) => row.getBoundingClientRect().height))],
        closed: details().map((detail) => detail.hidden),
        buttons: panel.querySelectorAll("button").length,
        failedRed: getComputedStyle(rows[2]!.lastElementChild!).color !== getComputedStyle(rows[3]!.lastElementChild!).color,
      };
      rows[0]!.click();
      rows[1]!.click();
      await frame();
      seen.expanded = rows.map((row) => row.getAttribute("aria-expanded"));
      seen.opened = details()
        .filter((detail) => !detail.hidden)
        .map((detail) => detail.textContent);
      rows[0]!.click();
      await frame();
      seen.closedAgain = details()[0]!.hidden;
      const result = document.createElement("pre");
      result.id = "mcp-server-panel-result";
      result.textContent = JSON.stringify(seen);
      document.body.append(result);
    })();
  }, []);
  return (
    <div className="w-80 p-4">
      <McpServerPanel servers={SERVERS} />
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
