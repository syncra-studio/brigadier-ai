//! The one table of subcommands and their arguments. The parser ([`crate::args`]) and the help
//! text ([`help`], [`command_help`]) are both made from it, so they can't drift apart.

/// One subcommand.
pub struct Command {
    /// Stable: a session's instructions and scripts call it by this name.
    pub name: &'static str,
    /// One line, for the list of commands.
    pub summary: &'static str,
    /// More lines, for the command's own help.
    pub details: &'static str,
    pub args: &'static [Arg],
    /// Built in this version; the others say so and exit 4.
    pub available: bool,
    /// The checked-in schema of its `--json` output (`schemas/<name>.schema.json`).
    pub schema: &'static str,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Command({})", self.name)
    }
}

/// One argument of a subcommand.
pub enum Arg {
    Positional {
        name: &'static str,
        help: &'static str,
        required: bool,
        /// Takes every remaining word (joined by spaces where one text is meant).
        rest: bool,
        /// The only words it accepts; empty for any.
        choices: &'static [&'static str],
    },
    Flag {
        long: &'static str,
        /// The value's name in help; `None` for a switch.
        value: Option<&'static str>,
        help: &'static str,
        /// The only values it accepts; empty for any.
        choices: &'static [&'static str],
        /// May be given more than once.
        repeat: bool,
    },
}

const fn positional(name: &'static str, help: &'static str) -> Arg {
    Arg::Positional {
        name,
        help,
        required: true,
        rest: false,
        choices: &[],
    }
}

const fn rest(name: &'static str, help: &'static str, required: bool) -> Arg {
    Arg::Positional {
        name,
        help,
        required,
        rest: true,
        choices: &[],
    }
}

const fn action(choices: &'static [&'static str], help: &'static str) -> Arg {
    Arg::Positional {
        name: "action",
        help,
        required: true,
        rest: false,
        choices,
    }
}

const fn flag(long: &'static str, value: &'static str, help: &'static str) -> Arg {
    Arg::Flag {
        long,
        value: Some(value),
        help,
        choices: &[],
        repeat: false,
    }
}

const fn switch(long: &'static str, help: &'static str) -> Arg {
    Arg::Flag {
        long,
        value: None,
        help,
        choices: &[],
        repeat: false,
    }
}

const fn choice(
    long: &'static str,
    value: &'static str,
    choices: &'static [&'static str],
    help: &'static str,
) -> Arg {
    Arg::Flag {
        long,
        value: Some(value),
        help,
        choices,
        repeat: false,
    }
}

/// Every subcommand, in the order help lists them.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "sessions",
        summary: "List this project's sidebar sessions (--all: every project's, and Chats).",
        details: "Each row: id, state, title, branch, lines added and removed, and how many \
                  workers it has (with --all, its project too). Workers are never listed here; \
                  see `brigadier workers`. `show` and `read` take any of these ids.",
        args: &[switch("all", "Every project's sessions, and every Chat.")],
        available: true,
        schema: include_str!("../schemas/sessions.schema.json"),
    },
    Command {
        name: "workers",
        summary: "List this session's own workers.",
        details: "Each row: id, number, state, agent, model, title, branch, lines added and \
                  removed, and how full its context is. These are the ids `send`, `wait` and \
                  `close` take.",
        args: &[],
        available: true,
        schema: include_str!("../schemas/workers.schema.json"),
    },
    Command {
        name: "show",
        summary: "Show one session or worker in full.",
        details: "An id is a session's or worker's id from `sessions` or `workers`, or a \
                  unique start of one.",
        args: &[positional("id", "The session or worker.")],
        available: true,
        schema: include_str!("../schemas/show.schema.json"),
    },
    Command {
        name: "read",
        summary: "Read a session's or worker's last message, last items, or screen.",
        details: "Without options: the last message its model wrote.",
        args: &[
            positional("id", "The session or worker."),
            flag("last", "N", "The last N items: messages, commands, tools."),
            switch(
                "screen",
                "The turn under way (or the last one), from its message on.",
            ),
        ],
        available: true,
        schema: include_str!("../schemas/read.schema.json"),
    },
    Command {
        name: "start",
        summary: "Start a worker of this session.",
        details: "The worker is a plain CLI session in this session's project, on its own \
                  branch in a new worktree by default. A Chat's worker works in the repository \
                  `--repo` names, or in a folder of its own with no git. It shows only inside \
                  this session, never in the sidebar. Prints its id; `brigadier wait` hears \
                  what it does: keep one running in the background.",
        args: &[
            flag(
                "title",
                "TEXT",
                "A short name for it; from the prompt by default.",
            ),
            choice(
                "agent",
                "AGENT",
                &["claude", "codex"],
                "Which CLI runs it; this session's by default.",
            ),
            flag(
                "model",
                "MODEL",
                "The model id; the CLI's own default by default.",
            ),
            flag("effort", "EFFORT", "The reasoning effort."),
            flag(
                "work",
                "KIND",
                "A kind of work from `brigadier models`: its row (the user's, else the shipped \
                 one) picks the agent, model and effort you don't give.",
            ),
            switch(
                "worktree",
                "Work in a new worktree of its own (the default).",
            ),
            switch(
                "local",
                "Work in this session's checkout (a Chat's: the --repo checkout).",
            ),
            flag(
                "repo",
                "PATH",
                "A Chat's worker: the repository it works in. Without it, a folder of its own.",
            ),
            flag(
                "base",
                "BRANCH",
                "The branch its worktree starts from and merges back into; this session's \
                 current branch by default.",
            ),
            choice(
                "permission",
                "MODE",
                &["ask", "accept-edits", "full-access"],
                "The CLI's own permission mode; this session's by default.",
            ),
            switch(
                "lean",
                "A research worker: the goal and the repository, with none of your MCP \
                 servers, no skills and no memory writes. Your instruction files still load.",
            ),
            Arg::Flag {
                long: "skill",
                value: Some("NAME"),
                help: "With --lean: a skill it keeps.",
                choices: &[],
                repeat: true,
            },
            Arg::Flag {
                long: "with",
                value: Some("WHAT"),
                help: "With --lean: computer-use gives it the computer tools while Settings › \
                       Computer use is on (every other worker has them then).",
                choices: &["computer-use"],
                repeat: true,
            },
            rest("prompt", "Its task.", true),
        ],
        available: true,
        schema: include_str!("../schemas/start.schema.json"),
    },
    Command {
        name: "send",
        summary: "Send a worker of yours or another session a message, or answer a worker's \
                  permission prompt.",
        details: "A message is queued into its turn if it is working, else it starts a turn. \
                  Another session (an id of `brigadier sessions --all`) gets it as a message \
                  from this session, never as its user's, and may answer with `brigadier send` \
                  back. Message another session only when the user asks, or when it is \
                  absolutely needed. Another session's workers are out of reach. --allow or \
                  --deny answers the permission prompt a worker's CLI shows (a `needs-input` \
                  event of `brigadier wait`).",
        args: &[
            positional("id", "The worker, or another session."),
            switch("allow", "Allow what its permission prompt asks."),
            switch("deny", "Deny what its permission prompt asks."),
            rest("text", "The message.", false),
        ],
        available: true,
        schema: include_str!("../schemas/send.schema.json"),
    },
    Command {
        name: "wait",
        summary: "Wait for an event from this session's workers.",
        details: "Returns at the first: a turn ended (with its last message), needs input, \
                  failed, interrupted, gone, usage limit, fresh context, quiet (20 minutes \
                  without a word while it works). Without ids, any of them. Each event is \
                  heard once, kept across a restart. Keep one running in the background while \
                  workers run: it wakes you when it returns. Given sessions instead, it returns \
                  at the end of their turn (once it took what you sent it), with its last \
                  message.",
        args: &[
            rest("id", "The workers, or the sessions, to wait for.", false),
            flag("timeout", "SECONDS", "Give up after this long."),
        ],
        available: true,
        schema: include_str!("../schemas/wait.schema.json"),
    },
    Command {
        name: "close",
        summary: "Close one of this session's workers.",
        details: "Stops its CLI and removes it from `brigadier workers`. Unfinished changes \
                  are kept as a commit on its branch.",
        args: &[positional("id", "The worker.")],
        available: true,
        schema: include_str!("../schemas/close.schema.json"),
    },
    Command {
        name: "continue-on",
        summary: "Move this session to another CLI, when the user asks.",
        details: "Its CLI writes the hand-off note, then a new session starts on the other CLI \
                  at once, on the first model the user's rules allow there, and carries on: \
                  the same thread, with a fresh context. Called from your own turn, pass \
                  --after-turn: it moves once that turn ends. Refused when that CLI isn't \
                  signed in or the rules allow no model on it. A worker is moved by its \
                  session.",
        args: &[
            Arg::Positional {
                name: "agent",
                help: "The CLI to continue on.",
                required: true,
                rest: false,
                choices: &["claude", "codex"],
            },
            switch("after-turn", "Move once the running turn ends."),
        ],
        available: true,
        schema: include_str!("../schemas/continue-on.schema.json"),
    },
    Command {
        name: "preview",
        summary: "Start, stop or read a preview the user can watch.",
        details: "`start <command…>` runs a dev server, app, simulator, PDF or test run as a \
                  preview the user sees (run in the foreground, no trailing `&`); `stop \
                  [<id>]` ends one, or every running one; `log [<id>]` reads the end of its \
                  output, the latest one's by default. Ids look like `preview-1`. It reads \
                  Starting until it is ready: say it runs only once `log` says so. Running \
                  Brigadier itself, give it a new BRIGADIER_DATA_DIR under /tmp and a dev \
                  identity, never the installed app's.",
        args: &[
            action(&["start", "stop", "log"], "What to do."),
            flag("name", "NAME", "start: a short name the user sees."),
            choice(
                "kind",
                "KIND",
                &["web", "app", "simulator", "pdf", "test"],
                "start: what it shows; read from the command by default.",
            ),
            flag(
                "workdir",
                "DIR",
                "start: the folder it runs in, relative to the checkout.",
            ),
            flag(
                "target",
                "TEXT",
                "start: the simulator's device or the PDF's path, when the command doesn't say.",
            ),
            Arg::Flag {
                long: "env",
                value: Some("NAME=VALUE"),
                help: "start: an environment variable for it.",
                choices: &[],
                repeat: true,
            },
            choice(
                "purpose",
                "WHY",
                &["proof", "user"],
                "start: `user` when the user asked to see it (a merge leaves it running); \
                 `proof` by default.",
            ),
            flag("tail", "N", "log: the last N lines (40 by default)."),
            flag(
                "wait",
                "SECONDS",
                "log: while it is starting, wait up to this long for it to run or end.",
            ),
            rest(
                "args",
                "start: the command; stop and log: the preview's id.",
                false,
            ),
        ],
        available: true,
        schema: include_str!("../schemas/preview.schema.json"),
    },
    Command {
        name: "browser",
        summary: "Drive Brigadier's browser.",
        details: "`open <url>` opens a tab and prints its id; `navigate <url>`, `read`, \
                  `click <ref>`, `type <ref> <text…>`, `screenshot`, `close` and `show` act \
                  on the tab `--tab` names. `read` lists the page's controls by ref (`e12`). \
                  Set `--asks` when a click or typing pays, sends something in the user's name \
                  or signs in: the user is asked first.",
        args: &[
            action(
                &[
                    "open",
                    "navigate",
                    "read",
                    "click",
                    "type",
                    "screenshot",
                    "close",
                    "show",
                ],
                "What to do.",
            ),
            flag("tab", "TAB", "The tab, as `open` printed it."),
            switch(
                "keep",
                "open: the page is for the user, so the tab stays open.",
            ),
            choice(
                "go",
                "STEP",
                &["back", "forward", "reload"],
                "navigate: instead of a URL.",
            ),
            switch("submit", "type: press Enter after typing."),
            choice(
                "asks",
                "WHAT",
                &["pay", "send", "sign-in"],
                "click and type: what it does that asks the user first.",
            ),
            flag(
                "out",
                "FILE",
                "screenshot: where to save it; the session's temporary folder by default.",
            ),
            rest(
                "args",
                "The URL, the control's ref, and the text to type.",
                false,
            ),
        ],
        available: true,
        schema: include_str!("../schemas/browser.schema.json"),
    },
    Command {
        name: "ask",
        summary: "Ask the user on a card and wait for the answer.",
        details: "The card always takes a free-text answer next to any options. Give 2 to 4 \
                  options or none, the recommended one first, so \"yes\" takes it. Waits \
                  until the user answers; run it in the background for a long wait.",
        args: &[
            Arg::Flag {
                long: "option",
                value: Some("TEXT"),
                help: "An option to offer; repeat for more, recommended first.",
                choices: &[],
                repeat: true,
            },
            flag(
                "title",
                "TEXT",
                "A few words over the card (\"Round 2 · Budget\").",
            ),
            rest("question", "The question.", true),
        ],
        available: true,
        schema: include_str!("../schemas/ask.schema.json"),
    },
    Command {
        name: "app",
        summary: "Read and act in the Brigadier app.",
        details: "`state` lists this session's tabs and terminals; `read-terminal \
                  <terminal>` reads one; `open <what> [<target>]` shows the user `file \
                  <path>`, `diff last-turn|branch|uncommitted`, `output <id>`, `url <url>`, \
                  `worker <id>` or `page <id>`; `terminal-run \
                  <command…>` types a command in a terminal tab; `show-session <id>` shows \
                  another session; `close-tab <tab>` closes a tab.",
        args: &[
            action(
                &[
                    "state",
                    "read-terminal",
                    "open",
                    "terminal-run",
                    "show-session",
                    "close-tab",
                ],
                "What to do.",
            ),
            flag("line", "N", "open file: the line to show."),
            flag(
                "terminal",
                "TERMINAL",
                "terminal-run: one of the session's terminals; your own by default.",
            ),
            switch(
                "show",
                "open and terminal-run: the user asked to see it, so it comes to the front.",
            ),
            rest("args", "The action's target.", false),
        ],
        available: true,
        schema: include_str!("../schemas/app.schema.json"),
    },
    Command {
        name: "page",
        summary: "Read and write the page the user's message came from, or the page or file \
                  beside a chat.",
        details: "`read <page>` prints the page and its revision; `write <page> --revision \
                  <rev> <text…>` replaces its text (Markdown, without its title), refused if \
                  it changed since that revision. The page's id is in the `[page …]` note on \
                  the message, or the note of a chat beside a page. A chat beside a file: \
                  `file-read` prints the file, its path and its revision; `file-write \
                  --revision <rev> <text…>` replaces its whole text, refused if it changed \
                  since. With no text, or `-`, the text comes from standard input.",
        args: &[
            action(&["read", "write", "file-read", "file-write"], "What to do."),
            flag(
                "revision",
                "REV",
                "write and file-write: the revision `read` or `file-read` printed.",
            ),
            flag("title", "TEXT", "write: a new title."),
            rest("args", "The page, and the text to write.", false),
        ],
        available: true,
        schema: include_str!("../schemas/page.schema.json"),
    },
    Command {
        name: "ledger",
        summary: "Add to, close, show or check against your own session's ledger.",
        details: "`add <text…>` appends one Markdown list entry, keeping the existing list \
                  marker, stamped `L<n> · <MM-DD HH:MM> · gen <g> ·`, and prints its id. Ids \
                  count up and are never reused, `ledger-archive.md` included. With no text, \
                  or `-`, reads standard input. Give the entry's kind and source (which \
                  message, command or file): `Rule from the user: \"…\" (message n)`, \
                  `Decision: …` or `I decided: …`, `Open: a promise or a question` (close it \
                  by id when it's done), or `Ruled out: …` / a failed check / an exact \
                  identifier / what you learned from reading (file:line). \
                  `close <id> \"<how>\"` appends a closing entry; \
                  nothing is rewritten. `show` prints the ledger verbatim; `--open` only the \
                  open items not closed, `--rules` only the rules and decisions still standing \
                  (the archive's too). `check <file>` lists the rule and decision ids the file \
                  never cites, the ids it cites that are closed, superseded or unknown, and the \
                  entries with no id; it judges nothing. Threads, workers and Chats use their \
                  own record/ledger.md; chats beside a tab cannot keep a ledger.",
        args: &[
            action(&["add", "close", "show", "check"], "What to do."),
            switch("open", "show: the open items not closed."),
            switch("rules", "show: the rules and decisions still standing."),
            rest(
                "args",
                "add: the entry's text; close: the id and how; check: the file.",
                false,
            ),
        ],
        available: true,
        schema: include_str!("../schemas/ledger.schema.json"),
    },
    Command {
        name: "checkpoint",
        summary: "Set or show your session's checkpoint.",
        details: "`set <text…>` replaces this session's checkpoint: goal, open requests with \
                  Brain item ids, files changed, last test and command, next step. With no \
                  text, or `-`, reads standard input. `show` prints the current checkpoint. \
                  Only a session's thread keeps one.",
        args: &[
            action(&["set", "show"], "What to do."),
            rest("args", "set: the checkpoint's text.", false),
        ],
        available: true,
        schema: include_str!("../schemas/checkpoint.schema.json"),
    },
    Command {
        name: "brain",
        summary: "Search the project's records: every session's transcript, ledger and notes.",
        details: "`search <query…>` finds the newest matching entries of the records the \
                  project's sessions keep: their CLI transcripts across every fresh context \
                  (with the whole outputs the CLI saved), their workers', their ledgers, \
                  ledger archives and hand-off notes. Each hit names its session, its \
                  item id or file and line, and a text window; a ledger entry learned from \
                  reading points at the file it cites, marked STALE once that file changed. \
                  `read <session> <from> <to>` gives that session's transcript items from \
                  <from> to <to> (at most 50) verbatim, a saved output whole. \
                  `read <session> --item <id>` reads one exact message, call or output. \
                  Filters also allow `read <session> --role tool --tool Bash`. \
                  Offset and limit apply separately to each item; excerpts report omitted bytes. \
                  A byte boundary within a UTF-8 character advances to its end.",
        args: &[
            action(&["search", "read"], "What to do."),
            flag(
                "session",
                "ID",
                "search: only this session and its workers (as hits name them).",
            ),
            flag(
                "limit",
                "N",
                "search: max hits (default 8); read: max UTF-8 bytes per item.",
            ),
            flag("item", "ID", "read: exact item id printed by search."),
            choice(
                "role",
                "ROLE",
                &["user", "assistant", "tool"],
                "Filter by item origin.",
            ),
            flag(
                "tool",
                "NAME",
                "Filter by exact CLI tool name (for example Bash or exec_command).",
            ),
            flag(
                "offset",
                "BYTES",
                "read: UTF-8 byte offset per item, from 0 (default 0).",
            ),
            rest(
                "args",
                "search: query; read: <session> --item <id>, or <session> <from> <to>.",
                false,
            ),
        ],
        available: true,
        schema: include_str!("../schemas/brain.schema.json"),
    },
    Command {
        name: "transcript",
        summary: "Search and read this session's own records.",
        details: "`search <words…>` is `brain search` within this session and its workers: \
                  your transcript across every fresh context, including what is no longer in \
                  your context, your ledger, its archive and your hand-off notes. `read \
                  <session> <from> <to>` gives items of this session's transcript (or a \
                  worker's) verbatim.",
        args: &[
            action(&["search", "read"], "What to do."),
            flag(
                "limit",
                "N",
                "search: max hits (default 8); read: max UTF-8 bytes per item.",
            ),
            flag("item", "ID", "read: exact item id printed by search."),
            choice(
                "role",
                "ROLE",
                &["user", "assistant", "tool"],
                "Filter by item origin.",
            ),
            flag(
                "tool",
                "NAME",
                "Filter by exact CLI tool name (for example Bash or exec_command).",
            ),
            flag(
                "offset",
                "BYTES",
                "read: UTF-8 byte offset per item, from 0 (default 0).",
            ),
            rest(
                "args",
                "search: query; read: <session> --item <id>, or <session> <from> <to>.",
                false,
            ),
        ],
        available: true,
        schema: include_str!("../schemas/transcript.schema.json"),
    },
    Command {
        name: "why",
        summary: "Which session turn wrote a line, and what was asked and answered then.",
        details: "`why <file>:<line>` (or `<file> --line N`; the path from this folder) walks \
                  the turn snapshots of the sessions that worked in that checkout, newest \
                  first, to the turn that wrote the line as it is now, through later moves. It \
                  prints that turn's session, number and time, the user's message whole, the \
                  reply's start with the command that reads it whole, and the ledger entries \
                  stamped in that turn, read from the CLI's own session files. A worker's line \
                  names the worker; a line no turn wrote names `git blame`'s commit.",
        args: &[
            positional("file", "The file, with :<line> unless --line gives it."),
            flag("line", "N", "The line, from 1."),
        ],
        available: true,
        schema: include_str!("../schemas/why.schema.json"),
    },
    Command {
        name: "code",
        summary: "Search the repository's code index.",
        details: "Instant, and kept current as files change. `search <query…>` finds symbol \
                  definitions by name (functions, types, classes, methods) and files by path, \
                  best matches first: use it before grepping. `refs <symbol>` lists where a \
                  symbol is defined and used, matched by name without type information. `map` \
                  shows the repository at a glance: top folders, modules and their \
                  dependencies, package manifests, scripts, services and their ports, and the \
                  most used definitions per module.",
        args: &[
            action(&["search", "refs", "map"], "What to do."),
            choice(
                "kind",
                "KIND",
                &["symbol", "file", "any"],
                "search: what to find; `any` by default.",
            ),
            flag(
                "language",
                "LANGUAGE",
                "search: only this language, e.g. rust, typescript, python.",
            ),
            flag(
                "path",
                "DIR",
                "search: only under this repository-relative folder.",
            ),
            flag(
                "limit",
                "N",
                "search and refs: at most this many (30 hits, 50 references by default).",
            ),
            rest(
                "args",
                "search: a name or path fragment; refs: the symbol's name.",
                false,
            ),
        ],
        available: true,
        schema: include_str!("../schemas/code.schema.json"),
    },
    Command {
        name: "library",
        summary: "The skills and MCP servers every agent has: list, add, remove, sign in.",
        details: "The Library is what Settings › Plugins adds: each item goes to every agent \
                  through the agent's own setup, so a plain `claude` or `codex` in a terminal \
                  has it too. `list` shows each item, where it's on and how each agent \
                  stands with it. `add <what…>` takes what you'd paste in Add: an `mcp add` \
                  line of any agent (after `--`, as it has flags of its own), server JSON, a \
                  server URL, a skill folder, a GitHub link or a skills.sh name, or `-` to \
                  read it from standard input; it goes in every project, or with `--project` \
                  only in that one, for you alone unless `--shared`. `remove <name>` takes \
                  it from every agent and project. `login <name>` signs every agent that \
                  needs it in to a server, each with its own sign-in, in the user's browser. \
                  A worker can only list.",
        args: &[
            action(&["list", "add", "remove", "login"], "What to do."),
            flag(
                "project",
                "DIR",
                "list: how the agents stand there; add: only in this project (`.` for this folder).",
            ),
            flag(
                "name",
                "NAME",
                "add: its name, when what you give has none.",
            ),
            switch(
                "shared",
                "add, with --project: for the whole team, in the project's committed files.",
            ),
            rest(
                "args",
                "add: what to add; remove and login: the item's name.",
                false,
            ),
        ],
        available: true,
        schema: include_str!("../schemas/library.schema.json"),
    },
    Command {
        name: "report",
        summary: "Keep a research answer's full report as a page of the user's.",
        details: "`save --title <title> <markdown…>`: everything the short answer leaves out, \
                  in Markdown (without its title), with its sources. With no text, or `-`, \
                  the text comes from standard input. It prints the link to put in your \
                  answer ([Full report](brigadier-page:<id>)); one click opens it. Never paste \
                  the report into the chat.",
        args: &[
            action(&["save"], "What to do."),
            flag(
                "title",
                "TEXT",
                "The report's title, a few words (\"AI models for 3D printing\").",
            ),
            rest("markdown", "The whole report.", false),
        ],
        available: true,
        schema: include_str!("../schemas/report.schema.json"),
    },
    Command {
        name: "feedback",
        summary: "Offer the user a report to Brigadier's developers, on a card.",
        details: "`offer --kind bug|idea --title <title> <body…>`: when the user seems annoyed \
                  with Brigadier itself (never their own project's code), or asks you to report \
                  something to its developers. The card's Send files it on GitHub as the user; \
                  nothing is sent without their click. body: what happened and what they \
                  expected, steps if known, a few plain lines in their words; no secrets, \
                  paths or project code. With no body, or `-`, it comes from standard input. \
                  One offer per incident: unless --asked, refused while an earlier one is \
                  unanswered or after they dismissed one.",
        args: &[
            action(&["offer"], "What to do."),
            choice(
                "kind",
                "KIND",
                &["bug", "idea"],
                "bug for something wrong, idea for something missing.",
            ),
            flag("title", "TEXT", "One line naming the problem."),
            switch("asked", "The user asked for it (\"report this\")."),
            rest("body", "What happened and what they expected.", false),
        ],
        available: true,
        schema: include_str!("../schemas/feedback.schema.json"),
    },
    Command {
        name: "approval",
        summary: "Ask the user to approve what only they may decide.",
        details: "`request --action <what> <details…>`: spending money, handling credentials \
                  yourself (a token, a password, the keychain directly), or destroying \
                  something outside this session's own work. Not for pushes, pull requests or \
                  deploys, nor for work inside the session. Returns at once; the decision \
                  arrives later as a message.",
        args: &[
            action(&["request"], "What to do."),
            flag("action", "TEXT", "What would happen, in one line."),
            rest("details", "Why, and what exactly.", true),
        ],
        available: true,
        schema: include_str!("../schemas/approval.schema.json"),
    },
    Command {
        name: "models",
        summary: "Models, usage and guidance for picking a worker's model.",
        details: "What each CLI reports, usage and reset times per account, the shipped \
                  guidance and the user's own rules, in one view.",
        args: &[],
        available: true,
        schema: include_str!("../schemas/models.schema.json"),
    },
];

/// The schema of every failed call's `--json` output.
pub const ERROR_SCHEMA: &str = include_str!("../schemas/error.schema.json");

/// The subcommand called `name`.
pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.name == name)
}

impl Command {
    /// `brigadier read <id> [--last N] [--screen] [--json]`.
    pub fn usage(&self) -> String {
        let mut usage = format!("brigadier {}", self.name);
        for arg in self.args {
            if let Arg::Positional {
                name,
                required,
                rest,
                choices,
                ..
            } = arg
            {
                let word = if choices.is_empty() {
                    format!("<{name}>")
                } else {
                    choices.join("|")
                };
                let word = if *rest { format!("{word}…") } else { word };
                usage.push(' ');
                usage.push_str(&if *required { word } else { format!("[{word}]") });
            }
        }
        for arg in self.args {
            if let Arg::Flag { long, value, .. } = arg {
                usage.push_str(&match value {
                    Some(value) => format!(" [--{long} {value}]"),
                    None => format!(" [--{long}]"),
                });
            }
        }
        usage.push_str(" [--json]");
        usage
    }
}

/// `brigadier --help`.
pub fn help() -> String {
    let width = COMMANDS.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut out = String::from(
        "brigadier: read and drive Brigadier from a session.\n\n\
         Usage: brigadier <command> [arguments] [--json]\n\nCommands:\n",
    );
    for command in COMMANDS {
        let soon = if command.available {
            ""
        } else {
            " (not available yet)"
        };
        out.push_str(&format!(
            "  {:width$}  {}{soon}\n",
            command.name, command.summary
        ));
    }
    out.push_str(
        "\nEvery command takes --json for machine-readable output.\n\
         `brigadier <command> --help` explains one command.\n\
         Exit codes: 0 done, 1 failed, 2 wrong arguments, 3 refused (no valid session), \
         4 not available yet, 5 Brigadier not reachable.\n",
    );
    out
}

/// `brigadier <command> --help`.
pub fn command_help(command: &Command) -> String {
    let mut out = format!("{}\n\nUsage: {}\n", command.summary, command.usage());
    if !command.details.is_empty() {
        out.push_str(&format!("\n{}\n", command.details));
    }
    if !command.available {
        out.push_str("\nNot available yet in this version of Brigadier.\n");
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    for arg in command.args {
        rows.push(match arg {
            Arg::Positional {
                name,
                help,
                choices,
                ..
            } => {
                let name = if choices.is_empty() {
                    format!("<{name}>")
                } else {
                    choices.join("|")
                };
                (name, (*help).to_owned())
            }
            Arg::Flag {
                long,
                value,
                help,
                choices,
                repeat,
            } => {
                let name = match value {
                    Some(value) => format!("--{long} {value}"),
                    None => format!("--{long}"),
                };
                let mut help = (*help).to_owned();
                if !choices.is_empty() {
                    help.push_str(&format!(" One of: {}.", choices.join(", ")));
                }
                if *repeat {
                    help.push_str(" Repeatable.");
                }
                (name, help)
            }
        });
    }
    rows.push(("--json".into(), "Machine-readable output.".into()));
    let width = rows.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    out.push_str("\nArguments:\n");
    for (name, help) in rows {
        out.push_str(&format!("  {name:width$}  {help}\n"));
    }
    out
}
