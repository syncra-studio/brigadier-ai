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
        summary: "List this project's sidebar sessions.",
        details: "Each row: id, state, title, branch, lines added and removed, and how many \
                  workers it has. Workers are never listed here; see `brigadier workers`.",
        args: &[],
        available: true,
        schema: include_str!("../schemas/sessions.schema.json"),
    },
    Command {
        name: "workers",
        summary: "List this session's own workers.",
        details: "Each row: id, number, state, agent, model, title, branch and lines added and \
                  removed. These are the ids `send`, `wait` and `close` take.",
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
        details: "The worker is a plain CLI session in this session's project. It shows only \
                  inside this session, never in the sidebar. Prints its id.",
        args: &[
            flag("title", "TEXT", "A short name for it."),
            choice("agent", "AGENT", &["claude", "codex"], "Which CLI runs it."),
            flag("model", "MODEL", "The model id; see `brigadier models`."),
            flag("effort", "EFFORT", "The reasoning effort."),
            switch(
                "worktree",
                "Work in a new worktree of its own (the default).",
            ),
            switch("local", "Work in this session's checkout."),
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
                "The CLI's own permission mode.",
            ),
            rest("prompt", "Its task.", true),
        ],
        available: false,
        schema: include_str!("../schemas/start.schema.json"),
    },
    Command {
        name: "send",
        summary: "Send one of this session's workers a message.",
        details: "Queued into its turn if it is working, else it starts a turn.",
        args: &[
            positional("id", "The worker."),
            rest("text", "The message.", true),
        ],
        available: false,
        schema: include_str!("../schemas/send.schema.json"),
    },
    Command {
        name: "wait",
        summary: "Wait for an event from this session's workers.",
        details: "Returns at the first: a turn ended (with its last message), needs input, \
                  gone, usage limit, fresh context. Without ids, any of them. Run long waits \
                  in the background.",
        args: &[
            rest("id", "The workers to wait for.", false),
            flag("timeout", "SECONDS", "Give up after this long."),
        ],
        available: false,
        schema: include_str!("../schemas/wait.schema.json"),
    },
    Command {
        name: "close",
        summary: "Close one of this session's workers.",
        details: "Stops its CLI and removes it from this session.",
        args: &[positional("id", "The worker.")],
        available: false,
        schema: include_str!("../schemas/close.schema.json"),
    },
    Command {
        name: "preview",
        summary: "Start, stop or read a preview the user can watch.",
        details: "`start <command…>` runs a dev server, app or test run as a preview; `stop \
                  <name>` ends it; `log <name>` reads its output.",
        args: &[
            action(&["start", "stop", "log"], "What to do."),
            flag("name", "NAME", "start: a short name the user sees."),
            choice(
                "kind",
                "KIND",
                &["web", "app", "simulator", "pdf", "test"],
                "start: what it shows.",
            ),
            flag(
                "workdir",
                "DIR",
                "start: the folder it runs in, in the checkout.",
            ),
            flag("tail", "N", "log: the last N lines."),
            rest(
                "args",
                "start: the command; stop and log: the preview's name.",
                false,
            ),
        ],
        available: false,
        schema: include_str!("../schemas/preview.schema.json"),
    },
    Command {
        name: "browser",
        summary: "Drive Brigadier's browser.",
        details: "`open <url>`, `navigate <url>`, `read`, `click <target>`, `type <text>`, \
                  `screenshot`, `close`, `show`.",
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
            flag(
                "tab",
                "TAB",
                "The tab, as `open` named it; the latest by default.",
            ),
            rest("args", "The action's URL, target or text.", false),
        ],
        available: false,
        schema: include_str!("../schemas/browser.schema.json"),
    },
    Command {
        name: "ask",
        summary: "Ask the user on a card and wait for the answer.",
        details: "The card always takes a free-text answer next to any options. Give the \
                  recommended option first, so \"yes\" takes it.",
        args: &[
            Arg::Flag {
                long: "option",
                value: Some("TEXT"),
                help: "An option to offer; repeat for more, recommended first.",
                choices: &[],
                repeat: true,
            },
            rest("question", "The question.", true),
        ],
        available: false,
        schema: include_str!("../schemas/ask.schema.json"),
    },
    Command {
        name: "app",
        summary: "Read and act in the Brigadier app.",
        details: "`state` lists the app's tabs and terminals; `read-terminal`, `open`, \
                  `terminal-run`, `show-session` and `close-tab` act on them.",
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
            rest("args", "The action's target.", false),
        ],
        available: false,
        schema: include_str!("../schemas/app.schema.json"),
    },
    Command {
        name: "page",
        summary: "List, read and write the user's pages.",
        details: "`list`, `read <page>`, `write <page> <text…>`.",
        args: &[
            action(&["list", "read", "write"], "What to do."),
            rest("args", "The page, and the text to write.", false),
        ],
        available: false,
        schema: include_str!("../schemas/page.schema.json"),
    },
    Command {
        name: "models",
        summary: "Models, usage and guidance for picking a worker's model.",
        details: "What each CLI reports, usage and reset times per account, the shipped \
                  guidance and the user's own rules, in one view.",
        args: &[],
        available: false,
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
