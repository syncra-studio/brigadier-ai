// Reading back a file of the last drop (src-tauri's drops.rs `dropped_file`), kept apart from Electron so it
// can be tested alone.
import { open, stat } from "node:fs/promises";
import path from "node:path";

/** brigadier_core::MAX_ATTACHMENT_BYTES, which the daemon holds attachments to. */
export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;

/** `file`'s bytes, when it is a file among `dropped` no larger than an attachment may be; nothing else is read. */
export async function droppedFile(dropped: readonly string[], file: string): Promise<Uint8Array> {
  if (!dropped.includes(file)) throw new Error("Only a dropped file can be read");
  const isFile = await stat(file).then((found) => found.isFile(), () => false);
  if (!isFile) throw new Error(`${file} isn't a file`);
  const name = path.basename(file) || file;
  const tooLarge = () => new Error(`${name} is too large to attach (maximum 10 MB)`);
  try {
    const handle = await open(file, "r");
    try {
      if ((await handle.stat()).size > MAX_ATTACHMENT_BYTES) throw tooLarge();
      // Read no further than the limit, should the file have grown since.
      const bytes = Buffer.allocUnsafe(MAX_ATTACHMENT_BYTES + 1);
      let length = 0;
      while (length < bytes.length) {
        const { bytesRead } = await handle.read(bytes, length, bytes.length - length);
        if (bytesRead === 0) break;
        length += bytesRead;
      }
      if (length > MAX_ATTACHMENT_BYTES) throw tooLarge();
      return bytes.subarray(0, length);
    } finally {
      await handle.close();
    }
  } catch (error) {
    if (error instanceof Error && error.message.endsWith("(maximum 10 MB)")) throw error;
    throw new Error(`Cannot read ${name}: ${error instanceof Error ? error.message : String(error)}`, { cause: error });
  }
}
