import { disclosureRow } from "@/components/conversation";
import type { OvernightRun } from "@/ipc/generated";

/** A proposed run's rules and the sources it reads, folded under "Rules and sources". */
export function RulesAndSources({ rules, sources }: { rules: OvernightRun["rules"]; sources: OvernightRun["sources"] }) {
  return (
    <details className="text-muted-foreground">
      <summary className={disclosureRow}>Rules and sources</summary>
      {rules && <p className="whitespace-pre-wrap">{rules}</p>}
      {sources.map((source) => <p key={source.path}>{source.path}{source.sections ? ` · ${source.sections}` : ""}</p>)}
    </details>
  );
}
