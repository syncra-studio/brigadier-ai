import { useAui, useThreadList, ContextArc, contextShare, type ComposerCommand, ComposerCommands } from "@/components/conversation";
import { type FC, useContext, useMemo, useState } from "react";

import { cliMenuCommands, conversationProvider } from "@/app/conversation/cliCommands";
import { useCliCommands } from "@/app/conversation/useCliCommands";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { ForkPlaces } from "@/app/conversation/ForkMenu";
import { COMPOSER_EDITABLE } from "@/app/conversation/composerTarget";
import { StatusCardContext } from "@/app/conversation/StatusCard";
import { useAction } from "@/app/conversation/useAction";
import { NameDialog } from "@/app/shell/NameDialog";
import { DropdownMenu, DropdownMenuTrigger } from "@/components/overlays";
import { findModel, type ModelGroup } from "@/components/shared";
import { PERMISSION_ICONS } from "@/app/shell/SetupPickers";
import {
  Archive,
  ArrowUpRight,
  Bolt,
  BranchAlt,
  ChatCompose,
  Document,
  InfoCircle,
  Lightbulb,
  Pencil,
  Pin,
  ProviderGlyph,
  SettingsSlider,
  Unpin,
} from "@/components/icons";
import type { Conversation, PermissionLevel, ProviderKind } from "@/ipc/generated";
import { PERMISSION_LABELS, useCanCompact } from "@/lib/setup";
import {
  archive,
  compact,
  forkConversation,
  renameConversation,
  select,
  setPinned,
  updateSetup,
} from "@/state/actions";
import { armBackgroundSend } from "@/state/backgroundSend";
import { useBoard } from "@/state/board";
import { isTaskTab } from "@/state/tabStates";
import { setDirectTo } from "@/state/directTo";

/** What `/init` asks the orchestrator, as Codex's `/init` asks its agent. */
const INIT_PROMPT =
  "Create an AGENTS.md file at the repository root with instructions for the coding agents " +
  "that work in this repository: how the project is laid out, how to build, run and test it, " +
  "its coding conventions, and what an agent must not do. Have the repository scouted first, " +
  "and keep the file short and specific to this project.";

/** The id of the newest answer on the shown branch, which `/fork` forks from. */
export function useLatestAnswer(): string | null {
  return useThreadList((s) => {
    for (let index = s.messages.length - 1; index >= 0; index--) {
      const custom = s.messages[index]?.metadata.custom as {
        block?: { answerId: string | null };
      };
      if (custom.block?.answerId) return custom.block.answerId;
    }
    return null;
  });
}

/**
 * The `/` menu in the composer, with the commands that mean something in Brigadier: Archive,
 * Compact (Chats whose CLI compacts), Fork, Init (sessions), Model, New, Permissions (where the
 * composer has its pill), Pin, Rename, Start as a new session (a project's sessions) and Status. A
 * draft has only Model and Permissions. Below them, under the CLI's name, what the CLI itself
 * says it runs from a message (its commands, skills and MCP prompts); picking one types it.
 */
export const SlashCommands: FC<{
  conversation: Conversation | null;
  /** The providers' models, for the Fast command of a model with a fast tier. */
  groups: readonly ModelGroup[];
  onOpenModel: () => void;
  /** The level the permission pill shows, and opening its menu; null where there is no pill. */
  permission?: { level: PermissionLevel; onOpen: () => void } | null;
  /** A draft's CLI, and the project and folder a new session would start in. */
  draft?: { provider: ProviderKind; projectId: string | null; repo: string | null } | null;
}> = ({ conversation, groups, onOpenModel, permission = null, draft = null }) => {
  const aui = useAui();
  const status = useContext(StatusCardContext);
  const compactable = useCanCompact(conversation?.setup);
  const share = useBoard((s) =>
    s.board && s.board.conversationId === conversation?.id ? contextShare(s.board.context) : null,
  );
  const answer = useLatestAnswer();
  const { run, error } = useAction();
  const [renaming, setRenaming] = useState(false);
  const [forking, setForking] = useState(false);
  const [listed, refreshListed] = useCliCommands(conversation ? { conversation } : draft);
  const provider = conversation ? conversationProvider(conversation) : (draft?.provider ?? null);

  const commands = useMemo<ComposerCommand[]>(() => {
    const model: ComposerCommand = {
      id: "model",
      label: "Model",
      icon: <SettingsSlider />,
      run: onOpenModel,
    };
    const permissions: ComposerCommand[] = permission
      ? [(() => {
          const Icon = PERMISSION_ICONS[permission.level];
          return {
            id: "permissions",
            label: "Permissions",
            description: `${PERMISSION_LABELS[permission.level]} · change how actions are approved`,
            icon: <Icon />,
            run: permission.onOpen,
          };
        })()]
      : [];
    if (!conversation) return [model, ...permissions];
    const id = conversation.id;
    const noun = conversation.kind === "chat" ? "chat" : "session";
    const pinned = conversation.pinnedAtMs !== null;
    const list: ComposerCommand[] = [
      {
        id: "archive",
        label: "Archive",
        description: `Archive the current ${noun}`,
        icon: <Archive />,
        run: () => run(() => archive(id)),
      },
      model,
      ...permissions,
      {
        id: "new",
        label: `New ${noun}`,
        description:
          conversation.kind === "chat"
            ? "Start a blank chat"
            : "Start a blank session in the same project",
        icon: <ChatCompose />,
        run: () =>
          select(
            conversation.kind === "session" && conversation.projectId
              ? { type: "draft", kind: "session", projectId: conversation.projectId }
              : { type: "draft", kind: "chat" },
          ),
      },
      {
        id: "pin",
        label: pinned ? `Unpin ${noun}` : `Pin ${noun}`,
        icon: pinned ? <Unpin /> : <Pin />,
        run: () => run(() => setPinned(id, !pinned)),
      },
      {
        id: "rename",
        label: "Rename",
        description: `Rename this ${noun}`,
        icon: <Pencil />,
        run: () => setRenaming(true),
      },
      {
        id: "status",
        label: "Status",
        description:
          conversation.kind === "chat"
            ? "Show chat ID, context usage, and rate limits"
            : "Show session ID and rate limits",
        icon: <InfoCircle />,
        run: () => status.setOpen(true),
      },
    ];
    if (answer) {
      list.push({
        id: "fork",
        label: `Fork ${noun}`,
        description:
          conversation.kind === "chat"
            ? "Fork this chat from its latest answer"
            : "Fork this session in the same folder or a new worktree",
        icon: <BranchAlt />,
        run: () =>
          conversation.kind === "chat"
            ? run(() => forkConversation(id, answer, "workspace"))
            : setForking(true),
      });
    }
    if (compactable && answer) {
      list.push({
        id: "compact",
        label: "Compact",
        description:
          share === null
            ? "Compact this chat's context"
            : `Compact this chat's context (${Math.round(share * 100)}% full)`,
        icon: <ContextArc share={share} className="size-icon-sm" />,
        run: () => run(() => compact(id)),
      });
    }
    const { setup } = conversation;
    const choice =
      setup?.type === "session" ? setup.orchestrator : setup?.type === "chat" ? setup.model : null;
    const fastTier = choice ? findModel(groups, choice)?.fast : null;
    if (setup && choice && fastTier) {
      const fast = choice.fast === true;
      const next = { ...choice, fast: !fast };
      list.push({
        id: "fast",
        label: "Fast",
        description: fast ? "Turn off fast mode" : fastTier,
        icon: <Bolt />,
        run: () =>
          run(() =>
            updateSetup(
              id,
              setup.type === "session"
                ? { ...setup, orchestrator: next }
                : { ...setup, model: next },
            ),
          ),
      });
    }
    if (setup?.type === "session") {
      list.push({
        id: "plan",
        label: "Plan mode",
        description: setup.planMode ? "Turn plan mode off" : "Turn plan mode on",
        icon: <Lightbulb />,
        run: () => run(() => updateSetup(id, { ...setup, planMode: !setup.planMode })),
      });
    }
    if (isTaskTab(conversation)) {
      list.push({
        id: "background",
        label: "Start as a new session",
        description: "Send the message as a new session in the background, and stay here",
        icon: <ArrowUpRight />,
        run: () => {
          setDirectTo(id, null);
          armBackgroundSend(id, true);
          requestAnimationFrame(() =>
            document.querySelector<HTMLElement>(COMPOSER_EDITABLE)?.focus(),
          );
        },
      });
    }
    if (conversation.kind === "session") {
      list.push({
        id: "init",
        label: "Init",
        description: "Create an AGENTS.md file with instructions for workers",
        icon: <Document />,
        run: () => {
          const composer = aui.composer();
          composer.setText(INIT_PROMPT);
          composer.send();
        },
      });
    }
    return list.toSorted((a, b) => a.label.localeCompare(b.label));
  }, [conversation, groups, answer, compactable, share, onOpenModel, permission, status, run, aui]);
  const menu = useMemo(
    () => [
      ...commands,
      ...cliMenuCommands(listed, commands, {
        name: provider ? PROVIDER_LABELS[provider] : undefined,
        icon: provider ? <ProviderGlyph provider={provider} className="size-icon-sm" /> : <Bolt />,
      }),
    ],
    [commands, listed, provider],
  );

  return (
    <>
      <ComposerCommands commands={menu} onOpen={refreshListed} />
      {error && (
        <p role="alert" className="text-destructive absolute bottom-full start-0 mb-2 text-xs">
          {error}
        </p>
      )}
      {conversation && answer && (
        <DropdownMenu open={forking} onOpenChange={setForking}>
          <DropdownMenuTrigger asChild>
            <span aria-hidden className="pointer-events-none absolute start-0 top-0" />
          </DropdownMenuTrigger>
          <ForkPlaces
            side="top"
            setup={conversation.setup}
            onPick={(place) => run(() => forkConversation(conversation.id, answer, place))}
          />
        </DropdownMenu>
      )}
      {conversation && (
        <NameDialog
          open={renaming}
          onOpenChange={setRenaming}
          title={conversation.kind === "chat" ? "Rename chat" : "Rename session"}
          label="Title"
          initialValue={conversation.title}
          confirmLabel="Rename"
          onSubmit={(title) => renameConversation(conversation.id, title)}
        />
      )}
    </>
  );
};
