// The Electron page's script: the shell's side of what src/ asks, then the real entry point.
import { setPageCommands } from "@/ipc/native";

import { browserCommands } from "./browserCommands";
import { keepTabsOnDisk } from "./keepTabs";
import { installTabLayer } from "./tabLayer";
import "./shell.css";

installTabLayer();
document.documentElement.dataset.shell = "electron";
if (window.brigadierNative) {
  setPageCommands(browserCommands);
} else if (import.meta.env.DEV) {
  // A dev run's stand-in window; a production build has none, and doesn't carry the stand-in's mocks.
  const { installShellIpc } = await import("./shellIpc");
  installShellIpc();
}
void import("@/main");
void keepTabsOnDisk();
