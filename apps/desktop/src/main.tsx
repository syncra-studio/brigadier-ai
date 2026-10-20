// The styles are linked from index.html, so the startup screen has them before this runs.
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "@/app/App";
import { appInfo } from "@/ipc/client";
import { trackFocusInput } from "@/lib/focusRing";
import { trackFullscreen } from "@/lib/fullscreen";
import { trackPageHidden } from "@/lib/pageHidden";
import { showStartupError, watchStartup } from "@/lib/splash";
import { markStartup } from "@/lib/startup";
import { startBridge } from "@/state/bridge";
import { useApp } from "@/state/store";

markStartup("script");
// Before anything is awaited, so a hang anywhere below still ends in something to do.
watchStartup();
// Not awaited at the top level: the setup window's chunk imports from this one, and would wait
// forever for a module that is itself waiting for it.
void start();

async function start(): Promise<void> {
  try {
    if (new URLSearchParams(location.search).get("window") === "computer-setup") {
      const { startComputerSetup } = await import("@/app/computerSetup/start");
      await startComputerSetup();
    } else {
      await startApp();
    }
  } catch (error) {
    console.error("startup failed", error);
    showStartupError();
  }
}

async function startApp(): Promise<void> {
  trackFocusInput();
  trackPageHidden();
  const info = await appInfo();
  document.documentElement.dataset.platform = info.platform;
  if (info.platform === "macos") {
    void trackFullscreen().catch((error: unknown) => {
      console.error("tracking full screen failed", error);
    });
  }
  useApp.setState({ info });
  await startBridge();

  const root = document.getElementById("root");
  if (!root) throw new Error("Missing #root element");

  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
