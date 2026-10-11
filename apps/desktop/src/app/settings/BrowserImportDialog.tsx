import { Check } from "@openai/apps-sdk-ui/components/Icon";
import { useCallback, useEffect, useState } from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import { SettingsSelect } from "@/app/settings/parts";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { ImportProfile, ImportProgress, ImportResult } from "@/ipc/generated";
import {
  firstProfile,
  importBrowser,
  importProfiles,
  importSummary,
  openFullDiskAccess,
} from "@/state/browserData";

const number = new Intl.NumberFormat();

function profileLabel(profile: ImportProfile): string {
  return `${profile.browser} — ${profile.name}`;
}

/** What macOS will ask before this import, in plain words; null when nothing. */
function permissionNote(profile: ImportProfile, cookies: boolean): string | null {
  if (profile.family === "chromium" && cookies) {
    return `macOS will ask whether Brigadier may use “${profile.browser} Safe Storage” from your Keychain: the key ${profile.browser} locks its cookies with. Choose Allow. Brigadier uses it only to copy the cookies.`;
  }
  return null;
}

function progressLine(progress: ImportProgress | null, browser: string): string {
  if (!progress || progress.type === "reading") return `Reading ${browser}'s data…`;
  if (progress.type === "cookies") {
    return `Adding cookies… ${number.format(progress.done)} of ${number.format(progress.total)}`;
  }
  return "Adding history…";
}

/**
 * Import from another browser: a one-time copy of a profile's cookies (stay signed in) and
 * history into Brigadier's browser. Never passwords or extensions; the other browser must be quit.
 */
export function BrowserImportDialog({
  open,
  onOpenChange,
  onImported,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onImported?: () => void;
}) {
  const [busy, setBusy] = useState(false);
  return (
    <Dialog open={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>Import from your browser</DialogTitle>
          <DialogDescription>Choose what to bring over to Brigadier's browser.</DialogDescription>
        </DialogHeader>
        {/* Made each time it opens: the browsers and whether they're open are read afresh. */}
        {open && (
          <ImportForm busy={busy} setBusy={setBusy} onImported={onImported} onClose={() => onOpenChange(false)} />
        )}
      </DialogContent>
    </Dialog>
  );
}

function ImportForm({
  busy,
  setBusy,
  onImported,
  onClose,
}: {
  busy: boolean;
  setBusy: (busy: boolean) => void;
  onImported?: (() => void) | undefined;
  onClose: () => void;
}) {
  const [profiles, setProfiles] = useState<ImportProfile[] | null>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  const [cookies, setCookies] = useState(true);
  const [history, setHistory] = useState(true);
  const [progress, setProgress] = useState<ImportProgress | null>(null);
  const [result, setResult] = useState<ImportResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(
    () =>
      importProfiles().then(
        (found) => {
          setProfiles(found);
          setChosen((current) =>
            current && found.some((p) => p.id === current) ? current : (firstProfile(found)?.id ?? null),
          );
        },
        (cause: unknown) => setError(errorText(cause)),
      ),
    [],
  );
  useEffect(() => {
    void load();
  }, [load]);

  const profile = profiles?.find((p) => p.id === chosen);
  const wantCookies = cookies && (profile?.hasCookies ?? false);
  const wantHistory = history && (profile?.hasHistory ?? false);

  const start = () => {
    if (!profile) return;
    setBusy(true);
    setError(null);
    setProgress(null);
    importBrowser(profile.id, { cookies: wantCookies, history: wantHistory }, setProgress)
      .then((done) => {
        setResult(done);
        onImported?.();
      })
      .catch((cause: unknown) => setError(errorText(cause)))
      .finally(() => setBusy(false));
  };

  if (result && profile) {
    return (
      <>
        <div className="flex flex-col gap-2 text-sm">
          {importSummary(profile.browser, result).map((line) => (
            <p key={line}>{line}</p>
          ))}
        </div>
        <DialogFooter>
          <Button type="button" onClick={onClose}>
            Done
          </Button>
        </DialogFooter>
      </>
    );
  }

  if (profiles && profiles.length === 0) {
    return (
      <>
        <p className="text-foreground/65 text-label">
          No other browser was found on this Mac. Brigadier can import from Safari, Chrome, Arc, Dia, Brave, Edge,
          Firefox, Zen and other Chromium- or Firefox-based browsers.
        </p>
        <DialogFooter>
          <Button type="button" onClick={onClose}>
            Close
          </Button>
        </DialogFooter>
      </>
    );
  }

  const note = profile && permissionNote(profile, wantCookies);
  const needsAccess = profile && !profile.readable;
  return (
    <>
      <div className="flex items-center gap-3">
        <span className="text-foreground/65 text-label shrink-0">From</span>
        <div className="min-w-0 flex-1 [&>button]:w-full">
          <SettingsSelect
            label="Import from"
            value={chosen ?? ""}
            options={(profiles ?? []).map((p) => ({
              value: p.id,
              label: profileLabel(p),
              ...(p.running ? { hint: "Open now: quit it first" } : {}),
            }))}
            onChange={setChosen}
            disabled={busy || !profiles}
          />
        </div>
      </div>
      {profile && (
        <div className="flex flex-col gap-1">
          <ImportBox
            label="Cookies"
            detail="Stay signed in to the sites you use."
            checked={wantCookies}
            disabled={busy || !profile.hasCookies}
            onChange={setCookies}
          />
          <ImportBox
            label="Browsing history"
            detail="The pages you visited, to search and revisit."
            checked={wantHistory}
            disabled={busy || !profile.hasHistory}
            onChange={setHistory}
          />
        </div>
      )}
      {profile?.running && (
        <p className="text-label">
          Quit {profile.browser} first, so everything copies as it is.{" "}
          <button type="button" className="underline underline-offset-2" onClick={() => void load()}>
            Check again
          </button>
        </p>
      )}
      {needsAccess && (
        <div className="flex flex-col items-start gap-2">
          <p className="text-label">
            To copy from {profile.browser}, allow Brigadier in System Settings › Privacy & Security › Full Disk
            Access, then open this again.
          </p>
          <Button type="button" variant="outline" size="sm" onClick={() => void openFullDiskAccess()}>
            Open System Settings
          </Button>
        </div>
      )}
      {note && !profile.running && !needsAccess && <p className="text-foreground/65 text-xs">{note}</p>}
      <p className="text-foreground/65 text-xs">A one-time copy. Passwords and extensions aren't brought over.</p>
      {busy && <p className="text-label">{progressLine(progress, profile?.browser ?? "")}</p>}
      <ErrorLine error={error} />
      <DialogFooter>
        <Button type="button" variant="ghost" disabled={busy} onClick={onClose}>
          Cancel
        </Button>
        <Button
          type="button"
          disabled={busy || !profile || profile.running || !!needsAccess || (!wantCookies && !wantHistory)}
          onClick={start}
        >
          {busy ? "Importing…" : "Import"}
        </Button>
      </DialogFooter>
    </>
  );
}

function ImportBox({
  label,
  detail,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  detail: string;
  checked: boolean;
  disabled: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-3 py-1.5 has-disabled:cursor-default has-disabled:opacity-50">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
        className="peer sr-only"
      />
      <span
        aria-hidden
        className="border-border peer-checked:bg-foreground/10 peer-focus-visible:ring-ring/50 mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-xs border peer-focus-visible:ring-1"
      >
        {checked && <Check className="size-icon-xs" />}
      </span>
      <span className="flex flex-col gap-0.5">
        <span className="text-label">{label}</span>
        <span className="text-foreground/65 text-xs">{detail}</span>
      </span>
    </label>
  );
}
