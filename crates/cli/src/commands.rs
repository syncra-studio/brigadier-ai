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
    /// The checked-in schema of its `--json` output (`schemas/<name>.schema.json`; `state`
    /// prints what `context` does, so it shares that one).
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
        name: "pr",
        summary: "Watch, forget or list this session's pull requests and their history.",
        details: "Workers act on their parent session. Watch requires a canonical HTTPS PR URL. \
                  --account selects the named gh login for this host/repository; credentials are never recorded. \
                  Forget suppresses automatic rediscovery until explicitly watched again. Merged/closed PRs stay in history.",
        args: &[
            action(&["watch", "forget", "list"], "The action."),
            rest(
                "args",
                "watch: PR URL; forget: canonical key or URL; list: no arguments.",
                false,
            ),
            flag(
                "account",
                "LOGIN",
                "watch: named gh account, required to fetch status.",
            ),
        ],
        available: true,
        schema: include_str!("../schemas/pr.schema.json"),
    },
    Command {
        name: "waiting",
        summary: "Add, complete or list explicit non-PR waits for this session.",
        details: "Workers create waits in their parent session. Durable IDs belong to that session; Done is idempotent. \
                  --by YYYY-MM-DD means midnight after that day in the stored IANA timezone (system timezone by default). \
                  RFC3339 instants are also accepted. Use pr watch for GitHub pull request links.",
        args: &[
            action(&["add", "done", "list"], "The action."),
            rest(
                "args",
                "add: plain words; done: durable wait ID and optional completion note; list: no arguments.",
                false,
            ),
            flag("by", "DATE", "YYYY-MM-DD or RFC3339 instant."),
            flag(
                "timezone",
                "IANA",
                "Timezone for interpreting a civil date.",
            ),
            flag("then", "TEXT", "What to do after the wait."),
            flag("link", "URL", "Related non-PR HTTP(S) link."),
        ],
        available: true,
        schema: include_str!("../schemas/waiting.schema.json"),
    },
    Command {
        name: "state",
        summary: "Everything you need now, in one call: workers, brief, rules, open items, \
                  routing (run it after any compaction).",
        details: "The same as `brigadier context` with no part: your workers with their state \
                  and progress, and each small part whole; the big parts (whole ledger, \
                  transcript, skills, MCP servers, project files) listed with the command that \
                  opens them.",
        args: &[],
        available: true,
        schema: include_str!("../schemas/context.schema.json"),
    },
    Command {
        name: "skill",
        summary: "The skills of your session's setup, or one skill's whole text.",
        details: "Alone it lists the skills and whether your CLI loaded each at launch; with a \
                  name it prints that skill whole. The same as `brigadier context skills`. A \
                  worker's CLI loads none: it pulls the one it needs here.",
        args: &[Arg::Positional {
            name: "name",
            help: "The skill to print whole; none lists them.",
            required: false,
            rest: false,
            choices: &[],
        }],
        available: true,
        schema: include_str!("../schemas/context.schema.json"),
    },
    Command {
        name: "context",
        summary: "Everything about your job, on pull: brief, decisions, open items, ledger, \
                  transcript, skills, MCP servers, project, routing.",
        details: "Nothing of it is put into your conversation (D13): run `brigadier context` \
                  after any compaction, then open what you need. Alone it \
                  lists each part with its size and the command that opens it. `brief`: your \
                  task as `brigadier start` gave it, or a session's first request. \
                  `decisions`: the rules and decisions still standing, quoted with their ids. \
                  `open`: open items and the cards waiting on the user. `ledger`: the whole \
                  ledger. A worker reads its session's ledger (read-only) and then its own. \
                  `checkpoint`: the session's checkpoint. `transcript`: your own records, the \
                  last items, `--grep` hits or one `--item`; a worker's `--session` reads its \
                  session's thread. `skills [<name>]`: the skills of your session's setup, or \
                  one skill's whole text (a worker's CLI loads none: it pulls one here). \
                  `mcps`: the setup's MCP servers and which were loaded at your launch; \
                  `brigadier mcp` calls any of them. `project`: the checkout's own CLAUDE.md and AGENTS.md (D17) and the \
                  project's facts. `routing`: `brigadier models`' guidance.",
        args: &[
            Arg::Positional {
                name: "part",
                help: "The part to open; none lists them.",
                required: false,
                rest: false,
                choices: &[
                    "waiting",
                    "brief",
                    "decisions",
                    "open",
                    "ledger",
                    "checkpoint",
                    "transcript",
                    "skills",
                    "mcps",
                    "project",
                    "routing",
                ],
            },
            Arg::Positional {
                name: "name",
                help: "skills: the skill to print whole.",
                required: false,
                rest: false,
                choices: &[],
            },
            flag(
                "grep",
                "TEXT",
                "transcript: search your records for these words.",
            ),
            flag("last", "N", "transcript: the last N items (20 by default)."),
            flag(
                "item",
                "ID",
                "transcript: one exact item, as a search hit names it, or positions as \
                 `--last` prints them (`7`, `0-8`), earlier fresh contexts' too.",
            ),
            switch(
                "session",
                "transcript: a worker reads its session's thread instead of its own records.",
            ),
        ],
        available: true,
        schema: include_str!("../schemas/context.schema.json"),
    },
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
                  removed, how full its context is, what git shows of its work (its commits, \
                  merged or not, behind, conflicts, uncommitted files) and the commands its \
                  latest turn ran with their exit codes. These are the ids `send`, `wait` and \
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
                "A short name for its job: two to four plain words, like \"Fix login redirect\", \
                 never a place like \"repo root\"; from the prompt by default.",
            ),
            flag(
                "agent",
                "AGENT",
                "Which CLI runs it, by its id; this session's by default.",
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
                "A kind of work from `brigadier models`: its row (the user's, else the automatic \
                 one) picks the agent, model and effort you don't give.",
            ),
            Arg::Flag {
                long: "reviewing",
                value: Some("ID"),
                help: "An adversarial review of a worker's work, or `session` for this \
                       session's own (its plan): the work \"Adversarial review\" on another \
                       ready CLI than the author's, else a fresh session of the author's. Its \
                       brief is told to read `brigadier show <id>` and `brigadier context`.",
                choices: &[],
                repeat: true,
            },
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
            Arg::Flag {
                long: "mcp",
                value: Some("NAME"),
                help: "An MCP server it gets, by the name `brigadier context mcps` lists; none \
                       by default. A worker starts with none of your setup but the project's \
                       instruction file (D13, D18).",
                choices: &[],
                repeat: true,
            },
            Arg::Flag {
                long: "with",
                value: Some("WHAT"),
                help: "computer-use gives it the computer tools while Settings › Computer use \
                       is on.",
                choices: &["computer-use"],
                repeat: true,
            },
            switch(
                "lean",
                "Every worker is naked now (D13): accepted and ignored.",
            ),
            Arg::Flag {
                long: "skill",
                value: Some("NAME"),
                help: "Gone: a worker has no skills (D13); it reads one with `brigadier skill \
                       <name>`.",
                choices: &[],
                repeat: true,
            },
            switch(
                "mockup",
                "The session's mockup, for a design the user wants to see before it is built: \
                 it saves its screens as mockup-1.png, mockup-2.png… in the folder its first \
                 message names, they show in the session's Mockup tab, and the user's notes on \
                 them come back to it.",
            ),
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
                  Another session (its title, as the user names it, or an id of `brigadier sessions \
                  --all`) gets it as a message \
                  from this session, never as its user's, and answers with `brigadier send` \
                  back (`brigadier help sessions`). Message another session only when the user \
                  asks, or when it is absolutely needed. Another session's workers are out of \
                  reach. A worker's \
                  permission prompt (a `needs-input` event of `brigadier wait`) reaches only \
                  you, never the user: judge it yourself and answer it with --allow, or \
                  --deny <reason>; the worker waits until you do.",
        args: &[
            positional("id", "The worker, or another session (its id or title)."),
            switch("allow", "Allow what its permission prompt asks."),
            switch(
                "deny",
                "Deny what its permission prompt asks; the text is the reason its CLI is told.",
            ),
            rest("text", "The message, or a --deny's reason.", false),
        ],
        available: true,
        schema: include_str!("../schemas/send.schema.json"),
    },
    Command {
        name: "wait",
        summary: "Wait for an event from this session's workers.",
        details: "Returns at the first: a turn ended (with its last message, and what git and \
                  its CLI showed: its commits, merged or not, uncommitted files, the commands \
                  it ran and how each exited), needs input, \
                  failed, interrupted, gone, usage limit, quiet (20 minutes \
                  without a word while it works), stalled (a background command with no new \
                  output and almost no CPU for 10 minutes: look at it, say so in the thread, \
                  and don't wait on it), starved (a compile held up 10 minutes behind the build \
                  slots or on an overloaded machine: not hung; say so, and pause other work if \
                  yours is what loads it), or a message sent to you meanwhile, so you read \
                  it now (`message`). Without ids, any of them. Each event is \
                  heard once, kept across a restart. Keep one running in the background while \
                  workers run: it wakes you when it returns. Given sessions instead, it returns \
                  at the end of their turn (once it took what you sent it), with its last \
                  message; or with its answer, when it sends you one meanwhile (given here only, not \
                  again as a message), or `removed` when its user deleted your message unread.",
        args: &[
            rest("id", "The workers, or the sessions, to wait for.", false),
            flag("timeout", "SECONDS", "Give up after this long."),
        ],
        available: true,
        schema: include_str!("../schemas/wait.schema.json"),
    },
    Command {
        name: "close",
        summary: "End one of this session's workers for good, its work kept on its branch (the user's \"end it\").",
        details: "Stops its CLI and removes it from `brigadier workers`. Unfinished changes \
                  are kept as a commit on its branch.",
        args: &[positional("id", "The worker.")],
        available: true,
        schema: include_str!("../schemas/close.schema.json"),
    },
    Command {
        name: "pause",
        summary: "Pause one of this session's workers (the user's \"stop it\", \"pause it\").",
        details: "Its turn ends now, as Esc ends it, and its CLI session is kept. What you \
                  send it meanwhile waits for `brigadier resume`; `brigadier wait` doesn't \
                  wait on it.",
        args: &[
            positional("id", "The worker."),
            rest("note", "What it should know when it goes on.", false),
        ],
        available: true,
        schema: include_str!("../schemas/pause.schema.json"),
    },
    Command {
        name: "resume",
        summary: "Resume a paused worker (the user's \"carry on\"), with an optional note.",
        details: "It gets what you sent it while paused and the note, then \"Continue where \
                  you stopped.\", in one turn of the same session: one call, no `send` first.",
        args: &[
            positional("id", "The worker."),
            rest("note", "What it should know as it goes on.", false),
        ],
        available: true,
        schema: include_str!("../schemas/resume.schema.json"),
    },
    Command {
        name: "switch",
        summary: "Move a worker that didn't switch on its own to another account, agent, \
                  model or effort.",
        details: "Without flags: the next account or routing row that can take it, in the \
                  order a usage limit uses. A working worker, or one waiting for a reset, goes \
                  on there at once with \"Continue where you stopped.\"; a paused one stays \
                  paused until `brigadier resume`.",
        args: &[
            positional("id", "The worker."),
            flag("agent", "AGENT", "The CLI it moves to."),
            flag(
                "model",
                "MODEL",
                "The model; another agent's default when not given.",
            ),
            flag("effort", "EFFORT", "The reasoning effort."),
        ],
        available: true,
        schema: include_str!("../schemas/switch.schema.json"),
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
                help: "The CLI to continue on, by its id.",
                required: true,
                rest: false,
                choices: &[],
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
                "start: `user` when the user asked to see it (a merge leaves it running, and a \
                 web page opens its own tab behind the user's, where Settings › Browser sends \
                 local links to Brigadier); `proof` by default.",
            ),
            flag(
                "open",
                "PATH",
                "start: the page to open on a web preview, a path on its own server starting \
                 with one `/` (`/docs/page.html?x=1`); its address is the server's origin plus \
                 this, whatever the server prints.",
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
                  `events` tells what happened in the tab: its pages, network and console. \
                  `eval <script>` runs JavaScript in the page of a tab you work in, on any site: \
                  an expression, or statements that `return`; `await` works. The user sees the \
                  script on a card and says yes or no each time, and the answer comes back as text. \
                  Password fields are hidden from all of these and can't be typed into: the \
                  user fills them in. \
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
                    "events",
                    "eval",
                ],
                "What to do.",
            ),
            flag("tab", "TAB", "The tab, as `open` printed it."),
            switch(
                "keep",
                "open: the page is for the user, so the tab stays open.",
            ),
            switch(
                "dev-url",
                "open: what `$BROWSER` asks for a dev server: refused unless the address is \
                 local and the user's local links open in Brigadier.",
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
                "kinds",
                "KINDS",
                "events: only these, by commas, of navigation, network, console, error.",
            ),
            flag(
                "since",
                "N",
                "events: only what came after this, as the last answer's `--since` said.",
            ),
            flag(
                "out",
                "FILE",
                "screenshot: the file to save it to, in your working folder and outside .git \
                 and the agents' settings (anywhere at Full access); the session's temporary \
                 folder by default.",
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
        summary: "Ask the user questions on one card and wait for the answers.",
        details: "Your CLI's own question tool asks on the same card; this is for a worker's \
                  question you pass up, a CLI without one, or a round bigger than its one call \
                  takes (harness D7, D36). Ask everything you can at once: one card per round, \
                  never one card per question; only questions that depend on its answers wait \
                  for the next round. One question: `brigadier ask \"<question>\" --option A \
                  --option B`. Several: `--question \"<q>\"` for each, each followed \
                  by its own `--option`s. Each question takes 2 to 8 options or none; label the one you \
                  recommend \"… (Recommended)\", first, so \"yes\" takes it; the card always takes a free-text answer \
                  too. Prints every question with its answer, once the user answers them \
                  all; run it in the background for a long wait.",
        args: &[
            Arg::Flag {
                long: "question",
                value: Some("TEXT"),
                help: "A question of the card, each followed by its own --options.",
                choices: &[],
                repeat: true,
            },
            Arg::Flag {
                long: "option",
                value: Some("TEXT"),
                help: "An option for the question before it; the one you recommend first, labelled \"… (Recommended)\".",
                choices: &[],
                repeat: true,
            },
            flag(
                "title",
                "TEXT",
                "A few words over the card, after the round number Brigadier counts (\"Budget\").",
            ),
            rest(
                "question",
                "One question, as words (instead of --question).",
                false,
            ),
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
                  another session; `close-tab <tab>` closes a tab; `visuals` reads the Chat drawing guide.",
        args: &[
            action(
                &[
                    "state",
                    "read-terminal",
                    "open",
                    "terminal-run",
                    "show-session",
                    "close-tab",
                    "visuals",
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
            flag(
                "item",
                "ID",
                "read: exact item id printed by search, or positions (`7`, `0-8`).",
            ),
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
        name: "mcp",
        summary: "Call any MCP server of your session's setup, loaded at your launch or not.",
        details: "The same on every CLI, for a session's thread and every worker, with no \
                  restart (D5). `list`: the setup's servers, and which your CLI loaded at \
                  launch. `tools <server>`: its tools, each with its input schema. `call \
                  <server> <tool> [<json>]`: runs one tool with the JSON object of its \
                  arguments (none: `{}`) and prints all it \
                  answered; an error it answers exits non-zero. One connection per server is \
                  kept for you while you use it, so what a call opens or returns stays good \
                  for your next call. `mcp run` is the local wrapper the CLIs' configs name.",
        args: &[
            action(&["list", "tools", "call"], "What to do."),
            rest(
                "args",
                "tools: <server>; call: <server> <tool> [<json>].",
                false,
            ),
        ],
        available: true,
        schema: include_str!("../schemas/mcp.schema.json"),
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
            flag(
                "item",
                "ID",
                "read: exact item id printed by search, or positions (`7`, `0-8`).",
            ),
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
                  with Brigadier itself (never their own project's code), when Brigadier, a CLI, \
                  you or a worker acts abnormally (a bug, stuck, looping, repeating, ending \
                  wrong), or when the user asks you to report something to its developers. The \
                  card's Send files it on GitHub as the user; nothing is sent without their \
                  click. body: what happened and what they \
                  expected, steps if known, a few plain lines in their words; no secrets, \
                  paths or project code. With no body, or `-`, it comes from standard input. \
                  One offer per incident: unless --asked, refused for a report still unanswered \
                  on its card or that they dismissed.",
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
        name: "fusion",
        summary: "Offer Fusion in a top-level Chat; only the user starts it.",
        details: "`offer <question>`: offer one answer from every installed CLI, merged into one reply. Nothing starts until the user clicks Ask. Only top-level Chats; refused in sessions, side chats, tab chats and workers. The question may come from standard input.",
        args: &[
            action(&["offer"], "What to do."),
            rest(
                "question",
                "The question to ask every installed CLI.",
                false,
            ),
        ],
        available: true,
        schema: include_str!("../schemas/fusion.schema.json"),
    },
    Command {
        name: "session",
        summary: "Propose work that should run apart as a new session, on a card.",
        details: "`propose --title <title> <task…>`: work that should run as a session of its \
                  own (a request outside this one, or one the user wants run apart). The user \
                  sees a card with Start as session; one click starts it on its own branch and \
                  worktree in this project, with the task as its first message. Nothing starts \
                  until they click: never start it yourself or do it here. task: complete on \
                  its own, as that session's first message. With no task, or `-`, it comes from \
                  standard input. The same task once a request.",
        args: &[
            action(&["propose"], "What to do."),
            flag(
                "title",
                "TEXT",
                "A short name for the card (\"Fix CI and run nightly\"); the task's start by \
                 default.",
            ),
            rest("task", "What the new session is to do.", false),
        ],
        available: true,
        schema: include_str!("../schemas/session.schema.json"),
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
                  guidance, the user's own rules and their advice notes, in one view. A rule \
                  capping or raising effort (for every model, one agent or one model) moves a \
                  start's effort to its limit; a start is refused only by a model rule, or when \
                  a cap and a minimum can't both be met. Advice restricts nothing: weigh it \
                  when you pick.",
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

/// A how-to page of `brigadier help <topic>`: what a session's instructions point to.
#[derive(Debug)]
pub struct Topic {
    pub name: &'static str,
    pub summary: &'static str,
    pub text: &'static str,
}

/// Every topic, in the order `brigadier help` lists them.
pub const TOPICS: &[Topic] = &[
    Topic {
        name: "orchestrate",
        summary: "Who does what: you, your subagents, your workers; reporting.",
        text: include_str!("help/orchestrate.md"),
    },
    Topic {
        name: "workers",
        summary: "Which worker, one short job, waves and every `wait` event.",
        text: concat!(
            include_str!("help/workers.md"),
            "\n",
            include_str!("help/briefs.md"),
            "\n",
            include_str!("help/routing.md")
        ),
    },
    Topic {
        name: "approvals",
        summary: "Answering a worker's permission prompt yourself.",
        text: include_str!("help/approvals.md"),
    },
    Topic {
        name: "routing",
        summary: "Which agent, model and effort a worker runs on.",
        text: include_str!("help/routing.md"),
    },
    Topic {
        name: "briefs",
        summary: "A worker's brief: five lines, and what it carries.",
        text: include_str!("help/briefs.md"),
    },
    Topic {
        name: "asking",
        summary: "Grilling the user, and what always needs their yes.",
        text: include_str!("help/asking.md"),
    },
    Topic {
        name: "integrate",
        summary: "Merging workers' steps, watching what ends later, gotchas.",
        text: include_str!("help/integrate.md"),
    },
    Topic {
        name: "reviews",
        summary: "Plan reviews and end verification.",
        text: include_str!("help/reviews.md"),
    },
    Topic {
        name: "context",
        summary: "What to pull, and where it comes from.",
        text: include_str!("help/context.md"),
    },
    Topic {
        name: "commands",
        summary: "The commands an orchestrator uses, with their flags.",
        text: include_str!("help/commands.md"),
    },
    // The reply rule (a session's answer goes to a `wait` on it, never also as a message) and a
    // removed message ending that `wait` are proposed (goal 4, D122).
    Topic {
        name: "sessions",
        summary: "Messaging another session, and answering one.",
        text: include_str!("help/sessions.md"),
    },
    Topic {
        name: "fusion",
        summary: "Running a Fusion the user started in a top-level Chat.",
        text: include_str!("help/fusion.md"),
    },
];

/// The topic called `name`.
pub fn topic(name: &str) -> Option<&'static Topic> {
    TOPICS.iter().find(|topic| topic.name == name)
}

/// `brigadier help <topic>`: its page, then the help of the command of the same name.
pub fn topic_help(topic: &Topic) -> String {
    match find(topic.name) {
        Some(command) => format!(
            "{}\n`brigadier {}` itself:\n{}",
            topic.text,
            command.name,
            command_help(command)
        ),
        None => topic.text.to_owned(),
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
    out.push_str("\nHow-to topics (`brigadier help <topic>`):\n");
    for topic in TOPICS {
        out.push_str(&format!("  {:width$}  {}\n", topic.name, topic.summary));
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
