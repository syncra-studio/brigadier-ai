import { useCallback, useEffect, useState } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { BrowserOffNote } from "@/app/settings/BrowserOffNote";
import { BrowserRow } from "@/app/settings/BrowserRow";
import { ProfileImportPick } from "@/app/settings/ProfileImportPick";
import { ProfileImportResults } from "@/app/settings/ProfileImportResults";
import { FlowHeader } from "@/app/settings/FlowHeader";
import { ImportBar } from "@/app/settings/ImportBar";
import { ImportStatusLine } from "@/app/settings/ImportStatusLine";
import { ListingRows } from "@/app/settings/ListingRows";
import { SettingsPage, SettingsSection } from "@/app/settings/parts";
import { Button } from "@/components/controls";
import { Dialog, DialogContent, DialogFooter } from "@/components/overlays";
import { SettingsCard } from "@/components/shared";
import type { ImportFailure, ImportProgress } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import {
  type ImportChoice,
  importProfile,
  type ImportWhat,
  importSources,
  lastImportSources,
  type ListedSource,
  type ProfileImportResult,
  useImportInto,
  openFullDiskAccess,
  openingImportSources,
  opensDialog,
  progressText,
  reasonText,
  retryable,
  rowView,
} from "@/state/browserImport";
import { useProfiles } from "@/state/browserProfiles";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/** The Import Browser Data page's rows, for the page and for Settings search. */
export const BROWSER_IMPORT_ROWS = {
  import: {
    label: "Import from your browser",
    description:
      "Bring sign-ins, history, bookmarks and passwords from Safari, Chrome, Arc, Dia, Brave, Edge, Firefox, Zen and other browsers",
  },
} as const;

const SECTION_DESCRIPTION =
  "Bring your profiles from another browser on this computer. Each one becomes its own profile here, so previews and agent tabs open already signed in.";

/** The browsers as last listed; `atMs` is when, which "Imported 3d ago" counts from. */
type List =
  | { status: "loading" }
  | { status: "ready"; sources: ListedSource[]; atMs: number }
  | { status: "error"; message: string };

/** The dialog an import is in, if any. */
type Flow =
  | { step: "choose"; source: ListedSource }
  | { step: "results"; source: ListedSource; results: ProfileImportResult[] }
  | { step: "importing"; source: ListedSource; progress: ImportProgress | null }
  | { step: "fullDiskAccess"; source: ListedSource; checked: boolean; checking: boolean }
  | { step: "blocked"; source: ListedSource; reason: ImportFailure; message: string | null; checking: boolean };

/**
 * Settings › Import Browser Data: one row per browser installed on this computer, with where it
 * stands and one button. Import… asks which of its profiles to bring, each as a new profile or
 * replacing one, and what of them, then says per profile how each part went. While the browser
 * is off there is nothing to import into, so the buttons wait, with a line that says why
 * and leads to Settings › Browser.
 */
export function BrowserImportPage() {
  const on = useApp((s) => s.settings.browserEnabled);
  // The browsers as last listed show at once; the page lists them again as it opens.
  const [list, setList] = useState<List>(() => {
    const last = lastImportSources();
    return last ? { status: "ready", ...last } : { status: "loading" };
  });
  const [flow, setFlow] = useState<Flow | null>(null);

  const fetchList = useCallback(
    (read: () => Promise<ListedSource[]> = importSources) =>
      read().then(
        (sources) => {
          setList({ status: "ready", sources, atMs: Date.now() });
          return sources;
        },
        (cause: unknown) => {
          setList({ status: "error", message: errorText(cause) || "Could not check the installed browsers." });
          return null;
        },
      ),
    [],
  );
  useEffect(() => {
    // A list on its way since the pointer came to the page's row answers for the opening.
    void fetchList(openingImportSources);
  }, [fetchList]);
  const load = () => {
    setList((current) => (current.status === "ready" ? current : { status: "loading" }));
    return fetchList();
  };

  const onProgress = (progress: ImportProgress) =>
    setFlow((current) => (current?.step === "importing" ? { ...current, progress } : current));

  /** Imports the ticked profiles in turn; a refusal that needs the person stops the rest. */
  const run = async (source: ListedSource, choices: ImportChoice[], what: ImportWhat) => {
    setFlow({ step: "importing", source, progress: null });
    const results: ProfileImportResult[] = [];
    try {
      for (const choice of choices) {
        const result = await importProfile(source, choice, what, onProgress);
        const { outcome } = result;
        if (outcome.type === "failed" && (outcome.reason === "needsFullDiskAccess" || opensDialog(outcome.reason))) {
          if (outcome.reason === "needsFullDiskAccess") setFlow({ step: "fullDiskAccess", source, checked: true, checking: false });
          else setFlow({ step: "blocked", source, reason: outcome.reason, message: null, checking: false });
          return;
        }
        results.push(result);
      }
      setFlow({ step: "results", source, results });
    } catch (cause: unknown) {
      setFlow(null);
      toast(`${source.name}: ${errorText(cause)}`, { tone: "error" });
    } finally {
      void load();
    }
  };

  /** Try again on one part that failed: that part only, into the profile it went to. */
  const retry = async (source: ListedSource, index: number, part: keyof ImportWhat) => {
    const earlier = flow?.step === "results" ? flow.results[index] : null;
    if (!earlier) return;
    const result = await importProfile(
      source,
      { ...earlier.choice, into: earlier.profile.id },
      { signIns: false, history: false, bookmarks: false, passwords: false, [part]: true },
      () => {},
    ).catch((cause: unknown) => {
      toast(`${source.name}: ${errorText(cause)}`, { tone: "error" });
      return null;
    });
    if (!result) return;
    setFlow((current) => {
      if (current?.step !== "results") return current;
      const results = current.results.map((each, at) => (at === index ? mergeRetry(each, result, part) : each));
      return { ...current, results };
    });
  };

  /** Import… on a browser that can be imported from. */
  const start = (source: ListedSource) => {
    if (source.profiles.length === 0) setFlow({ step: "blocked", source, reason: "gone", message: null, checking: false });
    else setFlow({ step: "choose", source });
  };

  /** Looks again after the person put something right, and goes on from where the browser is now. */
  const recheck = (current: Extract<Flow, { checking: boolean }>) => {
    setFlow({ ...current, checking: true });
    void load().then((sources) => {
      const fresh = sources?.find((source) => source.key === current.source.key);
      const blocked = (reason: ImportFailure) =>
        setFlow({ step: "blocked", source: fresh ?? current.source, reason, message: null, checking: false });
      if (!fresh) return blocked("gone");
      switch (fresh.state) {
        case "running":
          return blocked("running");
        case "needsFullDiskAccess":
          return setFlow({ step: "fullDiskAccess", source: fresh, checked: current.step === "fullDiskAccess", checking: false });
        case "noKeychainKey":
          return blocked("noKeychainKey");
        case "unavailable":
          return blocked("readFailed");
        default:
          return start(fresh);
      }
    });
  };

  const act = (source: ListedSource) => {
    const { action } = rowView(source);
    if (action === "recheck") void load();
    else if (action === "grant") setFlow({ step: "fullDiskAccess", source, checked: false, checking: false });
    else if (action === "import") start(source);
  };

  const importing = flow?.step === "importing";
  const profiles = useProfiles();
  const already = (source: ListedSource) => {
    const ids = Object.entries(source.imports).filter(([from]) => from.startsWith(`${source.key}/`)).map(([, place]) => place?.into);
    const names = profiles.filter((profile) => ids.includes(profile.id)).map((profile) => profile.name);
    if (names.length === 0) return undefined;
    return `${new Intl.ListFormat("en", { type: "conjunction" }).format(names)} already in Brigadier`;
  };
  return (
    <SettingsPage title="Import Browser Data">
      <SettingsSection
        title="Browsers"
        description={SECTION_DESCRIPTION}
        actions={
          <Button type="button" variant="ghost" size="sm" disabled={list.status === "loading" || importing} onClick={() => void load()}>
            Refresh
          </Button>
        }
      >
        <div data-setting={BROWSER_IMPORT_ROWS.import.label}>
          {list.status === "loading" ? (
            <ListingRows />
          ) : list.status === "error" ? (
            <ImportStatusLine error>{list.message}</ImportStatusLine>
          ) : list.sources.length === 0 ? (
            <ImportStatusLine>No supported browsers were found on this machine.</ImportStatusLine>
          ) : (
            <SettingsCard>
              {list.sources.map((source) => (
                <BrowserRow
                  key={source.key}
                  source={source}
                  nowMs={list.atMs}
                  disabled={!on || importing}
                  alreadyIn={already(source)}
                  skipped={emptyLine(source)}
                  onAction={() => act(source)}
                />
              ))}
            </SettingsCard>
          )}
        </div>
        {!on && (
          <BrowserOffNote data-slot="browser-off">
            The built-in browser is off, so there is nothing to import into. Turn it on in{" "}
            <button type="button" className="text-link hover:underline" onClick={() => openSettings("browser")}>
              Settings › Browser
            </button>
            .
          </BrowserOffNote>
        )}
      </SettingsSection>
      <ImportDialog
        flow={flow}
        onClose={() => setFlow(null)}
        onImport={(source, choices, what) => void run(source, choices, what)}
        onRetry={(source, index, part) => void retry(source, index, part)}
        onRecheck={recheck}
      />
    </SettingsPage>
  );
}

function ImportDialog({
  flow: current,
  onClose,
  onImport,
  onRetry,
  onRecheck,
}: {
  flow: Flow | null;
  onClose: () => void;
  onImport: (source: ListedSource, choices: ImportChoice[], what: ImportWhat) => void;
  onRetry: (source: ListedSource, index: number, part: keyof ImportWhat) => void;
  onRecheck: (flow: Extract<Flow, { checking: boolean }>) => void;
}) {
  // While it closes, the dialog keeps showing its last step instead of shrinking away blank.
  const [last, setLast] = useState(current);
  if (current !== null && current !== last) setLast(current);
  const flow = current ?? last;
  const importing = current?.step === "importing";
  // An import can't be walked away from halfway: no close button, Esc and clicks outside wait.
  const hold = (event: Event) => importing && event.preventDefault();
  return (
    <Dialog open={current !== null} onOpenChange={(open) => !open && !importing && onClose()}>
      <DialogContent
        data-slot="import-dialog"
        data-step={flow?.step}
        className={flow?.step === "choose" || flow?.step === "results" ? "max-w-import-dialog" : "max-w-md"}
        showCloseButton={flow?.step !== "importing"}
        onEscapeKeyDown={hold}
        onInteractOutside={hold}
      >
        {flow?.step === "choose" && <ImportPick source={flow.source} onCancel={onClose} onImport={onImport} />}
        {flow?.step === "results" && (
          <ProfileImportResults
            source={flow.source}
            results={flow.results}
            onRetry={(index, part) => onRetry(flow.source, index, part)}
            onDone={onClose}
          />
        )}
        {flow?.step === "importing" && (
          <>
            <FlowHeader source={flow.source} title={`Importing from ${flow.source.name}…`} description={progressText(flow.progress)} />
            <ImportBar progress={flow.progress} />
          </>
        )}
        {flow?.step === "fullDiskAccess" && (
          <>
            <FlowHeader
              source={flow.source}
              title={`Allow Full Disk Access for ${flow.source.name}`}
              description={`${flow.source.name} keeps its cookies in a protected folder. Turn on Full Disk Access for Brigadier in System Settings › Privacy & Security, then come back. You can turn it off again after the import.`}
            />
            {flow.checked && !flow.checking && (
              <p data-slot="import-still-off" className="text-destructive text-xs">
                Full Disk Access is still off. macOS may need Brigadier to quit and reopen before the change applies.
              </p>
            )}
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={onClose}>
                Cancel
              </Button>
              <Button type="button" variant="outline" onClick={() => void openFullDiskAccess()}>
                Open System Settings
              </Button>
              <Button type="button" disabled={flow.checking} onClick={() => onRecheck(flow)}>
                {flow.checking ? "Checking…" : "I've turned it on"}
              </Button>
            </DialogFooter>
          </>
        )}
        {flow?.step === "blocked" && (
          <>
            <FlowHeader
              source={flow.source}
              title={`Can't import from ${flow.source.name}`}
              description={flow.message ?? reasonText(flow.reason, flow.source.name)}
            />
            <DialogFooter>
              <Button type="button" variant="ghost" onClick={onClose}>
                Close
              </Button>
              {retryable(flow.reason) && (
                <Button type="button" disabled={flow.checking} onClick={() => onRecheck(flow)}>
                  {flow.checking ? "Checking…" : flow.reason === "running" ? "I've quit it" : "Try again"}
                </Button>
              )}
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

/** Reads which profile the page was asked to import into, once. */
function ImportPick({
  source,
  onCancel,
  onImport,
}: {
  source: ListedSource;
  onCancel: () => void;
  onImport: (source: ListedSource, choices: ImportChoice[], what: ImportWhat) => void;
}) {
  const [into] = useState(() => {
    const asked = useImportInto.getState().into;
    useImportInto.setState({ into: null });
    return asked;
  });
  return <ProfileImportPick key={source.key} source={source} into={into} onCancel={onCancel} onImport={onImport} />;
}

/** The source profiles with nothing to bring, which get no profile here. */
function emptyLine(source: ListedSource): string | undefined {
  const names = source.emptyProfiles;
  if (names.length === 0) return undefined;
  return `Skipped ${new Intl.ListFormat("en", { type: "conjunction" }).format(names)}: nothing to bring`;
}

/** A retried part's answer put over the first try's, which keeps the other parts. */
function mergeRetry(first: ProfileImportResult, again: ProfileImportResult, part: keyof ImportWhat): ProfileImportResult {
  if (first.outcome.type !== "done" || again.outcome.type !== "done") return part === "passwords" && first.outcome.type !== "done" ? again : first;
  const before = first.outcome;
  const now = again.outcome;
  if (part === "passwords") {
    const { passwordsError: _gone, ...rest } = before;
    const passwords = now.record.passwords;
    return { ...first, outcome: { ...rest, ...(now.passwordsError ? { passwordsError: now.passwordsError } : {}), record: { ...before.record, ...(passwords === undefined ? {} : { passwords }) } } };
  }
  if (part === "bookmarks") {
    const { bookmarksError: _gone, ...rest } = before;
    const bookmarks = now.record.bookmarks;
    return { ...first, outcome: { ...rest, ...(now.bookmarksError ? { bookmarksError: now.bookmarksError } : {}), record: { ...before.record, ...(bookmarks === undefined ? {} : { bookmarks }) } } };
  }
  if (part === "history") return { ...first, outcome: { ...before, historyError: now.historyError, record: { ...before.record, history: now.record.history } } };
  return first;
}
