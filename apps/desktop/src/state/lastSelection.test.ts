import assert from "node:assert/strict";
import { test } from "node:test";

import type { Conversation } from "@/ipc/generated";
import { LAST_SELECTION_KEY, rememberSelection, reopenable, savedSelection } from "@/state/lastSelection";

const conversation = (id: string, extra: Partial<Conversation> = {}) =>
  ({ id, title: id, lifecycle: "active", ...extra }) as unknown as Conversation;

test("a relaunch reopens the conversation that was open, while it exists and isn't archived (UI audit M2)", () => {
  const stored = new Map<string, string>();
  const previous = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => stored.get(key) ?? null,
      setItem: (key: string, value: string) => void stored.set(key, value),
      removeItem: (key: string) => void stored.delete(key),
    },
  });
  try {
    rememberSelection({ type: "conversation", id: "s1" });
    assert.equal(stored.get(LAST_SELECTION_KEY), "s1");
    // Settings keeps what it was opened from; the blank home forgets it.
    rememberSelection({ type: "settings", page: "general" });
    assert.equal(savedSelection(), "s1");
    const conversations = {
      s1: conversation("s1"),
      old: conversation("old", { lifecycle: "archived" }),
      side: conversation("side", { tabChat: {} as NonNullable<Conversation["tabChat"]> }),
      asleep: conversation("asleep", { lifecycle: "hibernated" }),
    };
    assert.equal(reopenable(savedSelection(), conversations), "s1");
    assert.equal(reopenable("asleep", conversations), "asleep");
    for (const gone of ["old", "side", "deleted", null]) assert.equal(reopenable(gone, conversations), null);
    rememberSelection({ type: "draft", kind: "chat" });
    assert.equal(savedSelection(), null);
  } finally {
    if (previous) Object.defineProperty(globalThis, "localStorage", previous);
    else Reflect.deleteProperty(globalThis, "localStorage");
  }
});
