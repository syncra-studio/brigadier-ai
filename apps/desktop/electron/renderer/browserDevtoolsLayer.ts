// DevTools' panes live in main, each a view over the slot the app keeps for it (electron/main/browserDevtools.ts).
import type { BrowserBounds } from "@/ipc/generated";

import { agentPage } from "./tabLayer";

export async function placeDevtools(id: string, bounds: BrowserBounds | null): Promise<void> {
  await window.brigadierShell.devtools("place", { guest: agentPage(id).guest, bounds });
}

export async function openDevtools(id: string, bounds: BrowserBounds): Promise<void> {
  await window.brigadierShell.devtools("open", { guest: agentPage(id).guest, bounds });
}

export async function closeDevtools(id: string): Promise<void> {
  await window.brigadierShell.devtools("close", { guest: agentPage(id).guest });
}
