// The Electron page's script. Everything the app's own code asks of its native shell goes through
// Tauri's `invoke`, so the shell is swapped underneath and src/ runs unchanged: this file installs
// the swap, then runs the real entry point.
import { installShellIpc } from "./shellIpc";
import { installTabLayer } from "./tabLayer";

installTabLayer();
installShellIpc();
void import("@/main");
