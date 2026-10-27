import { useEffect, useRef, useState, type ReactNode } from "react";

import { useAction } from "@/app/conversation/useAction";
import { Badge, Button, TooltipIconButton } from "@/components/controls";
import { Spinner, X } from "@/components/icons";
import { Dialog, DialogContent, DialogTitle } from "@/components/overlays";
import { Banner, CodeEditor, CodeFileHeader } from "@/components/shared";
import { formatBytes, shortPath } from "@/lib/format";
import { languageOf } from "@/lib/highlight";
import { cn } from "@/lib/utils";
import type { SkillSave } from "./adapter";
import type { PluginItemDetails, PluginItemView, PluginServerSettings } from "./types";

/** What a configured value shows as: never the value itself. */
export const HIDDEN_VALUE = "••••••••";

/** Each name with its value hidden. */
const hidden = (names: string[]) => Object.fromEntries(names.map((name) => [name, HIDDEN_VALUE]));

/** A server's settings as a file to read: its command, arguments and env names (or address and header names), no value. */
export function settingsText(settings: PluginServerSettings): string {
  const shown =
    settings.type === "stdio"
      ? { command: settings.command, args: settings.args, ...(settings.cwd ? { cwd: settings.cwd } : {}), ...(settings.envNames.length ? { env: hidden(settings.envNames) } : {}) }
      : { url: settings.url, ...(settings.headerNames.length ? { headers: hidden(settings.headerNames) } : {}) };
  return `${JSON.stringify(shown, null, 2)}\n`;
}

/**
 * One file the dialog shows: a SKILL.md (editable unless `readOnly` says why not), or a server's
 * settings, named `<server>.json` and found in the file they're saved in (`path`).
 */
type Shown = { name: string; path: string; shown: string; text: string; readOnly: string | null };

/** What Open shows of an item: each SKILL.md, or the server's settings beside the file it's saved in. */
function shownFiles(item: Pick<PluginItemView, "name" | "kind" | "files">, details: PluginItemDetails): Shown[] {
  if (details.server) {
    const shown = `${item.name}.json`;
    return [{ name: item.name, path: item.files[0] ?? shown, shown, text: settingsText(details.server), readOnly: "Server settings are read only here." }];
  }
  return details.skills.map((skill) => ({ name: skill.name, path: skill.path, shown: skill.path, text: skill.contents, readOnly: skill.readOnly }));
}

/**
 * Open: an item's files in a dialog the size of a file tab, each drawn as the thread's file tab
 * draws a file. A skill's SKILL.md keeps its edits until Save (or ⌘S); while it has unsaved
 * edits, only Cancel or × closes the dialog, so a stray Escape or click outside doesn't lose them.
 * A server's settings show read only, their secret values hidden.
 */
export function FileDialog({ item, details, save, create, onClose }: {
  item: Pick<PluginItemView, "name" | "kind" | "files">;
  onClose: () => void;
} & ({
  /** Reads the item's files afresh at each opening. */
  details: () => Promise<PluginItemDetails>;
  /** Writes one SKILL.md, by its path, if it still holds `expected`: "changed", with what it holds now, when it doesn't. */
  save: (path: string, contents: string, expected: string) => Promise<SkillSave>;
  create?: never;
} | {
  details?: never;
  save?: never;
  create: {
    value: string;
    onChange: (text: string) => void;
    onSave: () => void;
    controls: ReactNode;
    extra?: ReactNode;
    error: string | null;
    busy: boolean;
    disabled: boolean;
  };
})) {
  const [loadedFiles, setFiles] = useState<Shown[] | null>(null);
  const filename = item.kind === "skill" ? "SKILL.md" : "server.json";
  const files: Shown[] | null = create ? [{ name: item.name, path: filename, shown: filename, text: "", readOnly: null }] : loadedFiles;
  const [failed, setFailed] = useState<string | null>(null);
  const [at, setAt] = useState(0);
  /** Each file's text as edited, by its path. */
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [saved, setSaved] = useState(false);
  /** The files Save found changed since they were opened, by path, with what each holds now: nothing was written to them. */
  const [stale, setStale] = useState<Record<string, string>>({});
  const action = useAction();
  // Read once, as it opens: the text as opened is what a save expects to find on disk, until an
  // explicit Reload. The page reads the item again every few seconds and draws this dialog anew
  // each time; reading again then would take an outside edit as the text opened, and a save would
  // write over it.
  const opening = useRef({ details, item });
  useEffect(() => {
    let live = true;
    const { details: read, item: opened } = opening.current;
    if (!read) return;
    read().then(
      (found) => live && setFiles(shownFiles(opened, found)),
      (cause: unknown) => live && setFailed(cause instanceof Error ? cause.message : String(cause)),
    );
    return () => {
      live = false;
    };
  }, []);
  /** Shows the stale files as they are now; the edits made here to them are dropped. */
  const reload = () => {
    setFiles((now) => now && now.map((entry) => (entry.path in stale ? { ...entry, text: stale[entry.path]! } : entry)));
    setDrafts((now) => Object.fromEntries(Object.entries(now).filter(([path]) => !(path in stale))));
    setStale({});
  };
  const staleCount = Object.keys(stale).length;
  const file = files?.[at] ?? null;
  const editable = !!file && file.readOnly === null;
  const draft = create?.value ?? (file ? drafts[file.path] ?? file.text : "");
  const changed = (files ?? []).filter((entry) => entry.readOnly === null && drafts[entry.path] !== undefined && drafts[entry.path] !== entry.text);
  const dirty = create ? !!draft.trim() : changed.length > 0;
  const anyEditable = (files ?? []).some((entry) => entry.readOnly === null);
  const store = (then?: () => void) => {
    if (!save || !dirty || action.busy || staleCount) return;
    const writes = changed.map((entry) => ({ path: entry.path, text: drafts[entry.path]!, expected: entry.text }));
    action.run(async () => {
      const refused: Record<string, string> = {};
      for (const write of writes) {
        const outcome = await save(write.path, write.text, write.expected);
        if (outcome.type === "changed") refused[write.path] = outcome.current;
      }
      const written = writes.filter((write) => !(write.path in refused));
      setFiles((now) => now && now.map((entry) => ({ ...entry, text: written.find((write) => write.path === entry.path)?.text ?? entry.text })));
      setStale(refused);
      setSaved(written.length > 0);
      if (!Object.keys(refused).length) then?.();
    });
  };
  const error = create ? create.error : action.error;
  const busy = create ? create.busy : action.busy;
  const status = error ?? (dirty ? "Unsaved changes" : saved ? "Saved" : null);
  return (
    <Dialog open onOpenChange={(open) => !open && !dirty && onClose()}>
      <DialogContent
        data-dialog={create ? "add" : "plugin-file"}
        showCloseButton={false}
        aria-describedby={undefined}
        // The text takes focus once it's read (an editable one); until then the dialog holds it, not Copy path.
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          (event.currentTarget as HTMLElement).focus();
        }}
        className="max-w-plugin-file h-plugin-file-height flex flex-col gap-0 overflow-hidden p-0">
        <CodeFileHeader
          path={file?.path ?? item.name}
          shown={file ? shortPath(file.shown) : item.name}
          // A server's settings have a file only when it's saved in one.
          reveal={!create && file && (item.kind === "skill" || file.path !== file.shown) ? file.path : null}
          heading={DialogTitle}
          actions={
            <TooltipIconButton tooltip={dirty ? "Close without saving" : "Close"} size="icon-sm" onClick={onClose}>
              <X />
            </TooltipIconButton>
          }
        >
          {file && !editable && <Badge variant="secondary" title={file.readOnly ?? undefined}>Read only</Badge>}
          {file && <span className="text-muted-foreground shrink-0 text-xs">{formatBytes(new TextEncoder().encode(draft).length)}</span>}
        </CodeFileHeader>
        {create?.controls}
        {create?.extra && <div className="border-border max-h-48 shrink-0 overflow-y-auto border-b px-4 py-3">{create.extra}</div>}
        {files && files.length > 1 && (
          <div role="tablist" aria-label="Skills" className="border-border flex shrink-0 items-center gap-1 overflow-x-auto border-b px-3 py-1.5">
            {files.map((entry, index) => (
              <button
                key={entry.path}
                type="button"
                role="tab"
                aria-selected={index === at}
                onClick={() => setAt(index)}
                className={cn("rounded-capsule flex h-7 shrink-0 items-center px-3 text-label", index === at ? "bg-foreground/10" : "text-foreground/65")}
              >
                {entry.name}
              </button>
            ))}
          </div>
        )}
        {staleCount > 0 && (
          <Banner
            tone="warning"
            text={`${staleCount === 1 && files?.length === 1 ? "SKILL.md" : "A SKILL.md"} changed on disk after you opened it, so Save didn't write over it. Reload to see it as it is now; your edits here are dropped.`}
          >
            <Button variant="soft" size="sm" onClick={reload}>Reload</Button>
          </Banner>
        )}
        {file && file.readOnly && item.kind === "skill" && (
          <p data-slot="read-only-reason" className="text-muted-foreground border-border shrink-0 border-b px-4 py-1.5 text-xs">{file.readOnly}</p>
        )}
        {failed ? (
          <p role="alert" className="text-destructive p-4 text-sm">{failed}</p>
        ) : !file ? (
          <Spinner aria-label="Opening" className="text-muted-foreground size-icon-md m-auto animate-spin" />
        ) : (
          <CodeEditor
            key={file.path}
            autoFocus
            value={draft}
            language={languageOf(file.shown)}
            label={create ? "What to add" : item.kind === "skill" ? `${file.name}'s SKILL.md` : `${item.name}'s settings`}
            {...(editable
              ? {
                  onChange: (text: string) => {
                    if (create) { create.onChange(text); return; }
                    setDrafts((now) => ({ ...now, [file.path]: text }));
                    setSaved(false);
                  },
                  onSave: () => { if (create) { if (dirty && !busy && !create.disabled) create.onSave(); } else store(); },
                }
              : {})}
          />
        )}
        {anyEditable && (
          <footer className="border-border flex shrink-0 items-center gap-3 border-t px-4 py-3">
            <span role={error ? "alert" : "status"} className={cn("min-w-0 flex-1 truncate text-xs", error ? "text-destructive" : "text-muted-foreground")}>{status}</span>
            <Button variant="ghost" size="sm" onClick={onClose}>Cancel</Button>
            <Button size="sm" disabled={!dirty || busy || staleCount > 0 || create?.disabled} onClick={() => create ? create.onSave() : store(onClose)}>{busy ? create ? "Checking…" : "Saving…" : "Save"}</Button>
          </footer>
        )}
      </DialogContent>
    </Dialog>
  );
}
