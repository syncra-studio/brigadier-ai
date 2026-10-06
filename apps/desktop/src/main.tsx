// The styles are linked from index.html, so the startup screen has them before this runs.
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "@/app/App";
import { appInfo } from "@/ipc/client";
import { applyDensity, cachedDensity } from "@/lib/density";
import { trackFocusInput } from "@/lib/focusRing";
import { trackFullscreen } from "@/lib/fullscreen";
import { showStartupError, watchStartup } from "@/lib/splash";
import { markStartup } from "@/lib/startup";
import { startBridge } from "@/state/bridge";
import { useApp } from "@/state/store";

markStartup("script");
// Before anything is awaited, so a hang anywhere below still ends in something to do.
watchStartup();
try {
  // Density and platform are known before the first React paint, so nothing jumps.
  applyDensity(cachedDensity());
  trackFocusInput();
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
} catch (error) {
  console.error("startup failed", error);
  showStartupError();
}
