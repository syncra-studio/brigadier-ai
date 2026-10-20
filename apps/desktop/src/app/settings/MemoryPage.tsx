import { useEffect, useState } from "react";

import { errorText } from "@/app/dialogs/fields";
import { SettingsButton, SettingsCard, SettingsPage, SettingsRow } from "@/app/settings/parts";
import { Brain } from "@/components/ui/icon";
import { Input } from "@/components/ui/input";
import { useNow } from "@/hooks/use-now";
import type { Node, Origin } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { editMemory, forgetMemory, listMemories } from "@/state/brain";
import { useApp } from "@/state/store";

/** What Settings search finds on the page. */
export const MEMORY_ROWS = {
  memories: {
    label: "Memories",
    description: "What Brigadier remembers about you, used by every chat and session",
  },
} as const;

const ORIGINS: Record<Origin, string> = {
  index: "the code index",
  skeleton: "a skeleton pass",
  enrichment: "an enrichment job",
  report: "a worker's report",
  orchestrator: "a session",
  user: "you",
};

/** Where a memory was learned: the conversation's title while it exists, else its origin. */
function Source({ memory }: { memory: Node }) {
  const { sessionId, origin } = memory.provenance;
  const conversation = useApp((s) => (sessionId ? s.conversations[sessionId] : undefined));
  if (conversation) {
    return (
      <>
        from the {conversation.kind === "chat" ? "chat" : "session"} “{conversation.title}”
      </>
    );
  }
  return <>from {ORIGINS[origin]}</>;
}

/** The memories as last read, kept while the app runs, so the page shows them at once. */
let lastMemories: Node[] | null = null;

/**
 * What Brigadier remembers about the user (where they live, their trade, their tools, what they
 * prefer), newest first: each one edited in place or deleted at once.
 */
export function MemoryPage() {
  const [memories, setMemories] = useState<Node[] | null>(() => lastMemories);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    lastMemories = memories;
  }, [memories]);
  useEffect(() => {
    let current = true;
    listMemories().then(
      (list) => current && setMemories(list),
      (cause: unknown) => current && setError(errorText(cause)),
    );
    return () => {
      current = false;
    };
  }, []);

  return (
    <SettingsPage
      title="Memory"
      description="What Brigadier remembers about you: where you live, your work, your tools, what you prefer. Every chat and session uses it. Passwords, card numbers, health and money details are never kept."
    >
      {error && <p className="text-destructive text-label">{error}</p>}
      {memories !== null && (
        <SettingsCard>
          {memories.length === 0 ? (
            <div
              data-setting={MEMORY_ROWS.memories.label}
              className="text-foreground/65 flex items-center gap-3 px-4 py-3 text-label"
            >
              <Brain aria-hidden className="size-icon-md shrink-0" />
              Nothing yet. Tell a chat about yourself (“I live in Lisbon”, “I’m an electrician”) and
              it shows up here.
            </div>
          ) : (
            memories.map((memory, index) => (
              <MemoryRow
                key={memory.id}
                memory={memory}
                setting={index === 0 ? MEMORY_ROWS.memories.label : undefined}
                onEdited={(edited) =>
                  setMemories((list) => list?.map((entry) => (entry.id === edited.id ? edited : entry)) ?? null)
                }
                onDeleted={() =>
                  setMemories((list) => list?.filter((entry) => entry.id !== memory.id) ?? null)
                }
              />
            ))
          )}
        </SettingsCard>
      )}
    </SettingsPage>
  );
}

/** One memory: its text, when and where it was learned, and Edit and Delete. */
function MemoryRow({
  memory,
  setting,
  onEdited,
  onDeleted,
}: {
  memory: Node;
  /** The name search and links find the page's list by, on its first row. */
  setting?: string | undefined;
  onEdited: (memory: Node) => void;
  onDeleted: () => void;
}) {
  const now = useNow(60_000);
  const [draft, setDraft] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    action()
      .catch((cause: unknown) => setError(errorText(cause)))
      .finally(() => setBusy(false));
  };
  const save = () => {
    if (draft === null) return;
    if (draft.trim() === memory.title) {
      setDraft(null);
      return;
    }
    run(async () => {
      onEdited(await editMemory(memory.id, draft));
      setDraft(null);
    });
  };

  if (draft !== null) {
    return (
      <SettingsRow
        label={
          <Input
            autoFocus
            value={draft}
            aria-label="Memory"
            maxLength={240}
            disabled={busy}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                save();
              } else if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                setDraft(null);
                setError(null);
              }
            }}
            className="border-input bg-popover h-control-md rounded-control text-label w-full px-2.5 font-normal"
          />
        }
        setting={setting ?? memory.title}
        error={error}
      >
        <SettingsButton
          disabled={busy}
          onClick={() => {
            setDraft(null);
            setError(null);
          }}
        >
          Cancel
        </SettingsButton>
        <SettingsButton disabled={busy || draft.trim() === ""} onClick={save}>
          Save
        </SettingsButton>
      </SettingsRow>
    );
  }
  return (
    <SettingsRow
      label={<span className="wrap-break-word">{memory.title}</span>}
      setting={setting ?? memory.title}
      description={
        <>
          {formatAgo(memory.updatedAtMs, now)} · <Source memory={memory} />
        </>
      }
      error={error}
    >
      <SettingsButton aria-label={`Edit “${memory.title}”`} disabled={busy} onClick={() => setDraft(memory.title)}>
        Edit
      </SettingsButton>
      <SettingsButton
        destructive
        aria-label={`Delete “${memory.title}”`}
        disabled={busy}
        onClick={() =>
          run(async () => {
            await forgetMemory(memory.id);
            onDeleted();
          })
        }
      >
        Delete
      </SettingsButton>
    </SettingsRow>
  );
}
