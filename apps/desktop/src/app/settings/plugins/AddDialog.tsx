import { useId, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { Badge, Button, ErrorLine, Input, SettingsSelect } from "@/components/controls";
import { ChevronDown, ChevronRight, Folder, Plus, Spinner, X } from "@/components/icons";
import { PLAIN_FIELD } from "@/lib/plainTyping";
import type { PluginAgentResult, PluginDraft, PluginsAdapter } from "./adapter";
import { agentOf, failures, notes, PluginDialog, useWatch } from "./dialogs";
import { KIND_LABEL, shownDetail } from "./model";
import { AgentRow, PluginMark, RadioRow } from "./parts";
import type { PluginElsewhereView, PluginsView } from "./types";

export const ADD_HINT = "Works with an mcp add line from any agent, a server URL, mcpServers JSON, a GitHub link, a skills.sh name or a skill folder.";

/** One env or header value in Advanced: `pasted` came with the paste (its value stays in Brigadier unless typed over). */
type ValueRow = { id: number; name: string; value: string; pasted: boolean };

/** `from`: another app's server being imported, not a paste. */
type Step =
  | { type: "paste" }
  | { type: "review"; drafts: PluginDraft[]; from?: PluginElsewhereView }
  | { type: "adding"; drafts: PluginDraft[]; from?: PluginElsewhereView; results: PluginAgentResult[] | null };

/** G1–G4: paste, check where it goes, then every agent adds it. */
export function AddDialog({ view, adapter, refresh, onClose }: {
  view: PluginsView;
  adapter: PluginsAdapter;
  refresh: () => void;
  onClose: () => void;
}) {
  const id = useId();
  const [step, setStep] = useState<Step>({ type: "paste" });
  const [text, setText] = useState("");
  const [where, setWhere] = useState<"every" | "one">("every");
  const [project, setProject] = useState(view.projects[0]?.id ?? "");
  const [advanced, setAdvanced] = useState(false);
  const [name, setName] = useState("");
  /** What was pasted has no name of its own (a bare server, an address): the user gives one. */
  const [needsName, setNeedsName] = useState(false);
  const [values, setValues] = useState<ValueRow[]>([]);
  const action = useAction();
  const adding = step.type === "adding" && step.results === null;
  useWatch(refresh, adding);

  if (step.type === "paste") {
    const check = () => action.run(async () => {
      const { drafts, needsName: unnamed } = await adapter.checkAdd(text, needsName ? name.trim() : null);
      if (unnamed) {
        setNeedsName(true);
        return;
      }
      const problem = drafts.find((draft) => draft.error)?.error;
      if (problem) throw new Error(problem);
      if (!drafts.length) throw new Error("That doesn't look like an MCP server or a skill.");
      setName(drafts.length === 1 ? drafts[0]!.name : "");
      const server = drafts.length === 1 && drafts[0]!.kind === "server" ? drafts[0]! : null;
      setValues((server?.secrets ?? []).map((secret, index) => ({ id: index, name: secret, value: "", pasted: true })));
      setStep({ type: "review", drafts });
    });
    const choose = () => action.run(async () => {
      const folder = await adapter.pickFolder();
      if (folder) setText(folder);
    });
    return (
      <PluginDialog slot="add" onClose={onClose} title="Add a skill or MCP server" description="Paste what you have. Brigadier works out the rest and adds it to every agent.">
        <div className="flex flex-col gap-2">
          <textarea
            id={`${id}-paste`}
            aria-label="What to add"
            value={text}
            rows={6}
            autoFocus
            {...PLAIN_FIELD}
            onChange={(event) => {
              setText(event.target.value);
              setNeedsName(false);
            }}
            className="border-input bg-popover rounded-control placeholder:text-muted-foreground focus-visible:ring-ring/50 min-h-28 w-full resize-y border px-3 py-2 font-mono text-xs outline-none focus-visible:ring-2"
          />
          <p className="text-foreground/50 text-xs">{ADD_HINT}</p>
          {needsName && (
            <div data-slot="paste-name" className="flex flex-col gap-2 pt-2">
              <p className="text-foreground/65 text-xs">What you pasted has no name. Give this server one.</p>
              <div className="flex items-center gap-3">
                <label htmlFor={`${id}-paste-name`} className="text-foreground/65 w-20 shrink-0 text-label">Name</label>
                <Input id={`${id}-paste-name`} aria-label="Name" value={name} autoFocus {...PLAIN_FIELD} onChange={(event) => setName(event.target.value)} className="h-8 font-mono text-xs" />
              </div>
            </div>
          )}
          <ErrorLine error={action.error} />
        </div>
        {view.elsewhere.length > 0 && (
          <ImportList
            entries={view.elsewhere}
            onImport={(entry) => {
              setName(entry.name);
              setValues([]);
              setStep({ type: "review", drafts: [importDraft(entry)], from: entry });
            }}
          />
        )}
        <div className="flex items-center gap-2">
          <Button variant="ghost" onClick={choose}>Choose folder…</Button>
          <span className="flex-1" />
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button disabled={!text.trim() || (needsName && !name.trim()) || action.busy} onClick={check}>{action.busy ? "Checking…" : "Continue"}</Button>
        </div>
      </PluginDialog>
    );
  }

  const { drafts, from } = step;
  const single = drafts.length === 1 ? drafts[0]! : null;
  const shownName = single ? name.trim() || single.name : `${drafts.length} items`;
  const inProject = where === "one" && project ? view.projects.find((entry) => entry.id === project) ?? null : null;
  /** The one server's values are its headers (at an address) or its environment (a command). */
  // An imported server brings its own values from the app it's in.
  const server = single?.kind === "server" && !from ? single : null;
  const valuesLabel = server?.remote ? "Headers" : "Environment";

  if (step.type === "adding") {
    const done = step.results !== null;
    const problem = step.results && failures(step.results);
    // Added, with something to know: it didn't start, say.
    const note = step.results && !problem ? notes(step.results) : null;
    return (
      <PluginDialog slot="adding" onClose={onClose} title={`Adding ${shownName}`} description="Each agent adds it its own way, so it works in a terminal too.">
        <div className="flex flex-col gap-3">
          {step.results === null ? (
            <div data-slot="adding-wait" className="text-foreground/65 flex items-center gap-3 text-label">
              <Spinner aria-label="Adding" className="size-icon-sm animate-spin" />
              Adding it to every agent…
            </div>
          ) : (
            step.results.map((result) => <AddingRow key={result.agent} result={result} />)
          )}
        </div>
        {problem && <ErrorLine error={problem} />}
        {note && <p data-slot="add-note" className="text-warning text-xs whitespace-pre-wrap">{note}</p>}
        <div className="flex justify-end gap-2">
          {done ? <Button onClick={onClose}>Done</Button> : <Button variant="ghost" onClick={onClose}>Hide</Button>}
        </div>
      </PluginDialog>
    );
  }

  const add = () => {
    setStep({ type: "adding", drafts, ...(from ? { from } : {}), results: null });
    // A paste with no name of its own always needs the one given.
    const given = single ? name.trim() || (needsName ? single.name : "") : "";
    // A pasted value left empty keeps the one pasted; a row without a name or value is left out.
    const typed = server ? values.filter((row) => row.name.trim() && row.value) : [];
    const typedValues = typed.length ? Object.fromEntries(typed.map((row) => [row.name.trim(), row.value])) : null;
    const projectIds = inProject ? [inProject.id] : null;
    const request = from
      ? adapter.importPlugin(from.key, given && given !== from.name ? given : null, projectIds)
      : adapter.add(text, given && (needsName || given !== single?.name) ? given : null, projectIds, typedValues);
    request.then(
      (results) => {
        setStep({ type: "adding", drafts, ...(from ? { from } : {}), results });
        refresh();
      },
      (cause: unknown) => {
        setStep({ type: "review", drafts, ...(from ? { from } : {}) });
        action.run(() => Promise.reject(cause instanceof Error ? cause : new Error(String(cause))));
      },
    );
  };
  return (
    <PluginDialog
      slot="add-review"
      onClose={onClose}
      title={`${from ? "Import" : "Add"} ${shownName}${inProject ? ` to ${inProject.name}` : ""}`}
      description={from ? `An MCP server from ${from.foundIn}. Every agent gets it with its settings and values; ${from.foundIn}'s own stays as it is.` : single ? about(single) : "Each one goes to every agent."}
    >
      <div className="flex flex-col gap-5">
        <div className="flex flex-col gap-2">
          {drafts.map((draft) => (
            <div key={draft.name} data-slot="draft" className="bg-foreground/5 rounded-control flex items-center gap-3 px-4 py-3">
              <PluginMark item={draft} />
              <span className="flex min-w-0 flex-1 flex-col gap-1">
                <span className="flex items-baseline gap-2">
                  <span className="truncate text-label font-medium">{single ? shownName : draft.name}</span>
                  <span className="text-foreground/50 shrink-0 text-xs">{KIND_LABEL[draft.kind]}</span>
                </span>
                <span className="text-foreground/65 truncate font-mono text-xs">{draft.detail}</span>
              </span>
            </div>
          ))}
        </div>
        <div role="radiogroup" aria-label="Where" className="flex flex-col gap-4">
          <RadioRow checked={where === "every"} title="All projects" line="On in every project, and in new ones you add." onSelect={() => setWhere("every")} />
          {view.projects.length > 0 && (
            <RadioRow
              checked={where === "one"}
              title="Only in one project"
              line="Sessions in other projects won't see it."
              onSelect={() => setWhere("one")}
              end={
                <SettingsSelect
                  label="Project"
                  icon={<Folder aria-hidden className="text-foreground/65 size-icon-sm" />}
                  value={project}
                  options={view.projects.map((entry) => ({ value: entry.id, label: entry.name }))}
                  onChange={(value) => {
                    setProject(value);
                    setWhere("one");
                  }}
                  className="h-8 w-44"
                />
              }
            />
          )}
        </div>
        {(single || server) && <div className="flex flex-col gap-3">
          <button type="button" aria-expanded={advanced} onClick={() => setAdvanced(!advanced)} className="flex items-center gap-2 text-start">
            {advanced ? <ChevronDown aria-hidden className="text-foreground/65 size-icon-sm" /> : <ChevronRight aria-hidden className="text-foreground/65 size-icon-sm" />}
            <span className="text-label">Advanced</span>
            <span className="text-foreground/50 text-xs">{[single && "name", server && valuesLabel.toLowerCase()].filter(Boolean).join(", ")}</span>
          </button>
          {advanced && (
            <div className="flex flex-col gap-3 ps-6">
              {single && (
                <div className="flex items-center gap-3">
                  <label htmlFor={`${id}-name`} className="text-foreground/65 w-20 shrink-0 text-label">Name</label>
                  <Input id={`${id}-name`} value={name} {...PLAIN_FIELD} onChange={(event) => setName(event.target.value)} className="h-8 font-mono text-xs" />
                </div>
              )}
              {server && <ValueRows label={valuesLabel} rows={values} onChange={setValues} />}
            </div>
          )}
        </div>}
        <ErrorLine error={action.error} />
      </div>
      <div className="flex items-center gap-2">
        <Button variant="ghost" onClick={() => setStep({ type: "paste" })}>Back</Button>
        <span className="flex-1" />
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button onClick={add}>{from ? "Import" : "Add"}</Button>
      </div>
    </PluginDialog>
  );
}

/** The one server's headers or environment: name = value rows, each value hidden as it is typed. */
function ValueRows({ label, rows, onChange }: { label: string; rows: ValueRow[]; onChange: (update: (rows: ValueRow[]) => ValueRow[]) => void }) {
  const one = label === "Headers" ? "header" : "value";
  const set = (row: ValueRow, change: Partial<ValueRow>) => onChange((now) => now.map((entry) => (entry.id === row.id ? { ...entry, ...change } : entry)));
  return (
    <div className="flex items-start gap-3">
      <span className="text-foreground/65 w-20 shrink-0 pt-1.5 text-label">{label}</span>
      <div role="group" aria-label={label} className="flex min-w-0 flex-1 flex-col gap-2">
        {rows.map((row) => (
          <div key={row.id} data-slot="value-row" className="flex items-center gap-2">
            <Input
              aria-label={`${label} name`}
              value={row.name}
              readOnly={row.pasted}
              placeholder="Name"
              {...PLAIN_FIELD}
              onChange={(event) => set(row, { name: event.target.value })}
              className="h-8 w-36 shrink-0 font-mono text-xs"
            />
            <span aria-hidden className="text-foreground/50 text-xs">=</span>
            <Input
              type="password"
              aria-label={`${row.name.trim() || "New"} value`}
              value={row.value}
              placeholder={row.pasted ? "As pasted" : "Value"}
              {...PLAIN_FIELD}
              onChange={(event) => set(row, { value: event.target.value })}
              className="h-8 font-mono text-xs"
            />
            {!row.pasted && (
              <Button variant="ghost" size="icon-sm" className="text-muted-foreground" aria-label={`Remove ${row.name.trim() || one}`} onClick={() => onChange((now) => now.filter((entry) => entry.id !== row.id))}>
                <X />
              </Button>
            )}
          </div>
        ))}
        <Button
          variant="link-blue"
          size="inline"
          className="self-start"
          onClick={() => onChange((now) => [...now, { id: Math.max(-1, ...now.map((row) => row.id)) + 1, name: "", value: "", pasted: false }])}
        >
          <Plus />
          Add a {one}
        </Button>
        <p className="text-foreground/50 text-xs">Kept in your Keychain, never in a file.{rows.some((row) => row.pasted) ? " Leave one empty to keep what you pasted." : ""}</p>
      </div>
    </div>
  );
}

/** "An MCP server from mcp.sentry.dev. It signs in with your browser." */
function about(draft: PluginDraft): string {
  const signs = draft.signsIn ? " It signs in with your browser." : "";
  if (draft.kind === "skill") return `A skill from ${shownDetail(draft.detail)}.`;
  if (!draft.remote) return `An MCP server your agents start on this computer.${signs}`;
  let host = draft.detail;
  try {
    host = new URL(draft.detail).host;
  } catch {
    // Not a URL after all: the address as it was written.
  }
  return `An MCP server from ${host}.${signs}`;
}

/** Another app's server as Add's review shows it. */
const importDraft = (entry: PluginElsewhereView): PluginDraft => ({
  name: entry.name,
  kind: "server",
  detail: entry.url ?? entry.detail,
  remote: entry.url !== null,
  signsIn: false,
  secrets: [],
  error: null,
});

/** Other apps' servers, by the app each is in: none of the agents uses them until imported. */
function ImportList({ entries, onImport }: { entries: PluginElsewhereView[]; onImport: (entry: PluginElsewhereView) => void }) {
  const apps = [...new Set(entries.map((entry) => entry.foundIn))];
  return (
    <div data-slot="import" className="flex min-w-0 flex-col gap-4">
      {apps.map((app) => (
        <section key={app} aria-label={`Import from ${app}`} className="flex flex-col gap-2">
          <h3 className="text-foreground/65 text-label">Import from {app}</h3>
          {entries.filter((entry) => entry.foundIn === app).map((entry) => (
            <div key={entry.key} data-slot="import-row" className="bg-foreground/5 rounded-control flex items-center gap-3 px-3 py-2">
              <PluginMark item={{ kind: "server", name: entry.name, detail: entry.detail }} />
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="truncate text-label font-medium">{entry.name}</span>
                <span className="text-foreground/65 truncate font-mono text-xs">{entry.url ?? entry.detail}</span>
              </span>
              <Button variant="soft" size="sm" aria-label={`Import ${entry.name}`} onClick={() => onImport(entry)}>Import</Button>
            </div>
          ))}
        </section>
      ))}
    </div>
  );
}

/** How one agent took the add. */
function AddingRow({ result }: { result: PluginAgentResult }) {
  const agent = agentOf(result.agent);
  return result.ok
    ? <AgentRow agent={agent} line="Added" end={<Badge variant="success">Done</Badge>} />
    : <AgentRow agent={agent} tone="error" line={result.message ?? "Couldn't add it."} end={<Badge variant="destructive">Failed</Badge>} />;
}
