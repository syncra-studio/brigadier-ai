#!/usr/bin/env python3
"""Minimal Brigadier IPC client: 4-byte big-endian length + JSON frames.
The protocol version is the one the arm's daemon was built with: `<data>/ab-protocol` (written by
startd.sh), else BRIG_PROTO, else this checkout's crates/ipc. usage: bipc.py <data-dir> '<request json>'"""
import socket, struct, json, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))


def protocol_of(src):
    text = open(os.path.join(src, "crates/ipc/src/protocol.rs")).read()
    return int(re.search(r"PROTOCOL_VERSION: u32 = (\d+);", text).group(1))


def protocol(data):
    pinned = os.path.join(data, "ab-protocol")
    if os.path.exists(pinned):
        return int(open(pinned).read().strip())
    if os.environ.get("BRIG_PROTO"):
        return int(os.environ["BRIG_PROTO"])
    return protocol_of(os.path.join(HERE, "..", ".."))


def frame(s, o):
    b = json.dumps(o).encode(); s.sendall(struct.pack(">I", len(b)) + b)


def read(s):
    h = b""
    while len(h) < 4:
        c = s.recv(4 - len(h))
        if not c: return None
        h += c
    n = struct.unpack(">I", h)[0]; b = b""
    while len(b) < n:
        c = s.recv(n - len(b))
        if not c: return None
        b += c
    return json.loads(b)


def connect(data):
    s = socket.socket(socket.AF_UNIX); s.connect(data + "/run/brigadierd.sock")
    frame(s, {"type": "hello", "token": open(data + "/run/ipc.token").read().strip(),
              "protocol": protocol(data), "client": {"name": "tools-ab", "pid": os.getpid()}})
    return s, read(s)


_id = [0]


def call(s, req):
    _id[0] += 1; i = _id[0]
    frame(s, {"type": "request", "id": i, "request": req})
    while True:
        r = read(s)
        if r is None: return None
        if r.get("type") == "response" and r.get("id") == i:
            return r["result"]


def req(data, req_):
    s, _ = connect(data)
    try: return call(s, req_)
    finally: s.close()


if __name__ == "__main__":
    data = sys.argv[1]; r = json.loads(sys.argv[2])
    print(json.dumps(req(data, r), indent=1)[:int(os.environ.get("MAXC", "6000"))])
