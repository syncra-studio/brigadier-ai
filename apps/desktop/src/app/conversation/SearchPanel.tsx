import { ChevronDown, ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, memo, type ReactNode, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  CollapseAllGlyph, MatchCaseGlyph, MoreGlyph, RegexGlyph, ReplaceAllGlyph, WholeWordGlyph,
} from "@/components/glyphs/codicons";
import { FileIcon } from "@/components/glyphs/fileIcons";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import type { ReplaceOutcome, SearchFile, SearchLine, SearchQuery } from "@/ipc/generated";
import { useApp } from "@/state/store";
import { onWorkspaceChanged, useCheckoutKey, type WorkspaceTarget } from "@/state/workspaceCache";
import {
  clearSearch, EMPTY_FORM, replaceResults, runSearch, sameQuery, searchId, searchQuery, type SearchForm,
  setSearchForm, stopSearch, useWorkspaceSearch,
} from "@/state/workspaceSearch";

/**
 * The right sidebar's Search (⇧⌘F): text or a pattern across the checkout, with match case,
 * whole word and regular expression, files to include and exclude, and Replace all. Results
 * come grouped by file as the daemon finds them; a line opens the file there. What was last
 * found shows at once when the view opens, and the search runs again when the checkout changes.
 */

/** How long typing pauses before the search runs. */
const TYPING_MS = 250;
/** How long the checkout settles after a change before the search runs again. */
const CHANGED_MS = 300;

/** Opens `path` at `line`: in the preview tab, or with `keep` in a tab of its own. */
type OnOpen = (path: string, line: number, keep: boolean) => void;

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

function dirName(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

function plural(count: number, one: string, many = `${one}s`): string {
  return `${count.toLocaleString()} ${count === 1 ? one : many}`;
}

export function SearchPanel({ target, onOpen, readOnly }: {
  target: WorkspaceTarget;
  onOpen: OnOpen;
  /** Replace is left out: the session will work in a copy of its own, not made yet. */
  readOnly: boolean;
}) {
  const id = searchId(target);
  const checkout = useCheckoutKey(target);
  const form = useWorkspaceSearch((s) => s.entries[id]?.form ?? EMPTY_FORM);
  const stored = useWorkspaceSearch((s) => s.entries[id]?.results ?? null);
  // Results for another checkout of the same session (its copy made since) don't show.
  const results = stored?.checkout === checkout ? stored : null;
  const query = useMemo(() => searchQuery(form), [form]);
  const set = (change: Partial<SearchForm>) => setSearchForm(target, change);

  // A new search runs once typing pauses. The one last found is asked again at once as the view
  // opens, its results shown meanwhile and refreshed in place.
  const latest = useRef({ query, results });
  useLayoutEffect(() => {
    latest.current = { query, results };
  });
  const asked = query ? JSON.stringify(query) : "";
  useEffect(() => {
    const found = latest.current.results;
    const next = asked ? JSON.parse(asked) as SearchQuery : null;
    if (!next) {
      if (found) clearSearch(target);
      return;
    }
    const again = found?.checkout === checkout && sameQuery(found.query, next);
    const timer = setTimeout(() => runSearch(target, next), again ? 0 : TYPING_MS);
    return () => clearTimeout(timer);
  }, [target, asked, checkout]);
  // Closed, the view stops its search; opened again, it asks again.
  useEffect(() => () => stopSearch(target), [target]);
  // A new connection doesn't carry on the old one's search, nor hear what changed meanwhile:
  // after reconnecting, the query is found again, the old results kept meanwhile.
  const connected = useApp((s) => s.connection.status === "connected");
  const wasConnected = useRef(connected);
  useEffect(() => {
    const reconnected = connected && !wasConnected.current;
    wasConnected.current = connected;
    if (reconnected && latest.current.query) runSearch(target, latest.current.query);
  }, [target, connected]);
  // Edits to the checkout (an agent's, a replace's) find again, the old results kept meanwhile.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const unsubscribe = onWorkspaceChanged(target, () => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        if (latest.current.query) runSearch(target, latest.current.query);
      }, CHANGED_MS);
    });
    return () => {
      clearTimeout(timer);
      unsubscribe();
    };
  }, [target]);

  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());
  const [confirming, setConfirming] = useState<{ file?: string } | null>(null);
  const [replacing, setReplacing] = useState(false);
  const [outcome, setOutcome] = useState<ReplaceOutcome | { error: string } | null>(null);
  const matches = results?.files.reduce((sum, file) => sum + file.matches, 0) ?? 0;
  const searching = !!results && !results.done && !results.error;
  const replace = (file?: string) => {
    if (!results || !canReplace) return;
    setConfirming(null);
    setReplacing(true);
    replaceResults(target, results, form.replace, file)
      .then(setOutcome)
      .catch((error: unknown) => setOutcome({ error: error instanceof Error ? error.message : String(error) }))
      .finally(() => setReplacing(false));
  };
  // Only what a finished search found, for the query the fields hold.
  const canReplace = !readOnly && !!results?.done && results.files.length > 0 && !replacing && sameQuery(results.query, query);
  const confirmedFiles = results?.files.filter((file) => !confirming?.file || file.path === confirming.file) ?? [];
  const confirmedMatches = confirmedFiles.reduce((sum, file) => sum + file.matches, 0);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-border flex shrink-0 flex-col gap-1.5 border-b p-2">
        <Field label="Search" value={form.text} onChange={(text) => { set({ text }); setOutcome(null); }}
          onEnter={() => query && runSearch(target, query)}>
          <Toggle label="Match case" pressed={form.matchCase} onChange={(matchCase) => set({ matchCase })}><MatchCaseGlyph /></Toggle>
          <Toggle label="Whole word" pressed={form.wholeWord} onChange={(wholeWord) => set({ wholeWord })}><WholeWordGlyph /></Toggle>
          <Toggle label="Regular expression" pressed={form.regex} onChange={(regex) => set({ regex })}><RegexGlyph /></Toggle>
        </Field>
        {readOnly ? (
          <p data-slot="own-copy-note" className="text-muted-foreground px-1 text-xs">
            This session will work in its own copy: Replace works there once it starts
          </p>
        ) : (
          <Field label="Replace" value={form.replace} onChange={(value) => set({ replace: value })}
            onEnter={() => canReplace && setConfirming({})}>
            <TooltipIconButton tooltip="Replace all" size="icon-xs" disabled={!canReplace} onClick={() => setConfirming({})}>
              <ReplaceAllGlyph />
            </TooltipIconButton>
          </Field>
        )}
        <div className="flex items-center justify-end">
          <TooltipIconButton tooltip={form.details ? "Hide files to include and exclude" : "Files to include and exclude"}
            size="icon-xs" aria-expanded={form.details} onClick={() => set({ details: !form.details })}>
            <MoreGlyph />
          </TooltipIconButton>
        </div>
        {form.details && (
          <>
            <Glob label="Files to include" placeholder="e.g. src/**, *.ts" value={form.include} onChange={(include) => set({ include })} />
            <Glob label="Files to exclude" placeholder="e.g. *.test.ts, docs" value={form.exclude} onChange={(exclude) => set({ exclude })} />
          </>
        )}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-2" aria-busy={searching}>
        {results?.error ? (
          <p role="alert" className="text-destructive p-2 text-sm">{results.error}</p>
        ) : results && (
          <>
            <div className="flex h-control-sm items-center gap-1 px-1">
              <p data-slot="search-summary" className="text-muted-foreground min-w-0 flex-1 truncate text-xs">
                {results.files.length === 0
                  ? results.done ? "No results" : "Searching…"
                  : `${plural(matches, "result")} in ${plural(results.files.length, "file")}${searching ? "…" : ""}`}
              </p>
              {results.files.length > 0 && (
                <TooltipIconButton tooltip="Collapse all" size="icon-xs"
                  onClick={() => setCollapsed(new Set(results.files.map((file) => file.path)))}>
                  <CollapseAllGlyph />
                </TooltipIconButton>
              )}
            </div>
            {outcome && <ReplaceNote outcome={outcome} />}
            {results.done?.capped && (
              <p className="text-muted-foreground px-1 pb-1 text-xs">
                Only the first results are shown. Narrow the search to see the rest.
              </p>
            )}
            <ul aria-label="Search results">
              {results.files.map((file) => (
                <FileGroup key={file.path} file={file} open={!collapsed.has(file.path)}
                  onToggle={() => setCollapsed((current) => {
                    const next = new Set(current);
                    if (!next.delete(file.path)) next.add(file.path);
                    return next;
                  })}
                  onOpen={onOpen}
                  onReplace={canReplace ? () => setConfirming({ file: file.path }) : null} />
              ))}
            </ul>
          </>
        )}
      </div>
      <Dialog open={confirming !== null} onOpenChange={(open) => { if (!open) setConfirming(null); }}>
        <DialogContent>
          {results && (
            <>
              <DialogHeader>
                <DialogTitle>
                  Replace {plural(confirmedMatches, "match", "matches")} in {plural(confirmedFiles.length, "file")}?
                </DialogTitle>
                <DialogDescription>
                  {form.replace ? <>Each becomes “{form.replace}”.</> : "Each is deleted."} Files changed since the
                  search are left as they are.
                </DialogDescription>
              </DialogHeader>
              <DialogFooter>
                <Button variant="ghost" onClick={() => setConfirming(null)}>Cancel</Button>
                <Button disabled={!canReplace || confirmedFiles.length === 0} onClick={() => replace(confirming?.file)}>Replace</Button>
              </DialogFooter>
            </>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}

const Field: FC<{ label: string; value: string; onChange: (value: string) => void; onEnter: () => void; children: ReactNode }> = ({
  label, value, onChange, onEnter, children,
}) => (
  // Not a label: the buttons inside would take its text as their names.
  <div className="border-border rounded-control focus-within:border-ring flex h-control-md items-center gap-0.5 border ps-2 pe-0.5">
    <input value={value} placeholder={label} aria-label={label} spellCheck={false}
      onChange={(event) => onChange(event.target.value)}
      onKeyDown={(event) => {
        if (event.key === "Enter" && !event.nativeEvent.isComposing) {
          event.preventDefault();
          onEnter();
        }
      }}
      className="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-sm outline-none" />
    {children}
  </div>
);

const Toggle: FC<{ label: string; pressed: boolean; onChange: (pressed: boolean) => void; children: ReactNode }> = ({
  label, pressed, onChange, children,
}) => (
  <TooltipIconButton tooltip={label} size="icon-xs" aria-pressed={pressed} onClick={() => onChange(!pressed)}
    className="aria-pressed:bg-toolbar-pressed aria-pressed:text-foreground text-muted-foreground">
    {children}
  </TooltipIconButton>
);

const Glob: FC<{ label: string; placeholder: string; value: string; onChange: (value: string) => void }> = ({
  label, placeholder, value, onChange,
}) => (
  <label className="flex flex-col gap-0.5">
    <span className="text-muted-foreground px-1 text-xs">{label}</span>
    <input value={value} placeholder={placeholder} aria-label={label} spellCheck={false}
      onChange={(event) => onChange(event.target.value)}
      className="border-border rounded-control focus:border-ring placeholder:text-muted-foreground h-control-md border bg-transparent px-2 text-sm outline-none" />
  </label>
);

const ReplaceNote: FC<{ outcome: ReplaceOutcome | { error: string } }> = ({ outcome }) => {
  if ("error" in outcome) return <p role="alert" className="text-destructive px-1 pb-1 text-xs">{outcome.error}</p>;
  return (
    <div role="status" className="text-muted-foreground px-1 pb-1 text-xs">
      <p>Replaced {plural(outcome.matches, "match", "matches")} in {plural(outcome.files, "file")}.</p>
      {outcome.skipped.length > 0 && (
        <p className="text-warning">
          Left as {outcome.skipped.length === 1 ? "it was" : "they were"}: {outcome.skipped.map((skip) => `${baseName(skip.path)} (${skip.reason})`).join(", ")}
        </p>
      )}
    </div>
  );
};

const FileGroup = memo(function FileGroup({ file, open, onToggle, onOpen, onReplace }: {
  file: SearchFile;
  open: boolean;
  onToggle: () => void;
  onOpen: OnOpen;
  onReplace: (() => void) | null;
}) {
  return (
    <li>
      <div className="group/file hover:bg-muted rounded-control flex h-control-sm items-center">
        <button type="button" title={file.path} aria-expanded={open} onClick={onToggle}
          className="flex h-full min-w-0 flex-1 items-center gap-1.5 ps-1 text-start text-sm">
          {open ? <ChevronDown className="size-icon-xs shrink-0" /> : <ChevronRight className="size-icon-xs shrink-0" />}
          <FileIcon name={file.path} />
          {/* A long name shrinks only once its folder has gone. */}
          <span className="min-w-0 truncate">{baseName(file.path)}</span>
          <span className="text-muted-foreground min-w-0 flex-1 truncate text-xs">{dirName(file.path)}</span>
        </button>
        {onReplace && (
          <TooltipIconButton tooltip="Replace in this file" size="icon-xs" onClick={onReplace}
            className="hidden group-focus-within/file:inline-flex group-hover/file:inline-flex">
            <ReplaceAllGlyph />
          </TooltipIconButton>
        )}
        <span className="bg-muted text-muted-foreground me-1 shrink-0 rounded-full px-1.5 text-xs tabular-nums">{file.matches}</span>
      </div>
      {open && (
        <ul aria-label={`Matches in ${baseName(file.path)}`}>
          {file.lines.map((line) => (
            <li key={line.line}>
              <button type="button" title={`${file.path}:${line.line}`}
                onClick={() => onOpen(file.path, line.line, false)}
                onDoubleClick={() => onOpen(file.path, line.line, true)}
                className="hover:bg-muted rounded-control flex h-control-sm w-full items-center ps-8 pe-2 text-start text-sm">
                <Preview line={line} />
              </button>
            </li>
          ))}
        </ul>
      )}
    </li>
  );
});

/** A result line with its matches marked. */
const Preview: FC<{ line: SearchLine }> = ({ line }) => {
  const parts: ReactNode[] = [];
  let at = 0;
  for (const [index, range] of line.ranges.entries()) {
    if (range.start > at) parts.push(line.preview.slice(at, range.start));
    parts.push(<mark key={index} className="bg-warning/30 rounded-xs text-inherit">{line.preview.slice(range.start, range.end)}</mark>);
    at = range.end;
  }
  parts.push(line.preview.slice(at));
  return <span className="text-muted-foreground min-w-0 truncate whitespace-pre">{parts}</span>;
};
