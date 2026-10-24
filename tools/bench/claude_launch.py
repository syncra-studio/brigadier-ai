#!/usr/bin/env python3
"""Read-only bench audit of the shipped Claude launcher, selected in the isolated daemon's agentPaths."""
import hashlib
import json
import os
from pathlib import Path
import sys

EXCLUDE_DYNAMIC = "--exclude-dynamic-system-prompt-sections"


def launch(config_path, arguments):
    config = json.loads(Path(config_path).read_text())
    env = dict(os.environ)
    session = env.get("BRIGADIER_SESSION")
    # Discovery/version checks must behave exactly like the real executable.
    if session and ("-p" in arguments or "--print" in arguments):
        parent = config.get("parent")
        if not parent:
            raise RuntimeError("Bench parent session was not registered before CLI launch")
        role = "parent" if session == parent else "worker"
        standing = arguments[arguments.index("--append-system-prompt") + 1] if "--append-system-prompt" in arguments else None
        with Path(config["audit"]).open("a") as audit:
            audit.write(json.dumps(dict(session=session, role=role,
                                        exclude_dynamic=EXCLUDE_DYNAMIC in arguments,
                                        cache_ttl_env=env.get("CLAUDE_CODE_PROMPT_CACHE_TTL"),
                                        standing_sha256=hashlib.sha256(standing.encode()).hexdigest() if standing is not None else None,
                                        standing_placeholders=standing is not None and all("$" + name in standing for name in ("BRIGADIER_SESSION_DIR", "BRIGADIER_LEDGER")),
                                        standing_env_present=all(env.get(name) for name in ("BRIGADIER_SESSION_DIR", "BRIGADIER_LEDGER")))) + "\n")
    os.execve(config["binary"], [config["binary"], *arguments], env)


if __name__ == "__main__":
    launch(sys.argv[1], sys.argv[2:])
