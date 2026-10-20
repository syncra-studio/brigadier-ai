import type { FC } from "react";

import { TooltipIconButton, type TooltipIconButtonProps } from "@/components/assistant-ui/tooltip-icon-button";
import { Check, Copy } from "@/components/icons";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";

/**
 * Copies `text`, then shows a check and says "Copied" for `copiedDuration` ms. Any other button
 * prop passes through; `onClick` runs before the copy (e.g. to stop the click reaching a card).
 */
export const CopyButton: FC<
  Omit<TooltipIconButtonProps, "tooltip" | "children"> & { text: string; tooltip?: string; copiedDuration?: number }
> = ({ text, tooltip = "Copy", copiedDuration = 3000, onClick, ...props }) => {
  const { isCopied, copyToClipboard } = useCopyToClipboard({ copiedDuration });
  return (
    <TooltipIconButton
      {...props}
      tooltip={isCopied ? "Copied" : tooltip}
      onClick={(event) => {
        onClick?.(event);
        copyToClipboard(text);
      }}
    >
      {isCopied ? (
        <Check className="animate-in zoom-in-50 fade-in duration-200 ease-out" />
      ) : (
        <Copy className="animate-in zoom-in-75 fade-in duration-150" />
      )}
    </TooltipIconButton>
  );
};
