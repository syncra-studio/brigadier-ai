import { create } from "zustand";

/** A link-style button inside a toast ("View", "Undo"). */
export type ToastAction = { label: string; run: () => void };

export type Toast = {
  id: number;
  text: string;
  tone: "success" | "error";
  actions: ToastAction[];
};

/** Toasts on screen, oldest first. */
export const useToasts = create<{ toasts: Toast[] }>(() => ({ toasts: [] }));

let nextId = 0;

/**
 * Shows a toast at the top of the thread, confirming an action (e.g. "Archived chat"). Its id,
 * for taking it back.
 */
export function toast(
  text: string,
  options: { tone?: Toast["tone"]; actions?: ToastAction[] } = {},
): number {
  const entry: Toast = {
    id: nextId++,
    text,
    tone: options.tone ?? "success",
    actions: options.actions ?? [],
  };
  useToasts.setState((state) => ({ toasts: [...state.toasts, entry] }));
  return entry.id;
}

export function dismissToast(id: number): void {
  useToasts.setState((state) => ({ toasts: state.toasts.filter((entry) => entry.id !== id) }));
}
