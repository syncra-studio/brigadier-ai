/*
 * Branch names as git allows them (`git check-ref-format --branch`), and the prefix put before
 * the names Brigadier makes up: the same rules and words as the daemon's check
 * (`crates/git/src/refname.rs`), so a name or prefix shown as fine here is saved.
 */

/** Why `name` can't be a branch's name, in plain words, or `null` when git takes it. */
export function branchNameProblem(name: string): string | null {
  if (!name) return "A branch name can't be empty.";
  if (name === "@") return 'A branch can\'t be named "@".';
  if (name.startsWith("-")) return 'A branch name can\'t start with "-".';
  if (name.startsWith("/") || name.endsWith("/")) return 'A branch name can\'t start or end with "/".';
  if (name.endsWith(".")) return 'A branch name can\'t end with ".".';
  for (const bad of ["..", "@{", "//", "\\"]) {
    if (name.includes(bad)) return `A branch name can't contain "${bad}".`;
  }
  for (const c of name) {
    const code = c.charCodeAt(0);
    if (c === " ") return "A branch name can't contain spaces.";
    if (code < 0x20 || code === 0x7f) return "A branch name can't contain control characters.";
    if ("~^:?*[".includes(c)) return `A branch name can't contain "${c}".`;
  }
  for (const part of name.split("/")) {
    if (part.startsWith(".")) return 'No part of a branch name can start with ".".';
    if (part.endsWith(".lock")) return 'No part of a branch name can end with ".lock".';
  }
  return null;
}

/**
 * Why `prefix` can't go before the branch names Brigadier makes up, or `null`. It is checked
 * in the names it would make: a session's, a worker's and a suggested one. Empty is fine (no
 * prefix); a trailing `/` is what separates it.
 */
export function branchPrefixProblem(prefix: string): string | null {
  if (!prefix) return null;
  if (prefix.includes("//")) return 'The prefix can\'t contain "//".';
  if (prefix.endsWith(".")) return 'The prefix can\'t end with ".".';
  for (const rest of ["0a1b2c3d/session", "0a1b2c3d/task-1-fix-the-login", "fix-the-login"]) {
    const problem = branchNameProblem(`${prefix}${rest}`);
    if (problem) return problem.replace("A branch name", "The prefix").replace("a branch name", "the prefix");
  }
  return null;
}
