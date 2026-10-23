import type { FormEvent } from "react";

import { KINDS, NODE_KINDS } from "@/app/inspector/brain/kinds";
import { Picker } from "@/app/inspector/providers/Picker";
import { Search, Spinner } from "@/components/icons";
import { Button, Input } from "@/components/controls";
import type { NodeKind } from "@/ipc/generated";
import { PLAIN_FIELD } from "@/lib/plainTyping";

export const ALL_KINDS = "all";

/** The Brain search's query: the words, the kinds to look in, and Search. */
export function QueryBar({
  text,
  onTextChange,
  kind,
  onKindChange,
  busy,
  onSubmit,
}: {
  text: string;
  onTextChange: (text: string) => void;
  kind: NodeKind | typeof ALL_KINDS;
  onKindChange: (kind: NodeKind | typeof ALL_KINDS) => void;
  busy: boolean;
  onSubmit: (event: FormEvent) => void;
}) {
  return (
    <form onSubmit={onSubmit} className="flex shrink-0 items-center gap-2 border-b px-3 py-2">
      <Input
        value={text}
        onChange={(event) => onTextChange(event.target.value)}
        placeholder="Ask the Brain, as the orchestrator would"
        aria-label="Brain query"
        {...PLAIN_FIELD}
        className="h-control-sm text-xs"
      />
      <Picker
        label="Kinds"
        value={kind}
        options={[
          { value: ALL_KINDS, label: "All kinds" },
          ...NODE_KINDS.map((value) => ({ value, label: KINDS[value].label })),
        ]}
        onChange={(value) => onKindChange(value as NodeKind | typeof ALL_KINDS)}
      />
      <Button type="submit" size="xs" disabled={busy || !text.trim()}>
        {busy ? <Spinner className="animate-spin" /> : <Search />}
        Search
      </Button>
    </form>
  );
}
