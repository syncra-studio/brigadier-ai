// The Electron page's script: the shell's side of what src/ asks, then the real entry point.
import { setPageCommands } from "@/ipc/native";

import { browserCommands } from "./browserCommands";
import { installTabLayer } from "./tabLayer";
import "./shell.css";

installTabLayer();
document.documentElement.dataset.shell = "electron";
if (window.brigadierNative) {
  setPageCommands(browserCommands);
} else {
  // A dev run's stand-in window; a production build has none.
  const { installShellIpc } = await import("./shellIpc");
  installShellIpc();
}
void import("@/main");
