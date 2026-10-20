import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { create } from "zustand";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/overlays/dialog";
import { Chat, Comment, Folder, Search, Terminal } from "@/components/icons";
import { MenuRow } from "@/components/overlays/dropdown-menu";
import { Kbd } from "@/components/controls/kbd";
import { cn } from "@/lib/utils";
import { openConversation, select, setProjectExpanded } from "@/state/actions";
import { openFeedback } from "@/state/feedback";
import { useApp } from "@/state/store";

type Result =
  | { kind: "project" | "session" | "chat"; id: string; title: string; detail: string | null }
  | { kind: "feedback"; id: "feedback"; title: string; detail: null };

const MAX_RESULTS = 50;

/** Send feedback… shows among the results once what is typed is part of its name. */
const FEEDBACK = "Send feedback…";

/** The groups, in order, with their labels. */
const GROUPS = [
  { kinds: ["project"], label: "Projects" },
  { kinds: ["session"], label: "Sessions" },
  { kinds: ["chat"], label: "Chats" },
  { kinds: ["feedback"], label: "Help" },
] as const satisfies readonly { kinds: readonly Result["kind"][]; label: string }[];

function useResults(query: string): Result[] {
  const projects = useApp((s) => s.projects);
  const conversations = useApp((s) => s.conversations);
  return useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    const matches = (title: string) =>
      needle === "" || title.toLocaleLowerCase().includes(needle);
    // Archived conversations are listed in the Archived view only; side chats are temporary,
    // shown only beside their conversation.
    const listed = Object.values(conversations).filter(
      (conversation) => conversation.lifecycle !== "archived" && !conversation.sideOf,
    );
    const sessionsIn = (projectId: string) =>
      listed.filter((conversation) => conversation.kind === "session" && conversation.projectId === projectId).length;
    const projectResults: Result[] = Object.values(projects)
      .filter((project) => matches(project.name))
      .toSorted((a, b) => a.name.localeCompare(b.name))
      .map((project) => {
        const count = sessionsIn(project.id);
        return {
          kind: "project",
          id: project.id,
          title: project.name,
          detail: `${count} ${count === 1 ? "session" : "sessions"}`,
        };
      });
    const conversationResults: Result[] = listed
      .filter((conversation) => matches(conversation.title))
      .toSorted((a, b) => b.updatedAtMs - a.updatedAtMs)
      .map((conversation) => ({
        kind: conversation.kind,
        id: conversation.id,
        title: conversation.title,
        detail: conversation.kind === "session" ? (projects[conversation.projectId ?? ""]?.name ?? null) : null,
      }));
    const found = [...projectResults, ...conversationResults].slice(0, MAX_RESULTS);
    // In the listed order, grouped: projects, sessions, chats.
    const grouped = GROUPS.flatMap(({ kinds }) => found.filter((result) => (kinds as readonly string[]).includes(result.kind)));
    const feedback: Result[] =
      needle !== "" && FEEDBACK.toLocaleLowerCase().includes(needle)
        ? [{ kind: "feedback", id: "feedback", title: FEEDBACK, detail: null }]
        : [];
    return [...grouped, ...feedback];
  }, [query, projects, conversations]);
}

const ICONS = { project: Folder, session: Terminal, chat: Chat, feedback: Comment } as const;

/**
 * Search, laid out to the user's design and drawn with the app's dialog, rows and keys: a palette at the top centre over projects, sessions and
 * chats by title, grouped, the selected one with Open ↵; Send feedback… when typed for.
 */
export function SearchDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  // What a chosen result opens next (Send feedback), once the palette has closed and handed the
  // keyboard on, so the closing palette doesn't take it back.
  const after = useRef<(() => void) | null>(null);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        data-slot="search-palette"
        showCloseButton={false}
        onCloseAutoFocus={(event) => {
          const next = after.current;
          after.current = null;
          if (!next) return;
          event.preventDefault();
          next();
        }}
        className="max-w-palette top-42 block translate-y-0 gap-0 p-0"
      >
        <DialogTitle className="sr-only">Search</DialogTitle>
        <DialogDescription className="sr-only">
          Find projects, sessions and chats by title.
        </DialogDescription>
        {open && (
          <SearchBody
            onDone={(next) => {
              after.current = next ?? null;
              onOpenChange(false);
            }}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function SearchBody({ onDone }: { onDone: (after?: () => void) => void }) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const results = useResults(query);
  const current = Math.min(active, Math.max(results.length - 1, 0));

  const choose = (result: Result | undefined) => {
    if (!result) return;
    if (result.kind === "feedback") {
      onDone(() => openFeedback());
      return;
    }
    onDone();
    if (result.kind === "project") {
      setProjectExpanded(result.id, true);
      select({ type: "draft", kind: "session", projectId: result.id });
    } else {
      openConversation(result.id);
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive(Math.min(current + 1, results.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive(Math.max(current - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      choose(results[current]);
    }
  };

  return (
    <div className="flex flex-col">
      <div className="border-divider flex h-13.5 items-center gap-3 border-b ps-4.5 pe-4">
        <Search className="text-muted-foreground size-4 shrink-0" />
        <input
          autoFocus
          value={query}
          placeholder="Search projects, sessions and chats"
          aria-label="Search"
          aria-controls="search-results"
          aria-activedescendant={results[current] ? `search-${current}` : undefined}
          onChange={(event) => {
            setQuery(event.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
          className="text-foreground placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-sm outline-none"
        />
        <Kbd>esc</Kbd>
      </div>
      <div
        id="search-results"
        role="listbox"
        aria-label="Results"
        className="flex max-h-(--container-sm) flex-col gap-1.5 overflow-y-auto px-2 pt-2 pb-2.5"
      >
        {results.length === 0 && (
          <p className="text-muted-foreground px-3 py-6 text-center text-sm">
            {query.trim() ? `Nothing matches “${query.trim()}”.` : "Nothing here yet."}
          </p>
        )}
        {GROUPS.map(({ kinds, label }) => {
          const group = results.flatMap((result, index) =>
            (kinds as readonly string[]).includes(result.kind) ? [{ result, index }] : [],
          );
          if (group.length === 0) return null;
          return (
            <div key={label} role="group" aria-label={label} className="flex flex-col gap-px">
              <span className="text-muted-foreground px-2.5 pt-2 pb-1.5 text-xs font-medium">
                {label}
              </span>
              {group.map(({ result, index }) => {
                const Icon = ICONS[result.kind];
                const selected = index === current;
                return (
                  <div
                    key={`${result.kind}:${result.id}`}
                    id={`search-${index}`}
                    role="option"
                    tabIndex={-1}
                    aria-selected={selected}
                    onMouseMove={() => setActive(index)}
                    onClick={() => choose(result)}
                    onKeyDown={(event) => event.key === "Enter" && choose(result)}
                    className={cn(
                      "h-row rounded-control flex cursor-default items-center gap-3 px-2.5 text-sm",
                      selected && "bg-accent text-accent-foreground",
                    )}
                  >
                    <MenuRow
                      icon={<Icon />}
                      title={result.title}
                      hint={
                        selected ? (
                          <>
                            Open
                            <Kbd>↵</Kbd>
                          </>
                        ) : (
                          result.detail
                        )
                      }
                    />
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
      <div className="border-divider text-muted-foreground flex h-10 items-center gap-4.5 border-t px-4.5 text-xs">
        <span className="flex items-center gap-1.5">
          <Kbd>↑</Kbd>
          <Kbd>↓</Kbd>
          Navigate
        </span>
        <span className="flex items-center gap-1.5">
          <Kbd>↵</Kbd>
          Open
        </span>
      </div>
    </div>
  );
}

const useSearchUi = create<{ open: boolean }>(() => ({ open: false }));

/** Opens search (⌘K does too). */
export function openSearch(): void {
  useSearchUi.setState({ open: true });
}

/** Search over projects and conversations, from anywhere: ⌘K (Ctrl+K) opens it. */
export function GlobalSearch() {
  const open = useSearchUi((s) => s.open);
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === "k" && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        openSearch();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
  return <SearchDialog open={open} onOpenChange={(next) => useSearchUi.setState({ open: next })} />;
}
