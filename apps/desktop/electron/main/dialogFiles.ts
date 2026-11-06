// What the dialogs and opening commands do without Electron, so it can be tested in plain Node. The behaviour
// is the Tauri build's: src-tauri/src/documents.rs, src-tauri/src/downloads.rs and the checks in its main.rs.
import { randomBytes } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

export type IpcFailure = { code: "invalid" | "internal"; message: string };
export const failure = (code: IpcFailure["code"], message: string): IpcFailure => ({ code, message });

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
const errorCode = (error: unknown) => (error as NodeJS.ErrnoException | null)?.code;

/** A name's last part as Rust's `Path::file_name` reads it: none for `..`, `.` or nothing. */
export function fileName(name: string): string | null {
  const last = name.split(path.sep).filter((part) => part !== "" && part !== ".").pop();
  return last === undefined || last === ".." ? null : last;
}

/** A sibling temporary file keeps a failed write from truncating the original. */
export async function atomicWrite(destination: string, bytes: Uint8Array, prefix: string, keepMode: boolean): Promise<void> {
  const parent = path.dirname(destination);
  const temporary = path.join(parent, `${prefix}-${process.pid}-${randomBytes(6).toString("hex")}`);
  const file = await fs.open(temporary, "wx");
  try {
    if (keepMode) {
      const mode = await fs.stat(destination).then((found) => found.mode, () => null);
      if (mode !== null) await file.chmod(mode);
    }
    await file.writeFile(bytes);
    await file.sync();
    await file.close();
    await fs.rename(temporary, destination);
  } catch (error) {
    await file.close().catch(() => {});
    await fs.rm(temporary, { force: true }).catch(() => {});
    throw error;
  }
}

/** Reserves the name exclusively, so simultaneous downloads can't replace each other. */
export async function saveUnique(directory: string, name: string, bytes: Uint8Array): Promise<string> {
  const { name: stem, ext } = path.parse(name);
  for (let number = 0; ; number++) {
    const candidate = path.join(directory, number === 0 ? name : `${stem} (${number})${ext}`);
    try {
      await (await fs.open(candidate, "wx")).close();
    } catch (error) {
      if (errorCode(error) === "EEXIST") continue;
      throw error;
    }
    try {
      await atomicWrite(candidate, bytes, ".brigadier-copy", false);
      return candidate;
    } catch (error) {
      await fs.rm(candidate, { force: true }).catch(() => {});
      throw error;
    }
  }
}

/** The bytes to copy: exactly one of the page's bytes or a local file. */
export async function sourceBytes(bytes: Uint8Array | number[] | null | undefined, source: string | null | undefined): Promise<Uint8Array> {
  const hasBytes = bytes !== null && bytes !== undefined;
  const hasPath = source !== null && source !== undefined;
  if (hasBytes === hasPath) throw new Error("Provide exactly one of bytes or path");
  if (hasBytes) return Uint8Array.from(bytes);
  try {
    if (!(await fs.stat(source as string)).isFile()) throw new Error("Cannot copy a directory or a non-file source");
    return await fs.readFile(source as string);
  } catch (error) {
    throw new Error(message(error).startsWith("Cannot copy") ? message(error) : `Cannot read file: ${message(error)}`, { cause: error });
  }
}

/** Saves an independent copy: where the user picks when `choose` is given, else a new name in Downloads. */
export async function saveCopy(
  { name, bytes, source, downloads, choose }: {
    name: string;
    bytes: Uint8Array | number[] | null | undefined;
    source: string | null | undefined;
    downloads: () => string;
    choose: ((suggested: { directory: string; name: string }) => Promise<string | null>) | null;
  },
): Promise<string | null> {
  const content = await sourceBytes(bytes, source);
  const safeName = fileName(name) ?? "Untitled";
  let directory: string;
  try {
    directory = downloads();
  } catch (error) {
    throw new Error(`Cannot find Downloads: ${message(error)}`, { cause: error });
  }
  const destination = choose ? await choose({ directory, name: safeName }) : null;
  if (choose && destination === null) return null;
  try {
    if (destination !== null) {
      await atomicWrite(destination, content, ".brigadier-copy", false);
      return destination;
    }
    await fs.mkdir(directory, { recursive: true });
    return await saveUnique(directory, safeName, content);
  } catch (error) {
    throw new Error(`Cannot save copy: ${message(error)}`, { cause: error });
  }
}

type Grant = { path: string; last: Buffer };
export type ChooseDestination = (suggested: { directory: string | null; name: string }) => Promise<string | null>;

/**
 * Save grants belong to the shell: the page supplies text and an opaque id, never a destination. Grants end
 * when the app quits; restoring a document asks again. One queue serializes dialogs and writes, including
 * repeated saves while a dialog is open.
 */
export class Documents {
  private readonly grants = new Map<string, Grant>();
  private queue: Promise<unknown> = Promise.resolve();
  private readonly choose: ChooseDestination;

  constructor(choose: ChooseDestination) {
    this.choose = choose;
  }

  private inTurn<T>(work: () => Promise<T>): Promise<T> {
    const turn = this.queue.then(work, work);
    this.queue = turn.catch(() => {});
    return turn;
  }

  save(args: { id: string; text: string; directory: string | null; name: string; choose: boolean }): Promise<string | null> {
    return this.inTurn(async () => {
      const { id, text } = args;
      // Save As asks even when a destination is remembered; the old one stays until the new one is written.
      const grant = args.choose ? undefined : this.grants.get(id);
      let destination: string;
      if (grant) {
        try {
          await check(grant);
        } catch (error) {
          this.grants.delete(id);
          throw error;
        }
        destination = grant.path;
      } else {
        const picked = await this.choose({ directory: args.directory, name: fileName(args.name) ?? "Untitled" });
        if (picked === null) return null;
        destination = await resolveChosen(picked);
      }
      const written = Buffer.from(text);
      try {
        await atomicWrite(destination, written, ".brigadier-save", true);
      } catch (error) {
        // A failed write forgets the destination, so the next save asks instead of trying it again.
        this.grants.delete(id);
        throw new Error(`Cannot save to ${destination}: ${message(error)}. Your draft is unchanged; save again to choose another location.`, { cause: error });
      }
      this.grants.set(id, { path: destination, last: written });
      return destination;
    });
  }

  forget(id: string): Promise<void> {
    return this.inTurn(async () => {
      this.grants.delete(id);
    });
  }
}

async function check(grant: Grant): Promise<void> {
  let current: Buffer | null;
  try {
    current = await fs.readFile(grant.path);
  } catch (error) {
    if (errorCode(error) !== "ENOENT") throw new Error(`Cannot check the saved file: ${message(error)}. Your draft is unchanged.`, { cause: error });
    current = null;
  }
  if (current === null || !current.equals(grant.last)) {
    throw new Error("The file changed outside Brigadier. Your draft was not saved. Save again to choose a new destination or confirm replacement in the save dialog.");
  }
}

/** Follows a chosen link once: the grant names its target, so replacing the file keeps the link. */
async function resolveChosen(picked: string): Promise<string> {
  try {
    return await fs.realpath(picked);
  } catch (error) {
    const nothingThere = errorCode(error) === "ENOENT" && (await fs.lstat(picked).then(() => false, () => true));
    if (nothingThere) return picked;
    throw new Error(`Cannot resolve the selected file: ${message(error)}`, { cause: error });
  }
}

/** Only http, https and mailto: a thread's links come from models and must not launch other handlers. */
export function checkUrl(url: string): void {
  const colon = url.indexOf(":");
  const scheme = colon < 0 ? null : url.slice(0, colon).toLowerCase();
  if (scheme !== "http" && scheme !== "https" && scheme !== "mailto") throw failure("invalid", `only web and email links open: ${url}`);
}

/** Only an existing `.pdf` at an absolute path: the path comes from the thread. */
export async function checkPdf(file: string): Promise<void> {
  const isFile = await fs.stat(file).then((found) => found.isFile(), () => false);
  if (path.extname(file).toLowerCase() !== ".pdf" || !path.isAbsolute(file) || !isFile) {
    throw failure("invalid", `only a PDF file opens here: ${file}`);
  }
}

/** The file manager's name, as a person would read it. */
export function fileManager(platform: NodeJS.Platform): string {
  return platform === "darwin" ? "Finder" : platform === "win32" ? "File Explorer" : "the file manager";
}

/** How to open `target` with the app named `app`: `/usr/bin/open` by the app's name on macOS. */
export function openWithCommand(platform: NodeJS.Platform, app: string, target: string): { command: string; args: string[] } {
  return platform === "darwin" ? { command: "/usr/bin/open", args: ["-a", app, "--", target] } : { command: app, args: [target] };
}
