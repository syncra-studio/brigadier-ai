// What the app's page asks of its shell: the same commands, by the same names, as the Tauri build's
// `invoke_handler` (src-tauri/src/main.rs). A page gets its answer as an envelope, since a rejection across
// Electron's IPC keeps only a message and the page needs the daemon's error code.
import type { IpcMainInvokeEvent, WebContents } from "electron";

import { handleFromApp } from "./appPage";

export type Args = Record<string, unknown>;
export type CommandContext = { sender: WebContents };
export type Command = (args: Args, context: CommandContext) => unknown;

/** An answer already serialized, which main passes on unread: the daemon's replies are large and frequent. */
export class RawJson {
  readonly text: string;
  constructor(text: string) {
    this.text = text;
  }
}

/** A channel's id in a command's arguments, as `src/ipc/native.ts` sends it. */
export function channelId(value: unknown): number {
  const id = (value as { channelId?: unknown } | null)?.channelId;
  if (typeof id !== "number") throw new Error("expected a channel");
  return id;
}

/** Sends JSON down a page's channel. A page that went hears nothing. */
export function channelSender(sender: WebContents, id: number): (json: string) => void {
  return (json) => {
    if (!sender.isDestroyed()) sender.send("brigadier:channel", id, json);
  };
}

const commands = new Map<string, Command>();
/** What a page's call waits for: commands that know the daemon exist once its bridge has started. */
let ready: Promise<void> = Promise.resolve();

export function waitForCommands(until: Promise<void>): void {
  ready = until;
}

export function addCommands(added: Record<string, Command>): void {
  for (const [name, command] of Object.entries(added)) {
    if (commands.has(name)) throw new Error(`command ${name} is defined twice`);
    commands.set(name, command);
  }
}

export function commandNames(): string[] {
  return [...commands.keys()];
}

/** An error as the page rejects with it: the daemon's `{code, message}` whole, anything else its message. */
export function errorForPage(error: unknown): unknown {
  if (typeof error === "object" && error !== null && "code" in error && "message" in error && !(error instanceof Error)) return error;
  return error instanceof Error ? error.message : String(error);
}

async function run(event: IpcMainInvokeEvent, name: string, args: Args | undefined) {
  try {
    await ready;
    const command = commands.get(name);
    if (!command) throw new Error(`the shell has no command ${name}`);
    const value = await command(args ?? {}, { sender: event.sender });
    return value instanceof RawJson ? { json: value.text } : { value };
  } catch (error) {
    return { error: errorForPage(error) };
  }
}

export function installCommands(): void {
  handleFromApp("brigadier:invoke", (event, name: string, args?: Args) => run(event, name, args));
}
