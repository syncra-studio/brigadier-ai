import type { BoardDigest, SessionSentStep } from "@/app/conversation/blocks";
import { type ToolStep, unwrapCommand } from "@/app/conversation/activity/words";
import type { ProviderEvent } from "@/ipc/generated";

/**
 * The thread's own `brigadier send <session> <text>` (S30) as the row of the message it sent:
 * "Messaged Docs: Can you check the schema?". A send to one of the session's workers keeps the
 * row it has (the worker's "Messaged …").
 */

/** How short an id `brigadier` takes for a session or a worker (`manager::cli::MIN_ID`). */
const MIN_ID = 4;

/** What `brigadier send` prints for a message it gave another session (`cli::render::send`). */
const SENT_TO_SESSION = /^Sent (sm-[0-9a-f]+) to session (\S+?): /m;

/** What it prints for a message it gave a worker. */
const SENT_TO_WORKER = /^Sent to \S+?: /m;

/** Whether `id` names `full` as `brigadier` reads an id: whole, or the start of its last 8. */
export function namesId(full: string, id: string): boolean {
  return full === id || (id.length >= MIN_ID && full.slice(-8).startsWith(id));
}

/** A heredoc fed to the command line as one argument: `"$(cat <<'EOF'` … `EOF` `)"`. */
const HEREDOC = /"\$\(cat\s+<<-?\s*(['"]?)(\w+)\1[ \t]*\n([\s\S]*?)\n[ \t]*\2[ \t]*\n?\s*\)"/g;

/** A heredoc's body stands for itself in a word: a mark the lexer leaves whole. */
const MARK = "\u0000";

/**
 * A command line's words and separators as the shell splits them, its quotes removed; null when a
 * quote never closes (no shell would run it).
 */
function shellTokens(command: string): string[] | null {
  const bodies: string[] = [];
  const line = command.replace(HEREDOC, (_, _quote: string, _tag: string, body: string) => `${MARK}${bodies.push(body) - 1}${MARK}`);
  const tokens: string[] = [];
  let word: string | null = null;
  const add = (text: string) => {
    word = (word ?? "") + text;
  };
  const end = () => {
    if (word !== null) tokens.push(word.replace(new RegExp(`${MARK}(\\d+)${MARK}`, "g"), (_, index: string) => bodies[Number(index)] ?? ""));
    word = null;
  };
  for (let at = 0; at < line.length; at += 1) {
    const char = line[at]!;
    if (char === "\\") {
      const next = line[at + 1];
      at += 1;
      if (next !== undefined && next !== "\n") add(next);
    } else if (char === "'") {
      const close = line.indexOf("'", at + 1);
      if (close < 0) return null;
      add(line.slice(at + 1, close));
      at = close;
    } else if (char === "$" && line[at + 1] === "'") {
      // ANSI-C quoting: `$'a\nb'`.
      let text = "";
      at += 2;
      for (; at < line.length && line[at] !== "'"; at += 1) {
        if (line[at] === "\\" && at + 1 < line.length) {
          at += 1;
          const escaped = line[at]!;
          text += escaped === "n" ? "\n" : escaped === "t" ? "\t" : escaped;
        } else text += line[at];
      }
      if (at >= line.length) return null;
      add(text);
    } else if (char === '"') {
      let text = "";
      at += 1;
      for (; at < line.length && line[at] !== '"'; at += 1) {
        const next = line[at + 1];
        if (line[at] === "\\" && next !== undefined && '$`"\\\n'.includes(next)) {
          at += 1;
          if (next !== "\n") text += next;
        } else text += line[at];
      }
      if (at >= line.length) return null;
      add(text);
    } else if (/[;&|()\n]/.test(char)) {
      end();
      tokens.push(char);
    } else if (/\s/.test(char)) {
      end();
    } else add(char);
  }
  end();
  return tokens;
}

/**
 * What `command` gives `brigadier send`, when it runs it once to message someone: whom (`to`, as
 * the command names it) and the text; null for any other command, or a send that answers a
 * worker's prompt (`--allow`, `--deny`).
 */
export function sendArgs(command: string): { to: string; text: string } | null {
  const tokens = shellTokens(unwrapCommand(command));
  if (!tokens) return null;
  const found: { to: string; text: string }[] = [];
  let start = true;
  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index]!;
    if (/^[;&|()\n]$/.test(token)) {
      start = true;
      continue;
    }
    if (!start) continue;
    if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(token)) continue;
    start = false;
    if (!/^(?:\S*\/)?brigadier$/.test(token) || tokens[index + 1] !== "send") continue;
    const args: string[] = [];
    for (index += 2; index < tokens.length && !/^[;&|()\n]$/.test(tokens[index]!); index += 1) args.push(tokens[index]!);
    index -= 1;
    if (args.some((arg) => arg === "--allow" || arg === "--deny")) return null;
    const [to, ...words] = args;
    if (to === undefined) return null;
    found.push({ to, text: words.join(" ") });
  }
  return found.length === 1 ? found[0]! : null;
}

/** A call's command line, as the CLI gave it: a Codex command's, or a shell tool's input. */
function commandOf(event: ProviderEvent): string | null {
  if (event.type === "command") return event.command;
  if (event.type !== "toolCall" || !event.input) return null;
  try {
    const input: unknown = JSON.parse(event.input);
    const command = typeof input === "object" && input !== null ? (input as Record<string, unknown>)["command"] : undefined;
    return typeof command === "string" ? command : null;
  } catch {
    return null;
  }
}

/**
 * The row of a call that sent another session a message (S30), from the call and what it
 * printed; null for a call that is no such send. Once the call printed, its words say whom it
 * reached. Before (it runs), the id does: a session's, unless one of the session's workers has
 * it. A call that failed is the session's when what it printed says so; one the turn cut off
 * keeps its own row.
 */
export function sessionSent(step: ToolStep | null, event: ProviderEvent, board?: Pick<BoardDigest, "tasks" | "orchestratorSteps">): SessionSentStep | null {
  if (!step || step.item !== "command") return null;
  const command = commandOf(event);
  const args = command === null ? null : sendArgs(command);
  if (!args) return null;
  const output = "output" in event ? (event.output ?? "") : "";
  const sent = SENT_TO_SESSION.exec(output);
  if (!sent) {
    const workers = [
      ...Object.keys(board?.tasks ?? {}),
      ...(board?.orchestratorSteps ?? []).flatMap(({ kind }) => (kind.type === "messaged" ? [kind.taskId] : [])),
    ];
    if (SENT_TO_WORKER.test(output) || workers.some((id) => namesId(id, args.to)) || step.stopped) return null;
    if (step.status !== "inProgress" && !(step.status === "failed" && /\bsession\b/i.test(output))) return null;
  }
  return {
    type: "sessionSent",
    itemId: step.itemId,
    to: args.to,
    text: args.text.trim() || null,
    status: step.status,
    messageId: sent?.[1] ?? null,
    refused: !sent && step.status === "failed" ? output.trim().slice(0, 2000) || null : null,
  };
}
