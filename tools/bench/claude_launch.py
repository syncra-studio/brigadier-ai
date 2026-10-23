#!/usr/bin/env python3
"""Bench-only Claude launcher, selected in the isolated daemon's agentPaths."""
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
        if config["exclude_dynamic"]:
            arguments = [EXCLUDE_DYNAMIC, *arguments]
        override = config["worker_cache_ttl"] if role == "worker" else None
        if override:
            env["CLAUDE_CODE_PROMPT_CACHE_TTL"] = override
        with Path(config["audit"]).open("a") as audit:
            audit.write(json.dumps(dict(session=session, role=role,
                                        exclude_dynamic=config["exclude_dynamic"],
                                        cache_ttl_override=override,
                                        cache_ttl_env=env.get("CLAUDE_CODE_PROMPT_CACHE_TTL"))) + "\n")
    os.execve(config["binary"], [config["binary"], *arguments], env)


if __name__ == "__main__":
    launch(sys.argv[1], sys.argv[2:])
