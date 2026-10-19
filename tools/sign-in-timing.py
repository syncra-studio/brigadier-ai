#!/usr/bin/env python3
"""Times an agent's own sign-in the way Connect runs it, in a scratch config folder.

    python3 -I tools/sign-in-timing.py claude     # or codex

It runs `claude auth login` (or `codex login`) with no terminal and a scratch
CLAUDE_CONFIG_DIR / CODEX_HOME, opens the page it asks for, and prints, from the moment it
started: when the link came, each line it printed, when its auth status first says signed in,
and when it exited. Confirm in the browser when the page opens. Your own logins are never
touched: the scratch folder, and the Keychain item Claude Code makes for it, are removed at the
end (--keep leaves them).
"""

import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time

AGENTS = {
    "claude": {
        "binary": "claude",
        "home_var": "CLAUDE_CONFIG_DIR",
        "login": ["auth", "login"],
        "status": ["auth", "status", "--json"],
        "signed_in": lambda out, code: '"loggedIn": true' in out,
    },
    "codex": {
        "binary": "codex",
        "home_var": "CODEX_HOME",
        "login": ["login"],
        "status": ["login", "status"],
        "signed_in": lambda out, code: code == 0 and "Logged in" in out,
    },
}


def keychain_items():
    """The Claude Code credential items in the login keychain, by service name."""
    out = subprocess.run(
        ["security", "dump-keychain"], capture_output=True, text=True
    ).stdout
    return {
        line.split("=", 1)[1].strip().strip('"')
        for line in out.splitlines()
        if '"svce"' in line and "Claude Code-credentials" in line
    }


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    keep = "--keep" in sys.argv
    if len(args) != 1 or args[0] not in AGENTS:
        sys.exit(__doc__)
    agent = AGENTS[args[0]]
    binary = shutil.which(agent["binary"], path=os.path.expanduser("~/.local/bin") + ":" + os.environ["PATH"])
    if not binary:
        sys.exit(f"{agent['binary']} isn't installed")

    home = tempfile.mkdtemp(prefix=f"sign-in-timing-{args[0]}-")
    link_file = os.path.join(home, "link")
    browser = os.path.join(home, "browser")
    with open(browser, "w") as f:
        f.write(f"#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{link_file}'\n")
    os.chmod(browser, 0o700)
    env = dict(os.environ, BROWSER=browser, NO_COLOR="1")
    env[agent["home_var"]] = home
    items_before = keychain_items() if sys.platform == "darwin" else set()

    events = []
    started = time.monotonic()

    def note(what):
        events.append((time.monotonic() - started, what))
        print(f"{events[-1][0]:8.3f}s  {what}", flush=True)

    login = subprocess.Popen(
        [binary, *agent["login"]],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    note(f"started {' '.join([agent['binary'], *agent['login']])}")

    def read():
        for line in login.stdout:
            line = line.strip()
            if line:
                note(f"printed: {line[:100]}")

    def open_link():
        while login.poll() is None:
            if os.path.exists(link_file):
                link = open(link_file).read().split()[0]
                note("link passed back; opening it")
                subprocess.Popen(["open" if sys.platform == "darwin" else "xdg-open", link])
                return
            time.sleep(0.02)

    def watch_status():
        while True:
            done = subprocess.run(
                [binary, *agent["status"]], env=env, capture_output=True, text=True
            )
            if agent["signed_in"](done.stdout + done.stderr, done.returncode):
                note("auth status says signed in")
                return
            if login.poll() is not None and time.monotonic() - started > 1:
                return
            time.sleep(0.1)

    threads = [threading.Thread(target=t, daemon=True) for t in (read, open_link, watch_status)]
    for t in threads:
        t.start()
    try:
        code = login.wait(timeout=15 * 60)
        note(f"exited with {code}")
        threads[2].join(timeout=5)
    except (KeyboardInterrupt, subprocess.TimeoutExpired):
        login.terminate()
        note("stopped")
    finally:
        if not keep:
            if sys.platform == "darwin":
                for item in keychain_items() - items_before:
                    subprocess.run(["security", "delete-generic-password", "-s", item], capture_output=True)
            shutil.rmtree(home, ignore_errors=True)
        else:
            print(f"kept {home}")

    signed = next((at for at, what in events if what.startswith("auth status")), None)
    success = next((at for at, what in events if "successful" in what.lower()), None)
    exited = next((at for at, what in events if what.startswith("exited")), None)
    print()
    if success is not None and exited is not None:
        print(f"said signed in -> exited: {exited - success:.3f}s")
    if signed is not None and exited is not None:
        print(f"status signed in -> exited: {exited - signed:.3f}s")


if __name__ == "__main__":
    main()
