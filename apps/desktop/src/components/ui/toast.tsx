import { CheckCircle, InfoCircle, Warning, X } from "@openai/apps-sdk-ui/components/Icon";
import { Toast as ToastPrimitive } from "radix-ui";

import { cn } from "@/lib/utils";
import { dismissToast, type Toast, useToasts } from "@/state/toasts";

/** How long a toast stays when nobody hovers or focuses it. */
const DURATION_MS = 4000;

function ToastItem({ toast }: { toast: Toast }) {
  const Icon = toast.tone === "success" ? CheckCircle : toast.tone === "error" ? Warning : InfoCircle;
  return (
    <ToastPrimitive.Root
      data-slot="toast"
      data-tone={toast.tone}
      duration={DURATION_MS}
      onOpenChange={(open) => !open && dismissToast(toast.id)}
      className={cn(
        // A capsule for one line; a long error wraps inside the same rounded box.
        "bg-popover rounded-thread min-h-control-md flex max-w-lg items-center gap-2 border py-1 ps-3 pe-1.5 text-sm",
        "data-[state=open]:animate-in data-[state=open]:fade-in data-[state=open]:slide-in-from-top-2",
        "data-[state=closed]:animate-out data-[state=closed]:fade-out motion-reduce:animate-none",
        toast.tone === "success" && "border-success/30 text-success",
        toast.tone === "error" && "border-destructive/30 text-destructive",
        toast.tone === "neutral" && "text-foreground",
      )}
    >
      <Icon aria-hidden className={cn("size-icon-sm shrink-0", toast.tone === "neutral" && "text-muted-foreground")} />
      <ToastPrimitive.Title className="min-w-0">{toast.text}</ToastPrimitive.Title>
      {toast.actions.map((action) => (
        <ToastPrimitive.Action
          key={action.label}
          altText={action.label}
          onClick={action.run}
          className="text-foreground hover:text-foreground/80 ms-1 font-medium transition-colors"
        >
          {action.label}
        </ToastPrimitive.Action>
      ))}
      <ToastPrimitive.Close
        aria-label="Dismiss"
        className="hover:bg-foreground/10 rounded-capsule size-icon-button-sm flex shrink-0 items-center justify-center opacity-80 transition-colors hover:opacity-100"
      >
        <X className="size-icon-xs" />
      </ToastPrimitive.Close>
    </ToastPrimitive.Root>
  );
}

/**
 * Where toasts show: centred at the top of the conversation pane, just under its header, for
 * messages like "Changes reverted" and "Archived chat".
 */
export function Toaster({ className }: { className?: string }) {
  const toasts = useToasts((s) => s.toasts);
  return (
    <ToastPrimitive.Provider swipeDirection="up">
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} />
      ))}
      <ToastPrimitive.Viewport
        className={cn(
          "pointer-events-none absolute inset-x-0 z-50 flex flex-col items-center gap-2 outline-none *:pointer-events-auto",
          className,
        )}
      />
    </ToastPrimitive.Provider>
  );
}
