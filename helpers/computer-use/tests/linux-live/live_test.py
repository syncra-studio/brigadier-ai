"""The Linux computer-use live test (started by run.sh inside the container).

Drives brigadier-cu over stdio MCP, the way Brigadier starts it for an agent, against real GTK
apps on Xvfb under GNOME's window manager. Every step checks the tool's reply and the effect
on the app (readback, the file on disk, a dialog's own output). A sampler reads the active
window and the pointer every 25 ms; any change while the helper works fails that step.
Prints a step table and exits 1 when any step fails.
"""
import json
import os
import re
import subprocess
import sys
import threading
import time

from mcp_client import Mcp

BIN = os.environ["BRIGADIER_CU_BIN"]
OUT = os.environ.get("LINUX_LIVE_OUT", "/tmp")
WORK = "/tmp/linux-live"
# mutter runs every step. openbox and xfwm4 run the launch, the reads and the real-input steps:
# with their default settings they focus every new window, whatever the launcher asks (#694).
WM = os.environ.get("LINUX_LIVE_WM", "mutter")
MACOS_WORDS = re.compile(r"Safari|Finder|Apple|macOS|\bAX\b|AppKit|Cmd\b|⌘|\bDock\b|Spotlight|TCC")

rows = []


def x(*argv):
    return subprocess.run(argv, capture_output=True, text=True).stdout.strip()


class Desk:
    """The active X window and the core pointer, sampled every 25 ms."""

    def __init__(self):
        self.samples = []
        threading.Thread(target=self.run, daemon=True).start()

    @staticmethod
    def now():
        pointer = x("xdotool", "getmouselocation").split()[:2]
        return x("xdotool", "getactivewindow"), " ".join(pointer)

    def run(self):
        while True:
            self.samples.append(self.now())
            time.sleep(0.025)

    def changes(self, start, baseline):
        seen = []
        for sample in self.samples[start:]:
            if sample != baseline and sample not in seen:
                seen.append(sample)
        return seen


desk = None
mcp = None


def step(name, tool, args, check, settle=0.4, front_may_flash=False):
    """One tool call. `check(err, text, sc, images)` returns a failure reason or None.
    `front_may_flash`: the active window may move while the call runs (a window manager that
    focuses new windows) but must be back when it returns; the pointer never moves."""
    baseline, start = Desk.now(), len(desk.samples)
    try:
        err, text, sc, images = mcp.call(tool, args)
    except Exception as error:  # the helper died or answered garbage
        err, text, sc, images = True, f"no reply: {error}", None, 0
    time.sleep(settle)  # a window manager focuses a new window a beat late
    try:
        problem = check(err, text, sc or {}, images)
    except Exception as error:
        problem = f"check raised {error!r}"
    moved = desk.changes(start, baseline)
    if front_may_flash:
        flashed = any(sample[0] != baseline[0] for sample in moved)
        moved = [sample for sample in moved if sample[1] != baseline[1]]
        if Desk.now() != baseline:
            moved.append(Desk.now())
        if flashed and "restored" not in text:
            problem = (problem + "; " if problem else "") + "the front moved and the reply does not say so"
    if moved:
        problem = (problem + "; " if problem else "") + f"front/pointer changed {baseline} -> {moved[:3]}"
    name = name if WM == "mutter" else f"{name} [{WM}]"
    rows.append((name, problem is None, text, problem, len(desk.samples) - start))
    # A failure prints the whole reply: CI's log is all there is to debug it from.
    print(f"[{'PASS' if problem is None else 'FAIL'}] {name}: {short(text) if problem is None else text}"
          + (f"\n       {problem}" if problem else ""), flush=True)
    return err, text, sc or {}


def short(text, limit=160):
    text = " ".join(text.split())
    return text if len(text) <= limit else text[: limit - 1] + "…"


def windows_of(pid, title_part="", wait=6.0):
    """The pid's windows whose title contains `title_part`, polled through list_windows."""
    until = time.time() + wait
    while True:
        _, _, sc, _ = mcp.call("list_windows", {"pid": pid})
        found = [w for w in (sc or {}).get("windows", []) if title_part in w.get("title", "")]
        if found or time.time() > until:
            return found
        time.sleep(0.3)


def gone(pid, window_id, wait=4.0):
    """The window closed, polled through list_windows."""
    until = time.time() + wait
    while any(w["window_id"] == window_id for w in windows_of(pid, wait=0)):
        if time.time() > until:
            return False
        time.sleep(0.3)
    return True


def rows_of(sc):
    """element label -> element_token of a get_window_state read with tree_format elements/both."""
    return {e.get("label"): e["element_token"] for e in sc.get("elements", []) if e.get("label")}


def user_opens(argv, title):
    """An app the user opens (it takes the front), after which the user goes back to the terminal."""
    proc = subprocess.Popen(argv, stdout=subprocess.PIPE, text=True)
    until = time.time() + 6
    while not x("xdotool", "search", "--name", f"^{title}$") and time.time() < until:
        time.sleep(0.2)
    time.sleep(0.5)
    focus_user_terminal()
    return proc


def focus_user_terminal():
    x("xdotool", "windowactivate", "--sync", USER_WINDOW)
    x("xdotool", "mousemove", "1000", "700")
    time.sleep(0.3)


def expect(condition, reason):
    return None if condition else reason


def main():
    global desk, mcp, USER_WINDOW
    os.makedirs(WORK, exist_ok=True)
    notes = os.path.join(WORK, "notes.txt")
    with open(notes, "w") as file:
        file.write("Hello world\n")

    until = time.time() + 20
    while "window id" not in x("xprop", "-root", "_NET_SUPPORTING_WM_CHECK") and time.time() < until:
        time.sleep(0.2)
    time.sleep(1)  # mutter claims the screen before it manages windows
    # The user's own front window: a terminal, with the pointer resting on the desktop.
    # In the bottom-right corner, so the apps behind it keep points it doesn't cover.
    terminal = subprocess.Popen(["xterm", "-geometry", "60x12-0-0", "-title", "user-terminal"])
    until = time.time() + 10
    while not x("xdotool", "search", "--all", "--pid", str(terminal.pid), "--name", "^user-terminal$") and time.time() < until:
        time.sleep(0.2)
    USER_WINDOW = x("xdotool", "search", "--all", "--pid", str(terminal.pid), "--name", "^user-terminal$").split()[0]
    focus_user_terminal()
    if x("xdotool", "getactivewindow") != USER_WINDOW:
        return finish("the user's terminal could not be made the active window")
    desk = Desk()

    env = dict(
        os.environ,
        BRIGADIER_CU_AUDIENCE="brigadier",
        BRIGADIER_CU_HOME=os.path.join(WORK, "cu-home"),
        BRIGADIER_CU_DAEMON_APP=BIN,
        BRIGADIER_CU_SOCKET_AUTH_TOKEN="linux-live-token",
        BRIGADIER_CU_EXTERNAL_PERMISSION_FLOW="1",
        BRIGADIER_CU_DEFAULT_SESSION="brigadier-linux-live",
    )
    mcp = Mcp([BIN, "mcp"], env, open(os.path.join(OUT, "brigadier-cu.stderr.log"), "w"))

    # 1. The tool list agents get.
    listing = mcp.request("tools/list", {})
    wire = len(json.dumps(listing, separators=(",", ":"), ensure_ascii=False).encode())
    names = [tool["name"] for tool in listing["tools"]]
    words = sorted(set(MACOS_WORDS.findall(json.dumps(listing["tools"]))))
    problem = None
    if len(names) != 20 or wire >= 28_000 or words:
        problem = f"{len(names)} tools, {wire} bytes, macOS words {words}"
    rows.append(("tools/list", problem is None, f"{len(names)} tools, {wire} bytes, macOS words {words}", problem, 0))
    print(f"[{'PASS' if problem is None else 'FAIL'}] tools/list: {rows[-1][2]}", flush=True)

    # 2. launch_app in the background.
    # openbox and xfwm4 focus every new window: the front may flash, and the reply must say so.
    _, _, sc = step("launch_app mousepad", "launch_app", {"name": "mousepad", "additional_arguments": [notes]},
                    lambda e, t, sc, i: expect(not e and sc.get("pid") and sc.get("active") is False, "not launched in the background"),
                    settle=1.0, front_may_flash=WM != "mutter")
    pid = sc.get("pid")
    editor = windows_of(pid, "notes.txt") if pid else []
    if not editor:
        return finish("mousepad's window never appeared")
    wid = editor[0]["window_id"]

    # 3. Reads: a screenshot when asked, then the tree alone (a new read stales older tokens).
    step("get_window_state (screenshot asked)", "get_window_state", {"pid": pid, "window_id": wid, "include_screenshot": True, "query": "Hello"},
         lambda e, t, sc, i: expect(not e and i == 1, f"error={e} images={i}"))
    _, text, sc = step("get_window_state (tree, no screenshot)", "get_window_state", {"pid": pid, "window_id": wid, "tree_format": "both"},
                       lambda e, t, sc, i: expect(not e and i == 0 and "Hello world" in t, f"error={e} images={i}"))
    text_token = next((el["element_token"] for el in sc.get("elements", []) if "Hello world" in (el.get("value") or el.get("label") or "")), None)

    # 4. A shortcut whose menu item is disabled (nothing to save yet) says so and sends nothing.
    step("hotkey ctrl+s while Save is disabled", "hotkey", {"pid": pid, "window_id": wid, "keys": ["ctrl", "s"]},
         lambda e, t, sc, i: expect(e and "disabled" in t, "not refused with the disabled menu item as the reason"))

    # 5. type_text at the caret keeps the document, confirmed by readback.
    if text_token is None:
        return finish("no text element in mousepad's tree")
    typed = "Brigadier was here. "
    step("type_text at the caret", "type_text", {"element_token": text_token, "text": typed},
         lambda e, t, sc, i: expect(not e and sc.get("effect") == "confirmed" and typed in t and "Hello world" in t, "not confirmed with both texts"))

    # 5. A save shortcut, confirmed on disk.
    def saved(e, t, sc, i):
        until = time.time() + 3
        while time.time() < until:
            content = open(notes).read()
            if typed in content and "Hello world" in content:
                return expect(not e, "saved but the reply is an error")
            time.sleep(0.2)
        return f"file on disk is {open(notes).read()!r}"
    step("hotkey ctrl+s (save)", "hotkey", {"pid": pid, "window_id": wid, "keys": ["ctrl", "s"]}, saved)

    # 5b. Real input in the background, no menu item or button involved: a click at a point of
    # the text puts the caret there, then Return, a chord no menu carries (ctrl+End) and Return
    # again. The saved file shows each landed where it should.
    _, _, sc = step("get_window_state (text frame)", "get_window_state",
                    {"pid": pid, "window_id": wid, "tree_format": "elements", "include_screenshot": True},
                    lambda e, t, sc, i: expect(not e and i == 1, f"error={e} images={i}"))
    text_frame = next((el.get("screenshot_frame") for el in sc.get("elements", [])
                       if "Hello world" in (el.get("value") or el.get("label") or "") and el.get("screenshot_frame")), None)
    if text_frame is None:
        return finish("no screenshot frame for mousepad's text")
    if WM == "mutter":
        # shortcut: GTK3 under mutter reports its screen extents with the window's origin added
        # twice (#704); undo it here until that is fixed.
        geometry = x("xwininfo", "-id", str(wid))
        text_frame = dict(text_frame,
                          x=text_frame["x"] - int(re.search(r"Absolute upper-left X:\s+(-?\d+)", geometry).group(1)),
                          y=text_frame["y"] - int(re.search(r"Absolute upper-left Y:\s+(-?\d+)", geometry).group(1)))
    landed = lambda e, t, sc, i: expect(not e, f"refused (effect {sc.get('effect')})")
    # Real input: openbox and xfwm4 activate the window it lands in (core click-to-focus), and
    # the helper puts the user's window back; the reply must say so.
    flash = WM != "mutter"
    step("click at x/y inside the text (no control there)", "click",
         {"pid": pid, "window_id": wid, "x": text_frame["x"] + 2, "y": text_frame["y"] + 4}, landed, front_may_flash=flash)
    step("press_key return (no menu item)", "press_key", {"pid": pid, "window_id": wid, "key": "return"}, landed, front_may_flash=flash)
    step("hotkey ctrl+end (no menu item)", "hotkey", {"pid": pid, "window_id": wid, "keys": ["ctrl", "end"]}, landed, front_may_flash=flash)
    step("press_key return at the end", "press_key", {"pid": pid, "window_id": wid, "key": "return"}, landed, front_may_flash=flash)
    before_keys = open(notes).read()
    def saved_keys(e, t, sc, i):
        until = time.time() + 3
        while time.time() < until:
            # The click put the caret before the first character; ctrl+end moved it to the end.
            if open(notes).read() == "\n" + before_keys + "\n":
                return expect(not e, "saved but the reply is an error")
            time.sleep(0.2)
        return f"file on disk is {open(notes).read()!r}, was {before_keys!r}"
    step("hotkey ctrl+s (save the keys)", "hotkey", {"pid": pid, "window_id": wid, "keys": ["ctrl", "s"]}, saved_keys)
    if WM != "mutter":
        step("close (invoke_menu File > Close Window, editor)", "invoke_menu", {"pid": pid, "window_id": wid, "path": ["File", "Close Window"]},
             lambda e, t, sc, i: expect(not e and gone(pid, wid), "the window is still open"))
        terminal.kill()
        return finish(None)

    # 6. invoke_menu opens a dialog behind the user's window; read it and close it by token.
    step("invoke_menu Help > About", "invoke_menu", {"pid": pid, "window_id": wid, "path": ["Help", "About"]},
         lambda e, t, sc, i: expect(not e and windows_of(pid, "About"), "no About window"))
    about = windows_of(pid, "About")
    if about:
        _, _, sc = step("get_window_state (dialog)", "get_window_state", {"pid": pid, "window_id": about[0]["window_id"], "tree_format": "both"},
                        lambda e, t, sc, i: expect(not e and "Close" in rows_of(sc), "no Close button in the dialog"))
        close = rows_of(sc).get("Close")
        if close:
            step("click by element token (dialog Close)", "click", {"element_token": close},
                 lambda e, t, sc, i: expect(not e and gone(pid, about[0]["window_id"]), "the dialog is still open"))

    # 7. A GTK4 dialog the user opened: type into its field and press OK by token.
    entry = user_opens(["zenity", "--entry", "--title=Rename", "--text=New name"], "Rename")
    dialog = windows_of(entry.pid)
    if dialog:
        _, _, sc = step("get_window_state (GTK4 dialog)", "get_window_state", {"pid": entry.pid, "window_id": dialog[0]["window_id"], "tree_format": "both"},
                        lambda e, t, sc, i: expect(not e and "OK" in rows_of(sc), "no OK button"))
        field = next((el["element_token"] for el in sc.get("elements", []) if "text" in el.get("role", "")), None)
        if field:
            step("type_text into the dialog field", "type_text", {"element_token": field, "text": "report-final"},
                 lambda e, t, sc, i: expect(not e and sc.get("effect") == "confirmed", "not confirmed"))
        if rows_of(sc).get("OK"):
            step("click OK by element token", "click", {"element_token": rows_of(sc)["OK"]},
                 lambda e, t, sc, i: expect(not e and entry.communicate(timeout=5)[0].strip() == "report-final", "the dialog did not return the typed name"))
    else:
        rows.append(("zenity entry dialog", False, "", "the dialog never appeared", 0))
    if entry.poll() is None:
        entry.kill()

    # 8. A coordinate click from the screenshot: the dialog answers, the pointer stays put.
    question = user_opens(["zenity", "--question", "--title=Keep", "--text=Keep the file?"], "Keep")
    dialog = windows_of(question.pid)
    if dialog:
        _, _, sc = step("get_window_state (screenshot for pixels)", "get_window_state",
                        {"pid": question.pid, "window_id": dialog[0]["window_id"], "tree_format": "elements", "include_screenshot": True},
                        lambda e, t, sc, i: expect(not e and i == 1, "no screenshot"))
        yes = next((el.get("screenshot_frame") for el in sc.get("elements", []) if el.get("label") == "Yes"), None)
        if yes:
            point = {"x": yes["x"] + yes["w"] / 2, "y": yes["y"] + yes["h"] / 2}

            def answered(e, t, sc, i):
                try:
                    code = question.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    code = None
                if code == 0:
                    return expect(not e, "clicked, but the reply is an error")
                return expect(e and "not" in t.lower(), f"the dialog did not answer (exit {code}) and the reply claims success")
            step("click at screenshot x/y", "click", {"pid": question.pid, "window_id": dialog[0]["window_id"], **point}, answered)
    if question.poll() is None:
        question.kill()

    # 9. The file manager: launch, read, click a toolbar button by token.
    _, _, sc = step("launch_app thunar", "launch_app", {"name": "thunar", "additional_arguments": [WORK]},
                    lambda e, t, sc, i: expect(not e and sc.get("pid"), "not launched"))
    fm_pid = sc.get("pid")
    folder = windows_of(fm_pid, "linux-live") if fm_pid else []
    if folder:
        fm = folder[0]["window_id"]
        _, _, sc = step("get_window_state (file manager)", "get_window_state", {"pid": fm_pid, "window_id": fm, "tree_format": "both"},
                        lambda e, t, sc, i: expect(not e and "Open Parent" in rows_of(sc), "no Open Parent button"))
        parent = rows_of(sc).get("Open Parent")
        if parent:
            step("click by element token (Open Parent)", "click", {"element_token": parent},
                 lambda e, t, sc, i: expect(not e and windows_of(fm_pid, "tmp", wait=3), "the folder did not change"))
        # 10. Close.
        step("close (invoke_menu File > Close Window)", "invoke_menu", {"pid": fm_pid, "window_id": fm, "path": ["File", "Close Window"]},
             lambda e, t, sc, i: expect(not e and gone(fm_pid, fm), "the window is still open"))
    else:
        rows.append(("thunar window", False, "", "the file manager's window never appeared", 0))
    step("close (invoke_menu File > Close Window, editor)", "invoke_menu", {"pid": pid, "window_id": wid, "path": ["File", "Close Window"]},
         lambda e, t, sc, i: expect(not e and gone(pid, wid), "the window is still open"))

    # 11. Terminals are refused with the real reason.
    term_pid = int(x("xdotool", "getwindowpid", USER_WINDOW))
    refused = lambda e, t, sc, i: expect(e and "terminal" in t.lower(), "not refused as a terminal")
    step("get_window_state on a terminal", "get_window_state", {"pid": term_pid, "window_id": int(USER_WINDOW)}, refused)
    step("type_text into a terminal", "type_text", {"pid": term_pid, "window_id": int(USER_WINDOW), "text": "echo hi"}, refused)
    step("launch_app a terminal", "launch_app", {"name": "xterm"}, refused)
    # 12. GNOME's window frames (the title bar buttons around every app's window) are refused.
    frames_pid = x("pgrep", "-f", "mutter-x11-frames").split()[0] if x("pgrep", "-f", "mutter-x11-frames") else ""
    frame = x("xdotool", "search", "--all", "--pid", frames_pid, "--name", "^user-terminal$").split() if frames_pid else []
    if frame:
        step("get_window_state on a window frame", "get_window_state", {"pid": int(frames_pid), "window_id": int(frame[0])},
             lambda e, t, sc, i: expect(e and "window manager's frame" in t, "not refused as the window manager's frame"))
    else:
        rows.append(("window frame", False, "", "no mutter-x11-frames window around the terminal", 0))
    terminal.kill()
    return finish(None)


def finish(fatal):
    if fatal:
        rows.append(("setup", False, "", fatal, 0))
    if mcp:
        mcp.close()
        subprocess.run([BIN, "stop"], env=mcp.env, capture_output=True)
    failed = [row for row in rows if not row[1]]
    report = ["| Step | Result | Reply (shortened) | Front/pointer samples |", "| --- | --- | --- | --- |"]
    for name, ok, text, problem, samples in rows:
        cell = short(text, 140).replace("|", "\\|")
        result = "pass" if ok else f"FAIL: {problem}".replace("|", "\\|")
        report.append(f"| {name} | {result} | {cell} | {samples} |")
    total = len(desk.samples) if desk else 0
    report.append(f"\n{len(rows) - len(failed)}/{len(rows)} steps passed; {total} active-window/pointer samples.")
    with open(os.path.join(OUT, f"linux-live-report-{WM}.md"), "w") as file:
        file.write("\n".join(report) + "\n")
    print("\n".join(report), flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
