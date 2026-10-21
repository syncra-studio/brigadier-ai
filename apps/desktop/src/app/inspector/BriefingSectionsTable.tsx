import { formatTokens } from "@/app/inspector/orchestratorLog";
import type { RebirthRecord } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** A rebirth's briefing, section by section: its size and what was cut to fit. */
export function BriefingSectionsTable({ sections }: { sections: RebirthRecord["sections"] }) {
  return (
    <table className="w-full">
      <thead className="text-muted-foreground">
        <tr className="border-b">
          <th className="py-1 text-start font-medium">Briefing section</th>
          <th className="px-2 py-1 text-end font-medium">Tokens</th>
          <th className="px-2 py-1 text-end font-medium">Items</th>
          <th className="py-1 text-start font-medium">Cut to fit</th>
        </tr>
      </thead>
      <tbody>
        {sections.map((section) => (
          <tr key={section.name} className="border-b align-top last:border-b-0">
            <td className="py-1 font-mono">{section.name}</td>
            <td className="px-2 py-1 text-end tabular-nums">{formatTokens(section.tokens)}</td>
            <td className="px-2 py-1 text-end tabular-nums">{section.items}</td>
            <td className={cn("py-1", section.truncated ? "text-warning" : "text-muted-foreground")}>
              {section.truncated ? (section.note ?? "yes") : "no"}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
