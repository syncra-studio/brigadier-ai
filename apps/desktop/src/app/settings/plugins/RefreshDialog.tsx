import { useId, useState, type ReactNode } from "react";

import { useAction } from "@/app/conversation/useAction";
import { Button, Checkbox, ErrorLine } from "@/components/controls";
import { cn } from "@/lib/utils";
import type { PluginsAdapter } from "./adapter";
import { PluginDialog } from "./dialogs";
import { KIND_LABEL, listNames, shownDetail } from "./model";
import { PluginMark } from "./parts";
import type { PluginChanges } from "./types";

/** Why nothing is reported gone just now. */
export const unreadLine = (files: string[]) => `Couldn't read ${listNames(files)}, so nothing counts as removed for now. Fix it, then press Refresh again.`;

/** A heading, the one line that says what its choices mean, then its items. */
function Group({ title, line, children }: { title: string; line: string; children: ReactNode }) {
  return (
    <section aria-label={title} className="flex min-w-0 flex-col gap-2">
      <h3 className="text-label font-medium">{title}</h3>
      <p className="text-foreground/65 text-xs">{line}</p>
      <div className="flex min-w-0 flex-col gap-1.5">{children}</div>
    </section>
  );
}

/** `from` with `key` in it, or not. */
function toggled(from: ReadonlySet<string>, key: string, on: boolean) {
  const next = new Set(from);
  if (on) next.add(key); else next.delete(key);
  return next;
}

/** What Refresh found: what to add of the new, what to do about the gone. */
export function RefreshDialog({ changes, adapter, onClose, onDone }: {
  changes: PluginChanges;
  adapter: PluginsAdapter;
  /** Decides nothing: the same items are asked about at the next Refresh. */
  onClose: () => void;
  onDone: () => void;
}) {
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set());
  const [removing, setRemoving] = useState<ReadonlySet<string>>(new Set());
  const action = useAction();
  const id = useId();
  const apply = () => action.run(async () => {
    await adapter.applyChanges?.({
      add: changes.found.filter((item) => ticked.has(item.key)).map((item) => item.key),
      decline: changes.found.filter((item) => !ticked.has(item.key)).map((item) => item.key),
      remove: changes.gone.filter((item) => removing.has(item.key)).map((item) => item.key),
      keep: changes.gone.filter((item) => !removing.has(item.key)).map((item) => item.key),
    });
    onDone();
  });
  return (
    <PluginDialog slot="refresh" onClose={onClose} title="Changes found" description="Skills and MCP servers that came or went on this computer since you last chose.">
      {changes.found.length > 0 && (
        <Group title={`New (${changes.found.length})`} line="Tick what your agents should use. What you leave unticked stays out of Brigadier and isn't asked about again; you can bring it back from Add.">
          {changes.found.map((item) => (
            <label key={item.key} htmlFor={`${id}-${item.key}`} data-slot="refresh-new" className="bg-foreground/5 rounded-control flex cursor-pointer items-center gap-3 px-3 py-2">
              <Checkbox id={`${id}-${item.key}`} aria-label={`Add ${item.name}`} checked={ticked.has(item.key)} onCheckedChange={(on) => setTicked((now) => toggled(now, item.key, on === true))} />
              <PluginMark item={item} />
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="truncate text-label font-medium">{item.name}</span>
                <span className="text-foreground/65 truncate text-xs">{KIND_LABEL[item.kind]} in {item.foundIn}</span>
                <span className="text-foreground/65 truncate font-mono text-xs">{shownDetail(item.detail)}</span>
              </span>
            </label>
          ))}
        </Group>
      )}
      {changes.gone.length > 0 && (
        <Group title={`No longer on this computer (${changes.gone.length})`} line="Their files have left every agent. Remove forgets their on/off and project settings. Keep lists them as missing, and they come back with those settings if their files return.">
          {changes.gone.map((item) => (
            <div key={item.key} data-slot="refresh-gone" className="bg-foreground/5 rounded-control flex items-center gap-3 px-3 py-2">
              <span className="opacity-50"><PluginMark item={{ kind: item.kind, name: item.name, detail: "" }} /></span>
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="truncate text-label font-medium">{item.name}</span>
                <span className="text-foreground/65 truncate text-xs">{KIND_LABEL[item.kind]}</span>
              </span>
              <div role="radiogroup" aria-label={`What to do with ${item.name}`} className="flex shrink-0 items-center gap-1">
                {(["Keep", "Remove"] as const).map((label) => {
                  const chosen = removing.has(item.key) === (label === "Remove");
                  return <Button key={label} role="radio" aria-checked={chosen} variant={chosen ? "soft" : "ghost"} size="sm" className={cn(!chosen && "text-foreground/65")} onClick={() => setRemoving((now) => toggled(now, item.key, label === "Remove"))}>{label}</Button>;
                })}
              </div>
            </div>
          ))}
        </Group>
      )}
      {changes.unread.length > 0 && <p data-slot="refresh-unread" className="text-warning text-xs">{unreadLine(changes.unread)}</p>}
      <ErrorLine error={action.error} />
      <div className="flex justify-end gap-2">
        <Button variant="ghost" onClick={onClose}>Not now</Button>
        <Button disabled={action.busy} onClick={apply}>{action.busy ? "Applying…" : "Apply"}</Button>
      </div>
    </PluginDialog>
  );
}
