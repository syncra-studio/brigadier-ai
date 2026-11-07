// Every frame of every guest and popup on macOS (main/browserHost.ts lets it into subframes), until Apple grants the browser passkey entitlement (section 7). Stock Electron
// serves no passkey request and never answers one, so a page waits forever. A request the user would see fails at
// once and the tab says why. Passkey autofill (`conditional`) shows nothing and may wait, so it stays with Chromium.
// Availability checks stay Chromium's too: a page is told the truth, that no platform authenticator is there.
import { contextBridge, ipcRenderer } from "electron";

contextBridge.executeInMainWorld({
  func: (refused: () => void) => {
    const credentials = navigator.credentials;
    if (!credentials) return;
    for (const name of ["get", "create"] as const) {
      const real = credentials[name].bind(credentials);
      credentials[name] = (options?: CredentialRequestOptions & CredentialCreationOptions) => {
        if (!options?.publicKey || options.mediation === "conditional") return real(options);
        refused();
        return Promise.reject(new DOMException("The operation either timed out or was not allowed.", "NotAllowedError"));
      };
    }
  },
  args: [() => ipcRenderer.send("brigadier:guest-passkey-refused")],
});
