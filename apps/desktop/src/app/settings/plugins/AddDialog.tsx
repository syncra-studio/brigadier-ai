import { useId, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { Button, ErrorLine, Input, SettingsSelect } from "@/components/controls";
import { ChevronDown, ChevronRight, Folder, Plus, Spinner, X } from "@/components/icons";
import { PLAIN_FIELD } from "@/lib/plainTyping";
import type { PluginAgentResult, PluginDraft, PluginsAdapter } from "./adapter";
import { failures, notes, PluginDialog, useWatch } from "./dialogs";
import { KIND_LABEL, shownDetail } from "./model";
import { PluginMark, RadioRow } from "./parts";
import type { PluginElsewhereView, PluginKind, PluginsView } from "./types";
import { FileDialog, settingsText } from "./FileDialog";

export const ADD_HINT = "Works with an mcp add line from any agent, a server URL, mcpServers JSON, a GitHub link, a skills.sh name or a skill folder.";

/** One env or header value in Advanced: `pasted` came with the paste (its value stays in Brigadier unless typed over). */
type ValueRow = { id: number; name: string; value: string; pasted: boolean };

/** `from`: another app's server being imported, not a paste. */
export const SKILL_STARTER = "---\nname: \ndescription: \n---\n\n";

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
  const [kind, setKind] = useState<PluginKind>("server");
  const [texts, setTexts] = useState({ skill: SKILL_STARTER, server: "" });
  const text = texts[kind];
  const [sourceFolder, setSourceFolder] = useState<string | null>(null);
  const [importing, setImporting] = useState<PluginElsewhereView | null>(null);
  const [showImports, setShowImports] = useState(false);
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
      const skillText = kind === "skill" && text.trimStart().startsWith("---");
      const { drafts, needsName: unnamed } = await adapter.checkAdd(text, skillText ? null : name.trim() || null);
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
      if (importing && (drafts.length !== 1 || drafts[0]!.kind !== "server")) throw new Error("Import one MCP server at a time.");
      setStep({ type: "review", drafts, ...(importing ? { from: importing } : {}) });
    });
    const choose = () => action.run(async () => {
      const folder = await adapter.pickFolder();
      if (!folder) return;
      const { drafts } = await adapter.checkAdd(folder, null);
      const draft = drafts[0];
      if (drafts.length !== 1 || draft?.kind !== "skill" || draft.contents === undefined) throw new Error("Choose a folder containing one SKILL.md.");
      const draftName = draft.name || folder.split(/[\\/]/).filter(Boolean).at(-1) || "skill";
      setKind("skill");
      setTexts((current) => ({ ...current, skill: draft.contents!.trimStart().startsWith("---") ? draft.contents!
        : `---\nname: ${draftName}\ndescription: \n---\n\n${draft.contents}` }));
      setName(draftName);
      setSourceFolder(folder);
      setImporting(null);
      setNeedsName(false);
      setShowImports(false);
    });
    const loadImport = (entry: PluginElsewhereView) => action.run(async () => {
      const found = await adapter.details(entry.key);
      if (!found.server) throw new Error("This item is not an MCP server.");
      setKind("server");
      setTexts((current) => ({ ...current, server: settingsText(found.server!) }));
      setName(entry.name);
      setImporting(entry);
      setSourceFolder(null);
      setNeedsName(false);
      setShowImports(false);
    });
    return (
      <FileDialog
        item={{ name: kind === "skill" ? "New skill" : "New MCP server", kind, files: [] }}
        onClose={onClose}
        create={{
          value: text,
          onChange: (value) => { setTexts((current) => ({ ...current, [kind]: value })); setNeedsName(false); },
          onSave: check,
          error: action.error,
          busy: action.busy,
          disabled: needsName && !name.trim(),
          controls: <div className="border-border flex shrink-0 flex-wrap items-center gap-2 border-b px-4 py-2">
            <div role="tablist" aria-label="Plugin kind" className="flex items-center gap-1">
              {(["skill", "server"] as const).map((choice) => <Button key={choice} role="tab" aria-selected={kind === choice} variant={kind === choice ? "soft" : "ghost"} size="sm" onClick={() => {
                setKind(choice); setTexts((current) => ({ ...current, [choice]: choice === "skill" ? SKILL_STARTER : "" }));
                setImporting(null); setSourceFolder(null); setNeedsName(false); setName(""); setShowImports(false);
              }}>{KIND_LABEL[choice]}</Button>)}
            </div>
            <span className="flex-1" />
            <Button variant="ghost" size="sm" disabled={action.busy} onClick={choose}>Choose folder…</Button>
            {view.elsewhere.length > 0 && <Button variant="ghost" size="sm" onClick={() => setShowImports(!showImports)}>Import from…</Button>}
          </div>,
          extra: needsName || showImports ? <>
            {needsName && <div data-slot="paste-name" className="flex items-center gap-3">
              <label htmlFor={`${id}-paste-name`} className="text-foreground/65 shrink-0 text-label">Name</label>
              <Input id={`${id}-paste-name`} aria-label="Name" value={name} autoFocus {...PLAIN_FIELD} onChange={(event) => setName(event.target.value)} className="h-8 font-mono text-xs" />
            </div>}
            {showImports && <ImportList entries={view.elsewhere} onImport={loadImport} />}
          </> : null,
        }}
      />
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
      <PluginDialog slot="adding" onClose={onClose} title={`Adding ${shownName}`} description="Saved in Brigadier's library for your next sessions.">
        <div className="flex flex-col gap-3">
          {step.results === null ? (
            <div data-slot="adding-wait" className="text-foreground/65 flex items-center gap-3 text-label">
              <Spinner aria-label="Adding" className="size-icon-sm animate-spin" />
              Adding it to your library…
            </div>
          ) : (
            <p className="text-foreground/65 text-label">Added to your library.</p>
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
      ? adapter.importPlugin(from.key, given && given !== from.name ? given : null, projectIds, text)
      : adapter.add(text, given && (needsName || given !== single?.name) ? given : null, projectIds, typedValues, sourceFolder ?? undefined);
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
      description={from ? `An MCP server from ${from.foundIn}. Brigadier sessions get it with its settings and values; ${from.foundIn}'s own stays as it is.` : single ? about(single) : "Each one goes to every agent."}
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
        <p className="text-foreground/50 text-xs">Saved in Brigadier’s private library entry.{rows.some((row) => row.pasted) ? " Leave one empty to keep what you pasted." : ""}</p>
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
