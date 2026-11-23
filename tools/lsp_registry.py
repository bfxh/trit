#!/usr/bin/env python3
"""Language-server registry and on-demand gateway.

The registry is deliberately data, not a pile of installed plugins.  It carries
128 curated entries from the published nvim-lspconfig config list; only a small
core set has a vetted install recipe.  Everything else is detect-only until the
user explicitly installs it.

    python tools/lsp_registry.py list --json
    python tools/lsp_registry.py detect --json
    python tools/lsp_registry.py plan --lang python --json
    python tools/lsp_registry.py install --id pyright --apply
    python tools/lsp_registry.py probe --id rust_analyzer --root . --json

The probe is a real LSP `initialize` handshake (JSON-RPC framing over stdio),
not a `--version` check.  It never auto-installs; `install --apply` is the only
execution path and only for recipes in `install_allowlist`.
"""

from __future__ import annotations

import json
import os
import pathlib
import queue
import shutil
import subprocess
import sys
import threading
import time
from datetime import datetime

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_REGISTRY = os.path.join(HERE, "lsp_registry.json")


def load_registry(path=""):
    with open(path or DEFAULT_REGISTRY, "r", encoding="utf-8-sig") as fh:
        doc = json.load(fh)
    validate(doc)
    return doc


def validate(doc):
    if not isinstance(doc, dict) or doc.get("schema") not in ("lsp-registry/1", "lsp-registry/0"):
        raise ValueError("not an lsp-registry document")
    servers = doc.get("servers")
    if not isinstance(servers, list) or len(servers) < 100:
        raise ValueError("registry must carry at least 100 servers")
    seen = set()
    for index, entry in enumerate(servers):
        if not isinstance(entry, dict):
            raise ValueError(f"servers[{index}] must be an object")
        sid = entry.get("id")
        if not isinstance(sid, str) or not sid or sid in seen:
            raise ValueError(f"servers[{index}] has a missing/duplicate id")
        seen.add(sid)
        cmd = entry.get("command")
        if not isinstance(cmd, list) or not cmd or not all(isinstance(x, str) for x in cmd):
            raise ValueError(f"{sid}: command must be a non-empty string list")
        fts = entry.get("filetypes")
        if not isinstance(fts, list) or not fts or not all(isinstance(x, str) for x in fts):
            raise ValueError(f"{sid}: filetypes must be a non-empty string list")
        source = entry.get("source")
        if not isinstance(source, str) or not source.startswith("https://"):
            raise ValueError(f"{sid}: source must be an https URL")
        install = entry.get("install")
        if install is not None:
            if not isinstance(install, dict) or install.get("kind") != "command":
                raise ValueError(f"{sid}: install must be a command recipe or null")
            if not isinstance(install.get("command"), list) or not install["command"]:
                raise ValueError(f"{sid}: install.command must be a non-empty list")
            manager = install.get("manager")
            if manager not in doc.get("install_allowlist", []):
                raise ValueError(f"{sid}: install manager is not allowlisted")
    return True


ALIASES = {
    "py": "python", "python3": "python", "js": "javascript", "jsx": "javascriptreact",
    "ts": "typescript", "tsx": "typescriptreact", "c++": "cpp", "cxx": "cpp",
    "c#": "cs", "cs": "cs", "shell": "sh", "bash": "sh", "zsh": "sh",
    "powershell": "ps1", "pwsh": "ps1", "vue": "vue", "md": "markdown",
    "yml": "yaml", "jsonc": "json", "html5": "html", "htm": "html",
}


def lang_matches(entry, lang):
    want = ALIASES.get(lang.lower(), lang.lower())
    return any(ALIASES.get(ft.lower(), ft.lower()) == want for ft in entry.get("filetypes", []))


def command_available(command):
    if not command:
        return False
    exe = command[0]
    if os.path.isabs(exe):
        return os.path.isfile(exe)
    return shutil.which(exe) is not None


def detect(doc):
    rows = []
    for entry in doc["servers"]:
        rows.append({"id": entry["id"], "available": command_available(entry["command"]),
                     "command": entry["command"], "filetypes": entry["filetypes"],
                     "tier": entry.get("tier", "standard"),
                     "install": entry.get("install")})
    return rows


def plan(doc, lang):
    entries = {e["id"]: e for e in doc["servers"]}
    rows = [x for x in detect(doc) if lang_matches(entries[x["id"]], lang)]
    if not rows:
        return {"lang": lang, "available": [], "missing": [], "verdict": "no-match"}
    available = [x for x in rows if x["available"]]
    missing = [x for x in rows if not x["available"]]
    return {"lang": lang, "available": available, "missing": missing,
            "verdict": "available" if available else "missing"}


class LspSession:
    """Minimal JSON-RPC/stdio LSP session with a hard deadline."""

    def __init__(self, command, cwd, timeout_s=20):
        self.timeout_s = float(timeout_s)
        self.proc = subprocess.Popen(command, cwd=cwd, stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                     bufsize=0)
        self.q = queue.Queue()
        self.stderr = []
        self._stop = False
        self.reader = threading.Thread(target=self._read_stdout, daemon=True)
        self.err_reader = threading.Thread(target=self._read_stderr, daemon=True)
        self.reader.start()
        self.err_reader.start()

    def _read_stdout(self):
        try:
            while not self._stop:
                headers = {}
                while True:
                    line = self.proc.stdout.readline()
                    if not line:
                        self.q.put(None)
                        return
                    if line in (b"\r\n", b"\n"):
                        break
                    text = line.decode("ascii", "replace").strip()
                    if ":" in text:
                        k, v = text.split(":", 1)
                        headers[k.lower()] = v.strip()
                length = int(headers.get("content-length", "0") or 0)
                body = self.proc.stdout.read(length) if length else b""
                try:
                    self.q.put(json.loads(body.decode("utf-8", "replace")))
                except ValueError:
                    self.q.put({"__bad_frame__": body[:200].decode("utf-8", "replace")})
        except Exception as e:  # reader must never hang the caller
            self.q.put({"__reader_error__": str(e)})

    def _read_stderr(self):
        try:
            while not self._stop:
                line = self.proc.stderr.readline()
                if not line:
                    return
                if len(self.stderr) < 200:
                    self.stderr.append(line.decode("utf-8", "replace").rstrip())
        except Exception:
            return

    def send(self, obj):
        body = json.dumps(obj, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        frame = f"Content-Length: {len(body)}\r\n\r\n".encode("ascii") + body
        self.proc.stdin.write(frame)
        self.proc.stdin.flush()

    def request(self, rid, method, params, timeout_s=None):
        deadline = time.time() + float(timeout_s or self.timeout_s)
        self.send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        while time.time() < deadline:
            try:
                msg = self.q.get(timeout=max(0.01, deadline - time.time()))
            except queue.Empty:
                raise TimeoutError(f"no response to {method} within {deadline - time.time():.1f}s")
            if msg is None:
                raise RuntimeError("server closed stdout")
            if isinstance(msg, dict) and "__reader_error__" in msg:
                raise RuntimeError(msg["__reader_error__"])
            if isinstance(msg, dict) and "__bad_frame__" in msg:
                raise RuntimeError(f"bad LSP frame: {msg['__bad_frame__']}")
            if isinstance(msg, dict) and msg.get("id") == rid:
                return msg
            # notifications/other responses are not the answer; keep reading
        raise TimeoutError(f"no response to {method}")

    def notify(self, method, params=None):
        self.send({"jsonrpc": "2.0", "method": method, "params": params or {}})

    def close(self):
        try:
            if self.proc.poll() is None:
                try:
                    self.request(99, "shutdown", {}, timeout_s=2)
                except Exception:
                    pass
                try:
                    self.notify("exit", {})
                except Exception:
                    pass
                try:
                    self.proc.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    self.proc.terminate()
                    try:
                        self.proc.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        self.proc.kill()
        finally:
            self._stop = True
            for stream in (self.proc.stdin, self.proc.stdout, self.proc.stderr):
                try:
                    stream.close()
                except Exception:
                    pass


def probe(entry, root, timeout_s=20):
    t0 = time.time()
    if not command_available(entry["command"]):
        return {"id": entry["id"], "available": False, "seconds": 0,
                "error": f"command not found: {entry['command'][0]}"}
    session = LspSession(entry["command"], root, timeout_s)
    try:
        uri = pathlib.Path(os.path.abspath(root)).as_uri()
        resp = session.request(1, "initialize", {
            "processId": os.getpid(),
            "rootUri": uri,
            "capabilities": {},
            "workspaceFolders": [{"uri": uri, "name": os.path.basename(os.path.abspath(root))}],
        })
        session.notify("initialized", {})
        return {"id": entry["id"], "available": True,
                "seconds": round(time.time() - t0, 2),
                "capabilities": (resp.get("result") or {}).get("capabilities", {}),
                "server_info": (resp.get("result") or {}).get("serverInfo", {}),
                "stderr": session.stderr[-5:]}
    except Exception as e:
        return {"id": entry["id"], "available": True, "seconds": round(time.time() - t0, 2),
                "error": str(e), "stderr": session.stderr[-5:]}
    finally:
        session.close()


def install(entry, doc, apply=False):
    if entry.get("install") is None:
        return {"id": entry["id"], "status": "manual", "reason": "no vetted install recipe"}
    if entry["id"] not in doc.get("install_allowlist", []) and entry["install"].get("manager") not in doc.get("install_allowlist", []):
        return {"id": entry["id"], "status": "refused", "reason": "recipe is not in install_allowlist"}
    command = entry["install"]["command"]
    if not apply:
        return {"id": entry["id"], "status": "planned", "command": command}
    if shutil.which(command[0]) is None:
        return {"id": entry["id"], "status": "refused", "reason": f"manager not found: {command[0]}"}
    r = subprocess.run(command, capture_output=True, text=True, encoding="utf-8",
                       errors="replace", timeout=600)
    return {"id": entry["id"], "status": "ok" if r.returncode == 0 else "failed",
            "exit": r.returncode, "command": command,
            "output": ((r.stdout or "") + ("\n" + r.stderr if r.stderr else ""))[-1200:]}


def self_test():
    checks = 0

    def ok(cond, msg):
        nonlocal checks
        checks += 1
        if not cond:
            raise AssertionError(msg)

    doc = load_registry()
    ok(len(doc["servers"]) >= 100, f"registry has 100+ entries ({len(doc['servers'])})")
    ok(len({x["id"] for x in doc["servers"]}) == len(doc["servers"]), "ids unique")
    ok(all(x["source"].startswith("https://") for x in doc["servers"]), "sources are https")
    ok(all(x.get("install") is None or x["install"].get("manager") in doc["install_allowlist"]
           for x in doc["servers"]), "install recipes allowlisted")
    ok(any(x["id"] == "rust_analyzer" for x in doc["servers"]), "rust core present")
    ok(plan(doc, "python")["available"] != [] or plan(doc, "python")["missing"] != [], "python plan has candidates")
    ok(plan(doc, "not-a-language")["verdict"] == "no-match", "unknown language no-match")
    rows = detect(doc)
    ok(len(rows) == len(doc["servers"]), "detect covers every entry")
    ok(any(x["available"] for x in rows), "at least one local server is available")

    # Refuse an install recipe not in the allowlist.
    fake = {"id": "fake", "install": {"kind": "command", "command": ["echo", "x"], "manager": "nope"}}
    out = install(fake, doc, apply=False)
    ok(out["status"] == "refused", "unallowlisted install refused")

    # Real JSON-RPC handshake against an in-process fake LSP server.
    import tempfile
    fake_server = r'''
import sys, json
while True:
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line in (b"\r\n", b"\n"):
            break
        if b":" in line:
            k, v = line.split(b":", 1)
            headers[k.strip().lower()] = v.strip()
    n = int(headers.get(b"content-length", b"0") or b"0")
    body = sys.stdin.buffer.read(n)
    msg = json.loads(body.decode("utf-8"))
    if msg.get("method") == "initialize":
        out = {"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": {"hoverProvider": True}, "serverInfo": {"name": "fake"}}}
    elif msg.get("method") == "shutdown":
        out = {"jsonrpc": "2.0", "id": msg.get("id"), "result": None}
    elif msg.get("method") == "exit":
        sys.exit(0)
    else:
        continue
    raw = json.dumps(out).encode("utf-8")
    sys.stdout.buffer.write(b"Content-Length: " + str(len(raw)).encode() + b"\r\n\r\n" + raw)
    sys.stdout.buffer.flush()
'''
    with tempfile.TemporaryDirectory() as td:
        path = os.path.join(td, "fake_lsp.py")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(fake_server)
        entry = {"id": "fake", "command": [sys.executable, path]}
        out = probe(entry, td, timeout_s=10)
        ok(out["available"] and out.get("capabilities", {}).get("hoverProvider") is True, "fake LSP initialize")
        ok(out.get("server_info", {}).get("name") == "fake", "fake LSP server info")

    # A missing command is a clean unavailable, not a crash.
    out = probe({"id": "missing", "command": ["definitely-not-a-real-lsp-xyz"]}, ".", 2)
    ok(out["available"] is False and "command not found" in out["error"], "missing command handled")

    print(f"lsp-registry self-test: {checks}/{checks}")
    return 0


def parse(argv):
    out = {"cmd": "", "registry": "", "json": False, "lang": "", "id": "",
           "apply": False, "root": ".", "timeout": 20, "self_test": False}
    if not argv:
        return out
    out["cmd"] = argv[0]
    if out["cmd"] == "--self-test":
        out["self_test"] = True
        out["cmd"] = ""
    i = 1
    while i < len(argv):
        a = argv[i]
        if a in ("--json",):
            out["json"] = True
            i += 1
        elif a in ("--apply",):
            out["apply"] = True
            i += 1
        elif a in ("--self-test",):
            out["self_test"] = True
            i += 1
        elif a in ("--registry", "-Registry"):
            out["registry"] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
        elif a in ("--lang", "-Lang"):
            out["lang"] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
        elif a in ("--id", "-Id"):
            out["id"] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
        elif a in ("--root", "-Root"):
            out["root"] = argv[i + 1] if i + 1 < len(argv) else "."
            i += 2
        elif a in ("--timeout", "-Timeout"):
            try:
                out["timeout"] = float(argv[i + 1])
            except (IndexError, ValueError):
                pass
            i += 2
        elif a in ("--help", "-h"):
            out["help"] = True
            i += 1
        else:
            i += 1
    return out


def main(argv):
    a = parse(argv)
    if a.get("self_test"):
        return self_test()
    if a.get("help") or not a["cmd"]:
        print("lsp-registry: list|detect|plan|install|probe [--json] [--registry FILE] "
              "[--lang LANG] [--id ID] [--apply] [--root DIR] [--timeout S]")
        return 0
    try:
        doc = load_registry(a["registry"])
    except (OSError, ValueError) as e:
        print(f"lsp-registry: {e}")
        return 2
    if a["cmd"] == "list":
        rows = doc["servers"]
    elif a["cmd"] == "detect":
        rows = detect(doc)
    elif a["cmd"] == "plan":
        if not a["lang"]:
            print("lsp-registry: plan needs --lang")
            return 2
        rows = plan(doc, a["lang"])
    elif a["cmd"] == "install":
        entry = next((x for x in doc["servers"] if x["id"] == a["id"]), None)
        if not entry:
            print(f"lsp-registry: unknown id {a['id']}")
            return 2
        rows = install(entry, doc, a["apply"])
    elif a["cmd"] == "probe":
        entry = next((x for x in doc["servers"] if x["id"] == a["id"]), None)
        if not entry:
            print(f"lsp-registry: unknown id {a['id']}")
            return 2
        rows = probe(entry, a["root"], a["timeout"])
    else:
        print(f"lsp-registry: unknown command {a['cmd']}")
        return 2
    if a["cmd"] == "plan":
        if a["json"]:
            print(json.dumps(rows, ensure_ascii=True, sort_keys=True))
        else:
            print(f"lang={rows['lang']} verdict={rows['verdict']} "
                  f"available={[x['id'] for x in rows['available']]} "
                  f"missing={[x['id'] for x in rows['missing']]}")
        return 0
    if a["json"]:
        print(json.dumps(rows, ensure_ascii=True, sort_keys=True))
    elif isinstance(rows, dict):
        for k, v in rows.items():
            print(f"{k}: {v}")
    else:
        for row in rows:
            if isinstance(row, dict) and "available" in row:
                print(f"{row['id']:24} {'YES' if row['available'] else 'no ':3} {' '.join(row['command'])}")
            else:
                print(row)
    if a["cmd"] == "probe" and isinstance(rows, dict) and not rows.get("available"):
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
