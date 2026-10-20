import { useCallback, useEffect, useId, useState } from "react";

import { CheckboxRow, errorText } from "@/app/dialogs/fields";
import { SettingsCard, SettingsPage, SettingsSection } from "@/app/settings/parts";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Globe } from "@/components/icons";
import type { ImportFailure, ImportProgress, ImportSource } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { openSettings } from "@/state/actions";
import {
  counted,
  doneToast,
  doneTone,
  importBrowser,
  importedBadge,
  importSources,
  lastImportSources,
  openFullDiskAccess,
  openingImportSources,
  opensDialog,
  progressText,
  reasonText,
  retryable,
  type RowTone,
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
            <p className="text-destructive px-1 text-sm">{list.message}</p>
          ) : list.sources.length === 0 ? (
            <p className="text-foreground/65 px-1 text-sm">No supported browsers were found on this machine.</p>
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
          <p data-slot="browser-off" className="text-foreground/65 px-1 text-xs">
            The built-in browser is off, so there is nothing to import into. Turn it on in{" "}
            <button type="button" className="text-link hover:underline" onClick={() => openSettings("browser")}>
              Settings › Browser
            </button>
            .
          </p>
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

/**
 * While the browsers are first listed: rows the size of a browser's, empty, so the list fills
 * in where they stand rather than pushing the page down.
 */
function ListingRows() {
  return (
    <SettingsCard>
      <div role="status" aria-label="Looking for browsers" data-slot="import-listing">
        {[0, 1, 2].map((row) => (
          <div key={row} aria-hidden className="flex items-center gap-3 px-4 py-3">
            <span className="bg-foreground/5 size-8 shrink-0 rounded-lg" />
            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
              {/* Lines of a row's sizes, their words hidden. */}
              <span className="text-label w-fit rounded-sm bg-foreground/5 text-transparent select-none">
                A browser&apos;s name
              </span>
              <span className="w-fit rounded-sm bg-foreground/5 text-xs text-transparent select-none">
                Where it stands, and its profiles
              </span>
            </div>
          </div>
        ))}
      </div>
    </SettingsCard>
  );
}

/** A browser's app icon, or a globe in a tile when it has none. */
function BrowserIcon({ source, className }: { source: ImportSource; className?: string }) {
  return source.icon ? (
    <img src={source.icon} alt="" aria-hidden className={cn("size-8 shrink-0", className)} />
  ) : (
    <span aria-hidden className={cn("bg-foreground/5 text-foreground/65 flex size-8 shrink-0 items-center justify-center rounded-lg", className)}>
      <Globe className="size-icon-md" />
    </span>
  );
}

const DOT: Record<RowTone, string> = {
  ready: "bg-success",
  attention: "bg-warning",
  idle: "border-foreground/45 border",
};

function BrowserRow({
  source,
  nowMs,
  disabled,
  onAction,
}: {
  source: ImportSource;
  nowMs: number;
  disabled: boolean;
  onAction: () => void;
}) {
  const view = rowView(source);
  return (
    <div data-slot="import-row" data-browser={source.key} className="flex items-center gap-3 px-4 py-3">
      <BrowserIcon source={source} />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-2">
          <span className="text-label truncate font-medium">{source.name}</span>
          {source.record && (
            <Badge data-slot="import-badge" variant="secondary" className="font-normal">
              {importedBadge(source.record, nowMs)}
            </Badge>
          )}
        </div>
        <div className="text-foreground/65 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-0.5 text-xs">
          <span data-slot="import-status" className={cn("inline-flex shrink-0 items-center gap-1.5", view.tone === "attention" && "text-warning")}>
            <span aria-hidden className={cn("size-1.5 shrink-0 rounded-full", DOT[view.tone])} />
            {view.status}
          </span>
          {view.details.map((detail) => (
            <span key={detail} data-slot="import-detail" className="min-w-0">
              {detail}
            </span>
          ))}
        </div>
      </div>
      {view.label && (
        <Button variant="soft" size="sm" disabled={disabled || view.action === "none"} onClick={onAction}>
          {view.label}
        </Button>
      )}
    </div>
  );
}

/** The heading every step of the dialog shares: the browser's icon and what is going on. */
function FlowHeader({ source, title, description }: { source: ImportSource; title: string; description: string }) {
  return (
    <DialogHeader>
      <DialogTitle className="flex items-center gap-2.5">
        <BrowserIcon source={source} className="size-6" />
        {title}
      </DialogTitle>
      <DialogDescription>{description}</DialogDescription>
    </DialogHeader>
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

/** How far the cookies have come; indeterminate until they are counted. */
function ImportBar({ progress }: { progress: ImportProgress | null }) {
  const share = progress?.type === "cookies" && progress.total > 0 ? progress.done / progress.total : progress?.type === "history" ? 1 : null;
  return (
    <div
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={share === null ? undefined : Math.round(share * 100)}
      className="bg-foreground/8 h-1.5 overflow-hidden rounded-full"
    >
      <div
        className={cn("bg-foreground/60 h-full rounded-full transition-[width] motion-reduce:transition-none", share === null && "w-1/3 animate-pulse")}
        style={share === null ? undefined : { width: `${share * 100}%` }}
      />
    </div>
  );
}

/** Which profiles to copy, and whether history comes too. */
function ChooseStep({
  source,
  onCancel,
  onImport,
}: {
  source: ImportSource;
  onCancel: () => void;
  onImport: (source: ImportSource, ids: string[], history: boolean) => void;
}) {
  const [ticked, setTicked] = useState<Set<string>>(() => new Set(source.profiles[0] ? [source.profiles[0].id] : []));
  const [history, setHistory] = useState(true);
  const chosen = source.profiles.filter((profile) => ticked.has(profile.id));
  const cookies = chosen.reduce((sum, profile) => sum + (profile.cookies ?? 0), 0);
  const tick = (id: string, on: boolean) =>
    setTicked((current) => {
      const next = new Set(current);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });
  const note =
    source.family === "chromium"
      ? `${source.name} must stay closed during the import. macOS may ask for Keychain access to its encryption key; choose Allow (Always Allow avoids asking again).`
      : source.family === "gecko"
        ? `${source.name} must stay closed during the import.`
        : null;
  return (
    <>
      <FlowHeader
        source={source}
        title={`Import from ${source.name}`}
        description="Which profiles should be copied into Brigadier's browser?"
      />
      <div data-slot="import-profiles" className="border-divider bg-card rounded-settings flex flex-col overflow-hidden border">
        {source.profiles.map((profile) => (
          <ProfilePick
            key={profile.id}
            name={profile.name}
            cookies={profile.cookies}
            checked={ticked.has(profile.id)}
            onCheckedChange={(on) => tick(profile.id, on)}
          />
        ))}
      </div>
      <CheckboxRow
        label="Also bring browsing history"
        note="The pages you visited, to search and revisit."
        checked={history}
        onCheckedChange={setHistory}
      />
      {note && <p className="text-foreground/65 text-xs">{note}</p>}
      <DialogFooter>
        <Button type="button" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type="button"
          disabled={chosen.length === 0}
          onClick={() => onImport(source, chosen.map((profile) => profile.id), history)}
        >
          Import {counted(cookies, "cookie", "cookies")}
        </Button>
      </DialogFooter>
    </>
  );
}

function ProfilePick({
  name,
  cookies,
  checked,
  onCheckedChange,
}: {
  name: string;
  cookies: number | null;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  const id = useId();
  return (
    <div
      data-slot="import-profile"
      className="not-last:border-divider flex items-start gap-3 px-4 py-3 not-last:border-b"
    >
      <Checkbox
        id={id}
        checked={checked}
        onCheckedChange={(state) => onCheckedChange(state === true)}
        className="mt-0.5"
      />
      <label htmlFor={id} className="grid min-w-0 flex-1 gap-0.5">
        <span className="text-label break-words">{name}</span>
        {cookies !== null && <span className="text-foreground/65 text-xs">{counted(cookies, "cookie", "cookies")}</span>}
      </label>
    </div>
  );
}
