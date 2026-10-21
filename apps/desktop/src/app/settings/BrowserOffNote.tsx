import type { ComponentProps } from "react";

/** The grey line under a page's top card while the built-in browser is off. */
export function BrowserOffNote({ children, ...props }: ComponentProps<"p">) {
  return (
    <p {...props} className="text-foreground/65 px-1 text-xs">
      {children}
    </p>
  );
}
