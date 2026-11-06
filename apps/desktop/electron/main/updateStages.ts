// The nightly update's states, as src-tauri/src/updates.rs: found by a check, downloaded only once the user
// asks for it, installed when the app quits after that, and only once the daemon has stopped.

export type Shown = { state: "available" | "updating" | "failed" | "ready"; version: string };
/** Puts a downloaded update in place, reopening the app after it when told to. */
export type Install = (reopen: boolean) => void;
export type Download = () => Promise<Install>;

type Stage =
  | { kind: "nothing" }
  | { kind: "offered"; version: string; download: Download; failed: boolean }
  | { kind: "downloading"; version: string; download: Download }
  | { kind: "ready"; version: string; install: Install };

export class UpdateStages {
  private stage: Stage = { kind: "nothing" };
  /** "Restart to update" was chosen: the app opens again after quitting. */
  restart = false;

  shown(): Shown | null {
    switch (this.stage.kind) {
      case "nothing":
        return null;
      case "offered":
        return { state: this.stage.failed ? "failed" : "available", version: this.stage.version };
      case "downloading":
        return { state: "updating", version: this.stage.version };
      case "ready":
        return { state: "ready", version: this.stage.version };
    }
  }

  /** Offers what a check found, downloading nothing. A version already offered, or any once the user has asked for one, changes nothing. */
  offer(version: string, download: Download): Shown | null {
    if (this.stage.kind === "downloading" || this.stage.kind === "ready") return null;
    if (this.stage.kind === "offered" && this.stage.version === version) return null;
    this.stage = { kind: "offered", version, download, failed: false };
    return this.shown();
  }

  /** The user asked for the offered update. Nothing offered, or already under way, does nothing. */
  async take(show: (shown: Shown) => void): Promise<void> {
    if (this.stage.kind !== "offered") return;
    const { version, download } = this.stage;
    this.stage = { kind: "downloading", version, download };
    show({ state: "updating", version });
    try {
      const install = await download();
      this.stage = { kind: "ready", version, install };
    } catch (error) {
      console.warn("could not download the nightly update:", error);
      this.stage = { kind: "offered", version, download, failed: true };
    }
    const now = this.shown();
    if (now) show(now);
  }

  /**
   * At the app's end: installs the update the user asked for when the daemon has stopped (once), and says
   * whether to open the app again. One that was only found, or is still downloading, isn't installed.
   */
  finish(daemonStopped: boolean): { reopen: boolean; installing: boolean } {
    if (this.stage.kind !== "ready") return { reopen: this.restart, installing: false };
    const { install } = this.stage;
    this.stage = { kind: "nothing" };
    if (!daemonStopped) return { reopen: this.restart, installing: false };
    install(this.restart);
    return { reopen: this.restart, installing: true };
  }
}

/** Runs `work` while `progress` keeps being told of new bytes at least every `limitMs`. */
export function unlessStalled<T>(limitMs: number, work: Promise<T>, onProgress: (tick: () => void) => () => void, cancel: () => void): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    let timer: ReturnType<typeof setTimeout>;
    const stop = onProgress(() => {
      clearTimeout(timer);
      arm();
    });
    const arm = () => {
      timer = setTimeout(() => {
        stop();
        cancel();
        reject(new Error(`no new bytes for ${Math.round(limitMs / 1000)} s`));
      }, limitMs);
    };
    arm();
    work.then(
      (value) => {
        clearTimeout(timer);
        stop();
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        stop();
        reject(error instanceof Error ? error : new Error(String(error)));
      },
    );
  });
}

/** The feed this build checks, in words, where it checks at all. */
export function updateChannel(version: string, configured: boolean): string | null {
  if (!configured) return null;
  return version.includes("-nightly.") ? "Nightly" : "Release";
}

/** Whether this launch looks for updates: a packaged production build with a feed, not under --smoke. */
export const updatesEnabled = (dev: boolean, configured: boolean, smoke: boolean): boolean => !dev && configured && !smoke;
