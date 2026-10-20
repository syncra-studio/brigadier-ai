import { useState } from "react";

import {
  GROUP_ITEMS,
  count,
  doneLine,
  outcome,
  review,
  selection,
  size,
  startTicked,
  summaryLine,
  tickState,
  type Group,
  type Kept,
} from "@/app/settings/storage/model";
import { RoundBadge, RowText } from "@/app/settings/storage/parts";
import { Check, ChevronDown, ChevronRight, ChevronUp, FolderOpen, Lock, Sparkles, Spinner } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { SettingsCard } from "@/components/ui/settings-card";
import { revealPath } from "@/ipc/client";
import type { CleanItem, CleanReport, StorageReport } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** Where Free up space stands. */
export type FreeUp =
  | { phase: "rest"; error?: string }
  | { phase: "review"; scanId: string; ticked: ReadonlySet<string>; error?: string }
  | { phase: "removing"; scanId: string; ids: string[]; bytes: number; done: number; total: number }
  | { phase: "done"; scanId: string; ids: string[]; cleaned: CleanReport };

/**
 * The Free up space card, in each of its states: what can go; the review right in the card
 * (groups to tick or untick, what is kept on purpose); removing, with how far it got; and done,
 * with what was removed.
 */
export function FreeUpCard({
  report,
  scanned,
  state,
  onState,
  onRemove,
}: {
  /** The scan the card shows (the one a clean came from while it runs). */
  report: StorageReport | null;
  /** What Brigadier uses now, by the latest scan. */
  scanned: StorageReport | null;
  state: FreeUp;
  onState: (state: FreeUp) => void;
  onRemove: (scanId: string, ids: string[], bytes: number) => void;
}) {
  if (state.phase === "removing") return <Removing state={state} />;
  if (state.phase === "done") {
    return <Done report={report} state={state} totalBytes={scanned && scanned.scanId !== state.scanId ? scanned.totalBytes : null} />;
  }
  if (state.phase === "review" && report && report.scanId === state.scanId) {
    return <Review report={report} state={state} onState={onState} onRemove={onRemove} />;
  }
  const canGo = report ? selection(report, startTicked(report)).bytes : 0;
  const pickable = report?.items.some((item) => item.selectable) ?? false;
  return (
    <SettingsCard inset split={false} data-free-up="rest">
      <div className="flex items-center gap-4 py-4">
        <RoundBadge>
          <Sparkles />
        </RoundBadge>
        <RowText
          title={!report ? "Looking…" : canGo > 0 || startTicked(report).size > 0 ? `${size(canGo)} can go` : "Nothing to free up"}
          line={
            state.phase === "rest" && state.error ? (
              <span className="text-destructive">{state.error}</span>
            ) : report ? (
              summaryLine(report)
            ) : (
              "Looking at what Brigadier keeps on this computer…"
            )
          }
        />
        <Button
          variant="soft"
          size="sm"
          disabled={!report || !pickable}
          onClick={() => report && onState({ phase: "review", scanId: report.scanId, ticked: startTicked(report) })}
        >
          Review
        </Button>
      </div>
    </SettingsCard>
  );
}

function Review({
  report,
  state,
  onState,
  onRemove,
}: {
  report: StorageReport;
  state: Extract<FreeUp, { phase: "review" }>;
  onState: (state: FreeUp) => void;
  onRemove: (scanId: string, ids: string[], bytes: number) => void;
}) {
  const { groups, kept, keptBytes } = review(report);
  const ticked = state.ticked;
  const picked = selection(report, ticked);
  const canGo = selection(report, startTicked(report)).bytes;
  const set = (next: Set<string>) => onState({ phase: "review", scanId: state.scanId, ticked: next });
  const cancel = () => onState({ phase: "rest" });
  return (
    <SettingsCard inset split={false} data-free-up="review">
      <div className="flex items-center gap-4 py-4">
        <RoundBadge>
          <Sparkles />
        </RoundBadge>
        <RowText title={`${size(canGo)} can go`} line="Untick anything you want to keep. Nothing goes until you confirm." />
        <button
          type="button"
          aria-label="Close the review"
          onClick={cancel}
          className="text-muted-foreground hover:text-foreground shrink-0"
        >
          <ChevronUp aria-hidden className="size-3.75" />
        </button>
      </div>
      <div className="flex flex-col ps-13">
        {groups.map((group) => (
          <GroupRows
            key={group.category}
            group={group}
            ticked={ticked}
            onTick={(ids, on) => {
              const next = new Set(ticked);
              for (const id of ids) {
                if (on) next.add(id);
                else next.delete(id);
              }
              set(next);
            }}
          />
        ))}
        {kept.length > 0 && <KeptRows kept={kept} bytes={keptBytes} />}
      </div>
      <div className="border-divider flex items-center gap-3 border-t pt-3.5 pb-4 ps-13">
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="text-foreground text-xs font-medium">{size(picked.bytes)} selected</span>
          <span className={cn("text-xs", state.error ? "text-destructive" : "text-muted-foreground")}>
            {state.error ?? `${picked.picked} of ${count(picked.of, "item", "items")} · nothing else is touched`}
          </span>
        </div>
        <Button variant="ghost" size="sm" onClick={cancel}>Cancel</Button>
        <Button
          size="sm"
          disabled={picked.picked === 0}
          onClick={() =>
            onRemove(
              report.scanId,
              report.items.filter((item) => item.selectable && ticked.has(item.id)).map((item) => item.id),
              picked.bytes,
            )
          }
        >
          {picked.bytes > 0 ? `Free up ${size(picked.bytes)}` : `Tidy up ${count(picked.picked, "item", "items")}`}
        </Button>
      </div>
    </SettingsCard>
  );
}

/** A group's row (its box, open or closed, name, how many, size) and, open, its items. */
function GroupRows({
  group,
  ticked,
  onTick,
}: {
  group: Group;
  ticked: ReadonlySet<string>;
  onTick: (ids: string[], on: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const [shown, setShown] = useState(GROUP_ITEMS);
  const state = tickState(group, ticked);
  const picked = group.items.filter((item) => ticked.has(item.id));
  const on = picked.length;
  const ids = group.items.map((item) => item.id);
  return (
    <div data-group={group.category}>
      <div className="border-divider flex h-11.5 items-center gap-3 border-t">
        <Checkbox
          checked={state === "all" ? true : state === "some" ? "indeterminate" : false}
          aria-label={group.title}
          onCheckedChange={() => onTick(ids, state !== "all")}
        />
        <OpenButton open={open} label={group.title} onToggle={() => setOpen(!open)} />
        <span className="text-foreground min-w-0 flex-1 truncate text-label font-medium">{group.title}</span>
        <span className="text-muted-foreground shrink-0 text-xs">
          {state === "some" ? `${on} of ${group.items.length}` : count(group.items.length, "item", "items")}
        </span>
        <span className="text-foreground w-18 shrink-0 text-right text-xs font-medium">{size(state === "some" ? picked.reduce((total, item) => total + item.bytes, 0) : group.bytes)}</span>
      </div>
      {open && (
        <div className="flex flex-col pb-2 ps-13.5">
          {group.items.slice(0, shown).map((item) => (
            <ItemRow key={item.id} item={item} on={ticked.has(item.id)} onToggle={() => onTick([item.id], !ticked.has(item.id))} />
          ))}
          {group.items.length > shown && (
            <div className="pt-2 pb-1 ps-7">
              <Button variant="link-blue" size="inline" onClick={() => setShown(group.items.length)}>
                Show {group.items.length - shown} more
                <ChevronDown aria-hidden />
              </Button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function OpenButton({ open, label, onToggle }: { open: boolean; label: string; onToggle: () => void }) {
  return (
    <button
      type="button"
      aria-expanded={open}
      aria-label={`${open ? "Close" : "Open"} ${label}`}
      onClick={onToggle}
      className="text-muted-foreground hover:text-foreground shrink-0"
    >
      {open ? <ChevronDown aria-hidden className="size-3.5" /> : <ChevronRight aria-hidden className="size-3.5" />}
    </button>
  );
}

/** One item: its box, what it is, why it can go and where, its size, and Reveal. */
function ItemRow({ item, on, onToggle }: { item: CleanItem; on: boolean; onToggle: () => void }) {
  return (
    <div data-item={item.id} className="flex gap-3 py-2.5">
      <div className="pt-0.5">
        <Checkbox checked={on} aria-label={item.label} onCheckedChange={onToggle} />
      </div>
      <div className={cn("flex min-w-0 flex-1 flex-col gap-0.75", !on && "opacity-55")}>
        <span className="text-foreground text-xs font-medium">{item.label}</span>
        {item.reason && <span className="text-foreground/65 text-xs">{item.reason}</span>}
        {item.path && (
          <span className="text-muted-foreground truncate font-mono text-2xs">{item.path}</span>
        )}
      </div>
      <span
        className={cn(
          "w-16 shrink-0 text-right text-xs",
          on ? "text-foreground" : "text-muted-foreground",
        )}
      >
        {size(item.bytes)}
      </span>
      <RevealIcon path={item.path} />
    </div>
  );
}

function RevealIcon({ path }: { path: string | null }) {
  if (!path) return <span aria-hidden className="w-3.5 shrink-0" />;
  return (
    <button
      type="button"
      aria-label="Reveal in Finder"
      onClick={() => void revealPath(path)}
      className="text-muted-foreground hover:text-foreground h-fit shrink-0"
    >
      <FolderOpen aria-hidden className="size-3.5" />
    </button>
  );
}

/** What is kept on purpose: never picked, with why each stays. */
function KeptRows({ kept, bytes }: { kept: Kept[]; bytes: number }) {
  const [open, setOpen] = useState(false);
  return (
    <div data-group="kept">
      <div className="border-divider flex h-11.5 items-center gap-3 border-t">
        <Lock aria-hidden className="text-muted-foreground size-3.5 shrink-0" />
        <OpenButton open={open} label="Kept on purpose" onToggle={() => setOpen(!open)} />
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="text-foreground/65 text-label font-medium">Kept on purpose</span>
          <span className="text-muted-foreground text-xs">Brigadier still needs these, so they never go</span>
        </div>
        <span className="text-muted-foreground shrink-0 text-xs">{count(kept.length, "thing", "things")}</span>
        <span className="text-foreground/65 w-18 shrink-0 text-right text-xs font-medium">{size(bytes)}</span>
      </div>
      {open && (
        <div className="flex flex-col pb-2 ps-13.5">
          {kept.map((line) => (
            <div key={line.key} className="flex gap-3 py-2.5 ps-7">
              <div className="flex min-w-0 flex-1 flex-col gap-0.75">
                <span className="text-foreground/65 text-xs font-medium">{line.label}</span>
                <span className="text-muted-foreground text-xs">{line.reason}</span>
              </div>
              <span className="text-muted-foreground w-16 shrink-0 text-right text-xs">{size(line.bytes)}</span>
              <RevealIcon path={line.path} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function Removing({ state }: { state: Extract<FreeUp, { phase: "removing" }> }) {
  const share = state.total > 0 ? Math.min(1, state.done / state.total) : 0;
  return (
    <SettingsCard inset split={false} data-free-up="removing">
      <div role="status" className="flex items-center gap-4 py-4">
        <RoundBadge>
          <Spinner className="animate-spin motion-reduce:animate-none" />
        </RoundBadge>
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <div className="flex items-baseline justify-between gap-4">
            <span className="text-foreground text-label font-medium">Freeing up {size(state.bytes)}…</span>
            <span className="text-muted-foreground text-xs">
              {state.done} of {count(state.total, "item", "items")}
            </span>
          </div>
          <div className="bg-muted h-1 w-full overflow-hidden rounded-xs">
            <div className="bg-foreground h-full rounded-xs transition-[width]" style={{ width: `${share * 100}%` }} />
          </div>
        </div>
      </div>
    </SettingsCard>
  );
}

function Done({
  report,
  state,
  totalBytes,
}: {
  report: StorageReport | null;
  state: Extract<FreeUp, { phase: "done" }>;
  totalBytes: number | null;
}) {
  const [open, setOpen] = useState(false);
  const { cleaned } = state;
  const went = report ? outcome(report, state.ids, cleaned) : { removed: [], stayed: [] };
  // What went to the Trash takes its room until the Trash is emptied: the line says it.
  const freed = cleaned.reclaimedBytes;
  return (
    <SettingsCard inset split={false} data-free-up="done">
      <div className="flex items-center gap-4 py-4">
        <RoundBadge tone="done">
          <Check />
        </RoundBadge>
        <RowText
          title={cleaned.removed > 0 ? `${size(freed)} freed` : "Nothing was removed"}
          line={doneLine(cleaned, totalBytes)}
        />
        <Button
          variant="link-blue"
          size="inline"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          What was removed
        {open ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />}
        </Button>
      </div>
      {open && (
        <div data-removed className="border-divider flex flex-col border-t py-2 ps-13">
          {went.removed.map((item) => (
            <div key={item.id} className="flex gap-3 py-2">
              <span className="text-foreground min-w-0 flex-1 truncate text-xs">{item.label}</span>
              <span className="text-foreground/65 w-16 shrink-0 text-right text-xs">{size(item.bytes)}</span>
            </div>
          ))}
          {went.stayed.map((stayed) => (
            <div key={stayed.label + stayed.error} className="flex flex-col gap-0.75 py-2">
              <span className="text-foreground text-xs">{stayed.label}</span>
              <span className="text-destructive text-xs">Left in place: {stayed.error}</span>
            </div>
          ))}
          {went.removed.length === 0 && went.stayed.length === 0 && (
            <span className="text-muted-foreground py-2 text-xs">Nothing.</span>
          )}
        </div>
      )}
    </SettingsCard>
  );
}
