import { Channel, invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { SetupWindow } from "@/app/computerSetup/SetupWindow";
import type { BridgeEvent } from "@/ipc/generated";
import { onComputerUse } from "@/state/computerUse";

/**
 * The computer-use setup window's start: its own feed of computer use's status (the main
 * window's stays as it is), then the window itself, without the app's startup screen.
 */
export async function startComputerSetup(): Promise<void> {
  document.getElementById("splash")?.remove();
  delete document.documentElement.dataset.splash;
  const channel = new Channel<BridgeEvent>((event) => {
    if (event.type === "computerUse") onComputerUse(event.status);
  });
  await invoke("computer_setup_subscribe", { channel });
  const root = document.getElementById("root");
  if (!root) throw new Error("Missing #root element");
  createRoot(root).render(
    <StrictMode>
      <SetupWindow onClose={() => void getCurrentWindow().close()} />
    </StrictMode>,
  );
}
