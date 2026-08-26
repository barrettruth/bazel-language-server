#!/usr/bin/env python3
"""Minimal LSP client: drive a language server over stdio and dump responses.

Usage: lspdrive.py <server-cmd...> --root <dir> --script <probes.json>
"""

import json
import subprocess
import sys
import threading
from pathlib import Path
from urllib.parse import quote


class Client:
    def __init__(self, cmd, root):
        self.root = Path(root).resolve()
        self.proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
        )
        self.id = 0
        self.pending = {}
        self.lock = threading.Lock()
        self.cv = threading.Condition(self.lock)
        threading.Thread(target=self._reader, daemon=True).start()
        threading.Thread(target=self._drain_err, daemon=True).start()

    def _drain_err(self):
        for line in self.proc.stderr:
            sys.stderr.write("[server] " + line.decode("utf8", "replace"))

    def _reader(self):
        stream = self.proc.stdout
        while True:
            headers = {}
            while True:
                line = stream.readline()
                if not line:
                    return
                line = line.decode("ascii").strip()
                if not line:
                    break
                k, _, v = line.partition(":")
                headers[k.strip().lower()] = v.strip()
            n = int(headers.get("content-length", 0))
            body = stream.read(n)
            msg = json.loads(body)
            with self.cv:
                if "id" in msg and ("result" in msg or "error" in msg):
                    self.pending[msg["id"]] = msg
                else:
                    self.pending.setdefault("_notifications", []).append(msg)
                self.cv.notify_all()

    def _send(self, payload):
        body = json.dumps(payload).encode("utf8")
        self.proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)

    def notify(self, method, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def request(self, method, params, timeout=25):
        self.id += 1
        rid = self.id
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        with self.cv:
            ok = self.cv.wait_for(lambda: rid in self.pending, timeout=timeout)
            if not ok:
                return {"error": {"message": "TIMEOUT after %ss" % timeout}}
            return self.pending.pop(rid)

    def uri(self, rel):
        # Percent-encode: a path with a space is not a URI, and a server is
        # right to reject one.
        return "file://" + quote(str(self.root / rel))

    def open(self, rel):
        path = self.root / rel
        text = path.read_text()
        lang = "starlark"
        self.notify(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": self.uri(rel),
                    "languageId": lang,
                    "version": 1,
                    "text": text,
                }
            },
        )
        return text


def find(text, needle, occurrence=0):
    """Return (line, character) of the given occurrence of needle."""
    idx = -1
    for _ in range(occurrence + 1):
        idx = text.find(needle, idx + 1)
        if idx < 0:
            raise SystemExit("needle not found: %r" % needle)
    line = text.count("\n", 0, idx)
    col = idx - (text.rfind("\n", 0, idx) + 1)
    return line, col


def main():
    argv = sys.argv[1:]
    root = argv[argv.index("--root") + 1]
    cmd = argv[: argv.index("--root")]
    client = Client(cmd, root)

    init = client.request(
        "initialize",
        {
            "processId": None,
            "rootUri": client.uri(""),
            "workspaceFolders": [{"uri": client.uri(""), "name": "torture"}],
            "capabilities": {
                "textDocument": {
                    "definition": {"linkSupport": True},
                    "hover": {"contentFormat": ["markdown", "plaintext"]},
                    "completion": {
                        "completionItem": {"snippetSupport": False},
                    },
                    "documentSymbol": {"hierarchicalDocumentSymbolSupport": True},
                    "publishDiagnostics": {},
                },
                "workspace": {"symbol": {}, "workspaceFolders": True},
            },
        },
    )
    caps = init.get("result", {}).get("capabilities", {})
    print("=== ADVERTISED CAPABILITIES ===")
    print(json.dumps(sorted(caps.keys()), indent=None))
    print()
    client.notify("initialized", {})

    probes = json.loads(Path(argv[argv.index("--script") + 1]).read_text())
    for probe in probes:
        rel = probe["file"]
        text = client.open(rel)
        line, col = find(text, probe["needle"], probe.get("nth", 0))
        col += probe.get("offset", 0)
        method = probe["method"]
        params = {
            "textDocument": {"uri": client.uri(rel)},
            "position": {"line": line, "character": col},
        }
        if method == "textDocument/references":
            params["context"] = {"includeDeclaration": True}
        if method == "workspace/symbol":
            params = {"query": probe["needle"]}
        resp = client.request(method, params)
        print("### %s  |  %s  @ %s:%d:%d" % (probe["label"], method, rel, line + 1, col + 1))
        out = resp.get("result", resp.get("error"))
        s = json.dumps(out, indent=2)
        if len(s) > 1400:
            s = s[:1400] + "\n  ...[truncated]"
        print(s)
        print()

    client.request("shutdown", {})
    client.notify("exit", {})


if __name__ == "__main__":
    main()
