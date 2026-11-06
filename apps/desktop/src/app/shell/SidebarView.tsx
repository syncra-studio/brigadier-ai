import { useId, useMemo, useState, type ReactNode } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { Checkbox, Radio, RadioGroup } from "@/components/controls";
import { Plus, SettingsSlider } from "@/components/icons";
import { Popover, PopoverContent, PopoverTrigger, Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { ProjectBadge, RowAction } from "@/components/shared";
import type { Conversation, Project, SidebarMode } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { openAddProject } from "@/state/addProject";
import { editSettings, setSetting } from "@/state/settings";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

const saved = (write: Promise<unknown>) => void write.catch((error: unknown) => toast(errorText(error), { tone: "error" }));

/** What the View control leaves out of the list. */
export type ShowFilter = {
  /** The unticked projects that still exist. */
  hiddenProjects: ReadonlySet<string>;
  hideChats: boolean;
  /** Nothing is left out. */
  all: boolean;
};

export function useShowFilter(): ShowFilter {
  const hidden = useApp((s) => s.settings.sidebarHiddenProjects);
  const hideChats = useApp((s) => s.settings.sidebarHideChats);
  const projects = useApp((s) => s.projects);
  return useMemo(() => {
    const hiddenProjects = new Set(hidden.filter((id) => projects[id]));
    return { hiddenProjects, hideChats, all: hiddenProjects.size === 0 && !hideChats };
  }, [hidden, hideChats, projects]);
}

const isChat = (conversation: Conversation) => conversation.kind === "chat" || !conversation.projectId;

/** Whether the list shows `conversation`: a chat by the Chats tick, a session by its project's. */
export function shownBy(conversation: Conversation, show: ShowFilter): boolean {
  return isChat(conversation) ? !show.hideChats : !show.hiddenProjects.has(conversation.projectId!);
}

const GROUPINGS: { mode: SidebarMode; label: string }[] = [
  { mode: "inbox", label: "None" },
  { mode: "projects", label: "Project" },
];

const HEADING = "text-muted-foreground px-2 py-1.5 text-sm";
const ROW = "has-focus-visible:bg-foreground/8 hover:bg-foreground/8 h-control-sm flex items-center gap-2 rounded-2xl px-2 text-sm";
const RULE = "border-divider mx-2 my-1 border-t";

/** A line of the Show list: its tick, what it stands for, and how many it holds. */
function ShowRow({ name, checked, onChange, mark, count, none }: {
  name: string;
  checked: boolean;
  onChange: (on: boolean) => void;
  mark?: ReactNode;
  count: number;
  /** Said for a count of nothing, with the name dimmed; else the count itself. */
  none?: string;
}) {
  const empty = count === 0 && none !== undefined;
  return (
    <label data-slot="show-row" className={ROW}>
      <Checkbox aria-label={name} checked={checked} onCheckedChange={(next) => onChange(next === true)} />
      {mark}
      <span className={cn("mask-fade-end min-w-0 flex-1 overflow-hidden whitespace-nowrap", empty && "text-muted-foreground")}>{name}</span>
      <span data-slot="show-count" className="text-muted-foreground shrink-0 text-xs tabular-nums">{empty ? none : count}</span>
    </label>
  );
}

/**
 * The View control's button, on the header of Open (or of Projects when grouped), so it is there
 * whatever the list holds; the size of the + beside it, so both headers are the same. Beside it, how
 * many projects show, while anything is left out.
 */
export function ViewTrigger() {
  const projects = useApp((s) => Object.keys(s.projects).length);
  const show = useShowFilter();
  const shownProjects = projects - show.hiddenProjects.size;
  return (
    <>
      {!show.all && (
        <span data-slot="view-filtered" className="text-foreground/70 me-1 text-xs whitespace-nowrap">
          {shownProjects === 1 ? "1 project" : `${shownProjects} projects`}
        </span>
      )}
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <RowAction
              aria-label="View"
              data-slot="sidebar-view"
              className={cn("size-icon-button-sm data-[state=open]:bg-foreground/8 data-[state=open]:text-foreground [&_svg]:size-icon-sm", !show.all && "bg-foreground/8 text-foreground")}
            >
              <SettingsSlider />
            </RowAction>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent side="bottom">View</TooltipContent>
      </Tooltip>
    </>
  );
}

/**
 * Around the sidebar's lists, whichever grouping shows: the View menu, with Group by None or Project,
 * and Show: All, each project, and Chats. It stays as it is while Group by moves its button to the
 * other list's header.
 */
export function ViewMenu({ projects, children }: { projects: readonly Project[]; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const mode = useApp((s) => (s.settings.sidebarMode === "projects" ? "projects" : "inbox"));
  const conversations = useApp((s) => s.conversations);
  const show = useShowFilter();
  const groupBy = useId();
  // What each line holds: live conversations, pinned ones too.
  const counts = useMemo(() => {
    const byProject: Record<string, number> = {};
    let chats = 0;
    for (const conversation of Object.values(conversations)) {
      if (conversation.lifecycle === "archived" || conversation.sideOf) continue;
      if (isChat(conversation)) chats += 1;
      else byProject[conversation.projectId!] = (byProject[conversation.projectId!] ?? 0) + 1;
    }
    return { byProject, chats };
  }, [conversations]);
  // All counts the lines ticked below it, projects and Chats.
  const ticked = projects.filter((project) => !show.hiddenProjects.has(project.id)).length + (show.hideChats ? 0 : 1);
  const tickProject = (id: string, on: boolean) => {
    const others = [...show.hiddenProjects].filter((each) => each !== id);
    saved(setSetting("sidebarHiddenProjects", on ? others : [...others, id]));
  };
  // With everything ticked it unticks everything; from anything else it ticks everything.
  const tickAll = () => {
    const hiddenProjects = show.all ? projects.map((project) => project.id) : [];
    const hideChats = show.all;
    saved(editSettings((settings) => ({ ...settings, sidebarHiddenProjects: hiddenProjects, sidebarHideChats: hideChats })));
  };
  return (
    <Popover open={open} onOpenChange={setOpen}>
      {children}
      <PopoverContent data-slot="sidebar-view-menu" side="bottom" align="end" className="w-60">
        <div id={groupBy} className={HEADING}>Group by</div>
        <RadioGroup
          aria-labelledby={groupBy}
          value={mode}
          onValueChange={(next) => saved(setSetting("sidebarMode", next as SidebarMode))}
        >
          {GROUPINGS.map(({ mode: value, label }) => (
            <label key={value} className={ROW}>
              <Radio value={value} />
              {label}
            </label>
          ))}
        </RadioGroup>
        <div className={RULE} />
        <div className={HEADING}>Show</div>
        <ShowRow name="All" checked={show.all} onChange={tickAll} count={ticked} />
        <div className={RULE} />
        <div data-slot="show-list" className="scroll-edge-fade max-h-60 overflow-y-auto">
          {projects.map((project) => (
            <ShowRow
              key={project.id}
              name={project.name}
              checked={!show.hiddenProjects.has(project.id)}
              onChange={(on) => tickProject(project.id, on)}
              mark={<ProjectBadge name={project.name} projectId={project.id} />}
              count={counts.byProject[project.id] ?? 0}
              none="No sessions"
            />
          ))}
          <ShowRow
            name="Chats"
            checked={!show.hideChats}
            onChange={(on) => saved(setSetting("sidebarHideChats", !on))}
            mark={<ProjectBadge name={null} projectId={null} />}
            count={counts.chats}
            none="No chats"
          />
        </div>
        <div className={RULE} />
        <button
          type="button"
          className={cn(ROW, "text-foreground/85 w-full text-start outline-hidden [&_svg]:size-icon-md")}
          onClick={() => {
            setOpen(false);
            openAddProject();
          }}
        >
          <Plus className="text-muted-foreground" />
          Add project…
        </button>
      </PopoverContent>
    </Popover>
  );
}
