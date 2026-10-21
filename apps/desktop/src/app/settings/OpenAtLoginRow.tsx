import { useCallback, useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { GENERAL_ROWS } from "@/app/settings/generalRows";
import { SettingsSwitch } from "@/app/settings/parts";
import { Button } from "@/components/controls";
import { SettingsRow } from "@/components/shared";
import { type LoginItem, loginItem, openLoginItems, setLoginItem } from "@/ipc/client";

/** Open at login as last read, kept while the app runs. */
let lastLoginItem: LoginItem | null = null;

/**
 * Open at login, as the system has it: read when the page opens and whenever the window comes
 * back to the front (the user may have allowed or removed it in the system's settings). Not
 * shown where there is no way (a development build on macOS), once that is known. Waiting for the user's approval
 * reads as on, so turning it off takes the request back.
 */
export function OpenAtLoginRow() {
  // As last read, at once; read again as the page opens.
  const [state, setState] = useState<LoginItem | null>(() => lastLoginItem);
  const save = useAction();
  const load = useCallback(() => {
    loginItem()
      .then((read) => {
        lastLoginItem = read;
        setState(read);
      })
      .catch((error: unknown) => console.error("reading open at login failed", error));
  }, []);
  useEffect(() => {
    load();
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
  }, [load]);
  if (state === "unsupported") return null;
  return (
    <SettingsRow
      label={GENERAL_ROWS.openAtLogin.label}
      description={
        state === "needsApproval"
          ? "Allow Brigadier in System Settings › Login Items to finish turning this on"
          : GENERAL_ROWS.openAtLogin.description
      }
      error={save.error}
    >
      {state === "needsApproval" && (
        <Button variant="soft" size="sm" onClick={() => save.run(() => openLoginItems())}>
          Open System Settings
        </Button>
      )}
      <SettingsSwitch
        label={GENERAL_ROWS.openAtLogin.label}
        checked={state === "on" || state === "needsApproval"}
        // Until the first read answers (a moment), the switch waits where it stands.
        disabled={state === null}
        onCheckedChange={(on) =>
          save.run(async () => {
            const next = await setLoginItem(on);
            lastLoginItem = next;
            setState(next);
          })
        }
      />
    </SettingsRow>
  );
}
