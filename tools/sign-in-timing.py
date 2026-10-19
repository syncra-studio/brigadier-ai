#!/usr/bin/env python3
"""Times an agent's own sign-in the way Connect runs it, in a scratch config folder.

    python3 -I tools/sign-in-timing.py claude     # or codex

It runs `claude auth login` (or `codex login`) with no terminal and a scratch
CLAUDE_CONFIG_DIR / CODEX_HOME, opens the page it asks for, and prints, from the moment it
started: when the link came, each line it printed, when its login was stored (where it keeps it
changed: Claude Code's Keychain item on macOS, else its credentials file; Codex's auth.json), when
the app's Connect would say Connected (one status check right after that, as the app does), when
its auth status first says signed in, and when it exited. Confirm in the browser when the page opens. Your own logins are never
touched: the scratch folder, and the Keychain item Claude Code makes for it, are removed at the
end (--keep leaves them).
"""

import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unicodedata

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


def keychain_item(home, env):
    """Claude Code's Keychain service and account for the scratch config dir `home`."""
    # Verified in Claude Code 2.1.303: D1 hashes the NFC config path (not realpath),
    # and Vx uses USER or os.userInfo().username, with this account-name fallback.
    digest = hashlib.sha256(unicodedata.normalize("NFC", home).encode()).hexdigest()[:8]
    suffix = "-custom-oauth" if env.get("CLAUDE_CODE_CUSTOM_OAUTH_URL") else ""
    import pwd

    try:
        account = env.get("USER") or pwd.getpwuid(os.geteuid()).pw_name
    except (KeyError, OSError):
        account = "claude-code-user"
    if not re.fullmatch(r"[a-zA-Z0-9._-]+", account):
        account = "claude-code-user"
    return f"Claude Code{suffix}-credentials-{digest}", account


def login_mark(agent, home, env):
    """Where the login stands now, as the app reads it (dates only, never the credential)."""
    stamp = []
    if agent == "claude" and sys.platform == "darwin":
        service, account = keychain_item(home, env)
        found = subprocess.run(
            ["security", "find-generic-password", "-s", service, "-a", account],
            capture_output=True, text=True,
        )
        dates = [line.strip() for line in found.stdout.splitlines()
                 if line.strip().startswith(('"cdat"', '"mdat"'))]
        stamp.append(" ".join(dates) if found.returncode == 0 else "none")
    path = os.path.join(home, ".credentials.json" if agent == "claude" else "auth.json")
    try:
        meta = os.stat(path)
        stamp.append(f"{meta.st_mtime_ns}:{meta.st_size}")
    except FileNotFoundError:
        stamp.append("none")
    return "\n".join(stamp)


def cleanup_keychain(agent, home, env):
    """Delete only Claude's credential for this scratch config, never discovered items."""
    # Codex defaults to auth.json in CODEX_HOME; removing its scratch folder suffices.
    if sys.platform != "darwin" or agent != "claude":
        return

    service, account = keychain_item(home, env)
    if (not home or not os.path.isabs(home) or env.get("CLAUDE_CONFIG_DIR") != home
            or env.get("CLAUDE_SECURESTORAGE_CONFIG_DIR", home) != home):
        print(f"Skipped Keychain cleanup: storage is not the scratch config. "
              f"Check scratch item {service!r} manually.")
        return

    try:
        result = subprocess.run(
            ["security", "delete-generic-password", "-s", service, "-a", account],
            capture_output=True,
        )
        if result.returncode in (0, 44):  # Deleted, or no item (e.g. cancelled login).
            return
    except OSError:
        pass
    print(f"Could not clean up Keychain item {service!r}, account {account!r}; "
          "remove it manually if present.")


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

    events = []
    before = login_mark(args[0], home, env)
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

    def watch_mark():
        # As the app's Connect does: look every 0.1 s; once it changes, ask the status (a few
        # times, 0.25 s apart, should it lag) and that is when Connected shows.
        while login.poll() is None:
            if login_mark(args[0], home, env) != before:
                note("login stored")
                for attempt in range(4):
                    done = subprocess.run(
                        [binary, *agent["status"]], env=env, capture_output=True, text=True
                    )
                    if agent["signed_in"](done.stdout + done.stderr, done.returncode):
                        note("app shows Connected (status asked once the login was stored)")
                        return
                    time.sleep(0.25)
                return
            time.sleep(0.1)

    threads = [threading.Thread(target=t, daemon=True)
               for t in (read, open_link, watch_status, watch_mark)]
    for t in threads:
        t.start()
    try:
        code = login.wait(timeout=15 * 60)
        note(f"exited with {code}")
        threads[2].join(timeout=5)
        threads[3].join(timeout=5)
    except (KeyboardInterrupt, subprocess.TimeoutExpired):
        login.terminate()
        note("stopped")
    finally:
        if not keep:
            cleanup_keychain(args[0], home, env)
            shutil.rmtree(home, ignore_errors=True)
        else:
            print(f"kept {home}")

    stored = next((at for at, what in events if what == "login stored"), None)
    connected = next((at for at, what in events if what.startswith("app shows")), None)
    signed = next((at for at, what in events if what.startswith("auth status")), None)
    success = next((at for at, what in events if "successful" in what.lower()), None)
    exited = next((at for at, what in events if what.startswith("exited")), None)
    print()
    if stored is not None and connected is not None:
        print(f"login stored -> app shows Connected: {connected - stored:.3f}s")
    if stored is not None and success is not None:
        print(f"login stored -> said signed in: {success - stored:.3f}s (before: Connected at that line or the next 3 s check)")
    if success is not None and exited is not None:
        print(f"said signed in -> exited: {exited - success:.3f}s")
    if signed is not None and exited is not None:
        print(f"status signed in -> exited: {exited - signed:.3f}s")


if __name__ == "__main__":
    main()
