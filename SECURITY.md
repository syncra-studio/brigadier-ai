# Security policy

## Reporting a vulnerability

Report vulnerabilities privately, through GitHub:
[Report a vulnerability](https://github.com/syncra-studio/brigadier-ai/security/advisories/new)
(the repository's **Security** tab › **Report a vulnerability**).

Never report one in a public issue, pull request or discussion. A public report is closed, and
the details are removed where possible.

Include:

- the commit or version, your OS and the agent CLIs' versions;
- the permission level the session ran at;
- exact steps to reproduce, and what an attacker gains;
- a proof of concept if you have one. It must stay within your own machine and accounts.

We aim to reply within a week. Please give us a reasonable time to ship a fix before you
disclose anything publicly. If you want credit, the advisory will name you.

## Supported versions

Brigadier has no stable release yet. Only the latest `main` is supported: check that a problem
still exists there before you report it.

## Scope

In scope: anything that lets a worker, a thread, a web page or another local process do more than
the session's permission level allows. For example:

- a session doing more than its CLI's own permission mode lets it (Brigadier passes the level
  straight through and adds no sandbox of its own);
- reaching the daemon's IPC token or acting as the UI from a worker, or using another task's
  grant;
- a project's secrets leaking past redaction into what Brigadier records, shows or sends;
- a page in the built-in browser getting the app's IPC, or capabilities its site settings deny;
- computer use reading password fields, or acting on the apps it must never touch;
- a model registry update getting past its bounds checks;
- cleanup or uninstall deleting anything Brigadier didn't create.

Out of scope:

- the limits listed in the README under
  [What still depends on trust](README.md#what-still-depends-on-trust);
- what a session at **Full access** can do: there, workers run with your account's full rights
  by design;
- vulnerabilities in the `claude` or `codex` CLIs themselves: report those to their vendors;
- a prompt injection that only makes an agent do what its permission level already allows.
