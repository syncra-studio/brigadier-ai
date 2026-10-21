import { useState } from "react";

import { ClearForm } from "@/app/settings/ClearForm";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/overlays";

/**
 * Clear browsing data: a time range and what to remove from Brigadier's browser. Cookies and the
 * cache are WebKit's, so they show only where the browser can clear them (`siteData`).
 */
export function ClearBrowsingDataDialog({
  open,
  onOpenChange,
  siteData,
  onCleared,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  siteData: boolean;
  onCleared?: () => void;
}) {
  const [clearing, setClearing] = useState(false);
  return (
    <Dialog open={open} onOpenChange={(next) => !clearing && onOpenChange(next)}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>Clear browsing data</DialogTitle>
          <DialogDescription className="sr-only">
            Choose a time range and what to remove from Brigadier's browser.
          </DialogDescription>
        </DialogHeader>
        {/* Made each time it opens, so it starts from the usual picks. */}
        {open && (
          <ClearForm
            siteData={siteData}
            clearing={clearing}
            setClearing={setClearing}
            onDone={() => {
              onCleared?.();
              onOpenChange(false);
            }}
            onCancel={() => onOpenChange(false)}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}
