import { Chat, Folder, Terminal } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { create } from "zustand";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { openConversation, select, setProjectExpanded } from "@/state/actions";
import { useApp } from "@/state/store";

type Result =
  | { kind: "project"; id: string; title: string; detail: string }
  | { kind: "session" | "chat"; id: string; title: string; detail: string };

const MAX_RESULTS = 50;

function useResults(query: string): Result[] {
  const projects = useApp((s) => s.projects);
  const conversations = useApp((s) => s.conversations);
  return useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    const matches = (title: string) =>
      needle === "" || title.toLocaleLowerCase().includes(needle);
    const projectResults: Result[] = Object.values(projects)
      .filter((project) => matches(project.name))
      .toSorted((a, b) => a.name.localeCompare(b.name))
      .map((project) => ({
        kind: "project",
        id: project.id,
        title: project.name,
        detail: "Project",
      }));
    // Archived conversations are listed in the Archived view only.
    const conversationResults: Result[] = Object.values(conversations)
      .filter(
        (conversation) =>
          conversation.lifecycle !== "archived" &&
          // Side chats are temporary, shown only beside their conversation.
          !conversation.sideOf &&
          matches(conversation.title),
      )
      .toSorted((a, b) => b.updatedAtMs - a.updatedAtMs)
      .map((conversation) => ({
        kind: conversation.kind,
        id: conversation.id,
        title: conversation.title,
        detail:
          conversation.kind === "session"
            ? (projects[conversation.projectId ?? ""]?.name ?? "Session")
            : "Chat",
      }));
    return [...projectResults, ...conversationResults].slice(0, MAX_RESULTS);
  }, [query, projects, conversations]);
}

const ICONS = { project: Folder, session: Terminal, chat: Chat } as const;

export function SearchDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent showCloseButton={false} className="max-w-lg gap-0 p-0">
        <DialogTitle className="sr-only">Search</DialogTitle>
        <DialogDescription className="sr-only">
          Find projects, sessions and chats by title.
        </DialogDescription>
        {open && <SearchBody onDone={() => onOpenChange(false)} />}
      </DialogContent>
    </Dialog>
  );
}

function SearchBody({ onDone }: { onDone: () => void }) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const results = useResults(query);
  const current = Math.min(active, Math.max(results.length - 1, 0));

  const choose = (result: Result | undefined) => {
    if (!result) return;
    if (result.kind === "project") {
      setProjectExpanded(result.id, true);
      select({ type: "draft", kind: "session", projectId: result.id });
    } else {
      openConversation(result.id);
    }
    onDone();
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
      <div className="border-b p-2">
        <Input
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
          className="bg-transparent focus-visible:bg-transparent border-none outline-none"
        />
      </div>
      <div
        id="search-results"
        role="listbox"
        aria-label="Results"
        className="max-h-(--container-sm) overflow-y-auto p-1"
      >
        {results.length === 0 && (
          <p className="text-muted-foreground px-3 py-6 text-center text-sm">
            {query.trim() ? `Nothing matches “${query.trim()}”.` : "Nothing here yet."}
          </p>
        )}
        {results.map((result, index) => {
          const Icon = ICONS[result.kind];
          return (
            <div
              key={`${result.kind}:${result.id}`}
              id={`search-${index}`}
              role="option"
              tabIndex={-1}
              aria-selected={index === current}
              onMouseMove={() => setActive(index)}
              onClick={() => choose(result)}
              onKeyDown={(event) => event.key === "Enter" && choose(result)}
              className={cn(
                "h-row rounded-control flex cursor-default items-center gap-2 px-2 text-sm",
                index === current && "bg-accent text-accent-foreground",
              )}
            >
              <Icon className="text-muted-foreground size-icon-md shrink-0" />
              <span className="min-w-0 flex-1 truncate">{result.title}</span>
              <span className="text-muted-foreground shrink-0 text-xs">
                {result.detail}
              </span>
            </div>
          );
        })}
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
