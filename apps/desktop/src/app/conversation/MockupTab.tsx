import { type CSSProperties, type PointerEvent as ReactPointerEvent, type ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { type ImageState, type LoadedImage, loadImage, markedScreenshot, useStoredImage } from "@/app/conversation/mockupImages";
import { Button, type Choice, Segmented } from "@/components/controls";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, Popover, PopoverContent, PopoverTrigger, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/overlays";
import { ArrowUp, Check, ChevronDown, Comment, Grid, ImageSquare, SquareImage, Trash, X } from "@/components/icons";
import { EmptyState } from "@/components/shared";
import type { ArtifactRef } from "@/ipc/generated";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { addAttachment, send } from "@/state/actions";
import { useBoard } from "@/state/board";
import { earlierShots, type Mockup, mockupOf, revisedSince, sentShots, type Shot, shotsOfMockup, takesNotes } from "@/state/mockup";
import { changeNotes, markedName, markerNumber, mockupNotesOf, type MockupNote, NO_NOTES, notePlacement, notesByScreen, notesMessage, pointIn, sendEnded, takeNotes, useSessionNotes } from "@/state/mockupNotes";
import { useApp } from "@/state/store";

/** The zoom menu's steps. */
const ZOOMS = [25, 50, 100, 150, 200] as const;
const MIN_ZOOM = 10;
const MAX_ZOOM = 400;
/** A press that moves less than this is a click (it drops a marker), not a drag. */
const CLICK_SLOP = 8;

type Zoom = "fit" | number;
type View = "focused" | "canvas";

/** One image the tab can show: a mockup screenshot, its live capture or the side-by-side. */
type Item = { key: string; screen: number; kind: "mockup" | "live" | "compare"; ref: ArtifactRef };

const KIND_LABEL = { mockup: "Mockup", live: "Live", compare: "Side by side" } as const;

function itemsOf(mockup: Mockup, shown: readonly Shot[]): Item[] {
  // `shown` is the mockup's screenshots, or the earlier version's: its screens may differ.
  const numbers = [...new Set([...mockup.pairs.map((pair) => pair.number), ...shown.map((shot) => shot.screen)])].toSorted((a, b) => a - b);
  return numbers.flatMap((number) => {
    const pair = mockup.pairs.find((each) => each.number === number);
    const own = shown.find((shot) => shot.screen === number)?.ref ?? null;
    return ([["mockup", own], ["live", pair?.live ?? null], ["compare", pair?.compare ?? null]] as const)
      .flatMap(([kind, ref]) => ref ? [{ key: `${kind}-${number}`, screen: number, kind, ref }] : []);
  });
}

const nameOf = (ref: ArtifactRef) => ref.fileName ?? ref.title;

const STATUS_WORDS: Record<Mockup["status"], string> = {
  building: "Building", ready: "Ready", stopped: "Stopped", matching: "Being built for real", compared: "Compared with the app",
};

/** A capsule floating over the stage, holding its controls. */
const CAPSULE = "bg-popover/90 text-popover-foreground shadow-mockup-control rounded-capsule h-mockup-control pointer-events-auto flex shrink-0 items-center backdrop-blur-xl text-sm";

/**
 * The session's mockup (SESSION-FIXES-PLAN.md Q10): its screenshots to look at and mark with
 * notes, sent in one batch to the worker that made them; then, once the real work is built,
 * the live captures beside them. It is the page of the session's Mockup tab.
 */
export function MockupView({ conversationId, active }: { conversationId: string; active: boolean }) {
  const tasks = useBoard((s) => (s.board?.conversationId === conversationId ? s.board.tasks : null));
  const mockup = useMemo(() => (tasks ? mockupOf(tasks) : null), [tasks]);
  const stored = useSessionNotes(conversationId);
  // Notes belong to one mockup: a newer mockup in the session starts without them.
  const mine = !mockup || stored.task === null || stored.task === mockup.task.id;
  const { notes, commenting, sent } = mine ? stored : NO_NOTES;
  const taskId = mockup?.task.id ?? null;
  useEffect(() => {
    if (taskId) changeNotes(conversationId, (current) => (current.task === null || current.task === taskId ? { task: taskId } : { ...NO_NOTES, task: taskId }));
  }, [conversationId, taskId]);
  // After notes go, the screenshots they were made on stay until the user moves to the revision.
  const revised = Boolean(sent && mockup && revisedSince(mockup, sent));
  const shown = useMemo(() => {
    if (!mockup) return [];
    return sent && revised ? earlierShots(mockup, sent) : shotsOfMockup(mockup);
  }, [mockup, sent, revised]);
  const items = useMemo(() => (mockup ? itemsOf(mockup, shown) : []), [mockup, shown]);
  // Compared with the live app, it opens on every pair side by side.
  const [view, setView] = useState<View>(() => (mockup?.status === "compared" ? "canvas" : "focused"));
  // The comparison arriving while the tab is open shows it too.
  const compared = mockup?.status === "compared";
  const [wasCompared, setWasCompared] = useState(compared);
  if (compared !== wasCompared) {
    setWasCompared(compared);
    if (compared) setView("canvas");
  }
  const [picked, setPicked] = useState<string | null>(null);
  const [zoom, setZoom] = useState<Zoom>("fit");
  const [fitted, setFitted] = useState(100);
  const current = items.find((item) => item.key === picked) ?? items[0] ?? null;
  const canComment = Boolean(mockup && shown.length && takesNotes(mockup) && !sent);

  // A dismissed revision shows the next time the mockup opens: when its tab is back in front,
  // since a tab behind another stays mounted, or when it mounts again.
  const dismissed = Boolean(sent?.dismissed);
  useEffect(() => {
    if (!active && dismissed) changeNotes(conversationId, () => ({ sent: null }));
  }, [conversationId, active, dismissed]);
  useEffect(() => () => {
    if (mockupNotesOf(conversationId).sent?.dismissed) changeNotes(conversationId, () => ({ sent: null }));
  }, [conversationId]);

  const { sendNotes, sending, failed } = useSendNotes(conversationId, mockup, items);
  // Notes are frozen while they go, so none is made or changed after the send took them.
  const editable = commenting && !sending;
  useEffect(() => {
    // Only the tab in front sends: one behind another stays mounted, its notes unseen.
    if (!commenting || !active) return;
    const onKey = (event: KeyboardEvent) => {
      const typing = event.target instanceof HTMLElement && event.target.closest("input, textarea, [contenteditable]");
      if (event.key === "Enter" && (event.metaKey || event.ctrlKey) && !typing) {
        event.preventDefault();
        sendNotes();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [commenting, active, sendNotes]);

  if (!mockup) {
    return <Centered title="No mockup in this session yet." />;
  }
  const shownZoom = zoom === "fit" ? fitted : zoom;
  return (
    <TooltipProvider delayDuration={250}>
      <section data-slot="mockup" aria-label={`Mockup: ${mockup.task.title}`} className="bg-mockup-stage relative flex min-h-0 flex-1 flex-col overflow-hidden">
        <header className="h-mockup-header pointer-events-none absolute inset-x-0 top-0 z-10 flex items-center gap-1.5 p-2">
          {items.length > 1 && <ViewToggle view={view} onChange={setView} />}
          <FilePill mockup={mockup} version={sent && revised ? sent.version : mockup.version} items={items} current={view === "focused" ? current : null} onPick={(key) => { setPicked(key); setView("focused"); }} />
          <div className="min-w-0 flex-1" />
          {items.length > 0 && <ZoomMenu zoom={zoom} percent={shownZoom} onChange={setZoom} />}
        </header>
        {items.length === 0 ? (
          mockup.status === "building"
            ? <Centered title={<span className="shimmer">Building the mockup…</span>} detail="Its screenshots show here when its worker reports." />
            : <Centered title="The mockup stopped before it had screenshots." />
        ) : view === "canvas" ? (
          <Canvas conversationId={conversationId} items={items} zoom={zoom === "fit" ? 100 : zoom} commenting={editable}
            onZoom={setZoom} onFocus={(key) => { setPicked(key); setView("focused"); }} />
        ) : current && (
          <Focused conversationId={conversationId} item={current} zoom={zoom} commenting={editable && current.kind === "mockup"}
            onFitted={setFitted} onZoom={setZoom} />
        )}
        <Toolbar conversationId={conversationId} notes={notes} commenting={commenting} canComment={canComment}
          working={Boolean(sent && !revised)} revised={revised && !sent?.dismissed} sending={sending} failed={failed} onSend={sendNotes} />
      </section>
    </TooltipProvider>
  );
}

function Centered({ title, detail }: { title: ReactNode; detail?: ReactNode }) {
  return <EmptyState variant="centered" as="div" title={title} detail={detail} className="flex-1 p-6" />;
}

function Tip({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

const VIEWS: readonly Choice<View>[] = [
  { value: "focused", label: <SquareImage className="size-icon-sm" />, tip: "Focused view" },
  { value: "canvas", label: <Grid className="size-icon-sm" />, tip: "Canvas view" },
];

function ViewToggle({ view, onChange }: { view: View; onChange: (view: View) => void }) {
  return (
    <div className={cn(CAPSULE, "p-0.5")}>
      <Segmented label="Image view" value={view} options={VIEWS} onChange={onChange} />
    </div>
  );
}

/**
 * The mockup's name, the version of the screenshots shown (an earlier one while notes'
 * screenshots stay up) and its state; its menu lists every image to show.
 */
function FilePill({ mockup, version, items, current, onPick }: {
  mockup: Mockup; version: number; items: Item[]; current: Item | null; onPick: (key: string) => void;
}) {
  return (
    <DropdownMenu>
      <Tip label={mockup.task.title}>
        <DropdownMenuTrigger asChild>
          <button type="button" aria-label={`File actions for ${mockup.task.title}`} className={cn(CAPSULE, "hover:bg-popover min-w-0 shrink gap-2 ps-3 pe-1.5")}>
            <ImageSquare className="size-icon-sm shrink-0" />
            <span className="max-w-mockup-pill-text min-w-0 truncate">
              {current ? `${KIND_LABEL[current.kind]} · Screen ${current.screen}` : "Mockup"}
            </span>
            <span className="text-muted-foreground min-w-0 shrink-[2] truncate">v{version} · {STATUS_WORDS[mockup.status]}</span>
            <ChevronDown className="text-muted-foreground size-icon-xs shrink-0" />
          </button>
        </DropdownMenuTrigger>
      </Tip>
      <DropdownMenuContent align="start">
        {items.map((item) => (
          <DropdownMenuItem key={item.key} onSelect={() => onPick(item.key)}>
            <ImageSquare className="size-icon-sm" />
            <span className="flex-1 truncate">{nameOf(item.ref)}</span>
            <Check className={cn("size-icon-sm", item.key !== current?.key && "invisible")} />
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ZoomMenu({ zoom, percent, onChange }: { zoom: Zoom; percent: number; onChange: (zoom: Zoom) => void }) {
  const row = (label: string, chosen: boolean, value: Zoom) => (
    <DropdownMenuItem key={label} onSelect={() => onChange(value)}>
      <span className="flex-1">{label}</span>
      <Check className={cn("size-icon-sm", !chosen && "invisible")} />
    </DropdownMenuItem>
  );
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" aria-label={`Current zoom ${percent}%`} className={cn(CAPSULE, "hover:bg-popover gap-1 ps-2 pe-1 tabular-nums")}>
          {percent}%<ChevronDown className="text-muted-foreground size-icon-xs" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-mockup-menu">
        {ZOOMS.map((step) => row(`${step}%`, zoom === step, step))}
        <DropdownMenuSeparator />
        {row("Zoom to fit", zoom === "fit", "fit")}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Pinch or ⌘-scroll zooms the image stage. */
function useWheelZoom(element: HTMLElement | null, percent: number, onZoom: (zoom: Zoom) => void) {
  useEffect(() => {
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return;
      event.preventDefault();
      onZoom(Math.round(Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, percent * Math.exp(-event.deltaY * 0.01)))));
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [element, percent, onZoom]);
}

/** One image, centred under the header, fitted (never enlarged) or at a chosen zoom. */
function Focused({ conversationId, item, zoom, commenting, onFitted, onZoom }: {
  conversationId: string; item: Item; zoom: Zoom; commenting: boolean; onFitted: (percent: number) => void; onZoom: (zoom: Zoom) => void;
}) {
  const image = useStoredImage(item.ref.id);
  const [stage, setStage] = useState<HTMLDivElement | null>(null);
  const [room, setRoom] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    if (!stage) return;
    const measure = () => setRoom({ width: stage.clientWidth, height: stage.clientHeight - tokenPx("--spacing-mockup-header") });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(stage);
    return () => observer.disconnect();
  }, [stage]);
  const ready = image.status === "ready" ? image : null;
  const fit = ready && room.width > 0 ? Math.min(1, room.width / ready.width, room.height / ready.height) : 1;
  const scale = zoom === "fit" ? fit : zoom / 100;
  useEffect(() => { onFitted(Math.round(fit * 100)); }, [fit, onFitted]);
  useWheelZoom(stage, Math.round(scale * 100), onZoom);
  const pan = usePan(!commenting && zoom !== "fit");
  return (
    <div ref={setStage} {...pan} className={cn("pt-mockup-header flex min-h-0 flex-1 overflow-auto", pan.className)}>
      <div className="m-auto flex">
        <Picture image={image} label={`${KIND_LABEL[item.kind]}, screen ${item.screen}`}>
          {ready && (
            <Surface conversationId={conversationId} screen={item.kind === "mockup" ? item.screen : null} commenting={commenting}
              width={ready.width * scale} height={ready.height * scale} image={ready} />
          )}
        </Picture>
      </div>
    </div>
  );
}

/** Drag to pan a zoomed image, with a grab cursor. */
function usePan(enabled: boolean) {
  const from = useRef<{ x: number; y: number; left: number; top: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  if (!enabled) return { className: "" };
  return {
    className: dragging ? "cursor-grabbing" : "cursor-grab",
    onPointerDown: (event: ReactPointerEvent) => {
      if (event.button !== 0) return;
      const stage = event.currentTarget;
      from.current = { x: event.clientX, y: event.clientY, left: stage.scrollLeft, top: stage.scrollTop };
      event.currentTarget.setPointerCapture(event.pointerId);
      setDragging(true);
    },
    onPointerMove: (event: ReactPointerEvent) => {
      if (!from.current) return;
      const stage = event.currentTarget;
      stage.scrollLeft = from.current.left - (event.clientX - from.current.x);
      stage.scrollTop = from.current.top - (event.clientY - from.current.y);
    },
    onPointerUp: () => { from.current = null; setDragging(false); },
  };
}

/** An image's loading and failed states; its surface once it is ready. */
function Picture({ image, label, children }: { image: ImageState; label: string; children: ReactNode }) {
  if (image.status === "loading") return <div role="status" className="shimmer p-6 text-sm">Loading image…</div>;
  if (image.status === "error") {
    return (
      <div role="alert" aria-label={label} className="text-muted-foreground flex flex-col items-center gap-3 p-6 text-sm">
        <ImageSquare className="size-icon-lg" />
        Couldn’t load this preview
        <Button variant="outline" size="sm" onClick={image.retry}>Retry</Button>
      </div>
    );
  }
  return <>{children}</>;
}

/**
 * A screenshot at its displayed size, with its notes' markers. While commenting on a mockup
 * screen, a marker follows the pointer and a click drops a numbered one with a note box.
 */
function Surface({ conversationId, screen, commenting, width, height, image, className }: {
  conversationId: string; screen: number | null; commenting: boolean; width: number; height: number; image: LoadedImage; className?: string;
}) {
  const { notes } = useSessionNotes(conversationId);
  const own = screen === null ? [] : notes.filter((note) => note.screen === screen);
  const [draft, setDraft] = useState<{ x: number; y: number } | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [hover, setHover] = useState<{ x: number; y: number } | null>(null);
  const down = useRef<{ x: number; y: number } | null>(null);
  const active = commenting && screen !== null;
  if (!active && (draft || editing)) { setDraft(null); setEditing(null); }
  const size = { width, height };
  return (
    <div
      data-slot="mockup-surface"
      className={cn("relative shrink-0", active && "cursor-none", className)}
      style={{ width: `${width}px`, height: `${height}px` }}
      onPointerMove={active ? (event) => { const point = local(event); setHover(pointIn(point, point)); } : undefined}
      onPointerLeave={() => setHover(null)}
      onPointerDown={active ? (event) => { down.current = local(event); } : undefined}
      onPointerUp={active ? (event) => {
        const start = down.current;
        down.current = null;
        if (!start || event.target !== event.currentTarget) return;
        const end = local(event);
        if (Math.hypot(end.x - start.x, end.y - start.y) > CLICK_SLOP) return;
        setEditing(null);
        setDraft(pointIn(end, end));
      } : undefined}
    >
      <img src={image.url} alt="" draggable={false} className="shadow-mockup-image pointer-events-none size-full object-contain select-none" />
      {own.map((note) => (
        <Marker key={note.id} at={note} number={markerNumber(notes, note)} label={`Edit comment ${markerNumber(notes, note)}`}
          selected={editing === note.id} onClick={active ? () => { setDraft(null); setEditing(note.id); } : undefined} />
      ))}
      {draft && <Marker at={draft} number={own.length + 1} label="New comment" selected />}
      {active && hover && !draft && !editing && <Marker at={hover} label="" ghost />}
      {draft && screen !== null && (
        <NoteBox at={draft} surface={size} onCancel={() => setDraft(null)} onSave={(text) => {
          changeNotes(conversationId, (current) => ({ notes: [...current.notes, { id: crypto.randomUUID(), screen, ...draft, text }] }));
          setDraft(null);
        }} />
      )}
      {editing && (() => {
        const note = own.find((entry) => entry.id === editing);
        return note && (
          <NoteBox key={note.id} at={note} surface={size} initial={note.text} onCancel={() => setEditing(null)}
            onDelete={() => { changeNotes(conversationId, (current) => ({ notes: current.notes.filter((entry) => entry.id !== note.id) })); setEditing(null); }}
            onSave={(text) => { changeNotes(conversationId, (current) => ({ notes: current.notes.map((entry) => entry.id === note.id ? { ...entry, text } : entry) })); setEditing(null); }} />
        );
      })()}
    </div>
  );
}

/**
 * A pointer's place in the element it is over, and the element's size, both as drawn: under
 * Canvas view's CSS zoom they differ from its laid-out size, and only the two together give
 * the right fraction.
 */
function local(event: ReactPointerEvent) {
  const box = event.currentTarget.getBoundingClientRect();
  return { x: event.clientX - box.left, y: event.clientY - box.top, width: box.width, height: box.height };
}

const at = (point: { x: number; y: number }): CSSProperties => ({ left: `${point.x * 100}%`, top: `${point.y * 100}%` });

/** A note's marker: a numbered drop pointing at its place on the image. */
function Marker({ at: point, number, label, selected = false, ghost = false, onClick }: {
  at: { x: number; y: number }; number?: number; label: string; selected?: boolean; ghost?: boolean; onClick?: (() => void) | undefined;
}) {
  const className = cn(
    "rounded-marker bg-marker shadow-marker size-mockup-marker absolute flex -translate-x-1/2 -translate-y-1/2 items-center justify-center text-2xs leading-none font-bold text-foreground transition-transform duration-150 ease-enter motion-reduce:transition-none",
    selected && "scale-105", ghost && "pointer-events-none opacity-70", onClick && "hover:scale-105 cursor-pointer",
  );
  if (ghost) return <span aria-hidden className={className} style={at(point)} />;
  return onClick
    ? <button type="button" aria-label={label} className={className} style={at(point)} onPointerUp={(event) => event.stopPropagation()} onClick={onClick}>{number}</button>
    : <span aria-label={label} className={className} style={at(point)}>{number}</span>;
}

/**
 * The note box beside a marker: a one-line "Add a comment…" pill for a new note (Enter adds
 * it, Escape drops it), or a card to edit or delete a saved one (⌘↩ saves).
 */
function NoteBox({ at: point, surface, initial, onSave, onCancel, onDelete }: {
  at: { x: number; y: number }; surface: { width: number; height: number }; initial?: string;
  onSave: (text: string) => void; onCancel: () => void; onDelete?: () => void;
}) {
  const [text, setText] = useState(initial ?? "");
  const editing = initial !== undefined;
  const box = { width: tokenPx("--spacing-mockup-note"), height: tokenPx(editing ? "--spacing-mockup-note-edit" : "--spacing-mockup-note-input") };
  const gap = tokenPx("--spacing-mockup-marker") / 2 + tokenPx("--spacing-mockup-marker-gap");
  const place = notePlacement({ x: point.x * surface.width, y: point.y * surface.height }, box, surface, gap, tokenPx("--spacing") * 2);
  const style = { left: `${place.left}px`, top: `${place.top}px` };
  const save = () => { if (text.trim()) onSave(text.trim()); };
  const stop = { onPointerUp: (event: ReactPointerEvent) => event.stopPropagation(), onPointerDown: (event: ReactPointerEvent) => event.stopPropagation() };
  if (!editing) {
    return (
      <form {...stop} data-slot="mockup-note" style={style} onSubmit={(event) => { event.preventDefault(); save(); }}
        className="bg-popover shadow-mockup-control rounded-capsule w-mockup-note h-mockup-note-input absolute z-10 flex cursor-auto items-center gap-2 py-2 ps-4 pe-2 transition-[width,height,opacity] duration-150 ease-enter">
        <input autoFocus aria-label="Comment" placeholder="Add a comment…" value={text} onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => { if (event.key === "Escape") { event.preventDefault(); onCancel(); } }}
          className="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-sm outline-none" />
        {text.trim() && (
          <button type="submit" aria-label="Add comment" className="bg-foreground text-background rounded-capsule size-icon-button-sm flex shrink-0 items-center justify-center">
            <ArrowUp className="size-icon-sm" />
          </button>
        )}
      </form>
    );
  }
  return (
    <div {...stop} data-slot="mockup-note" style={style}
      className="bg-popover shadow-mockup-control rounded-xl w-mockup-note h-mockup-note-edit absolute z-10 flex cursor-auto flex-col overflow-hidden">
      <textarea autoFocus aria-label="Comment" value={text} onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) { event.preventDefault(); save(); }
          if (event.key === "Escape") { event.preventDefault(); onCancel(); }
        }}
        className="min-h-0 flex-1 resize-none bg-transparent px-4 pt-3 text-sm outline-none" />
      <div className="flex shrink-0 items-center gap-1.5 py-2 ps-2 pe-3">
        <Button variant="ghost" size="icon-sm" aria-label="Delete comment" onClick={onDelete}><Trash /></Button>
        <div className="flex-1" />
        <Button variant="outline" size="xs" className="rounded-capsule" onClick={onCancel}>Cancel</Button>
        <Button size="xs" className="rounded-capsule" disabled={!text.trim()} onClick={save}>Save</Button>
      </div>
    </div>
  );
}

/** Every image at once, a row per screen: the mockup, the live app, and the two side by side. */
function Canvas({ conversationId, items, zoom, commenting, onZoom, onFocus }: {
  conversationId: string; items: Item[]; zoom: number; commenting: boolean; onZoom: (zoom: Zoom) => void; onFocus: (key: string) => void;
}) {
  const [stage, setStage] = useState<HTMLDivElement | null>(null);
  useWheelZoom(stage, zoom, onZoom);
  const screens = [...new Set(items.map((item) => item.screen))];
  return (
    <div ref={setStage} className="pt-mockup-header min-h-0 flex-1 overflow-auto">
      <div className="flex min-w-max flex-col gap-6 px-3 pt-6 pb-24" style={{ zoom: zoom / 100 }}>
        {screens.map((screen) => (
          <div key={screen} className="flex flex-col gap-2">
            <span className="text-muted-foreground text-xs">Screen {screen}</span>
            <div className="flex items-start gap-2">
              {items.filter((item) => item.screen === screen).map((item) => (
                <Tile key={item.key} conversationId={conversationId} item={item} commenting={commenting && item.kind === "mockup"} onFocus={() => onFocus(item.key)} />
              ))}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function Tile({ conversationId, item, commenting, onFocus }: { conversationId: string; item: Item; commenting: boolean; onFocus: () => void }) {
  const image = useStoredImage(item.ref.id);
  const notes = useSessionNotes(conversationId).notes.filter((note) => item.kind === "mockup" && note.screen === item.screen);
  const label = `${KIND_LABEL[item.kind]}, screen ${item.screen}`;
  const height = image.status === "ready" ? Math.min(tokenPx("--spacing-mockup-tile"), image.height) : 0;
  const width = image.status === "ready" ? image.width * height / image.height : 0;
  return (
    <figure className="flex flex-col gap-1">
      <Picture image={image} label={label}>
        {image.status === "ready" && (commenting ? (
          <Surface conversationId={conversationId} screen={item.screen} commenting width={width} height={height} image={image}
            className={cn("rounded-xl overflow-visible", notes.length > 0 && "ring-marker ring-2")} />
        ) : (
          <button type="button" aria-label={`Show ${label}`} onClick={onFocus}
            className={cn("bg-mockup-stage/85 overflow-hidden rounded-xl transition-transform active:scale-[0.995]", notes.length > 0 && "ring-marker ring-2")}>
            <img src={image.url} alt="" style={{ width: `${width}px`, height: `${height}px` }} className="block" />
          </button>
        ))}
      </Picture>
      <figcaption className="text-muted-foreground text-xs">{KIND_LABEL[item.kind]}</figcaption>
    </figure>
  );
}

/**
 * The dark toolbar at the bottom: Comment; while commenting, the notes' count (hover lists
 * them), Send (⌘↩) and Cancel; after sending, Working; once revised, See latest changes.
 */
function Toolbar({ conversationId, notes, commenting, canComment, working, revised, sending, failed, onSend }: {
  conversationId: string; notes: MockupNote[]; commenting: boolean; canComment: boolean; working: boolean; revised: boolean;
  sending: boolean; failed: boolean; onSend: () => void;
}) {
  const mac = useApp((s) => s.info?.platform === "macos");
  const bar = "bg-popover text-popover-foreground shadow-mockup-toolbar h-mockup-toolbar pointer-events-auto flex items-center gap-1.5 rounded-xl p-0.5 text-sm";
  let content: ReactNode = null;
  if (revised) {
    content = (
      <div className="bg-marker text-foreground rounded-capsule pointer-events-auto flex h-7 items-center text-sm">
        <button type="button" className="h-full px-2" onClick={() => changeNotes(conversationId, () => ({ sent: null }))}>See latest changes</button>
        <span aria-hidden className="bg-foreground/20 h-full w-px" />
        <button type="button" aria-label="Dismiss latest changes" className="flex h-full w-6 items-center justify-center"
          onClick={() => changeNotes(conversationId, (current) => ({ sent: current.sent && { ...current.sent, dismissed: true } }))}>
          <X className="size-icon-sm" />
        </button>
      </div>
    );
  } else if (working) {
    content = <div role="status" className={cn(bar, "px-3")}><span className="shimmer">Working</span></div>;
  } else if (commenting) {
    content = (
      <div className={bar}>
        {notes.length ? <NotesList notes={notes} /> : <span className="text-muted-foreground px-2.5">Click on the image to add comments</span>}
        <Tip label={mac ? "Send ⌘↩" : "Send Ctrl+↩"}>
          <Button size="sm" className="rounded-capsule h-9" disabled={!notes.length || sending} onClick={onSend}>Send</Button>
        </Tip>
        <Button variant="ghost" size="icon" aria-label="Cancel" disabled={sending} onClick={() => changeNotes(conversationId, () => ({ notes: [], commenting: false }))}>
          <X />
        </Button>
      </div>
    );
  } else if (canComment) {
    content = (
      <div className={bar}>
        <Button variant="ghost" className="h-9 gap-1.5 rounded-xl px-2" onClick={() => changeNotes(conversationId, () => ({ commenting: true }))}>
          <Comment className="size-icon-sm" />Comment
        </Button>
      </div>
    );
  }
  if (!content) return null;
  return (
    <div role="toolbar" aria-label="Image editing tools" className="pointer-events-none absolute inset-x-4 bottom-4 z-10 flex flex-col items-center gap-2">
      {failed && <span role="alert" className="bg-popover text-destructive rounded-capsule px-3 py-1 text-xs">Could not send the comments. Try again.</span>}
      {content}
    </div>
  );
}

/** "{n} comments"; hovering lists them per image, as they will be sent. */
function NotesList({ notes }: { notes: MockupNote[] }) {
  const [open, setOpen] = useState(false);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button type="button" className="text-muted-foreground hover:text-foreground px-2.5" onPointerEnter={() => setOpen(true)} onPointerLeave={() => setOpen(false)}>
          {notes.length === 1 ? "1 comment" : `${notes.length} comments`}
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" className="w-mockup-list flex flex-col gap-3 p-3 text-sm" onOpenAutoFocus={(event) => event.preventDefault()}>
        {notesByScreen(notes).map(({ screen, notes: own }, index) => (
          <div key={screen} className="flex flex-col gap-2">
            <span className="font-medium">Image {index + 1}</span>
            <ol className="flex flex-col gap-2">
              {own.map((note, number) => (
                <li key={note.id} className="flex gap-2"><span className="text-muted-foreground">{number + 1}.</span>{note.text}</li>
              ))}
            </ol>
          </div>
        ))}
      </PopoverContent>
    </Popover>
  );
}

/**
 * Sends the notes as one message to the thread: its text names each marked screenshot,
 * attached with the markers drawn on it; the thread hands them to the mockup's worker.
 */
function useSendNotes(conversationId: string, mockup: Mockup | null, items: readonly Item[]) {
  // The send in flight is the session's, not this tab's: a tab closed and opened meanwhile
  // still has it, so the notes stay frozen and Send and ⌘↩ do nothing until it ends.
  const { sending, failed } = useSessionNotes(conversationId);
  const sendNotes = useCallback(() => {
    if (!mockup) return;
    const notes = takeNotes(conversationId);
    if (!notes) return;
    void (async () => {
      const files = await Promise.all(notesByScreen(notes).map(async ({ screen, notes: own }) => {
        const ref = items.find((item) => item.kind === "mockup" && item.screen === screen)?.ref;
        if (!ref) throw new Error(`no screenshot for screen ${screen}`);
        return markedScreenshot(await loadImage(ref.id), own, notes, markedName(screen));
      }));
      const attachments = await Promise.all(files.map((file) => addAttachment(file)));
      await send({ text: notesMessage(mockup.version, notes), attachments, mentions: [] }, undefined, { to: conversationId });
    })().then(
      () => sendEnded(conversationId, notes, { version: mockup.version, ...sentShots(mockup), dismissed: false }),
      () => sendEnded(conversationId, notes, null),
    );
  }, [conversationId, mockup, items]);
  return { sendNotes, sending, failed };
}
