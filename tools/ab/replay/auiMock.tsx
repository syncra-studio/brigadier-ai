// Stands in for @assistant-ui/react inside RequestBlock.tsx only. RequestBlock reads its block
// from assistant-ui's message state (`useAuiState`); the harness sets that state per frame to
// what ConversationView's convertMessage would give it.
import type { FC, ReactNode } from "react";

export type MockMessage = {
  id: string;
  metadata: { custom: Record<string, unknown>; submittedFeedback?: { type: string } };
  status: unknown;
  parts: { type: "text"; text: string }[];
  isLast: boolean;
};

let current: MockMessage | null = null;
export function setMessage(message: MockMessage): void {
  current = message;
}

export type TextMessagePartProps = { type: "text"; text: string };

export function useAuiState<T>(selector: (state: { message: MockMessage }) => T): T {
  if (!current) throw new Error("no message set");
  return selector({ message: current });
}

export function useAui() {
  return { message: () => ({ reload() {} }) };
}

const Pass: FC<{ children?: ReactNode; asChild?: boolean }> = ({ children }) => <>{children}</>;

export const MessagePrimitive = {
  Root: ({ children, ...props }: { children?: ReactNode } & Record<string, unknown>) => (
    <div {...props}>{children}</div>
  ),
  // The reply's text, plain (the real one renders it as Markdown through assistant-ui).
  PartByIndex: ({ index }: { index: number }) => (
    <div data-slot="replay-reply-text">{current?.parts[index]?.text ?? ""}</div>
  ),
};

export const ActionBarPrimitive = {
  Root: ({ children }: { children?: ReactNode }) => <div data-slot="answer-actions">{children}</div>,
  FeedbackPositive: Pass,
  FeedbackNegative: Pass,
  Reload: Pass,
};
