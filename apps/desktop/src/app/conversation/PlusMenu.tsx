import { ComposerPrimitive, useAui } from "@assistant-ui/react";
import { Chat, Folder, Lightbulb, Paperclip, Plus } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useCallback, useContext, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { WorkerGlyph } from "@/app/conversation/WorkerChip";
import {
  COMPOSER_EDITABLE,
  type ComposerTarget,
  ComposerTargetContext,
} from "@/app/conversation/composerTarget";
import { updateDraft } from "@/app/conversation/draftSetup";
import { useAction } from "@/app/conversation/useAction";
import { composerPill, floatingMenu } from "@/components/assistant-ui/elements/surfaces";
import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { Mention } from "@/ipc/generated";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import { select, updateSetup } from "@/state/actions";
import { useApp } from "@/state/store";

/** Other conversations the menu offers to mention, as the `@` menu shows them. */
const CHAT_ROWS = 5;

export const PLAN_PLACEHOLDER = "Describe your task to generate a plan...";

/**
 * Plan mode for a session or a session draft: the orchestrator plans and changes nothing
 * until the user approves a plan (the core enforces it; approving turns it off). A Chat
 * only talks, so there is nothing to hold back and it has no plan mode.
 */
export function usePlanMode(target: ComposerTarget | null): {
  available: boolean;
  on: boolean;
  set: (on: boolean) => Promise<void>;
} {
  const conversation = target?.conversation ?? null;
  const setup = conversation?.setup;
  if (conversation) {
    if (setup?.type !== "session" || conversation.lifecycle === "archived") {
      return { available: false, on: false, set: async () => {} };
    }
    return {
      available: true,
      on: setup.planMode,
      set: (on) => updateSetup(conversation.id, { ...setup, planMode: on }),
    };
  }
  const resolved = target?.resolved;
  if (resolved?.kind !== "session") return { available: false, on: false, set: async () => {} };
  return {
    available: true,
    on: resolved.draft.planMode,
    set: async (on) => updateDraft(resolved.project?.id ?? null, { planMode: on }),
  };
}

/** A section heading, in plain grey. */
const Section: FC<{ children: string }> = ({ children }) => (
  <DropdownMenuLabel className="font-sans text-sm font-normal tracking-normal normal-case">
    {children}
  </DropdownMenuLabel>
);

/** A row of the menu: icon, title, then grey detail. */
const Row: FC<{ icon: ReactNode; title: string; detail?: string | undefined }> = ({
  icon,
  title,
  detail,
}) => (
  <>
    <span className="text-muted-foreground flex size-icon-md shrink-0 items-center justify-center [&_svg]:size-icon-sm">
      {icon}
    </span>
    <span className="shrink-0">{title}</span>
    {detail && <span className="text-muted-foreground min-w-0 flex-1 truncate">{detail}</span>}
  </>
);

/** Where the menu goes: as wide as the composer card, just above it (over the rail). */
type Placement = { width: number; alignOffset: number; sideOffset: number };

/**
 * The `+` ("Add files and more"): the `@` list with an Add section first. Files (the native
 * picker), plan mode, and on a new chat "Work in a project"; then what `@` mentions without
 * typing: the session's workers and other conversations.
 */
export const PlusMenu: FC = () => {
  const target = useContext(ComposerTargetContext);
  const aui = useAui();
  const plan = usePlanMode(target);
  const action = useAction();
  const trigger = useRef<HTMLButtonElement>(null);
  const [placement, setPlacement] = useState<Placement | null>(null);
  const projects = useApp(
    useShallow((s) => Object.values(s.projects).toSorted((a, b) => a.name.localeCompare(b.name))),
  );
  const conversationId = target?.conversation?.id ?? null;
  const chats = useApp(
    useShallow((s) =>
      Object.values(s.conversations)
        .filter(
          (other) =>
            other.id !== conversationId && other.lifecycle !== "archived" && !other.sideOf,
        )
        .toSorted((a, b) => b.updatedAtMs - a.updatedAtMs)
        .slice(0, CHAT_ROWS),
    ),
  );
  // Opens with the first row highlighted, whatever opened it. Stable, so it runs once per
  // opening, not on every render (which would undo ↑/↓).
  const highlightFirst = useCallback((menu: HTMLDivElement | null) => {
    if (menu) requestAnimationFrame(() => menu.querySelector<HTMLElement>("[role=menuitem]")?.focus());
  }, []);
  if (!target) return null;
  const draftChat = !target.conversation && target.resolved.kind === "chat";

  const onOpenChange = (open: boolean) => {
    const button = trigger.current;
    // The menu sits on the card itself, covering the rail (hidden meanwhile).
    const composer =
      button?.closest<HTMLElement>("[data-slot=aui_composer-shell]") ??
      button?.closest<HTMLElement>("[data-slot=composer]");
    if (!open || !button || !composer) {
      setPlacement(null);
      return;
    }
    const from = button.getBoundingClientRect();
    const to = composer.getBoundingClientRect();
    setPlacement({
      width: to.width,
      alignOffset: to.left - from.left,
      sideOffset: from.top - to.top + tokenPx("--spacing"),
    });
  };

  const mention = (mentioned: Mention, name: string) => {
    if (mentioned.type !== "task") target.mentions.record(mentioned, name);
    const composer = aui.composer();
    const text = composer.getState().text;
    composer.setText(`${text}${text && !text.endsWith(" ") ? " " : ""}@${name} `);
  };

  return (
    <DropdownMenu open={placement !== null} onOpenChange={onOpenChange}>
      <DropdownMenuTrigger asChild>
        <TooltipIconButton
          ref={trigger}
          tooltip="Add files and more"
          side="bottom"
          size="icon-md"
          data-slot="plus-trigger"
          className="hover:bg-foreground/8 data-[state=open]:bg-foreground/8 rounded-capsule"
        >
          <Plus />
        </TooltipIconButton>
      </DropdownMenuTrigger>
      {placement && (
        <DropdownMenuContent
          ref={highlightFirst}
          data-slot="plus-menu"
          side="top"
          align="start"
          avoidCollisions={false}
          alignOffset={placement.alignOffset}
          sideOffset={placement.sideOffset}
          style={{ width: placement.width }}
          className={`${floatingMenu} max-h-command-list`}
          onCloseAutoFocus={(event) => {
            // Back to typing in this composer (a side chat has its own), as after picking
            // from the `@` menu.
            event.preventDefault();
            trigger.current
              ?.closest("[data-slot=composer]")
              ?.querySelector<HTMLElement>(COMPOSER_EDITABLE)
              ?.focus();
          }}
        >
          <Section>Add</Section>
          <ComposerPrimitive.AddAttachment asChild>
            <DropdownMenuItem>
              <Row icon={<Paperclip />} title="Files" />
            </DropdownMenuItem>
          </ComposerPrimitive.AddAttachment>
          {plan.available && (
            <DropdownMenuItem onSelect={() => action.run(() => plan.set(!plan.on))}>
              <Row
                icon={<Lightbulb />}
                title="Plan mode"
                detail={plan.on ? "Turn plan mode off" : "Turn plan mode on"}
              />
            </DropdownMenuItem>
          )}
          {draftChat && projects.length > 0 && (
            <DropdownMenuSub>
              <DropdownMenuSubTrigger>
                <Row icon={<Folder />} title="Work in a project" />
              </DropdownMenuSubTrigger>
              <DropdownMenuSubContent className={floatingMenu}>
                {projects.map((project) => (
                  <DropdownMenuItem
                    key={project.id}
                    onSelect={() => select({ type: "draft", kind: "session", projectId: project.id })}
                  >
                    <Row icon={<Folder />} title={project.name} />
                  </DropdownMenuItem>
                ))}
              </DropdownMenuSubContent>
            </DropdownMenuSub>
          )}
          {target.targets.length > 0 && <Section>Workers</Section>}
          {target.targets.map((worker) => (
            <DropdownMenuItem
              key={worker.id}
              onSelect={() => mention({ type: "task", id: worker.id }, `task-${worker.number}`)}
            >
              <Row icon={<WorkerGlyph taskId={worker.id} />} title={worker.title} />
            </DropdownMenuItem>
          ))}
          {chats.length > 0 && <Section>Chats</Section>}
          {chats.map((chat) => (
            <DropdownMenuItem
              key={chat.id}
              onSelect={() => mention({ type: "chat", id: chat.id, title: chat.title }, chat.title)}
            >
              <Row icon={<Chat />} title={chat.title} />
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      )}
    </DropdownMenu>
  );
};

/** The "💡 Plan" chip after the permission picker while plan mode is on. */
export const PlanChip: FC = () => {
  const target = useContext(ComposerTargetContext);
  const plan = usePlanMode(target);
  const action = useAction();
  if (!plan.available || !plan.on) return null;
  return (
    <>
      <span aria-hidden className="bg-foreground/15 mx-1 h-icon-md w-px" />
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-pressed
            className={cn(composerPill, "text-muted-foreground")}
            disabled={action.busy}
            onClick={() => action.run(() => plan.set(false))}
          >
            <Lightbulb />
            Plan
          </button>
        </TooltipTrigger>
        <TooltipContent side="bottom">Turn plan mode off</TooltipContent>
      </Tooltip>
    </>
  );
};
