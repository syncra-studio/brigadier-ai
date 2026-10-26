import { createContext } from "react";

export type Visuals = { running: boolean; prefill: (text: string) => void; openLink: (url: string) => void };
export const VisualsContext = createContext<Visuals | null>(null);

export function canDrawVisuals(conversation: { kind: string; sideOf?: string | null; tabChat?: unknown } | null | undefined, embedded: boolean): boolean {
  return conversation?.kind === "chat" && conversation.sideOf == null && conversation.tabChat == null && !embedded;
}

export type FrameMessage = { type: "ready" } | { type: "resize"; height: number } | { type: "sendPrompt"; text: string } | { type: "openLink"; url: string };

/** Only messages on the port transferred to this frame reach this reader. */
export function readFrameMessage(value: unknown): FrameMessage | null {
  if (!value || typeof value !== "object") return null;
  const data = value as Record<string, unknown>;
  if (data.type === "ready") return { type: "ready" };
  if (data.type === "resize" && typeof data.height === "number" && Number.isFinite(data.height)) {
    return { type: "resize", height: Math.max(50, Math.min(4000, data.height)) };
  }
  if (data.type === "sendPrompt" && typeof data.text === "string" && data.text.trim().length > 0 && data.text.length <= 4000) {
    return { type: "sendPrompt", text: data.text };
  }
  if (data.type === "openLink" && typeof data.url === "string" && data.url.length <= 4000) {
    try {
      if (new URL(data.url).protocol === "https:") return { type: "openLink", url: data.url };
    } catch { /* Invalid addresses are ignored. */ }
  }
  return null;
}

/** Messages from the window or an old frame cannot use a live frame's bridge. */
export function receiveFrameMessage(source: MessagePort, own: MessagePort | null, value: unknown): FrameMessage | null {
  return source === own ? readFrameMessage(value) : null;
}
