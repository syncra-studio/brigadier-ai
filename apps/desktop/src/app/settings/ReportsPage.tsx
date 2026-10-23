import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

import { SettingsPage, SettingsSection } from "@/app/settings/parts";
import { Button } from "@/components/controls";
import { SettingsCard, SettingsRow } from "@/components/shared";

export const REPORTS_ROWS = [{ label: "Continuity nightly", description: "Recall, decisions, tokens, compactions and cold returns" }];

type Arm = {
  block: string;
  profile: string;
  arm: string;
  status: string;
  errors?: string[];
  recall?: number | null;
  compactions?: number | null;
  seams?: number;
  contexts_before_checks?: number | null;
  applied?: number | null;
  planted?: number;
  contradictions?: number | null;
  unmarked_additions?: number | null;
  open_items?: number | null;
  task_passed?: boolean | null;
  price_weighted_ratio?: number;
  cold_return_cost_usd?: number | null;
  cold_return_observations?: number;
  usage?: { input: number | null; cached_input: number | null; cache_write: number | null; output: number | null; cost_usd: number | null };
};
export type ContinuityReport = {
  schema_version: number;
  created_at: string;
  source: string;
  run_url: string;
  baseline: boolean;
  providers: { provider: string; status: string; reason: string | null; arms: Arm[] }[];
  weekly: { status: string; reason?: string; sampled_lineages?: number; sessions?: { session_id: string; provider: string; compactions: number | null }[] };
  regressions: string[];
};

const number = (n: number | null | undefined) => n == null ? "Unavailable" : n.toLocaleString();
const dollars = (n: number | null | undefined) => n == null ? "Unavailable" : `$${n.toFixed(4)}`;

export function ReportsPage() {
  const [report, setReport] = useState<ContinuityReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const read = async () => {
    try {
      setReport(await invoke<ContinuityReport>("get_continuity_report"));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  };
  const refresh = () => {
    setLoading(true);
    setError(null);
    void read();
  };
  useEffect(() => {
    let current = true;
    void invoke<ContinuityReport>("get_continuity_report").then(
      (value) => { if (current) { setReport(value); setLoading(false); } },
      (cause: unknown) => { if (current) { setError(String(cause)); setLoading(false); } },
    );
    return () => { current = false; };
  }, []);
  return (
    <SettingsPage
      title="Reports"
      description="Nightly continuity measurements. Regressions open GitHub issues; measurements never hold work."
      actions={<Button variant="soft" size="sm" disabled={loading} onClick={refresh}>Refresh</Button>}
    >
      <SettingsSection title="Continuity nightly">
        <SettingsCard>
          <SettingsRow label={loading ? "Reading latest artifact…" : "Latest artifact"} error={error}
            description={report ? `${new Date(report.created_at).toLocaleString()} · ${report.source.slice(0, 10)}${report.baseline ? " · First baseline" : ""}` : "Run the Continuity battery workflow on main to create the first report."} />
        </SettingsCard>
      </SettingsSection>
      {report && <ContinuityNumbers report={report} />}
    </SettingsPage>
  );
}

/** Uses the same sections, cards and rows as every Settings page. */
export function ContinuityNumbers({ report }: { report: ContinuityReport }) {
  return <>
    {report.providers.map((provider) => <SettingsSection key={provider.provider} title={provider.provider === "claude" ? "Claude" : "Codex"}>
      <SettingsCard>
        <SettingsRow label={provider.status} description={provider.reason} />
        {provider.arms.map((arm) => <ArmNumbers key={`${arm.block}-${arm.profile}-${arm.arm}`} arm={arm} />)}
      </SettingsCard>
    </SettingsSection>)}
    <SettingsSection title="Weekly real lineages">
      <SettingsCard>
        <SettingsRow label={report.weekly.status} description={report.weekly.reason ?? `${number(report.weekly.sampled_lineages)} lineages sampled`} />
        {report.weekly.sessions?.map((session) => <SettingsRow key={`${session.provider}-${session.session_id}`} label={session.session_id}
          description={`${session.provider} · ${number(session.compactions)} compactions`} />)}
      </SettingsCard>
    </SettingsSection>
    <SettingsSection title="Regressions">
      <SettingsCard>
        {report.regressions.length ? report.regressions.map((line) => <SettingsRow key={line} label={line} />)
          : <SettingsRow label="None measured" description="Skipped and incomplete runs are not passes." />}
        <SettingsRow label="Run and artifact" description={report.run_url} />
      </SettingsCard>
    </SettingsSection>
  </>;
}

function ArmNumbers({ arm }: { arm: Arm }) {
  const contexts = arm.contexts_before_checks ?? (arm.arm === "plain" ? arm.compactions : arm.seams);
  return <SettingsRow label={`${arm.block === "decisions" ? "Decisions survive" : "Recall"} · ${arm.profile} · ${arm.arm}`} description={<>
    <div>{arm.status}{arm.errors?.length ? ` · ${arm.errors.join("; ")}` : ""}</div>
    <div>Compactions: {number(arm.compactions)} · Recall: {arm.recall == null ? "Unavailable" : `${Math.round(arm.recall * 100)}%`} · Task: {arm.task_passed == null ? "Unavailable" : arm.task_passed ? "Passed" : "Failed"}</div>
    {arm.block === "decisions" && <>
      <div>Applied: {number(arm.applied)} of {number(arm.planted)} planted · Brief contradictions: {number(arm.contradictions)} · Unmarked additions: {number(arm.unmarked_additions)} · Open items raised: {number(arm.open_items)} of 2</div>
      <div>{number(contexts)} {arm.arm === "plain" ? "native compactions" : "seams"} before checks{(contexts ?? 0) < 3 ? " · Three required to qualify this measurement" : ""}</div>
    </>}
    <div>Tokens · Input: {number(arm.usage?.input)} · Cached: {number(arm.usage?.cached_input)} · Cache writes: {number(arm.usage?.cache_write)} · Output: {number(arm.usage?.output)}</div>
    <div>Reported cost: {dollars(arm.usage?.cost_usd)} · Price-weighted ratio: {arm.price_weighted_ratio == null ? "Unavailable" : `${arm.price_weighted_ratio.toFixed(3)}× plain CLI`}</div>
    <div>Cold return: {dollars(arm.cold_return_cost_usd)} · {number(arm.cold_return_observations)} observed · Report only</div>
  </>} />;
}
