import { nativeWindow } from "@/ipc/native";

async function check(): Promise<void> {
  document.documentElement.toggleAttribute("data-fullscreen", await nativeWindow.isFullscreen());
}

/**
 * Marks the root `data-fullscreen` while the window is in full screen, where macOS hides the
 * traffic lights and the titlebar's sidebar toggle moves to their place (see
 * styles/tokens.css). The window resizes on entering and leaving full screen, so each resize
 * rechecks.
 */
export async function trackFullscreen(): Promise<void> {
  await nativeWindow.onResized(() => {
    void check().catch((error: unknown) => console.error("reading full screen failed", error));
  });
  await check();
}
