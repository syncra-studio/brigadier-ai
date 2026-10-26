import policy from "../../../../../crates/core/src/persistent_apps.json" with { type: "json" };

/** A deliberately small literal shell grammar; shared policy/corpus also drive the daemon. */
function tokens(command: string): (string | true)[] | null {
  const chars = [...command];
  const result: (string | boolean)[] = [];
  let word = "", active = false, quotedName = false;
  const finish = () => { if (active) result.push(quotedName && /^[A-Za-z_][A-Za-z0-9_]*=/.test(word) ? false : word); word = ""; active = false; quotedName = false; };
  for (let i = 0; i < chars.length; i++) {
    const char = chars[i]!;
    if (char === " " || char === "\t") { finish(); continue; }
    if (char === "'" || char === '"') {
      active = true;
      if (!word.includes("=")) quotedName = true;
      const quote = char;
      for (;;) {
        const next = chars[++i];
        if (next === undefined || /[\r\n\\]/.test(next) || (quote === '"' && /[$`]/.test(next))) return null;
        if (next === quote) break;
        word += next;
      }
    } else if (char === "&" && chars[i + 1] === "&") { finish(); result.push(true); i++; }
    else {
      if (/[\r\n;&|<>()[\]{}*?~#$`\\]/.test(char) || char.charCodeAt(0) < 32) return null;
      active = true;
      word += char;
    }
  }
  finish();
  return result.some((token) => token === false) ? null : result as (string | true)[];
}

function programName(program: string): string | null {
  return program.includes("/") && !program.startsWith("/") ? null : program.split("/").at(-1) ?? null;
}
const port = (value: string) => /^\d+$/.test(value) && Number(value) <= 65535;
type Rule = { programs: string[]; prefixes: string[][]; options: Record<string, string>; separator?: boolean; positional?: string; browser?: boolean };
function options(rule: Rule, args: string[]): boolean {
  let browser = false, positionals = 0;
  if (rule.separator && args[0] === "--") args = args.slice(1);
  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    if (!arg.startsWith("-")) {
      if (rule.positional === "port" && positionals++ === 0 && port(arg)) continue;
      if (rule.positional === "url" && /^(?:https?:\/\/\S+|about:blank)$/.test(arg)) continue;
      return false;
    }
    const equals = arg.indexOf("=");
    const name = equals < 0 ? arg : arg.slice(0, equals);
    const inline = equals < 0 ? undefined : arg.slice(equals + 1);
    const kind = rule.options[name];
    if (!kind) return false;
    if (kind === "flag") { if (inline !== undefined) return false; continue; }
    if (kind === "headless") { if (inline !== undefined && !["new", "old"].includes(inline)) return false; browser = true; continue; }
    const value = inline ?? (args[i + 1] && !args[i + 1]!.startsWith("-") ? args[++i] : undefined);
    if (kind === "optional" && value === undefined) continue;
    if (value === undefined || value === "" || value.startsWith("-")) return false;
    if (kind === "port" && !port(value)) return false;
    if (kind === "dimensions" && !/^\d+,\d+$/.test(value)) return false;
    if (name === "--remote-debugging-port") browser = true;
  }
  return !rule.browser || browser;
}

/** Actual command evidence only. Unknown/dynamic/test/build invocations continue holding work. */
export function persistentApp(command: string, depth = 0): boolean {
  if (depth > 2) return false;
  const parsed = tokens(command);
  if (!parsed) return false;
  let at = 0;
  while (typeof parsed[at] === "string" && /^[A-Za-z_][A-Za-z0-9_]*=/.test(parsed[at] as string)) {
    if (policy.unsafeAssignments.includes((parsed[at] as string).split("=")[0]!)) return false;
    at++;
  }
  while (parsed[at] === "cd") {
    at++;
    if (parsed[at] === "--") at++;
    const directory = parsed[at++];
    if (typeof directory !== "string" || !directory || directory.startsWith("-") || parsed[at++] !== true) return false;
    while (typeof parsed[at] === "string" && /^[A-Za-z_][A-Za-z0-9_]*=/.test(parsed[at] as string)) {
      if (policy.unsafeAssignments.includes((parsed[at] as string).split("=")[0]!)) return false;
      at++;
    }
  }
  const words = parsed.slice(at);
  if (words.some((word) => typeof word !== "string")) return false;
  const [program, ...args] = words as string[];
  if (!program) return false;
  const name = programName(program);
  if (name === null) return false;
  if (policy.shells.includes(name)) return args.length === 2 && ["-c", "-lc"].includes(args[0]!) && persistentApp(args[1]!, depth + 1);
  return policy.rules.some((rule: Rule) => rule.programs.includes(name) && rule.prefixes.some((prefix) => prefix.every((word, i) => args[i] === word) && options(rule, args.slice(prefix.length))));
}
