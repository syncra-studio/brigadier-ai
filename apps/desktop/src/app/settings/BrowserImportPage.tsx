import { useCallback, useEffect, useState } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { BrowserOffNote } from "@/app/settings/BrowserOffNote";
import { BrowserRow } from "@/app/settings/BrowserRow";
import { ChooseStep } from "@/app/settings/ChooseStep";
import { FlowHeader } from "@/app/settings/FlowHeader";
import { ImportBar } from "@/app/settings/ImportBar";
import { ImportStatusLine } from "@/app/settings/ImportStatusLine";
import { ListingRows } from "@/app/settings/ListingRows";
import { SettingsPage, SettingsSection } from "@/app/settings/parts";
import { Button } from "@/components/controls";
import { Dialog, DialogContent, DialogFooter } from "@/components/overlays";
import { SettingsCard } from "@/components/shared";
import type { ImportFailure, ImportProgress, ImportSource } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import {
  doneToast,
  doneTone,
  importBrowser,
  importSources,
  lastImportSources,
  openFullDiskAccess,
  openingImportSources,
  opensDialog,
  progressText,
  reasonText,
  retryable,
  rowView,
} from "@/state/browserImport";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/** The Import Browser Data page's rows, for the page and for Settings search. */
export const BROWSER_IMPORT_ROWS = {
  import: {
    label: "Import from your browser",
    description:
      "Bring cookies and browsing history from Safari, Chrome, Arc, Dia, Brave, Edge, Firefox, Zen and other browsers",
  },
} as const;

const SECTION_DESCRIPTION =
  "Bring your sign-ins from another browser on this Mac into Brigadier's browser, so previews and agent tabs open already signed in.";

/** The browsers as last listed; `atMs` is when, which "Imported 3d ago" counts from. */
type List =
  | { status: "loading" }
  | { status: "ready"; sources: ImportSource[]; atMs: number }
  | { status: "error"; message: string };

/** The dialog an import is in, if any. */
type Flow =
  | { step: "choose"; source: ImportSource }
  | { step: "importing"; source: ImportSource; progress: ImportProgress | null }
  | { step: "fullDiskAccess"; source: ImportSource; checked: boolean; checking: boolean }
  | { step: "blocked"; source: ImportSource; reason: ImportFailure; message: string | null; checking: boolean };

/**
 * Settings › Import Browser Data: one row per browser installed on this Mac, with where it
 * stands and one button. Import… copies a browser's cookies (and, if wanted, its history) into
 * Brigadier's one browser: straight away for one profile, after picking for several. While the
 * browser is off there is nothing to import into, so the buttons wait, with a line that says why
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
    (read: () => Promise<ImportSource[]> = importSources) =>
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

  const run = (source: ImportSource, ids: string[], history: boolean) => {
    setFlow({ step: "importing", source, progress: null });
    const onProgress = (progress: ImportProgress) =>
      setFlow((current) => (current?.step === "importing" ? { ...current, progress } : current));
    importBrowser(source.key, ids, history, onProgress)
      .then((outcome) => {
        if (outcome.type === "done") {
          setFlow(null);
          toast(doneToast(source.name, outcome.record), { tone: doneTone(outcome.record) });
          if (outcome.historyError) toast(`${source.name}: ${outcome.historyError}`, { tone: "error" });
          return;
        }
        const { reason, message } = outcome;
        if (reason === "needsFullDiskAccess") {
          setFlow({ step: "fullDiskAccess", source, checked: true, checking: false });
        } else if (opensDialog(reason)) {
          setFlow({ step: "blocked", source, reason, message: null, checking: false });
        } else {
          setFlow(null);
          toast(`${source.name}: ${reason === "readFailed" ? message : reasonText(reason, source.name)}`, { tone: "error" });
        }
      })
      .catch((cause: unknown) => {
        setFlow(null);
        toast(`${source.name}: ${errorText(cause)}`, { tone: "error" });
      })
      .finally(() => void load());
  };

  /** Import… on a browser that can be imported from. */
  const start = (source: ImportSource) => {
    const [only, ...more] = source.profiles;
    if (!only) setFlow({ step: "blocked", source, reason: "gone", message: null, checking: false });
    else if (more.length > 0) setFlow({ step: "choose", source });
    else run(source, [only.id], true);
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

  const act = (source: ImportSource) => {
    const { action } = rowView(source);
    if (action === "recheck") void load();
    else if (action === "grant") setFlow({ step: "fullDiskAccess", source, checked: false, checking: false });
    else if (action === "import") start(source);
  };

  const importing = flow?.step === "importing";
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
        onImport={run}
        onRecheck={recheck}
      />
    </SettingsPage>
  );
}

function ImportDialog({
  flow: current,
  onClose,
  onImport,
  onRecheck,
}: {
  flow: Flow | null;
  onClose: () => void;
  onImport: (source: ImportSource, ids: string[], history: boolean) => void;
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
        className="max-w-md"
        showCloseButton={flow?.step !== "importing"}
        onEscapeKeyDown={hold}
        onInteractOutside={hold}
      >
        {flow?.step === "choose" && <ChooseStep key={flow.source.key} source={flow.source} onCancel={onClose} onImport={onImport} />}
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
