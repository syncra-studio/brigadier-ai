"""A minimal stdio MCP client for brigadier-cu (stdlib only)."""
import json
import subprocess


class Mcp:
    def __init__(self, argv, env, stderr):
        self.env = env
        self.proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=stderr, env=env, text=True, bufsize=1)
        self.next_id = 0
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                    "clientInfo": {"name": "linux-live", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message):
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def request(self, method, params):
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError(f"brigadier-cu closed stdout during {method}")
            message = json.loads(line)
            if message.get("id") == self.next_id:
                if "error" in message:
                    raise RuntimeError(f"{method}: {message['error']}")
                return message["result"]

    def call(self, tool, args):
        """(is_error, text, structuredContent, image count) of one tools/call."""
        result = self.request("tools/call", {"name": tool, "arguments": args})
        content = result.get("content", [])
        text = "\n".join(item.get("text", "") for item in content if item.get("type") == "text")
        images = sum(1 for item in content if item.get("type") == "image")
        return bool(result.get("isError")), text, result.get("structuredContent"), images

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
