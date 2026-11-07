// A stand-in for the `electron` package, so `node --test` can load main's modules (`electron/main/*`). Importing this
// makes `electron` resolve to this very file and lets main's imports go without an extension, as the bundler does. So
// a test imports this first and then what it tests by `await import`, since a static import resolves before this runs.
import { statSync } from "node:fs";
import { registerHooks } from "node:module";
import { pathToFileURL } from "node:url";

const self = import.meta.url;
const electronFolder = new URL("../", self).href;

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier === "electron") return { url: self, shortCircuit: true };
    if (specifier.startsWith(".") && context.parentURL?.startsWith(electronFolder)) {
      const file = new URL(`${specifier}.ts`, context.parentURL);
      if (statSync(file, { throwIfNoEntry: false })?.isFile()) return nextResolve(pathToFileURL(file.pathname).href, context);
    }
    return nextResolve(specifier, context);
  },
});

type Handler = (event: unknown, ...args: unknown[]) => unknown;

/** What a test sets and reads. */
export const fake = {
  userData: "",
  encryption: true,
  backend: "gnome_libsecret",
  canPromptTouchID: true,
  touchIdAnswer: true,
  touchIdAsked: [] as string[],
  handlers: new Map<string, Handler>(),
  contents: new Map<number, unknown>(),
};

const SEAL = "fake-sealed:";

export const app = { getPath: () => fake.userData };

/** Reversible, and no value shows in what it gives; what it didn't seal it can't open, as a key that changed. */
export const safeStorage = {
  isEncryptionAvailable: () => fake.encryption,
  getSelectedStorageBackend: () => fake.backend,
  encryptString: (text: string) => Buffer.from(SEAL + Buffer.from(text).toString("base64")),
  decryptString: (sealed: Buffer) => {
    const text = sealed.toString();
    if (!text.startsWith(SEAL)) throw new Error("Error while decrypting the ciphertext provided to safeStorage.decryptString.");
    return Buffer.from(text.slice(SEAL.length), "base64").toString();
  },
};

export const systemPreferences = {
  canPromptTouchID: () => fake.canPromptTouchID,
  promptTouchID: async (reason: string) => {
    fake.touchIdAsked.push(reason);
    if (!fake.touchIdAnswer) throw new Error("Touch ID cancelled");
  },
};

export const webContents = { fromId: (id: number) => fake.contents.get(id), getAllWebContents: () => [] };
export const ipcMain = {
  handle: (channel: string, handler: Handler) => fake.handlers.set(channel, handler),
  on: () => {},
};
export const session = {};
export const BrowserWindow = {};
